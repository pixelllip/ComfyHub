// 「大标题 → 顶栏标题」滚动联动的回归（2026-11-05 用户的规则）：
//
//   页面顶部的大标题滚出视野时，同名标题要在顶栏里逐渐出现；滚回顶部时
//   顶栏标题消失、大标题回来。判据只有 `ScrollTitleProgress` 一个。
//
// 这条规则写在 AGENTS.md §6.3，设置家族 / 画廊 / 预设 / 标签都用同一套组件。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/scroll_linked_title.dart';

Widget _host(ScrollTitleProgress progress) {
  return MaterialApp(
    home: Scaffold(
      appBar: AppBar(
        title: FadingAppBarTitle(title: '页面标题', progress: progress),
      ),
      body: Column(
        children: [
          CollapsingPageTitle(title: '页面标题', progress: progress),
          Expanded(
            child: ScrollTitleListener(
              progress: progress,
              child: ListView.builder(
                itemCount: 40,
                itemBuilder: (context, i) =>
                    SizedBox(height: 40, child: Text('第 $i 行')),
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

void main() {
  testWidgets('顶部时只有大标题；滚动后顶栏标题出现', (tester) async {
    final progress = ScrollTitleProgress();
    addTearDown(progress.dispose);

    await tester.pumpWidget(_host(progress));
    await tester.pumpAndSettle();

    expect(progress.value, 0);
    expect(find.text('页面标题'), findsOneWidget, reason: '只有主体里的大标题');

    // 往上滚 120（> fadeDistance 56）→ 大标题完全离开
    await tester.drag(find.byType(ListView), const Offset(0, -120));
    await tester.pumpAndSettle();

    expect(progress.value, 1);
    expect(find.text('页面标题'), findsNWidgets(2), reason: '大标题还在树里（已透明），顶栏标题补上');
    // **标题块本身要收掉高度**，不能只把字淡出 —— 否则页首永远空一条。
    expect(tester.getSize(find.byType(CollapsingPageTitle)).height, 0);
  });

  testWidgets('滚回顶部：顶栏标题收回', (tester) async {
    final progress = ScrollTitleProgress();
    addTearDown(progress.dispose);

    await tester.pumpWidget(_host(progress));
    await tester.pumpAndSettle();

    await tester.drag(find.byType(ListView), const Offset(0, -160));
    await tester.pumpAndSettle();
    expect(find.text('页面标题'), findsNWidgets(2));

    await tester.drag(find.byType(ListView), const Offset(0, 200));
    await tester.pumpAndSettle();

    expect(progress.value, 0);
    expect(find.text('页面标题'), findsOneWidget, reason: '顶栏标题不该留在顶部');
  });
}
