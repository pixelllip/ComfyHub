// 跨端待审批（M5）回归：**手机排上来的提交在桌面上看得见、点得动**。
//
// # 背景（既定规则 + 契约 §4.1）
//
// 「有关工作流的交给桌面审批处理」、「手机点过提交**不算**批准」—— 手机上那一下只是
// 把活排进队列（`submit_job`，`refId` = `queueId`），真正提交要由桌面过同一道审批闸门。
// Rust 侧（`server-rs/src/submit_queue.rs` + `pending.rs`）已经把手机排的队并进
// `GET /api/ai/pending` 的 `approvals[]`，`callId` 形如 `submit:<queueId>`，
// 批准 / 拒绝走的还是 `POST /api/ai/tool-calls/{callId}/approve|deny`。
//
// # 这张卡在防什么
//
// 桌面上那张"等你批准"的卡平时是跟着 **Run 的 SSE** 长的，而手机排上来的那条
// **不在任何本机 Run 的事件流中** —— 缺少该卡片时，桌面上不可见、无法操作，
// 只能等它超时（表现就是"手机上提交了，桌面什么动静都没有"）。
//
// 这里钉住五件事：
//   (a) 有 `submit:<queueId>` 时卡看得见，点「批准」POST 的就是那个 callId，成功后那行消失；
//   (b) `accepted:false` → 如实说"桌面没接受"，**不许把那行去掉假装成功**；
//   (c) 空列表 → 整块不占位（右侧栏本来就窄）；
//   (d) 拉取失败 → 一行小字如实说明，页面别的地方照旧；
//   (e) 手机上（`forceLocal` 那条路）**不出现**这张卡，也不白轮询；
//   (f) 5 秒一轮，页面销毁（dispose）后停。
//
// 假后端沿用 test/ai_tools_ui_test.dart 的写法（MockClient，不出网）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/core/run_event_source.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

/// 这次用例里后端收到了什么、以及后端那边"待批列表现在是什么"。
class _Rec {
  /// 记的是**解码后**的路径。
  ///
  /// ⚠ 必须解码：客户端发 callId 时走 `Uri.encodeComponent`，`submit:q1` 会变成
  /// `submit%3Aq1`，而 `Uri.parse(...).path` **不会**把 %3A 还原回来 ——
  /// 断言原始 path 就永远匹配不上（真后端那边由 axum 的 Path 提取器解码，所以线上是对的）。
  final List<String> calls = [];

  /// `GET /api/ai/pending` 现在的 `approvals[]`（用例自己摆）。
  List<Map<String, dynamic>> approvals = [];

  /// 后端要不要"收下"这次批准 / 拒绝（对应响应里的 `accepted`）。
  bool acceptanceOk = true;

  /// `GET /api/ai/pending` 直接失败（模拟后端不可达 / 接口挂了）。
  bool pendingFails = false;

  int get polls => calls.where((c) => c == 'GET /api/ai/pending').length;
}

/// 一条**手机排队的提交**在 `/api/ai/pending` 里的样子。
///
/// 键名逐字对齐 `submit_queue.rs::pending_approvals`（也是工具审批那 6 个键：
/// `callId` / `runId` / `conversationId` / `name` / `argumentsJson` / `askedAt`）。
Map<String, dynamic> _queuedSubmit(String queueId, {int promptId = 310}) => {
  'callId': 'submit:$queueId',
  // 桌面 Run 才有 runId；排队的提交是"还没轮到跑"的命令，后端给空串
  'runId': '',
  'conversationId': 'c1',
  // 名字就是它真正要干的事（批准 = 让桌面真的提交）
  'name': 'comfy_submit',
  'argumentsJson': jsonEncode({'promptId': promptId, 'nsfw': false}),
  // ⚠ 审批这一份是 **ISO 文本**（提问那份才是毫秒数）
  'askedAt': '2026-09-27T05:30:00Z',
};

