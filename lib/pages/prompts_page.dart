import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import '../widgets/tag_editor.dart';
import 'prompt_detail_page.dart';
import 'prompt_edit_page.dart';

class PromptsPage extends StatefulWidget {
  const PromptsPage({super.key});

  @override
  State<PromptsPage> createState() => _PromptsPageState();
}

class _PromptsPageState extends State<PromptsPage> {
  final _searchController = TextEditingController();

  /// 批量管理：多选模式 + 已选中的提示词 id
  final Set<int> _selected = {};
  bool _selectionMode = false;
  bool _busy = false;

  @override
  void dispose() {
    _searchController.dispose();
    super.dispose();
  }

  void _exitSelection() {
    setState(() {
      _selectionMode = false;
      _selected.clear();
    });
  }

  void _toggle(int id) {
    setState(() {
      if (!_selected.remove(id)) _selected.add(id);
    });
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();

    return Scaffold(
      appBar: AppBar(
        leading: _selectionMode
            ? IconButton(
                tooltip: '退出多选',
                icon: const Icon(Icons.close),
                onPressed: _exitSelection,
              )
            : null,
        title: Text(_selectionMode ? '已选 ${_selected.length} 项' : '提示词库'),
        actions: _selectionMode
            ? [
                IconButton(
                  tooltip: '全选本页',
                  icon: const Icon(Icons.select_all),
                  onPressed: () => setState(() {
                    _selected
                      ..clear()
                      ..addAll(store.prompts.items.map((e) => e.id));
                  }),
                ),
                if (_busy)
                  const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 16),
                    child: Center(
                      child: SizedBox(
                          width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2)),
                    ),
                  ),
              ]
            : [
                IconButton(
                  tooltip: '批量管理',
                  icon: const Icon(Icons.checklist),
                  onPressed: () => setState(() => _selectionMode = true),
                ),
                IconButton(
                  tooltip: '刷新',
                  icon: const Icon(Icons.refresh),
                  onPressed: () => store.refreshAll(),
                ),
                const SizedBox(width: 4),
              ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(1),
          child: Divider(height: 1, color: Theme.of(context).dividerColor),
        ),
      ),
      floatingActionButton: _selectionMode && _selected.isNotEmpty
          ? null
          : FloatingActionButton.extended(
              onPressed: () async {
                final created = await Navigator.of(context).push<Prompt>(
                  MaterialPageRoute(builder: (_) => const PromptEditPage()),
                );
                if (created != null && context.mounted) {
                  context.read<LibraryStore>().refreshAll();
                }
              },
              icon: const Icon(Icons.add),
              label: const Text('新建提示词'),
            ),
      bottomNavigationBar: _selectionMode && _selected.isNotEmpty
          ? _SelectionBar(
              count: _selected.length,
              busy: _busy,
              onFavorite: () => _bulkFavorite(true),
              onUnfavorite: () => _bulkFavorite(false),
              onAddTags: _bulkAddTags,
              onDelete: _bulkDelete,
            )
          : null,
      body: Column(
        children: [
          _buildToolbar(context, store),
          Expanded(child: _buildBody(context, store)),
        ],
      ),
    );
  }

  Widget _buildToolbar(BuildContext context, LibraryStore store) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _searchController,
            textInputAction: TextInputAction.search,
            decoration: InputDecoration(
              hintText: '搜索标题 / 正向提示词 / 负向提示词 / 模型名…',
              prefixIcon: const Icon(Icons.search),
              suffixIcon: _searchController.text.isEmpty
                  ? null
                  : IconButton(
                      icon: const Icon(Icons.clear),
                      onPressed: () {
                        _searchController.clear();
                        store.setPromptQuery('');
                        setState(() {});
                      },
                    ),
            ),
            onChanged: (v) {
              setState(() {});
            },
            onSubmitted: store.setPromptQuery,
          ),
          const SizedBox(height: 10),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              children: [
                _kindFilter(store),
                const SizedBox(width: 8),
                FilterChip(
                  label: const Text('仅收藏'),
                  avatar: Icon(
                    store.onlyFavorite ? Icons.star : Icons.star_border,
                    size: 17,
                    color: store.onlyFavorite ? Colors.amber : null,
                  ),
                  selected: store.onlyFavorite,
                  onSelected: (v) => store.setOnlyFavorite(v),
                ),
                const SizedBox(width: 8),
                if (store.selectedTags.isNotEmpty) ...[
                  SegmentedButton<String>(
                    style: const ButtonStyle(visualDensity: VisualDensity.compact),
                    segments: const [
                      ButtonSegment(value: 'any', label: Text('任一标签')),
                      ButtonSegment(value: 'all', label: Text('全部标签')),
                    ],
                    selected: {store.tagMode},
                    onSelectionChanged: (s) => store.setTagMode(s.first),
                  ),
                  const SizedBox(width: 8),
                ],
                PopupMenuButton<String>(
                  tooltip: '排序',
                  initialValue: store.promptSort,
                  onSelected: store.setPromptSort,
                  itemBuilder: (_) => const [
                    PopupMenuItem(value: 'newest', child: Text('最新创建')),
                    PopupMenuItem(value: 'oldest', child: Text('最旧创建')),
                    PopupMenuItem(value: 'updated', child: Text('最近修改')),
                    PopupMenuItem(value: 'title', child: Text('按标题')),
                    PopupMenuItem(value: 'favorite', child: Text('收藏优先')),
                  ],
                  child: Chip(
                    avatar: const Icon(Icons.sort, size: 16),
                    label: Text(_sortLabel(store.promptSort)),
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  '共 ${store.prompts.total} 条',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ],
            ),
          ),
          if (store.selectedTags.isNotEmpty) ...[
            const SizedBox(height: 8),
            Wrap(
              children: [
                for (final t in store.selectedTags)
                  TagChip(
                    label: t,
                    selected: true,
                    onDeleted: () => store.toggleTag(t),
                    onTap: () => store.toggleTag(t),
                  ),
                TextButton.icon(
                  onPressed: store.clearTags,
                  icon: const Icon(Icons.clear_all, size: 16),
                  label: const Text('清空标签'),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  String _sortLabel(String sort) => switch (sort) {
        'oldest' => '最旧创建',
        'updated' => '最近修改',
        'title' => '按标题',
        'favorite' => '收藏优先',
        _ => '最新创建',
      };

  Widget _kindFilter(LibraryStore store) {
    const kinds = <(String?, String)>[
      (null, '全部'),
      ('IMAGE', '生图'),
      ('VIDEO', '生视频'),
      ('AUDIO', '生音频'),
      ('MIXED', '混合'),
    ];
    // 注意：这里不能返回 Wrap —— 外层是横向滚动，宽度无约束
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final k in kinds) ...[
          ChoiceChip(
            label: Text(k.$2),
            selected: store.promptKind == k.$1,
            onSelected: (_) => store.setPromptKind(k.$1),
          ),
          const SizedBox(width: 6),
        ],
      ],
    );
  }

  Widget _buildBody(BuildContext context, LibraryStore store) {
    if (store.loadingPrompts && store.prompts.items.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (store.promptsError != null) {
      return ErrorState(message: store.promptsError!, onRetry: store.refreshPrompts);
    }
    if (store.prompts.items.isEmpty) {
      return EmptyState(
        icon: Icons.saved_search_outlined,
        title: store.hasPromptFilter ? '没有匹配的提示词' : '还没有提示词',
        subtitle: store.hasPromptFilter
            ? '换个关键词或标签试试'
            : '把 ComfyUI 里跑通的提示词存进来，之后按标签就能快速找到',
        action: store.hasPromptFilter
            ? TextButton(onPressed: () {
                _searchController.clear();
                store.setPromptQuery('');
                store.clearTags();
                store.setPromptKind(null);
                store.setOnlyFavorite(false);
                setState(() {});
              }, child: const Text('清除筛选'))
            : null,
      );
    }

    return Column(
      children: [
        Expanded(
          // 列数 = 可用宽度 / 550（宽窗口自动多列，窄窗口仍是单列）
          child: AdaptiveColumnList(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
            runSpacing: 10,
            itemCount: store.prompts.items.length,
            itemBuilder: (context, i) {
              final prompt = store.prompts.items[i];
              return PromptCard(
                prompt: prompt,
                selectedTags: store.selectedTags,
                selectionMode: _selectionMode,
                selected: _selected.contains(prompt.id),
                onLongPress: () => setState(() {
                  _selectionMode = true;
                  _selected.add(prompt.id);
                }),
                onOpen: () =>
                    _selectionMode ? _toggle(prompt.id) : _openDetail(context, prompt),
                onToggleTag: store.toggleTag,
              );
            },
          ),
        ),
        _Pager(
          page: store.prompts.page,
          pages: store.prompts.pages,
          onChanged: store.setPromptPage,
        ),
      ],
    );
  }

  Future<void> _openDetail(BuildContext context, Prompt prompt) async {
    await Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => PromptDetailPage(promptId: prompt.id)),
    );
    if (context.mounted) context.read<LibraryStore>().refreshAll();
  }

  // -------------------------------------------------------------------------
  //  批量管理
  // -------------------------------------------------------------------------

  Future<void> _bulkFavorite(bool favorite) async {
    final store = context.read<LibraryStore>();
    final ids = List.of(_selected);
    await _run(
      () => store.setPromptsFavorite(ids, favorite),
      favorite ? '已收藏 ${ids.length} 条' : '已取消收藏 ${ids.length} 条',
    );
  }

  Future<void> _bulkAddTags() async {
    final store = context.read<LibraryStore>();
    final ids = List.of(_selected);
    final picked = await showDialog<List<String>>(
      context: context,
      builder: (_) => _AddTagsDialog(count: ids.length),
    );
    if (picked == null || picked.isEmpty) return;
    await _run(
      () => store.addTagsToPrompts(ids, picked),
      '已给 ${ids.length} 条提示词加上：${picked.join('、')}',
    );
  }

  Future<void> _bulkDelete() async {
    final store = context.read<LibraryStore>();
    final ids = List.of(_selected);
    final ok = await confirmDialog(
      context,
      title: '删除 ${ids.length} 条提示词',
      message: '确定删除这 ${ids.length} 条提示词吗？\n已关联的产物会保留，但会解除关联。',
    );
    if (!ok) return;
    await _run(() => store.deletePrompts(ids), '已删除 ${ids.length} 条');
  }

  Future<void> _run(Future<void> Function() action, String toast) async {
    if (_busy) return;
    setState(() => _busy = true);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await action();
      messenger.showSnackBar(SnackBar(content: Text(toast)));
      if (mounted) _exitSelection();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('操作失败: $e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

/// 批量加标签：直接复用提示词编辑页那套标签控件（可搜索词表、也能新建）
class _AddTagsDialog extends StatefulWidget {
  final int count;

  const _AddTagsDialog({required this.count});

  @override
  State<_AddTagsDialog> createState() => _AddTagsDialogState();
}

class _AddTagsDialogState extends State<_AddTagsDialog> {
  List<String> _tags = const [];

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text('给 ${widget.count} 条提示词加标签'),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: TagEditor(
            value: _tags,
            label: '要添加的标签',
            onChanged: (v) => setState(() => _tags = v),
          ),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(
          onPressed: _tags.isEmpty ? null : () => Navigator.pop(context, _tags),
          child: const Text('添加标签'),
        ),
      ],
    );
  }
}

