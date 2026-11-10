#include "clipboard_channel.h"

#include <flutter/method_channel.h>
#include <flutter/standard_method_codec.h>

#include <windows.h>
#include <shellapi.h>  // DragQueryFileW / HDROP

#include <cstdint>
#include <cstring>
#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace {

/// `DROPEFFECT_COPY`（值就是 1）。
///
/// 不引 `objidl.h` 取这个名字：runner 这一侧开着 `WIN32_LEAN_AND_MEAN` 一类的宏时，
/// `windows.h` 不一定把 ole 那一套头带进来，而为一个常量拉一整个 OLE 头不划算。
/// 规范值见 <https://learn.microsoft.com/windows/win32/com/dropeffect-constants>。
constexpr DWORD kDropEffectCopy = 1;

/// `DROPFILES`（`CF_HDROP` 负载的头）。
///
/// 系统头里它落在 `shlobj.h`（一个很重的 shell 头，而且这一侧开着
/// `WIN32_LEAN_AND_MEAN`）。为一个 **20 字节、布局稳定的公开 ABI** 结构拉那一整个头
/// 不划算，所以这里按规范声明一份，并用 `static_assert` 把尺寸钉死 ——
/// 布局真要是变了，**编译就会红**，而不是运行期静默写错偏移。
struct DropFilesHeader {
  DWORD pFiles;  ///< 文件列表相对本结构起点的字节偏移
  POINT pt;      ///< 拖放落点（我们不用，清零）
  BOOL fNC;      ///< 是否落在非客户区（我们不用，清零）
  BOOL fWide;    ///< 列表是不是宽字符（**必须是 TRUE**）
};
static_assert(sizeof(DropFilesHeader) == 20,
              "DROPFILES 的布局变了：CF_HDROP 的负载偏移要跟着改");

// UTF-8 → UTF-16。自己写而不是用引擎里那些辅助函数：那几个的名字/命名空间
// 在不同版本的 embedder 头文件里变过，调不通时编译错误会指到这里来。
std::wstring Widen(const std::string& text) {
  if (text.empty()) return std::wstring();
  const int length = ::MultiByteToWideChar(CP_UTF8, 0, text.data(),
                                           static_cast<int>(text.size()),
                                           nullptr, 0);
  if (length <= 0) return std::wstring();
  std::wstring wide(static_cast<size_t>(length), L'\0');
  ::MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                        wide.data(), length);
  return wide;
}

std::string Narrow(const std::wstring& text) {
  if (text.empty()) return std::string();
  const int length = ::WideCharToMultiByte(CP_UTF8, 0, text.data(),
                                           static_cast<int>(text.size()),
                                           nullptr, 0, nullptr, nullptr);
  if (length <= 0) return std::string();
  std::string narrow(static_cast<size_t>(length), '\0');
  ::WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                        narrow.data(), length, nullptr, nullptr);
  return narrow;
}

// 打开剪贴板。**要重试**：别的进程（输入法、截图工具、另一个资源管理器窗口）
// 随时可能正占着它，而 `OpenClipboard` 失败只回 FALSE、不排队。
bool OpenClipboardWithRetry() {
  for (int attempt = 0; attempt < 8; ++attempt) {
    if (::OpenClipboard(nullptr)) return true;
    ::Sleep(15);
  }
  return false;
}

bool HasFiles() { return ::IsClipboardFormatAvailable(CF_HDROP) != FALSE; }

std::vector<std::wstring> ReadFiles() {
  std::vector<std::wstring> paths;
  if (!HasFiles()) return paths;
  if (!OpenClipboardWithRetry()) return paths;

  if (HANDLE handle = ::GetClipboardData(CF_HDROP)) {
    auto drop = static_cast<HDROP>(handle);
    const UINT count = ::DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
    for (UINT index = 0; index < count; ++index) {
      const UINT length = ::DragQueryFileW(drop, index, nullptr, 0);
      if (length == 0) continue;
      std::wstring buffer(static_cast<size_t>(length) + 1, L'\0');
      const UINT written =
          ::DragQueryFileW(drop, index, buffer.data(), length + 1);
      if (written == 0) continue;
      buffer.resize(written);
      paths.push_back(std::move(buffer));
    }
  }
  ::CloseClipboard();
  return paths;
}

