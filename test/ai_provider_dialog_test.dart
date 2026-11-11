// 新建 Provider 对话框的回归：
//
//   1. **端点信任级别与凭据引用名都不再是表单项** —— 这两项由软件自动决定：
//      信任档由后端按 baseURL 推断，引用名由 Provider ID 推出来（`defaultCredentialRef`）；
//   2. 表单里**多了 API Key 一栏**：填了就跟着一起保存（不用"先建 provider、再进去填一次"）；
//   3. 推出来的引用名**永远合法**：ID 以数字开头时前缀 `P`（`2fast` → `P2FAST_API_KEY`），
//      不再出现"需手动填写引用名"的入口。

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

/// 记录新建 Provider 的请求体 + 写密钥的请求体（两者是**两条**请求：provider 行里没有密钥）。
class _Recorder {
  final List<String> providerCreates = [];
  final List<String> keyPuts = [];
}

MockClient _backend(_Recorder recorder) {
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path == '/api/ai/providers' && request.method == 'POST') {
      recorder.providerCreates.add(request.body);
      final sent = jsonDecode(request.body) as Map<String, dynamic>;
      body = {
        'id': sent['id'],
        'displayName': sent['displayName'],
        'api': sent['api'],
        'baseURL': sent['baseURL'],
        'credentialRef': sent['credentialRef'],
        // 后端在 endpointTrust 缺省时按地址推断（这里模拟它的推断结果）
        'endpointTrust': 'public',
        'enabled': true,
        'revision': 1,
        'credential': {'configured': false, 'source': 'none', 'writable': true},
      };
    } else if (path.endsWith('/credentials')) {
      if (request.method == 'PUT') recorder.keyPuts.add(request.body);
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
  return ChangeNotifierProvider<SettingsStore>.value(
    value: settings,
    child: MaterialApp(
      home: AiProviderSettingsPage(
        api: AiApiClient(settings.baseUrl, client: _backend(recorder)),
      ),
    ),
  );
}

Finder _field(String label) =>
    find.ancestor(of: find.text(label), matching: find.byType(TextField));

Future<void> _openDialog(WidgetTester tester, _Recorder recorder) async {
  tester.view.physicalSize = const Size(1400, 1200);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(await _page(recorder));
  await tester.pumpAndSettle();
  await tester.tap(find.text('新建 Provider').first);
  await tester.pumpAndSettle();
}

