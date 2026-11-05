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

    expect(
      first.accentMode,
      AppAccentMode.full,
      reason: '2026-11-05 起默认就是 A：完整动态取色',
    );

    await first.setFollowSystemAccent(false);
    expect(first.followSystemAccent, isFalse);
    expect(first.accentMode, AppAccentMode.off);

    SharedPreferences.setMockInitialValues({
      'comfyhub.followSystemAccent': false,
    });
    final second = SettingsStore();
    await second.load();
    expect(second.followSystemAccent, isFalse);
  });

  test('三档取色：默认 A 连表面一起染色，accent 只染组件', () {
    const accent = Color(0xFF00A0B0);
    // 默认档 = full（A），用默认种子构建出来就是"默认紫的完整方案"
    final base = AppTheme.build(Brightness.light);

    final accentOnly = AppTheme.build(
      Brightness.light,
      seedColor: accent,
      accentMode: AppAccentMode.accent,
    );
    expect(
      accentOnly.colorScheme.surface,
      base.colorScheme.surface,
      reason: 'accent 档只染组件，表面仍来自默认种子',
    );
    expect(accentOnly.colorScheme.primary, isNot(base.colorScheme.primary));

    // 不传 accentMode：默认就是 A（full）
    final full = AppTheme.build(Brightness.light, seedColor: accent);
    expect(
      full.colorScheme.surface,
      isNot(base.colorScheme.surface),
      reason: 'A 档：表面也由系统强调色派生（会带一层很淡的同色相）',
    );
    expect(
      full.colorScheme.surfaceContainerHighest,
      isNot(base.colorScheme.surfaceContainerHighest),
    );

    final off = AppTheme.build(
      Brightness.light,
      seedColor: accent,
      accentMode: AppAccentMode.off,
    );
    expect(off.colorScheme.primary, base.colorScheme.primary,
        reason: 'off 档不读系统颜色');
    expect(off.colorScheme.surface, base.colorScheme.surface);
  });

  test('形状 / 语义色令牌已经注册', () {
    final theme = AppTheme.build(Brightness.dark);
    expect(theme.extension<AppShapes>(), isNotNull);
    expect(theme.extension<AppColors>(), isNotNull);
    final light = AppTheme.build(Brightness.light);
    expect(theme.extension<AppColors>()!.success,
        isNot(light.extension<AppColors>()!.success));
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
    final seeded = AppTheme.build(Brightness.dark, seedColor: accent);
    expect(
      withAccent.colorScheme.primary,
      seeded.colorScheme.primary,
      reason: '强调色要作用于主色 / 选中态',
    );
    // 默认档是 A：表面（含侧栏 / 卡片的底色）也由系统强调色派生
    expect(withAccent.colorScheme.surface, seeded.colorScheme.surface);
    expect(
      withAccent.colorScheme.surfaceContainerHighest,
      seeded.colorScheme.surfaceContainerHighest,
    );
    expect(
      withAccent.navigationRailTheme.backgroundColor,
      seeded.colorScheme.surfaceContainerLow,
    );
    expect(
      withAccent.navigationBarTheme.backgroundColor,
      seeded.colorScheme.surfaceContainerLow,
    );
    // 填充卡：颜色来自 surfaceContainerHigh 角色，跟着 A 档一起染色
    expect(
      withAccent.cardTheme.color,
      seeded.colorScheme.surfaceContainerHigh,
      reason: '卡片是"实心浅一档"的 M3 填充卡，不是描边卡',
    );

    // 显式回到"只染组件"档：表面必须回到默认种子的方案
    await settings.setAccentMode(AppAccentMode.accent);
    final neutral = AppTheme.build(Brightness.dark);
    final accentOnly = ComfyHubApp.themeFor(settings, Brightness.dark);
    expect(accentOnly.colorScheme.surface, neutral.colorScheme.surface);
    expect(
      accentOnly.colorScheme.primary,
      isNot(neutral.colorScheme.primary),
      reason: 'accent 档仍然使用系统强调色，只是不染表面',
    );

    await settings.setAccentMode(AppAccentMode.off);
    final fallback = ComfyHubApp.themeFor(settings, Brightness.dark);
    expect(
      fallback.colorScheme.primary,
      neutral.colorScheme.primary,
      reason: '关掉后必须回到默认紫，而不是继续用检测到的颜色',
    );
    expect(fallback.colorScheme.primary, isNot(withAccent.colorScheme.primary));
  });
}
