// 气泡渲染回归：**同一条消息里的正文只能出现一次**。
//
// 用户 2026-09-19 的截图：一条带附件的用户消息里，同一句话上下各出现一次（图片夹在中间）。
// 根因是 `_MessageBubble` 里有两条渲染正文的路：
//   ① `_orderedBody()` 按有序块（parts）渲染；
//   ② 一条"没有段可用时退回整段正文"的兜底 `if (segments.isEmpty && m.text.isNotEmpty)`。
// 而**用户消息的 `segments` 是刻意不传的**（它是纯文本，不走 segments 那条路），
// 于是兜底那条对用户消息**永远成立** → 正文渲染两遍。
// 有附件时附件块正好插在两条正文中间，一眼就能看出来（就是截图里的样子）。
//
// 正确判据只能是"**有序块与段都没有产出任何内容**"，所以这里把三种情况都钉住：
// 有 parts / 只有附件 / 什么都没有（老数据），以及"流式过程中 parts 还没到"。

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
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

const _plainAsk = '你好，这是一条普通提问';
const _attachAsk = '这是一条带附件的提问';
const _legacyAsk = '这是一条老数据提问（没有有序块）';
const _streamDelta = '流式正文正在生成';

String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

http.StreamedResponse _jsonResponse(Object? body) => http.StreamedResponse(
      Stream.value(utf8.encode(jsonEncode(body))),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );

/// 会话 c1 里的三条历史消息：
/// ① 普通用户消息（有序块里有 text）
/// ② 带附件的用户消息（有序块 = text + attachment）—— 用户截图里的那一条
/// ③ 老数据的用户消息（**没有**有序块，只能靠兜底渲染）
const _conversationId = 'c1';

List<Map<String, dynamic>> _history() => [
      {
        'id': 'u1',
        'conversationId': _conversationId,
        'seq': 1,
        'role': 'user',
        'status': 'complete',
        'text': _plainAsk,
        'parts': [
          {'type': 'text', 'text': _plainAsk},
        ],
      },
      {
        'id': 'u2',
        'conversationId': _conversationId,
        'seq': 2,
        'role': 'user',
        'status': 'complete',
        'text': _attachAsk,
        'parts': [
          {'type': 'text', 'text': _attachAsk},
          {'type': 'attachment', 'text': 'cat.png', 'attachmentId': 'att-1'},
        ],
      },
      {
        'id': 'u3',
        'conversationId': _conversationId,
        'seq': 3,
        'role': 'user',
        'status': 'complete',
        'text': _legacyAsk,
        // 老数据（AIH-019 之前落库的消息）：一个有序块都没有
        'parts': <Object>[],
      },
      {
        'id': 'a1',
        'conversationId': _conversationId,
        'seq': 4,
        'role': 'assistant',
        'status': 'complete',
        'text': '好的，我看到了。',
        'parts': [
          {'type': 'text', 'text': '好的，我看到了。'},
        ],
      },
    ];

