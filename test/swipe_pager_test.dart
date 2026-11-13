import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/paged_swipe.dart';

/// 分页交互的回归（统一交互）：
///  ① 左右滑动 = 切页（不是滚一屏），且松手后内容真的换；
///  ② 翻页按钮同样走"滑入"过渡（`didUpdateWidget` 那条路），不是硬切；
///  ③ 第一页往右 / 最后一页往左**不能**越界，也不能把页码改成 0 或页数 +1；
///  ④ 只有一页时不挂手势（别把单页列表的横向拖动吃掉）。
void main() {
  testWidgets('左滑去下一页、右滑回上一页（松手后动画结束才换）', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 3));
    await tester.pumpAndSettle();
    expect(find.text('page 1'), findsOneWidget);

    // 拖动中内容跟手：先只动一点，松手前不应该已经换页
    final g = await tester.startGesture(
      tester.getCenter(find.byType(SwipePageView)),
    );
    await g.moveBy(const Offset(-260, 0));
    await tester.pump();
    expect(find.text('page 1'), findsOneWidget, reason: '还没松手，不该换页');
    // 2026-11-07：拖动到一半时**两页同时在树里**，不是等松手才建新页。
    expect(find.text('page 2'), findsOneWidget, reason: '拖到中间应该同时露出下一页的内容');

    await g.up();
    await tester.pumpAndSettle();
    expect(changes, [2]);
    expect(find.text('page 2'), findsOneWidget);
    expect(find.text('page 1'), findsNothing, reason: '动画结束，旧页注销');

    await tester.drag(find.byType(SwipePageView), const Offset(260, 0));
    await tester.pumpAndSettle();
    expect(changes, [2, 1]);
    expect(find.text('page 1'), findsOneWidget);
  });

  testWidgets('翻页按钮也播过渡（由外部 page 变化触发）', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 3));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('下一页'));
    await tester.pumpAndSettle();
    expect(changes, [2]);
    expect(find.text('page 2'), findsOneWidget);

    await tester.tap(find.byTooltip('上一页'));
    await tester.pumpAndSettle();
    expect(changes, [2, 1]);
    expect(find.text('page 1'), findsOneWidget);
  });

  testWidgets('翻页动画按帧推进：中途位置必须真的在移动', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 3));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('下一页'));
    await tester.pump();
    await tester.pump();
    final start = tester.getTopLeft(find.text('page 2')).dx;

    await tester.pump(const Duration(milliseconds: 120));
    final mid = tester.getTopLeft(find.text('page 2')).dx;
    expect(
      mid,
      lessThan(start),
      reason: '新页要逐帧滑入；没有动画监听时它会在结束时直接跳到位',
    );

    await tester.pumpAndSettle();
    expect(find.text('page 2'), findsOneWidget);
    expect(find.text('page 1'), findsNothing);
  });

  testWidgets('静止时就预建相邻页：翻页那一帧不再建整页', (tester) async {
    _ProbeState.built.clear();
    var page = 1;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) => Column(
              children: [
                Expanded(
                  child: SwipePageView(
                    page: page,
                    pages: 3,
                    builder: (p) => _Probe(p),
                    onPageChanged: (p) => setState(() => page = p),
                  ),
                ),
                PageBar(
                  page: page,
                  pages: 3,
                  onChanged: (p) => setState(() => page = p),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(_ProbeState.built, containsAll(<int>[1, 2]), reason: '下一页要提前建好');
    expect(
      find.text('page 2', skipOffstage: false),
      findsOneWidget,
      reason: '相邻页在树里，只是 Offstage',
    );
    expect(find.text('page 2'), findsNothing, reason: '默认 finder 跳过 Offstage');

    await tester.tap(find.byTooltip('下一页'));
    await tester.pumpAndSettle();
    expect(_ProbeState.built, containsAll(<int>[1, 2, 3]), reason: '新相邻页也要预建');
    expect(find.text('page 2'), findsOneWidget);
  });

  testWidgets('第一页往右滑不越界', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 3));
    await tester.pumpAndSettle();

    await tester.drag(find.byType(SwipePageView), const Offset(420, 0));
    await tester.pumpAndSettle();
    expect(changes, isEmpty);
    expect(find.text('page 1'), findsOneWidget);
  });

  testWidgets('最后一页往左滑不越界', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 3, initialPage: 3));
    await tester.pumpAndSettle();

    await tester.drag(find.byType(SwipePageView), const Offset(-420, 0));
    await tester.pumpAndSettle();
    expect(changes, isEmpty);
    expect(find.text('page 3'), findsOneWidget);
  });

  testWidgets('只有一页时不挂手势、也不显示翻页条', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(_host(changes: changes, pages: 1));
    await tester.pumpAndSettle();

    await tester.drag(find.byType(SwipePageView), const Offset(-420, 0));
    await tester.pumpAndSettle();
    expect(changes, isEmpty);
    expect(find.byTooltip('下一页'), findsNothing);
  });

  testWidgets('关掉滑动后横向拖动不翻页，但显式按钮照常可用', (tester) async {
    final changes = <int>[];
    await tester.pumpWidget(
      _host(changes: changes, pages: 3, swipeEnabled: false),
    );
    await tester.pumpAndSettle();

    await tester.drag(find.byType(SwipePageView), const Offset(-420, 0));
    await tester.pumpAndSettle();
    expect(changes, isEmpty, reason: '桌面画廊里横向拖动是"拖格子"，不该翻页');
    expect(find.text('page 1'), findsOneWidget);

    await tester.tap(find.byTooltip('下一页'));
    await tester.pumpAndSettle();
    expect(changes, [2]);
    expect(find.text('page 2'), findsOneWidget);
  });

  // 现象："点击翻页后重新加载界面，会出现显式注销旧界面的过程"。
  // 分页接口还没回来时，旧页必须**原地留着**；回来之后才滑走。
  testWidgets('新页数据没到时停在旧页，到了才滑过去（不闪空白 / 不闪加载页）', (tester) async {
    final key = GlobalKey<_AsyncHostState>();
    await tester.pumpWidget(_asyncHost(key));
    await tester.pumpAndSettle();
    expect(find.text('page 1'), findsOneWidget);

    await tester.tap(find.byTooltip('下一页'));
    await tester.pump();

    // 目标页码已经翻到 2，但内容还是第 1 页 —— 旧内容不许被拆掉。
    expect(find.text('第 2 / 3 页'), findsOneWidget);
    expect(find.text('page 1'), findsOneWidget);
    expect(find.text('page 2'), findsNothing);
    expect(find.byType(LinearProgressIndicator), findsOneWidget);

    // 数据到位：先是"旧页 + 新页同时在"的过渡帧，然后只剩新页。
    key.currentState!.deliver();
    await tester.pump();
    expect(find.text('page 1'), findsOneWidget);
    expect(find.text('page 2'), findsOneWidget);

    await tester.pumpAndSettle();
    expect(find.text('page 1'), findsNothing);
    expect(find.text('page 2'), findsOneWidget);
    expect(find.byType(LinearProgressIndicator), findsNothing);
  });

  testWidgets('滑动请求换页后也停在旧页等数据，不先回弹再加载', (tester) async {
    final key = GlobalKey<_AsyncHostState>();
    await tester.pumpWidget(_asyncHost(key));
    await tester.pumpAndSettle();

    await tester.drag(find.byType(SwipePageView), const Offset(-300, 0));
    await tester.pump();

    expect(key.currentState!.changes, [2]);
    expect(find.text('page 1'), findsOneWidget, reason: '新页没到，旧页不许消失');
    expect(find.byType(LinearProgressIndicator), findsOneWidget);

    key.currentState!.deliver();
    await tester.pumpAndSettle();
    expect(find.text('page 2'), findsOneWidget);
  });
}

