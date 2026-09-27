import 'dart:convert';
import 'dart:io' show Platform, SocketException;

import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;

import 'api_client.dart' show ApiException;
import 'pairing_token.dart';

/// 桌面 `/api/auth*` 与 `/api/ai/credentials` 的客户端（配对闸门，🔒 `docs/pairing-design.md` §11）。
///
/// # 它解决的是"人怎么把令牌交到手机手里"
///
/// 令牌本身早就在协议里（`X-ComfyHub-Token`，`docs/sync-api-contract.md` §0），
/// 但**手机上从来没有值**：Dart 侧调 `ensureRunKit` / `syncNow` 时 `token` 恒 `null`。
/// 这个文件补的就是那一段 —— 而且补在**唯一一条不靠扫码**的路径上：
/// 手机先举手（`pair/request`），桌面上显示出 6 位码，人在手机上把码敲进去（`pair/claim`）。
///
/// # 一条纪律：这里**没有**"取长期令牌"的方法
///
/// 令牌只在 `claim` 的响应里出现**一次**（§5.2）。之后手机把它写进 Android Keystore
/// （`PairingTokenStore.write`），要再读出来只能走 `KeystoreCredentials.get`
/// —— 与 API Key 同一条纪律（值不回显给界面）。
class InboundAuthApi {
  InboundAuthApi(this.baseUrl, {http.Client? client}) : _client = client ?? http.Client();

  /// 桌面地址（形如 `http://10.44.130.232:8081`，末尾斜杠无所谓）。
  final String baseUrl;
  final http.Client _client;

  void dispose() => _client.close();

  String get _root {
    var v = baseUrl.trim();
    while (v.endsWith('/')) {
      v = v.substring(0, v.length - 1);
    }
    return v;
  }

  Map<String, String> _headers(String? token) => {
        'Content-Type': 'application/json; charset=utf-8',
        'Accept': 'application/json',
        // 空令牌**不发这个头**：发一个空串会被当成"带了个错的令牌"，
        // 而闸门对这两件事的回答是不同的（403 vs 401），诊断会因此指错方向。
        if (token != null && token.isNotEmpty) 'X-ComfyHub-Token': token,
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
    // ⚠ **2xx 也可能是错误**：Kotlin 后端对未知路径也会回 `200 {"error":"接口不存在: …"}`
    // （它那个 StatusPages 的 404 处理器会把状态码重置成 200 —— Rust 侧刻意只对齐 body、
    // 不对齐那个 200）。灰度期桌面 App 连的还是 Kotlin（8080），所以这一条会真的发生：
    // 不认它的话，`PUT /api/auth` 会"成功"返回一个 enabled=false 的状态，
    // 界面上的表现为**开关弹回去、一个字都不解释**。
    if (json is Map && json['error'] != null) {
      final detail = json['detail']?.toString();
      throw ApiException(
        res.statusCode,
        json['error'].toString(),
        detail,
      );
    }
    if (res.statusCode >= 200 && res.statusCode < 300) return json;
    final msg = (json is Map && json['error'] != null) ? json['error'].toString() : 'HTTP ${res.statusCode}';
    final detail = (json is Map && json['detail'] != null) ? json['detail'].toString() : null;
    throw ApiException(res.statusCode, msg, detail);
  }

  Future<dynamic> _get(String path, {Map<String, String>? query, String? token}) async {
    final uri = Uri.parse('$_root$path').replace(queryParameters: query);
    try {
      return _decode(await _client.get(uri, headers: _headers(token)));
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接桌面 $_root', e.message);
    }
  }

  Future<dynamic> _send(String method, String path, Object? body, {String? token}) async {
    final req = http.Request(method, Uri.parse('$_root$path'))
      ..headers.addAll(_headers(token));
    if (body != null) req.body = jsonEncode(body);
    try {
      final streamed = await _client.send(req);
      return _decode(await http.Response.fromStream(streamed));
    } on SocketException catch (e) {
      throw ApiException(0, '无法连接桌面 $_root', e.message);
    }
  }

  // --- 桌面侧（仅回环可用；手机调会拿到 403）---------------------------------