// 把一段"整块全局内存"的剪贴板负载原样拷出来（`CF_DIB` / `PNG` 都是这个形状）。
// **调用时剪贴板必须已经打开**；失败回空表。
std::vector<uint8_t> GlobalBytes(UINT format) {
  std::vector<uint8_t> out;
  HANDLE handle = ::GetClipboardData(format);
  if (handle == nullptr) return out;
  const SIZE_T size = ::GlobalSize(handle);
  if (size == 0) return out;
  const void* source = ::GlobalLock(handle);
  if (source == nullptr) return out;
  const auto* bytes = static_cast<const uint8_t*>(source);
  out.assign(bytes, bytes + size);
  ::GlobalUnlock(handle);
  return out;
}

uint32_t ReadU32(const uint8_t* p) {
  return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
         (static_cast<uint32_t>(p[2]) << 16) |
         (static_cast<uint32_t>(p[3]) << 24);
}

uint16_t ReadU16(const uint8_t* p) {
  return static_cast<uint16_t>(static_cast<uint16_t>(p[0]) |
                               (static_cast<uint16_t>(p[1]) << 8));
}

/// `PNG` 注册格式名。
///
/// Chromium（浏览器里复制图片）以及部分截图工具在 `CF_DIB` 之外**还会再放一份
/// 已编码的 PNG**。有它就用它：一是不用自己转码、像素与原图逐字节一致，
/// 二是 32bpp DIB 里那个"未使用"的 alpha 字节不会把图弄成全透明。
UINT PngClipboardFormat() {
  static const UINT format = ::RegisterClipboardFormatW(L"PNG");
  return format;
}

// `BITMAPFILEHEADER`（14 字节）。系统头里它在 `wingdi.h`，但那一份依赖
// `WIN32_LEAN_AND_MEAN` 之类的宏开关才可见；这里按规范声明并用 `static_assert`
// 钉住尺寸 —— 与下面 `DropFilesHeader` 是同一个理由。
#pragma pack(push, 1)
struct BitmapFileHeader {
  WORD type;       ///< 'BM'
  DWORD size;      ///< 整个 .bmp 文件的字节数
  WORD reserved1;  ///< 0
  WORD reserved2;  ///< 0
  DWORD off_bits;  ///< 像素数据的起点（相对文件开头）
};
#pragma pack(pop)
static_assert(sizeof(BitmapFileHeader) == 14,
              "BITMAPFILEHEADER 必须是 14 字节：偏移写错就是一张花屏");

/// 把一段裸 DIB（`CF_DIB` / `CF_DIBV5` 的负载）包成 `.bmp` 文件的字节。
///
/// 为什么必须包这一层：Flutter 的解码器（Skia）认的是**带文件头的 BMP**，
/// 裸 DIB 没有 `bfOffBits` 那一层。而像素起点要按头里的位深 / 压缩方式 / 调色板
/// 算出来（8bpp 有调色板、`BI_BITFIELDS` 多 12 字节掩码），算错不会报错，只会花屏。
///
/// [force_opaque]：把 32bpp `BI_RGB` 的 alpha 字节强制置 255。按规范那一位
/// **未使用**，但截图工具给出来的常常是全 0 —— 照原样交给解码器会得到一张
/// 全透明的图（表现为"粘贴后没有内容"）。
std::vector<uint8_t> DibToBmp(const std::vector<uint8_t>& dib,
                              bool force_opaque) {
  std::vector<uint8_t> bmp;
  if (dib.size() < 40) return bmp;  // 连 BITMAPINFOHEADER 都不够

  const uint32_t header = ReadU32(dib.data());
  if (header < 40 || header > dib.size()) return bmp;
  const uint32_t bpp = ReadU16(dib.data() + 14);
  const uint32_t compression = ReadU32(dib.data() + 16);
  const uint32_t colors_used = ReadU32(dib.data() + 32);

  size_t extra = 0;    // BI_BITFIELDS 的掩码（仅 40 字节的老头才有）
  size_t palette = 0;  // 8bpp 及以下的调色板
  if (header == 40) {
    if (compression == BI_BITFIELDS && (bpp == 16 || bpp == 32)) {
      extra = 12;
    }
    if (bpp > 0 && bpp <= 8) {
      palette = static_cast<size_t>(colors_used != 0 ? colors_used
                                                     : (1u << bpp)) * 4;
    }
  }

  // 像素起点（相对 .bmp 文件开头）
  const size_t off_bits = 14 + header + extra + palette;
  if (off_bits > 14 + dib.size()) return bmp;  // 头里说的是"后面还有数据"却没有

  std::vector<uint8_t> payload = dib;
  if (force_opaque && bpp == 32 && compression == BI_RGB) {
    for (size_t i = (off_bits - 14) + 3; i < payload.size(); i += 4) {
      payload[i] = 0xFF;
    }
  }

  BitmapFileHeader file_header{};
  file_header.type = 0x4D42;  // 'BM'
  file_header.size = static_cast<DWORD>(14 + payload.size());
  file_header.off_bits = static_cast<DWORD>(off_bits);

  bmp.resize(file_header.size);
  std::memcpy(bmp.data(), &file_header, sizeof(file_header));
  std::memcpy(bmp.data() + sizeof(file_header), payload.data(), payload.size());
  return bmp;
}

