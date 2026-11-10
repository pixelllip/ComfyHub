import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/storage_models.dart';
import '../state/library_store.dart';

/// 「存储」卡（设置 → 本地服务）。
///
/// 三件事：**看得见**（各目录占了多少、`ai_run_events` 涨到多大）、**扫得着**
/// （磁盘上有、库里没有的文件）、**清得掉**（归档旧事件）。
///
/// # 为什么删除要做成两步
///
/// "孤儿"里有一类**不是垃圾**：恢复了更早的库备份之后，备份之后新生成的产物在库里
/// 没有记录 —— 直接删除会丢失最近的图片。该卡先扫描并显示数量与体积，
/// 确认后再删除。
///
/// # 为什么手机上不给按钮
///
/// 删除与归档在后端只允许回环调用（见 `web/storage.rs`），手机上点了只会拿到 403。
/// 占用照常显示（它只是读）。
class StorageCard extends StatefulWidget {
  const StorageCard({super.key});

  @override
  State<StorageCard> createState() => _StorageCardState();
}

class _StorageCardState extends State<StorageCard> {
  StorageUsage? _usage;
  String? _error;
  bool _loading = false;
  bool _busy = false;

  /// 删除 / 归档只在这台机器上做（后端只认回环，手机点了也是 403）。
  bool get _canManage => !kIsWeb && Platform.isWindows;

