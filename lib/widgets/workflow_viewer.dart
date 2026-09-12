import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// 这份 JSON 是不是「API 格式节点图」。
///
/// 界面格式工作流长这样：`{"nodes": [...], "links": [...], "version": 0.4}`；
/// API 格式是 `{"1": {"class_type": "KSampler", "inputs": {...}}, ...}` —— 每个值都是带 `class_type` 的对象。
/// agent / 脚本直接调 `/prompt` 生图时只会留下后者，提示语要跟着变，别让用户以为
/// "拖回去就是原来的界面工作流"。
bool looksLikeApiGraph(Object? json) {
  if (json is! Map || json.isEmpty) return false;
  return json.values.every((v) => v is Map && v.containsKey('class_type'));
}

/// ComfyUI 工作流查看器。
///
/// ComfyUI 存下来的是 UI 格式的工作流 JSON，用户把它拖回 ComfyUI 就能复现同一次生成。
/// 所以这里只做一件事：把后端存的原文完整、可选中地交到用户手上。

/// 单次渲染的字符上限。
///
/// 工作流通常只有几十 KB，但历史导入的大工作流可能上 MB；一次性丢给
/// SelectableText 布局会把界面卡住好几秒，所以超过这个量只渲染开头。
const int _maxDisplayChars = 400 * 1024;

/// 打开工作流弹窗。
///
/// [load] 返回 null 表示后端没有存过这次运行的工作流。
Future<void> showWorkflowDialog(
  BuildContext context, {
  required String title,
  required Future<String?> Function() load,
}) {
  // 在打开弹窗时就抓住调用方的 ScaffoldMessenger：弹窗自身的 context 不一定
  // 落在某个 Scaffold 之下，用调用方的那个才能把 SnackBar 稳定地弹出来。
  final messenger = ScaffoldMessenger.maybeOf(context);
  return showDialog<void>(
    context: context,
    builder: (_) => _WorkflowDialog(title: title, load: load, messenger: messenger),
  );
}

/// 详情页里的「查看工作流」按钮
class WorkflowButton extends StatelessWidget {
  final String label;
  final String title;
  final Future<String?> Function() load;

  const WorkflowButton({
    super.key,
    this.label = '查看工作流',
    this.title = '工作流',
    required this.load,
  });

  @override
  Widget build(BuildContext context) {
    return TextButton.icon(
      onPressed: () => showWorkflowDialog(context, title: title, load: load),
      icon: const Icon(Icons.account_tree_outlined, size: 16),
      label: Text(label),
      style: TextButton.styleFrom(
        visualDensity: VisualDensity.compact,
        padding: const EdgeInsets.symmetric(horizontal: 8),
      ),
    );
  }
}

class _WorkflowDialog extends StatefulWidget {
  final String title;
  final Future<String?> Function() load;
  /// 打开弹窗的那个页面所属的 messenger
  final ScaffoldMessengerState? messenger;

  const _WorkflowDialog({required this.title, required this.load, this.messenger});

  @override
  State<_WorkflowDialog> createState() => _WorkflowDialogState();
}

