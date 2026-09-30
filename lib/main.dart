import 'package:flutter/material.dart';

import 'app.dart';
import 'core/backend_launcher.dart';
import 'core/portable_paths.dart';
import 'core/settings_store.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // 必须在第一次碰 SharedPreferences 之前装（见 portable_paths.dart 文件头）：
  // 受限会话里 viewer.exe 写不了 %APPDATA%，偏好保存一直报 errno 5；
  // 默认位置写得进的正常机器上这一步是空操作。
  await installPortablePaths();
  final settings = SettingsStore();
  await settings.load();
  // 本地服务（后端）由 BackendLauncher 在首屏自动拉起：它直接执行
  // `<运行根>\backend\comfyhub-server-rs.exe`（见 lib\core\backend_paths.dart），
  // 不需要用户先手动做什么，也不需要 PowerShell。
  final launcher = BackendLauncher(settings);
  runApp(ComfyHubApp(settings: settings, launcher: launcher));
}
