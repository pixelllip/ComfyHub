import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../app.dart' show kWideLayoutMinWidth;
import '../core/workflow_blueprint.dart';
import 'workflow_blueprint_painter.dart';

/// 蓝图取数的两个入口（详情页把 `store.api` 的两个方法包一下传进来）。
///
/// 传闭包而不是 `ApiClient`：页面因此**不依赖网络**，用例可以直接喂一份假蓝图。
typedef BlueprintDiagnose = Future<Map<String, dynamic>> Function(String raw);
typedef BlueprintSave = Future<SavedWorkflow> Function(String raw, String? name);

/// 打开工作流蓝图（整屏）。
///
/// 从「查看工作流」弹窗里点「看蓝图」进来：弹窗**不关**，所以关掉蓝图就回到 JSON 原文，
/// 用户随时还能复制原文拖回 ComfyUI（那是当前最核心的用途）。
///
/// 蓝图**不在弹窗里**：弹窗正文被 clamp 在 760×460，而库内真实工作流的坐标包围盒
/// 实测约 9600×2400 —— 塞进去初始缩放只有 8%，没法看（研究文档 §6.4）。
Future<void> showWorkflowBlueprint(
  BuildContext context, {
  required String title,
  required String raw,
  required BlueprintDiagnose diagnose,
  BlueprintSave? save,
}) {
  return Navigator.of(context, rootNavigator: true).push<void>(
    MaterialPageRoute<void>(
      fullscreenDialog: true,
      builder: (_) => WorkflowBlueprintPage(
        title: title,
        raw: raw,
        diagnose: diagnose,
        save: save,
      ),
    ),
  );
}

/// 工作流蓝图页：左边画布、右边（窄屏是下边）节点详情与接线体检。
class WorkflowBlueprintPage extends StatefulWidget {
  final String title;
  final String raw;
  final BlueprintDiagnose diagnose;
  final BlueprintSave? save;

  const WorkflowBlueprintPage({
    super.key,
    required this.title,
    required this.raw,
    required this.diagnose,
    this.save,
  });

  @override
  State<WorkflowBlueprintPage> createState() => _WorkflowBlueprintPageState();
}

class _WorkflowBlueprintPageState extends State<WorkflowBlueprintPage> {
  final TransformationController _controller = TransformationController();

  /// 当前原文（编辑之后就是改过的那一份）
  late String _raw;
  WorkflowEditor? _editor;
  WorkflowBlueprint? _blueprint;
  BlueprintLayout? _layout;

  bool _loading = true;
  String? _error;
  bool _dirty = false;
  bool _saving = false;

  String? _selectedId;
  bool _editing = false;
  bool _showIssues = false;

  /// 初始视角只适配一次（用户自己缩放之后不许被抢回去）
  bool _fitted = false;

  Timer? _debounce;

