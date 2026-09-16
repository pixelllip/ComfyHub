import 'dart:math' as math;

import 'package:flutter/material.dart';

// 自适应多列布局。
//
// 桌面端窗口可以拉得很宽，单列列表会把一行拉成一米长（读起来很累、
// 右边一大片空着）。这里的规则很简单：
//
//     **列数 = 可用宽度 / 550**
//
// 也就是每列至少 550 逻辑像素，窗口越宽列越多，窄了自动退回单列。
// 550 是"一行中文提示词读起来不费劲"的经验值（再宽眼睛就要来回扫）。

/// 单列的理想宽度：小于它就不要分列，大于它的整数倍就多分一列
const double kColumnWidth = 550;

/// 列数上限。窗口再宽也不无限分列 —— 4 列（≈2200px）之后每列还是 550，
/// 再多就只是把内容切得更碎，对阅读没有帮助。
const int kMaxColumns = 4;

/// 按「窗口宽度 / [minColumnWidth]」算列数，至少 1 列、最多 [maxColumns] 列。
int adaptiveColumnCount(
  double width, {
  double minColumnWidth = kColumnWidth,
  int maxColumns = kMaxColumns,
}) {
  if (!width.isFinite || width <= 0 || minColumnWidth <= 0) return 1;
  final n = (width / minColumnWidth).floor();
  return n.clamp(1, math.max(1, maxColumns));
}

/// 等高的多列列表：把 [itemCount] 个条目按列数**按行**排开。
///
/// 同一行里的卡片高度对齐到最高的那一张（[IntrinsicHeight]），所以横着看是齐的；
/// 适合卡片高度接近的列表（提示词 / 标签）。
///
/// 懒加载：只构建可见的那些行，翻页 / 上千条也不会卡。
///
/// 性能注意：`IntrinsicHeight` 会让这一行**多走一遍固有尺寸查询**
/// （卡片里的 `Wrap` 标签、多行正文都要按列宽重新量一次），所以只在
/// "一行里真的并排了 ≥2 张卡片、需要等高"时才套它；
/// 单列（窄窗口）和每行的尾巴只有一个条目时，直接 `CrossAxisAlignment.start`
/// 排下去 —— 一个条目本来就没什么可对齐的，白花的那次查询直接省掉。
class AdaptiveColumnList extends StatelessWidget {
  final int itemCount;
  final Widget Function(BuildContext context, int index) itemBuilder;

  final double minColumnWidth;
  final int maxColumns;

  /// 列间距
  final double spacing;

  /// 行间距
  final double runSpacing;

  final EdgeInsetsGeometry padding;
  final ScrollController? controller;
  final ScrollPhysics? physics;

  const AdaptiveColumnList({
    super.key,
    required this.itemCount,
    required this.itemBuilder,
    this.minColumnWidth = kColumnWidth,
    this.maxColumns = kMaxColumns,
    this.spacing = 10,
    this.runSpacing = 10,
    this.padding = EdgeInsets.zero,
    this.controller,
    this.physics,
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
        final rows = (itemCount + columns - 1) ~/ columns;

        return ListView.builder(
          controller: controller,
          physics: physics,
          padding: padding,
          itemCount: rows,
          itemBuilder: (context, row) {
            final start = row * columns;
            final end = math.min(start + columns, itemCount);
            final items = <Widget>[
              for (var i = start; i < end; i++) ...[
                if (i > start) SizedBox(width: spacing),
                Expanded(child: itemBuilder(context, i)),
              ],
              // 最后一行不满时补空位，保证每列宽度和其他行一致
              for (var i = end; i < start + columns; i++) ...[
                SizedBox(width: spacing),
                const Expanded(child: SizedBox.shrink()),
              ],
            ];
            // 单条目行（窄窗口的单列，或最后一行的尾巴）不需要等高，也就没必要
            // 付 IntrinsicHeight 那次固有尺寸查询的钱。
            final single = end - start <= 1;
            return Padding(
              padding: EdgeInsets.only(bottom: row == rows - 1 ? 0 : runSpacing),
              child: single
                  ? Row(crossAxisAlignment: CrossAxisAlignment.start, children: items)
                  : IntrinsicHeight(
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: items,
                      ),
                    ),
            );
          },
        );
      },
    );
  }
}

/// 多列排布里的一"块"内容。
///
/// [estimatedHeight] 只用于**分列时的均衡**（把下一块放进当前最矮的那一列），
/// 不需要精确 —— 估错了最多是某列长一点，不会错位。
class AdaptiveSection {
  final Widget child;
  final double estimatedHeight;

  const AdaptiveSection(this.child, {required this.estimatedHeight});
}

/// 高度差很大的多列布局（设置页那种：一张卡几百像素、另一张只有一百多）。
///
/// 按行对齐会产生大片空白，所以这里改成"瀑布流"：每块依次放进当前最矮的一列，
/// 列与列之间互不影响。阅读顺序是"左列从上到下、再到右列"，和报纸分栏一致。
class AdaptiveColumns extends StatelessWidget {
  final List<AdaptiveSection> sections;

  final double minColumnWidth;
  final int maxColumns;

  /// 列间距
  final double spacing;

  /// 同一列里两块之间的间距
  final double runSpacing;

  const AdaptiveColumns({
    super.key,
    required this.sections,
    this.minColumnWidth = kColumnWidth,
    this.maxColumns = kMaxColumns,
    this.spacing = 20,
    this.runSpacing = 24,
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
            children: _withGaps(sections.map((s) => s.child).toList()),
          );
        }

        final buckets = List.generate(columns, (_) => <Widget>[]);
        final heights = List<double>.filled(columns, 0);
        for (final section in sections) {
          var target = 0;
          for (var i = 1; i < columns; i++) {
            if (heights[i] < heights[target]) target = i;
          }
          buckets[target].add(section.child);
          heights[target] += section.estimatedHeight + runSpacing;
        }

        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            for (var i = 0; i < columns; i++) ...[
              if (i > 0) SizedBox(width: spacing),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: _withGaps(buckets[i]),
                ),
              ),
            ],
          ],
        );
      },
    );
  }

  List<Widget> _withGaps(List<Widget> items) => [
        for (var i = 0; i < items.length; i++) ...[
          if (i > 0) SizedBox(height: runSpacing),
          items[i],
        ],
      ];
}
