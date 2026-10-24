import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/app.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/system_accent.dart';
import 'package:viewer/core/theme.dart';

/// 平台主题色（系统强调色）的回归。
///
/// 三件事：
///  ① Windows 的两个注册表键**字节序不同**（ABGR vs ARGB），解码不能套错；
///  ② 偏好默认"跟随系统"，并且能关掉、能记住；
///  ③ `ComfyHubApp.themeFor` 真的把强调色当种子色；关掉就回默认紫。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('Windows 两个键的字节序各自解码正确', () {
    const rgb = Color(0xFF720903);
    // 本机实测：同一个强调色在 AccentColorMenu 里是 0xFF030972（ABGR），
    // 在 ColorizationColor 里是 0xC4720903（ARGB）
    expect(SystemAccent.accentFromAbgr(0xFF030972), rgb);
    expect(SystemAccent.accentFromArgb(0xC4720903), rgb);
    expect(SystemAccent.accentFromAbgr(0), isNull, reason: '缺省值不是黑色强调色');
    expect(SystemAccent.accentFromArgb(0), isNull);
  });

  test('默认跟随系统强调色，关掉后记住', () async {
    SharedPreferences.setMockInitialValues({});
    final first = SettingsStore();
    await first.load();
    expect(first.followSystemAccent, isTrue, reason: '用户 2026-10-24 要的是"自动套用"');

    await first.setFollowSystemAccent(false);
    expect(first.followSystemAccent, isFalse);

    SharedPreferences.setMockInitialValues({
      'comfyhub.followSystemAccent': false,
    });
    final second = SettingsStore();
    await second.load();
    expect(second.followSystemAccent, isFalse);
  });

  test('主体底色：浅色是白、深色是灰（不跟强调色染色）', () {
    final light = AppTheme.build(Brightness.light).colorScheme.surface;
    final dark = AppTheme.build(Brightness.dark).colorScheme.surface;
    expect(light.computeLuminance(), greaterThan(0.9), reason: '浅色主体应该是白的');
    expect(dark.computeLuminance(), lessThan(0.1), reason: '深色主体应该是灰的');
  });

  test('主题按系统强调色取种子；关掉就回默认紫', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    const accent = Color(0xFF00A0B0);
    SystemAccent.debugSetColor(accent);
    addTearDown(() => SystemAccent.debugSetColor(null));

    final withAccent = ComfyHubApp.themeFor(settings, Brightness.dark);
    expect(
      withAccent.colorScheme.primary,
      AppTheme.build(Brightness.dark, seedColor: accent).colorScheme.primary,
      reason: '强调色要作用于主色 / 选中态',
    );
    // 背景（含侧边栏 / 底部导航）一律白 / 灰，强调色只改组件
    final neutral = AppTheme.build(Brightness.dark);
    expect(withAccent.colorScheme.surface, neutral.colorScheme.surface);
    expect(
      withAccent.colorScheme.surfaceContainerHighest,
      neutral.colorScheme.surfaceContainerHighest,
    );
    expect(
      withAccent.colorScheme.surfaceContainerHigh,
      neutral.colorScheme.surfaceContainerHigh,
    );
    expect(
      withAccent.navigationRailTheme.backgroundColor,
      neutral.colorScheme.surfaceContainerLow,
    );
    expect(
      withAccent.navigationBarTheme.backgroundColor,
      neutral.colorScheme.surfaceContainerLow,
    );
    // 卡片也走**中性** surfaceContainerLow（M3 默认）：与页面一起保持白 / 灰，
    // 不跟着强调色染色（用户 2026-10-24："全部界面统一回这种上色"）。
    expect(withAccent.cardTheme.color, isNull);
    expect(withAccent.cardTheme.color, neutral.cardTheme.color);

    await settings.setFollowSystemAccent(false);
    final fallback = ComfyHubApp.themeFor(settings, Brightness.dark);
    expect(
      fallback.colorScheme.primary,
      AppTheme.build(Brightness.dark).colorScheme.primary,
      reason: '关掉后必须回到默认紫，而不是继续用检测到的颜色',
    );
    expect(fallback.colorScheme.primary, isNot(withAccent.colorScheme.primary));
  });
}
