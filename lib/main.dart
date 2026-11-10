import 'dart:io' show Platform;

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';

import 'app.dart';
import 'core/backend_launcher.dart';
import 'core/inbound_auth_api.dart';
import 'core/portable_paths.dart';
import 'core/settings_store.dart';
import 'core/system_accent.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // 必须在第一次碰 SharedPreferences 之前装（见 portable_paths.dart 文件头）：
  // 受限会话里 viewer.exe 写不了 %APPDATA%，偏好保存一直报 errno 5；
  // 默认位置写得进的正常机器上这一步是空操作。
  await installPortablePaths();
  final settings = SettingsStore();
  await settings.load();
  // **配对令牌要在第一帧之前读进内存**（2026-11-09 真机）。
  //
  // 以前它只在 `AiWorkspaceStore._doLoad()` 里读，而画廊 / 提示词 / NSFW 开关的数据
  // 在启动闸门放开后**同时**开跑 —— 那些请求读的是 `PairingTokenCache`，此刻还是空，
  // 一进入 App 就出现一条 `需要配对令牌（X-ComfyHub-Token）`，需手动刷新才恢复。
  // 手机连的是电脑的后端，闸门开着时没有这个头一律 403，所以"谁先跑"必须由这里定。
  //
  // 桌面（以及 Web）没有密钥库：跳过，`PairingTokenCache` 保持 null（既有语义不变）。
  if (!kIsWeb && Platform.isAndroid) {
    try {
      await PairingTokenStore().read();
    } catch (_) {
      // 桥未启动 / 密钥库不可读：按"未配对"处理，页面会如实说明，在设置中重新配置即可 ——
      // 这一步绝不许拦住启动。
    }
  }
  // 平台强调色在**首帧之前**读一次：主题一画就要用，晚一步会先闪默认紫。
  // 取不到（Linux / macOS / 老 Android）就是 null，回默认配色 —— 不阻塞、不报错。
  await SystemAccent.detect();
  // 本地服务（后端）由 BackendLauncher 在首屏自动拉起：它直接执行
  // `<运行根>\backend\comfyhub-server-rs.exe`（见 lib\core\backend_paths.dart），
  // 无需预先手动操作，也不依赖 PowerShell。
  final launcher = BackendLauncher(settings);
  runApp(ComfyHubApp(settings: settings, launcher: launcher));
}
