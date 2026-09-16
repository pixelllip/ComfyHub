// AI 工作台页面回归（AIH-001 / AIH-002 / AIH-029 / AIH-048 / AIH-053）：
//
//   - 宽屏三栏 / 窄屏不出现三栏；
//   - Composer 始终可用；
//   - 模型能力徽标（文本/图片/视频/音频/文档/工具）按目录声明显示，不猜；
//   - 附件准入不通过时给出**具体原因**，并且发送按钮不可用（阻断而非"先发再说"）。

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

/// 最近一次 Run 请求体：用例据此断言"思考强度到底有没有真的发出去"。
Map<String, dynamic>? lastRunBody;

/// 本次用例里所有 Run 请求体（按顺序）。
final List<Map<String, dynamic>> lastRunBodies = [];

/// 事件流请求打到的 runId（按顺序）。
final List<String> sseProbe = [];

/// 按真实 SSE 格式拼事件（**冒号后必须有空格**，否则 `event:` 前缀匹配不上，
/// 客户端会把整个事件当成未知行丢掉 —— 这里踩过一次，别省这个空格）。
String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

/// SSE 响应必须给出 **UTF-8 字节**：正文里有中文，直接当字符串塞进 `http.Response`
/// 会被按 Latin-1 编码，客户端解码时炸成 "Contains invalid characters"。
http.Response _sseResponse(String body) => http.Response.bytes(
      utf8.encode(body),
      200,
      headers: {'content-type': 'text/event-stream; charset=utf-8'},
    );

