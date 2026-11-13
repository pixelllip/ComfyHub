import 'dart:async';

import 'package:file_picker/file_picker.dart';
import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/backend_paths.dart';
import '../core/data_dir_migration.dart';
import '../widgets/settings_scaffold.dart';

/// 「数据目录」子页：查看数据的存放位置，一键搬运，一键搬回默认位置。
///
/// 设计约束：只需一个按钮即可搬运，一个按钮搬回默认目录。
///
/// 因此这一页只保留两个动作：「搬到别处…」与「搬回默认位置」。
/// 选择目标文件夹后立即开始搬运，不再追加确认步骤；搬运过程本身是安全的：
/// 先停止程序 → 复制 → 逐个核对 → 核对一致后才记录新位置（`data-dir.json`）。
/// 原来的文件夹既不会被删除，也不会被修改。
class DataDirSettingsPage extends StatefulWidget {
  /// 设置搜索跳转到的区块（当前只有 `current`）。null = 从页首开始。
  final String? initialSectionId;

  const DataDirSettingsPage({super.key, this.initialSectionId});

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
    // 数据目录的结论只有一份：`BackendLauncher.dataDirInfo`（它算的时候带着
    // "上次用的是哪份库"的身份）。这里**不自己再算一遍**，否则界面会和实际起后端
    // 用的那份漂移。指针改过之后调 `refreshDataDirInfo()` 刷新。
    final info = launcher.dataDirInfo;
    final dataDir = info?.dataDir ?? launcher.dataDir;
    final isDefault = root != null && dataDir == root;
    final hint = dataDir == null
        ? '尚未确定数据的存放位置。'
        : isDefault
        ? '对话、图片和角色卡都保存在程序所在的文件夹里。'
        : '对话、图片和角色卡保存在这里，程序所在的文件夹中不再保存这些内容。';
    // 这次打开的不是"上次用的那份"时，给一条明确的退回路径。
    final canSwitchBack =
        info != null &&
        !info.matchesRemembered &&
        (info.knownLibraryPath ?? '').isNotEmpty;