MockClient _backend(_Rec rec) => MockClient((request) async {
  final path = Uri.decodeComponent(request.url.path);
  rec.calls.add('${request.method} $path');
  Object body;

  if (path == '/api/ai/pending') {
    if (rec.pendingFails) {
      return http.Response(
        jsonEncode({'error': '待审批接口挂了'}),
        503,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    }
    body = {'asks': <Object>[], 'approvals': rec.approvals};
  } else if (path.startsWith('/api/ai/tool-calls/')) {
    final segments = path.split('/');
    final callId = segments[4];
    // 真后端接受之后这一条**当场离开待办**（`submit_jobs.approval` 置位），
    // 所以假后端也要跟着改 —— 否则刷新一拉，刚批过的那行又回来了。
    if (rec.acceptanceOk) {
      rec.approvals = rec.approvals
          .where((a) => a['callId'] != callId)
          .toList();
    }
    body = {
      'callId': callId,
      'approved': segments.last == 'approve',
      'accepted': rec.acceptanceOk,
    };
  } else if (path == '/api/capture/jobs') {
    body = {
      'queueRunning': 0,
      'queuePending': 0,
      'comfyReachable': true,
      'submissions': <Object>[],
    };
  } else if (path == '/api/ai/providers') {
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
        'id': 'model-00',
        'displayName': '模型 00',
        'inputModalities': ['text'],
        'tools': true,
        'capabilitySource': 'manual',
        'enabled': true,
      },
    ];
  } else if (path == '/api/ai/conversations' && request.method == 'POST') {
    body = {
      'id': 'c1',
      'title': '新对话',
      'providerId': 'local-gw',
      'modelId': 'model-00',
      'messageCount': 0,
    };
  } else if (path == '/api/ai/conversations') {
    body = <Object>[];
  } else if (path == '/api/ai/conversations/c1/messages') {
    body = <Object>[];
  } else if (path == '/api/ai/skills') {
    body = <Object>[];
  } else if (path == '/api/ai/skills/roots') {
    body = {
      'userRoot': r'D:\ComfyHub\storage\ai\skills',
      'builtinRoot': r'D:\ComfyHub\skills\builtin',
      'userRootExists': true,
    };
  } else if (path == '/api/ai/memory') {
    body = {
      'content': '',
      'path': r'D:\ComfyHub\storage\ai\memory.md',
      'entryCount': 0,
      'maxChars': 8000,
      'maxEntries': 100,
    };
  } else if (path == '/api/ai/tools/policy') {
    body = {
      'writeRoots': [r'D:\ComfyHub\comfyui'],
      'readRoots': [r'D:\ComfyHub\comfyui'],
      'defaultWriteRoot': r'D:\ComfyHub\comfyui',
      'overrides': <String, String>{},
    };
  } else if (path == '/api/ai/tools') {
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

/// 宽屏挂上 AI 工作台：只有在 `width >= 1200` 时右侧栏（`_ContextPanel`）才会铺开，
/// 所以这些用例都按桌面尺寸挂 —— 否则"卡没出现"只是因为右侧栏压根没渲染（假阴性）。
void _desktop(WidgetTester tester) {
  tester.view.physicalSize = const Size(1600, 1200);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

Future<Widget> _homePage(_Rec rec, {bool local = false}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = AiApiClient(settings.baseUrl, client: _backend(rec));
  final store = AiWorkspaceStore(
    api: api,
    // 手机那一端 = 本地 harness（`openRunEventSource` 的 `forceLocal` 那条路）。
    // 这里只关心"界面画不画那张卡、轮不轮询"，所以不需要一个完整的假 harness：
    // 平台通道在用例里必然失败，Store 自己会把它记成"连接后端失败"，与本用例无关。
    source: local ? openRunEventSource(api: api, forceLocal: true) : null,
    credentials: local ? openCredentialStore(api: api, forceLocal: true) : null,
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
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('(a) 桌面：手机排队的提交看得见，点「批准」POST 那个 callId，成功后那行消失', (tester) async {
    _desktop(tester);
    final rec = _Rec()..approvals = [_queuedSubmit('q1')];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 手机排的那一条真的在桌面上露面了（这是本批补的那张卡的全部意义）
    expect(find.text('待审批'), findsOneWidget);
    expect(find.textContaining('手机排队的提交 ·'), findsOneWidget);
    // 参数按后端给的原文显示（不解析、不猜）
    expect(find.textContaining('promptId'), findsOneWidget);
    // 卡片上写明既定规则
    expect(find.textContaining('手机上点「提交」只是排队'), findsOneWidget);
    expect(rec.calls, contains('GET /api/ai/pending'));

    await tester.tap(find.byKey(const Key('ai-pending-approve-submit:q1')));
    await tester.pumpAndSettle();

    // 真的打到了那条 callId 的审批通道（那条通道同时也是工具审批走的那一条）
    expect(rec.calls, contains('POST /api/ai/tool-calls/submit:q1/approve'));
    // 后端接受了 → 这一条离开待办；没有别的待办 → 整块跟着消失
    expect(find.textContaining('手机排队的提交 ·'), findsNothing);
    expect(find.text('待审批'), findsNothing);
  });

  testWidgets('(b) accepted:false → 如实说"桌面没接受"，那行不许消失（不许假装成功）', (tester) async {
    _desktop(tester);
    final rec = _Rec()
      ..approvals = [_queuedSubmit('q1')]
      ..acceptanceOk = false;

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const Key('ai-pending-approve-submit:q1')));
    await tester.pumpAndSettle();

    expect(rec.calls, contains('POST /api/ai/tool-calls/submit:q1/approve'));
    expect(find.textContaining('桌面没有接受这一条'), findsOneWidget);
    // 关键：行还在（点晚了 / 早被人点过，都不该让这张待办凭空消失）
    expect(find.textContaining('手机排队的提交 ·'), findsOneWidget);
    expect(find.text('待审批'), findsOneWidget);
  });

  testWidgets('(c) 没有待办 → 整块不占位（右侧栏本来就窄）', (tester) async {
    _desktop(tester);
    final rec = _Rec();

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    expect(find.text('待审批'), findsNothing);
    expect(find.textContaining('待审批列表取不到'), findsNothing);
    // 问过了才叫"没有待办"（不是"没去问"）
    expect(rec.calls, contains('GET /api/ai/pending'));
  });

  testWidgets('(d) 拉取失败 → 一行小字如实说明，页面别的地方照旧', (tester) async {
    _desktop(tester);
    final rec = _Rec()..pendingFails = true;

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    expect(find.textContaining('待审批列表取不到'), findsOneWidget);
    expect(find.textContaining('待审批接口挂了'), findsOneWidget);
    // 不把整页搞得像坏了：右侧栏别的块照旧
    expect(find.text('模型能力'), findsOneWidget);
    expect(find.text('长期记忆'), findsOneWidget);
  });

  testWidgets('(e) 手机上（本地 harness）不出现这张卡，也不白轮询', (tester) async {
    _desktop(tester);
    final rec = _Rec()..approvals = [_queuedSubmit('q1')];

    await tester.pumpWidget(await _homePage(rec, local: true));
    await tester.pumpAndSettle();

    // 右侧栏确实挂着（宽屏）—— 不是"面板没渲染"造成的假阴性
    expect(find.text('模型能力'), findsOneWidget);
    // 审批归桌面：手机上不给一个点了没用的按钮
    expect(find.text('待审批'), findsNothing);
    expect(find.textContaining('手机排队的提交 ·'), findsNothing);
    // 一条请求都不许发（手机往后端问待批清单纯属浪费）
    expect(rec.calls.any((c) => c.contains('/api/ai/pending')), isFalse);
  });

  testWidgets('(f) 5 秒一轮；页面销毁（dispose）之后停', (tester) async {
    _desktop(tester);
    final rec = _Rec()..approvals = [_queuedSubmit('q1')];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();
    expect(rec.polls, 1, reason: '右侧栏一显示就先拉一次');

    await tester.pump(const Duration(seconds: 5));
    await tester.pump();
    expect(rec.polls, 2, reason: '5 秒一轮');

    // 页面销毁（切到别的功能页 / 关窗口）：定时器必须跟着停，否则它会一直问后端
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    final settled = rec.polls;
    await tester.pump(const Duration(seconds: 12));
    await tester.pump();
    expect(rec.polls, settled, reason: 'dispose 之后一轮都不许再发');
  });

  testWidgets('(g) 排队多了也只画 5 条 + "还有 N 条"（右侧栏窄，不能撑爆）', (tester) async {
    _desktop(tester);
    final rec = _Rec()
      ..approvals = [for (var i = 1; i <= 6; i++) _queuedSubmit('q$i')];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    expect(find.text('批准'), findsNWidgets(5), reason: '一屏最多 5 条');
    expect(
      find.textContaining('还有 1 条'),
      findsOneWidget,
      reason: '剩下的要如实说还有几条',
    );
    // 第 6 条折在"还有 N 条"里，不画出来
    expect(find.byKey(const Key('ai-pending-approve-submit:q6')), findsNothing);
    expect(find.text('6 项待批'), findsOneWidget);
  });

  testWidgets('(h) 窄屏（手机尺寸）从标题栏打开状态面板：那张卡照样画得下、不溢出', (tester) async {
    // 真机那一组尺寸（1080x2400 物理 / 2.8125 = 逻辑 384x853）。
    // 溢出会抛异常 → 用例自己会红（见 test/phone_layout_overflow_test.dart 的说明）。
    tester.view.physicalSize = const Size(1080, 2400);
    tester.view.devicePixelRatio = 2.8125;
    addTearDown(tester.view.reset);

    final rec = _Rec()..approvals = [_queuedSubmit('q1')];

    await tester.pumpWidget(await _homePage(rec));
    await tester.pumpAndSettle();

    // 窄屏没有第三栏，状态（含这张卡）从标题栏的「更多」菜单进
    await tester.tap(find.byTooltip('更多操作'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上下文与状态'));
    await tester.pumpAndSettle();

    expect(find.text('待审批'), findsOneWidget);
    expect(find.textContaining('手机排队的提交 ·'), findsOneWidget);
    expect(
      find.byKey(const Key('ai-pending-approve-submit:q1')),
      findsOneWidget,
    );
  });
}
