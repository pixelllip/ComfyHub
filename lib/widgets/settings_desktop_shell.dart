import 'package:material_ui/material_ui.dart';

import '../core/theme.dart';
import 'settings_scaffold.dart';

/// 桌面设置左侧分类栏的宽度（逻辑像素）。
///
/// 只放**大项主标题**，不放副标题；264 足够容纳最长的「ComfyUI 自动捕获」，
/// 又不至于把右侧正文挤得太窄。
const double kSettingsNavWidth = 264;

/// 桌面设置左侧的一行：图标 + 主标题，点开后右侧换成这一项的主体界面。
class SettingsNavItem {
  /// 稳定标识。用于 `IndexedStack` 的位置、用例定位与状态记忆。
  final String id;

  /// 主标题。**只有标题**，不要再加一句副标题。
  final String label;

  final IconData icon;

  /// 右侧正文的根页面。次级页面仍由它内部的 `Navigator.push` 负责。
  final WidgetBuilder builder;

  const SettingsNavItem({
    required this.id,
    required this.label,
    required this.icon,
    required this.builder,
  });
}

/// **桌面设置的分栏骨架**（2026-11-14）：
/// 左边是只有大项主标题的分类栏，右边是所选设置项的主体界面。
///
/// 右侧不是"一个页面"而是一层**独立的 `Navigator`**：设置子页里的
/// `Navigator.of(context).push(...)` 因此落在这一格里，次级菜单（Provider 详情、
/// 逐工具权限…）只替换右边，左侧分类栏一个字都不动。
///
/// 三层底色由 [AppSurfaces] 统一：
///   * 分类栏 = `sidebar`（比正文高一档）；
///   * 正文 = `content`；
///   * 正文里的卡片仍用主题的 `surfaceContainerHigh`。
///
/// 每一栏各有一个 `Navigator` 且只在第一次点开时创建，切回来时表单、滚动位置都还在
/// （与 `HomeShell` 的五页常驻同一个道理）。切分类**不会**清掉另一个分类里的
/// 半截表单，也不会重复拉一次数据。
class SettingsDesktopShell extends StatefulWidget {
  final List<SettingsNavItem> items;

  /// 默认打开哪一项（`items` 里的 id）。
  final String initialId;

  /// 搜索入口要打开的页面。**由这一层 push 到当前分类的 Navigator 里**，
  /// 所以搜索页与它的结果只替换右侧，左侧分类栏不动。
  final WidgetBuilder searchPageBuilder;

  /// 整页是否需要一个"退回上一页"的入口。
  ///
  /// 从设置搜索点进「NSFW 内容」时，这一页是叠在搜索结果之上的独立路由；
  /// 分栏骨架自己没有 AppBar，必须在这里补一个，否则桌面上只能靠系统返回。
  final bool showBack;

  final VoidCallback? onBack;

  const SettingsDesktopShell({
    super.key,
    required this.items,
    required this.initialId,
    required this.searchPageBuilder,
    this.showBack = false,
    this.onBack,
  });

  @override
  State<SettingsDesktopShell> createState() => _SettingsDesktopShellState();
}

class _SettingsDesktopShellState extends State<SettingsDesktopShell> {
  late String _selectedId;

  /// 已经点开过的分类。没点过的不建，避免一进设置就把八个子页的请求全打一遍。
  late final Set<String> _visited;

  /// 每个分类各自的 Navigator key。搜索页要 push 进**当前这一格**，
  /// 所以这里不能用每次 build 新建的 `ValueKey`。
  final Map<String, GlobalKey<NavigatorState>> _paneKeys = {};

  GlobalKey<NavigatorState> _paneKeyFor(String id) => _paneKeys.putIfAbsent(
    id,
    () => GlobalKey<NavigatorState>(debugLabel: 'settings-pane-$id'),
  );

  @override
  void initState() {
    super.initState();
    _selectedId = _initialId();
    _visited = {_selectedId};
  }

  String _initialId() {
    if (widget.items.any((item) => item.id == widget.initialId)) {
      return widget.initialId;
    }
    return widget.items.isEmpty ? '' : widget.items.first.id;
  }

  int get _selectedIndex {
    final index = widget.items.indexWhere((item) => item.id == _selectedId);
    return index < 0 ? 0 : index;
  }

  @override
  void didUpdateWidget(covariant SettingsDesktopShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    // 搜索结果重新打开设置首页时可能改默认项；分类被平台差异去掉时也要回退。
    // 这里只改字段，不 `setState`：`didUpdateWidget` 之后本来就会重建。
    if (widget.initialId != oldWidget.initialId) {
      final next = _initialId();
      if (next != _selectedId) {
        _selectedId = next;
        _visited.add(next);
      }
    } else if (!widget.items.any((item) => item.id == _selectedId)) {
      final fallback = widget.items.isEmpty ? '' : widget.items.first.id;
      _selectedId = fallback;
      _visited.add(fallback);
    }
  }

  void _select(String id) {
    if (id == _selectedId) return;
    setState(() {
      _selectedId = id;
      _visited.add(id);
    });
  }

