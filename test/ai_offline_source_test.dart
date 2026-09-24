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
import 'package:viewer/core/credential_store.dart';
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

  /// 本地源要不要"有规则工件"（默认有：多数用例不关心这件事）。
  bool hasKit = true;

  /// 上一次进程被杀时留下的"没跑完的 Run"有几条。
  int unfinished = 0;

  @override
  Future<int> recoverUnfinishedRuns() async {
    calls.add('recoverUnfinishedRuns');
    return unfinished;
  }

  /// 上行（M4）：默认"推上去 N 条"，用例可改。
  int outboxApplied = 0;
  int outboxPending = 0;
  String? outboxError;
  final List<String?> syncedBaseUrls = [];

  @override
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  }) async {
    calls.add('syncNow');
    syncedBaseUrls.add(baseUrl);
    return (
      ok: outboxError == null,
      applied: outboxApplied,
      pending: outboxPending,
      error: outboxError,
    );
  }

  @override
  Future<({bool ok, String? kitVersion, String? reason})> ensureRunKit({
    String? baseUrl,
    String? token,
  }) async {
    calls.add('ensureRunKit');
    return hasKit
        ? (ok: true, kitVersion: '2026-09-24.1', reason: null)
        : (ok: false, kitVersion: null, reason: '本地还没有规则工件（run-kit），且当前连不上桌面');
  }

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

  test('手机冷启动：桌面离线也能进聊天页（会话来自本地库，不报"连接后端失败"）', () async {
    // 远端全部 500（= 桌面不在线）
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async => http.Response('offline', 500)),
    );
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(api: api, source: source);

    await store.load();

    expect(store.loaded, isTrue, reason: '本地库拿得到会话，就不算"加载失败"');
    // 本地那条空会话被复用（不是新建的）
    expect(store.conversation?.id, 'local-conv-1');
    expect(source.calls, isNot(contains('createConversation')));
    // 模型清单拿不到要**如实说**，但不能说成"连接后端失败"（用户会以为整个 App 坏了）
    expect(store.error, contains('桌面不在线'));
    expect(store.error, isNot(contains('连接后端失败')));
  });

  test('手机冷启动：本地库一条会话都没有时，新会话建在本地', () async {
    final source = _EmptyLocalSource();
    final store = AiWorkspaceStore(
      api: AiApiClient(
        'http://127.0.0.1:8080',
        client: MockClient((request) async => http.Response('offline', 500)),
      ),
      source: source,
    );

    await store.load();

    expect(source.calls, contains('createConversation'), reason: '新会话要落在本地库');
    expect(store.conversation?.id, 'local-conv-new');
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

  // -------------------------------------------------------------------------
  //  M6：手机侧的凭据 / 清单缓存 / 规则工件 —— "安卓真正能发出去" 的三块砖
  // -------------------------------------------------------------------------

  test('手机：清单里"有没有 key"以**本机密钥库**为准，不是桌面那一份', () async {
    // 桌面说"已配置"（那是桌面托管文件里的 key，**不会下发到手机**）
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: false),
    );

    await store.load();

    expect(store.providers, isNotEmpty, reason: 'error=${store.error}');
    // 桌面那份是 managed/true —— 手机必须换成**本机**的答案，绝不能借它冒充
    expect(store.providers.first.credential.source, 'none');
    expect(store.providers.first.credential.source, isNot('managed'));
    expect(store.providers.first.credential.configured, isFalse);
    expect(store.providers.first.credential.label, '未配置');
    expect(store.providers.first.credentialRef, 'PHONE_KEY', reason: '引用名照旧来自桌面');

    // 本机填过 key 之后，状态立刻翻成 keystore（界面据此显示"已配置（本机密钥库）"）
    final filled = AiWorkspaceStore(
      api: _okApi(),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );
    await filled.load();
    expect(filled.providers.first.credential.source, 'keystore');
    expect(filled.providers.first.credential.label, '已配置（本机密钥库）');
  });

  test('手机：连过一次桌面之后，离线冷启动用本机缓存的清单', () async {
    final creds = _FakeDeviceCredentials(configured: true);

    // 第一次：桌面在线 —— 顺便把清单缓存下来
    final online = AiWorkspaceStore(api: _okApi(), source: _FakeLocalSource(), credentials: creds);
    await online.load();
    expect(online.providers.single.id, 'p1');
    expect(online.models.single.id, 'm-text');

    // 第二次：桌面离线 —— 仍然拿得到 provider 与模型（否则手机连"发"的资格都没有）
    final offline = AiWorkspaceStore(
      api: AiApiClient(
        'http://127.0.0.1:8080',
        client: MockClient((request) async => http.Response('offline', 500)),
      ),
      source: _FakeLocalSource(),
      credentials: creds,
    );
    await offline.load();

    expect(offline.providers.single.id, 'p1',
        reason: 'provider 清单是桌面下发的工件，手机要把它留在本机（M6）');
    expect(offline.models.single.id, 'm-text', reason: '模型目录也一样');
    expect(offline.models.single.tools, isTrue, reason: '缓存不能丢能力声明：丢了工具就没了');
    expect(offline.kitIssue, isNull, reason: '假源有 kit，不该被拦');
    expect(offline.notice, contains('缓存'));
  });

  test('手机：没有规则工件（run-kit）时**不许开跑**，而且要说清为什么', () async {    final source = _FakeLocalSource()..hasKit = false;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(source.calls, contains('ensureRunKit'), reason: '冷启动就该去取/刷新规则工件');
    final reason = store.sendBlockReason();
    expect(reason, isNotNull);
    expect(reason, contains('run-kit'));

    // 真的发不出去（不是只有提示、实际照发）
    await store.send('画一只猫');
    expect(source.calls, isNot(contains('startRun')));
  });

  test('手机冷启动：先给上次没跑完的 Run 收尾（被系统杀掉是常态）', () async {
    final source = _FakeLocalSource()..unfinished = 1;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(source.calls, contains('recoverUnfinishedRuns'));
    expect(store.recoveredRuns, 1);
    // 顺序：收尾必须在拉会话**之前**（否则界面上先出现一个永远转圈的幽灵气泡）
    final order = source.calls.where((c) => c == 'recoverUnfinishedRuns' || c == 'listConversations');
    expect(order.first, 'recoverUnfinishedRuns');
  });

  // -------------------------------------------------------------------------
  //  M4：上行 —— 手机产生的会话/消息要能到桌面
  // -------------------------------------------------------------------------

  test('手机冷启动会推一次上行，并把"推了几条"如实说出来', () async {
    final source = _FakeLocalSource()
      ..outboxApplied = 3
      ..outboxPending = 1;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(source.calls, contains('syncNow'), reason: '冷启动要把本机攒的东西推给桌面（M4）');
    expect(store.syncedToDesktop, 3);
    expect(store.pushNotice, contains('3 条'));
    expect(store.pushNotice, contains('还有 1 条待同步'), reason: '"还剩几条"必须说出来');
  });

  test('上行失败不拦聊天页（离线是手机的常态）', () async {
    final source = _FakeLocalSource()
      ..outboxPending = 2
      ..outboxError = '连不上上游：ConnectException';
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    // 页面照常可用，只是如实记一句"还有几条没上去"
    expect(store.loaded, isTrue);
    expect(store.pushNotice, contains('2 条'));
    expect(store.pushNotice, contains('ConnectException'));
  });

  test('一轮跑完之后也会顺手推一次（那时内容才定稿）', () async {
    final source = _FakeLocalSource()..outboxApplied = 2;
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080'),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    )
      ..providers = [_provider()]
      ..selectedProvider = _provider()
      ..selectedModel = _model();

    await store.send('画一只猫');

    expect(
      source.calls.where((c) => c == 'syncNow').length,
      greaterThanOrEqualTo(1),
      reason: 'Run 收尾是"内容定稿"的时刻，这时推上去桌面才看得到完整回复',
    );
    expect(store.syncedToDesktop, greaterThan(0));
  });
}

