import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show FilteringTextInputFormatter;

import '../core/api_client.dart' show ApiException;
import '../core/formatting.dart';
import '../core/inbound_auth_api.dart';
import '../core/pairing_payload.dart';
import '../core/pairing_token.dart';
import '../core/sync_endpoints.dart';
import '../widgets/pairing_qr.dart';
import 'scan_pairing_page.dart';

/// 设置页里的「远程访问与配对」一块（形状见 `docs/pairing-design.md` §11）。
///
/// # 它解决的是"人怎么把令牌交到手机手里"
///
/// 协议里的 `X-ComfyHub-Token` 早就有，但**手机上一次都没有值**（Dart 侧调用时
/// `token` 恒为 `null`，见 `AiWorkspaceStore.pairingToken`）。这一块就是那条
/// **不靠扫码**的交接路径（§11.1 通道 B）：
///
///   手机举手（`pair/request`）→ 桌面上显示 6 位码 → 人在手机上把码敲进去（`pair/claim`）
///
/// 安全属性只有一条，而且必须看得见：**码由桌面算出、只出现在桌面屏幕上**。
/// 所以手机**拿不到** ticket（`pair/request` 的响应里没有这个字段），
/// 它就只能靠人念一遍 —— 这是设计里刻意留的那道闸门，不是漏做的功能。
///
/// # 桌面与手机同一份代码、按平台分叉内容
///
/// 判据用 `Platform.isAndroid`（与 `openRunEventSource` / `openCredentialStore` 一致）：
/// 桌面是"开闸门 + 发码 + 管设备"，手机是"举手 + 敲码 + 存令牌 + 取密钥"。
/// 两边的端点不同（除了 `pair/request` / `pair/claim`，桌面那几个都只认回环），
/// 所以不硬凑成一棵组件树。
class PairingSection extends StatefulWidget {
  const PairingSection({
    super.key,
    required this.baseUrl,
    this.onPaired,
    this.onSyncCredentials,
    this.onSaveBaseUrl,
    this.apiFactory,
  });

  /// 当前后端地址（桌面 = `SettingsStore.baseUrl`；手机 = 手动填写的桌面地址）。
  final String baseUrl;

  /// 配对成功 / 忘记这台电脑之后叫一次：让 `AiWorkspaceStore` 重新读令牌、
  /// 重新取 run-kit（配对前失败的那一次很可能就是因为没有令牌）。
  final Future<void> Function()? onPaired;

  /// 「从桌面同步密钥」：返回**给人看的一句话**（拿到几把、哪几把没拿到）。
  ///
  /// 不在这里直接读密钥：值是"只在写入密钥库那一刻出现一次"的东西，
  /// 界面层不该经手它。没接线时这颗按钮不显示。
  final Future<String> Function()? onSyncCredentials;

  /// 手机上改完桌面地址写回 `SettingsStore.setBaseUrl`。没接线时只能看不能改。
  final Future<void> Function(String url)? onSaveBaseUrl;

  /// 仅供测试注入（默认 `InboundAuthApi(baseUrl)` 自带一个 http 客户端）。
  ///
  /// 没有它就没法在 widget 测试里喂"/api/auth 回了 ok:false"这种响应 ——
  /// 而**"后端没接受时开关不许画成已打开"**正是这一块最容易悄悄做错的地方。
  final InboundAuthApi Function(String baseUrl)? apiFactory;

  @override
  State<PairingSection> createState() => _PairingSectionState();
}

class _PairingSectionState extends State<PairingSection> {
  InboundAuthApi? _api;

  bool _loading = true;
  bool _busy = false;

  /// 如实说出来的失败 / 结果（永不"假装成功"）。
  String? _error;
  String? _message;

  // --- 桌面侧 ---
  bool _enabled = false;
  bool _hasToken = false;
  List<PairedDevice> _devices = const [];
  List<PendingPairing> _pending = const [];

  /// 「配对新手机」那一区展开了没有。
  ///
  /// **只有展开时才轮询**（§11 的实现纪律）：收起来就 `Timer.cancel`，
  /// 页面切走时整个 State 被销毁，定时器也跟着在 `dispose` 里收掉 ——
  /// 一个设置页不该在后台一直问后端"有人要配对吗"。
  bool _pendingExpanded = false;
  bool _polling = false;
  String? _pendingError;

  /// **一个计时器管两件事**（2026-09-30 合并：原来扫码一个 1s、轮询一个 2s）：
  /// 每秒重画（跳秒）+ 每两秒问一次"手机举手了吗"。
  ///
  /// 跑的条件是"有活会话"：区展开了，**或者**桌面上正摆着一个二维码等着人扫
  /// —— 二维码过期时也由它撤掉（避免扫描失效码）。
  Timer? _sessionTimer;
  int _sessionTick = 0;

  /// 本机局域网 IPv4（供手动输入到手机）。
  List<String> _localIps = const [];

  // --- 扫码配对（通道 A：桌面出二维码、手机扫；§9 拍板）---
  /// 桌面上正在展示的那次配对（ticket + 滚动码 + 可达地址）。
  ///
  /// ⚠ 它和 `_pending`（手机举手那条路）**是同一种东西**：后端 `pair/start` 与 `pair/request`
  /// 造的都是同一种 `Session`、`claim` 两条路都认。区别只有"二维码是谁生成的"，
  /// 所以界面上也收成同一种卡片（见 [`_sessionCard`]）。
  PairStart? _qr;
  String? _qrError;
  bool _qrBusy = false;

  // --- 手机侧 ---
  late final TextEditingController _baseUrlController;
  final TextEditingController _codeController = TextEditingController();
  bool _paired = false;

  String? _pairedDeviceId;
  bool _pairing = false;
  bool _syncingKeys = false;

  /// `Platform.isAndroid`：个别环境下平台判断会抛异常（同 `SettingsStore.defaultBaseUrl`）。
  bool get _isAndroid {
    try {
      return Platform.isAndroid;
    } catch (_) {
      return false;
    }
  }

