// AI 工作台页面回归（AIH-001 / AIH-002 / AIH-029 / AIH-048 / AIH-053）：
//
//   - 宽屏三栏 / 窄屏不出现三栏；
//   - Composer 始终可用；
//   - 模型能力徽标（文本/图片/视频/音频/文档/工具）按目录声明显示，不猜；
//   - 附件准入不通过时给出**具体原因**，并且发送按钮不可用（阻断而非"先发再说"）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/theme.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/widgets/app_menu.dart';

/// 最近一次 Run 请求体：用例据此断言"思考强度到底有没有真的发出去"。
Map<String, dynamic>? lastRunBody;

/// 本次用例里所有 Run 请求体（按顺序）。
final List<Map<String, dynamic>> lastRunBodies = [];

/// 事件流请求打到的 runId（按顺序）。
final List<String> sseProbe = [];

/// 被删除的会话 id（按顺序）：用于断言"空会话切换时自动删掉"。
final List<String> deletedConversations = [];

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
  String conversationId = 'c1',
  List<Map<String, dynamic>> conversations = const [],
  // 模型显示名可以换：名字长的模型会把输入区底行挤紧（用户 bug 的复现条件之一）
  String modelName = '纯文本模型',
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
          'displayName': modelName,
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
        'id': conversationId,
        'title': '新对话',
        'providerId': 'local-gw',
        'modelId': 'm-text',
        'messageCount': 0,
      };
    } else if (path == '/api/ai/conversations') {
      body = conversations;
    } else if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        request.method == 'DELETE') {
      deletedConversations.add(path.split('/').last);
      body = {'deleted': true, 'id': path.split('/').last};
    } else if (path == '/api/ai/conversations/$conversationId/runs') {
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
    } else if (path == '/api/ai/conversations/$conversationId/messages') {
      body = [
        {
          'id': 'u1',
          'conversationId': conversationId,
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '你好',
          'parts': <Object>[],
        },
        {
          'id': failFirstRun && runCount < 2 ? 'a1' : 'a2',
          'conversationId': conversationId,
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
  String conversationId = 'c1',
  List<Map<String, dynamic>> conversations = const [],
  ThemeData? theme,
  String modelName = '纯文本模型',
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
        conversationId: conversationId,
        conversations: conversations,
        modelName: modelName,
      ),
    ),
  );
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: MaterialApp(theme: theme, home: const AiHomePage()),
  );
}