Object? _json(String method, String path) {
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
        'reasoning': false,
        'enabled': true,
      }
    ];
  }
  if (path == '/api/ai/conversations' && method == 'POST') {
    return {
      'id': 'c-new',
      'title': '新对话',
      'providerId': 'local-gw',
      'modelId': 'm-text',
      'messageCount': 0,
    };
  }
  if (path == '/api/ai/conversations') {
    return [
      {
        'id': _conversationId,
        'title': '渲染回归',
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 4,
        'updatedAt': '2026-09-19T02:00:00.000Z',
      }
    ];
  }
  if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) && method == 'DELETE') {
    return {'deleted': true, 'id': path.split('/').last};
  }
  if (RegExp(r'^/api/ai/conversations/[^/]+/runs$').hasMatch(path)) {
    return {'runId': 'r1', 'assistantMessageId': 'a9', 'userMessageId': 'u9'};
  }
  if (RegExp(r'^/api/ai/conversations/[^/]+/messages$').hasMatch(path)) {
    final convId = path.split('/')[4];
    if (convId == _conversationId) return _history();
    return [
      {
        'id': 'u9',
        'conversationId': convId,
        'seq': 1,
        'role': 'user',
        'status': 'complete',
        'text': '说说看',
        'parts': [
          {'type': 'text', 'text': '说说看'},
        ],
      },
      {
        'id': 'a9',
        'conversationId': convId,
        'seq': 2,
        'role': 'assistant',
        'status': 'complete',
        'text': _streamDelta,
        'parts': [
          {'type': 'text', 'text': _streamDelta},
        ],
      },
    ];
  }
  if (path == '/api/ai/skills') return <Object>[];
  if (path == '/api/ai/skills/roots') {
    return {'userRoot': r'D:\x\skills', 'builtinRoot': r'D:\y\skills', 'userRootExists': true};
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

MockClient _backend() => MockClient((request) async {
      final path = request.url.path;
      return http.Response(
        jsonEncode(_json(request.method, path)),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });

/// 事件流由用例自己喂（用来观察"流式过程中"的渲染）。
MockClient _streamingBackend(Stream<List<int>> events) =>
    MockClient.streaming((request, _) async {
      final path = request.url.path;
      if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
        return http.StreamedResponse(events, 200,
            headers: {'content-type': 'text/event-stream; charset=utf-8'});
      }
      return _jsonResponse(_json(request.method, path));
    });

Future<Widget> _homePage(MockClient client) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient('http://127.0.0.1:8080', client: client),
  );
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  );
}