  /// `GET /api/auth` —— 闸门状态 + 已配对设备 + 待配对请求（§11.2）。
  Future<InboundAuthStatus> status({String? token}) async =>
      InboundAuthStatus.fromJson(_asMap(await _get('/api/auth', token: token)));

  /// `PUT /api/auth` `{enabled}` —— 打开/关闭「允许移动端连接」。
  ///
  /// 打开时后端会**顺手生成一个长期令牌**（若还没有），这是它现在唯一的用途。
  Future<InboundAuthStatus> setEnabled(bool enabled, {String? token}) async =>
      InboundAuthStatus.fromJson(
        _asMap(await _send('PUT', '/api/auth', {'enabled': enabled}, token: token)),
      );

  /// `POST /api/auth/rotate` —— 轮换全局令牌（所有设备立刻失效，无宽限期）。
  Future<Map<String, dynamic>> rotate({String? token}) async =>
      _asMap(await _send('POST', '/api/auth/rotate', const {}, token: token));

  /// `GET /api/auth/pair/pending` —— 桌面上那张"待配对"列表（6 位码就在里面）。
  Future<List<PendingPairing>> pending({String? token}) async =>
      (_asList(_asMap(await _get('/api/auth/pair/pending', token: token))['pending']))
          .map(PendingPairing.fromJson)
          .toList();

  /// `GET /api/auth/devices` —— 已配对设备（**不含令牌**）。
  Future<List<PairedDevice>> devices({String? token}) async =>
      (_asList(_asMap(await _get('/api/auth/devices', token: token))['devices']))
          .map(PairedDevice.fromJson)
          .toList();

  /// `POST /api/auth/devices/{id}/revoke` —— 吊销一台设备的令牌（**立刻**生效）。
  Future<void> revoke(String deviceId, {String? token}) =>
      _send('POST', '/api/auth/devices/${Uri.encodeComponent(deviceId)}/revoke', const {}, token: token);

  /// `POST /api/auth/devices/{id}/credentials` `{allow, allowUpload?}` ——
  /// 这台设备能不能**下行**密钥 / 能不能**上行**（改桌面上的）密钥。
  ///
  /// 两个开关**分开**（`docs/pairing-design.md` §14.1）：读和写不是同一种权力，
  /// 合成一个的后果是用户为了图方便把读打开、顺手也把写打开了 ——
  /// 而写能让"下一次桌面自己发请求"用上别人塞进来的 key。
  ///
  /// [allowUpload] 传 `null` 表示"这次只动下行那个开关"（**不发这个键**）：
  /// 旧后端不认识它，多发一个键不会报错但也不会生效，界面若要据此提示"旧版本"，
  /// 就得看回读出来的 [PairedDevice.allowCredentialsUpload] 有没有变。
  Future<PairedDevice> setAllowCredentials(
    String deviceId,
    bool allow, {
    bool? allowUpload,
    String? token,
  }) async =>
      PairedDevice.fromJson(
        _asMap(await _send(
          'POST',
          '/api/auth/devices/${Uri.encodeComponent(deviceId)}/credentials',
          {
            'allow': allow,
            'allowUpload': ?allowUpload,
          },
          token: token,
        )),
      );

  /// `PUT /api/ai/credentials` —— **密钥上行**：把手机上配的 key 推给桌面（§14.2）。
  ///
  /// 三条纪律：
  ///  · 走的是**已配对设备**令牌 + 设备上的 `allowCredentialsUpload`（默认关）；
  ///  · 值只在请求体里出现一次，**不写日志、不回显**；
  ///  · [rev] 是"我读到的那一版"（`ai.credentials.rev`）：落后于桌面就回
  ///    `STALE_IGNORED`（**宁可让用户重试一次，也不许悄悄覆盖**）。
  Future<CredentialUploadResult> uploadCredentials(
    Map<String, String> values, {
    int? rev,
    String? token,
  }) async =>
      CredentialUploadResult.fromJson(_asMap(await _send(
        'PUT',
        '/api/ai/credentials',
        {
          'credentials': values,
          'rev': ?rev,
        },
        token: token,
      )));

  // --- 手机侧（匿名 + 限速）--------------------------------------------------