/// 多选模式下底部那条操作栏
class _SelectionBar extends StatelessWidget {
  final int count;
  final bool busy;
  final Future<void> Function() onFavorite;
  final Future<void> Function() onUnfavorite;
  final Future<void> Function() onAddTags;
  final Future<void> Function() onDelete;

  const _SelectionBar({
    required this.count,
    required this.busy,
    required this.onFavorite,
    required this.onUnfavorite,
    required this.onAddTags,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHigh,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: Row(
        children: [
          Text('$count 项', style: theme.textTheme.titleSmall),
          const Spacer(),
          TextButton.icon(
            onPressed: busy ? null : onFavorite,
            icon: const Icon(Icons.star_border),
            label: const Text('收藏'),
          ),
          TextButton.icon(
            onPressed: busy ? null : onUnfavorite,
            icon: const Icon(Icons.star_outline),
            label: const Text('取消收藏'),
          ),
          TextButton.icon(
            onPressed: busy ? null : onAddTags,
            icon: const Icon(Icons.sell_outlined),
            label: const Text('加标签'),
          ),
          TextButton.icon(
            onPressed: busy ? null : onDelete,
            icon: const Icon(Icons.delete_outline),
            label: const Text('删除'),
            style: TextButton.styleFrom(foregroundColor: theme.colorScheme.error),
          ),
        ],
      ),
    );
  }
}

