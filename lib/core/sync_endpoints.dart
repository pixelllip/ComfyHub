import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;

/// 一个"可以连的后端地址"，以及它大概属于哪一类网络。
///
/// 为什么要分类：跨端用户实际会同时存在三条路 —— 模拟器/同机（回环）、
/// 同一 Wi-Fi（局域网）、异地（Tailscale）。**三者的地址长得完全不一样，
/// 但界面只能说人话**：`100.64.x.x` 对用户毫无意义，"Tailscale" 才有。
/// 反过来，分类**只用于显示与排序**，绝不参与"连不连得上"的判断 ——
/// 能不能用是 [probeSyncEndpoint] 打一次真实请求说了算的（见那里的注释）。
class SyncEndpoint {
  const SyncEndpoint(this.url, {required this.source});

  final String url;

  /// 这个候选是从哪来的：`自动发现` / `最近用过` / `桌面报告` / `手填`。
  ///
  /// **必须如实显示**：候选列表不是猜出来的，用户要能看出"这条是谁告诉我的"
  /// （桌面报告的那几条来自对端自己的网卡枚举，不是我们扫网段扫出来的）。
  final String source;

  SyncEndpointKind get kind => classifyEndpoint(url);
}

enum SyncEndpointKind { loopback, lan, tailscale, other }

extension SyncEndpointKindLabel on SyncEndpointKind {
  String get label => switch (this) {
        SyncEndpointKind.loopback => '回环',
        SyncEndpointKind.lan => '局域网',
        SyncEndpointKind.tailscale => 'Tailscale',
        SyncEndpointKind.other => '其他',
      };

  /// 给用户的一句"这条什么时候用得上"。
  String get hint => switch (this) {
        SyncEndpointKind.loopback => '同一台机器（模拟器用 10.0.2.2）',
        SyncEndpointKind.lan => '同一个 Wi-Fi / 网线',
        SyncEndpointKind.tailscale => '异地（Tailscale 组网里）',
        SyncEndpointKind.other => '手工填的地址',
      };
}

/// 按 IP 形状分类。**只用于显示**，不是连通性判据。
SyncEndpointKind classifyEndpoint(String url) {
  final host = Uri.tryParse(url.trim())?.host ?? '';
  if (host.isEmpty) return SyncEndpointKind.other;
  if (host == 'localhost' || host == '127.0.0.1' || host == '::1') {
    return SyncEndpointKind.loopback;
  }
  // 模拟器访问宿主机那三个特殊地址，用户视角同样是"本机"
  if (host == '10.0.2.2' || host == '10.0.3.2') return SyncEndpointKind.loopback;
  final parts = host.split('.');
  if (parts.length != 4) return SyncEndpointKind.other;
  final nums = parts.map(int.tryParse).toList();
  if (nums.any((n) => n == null || n < 0 || n > 255)) return SyncEndpointKind.other;
  final a = nums[0]!;
  final b = nums[1]!;
  if (a == 10 && b == 0 && (nums[2] == 2 || nums[2] == 3) && nums[3] == 2) {
    return SyncEndpointKind.loopback;
  }
  // Tailscale 走 CGNAT 段 100.64.0.0/10 —— 它看起来像"公网"但其实是私网组网，
  // 排在这里判断（否则会被归进"其他"，用户看不出该怎么用）。
  if (a == 100 && b >= 64 && b <= 127) return SyncEndpointKind.tailscale;
  if (a == 10) return SyncEndpointKind.lan;
  if (a == 172 && b >= 16 && b <= 31) return SyncEndpointKind.lan;
  if (a == 192 && b == 168) return SyncEndpointKind.lan;
  // 169.254/16 与其余（含公网域名）：不猜
  return SyncEndpointKind.other;
}

String _trimUrl(String url) {
  var v = url.trim();
  while (v.endsWith('/')) {
    v = v.substring(0, v.length - 1);
  }
  return v;
}

