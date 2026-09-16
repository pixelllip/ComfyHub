import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../core/ai_api_client.dart';
import '../models/ai_models.dart';

/// AI 工作台状态。
///
/// 单独一个 Store，**不塞进 LibraryStore**：画廊 / 提示词的刷新不应该让聊天页整体 rebuild。
class AiWorkspaceStore extends ChangeNotifier {
  /// [baseUrl] 每次取当前后端地址（设置里改地址后无需重建 store）；
  /// [api] 仅供测试注入；[prefs] 仅供测试注入（默认走 SharedPreferences）。
  AiWorkspaceStore({this.baseUrlProvider, AiApiClient? api, SharedPreferences? prefs})
      : _injected = api,
        _injectedPrefs = prefs;

  final String Function()? baseUrlProvider;
  final AiApiClient? _injected;
  final SharedPreferences? _injectedPrefs;
  SharedPreferences? _prefs;
  AiApiClient? _cached;
  String? _cachedFor;

  // --- 本地记忆（只用 SharedPreferences，绝不碰密钥） ---
  static const _kLastProvider = 'comfyhub.ai.lastProviderId';
  static const _kLastModel = 'comfyhub.ai.lastModelId';
  static const _kLastEffort = 'comfyhub.ai.lastReasoningEffort';
  static const _kDraftTextPrefix = 'comfyhub.ai.draft.';
  static const _kDraftAttachPrefix = 'comfyhub.ai.draftAttachments.';

  Future<SharedPreferences> get _store async =>
      _injectedPrefs ?? (_prefs ??= await SharedPreferences.getInstance());

  /// 记住"上一次用的模型"：下次开 App 或者新建对话直接选中它，
  /// 不用每次都在聊天框里重新挑一遍。
  Future<void> _rememberModel() async {
    final prefs = await _store;
    final p = selectedProvider?.id;
    final m = selectedModel?.id;
    if (p == null || m == null) return;
    await prefs.setString(_kLastProvider, p);
    await prefs.setString(_kLastModel, m);
    await prefs.setString(_kLastEffort, reasoningEffort.wire);
  }

  Future<void> _restoreRememberedModel() async {
    final prefs = await _store;
    final providerId = prefs.getString(_kLastProvider);
    final modelId = prefs.getString(_kLastModel);
    final effort = prefs.getString(_kLastEffort);
    if (providerId != null) {
      for (final p in providers) {
        if (p.id == providerId) selectedProvider = p;
      }
    }
    selectedProvider ??= providers.where((p) => p.enabled).firstOrNull;
    await _loadModels();
    if (modelId != null) {
      for (final m in models) {
        if (m.id == modelId) selectedModel = m;
      }
    }
    if (effort != null) {
      final parsed = AiReasoningEffort.parse(effort);
      if (parsed != null) reasoningEffort = parsed;
      _clampReasoningEffort();
    }
  }

  /// 输入区草稿：按会话保存。切换会话（以及把空会话删掉）都不会丢没发出去的文字。
  Future<void> saveDraft(String conversationId, String text, List<AiAttachment> attachments) async {
    final prefs = await _store;
    if (text.trim().isEmpty && attachments.isEmpty) {
      await prefs.remove('$_kDraftTextPrefix$conversationId');
      await prefs.remove('$_kDraftAttachPrefix$conversationId');
      return;
    }
    await prefs.setString('$_kDraftTextPrefix$conversationId', text);
    await prefs.setString(
      '$_kDraftAttachPrefix$conversationId',
      jsonEncode(attachments.map((a) => a.toJson()).toList()),
    );
  }

  /// 取回草稿；没有就返回空。
  Future<({String text, List<AiAttachment> attachments})> loadDraft(String conversationId) async {
    final prefs = await _store;
    final text = prefs.getString('$_kDraftTextPrefix$conversationId') ?? '';
    final raw = prefs.getString('$_kDraftAttachPrefix$conversationId');
    final attachments = <AiAttachment>[];
    if (raw != null && raw.isNotEmpty) {
      try {
        final list = jsonDecode(raw);
        if (list is List) {
          for (final e in list.whereType<Map>()) {
            final m = Map<String, dynamic>.from(e);
            attachments.add(AiAttachment(
              name: (m['name'] ?? '').toString(),
              modality: m['modality']?.toString(),
              mimeType: (m['mimeType'] ?? 'application/octet-stream').toString(),
              sizeBytes: (m['sizeBytes'] as num?)?.toInt() ?? 0,
            ));
          }
        }
      } catch (_) {
        // 草稿坏了就当没有，不影响主流程
      }
    }
    return (text: text, attachments: attachments);
  }

