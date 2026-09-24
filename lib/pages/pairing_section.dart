import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show FilteringTextInputFormatter;

import '../core/api_client.dart' show ApiException;
import '../core/formatting.dart';
import '../core/inbound_auth_api.dart';

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

  /// 当前后端地址（桌面 = `SettingsStore.baseUrl`；手机 = 用户手填的桌面地址）。
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
  Timer? _pollTimer;
  bool _polling = false;
  String? _pendingError;

  /// 本机局域网 IPv4（拼给用户照着往手机上敲）。
  List<String> _localIps = const [];

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
    // 地址变了（手机上用户改了、桌面上后端重启换了端口）：整个客户端换一个
    _api?.dispose();
    _api = null;
    if (_isAndroid) {
      if (_baseUrlController.text != widget.baseUrl) _baseUrlController.text = widget.baseUrl;
    } else {
      _api = _openApi(widget.baseUrl);
      unawaited(_loadStatus());
      unawaited(_loadLocalIps());
    }
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _pollTimer = null;
    _api?.dispose();
    _baseUrlController.dispose();
    _codeController.dispose();
    super.dispose();
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
        // 用户手机上明明看到了提示，却在桌面上找不到码。
        if (s.pending.isNotEmpty) _pendingExpanded = true;
      });
      _syncPolling();
    } catch (e) {
      if (mounted) setState(() => _error = '读取配对状态失败：$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 展开 / 收起「配对新手机」，并据此开关轮询。
  void _togglePending() {
    setState(() => _pendingExpanded = !_pendingExpanded);
    _syncPolling();
    if (_pendingExpanded) unawaited(_pollPending());
  }

  void _syncPolling() {
    final shouldPoll = mounted && _pendingExpanded && _pendingError == null;
    if (shouldPoll) {
      _pollTimer ??= Timer.periodic(const Duration(seconds: 2), (_) => unawaited(_pollPending()));
    } else {
      _pollTimer?.cancel();
      _pollTimer = null;
    }
  }

  Future<void> _pollPending() async {
    if (_polling) return;
    final api = _api;
    if (api == null) return;
    _polling = true;
    try {
      final list = await api.pending();
      if (!mounted) return;
      setState(() {
        _pending = list;
        _pendingError = null;
      });
    } catch (e) {
      if (!mounted) return;
      // 拉不到就不再每 2 秒撞一次（旧后端没有这个端点时尤其明显），
      // 把原因写在界面上，用户点「重试」再开始轮询
      setState(() => _pendingError = '读取待配对列表失败：$e');
      _syncPolling();
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
        // 否则用户以为打开了、手机却连不上，而界面上一点线索都没有。
        _enabled = next.enabled;
        _hasToken = next.hasToken;
        _devices = next.devices;
        _pending = next.pending;
        if (next.enabled == value) {
          _message = value
              ? '已开启「允许移动端连接」：手机要带配对令牌才能连这台电脑。'
              : '已关闭：局域网内任何设备都能连（今天的行为）。';
        } else {
          _error = '后端没有接受这个开关（响应里 enabled=${next.enabled}），设置没有被改。'
              '多半是后端还是旧构建 —— 在「本地服务」里重启一次本地服务。';
        }
      });
    } catch (e) {
      if (mounted) setState(() => _error = '切换失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _revoke(PairedDevice d) async {
    final api = _api;
    if (api == null) return;
    final ok = await _confirm(
      '吊销「${d.name.isEmpty ? d.id : d.name}」？',
      '这台设备手上的令牌会**立刻**失效（没有宽限期），它下次请求会拿到 401。'
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
      if (mounted) setState(() => _error = '吊销失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _setAllowCredentials(PairedDevice d, bool allow) async {
    final api = _api;
    if (api == null) return;
    setState(() {
      _busy = true;
      _error = null;
      _message = null;
    });
    try {
      final updated = await api.setAllowCredentials(d.id, allow);
      if (!mounted) return;
      setState(() {
        // ⚠ 后端只回 `{ok, id, allowCredentials}`（§11.2），**整条替换会把设备名
        // 和最后活跃时间擦掉** —— 所以只取"是否允许"这一个字段，其余保持原样。
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
              )
            else
              x,
        ];
        _message = updated.allowCredentials
            ? '已允许「${d.name.isEmpty ? d.id : d.name}」把桌面上的 API Key 取走。'
            : '已禁止「${d.name.isEmpty ? d.id : d.name}」取密钥：手机拿不到桌面上的 API Key。';
      });
    } catch (e) {
      if (mounted) setState(() => _error = '设置失败：$e');
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
      if (mounted) setState(() => _error = '读取本机配对状态失败：$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 配对新电脑：举手 → 让人敲桌面上那 6 位码 → 换令牌 → **立刻存进密钥库**。
  Future<void> _pair() async {
    if (_pairing) return;
    // 输入框里的地址就是"我要连的那台桌面"。不先把它存下来直接配对的话，
    // 用户改完地址点配对会**拿旧地址去问**、界面上却显示着新地址 —— 最难查的那类不一致。
    final typed = _baseUrlController.text.trim();
    final base = typed.isEmpty ? widget.baseUrl : typed;
    setState(() {
      _pairing = true;
      _error = null;
      _message = null;
    });
    final api = _openApi(base);
    try {
      if (widget.onSaveBaseUrl != null && typed.isNotEmpty && typed != widget.baseUrl) {
        await widget.onSaveBaseUrl!(typed);
      }
      // 身份必须与上行同步用的是**同一个** deviceId（生成两次 = 桌面当成两台手机）
      final identity = await PairingTokenStore().identity();
      final req = await api.requestPairing(
        deviceId: identity.deviceId,
        deviceName: identity.deviceName,
      );
      if (!mounted) return;
      _codeController.clear();
      final code = await _askCode();
      if (code == null || !mounted) return; // 用户取消：什么也没发生
      final claim = await api.claim(
        code: code,
        deviceId: identity.deviceId,
        deviceName: identity.deviceName,
        // ticket 走不到手机上（§11.1）：`pair/request` 的响应里只有 requestId
        requestId: req.requestId,
      );
      // 令牌**只在这个响应里出现一次**：先存进密钥库，存下了才算配对成功。
      // 存失败就把它当场丢掉（它只活在这个局部变量里，不写进任何状态），并如实报错 ——
      // 悄悄留一个"这次能用、下次启动就没了"的令牌，等于每次开 App 都要重配一遍。
      try {
        await PairingTokenStore().write(claim.token);
      } catch (e) {
        if (!mounted) return;
        setState(() => _error = '配对令牌写不进本机密钥库，这次配对作废（令牌已丢弃，'
            '没有任何地方留着它）：$e');
        return;
      }
      if (!mounted) return;
      setState(() {
        _paired = true;
        _pairedDeviceId = claim.deviceId;
        _message = '已配对（${_prefix(claim.deviceId)}）。'
            '桌面把「${identity.deviceName}」记成了这台设备。';
      });
      await widget.onPaired?.call();
    } on ApiException catch (e) {
      if (mounted) setState(() => _error = _pairErrorText(e));
    } catch (e) {
      if (mounted) setState(() => _error = '配对失败：$e');
    } finally {
      api.dispose();
      if (mounted) setState(() => _pairing = false);
    }
  }

  /// 弹出"敲 6 位码"的对话框（用户取消返回 null）。
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
              decoration: const InputDecoration(labelText: '6 位配对码', hintText: '例如 428193'),
              onSubmitted: (v) {
                if (v.trim().length == 6) Navigator.pop(ctx, v.trim());
              },
            ),
          ],
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          // 位数不够就点不动：让后端回一句"配对失败"不如在按钮上就拦住
          ValueListenableBuilder<TextEditingValue>(
            valueListenable: _codeController,
            builder: (_, value, _) {
              final ok = value.text.trim().length == 6;
              return FilledButton(
                onPressed: ok ? () => Navigator.pop(ctx, value.text.trim()) : null,
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
    final human = (e.detail == null || e.detail!.isEmpty) ? e.message : '${e.message}（${e.detail}）';
    switch (e.statusCode) {
      case 400:
        return '配对没通过：$human '
            '6 位码要按桌面上显示的那组原样敲；试错 5 次这张码就作废了。';
      case 410:
        return '这次配对会话已经失效：$human 请点「配对新电脑」重新举手（桌面会给出新的一组码）。';
      case 429:
        return '请求太频繁了：$human 等一会儿再点「配对新电脑」。';
      case 403:
        return '桌面拒绝了这次配对：$human 确认桌面上的「允许移动端连接」已经打开。';
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
      if (mounted) setState(() => _error = '忘记失败：$e');
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
      setState(() => _message = url.isEmpty ? '已清空桌面地址（桌面端 = 自动发现本机后端）。' : '桌面地址已保存：$url');
    } catch (e) {
      if (mounted) setState(() => _error = '保存地址失败：$e');
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
      if (mounted) setState(() => _error = '从桌面同步密钥失败：$e');
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
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
            FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('确认')),
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

  /// 这个待配对请求还剩几秒（拿不到时间戳返回 null）。
  int? _remainingSeconds(PendingPairing p) {
    final expire = _parseTime(p.codeExpiresAt) ?? _parseTime(p.expiresAt);
    if (expire == null) return null;
    return expire.difference(DateTime.now()).inSeconds;
  }

  /// 结果 / 错误提示（中文、如实）。
  Widget _feedback(ThemeData theme) {
    final rows = <Widget>[];
    if (_error != null) {
      rows.add(Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.error_outline, size: 18, color: theme.colorScheme.error),
          const SizedBox(width: 8),
          Expanded(child: SelectableText(_error!, style: theme.textTheme.bodySmall)),
        ],
      ));
    }
    if (_message != null) {
      rows.add(Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.check_circle_outline, size: 18, color: Colors.green),
          const SizedBox(width: 8),
          Expanded(child: SelectableText(_message!, style: theme.textTheme.bodySmall)),
        ],
      ));
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
                '打开后，手机要带上配对令牌才能连这台电脑的 AI。'
                '配对时码由这台电脑算出、只显示在下面 —— 手机拿不到算码的凭据，必须有人看着屏幕念一遍。',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
            if (_loading)
              const Padding(
                padding: EdgeInsets.fromLTRB(16, 4, 16, 12),
                child: LinearProgressIndicator(minHeight: 2),
              ),
            _switchRow(
              title: '允许移动端连接',
              subtitle: _enabled
                  ? '已开启：手机带配对令牌才能连（没带会被 403 挡下）。'
                  : '未开启：局域网内任何设备都能连（今天的行为）。',
              value: _enabled,
              onChanged: (_busy || _loading) ? null : _setEnabled,
            ),
            const Divider(height: 1),
            // ① 配对新手机（展开时才轮询）
            ListTile(
              contentPadding: const EdgeInsets.symmetric(horizontal: 8),
              leading: const Icon(Icons.qr_code_2_outlined),
              title: const Text('配对新手机'),
              subtitle: Text(_pending.isEmpty
                  ? '还没有待配对的请求'
                  : '有 ${_pending.length} 个待配对请求，下面这组码念给手机'),
              trailing: Icon(_pendingExpanded ? Icons.expand_less : Icons.expand_more),
              onTap: _togglePending,
            ),
            if (_pendingExpanded) _pendingArea(theme),
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
                            style: theme.textTheme.bodyMedium?.copyWith(fontFamily: 'monospace'),
                          ),
                        const SizedBox(height: 2),
                        Text(
                          '手机与这台电脑要在同一个局域网里；地址里的端口与桌面后端一致。',
                          style: theme.textTheme.labelSmall
                              ?.copyWith(color: theme.colorScheme.outline),
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
                  Expanded(child: Text('已配对设备', style: theme.textTheme.titleSmall)),
                  if (_hasToken)
                    Text(
                      '全局令牌已生成',
                      style: theme.textTheme.labelSmall
                          ?.copyWith(color: theme.colorScheme.outline),
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

  Widget _pendingArea(ThemeData theme) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (_pendingError != null)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(Icons.error_outline, size: 18, color: theme.colorScheme.error),
                const SizedBox(width: 8),
                Expanded(child: Text(_pendingError!, style: theme.textTheme.bodySmall)),
                TextButton(
                  onPressed: () {
                    setState(() => _pendingError = null);
                    _syncPolling();
                    unawaited(_pollPending());
                  },
                  child: const Text('重试'),
                ),
              ],
            )
          else if (_pending.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Text(
                '在手机上点「配对新电脑」，这里会出现一组 6 位码。',
                style: theme.textTheme.bodySmall,
              ),
            )
          else
            for (final p in _pending) _pendingRow(theme, p),
        ],
      ),
    );
  }

  Widget _pendingRow(ThemeData theme, PendingPairing p) {
    final remain = _remainingSeconds(p);
    final ip = p.ip ?? '';
    final name = (p.deviceName == null || p.deviceName!.isEmpty) ? '未知设备' : p.deviceName!;
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
              // 6 位码是这一块**唯一要给人看的东西**：等宽大字 + 可选中（方便核对）
              SelectableText(
                p.code.isEmpty ? '······' : p.code,
                style: theme.textTheme.headlineSmall?.copyWith(
                  fontFamily: 'monospace',
                  fontWeight: FontWeight.w700,
                  letterSpacing: 6,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                '$name${ip.isEmpty ? '' : ' · 来自 $ip'}',
                style: theme.textTheme.bodySmall,
              ),
              Text(
                remain == null
                    ? '在手机上敲这 6 位码'
                    : (remain > 0 ? '剩余 $remain 秒' : '这组码已过期，等桌面刷新出下一组'),
                style: theme.textTheme.labelSmall?.copyWith(
                  color: (remain != null && remain <= 0)
                      ? theme.colorScheme.error
                      : theme.colorScheme.outline,
                ),
              ),
            ],
          ),
        ),
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
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: _switchRow(
              title: '允许把密钥发给这台设备',
              subtitle: d.allowCredentials
                  ? '已允许：这台手机可以「从桌面同步密钥」把 API Key 取到本机密钥库。'
                  : '未允许：**手机拿不到桌面上的 API Key**（配对成功不等于能取密钥）。',
              value: d.allowCredentials,
              onChanged: _busy ? null : (v) => _setAllowCredentials(d, v),
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
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
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
                        : '还没配对：配一次对之后，本机才能连桌面取规则工件与密钥。',
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
                  '「从桌面同步密钥」只对**桌面已允许**「把密钥发给这台设备」的 Provider 有效；'
                  '没允许的那几把不会下发。',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
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
