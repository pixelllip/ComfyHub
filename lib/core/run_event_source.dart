import 'dart:async';
import 'dart:convert';
import 'dart:io' show Platform;

import 'package:flutter/services.dart';

import '../models/ai_models.dart';
import 'ai_api_client.dart';

/// 一次 Run 的发起参数（与桌面 `POST /api/ai/conversations/{id}/runs` 的字段对齐）。
class RunStartRequest {
  const RunStartRequest({
    required this.conversationId,
    required this.text,
    this.providerId,
    this.modelId,
    this.api,
    this.baseUrl,
    this.credentialRef,
    this.reasoningEffort,
    this.maxSteps,
    this.userMessageId,
    this.assistantMessageId,
    this.attachmentIds = const [],
    this.retryOfRunId,
    this.displayText,
    this.mentions = const [],
  });

  final String conversationId;
  final String text;
  final String? providerId;
  final String? modelId;

  /// 手机本地 harness 专用：协议 / 端点 / 凭据引用（桌面那侧由后端自己查库）。
  final String? api;
  final String? baseUrl;
  final String? credentialRef;
  final String? reasoningEffort;
  final int? maxSteps;

  /// 这次要发给模型的附件（M3）。**准入判定在创建 Run 之前完成**：
  /// 远端不通过会直接 400 且不产生任何上游请求（AIH-030）。
  final List<String> attachmentIds;

  /// 这次 Run 是重放哪一次失败的（AIH-024），远端用它把两次 Run 关联起来。
  final String? retryOfRunId;

  /// 用户**看到的**正文（卡片块已剥掉）；缺省 = [text]。
  ///
  /// `text` 仍是给模型的完整正文（含卡正文），界面按 [displayText] +
  /// [mentions] 画小卡片。
  final String? displayText;

  /// `@` 卡片 / `/技能` 的发送时快照（后端落成用户消息的 `mention` 块）。
  final List<OutgoingMention> mentions;

  /// 调用方**自己生成**的会话 / 消息主键（M3.2）。
  ///
  /// 为什么要由界面给：界面在"发出请求"的同一刻就乐观插好一条用户气泡 + 一条空的助手
  /// 流式气泡，随后靠事件里的 `messageId` 找到那条气泡往上写字。若本地 harness 自己另起
  /// UUID，界面手里那个 id 就永远对不上库里那条 —— 表现是**事件一条不少、界面上却一个
  /// 字都不长**，而且两端都不报错，极难查。
  ///
  /// 远端（桌面）后端自己生成这两个 id 并**回传**，所以走远端时这几个字段留 null。
  final String? userMessageId;
  final String? assistantMessageId;
}

/// 一次 Run 的发起结果。
class RunHandles {
  const RunHandles({
    required this.runId,
    required this.conversationId,
    this.userMessageId,
    this.assistantMessageId,
    this.kitVersion,
    this.promptVersion,
  });

  final String runId;
  final String conversationId;

  /// 用户消息 / 助手消息的主键。
  ///
  /// 远端：后端生成并回传；本地 harness：**就是请求里传进去的那一对**（回显）。
  /// 界面用它认自己乐观插的那两条气泡，也用它把"助手消息 → Run"记下来供重试用。
  final String? userMessageId;
  final String? assistantMessageId;
  final String? kitVersion;
  final String? promptVersion;
}

/// **Run 事件流的来源抽象**（`docs/implementation-master-plan.md` §5.3）。
///
/// 桌面与手机的事件**生产者不同、协议相同**：
///
/// | | 生产者 | 传输 |
/// | --- | --- | --- |
/// | 桌面 | 后端 `RunEventBus` | 远端 SSE |
/// | 手机 | 本地 Kotlin harness | platform channel（`EventChannel`） |
///
/// 有了这个抽象，`ai_workspace_store.dart` 那套渲染逻辑**两端共用、零分叉** ——
/// 这正是 `docs/run-event-contract.md` 那份契约存在的全部理由。
abstract class RunEventSource {
  /// 本地 harness（手机）还是远端后端（桌面）。
  bool get isLocal;

  Future<RunHandles> startRun(RunStartRequest request);

  Future<void> cancelRun(String runId);

