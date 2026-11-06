import 'dart:io' show Platform;

import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride, kIsWeb, TargetPlatform;
import 'package:flutter/material.dart';

/// 标题形态按**平台**分，不按窗口宽度分（用户 2026-11-05："手机端 UI 维持
/// 现状，电脑端的标题恢复原状（始终在 appbar 里）"）：
///
/// * 手机（Android / iOS）：大标题在正文里、滚走后在 AppBar 淡入；
/// * 电脑（Windows / macOS / Linux）：标题始终在 AppBar 里。
///
/// ⚠ 不要用 `kWideLayoutMinWidth` 判：电脑窗口拉窄到 900 以下时它仍是电脑端，
/// 不该长成手机的样子。
/// ⚠ 单测用 `debugDefaultTargetPlatformOverride` 模拟手机：`flutter test` 跑在
/// Windows 上，`Platform.isAndroid` 永远是假，手机那一套标题就一条用例都覆盖不到。
/// 正式运行时它是 null，判据仍然只有真实平台。
bool get useCollapsingPageTitle {
  final override = debugDefaultTargetPlatformOverride;
  if (override != null) {
    return override == TargetPlatform.android || override == TargetPlatform.iOS;
  }
  return !kIsWeb && (Platform.isAndroid || Platform.isIOS);
}

/// 「大标题 → AppBar 标题」的滚动联动（2026-11-05 用户定下的规则）：
///
/// * 页面顶部有一个**大标题**，它属于主体内容；
/// * 内容往上滚、大标题逐渐离开可视区时，**同一个标题**在顶部栏里逐渐出现；
/// * 滚回顶部时反过来：顶部栏的标题消失、大标题回来。
///
/// 判据只有一个：[progress]（0 = 大标题完整可见，1 = 大标题完全离开）。
/// 各页面只要把滚动偏移换算成它，剩下的交给下面两个组件，**不要各写一套**。
///
/// 进度用 [ValueNotifier] 而不是每帧 `setState`：滚动时只重建标题这一小块。
class ScrollTitleProgress extends ValueNotifier<double> {
  ScrollTitleProgress() : super(0);

  /// 距离大标题完全离开还差多少像素时开始淡入。
  static const double fadeDistance = 56;

  /// 按滚动偏移更新进度（`distance` 默认 [fadeDistance]）。
  void updateFromOffset(double offset, {double distance = fadeDistance}) {
    final next = (offset / distance).clamp(0.0, 1.0);
    if ((value - next).abs() > 0.01) value = next;
  }
}

/// 把子组件的滚动通知换算成 [ScrollTitleProgress]。
///
/// 只认最外层的滚动（`depth == 0`），页面里的横向滚动 / 内嵌列表不会误触发。
class ScrollTitleListener extends StatelessWidget {
  final ScrollTitleProgress progress;

  /// 大标题完全离开所需的滚动距离。
  final double distance;

  final Widget child;

  const ScrollTitleListener({
    super.key,
    required this.progress,
    required this.child,
    this.distance = ScrollTitleProgress.fadeDistance,
  });

  @override
  Widget build(BuildContext context) {
    return NotificationListener<ScrollNotification>(
      onNotification: (notification) {
        // 只认**纵向**且在最外层的滚动：画廊里还套着横向翻页的 `PageView`，
        // 它停在第二页时 `metrics.pixels` 是一整屏宽，会被误判成"已经滚到底"，
        // 大标题在页首就没了（用户 2026-11-05 报的"页首留一块空白"）。
        if (notification.depth == 0 &&
            notification.metrics.axis == Axis.vertical) {
          progress.updateFromOffset(
            notification.metrics.pixels,
            distance: distance,
          );
        }
        return false;
      },
      child: child,
    );
  }
}

/// 主体顶部的大标题：随滚动**上移 + 淡出**，像被内容推走一样。
class CollapsingPageTitle extends StatelessWidget {
  final String title;
  final ScrollTitleProgress progress;

  /// 占位高度。给得比文字实际高度大一点，滚动时才有"整块离开"的观感。
  final double height;

  /// 标题左边距（与页面内容对齐）。
  final double leftPadding;

  const CollapsingPageTitle({
    super.key,
    required this.title,
    required this.progress,
    this.height = 56,
    this.leftPadding = 16,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // ⚠ 高度要**跟着进度收掉**，不能只把字淡出：标题块留在布局里的话，滚走之后
    // 页面顶部会永远空着一条（用户 2026-11-05："搜索框等没有慢慢顶掉原大标题的
    // 位置，留有空白"）。现在滚动多少，标题块就矮多少，下面的内容自然顶上来。
    return AnimatedBuilder(
      animation: progress,
      builder: (context, _) {
        final t = progress.value;
        return Align(
          alignment: Alignment.topCenter,
          heightFactor: (1 - t).clamp(0.0, 1.0),
          child: SizedBox(
            width: double.infinity,
            height: height,
            child: ClipRect(
              child: Opacity(
                opacity: (1 - t).clamp(0.0, 1.0),
                child: Transform.translate(
                  offset: Offset(0, -height * 0.7 * t),
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: Padding(
                      padding: EdgeInsets.only(left: leftPadding, right: 16),
                      child: Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.displaySmall?.copyWith(
                          fontSize: 34,
                          height: 1.15,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// 顶部栏里的同名标题：大标题离开时淡入，完全滚回顶部时**整个不建**
/// （避免出现"同一个标题在页面上有两个"）。
class FadingAppBarTitle extends StatelessWidget {
  final String title;
  final ScrollTitleProgress progress;

  const FadingAppBarTitle({
    super.key,
    required this.title,
    required this.progress,
  });

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: progress,
      builder: (context, _) {
        final t = progress.value;
        if (t <= 0.01) return const SizedBox.shrink();
        return Opacity(
          opacity: t,
          child: Text(
            title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        );
      },
    );
  }
}

/// 没有 AppBar 的页面（设置子页）用的顶部浮层：滚动之后才出现。
///
/// 它**不占布局高度**（`Positioned` 浮在内容之上），所以未滚动时页面
/// 与以前一模一样；滚动后内容从下面穿过，浮层带背景色和标题。
class FadingTopBar extends StatelessWidget {
  final String title;
  final ScrollTitleProgress progress;

  /// 是否显示返回键（与 `SettingsScaffold` 的 `showBack` 一致）。
  final bool showBack;

  /// 标题行右侧的动作。
  final List<Widget> actions;

  const FadingTopBar({
    super.key,
    required this.title,
    required this.progress,
    this.showBack = true,
    this.actions = const [],
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AnimatedBuilder(
      animation: progress,
      builder: (context, _) {
        final t = progress.value;
        if (t <= 0.01) return const SizedBox.shrink();
        return Opacity(
          opacity: t,
          child: Material(
            color: theme.colorScheme.surface,
            child: SafeArea(
              bottom: false,
              child: SizedBox(
                height: 56,
                child: Row(
                  children: [
                    if (showBack)
                      BackButton(
                        style: IconButton.styleFrom(
                          foregroundColor: theme.colorScheme.onSurface,
                        ),
                      )
                    else
                      const SizedBox(width: 16),
                    Expanded(
                      child: Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.titleLarge,
                      ),
                    ),
                    ...actions,
                    const SizedBox(width: 8),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
