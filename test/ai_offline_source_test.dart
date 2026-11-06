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
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/core/inbound_auth_api.dart';
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

  /// 每次 `syncNow` 带上来的**配对令牌**（配对那条线的回归就盯它）。
  final List<String?> syncedTokens = [];

  @override
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  }) async {
    calls.add('syncNow');
    syncedBaseUrls.add(baseUrl);
    syncedTokens.add(token);
    return (
      ok: outboxError == null,
      applied: outboxApplied,
      pending: outboxPending,
      error: outboxError,
    );
  }

  /// 下行（M4 的另一半）：默认"什么都没拉到"（`up_to_date`），用例按需改。
  bool pullOk = true;
  String pullMode = 'up_to_date';
  int pullApplied = 0;
  int pullCursor = 918342;
  String? pullError;
  final List<String?> pulledBaseUrls = [];
  final List<String?> pulledTokens = [];

  @override
  Future<({bool ok, String mode, int applied, int cursor, String? error})>
  pullChanges({String? baseUrl, String? token}) async {
    calls.add('pullChanges');
    pulledBaseUrls.add(baseUrl);
    pulledTokens.add(token);
    return (
      ok: pullOk,
      mode: pullMode,
      applied: pullApplied,
      cursor: pullCursor,
      error: pullError,
    );
  }

  /// 每次 `ensureRunKit` 带上来的配对令牌。
  final List<String?> kitTokens = [];
  @override
  Future<({bool ok, String? kitVersion, String? reason})> ensureRunKit({
    String? baseUrl,
    String? token,
  }) async {
    calls.add('ensureRunKit');
    kitTokens.add(token);
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
    return List.of(conversations);
  }

  /// 本地库里的会话（默认一条空壳：多数用例只关心"离线也能进聊天页"）。
  final List<AiConversation> conversations = [
    const AiConversation(id: 'local-conv-1', title: '离线时起的会话'),
  ];

  /// 每次 `deleteConversation` 删掉的是哪条（M3.2 ③ 的第二块砖）。
  final List<String> deleted = [];

  /// 本机删除要不要失败（用户主动删会话那条路的"如实报错"用例）。
  bool failDelete = false;

  @override
  Future<void> deleteConversation(String conversationId) async {
    calls.add('deleteConversation');
    if (failDelete) throw StateError('库坏了');
    deleted.add(conversationId);
    conversations.removeWhere((c) => c.id == conversationId);
  }

  /// 本地 `_outbox` 里还剩几条（**不出网**的那次问话，回到前台触发器要先看它）。
  int pendingOutboxCount = 0;

  @override
  Future<int> pendingOutbox() async {
    calls.add('pendingOutbox');
    return pendingOutboxCount;
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
  Stream<AiRunEvent> events(String runId, {int after = 0}) =>
      Stream.fromIterable(const [
        AiRunEvent('run.completed', {'runId': 'local-run-1'}),
      ]);

  @override
  Future<bool> answerQuestion(
    String questionId,
    String? answer, {
    int? optionIndex,
  }) async {
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
  credential: AiCredentialStatus(
    configured: true,
    source: 'keystore',
    writable: true,
  ),
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

  setUp(() {
    SharedPreferences.setMockInitialValues({});
    // 手机侧的 provider / 模型清单现在**只从本机库来**（走桥 → `ProviderStore`）：
    // 每个用例都要有一个应答的假桥（默认：本机库是空的）。用例想塞几条就再调一次。
    mockPairingChannel();
  });

  test('平台默认值：Android 用本地 harness，其余平台用远端 SSE', () {
    final api = AiApiClient('http://127.0.0.1:8080');
    // `forceLocal` 就是 Android 那条分支的开关（测试进程里 Platform.isAndroid 恒为 false，
    // 所以用显式参数走同一条代码路径）
    expect(
      openRunEventSource(api: api, forceLocal: true),
      isA<LocalHarnessRunEventSource>(),
    );
    expect(
      openRunEventSource(api: api, forceLocal: false),
      isA<RemoteRunEventSource>(),
    );
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
    // 一轮跑完是"内容定稿"的时刻：上行推一次、下行也拉一次（M4 的两半并排）
    expect(source.calls, contains('pullChanges'), reason: 'Run 收尾也要拉一次下行');

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
    // 清单拿不到要**如实说**，但不能说成"连接后端失败"（用户会以为整个 App 坏了）。
    // 手机侧清单来自本机库，所以这里的"拿不到"= 本机库还没配过（默认空）——
    // 说的必须是**本机**这件事，而不是"桌面不在线"（桌面在不在都与它无关了）。
    expect(store.error, contains('这台手机上还没有配置任何 Provider'));
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
    final store =
        AiWorkspaceStore(
            api: AiApiClient('http://127.0.0.1:8080'),
            source: source,
          )
          ..conversations = const [
            AiConversation(id: 'local-conv-1', title: '离线时起的会话'),
          ];

    await store.openConversation('local-conv-1');

    expect(source.listedConversations, ['local-conv-1']);
    expect(store.messages.single.text, '本地库里的老消息');
  });

  test('事件源没回传助手消息 id 时当场失败（不许悄悄兜一个本地 id）', () async {
    final source = _BrokenSource();
    final store =
        AiWorkspaceStore(
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
  //  M6：手机侧的凭据 / **本机那份清单** / 规则工件 —— "安卓真正能发出去"的三块砖
  // -------------------------------------------------------------------------

  test('手机：清单来自**本机库**，"有没有 key"以本机密钥库为准（不借桌面那一份）', () async {
    // 本机库里那条 provider：桥回的 DTO 里**没有** `credential` 键
    mockPairingChannel(
      providers: [localProviderJson(credentialRef: 'PHONE_KEY')],
    );
    final hits = <String>[];
    final store = AiWorkspaceStore(
      api: offlineApiWith(hits),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: false),
    );

    await store.load();

    expect(store.providers, isNotEmpty, reason: 'error=${store.error}');
    // 本机密钥库说"没有" —— 绝不能用别处的状态冒充
    expect(store.providers.first.credential.source, 'none');
    expect(store.providers.first.credential.configured, isFalse);
    expect(store.providers.first.credential.label, '未配置');
    expect(
      store.providers.first.credentialRef,
      'PHONE_KEY',
      reason: '引用名来自本机库那一行',
    );
    expect(
      hits.where((h) => h.contains('/api/ai/providers')),
      isEmpty,
      reason: '手机侧清单只读本机库：桌面那条路一条请求都不许打（桌面离线正是常态）',
    );

    // 本机填过 key 之后，状态立刻翻成 keystore（界面据此显示"已配置（本机密钥库）"）
    final filled = AiWorkspaceStore(
      api: offlineApiWith(<String>[]),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );
    await filled.load();
    expect(filled.providers.first.credential.source, 'keystore');
    expect(filled.providers.first.credential.label, '已配置（本机密钥库）');
  });

  test('手机：本机库为空时如实说一句（不静默给一个空选择器）', () async {
    mockPairingChannel(); // 本机库是空的
    final store = AiWorkspaceStore(
      api: offlineApiWith(<String>[]),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(store.providers, isEmpty);
    expect(
      store.error,
      contains('还没有配置任何 Provider'),
      reason: '要如实说，而不是让用户对着空选择器猜',
    );
    expect(store.error, contains('不会自动合并'), reason: '顺带说清手机与桌面是两份清单');
    expect(store.error, contains('AI 模型与凭据'), reason: '要说清去哪儿建');
  });

  test('手机：清单与模型只从**本机库**来 —— 桌面离线照样可用，不再有第二份缓存', () async {
    mockPairingChannel(
      providers: [localProviderJson()],
      models: {
        'p1': [localModelJson()],
      },
    );
    final store = AiWorkspaceStore(
      api: offlineApiWith(<String>[]), // 桌面永远 500
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(store.providers.single.id, 'p1');
    expect(store.models.single.id, 'm-text', reason: '模型目录也从本机库来');
    expect(store.models.single.tools, isTrue, reason: '能力声明不能丢：丢了工具就没了');
    expect(store.kitIssue, isNull, reason: '假源有 kit，不该被拦');

    // ⚠ 那份 SharedPreferences 清单缓存**退役了**（理由见 `_loadLocalStartup` 的注释：
    //   本机库已经是离线可用的真源，两份清单会打架）。这里把它钉死 —— 谁哪天读回来就红。
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('comfyhub.ai.cachedProviders'), isNull);
    expect(prefs.getString('comfyhub.ai.cachedModels.p1'), isNull);
  });

  test('手机：没有规则工件（run-kit）时**不许开跑**，而且要说清为什么', () async {
    final source = _FakeLocalSource()..hasKit = false;
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
    final order = source.calls.where(
      (c) => c == 'recoverUnfinishedRuns' || c == 'listConversations',
    );
    expect(order.first, 'recoverUnfinishedRuns');
  });

  // -------------------------------------------------------------------------
  //  手机空壳会话清理：走本地库，不打远端 DELETE
  // -------------------------------------------------------------------------

  test('手机：清空壳会话删的是**本地库**（远端 DELETE 够不着本地镜像）', () async {
    final remoteCalls = <String>[];
    final source = _FakeLocalSource()
      ..conversations.addAll(const [
        AiConversation(id: 'local-old', title: '新对话'),
        AiConversation(id: 'local-older', title: '新对话'),
      ]);
    final store = AiWorkspaceStore(
      api: AiApiClient(
        'http://127.0.0.1:8080',
        client: MockClient((request) async {
          remoteCalls.add('${request.method} ${request.url.path}');
          return http.Response('offline', 500);
        }),
      ),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    // 列表按更新时间倒序：只留最新那条，其余两条清掉
    expect(store.conversation?.id, 'local-conv-1');
    expect(source.deleted, [
      'local-old',
      'local-older',
    ], reason: '空壳要在**本地库**里删掉');
    expect(
      remoteCalls.where((c) => c.startsWith('DELETE /api/ai/conversations/')),
      isEmpty,
      reason: '手机上那句远端 DELETE 够不着本地镜像，空壳会越攒越多：$remoteCalls',
    );
  });

  test('手机：空壳删完之后，列表与本地库里都不该再有它', () async {
    final source = _FakeLocalSource()
      ..conversations.addAll(const [
        AiConversation(id: 'local-old', title: '新对话'),
      ]);
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(source.deleted, ['local-old']);
    expect(source.conversations.map((c) => c.id), isNot(contains('local-old')));
    expect(store.conversations.map((c) => c.id), isNot(contains('local-old')));
  });

  // -------------------------------------------------------------------------
  //  用户主动删会话（§25.1 第三条的欠账）：本机 + 远端两半，且如实说清哪一半成了
  // -------------------------------------------------------------------------

  /// 用户主动删会话的**本机**那一半必须走 [RunEventSource.deleteConversation]（本地库）。
  ///
  /// 在这之前这一步只打**远端** DELETE：那条请求够不着本地镜像，于是"看起来删了、
  /// 其实还在"，而本地那份还带着一个本机删除标记（`local_deletions`）——
  /// 没有标记的话，桌面那份会在下一次下行同步时把它整行覆盖回来、**会话当场复活**。
  /// 标记的写在 Kotlin 侧（`HarnessStore.deleteConversation`，`SyncApplyTest` 盯着它）；
  /// 这条 Dart 用例钉的是**接线**：本机那一半真的走了本地实现。
  test('手机：用户主动删会话走**本地实现**（远端 DELETE 照旧打，但不假装两边都删了）', () async {
    final remoteCalls = <String>[];
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: AiApiClient(
        'http://127.0.0.1:8080',
        client: MockClient((request) async {
          remoteCalls.add('${request.method} ${request.url.path}');
          // 桌面离线（手机上这是常态）
          return http.Response('offline', 500);
        }),
      ),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();
    remoteCalls.clear();

    await store.deleteConversation('local-conv-1');

    expect(source.deleted, contains('local-conv-1'), reason: '本机那一半要走本地库的删除');
    expect(
      remoteCalls,
      contains('DELETE /api/ai/conversations/local-conv-1'),
      reason: '远端仍然要调：桌面是唯一真源，桌面那份也该删（这不是上行 op，R3 不变）',
    );
    expect(
      store.conversations.map((c) => c.id),
      isNot(contains('local-conv-1')),
    );
    expect(store.conversation?.id, isNot('local-conv-1'));
    expect(
      store.error,
      contains('桌面上那份还在'),
      reason: '桌面没删掉就必须如实说 —— 不许假装"两边都删了"',
    );
  });

  /// 本机那一半失败时：**不许**从列表里拿掉它（否则界面显示的"删掉了"是假的），
  /// 而且错误里要指名道姓说是本机没删掉。
  test('手机：本机没删掉时留在列表里，并如实说是本机那半失败', () async {
    final source = _FakeLocalSource()..failDelete = true;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();
    await store.deleteConversation('local-conv-1');

    expect(
      store.conversations.map((c) => c.id),
      contains('local-conv-1'),
      reason: '本机还在，列表就不许少它',
    );
    expect(store.error, contains('本机没删掉'));
  });

  // -------------------------------------------------------------------------
  //  M4 补全：上行不只在冷启动 / Run 收尾触发 —— 前台定时 + 回到前台
  // -------------------------------------------------------------------------

  testWidgets('前台定时：队列里还有东西才定时重推，推空了立刻停（不空转烧电）', (tester) async {
    mockPairingChannel();
    final source = _FakeLocalSource()
      ..outboxPending = 2
      ..outboxError = '连不上上游：ConnectException';
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();
    final atColdStart = source.calls.where((c) => c == 'syncNow').length;
    expect(store.pushNotice, contains('2 条'), reason: '冷启动推失败要如实记一句');
    expect(store.outboxRetryArmed, isTrue, reason: '还有 2 条没上去 → 该挂着重试');

    // 桌面回来了：下一次定时到点就该把这两条补推上去
    source
      ..outboxPending = 0
      ..outboxError = null
      ..outboxApplied = 2;
    await tester.pump(const Duration(minutes: 2));
    await tester.pump();

    expect(
      source.calls.where((c) => c == 'syncNow').length,
      greaterThan(atColdStart),
      reason: '前台定时这一拍必须真的再推一次（这就是 M4 缺的那种触发）',
    );
    expect(store.pushNotice, contains('已把本机 2 条记录同步到桌面'));
    expect(store.outboxRetryArmed, isFalse, reason: '推空了就要停：不能挂着空转');
  });

  testWidgets('队列是空的：不定时、也不打网络（用户停在聊天页不该被反复唤醒）', (tester) async {
    mockPairingChannel(); // 见 [mockPairingChannel]：fakeAsync 里没人应答的通道会挂死
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();
    final atColdStart = source.calls.where((c) => c == 'syncNow').length;
    expect(store.outboxRetryArmed, isFalse, reason: '一条待办都没有 → 不该挂定时器');

    await tester.pump(const Duration(minutes: 5));

    expect(
      source.calls.where((c) => c == 'syncNow').length,
      atColdStart,
      reason: '空队列绝不能定时轮询：那是白烧电，也是 M4 补全里唯一要防的坏做法',
    );
  });

  // -------------------------------------------------------------------------
  //  M4 的下行半边：拉一次（冷启动 / 回到前台 / 每轮 Run 收尾）
  // -------------------------------------------------------------------------

  /// 桌面实现：**恒** up_to_date —— 后端自己就是真源，没有"从别人那里拉"这回事。
  test('桌面源的下行恒 up_to_date（桌面自己就是真源，一条请求都不打）', () async {
    var calls = 0;
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        calls++;
        return http.Response('{}', 200);
      }),
    );
    final r = await RemoteRunEventSource(api)
        .pullChanges(baseUrl: 'http://10.0.0.5:8081', token: 'tok');

    expect(r.ok, isTrue);
    expect(r.mode, 'up_to_date');
    expect(r.applied, 0);
    expect(calls, 0, reason: '桌面不该为下行打任何请求');
  });

  /// 冷启动：上行推一次、下行也拉一次（M4 的两半并排），并把结果**如实**说出来。
  test('手机冷启动会拉一次下行，并把"同步下来几条"如实说出来', () async {
    final source = _FakeLocalSource()
      ..pullMode = 'changes'
      ..pullApplied = 3;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
      baseUrlProvider: () => 'http://10.0.0.5:8081',
    );

    await store.load();

    expect(source.calls, contains('pullChanges'), reason: '冷启动要拉一次（下行半边）');
    expect(source.pulledBaseUrls.last, 'http://10.0.0.5:8081');
    expect(store.syncedFromDesktop, 3);
    expect(store.pushNotice, contains('已从桌面同步 3 条记录'));
  });

  /// 全量重建与"补几条"是两件事，说法不能一样（用户可能刚被 410 过）。
  test('下行走的是全量时说法不一样（如实说"重建本机镜像"）', () async {
    final source = _FakeLocalSource()
      ..pullMode = 'bootstrap'
      ..pullApplied = 42;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(store.pushNotice, contains('全量重建本机镜像（42 行）'));
  });

  /// 下行失败**绝不拦聊天页**（与上行同一条纪律）：只写提示，页面照常起来。
  test('下行失败不拦聊天页：只记一句提示，游标下次接着拉', () async {
    final source = _FakeLocalSource()
      ..pullOk = false
      ..pullError = '拿不到桌面的同步清单（HTTP 500）：这次不同步，本机游标没动';
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    await store.load();

    expect(store.loaded, isTrue, reason: '拉不到桌面那份不该让整页起不来');
    expect(store.conversation, isNotNull, reason: '本地库里的会话照常可用');
    expect(store.pushNotice, contains('从桌面拉取失败'));
    expect(store.pushNotice, contains('游标没动'));
  });

  testWidgets('回到前台也会拉一次下行（哪怕本机队列是空的）', (tester) async {
    mockPairingChannel();
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();
    final atColdStart = source.calls.where((c) => c == 'pullChanges').length;
    final atColdStartPush = source.calls.where((c) => c == 'syncNow').length;

    // 本地一条待办都没有：推那一路不该动，但**拉**那一路要来一次
    // （桌面那边可能刚改过东西，而本机队列本来就是空的）
    source.pendingOutboxCount = 0;
    await store.onAppResumed();

    expect(
      source.calls.where((c) => c == 'pullChanges').length,
      greaterThan(atColdStart),
      reason: '回到前台要拉一次：上限是每次回前台一次，不是轮询',
    );
    expect(
      source.calls.where((c) => c == 'syncNow').length,
      atColdStartPush,
      reason: '队列空着的时候推那一路仍然一条请求都不打（那条纪律不变）',
    );
  });

  testWidgets('回到前台：推那一路没待办就不打，拉那一路照常来一次（两条纪律各归各）', (tester) async {
    mockPairingChannel(); // 见 [mockPairingChannel]：fakeAsync 里没人应答的通道会挂死
    final source = _FakeLocalSource()
      ..outboxPending = 3
      ..outboxError = '连不上上游：ConnectException';
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();
    final atColdStart = source.calls.where((c) => c == 'syncNow').length;

    // ① 本地队列是空的 → 回前台不该打任何网络
    source.pendingOutboxCount = 0;
    await store.onAppResumed();
    expect(
      source.calls.where((c) => c == 'syncNow').length,
      atColdStart,
      reason: '没有待办时回到前台不该推（先问本地队列，问完就返回）',
    );
    expect(source.calls, contains('pendingOutbox'), reason: '判据必须是本地队列的读数');

    // ② 有 3 条待办 → 回前台补推一次
    source.pendingOutboxCount = 3;
    await store.onAppResumed();
    expect(
      source.calls.where((c) => c == 'syncNow').length,
      greaterThan(atColdStart),
      reason: '这就是"网络刚恢复"那一刻该做的事',
    );
    expect(store.pushNotice, contains('3 条'));

    // 收尾：这一推又失败了（还有 3 条）→ 定时器挂着。`addTearDown` 来不及 ——
    // `testWidgets` 的"不许有 pending timer"断言跑在 tearDown **之前**，
    // 所以要在**用例体里**把它推空、让它自己停。
    source
      ..pendingOutboxCount = 0
      ..outboxPending = 0
      ..outboxError = null
      ..outboxApplied = 3;
    await tester.pump(const Duration(minutes: 1));
    expect(store.outboxRetryArmed, isFalse, reason: '推空了就该停');
  });

  testWidgets('回到前台的钩子真的装上了（投一次窗口生命周期事件即可）', (tester) async {
    mockPairingChannel(); // 见 [mockPairingChannel]：fakeAsync 里没人应答的通道会挂死
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();
    source.pendingOutboxCount = 1;
    final before = source.calls.where((c) => c == 'syncNow').length;

    // 走**真的**那一条路：store 自己注册的 AppLifecycleListener（app.dart 用的是同一个 API）
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();

    expect(
      source.calls.where((c) => c == 'syncNow').length,
      greaterThan(before),
      reason: '没有这个钩子，手机就得等下一次冷启动才会推',
    );
  });

  // -------------------------------------------------------------------------
  //  手机那份清单：**本机库**（`ai_providers` / `ai_models`）是唯一真源
  // -------------------------------------------------------------------------

  testWidgets('手机：本机库有 Provider、但没给它配模型 → 仍进得了聊天页，并如实说原因', (tester) async {
    // 本机库里有 provider，模型目录空（用户还没在手机上为它加过模型）
    mockPairingChannel(providers: [localProviderJson()]);
    final store = AiWorkspaceStore(
      api: offlineApiWith(<String>[]), // 桌面永远离线
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);

    await store.load();

    expect(store.loaded, isTrue, reason: '模型没配过也不该让整页起不来（以前会冒成"连接后端失败"）');
    expect(store.providers.single.id, 'p1', reason: 'provider 清单来自本机库');
    expect(store.models, isEmpty);
    expect(
      store.modelsIssue,
      isNotNull,
      reason: '空目录要说清楚，不能装作"这个 Provider 没模型"',
    );
    expect(store.error, contains('还没为'), reason: '如实说"本机还没给它配过模型清单"');
    expect(store.error, contains('AI 模型与凭据'), reason: '要说清去哪儿配');
    expect(store.error, isNot(contains('连接后端失败')));
    // 发不出去的原因照旧是一句人话（不是异常、不是空白）
    expect(store.sendBlockReason(), contains('Provider 与模型'));
  });

  testWidgets('手机：换到一个本机没配模型的 Provider → 不抛异常，当场说明原因', (tester) async {
    // 本机库里有两条 provider，但只给 p1 配了模型
    mockPairingChannel(
      providers: [
        localProviderJson(id: 'p1', name: '网关一'),
        localProviderJson(id: 'p2', name: '网关二', credentialRef: 'OTHER_KEY'),
      ],
      models: {
        'p1': [localModelJson()],
      },
    );
    final store = AiWorkspaceStore(
      api: offlineApiWith(<String>[]),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: true),
    );
    addTearDown(store.dispose);
    await store.load();
    expect(store.models, isNotEmpty);

    // 换到 p2：本机库里有这条 provider，但**没给它配过模型**
    await store.selectProvider(store.providers.firstWhere((p) => p.id == 'p2'));

    expect(store.models, isEmpty);
    expect(store.modelsIssue, contains('网关二'));
    expect(
      store.error,
      contains('网关二'),
      reason: '换完 Provider 要当场说明，别让用户对着空选择器猜',
    );
  });

  test('手机：那份 SharedPreferences 清单缓存**退役了**，密钥只进密钥库（AIH-012）', () async {
    mockPairingChannel(token: 'tok-phone-1', providers: [localProviderJson()]);
    final creds = _FakeDeviceCredentials(configured: false);
    final store = AiWorkspaceStore(
      api: offlineApiWith(<String>[]),
      source: _FakeLocalSource(),
      credentials: creds,
      baseUrlProvider: () => 'http://10.0.0.5:8081',
      authApiFactory: (base) => InboundAuthApi(
        base,
        client: MockClient(
          (request) async => http.Response(
            jsonEncode({
              'credentials': {'PHONE_KEY': 'sk-secret-a'},
            }),
            200,
            headers: const {'content-type': 'application/json; charset=utf-8'},
          ),
        ),
      ),
    );

    await store.load();
    await store.syncCredentialsFromDesktop(); // 从桌面取一把密钥（值只进密钥库）

    final prefs = await SharedPreferences.getInstance();
    // ★ 清单缓存退役：手机侧清单只有**本机库**那一份（两份会打架，见 `_loadLocalStartup`）
    expect(prefs.getString('comfyhub.ai.cachedProviders'), isNull);
    expect(prefs.getString('comfyhub.ai.cachedModels.p1'), isNull);
    // 密钥只进密钥库，本地任何一份盘上的东西都不许出现它
    expect(creds.saved['PHONE_KEY'], 'sk-secret-a');
    expect(
      '${prefs.getString('comfyhub.ai.lastProviderId')}'.contains(
        'sk-secret-a',
      ),
      isFalse,
    );
    // （"清单里只有能力声明与端点、没有 `credential` 键"这条防线现在由**本机库**承载：
    //   DTO 键集合由 harness 的 `ProviderStoreTest` 盯着；桥也不回 `credential`。）
  });

  // -------------------------------------------------------------------------
  //  M4：上行 —— 手机产生的会话/消息要能到桌面
  // -------------------------------------------------------------------------

  test('桌面：清单与模型仍然走 HTTP；那份 SharedPreferences 缓存已退役（一个键都不写）', () async {
    final calls = <String>[];
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        calls.add('${request.method} ${request.url.path}');
        const json = {'content-type': 'application/json; charset=utf-8'};
        if (request.url.path == '/api/ai/providers') {
          return http.Response(
            jsonEncode([localProviderJson()]),
            200,
            headers: json,
          );
        }
        if (request.url.path == '/api/ai/providers/p1/models') {
          return http.Response(
            jsonEncode([localModelJson()]),
            200,
            headers: json,
          );
        }
        return http.Response('[]', 200, headers: json);
      }),
    );
    // 桌面源：远端事件源 + 远端凭据 + 远端 provider 库（`forceLocal: false` 那条路）
    final store = AiWorkspaceStore(
      api: api,
      source: RemoteRunEventSource(api),
      credentials: RemoteCredentialStore(api),
    );

    await store.load();

    expect(
      calls,
      contains('GET /api/ai/providers'),
      reason: '桌面侧清单照旧来自后端（一个字未改）',
    );
    expect(calls, contains('GET /api/ai/providers/p1/models'));
    expect(store.providers.single.id, 'p1');
    expect(store.models, isNotEmpty, reason: '桌面的模型目录也照旧来自后端');
    final prefs = await SharedPreferences.getInstance();
    expect(
      prefs.getString('comfyhub.ai.cachedProviders'),
      isNull,
      reason:
          '那份缓存是**手机专有**的（旧代码里 `if (!credentials.isLocal) return`），'
          '手机侧改读本机库之后它整体退役了 —— 桌面侧不该凭空多出一份缓存',
    );
    expect(prefs.getString('comfyhub.ai.cachedModels.p1'), isNull);
  });

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
    // 还有 1 条待推 → M4 补全之后这里会挂上重试定时器；用例结束要收掉它
    expect(store.outboxRetryArmed, isTrue);
    store.dispose();
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
    // 顶部那条契约一个字没改：**没有抛**、页面可用，只是记了一句
    expect(store.outboxRetryArmed, isTrue, reason: '离线且有 2 条待办 → 前台会再试（不拦聊天页）');
    store.dispose();
  });

  test('一轮跑完之后也会顺手推一次（那时内容才定稿）', () async {
    final source = _FakeLocalSource()..outboxApplied = 2;
    final store =
        AiWorkspaceStore(
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

  // -------------------------------------------------------------------------
  //  配对令牌这条线（`docs/pairing-design.md` §11）
  //
  //  以前 Dart 侧调 `ensureRunKit` / `syncNow` 时 `token` **恒为 null**：
  //  闸门一开，这两条路就都是 403 —— 而界面上只会说"取不到规则工件"，
  //  看不出是"没带令牌"还是"桌面不在"。所以这里钉住"读到就必须带上去"。
  // -------------------------------------------------------------------------

  test('冷启动读到的配对令牌，必须带在 ensureRunKit / syncNow 上', () async {
    mockPairingChannel(
      token: 'tok-phone-1',
      deviceId: 'phone-1',
      deviceName: 'OnePlus 8T',
    );

    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
      baseUrlProvider: () => 'http://10.0.0.5:8081',
    );

    await store.load();

    expect(
      store.pairingToken,
      'tok-phone-1',
      reason: '令牌在内存里，真源是 Android Keystore',
    );
    expect(source.kitTokens, ['tok-phone-1'], reason: '取规则工件不带令牌 = 闸门一开就 403');
    expect(source.syncedTokens, contains('tok-phone-1'), reason: '上行同步同理');
    expect(source.syncedBaseUrls.last, 'http://10.0.0.5:8081');
  });

  test('没配对（密钥库里没有令牌）时照旧传 null，且不拦启动', () async {
    final source = _FakeLocalSource();
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: source,
      credentials: _FakeDeviceCredentials(configured: true),
    );

    // 这条通道在测试进程里没人应答（抛 MissingPluginException）：必须被吞掉
    await store.load();

    expect(store.loaded, isTrue, reason: '读不到令牌绝不能拦住启动');
    expect(store.pairingToken, isNull);
    expect(source.kitTokens, [null]);
  });

  test('从桌面同步密钥：写进本机密钥库，并如实说"拿到几把、哪几把没拿到"', () async {
    mockPairingChannel(token: 'tok-phone-1');
    final creds = _FakeDeviceCredentials(configured: false);
    late Uri url;

    final store = AiWorkspaceStore(
      api: _okApi(),
      source: _FakeLocalSource(),
      credentials: creds,
      baseUrlProvider: () => 'http://10.0.0.5:8081',
      authApiFactory: (base) => InboundAuthApi(
        base,
        client: MockClient((request) async {
          url = request.url;
          // 桌面只下发被允许的那一把；另一把**不出现也不报错**（§11.2）
          return http.Response(
            jsonEncode({
              'credentials': {'A_KEY': 'sk-secret-a'},
            }),
            200,
            headers: const {'content-type': 'application/json; charset=utf-8'},
          );
        }),
      ),
    );
    await store.load();
    // 清单里两条 provider，各自一把密钥（ref 是它俩的 credentialRef）
    store.providers = [
      _providerWithRef('p1', 'A_KEY'),
      _providerWithRef('p2', 'B_KEY'),
    ];

    final text = await store.syncCredentialsFromDesktop();

    expect(creds.saved['A_KEY'], 'sk-secret-a', reason: '值要写进本机密钥库（手机才发得出去）');
    expect(creds.saved.containsKey('B_KEY'), isFalse, reason: '桌面没给的那把不能凭空编一个');
    expect(url.queryParameters['refs'], 'A_KEY,B_KEY');
    expect(url.path, '/api/ai/credentials');
    expect(text, contains('A_KEY'));
    expect(text, contains('B_KEY'), reason: '"哪几把没拿到"必须说出来，否则用户以为都拿到了');
    // 密钥值**绝不能**进返回句 / 日志 / 界面状态（AIH-012）
    expect(text, isNot(contains('sk-secret-a')));
    expect(
      store.providers.first.credential.source,
      'keystore',
      reason: '取完要把"已配置"对齐到本机',
    );
  });

  test('从桌面同步密钥：没配对 / 非手机时不打请求，只回一句人话', () async {
    var requested = false;
    final store = AiWorkspaceStore(
      api: _okApi(),
      source: _FakeLocalSource(),
      credentials: _FakeDeviceCredentials(configured: false),
      baseUrlProvider: () => 'http://10.0.0.5:8081',
      authApiFactory: (base) => InboundAuthApi(
        base,
        client: MockClient((request) async {
          requested = true;
          return http.Response('{}', 200);
        }),
      ),
    );
    await store.load();
    store.providers = [_providerWithRef('p1', 'A_KEY')];

    // 本机密钥库里没有配对令牌 → 不去问桌面（问也是 403，白白制造一次失败）
    final noToken = await store.syncCredentialsFromDesktop();
    expect(noToken, contains('配对令牌'));
    expect(requested, isFalse);

    // 桌面自己就是密钥的真源：这条路径在桌面上没有意义
    final desktop = AiWorkspaceStore(
      api: _okApi(),
      source: RemoteRunEventSource(AiApiClient('http://127.0.0.1:8080')),
      credentials: _FakeDeviceCredentials(configured: false),
      baseUrlProvider: () => 'http://127.0.0.1:8080',
    );
    desktop.providers = [_providerWithRef('p1', 'A_KEY')];
    final onDesktop = await desktop.syncCredentialsFromDesktop();
    expect(onDesktop, contains('不需要'));
    expect(requested, isFalse);
  });
}