/// 一个「纯文本模型」的假后端：任何图片附件都会被后端的 preflight 阻断。
MockClient _fakeBackend({
  List<String> modalities = const ['text'],
  Map<String, String>? thinkingEfforts,
  bool reasoning = false,
  String? thinkingFormat,
  Map<String, dynamic>? usage,
  bool failFirstRun = false,
}) {
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
          'inputModalities': modalities,
          'tools': true,
          'reasoning': reasoning,
          'thinkingEfforts': thinkingEfforts ?? <String, String>{},
          'thinkingFormat': thinkingFormat,
          'capabilitySource': 'manual',
          'enabled': true,
        }
      ];
    } else if (path == '/api/ai/conversations' && request.method == 'POST') {
      body = {
        'id': 'c1',
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = <Object>[];
    } else if (path == '/api/ai/conversations/c1/runs') {
      // 记录发出去的请求体，供用例断言"思考强度有没有真的带上""重试有没有带 retryOfRunId"
      lastRunBody = jsonDecode(request.body) as Map<String, dynamic>;
      lastRunBodies.add(lastRunBody!);
      runCount++;
      final id = 'r$runCount';
      body = {'runId': id, 'assistantMessageId': 'a$runCount', 'userMessageId': 'u1'};
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      final startedRunId = path.split('/')[4];
      sseProbe.add(startedRunId);
      // 第一次 Run 可以直接失败（重试用例）
      if (failFirstRun && startedRunId == 'r1') {
        final fail = _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(2, 'run.failed', '{"runId":"r1","code":"RATE_LIMIT","message":"上游限流"}');
        return _sseResponse(fail);
      }
      // 统一事件流：两段文本增量 + 完成（AIH-021）
      final sse = _sse(1, 'run.started', '{"runId":"$startedRunId"}') +
          _sse(2, 'message.started', '{"messageId":"a$runCount"}') +
          _sse(3, 'text.delta', '{"messageId":"a$runCount","text":"你好，"}') +
          _sse(4, 'text.delta', '{"messageId":"a$runCount","text":"我是假模型"}') +
          _sse(
            5,
            'message.completed',
            '{"messageId":"a$runCount","text":"你好，我是假模型",'
                '"reasoningEffort":"${lastRunBody?['reasoningEffort'] ?? 'off'}",'
                '"usage":${jsonEncode(usage ?? const {'inputTokens': 120, 'outputTokens': 30})}}',
          ) +
          _sse(6, 'run.completed', '{"runId":"$startedRunId"}');
      return _sseResponse(sse);
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = [
        {
          'id': 'u1',
          'conversationId': 'c1',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '你好',
          'parts': <Object>[],
        },
        {
          'id': failFirstRun && runCount < 2 ? 'a1' : 'a2',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          'status': failFirstRun && runCount < 2 ? 'failed' : 'complete',
          'text': failFirstRun && runCount < 2 ? '' : '你好，我是假模型',
          'usage': failFirstRun && runCount < 2
              ? null
              : (usage ?? const {'inputTokens': 120, 'outputTokens': 30}),
          'reasoningEffort': lastRunBody?['reasoningEffort'],
          'parts': <Object>[],
        },
      ];
    } else if (path == '/api/ai/preflight') {
      body = {
        'allowed': false,
        'blockers': ['模型 纯文本模型 未声明支持图片输入'],
      };
    } else if (path.startsWith('/api/capture')) {
      body = {
        'enabled': true,
        'comfyUrl': 'http://127.0.0.1:8188',
        'comfyReachable': true,
        'queueRunning': 1,
        'queuePending': 2,
        'capturedRuns': 3,
        'capturedMedia': 4,
        'recent': <Object>[],
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

Future<Widget> _page({
  List<String> modalities = const ['text'],
  Map<String, String>? thinkingEfforts,
  bool reasoning = false,
  String? thinkingFormat,
  Map<String, dynamic>? usage,
  bool failFirstRun = false,
}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(
      settings.baseUrl,
      client: _fakeBackend(
        modalities: modalities,
        thinkingEfforts: thinkingEfforts,
        reasoning: reasoning,
        thinkingFormat: thinkingFormat,
        usage: usage,
        failFirstRun: failFirstRun,
      ),
    ),
  );
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  );
}

void main() {
  setUp(() {
    lastRunBody = null;
    lastRunBodies.clear();
  });

  testWidgets('宽屏显示三栏：会话列表 / 对话 / 上下文', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('新建对话'), findsOneWidget);
    expect(find.text('AI 工作台'), findsOneWidget); // 空态标题
    expect(find.text('上下文'), findsOneWidget); // 右侧栏
    // 左侧栏和右侧栏同时存在，说明是三栏而不是单列
    expect(find.byType(VerticalDivider), findsNWidgets(2));
  });

  testWidgets('窄屏不显示侧栏，输入框仍然可用', (tester) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('上下文'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(find.text('发送'), findsOneWidget);
  });

  testWidgets('模型能力徽标只显示目录声明的能力，未声明的一律标不支持', (tester) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(modalities: ['text', 'image']));
    await tester.pumpAndSettle();

    // 打开模型选择器
    await tester.tap(find.text('纯文本模型').first);
    await tester.pumpAndSettle();
    expect(find.textContaining('文本 / 图片'), findsWidgets);
  });

  testWidgets('附件被准入阻断时，给出原因且发送按钮禁用', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    // 模拟用户添加了一张图片附件（能力由后端判定）
    store.addAttachment(const AiAttachment(
      name: 'poster.png',
      modality: 'image',
      mimeType: 'image/png',
      sizeBytes: 2048,
    ));
    await tester.pumpAndSettle();

    expect(find.textContaining('附件未通过准入'), findsOneWidget);
    expect(find.textContaining('未声明支持图片输入'), findsOneWidget);

    final send = tester.widget<FilledButton>(
      find.ancestor(of: find.text('发送'), matching: find.byType(FilledButton)),
    );
    expect(send.onPressed, isNull, reason: '准入不通过时不允许发送');

    // 预检结论是阻断，不允许被前端"放行"
    expect(store.preflight?.allowed, isFalse);
  });

  testWidgets('发送后按统一事件流逐段显示助手回复，结束后恢复发送按钮', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.selectedModel?.id, 'm-text', reason: '加载后应自动选中目录里的模型');

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 用户消息 + 助手回复都在（回复来自 SSE 的 text.delta / message.completed）
    expect(find.text('你好'), findsWidgets);
    expect(find.text('你好，我是假模型'), findsOneWidget);
    expect(store.sending, isFalse);
    expect(store.running, isFalse);
    expect(find.text('发送'), findsOneWidget, reason: '结束后按钮回到「发送」');
  });

  // --- 思考强度（AIH-056） ------------------------------------------------

  testWidgets('模型没声明思考档位时选择器置灰，并说明原因', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('不支持思考'), findsOneWidget, reason: '没声明就别给一堆选了会被拒的选项');
    final picker = tester.widget<PopupMenuButton<AiReasoningEffort>>(
      find.byType(PopupMenuButton<AiReasoningEffort>),
    );
    expect(picker.enabled, isFalse);
  });

  testWidgets('只列出模型声明过的档位，选中的档位随请求发给后端', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    lastRunBody = null;

    await tester.pumpWidget(await _page(
      reasoning: true,
      thinkingFormat: 'deepseek',
      thinkingEfforts: {'off': 'none', 'low': 'low', 'high': 'high'},
    ));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    // 目录只声明了三档，所以界面上也只能有这三档（medium / max 不出现）
    expect(store.selectedModel!.selectableEfforts, [
      AiReasoningEffort.off,
      AiReasoningEffort.low,
      AiReasoningEffort.high,
    ]);
    expect(find.text('关闭'), findsOneWidget, reason: '默认关闭');

    await tester.tap(find.byType(PopupMenuButton<AiReasoningEffort>));
    await tester.pumpAndSettle();
    expect(find.text('低'), findsWidgets);
    expect(find.text('中'), findsNothing, reason: '未声明的档位不能出现');

    await tester.tap(find.text('高').last);
    await tester.pumpAndSettle();
    expect(store.reasoningEffort, AiReasoningEffort.high);

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(lastRunBody?['reasoningEffort'], 'high', reason: '选了的档位必须真的带上');
  });

  testWidgets('选关闭时不把思考参数发给后端', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    lastRunBody = null;

    await tester.pumpWidget(await _page(
      reasoning: true,
      thinkingEfforts: {'low': 'low', 'high': 'high'},
    ));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(lastRunBody?.containsKey('reasoningEffort'), isFalse);
  });

  // --- token 统计（AIH-057） ---------------------------------------------

  testWidgets('助手消息显示归一化后的 token 用量，输入区显示本对话汇总', (tester) async {
    tester.view.physicalSize = const Size(1200, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(
      usage: {'inputTokens': 1200, 'outputTokens': 300, 'cachedTokens': 400},
    ));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.usageSummary.isEmpty, isTrue, reason: '还没对话时不该显示统计');

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 单条消息：↑1.2k ↓300
    expect(find.text('↑1.2k ↓300'), findsOneWidget);
    // 本对话汇总（输入区）
    expect(find.textContaining('本对话 ↑1.2k ↓300'), findsOneWidget);
    expect(store.usageSummary.totalTokens, 1500);
    expect(store.usageSummary.cachedTokens, 400);
    expect(store.usageSummary.requests, 1);
  });

  // --- 重试（AIH-024） ---------------------------------------------------

  testWidgets('失败的回复给出「重试」，重试会新开 Run 并带上 retryOfRunId', (tester) async {
    tester.view.physicalSize = const Size(1200, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(failFirstRun: true));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 第一次失败：错误可见 + 有重试入口
    expect(find.textContaining('上游限流'), findsOneWidget);
    expect(find.text('重试'), findsOneWidget);
    expect(lastRunBodies.length, 1);
    expect(lastRunBodies.first.containsKey('retryOfRunId'), isFalse, reason: '首次请求不该带重试标记');

    await tester.tap(find.text('重试'));
    await tester.pumpAndSettle();

    // 第二次是**新 Run**，且通过 retryOfRunId 关联回第一次
    expect(lastRunBodies.length, 2, reason: '重试应当新开一个 Run，而不是复用旧的');
    expect(lastRunBodies.last['retryOfRunId'], 'r1');
    expect(lastRunBodies.last['text'], '你好', reason: '重试要重放原来的问题');
    expect(find.text('你好，我是假模型'), findsOneWidget);
    expect(find.text('重试'), findsNothing, reason: '成功之后不该还留着重试按钮');
    expect(store.sending, isFalse);
  });

  testWidgets('后端没给 usage 时消息上不显示假的 token 数字', (tester) async {
    tester.view.physicalSize = const Size(1200, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(usage: const {}));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.textContaining('↑'), findsNothing, reason: '没有 usage 就不该编一个出来');
    expect(find.text('你好，我是假模型'), findsOneWidget);
  });
}
