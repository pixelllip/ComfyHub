// AI 模型设置页回归（AIH-012 / AIH-053）：
//
//   1. **已配置密钥的 Provider 打开后，密钥输入框必须是空的** —— 只写不读，
//      界面上只有"已配置 / 未配置 / 来源"，任何情况下都不回显值；
//   2. 新建 Provider 只提供首期三种协议，没有已淘汰的旧 /completions。

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

/// 记录 PUT 出去的请求体，用来断言"前端没有把密钥回填再发一次"。
class _Recorder {
  final List<String> puts = [];
}

MockClient _backend(_Recorder recorder) {
  return MockClient((request) async {
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
      body = [
        {
          'providerId': 'local-gw',
          'id': 'm-text',
          'displayName': '纯文本模型',
          'inputModalities': ['text', 'image'],
          'tools': true,
          'capabilitySource': 'manual',
          'enabled': true,
        }
      ];
    } else if (path.endsWith('/credentials')) {
      if (request.method == 'PUT') recorder.puts.add(request.body);
      body = {'configured': true, 'source': 'managed', 'writable': true};
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

Future<Widget> _page(_Recorder recorder) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  recorder.puts.clear();
  final api = AiApiClient(settings.baseUrl, client: _backend(recorder));
  return ChangeNotifierProvider<SettingsStore>.value(
    value: settings,
    child: MaterialApp(home: AiProviderSettingsPage(api: api)),
  );
}

void main() {
  testWidgets('已配置的密钥不会回显：输入框初始为空，且只显示来源状态', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();

    // 列表里已有 Provider，选中它
    expect(find.text('本机网关'), findsOneWidget);
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();

    // 状态：已配置（受管），没有任何地方显示密钥值
    expect(find.text('已配置'), findsOneWidget);

    // 密钥输入框必须为空 —— 这是"只写不读"的界面保证
    final fields = tester.widgetList<TextField>(find.byType(TextField)).toList();
    expect(fields, isNotEmpty);
    for (final f in fields) {
      expect(f.controller?.text ?? '', isEmpty);
    }
    expect(recorder.puts, isEmpty, reason: '没有输入就不应该发出任何写密钥请求');
  });

  testWidgets('新建 Provider 只提供首期三种协议', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();

    // 打开协议下拉
    await tester.tap(find.text('OpenAI Chat Completions'));
    await tester.pumpAndSettle();

    expect(find.text('OpenAI Responses'), findsWidgets);
    expect(find.text('Anthropic Messages'), findsWidgets);
    // 旧的文本 /completions 不在首期范围（DEC-002）
    expect(find.textContaining('/v1/completions'), findsNothing);
  });
}
