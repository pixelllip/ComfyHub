import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../models/models.dart';

/// 后端返回的错误
class ApiException implements Exception {
  final int statusCode;
  final String message;
  final String? detail;

  ApiException(this.statusCode, this.message, [this.detail]);

  @override
  String toString() =>
      detail == null || detail!.isEmpty ? message : '$message ($detail)';
}

/// ComfyHub Kotlin 后端的 REST 客户端。
class ApiClient {
  final String baseUrl;
  final http.Client _client;

  ApiClient(this.baseUrl, {http.Client? client}) : _client = client ?? http.Client();

  void dispose() => _client.close();

  Uri _uri(String path, [Map<String, dynamic>? query]) {
    final q = <String, String>{};
    query?.forEach((k, v) {
      if (v == null) return;
      final s = v.toString();
      if (s.isEmpty) return;
      q[k] = s;
    });
    return Uri.parse('$baseUrl$path').replace(queryParameters: q.isEmpty ? null : q);
  }

  /// 把后端的相对路径（/api/media/1/file）拼成完整 URL
  String absolute(String relative) =>
      relative.startsWith('http') ? relative : '$baseUrl$relative';

  Map<String, String> get _jsonHeaders => const {
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

    final msg = (json is Map && json['error'] != null)
        ? json['error'].toString()
        : 'HTTP ${res.statusCode}';
    final detail = (json is Map && json['detail'] != null) ? json['detail'].toString() : null;
    throw ApiException(res.statusCode, msg, detail);
  }

