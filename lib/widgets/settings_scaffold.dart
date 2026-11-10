import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'scroll_linked_title.dart';

/// **设置正文的宽度上限**（逻辑像素）。
///
/// 2026-11-09 起设置家族**不再分栏**：多列时高度差 2–4 倍的块并排，底边永远
/// 参差（就算按估算高度切得再准也一样），而设置是"读 + 改"的表单，不是画廊。
/// 所以正文统一单列；宽屏多出来的空间交给**行内的控件与首页的等高磁贴**
/// （[SettingsTileGrid]），而不是再放第二列设置。
///
/// 超宽屏的规则是"**停在 1680、居中**"，不是继续铺满：
///   · 1495 逻辑宽时正文即为全宽（1375），与此前一致；
///   · 1920 时 1680，两侧各 76；再宽（2560 / 3440）也只到 1680，多出来的留白。
/// 理由：设置行的可读宽度由"标签 ↔ 右端控件"的距离决定，超过 ~1700 就开始
/// 来回找；而"再宽一列"正是这次要消掉的高度参差。**别再往上抬**，也别在
/// 超宽屏加第二列设置 —— 那两件事都会把问题带回来。
const double kSettingsContentMaxWidth = 1680;

/// 设置类页面的统一骨架：顶栏 + 可滚动内容。
///
/// 2026-11-05 起设置首页只放"入口"，具体配置都进了子页 —— 子页与首页共用
/// 这一套骨架，标题层级、左右留白、分列规则才不会各写各的。
///
/// **正文单列**（2026-11-09）：块（[SettingsSection]）从上到下依次排，块内是
/// 标题 + 一张卡。不再用 `AdaptiveColumns` 分栏 —— 见 [kSettingsContentMaxWidth]。
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
    return LayoutBuilder(
      builder: (context, constraints) {
        // 首页没有返回键，标题就当成"内容区的页头"：把它挪到正文那一列的左边缘，
        // 免得宽窗口下标题贴在最左边、正文却居中，中间空出一大截。
        // 有返回键的子页保持"返回键 + 标题"贴左（那是系统级的页头，不跟着正文走）。
        final indent = (phone || widget.showBack)
            ? 0.0
            : math.max(
                0.0,
                (constraints.maxWidth - kSettingsContentMaxWidth - 32) / 2,
              );
        return Scaffold(
          appBar: SettingsAppBar(
            title: widget.title,
            showBack: widget.showBack,
            progress: _progress,
            titleIndent: indent,
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
                      child: Center(
                        child: ConstrainedBox(
                          constraints: const BoxConstraints(
                            maxWidth: kSettingsContentMaxWidth,
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: _withSectionGaps([
                              for (final section in widget.sections) section,
                            ]),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// 设置家族的**统一顶栏**（首页 / 子页 / 两个自定义正文的页面共用）。
///
/// * 返回键**始终在 AppBar 里**，带外圈、40×40 的圆形 `BackButton`
///   （返回按钮位于 AppBar）；
/// * 电脑端标题**始终在 AppBar 里**；
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

  /// 标题额外向右缩进（逻辑像素）。
  ///
  /// 只给"没有返回键的首页"用：宽窗口下正文是居中的一列，标题若仍贴在
  /// 最左边就会和正文差出一大截。子页保持 0 —— 返回键与标题是一个整体，
  /// 不跟着正文的列走。
  final double titleIndent;

  const SettingsAppBar({
    super.key,
    required this.title,
    required this.progress,
    this.showBack = true,
    this.onBack,
    this.actions = const [],
    this.titleIndent = 0,
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
      // 返回键在 leading 槽里时，标题从槽后开始；首页没有返回键时与正文的左边缘对齐
      // （正文是一列限宽居中的卡片，`titleIndent` 由 SettingsScaffold 按同一个宽度算）。
      titleSpacing: showBack ? 0 : 16 + titleIndent,
      title: phone
          ? FadingAppBarTitle(title: title, progress: progress)
          : Text(title, maxLines: 1, overflow: TextOverflow.ellipsis),
      actions: actions.isEmpty ? null : [...actions, const SizedBox(width: 4)],
    );
  }
}

/// AppBar 里的返回键：**带外圈的圆形按钮**。
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
/// [estimatedHeight] **已经不参与布局**（正文单列后没有"把下一块放进最矮的
/// 那一列"这件事了）。字段保留是因为不少子页还在传它；将来要恢复分栏时
/// 它仍然是均衡的输入。
class SettingsSection extends StatelessWidget {
  final String title;
  final Widget child;
  final double estimatedHeight;

  /// 放在**等高的并排行**里时置 true：卡片撑满这一行剩下的高度。
  ///
  /// 设置首页的状态区是 4:6 的 NSFW / 库统计两栏，
  /// 两张卡的下边缘必须齐平。平时纵向排布时保持 false —— `Expanded` 在没有
  /// 高度约束的 `Column` 里会直接断言失败。
  final bool fill;

  const SettingsSection({
    super.key,
    required this.title,
    required this.child,
    this.estimatedHeight = 280,
    this.fill = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final header = title.isEmpty
        ? null
        : Padding(
            padding: const EdgeInsets.only(left: 4, bottom: 8),
            child: Text(
              title,
              style: theme.textTheme.titleSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
                fontWeight: FontWeight.w700,
              ),
            ),
          );
    // 首页第一组（入口磁贴）不需要小标题，传空串即可 —— 不渲染空标题行。
    if (header == null && !fill) return child;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [?header, if (fill) Expanded(child: child) else child],
    );
  }
}

/// 相邻两块之间的间距（与旧分栏里的 `runSpacing` 一致）。
const double _sectionGap = 24;

List<Widget> _withSectionGaps(List<Widget> sections) => [
  for (var i = 0; i < sections.length; i++) ...[
    if (i > 0) const SizedBox(height: _sectionGap),
    sections[i],
  ],
];

/// 设置首页（以及子页里的跳转项）的**入口行**：图标 + 标题 + 一句说明 + 箭头。
///
/// 一张卡里超过两项的配置不放在首页，收进子界面；
/// 首页只留这种"能点进去"的行。
///
/// ⚠ **手机 / 窄窗口仍然使用它**（手机端排版保持不变、
/// 电脑端窄屏恢复为手机排版）。宽屏首页才换成 [SettingsTileGrid]
/// 的等高磁贴 —— 两套并存的判据在 `settings_page.dart`（宽度 ≥
/// `kWideLayoutMinWidth`）。
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