/// 打开那条历史会话（冷启动落在空会话上，历史要点开左侧列表才进来）。
Future<void> _openHistory(WidgetTester tester) async {
  await tester.pumpAndSettle();
  await tester.tap(find.text('渲染回归'));
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('用户消息：有序块里有正文时只渲染一次（不再走整段正文的兜底）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _homePage(_backend()));
    await _openHistory(tester);

    expect(find.text(_plainAsk), findsOneWidget,
        reason: '同一句正文在一条消息里出现两次（用户 2026-09-19 报的截图就是这个）');
  });

  testWidgets('带附件的用户消息：正文一次 + 附件一次（图片不会把正文挤成上下两段）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _homePage(_backend()));
    await _openHistory(tester);

    expect(find.text(_attachAsk), findsOneWidget, reason: '正文只该有一次');
    // 附件仍然要渲染（修重复不能把附件一起弄丢）：缩略图就是那个 Image
    expect(find.byType(Image), findsWidgets, reason: '附件缩略图还要在');
  });

  testWidgets('老数据（一个有序块都没有）仍然退回整段正文', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _homePage(_backend()));
    await _openHistory(tester);

    expect(find.text(_legacyAsk), findsOneWidget,
        reason: 'AIH-019 之前落库的消息没有 parts，兜底那条必须还在（不能把老数据变成空白）');
  });

  testWidgets('流式生成中（parts 还没到）助手正文也只渲染一次', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(await _homePage(_streamingBackend(events.stream)));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '说说看');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 流式阶段：助手消息的 parts 还没到，正文只存在于实时段里
    events.add(utf8.encode(
      _sse(1, 'run.started', '{"runId":"r1"}') +
          _sse(2, 'text.delta', jsonEncode({'messageId': 'a9', 'text': _streamDelta})),
    ));
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.textContaining(_streamDelta, findRichText: true), findsOneWidget,
        reason: '流式正文走"实时段"这条渲染路，兜底不能再渲染一遍');

    events.add(utf8.encode(
      _sse(3, 'message.completed',
          jsonEncode({'messageId': 'a9', 'text': _streamDelta, 'steps': 0})) +
          _sse(4, 'run.completed', '{"runId":"r1"}'),
    ));
    // 不能 await：单订阅 controller 的 done future 要等消费方收尾
    unawaited(events.close());
    await tester.pumpAndSettle();

    expect(find.textContaining(_streamDelta, findRichText: true), findsOneWidget,
        reason: '落库后（有 parts 了）同样只渲染一次');
  });

  // 用户 bug⑥（2026-09-19）："AI 的思考、回答和工具调用错位（没有按时间顺序显示），
  // 直到最终答复生成时才回到正确位置"。
  //
  // 根因：`message.parts`（权威有序块）**只在整条消息结束时**才到（后端在
  // `message.completed` 里一次给全）。流式阶段气泡退回 `segments`，而实时段里
  // 早先只有思考与正文 —— 工具卡只能在末尾统一补一坨，于是
  // "思考 → 工具 → 正文" 被画成 "思考 → 正文 → 工具"，落库那一瞬间又跳回去。
  // 防线：实时段里也要记下**工具卡到达的位置**，这条用例量三个块的纵向坐标。
  testWidgets('流式生成中：思考 / 工具卡 / 正文按到达顺序交错（工具卡不能堆到最后）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    const reasoning = '先看看这个文件长什么样';
    const body = '看完了，里面写的是标题。';

    final events = StreamController<List<int>>();
    await tester.pumpWidget(await _homePage(_streamingBackend(events.stream)));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '看看 a.md');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 到达顺序：思考 → 工具调用（请求 / 开始 / 完成）→ 正文
    events.add(utf8.encode(
      _sse(1, 'run.started', '{"runId":"r1"}') +
          _sse(2, 'reasoning.delta', jsonEncode({'messageId': 'a9', 'text': reasoning})) +
          _sse(
              3,
              'tool.requested',
              '{"runId":"r1","callId":"call-1","name":"read_file",'
                  '"arguments":"{}","approval":"not_required"}') +
          _sse(4, 'tool.started', '{"callId":"call-1","name":"read_file"}') +
          _sse(5, 'tool.completed',
              '{"callId":"call-1","name":"read_file","elapsedMs":8,"preview":"# 标题"}') +
          _sse(6, 'text.delta', jsonEncode({'messageId': 'a9', 'text': body})),
    ));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump();

    final reasoningY = tester.getTopLeft(find.text(reasoning)).dy;
    final toolY = tester.getTopLeft(find.text('read_file')).dy;
    final bodyY = tester.getTopLeft(find.textContaining(body, findRichText: true)).dy;

    expect(reasoningY, lessThan(toolY), reason: '思考在工具卡上面');
    expect(toolY, lessThan(bodyY),
        reason: '工具卡要插在它到达时的那个位置（正文之前），不能等所有正文铺完再补到末尾');

    // 收尾（不能 await：单订阅 controller 的 done future 要等消费方收尾）
    unawaited(events.close());
    await tester.pumpAndSettle();
  });
  // 环节标记的兜底摘取（手机 harness 本地跑模型，后端那条摘取链路不在它前面）。
  // 只摘"名字恰好是那六个"的：别的方括号文本一个字都不能动（与后端"绝不吞内容"同一条纪律）。
  test('stripStepMarkers：只摘固定枚举的环节标记', () {
    expect(stripStepMarkers('[步骤]正在查资料[/步骤]接着干'), '接着干');
    expect(stripStepMarkers('前[步骤]正在分析请求[/步骤]后'), '前后');
    expect(stripStepMarkers('[步骤] 正在写文件 [/步骤]ok'), 'ok', reason: '允许名字两边有空白');
    // 枚举外的名字、别的方括号：原样保留（那是模型真的写出来的字）
    expect(stripStepMarkers('[步骤]正在玩耍[/步骤]x'), '[步骤]正在玩耍[/步骤]x');
    expect(stripStepMarkers('看 [1] 这张图'), '看 [1] 这张图');
    expect(stripStepMarkers('没有标记'), '没有标记');
  });
}