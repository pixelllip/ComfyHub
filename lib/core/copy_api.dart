/// AI 文案台的后端客户端（唯一设计真源：`docs/copy-review-design.md` §3 / §4）。
///
/// 与 `AiApiClient` 同一套路（构造传 `baseUrl`、可注入 `http.Client` 与令牌回调），
/// 但**刻意不复用那个类**，两个理由：
///
/// 1. **错误体的读法不一样**：这里要同时留住 `error`（稳定码）与 `detail`（人话），
///    设计 §3/§4 与 `api_error_envelope_test.dart` 盯的就是这个形状 ——
///    扔进 `AiApiException` 那把"只留一句人话"的模子里，码就没了；
/// 2. **这一层要能被离线路径用**：`CopyOverrides` 拉不到覆盖层时必须**静默回退**到
///    Dart 里的默认字面量，所以抛错与返回空的边界要在这里说清楚。
library;

import 'dart:convert';

import 'package:http/http.dart' as http;

import 'pairing_token.dart';

/// 批量修改的六种操作（设计 §4）—— **这是唯一真源**：
/// 界面的下拉项、要显示哪几个输入框、发给后端的 `op` 字面量全部从它派生，
/// 免得"界面多写一个 op、后端不认"这种只有真机能发现的错位。
enum CopyBatchOp {
  replace('replace', usesFind: true, usesReplace: true),
  regexReplace('regex_replace', usesRegex: true, usesReplace: true),
  append('append', usesReplace: true),
  prepend('prepend', usesReplace: true),
  deleteLines('delete_lines', usesFind: true),
  insertAfter('insert_after', usesFind: true, usesReplace: true);

  const CopyBatchOp(
    this.wire, {
    this.usesFind = false,
    this.usesReplace = false,
    this.usesRegex = false,
  });

  /// 发给后端的名字（设计 §4 那六行字面量，**不许改**：改了后端就不认）。
  final String wire;

  /// 这一种 op 需要 `find`。
  final bool usesFind;

  /// 这一种 op 需要 `replace`。
  final bool usesReplace;

  /// 这一种 op 需要 `regex`。
  final bool usesRegex;
}

/// 文案清单里的一条（`GET /api/copy/catalog` / `GET|PUT /api/copy/{key}` / `verify`）。
///
/// 解析是**宽容**的：缺字段回 `null` / 空表，一个默认值都不编 —— 因为"后端没给"
/// 和"后端给了空字符串"在界面上必须能分辨（前者要如实说"没给"，后者要如实显示空）。
class CopyEntry {
  /// `§2` 的命名空间（`prompt.s17` / `tool.comfy_submit.description` / `ui.*` …）。
  /// 清单里它就是主键；后端没给时读成空串（界面据此判定"这条不能改"）。
  final String key;

  /// `prompt` / `tool` / `skill` / `ui`。缺了也有兜底：从 key 的第一段推（见 [scopeOf]）。
  final String scope;

  final String? title;

  /// 默认文本（真源：Rust 常量 / 磁盘文件 / Dart 字面量）。**只读**。
  final String? defaultText;

  /// 当前生效的文本（有覆盖就是覆盖值，没有就是默认值）。
  final String? currentText;

  /// 用户改过（覆盖层里有这一行）。
  final bool overridden;

  /// 写这条覆盖时"默认文本"的 sha256。
  final String? baseHash;

  /// 上游改过默认值 —— 设计 §1 纪律 2：这条**自动变回"待重新校验"**。
  final bool baseChanged;

  /// 人工校验时间（ISO-8601）。文本一改后端就清空它。
  final String? verifiedAt;

  /// 用户自己的备注（"为什么改"）。
  final String? note;

  /// 这条文案在哪被用 / 在哪定义（文件:行 / 端点 / 界面位置）。
  /// **没有它就没法校验**（设计 §3）—— 所以界面上必须显示出来。
  final List<String> refs;

  const CopyEntry({
    required this.key,
    this.scope = '',
    this.title,
    this.defaultText,
    this.currentText,
    this.overridden = false,
    this.baseHash,
    this.baseChanged = false,
    this.verifiedAt,
    this.note,
    this.refs = const [],
  });