  Future<dynamic> _get(String path, [Map<String, dynamic>? query]) async {
    try {
      return _decode(await _client.get(_uri(path, query)));
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  Future<dynamic> _send(String method, String path, Object? body) async {
    final req = http.Request(method, _uri(path))
      ..headers.addAll(_jsonHeaders);
    if (body != null) req.body = jsonEncode(body);
    try {
      final streamed = await _client.send(req);
      return _decode(await http.Response.fromStream(streamed));
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  // -------------------------------------------------------------------------
  //  健康检查 / 统计
  // -------------------------------------------------------------------------

  Future<Map<String, dynamic>> health() async =>
      Map<String, dynamic>.from(await _get('/api/health') as Map);

  Future<LibraryStats> stats() async =>
      LibraryStats.fromJson(Map<String, dynamic>.from(await _get('/api/stats') as Map));

  // -------------------------------------------------------------------------
  //  提示词
  // -------------------------------------------------------------------------

  Future<Paged<Prompt>> listPrompts({
    String? q,
    List<String> tags = const [],
    String tagMode = 'any',
    String? kind,
    bool? favorite,
    bool? hasMedia,
    String sort = 'newest',
    int page = 1,
    int size = 20,
  }) async {
    final json = await _get('/api/prompts', {
      'q': q,
      'tags': tags.isEmpty ? null : tags.join(','),
      'tagMode': tagMode,
      'kind': kind,
      'favorite': favorite == null ? null : (favorite ? '1' : '0'),
      'hasMedia': hasMedia == null ? null : (hasMedia ? '1' : '0'),
      'sort': sort,
      'page': page,
      'size': size,
    });
    return Paged.fromJson(
      Map<String, dynamic>.from(json as Map),
      (m) => Prompt.fromJson(m),
    );
  }

  Future<Prompt> getPrompt(int id) async =>
      Prompt.fromJson(Map<String, dynamic>.from(await _get('/api/prompts/$id') as Map));

  Future<Prompt> createPrompt(Prompt p) async => Prompt.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/prompts', p.toInputJson()) as Map));

  Future<Prompt> updatePrompt(Prompt p) async => Prompt.fromJson(Map<String, dynamic>.from(
      await _send('PUT', '/api/prompts/${p.id}', p.toInputJson()) as Map));

  Future<void> deletePrompt(int id) async => _send('DELETE', '/api/prompts/$id', null);

  Future<Prompt> duplicatePrompt(int id) async => Prompt.fromJson(Map<String, dynamic>.from(
      await _send('POST', '/api/prompts/$id/duplicate', null) as Map));

  Future<void> setPromptFavorite(int id, bool favorite) async =>
      _send('POST', '/api/prompts/$id/favorite', {'favorite': favorite});

  Future<Prompt> addPromptTags(int id, List<String> tags) async =>
      Prompt.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/prompts/$id/tags', {'tags': tags}) as Map));

  Future<Prompt> removePromptTag(int promptId, int tagId) async =>
      Prompt.fromJson(Map<String, dynamic>.from(
          await _send('DELETE', '/api/prompts/$promptId/tags/$tagId', null) as Map));

  Future<List<MediaAsset>> promptMedia(int id) async {
    final json = await _get('/api/prompts/$id/media') as List;
    return json.map((e) => MediaAsset.fromJson(Map<String, dynamic>.from(e as Map))).toList();
  }

  // -------------------------------------------------------------------------
  //  标签
  // -------------------------------------------------------------------------

  Future<List<Tag>> listTags({String? q, String? category, String sort = 'popular', int limit = 500}) async {
    final json = await _get('/api/tags', {
      'q': q,
      'category': category,
      'sort': sort,
      'limit': limit,
    }) as List;
    return json.map((e) => Tag.fromJson(Map<String, dynamic>.from(e as Map))).toList();
  }

  Future<List<String>> tagCategories() async =>
      (await _get('/api/tags/categories') as List).map((e) => e.toString()).toList();

  Future<Tag> createTag(String name, {String? category, String? color, String? description}) async =>
      Tag.fromJson(Map<String, dynamic>.from(await _send('POST', '/api/tags', {
        'name': name,
        'category': category,
        'color': color,
        'description': description,
      }) as Map));

  Future<Tag> updateTag(int id, String name, {String? category, String? color, String? description}) async =>
      Tag.fromJson(Map<String, dynamic>.from(await _send('PUT', '/api/tags/$id', {
        'name': name,
        'category': category,
        'color': color,
        'description': description,
      }) as Map));

  Future<void> deleteTag(int id) async => _send('DELETE', '/api/tags/$id', null);

  // -------------------------------------------------------------------------
  //  生成产物
  // -------------------------------------------------------------------------

  Future<Paged<MediaAsset>> listMedia({
    String? q,
    List<String> tags = const [],
    String tagMode = 'any',
    String? kind,
    int? promptId,
    bool? favorite,
    bool untagged = false,
    String sort = 'newest',
    int page = 1,
    int size = 24,
  }) async {
    final json = await _get('/api/media', {
      'q': q,
      'tags': tags.isEmpty ? null : tags.join(','),
      'tagMode': tagMode,
      'kind': kind,
      'promptId': promptId,
      'favorite': favorite == null ? null : (favorite ? '1' : '0'),
      'untagged': untagged ? '1' : null,
      'sort': sort,
      'page': page,
      'size': size,
    });
    return Paged.fromJson(
      Map<String, dynamic>.from(json as Map),
      (m) => MediaAsset.fromJson(m),
    );
  }

  Future<MediaAsset> getMedia(int id) async =>
      MediaAsset.fromJson(Map<String, dynamic>.from(await _get('/api/media/$id') as Map));

  /// 视频封面（第一帧）的完整地址。
  ///
  /// 走后端的 `GET /api/media/{id}/poster`（Windows 缩略图管线抽帧 + 缓存），
  /// 非视频 / 抽不出来时后端回 204，界面该退化成占位图标。画廊格子与详情页共用。
  String mediaPosterUrl(int id) => '$baseUrl/api/media/$id/poster';

  Future<Prompt?> mediaPrompt(int id) async {
    final res = await _client.get(_uri('/api/media/$id/prompt'));
    if (res.statusCode == 204) return null;
    if (res.statusCode >= 200 && res.statusCode < 300) {
      return Prompt.fromJson(
          Map<String, dynamic>.from(jsonDecode(utf8.decode(res.bodyBytes)) as Map));
    }
    _decode(res);
    return null;
  }

  Future<MediaAsset> updateMedia(
    int id, {
    int? promptId,
    bool clearPrompt = false,
    String? title,
    String? notes,
    bool? favorite,
    String? source,
  }) async =>
      MediaAsset.fromJson(Map<String, dynamic>.from(await _send('PATCH', '/api/media/$id', {
        if (clearPrompt) 'clearPrompt': true,
        if (!clearPrompt && promptId != null) 'promptId': promptId,
        // Dart 的 null-aware map value：值为 null 时整条不写入
        'title': ?title,
        'notes': ?notes,
        'favorite': ?favorite,
        'source': ?source,
      }) as Map));

  Future<void> deleteMedia(int id) async => _send('DELETE', '/api/media/$id', null);

  /// 上传若干文件；[promptId] 为空表示"暂不关联"。
  Future<UploadResult> uploadMedia({
    required List<String> filePaths,
    int? promptId,
    String? kind,
    String? title,
    String? source,
    String? notes,
    List<String> tags = const [],
  }) async {
    final req = http.MultipartRequest('POST', _uri('/api/media/upload'));
    if (promptId != null) req.fields['promptId'] = promptId.toString();
    if (kind != null) req.fields['kind'] = kind;
    if (title != null && title.isNotEmpty) req.fields['title'] = title;
    if (source != null && source.isNotEmpty) req.fields['source'] = source;
    if (notes != null && notes.isNotEmpty) req.fields['notes'] = notes;
    if (tags.isNotEmpty) req.fields['tags'] = tags.join(',');

    for (final path in filePaths) {
      req.files.add(await http.MultipartFile.fromPath('files', path));
    }

    try {
      final streamed = await _client.send(req);
      final res = await http.Response.fromStream(streamed);
      return UploadResult.fromJson(
          Map<String, dynamic>.from(_decode(res) as Map));
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  // -------------------------------------------------------------------------
  //  ComfyUI 自动捕获
  // -------------------------------------------------------------------------

  Future<CaptureConfig> captureConfig() async =>
      CaptureConfig.fromJson(Map<String, dynamic>.from(await _get('/api/capture/config') as Map));

  Future<CaptureConfig> updateCaptureConfig(CaptureConfig config) async =>
      CaptureConfig.fromJson(Map<String, dynamic>.from(
          await _send('PUT', '/api/capture/config', config.toJson()) as Map));

  Future<CaptureStatus> captureStatus() async =>
      CaptureStatus.fromJson(Map<String, dynamic>.from(await _get('/api/capture/status') as Map));

  /// 探测 ComfyUI 装在哪（用户"其他建议"第 3 条）。**只读**：不会自动改配置。
  Future<ComfyLocation> locateComfy() async =>
      ComfyLocation.fromJson(Map<String, dynamic>.from(await _get('/api/capture/locate') as Map));

  /// 把探测到的输出目录写进配置（用户点了「使用这个目录」）。
  Future<CaptureConfig> applyComfyLocation(String outputDir) async =>
      CaptureConfig.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/capture/locate/apply', {'outputDir': outputDir}) as Map));

  /// 立刻轮询一次 ComfyUI 的 /history（App 里点「立即同步」时用）
  Future<CapturePollResult> pollCapture() async => CapturePollResult.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/capture/poll', null) as Map));

  /// 导入某个目录里历史上已经生成好的产物
  Future<ImportFolderResult> importCaptureFolder({
    required String dir,
    bool recursive = true,
    int limit = 200,
    bool linkWorkflow = true,
    List<String> tags = const [],
  }) async =>
      ImportFolderResult.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/capture/import', {
        'dir': dir,
        'recursive': recursive,
        'limit': limit,
        'linkWorkflow': linkWorkflow,
        'tags': tags,
      }) as Map));

  /// 把**本机 ComfyUI 里已经保存的工作流文件**批量读进库（用户 bug ⑤）。
  ///
  /// `dir` 不传就自动发现本机 ComfyUI 的 `user\<用户>\workflows` 目录 ——
  /// 过去这些工作流只有"在 ComfyUI 里跑过一次被捕获"或"AI 手动按路径读"才会进库，
  /// 首次使用时库里是空的。
  Future<ImportWorkflowsResult> importWorkflowFiles({String? dir, int limit = 200}) async =>
      ImportWorkflowsResult.fromJson(Map<String, dynamic>.from(
          await _send('POST', '/api/capture/import-workflows', {
        if (dir != null && dir.isNotEmpty) 'dir': dir,
        'limit': limit,
      }) as Map));

  /// 提示词对应的完整工作流 JSON；没存过返回 null
  Future<String?> promptWorkflow(int promptId) => _workflowText('/api/prompts/$promptId/workflow');

  /// 产物对应的完整工作流 JSON；没存过返回 null
  Future<String?> mediaWorkflow(int mediaId) => _workflowText('/api/media/$mediaId/workflow');

  Future<String?> _workflowText(String path) async {
    try {
      final res = await _client.get(_uri(path)).timeout(const Duration(seconds: 30));
      if (res.statusCode == 204 || res.statusCode == 404) return null;
      if (res.statusCode >= 200 && res.statusCode < 300) {
        return utf8.decode(res.bodyBytes);
      }
      _decode(res);
      return null;
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }
}