class PromptCard extends StatelessWidget {
  final Prompt prompt;
  final Set<String> selectedTags;
  final VoidCallback onOpen;
  final void Function(String tag) onToggleTag;
  final VoidCallback? onLongPress;
  final bool selectionMode;
  final bool selected;

  const PromptCard({
    super.key,
    required this.prompt,
    required this.selectedTags,
    required this.onOpen,
    required this.onToggleTag,
    this.onLongPress,
    this.selectionMode = false,
    this.selected = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.read<LibraryStore>();

    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onOpen,
        onLongPress: onLongPress,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  if (selectionMode) ...[
                    Icon(
                      selected ? Icons.check_circle : Icons.radio_button_unchecked,
                      size: 20,
                      color: selected ? theme.colorScheme.primary : theme.colorScheme.outline,
                    ),
                    const SizedBox(width: 8),
                  ],
                  KindBadge(kind: prompt.kind),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      prompt.title.isEmpty ? '未命名提示词' : prompt.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
                    ),
                  ),
                  if (prompt.mediaCount > 0)
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Chip(
                        visualDensity: VisualDensity.compact,
                        avatar: const Icon(Icons.photo_library_outlined, size: 14),
                        label: Text('${prompt.mediaCount}'),
                      ),
                    ),
                  // 多选模式下藏掉单条操作，避免"想选中却点了收藏"
                  if (!selectionMode) ...[
                    IconButton(
                      tooltip: prompt.favorite ? '取消收藏' : '收藏',
                      visualDensity: VisualDensity.compact,
                      icon: Icon(
                        prompt.favorite ? Icons.star : Icons.star_border,
                        color: prompt.favorite ? Colors.amber : theme.colorScheme.outline,
                      ),
                      onPressed: () => store.togglePromptFavorite(prompt),
                    ),
                    PopupMenuButton<String>(
                      tooltip: '更多',
                      onSelected: (v) async {
                        final messenger = ScaffoldMessenger.of(context);
                        switch (v) {
                          case 'edit':
                            await Navigator.of(context).push(
                              MaterialPageRoute(builder: (_) => PromptEditPage(prompt: prompt)),
                            );
                            if (context.mounted) context.read<LibraryStore>().refreshAll();
                            break;
                          case 'duplicate':
                            await store.api.duplicatePrompt(prompt.id);
                            await store.refreshAll();
                            messenger.showSnackBar(const SnackBar(content: Text('已复制')));
                            break;
                          case 'delete':
                            final ok = await confirmDialog(
                              context,
                              title: '删除提示词',
                              message: '确定删除「${prompt.title}」吗？\n已关联的产物会保留，但会解除关联。',
                            );
                            if (ok) {
                              await store.deletePrompt(prompt.id);
                              messenger.showSnackBar(const SnackBar(content: Text('已删除')));
                            }
                            break;
                        }
                      },
                      itemBuilder: (_) => const [
                        PopupMenuItem(value: 'edit', child: ListTile(leading: Icon(Icons.edit), title: Text('编辑'))),
                        PopupMenuItem(value: 'duplicate', child: ListTile(leading: Icon(Icons.copy), title: Text('复制一份'))),
                        PopupMenuItem(value: 'delete', child: ListTile(leading: Icon(Icons.delete_outline), title: Text('删除'))),
                      ],
                    ),
                  ] else if (prompt.favorite)
                    const Icon(Icons.star, size: 18, color: Colors.amber),
                ],
              ),
              const SizedBox(height: 6),
              Text(
                prompt.positivePrompt,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodyMedium?.copyWith(height: 1.45),
              ),
              if (prompt.tags.isNotEmpty) ...[
                const SizedBox(height: 10),
                Wrap(
                  children: [
                    for (final tag in prompt.tags)
                      TagChip(
                        label: tag.name,
                        count: tag.useCount,
                        color: parseHexColor(tag.color),
                        selected: selectedTags.contains(tag.name),
                        dense: true,
                        onTap: () => onToggleTag(tag.name),
                      ),
                  ],
                ),
              ],
              const SizedBox(height: 6),
              Row(
                children: [
                  if (prompt.checkpoint != null)
                    Flexible(
                      child: Text(
                        prompt.checkpoint!,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                      ),
                    ),
                  const Spacer(),
                  Text(
                    relativeTime(prompt.updatedAt ?? prompt.createdAt),
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Pager extends StatelessWidget {
  final int page;
  final int pages;
  final ValueChanged<int> onChanged;

  const _Pager({required this.page, required this.pages, required this.onChanged});

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: Theme.of(context).dividerColor)),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          IconButton(
            icon: const Icon(Icons.chevron_left),
            onPressed: page > 1 ? () => onChanged(page - 1) : null,
          ),
          Text('第 $page / $pages 页'),
          IconButton(
            icon: const Icon(Icons.chevron_right),
            onPressed: page < pages ? () => onChanged(page + 1) : null,
          ),
        ],
      ),
    );
  }
}

Future<bool> confirmDialog(
  BuildContext context, {
  required String title,
  required String message,
  String confirmText = '删除',
}) async {
  final result = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: Text(message),
      actions: [
        TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: Theme.of(ctx).colorScheme.error),
          onPressed: () => Navigator.pop(ctx, true),
          child: Text(confirmText),
        ),
      ],
    ),
  );
  return result ?? false;
}
