import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../models/backup_models.dart';
import '../models/models.dart';
import '../models/storage_models.dart';
import '../models/train_models.dart';
import 'pairing_token.dart';
import 'workflow_blueprint.dart';

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

  /// **配对令牌**（`X-ComfyHub-Token`）的取值回调（`docs/pairing-design.md` §11）。
  ///
  /// **默认就是"每次现取进程内那一份"**（[PairingTokenCache.read]）—— 这是刻意的：
  /// 闸门一开，不带令牌的请求在局域网上一律 403，而"某个页面忘了接线"这种漏法
  /// 2026-09-24 真机验收真的发生过（「AI 模型与凭据」页一条红横幅 `需要配对令牌`，
  /// 而同一个 App 里的 AI 工作台带着令牌跑得好好的）。
  /// 做成默认值之后，**新增页面什么都不用记得**。
  ///
  /// 桌面侧它恒为 `null`（没有任何东西会写那个缓存）→ 请求与以前**逐字节相同**。
  final String? Function()? tokenProvider;

  ApiClient(
    this.baseUrl, {
    http.Client? client,
    String? Function()? tokenProvider,
  }) : tokenProvider = tokenProvider ?? PairingTokenCache.read,
       _client = client ?? http.Client();

  void dispose() => _client.close();

  /// 只有"确实有令牌"时才该加的那个头；没有就回 `null`（= 与以前逐字节相同）。
  Map<String, String>? _authHeaders() {
    final token = tokenProvider?.call();
    if (token == null || token.isEmpty) return null;
    return {'X-ComfyHub-Token': token};
  }

  Uri _uri(String path, [Map<String, dynamic>? query]) {
    final q = <String, String>{};
    query?.forEach((k, v) {
      if (v == null) return;
      final s = v.toString();
      if (s.isEmpty) return;
      q[k] = s;
    });
    return Uri.parse('$baseUrl$path')
        .replace(queryParameters: q.isEmpty ? null : q);
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
    final detail = (json is Map && json['detail'] != null)
        ? json['detail'].toString()
        : null;
    throw ApiException(res.statusCode, msg, detail);
  }

  Future<dynamic> _get(String path, [Map<String, dynamic>? query]) async {
    try {
      return _decode(
        await _client.get(_uri(path, query), headers: _authHeaders()),
      );
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  /// 下载产物原件 / 缩略图（手机端缓存用）。
  ///
  /// 与 `_get` 共用**同一个令牌来源**：媒体 URL 在远程访问时同样要过配对闸门。
  /// 非 2xx 一律抛 [ApiException]，**不返回半截字节**（缓存层据此不落盘）。
  Future<List<int>> downloadBytes(String url) async {
    try {
      final res = await _client.get(
        Uri.parse(absolute(url)),
        headers: _authHeaders(),
      );
      if (res.statusCode >= 200 && res.statusCode < 300) {
        return res.bodyBytes;
      }
      throw ApiException(res.statusCode, '下载产物失败（HTTP ${res.statusCode}）');
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  /// 下载产物**直接落盘**（流式，不把整份塞进内存）。
  ///
  /// 为什么要有它：[downloadBytes] 会把整个响应读成 `List<int>`，而手机端缓存的原件
  /// 可能是**几百 MB 的视频** —— 那样下载等于赌堆上限。这里 `send` 之后一段一段
  /// 写进 [target]，内存占用只有缓冲区大小。
  ///
  /// 三条与 [downloadBytes] 一致的纪律：同一个令牌来源（远程访问要过配对闸门）、
  /// 非 2xx 抛 [ApiException]、**失败不留半截文件**（调用方负责删 `.part`）。
  ///
  /// @return 实际落盘的字节数。
  Future<int> downloadToFile(String url, File target) async {
    final request = http.Request('GET', Uri.parse(absolute(url)));
    final headers = _authHeaders();
    if (headers != null) request.headers.addAll(headers);

    final http.StreamedResponse res;
    try {
      res = await _client.send(request);
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
    if (res.statusCode < 200 || res.statusCode >= 300) {
      // 排空响应体：不读走的话连接会悬着，下一次请求可能排在它后面。
      await res.stream.drain<void>();
      throw ApiException(res.statusCode, '下载产物失败（HTTP ${res.statusCode}）');
    }
    final sink = target.openWrite();
    try {
      // `pipe` 结束时会把 sink 关掉；中途出错自己收尾，别留一个没关的写句柄。
      await res.stream.pipe(sink);
    } catch (_) {
      try {
        await sink.close();
      } catch (_) {
        // sink 已经坏掉了，忽略
      }
      rethrow;
    }
    return target.length();
  }

  Future<dynamic> _send(String method, String path, Object? body) async {
    final req = http.Request(method, _uri(path))
      ..headers.addAll(_jsonHeaders)
      ..headers.addAll(_authHeaders() ?? const {});
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

  /// **后端受不受限**（写不写得进项目外）—— 连判据一起给，见 [ConfinementInfo]。
  ///
  /// ⚠ 老后端没有这个端点（404）：那就当成"不知道"（回 `null`），
  /// **不许当成"不受限"** —— 那会让横幅永远不亮，正是这次要解决的问题。
  Future<ConfinementInfo?> confinement() async {
    try {
      final json = await _get('/api/system/confinement');
      if (json is! Map) return null;
      return ConfinementInfo.fromJson(Map<String, dynamic>.from(json));
    } on ApiException catch (e) {
      if (e.statusCode == 404) return null;
      rethrow;
    }
  }

  Future<LibraryStats> stats() async => LibraryStats.fromJson(
    Map<String, dynamic>.from(await _get('/api/stats') as Map),
  );

  /// **这次出网走不走代理** —— 连"这份设置从哪来"一起给，见 [OutboundInfo]。
  ///
  /// [probe] 为真时后端会真的向 Civitai / Danbooru 各发一次请求
  /// （目标是后端写死的，界面不能传 URL）—— 只有它能把"配置写对了"
  /// 和"配置写对了但这条链路其实不通"分开。所以只在用户点「测试出网」时用。
  Future<OutboundInfo> outbound({bool probe = false}) async =>
      OutboundInfo.fromJson(
        Map<String, dynamic>.from(
          await _get('/api/system/outbound${probe ? '?probe=1' : ''}') as Map,
        ),
      );

  // -------------------------------------------------------------------------
  //  备份 / 恢复（`server-rs/src/web/backup.rs`）
  // -------------------------------------------------------------------------

  Future<BackupListing> listBackups() async => BackupListing.fromJson(
    Map<String, dynamic>.from(await _get('/api/system/backups') as Map),
  );

  /// 现在拍一张（后端用 `VACUUM INTO`，不会打断正在跑的请求）。
  Future<BackupItem> createBackup() async {
    final json = Map<String, dynamic>.from(
      await _send('POST', '/api/system/backups', null) as Map,
    );
    return BackupItem.fromJson(Map<String, dynamic>.from(json['item'] as Map));
  }

  Future<void> deleteBackup(String name) async =>
      _send('DELETE', '/api/system/backups/${Uri.encodeComponent(name)}', null);

  /// 校验 + 拍保险 + 落标记；**真正换库要重启后端**（响应里的 `restartRequired`）。
  Future<RestorePreparation> restoreBackup(String name) async =>
      RestorePreparation.fromJson(
        Map<String, dynamic>.from(
          await _send('POST', '/api/system/restore', {'name': name}) as Map,
        ),
      );

  // -------------------------------------------------------------------------
  //  存储 / 孤儿 / 事件归档（`server-rs/src/web/storage.rs`）
  // -------------------------------------------------------------------------

  Future<StorageUsage> storageUsage() async => StorageUsage.fromJson(
    Map<String, dynamic>.from(await _get('/api/system/storage') as Map),
  );

  /// **只扫描不删**（`apply` 缺省 false）。删之前先看一遍数量与体积。
  ///
  /// ⚠ 必须发一个 `{}`：后端用的是 `Json<OrphanInput>`，无 body 的 POST 会被
  /// 415 / 400 拒掉（`createBackup` 那条没有 body 提取器，才能传 null）。
  Future<OrphanScan> scanOrphans() async => OrphanScan.fromJson(
    Map<String, dynamic>.from(
      await _send(
        'POST',
        '/api/system/storage/orphans',
        const <String, dynamic>{},
      ) as Map,
    ),
  );

  Future<CleanReport> cleanOrphans() async => CleanReport.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/system/storage/orphans', {'apply': true})
          as Map,
    ),
  );

  /// 把 `days` 天之前的事件写成 JSONL 再删库。
  Future<EventArchiveReport> archiveEvents({int days = 30}) async {
    final json = Map<String, dynamic>.from(
      await _send('POST', '/api/system/events/archive', {'days': days}) as Map,
    );
    return EventArchiveReport.fromJson(
      Map<String, dynamic>.from(json['report'] as Map),
    );
  }

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

  Future<Prompt> getPrompt(int id) async => Prompt.fromJson(
    Map<String, dynamic>.from(await _get('/api/prompts/$id') as Map),
  );

  Future<Prompt> createPrompt(Prompt p) async => Prompt.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/prompts', p.toInputJson()) as Map,
    ),
  );

  Future<Prompt> updatePrompt(Prompt p) async => Prompt.fromJson(
    Map<String, dynamic>.from(
      await _send('PUT', '/api/prompts/${p.id}', p.toInputJson()) as Map,
    ),
  );

  Future<void> deletePrompt(int id) async =>
      _send('DELETE', '/api/prompts/$id', null);

  Future<Prompt> duplicatePrompt(int id) async => Prompt.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/prompts/$id/duplicate', null) as Map,
    ),
  );

  Future<void> setPromptFavorite(int id, bool favorite) async =>
      _send('POST', '/api/prompts/$id/favorite', {'favorite': favorite});

  Future<Prompt> addPromptTags(int id, List<String> tags) async =>
      Prompt.fromJson(
        Map<String, dynamic>.from(
          await _send('POST', '/api/prompts/$id/tags', {'tags': tags}) as Map,
        ),
      );

  Future<Prompt> removePromptTag(int promptId, int tagId) async =>
      Prompt.fromJson(
        Map<String, dynamic>.from(
          await _send('DELETE', '/api/prompts/$promptId/tags/$tagId', null)
              as Map,
        ),
      );

  Future<List<MediaAsset>> promptMedia(int id) async {
    final json = await _get('/api/prompts/$id/media') as List;
    return json
        .map((e) => MediaAsset.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();
  }

  /// 与这条**预设**同款提示词的产物（按归一化后的提示词匹配，2026-10-11）。
  ///
  /// 与 [promptMedia]（只认 `prompt_id` 直连）的区别：自动捕获的产物指向的是
  /// **另一条**提示词行，所以直连会漏掉它们 —— 而"用这套预设同款提示词的产物"
  /// 正是用户要看到的那些。
  Future<List<MediaAsset>> presetMedia(int id) async {
    final json = await _get('/api/presets/$id/media') as List;
    return json
        .map((e) => MediaAsset.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();
  }

  // -------------------------------------------------------------------------
  //  标签
  // -------------------------------------------------------------------------

  Future<List<Tag>> listTags({
    String? q,
    String? category,
    String sort = 'popular',
    int limit = 500,
  }) async {
    final json = await _get('/api/tags', {
      'q': q,
      'category': category,
      'sort': sort,
      'limit': limit,
    }) as List;
    return json
        .map((e) => Tag.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();
  }

  Future<List<String>> tagCategories() async =>
      (await _get('/api/tags/categories') as List)
          .map((e) => e.toString())
          .toList();

  Future<Tag> createTag(
    String name, {
    String? category,
    String? color,
    String? description,
  }) async => Tag.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/tags', {
        'name': name,
        'category': category,
        'color': color,
        'description': description,
      }) as Map,
    ),
  );

  Future<Tag> updateTag(
    int id,
    String name, {
    String? category,
    String? color,
    String? description,
  }) async => Tag.fromJson(
    Map<String, dynamic>.from(
      await _send('PUT', '/api/tags/$id', {
        'name': name,
        'category': category,
        'color': color,
        'description': description,
      }) as Map,
    ),
  );

  Future<void> deleteTag(int id) async =>
      _send('DELETE', '/api/tags/$id', null);

  // -------------------------------------------------------------------------
  //  生成产物
  // -------------------------------------------------------------------------

  Future<Paged<MediaAsset>> listMedia({
    String? q,
    List<String> tags = const [],
    String tagMode = 'any',
    String? kind,
    int? promptId,
    int? collectionId,

    /// **折叠集合**：一个集合只回一条（最早那条当代表），画廊据此显示成"一组"。
    bool collapse = false,
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
      'collectionId': collectionId,
      'collapse': collapse ? '1' : null,
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

  Future<MediaAsset> getMedia(int id) async => MediaAsset.fromJson(
    Map<String, dynamic>.from(await _get('/api/media/$id') as Map),
  );

  /// 视频封面（第一帧）的完整地址。
  ///
  /// 走后端的 `GET /api/media/{id}/poster`（Windows 缩略图管线抽帧 + 缓存），
  /// 非视频 / 抽不出来时后端回 204，界面该退化成占位图标。画廊格子与详情页共用。
  String mediaPosterUrl(int id) => '$baseUrl/api/media/$id/poster';

  Future<Prompt?> mediaPrompt(int id) async {
    final res = await _client.get(
      _uri('/api/media/$id/prompt'),
      headers: _authHeaders(),
    );
    if (res.statusCode == 204) return null;
    if (res.statusCode >= 200 && res.statusCode < 300) {
      return Prompt.fromJson(
        Map<String, dynamic>.from(
          jsonDecode(utf8.decode(res.bodyBytes)) as Map,
        ),
      );
    }
    _decode(res);
    return null;
  }

  /// 改一条产物的元信息。
  ///
  /// 归属（集合）两组参数与 `promptId` / `clearPrompt` **同形**（2026-10-18）：
  /// * `collectionId` = 手动**移进**那个集合（后端会校验 kind 一致，不一致回 400）；
  /// * `clearCollection` = 手动**移出**（后端同时打上 `collection_opt_out`，
  ///   否则下次启动的回填会把它静默吸回原组）。
  Future<MediaAsset> updateMedia(
    int id, {
    int? promptId,
    bool clearPrompt = false,
    String? title,
    String? notes,
    bool? favorite,
    String? source,

    /// 标记 / 取消标记成人内容（用户 bug 2026-09-20：模糊只作用于标记过的条目）
    bool? nsfw,
    int? collectionId,
    bool clearCollection = false,
  }) async => MediaAsset.fromJson(
    Map<String, dynamic>.from(
      await _send('PATCH', '/api/media/$id', {
        if (clearPrompt) 'clearPrompt': true,
        if (!clearPrompt && promptId != null) 'promptId': promptId,
        if (clearCollection) 'clearCollection': true,
        if (!clearCollection && collectionId != null)
          'collectionId': collectionId,
        // Dart 的 null-aware map value：值为 null 时整条不写入
        'title': ?title,
        'notes': ?notes,
        'favorite': ?favorite,
        'source': ?source,
        'nsfw': ?nsfw,
      }) as Map,
    ),
  );

  Future<void> deleteMedia(int id) async =>
      _send('DELETE', '/api/media/$id', null);

  /// **导出产物**（2026-10-27）：把原件 + 绑定的工作流 / 提示词信息写进 [dir]。
  ///
  /// 三种范围**互斥**（与后端优先级一致：`mediaIds` > `collectionId` > `favorites`）：
  /// * [mediaIds] 非空 = 只导这几条（画廊多选）；
  /// * [collectionId] = 只导这个集合；
  /// * [favorites] = 只导收藏；
  /// * 都不给 = 全部。
  ///
  /// ⚠ 后端只允许**本机回环**调用（它会往任意绝对路径写文件），手机端不要接这颗按钮。
  Future<MediaExportReport> exportMedia({
    required String dir,
    List<int>? mediaIds,
    int? collectionId,
    bool favorites = false,
    bool includeWorkflow = true,
    bool includeApiWorkflow = true,
  }) async {
    final json = await _send('POST', '/api/media/export', {
      'dir': dir,
      'mediaIds': ?mediaIds,
      'collectionId': ?collectionId,
      if (favorites) 'favorites': true,
      'includeWorkflow': includeWorkflow,
      'includeApiWorkflow': includeApiWorkflow,
    });
    return MediaExportReport.fromJson(Map<String, dynamic>.from(json as Map));
  }

  /// 这条产物**在磁盘上的绝对路径**。
  ///
  /// 「复制文件本身」（写 `CF_HDROP`）要它。⚠ **不要**在前端自己拼
  /// `<storage>\media\<storedName>` —— 路径解析只有后端一份真源，
  /// 前端拼等于长出第二份（AGENTS §7）。
  ///
  /// 文件不在盘上时后端回 404（调用方要如实说"文件不见了"）。
  Future<String> mediaPath(int id) async {
    final json = await _get('/api/media/$id/path');
    return (Map<String, dynamic>.from(json as Map)['path'] ?? '').toString();
  }

  /// **重新自动整理**：把库里已存的产物按当前规则重算一遍分组。
  ///
  /// ⚠ 后端**只重算算法分出来的组**（`origin='auto'`）：用户新建的 / 被用户改过的集合、
  /// 以及"用户已移出"的产物一个都不动。这条纪律在前端**不许绕**（别在本地先拆再合）。
  ///
  /// 返回这次重算了多少张。
  Future<int> regroupCollections() async {
    final json = await _send(
      'POST',
      '/api/media/collections/regroup',
      const {},
    );
    return (Map<String, dynamic>.from(json as Map)['regrouped'] as num?)
            ?.toInt() ??
        0;
  }

  /// 所有集合（手动在前）——「移到集合…」的选择器吃它。
  Future<List<MediaCollection>> listCollections() async {
    final raw = await _get('/api/media/collections');
    return ((raw as List?) ?? const [])
        .map(
          (e) => MediaCollection.fromJson(Map<String, dynamic>.from(e as Map)),
        )
        .toList(growable: false);
  }

  /// **新建一个手动集合**，并把 [mediaIds] 一起移进去。
  ///
  /// 400 的两种情形都要如实显示给用户（后端的原话）：
  /// ① 选中的产物 kind 不一致；② 一条都没选。
  Future<MediaCollection> createCollection(
    List<int> mediaIds, {
    String? title,
  }) async {
    final json = await _send('POST', '/api/media/collections', {
      'mediaIds': mediaIds,
      if (title != null && title.trim().isNotEmpty) 'title': title.trim(),
    });
    return MediaCollection.fromJson(Map<String, dynamic>.from(json as Map));
  }

  /// **给集合改名**（集合详情里的「重命名」）。
  ///
  /// 后端会顺手把这一组转成手动（`origin = manual`）：用户起了名字，它就归用户，
  /// 下次「重新自动整理」不会把组连同名字一起解散。
  Future<MediaCollection> renameCollection(int id, String title) async {
    final json = await _send('PATCH', '/api/media/collections/$id', {
      'title': title.trim(),
    });
    return MediaCollection.fromJson(Map<String, dynamic>.from(json as Map));
  }

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
    req.headers.addAll(_authHeaders() ?? const {});
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
        Map<String, dynamic>.from(_decode(res) as Map),
      );
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接后端 $baseUrl', e.message);
    }
  }

  // -------------------------------------------------------------------------
  //  ComfyUI 自动捕获
  // -------------------------------------------------------------------------

  Future<CaptureConfig> captureConfig() async => CaptureConfig.fromJson(
    Map<String, dynamic>.from(await _get('/api/capture/config') as Map),
  );

  Future<CaptureConfig> updateCaptureConfig(CaptureConfig config) async =>
      CaptureConfig.fromJson(
        Map<String, dynamic>.from(
          await _send('PUT', '/api/capture/config', config.toJson()) as Map,
        ),
      );

  Future<CaptureStatus> captureStatus() async => CaptureStatus.fromJson(
    Map<String, dynamic>.from(await _get('/api/capture/status') as Map),
  );

  /// 探测 ComfyUI 装在哪（用户"其他建议"第 3 条）。**只读**：不会自动改配置。
  Future<ComfyLocation> locateComfy() async => ComfyLocation.fromJson(
    Map<String, dynamic>.from(await _get('/api/capture/locate') as Map),
  );

  /// 把探测到的输出目录写进配置（用户点了「使用这个目录」）。
  Future<CaptureConfig> applyComfyLocation(String outputDir) async =>
      CaptureConfig.fromJson(
        Map<String, dynamic>.from(
          await _send('POST', '/api/capture/locate/apply', {
            'outputDir': outputDir,
          }) as Map,
        ),
      );

  /// 立刻轮询一次 ComfyUI 的 /history（App 里点「立即同步」时用）
  Future<CapturePollResult> pollCapture() async => CapturePollResult.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/capture/poll', null) as Map,
    ),
  );

  /// 导入某个目录里历史上已经生成好的产物
  Future<ImportFolderResult> importCaptureFolder({
    required String dir,
    bool recursive = true,
    int limit = 200,
    bool linkWorkflow = true,
    List<String> tags = const [],
  }) async => ImportFolderResult.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/capture/import', {
        'dir': dir,
        'recursive': recursive,
        'limit': limit,
        'linkWorkflow': linkWorkflow,
        'tags': tags,
      }) as Map,
    ),
  );

  /// 把**本机 ComfyUI 里已经保存的工作流文件**批量读进库（用户 bug ⑤）。
  ///
  /// `dir` 不传就自动发现本机 ComfyUI 的 `user\<用户>\workflows` 目录 ——
  /// 过去这些工作流只有"在 ComfyUI 里跑过一次被捕获"或"AI 手动按路径读"才会进库，
  /// 首次使用时库里是空的。
  Future<ImportWorkflowsResult> importWorkflowFiles({
    String? dir,
    int limit = 200,
  }) async => ImportWorkflowsResult.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/capture/import-workflows', {
        if (dir != null && dir.isNotEmpty) 'dir': dir,
        'limit': limit,
      }) as Map,
    ),
  );

  /// 本机 ComfyUI 里已经保存的工作流文件（**只读**，`GET /api/capture/workflows`）。
  ///
  /// 「选择现有工作流」用：给用户一个能点、能看蓝图、能让 AI 照着跑的清单。
  /// `q` 按文件名过滤；不传就按最近修改排序。
  Future<ComfyWorkflowList> listWorkflowFiles({
    String? q,
    int limit = 50,
  }) async => ComfyWorkflowList.fromJson(
    Map<String, dynamic>.from(
      await _get('/api/capture/workflows', {'q': q, 'limit': limit}) as Map,
    ),
  );

  /// 读一份本机工作流的原文（`GET /api/capture/workflow?path=…`）。
  ///
  /// 路径准入在后端（读白名单）；前端**不拼盘路径**。文件不存在 / 越界会抛 [ApiException]。
  Future<String> readWorkflowFile(String path) async {
    final json = await _get('/api/capture/workflow', {'path': path});
    final raw = (json is Map) ? json['raw'] : null;
    if (raw is! String) {
      throw ApiException(0, '后端没有返回工作流原文', path);
    }
    return raw;
  }

  /// 提示词对应的完整工作流 JSON；没存过返回 null
  Future<String?> promptWorkflow(int promptId) =>
      _workflowText('/api/prompts/$promptId/workflow');

  /// 产物对应的完整工作流 JSON；没存过返回 null
  Future<String?> mediaWorkflow(int mediaId) =>
      _workflowText('/api/media/$mediaId/workflow');

  Future<String?> _workflowText(String path) async {
    try {
      final res = await _client
          .get(_uri(path), headers: _authHeaders())
          .timeout(const Duration(seconds: 30));
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

  /// 把一份工作流原文交给后端**算成蓝图 + 接线体检**（`POST /api/workflow/blueprint`）。
  ///
  /// 为什么要过一趟后端而不是在 Dart 里解析：判"哪个插口是必填"、"这个类本机有没有"、
  /// "这两根线的类型对不对"都要 ComfyUI 的 `/object_info`，而**只有后端能问它**。
  /// 判据也只有那一份（`workflow_blueprint.rs`）—— 前端再写一套解析，两套迟早分叉。
  ///
  /// 界面在蓝图上改完参数之后**用同一个方法**就地复检（不落库、不改原文件）。
  Future<Map<String, dynamic>> workflowBlueprint(
    String raw, {
    bool diagnose = true,
  }) async => Map<String, dynamic>.from(
    await _send('POST', '/api/workflow/blueprint', {
      'raw': raw,
      'diagnose': diagnose,
    }) as Map,
  );

  /// 把（改过的）工作流存成一份文件，回它的绝对路径。
  ///
  /// 落点由**后端**的白名单决定（`<项目根>\comfyui\workflows\`），前端不拼路径。
  Future<SavedWorkflow> saveWorkflow({
    required String raw,
    String? name,
  }) async => SavedWorkflow.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/workflow/save', {
        'raw': raw,
        if (name != null && name.isNotEmpty) 'name': name,
      }) as Map,
    ),
  );

  // -------------------------------------------------------------------------
  //  本地 LoRA 训练引擎（可选能力）
  // -------------------------------------------------------------------------

  /// 引擎装没装、守护进程 / WebUI 在哪个端口。
  Future<TrainEngineStatus> trainStatus() async => TrainEngineStatus.fromJson(
    Map<String, dynamic>.from(await _get('/api/train/status') as Map),
  );

  /// 起守护进程 + WebUI（幂等）。
  Future<TrainEngineStatus> trainStart() async => TrainEngineStatus.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/train/start', const {}) as Map,
    ),
  );

  /// 停 WebUI + 守护进程；**不会 kill 正在跑的训练**（等它跑完）。
  Future<TrainEngineStatus> trainStop() async => TrainEngineStatus.fromJson(
    Map<String, dynamic>.from(
      await _send('POST', '/api/train/stop', const {}) as Map,
    ),
  );
}
