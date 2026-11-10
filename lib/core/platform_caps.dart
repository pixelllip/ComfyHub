import 'dart:io' show Platform;

import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride, kIsWeb, TargetPlatform;

/// 「这台机器是不是 Windows 桌面端」。
///
/// 设置首页有几项只在 Windows 桌面端存在：本地服务（后端 + 数据库）、数据目录、
/// 导出全部产物。手机连接的是桌面后端，本机没有 Rust 后端可启动，也没有盘符与
/// 资源管理器，这些入口在手机上不显示。
///
/// ⚠ 单测在 Windows 上运行，`Platform.isWindows` 恒为 true；因此先用
/// `debugDefaultTargetPlatformOverride` 覆盖"模拟手机"的用例
/// （与 `scroll_linked_title.dart` 的 `useCollapsingPageTitle` 同一做法）。
/// 正式运行时该值为 null，判据只取真实平台。
bool get isWindowsDesktopHost {
  if (kIsWeb) return false;
  final override = debugDefaultTargetPlatformOverride;
  if (override != null) return override == TargetPlatform.windows;
  return Platform.isWindows;
}
