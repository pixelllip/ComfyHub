import 'dart:convert';

import 'package:http/http.dart' as http;

import '../models/ai_models.dart';

/// AI 工作台的后端客户端（AIH-006 / AIH-012 / AIH-018 / AIH-029）。
///
/// 关键约束：**没有任何读取 API Key 的方法** —— 后端也只提供状态。
/// 前端因此不可能把密钥写进状态、日志或导出。
class AiApiClient {
  final String baseUrl;
  final http.Client _client;

  /// **配对令牌**（`X-ComfyHub-Token`）的取值回调（`docs/pairing-design.md` §11）。
  ///
  /// 默认 `null` = **一个字节都不变**：不带头时后端照旧按"闸门没开 / 回环"放行，
  /// 桌面三条 e2e 与既有用例都走这条（它们一个头都不带）。
  ///
  /// 做成回调而不是构造时取一次值：手机配对成功那一刻令牌才出现在内存里
  /// （`AiWorkspaceStore.pairingToken`），缓存一份头会让它要等 client 重建才生效。
  final String? Function()? tokenProvider;

  AiApiClient(this.baseUrl, {http.Client? client, this.tokenProvider})
      : _client = client ?? http.Client();

  void dispose() => _client.close();

  Uri _uri(String path) => Uri.parse('$baseUrl$path');

  /// 每个请求的公共头（令牌**现取**；空令牌不发这个头，理由见
  /// `InboundAuthApi._headers`：空串会被当成"带了个错的令牌"）。
  Map<String, String> get _headers {
    final token = tokenProvider?.call();
    return {
      'Content-Type': 'application/json; charset=utf-8',
      'Accept': 'application/json',
      if (token != null && token.isNotEmpty) 'X-ComfyHub-Token': token,
    };
  }

  dynamic _decode(http.Response res) {
    final body = utf8.decode(res.bodyBytes);
    dynamic json;
    if (body.isNotEmpty) {
      try {
        json = jsonDecode(body);
      } catch (_) {
        json = null;
      }
    }
    if (res.statusCode >= 200 && res.statusCode < 300) return json;
    // 顺序是有讲究的：AI 领域错误的**人话在 `detail` 里**（`error` 是稳定码，AIH-024），
    // 普通错误的**人话在 `error` 里**（`detail` 不出现）。两条路都得能拿到人话。
    final message = (json is Map && json['detail'] != null)
        ? json['detail'].toString()
        : (json is Map && json['error'] != null
            ? json['error'].toString()
            // 最后的兜底：`message` 是老形状（Rust 侧 2026-09-24 已对齐成 {error, detail?}）
            : (json is Map && json['message'] != null ? json['message'].toString() : '请求失败'));
    throw AiApiException(res.statusCode, message);
  }

  Future<dynamic> _get(String path) async => _decode(await _client.get(_uri(path), headers: _headers));

  Future<dynamic> _send(String method, String path, [Object? body]) async {
    final req = http.Request(method, _uri(path))
      ..headers.addAll(_headers)
      ..body = body == null ? '' : jsonEncode(body);
    final streamed = await _client.send(req);
    return _decode(await http.Response.fromStream(streamed));
  }

  // --- Provider ---------------------------------------------------------

  Future<List<AiProvider>> listProviders() async {
    final raw = await _get('/api/ai/providers');
    return (raw as List).whereType<Map>().map((e) => AiProvider.fromJson(Map<String, dynamic>.from(e))).toList();
  }

  Future<AiProvider> createProvider(Map<String, dynamic> body) async =>
      AiProvider.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/providers', body) as Map));

  Future<AiProvider> updateProvider(String id, Map<String, dynamic> body) async => AiProvider.fromJson(
      Map<String, dynamic>.from(await _send('PUT', '/api/ai/providers/$id', body) as Map));

  Future<void> deleteProvider(String id) async => _send('DELETE', '/api/ai/providers/$id');

  /// 连接测试：只拿回状态与稳定错误码，**不会拿回密钥**（AIH-008）。
  Future<AiProviderTestResult> testProvider(String id) async => AiProviderTestResult.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/ai/providers/$id/test') as Map));

  /// 模型发现：返回候选，**不落库**；用户勾选后再由 [saveModels] 保存（AIH-009）。
  Future<AiDiscoverResult> discoverModels(String id) async => AiDiscoverResult.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/ai/providers/$id/discover-models') as Map));