  /// 落一条新会话。
  ///
  /// **它必须在这个抽象里**（M3.2 ②）：只把"事件流"换个来源是不够的 ——
  /// 离线时事件有了、会话却查不到，界面照样一个字都长不出来，而两端都不报错。
  /// 所以"会话 / 消息的持久化"与"事件流"**走同一个来源**：
  /// 手机落本地库（Kotlin `HarnessStore`），桌面走 HTTP。
  ///
  /// [conversationId] 只有本地实现认（界面自己生成、要拿同一个 id 认自己插的那条）；
  /// 远端由后端生成并回传，传进来也忽略。
  Future<AiConversation> createConversation({
    String? conversationId,
    String? title,
    String? providerId,
    String? modelId,
  });

  /// 拉一条会话的消息（**含有序块 `parts`** —— 工具卡 / 思考段全靠它）。
  Future<List<AiMessage>> listMessages(String conversationId);

  /// 删掉一条会话（删的是**哪个库**由实现决定）。
  ///
  /// 为什么它必须在这个抽象里（M3.2 ③ 留下的坑）：界面切会话时会顺手清掉"上一条空壳"
  /// （AIH-018），而手机上那一步原先打的是**远端** `DELETE /api/ai/conversations/{id}` ——
  /// 可手机上会话的真源是**本地库**，那条请求既够不着本地镜像（空壳越攒越多），
  /// 离线时还每次白报一个失败。所以清理必须与持久化走**同一个来源**：
  /// 手机删本地库，桌面删后端库。
  ///
  /// ⚠ 手机上**不会**因此向上行通道写一个 `delete`（R3：上行只有 `upsert`，
  /// 手机不许伪造删除）。没推上去的，桌面根本不知道它存在过；已经推上去的，
  /// 桌面那份留着 —— 这里不假装"两边都删了"。
  ///
  /// ## ★ 手机上它还会留一个**本机删除标记**（2026-09-27）
  ///
  /// 只删本地行是不够的：桌面那份还在，下一次下行 apply 会把它整行覆盖回来、
  /// **会话当场复活**（两端都不报错）。所以本地实现（Kotlin
  /// `HarnessStore.deleteConversation`）在同一个事务里写一行 `local_deletions`，
  /// 下行覆盖时按它跳过这一个 id（`SyncApply` / `HarnessStore.applyMirrorRow`）。
  /// **它不是 wire 上的 delete**：标记不出本机、不进同步范围、不挂触发器。
  ///
  /// 用户主动删会话时，除了这里改走本地实现，界面**仍然会打一次远端 DELETE**
  /// （桌面是唯一真源，桌面那份也该删）—— 那一句在 `AiWorkspaceStore.deleteConversation`
  /// 里，不是这个抽象的一部分：本方法只负责"本端这一份"。
  ///
  /// 幂等：删一个不存在的 id **不算失败**（重复清理是常态）。
  Future<void> deleteConversation(String conversationId);

  /// 本机**还有几条没推到桌面**（手机专有；桌面恒 0）。
  ///
  /// 它是"要不要再推一次"的判据（见 `ai_workspace_store` 的前台定时与回到前台触发）：
  /// 问的是**本地库**，一条网络请求都不打 —— 所以"队列空的时候别空转烧电"这件事
  /// 才做得到。
  Future<int> pendingOutbox();

  /// 会话列表（`_consumeRun` 收尾的"与服务端对齐"要用它）。
  Future<List<AiConversation>> listConversations();

  /// 事件流（`after` 用于断线续传，语义与 SSE 的 `?after=<seq>` 一致）。
  Stream<AiRunEvent> events(String runId, {int after = 0});

  /// 确保**规则工件**（run-kit）可用（M3，🔒 `docs/run-kit-contract.md` §3.4）。
  ///
  /// 手机：先向桌面取一次（顺便刷新缓存），取不到就用缓存；**一份都没有 → `ok:false`**，
  /// 界面必须据此**拦住发送** —— 系统提示 / 工具描述 / 安全横幅只有这一份来源（R13），
  /// 没有任何规则时"先跑起来再说"就是把一份瞎编的规则交给模型。
  ///
  /// 桌面：不需要（规则在后端手上），恒回 `ok:true`。
  ///
  /// ⚠ 这个方法是 M3.2 里漏掉的一环：桥的 `startRun` 没有 kit 时会回 `NO_RUN_KIT`，
  /// 而在此之前**没有任何一处调用过它** —— 也就是说手机上第一条消息必然失败，
  /// 报错还是 `NO_RUN_KIT` 这种机器码。
  Future<({bool ok, String? kitVersion, String? reason})> ensureRunKit({
    String? baseUrl,
    String? token,
  });

