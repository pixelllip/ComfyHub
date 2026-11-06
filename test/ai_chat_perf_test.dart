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
import 'package:viewer/models/ai_models.dart';
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

  /// 权限档（用户建议 ① 的用例要验证"生成中切档"）：默认 `ask`。
  String permissionMode = 'ask';

  /// 冷启动落在一条空会话上（设计如此），历史要点开左侧列表里的那条才进来。
  static const String historyConversationId = 'c1';
  static const String freshConversationId = 'c-new';

  /// 第二条历史会话：内容**和第一条一样长**。
  ///
  /// 用来复现"内容整批换掉（切会话）而 `pixels` 被原样带过去"：总长一样时它甚至
  /// 不会被夹回来，用户落在新会话的**半路**上 —— 聊天软件里这是错的，
  /// 换会话该落在最新一条。
  static const String otherConversationId = 'c3';

  /// 第三条历史会话：**只有两条短消息**（内容比第一条短得多）。
  ///
  /// 用来复现用户实测的"总长第一次量完会变小，`pixels` 还停在按旧总长算的位置上
  /// —— 内容被顶到视口外，第一条消息在界面上直接消失"。
  static const String shortConversationId = 'c2';

  Object? body(String method, String path, {String? requestBody}) {
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
          'credential': {
            'configured': true,
            'source': 'managed',
            'writable': true,
          },
        },
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
        },
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
          'messageCount': 30,
          'updatedAt': '2026-09-19T01:00:00.000Z',
        },
        {
          'id': otherConversationId,
          'title': '另一个长会话',
          'providerId': 'local-gw',
          'modelId': 'm-text',
          'messageCount': 30,
          'updatedAt': '2026-09-18T01:00:00.000Z',
        },
        {
          'id': shortConversationId,
          'title': '短会话',
          'providerId': 'local-gw',
          'modelId': 'm-text',
          'messageCount': 2,
          'updatedAt': '2026-09-17T01:00:00.000Z',
        },
      ];
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        method == 'DELETE') {
      return {'deleted': true, 'id': path.split('/').last};
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        method == 'PATCH') {
      // 重命名 / 归档走这条（后端回的是改完的那一条会话）
      final id = path.split('/').last;
      final patch = jsonDecode(requestBody ?? '{}') as Map<String, dynamic>;
      return {
        'id': id,
        'title': (patch['title'] ?? '卡顿复现').toString(),
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 30,
        'updatedAt': '2026-09-19T01:00:00.000Z',
      };
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+/runs$').hasMatch(path)) {
      runCount++;
      return {
        'runId': 'r$runCount',
        'assistantMessageId': 'a1',
        'userMessageId': 'u1',
      };
    }
    if (RegExp(r'^/api/ai/conversations/[^/]+/messages$').hasMatch(path)) {
      final convId = path.split('/')[4];
      if (convId == otherConversationId) {
        // 和第一条同构（同样 30 条长短交错），只是 id 不同：
        // 总长一样 → 切过去之后 `pixels` 连"被夹回来"都不会发生。
        return [
          for (var i = 1; i <= 30; i++)
            {
              'id': 'o$i',
              'conversationId': convId,
              'seq': i,
              'role': i.isOdd ? 'user' : 'assistant',
              'status': 'complete',
              'text': _variedText(i),
              'parts': <Object>[],
            },
        ];
      }
      if (convId == shortConversationId) {
        // 两条短消息：总长比第一条小一个数量级（用户实测"估计 220 → 真实 52"的放大版）
        return [
          {
            'id': 's1',
            'conversationId': convId,
            'seq': 1,
            'role': 'user',
            'status': 'complete',
            'text': '短会话的第一句。',
            'parts': <Object>[],
          },
          {
            'id': 's2',
            'conversationId': convId,
            'seq': 2,
            'role': 'assistant',
            'status': 'complete',
            'text': '短会话的最后一句。',
            'parts': <Object>[],
          },
        ];
      }
      final history = convId == historyConversationId
          // **长短交错**的一长串：滚动条跳变只在"高度差异大 + 项目多"时才明显
          // （用户现场就是一条聊了很久的会话）。30 条足够复现，也让
          // "总长稳定性"那条用例真的量得到东西。
          ? [for (var i = 1; i <= 30; i++) _historyMessage(i)]
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
    // 权限档（用户建议 ①）：GET 给当前档，PUT 收下新的档再回读 ——
    // 与后端 `PUT /api/ai/tools/policy` 同语义，否则切档会静默回落。
    if (path == '/api/ai/tools/policy') {
      if (method == 'PUT') {
        permissionMode =
            ((jsonDecode(requestBody ?? '{}') as Map)['permissionMode'] ?? '')
                .toString()
                .trim();
        if (permissionMode.isEmpty) permissionMode = 'ask';
      }
      return {
        'writeRoots': <Object>[],
        'readRoots': <Object>[],
        'overrides': <String, String>{},
        'maxToolSteps': 8,
        'maxCallsPerRun': 16,
        'maxReadBytes': 2 * 1024 * 1024,
        'maxWriteBytes': 4 * 1024 * 1024,
        'permissionMode': permissionMode,
      };
    }
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
      buffer.write(
        _sse(
          seq++,
          'text.delta',
          jsonEncode({'messageId': 'a1', 'text': '段$i，'}),
        ),
      );
    }
    buffer.write(
      _sse(
        seq++,
        'message.completed',
        jsonEncode({
          'messageId': 'a1',
          'text': text,
          'steps': 0,
          'reasoningEffort': 'off',
        }),
      ),
    );
    buffer.write(_sse(seq, 'run.completed', '{"runId":"$runId"}'));
    return buffer.toString();
  }
}