  Future<void> _clearDraft(String conversationId) async {
    final prefs = await _store;
    await prefs.remove('$_kDraftTextPrefix$conversationId');
    await prefs.remove('$_kDraftAttachPrefix$conversationId');
  }

  AiApiClient get _api {
    final injected = _injected;
    if (injected != null) return injected;
    final base = baseUrlProvider!();
    if (_cached == null || _cachedFor != base) {
      _cached?.dispose();
      _cached = AiApiClient(base);
      _cachedFor = base;
    }
    return _cached!;
  }

  // --- Provider / 模型 ---
  List<AiProvider> providers = const [];
  List<AiModel> models = const [];
  AiProvider? selectedProvider;
  AiModel? selectedModel;

  // --- 会话 / 消息 ---
  List<AiConversation> conversations = const [];
  AiConversation? conversation;
  List<AiMessage> messages = const [];

  // --- Skills（M5） ------------------------------------------------------
  /// 后端扫出来的 skills（磁盘是真源，**不做本地缓存**：改完立刻重拉）。
  List<AiSkill> skills = const [];
  String? skillsError;

  /// 刷新 / 删除 / 导入进行中：界面据此显示进度并防重复点击。
  bool skillsBusy = false;

  // --- 工具清单（M4） ----------------------------------------------------
  /// 只用来在工具卡上显示分类图标；拿不到不影响聊天。
  List<AiToolInfo> tools = const [];

  // --- 输入区 ---
  final List<AiAttachment> attachments = [];

  /// 思考强度（AIH-056）：只有当前模型声明了可选档位时才有意义。
  AiReasoningEffort reasoningEffort = AiReasoningEffort.off;

  AiPreflightResult? preflight;
  bool sending = false;
  bool loading = false;
  String? error;

  /// 当前会话的 token 汇总（AIH-057），由消息里的 usage 聚合而来。
  AiUsageSummary get usageSummary => AiUsageSummary.of(messages);

  /// 正在执行的 Run（用于"停止"按钮，AIH-022）。
  String? _activeRunId;
  bool get running => _activeRunId != null;

  /// 当前还没接通的能力，用**明确的话**告诉用户，而不是假装成功。
  String? notice;

  bool get hasProvider => selectedProvider != null;
  bool get canSend => !sending && selectedModel != null && (preflight?.allowed ?? true);

  // -----------------------------------------------------------------------
  //  Skills（M5 / AIH-037 ~ AIH-045）
  // -----------------------------------------------------------------------

