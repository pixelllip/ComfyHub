// 回归（用户 bug 2026-09-21：`ask_user_question` 的作答体验不完善）：
//
//   ① 提问要**向用户抛出选项**，点一下就答；末尾**固定**再给一条"自己输入答案"
//      （用户原话："最末尾选项固定让用户决定（输入你的答案）"）；
//   ② 提问期间**接管聊天下部的模块**（原来的输入框整块换成答题卡），
//      参考 DeepSeek Harness 的 composer takeover —— 答完 / 跳过后普通输入框自动回来。
//
// 两条都是"界面结构"级别的约定：出了问题用户是直接看不懂该怎么答，所以这里
// 走真实链路（起一次 Run + 手拼 SSE 的 `user.question` 事件），把四件事钉住：
//   · `user.question` 到达后：答题卡出现、问题与选项都在、末尾固定是"自己输入答案"；
//   · 点选项 → POST /api/ai/questions/{id}/answer（带 optionIndex、不带文字）；
//   · 自己打字 → POST 带上 answer 文本；
//   · 跳过 → POST .../skip；答完输入区**让位**（普通输入框回来）。

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

const _questionId = 'q-1';
const _question = '这次用哪张参考图？';
const _options = ['用附件里的那张猫', '用上一轮的图'];

/// 按真实 SSE 格式拼事件（冒号后必须有空格）。
String _sse(int seq, String type, String dataJson) =>
    'id: $seq\nevent: $type\ndata: $dataJson\n\n';

/// 假后端收到的"答题"请求（按顺序）：用来断言界面上的动作真的打到了对的接口。
final List<Map<String, dynamic>> answered = [];

/// 假后端收到的跳过请求。
final List<String> skipped = [];

/// 提问事件（`user.question` 的 payload 就是 `PendingQuestion.toJson()`）。
///
/// ⚠️ **这里刻意长得和真后端一模一样**（2026-09-21 按 `ai_run_events` 里那条真事件抄的）：
///  - 真事件里**没有 `callId`**（不要加，加了就测不出下面那个 bug）；
///  - `question` 是**内层对象**：`PendingQuestion.toJson()` 整份塞在 `question` 键下，
///    外面只多一个 `runId`。
String _askEvent() => _sse(
      3,
      'user.question',
      jsonEncode({
        'runId': 'r1',
        'question': {
          'questionId': _questionId,
          'runId': 'r1',
          'conversationId': 'c1',
          'question': _question,
          'header': '选择题',
          'options': _options,
          'allowFreeText': true,
          'answered': false,
          'askedAt': 1,
        },
      }),
    );

/// `user.question` **之前**一定会先来这两条（工具调用是"先 requested、再 started、
/// 然后才轮到工具自己去问"）。它们的 `callId` 是**模型的调用号**，
/// 与 `user.question` 里的 `questionId` **不是同一个值** ——
/// 2026-09-21 的 bug 就出在这里：提问卡被挂在 `questionId` 这个键上，
/// 于是 `pendingQuestion` 在 `callId` 那条上找不到问题，输入区永远不接管。
String _askToolEvents() => _sse(
      1,
      'tool.requested',
      jsonEncode({
        'runId': 'r1',
        'callId': _callId,
        'name': 'ask_user_question',
        'arguments': '{"question":"$_question"}',
        'approval': 'not_required',
      }),
    ) +
    _sse(2, 'tool.started', jsonEncode({'callId': _callId, 'name': 'ask_user_question'}));

const _callId = 'call_00_w3oy2hgdwhoanKYvvw2Y8083';

/// 事件体的拼装（抽出来是为了在用例里能断言"假后端真的发了这些事件"）。
String _body() =>
    _sse(0, 'run.started', '{"runId":"r1"}') +
    _sse(1, 'message.started', '{"messageId":"a1"}') +
    // 真事件的顺序：tool.requested → tool.started → user.question
    _askToolEvents() +
    _askEvent() +
    _askCompleted();

