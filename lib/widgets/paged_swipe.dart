import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

/// **分页内容的左右滑动切换**（用户 2026-10-24 定的统一交互）。
///
/// 需求原话："对于详情部分，左右滑动的逻辑是切换当前显示页数（有过渡动画不是硬切），
/// 其中左右滑动切换显示页数的设计同步到其他需要翻页的界面"。
///
/// ## 它做什么
///
///   * 在内容上左右拖动时，内容**跟手**平移（到第一页 / 最后一页有阻尼，不会拖出空白）；
///   * 松手后超过阈值（约 18% 宽度，或快速甩动）就切页：旧内容滑出、新内容从另一侧滑入；
///   * 翻页按钮（[PageBar]）改 `page` 时，新内容同样**从对应方向滑入** —— 不是硬切；
///   * 页数 ≤ 1 时它完全透明（连手势都不挂）。
///
/// ## 为什么不用 `PageView`
///
/// 这些列表的数据是**后端分页**的，`PageView` 要求每一页都是现成的 widget；用它就得
/// 同时预取前后页，还会把"当前页"变成两套真相（PageController 与 store 的 page）。
/// 这里只做"过渡表现"，页码真源仍然只有各处 store。
///
/// ⚠ 手势只认**水平**拖动：子树里的纵向滚动、缩略图长按拖动都各自照常工作。
/// 画廊里 `Draggable` 的即时拖动和它是同一个竞技场（指针先落在图格上时图格优先），
/// 这是刻意的 —— 拖图比翻页更"局部"。
class SwipePageView extends StatefulWidget {
  final int page;
  final int pages;

  /// 翻页回调。只会在"手势确实要求换页"时调用，且**动画滑出之后**才调用。
  final ValueChanged<int> onPageChanged;

  final Widget child;

  const SwipePageView({
    super.key,
    required this.page,
    required this.pages,
    required this.onPageChanged,
    required this.child,
  });

  @override
  State<SwipePageView> createState() => _SwipePageViewState();
}

