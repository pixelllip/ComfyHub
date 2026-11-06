import 'package:flutter/material.dart';

import 'adaptive_layout.dart';
import 'scroll_linked_title.dart';

/// 设置类页面的统一骨架：顶栏 + 可滚动内容。
///
/// 2026-11-05 起设置首页只放"入口"，具体配置都进了子页 —— 子页与首页共用
/// 这一套骨架，标题层级、左右留白、分列规则才不会各写各的。
///
/// **宽窗口仍然多列**（用户 2026-11-05 明确要求）：分列交给
/// [AdaptiveColumns]（列数 = 可用宽度 / 550，最多 4 列），窄窗口自动退回单列。
/// 所以页面内容要按"块"（[SettingsSection]）组织，块内是标题 + 一张卡；
/// 分列时整块一起走，不会出现"标题在左列、卡片在右列"。
class SettingsScaffold extends StatefulWidget {
  final String title;
  final List<SettingsSection> sections;

  /// 子页顶栏默认带返回箭头；首页传 `false`（它在 `HomeShell` 的内容区里）。
  final bool showBack;

  const SettingsScaffold({
    super.key,
    required this.title,
    required this.sections,
    this.showBack = true,
  });

  @override
  State<SettingsScaffold> createState() => _SettingsScaffoldState();
}

/// 手机端大标题块的高度：原页头是「上 8 + 34 号字（约 39）+ 下 20」≈ 68。
/// 滚动时按 [ScrollTitleProgress] **收掉高度**，下面的内容才会顶上来
/// （AGENTS §6.3：标题滚走之后页首不许永远空一条）。
const double _pageTitleHeight = 68;

/// 大标题左边距：正文左右各 16，块内标题再缩进 4，所以是 20。
const double _pageTitleLeft = 20;

class _SettingsScaffoldState extends State<SettingsScaffold> {
  /// 大标题 → 顶栏标题的联动进度（见 [ScrollTitleProgress]）。
  final _progress = ScrollTitleProgress();

