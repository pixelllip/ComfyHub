import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/settings_store.dart';
import '../core/theme.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/backup_card.dart';
import '../widgets/outbound_card.dart';
import '../widgets/settings_scaffold.dart';
import '../widgets/storage_card.dart';
import '../widgets/settings_switch_row.dart';

/// 「本地服务（后端 + 数据库）」子页。
///
/// 2026-11-05 从设置首页搬出来：这一块原来一张卡里塞了 3 个开关 + 状态 + 4 个
/// 服务动作 + 运行根目录 + 后端地址，属于"超过两项就该收进子界面"的典型。
/// 现在它同时收编了原来独立的「后端服务」卡（地址 / 测试连接 / 存储目录）——
/// 这两件事本来就是同一个进程的两面。
class LocalServicesSettingsPage extends StatefulWidget {
  /// 设置搜索跳转到的区块（例如 `status` / `url`）。null = 从页首开始。
  final String? initialSectionId;

  const LocalServicesSettingsPage({super.key, this.initialSectionId});

  @override
  State<LocalServicesSettingsPage> createState() =>
      _LocalServicesSettingsPageState();
}

class _LocalServicesSettingsPageState extends State<LocalServicesSettingsPage> {
  late final TextEditingController _urlController;
  late final TextEditingController _projectRootController;

  /// 地址框的焦点：**只有它没被编辑时才跟着真实地址同步**。
  final FocusNode _urlFocus = FocusNode();

  late SettingsStore _settings;

  bool _testing = false;
  String? _testResult;
  bool _testOk = false;
  Map<String, dynamic>? _health;

  /// 出网代理（`GET /api/system/outbound`）—— 打开这一页就读一次，不往外发流量。
  OutboundInfo? _outbound;
  bool _outboundLoading = false;
  bool _outboundProbing = false;
  String? _outboundError;

