import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/core/run_event_source.dart';

/// `RunEventSource` 的**本地实现**（手机 harness 的通道那一侧）。
///
/// 为什么这条防线值得存在：手机上的事件由 Kotlin 产生，却由**与桌面同一套** Flutter 代码渲染。
/// 通道两端的约定一旦对不上（键名、payload 是对象还是字符串、`after` 的语义），
/// 表现是"界面上某个字段永远是空的"—— 编译通过、两边各自的用例都可能全绿。
/// 所以这里把**通道协议**用假通道钉住。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const methodChannel = MethodChannel(LocalHarnessRunEventSource.methodChannelName);
  const eventChannel = EventChannel(LocalHarnessRunEventSource.eventChannelName);
  final messenger = TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  final methodCalls = <MethodCall>[];

  setUp(() {
    methodCalls.clear();
  });

  tearDown(() {
    messenger.setMockMethodCallHandler(methodChannel, null);
    messenger.setMockStreamHandler(eventChannel, null);
  });

  LocalHarnessRunEventSource source() =>
      LocalHarnessRunEventSource(methodChannel: methodChannel, eventChannel: eventChannel);

  void mockMethods(Future<Object?> Function(MethodCall call) handler) {
    messenger.setMockMethodCallHandler(methodChannel, (call) async {
      methodCalls.add(call);
      return handler(call);
    });
  }

  test('isLocal 为真，且通道名与 Kotlin 侧一致（改了要同时改两边）', () {
    expect(source().isLocal, isTrue);
    expect(LocalHarnessRunEventSource.methodChannelName, 'comfyhub/harness');
    expect(LocalHarnessRunEventSource.eventChannelName, 'comfyhub/harness/events');
  });

  test('startRun 把参数原样送到 Kotlin，并解析回 runId/conversationId', () async {
    mockMethods((call) async {
      expect(call.method, 'startRun');
      final args = Map<String, dynamic>.from(call.arguments as Map);
      expect(args['conversationId'], 'c1');
      expect(args['text'], '画一只猫');
      expect(args['api'], 'openai-completions');
      expect(args['baseUrl'], 'https://api.example.com/v1');
      expect(args['credentialRef'], 'ref-1');
      return jsonEncode({
        'runId': 'r1',
        'conversationId': 'c1',
        'kitVersion': '2026-09-23.1',
        'promptVersion': 'v28',
      });
    });

    final handles = await source().startRun(const RunStartRequest(
      conversationId: 'c1',
      text: '画一只猫',
      providerId: 'p1',
      modelId: 'm1',
      api: 'openai-completions',
      baseUrl: 'https://api.example.com/v1',
      credentialRef: 'ref-1',
    ));

    expect(handles.runId, 'r1');
    expect(handles.conversationId, 'c1');
    expect(handles.kitVersion, '2026-09-23.1');
  });

  test('startRun 把界面生成的两个消息 id 送下去，并读回 Kotlin 的回显（M3.2）', () async {
    mockMethods((call) async {
      expect(call.method, 'startRun');
      final args = Map<String, dynamic>.from(call.arguments as Map);
      // 界面生成的 id 必须**真的送到** Kotlin（否则 harness 会另起 UUID，
      // 界面手里的 id 就永远查不到库里那条 —— 事件齐全但界面一个字都不长）
      expect(args['userMessageId'], 'ui-user-1');
      expect(args['assistantMessageId'], 'ui-assistant-1');
      return jsonEncode({
        'runId': 'r1',
        'conversationId': 'c1',
        'userMessageId': 'ui-user-1',
        'assistantMessageId': 'ui-assistant-1',
        'kitVersion': 'k1',
        'promptVersion': 'v1',
      });
    });

    final handles = await source().startRun(const RunStartRequest(
      conversationId: 'c1',
      text: '你好',
      userMessageId: 'ui-user-1',
      assistantMessageId: 'ui-assistant-1',
    ));

    expect(handles.userMessageId, 'ui-user-1');
    expect(handles.assistantMessageId, 'ui-assistant-1');
  });

  test('createConversation 在本地库建会话，并带回那条会话（M3.2 ② 的第一步）', () async {
    mockMethods((call) async {
      expect(call.method, 'createConversation');
      final args = Map<String, dynamic>.from(call.arguments as Map);
      expect(args['conversationId'], 'conv-ui-1');
      expect(args['title'], '手机上起的');
      expect(args['providerId'], 'p1');
      return jsonEncode({
        'id': 'conv-ui-1',
        'title': '手机上起的',
        'providerId': 'p1',
        'modelId': 'm1',
        'updatedAt': '2026-09-24T10:00:00.000Z',
      });
    });

    final conv = await source().createConversation(
      conversationId: 'conv-ui-1',
      title: '手机上起的',
      providerId: 'p1',
      modelId: 'm1',
    );

    expect(conv['id'], 'conv-ui-1');
    expect(conv['title'], '手机上起的');
    expect(conv['modelId'], 'm1');
  });

  test('fetchRunKit 把 {ok=false, reason} 原样带回来（拿不到规则就不许开跑）', () async {
    mockMethods((call) async {
      expect(call.method, 'fetchRunKit');
      return jsonEncode({'ok': false, 'reason': '桌面已有更新的规则版本（2026-09-24.1 ≠ 2026-09-23.1）'});
    });

    final result = await source().fetchRunKit(baseUrl: 'http://desk:8080');
    expect(result['ok'], isFalse);
    expect(result['reason'].toString(), contains('规则版本'));
  });

  test('cachedRunKit 在没缓存时返回 null', () async {
    mockMethods((call) async => call.method == 'cachedRunKit' ? null : '{}');
    expect(await source().cachedRunKit(), isNull);
  });

  test('事件流：{seq,type,payloadJson} 映射成 AiRunEvent，payload 被解开且带 seq', () async {
    messenger.setMockStreamHandler(
      eventChannel,
      MockStreamHandler.inline(
        onListen: (arguments, sink) {
          // Kotlin 侧会带上 Dart 给的 runId / after（订阅参数）
          expect(arguments, {'runId': 'r1', 'after': 3});
          sink.success({'seq': 4, 'type': 'text.delta', 'payloadJson': '{"text":"你好"}'});
          sink.success({'seq': 5, 'type': 'run.failed', 'payloadJson': '{"code":"RATE_LIMIT","message":"被限流"}'});
          sink.endOfStream();
        },
      ),
    );

    final events = await source().events('r1', after: 3).toList();
    expect(events, hasLength(2));
    expect(events.first.type, 'text.delta');
    expect(events.first.text, '你好');
    expect(events.first.data['seq'], 4);
    // run.failed 的键是 code / message（与桌面逐字一致）—— 读错键，界面上就是一句空提示
    expect(events.last.type, 'run.failed');
    expect(events.last.code, 'RATE_LIMIT');
    expect(events.last.message, '被限流');
    expect(events.last.isTerminal, isTrue);
  });

  test('answerQuestion / cancelRun / recoverUnfinished 走对应的方法名', () async {
    mockMethods((call) async => switch (call.method) {
          'answerQuestion' => true,
          'cancelRun' => true,
          'recoverUnfinished' => 2,
          'pendingOutbox' => 3,
          _ => null,
        });

    final s = source();
    await s.answerQuestion('q1', '要赛博朋克', optionIndex: 1);
    await s.cancelRun('r1');
    expect(await s.recoverUnfinished(), 2);
    expect(await s.pendingOutbox(), 3);

    expect(methodCalls.map((c) => c.method).toList(),
        ['answerQuestion', 'cancelRun', 'recoverUnfinished', 'pendingOutbox']);
    final answerArgs = Map<String, dynamic>.from(methodCalls.first.arguments as Map);
    expect(answerArgs['questionId'], 'q1');
    expect(answerArgs['answer'], '要赛博朋克');
    expect(answerArgs['optionIndex'], 1);
  });

  test('本地会话列表：Kotlin 给 JSON 字符串，这里解成 Map 列表', () async {
    mockMethods((call) async => jsonEncode([
          {'id': 'c1', 'title': '离线时起的会话', 'updatedAt': '2026-09-23T05:30:00.000Z'},
        ]));
    final list = await source().listConversations();
    expect(list, hasLength(1));
    expect(list.single['title'], '离线时起的会话');
  });
}