  /// 把**上次没跑完的 Run** 老实收尾（返回收尾了几条）。
  ///
  /// 手机上前台服务有 6h/24h 配额、后台随时被系统杀（总纲 §7 风险表），所以进程重启后
  /// 必须能认出"有一个 Run 没跑完"，而不是留一个永远 `running` 的幽灵；
  /// 收尾只改状态与错误说明，**已经落库的正文与块一个字不动**（R6）。
  ///
  /// 桌面：后端进程自己管这件事，恒回 0（手机冷启动时调它，不会造成任何远端请求）。
  Future<int> recoverUnfinishedRuns();

  /// 把本机 `_outbox` 里攒的东西推到桌面（**M4**，契约 🔒 §4）。
  ///
  /// 手机上产生的会话 / 消息 / 块会落在本地库并进队列，桌面在线时推上去才"桌面可见可续写"。
  /// 只推**一批**；`pending` 是推完之后还剩几条（想推完就再调一次）。
  ///
  /// ⚠ `ok=false` 时**不许**在界面上说"已同步"：那会让用户以为桌面上已经有了。
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  });

  /// 从桌面**拉一次**（**M4 的下行半边**，契约 🔒 §1~§3）。
  ///
  /// 与 [syncNow] 是一对：上行推本机产生的，下行把桌面那份投影到本地镜像。
  ///
  /// 手机实现走本地 harness（`SyncPull.pullOnce`）：握手 `manifest` → 首次 /
  /// `schemaVersion` 变了 / 收到 **410** 时走 `bootstrap`，其余走 `changes` 增量；
  /// **游标只在整趟成功之后才推进**，失败时一个字都不动（下次还能接着拉）。
  /// `mode` 是这次实际走的那条路：`bootstrap` / `changes` / `up_to_date`。
  ///
  /// 桌面实现**恒** `up_to_date` / 0 条：后端自己就是真源，没有"从别人那里拉"这回事。
  ///
  /// ⚠ `ok=false` 时别拿 `mode` 当结论（判据永远是 `ok`）；失败**不许**拦聊天页 ——
  /// 桌面不在线是手机的常态（与 [syncNow] 同一条纪律）。
  Future<({bool ok, String mode, int applied, int cursor, String? error})>
  pullChanges({String? baseUrl, String? token});

  /// 回答 `user.question`（提问闸门；桌面是 HTTP，手机是本地闸门）。
  ///
  /// [answer] 与 [optionIndex] **至少给一个**，另一个留 null：点了选项就用 `optionIndex`，
  /// 手打文字就用 `answer`。⚠ 别把 null 写成空串 —— 那会让"点选项"的请求里多出一个
  /// `answer: ""`，后端会当成用户答了一段空文字（选项那半边就白点了）。
  ///
  /// 返回**是否真的被采纳**：`false` = 这条提问已经结束（超时 / Run 停了），
  /// 界面要如实说出来，不能假装收到。
  Future<bool> answerQuestion(
    String questionId,
    String? answer, {
    int? optionIndex,
  });
}

/// 桌面实现：转发给 `AiApiClient`（远端 SSE）。
class RemoteRunEventSource implements RunEventSource {
  RemoteRunEventSource(this._api);

  final AiApiClient _api;

  @override
  bool get isLocal => false;

  @override
  Future<RunHandles> startRun(RunStartRequest request) async {
    final run = await _api.startRun(
      request.conversationId,
      text: request.text,
      providerId: request.providerId ?? '',
      modelId: request.modelId ?? '',
      reasoningEffort: request.reasoningEffort,
      attachmentIds: request.attachmentIds,
      retryOfRunId: request.retryOfRunId,
      displayText: request.displayText,
      mentions: request.mentions,
    );
    // 远端这两个 id 由后端生成：**回传什么就用什么**（不要拿请求里那份，远端才是真源）
    return RunHandles(
      runId: run.runId,
      conversationId: request.conversationId,
      userMessageId: run.userMessageId,
      assistantMessageId: run.assistantMessageId,
    );
  }