    return SettingsScaffold(
      title: '数据目录',
      initialSectionId: widget.initialSectionId,
      sections: [
        SettingsSection(
          title: '当前位置',
          id: 'current',
          estimatedHeight: 240,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          dataDir ?? '尚未确定数据的存放位置',
                          style: theme.textTheme.bodyMedium,
                        ),
                      ),
                      IconButton(
                        tooltip: '打开文件夹',
                        icon: const Icon(Icons.folder_open_outlined, size: 20),
                        onPressed: dataDir == null || _busy
                            ? null
                            : () => _openFolder(dataDir),
                      ),
                    ],
                  ),
                  Text(
                    hint,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  if (info?.notice != null) ...[
                    const SizedBox(height: 6),
                    Text(
                      info!.notice!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ],
                  if (canSwitchBack) ...[
                    const SizedBox(height: 4),
                    TextButton(
                      onPressed: _busy
                          ? null
                          : () => _switchTo(launcher, info.knownLibraryPath!),
                      child: const Text('换回上次那份'),
                    ),
                  ],
                  const SizedBox(height: 8),
                  Wrap(
                    spacing: 8,
                    runSpacing: 4,
                    children: [
                      FilledButton.icon(
                        onPressed: dataDir == null || _busy
                            ? null
                            : () => _changeLocation(launcher),
                        icon: const Icon(
                          Icons.drive_file_move_outlined,
                          size: 18,
                        ),
                        label: const Text('搬到别处…'),
                      ),
                      if (!isDefault)
                        OutlinedButton.icon(
                          onPressed: dataDir == null || _busy
                              ? null
                              : () => _moveBackToDefault(launcher),
                          icon: const Icon(
                            Icons.settings_backup_restore,
                            size: 18,
                          ),
                          label: const Text('搬回默认位置'),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        if (info != null && info.otherCandidates.isNotEmpty)
          SettingsSection(
            title: '还发现一份数据',
            id: 'other',
            estimatedHeight: 160,
            child: Card(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final candidate in info.otherCandidates)
                    ListTile(
                      dense: true,
                      title: Text(candidate.path),
                      subtitle: Text(
                        '共 ${sizeText(candidate.bytes)}。其中也包含对话、图片和角色卡，'
                        '但与当前使用的不是同一份。',
                      ),
                      trailing: TextButton(
                        onPressed: _busy
                            ? null
                            : () => _switchTo(launcher, candidate.path),
                        child: const Text('改用这一份'),
                      ),
                    ),
                ],
              ),
            ),
          ),
        SettingsSection(
          title: '说明',
          id: 'help',
          estimatedHeight: 180,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text(
                '开始搬运前，程序会先停止运行；随后逐个复制对话、图片和角色卡，'
                '复制完成后逐个核对。全部核对一致，才会改用新的位置，并自动重新启动。\n'
                '原来文件夹中的内容不会被删除，也不会被修改。确认新位置正常后，'
                '是否删除由你决定。\n'
                '程序所在文件夹中的程序文件、运行记录和设置不会随之移动。',
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
            id: 'progress',
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
      if (!ok && mounted) _toast('无法打开该文件夹，系统中没有可用的文件管理器。');
    } catch (_) {
      if (mounted) _toast('无法打开该文件夹，系统中没有可用的文件管理器。');
    }
  }

  /// 「搬到别处…」：选择目标文件夹后立即开始搬运，不再追加确认步骤。
  Future<void> _changeLocation(BackendLauncher launcher) async {
    final picked = await FilePicker.getDirectoryPath(dialogTitle: '选择新的存放位置');
    if (picked == null || !mounted) return;
    // 直接选择 `…\storage` 时按上一级计算，否则会形成 `storage\storage`。
    final target = normalizeDataDirTarget(picked);
    await _migrateTo(launcher, target);
  }

  /// 「搬回默认位置」：直接搬回程序所在的文件夹，无需另行选择。
  Future<void> _moveBackToDefault(BackendLauncher launcher) async {
    final root = launcher.projectRoot;
    if (root == null) return;
    await _migrateTo(launcher, root);
  }

  /// 切换到另一个存放位置（只更新记录位置的那一个文件，不复制任何内容）。
  Future<void> _switchTo(BackendLauncher launcher, String target) async {
    setState(() => _status = '正在切换到 $target…');
    final ok = await launcher.switchDataDir(target);
    if (!mounted) return;
    launcher.refreshDataDirInfo();
    setState(() => _status = '');
    _toast(ok ? '已切换到 $target。' : '无法切换到 $target。可能记录没有写入，或者该文件夹当前无法访问。');
  }

  Future<void> _migrateTo(BackendLauncher launcher, String target) async {
    final root = launcher.projectRoot;
    final current = launcher.dataDir;
    if (root == null || current == null) {
      _toast('尚未确定数据的存放位置，请先让程序正常运行。');
      return;
    }

    final check = await checkDataDirTarget(target: target, current: current);
    if (!mounted) return;
    if (!check.ok) {
      await _info('不能搬到这个文件夹', check.message);
      return;
    }

    final summary = check.source.files == 0
        ? '当前的位置没有数据，搬运后的新位置同样是空的。'
        : '共 ${check.source.files} 个文件，合计约 ${_size(check.source.bytes)}。';
    await _runMigration(
      launcher,
      root: root,
      from: current,
      to: target,
      summary: summary,
    );
  }

  Future<void> _runMigration(
    BackendLauncher launcher, {
    required String root,
    required String from,
    required String to,
    required String summary,
  }) async {
    // 回退时要恢复的"原来那个指针值"（null = 本来就用默认目录）
    final previousPointer = readDataDirOverride(root);
    final cancelled = <bool>[false];
    final progress = ValueNotifier<MigrationProgress?>(null);
    final nav = Navigator.of(context, rootNavigator: true);

    setState(() {
      _busy = true;
      _status = '正在停止程序…';
    });

    unawaited(
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (dialogContext) => AlertDialog(
          title: const Text('正在搬运'),
          content: SizedBox(
            width: 460,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('现在的位置是 $from'),
                const SizedBox(height: 4),
                Text('新的位置是 $to'),
                const SizedBox(height: 8),
                Text(
                  summary,
                  style: Theme.of(dialogContext).textTheme.bodySmall,
                ),
                const SizedBox(height: 12),
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
                          '已复制 ${value.copiedFiles} / ${value.totalFiles} 个文件'
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
                const Text('搬运期间请不要关闭窗口。原来的文件夹不会被删除。'),
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
      result = MigrationResult(ok: false, message: '搬运过程中出错（$e）。原来的内容没有被修改。');
    }

    if (!mounted) {
      // 页面被关掉了也要把后端拉回可用状态：不能因为关了个弹窗让程序一直停着
      nav.pop();
      if (!result.ok && !writeDataDirOverride(root, previousPointer)) {
        // 页面已经没了，弹不出提示：至少留一行日志（下次启动会去读这个指针）
        debugPrint('[data-dir] 迁移失败后回滚 data-dir.json 也没成功，指针可能还指着新目录');
      }
      await launcher.ensureRunning(force: true);
      progress.dispose();
      return;
    }
    nav.pop(); // 关掉进度弹窗
    progress.dispose();

    if (!result.ok) {
      // 指针没写（或已回滚），旧目录还在用：把程序按原样起回来。
      // ⚠ 回滚**可能失败**（data-dir.json 写不进去），那时下一句就不能说"原来的位置仍在使用"。
      final restored = writeDataDirOverride(root, previousPointer);
      await launcher.ensureRunning(force: true);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _status = '';
      });
      await _info(
        result.cancelled ? '已取消' : '没有搬成',
        restored
            ? result.message
            : '${result.message}\n\n'
                  '而且"恢复原来的存放位置"也没有成功（<根>\\data-dir.json 无法写入）。'
                  '下次启动会按该文件中现在记录的位置执行，请到「设置 → 数据目录」确认。',
      );
      return;
    }

    // 位置记录已经指向新目录：用新目录启动程序，起不来就回退。
    final ok = await launcher.ensureRunning(force: true);
    if (!mounted) return;
    if (!ok) {
      final restored = writeDataDirOverride(root, previousPointer);
      final recovered = await launcher.ensureRunning(force: true);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _status = '';
      });
      // 回滚本身也可能失败：那时不能把"恢复原来的位置"说成已经发生
      final String tail;
      if (!restored) {
        tail = '。而且"恢复原来的存放位置"也没有成功（data-dir.json 无法写入），请到「设置 → 数据目录」确认';
      } else if (recovered) {
        tail = '，已恢复原来的位置并重新启动。';
      } else {
        tail = '，恢复原来的位置后也未能启动，请到「本地服务」查看运行记录。';
      }
      await _info('搬运完成，但程序无法从新位置启动', '内容已复制到 $to，但程序无法从该位置启动$tail');
      return;
    }

    setState(() {
      _busy = false;
      _status = '';
    });

    // 迁移完成后弹窗询问是否删除旧数据，默认保留。
    final remove = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('搬运完成'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('对话、图片和角色卡已保存到 $to，程序已从新位置重新启动。'),
            if (result.backupPath != null) ...[
              const SizedBox(height: 8),
              Text(
                '新的位置原本已有一份数据，这份数据已保存到 ${result.backupPath}，'
                '没有被覆盖。',
              ),
            ],
            const SizedBox(height: 12),
            Text('原来的文件夹仍然存在：$from'),
            const SizedBox(height: 8),
            const Text('确认对话、图片和角色卡都完整后，可以自行删除原来的文件夹。程序不会自动删除。'),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('暂不删除'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('删除原来的数据'),
          ),
        ],
      ),
    );

    if (remove == true) {
      // 搬到上一级时"旧数据目录"就是新产物目录：只清旧库，产物原地留着。
      final report = await deleteOldData(
        from,
        newStorageDir: resolvedStorageDirOf(to),
      );
      if (!mounted) return;
      if (report.failed.isEmpty) {
        _toast(
          '原来的数据已删除，共 ${report.deletedFiles} 个文件'
          '（${_size(report.freedBytes)}）。',
        );
      } else {
        await _info(
          '有一部分没有删除',
          '已删除 ${report.deletedFiles} 个文件（${_size(report.freedBytes)}），'
              '仍有 ${report.failed.length} 个无法删除，可能正被其他程序占用：\n\n'
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
