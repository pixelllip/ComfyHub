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
        : (json is Map && json['error'] != null ? json['error'].toString() : '请求失败');
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
}

class AiApiException implements Exception {
  final int statusCode;
  final String message;

  AiApiException(this.statusCode, this.message);

  @override
  String toString() => message;
}
