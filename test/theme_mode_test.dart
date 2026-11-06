// 界面外观（深色 / 浅色 / 跟随系统）的回归。
//
// 这条链路有三段，任何一段断了用户看到的都是"点了没反应"或者"下次启动又变回去"：
//   ① 偏好：`SettingsStore` 默认深色、写盘、冷启动读回来；
//   ② 脏值：认不出的值（老版本写的 / 手改过的偏好）回默认，**不许抛** ——
//      它在 App 启动路径上，抛了就是白屏；
//   ③ 接线：`MaterialApp.themeMode` 跟着 `SettingsStore` 的通知走
//      （`ComfyHubApp` 里那个 `ListenableBuilder`）。
//
// ⚠ 真源只有一处：`SettingsStore.themeMode`。别在页面里自己存一份 ——
// 那样设置页的 chip 和真正生效的主题会各说各话。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/app.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/theme.dart';
import 'package:viewer/pages/ai_home_page.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('默认深色；选了浅色会落盘，冷启动读得回来', () async {
    SharedPreferences.setMockInitialValues({});

    final first = SettingsStore();
    await first.load();
    expect(first.themeMode, AppThemeMode.dark, reason: '默认必须是深色：它是存量用户原有的外观');

    await first.setThemeMode(AppThemeMode.light);
    expect(
      (await SharedPreferences.getInstance()).getString('comfyhub.themeMode'),
      'light',
      reason: '真源要落在偏好里，否则关掉 App 就丢了',
    );

    // 模拟下次冷启动
    final second = SettingsStore();
    await second.load();
    expect(second.themeMode, AppThemeMode.light);
  });

  test('偏好里是认不出的值 → 回默认，不抛（启动路径上不许崩）', () async {
    SharedPreferences.setMockInitialValues({
      'comfyhub.themeMode': 'darkmode-v2',
    });

    final settings = SettingsStore();
    await settings.load();
    expect(settings.themeMode, AppThemeMode.dark);
  });

  // 设置页点一下 chip 就要立刻换肤 —— 钉住的是 `ComfyHubApp` 那层 ListenableBuilder：
  // 少了它，`settings` 变了但 `MaterialApp` 不重建，用户看到的是"点了没反应"。
  testWidgets('切换外观后整个界面立刻跟着变（不需要重启 App）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 关掉"启动时自动拉起服务"，免得测试真去连本机后端
    SharedPreferences.setMockInitialValues({
      'comfyhub.autoStartBackend': false,
      'comfyhub.themeMode': 'light',
    });
    final settings = SettingsStore();
    await settings.load();
    final launcher = BackendLauncher(settings);
    addTearDown(launcher.dispose);

    await tester.pumpWidget(
      ComfyHubApp(settings: settings, launcher: launcher),
    );
    await tester.pumpAndSettle();

    // 落地页是 AI 工作台（AIH-001）；这里只借它拿一个"在 App 里面"的 context
    final context = tester.element(find.byType(AiHomePage));
    expect(
      Theme.of(context).brightness,
      Brightness.light,
      reason: '偏好是浅色，就该装浅色主题',
    );

    await settings.setThemeMode(AppThemeMode.dark);
    await tester.pumpAndSettle();
    expect(
      Theme.of(tester.element(find.byType(AiHomePage))).brightness,
      Brightness.dark,
      reason: '改完偏好要立刻生效（ListenableBuilder 那一层）',
    );
  });
}
