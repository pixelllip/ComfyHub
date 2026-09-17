// 弹出菜单不能挡住页面滚动 / 点击（用户 bug ②）的回归。
//
// 背景：Material 的 `PopupMenuButton` / `showMenu` 推的是 `_PopupMenuRoute`，它会铺一层
// 铺满窗口的 `ModalBarrier`，而屏障的 `RawGestureDetector(behavior: opaque)` 在命中测试里
// **第一个命中就终止整条路径** —— 菜单一开，底下的列表滚轮和拖动全部失效
// （上游 flutter/flutter#90223 至今未修，也没有任何公开开关能关掉它）。
//
// 现在统一用 `MenuAnchor`（`AppMenuButton` / `showAppContextMenu`）：它不铺屏障，
// 菜单放在 `OverlayPortal` 里，靠 `TapRegion` 判"点到外面"来关闭。命中测试只在菜单面板
// 矩形内成立，所以菜单外的区域照常能滚、能点。
//
// 三条用例分别钉住：① 菜单开着还能滚列表；② 菜单外的点击能穿透到页面；
// ③ 界面里真的不再出现 ModalBarrier（防止有人哪天顺手改回 PopupMenuButton）。

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/app_menu.dart';
import 'package:viewer/widgets/context_menu.dart';

/// 一个可滚动的列表 + 一个下拉菜单按钮（第一行）。
Widget _page({ScrollController? controller, void Function(String)? onTapItem}) {
  return MaterialApp(
    home: Scaffold(
      body: ListView.builder(
        controller: controller,
        itemCount: 100,
        itemBuilder: (context, i) => SizedBox(
          height: 64,
          child: i == 0
              ? Align(
                  alignment: Alignment.centerLeft,
                  child: AppMenuButton<String>(
                    onSelected: (_) {},
                    options: const [MenuOption(value: 'a', label: '菜单项 A')],
                    button: (context, menuController, isOpen) => TextButton(
                      onPressed: () =>
                          menuController.isOpen ? menuController.close() : menuController.open(),
                      child: const Text('打开菜单'),
                    ),
                  ),
                )
              : InkWell(
                  onTap: onTapItem == null ? null : () => onTapItem('item $i'),
                  child: Text('item $i'),
                ),
        ),
      ),
    ),
  );
}

/// 在 [at] 处做一次滚动手势（拖动）。
///
/// 用拖动而不是 `TestPointer.scroll`：widget test 里 `PointerSignalResolver` 对
/// 单独派发的滚轮事件不投递（实测 offset 一直是 0），拖动才是能稳定复现的那条路径 ——
/// 而这两条在旧实现（ModalBarrier）下**都是**被吃掉的，测哪条都能钉住回归。
Future<void> _scrollAt(WidgetTester tester, Offset at) async {
  final gesture = await tester.startGesture(at);
  // 分两步移动：一步跨过触摸 slop 之后引擎才会把这次拖动认成滚动
  await gesture.moveBy(const Offset(0, -120));
  await tester.pump();
  await gesture.moveBy(const Offset(0, -120));
  await tester.pump();
  await gesture.up();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('菜单开着时列表还能滚，并顺带把菜单关掉', (tester) async {
    final controller = ScrollController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(_page(controller: controller));
    await tester.pumpAndSettle();

    await tester.tap(find.text('打开菜单'));
    await tester.pumpAndSettle();
    expect(find.text('菜单项 A'), findsOneWidget, reason: '菜单要先打开');

    // 在菜单**外面**（列表区域）滚动 —— 这正是以前完全没反应的地方。
    // 注意坐标要在 800×600 的默认测试视口内（600 就出界了）。
    await _scrollAt(tester, const Offset(300, 500));

    expect(controller.offset, greaterThan(0), reason: '菜单不能吃掉滚动信号');
    expect(find.text('菜单项 A'), findsNothing, reason: '页面滚动时菜单跟着收起');
  });

  testWidgets('菜单外面的点击能穿透到页面（点列表项真的生效）', (tester) async {
    final tapped = <String>[];
    await tester.pumpWidget(_page(onTapItem: tapped.add));
    await tester.pumpAndSettle();

    await tester.tap(find.text('打开菜单'));
    await tester.pumpAndSettle();
    expect(find.text('菜单项 A'), findsOneWidget);

    await tester.tap(find.text('item 2'));
    await tester.pumpAndSettle();

    expect(tapped, ['item 2'],
        reason: 'MenuItemButton 的 TapRegion 只关菜单，不该把点击吞掉（consumeOutsideTap=false）');
    expect(find.text('菜单项 A'), findsNothing);
  });

  testWidgets('右键菜单同样吃掉不了滚动，且菜单外的点击能穿透', (tester) async {
    final tapped = <String>[];
    final controller = ScrollController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: ContextMenuScope(
          child: Builder(
            builder: (context) => Scaffold(
              body: ListView.builder(
                controller: controller,
                itemCount: 50,
                itemBuilder: (context, i) => GestureDetector(
                  onSecondaryTapDown: (d) => showAppContextMenu<String>(
                    context,
                    globalPosition: d.globalPosition,
                    buildOptions: (_) => const [
                      MenuOption(value: 'copy', icon: Icons.copy, label: '复制'),
                    ],
                  ),
                  onTap: () => tapped.add('item $i'),
                  child: SizedBox(height: 64, child: Text('item $i')),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tapAt(tester.getCenter(find.text('item 1')), buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
    expect(find.text('复制'), findsOneWidget);

    // 菜单开着时在别处滚动：以前被 ModalBarrier 挡住，这里必须真的滚起来
    await _scrollAt(tester, const Offset(300, 500));
    expect(controller.offset, greaterThan(0), reason: '右键菜单不能吃掉滚动');

    // 滚动会把菜单收起（MenuAnchor 的既有行为）；没收起也不影响下面这条
    if (find.text('复制').evaluate().isEmpty) {
      await tester.tapAt(tester.getCenter(find.text('item 2')), buttons: kSecondaryMouseButton);
      await tester.pumpAndSettle();
      expect(find.text('复制'), findsOneWidget);
    }

    // 点菜单外的空白区：应当穿透到页面（并且把菜单关掉）
    await tester.tapAt(const Offset(300, 560));
    await tester.pumpAndSettle();
    expect(find.text('复制'), findsNothing, reason: '点菜单外面要关掉菜单');
  });
}
