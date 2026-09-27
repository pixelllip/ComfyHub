import 'dart:io' show Platform;

import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/pairing_token.dart';
import '../core/run_event_source.dart';
import '../core/settings_store.dart';
import '../core/sync_endpoints.dart';

/// 「同步与连接」：选一台桌面、试通它、看同步是不是真的在跑。
///
/// 为什么值得单独一页（而不是继续用「后端服务」那张卡的一个文本框）：
/// 跨端用户实际有**三条路**要来回切 —— 模拟器/同机（回环）、同一 Wi-Fi（局域网）、
/// 异地（Tailscale）。文本框只能"手抄 IP"，而抄错一个数字的表现是"连不上"，
/// 用户根本分不清是地址错了、没配对、还是对方没开。
/// 所以这一页把三件事摆在一起：
///  ① **候选地址**（自动发现 / 最近用过 / 桌面自己报告的）—— 点一下就切过去；
///  ② **测试连接**的真实结果（打一次 `/api/health`，连不上就如实说为什么）；
///  ③ **同步状态**（本机还有几条没上行、点一下立即上行、结果是几成几败）。
///
/// ⚠ 这一页**不发明任何判据**：能不能用 = 请求回没回 200；
/// 候选从哪来 = 每条都带 `source` 标签；拿不到就显示"拿不到"，不猜。
class SyncSettingsPage extends StatefulWidget {
  const SyncSettingsPage({
    super.key,
    this.client,
    this.tokenProvider,
    this.pendingProvider,
    this.syncRunner,
  });

  /// 测试注入（默认每次现建一个 http.Client）。
  final http.Client? client;

  /// 测试注入（默认读进程内那份配对令牌缓存 `PairingTokenCache.read`）。
  final String? Function()? tokenProvider;

  /// 测试注入：本机待上行条数。
  final Future<int> Function()? pendingProvider;

  /// 测试注入：立即上行。
  final Future<({bool ok, int applied, int pending, String? error})> Function(
    String? baseUrl,
    String? token,
  )? syncRunner;

  @override
  State<SyncSettingsPage> createState() => _SyncSettingsPageState();
}

class _SyncSettingsPageState extends State<SyncSettingsPage> {
  late final TextEditingController _controller;
  late final SettingsStore _settings;

  List<SyncEndpoint> _remote = const [];
  String? _discoverNote;

  SyncProbe? _probe;
  bool _probing = false;

  int? _pending;
  String? _pendingNote;
  bool _pendingLoading = false;

  bool _syncing = false;
  String? _syncNote;

  bool get _isAndroid {
    try {
      return Platform.isAndroid;
    } catch (_) {
      return false;
    }
  }

  String? _token() => (widget.tokenProvider ?? PairingTokenCache.read)();

