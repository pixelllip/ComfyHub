//! Windows：把 shared_preferences 的落盘位置固定在项目内（`<项目根>\app-support`）。
//!
//! ## 约束
//!
//! 受限会话中，映像位于工作区内的进程只能写工作区（判据见 AGENTS §12）；
//! debug / Release 的 viewer.exe 都在工作区内。写入 `%APPDATA%` 会返回
//! `PathAccessException`（errno 5）。
//!
//! 偏好属于 ComfyHub 自身产生的内容，按"自身产生的内容存在自身文件夹内"的约束，
//! 默认落在项目内，不写入 `%APPDATA%`。首次运行会把 `%APPDATA%` 中已有的偏好
//! 一次性导入（保留草稿与已选 provider-model）。设置不再跨安装共享。
//!
//! ## 落点变更（2026-11-06）
//!
//! 数据目录（库 + `storage`）可由 `data-dir.json` 指向其他磁盘。偏好不随之移动：
//! 读取偏好时数据目录尚不可知，且数据目录被删除后设置不应消失。偏好因此位于
//! **运行根目录下**，与数据分离；旧位置的文件一次性复制，不删除旧文件。
//!
//! ## 实现
//!
//! `shared_preferences_windows` 把 `pathProvider` 作为公开实例字段
//! （`SharedPreferencesWindows.pathProvider`）。在首次访问 SharedPreferences 之前
//! 将其替换为指向项目内的实现（`pathProvider` 是该包**唯一注入点**，标记为
//! `@visibleForTesting`，没有官方替代）。
//!
//! VM service 的 DevFS 走 `%TEMP%`，代码里改不了（`Directory.systemTemp` 取自本进程 TMP/TEMP），
//! 由 `windows\runner\main.cpp` 在引擎起来之前处理。

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider_windows/path_provider_windows.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_platform_interface.dart';
import 'package:shared_preferences_windows/shared_preferences_windows.dart';

import 'backend_launcher.dart';

const String _prefsFileName = 'shared_preferences.json';

/// 在 main() 里、任何 SharedPreferences 使用之前调用。
/// 返回 true = 落盘位置已指向项目内（无论是不是这次刚迁移的）。
Future<bool> installPortablePaths() async {
  if (kIsWeb || !Platform.isWindows) return false;
  try {
    final String? root = BackendLauncher.detectProjectRoot();
    if (root == null) return false;
    final sep = Platform.pathSeparator;
    final workspaceDir = '$root${sep}app-support';
    await Directory(workspaceDir).create(recursive: true);
    // 一次性导入：先把"上一版落点"（`<根>\storage\app-support`）里的那份拷过来，
    // 再兜底 `%APPDATA%` 的老文件（没有就跳过）。目标已存在时两个都不动。
    _migratePrefsFile('$root${sep}storage${sep}app-support', workspaceDir);
    final String? legacyDir = await PathProviderWindows()
        .getApplicationSupportPath();
    _migrateLegacyPrefsFile(legacyDir, workspaceDir);
    final store = SharedPreferencesStorePlatform.instance;
    if (store is SharedPreferencesWindows) {
      // pathProvider 是 shared_preferences_windows 给的**唯一注入缝**
      // （标了 @visibleForTesting，但没有官方替代 —— 不接它就没有别的办法
      // 改落盘位置）。这里是刻意使用，不是测试代码漏进生产。
      // ignore: invalid_use_of_visible_for_testing_member
      store.pathProvider = WorkspacePathProvider(workspaceDir);
      debugPrint(
        '偏好落盘位置：$workspaceDir'
        '（内容一律存在自身文件夹内；旧位置 $legacyDir 的那份已一次性导入）',
      );
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

/// 把旧的偏好文件从一个目录拷到新位置（**只拷一次**：目标已存在就不动）。
/// 受限进程**读**项目外是允许的，所以即便 errno 5 的现场，迁移也做得成。
void _migrateLegacyPrefsFile(String? legacyDir, String newDir) {
  if (legacyDir == null || legacyDir.isEmpty) return;
  _migratePrefsFile(legacyDir, newDir);
}

/// 从 [fromDir] 拷一份 `shared_preferences.json` 到 [newDir]（目标已存在就不动）。
void _migratePrefsFile(String? fromDir, String newDir) {
  if (fromDir == null || fromDir.isEmpty) return;
  final sep = Platform.pathSeparator;
  final src = File('$fromDir$sep$_prefsFileName');
  final dst = File('$newDir$sep$_prefsFileName');
  if (!src.existsSync() || dst.existsSync()) return;
  try {
    src.copySync(dst.path);
  } catch (e) {
    // 迁移失败只丢"上次的草稿 / 选中项"，不值得拦启动。
    debugPrint('偏好文件迁移失败（忽略）: $e');
  }
}
