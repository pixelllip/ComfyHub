#include "file_drop_channel.h"

#include <flutter/standard_method_codec.h>

#include <shellapi.h>  // DragAcceptFiles / DragQueryFileW / DragFinish / HDROP

#include <map>
#include <string>
#include <utility>
#include <vector>

namespace {

// UTF-16 → UTF-8。
//
// **中文路径全靠它**：`flutter::EncodableValue` 只收 `std::string`（标准编解码器
// 按 UTF-8 写字符串），直接把宽字符塞进去会得到乱码或者被截断。
//
// 与 `clipboard_channel.cpp` 里那份是**刻意的两份**（都放在匿名命名空间、文件内
// 可见）：一边要 `Widen`、一边只要 `Narrow`，为这两个小函数开一个共享头不划算，
// 而且自建辅助函数能避开引擎头文件里那些改过名字的转换工具（编译错误会直接指到
// 这里来）。
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

// `WM_DROPFILES` 交下来的那个 `HDROP` **必须** `DragFinish`。
//
// 它是 Shell 为这一次投递分配的一块（可移动）全局内存，不是句柄表里那种"关了
// 窗口就跟着走"的东西：不释放就是**每次拖放都漏一份**，用户拖几十次之后进程里
// 就攒着一堆死内存。用 RAII 兜住 —— 下面任何一条提前 return（取不到路径、
// channel 还没注册…）都不会漏掉这一次。
class DropHandle {
 public:
  explicit DropHandle(HDROP drop) : drop_(drop) {}
  ~DropHandle() {
    if (drop_ != nullptr) {
      ::DragFinish(drop_);
    }
  }
  DropHandle(const DropHandle&) = delete;
  DropHandle& operator=(const DropHandle&) = delete;

 private:
  HDROP drop_;
};

// 取这一批拖进来的路径（UTF-8）。
//
// `DragQueryFileW` 要**两趟**：`0xFFFFFFFF` 那一趟只回**个数**，然后按下标逐个
// 取（第二趟先问长度、再填缓冲）。这是它的用法约定 —— 一趟拿不到名字本身。
std::vector<std::string> ReadDroppedPaths(HDROP drop) {
  std::vector<std::string> paths;
  const UINT count = ::DragQueryFileW(drop, 0xFFFFFFFF, nullptr, 0);
  if (count == 0) return paths;
  paths.reserve(count);
  for (UINT index = 0; index < count; ++index) {
    const UINT length = ::DragQueryFileW(drop, index, nullptr, 0);
    if (length == 0) continue;
    std::wstring buffer(static_cast<size_t>(length) + 1, L'\0');
    const UINT written =
        ::DragQueryFileW(drop, index, buffer.data(), length + 1);
    if (written == 0) continue;
    buffer.resize(written);  // 回的是"不含结尾 NUL 的字符数"
    std::string path = Narrow(buffer);
    if (!path.empty()) paths.push_back(std::move(path));
  }
  return paths;
}

}  // namespace

FileDropChannel& FileDropChannel::instance() {
  // 进程级唯一。刻意不做成"可销毁的"：拖放随时可能来，而 runner 只在启动时
  // 注册一次（和 `clipboard_channel.cpp` 里让通道活到进程结束是同一个理由）。
  static FileDropChannel channel;
  return channel;
}

void FileDropChannel::Register(flutter::BinaryMessenger* messenger, HWND hwnd) {
  if (messenger != nullptr && !channel_) {
    channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
        messenger, "comfyhub/file_drop",
        &flutter::StandardMethodCodec::GetInstance());
    // Dart → native 的方法**一个都没有**：这条通道只往 Dart 推，所以不设
    // SetMethodCallHandler。哪天真有 Dart 侧调过来，标准回复就是 NotImplemented
    // —— 比在这里塞一个空实现更诚实（空实现会让人以为"调通了"）。
  }

  if (hwnd != nullptr) {
    hwnd_ = hwnd;
    // 窗口与通道**都就绪之后**才开（见头注释）：先开的话，用户在注册完成前拖进来
    // 的那批会因为 Dart 侧还没有接收者而丢掉。
    ::DragAcceptFiles(hwnd, TRUE);
  }
}

void FileDropChannel::HandleDropFiles(WPARAM wparam) {
  auto* drop = reinterpret_cast<HDROP>(wparam);
  if (drop == nullptr) return;
  DropHandle handle(drop);  // 出作用域一定 DragFinish（句柄/内存不泄漏）

  if (channel_ == nullptr) {
    // 还没注册（或注册失败）：**静默丢掉这一批**。不弹窗、不崩 ——
    // "这次拖不进来"已是既成事实，报错也帮不上忙，反而会把聊天那条链路搅进来。
    return;
  }

  std::vector<std::string> paths = ReadDroppedPaths(drop);
  if (paths.empty()) {
    // 空的一批不往 Dart 推：监听方没必要为"一次没内容的拖放"白跑一趟
    // （Dart 侧同样有一条"空列表不推"的兜底）。
    return;
  }

  flutter::EncodableList list;
  list.reserve(paths.size());
  for (const auto& path : paths) {
    list.push_back(flutter::EncodableValue(path));
  }

  flutter::EncodableMap arguments;
  arguments[flutter::EncodableValue("paths")] =
      flutter::EncodableValue(std::move(list));

  // 不传 result = 不等回执：Dart 侧有没有人在听都不关原生的事（没人听时引擎会
  // 把这条丢掉，`lib/core/file_drop.dart` 的类注释里写了这件事）。
  channel_->InvokeMethod("droppedFiles",
                         std::make_unique<flutter::EncodableValue>(
                             std::move(arguments)));
}

void FileDropChannel::Revoke() {
  if (hwnd_ != nullptr) {
    ::DragAcceptFiles(hwnd_, FALSE);
    hwnd_ = nullptr;
  }
}
