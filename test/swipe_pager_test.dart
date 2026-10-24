import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/paged_swipe.dart';

/// 分页交互的回归（用户 2026-10-24 定的统一交互）：
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

    await g.up();
    await tester.pumpAndSettle();
    expect(changes, [2]);
    expect(find.text('page 2'), findsOneWidget);

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
}

Widget _host({
  required List<int> changes,
  required int pages,
  int initialPage = 1,
}) => MaterialApp(
  home: Scaffold(
    body: _Host(changes: changes, pages: pages, initialPage: initialPage),
  ),
);

class _Host extends StatefulWidget {
  final List<int> changes;
  final int pages;
  final int initialPage;

  const _Host({
    required this.changes,
    required this.pages,
    required this.initialPage,
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
            onPageChanged: _go,
            child: ColoredBox(
              color: Colors.blueGrey,
              child: Center(child: Text('page $_page')),
            ),
          ),
        ),
        PageBar(page: _page, pages: widget.pages, onChanged: _go),
      ],
    );
  }
}