  /// 拉一次 skills 列表。失败**不抛异常**（右侧栏不该因为 skills 挂了就整页报错），
  /// 只把原因记进 [skillsError]。
  Future<void> reloadSkills() async {
    skillsBusy = true;
    skillsError = null;
    notifyListeners();
    try {
      skills = await _api.listSkills();
    } catch (e) {
      skillsError = '$e';
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 删除用户 skill；成功后重拉列表（磁盘是真源）。内置 skill 会被后端拒绝，
  /// 这里把后端的原话返回给调用方去弹提示。
  Future<({bool ok, String message})> deleteSkill(String name) async {
    skillsBusy = true;
    notifyListeners();
    try {
      await _api.deleteSkill(name);
      skills = await _api.listSkills();
      skillsError = null;
      return (ok: true, message: '已删除 skill「$name」');
    } catch (e) {
      return (ok: false, message: '删除「$name」失败：$e');
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 从 `%USERPROFILE%\.dsh\skills` 导入（用户在右侧栏显式点击）。
  Future<({bool ok, String message})> importDshSkills() async {
    skillsBusy = true;
    notifyListeners();
    try {
      final result = await _api.importDshSkills();
      skills = await _api.listSkills();
      skillsError = null;
      return (ok: true, message: '${result.summary}（来源：${result.source}）');
    } catch (e) {
      return (ok: false, message: '从 DSH 导入失败：$e');
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 注册 / 覆盖一个用户 skill（AI 或用户在界面上登记）。
  Future<({bool ok, String message})> registerSkill({
    required String name,
    required String description,
    String? whenToUse,
    required String content,
  }) async {
    try {
      final saved = await _api.createSkill(
        name: name,
        description: description,
        whenToUse: whenToUse,
        content: content,
      );
      skills = await _api.listSkills();
      skillsError = null;
      notifyListeners();
      return (ok: true, message: '已注册 skill「${saved.name}」');
    } catch (e) {
      notifyListeners();
      return (ok: false, message: '注册失败：$e');
    }
  }

  /// 工具清单：气泡上的分类图标要用。失败静默（非关键路径）。
  Future<void> reloadTools() async {
    try {
      tools = await _api.listTools();
    } catch (_) {
      // 后端老版本 / 临时不可用：图标退回按名字猜，不打扰用户
    }
    notifyListeners();
  }

  /// 工具名 → 分类（skill / files / comfy）。先查后端给的清单，查不到按名字兜底。
  String toolCategoryOf(String name) {
    for (final t in tools) {
      if (t.name == name) return t.category;
    }
    if (name.contains('skill')) return 'skill';
    if (name.startsWith('read_') ||
        name.startsWith('write_') ||
        name.startsWith('list_dir') ||
        name.contains('file') ||
        name.contains('dir')) {
      return 'files';
    }
    return 'comfy';
  }

  // -----------------------------------------------------------------------
  //  工具调用（M4）：批准 / 拒绝
  // -----------------------------------------------------------------------

  /// 批准一次待批准的工具调用（AIH-035 / AIH-049）。
  Future<void> approveToolCall(String callId) => _resolveToolCall(callId, approve: true);

  Future<void> denyToolCall(String callId) => _resolveToolCall(callId, approve: false);

  Future<void> _resolveToolCall(String callId, {required bool approve}) async {
    try {
      final accepted = approve ? await _api.approveToolCall(callId) : await _api.denyToolCall(callId);
      if (!accepted) {
        // 点晚了：这次调用已经结束 / 超时，**如实说**，不要假装批准成功
        notice = '这次工具调用已经结束或超时，${approve ? '批准' : '拒绝'}没有生效。';
      } else {
        _patchToolCall(
          callId,
          (c) => c.copyWith(
            status: approve ? AiToolCallStatus.running : AiToolCallStatus.denied,
            approval: approve ? c.approval : 'denied',
          ),
        );
        notice = approve ? '已批准这次工具调用，模型会继续执行。' : '已拒绝这次工具调用。';
      }
    } catch (e) {
      notice = '${approve ? '批准' : '拒绝'}失败：$e';
    }
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  加载
  // -----------------------------------------------------------------------

  /// 冷启动加载。
  ///
  /// 每次开 App **都新建一个聊天记录**（用户明确要求）：历史会话仍在左侧列表里，
  /// 但默认落在一个干净的对话上，不用先手动点「新建对话」再开聊。
  /// 上一次用的模型会从本地记忆里恢复，这次也不用重新挑。
  Future<void> load() async {
    loading = true;
    error = null;
    notifyListeners();
    try {
      providers = await _api.listProviders();
      await _restoreRememberedModel();
      conversations = await _api.listConversations();
      await newConversation();
    } catch (e) {
      error = '连接后端失败：$e';
    } finally {
      loading = false;
      notifyListeners();
    }
    // skills / 工具清单走"尽力而为"：拉不到不影响聊天（各自把失败记在自己的字段里）
    await reloadSkills();
    await reloadTools();
  }

  Future<void> _loadModels() async {
    final p = selectedProvider;
    if (p == null) {
      models = const [];
      selectedModel = null;
      return;
    }
    models = await _api.listModels(p.id);
    if (selectedModel != null && !models.any((m) => m.id == selectedModel!.id)) {
      selectedModel = null;
    }
    selectedModel ??= models.where((m) => m.enabled).firstOrNull;
    _clampReasoningEffort();
  }

  Future<void> selectProvider(AiProvider? p) async {
    selectedProvider = p;
    selectedModel = null;
    await _loadModels();
    await refreshPreflight();
    notifyListeners();
  }

  /// 切换模型必须立刻重算准入（AIH-029），不能等到发送时才提示。
  Future<void> selectModel(AiModel? m) async {
    selectedModel = m;
    _clampReasoningEffort();
    await refreshPreflight();
    await _rememberModel();
    notifyListeners();
  }

  /// 选择思考强度。模型没声明这个档位时直接忽略（真源在模型目录，AIH-056）。
  void selectReasoningEffort(AiReasoningEffort effort) {
    final allowed = selectedModel?.selectableEfforts ?? const <AiReasoningEffort>[];
    if (!allowed.contains(effort)) return;
    reasoningEffort = effort;
    _rememberModel();
    notifyListeners();
  }

  /// 换模型后把档位收敛回合法集合：新模型没有这个档位就退回"关闭"。
  void _clampReasoningEffort() {
    final allowed = selectedModel?.selectableEfforts ?? const <AiReasoningEffort>[];
    if (!allowed.contains(reasoningEffort)) reasoningEffort = AiReasoningEffort.off;
  }

  // -----------------------------------------------------------------------
  //  会话
  // -----------------------------------------------------------------------

  Future<void> newConversation() async {
    try {
      // 换新对话之前，把上一个"空壳"清掉（AIH-018 的清理策略，见 [isEmptyConversation]）
      await _cleanupEmptyConversation();
      final conv = await _api.createConversation(
        providerId: selectedProvider?.id,
        modelId: selectedModel?.id,
      );
      conversations = [conv, ...conversations];
      conversation = conv;
      messages = const [];
      _liveReasoning.clear();
      _liveToolCalls.clear();
      notice = null;
    } catch (e) {
      error = '新建会话失败：$e';
    }
    notifyListeners();
  }

  /// 打开历史会话。同样先清理上一个空会话。
  Future<void> openConversation(String id) async {
    if (conversation?.id == id) return;
    try {
      await _cleanupEmptyConversation();
      conversation = conversations.firstWhere((c) => c.id == id);
      messages = await _api.listMessages(id);
      _pruneLiveState();
      notice = null;
    } catch (e) {
      error = '打开会话失败：$e';
    }
    notifyListeners();
  }

  /// 当前会话是不是"空壳"：一条消息都没有，输入区也没有待发送的内容。
  ///
  /// 输入区有文字或附件时**不删** —— 那些草稿挂在会话上（见 [saveDraft]），
  /// 删掉等于把用户打了一半的字扔掉。
  bool get isEmptyConversation =>
      (conversation?.id ?? '').isNotEmpty && messages.isEmpty && attachments.isEmpty;

  /// 切换会话时顺手删掉上一个空会话（用户要求）：聊天记录列表不该堆一串"新对话"。
  Future<void> _cleanupEmptyConversation() async {
    final conv = conversation;
    if (conv == null) return;
    if (messages.isNotEmpty || attachments.isNotEmpty) return;
    // 本地乐观插入的占位（还没落库）不能拿去删
    if (conv.id.startsWith('local-')) return;
    try {
      await _api.deleteConversation(conv.id);
      conversations = conversations.where((c) => c.id != conv.id).toList();
    } catch (_) {
      // 删不掉也不影响切换（可能是归档 / 网络抖动），保持静默
    }
  }

  Future<void> renameConversation(String title) async {
    final conv = conversation;
    if (conv == null) return;
    try {
      final updated = await _api.patchConversation(conv.id, {'title': title});
      conversations = conversations.map((c) => c.id == updated.id ? updated : c).toList();
      conversation = updated;
    } catch (e) {
      error = '重命名失败：$e';
    }
    notifyListeners();
  }

  Future<void> archiveConversation(String id, {bool archived = true}) async {
    try {
      await _api.patchConversation(id, {'archived': archived});
      conversations = await _api.listConversations(includeArchived: archived);
      if (conversation?.id == id && archived) {
        conversation = null;
        messages = const [];
      }
    } catch (e) {
      error = '归档失败：$e';
    }
    notifyListeners();
  }

  Future<void> deleteConversation(String id) async {
    try {
      await _api.deleteConversation(id);
      conversations = conversations.where((c) => c.id != id).toList();
      if (conversation?.id == id) {
        conversation = null;
        messages = const [];
      }
    } catch (e) {
      error = '删除失败：$e';
    }
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  附件
  // -----------------------------------------------------------------------

  void addAttachment(AiAttachment a) {
    attachments.add(a);
    refreshPreflight();
  }

  void removeAttachmentAt(int index) {
    if (index < 0 || index >= attachments.length) return;
    attachments.removeAt(index);
    refreshPreflight();
  }

  void clearAttachments() {
    attachments.clear();
    preflight = null;
    notifyListeners();
  }

  /// 预检由**后端**判定（AIH-029/030）；前端只展示结论，不自行放行。
  Future<void> refreshPreflight() async {
    final p = selectedProvider;
    final m = selectedModel;
    if (p == null || m == null || attachments.isEmpty) {
      preflight = null;
      notifyListeners();
      return;
    }
    try {
      preflight = await _api.preflight(providerId: p.id, modelId: m.id, attachments: attachments);
    } catch (e) {
      preflight = AiPreflightResult(allowed: false, blockers: ['预检请求失败：$e']);
    }
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  发送
  // -----------------------------------------------------------------------

  /// 发送当前输入。
  ///
  /// 现阶段（M1/M2 之间）只做两件**真实**的事：把用户消息与附件引用持久化，
  /// 并在预检不通过时**阻断**。模型执行（Run + SSE）属于 M2，未接通前用
  /// [notice] 明确告知，绝不伪造助手回复。
  Future<void> send(String text) async {
    if (sending) return;
    final body = text.trim();
    if (body.isEmpty && attachments.isEmpty) return;
    if (!(preflight?.allowed ?? true)) {
      notice = '有附件未通过准入，已阻断发送：${preflight!.blockers.join('；')}';
      notifyListeners();
      return;
    }
    final provider = selectedProvider;
    final model = selectedModel;
    if (provider == null || model == null) {
      error = '请先选择 Provider 与模型（设置 → AI 模型）';
      notifyListeners();
      return;
    }

    sending = true;
    error = null;
    notice = null;
    notifyListeners();

    try {
      if (conversation == null) {
        final conv = await _api.createConversation(
          title: body.length > 20 ? body.substring(0, 20) : body,
          providerId: provider.id,
          modelId: model.id,
        );
        conversations = [conv, ...conversations];
        conversation = conv;
      }
      final conv = conversation!;
      // 模型没声明推理能力时不带这个字段（后端也会再挡一次，AIH-056）
      final effort = (model.supportsReasoningEffort && reasoningEffort.isThinking)
          ? reasoningEffort.wire
          : null;
      final start = await _api.startRun(
        conv.id,
        text: body,
        providerId: provider.id,
        modelId: model.id,
        reasoningEffort: effort,
      );
      _activeRunId = start.runId;
      // 记下"助手消息 → Run"的关联：失败后的「重试」要把它作为 retryOfRunId 带回去（AIH-024）。
      // 在请求返回时就记，而不是等 `run.started` 事件 —— 后者在流中断时根本收不到。
      _runIds[start.assistantMessageId] = start.runId;

      // 乐观插入：用户消息 + 空的助手流式消息，收到 delta 就地增长
      messages = [
        ...messages,
        AiMessage(
          id: start.userMessageId ?? 'local-user',
          conversationId: conv.id,
          seq: messages.length + 1,
          role: 'user',
          status: 'complete',
          text: body,
        ),
        AiMessage(
          id: start.assistantMessageId,
          conversationId: conv.id,
          seq: messages.length + 2,
          role: 'assistant',
          status: 'streaming',
          text: '',
          modelId: model.id,
          reasoningEffort: effort,
        ),
      ];
      attachments.clear();
      preflight = null;
      // 已经发出去了，本地草稿也一并清掉（否则下次切回这个会话会看到旧内容）
      unawaited(_clearDraft(conv.id));
      notifyListeners();

      await _consumeRun(start.runId, start.assistantMessageId, conv.id);
    } on AiApiException catch (e) {
      error = '启动失败：${e.message}';
      sending = false;
      notifyListeners();
    } catch (e) {
      error = '发送失败：$e';
      sending = false;
      notifyListeners();
    }
  }

  Future<void> _consumeRun(String runId, String assistantMessageId, String conversationId) async {
    var streamed = '';
    // 这一条助手消息的实时思考 / 工具调用（气泡边流边渲染）
    _liveReasoning.remove(assistantMessageId);
    _liveToolCalls.remove(assistantMessageId);
    try {
      await for (final event in _api.runEvents(runId)) {
        switch (event.type) {
          case 'text.delta':
            streamed += event.text ?? '';
            _replaceMessage(assistantMessageId, text: streamed, status: 'streaming');
            notifyListeners();
          // 思考增量（M4）：按消息累加，气泡里折叠展示
          case 'reasoning.delta':
            final delta = event.text ?? '';
            if (delta.isEmpty) break;
            _liveReasoning.update(assistantMessageId, (v) => v + delta, ifAbsent: () => delta);
            notifyListeners();
          // 工具调用请求：approval=pending 的要在气泡上等用户点「批准 / 拒绝」
          case 'tool.requested':
            final approval = event.approval ?? 'not_required';
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) => (prev ?? AiToolCallState(callId: event.callId ?? '', name: event.toolName ?? ''))
                  .copyWith(
                name: event.toolName ?? prev?.name,
                arguments: event.arguments ?? prev?.arguments,
                approval: approval,
                status: approval == 'pending'
                    ? AiToolCallStatus.pendingApproval
                    : AiToolCallStatus.running,
              ),
            );
            notifyListeners();
          case 'tool.started':
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) => (prev ?? AiToolCallState(callId: event.callId ?? '', name: event.toolName ?? ''))
                  .copyWith(name: event.toolName ?? prev?.name, status: AiToolCallStatus.running),
            );
            notifyListeners();
          case 'tool.completed':
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) => (prev ?? AiToolCallState(callId: event.callId ?? '', name: event.toolName ?? ''))
                  .copyWith(
                name: event.toolName ?? prev?.name,
                status: AiToolCallStatus.ok,
                preview: event.preview ?? prev?.preview,
                elapsedMs: event.elapsedMs ?? prev?.elapsedMs,
              ),
            );
            notifyListeners();
          case 'tool.failed':
            final denied = (event.approval ?? '') == 'denied';
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) => (prev ?? AiToolCallState(callId: event.callId ?? '', name: event.toolName ?? ''))
                  .copyWith(
                name: event.toolName ?? prev?.name,
                // 被拒绝不是"出错"：状态要分开显示
                status: denied ? AiToolCallStatus.denied : AiToolCallStatus.failed,
                error: event.message ?? event.code,
                approval: event.approval ?? prev?.approval,
                elapsedMs: event.elapsedMs ?? prev?.elapsedMs,
              ),
            );
            notifyListeners();
          case 'message.completed':
            streamed = event.text ?? streamed;
            // parts 是**权威的有序块**（正文 / 思考 / 工具调用 / 工具结果交错）；
            // 老后端不给 parts 时退回实时累积，二者取其一，不混用。
            final parts = event.parts;
            final reasoning = event.reasoning;
            if (parts == null && (reasoning ?? '').isNotEmpty) {
              _liveReasoning[assistantMessageId] = reasoning!;
            }
            _replaceMessage(
              assistantMessageId,
              text: streamed,
              status: 'complete',
              parts: parts,
              // token 统计（AIH-057）：后端已在事件里给了归一化 usage
              usage: event.data['usage'] is Map
                  ? AiTokenUsage.fromJson(Map<String, dynamic>.from(event.data['usage'] as Map))
                  : null,
              reasoningEffort: event.data['reasoningEffort']?.toString(),
            );
            notifyListeners();
          case 'run.failed':
            _replaceMessage(assistantMessageId, text: streamed, status: 'failed');
            error = '[${event.code ?? 'ERROR'}] ${event.message ?? '运行失败'}';
          case 'run.cancelled':
            _replaceMessage(assistantMessageId, text: streamed, status: 'cancelled');
            notice = '已停止本次生成。';
          // 这些事件不需要额外动作：
          //  - run.started 带 tools 清单（留给后续"本次可用工具"展示）
          //  - message.started / run.completed 只标示阶段
          case 'run.started':
          case 'message.started':
          case 'run.completed':
            break;
          default:
            // 未知事件（后端加了新的）不能悄悄丢：至少留一条可查的日志
            debugPrint('AI 事件流收到未处理事件：${event.type}');
        }
      }
    } catch (e) {
      error = '事件流中断：$e';
    } finally {
      _activeRunId = null;
      sending = false;
      // 与服务端对齐一次（seq、状态、usage 都以库为准）
      await _reloadMessages(conversationId);
      conversations = await _safeListConversations();
      notifyListeners();
    }
  }

  /// 停止：取消上游请求（AIH-022）。
  Future<void> stop() async {
    final runId = _activeRunId;
    if (runId == null) return;
    try {
      await _api.cancelRun(runId);
      notice = '已请求停止…';
    } catch (e) {
      error = '停止失败：$e';
    }
    notifyListeners();
  }

  /// 重试（AIH-024）：**新建一个 Run**，用原来的问题、同一个 Provider/模型与思考强度，
  /// 通过 `retryOfRunId` 关联回去，便于事后看出这是哪次失败的重放。
  ///
  /// 不重放工具调用（首期还没有工具）；失败/取消的助手消息会被换成新的流式占位。
  Future<void> retry(String assistantMessageId) async {
    if (sending) return;
    final conv = conversation;
    final p = selectedProvider;
    final m = selectedModel;
    if (conv == null || p == null || m == null) {
      error = '无法重试：请先选择 Provider 与模型';
      notifyListeners();
      return;
    }
    final index = messages.indexWhere((x) => x.id == assistantMessageId);
    if (index <= 0) {
      error = '无法重试：找不到这条回复对应的提问';
      notifyListeners();
      return;
    }
    // 助手消息前面最近的一条用户消息就是这次要重放的提问
    final ask = messages
        .sublist(0, index)
        .lastWhere((x) => x.isUser, orElse: () => messages[index]);
    if (!ask.isUser) {
      error = '无法重试：找不到这条回复对应的提问';
      notifyListeners();
      return;
    }

    sending = true;
    error = null;
    notice = null;
    notifyListeners();

    try {
      final effort = (m.supportsReasoningEffort && reasoningEffort.isThinking)
          ? reasoningEffort.wire
          : null;
      final start = await _api.startRun(
        conv.id,
        text: ask.text,
        providerId: p.id,
        modelId: m.id,
        reasoningEffort: effort,
        retryOfRunId: _runIdOf(assistantMessageId),
      );
      _activeRunId = start.runId;
      // 这次 Run 与助手消息的关联就地记下（不依赖事件流，刷新消息也不会丢）
      _runIds[start.assistantMessageId] = start.runId;
      // 就地替换那条失败消息，不新增一条重复的助手气泡
      messages = [
        for (final msg in messages)
          if (msg.id == assistantMessageId)
            AiMessage(
              id: start.assistantMessageId,
              conversationId: conv.id,
              seq: msg.seq,
              role: 'assistant',
              status: 'streaming',
              text: '',
              modelId: m.id,
              reasoningEffort: effort,
            )
          else
            msg,
      ];
      notifyListeners();
      await _consumeRun(start.runId, start.assistantMessageId, conv.id);
    } on AiApiException catch (e) {
      error = '重试失败：${e.message}';
      sending = false;
      notifyListeners();
    } catch (e) {
      error = '重试失败：$e';
      sending = false;
      notifyListeners();
    }
  }

  /// 这次 Run 的 id（由 `run.started` 事件记下），重试时作为 `retryOfRunId` 带回去。
  final Map<String, String> _runIds = {};

  String? _runIdOf(String assistantMessageId) => _runIds[assistantMessageId];

  // --- 流式实时状态（工具卡 / 思考）---------------------------------------

  /// 助手消息 id → 实时工具调用（有序）。
  final Map<String, List<AiToolCallState>> _liveToolCalls = {};

  /// 助手消息 id → 实时思考正文。
  final Map<String, String> _liveReasoning = {};

  /// 一条消息要渲染的工具调用（有序）。
  ///
  /// 优先用 `parts`（`message.completed` 给的权威有序块 / 历史消息）；
  /// 流式过程中 parts 还没有，就用事件累积的实时列表。
  List<AiToolCallState> toolCallsFor(AiMessage message) {
    final fromParts = AiToolCallState.listFromParts(message.parts);
    if (fromParts.isNotEmpty) return fromParts;
    return _liveToolCalls[message.id] ?? const [];
  }

  /// 一条消息的思考正文（`reasoning.delta` 累积 / parts 里的 reasoning 块）。
  String reasoningFor(AiMessage message) {
    final fromParts = message.parts
        .where((p) => p.type == 'reasoning')
        .map((p) => p.text ?? '')
        .where((t) => t.isNotEmpty)
        .join('\n\n');
    if (fromParts.isNotEmpty) return fromParts;
    return _liveReasoning[message.id] ?? '';
  }

  void _upsertToolCall(
    String messageId,
    String callId,
    AiToolCallState Function(AiToolCallState? previous) update,
  ) {
    if (callId.isEmpty) return;
    final list = _liveToolCalls.putIfAbsent(messageId, () => <AiToolCallState>[]);
    final i = list.indexWhere((c) => c.callId == callId);
    final next = update(i < 0 ? null : list[i]);
    if (i < 0) {
      list.add(next);
    } else {
      list[i] = next;
    }
  }

  void _patchToolCall(String callId, AiToolCallState Function(AiToolCallState call) update) {
    for (final list in _liveToolCalls.values) {
      final i = list.indexWhere((c) => c.callId == callId);
      if (i >= 0) list[i] = update(list[i]);
    }
  }

  /// 流式状态只对"还在列表里的消息"有意义：换会话 / 刷新后清掉孤儿，避免内存只增不减。
  void _pruneLiveState() {
    final ids = messages.map((m) => m.id).toSet();
    _liveReasoning.removeWhere((k, _) => !ids.contains(k));
    _liveToolCalls.removeWhere((k, _) => !ids.contains(k));
  }

  /// 就地改一条消息。
  ///
  /// **性能**：逐字回包时每次 delta 都会走到这里，所以只复制列表本身
  /// （O(n) 指针拷贝），**不再重建其它 `AiMessage`**——之前 `map().toList()`
  /// 会把整段历史的每个气泡都重造一遍，长对话越聊越卡。
  void _replaceMessage(
    String id, {
    String? text,
    String? status,
    AiTokenUsage? usage,
    String? reasoningEffort,
    List<AiMessagePart>? parts,
  }) {
    final index = messages.indexWhere((m) => m.id == id);
    if (index < 0) return;
    final next = List<AiMessage>.of(messages);
    next[index] = messages[index].copyWith(
      text: text,
      status: status,
      usage: usage,
      reasoningEffort: reasoningEffort,
      parts: parts,
    );
    messages = next;
  }

  Future<void> _reloadMessages(String conversationId) async {
    try {
      final fresh = await _api.listMessages(conversationId);
      // 后端可能还没落 parts（消息仍在写库）：保留事件里拿到的那些，别把工具卡擦掉
      final known = {for (final m in messages) m.id: m.parts};
      messages = [
        for (final m in fresh)
          if (m.parts.isEmpty && (known[m.id]?.isNotEmpty ?? false))
            m.copyWith(parts: known[m.id])
          else
            m,
      ];
      _pruneLiveState();
    } catch (_) {
      // 保留本地已有的流式内容，别因为一次刷新失败把回复擦掉
    }
  }

  Future<List<AiConversation>> _safeListConversations() async {
    try {
      return await _api.listConversations();
    } catch (_) {
      return conversations;
    }
  }

  void clearNotice() {
    notice = null;
    notifyListeners();
  }
}

extension _FirstOrNull<T> on Iterable<T> {
  T? get firstOrNull => isEmpty ? null : first;
}
