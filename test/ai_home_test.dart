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

/// 一个「纯文本模型」的假后端：任何图片附件都会被后端的 preflight 阻断。
MockClient _fakeBackend({
  List<String> modalities = const ['text'],
  Map<String, String>? thinkingEfforts,
  bool reasoning = false,
  String? thinkingFormat,
  Map<String, dynamic>? usage,
}) {
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
      // 记录发出去的思考强度，供用例断言"到底有没有真的带上"
      lastRunBody = jsonDecode(request.body) as Map<String, dynamic>;
      body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
    } else if (path == '/api/ai/runs/r1/events') {
      // 统一事件流：两段文本增量 + 完成（AIH-021）
      final sse = 'id: 1\nevent: run.started\ndata: {"runId":"r1"}\n\n'
          'id: 2\nevent: message.started\ndata: {"messageId":"a1"}\n\n'
          'id: 3\nevent: text.delta\ndata: {"messageId":"a1","text":"你好，"}\n\n'
          'id: 4\nevent: text.delta\ndata: {"messageId":"a1","text":"我是假模型"}\n\n'
          'id: 5\nevent: message.completed\ndata: {"messageId":"a1","text":"你好，我是假模型",'
          '"reasoningEffort":"${lastRunBody?['reasoningEffort'] ?? 'off'}",'
          '"usage":${jsonEncode(usage ?? const {'inputTokens': 120, 'outputTokens': 30})}}\n\n'
          'id: 6\nevent: run.completed\ndata: {"runId":"r1"}\n\n';
      return http.Response(sse, 200, headers: {'content-type': 'text/event-stream'});
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
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          'status': 'complete',
          'text': '你好，我是假模型',
          'usage': usage ?? const {'inputTokens': 120, 'outputTokens': 30},
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
