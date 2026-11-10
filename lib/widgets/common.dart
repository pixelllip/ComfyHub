import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/formatting.dart';
import '../core/theme.dart';
import '../models/models.dart';
import 'app_menu.dart';
import 'context_menu.dart';

// 菜单相关的东西（MenuOption / AppMenuButton / ContextMenuScope）与 common.dart 里
// 的通用组件是一套，统一从这里转出去，调用点不用多写一行 import。
export 'app_menu.dart';
export 'context_menu.dart';

/// 标签小胶囊。可用于展示、筛选、删除。
///
/// **走 M3 的 `FilterChip`**（不是手搓的 `Material + InkWell`）：选中态、对勾、
/// 删除按钮、状态层与命中区都由框架提供，深浅两套主题下也不会再出现
/// "白字压在浅色标签上"（以前 `Colors.white` 写死，浅色主题 + 浅绿标签
/// 对比度只有约 2:1）。
///
/// 标签颜色仍由名称稳定散列决定（颜色可用于识别标签），但只取**色相**：
/// 饱和度 / 亮度跟着当前配色走，所以动态取色下也不会撞色。
class TagChip extends StatelessWidget {
  final String label;
  final int? count;
  final Color? color;
  final bool selected;
  final VoidCallback? onTap;
  final VoidCallback? onDeleted;
  final bool dense;

  const TagChip({
    super.key,
    required this.label,
    this.count,
    this.color,
    this.selected = false,
    this.onTap,
    this.onDeleted,
    this.dense = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final accent = color ?? tagColor(label, scheme: scheme);
    // 选中态用标签色填充：文字颜色按"这块颜色上读不读得清"来选，
    // 不再无条件用白色（浅色标签 + 白字 = 看不见）。
    final selectedFg =
        ThemeData.estimateBrightnessForColor(accent) == Brightness.dark
        ? Colors.white
        : Colors.black87;
    final fg = selected ? selectedFg : scheme.onSurfaceVariant;

    return Padding(
      padding: EdgeInsets.only(right: dense ? 4 : 6, bottom: dense ? 4 : 6),
      child: FilterChip(
        selected: selected,
        // 只有可点的才给 `onSelected`（纯展示标签保持禁用态观感）。
        onSelected: onTap == null ? null : (_) => onTap!(),
        showCheckmark: selected,
        checkmarkColor: selectedFg,
        selectedColor: accent,
        backgroundColor: Colors.transparent,
        side: BorderSide(color: scheme.outlineVariant),
        // 未选中时左侧留一个色点：颜色编码不丢，又不用把整块底色染上。
        avatar: selected || dense
            ? null
            : Container(
                width: 7,
                height: 7,
                decoration: BoxDecoration(
                  color: accent,
                  shape: BoxShape.circle,
                ),
              ),
        label: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              label,
              style: TextStyle(
                color: fg,
                fontSize: dense ? 11.5 : 13,
                fontWeight: selected ? FontWeight.w700 : FontWeight.w600,
                letterSpacing: 0,
              ),
            ),
            if (count != null) ...[
              const SizedBox(width: 4),
              Text(
                '$count',
                style: TextStyle(
                  color: fg.withValues(alpha: 0.78),
                  fontSize: dense ? 11 : 11.5,
                  fontWeight: FontWeight.w500,
                  letterSpacing: 0,
                ),
              ),
            ],
          ],
        ),
        labelPadding: const EdgeInsets.symmetric(horizontal: 2),
        visualDensity: VisualDensity.compact,
        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
        onDeleted: onDeleted,
        deleteIcon: onDeleted == null
            ? null
            : Icon(Icons.close, size: dense ? 13 : 15, color: fg),
        deleteIconColor: fg,
        deleteButtonTooltipMessage: '删除标签',
      ),
    );
  }
}

/// 筛选控件行：**一行放不下时折行，不使用横向滚动**。
///
/// 画廊 / 提示词库 / 标签页顶部那一排「全部 / 图片 / 视频 / … / 仅收藏 / 未关联 /
/// 排序 / 共 N 个」原来是 `SingleChildScrollView(scrollDirection: horizontal)` ——
/// 窗口变窄时右侧控件会被裁掉一半，需要横向拖动才能看到。
/// 该交互**不符合直觉**：一行显示不完的控件用两行，两行不够用三行。
///
/// 所以这里统一走 `Wrap`：放不下就换行，几行都行，**一个控件都不会消失**。
/// 统计文字（"共 N 个"）不要塞进这里 —— 它会被当成换行项，单独放更稳。
class FilterChipRow extends StatelessWidget {
  /// 一组组地传：**每一组内部**用 [spacing] 隔开，组与组之间也会换行。
  ///
  /// 传"组"而不是"拉平的 children"，是因为调用方常需要 `if (…) ...[]`
  /// 这种条件组（`selectedTags.isNotEmpty` 才有排序按钮等等）。
  final List<Widget> children;