/// 把配对令牌 / 设备身份灌进假的本机密钥库（真身是 Android Keystore，走桥）。
///
/// ⚠ **`testWidgets` 里的用例必须先调它（哪怕不要令牌）**：`testWidgets` 跑在
/// `fakeAsync` 区里，而"没人应答的 method channel"要**经过引擎回包**（真实事件循环）——
/// 那一步永远不会被 fakeAsync 的时钟推进，于是 `await store.load()`（内部要读配对令牌）
/// 会**当场挂死**：整条 `flutter test` 卡住、不报错、也不超时。
/// 装了应答者之后，回包走的是纯 Dart 那条路，正常。
/// 实测踩过：`00:00 +12: 前台定时…` 之后就再没有任何输出。
void mockPairingChannel({
  String? token,
  String deviceId = 'phone-1',
  String deviceName = 'OnePlus 8T',

  /// **本机库那份清单**（手机侧 provider 的唯一真源，`ProviderStore` → 桥）。
  ///
  /// 默认空 = "这台手机上还没配过"（就是用户第一次打开 App 时的样子）。
  /// ⚠ 桥回的 DTO 里**没有** `credential` 键：那是"这台机器有没有 key"的即时状态，
  /// 真源是本机密钥库（界面用 `CredentialStore.statusOf` 现问）。
  List<Map<String, dynamic>> providers = const [],
  Map<String, List<Map<String, dynamic>>> models = const {},
}) {
  const channel = MethodChannel('comfyhub/harness');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(channel, (call) async {
    final args = Map<String, dynamic>.from(
      (call.arguments as Map?) ?? const {},
    );
    switch (call.method) {
      case 'getCredential':
        return args['ref'] == PairingTokenStore.ref ? token : null;
      case 'pairingIdentity':
        return jsonEncode({'deviceId': deviceId, 'deviceName': deviceName});
      // --- 本机库那份清单（ProviderStore 的五个方法）---
      case 'listProviders':
        return jsonEncode(providers);
      case 'listModels':
        return jsonEncode(models[args['providerId']?.toString()] ?? const []);
      case 'saveProvider':
        return args['provider']; // 回执 = 保存后的那一条（桥就是回显落库那一行）
      case 'saveModels':
        return args['models'];
      case 'deleteProvider':
        return true;
    }
    return null;
  });
  addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
}

