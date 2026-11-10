// 设置首页入口磁贴的回归（参照 iPad 入口，不使用第二根
// 左侧导航；多列不要再出现列高不一"）。
//
// 钉住三件事：
//   1) 同一行磁贴**等高**（标题 / 说明一行还是两行都不影响）；
//   2) 换行后回到第一列，且每列宽度恒定（最后一行不满时不被拉宽）；
//   3) 窄窗口（320dp）逐张叠放，不溢出。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/settings_tile.dart';

Widget _tile(String title, String subtitle) => SettingsTile(
  icon: Icons.tune,
  title: title,
  subtitle: subtitle,
  onTap: () {},
);

Future<void> _pump(WidgetTester tester, double width, List<Widget> tiles) async {
  tester.view.physicalSize = Size(width, 1200);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: SettingsTileGrid(tiles: tiles),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('同一行等高：标题一行 / 两行不影响行高', (tester) async {
    await _pump(tester, 1000, [
      _tile('短', '一句'),
      _tile('这是一个会折成两行的很长很长的标题', '说明也会折行，再长一点看看效果'),
      _tile('中等长度', '一句说明'),
    ]);

    final cards = find.byType(Card);
    expect(cards, findsNWidgets(3));
    final a = tester.getRect(cards.at(0));
    final b = tester.getRect(cards.at(1));
    final c = tester.getRect(cards.at(2));

    expect(b.top, closeTo(a.top, 0.5), reason: '同一行顶边对齐');
    expect(b.height, closeTo(a.height, 0.5), reason: '同一行必须等高');
    expect(c.height, closeTo(a.height, 0.5));
    expect(b.left, greaterThan(a.right), reason: '并排而不是叠放');
  });

  testWidgets('最后一行不满时不被拉宽，换行回到第一列', (tester) async {
    await _pump(tester, 1000, [
      for (var i = 0; i < 5; i++) _tile('入口 $i', '一句说明'),
    ]);

    final cards = find.byType(Card);
    final first = tester.getRect(cards.at(0));
    final fourth = tester.getRect(cards.at(3));
    expect(fourth.left, closeTo(first.left, 0.5), reason: '第四张换行回第一列');
    expect(fourth.top, greaterThan(first.bottom), reason: '第四张在第二行');
    expect(fourth.width, closeTo(first.width, 0.5), reason: '每列宽度恒定');
  });

  testWidgets('320dp：磁贴逐张叠放，不溢出', (tester) async {
    await _pump(tester, 320, [
      _tile('本地服务（后端 + 数据库）', '启动、停止、日志与后端地址'),
      _tile('远程访问与配对', '二维码、已授权设备、同步密钥'),
    ]);

    expect(tester.takeException(), isNull);
    final cards = find.byType(Card);
    final first = tester.getRect(cards.at(0));
    final second = tester.getRect(cards.at(1));
    expect(second.top, greaterThan(first.bottom));
    expect(first.width, closeTo(320 - 32, 0.5));
  });
}
