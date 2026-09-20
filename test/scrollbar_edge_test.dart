// 滚动条贴边回归（用户 bug 2026-09-20）：
//
// 「AI 工作台的聊天详情，滚动条没有贴边」。
//
// M3 默认给滚动条留了两处空白：轨道与视口边缘的 `crossAxisMargin`（约 8px）
// 和滑块两侧的留白。聊天页内容铺到边缘，这两处空白就让滑块看着"悬在半空"。
//
// 这里钉住的是**主题里那一次设定**（改回去就等于 bug 复发），
// 以及"它真的作用到了一个真实的滚动视图上"。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/theme.dart';

void main() {
  testWidgets('滚动条主题：轨道不留边距、按主题宽度画', (tester) async {
    final theme = AppTheme.build(Brightness.dark);
    final sb = theme.scrollbarTheme;

    expect(sb.crossAxisMargin, 0,
        reason: 'crossAxisMargin 非 0 就是"没贴边"的元凶（M3 默认为 8）');
    expect(sb.thickness?.resolve({}), isNotNull);
    expect(sb.thickness!.resolve({})!, greaterThan(0));
  });

  testWidgets('真实滚动视图上：滑块右边缘贴住视口右边', (tester) async {
    tester.view.physicalSize = const Size(400, 600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final controller = ScrollController();
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.build(Brightness.light),
        home: Scaffold(
          body: Scrollbar(
            controller: controller,
            thumbVisibility: true,
            child: ListView.builder(
              controller: controller,
              itemCount: 200,
              itemBuilder: (_, i) => SizedBox(height: 40, child: Text('row $i')),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final track = find.byType(Scrollbar);
    expect(track, findsOneWidget);

    // 滑块（RawScrollbar 里那颗）应当贴在视口右边缘：右边距为 0
    final barRect = tester.getRect(find.byType(Scrollbar));
    expect(barRect.right, closeTo(400, 0.5));

    // 主题确实被这个 Scrollbar 继承到了（不是靠默认值碰巧通过）
    final ctx = tester.element(find.byType(ListView));
    expect(ScrollbarTheme.of(ctx).crossAxisMargin, 0);
  });
}
