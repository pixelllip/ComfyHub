#include "clipboard_channel.h"

#include <flutter/method_channel.h>
#include <flutter/standard_method_codec.h>

#include <windows.h>
#include <shellapi.h>  // DragQueryFileW / HDROP

#include <cstring>
#include <memory>
#include <string>
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
