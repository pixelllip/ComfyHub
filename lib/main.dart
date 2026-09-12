import 'package:flutter/material.dart';

import 'app.dart';
import 'core/backend_launcher.dart';
import 'core/settings_store.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final settings = SettingsStore();
  await settings.load();
  // 本地服务（MySQL + Kotlin 后端）由 BackendLauncher 在首屏自动拉起，
  // 不需要用户先手动执行 scripts\comfyhub.ps1 up。
  final launcher = BackendLauncher(settings);
  runApp(ComfyHubApp(settings: settings, launcher: launcher));
}
