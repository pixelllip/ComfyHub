import 'dart:io' show Platform;

import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride, kIsWeb, TargetPlatform;

/// 「这台机器是不是 Windows 桌面端」。
///
/// 设置首页有几项**只有 Windows 桌面端才有**：本地服务（后端 + 数据库）、
/// 数据目录、导出全部产物。手机连的是桌面的后端，本机既没有 Rust 后端可拉，
/// 也没有盘符与资源管理器 —— 这些入口在手机上一条都不该出现
/// （用户 2026-09-28 与 2026-11-08 两次报的都是同一件事：设置页出现了
/// 只有电脑才能用的功能）。
///
/// ⚠ 单测跑在 Windows 上，`Platform.isWindows` 永远是 true；所以先用
/// `debugDefaultTargetPlatformOverride` 兜住"模拟手机"的用例
/// （与 `scroll_linked_title.dart` 的 `useCollapsingPageTitle` 同一套做法）。
/// 正式运行时它是 null，判据仍然只有真实平台。
bool get isWindowsDesktopHost {
  if (kIsWeb) return false;
  final override = debugDefaultTargetPlatformOverride;
  if (override != null) return override == TargetPlatform.windows;
  return Platform.isWindows;
}
