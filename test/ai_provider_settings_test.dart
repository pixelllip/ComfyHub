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

  /// 记录保存过的模型目录请求体
  final List<String> modelSaves = [];

  /// 记录新建 Provider 的请求体（用来断言没有硬塞一个"仅本机"的信任级别）
  final List<String> providerCreates = [];
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
        // 后端在 endpointTrust 缺省时按地址推断
        'endpointTrust': sent['endpointTrust'] ?? 'public',
        'enabled': true,
        'revision': 1,
        'credential': {'configured': false, 'source': 'none', 'writable': true},
      };
    } else if (path == '/api/ai/providers' && request.method == 'GET') {
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
          'credential': {
            'configured': true,
            'source': 'managed',
            'writable': true,
          },
        },
      ];
    } else if (path == '/api/ai/providers/local-gw/models') {
      if (request.method == 'PUT') recorder.modelSaves.add(request.body);
      final sent = request.method == 'PUT'
          ? jsonDecode(request.body) as Map<String, dynamic>
          : null;
      body = sent == null
          ? [
              {
                'providerId': 'local-gw',
                'id': 'm-text',
                'displayName': '纯文本模型',
                'inputModalities': ['text', 'image'],
                'tools': true,
                'capabilitySource': 'manual',
                'enabled': true,
              },
            ]
          : (sent['models'] as List);
    } else if (path.endsWith('/discover-models')) {
      body = {
        'ok': true,
        'message': '发现 2 个候选：接口声明 1 个、内置目录 0 个、未识别 1 个',
        'candidates': [
          {
            'id': 'vision-model',
            'displayName': '视觉模型',
            'modalities': ['text', 'image'],
            'tools': true,
            'capabilitySource': 'discovered',
            'capabilityNote': '接口在下发的模型信息里声明了输入模态',
          },
          {
            'id': 'acme-mystery',
            'displayName': '神秘模型',
            'modalities': ['text'],
            'tools': false,
            'capabilitySource': 'unknown',
            'capabilityNote': '接口未声明、内置目录也没有 → 按仅文本处理，需要图片请手工勾选',
          },
        ],
      };
    } else if (path.endsWith('/credentials')) {
      if (request.method == 'PUT') recorder.puts.add(request.body);
      body = {'configured': true, 'source': 'managed', 'writable': true};
    } else if (path.endsWith('/test')) {
      body = {
        'ok': false,
        'errorCode': 'PROVIDER_UNREACHABLE',
        'message': '无法连接：ConnectException',
        'httpStatus': null,
      };
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
    final fields = tester
        .widgetList<TextField>(find.byType(TextField))
        .toList();
    expect(fields, isNotEmpty);
    for (final f in fields) {
      expect(f.controller?.text ?? '', isEmpty);
    }
    expect(recorder.puts, isEmpty, reason: '没有输入就不应该发出任何写密钥请求');
  });

  testWidgets('连接测试展示稳定错误码，且界面里不出现密钥', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('测试连接'));
    await tester.pumpAndSettle();

    // 详情面板里显示稳定错误码；同时会有一次浮动提示（两种都算"看得见"）
    expect(find.textContaining('PROVIDER_UNREACHABLE'), findsWidgets);
    expect(find.textContaining('无法连接'), findsWidgets);
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

  testWidgets('填公网地址直接创建：不硬塞「仅本机」信任级别，创建后出现在列表里', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();

    // 表单里已经没有信任档与引用名这两栏（用户要求：都由软件自动决定），
    // 模拟"填完就点创建"的真实操作。
    expect(find.text('端点信任级别'), findsNothing);
    expect(find.text('凭据引用名（可选）'), findsNothing);
    await tester.enterText(
      find.widgetWithText(TextField, 'Provider ID（创建后不可改）'),
      'deepseek-gw',
    );
    await tester.enterText(find.widgetWithText(TextField, '显示名'), 'DeepSeek');
    await tester.enterText(
      find.widgetWithText(TextField, 'Base URL'),
      'https://api.deepseek.com/v1',
    );
    // API Key 顺手填上：创建与存密钥是**两条**请求，两条都要发出去
    await tester.enterText(
      find.widgetWithText(TextField, 'API Key（可选）'),
      'sk-e2e-secret',
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();

    // 1) 请求体里**不带 endpointTrust**（由后端按地址推断），也不带密钥
    expect(recorder.providerCreates, hasLength(1));
    final sent =
        jsonDecode(recorder.providerCreates.single) as Map<String, dynamic>;
    expect(
      sent.containsKey('endpointTrust'),
      isFalse,
      reason: '信任档由软件按地址决定：不带这个键，后端就不会拿一个"仅本机"去挡公网地址',
    );
    expect(
      sent['credentialRef'],
      'DEEPSEEK_GW_API_KEY',
      reason: '引用名由程序按 ID 推出',
    );
    expect(sent['baseURL'], 'https://api.deepseek.com/v1');
    expect(
      sent.values.contains('sk-e2e-secret'),
      isFalse,
      reason: '密钥不进 provider 行（AIH-012）',
    );

    // 2) 密钥单独存一次（对话框里填了就必须落到 CredentialStore）
    expect(recorder.puts, hasLength(1));
    expect(recorder.puts.single, contains('sk-e2e-secret'));

    // 3) 创建成功后确实进了列表，并且自动选中（右侧出现详情）
    expect(find.text('DeepSeek'), findsWidgets);
    expect(find.textContaining('revision'), findsWidgets);
  });

  testWidgets('非法 Provider ID 会被就地拦下（按钮禁用 + 说明原因）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.widgetWithText(TextField, 'Provider ID（创建后不可改）'),
      'My Gateway',
    );
    await tester.enterText(find.widgetWithText(TextField, '显示名'), 'x');
    await tester.enterText(
      find.widgetWithText(TextField, 'Base URL'),
      'https://api.deepseek.com/v1',
    );
    await tester.pumpAndSettle();

    expect(find.textContaining('只能小写字母/数字'), findsOneWidget);
    final create = tester.widget<FilledButton>(
      find.ancestor(of: find.text('创建'), matching: find.byType(FilledButton)),
    );
    expect(create.onPressed, isNull, reason: 'ID 不合法时不允许提交（以前是提交后静默失败）');
    expect(recorder.providerCreates, isEmpty);
  });

  testWidgets('手动添加模型会立刻保存，不是只加在界面上', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();
    // 懒构建表头（Provider/密钥区）比视口高，模型行要拖一下才进视口 ——
    // 与 model_list_scroll_test 的同一约定。
    await tester.drag(
      find.byType(CustomScrollView).first,
      const Offset(0, -400),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('手动添加'));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.widgetWithText(TextField, '模型 ID（请求里用的那个）'),
      'deepseek-chat',
    );
    await tester.enterText(
      find.widgetWithText(TextField, '显示名'),
      'DeepSeek Chat',
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();
    await tester.drag(
      find.byType(CustomScrollView).first,
      const Offset(0, -400),
    );
    await tester.pumpAndSettle();

    // 界面里出现新模型
    expect(find.text('DeepSeek Chat'), findsWidgets);

    // 并且真的 PUT 到了后端（"点了添加却没加上"的直接回归）
    expect(recorder.modelSaves, isNotEmpty, reason: '点一次「添加」就应该落库');
    final saved = jsonDecode(recorder.modelSaves.last) as Map<String, dynamic>;
    final models = (saved['models'] as List).cast<Map<String, dynamic>>();
    expect(models.map((m) => m['id']), contains('deepseek-chat'));
    expect(
      models.map((m) => m['id']),
      contains('m-text'),
      reason: '原有模型不能被覆盖掉',
    );
  });

  testWidgets('获取可用模型：能力自动预填并标出来源，加入时按预填写库', (tester) async {
    tester.view.physicalSize = const Size(1400, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final recorder = _Recorder();
    await tester.pumpWidget(await _page(recorder));
    await tester.pumpAndSettle();
    await tester.tap(find.text('本机网关'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('获取可用模型'));
    await tester.pumpAndSettle();

    // 两个候选都带来源徽标：一个"接口声明"，一个"未识别（默认仅文本）"
    expect(find.text('接口声明'), findsOneWidget);
    expect(find.text('未识别（默认仅文本）'), findsOneWidget);
    expect(find.textContaining('接口未声明、内置目录也没有'), findsWidgets);

    await tester.tap(find.text('加入 2 个'));
    await tester.pumpAndSettle();

    // 写库时：视觉模型带 image，未识别的只有 text
    expect(recorder.modelSaves, isNotEmpty);
    final saved = jsonDecode(recorder.modelSaves.last) as Map<String, dynamic>;
    final models = (saved['models'] as List).cast<Map<String, dynamic>>();
    final vision = models.firstWhere((m) => m['id'] == 'vision-model');
    final mystery = models.firstWhere((m) => m['id'] == 'acme-mystery');

    expect(
      vision['inputModalities'],
      contains('image'),
      reason: '接口声明的图片能力要带过来',
    );
    expect(vision['capabilitySource'], 'discovered');
    expect(vision['tools'], isTrue);

    expect(mystery['inputModalities'], ['text'], reason: '未识别的模型不能凭空多出图片能力');
    expect(
      mystery['capabilitySource'],
      'manual',
      reason: '未识别＝用户接受了保守默认，标成手工声明',
    );
  });

  testWidgets('新建时填了密钥，详情立刻显示「已配置」而不是「未配置」', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();

    // 按真后端的形状搭一个假库：**存完密钥之后**，`GET /providers` 才会说 configured。
    //
    // 这正是用户 2026-10 报的 bug 的成因：`POST /providers` 的返回值里 credential 是
    // **回读那一刻**算的（那时密钥还没 PUT 过去），页面拿它当"当前选中项"就永远差一步。
    final stored = <String>[];
    final client = MockClient((request) async {
      final path = request.url.path;
      Object body;
      if (path == '/api/ai/providers' && request.method == 'POST') {
        final sent = jsonDecode(request.body) as Map<String, dynamic>;
        body = {
          'id': sent['id'],
          'displayName': sent['displayName'],
          'api': sent['api'],
          'baseURL': sent['baseURL'],
          'credentialRef': sent['credentialRef'],
          'endpointTrust': 'public',
          'enabled': true,
          'revision': 1,
          'credential': {
            'configured': false,
            'source': 'none',
            'writable': true,
          },
        };
      } else if (path == '/api/ai/providers' && request.method == 'GET') {
        body = [
          {
            'id': 'deepseek',
            'displayName': 'DeepSeek',
            'api': 'openai-completions',
            'baseURL': 'https://api.deepseek.com/v1',
            'credentialRef': 'DEEPSEEK_API_KEY',
            'endpointTrust': 'public',
            'enabled': true,
            'revision': 1,
            'credential': {
              'configured': stored.isNotEmpty,
              'source': stored.isEmpty ? 'none' : 'managed',
              'writable': true,
            },
          },
        ];
      } else if (path.endsWith('/credentials') && request.method == 'PUT') {
        stored.add(request.body);
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

    await tester.pumpWidget(
      ChangeNotifierProvider<SettingsStore>.value(
        value: settings,
        child: MaterialApp(
          home: AiProviderSettingsPage(
            api: AiApiClient(settings.baseUrl, client: client),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.widgetWithText(TextField, 'Provider ID（创建后不可改）'),
      'deepseek',
    );
    await tester.enterText(find.widgetWithText(TextField, '显示名'), 'DeepSeek');
    await tester.enterText(
      find.widgetWithText(TextField, 'Base URL'),
      'https://api.deepseek.com/v1',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'API Key（可选）'),
      'sk-demo-123',
    );
    await tester.tap(find.widgetWithText(FilledButton, '创建'));
    await tester.pumpAndSettle();

    expect(stored, hasLength(1), reason: '对话框里填的密钥必须真的存下去');

    // 密钥**确实已经存进去**了（真源在后端那份凭据库里），界面上不许还写"未配置"
    expect(find.text('未配置'), findsNothing);
    expect(find.text('已配置'), findsOneWidget);
  });
}