/// `CF_BITMAP`（`HBITMAP`）→ 32bpp DIB（`GetDIBits` 自己填像素）。
/// **调用时剪贴板必须已经打开**（位图句柄归剪贴板所有）。
std::vector<uint8_t> BitmapToDib(HBITMAP bitmap) {
  std::vector<uint8_t> dib;
  if (bitmap == nullptr) return dib;
  BITMAP info{};
  if (::GetObjectW(bitmap, sizeof(info), &info) != sizeof(info)) return dib;
  if (info.bmWidth <= 0 || info.bmHeight <= 0) return dib;

  BITMAPINFOHEADER header{};
  header.biSize = sizeof(header);
  header.biWidth = info.bmWidth;
  header.biHeight = info.bmHeight;  // 正数 = 自下而上，正是 BMP 的约定
  header.biPlanes = 1;
  header.biBitCount = 32;
  header.biCompression = BI_RGB;
  header.biSizeImage =
      static_cast<DWORD>(info.bmWidth) * static_cast<DWORD>(info.bmHeight) * 4;

  std::vector<uint8_t> pixels(header.biSizeImage);
  HDC dc = ::GetDC(nullptr);
  if (dc == nullptr) return dib;
  BITMAPINFO dto{};
  dto.bmiHeader = header;
  const int lines = ::GetDIBits(dc, bitmap, 0, static_cast<UINT>(info.bmHeight),
                                pixels.data(), &dto, DIB_RGB_COLORS);
  ::ReleaseDC(nullptr, dc);
  if (lines != info.bmHeight) return dib;

  dib.resize(sizeof(header) + pixels.size());
  std::memcpy(dib.data(), &header, sizeof(header));
  std::memcpy(dib.data() + sizeof(header), pixels.data(), pixels.size());
  return dib;
}

/// 剪贴板里的一幅图（读不到时 `ok == false`）。
struct ClipboardImage {
  bool ok = false;
  bool png = false;  ///< true = [bytes] 已经是 PNG；false = 是 .bmp
  std::vector<uint8_t> bytes;
};

bool HasImage() {
  const UINT png = PngClipboardFormat();
  return ::IsClipboardFormatAvailable(CF_DIB) != FALSE ||
         ::IsClipboardFormatAvailable(CF_DIBV5) != FALSE ||
         ::IsClipboardFormatAvailable(CF_BITMAP) != FALSE ||
         (png != 0 && ::IsClipboardFormatAvailable(png) != FALSE);
}