  @override
  void initState() {
    super.initState();
    _baseUrlController = TextEditingController(text: widget.baseUrl);
    if (_isAndroid) {
      // 手机侧只有"举手 / 兑码"两件事，客户端在 `_pair()` 里按**输入框里那个地址**现造
      unawaited(_loadPairedState());
    } else {
      _api = _openApi(widget.baseUrl);
      unawaited(_loadStatus());
      unawaited(_loadLocalIps());
    }
  }

  InboundAuthApi _openApi(String baseUrl) =>
      (widget.apiFactory ?? (b) => InboundAuthApi(b))(baseUrl);

  @override
  void didUpdateWidget(covariant PairingSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.baseUrl == oldWidget.baseUrl) return;
    // 地址变化（手机上修改、桌面后端重启换端口）：整个客户端更换一个
    _api?.dispose();
    _api = null;
    if (_isAndroid) {
      if (_baseUrlController.text != widget.baseUrl) {
        _baseUrlController.text = widget.baseUrl;
      }
    } else {
      _api = _openApi(widget.baseUrl);
      unawaited(_loadStatus());
      unawaited(_loadLocalIps());
    }
  }

  @override
  void dispose() {
    _sessionTimer?.cancel();
    _sessionTimer = null;
    _api?.dispose();
    _baseUrlController.dispose();
    _codeController.dispose();
    super.dispose();
  }

  // -------------------------------------------------------------------------
  //  扫码配对（通道 A）：桌面出二维码 → 手机扫 → 手机把码敲进来
  // -------------------------------------------------------------------------

  /// 让后端起一次扫码配对会话，并把二维码画出来（§5.1，**仅回环**）。
  ///
  /// 二维码那串文本由 [`PairingPayload.encode`] 编 —— 格式只有那一份实现。
  Future<void> _startQrPairing() async {
    final api = _api;
    if (api == null) return;
    setState(() {
      _qrBusy = true;
      _qrError = null;
      _message = null;
    });
    try {
      final start = await api.startPairing(token: PairingTokenCache.read());
      if (!mounted) return;
      if (!start.usable) {
        setState(() => _qrError = '桌面回的配对信息不完整，无法生成二维码。');
        return;
      }
      setState(() => _qr = start);
      _syncSessionTimer();
    } on ApiException catch (e) {
      if (!mounted) return;
      setState(() {
        _qrError = switch (e.statusCode) {
          404 || 501 => '这台后端还没有"扫码配对"接口（旧版本）—— 用上面那条手动敲验证码的路。',
          403 => '生成二维码只允许在桌面本机上操作（127.0.0.1）。',
          _ => '生成二维码失败，${e.message}',
        };
      });
    } catch (e) {
      if (mounted) setState(() => _qrError = '生成二维码失败，$e');
    } finally {
      if (mounted) setState(() => _qrBusy = false);
    }
  }

  /// **唯一的计时器**：跳秒 + 每两秒问一次"手机举手了吗" + 二维码到点自清。
  void _syncSessionTimer() {
    // 有活会话才跑：区展开了（有人在等）或桌面上摆着二维码（等人扫）
    final need = mounted && (_pendingExpanded || _qr != null);
    if (!need) {
      _sessionTimer?.cancel();
      _sessionTimer = null;
      return;
    }
    _sessionTimer ??= Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      final qr = _qr;
      if (qr != null &&
          (_remaining(qr.expiresAt) ?? Duration.zero) == Duration.zero) {
        setState(() {
          _qr = null;
          _qrError = '二维码已过期（120 秒有效）—— 点「生成二维码」再来一次。';
        });
        _syncSessionTimer();
        return;
      }
      setState(() {}); // 只为了让"剩余 mm:ss"跳秒
      _sessionTick++;
      if (_sessionTick % 2 == 0) unawaited(_pollPending()); // 每 2 秒轮一次
    });
  }

  /// 到期时刻 → 剩余时长（解析不出来时回 null：**不编一个倒计时**）。
  Duration? _remaining(String? iso) {
    if (iso == null || iso.isEmpty) return null;
    final at = DateTime.tryParse(iso);
    if (at == null) return null;
    final left = at.toLocal().difference(DateTime.now());
    return left.isNegative ? Duration.zero : left;
  }

  /// 剩余时长 → `m:ss`（登在会话卡右上角；解析不出来就不显示，**不编一个倒计时**）。
  String _mmss(Duration left) {
    final m = left.inMinutes;
    final s = left.inSeconds.remainder(60).toString().padLeft(2, '0');
    return '$m:$s';
  }

  // -------------------------------------------------------------------------
  //  桌面侧
  // -------------------------------------------------------------------------

  Future<void> _loadStatus() async {
    final api = _api;
    if (api == null) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final s = await api.status();
      if (!mounted) return;
      setState(() {
        _enabled = s.enabled;
        _hasToken = s.hasToken;
        _devices = s.devices;
        _pending = s.pending;
        // 已经有人在等配对：直接展开 —— 否则 6 位码藏在收起的折叠里，
        // 手机上已显示提示，但桌面上没有二维码。
        if (s.pending.isNotEmpty) _pendingExpanded = true;
      });
      _syncSessionTimer();
    } catch (e) {
      if (mounted) setState(() => _error = '读取配对状态失败，$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 展开 / 收起「配对新手机」，并据此开关轮询。
  void _togglePending() {
    setState(() => _pendingExpanded = !_pendingExpanded);
    _syncSessionTimer();
    if (_pendingExpanded) unawaited(_pollPending());
  }

  Future<void> _pollPending() async {
    if (_polling) return;
    final api = _api;
    if (api == null) return;
    _polling = true;
    try {
      final list = await api.pending();
      if (!mounted) return;
      final before = _pending.map((p) => p.requestId).toSet();
      final after = list.map((p) => p.requestId).toSet();
      final claimed = before.difference(after).isNotEmpty;
      setState(() {
        _pending = list;
        _pendingError = null;
      });
      // 待配对少了一条 = 手机刚点完「配对」。设备列表要立刻跟上，
      // 否则桌面上「已配对设备」需要手动重进该页才出现
      // （2026-11-08 真机：手机已连上，桌面还只显示旧设备）。
      if (claimed) unawaited(_loadStatus());
    } catch (e) {
      if (!mounted) return;
      // 拉不到就不再每 2 秒撞一次（旧后端没有这个端点时尤其明显），
      // 在界面上显示原因，点「重试」后重新开始轮询
      setState(() => _pendingError = '读取待配对列表失败，$e');
      _syncSessionTimer();
    } finally {
      _polling = false;
    }
  }

  Future<void> _setEnabled(bool value) async {
    final api = _api;
    if (api == null) return;
    setState(() {
      _busy = true;
      _error = null;
      _message = null;
    });
    try {
      final next = await api.setEnabled(value);
      if (!mounted) return;
      setState(() {
        // ⚠ 画的是**后端说的那个状态**，不是"我请求的那个状态"：
        // 后端没接受（响应里 enabled 还是 false）时必须如实显示，
        // 否则会显示为已打开，但手机无法连接，且界面上没有提示。
        _enabled = next.enabled;
        _hasToken = next.hasToken;
        _devices = next.devices;
        _pending = next.pending;
        if (next.enabled == value) {
          _message = value
              ? '已开启「允许移动端连接」，手机要带配对令牌才能连这台电脑。'
              : '已关闭，局域网内任何设备都能连。';
        } else {
          _error = '后端没有接受这个开关（响应里 enabled=${next.enabled}），设置没有被改。';
        }
      });
    } catch (e) {
      if (mounted) setState(() => _error = '切换失败，$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _revoke(PairedDevice d) async {
    final api = _api;
    if (api == null) return;
    final ok = await _confirm(
      '吊销「${d.name.isEmpty ? d.id : d.name}」？',
      '这台设备手上的令牌会立刻失效，'
          '要重新连上就得在手机上重新配对一次。',
    );
    if (ok != true || !mounted) return;
    setState(() {
      _busy = true;
      _error = null;
      _message = null;
    });
    try {
      await api.revoke(d.id);
      final list = await api.devices();
      if (!mounted) return;
      setState(() {
        _devices = list;
        _message = '已吊销「${d.name.isEmpty ? d.id : d.name}」的令牌。';
      });
    } catch (e) {
      if (mounted) setState(() => _error = '吊销失败，$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 改这台设备的凭据开关（**读与写是两个开关**，`docs/pairing-design.md` §14.1）。
  ///
  /// 只改一个时另一个**原样传当前值**（后端按"传了就改"处理）；本地重建设备行时
  /// 必须把**两个**开关都带上 —— 漏掉一个就是"点下行、上行被悄悄关掉"，
  /// 下一次点「推给桌面」会返回 403。
  Future<void> _setDeviceCredentials(
    PairedDevice d, {
    bool? allow,
    bool? allowUpload,
  }) async {
    final api = _api;
    if (api == null) return;
    final wantAllow = allow ?? d.allowCredentials;
    setState(() {
      _busy = true;
      _error = null;
      _message = null;
    });
    try {
      final updated = await api.setAllowCredentials(
        d.id,
        wantAllow,
        allowUpload: allowUpload,
      );
      if (!mounted) return;
      final name = d.name.isEmpty ? d.id : d.name;
      setState(() {
        _devices = [
          for (final x in _devices)
            if (x.id == d.id)
              PairedDevice(
                id: x.id,
                name: x.name,
                pairedAt: x.pairedAt,
                lastSeenAt: x.lastSeenAt,
                lastIp: x.lastIp,
                allowCredentials: updated.allowCredentials,
                allowCredentialsUpload: updated.allowCredentialsUpload,
                // 这个接口不动自动提交，但重建行时**必须原样带上**：漏掉就是
                // "点一下密钥开关，自动提交被悄悄关掉"。
                autoApproveSubmits: x.autoApproveSubmits,
              )
            else
              x,
        ];
        if (allowUpload != null) {
          if (allowUpload && !updated.allowCredentialsUpload) {
            // 回读为 false = 这台后端**不认识**这个开关（旧版本）—— 如实说，别假装开了
            _error =
                '这台后端还不支持"允许设备改密钥"（旧版本），开关没有生效。'
                '升级桌面后端之后再来勾这一项。';
          } else {
            _message = updated.allowCredentialsUpload
                ? '已允许「$name」改桌面上的密钥，当桌面联网且客户端在线时，手机端改的密钥会推送到桌面端。'
                : '已禁止「$name」改桌面上的密钥。';
          }
        } else {
          _message = updated.allowCredentials
              ? '已允许「$name」取密钥。'
              : '已禁止「$name」取密钥。';
        }
      });
    } catch (e) {
      if (mounted) setState(() => _error = '设置失败，$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 改这台设备的「自动提交」开关（免桌面批准，默认关）。
  ///
  /// 打开后，这台手机排队提交的 `submit_job` 会直接以 `approval='auto'` 进队列，
  /// 不再等桌面点批准。代价：拿到这台手机或它的配对令牌的人可以**不经过桌面确认
  /// 就消耗 GPU** —— 文案必须把这句写出来，而不是只写"更省事"。
  Future<void> _setDeviceAutoApprove(PairedDevice d, bool enabled) async {
    final api = _api;
    if (api == null) return;
    final name = d.name.isEmpty ? d.id : d.name;
    setState(() {
      _busy = true;
      _error = null;
      _message = null;
    });
    try {
      final updated = await api.setAutoApproveSubmits(d.id, enabled);
      if (!mounted) return;
      setState(() {
        _devices = [
          for (final x in _devices)
            if (x.id == d.id)
              PairedDevice(
                id: x.id,
                name: x.name,
                pairedAt: x.pairedAt,
                lastSeenAt: x.lastSeenAt,
                lastIp: x.lastIp,
                allowCredentials: x.allowCredentials,
                allowCredentialsUpload: x.allowCredentialsUpload,
                autoApproveSubmits: updated.autoApproveSubmits,
              )
            else
              x,
        ];
        if (enabled && !updated.autoApproveSubmits) {
          // 回读为 false = 这台后端不认识这个开关（旧版本）—— 如实说，别假装开了
          _error =
              '这台后端还不支持"自动提交"（旧版本），开关没有生效。'
              '升级桌面后端之后再来勾这一项。';
        } else {
          _message = updated.autoApproveSubmits
              ? '已允许「$name」免桌面批准自动提交。这台手机之后排队的图会直接跑，'
                    '不再等你点批准；随时可以在这里关掉。'
              : '已恢复「$name」的提交需要桌面批准。';
        }
      });
    } catch (e) {
      if (mounted) setState(() => _error = '设置失败，$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 本机可达地址：`NetworkInterface.list()` 里的非回环 IPv4 + **当前后端端口**。
  Future<void> _loadLocalIps() async {
    try {
      final nics = await NetworkInterface.list(
        type: InternetAddressType.IPv4,
        includeLoopback: false,
      );
      final ips = <String>[
        for (final n in nics)
          for (final a in n.addresses)
            if (!a.isLoopback && a.address.isNotEmpty) a.address,
      ]..sort((a, b) => _ipRank(a).compareTo(_ipRank(b)));
      if (!mounted) return;
      setState(() => _localIps = ips.take(4).toList());
    } catch (_) {
      // 拿不到网卡列表就不显示（界面上那句"没找到局域网地址"照旧成立），不报错
    }
  }

  /// 家用网段排在前面：VPN / docker / 虚拟网卡的地址排在后面（它们通常连不通手机）。
  static int _ipRank(String ip) {
    if (ip.startsWith('192.168.')) return 0;
    if (ip.startsWith('10.')) return 1;
    if (RegExp(r'^172\.(1[6-9]|2\d|3[01])\.').hasMatch(ip)) return 2;
    if (ip.startsWith('169.254.')) return 9; // link-local：基本没用
    return 5;
  }

  /// 手机上要填的地址（端口从当前后端地址里取 —— 桌面写的是 `127.0.0.1:<真实端口>`）。
  List<String> get _reachableUrls {
    final base = Uri.tryParse(widget.baseUrl.trim());
    if (base == null || !base.hasScheme || base.host.isEmpty) return const [];
    return [for (final ip in _localIps) '${base.scheme}://$ip:${base.port}'];
  }

  // -------------------------------------------------------------------------
  //  手机侧
  // -------------------------------------------------------------------------

  /// 冷启动：本机密钥库里**有没有**配对令牌（有就显示"已配对"）。
  Future<void> _loadPairedState() async {
    setState(() => _loading = true);
    try {
      final token = await PairingTokenStore().read();
      var deviceId = '';
      if (token != null) {
        try {
          deviceId = (await PairingTokenStore().identity()).deviceId;
        } catch (_) {
          // 身份读不到不影响"已配对"这件事（令牌在就行）
        }
      }
      if (!mounted) return;
      setState(() {
        _paired = token != null;
        _pairedDeviceId = deviceId.isEmpty ? null : deviceId;
      });
    } catch (e) {
      if (mounted) setState(() => _error = '读取本机配对状态失败，$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 扫桌面上的二维码（通道 A，§3.2）→ **扫完直接接着配对**。
  ///
  /// 两件事各管一段：扫码解决"去哪、传什么"（把 ticket 和地址带回来），
  /// 敲 6 位码解决"你是不是就站在旁边"。所以扫完**不要求再点一次按钮**
  /// （扫码完成后直接继续配对，不需要再点「配对新电脑」）。
  ///
  /// ⚠ 顺手修掉一个真 bug：老代码把扫到的地址只写进一句提示（"地址已填好"），
  /// **从来没填进输入框** —— 于是"二维码省掉手抄地址"这件事根本没发生，
  /// 配对照样拿的是上一次存的地址。现在真的填进去（下面第一句）。
  Future<void> _scanThenPair() async {
    final payload = await Navigator.of(context).push<PairingPayload>(
      MaterialPageRoute(builder: (_) => const ScanPairingPage()),
    );
    if (payload == null || !mounted) return;
    // ⚠ 不能拿 `urls.first`：桌面的候选列表里回环（`127.0.0.1`）天然排在最前，
    // 手机上连它等于连自己 —— 扫码 + 验证码全对，界面只报"连接失败"。
    //
    // 也不能只拿 `preferredUrl`：校园 / 公司 Wi-Fi 常把 IPv4 在 AP 之间隔离、
    // 只留 IPv6 能通（2026-11-08 实测）。二维码里两条都有时，逐条真探，
    // 哪条 `/api/health` 通就用哪条。
    final candidates = <String>[
      if (payload.preferredUrl != null) payload.preferredUrl!,
      ...payload.urls,
    ];
    final working = await firstReachableUrl(candidates);
    if (!mounted) return;
    if (working != null) {
      _baseUrlController.text = working;
    } else {
      final typed = _baseUrlController.text.trim();
      final typedHost = Uri.tryParse(typed)?.host ?? '';
      final typedUsable =
          typed.isNotEmpty &&
          typedHost.isNotEmpty &&
          !PairingPayload.isLoopbackHost(typedHost);
      if (!typedUsable) {
        setState(
          () => _error =
              '二维码里的地址都连不上（试了 ${candidates.length} 条，含 IPv6）。'
              '确认手机和电脑在同一个 Wi-Fi，或电脑的 IPv6 可达；'
              '也可以先在上面的输入框手填桌面地址再扫。',
        );
        return;
      }
      // 手动填写的地址可用：沿用它继续配对。
    }
    await _pair(scanned: payload);
  }

  /// 配对新电脑：举手 → 让人敲桌面上那 6 位码 → 换令牌 → **立刻存进密钥库**。
  ///
  /// `scanned` = 扫到的二维码（有它就不用"举手"那一趟：桌面已经算好码了）。
  Future<void> _pair({PairingPayload? scanned}) async {
    if (_pairing) return;
    // 输入框里的地址就是"我要连的那台桌面"。不先把它存下来直接配对的话，
    // 修改地址后点配对会**使用旧地址请求**，而界面显示新地址 —— 位置不一致。
    final typed = _baseUrlController.text.trim();
    final base = typed.isEmpty ? widget.baseUrl : typed;
    setState(() {
      _pairing = true;
      _error = null;
      _message = null;
    });
    final api = _openApi(base);
    try {
      if (widget.onSaveBaseUrl != null &&
          typed.isNotEmpty &&
          typed != widget.baseUrl) {
        await widget.onSaveBaseUrl!(typed);
      }
      // 身份必须与上行同步用的是**同一个** deviceId（生成两次 = 桌面当成两台手机）
      final identity = await PairingTokenStore().identity();
      // 两条通道（§3.2）：**扫码那条桌面已经算好码了**，手机不用再举手；
      // 手敲那条要先 requestPairing 让桌面把码显示出来。
      String? requestId;
      if (scanned == null) {
        final req = await api.requestPairing(
          deviceId: identity.deviceId,
          deviceName: identity.deviceName,
        );
        requestId = req.requestId;
      }
      if (!mounted) return;
      _codeController.clear();
      final code = await _askCode();
      if (code == null || !mounted) return; // 用户取消：什么也没发生
      final claim = await api.claim(
        code: code,
        deviceId: identity.deviceId,
        deviceName: identity.deviceName,
        requestId: requestId,
        // ticket 来自扫到的二维码；手敲那条走 requestId（ticket 到不了手机上）
        ticket: scanned?.ticket,
      );
      // 令牌**只在这个响应里出现一次**：先存进密钥库，存下了才算配对成功。
      // 存失败就把它当场丢掉（它只活在这个局部变量里，不写进任何状态），并如实报错 ——
      // 悄悄留一个"这次能用、下次启动就没了"的令牌，等于每次开 App 都要重配一遍。
      try {
        await PairingTokenStore().write(claim.token);
      } catch (e) {
        if (!mounted) return;
        setState(
          () => _error =
              '配对令牌写不进本机密钥库，这次配对作废，令牌已经丢弃，'
              '没有任何地方留着它。错误是 $e。',
        );
        return;
      }
      if (!mounted) return;
      setState(() {
        _paired = true;
        _pairedDeviceId = claim.deviceId;
        _message =
            '已配对（${_prefix(claim.deviceId)}）。'
            '桌面把「${identity.deviceName}」记成了这台设备。';
      });
      await widget.onPaired?.call();
    } on ApiException catch (e) {
      if (mounted) setState(() => _error = _pairErrorText(e));
    } catch (e) {
      if (mounted) setState(() => _error = '配对失败，$e');
    } finally {
      api.dispose();
      if (mounted) setState(() => _pairing = false);
    }
  }

  /// 弹出输入 6 位码的对话框（取消时返回 null）。
  Future<String?> _askCode() {
    return showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('输入电脑上显示的 6 位码'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('在电脑的「设置 → 远程访问与配对 → 配对新手机」里能看到这组码。'),
            const SizedBox(height: 12),
            TextField(
              controller: _codeController,
              autofocus: true,
              keyboardType: TextInputType.number,
              maxLength: 6,
              inputFormatters: [FilteringTextInputFormatter.digitsOnly],
              decoration: const InputDecoration(
                labelText: '6 位配对码',
                hintText: '例如 428193',
              ),
              onSubmitted: (v) {
                if (v.trim().length == 6) Navigator.pop(ctx, v.trim());
              },
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('取消'),
          ),
          // 位数不够就点不动：让后端回一句"配对失败"不如在按钮上就拦住
          ValueListenableBuilder<TextEditingValue>(
            valueListenable: _codeController,
            builder: (_, value, _) {
              final ok = value.text.trim().length == 6;
              return FilledButton(
                onPressed: ok
                    ? () => Navigator.pop(ctx, value.text.trim())
                    : null,
                child: const Text('配对'),
              );
            },
          ),
        ],
      ),
    );
  }

  /// 配对失败时把后端给的**人话**显示出来，并说清下一步该干什么（§11.2 的错误码）。
  String _pairErrorText(ApiException e) {
    final human = (e.detail == null || e.detail!.isEmpty)
        ? e.message
        : '${e.message}（${e.detail}）';
    switch (e.statusCode) {
      case 400:
        return '配对没通过。$human '
            '6 位码要按桌面上显示的那组原样敲，试错 5 次这张码就作废了。';
      case 410:
        return '这次配对会话已经失效。$human 请点「配对新电脑」重新举手，桌面会给出新的一组码。';
      case 429:
        return '请求太频繁了。$human 等一会儿再点「配对新电脑」。';
      case 403:
        return '桌面拒绝了这次配对。$human 确认桌面上的「允许移动端连接」已经打开。';
      default:
        return human;
    }
  }

  /// 忘记这台电脑：**只清本机密钥库**。
  ///
  /// 刻意**不调** `revoke`：那是只认回环的端点，手机调过去必然 403 ——
  /// 要真正把设备从桌面上摘掉，得在桌面上点那台设备的「吊销」。
  Future<void> _forget() async {
    final ok = await _confirm(
      '忘记这台电脑？',
      '只会删掉本机密钥库里的配对令牌，本机不再带它连桌面。'
          '桌面上的那条设备记录还在（要一起清掉，请在桌面上点那台设备的「吊销」）。',
    );
    if (ok != true || !mounted) return;
    setState(() {
      _error = null;
      _message = null;
    });
    try {
      await PairingTokenStore().clear();
      if (!mounted) return;
      setState(() {
        _paired = false;
        _pairedDeviceId = null;
        _message = '已忘记这台电脑（本机不再带配对令牌）。';
      });
      await widget.onPaired?.call();
    } catch (e) {
      if (mounted) setState(() => _error = '忘记失败，$e');
    }
  }

  Future<void> _saveBaseUrl() async {
    final save = widget.onSaveBaseUrl;
    if (save == null) return;
    final url = _baseUrlController.text.trim();
    setState(() {
      _error = null;
      _message = null;
    });
    try {
      await save(url);
      if (!mounted) return;
      setState(
        () => _message = url.isEmpty
            ? '已清空桌面地址（桌面端 = 自动发现本机后端）。'
            : '桌面地址已保存为 $url',
      );
    } catch (e) {
      if (mounted) setState(() => _error = '保存地址失败，$e');
    }
  }

  Future<void> _syncCredentials() async {
    final fn = widget.onSyncCredentials;
    if (fn == null || _syncingKeys) return;
    setState(() {
      _syncingKeys = true;
      _error = null;
      _message = null;
    });
    try {
      final text = await fn();
      if (mounted) setState(() => _message = text);
    } catch (e) {
      if (mounted) setState(() => _error = '从桌面同步密钥失败，$e');
    } finally {
      if (mounted) setState(() => _syncingKeys = false);
    }
  }

  // -------------------------------------------------------------------------
  //  公共小件
  // -------------------------------------------------------------------------

  Future<bool?> _confirm(String title, String body) => showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(ctx, false),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, true),
          child: const Text('确认'),
        ),
      ],
    ),
  );

  static String _prefix(String deviceId) {
    if (deviceId.isEmpty) return '设备 id 未知';
    final head = deviceId.length <= 8 ? deviceId : deviceId.substring(0, 8);
    return '设备 $head…';
  }

  static DateTime? _parseTime(String? raw) {
    if (raw == null || raw.isEmpty) return null;
    return DateTime.tryParse(raw)?.toLocal();
  }

  /// 结果 / 错误提示（中文、如实）。
  Widget _feedback(ThemeData theme) {
    final rows = <Widget>[];
    if (_error != null) {
      rows.add(
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(Icons.error_outline, size: 18, color: theme.colorScheme.error),
            const SizedBox(width: 8),
            Expanded(
              child: SelectableText(_error!, style: theme.textTheme.bodySmall),
            ),
          ],
        ),
      );
    }
    if (_message != null) {
      rows.add(
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Icon(
              Icons.check_circle_outline,
              size: 18,
              color: Colors.green,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: SelectableText(
                _message!,
                style: theme.textTheme.bodySmall,
              ),
            ),
          ],
        ),
      );
    }
    if (rows.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (var i = 0; i < rows.length; i++) ...[
            if (i > 0) const SizedBox(height: 6),
            rows[i],
          ],
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return _isAndroid ? _androidCard(theme) : _desktopCard(theme);
  }

  // -------------------------------------------------------------------------
  //  桌面（Windows）：开闸门 + 发码 + 管设备
  // -------------------------------------------------------------------------

  Widget _desktopCard(ThemeData theme) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 8, 8, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 4, 8, 8),
              child: Text(
                '打开后，手机要带上配对令牌才能连这台电脑的 AI。配对时码由这台电脑算，只显示在下面。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
            if (_loading)
              const Padding(
                padding: EdgeInsets.fromLTRB(16, 4, 16, 12),
                child: LinearProgressIndicator(minHeight: 2),
              ),
            _switchRow(
              title: '允许移动端连接',
              // ⚠ 这一项是**配对闸门**（局域网请求要不要带令牌），**不是**
              // 「本地服务」那张卡里的「允许手机 / 局域网设备连接」（那个决定
              // 后端听不听局域网）。两个名字都像"允许手机连"，所以两边都要把
              // 顺序写出来 —— 关掉它不是"不允许连接"，而是"局域网内任何设备
              // 都能连、不用令牌"，那反而更松。
              subtitle: _enabled
                  ? '② 已开启，手机要带配对令牌才能连，没带会被 403 挡下。'
                        '前提是「本地服务」里的 ① 已开且重启过后端。'
                  : '② 未开启，局域网内任何设备都能连，不需要令牌。'
                        '要连手机的话建议开启，前提是「本地服务」里的 ① 已开。',
              value: _enabled,
              onChanged: (_busy || _loading) ? null : _setEnabled,
            ),
            const Divider(height: 1),
            // ① 配对新手机（展开时才轮询）
            ListTile(
              contentPadding: const EdgeInsets.symmetric(horizontal: 8),
              leading: const Icon(Icons.qr_code_2_outlined),
              title: const Text('配对新手机'),
              subtitle: Text(
                _pending.isEmpty
                    ? '还没有待配对的请求'
                    : '有 ${_pending.length} 个待配对请求，下面这组码念给手机',
              ),
              trailing: Icon(
                _pendingExpanded ? Icons.expand_less : Icons.expand_more,
              ),
              onTap: _togglePending,
            ),
            if (_pendingExpanded) _sessionsArea(theme),
            const Divider(height: 1),
            // ② 手机上填哪个地址
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 10, 8, 0),
              child: Text('手机上填这个地址', style: theme.textTheme.titleSmall),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 4, 8, 0),
              child: _reachableUrls.isEmpty
                  ? Text(
                      '没找到局域网地址（这台电脑可能没连 Wi-Fi / 网线，或者后端不在本机）。'
                      '也可以自己在手机上看「设置 → 后端服务」里填地址。',
                      style: theme.textTheme.bodySmall,
                    )
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        for (final url in _reachableUrls)
                          SelectableText(
                            url,
                            style: theme.textTheme.bodyMedium?.copyWith(
                              fontFamily: 'monospace',
                            ),
                          ),
                        const SizedBox(height: 2),
                        Text(
                          '手机与这台电脑要在同一个局域网里，地址里的端口要与桌面后端一致。',
                          style: theme.textTheme.labelSmall?.copyWith(
                            color: theme.colorScheme.outline,
                          ),
                        ),
                      ],
                    ),
            ),
            const Divider(height: 18),
            // ③ 已配对设备
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 0, 8, 0),
              child: Row(
                children: [
                  Expanded(
                    child: Text('已配对设备', style: theme.textTheme.titleSmall),
                  ),
                  if (_hasToken)
                    Text(
                      '全局令牌已生成',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(height: 4),
            if (_devices.isEmpty)
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 0, 8, 4),
                child: Text('还没有配对的手机。', style: theme.textTheme.bodySmall),
              )
            else
              for (final d in _devices) _deviceTile(theme, d),
            _feedback(theme),
          ],
        ),
      ),
    );
  }

  /// 一条等待中的配对会话（桌面这侧）。
  ///
  /// ⚠ **只有一种卡片**：手机发起的（`pending`）与桌面生成二维码的（`qr`）
  /// 在界面上是同一种交互 —— 显示一组 6 位码等待输入，区别仅为二维码条目还包含
  /// ticket + 地址（扫码后无需手动输入地址）。此前有两套渲染与两套计时器，
  /// 需要分别理解"配对新手机"与"扫码配对"两个概念。
  Widget _sessionCard(
    ThemeData theme, {
    PairStart? qr,
    PendingPairing? pending,
  }) {
    final code = qr?.code ?? pending?.code ?? '';
    final left =
        _remaining(qr?.codeExpiresAt ?? pending?.codeExpiresAt) ??
        _remaining(qr?.expiresAt ?? pending?.expiresAt);
    final expired = left == Duration.zero;
    final ip = pending?.ip ?? '';
    final name = (pending?.deviceName ?? '').isEmpty
        ? '未知设备'
        : pending!.deviceName!;
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Material(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.3),
        borderRadius: BorderRadius.circular(12),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(
                    qr != null ? Icons.qr_code_2 : Icons.phone_iphone,
                    size: 16,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 6),
                  Text(
                    qr != null ? '扫码配对' : '手机举手',
                    style: theme.textTheme.labelMedium,
                  ),
                  const Spacer(),
                  // ⚠ 「剩余」每张卡**只许出现一次**（用例 `find.textContaining('剩余')` 数它）
                  Text(
                    left == null ? '' : (expired ? '已过期' : '剩余 ${_mmss(left)}'),
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: expired
                          ? theme.colorScheme.error
                          : theme.colorScheme.outline,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              // 6 位码是这一块**唯一要给人看的东西**：等宽大字 + 可选中（方便核对）
              SelectableText(
                code.isEmpty ? '······' : code,
                style: theme.textTheme.headlineSmall?.copyWith(
                  fontFamily: 'monospace',
                  fontWeight: FontWeight.w700,
                  letterSpacing: 6,
                ),
              ),
              const SizedBox(height: 2),
              Text(
                qr != null
                    ? '手机上敲这 6 位码（扫上面的二维码可省去手抄地址）'
                    : (expired
                          ? '这组码已过期，让手机重新点一次「配对新电脑」'
                          : '$name${ip.isEmpty ? '' : ' · 来自 $ip'} —— 在手机上敲这 6 位码'),
                style: theme.textTheme.labelSmall?.copyWith(
                  color: expired
                      ? theme.colorScheme.error
                      : theme.colorScheme.outline,
                ),
              ),
              if (qr != null) ...[
                const SizedBox(height: 8),
                // 二维码要**浅底**才扫得动 —— 白底那层在 `PairingQrImage` 里
                PairingQrImage(
                  payload: PairingPayload(
                    ticket: qr.ticket,
                    expiresAt: qr.expiresAt,
                    urls: qr.urls,
                  ),
                ),
                const SizedBox(height: 6),
                if (qr.urls.isEmpty)
                  Text(
                    '桌面没枚举出可达地址 —— 手机上请手填地址。',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.error,
                    ),
                  )
                else
                  for (final u in qr.urls)
                    SelectableText(u, style: theme.textTheme.labelSmall),
              ],
            ],
          ),
        ),
      ),
    );
  }

  /// 「配对新手机」那一区：说明 + 所有等待中的会话（两种来路混在一条列表里）。
  Widget _sessionsArea(ThemeData theme) {
    final qr = _qr;
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '两条路都行。手机上点「配对新电脑」（这里会出现一组 6 位码），'
            '或者桌面出二维码（手机扫码，地址不用手抄）。两条都还要在手机上敲那 6 位码。',
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              FilledButton.icon(
                onPressed: _qrBusy ? null : _startQrPairing,
                icon: const Icon(Icons.qr_code_2, size: 18),
                label: Text(qr == null ? '生成二维码' : '重新生成二维码'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          if (_qrError != null)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.error_outline,
                  size: 16,
                  color: theme.colorScheme.error,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(_qrError!, style: theme.textTheme.labelSmall),
                ),
              ],
            ),
          if (_pendingError != null)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.error_outline,
                  size: 18,
                  color: theme.colorScheme.error,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(_pendingError!, style: theme.textTheme.bodySmall),
                ),
                TextButton(
                  onPressed: () {
                    setState(() => _pendingError = null);
                    _syncSessionTimer();
                    unawaited(_pollPending());
                  },
                  child: const Text('重试'),
                ),
              ],
            ),
          // 两种来路混排：二维码那条排在上面（它带着地址，最该被看见）
          if (qr != null) _sessionCard(theme, qr: qr),
          for (final p in _pending) _sessionCard(theme, pending: p),
          if (qr == null &&
              _pending.isEmpty &&
              _pendingError == null &&
              _qrError == null)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Text('现在没有等待中的配对。', style: theme.textTheme.bodySmall),
            ),
        ],
      ),
    );
  }

  Widget _deviceTile(ThemeData theme, PairedDevice d) {
    final name = d.name.isEmpty ? '（未命名设备）' : d.name;
    final ip = d.lastIp ?? '';
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.smartphone, size: 18),
              const SizedBox(width: 8),
              Expanded(child: Text(name, style: theme.textTheme.bodyMedium)),
              TextButton(
                onPressed: _busy ? null : () => _revoke(d),
                child: const Text('吊销'),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 26, bottom: 4),
            child: Text(
              '最后活跃 ${relativeTime(_parseTime(d.lastSeenAt))}'
              '${ip.isEmpty ? '' : ' · 最后 IP $ip'}',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: _switchRow(
              title: '允许把密钥发给这台设备',
              subtitle: d.allowCredentials
                  ? '已允许，这台手机可以「从桌面同步密钥」把 API Key 取到本机密钥库。'
                  : '未允许，手机拿不到桌面上的 API Key（配对成功不等于能取密钥）。',
              value: d.allowCredentials,
              onChanged: _busy
                  ? null
                  : (v) => _setDeviceCredentials(d, allow: v),
            ),
          ),
          // ★ 第二个开关：**改**桌面上的密钥（上行，§14.1）。
          //   与上一个开关分开：读和写属于不同权限 —— 合并后容易
          //   在开启读权限时同时开启写权限，而写权限会使桌面后续请求
          //   使用外部写入的 key。
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: _switchRow(
              title: '允许这台设备改桌面上的密钥',
              subtitle: d.allowCredentialsUpload
                  ? '已允许，手机上可以「推给桌面」，把它本机密钥库里的那一把写到桌面上。'
                  : '未允许（默认），此时手机推上来的密钥会被桌面拒绝（403）。',
              value: d.allowCredentialsUpload,
              onChanged: _busy
                  ? null
                  : (v) => _setDeviceCredentials(d, allowUpload: v),
            ),
          ),
          // ★ 第三个开关：**免桌面批准自动提交**（默认关，2026-11-08）。
          //   打开后手机排队的 submit_job 直接跑，不再等桌面点批准 ——
          //   代价是拿到这台手机（或它的令牌）的人可以不确认就烧 GPU，
          //   所以文案把代价写出来，而不是只写"更省事"。
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: _switchRow(
              title: '允许这台手机自动提交（免桌面批准）',
              subtitle: d.autoApproveSubmits
                  ? '已开启：这台手机排队的图会直接跑，不再等你点批准。'
                        '拿到这台手机的人因此可以不经过桌面确认就消耗 GPU。'
                  : '未开启（默认）：每次排队提交都要在桌面点一次批准。',
              value: d.autoApproveSubmits,
              onChanged: _busy ? null : (v) => _setDeviceAutoApprove(d, v),
            ),
          ),
        ],
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  手机（Android）：举手 + 敲码 + 存令牌 + 取密钥
  // -------------------------------------------------------------------------

  Widget _androidCard(ThemeData theme) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '配对之后，这台手机会把配对令牌存进本机密钥库，用它连桌面取规则工件与密钥。'
              '配对码由桌面算出并显示在桌面屏幕上，手机这边只负责把它敲进来。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _baseUrlController,
              decoration: const InputDecoration(
                labelText: '桌面地址',
                hintText: '例如 http://192.168.1.20:8081',
                helperText: '电脑上「远程访问与配对 → 手机上填这个地址」里显示的那个地址',
              ),
            ),
            const SizedBox(height: 4),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                if (widget.onSaveBaseUrl != null)
                  TextButton.icon(
                    onPressed: _saveBaseUrl,
                    icon: const Icon(Icons.save_outlined, size: 18),
                    label: const Text('保存地址'),
                  ),
              ],
            ),
            const Divider(height: 20),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  _paired ? Icons.link : Icons.link_off,
                  size: 18,
                  color: _paired ? Colors.green : theme.colorScheme.outline,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    _paired
                        ? '已配对${_pairedDeviceId == null ? '' : '（${_prefix(_pairedDeviceId!)}）'}'
                        : '还没配对，配一次对之后，本机才能连桌面取规则工件与密钥。',
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 10,
              runSpacing: 8,
              children: [
                FilledButton.icon(
                  onPressed: _pairing ? null : _pair,
                  icon: _pairing
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.add_link),
                  label: const Text('配对新电脑'),
                ),
                if (_paired)
                  OutlinedButton.icon(
                    onPressed: _pairing ? null : _forget,
                    icon: const Icon(Icons.link_off),
                    label: const Text('忘记这台电脑'),
                  ),
                // 通道 A 的入口：桌面出二维码，手机扫过来（地址不用手抄）
                OutlinedButton.icon(
                  onPressed: _pairing ? null : _scanThenPair,
                  icon: const Icon(Icons.qr_code_scanner),
                  label: const Text('扫桌面上的二维码'),
                ),
                // 没接线时不显示：一颗点了没反应的按钮比没有按钮更糟
                if (_paired && widget.onSyncCredentials != null)
                  OutlinedButton.icon(
                    onPressed: _syncingKeys ? null : _syncCredentials,
                    icon: _syncingKeys
                        ? const SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.key_outlined),
                    label: const Text('从桌面同步密钥'),
                  ),
              ],
            ),
            if (_paired && widget.onSyncCredentials != null)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  '只会同步「桌面已允许同步」的密钥，没允许的不会下发。',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
            _feedback(theme),
          ],
        ),
      ),
    );
  }

  /// 开关行：与 `settings_page.dart` 里的 `_SwitchRow` **同一套做法**
  /// （自带内边距 + Material 底色、不用 `SwitchListTile(contentPadding: EdgeInsets.zero)` 贴边）。
  ///
  /// 为什么复制一份而不是复用：那边那个是**私有类**，跨文件用不了；
  /// 而它的两条要点（底色落在 `Material` 上、左右各留 10）必须一模一样，所以照着写。
  Widget _switchRow({
    required String title,
    required String subtitle,
    required bool value,
    required ValueChanged<bool>? onChanged,
  }) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Material(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.3),
        borderRadius: BorderRadius.circular(12),
        clipBehavior: Clip.antiAlias,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10),
          child: SwitchListTile(
            contentPadding: EdgeInsets.zero,
            value: value,
            onChanged: onChanged,
            title: Text(title),
            subtitle: Text(subtitle),
          ),
        ),
      ),
    );
  }
}
