// 回归（`ask_follow_up`，2026-11-13 的「非阻塞变种」）：
//
//   ① 问题抛在**回复末尾**，不接管底部输入框（`pendingQuestion` 为空）；
//   ② 一张卡**分页**（第 N/M 题），每题可以不选，最后一页是「提交」；
//   ③ 点提交：逐题记答案，再把选择拼成**一条用户消息**发出去（开下一轮）——
//      没选的题写「没有选择」，模型附的 `submitText` 跟在最后。

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

const _callId = 'call_follow_1';
const _q1 = 'q-follow-1';
const _q2 = 'q-follow-2';

String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

final List<Map<String, dynamic>> answered = [];
final List<String> skipped = [];

/// 这两题（问题 / 选项 / 分组）是流里与历史 `parts` 共用的同一份事实。
List<Map<String, dynamic>> _askedAll() => [
  {
    'questionId': _q1,
    'runId': 'r1',
    'conversationId': 'c1',
    'question': '跑哪一份？',
    'options': const ['应用内提交的那份', 'ComfyUI 里跑的那份'],
    'header': '工作流',
    'allowFreeText': true,
    'answered': false,
    'askedAt': 1,
    'index': 0,
    'total': 2,
    'groupId': 'g-follow',
  },
  {
    'questionId': _q2,
    'runId': 'r1',
    'conversationId': 'c1',
    'question': '要几张？',
    'options': const ['1 张', '4 张'],
    'header': '数量',
    'allowFreeText': true,
    'answered': false,
    'askedAt': 1,
    'index': 1,
    'total': 2,
    'groupId': 'g-follow',
  },
];

/// 第一条助手消息定稿后的 parts（正文 + 调用 + 结果）。
///
/// 流里的 `message.completed` 与假后端 `/messages` 必须给同一份 ——
/// 否则一次「与服务端对齐」就会把提问卡（只在 `parts` 里）擦掉。
List<Map<String, dynamic>> _a1Parts() {
  final all = _askedAll();
  return [
    {'type': 'text', 'text': '看两套都合适，你挑一份。'},
    {
      'type': 'tool_call',
      'toolCallId': _callId,
      'jsonPayload': {
        'name': 'ask_follow_up',
        'arguments': '{"questions":[...]}',
      },
    },
    {
      'type': 'tool_result',
      'toolCallId': _callId,
      'text': '问题已经放到回复末尾',
      'jsonPayload': {
        'name': 'ask_follow_up',
        'ok': true,
        'submitText': '请按这个继续',
        'askedAll': all,
        'asked': all.first,
      },
    },
  ];
}

/// 一次结尾提问的完整事件流（与真后端同形）：
/// `tool.requested → tool.started → user.question ×2 → tool.completed → message.completed → run.completed`。
String _stream({bool secondRun = false}) {
  final all = _askedAll();
  if (secondRun) {
    // 提交之后开启的新一轮：只要一个正常收尾，用例不关心它写了什么。
    return _sse(0, 'run.started', '{"runId":"r2"}') +
        _sse(1, 'message.started', '{"messageId":"a2"}') +
        _sse(
          2,
          'message.completed',
          jsonEncode({'messageId': 'a2', 'text': '好的。', 'steps': 1}),
        ) +
        _sse(3, 'run.completed', '{"runId":"r2"}');
  }
  return _sse(0, 'run.started', '{"runId":"r1"}') +
      _sse(1, 'message.started', '{"messageId":"a1"}') +
      _sse(
        2,
        'tool.requested',
        jsonEncode({
          'runId': 'r1',
          'callId': _callId,
          'name': 'ask_follow_up',
          'arguments': '{"questions":[...]}',
          'approval': 'not_required',
        }),
      ) +
      _sse(
        3,
        'tool.started',
        jsonEncode({'callId': _callId, 'name': 'ask_follow_up'}),
      ) +
      _sse(
        4,
        'user.question',
        jsonEncode({'runId': 'r1', 'question': all[0]}),
      ) +
      _sse(
        5,
        'user.question',
        jsonEncode({'runId': 'r1', 'question': all[1]}),
      ) +
      _sse(
        6,
        'tool.completed',
        jsonEncode({
          'callId': _callId,
          'name': 'ask_follow_up',
          'elapsedMs': 3,
          'result': {
            'pending': true,
            'endsTurn': true,
            'submitText': '请按这个继续',
            'askedAll': all,
            'asked': all[0],
          },
        }),
      ) +
      _sse(
        7,
        'message.completed',
        jsonEncode({
          'messageId': 'a1',
          'text': '看两套都合适，你挑一份。',
          'steps': 1,
          'parts': _a1Parts(),
        }),
      ) +
      _sse(8, 'run.completed', '{"runId":"r1"}');
}

/// 假后端（只实现这条用例用到的几路）。
class _Fake {
  /// 第二轮（用户提交后）真的发出的那条消息正文。
  String? sentText;