/// 本机一侧的候选：当前地址 → 自动发现（`.run\api.json`）→ 最近用过的。
///
/// 顺序就是"最可能想点的排前面"；重复的按 [SyncEndpoint.url] 去掉（保留第一次的 source）。
List<SyncEndpoint> localEndpointCandidates({
  required String autoUrl,
  required String current,
  required List<String> history,
}) {
  final out = <SyncEndpoint>[];
  final seen = <String>{};
  void add(String url, String source) {
    final v = _trimUrl(url);
    if (v.isEmpty || !seen.add(v)) return;
    out.add(SyncEndpoint(v, source: source));
  }

  add(current, '当前使用');
  add(autoUrl, '自动发现');
  for (final h in history) {
    add(h, '最近用过');
  }
  return out;
}

/// 问一次**对面**：它自己有哪些可达地址（`GET /api/net/urls`）。
///
/// ⚠ **当前自动那条路不调它**（2026-09-30 删掉「同步与连接」页之后就没人调了）：
/// 自动挑地址跑在"还没连上任何桌面"的时刻，问不到对面。保留它是因为**手机配对**那条路
/// 需要它（先把地址显示给人看，见 `docs/pairing-design.md`）—— 改那块时从这儿接。
///
/// 三条纪律：
///  · **best-effort**：老后端没有这条端点（404）、或现在就连不上 —— 都不是错误，
///    回空列表 + 一句 note 即可（界面照旧有"自动发现 / 最近用过 / 手填"三条路）；
///  · **不许编**：对面给什么就显示什么，绝不按网卡名猜地址；
///  · 标 `source: 桌面报告`：用户要知道这几条是对端告诉我们的。
Future<({List<SyncEndpoint> endpoints, String? note})> discoveredEndpoints(
  String baseUrl, {
  http.Client? client,
  String? token,
  Duration timeout = const Duration(seconds: 5),
}) async {
  final base = _trimUrl(baseUrl);
  if (base.isEmpty) return (endpoints: const <SyncEndpoint>[], note: null);
  final own = client ?? http.Client();
  try {
    final reply = await own.get(
      Uri.parse('$base/api/net/urls'),
      headers: {
        'Accept': 'application/json',
        if (token != null && token.isNotEmpty) 'X-ComfyHub-Token': token,
      },
    ).timeout(timeout);
    if (reply.statusCode == 404) {
      return (endpoints: const <SyncEndpoint>[], note: '这台后端还没有"报告自己地址"的接口（旧版本）');
    }
    if (reply.statusCode != 200) {
      return (endpoints: const <SyncEndpoint>[], note: '对面回 HTTP ${reply.statusCode}，没拿到候选地址');
    }
    final decoded = jsonDecode(reply.body);
    final raw = decoded is Map ? decoded['urls'] : null;
    if (raw is! List) return (endpoints: const <SyncEndpoint>[], note: '对端回的形状不对（没有 urls 数组）');
    final out = <SyncEndpoint>[];
    final seen = <String>{};
    for (final item in raw) {
      final url = item is String
          ? item
          : (item is Map ? (item['url'] ?? '').toString() : '');
      final v = _trimUrl(url);
      if (v.isEmpty || !v.startsWith('http') || !seen.add(v)) continue;
      out.add(SyncEndpoint(v, source: '桌面报告'));
    }
    return (endpoints: out, note: out.isEmpty ? '对面说它没有别的可达地址' : null);
  } on TimeoutException {
    return (endpoints: const <SyncEndpoint>[], note: '问对端要地址超时（它可能不在线）');
  } catch (e) {
    return (endpoints: const <SyncEndpoint>[], note: '问对端要地址失败：$e');
  } finally {
    if (client == null) own.close();
  }
}

/// 一次"测试连接"的结果。
///
/// **成败只由真实请求决定**：[ok] 为 true 意味着 `/api/health` 真的回了 200
/// 且带着 ComfyHub 那几个键。任何"猜它应该行"的实现都会在用户面前变成假绿灯。
class SyncProbe {
  const SyncProbe({
    required this.ok,
    required this.baseUrl,
    required this.elapsedMs,
    this.version,
    this.impl,
    this.database,
    this.schemaVersion,
    this.kitVersion,
    this.cursor,
    this.note,
    this.error,
  });