  /// 同一行内相邻控件之间的间距（也用于行内最后一个与行尾之间，视觉上没差别）。
  final double spacing;

  /// 行与行之间的间距。
  final double runSpacing;

  /// 整行在横向的摆放：
  ///   · `start`（默认）—— 工具栏，靠左；
  ///   · `end` —— 例如 AI 工作台输入区那一行（token 汇总 + 发送按钮贴右）。
  final WrapAlignment alignment;

  const FilterChipRow({
    super.key,
    required this.children,
    this.spacing = 6,
    this.runSpacing = 6,
    this.alignment = WrapAlignment.start,
  });

  @override
  Widget build(BuildContext context) {
    // `runAlignment` 故意不暴露、也不动：它只在**多行**时决定"行块整体靠哪边"，
    // 默认 start 就是"从容器顶部开始往下排"。曾经想顺手设成 end 让多行贴底，
    // 那会让窄屏输入区里那一排贴在输入框下沿，看着像被压住。
    // 调用方要的"每行贴右"是 `alignment` 的事，跟 `runAlignment` 无关。
    return Wrap(
      spacing: spacing,
      runSpacing: runSpacing,
      alignment: alignment,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: children,
    );
  }
}

/// 筛选栏里的**一个分区**：`小标题 + 内容`，装在一个**浅底圆角容器**里。
///
/// 布局依据：
///  ① 原实现为"**一种一行**"（标题占 34px 固定栏、内容另起），宽窗口下左侧大面积空白、
///     整个工具栏又高 → 改成组内联排；
///  ② 改用**竖分隔线**分组时排版异常，尤其是分隔线；
///     分隔线在 `Wrap` 中是**独立项**，换行时会单独留在行尾；
///     且组间间距与组内间距相同，边界无法区分。
///
/// 所以现在用**容器**表达分组：浅底 + 圆角 + 一圈极淡的边。
/// 它在换行时是**整块**移动的（不会像分隔线那样断在行尾），
/// 组内元素贴着标题、组与组之间是容器外的那道间距 —— 一眼分得清。
class FilterGroup extends StatelessWidget {
  final String title;
  final List<Widget> children;

  /// 组内元素间距。
  final double spacing;

  const FilterGroup({
    super.key,
    required this.title,
    required this.children,
    this.spacing = 6,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 6, 10, 6),
      decoration: BoxDecoration(
        // 只给一层**很淡的底**，**不加描边** —— 组里的 `ChoiceChip` / `FilterChip`
        // 自己就带描边，容器再套一圈会出现"双边框"，看着很脏（2026-10-11 试过）。
        // 分组靠"底色块 + 块之间的空隙"表达，足够清楚。
        // 用 M3 的表面角色而不是 `@0.3` 的叠色：叠色在深色下会发灰、层次不清。
        color: theme.colorScheme.surfaceContainer,
        borderRadius: AppShapes.of(context).mediumRadius,
      ),
      child: Wrap(
        spacing: spacing,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(
            title,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          ...children,
        ],
      ),
    );
  }
}

/// 一排**分区**：放不下就**整组**换行。
///
/// 用 `Wrap` 而不是 `Row`：窄窗口下"一组"自己换行，而不是把某一组切在屏幕外
/// （不使用横向滚动）。
///
/// ⚠ **组之间不放分隔线**：
/// 分隔线在 `Wrap` 里会变成"行尾的一根孤线"。分组的视觉边界交给
/// [`FilterGroup`] 自己的容器 —— 它换行时整块走。
class FilterGroupRow extends StatelessWidget {
  final List<Widget> groups;

  /// 组与组之间的间距。
  final double spacing;

  const FilterGroupRow({super.key, required this.groups, this.spacing = 8});

  @override
  Widget build(BuildContext context) {
    return Wrap(
      spacing: spacing,
      runSpacing: spacing,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: groups,
    );
  }
}

