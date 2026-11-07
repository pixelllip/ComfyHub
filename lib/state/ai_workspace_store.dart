import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show AppLifecycleListener, FocusNode;
import 'package:shared_preferences/shared_preferences.dart';

import '../core/ai_api_client.dart';
import '../core/credential_store.dart';
import '../core/inbound_auth_api.dart';
import '../core/pairing_token.dart';
import '../core/provider_store.dart';
import '../core/run_event_source.dart';
import '../core/workflow_blueprint.dart';
import '../models/ai_models.dart';
import '../models/layout_embed.dart';
import '../models/models.dart' show ComfyWorkflowList;

/// AI 工作台状态。
///
/// 单独一个 Store，**不塞进 LibraryStore**：画廊 / 提示词的刷新不应该让聊天页整体 rebuild。
class AiWorkspaceStore extends ChangeNotifier {
  /// [baseUrl] 每次取当前后端地址（设置里改地址后无需重建 store）；
  /// [api] 仅供测试注入；[prefs] 仅供测试注入（默认走 SharedPreferences）；
  /// [source] 仅供测试注入（默认按平台选：Android → 本地 harness，其余 → 远端 HTTP）；
  /// [credentials] 仅供测试注入（默认按平台选：Android → 本机密钥库，其余 → 后端托管）；
  /// [providers] 仅供测试注入（默认按平台选：Android → **本机库**，其余 → 后端库）；
  /// [authApiFactory] 仅供测试注入（`syncCredentialsFromDesktop` 用的那个客户端）；
  /// [backendReady] **冷启动闸门**：等本地服务有个结论再发第一批请求
  /// （`BackendLauncher.settled`；不注入 = 不等）。
  AiWorkspaceStore({
    this.baseUrlProvider,
    AiApiClient? api,
    SharedPreferences? prefs,
    RunEventSource? source,
    CredentialStore? credentials,
    ProviderStore? providers,
    InboundAuthApi Function(String baseUrl)? authApiFactory,
    this.backendReady,
  }) : _injected = api,
       _injectedPrefs = prefs,
       _injectedSource = source,
       _injectedCredentials = credentials,
       _injectedProviders = providers,
       _injectedAuthApi = authApiFactory;

  final String Function()? baseUrlProvider;

  /// 冷启动时"后端能不能开始干活"的闸门（见 [AiWorkspaceStore] 构造参数注释）。
  ///
  /// 不注入 = 不等（测试与手机那几条路本来就不需要）。
  final Future<void> Function()? backendReady;

  /// 页面在**打后端的初始化动作**之前先 `await` 它（转发 [backendReady]）。
  ///
  /// 为什么要暴露出来而不是让页面自己去读 `BackendLauncher`：AI 页里那些
  /// "一显示就去拉"的地方（右侧栏的 ComfyUI 状态 / 实时进度 / 待审批）拿到的
  /// `baseUrl` 在冷启动那一秒还是 8080 兜底值，请求注定失败 —— 而其中
  /// `/api/capture/status` 那个失败是**会显示出来且不会自己消失**的（只有用户手点刷新）。
  /// 走 store 这一道的好处：测试里没注入闸门时它是个空操作，不会因为缺 provider 就炸。
  Future<void> waitForBackend() => backendReady?.call() ?? Future<void>.value();

  final AiApiClient? _injected;
  final SharedPreferences? _injectedPrefs;
  final RunEventSource? _injectedSource;
  final CredentialStore? _injectedCredentials;
  final ProviderStore? _injectedProviders;
  final InboundAuthApi Function(String baseUrl)? _injectedAuthApi;
  SharedPreferences? _prefs;
  AiApiClient? _cached;
  String? _cachedFor;
  RunEventSource? _cachedSource;
  AiApiClient? _cachedSourceFor;
  CredentialStore? _cachedCredentials;
  AiApiClient? _cachedCredentialsFor;
  ProviderStore? _cachedProviders;
  AiApiClient? _cachedProvidersFor;

  // --- 本地记忆（只用 SharedPreferences，绝不碰密钥） ---
  static const _kLastProvider = 'comfyhub.ai.lastProviderId';
  static const _kLastModel = 'comfyhub.ai.lastModelId';
  static const _kLastEffort = 'comfyhub.ai.lastReasoningEffort';
  static const _kDraftTextPrefix = 'comfyhub.ai.draft.';
  static const _kDraftAttachPrefix = 'comfyhub.ai.draftAttachments.';

  /// Provider 清单与模型目录的来源（M6 的另一半）：**手机 = 本机库**、桌面 = 后端库。
  ///
  /// ⚠ `forceLocal` 用的是**事件源**那一位（`_source.isLocal`）—— 它与
  /// `openRunEventSource` / `openCredentialStore` / `openProviderStore` 是**同一套平台判据**
  /// （生产里都是 `Platform.isAndroid`；测试里注入本地源时它就是 true）。
  /// 不这么接的话会出现一种很难查的错：**"手机"的用例偷偷去打 HTTP**
  /// （测试进程里 `Platform.isAndroid` 恒为 false）。
  ProviderStore get _providerStore {
    final api = _api;
    if (_cachedProviders == null || _cachedProvidersFor != api) {
      _cachedProviders =
          _injectedProviders ??
          openProviderStore(api: api, forceLocal: _source.isLocal);
      _cachedProvidersFor = api;
    }
    return _cachedProviders!;
  }

  /// 这一台是不是**手机**（清单、模型目录、密钥都归本机管）。
  bool get _isPhone => _source.isLocal;

  /// 布局 → 提示词：用**当前选中的 Provider / 模型**当译者。
  ///
  /// 不另开一套配置：用户在工作台挑哪一个，翻译就用哪一个（想用便宜的本地模型
  /// 就在这里把它选中即可）。校验不过的重试在服务端做，这里只负责原样带回。
  Future<Map<String, dynamic>> translateLayout({
    required Map<String, Object?> spec,
    required String intent,
  }) {
    final provider = selectedProvider;
    final model = selectedModel;
    if (provider == null || model == null) {
      throw StateError('还没有选模型');
    }
    return _api.translateLayout(
      spec: spec,
      intent: intent,
      providerId: provider.id,
      model: model.id,
    );
  }

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

  /// 只把**Provider 清单**重新读一次（不重启会话、不碰草稿、不动正在跑的 Run）。
  ///
  /// 为什么需要它（总纲 §27.2 第 12 条）：手机上 Provider 的真源是**本机库**，
  /// 而「AI 模型与凭据」页新建 / 改完一个 Provider 之后，聊天页手里那份 `providers`
  /// 还是进页面时读的旧清单 —— 用户必须切走再切回来（= 重新跑一遍 [load]）才看得到，
  /// 于是看起来就像"我明明建好了它却说没有"。
  ///
  /// 触发点是**打开模型选择器那一刻**（`_ModelPickerDialog.initState`）：要挑模型的时候
  /// 清单必须是最新的，而这一次重读只是本机库的一次查询（手机上一条网络请求都不打）。
  /// 清单真的变了才顺手重读模型目录 —— 没变时也调 `_loadModels` 就是白白抖一下选择器。
  Future<void> refreshProviders() async {
    try {
      final fresh = await _overlayDeviceCredentials(
        await _providerStore.listProviders(),
      );
      final changed =
          fresh.length != providers.length ||
          !List.generate(
            fresh.length,
            (i) =>
                fresh[i].id == providers[i].id &&
                fresh[i].revision == providers[i].revision,
          ).every((same) => same);
      providers = fresh;
      if (providers.isEmpty) {
        error ??= _emptyLocalProvidersNotice;
      } else if (error == _emptyLocalProvidersNotice) {
        error = null;
      }
      if (changed) await _loadModels();
      // 跨 Provider 的完整目录：选择器按来源分组显示要的就是它（见 [catalogByProvider]）。
      // 放在"清单变没变"判据**之外** —— 模型的增删改不会动 provider 那一行，
      // 而用户要的恰恰是"刚加的模型立刻能选到"。
      await _loadCatalog();
    } catch (e) {
      error = "读本机那份 Provider 清单失败，$e";
    }
    notifyListeners();
  }

  /// 把**每条** Provider 的模型目录各读一次（选择器按来源分组显示的数据源）。
  ///
  /// 三条纪律：
  ///  1. **逐条独立**：一条读失败只记进 [catalogIssues]，其余照常显示
  ///     —— 少一个 Provider 的模型不该让整个选择器空掉；
  ///  2. **来源与 [models] 一样**（本机库 / 后端库，见 [_modelsOf]），不许在这里另立一套；
  ///  3. **不抛**：调用方（[refreshProviders]）不该因为一条 Provider 的目录读不到而失败。
  Future<void> _loadCatalog() async {
    final modelsByProvider = <String, List<AiModel>>{};
    final issues = <String, String>{};
    for (final p in providers) {
      try {
        modelsByProvider[p.id] = await _modelsOf(p);
      } catch (e) {
        issues[p.id] = '$e';
      }
    }
    catalogByProvider = modelsByProvider;
    catalogIssues = issues;
  }

  /// 某条 Provider 的模型目录 —— **真源与 [AiWorkspaceStore._loadModels] 同一个**。
  ///
  /// 单独写一遍就会出"聊天页看得到、选择器看不到"这种自相矛盾（同一个 provider
  /// 的目录在同一台机器上只有一个真源）。
  Future<List<AiModel>> _modelsOf(AiProvider p) async =>
      _isPhone ? _providerStore.listModels(p.id) : _api.listModels(p.id);

  /// 只把**当前 Provider 的模型目录**重新读一次（与 [refreshProviders] 同一个触发点）。
  ///
  /// 为什么要与 [refreshProviders] 分开（用户 bug「对于模型声明，需要重启客户端才能生效」）：
  /// 改的是**模型**的能力声明（补图片输入 / 勾工具 / 调思考档位），provider 行一个字节都不动
  /// —— 那边的"清单变没变"判不出来，于是聊天页手里这份旧目录一直用到重启：
  /// 发消息才被后端拒绝"模型未声明支持图片输入"。
  ///
  /// 代价是一次目录查询（手机本机库 / 桌面一个 GET），换来"打开选择器时目录也是最新的"。
  /// 原来选中的模型按 id 找回来 —— 目录重读后对象是新的，不找回来选择器会显示成空。
  Future<void> refreshModels() async {
    try {
      final rememberedId = selectedModel?.id;
      await _loadModels();
      if (rememberedId != null) {
        selectedModel =
            models.where((m) => m.id == rememberedId).firstOrNull ??
            selectedModel;
        _clampReasoningEffort();
      }
    } catch (e) {
      error = '重读模型目录失败，$e';
    }
    notifyListeners();
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

  /// **预设页 → 工作台**的交接文本（用户 2026-10-11：「▶️运行」= 送到聊天框并跳转过去）。
  ///
  /// 为什么用"待取用的一格"而不是给 `AiHomePage` 加构造参数：
  /// 那个页面是 `const AiHomePage()`（导航列表里的一项，`pages[_index]` 复用同一个实例），
  /// 加参数就得把它改成非 const、还要宿主往下穿回调 —— 而它本来就不该知道"预设"这回事。
  ///
  /// 用法：预设页 `store.handoffToComposer(text)` → 宿主切到工作台 → 工作台挂载后
  /// 在 [`takeHandoff`] 里取走并填进输入框。**取走即清空**（只能被消费一次）。
  String? _pendingComposerText;

  /// 有没有等着被填进输入框的文本（工作台据此决定要不要切过去）。
  bool get hasHandoff => (_pendingComposerText ?? '').isNotEmpty;

  /// 把一段文本交给工作台的输入框。
  void handoffToComposer(String text) {
    final trimmed = text.trim();
    if (trimmed.isEmpty) return;
    _pendingComposerText = trimmed;
    notifyListeners();
  }

  /// 取走交接文本（**取走即清空**：只能消费一次，否则每次切回工作台都会再填一遍）。
  String? takeHandoff() {
    final text = _pendingComposerText;
    _pendingComposerText = null;
    return text;
  }

  /// 输入区草稿：按会话保存。切换会话（以及把空会话删掉）都不会丢没发出去的文字。
  ///
  /// [attachments] **不传**表示"只动文字，附件那半边保持原样"：附件托盘归 Store 管
  /// （切换会话时 [_stashAttachments] 存走、进入时按草稿恢复），页面只负责把输入框
  /// 里的文字落盘。传 `const []` 才是"明确清空附件"。
  Future<void> saveDraft(
    String conversationId,
    String text, {
    List<AiAttachment>? attachments,
  }) async {
    final prefs = await _store;
    final next =
        attachments ??
        _decodeAttachments(
          prefs.getString('$_kDraftAttachPrefix$conversationId'),
        );
    if (text.trim().isEmpty && next.isEmpty) {
      await prefs.remove('$_kDraftTextPrefix$conversationId');
      await prefs.remove('$_kDraftAttachPrefix$conversationId');
      return;
    }
    await prefs.setString('$_kDraftTextPrefix$conversationId', text);
    await prefs.setString(
      '$_kDraftAttachPrefix$conversationId',
      jsonEncode(next.map((a) => a.toJson()).toList()),
    );
  }

  /// 取回草稿；没有就返回空。
  Future<({String text, List<AiAttachment> attachments})> loadDraft(
    String conversationId,
  ) async {
    final prefs = await _store;
    return (
      text: prefs.getString('$_kDraftTextPrefix$conversationId') ?? '',
      attachments: _decodeAttachments(
        prefs.getString('$_kDraftAttachPrefix$conversationId'),
      ),
    );
  }

  /// 草稿里的附件是 JSON 里的一段，解坏了就当没有（不能影响主流程）。
  static List<AiAttachment> _decodeAttachments(String? raw) {
    final out = <AiAttachment>[];
    if (raw == null || raw.isEmpty) return out;
    try {
      final list = jsonDecode(raw);
      if (list is List) {
        for (final e in list.whereType<Map>()) {
          final m = Map<String, dynamic>.from(e);
          out.add(
            AiAttachment(
              // id 要一起存：草稿恢复出来的附件已经在后端了，重发时直接引用，不必再传一次
              id: m['id']?.toString(),
              name: (m['name'] ?? '').toString(),
              modality: m['modality']?.toString(),
              mimeType: (m['mimeType'] ?? 'application/octet-stream')
                  .toString(),
              sizeBytes: (m['sizeBytes'] as num?)?.toInt() ?? 0,
              width: (m['width'] as num?)?.toInt(),
              height: (m['height'] as num?)?.toInt(),
            ),
          );
        }
      }
    } catch (_) {
      // 坏草稿不抛异常
    }
    return out;
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
      // 令牌**不用显式接**：`AiApiClient` 默认就读进程内那一份（[PairingTokenCache]），
      // 于是"某个页面忘了带令牌"这类漏法从根上没有了 —— 2026-09-24 真机验收真踩过
      _cached = AiApiClient(base);
      _cachedFor = base;
    }
    return _cached!;
  }

  /// **Run 与持久化的来源**（M3.2 ②）。
  ///
  /// 手机（Android）上它指向**本地 Kotlin harness**：会话、消息、事件全在手机本地库，
  /// 桌面不在线也照样能聊；其余平台转发给 HTTP 后端。
  ///
  /// ⚠ 分叉只在这一处：`send()` / `_consumeRun()` / `openConversation()` 等**一律走 `_source`**，
  /// 不许再直接调 `_api.startRun` / `_api.runEvents` —— 只换事件流、持久化仍打远端的话，
  /// 离线时是"事件有、会话查不到"，界面一个字都不长，两端还都不报错。
  RunEventSource get _source {
    final injected = _injectedSource;
    if (injected != null) return injected;
    final api = _api;
    // 与 `_api` 一样按后端地址缓存：设置里改了地址要换一个新的来源
    if (_cachedSource == null || !identical(_cachedSourceFor, api)) {
      _cachedSource = openRunEventSource(api: api);
      _cachedSourceFor = api;
    }
    return _cachedSource!;
  }

  /// **凭据的来源**（M6）。
  ///
  /// 与 [_source] 同一个判据、同一个理由：手机上 Run 由本地 harness 执行、会话落在本地库，
  /// 那"这把 key 在不在手上"也必须问**本机** —— 桌面的 key 不下行（总纲 §3.3），
  /// 用桌面的状态显示"已配置"只会让用户点发送然后撞 401。
  CredentialStore get credentials {
    final injected = _injectedCredentials;
    if (injected != null) return injected;
    final api = _api;
    if (_cachedCredentials == null || !identical(_cachedCredentialsFor, api)) {
      _cachedCredentials = openCredentialStore(api: api);
      _cachedCredentialsFor = api;
    }
    return _cachedCredentials!;
  }