  @override
  Future<AiConversation> createConversation({
    String? conversationId,
    String? title,
    String? providerId,
    String? modelId,
  }) =>
      // 远端由后端生成主键：请求里那份（如果有）不是真源，**回传什么就用什么**
      _api.createConversation(
        title: title,
        providerId: providerId,
        modelId: modelId,
      );

  @override
  Future<List<AiMessage>> listMessages(String conversationId) =>
      _api.listMessages(conversationId);

  /// 桌面：删的是**后端库**里那一条（`DELETE /api/ai/conversations/{id}`）。
  @override
  Future<void> deleteConversation(String conversationId) =>
      _api.deleteConversation(conversationId);

  /// 桌面：上行队列在**手机**手上（`_outbox` 是手机侧的东西），这里恒 0。
  @override
  Future<int> pendingOutbox() async => 0;

  @override
  Future<List<AiConversation>> listConversations() => _api.listConversations();

  @override
  Future<void> cancelRun(String runId) => _api.cancelRun(runId);

  @override
  Stream<AiRunEvent> events(String runId, {int after = 0}) =>
      _api.runEvents(runId, after: after);

  @override
  Future<bool> answerQuestion(
    String questionId,
    String? answer, {
    int? optionIndex,
  }) =>
      _api.answerQuestion(questionId, answer: answer, optionIndex: optionIndex);

  /// 桌面不需要本地规则工件：系统提示 / 工具清单本来就在后端手里
  /// （每一次 Run 由后端现拼），所以这里恒 true。
  @override
  Future<({bool ok, String? kitVersion, String? reason})> ensureRunKit({
    String? baseUrl,
    String? token,
  }) async => (ok: true, kitVersion: null, reason: null);

  /// 桌面：Run 的收尾归后端进程自己管，手机冷启动不该因此打任何请求。
  @override
  Future<int> recoverUnfinishedRuns() async => 0;

  /// 桌面：上行队列在**手机**手上（`_outbox` 是手机侧的东西），这里没什么可推的。
  @override
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  }) async => (ok: true, applied: 0, pending: 0, error: null);

  /// 桌面：**恒 `up_to_date`** —— 后端自己就是真源，没有"从别人那里拉"这回事。
  ///
  /// （手机上这条路会把 `manifest` / `changes` / `bootstrap` 走一遍，见本地实现。）
  @override
  Future<({bool ok, String mode, int applied, int cursor, String? error})>
  pullChanges({String? baseUrl, String? token}) async =>
      (ok: true, mode: 'up_to_date', applied: 0, cursor: 0, error: null);
}

/// 手机实现：`MethodChannel`（调用）+ `EventChannel`（事件流）。
///
/// ⚠ **规则类输入一条都不在这里**：系统提示、工具描述、安全横幅全部由桌面的 run-kit 下发
/// （红线 R13）。这个类只负责"搬"，不负责"定"。
class LocalHarnessRunEventSource implements RunEventSource {
  LocalHarnessRunEventSource({
    MethodChannel? methodChannel,
    EventChannel? eventChannel,
    this.onEventChannelError,
  }) : _method = methodChannel ?? const MethodChannel(methodChannelName),
       _events = eventChannel ?? const EventChannel(eventChannelName);

  /// 与 Kotlin 侧 `HarnessBridge.METHOD_CHANNEL` 同名（改了要同时改两边）。
  static const String methodChannelName = 'comfyhub/harness';

  /// 与 Kotlin 侧 `HarnessBridge.EVENT_CHANNEL` 同名。
  static const String eventChannelName = 'comfyhub/harness/events';

  final MethodChannel _method;
  final EventChannel _events;
  final void Function(Object error, StackTrace stack)? onEventChannelError;

  @override
  bool get isLocal => true;

  /// 本地库的 schema 版本 / 已实现的工具名 / 已缓存的 kit 版本。
  Future<Map<String, dynamic>> capabilities() async =>
      _map(await _method.invokeMethod<String>('capabilities') ?? '{}');

  /// 取一次 run-kit 并缓存（`{ok, kitVersion, kit, reason?, warning?}`）。
  ///
  /// `ok=false` 时**不许开跑**：那说明手里没有可用的规则工件。
  Future<Map<String, dynamic>> fetchRunKit({
    String? baseUrl,
    String? token,
    String? providerId,
    String? modelId,
  }) async => _map(
    await _method.invokeMethod<String>('fetchRunKit', {
          'baseUrl': ?baseUrl,
          'token': ?token,
          'providerId': ?providerId,
          'modelId': ?modelId,
        }) ??
        '{}',
  );