  /// `POST /api/auth/pair/request` —— 手机举手，桌面屏幕上会出现 6 位码。
  ///
  /// ⚠ 响应里**没有 ticket**（设计 §11.1 的那条纪律）：手机拿不到算码的密钥，
  /// 于是"必须看到桌面屏幕"这件事才成立。
  Future<PairingRequest> requestPairing({
    required String deviceId,
    required String deviceName,
  }) async =>
      PairingRequest.fromJson(_asMap(await _send('POST', '/api/auth/pair/request', {
        'deviceId': deviceId,
        'deviceName': deviceName,
      })));

  /// `POST /api/auth/pair/claim` —— 交人敲进来的 6 位码，换**这台设备专属**的长期令牌。
  ///
  /// 令牌**只在这个响应里出现一次**，调用方必须立刻存进密钥库
  /// （[PairingTokenStore.write]），存不下就把它丢掉并如实报错 —— 不存就等于每次启动都要重扫。
  Future<PairingClaim> claim({
    required String code,
    required String deviceId,
    required String deviceName,
    String? requestId,
    String? ticket,
  }) async =>
      PairingClaim.fromJson(_asMap(await _send('POST', '/api/auth/pair/claim', {
        'code': code,
        'deviceId': deviceId,
        'deviceName': deviceName,
        // `?x` 是"值非空才带这个键"（与本仓其它地方同一写法）：
        // 带一个 null 上去会被后端当成"给了个空 ticket"，两种失败就分不清了
        'requestId': ?requestId,
        'ticket': ?ticket,
      })));

  /// `GET /api/ai/credentials?refs=a,b` —— **密钥下行**（仅"已授权设备"或本机回环）。
  ///
  /// 未授权的 ref **不会出现在结果里**（不报错）：调用方据此如实说"哪几把没拿到"。
  Future<Map<String, String>> fetchCredentials(List<String> refs, {String? token}) async {
    final wanted = refs.map((e) => e.trim()).where((e) => e.isNotEmpty).toSet().toList();
    if (wanted.isEmpty) return const {};
    final raw = await _get('/api/ai/credentials', query: {'refs': wanted.join(',')}, token: token);
    final map = _asMap(_asMap(raw)['credentials']);
    return map.map((k, v) => MapEntry(k, v?.toString() ?? ''));
  }

  static Map<String, dynamic> _asMap(dynamic raw) =>
      raw is Map ? Map<String, dynamic>.from(raw) : <String, dynamic>{};

  static List<Map<String, dynamic>> _asList(dynamic raw) => raw is List
      ? raw.whereType<Map>().map((e) => Map<String, dynamic>.from(e)).toList()
      : const [];
}

/// `POST /api/auth/pair/request` 的响应。
class PairingRequest {
  const PairingRequest({
    required this.requestId,
    this.expiresAt,
    this.codeExpiresAt,
    this.serverTime,
  });

  /// 配对会话 id。**它不是 ticket**（见 [InboundAuthApi.requestPairing] 的注释）。
  final String requestId;
  final String? expiresAt;

  /// 当前这一个 6 位码的过期时刻（界面据此显示倒计时；到点要重新拉一次）。
  final String? codeExpiresAt;
  final String? serverTime;

  factory PairingRequest.fromJson(Map<String, dynamic> json) => PairingRequest(
        requestId: (json['requestId'] ?? '').toString(),
        expiresAt: json['expiresAt']?.toString(),
        codeExpiresAt: json['codeExpiresAt']?.toString(),
        serverTime: json['serverTime']?.toString(),
      );
}

/// `POST /api/auth/pair/claim` 的响应（**长期令牌唯一一次出现的地方**）。
class PairingClaim {
  const PairingClaim({
    required this.deviceId,
    required this.token,
    this.schemaVersion,
    this.serverTime,
  });

  final String deviceId;
  final String token;
  final int? schemaVersion;
  final String? serverTime;

  factory PairingClaim.fromJson(Map<String, dynamic> json) => PairingClaim(
        deviceId: (json['deviceId'] ?? '').toString(),
        token: (json['token'] ?? '').toString(),
        schemaVersion: (json['schemaVersion'] as num?)?.toInt(),
        serverTime: json['serverTime']?.toString(),
      );
}

