import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../widgets/adaptive_layout.dart';

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
      if (!mounted) return;
      setState(() {
        _tools = tools;
        _policy = policy;
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// 所有改动都走这一条：PUT 只发要改的字段，返回生效后的完整策略。
  Future<void> _patch(Map<String, dynamic> patch, String okMessage) async {
    setState(() => _saving = true);
    try {
      final next = await _api.updateToolPolicy(patch);
      if (!mounted) return;
      setState(() => _policy = next);
      _snack(okMessage);
      // 生效权限可能被后端复算（例如 deny 的工具），重新拉一遍工具清单
      final tools = await _api.listTools();
      if (mounted) setState(() => _tools = tools);
    } catch (e) {
      if (mounted) _snack('保存失败：$e', error: true);
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
          backgroundColor: error ? Theme.of(context).colorScheme.errorContainer : null,
        ),
      );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('AI 工具权限'),
        actions: [
          IconButton(
            tooltip: '重新加载',
            onPressed: _loading ? null : _reload,
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : Column(
              children: [
                if (_saving) const LinearProgressIndicator(minHeight: 2),
                if (_error != null)
                  Container(
                    width: double.infinity,
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                    color: Theme.of(context).colorScheme.errorContainer,
                    child: Text('读取工具权限失败：$_error',
                        style: Theme.of(context).textTheme.bodySmall),
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
                        AdaptiveSection(_toolsCard(), estimatedHeight: 560),
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
                Icon(Icons.shield_outlined, size: 18, color: theme.colorScheme.onTertiaryContainer),
                const SizedBox(width: 6),
                Text('默认策略',
                    style: theme.textTheme.titleSmall
                        ?.copyWith(color: theme.colorScheme.onTertiaryContainer)),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              _policy.defaultWriteHint,
              style: theme.textTheme.bodyMedium
                  ?.copyWith(color: theme.colorScheme.onTertiaryContainer),
            ),
            const SizedBox(height: 4),
            Text(
              '工具调用要看权限：允许 = 直接执行，询问 = 在聊天里等你点「批准 / 拒绝」，拒绝 = 直接挡掉。'
              '需要审批的工具在回复气泡里出按钮；没人点就会按拒绝处理。',
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onTertiaryContainer),
            ),
          ],
        ),
      ),
    );
  }

  Widget _budgetCard() {
    final theme = Theme.of(context);
    const stepOptions = [1, 2, 4, 6, 8, 12, 16, 24];
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
              '限制单次回复里模型能自己跑多少步，防止它绕圈子烧钱。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 10),
            _dropdownRow<int>(
              label: '工具轮数上限',
              value: stepOptions.contains(_policy.maxToolSteps) ? _policy.maxToolSteps : 8,
              options: stepOptions,
              labelOf: (v) => '$v 轮',
              onChanged: (v) => _patch({'maxToolSteps': v}, '工具轮数上限已设为 $v'),
            ),
            const SizedBox(height: 8),
            _dropdownRow<int>(
              label: '调用总数上限',
              value: callOptions.contains(_policy.maxCallsPerRun) ? _policy.maxCallsPerRun : 16,
              options: callOptions,
              labelOf: (v) => '$v 次',
              onChanged: (v) => _patch({'maxCallsPerRun': v}, '调用总数上限已设为 $v'),
            ),
            if (_policy.maxReadBytes > 0 || _policy.maxWriteBytes > 0) ...[
              const SizedBox(height: 10),
              Text(
                '单次读写上限：读 ${_policy.maxReadBytes} 字节 · 写 ${_policy.maxWriteBytes} 字节（后端固定，界面不可改）',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
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
            Text(hint,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline)),
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
                    constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
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

  Widget _toolsCard() {
    final theme = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('逐工具权限', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              '共 ${_tools.length} 个工具。改动立刻生效（后端每次调用现算，不用重启）。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 8),
            if (_tools.isEmpty)
              Text('后端没有返回工具清单', style: theme.textTheme.bodySmall)
            else
              // 工具清单固定十来条，一次建完没有卡顿问题
              for (final tool in _tools)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        switch (tool.category) {
                          'skill' => Icons.auto_awesome_outlined,
                          'files' => Icons.folder_outlined,
                          'memory' => Icons.push_pin_outlined,
                          _ => Icons.dns_outlined,
                        },
                        size: 16,
                        color: tool.isDenied
                            ? theme.colorScheme.error
                            : theme.colorScheme.outline,
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              children: [
                                Flexible(
                                  child: Text(tool.name,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: theme.textTheme.labelMedium),
                                ),
                                if (tool.mutating) ...[
                                  const SizedBox(width: 6),
                                  Tooltip(
                                    message: '这个工具会写盘或改数据',
                                    child: Icon(Icons.edit_outlined,
                                        size: 13, color: theme.colorScheme.error),
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
                              style: theme.textTheme.labelSmall
                                  ?.copyWith(color: theme.colorScheme.outline),
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
                                _patch({
                                  'overrides': {..._policy.overrides, tool.name: v},
                                }, '${tool.name} 已设为「${AiToolPolicy.accessLabel(v)}」');
                              },
                        items: [
                          for (final entry in AiToolPolicy.accessOptions.entries)
                            DropdownMenuItem(value: entry.key, child: Text(entry.value)),
                        ],
                      ),
                    ],
                  ),
                ),
          ],
        ),
      ),
    );
  }

  Future<void> _addWriteRoot(String root) => _patch(
        {
          'writeRoots': [..._policy.writeRoots, root],
        },
        '已加入写目录：$root',
      );

  Future<void> _addReadRoot(String root) => _patch(
        {
          'readRoots': [..._policy.readRoots, root],
        },
        '已加入读目录：$root',
      );

  /// 移除一个目录。**新建列表**再发（`_policy` 里的列表可能是 const，而且就地改会
  /// 让"撤销 / 重试"拿到被改过的旧状态）。
  Future<void> _removeWriteRoot(String root) => _patch(
        {
          'writeRoots': [for (final r in _policy.writeRoots) if (r != root) r],
        },
        '已移除写目录：$root',
      );

  Future<void> _removeReadRoot(String root) => _patch(
        {
          'readRoots': [for (final r in _policy.readRoots) if (r != root) r],
        },
        '已移除读目录：$root',
      );
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
