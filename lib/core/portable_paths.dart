//! Windows：把 shared_preferences 的落盘位置搬进项目内（`<项目根>\storage\app-support`）。
//!
//! ## 为什么（2026-09-28 用户报的现场）
//!
//! ```text
//! Error saving preferences to disk: PathAccessException: Cannot open file,
//! path = 'C:\Users\Administrator\AppData\Roaming\com.example\viewer\shared_preferences.json'
//! (OS Error: 拒绝访问。, errno = 5)
//! ```
//!
//! 根因**不是 ACL，也不是两个实例并发写**：受限会话（AI 助手的文件沙箱）会把
//! "映像在工作区内"的进程整个关在工作区里，工作区之外一个字节都写不了 —— 而
//! debug / Release 的 viewer.exe 都在工作区内（实测对照与判据见
//! `docs\pitfalls.md`「后端删不掉提示词 / 会话」与「App 点启动时 WMI 被拒」两条）。
//! 同一个根因还打着 MCP 子进程（`comfy-cli` 的 `%LOCALAPPDATA%\comfy-cli\secrets.json.lock`
//! 报 Errno 13，修法在 `server-rs\src\mcp.rs` 的 `McpClient::spawn`）。
//!
//! ## 怎么修
//!
//! `shared_preferences_windows` 把 `pathProvider` 挂成**公开实例字段**
//! （`SharedPreferencesWindows.pathProvider`）—— 在第一次碰 SharedPreferences 之前
//! 把它换成"指向项目内"的实现即可。判据刻意保守：
//!
//! - **默认位置写得进就一切照旧**（正常机器上连落盘位置都不变，零行为差异）；
//! - 只有探测到写不进才重定向，并把旧的 `shared_preferences.json` 一次性拷过来，
//!   用户已有的草稿 / 上次选的 provider-model 不丢。
//!
//! 注意：只重定向 app-support（shared_preferences 用它）。VM service 的 DevFS 走
//! `%TEMP%`，那要靠 `scripts\dev-app.ps1` 在进程拉起前设 TMP/TEMP，代码里改不了。

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider_windows/path_provider_windows.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_platform_interface.dart';
import 'package:shared_preferences_windows/shared_preferences_windows.dart';

import 'backend_launcher.dart';

const String _prefsFileName = 'shared_preferences.json';

/// 在 main() 里、任何 SharedPreferences 使用之前调用。
/// 返回 true = 发生了重定向（默认位置写不进，已改用项目内）。
Future<bool> installPortablePaths() async {
  if (kIsWeb || !Platform.isWindows) return false;
  try {
    final String? root = BackendLauncher.detectProjectRoot();
    if (root == null) return false;
    final sep = Platform.pathSeparator;
    final String? legacyDir = await PathProviderWindows().getApplicationSupportPath();
    if (legacyDir != null && dirWritable(legacyDir)) {
      return false; // 默认位置好好的：一切照旧，连迁移都不做
    }
    final String workspaceDir = '$root${sep}storage${sep}app-support';
    await Directory(workspaceDir).create(recursive: true);
    _migrateLegacyPrefsFile(legacyDir, workspaceDir);
    final store = SharedPreferencesStorePlatform.instance;
    if (store is SharedPreferencesWindows) {
      // pathProvider 是 shared_preferences_windows 给的**唯一注入缝**
      // （标了 @visibleForTesting，但没有官方替代 —— 不接它就没有别的办法
      // 改落盘位置）。这里是刻意使用，不是测试代码漏进生产。
      // ignore: invalid_use_of_visible_for_testing_member
      store.pathProvider = WorkspacePathProvider(workspaceDir);
      debugPrint('偏好落盘位置已重定向到项目内：$workspaceDir'
          '（默认位置 $legacyDir 写不进 —— 见 docs/pitfalls.md「映像在工作区内的进程」）');
      return true;
    }
    return false;
  } catch (e) {
    debugPrint('installPortablePaths 失败（忽略，按默认位置走）: $e');
    return false;
  }
}

/// 指向项目内的 pathProvider：只覆盖 app-support，其余沿用默认实现。
class WorkspacePathProvider extends PathProviderWindows {
  WorkspacePathProvider(this.dir);

  final String dir;

  @override
  Future<String?> getApplicationSupportPath() async => dir;
}

/// 目录当前用户能不能建 / 写文件。目录不存在就先建（shared_preferences 首次写盘
/// 也是 `createSync(recursive: true)`，所以"建不出来"本身就等于"写不进"）。
bool dirWritable(String path) {
  try {
    final dir = Directory(path);
    if (!dir.existsSync()) dir.createSync(recursive: true);
    final probe = File('$path${Platform.pathSeparator}.comfyhub-write-probe');
    final handle = probe.openSync(mode: FileMode.write);
    handle.closeSync();
    probe.deleteSync();
    return true;
  } catch (_) {
    return false;
  }
}

/// 把旧的偏好文件拷到新位置（只拷一次：目标已存在就不动）。
/// 受限进程**读**项目外是允许的，所以即便 errno 5 的现场，迁移也做得成。
void _migrateLegacyPrefsFile(String? legacyDir, String newDir) {
  if (legacyDir == null || legacyDir.isEmpty) return;
  final sep = Platform.pathSeparator;
  final src = File('$legacyDir$sep$_prefsFileName');
  final dst = File('$newDir$sep$_prefsFileName');
  if (!src.existsSync() || dst.existsSync()) return;
  try {
    src.copySync(dst.path);
  } catch (e) {
    // 迁移失败只丢"上次的草稿 / 选中项"，不值得拦启动。
    debugPrint('偏好文件迁移失败（忽略）: $e');
  }
}