  Future<AiCredentialStatus> credentialStatus(String providerId) async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _get('/api/ai/providers/$providerId/credentials') as Map));

  /// 只写：值传出去之后前端不再持有，也不回读（留空表示不修改）。
  Future<AiCredentialStatus> setCredential(String providerId, String value) async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _send('PUT', '/api/ai/providers/$providerId/credentials', {'value': value}) as Map));

  Future<AiCredentialStatus> removeCredential(String providerId) async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _send('DELETE', '/api/ai/providers/$providerId/credentials') as Map));

  // --- 联网搜索（Keenable）的可选密钥 -------------------------------------
  //
  // 与 Provider 凭据同一套纪律（AIH-012）：只写不读、状态里没有值、
  // env 来源只读。出厂免密钥就能用，这里只是给想提额的用户一个入口。

  Future<AiCredentialStatus> webSearchCredentialStatus() async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _get('/api/ai/web-search/credential') as Map));

  /// 只写：值传出去之后前端不再持有，也不回读（留空表示不修改）。
  Future<AiCredentialStatus> setWebSearchCredential(String value) async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _send('PUT', '/api/ai/web-search/credential', {'value': value}) as Map));

  Future<AiCredentialStatus> removeWebSearchCredential() async =>
      AiCredentialStatus.fromJson(Map<String, dynamic>.from(
          await _send('DELETE', '/api/ai/web-search/credential') as Map));

  // --- 模型目录 ----------------------------------------------------------

  Future<List<AiModel>> listModels(String providerId) async {
    final raw = await _get('/api/ai/providers/$providerId/models');
    return (raw as List).whereType<Map>().map((e) => AiModel.fromJson(Map<String, dynamic>.from(e))).toList();
  }

  Future<List<AiModel>> saveModels(String providerId, List<Map<String, dynamic>> models) async {
    final raw = await _send('PUT', '/api/ai/providers/$providerId/models', {'models': models});
    return (raw as List).whereType<Map>().map((e) => AiModel.fromJson(Map<String, dynamic>.from(e))).toList();
  }

  // --- Skills（M5） ------------------------------------------------------

  /// Skills 列表：只给元数据，**不读正文**（磁盘是真源，后端刻意不缓存）。
  Future<List<AiSkill>> listSkills() async {
    final raw = await _get('/api/ai/skills');
    return (raw as List)
        .whereType<Map>()
        .map((e) => AiSkill.fromJson(Map<String, dynamic>.from(e)))
        .toList();
  }

  Future<AiSkillDetail> skillDetail(String name) async => AiSkillDetail.fromJson(
      Map<String, dynamic>.from(await _get('/api/ai/skills/${Uri.encodeComponent(name)}') as Map));

  /// 注册（或覆盖）用户 skill；后端 201 + SkillDto。
  Future<AiSkill> createSkill({
    required String name,
    required String description,
    String? whenToUse,
    required String content,
  }) async =>
      AiSkill.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/skills', {
        'name': name,
        'description': description,
        'whenToUse': ?whenToUse,
        'content': content,
      }) as Map));

  /// 删除用户 skill。内置 skill 会被后端拒绝（错误消息原样展示给用户）。
  Future<void> deleteSkill(String name) async =>
      _send('DELETE', '/api/ai/skills/${Uri.encodeComponent(name)}');

  /// skills 投放口在哪（界面显示绝对路径，让用户知道往哪个文件夹拷）。
  Future<AiSkillRoots> skillRoots() async =>
      AiSkillRoots.fromJson(Map<String, dynamic>.from(await _get('/api/ai/skills/roots') as Map));

  /// 重新扫描投放口：给"拷进来但没写 frontmatter"的 skill 自动登记，并返回最新清单。
  /// 应用开着的时候往文件夹里拷东西，点一下就生效，不用重启（启动时后端也会扫一次）。
  Future<AiSkillRescanResult> rescanSkills() async => AiSkillRescanResult.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/ai/skills/rescan') as Map));

  // --- 长期记忆（M6） ----------------------------------------------------

  Future<AiMemory> memory() async =>
      AiMemory.fromJson(Map<String, dynamic>.from(await _get('/api/ai/memory') as Map));

  /// 整篇替换（界面的「保存」）。
  Future<AiMemory> saveMemory(String content) async => AiMemory.fromJson(
      Map<String, dynamic>.from(await _send('PUT', '/api/ai/memory', {'content': content}) as Map));

  /// 追加一条（界面的「添加一条」）。
  Future<AiMemory> appendMemory(String content) async => AiMemory.fromJson(Map<String, dynamic>.from(
      await _send('POST', '/api/ai/memory/entries', {'content': content}) as Map));

  /// 按下标删几条（界面的单条 / 批量删除）。下标与 `AiMemory.entries` 同源。
  Future<AiMemory> deleteMemoryEntries(List<int> indices) async => AiMemory.fromJson(
      Map<String, dynamic>.from(
          await _send('POST', '/api/ai/memory/delete', {'indices': indices}) as Map));

  Future<AiMemory> clearMemory() async =>
      AiMemory.fromJson(Map<String, dynamic>.from(await _send('DELETE', '/api/ai/memory') as Map));

  // --- 工具与权限（M4） --------------------------------------------------

  Future<List<AiToolInfo>> listTools() async {
    final raw = await _get('/api/ai/tools');
    return (raw as List)
        .whereType<Map>()
        .map((e) => AiToolInfo.fromJson(Map<String, dynamic>.from(e)))
        .toList();
  }

  Future<AiToolPolicy> toolPolicy() async =>
      AiToolPolicy.fromJson(Map<String, dynamic>.from(await _get('/api/ai/tools/policy') as Map));

  /// 只发要改的字段（`any subset`），返回生效后的完整策略。
  Future<AiToolPolicy> updateToolPolicy(Map<String, dynamic> patch) async => AiToolPolicy.fromJson(
      Map<String, dynamic>.from(await _send('PUT', '/api/ai/tools/policy', patch) as Map));

  /// 批准 / 拒绝一次工具调用。返回 `accepted`：false 表示这次调用已经结束或超时，
  /// 按钮点晚了 —— 界面要把这个如实说出来，不能假装成功。
  Future<bool> approveToolCall(String callId) async =>
      _approvalAccepted(await _send('POST', '/api/ai/tool-calls/${Uri.encodeComponent(callId)}/approve'));

  Future<bool> denyToolCall(String callId) async =>
      _approvalAccepted(await _send('POST', '/api/ai/tool-calls/${Uri.encodeComponent(callId)}/deny'));

  bool _approvalAccepted(dynamic raw) => raw is Map && raw['accepted'] == true;

  // --- 提问（用户建议 ⑥）------------------------------------------------

  /// 这个 Run 现在还在等用户回答什么（没有就返回 null）。
  ///
  /// 给"界面重开 / 断线重连"用：SSE 里那条 `user.question` 事件可能是在界面
  /// 没挂着的时候发出的，靠这个接口把那张卡片补回来。
  Future<AiQuestion?> pendingQuestion(String runId) async {
    final raw = Map<String, dynamic>.from(
        await _get('/api/ai/runs/${Uri.encodeComponent(runId)}/question') as Map);
    if (raw['pending'] != true) return null;
    final q = raw['question'];
    return q is Map ? AiQuestion.fromJson(Map<String, dynamic>.from(q)) : null;
  }

  /// 回答一条提问。[optionIndex] 与 [answer] 至少给一个。
  ///
  /// 返回 `accepted`：false 表示这条提问已经结束了（超时 / Run 已停），
  /// 界面要如实告诉用户"这次回答没被采纳"，不能假装成功。
  Future<bool> answerQuestion(String questionId, {int? optionIndex, String? answer}) async {
    final raw = await _send(
      'POST',
      '/api/ai/questions/${Uri.encodeComponent(questionId)}/answer',
      {
        'optionIndex': ?optionIndex,
        'answer': ?answer,
      },
    );
    return raw is Map && raw['accepted'] == true;
  }

  /// 跳过一条提问：当作"用户没有回答"放行（模型据此基于已有信息继续）。
  Future<bool> skipQuestion(String questionId) async {
    final raw = await _send('POST', '/api/ai/questions/${Uri.encodeComponent(questionId)}/skip');
    return raw is Map && raw['accepted'] == true;
  }

  // --- NSFW 内容开关（2026-09-20） ----------------------------------------

  /// 读 NSFW 设置（是否注入那段越狱提示词 + 预览图要不要打码）。
  ///
  /// 真源在后端（`app_settings` 的 `ai.nsfw`）：`enabled` 决定系统提示词里注不注入那一段，
  /// 所以前端**不能**自己存一份 —— 否则会出现"界面说开着、后端其实没注入"。
  Future<AiNsfwSettings> nsfwSettings() async =>
      AiNsfwSettings.fromJson(Map<String, dynamic>.from(await _get('/api/ai/nsfw') as Map));

  /// 写 NSFW 设置，返回**生效后**的那份（`effectiveBlur` 由后端算好）。
  Future<AiNsfwSettings> updateNsfwSettings({required bool enabled, required bool blur}) async =>
      AiNsfwSettings.fromJson(Map<String, dynamic>.from(
          await _send('PUT', '/api/ai/nsfw', {'enabled': enabled, 'blur': blur}) as Map));

  // --- 内置模型目录（只读预览 / 对齐） ------------------------------------

  /// 预览：内置目录有哪些模型、库里有多少条与它不一致。**不写库**。
  Future<AiBuiltinCatalogStatus> builtinStatus() async => AiBuiltinCatalogStatus.fromJson(
      Map<String, dynamic>.from(await _get('/api/ai/builtin/status') as Map));

  /// 对齐内置目录。
  ///  - `add-missing`：只补库里缺的模型，**绝不改已有行**；
  ///  - `refresh-capabilities`：把同名模型的能力（模态 / 工具 / 推理 / 档位 / 方言 /
  ///    上下文）对齐过来，不新增不删除、不动展示名与启用状态。
  Future<AiBuiltinCatalogStatus> syncBuiltinCatalog({required String mode}) async =>
      AiBuiltinCatalogStatus.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/ai/builtin/sync', {'mode': mode}) as Map));

  // --- 模型能力目录（在线更新 / 实测校准，2026-09-23） ----------------------

  /// 能力目录状态（内置表 + 在线目录的新鲜度 + 校准开关）。
  Future<AiCapabilityStatus> capabilityStatus() async => AiCapabilityStatus.fromJson(
      Map<String, dynamic>.from(await _get('/api/ai/capabilities/status') as Map));

  /// 立即从在线数据集（models.dev）刷新能力目录。失败不抛错：返回体里
  /// `lastMessage` 带着如实的原因（旧目录原样保留），界面照常显示。
  Future<AiCapabilityStatus> refreshCapabilityCatalog() async => AiCapabilityStatus.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/ai/capabilities/refresh', const {}) as Map));

  /// 开/关「实测校准」（校准会发真实请求，默认关）。
  Future<AiCapabilityStatus> setProbeEnabled({required bool enabled}) async =>
      AiCapabilityStatus.fromJson(Map<String, dynamic>.from(
          await _send('PUT', '/api/ai/capabilities/probe-toggle', {'enabled': enabled}) as Map));

  /// 校准思考档位：真发极小请求从最高档往下试，把网关实际接受的档位写回。
  /// `providerId` 缺省 = 所有启用的 Provider；`modelId` 缺省 = 该 Provider 全部推理模型。
  Future<AiCapabilityProbeResponse> probeCapabilities({String? providerId, String? modelId}) async =>
      AiCapabilityProbeResponse.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/ai/capabilities/probe', {
            'providerId': ?providerId,
            'modelId': ?modelId,
          }) as Map));

  // --- 附件（M3） --------------------------------------------------------

  /// 上传附件（multipart，字段名 `files`）。
  ///
  /// 传的是**磁盘路径**而不是字节：桌面端 `file_picker` 给的就是路径，
  /// 让 `http` 直接流式读文件，避免把几十 MB 读进 Dart 堆。
  /// 类型由后端按签名判定（AIH-027）：认不出来会被拒，错误消息原样带回界面。
  Future<AiAttachmentUploadResult> uploadAttachments(
    List<({String name, String path})> files,
  ) async {
    final req = http.MultipartRequest('POST', _uri('/api/ai/attachments'));
    for (final f in files) {
      req.files.add(await http.MultipartFile.fromPath('files', f.path, filename: f.name));
    }
    final streamed = await _client.send(req);
    final res = await http.Response.fromStream(streamed);
    return AiAttachmentUploadResult.fromJson(
        Map<String, dynamic>.from(_decode(res) as Map));
  }

  /// 缩略图 / 视频预览帧（**同一张接口**）：图片给缩略图，视频给第一帧预览图。
  /// 没有可看的图时后端回 204，界面要退化成文件图标（用 `errorBuilder`）。
  String attachmentThumbUrl(String attachmentId) =>
      '$baseUrl/api/ai/attachments/$attachmentId/thumb';

  /// 原件（点开看大图 / 播视频）。
  String attachmentFileUrl(String attachmentId) =>
      '$baseUrl/api/ai/attachments/$attachmentId/file';

  /// **画廊产物**（不是附件）的缩略图 / 封面地址。
  ///
  /// 这两个必须走画廊的 `/api/media/{id}/...`：附件那两条接口的 id 空间完全不同，
  /// 把媒体 id 塞进附件接口只会 404 —— 界面上就是"生成的产物预览图不可用"
  /// （用户报的 bug）。图片走 `thumb`，视频走 `poster`（后端抽第一帧）。
  String mediaThumbUrl(int mediaId) => '$baseUrl/api/media/$mediaId/thumb';
  String mediaPosterUrl(int mediaId) => '$baseUrl/api/media/$mediaId/poster';

  /// 本次生成真正入库的那份提示词 / 工作流原文（用户建议 ①："生成的产物"包括生成的工作流）。
  ///
  /// 后端没存过工作流时返回 null（HTTP 204），界面据此说"这次没有工作流"。
  /// 走画廊的 `/api/prompts/{id}/workflow`：与上面那两个媒体地址同一个道理，
  /// **必须用注入进来的 http client**，否则测试里换了假后端这条就测不到
  /// （真实客户端在 widget 测试里一律 400）。
  Future<String?> promptWorkflow(int promptId) async {
    final res = await _client.get(_uri('/api/prompts/$promptId/workflow'), headers: _headers);
    if (res.statusCode == 204 || res.statusCode == 404) return null;
    if (res.statusCode < 200 || res.statusCode >= 300) {
      throw AiApiException(res.statusCode, '读取工作流失败（HTTP ${res.statusCode}）');
    }
    return res.body.isEmpty ? null : res.body;
  }

  /// 删除还没用进聊天记录的附件（已被引用的会报错，调用方可以忽略）。
  Future<void> deleteAttachment(String attachmentId) async =>
      _send('DELETE', '/api/ai/attachments/$attachmentId');

  // --- 附件预检 ----------------------------------------------------------

  /// 准入预检（**纯计算**，不产生上游请求）。
  ///
  /// 有新式 `attachmentIds` 就只发它 —— 后端以**库里的事实**为准，前端声明只是线索；
  /// 老式 `attachments`（只有名字 / MIME / 文件头）保留给直接构造附件的场景。
  Future<AiPreflightResult> preflight({
    required String providerId,
    required String modelId,
    List<AiAttachment> attachments = const [],
    List<String> attachmentIds = const [],
  }) async =>
      AiPreflightResult.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/preflight', {
        'providerId': providerId,
        'modelId': modelId,
        if (attachmentIds.isNotEmpty)
          'attachmentIds': attachmentIds
        else
          'attachments': attachments.map((a) => a.toJson()).toList(),
      }) as Map));

  // --- 会话与消息 --------------------------------------------------------

  Future<List<AiConversation>> listConversations({bool includeArchived = false}) async {
    final raw = await _get('/api/ai/conversations${includeArchived ? '?includeArchived=1' : ''}');
    return (raw as List)
        .whereType<Map>()
        .map((e) => AiConversation.fromJson(Map<String, dynamic>.from(e)))
        .toList();
  }

  Future<AiConversation> createConversation({String? title, String? providerId, String? modelId}) async =>
      AiConversation.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/conversations', {
        'title': ?title,
        'providerId': ?providerId,
        'modelId': ?modelId,
      }) as Map));

  Future<AiConversation> patchConversation(String id, Map<String, dynamic> body) async => AiConversation.fromJson(
      Map<String, dynamic>.from(await _send('PATCH', '/api/ai/conversations/$id', body) as Map));

  Future<void> deleteConversation(String id) async => _send('DELETE', '/api/ai/conversations/$id');

  Future<List<AiMessage>> listMessages(String conversationId) async {
    final raw = await _get('/api/ai/conversations/$conversationId/messages');
    return (raw as List).whereType<Map>().map((e) => AiMessage.fromJson(Map<String, dynamic>.from(e))).toList();
  }

  Future<AiMessage> appendMessage(
    String conversationId, {
    required String role,
    String text = '',
    String status = 'complete',
    String? modelId,
    List<Map<String, dynamic>> parts = const [],
  }) async =>
      AiMessage.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/conversations/$conversationId/messages', {
        'role': role,
        'text': text,
        'status': status,
        'modelId': ?modelId,
        'parts': parts,
      }) as Map));

  // --- Run（AIH-020 / AIH-021 / AIH-022） --------------------------------

  /// 创建 Run：立即返回 runId（后端 202），真正的执行在后台。
  ///
  /// [reasoningEffort] 是思考强度（AIH-056）：`off/low/medium/high/max`（界面上的
  /// 「默认」就是 `off`，wire 名保持不动），后端会按模型目录复核——模型没声明推理能力
  /// 就直接拒绝，不会悄悄忽略。**传 `null` = 默认**：一个思考字段都不发，由网关决定。
  ///
  /// [attachmentIds] 是这次要发给模型的附件（M3）：**准入判定在创建 Run 之前完成**，
  /// 不通过会直接 400，不会产生任何上游请求（AIH-030）。
  Future<AiRunStart> startRun(
    String conversationId, {
    required String text,
    required String providerId,
    required String modelId,
    String? reasoningEffort,
    List<String> attachmentIds = const [],
    String? retryOfRunId,
  }) async =>
      AiRunStart.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/ai/conversations/$conversationId/runs', {
        'text': text,
        'providerId': providerId,
        'modelId': modelId,
        'reasoningEffort': ?reasoningEffort,
        if (attachmentIds.isNotEmpty) 'attachmentIds': attachmentIds,
        'retryOfRunId': ?retryOfRunId,
      }) as Map));

  Future<void> cancelRun(String runId) async => _send('POST', '/api/ai/runs/$runId/cancel');

  /// ComfyUI 实时进度（用户"其他建议"第 1 条）：队列状态 + 最近提交的任务。
  ///
  /// 走的是同一个后端的 `/api/capture/jobs`（不是 `/api/ai/*`），所以用同一个客户端实例、
  /// 同一套错误处理；**不接受任意 URL**，和工具层一样只认应用配置里的 ComfyUI 地址。
  Future<AiComfyJobs> comfyJobs() async =>
      AiComfyJobs.fromJson(Map<String, dynamic>.from(await _get('/api/capture/jobs') as Map));

  /// 订阅统一事件流（SSE）。`after` 用于断线续传。
  Stream<AiRunEvent> runEvents(String runId, {int after = 0}) async* {
    final req = http.Request('GET', _uri('/api/ai/runs/$runId/events?after=$after'))
      ..headers['Accept'] = 'text/event-stream';
    // 事件流也是这个后端的一个请求：手机带令牌连桌面时才不会被闸门挡在 403
    // （不传 tokenProvider 时这里与以前完全一样，只有 Accept 一个头）
    final token = tokenProvider?.call();
    if (token != null && token.isNotEmpty) req.headers['X-ComfyHub-Token'] = token;
    final res = await _client.send(req);
    if (res.statusCode != 200) {
      final body = await res.stream.bytesToString();
      throw AiApiException(res.statusCode, body.isEmpty ? '事件流连接失败' : body);
    }
    String? event;
    final data = StringBuffer();
    await for (final line in res.stream.transform(utf8.decoder).transform(const LineSplitter())) {
      if (line.isEmpty) {
        if (event != null || data.isNotEmpty) {
          Map<String, dynamic> parsed = const {};
          final raw = data.toString().trim();
          if (raw.isNotEmpty && raw != '{}') {
            try {
              final decoded = jsonDecode(raw);
              if (decoded is Map) parsed = Map<String, dynamic>.from(decoded);
            } catch (_) {
              parsed = const {};
            }
          }
          yield AiRunEvent(event ?? 'message', parsed);
        }
        event = null;
        data.clear();
        continue;
      }
      if (line.startsWith(':')) continue;
      if (line.startsWith('event:')) {
        event = line.substring(6).trim();
      } else if (line.startsWith('data:')) {
        if (data.isNotEmpty) data.write('\n');
        data.write(line.substring(5).trimLeft());
      }
    }
  }
}

class AiApiException implements Exception {
  final int statusCode;
  final String message;

  AiApiException(this.statusCode, this.message);

  @override
  String toString() => message;
}