/// Run 收尾时的落库形态（parts 里那条 `tool_call` + `tool_result`）：
/// 答完之后这张卡要切成"已回答"，历史里一眼能看出这一轮基于什么做的。
String _askCompleted() => _sse(
      4,
      'message.completed',
      jsonEncode({
        'messageId': 'a1',
        'text': '好的。',
        'steps': 1,
        'parts': [
          {'type': 'text', 'text': '好的。'},
          {
            'type': 'tool_call',
            'toolCallId': _questionId,
            'jsonPayload': {
              'name': 'ask_user_question',
              'arguments': '{"question":"$_question"}',
            },
          },
          {
            'type': 'tool_result',
            'toolCallId': _questionId,
            'text': '【用户回答】',
            'jsonPayload': {
              'name': 'ask_user_question',
              'ok': true,
              'question': {
                'questionId': _questionId,
                'question': _question,
                'options': _options,
                'allowFreeText': true,
                'answered': true,
              },
            },
          },
        ],
      }),
    ) +
    _sse(5, 'run.completed', '{"runId":"r1"}');

/// 假后端：SSE 里 `user.question` 之后**还跟着** `message.completed` + `run.completed`
/// —— 那是"用户回答之后"才发生的收尾。事件流是**一次性全部给出**的（客户端的
/// `runEvents` 一次性读完再逐条 yield），但客户端的 `_consumeRun` 会在
/// `user.question` 那条上挂住等回答，所以用例看到的仍然是"提问时 Run 还没结束"。
class _Fake {
  List<Map<String, dynamic>> _messages() => [
        {
          'id': 'u1',
          'conversationId': 'c1',
          'seq': 1,
          'role': 'user',
          'status': 'complete',
          'text': '帮我生一张图',
          'parts': [
            {'type': 'text', 'text': '帮我生一张图'},
          ],
        },
        {
          'id': 'a1',
          'conversationId': 'c1',
          'seq': 2,
          'role': 'assistant',
          // 还在等回答：这一轮没收尾（真后端此时也还是 streaming）
          'status': 'streaming',
          'text': '好的。',
          'parts': [
            {'type': 'text', 'text': '好的。'},
          ],
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
              'tools': true,
              'reasoning': false,
              'thinkingEfforts': <String, String>{},
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
          body = [
            {'id': 'c1', 'title': '新对话', 'messageCount': 2, 'archived': false},
          ];
        } else if (path == '/api/ai/conversations/c1/messages') {
          body = _messages();
        } else if (path == '/api/ai/conversations/c1/runs') {
          body = {'runId': 'r1', 'assistantMessageId': 'a1', 'userMessageId': 'u1'};
        } else if (path == '/api/ai/runs/r1/events') {
          // 客户端 `runEvents` 会把整条流读完再逐条 yield，所以这里**一次性**
          // 把"提问 + 收尾"都给出去。用例看到的仍然是"提问时 Run 还没结束"：
          // `_consumeRun` 在 `user.question` 那条上就挂住等用户回答了。
          return http.Response.bytes(
            utf8.encode(
              _body(),
            ),
            200,
            headers: {'content-type': 'text/event-stream; charset=utf-8'},
          );
        } else if (path == '/api/ai/questions/$_questionId/answer') {
          answered.add(jsonDecode(request.body) as Map<String, dynamic>);
          body = {'accepted': true};
        } else if (path == '/api/ai/questions/$_questionId/skip') {
          skipped.add(_questionId);
          body = {'accepted': true};
        } else if (path == '/api/ai/runs/r1/question') {
          body = {'pending': false};
        } else if (path == '/api/ai/comfy/jobs') {
          body = {'running': 0, 'pending': 0, 'jobs': <Object>[]};
        } else {
          body = <Object>[];
        }
        return http.Response(jsonEncode(body), 200,
            headers: {'content-type': 'application/json; charset=utf-8'});
      });
}