class _SwipePageViewState extends State<SwipePageView>
    with SingleTickerProviderStateMixin {
  static const Duration _slide = Duration(milliseconds: 260);
  static const Duration _snap = Duration(milliseconds: 180);

  /// 0 = 正常位置；+1 = 内容整体右移一屏；-1 = 左移一屏。
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: _slide,
  );

  /// 手指拖动产生的位移（像素）。
  double _dragPx = 0;

  /// 外部换页（翻页按钮 / 快捷键）时，新内容先停在进入侧的那一帧。
  ///
  /// 为什么要它：`didUpdateWidget` 里不能直接改 `AnimationController.value`
  /// （那会在 build 期间触发监听者 setState）。用这个值让**第一帧**就把新内容
  /// 摆到屏幕外，post-frame 再开始动画 —— 不会先闪一下再滑。
  double _pendingFraction = 0;

  /// 手势触发的换页正在播（期间忽略外部 page 变化，避免两套动画打架）。
  bool _switching = false;

  /// 外部换页的序号：连续的 page 变化只让最后一次的 post-frame 生效。
  int _externalSeq = 0;

  double _width = 0;

  @override
  void didUpdateWidget(covariant SwipePageView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.page == oldWidget.page || _switching) return;
    final dir = widget.page > oldWidget.page ? 1.0 : -1.0;
    final seq = ++_externalSeq;
    _pendingFraction = dir;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _switching || seq != _externalSeq) return;
      _pendingFraction = 0;
      _controller.value = dir;
      _controller.animateTo(0, duration: _slide, curve: Curves.easeOutCubic);
    });
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _onDragStart(DragStartDetails details) {
    _controller.stop();
    _dragPx = 0;
  }

  void _onDragUpdate(DragUpdateDetails details) {
    final delta = details.primaryDelta ?? 0;
    setState(() {
      _dragPx += delta;
      if (_width <= 0) return;
      final atFirst = widget.page <= 1;
      final atLast = widget.page >= widget.pages;
      // 到头的方向给一点阻尼，让人知道"已经没有了"，而不是拖出大片空白
      if (atFirst && _dragPx > 0) {
        _dragPx = _dragPx.clamp(0.0, _width * 0.14).toDouble();
      } else if (atLast && _dragPx < 0) {
        _dragPx = _dragPx.clamp(-_width * 0.14, 0.0).toDouble();
      }
    });
  }

  void _onDragEnd(DragEndDetails details) {
    final velocity = details.primaryVelocity ?? 0;
    final dx = _dragPx;
    final fast = velocity.abs() > 520 && dx.abs() > 12;
    final farEnough = dx.abs() >= _width * 0.18;
    final dir = dx < 0 ? 1 : -1;
    final canGo = dir > 0 ? widget.page < widget.pages : widget.page > 1;
    if ((!farEnough && !fast) || !canGo) {
      _snapBack();
      return;
    }
    unawaited(_swipeTo(dir));
  }

  void _onDragCancel() => _snapBack();

  void _snapBack() {
    final from = _width <= 0 ? 0.0 : _dragPx / _width;
    setState(() => _dragPx = 0);
    _controller.value = from.clamp(-1.0, 1.0);
    _controller.animateTo(0, duration: _snap, curve: Curves.easeOutCubic);
  }

  /// [dir] = +1 下一页，-1 上一页。
  Future<void> _swipeTo(int dir) async {
    if (_switching || _width <= 0) return;
    _switching = true;
    try {
      // 先把"手指拖到哪"接到动画控制器上，再平滑滑出
      final start = _dragPx / _width;
      setState(() => _dragPx = 0);
      _controller.value = start.clamp(-1.0, 1.0);
      await _controller.animateTo(
        -dir.toDouble(),
        duration: _slide,
        curve: Curves.easeOutCubic,
      );
      if (!mounted) return;
      // 旧内容已经滑出：让父级换页；新内容先停在**进入侧**
      _pendingFraction = dir.toDouble();
      widget.onPageChanged(widget.page + dir);
      await WidgetsBinding.instance.endOfFrame;
      if (!mounted) return;
      _pendingFraction = 0;
      _controller.value = dir.toDouble();
      await _controller.animateTo(
        0,
        duration: _slide,
        curve: Curves.easeOutCubic,
      );
    } on TickerCanceled {
      // 页面在动画途中被销毁：正常情况，什么都不用做
    } finally {
      _switching = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    if (widget.pages <= 1) return widget.child;
    return LayoutBuilder(
      builder: (context, constraints) {
        _width = constraints.maxWidth;
        return ClipRect(
          child: AnimatedBuilder(
            animation: _controller,
            builder: (context, child) {
              final fraction = _width <= 0
                  ? 0.0
                  : _dragPx / _width + _pendingFraction + _controller.value;
              // 滑出去的旧内容稍微淡一点：两个方向都看得出"在换页"而不是在拖动整页
              final opacity =
                  1.0 - math.min(0.28, fraction.abs() * 0.28).toDouble();
              return Transform.translate(
                offset: Offset(fraction * _width, 0),
                child: Opacity(opacity: opacity, child: child),
              );
            },
            child: GestureDetector(
              // 让子树的点击 / 纵向滚动先正常参与命中测试
              behavior: HitTestBehavior.deferToChild,
              onHorizontalDragStart: _onDragStart,
              onHorizontalDragUpdate: _onDragUpdate,
              onHorizontalDragEnd: _onDragEnd,
              onHorizontalDragCancel: _onDragCancel,
              child: widget.child,
            ),
          ),
        );
      },
    );
  }
}

/// 分页条：`‹ 第 3 / 12 页 ›`，与 [SwipePageView] 配套。
///
/// 抽出来是因为画廊与预设页原本各写了一份**逐字相同**的实现；统一之后
/// "左右滑动可翻页"的提示、按钮语义、窄屏形态只有一处要维护。
/// 页数 ≤ 1 时它不占任何高度（调用方不用自己判 `pages > 1`）。
class PageBar extends StatelessWidget {
  final int page;
  final int pages;
  final ValueChanged<int> onChanged;

  const PageBar({
    super.key,
    required this.page,
    required this.pages,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 6),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: LayoutBuilder(
        builder: (context, constraints) {
          // 360dp 上只放按钮与页码；中等宽度才加"左右滑动"的提示（不给窄屏添堵）
          final showHint = constraints.maxWidth >= 520;
          return Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              IconButton(
                tooltip: '上一页',
                visualDensity: VisualDensity.compact,
                icon: const Icon(Icons.chevron_left),
                onPressed: page > 1 ? () => onChanged(page - 1) : null,
              ),
              Text('第 $page / $pages 页'),
              IconButton(
                tooltip: '下一页',
                visualDensity: VisualDensity.compact,
                icon: const Icon(Icons.chevron_right),
                onPressed: page < pages ? () => onChanged(page + 1) : null,
              ),
              if (showHint) ...[
                const SizedBox(width: 14),
                Icon(
                  Icons.swipe_outlined,
                  size: 15,
                  color: theme.colorScheme.outline,
                ),
                const SizedBox(width: 4),
                Text(
                  '左右滑动翻页',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ],
            ],
          );
        },
      ),
    );
  }
}
