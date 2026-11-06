import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import '../widgets/scroll_linked_title.dart';
import 'prompts_page.dart' show confirmDialog;
import 'tag_results_page.dart';

/// 标签卡的最小宽度：**360 宽的窗口下的一列**（360 减去左右各 16 的列表内边距）。
///
/// 用户 2026-11-05 定的底线：标签卡再窄就放不下 16 个英文字符的标签名
/// （左边有颜色点，右边有计数与两个操作按钮）。宽窗口按这个宽度分列，
/// 所以任何一列都不会比最小窗口下的那张卡更窄。
const double kTagCardMinWidth = 328;

class TagsPage extends StatefulWidget {
  const TagsPage({super.key});

  @override
  State<TagsPage> createState() => _TagsPageState();
}

class _TagsPageState extends State<TagsPage> {
  final _searchController = TextEditingController();

  /// 大标题 → AppBar 标题的联动进度（见 [ScrollTitleProgress]）。
  final _titleProgress = ScrollTitleProgress();
  String _query = '';
  String _sort = 'popular';
  String? _category;

  @override
  void dispose() {
    _searchController.dispose();
    _titleProgress.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();
    // 电脑端标题始终在 AppBar；手机端保留滚动联动（用户 2026-11-05）。
    final phone = useCollapsingPageTitle;

    var tags = store.tags.where((t) {
      if (_category != null && t.category != _category) return false;
      if (_query.isEmpty) return true;
      final q = _query.toLowerCase();
      return t.name.toLowerCase().contains(q) ||
          (t.description ?? '').toLowerCase().contains(q) ||
          (t.category ?? '').toLowerCase().contains(q);
    }).toList();

    tags.sort(
      (a, b) => switch (_sort) {
        'name' => a.name.compareTo(b.name),
        'newest' => b.id.compareTo(a.id),
        _ =>
          b.useCount != a.useCount
              ? b.useCount - a.useCount
              : a.name.compareTo(b.name),
      },
    );

    return Scaffold(
      appBar: AppBar(
        // 电脑端标题始终在 AppBar；手机端大标题滚出视野后才淡入。
        title: phone
            ? FadingAppBarTitle(title: '标签', progress: _titleProgress)
            : const Text('标签'),
        actions: [
          IconButton(
            tooltip: '刷新',
            icon: const Icon(Icons.refresh),
            onPressed: store.refreshTags,
          ),
          const SizedBox(width: 4),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _editTag(context, null),
        icon: const Icon(Icons.add),
        label: const Text('新建标签'),
      ),
      body: Column(
        children: [
          if (phone) CollapsingPageTitle(title: '标签', progress: _titleProgress),
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
                // 分类筛选行**折行显示**（用户要求：一行放不下就两行、三行，
                // 不要横向滚动）。以前是 `SingleChildScrollView(horizontal)`：
                // 分类一多 / 窗口一窄，右侧的分类和排序就被切在屏幕外。
                FilterChipRow(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    ChoiceChip(
                      label: const Text('全部分类'),
                      selected: _category == null,
                      onSelected: (_) => setState(() => _category = null),
                    ),
                    for (final c in store.tagCategories)
                      ChoiceChip(
                        label: Text(c),
                        selected: _category == c,
                        onSelected: (_) => setState(() => _category = c),
                      ),
                    AppMenuButton<String>(
                      tooltip: '排序',
                      onSelected: (v) => setState(() => _sort = v),
                      options: const [
                        MenuOption(
                          value: 'popular',
                          icon: Icons.trending_up,
                          label: '按使用频率',
                        ),
                        MenuOption(
                          value: 'name',
                          icon: Icons.sort_by_alpha,
                          label: '按名称',
                        ),
                        MenuOption(
                          value: 'newest',
                          icon: Icons.schedule,
                          label: '按创建时间',
                        ),
                      ],
                      button: (context, controller, isOpen) => InkWell(
                        borderRadius: BorderRadius.circular(8),
                        onTap: () => controller.isOpen
                            ? controller.close()
                            : controller.open(),
                        child: Chip(
                          avatar: const Icon(Icons.sort, size: 16),
                          label: Text(switch (_sort) {
                            'name' => '按名称',
                            'newest' => '按创建时间',
                            _ => '按使用频率',
                          }),
                        ),
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
          Expanded(
            child: ScrollTitleListener(
              progress: _titleProgress,
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
                        // 列数 = 可用宽度 / [kTagCardMinWidth]（328 = 360 窗口的一列）。
                        //
                        // ⚠ 这里刻意**不用**通用的 550（用户 2026-10-12：
                        // "标签页的卡片宽度过大，缩小为原来一半"）：标签卡里只有
                        // 一个名字 + 一行分类/说明 + 一个计数 + 两个按钮，
                        // 按 550 分列时在 1440 窗口上每张 ≈656 宽，名字只占左边
                        // 一小截、右边全是空的。现在以"360 窗口下能完整显示
                        // 16 个英文字符"为下限，宽窗口按这个下限分列。
                        //
                        // 卡片高度固定，所以直接用 GridView（`mainAxisExtent`），
                        // 不用 `AdaptiveColumnList` —— 那个是给"每行等高"的卡片用的。
                        final columns = adaptiveColumnCount(
                          constraints.maxWidth,
                          // **下限就是 360 宽窗口下的一列**（用户 2026-11-05）：
                          // 再窄卡片就放不下 16 个英文字符的标签名了。
                          minColumnWidth: kTagCardMinWidth,
                        );
                        return GridView.builder(
                          padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
                          // 卡片外层 RepaintBoundary 由 delegate 自动加；
                          // keep-alive 对标签卡没用，关掉省一层包装
                          addAutomaticKeepAlives: false,
                          gridDelegate:
                              SliverGridDelegateWithFixedCrossAxisCount(
                                crossAxisCount: columns,
                                crossAxisSpacing: 8,
                                mainAxisSpacing: 8,
                                mainAxisExtent: 78,
                              ),
                          itemCount: tags.length,
                          itemBuilder: (context, i) {
                            final tag = tags[i];
                            final color =
                                parseHexColor(tag.color) ?? tagColor(tag.name);
                            return Card(
                              child: ListTile(
                                // 左右内边距收一点：标题要多留出一个 16 字符英文名
                                // 的位置（两枚按钮 + 计数在右边很占地方）。
                                contentPadding: const EdgeInsets.only(
                                  left: 10,
                                  right: 2,
                                ),
                                horizontalTitleGap: 8,
                                leading: Container(
                                  width: 12,
                                  height: 12,
                                  decoration: BoxDecoration(
                                    color: color,
                                    shape: BoxShape.circle,
                                  ),
                                ),
                                title: Text(
                                  tag.name,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                subtitle: Text(
                                  [
                                    if ((tag.category ?? '').isNotEmpty)
                                      tag.category!,
                                    if ((tag.description ?? '').isNotEmpty)
                                      tag.description!,
                                  ].join(' · '),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                trailing: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    // 计数用紧凑胶囊而不是 Chip：Chip 的左右内边距
                                    // 会把标题宽度吃掉一截（16 字符标签名就是这么被挤掉的）。
                                    Container(
                                      padding: const EdgeInsets.symmetric(
                                        horizontal: 6,
                                        vertical: 2,
                                      ),
                                      decoration: BoxDecoration(
                                        color: Theme.of(context)
                                            .colorScheme
                                            .surfaceContainerHigh,
                                        borderRadius: BorderRadius.circular(8),
                                      ),
                                      child: Text(
                                        '${tag.useCount}',
                                        style: Theme.of(context)
                                            .textTheme
                                            .labelSmall,
                                      ),
                                    ),
                                    // 按钮**不许为了省宽度而缩小命中区**（用户
                                    // 2026-11-05："标签卡片按钮太小，很难按到"）。
                                    // 44 是视觉尺寸，默认的 padded 会把命中区补到 48；
                                    // 省下来的宽度靠"计数不用 Chip + 卡片留白收紧"。
                                    IconButton(
                                      tooltip: '编辑',
                                      icon: const Icon(
                                        Icons.edit_outlined,
                                        size: 20,
                                      ),
                                      padding: EdgeInsets.zero,
                                      constraints: const BoxConstraints(
                                        minWidth: 44,
                                        minHeight: 44,
                                      ),
                                      onPressed: () => _editTag(context, tag),
                                    ),
                                    IconButton(
                                      tooltip: '删除',
                                      icon: const Icon(
                                        Icons.delete_outline,
                                        size: 20,
                                      ),
                                      padding: EdgeInsets.zero,
                                      constraints: const BoxConstraints(
                                        minWidth: 44,
                                        minHeight: 44,
                                      ),
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
                                      builder: (_) =>
                                          TagResultsPage(tag: tag.name),
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
                      '#7C5CFF',
                      '#4CAF50',
                      '#2196F3',
                      '#E91E63',
                      '#FF9800',
                      '#00BCD4',
                      '#F44336',
                      '#795548',
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
                                color: colorHex == c
                                    ? Colors.white
                                    : Colors.transparent,
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
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('取消'),
            ),
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
            category: categoryController.text.trim().isEmpty
                ? null
                : categoryController.text.trim(),
            color: colorHex,
            description: descController.text.trim().isEmpty
                ? null
                : descController.text.trim(),
          );
        } else {
          await store.api.updateTag(
            tag.id,
            nameController.text.trim(),
            category: categoryController.text.trim().isEmpty
                ? null
                : categoryController.text.trim(),
            color: colorHex,
            description: descController.text.trim().isEmpty
                ? null
                : descController.text.trim(),
          );
        }
        await store.refreshAll();
      } catch (e) {
        if (context.mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text('保存失败: $e')));
        }
      }
    }

    nameController.dispose();
    categoryController.dispose();
    descController.dispose();
  }
}
