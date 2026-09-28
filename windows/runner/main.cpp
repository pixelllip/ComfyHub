#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <string>

#include "flutter_window.h"
#include "utils.h"

namespace {

// 这个文件能不能建出来并写进去（用完即删）。
bool FileWritable(const std::wstring& path) {
  HANDLE handle = ::CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr,
                                CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, nullptr);
  if (handle == INVALID_HANDLE_VALUE) {
    return false;
  }
  ::CloseHandle(handle);
  ::DeleteFileW(path.c_str());
  return true;
}

std::wstring ExecutableDir() {
  wchar_t buffer[MAX_PATH + 1] = {0};
  const DWORD len = ::GetModuleFileNameW(nullptr, buffer, MAX_PATH);
  std::wstring path(buffer, len);
  const size_t slash = path.find_last_of(L"\\/");
  return slash == std::wstring::npos ? std::wstring() : path.substr(0, slash);
}

bool FileExists(const std::wstring& path) {
  return ::GetFileAttributesW(path.c_str()) != INVALID_FILE_ATTRIBUTES;
}

// 2026-09-28（用户报 "flutter run -d windows 到打开应用之后，调试直接断掉"）：
// 受限会话（AI 助手的文件沙箱）会把"映像在工作区内"的进程整个关在工作区里 ——
// 工作区之外一个字节都写不了，连自己的 `%TEMP%` 都写不了。而 debug 版的
// VM service 要在 temp 里建 DevFS 目录，于是 flutter run 当场崩：
//
//   _createDevFS: Unexpected exception: PathAccessException: Creation of temporary
//   directory failed, path = 'C:\Users\...\AppData\Local\Temp' (OS Error: 拒绝访问。, errno = 5)
//
// Dart 侧救不了：DevFS 是 VM service 直接用 `Directory.systemTemp` 建的，而
// systemTemp 取自**本进程的 TMP/TEMP** —— 只有在引擎（Dart VM）起来之前设才有效，
// 所以这一步必须在这里、`FlutterWindow` 之前做（放在 Dart 的 main() 里已经晚了）。
//
// 判据刻意保守：**默认 temp 建得出文件就什么都不做**（正常机器零行为差异）；
// 只有它不可用才退到工作区内的 `<根>\.run\tmp`（发布包本来就是便携式的，
// 可写数据都在包内，与 `scripts\dev-app.ps1` 给 TMP/TEMP 指路是同一个配方）。
void EnsureWritableTempDir() {
  wchar_t temp[MAX_PATH + 1] = {0};
  const DWORD len = ::GetTempPathW(MAX_PATH, temp);
  if (len > 0 && len < MAX_PATH) {
    const std::wstring probe = std::wstring(temp) + L"comfyhub-temp-probe-" +
                               std::to_wstring(::GetCurrentProcessId()) + L".tmp";
    if (FileWritable(probe)) {
      return;  // 默认 temp 好好的：一切照旧
    }
  }

  // 往上找 `scripts\comfyhub.ps1` 定工作区根（与 Dart 侧
  // BackendLauncher.detectProjectRoot 同一套判据：源码树与发布包都认）。
  std::wstring dir = ExecutableDir();
  std::wstring root;
  for (int depth = 0; depth < 8 && !dir.empty(); depth++) {
    if (FileExists(dir + L"\\scripts\\comfyhub.ps1")) {
      root = dir;
      break;
    }
    const size_t slash = dir.find_last_of(L"\\/");
    if (slash == std::wstring::npos) {
      break;
    }
    const std::wstring parent = dir.substr(0, slash);
    if (parent == dir) {
      break;
    }
    dir = parent;
  }
  if (root.empty()) {
    return;  // 认不出工作区：保持原样，让上层照旧报错（不猜路径）
  }

  const std::wstring run_dir = root + L"\\.run";
  const std::wstring tmp_dir = run_dir + L"\\tmp";
  ::CreateDirectoryW(run_dir.c_str(), nullptr);
  ::CreateDirectoryW(tmp_dir.c_str(), nullptr);
  const std::wstring probe = tmp_dir + L"\\comfyhub-temp-probe-" +
                             std::to_wstring(::GetCurrentProcessId()) + L".tmp";
  if (!FileWritable(probe)) {
    return;  // 连工作区里的 temp 都写不了：别把 TMP 指到一个更坏的地方
  }
  ::SetEnvironmentVariableW(L"TMP", tmp_dir.c_str());
  ::SetEnvironmentVariableW(L"TEMP", tmp_dir.c_str());
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // 必须在 FlutterWindow（= 起引擎 / Dart VM）之前：DevFS 用的是 Dart 的
  // Directory.systemTemp，而它取自本进程的 TMP/TEMP。
  EnsureWritableTempDir();

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1440, 920);
  if (!window.Create(L"ComfyHub - ComfyUI 提示词管理", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