/// 历史里的一条消息。
///
/// **高度差异故意做得很大**（短句 / 中段 / 带大代码块的长段，三种交错）：
/// 滚动条跳变（总长估算乱跳）只在"有的气泡一行、有的两屏"时才明显 ——
/// 用户现场就是这种聊了很久的会话。"总长稳定性"那条用例全靠它量得出东西。
Map<String, dynamic> _historyMessage(int i) => {
  'id': 'h$i',
  'conversationId': _Fake.historyConversationId,
  'seq': i,
  'role': i.isOdd ? 'user' : 'assistant',
  'status': 'complete',
  'text': _variedText(i),
  'parts': <Object>[],
};

String _variedText(int i) {
  if (i % 5 == 0) {
    final b = StringBuffer('第 $i 条：很长的回答。\n\n```json\n');
    for (var k = 0; k < 14; k++) {
      b.writeln('{"node": $k, "input": "checkpoint-$i", "value": "value-$k"}');
    }
    b.writeln('```\n\n后面还有一段收尾说明文字，再加一点点高度。');
    return b.toString();
  }
  if (i % 3 == 0) {
    return '第 $i 条：中等长度的一段说明文字，大概三四行的样子，'
        '用来把气泡高度做出层次。再来一句凑数的话。';
  }
  return '第 $i 条：短句。';
}

bool _isSsePath(String path) =>
    path.startsWith('/api/ai/runs/') && path.endsWith('/events');

