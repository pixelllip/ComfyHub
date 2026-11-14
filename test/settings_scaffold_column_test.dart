// 设置正文单列的回归（2026-11-09）：
//
// 显示列数大于等于 2 列时，设置项容易出现不同列高度不一，
// 导致视觉失衡。" —— 设置是"读 + 改"的表单，不是画廊；正文统一单列、限宽居中，
// 多列只留给桌面端左侧的分类栏（见 settings_desktop_shell_test.dart）。
//
// 这里钉住三件事：
//   1) 宽窗口下正文仍然单列，宽度不超过 kSettingsContentMaxWidth；
//   2) 窄窗口铺满可用宽度（左右各 16 的留白不变）；
//   3) 块与块之间的间距是 24，不会因为"分栏"消失而挤在一起。

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/settings_scaffold.dart';

SettingsSection _section(String title, double height) => SettingsSection(
  title: title,
  child: Card(
    child: SizedBox(height: height, child: Text('内容$title')),
  ),
);

Future<void> _pump(WidgetTester tester, double width) async {
  tester.view.physicalSize = Size(width, 1600);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    MaterialApp(
      home: SettingsScaffold(
        title: '设置',
        showBack: false,
        sections: [
          _section('一', 120),
          _section('二', 80),
          _section('三', 200),
        ],
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('宽窗口：正文单列、限宽 1680、块从上到下排', (tester) async {
    await _pump(tester, 1900);

    final cards = find.byType(Card);
    expect(cards, findsNWidgets(3));
    final first = tester.getRect(cards.at(0));
    final second = tester.getRect(cards.at(1));
    final third = tester.getRect(cards.at(2));

    expect(first.width, closeTo(kSettingsContentMaxWidth, 1));
    expect(first.left, closeTo((1900 - kSettingsContentMaxWidth) / 2, 1));
    expect(second.left, closeTo(first.left, 0.5), reason: '第二块不许换到右列');
    expect(third.left, closeTo(first.left, 0.5));
    expect(second.width, closeTo(first.width, 0.5));

    expect(second.top, greaterThan(first.top));
    expect(third.top, greaterThan(second.top));
    // 块间距 24：上一张卡的下边到下一块小标题的上边。
    expect(
      tester.getRect(find.text('二')).top - (first.top + first.height),
      closeTo(24, 1),
    );
  });

  testWidgets('窄窗口：正文铺满可用宽度（左右 16）', (tester) async {
    await _pump(tester, 600);

    final first = tester.getRect(find.byType(Card).at(0));
    expect(first.left, closeTo(16, 0.5));
    expect(first.width, closeTo(600 - 32, 0.5));
    expect(
      tester.getRect(find.byType(Card).at(1)).left,
      closeTo(first.left, 0.5),
    );
  });
}
