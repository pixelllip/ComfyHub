// 新建 Provider 对话框的回归：
//
//   凭据引用名由程序**自动生成**：Provider ID 全大写、`-` 换成 `_`、末尾加 `_API_KEY`
//   （后端要求环境变量风格）。它跟着 ID 实时变，只有用户主动改过才停。
//   界面上它是可选项（正常不用手填），提交时若还空着就自动补上。

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
  testWidgets('凭据引用名默认 = Provider ID 全大写 + - 换 _ + 末尾 _API_KEY', (tester) async {
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
      of: find.text('凭据引用名（可选）'),
      matching: find.byType(TextField),
    );
    expect(refField, findsOneWidget, reason: '它是可选项，不该写成必填');

    await tester.enterText(idField, 'my-gateway');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'MY_GATEWAY_API_KEY');

    // 继续改 ID：还没手动碰过引用名，应该继续跟着变
    await tester.enterText(idField, 'deepseek');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'DEEPSEEK_API_KEY');

    // 用户手动改了引用名 → 之后 ID 再变也不覆盖
    await tester.enterText(refField, 'MY_CUSTOM_KEY');
    await tester.pumpAndSettle();
    await tester.enterText(idField, 'qwen');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, 'MY_CUSTOM_KEY',
        reason: '用户自己填过就别再自动覆盖');
  });

  testWidgets('引用名留空也能提交（程序会补默认值）；ID 以数字开头时默认值非法会被挡住', (tester) async {
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
      of: find.text('凭据引用名（可选）'),
      matching: find.byType(TextField),
    );

    FilledButton createButton() => tester.widget<FilledButton>(
          find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
        );

    await tester.enterText(idField, 'my-gateway');
    await tester.enterText(nameField, '我的网关');
    await tester.enterText(urlField, 'https://api.example.com/v1');
    await tester.pumpAndSettle();
    expect(createButton().onPressed, isNotNull, reason: '默认值自动生成，正常情况直接能提交');

    // 手动清空也不拦着：提交时会补回默认值
    await tester.enterText(refField, '');
    await tester.pumpAndSettle();
    expect(createButton().onPressed, isNotNull, reason: '留空 = 用默认值，不算错');

    // `2fast` 是合法 kebab ID，但全大写 `2FAST_API_KEY` 不是合法引用名（必须以字母开头）
    await tester.enterText(idField, '2fast');
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(refField).controller!.text, '2FAST_API_KEY');
    expect(createButton().onPressed, isNull, reason: '推不出合法的引用名时要让用户自己填');

    // 手动填一个合法的就能提交
    await tester.enterText(refField, 'FAST_TWO_API_KEY');
    await tester.pumpAndSettle();
    expect(createButton().onPressed, isNotNull);
  });
}
