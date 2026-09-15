import 'package:flutter/foundation.dart';

import '../core/ai_api_client.dart';
import '../models/ai_models.dart';

/// AI 工作台状态。
///
/// 单独一个 Store，**不塞进 LibraryStore**：画廊 / 提示词的刷新不应该让聊天页整体 rebuild。
class AiWorkspaceStore extends ChangeNotifier {
  /// [baseUrl] 每次取当前后端地址（设置里改地址后无需重建 store）；
  /// [api] 仅供测试注入。
  AiWorkspaceStore({this.baseUrlProvider, AiApiClient? api}) : _injected = api;

  final String Function()? baseUrlProvider;
  final AiApiClient? _injected;
  AiApiClient? _cached;
  String? _cachedFor;

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

  // --- 输入区 ---
  final List<AiAttachment> attachments = [];
  AiPreflightResult? preflight;
  bool sending = false;
  bool loading = false;
  String? error;

  /// 当前还没接通的能力，用**明确的话**告诉用户，而不是假装成功。
  String? notice;

  bool get hasProvider => selectedProvider != null;
  bool get canSend => !sending && selectedModel != null && (preflight?.allowed ?? true);

  /// 内置 Skill 目录（AIH-041/AIH-042）。
  ///
  /// 目录先按需求登记出来；按需加载（load_skill）属于 M5，未接通前明确标注，
  /// 绝不显示成"已可用"。
  static const skillCatalog = <String, String>{
    'anima-prompt': '将需求转为 Anima 优化提示词',
    'anima-scene-prompt': '纯场景 / 背景 / 地图资源提示词',
    'anima-workflow': '跑批、对比、审计与交付约定',
    'anima-change': '画面突变 / 记忆点呈现方法论',
    'anima-doujin-plan': '多页套图剧本与分镜设计',
    'h3-prompt-writing': 'MiniMax H3 视频提示词结构',
    'music-caption-rewriter': '音乐描述改写为结构化 caption',
  };

  // -----------------------------------------------------------------------
  //  加载
  // -----------------------------------------------------------------------

  Future<void> load() async {
    loading = true;
    error = null;
    notifyListeners();
    try {
      providers = await _api.listProviders();
      selectedProvider ??= providers.where((p) => p.enabled).firstOrNull;
      await _loadModels();
      conversations = await _api.listConversations();
      if (conversation == null && conversations.isNotEmpty) {
        await openConversation(conversations.first.id);
      }
    } catch (e) {
      error = '连接后端失败：$e';
    } finally {
      loading = false;
      notifyListeners();
    }
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
    await refreshPreflight();
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  会话
  // -----------------------------------------------------------------------

  Future<void> newConversation() async {
    try {
      final conv = await _api.createConversation(
        providerId: selectedProvider?.id,
        modelId: selectedModel?.id,
      );
      conversations = [conv, ...conversations];
      conversation = conv;
      messages = const [];
      notice = null;
    } catch (e) {
      error = '新建会话失败：$e';
    }
    notifyListeners();
  }

  Future<void> openConversation(String id) async {
    try {
      conversation = conversations.firstWhere((c) => c.id == id);
      messages = await _api.listMessages(id);
      notice = null;
    } catch (e) {
      error = '打开会话失败：$e';
    }
    notifyListeners();
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

    sending = true;
    error = null;
    try {
      if (conversation == null) {
        final conv = await _api.createConversation(
          title: body.length > 20 ? body.substring(0, 20) : body,
          providerId: selectedProvider?.id,
          modelId: selectedModel?.id,
        );
        conversations = [conv, ...conversations];
        conversation = conv;
      }
      final conv = conversation!;
      final parts = <Map<String, dynamic>>[
        if (body.isNotEmpty) {'type': 'text', 'text': body},
        for (final a in attachments)
          {'type': 'attachment', 'text': a.name, 'jsonPayload': a.toJson()},
      ];
      await _api.appendMessage(conv.id, role: 'user', text: body, parts: parts);
      messages = await _api.listMessages(conv.id);
      attachments.clear();
      preflight = null;
      notice = '消息已保存。模型执行（Run + 流式事件）属于下一阶段，尚未接通。';
    } catch (e) {
      error = '发送失败：$e';
    } finally {
      sending = false;
      notifyListeners();
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