  @override
  void initState() {
    super.initState();
    final settings = context.read<SettingsStore>();
    _settings = settings;
    _urlController = TextEditingController(text: settings.baseUrl);
    // 后端重启会更换端口（自动分配），地址变化时输入框需同步 —— 否则界面显示
    // 的是上一轮的**失效地址**，据此排查会得到错误结论。
    settings.addListener(_syncUrlField);
    _projectRootController = TextEditingController(
      text: settings.projectRoot ?? BackendLauncher.detectProjectRoot() ?? '',
    );
    // 进页就读一次：`probe` 为假，后端只读配置、不发请求。
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadOutbound());
  }

  @override
  void dispose() {
    _settings.removeListener(_syncUrlField);
    _urlFocus.dispose();
    _urlController.dispose();
    _projectRootController.dispose();
    super.dispose();
  }

  /// 地址框跟随**当前真实地址**；仅在没有编辑该输入框时同步。
  void _syncUrlField() {
    if (!mounted || _urlFocus.hasFocus) return;
    final v = _settings.baseUrl;
    if (_urlController.text != v) _urlController.text = v;
  }

  Future<void> _test() async {
    final settings = context.read<SettingsStore>();
    final store = context.read<LibraryStore>();
    setState(() {
      _testing = true;
      _testResult = null;
    });
    // 判据是"**本机地址始终自动**"：填写回环地址不会成为"手动覆盖"，
    // 真源只有 `.run\api.json`。保存后将输入框同步为真实地址并提示。
    final wasLocalTyped = SettingsStore.isLocalBackendUrl(_urlController.text);
    await settings.setBaseUrl(_urlController.text);
    if (!mounted) return;
    _urlController.text = settings.baseUrl;
    final localNote = (wasLocalTyped && _urlController.text.isNotEmpty)
        ? '（本机地址永远自动，端口由后端启动时挑选，以 .run\\api.json 为准）\n'
        : '';
    try {
      final h = await store.api.health();
      if (!mounted) return;
      setState(() {
        _health = h;
        _testOk = h['status'] == 'ok';
        _testResult = _testOk
            ? '$localNote连接成功（版本 ${h['version']}，数据库 ${h['database']}）'
            : '$localNote服务可访问，但状态是 ${h['status']}';
      });
      await store.refreshAll();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _testOk = false;
        _health = null;
        _testResult = '$localNote连接失败，$e';
      });
    } finally {
      if (mounted) setState(() => _testing = false);
    }
  }

  /// 读一次出网代理设置。[probe] 为真时后端向 Civitai / Danbooru 各发送一次请求
  /// （仅在点「测试出网」时执行：打开页面不应产生外网流量）。
  Future<void> _loadOutbound({bool probe = false}) async {
    final api = context.read<LibraryStore>().api;
    setState(() {
      if (probe) {
        _outboundProbing = true;
      } else {
        _outboundLoading = true;
      }
      _outboundError = null;
    });
    try {
      final info = await api.outbound(probe: probe);
      if (!mounted) return;
      setState(() => _outbound = info);
    } catch (e) {
      if (!mounted) return;
      setState(() => _outboundError = '$e');
    } finally {
      if (mounted) {
        setState(() {
          _outboundLoading = false;
          _outboundProbing = false;
        });
      }
    }
  }

  /// 跑一个服务动作（重启 / 停止），**完事重新发现后端地址**：端口是自动挑的，
  /// 不重新读 `.run\api.json` 就会一直对着旧端口说话。
  Future<void> _runActionThenRediscover(String action) async {
    final launcher = context.read<BackendLauncher>();
    final settings = context.read<SettingsStore>();
    await launcher.runAction(action);
    if (!mounted) return;
    try {
      await settings.refreshBaseUrlFromPortFile();
      if (!mounted) return;
      await context.read<LibraryStore>().refreshAll();
    } catch (_) {
      // 重新发现失败不该把重启按钮变成报错
    }
  }

  Future<void> _startLocalServices(
    BackendLauncher launcher, {
    bool force = true,
  }) async {
    await launcher.ensureRunning(force: force);
    if (!mounted) return;
    if (launcher.isReady) {
      await context.read<LibraryStore>().refreshAll();
    }
  }

  void _showLogs(BackendLauncher launcher) {
    showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('本地服务日志'),
        content: SizedBox(
          width: 720,
          height: 420,
          child: AnimatedBuilder(
            animation: launcher,
            builder: (_, _) {
              final lines = launcher.logLines;
              if (lines.isEmpty) {
                return const Center(child: Text('还没有输出'));
              }
              return ListView.builder(
                reverse: true,
                itemCount: lines.length,
                itemBuilder: (_, i) => Text(
                  lines[lines.length - 1 - i],
                  style: Theme.of(ctx).textTheme.labelSmall
                      ?.copyWith(fontFamily: 'monospace'),
                ),
              );
            },
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('关闭'),
          ),
        ],
      ),
    );
  }

  Future<void> _openFolder(String path) async {
    if (path.isEmpty) return;
    try {
      await launchUrl(Uri.directory(path));
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('打开失败，$e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = AppColors.of(context);
    final settings = context.watch<SettingsStore>();
    final launcher = context.watch<BackendLauncher>();

    final busy = launcher.busy;
    final statusColor = launcher.isReady
        ? colors.success
        : (launcher.phase == StartupPhase.failed
              ? theme.colorScheme.error
              : theme.colorScheme.primary);
    final tail = launcher.logLines.length > 4
        ? launcher.logLines.sublist(launcher.logLines.length - 4)
        : launcher.logLines;

    return SettingsScaffold(
      title: '本地服务',
      initialSectionId: widget.initialSectionId,
      sections: [
        SettingsSection(
          title: '开关',
          id: 'switches',
          estimatedHeight: 340,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SettingsSwitchRow(
                    value: settings.autoStartBackend,
                    onChanged: settings.setAutoStartBackend,
                    title: '启动 App 时自动拉起服务',
                    subtitle:
                        '启动前先确认数据库可用。数据库现在是本地 SQLite 文件，'
                        '后端会自己建，所以这一步通常只是一次健康检查。',
                  ),
                  SettingsSwitchRow(
                    value: settings.stopServicesOnExit,
                    onChanged: (value) async {
                      await settings.setStopServicesOnExit(value);
                      // 关掉时要**真的撤销认领**，否则退出收尾照样会把它停掉
                      if (!value) await launcher.disarm();
                    },
                    title: '关闭 App 时一并停止本地服务',
                    subtitle:
                        '开着时，关掉 App 会停掉本机的后端，包括启动 App 之前就在跑的那个。\n'
                        '关掉时服务常驻，下次开 App 就是热启动。',
                  ),
                  SettingsSwitchRow(
                    value: settings.allowRemoteBackend,
                    onChanged: (value) async {
                      final messenger = ScaffoldMessenger.of(context);
                      await settings.setAllowRemoteBackend(value);
                      if (!mounted) return;
                      messenger.showSnackBar(
                        SnackBar(
                          content: Text(
                            value
                                ? '已允许局域网访问，重启后端（下面的「重启」按钮）后生效。'
                                : '已恢复仅本机访问，重启后端后生效。',
                          ),
                        ),
                      );
                    },
                    title: '允许手机 / 局域网设备连接',
                    // ⚠ 这一项**不是**「远程访问与配对」里的配对闸门：
                    //   · 这里 = 要不要让后端监听局域网（起进程时定 → 必须重启后端）；
                    //   · 那里 = 局域网请求要不要带令牌（API 即刻生效）。
                    subtitle:
                        '① 让后端监听局域网（改完要点下面的「重启」才生效）。\n'
                        '手机最终能不能连，还要看「远程访问与配对」里的 ② 配对闸门。',
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '服务状态',
          id: 'status',
          estimatedHeight: 360,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        launcher.isReady
                            ? Icons.check_circle_outline
                            : Icons.info_outline,
                        size: 18,
                        color: statusColor,
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          launcher.message,
                          style: theme.textTheme.bodySmall,
                        ),
                      ),
                    ],
                  ),
                  // 「端口未能恢复为上次使用的值」必须**可见**：手机保存的地址随即失效，
                  // 不显示时只会遇到"手机突然连不上"，无法判断需要重新扫码。
                  if (launcher.portFallbackFrom != null) ...[
                    const SizedBox(height: 8),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          Icons.warning_amber_rounded,
                          size: 18,
                          color: theme.colorScheme.error,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            '端口 ${launcher.portFallbackFrom} 这次没能用上（被别的程序占了），'
                            '后端改听了另一个端口，手机上保存的地址已经失效。'
                            '请让手机重新扫一次「远程访问与配对」里的二维码。',
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: theme.colorScheme.error,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ],
                  if (tail.isNotEmpty &&
                      (busy || launcher.phase == StartupPhase.failed)) ...[
                    const SizedBox(height: 8),
                    Container(
                      width: double.infinity,
                      padding: const EdgeInsets.all(8),
                      decoration: BoxDecoration(
                        color: theme.colorScheme.surfaceContainer,
                        borderRadius: BorderRadius.circular(8),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          for (final line in tail)
                            Text(
                              line,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: theme.textTheme.labelSmall?.copyWith(
                                fontFamily: 'monospace',
                              ),
                            ),
                        ],
                      ),
                    ),
                  ],
                  const SizedBox(height: 8),
                  const Divider(height: 24),
                  Wrap(
                    spacing: 10,
                    runSpacing: 8,
                    children: [
                      FilledButton.icon(
                        onPressed: busy
                            ? null
                            : () => _startLocalServices(launcher),
                        icon: const Icon(Icons.play_arrow),
                        label: const Text('启动 / 修复服务'),
                      ),
                      OutlinedButton.icon(
                        onPressed: busy
                            ? null
                            : () => _runActionThenRediscover('restart'),
                        icon: const Icon(Icons.restart_alt),
                        label: const Text('重启'),
                      ),
                      OutlinedButton.icon(
                        onPressed: busy
                            ? null
                            : () => launcher.runAction('down'),
                        icon: const Icon(Icons.stop_circle_outlined),
                        label: const Text('停止'),
                      ),
                      TextButton.icon(
                        onPressed: () => _showLogs(launcher),
                        icon: const Icon(Icons.article_outlined),
                        label: const Text('查看日志'),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        // 出网走不走代理（2026-11-09）：跟「后端地址」一样属于"这一台机器怎么联网"，
        // 但比地址更容易被误会 —— danbooru 回 503 那次，谁都以为是没吃代理。
        SettingsSection(
          title: '联网代理',
          id: 'proxy',
          estimatedHeight: 320,
          child: OutboundCard(
            info: _outbound,
            loading: _outboundLoading,
            probing: _outboundProbing,
            error: _outboundError,
            onRefresh: () => _loadOutbound(),
            onProbe: () => _loadOutbound(probe: true),
          ),
        ),
        // 备份与恢复：位于「服务状态」之后 —— 恢复后需要重启服务，
        // 两件事在同一屏。
        const SettingsSection(
          title: '备份与恢复',
          id: 'backup',
          estimatedHeight: 460,
          child: BackupCard(),
        ),
        // 存储占用 / 孤儿 / 事件归档：与备份同屏，便于排查磁盘占用。
        const SettingsSection(
          title: '存储',
          id: 'storage',
          estimatedHeight: 420,
          child: StorageCard(),
        ),
        SettingsSection(
          title: '运行根目录',
          id: 'root',
          estimatedHeight: 190,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    settings.projectRoot ?? '未定位到项目目录',
                    style: theme.textTheme.bodyMedium,
                  ),
                  const SizedBox(height: 6),
                  // 按钮组用 `Wrap`：窄窗口下两枚按钮的自然宽度加起来比卡片还宽。
                  Wrap(
                    spacing: 8,
                    runSpacing: 4,
                    children: [
                      TextButton.icon(
                        onPressed: () {
                          _projectRootController.text =
                              BackendLauncher.detectProjectRoot() ?? '';
                        },
                        icon: const Icon(Icons.search, size: 18),
                        label: const Text('自动探测'),
                      ),
                      TextButton.icon(
                        onPressed: () async {
                          final messenger = ScaffoldMessenger.of(context);
                          await settings.setProjectRoot(
                            _projectRootController.text,
                          );
                          if (!mounted) return;
                          messenger.showSnackBar(
                            const SnackBar(content: Text('运行根目录已保存')),
                          );
                        },
                        icon: const Icon(Icons.save_outlined, size: 18),
                        label: const Text('保存'),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '后端地址',
          id: 'url',
          estimatedHeight: 380,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  TextField(
                    controller: _urlController,
                    focusNode: _urlFocus,
                    decoration: const InputDecoration(
                      labelText: '后端地址',
                      hintText: '留空就自动使用 .run\\api.json 里记录的端口',
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'Windows 桌面端用 127.0.0.1，Android 模拟器用 10.0.2.2，真机用电脑的局域网 IP。\n'
                    '本机地址（127.0.0.1 / localhost）永远自动，端口由后端启动时挑选，重启会换，'
                    'App 每次都读它写在 .run\\api.json 里的真实端口，这里填什么都不会保存。\n'
                    '只有要连别的机器时手填才有意义，填了就不会被自动改。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 12),
                  // 窄窗口下这两枚按钮加起来比卡片宽（320px 实测溢出 7.1px），用 Wrap。
                  Wrap(
                    spacing: 10,
                    runSpacing: 4,
                    children: [
                      FilledButton.icon(
                        onPressed: _testing ? null : _test,
                        icon: _testing
                            ? const SizedBox(
                                width: 16,
                                height: 16,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                ),
                              )
                            : const Icon(Icons.wifi_tethering),
                        label: const Text('保存并测试连接'),
                      ),
                      TextButton(
                        onPressed: () {
                          _urlController.text = SettingsStore.defaultBaseUrl();
                        },
                        child: const Text('恢复默认'),
                      ),
                    ],
                  ),
                  if (_testResult != null) ...[
                    const SizedBox(height: 12),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          _testOk
                              ? Icons.check_circle_outline
                              : Icons.error_outline,
                          size: 18,
                          color: _testOk
                              ? colors.success
                              : theme.colorScheme.error,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: SelectableText(
                            _testResult!,
                            style: theme.textTheme.bodySmall,
                          ),
                        ),
                      ],
                    ),
                  ],
                  if (_health != null) ...[
                    const SizedBox(height: 8),
                    Row(
                      children: [
                        Expanded(
                          child: SelectableText(
                            '存储目录是 ${_health!['storageDir']}',
                            style: theme.textTheme.labelSmall?.copyWith(
                              color: theme.colorScheme.onSurfaceVariant,
                            ),
                          ),
                        ),
                        TextButton(
                          onPressed: () =>
                              _openFolder('${_health!['storageDir']}'),
                          child: const Text('打开'),
                        ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}
