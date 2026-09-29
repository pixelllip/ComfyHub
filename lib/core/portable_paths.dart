//! Windows：把 shared_preferences 的落盘位置固定在项目内（`<项目根>\storage\app-support`）。
//!
//! ## 为什么（2026-09-28 用户报的现场，2026-09-29 改了方向）
//!
//! ```text
//! Error saving preferences to disk: PathAccessException: Cannot open file,
//! path = 'C:\Users\Administrator\AppData\Roaming\com.example\viewer\shared_preferences.json'
//! (OS Error: 拒绝访问。, errno = 5)
//! ```
//!
//! 根因**不是 ACL**：受限会话把"映像在工作区内"的进程整个关在工作区里，工作区之外写不了
//! （判据与实测见 AGENTS §12）。而 debug / Release 的 viewer.exe 都在工作区内。
//!
//! 原来的修法是"**默认位置写得进就一切照旧，写不进才重定向**"。方向错了：
//! ① 判据是"探针能不能建文件"，而那取决于进程映像位置，于是同一条逻辑在源码树/发布包
//!    给出相反结论；② 更根本的是 —— 偏好是 **ComfyHub 自己产生的内容**，
//!    按"自己产生的内容存在自身文件夹内"这条，它默认就该在 `<项目根>\storage\app-support`，
//!    不该散在 `%APPDATA%`。
//!
//! **现在：默认就落在项目内，并一次性把 `%APPDATA%` 里已有的那份导入过来**
//! （用户上次的草稿 / 选中的 provider-model 不丢）。代价是设置不再跨安装共享 ——
//! 但 db 与 storage 本来就在各自文件夹里，这与"发布包是便携的"本来就是一致的。
//!
//! ## 怎么实现
//!
//! `shared_preferences_windows` 把 `pathProvider` 挂成**公开实例字段**
//! （`SharedPreferencesWindows.pathProvider`）—— 在第一次碰 SharedPreferences 之前
//! 把它换成"指向项目内"的实现即可（`pathProvider` 是这个包给的**唯一注入缝**，
//! 标了 `@visibleForTesting`，但没有官方替代 —— 不接它就没有别的办法改落盘位置）。
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
    final workspaceDir = '$root${sep}storage${sep}app-support';
    await Directory(workspaceDir).create(recursive: true);
    // 一次性导入：把用户此前落在 %APPDATA% 的那份搬进来（没有就跳过）
    final String? legacyDir = await PathProviderWindows().getApplicationSupportPath();
    _migrateLegacyPrefsFile(legacyDir, workspaceDir);
    final store = SharedPreferencesStorePlatform.instance;
    if (store is SharedPreferencesWindows) {
      // pathProvider 是 shared_preferences_windows 给的**唯一注入缝**
      // （标了 @visibleForTesting，但没有官方替代 —— 不接它就没有别的办法
      // 改落盘位置）。这里是刻意使用，不是测试代码漏进生产。
      // ignore: invalid_use_of_visible_for_testing_member
      store.pathProvider = WorkspacePathProvider(workspaceDir);
      debugPrint('偏好落盘位置：$workspaceDir'
          '（内容一律存在自身文件夹内；旧位置 $legacyDir 的那份已一次性导入）');
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

/// 把旧的偏好文件从 `%APPDATA%` 拷到新位置（**只拷一次**：目标已存在就不动）。
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