  final bool ok;
  final String baseUrl;
  final int elapsedMs;
  final String? version;

  /// `rust` / `kotlin`（后端自己报的；老后端没这个键时为 null）。
  final String? impl;
  final String? database;
  final int? schemaVersion;

  /// `/api/sync/manifest` 里的规则工件版本（拿不到就 null，**不编**）。
  final String? kitVersion;
  final int? cursor;

  /// 部分成功时的一句如实说明（例如 health 通了但 manifest 要令牌）。
  final String? note;
  final String? error;

  /// 给界面的一行结论（人话）。
  String get summary {
    if (!ok) return '连不上：${error ?? '未知原因'}';
    final bits = <String>[
      if (impl != null) '后端 $impl',
      if (version != null) 'v$version',
      if (schemaVersion != null) 'schema v$schemaVersion',
      if (kitVersion != null) 'run-kit $kitVersion',
      '${elapsedMs}ms',
    ];
    return '连接正常（${bits.join(' · ')}）';
  }
}

/// 打一次真实的 `/api/health`（+ best-effort 的 `/api/sync/manifest`）。
///
/// 为什么 health 是**唯一**的通过判据、manifest 只作补充：manifest 需要配对令牌
/// （未配对/令牌过期时回 401），而"这个地址后面有没有一台 ComfyHub"跟"我配没配对"
/// 是两件事 —— 混在一起会让用户以为地址填错了。
Future<SyncProbe> probeSyncEndpoint(
  String baseUrl, {
  http.Client? client,
  String? token,
  Duration timeout = const Duration(seconds: 8),
}) async {
  final base = _trimUrl(baseUrl);
  final sw = Stopwatch()..start();
  if (base.isEmpty || !base.startsWith('http')) {
    return SyncProbe(
      ok: false,
      baseUrl: base,
      elapsedMs: 0,
      error: '地址要以 http:// 开头（现在是「$baseUrl」）',
    );
  }
  final own = client ?? http.Client();
  Map<String, String> headers() => {
        'Accept': 'application/json',
        if (token != null && token.isNotEmpty) 'X-ComfyHub-Token': token,
      };
  try {
    final reply = await own
        .get(Uri.parse('$base/api/health'), headers: headers())
        .timeout(timeout);
    if (reply.statusCode != 200) {
      return SyncProbe(
        ok: false,
        baseUrl: base,
        elapsedMs: sw.elapsedMilliseconds,
        error: 'HTTP ${reply.statusCode}：${reply.body.trim().isEmpty ? '（空响应）' : _snippet(reply.body)}',
      );
    }
    final decoded = jsonDecode(reply.body);
    if (decoded is! Map || decoded['status'] == null || decoded['database'] == null) {
      return SyncProbe(
        ok: false,
        baseUrl: base,
        elapsedMs: sw.elapsedMilliseconds,
        error: '这个地址回的不是 ComfyHub 后端（/api/health 少了 status / database）：${_snippet(reply.body)}',
      );
    }
    // 补充信息：manifest 拿不到不算失败（多半是没配对 / 令牌过期）
    String? note;
    String? kitVersion;
    int? cursor;
    try {
      final m = await own
          .get(Uri.parse('$base/api/sync/manifest'), headers: headers())
          .timeout(timeout);
      if (m.statusCode == 200) {
        final body = jsonDecode(m.body);
        if (body is Map) {
          kitVersion = body['kitVersion']?.toString();
          cursor = (body['cursor'] as num?)?.toInt();
        }
      } else {
        note = '同步清单要配对令牌（HTTP ${m.statusCode}）—— 地址是对的，配对还没成';
      }
    } catch (_) {
      note = '同步清单没取到（不影响"地址对不对"这个结论）';
    }
    return SyncProbe(
      ok: true,
      baseUrl: base,
      elapsedMs: sw.elapsedMilliseconds,
      version: decoded['version']?.toString(),
      impl: decoded['impl']?.toString(),
      database: decoded['database']?.toString(),
      schemaVersion: (decoded['schemaVersion'] as num?)?.toInt(),
      kitVersion: kitVersion,
      cursor: cursor,
      note: note,
    );
  } on TimeoutException {
    return SyncProbe(
      ok: false,
      baseUrl: base,
      elapsedMs: sw.elapsedMilliseconds,
      error: '超时（${timeout.inSeconds}s 内没响应）—— 地址可能不在这个网里',
    );
  } catch (e) {
    return SyncProbe(
      ok: false,
      baseUrl: base,
      elapsedMs: sw.elapsedMilliseconds,
      error: '$e',
    );
  } finally {
    if (client == null) own.close();
  }
}