/// 已配对的一台设备（`GET /api/auth/devices`）。**只有事实，没有令牌。**
class PairedDevice {
  const PairedDevice({
    required this.id,
    required this.name,
    this.pairedAt,
    this.lastSeenAt,
    this.lastIp,
    this.allowCredentials = false,
    this.allowCredentialsUpload = false,
  });

  final String id;
  final String name;
  final String? pairedAt;
  final String? lastSeenAt;
  final String? lastIp;

  /// 是否允许**把桌面上的 API Key 下发给这台设备**（默认 `false`）。
  final bool allowCredentials;

  /// 是否允许**这台设备改桌面上的 API Key**（上行，默认 `false`，§14.1）。
  ///
  /// 旧后端没有这个字段 → 恒 `false`。界面据此如实说"这台后端还不支持上行密钥"，
  /// **别**把它当成"用户没开"（两者的处置完全不同）。
  final bool allowCredentialsUpload;

  factory PairedDevice.fromJson(Map<String, dynamic> json) => PairedDevice(
        id: (json['id'] ?? '').toString(),
        name: (json['name'] ?? '').toString(),
        pairedAt: json['pairedAt']?.toString(),
        lastSeenAt: json['lastSeenAt']?.toString(),
        lastIp: json['lastIp']?.toString(),
        allowCredentials: json['allowCredentials'] == true,
        allowCredentialsUpload: json['allowCredentialsUpload'] == true,
      );
}

/// `PUT /api/ai/credentials` 的回执（§14.2）。
///
/// `rejected` **逐条带码**：手机要能区分"永远不可能成功"（`FORBIDDEN` / `BAD_PAYLOAD`）
/// 与"桌面那份更新了、你重试也没用"（`STALE_IGNORED`），而不是只看到一个失败总数。
class CredentialUploadResult {
  const CredentialUploadResult({
    this.applied = const [],
    this.rejected = const [],
    this.rev,
  });

  final List<String> applied;
  final List<CredentialRejection> rejected;

  /// 桌面写完之后的 `ai.credentials.rev`（下次上行要带它）。
  final int? rev;