class _WorkflowDialogState extends State<_WorkflowDialog> {
  bool _loading = true;
  String? _error;
  /// 后端返回的完整原文；null 表示这次运行没存工作流
  String? _raw;
  /// 实际渲染出来的文本（超大时是截断版）
  String _display = '';
  /// 存的是 API 格式节点图（agent / 脚本提交的运行）而不是界面工作流
  bool _apiFormat = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final text = await widget.load();
      if (!mounted) return;
      setState(() {
        _loading = false;
        _raw = text;
        _display = _render(text);
        _apiFormat = _detectApiFormat(text);
      });
    } catch (e) {
      // 读取失败就地展示 + 重试，不要把异常抛给上层把整页变成错误页
      if (!mounted) return;
      setState(() {
        _loading = false;
        _raw = null;
        _display = '';
        _error = '$e';
      });
    }
  }

  /// 解析失败就按界面格式处理（提示语保守一点没关系，内容本身照旧显示）
  bool _detectApiFormat(String? raw) {
    if (raw == null) return false;
    try {
      return looksLikeApiGraph(jsonDecode(raw));
    } catch (_) {
      return false;
    }
  }

  /// 得到真正要渲染的文本：正常情况下格式化 JSON，超大时只取开头一段
  String _render(String? raw) {
    if (raw == null) return '';
    if (raw.length <= _maxDisplayChars) return _pretty(raw);

    // 超大工作流跳过格式化：JsonEncoder 也要把整个对象遍历一遍，这一步省掉
    final end = _safeCut(raw, _maxDisplayChars);
    return '${raw.substring(0, end)}\n\n'
        '…（工作流超过 400 KB，这里只显示开头，已省略后面 ${raw.length - end} 个字符；'
        '点「复制」拿到的是完整内容）';
  }

  /// 从 [index] 处切开时不要把 UTF-16 代理对劈成两半，否则会渲染出乱码方块
  int _safeCut(String text, int index) {
    if (index >= text.length) return text.length;
    final prev = text.codeUnitAt(index - 1);
    return (prev >= 0xD800 && prev <= 0xDBFF) ? index - 1 : index;
  }

  static String _pretty(String raw) {
    try {
      return const JsonEncoder.withIndent('  ').convert(jsonDecode(raw));
    } catch (_) {
      // 后端理论上只会存 JSON；万一不是，原文也比报错强
      return raw;
    }
  }

  Future<void> _copy() async {
    final text = _raw;
    if (text == null) return;
    // 复制永远给完整原文，即使用户看到的是截断版
    await Clipboard.setData(ClipboardData(text: text));
    if (!mounted) return;
    widget.messenger?.showSnackBar(
      const SnackBar(
        content: Text('已复制，可以直接粘回 ComfyUI'),
        duration: Duration(milliseconds: 1600),
        behavior: SnackBarBehavior.floating,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 跟着窗口缩放，避免小窗口里内容把弹窗撑爆
    final size = MediaQuery.sizeOf(context);
    final width = (size.width - 120).clamp(280.0, 760.0).toDouble();
    final height = (size.height - 260).clamp(220.0, 460.0).toDouble();

    return AlertDialog(
      title: Row(
        children: [
          Expanded(
            child: Text(widget.title, maxLines: 1, overflow: TextOverflow.ellipsis),
          ),
          // 复制按钮放标题栏，正文区就能整块留给工作流
          if (_raw != null)
            TextButton.icon(
              onPressed: _copy,
              icon: const Icon(Icons.copy_all_outlined, size: 16),
              label: const Text('复制'),
            ),
        ],
      ),
      content: SizedBox(width: width, height: height, child: _body(theme)),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('关闭'),
        ),
      ],
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading) {
      return const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircularProgressIndicator(),
            SizedBox(height: 14),
            Text('正在读取工作流…'),
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
            Icon(Icons.cloud_off_outlined, size: 46, color: theme.colorScheme.error),
            const SizedBox(height: 12),
            SelectableText(
              error,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall,
            ),
            const SizedBox(height: 16),
            FilledButton.icon(
              onPressed: _load,
              icon: const Icon(Icons.refresh),
              label: const Text('重试'),
            ),
          ],
        ),
      );
    }

    if (_raw == null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.account_tree_outlined, size: 46, color: theme.colorScheme.outline),
            const SizedBox(height: 12),
            Text('这次生成没有保存工作流', style: theme.textTheme.titleSmall),
            const SizedBox(height: 8),
            Text(
              '只有 ComfyUI 自动捕获或历史导入的运行会带上工作流，手动新建的提示词没有。',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ],
        ),
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(Icons.info_outline, size: 14, color: theme.colorScheme.outline),
            const SizedBox(width: 6),
            Expanded(
              child: Text(
                _apiFormat
                    ? '这是提交给 ComfyUI 的 API 格式节点图（agent / 脚本生图只会留下这份）：'
                        '参数与连接都在里面，存成 .json 拖进 ComfyUI 就能加载'
                    : '这是 ComfyUI 的工作流 JSON，拖回 ComfyUI 即可复现',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Expanded(
          child: Container(
            width: double.infinity,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.5),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(color: theme.dividerColor.withValues(alpha: 0.4)),
            ),
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(12),
              child: SelectableText(
                _display,
                style: theme.textTheme.bodySmall?.copyWith(
                  fontFamily: 'monospace',
                  height: 1.45,
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
