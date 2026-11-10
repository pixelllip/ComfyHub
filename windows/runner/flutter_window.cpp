#include "flutter_window.h"

#include <optional>

#include "clipboard_channel.h"
#include "file_drop_channel.h"
#include "flutter/generated_plugin_registrant.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  // 自建通道（2026-10-18）：读/写剪贴板里的**文件**（CF_HDROP）。
  // 内置的 Clipboard 只会写 CF_UNICODETEXT，"复制文件本身"做不到 —— 见通道的头注释。
  RegisterClipboardChannel(flutter_controller_->engine()->messenger());
  // 拖进窗口的文件（2026-10-18）：DragAcceptFiles + WM_DROPFILES。
  // ⚠ 必须放在此处（窗口与通道均就绪后）—— 提前启用拖放时，
  // 拖入的文件会因 Dart 侧没有接收者而丢失（见 file_drop_channel.h）。
  FileDropChannel::instance().Register(flutter_controller_->engine()->messenger(),
                                       GetHandle());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  // 窗口要没了：撤销拖放接受（用 Register 时记下的 hwnd —— 走到这里时
  // `Win32Window::window_handle_` 可能已经置空了，见 file_drop_channel.h）。
  FileDropChannel::instance().Revoke();
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_DROPFILES:
      // 从资源管理器拖进来的文件（见 file_drop_channel.h）。
      //
      // 接在**这一层**：`Win32Window::WndProc` 把所有消息都转给虚函数
      // `MessageHandler`，而 channel 在这个子类手里 —— 基类不用动，也不必为它
      // 开虚函数钩子。Shell 从落点窗口（引擎的子窗口）沿父链往上找
      // `WS_EX_ACCEPTFILES`，落到这里的就是顶层窗口的这个 hwnd。
      //
      // ⚠ 一定 `return 0`：这条消息约定"处理了就回 0"；而且无论 Dart 有没有收到，
      // 这一批的 HDROP 都已经在 HandleDropFiles 里 DragFinish 了，不能再交给
      // DefWindowProc 去碰。
      FileDropChannel::instance().HandleDropFiles(wparam);
      return 0;

    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
