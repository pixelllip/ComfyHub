// 思考强度「关闭」档（用户要求：允许我关闭模型思考，加一个"关"的选项，**不影响模型声明**）。
//
// 关键约定：
//   · 「关闭」不需要模型在 thinkingEfforts 里声明 off —— 它不发任何思考参数，任何网关都成立；
//   · 真正的思考档位仍然只列模型声明过的（不猜、不降级，AIH-056）；
//   · 选「关闭」后创建 Run 的请求体里**不带** reasoningEffort 字段（后端缺省即 off）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

/// 每次创建 Run 的请求体（按顺序）。
final List<Map<String, dynamic>> runBodies = [];

String _sse(int seq, String type, String dataJson) => 'id: $seq\nevent: $type\ndata: $dataJson\n\n';

http.Response _sseResponse(String body) => http.Response.bytes(
      utf8.encode(body),
      200,
      headers: {'content-type': 'text/event-stream; charset=utf-8'},
    );

/// 假后端：模型声明了 reasoning + high/max，但**没有声明 off**。
MockClient _backend() {
  var runCount = 0;
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path == '/api/ai/providers') {
      body = [
        {
          'id': 'local-gw',
          'displayName': '本机网关',
          'api': 'openai-completions',
          'baseURL': 'http://127.0.0.1:11434/v1',
          'endpointTrust': 'loopback',
          'enabled': true,
          'revision': 1,
          'credential': {'configured': true, 'source': 'managed', 'writable': true},
        }
      ];
    } else if (path == '/api/ai/providers/local-gw/models') {
      body = [
        {
          'providerId': 'local-gw',
          'id': 'm-think',
          'displayName': '会思考的模型',
          'inputModalities': ['text'],
          'tools': true,
          'reasoning': true,
          'thinkingEfforts': {'high': 'high', 'max': 'max'},
          'thinkingFormat': 'openai',
          'capabilitySource': 'manual',
          'enabled': true,
        }
      ];
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      body = {
        'id': 'c1',
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-think',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = <Object>[];
    } else if (path == '/api/ai/conversations/c1/runs') {
      runBodies.add(jsonDecode(request.body) as Map<String, dynamic>);
      runCount++;
      body = {'runId': 'r$runCount', 'assistantMessageId': 'a$runCount', 'userMessageId': 'u1'};
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      final runId = path.split('/')[4];
      return _sseResponse(
        _sse(1, 'run.started', '{"runId":"$runId"}') +
            _sse(2, 'text.delta', '{"messageId":"a$runCount","text":"好"}') +
            _sse(3, 'message.completed', '{"messageId":"a$runCount","text":"好"}') +
            _sse(4, 'run.completed', '{"runId":"$runId"}'),
      );
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = <Object>[];
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

Future<AiWorkspaceStore> _store(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1200, 900);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: _backend()),
  );
  await tester.pumpWidget(MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  ));
  await tester.pumpAndSettle();
  return store;
}

void main() {
  setUp(() {
    runBodies.clear();
  });

  test('「关闭」永远可选且排在最前；没声明的档位不给选，不支持推理则完全不给选', () {
    const model = AiModel(
      providerId: 'p',
      id: 'm',
      displayName: 'M',
      reasoning: true,
      thinkingEfforts: {'high': 'high', 'max': 'max'},
    );
    final options = model.selectableEfforts;
    expect(options.first, AiReasoningEffort.off, reason: '关必须排在最前');
    expect(options, containsAll([AiReasoningEffort.high, AiReasoningEffort.max]));
    // 没声明的档位仍然不出现（不猜、不降级）
    expect(options, isNot(contains(AiReasoningEffort.low)));
    expect(options, isNot(contains(AiReasoningEffort.medium)));
    expect(options, isNot(contains(AiReasoningEffort.minimal)));
    expect(model.thinkingEfforts.containsKey('off'), isFalse, reason: '模型声明不需要为了"关"而改动');

    // 完全没声明推理能力的模型：一个档位都不给（后端也会拒）
    const plain = AiModel(providerId: 'p', id: 'm', displayName: 'M');
    expect(plain.selectableEfforts, isEmpty);
  });

  testWidgets('思考强度选择器里能选「关闭」，选了之后 Run 请求不带思考参数', (tester) async {
    final store = await _store(tester);
    expect(store.reasoningEffort, AiReasoningEffort.off);

    // 选「高」→ 请求体里必须带 high（证明选择器真的生效）
    await tester.tap(find.byType(PopupMenuButton<AiReasoningEffort>));
    await tester.pumpAndSettle();
    expect(find.text('高'), findsOneWidget, reason: '模型声明过的档位要列出来');
    expect(find.text('关闭'), findsWidgets, reason: '「关闭」即使没声明也要能选');
    await tester.tap(find.text('高'));
    await tester.pumpAndSettle();
    expect(store.reasoningEffort, AiReasoningEffort.high);

    await tester.enterText(find.byType(TextField), '第一问');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();
    expect(runBodies.last['reasoningEffort'], 'high');

    // 再选「关闭」→ 请求体里不带这个字段（后端缺省即 off）
    await tester.tap(find.byType(PopupMenuButton<AiReasoningEffort>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('关闭').last);
    await tester.pumpAndSettle();
    expect(store.reasoningEffort, AiReasoningEffort.off);

    await tester.enterText(find.byType(TextField), '第二问');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();
    expect(runBodies.length, 2);
    expect(
      runBodies.last.containsKey('reasoningEffort'),
      isFalse,
      reason: '关闭思考时不应该发任何思考参数',
    );
  });
}
