// AI 工作台「卡顿」类回归（用户 bug 9.19）：
//
//   1. 聊天快速上下滑动卡顿 —— 同一段文本反复解析 Markdown 是主因之一；
//   2. 流式输出时每来一个字就整页重建一次（含一屏历史气泡）；
//   3. 展开正在持续流式输出的思考过程后更卡、且看不到新内容。
//
// 每一条都钉成**可观察的事实**，不是"感觉更快了"：
//   · `parseMarkdown` 对同一段文本只解析一次（缓存命中返回同一个对象）；
//   · 与消息无关的通知**不会让任何气泡重建**（`aiChatBubbleBuilds` 的增长量）；
//   · 一段回包里的十几个增量只通知个位数次；
//   · 展开的流式思考：贴底跟到最新一行，且**没变过的块复用同一个 widget**。

import 'dart:async';
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

http.StreamedResponse _jsonResponse(Object? body) => http.StreamedResponse(
      Stream.value(utf8.encode(jsonEncode(body))),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );

/// 假后端的状态与 JSON 路由（SSE 那一路由两个 client 各自处理）。
class _Fake {
  _Fake({this.text = '段0，'});

  /// 助手消息的完整正文（`message.completed` / 历史消息都用它）。
  final String text;
  int runCount = 0;

  /// 冷启动落在一条空会话上（设计如此），历史要点开左侧列表里的那条才进来。
  static const String historyConversationId = 'c1';
  static const String freshConversationId = 'c-new';