/// 读剪贴板里的位图。顺序：PNG（保真）→ CF_DIBV5 → CF_DIB → CF_BITMAP。
///
/// V5 优先于 V4/DIB：它带 alpha 掩码，透明背景的图不会在转换时丢掉。
///
/// ⚠ **`IsClipboardFormatAvailable` 只说明"它声称有这个格式"，不等于拿得到数据**：
/// .NET `DataObject` / OLE 延迟渲染会把 `CF_DIBV5` / `CF_BITMAP` 列进格式表，
/// 而 `GetClipboardData` 回 NULL（实测：PowerShell 里 `Clipboard.SetDataObject`
/// 之后 `CF_DIBV5 available=1 handle=0`）。所以这里**必须逐个格式试**，
/// 读到真数据才算数 —— 一旦"选中" V5 就放弃 DIB，这类剪贴板会一张图都读不出来。
ClipboardImage ReadImage() {
  ClipboardImage image;
  if (!HasImage()) return image;
  if (!OpenClipboardWithRetry()) return image;

  const UINT png_format = PngClipboardFormat();
  if (png_format != 0 && ::IsClipboardFormatAvailable(png_format)) {
    auto bytes = GlobalBytes(png_format);
    // PNG 的签名是 8 字节，短于它的一定不是图
    if (bytes.size() > 8 && bytes[0] == 0x89 && bytes[1] == 0x50) {
      image.ok = true;
      image.png = true;
      image.bytes = std::move(bytes);
    }
  }

  if (!image.ok) {
    const UINT kDibFormats[] = {CF_DIBV5, CF_DIB};
    for (UINT format : kDibFormats) {
      if (!::IsClipboardFormatAvailable(format)) continue;
      auto bmp = DibToBmp(GlobalBytes(format), /*force_opaque=*/true);
      if (!bmp.empty()) {
        image.ok = true;
        image.png = false;
        image.bytes = std::move(bmp);
        break;
      }
    }
  }

  if (!image.ok && ::IsClipboardFormatAvailable(CF_BITMAP)) {
    auto handle = ::GetClipboardData(CF_BITMAP);
    if (handle != nullptr) {
      auto bmp = DibToBmp(BitmapToDib(static_cast<HBITMAP>(handle)),
                          /*force_opaque=*/true);
      if (!bmp.empty()) {
        image.ok = true;
        image.png = false;
        image.bytes = std::move(bmp);
      }
    }
  }

  ::CloseClipboard();
  return image;
}

// 把一段字节写进一块**可移动全局内存**；失败回 nullptr。
// 调用方负责：交给 `SetClipboardData`（成功即所有权转移）或自己 `GlobalFree`。
HGLOBAL MakeGlobal(const void* data, size_t bytes) {
  HGLOBAL block = ::GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
  if (block == nullptr) return nullptr;
  void* target = ::GlobalLock(block);
  if (target == nullptr) {
    ::GlobalFree(block);
    return nullptr;
  }
  std::memcpy(target, data, bytes);
  ::GlobalUnlock(block);
  return block;
}