/// 本机库里的一条 provider（桥回的 DTO 形状；**不含 `credential`**）。
Map<String, dynamic> localProviderJson({
  String id = 'p1',
  String name = '公司网关',
  String credentialRef = 'PHONE_KEY',
}) => {
  'id': id,
  'displayName': name,
  'api': 'openai-completions',
  'baseURL': 'https://api.example.com/v1',
  'credentialRef': credentialRef,
  'endpointTrust': 'public',
  'enabled': true,
  'revision': 1,
  'createdAt': '2026-09-27T00:00:00.000Z',
  'updatedAt': '2026-09-27T00:00:00.000Z',
};

/// 本机库里的一个模型（能力声明是"发得出去"的前提）。
Map<String, dynamic> localModelJson({
  String id = 'm-text',
  String name = '纯文本模型',
}) => {
  'providerId': 'p1',
  'id': id,
  'displayName': name,
  'inputModalities': ['text'],
  'tools': true,
  'capabilitySource': 'manual',
  'enabled': true,
};

/// 一个**桌面永远离线**的客户端 + 命中记录：用来证明"手机侧清单一条网络请求都不打"。
AiApiClient offlineApiWith(List<String> hits) => AiApiClient(
  'http://127.0.0.1:8080',
  client: MockClient((request) async {
    hits.add('${request.method} ${request.url.path}');
    return http.Response('offline', 500);
  }),
);