  // --- Provider / 模型 ---
  List<AiProvider> providers = const [];
  List<AiModel> models = const [];
  AiProvider? selectedProvider;
  AiModel? selectedModel;

  /// **每条 Provider 各自的模型目录**（`providerId → 模型`）。
  ///
  /// 为什么不能只留 [models]：它只是**当前选中那条**的目录，于是工作台里
  /// "换 Provider"根本无处可去 —— 新建的 Provider 永远选不上（用户 2026-10 报的 bug：
  /// 「新加入的 provider 无法在工作台中选用」）。选择器改成按来源分组列出之后，
  /// 这里就是那一组一组的数据源。
  ///
  /// ⚠ 它是**打开选择器那一刻**才拉的（[refreshProviders]），不是冷启动就拉：
  /// 冷启动就为每条 Provider 挨个发一次目录请求，启动会无谓地慢下来。
  Map<String, List<AiModel>> catalogByProvider = const {};

  /// 某条 Provider 的目录读失败时的原因（`providerId → 人话`）。
  ///
  /// 单独一个字段而不是并进 [error]：一条 Provider 挂了不该让整页都变成错误态，
  /// 但界面上必须**如实标出是哪一条**（否则用户会以为"它一个模型都没有"）。
  Map<String, String> catalogIssues = const {};

  // --- 会话 / 消息 ---
  List<AiConversation> conversations = const [];
  AiConversation? conversation;
  List<AiMessage> messages = const [];

  // --- Skills（M5） ------------------------------------------------------
  /// 后端扫出来的 skills（磁盘是真源，**不做本地缓存**：改完立刻重拉）。
  List<AiSkill> skills = const [];
  String? skillsError;

  /// skills 投放口的位置（把 skill 拷进这个文件夹就算装好）。
  AiSkillRoots? skillRoots;

  /// 刷新 / 删除 / 导入进行中：界面据此显示进度并防重复点击。
  bool skillsBusy = false;

  // --- 长期记忆（M6） ----------------------------------------------------
  /// 跨对话保留的用户偏好 / 约定；每次 Run 由后端注入系统提示。
  AiMemory memory = const AiMemory();
  String? memoryError;
  bool memoryBusy = false;

  // --- 工具清单（M4） ----------------------------------------------------
  /// 只用来在工具卡上显示分类图标；拿不到不影响聊天。
  List<AiToolInfo> tools = const [];

  // --- 权限档（用户建议 ⑤） ----------------------------------------------
  /// `ask`（默认，需要审批的工具要用户点批准）/ `full`（自动允许，AI 无需批准）。
  ///
  /// 真源在后端（`ai.tools.policy`，每次 Run 现读），这里只是界面上的镜像：
  /// 切档写回后端后，**下一次回复立刻生效**（后端会把那段规则即时注入系统提示）。
  String permissionMode = AiToolPolicy.modeAsk;

  bool get fullPermission => permissionMode == AiToolPolicy.modeFull;

  /// 当前权限档的界面名字（用户建议 ①，2026-09-20：正在生成的回复气泡里要能看见它）。
  String get permissionLabel =>
      fullPermission ? AiToolPolicy.modeFullLabel : AiToolPolicy.modeAskLabel;

  /// 正在切换权限档：按钮据此禁用，避免连点。
  bool permissionBusy = false;

  // --- 输入区 ---
  final List<AiAttachment> attachments = [];

  /// 附件正在上传（M3）：期间禁止发送，托盘显示进度。
  bool uploadingAttachments = false;

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

  /// **正在流式生成的那一条助手消息**（用户建议 2026-09-20）：
  /// 「AI 正在生成」的转圈动画要画在**它自己那个气泡的底部**，而不是页脚浮层。
  ///
  /// 两个条件缺一不可：
  ///   · `sending` —— **这一轮还活着**。光看消息状态是不够的：库里可能留着一条
  ///     `streaming` 的旧回复（流中断、App 被杀），只按状态画就会让那条老气泡
  ///     永远转圈，而它其实早就没人推了；
  ///   · 列表里最后一条助手消息状态是 `streaming` —— 正在写的就是它。
  ///
  /// 不另存一个 id 字段：那样就多了一份可能与 `messages` 不一致的状态，
  /// 而重试（助手消息 id 会换一条）正好是容易忘记同步的地方。
  AiMessage? get streamingMessage {
    if (!sending) return null;
    for (var i = messages.length - 1; i >= 0; i--) {
      final m = messages[i];
      if (m.isUser) continue;
      if (m.status == 'streaming') return m;
      // 末尾的助手消息已经是终态，说明这一轮没有在飞的了
      return null;
    }
    return null;
  }

  // -----------------------------------------------------------------------
  //  「AI 正在等用户回答」的那条提问（用户 bug 2026-09-21）
  // -----------------------------------------------------------------------

  /// 提问接管输入区时那个"自己输入答案"文本框的焦点。
  ///
  /// 放在 Store 上是为了让**气泡里的提问卡**能说"去下面那个框里打字"：
  /// 卡片与输入区隔着好几层组件，回调传下去会污染一大串构造参数，
  /// 而焦点本身是"这一轮提问"的状态，本来就属于 Store。
  final FocusNode pendingQuestionFocus = FocusNode(
    debugLabel: 'ask-user-question',
  );

  /// **当前正等着用户回答的那条提问**（没有就返回 null）。
  ///
  /// 输入区靠它决定"是被提问接管，还是普通输入框"（用户 bug 2026-09-21：
  /// "在调用 ask_user_question 时，将原来聊天详情下部的模块替换（输入框等）"）。
  ///
  /// "还在等"的判据 = 有提问 && 还没 `answered`。**不看 Run 状态**：
  /// 提问闸门是挂在 Run 里的（Run 还没结束），而 `sending` 只表示这次 Run 活着。
  AiQuestion? get pendingQuestion {
    AiQuestion? best;
    void consider(AiToolCallState call, {required bool byName}) {
      // 名字只在流式那一条路上可靠（`tool.requested` 给的名字），历史消息里可能缺失 ——
      // 所以第二遍兜底时**不看名字**，只看"有没有一条还没回答的提问"。
      if (byName && call.name != 'ask_user_question') return;
      for (final q in call.allQuestions) {
        if (q.answered) continue;
        // 同一张卡里取**题号最大**的那条（= 最后收到的一题）
        if (best == null || q.index >= best!.index) best = q;
      }
    }

    for (final list in _liveToolCalls.values) {
      for (final call in list) {
        consider(call, byName: true);
      }
    }
    if (best != null) return best;
    for (final m in messages) {
      for (final call in AiToolCallState.listFromParts(m.parts)) {
        consider(call, byName: true);
      }
    }
    if (best != null) return best;
    // 兜底：**不看名字**，只看"有没有一条还没回答的提问"。
    // 而"有没有问题"这个事实本身就够了（用户 bug 2026-09-21：输入区不接管）。
    for (final list in _liveToolCalls.values) {
      for (final call in list) {
        consider(call, byName: false);
      }
    }
    if (best != null) return best;
    for (final m in messages) {
      for (final call in AiToolCallState.listFromParts(m.parts)) {
        consider(call, byName: false);
      }
    }
    return null;
  }

  /// **整张提问卡**：与 [pendingQuestion] 同一组（同 `groupId`）的所有题，按题号排。
  ///
  /// 多问改造（2026-09-22）之后，一次 `ask_user_question` 会生成 N 条同组的提问，
  /// 后端的 `user.question` 也是**一题一条事件** —— 界面必须把这几条一起画成
  /// "一张卡分 N 页"，否则第 2 题起就看不见，而模型那边要等**整组**答完（Run 挂到超时）。
  List<AiQuestion> get pendingQuestions {
    final one = pendingQuestion;
    if (one == null) return const [];
    List<AiQuestion> groupOf(AiToolCallState call) =>
        call.allQuestions.where((q) => q.key == one.key).toList();
    for (final list in _liveToolCalls.values) {
      for (final call in list) {
        final g = groupOf(call);
        if (g.isNotEmpty) return g;
      }
    }
    for (final m in messages) {
      for (final call in AiToolCallState.listFromParts(m.parts)) {
        final g = groupOf(call);
        if (g.isNotEmpty) return g;
      }
    }
    return [one];
  }

  // -----------------------------------------------------------------------
  //  流式期间的刷新节流（用户 bug：聊天快速滑动 / 展开流式思考时卡顿）
  // -----------------------------------------------------------------------

  /// 流式增量攒下的"待通知"标记与定时器（见 [_notifyStreaming]）。
  bool _streamDirty = false;
  Timer? _streamFlush;

  /// 一帧最多通知一次（约 16ms）。
  ///
  /// 上游逐字回包时**每一个字**都会触发一次状态变化，直接 `notifyListeners()` 等于
  /// 每秒重建几十次界面：滚动条跟着跳、Markdown 跟着重排、长回复越流越卡。
  /// 攒到下一帧再通知，肉眼完全看不出差别（60fps 就是 16ms），
  /// 但重建次数从"每个字一次"降到"每帧一次"。
  void _notifyStreaming() {
    _streamDirty = true;
    _streamFlush ??= Timer(const Duration(milliseconds: 16), () {
      _streamFlush = null;
      if (!_streamDirty) return;
      _streamDirty = false;
      notifyListeners();
    });
  }

  /// 把攒着的通知丢掉、立刻通知一次。
  ///
  /// 用在"必须当场看到"的时刻（消息完成、工具卡状态变化、Run 结束）：
  /// 既不能让这些变化等下一个 16ms，也不能让攒着的那次通知再补发一次。
  void _notifyNow() {
    _streamFlush?.cancel();
    _streamFlush = null;
    _streamDirty = false;
    notifyListeners();
  }

  /// 每条消息**流式实时状态**的版本号（思考增量 / 工具卡状态）。
  ///
  /// 消息列表按"这条 `AiMessage` 实例有没有变"来跳过重建（见 `_MessageBubbleSlot`），
  /// 而思考增量、工具卡状态**不改 `AiMessage` 实例**（它们挂在实时状态上）。
  /// 有这个版本号，那两类变化才不会被误判成"没变"。
  ///
  /// 刻意做成**按消息**的：全局计数器会让"正在流式的那一条"每来一个字就把
  /// 一屏历史气泡全部标脏 —— 那就等于没省。`_liveRevisionOf()` 只看这一条。
  final Map<String, int> _liveRevisions = {};

  /// 这条消息的实时状态版本（没有实时状态就是 0）。
  int liveRevisionOf(String messageId) => _liveRevisions[messageId] ?? 0;

  void _bumpLiveRevision(String messageId) {
    _liveRevisions[messageId] = (_liveRevisions[messageId] ?? 0) + 1;
  }

  /// 当前还没接通的能力，用**明确的话**告诉用户，而不是假装成功。
  String? notice;

  bool get hasProvider => selectedProvider != null;

  /// 发送按钮是否可用（只看"状态"，不看正文）。
  bool get canSend => sendBlockReason() == null;