  /// 库里的消息（`/messages` 原样返回）。
  ///
  /// a1 的 `parts` 必须和流里的 `message.completed` 是同一份 —— 对齐时
  /// 提问卡只活在 parts 里，给一份缺了调用块的 parts 会把它擦掉。
  final List<Map<String, dynamic>> messages = [
    {
      'id': 'u1',
      'conversationId': 'c1',
      'seq': 1,
      'role': 'user',
      'status': 'complete',
      'text': '用我最近的工作流画一张',
      'parts': [
        {'type': 'text', 'text': '用我最近的工作流画一张'},
      ],
    },
    {
      'id': 'a1',
      'conversationId': 'c1',
      'seq': 2,
      'role': 'assistant',
      'status': 'complete',
      'text': '看两套都合适，你挑一份。',
      'parts': _a1Parts(),
    },
  ];

  MockClient client() => MockClient((request) async {
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
          'displayName': '纯文本模型',
          'inputModalities': ['text'],
          'tools': true,
          'reasoning': false,
          'thinkingEfforts': <String, String>{},
          'capabilitySource': 'manual',
          'enabled': true,
        },
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
      body = [
        {'id': 'c1', 'title': '新对话', 'messageCount': 2, 'archived': false},
      ];
    } else if (path == '/api/ai/conversations/c1/messages') {
      body = messages;
    } else if (path == '/api/ai/conversations/c1/runs' &&
        request.method == 'POST') {
      final payload = jsonDecode(request.body) as Map<String, dynamic>;
      final text = payload['text']?.toString() ?? '';
      if (text.startsWith('用我最近的工作流')) {
        // 第一轮：走 r1（就是那条带提问卡的流）
        body = {
          'runId': 'r1',
          'assistantMessageId': 'a1',
          'userMessageId': 'u1',
        };
      } else {
        // 提交之后的新一轮：正文原样记下来，用例据此断言拼出来的消息
        sentText = text;
        // 这条用户消息要进"库"：一轮结束后界面会与服务端对齐，
        // 库里没有它的话，结尾提问卡的"被新提问顶掉"状态会当场回滚。
        messages.add({
          'id': 'u${messages.length + 1}',
          'conversationId': 'c1',
          'seq': messages.length + 1,
          'role': 'user',
          'status': 'complete',
          'text': text,
          'parts': [
            {'type': 'text', 'text': text},
          ],
        });
        body = {
          'runId': 'r2',
          'assistantMessageId': 'a2',
          'userMessageId': 'u2',
        };
      }
    } else if (path == '/api/ai/runs/r1/events') {
      return http.Response.bytes(
        utf8.encode(_stream()),
        200,
        headers: {'content-type': 'text/event-stream; charset=utf-8'},
      );
    } else if (path == '/api/ai/runs/r2/events') {
      return http.Response.bytes(
        utf8.encode(_stream(secondRun: true)),
        200,
        headers: {'content-type': 'text/event-stream; charset=utf-8'},
      );
    } else if (path.startsWith('/api/ai/questions/') &&
        path.endsWith('/answer')) {
      answered.add({
        'id': path.split('/')[4],
        ...jsonDecode(request.body) as Map<String, dynamic>,
      });
      body = {'accepted': true};
    } else if (path.startsWith('/api/ai/questions/') &&
        path.endsWith('/skip')) {
      skipped.add(path.split('/')[4]);
      body = {'accepted': true};
    } else if (path == '/api/ai/comfy/jobs') {
      body = {'running': 0, 'pending': 0, 'jobs': <Object>[]};
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

Future<AiWorkspaceStore> _open(WidgetTester tester, _Fake fake) async {
  tester.view.physicalSize = const Size(1400, 1100);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: fake.client()),
  );
  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
      ],
      child: const MaterialApp(home: AiHomePage()),
    ),
  );
  await tester.pumpAndSettle();
  unawaited(store.send('用我最近的工作流画一张'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 50));
  return store;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    answered.clear();
    skipped.clear();
  });

  testWidgets('结尾提问不接管输入框，卡片分页显示', (tester) async {
    final store = await _open(tester, _Fake());
    // 非阻塞：不进入"等你回答"，底部仍是普通输入框
    expect(store.pendingQuestion, isNull, reason: '结尾提问不该接管输入区');
    expect(find.byKey(const Key('ask-follow-up-submit')), findsOneWidget);
    expect(find.byKey(const Key('ask-answer-submit')), findsNothing);
    expect(find.text('第 1/2 题'), findsOneWidget);
    expect(find.text('工作流'), findsOneWidget);
    expect(find.text('跑哪一份？'), findsOneWidget);
    expect(find.text('下一题'), findsOneWidget);
  });

  testWidgets('提交：逐题记录答案 + 拼成一条用户消息（没选的写「没有选择」）', (tester) async {
    final fake = _Fake();
    await _open(tester, fake);

    // 第 1 题选第一个选项
    await tester.tap(find.byKey(const Key('ask-follow-up-option-0')));
    await tester.pump();
    await tester.tap(find.text('下一题'));
    await tester.pump();
    expect(find.text('第 2/2 题'), findsOneWidget);

    // 第 2 题不选（提交时按"没有选择"记）
    await tester.enterText(
      find.byKey(const Key('ask-follow-up-supplement')),
      '顺便给我 YAML 配置',
    );
    await tester.tap(find.byKey(const Key('ask-follow-up-submit')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(answered.single['id'], _q1);
    expect(answered.single['answer'], '应用内提交的那份');
    expect(skipped, contains(_q2), reason: '没选的题按跳过记录，不许替用户补答案');
    expect(fake.sentText, isNotNull, reason: '提交必须发出一条新的用户消息');
    expect(fake.sentText, contains('工作流：应用内提交的那份'));
    expect(fake.sentText, contains('数量：没有选择'));
    expect(fake.sentText, contains('顺便给我 YAML 配置'));
    expect(fake.sentText, contains('请按这个继续'));

    // 卡片收成"已回答"记录
    expect(find.text('已回答'), findsOneWidget);
    expect(find.byKey(const Key('ask-follow-up-submit')), findsNothing);
  });

  testWidgets('每题都可以不选：直接提交也能发出（全部「没有选择」）', (tester) async {
    final fake = _Fake();
    await _open(tester, fake);
    await tester.tap(find.text('下一题'));
    await tester.pump();
    await tester.tap(find.byKey(const Key('ask-follow-up-submit')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(answered, isEmpty);
    expect(skipped, containsAll([_q1, _q2]));
    expect(fake.sentText, contains('工作流：没有选择'));
    expect(fake.sentText, contains('数量：没有选择'));
  });

  // 用户反馈（2026-11-13）：提交后要**当场**收成"已回答"记录，不用退出重进；
  // 记录默认收起，但展开要能回看当时给过哪些选项、选的是哪一个。
  testWidgets('提交后立刻收成记录卡，展开能看当时的选项', (tester) async {
    final fake = _Fake();
    await _open(tester, fake);

    // 第 1 题选第一个；第 2 题不选（跳过）
    await tester.tap(find.byKey(const Key('ask-follow-up-option-0')));
    await tester.pump();
    await tester.tap(find.text('下一题'));
    await tester.pump();
    await tester.tap(find.byKey(const Key('ask-follow-up-submit')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pumpAndSettle();

    // ① 立刻是"已回答"（不等重开对话），并且默认收起
    expect(find.text('已回答'), findsOneWidget);
    expect(find.byKey(const Key('ask-follow-up-submit')), findsNothing);
    expect(find.text('跑哪一份？'), findsNothing, reason: '记录卡默认收起');

    // ② 展开：题目、当时的每个选项、选中的答案都在
    await tester.tap(find.byKey(const Key('ask-follow-up-record-toggle')));
    await tester.pump();
    expect(find.text('跑哪一份？'), findsOneWidget);
    expect(find.text('要几张？'), findsOneWidget);
    // 选项与答案文字可能是同一个字符串（选了第一项），所以按"至少一个"断言
    expect(find.text('应用内提交的那份'), findsWidgets);
    expect(find.text('ComfyUI 里跑的那份'), findsOneWidget);
    expect(find.text('1 张'), findsOneWidget);
    expect(find.text('4 张'), findsOneWidget);
    // 跳过的那题也要如实回放
    expect(find.text('（已跳过，没有回答）'), findsOneWidget);
  });

  // 用户要求：**不管**新提问是打字发的还是点卡片提交的，旧提问都要自动折叠。
  testWidgets('用户自己发一条新消息：旧提问卡自动收起，仍可展开继续答', (tester) async {
    final fake = _Fake();
    await _open(tester, fake);
    expect(find.byKey(const Key('ask-follow-up-submit')), findsOneWidget);

    // 不点卡片，直接在底部输入区发一条自己的消息
    final composer = find.byWidgetPredicate(
      (w) =>
          w is TextField &&
          (w.decoration?.hintText ?? '').startsWith('描述你的生图需求'),
    );
    await tester.enterText(composer, '先不管这个，换个问题');
    await tester.tap(find.text('发送'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pumpAndSettle();

    // ① 旧提问卡收成一行标题，选项区不再占版面
    expect(
      find.byKey(const Key('ask-follow-up-submit')),
      findsNothing,
      reason: '用户已经往下问了，旧提问要自动收起',
    );
    expect(
      find.byKey(const Key('ask-follow-up-toggle')),
      findsOneWidget,
      reason: '收起后仍留一个展开入口',
    );

    // ② 展开后仍能看到（并继续作答）当时的选项
    await tester.tap(find.byKey(const Key('ask-follow-up-toggle')));
    await tester.pump();
    expect(find.text('跑哪一份？'), findsOneWidget);
    expect(find.byKey(const Key('ask-follow-up-submit')), findsOneWidget);
  });
}