MockClient _backend(_Fake fake, {int deltas = 1}) =>
    MockClient((request) async {
      final path = request.url.path;
      if (_isSsePath(path)) {
        return _sseResponse(fake.textDeltasSse(path.split('/')[4], deltas));
      }
      return http.Response(
        jsonEncode(fake.body(request.method, path, requestBody: request.body)),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });

/// 事件流由用例自己一片一片喂（用来观察"流式过程中"的界面状态）。
MockClient _streamingBackend(_Fake fake, Stream<List<int>> events) =>
    MockClient.streaming((request, _) async {
      final path = request.url.path;
      if (_isSsePath(path)) {
        return http.StreamedResponse(
          events,
          200,
          headers: {'content-type': 'text/event-stream; charset=utf-8'},
        );
      }
      return _jsonResponse(
        fake.body(request.method, path, requestBody: _requestBody(request)),
      );
    });

/// `MockClient.streaming` 给的是 `BaseRequest`（没有 body 字段）——
/// 真类型是 `Request` 时把 body 取出来，给 PATCH 那条假路由用。
String? _requestBody(http.BaseRequest request) =>
    request is http.Request ? request.body : null;

AiWorkspaceStore? lastStore;

Future<Widget> _homePage(
  MockClient client, {
  ScrollBehavior? scrollBehavior,
}) async {
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
    child: MaterialApp(
      // 滚动条的拖动 / 鼠标拖动在 widget test 的默认作用域里是关着的，
      // 要测"用户拖着滚"就得把它打开（见下面那条用户 bug 用例）。
      scrollBehavior: scrollBehavior,
      home: const AiHomePage(),
    ),
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

      expect(
        identical(first, second),
        isTrue,
        reason: '同一段文本必须复用上一次的解析结果，否则滚动时每次重建都要重解析一遍',
      );
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
    expect(lastStore!.messages.length, 31, reason: '30 条历史 + 1 条 fixture 助手消息');
    expect(
      find.byType(MarkdownText).evaluate().length,
      greaterThan(1),
      reason: '一屏里得有不止一条气泡，这个用例才有意义',
    );

    final store = lastStore!;
    final before = aiChatBubbleBuilds;
    for (var i = 0; i < 5; i++) {
      // `clearNotice()` 会 notifyListeners()，但**没有任何消息发生变化**。
      // 整页当然会重建（标题栏、输入区都要刷新），但一屏历史气泡必须整棵跳过 ——
      // 否则"逐字回包时的每一次刷新"都要重排全部 Markdown。
      store.clearNotice();
      await tester.pump();
    }

    expect(
      aiChatBubbleBuilds,
      before,
      reason: '消息没变就不该重建气泡（用户 bug：聊天快速滑动 / 流式输出时卡顿）',
    );
  });

  test('一段回包里的十几个增量只通知个位数次（不是每来一个字就整页刷一次）', () async {
    const deltas = 12;
    final text = [for (var i = 0; i < deltas; i++) '段$i，'].join();
    SharedPreferences.setMockInitialValues({});
    final store = AiWorkspaceStore(
      api: AiApiClient(
        'http://127.0.0.1:8080',
        client: _backend(_Fake(text: text), deltas: deltas),
      ),
    );
    await store.load();

    var notices = 0;
    store.addListener(() => notices++);
    await store.send('开始流式');

    // 修之前：每个 delta 一次 notifyListeners()，12 个增量就是 12+ 次整页重建。
    expect(notices, lessThan(6), reason: '流式增量必须攒到一帧再通知，不能一个一个字地重建界面');
    expect(store.messages.last.text, text, reason: '攒着通知不能把内容攒丢');
  });

  // 用户 bug（2026-09-20）："如果在 AI 正输出回答的时候滚动页面，只滚一小段的话
  // 界面会立即重置到底部，再继续滚才不会滚下去。"
  //
  // 根因在 `_onScroll` 的"贴底"判据：老写法是 `distance < 80`，等于给"贴底"画了个
  // 80px 宽的笼子。流式期间用户往上滚一点点，内容同时在往下长，
  // `maxScrollExtent - pixels` 一直落在 80 以内 → 下一帧又被判成"贴底"、
  // 被 `_followIfAtBottom()` 的 `animateTo(max)` 拽回去。
  // 一次滚够 80px 以上才逃得出去 —— 用户的"再继续滚才不会滚下去"就是这么来的。
  //
  // 用例走**真实链路**：贴底 → 往上跳 30px（等价于滚轮推一下 / 拖一小段，
  // 会触发同一个 `_onScroll`）→ 继续灌流式增量 → 位置必须留在原地。
  testWidgets('流式输出中往上滚一小段：位置要留住，不能被拽回底部（用户 bug）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(
      await _homePage(_streamingBackend(_Fake(), events.stream)),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '写一份很长的方案');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 第一批：足够长，列表可滚动且**已经贴底**
    events.add(
      utf8.encode(
        _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'text.delta',
              jsonEncode({'messageId': 'a1', 'text': _lines(1, 100)}),
            ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));
    // 贴底那一下是 `animateTo`（180ms）：要真的把它跑完再量位置。
    // **不能用 `pumpAndSettle`**：生成期间底部那个"AI 正在生成…"提示里的
    // `CircularProgressIndicator` 是永不停的动画，`pumpAndSettle` 会一直等到超时
    // （空转 300ms 足够跑完那次 180ms 的贴底动画）。
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }

    final position = tester
        .state<ScrollableState>(
          find.descendant(
            of: find.byKey(const Key('ai-message-list')),
            matching: find.byType(Scrollable),
          ),
        )
        .position;
    expect(
      position.maxScrollExtent,
      greaterThan(200),
      reason: '得真的能滚，这个用例才有意义',
    );
    final bottom = position.pixels;
    expect(
      bottom,
      closeTo(position.maxScrollExtent, 1.5),
      reason: '流式输出时本来就该贴底',
    );

    // 用户往上滚**一小段**（30px —— 老判据那个 80px 笼子里的一小步）
    position.jumpTo(bottom - 30);
    await tester.pump();
    expect(position.pixels, closeTo(bottom - 30, 1.0), reason: '手势本身要真的滚上去');

    // AI 还在往外吐字（用户说的"正输出回答的时候"）：每帧都来一段
    for (var i = 0; i < 8; i++) {
      events.add(
        utf8.encode(
          _sse(
            10 + i,
            'text.delta',
            jsonEncode({
              'messageId': 'a1',
              'text': '\n${_lines(110 + i * 5, 114 + i * 5)}',
            }),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 16));
    }

    expect(
      position.pixels,
      closeTo(bottom - 30, 2.0),
      reason: '用户已经往上翻了，流式的新内容不该把他拽回底部（用户 bug：只滚一小段立即重置到底部）',
    );
    expect(
      find.byTooltip('回到最新消息'),
      findsOneWidget,
      reason: '翻上去之后右下角要出现"回到最新消息"',
    );

    // 自己滚回底部：贴底之后必须重新跟随（不然"跟随"就永久失效了）
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    events.add(
      utf8.encode(
        _sse(
          50,
          'text.delta',
          jsonEncode({'messageId': 'a1', 'text': '\n${_lines(200, 204)}'}),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 16));
    // 同样不能用 `pumpAndSettle`：这一轮还没结束，底部提示的转圈动画不会停
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(
      position.pixels,
      closeTo(position.maxScrollExtent, 1.5),
      reason: '滚回底部后要重新贴底（跟随不能永久失效）',
    );

    // 收尾：正常结束这一轮（免得留下未完成的流 / 定时器）
    events.add(
      utf8.encode(
        _sse(
              99,
              'message.completed',
              jsonEncode({
                'messageId': 'a1',
                'text': _lines(1, 100),
                'steps': 0,
                'reasoningEffort': 'off',
              }),
            ) +
            _sse(100, 'run.completed', '{"runId":"r1"}'),
      ),
    );
    unawaited(events.close());
    await tester.pumpAndSettle();
  });

  // 用户 bug（2026-09-20）："快速拉动滚动条时，其位置变化异常。"
  //
  // 和上一条是同一个根因的另一个症状：老判据（距离 < 80 也算贴底）让用户以为
  // "我还在底部"，于是 `_followIfAtBottom()` 的 `animateTo(maxScrollExtent)` 一直在排队，
  // 用户一边拉滚动条、那个 180ms 的动画一边在跑 —— 手指和动画各拉一个方向，位置就跳。
  // 容差收到 1.5 之后，用户滚一像素就脱离"贴底"，跟随当场不再排新的动画。
  //
  // 这条用例直接**真的拖**：从贴底往上拖 300px，然后要求位置就是"往上 300px 左右"，
  // 而不是被拖回底部；再往上拖，位置继续跟着走。
  testWidgets('用户往上拖时，位置跟手、不被跟随动画拽回底部（用户 bug：位置变化异常）', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(
      await _homePage(_streamingBackend(_Fake(), events.stream)),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '写一份很长的方案');
    await tester.tap(find.text('发送'));
    await tester.pump();

    events.add(
      utf8.encode(
        _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'text.delta',
              jsonEncode({'messageId': 'a1', 'text': _lines(1, 100)}),
            ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));
    // 同样不能用 `pumpAndSettle` —— 这一轮还没结束，底部"AI 正在生成…"那个
    // `CircularProgressIndicator` 是永不停的动画（空转 300ms 足够跑完 180ms 的贴底动画）。
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }

    final position = tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byKey(const Key('ai-message-list')),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;
    final bottom = position.pixels;
    expect(bottom, closeTo(position.maxScrollExtent, 1.5), reason: '起点是贴底的');

    // 拖动期间 AI 还在吐字（这时老代码会不停排跟随动画）
    events.add(
      utf8.encode(
        _sse(
          3,
          'text.delta',
          jsonEncode({'messageId': 'a1', 'text': '\n${_lines(101, 140)}'}),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 16));

    await tester.drag(
      find.byKey(const Key('ai-message-list')),
      const Offset(0, 300),
    );
    await tester.pump();

    // 被"拽回底部"的话这里会重新贴到 max；跟手的话应该往上走了 300px 上下。
    // 只钉"确实离开底部"（拖动本身带一点物理摩擦，不写死像素数）。
    expect(
      position.pixels,
      lessThan(bottom - 150),
      reason: '往上拖了 300px，位置必须真的上去（被动画拽回底部就是这个断言先红）',
    );

    // 再拖一次：仍然跟手（不是"只有第一次能动"）
    events.add(
      utf8.encode(
        _sse(
          4,
          'text.delta',
          jsonEncode({'messageId': 'a1', 'text': '\n${_lines(141, 180)}'}),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 16));
    final afterFirst = position.pixels;
    await tester.drag(
      find.byKey(const Key('ai-message-list')),
      const Offset(0, 200),
    );
    await tester.pump();
    expect(
      position.pixels,
      lessThan(afterFirst - 100),
      reason: '第二次拖动同样要跟手（跟随动画不许把人往回拽）',
    );

    // 收尾：正常结束这一轮
    events.add(
      utf8.encode(
        _sse(
              9,
              'message.completed',
              jsonEncode({
                'messageId': 'a1',
                'text': _lines(1, 100),
                'steps': 0,
                'reasoningEffort': 'off',
              }),
            ) +
            _sse(10, 'run.completed', '{"runId":"r1"}'),
      ),
    );
    unawaited(events.close());
    await tester.pumpAndSettle();
  });

  // 用户撞上的第四个坑（2026-09-20）："总长第一次量完会变小，而 pixels 还停在
  // 按旧总长算的位置上 —— 内容被顶到视口外，第一条消息在界面上直接消失。"
  //
  // 对应的真实时机就是"内容整批换掉"：**切会话**。切过去之后 `messages` 整批换成
  // 新的，而 `_Thread` 只是换了数据，**滚动位置不会自己重置** —— `pixels` 被原样
  // 带到新会话上。新内容短就是"被顶到视口外"，新内容一样长就是"落在半路"，
  // 两者都是错的：聊天软件里换会话就该落在**最新一条**。
  //
  // 这一版把"总长变小"这件事彻底绕开了：内容高度就是真实高度，框架不会把 `pixels`
  // 夹到视口外去（旧版会，因为总长是估算的、可能比真值小）。剩下这一条是
  // "换会话要重置位置"的**产品纪律**，由界面显式负责，不靠框架的夹取去猜。
  testWidgets('切换会话落在最新一条（内容整批换掉时位置要重置，用户 bug）', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await _homePage(_backend(_Fake(text: _lines(1, 60)))),
    );
    await tester.pumpAndSettle();

    // 先在长会话里滚上去（`pixels` 远离底部）
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();
    final position = tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byKey(const Key('ai-message-list')),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;
    expect(position.maxScrollExtent, greaterThan(400), reason: '长会话得真的能滚');
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    position.jumpTo(position.pixels - 1500);
    await tester.pump();
    expect(position.pixels, greaterThan(0), reason: '要真的停在半路，这个用例才有意义');

    // 切到另一个**同样长**的会话：总长一样，`pixels` 连"被夹回来"都不会发生
    await tester.tap(find.text('另一个长会话'));
    await tester.pumpAndSettle();

    expect(
      position.pixels,
      closeTo(position.maxScrollExtent, 1.5),
      reason:
          '换会话要落在最新一条；位置被原样带过去的话会停在半路'
          '（用户 bug：内容整批换掉后位置还在按旧总长算）',
    );

    // 反向再验一次：从半路切回第一条会话，同样要贴底
    position.jumpTo(position.pixels - 800);
    await tester.pump();
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();
    expect(
      position.pixels,
      closeTo(position.maxScrollExtent, 1.5),
      reason: '两个方向都要重置（不是"只有第一次切才生效"）',
    );

    // 第三种：切到**内容短得多**的会话。
    // 这是用户实测的那个现场 —— 总长一下子小掉一个数量级，
    // `pixels` 要是还按旧总长算，新内容就被顶到视口外去了。
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    expect(position.pixels, greaterThan(1000), reason: '长会话要真的滚得动，这个用例才有意义');
    await tester.tap(find.text('短会话'));
    await tester.pumpAndSettle();
    expect(
      position.pixels,
      closeTo(position.maxScrollExtent, 1.5),
      reason: '切到短会话同样要落在最新一条（不能被顶到视口外）',
    );
    final first = find.text('短会话的第一句。');
    expect(first, findsOneWidget, reason: '第一条消息要真的可见');
    final listRect = tester.getRect(find.byKey(const Key('ai-message-list')));
    expect(
      tester.getRect(first).top,
      greaterThanOrEqualTo(listRect.top - 0.5),
      reason: '第一条消息不能被顶到视口上面去（用户报的"直接消失"）',
    );
  });

  // 上面两条证明"总长精确"。这条把结论落到**用户看到的那个滑块**上：
  // 滑块的长度 = `viewport / (maxScrollExtent + viewport)`，位置 = `pixels / maxScrollExtent`
  // —— 总长一稳，滑块就稳。
  //
  // （不直接 `find.byType(RawScrollbar)`：widget test 的运行平台不是桌面，
  //  Material 也不会给它挂滚动条，那样断言只会红得没有意义。
  //  这里按滑块的**定义**算，等价于量它。）
  //
  // 视口按本机真实显示设置来（1707×920 物理 / dpr 1.25 → 1366×736 逻辑），
  // 免得"逻辑像素下没问题、真实缩放下才有问题"。
  testWidgets('滚动条滑块：往上滚时长度不变、位置单调，不会来回跳', (tester) async {
    tester.view.physicalSize = const Size(1707, 920);
    tester.view.devicePixelRatio = 1.25;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await _homePage(_backend(_Fake(text: _lines(1, 60)))),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();

    final position = tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byKey(const Key('ai-message-list')),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();

    // 应用用默认的 Material 滚动条：滑块长度与位置就是下面这两个比值。
    double thumbRatio() =>
        position.viewportDimension /
        (position.maxScrollExtent + position.viewportDimension);
    double thumbProgress() => position.pixels / position.maxScrollExtent;

    final thumbs = <double>{};
    final progress = <double>[];
    for (var i = 0; i < 12; i++) {
      await tester.pump();
      thumbs.add(thumbRatio());
      progress.add(thumbProgress());
      position.jumpTo(
        (position.pixels - 400).clamp(0.0, position.maxScrollExtent),
      );
      await tester.pump();
      if (position.pixels <= 0) break;
    }

    // 滑块长度只该随"总长"变；总长精确时它全程一致（旧版估算会一路变短）
    expect(
      thumbs.length,
      1,
      reason:
          '总长精确 → 滑块长度全程不变，实测 ${thumbs.map((t) => t.toStringAsFixed(4)).toSet()}',
    );
    // 位置单调：往上滚，滑块只会越来越靠近顶部，不会往回跳
    for (var i = 1; i < progress.length; i++) {
      expect(
        progress[i],
        lessThanOrEqualTo(progress[i - 1] + 0.001),
        reason:
            '往上滚时滑块位置必须单调上移，不能跳回来'
            '（实测 ${progress.map((t) => t.toStringAsFixed(3)).toList()}）',
      );
    }
  });

  // 用户 bug（2026-09-20）："在最新一条聊天记录里，滚动条会跳变 1/3 的进度。"
  //
  // 根因：懒构建的 `ListView.builder` 对**还没布局到的**子项只能估算总长，
  // 估法是把剩余未建项数乘以"已建那几项的平均高度"（`estimateMaxScrollOffset`）——
  // 聊天气泡高度差 5 倍以上，这个均值每滚一段就换一次，总长就来回跳，
  // 而滚动条滑块的长度与位置都是 `pixels / maxScrollExtent` 算出来的。
  // 本机实测（30 条长短交错、1100×800、真值 max=4137）：默认缓存下往上滚一趟
  // 总长变了 **18 次**、其中 **9 次是回缩**，范围 3539~5392。
  //
  // 走过的两步（第二步才是答案）：
  //   ① 把缓存窗口放到前后各 2 个视口 —— 只让均值更稳，实测变化 17→11、
  //      回缩 9→4，**压掉一半、压不到 0**（"估"这个本质没变）；
  //   ② 换成 `SingleChildScrollView + Column` —— 内容高度**就是真实高度**，
  //      `maxScrollExtent` 从第一帧起精确，滑块全程不变。
  //
  // 这条用例现在钉的是**更强的性质**：滚一趟，总长一次都不许变
  // （以前只能钉"回缩不超过 8 次"）。
  testWidgets('长会话滚动时总长一次都不变（用户 bug：滚动条跳变 1/3 进度）', (tester) async {
    tester.view.physicalSize = const Size(1100, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await _homePage(_backend(_Fake(text: _lines(1, 60)))),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();

    final position = tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byKey(const Key('ai-message-list')),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;
    expect(
      position.maxScrollExtent,
      greaterThan(400),
      reason: '得真的能滚，这个用例才有意义',
    );

    // 往上滚一趟，记录总长的每一次变化（含幅度与方向）
    final values = <double>{position.maxScrollExtent};
    var changes = 0;
    var shrinks = 0;
    var last = position.maxScrollExtent;
    for (var i = 0; i < 60; i++) {
      position.jumpTo(
        (position.pixels - 120).clamp(0.0, position.maxScrollExtent),
      );
      await tester.pump();
      final max = position.maxScrollExtent;
      if ((max - last).abs() > 0.5) {
        changes++;
        if (max < last) shrinks++;
        values.add(max);
      }
      last = max;
      if (position.pixels <= 0) break;
    }

    final lo = values.reduce((a, b) => a < b ? a : b);
    final hi = values.reduce((a, b) => a > b ? a : b);
    debugPrint(
      '[总长稳定性] 变化=$changes 回缩=$shrinks 范围=${lo.toStringAsFixed(0)}~'
      '${hi.toStringAsFixed(0)} 摆动=${((hi - lo) / hi * 100).toStringAsFixed(1)}%',
    );
    // 精确总长下这里必须是 **0 次**：滚动条滑块的长度与位置全程不变。
    // 退回懒构建（`ListView.builder`）实测 13~24 次变化、5~12 次回缩，必红。
    expect(
      changes,
      0,
      reason:
          '总长一变，滚动条滑块的长度/位置就跟着变（用户 bug：跳变 1/3 进度）'
          '（实测 $lo~$hi，变化 $changes 次，回缩 $shrinks 次）',
    );
  });

  // 上一条只证明"滚动时不变"，这条证明**精确**：总长必须等于真实内容高度。
  //
  // 判据来自 `ScrollPosition` 自己的定义：`maxScrollExtent = scrollExtent − viewportDimension`，
  // 而 `Column` 的 `scrollExtent` 就是它子项的真实高度之和。所以拿
  // "最底那条气泡的相对位置"当独立参照来反推，两边必须严格相等 ——
  // 估算出来的总长（懒构建那套）在这里一定会差出几百像素。
  testWidgets('精确总长：maxScrollExtent 与内容真实高度严格相等', (tester) async {
    tester.view.physicalSize = const Size(1100, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await _homePage(_backend(_Fake(text: _lines(1, 60)))),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();

    final position = tester
        .state<ScrollableState>(
          find
              .descendant(
                of: find.byKey(const Key('ai-message-list')),
                matching: find.byType(Scrollable),
              )
              .first,
        )
        .position;

    // 滚到底：`pixels` 必须真的等于 `maxScrollExtent`（估算偏高时会被夹住/滚进空白）
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    expect(
      position.pixels,
      closeTo(position.maxScrollExtent, 0.5),
      reason: '精确总长下能真的滚到最底，不会滚进估算多出来的空白',
    );

    // 独立参照：最后一条气泡的底边相对视口底部的位置。
    // 贴底时它必须正好落在视口下沿（差的就是最后那段 6px 内边距 + 6px 滚动视图内边距）。
    final last = tester.getRect(
      find.byKey(ValueKey(lastStore!.messages.last.id)),
    );
    final viewport = tester.getRect(find.byKey(const Key('ai-message-list')));
    expect(
      last.bottom,
      closeTo(viewport.bottom - 6, 1.0),
      reason:
          '贴底后最后一条气泡的底边要正好落在视口下沿；'
          '对不上说明总长与真实内容高度不一致（那就是"估算"）',
    );
  });

  // 用户 bug（2026-09-20）的性能现场：DevTools 抓到一帧 **Build 24.7ms**（UI Jank），
  // Rebuild Stats 上排前几名的全是**左侧会话列表**里的东西
  // （Tooltip 12 / MenuAnchor 12 / ListTile 10 / AppMenuButton 10）。
  //
  // 根因不是滚动本身，是**通知**：流式回包（攒到一帧也有十几次/秒）与 ComfyUI
  // 进度轮询（1.5 秒一次）都会 `notifyListeners()`，而页面用 `context.watch`
  // 盯着整个 store、`build()` 里又新建 `_ConversationList(...)` ——
  // 于是每通知一次，那一栏（几十个 ListTile + 每个一个菜单锚点 + Tooltip）
  // 就整个重排一遍。会话列表跟"消息在长"毫无关系。
  //
  // 修复：① 会话主体抽成 `_Thread` 单独 watch，页面 State 不再 watch；
  // ② 侧栏自己 `addListener`，只在**会话列表实例或选中项真的变了**时才 setState；
  // ③ 侧栏 widget 实例提到 State 字段里（实例不变，Flutter 整棵跳过）。
  // 实测：通知期间侧栏重建从"每通知 1 次"降到 **0 次**。
  testWidgets('与侧栏无关的通知（流式回包 / 进度轮询）不会再重建会话列表', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _homePage(_backend(_Fake(text: '段0，'))));
    await tester.pumpAndSettle();
    // 冷启动落在空会话上，历史会话要点开才进来（侧栏这时才真的有几十行）
    await tester.tap(find.text('卡顿复现'));
    await tester.pumpAndSettle();

    final store = lastStore!;
    final before = sidebarBuilds;
    for (var i = 0; i < 5; i++) {
      // `clearNotice()` 会 notifyListeners()，但会话列表与选中项都没变
      store.clearNotice();
      await tester.pump();
    }

    expect(
      sidebarBuilds,
      before,
      reason:
          '侧栏跟消息/进度无关：那些通知不该让它重建'
          '（修之前每通知一次重建一次，DevTools 上一帧 Build 24.7ms）',
    );

    // 反例：会话列表真的变了（重命名）时，侧栏**必须**跟着更新
    await store.renameConversation('改个名字');
    await tester.pumpAndSettle();
    expect(sidebarBuilds, greaterThan(before), reason: '会话名变了，侧栏当然要重建');
    expect(find.text('改个名字'), findsWidgets, reason: '改完要看得见');
  });

  testWidgets('展开的流式思考：贴底跟到最新一行，没变过的块复用同一个 widget', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(
      await _homePage(_streamingBackend(_Fake(), events.stream)),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '想一个长方案');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 前 8 行思考（正好一块）+ 展开
    events.add(
      utf8.encode(
        _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'reasoning.delta',
              jsonEncode({'messageId': 'a1', 'text': _lines(1, 8)}),
            ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(find.text('思考过程'));
    await tester.pump();

    // 用"包含某一行"来找，而不是"整块文本相等"：
    // 这样即使退回"整段一个 Text"的老写法，这两条断言也依然成立（失败原因才是对的）。
    final firstBlock = find.textContaining('第 1 行思考内容');
    expect(firstBlock, findsOneWidget);
    final firstWidget = tester.widget<Text>(firstBlock);

    // 再来 32 行：内容比 320px 的视口高得多
    events.add(
      utf8.encode(
        _sse(
          3,
          'reasoning.delta',
          jsonEncode({'messageId': 'a1', 'text': '\n${_lines(9, 40)}'}),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(); // 帧后回调里那次 jumpTo 需要再来一帧

    // ① 展开后要能看到最新一行（用户 bug："展开后无法及时刷新"）
    final lastLine = find.textContaining('第 40 行思考内容');
    final viewport = tester.getRect(
      find
          .ancestor(of: lastLine, matching: find.byType(SingleChildScrollView))
          .first,
    );
    expect(
      tester.getRect(lastLine).bottom,
      lessThanOrEqualTo(viewport.bottom + 0.5),
      reason: '贴底时内层视口要跟到最新一行，否则新思考长在可视区外面，看起来像没刷新',
    );

    // ② 没变过的块要复用上一次的 widget（用户 bug："展开后卡顿"）：
    //    复用之后 Flutter 连 RenderParagraph 的布局都不重做，每帧只重排最后一块。
    expect(
      identical(tester.widget<Text>(firstBlock), firstWidget),
      isTrue,
      reason: '第 1 块没有变化，必须还是同一个 widget 实例（否则每帧都在重排整段思考）',
    );

    // 收尾：正常结束这一轮（免得留下未完成的流 / 定时器）
    events.add(
      utf8.encode(
        _sse(
              4,
              'message.completed',
              jsonEncode({
                'messageId': 'a1',
                'text': '想完了。',
                'steps': 0,
                'reasoningEffort': 'off',
              }),
            ) +
            _sse(5, 'run.completed', '{"runId":"r1"}'),
      ),
    );
    // 注意：**不能 await 这次 close** —— 单订阅 controller 的 done future 要等消费方收尾，
    // `await close()` 会把用例挂死（本机踩过一次，表现为整个 test 进程不动了）。
    unawaited(events.close());
    await tester.pumpAndSettle();
  });

  // 用户建议（2026-09-20）：「"AI 正在生成"不要出现在底部（很影响观感），
  // 而是当前正在生成回复的气泡底部，并且只保留转圈圈动画」。
  //
  // 这里钉三件事，缺一条就是建议没落地：
  //   1. **页脚那条提示整个没有了**（连文字都不该存在）；
  //   2. 转圈动画画在**正在流的那一条气泡里**（不是页面底部）；
  //   3. 这一轮结束就收掉（不能留下一个转个不停的历史气泡）。
  testWidgets('「正在生成」只在当前气泡底部转圈，页脚不再有提示', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(
      await _homePage(_streamingBackend(_Fake(), events.stream)),
    );
    await tester.pumpAndSettle();

    // 还没发消息：不该莫名其妙冒出一个"正在生成"
    expect(find.text('AI 正在生成…'), findsNothing, reason: '页脚那条提示已经删掉了');
    expect(find.text('正在生成…'), findsNothing, reason: '气泡里那行字也删掉了，只留转圈');

    await tester.enterText(find.byType(TextField).first, '写一份很长的方案');
    await tester.tap(find.text('发送'));
    await tester.pump();

    // 首字还没到：气泡里就该有一个转圈（而不是一片空白）
    expect(
      find.byType(CircularProgressIndicator),
      findsOneWidget,
      reason: '首字未到时，正在生成的那条气泡里要有一个转圈',
    );

    // 已经有正文了：转圈**依然在**（用户要的就是"只要还在生成就一直转"）
    events.add(
      utf8.encode(
        _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'text.delta',
              jsonEncode({'messageId': 'a1', 'text': _lines(1, 40)}),
            ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));

    final spinner = find.byType(CircularProgressIndicator);
    expect(spinner, findsOneWidget, reason: '有正文时页脚那条旧提示会消失，转圈不能跟着消失');
    expect(find.text('AI 正在生成…'), findsNothing, reason: '任何阶段都不该再有页脚提示');

    // 它在**最后一条气泡内部**，而且贴在气泡底部（用户说的"气泡底部"）
    final bubbleRect = tester.getRect(
      find.byKey(ValueKey('ai-bubble-a1')).first,
    );
    final spinnerRect = tester.getRect(spinner);
    expect(
      bubbleRect.contains(spinnerRect.center),
      isTrue,
      reason: '转圈必须在正在生成的那条气泡里，不能飘在页面底部',
    );
    expect(spinnerRect.bottom, lessThanOrEqualTo(bubbleRect.bottom + 0.5));
    expect(
      bubbleRect.bottom - spinnerRect.bottom,
      lessThan(24),
      reason: '要贴在气泡底部（在末尾内容的下方）',
    );

    // 这一轮结束：转圈必须收掉，不能一直转
    events.add(
      utf8.encode(
        _sse(
              3,
              'message.completed',
              jsonEncode({
                'messageId': 'a1',
                'text': '写好了。',
                'steps': 0,
                'reasoningEffort': 'off',
              }),
            ) +
            _sse(4, 'run.completed', '{"runId":"r1"}'),
      ),
    );
    unawaited(events.close());
    await tester.pumpAndSettle();
    expect(
      find.byType(CircularProgressIndicator),
      findsNothing,
      reason: '这一轮结束后要收掉（否则会一直转下去）',
    );
  });

  // 用户建议（2026-09-20）①：「实时权限切换，并在正在运行的回复气泡中展示相关信息」。
  //
  // "实时"是可观察的事实：生成**过程中**切档，那个气泡里的字立刻跟着变 ——
  // 不是等这一轮结束、也不用去输入区低头找按钮。
  testWidgets('生成中切权限档：正在生成的气泡里立刻显示新档位', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final events = StreamController<List<int>>();
    await tester.pumpWidget(
      await _homePage(_streamingBackend(_Fake(), events.stream)),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField).first, '帮我改一下工作流');
    await tester.tap(find.text('发送'));
    await tester.pump();
    events.add(
      utf8.encode(
        _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'text.delta',
              jsonEncode({'messageId': 'a1', 'text': _lines(1, 8)}),
            ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 50));

    // 默认档是「询问」，气泡里要写着
    expect(
      find.textContaining('本轮权限是 ${AiToolPolicy.modeAskLabel}'),
      findsOneWidget,
    );

    // 生成**还没结束**就切档（转圈动画会让 pumpAndSettle 超时，这里只能 pump）
    await tester.tap(find.text(AiToolPolicy.modeAskLabel));
    await tester.pump(const Duration(milliseconds: 100));
    final fullItem = find.text(AiToolPolicy.modeFullLabel).last;
    await tester.ensureVisible(fullItem);
    await tester.tap(fullItem);
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pump(const Duration(milliseconds: 100));

    expect(
      find.textContaining('本轮权限是 ${AiToolPolicy.modeFullLabel}'),
      findsOneWidget,
      reason: '切档要**当场**反映到正在生成的回复气泡里（用户建议 ①）',
    );

    // 这一轮结束：那个"实时"标记跟着转圈一起收掉（后端没按轮存权限档，
    // 留下来就是一条假的历史记录）
    events.add(
      utf8.encode(
        _sse(
              3,
              'message.completed',
              jsonEncode({
                'messageId': 'a1',
                'text': '改好了。',
                'steps': 0,
                'reasoningEffort': 'off',
              }),
            ) +
            _sse(4, 'run.completed', '{"runId":"r1"}'),
      ),
    );
    unawaited(events.close());
    await tester.pumpAndSettle();
    expect(
      find.textContaining('本轮权限是 '),
      findsNothing,
      reason: '历史气泡上挂"现在的档位"是假的，必须收掉',
    );
  });
}
