import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/common.dart';
import '../widgets/media_thumb.dart';
import '../widgets/upload_sheet.dart';
import 'media_detail_page.dart';
import 'prompts_page.dart' show confirmDialog;

class GalleryPage extends StatefulWidget {
  const GalleryPage({super.key});

  @override
  State<GalleryPage> createState() => _GalleryPageState();
}

class _GalleryPageState extends State<GalleryPage> {
  final _searchController = TextEditingController();
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

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();
    final theme = Theme.of(context);

    return Scaffold(
      appBar: AppBar(
        leading: _selectionMode
            ? IconButton(
                icon: const Icon(Icons.close),
                onPressed: _exitSelection,
              )
            : null,
        title: Text(_selectionMode ? '已选 ${_selected.length} 项' : '画廊'),
        actions: _selectionMode
            ? [
                IconButton(
                  tooltip: '全选本页',
                  icon: const Icon(Icons.select_all),
                  onPressed: () => setState(() {
                    _selected
                      ..clear()
                      ..addAll(store.media.items.map((e) => e.id));
                  }),
                ),
                if (_busy)
                  const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 16),
                    child: Center(
                      child: SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2)),
                    ),
                  ),
              ]
            : [
                IconButton(
                  tooltip: '多选',
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
          child: Divider(height: 1, color: theme.dividerColor),
        ),
      ),
      floatingActionButton: _selectionMode && _selected.isNotEmpty
          ? null
          : FloatingActionButton.extended(
              onPressed: () async {
                final ok = await showUploadSheet(context);
                if (ok) await store.refreshAll();
              },
              icon: const Icon(Icons.add_photo_alternate_outlined),
              label: const Text('上传产物'),
            ),
      bottomNavigationBar: _selectionMode && _selected.isNotEmpty
          ? _SelectionBar(
              count: _selected.length,
              onLink: _bulkLink,
              onFavorite: _bulkFavorite,
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
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _searchController,
            decoration: InputDecoration(
              hintText: '搜索产物 / 关联提示词内容…',
              prefixIcon: const Icon(Icons.search),
              suffixIcon: _searchController.text.isEmpty
                  ? null
                  : IconButton(
                      icon: const Icon(Icons.clear),
                      onPressed: () {
                        _searchController.clear();
                        store.setMediaQuery('');
                        setState(() {});
                      },
                    ),
            ),
            onChanged: (_) => setState(() {}),
            onSubmitted: store.setMediaQuery,
          ),
          const SizedBox(height: 10),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              children: [
                for (final entry in const <(String?, String)>[
                  (null, '全部'),
                  ('IMAGE', '图片'),
                  ('VIDEO', '视频'),
                  ('AUDIO', '音频'),
                ]) ...[
                  ChoiceChip(
                    label: Text(entry.$2),
                    selected: store.mediaKind == entry.$1,
                    onSelected: (_) => store.setMediaKind(entry.$1),
                  ),
                  const SizedBox(width: 6),
                ],
                FilterChip(
                  label: const Text('仅收藏'),
                  avatar: Icon(
                    store.mediaOnlyFavorite ? Icons.star : Icons.star_border,
                    size: 16,
                    color: store.mediaOnlyFavorite ? Colors.amber : null,
                  ),
                  selected: store.mediaOnlyFavorite,
                  onSelected: store.setMediaOnlyFavorite,
                ),
                const SizedBox(width: 6),
                FilterChip(
                  label: const Text('未关联'),
                  selected: store.mediaUntagged,
                  onSelected: store.setMediaUntagged,
                ),
                const SizedBox(width: 6),
                if (store.mediaTags.isNotEmpty)
                  SegmentedButton<String>(
                    style: const ButtonStyle(visualDensity: VisualDensity.compact),
                    segments: const [
                      ButtonSegment(value: 'any', label: Text('任一标签')),
                      ButtonSegment(value: 'all', label: Text('全部标签')),
                    ],
                    selected: {store.mediaTagMode},
                    onSelectionChanged: (s) => store.setMediaTagMode(s.first),
                  ),
                const SizedBox(width: 6),
                PopupMenuButton<String>(
                  tooltip: '排序',
                  initialValue: store.mediaSort,
                  onSelected: store.setMediaSort,
                  itemBuilder: (_) => const [
                    PopupMenuItem(value: 'newest', child: Text('最新加入')),
                    PopupMenuItem(value: 'oldest', child: Text('最旧加入')),
                    PopupMenuItem(value: 'name', child: Text('按文件名')),
                    PopupMenuItem(value: 'largest', child: Text('按体积')),
                    PopupMenuItem(value: 'favorite', child: Text('收藏优先')),
                  ],
                  child: Chip(
                    avatar: const Icon(Icons.sort, size: 16),
                    label: Text(_sortLabel(store.mediaSort)),
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  '共 ${store.media.total} 个',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ],
            ),
          ),
          if (store.mediaTags.isNotEmpty) ...[
            const SizedBox(height: 8),
            Wrap(
              children: [
                for (final t in store.mediaTags)
                  TagChip(
                    label: t,
                    selected: true,
                    onDeleted: () => store.toggleMediaTag(t),
                    onTap: () => store.toggleMediaTag(t),
                  ),
                TextButton.icon(
                  onPressed: store.clearMediaTags,
                  icon: const Icon(Icons.clear_all, size: 16),
                  label: const Text('清空'),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  String _sortLabel(String s) => switch (s) {
        'oldest' => '最旧加入',
        'name' => '按文件名',
        'largest' => '按体积',
        'favorite' => '收藏优先',
        _ => '最新加入',
      };

  Widget _buildBody(BuildContext context, LibraryStore store) {
    if (store.loadingMedia && store.media.items.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (store.mediaError != null) {
      return ErrorState(message: store.mediaError!, onRetry: store.refreshMedia);
    }
    if (store.media.items.isEmpty) {
      return EmptyState(
        icon: Icons.photo_library_outlined,
        title: store.hasMediaFilter ? '没有匹配的产物' : '画廊还是空的',
        subtitle: store.hasMediaFilter
            ? '换个关键词或标签试试'
            : '把 ComfyUI 生成的图片 / 视频 / 音频拖进来，关联上提示词后就能一眼看到出处',
        action: FilledButton.icon(
          onPressed: () async {
            final ok = await showUploadSheet(context);
            if (ok) await store.refreshAll();
          },
          icon: const Icon(Icons.upload_file),
          label: const Text('上传产物'),
        ),
      );
    }

    return Column(
      children: [
        Expanded(
          child: GridView.builder(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 96),
            gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
              maxCrossAxisExtent: 230,
              crossAxisSpacing: 10,
              mainAxisSpacing: 10,
              childAspectRatio: 1,
            ),
            itemCount: store.media.items.length,
            itemBuilder: (context, i) {
              final m = store.media.items[i];
              return GestureDetector(
                onLongPress: () => setState(() {
                  _selectionMode = true;
                  _selected.add(m.id);
                }),
                // 右键：单个产物最常用的三个操作，不用先点进详情页
                onSecondaryTapDown: (details) => _showThumbMenu(m, details.globalPosition),
                child: MediaThumb(
                  media: m,
                  selectionMode: _selectionMode,
                  selected: _selected.contains(m.id),
                  onToggleSelect: () => setState(() {
                    if (!_selected.remove(m.id)) _selected.add(m.id);
                  }),
                  onTap: () async {
                    // 先把 store 取出来，避免 await 之后再碰 context
                    final store = context.read<LibraryStore>();
                    await Navigator.of(context).push(
                      MaterialPageRoute(builder: (_) => MediaDetailPage(mediaId: m.id)),
                    );
                    await store.refreshMedia();
                  },
                ),
              );
            },
          ),
        ),
        if (store.media.pages > 1)
          Container(
            padding: const EdgeInsets.symmetric(vertical: 6),
            decoration: BoxDecoration(
              border: Border(top: BorderSide(color: Theme.of(context).dividerColor)),
            ),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                IconButton(
                  icon: const Icon(Icons.chevron_left),
                  onPressed: store.mediaPage > 1 ? () => store.setMediaPage(store.mediaPage - 1) : null,
                ),
                Text('第 ${store.mediaPage} / ${store.media.pages} 页'),
                IconButton(
                  icon: const Icon(Icons.chevron_right),
                  onPressed: store.mediaPage < store.media.pages
                      ? () => store.setMediaPage(store.mediaPage + 1)
                      : null,
                ),
              ],
            ),
          ),
      ],
    );
  }

  // -------------------------------------------------------------------------
  //  右键菜单
  // -------------------------------------------------------------------------

  /// 画廊里对单个产物右键：关联提示词 / 收藏 / 删除。
  ///
  /// 这三个是"看图中途最想干的事"，走右键就不用先点进详情页再出来。
  Future<void> _showThumbMenu(MediaAsset media, Offset globalPosition) async {
    // 多选模式下右键留给选择操作，别弹菜单打断
    if (_selectionMode) return;

    final action = await showContextMenuAt<String>(
      context,
      globalPosition: globalPosition,
      items: [
        contextMenuItem(
          context,
          value: 'link',
          icon: Icons.link,
          label: media.hasPrompt ? '更换关联提示词…' : '关联提示词…',
        ),
        contextMenuItem(
          context,
          value: 'favorite',
          icon: media.favorite ? Icons.star_border : Icons.star,
          label: media.favorite ? '取消收藏' : '收藏',
        ),
        const PopupMenuDivider(),
        contextMenuItem(
          context,
          value: 'delete',
          icon: Icons.delete_outline,
          label: '删除',
          danger: true,
        ),
      ],
    );
    if (action == null || !mounted) return;

    final store = context.read<LibraryStore>();
    final messenger = ScaffoldMessenger.of(context);

    switch (action) {
      case 'link':
        final picked = await showPromptPicker(context);
        if (picked == null || !mounted) return;
        await store.api.updateMedia(media.id, promptId: picked.id);
        await store.refreshAll();
        messenger.showSnackBar(SnackBar(content: Text('已关联到「${picked.title}」')));
        break;
      case 'favorite':
        await store.api.updateMedia(media.id, favorite: !media.favorite);
        await store.refreshAll();
        break;
      case 'delete':
        final ok = await confirmDialog(
          context,
          title: '删除产物',
          message: '确定删除「${media.originalName}」吗？磁盘上的文件也会被删除。',
        );
        if (!ok || !mounted) return;
        await store.deleteMedia(media.id);
        messenger.showSnackBar(const SnackBar(content: Text('已删除')));
        break;
    }
  }

  // -------------------------------------------------------------------------
  //  批量操作
  // -------------------------------------------------------------------------

  Future<void> _bulkLink() async {
    final store = context.read<LibraryStore>();
    final picked = await showPromptPicker(context);
    if (picked == null) return;
    await _run(() async {
      for (final id in _selected) {
        await store.api.updateMedia(id, promptId: picked.id);
      }
    }, '已关联到「${picked.title}」');
  }

  Future<void> _bulkFavorite() async {
    final store = context.read<LibraryStore>();
    await _run(() async {
      for (final id in _selected) {
        await store.api.updateMedia(id, favorite: true);
      }
    }, '已收藏 ${_selected.length} 项');
  }

  Future<void> _bulkDelete() async {
    final store = context.read<LibraryStore>();
    final ok = await confirmDialog(
      context,
      title: '删除 ${_selected.length} 个产物',
      message: '磁盘上的文件也会被一并删除，且不可恢复。',
    );
    if (!ok) return;
    await _run(() async {
      for (final id in List.of(_selected)) {
        await store.api.deleteMedia(id);
      }
    }, '已删除 ${_selected.length} 项');
  }

  Future<void> _run(Future<void> Function() action, String toast) async {
    setState(() => _busy = true);
    final store = context.read<LibraryStore>();
    final messenger = ScaffoldMessenger.of(context);
    try {
      await action();
      await Future.wait([store.refreshMedia(), store.refreshPrompts(), store.refreshStats()]);
      messenger.showSnackBar(SnackBar(content: Text(toast)));
      if (mounted) _exitSelection();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('操作失败: $e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

class _SelectionBar extends StatelessWidget {
  final int count;
  final Future<void> Function() onLink;
  final Future<void> Function() onFavorite;
  final Future<void> Function() onDelete;

  const _SelectionBar({
    required this.count,
    required this.onLink,
    required this.onFavorite,
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
          TextButton.icon(onPressed: onLink, icon: const Icon(Icons.link), label: const Text('关联提示词')),
          TextButton.icon(onPressed: onFavorite, icon: const Icon(Icons.star_border), label: const Text('收藏')),
          TextButton.icon(
            onPressed: onDelete,
            icon: const Icon(Icons.delete_outline),
            label: const Text('删除'),
            style: TextButton.styleFrom(foregroundColor: theme.colorScheme.error),
          ),
        ],
      ),
    );
  }
}