  /// 桌面离线时用缓存的那一份（拿不到返回 null）。
  Future<Map<String, dynamic>?> cachedRunKit() async {
    final raw = await _method.invokeMethod<String>('cachedRunKit');
    if (raw == null || raw.isEmpty) return null;
    return _map(raw);
  }

  /// 取一次（拿不到就用缓存）；**一份都没有就是 `ok:false`**，界面据此拦住发送。
  ///
  /// 只看 `ok`：`reason` / `error` 是给人看的话（桥那边已经分好了
  /// "本地还没有规则工件"与"这次请求为什么失败"）。
  @override
  Future<({bool ok, String? kitVersion, String? reason})> ensureRunKit({
    String? baseUrl,
    String? token,
  }) async {
    final r = await fetchRunKit(baseUrl: baseUrl, token: token);
    if (r['ok'] == true) {
      final version = r['kitVersion']?.toString();
      return (ok: true, kitVersion: version, reason: r['warning']?.toString());
    }
    // `reason` 优先：那是"缺规则"这件事本身；`error` 才是我这次为什么没取到
    final reason = (r['reason'] ?? r['error'] ?? '拿不到规则工件（run-kit）').toString();
    return (ok: false, kitVersion: null, reason: reason);
  }

  /// 把上次没跑完的 Run 按 R6 收尾，返回收尾了几条。
  Future<int> recoverUnfinished() async =>
      await _method.invokeMethod<int>('recoverUnfinished') ?? 0;

  @override
  Future<int> recoverUnfinishedRuns() => recoverUnfinished();

  /// 推一批本机产生的会话 / 消息 / 块到桌面（M4）。桥那边已经分好了"永久 / 暂时"。
  @override
  Future<({bool ok, int applied, int pending, String? error})> syncNow({
    String? baseUrl,
    String? token,
  }) async {
    final raw = await _method.invokeMethod<String>('syncNow', {
      'baseUrl': ?baseUrl,
      'token': ?token,
    });
    final map = _map(raw ?? '{}');
    return (
      ok: map['ok'] == true,
      applied: (map['applied'] as num?)?.toInt() ?? 0,
      pending: (map['pending'] as num?)?.toInt() ?? 0,
      // `reason`（还不知道桌面地址）与 `error`（这次为什么没推成）都要能显示出来
      error: (map['error'] ?? map['reason'])?.toString(),
    );
  }

  /// 本地会话列表（`_consumeRun` 收尾对齐时用；桥回的是 JSON 字符串）。
  @override
  Future<List<AiConversation>> listConversations() async =>
      (await listConversationRows()).map(AiConversation.fromJson).toList();

  Future<List<Map<String, dynamic>>> listConversationRows() async =>
      _list(await _method.invokeMethod<String>('listConversations') ?? '[]');

  /// 在**本地库**建一条会话（M3.2 ②：离线也要能开会话）。
  ///
  /// [conversationId] 由界面生成并传下去（理由同 `startRun` 的那一对消息 id：
  /// 界面要拿同一个 id 去认自己插的那条会话）；不传则由 Kotlin 侧生成并**回传**
  /// （两种都支持：真源永远是**返回的那条记录**，不是请求里那份）。
  ///
  /// ⚠ 本地建的会话会**进 `_outbox`**（`HarnessStore.createConversation` 默认 `queue=true`），
  /// 等桌面在线时由 `/api/sync/push` 上行 —— 这正是 M4 要的那种"手机产生的会话在桌面可见"。
  @override
  Future<AiConversation> createConversation({
    String? conversationId,
    String? title,
    String? providerId,
    String? modelId,
  }) async {
    final raw = await _method.invokeMethod<String>('createConversation', {
      'conversationId': ?conversationId,
      'title': title ?? '新对话',
      'providerId': ?providerId,
      'modelId': ?modelId,
    });
    return AiConversation.fromJson(_map(raw ?? '{}'));
  }

