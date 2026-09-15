// 界面语言的回归测试。
//
// 文本框右键菜单里的「复制 / 全选」不是 App 自己写的文案，而是
// MaterialLocalizations 提供的：不接 flutter_localizations、不指定 zh_CN，
// 界面主体是中文、这些菜单却是英文的 "Copy / Select all"。
// 这里直接查 ComfyHubApp 真正装上的那套本地化。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/app.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';

void main() {
  testWidgets('App 装上中文 Material 本地化：选择菜单是「复制 / 全选」而不是 Copy / Select all',
      (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 关掉"启动时自动拉起服务"，免得测试真去连本机 MySQL / 后端
    SharedPreferences.setMockInitialValues({'comfyhub.autoStartBackend': false});
    final settings = SettingsStore();
    await settings.load();
    final launcher = BackendLauncher(settings);
    addTearDown(launcher.dispose);

    await tester.pumpWidget(ComfyHubApp(settings: settings, launcher: launcher));
    await tester.pumpAndSettle();

    // 主界面现在落在 AI 工作台（AIH-001），中文本地化断言与落地页无关
    expect(find.byType(AiHomePage), findsOneWidget);

    // 系统级菜单文案来自 MaterialLocalizations
    final context = tester.element(find.byType(AiHomePage));
    final l10n = MaterialLocalizations.of(context);
    expect(l10n.copyButtonLabel, '复制');
    expect(l10n.selectAllButtonLabel, '全选');
    expect(l10n.cutButtonLabel, '剪切');
    expect(l10n.pasteButtonLabel, '粘贴');
  });
}
