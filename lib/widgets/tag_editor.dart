import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../state/library_store.dart';
import 'common.dart';

/// 标签编辑控件：可展示已有标签、输入新标签、从词表里点选。
class TagEditor extends StatefulWidget {
  final List<String> value;
  final ValueChanged<List<String>> onChanged;
  final String label;
  final bool allowCreate;

  const TagEditor({
    super.key,
    required this.value,
    required this.onChanged,
    this.label = '标签',
    this.allowCreate = true,
  });

  @override
  State<TagEditor> createState() => _TagEditorState();
}

class _TagEditorState extends State<TagEditor> {
  final _controller = TextEditingController();
  final _focusNode = FocusNode();
  bool _showSuggestions = false;

  @override
  void dispose() {
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _add(String raw) {
    final name = raw.trim();
    if (name.isEmpty) return;
    if (widget.value.contains(name)) {
      _controller.clear();
      return;
    }
    widget.onChanged([...widget.value, name]);
    _controller.clear();
    setState(() {});
  }

  void _remove(String name) {
    widget.onChanged(widget.value.where((e) => e != name).toList());
    setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final allTags = context.watch<LibraryStore>().tags;
    final q = _controller.text.trim().toLowerCase();
    final suggestions = allTags
        .where((t) =>
            !widget.value.contains(t.name) &&
            (q.isEmpty || t.name.toLowerCase().contains(q)))
        .take(q.isEmpty ? 14 : 8)
        .toList();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(widget.label, style: theme.textTheme.labelLarge),
        const SizedBox(height: 6),
        if (widget.value.isNotEmpty)
          Wrap(
            children: [
              for (final name in widget.value)
                TagChip(
                  label: name,
                  selected: true,
                  onDeleted: () => _remove(name),
                  onTap: () => _remove(name),
                ),
            ],
          ),
        if (widget.value.isNotEmpty) const SizedBox(height: 4),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _controller,
                focusNode: _focusNode,
                decoration: InputDecoration(
                  hintText: widget.allowCreate ? '输入标签后回车，例如：赛博朋克' : '搜索标签',
                  prefixIcon: const Icon(Icons.sell_outlined, size: 18),
                ),
                onChanged: (_) => setState(() => _showSuggestions = true),
                onSubmitted: _add,
                onTap: () => setState(() => _showSuggestions = true),
              ),
            ),
            const SizedBox(width: 8),
            FilledButton.tonal(
              onPressed: () => _add(_controller.text),
              child: const Text('添加'),
            ),
          ],
        ),
        if (_showSuggestions && suggestions.isNotEmpty) ...[
          const SizedBox(height: 8),
          Text(
            q.isEmpty ? '常用标签' : '匹配的标签',
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
          ),
          const SizedBox(height: 4),
          Wrap(
            children: [
              for (final tag in suggestions)
                TagChip(
                  label: tag.name,
                  count: tag.useCount,
                  color: parseHexColor(tag.color),
                  dense: true,
                  onTap: () => _add(tag.name),
                ),
            ],
          ),
        ],
      ],
    );
  }
}

/// 标签筛选面板：在画廊 / 提示词库侧边用
class TagFilterPanel extends StatelessWidget {
  final Set<String> selected;
  final String? category;
  final ValueChanged<String> onToggle;
  final ValueChanged<String?> onCategory;
  final VoidCallback onClear;

  const TagFilterPanel({
    super.key,
    required this.selected,
    required this.category,
    required this.onToggle,
    required this.onCategory,
    required this.onClear,
  });

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();
    final theme = Theme.of(context);
    final tags = store.tags.where((t) => category == null || t.category == category).toList();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('标签筛选', style: theme.textTheme.titleSmall),
            const Spacer(),
            if (selected.isNotEmpty)
              TextButton(onPressed: onClear, child: const Text('清空')),
          ],
        ),
        const SizedBox(height: 4),
        Wrap(
          spacing: 6,
          children: [
            ChoiceChip(
              label: const Text('全部'),
              selected: category == null,
              onSelected: (_) => onCategory(null),
            ),
            for (final c in store.tagCategories)
              ChoiceChip(
                label: Text(c),
                selected: category == c,
                onSelected: (_) => onCategory(c),
              ),
          ],
        ),
        const SizedBox(height: 8),
        Expanded(
          child: tags.isEmpty
              ? Text('暂无标签', style: theme.textTheme.bodySmall)
              : SingleChildScrollView(
                  child: Wrap(
                    children: [
                      for (final tag in tags)
                        TagChip(
                          label: tag.name,
                          count: tag.useCount,
                          color: parseHexColor(tag.color),
                          selected: selected.contains(tag.name),
                          dense: true,
                          onTap: () => onToggle(tag.name),
                        ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }
}
