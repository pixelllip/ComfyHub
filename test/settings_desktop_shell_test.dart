// 桌面设置分栏骨架的回归（2026-11-14）：
//
//   1) 左侧分类栏只有大项主标题，点哪一项右侧就换哪一页；
//   2) 次级页面由右侧自己的 Navigator 承接 —— 推入后左侧分类栏仍在，
//      返回只退回到右侧的根页面；
//   3) 切走再切回来，那一栏的路由栈原样保留（不会把次级页弄丢）。

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/settings_desktop_shell.dart';

/// 右侧甲页：根页面 + 一个"进入次级"按钮。
Widget _rootA() => Scaffold(
  appBar: AppBar(title: const Text('甲页')),
  body: Center(
    child: Builder(
      builder: (context) => TextButton(
        onPressed: () => Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => Scaffold(
              appBar: AppBar(title: const Text('甲的次级页')),
              body: const Center(child: Text('次级内容')),
            ),
          ),
        ),
        child: const Text('进入次级'),
      ),
    ),
  ),
);

Widget _rootB() => Scaffold(
  appBar: AppBar(title: const Text('乙页')),
  body: const Center(child: Text('乙内容')),
);

Future<void> _pump(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1400, 900);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    MaterialApp(
      home: SettingsDesktopShell(
        initialId: 'a',
        searchPageBuilder: (_) => Scaffold(
          appBar: AppBar(title: const Text('搜索页')),
          body: const Center(child: Text('搜索内容')),
        ),
        items: [
          SettingsNavItem(
            id: 'a',
            label: '甲',
            icon: Icons.circle_outlined,
            builder: (_) => _rootA(),
          ),
          SettingsNavItem(
            id: 'b',
            label: '乙',
            icon: Icons.square_outlined,
            builder: (_) => _rootB(),
          ),
        ],
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('点分类换右侧页面；次级页留在右侧，左侧栏不动', (tester) async {
    await _pump(tester);

    expect(find.text('甲页'), findsOneWidget);
    expect(find.text('乙页'), findsNothing);

    // 次级页推入右侧这一格：左侧分类栏仍然在。
    await tester.tap(find.text('进入次级'));
    await tester.pumpAndSettle();
    expect(find.text('次级内容'), findsOneWidget);
    expect(find.byKey(const Key('settings-nav')), findsOneWidget);
    expect(find.text('甲'), findsOneWidget);
    expect(find.text('乙'), findsOneWidget);

    // 切到乙再切回甲：甲那栏的路由栈还在，仍停在次级页。
    await tester.tap(find.text('乙'));
    await tester.pumpAndSettle();
    expect(find.text('乙内容'), findsOneWidget);
    expect(find.text('次级内容'), findsNothing);

    await tester.tap(find.text('甲'));
    await tester.pumpAndSettle();
    expect(find.text('次级内容'), findsOneWidget);
    expect(find.text('甲页'), findsNothing);

    // 返回只退出右侧的次级页，不关掉整个设置分栏。
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('甲页'), findsOneWidget);
    expect(find.text('次级内容'), findsNothing);
    expect(find.byKey(const Key('settings-nav')), findsOneWidget);
  });

  testWidgets('搜索在当前分类的右侧打开，左侧分类栏不动', (tester) async {
    await _pump(tester);

    await tester.tap(find.text('搜索设置'));
    await tester.pumpAndSettle();

    // 搜索页铺的是右侧正文那一格，不是整个窗口。
    expect(find.text('搜索页'), findsOneWidget);
    expect(find.byKey(const Key('settings-nav')), findsOneWidget);
    expect(find.text('甲'), findsOneWidget);
    expect(find.text('乙'), findsOneWidget);

    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('搜索页'), findsNothing);
    expect(find.text('甲页'), findsOneWidget);
  });
}