  factory CopyEntry.fromJson(Map<String, dynamic> json) => CopyEntry(
        key: _optString(json['key']) ?? '',
        scope: _optString(json['scope']) ?? '',
        title: _optString(json['title']),
        // 设计 §3 的响应字段叫 `default` / `current`，任务书里写的是
        // `defaultText` / `currentText` —— 两个名字都认（成本是一行），
        // 后端不必为了命名分歧改协议，界面也不会因为"少认一个键"而整片空白。
        defaultText: _optString(json['default'] ?? json['defaultText']),
        currentText: _optString(json['current'] ?? json['currentText']),
        // 布尔没有"未知"这一档。缺键读成 false 是**保守**方向：
        // `baseChanged` 缺 → 落在"未校验"（要你看一眼），而不是"已校验"。
        overridden: json['overridden'] == true,
        baseHash: _optString(json['baseHash']),
        baseChanged: json['baseChanged'] == true,
        verifiedAt: _optString(json['verifiedAt']),
        note: _optString(json['note']),
        refs: _optRefs(json['refs']),
      );

  /// 分组用的 scope：后端没给就从 key 的第一段推（`tool.x.y` → `tool`）。
  String get scopeOf {
    if (scope.isNotEmpty) return scope;
    final dot = key.indexOf('.');
    return dot > 0 ? key.substring(0, dot) : '';
  }
}

/// 批量修改里一条的预览 / 结果（设计 §4 的 `items[]`）。
class CopyDiff {
  final String key;

  /// 改之前的文本（后端按 op 算好的那一份）。
  final String? before;

  /// 改之后的文本。
  final String? after;

  /// 后端给的 unified diff（界面直接贴出来，**不在前端另算一份**）。
  final String? diff;

  /// 这一次真的会改到东西吗（find 没命中就是 false —— 不是失败）。
  final bool changed;

  /// 被机器校验拦下的原因（占位符改坏 / R13 白名单 / 长度预算 / 文件不可写…）。
  /// 有它就说明**这一条没改**，且**不连坐别的条**（设计 §4 硬规矩 2）。
  final String? blocked;

  const CopyDiff({
    required this.key,
    this.before,
    this.after,
    this.diff,
    this.changed = false,
    this.blocked,
  });

  factory CopyDiff.fromJson(Map<String, dynamic> json) => CopyDiff(
        key: _optString(json['key']) ?? '',
        before: _optString(json['before']),
        after: _optString(json['after']),
        diff: _optString(json['diff']),
        changed: json['changed'] == true,
        blocked: _optString(json['blocked']),
      );
}

/// 审计里的一行（`GET /api/copy/audit`）：谁、什么时候、把哪条从什么改成了什么。
///
/// ⚠ **形状没有冻结**：设计 §3 只写了"追加式审计"这句话，没给字段表。
/// 所以这里一律按可空读、多认几个别名（`op`/`action`、`at`/`time`），
/// 后端定下来之后对齐字段名即可（界面只读它，不做判据）。
class CopyAuditRow {
  final String? at;
  final String? actor;
  final String? key;
  final String? op;
  final String? before;
  final String? after;
  final String? note;

  const CopyAuditRow({this.at, this.actor, this.key, this.op, this.before, this.after, this.note});

  factory CopyAuditRow.fromJson(Map<String, dynamic> json) => CopyAuditRow(
        at: _optString(json['at'] ?? json['time'] ?? json['updatedAt']),
        actor: _optString(json['actor'] ?? json['by']),
        key: _optString(json['key']),
        op: _optString(json['op'] ?? json['action']),
        before: _optString(json['before']),
        after: _optString(json['after']),
        note: _optString(json['note']),
      );
}

class CopyApi {
  final String baseUrl;
  final http.Client _client;

  /// 配对令牌（`X-ComfyHub-Token`）的取值回调；默认与 `AiApiClient` **同一份**
  /// 进程内缓存 —— 手机上不带令牌的请求会被闸门挡在 403，而"某个页面忘了接线"
  /// 是 2026-09-24 真机验收真踩过的坑。
  final String? Function()? tokenProvider;

