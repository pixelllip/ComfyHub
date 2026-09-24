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

  /// 事件流（`after` 用于断线续传，语义与 SSE 的 `?after=<seq>` 一致）。
  Stream<AiRunEvent> events(String runId, {int after = 0});

  /// 回答 `user.question`（提问闸门；桌面是 HTTP，手机是本地闸门）。
  Future<void> answerQuestion(String questionId, String answer, {int? optionIndex});
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
  Future<void> cancelRun(String runId) => _api.cancelRun(runId);

  @override
  Stream<AiRunEvent> events(String runId, {int after = 0}) => _api.runEvents(runId, after: after);

  @override
  Future<void> answerQuestion(String questionId, String answer, {int? optionIndex}) =>
      _api.answerQuestion(questionId, answer: answer, optionIndex: optionIndex);
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
  })  : _method = methodChannel ?? const MethodChannel(methodChannelName),
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
  }) async =>
      _map(await _method.invokeMethod<String>('fetchRunKit', {
        'baseUrl': ?baseUrl,
        'token': ?token,
        'providerId': ?providerId,
        'modelId': ?modelId,
      }) ??
      '{}');

  /// 桌面离线时用缓存的那一份（拿不到返回 null）。
  Future<Map<String, dynamic>?> cachedRunKit() async {
    final raw = await _method.invokeMethod<String>('cachedRunKit');
    if (raw == null || raw.isEmpty) return null;
    return _map(raw);
  }

  /// 把上次没跑完的 Run 按 R6 收尾，返回收尾了几条。
  Future<int> recoverUnfinished() async =>
      await _method.invokeMethod<int>('recoverUnfinished') ?? 0;

  Future<List<Map<String, dynamic>>> listConversations() async =>
      _list(await _method.invokeMethod<String>('listConversations') ?? '[]');

  /// 在**本地库**建一条会话（M3.2 ②：离线也要能开会话）。
  ///
  /// `conversationId` 由界面生成并传下去（理由同 `startRun` 的那一对消息 id：
  /// 界面要拿同一个 id 去认自己插的那条会话）。
  ///
  /// ⚠ 本地建的会话会**进 `_outbox`**（`HarnessStore.createConversation` 默认 `queue=true`），
  /// 等桌面在线时由 `/api/sync/push` 上行 —— 这正是 M4 要的那种"手机产生的会话在桌面可见"。
  Future<Map<String, dynamic>> createConversation({
    required String conversationId,
    required String title,
    String? providerId,
    String? modelId,
    String? promptVersion,
  }) async =>
      _map(await _method.invokeMethod<String>('createConversation', {
        'conversationId': conversationId,
        'title': title,
        'providerId': ?providerId,
        'modelId': ?modelId,
        'promptVersion': ?promptVersion,
      }) ??
      '{}');

  Future<List<Map<String, dynamic>>> listMessages(String conversationId) async =>
      _list(await _method.invokeMethod<String>('listMessages', {'conversationId': conversationId}) ?? '[]');

  Future<List<Map<String, dynamic>>> listParts(String messageId) async =>
      _list(await _method.invokeMethod<String>('listParts', {'messageId': messageId}) ?? '[]');

  Future<int> pendingOutbox() async => await _method.invokeMethod<int>('pendingOutbox') ?? 0;

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
      conversationId: (map['conversationId'] ?? request.conversationId).toString(),
      // harness 回显的就是库里那条；拿不到才回落到本地那份（两者应当相等，
      // 不相等说明桥没把 id 透下去 —— 那时界面会画不出字，所以别在这里悄悄兜住）
      userMessageId: (map['userMessageId'] ?? request.userMessageId)?.toString(),
      assistantMessageId: (map['assistantMessageId'] ?? request.assistantMessageId)?.toString(),
      kitVersion: map['kitVersion']?.toString(),
      promptVersion: map['promptVersion']?.toString(),
    );
  }

  @override
  Future<void> cancelRun(String runId) =>
      _method.invokeMethod<bool>('cancelRun', {'runId': runId});

  @override
  Future<void> answerQuestion(String questionId, String answer, {int? optionIndex}) =>
      _method.invokeMethod<bool>('answerQuestion', {
        'questionId': questionId,
        'answer': answer,
        'optionIndex': ?optionIndex,
      });

  /// 订阅本地事件流。
  ///
  /// 通道送的是 `{seq, type, payloadJson}`（payload 用**字符串**过线：嵌套 Map 的
  /// 类型映射在两端各有一套规则，字符串最不容易出意外）。
  @override
  Stream<AiRunEvent> events(String runId, {int after = 0}) {
    final raw = _events.receiveBroadcastStream({'runId': runId, 'after': after});
    return raw.map((dynamic event) {
      final map = (event as Map).cast<Object?, Object?>();
      final type = (map['type'] ?? 'message').toString();
      final seq = (map['seq'] as num?)?.toInt() ?? 0;
      final payloadRaw = map['payloadJson']?.toString() ?? '{}';
      final data = _map(payloadRaw);
      // seq 也放进 payload：`after` 续传与"这条是不是老事件"的判断都靠它
      data['seq'] = seq;
      return AiRunEvent(type, data);
    }).handleError((Object e, StackTrace s) {
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
    return LocalHarnessRunEventSource(methodChannel: methodChannel, eventChannel: eventChannel);
  }
  return RemoteRunEventSource(api);
}

Map<String, dynamic> _map(String raw) {
  if (raw.trim().isEmpty) return <String, dynamic>{};
  final decoded = jsonDecode(raw);
  return decoded is Map ? Map<String, dynamic>.from(decoded) : <String, dynamic>{};
}

List<Map<String, dynamic>> _list(String raw) {
  final decoded = jsonDecode(raw);
  if (decoded is! List) return const [];
  return decoded.whereType<Map>().map((e) => Map<String, dynamic>.from(e)).toList();
}
