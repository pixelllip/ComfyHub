// 回归（M3.2 ②「让手机离线真的能用」）：
//
// 手机上的 Run 由**本地 Kotlin harness** 执行，桌面不在线也必须能开会话、发消息、收流式。
// 这件事的全部风险都在"接错线"上：只把**事件流**换成本地源是不够的 ——
// 会话与消息若仍打远端 HTTP，离线时就是"事件一条不少、界面上一个字都不长"，
// 而两端都不报错（`cba4016` 修掉的就是同一类静默故障）。
//
// 所以这里钉住三件事：
//   ① 平台默认值：Android → 本地源，其余 → 远端源；
//   ② `send()` 的**持久化**（建会话 / 拉消息 / 拉会话列表）走的是同一个 `RunEventSource`；
//   ③ 走本地源时**一个远端请求都不打**（真的没有偷偷回落到 HTTP）。

import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/run_event_source.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/state/ai_workspace_store.dart';

/// 假的**本地**事件源：记录每一次调用，证明界面真的从它这里走。
class _FakeLocalSource implements RunEventSource {
  final List<String> calls = [];
  final List<RunStartRequest> starts = [];
  final List<String> listedConversations = [];

  @override
  bool get isLocal => true;

  @override
  Future<AiConversation> createConversation({
    String? conversationId,
    String? title,
    String? providerId,
    String? modelId,
  }) async {
    calls.add('createConversation');
    return AiConversation(
      id: conversationId ?? 'local-conv-1',
      title: title ?? '新对话',
      providerId: providerId,
      modelId: modelId,
    );
  }

  @override
  Future<List<AiMessage>> listMessages(String conversationId) async {
    listedConversations.add(conversationId);
    // 真 harness 的行为：库里有什么就回什么（起过 Run 之后就有一问一答）
    if (_started.isNotEmpty) {
      return [
        for (final (i, m) in _started.indexed) ...[
          AiMessage(
            id: m.$1,
            conversationId: conversationId,
            seq: i * 2 + 1,
            role: 'user',
            status: 'complete',
            text: '库里的提问',
          ),
          AiMessage(
            id: m.$2,
            conversationId: conversationId,
            seq: i * 2 + 2,
            role: 'assistant',
            status: 'complete',
            text: '库里的回答',
          ),
        ],
      ];
    }
    return const [
      AiMessage(
        id: 'lib-user',
        conversationId: 'local-conv-1',
        seq: 1,
        role: 'user',
        status: 'complete',
        text: '本地库里的老消息',
      ),
    ];
  }

  final List<(String, String)> _started = [];

  @override
  Future<List<AiConversation>> listConversations() async {
    calls.add('listConversations');
    return const [AiConversation(id: 'local-conv-1', title: '离线时起的会话')];
  }

  @override
  Future<RunHandles> startRun(RunStartRequest request) async {
    calls.add('startRun');
    starts.add(request);
    // 桥的行为：调用方给了 id 就用它，没给就自己生成（这里固定成可断言的值）
    final userId = request.userMessageId ?? 'lib-user';
    final assistantId = request.assistantMessageId ?? 'lib-assistant';
    _started.add((userId, assistantId));
    return RunHandles(
      runId: 'local-run-1',
      conversationId: request.conversationId,
      userMessageId: userId,
      assistantMessageId: assistantId,
    );
  }

  @override
  Future<void> cancelRun(String runId) async => calls.add('cancelRun');

  @override
  Stream<AiRunEvent> events(String runId, {int after = 0}) => Stream.fromIterable(const [
        AiRunEvent('run.completed', {'runId': 'local-run-1'}),
      ]);

  @override
  Future<bool> answerQuestion(String questionId, String? answer, {int? optionIndex}) async {
    calls.add('answerQuestion');
    return true;
  }
}

