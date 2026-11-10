import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import '../widgets/paged_swipe.dart';
import '../widgets/scroll_linked_title.dart';
import '../widgets/tag_editor.dart';
import 'cards_page.dart';
import 'layout_canvas_page.dart';
import 'preset_run.dart';
import 'prompt_detail_page.dart';
import 'prompt_edit_page.dart';

/// 预设库顶部的三个**同级页签**。
///
/// 常规预设 / 角色卡 / 画风卡以前是"一个主页面 + 一个卡片库子页"，现在并排；
/// 角色卡与画风卡只是 [CardsView] 的一种 kind，字段 / 编辑器 / 示例管理完全共用。
enum PresetSection {
  prompts('常规预设', Icons.text_snippet_outlined),
  characters('角色卡', Icons.badge_outlined),
  styles('画风卡', Icons.palette_outlined);

  final String label;
  final IconData icon;

  const PresetSection(this.label, this.icon);

  /// 卡片页签对应的种类；常规预设是 null。
  CardKind? get cardKind => switch (this) {
    PresetSection.characters => CardKind.character,
    PresetSection.styles => CardKind.style,
    PresetSection.prompts => null,
  };
}

/// **预设页**（2026-10-11 由「提示词」页改组而来）。
///
/// 定位：预设 = **可复用的一段提示词 + 用途**。这一页只保留三件事：
/// **查看 / 修改提示词 / 用途（生图·生视频·生音频）**，外加把预设送进工作台执行。
///
/// 刻意**不再**在这页做的东西：
///  * 产物反查 —— 从画廊详情进入查看提示词。
///
/// 2026-10-24 起角色卡 / 画风卡**不再是子页**：顶部一条导航把它们和常规预设并排
/// 显示（[PresetSection]），三者的列表区互不干扰、各自保留搜索与滚动位置。
class PromptsPage extends StatefulWidget {
  /// 「▶️运行」的落点：把这段文本交给 AI 工作台并跳过去。
  ///
  /// 由 `HomeShell` 往下传（切页只有它能做，`_index` 是它的状态）——
  /// 与受限横幅 `onOpenSettings` 同一个形状。`null` = 没人接（测试里直接起这一页时），
  /// 那时按钮**不显示**（宁可不给，也不给一个点了没反应的按钮）。
  final void Function(String text)? onSendToWorkspace;

  /// 角色卡 / 画风卡页签用的后端客户端。**仅供测试注入**；
  /// 正式运行时给 null，由 [CardsView] 按设置里的后端地址创建。
  final AiApiClient? cardsApi;

  const PromptsPage({super.key, this.onSendToWorkspace, this.cardsApi});

  @override
  State<PromptsPage> createState() => _PromptsPageState();
}

