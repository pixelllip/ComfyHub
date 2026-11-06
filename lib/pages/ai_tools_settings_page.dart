import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/settings_scaffold.dart';

/// AI 工具权限（M4 / AIH-031 ~ AIH-036 / AIH-049）。
///
/// 三件事，全部来自后端（`GET /api/ai/tools` 与 `GET/PUT /api/ai/tools/policy`）：
///  1. **写 / 读白名单**：默认只能写后端给的产物目录，其它位置一律拒绝；
///  2. **逐工具权限**：allow / ask / deny；
///  3. **预算**：单次回复的工具轮数与调用总数上限。
///
/// 这里**不做本地校验、也不本地兜底判权限**：白名单的解析、`.git` 之类的禁写段
/// 都由后端 `ToolPolicy` 决定，前端只负责把用户的选择发过去并把生效结果展示回来。
class AiToolsSettingsPage extends StatefulWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const AiToolsSettingsPage({super.key, this.api});

  @override
  State<AiToolsSettingsPage> createState() => _AiToolsSettingsPageState();
}

class _AiToolsSettingsPageState extends State<AiToolsSettingsPage> {
  late AiApiClient _api;
  bool _loading = true;
  bool _saving = false;
  String? _error;

  List<AiToolInfo> _tools = const [];
  AiToolPolicy _policy = const AiToolPolicy();

  final _writeInput = TextEditingController();
  final _readInput = TextEditingController();

  /// 联网搜索（Keenable）的可选密钥。输入框**永远是空字符串**：只写不读，
  /// 不留任何回显可能（AIH-012，与 Provider 凭据同一约定）。
  final _searchKey = TextEditingController();
  AiCredentialStatus _searchCred = AiCredentialStatus.none;
  bool _searchBusy = false;