class _Probe extends StatefulWidget {
  final int page;

  const _Probe(this.page);

  @override
  State<_Probe> createState() => _ProbeState();
}

class _ProbeState extends State<_Probe> {
  static final List<int> built = <int>[];

  @override
  void initState() {
    super.initState();
    built.add(widget.page);
  }

  @override
  Widget build(BuildContext context) => Center(child: Text('page ${widget.page}'));
}

Widget _asyncHost(GlobalKey<_AsyncHostState> key) => MaterialApp(
  home: Scaffold(body: _AsyncHost(key: key, pages: 3)),
);

class _AsyncHost extends StatefulWidget {
  final int pages;

  const _AsyncHost({super.key, required this.pages});

  @override
  State<_AsyncHost> createState() => _AsyncHostState();
}

class _AsyncHostState extends State<_AsyncHost> {
  final changes = <int>[];

  /// 需要显示的页；[content] 是列表数据实际装着的页。
  int page = 1;
  int content = 1;
  final Set<int> loaded = {1};

  void go(int next) {
    changes.add(next);
    setState(() => page = next);
  }

  /// 模拟后端的这一页数据到了。
  void deliver() => setState(() {
    loaded.add(page);
    content = page;
  });

  @override
  Widget build(BuildContext context) {
    final busy = !loaded.contains(page);
    return Column(
      children: [
        Expanded(
          child: SwipePageView(
            page: page,
            pages: widget.pages,
            builder: (p) => loaded.contains(p)
                ? ColoredBox(
                    color: Colors.blueGrey,
                    child: Center(child: Text('page $p')),
                  )
                : null,
            onPageChanged: go,
          ),
        ),
        PageBar(page: page, pages: widget.pages, busy: busy, onChanged: go),
      ],
    );
  }
}

Widget _host({
  required List<int> changes,
  required int pages,
  int initialPage = 1,
  bool swipeEnabled = true,
}) => MaterialApp(
  home: Scaffold(
    body: _Host(
      changes: changes,
      pages: pages,
      initialPage: initialPage,
      swipeEnabled: swipeEnabled,
    ),
  ),
);

class _Host extends StatefulWidget {
  final List<int> changes;
  final int pages;
  final int initialPage;
  final bool swipeEnabled;

  const _Host({
    required this.changes,
    required this.pages,
    required this.initialPage,
    required this.swipeEnabled,
  });

  @override
  State<_Host> createState() => _HostState();
}

class _HostState extends State<_Host> {
  late int _page = widget.initialPage;

  void _go(int page) {
    widget.changes.add(page);
    setState(() => _page = page);
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Expanded(
          child: SwipePageView(
            page: _page,
            pages: widget.pages,
            swipeEnabled: widget.swipeEnabled,
            builder: (p) => ColoredBox(
              color: Colors.blueGrey,
              child: Center(child: Text('page $p')),
            ),
            onPageChanged: _go,
          ),
        ),
        PageBar(page: _page, pages: widget.pages, onChanged: _go),
      ],
    );
  }
}
