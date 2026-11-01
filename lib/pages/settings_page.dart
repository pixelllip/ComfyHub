import 'dart:io' show Platform;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/formatting.dart';
import '../core/media_cache.dart';
import '../core/settings_store.dart';
import '../core/system_accent.dart';
import '../core/theme.dart';
import '../models/models.dart';
import '../state/ai_workspace_store.dart';
import '../state/library_store.dart';
import '../state/nsfw_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/export_dialog.dart';
import '../widgets/media_cache_card.dart';
import 'ai_provider_settings_page.dart';
import 'pairing_section.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late final TextEditingController _urlController;
  late final TextEditingController _projectRootController;
  late final TextEditingController _comfyUrlController;
  late final TextEditingController _comfyOutputController;
  late final TextEditingController _autoTagController;

  /// 地址框的焦点：**只有它没被编辑时才跟着真实地址同步**（见 [_syncUrlField]）。
  final FocusNode _urlFocus = FocusNode();

  late SettingsStore _settings;

  bool _testing = false;
  String? _testResult;
  bool _testOk = false;
  Map<String, dynamic>? _health;

  // --- ComfyUI 自动捕获 ---
  CaptureConfig? _capture;
  CaptureStatus? _captureStatus;
  bool _loadingCapture = false;
  String? _captureError;
  String? _captureMessage;
  bool _captureBusy = false;
  bool _captureEnabled = true;
  int _pollSeconds = 4;

  /// NSFW 两个开关正在写库（避免连点把两次写入交叉）。
  bool _nsfwBusy = false;

  /// 产物缓存占用（手机端；null = 还没统计完）。
  MediaCacheStats? _cacheStats;
  bool _cacheBusy = false;

  /// 「自动查找 ComfyUI」的探测结果（用户"其他建议"第 3 条）；null = 还没查过。
  ComfyLocation? _comfyLocation;

  @override
  void initState() {
    super.initState();
    final settings = context.read<SettingsStore>();
    _settings = settings;
    _urlController = TextEditingController(text: settings.baseUrl);
    // 后端重启会换端口（自动挑的），地址一变这框就得跟着走 —— 否则界面上摆的是一个
    // **上一轮的**死地址，用户看着它去排查，只会越查越糊涂（2026-10-12 那个 bug 里
    // 存的正是 2175，而后端跑在 9610）。
    settings.addListener(_syncUrlField);
    // 没手动设过就直接把自动探测到的路径填进去，省得用户对着空框猜
    _projectRootController = TextEditingController(
      text: settings.projectRoot ?? BackendLauncher.detectProjectRoot() ?? '',
    );
    _comfyUrlController = TextEditingController(text: 'http://127.0.0.1:8188');
    _comfyOutputController = TextEditingController();
    _autoTagController = TextEditingController(text: 'ComfyUI');
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _loadCapture();
      _loadCacheStats();
    });
  }

  /// 读一次产物缓存占用（手机端设置页那一节；桌面端 `stats()` 直接回 0，不碰磁盘）。
  Future<void> _loadCacheStats() async {
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() => _cacheStats = stats);
  }

  /// 地址框跟着**当前真实地址**走。
  ///
  /// 只在用户没在编辑这个框的时候同步 —— 否则会把他正打的字吃掉。用户手填的地址
  /// （别的机器）不会被自动改，所以对他没有副作用。
  void _syncUrlField() {
    if (!mounted || _urlFocus.hasFocus) return;
    final v = _settings.baseUrl;
    if (_urlController.text != v) _urlController.text = v;
  }

  @override
  void dispose() {
    _settings.removeListener(_syncUrlField);
    _urlFocus.dispose();
    _urlController.dispose();
    _projectRootController.dispose();
    _comfyUrlController.dispose();
    _comfyOutputController.dispose();
    _autoTagController.dispose();
    super.dispose();
  }

  // -------------------------------------------------------------------------
  //  后端地址
  // -------------------------------------------------------------------------

  /// 重新读一次平台强调色：用户在系统里换了颜色，不用重启 App 就能看到。
  Future<void> _redetectAccent() async {
    final messenger = ScaffoldMessenger.of(context);
    await _settings.refreshSystemAccent();
    if (!mounted) return;
    messenger.showSnackBar(SnackBar(content: Text(SystemAccent.sourceLabel)));
  }

  /// 跑一个服务动作（重启 / 停止），**完事重新发现后端地址**。
  ///
  /// 为什么需要（2026-09-22）：端口现在是自动挑的（`COMFYHUB_PORT` 默认 0），
  /// 重启会换一个端口。不重新读一次 `.run\api.json`，App 会继续对着旧端口说话 ——
  /// 界面上的表现就是"重启成功但什么都刷不出来"。
  /// 用户手填过地址时 [SettingsStore.refreshBaseUrlFromPortFile] 不会动它（那是远程配置）。
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
      // 重新发现失败不该把重启按钮变成报错：地址没变就还是原来的行为
    }
  }

  Future<void> _test() async {
    final settings = context.read<SettingsStore>();
    final store = context.read<LibraryStore>();
    setState(() {
      _testing = true;
      _testResult = null;
    });
    // ⚠ 判据是"**本机地址永远自动**"（`SettingsStore.isLocalBackendUrl`）：用户把
    // 回环地址填回来（很常见 —— 框里本来就是这个字符串）不会变成"手填的覆盖"，
    // 后端端口是自动挑的，真源只有 `.run\api.json`。所以这里存完要把框里的值
    // 拉回**真实地址**，并当面说一句，别让人以为"我填的没生效"。
    final wasLocalTyped = SettingsStore.isLocalBackendUrl(_urlController.text);
    await settings.setBaseUrl(_urlController.text);
    if (!mounted) return;
    _urlController.text = settings.baseUrl;
    final localNote = (wasLocalTyped && _urlController.text.isNotEmpty)
        ? '（本机地址永远自动：端口是后端启动时挑的，以 .run\\api.json 为准）\n'
        : '';
    try {
      final h = await store.api.health();
      if (!mounted) return;
      setState(() {
        _health = h;
        _testOk = h['status'] == 'ok';
        _testResult = _testOk
            ? '$localNote连接成功（版本 ${h['version']}，数据库 ${h['database']}）'
            : '$localNote服务可访问，但状态异常：${h['status']}';
      });
      await store.refreshAll();
      await _loadCapture();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _testOk = false;
        _health = null;
        _testResult = '$localNote连接失败：$e';
      });
    } finally {
      if (mounted) setState(() => _testing = false);
    }
  }

  // -------------------------------------------------------------------------
  //  自动捕获
  // -------------------------------------------------------------------------

  Future<void> _loadCapture() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _loadingCapture = true;
      _captureError = null;
    });
    try {
      final cfg = await store.api.captureConfig();
      final status = await store.api.captureStatus();
      if (!mounted) return;
      setState(() {
        _capture = cfg;
        _captureStatus = status;
        _captureEnabled = cfg.enabled;
        _pollSeconds = cfg.pollSeconds;
        _comfyUrlController.text = cfg.comfyUrl;
        _comfyOutputController.text = cfg.outputDir ?? '';
        _autoTagController.text = cfg.autoTag;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '$e');
    } finally {
      if (mounted) setState(() => _loadingCapture = false);
    }
  }

  /// 自动查找本机的 ComfyUI（用户"其他建议"第 3 条）。
  ///
  /// 探测本身是只读的：**不点「使用这个目录」就不会改任何设置**。
  Future<void> _locateComfy() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = null;
      _captureError = null;
    });
    try {
      final found = await store.api.locateComfy();
      if (!mounted) return;
      setState(() {
        _comfyLocation = found;
        _captureMessage = found.outputDir == null
            ? '没找到 ComfyUI 的安装位置。'
            : '找到了：${found.outputDir}';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '查找失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  /// 把探测到的输出目录写进配置（用户显式点的那一下）。
  Future<void> _applyComfyLocation(String outputDir) async {
    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = null;
      _captureError = null;
    });
    try {
      final saved = await store.api.applyComfyLocation(outputDir);
      if (!mounted) return;
      setState(() {
        _capture = saved;
        _comfyOutputController.text = saved.outputDir ?? '';
        _comfyLocation = null;
        _captureMessage = '已使用探测到的输出目录：${saved.outputDir}';
      });
      await _refreshCaptureStatus();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '应用失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  /// 探测结果面板：找到了就给一句"从哪找到的 + 一键使用"，没找到就给该怎么填。
  Widget _comfyLocationPanel(ThemeData theme) {
    final loc = _comfyLocation!;
    return Padding(
      padding: const EdgeInsets.only(top: 4, bottom: 4),
      child: Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (loc.outputDir != null) ...[
                Text('找到 ComfyUI：${loc.home ?? '-'}', style: theme.textTheme.bodySmall),
                const SizedBox(height: 2),
                Text(
                  '输出目录：${loc.outputDir}'
                  '${loc.source != null ? '（${loc.source}）' : ''}',
                  style: theme.textTheme.labelSmall,
                ),
                const SizedBox(height: 6),
                if (loc.canApply)
                  FilledButton.tonal(
                    onPressed: _captureBusy ? null : () => _applyComfyLocation(loc.outputDir!),
                    child: const Text('使用这个目录'),
                  )
                else
                  Text('已经在用这个目录了。', style: theme.textTheme.labelSmall),
              ] else
                Text(
                  loc.note ?? '没找到 ComfyUI。请手工填上面的输出目录。',
                  style: theme.textTheme.bodySmall,
                ),
            ],
          ),
        ),
      ),
    );
  }

  Future<void> _saveCapture() async {    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = null;
      _captureError = null;
    });
    try {
      final base = _capture ?? const CaptureConfig();
      final saved = await store.api.updateCaptureConfig(base.copyWith(
        enabled: _captureEnabled,
        comfyUrl: _comfyUrlController.text.trim(),
        outputDir: _comfyOutputController.text.trim(),
        clearOutputDir: _comfyOutputController.text.trim().isEmpty,
        pollSeconds: _pollSeconds,
        autoTag: _autoTagController.text.trim(),
      ));
      if (!mounted) return;
      setState(() {
        _capture = saved;
        _captureMessage = _captureEnabled ? '自动捕获已开启' : '自动捕获已关闭';
      });
      await _refreshCaptureStatus();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '保存失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  Future<void> _refreshCaptureStatus() async {
    final store = context.read<LibraryStore>();
    try {
      final status = await store.api.captureStatus();
      if (!mounted) return;
      setState(() => _captureStatus = status);
    } catch (_) {
      // 状态拉不到不影响设置本身
    }
  }

  Future<void> _syncNow() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = null;
      _captureError = null;
    });
    try {
      final result = await store.api.pollCapture();
      if (!mounted) return;
      setState(() => _captureMessage = result.summary);
      await _refreshCaptureStatus();
      await store.refreshAll();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '同步失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  Future<void> _importFolder() async {
    final dir = await FilePicker.getDirectoryPath(
      dialogTitle: '选择 ComfyUI 的输出目录（会读取 PNG 内嵌的提示词与工作流）',
    );
    if (dir == null || dir.isEmpty) return;
    if (!mounted) return;

    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = '正在扫描 $dir …';
      _captureError = null;
    });
    try {
      final result = await store.api.importCaptureFolder(dir: dir, limit: 500);
      if (!mounted) return;
      setState(() => _captureMessage = result.summary);
      await _refreshCaptureStatus();
      await store.refreshAll();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '导入失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  /// 把本机 ComfyUI 里**已经保存的工作流文件**批量读进库（用户 bug ⑤）。
  ///
  /// 为什么需要这颗按钮：库里的工作流过去只有两个来源 —— 自动捕获（轮询 `/history`）与
  /// AI 按路径读。于是"用户在 ComfyUI 里存过、但本项目跑起来之后没跑过"的工作流
  /// **在 AI 眼里等于不存在**，首次使用时尤其明显。这里一键把它们读进来（幂等，重复点没关系）。
  Future<void> _importWorkflows() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = '正在扫描本机 ComfyUI 的 workflows 目录…';
      _captureError = null;
    });
    try {
      final result = await store.api.importWorkflowFiles();
      if (!mounted) return;
      setState(() => _captureMessage = result.summary);
      await _refreshCaptureStatus();
      await store.refreshAll();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '导入工作流失败：$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  Future<void> _pickComfyOutputDir() async {
    final dir = await FilePicker.getDirectoryPath(dialogTitle: '选择 ComfyUI 的输出目录');
    if (dir == null || dir.isEmpty) return;
    setState(() => _comfyOutputController.text = dir);
  }

  // -------------------------------------------------------------------------
  //  本地服务
  // -------------------------------------------------------------------------

  Future<void> _startLocalServices(BackendLauncher launcher, {bool force = true}) async {
    await launcher.ensureRunning(force: force);
    if (!mounted) return;
    if (launcher.isReady) {
      await context.read<LibraryStore>().refreshAll();
      await _loadCapture();
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
                  style: Theme.of(ctx).textTheme.labelSmall?.copyWith(fontFamily: 'monospace'),
                ),
              );
            },
          ),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('关闭')),
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
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('打开失败：$e')));
    }
  }

  // -------------------------------------------------------------------------
  //  远程访问与配对（`docs/pairing-design.md` §11）
  // -------------------------------------------------------------------------

  /// 手机上配对成功 / 忘记这台电脑之后：让 AI 工作台**重新读一次令牌**，
  /// 再取一次规则工件（配对前那次失败很可能就是因为没有令牌）。
  ///
  /// `context.read<AiWorkspaceStore?>()`（可空）而不是必填：设置页既有的用例
  /// （`settings_layout_test`）不注册这个 store —— 取不到就什么也不做，
  /// 配对本身照样成功，不该因为"状态没挂上"就报错。
  Future<void> _reloadPairingToken() async {
    if (!mounted) return;
    final store = context.read<AiWorkspaceStore?>();
    await store?.reloadPairingToken();
  }

  /// 「从桌面同步密钥」：真活在 `AiWorkspaceStore` 里（值只经手一次，界面不碰它）。
  Future<String> _syncCredentialsFromDesktop() async {
    if (!mounted) return '设置页已经关掉了，这次同步没有执行。';
    final store = context.read<AiWorkspaceStore?>();
    if (store == null) return 'AI 工作台还没准备好，稍后再试一次。';
    return store.syncCredentialsFromDesktop();
  }

  // -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final store = context.watch<LibraryStore>();
    final launcher = context.watch<BackendLauncher>();

    // 「本地服务 / 后端地址 / 打开项目目录」这些只在 **Windows 桌面端**有意义
    // （pwsh 拉起后端、本机盘符、资源管理器）。手机上它们一行都不该出现
    // （用户 bug 2026-09-28：手机设置显示了只有 Windows 有效的设置项）——
    // 手机连的是桌面端后端，地址选择在「同步与连接」里做。
    final isWindowsDesktop = !kIsWeb && Platform.isWindows;

    return Scaffold(
      appBar: AppBar(
        title: const Text('设置'),
      ),
      body: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 48),
        // 列数 = 可用宽度 / 550（宽窗口自动分栏，窄窗口仍是单列）
        child: AdaptiveColumns(
          sections: [
            if (isWindowsDesktop)
              AdaptiveSection(
                _section(theme, '本地服务（后端 + 数据库）',
                    _localServicesCard(theme, settings, launcher)),
                estimatedHeight: 820,
              ),
            AdaptiveSection(
              _section(
                theme,
                'AI 模型与凭据',
                Card(
                  child: ListTile(
                    contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
                    leading: const Icon(Icons.smart_toy_outlined),
                    title: const Text('Provider / 模型目录 / API Key'),
                    subtitle: const Text('配置协议、Base URL 与模型能力；密钥只写不读，不会出现在界面、数据库普通字段和日志里'),
                    trailing: const Icon(Icons.chevron_right),
                    onTap: () => Navigator.of(context).push(
                      MaterialPageRoute(builder: (_) => const AiProviderSettingsPage()),
                    ),
                  ),
                ),
              ),
              estimatedHeight: 120,
            ),
            // 紧跟「AI 模型与凭据」：「配对」与「凭据」是同一件事的两半 ——
            // 配对换来的是"这台手机能不能用桌面的东西"的令牌，而令牌正是
            // 密钥下行（`/api/ai/credentials`）的准入凭据。两块挨着看，用户才知道
            // "配完对还要在桌面上允许这台设备取密钥"。
            //
            // ⚠ 原来的「同步与连接」那一页**已经删掉**（2026-09-30 用户要求"改成自动、不用用户管"）：
            //   桌面端那一页纯属噪音（自己连自己），手机端的"选哪台桌面"现在由
            //   `autoPickBaseUrl` 在启动时自己挑（见 `app.dart` 的 `_pickDesktopAutomatically`），
            //   挑不到就保持原样、由各页面如实报"连不上"，不再让人去点一张地址列表。
            AdaptiveSection(
              _section(
                theme,
                '远程访问与配对',
                PairingSection(
                  baseUrl: settings.baseUrl,
                  onPaired: _reloadPairingToken,
                  onSyncCredentials: _syncCredentialsFromDesktop,
                  onSaveBaseUrl: settings.setBaseUrl,
                ),
              ),
              estimatedHeight: 560,
            ),
            AdaptiveSection(
              _section(theme, 'NSFW 内容', _nsfwCard(theme)),
              estimatedHeight: 300,
            ),
            // 手机端「产物缓存」（2026-11-01 用户要求给缓存设上限）。
            // 桌面端原件本来就在盘上，一行都不显示；判据与上面几节同一条。
            if (!isWindowsDesktop && !kIsWeb)
              AdaptiveSection(
                _section(theme, '产物缓存（本机）', _cacheCard(theme)),
                estimatedHeight: 240,
              ),
            // 「后端服务」（后端地址 + 存储目录）只在 Windows 桌面端有用：
            // 手机连的桌面地址在「同步与连接」里选。
            if (isWindowsDesktop)
              AdaptiveSection(
                _section(
                  theme,
                  '后端服务',
                Card(
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
                            hintText: '留空 = 自动（按 .run\\api.json 里后端记录的端口）',
                          ),
                        ),
                        const SizedBox(height: 8),
                        Text(
                          'Windows 桌面端用 127.0.0.1；Android 模拟器用 10.0.2.2；真机用电脑的局域网 IP。\n'
                          '本机地址（127.0.0.1 / localhost）永远自动：端口是后端启动时挑的（重启会换），'
                          'App 每次都读它写在 .run\\api.json 里的真实端口 —— 这里填什么都不会保存下来。\n'
                          '只有要连别的机器时手填才有意义（填了就不会被自动改）。',
                          style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                        ),
                        const SizedBox(height: 12),
                        // 窄窗口下这两枚按钮加起来比卡片宽（320px 实测溢出 7.1px）
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
                                      child: CircularProgressIndicator(strokeWidth: 2),
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
                                _testOk ? Icons.check_circle_outline : Icons.error_outline,
                                size: 18,
                                color: _testOk ? Colors.green : theme.colorScheme.error,
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
                                  '存储目录: ${_health!['storageDir']}',
                                  style: theme.textTheme.labelSmall
                                      ?.copyWith(color: theme.colorScheme.outline),
                                ),
                              ),
                              TextButton(
                                onPressed: () => _openFolder('${_health!['storageDir']}'),
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
              estimatedHeight: 320,
            ),
            AdaptiveSection(
              _section(theme, 'ComfyUI 自动捕获', _captureCard(theme)),
              estimatedHeight: 660,
            ),
            AdaptiveSection(
              _section(
                theme,
                '库统计',
                Card(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                    child: Column(
                      children: [
                        _statRow(theme, '提示词', '${store.stats.prompts}',
                            '生图 ${store.stats.byKind['IMAGE'] ?? 0} · 生视频 ${store.stats.byKind['VIDEO'] ?? 0} · 生音频 ${store.stats.byKind['AUDIO'] ?? 0}'),
                        const Divider(height: 20),
                        _statRow(theme, '生成产物', '${store.stats.media}',
                            '图片 ${store.stats.byMediaKind['IMAGE'] ?? 0} · 视频 ${store.stats.byMediaKind['VIDEO'] ?? 0} · 音频 ${store.stats.byMediaKind['AUDIO'] ?? 0}'),
                        const Divider(height: 20),
                        _statRow(theme, '标签', '${store.stats.tags}', null),
                        const Divider(height: 20),
                        _statRow(theme, '收藏',
                            '${store.stats.favoritePrompts} 提示词 / ${store.stats.favoriteMedia} 产物', null),
                        if (_captureStatus != null) ...[
                          const Divider(height: 20),
                          _statRow(
                            theme,
                            '自动捕获',
                            '${_captureStatus!.capturedRuns} 次运行 / ${_captureStatus!.capturedMedia} 个产物',
                            _captureStatus!.lastPollAt == null
                                ? '还没轮询过'
                                : '最近轮询 ${relativeTime(_captureStatus!.lastPollAt)}',
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
              estimatedHeight: 340,
            ),
            // 导出（2026-10-27）：产物原件本来就在 storage\media 里，但"带出去"
            // 需要把工作流 / 提示词 / 集合归属一起打包 —— 这一张卡是那个入口。
            if (isWindowsDesktop)
              AdaptiveSection(
                _section(
                  theme,
                  '导出产物',
                  Card(
                    child: ListTile(
                      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
                      leading: const Icon(Icons.save_alt),
                      title: const Text('导出全部产物…'),
                      subtitle: const Text(
                        '按 图片 / 视频 / 音频 分目录；同一集合单独成夹；'
                        '工作流与提示词信息一起带走（可先在画廊多选只导一部分）',
                      ),
                      trailing: const Icon(Icons.chevron_right),
                      onTap: () => showExportDialog(context),
                    ),
                  ),
                ),
                estimatedHeight: 120,
              ),
            AdaptiveSection(
              _section(
                theme,
                '界面',
                Card(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                    child: Column(
                      children: [
                        // 一行放下：**标题在左、控件在右**（与下面「每页数量」同一套排法）。
                        //
                        // ⚠ 别退回"`ListTile` + 另起一行 `Wrap` 放三个 chip"：这个卡片里的
                        // `Column` 是**默认的 `crossAxisAlignment: center`**，而 `Wrap`
                        // 只占内容宽度 —— 于是那三个 chip 会在整行里**居中**，
                        // 看着就是"标题独在左边、控件飘在中间"（用户 2026-10-13 报的
                        // "排版奇怪，不应该一行搞定吗"）。要么给 Row/Column 定对齐，
                        // 要么就老老实实一行。
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('主题'),
                          trailing: SegmentedButton<AppThemeMode>(
                            // 三段都是两三个字，320dp 上量过不会溢出
                            // （回归：settings_layout_test 的窄窗口那一条）
                            segments: [
                              for (final mode in AppThemeMode.values)
                                ButtonSegment(value: mode, label: Text(mode.label)),
                            ],
                            selected: {settings.themeMode},
                            onSelectionChanged: (picked) => settings.setThemeMode(picked.first),
                            // 不要勾选图标：三个短标签本来就窄，加了勾反而挤
                            // （选中态靠底色区分，M3 的 segmented button 本来就看得清）
                            showSelectedIcon: false,
                            style: const ButtonStyle(visualDensity: VisualDensity.compact),
                          ),
                        ),
                        const Divider(height: 1),
                        // 系统强调色（2026-10-24）：Windows / Android 12+ 读得到，
                        // 读到就自动当主题种子色；读不到就是"没有这一项"，回默认紫。
                        // 点整行 = 重新检测（用户在系统里换了强调色回来不用重启 App）。
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          onTap: _redetectAccent,
                          title: Row(
                            children: [
                              Container(
                                width: 12,
                                height: 12,
                                decoration: BoxDecoration(
                                  shape: BoxShape.circle,
                                  color: SystemAccent.color ?? AppTheme.defaultSeed,
                                  border: Border.all(
                                    color: theme.colorScheme.outlineVariant,
                                  ),
                                ),
                              ),
                              const SizedBox(width: 8),
                              const Flexible(child: Text('系统主题色')),
                            ],
                          ),
                          subtitle: Text(
                            SystemAccent.available
                                ? '${SystemAccent.sourceLabel}。已自动套用；关掉就用默认配色。点这一行重新检测。'
                                : '${SystemAccent.sourceLabel}，使用默认配色（换台设备 / 升级系统后点这一行再试）。',
                          ),
                          trailing: Switch(
                            // 取不到颜色时这一项没有意义：显示为关、且不可点，
                            // 免得用户以为"打开了却没效果"。
                            value: settings.followSystemAccent &&
                                SystemAccent.available,
                            onChanged: SystemAccent.available
                                ? (v) => settings.setFollowSystemAccent(v)
                                : null,
                          ),
                        ),
                        const Divider(height: 1),
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('每页数量'),
                          subtitle: Text('当前 ${settings.pageSize} 条'),
                          trailing: SizedBox(
                            width: 180,
                            child: Slider(
                              value: settings.pageSize.toDouble(),
                              min: 8,
                              max: 96,
                              divisions: 11,
                              label: '${settings.pageSize}',
                              onChanged: (v) => settings.setPageSize(v.round()),
                              onChangeEnd: (_) => store.refreshAll(),
                            ),
                          ),
                        ),
                        const Divider(height: 1),
                        // 「集合详情」两种形态由**用户自己定**（用户 2026-10-18：
                        // "能不能让我自由选择在电脑/手机排版的画廊的集合详情行为？"）
                        // —— 以前是代码按宽度档替他定（宽屏停靠、窄屏弹窗）。
                        // 两个宽度档各一项，改完**当场生效**：`SettingsStore` 是
                        // ChangeNotifier，画廊那边 watch 着它。
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('集合详情：宽屏'),
                          subtitle: const Text(
                            '窗口够宽（≥ 900）时用哪种。停靠面板 = 占住真实布局、画廊网格自己收窄，'
                            '谁都不遮谁；居中弹窗 = 浮在画廊上、背景虚化。',
                          ),
                          trailing: SegmentedButton<CollectionDetailStyle>(
                            segments: [
                              for (final style in CollectionDetailStyle.values)
                                ButtonSegment(value: style, label: Text(style.label)),
                            ],
                            selected: {settings.collectionDetailWide},
                            onSelectionChanged: (picked) =>
                                settings.setCollectionDetailWide(picked.first),
                            showSelectedIcon: false,
                            style: const ButtonStyle(visualDensity: VisualDensity.compact),
                          ),
                        ),
                        const Divider(height: 1),
                        ListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('集合详情：窄屏'),
                          subtitle: const Text(
                            '窗口较窄（< 900）时用哪种。停靠面板 = 占住真实布局、画廊网格自己收窄'
                            '（窗口很窄时网格会被挤得很小）；居中弹窗 = 浮在画廊上、背景虚化。',
                          ),
                          trailing: SegmentedButton<CollectionDetailStyle>(
                            segments: [
                              for (final style in CollectionDetailStyle.values)
                                ButtonSegment(value: style, label: Text(style.label)),
                            ],
                            selected: {settings.collectionDetailNarrow},
                            onSelectionChanged: (picked) =>
                                settings.setCollectionDetailNarrow(picked.first),
                            showSelectedIcon: false,
                            style: const ButtonStyle(visualDensity: VisualDensity.compact),
                          ),
                        ),
                        // 「打开项目目录」是资源管理器的事，手机上没有这一说
                        if (isWindowsDesktop) ...[
                          const Divider(height: 1),
                          ListTile(
                            contentPadding: EdgeInsets.zero,
                            leading: const Icon(Icons.folder_open_outlined),
                            title: const Text('打开项目目录'),
                            subtitle: Text(launcher.projectRoot ?? '未定位到项目目录'),
                            onTap: launcher.projectRoot == null
                                ? null
                                : () => _openFolder(launcher.projectRoot!),
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
              estimatedHeight: 380,
            ),
          ],
        ),
      ),
    );
  }

  /// 设置页的一块 = 小标题 + 卡片。分列时整块一起走，不会标题和卡片被拆到两列。
  Widget _section(ThemeData theme, String title, Widget card) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(title, style: theme.textTheme.titleSmall),
          const SizedBox(height: 8),
          card,
        ],
      );

  // -------------------------------------------------------------------------
  //  产物缓存（手机端，2026-11-01 用户要求"给缓存设个上限"）
  //
  //  它曾经是**唯一**会无界长大的本地存储（看过的缩略图 + 原件，原件含视频）。
  //  这一节只做"看得见 + 能清"；真正的淘汰在 `MediaCache` 里按总字节 LRU 做
  //  （缩略图 150MB、原件按这里的档位，默认 1.5GB）。
  // -------------------------------------------------------------------------

  Widget _cacheCard(ThemeData theme) {
    final settings = context.watch<SettingsStore>();
    return MediaCacheCard(
      stats: _cacheStats,
      budgetMb: settings.mediaCacheOrigBudgetMb,
      busy: _cacheBusy,
      onBudgetChanged: _onBudgetChanged,
      onClear: _clearCache,
    );
  }

  /// 换档位：写偏好（`SettingsStore` 会**当场**把新预算交给缓存并淘汰一次），
  /// 再等一拍重新统计 —— 淘汰是异步的，紧接着读会读到淘汰前的数字。
  Future<void> _onBudgetChanged(int mb) async {
    final settings = context.read<SettingsStore>();
    await settings.setMediaCacheOrigBudgetMb(mb);
    await Future<void>.delayed(const Duration(milliseconds: 120));
    await _loadCacheStats();
  }

  Future<void> _clearCache() async {
    setState(() => _cacheBusy = true);
    final freed = await MediaCache.instance.clear();
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() {
      _cacheStats = stats;
      _cacheBusy = false;
    });
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(freed > 0 ? '已清理 ${formatSize(freed)}' : '缓存本来就是空的'),
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  NSFW 内容（2026-09-20 用户要求）
  //
  //  两个选项**放一起**（用户原话）。真源在后端的 `ai.nsfw`，这里只是读写通道：
  //  ① 开关：开 = 系统提示词里注入那段"越狱"提示词，关 = 注入一句"本机没开"；
  //  ② 模糊：开 = 预览图打码（点一下解开这一张），且**只有 ① 也开着才生效**。
  // -------------------------------------------------------------------------

  Widget _nsfwCard(ThemeData theme) {
    final nsfw = context.watch<NsfwStore>();
    final busy = _nsfwBusy;
    final enabled = nsfw.nsfwEnabled;
    final blur = nsfw.blurOptionEnabled;

    Future<void> apply({bool? nextEnabled, bool? nextBlur}) async {
      setState(() => _nsfwBusy = true);
      final next = await nsfw.save(
        enabled: nextEnabled ?? enabled,
        // 关掉开关时模糊也一并落成"关"：不然下次再打开会莫名其妙地全糊
        blur: (nextEnabled == false) ? false : (nextBlur ?? blur),
      );
      if (!mounted) return;
      setState(() => _nsfwBusy = false);
      if (!next.ok) {
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(next.message)));
      }
    }

    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 8, 8, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 4, 8, 8),
              child: Text(
                '成人内容相关。改完下一次回复立刻生效。',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
            _SwitchRow(
              title: 'NSFW 模式',
              subtitle: enabled
                  ? '已开启，但是AI工作台的对话有可能被审查严格的模型拦截。'
                  : '未开启，模型会按常规内容策略作答。',
              value: enabled,
              onChanged: busy ? null : (v) => apply(nextEnabled: v),
            ),
            const Divider(height: 1),
            _SwitchRow(
              title: 'NSFW 内容模糊显示',
              subtitle: !enabled
                  ? '先开启上面的「NSFW 模式」才有意义。'
                  : (blur
                      ? '已开启：画廊 / 提示词 / 标签页与 AI 回复里的预览图会模糊'
                      : '未开启：预览图原样显示。'),
              value: blur,
              // 开关没开时这一项置灰：两个选项的从属关系要一眼看得出来
              onChanged: (busy || !enabled) ? null : (v) => apply(nextBlur: v),
            ),
            if (nsfw.promptVersion.isNotEmpty)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
                child: Text(
                  '注入正文版本：${nsfw.promptVersion}',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ),
            if (nsfw.error != null)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 6, 16, 0),
                child: Text(
                  nsfw.error!,
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error),
                ),
              ),
          ],
        ),
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  本地服务卡片
  // -------------------------------------------------------------------------

  Widget _localServicesCard(ThemeData theme, SettingsStore settings, BackendLauncher launcher) {
    final busy = launcher.busy;
    final statusColor = launcher.isReady
        ? Colors.green
        : (launcher.phase == StartupPhase.failed ? theme.colorScheme.error : theme.colorScheme.primary);
    final tail = launcher.logLines.length > 4
        ? launcher.logLines.sublist(launcher.logLines.length - 4)
        : launcher.logLines;

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _SwitchRow(
              value: settings.autoStartBackend,
              onChanged: settings.setAutoStartBackend,
              title: '启动 App 时自动拉起服务',
              subtitle: '先把数据库准备好再起后端；P2 起数据库是本地 SQLite 文件，后端会自己建，'
                  '所以这一步通常只是一次健康检查',
            ),
            _SwitchRow(
              value: settings.stopServicesOnExit,
              onChanged: (value) async {
                await settings.setStopServicesOnExit(value);
                // 关掉时要**真的撤销认领**，否则退出收尾照样会把它停掉
                if (!value) await launcher.disarm();
              },
              title: '关闭 App 时一并停止本地服务',
              subtitle: '开着时：关掉 App 就停掉本机的后端（包括启动 App 之前就已经在跑的）。\n'
                  '关掉时：服务常驻，下次开 App 是热启动。\n',
            ),
            _SwitchRow(
              value: settings.allowRemoteBackend,
              onChanged: (value) async {
                await settings.setAllowRemoteBackend(value);
                if (!mounted) return;
                ScaffoldMessenger.of(context).showSnackBar(SnackBar(
                  content: Text(value
                      ? '已允许局域网访问，重启后端（下面的「重启」按钮）后生效。'
                      : '已恢复仅本机访问，重启后端后生效。'),
                ));
              },
              title: '允许手机 / 局域网设备连接',
              // ⚠ 这一项**不是**「远程访问与配对」里那个「允许移动端连接」：
              //   · 这里 = **要不要让后端监听局域网**（起进程时定 → 必须重启后端）；
              //   · 那里 = **配对闸门**（局域网请求要不要带令牌，API 即刻生效）。
              // 两个名字都像"允许手机连"，所以必须在这里把顺序讲出来，
              // 否则用户永远分不清"我明明开了，手机还是连不上"是卡在哪一道。
              subtitle: '① 让后端监听局域网（改完要点下面的「重启」才生效）。\n'
                  '手机最终能不能连，还要看「远程访问与配对」里的 ② 配对闸门。',
            ),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  launcher.isReady ? Icons.check_circle_outline : Icons.info_outline,
                  size: 18,
                  color: statusColor,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(launcher.message, style: theme.textTheme.bodySmall),
                ),
              ],
            ),
            // 「端口没能要回上次那个」必须**看得见**（2026-10-13）：
            // 后端平时会要回上一次的端口（`--port <上次>`），所以手机上存的
            // `http://<局域网IP>:<端口>` 一直有效；一旦那个端口被别的程序占了，
            // 后端只能回落 —— **手机保存的地址当场失效**。
            // 不写出来的话，用户遇到的只是"手机突然连不上"，猜不到该去重扫二维码
            // （本仓 §12.3：界面上看不见的现象，用户和模型就只能靠猜）。
            if (launcher.portFallbackFrom != null) ...[
              const SizedBox(height: 8),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(Icons.warning_amber_rounded, size: 18, color: theme.colorScheme.error),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      '端口 ${launcher.portFallbackFrom} 这次没能用上（被别的程序占了），'
                      '后端改听另一个端口 —— 手机上保存的地址已失效：'
                      '请让手机重新扫一次「远程访问与配对」里的二维码。',
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.error),
                    ),
                  ),
                ],
              ),
            ],
            if (tail.isNotEmpty && (busy || launcher.phase == StartupPhase.failed)) ...[
              const SizedBox(height: 8),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.3),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final line in tail)
                      Text(
                        line,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelSmall?.copyWith(fontFamily: 'monospace'),
                      ),
                  ],
                ),
              ),
            ],
            const SizedBox(height: 6),
            // 按钮组用 `Wrap`（用户 bug：窄窗口下"控件显示不全"）：窗口窄到 ~360px
            // （纯聊天/单列设置卡）时，两枚 `TextButton.icon` 的自然宽度加起来比卡片
            // 还宽，`Row` 只能**溢出被裁**（实测 320px 时溢出 7.1px）。`Wrap` 放不下
            // 就换行，按钮一个都不会消失。
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                TextButton.icon(
                  onPressed: () {
                    _projectRootController.text = BackendLauncher.detectProjectRoot() ?? '';
                  },
                  icon: const Icon(Icons.search, size: 18),
                  label: const Text('自动探测'),
                ),
                TextButton.icon(
                  onPressed: () async {
                    await settings.setProjectRoot(_projectRootController.text);
                    if (!mounted) return;
                    ScaffoldMessenger.of(context)
                        .showSnackBar(const SnackBar(content: Text('运行根目录已保存')));
                  },
                  icon: const Icon(Icons.save_outlined, size: 18),
                  label: const Text('保存'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            const Divider(height: 24),
            Wrap(
              spacing: 10,
              runSpacing: 8,
              children: [
                FilledButton.icon(
                  onPressed: busy ? null : () => _startLocalServices(launcher),
                  icon: const Icon(Icons.play_arrow),
                  label: const Text('启动 / 修复服务'),
                ),
                OutlinedButton.icon(
                  onPressed: busy ? null : () => _runActionThenRediscover('restart'),
                  icon: const Icon(Icons.restart_alt),
                  label: const Text('重启'),
                ),
                OutlinedButton.icon(
                  onPressed: busy ? null : () => launcher.runAction('down'),
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
    );
  }

  // -------------------------------------------------------------------------
  //  自动捕获卡片
  // -------------------------------------------------------------------------

  Widget _captureCard(ThemeData theme) {
    final status = _captureStatus;
    final reachable = status?.comfyReachable ?? false;

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  reachable ? Icons.link : Icons.link_off,
                  size: 18,
                  color: reachable ? Colors.green : theme.colorScheme.error,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    status == null
                        ? '尚未读到状态'
                        : reachable
                            ? '已连上 ComfyUI ${status.comfyUrl}'
                                '（队列 运行中 ${status.queueRunning} / 等待 ${status.queuePending}）'
                            : '连不上 ComfyUI ${status.comfyUrl}',
                    style: theme.textTheme.bodySmall,
                  ),
                ),
                if (_loadingCapture)
                  const SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                else
                  IconButton(
                    tooltip: '刷新状态',
                    onPressed: _captureBusy ? null : _loadCapture,
                    icon: const Icon(Icons.refresh),
                  ),
              ],
            ),
            if (status?.lastError != null) ...[
              const SizedBox(height: 4),
              Text(
                '最近一次错误：${status!.lastError}',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error),
              ),
            ],
            const SizedBox(height: 8),
            Text(
              '后端每隔几秒，就把「提示词 + 全部参数 + 工作流 + 生成的图片/视频/音频」整条收进库里并互相关联。ComfyUI 开着就行，不需要改动你的工作流。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 12),
            _SwitchRow(
              value: _captureEnabled,
              onChanged: _capture == null ? null : (v) => setState(() => _captureEnabled = v),
              title: '开启自动捕获',
              subtitle: '关闭后只保留手动「立即同步」和目录导入',
            ),
            TextField(
              controller: _comfyUrlController,
              decoration: const InputDecoration(
                labelText: 'ComfyUI 地址',
                hintText: 'http://127.0.0.1:8188',
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _comfyOutputController,
              decoration: const InputDecoration(
                labelText: 'ComfyUI 输出目录（可选）',
                hintText: r'例如 D:\Comfy-Desktop\ComfyUI-Shared\output',
                helperText: '填了就直接读本地文件（快、不占带宽）；留空则通过 HTTP 下载。',
              ),
            ),
            const SizedBox(height: 4),
            Align(
              alignment: Alignment.centerLeft,
              child: Wrap(
                spacing: 8,
                children: [
                  TextButton.icon(
                    onPressed: _pickComfyOutputDir,
                    icon: const Icon(Icons.folder_open, size: 18),
                    label: const Text('选择目录…'),
                  ),
                  // 用户"其他建议"第 3 条：发布包是便携式的，ComfyUI 装在哪不能靠猜。
                  // 探测是只读的，点「使用这个目录」才写进配置（绝不偷偷改用户设置）。
                  TextButton.icon(
                    onPressed: _captureBusy ? null : _locateComfy,
                    icon: const Icon(Icons.travel_explore, size: 18),
                    label: const Text('自动查找 ComfyUI'),
                  ),
                ],
              ),
            ),
            if (_comfyLocation != null) _comfyLocationPanel(theme),
            const SizedBox(height: 4),
            Row(
              children: [
                SizedBox(
                  width: 96,
                  child: Text('轮询间隔', style: theme.textTheme.bodyMedium),
                ),
                Expanded(
                  child: Slider(
                    value: _pollSeconds.toDouble(),
                    min: 1,
                    max: 30,
                    divisions: 29,
                    label: '$_pollSeconds 秒',
                    onChanged: _capture == null
                        ? null
                        : (v) => setState(() => _pollSeconds = v.round()),
                  ),
                ),
                SizedBox(
                  width: 64,
                  child: Text('$_pollSeconds 秒', style: theme.textTheme.labelMedium),
                ),
              ],
            ),
            TextField(
              controller: _autoTagController,
              decoration: const InputDecoration(
                labelText: '自动标签',
                helperText: '捕获进来的提示词会自动打上这个标签，方便在画廊里筛「ComfyUI 生成的」',
              ),
            ),
            const SizedBox(height: 14),
            Wrap(
              spacing: 10,
              runSpacing: 8,
              children: [
                FilledButton.icon(
                  onPressed: (_captureBusy || _capture == null) ? null : _saveCapture,
                  icon: const Icon(Icons.save_outlined),
                  label: const Text('保存配置'),
                ),
                OutlinedButton.icon(
                  onPressed: _captureBusy ? null : _syncNow,
                  icon: const Icon(Icons.sync),
                  label: const Text('立即同步'),
                ),
                OutlinedButton.icon(
                  onPressed: _captureBusy ? null : _importFolder,
                  icon: const Icon(Icons.download_for_offline_outlined),
                  label: const Text('导入已有产物…'),
                ),
                // 用户 bug ⑤：库只收"捕获过的运行"，而用户机器上的工作流一直在磁盘上
                // （`user\<用户>\workflows`）—— 一键把它们读进来，第一次用 AI 就不用"先跑一次"了
                OutlinedButton.icon(
                  onPressed: _captureBusy ? null : _importWorkflows,
                  icon: const Icon(Icons.account_tree_outlined),
                  label: const Text('导入本机工作流'),
                ),
              ],
            ),
            if (_captureError != null) ...[
              const SizedBox(height: 10),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(Icons.error_outline, size: 18, color: theme.colorScheme.error),
                  const SizedBox(width: 8),
                  Expanded(
                    child: SelectableText(_captureError!, style: theme.textTheme.bodySmall),
                  ),
                ],
              ),
            ],
            if (_captureMessage != null) ...[
              const SizedBox(height: 10),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Icon(Icons.check_circle_outline, size: 18, color: Colors.green),
                  const SizedBox(width: 8),
                  Expanded(
                    child: SelectableText(_captureMessage!, style: theme.textTheme.bodySmall),
                  ),
                ],
              ),
            ],
            if (status != null && status.recent.isNotEmpty) ...[
              const Divider(height: 24),
              Text('最近捕获', style: theme.textTheme.labelLarge),
              const SizedBox(height: 4),
              for (final run in status.recent.take(8))
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 3),
                  child: Row(
                    children: [
                      Icon(
                        run.status == 'success'
                            ? Icons.check_circle_outline
                            : (run.status == 'error' ? Icons.error_outline : Icons.remove_circle_outline),
                        size: 15,
                        color: run.status == 'success'
                            ? Colors.green
                            : theme.colorScheme.outline,
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          run.title?.isNotEmpty == true ? run.title! : run.runKey,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodySmall,
                        ),
                      ),
                      Text(
                        '${run.mediaCount} 个产物 · ${relativeTime(run.capturedAt)}',
                        style: theme.textTheme.labelSmall
                            ?.copyWith(color: theme.colorScheme.outline),
                      ),
                    ],
                  ),
                ),
            ],
          ],
        ),
      ),
    );
  }

  // -------------------------------------------------------------------------

  Widget _statRow(ThemeData theme, String label, String value, String? sub) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 90,
          child: Text(label, style: theme.textTheme.bodyMedium),
        ),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                value,
                style: theme.textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w700),
              ),
              if (sub != null && sub.isNotEmpty)
                Text(
                  sub,
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// 设置项里的开关行。
///
/// `SwitchListTile` 自己的 `contentPadding` 是零，直接放进卡片里的话，
/// 标题会顶到卡片左边缘、开关顶到右边缘，跟同卡片里输入框的留白对不上 ——
/// 所以统一套一层带圆角底色的内边距（顺便把这一行和其他内容区分开）。
class _SwitchRow extends StatelessWidget {
  final bool value;
  final ValueChanged<bool>? onChanged;
  final String title;
  final String subtitle;

  const _SwitchRow({
    required this.value,
    required this.onChanged,
    required this.title,
    required this.subtitle,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 底色必须落在 Material 上（不能用 Container 的 decoration）：
    // ListTile 的水波纹画在最近的 Material 上，中间垫一层 DecoratedBox 会把它盖住。
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