/// 桌面在线：provider 清单 + 模型目录都正常返回（用来灌"本机缓存"）。
///
/// ⚠ 响应必须带 `charset=utf-8`：`http.Response` 默认按 **Latin-1** 编码 body，
/// 中文显示名会在 `http` 层就抛 "Contains invalid characters"（看起来像后端挂了）。
AiApiClient _okApi() => AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        const json = {'content-type': 'application/json; charset=utf-8'};
        if (request.url.path == '/api/ai/providers') {
          return http.Response(
            jsonEncode([
              {
                'id': 'p1',
                'displayName': '本机网关',
                'api': 'openai-completions',
                'baseURL': 'http://127.0.0.1:11434/v1',
                'credentialRef': 'PHONE_KEY',
                'endpointTrust': 'loopback',
                'enabled': true,
                'revision': 1,
                // 桌面侧说"已配置" —— 手机必须用自己的状态覆盖它
                'credential': {'configured': true, 'source': 'managed', 'writable': true},
              }
            ]),
            200,
            headers: json,
          );
        }
        if (request.url.path == '/api/ai/providers/p1/models') {
          return http.Response(
            jsonEncode([
              {
                'providerId': 'p1',
                'id': 'm-text',
                'displayName': '纯文本模型',
                'inputModalities': ['text'],
                'attachmentTransports': <String, dynamic>{},
                'mimeAllowlist': <String>[],
                'tools': true,
                'parallelTools': false,
                'reasoning': false,
                'thinkingEfforts': <String, dynamic>{},
                'capabilitySource': 'manual',
                'enabled': true,
              }
            ]),
            200,
            headers: json,
          );
        }
        return http.Response(jsonEncode(<String, dynamic>{}), 200, headers: json);
      }),
    );

/// 假的本机密钥库（手机上真身是 Android Keystore，走桥）。
class _FakeDeviceCredentials implements CredentialStore {
  _FakeDeviceCredentials({required this.configured});

  bool configured;

  @override
  bool get isLocal => true;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async {
    final ref = provider.credentialRef?.trim();
    if (ref == null || ref.isEmpty) {
      return const AiCredentialStatus(configured: false, source: 'none', writable: false);
    }
    return AiCredentialStatus(
      configured: configured,
      source: configured ? 'keystore' : 'none',
      writable: true,
    );
  }

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) async {
    configured = true;
    return statusOf(provider);
  }

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) async {
    configured = false;
    return statusOf(provider);
  }
}

/// 只回 `runId`、不回消息 id 的事件源（模拟桥漏传那一对主键）。
class _BrokenSource extends _FakeLocalSource {
  @override
  Future<RunHandles> startRun(RunStartRequest request) async =>
      RunHandles(runId: 'r1', conversationId: request.conversationId);
}

/// 本地库空库（第一次装 App）：冷启动必须**在本地**建一条新会话。
class _EmptyLocalSource extends _FakeLocalSource {
  @override
  Future<List<AiConversation>> listConversations() async => const [];

  @override
  Future<AiConversation> createConversation({
    String? conversationId,
    String? title,
    String? providerId,
    String? modelId,
  }) async {
    calls.add('createConversation');
    return AiConversation(id: 'local-conv-new', title: title ?? '新对话');
  }
}