class _PromptsPageState extends State<PromptsPage>
    with SingleTickerProviderStateMixin {
  final _searchController = TextEditingController();

  /// 顶部三个页签：用 `TabBar` 是为了拿到与主题一致的下划线 / 文字样式；
  /// **不用 `TabBarView`** —— 内容要保留状态（IndexedStack），而且左右滑动
  /// 属于列表分页，不能被"切页签"抢走。
  late final TabController _tabs = TabController(
    length: PresetSection.values.length,
    vsync: this,
  );
  int _sectionIndex = 0;

  /// 访问过的页签才真的建（卡片页签首次点开才拉数据）。
  final Set<int> _visitedSections = {0};

  /// 两种卡各一个 key：宿主页的「新建 / 刷新」要能调到当前页签那份视图。
  final Map<CardKind, GlobalKey<CardsViewState>> _cardViews = {
    for (final kind in CardKind.values) kind: GlobalKey<CardsViewState>(),
  };

  /// 批量管理：多选模式 + 已选中的提示词 id
  final Set<int> _selected = {};
  bool _selectionMode = false;
  bool _busy = false;

  /// 大标题 → AppBar 标题的联动进度（见 [ScrollTitleProgress]）。
  final _titleProgress = ScrollTitleProgress();

  PresetSection get _section => PresetSection.values[_sectionIndex];

  @override
  void dispose() {
    _searchController.dispose();
    _tabs.dispose();
    _titleProgress.dispose();
    super.dispose();
  }

  void _switchSection(int index) {
    if (index == _sectionIndex) return;
    setState(() {
      // 多选是"常规预设"的操作；切到卡片页签还挂着底部批量条只会让人误操作
      _selectionMode = false;
      _selected.clear();
      _sectionIndex = index;
      _visitedSections.add(index);
    });
  }

  void _refreshCurrent() {
    if (_section == PresetSection.prompts) {
      context.read<LibraryStore>().refreshAll();
    } else {
      _cardViews[_section.cardKind]?.currentState?.reload();
    }
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
    final section = _section;
    final fab = _buildFab();
    // 电脑端标题始终在 AppBar；手机端保留滚动联动。
    final phone = useCollapsingPageTitle;

    return Scaffold(
      appBar: AppBar(
        leading: _selectionMode
            ? IconButton(
                tooltip: '退出多选',
                icon: const Icon(Icons.close),
                onPressed: _exitSelection,
              )
            : null,
        // 电脑端标题始终在 AppBar；手机端大标题滚出视野后才淡入。
        title: _selectionMode
            ? Text('已选 ${_selected.length} 项')
            : phone
            ? FadingAppBarTitle(title: '预设库', progress: _titleProgress)
            : const Text('预设库'),
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
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    ),
                  ),
              ]
            : [
                // 布局工具与页签无关：三张页签底下都能进去（它产出的就是提示词）
                IconButton(
                  tooltip: '布局 → 提示词',
                  icon: const Icon(Icons.dashboard_customize_outlined),
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute(builder: (_) => const LayoutCanvasPage()),
                  ),
                ),
                // 批量管理只属于常规预设：卡片页签没有"批量"这个概念
                if (section == PresetSection.prompts)
                  IconButton(
                    tooltip: '批量管理',
                    icon: const Icon(Icons.checklist),
                    onPressed: () => setState(() => _selectionMode = true),
                  ),
                IconButton(
                  tooltip: '刷新',
                  icon: const Icon(Icons.refresh),
                  onPressed: _refreshCurrent,
                ),
                const SizedBox(width: 4),
              ],
      ),
      floatingActionButton: fab == null
          ? null
          : liftFabAbovePageBar(
              fab,
              // 卡片页签没有 PageBar；只有常规预设的翻页条会被 FAB 压住
              lift: section == PresetSection.prompts && store.prompts.pages > 1,
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
          if (!_selectionMode && phone)
            CollapsingPageTitle(title: '预设库', progress: _titleProgress),
          // 详情（列表）**上面**的一条导航：常规预设 / 角色卡 / 画风卡同级
          Material(
            color: Theme.of(context).colorScheme.surface,
            child: TabBar(
              controller: _tabs,
              onTap: _switchSection,
              // 320dp 上三个页签各 ≈106；给 Row 留的横向内边距必须小，
              // 否则"图标 + 两个字"会把标签挤溢出
              labelPadding: const EdgeInsets.symmetric(horizontal: 6),
              tabs: [
                for (final s in PresetSection.values)
                  Tab(
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(s.icon, size: 18),
                        const SizedBox(width: 6),
                        Text(s.label),
                      ],
                    ),
                  ),
              ],
            ),
          ),
          // 三份内容**常驻**（访问过才建）：切回来时搜索词 / 滚动位置都还在
          Expanded(
            child: ScrollTitleListener(
              progress: _titleProgress,
              child: IndexedStack(
                index: _sectionIndex,
                sizing: StackFit.expand,
                children: [
                  _promptsTab(context, store),
                  for (final tab in PresetSection.values.skip(1))
                    _visitedSections.contains(tab.index)
                        ? CardsView(
                            key: _cardViews[tab.cardKind],
                            kind: tab.cardKind!,
                            api: widget.cardsApi,
                          )
                        : const SizedBox.shrink(),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// 新建按钮跟着页签走：常规预设新建提示词，两个卡片页签新建对应种类的卡。
  Widget? _buildFab() {
    if (_selectionMode && _selected.isNotEmpty) return null;
    switch (_section) {
      case PresetSection.prompts:
        return FloatingActionButton.extended(
          onPressed: () async {
            final created = await Navigator.of(context).push<Prompt>(
              MaterialPageRoute(builder: (_) => const PromptEditPage()),
            );
            if (created != null && mounted) {
              context.read<LibraryStore>().refreshAll();
            }
          },
          icon: const Icon(Icons.add),
          label: const Text('新建提示词'),
        );
      case PresetSection.characters:
      case PresetSection.styles:
        return FloatingActionButton.extended(
          onPressed: () =>
              _cardViews[_section.cardKind]?.currentState?.createNew(),
          icon: const Icon(Icons.add),
          label: Text(_section.cardKind!.newLabel),
        );
    }
  }

  /// 常规预设页签的内容（筛选栏 + 列表），与另外两个页签互不影响。
  Widget _promptsTab(BuildContext context, LibraryStore store) {
    return Column(
      children: [
        _buildToolbar(context, store),
        Expanded(child: _buildBody(context, store)),
      ],
    );
  }

  Widget _buildToolbar(BuildContext context, LibraryStore store) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final collapsed = settings.promptFiltersCollapsed;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 搜索栏 + **折叠按钮**（与画廊同一套交互，按钮位于搜索栏右侧）
          Row(
            children: [
              Expanded(
                child: TextField(
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
              ),
              const SizedBox(width: 4),
              IconButton(
                tooltip: collapsed ? '展开筛选' : '收起筛选',
                icon: Icon(collapsed ? Icons.expand_more : Icons.expand_less),
                onPressed: () => settings.setPromptFiltersCollapsed(!collapsed),
              ),
            ],
          ),
          // 收起时显示生效筛选摘要（与画廊一致：否则会误认为"预设数量减少"）
          if (collapsed)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: _FilterSummary(text: _filterSummary(store), theme: theme),
            ),
          if (!collapsed) ...[
            const SizedBox(height: 10),
            _filterSections(store, theme),
          ],
        ],
      ),
    );
  }

  /// 当前生效的筛选项摘要（形如 `生图 · 仅收藏 · 最新创建`）。
  String _filterSummary(LibraryStore store) {
    final parts = <String>[
      switch (store.promptKind) {
        'IMAGE' => '生图',
        'VIDEO' => '生视频',
        'AUDIO' => '生音频',
        'MIXED' => '混合',
        _ => '全部',
      },
      if (store.onlyFavorite) '仅收藏',
      if (store.onlyUnlinked) '未关联产物',
      if (store.selectedTags.isNotEmpty) '标签 ${store.selectedTags.join('/')}',
      if (store.promptQuery.isNotEmpty) '搜索「${store.promptQuery}」',
      _sortLabel(store.promptSort),
    ];
    return parts.join(' · ');
  }

  /// 筛选选项**按功能分区**，但**不"一种一行"**：
  /// 组内联排、组与组之间一条竖线，宽窗口一行塞得下两三组，窄窗口整组换行。
  Widget _filterSections(LibraryStore store, ThemeData theme) {
    return FilterGroupRow(
      groups: [
        FilterGroup(title: '用途', children: _kindFilter(store)),
        FilterGroup(
          title: '状态',
          children: [
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
            // 「未关联产物」：产物被删掉之后提示词会变成未关联，
            // 在此筛出，配合「一键清除未关联」使用
            FilterChip(
              label: const Text('未关联产物'),
              avatar: Icon(
                Icons.link_off,
                size: 16,
                color: store.onlyUnlinked ? Colors.orange : null,
              ),
              selected: store.onlyUnlinked,
              onSelected: (v) => store.setOnlyUnlinked(v),
            ),
          ],
        ),
        if (store.selectedTags.isNotEmpty)
          FilterGroup(
            title: '标签',
            children: [
              for (final t in store.selectedTags)
                TagChip(
                  label: t,
                  selected: true,
                  onDeleted: () => store.toggleTag(t),
                  onTap: () => store.toggleTag(t),
                ),
              SegmentedButton<String>(
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
                segments: const [
                  ButtonSegment(value: 'any', label: Text('任一标签')),
                  ButtonSegment(value: 'all', label: Text('全部标签')),
                ],
                selected: {store.tagMode},
                onSelectionChanged: (s) => store.setTagMode(s.first),
              ),
              TextButton.icon(
                onPressed: store.clearTags,
                icon: const Icon(Icons.clear_all, size: 16),
                label: const Text('清空标签'),
              ),
            ],
          ),
        FilterGroup(
          title: '排序',
          children: [
            AppMenuButton<String>(
              tooltip: '排序',
              onSelected: store.setPromptSort,
              options: const [
                MenuOption(
                  value: 'newest',
                  icon: Icons.schedule,
                  label: '最新创建',
                ),
                MenuOption(value: 'oldest', icon: Icons.history, label: '最旧创建'),
                MenuOption(value: 'updated', icon: Icons.update, label: '最近修改'),
                MenuOption(
                  value: 'title',
                  icon: Icons.sort_by_alpha,
                  label: '按标题',
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
                  label: Text(_sortLabel(store.promptSort)),
                ),
              ),
            ),
            Text(
              '共 ${store.prompts.total} 条',
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
              label: const Text('一键清除未关联'),
              style: TextButton.styleFrom(
                foregroundColor: theme.colorScheme.error,
              ),
            ),
          ],
        ),
      ],
    );
  }

  String _sortLabel(String sort) => switch (sort) {
    'oldest' => '最旧创建',
    'updated' => '最近修改',
    'title' => '按标题',
    'favorite' => '收藏优先',
    _ => '最新创建',
  };

  /// 提示词类型筛选（"全部 / 生图 / 生视频 / 生音频 / 混合"）。
  ///
  /// 返回**列表**而不是 `Row`：外层的筛选行是 `FilterChipRow`（`Wrap`），
  /// 这些 chip 作为独立项参与折行（放不下时换行）。
  List<Widget> _kindFilter(LibraryStore store) {
    const kinds = <(String?, String)>[
      (null, '全部'),
      ('IMAGE', '生图'),
      ('VIDEO', '生视频'),
      ('AUDIO', '生音频'),
      ('MIXED', '混合'),
    ];
    return [
      for (final k in kinds)
        ChoiceChip(
          label: Text(k.$2),
          selected: store.promptKind == k.$1,
          onSelected: (_) => store.setPromptKind(k.$1),
        ),
    ];
  }

  Widget _buildBody(BuildContext context, LibraryStore store) {
    if (store.loadingPrompts && store.prompts.items.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (store.promptsError != null) {
      return ErrorState(
        message: store.promptsError!,
        onRetry: store.refreshPrompts,
      );
    }
    if (store.prompts.items.isEmpty) {
      return EmptyState(
        icon: Icons.saved_search_outlined,
        title: store.hasPromptFilter ? '没有匹配的提示词' : '还没有提示词',
        subtitle: store.hasPromptFilter
            ? '换个关键词或标签试试'
            : '把 ComfyUI 里跑通的提示词存进来，之后按标签就能快速找到',
        action: store.hasPromptFilter
            ? TextButton(
                onPressed: () {
                  _searchController.clear();
                  store.setPromptQuery('');
                  store.clearTags();
                  store.setPromptKind(null);
                  store.setOnlyFavorite(false);
                  setState(() {});
                },
                child: const Text('清除筛选'),
              )
            : null,
      );
    }

    // 页码请求走后端分页：`promptPage` 表示目标页码。
    // 相邻页由 store 预取（`ensurePromptPage`），拖动时两页要同时在树里。
    final pageBusy = store.promptPage != store.prompts.page;
    return Column(
      children: [
        Expanded(
          // 左右滑动 = 翻当前列表的页（两页同时滑动的过渡，见 SwipePageView）
          child: SwipePageView(
            page: store.promptPage,
            pages: store.prompts.pages,
            builder: (page) {
              final data = store.promptPageData(page);
              return data == null ? null : _promptList(context, store, data);
            },
            onPageChanged: store.setPromptPage,
            onPageNeeded: store.ensurePromptPage,
          ),
        ),
        PageBar(
          page: store.promptPage,
          pages: store.prompts.pages,
          busy: pageBusy,
          onChanged: store.setPromptPage,
        ),
      ],
    );
  }

  /// 一页提示词列表。当前页与预取到的相邻页共用 —— 翻页动画要同时画出两页。
  Widget _promptList(
    BuildContext context,
    LibraryStore store,
    Paged<Prompt> data,
  ) => AdaptiveColumnList(
    padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
    runSpacing: 10,
    itemCount: data.items.length,
    itemBuilder: (context, i) {
      final prompt = data.items[i];
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
        // 「▶️运行」：交给宿主（HomeShell）送进工作台并切过去。
        // 宿主没接（比如用例里直接起这一页）时传 null，按钮就不显示。
        onRun: widget.onSendToWorkspace == null
            ? null
            : () => _runPreset(prompt),
      );
    },
  );

  Future<void> _openDetail(BuildContext context, Prompt prompt) async {
    await Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => PromptDetailPage(promptId: prompt.id)),
    );
    if (context.mounted) context.read<LibraryStore>().refreshAll();
  }

  /// 「▶️运行」：弹窗确认目标产物类型 → 把提示词送进工作台并跳过去。
  ///
  /// **不自动发送**：只填入输入框，由使用方按发送 ——
  /// 这样不产生"无人值守的提交"，`comfy_submit` 那道审批闸门一个字都不改。
  Future<void> _runPreset(Prompt prompt) async {
    final send = widget.onSendToWorkspace;
    if (send == null) return;
    final kind = await showPresetRunDialog(context, prompt);
    if (kind == null || !mounted) return;
    // 类型以"预设自身的用途"为准；临时修改时按修改后的执行（写入指令交给模型）
    send(buildPresetRunText(prompt, kind));
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
      '已给 ${ids.length} 条提示词加上 ${picked.join('、')}',
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

  /// 一键清除"未关联产物"的提示词。
  ///
  /// 直接删库，不做"解除关联"——它们本来就没有关联任何东西可解除。
  /// 删除前显示**条数与若干标题**，避免被理解为筛选操作。
  Future<void> _clearUnlinked() async {
    final store = context.read<LibraryStore>();
    final total = await store.countUnlinkedPrompts();
    if (!mounted) return;
    if (total == 0) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('没有未关联产物的提示词')));
      return;
    }
    // 取前几条标题作为示例
    final sample = await store.api.listPrompts(
      hasMedia: false,
      page: 1,
      size: 5,
    );
    if (!mounted) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('清除 $total 条未关联产物的提示词？'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              '这些提示词目前没有关联任何产物（产物被删掉了，或还没关联过）。\n'
              '删除后不可恢复，产物本身不受影响。',
            ),
            const SizedBox(height: 10),
            for (final p in sample.items)
              Text('· ${p.title}', style: Theme.of(ctx).textTheme.bodySmall),
            if (total > sample.items.length)
              Text(
                '…… 还有 ${total - sample.items.length} 条',
                style: Theme.of(ctx).textTheme.labelSmall,
              ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: Theme.of(ctx).colorScheme.error,
            ),
            onPressed: () => Navigator.pop(ctx, true),
            child: Text('删除 $total 条'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    await _run(() async {
      // 真正删除交给 store 循环按批处理（边删边翻页会漏掉一部分）
      final removed = await store.deleteUnlinkedPrompts();
      if (removed != total) {
        debugPrint('未关联提示词期望删除 $total 条，实际删除 $removed 条');
      }
    }, '已清除 $total 条未关联提示词');
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
/// 收起筛选栏时露出的"当前生效的筛选摘要"（与画廊那份同形同义）。
///
/// 收起来之后**必须**能看出自己在筛选状态，否则会以为"预设怎么少了"。
class _FilterSummary extends StatelessWidget {
  final String text;
  final ThemeData theme;

  const _FilterSummary({required this.text, required this.theme});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        // 用 surface 角色代替"叠 45% alpha"：与画廊的筛选摘要保持同一档。
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
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
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
            style: TextButton.styleFrom(
              foregroundColor: theme.colorScheme.error,
            ),
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

  /// 「▶️运行」：null = 不显示这个按钮（宿主没接这条回调时）。
  final VoidCallback? onRun;

  const PromptCard({
    super.key,
    required this.prompt,
    required this.selectedTags,
    required this.onOpen,
    required this.onToggleTag,
    this.onLongPress,
    this.selectionMode = false,
    this.selected = false,
    this.onRun,
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
                      selected
                          ? Icons.check_circle
                          : Icons.radio_button_unchecked,
                      size: 20,
                      color: selected
                          ? theme.colorScheme.primary
                          : theme.colorScheme.outline,
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
                      style: theme.textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  if (prompt.mediaCount > 0)
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Chip(
                        visualDensity: VisualDensity.compact,
                        avatar: const Icon(
                          Icons.photo_library_outlined,
                          size: 14,
                        ),
                        label: Text('${prompt.mediaCount}'),
                      ),
                    )
                  // 产物被删掉（或还没关联上）时给出**一眼能看到**的标记：
                  // 提示词仍然存在，但已不对应任何产物
                  else
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Tooltip(
                        message: '没有关联任何产物，可能是产物被删掉了，或还没关联过',
                        child: Chip(
                          visualDensity: VisualDensity.compact,
                          backgroundColor: Colors.orange.withValues(
                            alpha: 0.12,
                          ),
                          side: BorderSide(
                            color: Colors.orange.withValues(alpha: 0.6),
                          ),
                          avatar: const Icon(
                            Icons.link_off,
                            size: 14,
                            color: Colors.orange,
                          ),
                          label: Text(
                            '未关联',
                            style: theme.textTheme.labelSmall?.copyWith(
                              color: Colors.orange,
                            ),
                          ),
                        ),
                      ),
                    ),
                  // 多选模式下藏掉单条操作，避免"想选中却点了收藏"
                  if (!selectionMode) ...[
                    // 「▶️运行」：这个页面的主操作，所以放成**显式按钮**而不是收进"更多"。
                    // 宿主没接回调时（测试里直接起这一页）不显示。
                    if (onRun != null)
                      Padding(
                        padding: const EdgeInsets.only(right: 4),
                        child: FilledButton.tonalIcon(
                          onPressed: onRun,
                          icon: const Icon(Icons.play_arrow_rounded, size: 18),
                          label: const Text('运行'),
                          style: FilledButton.styleFrom(
                            visualDensity: VisualDensity.compact,
                          ),
                        ),
                      ),
                    IconButton(
                      tooltip: prompt.favorite ? '取消收藏' : '收藏',
                      visualDensity: VisualDensity.compact,
                      icon: Icon(
                        prompt.favorite ? Icons.star : Icons.star_border,
                        color: prompt.favorite
                            ? Colors.amber
                            : theme.colorScheme.outline,
                      ),
                      onPressed: () => store.togglePromptFavorite(prompt),
                    ),
                    AppMenuButton<String>(
                      tooltip: '更多',
                      onSelected: (v) async {
                        final messenger = ScaffoldMessenger.of(context);
                        switch (v) {
                          case 'edit':
                            await Navigator.of(context).push(
                              MaterialPageRoute(
                                builder: (_) => PromptEditPage(prompt: prompt),
                              ),
                            );
                            if (context.mounted) {
                              context.read<LibraryStore>().refreshAll();
                            }
                            break;
                          case 'duplicate':
                            await store.api.duplicatePrompt(prompt.id);
                            await store.refreshAll();
                            messenger.showSnackBar(
                              const SnackBar(content: Text('已复制')),
                            );
                            break;
                          case 'delete':
                            final ok = await confirmDialog(
                              context,
                              title: '删除提示词',
                              message:
                                  '确定删除「${prompt.title}」吗？\n已关联的产物会保留，但会解除关联。',
                            );
                            if (ok) {
                              await store.deletePrompt(prompt.id);
                              messenger.showSnackBar(
                                const SnackBar(content: Text('已删除')),
                              );
                            }
                            break;
                        }
                      },
                      options: const [
                        MenuOption(
                          value: 'edit',
                          icon: Icons.edit,
                          label: '编辑',
                        ),
                        MenuOption(
                          value: 'duplicate',
                          icon: Icons.copy,
                          label: '复制一份',
                        ),
                        MenuOption(
                          value: 'delete',
                          icon: Icons.delete_outline,
                          label: '删除',
                          danger: true,
                          dividerBefore: true,
                        ),
                      ],
                      button: (context, controller, isOpen) => IconButton(
                        tooltip: '更多',
                        visualDensity: VisualDensity.compact,
                        onPressed: () => controller.isOpen
                            ? controller.close()
                            : controller.open(),
                        icon: const Icon(Icons.more_vert, size: 18),
                      ),
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
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.outline,
                        ),
                      ),
                    ),
                  const Spacer(),
                  Text(
                    relativeTime(prompt.updatedAt ?? prompt.createdAt),
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
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
        TextButton(
          onPressed: () => Navigator.pop(ctx, false),
          child: const Text('取消'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(
            backgroundColor: Theme.of(ctx).colorScheme.error,
          ),
          onPressed: () => Navigator.pop(ctx, true),
          child: Text(confirmText),
        ),
      ],
    ),
  );
  return result ?? false;
}
