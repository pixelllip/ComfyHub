import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';

/// 打开"导出产物"弹窗。
///
/// 四个入口共用这一个组件：
/// * 画廊顶栏 → 全部（或仅收藏）；
/// * 画廊多选 → 选中的 [mediaIds]（集合格子已经被展开成成员 id）；
/// * 产物详情页「更多」→ 这一条（[mediaIds] 只有一项）；
/// * 设置页 → 全部。
///
/// [collectionLabel] 只用于把范围说清楚（"集合「夜景三连」"）。
Future<MediaExportReport?> showExportDialog(
  BuildContext context, {
  List<int>? mediaIds,
  int? collectionId,
  String? collectionLabel,
}) {
  return showDialog<MediaExportReport>(
    context: context,
    // 点弹窗外面就能关（用户 2026-11-05："这个窗口不允许我点击弹窗以外的区域
    // 以退出"）。导出进行中的误点由弹窗内部的 `PopScope(canPop: !_busy)` 挡住，
    // 所以这里放开不会把正在写的文件关掉。
    barrierDismissible: true,
    builder: (_) => _ExportDialog(
      mediaIds: mediaIds,
      collectionId: collectionId,
      collectionLabel: collectionLabel,
    ),
  );
}

class _ExportDialog extends StatefulWidget {
  final List<int>? mediaIds;
  final int? collectionId;
  final String? collectionLabel;

  const _ExportDialog({this.mediaIds, this.collectionId, this.collectionLabel});

  @override
  State<_ExportDialog> createState() => _ExportDialogState();
}

class _ExportDialogState extends State<_ExportDialog> {
  final _dirController = TextEditingController();

  bool _onlyFavorites = false;
  bool _includeWorkflow = true;
  bool _includeApiWorkflow = true;
  bool _busy = false;
  String? _error;
  MediaExportReport? _report;

  bool get _fixedScope =>
      widget.mediaIds != null || widget.collectionId != null;

  @override
  void dispose() {
    _dirController.dispose();
    super.dispose();
  }

  Future<void> _pickFolder() async {
    final picked = await FilePicker.getDirectoryPath(dialogTitle: '选择产物导出目录');
    if (picked == null || picked.isEmpty || !mounted) return;
    setState(() => _dirController.text = picked);
  }

  Future<void> _export() async {
    final dir = _dirController.text.trim();
    if (dir.isEmpty) {
      setState(() => _error = '先选一个导出目录。');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
      _report = null;
    });
    try {
      final report = await context.read<LibraryStore>().api.exportMedia(
        dir: dir,
        mediaIds: widget.mediaIds,
        collectionId: widget.collectionId,
        favorites: _fixedScope ? false : _onlyFavorites,
        includeWorkflow: _includeWorkflow,
        includeApiWorkflow: _includeApiWorkflow,
      );
      if (!mounted) return;
      setState(() => _report = report);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _openFolder() async {
    final dir = _report?.dir ?? _dirController.text.trim();
    if (dir.isEmpty) return;
    try {
      final opened = await launchUrl(Uri.directory(dir));
      if (!opened && mounted) {
        setState(() => _error = '系统没有打开这个文件夹，$dir');
      }
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '打开文件夹失败，$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final report = _report;
    return PopScope(
      canPop: !_busy,
      child: AlertDialog(
        title: Text(report == null ? '导出产物' : '导出结果'),
        scrollable: true,
        content: SizedBox(
          width: 460,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: report == null
                ? _buildForm(theme)
                : _buildReport(theme, report),
          ),
        ),
        actions: report == null
            ? [
                TextButton(
                  onPressed: _busy ? null : () => Navigator.of(context).pop(),
                  child: const Text('取消'),
                ),
                FilledButton.icon(
                  onPressed: _busy ? null : _export,
                  icon: _busy
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.save_alt, size: 18),
                  label: Text(_busy ? '正在导出…' : '开始导出'),
                ),
              ]
            : [
                TextButton.icon(
                  onPressed: _openFolder,
                  icon: const Icon(Icons.folder_open, size: 18),
                  label: const Text('打开文件夹'),
                ),
                FilledButton(
                  onPressed: () => Navigator.of(context).pop(report),
                  child: const Text('关闭'),
                ),
              ],
      ),
    );
  }