/// 提示词类型徽标
class KindBadge extends StatelessWidget {
  final PromptKind kind;
  final bool compact;

  /// 贴在图片/视频缩略图上时用「实心」样式。
  /// 缩略图底色深浅不定，半透明底（alpha 0.14）+ 同色文字基本读不出来，
  /// 所以压在图上时改用不透明底色 + 白字 + 投影。
  final bool onImage;

  const KindBadge({
    super.key,
    required this.kind,
    this.compact = false,
    this.onImage = false,
  });

  static IconData iconOfKind(PromptKind k) => switch (k) {
    PromptKind.image => Icons.image_outlined,
    PromptKind.video => Icons.movie_outlined,
    PromptKind.audio => Icons.music_note_outlined,
    PromptKind.mixed => Icons.auto_awesome_mosaic_outlined,
  };

  /// 该类型在当前配色里的"强调色"。
  ///
  /// 以前是四个写死的 M2 500 色（Green / Blue / Purple / Orange 500）：
  /// 它们在浅色主题下偏亮、在动态取色下与主色打架。现在改用 M3 的角色 ——
  /// 图片走 primary、视频走 tertiary、音频走 secondary，"混合"走中性色。
  static Color colorOfKind(ColorScheme scheme, PromptKind k) => switch (k) {
    PromptKind.image => scheme.primary,
    PromptKind.video => scheme.tertiary,
    PromptKind.audio => scheme.secondary,
    PromptKind.mixed => scheme.onSurfaceVariant,
  };

  static Color _containerOf(ColorScheme scheme, PromptKind k) => switch (k) {
    PromptKind.image => scheme.primaryContainer,
    PromptKind.video => scheme.tertiaryContainer,
    PromptKind.audio => scheme.secondaryContainer,
    PromptKind.mixed => scheme.surfaceContainerHighest,
  };

  static Color _onContainerOf(ColorScheme scheme, PromptKind k) => switch (k) {
    PromptKind.image => scheme.onPrimaryContainer,
    PromptKind.video => scheme.onTertiaryContainer,
    PromptKind.audio => scheme.onSecondaryContainer,
    PromptKind.mixed => scheme.onSurfaceVariant,
  };

  static Color _onColorOf(ColorScheme scheme, PromptKind k) => switch (k) {
    PromptKind.image => scheme.onPrimary,
    PromptKind.video => scheme.onTertiary,
    PromptKind.audio => scheme.onSecondary,
    PromptKind.mixed => scheme.surface,
  };

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final color = colorOfKind(scheme, kind);
    // 压在图片 / 视频上时用实心角色色 + on 色（不再叠 alpha：0.95 的白字
    // 底色在浅色主题下会变成"浅底白字"）。
    final bg = onImage ? color : _containerOf(scheme, kind);
    final fg = onImage
        ? _onColorOf(scheme, kind)
        : _onContainerOf(scheme, kind);
    return Container(
      padding: EdgeInsets.symmetric(
        horizontal: compact ? 6 : 8,
        vertical: compact ? 2 : 3,
      ),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(AppShapes.standard.small),
        // 缩略图底色深浅不定，实心角标需要一点投影把边缘拉开
        // （这是"压在图片上"的局部阴影，不是列表项那种每帧 N 份的 elevation）。
        boxShadow: onImage
            ? const [
                BoxShadow(
                  color: Colors.black26,
                  blurRadius: 4,
                  offset: Offset(0, 1),
                ),
              ]
            : null,
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(iconOfKind(kind), size: compact ? 11 : 13, color: fg),
          const SizedBox(width: 4),
          Text(
            kind.label,
            style: TextStyle(
              fontSize: compact ? 10 : 11.5,
              color: fg,
              fontWeight: onImage ? FontWeight.w700 : FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

/// 媒体类型徽标
class MediaKindBadge extends StatelessWidget {
  final MediaKind kind;

  const MediaKindBadge({super.key, required this.kind});

  @override
  Widget build(BuildContext context) {
    final promptKind = switch (kind) {
      MediaKind.image => PromptKind.image,
      MediaKind.video => PromptKind.video,
      MediaKind.audio => PromptKind.audio,
    };
    return KindBadge(kind: promptKind, compact: true, onImage: true);
  }
}

/// 空状态占位
class EmptyState extends StatelessWidget {
  final IconData icon;
  final String title;
  final String? subtitle;
  final Widget? action;

  const EmptyState({
    super.key,
    required this.icon,
    required this.title,
    this.subtitle,
    this.action,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 56, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: 14),
            Text(title, style: theme.textTheme.titleMedium),
            if (subtitle != null) ...[
              const SizedBox(height: 6),
              Text(
                subtitle!,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ],
            if (action != null) ...[const SizedBox(height: 18), action!],
          ],
        ),
      ),
    );
  }
}

/// 错误提示 + 重试
class ErrorState extends StatelessWidget {
  final String message;
  final VoidCallback? onRetry;

