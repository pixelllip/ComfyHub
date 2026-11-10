import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../core/theme.dart';
import 'adaptive_layout.dart';

/// 设置首页的**入口磁贴**：图标 + 名称 + 一句说明 + 箭头。
///
/// 首页使用 iPad 式入口，不新增左侧导航。
/// 于是入口从"一条一条的卡片行"改成等高磁贴：多个磁贴并排时**同一行高度
/// 自动拉平**（[SettingsTileGrid] 用 `IntrinsicHeight`），不会像以前的瀑布流
/// 那样出现某一列比另一列长一截。
///
/// 磁贴内容刻意压到两行（名称 / 说明各最多两行），并且**横向布局**（图标在左、
/// 文字在右）：窄到 320dp 时它仍然是一张紧凑的入口卡，不会变成半张空白。
class SettingsTile extends StatelessWidget {
  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback? onTap;

  const SettingsTile({
    super.key,
    required this.icon,
    required this.title,
    required this.subtitle,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final shapes = AppShapes.of(context);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 14, 10, 14),
          child: Row(
            children: [
              // 图标底色用主色的淡层：深色 / 浅色 / 动态取色三套主题下都成立，
              // 不像 `primaryContainer` 那样在动态取色时会变得很艳。
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  color: theme.colorScheme.primary.withValues(alpha: 0.14),
                  borderRadius: shapes.smallRadius,
                ),
                child: Icon(icon, size: 20, color: theme.colorScheme.primary),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleSmall,
                    ),
                    const SizedBox(height: 2),
                    Text(
                      subtitle,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                        height: 1.35,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 6),
              Icon(
                Icons.chevron_right,
                size: 18,
                color: theme.colorScheme.onSurfaceVariant.withValues(
                  alpha: 0.6,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 等高的磁贴网格：按可用宽度决定列数（默认 280 起一列，最多 3 列）。
///
/// **同一行的磁贴高度一律拉平**（[IntrinsicHeight] + `stretch`），所以行与行
/// 之间是平齐的；最后一行不满时补空位，保证每列宽度恒定。
///
/// 之所以能"等高"，不是因为估得准，而是因为磁贴本身只有两行文字：一行高度
/// 与两行高度的差别由 [IntrinsicHeight] 当场量出来，不依赖任何估算值。
class SettingsTileGrid extends StatelessWidget {
  final List<Widget> tiles;

  final double minColumnWidth;
  final int maxColumns;

  /// 列间距
  final double spacing;

  /// 行间距
  final double runSpacing;

  const SettingsTileGrid({
    super.key,
    required this.tiles,
    this.minColumnWidth = 280,
    this.maxColumns = 3,
    this.spacing = 12,
    this.runSpacing = 12,
  });

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final columns = adaptiveColumnCount(
          constraints.maxWidth,
          minColumnWidth: minColumnWidth,
          maxColumns: maxColumns,
        );
        if (columns <= 1) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var i = 0; i < tiles.length; i++) ...[
                if (i > 0) SizedBox(height: runSpacing),
                tiles[i],
              ],
            ],
          );
        }

        final rows = <Widget>[];
        for (var start = 0; start < tiles.length; start += columns) {
          final end = math.min(start + columns, tiles.length);
          rows.add(
            IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (var i = start; i < end; i++) ...[
                    if (i > start) SizedBox(width: spacing),
                    Expanded(child: tiles[i]),
                  ],
                  // 最后一行不满时补空位，否则剩下的磁贴会被拉宽
                  for (var i = end; i < start + columns; i++) ...[
                    SizedBox(width: spacing),
                    const Expanded(child: SizedBox.shrink()),
                  ],
                ],
              ),
            ),
          );
        }

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < rows.length; i++) ...[
              if (i > 0) SizedBox(height: runSpacing),
              rows[i],
            ],
          ],
        );
      },
    );
  }
}
