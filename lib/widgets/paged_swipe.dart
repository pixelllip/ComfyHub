import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

/// **分页内容的左右滑动切换**（统一交互）。
///
/// 过渡要求：
///
/// > 过渡动画同时显示新旧两页的内容，动画结束后注销旧界面。
///
/// 因此它不是"旧内容淡出、新内容淡入"的替换器，而是一个**小号 PageView**：
///
///   * 手指拖动时，当前页跟着走，相邻页从另一侧一起露出来 —— 拖到一半就是
///     "左边半页旧内容 + 右边半页新内容"；
///   * 松手过阈值（约 18% 宽度，或快速甩动）就滑到相邻页：整个动画里两页始终
///     同时可见，直到动画结束才把旧页从树里拿掉；
///   * 翻页按钮 / 快捷键改 [page] 时也走同一条动画，不硬切；
///   * 新一页的数据还没到时，旧页**原地留着**（不闪加载圈、不先滑出去再滑回来），
///     只在内容区顶上显示一条 2px 的细进度条；数据到了再滑。
///
/// ## 每页内容怎么来
///
/// 用 [builder] 按页码现取。**邻居页的数据要调用方提前准备好**（画廊 / 预设页
/// 由 store 分页缓存，详情页由同批产物缓存）；拿不到就返回 `null`，组件会：
///   1. 通过 [onPageNeeded] 请求这一页；
///   2. 在数据到之前不硬滑，旧页继续显示。
///
/// ## 以后接新的分页界面
///
/// ```dart
/// SwipePageView(
///   page: store.requestedPage,     // 点击后立刻变
///   pages: store.pages,
///   builder: (page) => store.hasPage(page) ? MyList(items: store.itemsOf(page)) : null,
///   onPageChanged: store.setPage,
///   onPageNeeded: store.loadPage,  // 预取相邻页
/// )
/// ```
///
/// ⚠ 手势只认**水平**拖动。子树里如果自己有纵向滚动 / 拖动，各自照常工作。
class SwipePageView extends StatefulWidget {
  /// 目标页（点翻页按钮 / 滑动松手后立即切换）。
  final int page;

  final int pages;

  /// 按页码取一页的内容。数据还没准备好时返回 `null`。
  final Widget? Function(int page) builder;

  /// 翻页回调。手势触发的换页在**松手后立即**调用（不等数据）。
  final ValueChanged<int> onPageChanged;

  /// 需要某一页的数据（用于预热相邻页）。可以重复调用，调用方自己做去重。
  final ValueChanged<int>? onPageNeeded;

  /// 让外部（比如大图查看器的 `Listener`）也能驱动这套拖动。
  final SwipePageController? controller;

  /// 要不要挂"左右拖动 = 翻页"的手势。
  ///
  /// 桌面画廊里每个格子本身可以直接拖去组成集合，横向拖动会被格子先抢走；
  /// 这时关掉翻页手势、只用显式按钮，反而不会出现"想翻页却拖走了一张图"。
  /// 手机端格子是长按才拖，和平滑翻页不冲突，保持开启。
  final bool swipeEnabled;

  /// 旧页滑出 + 新页滑入的时长。
  final Duration duration;

  /// 一次换页动画（或回弹）结束时回调；用于把"新页的信息面板"推迟到动画之后更新。
  final VoidCallback? onTransitionEnd;

  /// 这一页内容的版本。父级重建时版本没变的页**不重新 build** —— 翻页只改页码，
  /// 不该把当前页 / 预建页的整张网格或列表再建一遍。`null` = 每次都重建。
  final Object? Function(int page)? pageVersion;

  const SwipePageView({
    super.key,
    required this.page,
    required this.pages,
    required this.builder,
    required this.onPageChanged,
    this.onPageNeeded,
    this.controller,
    this.swipeEnabled = true,
    this.duration = const Duration(milliseconds: 300),
    this.onTransitionEnd,
    this.pageVersion,
  });

