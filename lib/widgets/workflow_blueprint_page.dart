import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/layout_constants.dart';
import '../core/workflow_blueprint.dart';
import 'markdown_view.dart';
import 'workflow_blueprint_painter.dart';

/// 蓝图取数的两个入口（详情页把 `store.api` 的两个方法包一下传进来）。
///
/// 传闭包而不是 `ApiClient`：页面因此**不依赖网络**，用例可以直接喂一份假蓝图。
/// [diagnose] = false 时只要**结构**（节点 / 连线 / 坐标），不跑接线体检 ——
/// 后端据此跳过 `/object_info`（兆字节级），界面可以先秒开一张图、再补体检。
typedef BlueprintDiagnose = Future<Map<String, dynamic>> Function(
  String raw, {
  bool diagnose,
});
typedef BlueprintSave = Future<SavedWorkflow> Function(
  String raw,
  String? name,
);

/// 打开工作流蓝图（整屏）。
///
/// 从「查看工作流」弹窗点「看蓝图」进入：弹窗**不关闭**，关闭蓝图后回到 JSON 原文，
/// 仍可复制原文并拖回 ComfyUI（当前的核心用途）。
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

  /// 只认**最后一次** `_load` 的结果：改参数会 350ms 防抖后重跑，慢的那次回来不许覆盖快的。
  int _loadSeq = 0;

  /// 同一份原文的蓝图缓存（最多 6 份，5 分钟）：
  /// "关闭蓝图 → 再次打开"是常态，第二次不应重复等待网络与体检。
  static final Map<String, ({WorkflowBlueprint blueprint, DateTime at})>
  _cache = {};
  static const int _cacheLimit = 6;
  static const Duration _cacheTtl = Duration(minutes: 5);

  String? _selectedId;
  bool _editing = false;
  bool _showIssues = false;

  /// 正在**就地编辑**哪一行参数（`widgets_values` 的下标）。null = 未编辑。
  ///
  /// 编辑应在蓝图上直接操作，不在右侧边栏逐项选择。
  int? _editingWidget;

  /// 侧/下方面板是否展开。
  ///
  /// 折叠后画布变宽 —— 此时**不重新适配**：目的通常是查看更多图，
  /// 重置为"适应窗口"会改变视角。需要重新适配时使用工具栏的「适应窗口」。
  bool _panelOpen = true;

  /// 初始视角只适配一次（手动缩放后不得被重置）
  bool _fitted = false;

  /// 画布**自己**的视口尺寸（不是 `_body` 的 `constraints.biggest`）。
  ///
  /// ⚠ 已知问题："打开之后默认跑到右上角"。原因是适配使用**整个 body** 的
  /// 尺寸（含右侧 380 面板与上方横幅），比画布宽，计算出的平移把内容推到右侧。
  /// 现在由 [`_canvasArea`] 的 `LayoutBuilder` 给出真实尺寸。
  Size? _viewport;

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
    final raw = _raw;
    final seq = ++_loadSeq;
    final cached = _cachedBlueprint(raw);
    if (cached != null) {
      // 缓存命中：先绘制，不等待网络。后台仍会复检一次（文件可能在 ComfyUI 中被修改）。
      _applyBlueprint(cached, loading: false);
    } else {
      setState(() {
        _loading = true;
        _error = null;
      });
    }

    // ① 结构快照（diagnose=false）：后端不取 `/object_info`，通常几毫秒返回。
    //    先显示图；② 的体检返回后再补问题清单。
    Future<Map<String, dynamic>>? fast;
    if (cached == null) {
      fast = widget.diagnose(raw, diagnose: false);
    }
    // ② 全量体检：与快照**并发**发出，不要等快照回来再发。
    final full = widget.diagnose(raw, diagnose: true);

    if (fast != null) {
      try {
        final json = await fast;
        if (!mounted || seq != _loadSeq) return;
        // 全量已经先回来时不要再拿快照覆盖它（快照没有体检结论）。
        if (_blueprint == null || _loading) {
          _applyBlueprint(WorkflowBlueprint.fromJson(json), loading: false);
        }
      } catch (_) {
        // 快照失败不单独报错：全量那条路会给出真正的原因。
      }
    }

    try {
      final json = await full;
      if (!mounted || seq != _loadSeq) return;
      final blueprint = WorkflowBlueprint.fromJson(json);
      _rememberBlueprint(raw, blueprint);
      _applyBlueprint(blueprint, loading: false);
    } catch (e) {
      if (!mounted || seq != _loadSeq) return;
      setState(() {
        _loading = false;
        // 已经有快照可看时，别把整页变成错误页；错误留给"重试"与全量失败时展示。
        if (_blueprint == null) _error = '$e';
      });
    }
  }

  /// 把一份蓝图套到页面上。**不重置 `_fitted`**（视角不得被重置）。
  void _applyBlueprint(WorkflowBlueprint blueprint, {required bool loading}) {
    setState(() {
      _loading = loading;
      if (_blueprint == null) _error = null;
      _blueprint = blueprint;
      _layout = BlueprintLayout.of(blueprint);
      if (_selectedId != null && blueprint.nodeById(_selectedId!) == null) {
        _selectedId = null;
      }
    });
  }

  WorkflowBlueprint? _cachedBlueprint(String raw) {
    final hit = _cache[raw];
    if (hit == null) return null;
    if (DateTime.now().difference(hit.at) > _cacheTtl) {
      _cache.remove(raw);
      return null;
    }
    return hit.blueprint;
  }

  static void _rememberBlueprint(String raw, WorkflowBlueprint blueprint) {
    _cache.remove(raw);
    _cache[raw] = (blueprint: blueprint, at: DateTime.now());
    while (_cache.length > _cacheLimit) {
      _cache.remove(_cache.keys.first);
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
      final name =
          '${_slug(base.isEmpty ? 'workflow' : base)}${_dirty ? '-edited' : ''}.json';
      final saved = await save(_raw, name);
      if (!mounted) return;
      setState(() => _saving = false);
      _toast(
        '已存到 ${saved.path}（${(saved.bytes / 1024).toStringAsFixed(0)} KB）—— ${saved.hint}',
      );
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      _toast('保存失败，$e');
    }
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(text),
        duration: const Duration(milliseconds: 3200),
        behavior: SnackBarBehavior.floating,
      ),
    );
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

  /// 画布四周的留白（**画布坐标**，会跟着缩放一起变小/变大）。
  ///
  /// `_canvas` 与 `_fitToView` **必须用同一个数**：适配的平移量是按
  /// "图包围盒 + 这份留白"这个矩形算的，两处各写一个数就会错位。
  static const double _canvasPadding = 40.0;

  /// 把整张图**缩放对齐到窗口正中**。
  ///
  /// ⚠ 平移量必须按**画布坐标**算，不能拿 `bounds.left/top` 当参照系：
  /// `origin = _canvasPadding - bounds.left`，也就是说 `bounds.left` 已经被 `origin`
  /// 抵消过一次了，再减一次 `bounds.left * fit` 等于按"图坐标"平移到一个已经是
  /// "画布坐标"的空间里 —— 表现就是内容整体往右下推。
  /// 已知问题："打开之后默认跑到右上角"。
  void _fitToView(Size viewport) {
    final layout = _layout;
    if (layout == null || layout.bounds.isEmpty) return;
    // 屏幕上的留白（与画布留白是两件事：这个是"别贴边"，那个是画布自己的内边距）
    const margin = 24.0;
    final scaleX = ((viewport.width - margin * 2) / layout.bounds.width)
        .clamp(0.05, 1.0)
        .toDouble();
    final scaleY = ((viewport.height - margin * 2) / layout.bounds.height)
        .clamp(0.05, 1.0)
        .toDouble();
    final fit = scaleX < scaleY ? scaleX : scaleY;
    // 让"图包围盒 + 画布留白"这一块的**中心**落在视口中心
    final dx =
        viewport.width / 2 - (_canvasPadding + layout.bounds.width / 2) * fit;
    final dy =
        viewport.height / 2 - (_canvasPadding + layout.bounds.height / 2) * fit;
    _controller.value = Matrix4.identity()
      ..translateByDouble(dx, dy, 0, 1)
      ..scaleByDouble(fit, fit, 1, 1);
  }

  /// 画布区：**在画布自己的 LayoutBuilder 里**量视口，再交给 [`_canvas`]。
  ///
  /// 两件事只有在这里才做得对：
  /// 1. "适应窗口"要用**画布**尺寸（右/下侧面板、上方横幅都不算画布面积）；
  /// 2. 首帧时 `_layout` 刚算好、尺寸也刚确定，这时适配一次才落在正中。
  Widget _canvasArea() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final size = constraints.biggest;
        if (size.width > 0 && size.height > 0 && size != _viewport) {
          _viewport = size;
          if (!_fitted) {
            _fitted = true;
            WidgetsBinding.instance.addPostFrameCallback((_) {
              // 尺寸在这一帧里又变了就别适配了 —— 等下一次进入这个分支
              if (mounted && _viewport == size) _fitToView(size);
            });
          }
        }
        return ValueListenableBuilder<Matrix4>(
          valueListenable: _controller,
          builder: (context, matrix, _) {
            final palette = BlueprintPalette.of(Theme.of(context).brightness);
            return Stack(
              children: [
                Positioned.fill(child: _canvas(size, matrix, palette)),
                // 就地编辑层：**屏幕坐标**，盖在 `InteractiveViewer` 之上。
                // 放进 `InteractiveViewer` 里面的话控件会跟着缩放糊掉、还会跟拖动抢手势。
                ..._inlineEditor(matrix, size),
              ],
            );
          },
        );
      },
    );
  }

  Widget _canvas(Size viewportSize, Matrix4 matrix, BlueprintPalette palette) {
    final blueprint = _blueprint!;
    final layout = _layout!;
    final width = layout.bounds.width + _canvasPadding * 2;
    final height = layout.bounds.height + _canvasPadding * 2;
    final origin = Offset(
      _canvasPadding - layout.bounds.left,
      _canvasPadding - layout.bounds.top,
    );

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
            onTapUp: (details) =>
                _onCanvasTap(details.localPosition, origin, layout),
            // 跟着缩放重建：**视口剔除与 LOD 都要当前变换**（`InteractiveViewer` 只变换
            // 子节点、不会重建它，所以不监听就永远拿的是首帧那个比例）。
            child: CustomPaint(
              size: Size(width, height),
              painter: BlueprintPainter(
                BlueprintPainterSpec(
                  blueprint: blueprint,
                  layout: layout,
                  origin: origin,
                  viewport: visibleGraphRect(matrix, viewportSize, origin),
                  selectedId: _selectedId,
                  scale: blueprintScaleOf(matrix),
                  // 深浅主题各一套配色 —— 画笔里再写死一套就会重演"深色下压一片近白"
                  palette: palette,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// 画布上的一次点击：先命中节点，**编辑模式下再往下一层命中"参数行"**。
  void _onCanvasTap(Offset local, Offset origin, BlueprintLayout layout) {
    final graph = local - origin;
    final hit = hitTestNode(layout, graph);
    final blueprint = _blueprint;
    var row = _editingWidget;
    if (hit == null || !_editing || blueprint == null) {
      row = null;
    } else {
      final node = blueprint.nodeById(hit);
      final box = layout.boxOf(hit);
      row = (node == null || box == null)
          ? null
          : hitTestWidgetRow(node, box, graph);
    }
    setState(() {
      _selectedId = hit;
      _editingWidget = row;
      if (hit != null) _showIssues = false;
    });
  }

  /// 就地编辑层：把选中的那一行参数投影到屏幕，放一个真控件在那儿。
  ///
  /// 投影用当前 `Matrix4`（画布坐标 → 屏幕），加 `origin` 就是"图坐标 → 画布坐标"。
  List<Widget> _inlineEditor(Matrix4 matrix, Size viewport) {
    final index = _editingWidget;
    final id = _selectedId;
    final layout = _layout;
    final blueprint = _blueprint;
    if (!_editing ||
        index == null ||
        id == null ||
        layout == null ||
        blueprint == null) {
      return const [];
    }
    final node = blueprint.nodeById(id);
    final box = layout.boxOf(id);
    if (node == null || box == null || index >= node.widgets.length) {
      return const [];
    }

    final origin = Offset(
      _canvasPadding - layout.bounds.left,
      _canvasPadding - layout.bounds.top,
    );
    // 编辑器落在**值那一格**上（点击后右侧出现可编辑 textfield），
    // 而不是盖住整个节点。
    final cell = blueprintWidgetValueRect(node, box, index);
    final target = Rect.fromPoints(
      MatrixUtils.transformPoint(matrix, cell.topLeft + origin),
      MatrixUtils.transformPoint(matrix, cell.bottomRight + origin),
    );
    // 只要求"这一格在屏幕上有一定宽度"：编辑器是**固定尺寸的浮层**，
    // 不需要源格有多高 —— 缩小看全局时那一行只有几像素高，那是正常的，
    // 因为浮层是给人打字的。
    if (target.width < 32) return const [];

    final widget = node.widgets[index];
    // 多行文本行的源矩形可能比视口还宽（一段提示词占满整个节点）：
    // 浮层是给人打字的，**不许超出屏幕**，也不小于 150。
    final width = math.min(
      math.max(target.width + 4, 150.0),
      math.max(150.0, viewport.width - 8),
    );
    final maxLeft = viewport.width - width - 4;
    final left = (target.left - 2).clamp(4.0, maxLeft < 4 ? 4.0 : maxLeft);
    return [
      Positioned(
        left: left,
        top: target.top - 2,
        width: width,
        height: 34,
        child: _InlineWidgetEditor(
          key: ValueKey('inline-widget-${node.id}-$index-${widget.value}'),
          label: widget.label,
          value: widget.value,
          onCommit: (next) {
            if (_editor?.setWidgetValue(node.id, index, next) == true) {
              setState(() => _editingWidget = null);
              _afterEdit();
            }
          },
          onCancel: () => setState(() => _editingWidget = null),
        ),
      ),
    ];
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
        title: Text(
          '工作流蓝图 · ${widget.title}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        actions: [
          if (blueprint != null && blueprint.known) ...[
            if (blueprint.issues.isNotEmpty)
              TextButton.icon(
                onPressed: () => setState(() => _showIssues = true),
                icon: Icon(
                  blueprint.errorCount > 0
                      ? Icons.error_outline
                      : Icons.report_problem_outlined,
                  size: 18,
                  color: blueprint.errorCount > 0
                      ? theme.colorScheme.error
                      : null,
                ),
                label: Text(
                  '${blueprint.errorCount} 错 / ${blueprint.warnCount} 警',
                ),
              ),
            IconButton(
              key: const Key('blueprint-panel-toggle'),
              tooltip: _panelOpen ? '收起面板' : '展开面板',
              onPressed: () => setState(() => _panelOpen = !_panelOpen),
              icon: Icon(
                _panelOpen ? Icons.view_sidebar : Icons.view_sidebar_outlined,
              ),
            ),
            IconButton(
              tooltip: '适应窗口',
              onPressed: () {
                final viewport = _viewport;
                if (viewport != null) _fitToView(viewport);
              },
              icon: const Icon(Icons.fit_screen_outlined),
            ),
            if (_editor?.editable == true)
              IconButton(
                tooltip: _editing ? '退出编辑' : '编辑参数与节点状态',
                onPressed: () => setState(() {
                  _editing = !_editing;
                  // 退出编辑就把就地编辑那一行收掉，免得留一个孤儿输入框
                  if (!_editing) _editingWidget = null;
                }),
                icon: Icon(
                  _editing ? Icons.edit_off_outlined : Icons.edit_outlined,
                ),
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
                    ? const SizedBox(
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
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
              Icon(
                Icons.cloud_off_outlined,
                size: 46,
                color: theme.colorScheme.error,
              ),
              const SizedBox(height: 12),
              SelectableText(
                error,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall,
              ),
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
              Icon(
                Icons.help_outline,
                size: 46,
                color: theme.colorScheme.outline,
              ),
              const SizedBox(height: 12),
              Text('这份 JSON 不是认得的工作流', style: theme.textTheme.titleSmall),
              const SizedBox(height: 8),
              Text(
                blueprint.warnings.join('\n'),
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ],
          ),
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = constraints.maxWidth >= kWideLayoutMinWidth;
        final canvas = _canvasArea();
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
                    if (_panelOpen) ...[
                      const VerticalDivider(width: 1),
                      SizedBox(width: 380, child: panel),
                    ],
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
            if (_panelOpen) ...[
              const Divider(height: 1),
              SizedBox(
                height: (constraints.maxHeight * 0.42).clamp(200.0, 420.0),
                child: panel,
              ),
            ],
          ],
        );
      },
    );
  }

  Widget _banner(ThemeData theme, WorkflowBlueprint blueprint) {
    final notes = <String>[
      if (blueprint.autoLayout)
        '这是 API 格式节点图，它没有保存画布坐标，位置是自动排出来的，'
            '与你在 ComfyUI 里看到的那张图不是同一张（它只含真正被执行的节点）',
      if (!blueprint.objectInfo)
        '这次没连上 ComfyUI（拿不到节点定义），只做了结构检查，'
            '判不出"哪个插口是必填""这两根线的类型对不对"',
      if (_editing && _dirty) '改完会自动复检（不落库、不动原文件）。要带走请用「复制」或「保存」',
      if (blueprint.warnings.isNotEmpty) blueprint.warnings.join('\n'),
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
            child: MarkdownText(
              notes.join('\n'),
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
              selectable: false,
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
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
              if (_loading)
                const SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
          child: SegmentedButton<bool>(
            segments: [
              const ButtonSegment(
                value: false,
                label: Text('节点'),
                icon: Icon(Icons.crop_square, size: 16),
              ),
              ButtonSegment(
                value: true,
                label: Text('体检 ${blueprint.issues.length}'),
                icon: const Icon(Icons.fact_check_outlined, size: 16),
              ),
            ],
            selected: {_showIssues},
            onSelectionChanged: (value) =>
                setState(() => _showIssues = value.first),
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
          Icon(
            Icons.touch_app_outlined,
            size: 36,
            color: theme.colorScheme.outline,
          ),
          const SizedBox(height: 10),
          Text(
            '点画布上的任意节点看它的参数、插口与接线问题',
            textAlign: TextAlign.center,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            '滚轮缩放 · 拖动平移',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
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
            Icon(
              Icons.check_circle_outline,
              size: 40,
              color: theme.colorScheme.primary,
            ),
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
                          style: theme.textTheme.labelLarge?.copyWith(
                            color: colour,
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  // ⚠ 后端这几段文案包含 markdown（`` `comfy_load_workflow` ``、`**加粗**`、
                  // 以及本仓库约定的 `##强调##`）。用 `Text` 直接显示会呈现**字面的反引号和井号**
                  // —— 表现为"到处都是 markdown 格式文案，但没有渲染"。
                  MarkdownText(
                    issue.detail,
                    style: theme.textTheme.bodySmall,
                    selectable: false,
                  ),
                  if (issue.fix != null) ...[
                    const SizedBox(height: 6),
                    MarkdownText(
                      '修法是 ${issue.fix}',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.primary,
                      ),
                      selectable: false,
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
                            label: Text(
                              '#$id',
                              style: theme.textTheme.labelSmall,
                            ),
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

  Widget _nodeDetails(
    ThemeData theme,
    WorkflowBlueprint blueprint,
    BlueprintNode node,
  ) {
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
          '${node.mode == kNodeModeNormal
              ? ''
              : node.muted
              ? ' · 静音'
              : ' · 旁路'}',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        if (issues.isNotEmpty) ...[
          const SizedBox(height: 10),
          for (final issue in issues)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              // 同上：这几行也带 markdown，别让反引号与井号原样露出来
              child: MarkdownText(
                '${issue.isError
                    ? '错误'
                    : issue.isWarn
                    ? '警告'
                    : '提示'}，${issue.title}',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: issue.isError
                      ? theme.colorScheme.error
                      : const Color(0xFFF9A825),
                ),
                selectable: false,
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
              if (_editor?.setNodeMode(node.id, value.first) == true) {
                _afterEdit();
              }
            },
            showSelectedIcon: false,
          ),
          const SizedBox(height: 6),
          Text(
            '旁路是数据穿过去但不执行这个节点，静音是它和它的下游都拿不到东西',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        ],
        const SizedBox(height: 14),
        Text('参数（${node.widgets.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        if (node.widgets.isEmpty)
          Text(
            '这个节点没有参数',
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          )
        else
          for (final widget in node.widgets) _widgetRow(theme, node, widget),
        const SizedBox(height: 14),
        Text('输入插口（${node.inputs.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        for (var i = 0; i < node.inputs.length; i++)
          _portRow(theme, blueprint, node, i, node.inputs[i]),
        const SizedBox(height: 14),
        Text('输出（${node.outputs.length}）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        for (var i = 0; i < node.outputs.length; i++)
          _portRow(theme, blueprint, node, i, node.outputs[i]),
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

  /// 一个参数一行，**左侧名称（不变）、右侧值（可变）**。
  ///
  /// 显示形式为：text（不变） | xxx...（可变）；点击后右侧出现可编辑 textfield。
  ///
  /// 不采用 `名称 = 值` 单行布局：长值（一段提示词）会把名称挤掉，
  /// 而"这是什么参数"恰恰是最不该丢的信息。
  ///
  /// ⚠ 类型是从**当前值**推的，不是从 `/object_info` 推的 —— 蓝图里没有控件声明，
  /// 所以这里只保证"改得动"，不假装知道取值范围。
  Widget _widgetRow(
    ThemeData theme,
    BlueprintNode node,
    BlueprintWidget widget,
  ) {
    final value = widget.value;
    final label = Padding(
      padding: const EdgeInsets.only(top: 9, right: 8),
      child: SizedBox(
        width: kBlueprintParamLabelWidth,
        child: Text(
          widget.label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
      ),
    );

    if (!_editing) {
      return Padding(
        padding: const EdgeInsets.only(bottom: 4),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            label,
            Expanded(
              child: SelectableText(
                value?.toString() ?? '—',
                // 一段提示词在面板里只露三行等于看不到内容；多行文本给到 8 行，
                // 再长的仍然靠滚动看（列表本身就能滚）。
                maxLines:
                    value is String &&
                        (value.contains('\n') || value.length > 48)
                    ? 8
                    : 3,
                style: theme.textTheme.bodySmall,
              ),
            ),
          ],
        ),
      );
    }

    if (value is bool) {
      return Padding(
        padding: const EdgeInsets.only(bottom: 2),
        child: Row(
          children: [
            label,
            Expanded(
              child: Align(
                alignment: Alignment.centerLeft,
                child: Switch(
                  value: value,
                  onChanged: (next) {
                    if (_editor?.setWidgetValue(node.id, widget.index, next) ==
                        true) {
                      _afterEdit();
                    }
                  },
                ),
              ),
            ),
          ],
        ),
      );
    }

    final numeric = value is num;
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          label,
          Expanded(
            child: _PanelWidgetField(
              // 换节点 / 换值要重建（`initialValue` 只在首帧生效）
              key: ValueKey('panel-widget-${node.id}-${widget.index}-$value'),
              initial: value?.toString() ?? '',
              numeric: numeric,
              onCommit: (text) {
                final next = coerceWidgetValue(text, value);
                if (_editor?.setWidgetValue(node.id, widget.index, next) ==
                    true) {
                  _afterEdit();
                }
              },
            ),
          ),
        ],
      ),
    );
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
              child: Icon(
                Icons.report_problem_outlined,
                size: 14,
                color: colour,
              ),
            ),
        ],
      ),
    );
  }
}

/// 面板里"参数名"那一列的宽度。
///
/// 与画布上的两列布局保持同一观感（名称窄、值宽）：
/// 显示形式为：text（不变） | xxx...（可变）。
const double kBlueprintParamLabelWidth = 96;

/// 文本框里的字符串 → 原来的类型（数字还是数字，别的原样）。
///
/// ⚠ 类型是从**当前值**推的，不是从 `/object_info` 推的 —— 蓝图里没有控件声明，
/// 所以这里只保证"改得动、改得准"，不假装知道取值范围。
Object? coerceWidgetValue(String text, Object? previous) {
  if (previous is int) return int.tryParse(text.trim()) ?? previous;
  if (previous is double) return double.tryParse(text.trim()) ?? previous;
  return text;
}

/// 面板里的一个参数值输入框。
///
/// 单独做成 StatefulWidget 只为一件事：**它自己持有 `TextEditingController`**，
/// 这样才能在"点到别处"时把当前文本提交上去。
///
/// ⚠ "点到别处"必须提交：多行文本框中回车为换行，`onFieldSubmitted`
/// **不会触发** —— 只挂它时，修改后点击别处即离开，
/// 而**内容未落库且不报错**。
class _PanelWidgetField extends StatefulWidget {
  final String initial;
  final bool numeric;
  final ValueChanged<String> onCommit;

  const _PanelWidgetField({
    super.key,
    required this.initial,
    required this.numeric,
    required this.onCommit,
  });

  @override
  State<_PanelWidgetField> createState() => _PanelWidgetFieldState();
}

class _PanelWidgetFieldState extends State<_PanelWidgetField> {
  late final TextEditingController _controller = TextEditingController(
    text: widget.initial,
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _commit() => widget.onCommit(_controller.text);

  @override
  Widget build(BuildContext context) {
    return TextField(
      // 面板里"参数值"那一格的稳定锚点（用例按它定位；画布上的就地编辑器另有 key）
      key: const Key('blueprint-panel-widget-field'),
      controller: _controller,
      keyboardType: widget.numeric
          ? const TextInputType.numberWithOptions(decimal: true, signed: true)
          : TextInputType.multiline,
      maxLines: widget.numeric ? 1 : 6,
      style: Theme.of(context).textTheme.bodySmall,
      decoration: const InputDecoration(
        isDense: true,
        border: OutlineInputBorder(),
      ),
      onSubmitted: (_) => _commit(),
      onTapOutside: (_) => _commit(),
    );
  }
}

/// 画布上的**就地参数编辑器**：一个贴在那行参数上的小输入框。
///
/// 编辑应在蓝图上直接操作，不在右侧边栏逐项选择。
///
/// 只提供文本框、不按类型分派控件：画布上该行通常只有十几像素高，
/// 无法容纳开关与下拉框；可通过「适应窗口」放大后再编辑。
///
/// ⚠ 它**不能**放进 `InteractiveViewer` 里面：那样控件会跟着缩放一起糊，
/// 内嵌的 `TextField` 还会跟画布的拖动抢手势。这里是由调用方按当前矩阵投影出来的
/// **屏幕坐标**位置（见 `_inlineEditor`）。
class _InlineWidgetEditor extends StatefulWidget {
  final String label;
  final Object? value;
  final ValueChanged<Object?> onCommit;
  final VoidCallback onCancel;

  const _InlineWidgetEditor({
    super.key,
    required this.label,
    required this.value,
    required this.onCommit,
    required this.onCancel,
  });

  @override
  State<_InlineWidgetEditor> createState() => _InlineWidgetEditorState();
}

class _InlineWidgetEditorState extends State<_InlineWidgetEditor> {
  late final TextEditingController _controller = TextEditingController(
    text: widget.value?.toString() ?? '',
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      elevation: 4,
      borderRadius: BorderRadius.circular(6),
      color: theme.colorScheme.surfaceContainerHigh,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 6),
        child: TextField(
          key: const Key('blueprint-inline-widget-field'),
          controller: _controller,
          autofocus: true,
          style: theme.textTheme.bodySmall,
          decoration: InputDecoration(
            labelText: widget.label,
            isDense: true,
            border: InputBorder.none,
            suffixIcon: IconButton(
              tooltip: '取消',
              iconSize: 14,
              visualDensity: VisualDensity.compact,
              onPressed: widget.onCancel,
              icon: const Icon(Icons.close),
            ),
          ),
          onSubmitted: (text) =>
              widget.onCommit(coerceWidgetValue(text, widget.value)),
          // 点到别处 = **提交**（不是取消）：把刚打的字丢掉比"多改一次"更气人
          onTapOutside: (_) => widget.onCommit(
            coerceWidgetValue(_controller.text, widget.value),
          ),
        ),
      ),
    );
  }
}