  const ErrorState({super.key, required this.message, this.onRetry});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.cloud_off_outlined,
              size: 52,
              color: theme.colorScheme.error,
            ),
            const SizedBox(height: 14),
            Text('加载失败', style: theme.textTheme.titleMedium),
            const SizedBox(height: 8),
            SelectableText(
              message,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall,
            ),
            if (onRetry != null) ...[
              const SizedBox(height: 18),
              FilledButton.icon(
                onPressed: onRetry,
                icon: const Icon(Icons.refresh),
                label: const Text('重试'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// 带复制按钮的文本框（提示词展示用）
///
/// 注意：这里刻意不给 SelectableText 设 maxLines —— 它底层是 EditableText，
/// 设了 maxLines: 30 会**预留** 30 行的高度，把卡片撑得极高。让它按内容自然撑开即可。
class CopyableText extends StatelessWidget {
  final String? text;
  final String label;
  final bool monospace;

  const CopyableText({
    super.key,
    required this.text,
    required this.label,
    this.monospace = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final value = (text ?? '').trim();
    if (value.isEmpty) {
      return Text(
        '$label 为空',
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text(
              label,
              style: theme.textTheme.labelMedium?.copyWith(
                color: theme.colorScheme.primary,
                fontWeight: FontWeight.w700,
              ),
            ),
            const Spacer(),
            IconButton(
              visualDensity: VisualDensity.compact,
              tooltip: '复制',
              icon: const Icon(Icons.copy_all_outlined, size: 16),
              onPressed: () => copyToClipboard(context, value, label: label),
            ),
          ],
        ),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainer,
            borderRadius: AppShapes.of(context).smallRadius,
            border: Border.all(color: theme.colorScheme.outlineVariant),
          ),
          child: SelectableText(
            value,
            style: theme.textTheme.bodyMedium?.copyWith(
              height: 1.5,
              fontFamily: monospace ? 'monospace' : null,
            ),
          ),
        ),
      ],
    );
  }
}

/// 参数小格子
class MetaChip extends StatelessWidget {
  final String label;
  final String? value;
  final IconData? icon;

  const MetaChip({super.key, required this.label, this.value, this.icon});

  @override
  Widget build(BuildContext context) {
    if (value == null || value!.isEmpty) return const SizedBox.shrink();
    final theme = Theme.of(context);
    return Container(
      margin: const EdgeInsets.only(right: 8, bottom: 8),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHigh,
        borderRadius: AppShapes.of(context).smallRadius,
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 13, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(width: 4),
          ],
          Text(
            label,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(width: 6),
          Text(
            value!,
            style: theme.textTheme.labelMedium?.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

Future<void> copyToClipboard(
  BuildContext context,
  String text, {
  String label = '内容',
}) async {
  await Clipboard.setData(ClipboardData(text: text));
  if (!context.mounted) return;
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text('$label已复制'),
      duration: const Duration(milliseconds: 1200),
      behavior: SnackBarBehavior.floating,
    ),
  );
}

/// 在鼠标**右键点下去的位置**弹出菜单。
///
/// 历史实现基于 `showMenu`（Material 的 `_PopupMenuRoute`）。它会铺一层铺满窗口的
/// `ModalBarrier`，在命中测试里第一个命中就终止整条路径 —— **菜单一开，页面滚轮与
/// 拖动全部失效**。现在统一走 [showAppContextMenu]（`MenuAnchor` +
/// `TapRegion`，不铺屏障），见 `lib/widgets/context_menu.dart`。
///
/// 调用方所在页面必须已经挂了 `ContextMenuScope`（`pages/*` 都挂了）。
Future<T?> showContextMenuAt<T>(
  BuildContext context, {
  required Offset globalPosition,
  required MenuSpec<T> Function(BuildContext context) items,
}) => showAppContextMenu<T>(
  context,
  globalPosition: globalPosition,
  buildOptions: items,
);