  /// 搜索页也落在右侧这一格：左侧分类栏保持可见，搜索结果点进去的页面
  /// 同样由这一格的 Navigator 承接。
  void _openSearch() {
    final navigator = _paneKeys[_selectedId]?.currentState;
    if (navigator == null) return;
    navigator.push(
      MaterialPageRoute<void>(
        builder: (routeContext) => widget.searchPageBuilder(routeContext),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;

    return Scaffold(
      key: const Key('settings-desktop-shell'),
      body: LayoutBuilder(
        builder: (context, constraints) {
          // 右侧正文按**自己这一格的宽度**排版。
          //
          // 设置子页里还有"宽屏分两栏 / 窄屏分层"的判断（例如 Provider 列表 +
          // 详情）；不覆盖的话，1200 宽的窗口扣掉分类栏只剩 900 出头，子页却仍
          // 以为自己是宽屏，详情会被挤成一条缝。覆盖后子页的断点量的是它能用的宽度。
          final paneWidth = constraints.maxWidth - kSettingsNavWidth;
          final paneMedia = MediaQuery.of(context).copyWith(
            size: Size(
              paneWidth < 0 ? 0 : paneWidth,
              constraints.maxHeight.isFinite ? constraints.maxHeight : 0,
            ),
          );

          return Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: kSettingsNavWidth,
                child: Material(
                  key: const Key('settings-nav'),
                  color: AppSurfaces.sidebar(scheme),
                  // 分类栏在标题更长的语言 / 更矮的窗口里自己滚，不把正文顶出去。
                  child: SafeArea(
                    right: false,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        _header(theme),
                        _searchEntry(theme),
                        const SizedBox(height: 6),
                        Expanded(
                          child: ListView.builder(
                            padding: const EdgeInsets.fromLTRB(10, 0, 10, 16),
                            itemCount: widget.items.length,
                            itemBuilder: (context, index) {
                              final item = widget.items[index];
                              return _navRow(
                                theme,
                                item,
                                selected: item.id == _selectedId,
                              );
                            },
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
              Expanded(
                child: ColoredBox(
                  key: const Key('settings-pane'),
                  color: AppSurfaces.content(scheme),
                  child: MediaQuery(
                    data: paneMedia,
                    child: IndexedStack(
                      index: _selectedIndex,
                      sizing: StackFit.expand,
                      children: [
                        for (final item in widget.items)
                          if (_visited.contains(item.id))
                            SettingsPaneScope(
                              child: Navigator(
                                // 每个分类各一层路由栈：切回来时详情页还在原处；
                                // 切分类时旧栈原样保留，不重建、也不串页。
                                key: _paneKeyFor(item.id),
                                onGenerateRoute: (settings) =>
                                    MaterialPageRoute<void>(
                                      settings: settings,
                                      builder: (routeContext) =>
                                          item.builder(routeContext),
                                    ),
                              ),
                            )
                          else
                            const SizedBox.shrink(),
                      ],
                    ),
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Widget _header(ThemeData theme) {
    final scheme = theme.colorScheme;
    return Padding(
      padding: EdgeInsets.fromLTRB(widget.showBack ? 12 : 20, 12, 16, 10),
      child: Row(
        children: [
          if (widget.showBack) ...[
            BackButton(
              onPressed: widget.onBack,
              style: IconButton.styleFrom(
                backgroundColor: scheme.surfaceContainerHighest,
                foregroundColor: scheme.onSurface,
                minimumSize: const Size(40, 40),
                maximumSize: const Size(40, 40),
                padding: EdgeInsets.zero,
                shape: const CircleBorder(),
              ),
            ),
            const SizedBox(width: 8),
          ],
          Expanded(
            child: Text(
              '设置',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              // 与右侧各分类的 AppBar 标题**同一套字号 / 字重**：
              // AppBar 的标题样式已经是全局唯一真源，这里直接复用它。
              style:
                  theme.appBarTheme.titleTextStyle ??
                  theme.textTheme.titleLarge,
            ),
          ),
        ],
      ),
    );
  }

  /// 搜索入口做得像一条输入框，而不是第 9 个设置分类：
  /// 它不属于"设置大项"，点开是一个独立的结果页。
  Widget _searchEntry(ThemeData theme) {
    final scheme = theme.colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
      child: Material(
        color: scheme.surfaceContainerHighest,
        shape: const StadiumBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: _openSearch,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
            child: Row(
              children: [
                Icon(Icons.search, size: 19, color: scheme.onSurfaceVariant),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    '搜索设置',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  /// 一行分类：**图标 + 主标题**，没有副标题。
  Widget _navRow(
    ThemeData theme,
    SettingsNavItem item, {
    required bool selected,
  }) {
    final scheme = theme.colorScheme;
    final foreground = selected
        ? scheme.onSecondaryContainer
        : scheme.onSurface;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: ListTile(
        key: ValueKey('settings-nav-${item.id}'),
        selected: selected,
        // 44 是 Material 导航项的高度：比右侧 56 的 AppBar 低一档，
        // 分类栏看起来是"导航"，不是一排和页头抢体量的大按钮。
        dense: true,
        minTileHeight: 44,
        selectedTileColor: scheme.secondaryContainer,
        selectedColor: scheme.onSecondaryContainer,
        iconColor: selected
            ? scheme.onSecondaryContainer
            : scheme.onSurfaceVariant,
        shape: const StadiumBorder(),
        contentPadding: const EdgeInsets.symmetric(horizontal: 16),
        leading: Icon(item.icon, size: 20),
        title: Text(
          item.label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.labelLarge?.copyWith(
            color: foreground,
            fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
          ),
        ),
        onTap: () => _select(item.id),
      ),
    );
  }
}
