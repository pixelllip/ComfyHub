// AI 工作台页面回归（AIH-001 / AIH-002 / AIH-029 / AIH-048 / AIH-053）：
//
//   - 宽屏三栏 / 窄屏不出现三栏；
//   - Composer 始终可用；
//   - 模型能力徽标（文本/图片/视频/音频/文档/工具）按目录声明显示，不猜；
//   - 附件准入不通过时给出**具体原因**，并且发送按钮不可用（阻断而非"先发再说"）。

import 'dart:convert';

import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride;
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/layout_constants.dart';
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
/// 会被按 Latin-1 编码，客户端解码时报错 "Contains invalid characters"。
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
  // 模型显示名可替换：名称较长的模型会使输入区底行变紧（复现条件之一）
  String modelName = '纯文本模型',
  // 权限档：`full` 时那枚按钮的文案是「自动允许（无需批准）」（底行最宽的一枚控件）
  String permissionMode = AiToolPolicy.modeAsk,
  // 画风卡列表可替换：重拉卡片库的用例要在两次打开之间改这张表
  List<Map<String, dynamic>>? styleCards,
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
          'credential': {
            'configured': true,
            'source': 'managed',
            'writable': true,
          },
        },
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
        },
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
      // 真后端是 `ORDER BY c.updated_at DESC`，照做 —— 用例里"聊完顶到最前"才有意义
      body = [...conversations]
        ..sort(
          (a, b) => ((b['updatedAt'] ?? '') as String).compareTo(
            (a['updatedAt'] ?? '') as String,
          ),
        );
    } else if (RegExp(r'^/api/ai/conversations/[^/]+$').hasMatch(path) &&
        request.method == 'DELETE') {
      deletedConversations.add(path.split('/').last);
      body = {'deleted': true, 'id': path.split('/').last};
    } else if (path == '/api/ai/conversations/$conversationId/runs') {
      // 记录发出去的请求体，供用例断言"思考强度有没有真的带上""重试有没有带 retryOfRunId"
      lastRunBody = jsonDecode(request.body) as Map<String, dynamic>;
      lastRunBodies.add(lastRunBody!);
      runCount++;
      // 真后端在发消息时会 `UPDATE ai_conversations SET updated_at = …` 并把 message_count 顶上去
      // （见 AiConversationRepo.appendMessage）；这里也照做，否则用例结尾那次"对齐一次列表"
      // 会把界面刚写上的"刚刚"打回 fixture 里的旧时间。
      for (final c in conversations) {
        if (c['id'] == conversationId) {
          c['messageCount'] = ((c['messageCount'] as num?)?.toInt() ?? 0) + 2;
          c['updatedAt'] = DateTime.now().toUtc().toIso8601String();
        }
      }
      final id = 'r$runCount';
      body = {
        'runId': id,
        'assistantMessageId': 'a$runCount',
        'userMessageId': 'u1',
      };
    } else if (path.startsWith('/api/ai/runs/') && path.endsWith('/events')) {
      final startedRunId = path.split('/')[4];
      sseProbe.add(startedRunId);
      // 第一次 Run 可以直接失败（重试用例）
      if (failFirstRun && startedRunId == 'r1') {
        final fail =
            _sse(1, 'run.started', '{"runId":"r1"}') +
            _sse(
              2,
              'run.failed',
              '{"runId":"r1","code":"RATE_LIMIT","message":"上游限流"}',
            );
        return _sseResponse(fail);
      }
      // 统一事件流：两段文本增量 + 完成（AIH-021）
      final sse =
          _sse(1, 'run.started', '{"runId":"$startedRunId"}') +
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
    } else if (path == '/api/ai/character-cards') {
      body = [
        {
          'id': 1,
          'name': '凛',
          'prompt': 'Rin, black hair in a low ponytail, grey eyes, dry skin.',
          'examples': <Object>[],
        },
        {
          'id': 3,
          'name': '诗音',
          'prompt': 'Shion, silver bob cut, amber eyes, a thin scar on the left brow.',
          'examples': <Object>[],
        },
      ];
    } else if (path == '/api/ai/style-cards') {
      body =
          styleCards ??
          [
            {
              'id': 2,
              'name': '物理感动画',
              'prompt': 'Physically based anime rendering: three-dimensional matte shading.',
              'negative': 'soft shading, blurry',
              'examples': <Object>[],
            },
          ];
    } else if (path == '/api/ai/tools/policy') {
      // 权限档的真源：界面据此决定按钮显示的档位
      body = {'permissionMode': permissionMode};
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

/// 造一个只在内存里跑的 AI 工作台页面（假后端 + mock 设置）。
///
/// **对测试文件公开**（不带下划线）：别的回归用例需要"同一个页面的另一套窗口尺寸"
/// 时直接复用，别各自再抄一份假后端 —— 抄出来的第二份早晚会跟这份对不上。
Future<Widget> buildAiHomePage({
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
  String permissionMode = AiToolPolicy.modeAsk,
  List<Map<String, dynamic>>? styleCards,
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
        permissionMode: permissionMode,
        styleCards: styleCards,
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    expect(find.text('新建对话'), findsOneWidget);
    expect(find.text('AI 工作台'), findsOneWidget); // 空态标题
    // 右侧栏用「ComfyUI」/ Skills 目录做锚点：以前那栏顶部写着"上下文"，
    // 但栏中只有模型能力 + ComfyUI 状态 + Skills，与名称不符。
    expect(find.text('ComfyUI'), findsOneWidget);
    expect(find.text('上下文'), findsNothing, reason: '「上下文」这个含混的标题已经去掉');
    // 左侧栏和右侧栏同时存在，说明是三栏而不是单列
    expect(find.byKey(const Key('ai-conversation-pane')), findsOneWidget);
    expect(find.byKey(const Key('ai-context-pane')), findsOneWidget);
    // 三栏之间**不画分隔线**（用户要求：模块边界用底色差表达，不用线条）。
    expect(
      find.byType(VerticalDivider),
      findsNothing,
      reason: '三栏之间不许再有贯穿整屏的竖线',
    );

    final left = tester.getRect(find.byKey(const Key('ai-conversation-pane')));
    final right = tester.getRect(find.byKey(const Key('ai-context-pane')));
    expect(left.width, kAiConversationPaneWidth);
    expect(right.width, kAiContextPaneWidth);
    expect(
      right.left - left.right - kAiResizeHandleWidth * 2,
      greaterThanOrEqualTo(kAiCenterMinWidth),
      reason: '默认宽度下中栏要足够宽',
    );
  });

  testWidgets('900~1199px：第三栏铺不开，但 Comfy 状态仍然可达（AIH-002 死角）', (tester) async {
    tester.view.physicalSize = const Size(1000, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    // 这一段仍然是两栏（900px 宽时再塞一栏，正文区只剩约 400px，太挤）
    expect(find.byKey(const Key('ai-conversation-pane')), findsOneWidget);
    expect(find.byKey(const Key('ai-context-pane')), findsNothing);
    expect(find.byType(VerticalDivider), findsNothing);
    expect(find.text('ComfyUI'), findsNothing);

    // 但入口必须在：这一段既不是 <900 的右下角 FAB，也不是 ≥1200 的第三栏，
    // 曾经两边都不占 → Comfy 状态整块内容在界面上不可达（需求审计 AIH-002）。
    // 2026-11-05 起顶栏收敛成「主操作 + 更多菜单」，入口在菜单里。
    expect(
      find.byTooltip('更多操作'),
      findsOneWidget,
      reason: 'AIH-002：900~1199px 必须有一个通往状态的入口',
    );

    await tester.tap(find.byTooltip('更多操作'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上下文与状态'));
    await tester.pumpAndSettle();
    expect(find.text('上下文与状态'), findsOneWidget, reason: '入口名字要和面板内容一致');
    expect(find.text('ComfyUI'), findsWidgets, reason: '点开就能看到状态内容');
  });

  testWidgets('≥1200px 有第三栏，就不必再占标题栏一个入口', (tester) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    expect(find.text('ComfyUI'), findsOneWidget);
    // 第三栏已经铺开：菜单里不该再有"ComfyUI 与上下文"（入口只留一个）。
    await tester.tap(find.byTooltip('更多操作'));
    await tester.pumpAndSettle();
    expect(find.text('上下文与状态'), findsNothing);
  });

  // 右侧栏除实时进度外的内容也使用相同容器分组。
  //
  // 以前只有"实时进度"是 Card，其余几块（模型能力 / ComfyUI / Skills / 长期记忆）
  // 是直接摊在背景上的文字与列表 —— 一眼看不出哪几行属于同一块、到哪里算下一块。
  testWidgets('右侧栏每块各自一个卡片容器（用户建议）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    // 四块各有一张卡（Skills 那张是 `DecoratedSliver`：条目要懒构建，见页面里的注释）
    for (final key in const [
      'ai-panel-card-model',
      'ai-panel-card-comfy',
      'ai-panel-card-skills',
      'ai-panel-card-memory',
    ]) {
      expect(find.byKey(Key(key)), findsOneWidget, reason: '$key 这块要有自己的容器');
    }

    // 三张能取到矩形的卡自上而下依次排开、互不重叠
    final model = tester.getRect(find.byKey(const Key('ai-panel-card-model')));
    final comfy = tester.getRect(find.byKey(const Key('ai-panel-card-comfy')));
    final memory = tester.getRect(
      find.byKey(const Key('ai-panel-card-memory')),
    );
    expect(comfy.top, greaterThan(model.bottom), reason: '两块之间要断开');
    expect(memory.top, greaterThan(comfy.bottom), reason: '两块之间要断开');

    // 卡片外观统一：Skills 那张（sliver）与其余几张必须是同一套底色 / 圆角
    final modelCard =
        tester
                .widget<DecoratedBox>(
                  find
                      .descendant(
                        of: find.byKey(const Key('ai-panel-card-model')),
                        matching: find.byType(DecoratedBox),
                      )
                      .first,
                )
                .decoration
            as BoxDecoration;
    final skillsCard =
        tester
                .widget<DecoratedSliver>(
                  find.byKey(const Key('ai-panel-card-skills')),
                )
                .decoration
            as BoxDecoration;
    expect(skillsCard.color, modelCard.color, reason: 'Skills 那张卡的底色要跟别的卡一样');
    expect(skillsCard.borderRadius, modelCard.borderRadius);
  });

  // 聊天主体与两侧边栏之间存在大片空白，空间利用率低；
  // 滚动条需贴边，气泡与边缘距离为 20 到 30 像素。
  //
  // 原因是两处**默认对齐**，仅调整主题中的 `crossAxisMargin` 无法解决：
  //   ① `_Thread` 的 Column 未设置 crossAxisAlignment（默认 center）→ 消息区缩至
  //      "最宽气泡 + 内边距"（720 + 32 = 752）后**居中**，两侧留白；
  //      滚动条画在 752 区域的右边缘，与右侧栏距离较远（未贴边）；
  //   ② `_MessageList` 的 Column 使用 start → 每条消息只有气泡宽度，
  //      `_MessageBubble` 中"请求消息靠右"（end）因此**没有生效**。
  testWidgets('聊天主体铺满整栏：气泡距边缘 24、滚动条贴右侧边栏（用户建议）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    // 发送一条短消息：一条请求气泡 + 一条助手气泡，均短于整栏，
    // 这样才量得到它们各自贴的是哪一边
    await tester.enterText(find.byType(TextField).first, '短消息');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 三栏之间没有分隔线，栏宽直接取两栏的实际矩形：
    // 消息区两侧各让出一条拖动条的宽度。
    final leftPane = tester.getRect(
      find.byKey(const Key('ai-conversation-pane')),
    );
    final rightPane = tester.getRect(find.byKey(const Key('ai-context-pane')));
    final paneLeft = leftPane.right + kAiResizeHandleWidth;
    final paneRight = rightPane.left - kAiResizeHandleWidth;
    final list = tester.getRect(find.byKey(const Key('ai-message-list')));

    expect(list.left, closeTo(paneLeft, 0.5), reason: '消息区要从左边栏与拖动条之后开始');
    expect(
      list.right,
      closeTo(paneRight, 0.5),
      reason: '消息区要铺到右侧栏之前 —— 滚动条就画在这个右边缘，这才叫"贴边"',
    );
    expect(
      list.width,
      greaterThan(900),
      reason: '1600 宽的窗口下不许再缩成 752 居中（旧行为：两侧各空一大片）',
    );

    final userBubble = tester.getRect(
      find.byKey(const ValueKey('ai-bubble-u1')).first,
    );
    // 助手那条：一轮跑完界面会与服务端对齐一次（`_reloadMessages`），
    // 所以 id 是假后端 `/messages` 里给的那个（a2），不是乐观插入时的 a1
    final assistantBubble = tester.getRect(
      find.byKey(const ValueKey('ai-bubble-a2')).first,
    );
    expect(
      assistantBubble.left,
      closeTo(paneLeft + 24, 0.5),
      reason: '助手气泡距左边栏 24',
    );
    expect(
      userBubble.right,
      closeTo(paneRight - 24, 0.5),
      reason: '用户气泡要贴右（旧行为：item 缩成气泡那么宽，end 对齐等于没生效）',
    );
  });

  // 左右侧栏可以拖动加宽（用户建议）：下限就是拖动出现之前的 240 / 260，
  // 上限 480，且两栏加起来必须给中栏留出 kAiCenterMinWidth；松手后宽度落盘。
  testWidgets('左右侧栏可拖动加宽，下限与中栏保底都成立（用户建议）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    double widthOf(String key) => tester.getRect(find.byKey(Key(key))).width;

    // ① 往左拖：已经在下限，宽度不许再变小
    await tester.drag(
      find.byKey(const Key('ai-conversation-resize')),
      const Offset(-120, 0),
    );
    await tester.pumpAndSettle();
    expect(widthOf('ai-conversation-pane'), kAiConversationPaneWidth);

    // ② 往右拖 80：会话列表加宽 80
    await tester.drag(
      find.byKey(const Key('ai-conversation-resize')),
      const Offset(80, 0),
    );
    await tester.pumpAndSettle();
    expect(
      widthOf('ai-conversation-pane'),
      closeTo(kAiConversationPaneWidth + 80, 0.5),
    );

    // ③ 右栏的拖动条在它左侧：往左拖 90 = 加宽 90
    await tester.drag(
      find.byKey(const Key('ai-context-resize')),
      const Offset(-90, 0),
    );
    await tester.pumpAndSettle();
    expect(widthOf('ai-context-pane'), closeTo(kAiContextPaneWidth + 90, 0.5));

    // ④ 拖过上限：停在最宽处，中栏没有被压没
    await tester.drag(
      find.byKey(const Key('ai-conversation-resize')),
      const Offset(600, 0),
    );
    await tester.pumpAndSettle();
    expect(widthOf('ai-conversation-pane'), kAiSidePaneMaxWidth);
    // 中栏（两栏之间，含两条拖动条）仍在保底宽度以上
    final left = tester.getRect(find.byKey(const Key('ai-conversation-pane')));
    final right = tester.getRect(find.byKey(const Key('ai-context-pane')));
    expect(
      right.left - left.right - kAiResizeHandleWidth * 2,
      greaterThanOrEqualTo(kAiCenterMinWidth),
    );

    // ⑤ 松手后宽度落盘（下次启动沿用），拖动过程不写偏好
    final settings = tester
        .element(find.byType(AiHomePage))
        .read<SettingsStore>();
    expect(settings.aiConversationPaneWidth, kAiSidePaneMaxWidth);
    expect(settings.aiContextPaneWidth, closeTo(kAiContextPaneWidth + 90, 0.5));
  });

  testWidgets('会话列表：显示最近一次对话时间，聊完立刻变成「刚刚」（用户建议 ⑥）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        conversations: [
          {
            'id': 'c2',
            'title': '五分钟前聊过的',
            'messageCount': 4,
            'updatedAt': DateTime.now()
                .toUtc()
                .subtract(const Duration(minutes: 5))
                .toIso8601String(),
          },
          // 后端给的是 UTC（`Instant.toString()`），前端要按本地时区显示才对
          {
            'id': 'c1',
            'title': '三天前聊过的',
            'messageCount': 0,
            'updatedAt': DateTime.now()
                .toUtc()
                .subtract(const Duration(days: 3))
                .toIso8601String(),
          },
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('4 条消息 · 5 分钟前'), findsOneWidget);
    expect(find.text('0 条消息 · 3 天前'), findsOneWidget);

    // 在空会话（c1）里发一条消息 → 那条会话的"最近一次对话"必须跟着变成刚刚，
    // 并且顶到列表最前（后端也按 updated_at DESC 排）
    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.textContaining('刚刚'), findsWidgets, reason: '刚发过消息的会话不能还写着旧时间');
    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    expect(store.conversation?.id, 'c1');
    expect(store.conversations.first.id, 'c1', reason: '刚聊过的会话要顶到列表最前');
  });

  testWidgets('会话列表：当前会话有明确底色，按下 / 悬停高亮被压淡（用户 bug：次新那条看着像被选中）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 用 App 真正在跑的深色主题：Material 默认的按下高亮（~38% 白）只在深色下才这么抢眼，
    // 拿默认浅色主题测等于没测。
    await tester.pumpWidget(
      await buildAiHomePage(
        theme: AppTheme.build(Brightness.dark),
        conversations: [
          {'id': 'cA', 'title': '会话 A', 'messageCount': 2},
          {'id': 'cB', 'title': '会话 B', 'messageCount': 4},
        ],
      ),
    );
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    await store.openConversation('cA');
    await tester.pumpAndSettle();

    ListTile tileOf(String title) =>
        tester.widget<ListTile>(find.widgetWithText(ListTile, title));

    // ListTile 的底色由它自己那层 Ink 的 ShapeDecoration 画出来
    Color? tileBackground(String title) {
      final ink = tester.widget<Ink>(
        find
            .descendant(
              of: find.widgetWithText(ListTile, title),
              matching: find.byType(Ink),
            )
            .first,
      );
      final decoration = ink.decoration;
      return decoration is ShapeDecoration ? decoration.color : null;
    }

    // ① 当前会话必须有**看得见的底色**。
    //    M3 的 ListTile 选中态默认只把文字染成主题色、没有底色，于是"鼠标压过的那一行"
    //    反而比选中态更接近选中。
    expect(tileOf('会话 A').selected, isTrue);
    expect(tileOf('会话 B').selected, isFalse);
    expect(
      tileBackground('会话 A')?.a,
      greaterThan(0.1),
      reason: '当前会话的底色要看得出来，不能是透明或接近透明',
    );
    expect(
      tileBackground('会话 B'),
      anyOf(isNull, isA<Color>().having((c) => c.a, 'alpha', 0)),
      reason: '没打开的会话不该有底色',
    );

    // ② 按下 / 水波纹高亮必须压淡：它比选中态还亮，而且窗口在收到 mouse-up 之前
    //    失去焦点 / 被最小化时会**卡在屏幕上**，看着就是"某一条被选中了"。
    final tileTheme = Theme.of(
      tester.element(find.widgetWithText(ListTile, '会话 A')),
    );
    expect(
      tileTheme.highlightColor.a,
      lessThanOrEqualTo(0.12),
      reason: '按下高亮必须比选中底色淡，卡住也不刺眼',
    );
    expect(tileTheme.splashColor.a, lessThanOrEqualTo(0.12));
  });

  testWidgets('窄屏不显示侧栏，输入框仍然可用', (tester) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    expect(find.text('ComfyUI'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(find.text('发送'), findsOneWidget);
  });

  // 现象："宽度低于一定值时控件显示不全；进一步降低宽度，
  // 当仅有聊天详情本身时，本来该出现在 appbar 里的 comfy UI 状态变成了盖住发送按钮的
  // 独立按钮，同时底下控件也显示不全。"
  //
  // 三个根因，三条用例各盯一个：
  //   ① 窄屏那一支用的是钉死在右下角的 `FloatingActionButton` —— 而输入区的「发送」
  //      按钮正好在右下角，于是窗口一窄就**被 FAB 盖住**（实测 320~899px 全部重叠，
  //      连发送按钮的点击面都被吃掉）。
  //   ② 输入区底行左侧操作组能横向滚动，但窄窗口下视口只剩 21px（内容 847px）——
  //      权限档 / 模型选择被从**左边缘**裁掉，显示不全且无法滚动。
  //   ③ 约束：**一行放不下时折行**，不使用横向滚动。
  testWidgets('窄屏：ComfyUI 状态入口在标题栏，不再有盖住发送按钮的 FAB', (tester) async {
    tester.view.physicalSize = const Size(420, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    // 全局唯一的 FAB 会压住输入区：**一个都不许有**（右下角那个位置属于「发送」）
    expect(
      find.byType(FloatingActionButton),
      findsNothing,
      reason: '窄屏的 Comfy 状态入口进了标题栏，不该再留一个 FAB 压住发送按钮',
    );

    final entry = find.byTooltip('更多操作');
    expect(entry, findsOneWidget, reason: '第三栏铺不开时，标题栏必须有一个可达的入口');

    final send = tester.getRect(find.widgetWithText(FilledButton, '发送'));
    expect(
      tester.getRect(entry).overlaps(send),
      isFalse,
      reason: '入口不能压在发送按钮上',
    );

    await tester.tap(entry);
    await tester.pumpAndSettle();
    await tester.tap(find.text('上下文与状态'));
    await tester.pumpAndSettle();
    expect(find.text('ComfyUI'), findsWidgets, reason: '点开就能看到状态内容');
  });

  testWidgets('窄屏输入区底行：紧凑图标 + 发送挤在一排，控件一个都不被裁', (tester) async {
    tester.view.physicalSize = const Size(360, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        modelName: 'DeepSeek V4.1 Flash',
        modalities: const ['text', 'image'],
        usage: {'inputTokens': 12167, 'outputTokens': 28},
        // 权限档切到"自动允许（无需批准）"：宽屏下这是底行最宽的那枚控件
        permissionMode: AiToolPolicy.modeFull,
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '你好');
    await tester.tap(find.widgetWithText(FilledButton, '发送'));
    await tester.pumpAndSettle();

    // 输入区宽度（360 - 两侧 12）= 336 < 440 → 控件退成**图标**档（见 `_ComposerLabel`）
    // ★ 底行**始终是一排**（两行排版 2026-10-11 已删）
    expect(
      find.byKey(const Key('ai-composer-two-row')),
      findsNothing,
      reason: '两行排版已删除：窄了该压文字，不该折成两行',
    );
    final input = tester.getRect(find.byType(TextField).first);
    final send = tester.getRect(find.widgetWithText(FilledButton, '发送'));
    final attach = tester.getRect(find.byTooltip('添加附件 / 插入卡片（@）'));
    expect(send.right, closeTo(input.right, 1), reason: '发送按钮贴右（和输入框右边缘对齐）');
    expect(
      (send.center.dy - attach.center.dy).abs(),
      lessThan(6),
      reason: '发送必须和操作控件同一排',
    );
    expect(tester.takeException(), isNull, reason: '360 宽下不许 overflow');

    // 四枚控件都还在（一个都没被丢掉），信息进 tooltip。
    //
    // 「模型选择」那条**按前缀找**：换一个 Provider 之后光看模型名认不出它是谁家的，
    // 所以紧凑形态的 tooltip 额外带着来源（2026-10：工作台的模型按 Provider 分组，
    // 来源要标出来）。那行来源跟着选中项变，逐字匹配会让这条用例一换 Provider 就红。
    final modelTooltip = find.byWidgetPredicate(
      (w) =>
          w is Tooltip &&
          (w.message ?? '').startsWith('当前模型是 DeepSeek V4.1 Flash（文本 / 图片）'),
    );
    expect(
      tester.widget<Tooltip>(modelTooltip).message,
      contains('来自 本机网关'),
      reason: 'tooltip 必须写出这个模型来自哪条 Provider',
    );
    final controls = <String, Finder>{
      '添加附件 / 插入卡片（@）': find.byTooltip('添加附件 / 插入卡片（@）'),
      '权限档': find.byTooltip(
        '当前是${AiToolPolicy.modeFullLabel}，AI 调用工具不会等你批准。'
        '可读 / 可写目录范围不变，越界仍会被拒。点一下改回「${AiToolPolicy.modeAskLabel}」。',
      ),
      '模型选择': modelTooltip,
      '思考强度': find.byTooltip('不支持思考。该模型未声明推理能力，请到「设置 → AI 模型」声明。'),
    };
    for (final entry in controls.entries) {
      expect(entry.value, findsWidgets, reason: '${entry.key} 必须还在底行里');
      final rect = tester.getRect(entry.value.last);
      expect(
        rect.left,
        greaterThanOrEqualTo(input.left - 0.5),
        reason: '${entry.key} 不能被从左边裁掉（用户报的"控件显示不全"）',
      );
      expect(
        rect.right,
        lessThanOrEqualTo(input.right + 0.5),
        reason: '${entry.key} 不能被从右边裁掉',
      );
    }

    // 四枚 + 发送：360（内容区 336）时整排一次放得下，发送收在最右
    for (final entry in controls.entries) {
      expect(
        (tester.getRect(entry.value.last).center.dy - send.center.dy).abs(),
        lessThan(6),
        reason: '${entry.key} 要和发送按钮同一排',
      );
      expect(
        tester.getRect(entry.value.last).height,
        greaterThanOrEqualTo(44),
        reason: '${entry.key} 的命中高度要 ≥44（不然点不准）',
      );
    }
    // 整排贴着输入区右边缘收尾（收在发送按钮上）
    expect(
      send.right,
      closeTo(input.right, 1),
      reason: '这一排的右边界就是发送按钮 / 输入区右边缘',
    );
    // 操作控件**从左侧开始排列**（除发送按钮与 token 用量外均靠左）：
    // 老写法整排 `Wrap(alignment: end)`，附件左边空着一大片，所有控件都被甩到右边。
    // 容差 5px 是 `IconButton` 自己的内缩（48 的命中区里那 40 宽的可见盒居中，左缩 4）。
    expect(
      tester.getRect(find.byTooltip('添加附件 / 插入卡片（@）')).left,
      closeTo(input.left, 5),
      reason: '第一枚操作控件（添加附件）必须贴在输入区左边缘，不能整排靠右',
    );
    expect(find.byTooltip('不支持思考。该模型未声明推理能力，请到「设置 → AI 模型」声明。'), findsWidgets);
    // 窄屏不显示 token 用量
    expect(
      find.textContaining('本对话 ↑'),
      findsNothing,
      reason: '窄屏下 token 汇总让位给操作控件',
    );
  });

  // 「最小窗口宽度」那一档也要能看：320 是产品约定的下限（`windows/runner/win32_window.cpp`
  // 的 `kMinWindowWidthLogical`，即窗口可达到的最小宽度）。该档下底行为**一排**：
  // 四枚紧凑图标 + 发送按钮，全部落在输入区范围内。
  testWidgets('最小宽度 320：四枚图标 + 发送一排放下，不裁不溢出', (tester) async {
    tester.view.physicalSize = const Size(320, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        modelName: 'DeepSeek V4.1 Flash',
        modalities: const ['text', 'image'],
        usage: {'inputTokens': 12167, 'outputTokens': 28},
        permissionMode: AiToolPolicy.modeFull,
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '你好');
    await tester.tap(find.widgetWithText(FilledButton, '发送'));
    await tester.pumpAndSettle();

    // ★ 底行**始终为一排**（特定宽度下控件变为两行会影响可读性）。
    // 以前 560 以下会切到 `ai-composer-two-row` 那个两行布局 —— 现在那个分支已经删了，
    // 判据只剩"窗口宽度"，窄了就压缩文字/退图标，**不折行**。
    expect(
      find.byKey(const Key('ai-composer-two-row')),
      findsNothing,
      reason: '两行排版已删除：窄了该压文字，不该折成两行',
    );
    final input = tester.getRect(find.byType(TextField).first);
    final send = tester.getRect(find.widgetWithText(FilledButton, '发送'));
    final attach = tester.getRect(find.byTooltip('添加附件 / 插入卡片（@）'));
    expect(send.right, closeTo(input.right, 1), reason: '发送按钮贴右（和输入框右边缘对齐）');

    // 附件和发送**同一排**（发送是个固定宽的按钮，不该独占一行），
    // 整排都落在输入区里，没有任何 RenderFlex overflow。
    expect(
      (send.center.dy - attach.center.dy).abs(),
      lessThan(6),
      reason: '发送必须和操作控件同一排',
    );
    expect(attach.left, greaterThanOrEqualTo(input.left - 0.5));
    expect(
      attach.left,
      closeTo(input.left, 5),
      reason: '操作控件从左边起排（用户：除了发送，其他都靠左）',
    );
    expect(tester.takeException(), isNull, reason: '320 宽下不许 overflow');

    // ★ 每个操作控件需**尺寸足够、便于点击**。
    // 44 是 Windows / Material 都认的最小可点尺寸。
    final controls320 = <String, Finder>{
      '添加附件 / 插入卡片（@）': find.byTooltip('添加附件 / 插入卡片（@）'),
      '权限档': find.byTooltip(
        '当前是${AiToolPolicy.modeFullLabel}，AI 调用工具不会等你批准。'
        '可读 / 可写目录范围不变，越界仍会被拒。点一下改回「${AiToolPolicy.modeAskLabel}」。',
      ),
      '思考强度': find.byTooltip('不支持思考。该模型未声明推理能力，请到「设置 → AI 模型」声明。'),
    };
    for (final entry in controls320.entries) {
      expect(entry.value, findsWidgets, reason: '${entry.key} 必须还在底行里');
      final rect = tester.getRect(entry.value.last);
      expect(
        rect.height,
        greaterThanOrEqualTo(44),
        reason: '${entry.key} 的命中高度要 ≥44（不然点不准）',
      );
      expect(
        rect.center.dy,
        closeTo(attach.center.dy, 6),
        reason: '${entry.key} 和附件同一排',
      );
      expect(
        rect.right,
        lessThanOrEqualTo(input.right + 0.5),
        reason: '${entry.key} 不能被从右边裁掉',
      );
    }

    // 操作控件全都排在发送按钮**左边**（发送仍然是最右那一枚）
    final effort = tester.getRect(
      find.byTooltip('不支持思考。该模型未声明推理能力，请到「设置 → AI 模型」声明。'),
    );
    expect(
      effort.right,
      lessThanOrEqualTo(send.left + 0.5),
      reason: '操作控件在左、发送在右（和宽屏的次序一致）',
    );

    // 窄屏不显示 token 用量（它让位给操作控件），但宽屏下必须还在
    expect(find.textContaining('本对话 ↑'), findsNothing);
  });

  // ★ 三档文字密度：判据是**窗口宽度**，不是平台。
  //
  // 约束：
  //   · 控件过小难以点击，且未显示关键信息；特定宽度下变为两行排列影响可读性。
  //   · 判据是窗口宽度，不是平台。
  //
  // 因此这一组在**同一台机器**上只改变窗口宽度，平台不参与判定。
  // ⚠ 每档单独一条用例（**不要在同一条里反复 pumpWidget 换尺寸**）：
  // 换了尺寸之后旧的 element 树还挂着，`find` 会看到两份混在一起的东西。
  testWidgets('底行密度·宽窗口：给全称（"自动允许（无需批准）"）', (tester) async {
    tester.view.physicalSize = const Size(1600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      await buildAiHomePage(permissionMode: AiToolPolicy.modeFull),
    );
    await tester.pumpAndSettle();
    expect(
      find.text(AiToolPolicy.modeFullLabel),
      findsOneWidget,
      reason: '宽窗口给全称',
    );
    expect(find.byKey(const Key('ai-composer-two-row')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('底行密度·中等窗口：给短标签（"自动"），不能退成没有信息的图标', (tester) async {
    tester.view.physicalSize = const Size(960, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      await buildAiHomePage(permissionMode: AiToolPolicy.modeFull),
    );
    await tester.pumpAndSettle();
    expect(
      find.text(AiToolPolicy.modeFullLabel),
      findsNothing,
      reason: '中档不该再铺全称',
    );
    expect(find.text('自动'), findsOneWidget, reason: '中档要给短标签');
    expect(find.byKey(const Key('ai-composer-two-row')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('底行密度·最窄窗口：退成图标，但档位仍可查、命中高度仍 ≥44', (tester) async {
    tester.view.physicalSize = const Size(340, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      await buildAiHomePage(permissionMode: AiToolPolicy.modeFull),
    );
    await tester.pumpAndSettle();
    expect(find.text('自动'), findsNothing, reason: '最窄档退成图标');
    final permTip = find.byTooltip(
      '当前是${AiToolPolicy.modeFullLabel}，AI 调用工具不会等你批准。'
      '可读 / 可写目录范围不变，越界仍会被拒。点一下改回「${AiToolPolicy.modeAskLabel}」。',
    );
    expect(permTip, findsOneWidget, reason: '退成图标后当前档位必须还能查到（tooltip）');
    expect(
      tester.getRect(permTip).height,
      greaterThanOrEqualTo(44),
      reason: '最窄档也要点得准（用户："按钮太小了比较难点"）',
    );
    expect(
      find.byKey(const Key('ai-composer-two-row')),
      findsNothing,
      reason: '最窄档也不许折成两行',
    );
    expect(tester.takeException(), isNull, reason: '340 宽下不许 overflow');
  });

  // 反向保险：窗口宽裕时**不换行**（否则底行会白白多出一行高度）
  testWidgets('宽窗口输入区底行仍然是一行', (tester) async {
    tester.view.physicalSize = const Size(1600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage(modelName: 'GPT-4o'));
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('ai-composer-two-row')), findsNothing);
    // 一行时发送按钮和"添加附件"在同一水平带上（换行后发送会落到操作组下面）
    final attach = tester.getRect(find.byTooltip('添加附件 / 插入卡片（@）'));
    final send = tester.getRect(find.widgetWithText(FilledButton, '发送'));
    expect(
      (send.center.dy - attach.center.dy).abs(),
      lessThan(8),
      reason: '宽窗口下发送按钮仍和操作组同一行',
    );
  });

  testWidgets('冷启动落在新建的聊天记录上，而不是重新打开上次那条', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    expect(store.conversation?.id, 'c1');
    expect(store.messages, isEmpty, reason: '新开一条，不该带出历史消息');
    expect(find.text('AI 工作台'), findsOneWidget, reason: '空态页面 = 新会话');
  });

  testWidgets('输入框草稿按会话保存：切走再切回来文字还在', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    await store.selectModel(store.models.first);
    await tester.pumpAndSettle();

    // 新建一个 store（同一份 SharedPreferences）→ 模拟下次冷启动
    final second = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080', client: _fakeBackend()),
    );
    await second.load();
    expect(second.selectedModel?.id, 'm-text', reason: '上次选的模型要自动选回来');
    expect(second.selectedProvider?.id, 'local-gw');
  });

  testWidgets('模型能力徽标只显示目录声明的能力，未声明的一律标不支持', (tester) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(modalities: ['text', 'image']),
    );
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    // 模拟添加一张图片附件（能力由后端判定）
    store.addAttachment(
      const AiAttachment(
        name: 'poster.png',
        modality: 'image',
        mimeType: 'image/png',
        sizeBytes: 2048,
      ),
    );
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

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    expect(store.selectedModel?.id, 'm-text', reason: '加载后应自动选中目录里的模型');

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 请求消息与助手回复均存在（回复来自 SSE 的 text.delta / message.completed）
    expect(find.text('你好'), findsWidgets);
    expect(find.text('你好，我是假模型'), findsOneWidget);
    expect(store.sending, isFalse);
    expect(store.running, isFalse);
    expect(find.text('发送'), findsOneWidget, reason: '结束后按钮回到「发送」');
  });

  // --- 思考强度（AIH-056） ------------------------------------------------

  testWidgets('模型没声明思考档位时选择器置灰，并说明原因', (tester) async {
    // ⚠ 窗口要够宽（1600）：2026-10-11 起底行控件有**三档文字密度**
    // （全称 / 短标签 / 纯图标，见 `_ComposerLabel`），1200 宽时对话栏只剩 ~635，
    // 走的是**短标签**档（显示"不支持"）。真实桌面窗口（1600）才是全称档。
    tester.view.physicalSize = const Size(1600, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    expect(find.text('不支持思考'), findsOneWidget, reason: '没声明就别给一堆选了会被拒的选项');
    // 没有可选档位时按钮不可点（点到也不会弹菜单）：InkWell.onTap 为 null
    final button = tester.widget<InkWell>(
      find
          .ancestor(of: find.text('不支持思考'), matching: find.byType(InkWell))
          .first,
    );
    expect(button.onTap, isNull, reason: '没档位时不该给出可点的选择器');
  });

  testWidgets('只列出模型声明过的档位，选中的档位随请求发给后端', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    lastRunBody = null;

    await tester.pumpWidget(
      await buildAiHomePage(
        reasoning: true,
        thinkingFormat: 'deepseek',
        thinkingEfforts: {'off': 'none', 'low': 'low', 'high': 'high'},
      ),
    );
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    // 目录声明了三档思考 + 一个关闭取值，所以界面上是「默认 → 关闭 → 低 → 高」
    // （medium / max 不出现；「关闭」2026-11-09 起才独立成档）
    expect(store.selectedModel!.selectableEfforts, [
      AiReasoningEffort.off,
      AiReasoningEffort.disabled,
      AiReasoningEffort.low,
      AiReasoningEffort.high,
    ]);
    expect(
      find.text('默认'),
      findsOneWidget,
      reason: '缺省档位叫「默认」（2026-09-20 由「关闭」改名）',
    );

    await tester.tap(find.byType(AppMenuButton<AiReasoningEffort>));
    await tester.pumpAndSettle();
    expect(find.text('低'), findsWidgets);
    expect(
      find.text('关闭'),
      findsOneWidget,
      reason: '目录声明了关闭取值 → 这一档必须能选（用户 2026-11-09 报的 bug）',
    );
    expect(find.text('中'), findsNothing, reason: '未声明的档位不能出现');

    await tester.tap(find.text('高').last);
    await tester.pumpAndSettle();
    expect(store.reasoningEffort, AiReasoningEffort.high);

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(lastRunBody?['reasoningEffort'], 'high', reason: '选了的档位必须真的带上');
  });

  testWidgets('选「默认」时不把思考参数发给后端', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    lastRunBody = null;

    await tester.pumpWidget(
      await buildAiHomePage(
        reasoning: true,
        thinkingEfforts: {'low': 'low', 'high': 'high'},
      ),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(lastRunBody?.containsKey('reasoningEffort'), isFalse);
  });

  // --- token 统计（AIH-057） ---------------------------------------------

  // token 用量数值过大时切换 K/M/B 单位。
  // 老实现只有 `k` 一档 → 150 万会写成 `1500k`（四位数还带单位，比原来还长）。
  test('token 用量按数量级切 K / M / B 单位', () {
    expect(AiTokenUsage.compact(0), '0');
    expect(AiTokenUsage.compact(999), '999');
    expect(AiTokenUsage.compact(1000), '1K', reason: '正好整数不留 .0');
    expect(AiTokenUsage.compact(1200), '1.2K');
    expect(AiTokenUsage.compact(12167), '12K', reason: '换算后整数位到两位就不留小数');
    expect(AiTokenUsage.compact(999499), '999K', reason: '不到 M 就不上台阶');
    expect(AiTokenUsage.compact(1000000), '1M');
    expect(AiTokenUsage.compact(1500000), '1.5M', reason: '1500k 就是这条要修掉的样子');
    expect(AiTokenUsage.compact(25000000), '25M');
    expect(AiTokenUsage.compact(1000000000), '1B');
    expect(AiTokenUsage.compact(1234567890), '1.2B');
  });

  testWidgets('助手消息显示归一化后的 token 用量，输入区显示本对话汇总', (tester) async {
    tester.view.physicalSize = const Size(1200, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        usage: {'inputTokens': 1200, 'outputTokens': 300, 'cachedTokens': 400},
      ),
    );
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    expect(store.usageSummary.isEmpty, isTrue, reason: '还没对话时不该显示统计');

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 单条消息：↑1.2K ↓300
    expect(find.text('↑1.2K ↓300'), findsOneWidget);
    // 本对话汇总（输入区）
    expect(find.textContaining('本对话 ↑1.2K ↓300'), findsOneWidget);
    expect(store.usageSummary.totalTokens, 1500);
    expect(store.usageSummary.cachedTokens, 400);
    expect(store.usageSummary.requests, 1);
  });

  testWidgets('输入区底行：token 汇总不被挤没，发送按钮永远贴右（用户 bug）', (tester) async {
    // 1440 宽：中栏 = 1440 - 240(会话列表) - 260(右栏) - 2 = 938，
    // 模型名长 + 能力徽标多的时候底行会被挤紧，正好复现"汇总被省略 / 按钮不贴右"。
    tester.view.physicalSize = const Size(1440, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        modelName: 'DeepSeek V4.1 Flash',
        modalities: const ['text', 'image', 'video', 'audio'],
        usage: {'inputTokens': 6400, 'outputTokens': 1500},
      ),
    );
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
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
    expect(
      send.right,
      closeTo(input.right, 1),
      reason: '发送按钮必须贴右（不能被顶出去、也不该缩在中间）',
    );
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

    await tester.pumpWidget(
      await buildAiHomePage(
        modelName: 'GPT-4o',
        usage: {'inputTokens': 1200, 'outputTokens': 300},
      ),
    );
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    final send = tester.getRect(
      find.ancestor(of: find.text('发送'), matching: find.byType(FilledButton)),
    );
    final input = tester.getRect(find.byType(TextField));
    expect(send.right, closeTo(input.right, 1), reason: '宽裕时按钮右边也不该留一截空白');
    expect(find.textContaining('本对话 ↑1.2K ↓300'), findsOneWidget);
  });

  // --- 重试（AIH-024） ---------------------------------------------------

  testWidgets('失败的回复给出「重试」，重试会新开 Run 并带上 retryOfRunId', (tester) async {
    tester.view.physicalSize = const Size(1200, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage(failFirstRun: true));
    await tester.pumpAndSettle();

    final store = tester
        .element(find.byType(AiHomePage))
        .read<AiWorkspaceStore>();
    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    // 第一次失败：错误可见 + 有重试入口
    expect(find.textContaining('上游限流'), findsOneWidget);
    expect(find.text('重试'), findsOneWidget);
    expect(lastRunBodies.length, 1);
    expect(
      lastRunBodies.first.containsKey('retryOfRunId'),
      isFalse,
      reason: '首次请求不该带重试标记',
    );

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

    await tester.pumpWidget(await buildAiHomePage(usage: const {}));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '你好');
    await tester.tap(find.text('发送'));
    await tester.pumpAndSettle();

    expect(find.textContaining('↑'), findsNothing, reason: '没有 usage 就不该编一个出来');
    expect(find.text('你好，我是假模型'), findsOneWidget);
  });

  testWidgets('@ 卡片：角色卡可以选多张（多角色同框），画风卡仍只有一张', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    // 入口在底行最左的 "+" 菜单里（4 枚控件 + 发送已经占满 320~440 的宽度）
    Future<void> openCardPicker() async {
      await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('角色卡 / 画风卡…'));
      await tester.pumpAndSettle();
    }

    await openCardPicker();
    expect(find.text('凛'), findsOneWidget);
    await tester.tap(find.text('凛'));
    await tester.pumpAndSettle();
    expect(find.textContaining('角色卡 · 凛'), findsOneWidget);

    // 选择一张后收起；选择第二张需重新打开。
    // （面板常驻会把输入区顶出屏幕，尤其是键盘弹起时。）
    expect(find.text('诗音'), findsNothing, reason: '选完第一张面板就该收起');
    await openCardPicker();
    await tester.tap(find.text('诗音'));
    await tester.pumpAndSettle();
    expect(find.textContaining('角色卡 · 凛'), findsOneWidget);
    expect(find.textContaining('角色卡 · 诗音'), findsOneWidget);

    await openCardPicker();
    await tester.tap(find.text('画风卡'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('物理感动画'));
    await tester.pumpAndSettle();
    expect(find.textContaining('画风卡 · 物理感动画'), findsOneWidget);

    // 面板已收起（选择后即收），写入需求内容
    await tester.enterText(find.byType(TextField).first, '保留构图');
    await tester.tap(find.widgetWithText(FilledButton, '发送'));
    await tester.pumpAndSettle();

    final sent = lastRunBody!['text'] as String;
    expect(sent, startsWith('[STYLE SPEC]\n物理感动画\n'));
    expect(sent, contains('[CHARACTER CARD]\n凛\n'));
    expect(sent, contains('[CHARACTER CARD]\n诗音\n'));
    // 负面提示词也要投给模型：以前只发 name + 正文，卡上的负面从没到过模型。
    expect(sent, contains('[STYLE NEGATIVE]\nsoft shading, blurry'));
    // 空负面不写空段（两张角色卡都没有负面）。
    expect(sent, isNot(contains('[CHARACTER NEGATIVE]')));
    // 角色卡按选中顺序排在画风卡之后：凛 → 诗音
    expect(
      sent.indexOf('[CHARACTER CARD]\n凛\n'),
      lessThan(sent.indexOf('[CHARACTER CARD]\n诗音\n')),
    );
    expect(sent, endsWith('保留构图'));
    // 发出去之后：输入区的"待发送卡片"清空（下一轮不会重复带上一轮的卡），
    // 但**这条消息自己**要留下快照小卡片（点开能看到当时那一版）。
    expect(
      find.byWidgetPredicate(
        (w) =>
            w is InputChip && w.key.toString().contains('character-card-chip-'),
      ),
      findsNothing,
    );
    expect(
      find.byWidgetPredicate(
        (w) => w is InputChip && w.key.toString().contains('style-card-chip-'),
      ),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('mention-character_card-凛')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('mention-character_card-诗音')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('mention-style_card-物理感动画')),
      findsOneWidget,
    );
    // 发送内容同时包含"界面显示的正文"与卡片快照（模型侧仍为原卡块）
    expect(lastRunBody!['displayText'], '保留构图');
    final mentions = lastRunBody!['mentions'] as List;
    expect(mentions.first['kind'], 'style_card');
    expect(
      mentions.first['modelText'],
      contains('[STYLE NEGATIVE]\nsoft shading, blurry'),
      reason: 'mention 的 modelText 是模型真正拿到的那段，负面必须在里面',
    );
    expect(
      (mentions[1]['snapshot'] as Map)['prompt'],
      contains('Rin, black hair'),
    );

    // 点开小卡片：弹窗显示的是**发送时**的卡正文
    await tester.tap(find.byKey(const ValueKey('mention-character_card-凛')));
    await tester.pumpAndSettle();
    expect(find.text('角色卡 · 凛'), findsWidgets);
    expect(find.textContaining('Rin, black hair'), findsOneWidget);
  });

  testWidgets('@ 面板每次打开都重拉卡片库（改过的卡不会拿旧正文）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final styles = <Map<String, dynamic>>[
      {
        'id': 7,
        'name': '旧画风',
        'prompt': 'Old body.',
        'negative': 'old negative',
        'examples': <Object>[],
      },
    ];
    await tester.pumpWidget(await buildAiHomePage(styleCards: styles));
    await tester.pumpAndSettle();

    Future<void> openPicker() async {
      await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('角色卡 / 画风卡…'));
      await tester.pumpAndSettle();
    }

    await openPicker();
    await tester.tap(find.text('画风卡'));
    await tester.pumpAndSettle();
    expect(find.text('旧画风'), findsOneWidget);
    await tester.tap(find.text('旧画风'));
    await tester.pumpAndSettle();
    expect(find.textContaining('画风卡 · 旧画风'), findsOneWidget);

    // 模拟"在预设库 / AI 那边把卡改了"：同一 id 换一份正文与名字。
    styles
      ..clear()
      ..add({
        'id': 7,
        'name': '新画风',
        'prompt': 'New body.',
        'examples': <Object>[],
      });

    await openPicker();
    await tester.tap(find.text('画风卡'));
    await tester.pumpAndSettle();
    expect(find.text('新画风'), findsOneWidget, reason: '重新打开要拿到新正文');
    expect(find.text('旧画风'), findsNothing);
    // 已经选中的那张也要换成新对象：否则下一次发送仍用旧正文。
    expect(find.textContaining('画风卡 · 新画风'), findsOneWidget);
  });

  testWidgets('@ 面板搜索按空格分词（"波普 平涂" 要能查到）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      await buildAiHomePage(
        styleCards: [
          {
            'id': 8,
            'name': '波普赛璐璐硬边平涂',
            'prompt': 'Hard-edged poster colour.',
            'examples': <Object>[],
          },
          {
            'id': 9,
            'name': '柔和水彩',
            'prompt': 'Soft watercolour wash.',
            'examples': <Object>[],
          },
        ],
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('角色卡 / 画风卡…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('画风卡'));
    await tester.pumpAndSettle();

    final search = find.byWidgetPredicate(
      (w) => w is TextField && w.decoration?.hintText == '搜卡片名 / 正文',
    );
    expect(search, findsOneWidget);

    // 旧实现拿整串 "波普 平涂" 去 contains，永远查不到。
    await tester.enterText(search, '波普 平涂');
    await tester.pumpAndSettle();
    expect(find.text('波普赛璐璐硬边平涂'), findsOneWidget);
    expect(find.text('柔和水彩'), findsNothing);

    await tester.enterText(search, '波普 不存在');
    await tester.pumpAndSettle();
    expect(find.text('没有匹配的卡片'), findsOneWidget);
  });

  testWidgets('@ 卡片：再点一次已选中的角色卡 = 取消（多选是切换，不是只能加）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('角色卡 / 画风卡…'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('凛'));
    await tester.pumpAndSettle();
    // 选择后收起：选择第二张需重新打开
    await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('角色卡 / 画风卡…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('诗音'));
    await tester.pumpAndSettle();
    expect(find.textContaining('角色卡 · 凛'), findsOneWidget);
    expect(find.textContaining('角色卡 · 诗音'), findsOneWidget);

    // 再选一次诗音 = 取消这一张
    await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('角色卡 / 画风卡…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('诗音'));
    await tester.pumpAndSettle();
    expect(find.textContaining('角色卡 · 诗音'), findsNothing);
    expect(find.textContaining('角色卡 · 凛'), findsOneWidget);
  });

  // 手机上卡片选择器改为**底部弹层**：独立路由、独立高度与滚动，
  // 不与输入区争用高度。
  testWidgets('手机：卡片选择器是底部弹层，选完自动关闭', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      tester.view.physicalSize = const Size(720, 1600);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(await buildAiHomePage());
      await tester.pumpAndSettle();

      await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('角色卡 / 画风卡…'));
      await tester.pumpAndSettle();

      // 弹层里能看到卡片（说明不是被挤成一条的输入区内嵌面板）
      expect(find.text('凛'), findsOneWidget);
      await tester.tap(find.text('凛'));
      await tester.pumpAndSettle();

      // 选完弹层自动关：列表里那张卡不在了，chip 留了下来
      expect(find.text('凛'), findsNothing, reason: '弹层选完就该关掉');
      expect(find.textContaining('角色卡 · 凛'), findsOneWidget);
      expect(tester.takeException(), isNull);
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  // 多行文本输入不能溢出：输入框行数按
  // "输入区可用高度 - 卡片 - 底行"计算；键盘高度（viewInsets）变化时同样成立。
  testWidgets('手机 + 键盘 + 卡片 + 长文本：输入区不溢出', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      tester.view.physicalSize = const Size(360, 780);
      tester.view.devicePixelRatio = 1.0;
      tester.view.viewInsets = const FakeViewPadding(bottom: 300);
      addTearDown(tester.view.reset);

      await tester.pumpWidget(await buildAiHomePage());
      await tester.pumpAndSettle();

      // 先选一张卡（走底部弹层）
      await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('角色卡 / 画风卡…'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('凛'));
      await tester.pumpAndSettle();
      expect(find.textContaining('角色卡 · 凛'), findsOneWidget);

      // 再塞一大段文本：超过行数上限时在输入框内部滚，不许把页面顶出去
      await tester.enterText(
        find.byType(TextField).first,
        'Please keep this prompt very long to test wrapping in the composer. ' *
            6,
      );
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: '多行输入不许 overflow');
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  // 手机端 "输入 @ → 取消 → 焦点回到输入框后再次弹出，需取消两次才能输入"。
  // 取消时必须移除行末的 `@`，且同一段文本不重复弹出。
  testWidgets('手机：取消卡片弹层 → @ 被摘掉且不会重复弹', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      tester.view.physicalSize = const Size(360, 780);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(await buildAiHomePage());
      await tester.pumpAndSettle();

      final field = find.byType(TextField).first;
      await tester.enterText(field, '@');
      await tester.pumpAndSettle();
      expect(find.text('凛'), findsOneWidget, reason: '弹层出现');

      // 取消（弹层右上角的 X）
      await tester.tap(find.byTooltip('收起卡片面板'));
      await tester.pumpAndSettle();
      expect(find.text('凛'), findsNothing, reason: '取消后弹层关闭');
      expect(
        tester.widget<TextField>(field).controller!.text,
        '',
        reason: '行末那个 @ 要摘掉，不然接着打字会变成 "@正文"',
      );

      // 焦点回到输入框：同一段文本不许再弹一次
      await tester.tap(field);
      await tester.pumpAndSettle();
      expect(find.text('凛'), findsNothing, reason: '焦点回来不许重复弹层');

      await tester.enterText(field, 'hello');
      await tester.pumpAndSettle();
      expect(tester.widget<TextField>(field).controller!.text, 'hello');
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });

  // 点击角色卡 / 画风卡时，弹窗显示详细内容（只读）。
  testWidgets('已选卡片：点 chip 打开只读详情', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await buildAiHomePage());
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('添加附件 / 插入卡片（@）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('角色卡 / 画风卡…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('凛'));
    await tester.pumpAndSettle();

    final chip = find.byWidgetPredicate(
      (w) =>
          w is InputChip &&
          w.label is Text &&
          (w.label as Text).data == '角色卡 · 凛',
    );
    expect(chip, findsOneWidget);
    await tester.tap(chip);
    await tester.pumpAndSettle();

    expect(find.text('正面提示词'), findsOneWidget);
    expect(find.textContaining('Rin, black hair'), findsWidgets);
    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
    expect(find.text('正面提示词'), findsNothing);
  });

  // 回车 = 发送仅适用于电脑端。
  testWidgets('手机：回车是换行，不是发送', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      tester.view.physicalSize = const Size(360, 780);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(await buildAiHomePage());
      await tester.pumpAndSettle();

      final field = find.byType(TextField).first;
      await tester.enterText(field, '第一行');
      await tester.pump();
      // 个别输入法把软键盘回车当原始 Enter 键事件发上来 —— 手机上不许因此发送。
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();

      expect(lastRunBody, isNull, reason: '手机回车不许发送');
      expect(
        tester.widget<TextField>(field).controller!.text,
        '第一行',
        reason: '没发送就不该清空输入框',
      );
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
