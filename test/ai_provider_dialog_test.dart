// 新建 Provider 对话框的回归：
//
//   凭据引用名**实际是必填的**（没它就存不了 API Key），所以默认值直接由
//   Provider ID 推出来：全大写，`-` 换成 `_`（后端要求环境变量风格）。
//   用户手动改过之后，就不该再被 ID 覆盖。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

/// 只用来让页面能加载起来；新建对话框本身不发请求。
MockClient _emptyBackend() => MockClient((request) async {
      return http.Response(
        '[]',
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });

void main() {
  testWidgets('凭据引用名默认跟随 Provider ID 全大写（- 换成 _）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();

    await tester.pumpWidget(
      ChangeNotifierProvider<SettingsStore>.value(
        value: settings,
        child: MaterialApp(
          home: AiProviderSettingsPage(
            api: AiApiClient(settings.baseUrl, client: _emptyBackend()),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();

    final idField = find.ancestor(
      of: find.text('Provider ID（创建后不可改）'),
      matching: find.byType(TextField),
    );
    final refField = find.ancestor(
      of: find.text('凭据引用名（必填）'),
      matching: find.byType(TextField),
    );
    expect(refField, findsOneWidget);

    await tester.enterText(idField, 'my-gateway');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'MY_GATEWAY');

    // 继续改 ID：还没手动碰过引用名，应该继续跟着变
    await tester.enterText(idField, 'deepseek');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'DEEPSEEK');

    // 用户手动改了引用名 → 之后 ID 再变也不覆盖
    await tester.enterText(refField, 'MY_CUSTOM_KEY');
    await tester.pumpAndSettle();
    await tester.enterText(idField, 'qwen');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'MY_CUSTOM_KEY',
        reason: '用户自己填过就别再自动覆盖');
  });

  testWidgets('引用名空着不能提交；ID 以数字开头时默认值非法也会被挡住', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();

    await tester.pumpWidget(
      ChangeNotifierProvider<SettingsStore>.value(
        value: settings,
        child: MaterialApp(
          home: AiProviderSettingsPage(
            api: AiApiClient(settings.baseUrl, client: _emptyBackend()),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();

    final idField = find.ancestor(
      of: find.text('Provider ID（创建后不可改）'),
      matching: find.byType(TextField),
    );
    final nameField = find.ancestor(of: find.text('显示名'), matching: find.byType(TextField));
    final urlField = find.ancestor(of: find.text('Base URL'), matching: find.byType(TextField));
    final refField = find.ancestor(
      of: find.text('凭据引用名（必填）'),
      matching: find.byType(TextField),
    );

    FilledButton createButton() => tester.widget<FilledButton>(
          find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
        );

    // `2fast` 是合法 kebab ID，但全大写 `2FAST` 不是合法引用名（必须以字母开头）
    await tester.enterText(idField, '2fast');
    await tester.enterText(nameField, '二号网关');
    await tester.enterText(urlField, 'https://api.example.com/v1');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, '2FAST');
    expect(createButton().onPressed, isNull, reason: '推不出合法的引用名时要让用户自己填');

    // 手动填一个合法的就能提交
    await tester.enterText(refField, 'FAST_TWO');
    await tester.pumpAndSettle();
    expect(createButton().onPressed, isNotNull);

    // 清空引用名 → 又不给提交（它是必填）
    await tester.enterText(refField, '');
    await tester.pumpAndSettle();
    expect(createButton().onPressed, isNull);
    expect(find.textContaining('必填'), findsWidgets);
  });
}