  /// 消息行（不含 `parts`）。
  Future<List<Map<String, dynamic>>> listMessageRows(
    String conversationId,
  ) async => _list(
    await _method.invokeMethod<String>('listMessages', {
          'conversationId': conversationId,
        }) ??
        '[]',
  );

  /// 一条消息的有序块。
  Future<List<Map<String, dynamic>>> listParts(String messageId) async => _list(
    await _method.invokeMethod<String>('listParts', {'messageId': messageId}) ??
        '[]',
  );

  /// 拉一条会话的消息，**把 `parts` 并进来**。
  ///
  /// ⚠ 这里显式做一次合并：桥的 `listMessages` 只回消息行（Kotlin 侧的消息表与块表是两张表），
  /// 要是就这么交给 `AiMessage.fromJson`，每条消息的 `parts` 都是空的 —— 表现是
  /// **工具卡、思考段、产物入口卡全不见**（正文还在，所以看起来"只是少了点东西"）。
  ///
  /// 为什么不改成"桥一次把 parts 带上"：那要动 Kotlin 侧协议；这里先按**逐条补齐**做，
  /// 键名与桌面 `GET /api/ai/conversations/{id}/messages` 逐字一致。
  @override
  Future<List<AiMessage>> listMessages(String conversationId) async {
    final rows = await listMessageRows(conversationId);
    final out = <AiMessage>[];
    for (final row in rows) {
      final id = row['id']?.toString() ?? '';
      final parts = id.isEmpty
          ? const <Map<String, dynamic>>[]
          : await listParts(id);
      out.add(AiMessage.fromJson({...row, 'parts': parts}));
    }
    return out;
  }

  /// 删掉**本地库**里这一条会话（M3.2 ③ 的第二块砖）。
  ///
  /// 桥回的是"本机是否真的删掉了一行"：`false` = 本来就没有 —— 与桌面 `DELETE` 的语义一致
  /// （幂等），所以这里**不把它当失败**。真正该报错的情况桥会回 `HARNESS_ERROR`
  /// （`HarnessStore.deleteConversation` 对库错误一律抛），那才变成异常 ——
  /// 静默成功会让界面以为删掉了、空壳其实还在。
  @override
  Future<void> deleteConversation(String conversationId) =>
      _method.invokeMethod<bool>('deleteConversation', {
        'conversationId': conversationId,
      });

  /// 本机 `_outbox` 里还剩几条（**只读本地库**，不出网）。
  @override
  Future<int> pendingOutbox() async =>
      await _method.invokeMethod<int>('pendingOutbox') ?? 0;

  /// 从桌面拉一次（M4 的下行半边，契约 §1~§3）。
  ///
  /// 桥那边已经把"能不能拉成"分好了：`ok` 是唯一判据，`mode` 只在 `ok=true` 时是结论
  /// （`ok=false` 时它只是"本来打算走哪条"）。`pending` 那套与上行无关，这里没有。
  @override
  Future<({bool ok, String mode, int applied, int cursor, String? error})>
  pullChanges({String? baseUrl, String? token}) async {
    final raw = await _method.invokeMethod<String>('syncPull', {
      'baseUrl': ?baseUrl,
      'token': ?token,
    });
    final map = _map(raw ?? '{}');
    return (
      ok: map['ok'] == true,
      mode: (map['mode'] ?? 'up_to_date').toString(),
      applied: (map['applied'] as num?)?.toInt() ?? 0,
      cursor: (map['cursor'] as num?)?.toInt() ?? 0,
      error: map['error']?.toString(),
    );
  }

  Future<List<String>> toolNames() async =>
      (await _method.invokeMethod<List<Object?>>('toolNames') ?? const [])
          .map((e) => e.toString())
          .toList();

  Future<bool> hasCredential(String ref) async =>
      await _method.invokeMethod<bool>('hasCredential', {'ref': ref}) ?? false;

  Future<void> putCredential(String ref, String value) =>
      _method.invokeMethod<bool>('putCredential', {'ref': ref, 'value': value});

  Future<void> removeCredential(String ref) =>
      _method.invokeMethod<bool>('removeCredential', {'ref': ref});

  Future<List<String>> credentialRefs() async =>
      (await _method.invokeMethod<List<Object?>>('credentialRefs') ?? const [])
          .map((e) => e.toString())
          .toList();