AiProvider _providerWithRef(String id, String ref) => AiProvider(
  id: id,
  displayName: id,
  api: 'openai-completions',
  baseURL: 'http://127.0.0.1:11434/v1',
  credentialRef: ref,
  endpointTrust: 'loopback',
  enabled: true,
  revision: 1,
  credential: const AiCredentialStatus(
    configured: false,
    source: 'none',
    writable: true,
  ),
);

/// 桌面在线：provider 清单 + 模型目录都正常返回（用来灌"本机缓存"）。
///
/// [modelsOk] = false 时模型目录那一路 500 —— 用来造"provider 清单缓存到了、
/// 模型目录没缓存到"这种真实存在的状态（M6 剩余那一半）。
///
/// ⚠ 响应必须带 `charset=utf-8`：`http.Response` 默认按 **Latin-1** 编码 body，
/// 中文显示名会在 `http` 层就抛 "Contains invalid characters"（看起来像后端挂了）。
AiApiClient _okApi({bool modelsOk = true}) => AiApiClient(
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
            'credential': {
              'configured': true,
              'source': 'managed',
              'writable': true,
            },
          },
        ]),
        200,
        headers: json,
      );
    }
    if (request.url.path == '/api/ai/providers/p1/models') {
      if (!modelsOk) return http.Response('模型目录这次挂了', 500, headers: json);
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
          },
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

  /// 写进"密钥库"的值（ref → 值）。真身是 Android Keystore，只写不读。
  final Map<String, String> saved = {};

  @override
  bool get isLocal => true;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async {
    final ref = provider.credentialRef?.trim();
    if (ref == null || ref.isEmpty) {
      return const AiCredentialStatus(
        configured: false,
        source: 'none',
        writable: false,
      );
    }
    return AiCredentialStatus(
      configured: configured,
      source: configured ? 'keystore' : 'none',
      writable: true,
    );
  }

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) async {
    saved[provider.credentialRef?.trim() ?? ''] = value;
    configured = true;
    return statusOf(provider);
  }

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) async {
    configured = false;
    return statusOf(provider);
  }

  /// 假的"取出来"（真身是桥的 `getCredential`）：**只有「推给桌面」那条路会用它**。
  ///
  /// 它回 `saved` 里那把（没存过就回 null —— 调用方要如实说"没有可推的"，
  /// 而不是拿空串把桌面上那把好的覆盖掉）。
  @override
  Future<String?> reveal(AiProvider provider) async =>
      saved[provider.credentialRef?.trim() ?? ''];
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