  /// 这一次发送会不会被拒：可以发返回 `null`，否则返回**为什么不能发**。
  ///
  /// 这是准入判断的**唯一真源**（[send] 自己也用它），而且必须是**同步**的：
  /// 页面得在"清空输入框"之前就知道能不能发 —— 以前是"先清空、再由 send() 拒绝"，
  /// 于是最常见的"正在生成中又按了一次回车"会把用户刚打的字**静默吃掉**
  /// （用户报的 bug：只能自己把上一条复制一遍再发一次）。被拒时一个字节都不能动。
  String? sendBlockReason() {
    if (sending) return '正在生成中。先点「停止」，或等这一轮结束再发。你打的字还在输入框里。';
    if (uploadingAttachments) return '附件还在上传，等它传完再发（免得只发出去一半）。';
    // 手机：规则工件（系统提示 / 工具描述 / 安全横幅）只有桌面能产出（R13）。
    // 一份都没有时**不许开跑** —— 否则模型会拿着一套瞎编的规则回答（R13 的另一面）。
    if (_source.isLocal && kitIssue != null) {
      return '本机还没有可用的规则工件（run-kit）。$kitIssue';
    }
    if (!(preflight?.allowed ?? true)) {
      return '有附件未通过准入，已阻断发送。${preflight!.blockers.join('。')}';
    }
    if (selectedProvider == null || selectedModel == null) {
      return '请先选择 Provider 与模型（设置 → AI 模型）';
    }
    // 还有没上传成功的附件时不给发：否则用户以为图发出去了，其实只发了文字
    if (!attachments.every((a) => a.hasId)) {
      return '有附件还没上传成功，已阻断发送。请移除它们或重新选择文件。';
    }
    return null;
  }

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
      // 投放口路径只问一次（它是后端算出来的绝对路径，运行期不会变）
      skillRoots ??= await _api.skillRoots();
      skills = await _api.listSkills();
    } catch (e) {
      skillsError = '$e';
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 重新扫描投放口：**给"拷进来但没写 frontmatter"的 skill 自动补上并登记**，
  /// 然后刷新列表。用户不用重启应用，也不用点任何"导入"。
  Future<({bool ok, String message})> rescanSkills() async {
    skillsBusy = true;
    notifyListeners();
    try {
      final result = await _api.rescanSkills();
      skills = result.skills;
      skillsError = null;
      return (ok: result.errors.isEmpty, message: result.summary);
    } catch (e) {
      return (ok: false, message: '重新扫描失败，$e');
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
      final deleted = await _api.deleteSkill(name);
      if (!deleted) {
        return (ok: false, message: '没有删掉「$name」，它可能已经不在了');
      }
      skills = await _api.listSkills();
      skillsError = null;
      return (ok: true, message: '已删除 skill「$name」');
    } catch (e) {
      return (ok: false, message: '删除「$name」失败，$e');
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 停用 / 启用一个 skill（内置与用户都允许）。停用只影响对话，文件不动。
  Future<({bool ok, String message})> setSkillEnabled(
    String name,
    bool enabled,
  ) async {
    skillsBusy = true;
    notifyListeners();
    try {
      final updated = await _api.setSkillEnabled(name, enabled);
      skills = [
        for (final skill in skills) skill.name == updated.name ? updated : skill,
      ];
      skillsError = null;
      return (
        ok: true,
        message: enabled ? '已启用 skill「$name」' : '已停用 skill「$name」',
      );
    } catch (e) {
      return (ok: false, message: '${enabled ? "启用" : "停用"}「$name」失败，$e');
    } finally {
      skillsBusy = false;
      notifyListeners();
    }
  }

  /// 长期记忆（M6）：读回来给右侧栏显示 / 编辑。
  ///
  /// [silent] 是给"AI 调用了 `remember`"这条路的（用户 bug：更新长期记忆之后右侧栏不刷新）：
  /// 它发生在一次 Run 的中间，右侧栏的「长期记忆」面板必须当场跟上条数，
  /// 但**不能**顺手把编辑按钮换成转圈（那是用户自己点编辑时的反馈，见 [memoryBusy]）。
  /// 失败同样静默 —— 这条路径是"顺带对齐一下"，失败不该盖掉面板上原有的错误说明。
  Future<void> loadMemory({bool silent = false}) async {
    if (!silent) {
      memoryBusy = true;
      notifyListeners();
    }
    try {
      memory = await _api.memory();
      memoryError = null;
      if (silent) notifyListeners();
    } catch (e) {
      if (!silent) memoryError = '$e';
    } finally {
      if (!silent) {
        memoryBusy = false;
        notifyListeners();
      }
    }
  }

  /// 整篇保存（用户在编辑器里改完）。
  Future<({bool ok, String message})> saveMemory(String content) async {
    memoryBusy = true;
    notifyListeners();
    try {
      memory = await _api.saveMemory(content);
      memoryError = null;
      return (ok: true, message: '长期记忆已保存（${memory.entryCount} 条）');
    } catch (e) {
      memoryError = '$e';
      return (ok: false, message: '保存失败，$e');
    } finally {
      memoryBusy = false;
      notifyListeners();
    }
  }

  /// 追加一条。
  Future<({bool ok, String message})> addMemory(String content) async {
    memoryBusy = true;
    notifyListeners();
    try {
      memory = await _api.appendMemory(content);
      memoryError = null;
      return (ok: true, message: '已添加（共 ${memory.entryCount} 条）');
    } catch (e) {
      memoryError = '$e';
      return (ok: false, message: '添加失败，$e');
    } finally {
      memoryBusy = false;
      notifyListeners();
    }
  }

  /// 按下标删除几条（界面的单条 / 批量删除）。
  ///
  /// 下标由界面从 `memory.entries` 里选中给出，与后端的行序同源。
  Future<({bool ok, String message})> deleteMemoryEntries(
    List<int> indices,
  ) async {
    if (indices.isEmpty) return (ok: true, message: '没有选中任何一条');
    memoryBusy = true;
    notifyListeners();
    try {
      memory = await _api.deleteMemoryEntries(indices);
      memoryError = null;
      return (
        ok: true,
        message: '已删除 ${indices.length} 条（剩 ${memory.entryCount} 条）',
      );
    } catch (e) {
      memoryError = '$e';
      return (ok: false, message: '删除失败，$e');
    } finally {
      memoryBusy = false;
      notifyListeners();
    }
  }

  /// 清空（不可逆，界面必须先确认）。
  Future<({bool ok, String message})> clearMemory() async {
    memoryBusy = true;
    notifyListeners();
    try {
      memory = await _api.clearMemory();
      memoryError = null;
      return (ok: true, message: '长期记忆已清空');
    } catch (e) {
      memoryError = '$e';
      return (ok: false, message: '清空失败，$e');
    } finally {
      memoryBusy = false;
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
      return (ok: false, message: '注册失败，$e');
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

  /// 读一次权限档（真源在后端策略里）。失败静默：拿不到就按最保守的「询问」显示。
  Future<void> loadPermissionMode() async {
    try {
      final policy = await _api.toolPolicy();
      permissionMode = policy.permissionMode;
      notifyListeners();
    } catch (_) {
      // 后端不可用：保持当前显示，发送时后端自己还有一道判定
    }
  }

  /// 切换权限档（用户建议 ⑤：询问 / 自动允许（无需批准））。
  ///
  /// **先写后端再改界面**：这是权限，不能"界面看着切了、后端其实没切"。
  /// 是**全局**设置（存在 `ai.tools.policy` 里），切一次对之后所有对话都有效；
  /// 后端每次 Run 现读策略，所以下一次回复立刻按新档位走（系统提示也即时注入）。
  Future<void> setPermissionMode(String mode) async {
    if (mode == permissionMode || permissionBusy) return;
    permissionBusy = true;
    notifyListeners();
    try {
      final policy = await _api.updateToolPolicy({'permissionMode': mode});
      permissionMode = policy.permissionMode;
      if (policy.permissionMode != mode) {
        // 后端没接受这一档：旧后端（响应里根本没有 permissionMode 字段）会静默回落
        // 成「询问」，界面上看着就是"点了没反应"（用户 bug ①）。这里如实说出来，
        // 而不是让用户一直点。
        notice =
            '切换权限失败，后端没有接受「${mode == AiToolPolicy.modeFull ? AiToolPolicy.modeFullLabel : AiToolPolicy.modeAskLabel}」'
            '（当前仍是「${policy.permissionMode == AiToolPolicy.modeFull ? AiToolPolicy.modeFullLabel : AiToolPolicy.modeAskLabel}」）。'
            '多半是后端还是旧构建 —— 在「设置 → 本地服务」里重启一次本地服务即可。';
        return;
      }
      notice = policy.permissionMode == AiToolPolicy.modeFull
          ? '已切换到「${AiToolPolicy.modeFullLabel}」，AI 调用工具不再等你批准（可读 / 可写目录范围不变，越界仍会被拒）。'
          : '已切换到「${AiToolPolicy.modeAskLabel}」，写盘 / 提交这类改动操作会先等你批准。';
    } catch (e) {
      notice = '切换权限失败，$e';
    } finally {
      permissionBusy = false;
      notifyListeners();
    }
  }

  /// 工具名 → 分类（skill / files / comfy / memory）。先查后端给的清单，查不到按名字兜底。
  String toolCategoryOf(String name) {
    for (final t in tools) {
      if (t.name == name) return t.category;
    }
    if (name.contains('skill')) return 'skill';
    if (name == 'remember' || name.contains('memory')) return 'memory';
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
  Future<void> approveToolCall(String callId) =>
      _resolveToolCall(callId, approve: true);

  // -----------------------------------------------------------------------
  //  ComfyUI 实时进度（用户"其他建议"第 1 条）
  // -----------------------------------------------------------------------

  /// 最近一次拿到的 ComfyUI 实时进度。右侧栏显示它。
  AiComfyJobs comfyJobs = AiComfyJobs.empty;

  /// 最近一次拉实时进度**失败**的原因（成功为 null）。
  ///
  /// 右侧栏靠它区分两件事：**ComfyUI 没开**（`comfyJobs.comfyReachable == false`，
  /// 是后端连得上、ComfyUI 连不上）与**连后端都拉不到**（这里是非 null）。
  /// 以前失败被 `catch (_)` 吞掉，界面只剩"不可达 / 当前没有任务"，两种毛病看起来一样。
  String? comfyJobsError;

  Timer? _jobsTimer;
  bool _jobsInFlight = false;

  /// 开始轮询实时进度（页面开始显示时调）。
  ///
  /// 节奏是"有活动时 1.5 秒一次、闲着时 6 秒一次"：闲着还勤问 ComfyUI，
  /// 既费电又会让 ComfyUI 的请求日志刷屏；但没有活动时**不能完全停**，
  /// 否则用户在 ComfyUI 里手点一下就看不到进度了。
  void startWatchingComfy() {
    _jobsTimer?.cancel();
    _jobsTimer = Timer.periodic(
      comfyJobs.hasActivity
          ? const Duration(milliseconds: 1500)
          : const Duration(seconds: 6),
      (_) => unawaited(refreshComfyJobs()),
    );
    unawaited(refreshComfyJobs());
  }

  void stopWatchingComfy() {
    _jobsTimer?.cancel();
    _jobsTimer = null;
  }

  /// 拉一次实时进度。失败**不弹错**（进度是辅助信息，不该因为 ComfyUI 没开就骚扰用户），
  /// 但原因要留在 [comfyJobsError] 里 —— 界面据此说"拉不到"，不许装成"没有任务"。
  Future<void> refreshComfyJobs() async {
    if (_jobsInFlight) return;
    _jobsInFlight = true;
    try {
      final next = await _api.comfyJobs();
      // 活动状态变了就重建定时器（空闲 ↔ 有任务 的节奏不一样）
      final wasActive = comfyJobs.hasActivity;
      comfyJobs = next;
      comfyJobsError = null;
      notifyListeners();
      if (wasActive != next.hasActivity) startWatchingComfy();
    } catch (e) {
      // 静默不弹错，但**不能把"拉不到"当成"没有活动"**：原因记下来给界面用。
      final message = '$e';
      if (comfyJobsError != message) {
        comfyJobsError = message;
        debugPrint('[comfy] 拉实时进度失败：$message');
        notifyListeners();
      }
    } finally {
      _jobsInFlight = false;
    }
  }

  // -----------------------------------------------------------------------
  //  跨端待审批（M5）：手机排上来的提交也在这一张卡上等桌面点批准
  // -----------------------------------------------------------------------

  /// 最近一次拿到的待批总览（`GET /api/ai/pending`）。
  ///
  /// 里面装的是**两类同形的东西**：桌面 Run 的待批工具调用，以及**手机排队的提交**
  /// （`callId` 形如 `submit:<queueId>`，契约 §4.1）。后者不在任何本机 Run 的事件流里，
  /// 所以只能靠这个接口让它露面 —— 否则"手机排的队"在桌面上看不见、点不了。
  AiPendingOverview pendingOverview = AiPendingOverview.empty;

  /// 拉待批清单失败的原因（成功为 null）。
  ///
  /// 界面**如实显示一行**，绝不把"拉不到"装成"没有待批"——那两句对用户的意思完全相反。
  String? pendingError;

  Timer? _pendingTimer;
  bool _pendingInFlight = false;

  /// 这台设备是不是"手机那一端"（本机 harness + 本机密钥库）。
  ///
  /// 审批一律归桌面（用户拍板："有关工作流的交给桌面审批处理"），所以界面靠它决定
  /// **画不画那张待批卡**、要不要白轮询。
  bool get isLocal => _source.isLocal;

  /// 开始轮询待批清单（右侧栏开始显示时调；与 [startWatchingComfy] 同一条纪律）。
  ///
  /// 节奏是固定的 5 秒一次：这里没有"活动与否"可看，而"有人等你点批准"这件事
  /// 慢一秒发现都算慢 —— 但也不能更勤，右侧栏一开着就每秒问后端一次是纯浪费。
  void startWatchingPending() {
    _pendingTimer?.cancel();
    _pendingTimer = Timer.periodic(
      const Duration(seconds: 5),
      (_) => unawaited(refreshPending()),
    );
    unawaited(refreshPending());
  }

  void stopWatchingPending() {
    _pendingTimer?.cancel();
    _pendingTimer = null;
  }

  /// 拉一次待批清单。失败**只记在 [pendingError]**：不弹错、也不清空上一次拿到的列表
  /// （清空就等于告诉用户"没人等你"，那是假话）。
  Future<void> refreshPending() async {
    if (_pendingInFlight) return;
    _pendingInFlight = true;
    try {
      pendingOverview = await _api.pending();
      pendingError = null;
    } catch (e) {
      pendingError = '$e';
    } finally {
      _pendingInFlight = false;
      notifyListeners();
    }
  }

  /// 批准 / 拒绝一条待批（工具调用与手机排队的提交**同一条通道**）。
  ///
  /// 返回的 `accepted` 是后端给的实话：`false` = 这一条已经结束 / 超时 / 早被人点过，
  /// 界面**不许假装成功**（更不许把那行去掉）。
  ///
  /// 与 [approveToolCall] 的差别只在"这条不在本机任何 Run 的事件流里"：
  /// 所以这里不 patch 气泡里的工具卡，而是**就地更新这张跨端列表**。
  Future<({bool accepted, String message})> resolvePendingApproval(
    String callId, {
    required bool approve,
  }) async {
    try {
      final accepted = approve
          ? await _api.approveToolCall(callId)
          : await _api.denyToolCall(callId);
      if (!accepted) {
        return (
          accepted: false,
          message: '桌面没有接受这一条${approve ? '批准' : '拒绝'}，它可能已经结束或超时了，可以等下一次刷新确认。',
        );
      }
      // 被接受了：后端那一刻确实把它移出了待办，就地摘掉（不靠下一次轮询刚好拉回来）。
      pendingOverview = pendingOverview.withoutApproval(callId);
      notifyListeners();
      // 再拉一次与后端对齐：成功也要拉 —— 别的设备可能同时又排了新的。
      unawaited(refreshPending());
      // 说人话要分两类（同一张卡上两类的"批准"含义不同）：
      // 排队提交走 R10 那条全链路，**跑完的产物不在这一句里许诺**（结果回发起那条会话）；
      // 别的工具调用则是"模型接着往下做"。
      final queued = callId.startsWith('submit:');
      return (
        accepted: true,
        message: approve
            ? (queued
                  ? '已批准，桌面会走 comfy_submit 那条链路真提交，跑完产物进画廊、结果回那条会话。'
                  : '已批准这次工具调用，模型会继续执行。')
            : (queued ? '已拒绝这一条排队提交，桌面不会提交它。' : '已拒绝这次工具调用。'),
      );
    } catch (e) {
      return (accepted: false, message: '${approve ? '批准' : '拒绝'}失败，$e');
    }
  }

  @override
  void dispose() {
    stopWatchingComfy();
    stopWatchingPending();
    // 攒着的流式通知定时器要一起收掉，否则页面都销毁了还会被回调
    _streamFlush?.cancel();
    _streamFlush = null;
    // 上行重试与生命周期钩子同理：Store 都没了就不该再往外发东西
    _cancelOutboxRetry();
    _lifecycle?.dispose();
    _lifecycle = null;
    pendingQuestionFocus.dispose();
    super.dispose();
  }

  /// 一条助手消息里**真正产出的画廊产物**（用于回复末尾的「画廊入口卡」）。
  ///
  /// 只认工具结构化结果里的 `mediaIds`：别的路径（比如模型在正文里说"生成了 id=7"）
  /// 一律不信 —— 产物入口必须来自后端确认入库的事实。
  List<int> producedMediaIds(AiMessage message) {
    final seen = <int>{};
    for (final call in toolCallsFor(message)) {
      for (final id in call.mediaIds) {
        if (id > 0) seen.add(id);
      }
    }
    return seen.toList();
  }

  /// 一条助手消息里**真正入库的提示词**（= 生成这些产物用的那份工作流）。
  ///
  /// 用户建议 ①："「生成的产物」包括生成的工作流" —— 所以回复末尾那张卡除了缩略图，
  /// 还要能直接打开这次真正跑过的工作流（而不是只给几张结果图）。
  /// 同样只认后端确认的 `capturedPromptId`，不解析正文。
  List<int> producedPromptIds(AiMessage message) {
    final seen = <int>{};
    for (final call in toolCallsFor(message)) {
      final id = call.promptId;
      if (id != null && id > 0) seen.add(id);
    }
    return seen.toList();
  }

  /// 一条助手消息里**被标记为成人内容**的产物（用户 bug 2026-09-20）。
  ///
  /// 与 [producedMediaIds] 同源（工具结构化结果），只是这一份只含
  /// `comfy_submit` 明说过 `nsfw=true` 的那几张：回复里的产物卡只糊这些，
  /// 其余正常内容一张都不糊 —— 以前是"开关一开全糊"。
  List<int> producedNsfwMediaIds(AiMessage message) {
    final seen = <int>{};
    for (final call in toolCallsFor(message)) {
      for (final id in call.nsfwMediaIds) {
        if (id > 0) seen.add(id);
      }
    }
    return seen.toList();
  }

  /// 一条助手消息里 `layout_propose` 给出的候选布局（2026-11-03）。
  ///
  /// 与 [producedMediaIds] 同一条纪律：**只认工具结构化结果**，不解析正文。
  /// 按到达顺序去重（同一个标题 + 同一组框只留一份，模型偶尔会重复调一次）。
  List<LayoutCandidate> layoutCandidates(AiMessage message) {
    final seen = <String>{};
    final out = <LayoutCandidate>[];
    for (final call in toolCallsFor(message)) {
      for (final candidate in call.layoutCandidates) {
        final key =
            '${candidate.title}|${candidate.spec.regions.length}|'
            '${candidate.spec.toJson()}';
        if (seen.add(key)) out.add(candidate);
      }
    }
    return out;
  }

  Future<void> denyToolCall(String callId) =>
      _resolveToolCall(callId, approve: false);

  /// 回答 AI 的提问（用户建议 ⑥）。
  ///
  /// **不新建消息、也不改 Run 状态**：后端把这段文字当工具结果喂回模型，
  /// 同一个 Run 接着往下做 —— 这正是用户要的"回答结束时不中断当前对话"。
  ///
  /// [optionIndex] 与 [text] 至少给一个；点了按钮就用前者（后端会把它翻成选项文字）。
  Future<({bool ok, String message})> answerQuestion(
    String questionId, {
    int? optionIndex,
    String? text,
  }) async {
    try {
      final accepted = await _source.answerQuestion(
        questionId,
        text,
        optionIndex: optionIndex,
      );
      if (!accepted) {
        // 提问已经结束（超时 / Run 停了）：如实说，不能假装收到
        return (ok: false, message: '这条提问已经结束了（超时或本次回答已停止），这次回答没有被采纳。');
      }
      // 本地立刻切成"已回答"：不等 SSE 回来（它也会来，但用户点完就该有反馈）。
      // 按 **callId** 定位（提问卡挂在模型那条调用上，见 `user.question` 的注释）；
      // 拿不到 callId（历史消息里答过了）就退回 questionId。
      _patchToolCall(
        _liveAskCallIdHere() ?? questionId,
        (call) => _applyLocalAnswer(
          call,
          questionId,
          answer:
              text ??
              (optionIndex != null
                  ? call.question?.options.elementAtOrNull(optionIndex)
                  : null),
          optionIndex: optionIndex,
        ),
      );
      notifyListeners();
      return (ok: true, message: '已回答');
    } catch (e) {
      return (ok: false, message: '回答失败，$e');
    }
  }

  /// 跳过提问：后端当作"用户没有回答"放行，模型基于已有信息继续。
  Future<({bool ok, String message})> skipQuestion(String questionId) async {
    try {
      await _api.skipQuestion(questionId);
      _patchToolCall(
        _liveAskCallIdHere() ?? questionId,
        (call) => _applyLocalAnswer(call, questionId, answer: ''),
      );
      notifyListeners();
      return (ok: true, message: '已跳过');
    } catch (e) {
      return (ok: false, message: '跳过失败，$e');
    }
  }

  /// 本地把某**一题**标成"已回答"（同一张卡里别的题不动）。
  ///
  /// 多问改造（2026-09-22）：一张卡有 N 题，必须先全部答完 / 跳过才收卡片 ——
  /// 所以整卡的 `status` 只在**最后一题**落定后才变 `ok`；提前变会让输入区
  /// 在还剩几题没答的时候就让位给普通输入框（模型那边可还在等）。
  static AiToolCallState _applyLocalAnswer(
    AiToolCallState call,
    String questionId, {
    required String? answer,
    int? optionIndex,
  }) {
    final group = call.allQuestions;
    if (group.isEmpty) return call;
    AiQuestion mark(AiQuestion q) => q.questionId == questionId
        ? q.copyWith(answered: true, answer: answer, optionIndex: optionIndex)
        : q;
    final next = group.map(mark).toList();
    final allDone = next.every((q) => q.answered);
    return call.copyWith(
      questions: next,
      question: mark(call.question ?? next.first),
      status: allDone ? AiToolCallStatus.ok : call.status,
    );
  }

  /// 这个 `ask_user_question` 调用**还在等用户回答**吗。
  ///
  /// ⚠ 空 `questions` 也算"还在等"：`tool.requested` 先到、问题后到，那段窗口里
  /// 卡片上还没有任何题 —— 把它算成"答完了"会让提问卡挂到 `questionId` 上另开一条
  /// （用户 bug 2026-09-21 的根因，就这么复发的）。
  static bool _askPending(AiToolCallState call) {
    if (call.name != 'ask_user_question') return false;
    final qs = call.allQuestions;
    return qs.isEmpty || qs.any((q) => !q.answered);
  }

  /// 当前所有消息里那个"还在等回答"的 `ask_user_question` 调用号。
  ///
  /// [answerQuestion] / [skipQuestion] 直接按它给气泡里那张卡切状态：
  /// 提问事件带的 `questionId` 与工具调用的 `callId` 是两个值，混用会切不中。
  String? _liveAskCallIdHere() {
    for (final list in _liveToolCalls.values) {
      for (final call in list) {
        if (_askPending(call)) return call.callId;
      }
    }
    return null;
  }

  Future<void> _resolveToolCall(String callId, {required bool approve}) async {
    try {
      final accepted = approve
          ? await _api.approveToolCall(callId)
          : await _api.denyToolCall(callId);
      if (!accepted) {
        // 点晚了：这次调用已经结束 / 超时，**如实说**，不要假装批准成功
        notice = '这次工具调用已经结束或超时，${approve ? '批准' : '拒绝'}没有生效。';
      } else {
        _patchToolCall(
          callId,
          (c) => c.copyWith(
            status: approve
                ? AiToolCallStatus.running
                : AiToolCallStatus.denied,
            approval: approve ? c.approval : 'denied',
          ),
        );
        notice = approve ? '已批准这次工具调用，模型会继续执行。' : '已拒绝这次工具调用。';
      }
    } catch (e) {
      notice = '${approve ? '批准' : '拒绝'}失败，$e';
    }
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  加载
  // -----------------------------------------------------------------------

  /// 冷启动加载：**一个 Store 只跑一次**（重复调用直接返回同一个 Future）。
  ///
  /// 为什么必须去重：`HomeShell` 每次切页都会重建 `AiHomePage`（不是 IndexedStack），
  /// 页面 `didChangeDependencies` 里那句 `load()` 于是会跟着跑第二遍 —— 而加载流程里
  /// 会新建会话，用户看到的就是"打开 ComfyHub 冒出好几条新对话"（用户报的 bug ①）。
  bool _loaded = false;
  Future<void>? _loading;

  /// 是否已经成功加载过一次。
  bool get loaded => _loaded;

  Future<void> load() {
    if (_loaded) return Future<void>.value();
    return _loading ??= _doLoad();
  }

  Future<void> _doLoad() async {
    loading = true;
    error = null;
    notifyListeners();
    try {
      // **先等本地服务有个结论**（见 [backendReady] / `BackendLauncher.settled`）：
      // 冷启动时 `.run\api.json` 还没写出来，这一刻 `baseUrl` 是 8080 兜底值，
      // 发出去的请求注定失败 —— 而后端 1 秒后就绪，用户看到的就是一句莫名其妙的
      // "连接后端失败，ClientException … 127.0.0.1:8080/api/ai/providers"。
      await backendReady?.call();
      // 先尽力把配对令牌读进内存（手机专有）：下面 ensureRunKit / syncOutbox 都要带它。
      // ⚠ 必须 **await**：不等它回来就往下走，`ensureRunKit` / `syncOutbox` 拿到的还是
      // null —— 那就等于这个洞没补（真的踩过：`unawaited` 与"令牌恒 null"是同一个现象）。
      // 它内部吞掉所有异常，所以这一步绝不会拦住启动。
      await _loadPairingToken();
      // 冷启动的"东西从哪来"分两种（M3.2 ③）：
      //   桌面：会话在**后端库**里 → 后端连不上就如实报"连接后端失败"（不能自己编数据）；
      //   手机：会话在**本地库**里 → 桌面不在线也必须能进聊天页。
      // 以前这里不分叉，第一条就是 `GET /api/ai/providers`，于是手机上桌面一离线
      // 整页就是"连接后端失败" —— 手里明明有历史会话，却一个字都看不到。
      if (_source.isLocal) {
        await _loadLocalStartup();
      } else {
        providers = await _api.listProviders();
        await _restoreRememberedModel();
        conversations = await _api.listConversations();
        await _startFreshConversation();
      }
      _loaded = true;
    } catch (e) {
      error = '连接后端失败，$e';
      // 失败不算"加载过"：下次进页面还能重试
      _loading = null;
    } finally {
      loading = false;
      notifyListeners();
    }
    if (!_loaded) return;
    // skills / 工具清单 / 长期记忆走"尽力而为"：拉不到不影响聊天（各自把失败记在自己的字段里）
    await reloadSkills();
    await reloadTools();
    await loadMemory();
    await loadPermissionMode();
  }

  /// 手机（本地 harness）的冷启动。
  ///
  /// **会话与消息只认本地库**，所以这一段不需要桌面在场；provider / 模型清单是
  /// 桌面版本化下发的工件（M6 才真正下行），拿不到时**只把这件事说出来**，
  /// 绝不让整页起不来 —— 用户至少还能翻历史会话。
  Future<void> _loadLocalStartup() async {
    // 装上"回到前台补推一次上行"的钩子（M4 补全）：只装一次，只装在手机上
    _installLifecycleHook();
    // 先给上一轮"没跑完"的 Run 收尾（被系统杀掉是手机的常态）：
    // 必须在拉会话/消息**之前**做，否则界面上会先出现一个永远转圈的幽灵气泡。
    // 收尾只改状态与错误说明，已经说出去的话一个字不动（R6）。
    try {
      recoveredRuns = await _source.recoverUnfinishedRuns();
    } catch (e) {
      // 收尾失败不该拦住整个聊天页。但界面上会留下"永远转圈的幽灵气泡"，
      // 所以留一行日志 —— "上一轮没跑完"与"收尾失败"是两种毛病。
      debugPrint('[M3] 上一轮 Run 收尾失败，界面上可能留着转圈的旧气泡：$e');
      recoveredRuns = 0;
    }
    conversations = await _source.listConversations();

    // ★ **清单只读本机库**（2026-09-27 用户拍板"手机完全可以独立配置 provider"的收尾）。
    //
    // 在这之前这里打的是 `GET /api/ai/providers`（桌面 HTTP），离线时回落到
    // SharedPreferences 里那份缓存 —— 而用户**在手机上新建的 provider 只写本机库**，
    // 于是它永远不出现在聊天页的选择器里（"独立配置 provider"就还差这一段）。
    //
    // ## 那份 SharedPreferences 缓存**退役了**（不是"留着不用"）
    //
    // 它当年的用途是"桌面清单在离线时可读"，而那个位置现在由**本机库**占着
    // （`ai_providers` / `ai_models` 本来就是本机表，离线可用，而且能写）。
    // 两份都读就会出现同一个 provider 两套 baseURL / 能力声明 —— 谁赢看不出来，
    // 而它决定"这条消息发得出去吗"。**一个端只留一份清单**（§14.4：两端各一份、
    // 不做自动合并）；缓存代码与那两个键一并删掉，免得日后有人顺手读回来。
    try {
      providers = await _providerStore.listProviders();
      // 空清单要**如实说**：不然用户只看到一个空选择器，只能猜"是不是模型全被删了"。
      // 也顺带说清手机与桌面是两份清单（否则他会以为桌面配过就够了）。
      error = providers.isEmpty ? _emptyLocalProvidersNotice : null;
    } catch (e) {
      providers = const [];
      error = '读本机那份 Provider 清单失败，$e';
    }
    // "这台手机有没有 key"只有本机密钥库知道：本机库那几行里**没有**凭据状态，
    // 所以要现问一次（桌面路径由后端随清单下发，不需要这一步）。
    providers = await _overlayDeviceCredentials(providers);
    await _restoreRememberedModel();
    // 模型目录同样是本机库：本机还没为这条 Provider 配过时要如实说（见 [_loadModels]）
    await _loadModels();
    if (modelsIssue != null) error = modelsIssue;
    // 规则工件（run-kit）：手机必须先拿到它才能开跑（R13）。
    // 这一步以前根本没被调用过，所以手机上第一条消息必然 `NO_RUN_KIT`。
    await ensureRunKit();
    await _startFreshConversation();
    // 上行（M4）：把本机攒的会话/消息推给桌面。**放在最后**且失败不抛 ——
    // 离线是手机的常态，同步不上去绝不该让聊天页起不来。
    await syncOutbox();
    // 下行（M4 的另一半）：把桌面那份投影下来（首次会走全量 + 建游标）。
    // 同一套纪律：失败只记一句提示，绝不拦聊天页。
    await pullChanges();
  }

  /// 本机库里一条 Provider 都没有时那句**人话**（手机上第一件事就是去建一条）——
  /// 或者走「从电脑同步 Provider」把桌面那份直接搬过来（`docs/pairing-design.md` §14.6）。
  static const _emptyLocalProvidersNotice =
      '这台手机上还没有配置任何 Provider —— 手机与桌面各有一份清单，不会自动合并。'
      '去「AI 模型与凭据」新建一条（填 Base URL 与 API Key），或点那里的'
      '「从电脑同步 Provider」把桌面那份清单和模型目录搬过来（API Key 要单独取回），它就会出现在这里。';

  /// **配对令牌**（`X-ComfyHub-Token`，`docs/pairing-design.md` §11）。
  ///
  /// 手机上它是"这台手机等于这台桌面的主人"的凭据：没有它，上面那些要连桌面的路
  /// （取规则工件 / 上行同步 / 取密钥）在闸门打开后一律 403 —— 这正是"手机侧看起来
  /// 全都接好了、却一条都跑不通"的原因（以前这里根本没有这个字段，`token` 恒 null）。
  ///
  /// ⚠ **只在内存里、且只有一份**：真源是 Android Keystore（[PairingTokenStore]），
  /// 这份读的是进程内缓存 [`PairingTokenCache`]（**HTTP 客户端也默认读它**，见
  /// `ApiClient` 的构造函数）。
  /// 2026-09-24 真机验收踩过一次"两份令牌"的坑：AI 工作台这一路带着令牌跑得好好的，
  /// 而「AI 模型与凭据」页自己造的客户端没带 → 那边一条红横幅 `需要配对令牌`。
  /// 所以这里**只留一个来源**，别再加第二个字段。
  String? get pairingToken => PairingTokenCache.value;

  /// 冷启动时**尽力**把令牌读进内存。
  ///
  /// 三条纪律：只在手机（本地 harness）上问、失败**绝不拦启动**、读不到就当没配对。
  Future<void> _loadPairingToken() async {
    if (!_source.isLocal) return; // 桌面没有密钥库：配对令牌只存在于手机
    try {
      // 读出来的值由 `PairingTokenStore.read()` 写进进程内缓存（上面那个 getter 读的就是它）
      await PairingTokenStore().read();
    } catch (_) {
      // 桥不可用 / 密钥库读不动：当作"还没配对"，用户去设置页配一次即可
      PairingTokenCache.set(null);
    }
  }

  /// 配对成功（或「忘记这台电脑」）之后调一次：重新读令牌，再把要带令牌的两条路各走一遍。
  ///
  /// 为什么必须当场重取 run-kit：冷启动时 `ensureRunKit` 很可能正是**因为没有令牌**
  /// 才失败的（闸门开着时匿名取规则工件会被挡下），而"没有规则就不许发"是硬闸门
  /// （[sendBlockReason]）—— 不重试的话，用户配完对还得重启 App 才能聊天。
  Future<void> reloadPairingToken() async {
    await _loadPairingToken();
    notifyListeners();
    await ensureRunKit();
    await syncOutbox();
  }

  /// **规则工件（run-kit）**：当前是否可用（不可用时是给人看的原因）。
  ///
  /// 它同时是界面的准入依据（[sendBlockReason]）：没有规则就不能开跑 ——
  /// 系统提示 / 工具描述 / 安全横幅只有这一份来源（R13）。
  String? kitIssue;

  /// 当前缓存的 kit 版本（可追溯性，契约 §3.4）。
  String? kitVersion;

  /// 上一次冷启动给几条"没跑完的 Run"收了尾（0 = 没有）。
  ///
  /// 界面据此可以如实说一句"上次那条回复被中断了"，而不是让用户对着一个
  /// 永远转圈的助手气泡猜（手机被系统杀掉是常态，见总纲 §7 风险表）。
  int recoveredRuns = 0;

  /// 取/刷新一次规则工件。桌面源恒成功（规则在后端手上）。
  Future<void> ensureRunKit() async {
    try {
      final r = await _source.ensureRunKit(
        baseUrl: baseUrlProvider?.call(),
        // 手机要连的桌面在闸门打开后只认这个头；不传的话这里必然 403
        token: pairingToken,
      );
      kitVersion = r.kitVersion;
      kitIssue = r.ok ? null : (r.reason ?? '本机还没有规则工件（run-kit）');
    } catch (e) {
      // 连通道都炸了也要如实说，不能让"没有规则"变成一条静默的失败
      kitIssue = '取规则工件失败，$e';
    }
    // 拿不到规则 = **发不出去**（按钮会一直是灰的）。这条日志是给排查用的：
    // 手机上"按钮点了没反应"最常见的原因就是它，而界面上没有别的地方会说。
    if (kitIssue != null) debugPrint('[M3] run-kit 不可用，已拦住发送：$kitIssue');
  }

  /// 把本机产生的会话 / 消息推到桌面（**M4**）。
  ///
  /// 桌面在线才有意义，所以**失败不抛**（离线是常态）—— 但成功推上去几条要**说出来**，
  /// 否则用户不知道"手机上聊的东西桌面能不能看到"。返回推上去的条数。
  Future<int> syncOutbox() async {
    if (!_source.isLocal) return 0;
    // 防重入：定时器到点时上一拍还没跑完就**跳过这一拍**，别把请求堆起来
    if (_outboxPushing) return 0;
    _outboxPushing = true;
    try {
      final r = await _source.syncNow(
        baseUrl: baseUrlProvider?.call(),
        // 同 ensureRunKit：上行也是连桌面，闸门开着时没有这个头就是 403
        token: pairingToken,
      );
      if (r.ok && r.applied > 0) {
        syncedToDesktop += r.applied;
        pushNotice =
            '已把本机 ${r.applied} 条记录同步到桌面'
            '${r.pending > 0 ? '（还有 ${r.pending} 条待同步）' : ''}。';
      } else if (!r.ok && r.error != null && r.pending > 0) {
        // 离线时不吵人：只在**确实有东西没推上去**时记一句
        pushNotice = '本机还有 ${r.pending} 条没同步到桌面，${r.error}';
      }
      // M4 补全：队列里还有东西就**挂着定时重试**，推空了立刻停 ——
      // 既不能"只在冷启动与 Run 收尾各推一次"（用户停在聊天页就永远不推了），
      // 也不能空转（空队列还每 45 秒打一次网络 = 白烧电）。
      if (r.pending > 0) {
        _armOutboxRetry();
      } else {
        _cancelOutboxRetry();
      }
      return r.applied;
    } catch (e) {
      // 通道本身炸了：pending 未知，**不动**已经挂着的定时器（挂着就还会重试）
      pushNotice = '同步到桌面失败，$e';
      return 0;
    } finally {
      _outboxPushing = false;
      // 上行是"看不见的"那条路（离线时本来就会失败）：只留一条可查的日志
      debugPrint('[M4] 上行：$pushNotice');
    }
  }

  bool _outboxPushing = false;

  /// 从桌面**拉一次**（**M4 的下行半边**，契约 §1~§3）。
  ///
  /// 与 [syncOutbox] 并排、同一套纪律：
  ///  - 只有手机走这条路（桌面自己就是真源，`_source.pullChanges` 恒 `upToDate`）；
  ///  - **失败不抛**（桌面不在线是常态）：只写 [pushNotice] 那条既有通道，
  ///    **绝不拦聊天页**；
  ///  - 防重入：冷启动 / 回前台 / Run 收尾可能挨得很近，上一趟没跑完就跳过这一趟
  ///    （与上行那条 `_outboxPushing` 同一个理由）。
  ///
  /// 返回值是**落库的行数**（给人看的"这次同步下来几条"）。
  Future<int> pullChanges() async {
    if (!_source.isLocal) return 0;
    if (_pullingChanges) return 0;
    _pullingChanges = true;
    try {
      final r = await _source.pullChanges(
        baseUrl: baseUrlProvider?.call(),
        // 同 ensureRunKit / syncNow：下行也是连桌面，闸门开着时没有这个头就是 403
        token: pairingToken,
      );
      if (r.ok) {
        if (r.applied > 0) {
          syncedFromDesktop += r.applied;
          // 全量重建与增量补几条是**两件事**，说法不能一样（前者用户可能刚 410 过）
          pushNotice = r.mode == 'bootstrap'
              ? '已从桌面全量重建本机镜像（${r.applied} 行）。'
              : '已从桌面同步 ${r.applied} 条记录。';
        }
        debugPrint(
          '[M4] 下行：mode=${r.mode} applied=${r.applied} cursor=${r.cursor}',
        );
      } else {
        debugPrint(
          '[M4] 下行：ok=false mode=${r.mode} cursor=${r.cursor} error=${r.error}',
        );
        if (r.error != null) {
          // 失败要如实说，但不吵人：游标一个字都没动，下次接着拉
          pushNotice = '从桌面拉取失败，${r.error}';
        }
      }
      return r.applied;
    } catch (e) {
      // 通道本身炸了（桥没起来 / 库坏了）：与上行同一条处置 —— 只记一句
      pushNotice = '从桌面拉取失败，$e';
      debugPrint('[M4] 下行：异常 $e');
      return 0;
    } finally {
      _pullingChanges = false;
    }
  }

  bool _pullingChanges = false;

  /// 本次会话累计**从桌面拉下来**的行数（界面上与 [syncedToDesktop] 对称）。
  int syncedFromDesktop = 0;

  /// 上行重试的定时器（只在**队列里真有待办**时存在）。
  Timer? _outboxRetry;

  /// 定时重试的间隔。故意不短：上行是后台补课，不是实时通道；
  /// 真正"内容定稿"的时刻（每轮 Run 收尾）本来就会各推一次。
  static const _kOutboxRetryInterval = Duration(seconds: 45);

  /// 现在有没有挂着上行重试（回归用例据此证明"空队列不空转"）。
  @visibleForTesting
  bool get outboxRetryArmed => _outboxRetry != null;

  void _armOutboxRetry() {
    if (!_source.isLocal) return;
    // 已经挂着就不叠第二个（`periodic` 会一直拍到队列被推空）
    _outboxRetry ??= Timer.periodic(
      _kOutboxRetryInterval,
      (_) => unawaited(syncOutbox()),
    );
  }

  void _cancelOutboxRetry() {
    _outboxRetry?.cancel();
    _outboxRetry = null;
  }

  /// 回到前台（很可能刚恢复网络）补推一次（**M4 补全**）。
  ///
  /// 为什么是"回到前台"而不是"网络恢复"：Android 的连接状态回调要新引一个依赖
  /// （`connectivity_plus`），而这个工程里**已有的**生命周期钩子是 `AppLifecycleListener`
  /// （`lib/app.dart` 用它做退出的收尾），所以这条就落在"回到前台"上 ——
  /// 用户从别的 App / 锁屏回来时补推一次，正是"刚恢复网络"最常见的那一刻。
  ///
  /// 两条纪律：**先看本地队列**（没有待办就一条网络请求都不打），以及
  /// 失败只记 [pushNotice]、**绝不拦聊天页**（离线是手机的常态）。
  Future<void> onAppResumed() async {
    if (!_source.isLocal) return;
    // ① 上行：**先问本地队列**，没有待办就不推（绝不空转烧电）
    if (await _pendingOutbox() > 0) await syncOutbox();
    // ② 下行：**不管本机队列空不空都要拉一次** —— 桌面那边可能刚改过东西
    //    （用户刚在桌面上删了一条会话 / 另开了一台设备在聊），而本机队列本来就是空的。
    //    上限是"每次回前台一次"，不是轮询；失败只记提示。
    await pullChanges();
  }

  /// 问**本地库**还有几条待推（不出网）。
  ///
  /// 读不动时当作"没有待办"：这条通道坏了是另一件事（比上行失败严重得多），
  /// 不该让"回到前台"变成一次崩溃；下次 Run 收尾 / 冷启动照旧会推。
  Future<int> _pendingOutbox() async {
    try {
      return await _source.pendingOutbox();
    } catch (e) {
      // 读不动当作"没有待办"（这条通道坏了是另一件事，不该让"回到前台"变成崩溃）。
      // 但**别静默**：真机上"消息一直没上桌面"时就靠这一行日志区分是网络还是本机库。
      debugPrint('[M4] 读不到本地待推队列，这一拍先不推：$e');
      return 0;
    }
  }

  /// 生命周期钩子（只在手机上，且只装一次）：回到前台补推一次上行。
  AppLifecycleListener? _lifecycle;

  void _installLifecycleHook() {
    if (_lifecycle != null || !_source.isLocal) return;
    try {
      _lifecycle = AppLifecycleListener(
        onResume: () => unawaited(onAppResumed()),
      );
    } catch (_) {
      // 极少数宿主（纯 Dart 单测）没有 WidgetsBinding：没有这个钩子不影响其它路 ——
      // 冷启动与 Run 收尾那两处触发照旧在
    }
  }

  /// 本次会话累计推上桌面的条数（界面可以据此显示"已同步"）。
  int syncedToDesktop = 0;

  /// 上行的如实说明（推成功 / 还有几条没上去 / 为什么没上去）。
  String? pushNotice;

  /// **从桌面取密钥**（`GET /api/ai/credentials?refs=…`，`docs/pairing-design.md` §11.2）。
  ///
  /// 返回的是**给人看的一句话**（拿到几把、哪几把没拿到），不是异常也不是布尔 ——
  /// 这件事的失败方式是"一部分拿到、一部分没有"，只回 ok/失败 就没法如实说。
  ///
  /// 四条纪律：
  ///  ① 只在手机（本地 harness）上做：桌面自己就是密钥的真源，没有"从别处取"这回事；
  ///  ② ref 只从 provider 的 `credentialRef` 来（"这把密钥叫什么名字"只有它有）；
  ///  ③ 值只经手一次：从响应直接写进**本机密钥库**，**绝不**进返回句、日志或界面状态
  ///     （AIH-012：值不回读、不回显）；
  ///  ④ 未授权的 ref **后端不返回也不报错**，所以"没拿到"要由调用方按差集如实说出来。
  Future<String> syncCredentialsFromDesktop() async {
    if (!_source.isLocal) {
      return '这台设备上的密钥就在本机后端，不需要从桌面同步。';
    }
    final token = pairingToken?.trim() ?? '';
    if (token.isEmpty) {
      return '还没有配对令牌，先在设置 →「远程访问与配对」里和桌面配一次对，再来取密钥。';
    }
    final base = baseUrlProvider?.call().trim() ?? '';
    if (base.isEmpty) return '还不知道桌面地址，先在设置里填上桌面的地址。';

    final refs = <String>{};
    for (final p in providers) {
      final ref = p.credentialRef?.trim();
      if (ref != null && ref.isNotEmpty) refs.add(ref);
    }
    if (refs.isEmpty) return '模型清单里没有任何需要密钥的 Provider，没有可同步的东西。';

    final api = _injectedAuthApi?.call(base) ?? InboundAuthApi(base);
    try {
      final got = await api.fetchCredentials(refs.toList(), token: token);
      // 空值等于没拿到：后端对"桌面没配过"与"没授权"都不给这个 ref
      final delivered = <String, String>{
        for (final e in got.entries)
          if (e.value.trim().isNotEmpty) e.key: e.value,
      };
      final missing = refs.where((r) => !delivered.containsKey(r)).toList();
      final written = <String>[];
      final failed = <String>[];
      for (final entry in delivered.entries) {
        final provider = _providerForRef(entry.key);
        if (provider == null || !credentials.isLocal) {
          failed.add(entry.key);
          continue;
        }
        try {
          // 值在这里**只出现这一次**（写进 Keystore 之后本进程不再持有它）
          await credentials.save(provider, entry.value);
          written.add(entry.key);
        } catch (_) {
          failed.add(entry.key);
        }
      }
      if (written.isNotEmpty) await _refreshCredentialStatus();
      final parts = <String>[
        written.isEmpty
            ? '桌面没有下发任何密钥'
            : '已从桌面取得并写入本机密钥库 ${written.length} 把（${written.join('、')}）',
      ];
      if (missing.isNotEmpty) {
        parts.add(
          '${missing.length} 把没拿到（桌面没配过，或这台设备还没被允许下行密钥），分别是'
          '${missing.join('、')}',
        );
      }
      if (failed.isNotEmpty) {
        parts.add('${failed.length} 把写不进本机密钥库（${failed.join('、')}）');
      }
      return '${parts.join('。')}。';
    } catch (e) {
      return '从桌面同步密钥失败，$e';
    } finally {
      api.dispose();
    }
  }

  /// 找这个 ref 属于哪条 provider（密钥下行只回 ref，"怎么存"要按 provider 走）。
  AiProvider? _providerForRef(String ref) {
    for (final p in providers) {
      if ((p.credentialRef ?? '').trim() == ref) return p;
    }
    return null;
  }

  /// 取完密钥把清单上的"有没有 key"重新对齐到**本机**（不然界面上还是"未配置"）。
  Future<void> _refreshCredentialStatus() async {
    try {
      providers = await _overlayDeviceCredentials(providers);
      notifyListeners();
    } catch (_) {
      // 只是显示没跟上，不改变"已经写进密钥库"这个事实
    }
  }

  /// 把每条 provider 的凭据状态换成**本机**的（手机专有）。
  ///
  /// 拿不到就原样保留桌面那份，并且**不抛** —— 这里失败不该让整个列表消失
  /// （用户至少还能看见 provider 与模型）。
  Future<List<AiProvider>> _overlayDeviceCredentials(
    List<AiProvider> list,
  ) async {
    if (!credentials.isLocal) return list;
    final out = <AiProvider>[];
    for (final p in list) {
      try {
        out.add(p.withCredential(await credentials.statusOf(p)));
      } catch (_) {
        out.add(p);
      }
    }
    return out;
  }

  // 手机侧那份 SharedPreferences **清单缓存已退役**（2026-09-27）。
  //
  // 它当年的用途是"桌面下发的清单在离线时可读"，而那个位置现在由**本机库**
  // （`ai_providers` / `ai_models`，见 `lib/core/provider_store.dart`）占着：
  // 既然是本机表，离线可读**又可写**，用户能在手机上独立配 provider。
  // 两份都读就会出现同一个 provider 两套 baseURL / 能力声明 —— 谁赢看不出来，
  // 而它决定"这条消息发得出去吗"。所以**一个端只留一份清单**（§14.4：两端各一份、
  // 不做自动合并），缓存代码连同那两个键一并删掉，免得日后有人顺手读回来。
  //
  // ⚠ 与 AIH-012 的关系：那条纪律是"清单里不许有密钥"，它现在由**本机库**那份承载 ——
  // `ai_providers` 表里只有 `credential_ref`（名字），值在本机密钥库；
  // DTO 的键集合由 harness 的 `ProviderStoreTest` 盯着（`credential` 键都不许写）。

  /// 挑一条现成的空会话接着用；一条都没有才真的新建。
  ///
  /// "空"要**同时**满足两件事：会话里一条消息都没有、本地草稿里也没有打了一半的字
  /// 或挂着的附件。只看 `messageCount` 会把用户没发出去的内容当成垃圾删掉。
  /// 顺带清掉历史遗留的多余空壳（只留最新的一条，列表按更新时间倒序）。
  Future<void> _startFreshConversation() async {
    final reusable = <AiConversation>[];
    for (final c in conversations) {
      if (c.archived || c.messageCount != 0) continue;
      final draft = await loadDraft(c.id);
      if (draft.text.trim().isEmpty && draft.attachments.isEmpty) {
        reusable.add(c);
      }
    }
    if (reusable.isEmpty) {
      await _createConversation();
      return;
    }
    for (final extra in reusable.skip(1)) {
      await _deleteConversationQuietly(extra.id);
    }
    _openEmptyConversation(reusable.first);
  }

  /// 把输入区托盘换成这条会话草稿里挂着的附件（附件按会话归属，不跨会话搬运）。
  Future<void> _applyDraftAttachments(String conversationId) async {
    final draft = await loadDraft(conversationId);
    attachments
      ..clear()
      ..addAll(draft.attachments);
    if (attachments.isEmpty) {
      preflight = null;
      notifyListeners();
    } else {
      await refreshPreflight();
    }
  }

  /// 离开当前会话之前，把输入区的附件托盘存回它的草稿。
  Future<void> _stashAttachments() async {
    final conv = conversation;
    if (conv == null || conv.id.startsWith('local-')) return;
    final prefs = await _store;
    final text = prefs.getString('$_kDraftTextPrefix${conv.id}') ?? '';
    await saveDraft(conv.id, text, attachments: attachments);
  }

  /// 模型目录当前的**如实说明**（离线且本机没缓存过这个 Provider 时非 null）。
  ///
  /// 为什么要单独一个字段：[providers] 从缓存来时界面已经有清单可用了，但
  /// "**这个** Provider 的模型目录本机还没缓存过"是另一件事 —— 不说清楚，
  /// 用户只会看到一个空的选择器，然后猜"是不是模型全被删了"。
  String? modelsIssue;

  Future<void> _loadModels() async {
    final p = selectedProvider;
    modelsIssue = null;
    if (p == null) {
      models = const [];
      selectedModel = null;
      return;
    }
    if (_isPhone) {
      // 手机：模型目录也来自**本机库**（离线可用的真源）。
      //
      // 空目录要如实说：能力声明是"发得出去"的前提（附件与工具在准入那一步看它），
      // 而这里一条都没有时用户只会看到一个空选择器 —— 得告诉他去哪儿加。
      try {
        models = await _providerStore.listModels(p.id);
        if (models.isEmpty) {
          modelsIssue =
              '本机还没为「${p.displayName}」配过模型清单，'
              '去「AI 模型与凭据」里加一个模型（能力要显式声明，别指望它按名字猜），'
              '加完这里就能选它了。';
        }
      } catch (e) {
        models = const [];
        modelsIssue = '读本机那份模型清单失败，$e';
      }
    } else {
      // 桌面：后端是唯一真源，拿不到就**如实抛**（界面显示"连接后端失败"）——
      // 给一个空目录会让人以为"模型都被删了"。
      models = await _api.listModels(p.id);
    }
    if (selectedModel != null &&
        !models.any((m) => m.id == selectedModel!.id)) {
      selectedModel = null;
    }
    selectedModel ??= models.where((m) => m.enabled).firstOrNull;
    _clampReasoningEffort();
  }

  Future<void> selectProvider(AiProvider? p) async {
    selectedProvider = p;
    selectedModel = null;
    await _loadModels();
    // 离线且本机没缓存过这个 Provider 的目录：**当场说出来**（否则用户只看到一个空选择器，
    // 只能猜"是不是模型全被删了"）。成功 / 有缓存时不动 error —— 别把别的错误盖掉。
    if (modelsIssue != null) error = modelsIssue;
    await refreshPreflight();
    notifyListeners();
  }

  /// 选用**另一条 Provider** 下的模型（工作台的模型选择器按来源分组之后走的就是这条路）。
  ///
  /// 为什么不直接 "先 [selectProvider] 再 [selectModel]"：那两步都是 `await` 的，
  /// 而 `_loadModels()` 收尾会把 `selectedModel` 收敛成"目录里第一个启用的"——
  /// 夹在中间的那次选中于是**被覆盖掉**（选到别的模型，或者干脆空着）。
  /// 这里把"换 Provider"和"选模型"合成一步，顺序才定得下来。
  Future<void> selectModelFrom(AiProvider p, AiModel m) async {
    if (selectedProvider?.id != p.id) {
      selectedProvider = p;
      selectedModel = null;
      try {
        await _loadModels();
      } catch (e) {
        // 目录读失败**不该让用户点不中**：那条模型明明就摆在这份界面上。
        error = '读「${p.displayName}」的模型目录失败，$e';
        models = catalogByProvider[p.id] ?? const [];
      }
      if (!models.any((e) => e.id == m.id)) {
        models = catalogByProvider[p.id] ?? models;
      }
    }
    await selectModel(m);
  }

  /// 切换模型必须立刻重算准入（AIH-029），不能等到发送时才提示。
  Future<void> selectModel(AiModel? m) async {
    selectedModel = m;
    _clampReasoningEffort();
    await refreshPreflight();
    await _rememberModel();
    notifyListeners();
  }

  /// 把消息里记的 `modelId` 翻成界面上好认的名字（用户建议 2026-09-20：
  /// "如果本轮对话使用模型与上轮不同，需要在回复气泡显示出来"）。
  ///
  /// 后端给每条助手消息都落了 `model_id`（`ai_messages.model_id`），所以**历史气泡**
  /// 也能如实显示当时用的是哪个模型 —— 不是"当前选中的模型"。
  /// 目录里查不到（用户换了 Provider / 删过模型）时退回 id 本身，绝不显示空白。
  String modelLabelOf(String? modelId) {
    final id = (modelId ?? '').trim();
    if (id.isEmpty) return '';
    for (final m in models) {
      if (m.id == id) return m.displayName.isEmpty ? id : m.displayName;
    }
    return id;
  }

  /// 选择思考强度。模型没声明这个档位时直接忽略（真源在模型目录，AIH-056）。
  void selectReasoningEffort(AiReasoningEffort effort) {
    final allowed =
        selectedModel?.selectableEfforts ?? const <AiReasoningEffort>[];
    if (!allowed.contains(effort)) return;
    reasoningEffort = effort;
    _rememberModel();
    notifyListeners();
  }

  /// 换模型后把档位收敛回合法集合：新模型没有这个档位就退回"关闭"。
  void _clampReasoningEffort() {
    final allowed =
        selectedModel?.selectableEfforts ?? const <AiReasoningEffort>[];
    if (!allowed.contains(reasoningEffort)) {
      reasoningEffort = AiReasoningEffort.off;
    }
  }

  // -----------------------------------------------------------------------
  //  会话
  // -----------------------------------------------------------------------

  Future<void> newConversation() async {
    try {
      // 换新对话之前，把上一个"空壳"清掉（AIH-018 的清理策略，见 [isEmptyConversation]）
      await _stashAttachments();
      await _cleanupEmptyConversation();
      await _createConversation();
    } catch (e) {
      error = '新建会话失败，$e';
    }
    notifyListeners();
  }

  /// 真正落库一条新会话，并切过去（新建 / 冷启动都走这里）。
  ///
  /// 落库的地点由 [_source] 决定：桌面落后端库，手机落**本地库**（桌面离线也能开新对话）。
  Future<void> _createConversation() async {
    final conv = await _source.createConversation(
      providerId: selectedProvider?.id,
      modelId: selectedModel?.id,
    );
    conversations = [conv, ...conversations];
    _openEmptyConversation(conv);
  }

  /// 切到一条空会话：消息、附件托盘、流式状态一起清干净。
  void _openEmptyConversation(AiConversation conv) {
    conversation = conv;
    messages = const [];
    attachments.clear();
    preflight = null;
    _liveReasoning.clear();
    _liveToolCalls.clear();
    notice = null;
    _syncConversationStats();
  }

  /// 打开历史会话。同样先清理上一个空会话。
  Future<void> openConversation(String id) async {
    if (conversation?.id == id) return;
    try {
      await _stashAttachments();
      await _cleanupEmptyConversation();
      conversation = conversations.firstWhere((c) => c.id == id);
      // 库里那条流式助手的 text 是空的（整轮跑完才落库）——切回来时用内存里
      // 那份正在长的正文盖回去，不然用户看到的就是"前面的答复全没了"
      messages = _overlayInflight(await _source.listMessages(id));
      _syncConversationStats();
      await _applyDraftAttachments(id);
      _pruneLiveState();
      notice = null;
    } catch (e) {
      error = '打开会话失败，$e';
    }
    notifyListeners();
  }

  /// 当前会话是不是"空壳"：一条消息都没有，输入区也没有待发送的内容。
  bool get isEmptyConversation =>
      (conversation?.id ?? '').isNotEmpty &&
      messages.isEmpty &&
      attachments.isEmpty;

  /// 切换会话时顺手删掉上一个空会话（用户要求）：聊天记录列表不该堆一串"新对话"。
  ///
  /// 判定要连**本地草稿**一起看：输入框里打了一半的字也是用户内容，
  /// 只看 `messages` 会把"打了字还没发"的会话连同草稿一起删掉（用户报的 bug ②）。
  Future<void> _cleanupEmptyConversation() async {
    final conv = conversation;
    if (conv == null) return;
    if (messages.isNotEmpty || attachments.isNotEmpty) return;
    // 本地乐观插入的占位（还没落库）不能拿去删
    if (conv.id.startsWith('local-')) return;
    final draft = await loadDraft(conv.id);
    if (draft.text.trim().isNotEmpty || draft.attachments.isNotEmpty) return;
    await _deleteConversationQuietly(conv.id);
  }

  /// 删一条会话（连它的草稿一起清掉）。删不掉也不影响切换：可能是归档 / 网络抖动。
  ///
  /// ⚠ 走 [_source] 而不是 `_api`（M3.2 ③ 的第二块砖）：手机上会话的真源是**本地库**，
  /// 原先那句远端 `DELETE` 够不着本地镜像 —— 于是每次清空壳都"看起来做了、实际没做"，
  /// 空壳越攒越多，离线时还白报一个失败。
  Future<void> _deleteConversationQuietly(String id) async {
    try {
      await _source.deleteConversation(id);
      conversations = conversations.where((c) => c.id != id).toList();
      if (conversation?.id == id) {
        conversation = null;
        messages = const [];
      }
      await _clearDraft(id);
    } catch (_) {
      // 静默：清理是"顺手做的事"，失败不该弹错误
    }
  }

  /// 后端把会话标题总结出来了（`conversation.updated`）：本地就地更新，**不再拉一次接口**。
  ///
  /// 标题是"用户第一问之后自动来的"，不该为了它多打一次网络往返；
  /// 但**只改标题**这一个字段，不整条覆盖 —— 事件里的会话对象没有 messageCount 之类的新鲜数据。
  void _applyConversationTitle(String conversationId, String title) {
    conversations = [
      for (final c in conversations)
        if (c.id == conversationId) c.copyWith(title: title) else c,
    ];
    final current = conversation;
    if (current != null && current.id == conversationId) {
      conversation = current.copyWith(title: title);
    }
  }

  Future<void> renameConversation(String title) async {
    final conv = conversation;
    if (conv == null) return;
    try {
      final updated = await _api.patchConversation(conv.id, {'title': title});
      conversations = conversations
          .map((c) => c.id == updated.id ? updated : c)
          .toList();
      conversation = updated;
    } catch (e) {
      error = '重命名失败，$e';
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
      error = '归档失败，$e';
    }
    notifyListeners();
  }

  /// 用户主动删一条会话（会话列表的菜单）。
  ///
  /// ## 两半都要删，而且**如实**说清哪一半成了
  ///
  /// 手机上这条会话有两份：**本机镜像**（本地库 —— 界面上看到的就是它）与**桌面那份**
  /// （桌面是唯一真源）。所以：
  ///
  ///  - **本机**：走 [_source] 的**本地实现**（Kotlin `HarnessStore.deleteConversation`）——
  ///    删掉本地行（消息 / 块 / Run / 工具明细靠外键级联，R1）**并写一个本机删除标记**
  ///    （`local_deletions`）。标记是必需的：上行**没有 delete op**，桌面那份不知道本机删过它，
  ///    下一次下行同步就会把它整行覆盖回来、会话**当场复活**（总纲 §25.1 第三条的欠账）。
  ///    在这之前这一步打的是**远端** `DELETE` —— 那条请求够不着本地镜像，
  ///    于是"看起来删了、其实还在"，而本地那份还会在下一次同步时把界面上的行补齐；
  ///  - **桌面**：远端删除**仍然调**（桌面是唯一真源，桌面那份也该删）。
  ///    ⚠ 红线 R3 一个字没改：上行**只有 upsert**，这里没有、也不许有"叫桌面删掉它"的上行 op ——
  ///    下面那句 DELETE 是界面直接打的**远端请求**，与 `_outbox` 无关。
  ///
  /// 两半可能**单独**失败（桌面离线是常态），所以不假装"两边都删了"：
  /// 本机没删掉就把那条留在列表里并说清；桌面没删掉就明说"桌面那份还在"。
  Future<void> deleteConversation(String id) async {
    // 桌面源：只有远端那一半（与改动前完全一致 —— 桌面就是它自己的真源）
    if (!_source.isLocal) {
      try {
        await _api.deleteConversation(id);
        conversations = conversations.where((c) => c.id != id).toList();
        if (conversation?.id == id) {
          conversation = null;
          messages = const [];
        }
      } catch (e) {
        error = '删除失败，$e';
      }
      notifyListeners();
      return;
    }

    String? localFailure;
    try {
      await _source.deleteConversation(id);
    } catch (e) {
      localFailure = '$e';
    }
    String? remoteFailure;
    try {
      await _api.deleteConversation(id);
    } catch (e) {
      remoteFailure = '$e';
    }

    // 界面按**本机**那一半更新（手机上会话的真源是本地库）：本机删掉了才从列表里拿掉
    if (localFailure == null) {
      conversations = conversations.where((c) => c.id != id).toList();
      if (conversation?.id == id) {
        conversation = null;
        messages = const [];
      }
    }

    if (localFailure != null && remoteFailure != null) {
      error = '删除失败，本机 $localFailure，桌面 $remoteFailure';
    } else if (localFailure != null) {
      error = '桌面那份已删除，但本机没删掉，$localFailure';
    } else if (remoteFailure != null) {
      error = '已从本机删除，但桌面上那份还在（$remoteFailure）—— 桌面不在线时就是这样';
    }
    notifyListeners();
  }

  // -----------------------------------------------------------------------
  //  附件
  // -----------------------------------------------------------------------

  void addAttachment(AiAttachment a) {
    attachments.add(a);
    unawaited(_stashAttachments());
    refreshPreflight();
  }

  /// 选好文件后先**上传到后端**再进托盘（M3）。
  ///
  /// 为什么要先上传：准入判定、严格类型识别、缩略图 / 视频预览帧都在后端做，
  /// 前端只拿到一个 id 和客观事实，不把字节留在内存里。
  /// 上传失败**如实报出来**（哪个文件、为什么），不是静默吞掉。
  Future<void> attachFiles(List<({String name, String path})> files) async {
    if (files.isEmpty) return;
    await _runUpload(() => _api.uploadAttachments(files));
  }

  /// 剪贴板里的**一张图**（截图 / 浏览器复制图片按 Ctrl+V）—— 直接传字节。
  ///
  /// 与 [attachFiles] 是**同一条上传链路、同一套准入**：类型识别、缩略图、
  /// 预检都在后端做；这里只多了一个"字节从哪来"的差别。
  Future<void> attachImageBytes(Uint8List bytes, {required String name}) async {
    if (bytes.isEmpty) return;
    await _runUpload(
      () => _api.uploadAttachmentBytes(name: name, bytes: bytes),
    );
  }

  /// 上传的共同尾巴：置忙 → 收结果（逐条失败也要如实说）→ 存草稿 → 重跑预检。
  ///
  /// 抽出来是为了让"选文件"与"粘图片"不会长成两份逐渐走样的实现
  /// （尤其是 `notice` / `error` / `uploadingAttachments` 这三件事）。
  Future<void> _runUpload(
    Future<AiAttachmentUploadResult> Function() upload,
  ) async {
    uploadingAttachments = true;
    error = null;
    notice = null;
    notifyListeners();
    try {
      final result = await upload();
      attachments.addAll(result.items);
      if (result.failed.isNotEmpty) {
        notice = '有 ${result.failed.length} 个附件没能上传，${result.failedLabel}';
      }
      await _stashAttachments();
      await refreshPreflight();
    } catch (e) {
      error = '附件上传失败，$e';
    } finally {
      uploadingAttachments = false;
      notifyListeners();
    }
  }

  void removeAttachmentAt(int index) {
    if (index < 0 || index >= attachments.length) return;
    final removed = attachments.removeAt(index);
    // 顺手删掉后端那份：还没用进聊天记录的附件留着只会占盘。
    // 已被聊天记录引用的会被后端拒绝（历史消息还要显示它的缩略图），这里忽略即可。
    final id = removed.id;
    if (id != null) unawaited(_api.deleteAttachment(id).catchError((_) {}));
    unawaited(_stashAttachments());
    refreshPreflight();
  }

  void clearAttachments() {
    attachments.clear();
    unawaited(_stashAttachments());
    preflight = null;
    notifyListeners();
  }

  /// 预检由**后端**判定（AIH-029/030）；前端只展示结论，不自行放行。
  ///
  /// 已经有后端 id 的附件只发 id：后端以库里的事实为准（前端自称的 MIME / 模态只是线索）。
  Future<void> refreshPreflight() async {
    final p = selectedProvider;
    final m = selectedModel;
    if (p == null || m == null || attachments.isEmpty) {
      preflight = null;
      notifyListeners();
      return;
    }
    try {
      final ids = attachments.map((a) => a.id).whereType<String>().toList();
      preflight = await _api.preflight(
        providerId: p.id,
        modelId: m.id,
        attachments: ids.length == attachments.length ? const [] : attachments,
        attachmentIds: ids,
      );
    } catch (e) {
      preflight = AiPreflightResult(allowed: false, blockers: ['预检请求失败，$e']);
    }
    notifyListeners();
  }

  /// 有些文件拿不到本地路径（file_picker 偶发）：如实告诉用户，
  /// 而不是"选了却什么都没发生"。
  void noteAttachmentsUnreadable(List<String> names) {
    if (names.isEmpty) return;
    notice = '这些文件读不到本地路径，已跳过 ${names.join('、')}';
    notifyListeners();
  }

  /// 某个附件在当前模型下的缩略图 / 预览帧地址（界面用）。
  String? thumbUrlFor(String? attachmentId) =>
      attachmentId == null || attachmentId.isEmpty
      ? null
      : _api.attachmentThumbUrl(attachmentId);

  /// 后端根地址。**只给界面拼图片地址用**（提问卡里的裁图是后端给的相对路径）。
  String get baseUrl => _api.baseUrl;

  /// 原件的地址（点开看大图 / 播视频）。
  String? fileUrlFor(String? attachmentId) =>
      attachmentId == null || attachmentId.isEmpty
      ? null
      : _api.attachmentFileUrl(attachmentId);

  /// **画廊产物**（媒体 id）的缩略图 / 视频封面地址。
  ///
  /// 必须与附件那两条分开：`/api/ai/attachments/{id}/...` 和 `/api/media/{id}/...`
  /// 是两套 id 空间，混用一定 404（用户报的"生成产物后预览图不可用"）。
  String mediaThumbUrlFor(int mediaId) => _api.mediaThumbUrl(mediaId);
  String mediaPosterUrlFor(int mediaId) => _api.mediaPosterUrl(mediaId);

  /// 一条提示词的工作流原文（画廊接口，但走 AI 的 http client 以便测试能拦到）。
  ///
  /// 用户建议 ①："生成的产物"包括生成的工作流 —— 回复末尾那张卡上的「查看工作流」用它。
  Future<String?> promptWorkflow(int promptId) => _api.promptWorkflow(promptId);

  /// 工作流蓝图 + 接线体检（那条卡上的「看蓝图」用它；同样走注入的 http client）。
  Future<Map<String, dynamic>> workflowBlueprint(
    String raw, {
    bool diagnose = true,
  }) => _api.workflowBlueprint(raw, diagnose: diagnose);

  /// 本机已保存的工作流清单（输入区「选择现有工作流」用；只读）。
  Future<ComfyWorkflowList> listWorkflowFiles({String? q, int limit = 50}) =>
      _api.listWorkflowFiles(q: q, limit: limit);

  /// 读一份本机工作流原文（路径准入在后端）。
  Future<String> readWorkflowFile(String path) => _api.readWorkflowFile(path);

  /// 把改过的工作流存成文件（落点由后端白名单决定）。
  Future<SavedWorkflow> saveWorkflow({required String raw, String? name}) =>
      _api.saveWorkflow(raw: raw, name: name);

  /// 卡片库（角色卡 / 画风卡）—— 工作台输入区的 `@` 菜单用。
  ///
  /// **只有桌面后端有这两张表**：手机侧没有卡片库，`store.isLocal` 时界面不显示入口。
  /// 失败不在这里吞（调用方要显示"读不到卡片库"），也绝不返回空列表冒充"一张卡都没有"。
  Future<List<CharacterCard>> listCharacterCards({String? query}) =>
      _api.listCharacterCards(query: query);

  Future<List<StyleCard>> listStyleCards({String? query}) =>
      _api.listStyleCards(query: query);

  // -----------------------------------------------------------------------
  //  发送
  // -----------------------------------------------------------------------

  /// 发送当前输入。
  ///
  /// 准入判断是**同步**的（[sendBlockReason]，页面用它决定要不要清空输入框）；这里再挡一次，
  /// 因为 `send()` 也可能被别处直接调用 —— 被拒时**只**写 [notice]，绝不动调用方的输入框。
  /// 通过之后：先把这条会话的草稿作废（顺序见下面那段注释），再创建 Run，
  /// 并一直 await 到整轮结束（流式收完 → 落库 → 与服务端对齐）。
  Future<void> send(String text) => sendMessage(OutgoingMessage.plain(text));

  Future<void> sendMessage(OutgoingMessage message) async {
    final text = message.text;
    final body = text.trim();
    final visible = message.displayText.trim();
    // 没什么可发的（空正文 + 没附件）：静默返回，不弹任何提示
    if (body.isEmpty && attachments.isEmpty) return;
    // 准入判断与页面共用同一份（[sendBlockReason]）：不通过就**只**写提示，
    // 绝不先清空输入框 —— 用户打的字必须原样留着。
    final blocked = sendBlockReason();
    if (blocked != null) {
      notice = blocked;
      notifyListeners();
      return;
    }
    final provider = selectedProvider!;
    final model = selectedModel!;
    // 上传失败的附件没有 id：上面已经挡过了，这里拿到的都是能发的
    final ids = attachments.map((a) => a.id).whereType<String>().toList();

    sending = true;
    error = null;
    notice = null;

    try {
      if (conversation == null) {
        final titleSource = visible.isNotEmpty ? visible : body;
        final conv = await _source.createConversation(
          title: titleSource.length > 20
              ? titleSource.substring(0, 20)
              : titleSource,
          providerId: provider.id,
          modelId: model.id,
        );
        conversations = [conv, ...conversations];
        conversation = conv;
      }
      final conv = conversation!;
      // **顺序是这条 bug 的关键**：这条会话的草稿（文字 + 附件）必须在任何
      // `notifyListeners()` **之前**清掉。页面收到通知会去"取回草稿"，
      // 那时草稿要是还在，就等于把用户刚发出去的话又灌回输入框
      // （用户报的"特定情况下一条消息会复制一遍再发送"）。
      // 以前这一步在 `startRun` 之后才做（几十毫秒以后），中间那次通知正好落在这个空档。
      await _clearDraft(conv.id);
      notifyListeners(); // 到这里：发送中 + 这条会话的草稿已作废
      // 模型没声明推理能力时不带这个字段（后端也会再挡一次，AIH-056）
      final effort =
          (model.supportsReasoningEffort && reasoningEffort.isThinking)
          ? reasoningEffort.wire
          : null;
      final start = await _source.startRun(
        RunStartRequest(
          conversationId: conv.id,
          text: text,
          displayText: message.displayText,
          mentions: message.mentions,
          providerId: provider.id,
          modelId: model.id,
          reasoningEffort: effort,
          // 手机本地 harness 要用这三样自己发上游请求（桌面那侧由后端查库，传了也不用）
          api: provider.api,
          baseUrl: provider.baseURL,
          credentialRef: provider.credentialRef,
          attachmentIds: ids,
        ),
      );
      _activeRunId = start.runId;
      // 事件源必须把**库里那条**助手消息的 id 给回来：界面靠它认自己插的流式气泡。
      // 拿不到就当场失败 —— 悄悄兜一个本地 id 会让"事件一条不少、界面一个字都不长"，
      // 而那正是最难查的一类故障（M3.2 ① 修的就是它）。
      final assistantId = start.assistantMessageId;
      if (assistantId == null || assistantId.isEmpty) {
        throw StateError(
          '事件源没有回传助手消息 id（${_source.isLocal ? "本地 harness" : "远端后端"}）',
        );
      }
      // 记下"助手消息 → Run"的关联：失败后的「重试」要把它作为 retryOfRunId 带回去（AIH-024）。
      // 在请求返回时就记，而不是等 `run.started` 事件 —— 后者在流中断时根本收不到。
      _runIds[assistantId] = start.runId;
      // 这条就是"正在流式"的那条：切走会话再切回来，靠它认出内存里那份正文（_liveText）
      _activeAssistantId = assistantId;

      // 乐观插入：用户消息 + 空的助手流式消息，收到 delta 就地增长。
      // 用户消息也带上 attachment 块：气泡里立刻就能看到缩略图，不用等重新拉历史。
      messages = [
        ...messages,
        AiMessage(
          id: start.userMessageId ?? 'local-user',
          conversationId: conv.id,
          seq: messages.length + 1,
          role: 'user',
          status: 'complete',
          text: text,
          parts: [
            // 卡片 / 技能引用放在正文前面，与后端落库顺序一致：
            // 界面按 parts 顺序画，模型按同一顺序读。
            for (final mention in message.mentions)
              AiMessagePart(
                type: 'mention',
                payload: {
                  'kind': mention.kind,
                  'name': mention.name,
                  if (mention.modelText != null)
                    'modelText': mention.modelText,
                  if (mention.snapshot != null) 'snapshot': mention.snapshot,
                },
              ),
            if (visible.isNotEmpty)
              AiMessagePart(type: 'text', text: message.displayText),
            for (final a in attachments)
              AiMessagePart(
                type: 'attachment',
                text: a.name,
                attachmentId: a.id,
              ),
          ],
        ),
        AiMessage(
          id: assistantId,
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
      // 乐观插进去的这两条也要算进"当前会话有多少条消息"（用户 bug ①）
      _syncConversationStats();
      // 草稿在上面（第一次 notify 之前）就已经清掉了 —— 这里补一次没有意义，
      // 但也绝不能省掉上面那次：顺序错了就会把刚发出去的话灌回输入框。
      notifyListeners();

      await _consumeRun(start.runId, assistantId, conv.id);
    } on AiApiException catch (e) {
      error = '启动失败，${e.message}';
      sending = false;
      notifyListeners();
    } catch (e) {
      error = '发送失败，$e';
      sending = false;
      notifyListeners();
    }
  }

  Future<void> _consumeRun(
    String runId,
    String assistantMessageId,
    String conversationId,
  ) async {
    var streamed = '';
    // 这一条助手消息的实时思考 / 工具调用（气泡边流边渲染）
    _liveReasoning.remove(assistantMessageId);
    _liveSegments.remove(assistantMessageId);
    _liveToolCalls.remove(assistantMessageId);
    _liveText.remove(assistantMessageId);
    try {
      await for (final event in _source.events(runId)) {
        switch (event.type) {
          case 'text.delta':
            streamed += event.text ?? '';
            // 已到达的正文单独记一份：切走会话再切回来时，`messages` 会被库里那份
            // （整轮跑完才落库）替换，这里就是唯一能救回正文的来源（_overlayInflight）
            _liveText[assistantMessageId] = streamed;
            _appendLiveSegment(assistantMessageId, 'text', event.text ?? '');
            _replaceMessage(
              assistantMessageId,
              text: streamed,
              status: 'streaming',
            );
            // 逐字回包只攒着，交给下一帧统一刷新（见 _notifyStreaming）
            _notifyStreaming();
          // 思考增量（M4）：按消息累加，气泡里折叠展示
          case 'reasoning.delta':
            final delta = event.text ?? '';
            if (delta.isEmpty) break;
            _liveReasoning.update(
              assistantMessageId,
              (v) => v + delta,
              ifAbsent: () => delta,
            );
            _appendLiveSegment(assistantMessageId, 'reasoning', delta);
            _notifyStreaming();
          // 关键步骤（环节）开始（用户建议 2026-09-28）：`[步骤]正在查资料[/步骤]` 被后端摘掉后
          // 发出来的是**环节名**。界面拿它在**这个环节的第一张工具卡之前**画出环节头 ——
          // 与工具卡同一个道理：位置必须**当场**记进实时段，否则流式期间分组只能事后补，
          // 又变成"工具卡先冒出来、分组后到"（用户 bug⑥ 的同一种错位）。
          case 'step.started':
            final phase = event.data['phase']?.toString() ?? '';
            if (phase.isEmpty) break;
            _appendLiveStep(assistantMessageId, phase);
            _notifyNow();
          // 工具调用请求：approval=pending 的要在气泡上等用户点「批准 / 拒绝」
          case 'tool.requested':
            final approval = event.approval ?? 'not_required';
            final added = _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) =>
                  (prev ??
                          AiToolCallState(
                            callId: event.callId ?? '',
                            name: event.toolName ?? '',
                          ))
                      .copyWith(
                        name: event.toolName ?? prev?.name,
                        arguments: event.arguments ?? prev?.arguments,
                        reason: event.reason ?? prev?.reason,
                        approval: approval,
                        status: approval == 'pending'
                            ? AiToolCallStatus.pendingApproval
                            : AiToolCallStatus.running,
                      ),
            );
            // 第一次见到这个调用号：把工具卡的**位置**也记进实时段，
            // 流式期间才画得出"思考 → 工具 → 正文"的真实交错（用户 bug⑥）
            if (added) {
              _appendLiveToolCall(assistantMessageId, event.callId ?? '');
            }
            _notifyNow();
          case 'tool.started':
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) =>
                  (prev ??
                          AiToolCallState(
                            callId: event.callId ?? '',
                            name: event.toolName ?? '',
                          ))
                      .copyWith(
                        name: event.toolName ?? prev?.name,
                        status: AiToolCallStatus.running,
                      ),
            );
            _notifyNow();
          // 提问（用户建议 ⑥）：AI 当场问一句，**这一次回答不会结束**。
          // 与"等待批准"分开处理：这里要画的是一张带选项 / 输入框的卡片。
          case 'user.question':
            final q = _questionFrom(event.data);
            if (q == null) {
              debugPrint('user.question 事件没有带 question 字段：${event.data}');
              break;
            }
            // ⚠️ 提问事件**不带 `callId`**（真事件里只有 `questionId`，两者不是一回事：
            // 前者是模型的调用号 `call_00_…`，后者是后端生成的 UUID）。所以这里必须
            // 把问题**贴到那条已经存在的 `ask_user_question` 调用上**，不能按 `questionId`
            // 另开一条 —— 用户的 bug（2026-09-21）就是这么来的：
            // 提问卡被挂在另一个键上，于是 `pendingQuestion` 在 `callId` 那条上找不到问题，
            // 输入区永远不接管（而 `tool.completed` / `tool.failed` 也按 `callId` 更新，
            // 结果就是"工具卡一直在运行中、提问卡根本不出现"）。
            final askCallId =
                _liveAskCallId(assistantMessageId) ??
                event.callId ??
                q.questionId;
            _upsertToolCall(
              assistantMessageId,
              askCallId,
              (prev) =>
                  (prev ??
                          AiToolCallState(
                            callId: askCallId,
                            name: 'ask_user_question',
                          ))
                      .copyWith(
                        name: 'ask_user_question',
                        status: AiToolCallStatus.pendingApproval,
                        // 把调用号一起记进问题里：答题 / 切状态都要按它定位
                        // 多问（2026-09-22）：一题一条事件、都挂同一个 `callId`，
                        // 必须**并进同一张卡**；只留最后一条会把前面的题顶掉。
                        questions: AiToolCallState.mergeQuestions(
                          prev?.questions ?? const [],
                          q.copyWith(callId: askCallId),
                        ),
                        // `question` 仍是**第一条**（历史回放与旧用例照旧读它）
                        question:
                            prev?.question ?? q.copyWith(callId: askCallId),
                        // 卡片要画在正确的位置：与工具卡一样，第一次见到就记进实时段
                        arguments: (prev?.arguments ?? '').isEmpty
                            ? '{"question":"${q.question}"}'
                            : prev?.arguments,
                      ),
            );
            _notifyNow();
          case 'user.question.answered':
            final qid = event.data['questionId']?.toString() ?? '';
            if (qid.isEmpty) break;
            final ans = event.data['answer']?.toString() ?? '';
            final idx = (event.data['optionIndex'] as num?)?.toInt();
            // 提问卡按 **callId**（拿不到才退回 questionId）更新：答完 → 卡片切成"已回答"。
            // 多问时只标**这一题**：整张卡要等最后一题答完才收（否则输入区会提前让位）。
            _patchToolCall(
              _liveAskCallId(assistantMessageId) ?? qid,
              (call) =>
                  _applyLocalAnswer(call, qid, answer: ans, optionIndex: idx),
            );
            _notifyNow();
          case 'tool.completed':
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) =>
                  (prev ??
                          AiToolCallState(
                            callId: event.callId ?? '',
                            name: event.toolName ?? '',
                          ))
                      .copyWith(
                        name: event.toolName ?? prev?.name,
                        status: AiToolCallStatus.ok,
                        preview: event.preview ?? prev?.preview,
                        elapsedMs: event.elapsedMs ?? prev?.elapsedMs,
                        // 结构化结果里的产物 id：回复末尾的「画廊入口卡」（用户建议 ⑤）用它
                        mediaIds: AiToolCallState.mediaIdsOf(
                          event.data['result'],
                        ),
                        // 入库的提示词 id：那张卡还要能打开"生成这些图的工作流"（用户建议 ①）
                        promptId: AiToolCallState.promptIdOf(
                          event.data['result'],
                        ),
                        // 2D 布局模板（2026-11-03）：候选框来自工具的结构化结果，
                        // 回复末尾据此画「布局模板」卡
                        layoutCandidates: LayoutCandidate.listFromPayload(
                          event.data['result'],
                        ),
                      ),
            );
            // 提交了任务：立刻刷新进度，用户马上就能在右侧栏看到"排队中 / 正在生成"
            unawaited(refreshComfyJobs());
            // AI 用 `remember` 写了长期记忆：右侧栏的条数 / 预览必须当场跟上
            // （用户 bug：写完长期记忆后侧边栏不刷新，要切走再切回来才变）。
            // 后端改的是 memory.md，这边只有重新拉一次才知道新条数。
            if (event.toolName == 'remember') {
              unawaited(loadMemory(silent: true));
            }
            _notifyNow();
          case 'tool.failed':
            final denied = (event.approval ?? '') == 'denied';
            _upsertToolCall(
              assistantMessageId,
              event.callId ?? '',
              (prev) =>
                  (prev ??
                          AiToolCallState(
                            callId: event.callId ?? '',
                            name: event.toolName ?? '',
                          ))
                      .copyWith(
                        name: event.toolName ?? prev?.name,
                        // 被拒绝不是"出错"：状态要分开显示
                        status: denied
                            ? AiToolCallStatus.denied
                            : AiToolCallStatus.failed,
                        error: event.message ?? event.code,
                        approval: event.approval ?? prev?.approval,
                        elapsedMs: event.elapsedMs ?? prev?.elapsedMs,
                      ),
            );
            _notifyNow();
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
                  ? AiTokenUsage.fromJson(
                      Map<String, dynamic>.from(event.data['usage'] as Map),
                    )
                  : null,
              reasoningEffort: event.data['reasoningEffort']?.toString(),
            );
            _notifyNow();
          case 'run.failed':
            _replaceMessage(
              assistantMessageId,
              text: streamed,
              status: 'failed',
            );
            error = '[${event.code ?? 'ERROR'}] ${event.message ?? '运行失败'}';
          case 'run.cancelled':
            _replaceMessage(
              assistantMessageId,
              text: streamed,
              status: 'cancelled',
            );
            notice = '已停止本次生成。';
          // 会话标题被后端自动总结出来了（用户建议 ③）：立刻刷新标题与对话列表，
          // 不等这次 Run 结束 —— 用户第一眼就能在 AppBar 上看到这条对话叫什么。
          case 'conversation.updated':
            final newTitle = event.data['title']?.toString();
            final convId =
                event.data['conversationId']?.toString() ?? conversation?.id;
            if (newTitle != null && newTitle.isNotEmpty && convId != null) {
              _applyConversationTitle(convId, newTitle);
              _notifyNow();
            }
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
      error = '事件流中断，$e';
    } finally {
      _activeRunId = null;
      sending = false;
      // 与服务端对齐一次（seq、状态、usage 都以库为准）
      await _reloadMessages(conversationId);
      conversations = await _safeListConversations();
      // 这一刻库里已经是权威（正文 / parts 都落库了）：内存里那份"正在长"的正文功成身退。
      // ⚠ 必须在 _reloadMessages **之后**清 —— 对齐时还要靠它兜"库里没落全"的底。
      _liveText.remove(assistantMessageId);
      _activeAssistantId = null;
      // 一轮跑完是"内容定稿"的时刻：顺手把本机的会话/消息推给桌面（M4）。
      // 桌面不在线时它只记一句"还有几条没上去"，不拦任何东西。
      await syncOutbox();
      // 同时把桌面那份的变化拉下来（同一个理由：这是"内容定稿"的时刻，
      // 用户接下来多半要看/继续这段对话，两边的镜像该对齐）。
      await pullChanges();
      _notifyNow();
    }
  }

  /// 停止：取消上游请求（AIH-022）。
  Future<void> stop() async {
    final runId = _activeRunId;
    if (runId == null) return;
    try {
      await _source.cancelRun(runId);
      notice = '已请求停止…';
    } catch (e) {
      error = '停止失败，$e';
    }
    notifyListeners();
  }

  /// 重试（AIH-024）：**新建一个 Run**，用原来的问题、同一个 Provider/模型与思考强度，
  /// 通过 `retryOfRunId` 关联回去，便于事后看出这是哪次失败的重放。
  ///
  /// 不重放工具调用（后端会把已经跑完的那段半截回复 + 工具结果当历史喂回上游，模型自己接着做）。
  /// 失败/取消的助手消息**有内容就留着**、重试另起一条气泡；只有空壳才就地换掉
  /// （判据在后端 `AiMessageDto.isEmptyReply()`，本机靠返回的 `assistantMessageId` 是否变化来分辨）。
  Future<void> retry(String assistantMessageId) async {
    if (sending) return;
    final conv = conversation;
    final p = selectedProvider;
    final m = selectedModel;
    if (conv == null || p == null || m == null) {
      error = '无法重试，请先选择 Provider 与模型';
      notifyListeners();
      return;
    }
    final index = messages.indexWhere((x) => x.id == assistantMessageId);
    if (index <= 0) {
      error = '无法重试，找不到这条回复对应的提问';
      notifyListeners();
      return;
    }
    // 助手消息前面最近的一条用户消息就是这次要重放的提问
    final ask = messages
        .sublist(0, index)
        .lastWhere((x) => x.isUser, orElse: () => messages[index]);
    if (!ask.isUser) {
      error = '无法重试，找不到这条回复对应的提问';
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
      final start = await _source.startRun(
        RunStartRequest(
          conversationId: conv.id,
          text: ask.text,
          providerId: p.id,
          modelId: m.id,
          reasoningEffort: effort,
          api: p.api,
          baseUrl: p.baseURL,
          credentialRef: p.credentialRef,
          retryOfRunId: _runIdOf(assistantMessageId),
        ),
      );
      _activeRunId = start.runId;
      // 理由同 `send()`：拿不到就当场失败，不悄悄兜底
      final freshAssistantId = start.assistantMessageId;
      if (freshAssistantId == null || freshAssistantId.isEmpty) {
        throw StateError(
          '事件源没有回传助手消息 id（${_source.isLocal ? "本地 harness" : "远端后端"}）',
        );
      }
      // 这次 Run 与助手消息的关联就地记下（不依赖事件流，刷新消息也不会丢）
      _runIds[freshAssistantId] = start.runId;
      // 这条就是"正在流式"的那条（与 send() 同：切走会话再切回来要靠它找回正文）
      _activeAssistantId = freshAssistantId;

      AiMessage streamingPlaceholder(int seq) => AiMessage(
        id: freshAssistantId,
        conversationId: conv.id,
        seq: seq,
        role: 'assistant',
        status: 'streaming',
        text: '',
        modelId: m.id,
        reasoningEffort: effort,
      );

      if (freshAssistantId == assistantMessageId) {
        // 后端**就地复用**了那条失败气泡（它是空的）→ 同样就地换成流式占位，不新增气泡
        messages = [
          for (final msg in messages)
            if (msg.id == assistantMessageId)
              streamingPlaceholder(msg.seq)
            else
              msg,
        ];
      } else {
        // 后端**另起**了一条：原来那条失败回复里有内容（正文 / 思考 / 工具卡 / 产物入口卡），
        // 必须原样留着 —— 以前这里无条件就地替换，用户点一次「重试」就把 AI 已经说完的内容
        // 全擦掉了（用户 bug：429 截断之后"直接看不到它已经回复完的内容"）。
        // 新的流式占位追加在末尾（后端取的 seq 就是 max+1）。
        messages = [...messages, streamingPlaceholder(messages.length + 1)];
      }
      _syncConversationStats();
      notifyListeners();
      await _consumeRun(start.runId, freshAssistantId, conv.id);
    } on AiApiException catch (e) {
      error = '重试失败，${e.message}';
      sending = false;
      notifyListeners();
    } catch (e) {
      error = '重试失败，$e';
      sending = false;
      notifyListeners();
    }
  }

  /// 这次 Run 的 id（由 `run.started` 事件记下），重试时作为 `retryOfRunId` 带回去。
  final Map<String, String> _runIds = {};

  String? _runIdOf(String assistantMessageId) => _runIds[assistantMessageId];

  /// **正在流式**的那条助手消息 id（`send()` / `retry()` 记下，整轮收尾时清掉）。
  String? _activeAssistantId;

  /// 正在流式的那条助手**已到达的正文**（按助手消息 id 记）。
  ///
  /// 为什么不直接读 `messages` 里那条：切走会话时 `messages` 会被库里那份替换，
  /// 而库里这条要等**整轮跑完**才有正文 —— 这张表是"切走再切回来"时唯一能救回
  /// 正文的来源（见 [_overlayInflight]）。
  final Map<String, String> _liveText = {};

  // --- 流式实时状态（工具卡 / 思考）---------------------------------------

  /// 助手消息 id → 实时工具调用（有序）。
  final Map<String, List<AiToolCallState>> _liveToolCalls = {};

  /// 助手消息 id → 实时思考正文。
  final Map<String, String> _liveReasoning = {};

  /// 助手消息 id → **按流顺序**排好的段（思考 / 正文 / 工具卡的位置）。
  ///
  /// 用户要求"思考过程按照流顺序如实显示"：正文与思考是交错的，
  /// 只把思考单独堆在气泡顶上就不是"如实"了。流式过程中按到达顺序追加，
  /// `message.completed` 之后以 parts 还原的段为准。
  ///
  /// 段里还会出现 `type == 'tool_call'`、`text == callId` 的"占位段"：
  /// 它记的是**工具卡到达时插在哪个位置**（见 [_appendLiveToolCall]）。
  final Map<String, List<AiMessageSegment>> _liveSegments = {};

  /// 一条助手消息按流顺序排好的段（思考 / 正文）。
  ///
  /// 优先用 `parts`（权威有序块）；流式过程中 parts 还没到，就用事件累积的实时段。
  List<AiMessageSegment> segmentsFor(AiMessage message) {
    final fromParts = AiMessageSegment.fromParts(message.parts);
    if (fromParts.isNotEmpty) return fromParts;
    final live = _liveSegments[message.id];
    if (live != null && live.isNotEmpty) return live;
    // 兜底：老后端只给一段汇总的 reasoning（没有 parts）
    final legacy = _liveReasoning[message.id];
    if (legacy != null && legacy.isNotEmpty) {
      return [AiMessageSegment('reasoning', legacy)];
    }
    return const [];
  }

  /// 往实时段列表里追加一片增量：与上一段同类型就并进去，否则新开一段。
  void _appendLiveSegment(String messageId, String type, String delta) {
    if (delta.isEmpty) return;
    _bumpLiveRevision(messageId);
    final list = _liveSegments.putIfAbsent(
      messageId,
      () => <AiMessageSegment>[],
    );
    if (list.isNotEmpty && list.last.type == type) {
      list[list.length - 1] = AiMessageSegment(type, list.last.text + delta);
    } else {
      list.add(AiMessageSegment(type, delta));
    }
  }

  /// 把一个**环节**记进实时段（用户建议 2026-09-28：关键步骤折叠）。
  ///
  /// 与 [`_appendLiveSegment`] 的区别：**每个环节都新开一段**（同名的相邻环节合并的话，
  /// 界面就分不清"一个环节里两段操作"与"两个环节"了 —— 与工具卡那条同一个理由）。
  void _appendLiveStep(String messageId, String phase) {
    if (phase.isEmpty) return;
    _bumpLiveRevision(messageId);
    _liveSegments
        .putIfAbsent(messageId, () => <AiMessageSegment>[])
        .add(AiMessageSegment('step', phase));
  }

  /// 把一个工具调用的**位置**记进实时段（用户 bug⑥：流式时工具卡全跑到最后）。
  ///
  /// 为什么必须记位置：`message.parts` 是**整条消息结束时**才到的（后端在
  /// `message.completed` 里一次给全），所以流式阶段气泡只能靠实时段还原顺序。
  /// 早先这里只记思考与正文，工具卡就只能在末尾统一补一坨 —— 一轮
  /// "思考 → 工具 → 正文" 被画成 "思考 → 正文 → 工具"，直到最终答复落库才跳回正确位置。
  ///
  /// 与 [_appendLiveSegment] 的区别：**每个工具调用都新开一段**，
  /// 相邻两个工具调用合并会把两个 callId 粘成一个字符串。
  void _appendLiveToolCall(String messageId, String callId) {
    if (callId.isEmpty) return;
    _bumpLiveRevision(messageId);
    _liveSegments
        .putIfAbsent(messageId, () => <AiMessageSegment>[])
        .add(AiMessageSegment('tool_call', callId));
  }

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

  /// 插入 / 更新一条实时工具调用。
  ///
  /// 返回 `true` 表示这是**新出现**的一个调用号（调用方要靠它在实时段里记下位置）。
  bool _upsertToolCall(
    String messageId,
    String callId,
    AiToolCallState Function(AiToolCallState? previous) update,
  ) {
    if (callId.isEmpty) return false;
    _bumpLiveRevision(messageId);
    final list = _liveToolCalls.putIfAbsent(
      messageId,
      () => <AiToolCallState>[],
    );
    final i = list.indexWhere((c) => c.callId == callId);
    final next = update(i < 0 ? null : list[i]);
    if (i < 0) {
      list.add(next);
      return true;
    }
    list[i] = next;
    return false;
  }

  void _patchToolCall(
    String callId,
    AiToolCallState Function(AiToolCallState call) update,
  ) {
    for (final entry in _liveToolCalls.entries) {
      final list = entry.value;
      final i = list.indexWhere((c) => c.callId == callId);
      if (i >= 0) {
        list[i] = update(list[i]);
        // 只有**真的动过**的那条消息才算变了
        _bumpLiveRevision(entry.key);
      }
    }
  }

  /// 一个 `user.question` 事件里解出提问。
  ///
  /// **两种形状都要认**（用户 bug 2026-09-21 的现场根因之一）：
  ///  - 后端现在发的是**嵌套**的 `{runId, question: {…PendingQuestion…}}`（与
  ///    `message.completed` 里的 `asked` 同一个形状）；
  ///  - 但历史上后端把整份 `PendingQuestion` **平铺**在 data 顶层 —— 那时
  ///    `data['question']` 是**问题文字那个字符串**，`is Map` 判假，
  ///    于是界面永远解不出提问（输入区不接管、工具卡一直"运行中"）。
  ///
  /// 平铺形状怎么认：`data['question']` 是字符串 **且** data 里有 `questionId`
  /// —— 那 data 本身就是一份 `PendingQuestion`。刻意**不**猜别的可能，
  /// 免得把某条未来事件的字段误当成提问。
  static AiQuestion? _questionFrom(Map<String, dynamic> data) {
    final nested = data['question'];
    if (nested is Map) {
      return AiQuestion.fromJson(Map<String, dynamic>.from(nested));
    }
    if (nested is String &&
        (data['questionId']?.toString().isNotEmpty ?? false)) {
      return AiQuestion.fromJson(data);
    }
    return null;
  }

  /// 这条助手消息里**正在等回答**的那个 `ask_user_question` 调用的 `callId`。  ///
  /// 提问事件落在哪条调用上，答案与状态就要回到哪条上（见 `user.question` 那段注释）：
  /// 两个 id 不是一回事，混用就会出现"工具卡一直运行中、输入区不接管"。
  String? _liveAskCallId(String messageId) {
    for (final call in _liveToolCalls[messageId] ?? const <AiToolCallState>[]) {
      if (_askPending(call)) {
        return call.callId;
      }
    }
    return null;
  }

  /// 流式状态只对"还在列表里的消息"有意义：换会话 / 刷新后清掉孤儿，避免内存只增不减。
  ///
  /// ⚠ **正在流式的那条不算孤儿**（用户 bug：生成答复时切到别的会话再切回来，
  /// 前面已显示的部分答复全部消失）：它还不在这份 `messages` 里（或者被库里那份
  /// 空壳顶掉了），但内存里的实时段 / 思考 / 工具卡是切回来时唯一的还原来源 ——
  /// 清了它，切回来的就是一条空气泡。
  void _pruneLiveState() {
    final ids = messages.map((m) => m.id).toSet();
    final active = _activeAssistantId;
    if (active != null) ids.add(active);
    _liveReasoning.removeWhere((k, _) => !ids.contains(k));
    _liveSegments.removeWhere((k, _) => !ids.contains(k));
    _liveToolCalls.removeWhere((k, _) => !ids.contains(k));
    _liveRevisions.removeWhere((k, _) => !ids.contains(k));
  }

  /// 把内存里"正在长"的那份流式正文盖回刚从库里读出来的清单上（**纯前端合并**）。
  ///
  /// 用户 bug（2026-09-28）：生成答复时切到别的会话再切回来，前面已显示的部分答复全部消失。
  /// 根因：流式正文只在**整轮跑完**才落库，期间库里那条助手消息的 text 是空的 ——
  /// 任何一次"从库里重读"（[openConversation] / [_reloadMessages]）都会把内存里
  /// 正在长的那份抹掉。修法：重读之后把还活着的那条盖回去（正文取更长的那份，
  /// 状态仍是 streaming）；思考与工具卡由 [_pruneLiveState] 的"活跃条目不清理"保住。
  List<AiMessage> _overlayInflight(List<AiMessage> fresh) {
    final liveId = _activeAssistantId;
    if (liveId == null) return fresh;
    final live = _liveText[liveId];
    return [
      for (final m in fresh)
        if (m.id == liveId && (live ?? '').length > m.text.length)
          m.copyWith(text: live, status: 'streaming')
        else
          m,
    ];
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
    _syncConversationStats();
  }

  /// 把"当前会话有多少条消息 / 最近一次对话是什么时候"对齐到本机已发生的事实，
  /// 并把有新消息的那条顶到会话列表最前面。
  ///
  /// 为什么需要（用户 bug ①）：AppBar 上的"N 条消息"读的是 `conversation.messageCount`，
  /// 而那个数字**只有拉会话列表时后端才会给**。发出去的消息是本机乐观插进去的、
  /// 流式回复也是本机长出来的，所以不同步的话，标题栏那个数字会一直停在"打开这条会话时"的值，
  /// 要切走再切回来才变。
  ///
  /// 时间戳（用户建议 ⑥）是同一个道理：左侧会话列表上写的"最近一次对话"如果也读后端那份旧值，
  /// 刚聊完看着还是"3 天前"。所以这里在**消息数真的变了**的时候把它改成当前时间 ——
  /// 只是打开旧会话看看不改，否则"翻一下就变成刚刚"。
  ///
  /// 后端列表按 `updated_at DESC` 排；本机既然长了新消息，就同样把它顶到最前，
  /// 不然会出现"列表最后一条写着刚刚"这种自相矛盾的画面。
  void _syncConversationStats() {
    final conv = conversation;
    if (conv == null) return;
    final count = messages.length;
    if (conv.messageCount == count) return;
    final updated = conv.copyWith(
      messageCount: count,
      updatedAt: DateTime.now(),
    );
    conversation = updated;
    conversations = [
      updated,
      for (final c in conversations)
        if (c.id != conv.id) c,
    ];
  }

  Future<void> _reloadMessages(String conversationId) async {
    try {
      // 用户已经切到别的会话：这次对齐是给**那条**会话的，别把旧会话的消息
      // 盖进当前屏幕（否则 B 会话里会突然冒出 A 会话的消息）。
      if (conversation?.id != conversationId) return;
      final fresh = await _source.listMessages(conversationId);
      // 后端可能还没落 parts（消息仍在写库）：保留事件里拿到的那些，别把工具卡擦掉
      final known = {for (final m in messages) m.id: m.parts};
      // 同理，正文 / 状态以"内存里正在长的那份"兜底（库里要等整轮跑完才有）
      messages = _overlayInflight([
        for (final m in fresh)
          if (m.parts.isEmpty && (known[m.id]?.isNotEmpty ?? false))
            m.copyWith(parts: known[m.id])
          else
            m,
      ]);
      _pruneLiveState();
      _syncConversationStats();
    } catch (_) {
      // 保留本地已有的流式内容，别因为一次刷新失败把回复擦掉
    }
  }

  Future<List<AiConversation>> _safeListConversations() async {
    try {
      return await _source.listConversations();
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
