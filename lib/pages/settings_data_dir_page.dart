import 'dart:async';
import 'dart:io' show Directory;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/backend_paths.dart';
import '../core/data_dir_migration.dart';
import '../widgets/settings_scaffold.dart';

/// 「数据目录」子页：看当前在哪、打开它、换一个位置（带迁移）。
///
/// 用户 2026-11-06："我能在设置内修改数据目录吗？还要考虑数据迁移。"
///
/// 数据目录 = 库（`.db\comfyhub.db`）+ 产物 / AI 数据（`storage\`）。默认就是运行根目录；
/// 换位置时**先停本地服务、再复制、再校验、最后才落指针**（`data-dir.json`），
/// 复制失败 / 取消都不会动旧数据。
class DataDirSettingsPage extends StatefulWidget {
  const DataDirSettingsPage({super.key});

  @override
  State<DataDirSettingsPage> createState() => _DataDirSettingsPageState();
}

class _DataDirSettingsPageState extends State<DataDirSettingsPage> {
  bool _busy = false;
  String _status = '';

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final launcher = context.watch<BackendLauncher>();
    final root = launcher.projectRoot;
    final dataDir = launcher.dataDir;
    final isDefault = root != null && dataDir == root;

    return SettingsScaffold(
      title: '数据目录',
      sections: [
        SettingsSection(
          title: '当前位置',
          estimatedHeight: 220,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    dataDir ?? '还没定位到数据目录',
                    style: theme.textTheme.bodyMedium,
                  ),
                  if (dataDir != null) ...[
                    const SizedBox(height: 4),
                    Text(
                      '产物在 ${dataStorageDirOf(dataDir)}',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                  const SizedBox(height: 6),
                  Text(
                    isDefault
                        ? '库和产物都在程序目录里。想放到别的盘就点下面的「更改位置」。'
                        : '库和产物在这个目录里，程序目录只留备份、日志和工作流。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Wrap(
                    spacing: 8,
                    runSpacing: 4,
                    children: [
                      TextButton.icon(
                        onPressed: dataDir == null || _busy
                            ? null
                            : () => _openFolder(dataDir),
                        icon: const Icon(Icons.folder_open_outlined, size: 18),
                        label: const Text('打开'),
                      ),
                      TextButton.icon(
                        onPressed: dataDir == null || _busy
                            ? null
                            : () => _changeLocation(launcher),
                        icon: const Icon(
                          Icons.drive_file_move_outlined,
                          size: 18,
                        ),
                        label: const Text('更改位置…'),
                      ),
                      if (!isDefault)
                        TextButton.icon(
                          onPressed: _busy
                              ? null
                              : () => _moveBackToDefault(launcher),
                          icon: const Icon(
                            Icons.settings_backup_restore,
                            size: 18,
                          ),
                          label: const Text('搬回程序目录'),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '说明',
          estimatedHeight: 220,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text(
                '换位置时会先把本地服务停下来，再把库和产物复制过去；'
                '复制完会逐个文件核对，核对通过才切换。旧目录不会自动删除，'
                '搬完会问你一次。\n'
                '库备份（backups）、运行日志（.run）、ComfyUI 工作流目录和设置文件'
                '留在程序目录里，不跟着数据走。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
          ),
        ),
        if (_busy)
          SettingsSection(
            title: '进度',
            estimatedHeight: 80,
            child: Card(
              child: ListTile(
                leading: const SizedBox(
                  width: 20,
                  height: 20,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
                title: Text(_status),
              ),
            ),
          ),
      ],
    );
  }

  Future<void> _openFolder(String path) async {
    try {
      final ok = await launchUrl(Uri.directory(path));
      if (!ok && mounted) _toast('打不开这个目录，系统里没有能打开文件夹的程序。');
    } catch (_) {
      if (mounted) _toast('打不开这个目录，系统里没有能打开文件夹的程序。');
    }
  }

  Future<void> _changeLocation(BackendLauncher launcher) async {
    final picked = await FilePicker.getDirectoryPath(dialogTitle: '选择数据目录');
    if (picked == null || !mounted) return;
    await _migrateTo(launcher, picked);
  }

  Future<void> _moveBackToDefault(BackendLauncher launcher) async {
    final root = launcher.projectRoot;
    if (root == null) return;
    await _migrateTo(launcher, root);
  }

  Future<void> _migrateTo(BackendLauncher launcher, String target) async {
    final root = launcher.projectRoot;
    final current = launcher.dataDir;
    if (root == null || current == null) {
      _toast('还没定位到数据目录，先让本地服务跑起来。');
      return;
    }

    final check = await checkDataDirTarget(target: target, current: current);
    if (!mounted) return;
    if (!check.ok) {
      await _info('不能搬到这个目录', check.message);
      return;
    }

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('把数据搬到新位置？'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('从  $current'),
            const SizedBox(height: 4),
            Text('搬到  ${Directory(target).absolute.path}'),
            const SizedBox(height: 12),
            Text(
              check.source.files == 0
                  ? '现在还没有数据，切换过去就是空的。'
                  : '一共 ${check.source.files} 个文件，'
                        '约 ${_size(check.source.bytes)}。',
            ),
            const SizedBox(height: 8),
            const Text('搬的时候本地服务会先停下来，搬完自动重启。旧目录不会自动删除。'),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('开始搬'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    await _runMigration(launcher, root: root, from: current, to: target);
  }

  Future<void> _runMigration(
    BackendLauncher launcher, {
    required String root,
    required String from,
    required String to,
  }) async {
    // 回退时要恢复的"原来那个指针值"（null = 本来就用默认目录）
    final previousPointer = readDataDirOverride(root);
    final cancelled = <bool>[false];
    final progress = ValueNotifier<MigrationProgress?>(null);
    final nav = Navigator.of(context, rootNavigator: true);

    setState(() {
      _busy = true;
      _status = '正在停止本地服务…';
    });

    unawaited(
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (dialogContext) => AlertDialog(
          title: const Text('正在搬数据'),
          content: SizedBox(
            width: 460,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ValueListenableBuilder<MigrationProgress?>(
                  valueListenable: progress,
                  builder: (_, value, _) {
                    if (value == null) {
                      return const LinearProgressIndicator();
                    }
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        LinearProgressIndicator(value: value.fraction),
                        const SizedBox(height: 8),
                        Text(
                          '${value.copiedFiles} / ${value.totalFiles} 个文件'
                          '（${_size(value.copiedBytes)} / ${_size(value.totalBytes)}）',
                        ),
                        if (value.currentPath.isNotEmpty)
                          Padding(
                            padding: const EdgeInsets.only(top: 4),
                            child: Text(
                              value.currentPath,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: Theme.of(dialogContext)
                                  .textTheme
                                  .bodySmall,
                            ),
                          ),
                      ],
                    );
                  },
                ),
                const SizedBox(height: 12),
                const Text('搬完之前别关窗口。旧数据不会被动。'),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => cancelled[0] = true,
              child: const Text('取消'),
            ),
          ],
        ),
      ),
    );

    MigrationResult result;
    try {
      await launcher.stopBackend();
      result = await migrateDataDir(
        source: from,
        target: to,
        root: root,
        onProgress: (value) {
          progress.value = value;
          if (mounted) setState(() => _status = '正在复制 ${value.currentPath}');
        },
        isCancelled: () => cancelled[0],
      );
    } catch (e) {
      result = MigrationResult(ok: false, message: '迁移出错了（$e）。旧数据没动。');
    }

    if (!mounted) {
      // 页面被关掉了也要把后端拉回可用状态：不能因为关了个弹窗让本地服务一直停着
      nav.pop();
      if (!result.ok) writeDataDirOverride(root, previousPointer);
      await launcher.ensureRunning(force: true);
      progress.dispose();
      return;
    }
    nav.pop(); // 关掉进度弹窗
    progress.dispose();

    if (!result.ok) {
      // 指针没写（或已回滚），旧目录还在用：把后端按原样起回来
      writeDataDirOverride(root, previousPointer);
      await launcher.ensureRunning(force: true);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _status = '';
      });
      await _info(result.cancelled ? '已取消' : '没有搬成', result.message);
      return;
    }

    // 指针已经指向新目录：用新目录起后端，起不来就回退。
    final ok = await launcher.ensureRunning(force: true);
    if (!mounted) return;
    if (!ok) {
      writeDataDirOverride(root, previousPointer);
      final recovered = await launcher.ensureRunning(force: true);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _status = '';
      });
      await _info(
        '搬完了，但后端用新目录没起来',
        '数据已经复制到 $to，但后端用这个目录启动失败'
            '${recovered ? '，已经改回原来的目录并重启。' : '，改回原来的目录也没起来，请到「本地服务」看日志。'}',
      );
      return;
    }

    setState(() {
      _busy = false;
      _status = '';
    });

    // 用户 2026-11-06 选的"迁移完弹窗问我"：旧数据删不删当场问，默认保留。
    final remove = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('搬好了'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('数据现在在 $to，本地服务已经用新目录跑起来了。'),
            const SizedBox(height: 12),
            Text('旧目录还在：$from'),
            const SizedBox(height: 8),
            const Text('确认画廊、提示词都正常之后，可以删掉旧目录里的数据（备份、日志和工作流不删）。'),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('先留着'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('删掉旧数据'),
          ),
        ],
      ),
    );

    if (remove == true) {
      final report = await deleteOldData(from);
      if (!mounted) return;
      if (report.failed.isEmpty) {
        _toast(
          '旧数据已删除（${report.deletedFiles} 个文件，'
          '${_size(report.freedBytes)}）。',
        );
      } else {
        await _info(
          '有一部分没删掉',
          '已经删了 ${report.deletedFiles} 个文件（${_size(report.freedBytes)}），'
              '还有 ${report.failed.length} 个删不掉（可能被别的程序占着）：\n\n'
              '${report.failed.take(5).join('\n')}',
        );
      }
    }
  }

  Future<void> _info(String title, String message) {
    return showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(title),
        content: Text(message),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: const Text('知道了'),
          ),
        ],
      ),
    );
  }

  void _toast(String message) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  static String _size(int bytes) {
    if (bytes < 1024) return '$bytes B';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
    if (bytes < 1024 * 1024 * 1024) {
      return '${(bytes / 1024 / 1024).toStringAsFixed(1)} MB';
    }
    return '${(bytes / 1024 / 1024 / 1024).toStringAsFixed(2)} GB';
  }
}
