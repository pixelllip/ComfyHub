import 'dart:async';
import 'dart:io' show Platform;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../app.dart' show kWideLayoutMinWidth;
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/collection_thumb.dart';
import '../widgets/common.dart';
import '../widgets/export_dialog.dart';
import '../widgets/media_thumb.dart';
import '../widgets/paged_swipe.dart';
import '../widgets/upload_sheet.dart';
import 'media_detail_page.dart';
import 'prompts_page.dart' show confirmDialog;

class GalleryPage extends StatefulWidget {
  const GalleryPage({super.key});

  @override
  State<GalleryPage> createState() => _GalleryPageState();
}

/// 集合面板的尺寸区间。
///
/// ⚠ 它们**不是布局断点** —— 断点仍然只有 `kWideLayoutMinWidth` 一个。
/// 这两个数说的是"这块面板自己长什么样"，不是"窗口够不够宽"。
///
/// * 面板最窄 / 最宽：再窄里面的成员格子没法看，再宽就喧宾夺主。
///
/// ⚠ **"停靠还是弹窗"不由这几个数决定**（用户 2026-10-18："能不能让我自由选择…"）：
/// 形态**只看设置**（`SettingsStore.collectionDetailWide` / `collectionDetailNarrow`，
/// 见 [_libraryBody]），这里的宽度算式只管"停靠时这块面板多宽"。
/// 以前还有一条"扣掉面板后网格剩不下 `kCollectionGridMinWidth` 就退回弹窗"的
/// 自动回退 —— **已删**：那正是替用户做决定，他要的是"我自己定"。
const double kCollectionPanelMinWidth = 300;
const double kCollectionPanelMaxWidth = 480;

/// 测试用：强制"鼠标优先"这个判据（`Platform` 在用例里改不动）。
///
/// 默认 `null` = 按平台判。**两条手势路径都得能测**：即时拖（桌面）与长按拖（触屏），
/// 后者尤其不能坏 —— 它决定了触屏上会不会跟滚动抢手势。
@visibleForTesting
bool? debugMouseFirstOverride;

class _GalleryPageState extends State<GalleryPage> {
  final _searchController = TextEditingController();

  /// 多选里的一项：`key` 是**代表那条**的 media id（集合取封面那条）。
  ///
  /// * `size` = 这一项覆盖多少张产物（单条 = 1，集合 = 整组成员数）；
  /// * `collection` = 非空表示"这一项代表整个集合"；
  /// * `kind` = 这一项的产物类型（`IMAGE` / `VIDEO` / …）——「移到集合…」要靠它
  ///   把"装不下这一批"的目标置灰并说明原因（一个集合只装一种，用户 2026-10-18 拍板）。
  ///
  /// ⚠ 用户 2026-10-18 拍板："集合格子拖动时，动整组。" 所以选中态**不能只存代表 id**
  /// —— 以前 `Set<int>` 里一个集合格子和一张单图长得一模一样，批量删就只删掉了封面
  /// （这正是"一次只能删一张"的另一面）。
  final Map<int, ({int size, int? collection, String kind})> _picked = {};

  bool _selectionMode = false;
  bool _busy = false;

  /// 正"点开"的那个集合（用户 2026-10-18："点进文件夹，在弹窗展开文件夹列出所有图片"）。
  ///
  /// ⚠ 它**不是 `showDialog`**：模态弹窗会铺一层 `ModalBarrier`，把底下的画廊整个
  /// 挡在命中测试之外 —— 那样"把成员拖出文件夹、拖到画廊里的另一个格子上"
  /// 根本收不到落点（这恰恰是用户要的那条拖动逻辑）。
  /// 所以这里做成**同一个页面里的浮层**：画廊还在，落点也还在。
  MediaAsset? _openFolder;
  List<MediaAsset>? _folderMembers;

  /// 集合弹窗里的**多选**（用户 2026-10-18："在集合内我不能多选操作"）。
  bool _folderSelection = false;
  final Set<int> _folderPicked = {};

  /// 拖动中：弹窗与背景都让开，好让指针落到画廊的格子上。
  bool _folderDragging = false;

  /// 弹窗的位置（拖动**开始**时记一次，见 `_captureFolderRect`）。
  GlobalKey _folderKey = GlobalKey();
  Rect? _folderRect;

  /// 这台机器是不是"鼠标优先"。
  ///
  /// 与 [`kWideLayoutMinWidth`] **不是一回事**：那是布局断点，这是输入设备。
  /// 桌面用**滚轮**翻页，所以即时左键拖动不会跟滚动打架（这正是长按拖在桌面上
  /// 显得"不好用"的原因）；触屏上必须长按才起拖，否则轻轻一划就变成拖图。
  static bool get _mouseFirst =>
      debugMouseFirstOverride ??
      (Platform.isWindows || Platform.isLinux || Platform.isMacOS);

  /// 导出写的是**后端所在机器**的磁盘，而且后端只允许回环调用 ——
  /// 手机上那颗按钮只会撞 403，所以干脆不出现。
  static bool get _canExport => Platform.isWindows;

  /// 选中项覆盖的**产物总数**（不是格子数）—— 底部栏与标题都按它显示。
  int get _pickedCount =>
      _picked.values.fold(0, (sum, pick) => sum + pick.size);

  /// 把当前一页的格子收进"选中"（全选本页用）。
  ///
  /// ⚠ 「仅收藏」档里集合被**无视**（每个格子都是一张单图，见
  /// `LibraryStore.mediaFavoritesFlat`）：这里也必须按一张算 ——
  /// 否则界面上勾的是一张、批量删/收藏却动了整组。
  void _pickOf(MediaAsset media) {
    final favoritesFlat = context.read<LibraryStore>().mediaFavoritesFlat;
    final grouped = media.isCollection && !favoritesFlat;
    _picked[media.id] = (
      size: grouped ? media.collectionSize : 1,
      collection: grouped ? media.collectionId : null,
      kind: media.kind.name.toUpperCase(),
    );
  }

  @override
  void dispose() {
    _searchController.dispose();
    super.dispose();
  }

  void _exitSelection() {
    setState(() {
      _selectionMode = false;
      _picked.clear();
    });
  }

  /// 点一下格子：勾选 / 取消。**集合格子勾的是整组**（记成员数）。
  void _togglePick(MediaAsset media) {
    setState(() {
      if (_picked.remove(media.id) != null) return;
      _pickOf(media);
    });
  }