  factory CredentialUploadResult.fromJson(Map<String, dynamic> json) => CredentialUploadResult(
        applied: (json['applied'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        rejected: (json['rejected'] as List?)
                ?.whereType<Map>()
                .map((e) => CredentialRejection.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
        rev: (json['rev'] as num?)?.toInt(),
      );
}

/// 一条被拒的凭据上行。
class CredentialRejection {
  const CredentialRejection({required this.ref, required this.code, this.message});

  final String ref;

  /// `FORBIDDEN`（开关没开/设备没授权）/ `STALE_IGNORED`（桌面那份更新）/
  /// `BAD_PAYLOAD`（ref 形状不合法）。
  final String code;
  final String? message;

  factory CredentialRejection.fromJson(Map<String, dynamic> json) => CredentialRejection(
        ref: (json['ref'] ?? '').toString(),
        code: (json['code'] ?? '').toString(),
        message: json['message']?.toString(),
      );

  /// 给人看的一句话（界面直接显示它，别只显示码）。
  String get hint => switch (code) {
        'FORBIDDEN' => '桌面没有允许这台设备改密钥（要在桌面上的设备卡片里勾选）',
        'STALE_IGNORED' => '桌面上这把更新过 —— 先从桌面同步一次，再推你改的这版',
        'BAD_PAYLOAD' => message ?? '这把的名字（ref）不合法，桌面拒收了',
        _ => message ?? '桌面拒收（$code）',
      };
}

/// 桌面屏幕上正在等的那次配对（`GET /api/auth/pair/pending`）。
class PendingPairing {
  const PendingPairing({
    required this.requestId,
    required this.code,
    this.ip,
    this.deviceName,
    this.codeExpiresAt,
    this.expiresAt,
  });

  final String requestId;

  /// **6 位十进制码** —— 人从桌面屏幕上念、在手机上敲的那一串。
  final String code;
  final String? ip;
  final String? deviceName;
  final String? codeExpiresAt;
  final String? expiresAt;

  factory PendingPairing.fromJson(Map<String, dynamic> json) => PendingPairing(
        requestId: (json['requestId'] ?? '').toString(),
        code: (json['code'] ?? '').toString(),
        ip: json['ip']?.toString(),
        deviceName: json['deviceName']?.toString(),
        codeExpiresAt: json['codeExpiresAt']?.toString(),
        expiresAt: json['expiresAt']?.toString(),
      );
}

/// `GET/PUT /api/auth` 的响应。
class InboundAuthStatus {
  const InboundAuthStatus({
    required this.enabled,
    required this.hasToken,
    this.schema,
    this.devices = const [],
    this.pending = const [],
  });

  /// 闸门总开关。**关着 = 今天的行为**（局域网内任何设备都能连）。
  final bool enabled;

  /// 是否已经有长期令牌（打开开关时后端会顺手生成一个）。
  final bool hasToken;
  final String? schema;
  final List<PairedDevice> devices;
  final List<PendingPairing> pending;

  factory InboundAuthStatus.fromJson(Map<String, dynamic> json) => InboundAuthStatus(
        enabled: json['enabled'] == true,
        hasToken: json['hasToken'] == true,
        schema: json['schema']?.toString(),
        devices: InboundAuthApi._asList(json['devices']).map(PairedDevice.fromJson).toList(),
        pending: InboundAuthApi._asList(json['pending']).map(PendingPairing.fromJson).toList(),
      );
}

/// 手机上的**配对令牌 + 设备身份**（Android Keystore）。
///
/// # 为什么令牌也进密钥库，而不是 SharedPreferences
///
/// 它是"这台手机等于这台桌面的主人"的凭据，与一把 API Key 同级（`auth.pairing` 这个 ref
/// 在 `docs/pairing-design.md` §3.2 第 13 步就已经定下来了）。
/// 存进去之后**只能由桥读出来**（`getCredential`），界面拿不到它。
class PairingTokenStore {
  PairingTokenStore({MethodChannel? channel})
      : _method = channel ?? const MethodChannel(methodChannelName);

  /// 与 `LocalHarnessRunEventSource.methodChannelName` 同一个通道（复用常量，改一处就够）。
  static const String methodChannelName = 'comfyhub/harness';

  /// 令牌在密钥库里的名字（桌面后端不看这个名字，它只看请求头）。
  static const String ref = 'auth.pairing';

  final MethodChannel _method;

  /// **内存里的那一份令牌**（进程内共享，不落盘）—— 见 [`PairingTokenCache`]。
  ///
  /// 保留这个 getter 是为了读起来顺：`PairingTokenStore.cached` 就是"这台机器现在拿着什么"。
  static String? get cached => PairingTokenCache.value;

  /// 读回令牌（**手机上唯一会"取出密钥"的地方**：它要放进请求头）。
  Future<String?> read() async {
    final v = await _method.invokeMethod<String>('getCredential', {'ref': ref});
    PairingTokenCache.set(v);
    return PairingTokenCache.value;
  }

  /// 存令牌。存失败**必须让调用方看见**（存不下去的令牌 = 每次启动都要重配）。
  Future<void> write(String token) async {
    if (token.trim().isEmpty) {
      throw StateError('令牌为空，拒绝写入密钥库');
    }
    await _method.invokeMethod<bool>('putCredential', {'ref': ref, 'value': token});
    PairingTokenCache.set(token);
  }

  Future<void> clear() async {
    await _method.invokeMethod<bool>('removeCredential', {'ref': ref});
    PairingTokenCache.set(null);
  }

  /// 配对时要报给桌面的身份。
  ///
  /// `deviceId` **必须**与上行同步用的是同一个（`sync.deviceId`）——
  /// 生成两次 = 桌面把同一台手机当成两台设备（`SyncPush` 里那条纪律）。
  Future<({String deviceId, String deviceName})> identity() async {
    final raw = await _method.invokeMethod<String>('pairingIdentity');
    final map = raw == null || raw.isEmpty
        ? const <String, dynamic>{}
        : Map<String, dynamic>.from(jsonDecode(raw) as Map);
    final id = (map['deviceId'] ?? '').toString();
    final name = (map['deviceName'] ?? '').toString();
    // 设备名兜底成平台名而不是空串：桌面上那一行总得有东西可显示
    return (deviceId: id, deviceName: name.isEmpty ? Platform.operatingSystem : name);
  }
}