  Object? body(String method, String path) {
    if (path == '/api/ai/providers') {
      return [
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
    }
    if (path == '/api/ai/providers/local-gw/models') {
      return [
        {
          'providerId': 'local-gw',
          'id': 'm-text',
          'displayName': '纯文本模型',
          'inputModalities': ['text'],
          'tools': false,
          'reasoning': true,
          'enabled': true,
        }
      ];
    }
    if (path == '/api/ai/conversations' && method == 'POST') {
      return {
        'id': freshConversationId,
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 0,
      };
    }
    if (path == '/api/ai/conversations') {
      return [
        {
          'id': historyConversationId,
          'title': '卡顿复现',
          'providerId': 'local-gw',
          'modelId': 'm-text',
          'messageCount': 8,
          'updatedAt': '2026-09-19T01:00:00.000Z',
        }
      ];
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) && method == 'DELETE') {
      return {'deleted': true, 'id': path.split('/').last};
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+/runs$').hasMatch(path)) {
      runCount++;
      return {'runId': 'r$runCount', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+/messages$').hasMatch(path)) {
      final convId = path.split('/')[4];
      final history = convId == historyConversationId
          ? [for (var i = 1; i <= 8; i++) _historyMessage(i)]
          : <Map<String, dynamic>>[
              {
                'id': 'u1',
                'conversationId': convId,
                'seq': 1,
                'role': 'user',
                'status': 'complete',
                'text': '开始流式',
                'parts': <Object>[],
              },
            ];
      return [
        ...history,
        {
          'id': 'a1',
          'conversationId': convId,
          'seq': 9,
          'role': 'assistant',
          'status': 'complete',
          'text': text,
          'parts': <Object>[],
        },
      ];
    }
    if (path == '/api/ai/skills') return <Object>[];
    if (path == '/api/ai/skills/roots') {
      return {
        'userRoot': r'D:\ComfyHub\storage\ai\skills',
        'builtinRoot': r'D:\ComfyHub\skills\builtin',
        'userRootExists': true,
      };
    }
    if (path == '/api/capture/jobs') {
      return {
        'queueRunning': 0,
        'queuePending': 0,
        'comfyReachable': true,
        'submissions': <Object>[],
      };
    }
    return <Object>[];
  }

  /// 一次"逐字回包"的完整事件流（一次性返回，用来量"重建了多少次"）。
  String textDeltasSse(String runId, int deltas) {
    final buffer = StringBuffer(_sse(1, 'run.started', '{"runId":"$runId"}'));
    var seq = 2;
    for (var i = 0; i < deltas; i++) {
      buffer.write(_sse(seq++, 'text.delta', jsonEncode({'messageId': 'a1', 'text': '段$i，'})));
    }
    buffer.write(_sse(
      seq++,
      'message.completed',
      jsonEncode({'messageId': 'a1', 'text': text, 'steps': 0, 'reasoningEffort': 'off'}),
    ));
    buffer.write(_sse(seq, 'run.completed', '{"runId":"$runId"}'));
    return buffer.toString();
  }
}

/// 历史里的一条助手消息（内容足够长，才谈得上"每次解析都要花钱"）。
Map<String, dynamic> _historyMessage(int i) => {
      'id': 'h$i',
      'conversationId': _Fake.historyConversationId,
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

bool _isSsePath(String path) =>
    path.startsWith('/api/ai/runs/') && path.endsWith('/events');

MockClient _backend(_Fake fake, {int deltas = 1}) => MockClient((request) async {
      final path = request.url.path;
      if (_isSsePath(path)) {
        return _sseResponse(fake.textDeltasSse(path.split('/')[4], deltas));
      }
      return http.Response(
        jsonEncode(fake.body(request.method, path)),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });

/// 事件流由用例自己一片一片喂（用来观察"流式过程中"的界面状态）。
MockClient _streamingBackend(_Fake fake, Stream<List<int>> events) =>
    MockClient.streaming((request, _) async {
      final path = request.url.path;
      if (_isSsePath(path)) {
        return http.StreamedResponse(events, 200,
            headers: {'content-type': 'text/event-stream; charset=utf-8'});
      }
      return _jsonResponse(fake.body(request.method, path));
    });

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

String _lines(int from, int to) =>
    [for (var i = from; i <= to; i++) '第 $i 行思考内容，稍微长一点。'].join('\n');

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

    await tester.pumpWidget(await _homePage(_backend(_Fake(text: '段0，'))));
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
      // 整页当然会重建（标题栏、输入区都要刷新），但一屏历史气泡必须整棵跳过 ——
      // 否则"逐字回包时的每一次刷新"都要重排全部 Markdown。
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
      api: AiApiClient('http://127.0.0.1:8080',
          client: _backend(_Fake(text: text), deltas: deltas)),
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

  testWidgets('展开的流式思考：贴底跟到最新一行，没变过的块复用同一个 widget', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(await _homePage(_streamingBackend(_Fake(), events.stream)));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '想一个长方案');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 前 8 行思考（正好一块）+ 展开
    events.add(utf8.encode(
      _sse(1, 'run.started', '{"runId":"r1"}') +
          _sse(2, 'reasoning.delta', jsonEncode({'messageId': 'a1', 'text': _lines(1, 8)})),
    ));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(find.text('思考过程'));
    await tester.pump();

    // 用"包含某一行"来找，而不是"整块文本相等"：
    // 这样即使退回"整段一个 Text"的老写法，这两条断言也依然成立（失败原因才是对的）。
    final firstBlock = find.textContaining('第 1 行思考内容');
    expect(firstBlock, findsOneWidget);
    final firstWidget = tester.widget<Text>(firstBlock);

    // 再来 32 行：内容比 320px 的视口高得多
    events.add(utf8.encode(
      _sse(3, 'reasoning.delta', jsonEncode({'messageId': 'a1', 'text': '\n${_lines(9, 40)}'})),
    ));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(); // 帧后回调里那次 jumpTo 需要再来一帧

    // ① 展开后要能看到最新一行（用户 bug："展开后无法及时刷新"）
    final lastLine = find.textContaining('第 40 行思考内容');
    final viewport = tester.getRect(
      find.ancestor(of: lastLine, matching: find.byType(SingleChildScrollView)).first,
    );
    expect(tester.getRect(lastLine).bottom, lessThanOrEqualTo(viewport.bottom + 0.5),
        reason: '贴底时内层视口要跟到最新一行，否则新思考长在可视区外面，看起来像没刷新');

    // ② 没变过的块要复用上一次的 widget（用户 bug："展开后卡顿"）：
    //    复用之后 Flutter 连 RenderParagraph 的布局都不重做，每帧只重排最后一块。
    expect(identical(tester.widget<Text>(firstBlock), firstWidget), isTrue,
        reason: '第 1 块没有变化，必须还是同一个 widget 实例（否则每帧都在重排整段思考）');

    // 收尾：正常结束这一轮（免得留下未完成的流 / 定时器）
    events.add(utf8.encode(
      _sse(4, 'message.completed',
          jsonEncode({'messageId': 'a1', 'text': '想完了。', 'steps': 0, 'reasoningEffort': 'off'})) +
          _sse(5, 'run.completed', '{"runId":"r1"}'),
    ));
    // 注意：**不能 await 这次 close** —— 单订阅 controller 的 done future 要等消费方收尾，
    // `await close()` 会把用例挂死（本机踩过一次，表现为整个 test 进程不动了）。
    unawaited(events.close());
    await tester.pumpAndSettle();
  });
}