  /// 附件缓存（裁图 / 搜图 / 上传）：只展示 + 手动清"没被引用"的旧图。
  AiAttachmentCacheStats? _cacheStats;
  bool _cacheBusy = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _api = widget.api ?? AiApiClient(context.read<SettingsStore>().baseUrl);
      _reload();
    });
  }

  @override
  void dispose() {
    _writeInput.dispose();
    _readInput.dispose();
    _searchKey.dispose();
    super.dispose();
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final tools = await _api.listTools();
      final policy = await _api.toolPolicy();
      // 密钥状态拿不到不挡这一页：联网搜索出厂免密钥就能用
      AiCredentialStatus cred = AiCredentialStatus.none;
      try {
        cred = await _api.webSearchCredentialStatus();
      } catch (_) {}
      // 缓存统计拿不到也不挡这一页（老后端没有这条端点）
      AiAttachmentCacheStats? stats;
      try {
        stats = await _api.attachmentCacheStats();
      } catch (_) {}
      if (!mounted) return;
      setState(() {
        _tools = tools;
        _policy = policy;
        _searchCred = cred;
        if (stats != null) _cacheStats = stats;
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 所有改动都走这一条：PUT 只发要改的字段，返回生效后的完整策略。
  ///
  /// 返回值给子页（逐工具权限）用 —— 它在自己的路由里，父页 `setState` 不会
  /// 重建它，所以必须拿到"生效后的 tools + policy"自己更新。
  Future<(List<AiToolInfo>, AiToolPolicy)?> _patch(
    Map<String, dynamic> patch,
    String okMessage,
  ) async {
    setState(() => _saving = true);
    try {
      final next = await _api.updateToolPolicy(patch);
      if (!mounted) return null;
      setState(() => _policy = next);
      _snack(okMessage);
      // 生效权限可能被后端复算（例如 deny 的工具），重新拉一遍工具清单
      final tools = await _api.listTools();
      if (mounted) setState(() => _tools = tools);
      return (tools, next);
    } catch (e) {
      if (mounted) _snack('保存失败，$e', error: true);
      return null;
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          content: Text(message),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error
              ? Theme.of(context).colorScheme.errorContainer
              : null,
        ),
      );
  }

  // --- 联网搜索（Keenable）的可选密钥 ---------------------------------------

  /// 只写：值传出去之后前端不再持有，也不回读（AIH-012）。
  Future<void> _saveSearchKey() async {
    final value = _searchKey.text;
    if (value.trim().isEmpty) {
      _snack('密钥留空就是不修改，要清除请点「移除密钥」。');
      return;
    }
    setState(() => _searchBusy = true);
    try {
      final status = await _api.setWebSearchCredential(value);
      _searchKey.clear(); // 立刻丢弃，不在内存里多留一秒
      if (mounted) setState(() => _searchCred = status);
      _snack('密钥已保存（${status.source}）。值不会再被读回。');
    } catch (e) {
      _snack('保存密钥失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _searchBusy = false);
    }
  }

  Future<void> _removeSearchKey() async {
    setState(() => _searchBusy = true);
    try {
      final status = await _api.removeWebSearchCredential();
      if (mounted) setState(() => _searchCred = status);
      _snack('密钥已移除，联网搜索回到免密钥模式。');
    } catch (e) {
      _snack('移除失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _searchBusy = false);
    }
  }

  /// 联网搜索卡：说明 + 可选密钥。**出厂免密钥可用**，这里只是给想提额的用户一个入口。
  Widget _webSearchCard() {
    final theme = Theme.of(context);
    final cred = _searchCred;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('联网搜索（Keenable）', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              'web_search 和 web_fetch 都走这个后端。出厂免密钥就能用，按来源限速，每小时约 1000 次。'
              '两个工具的默认权限都是「询问」，因为查询词会发给搜索服务，每一次都要你点批准。'
              '嫌烦可以在下面改成「允许」。抓取目标只接受公开网址，内网与本机地址会被拒绝。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 10),
            Row(
              children: [
                Icon(
                  Icons.circle,
                  size: 10,
                  color: !cred.configured
                      ? theme.colorScheme.outline
                      : (cred.source == 'env'
                            ? theme.colorScheme.outline
                            : Colors.green),
                ),
                const SizedBox(width: 6),
                Text(
                  cred.source == 'none' ? '未填密钥（免密钥模式）' : cred.label,
                  style: theme.textTheme.bodySmall,
                ),
              ],
            ),
            const SizedBox(height: 8),
            if (cred.source == 'env')
              Text(
                '密钥由环境变量 COMFYHUB_CRED_keenable 提供，是只读的，不能在应用内修改或移除。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              )
            else ...[
              TextField(
                controller: _searchKey,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(
                  labelText: '粘贴 Keenable 密钥（可选，留空表示不修改）',
                  helperText: '只用来提高额度，可在 keenable.ai/console 创建。只粘贴值本身，不要带引号或 NAME=value。',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  FilledButton.icon(
                    onPressed: _searchBusy ? null : _saveSearchKey,
                    icon: const Icon(Icons.key, size: 18),
                    label: const Text('保存密钥'),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton.icon(
                    onPressed: _searchBusy || !cred.configured
                        ? null
                        : _removeSearchKey,
                    icon: const Icon(Icons.delete_outline, size: 18),
                    label: const Text('移除密钥'),
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    // 与其它设置子页同一套骨架（AppBar + 圆形返回键 + 手机端大标题滚动联动）：
    // 以前这一页没有 AppBar，页头直接顶在 body 上，手机上会压到状态栏。
    return SettingsPageScaffold(
      title: 'AI 工具权限',
      actions: [
        IconButton(
          tooltip: '重新加载',
          onPressed: _loading ? null : _reload,
          icon: const Icon(Icons.refresh),
        ),
      ],
      child: _loading
          ? const Center(child: CircularProgressIndicator())
          : Column(
              children: [
                if (_saving) const LinearProgressIndicator(minHeight: 2),
                if (_error != null)
                  Container(
                    width: double.infinity,
                    padding: const EdgeInsets.symmetric(
                      horizontal: 16,
                      vertical: 10,
                    ),
                    color: Theme.of(context).colorScheme.errorContainer,
                    child: Text(
                      '读取工具权限失败，$_error',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ),
                Expanded(
                  // 卡片高度差几倍（默认策略一段、白名单两段、工具清单很长），
                  // 按 AGENTS §6 用 AdaptiveColumns 的瀑布流而不是单列。
                  child: SingleChildScrollView(
                    padding: const EdgeInsets.all(16),
                    child: AdaptiveColumns(
                      sections: [
                        AdaptiveSection(_defaultBanner(), estimatedHeight: 120),
                        AdaptiveSection(_budgetCard(), estimatedHeight: 220),
                        AdaptiveSection(
                          _rootsCard(
                            title: '写白名单',
                            hint: '这些目录里的文件，工具才允许创建 / 覆盖 / 删除。',
                            roots: _policy.writeRoots,
                            controller: _writeInput,
                            onAdd: _addWriteRoot,
                            onRemove: _removeWriteRoot,
                          ),
                          estimatedHeight: 280,
                        ),
                        AdaptiveSection(
                          _rootsCard(
                            title: '读白名单',
                            hint: '这些目录里的文件，工具才允许读取（写白名单内的目录默认可读）。',
                            roots: _policy.readRoots,
                            controller: _readInput,
                            onAdd: _addReadRoot,
                            onRemove: _removeReadRoot,
                          ),
                          estimatedHeight: 280,
                        ),
                        if (_policy.autoReadRoots.isNotEmpty)
                          AdaptiveSection(
                            _autoReadCard(),
                            estimatedHeight: 160,
                          ),
                        AdaptiveSection(_webSearchCard(), estimatedHeight: 320),
                        AdaptiveSection(
                          _attachmentCacheCard(),
                          estimatedHeight: 240,
                        ),
                        AdaptiveSection(
                          _toolsEntryCard(),
                          estimatedHeight: 160,
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
    );
  }

  /// 默认状态要**大声说清楚**：不然用户会以为"没配就是随便写"。
  Widget _defaultBanner() {
    final theme = Theme.of(context);
    return Card(
      color: theme.colorScheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.shield_outlined,
                  size: 18,
                  color: theme.colorScheme.onTertiaryContainer,
                ),
                const SizedBox(width: 6),
                Text(
                  '默认策略',
                  style: theme.textTheme.titleSmall?.copyWith(
                    color: theme.colorScheme.onTertiaryContainer,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              _policy.defaultWriteHint,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onTertiaryContainer,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              '工具调用分允许、询问、拒绝三档。允许会直接执行，询问会在聊天里等你点「批准 / 拒绝」，'
              '拒绝会直接挡掉。需要审批的工具会在回复气泡里出按钮，没人点就按拒绝处理。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onTertiaryContainer,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// 后端**自动发现**的只读目录（用户 bug ④）。
  ///
  /// 只展示、不可编辑：它不是用户配置，而是每次现算的探测结果（本机 ComfyUI 装在哪、
  /// 共享目录在哪）。放在这里是为了回答"为什么我明明没配、AI 却能读我的工作流文件"。
  Widget _autoReadCard() {
    final theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.travel_explore_outlined,
                  size: 18,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(width: 6),
                Text('自动放行（只读）', style: theme.textTheme.titleSmall),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              '这些是本机 ComfyUI 的目录，后端每次现探、自动只读放行，'
              '所以 AI 能直接读你自己的工作流文件（例如 user\\default\\workflows 下的 .json），'
              '不用你手动加白名单。写入仍然只允许上面那些目录，这些目录不在写白名单里。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 8),
            for (final root in _policy.autoReadRoots)
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: SelectableText(
                  root,
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'monospace',
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _budgetCard() {
    final theme = Theme.of(context);
    const stepOptions = [1, 2, 4, 6, 8, 12, 16, 25, 32, 48];
    const callOptions = [4, 8, 16, 24, 32, 48, 64, 100];
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('预算', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              '限制单次回复里模型能自己跑多少步，防止它绕圈子烧钱。'
              '（回答一次 AI 的提问会重置"调用总数"这一窗的额度 —— '
              '你刚做完选择，它就该接着把这件事做完。）',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 10),
            _dropdownRow<int>(
              label: '工具轮数上限',
              value: stepOptions.contains(_policy.maxToolSteps)
                  ? _policy.maxToolSteps
                  : 25,
              options: stepOptions,
              labelOf: (v) => '$v 轮',
              onChanged: (v) => _patch({'maxToolSteps': v}, '工具轮数上限已设为 $v'),
            ),
            const SizedBox(height: 8),
            _dropdownRow<int>(
              label: '调用总数上限',
              value: callOptions.contains(_policy.maxCallsPerRun)
                  ? _policy.maxCallsPerRun
                  : 48,
              options: callOptions,
              labelOf: (v) => '$v 次',
              onChanged: (v) => _patch({'maxCallsPerRun': v}, '调用总数上限已设为 $v'),
            ),
            if (_policy.maxReadBytes > 0 || _policy.maxWriteBytes > 0) ...[
              const SizedBox(height: 10),
              Text(
                // 用户 2026-09-19：「单轮读写上限给得太低了」→ 读 256KB→2MB、写 1MB→4MB，
                // 而且 read_file 现在支持 offset 分段读（没读完会告诉你下一段从哪读）。
                '单次读写上限是读 ${formatSize(_policy.maxReadBytes)}、'
                '写 ${formatSize(_policy.maxWriteBytes)}。后端固定，界面不能改，'
                '读不完可以按 offset 分段读。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _dropdownRow<T>({
    required String label,
    required T value,
    required List<T> options,
    required String Function(T) labelOf,
    required Future<void> Function(T) onChanged,
  }) {
    final theme = Theme.of(context);
    return Row(
      children: [
        Expanded(child: Text(label, style: theme.textTheme.bodyMedium)),
        DropdownButton<T>(
          value: value,
          isDense: true,
          onChanged: _saving
              ? null
              : (v) {
                  if (v != null) onChanged(v);
                },
          items: [
            for (final option in options)
              DropdownMenuItem<T>(value: option, child: Text(labelOf(option))),
          ],
        ),
      ],
    );
  }

  Widget _rootsCard({
    required String title,
    required String hint,
    required List<String> roots,
    required TextEditingController controller,
    required Future<void> Function(String) onAdd,
    required Future<void> Function(String) onRemove,
  }) {
    final theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              hint,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 8),
            if (roots.isEmpty)
              Text('（空）', style: theme.textTheme.bodySmall)
            else
              // 白名单通常只有几条，条数由后端算出来，这里不做懒构建
              for (final root in roots)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  title: Text(root, style: theme.textTheme.bodySmall),
                  trailing: IconButton(
                    tooltip: '移除',
                    icon: const Icon(Icons.remove_circle_outline, size: 16),
                    visualDensity: VisualDensity.compact,
                    constraints: const BoxConstraints(
                      minWidth: 28,
                      minHeight: 28,
                    ),
                    padding: EdgeInsets.zero,
                    onPressed: _saving ? null : () => onRemove(root),
                  ),
                ),
            const SizedBox(height: 6),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: controller,
                    decoration: const InputDecoration(
                      hintText: '要放行的目录（绝对路径）',
                      border: OutlineInputBorder(),
                      isDense: true,
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  onPressed: _saving
                      ? null
                      : () {
                          final text = controller.text.trim();
                          if (text.isEmpty) return;
                          controller.clear();
                          onAdd(text);
                        },
                  child: const Text('添加'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  /// 附件缓存（2026-11-04）：裁图 / 搜图 / 上传都落成附件，这里给出占用与手动清理。
  ///
  /// **只删没被引用的**：消息块里的附件、提问卡里的裁图 / 选项图都算引用，
  /// 删了历史里就是破图。规则的真源在后端，界面不自己判断。
  Widget _attachmentCacheCard() {
    final theme = Theme.of(context);
    final stats = _cacheStats;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.photo_library_outlined,
                  size: 18,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(width: 6),
                Text('参考图 / 截图缓存', style: theme.textTheme.titleSmall),
                const Spacer(),
                if (_cacheBusy)
                  const SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                else
                  IconButton(
                    tooltip: '重新统计',
                    visualDensity: VisualDensity.compact,
                    icon: const Icon(Icons.refresh, size: 17),
                    onPressed: _loadCacheStats,
                  ),
              ],
            ),
            const SizedBox(height: 6),
            if (stats == null)
              Text(
                '还没统计到缓存占用（后端未提供这条端点，或刚才读取失败）。',
                style: theme.textTheme.bodySmall,
              )
            else ...[
              Text(
                '共 ${stats.count} 张 · ${formatSize(stats.bytes)}'
                '（上限 ${formatSize(stats.maxBytes)}）',
                style: theme.textTheme.bodyMedium,
              ),
              const SizedBox(height: 2),
              Text(
                '用户上传 ${formatSize(stats.bytesOf('upload'))} · '
                '设定图裁片 ${formatSize(stats.bytesOf('crop'))} · '
                '网络参考图 ${formatSize(stats.bytesOf('search'))}',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                '被聊天记录或提问卡引用的图不会被删，历史里的配图要能重开。'
                '没被引用的保留 ${stats.graceDays} 天，总量超过上限时从最老的没引用图开始淘汰，'
                '不会动正在等回答的那一轮。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 8),
              Align(
                alignment: Alignment.centerRight,
                child: OutlinedButton.icon(
                  onPressed: _cacheBusy ? null : _clearAttachmentCache,
                  icon: const Icon(Icons.delete_sweep_outlined, size: 17),
                  label: const Text('清理未引用的旧图'),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  Future<void> _loadCacheStats() async {
    try {
      final stats = await _api.attachmentCacheStats();
      if (mounted) setState(() => _cacheStats = stats);
    } catch (e) {
      if (mounted) _snack('读取缓存占用失败，$e', error: true);
    }
  }

  Future<void> _clearAttachmentCache() async {
    setState(() => _cacheBusy = true);
    try {
      final report = await _api.gcAttachmentCache();
      final stats = await _api.attachmentCacheStats();
      if (!mounted) return;
      setState(() {
        _cacheStats = stats;
        _cacheBusy = false;
      });
      _snack(
        report.removed > 0
            ? '已清理 ${report.removed} 张，释放 ${formatSize(report.freedBytes)}'
            : '没有可清理的旧图（被引用的图一律保留）',
      );
    } catch (e) {
      if (!mounted) return;
      setState(() => _cacheBusy = false);
      _snack('清理失败，$e', error: true);
    }
  }

  /// 「逐工具权限」的入口。
  ///
  /// 这一块**不再摊在本页**：工具有一百多个，每个还带两三行说明，展开后整页
  /// 会变成一条几百行的长卷（用户 2026-11-05："这种预测会非常长的卡片，应该
  /// 放进子界面"）。这里只留一行入口，点进去是独立的一页（带搜索）。
  Widget _toolsEntryCard() {
    return Card(
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        leading: const Icon(Icons.tune),
        title: const Text('逐工具权限'),
        subtitle: Text(
          _tools.isEmpty
              ? '后端还没有返回工具清单。'
              : '共 ${_tools.length} 个工具，每个都能单独设成允许、询问或拒绝。',
        ),
        trailing: const Icon(Icons.chevron_right),
        onTap: _tools.isEmpty
            ? null
            : () => Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => _ToolPermissionsPage(
                    tools: _tools,
                    policy: _policy,
                    onPatch: _patch,
                  ),
                ),
              ),
      ),
    );
  }

  Future<void> _addWriteRoot(String root) => _patch({
    'writeRoots': [..._policy.writeRoots, root],
  }, '已加入写目录 $root');

  Future<void> _addReadRoot(String root) => _patch({
    'readRoots': [..._policy.readRoots, root],
  }, '已加入读目录 $root');

  /// 移除一个目录。**新建列表**再发（`_policy` 里的列表可能是 const，而且就地改会
  /// 让"撤销 / 重试"拿到被改过的旧状态）。
  Future<void> _removeWriteRoot(String root) => _patch({
    'writeRoots': [
      for (final r in _policy.writeRoots)
        if (r != root) r,
    ],
  }, '已移除写目录 $root');

  Future<void> _removeReadRoot(String root) => _patch({
    'readRoots': [
      for (final r in _policy.readRoots)
        if (r != root) r,
    ],
  }, '已移除读目录 $root');
}

/// 「逐工具权限」子页。
///
/// 工具清单可能上百条、每条两三行说明 —— 放在 AI 工具权限页里会把那一页
/// 拉成几百行的长卷，所以单独一页，并给一个搜索框。
class _ToolPermissionsPage extends StatefulWidget {
  final List<AiToolInfo> tools;
  final AiToolPolicy policy;

  /// 保存一档权限；返回值是生效后的 (tools, policy)，父页的 `setState` 到不了
  /// 这条路由，得靠它自己更新。
  final Future<(List<AiToolInfo>, AiToolPolicy)?> Function(
    Map<String, dynamic> patch,
    String message,
  )
  onPatch;

  const _ToolPermissionsPage({
    required this.tools,
    required this.policy,
    required this.onPatch,
  });

  @override
  State<_ToolPermissionsPage> createState() => _ToolPermissionsPageState();
}

class _ToolPermissionsPageState extends State<_ToolPermissionsPage> {
  late List<AiToolInfo> _tools = widget.tools;
  late AiToolPolicy _policy = widget.policy;
  bool _saving = false;
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  List<AiToolInfo> get _shown {
    final q = _query.text.trim().toLowerCase();
    if (q.isEmpty) return _tools;
    return [
      for (final t in _tools)
        if (t.name.toLowerCase().contains(q) ||
            t.description.toLowerCase().contains(q))
          t,
    ];
  }

  Future<void> _setAccess(AiToolInfo tool, String value) async {
    setState(() => _saving = true);
    final result = await widget.onPatch({
      'overrides': {..._policy.overrides, tool.name: value},
    }, '${tool.name} 已设为「${AiToolPolicy.accessLabel(value)}」');
    if (!mounted) return;
    setState(() {
      _saving = false;
      if (result != null) {
        _tools = result.$1;
        _policy = result.$2;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final shown = _shown;
    return SettingsPageScaffold(
      title: '逐工具权限',
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: TextField(
              controller: _query,
              onChanged: (_) => setState(() {}),
              decoration: const InputDecoration(
                hintText: '按工具名或说明筛选',
                prefixIcon: Icon(Icons.search),
                isDense: true,
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 10, 20, 6),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                '共 ${_tools.length} 个工具。改动立刻生效，后端每次调用现算，不用重启。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
          ),
          Expanded(
            child: shown.isEmpty
                ? Center(
                    child: Text('没有匹配的工具', style: theme.textTheme.bodyMedium),
                  )
                : ListView.separated(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 32),
                    itemCount: shown.length,
                    // 工具之间给一个小间距，最后一项后面不留空
                    // （用户 2026-11-05："考虑在每项之间给定较小的 sizedbox"）。
                    separatorBuilder: (_, _) => const SizedBox(height: 6),
                    itemBuilder: (context, i) => _row(theme, shown[i]),
                  ),
          ),
        ],
      ),
    );
  }

  Widget _row(ThemeData theme, AiToolInfo tool) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(
          switch (tool.category) {
            'skill' => Icons.auto_awesome_outlined,
            'files' => Icons.folder_outlined,
            'memory' => Icons.push_pin_outlined,
            'layout' => Icons.dashboard_customize_outlined,
            'web' => Icons.travel_explore_outlined,
            'library' => Icons.menu_book_outlined,
            _ => Icons.dns_outlined,
          },
          size: 16,
          color: tool.isDenied
              ? theme.colorScheme.error
              : theme.colorScheme.onSurfaceVariant,
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Flexible(
                    child: Text(
                      tool.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelMedium,
                    ),
                  ),
                  if (tool.mutating) ...[
                    const SizedBox(width: 6),
                    Tooltip(
                      message: '这个工具会写盘或改数据',
                      child: Icon(
                        Icons.edit_outlined,
                        size: 13,
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ],
                  if (tool.overridden) ...[
                    const SizedBox(width: 6),
                    const _Tag('已改'),
                  ],
                ],
              ),
              Text(
                tool.description,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 8),
        DropdownButton<String>(
          value: AiToolPolicy.accessOptions.containsKey(tool.access)
              ? tool.access
              : 'ask',
          isDense: true,
          onChanged: _saving
              ? null
              : (v) {
                  if (v == null) return;
                  _setAccess(tool, v);
                },
          items: [
            for (final entry in AiToolPolicy.accessOptions.entries)
              DropdownMenuItem(value: entry.key, child: Text(entry.value)),
          ],
        ),
      ],
    );
  }
}

class _Tag extends StatelessWidget {
  final String text;
  const _Tag(this.text);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
      decoration: BoxDecoration(
        color: theme.colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text, style: theme.textTheme.labelSmall),
    );
  }
}