/// 起一次 Run 并等到提问卡出现（事件流挂在提问上，Run 不会结束）。
Future<AiWorkspaceStore> _openAsk(
  WidgetTester tester,
  _Fake fake, {
  Size size = const Size(1400, 1200),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: fake.client()),
  );
  await tester.pumpWidget(MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  ));
  await tester.pumpAndSettle();

  unawaited(store.send('帮我生一张图'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 50));
  expect(store.pendingQuestion?.questionId, _questionId, reason: '事件到了就该进"等你回答"状态');
  return store;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    answered.clear();
    skipped.clear();
  });

  testWidgets('提问接管输入区：选项 + 末尾固定「自己输入答案」', (tester) async {
    await _openAsk(tester, _Fake());

    expect(find.text(_question), findsWidgets, reason: '问题要显示出来');
    expect(find.text(_options[0]), findsWidgets);
    expect(find.text(_options[1]), findsWidgets);
    expect(find.text('自己输入答案'), findsWidgets, reason: '末尾固定留给用户自己决定');
    expect(find.byKey(const Key('ask-answer-field')), findsOneWidget);
  });

  // 用户 bug 2026-09-21 的**根因回归**：`user.question` 事件里没有 `callId`，
  // 提问卡必须贴到 `tool.requested` 那条已经存在的调用上，不能按 `questionId` 另开一条。
  // 另开一条的后果就是用户截图那样：工具卡一直在"运行中"，提问卡根本不出现、输入区不接管。
  testWidgets('提问卡贴在模型那条调用上：只有一张卡、callId 不被 questionId 顶掉', (tester) async {
    final store = await _openAsk(tester, _Fake());
    final message = store.messages.firstWhere((m) => !m.isUser);

    final calls = store.toolCallsFor(message);
    expect(calls, hasLength(1), reason: '一次提问只能有一条调用，别按 questionId 再开一条');
    expect(calls.single.callId, _callId, reason: '模型那条调用的 callId 不能被 questionId 顶掉');
    expect(calls.single.question?.questionId, _questionId);
    expect(calls.single.question?.callId, _callId, reason: '问题里也要记着它挂在哪个调用上');

    // 界面上：一张提问卡（问题+选项），而不是"工具卡里摊着一份参数 JSON、状态还在运行中"
    expect(find.text('参数'), findsNothing, reason: '提问卡不该画成工具卡（那张卡会摊出参数 JSON）');
    expect(find.text('ask_user_question'), findsNothing, reason: '提问卡不显示工具名');
    expect(find.text(_question), findsWidgets);
    expect(find.text('自己输入答案'), findsWidgets);
  });

  testWidgets('点选项即答：POST 带 optionIndex，答完输入区让位', (tester) async {
    final store = await _openAsk(tester, _Fake());

    await tester.tap(find.text(_options[0]).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(answered.single['optionIndex'], 0);
    expect(answered.single['answer'], isNull, reason: '点选项走 optionIndex，别再带一段文字');
    expect(store.pendingQuestion, isNull, reason: '答完就该退回普通输入区');
    expect(find.byKey(const Key('ask-answer-field')), findsNothing);
  });

  testWidgets('自己输入答案：POST 带文本', (tester) async {
    await _openAsk(tester, _Fake());

    await tester.enterText(find.byKey(const Key('ask-answer-field')), '用我自己传的那张');
    await tester.pump();
    await tester.tap(find.byKey(const Key('ask-answer-submit')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(answered.single['answer'], '用我自己传的那张');
    expect(answered.single['optionIndex'], isNull);
  });

  testWidgets('跳过这个问题：POST .../skip', (tester) async {
    final store = await _openAsk(tester, _Fake());

    await tester.tap(find.text('跳过这个问题').first);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(skipped, [_questionId]);
    expect(store.pendingQuestion, isNull);
  });

  testWidgets('窄窗口下答题卡排得下：输入框与提交按钮都在，不溢出', (tester) async {
    // 窗口窄到 360（App 的最小宽度是 320）：答题卡里的选项行 / 输入行 / 底行都要自己折，
    // 不能横向溢出 —— 溢出在 `flutter test` 里会直接抛异常，这个用例就是那道闸。
    await _openAsk(tester, _Fake(), size: const Size(360, 900));

    expect(find.byKey(const Key('ask-answer-field')), findsOneWidget);
    expect(find.byKey(const Key('ask-answer-submit')), findsOneWidget);
    expect(find.text('跳过这个问题'), findsWidgets);
  });
}
