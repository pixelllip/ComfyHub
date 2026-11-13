// 自适应多列布局的回归测试。
//
// 需求是「单列列表（提示词 / 标签 / 设置页）的列数 = 窗口宽度 / 550」，
// 这里钉住算列的规则，以及宽窗口下确实排成了多列、窄窗口退回单列。

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/adaptive_layout.dart';

Widget _box(BuildContext context, int i) =>
    Container(key: ValueKey('box$i'), height: 40, color: Colors.red);

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

  testWidgets('AdaptiveColumns：两列按连续切分（左列前几块、右列剩下的），块不会被拆开', (tester) async {
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
    final r1 = tester.getRect(find.byKey(const ValueKey('section1')));
    final r2 = tester.getRect(find.byKey(const ValueKey('section2')));
    final r3 = tester.getRect(find.byKey(const ValueKey('section3')));

    // 两列改成"连续切分"：先枚举切点取两列最接近的那个（400+200 vs 260+120），
    // 所以左列是前两块、右列是后两块 —— 阅读顺序仍然是左列从上到下再接右列。
    expect(r1.left, closeTo(r0.left, 0.5), reason: '第 2 块留在左列');
    expect(r1.top, greaterThan(r0.bottom + 1));
    expect(r2.left, closeTo(r0.right + 20, 1), reason: '第 3 块是右列的起点');
    expect(r2.top, closeTo(r0.top, 0.5));
    expect(r3.left, closeTo(r2.left, 0.5));
    expect(r3.top, greaterThan(r2.bottom + 1));
  });

  testWidgets('AdaptiveColumns：一块高、两块矮时，把两块矮的放同一列（左列略长）', (tester) async {
    // ComfyUI 自动捕获页的真实比例：捕获配置 400、地址与目录 360、轮询与操作 820。
    // 逐块贪心会把"地址与目录"塞进右列、再把轮询也堆上去（右列 ≈ 两倍长）；
    // 连续切分应该选 400+360 vs 820 —— 两列几乎一样长，且左列略长。
    tester.view.physicalSize = const Size(1200, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    const heights = [400.0, 360.0, 820.0];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AdaptiveColumns(
            sections: [
              for (var i = 0; i < heights.length; i++)
                AdaptiveSection(
                  Container(
                    key: ValueKey('case$i'),
                    height: heights[i],
                    color: Colors.blue,
                  ),
                  estimatedHeight: heights[i],
                ),
            ],
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final r0 = tester.getRect(find.byKey(const ValueKey('case0')));
    final r1 = tester.getRect(find.byKey(const ValueKey('case1')));
    final r2 = tester.getRect(find.byKey(const ValueKey('case2')));

    expect(r1.left, closeTo(r0.left, 0.5), reason: '前两块在左列');
    expect(r1.top, greaterThan(r0.bottom + 1));
    expect(r2.left, closeTo(r0.right + 20, 1), reason: '高块独占右列');
    expect(r2.top, closeTo(r0.top, 0.5));
    // 左列：400 + 360 + 间距 ≈ 784；右列：820 —— 左列略短但整体相当（差 < 60）
    final leftHeight = r1.bottom - r0.top;
    final rightHeight = r2.bottom - r2.top;
    expect((leftHeight - rightHeight).abs(), lessThan(60));
  });
}
