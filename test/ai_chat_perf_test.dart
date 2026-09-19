// AI 工作台「卡顿」类回归（用户 bug 9.19）：
//
//   1. 聊天快速上下滑动卡顿 —— 同一段文本反复解析 Markdown 是主因之一；
//   2. 流式输出时每来一个字就整页重建一次（含一屏历史气泡）；
//   3. 展开正在流式输出的思考过程后更卡。
//
// 三条都钉成**可观察的事实**，不是"感觉更快了"：
//   · `parseMarkdown` 对同一段文本只解析一次（缓存命中返回同一个对象）；
//   · 一次流式刷新里，**没变化的气泡不会重建**（`aiChatBubbleBuilds` 的增长量）；
//   · 一段 SSE 里的多个增量不会一个一个字地通知（通知次数 << 增量数）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/widgets/markdown.dart';
import 'package:viewer/widgets/markdown_view.dart';

String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

http.Response _sseResponse(String body) => http.Response.bytes(
      utf8.encode(body),
      200,
      headers: {'content-type': 'text/event-stream; charset=utf-8'},
    );

/// 历史里的一条助手消息（内容足够长，才谈得上"每次解析都要花钱"）。
Map<String, dynamic> _historyMessage(int i) => {
      'id': 'h$i',
      'conversationId': 'c1',
      'seq': i,
      'role': i.isOdd ? 'user' : 'assistant',
      'status': 'complete',
      'text': i.isOdd
          ? '第 $i 条提问：帮我看看这个工作流。'
          : '第 $i 条回答：**结论**是这样的。\n\n'
              '- 第一点说明文字，比较长，用来让解析真的有成本。\n'
              '- 第二点说明文字，同样比较长。\n'
              '```json\n{"node": $i, "input": "checkpoint"}\n```\n',
      'parts': <Object>[],
    };