  @override
  State<SwipePageView> createState() => _SwipePageViewState();
}

/// 让页面内容自己驱动翻页拖动（大图查看器在 1:1 时用原始指针事件驱动它）。
///
/// 为什么需要它：`InteractiveViewer` 自己有一个 `ScaleGestureRecognizer`，
/// 手指在大图上横拖时它会先赢下手势竞技场，包在外面的 `GestureDetector`
/// 根本收不到 `onHorizontalDrag*`。所以看图器在缩放为 1 时用 `Listener`
/// 直接读指针位移，再通过这个 controller 交给翻页组件。
class SwipePageController {
  _SwipePageViewState? _state;

  void _attach(_SwipePageViewState state) => _state = state;
  void _detach(_SwipePageViewState state) {
    if (identical(_state, state)) _state = null;
  }

  void beginDrag() => _state?._beginDrag();
  void dragBy(double dx) => _state?._dragBy(dx);
  void endDrag({double velocity = 0}) => _state?._endDrag(velocity);
  void cancelDrag() => _state?._cancelDrag();
}

class _SwipePageViewState extends State<SwipePageView>
    with SingleTickerProviderStateMixin {
  static const Duration _snap = Duration(milliseconds: 180);

  late final AnimationController _anim = AnimationController(
    vsync: this,
    duration: widget.duration,
  );

  /// 当前**居中**的那一页。动画结束后才更新（动画期间旧页还在）。
  int _shownPage = 1;

  /// 已经拿到内容的页（widget 描述，不是 Element）。只在拖动 / 动画时进树。
  final Map<int, Widget> _pages = <int, Widget>{};

  /// 每页内容的版本（见 [SwipePageView.pageVersion]）。
  final Map<int, Object?> _pageVersions = <int, Object?>{};

  /// 水平拖动位移（像素）。`<0` = 往下一页拖。
  double _dragPx = 0;

  bool _dragging = false;

  /// 回弹动画（没到阈值）在播。
  bool _settling = false;

  /// 换页动画在播。
  bool _transitioning = false;
  bool _animStarted = false;
  int _fromPage = 1;
  int _toPage = 1;
  double _direction = 1;
  double _startOffset = 0;

  /// 已经请求过、但内容还没到的页（避免每帧重复请求）。
  final Set<int> _requested = <int>{};

  /// 每页一个 `GlobalKey`：页面在 Stack 里的槽位会随当前页变化，靠它保住
  /// Element / State（滚动位置、图片缩放状态都不丢）。
  final Map<int, GlobalKey> _pageKeys = <int, GlobalKey>{};

  GlobalKey _pageKey(int page) => _pageKeys.putIfAbsent(
    page,
    () => GlobalKey(debugLabel: 'swipe-page-$page'),
  );

  double _width = 0;

  bool get _waiting => widget.page != _shownPage && _pages[widget.page] == null;

  @override
  void initState() {
    super.initState();
    _shownPage = widget.page.clamp(1, math.max(1, widget.pages));
    widget.controller?._attach(this);
    // ⚠ 动画期间必须有人重建：`build` 读的是 `_anim.value`，而 `AnimationController`
    // 只推进数值、不会自动标脏。缺这个监听时，控制器跑完只由 `whenComplete` 的
    // `setState` 重建一次 —— 翻页从起点直接跳到终点，中间一帧都没有。
    _anim.addListener(_onAnimTick);
  }

  void _onAnimTick() {
    if (!mounted) return;
    setState(() {});
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _refreshPages();
    _scheduleNeeded();
  }

  @override
  void didUpdateWidget(covariant SwipePageView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller?._detach(this);
      widget.controller?._attach(this);
    }
    if (widget.pages <= 1) {
      _anim.stop();
      _transitioning = false;
      _animStarted = false;
      _settling = false;
      _dragging = false;
      _dragPx = 0;
      _shownPage = 1;
      _pages.removeWhere((page, _) => page != 1);
      _refreshPages();
      return;
    }

    _refreshPages();
    _scheduleNeeded();

    // 目标页的数据到了：从当前页滑过去（两页同时可见）。
    if (!_transitioning &&
        widget.page != _shownPage &&
        _pages[widget.page] != null) {
      _startTransitionTo(widget.page);
    }

    // 动画途中父级重建：进入的那一页内容跟着更新（收藏 / 选择态等）。
    // 内容版本没变就不重建 —— 快速连点时父级每来一次通知都重建整张网格，
    // 正是"快速切页更卡"的来源。
    if (_transitioning) {
      final version = widget.pageVersion?.call(_toPage);
      if (widget.pageVersion == null || _pageVersions[_toPage] != version) {
        final refreshed = widget.builder(_toPage);
        if (refreshed != null) {
          _pages[_toPage] = refreshed;
          _pageVersions[_toPage] = version;
        }
      }
    }
  }

  @override
  void dispose() {
    widget.controller?._detach(this);
    _anim.dispose();
    super.dispose();
  }

  // -------------------------------------------------------------------------
  //  页缓存 / 预取
  // -------------------------------------------------------------------------

  Set<int> _wantedPages() {
    final wanted = <int>{
      _shownPage,
      widget.page,
      if (_transitioning) _fromPage,
      if (_transitioning) _toPage,
    };
    // 只在"目标页就在旁边"时预建左右邻居。快速连点跳到远处时，把两边的
    // 邻居都挂上会同时保活四五页网格，反而更慢。
    if ((widget.page - _shownPage).abs() <= 1) {
      if (widget.page > 1) wanted.add(widget.page - 1);
      if (widget.page < widget.pages) wanted.add(widget.page + 1);
    }
    return wanted;
  }

  void _refreshPages() {
    final wanted = _wantedPages();
    for (final page in wanted) {
      if (page < 1 || page > widget.pages) {
        _pages.remove(page);
        _pageVersions.remove(page);
        continue;
      }
      final version = widget.pageVersion?.call(page);
      if (widget.pageVersion != null &&
          _pages.containsKey(page) &&
          _pageVersions[page] == version) {
        // 页码变了但这一页的内容没变：复用旧 widget，整张网格 / 列表不重建。
        continue;
      }
      final content = widget.builder(page);
      if (content == null) {
        _pages.remove(page);
        _pageVersions.remove(page);
      } else {
        _pages[page] = content;
        _pageVersions[page] = version;
      }
    }
    _pages.removeWhere((page, _) => !wanted.contains(page));
    _pageVersions.removeWhere((page, _) => !_pages.containsKey(page));
  }

  void _scheduleNeeded() {
    final wanted = <int>{
      if (_shownPage > 1) _shownPage - 1,
      if (_shownPage < widget.pages) _shownPage + 1,
      if (widget.page > 1) widget.page - 1,
      if (widget.page < widget.pages) widget.page + 1,
    };
    wanted.removeWhere(
      (page) => _pages.containsKey(page) || _requested.contains(page),
    );
    if (wanted.isEmpty || widget.onPageNeeded == null) return;
    _requested.addAll(wanted);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      for (final page in wanted) {
        if (!_pages.containsKey(page)) widget.onPageNeeded?.call(page);
        _requested.remove(page);
      }
    });
  }

  // -------------------------------------------------------------------------
  //  换页动画
  // -------------------------------------------------------------------------

  void _startTransitionTo(int target) {
    final content = _pages[target];
    if (content == null) return;
    _fromPage = _shownPage;
    _toPage = target;
    _direction = target >= _shownPage ? 1.0 : -1.0;
    _startOffset = _dragPx;
    _pages[target] = content;
    _dragPx = 0;
    _dragging = false;
    _settling = false;
    _transitioning = true;
    _animStarted = false;

    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_transitioning) return;
      final start = _width <= 0
          ? 0.0
          : (_startOffset.abs() / _width).clamp(0.0, 1.0);
      _anim.value = start;
      _animStarted = true;
      final left = 1.0 - start;
      final duration = widget.duration * left;
      unawaited(
        _anim
            .animateTo(
              1,
              duration: duration < const Duration(milliseconds: 60)
                  ? const Duration(milliseconds: 60)
                  : duration,
              curve: Curves.easeOutCubic,
            )
            .whenComplete(_finishTransition),
      );
    });
  }

  void _finishTransition() {
    if (!mounted) return;
    final finished = _toPage;
    setState(() {
      _transitioning = false;
      _animStarted = false;
      _anim.value = 0;
      _shownPage = finished;
      _startOffset = 0;
    });
    _refreshPages();
    _scheduleNeeded();

    // 动画的 300ms 内可能再次点击翻页：数据到达后继续滑动，保持页码与画面一致。
    if (widget.page != _shownPage) {
      if (_pages[widget.page] != null) _startTransitionTo(widget.page);
    } else {
      // 页码已经对齐：这次换页的收尾交给宿主（信息面板等）。
      widget.onTransitionEnd?.call();
    }
  }

  void _snapBack() {
    final from = _width <= 0 ? 0.0 : (_dragPx / _width).clamp(-1.0, 1.0);
    setState(() {
      _dragPx = 0;
      _settling = true;
    });
    _anim.value = from;
    unawaited(
      _anim
          .animateTo(0, duration: _snap, curve: Curves.easeOutCubic)
          .whenComplete(() {
            if (!mounted) return;
            setState(() {
              _settling = false;
              _anim.value = 0;
            });
          }),
    );
  }

  // -------------------------------------------------------------------------
  //  拖动（手势 / 外部 controller 共用）
  // -------------------------------------------------------------------------

  void _beginDrag() {
    if (_transitioning || _waiting || _dragging) return;
    _anim.stop();
    _dragging = true;
    if (_settling) {
      _dragPx = _anim.value * _width;
      _settling = false;
    }
  }

  void _dragBy(double dx) {
    if (_transitioning || !_dragging) return;
    setState(() {
      _dragPx += dx;
      if (_width <= 0) return;
      final atFirst = _shownPage <= 1;
      final atLast = _shownPage >= widget.pages;
      // 到头的方向给一点阻尼，让人知道"已经没有了"，而不是拖出大片空白
      if (atFirst && _dragPx > 0) {
        _dragPx = _dragPx.clamp(0.0, _width * 0.14).toDouble();
      } else if (atLast && _dragPx < 0) {
        _dragPx = _dragPx.clamp(-_width * 0.14, 0.0).toDouble();
      }
    });
  }

  void _onDragStart(DragStartDetails details) => _beginDrag();

  void _onDragUpdate(DragUpdateDetails details) =>
      _dragBy(details.primaryDelta ?? 0);

  void _onDragEnd(DragEndDetails details) =>
      _endDrag(details.primaryVelocity ?? 0);

  void _onDragCancel() => _cancelDrag();

  void _endDrag(double velocity) {
    if (_transitioning || !_dragging) return;
    _dragging = false;
    final dx = _dragPx;
    final fast = velocity.abs() > 520 && dx.abs() > 12;
    final farEnough = dx.abs() >= _width * 0.18;
    final dir = dx < 0 ? 1 : -1;
    final target = _shownPage + dir;
    final canGo = target >= 1 && target <= widget.pages;
    if ((!farEnough && !fast) || !canGo) {
      _snapBack();
      return;
    }

    // **不**把 _dragPx 归零：数据没到之前，旧页停在松手的位置，新页露出多少
    // 就露多少；数据到了 _startTransitionTo 会从那里接着滑。
    widget.onPageChanged(target);
    if (_pages[target] != null) {
      _startTransitionTo(target);
    } else {
      setState(() {}); // 让顶部细进度条出现
    }
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      // 宿主没接这次换页（用例 / 只读视图）：回到原位，别卡在半路。
      if (!_transitioning && !_dragging && widget.page != target) {
        _snapBack();
      }
    });
  }

  void _cancelDrag() {
    if (!_dragging) return;
    _dragging = false;
    _snapBack();
  }

  // -------------------------------------------------------------------------
  //  构建
  // -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    if (widget.pages <= 1) {
      return _pages[_shownPage] ??
          widget.builder(_shownPage) ??
          const SizedBox.shrink();
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        _width = constraints.maxWidth;
        final current = _pages[_shownPage];
        if (current == null) return const SizedBox.shrink();
        final primary = PrimaryScrollController.maybeOf(context);
        final pages = _pages.keys.toList()..sort();

        // **所有缓存的页都留在树里**（当前页可见，相邻页 `Offstage`）。
        // 相邻页在静止时就已经建好、布局好，翻页那一帧只移动图层 —— 否则
        // 目标页的整张网格 / 列表会在动画第一帧才 build，直接吃掉几十毫秒。
        final body = ClipRect(
          child: Stack(
            fit: StackFit.expand,
            children: [
              for (final page in pages) _pageLayer(page, primary),
              if (_waiting)
                const Positioned(
                  top: 0,
                  left: 0,
                  right: 0,
                  child: LinearProgressIndicator(minHeight: 2),
                ),
            ],
          ),
        );

        // 大图查看器自带原始指针拖动（`controller != null`）：别再挂一层
        // `GestureDetector`，否则同一次横向拖动会被两边各算一遍。
        // 桌面画廊则因为格子拖拽会先赢下手势，直接不挂 —— 用显式翻页按钮。
        if (widget.controller != null || !widget.swipeEnabled) return body;
        return GestureDetector(
          behavior: HitTestBehavior.deferToChild,
          onHorizontalDragStart: _onDragStart,
          onHorizontalDragUpdate: _onDragUpdate,
          onHorizontalDragEnd: _onDragEnd,
          onHorizontalDragCancel: _onDragCancel,
          child: body,
        );
      },
    );
  }

  /// 一页的图层。`GlobalKey` 让页面在 Stack 里的位置变化时保住 Element / State
  /// （翻页结束时当前页会从"右侧那一格"变成"中间那一格"）。
  Widget _pageLayer(int page, ScrollController? primary) {
    final child = _pages[page];
    if (child == null) return const SizedBox.shrink();
    final visible = _isPageVisible(page);
    final controller = page == _shownPage ? primary : null;
    final content = controller == null
        ? PrimaryScrollController.none(child: child)
        : PrimaryScrollController(controller: controller, child: child);
    return Positioned.fill(
      child: Transform.translate(
        offset: Offset(_offsetFor(page), 0),
        // 两页同时在场时，每页各自是一个独立图层：翻页每帧只改变换矩阵，内容不重画。
        child: RepaintBoundary(
          child: Offstage(
            offstage: !visible,
            child: TickerMode(
              enabled: page == _shownPage,
              child: IgnorePointer(
                ignoring: page != _shownPage,
                child: KeyedSubtree(
                  key: _pageKey(page),
                  child: content,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  bool _isPageVisible(int page) {
    if (_transitioning) return page == _fromPage || page == _toPage;
    if (page == _shownPage) return true;
    if (!(_dragging || _settling || _dragPx != 0)) return false;
    return page == _shownPage - 1 || page == _shownPage + 1;
  }

  double _offsetFor(int page) {
    if (_transitioning) {
      if (page == _fromPage) return _transitionOffset();
      if (page == _toPage) return _transitionOffset() + _direction * _width;
      return (page - _shownPage) * _width;
    }
    return (page - _shownPage) * _width + _dragOffset();
  }

  double _dragOffset() {
    if (_width <= 0) return 0;
    if (_settling) return _anim.value * _width;
    return _dragPx;
  }

  double _transitionOffset() {
    if (_width <= 0) return 0;
    final p = _animStarted ? _anim.value : 0.0;
    final end = -_direction * _width;
    return _startOffset + (end - _startOffset) * p;
  }
}

/// 把 FAB 抬到 [PageBar] 之上（移动端底部的「新建 / 上传」会盖住翻页按钮）。
///
/// Scaffold 只把 FAB 定位在**屏幕**底部，不知道 body 里还有一条 PageBar；
/// 于是 360dp 上「上传产物」/「新建提示词」的右半边正好压在「下一页」上
/// 这里用透明 padding 把 FAB 的布局盒整体抬高，
/// 比在 PageBar 上留右内边距更稳：翻页按钮仍然居中，FAB 也不会跑到内容上面。
const double kFabPageBarLift = 56;

Widget liftFabAbovePageBar(Widget fab, {bool lift = true}) => lift
    ? Padding(
        padding: const EdgeInsets.only(bottom: kFabPageBarLift),
        child: fab,
      )
    : fab;

/// 分页条：`‹ 第 3 / 12 页 ›`，与 [SwipePageView] 配套。
///
/// 抽出来是因为画廊与预设页原本各写了一份**逐字相同**的实现；统一之后
/// "左右滑动可翻页"的提示、按钮语义、窄屏形态只有一处要维护。
/// 页数 ≤ 1 时它不占任何高度（调用方不用自己判 `pages > 1`）。
class PageBar extends StatelessWidget {
  final int page;
  final int pages;

  /// 新一页还在路上：页码先显示目标值，但按钮暂时按不动（避免连点出一串请求）。
  final bool busy;

  /// 是否显示"左右滑动翻页"的提示（桌面画廊关掉滑动手势时就不该再提示它）。
  final bool showSwipeHint;

  final ValueChanged<int> onChanged;

  const PageBar({
    super.key,
    required this.page,
    required this.pages,
    required this.onChanged,
    this.busy = false,
    this.showSwipeHint = true,
  });

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 6),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: LayoutBuilder(
        builder: (context, constraints) {
          // 桌面 / 平板：按钮带文字（"上一页 / 下一页"），一眼就能看到；
          // 360dp 上只放图标与页码，别把翻页条挤成两行。
          final wide = constraints.maxWidth >= 520;
          final canPrev = !busy && page > 1;
          final canNext = !busy && page < pages;
          final prev = wide
              ? Tooltip(
                  message: '上一页',
                  child: TextButton.icon(
                    onPressed: canPrev ? () => onChanged(page - 1) : null,
                    icon: const Icon(Icons.chevron_left, size: 18),
                    label: const Text('上一页'),
                    style: TextButton.styleFrom(
                      visualDensity: VisualDensity.compact,
                      padding: const EdgeInsets.symmetric(horizontal: 10),
                    ),
                  ),
                )
              : IconButton(
                  tooltip: '上一页',
                  visualDensity: VisualDensity.compact,
                  icon: const Icon(Icons.chevron_left),
                  onPressed: canPrev ? () => onChanged(page - 1) : null,
                );
          final next = wide
              ? Tooltip(
                  message: '下一页',
                  child: TextButton.icon(
                    onPressed: canNext ? () => onChanged(page + 1) : null,
                    icon: const Icon(Icons.chevron_right, size: 18),
                    label: const Text('下一页'),
                    style: TextButton.styleFrom(
                      visualDensity: VisualDensity.compact,
                      padding: const EdgeInsets.symmetric(horizontal: 10),
                    ),
                  ),
                )
              : IconButton(
                  tooltip: '下一页',
                  visualDensity: VisualDensity.compact,
                  icon: const Icon(Icons.chevron_right),
                  onPressed: canNext ? () => onChanged(page + 1) : null,
                );
          return Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              prev,
              Text('第 $page / $pages 页'),
              next,
              if (wide && showSwipeHint) ...[
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
