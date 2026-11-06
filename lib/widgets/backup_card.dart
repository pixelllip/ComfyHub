import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/backup_models.dart';
import '../state/library_store.dart';

/// 「备份与恢复」卡（设置 → 本地服务）。
///
/// # 为什么恢复要重启
///
/// 换库只能发生在**没有任何连接**的时候（后端 `infra/backup.rs` 的文件头写了理由）。
/// 这条卡片的顺序是：`POST /api/system/restore`（校验 + 拍保险 + 落标记）→
/// 重启本地服务 → 重启时 `open_or_create` 看到标记才真的换文件 → 再读一次列表，
/// 把后端写下的 `lastRestore` 原样说给用户听。
///
/// # 只备份库
///
/// 产物原图在 `<根>\storage`，几十 GB，不在备份里。这一点必须写在卡片上 ——
/// 用户以为"备份了全部"、真出事时才发现图没了，比没有这个功能更糟。
class BackupCard extends StatefulWidget {
  const BackupCard({super.key, this.onRestart});

  /// 恢复之后"重启本地服务"这一步。**只给测试注入**；生产不传，
  /// 走 [BackendLauncher.runAction]（与设置页那颗「重启」按钮同一个出口）。
  final Future<void> Function()? onRestart;

  @override
  State<BackupCard> createState() => _BackupCardState();
}