void main() {
  /// 引用名由纯函数推导（不在界面显示），两种 ID 都需得到合法值。
  test('凭据引用名由 ID 推出，且永远合法（数字开头补前缀）', () {
    expect(defaultCredentialRef('my-gateway'), 'MY_GATEWAY_API_KEY');
    expect(defaultCredentialRef('deepseek'), 'DEEPSEEK_API_KEY');
    expect(
      defaultCredentialRef('2fast'),
      'P2FAST_API_KEY',
      reason: 'ID 可以是 2fast，但引用名必须以字母开头 —— 不许再把这件事推给用户',
    );
    expect(isValidCredentialRef(defaultCredentialRef('2fast')), isTrue);
    expect(
      isValidCredentialRef('2FAST_API_KEY'),
      isFalse,
      reason: '这一条正是补前缀的理由',
    );
  });

  testWidgets('表单里没有端点信任级别与凭据引用名，但有 API Key 一栏', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    expect(find.text('API Key（可选）'), findsOneWidget, reason: '新建时就能填密钥');
    expect(find.text('端点信任级别'), findsNothing, reason: '信任档改由软件按地址决定，不再问用户');
    expect(find.text('凭据引用名（可选）'), findsNothing, reason: '引用名由程序按 ID 生成，不再问用户');
    // 协议下拉框保留（必须手动选择：三种协议不能推测）
    expect(find.text('OpenAI Chat Completions'), findsOneWidget);
  });

  testWidgets('填完就创建：请求体由程序补齐引用名、不带信任档，密钥单独存一次', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'my-gateway');
    await tester.enterText(_field('显示名'), '我的网关');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.enterText(_field('API Key（可选）'), 'sk-secret-value');
    await tester.pumpAndSettle();

    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();

    expect(recorder.providerCreates, hasLength(1));
    final sent =
        jsonDecode(recorder.providerCreates.single) as Map<String, dynamic>;
    expect(
      sent['credentialRef'],
      'MY_GATEWAY_API_KEY',
      reason: '引用名由程序按 ID 推出',
    );
    expect(
      sent.containsKey('endpointTrust'),
      isFalse,
      reason: '信任档**一个字节都不发**：由后端按地址推断（硬塞一档会让公网地址被拒）',
    );
    expect(
      sent.containsKey('apiKey') || sent.values.contains('sk-secret-value'),
      isFalse,
      reason: '密钥不许混进 provider 行（AIH-012：值只写不读、单独存）',
    );

    // 密钥走 CredentialStore 单独存一次（这里是桌面那条 = PUT /credentials）
    expect(recorder.keyPuts, hasLength(1));
    expect(recorder.keyPuts.single, contains('sk-secret-value'));
  });

  testWidgets('额外请求头与查询参数随 Provider 一起提交', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'opencode-gw');
    await tester.enterText(_field('显示名'), 'OpenCode');
    await tester.enterText(_field('Base URL'), 'https://opencode.ai/zen/v1');
    await tester.ensureVisible(find.text('额外请求参数（可选）'));
    await tester.tap(find.text('额外请求参数（可选）'));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const ValueKey('extra-header-name-0')),
      'x-opencode-session',
    );
    await tester.enterText(
      find.byKey(const ValueKey('extra-header-value-0')),
      'ses_{{session}}',
    );
    await tester.enterText(
      find.byKey(const ValueKey('extra-query-name-0')),
      'api-version',
    );
    await tester.enterText(
      find.byKey(const ValueKey('extra-query-value-0')),
      '2024-10-21',
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();

    final sent =
        jsonDecode(recorder.providerCreates.single) as Map<String, dynamic>;
    expect(
      sent['headers'],
      {'x-opencode-session': 'ses_{{session}}'},
      reason: '存的是模板原文：每台设备 / 每条对话发请求时才展开',
    );
    expect(sent['query'], {'api-version': '2024-10-21'});
  });

  testWidgets('额外请求头名称不合法时就地拦下，不发请求', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'my-gateway');
    await tester.enterText(_field('显示名'), '我的网关');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.ensureVisible(find.text('额外请求参数（可选）'));
    await tester.tap(find.text('额外请求参数（可选）'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('extra-header-name-0')),
      'bad header',
    );
    await tester.pumpAndSettle();

    expect(find.textContaining('名称不合法'), findsOneWidget);
    final create = tester.widget<FilledButton>(
      find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
    );
    expect(create.onPressed, isNull);
    expect(recorder.providerCreates, isEmpty);
  });

  testWidgets('额外请求头里拼错变量会被就地拦下', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'my-gateway');
    await tester.enterText(_field('显示名'), '我的网关');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.ensureVisible(find.text('额外请求参数（可选）'));
    await tester.tap(find.text('额外请求参数（可选）'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('extra-header-name-0')),
      'x-opencode-session',
    );
    await tester.enterText(
      find.byKey(const ValueKey('extra-header-value-0')),
      'ses_{{sesion}}',
    );
    await tester.pumpAndSettle();

    expect(find.textContaining('不认识的变量'), findsOneWidget);
    final create = tester.widget<FilledButton>(
      find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
    );
    expect(create.onPressed, isNull);
    expect(recorder.providerCreates, isEmpty);
  });

  testWidgets('ID 以数字开头也能直接提交（引用名由程序补前缀，不再是死角）', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), '2fast');
    await tester.enterText(_field('显示名'), '快网关');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.pumpAndSettle();

    final create = tester.widget<FilledButton>(
      find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
    );
    expect(create.onPressed, isNotNull, reason: '以前这里会禁用并要求用户手填引用名');

    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();
    final sent =
        jsonDecode(recorder.providerCreates.single) as Map<String, dynamic>;
    expect(sent['credentialRef'], 'P2FAST_API_KEY');
  });

  testWidgets('密钥留空也照样能建（稍后在详情里补）', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'my-gateway');
    await tester.enterText(_field('显示名'), '我的网关');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.pumpAndSettle();
    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();

    expect(recorder.providerCreates, hasLength(1));
    expect(recorder.keyPuts, isEmpty, reason: '留空 = 不写密钥，不该发一次空值上去');
  });

  testWidgets('非法 Provider ID 会被就地拦下（按钮禁用 + 说明原因）', (tester) async {
    final recorder = _Recorder();
    await _openDialog(tester, recorder);

    await tester.enterText(_field('Provider ID（创建后不可改）'), 'My Gateway');
    await tester.enterText(_field('显示名'), 'x');
    await tester.enterText(_field('Base URL'), 'https://api.example.com/v1');
    await tester.pumpAndSettle();

    expect(find.textContaining('只能小写字母/数字'), findsOneWidget);
    final create = tester.widget<FilledButton>(
      find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
    );
    expect(create.onPressed, isNull, reason: 'ID 不合法时不允许提交（以前是提交后静默失败）');
    expect(recorder.providerCreates, isEmpty);
  });
}
