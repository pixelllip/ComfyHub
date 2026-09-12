import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// ComfyHub 主题。
///
/// ## 为什么要单独定制字体
///
/// Flutter 在 Windows 上的默认字体族是 **Segoe UI**，它**不含中文字形**，
/// 中文只能靠引擎的系统兜底去挑一个字体。这会带来两个后果：
///   1. 中英混排时字形风格不统一，中文明显偏细、发虚；
///   2. Material 3 的默认字号 / 字距是按 Roboto（拉丁字体）调的
///      （bodyMedium 14px + letterSpacing 0.25），套到方块字上会显得又小又挤。
///
/// 所以这里做三件事：
///   · 显式指定中文字体族，并按平台给一串兜底；
///   · 把字距归零、行高放到 1.5~1.6（中文需要更大的行距才不挤）；
///   · 小字号统一提高一档字重，让笔画在低分屏上也立得住。
class AppTheme {
  AppTheme._();

  /// 中文兜底字体链。按优先级排列，缺哪个会自动往后找。
  static const List<String> cjkFallback = <String>[
    'Microsoft YaHei UI', // Windows 11 默认中文 UI 字体
    'Microsoft YaHei',    // Windows 中文正文字体
    'PingFang SC',        // macOS / iOS
    'Hiragino Sans GB',   // 老版本 macOS
    'Noto Sans CJK SC',   // Linux / Android 常见
    'Source Han Sans SC', // 思源黑体
    'WenQuanYi Micro Hei',// Linux
    'SimHei',
    'sans-serif',
  ];

  /// 各平台首选字体。返回 null 表示交给系统默认（Android / iOS / Web 自带的中文都不错）。
  static String? primaryFontFamily() {
    if (kIsWeb) return null;
    try {
      if (Platform.isWindows) return 'Microsoft YaHei UI';
      if (Platform.isLinux) return 'Noto Sans CJK SC';
    } catch (_) {
      // 平台判断在个别环境会抛异常，忽略
    }
    return null;
  }

  static ThemeData build(Brightness brightness) {
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFF7C5CFF),
      brightness: brightness,
    );

    final base = ThemeData(useMaterial3: true, colorScheme: scheme);
    final family = primaryFontFamily();

    return base.copyWith(
      textTheme: _tune(base.textTheme, family),
      primaryTextTheme: _tune(base.primaryTextTheme, family),
      visualDensity: VisualDensity.standard,
      cardTheme: CardThemeData(
        elevation: 0,
        margin: EdgeInsets.zero,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: BorderSide(color: scheme.outlineVariant.withValues(alpha: 0.5)),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: scheme.surfaceContainerHighest.withValues(alpha: 0.35),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(10),
          borderSide: BorderSide.none,
        ),
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
      ),
      snackBarTheme: const SnackBarThemeData(behavior: SnackBarBehavior.floating),
      listTileTheme: const ListTileThemeData(
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(10)),
        ),
      ),
      chipTheme: base.chipTheme.copyWith(
        labelStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 13,
          fontWeight: FontWeight.w500,
          letterSpacing: 0,
        ),
      ),
      navigationRailTheme: NavigationRailThemeData(
        labelType: NavigationRailLabelType.all,
        selectedLabelTextStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0,
        ),
        unselectedLabelTextStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          fontWeight: FontWeight.w500,
          letterSpacing: 0,
          color: scheme.onSurfaceVariant,
        ),
      ),
      navigationBarTheme: NavigationBarThemeData(
        labelTextStyle: WidgetStatePropertyAll(
          TextStyle(
            fontFamily: family,
            fontFamilyFallback: cjkFallback,
            fontSize: 12.5,
            fontWeight: FontWeight.w600,
            letterSpacing: 0,
          ),
        ),
      ),
      tooltipTheme: base.tooltipTheme.copyWith(
        textStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          letterSpacing: 0,
          color: scheme.onInverseSurface,
        ),
      ),
    );
  }

  /// 把 Material 3 的默认排版改成"中文友好"版本。
  static TextTheme _tune(TextTheme base, String? family) {
    TextStyle style(
      double size,
      FontWeight weight, {
      double height = 1.55,
      Color? color,
    }) =>
        TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: size,
          fontWeight: weight,
          height: height,
          // 拉丁字体的字距放到中文上会显得散，统一归零
          letterSpacing: 0,
          color: color,
        );

    return base.copyWith(
      // 大标题：字重拉满，中文才有分量
      displaySmall: style(30, FontWeight.w700, height: 1.3),
      headlineMedium: style(24, FontWeight.w700, height: 1.3),
      headlineSmall: style(21, FontWeight.w700, height: 1.35),
      titleLarge: style(19, FontWeight.w700, height: 1.35),
      titleMedium: style(16, FontWeight.w600, height: 1.4),
      titleSmall: style(14.5, FontWeight.w600, height: 1.4),
      // 正文：字号 +0.5、行高 1.6，长时间读提示词不累
      bodyLarge: style(15.5, FontWeight.w400, height: 1.6),
      bodyMedium: style(14.5, FontWeight.w400, height: 1.6),
      bodySmall: style(13, FontWeight.w400, height: 1.55),
      // 标签 / 元信息：小字号必须加字重，否则笔画发虚
      labelLarge: style(14, FontWeight.w600, height: 1.4),
      labelMedium: style(12.5, FontWeight.w500, height: 1.4),
      labelSmall: style(12, FontWeight.w500, height: 1.4),
    );
  }
}
