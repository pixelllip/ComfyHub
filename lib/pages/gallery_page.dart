import 'dart:async';
import 'dart:io' show Platform;
import 'dart:ui' as ui;

import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';

import '../core/layout_constants.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/collection_thumb.dart';
import '../widgets/scroll_linked_title.dart';
import '../widgets/common.dart';
import '../widgets/export_dialog.dart';
import '../widgets/media_thumb.dart';
import '../widgets/nsfw_blur.dart';
import '../widgets/paged_swipe.dart';
import '../widgets/tag_editor.dart';
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
/// ⚠ **"停靠或弹窗"不由这几个数决定**：
/// 形态**只看设置**（`SettingsStore.collectionDetailWide` / `collectionDetailNarrow`，
/// 见 [_libraryBody]），这里的宽度算式只管"停靠时这块面板多宽"。
/// 以前还有一条"扣掉面板后网格剩不下 `kCollectionGridMinWidth` 就退回弹窗"的
/// 自动回退 —— **已删除**：该行为替代使用方做决定。
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

  /// 大标题 → AppBar 标题的联动进度（见 [ScrollTitleProgress]）。
  final _titleProgress = ScrollTitleProgress();

  /// 多选里的一项：`key` 是**代表那条**的 media id（集合取封面那条）。
  ///
  /// * `size` = 这一项覆盖多少张产物（单条 = 1，集合 = 整组成员数）；
  /// * `collection` = 非空表示"这一项代表整个集合"；
  /// * `kind` = 这一项的产物类型（`IMAGE` / `VIDEO` / …）——「移到集合…」要靠它
  ///   把"装不下这一批"的目标置灰并说明（一个集合只装一种）。
  ///
  /// ⚠ 集合格子拖动时移动整组，因此选中态**不能只存代表 id**
  /// —— 以前 `Set<int>` 里一个集合格子和一张单图长得一模一样，批量删就只删掉了封面
  /// （这正是"一次只能删一张"的另一面）。
  final Map<int, ({int size, int? collection, String kind})> _picked = {};

  bool _selectionMode = false;
  bool _busy = false;

  /// 正在"点开"的集合（在弹窗中展开并列出所有图片）。
  ///
  /// ⚠ 它**不是 `showDialog`**：模态弹窗会铺一层 `ModalBarrier`，把底下的画廊整个
  /// 挡在命中测试之外 —— 那样"把成员拖出文件夹、拖到画廊里的另一个格子上"
  /// 无法收到落点（这正是所需的拖动行为）。
  /// 所以这里做成**同一个页面里的浮层**：画廊还在，落点也还在。
  MediaAsset? _openFolder;
  List<MediaAsset>? _folderMembers;

  /// 展开的这一组的**摘要**（名字 / 是算法分组还是手动集合）。
  ///
  /// 与 `_folderMembers` 分开取：名字读不到不该让成员也打不开；反过来也一样。
  /// 集合名真源在后端（`GET /api/media/collections`），前端不自己猜。
  MediaCollection? _folderCollection;

  /// 集合弹窗中的**多选**。
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
    _titleProgress.dispose();
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
    // 电脑端标题**始终在 AppBar 里**；
    // 手机端保留"大标题在正文里、滚走后在 AppBar 淡入"的联动。
    // 判据是**平台**，不是窗口宽度 —— 电脑窗口拉窄不该长成手机的样子。
    final phone = useCollapsingPageTitle;

    // ContextMenuScope：右键菜单的宿主。它**不铺 ModalBarrier**，所以菜单开着时
    // 画廊仍可滚动。一个页面只挂一个。
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
            // 桌面端标题始终在 AppBar；手机端大标题滚出视野后才淡入。
            title: _selectionMode
                ? Text('已选 $_pickedCount 张')
                : phone
                ? FadingAppBarTitle(title: '画廊', progress: _titleProgress)
                : const Text('画廊'),
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
              if (!_selectionMode && phone)
                CollapsingPageTitle(
                  title: '画廊',
                  progress: _titleProgress,
                  // 比设置页矮一点：画廊下面还压着固定的筛选栏 + 集合面板，
                  // 标题块太高会把集合面板的可视高度挤掉一截。
                  height: 44,
                ),
              _buildToolbar(context, store),
              Expanded(
                child: ScrollTitleListener(
                  progress: _titleProgress,
                  child: _buildBody(context, store),
                ),
              ),
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
          // 搜索栏 + **折叠按钮**（按钮位于搜索栏右侧）
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
          // 收起时**显示当前生效的筛选摘要** —— 否则无法判断处于筛选状态，
          // 会误认为"图片数量减少"。
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

  /// 筛选选项**按功能分区**，但**不"一种一行"**：
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
              // 已打开的集合面板不再符合该语义，一并收起，避免出现两个冲突视图。
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
  /// 面板固定宽度在窄屏难以使用 —— 固定 380 在 900 出头的小窗口上
  /// 会把网格挤到只剩一点点，而在 2560 的大屏上又显得局促。
  ///
  /// ⚠ 这两个数**不是布局断点**（断点仍然只有 `kWideLayoutMinWidth` 一个）：
  /// 它们是"这块面板自己长什么样"的尺寸区间。
  static double collectionPanelWidth(double available) => (available * 0.32)
      .clamp(kCollectionPanelMinWidth, kCollectionPanelMaxWidth);

  /// 一页产物网格。当前页与预取到的相邻页都用它 —— 翻页动画要同时画出两页，
  /// 所以这里只吃"这一页的 [data]"，不再直接读 `store.media.items`。
  Widget _mediaGrid(LibraryStore store, Paged<MediaAsset> data) =>
      GridView.builder(
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
        itemCount: data.items.length,
        itemBuilder: (context, i) {
          final m = data.items[i];
          // 集合格子（同一批微调的产物折叠成一张）：叠放封面 + 右下角总数。
          // 判定完全来自后端（`collectionId` + `collectionSize > 1`），前端不猜。
          //
          // ⚠ 例外是「仅收藏」：那一档**无视集合**，每个收藏都要显示成**单张图**
          // 后端的折叠同样关闭，这里再把格子的
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
                  // 点文件夹 = **展开列出成员**，不再直接跳转封面详情页。
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
            // 长按 = 进入"整理/多选"模式（两件事合并为一个模式）。
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
      );

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
    // 页码请求走后端分页：`mediaPage` 表示目标页码。
    // 相邻页由 store 预取（`ensureMediaPage`），拖动时两页要同时在树里。
    final pageBusy = store.mediaPage != store.media.page;
    final grid = Column(
      children: [
        Expanded(
          // 左右滑动 = 翻当前页（与预设页同一套交互，见 SwipePageView）
          child: SwipePageView(
            page: store.mediaPage,
            pages: store.media.pages,
            builder: (page) {
              final data = store.mediaPageData(page);
              return data == null ? null : _mediaGrid(store, data);
            },
            onPageChanged: store.setMediaPage,
            onPageNeeded: store.ensureMediaPage,
            // 页码变了但这一页数据 / 选中状态没变时复用旧网格：翻页那一帧不重建
            // 几十张卡片（当前页与预建的相邻页都不重建）。
            pageVersion: (page) {
              final data = store.mediaPageData(page);
              return Object.hash(
                data,
                _selectionMode,
                Object.hashAll(_picked.keys),
                store.mediaFavoritesFlat,
              );
            },
            // 桌面格子可以直接拖去组成集合，横向拖动会被格子先抢走；
            // 关掉翻页手势，翻页只走下面那条显式按钮，避免"想翻页却拖走一张图"。
            swipeEnabled: !_mouseFirst,
          ),
        ),
        PageBar(
          page: store.mediaPage,
          pages: store.media.pages,
          busy: pageBusy,
          showSwipeHint: !_mouseFirst,
          onChanged: store.setMediaPage,
        ),
      ],
    );

    final open = _openFolder != null;
    // ⚠ 两件事**分工**，别混：
    //
    //  · **形态**（停靠 / 弹窗）只由**设置**决定 —— 宽档读
    //    `collectionDetailWide`、窄档读 `collectionDetailNarrow`。
    //    这里**没有"空间不够就回退弹窗"的分支**：选择停靠时即停靠，
    //    即使窗口很窄。
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
                    // （由设置决定是否停靠，不以"窗口够宽"为判据，见上面 [docked]）
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
        // 选择弹窗模式时才使用居中弹窗（虚化背景；拖出时完全让开，见 `_folderLayer`）
        if (open && !docked) _folderLayer(theme),
        // 拖动中给一条"移出集合"的落点提示（两种形态都有；
        // 现在**松手**也算移出，该处仅作兜底）
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
          value: 'tags',
          icon: Icons.sell_outlined,
          label: '编辑标签…',
        ),
        MenuOption(
          value: 'favorite',
          icon: media.favorite ? Icons.star_border : Icons.star,
          label: media.favorite ? '取消收藏' : '收藏',
        ),
        // 手动标记成人内容：AI 未声明的（自行导入的、旧数据）
        // 由手动标记决定 —— 「预览图模糊」只作用于标记过的条目。
        MenuOption(
          value: 'nsfw',
          icon: media.nsfw ? Icons.visibility : Icons.visibility_off_outlined,
          label: media.nsfw ? '取消 NSFW 标记' : '标记为 NSFW',
        ),
        // ⚠ 集合格子必须**两个删除项并列**，且名字里带数量 ——
        // 这是"删一张 / 删整组"的唯一区分点。
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
        // 归类纠错：整组拆开 = 每个成员都标记"已移出"，之后回填不会再把它们收回。
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
      case 'tags':
        final tags = await showTagEditorDialog(
          context,
          initial: media.ownTags,
          title: '编辑产物标签',
          label: '产物标签',
        );
        if (tags == null || !mounted) return;
        await store.api.updateMedia(media.id, tags: tags);
        await store.refreshAll();
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
  /// 刚拆出的图片会在**下次启动**被回填静默收回原组。
  /// 拖动载荷：把一格变成"可拖的"。
  ///
  /// 桌面与触屏**两套手势**，因为翻页方式不同：
  /// * 桌面用**滚轮**翻页 ⇒ 即时左键拖不会跟滚动打架，而且这也正是文件管理器的习惯
  ///   （长按拖在鼠标上不直观，桌面端表现为"不支持拖动"）；
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
      _folderCollection = null;
    });
    // 成员与名字**并行**取：名字只影响标题，读不到也不该让成员列表打不开。
    final membersFuture = _collectionMembers(collection.collectionId);
    final metaFuture = _collectionMeta(collection.collectionId);
    final members = await membersFuture;
    final meta = await metaFuture;
    if (!mounted || _openFolder?.id != collection.id) return;
    setState(() {
      _folderMembers = members ?? const [];
      _folderCollection = meta;
    });
  }

  /// 展开的那一组在后端叫什么（名字 / 算法分组还是手动集合）。
  ///
  /// 读不到就返回 `null`，标题退回中性的「集合」—— 不能因此把面板关上：
  /// 进入该页面的目的是查看图片，名称仅作标题。
  Future<MediaCollection?> _collectionMeta(int? collectionId) async {
    if (collectionId == null) return null;
    try {
      final rows = await context.read<LibraryStore>().api.listCollections();
      for (final row in rows) {
        if (row.id == collectionId) return row;
      }
    } catch (_) {
      // 名字读不到不影响看图（标题会显示「集合」）
    }
    return null;
  }

  void _closeFolder() {
    if (_openFolder == null) return;
    setState(() {
      _openFolder = null;
      _folderMembers = null;
      _folderCollection = null;
      _folderSelection = false;
      _folderPicked.clear();
      _folderDragging = false;
      _folderRect = null;
      // 换一个新的 key：下一组要用一份干净的矩形
      _folderKey = GlobalKey();
    });
  }

  /// 展开面板的标题：**集合自己的名字**，不再一律写"这一组"。
  ///
  /// * 有名称（手动填写 / 新建时自动生成）= 使用名称；
  /// * 算法分组无名称 = 「算法分组」（如实显示，不显示为手动命名）；
  /// * 老手动集合没名字 = 「未命名集合」，点旁边的铅笔就能补一个。
  String _folderTitleText() {
    final meta = _folderCollection;
    final title = meta?.title?.trim();
    if (title != null && title.isNotEmpty) return title;
    if (meta == null) return '集合';
    return meta.isManual ? '未命名集合' : '算法分组';
  }

  /// 给展开的这一组改名（面板标题旁的铅笔）。
  ///
  /// 改名之后标题**当场**换成新名字；后端还会把算法分组转成手动 ——
  /// 所以弹窗里对自动分组多解释一句"以后重新整理不会动它"。
  Future<void> _renameFolder() async {
    final folder = _openFolder;
    final collectionId = folder?.collectionId;
    if (folder == null || collectionId == null) return;

    final meta = _folderCollection;
    final name = await showDialog<String>(
      context: context,
      builder: (_) => _RenameCollectionDialog(
        initial: (meta?.title ?? '').trim(),
        explainManual: meta != null && !meta.isManual,
      ),
    );
    if (name == null || !mounted) return;
    if (name.isEmpty) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('集合名不能为空')));
      return;
    }
    try {
      final updated = await context.read<LibraryStore>().api.renameCollection(
        collectionId,
        name,
      );
      if (!mounted) return;
      setState(() => _folderCollection = updated);
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('改名失败，$e')));
    }
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

  /// 展开的文件夹：**居中弹窗 + 虚化背景**。
  ///
  /// # 为什么它是页面里的一层，而不是 `showDialog`
  ///
  /// 拖动逻辑：将内容拖出弹窗后弹窗关闭，再移动到目标元素上，松手完成移动。
  /// **拖动过程中弹窗必须让开**。`showDialog` 有两个限制：
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

  /// 集合面板本体：**停靠或居中弹窗由设置决定**
  /// （`SettingsStore.collectionDetailWide` / `collectionDetailNarrow`，见 [_libraryBody]）。
  /// [docked] 只影响它自己的外观（圆角 / 阴影 / 尺寸），不参与"谁决定形态"。
  ///
  /// ⚠ **整个面板本身是一个落点**：只把成员格子做成落点的话，"把外面的一张加进这一组"
  /// 就没有可放置的位置。
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
        // 手机（窄屏）上**限制弹窗大小**：
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
                '${_folderTitleText()}（${members?.length ?? folder.collectionSize} 张）',
                key: const Key('collection-panel-title'),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
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
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    tooltip: '重命名集合',
                    icon: const Icon(Icons.edit_outlined, size: 18),
                    onPressed: _renameFolder,
                  ),
                  IconButton(
                    tooltip: '关闭',
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: _closeFolder,
                  ),
                ],
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: members == null
                  ? const Center(child: CircularProgressIndicator())
                  : members.isEmpty
                  ? Center(
                      child: Text('这里还没有产物', style: theme.textTheme.bodySmall),
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
            // 集合内**多选**
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
  /// 结果是窗口中间的格子**始终无法接收落点**。
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
  /// （在集合内拖动内容且终点位于集合弹窗内时，集合弹窗会关闭）。
  void _finishFolderDrag() {
    final left = _folderDragging;
    _endFolderDrag();
    if (left) _closeFolder();
  }

  /// 松手在**所有落点之外**。
  ///
  /// 将内容拖出集合弹窗后，**在弹窗外任意位置松手**都算移出集合，
  /// 不需要拖到底部横条（该横条仅作提示，也可作为落点）。
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
          // 收藏标记：集合内的成员被收藏时也要**可见**，
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
  /// 三种情况的语义：
  /// 1. 拖的是**集合格子** → **整组一起过去**（这是"动整组"的落点）；
  /// 2. 目标已经在某个集合里 → 并进去；
  /// 3. 目标还没有集合 → **当场新建一个**（目标 + 拖过来的）—— 这就是手机里
  ///    "把两个图标拖到一起就成了一个文件夹"。
  ///
  /// kind 不一致时**前端先拦截**并提示：后端同样会拒绝（400），
  /// 但需要等待一次往返才显示错误，因此当场提示。
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
  /// 桌面端拖动一个集合格子只会移动封面那一张。
  Future<List<int>?> _expandOne(int mediaId) async {
    // ① 弹窗里勾了一批 → 拖其中任意一张 = **整批一起走**。
    // ⚠ 这里必须看 `_folderPicked`：弹窗里的多选与画廊主界面那个 `_picked`
    // **是两套**，只看后者的话"集合内多选之后拖一张"只会搬走那一张
    // （同时需要支持多选拖拽）。
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
  /// 规则需在操作前可见（手动集合不得混放图片和视频）。
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
    // 名字留空时后端会给默认名（`集合 1`…），提示里用后端回来的那个，
    // 别在这里自己复制一份命名规则。
    var createdLabel = name;
    await _run(
      () async {
        final created = await store.api.createCollection(ids, title: name);
        createdLabel = created.label;
      },
      '已新建集合',
      toastOf: () => '已新建集合「$createdLabel」并移入 ${ids.length} 张',
    );
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
  /// 将库中现有图片重新自动整理。
  ///
  /// 该操作会**解散现有自动分组**后重新计算，因此需要确认。
  /// 确认框需说明"哪些会被修改、哪些不会"：
  /// 手动创建 / 修改过的集合与已移出的产物不会被改动。
  Future<void> _regroup() async {
    final ok = await confirmDialog(
      context,
      title: '重新自动整理？',
      message:
          '按提示词把算法分出来的分组重算一遍。\n\n'
          '你手动新建或调整过的集合、以及你手动移出过的产物，都不会被改动。',
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
  /// ⚠ **成功不提示**：移动 / 移出 / 删除完成后，
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
        // 修改归属后展开的文件夹已过期 —— 收起它，显示刷新后的画廊
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
/// 收起后**必须**能判断当前处于筛选状态，
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

  /// 这一条**窄屏放不下五个带文字的按钮**（2026-11-09 真机：360dp 上
  /// 「移到集合…」被裁掉，右边还挂着一条溢出条纹）。判据与 `PageBar` 同一个数：
  /// 够宽给文字，不够宽退回图标 + tooltip —— 不是"缩字号"，那样中文会挤成两行。
  static const double _kTextMinWidth = 520;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHigh,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final withText = constraints.maxWidth >= _kTextMinWidth;
          Widget action({
            required IconData icon,
            required String label,
            required Future<void> Function() onPressed,
            Color? color,
          }) {
            if (withText) {
              return TextButton.icon(
                onPressed: onPressed,
                icon: Icon(icon),
                label: Text(label),
                style: TextButton.styleFrom(foregroundColor: color),
              );
            }
            // 窄屏只要图标：`Tooltip` 保住可读性，语义上也仍然有名字。
            // ⚠ 还必须把按钮自己的最小宽度压下来：M3 的 `TextButton` 默认
            // `minimumSize: Size(64, 40)`，光去掉文字仍是 64dp 一个 —— 五个就是 320dp，
            // 加计数与内边距在 360dp 上照样溢出（2026-11-09 真机 + 用例同时抓到）。
            return Tooltip(
              message: label,
              child: TextButton(
                onPressed: onPressed,
                style: TextButton.styleFrom(
                  foregroundColor: color,
                  minimumSize: const Size(44, 40),
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                ),
                child: Icon(icon),
              ),
            );
          }

          return Row(
            children: [
              Text('$count 张', style: theme.textTheme.titleSmall),
              const Spacer(),
              action(
                icon: Icons.link,
                label: '关联提示词',
                onPressed: onLink,
              ),
              action(
                icon: Icons.star_border,
                label: '收藏',
                onPressed: onFavorite,
              ),
              // 归类纠错（2026-10-18）：可以整组拖出去，也可以整批塞进另一个集合
              action(
                icon: Icons.drive_file_move_outline,
                label: '移到集合…',
                onPressed: onMove,
              ),
              action(
                icon: Icons.call_split,
                label: '移出集合',
                onPressed: onDetach,
              ),
              action(
                icon: Icons.delete_outline,
                label: '删除',
                onPressed: onDelete,
                color: theme.colorScheme.error,
              ),
            ],
          );
        },
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
/// * 否则看 [title] = **新建**一个集合（名字可以是空的 —— 后端会给它一个默认名）。
class CollectionTarget {
  final MediaCollection? collection;
  final String? title;

  const CollectionTarget.existing(MediaCollection this.collection)
    : title = null;
  const CollectionTarget.create(this.title) : collection = null;
}

/// 选一个目标集合，或者当场新建一个。
///
/// `mediaKind`：一个集合只装一种（不得混放图片和视频），
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

  /// 集合封面的小预览。没有封面 id 时退回原来的文件夹图标。
  ///
  /// 一排「未命名集合（3 张）」外观完全相同，没有预览图时无法选中。
  Widget _collectionCover(
    ThemeData theme,
    MediaCollection collection, {
    required bool fits,
  }) {
    final id = collection.coverMediaId;
    if (id == null) {
      return Icon(
        collection.isManual ? Icons.folder_outlined : Icons.auto_awesome_motion,
        size: 22,
        color: fits ? null : theme.disabledColor,
      );
    }
    final kind = collection.kind.toUpperCase();
    if (kind == 'AUDIO') {
      return _coverPlaceholder(theme, kind, dimmed: !fits);
    }
    final api = context.read<LibraryStore>().api;
    // 图片走 /thumb；视频走 /poster（视频的 /thumb 是 204）。
    final url = kind == 'VIDEO'
        ? api.mediaPosterUrl(id)
        : api.absolute('/api/media/$id/thumb');
    final dpr = MediaQuery.devicePixelRatioOf(context);
    return Opacity(
      opacity: fits ? 1 : 0.45,
      child: NsfwBlur(
        nsfw: collection.coverNsfw,
        revealKey: 'media:$id',
        borderRadius: BorderRadius.circular(8),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(8),
          child: CachedThumbImage(
            mediaId: id,
            url: url,
            api: api,
            cacheWidth: (48 * dpr).round(),
            theme: theme,
            loading: () => _coverPlaceholder(theme, kind),
            failed: () => _coverPlaceholder(theme, kind),
          ),
        ),
      ),
    );
  }

  Widget _coverPlaceholder(
    ThemeData theme,
    String kind, {
    bool dimmed = false,
  }) {
    return Container(
      color: theme.colorScheme.surfaceContainerHighest,
      alignment: Alignment.center,
      child: Icon(
        kind == 'VIDEO'
            ? Icons.movie_outlined
            : kind == 'AUDIO'
            ? Icons.graphic_eq
            : Icons.image_outlined,
        size: 20,
        color: dimmed ? theme.disabledColor : theme.colorScheme.outline,
      ),
    );
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
                      minLeadingWidth: 52,
                      leading: SizedBox(
                        width: 48,
                        height: 48,
                        child: _collectionCover(theme, collection, fits: fits),
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
                      labelText: '新建集合的名字（不填就自动起名）',
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

/// 「重命名集合」的小弹窗。
///
/// 单独一个 StatefulWidget 是为了**自己管 `TextEditingController`**：在页面方法里
/// new 一个、`showDialog` 一返回就 `dispose()`，退场动画里 TextField 还会用到它，
/// 报 "A TextEditingController was used after being disposed"。交给 State 管，
/// 弹窗真正拆掉时才释放。
class _RenameCollectionDialog extends StatefulWidget {
  final String initial;

  /// 算法分组的补充说明：命名后归为手动分组，重新整理不会解散。
  final bool explainManual;

  const _RenameCollectionDialog({
    required this.initial,
    required this.explainManual,
  });

  @override
  State<_RenameCollectionDialog> createState() =>
      _RenameCollectionDialogState();
}

class _RenameCollectionDialogState extends State<_RenameCollectionDialog> {
  late final TextEditingController _controller = TextEditingController(
    text: widget.initial,
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _submit() => Navigator.of(context).pop(_controller.text.trim());

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: const Text('重命名集合'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _controller,
            autofocus: true,
            decoration: const InputDecoration(labelText: '集合名'),
            onSubmitted: (_) => _submit(),
          ),
          if (widget.explainManual) ...[
            const SizedBox(height: 10),
            Text(
              '改了名字之后这一组归你手动管理，重新自动整理不会动它。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('保存')),
      ],
    );
  }
}