  @override
  Future<RunHandles> startRun(RunStartRequest request) async {
    final raw = await _method.invokeMethod<String>('startRun', {
      'conversationId': request.conversationId,
      'text': request.text,
      'providerId': ?request.providerId,
      'modelId': ?request.modelId,
      'api': ?request.api,
      'baseUrl': ?request.baseUrl,
      'credentialRef': ?request.credentialRef,
      'reasoningEffort': ?request.reasoningEffort,
      'maxSteps': ?request.maxSteps,
      // 界面生成的一对 id 传下去：harness 按它们落库，**回显**回来给界面认气泡
      'userMessageId': ?request.userMessageId,
      'assistantMessageId': ?request.assistantMessageId,
    });
    final map = _map(raw ?? '{}');
    return RunHandles(
      runId: (map['runId'] ?? '').toString(),
      conversationId: (map['conversationId'] ?? request.conversationId)
          .toString(),
      // harness 回显的就是库里那条；拿不到才回落到本地那份（两者应当相等，
      // 不相等说明桥没把 id 透下去 —— 那时界面会画不出字，所以别在这里悄悄兜住）
      userMessageId: (map['userMessageId'] ?? request.userMessageId)
          ?.toString(),
      assistantMessageId:
          (map['assistantMessageId'] ?? request.assistantMessageId)?.toString(),
      kitVersion: map['kitVersion']?.toString(),
      promptVersion: map['promptVersion']?.toString(),
    );
  }

  @override
  Future<void> cancelRun(String runId) =>
      _method.invokeMethod<bool>('cancelRun', {'runId': runId});

  @override
  Future<bool> answerQuestion(
    String questionId,
    String? answer, {
    int? optionIndex,
  }) async =>
      await _method.invokeMethod<bool>('answerQuestion', {
        'questionId': questionId,
        // Kotlin 侧 `argument<String>("answer").orEmpty()`：null 与空串等价，这里如实传
        'answer': ?answer,
        'optionIndex': ?optionIndex,
      }) ??
      false;

  /// 订阅本地事件流。
  ///
  /// 通道送的是 `{seq, type, payloadJson}`（payload 用**字符串**过线：嵌套 Map 的
  /// 类型映射在两端各有一套规则，字符串最不容易出意外）。
  @override
  Stream<AiRunEvent> events(String runId, {int after = 0}) {
    final raw = _events.receiveBroadcastStream({
      'runId': runId,
      'after': after,
    });
    return raw
        .map((dynamic event) {
          final map = (event as Map).cast<Object?, Object?>();
          final type = (map['type'] ?? 'message').toString();
          final seq = (map['seq'] as num?)?.toInt() ?? 0;
          final payloadRaw = map['payloadJson']?.toString() ?? '{}';
          final data = _map(payloadRaw);
          // seq 也放进 payload：`after` 续传与"这条是不是老事件"的判断都靠它
          data['seq'] = seq;
          return AiRunEvent(type, data);
        })
        .handleError((Object e, StackTrace s) {
          onEventChannelError?.call(e, s);
          throw e;
        });
  }
}

/// 选一个事件源。
///
/// 手机（Android）用**本地 harness**，其余平台用**远端**。
/// ⚠ 这是一个**平台默认值**，不是"手机不能连桌面"：手机发起的 Run 若由桌面执行，
/// 仍然可以拿远端那个实现（总纲 §7 待定项 3 就在讨论这件事）。
RunEventSource openRunEventSource({
  required AiApiClient api,
  MethodChannel? methodChannel,
  EventChannel? eventChannel,
  bool? forceLocal,
}) {
  final local = forceLocal ?? Platform.isAndroid;
  if (local) {
    return LocalHarnessRunEventSource(
      methodChannel: methodChannel,
      eventChannel: eventChannel,
    );
  }
  return RemoteRunEventSource(api);
}

Map<String, dynamic> _map(String raw) {
  if (raw.trim().isEmpty) return <String, dynamic>{};
  final decoded = jsonDecode(raw);
  return decoded is Map
      ? Map<String, dynamic>.from(decoded)
      : <String, dynamic>{};
}

List<Map<String, dynamic>> _list(String raw) {
  final decoded = jsonDecode(raw);
  if (decoded is! List) return const [];
  return decoded
      .whereType<Map>()
      .map((e) => Map<String, dynamic>.from(e))
      .toList();
}