  @override
  void initState() {
    super.initState();
    _settings = context.read<SettingsStore>();
    _controller = TextEditingController(text: _settings.baseUrl);
    // 首屏先把"本机一侧"的候选摆出来（不发任何请求），
    // 再去问对端要它的地址列表、问本机队列有几条 —— 两步都是 best-effort。
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _loadRemoteCandidates();
      _loadPending();
    });
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _loadRemoteCandidates() async {
    final r = await discoveredEndpoints(
      _settings.baseUrl,
      client: widget.client,
      token: _token(),
    );
    if (!mounted) return;
    setState(() {
      _remote = r.endpoints;
      _discoverNote = r.note;
    });
  }

  Future<void> _loadPending() async {
    setState(() {
      _pendingLoading = true;
      _pendingNote = null;
    });
    try {
      final n = await (widget.pendingProvider ?? _defaultPending)();
      if (!mounted) return;
      setState(() {
        _pending = n;
        _pendingLoading = false;
      });
    } catch (e) {
      if (!mounted) return;
      // 队列读不到就如实说 —— 显示一个假的 0 会让用户以为"都推上去了"
      setState(() {
        _pending = null;
        _pendingNote = '读本机上行列队失败：$e';
        _pendingLoading = false;
      });
    }
  }

  Future<int> _defaultPending() => _openSource().pendingOutbox();

  RunEventSource _openSource() => openRunEventSource(
        api: AiApiClient(
          _settings.baseUrl,
          tokenProvider: widget.tokenProvider ?? PairingTokenCache.read,
        ),
      );

  Future<void> _apply(String url) async {
    await _settings.setBaseUrl(url);
    if (!mounted) return;
    setState(() {
      _controller.text = _settings.baseUrl;
      _probe = null;
    });
    await _probeNow();
  }

  Future<void> _saveTyped() async {
    await _settings.setBaseUrl(_controller.text);
    if (!mounted) return;
    setState(() {
      _controller.text = _settings.baseUrl;
      _probe = null;
    });
    await _probeNow();
  }

  Future<void> _restoreAuto() async {
    await _settings.setBaseUrl('');
    if (!mounted) return;
    setState(() {
      _controller.text = _settings.baseUrl;
      _probe = null;
    });
    await _probeNow();
  }

  Future<void> _probeNow() async {
    setState(() => _probing = true);
    final r = await probeSyncEndpoint(
      _settings.baseUrl,
      client: widget.client,
      token: _token(),
    );
    if (!mounted) return;
    setState(() {
      _probe = r;
      _probing = false;
    });
    await _loadRemoteCandidates();
  }

  Future<void> _syncNow() async {
    setState(() {
      _syncing = true;
      _syncNote = null;
    });
    try {
      final run = widget.syncRunner ?? _defaultSync;
      final r = await run(_settings.baseUrl, _token());
      if (!mounted) return;
      setState(() {
        _syncing = false;
        _pending = r.pending;
        _syncNote = r.error != null
            ? '没推成：${r.error}（还剩 ${r.pending} 条）'
            : '这次推上去 ${r.applied} 条，还剩 ${r.pending} 条';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _syncing = false;
        _syncNote = '上行出错：$e';
      });
      await _loadPending();
    }
  }

  Future<({bool ok, int applied, int pending, String? error})> _defaultSync(
    String? baseUrl,
    String? token,
  ) =>
      _openSource().syncNow(baseUrl: baseUrl, token: token);

  List<SyncEndpoint> get _candidates => localEndpointCandidates(
        autoUrl: SettingsStore.defaultBaseUrl(),
        current: _settings.baseUrl,
        history: _settings.baseUrlHistory,
      );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('同步与连接')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          _currentCard(theme),
          const SizedBox(height: 12),
          _candidatesCard(theme),
          const SizedBox(height: 12),
          _syncCard(theme),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------- 当前连接

  Widget _currentCard(ThemeData theme) {
    final probe = _probe;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('当前连接', style: theme.textTheme.titleSmall),
            const SizedBox(height: 6),
            SelectableText(
              _settings.baseUrl.isEmpty ? '（未设置）' : _settings.baseUrl,
              style: theme.textTheme.bodyMedium,
            ),
            const SizedBox(height: 2),
            Text(
              _settings.baseUrlIsAuto
                  ? '自动：桌面端按 .run\\api.json 里后端记录的真实端口（重启会换端口）'
                  : '手填/选过的地址（不会自动改）',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 10,
              runSpacing: 4,
              children: [
                FilledButton.icon(
                  onPressed: _probing ? null : _probeNow,
                  icon: _probing
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.wifi_tethering),
                  label: const Text('测试连接'),
                ),
                TextButton(
                  onPressed: _probing ? null : _loadRemoteCandidates,
                  child: const Text('重新问对端要地址'),
                ),
              ],
            ),
            if (probe != null) ...[
              const SizedBox(height: 12),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    probe.ok ? Icons.check_circle_outline : Icons.error_outline,
                    size: 18,
                    color: probe.ok ? Colors.green : theme.colorScheme.error,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SelectableText(probe.summary, style: theme.textTheme.bodySmall),
                        if (probe.note != null)
                          Text(
                            probe.note!,
                            style: theme.textTheme.labelSmall
                                ?.copyWith(color: theme.colorScheme.outline),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }

  // ---------------------------------------------------------------- 选地址

  Widget _candidatesCard(ThemeData theme) {
    final candidates = [..._candidates, ..._remote];
    final seen = <String>{};
    final unique = [
      for (final c in candidates)
        if (seen.add(c.url)) c,
    ];
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('选择地址', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              '点一下就切过去并发一次测试请求。每条都标了它的来路 —— '
              '（回环 = 同一台机器 / 模拟器；局域网 = 同一个 Wi-Fi；Tailscale = 异地组网）。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 10),
            if (unique.isEmpty)
              Text('还没有候选地址，手工填一个吧。', style: theme.textTheme.bodySmall)
            else
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final c in unique)
                    ChoiceChip(
                      selected: c.url == _settings.baseUrl,
                      onSelected: (_) => _apply(c.url),
                      avatar: Icon(_iconFor(c.kind), size: 16),
                      label: Text('${c.kind.label} · ${c.url}'),
                      tooltip: '${c.kind.hint}\n来源：${c.source}',
                    ),
                ],
              ),
            if (_discoverNote != null) ...[
              const SizedBox(height: 8),
              Text(
                _discoverNote!,
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ],
            const SizedBox(height: 14),
            TextField(
              controller: _controller,
              decoration: const InputDecoration(
                labelText: '手工地址',
                hintText: '例如 http://192.168.1.9:8080 或 http://100.64.0.5:8080',
              ),
              onSubmitted: (_) => _saveTyped(),
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 10,
              runSpacing: 4,
              children: [
                FilledButton.icon(
                  onPressed: _probing ? null : _saveTyped,
                  icon: const Icon(Icons.save_outlined),
                  label: const Text('保存并使用'),
                ),
                TextButton(
                  onPressed: _probing ? null : _restoreAuto,
                  child: const Text('恢复自动'),
                ),
              ],
            ),
            if (_settings.baseUrlHistory.isNotEmpty) ...[
              const Divider(height: 24),
              Text('最近用过', style: theme.textTheme.labelLarge),
              const SizedBox(height: 4),
              for (final h in _settings.baseUrlHistory)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(_iconFor(classifyEndpoint(h)), size: 18),
                  title: Text(h, style: theme.textTheme.bodySmall),
                  trailing: IconButton(
                    tooltip: '忘记这个地址',
                    icon: const Icon(Icons.close, size: 16),
                    onPressed: () async {
                      await _settings.forgetBaseUrl(h);
                      if (mounted) setState(() {});
                    },
                  ),
                  onTap: () => _apply(h),
                ),
            ],
          ],
        ),
      ),
    );
  }

  IconData _iconFor(SyncEndpointKind kind) => switch (kind) {
        SyncEndpointKind.loopback => Icons.computer,
        SyncEndpointKind.lan => Icons.router,
        SyncEndpointKind.tailscale => Icons.public,
        SyncEndpointKind.other => Icons.link,
      };

  // ---------------------------------------------------------------- 同步

  Widget _syncCard(ThemeData theme) {
    final pending = _pending;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('同步', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              _isAndroid
                  ? '手机上产生的会话/消息/块/这次 Run 的工具过程都先落在本机库，'
                      '再由这里推给桌面（桌面是唯一真源）。没推上去的东西一直都在本机，不会丢。'
                  : '上行队列在**手机**那一侧：桌面不排队（这里的 0 是真实读数，不是没实现）。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 10),
            if (_pendingLoading)
              const LinearProgressIndicator(minHeight: 2)
            else
              Text(
                pending == null ? '待上行：读不到（见下面的原因）' : '待上行：$pending 条',
                style: theme.textTheme.bodyMedium,
              ),
            if (_pendingNote != null) ...[
              const SizedBox(height: 4),
              Text(
                _pendingNote!,
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error),
              ),
            ],
            const SizedBox(height: 10),
            Wrap(
              spacing: 10,
              runSpacing: 4,
              children: [
                FilledButton.icon(
                  onPressed: _syncing ? null : _syncNow,
                  icon: _syncing
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.cloud_upload_outlined),
                  label: const Text('立即同步'),
                ),
                TextButton(
                  onPressed: _pendingLoading ? null : _loadPending,
                  child: const Text('刷新队列读数'),
                ),
              ],
            ),
            if (_syncNote != null) ...[
              const SizedBox(height: 8),
              SelectableText(_syncNote!, style: theme.textTheme.bodySmall),
            ],
          ],
        ),
      ),
    );
  }
}