  CopyApi(this.baseUrl, {http.Client? client, String? Function()? tokenProvider})
      : tokenProvider = tokenProvider ?? PairingTokenCache.read,
        _client = client ?? http.Client();

  void dispose() => _client.close();

  Uri _uri(String path) => Uri.parse('$baseUrl$path');

  /// 拼 query：**空值一律不发那个键**（`?scope=&q=` 这种空参会让后端拿到空串，
  /// 与"不筛"是两回事）。值过 `encodeQueryComponent`，中文关键字不会打歪。
  static String _pathWithQuery(String path, Map<String, String> params) {
    final kept = params.entries.where((e) => e.value.isNotEmpty).toList();
    if (kept.isEmpty) return path;
    return '$path?${kept.map((e) => '${e.key}=${Uri.encodeQueryComponent(e.value)}').join('&')}';
  }

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

    final map = json is Map ? Map<String, dynamic>.from(json) : null;
    var error = _optString(map?['error']);
    final detail = _optString(map?['detail']);
    // 非 JSON 的响应（代理页 / 网关 502 / 空体）没有 `error` 键：把原文当 `error`
    // 交出去 —— 用户至少能看到挡在前面的是什么，比一句"请求失败"有用。
    if (error == null && map == null && body.trim().isNotEmpty) error = body.trim();
    throw CopyApiException(res.statusCode, error ?? 'HTTP ${res.statusCode}', detail);
  }

  Future<dynamic> _get(String path) async => _decode(await _client.get(_uri(path), headers: _headers));

  Future<dynamic> _send(String method, String path, [Object? body]) async {
    final req = http.Request(method, _uri(path))
      ..headers.addAll(_headers)
      ..body = body == null ? '' : jsonEncode(body);
    final streamed = await _client.send(req);
    return _decode(await http.Response.fromStream(streamed));
  }

  // --- 清单 / 详情 --------------------------------------------------------

  /// 清单（设计 §3）。
  ///
  /// [verifiedOnly] 对应 query 的 `verified=`：**true = 只看已校验过，
  /// false = 只看还没校验的（就是界面上的「只看待校验」），null = 不筛** ——
  /// `false` 与 `null` **不是一回事**，别用 `_onlyUnverified ? true : null` 这种写法。
  /// 哪些条算"还没校验"由后端定：`baseChanged`（默认值变过）按设计 §1 纪律 2
  /// 属于"待重新校验"，这一判据不在前端复制一份。
  Future<List<CopyEntry>> catalog({String? scope, String? q, bool? verifiedOnly}) async {
    final raw = await _get(_pathWithQuery('/api/copy/catalog', {
      'scope': scope ?? '',
      'q': q ?? '',
      if (verifiedOnly != null) 'verified': verifiedOnly ? '1' : '0',
    }));
    return _entriesOf(raw);
  }

  /// 详情：默认文本 + 当前文本，供界面直接 diff（设计 §3）。
  Future<CopyEntry> detail(String key) async =>
      CopyEntry.fromJson(Map<String, dynamic>.from(
          await _get('/api/copy/${Uri.encodeComponent(key)}') as Map));

  /// 改一条（body：`{text, note?}`）。`skill.*` 由后端走文件写回（设计 §2）。
  Future<CopyEntry> put(String key, String text, {String? note}) async => CopyEntry.fromJson(
      Map<String, dynamic>.from(await _send('PUT', '/api/copy/${Uri.encodeComponent(key)}', {
        'text': text,
        'note': ?note,
      }) as Map));

  /// 还原默认（删掉覆盖层那一行）。
  Future<void> reset(String key) async => _send('DELETE', '/api/copy/${Uri.encodeComponent(key)}');

  /// 标记"我读过并确认"（写 `verified_at`）。
  Future<CopyEntry> verify(String key, {String? note}) async => CopyEntry.fromJson(
      Map<String, dynamic>.from(await _send('POST', '/api/copy/${Uri.encodeComponent(key)}/verify', {
        'note': ?note,
      }) as Map));

  /// 追加式审计（最新在前由后端定，前端不重排）。
  Future<List<CopyAuditRow>> audit({int limit = 50}) async {
    final raw = await _get('/api/copy/audit?limit=$limit');
    return _mapList(raw).map(CopyAuditRow.fromJson).toList();
  }

  // --- 批量（设计 §4：这是"批量"的全部价值）--------------------------------

  /// 批量修改。**`dryRun` 是必传的位置参数**（命名参数里没有默认值）：
  /// 没有"预览 diff"这一步的批量改文案就是赌博（设计 §4 硬规矩 1），
  /// 所以调用方必须自己写清楚这一次是预览还是真改。
  ///
  /// [confirm] 只在**真给 `true`** 时才会出现在 body 里：设计 §7 C2 写着
  /// "`prompt.nsfw` 要 `confirm`"，而 §4 的 body 里没有这个键 ——
  /// 只在需要时多发一个键，按 §4 原样实现的后端也不会因此收到陌生字段。
  /// 界面的二次确认（设计 §4 硬规矩 4）在 `copy_review_page` 里，早于这里发生。
  Future<({List<CopyDiff> items, int changed})> batch({
    required List<String> keys,
    required CopyBatchOp op,
    String? find,
    String? replace,
    String? regex,
    bool confirm = false,
    required bool dryRun,
  }) async {
    final raw = await _send('POST', '/api/copy/batch', {
      'keys': keys,
      'op': op.wire,
      'find': ?find,
      'replace': ?replace,
      'regex': ?regex,
      if (confirm) 'confirm': true,
      'dryRun': dryRun,
    });
    final items = _mapList(raw).map(CopyDiff.fromJson).toList();
    // `changed` 优先用后端算的那个数；后端没给就按 items 数出来。
    // 两种都只数**真的会变**的条 —— `blocked` / "find 没命中"都不算（设计 §4 硬规矩 2）。
    final serverCount = raw is Map ? raw['changed'] : null;
    return (items: items, changed: serverCount is int ? serverCount : items.where((d) => d.changed).length);
  }

  // --- 解析小工具 ---------------------------------------------------------

  /// 清单按设计是**裸数组**；包一层 `{items: [...]}` 也认 ——
  /// 这层宽容只有一行成本，收益是"后端换个包装"不会让整页变空。
  List<CopyEntry> _entriesOf(dynamic raw) => _mapList(raw).map(CopyEntry.fromJson).toList();

  List<Map<String, dynamic>> _mapList(dynamic raw) {
    final list = raw is Map ? raw['items'] : raw;
    if (list is! List) return const [];
    return list.whereType<Map>().map((e) => Map<String, dynamic>.from(e)).toList();
  }
}

