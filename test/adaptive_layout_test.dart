// 自适应多列布局的回归测试。
//
// 需求是「单列列表（提示词 / 标签 / 设置页）的列数 = 窗口宽度 / 550」，
// 这里钉住算列的规则，以及宽窗口下确实排成了多列、窄窗口退回单列。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/adaptive_layout.dart';

Widget _box(BuildContext context, int i) => Container(
      key: ValueKey('box$i'),
      height: 40,
      color: Colors.red,
    );

Future<void> _pumpList(WidgetTester tester, double width, int count) async {
  tester.view.physicalSize = Size(width, 800);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: AdaptiveColumnList(itemCount: count, itemBuilder: _box),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  test('列数 = 可用宽度 / 550', () {
    expect(adaptiveColumnCount(400), 1);
    expect(adaptiveColumnCount(549), 1);
    expect(adaptiveColumnCount(550), 1, reason: '刚好一个列宽还是单列');
    expect(adaptiveColumnCount(1099), 1);
    expect(adaptiveColumnCount(1100), 2);
    expect(adaptiveColumnCount(1649), 2);
    expect(adaptiveColumnCount(1650), 3);
    expect(adaptiveColumnCount(2200), 4);
  });

  test('窄窗口 / 异常宽度退回单列，超宽也不无限分列', () {
    expect(adaptiveColumnCount(0), 1);
    expect(adaptiveColumnCount(-100), 1);
    expect(adaptiveColumnCount(double.infinity), 1);
    expect(adaptiveColumnCount(9000), kMaxColumns);
  });

  testWidgets('AdaptiveColumnList：宽窗口一行两张卡，下一张换行且回到第一列', (tester) async {
    await _pumpList(tester, 1200, 4);

    final r0 = tester.getRect(find.byKey(const ValueKey('box0')));
    final r1 = tester.getRect(find.byKey(const ValueKey('box1')));
    final r2 = tester.getRect(find.byKey(const ValueKey('box2')));

    expect(r1.top, closeTo(r0.top, 0.5), reason: '第 1、2 张在同一行');
    expect(r1.left, greaterThan(r0.right - 1), reason: '第 2 张在第 1 张右边');
    expect(r2.top, greaterThan(r0.bottom + 1), reason: '第 3 张换行');
    expect(r2.left, closeTo(r0.left, 0.5), reason: '换行后回到第一列');
  });

  testWidgets('AdaptiveColumnList：窗口不足 1100 时退回单列', (tester) async {
    await _pumpList(tester, 900, 3);

    final r0 = tester.getRect(find.byKey(const ValueKey('box0')));
    final r1 = tester.getRect(find.byKey(const ValueKey('box1')));
    expect(r1.left, closeTo(r0.left, 0.5));
    expect(r1.top, greaterThan(r0.bottom + 1));
  });

  testWidgets('AdaptiveColumns：把块依次放进当前最矮的一列，块不会被拆开', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AdaptiveColumns(
            sections: [
              for (var i = 0; i < 4; i++)
                AdaptiveSection(
                  Container(
                    key: ValueKey('section$i'),
                    height: [400.0, 200.0, 260.0, 120.0][i],
                    color: Colors.blue,
                  ),
                  estimatedHeight: [400.0, 200.0, 260.0, 120.0][i],
                ),
            ],
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final r0 = tester.getRect(find.byKey(const ValueKey('section0')));
    final r2 = tester.getRect(find.byKey(const ValueKey('section2')));
    // 第 3 块（260 高）应该接在最矮的那列（第 2 块所在的右列）下面
    expect(r2.left, closeTo(r0.right + 20, 1));
    expect(r2.top, greaterThan(r0.top));
  });
}