  /// 打开详情页，回来后刷新（删除 / 收藏 / 打标签都会改变列表）。
  Future<void> _openDetail(int mediaId) async {
    final store = context.read<LibraryStore>();
    await Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => MediaDetailPage(mediaId: mediaId)),
    );
    await store.refreshMedia();
  }

  /// 导出**全部**产物（不受画廊当前筛选影响 —— 弹窗里会把范围写清楚）。
  Future<void> _exportAll() async {
    await showExportDialog(context);
  }

  /// 导出多选中的产物。集合格子在 `_expandPicks` 里已经摊平成成员 id，
  /// 所以后端仍然按成员所属的集合分文件夹。
  Future<void> _exportSelected() async {
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    await showExportDialog(context, mediaIds: ids);
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<LibraryStore>();

    // ContextMenuScope：右键菜单的宿主。它**不铺 ModalBarrier**，所以菜单开着时
    // 画廊照样能滚（用户 bug ②）。一个页面只挂一个。
    // 外面套 Builder：让下面的 context 落在 Scope **之内**，右键菜单才找得到宿主。
    return ContextMenuScope(
      child: Builder(
        builder: (context) => Scaffold(
          appBar: AppBar(
            leading: _selectionMode
                ? IconButton(
                    icon: const Icon(Icons.close),
                    onPressed: _exitSelection,
                  )
                : null,
            title: Text(_selectionMode ? '已选 $_pickedCount 张' : '画廊'),
            actions: _selectionMode
                ? [
                    IconButton(
                      tooltip: '全选本页',
                      icon: const Icon(Icons.select_all),
                      onPressed: () => setState(() {
                        // 集合格子整组计入：全选一页 10 格，可能是 30 张
                        _picked.clear();
                        for (final media in store.media.items) {
                          _pickOf(media);
                        }
                      }),
                    ),
                    if (_busy)
                      const Padding(
                        padding: EdgeInsets.symmetric(horizontal: 16),
                        child: Center(
                          child: SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          ),
                        ),
                      )
                    else if (_picked.isNotEmpty && _canExport)
                      IconButton(
                        tooltip: '导出选中的产物',
                        icon: const Icon(Icons.save_alt),
                        onPressed: _exportSelected,
                      ),
                  ]
                : [
                    IconButton(
                      tooltip: '多选',
                      icon: const Icon(Icons.checklist),
                      onPressed: () => setState(() => _selectionMode = true),
                    ),
                    IconButton(
                      tooltip: '重新自动整理',
                      icon: const Icon(Icons.auto_awesome_motion),
                      onPressed: _busy ? null : _regroup,
                    ),
                    if (_canExport)
                      IconButton(
                        tooltip: '导出产物',
                        icon: const Icon(Icons.save_alt),
                        onPressed: _exportAll,
                      ),
                    IconButton(
                      tooltip: '刷新',
                      icon: const Icon(Icons.refresh),
                      onPressed: () => store.refreshAll(),
                    ),
                    const SizedBox(width: 4),
                  ],
          ),
          floatingActionButton: _selectionMode && _picked.isNotEmpty
              ? null
              : liftFabAbovePageBar(
                  FloatingActionButton.extended(
                    onPressed: () async {
                      final ok = await showUploadSheet(context);
                      if (ok) await store.refreshAll();
                    },
                    icon: const Icon(Icons.add_photo_alternate_outlined),
                    label: const Text('上传产物'),
                  ),
                  // 只有真有翻页条时才需要抬高；单页画廊别白留一条空白
                  lift: store.media.pages > 1,
                ),
          bottomNavigationBar: _selectionMode && _picked.isNotEmpty
              ? _SelectionBar(
                  count: _pickedCount,
                  onLink: _bulkLink,
                  onFavorite: _bulkFavorite,
                  onMove: _bulkMove,
                  onDetach: _bulkDetach,
                  onDelete: _bulkDelete,
                )
              : null,
          body: Column(
            children: [
              _buildToolbar(context, store),
              Expanded(child: _buildBody(context, store)),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildToolbar(BuildContext context, LibraryStore store) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final collapsed = settings.galleryFiltersCollapsed;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 搜索栏 + **折叠按钮**（用户 2026-10-11 定：按钮放搜索栏右侧）
          Row(
            children: [
              Expanded(
                child: TextField(
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
              ),
              const SizedBox(width: 4),
              IconButton(
                tooltip: collapsed ? '展开筛选' : '收起筛选',
                icon: Icon(collapsed ? Icons.expand_more : Icons.expand_less),
                onPressed: () =>
                    settings.setGalleryFiltersCollapsed(!collapsed),
              ),
            ],
          ),
          // 收起时**露出当前生效的筛选摘要** —— 否则用户看不出自己在筛选状态，
          // 会以为"图怎么少了"（用户 2026-10-11 明确要求）。
          if (collapsed)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: _FilterSummary(text: _filterSummary(store), theme: theme),
            ),
          if (!collapsed) ...[
            const SizedBox(height: 10),
            _filterSections(context, store, theme),
          ],
        ],
      ),
    );
  }

  /// 当前生效的筛选项摘要（形如 `生图 · 仅收藏 · 最新加入`）。
  String _filterSummary(LibraryStore store) {
    final parts = <String>[
      switch (store.mediaKind) {
        'IMAGE' => '图片',
        'VIDEO' => '视频',
        'AUDIO' => '音频',
        _ => '全部',
      },
      if (store.mediaOnlyFavorite) '仅收藏',
      if (store.mediaUntagged) '未关联',
      if (store.mediaTags.isNotEmpty) '标签 ${store.mediaTags.join('/')}',
      if (store.mediaQuery.isNotEmpty) '搜索「${store.mediaQuery}」',
      _sortLabel(store.mediaSort),
    ];
    return parts.join(' · ');
  }

  /// 筛选选项**按功能分区**，但**不"一种一行"**（2026-10-11 用户改的口径）：
  /// 组内联排、组与组之间一条竖线，宽窗口一行塞得下两三组，窄窗口整组换行。
  ///
  /// 为什么分区：这些 chip 原来是一条扁平的流，"一键清除"那种**破坏性**动作
  /// 与"只看收藏"混在一起，扫一眼分不出哪个会改数据。
  Widget _filterSections(
    BuildContext context,
    LibraryStore store,
    ThemeData theme,
  ) {
    return FilterGroupRow(
      groups: [
        FilterGroup(
          title: '类型',
          children: [
            for (final entry in const <(String?, String)>[
              (null, '全部'),
              ('IMAGE', '图片'),
              ('VIDEO', '视频'),
              ('AUDIO', '音频'),
            ])
              ChoiceChip(
                label: Text(entry.$2),
                selected: store.mediaKind == entry.$1,
                onSelected: (_) => store.setMediaKind(entry.$1),
              ),
          ],
        ),
        FilterGroup(
          title: '状态',
          children: [
            FilterChip(
              label: const Text('仅收藏'),
              avatar: Icon(
                store.mediaOnlyFavorite ? Icons.star : Icons.star_border,
                size: 16,
                color: store.mediaOnlyFavorite ? Colors.amber : null,
              ),
              selected: store.mediaOnlyFavorite,
              // 这一档**无视集合**（见 `LibraryStore.mediaFavoritesFlat`）：
              // 还开着的集合面板已经不符合这个语义，顺手收起，别让用户对着两个矛盾的视图。
              onSelected: (v) {
                _closeFolder();
                store.setMediaOnlyFavorite(v);
              },
            ),
            FilterChip(
              label: const Text('未关联'),
              selected: store.mediaUntagged,
              onSelected: store.setMediaUntagged,
            ),
          ],
        ),
        if (store.mediaTags.isNotEmpty)
          FilterGroup(
            title: '标签',
            children: [
              for (final t in store.mediaTags)
                TagChip(
                  label: t,
                  selected: true,
                  onDeleted: () => store.toggleMediaTag(t),
                  onTap: () => store.toggleMediaTag(t),
                ),
              SegmentedButton<String>(
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
                segments: const [
                  ButtonSegment(value: 'any', label: Text('任一')),
                  ButtonSegment(value: 'all', label: Text('全部')),
                ],
                selected: {store.mediaTagMode},
                onSelectionChanged: (s) => store.setMediaTagMode(s.first),
              ),
              TextButton.icon(
                onPressed: store.clearMediaTags,
                icon: const Icon(Icons.clear_all, size: 16),
                label: const Text('清空'),
              ),
            ],
          ),
        FilterGroup(
          title: '排序',
          children: [
            AppMenuButton<String>(
              tooltip: '排序',
              onSelected: store.setMediaSort,
              options: const [
                MenuOption(
                  value: 'newest',
                  icon: Icons.schedule,
                  label: '最新加入',
                ),
                MenuOption(value: 'oldest', icon: Icons.history, label: '最旧加入'),
                MenuOption(
                  value: 'name',
                  icon: Icons.sort_by_alpha,
                  label: '按文件名',
                ),
                MenuOption(
                  value: 'largest',
                  icon: Icons.data_usage,
                  label: '按体积',
                ),
                MenuOption(
                  value: 'favorite',
                  icon: Icons.star_border,
                  label: '收藏优先',
                ),
              ],
              button: (context, controller, isOpen) => InkWell(
                borderRadius: BorderRadius.circular(8),
                onTap: () =>
                    controller.isOpen ? controller.close() : controller.open(),
                child: Chip(
                  avatar: const Icon(Icons.sort, size: 16),
                  label: Text(_sortLabel(store.mediaSort)),
                ),
              ),
            ),
            Text(
              '共 ${store.media.total} 个',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
        // 清理是**破坏性**动作：单独一组，别跟筛选混在一起
        FilterGroup(
          title: '清理',
          children: [
            TextButton.icon(
              onPressed: _busy ? null : _clearUnlinked,
              icon: const Icon(Icons.cleaning_services_outlined, size: 16),
              label: const Text('清除未关联产物'),
              style: TextButton.styleFrom(
                foregroundColor: theme.colorScheme.error,
              ),
            ),
          ],
        ),
      ],
    );
  }

  String _sortLabel(String s) => switch (s) {
    'oldest' => '最旧加入',
    'name' => '按文件名',
    'largest' => '按体积',
    'favorite' => '收藏优先',
    _ => '最新加入',
  };

  Widget _buildBody(BuildContext context, LibraryStore store) => LayoutBuilder(
    builder: (context, constraints) =>
        _libraryBody(context, store, constraints.maxWidth),
  );

  /// 集合面板的宽度：**随可用宽度缩放**，并夹在一个合理区间里。
  ///
  /// 用户 2026-10-18："面板是固定宽度，在窄屏很难用。" —— 固定 380 在 900 出头的小窗口上
  /// 会把网格挤到只剩一点点，而在 2560 的大屏上又显得局促。
  ///
  /// ⚠ 这两个数**不是布局断点**（断点仍然只有 `kWideLayoutMinWidth` 一个）：
  /// 它们是"这块面板自己长什么样"的尺寸区间。
  static double collectionPanelWidth(double available) => (available * 0.32)
      .clamp(kCollectionPanelMinWidth, kCollectionPanelMaxWidth);

  Widget _libraryBody(
    BuildContext context,
    LibraryStore store,
    double available,
  ) {
    if (store.loadingMedia && store.media.items.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (store.mediaError != null) {
      return ErrorState(
        message: store.mediaError!,
        onRetry: store.refreshMedia,
      );
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

    final theme = Theme.of(context);
    final grid = Column(
      children: [
        Expanded(
          // 左右滑动 = 翻当前页（与预设页同一套交互，见 SwipePageView）
          child: SwipePageView(
            page: store.mediaPage,
            pages: store.media.pages,
            onPageChanged: store.setMediaPage,
            child: GridView.builder(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 96),
              // 每个格子**已经**带了自己的 RepaintBoundary（SliverChildBuilderDelegate
              // 的 addRepaintBoundaries 默认就是 true），所以这里不用再手包一层；
              // 但 keep-alive 媒体格子用不上（没有输入框 / 可保留的选择），关掉就少两个
              // 包装 Element + State 要建。cacheExtent 也用默认的 250 逻辑像素：
              // 只多预建一行的量，正是它挡住了"滑动时把几百张离屏图都解出来"。
              addAutomaticKeepAlives: false,
              gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                maxCrossAxisExtent: 230,
                crossAxisSpacing: 10,
                mainAxisSpacing: 10,
                childAspectRatio: 1,
              ),
              itemCount: store.media.items.length,
              itemBuilder: (context, i) {
                final m = store.media.items[i];
                // 集合格子（同一批微调的产物折叠成一张）：叠放封面 + 右下角总数。
                // 判定完全来自后端（`collectionId` + `collectionSize > 1`），前端不猜。
                //
                // ⚠ 例外是「仅收藏」：那一档**无视集合**，每个收藏都要显示成**单张图**
                // （用户 2026-10-24）。后端的折叠也确实是关掉的，这里再把格子的
                // "集合外观"关掉 —— 两件事必须同时做，不然会看到一个只含收藏的文件夹。
                final favoritesFlat = store.mediaFavoritesFlat;
                final grouped = m.isCollection && !favoritesFlat;
                final thumb = grouped
                    ? CollectionThumb(
                        media: m,
                        previewIds: m.collectionPreview,
                        selectionMode: _selectionMode,
                        selected: _picked.containsKey(m.id),
                        onToggleSelect: () => _togglePick(m),
                        // 点文件夹 = **展开列出成员**（用户 2026-10-18："点进文件夹…再选择其一
                        // 进入它的详情"），不再直接跳封面的详情页。
                        onTap: () => _showFolder(m),
                      )
                    : MediaThumb(
                        media: m,
                        selectionMode: _selectionMode,
                        selected: _picked.containsKey(m.id),
                        onToggleSelect: () => _togglePick(m),
                        onTap: () => _openDetail(m.id),
                      );
                return GestureDetector(
                  // 长按 = 进"整理/多选"模式（两件事合成一个模式，用户 2026-10-18 拍板）。
                  // ⚠ 集合格子进来时**整组计入**，不然"选了一组、删掉一张"又会重演。
                  onLongPress: () => setState(() {
                    _selectionMode = true;
                    _pickOf(m);
                  }),
                  // 右键：单个产物最常用的三个操作，不用先点进详情页。
                  // 这里把 itemBuilder 的 context 传下去 —— 它在 ContextMenuScope **里面**，
                  // 而 `_GalleryPageState.context` 在 Scope 外面（页面 build 才返回 Scope），
                  // 用后者去找右键菜单宿主必然找不到。
                  onSecondaryTapDown: (details) =>
                      _showThumbMenu(context, m, details.globalPosition),
                  child: _draggableTile(m, thumb),
                );
              },
            ),
          ),
        ),
        PageBar(
          page: store.mediaPage,
          pages: store.media.pages,
          onChanged: store.setMediaPage,
        ),
      ],
    );

    final open = _openFolder != null;
    // ⚠ 两件事**分工**，别混：
    //
    //  · **形态**（停靠 / 弹窗）只由**用户的设置**决定 —— 宽档读
    //    `collectionDetailWide`、窄档读 `collectionDetailNarrow`
    //    （用户 2026-10-18："能不能让我自由选择在电脑/手机排版的画廊的集合详情行为？"）。
    //    这里**刻意没有任何"空间不够就回退弹窗"的分支**：那是上一版替用户做的决定，
    //    而这次他要的恰恰是"我自己定" —— 选了停靠就停靠，哪怕窗口很窄。
    //  · **面板宽度**由 [collectionPanelWidth] 决定（随可用宽度缩放、夹在 300~480）。
    //    它是"停靠时面板长什么样"，跟"停靠还是弹窗"是两件事 —— 一个字都不改。
    final settings = context.watch<SettingsStore>();
    final wide = MediaQuery.sizeOf(context).width >= kWideLayoutMinWidth;
    final style = wide
        ? settings.collectionDetailWide
        : settings.collectionDetailNarrow;
    final docked = open && style == CollectionDetailStyle.dock;
    final panelWidth = collectionPanelWidth(available);

    return Stack(
      children: [
        Positioned.fill(
          child: docked
              ? Row(
                  children: [
                    Expanded(child: grid),
                    // 停靠：面板**占真实布局空间**（网格自己收窄），谁都不遮谁 ——
                    // 于是"让开 / 回来 / 只藏不销毁"整套机制都不需要。
                    // （选停靠的是**用户**，不是"窗口够宽"这个判据，见上面 [docked]）
                    SizedBox(
                      width: panelWidth,
                      child: LayoutBuilder(
                        builder: (context, constraints) =>
                            _folderPanel(theme, constraints, docked: true),
                      ),
                    ),
                  ],
                )
              : grid,
        ),
        // 用户选了弹窗才用居中弹窗（虚化背景；拖出去时它彻底让开，见 `_folderLayer`）
        if (open && !docked) _folderLayer(theme),
        // 拖动中给一条"移出集合"的落点提示（两种形态都有；
        // 现在**随手松手**也算移出，它只是顺手接得住）
        if (open && _folderDragging) _detachStrip(theme),
      ],
    );
  }

  // -------------------------------------------------------------------------
  //  右键菜单
  // -------------------------------------------------------------------------

  /// 画廊里对单个产物右键：关联提示词 / 收藏 / 删除。
  ///
  /// 这三个是"看图中途最想干的事"，走右键就不用先点进详情页再出来。
  ///
  /// [source] 必须是**在 `ContextMenuScope` 里面**的 context（这里传的是格子的
  /// `itemBuilder` context）：`showAppContextMenu` 要靠它往上找菜单宿主。
  Future<void> _showThumbMenu(
    BuildContext source,
    MediaAsset media,
    Offset globalPosition,
  ) async {
    // 多选态：右键按桌面惯例处理 —— 点未选中的那一格 = 把选择换成它，点已选中的 = 保持。
    // 菜单**只给"对整批"的动作**，不再出现"删这一张"，避免"以为删一组、其实删一张"。
    if (_selectionMode) {
      if (!_picked.containsKey(media.id)) {
        setState(() {
          _picked.clear();
          _pickOf(media);
        });
      }
      final count = _pickedCount;
      final picked = await showContextMenuAt<String>(
        source,
        globalPosition: globalPosition,
        items: (_) => [
          MenuOption(
            value: 'deleteSelected',
            icon: Icons.delete_outline,
            label: '删除选中的 $count 张',
            danger: true,
          ),
          const MenuOption(
            value: 'cancelSelection',
            icon: Icons.close,
            label: '取消选择',
          ),
        ],
      );
      if (picked == null || !mounted) return;
      if (picked == 'deleteSelected') {
        final ids = await _expandPicks();
        if (ids == null || !mounted) return;
        await _deleteMediaIds(ids, what: '$count 张');
      } else {
        _exitSelection();
      }
      return;
    }

    // 集合格子（叠放 N 张）与普通格子的删除语义**不同**：前者要能"删整组"。
    // 「仅收藏」档无视集合（每个格子都是单图），所以那里**一律按单张**处理 ——
    // 否则菜单会对着界面上的一张图说"删除整个集合（3 张）"。
    final favoritesFlat = context.read<LibraryStore>().mediaFavoritesFlat;
    final isGroup = media.isCollection && !favoritesFlat;
    final action = await showContextMenuAt<String>(
      source,
      globalPosition: globalPosition,
      items: (_) => [
        MenuOption(
          value: 'link',
          icon: Icons.link,
          label: media.hasPrompt ? '更换关联提示词…' : '关联提示词…',
        ),
        MenuOption(
          value: 'favorite',
          icon: media.favorite ? Icons.star_border : Icons.star,
          label: media.favorite ? '取消收藏' : '收藏',
        ),
        // 手动标记成人内容（用户 bug 2026-09-20）：AI 没声明过的（自己导入的、老数据）
        // 由用户说了算 —— 「预览图模糊」只作用于标记过的条目。
        MenuOption(
          value: 'nsfw',
          icon: media.nsfw ? Icons.visibility : Icons.visibility_off_outlined,
          label: media.nsfw ? '取消 NSFW 标记' : '标记为 NSFW',
        ),
        // ⚠ 集合格子必须**两个删除项并列**，且名字里带数量 ——
        // 这是"删一张 / 删整组"的唯一区分点（用户 bug 2026-10-18）。
        if (isGroup)
          MenuOption(
            value: 'deleteGroup',
            icon: Icons.delete_sweep_outlined,
            label: '删除整个集合（${media.collectionSize} 张）',
            danger: true,
            dividerBefore: true,
          ),
        MenuOption(
          value: 'delete',
          icon: Icons.delete_outline,
          label: isGroup ? '只删封面这一张' : '删除',
          danger: true,
          dividerBefore: !isGroup,
        ),
        // 归类纠错（用户 2026-10-18："生成的内容自动分类到错误的集合…允许用户处理"）：
        // 整组拆开 = 每个成员都打上"用户已移出"，之后回填不会再把它们吸回来。
        if (isGroup)
          MenuOption(
            value: 'detachGroup',
            icon: Icons.call_split,
            label: '拆开这一组（${media.collectionSize} 张）',
            dividerBefore: true,
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
        messenger.showSnackBar(
          SnackBar(content: Text('已关联到「${picked.title}」')),
        );
        break;
      case 'favorite':
        await store.api.updateMedia(media.id, favorite: !media.favorite);
        await store.refreshAll();
        break;
      case 'nsfw':
        await store.api.updateMedia(media.id, nsfw: !media.nsfw);
        await store.refreshAll();
        messenger.showSnackBar(
          SnackBar(
            content: Text(media.nsfw ? '已取消 NSFW 标记' : '已标记为 NSFW，预览图会按设置打码'),
          ),
        );
        break;
      case 'delete':
        await _deleteMediaIds([media.id], what: '「${media.originalName}」');
        break;
      case 'deleteGroup':
        final ids = await _collectionMemberIds(media.collectionId);
        if (ids == null || !mounted) return;
        await _deleteMediaIds(ids, what: '这个集合（共 ${ids.length} 张）');
        break;
      case 'detachGroup':
        final ids = await _collectionMemberIds(media.collectionId);
        if (ids == null || !mounted) return;
        await _detachFromCollections(ids, what: '这一组（${ids.length} 张）');
        break;
    }
  }

  /// 把一批产物**移出集合**（归类纠错的正解）。
  ///
  /// 后端在 `clearCollection` 时同时写 `collection_opt_out = 1` —— 少了那一步，
  /// 用户刚拆出来的图会在**下次启动**被回填静默吸回原组（这条是本仓库最隐蔽的坑之一）。
  /// 拖动载荷：把一格变成"可拖的"。
  ///
  /// 桌面与触屏**两套手势**，因为翻页方式不同：
  /// * 桌面用**滚轮**翻页 ⇒ 即时左键拖不会跟滚动打架，而且这也正是文件管理器的习惯
  ///   （用户 2026-10-18 报"电脑端画廊并不支持拖动行为"，就是长按拖在鼠标上不直观）；
  /// * 触屏上必须长按才起拖，否则轻轻一划就变成拖图。
  Widget _dragPayload(
    MediaAsset media,
    Widget child, {
    VoidCallback? onDragStarted,
    void Function(DragUpdateDetails)? onDragUpdate,
    VoidCallback? onDragCompleted,
    VoidCallback? onDragCanceled,
  }) {
    // 反馈只用一小片文字，**不拿整张图当 feedback**：几百 KB 的图跟在指针下面很贵，
    // 而且盖住落点反而看不清放到哪儿了。
    // 「仅收藏」档里那格子就是一张单图，标签也别再说"一组 N 张"。
    final favoritesFlat = context.read<LibraryStore>().mediaFavoritesFlat;
    final grouped = media.isCollection && !favoritesFlat;
    final feedback = _DragChip(
      label: grouped ? '一组的 ${media.collectionSize} 张' : media.originalName,
    );
    final dragging = Opacity(opacity: 0.35, child: child);
    if (_mouseFirst) {
      return Draggable<int>(
        data: media.id,
        feedback: feedback,
        childWhenDragging: dragging,
        onDragStarted: onDragStarted,
        onDragUpdate: onDragUpdate,
        onDragCompleted: onDragCompleted,
        onDraggableCanceled: onDragCanceled == null
            ? null
            : (_, _) => onDragCanceled(),
        child: child,
      );
    }
    return LongPressDraggable<int>(
      data: media.id,
      feedback: feedback,
      childWhenDragging: dragging,
      onDragStarted: onDragStarted,
      onDragUpdate: onDragUpdate,
      onDragCompleted: onDragCompleted,
      onDraggableCanceled: onDragCanceled == null
          ? null
          : (_, _) => onDragCanceled(),
      child: child,
    );
  }

  /// 一格的完整拖动语义：能拖、也能接（拖到它头上 = 并进它所在的集合）。
  Widget _draggableTile(MediaAsset media, Widget child) {
    // 桌面随时可拖；触屏只在整理模式里拖（模式外那一下长按是"进模式"）
    if (!_mouseFirst && !_selectionMode) return child;
    return DragTarget<int>(
      // 拖到自己头上不算
      onWillAcceptWithDetails: (details) => details.data != media.id,
      onAcceptWithDetails: (details) =>
          unawaited(_dropOnto(draggedId: details.data, target: media)),
      builder: (context, candidates, _) => DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: candidates.isEmpty
                ? Colors.transparent
                : Theme.of(context).colorScheme.primary,
            width: 2,
          ),
        ),
        child: _dragPayload(media, child),
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  "点开文件夹"（非模态浮层）
  // -------------------------------------------------------------------------

  Future<void> _showFolder(MediaAsset collection) async {
    setState(() {
      _openFolder = collection;
      _folderMembers = null;
    });
    final members = await _collectionMembers(collection.collectionId);
    if (!mounted || _openFolder?.id != collection.id) return;
    setState(() => _folderMembers = members ?? const []);
  }

  void _closeFolder() {
    if (_openFolder == null) return;
    setState(() {
      _openFolder = null;
      _folderMembers = null;
      _folderSelection = false;
      _folderPicked.clear();
      _folderDragging = false;
      _folderRect = null;
      // 换一个新的 key：下一组要用一份干净的矩形
      _folderKey = GlobalKey();
    });
  }

  /// 一个集合的成员（**带缩略图信息**，展开文件夹要画图，不能只要 id）。
  Future<List<MediaAsset>?> _collectionMembers(int? collectionId) async {
    if (collectionId == null) return const [];
    final store = context.read<LibraryStore>();
    try {
      final page = await store.api.listMedia(
        collectionId: collectionId,
        sort: 'oldest',
        size: 500,
      );
      return page.items;
    } catch (e) {
      if (!mounted) return null;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('展开这一组失败，$e')));
      return null;
    }
  }

  /// 展开的文件夹：**居中弹窗 + 虚化背景**（用户 2026-10-18："虚化后面的内容，
  /// 然后在正中心展开"）。
  ///
  /// # 为什么它是页面里的一层，而不是 `showDialog`
  ///
  /// 用户要的拖动逻辑是："把内容移出到弹窗外、弹窗关闭，再移动到某个元素上，
  /// 松手完成移动。" —— **拖到一半时弹窗必须让开**。而 `showDialog` 有两条硬伤：
  /// ① 弹窗里挂着 `Draggable`，一旦 pop，那棵子树连同它的 State 一起被销毁，
  /// 拖拽当场断掉；② 它铺的 `ModalBarrier` 会把底下的画廊整个挡在命中测试之外，
  /// 那样松手也落不到格子上。
  ///
  /// 这里的做法：拖动一开始就把弹窗**藏起来但不销毁**（`Visibility(maintainState: true)`），
  /// 同时撤掉背景那两层。于是拖拽载荷（活在根 `Overlay` 里）还在、背景也让开了，
  /// 松手就能落在画廊的格子上；指针拖回弹窗范围内时它再显出来。
  Widget _folderLayer(ThemeData theme) {
    final dragging = _folderDragging;
    return Stack(
      // ⚠ `StackFit.expand` 不能少：这一层是外层 `Stack` 的**非定位子节点**，
      // 不撑开的话它会被收缩到"最大子节点"（也就是那个弹窗）那么大，
      // 于是 `Center` 变成"在弹窗里居中" —— 弹窗跑到**左上角**，
      // 而底部那条也会贴到弹窗下沿而不是窗口底部。
      fit: StackFit.expand,
      children: [
        // 背景：虚化 + 压暗。拖动开始后整层让开（不再吃指针）
        if (!dragging) ...[
          Positioned.fill(
            child: BackdropFilter(
              filter: ui.ImageFilter.blur(sigmaX: 6, sigmaY: 6),
              child: ColoredBox(color: Colors.black.withValues(alpha: 0.42)),
            ),
          ),
          // 点背景 = 关掉（与弹窗的惯例一致）
          Positioned.fill(
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: _closeFolder,
            ),
          ),
        ],
        // ⚠ 尺寸要用**这一层真实拿到的约束**（`LayoutBuilder`），不能用
        // `MediaQuery` 的屏幕尺寸：页面里还有标题栏 / 搜索栏 / 底栏，
        // 用屏幕高度算出来的弹窗会**顶到窗口底部**，正好压在"移出集合"条上，
        // 于是"拖到底部条"被误判成"拖回弹窗里"，弹窗一让开又弹回来、永远放不下。
        LayoutBuilder(
          builder: (context, constraints) => Center(
            child: Padding(
              // 底部留出一条的位置：弹窗**永远不许**压到它上面
              padding: const EdgeInsets.fromLTRB(20, 20, 20, 72),
              child: Visibility(
                visible: !dragging,
                maintainState: true,
                child: _folderPanel(theme, constraints),
              ),
            ),
          ),
        ),
      ],
    );
  }

  /// 集合面板本体：**停靠还是居中弹窗由用户的设置决定**
  /// （`SettingsStore.collectionDetailWide` / `collectionDetailNarrow`，见 [_libraryBody]）。
  /// [docked] 只影响它自己的外观（圆角 / 阴影 / 尺寸），不参与"谁决定形态"。
  ///
  /// ⚠ **整个面板本身是一个落点**：只把成员格子做成落点的话，"把外面的一张加进这一组"
  /// 就没有地方可放 —— 用户 2026-10-18 报的 B 的那个 bug 正是这个。
  Widget _folderPanel(
    ThemeData theme,
    BoxConstraints constraints, {
    bool docked = false,
  }) {
    final folder = _openFolder!;
    final members = _folderMembers;
    final picked = _folderPicked.length;
    final panel = Material(
      key: _folderKey,
      elevation: docked ? 0 : 8,
      borderRadius: docked ? BorderRadius.zero : BorderRadius.circular(16),
      clipBehavior: Clip.antiAlias,
      color: theme.colorScheme.surfaceContainerHigh,
      child: SizedBox(
        // 手机（窄屏）上**限制弹窗大小**：用户拍板的"手机 UI 限制弹窗大小就可以了"
        width: docked
            ? constraints.maxWidth
            : (constraints.maxWidth * 0.78).clamp(320.0, 620.0),
        height: docked
            ? constraints.maxHeight
            : (constraints.maxHeight * 0.80).clamp(240.0, 640.0),
        child: Column(
          children: [
            ListTile(
              dense: true,
              leading: const Icon(Icons.folder_open, size: 18),
              title: Text(
                '这一组（${members?.length ?? folder.collectionSize} 张）',
                style: theme.textTheme.labelLarge,
              ),
              subtitle: Text(
                _folderSelection
                    ? '已选 $picked 张'
                    : '点一张进详情，长按可多选。把一张拖到弹窗外，就是移出或转移集合',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              trailing: IconButton(
                tooltip: '关闭',
                icon: const Icon(Icons.close, size: 18),
                onPressed: _closeFolder,
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: members == null
                  ? const Center(child: CircularProgressIndicator())
                  : members.isEmpty
                  ? Center(
                      child: Text('这一组空了', style: theme.textTheme.bodySmall),
                    )
                  : GridView.builder(
                      padding: const EdgeInsets.all(12),
                      gridDelegate:
                          const SliverGridDelegateWithMaxCrossAxisExtent(
                            maxCrossAxisExtent: 120,
                            crossAxisSpacing: 10,
                            mainAxisSpacing: 10,
                          ),
                      itemCount: members.length,
                      itemBuilder: (context, i) =>
                          _folderMemberTile(theme, members[i]),
                    ),
            ),
            // 集合内**多选**（用户 2026-10-18："在集合内我不能多选操作"）
            if (_folderSelection && picked > 0)
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 6,
                ),
                decoration: BoxDecoration(
                  border: Border(top: BorderSide(color: theme.dividerColor)),
                ),
                child: Row(
                  children: [
                    Text('已选 $picked 张', style: theme.textTheme.labelLarge),
                    const Spacer(),
                    TextButton.icon(
                      onPressed: _folderSelectAll,
                      icon: const Icon(Icons.select_all, size: 16),
                      label: const Text('全选'),
                    ),
                    TextButton.icon(
                      onPressed: _folderDetachPicked,
                      icon: const Icon(Icons.call_split, size: 16),
                      label: const Text('移出集合'),
                    ),
                    TextButton.icon(
                      onPressed: _folderDeletePicked,
                      icon: const Icon(Icons.delete_outline, size: 16),
                      label: const Text('删除'),
                      style: TextButton.styleFrom(
                        foregroundColor: theme.colorScheme.error,
                      ),
                    ),
                    TextButton(
                      onPressed: () => setState(() {
                        _folderSelection = false;
                        _folderPicked.clear();
                      }),
                      child: const Text('取消'),
                    ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );

    return DragTarget<int>(
      // 拖到面板上（标题 / 空白 / 成员之间的缝）= **加进这一组**
      onAcceptWithDetails: (details) =>
          unawaited(_dropOnto(draggedId: details.data, target: folder)),
      builder: (context, candidates, _) => DecoratedBox(
        decoration: BoxDecoration(
          border: Border.all(
            color: candidates.isEmpty
                ? Colors.transparent
                : theme.colorScheme.primary,
            width: 3,
          ),
        ),
        // ⚠ `DragTarget.builder` 的三个参数是 (context, candidateData, rejectedData) ——
        // **没有 `child`**。把第三个参数命名成 `child` 会静默把它当成列表用。
        child: panel,
      ),
    );
  }

  // ---- 文件夹里的多选与拖动状态 ------------------------------------------

  void _toggleFolderPick(int mediaId) {
    setState(() {
      if (!_folderPicked.remove(mediaId)) _folderPicked.add(mediaId);
    });
  }

  void _folderSelectAll() {
    final members = _folderMembers;
    if (members == null) return;
    setState(() {
      _folderSelection = true;
      _folderPicked
        ..clear()
        ..addAll(members.map((m) => m.id));
    });
  }

  Future<void> _folderDetachPicked() async {
    final ids = List.of(_folderPicked);
    if (ids.isEmpty) return;
    await _detachFromCollections(ids, what: '${ids.length} 张');
  }

  Future<void> _folderDeletePicked() async {
    final ids = List.of(_folderPicked);
    if (ids.isEmpty) return;
    await _deleteMediaIds(ids, what: '选中的 ${ids.length} 张');
  }

  /// 拖动**开始**时把弹窗当前的矩形记下来。
  ///
  /// 为什么不每次移动都现算：弹窗一旦被藏起来（`Offstage`），它的渲染对象就不再参与
  /// 布局 —— 那时候再问它的位置就不可靠了。而它在整个拖动过程里**不会动**，
  /// 记一次就够。
  void _captureFolderRect() {
    final box = _folderKey.currentContext?.findRenderObject() as RenderBox?;
    if (box != null && box.hasSize) {
      _folderRect = box.localToGlobal(Offset.zero) & box.size;
    }
  }

  /// 拖动中：指针离开弹窗 → 弹窗让开（**只藏不销毁**，否则拖拽当场断）。
  ///
  /// ⚠ **一旦让开就一直让开**，不再"拖回来就重新挡住"。之前那版会让开又回来，
  /// 结果是窗口中间那一片格子**永远落不到** —— 用户 2026-10-18 报的正是这个。
  void _onFolderDragMove(DragUpdateDetails details) {
    if (_folderDragging) return;
    final rect = _folderRect;
    if (rect == null) return;
    if (!rect.contains(details.globalPosition)) {
      setState(() => _folderDragging = true);
    }
  }

  void _endFolderDrag() {
    if (_folderDragging) setState(() => _folderDragging = false);
    _folderRect = null;
  }

  /// 落到了某个落点上（某个格子 / 底部那条）：这一趟的目的已经达到。
  ///
  /// **只在"拖出去过"时才收掉弹窗** —— 在弹窗内部把一张拖到另一张上，不该把弹窗关掉
  /// （用户 2026-10-18："在集合内拖动内容（终点在集合弹窗内），会导致集合弹窗关闭"）。
  void _finishFolderDrag() {
    final left = _folderDragging;
    _endFolderDrag();
    if (left) _closeFolder();
  }

  /// 松手在**所有落点之外**。
  ///
  /// 用户 2026-10-18："我还希望集合弹窗内将内容拖动到弹窗外，能够直接解除移出集合，
  /// 而不再只能拖动到屏幕底。" —— 拖出弹窗之后**随便在哪儿松手**都算移出，
  /// 不用去够底部那条横条（它现在只是一条提示，顺手也能落）。
  Future<void> _folderDragOut(int mediaId) async {
    final left = _folderDragging;
    _endFolderDrag();
    if (!left) return; // 在弹窗里松手的，什么都不做
    final ids = await _expandOne(mediaId);
    if (ids == null || !mounted) return;
    await _detachFromCollections(ids, what: '${ids.length} 张');
  }

  /// 拖动中的"移出集合"落点条（弹窗让开之后才出现）。
  Widget _detachStrip(ThemeData theme) {
    return Positioned(
      left: 0,
      right: 0,
      bottom: 0,
      child: DragTarget<int>(
        onAcceptWithDetails: (details) => unawaited(_dropOut(details.data)),
        builder: (context, candidates, _) => Container(
          padding: const EdgeInsets.symmetric(vertical: 14),
          color: theme.colorScheme.surfaceContainerHigh,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                Icons.call_split,
                size: 16,
                color: candidates.isEmpty
                    ? theme.colorScheme.outline
                    : theme.colorScheme.primary,
              ),
              const SizedBox(width: 8),
              Text(
                '拖到这里 = 移出集合',
                style: theme.textTheme.labelLarge?.copyWith(
                  color: candidates.isEmpty
                      ? theme.colorScheme.outline
                      : theme.colorScheme.primary,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// 文件夹里的一张：点进详情 / 点选；长按进多选；也能拖出去。
  Widget _folderMemberTile(ThemeData theme, MediaAsset media) {
    final store = context.read<LibraryStore>();
    final selected = _folderPicked.contains(media.id);
    final tile = InkWell(
      onTap: _folderSelection
          ? () => _toggleFolderPick(media.id)
          : () => _openDetail(media.id),
      // 长按 = 进集合内多选（与画廊主界面同一套肌肉记忆）
      onLongPress: () => setState(() {
        _folderSelection = true;
        _folderPicked.add(media.id);
      }),
      child: Stack(
        fit: StackFit.expand,
        children: [
          Container(
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(8),
              border: Border.all(
                color: selected
                    ? theme.colorScheme.primary
                    : theme.dividerColor,
                width: selected ? 2 : 1,
              ),
            ),
            child: Image.network(
              // 与 `MediaThumb` **同一条取法**（`absolute(thumbUrl ?? fileUrl)`）：
              // 两处各拼一份就会出现"画廊里有图、文件夹里裂图"。
              store.api.absolute(media.thumbUrl ?? media.fileUrl),
              // 稳定锚点：用例要能精确点到**弹窗里**的那张（画廊里也有一堆 Image）
              key: const Key('folder-member-thumb'),
              fit: BoxFit.cover,
              filterQuality: FilterQuality.low,
              cacheWidth: (120 * MediaQuery.devicePixelRatioOf(context))
                  .round()
                  .clamp(1, 512),
              errorBuilder: (_, _, _) => Icon(
                Icons.broken_image_outlined,
                size: 16,
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          if (_folderSelection)
            Positioned(
              top: 4,
              left: 4,
              child: Icon(
                selected ? Icons.check_circle : Icons.circle_outlined,
                size: 18,
                color: selected ? theme.colorScheme.primary : Colors.white70,
              ),
            ),
          // 收藏标记（用户 2026-10-24）：集合里的成员被收藏时也要**看得出来**，
          // 与画廊里散图的星标一致 —— 以前展开后收藏状态整个消失了。
          // 小图上不加渐变（那是 `MediaThumb` 的做法），改成垫一枚半透明圆底，
          // 浅色缩略图上星标也读得清；它在右上角，不与左上角的勾选圈打架。
          if (media.favorite)
            Positioned(
              top: 4,
              right: 4,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: Colors.black.withValues(alpha: 0.42),
                ),
                child: const Icon(Icons.star, size: 13, color: Colors.amber),
              ),
            ),
        ],
      ),
    );
    return DragTarget<int>(
      onWillAcceptWithDetails: (details) => details.data != media.id,
      onAcceptWithDetails: (details) =>
          unawaited(_dropOnto(draggedId: details.data, target: media)),
      builder: (context, candidates, _) => DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
            color: candidates.isEmpty
                ? Colors.transparent
                : theme.colorScheme.primary,
            width: 2,
          ),
        ),
        child: _dragPayload(
          media,
          tile,
          onDragStarted: _captureFolderRect,
          onDragUpdate: _onFolderDragMove,
          onDragCompleted: _finishFolderDrag,
          // ⚠ 落到**所有落点之外**时走这条：松手在弹窗外的空地上 = 直接移出集合
          onDragCanceled: () => unawaited(_folderDragOut(media.id)),
        ),
      ),
    );
  }

  /// 拖出集合：单张就是它自己，**集合格子 = 整组一起拆开**。
  Future<void> _dropOut(int draggedId) async {
    final ids = await _expandOne(draggedId);
    if (ids == null || !mounted) return;
    await _detachFromCollections(ids, what: '${ids.length} 张');
  }

  /// 拖到某一格上松手。
  ///
  /// 三种情况，语义都来自用户拍板的那几条：
  /// 1. 拖的是**集合格子** → **整组一起过去**（这是"动整组"的落点）；
  /// 2. 目标已经在某个集合里 → 并进去；
  /// 3. 目标还没有集合 → **当场新建一个**（目标 + 拖过来的）—— 这就是手机里
  ///    "把两个图标拖到一起就成了一个文件夹"。
  ///
  /// kind 不一致**在前端就先拦一道**并说人话：后端也会拒（400），但让用户等到一次
  /// 往返之后才看到红字，不如当场说。
  Future<void> _dropOnto({
    required int draggedId,
    required MediaAsset target,
  }) async {
    final dragged = context.read<LibraryStore>().media.items.firstWhere(
      (item) => item.id == draggedId,
      orElse: () => target,
    );
    if (dragged.kind != target.kind) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            '${dragged.kind.name.toUpperCase()} 放不进 ${target.kind.name.toUpperCase()} 的集合',
          ),
        ),
      );
      return;
    }

    final ids = await _expandOne(draggedId);
    if (ids == null || !mounted) return;

    final store = context.read<LibraryStore>();
    final collectionId = target.collectionId;
    if (collectionId != null) {
      await _run(() async {
        for (final id in ids) {
          await store.api.updateMedia(id, collectionId: collectionId);
        }
      }, '已把 ${ids.length} 张并进这一组');
      return;
    }

    final merged = <int>{target.id, ...ids}.toList(growable: false);
    await _run(() async {
      await store.api.createCollection(merged);
    }, '已新建集合（${merged.length} 张）');
  }

  /// 把**一项**选中展开成具体产物 id。拉不到回 `null`。
  ///
  /// ⚠ 别只看 `_picked`：**桌面可以直接拖，不需要先勾选**。
  /// "拖集合格子 = 动整组"这条与"有没有先选过它"无关 —— 只看 `_picked` 的话，
  /// 桌面上拖一个集合格子只会搬走封面那一张（正是用户报的那类 bug）。
  Future<List<int>?> _expandOne(int mediaId) async {
    // ① 弹窗里勾了一批 → 拖其中任意一张 = **整批一起走**。
    // ⚠ 这里必须看 `_folderPicked`：弹窗里的多选与画廊主界面那个 `_picked`
    // **是两套**，只看后者的话"集合内多选之后拖一张"只会搬走那一张
    // （用户 2026-10-18："同时要做到可以多选拖拽"）。
    if (_folderPicked.length > 1 && _folderPicked.contains(mediaId)) {
      return List.of(_folderPicked);
    }
    // ② 画廊多选态勾了一批 → 同理。否则"全选之后拖出去"一次只搬一张。
    if (_picked.length > 1 && _picked.containsKey(mediaId)) {
      return _expandPicks();
    }
    // ③ 「仅收藏」档**无视集合**：拖哪张就只动哪张 —— 界面上它本来就是一张单图，
    // 按集合摊开会让"拖走一张、结果动了整组"（与显示对不上）。
    if (context.read<LibraryStore>().mediaFavoritesFlat) return [mediaId];
    final pick = _picked[mediaId];
    final collectionId = pick?.collection ?? _mediaById(mediaId)?.collectionId;
    if (collectionId == null) return [mediaId];
    final members = await _collectionMemberIds(collectionId);
    if (members == null) return null;
    return members.isEmpty ? [mediaId] : members;
  }

  /// 按 id 找当前页里的那一格（拖拽只发生在看得见的格子上）。
  MediaAsset? _mediaById(int mediaId) {
    for (final item in context.read<LibraryStore>().media.items) {
      if (item.id == mediaId) return item;
    }
    return null;
  }

  Future<void> _detachFromCollections(
    List<int> ids, {
    required String what,
  }) async {
    if (ids.isEmpty) return;
    await _run(() async {
      final store = context.read<LibraryStore>();
      for (final id in ids) {
        await store.api.updateMedia(id, clearCollection: true);
      }
    }, '已把$what移出集合');
  }

  // -------------------------------------------------------------------------
  //  删除
  // -------------------------------------------------------------------------

  /// 删一批产物：**先按真实条数问一遍**，再逐条删，最后统一刷新一次。
  ///
  /// 为什么逐条而不另造批量端点：删除的语义（清集合、删缩略图、删磁盘文件）已经在
  /// `DELETE /api/media/{id}` 里，复制一份必然分叉。
  Future<void> _deleteMediaIds(List<int> ids, {required String what}) async {
    if (ids.isEmpty) return;
    final ok = await confirmDialog(
      context,
      title: ids.length == 1 ? '删除产物' : '删除 ${ids.length} 个产物',
      message: '确定删除$what吗？磁盘上的文件也会被一并删除，且不可恢复。',
    );
    if (!ok || !mounted) return;
    await _run(() async {
      final store = context.read<LibraryStore>();
      for (final id in ids) {
        await store.api.deleteMedia(id);
      }
    }, '已删除 ${ids.length} 项');
  }

  /// 一个集合的**全部**成员 id（拉不到就回 `null`，让调用方放弃而不是删错东西）。
  ///
  /// ⚠ `media.collectionPreview` 只有前 3 张（后端为了省一次查询），**不能**当成员表用。
  /// `collectionId` 为空（孤儿产物）时返回空列表 —— 调用方自己知道那一条怎么处理。
  Future<List<int>?> _collectionMemberIds(int? collectionId) async {
    if (collectionId == null) return const [];
    final store = context.read<LibraryStore>();
    try {
      final page = await store.api.listMedia(
        collectionId: collectionId,
        sort: 'oldest',
        size: 500,
      );
      return page.items.map((e) => e.id).toList(growable: false);
    } catch (e) {
      if (!mounted) return null;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('拉取集合成员失败，未执行操作，$e')));
      return null;
    }
  }

  /// 把当前选中项摊平成**具体产物 id**：单条就是它自己，集合是整组成员。
  ///
  /// 拉不到任何一个集合就整批放弃并回 `null` —— 宁可什么都不做，也不要"以为删了 9 张、
  /// 其实只删了 3 张"。
  Future<List<int>?> _expandPicks() async {
    final ids = <int>{};
    for (final entry in _picked.entries) {
      final collectionId = entry.value.collection;
      if (collectionId == null) {
        ids.add(entry.key);
        continue;
      }
      final members = await _collectionMemberIds(collectionId);
      if (members == null) return null;
      // 成员一个都没拉到（并发删空）：退回只处理代表那一张，不静默丢
      if (members.isEmpty) {
        ids.add(entry.key);
      } else {
        ids.addAll(members);
      }
    }
    return ids.toList(growable: false);
  }

  // -------------------------------------------------------------------------
  //  批量操作
  // -------------------------------------------------------------------------

  Future<void> _bulkLink() async {
    final store = context.read<LibraryStore>();
    final picked = await showPromptPicker(context);
    if (picked == null) return;
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    await _run(() async {
      for (final id in ids) {
        await store.api.updateMedia(id, promptId: picked.id);
      }
    }, '已关联 ${ids.length} 张到「${picked.title}」');
  }

  Future<void> _bulkFavorite() async {
    final store = context.read<LibraryStore>();
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    await _run(() async {
      for (final id in ids) {
        await store.api.updateMedia(id, favorite: true);
      }
    }, '已收藏 ${ids.length} 张');
  }

  Future<void> _bulkDelete() async {
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    await _deleteMediaIds(ids, what: '选中的 ${ids.length} 张');
  }

  /// 批量**移出集合**：归类分错了之后最常用的纠错动作。
  Future<void> _bulkDetach() async {
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    await _detachFromCollections(ids, what: '选中的 ${ids.length} 张');
  }

  /// 批量**移到集合…**：选一个已有的，或者当场新建一个。
  ///
  /// kind 不一致的目标在选择器里就**置灰并说明原因** —— 不等后端回 400，
  /// 规则要在点下去之前就看得见（用户拍板："手动集合不许混图像和视频"）。
  Future<void> _bulkMove() async {
    final store = context.read<LibraryStore>();
    final ids = await _expandPicks();
    if (ids == null || !mounted) return;
    final target = await showCollectionPicker(
      context,
      mediaKind: _picked.values.first.kind,
      count: ids.length,
    );
    if (target == null || !mounted) return;

    final existing = target.collection;
    if (existing != null) {
      await _run(() async {
        for (final id in ids) {
          await store.api.updateMedia(id, collectionId: existing.id);
        }
      }, '已把 ${ids.length} 张移入「${existing.label}」');
      return;
    }
    final name = (target.title ?? '').trim();
    await _run(() async {
      await store.api.createCollection(ids, title: name);
    }, '已新建集合「${name.isEmpty ? '未命名' : name}」并移入 ${ids.length} 张');
  }

  /// 清除未关联产物：没有关联任何提示词的图片 / 视频 / 音频。
  ///
  /// 这类产物通常是"提示词被删掉了，产物留了下来"；磁盘文件会一起删，
  /// 所以先报条数再确认。
  Future<void> _clearUnlinked() async {
    final store = context.read<LibraryStore>();
    final total = await store.countUnlinkedMedia();
    if (!mounted) return;
    if (total == 0) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('没有未关联的产物')));
      return;
    }
    final ok = await confirmDialog(
      context,
      title: '清除 $total 个未关联产物？',
      message:
          '这些产物没有关联任何提示词（多为提示词被删后留下的）。\n'
          '磁盘上的文件会被删除，且不可恢复。提示词本身不受影响。',
      confirmText: '删除 $total 个',
    );
    if (!ok || !mounted) return;
    await _run(() async {
      // 每次重新取第一页：边删边翻页会因为页码前移漏掉一部分
      while (true) {
        final page = await store.api.listMedia(
          untagged: true,
          page: 1,
          size: 200,
        );
        if (page.items.isEmpty) break;
        for (final m in page.items) {
          await store.api.deleteMedia(m.id);
        }
      }
    }, '已清除 $total 个未关联产物');
  }

  /// 「重新自动整理」：把**已存的**产物按当前规则重算一遍分组。
  ///
  /// 用户 2026-10-18："把我数据库目前存入的图片重新自动整理。"
  ///
  /// 为什么要先问一句：它会**解散现有的自动分组**再重算 —— 用户看到的是"我的组
  /// 刚才还在这儿，现在散了"。所以确认框里要把"哪些会被动、哪些不会"说清楚：
  /// 手动建/改过的集合与已移出的产物后端一个都不碰。
  Future<void> _regroup() async {
    final ok = await confirmDialog(
      context,
      title: '重新自动整理？',
      message:
          '按提示词把**算法分出来的**分组重算一遍。\n\n'
          '你手动新建或调整过的集合、以及你手动移出过的产物，**都不会被改动**。',
    );
    if (!ok || !mounted) return;
    final store = context.read<LibraryStore>();
    var regrouped = 0;
    await _run(
      () async {
        regrouped = await store.api.regroupCollections();
      },
      '已重新自动整理',
      toastOf: () => '已重新自动整理，这次重算了 $regrouped 张',
      // 这一条**要**播报：重算是后台跑完才知道几件事的，光看格子变没变看不出来
      announce: true,
    );
  }

  /// 跑一个会改库的动作。
  ///
  /// ⚠ **成功不播报**（用户 2026-10-18："不许播报那么多提示"）：移动 / 移出 / 删除改完，
  /// 格子当场就变了，再弹一条"已把 N 张并进这一组"纯属噪音 —— 一屏操作下来能弹七八条，
  /// 把真正要看的东西盖住。只有**结果看不出来**的动作才 `announce: true`
  /// （例如"重新自动整理"到底重算了多少张）。**失败一律播报**，那时候不说话才是真的坏。
  Future<void> _run(
    Future<void> Function() action,
    String toast, {

    /// 提示语要**读了结果才知道**时给这个（例如"这次重算了 N 张"）。
    String Function()? toastOf,
    bool announce = false,
  }) async {
    setState(() => _busy = true);
    final store = context.read<LibraryStore>();
    final messenger = ScaffoldMessenger.of(context);
    try {
      await action();
      await Future.wait([
        store.refreshMedia(),
        store.refreshPrompts(),
        store.refreshStats(),
      ]);
      if (announce) {
        messenger.showSnackBar(
          SnackBar(content: Text(toastOf?.call() ?? toast)),
        );
      }
      if (mounted) {
        // 改完归属之后展开的文件夹已经过期了 —— 收起它，让用户看到刷新后的画廊
        _closeFolder();
        _exitSelection();
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('操作失败: $e')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

/// 收起筛选栏时露出的"当前生效的筛选摘要"。
///
/// 用户 2026-10-11 明确要求：收起来之后**必须**能看出自己在筛选状态，
/// 否则会以为"图怎么少了"。
class _FilterSummary extends StatelessWidget {
  final String text;
  final ThemeData theme;

  const _FilterSummary({required this.text, required this.theme});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        // 用 surface 角色代替"叠 45% alpha"：深色下叠色会发灰，动态取色下还会串色。
        color: theme.colorScheme.surfaceContainerHigh,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        children: [
          Icon(
            Icons.filter_alt_outlined,
            size: 14,
            color: theme.colorScheme.outline,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _SelectionBar extends StatelessWidget {
  final int count;
  final Future<void> Function() onLink;
  final Future<void> Function() onFavorite;
  final Future<void> Function() onMove;
  final Future<void> Function() onDetach;
  final Future<void> Function() onDelete;

  const _SelectionBar({
    required this.count,
    required this.onLink,
    required this.onFavorite,
    required this.onMove,
    required this.onDetach,
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
          Text('$count 张', style: theme.textTheme.titleSmall),
          const Spacer(),
          TextButton.icon(
            onPressed: onLink,
            icon: const Icon(Icons.link),
            label: const Text('关联提示词'),
          ),
          TextButton.icon(
            onPressed: onFavorite,
            icon: const Icon(Icons.star_border),
            label: const Text('收藏'),
          ),
          // 归类纠错（2026-10-18）：可以整组拖出去，也可以整批塞进另一个集合
          TextButton.icon(
            onPressed: onMove,
            icon: const Icon(Icons.drive_file_move_outline),
            label: const Text('移到集合…'),
          ),
          TextButton.icon(
            onPressed: onDetach,
            icon: const Icon(Icons.call_split),
            label: const Text('移出集合'),
          ),
          TextButton.icon(
            onPressed: onDelete,
            icon: const Icon(Icons.delete_outline),
            label: const Text('删除'),
            style: TextButton.styleFrom(
              foregroundColor: theme.colorScheme.error,
            ),
          ),
        ],
      ),
    );
  }
}

/// 拖动时跟在指针下面的那一小片提示。
///
/// 刻意**不用缩略图**：几百 KB 的图跟着指针走很贵，而且正好盖住落点。
class _DragChip extends StatelessWidget {
  final String label;

  const _DragChip({required this.label});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      elevation: 4,
      borderRadius: BorderRadius.circular(8),
      color: theme.colorScheme.secondaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 220),
          child: Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelMedium?.copyWith(
              color: theme.colorScheme.onSecondaryContainer,
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  「移到集合…」的选择器
// ---------------------------------------------------------------------------

/// 选择器的结果。
///
/// * [collection] 非空 = 移进这个已有集合；
/// * 否则看 [title] = **新建**一个集合（名字可以是空的，后端会存 NULL）。
class CollectionTarget {
  final MediaCollection? collection;
  final String? title;

  const CollectionTarget.existing(MediaCollection this.collection)
    : title = null;
  const CollectionTarget.create(this.title) : collection = null;
}

/// 选一个目标集合，或者当场新建一个。
///
/// 为什么要有 `mediaKind`：一个集合只装一种（用户 2026-10-18 拍板"不许混图像和视频"），
/// 所以装不下的目标要**在点下去之前就置灰并说明原因**，而不是等后端回 400。
Future<CollectionTarget?> showCollectionPicker(
  BuildContext context, {
  required String mediaKind,
  required int count,
}) {
  return showDialog<CollectionTarget>(
    context: context,
    builder: (_) => _CollectionPickerDialog(mediaKind: mediaKind, count: count),
  );
}

class _CollectionPickerDialog extends StatefulWidget {
  final String mediaKind;
  final int count;

  const _CollectionPickerDialog({required this.mediaKind, required this.count});

  @override
  State<_CollectionPickerDialog> createState() =>
      _CollectionPickerDialogState();
}

class _CollectionPickerDialogState extends State<_CollectionPickerDialog> {
  final _nameController = TextEditingController();
  List<MediaCollection>? _all;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _nameController.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final rows = await context.read<LibraryStore>().api.listCollections();
      if (!mounted) return;
      setState(() => _all = rows);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final rows = _all;
    return AlertDialog(
      title: Text('把 ${widget.count} 张移到…'),
      content: SizedBox(
        width: 380,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (_error != null)
              Text('读不到集合清单，$_error', style: theme.textTheme.bodySmall)
            else if (rows == null)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 16),
                child: Center(child: CircularProgressIndicator()),
              )
            else if (rows.isEmpty)
              Text('还没有任何集合 —— 在下面起个名字新建一个', style: theme.textTheme.bodySmall)
            else
              Flexible(
                child: ListView.builder(
                  shrinkWrap: true,
                  itemCount: rows.length,
                  itemBuilder: (context, i) {
                    final collection = rows[i];
                    final fits = collection.accepts(widget.mediaKind);
                    return ListTile(
                      dense: true,
                      enabled: fits,
                      leading: Icon(
                        collection.isManual
                            ? Icons.folder_outlined
                            : Icons.auto_awesome_motion,
                        size: 18,
                        color: fits ? null : theme.disabledColor,
                      ),
                      title: Text(
                        collection.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: Text(
                        fits
                            ? '${collection.kind} · ${collection.size} 张'
                            : '装不下，那个集合是 ${collection.kind}',
                        style: theme.textTheme.labelSmall,
                      ),
                      onTap: fits
                          ? () =>
                                Navigator.of(context)
                                    .pop(CollectionTarget.existing(collection))
                          : null,
                    );
                  },
                ),
              ),
            const Divider(height: 20),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _nameController,
                    decoration: const InputDecoration(
                      labelText: '新建集合的名字（可空）',
                      isDense: true,
                    ),
                    onSubmitted: (_) => _create(),
                  ),
                ),
                const SizedBox(width: 8),
                FilledButton(onPressed: _create, child: const Text('新建并移入')),
              ],
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

  void _create() =>
      Navigator.of(context).pop(CollectionTarget.create(_nameController.text));
}
