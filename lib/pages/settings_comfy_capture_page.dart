import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../core/theme.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/settings_scaffold.dart';
import '../widgets/settings_switch_row.dart';

/// 「ComfyUI 自动捕获」子页。
///
/// 从设置首页移出（一张卡中超过两项的配置收进子界面）。
/// 页面内容与原来那张卡**一模一样**（状态 / 开关 / 地址 / 输出目录 / 轮询 /
/// 自动标签 / 保存 / 立即同步 / 导入），只是现在有独立的顶栏与滚动区。
class ComfyCaptureSettingsPage extends StatefulWidget {
  /// 设置搜索跳转到的区块（例如 `address` / `polling`）。null = 从页首开始。
  final String? initialSectionId;

  const ComfyCaptureSettingsPage({super.key, this.initialSectionId});

  @override
  State<ComfyCaptureSettingsPage> createState() =>
      _ComfyCaptureSettingsPageState();
}

class _ComfyCaptureSettingsPageState extends State<ComfyCaptureSettingsPage> {
  CaptureConfig? _capture;
  CaptureStatus? _captureStatus;
  bool _loadingCapture = false;
  String? _captureError;
  String? _captureMessage;
  bool _captureBusy = false;
  bool _captureEnabled = true;
  int _pollSeconds = 4;

  /// 「自动查找 ComfyUI」的探测结果；null = 还没查过。
  ComfyLocation? _comfyLocation;

  late final TextEditingController _comfyUrlController;
  late final TextEditingController _comfyOutputController;
  late final TextEditingController _autoTagController;

