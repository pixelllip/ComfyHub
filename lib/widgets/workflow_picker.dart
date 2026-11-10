import 'dart:async';

import 'package:flutter/material.dart';

import '../models/models.dart';

/// 「选择现有工作流」对话框：列出本机 ComfyUI 中已保存的工作流。
///
/// - 提供选择现有工作流的入口，不生成新的工作流；
/// - 选中的路径写回输入框，明确指定使用该份工作流。
///
/// 该对话框只做**选择**，不提供「看蓝图」按钮（蓝图仍可从工作流卡片 /
/// 详情页的「查看工作流」进入）。
///
/// 只读：列表走 `GET /api/capture/workflows`（路径准入在后端，前端不拼盘路径）。
Future<ComfyWorkflowFile?> showWorkflowPicker(
  BuildContext context, {
  required Future<ComfyWorkflowList> Function({String? q}) loadList,
}) {
  return showDialog<ComfyWorkflowFile>(
    context: context,
    builder: (_) => _WorkflowPickerDialog(loadList: loadList),
  );
}

class _WorkflowPickerDialog extends StatefulWidget {
  final Future<ComfyWorkflowList> Function({String? q}) loadList;

  const _WorkflowPickerDialog({required this.loadList});

  @override
  State<_WorkflowPickerDialog> createState() => _WorkflowPickerDialogState();
}

class _WorkflowPickerDialogState extends State<_WorkflowPickerDialog> {
  final _query = TextEditingController();
  Timer? _debounce;

  bool _loading = true;
  String? _error;
  ComfyWorkflowList? _list;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _query.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final list = await widget.loadList(
        q: _query.text.trim().isEmpty ? null : _query.text.trim(),
      );
      if (!mounted) return;
      setState(() {
        _loading = false;
        _list = list;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = '$e';
      });
    }
  }

  void _onQueryChanged(String _) {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 280), () {
      if (mounted) _load();
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final size = MediaQuery.sizeOf(context);
    final width = (size.width - 64).clamp(320.0, 760.0).toDouble();
    final height = (size.height - 150).clamp(320.0, 600.0).toDouble();
    return AlertDialog(
      title: Row(
        children: [
          const Icon(Icons.account_tree_outlined, size: 20),
          const SizedBox(width: 8),
          const Expanded(child: Text('选择现有工作流')),
          IconButton(
            tooltip: '刷新',
            onPressed: _loading ? null : _load,
            icon: const Icon(Icons.refresh, size: 19),
          ),
        ],
      ),
      content: SizedBox(
        width: width,
        height: height,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _query,
              onChanged: _onQueryChanged,
              decoration: const InputDecoration(
                isDense: true,
                prefixIcon: Icon(Icons.search, size: 19),
                hintText: '按文件名筛选（例如 krea2 / moss / anima）',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 10),
            Expanded(child: _body(theme)),
            const SizedBox(height: 6),
            Text(
              '这些是 ComfyUI 里已经保存的工作流。选一份后会把路径写进输入框 —— '
              '补一句「想生成什么」再发送，AI 就会基于这一份出图（不会改你的原文件）。',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
      ],
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading && _list == null) {
      return const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircularProgressIndicator(),
            SizedBox(height: 12),
            Text('正在读本机工作流列表…'),
          ],
        ),
      );
    }
    final error = _error;
    if (error != null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.cloud_off_outlined,
              size: 42,
              color: theme.colorScheme.error,
            ),
            const SizedBox(height: 10),
            SelectableText(error, textAlign: TextAlign.center),
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: _load,
              icon: const Icon(Icons.refresh),
              label: const Text('重试'),
            ),
          ],
        ),
      );
    }
    final list = _list;
    if (list == null) return const SizedBox.shrink();
    if (list.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.folder_off_outlined,
              size: 42,
              color: theme.colorScheme.onSurfaceVariant,
            ),
            const SizedBox(height: 10),
            Text(
              list.message ??
                  '没找到本机 ComfyUI 的工作流。可以先去「设置 → ComfyUI 自动捕获 → 导入本机工作流」。',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall,
            ),
          ],
        ),
      );
    }
    return ListView.separated(
      itemCount: list.files.length,
      separatorBuilder: (_, _) => const Divider(height: 1),
      itemBuilder: (context, index) {
        final file = list.files[index];
        return ListTile(
          dense: true,
          visualDensity: VisualDensity.compact,
          leading: Icon(
            file.format == 'api'
                ? Icons.data_object
                : Icons.account_tree_outlined,
            size: 19,
            color: theme.colorScheme.primary,
          ),
          title: Text(
            file.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelLarge,
          ),
          subtitle: Text(
            '${file.formatLabel} · ${file.sizeLabel}${file.inLibrary ? ' · 已入库' : ''}\n${file.path}',
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall,
          ),
          isThreeLine: true,
          onTap: () => Navigator.of(context).pop(file),
          // 只做选择：不提供「看蓝图」入口。
          trailing: const Icon(Icons.chevron_right, size: 20),
        );
      },
    );
  }
}
