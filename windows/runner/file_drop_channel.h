#ifndef RUNNER_FILE_DROP_CHANNEL_H_
#define RUNNER_FILE_DROP_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>

#include <windows.h>

#include <memory>

// 把"从资源管理器拖进窗口的文件"送到 Dart：`comfyhub/file_drop` 通道。
//
// # 为什么这件事必须落到原生
//
// Flutter 框架**没有**"外部文件拖进窗口"的 API：`Draggable` / `DragTarget` 传的
// 是**应用内**的数据（画廊里那种拖动），跨进程的拖放引擎这一侧一个字都没接。
//
// # 用哪一套拖放
//
// Windows 上有两套：① 老的 `DragAcceptFiles` + `WM_DROPFILES`（只回 `CF_HDROP`
// 里的**文件路径**）；② OLE 的 `RegisterDragDrop` + `IDropTarget`（能拿任意
// `IDataObject` 格式：位图、HTML、文本…）。
//
// 界面要的是"把文件拖进来当附件" —— 需要的恰恰就是**路径**，所以用①：
// 零依赖、不用引 ole 那一套头、不用自己实现 `IDropTarget` 的引用计数（拖放期间
// 会进模态消息循环，写错了是死锁 / 掉引用），对"只要路径"这个需求①够用。
//
// ⚠ ①的边界（**是这一套的固有边界，不是这里写漏了**）：
//   * 只拿得到 `CF_HDROP` 里的**文件系统路径**。浏览器里拖出来的一张图（OLE
//     载荷是位图 / HTML，压根不带路径）、压缩包**内部**的条目、手机 MTP 设备上的
//     文件，都给不出真实路径 —— 那时 `DragQueryFileW` 回的是**显示名**
//     （例如 `a.png`，而不是 `D:\pics\a.png`）。
//   * 提权与非提权进程之间拖不动（UIPI 挡着），一侧是管理员时表现是"拖上去没反应"。
//   要"从浏览器直接拖一张图进来"，那是**另一条通道**（OLE `IDropTarget`），
//   别指望在这条上加参数能变出来。
//
// # 消息接在哪一层
//
// 引擎自己的子窗口（Flutter view）没有开 `WS_EX_ACCEPTFILES`，但 Shell 是从
// **落点窗口沿父链往上找**接受者的，所以落在客户区任意位置都会送到**顶层窗口**
// —— 于是 `WM_DROPFILES` 在 `FlutterWindow::MessageHandler` 里接（它手里才有
// channel；基类 `Win32Window` 不用动，也不必为它开虚函数钩子）。
//
// # 方向与形状
//
// 这条通道是 **native → Dart**（和 `clipboard_channel.cpp` 的 Dart → native
// 正好相反）：原生收到 `WM_DROPFILES` 就**主动**调 Dart 的方法
//   * 方法名：`droppedFiles`
//   * 参数：  `{"paths": ["D:\\a.png", "D:\\b.jpg"]}`（`List<String>`，UTF-8）
//
// 路径一律走 UTF-16（`DragQueryFileW`）→ UTF-8（`WideCharToMultiByte`）：
// `flutter::EncodableValue` 只收 `std::string`，中文路径不转就是乱码。
//
// ## 为什么用 `MethodChannel` 而不是 `EventChannel`
//
// 拖放是**离散事件**（一次一批），不是"一条持续的数据流"。`EventChannel` 还要
// 求 Dart 侧先 subscribe、原生侧维护 `EventSink` 的存活，多一份状态就多一处
// 能空指针的地方；Dart 侧用广播 `StreamController` 把 `MethodCall` 转成流，
// 一样能让多个页面同时听（见 `lib/core/file_drop.dart`）。
class FileDropChannel {
 public:
  // 进程内唯一的一个（runner 只在启动时注册一次）。
  static FileDropChannel& instance();

  // 注册通道，并在 |hwnd| 上开 `DragAcceptFiles`。
  //
  // ⚠ 调用时机：**窗口与通道都就绪之后**（`FlutterWindow::OnCreate` 里，引擎、
  // view、channel 都创建之后）。提前启用时，接收到的路径会因
  // Dart 侧没有接收者而丢失，表现为"拖入无反应"。
  void Register(flutter::BinaryMessenger* messenger, HWND hwnd);

  // 处理一次 `WM_DROPFILES`（`wparam` 就是那个 `HDROP`）：取路径 → 推给 Dart。
  //
  // 内部一定 `DragFinish`（RAII），通道还没注册 / 路径为空时**静默丢弃**；
  // 调用方**一律 `return 0`** —— 这条消息约定"处理了就回 0"，而无论送没送出去，
  // 这一批的 `HDROP` 都已经在我们手里释放了，`DefWindowProc` 无事可做。
  void HandleDropFiles(WPARAM wparam);

  // 撤销 `DragAcceptFiles`（窗口销毁前调，用 `Register` 时记下的 hwnd）。
  //
  // 不在 `WM_DESTROY` 的 switch 里做：那一条要跟引擎的
  // `HandleTopLevelWindowProc` 抢（它可能先返回了结果），而 `OnDestroy()` 是
  // 一定会跑到的。
  void Revoke();

 private:
  FileDropChannel() = default;
  ~FileDropChannel() = default;
  FileDropChannel(const FileDropChannel&) = delete;
  FileDropChannel& operator=(const FileDropChannel&) = delete;

  // 通道要活到进程结束：`FileDropChannel::instance()` 是个函数内静态，进程退出
  // 才析构（`~MethodChannel` 是平凡的，不碰 messenger）。
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;

  // 开过 `DragAcceptFiles` 的那个顶层窗口（`Revoke` 要用）。
  HWND hwnd_ = nullptr;
};

#endif  // RUNNER_FILE_DROP_CHANNEL_H_