  @override
  void initState() {
    super.initState();
    _comfyUrlController = TextEditingController(text: 'http://127.0.0.1:8188');
    _comfyOutputController = TextEditingController();
    _autoTagController = TextEditingController(text: 'ComfyUI');
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadCapture());
  }

  @override
  void dispose() {
    _comfyUrlController.dispose();
    _comfyOutputController.dispose();
    _autoTagController.dispose();
    super.dispose();
  }

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

  /// 自动查找本机的 ComfyUI。探测本身是只读的：**不点「使用这个目录」就不会改任何设置**。
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
            : '找到了输出目录 ${found.outputDir}';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '查找失败，$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  /// 将探测到的输出目录写进配置（手动触发）。
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
        _captureMessage = '已使用探测到的输出目录 ${saved.outputDir}';
      });
      await _refreshCaptureStatus();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '应用失败，$e');
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
                Text(
                  '找到 ComfyUI，位置在 ${loc.home ?? '-'}',
                  style: theme.textTheme.bodySmall,
                ),
                const SizedBox(height: 2),
                Text(
                  '输出目录是 ${loc.outputDir}'
                  '${loc.source != null ? '（${loc.source}）' : ''}',
                  style: theme.textTheme.labelSmall,
                ),
                const SizedBox(height: 6),
                if (loc.canApply)
                  FilledButton.tonal(
                    onPressed: _captureBusy
                        ? null
                        : () => _applyComfyLocation(loc.outputDir!),
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

  Future<void> _saveCapture() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _captureBusy = true;
      _captureMessage = null;
      _captureError = null;
    });
    try {
      final base = _capture ?? const CaptureConfig();
      final saved = await store.api.updateCaptureConfig(
        base.copyWith(
          enabled: _captureEnabled,
          comfyUrl: _comfyUrlController.text.trim(),
          outputDir: _comfyOutputController.text.trim(),
          clearOutputDir: _comfyOutputController.text.trim().isEmpty,
          pollSeconds: _pollSeconds,
          autoTag: _autoTagController.text.trim(),
        ),
      );
      if (!mounted) return;
      setState(() {
        _capture = saved;
        _captureMessage = _captureEnabled ? '自动捕获已开启' : '自动捕获已关闭';
      });
      await _refreshCaptureStatus();
    } catch (e) {
      if (!mounted) return;
      setState(() => _captureError = '保存失败，$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
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
      setState(() => _captureError = '同步失败，$e');
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
      setState(() => _captureError = '导入失败，$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  /// 把本机 ComfyUI 里已经保存的工作流文件批量读进库（幂等，重复点没关系）。
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
      setState(() => _captureError = '导入工作流失败，$e');
    } finally {
      if (mounted) setState(() => _captureBusy = false);
    }
  }

  Future<void> _pickComfyOutputDir() async {
    final dir = await FilePicker.getDirectoryPath(
      dialogTitle: '选择 ComfyUI 的输出目录',
    );
    if (dir == null || dir.isEmpty) return;
    setState(() => _comfyOutputController.text = dir);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = AppColors.of(context);
    final status = _captureStatus;
    final reachable = status?.comfyReachable ?? false;

    return SettingsScaffold(
      title: 'ComfyUI 自动捕获',
      initialSectionId: widget.initialSectionId,
      sections: [
        SettingsSection(
          title: '捕获配置',
          id: 'capture',
          // 估算值需接近实际，否则两列的"连续切分"会选错切点。
          estimatedHeight: 400,
          child: Card(
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
                        color: reachable
                            ? colors.success
                            : theme.colorScheme.error,
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
                      '最近一次错误是 ${status!.lastError}',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ],
                  const SizedBox(height: 8),
                  Text(
                    '后端每隔几秒，就把「提示词 + 全部参数 + 工作流 + 生成的图片/视频/音频」整条收进库里并互相关联。ComfyUI 开着就行，不需要改动你的工作流。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 12),
                  SettingsSwitchRow(
                    value: _captureEnabled,
                    onChanged: _capture == null
                        ? null
                        : (v) => setState(() => _captureEnabled = v),
                    title: '开启自动捕获',
                    subtitle: '关闭后只保留手动「立即同步」和目录导入',
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '地址与目录',
          id: 'address',
          estimatedHeight: 360,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
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
                      hintText: r'例如 D:\ComfyUI\output（按你自己装的位置填）',
                      helperText: '填了就直接读本地文件，快且不占带宽。留空则通过 HTTP 下载。',
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
                        // 发布包是便携式的，ComfyUI 安装位置不固定；探测为只读，
                        // 点「使用这个目录」后才写入配置。
                        TextButton.icon(
                          onPressed: _captureBusy ? null : _locateComfy,
                          icon: const Icon(Icons.travel_explore, size: 18),
                          label: const Text('自动查找 ComfyUI'),
                        ),
                      ],
                    ),
                  ),
                  if (_comfyLocation != null) _comfyLocationPanel(theme),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '轮询与操作',
          id: 'polling',
          // 轮询 + 自动标签 + 四个按钮 + 最近捕获（最多 8 条）
          estimatedHeight: 820,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
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
                        child: Text(
                          '$_pollSeconds 秒',
                          style: theme.textTheme.labelMedium,
                        ),
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
                        onPressed: (_captureBusy || _capture == null)
                            ? null
                            : _saveCapture,
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
                        Icon(
                          Icons.error_outline,
                          size: 18,
                          color: theme.colorScheme.error,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: SelectableText(
                            _captureError!,
                            style: theme.textTheme.bodySmall,
                          ),
                        ),
                      ],
                    ),
                  ],
                  if (_captureMessage != null) ...[
                    const SizedBox(height: 10),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          Icons.check_circle_outline,
                          size: 18,
                          color: colors.success,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: SelectableText(
                            _captureMessage!,
                            style: theme.textTheme.bodySmall,
                          ),
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
                                  : (run.status == 'error'
                                        ? Icons.error_outline
                                        : Icons.remove_circle_outline),
                              size: 15,
                              color: run.status == 'success'
                                  ? colors.success
                                  : theme.colorScheme.onSurfaceVariant,
                            ),
                            const SizedBox(width: 8),
                            Expanded(
                              child: Text(
                                run.title?.isNotEmpty == true
                                    ? run.title!
                                    : run.runKey,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: theme.textTheme.bodySmall,
                              ),
                            ),
                            Text(
                              '${run.mediaCount} 个产物 · ${relativeTime(run.capturedAt)}',
                              style: theme.textTheme.labelSmall?.copyWith(
                                color: theme.colorScheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                        ),
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