class _BackupCardState extends State<BackupCard> {
  BackupListing? _listing;
  String? _error;
  bool _loading = false;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    // 页面刚上来时后端可能还没起：失败就画一行"读不到"，别把整页拖红
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _load();
    });
  }

  Future<void> _load() async {
    if (_loading) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      await _reload();
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '读不到备份列表，$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _reload() async {
    final listing = await context.read<LibraryStore>().api.listBackups();
    if (!mounted) return;
    setState(() {
      _listing = listing;
      _error = null;
    });
  }

  Future<void> _createBackup() async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      final item = await context.read<LibraryStore>().api.createBackup();
      await _reload();
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('备份好了，${item.name}')));
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('备份失败，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _delete(BackupItem item) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('删掉这份备份？'),
        content: Text('删掉之后就不在列表里了，也没法再恢复它。${item.name}'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('删掉')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      await context.read<LibraryStore>().api.deleteBackup(item.name);
      await _reload();
      if (!mounted) return;
      messenger.showSnackBar(const SnackBar(content: Text('那份备份已经删掉了')));
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('删不掉，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _restore(BackupItem item) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('恢复这份备份？'),
        content: Text(
          '当前数据库会被「${item.name}」替换。恢复前会自动留一份保险，选错了还能换回来。'
          '产物原图不受影响。恢复完成后本地服务会自动重启一次。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('恢复'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;

    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      final prepared = await context.read<LibraryStore>().api.restoreBackup(item.name);
      // 标记已经落好：重启一次，后端启动时才会真的换文件
      await _restartService();
      await _reload();
      if (!mounted) return;
      final last = _listing?.lastRestore;
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            (last != null && last.message.isNotEmpty)
                ? last.message
                : '已准备好恢复 ${prepared.name}，本地服务正在重启。',
          ),
        ),
      );
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('恢复失败，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 「标记已落、等着换库」时补上最后一步。
  Future<void> _finishPending() async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      await _restartService();
      await _reload();
      if (!mounted) return;
      final last = _listing?.lastRestore;
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            (last != null && last.message.isNotEmpty)
                ? last.message
                : '本地服务已重启。',
          ),
        ),
      );
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('重启失败，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _restartService() async {
    if (widget.onRestart != null) {
      await widget.onRestart!();
      return;
    }
    final launcher = context.read<BackendLauncher>();
    final settings = context.read<SettingsStore>();
    final store = context.read<LibraryStore>();
    await launcher.runAction('restart');
    if (!mounted) return;
    // 端口是自动挑的：重启之后必须重新发现，否则后面读的还是旧端口
    try {
      await settings.refreshBaseUrlFromPortFile();
    } catch (_) {
      // 重新发现失败不该把"恢复成功"变成一条报错
    }
    if (!mounted) return;
    try {
      await store.refreshAll();
    } catch (_) {
      // 同上：库已经换了，列表刷新失败只影响这一屏
    }
  }

  Future<void> _openFolder(String dir) async {
    if (dir.isEmpty) return;
    try {
      await launchUrl(Uri.directory(dir));
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('打不开，$e')));
    }
  }

  /// 恢复**只能在这台运行后端的电脑上**做：后端那条端点只认回环调用，
  /// 而且换完库之后要由本机把服务重启一次 —— 手机上两件事都做不到。
  /// 列出备份 / 立刻备份 / 删除在手机上仍然可用（它们只碰备份目录）。
  bool get _canRestore => !kIsWeb && Platform.isWindows;

  String _when(String iso) {
    final dt = DateTime.tryParse(iso)?.toLocal();
    if (dt == null) return iso;
    String two(int n) => n.toString().padLeft(2, '0');
    return '${dt.year}-${two(dt.month)}-${two(dt.day)} ${two(dt.hour)}:${two(dt.minute)}';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final items = _listing?.items ?? const <BackupItem>[];

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '备份只保存数据库里的内容，提示词、标签、角色卡、画风卡、会话和工作流配方都在里面。'
              '产物原图本来就在 storage 里，不在备份内，恢复也不会动到它。',
              style: muted,
            ),
            if (_error != null) ...[
              const SizedBox(height: 10),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(Icons.error_outline, size: 18, color: theme.colorScheme.error),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      _error!,
                      style: theme.textTheme.bodySmall
                          ?.copyWith(color: theme.colorScheme.error),
                    ),
                  ),
                  TextButton(onPressed: _loading ? null : _load, child: const Text('重试')),
                ],
              ),
            ],
            if (!_canRestore) ...[
              const SizedBox(height: 8),
              Text('恢复只能在运行后端的电脑上操作，这里只列出备份。', style: muted),
            ],
            if (_canRestore && _listing?.pending != null) ...[
              const SizedBox(height: 10),
              _Banner(
                icon: Icons.restart_alt,
                color: theme.colorScheme.primary,
                text: '已准备好恢复到「${_listing!.pending}」，重启本地服务后生效。',
                action: _busy
                    ? null
                    : TextButton(onPressed: _finishPending, child: const Text('重启并完成')),
              ),
            ],
            if (_listing?.lastRestore != null) ...[
              const SizedBox(height: 10),
              _Banner(
                icon: _listing!.lastRestore!.ok
                    ? Icons.check_circle_outline
                    : Icons.error_outline,
                color: _listing!.lastRestore!.ok
                    ? theme.colorScheme.primary
                    : theme.colorScheme.error,
                text: _listing!.lastRestore!.message,
                trailing: Text(
                  _when(_listing!.lastRestore!.at),
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ),
            ],
            const SizedBox(height: 8),
            if (_loading && _listing == null)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: LinearProgressIndicator(),
              )
            else if (items.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Text('还没有备份。点「立即备份」拍一张。', style: muted),
              )
            else
              for (final item in items) _row(context, item),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                FilledButton.icon(
                  onPressed: (_busy || _loading) ? null : _createBackup,
                  icon: const Icon(Icons.save_outlined, size: 18),
                  label: const Text('立即备份'),
                ),
                OutlinedButton.icon(
                  onPressed: _loading ? null : _load,
                  icon: const Icon(Icons.refresh, size: 18),
                  label: const Text('刷新'),
                ),
                TextButton.icon(
                  onPressed: (_listing?.dir.isNotEmpty ?? false)
                      ? () => _openFolder(_listing!.dir)
                      : null,
                  icon: const Icon(Icons.folder_open, size: 18),
                  label: const Text('打开备份目录'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _row(BuildContext context, BackupItem item) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          Icon(
            item.isSafety ? Icons.health_and_safety_outlined : Icons.archive_outlined,
            size: 18,
            color: theme.colorScheme.onSurfaceVariant,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  item.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.bodyMedium,
                ),
                Text(
                  '${formatSize(item.bytes)} · ${_when(item.createdAt)}'
                  '${item.isSafety ? ' · 恢复前自动留的' : ''}',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
          if (_canRestore)
            TextButton(
              onPressed: _busy ? null : () => _restore(item),
              child: const Text('恢复'),
            ),
          IconButton(
            tooltip: '删掉这份备份',
            onPressed: _busy ? null : () => _delete(item),
            icon: const Icon(Icons.delete_outline, size: 18),
          ),
        ],
      ),
    );
  }
}

class _Banner extends StatelessWidget {
  const _Banner({
    required this.icon,
    required this.color,
    required this.text,
    this.action,
    this.trailing,
  });

  final IconData icon;
  final Color color;
  final String text;
  final Widget? action;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 18, color: color),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(text, style: theme.textTheme.bodySmall),
                if (trailing != null) ...[
                  const SizedBox(height: 2),
                  trailing!,
                ],
              ],
            ),
          ),
          ?action,
        ],
      ),
    );
  }
}
