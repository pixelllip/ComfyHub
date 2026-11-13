// **"白底白字"的防线**（2026-10-13 加浅色主题时抓到的那一类 bug）。
//
// # 这条用例在防什么
//
// `TextStyle.color` 为**空**不等于"继承"：Flutter 到 Skia 那一段没有颜色可用时
// **按白色画**。所以只要主题里某个样式没写颜色，用它的文字就是白字：
//
//   · 深色主题下看着完全正常（白字在深底上就是对的）—— 它因此藏了很久；
//   · 一切到浅色，这些字变成**白底白字**：区块小标题、AppBar 标题、
//     未选中的 chip 文案… 而 `ListTile` / 按钮这些由 M3 默认值补了颜色的地方
//     又一切正常，现象非常像"只有小字有问题"。
//
// 两类地方会踩：① `textTheme` 里漏写颜色（以前是 `color: color`，调用点不传 = null）；
// ② 主题里**覆盖掉 M3 默认样式**的项（默认值里带着颜色，被我们整个顶掉）。
// 下面两条分别盯这两类，最后再用"实际解析出来的颜色"兜一道底
// （样式里有颜色 ≠ 组件真的用了它）。
import 'package:material_ui/material_ui.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/theme.dart';

/// 逐个样式检查"是否有颜色"。**两种主题都检查** —— 该问题只在浅色下可见，
/// 只查深色等于没查。
///
/// ⚠ 这里刻意**手写清单**而不是反射/遍历：`TextTheme` 没有"列出所有样式"的接口，
/// 而漏掉一个样式就等于留下该问题（清单本身也是"主题提供了哪些档位"的说明）。
void _expectAllStylesColored(TextTheme theme, Brightness brightness) {
  final label = brightness.name;
  final styles = <String, TextStyle?>{
    'displaySmall': theme.displaySmall,
    'headlineMedium': theme.headlineMedium,
    'headlineSmall': theme.headlineSmall,
    'titleLarge': theme.titleLarge,
    'titleMedium': theme.titleMedium,
    'titleSmall': theme.titleSmall,
    'bodyLarge': theme.bodyLarge,
    'bodyMedium': theme.bodyMedium,
    'bodySmall': theme.bodySmall,
    'labelLarge': theme.labelLarge,
    'labelMedium': theme.labelMedium,
    'labelSmall': theme.labelSmall,
  };
  styles.forEach((name, style) {
    expect(style, isNotNull, reason: '[$label] $name 不该是空的（主题必须给出这一档）');
    expect(style!.color, isNotNull, reason: '[$label] $name 没有颜色 → 会被画成白色');
  });
}

void main() {
  test('两套主题里每个文字样式都带显式颜色（否则会被画成白色）', () {
    for (final brightness in Brightness.values) {
      final theme = AppTheme.build(brightness);
      _expectAllStylesColored(theme.textTheme, brightness);
      _expectAllStylesColored(theme.primaryTextTheme, brightness);
    }
  });

  // 这一组是"我们替换了 M3 默认值"的地方：默认值里都带着 onSurface /
  // onSurfaceVariant / onSecondaryContainer，被我们覆盖掉就必须自己补回来。
  test('替掉 M3 默认值的主题项自己带颜色', () {
    for (final brightness in Brightness.values) {
      final theme = AppTheme.build(brightness);
      final label = brightness.name;

      void check(String what, TextStyle? style) => expect(
        style?.color,
        isNotNull,
        reason: '[$label] $what 覆盖了 M3 默认值，必须自己给颜色',
      );

      check('appBarTheme.titleTextStyle', theme.appBarTheme.titleTextStyle);
      check('tabBarTheme.labelStyle', theme.tabBarTheme.labelStyle);
      check(
        'tabBarTheme.unselectedLabelStyle',
        theme.tabBarTheme.unselectedLabelStyle,
      );
      check('chipTheme.labelStyle', theme.chipTheme.labelStyle);
      check(
        'chipTheme.secondaryLabelStyle',
        theme.chipTheme.secondaryLabelStyle,
      );
      check(
        'navigationRailTheme.selectedLabelTextStyle',
        theme.navigationRailTheme.selectedLabelTextStyle,
      );
      check(
        'navigationRailTheme.unselectedLabelTextStyle',
        theme.navigationRailTheme.unselectedLabelTextStyle,
      );
      // 底部导航是按状态取样式的：三态都要有颜色（禁用态以前也没管过）
      for (final states in const [
        {WidgetState.selected},
        <WidgetState>{},
        {WidgetState.disabled},
      ]) {
        check(
          'navigationBarTheme.labelTextStyle$states',
          theme.navigationBarTheme.labelTextStyle?.resolve(states),
        );
      }
    }
  });

  // 兜底那一道：**实际画出来的**颜色必须和底色相反。
  // 上面两条查的是"样式里有没有颜色"，这条查的是"文本组件最终拿到的颜色"——
  // 只有它能抓住"样式有颜色、但被别的层覆盖成 null/白"的写法。
  testWidgets('浅色主题里标题/正文实际解析成深色，深色主题里解析成浅色', (tester) async {
    for (final entry in const [
      (Brightness.light, true),
      (Brightness.dark, false),
    ]) {
      final (brightness, expectDark) = entry;
      final theme = AppTheme.build(brightness);
      await tester.pumpWidget(
        MaterialApp(
          theme: theme,
          home: Scaffold(
            body: Material(
              child: Column(
                children: [
                  // 这三档覆盖了"直接用 textTheme 当 style"的典型写法
                  Text('标题', style: theme.textTheme.titleSmall),
                  Text('正文', style: theme.textTheme.bodyMedium),
                  Text('小字', style: theme.textTheme.labelSmall),
                ],
              ),
            ),
          ),
        ),
      );

      for (final text in const ['标题', '正文', '小字']) {
        final paragraph = tester.renderObject<RenderParagraph>(find.text(text));
        final color = paragraph.text.style?.color;
        expect(color, isNotNull, reason: '[${brightness.name}] $text 没有解析出颜色');
        final dark = color!.computeLuminance() < 0.5;
        expect(
          dark,
          expectDark,
          reason:
              '[${brightness.name}] $text 解析成了 ${dark ? '深' : '浅'}色 —— '
              '与底色同色就是看不见（浅色主题必须解析成深色）',
        );
      }
    }
  });
}
