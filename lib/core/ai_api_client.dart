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

  AiApiClient(this.baseUrl, {http.Client? client}) : _client = client ?? http.Client();

  void dispose() => _client.close();

  Uri _uri(String path) => Uri.parse('$baseUrl$path');

  static const _headers = {
    'Content-Type': 'application/json; charset=utf-8',
    'Accept': 'application/json',
  };

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
    final message = (json is Map && json['detail'] != null)
        ? json['detail'].toString()
        : (json is Map && json['error'] != null
            ? json['error'].toString()
            // 后端的统一错误体是 {code, message}：带上它，用户才看得懂为什么失败
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

  /// 从 `%USERPROFILE%\.dsh\skills` 导入（用户显式动作；目录不存在时后端报错）。
  Future<AiSkillImportResult> importDshSkills() async => AiSkillImportResult.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/ai/skills/import-dsh') as Map));

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

  // --- 附件预检 ----------------------------------------------------------

  Future<AiPreflightResult> preflight({
    required String providerId,
    required String modelId,
    required List<AiAttachment> attachments,
  }) async =>
      AiPreflightResult.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/ai/preflight', {
        'providerId': providerId,
        'modelId': modelId,
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
  /// [reasoningEffort] 是思考强度（AIH-056）：`off/low/medium/high/max`，
  /// 后端会按模型目录复核——模型没声明推理能力就直接拒绝，不会悄悄忽略。
  Future<AiRunStart> startRun(
    String conversationId, {
    required String text,
    required String providerId,
    required String modelId,
    String? reasoningEffort,
    String? retryOfRunId,
  }) async =>
      AiRunStart.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/ai/conversations/$conversationId/runs', {
        'text': text,
        'providerId': providerId,
        'modelId': modelId,
        'reasoningEffort': ?reasoningEffort,
        'retryOfRunId': ?retryOfRunId,
      }) as Map));

  Future<void> cancelRun(String runId) async => _send('POST', '/api/ai/runs/$runId/cancel');

  /// 订阅统一事件流（SSE）。`after` 用于断线续传。
  Stream<AiRunEvent> runEvents(String runId, {int after = 0}) async* {
    final req = http.Request('GET', _uri('/api/ai/runs/$runId/events?after=$after'))
      ..headers['Accept'] = 'text/event-stream';
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
