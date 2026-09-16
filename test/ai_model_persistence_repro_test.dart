// 复现：获取可用模型并加入后，退出「AI 模型与凭据」再进来，模型全部不显示。
//
// 用一个**有状态**的假后端（PUT 存、GET 读，跟真后端一样），
// 模拟真实的"退出页面 → 重新进入 → 选中 Provider"三步。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

class _StatefulBackend {
  List<Map<String, dynamic>> models = [];

  MockClient client() => MockClient((request) async {
        final path = request.url.path;
        Object body;

        if (path == '/api/ai/providers' && request.method == 'GET') {
          body = [
            {
              'id': 'local-gw',
              'displayName': '本机网关',
              'api': 'openai-completions',
              'baseURL': 'http://127.0.0.1:11434/v1',
              'credentialRef': 'LOCAL_KEY',
              'endpointTrust': 'loopback',
              'enabled': true,
              'revision': 3,
              'credential': {'configured': true, 'source': 'managed', 'writable': true},
            }
          ];
        } else if (path == '/api/ai/providers/local-gw/models') {
          if (request.method == 'PUT') {
            final sent = jsonDecode(request.body) as Map<String, dynamic>;
            models = (sent['models'] as List).cast<Map<String, dynamic>>();
          }
          body = models;
        } else {
          body = <Object>[];
        }

        return http.Response(
          jsonEncode(body),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      });
}

Future<Widget> _page(_StatefulBackend backend) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = AiApiClient(settings.baseUrl, client: backend.client());
  return ChangeNotifierProvider<SettingsStore>.value(
    value: settings,
    child: MaterialApp(home: AiProviderSettingsPage(api: api)),
  );
}

void main() {
  testWidgets('退出再进入后，已加入的模型仍然显示', (tester) async {
    tester.view.physicalSize = const Size(1400, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final backend = _StatefulBackend();

    // ---- 第一次进入：选中 Provider → 获取可用模型 → 加入 ----
    await tester.pumpWidget(await _page(backend));
    await tester.pumpAndSettle();
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();

    // 第一次进入时目录是空的
    expect(find.text('还没有模型'), findsOneWidget);

    // 直接手动加一个（省掉 discover 的交互分支）
    await tester.tap(find.text('手动添加'));
    await tester.pumpAndSettle();
    await tester.enterText(
        find.widgetWithText(TextField, '模型 ID（请求里用的那个）'), 'deepseek-chat');
    await tester.enterText(find.widgetWithText(TextField, '显示名'), 'DeepSeek Chat');
    await tester.pumpAndSettle();
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();

    expect(backend.models, hasLength(1), reason: '添加后应该已经落库');
    expect(find.text('DeepSeek Chat'), findsWidgets);

    // ---- 退出页面（重新挂载一个全新的页面，等价于 back 再进来）----
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();

    await tester.pumpWidget(await _page(backend));
    await tester.pumpAndSettle();

    // ---- 重新进入：选中同一个 Provider ----
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();

    expect(find.text('还没有模型'), findsNothing,
        reason: '后端明明有 1 个模型，界面上不该说"还没有模型"');
    expect(find.text('DeepSeek Chat'), findsWidgets);
  });
}