void main() {
  setUp(() {
    lastRunBody = null;
    lastRunBodies.clear();
    deletedConversations.clear();
  });

  testWidgets('宽屏显示三栏：会话列表 / 对话 / 右侧状态栏', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('新建对话'), findsOneWidget);
    expect(find.text('AI 工作台'), findsOneWidget); // 空态标题
    // 右侧栏用「ComfyUI」/ Skills 目录做锚点：以前那栏顶部写着"上下文"，
    // 但栏里其实只有模型能力 + ComfyUI 状态 + Skills，名不副实（用户建议第 5 条）。
    expect(find.text('ComfyUI'), findsOneWidget);
    expect(find.text('上下文'), findsNothing, reason: '「上下文」这个含混的标题已经去掉');
    // 左侧栏和右侧栏同时存在，说明是三栏而不是单列
    expect(find.byType(VerticalDivider), findsNWidgets(2));
  });

  testWidgets('900~1199px：第三栏铺不开，但 Comfy 状态仍然可达（AIH-002 死角）', (tester) async {
    tester.view.physicalSize = const Size(1000, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    // 这一段仍然是两栏（900px 宽时再塞一栏，正文区只剩约 400px，太挤）
    expect(find.byType(VerticalDivider), findsOneWidget);
    expect(find.text('ComfyUI'), findsNothing);

    // 但入口必须在：这一段既不是 <900 的右下角 FAB，也不是 ≥1200 的第三栏，
    // 曾经两边都不占 → Comfy 状态整块内容在界面上不可达（需求审计 AIH-002）。
    final entry = find.byTooltip('ComfyUI 状态');
    expect(entry, findsOneWidget, reason: 'AIH-002：900~1199px 必须有一个 Comfy 状态入口');

    await tester.tap(entry);
    await tester.pumpAndSettle();
    expect(find.text('ComfyUI'), findsWidgets, reason: '点开就能看到状态内容');
  });

  testWidgets('≥1200px 有第三栏，就不必再占标题栏一个入口', (tester) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('ComfyUI'), findsOneWidget);
    expect(find.byTooltip('ComfyUI 状态'), findsNothing);
  });

  testWidgets('会话列表：当前会话有明确底色，按下 / 悬停高亮被压淡（用户 bug：次新那条看着像被选中）',
      (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 用 App 真正在跑的深色主题：Material 默认的按下高亮（~38% 白）只在深色下才这么抢眼，
    // 拿默认浅色主题测等于没测。
    await tester.pumpWidget(await _page(
      theme: AppTheme.build(Brightness.dark),
      conversations: [
        {'id': 'cA', 'title': '会话 A', 'messageCount': 2},
        {'id': 'cB', 'title': '会话 B', 'messageCount': 4},
      ],
    ));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    await store.openConversation('cA');
    await tester.pumpAndSettle();

    ListTile tileOf(String title) =>
        tester.widget<ListTile>(find.widgetWithText(ListTile, title));

    // ListTile 的底色由它自己那层 Ink 的 ShapeDecoration 画出来
    Color? tileBackground(String title) {
      final ink = tester.widget<Ink>(find
          .descendant(of: find.widgetWithText(ListTile, title), matching: find.byType(Ink))
          .first);
      final decoration = ink.decoration;
      return decoration is ShapeDecoration ? decoration.color : null;
    }

    // ① 当前会话必须有**看得见的底色**。
    //    M3 的 ListTile 选中态默认只把文字染成主题色、没有底色，于是"鼠标压过的那一行"
    //    反而比真选中的更像选中 —— 用户报的就是这个。
    expect(tileOf('会话 A').selected, isTrue);
    expect(tileOf('会话 B').selected, isFalse);
    expect(tileBackground('会话 A')?.a, greaterThan(0.1),
        reason: '当前会话的底色要看得出来，不能是透明或接近透明');
    expect(tileBackground('会话 B'), anyOf(isNull, isA<Color>().having((c) => c.a, 'alpha', 0)),
        reason: '没打开的会话不该有底色');

    // ② 按下 / 水波纹高亮必须压淡：它比选中态还亮，而且窗口在收到 mouse-up 之前
    //    失去焦点 / 被最小化时会**卡在屏幕上**，看着就是"某一条被选中了"。
    final tileTheme = Theme.of(tester.element(find.widgetWithText(ListTile, '会话 A')));
    expect(tileTheme.highlightColor.a, lessThanOrEqualTo(0.12),
        reason: '按下高亮必须比选中底色淡，卡住也不刺眼');
    expect(tileTheme.splashColor.a, lessThanOrEqualTo(0.12));
  });

  testWidgets('窄屏不显示侧栏，输入框仍然可用', (tester) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    expect(find.text('ComfyUI'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(find.text('发送'), findsOneWidget);
  });

  testWidgets('冷启动落在新建的聊天记录上，而不是重新打开上次那条', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.conversation?.id, 'c1');
    expect(store.messages, isEmpty, reason: '新开一条，不该带出历史消息');
    expect(find.text('AI 工作台'), findsOneWidget, reason: '空态页面 = 新会话');
  });

  testWidgets('输入框草稿按会话保存：切走再切回来文字还在', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    await tester.enterText(find.byType(TextField), '打了一半的需求');
    await tester.pumpAndSettle();

    // 输入停下就已经落盘（不用等发送或关窗口）
    final saved = await store.loadDraft('c1');
    expect(saved.text, '打了一半的需求');

    // 清空输入 → 草稿一起清掉，不会下次又冒出来
    await tester.enterText(find.byType(TextField), '');
    await tester.pumpAndSettle();
    expect((await store.loadDraft('c1')).text, isEmpty);
  });

  testWidgets('切走时把空会话删掉，有内容的会话留着', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.isEmptyConversation, isTrue);

    // 再点一次「新建对话」：上一个一条消息都没有的会话应该被清掉
    await store.newConversation();
    await tester.pumpAndSettle();

    expect(deletedConversations, contains('c1'), reason: '一条消息都没有的空会话应该被清掉');
  });

  testWidgets('有内容的会话切走时不会被删', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();
    expect(store.isEmptyConversation, isFalse);

    await store.newConversation();
    await tester.pumpAndSettle();

    expect(deletedConversations, isNot(contains('c1')), reason: '聊过的会话不能顺手删掉');
  });

  testWidgets('记住上次用的模型：下一次加载直接选中它', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page());
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    await store.selectModel(store.models.first);
    await tester.pumpAndSettle();

    // 新建一个 store（同一份 SharedPreferences）→ 模拟下次冷启动
    final second = AiWorkspaceStore(api: AiApiClient('http://127.0.0.1:8080', client: _fakeBackend()));
    await second.load();
    expect(second.selectedModel?.id, 'm-text', reason: '上次选的模型要自动选回来');
    expect(second.selectedProvider?.id, 'local-gw');
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
    // 没有可选档位时按钮不可点（点到也不会弹菜单）：InkWell.onTap 为 null
    final button = tester.widget<InkWell>(
      find.ancestor(
        of: find.text('不支持思考'),
        matching: find.byType(InkWell),
      ).first,
    );
    expect(button.onTap, isNull, reason: '没档位时不该给出可点的选择器');
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

    await tester.tap(find.byType(AppMenuButton<AiReasoningEffort>));
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

  testWidgets('输入区底行：token 汇总不被挤没，发送按钮永远贴右（用户 bug）', (tester) async {
    // 1440 宽 = 用户那台机器的窗口：中栏 = 1440 - 240(会话列表) - 260(右栏) - 2 = 938，
    // 模型名长 + 能力徽标多的时候底行会被挤紧，正好复现"汇总被省略 / 按钮不贴右"。
    tester.view.physicalSize = const Size(1440, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(
      modelName: 'DeepSeek V4.1 Flash',
      modalities: const ['text', 'image', 'video', 'audio'],
      usage: {'inputTokens': 6400, 'outputTokens': 1500},
    ));
    await tester.pumpAndSettle();

    final store = tester.element(find.byType(AiHomePage)).read<AiWorkspaceStore>();
    expect(store.usageSummary.isEmpty, isTrue, reason: '还没对话时不显示统计');

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    final usage = find.textContaining('本对话 ↑');
    expect(usage, findsOneWidget);

    // ① 汇总没被挤掉：它的渲染宽度 == 自己的固有宽度。
    //    被省略号截断时宽度会被压到上限（220），这条就会红。
    final para = tester.renderObject<RenderParagraph>(usage);
    expect(
      para.size.width,
      greaterThanOrEqualTo(para.getMaxIntrinsicWidth(double.infinity) - 1),
      reason: 'token 汇总不该被别的控件挤到省略',
    );

    // ② 发送按钮贴在输入区内容的最右边：以前汇总一旦用不满自己那份 flex，
    //    多出来的空白就落在按钮右边，按钮于是不贴右。
    final send = tester.getRect(
      find.ancestor(of: find.text('发送'), matching: find.byType(FilledButton)),
    );
    final input = tester.getRect(find.byType(TextField));
    expect(send.right, closeTo(input.right, 1), reason: '发送按钮必须贴右（不能被顶出去、也不该缩在中间）');
    // 按钮右边不该再有空白：贴右的另一种写法是"右边还剩一截"
    expect(send.right, greaterThanOrEqualTo(input.right - 1));
    // 汇总排在按钮左边（中间只隔 12px），不是被甩到别处
    expect(send.left, greaterThan(para.localToGlobal(Offset.zero).dx));
  });

  testWidgets('输入区底行：窗口宽裕时发送按钮右边也不留空白（用户 bug）', (tester) async {
    // 1600 宽 → 中栏 1098：底行非常宽裕。旧写法里 `Spacer` 只吃掉一半剩余空间、
    // 汇总又是 loose 的 `Flexible`，用不满的那半空白就落在**按钮右边** ——
    // 表现就是"发送按钮没有固定在最右侧"。
    tester.view.physicalSize = const Size(1600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _page(
      modelName: 'GPT-4o',
      usage: {'inputTokens': 1200, 'outputTokens': 300},
    ));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    final send = tester.getRect(
      find.ancestor(of: find.text('发送'), matching: find.byType(FilledButton)),
    );
    final input = tester.getRect(find.byType(TextField));
    expect(send.right, closeTo(input.right, 1), reason: '宽裕时按钮右边也不该留一截空白');
    expect(find.textContaining('本对话 ↑1.2k ↓300'), findsOneWidget);
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