  @override
  void dispose() {
    _progress.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final phone = useCollapsingPageTitle;
    return Scaffold(
      appBar: SettingsAppBar(
        title: widget.title,
        showBack: widget.showBack,
        progress: _progress,
      ),
      body: SafeArea(
        top: false,
        child: Column(
          children: [
            // 手机端：大标题属于正文，随滚动**上移 + 淡出、高度收掉**，
            // 同一个标题在 AppBar 里按同一个进度淡入（AGENTS §6.3）；
            // 电脑端：标题始终在 AppBar 上，正文里不再重复一个大标题。
            if (phone)
              CollapsingPageTitle(
                title: widget.title,
                progress: _progress,
                height: _pageTitleHeight,
                leftPadding: _pageTitleLeft,
              ),
            Expanded(
              child: ScrollTitleListener(
                progress: _progress,
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(16, 8, 16, 48),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      // 设置卡比列表行窄，550 的标准列宽会让"其实放得下两列"的窗口退回
                      // 单列（实测 1140 逻辑宽时 content≈1030 → 除以 550 只剩 1 列）。
                      // 460 是"标题 + 一句说明 + 箭头"这一行的舒适宽度，窗口一到 ~1000
                      // 逻辑宽就能并排两列。
                      AdaptiveColumns(
                        minColumnWidth: 460,
                        sections: [
                          for (final section in widget.sections)
                            AdaptiveSection(
                              section,
                              estimatedHeight: section.estimatedHeight,
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 设置家族的**统一顶栏**（首页 / 子页 / 两个自定义正文的页面共用）。
///
/// * 返回键**始终在 AppBar 里**，带外圈、40×40 的圆形 `BackButton`
///   （用户 2026-11-05："这种返回按钮应该待在 appbar"）；
/// * 电脑端标题**始终在 AppBar 里**（用户 2026-11-05：电脑端恢复原状）；
/// * 手机端这条 AppBar 是透明的，标题按 [progress] 淡入 —— 大标题在正文里
///   滚走之后，这里才出现同一个名字（AGENTS §6.3）。
class SettingsAppBar extends StatelessWidget implements PreferredSizeWidget {
  final String title;
  final bool showBack;

  /// 自定义返回动作（默认 `Navigator.maybePop`）。
  final VoidCallback? onBack;

  final List<Widget> actions;

  /// 大标题 → 顶栏标题的联动进度（手机端用；电脑端忽略）。
  final ScrollTitleProgress progress;

  const SettingsAppBar({
    super.key,
    required this.title,
    required this.progress,
    this.showBack = true,
    this.onBack,
    this.actions = const [],
  });

  @override
  Size get preferredSize => const Size.fromHeight(kToolbarHeight);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final phone = useCollapsingPageTitle;
    return AppBar(
      backgroundColor: phone ? Colors.transparent : null,
      surfaceTintColor: phone ? Colors.transparent : null,
      elevation: phone ? 0 : null,
      scrolledUnderElevation: phone ? 0 : null,
      automaticallyImplyLeading: false,
      leading: showBack ? _circleBackButton(theme, onPressed: onBack) : null,
      // 返回键在 leading 槽里时，标题从槽后开始；首页没有返回键时与正文的 16 对齐。
      titleSpacing: showBack ? 0 : 16,
      title: phone
          ? FadingAppBarTitle(title: title, progress: progress)
          : Text(title, maxLines: 1, overflow: TextOverflow.ellipsis),
      actions: actions.isEmpty ? null : [...actions, const SizedBox(width: 4)],
    );
  }
}

/// AppBar 里的返回键：**带外圈的圆形按钮**（用户 2026-11-05 指定的样式）。
///
/// 用 `BackButton` 而不是裸 `IconButton`：`tester.pageBack()` 与系统返回都
/// 认这个类型；外圈靠 40×40 的按钮背景画，不会撑满 56 的 leading 槽。
Widget _circleBackButton(ThemeData theme, {VoidCallback? onPressed}) => Center(
      child: BackButton(
        onPressed: onPressed,
        style: IconButton.styleFrom(
          backgroundColor: theme.colorScheme.surfaceContainerHigh,
          foregroundColor: theme.colorScheme.onSurface,
          minimumSize: const Size(40, 40),
          maximumSize: const Size(40, 40),
          padding: EdgeInsets.zero,
          shape: const CircleBorder(),
        ),
      ),
    );

/// **正文自己排版**的设置页骨架（AI 模型与凭据 / AI 工具权限 / 逐工具权限）。
///
/// 这两类页面的正文是列表、双栏或子面板，塞不进 [SettingsScaffold] 的
/// [SettingsSection] 结构；但顶栏、返回键与大标题联动必须和其它设置子页**一模一样**
/// —— 以前它们各自 `Scaffold(body: Column[SettingsPageHeader…])`，手机上既没有
/// AppBar（页头顶进状态栏），大标题也不会随滚动淡出。
///
/// [child] 自己负责滚动（页内 `ListView` / `CustomScrollView` 都行），
/// 这里只把它的**最外层纵向滚动**换算成标题进度。
class SettingsPageScaffold extends StatefulWidget {
  final String title;

  /// 正文。它自己是一块能撑满剩余高度的内容（列表 / 分栏 / 面板）。
  final Widget child;

  final bool showBack;

  /// 自定义返回动作（例如详情面板退回列表，而不是弹出整个路由）。
  final VoidCallback? onBack;

  final List<Widget> actions;

  const SettingsPageScaffold({
    super.key,
    required this.title,
    required this.child,
    this.showBack = true,
    this.onBack,
    this.actions = const [],
  });

  @override
  State<SettingsPageScaffold> createState() => _SettingsPageScaffoldState();
}

class _SettingsPageScaffoldState extends State<SettingsPageScaffold> {
  final _progress = ScrollTitleProgress();

  @override
  void dispose() {
    _progress.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final phone = useCollapsingPageTitle;
    return Scaffold(
      appBar: SettingsAppBar(
        title: widget.title,
        showBack: widget.showBack,
        onBack: widget.onBack,
        actions: widget.actions,
        progress: _progress,
      ),
      body: SafeArea(
        top: false,
        child: Column(
          children: [
            if (phone)
              CollapsingPageTitle(
                title: widget.title,
                progress: _progress,
                height: _pageTitleHeight,
                leftPadding: _pageTitleLeft,
              ),
            // 只有手机端才需要换算进度；电脑端标题常驻 AppBar，不挂监听。
            Expanded(
              child: phone
                  ? ScrollTitleListener(
                      progress: _progress,
                      child: widget.child,
                    )
                  : widget.child,
            ),
          ],
        ),
      ),
    );
  }
}

/// 设置页的一块 = 小标题 + 卡片。
///
/// [estimatedHeight] 只用于分列时"把下一块放进当前最矮的一列"的均衡，
/// 不用精确（估小了最多某列长一点，不会错位）。
class SettingsSection extends StatelessWidget {
  final String title;
  final Widget child;
  final double estimatedHeight;

  const SettingsSection({
    super.key,
    required this.title,
    required this.child,
    this.estimatedHeight = 280,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 8),
          child: Text(
            title,
            style: theme.textTheme.titleSmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        child,
      ],
    );
  }
}

/// 设置首页（以及子页里的跳转项）的**入口行**：图标 + 标题 + 一句说明 + 箭头。
///
/// 用户 2026-11-05 的要求：一张卡里超过两项的配置不要摊在首页，收进子界面；
/// 首页只留这种"能点进去"的行。
class SettingsEntryTile extends StatelessWidget {
  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback? onTap;
  final Widget? trailing;

  const SettingsEntryTile({
    super.key,
    required this.icon,
    required this.title,
    required this.subtitle,
    this.onTap,
    this.trailing,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        leading: Icon(icon, color: theme.colorScheme.primary),
        title: Text(title, style: theme.textTheme.titleMedium),
        subtitle: Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Text(
            subtitle,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        trailing: trailing ?? const Icon(Icons.chevron_right),
        onTap: onTap,
      ),
    );
  }
}
