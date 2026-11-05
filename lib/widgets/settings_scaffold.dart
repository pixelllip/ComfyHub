import 'package:flutter/material.dart';

import 'adaptive_layout.dart';

/// 设置类页面的统一骨架：顶栏 + 可滚动内容。
///
/// 2026-11-05 起设置首页只放"入口"，具体配置都进了子页 —— 子页与首页共用
/// 这一套骨架，标题层级、左右留白、分列规则才不会各写各的。
///
/// **宽窗口仍然多列**（用户 2026-11-05 明确要求）：分列交给
/// [AdaptiveColumns]（列数 = 可用宽度 / 550，最多 4 列），窄窗口自动退回单列。
/// 所以页面内容要按"块"（[SettingsSection]）组织，块内是标题 + 一张卡；
/// 分列时整块一起走，不会出现"标题在左列、卡片在右列"。
class SettingsScaffold extends StatelessWidget {
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
  Widget build(BuildContext context) {
    return Scaffold(
      // **标题不放 AppBar**（2026-11-05 用户对照 Material You 设置页）：
      // 参考设计是"左上角一个圆形返回键 + 正文顶部一个大号标题"。
      // 好处是标题属于内容、能随内容一起滚动，也不会在窄屏被 AppBar 压成小字。
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 48),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SettingsPageHeader(
                title: title,
                showBack: showBack,
                padding: EdgeInsets.zero,
              ),
              // 设置卡比列表行窄，550 的标准列宽会让"其实放得下两列"的窗口退回
              // 单列（实测 1140 逻辑宽时 content≈1030 → 除以 550 只剩 1 列）。
              // 460 是"标题 + 一句说明 + 箭头"这一行的舒适宽度，窗口一到 ~1000
              // 逻辑宽就能并排两列。
              AdaptiveColumns(
                minColumnWidth: 460,
                sections: [
                  for (final section in sections)
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
    );
  }
}

/// 设置类页面的**大标题头**：圆形返回键 + 正文顶部的大号标题（不是 AppBar）。
///
/// 2026-11-05 用户对照 Material You 设置页后定的形态；AI 模型与凭据、AI 工具权限
/// 这两个不是 `SettingsScaffold` 的页面也复用它，保证"同一套页面标题"。
class SettingsPageHeader extends StatelessWidget {
  final String title;

  /// 是否显示返回键（首页 / `HomeShell` 内容区里的页面传 `false`）。
  final bool showBack;

  /// 自定义返回动作（默认 `Navigator.maybePop`）。
  final VoidCallback? onBack;

  /// 标题行右侧的动作（例如「重新加载」）。
  final List<Widget> actions;

  final EdgeInsetsGeometry padding;

  const SettingsPageHeader({
    super.key,
    required this.title,
    this.showBack = true,
    this.onBack,
    this.actions = const [],
    this.padding = const EdgeInsets.fromLTRB(16, 8, 16, 0),
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final hasRow = showBack || actions.isNotEmpty;
    return Padding(
      padding: padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (hasRow)
            Row(
              children: [
                if (showBack)
                  BackButton(
                    onPressed: onBack,
                    style: IconButton.styleFrom(
                      backgroundColor: theme.colorScheme.surfaceContainerHigh,
                      foregroundColor: theme.colorScheme.onSurface,
                      minimumSize: const Size(44, 44),
                    ),
                  ),
                const Spacer(),
                ...actions,
              ],
            ),
          Padding(
            padding: EdgeInsets.only(
              left: 4,
              top: hasRow ? 12 : 8,
              bottom: 20,
            ),
            child: Text(
              title,
              style: theme.textTheme.displaySmall?.copyWith(
                fontSize: 34,
                height: 1.15,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
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