  @override
  void initState() {
    super.initState();
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
      final usage = await context.read<LibraryStore>().api.storageUsage();
      if (!mounted) return;
      setState(() => _usage = usage);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '读不到占用，$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _cleanOrphans() async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      final scan = await context.read<LibraryStore>().api.scanOrphans();
      if (!mounted) return;
      if (scan.count == 0) {
        messenger.showSnackBar(
          const SnackBar(content: Text('没有孤儿文件，磁盘上的东西在库里都有记录。')),
        );
        return;
      }
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: const Text('清理孤儿文件？'),
          content: Text(
            '有 ${scan.count} 个文件在库里没有记录，一共 ${formatSize(scan.bytes)}。'
            '它们可能是上传中断留下的；如果你恢复过更早的备份，备份之后新生成的产物也会出现在这里。'
            '删掉就找不回来了。',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: const Text('删掉'),
            ),
          ],
        ),
      );
      if (confirmed != true || !mounted) return;
      final report = await context.read<LibraryStore>().api.cleanOrphans();
      if (!mounted) return;
      await _load();
      if (!mounted) return;
      final more = report.remaining > 0
          ? '还有 ${report.remaining} 个，再点一次可以继续。'
          : '';
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            '删掉了 ${report.deleted} 个文件，腾出 ${formatSize(report.bytes)}。$more',
          ),
        ),
      );
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('清理失败，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _archiveEvents() async {
    final messenger = ScaffoldMessenger.of(context);
    final stats = _usage?.events;
    final rows = stats?.rows ?? 0;
    final oldest = stats?.oldest == null
        ? ''
        : '，最早的一条在 ${_when(stats!.oldest!)}';
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('归档旧事件？'),
        content: Text(
          '30 天前的运行事件会先写进 storage/ai/run-events 里的一个文件，再从库里删掉。'
          '库里现在有 $rows 条事件$oldest。正在跑的对话一条都不会动。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('归档'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    setState(() => _busy = true);
    try {
      final report = await context.read<LibraryStore>().api.archiveEvents();
      if (!mounted) return;
      await _load();
      if (!mounted) return;
      if (report.archived == 0) {
        messenger.showSnackBar(
          const SnackBar(content: Text('没有 30 天前的事件，库不用动。')),
        );
        return;
      }
      final name = report.file.split(RegExp(r'[\\/]')).last;
      final more = report.remaining > 0
          ? '还有 ${report.remaining} 条，再点一次可以继续。'
          : '';
      messenger.showSnackBar(
        SnackBar(content: Text('归档了 ${report.archived} 条事件，写进 $name。$more')),
      );
    } catch (e) {
      if (!mounted) return;
      messenger.showSnackBar(SnackBar(content: Text('归档失败，$e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String _when(String iso) {
    final dt = DateTime.tryParse(iso)?.toLocal();
    if (dt == null) return iso;
    String two(int n) => n.toString().padLeft(2, '0');
    return '${dt.year}-${two(dt.month)}-${two(dt.day)}';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final usage = _usage;
    final events = usage?.events;

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('备份、产物和 AI 数据都放在这台机器上。这里只显示占用，点下面的按钮才会删东西。', style: muted),
            if (_error != null) ...[
              const SizedBox(height: 10),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    Icons.error_outline,
                    size: 18,
                    color: theme.colorScheme.error,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      _error!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ),
                  TextButton(
                    onPressed: _loading ? null : _load,
                    child: const Text('重试'),
                  ),
                ],
              ),
            ],
            if (_loading && usage == null)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: LinearProgressIndicator(),
              )
            else if (usage != null) ...[
              const SizedBox(height: 8),
              Row(
                children: [
                  Expanded(
                    child: Text('总共', style: theme.textTheme.titleSmall),
                  ),
                  Text(
                    formatSize(usage.totalBytes),
                    style: theme.textTheme.titleSmall,
                  ),
                ],
              ),
              const Divider(height: 16),
              for (final dir in usage.dirs)
                _row(
                  context,
                  label: dir.label,
                  trailing: dir.files > 0
                      ? '${formatSize(dir.bytes)} · ${dir.files} 个文件'
                      : formatSize(dir.bytes),
                  muted: dir.bytes == 0,
                ),
              const Divider(height: 16),
              _row(
                context,
                label: '运行事件',
                trailing: events == null || events.rows == 0
                    ? '还没有'
                    : '${events.rows} 条',
                muted: events == null || events.rows == 0,
              ),
              if (events != null && events.rows > 0)
                Padding(
                  padding: const EdgeInsets.only(left: 0, top: 2),
                  child: Text(
                    '最早一条在 ${_when(events.oldest ?? '')}',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
            ],
            if (!_canManage) ...[
              const SizedBox(height: 8),
              Text('清理与归档只能在运行后端的电脑上操作。', style: muted),
            ],
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                OutlinedButton.icon(
                  onPressed: _loading ? null : _load,
                  icon: const Icon(Icons.refresh, size: 18),
                  label: const Text('刷新'),
                ),
                if (_canManage) ...[
                  TextButton.icon(
                    onPressed: (_busy || _loading) ? null : _cleanOrphans,
                    icon: const Icon(
                      Icons.cleaning_services_outlined,
                      size: 18,
                    ),
                    label: const Text('清理孤儿文件…'),
                  ),
                  TextButton.icon(
                    onPressed: (_busy || _loading) ? null : _archiveEvents,
                    icon: const Icon(Icons.inventory_2_outlined, size: 18),
                    label: const Text('归档旧事件…'),
                  ),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _row(
    BuildContext context, {
    required String label,
    required String trailing,
    bool muted = false,
  }) {
    final theme = Theme.of(context);
    // 右侧那一列**限宽**：320 宽的手机上 "1234 条 · 最早 2026-09-01" 这种长串
    // 会把 Row 顶出屏幕（实测溢出 29px）。限到 62% 之后放不下就换行 + 省略。
    return LayoutBuilder(
      builder: (context, constraints) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 3),
        child: Row(
          children: [
            Expanded(
              child: Text(
                label,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: muted ? theme.colorScheme.onSurfaceVariant : null,
                ),
              ),
            ),
            const SizedBox(width: 12),
            ConstrainedBox(
              constraints: BoxConstraints(
                maxWidth: constraints.maxWidth * 0.62,
              ),
              child: Text(
                trailing,
                textAlign: TextAlign.right,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