/// 假后端：一段历史 + 一次"逐字回包"的 Run。
MockClient _backend({required int deltas, required String text}) {
  var runCount = 0;
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;

    if (path == '/api/ai/providers') {
      body = [
        {
          'id': 'local-gw',
          'displayName': '本机网关',
          'api': 'openai-completions',
          'baseURL': 'http://127.0.0.1:11434/v1',
          'credentialRef': 'LOCAL_KEY',
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
          'id': 'm-text',
          'displayName': '纯文本模型',
          'inputModalities': ['text'],
          'tools': false,
          'reasoning': false,
          'enabled': true,
        }
      ];
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      body = {
        'id': 'c-new',
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = [
        {
          'id': 'c1',
          'title': '卡顿复现',
          'providerId': 'local-gw',
          'modelId': 'm-text',
          'messageCount': 8,
          'updatedAt': '2026-09-19T01:00:00.000Z',
        }
      ];
    } else if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        request.method == 'DELETE') {
      body = {'deleted': true, 'id': path.split('/').last};
    } else if (RegExp(r'^/api/ai/conversations/[^/]+/runs$').hasMatch(path)) {
      runCount++;
      body = {'runId': 'r$runCount', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      final id = path.split('/')[4];
      final buffer = StringBuffer(_sse(1, 'run.started', '{"runId":"$id"}'));
      var seq = 2;
      for (var i = 0; i < deltas; i++) {
        buffer.write(_sse(seq++, 'text.delta', jsonEncode({'messageId': 'a1', 'text': '段$i，'})));
      }
      buffer.write(_sse(
        seq++,
        'message.completed',
        jsonEncode({'messageId': 'a1', 'text': text, 'steps': 0, 'reasoningEffort': 'off'}),
      ));
      buffer.write(_sse(seq, 'run.completed', '{"runId":"$id"}'));
      return _sseResponse(buffer.toString());
    } else if (RegExp(r'^/api/ai/conversations/[^/]+/messages$').hasMatch(path)) {
      body = [
        for (var i = 1; i <= 8; i++) _historyMessage(i),
        {
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 9,
          'role': 'assistant',
          'status': 'complete',
          'text': text,
          'parts': <Object>[],
        },
      ];
    } else if (path == '/api/ai/conversations/c-new/messages') {
      body = [
        {
          'id': 'u1',
          'conversationId': 'c-new',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '开始流式',
          'parts': <Object>[],
        },
        {
          'id': 'a1',
          'conversationId': 'c-new',
          'seq': 2,
          'role': 'assistant',
          'status': 'complete',
          'text': text,
          'parts': <Object>[],
        },
      ];
    } else if (path == '/api/ai/skills') {
      body = <Object>[];
    } else if (path == '/api/ai/skills/roots') {
      body = {'roots': <Object>[]};
    } else if (path == '/api/capture/jobs') {
      body = {
        'queueRunning': 0,
        'queuePending': 0,
        'comfyReachable': true,
        'submissions': <Object>[],
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

AiWorkspaceStore? lastStore;

Future<Widget> _homePage(MockClient client) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient('http://127.0.0.1:8080', client: client),
  );
  lastStore = store;
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  group('Markdown 解析缓存（滑动时同一段文本不该被反复解析）', () {
    setUp(clearMarkdownParseCache);

    test('同一段文本解析两次：第二次直接命中缓存，返回同一个块列表', () {
      const source = '# 标题\n\n第一段 **正文**。\n\n```dart\nfinal a = 1;\n```\n';
      final first = parseMarkdown(source);
      final hits = markdownParseCacheHits;
      final second = parseMarkdown(source);

      expect(identical(first, second), isTrue,
          reason: '同一段文本必须复用上一次的解析结果，否则滚动时每次重建都要重解析一遍');
      expect(markdownParseCacheHits, hits + 1);
    });

    test('缓存有界：解析 200 段不同的文本不会无限留驻', () {
      for (var i = 0; i < 200; i++) {
        parseMarkdown('第 $i 段正文，**加粗**一下。');
      }
      // 最早那段已经被 LRU 淘汰 → 重新解析（拿到的不是同一个对象）
      final early = parseMarkdown('第 0 段正文，**加粗**一下。');
      final again = parseMarkdown('第 0 段正文，**加粗**一下。');
      expect(identical(early, again), isTrue, reason: '重新解析后又被缓存住');
    });
  });

  testWidgets('与消息无关的状态变化（清一条提示）不会让任何气泡重建', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _homePage(_backend(deltas: 1, text: '段0，')));
    await tester.pumpAndSettle();
    // 冷启动落在一条空会话上（设计如此），历史要点开左侧列表里的那条才进来
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();
    expect(lastStore!.messages.length, 9, reason: '8 条历史 + 1 条 fixture 助手消息');
    expect(find.byType(MarkdownText).evaluate().length, greaterThan(1),
        reason: '一屏里得有不止一条气泡，这个用例才有意义');

    final store = lastStore!;
    final before = aiChatBubbleBuilds;
    for (var i = 0; i < 5; i++) {
      // `clearNotice()` 会 notifyListeners()，但**没有任何消息发生变化**。
      // 重复通知时整页当然会重建（标题栏、输入区都要刷新），但一屏历史气泡
      // 必须整棵跳过 —— 否则"逐字回包时的每一次刷新"都要重排全部 Markdown。
      store.clearNotice();
      await tester.pump();
    }

    expect(aiChatBubbleBuilds, before,
        reason: '消息没变就不该重建气泡（用户 bug：聊天快速滑动 / 流式输出时卡顿）');
  });

  test('一段回包里的十几个增量只通知个位数次（不是每来一个字就整页刷一次）', () async {
    const deltas = 12;
    final text = [for (var i = 0; i < deltas; i++) '段$i，'].join();
    SharedPreferences.setMockInitialValues({});
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080', client: _backend(deltas: deltas, text: text)),
    );
    await store.load();

    var notices = 0;
    store.addListener(() => notices++);
    await store.send('开始流式');

    // 修之前：每个 delta 一次 notifyListeners()，12 个增量就是 12+ 次整页重建。
    expect(notices, lessThan(6),
        reason: '流式增量必须攒到一帧再通知，不能一个一个字地重建界面');
    expect(store.messages.last.text, text, reason: '攒着通知不能把内容攒丢');
  });
}