bool WriteFiles(const std::vector<std::wstring>& paths) {
  if (paths.empty()) return false;

  // ---- CF_HDROP 的负载：DROPFILES 头 + 一串"各自以 NUL 结尾"的宽路径 + 末尾再一个 NUL
  size_t chars = 1;  // 列表末尾那个 NUL
  for (const auto& path : paths) chars += path.size() + 1;
  const size_t bytes = sizeof(DropFilesHeader) + chars * sizeof(wchar_t);

  HGLOBAL drop_block = ::GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
  if (drop_block == nullptr) return false;
  {
    auto* drop = static_cast<DropFilesHeader*>(::GlobalLock(drop_block));
    if (drop == nullptr) {
      ::GlobalFree(drop_block);
      return false;
    }
    drop->pFiles = static_cast<DWORD>(sizeof(DropFilesHeader));
    drop->fWide = TRUE;  // 必须是 TRUE：否则系统按 ANSI 解释我们写的宽字符
    auto* cursor = reinterpret_cast<wchar_t*>(
        reinterpret_cast<unsigned char*>(drop) + sizeof(DropFilesHeader));
    for (const auto& path : paths) {
      std::memcpy(cursor, path.c_str(), path.size() * sizeof(wchar_t));
      cursor += path.size();
      *cursor++ = L'\0';
    }
    *cursor = L'\0';
    ::GlobalUnlock(drop_block);
  }

  // ---- 兜底：纯文本路径。粘到不支持 CF_HDROP 的地方（记事本、聊天框）至少得到路径
  std::wstring joined;
  for (size_t index = 0; index < paths.size(); ++index) {
    if (index > 0) joined += L"\r\n";
    joined += paths[index];
  }
  HGLOBAL text_block =
      MakeGlobal(joined.c_str(), (joined.size() + 1) * sizeof(wchar_t));

  // ---- "首选拖放效果 = 复制"。不写这一项，资源管理器会把这次粘贴当**剪切**，
  //      源文件会被搬走 —— 这是最容易被忽略、后果最严重的一项。
  static UINT drop_effect_format =
      ::RegisterClipboardFormatW(L"Preferred DropEffect");
  const DWORD effect = kDropEffectCopy;
  HGLOBAL effect_block = MakeGlobal(&effect, sizeof(effect));

  if (!OpenClipboardWithRetry()) {
    ::GlobalFree(drop_block);
    if (text_block) ::GlobalFree(text_block);
    if (effect_block) ::GlobalFree(effect_block);
    return false;
  }

  ::EmptyClipboard();
  // ⚠ 顺序与所有权：`EmptyClipboard` 先清，然后逐项 `SetClipboardData`。
  // **成功交给系统之后绝不能再 GlobalFree**（那是双重释放）。
  const bool ok = ::SetClipboardData(CF_HDROP, drop_block) != nullptr;
  if (ok) {
    if (text_block) ::SetClipboardData(CF_UNICODETEXT, text_block);
    if (effect_block && drop_effect_format != 0) {
      ::SetClipboardData(drop_effect_format, effect_block);
    }
  }
  ::CloseClipboard();

  if (!ok) {
    // CF_HDROP 没交出去：三块内存都还在我们手上
    ::GlobalFree(drop_block);
    if (text_block) ::GlobalFree(text_block);
    if (effect_block) ::GlobalFree(effect_block);
  }
  return ok;
}

}  // namespace

void RegisterClipboardChannel(flutter::BinaryMessenger* messenger) {
  auto channel =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          messenger, "comfyhub/clipboard",
          &flutter::StandardMethodCodec::GetInstance());

  channel->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) {
        const std::string& method = call.method_name();

        if (method == "hasFiles") {
          result->Success(flutter::EncodableValue(HasFiles()));
          return;
        }

        if (method == "readFiles") {
          flutter::EncodableList list;
          for (const auto& path : ReadFiles()) {
            list.push_back(flutter::EncodableValue(Narrow(path)));
          }
          result->Success(flutter::EncodableValue(list));
          return;
        }

        if (method == "hasImage") {
          result->Success(flutter::EncodableValue(HasImage()));
          return;
        }

        if (method == "readImage") {
          const auto image = ReadImage();
          if (!image.ok) {
            // **没有就是没有**：回 null，让 Dart 那边退回"粘文本"那条路。
            result->Success(flutter::EncodableValue());
            return;
          }
          flutter::EncodableMap payload;
          payload[flutter::EncodableValue("format")] =
              flutter::EncodableValue(image.png ? "png" : "bmp");
          payload[flutter::EncodableValue("bytes")] =
              flutter::EncodableValue(image.bytes);
          result->Success(flutter::EncodableValue(payload));
          return;
        }

        if (method == "writeFiles") {
          std::vector<std::wstring> paths;
          if (const auto* args =
                  std::get_if<flutter::EncodableList>(call.arguments())) {
            for (const auto& item : *args) {
              if (const auto* text = std::get_if<std::string>(&item)) {
                paths.push_back(Widen(*text));
              }
            }
          }
          result->Success(flutter::EncodableValue(WriteFiles(paths)));
          return;
        }

        result->NotImplemented();
      });

  // 通道要活到进程结束。runner 只在启动时注册一次，这里**故意让它泄漏一个**
  // （比塞一个进程级单例简单，也和 Flutter 官方插件的写法一致）。
  channel.release();
}