AiProvider _provider() => const AiProvider(
      id: 'p1',
      displayName: '本机网关',
      api: 'openai-completions',
      baseURL: 'http://127.0.0.1:11434/v1',
      credentialRef: 'PHONE_KEY',
      endpointTrust: 'loopback',
      enabled: true,
      revision: 1,
      credential: AiCredentialStatus(configured: true, source: 'keystore', writable: true),
    );

AiModel _model() => const AiModel(
      providerId: 'p1',
      id: 'm-text',
      displayName: '纯文本模型',
      inputModalities: ['text'],
      tools: true,
      reasoning: false,
      capabilitySource: 'manual',
      enabled: true,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('平台默认值：Android 用本地 harness，其余平台用远端 SSE', () {
    final api = AiApiClient('http://127.0.0.1:8080');
    // `forceLocal` 就是 Android 那条分支的开关（测试进程里 Platform.isAndroid 恒为 false，
    // 所以用显式参数走同一条代码路径）
    expect(openRunEventSource(api: api, forceLocal: true), isA<LocalHarnessRunEventSource>());
    expect(openRunEventSource(api: api, forceLocal: false), isA<RemoteRunEventSource>());
    // 传了假通道：证明它真的不去碰真通道（构造不抛异常即算过）
    final local = openRunEventSource(
      api: api,
      forceLocal: true,
      methodChannel: const MethodChannel('test/harness'),
      eventChannel: const EventChannel('test/harness/events'),
    );
    expect(local.isLocal, isTrue);
  });

  test('send() 全程走本地源：建会话 / 起 Run / 收事件 / 收尾对齐，且不打任何远端接口', () async {
    final remoteCalls = <String>[];
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        // 本地源就不该有任何远端流量（离线时这些请求全是超时/连接失败）
        remoteCalls.add('${request.method} ${request.url.path}');
        return http.Response(jsonEncode(<String, dynamic>{}), 500);
      }),
    );
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(api: api, source: source)
      ..providers = [_provider()]
      ..selectedProvider = _provider()
      ..selectedModel = _model();

    await store.send('画一只猫');

    expect(source.calls, contains('createConversation'), reason: '会话要落在本地库');
    expect(source.calls, contains('startRun'), reason: 'Run 由本地 harness 执行');
    expect(source.calls, contains('listConversations'), reason: '收尾对齐也走同一个来源');

    // 界面拿到的是**本地库那条会话**（不是远端回的那条）
    expect(store.conversation?.id, 'local-conv-1');
    expect(store.messages.map((m) => m.role).toList(), ['user', 'assistant']);

    // 发出去的请求里带着手机自己发上游要用的三样（桌面那侧由后端查库）
    expect(source.starts.single.api, 'openai-completions');
    expect(source.starts.single.baseUrl, 'http://127.0.0.1:11434/v1');
    expect(source.starts.single.credentialRef, 'PHONE_KEY');

    expect(remoteCalls, isEmpty, reason: '本地源一个远端请求都不该打：$remoteCalls');
  });

  test('切会话时从本地库拉消息（不是远端 HTTP）', () async {
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080'),
      source: source,
    )
      ..conversations = const [AiConversation(id: 'local-conv-1', title: '离线时起的会话')];

    await store.openConversation('local-conv-1');

    expect(source.listedConversations, ['local-conv-1']);
    expect(store.messages.single.text, '本地库里的老消息');
  });

  test('事件源没回传助手消息 id 时当场失败（不许悄悄兜一个本地 id）', () async {
    final source = _BrokenSource();
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080'),
      source: source,
    )
      ..providers = [_provider()]
      ..selectedProvider = _provider()
      ..selectedModel = _model();

    await store.send('画一只猫');

    // 悄悄兜底的话：事件一条不少、界面上一个字都不长，两端还都不报错
    expect(store.error, contains('没有回传助手消息 id'));
  });
}

/// 只回 `runId`、不回消息 id 的事件源（模拟桥漏传那一对主键）。
class _BrokenSource extends _FakeLocalSource {
  @override
  Future<RunHandles> startRun(RunStartRequest request) async =>
      RunHandles(runId: 'r1', conversationId: request.conversationId);
}