  @override
  void initState() {
    super.initState();
    _raw = widget.raw;
    _editor = WorkflowEditor.parse(_raw);
    _load();
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _controller.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final json = await widget.diagnose(_raw);
      if (!mounted) return;
      final blueprint = WorkflowBlueprint.fromJson(json);
      setState(() {
        _loading = false;
        _blueprint = blueprint;
        _layout = BlueprintLayout.of(blueprint);
        _fitted = false;
        // 选中的节点没了（比如换了一份图）就取消选中
        if (_selectedId != null && blueprint.nodeById(_selectedId!) == null) {
          _selectedId = null;
        }
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = '$e';
      });
    }
  }

  /// 改完参数**就地复检**：不落库、不改原文件，只把新的结论拿回来。
  void _rediagnose() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 350), () {
      if (mounted) _load();
    });
  }

  /// 改完之后的收尾：**先把原文重新序列化**，再就地复检。
  ///
  /// ⚠ `_raw` 是唯一真源（复制、保存、复检都读它）：忘了这一步的表现是
  /// "界面上改了、复检还是旧结论、复制出来的还是旧的" —— 而**一个错都不报**。
  void _afterEdit() {
    final editor = _editor;
    if (editor != null) _raw = editor.toJson();
    setState(() => _dirty = true);
    _rediagnose();
  }

  // -------------------------------------------------------------------------
  //  工具栏动作
  // -------------------------------------------------------------------------
  Future<void> _copy() async {
    await Clipboard.setData(ClipboardData(text: _raw));
    if (!mounted) return;
    _toast('已复制${_dirty ? '（改过之后的）' : ''}工作流 JSON，可以直接粘回 ComfyUI');
  }

  Future<void> _save() async {
    final save = widget.save;
    if (save == null) return;
    setState(() => _saving = true);
    try {
      final base = (_blueprint?.title?.trim().isNotEmpty ?? false)
          ? _blueprint!.title!.trim()
          : widget.title.trim();
      final name = '${_slug(base.isEmpty ? 'workflow' : base)}${_dirty ? '-edited' : ''}.json';
      final saved = await save(_raw, name);
      if (!mounted) return;
      setState(() => _saving = false);
      _toast('已存到 ${saved.path}（${(saved.bytes / 1024).toStringAsFixed(0)} KB）—— ${saved.hint}');
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      _toast('保存失败：$e');
    }
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(
      content: Text(text),
      duration: const Duration(milliseconds: 3200),
      behavior: SnackBarBehavior.floating,
    ));
  }

  /// 文件名用的短名（中文原样留，只挡掉路径分隔符与控制字符）。
  static String _slug(String text) {
    final cleaned = text
        .replaceAll(RegExp(r'[\\/:*?"<>|\x00-\x1f]'), '-')
        .replaceAll(RegExp(r'\s+'), '-')
        .trim();
    if (cleaned.isEmpty) return 'workflow';
    return cleaned.length <= 48 ? cleaned : cleaned.substring(0, 48);
  }

  // -------------------------------------------------------------------------
  //  画布
  // -------------------------------------------------------------------------
  void _fitToView(Size viewport) {
    final layout = _layout;
    if (layout == null || layout.bounds.isEmpty) return;
    final padding = 24.0;
    final scale = ((viewport.width - padding * 2) / layout.bounds.width)
        .clamp(0.05, 1.0)
        .toDouble();
    final scaleY = ((viewport.height - padding * 2) / layout.bounds.height)
        .clamp(0.05, 1.0)
        .toDouble();
    final fit = scale < scaleY ? scale : scaleY;
    final dx = (viewport.width - layout.bounds.width * fit) / 2 - layout.bounds.left * fit;
    final dy = (viewport.height - layout.bounds.height * fit) / 2 - layout.bounds.top * fit;
    _controller.value = Matrix4.identity()
      ..translateByDouble(dx, dy, 0, 1)
      ..scaleByDouble(fit, fit, 1, 1);
  }

  Widget _canvas(Size viewportSize) {
    final blueprint = _blueprint!;
    final layout = _layout!;
    final padding = 40.0;
    final width = layout.bounds.width + padding * 2;
    final height = layout.bounds.height + padding * 2;
    final origin = Offset(padding - layout.bounds.left, padding - layout.bounds.top);

    return ClipRect(
      child: InteractiveViewer(
        transformationController: _controller,
        constrained: false,
        minScale: 0.02,
        maxScale: 6,
        boundaryMargin: const EdgeInsets.all(4000),
        child: SizedBox(
          width: width,
          height: height,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTapUp: (details) {
              final graph = details.localPosition - origin;
              final hit = hitTestNode(layout, graph);
              setState(() {
                _selectedId = hit;
                if (hit != null) _showIssues = false;
              });
            },
            // 跟着缩放重建：**视口剔除与 LOD 都要当前变换**（`InteractiveViewer` 只变换
            // 子节点、不会重建它，所以不监听就永远拿的是首帧那个比例）。
            child: ValueListenableBuilder<Matrix4>(
              valueListenable: _controller,
              builder: (context, matrix, _) {
                return CustomPaint(
                  size: Size(width, height),
                  painter: BlueprintPainter(BlueprintPainterSpec(
                    blueprint: blueprint,
                    layout: layout,
                    origin: origin,
                    viewport: visibleGraphRect(matrix, viewportSize, origin),
                    selectedId: _selectedId,
                    scale: matrix.getMaxScaleOnAxis(),
                  )),
                );
              },
            ),
          ),
        ),
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  build
  // -------------------------------------------------------------------------
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final blueprint = _blueprint;
    return Scaffold(
      appBar: AppBar(
        title: Text('工作流蓝图 · ${widget.title}', maxLines: 1, overflow: TextOverflow.ellipsis),
        actions: [
          if (blueprint != null && blueprint.known) ...[
            if (blueprint.issues.isNotEmpty)
              TextButton.icon(
                onPressed: () => setState(() => _showIssues = true),
                icon: Icon(
                  blueprint.errorCount > 0 ? Icons.error_outline : Icons.report_problem_outlined,
                  size: 18,
                  color: blueprint.errorCount > 0 ? theme.colorScheme.error : null,
                ),
                label: Text('${blueprint.errorCount} 错 / ${blueprint.warnCount} 警'),
              ),
            if (_editor?.editable == true)
              IconButton(
                tooltip: _editing ? '退出编辑' : '编辑参数与节点状态',
                onPressed: () => setState(() => _editing = !_editing),
                icon: Icon(_editing ? Icons.edit_off_outlined : Icons.edit_outlined),
              ),
            IconButton(
              tooltip: '复制工作流 JSON',
              onPressed: _copy,
              icon: const Icon(Icons.copy_all_outlined),
            ),
            if (widget.save != null)
              IconButton(
                tooltip: '存成文件（拖进 ComfyUI 就能打开）',
                onPressed: _saving ? null : _save,
                icon: _saving
                    ? const SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2))
                    : const Icon(Icons.save_alt_outlined),
              ),
          ],
        ],
      ),
      body: _body(theme),
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading && _blueprint == null) {
      return const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircularProgressIndicator(),
            SizedBox(height: 14),
            Text('正在算蓝图与接线体检…'),
          ],
        ),
      );
    }
    final error = _error;
    if (error != null && _blueprint == null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.cloud_off_outlined, size: 46, color: theme.colorScheme.error),
              const SizedBox(height: 12),
              SelectableText(error, textAlign: TextAlign.center, style: theme.textTheme.bodySmall),
              const SizedBox(height: 16),
              FilledButton.icon(
                onPressed: _load,
                icon: const Icon(Icons.refresh),
                label: const Text('重试'),
              ),
            ],
          ),
        ),
      );
    }
    final blueprint = _blueprint!;
    if (!blueprint.known) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.help_outline, size: 46, color: theme.colorScheme.outline),
              const SizedBox(height: 12),
              Text('这份 JSON 不是认得的工作流', style: theme.textTheme.titleSmall),
              const SizedBox(height: 8),
              Text(
                blueprint.warnings.join('\n'),
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ],
          ),
        ),
      );
    }
    return LayoutBuilder(builder: (context, constraints) {
      final wide = constraints.maxWidth >= kWideLayoutMinWidth;
      if (!_fitted) {
        _fitted = true;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) _fitToView(constraints.biggest);
        });
      }
      final canvas = _canvas(constraints.biggest);
      final panel = _panel(theme, blueprint);
      final banner = _banner(theme, blueprint);
      if (wide) {
        return Column(
          children: [
            banner,
            Expanded(
              child: Row(
                children: [
                  Expanded(child: canvas),
                  const VerticalDivider(width: 1),
                  SizedBox(width: 380, child: panel),
                ],
              ),
            ),
          ],
        );
      }
      return Column(
        children: [
          banner,
          Expanded(child: canvas),
          const Divider(height: 1),
          SizedBox(
            height: (constraints.maxHeight * 0.42).clamp(200.0, 420.0),
            child: panel,
          ),
        ],
      );
    });
  }

  Widget _banner(ThemeData theme, WorkflowBlueprint blueprint) {
    final notes = <String>[
      if (blueprint.autoLayout)
        '这是 API 格式节点图：它没有保存画布坐标，位置是**自动排出来的**，'
            '与你在 ComfyUI 里看到的那张图不是同一张（它只含真正被执行的节点）',
      if (!blueprint.objectInfo)
        '这次没连上 ComfyUI（拿不到节点定义）：只做了结构检查，'
            '判不出"哪个插口是必填""这两根线的类型对不对"',
      if (_editing && _dirty) '改完会自动复检（不落库、不动原文件）；要带走请用「复制」或「保存」',
      if (blueprint.warnings.isNotEmpty) blueprint.warnings.join('；'),
    ];
    if (notes.isEmpty) return const SizedBox.shrink();
    return Container(
      width: double.infinity,
      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.55),
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.info_outline, size: 15, color: theme.colorScheme.outline),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              notes.join('\n'),
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
        ],
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  右侧 / 下侧面板
  // -------------------------------------------------------------------------
  Widget _panel(ThemeData theme, WorkflowBlueprint blueprint) {
    final node = _selectedId == null ? null : blueprint.nodeById(_selectedId!);
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  '${blueprint.isUi ? '界面格式' : 'API 格式'} · ${blueprint.nodes.length} 节点 / '
                  '${blueprint.edges.length} 连线 / ${blueprint.groups.length} 分组'
                  '${_selectedId == null ? '' : ' · 已选 #$_selectedId'}',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ),
              if (_loading)
                const SizedBox(width: 14, height: 14, child: CircularProgressIndicator(strokeWidth: 2)),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
          child: SegmentedButton<bool>(
            segments: [
              const ButtonSegment(value: false, label: Text('节点'), icon: Icon(Icons.crop_square, size: 16)),
              ButtonSegment(
                value: true,
                label: Text('体检 ${blueprint.issues.length}'),
                icon: const Icon(Icons.fact_check_outlined, size: 16),
              ),
            ],
            selected: {_showIssues},
            onSelectionChanged: (value) => setState(() => _showIssues = value.first),
            showSelectedIcon: false,
          ),
        ),
        Expanded(
          child: _showIssues
              ? _issuesList(theme, blueprint)
              : (node == null
                  ? _emptySelection(theme)
                  : _nodeDetails(theme, blueprint, node)),
        ),
      ],
    );
  }

  Widget _emptySelection(ThemeData theme) => Center(
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.touch_app_outlined, size: 36, color: theme.colorScheme.outline),
              const SizedBox(height: 10),
              Text(
                '点画布上的任意节点看它的参数、插口与接线问题',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
              const SizedBox(height: 6),
              Text(
                '滚轮缩放 · 拖动平移',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ],
          ),
        ),
      );

  Widget _issuesList(ThemeData theme, WorkflowBlueprint blueprint) {
    if (blueprint.issues.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.check_circle_outline, size: 40, color: theme.colorScheme.primary),
            const SizedBox(height: 10),
            Text('接线体检没发现问题', style: theme.textTheme.bodyMedium),
          ],
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(10, 4, 10, 16),
      itemCount: blueprint.issues.length,
      itemBuilder: (context, index) {
        final issue = blueprint.issues[index];
        final colour = issue.isError
            ? theme.colorScheme.error
            : issue.isWarn
                ? const Color(0xFFF9A825)
                : theme.colorScheme.primary;
        return Card(
          margin: const EdgeInsets.only(bottom: 8),
          child: InkWell(
            onTap: issue.nodes.isEmpty
                ? null
                : () => setState(() {
                      _selectedId = issue.nodes.first;
                      _showIssues = false;
                    }),
            child: Padding(
              padding: const EdgeInsets.all(10),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(
                        issue.isError
                            ? Icons.error_outline
                            : issue.isWarn
                                ? Icons.warning_amber_outlined
                                : Icons.info_outline,
                        size: 15,
                        color: colour,
                      ),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          issue.title,
                          style: theme.textTheme.labelLarge?.copyWith(color: colour),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  SelectableText(issue.detail, style: theme.textTheme.bodySmall),
                  if (issue.fix != null) ...[
                    const SizedBox(height: 6),
                    Text(
                      '修法：${issue.fix}',
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.primary),
                    ),
                  ],
                  if (issue.nodes.isNotEmpty) ...[
                    const SizedBox(height: 6),
                    Wrap(
                      spacing: 6,
                      runSpacing: 4,
                      children: [
                        for (final id in issue.nodes.take(12))
                          ActionChip(
                            label: Text('#$id', style: theme.textTheme.labelSmall),
                            visualDensity: VisualDensity.compact,
                            onPressed: () => setState(() {
                              _selectedId = id;
                              _showIssues = false;
                            }),
                          ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _nodeDetails(ThemeData theme, WorkflowBlueprint blueprint, BlueprintNode node) {
    final issues = blueprint.issuesForNode(node.id);
    return ListView(
      padding: const EdgeInsets.fromLTRB(12, 4, 12, 20),
      children: [
        Row(
          children: [
            Expanded(
              child: Text(node.displayTitle, style: theme.textTheme.titleSmall),
            ),
            _kindChip(theme, node),
          ],
        ),
        const SizedBox(height: 2),
        SelectableText(
          '${node.className} · 节点 #${node.id}'
          '${node.mode == kNodeModeNormal ? '' : node.muted ? ' · 静音' : ' · 旁路'}',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
        if (issues.isNotEmpty) ...[
          const SizedBox(height: 10),
          for (final issue in issues)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(
                '${issue.isError ? '错误' : issue.isWarn ? '警告' : '提示'}：${issue.title}',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: issue.isError ? theme.colorScheme.error : const Color(0xFFF9A825),
                ),
              ),
            ),
        ],
        if (_editing) ...[
          const SizedBox(height: 12),
          TextFormField(
            // 换节点要重建（`initialValue` 只在首帧生效）
            key: ValueKey('title-${node.id}-${node.title}'),
            initialValue: node.title,
            decoration: const InputDecoration(
              labelText: '标题',
              isDense: true,
              border: OutlineInputBorder(),
            ),
            onFieldSubmitted: (value) {
              if (_editor?.setNodeTitle(node.id, value) == true) _afterEdit();
            },
          ),
          const SizedBox(height: 10),
          SegmentedButton<int>(
            segments: const [
              ButtonSegment(value: kNodeModeNormal, label: Text('正常')),
              ButtonSegment(value: kNodeModeBypassed, label: Text('旁路')),
              ButtonSegment(value: kNodeModeMuted, label: Text('静音')),
            ],
            selected: {node.mode},
            onSelectionChanged: (value) {
              if (_editor?.setNodeMode(node.id, value.first) == true) _afterEdit();
            },
            showSelectedIcon: false,
          ),
          const SizedBox(height: 6),
          Text(
            '旁路 = 数据穿过去但不执行这个节点；静音 = 它和它的下游都拿不到东西',
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
          ),
        ],
        const SizedBox(height: 14),
        Text('参数（${node.widgets.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        if (node.widgets.isEmpty)
          Text('这个节点没有参数', style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline))
        else
          for (final widget in node.widgets) _widgetRow(theme, node, widget),
        const SizedBox(height: 14),
        Text('输入插口（${node.inputs.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        for (var i = 0; i < node.inputs.length; i++) _portRow(theme, blueprint, node, i, node.inputs[i]),
        const SizedBox(height: 14),
        Text('输出（${node.outputs.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        for (var i = 0; i < node.outputs.length; i++) _portRow(theme, blueprint, node, i, node.outputs[i]),
      ],
    );
  }

  Widget _kindChip(ThemeData theme, BlueprintNode node) {
    final label = switch (node.kind) {
      'note' => '注释',
      'reroute' => '转发',
      'variable' => '变量',
      'broadcast' => '广播',
      'display' => '显示',
      'bypasser' => '开关',
      'subgraph' => '组节点',
      'unknown' => '本机没有',
      _ => '节点',
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(4),
      ),
      child: Text(label, style: theme.textTheme.labelSmall),
    );
  }

  /// 一个参数一行。**编辑开关打开时**按值的类型给对应的输入控件：
  /// 布尔给开关、数字给数字框、其余给文本框。
  ///
  /// ⚠ 类型是从**当前值**推的，不是从 `/object_info` 推的 —— 蓝图里没有控件声明，
  /// 所以这里只保证"改得动"，不假装知道取值范围（要精确改参数请用 ComfyUI 或 AI 工具）。
  Widget _widgetRow(ThemeData theme, BlueprintNode node, BlueprintWidget widget) {
    final value = widget.value;
    if (!_editing) {
      return Padding(
        padding: const EdgeInsets.only(bottom: 4),
        child: SelectableText(
          '${widget.label} = ${value ?? '—'}',
          maxLines: 3,
          style: theme.textTheme.bodySmall,
        ),
      );
    }
    if (value is bool) {
      return SwitchListTile(
        dense: true,
        contentPadding: EdgeInsets.zero,
        title: Text(widget.label, style: theme.textTheme.bodySmall),
        value: value,
        onChanged: (next) {
          if (_editor?.setWidgetValue(node.id, widget.index, next) == true) _afterEdit();
        },
      );
    }
    final numeric = value is num;
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: TextFormField(
        initialValue: value?.toString() ?? '',
        keyboardType: numeric
            ? const TextInputType.numberWithOptions(decimal: true, signed: true)
            : TextInputType.multiline,
        maxLines: numeric ? 1 : 3,
        decoration: InputDecoration(
          labelText: widget.label,
          isDense: true,
          border: const OutlineInputBorder(),
        ),
        onFieldSubmitted: (text) {
          final next = _coerce(text, value);
          if (_editor?.setWidgetValue(node.id, widget.index, next) == true) _afterEdit();
        },
      ),
    );
  }

  /// 文本框里的字符串 → 原来的类型（数字还是数字，别的原样）。
  static Object? _coerce(String text, Object? previous) {
    if (previous is int) return int.tryParse(text.trim()) ?? previous;
    if (previous is double) return double.tryParse(text.trim()) ?? previous;
    return text;
  }

  Widget _portRow(
    ThemeData theme,
    WorkflowBlueprint blueprint,
    BlueprintNode node,
    int index,
    BlueprintPort port,
  ) {
    final issues = blueprint.issuesForSlot(node.id, port.name);
    final connected = port.link != null || port.links.isNotEmpty;
    final colour = issues.any((issue) => issue.isError)
        ? theme.colorScheme.error
        : issues.any((issue) => issue.isWarn)
            ? const Color(0xFFF9A825)
            : theme.colorScheme.outline;
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            connected ? Icons.link : Icons.link_off,
            size: 14,
            color: connected ? theme.colorScheme.outline : colour,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              '${port.name.isEmpty ? '(未命名)' : port.name}'
              '${port.type.isEmpty ? '' : ' : ${port.type}'}'
              '${port.widget ? ' · 控件' : ''}'
              '${port.isOpenSocket ? ' · 空着' : ''}',
              style: theme.textTheme.bodySmall?.copyWith(color: colour),
            ),
          ),
          if (issues.isNotEmpty)
            Tooltip(
              message: issues.map((issue) => issue.title).join('\n'),
              child: Icon(Icons.report_problem_outlined, size: 14, color: colour),
            ),
        ],
      ),
    );
  }
}
