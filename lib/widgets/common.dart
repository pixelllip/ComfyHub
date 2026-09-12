import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/formatting.dart';
import '../models/models.dart';

/// 标签小胶囊。可用于展示、筛选、删除。
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
    final base = color ?? tagColor(label);
    final bg = selected ? base : base.withValues(alpha: 0.12);
    final fg = selected ? Colors.white : Color.alphaBlend(base.withValues(alpha: 0.85), theme.colorScheme.surface);

    return Padding(
      padding: EdgeInsets.only(right: dense ? 4 : 6, bottom: dense ? 4 : 6),
      child: Material(
        color: bg,
        borderRadius: BorderRadius.circular(20),
        child: InkWell(
          borderRadius: BorderRadius.circular(20),
          onTap: onTap,
          child: Padding(
            padding: EdgeInsets.symmetric(
              horizontal: dense ? 8 : 10,
              vertical: dense ? 3 : 5,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (selected) ...[
                  const Icon(Icons.check, size: 13, color: Colors.white),
                  const SizedBox(width: 3),
                ],
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
                if (onDeleted != null) ...[
                  const SizedBox(width: 2),
                  GestureDetector(
                    onTap: onDeleted,
                    child: Icon(Icons.close, size: dense ? 12 : 14, color: fg),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
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

  const KindBadge({super.key, required this.kind, this.compact = false, this.onImage = false});

  static IconData iconOfKind(PromptKind k) => switch (k) {
        PromptKind.image => Icons.image_outlined,
        PromptKind.video => Icons.movie_outlined,
        PromptKind.audio => Icons.music_note_outlined,
        PromptKind.mixed => Icons.auto_awesome_mosaic_outlined,
      };

  static Color colorOfKind(PromptKind k) => switch (k) {
        PromptKind.image => const Color(0xFF4CAF50),
        PromptKind.video => const Color(0xFF2196F3),
        PromptKind.audio => const Color(0xFF9C27B0),
        PromptKind.mixed => const Color(0xFFFF9800),
      };

  @override
  Widget build(BuildContext context) {
    final color = colorOfKind(kind);
    final bg = onImage ? color.withValues(alpha: 0.95) : color.withValues(alpha: 0.14);
    final fg = onImage ? Colors.white : color;
    return Container(
      padding: EdgeInsets.symmetric(horizontal: compact ? 6 : 8, vertical: compact ? 2 : 3),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(6),
        boxShadow: onImage
            ? const [BoxShadow(color: Colors.black38, blurRadius: 4, offset: Offset(0, 1))]
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
              shadows: onImage ? const [Shadow(blurRadius: 2, color: Colors.black54)] : null,
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
            Icon(icon, size: 56, color: theme.colorScheme.outline),
            const SizedBox(height: 14),
            Text(title, style: theme.textTheme.titleMedium),
            if (subtitle != null) ...[
              const SizedBox(height: 6),
              Text(
                subtitle!,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
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
            Icon(Icons.cloud_off_outlined, size: 52, color: theme.colorScheme.error),
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
        '$label：（空）',
        style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
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
            color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.5),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: theme.dividerColor.withValues(alpha: 0.4)),
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
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.6),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 13, color: theme.colorScheme.outline),
            const SizedBox(width: 4),
          ],
          Text(
            label,
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
          ),
          const SizedBox(width: 6),
          Text(
            value!,
            style: theme.textTheme.labelMedium?.copyWith(fontWeight: FontWeight.w600),
          ),
        ],
      ),
    );
  }
}

Future<void> copyToClipboard(BuildContext context, String text, {String label = '内容'}) async {
  await Clipboard.setData(ClipboardData(text: text));
  if (!context.mounted) return;
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text('$label已复制'),
      duration: const Duration(milliseconds: 1200),
      behavior: SnackBarBehavior.floating,
      width: 240,
    ),
  );
}

/// 在鼠标**右键点下去的位置**弹出菜单。
///
/// [showMenu] 要的是"相对 Overlay 的矩形"，每个调用点各写一遍很容易算错
/// （算错了菜单就跑到屏幕角上），所以统一收到这里。
Future<T?> showContextMenuAt<T>(
  BuildContext context, {
  required Offset globalPosition,
  required List<PopupMenuEntry<T>> items,
}) {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
  return showMenu<T>(
    context: context,
    position: RelativeRect.fromRect(
      globalPosition & const Size(1, 1),
      Offset.zero & overlay.size,
    ),
    items: items,
  );
}

/// 右键菜单里的一项：图标 + 文字，[danger] 用于删除这种破坏性操作。
PopupMenuItem<T> contextMenuItem<T>(
  BuildContext context, {
  required T value,
  required IconData icon,
  required String label,
  bool enabled = true,
  bool danger = false,
}) {
  final theme = Theme.of(context);
  final color = danger ? theme.colorScheme.error : null;
  return PopupMenuItem<T>(
    value: value,
    enabled: enabled,
    height: 42,
    child: Row(
      children: [
        Icon(
          icon,
          size: 18,
          color: enabled ? color ?? theme.colorScheme.onSurfaceVariant : theme.disabledColor,
        ),
        const SizedBox(width: 10),
        Text(
          label,
          style: enabled && danger ? TextStyle(color: color) : null,
        ),
      ],
    ),
  );
}
