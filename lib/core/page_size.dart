import 'dart:ui' show Size;

import 'layout_constants.dart';

/// 画廊网格一格的最大宽度与格间距（与 `gallery_page.dart` 的 `SliverGridDelegate`
/// 保持一致）。
const double kGalleryCellMaxExtent = 230;
const double kGalleryCellSpacing = 10;

/// 侧边导航栏占用的宽度（`NavigationRail` 的默认宽度）。
const double kNavRailWidth = 80;

/// 电脑端"排满一页"的每页数量。
///
/// 判据是**窗口逻辑尺寸**，不是某个页面的实际约束：画廊 / 预设 / 标签都按同一个
/// 数字分页，窗口一改这个数字就跟着改。算法与画廊网格一致：
///   1. 扣掉侧边栏与左右内边距，按 240 一格算出列数；
///   2. 按实际格宽（列数定了之后会摊开）算出可见行数；
///   3. 列数 × 行数就是"排满一页"的条数，夹在 8~120（与设置里的滑杆同档）。
///
/// 高度扣掉的部分是标题栏、筛选栏、翻页条与网格上下内边距的估算值；它是
/// "排满一页"的近似，不要求逐像素精确。
int autoPageSizeFor(Size windowSize) {
  final rail = windowSize.width >= kWideLayoutMinWidth ? kNavRailWidth : 0.0;
  final width = (windowSize.width - rail - 32).clamp(1.0, double.infinity);
  final columns = (width / (kGalleryCellMaxExtent + kGalleryCellSpacing))
      .ceil()
      .clamp(1, 12);
  final cellWidth =
      (width - (columns - 1) * kGalleryCellSpacing) / columns;
  final height = (windowSize.height - 170).clamp(1.0, double.infinity);
  // 只算**完整可见**的行：多算一行会在窗口底部露出半排格子，用户看到的是
  // "这个窗口明明只放得下三排，却排了四排"。
  final rows = (height / (cellWidth + kGalleryCellSpacing))
      .floor()
      .clamp(1, 12);
  return (columns * rows).clamp(8, 120);
}
