import 'dart:convert';

import 'sync_endpoints.dart' show SyncEndpointKind, classifyEndpoint;

/// 配对二维码的内容（🔒 `docs/pairing-design.md` §4.5）——
/// **这个格式只有这一份实现**（桌面编码、手机解码都调这里）。
///
/// ```
/// comfyhub://pair?v=1&t=<ticket>&e=<expiresAt>&u=<base64url(urls 用 \n 连接)>
/// ```
///
/// 四条纪律：
/// 1. **自造 scheme**（不是 `http://`）：将来手机真做 deep link 时不会与"打开网页"混淆；
/// 2. **`v=1` 必须带**：没有版本号，将来改字段就会让"旧手机扫新码"变成一条看不懂的错误
///    —— 所以版本不认识时要说清"请更新 App"，而不是"这不是 ComfyHub 的码"；
/// 3. **长期令牌永远不在这里**（§8）：二维码里只有 ticket + 地址 + 过期时间；
/// 4. **`u` 用 base64url**：地址列表用 `\n` 连接后整体编码 —— 多条 URL 里带 `&` / `=` 的
///    概率不低（端口、路径），不编码就会把查询串撕成两半而且**不报错**。
class PairingPayload {
  const PairingPayload({
    required this.ticket,
    this.expiresAt,
    this.urls = const [],
  });

  static const String scheme = 'comfyhub';
  static const String host = 'pair';
  static const int version = 1;

  final String ticket;
  final String? expiresAt;

  /// 桌面枚举出来的可达地址（§4.4）。**可能为空**：那种情况下界面要如实说
  /// "桌面没枚举出可达地址，请手填"，而不是给一个空二维码。
  final List<String> urls;

  /// 手机上**真正该拿去连**的那个地址：跳过回环 / 模拟器别名。
  ///
  /// 为什么要跳过：二维码是桌面生成的，而桌面的候选列表里天然会有
  /// `http://127.0.0.1:<port>`（回环最常用）。手机拿到它是在连**手机自己**，
  /// 结果必然是 `Connection refused` —— 2026-10-31 真机现场：扫码 + 输入验证码
  /// 全对，界面只报"连接失败"，根因就是取了 `urls.first`。
  ///
  /// 返回 `null` = 这份二维码里没有手机上可用的地址（只有回环 / 空表）。
  /// 调用方**不许**拿 `urls.first` 兜底，那正是这个 bug。
  String? get preferredUrl {
    String? best;
    var bestRank = 1 << 30;
    for (final raw in urls) {
      final uri = Uri.tryParse(raw.trim());
      if (uri == null || uri.host.isEmpty) continue;
      if (isLoopbackHost(uri.host)) continue;
      // 局域网优先，其次 Tailscale，最后其它。旧版桌面可能把 Tailscale 排在
      // 局域网前面（`net_urls::assemble` 的顺序），这里不能假设二维码已排好。
      final rank = switch (classifyEndpoint(raw)) {
        SyncEndpointKind.lan => 0,
        SyncEndpointKind.tailscale => 1,
        _ => 2,
      };
      if (rank < bestRank) {
        best = raw.trim();
        bestRank = rank;
      }
    }
    return best;
  }

  /// 回环 / 模拟器专用地址（`10.0.2.2` 在手机上不是宿主机）。
  static bool isLoopbackHost(String host) {
    final h = host.toLowerCase();
    return h == 'localhost' ||
        h == '::1' ||
        h == '0.0.0.0' ||
        h == '10.0.2.2' ||
        h == '10.0.3.2' ||
        h.startsWith('127.');
  }

  /// 编成二维码要画的那串文本。
  String encode() {
    final params = <String, String>{'v': '$version', 't': ticket};
    if (expiresAt != null && expiresAt!.isNotEmpty) params['e'] = expiresAt!;
    if (urls.isNotEmpty) {
      params['u'] = base64Url
          .encode(utf8.encode(urls.join('\n')))
          .replaceAll('=', '');
    }
    final query = params.entries
        .map(
          (e) =>
              '${Uri.encodeQueryComponent(e.key)}=${Uri.encodeQueryComponent(e.value)}',
        )
        .join('&');
    return '$scheme://$host?$query';
  }

  /// 解析扫码结果。**不认识的一律如实回错误**，绝不猜。
  static PairingParse parse(String raw) {
    final text = raw.trim();
    if (text.isEmpty) return const PairingParse(error: '扫到的内容是空的');
    final uri = Uri.tryParse(text);
    if (uri == null) return const PairingParse(error: '这不是一个能解析的二维码内容');
    if (uri.scheme != scheme || uri.host != host) {
      // 如实把"扫到的是什么"说出来：用户往往是扫错了码（Wi-Fi 分享码、别的 App 的码）
      final what = uri.scheme.isEmpty
          ? '一段普通文本'
          : '${uri.scheme}://${uri.host} 的码';
      return PairingParse(error: '这不是 ComfyHub 的配对码（扫到的是$what）');
    }
    final q = uri.queryParameters;
    final v = q['v'];
    if (v == null || v.isEmpty) {
      return const PairingParse(error: '这个配对码没有版本号 —— 多半不是 ComfyHub 生成的');
    }
    if (v != '$version') {
      // ★ 版本不认识 ≠ 不是我们的码：如实让用户去更新 App
      return PairingParse(
        error: '这个配对码是 v$v 格式，比这台 App 认识的新（v$version）—— 请先更新手机上的 ComfyHub',
      );
    }
    final ticket = (q['t'] ?? '').trim();
    if (ticket.isEmpty) {
      return const PairingParse(error: '这个配对码里没有 ticket（内容不完整）');
    }

    var urls = <String>[];
    final rawUrls = q['u'];
    if (rawUrls != null && rawUrls.isNotEmpty) {
      try {
        // 编码时去掉了 padding，解回来要补上（`base64Url.normalize` 就是干这个的）
        final decoded = utf8.decode(
          base64Url.decode(base64Url.normalize(rawUrls)),
        );
        urls = decoded
            .split('\n')
            .map((u) => u.trim())
            .where((u) => u.startsWith('http://') || u.startsWith('https://'))
            .toList();
      } catch (_) {
        return const PairingParse(error: '这个配对码里的地址列表读不出来（内容损坏）');
      }
    }
    return PairingParse(
      payload: PairingPayload(ticket: ticket, expiresAt: q['e'], urls: urls),
    );
  }
}

/// 解析结果：要么有 [payload]，要么有 [error]（**两者不同时为空**）。
class PairingParse {
  const PairingParse({this.payload, this.error});

  final PairingPayload? payload;
  final String? error;

  bool get ok => payload != null;
}