/// `{error, detail?}`（Rust 侧的稳定形状，见 `test/api_error_envelope_test.dart`）。
///
/// 两个键**都留着**：`error` 是稳定码（`NOT_FOUND` / `PLACEHOLDER_BROKEN`…），
/// `detail` 是人话。界面要展示人话、日志/断言要认码 —— 只剩一个就没法两全。
class CopyApiException implements Exception {
  final int statusCode;

  /// 稳定错误码；普通错误里它就是那句人话（与 `ApiClient` 的形状一致）。
  final String error;

  /// 附加说明；最常是"给人看的那句"。
  final String? detail;

  CopyApiException(this.statusCode, this.error, [this.detail]);

  /// 给人看的那句：有 `detail` 就用它，否则用 `error`。
  String get message => (detail != null && detail!.isNotEmpty) ? detail! : error;

  @override
  String toString() => message;
}

String? _optString(dynamic v) {
  if (v == null) return null;
  return v is String ? v : v.toString();
}

/// `refs` 按设计是"文件:行 / 端点 / 界面位置"的清单。元素不是字符串时原样
/// `toString()`（不猜结构、不丢信息），空元素丢掉 —— 界面上一个空行没有意义。
List<String> _optRefs(dynamic v) {
  if (v is! List) return const [];
  final out = <String>[];
  for (final e in v) {
    final s = _optString(e);
    if (s != null && s.isNotEmpty) out.add(s);
  }
  return out;
}
