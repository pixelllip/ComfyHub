import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import 'prompts_page.dart' show confirmDialog;
import 'tag_results_page.dart';

class TagsPage extends StatefulWidget {
  const TagsPage({super.key});

  @override
  State<TagsPage> createState() => _TagsPageState();
}

class _TagsPageState extends State<TagsPage> {
  final _searchController = TextEditingController();
  String _query = '';
  String _sort = 'popular';
  String? _category;

  @override
  void dispose() {
    _searchController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();
    final theme = Theme.of(context);

    var tags = store.tags.where((t) {
      if (_category != null && t.category != _category) return false;
      if (_query.isEmpty) return true;
      final q = _query.toLowerCase();
      return t.name.toLowerCase().contains(q) ||
          (t.description ?? '').toLowerCase().contains(q) ||
          (t.category ?? '').toLowerCase().contains(q);
    }).toList();

    tags.sort((a, b) => switch (_sort) {
          'name' => a.name.compareTo(b.name),
          'newest' => b.id.compareTo(a.id),
          _ => b.useCount != a.useCount ? b.useCount - a.useCount : a.name.compareTo(b.name),
        });

    return Scaffold(
      appBar: AppBar(
        title: const Text('标签'),
        actions: [
          IconButton(
            tooltip: '刷新',
            icon: const Icon(Icons.refresh),
            onPressed: store.refreshTags,
          ),
          const SizedBox(width: 4),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(1),
          child: Divider(height: 1, color: theme.dividerColor),
        ),
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _editTag(context, null),
        icon: const Icon(Icons.add),
        label: const Text('新建标签'),
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 6),
            child: Column(
              children: [
                TextField(
                  controller: _searchController,
                  decoration: const InputDecoration(
                    hintText: '搜索标签…',
                    prefixIcon: Icon(Icons.search),
                  ),
                  onChanged: (v) => setState(() => _query = v.trim()),
                ),
                const SizedBox(height: 10),
                SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: Row(
                    children: [
                      ChoiceChip(
                        label: const Text('全部分类'),
                        selected: _category == null,
                        onSelected: (_) => setState(() => _category = null),
                      ),
                      const SizedBox(width: 6),
                      for (final c in store.tagCategories) ...[
                        ChoiceChip(
                          label: Text(c),
                          selected: _category == c,
                          onSelected: (_) => setState(() => _category = c),
                        ),
                        const SizedBox(width: 6),
                      ],
                      PopupMenuButton<String>(
                        initialValue: _sort,
                        onSelected: (v) => setState(() => _sort = v),
                        itemBuilder: (_) => const [
                          PopupMenuItem(value: 'popular', child: Text('按使用频率')),
                          PopupMenuItem(value: 'name', child: Text('按名称')),
                          PopupMenuItem(value: 'newest', child: Text('按创建时间')),
                        ],
                        child: Chip(
                          avatar: const Icon(Icons.sort, size: 16),
                          label: Text(switch (_sort) {
                            'name' => '按名称',
                            'newest' => '按创建时间',
                            _ => '按使用频率',
                          }),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          Expanded(
            child: store.loadingTags && store.tags.isEmpty
                ? const Center(child: CircularProgressIndicator())
                : tags.isEmpty
                    ? EmptyState(
                        icon: Icons.sell_outlined,
                        title: _query.isEmpty ? '还没有标签' : '没有匹配的标签',
                        subtitle: '标签是提示词的检索维度，建议按「风格 / 角色 / 画质」分类管理',
                        action: FilledButton.icon(
                          onPressed: () => _editTag(context, null),
                          icon: const Icon(Icons.add),
                          label: const Text('新建标签'),
                        ),
                      )
                    : LayoutBuilder(
                        builder: (context, constraints) {
                          // 列数 = 可用宽度 / 550；标签卡片高度固定，直接用 GridView 更省事
                          final columns = adaptiveColumnCount(constraints.maxWidth);
                          return GridView.builder(
                            padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
                            gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                              crossAxisCount: columns,
                              crossAxisSpacing: 8,
                              mainAxisSpacing: 8,
                              mainAxisExtent: 78,
                            ),
                            itemCount: tags.length,
                            itemBuilder: (context, i) {
                              final tag = tags[i];
                              final color = parseHexColor(tag.color) ?? tagColor(tag.name);
                              return Card(
                                child: ListTile(
                                  leading: Container(
                                    width: 12,
                                    height: 12,
                                    decoration:
                                        BoxDecoration(color: color, shape: BoxShape.circle),
                                  ),
                                  title: Text(tag.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                                  subtitle: Text(
                                    [
                                      if ((tag.category ?? '').isNotEmpty) tag.category!,
                                      if ((tag.description ?? '').isNotEmpty) tag.description!,
                                    ].join(' · '),
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                  trailing: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      Chip(
                                        visualDensity: VisualDensity.compact,
                                        label: Text('${tag.useCount}'),
                                      ),
                                      IconButton(
                                        tooltip: '编辑',
                                        icon: const Icon(Icons.edit_outlined, size: 18),
                                        onPressed: () => _editTag(context, tag),
                                      ),
                                      IconButton(
                                        tooltip: '删除',
                                        icon: const Icon(Icons.delete_outline, size: 18),
                                        onPressed: () async {
                                          final ok = await confirmDialog(
                                            context,
                                            title: '删除标签',
                                            message:
                                                '删除「${tag.name}」会同时解除它在所有提示词和产物上的关联。',
                                          );
                                          if (!ok) return;
                                          await store.api.deleteTag(tag.id);
                                          await store.refreshAll();
                                        },
                                      ),
                                    ],
                                  ),
                                  onTap: () async {
                                    await Navigator.of(context).push(
                                      MaterialPageRoute(
                                        builder: (_) => TagResultsPage(tag: tag.name),
                                      ),
                                    );
                                    if (context.mounted) {
                                      context.read<LibraryStore>().refreshAll();
                                    }
                                  },
                                ),
                              );
                            },
                          );
                        },
                      ),
          ),
        ],
      ),
    );
  }

  Future<void> _editTag(BuildContext context, Tag? tag) async {
    final store = context.read<LibraryStore>();
    final nameController = TextEditingController(text: tag?.name ?? '');
    final categoryController = TextEditingController(text: tag?.category ?? '');
    final descController = TextEditingController(text: tag?.description ?? '');
    String colorHex = tag?.color ?? '#7C5CFF';

    final saved = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setState) => AlertDialog(
          title: Text(tag == null ? '新建标签' : '编辑标签'),
          content: SizedBox(
            width: 420,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: nameController,
                  autofocus: true,
                  decoration: const InputDecoration(labelText: '标签名'),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: categoryController,
                  decoration: const InputDecoration(
                    labelText: '分类',
                    hintText: '风格 / 角色 / 画质 / 负面 / 其它',
                  ),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: descController,
                  decoration: const InputDecoration(labelText: '说明（可选）'),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    const Text('颜色'),
                    const SizedBox(width: 12),
                    for (final c in const [
                      '#7C5CFF', '#4CAF50', '#2196F3', '#E91E63',
                      '#FF9800', '#00BCD4', '#F44336', '#795548',
                    ])
                      Padding(
                        padding: const EdgeInsets.only(right: 6),
                        child: GestureDetector(
                          onTap: () => setState(() => colorHex = c),
                          child: Container(
                            width: 22,
                            height: 22,
                            decoration: BoxDecoration(
                              color: parseHexColor(c),
                              shape: BoxShape.circle,
                              border: Border.all(
                                color: colorHex == c ? Colors.white : Colors.transparent,
                                width: 2,
                              ),
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
              ],
            ),
          ),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: const Text('保存'),
            ),
          ],
        ),
      ),
    );

    if (saved == true && nameController.text.trim().isNotEmpty) {
      try {
        if (tag == null) {
          await store.api.createTag(
            nameController.text.trim(),
            category: categoryController.text.trim().isEmpty ? null : categoryController.text.trim(),
            color: colorHex,
            description: descController.text.trim().isEmpty ? null : descController.text.trim(),
          );
        } else {
          await store.api.updateTag(
            tag.id,
            nameController.text.trim(),
            category: categoryController.text.trim().isEmpty ? null : categoryController.text.trim(),
            color: colorHex,
            description: descController.text.trim().isEmpty ? null : descController.text.trim(),
          );
        }
        await store.refreshAll();
      } catch (e) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('保存失败: $e')));
        }
      }
    }

    nameController.dispose();
    categoryController.dispose();
    descController.dispose();
  }
}