  List<Widget> _buildForm(ThemeData theme) {
    final scope = _scopeText();
    return [
      Text(
        '把原件连同它绑定的工作流 / 提示词信息一起写进你选的文件夹。'
        '目录按图片、视频、音频、其它分开，集合作品单独一个文件夹。',
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
      const SizedBox(height: 16),
      Text('范围', style: theme.textTheme.labelLarge),
      const SizedBox(height: 4),
      if (_fixedScope)
        Text(scope, style: theme.textTheme.bodyMedium)
      else ...[
        _scopeRow(theme, label: '全部产物', value: false),
        _scopeRow(theme, label: '仅收藏', value: true),
      ],
      const SizedBox(height: 16),
      Text('导出到', style: theme.textTheme.labelLarge),
      const SizedBox(height: 6),
      Row(
        children: [
          Expanded(
            child: TextField(
              controller: _dirController,
              enabled: !_busy,
              decoration: const InputDecoration(
                isDense: true,
                hintText: r'例如 D:\导出\ComfyHub',
                border: OutlineInputBorder(),
              ),
              onChanged: (_) {
                if (_error != null) setState(() => _error = null);
              },
            ),
          ),
          const SizedBox(width: 8),
          OutlinedButton.icon(
            onPressed: _busy ? null : _pickFolder,
            icon: const Icon(Icons.folder_open, size: 18),
            label: const Text('选择…'),
          ),
        ],
      ),
      const SizedBox(height: 8),
      Text(
        '直接写进这个文件夹（不新建子文件夹）。同名文件不会被覆盖，会自动加序号。',
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      ),
      const SizedBox(height: 12),
      CheckboxListTile(
        value: _includeWorkflow,
        onChanged: _busy
            ? null
            : (value) => setState(() => _includeWorkflow = value ?? false),
        contentPadding: EdgeInsets.zero,
        dense: true,
        controlAffinity: ListTileControlAffinity.leading,
        title: const Text('包含界面格式工作流'),
        subtitle: const Text('同内容只落一份，放在「工作流」文件夹里'),
      ),
      CheckboxListTile(
        value: _includeApiWorkflow,
        onChanged: _busy
            ? null
            : (value) => setState(() => _includeApiWorkflow = value ?? false),
        contentPadding: EdgeInsets.zero,
        dense: true,
        controlAffinity: ListTileControlAffinity.leading,
        title: const Text('包含 API 格式节点图'),
        subtitle: const Text('有捕获记录的产物才带，这是当时真正提交给 ComfyUI 的那份'),
      ),
      if (_error != null) ...[
        const SizedBox(height: 8),
        Text(
          _error!,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.error,
          ),
        ),
      ],
    ];
  }

  List<Widget> _buildReport(ThemeData theme, MediaExportReport report) {
    final parts = <String>[];
    for (final entry in report.byKind.entries) {
      final label = switch (entry.key) {
        'IMAGE' => '图片',
        'VIDEO' => '视频',
        'AUDIO' => '音频',
        _ => '其它',
      };
      if (entry.value > 0) parts.add('$label ${entry.value}');
    }
    return [
      Row(
        children: [
          Icon(
            report.hasFailures
                ? Icons.warning_amber_outlined
                : Icons.check_circle_outline,
            color: report.hasFailures
                ? theme.colorScheme.error
                : theme.colorScheme.primary,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              report.hasFailures ? '导出完成，但有条目没导出去' : '导出完成',
              style: theme.textTheme.titleSmall,
            ),
          ),
        ],
      ),
      const SizedBox(height: 12),
      Text(
        '产物 ${report.exported} 个'
        '${parts.isEmpty ? '' : '（${parts.join(' · ')}）'}'
        ' · 集合 ${report.collections} 个'
        ' · 工作流 ${report.workflows} 份'
        ' · ${formatSize(report.totalBytes)}',
        style: theme.textTheme.bodyMedium,
      ),
      const SizedBox(height: 8),
      SelectableText('目录是 ${report.dir}', style: theme.textTheme.bodySmall),
      const SizedBox(height: 4),
      SelectableText(
        '导出清单是 ${report.manifest}',
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      ),
      if (report.failed.isNotEmpty) ...[
        const Divider(height: 24),
        Text(
          '没导出的（${report.failed.length}）',
          style: theme.textTheme.labelLarge,
        ),
        const SizedBox(height: 4),
        for (final failure in report.failed.take(6))
          Padding(
            padding: const EdgeInsets.only(bottom: 2),
            child: Text(
              '· ${failure.name ?? '#${failure.mediaId}'}，${failure.reason}',
              style: theme.textTheme.bodySmall,
            ),
          ),
        if (report.failed.length > 6)
          Text(
            '……还有 ${report.failed.length - 6} 条，见导出清单。',
            style: theme.textTheme.labelSmall,
          ),
      ],
      if (report.warnings.isNotEmpty) ...[
        const Divider(height: 24),
        Text(
          '附带信息没写全（${report.warnings.length}）',
          style: theme.textTheme.labelLarge,
        ),
        const SizedBox(height: 4),
        for (final warning in report.warnings.take(4))
          Padding(
            padding: const EdgeInsets.only(bottom: 2),
            child: Text('· $warning', style: theme.textTheme.bodySmall),
          ),
      ],
      if (_error != null) ...[
        const SizedBox(height: 8),
        Text(
          _error!,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.error,
          ),
        ),
      ],
    ];
  }

  String _scopeText() {
    if (widget.mediaIds != null) {
      return '选中的 ${widget.mediaIds!.length} 个产物';
    }
    if (widget.collectionId != null) {
      final label = widget.collectionLabel?.trim() ?? '';
      return label.isEmpty ? '整个集合' : '集合「$label」';
    }
    return _onlyFavorites ? '仅收藏' : '全部产物';
  }

  /// 不用 `RadioListTile` / `RadioGroup`：前者在新 SDK 上已弃用，
  /// 而后者一来版本参差（本仓库钉 3.13 之外还要能构建），
  /// 二来这一行本来就是个二选一，没必要为它引一层分组状态。
  Widget _scopeRow(
    ThemeData theme, {
    required String label,
    required bool value,
  }) {
    final selected = _onlyFavorites == value;
    return ListTile(
      dense: true,
      contentPadding: EdgeInsets.zero,
      visualDensity: VisualDensity.compact,
      leading: Icon(
        selected ? Icons.radio_button_checked : Icons.radio_button_off,
        size: 20,
        color: selected ? theme.colorScheme.primary : theme.colorScheme.outline,
      ),
      title: Text(label),
      onTap: () => setState(() => _onlyFavorites = value),
    );
  }
}
