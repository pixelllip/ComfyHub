import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/backend_launcher.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import 'ai_provider_settings_page.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late final TextEditingController _urlController;
  late final TextEditingController _projectRootController;
  late final TextEditingController _mysqlDataDirController;
  late final TextEditingController _comfyUrlController;
  late final TextEditingController _comfyOutputController;
  late final TextEditingController _autoTagController;

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

  /// 「自动查找 ComfyUI」的探测结果（用户"其他建议"第 3 条）；null = 还没查过。
  ComfyLocation? _comfyLocation;

  @override
  void initState() {
    super.initState();
    final settings = context.read<SettingsStore>();
    _urlController = TextEditingController(text: settings.baseUrl);
    // 没手动设过就直接把自动探测到的路径填进去，省得用户对着空框猜
    _projectRootController = TextEditingController(
      text: settings.projectRoot ?? BackendLauncher.detectProjectRoot() ?? '',
    );
    _mysqlDataDirController = TextEditingController(text: settings.mysqlDataDir ?? '');
    _comfyUrlController = TextEditingController(text: 'http://127.0.0.1:8188');
    _comfyOutputController = TextEditingController();
    _autoTagController = TextEditingController(text: 'ComfyUI');
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadCapture());
  }

  @override
  void dispose() {
    _urlController.dispose();
    _projectRootController.dispose();
    _mysqlDataDirController.dispose();
    _comfyUrlController.dispose();
    _comfyOutputController.dispose();
    _autoTagController.dispose();
    super.dispose();
  }

  // -------------------------------------------------------------------------
  //  后端地址
  // -------------------------------------------------------------------------

  Future<void> _test() async {
    final settings = context.read<SettingsStore>();
    final store = context.read<LibraryStore>();
    setState(() {
      _testing = true;
      _testResult = null;
    });
    await settings.setBaseUrl(_urlController.text);
    try {
      final h = await store.api.health();
      if (!mounted) return;
      setState(() {
        _health = h;
        _testOk = h['status'] == 'ok';
        _testResult = _testOk
            ? '连接成功（版本 ${h['version']}，数据库 ${h['database']}）'
            : '服务可访问，但状态异常：${h['status']}';
      });
      await store.refreshAll();
      await _loadCapture();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _testOk = false;
        _health = null;
        _testResult = '连接失败：$e';
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
        _captureMessage = _captureEnabled ? '已保存，自动捕获已开启' : '已保存，自动捕获已关闭';
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
      dialogTitle: '选择 ComfyUI 的 output 目录（会读取 PNG 内嵌的提示词与工作流）',
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

  Future<void> _relocateMysql(BackendLauncher launcher) async {
    final target = _mysqlDataDirController.text.trim();
    if (target.isEmpty) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('迁移 MySQL 数据目录'),
        content: Text(
          '将把整个 MySQL 实例目录（数据文件 + 配置 + 日志）复制到：\n$target\n\n'
          '过程中会先停 MySQL、复制完成后再拉起来。源目录不会被删除，'
          '确认新目录没问题后可以自己删掉。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('开始迁移')),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    final ok = await launcher.moveMysqlData(target);
    if (!mounted) return;
    setState(() {
      _mysqlDataDirController.text = context.read<SettingsStore>().mysqlDataDir ?? target;
    });
    if (!ok) {
      _showLogs(launcher);
    } else {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('MySQL 数据目录已迁移到 $target')),
      );
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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final store = context.watch<LibraryStore>();
    final launcher = context.watch<BackendLauncher>();

    return Scaffold(
      appBar: AppBar(
        title: const Text('设置'),
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(1),
          child: Divider(height: 1, color: theme.dividerColor),
        ),
      ),
      body: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 48),
        // 列数 = 可用宽度 / 550（宽窗口自动分栏，窄窗口仍是单列）
        child: AdaptiveColumns(
          sections: [
            AdaptiveSection(
              _section(theme, '本地服务（MySQL + 后端）',
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
                          decoration: const InputDecoration(
                            labelText: '后端地址',
                            hintText: 'http://127.0.0.1:8080',
                          ),
                        ),
                        const SizedBox(height: 8),
                        Text(
                          'Windows 桌面端用 127.0.0.1；Android 模拟器用 10.0.2.2；真机用电脑的局域网 IP。',
                          style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                        ),
                        const SizedBox(height: 12),
                        Row(
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
                            const SizedBox(width: 10),
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
            AdaptiveSection(
              _section(
                theme,
                '界面',
                Card(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                    child: Column(
                      children: [
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
                    ),
                  ),
                ),
              ),
              estimatedHeight: 200,
            ),
            AdaptiveSection(
              _section(
                theme,
                '关于',
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text('ComfyHub', style: theme.textTheme.titleMedium),
                        const SizedBox(height: 4),
                        Text(
                          'Flutter 前端 + Kotlin(Ktor) 后端 + 本地 MySQL 的 ComfyUI 提示词与生成产物管理器。'
                          '后端会轮询 ComfyUI 的 /history 自动捕获工作流、参数与产物。',
                          style: theme.textTheme.bodySmall,
                        ),
                        const SizedBox(height: 12),
                        _endpoint(theme, 'GET', '/api/prompts', '提示词列表 / 搜索 / 标签筛选'),
                        _endpoint(theme, 'POST', '/api/prompts', '新建提示词'),
                        _endpoint(theme, 'GET', '/api/prompts/{id}/media', '某条提示词下的全部产物'),
                        _endpoint(theme, 'POST', '/api/media/upload', '上传产物（multipart，可多文件）'),
                        _endpoint(theme, 'GET', '/api/media/{id}', '产物详情（含关联提示词摘要）'),
                        _endpoint(theme, 'PATCH', '/api/media/{id}', '关联 / 更换提示词、收藏、改名'),
                        _endpoint(theme, 'GET', '/api/media/{id}/file', '原始文件（支持 Range，可拖动进度）'),
                        _endpoint(theme, 'GET', '/api/tags', '标签词表'),
                        _endpoint(theme, 'GET', '/api/capture/status', '自动捕获状态 + 最近捕获记录'),
                        _endpoint(theme, 'PUT', '/api/capture/config', '修改自动捕获配置'),
                        _endpoint(theme, 'POST', '/api/capture/poll', '立刻轮询一次 ComfyUI'),
                        _endpoint(theme, 'POST', '/api/capture/import', '导入已有产物目录（含 PNG 内嵌工作流）'),
                        _endpoint(theme, 'POST', '/api/ingest/comfyui', '捕获入口（自定义节点 / 外部脚本推送）'),
                        _endpoint(theme, 'GET', '/api/prompts/{id}/workflow', '该提示词的完整工作流 JSON'),
                      ],
                    ),
                  ),
                ),
              ),
              estimatedHeight: 640,
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
              subtitle: '按「MySQL → 后端」的顺序启动，后端已健康就不会重复启动',
            ),
            _SwitchRow(
              value: settings.stopServicesOnExit,
              onChanged: (value) async {
                await settings.setStopServicesOnExit(value);
                // 关掉时要**真的撤销**已挂的守护进程，否则它在 App 死后照样停服务
                if (!value) await launcher.disarmOwnerWatch();
              },
              title: '关闭 App 时一并停止本地服务',
              subtitle: '开着时：关掉 App 就停掉本机的 MySQL / 后端（包括启动 App 之前就已经在跑的），'
                  '下次开 App 是冷启动。关掉时：服务常驻，下次开 App 是热启动。',
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
            const SizedBox(height: 14),
            TextField(
              controller: _projectRootController,
              decoration: InputDecoration(
                labelText: '项目根目录',
                hintText: BackendLauncher.detectProjectRoot() ?? r'例如 D:\myProject\FlutterProject\viewer',
                helperText: '里面要有 scripts\\comfyhub.ps1；留空表示自动探测',
              ),
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                TextButton.icon(
                  onPressed: () {
                    _projectRootController.text = BackendLauncher.detectProjectRoot() ?? '';
                  },
                  icon: const Icon(Icons.search, size: 18),
                  label: const Text('自动探测'),
                ),
                const SizedBox(width: 4),
                TextButton.icon(
                  onPressed: () async {
                    await settings.setProjectRoot(_projectRootController.text);
                    if (!mounted) return;
                    ScaffoldMessenger.of(context)
                        .showSnackBar(const SnackBar(content: Text('项目根目录已保存')));
                  },
                  icon: const Icon(Icons.save_outlined, size: 18),
                  label: const Text('保存'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            TextField(
              controller: _mysqlDataDirController,
              decoration: const InputDecoration(
                labelText: 'MySQL 数据目录',
                hintText: r'例如 D:\mysql-data\comfyhub',
                helperText: '留空 = 项目下的 .mysql；换目录后点「迁移现有数据到该目录」',
              ),
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                TextButton.icon(
                  onPressed: () async {
                    await settings.setMysqlDataDir(_mysqlDataDirController.text);
                    if (!mounted) return;
                    ScaffoldMessenger.of(context)
                        .showSnackBar(const SnackBar(content: Text('MySQL 数据目录已保存，重启服务后生效')));
                  },
                  icon: const Icon(Icons.save_outlined, size: 18),
                  label: const Text('保存'),
                ),
                const SizedBox(width: 4),
                TextButton.icon(
                  onPressed: busy ? null : () => _relocateMysql(launcher),
                  icon: const Icon(Icons.drive_file_move_outline, size: 18),
                  label: const Text('迁移现有数据到该目录…'),
                ),
              ],
            ),
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
                  onPressed: busy ? null : () => launcher.runAction('restart'),
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
              '后端每隔几秒读一次 ComfyUI 的 /history：跑完一次生成，就把「提示词 + 全部参数 + 工作流 + 生成的图片/视频/音频」'
              '整条收进库里并互相关联。ComfyUI 开着就行，不需要改动你的工作流。',
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
                helperText: '填了就直接读本地文件（快、不占带宽）；留空则通过 HTTP 下载',
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

  Widget _endpoint(ThemeData theme, String method, String path, String desc) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 52,
            child: Text(
              method,
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.primary,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SelectableText(path, style: theme.textTheme.bodySmall?.copyWith(fontFamily: 'monospace')),
                Text(
                  desc,
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ],
            ),
          ),
        ],
      ),
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