String _snippet(String body) {
  final t = body.replaceAll('\n', ' ').trim();
  return t.length <= 200 ? t : '${t.substring(0, 200)}…';
}

/// 一次"自动挑桌面"的结果。
class AutoPickedEndpoint {
  const AutoPickedEndpoint({
    required this.baseUrl,
    required this.probe,
    required this.tried,
    required this.candidates,
  });

  /// 挑中的地址；一条都没通就是 `null`（调用方**保持现状**，别把地址清成空的）。
  final String? baseUrl;

  /// 挑中那次的探测结论（`baseUrl` 为 null 时是**最后一次**的结论，用来如实说为什么没挑到）。
  final SyncProbe? probe;

  /// 试了几条。
  final int tried;

  /// 试过哪些候选（诊断用；顺序就是试的顺序）。
  final List<SyncEndpoint> candidates;

  /// 给人看的一行（挑不到时说明试过哪几条）。
  String get summary {
    if (baseUrl != null) return '已自动连上 $baseUrl（${probe?.summary ?? ''}）';
    if (candidates.isEmpty) return '没有可试的地址';
    final list = candidates.map((e) => '${e.url}（${e.source}）').join('、');
    return '试了 $tried 个地址都没连上：$list。最后一次：${probe?.error ?? '未知原因'}';
  }
}

/// **自动挑一台能连的桌面**（用户不再需要管"连哪台"）。
///
/// 为什么可以自动：候选本来就只有那么几条（最近用过的 → 本机自动值 → 历史），
/// 而每条能不能用一条 `/api/health` 就判死了（[probeSyncEndpoint]）。原来把这件事
/// 摆成一页让用户点，等于把机器该做的活推给了人 —— 2026-09-30 用户要求改成自动。
///
/// 三条纪律：
///  · **一条都不通就原样返回 `null`**，调用方保持现有地址（**不许清空**）——
///    "连不上"是事实，不是"把配置删掉"的理由；
///  · **超时短**（默认 3s）：这是在启动路径上跑的，串行试 4 条也只是十几秒；
///  · **绝不猜地址**：只试 [localEndpointCandidates] 给的那几条（当前 / 自动发现 / 历史），
///    不按网卡名编一个出来。
Future<AutoPickedEndpoint> autoPickBaseUrl({
  required String autoUrl,
  required String current,
  List<String> history = const [],
  String? token,
  http.Client? client,
  Duration timeout = const Duration(seconds: 3),
}) async {
  final candidates = localEndpointCandidates(
    autoUrl: autoUrl,
    current: current,
    history: history,
  );
  SyncProbe? last;
  var tried = 0;
  for (final candidate in candidates) {
    tried++;
    final probe = await probeSyncEndpoint(
      candidate.url,
      client: client,
      token: token,
      timeout: timeout,
    );
    last = probe;
    if (probe.ok) {
      return AutoPickedEndpoint(
        baseUrl: probe.baseUrl,
        probe: probe,
        tried: tried,
        candidates: candidates,
      );
    }
  }
  return AutoPickedEndpoint(
    baseUrl: null,
    probe: last,
    tried: tried,
    candidates: candidates,
  );
}
