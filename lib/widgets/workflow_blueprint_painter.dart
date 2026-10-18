import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../core/workflow_blueprint.dart';

/// 一次绘制实际画了多少东西（**给用例看的**，不是给用户看的）。
///
/// 仓库约定：界面类改动不写 golden / 不断言像素，而是把"画了什么"变成可断言的量
/// （见 `docs/archive/workflow-blueprint-research.md` §5 的 P2 验收）。视口剔除是性能的
/// 唯一防线，所以它必须有回归。
class BlueprintPaintStats {
  /// 这一帧画到的节点数
  final int drawnNodes;
  /// 这一帧跳过的节点数（视口之外）
  final int culledNodes;
  final int drawnEdges;
  final int culledEdges;

  const BlueprintPaintStats({
    this.drawnNodes = 0,
    this.culledNodes = 0,
    this.drawnEdges = 0,
    this.culledEdges = 0,
  });

  @override
  String toString() =>
      'BlueprintPaintStats(drawn=$drawnNodes, culled=$culledNodes, edges=$drawnEdges)';
}

/// WCAG 相对亮度（0 = 黑，1 = 白）。
///
/// 不只给测试用：`BlueprintPalette.isDark` 就靠它决定分组底板的 alpha 该往哪边调。
/// 顺带，调色板"刺不刺眼"因此能**算**出来 —— 仓库不做 golden / 截图回归
/// （`docs/workflow-blueprint-design.md` §6），"观感"只能变成可断言的量。
double relativeLuminance(Color color) {
  double channel(double c) =>
      c <= 0.03928 ? c / 12.92 : math.pow((c + 0.055) / 1.055, 2.4).toDouble();
  return 0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b);
}

/// WCAG 对比度（1:1 ~ 21:1）。
@visibleForTesting
double contrastRatio(Color a, Color b) {
  final la = relativeLuminance(a);
  final lb = relativeLuminance(b);
  final hi = math.max(la, lb);
  final lo = math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

/// 蓝图配色。**两套主题各一份，都得成立**。
///
/// # 为什么要有这个类
///
/// 2026-10-18 用户报："蓝图对比度过高，比较刺眼。" 真因是画布把**一整套浅色**写死在
/// 画笔里（节点底 `#F3F5F7` + 近黑正文），而 App **默认深色主题** ——
/// 深底上压一片近白的大色块，对比度约 17:1，远高于 WCAG AAA 的 7:1。
/// 反过来切浅色主题时，节点底与浅色画布几乎同色、分组底板 `alpha 0.06` 也看不见。
/// 也就是说：那套颜色**只对"它没被设计来适配"的那个主题成立**，两边都不对。
///
/// 现在的规矩两条：
/// 1. **节点底跟画布同一侧**（深色主题下也是深色），只靠**一档明度差 + 一圈描边**分层
///    （AGENTS §6.1：层次用色彩表达，不用 elevation / 阴影）。
/// 2. `kind` 的语义色**只上边框与标题条**，不再整块铺底 —— 这是 ComfyUI 的观感来源，
///    也是"大面积高对比"被消掉的直接原因。
class BlueprintPalette {
  /// 画布底色。**画布必须有自己的底色**，别再借 `Scaffold` 的 surface
  /// （借了就没法保证"节点底与画布底同侧"这件事）。
  final Color canvas;
  /// 网格线（很淡，只帮着看清平移了多少）。
  final Color grid;
  /// 普通节点底色。
  final Color body;
  /// 普通节点描边。
  final Color border;
  /// 节点标题文字。
  final Color title;
  /// 节点类名（次要文字）。
  final Color subtitle;
  /// 插口名文字。
  final Color portLabel;
  /// 参数行的小格子底。
  final Color widgetBar;
  /// 参数行的文字。
  final Color widgetText;
  /// `kind` 的语义强调色（**只用在边框与标题条上**）。
  final Map<String, Color> accents;

  const BlueprintPalette({
    required this.canvas,
    required this.grid,
    required this.body,
    required this.border,
    required this.title,
    required this.subtitle,
    required this.portLabel,
    required this.widgetBar,
    required this.widgetText,
    required this.accents,
  });

  static BlueprintPalette of(Brightness brightness) =>
      brightness == Brightness.dark ? dark : light;

  /// 画布底色是不是深色一侧 —— 用它决定分组底板的 alpha 该往哪边调
  /// （深色下 0.06 的底色等于没有，浅色下 0.3 又会压住内容）。
  bool get isDark => relativeLuminance(canvas) < 0.5;

  /// 这一类节点的强调色（认不出来回普通描边色 —— **不猜**）。
  Color accentOf(String kind) => accents[kind] ?? border;

  /// 深色：一切都在深色一侧，靠 `body` 比 `canvas` 亮一档来分层。
  static const BlueprintPalette dark = BlueprintPalette(
    canvas: Color(0xFF1A1A1E),
    grid: Color(0xFF26262C),
    body: Color(0xFF2A2A31),
    border: Color(0xFF45454F),
    title: Color(0xFFE8E8EC),
    subtitle: Color(0xFF9C9CA8),
    portLabel: Color(0xFFB6B6C2),
    widgetBar: Color(0xFF35353E),
    widgetText: Color(0xFFCACAD4),
    accents: {
      'note': Color(0xFFC9A227),
      'reroute': Color(0xFF7F8C9B),
      'variable': Color(0xFFA56BC4),
      'broadcast': Color(0xFFD08A3E),
      'display': Color(0xFF3FA9B8),
      'bypasser': Color(0xFF9A8577),
      'subgraph': Color(0xFF8A93D6),
      'unknown': Color(0xFFE06C63),
    },
  );

  /// 浅色：同一套规矩反过来 —— 节点底比画布**白**一档。
  static const BlueprintPalette light = BlueprintPalette(
    canvas: Color(0xFFEFEFF3),
    grid: Color(0xFFE3E3E9),
    body: Color(0xFFFFFFFF),
    border: Color(0xFFC6C6D0),
    title: Color(0xFF1B1B1F),
    subtitle: Color(0xFF6C6C78),
    portLabel: Color(0xFF4A4A56),
    widgetBar: Color(0xFFECECF1),
    widgetText: Color(0xFF3A3A45),
    accents: {
      'note': Color(0xFFB58900),
      'reroute': Color(0xFF607D8B),
      'variable': Color(0xFF8E44AD),
      'broadcast': Color(0xFFC1671B),
      'display': Color(0xFF00707C),
      'bypasser': Color(0xFF795548),
      'subgraph': Color(0xFF5C6BC0),
      'unknown': Color(0xFFC62828),
    },
  );
}

/// 节点内的几何常量 —— **只有这一份**：绘制与命中测试都读它。
///
/// 为什么必须共用：一旦绘制和命中各写一套，表现就是"点得到的地方与画出来的地方
/// 不重合"，而且**一个错都不报**（点击落空只会让人以为"这里不能点"）。
const double kBlueprintHeaderHeight = 26;
const double kBlueprintPortHeight = 18;
/// 参数行之间的步距（含小格子本体的高度）。
const double kBlueprintWidgetRowStep = 16;
/// 参数小格子的高度。
const double kBlueprintWidgetBarHeight = 11;

/// 一个**参数行**在画布坐标下的矩形。
///
/// 与 [`BlueprintPainter._paintPorts`] 里画那一行用的是**同一个算式**。
Rect blueprintWidgetRowRect(BlueprintNode node, Rect box, int index) {
  final ports = node.inputs.length > node.outputs.length ? node.inputs.length : node.outputs.length;
  final top = box.top +
      kBlueprintHeaderHeight +
      kBlueprintPortHeight * ports +
      kBlueprintWidgetRowStep * index +
      4;
  return Rect.fromLTWH(box.left + 6, top, box.width - 12, kBlueprintWidgetBarHeight);
}

/// 参数行里"名字"那一格（**固定不变**的那一半）。
///
/// 宽度取行宽的 38%，并夹在 28~96 之间 —— 太窄名字看不出是什么，太宽值没地方了。
double _blueprintLabelWidth(Rect row) => (row.width * 0.38).clamp(28.0, 96.0);

Rect blueprintWidgetLabelRect(BlueprintNode node, Rect box, int index) {
  final row = blueprintWidgetRowRect(node, box, index);
  return Rect.fromLTWH(row.left, row.top, _blueprintLabelWidth(row), row.height);
}

/// 参数行里"值"那一格（**可变**的那一半）。
///
/// 用户 2026-10-18："最起码得显示成：text（不变） | xxx...（可变）的形式，
/// 点击的时候右边出现可修改的 textfield。" —— 就地编辑器就落在这个矩形上。
Rect blueprintWidgetValueRect(BlueprintNode node, Rect box, int index) {
  final row = blueprintWidgetRowRect(node, box, index);
  final label = _blueprintLabelWidth(row);
  return Rect.fromLTWH(row.left + label, row.top, row.width - label, row.height);
}

/// 点击位置（**图坐标**）落在哪一行参数上；不在任何一行上回 `null`。
///
/// 命中区比画出来的小格子**上下各放宽 3px**：小格子只有 11px 高，按精确矩形点
/// 对手感和用例都太苛刻。
int? hitTestWidgetRow(BlueprintNode node, Rect box, Offset point) {
  for (var index = 0; index < node.widgets.length; index++) {
    final rect = blueprintWidgetRowRect(node, box, index).inflate(3);
    // 超出节点底边的行根本不画（见 `_paintPorts` 的 `break`），也就不该被点到
    if (rect.top > box.bottom - 10) break;
    if (rect.contains(point)) return index;
  }
  return null;
}

/// 蓝图绘制器的输入（**刻意与 widget 解耦**：用例可以直接调它，不用 pump 整个页面）。
class BlueprintPainterSpec {
  final WorkflowBlueprint blueprint;
  final BlueprintLayout layout;
  /// 图坐标 → 画布坐标的平移（画布左上角留白 + 包围盒可能为负）
  final Offset origin;
  /// **当前视口**（图坐标）。剔除就靠它 —— 上千节点的图只画看得见的那几个。
  final Rect viewport;
  final String? selectedId;
  final Set<String> highlightedNodes;
  /// 低于这个缩放就不画正文（LOD）：缩到 5% 还逐字排版会让整帧卡住
  final double scale;
  /// 配色（深浅两套之一）。**必须由页面按 `Theme.of(context).brightness` 传进来** ——
  /// 画笔里再写死一套颜色就会重演"深色主题下压一片近白"那个 bug。
  final BlueprintPalette palette;

  const BlueprintPainterSpec({
    required this.blueprint,
    required this.layout,
    this.origin = Offset.zero,
    this.viewport = Rect.largest,
    this.selectedId,
    this.highlightedNodes = const {},
    this.scale = 1,
    this.palette = BlueprintPalette.light,
  });
}

/// 把蓝图**画出来**：分组底板 → 连线 → 节点框 → 选中/问题标记。
///
/// 为什么用 `CustomPainter` 而不是给每个节点建 widget：实测一份真存档可以到
/// 91 个节点 / 7547px 宽的注释框，而上千节点的图用 widget 重建会很贵。
/// 画布只画**视口内**的东西（[BlueprintPaintStats] 钉住这一条）。
class BlueprintPainter extends CustomPainter {
  final BlueprintPainterSpec spec;
  final void Function(BlueprintPaintStats stats)? onPainted;

  BlueprintPainter(this.spec, {this.onPainted});

  static const double _portHeight = kBlueprintPortHeight;
  static const double _headerHeight = kBlueprintHeaderHeight;
  static const double _portRadius = 4;

  @override
  void paint(Canvas canvas, Size size) {
    // 视口是**图坐标**下那一块（由页面按当前变换算好传进来）：
    // 用 `size`（画布整个尺寸 = 整张图）去剔除等于一条都剔不掉。
    final viewport = spec.viewport;
    var drawnNodes = 0;
    var culledNodes = 0;
    var drawnEdges = 0;
    var culledEdges = 0;

    canvas.save();
    canvas.translate(spec.origin.dx, spec.origin.dy);
    // 画布**自己的**底色：铺满整张图（视口只是它的一个窗口）。
    // 别再借 `Scaffold` 的 surface —— 借了就没法保证"节点底与画布底同侧"。
    canvas.drawRect(Offset.zero & size, Paint()..color = spec.palette.canvas);
    _paintGrid(canvas, viewport);
    _paintGroups(canvas, viewport);
    final edges = _paintEdges(canvas, viewport);
    drawnEdges = edges.$1;
    culledEdges = edges.$2;
    final nodes = _paintNodes(canvas, viewport);
    drawnNodes = nodes.$1;
    culledNodes = nodes.$2;
    canvas.restore();

    onPainted?.call(BlueprintPaintStats(
      drawnNodes: drawnNodes,
      culledNodes: culledNodes,
      drawnEdges: drawnEdges,
      culledEdges: culledEdges,
    ));
  }

  // -------------------------------------------------------------------------
  //  画布网格
  // -------------------------------------------------------------------------

  /// 网格步长（图坐标）。
  static const double _gridStep = 48;

  /// 很淡的网格：**只画视口内那一片**，而且线宽按缩放倒推，屏幕上永远是 1px。
  ///
  /// ⚠ 两道护栏缺一不可：`_canvasArea` 在拿不到逆矩阵时会把视口回成 `Rect.largest`
  /// （`visibleGraphRect` 的兜底），那种视口上按步长循环等于死循环。
  void _paintGrid(Canvas canvas, Rect viewport) {
    if (spec.scale < 0.15) return;
    if (!viewport.isFinite || viewport.isEmpty) return;
    if ((viewport.width / _gridStep).ceil() > 200 || (viewport.height / _gridStep).ceil() > 200) {
      return;
    }
    final paint = Paint()
      ..color = spec.palette.grid
      ..strokeWidth = 1 / math.max(spec.scale, 0.05);
    final left = (viewport.left / _gridStep).floorToDouble() * _gridStep;
    final top = (viewport.top / _gridStep).floorToDouble() * _gridStep;
    for (var x = left; x <= viewport.right; x += _gridStep) {
      canvas.drawLine(Offset(x, viewport.top), Offset(x, viewport.bottom), paint);
    }
    for (var y = top; y <= viewport.bottom; y += _gridStep) {
      canvas.drawLine(Offset(viewport.left, y), Offset(viewport.right, y), paint);
    }
  }

  // -------------------------------------------------------------------------
  //  分组框
  // -------------------------------------------------------------------------
  void _paintGroups(Canvas canvas, Rect viewport) {
    for (final group in spec.blueprint.groups) {
      if (!group.valid) continue;
      final rect = Rect.fromLTWH(group.left, group.top, group.width, group.height);
      if (!visibleInViewport(rect, viewport)) continue;
      final colour = _parseColor(group.color) ?? const Color(0xFF3F789E);
      // 深色画布下 alpha 0.06 的底色**等于没画**，所以按明暗各给一档
      final dark = spec.palette.isDark;
      final fill = Paint()..color = colour.withValues(alpha: dark ? 0.12 : 0.07);
      final border = Paint()
        ..color = colour.withValues(alpha: dark ? 0.75 : 0.55)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2;
      final rounded = RRect.fromRectAndRadius(rect, const Radius.circular(8));
      canvas.drawRRect(rounded, fill);
      canvas.drawRRect(rounded, border);
      if (spec.scale > 0.25) {
        _text(
          canvas,
          group.title,
          Offset(rect.left + 10, rect.top + 6),
          color: colour,
          size: 13,
          bold: true,
        );
      }
    }
  }

  // -------------------------------------------------------------------------
  //  连线
  // -------------------------------------------------------------------------
  (int, int) _paintEdges(Canvas canvas, Rect viewport) {
    var drawn = 0;
    var culled = 0;
    for (final edge in spec.blueprint.edges) {
      final from = spec.layout.boxOf(edge.fromNode);
      final to = spec.layout.boxOf(edge.toNode);
      if (from == null || to == null) continue;
      final start = _outputAnchor(from, edge.fromSlot);
      final end = _inputAnchor(to, edge.toSlot);
      // 剔除：整条贝塞尔都在视口外就跳过（用两头的包围盒近似，够用且便宜）
      if (!visibleInViewport(Rect.fromPoints(start, end).inflate(40), viewport)) {
        culled++;
        continue;
      }
      drawn++;
      final severity = _edgeSeverity(edge);
      final colour = severity == 'error'
          ? const Color(0xFFE53935)
          : severity == 'warn'
              ? const Color(0xFFF9A825)
              : const Color(0xFF7A8CA0);
      final paint = Paint()
        ..color = colour.withValues(alpha: severity == 'none' ? 0.75 : 1)
        ..style = PaintingStyle.stroke
        ..strokeWidth = severity == 'none' ? 1.6 : 2.6;
      final path = Path()
        ..moveTo(start.dx, start.dy)
        ..cubicTo(
          start.dx + (end.dx - start.dx).abs().clamp(40, 160),
          start.dy,
          end.dx - (end.dx - start.dx).abs().clamp(40, 160),
          end.dy,
          end.dx,
          end.dy,
        );
      canvas.drawPath(path, paint);
      canvas.drawCircle(end, 2.6, Paint()..color = colour);
    }
    return (drawn, culled);
  }

  /// 这条线自己有没有结论（类型不匹配 / 引用了不存在的节点）。
  String _edgeSeverity(BlueprintEdge edge) {
    for (final issue in spec.blueprint.issues) {
      if (issue.links.isNotEmpty && edge.id != null && issue.links.contains(edge.id)) {
        return issue.severity;
      }
    }
    return 'none';
  }

  // -------------------------------------------------------------------------
  //  节点
  // -------------------------------------------------------------------------
  (int, int) _paintNodes(Canvas canvas, Rect viewport) {
    var drawn = 0;
    var culled = 0;
    for (final node in spec.blueprint.nodes) {
      final rect = spec.layout.boxOf(node.id);
      if (rect == null) continue;
      if (!visibleInViewport(rect, viewport)) {
        culled++;
        continue;
      }
      drawn++;
      _paintNode(canvas, node, rect);
    }
    return (drawn, culled);
  }

  void _paintNode(Canvas canvas, BlueprintNode node, Rect rect) {
    final severity = spec.blueprint.severityForNode(node.id);
    final selected = spec.selectedId == node.id;
    final dim = node.bypassed || node.muted;
    final alpha = dim ? 0.42 : 1.0;

    // 底色**不再按 kind 变**：大面积铺语义色正是"刺眼"的来源。
    // 语义区分改由上色的边框 + 标题条承担（见下面的 `border`）。
    var body = spec.palette.body;
    if (dim) body = body.withValues(alpha: alpha);

    final border = switch (severity) {
      'error' => const Color(0xFFE05252),
      'warn' => const Color(0xFFE0A32E),
      'info' => const Color(0xFF4C9BE8),
      _ => spec.palette.accentOf(node.kind),
    };

    final rounded = RRect.fromRectAndRadius(rect, const Radius.circular(6));
    canvas.drawRRect(rounded, Paint()..color = body);
    canvas.drawRRect(
      rounded,
      Paint()
        ..color = selected ? const Color(0xFF00B0FF) : border.withValues(alpha: dim ? 0.5 : 1)
        ..style = PaintingStyle.stroke
        ..strokeWidth = selected ? 3 : (severity == 'none' ? 1.2 : 2.4),
    );

    // 标题条
    final header = Rect.fromLTWH(rect.left, rect.top, rect.width, _headerHeight);
    if (rect.height > _headerHeight) {
      canvas.save();
      canvas.clipRRect(rounded);
      canvas.drawRect(
        header,
        Paint()..color = border.withValues(alpha: dim ? 0.10 : 0.22),
      );
      canvas.restore();
    }

    if (spec.scale > 0.22) {
      _text(
        canvas,
        node.displayTitle,
        Offset(rect.left + 6, rect.top + 5),
        color: spec.palette.title.withValues(alpha: dim ? 0.45 : 1),
        size: 12,
        bold: true,
        maxWidth: rect.width - 12,
      );
      // 类名（标题被用户改过时它才是"这到底是什么节点"）
      if (spec.scale > 0.5 && node.title.trim().isNotEmpty && node.title != node.className) {
        _text(
          canvas,
          node.className,
          Offset(rect.left + 6, rect.top + _headerHeight - 4),
          color: spec.palette.subtitle.withValues(alpha: dim ? 0.5 : 1),
          size: 10,
          maxWidth: rect.width - 12,
        );
      }
    }

    // 插口：左边输入、右边输出
    if (spec.scale > 0.3) {
      _paintPorts(canvas, node, rect, dim);
    }

    // 角标：旁路 / 静音 / 纯前端
    if (node.bypassed || node.muted || node.frontendOnly) {
      final label = node.muted ? '静音' : (node.bypassed ? '旁路' : '前端');
      _badge(canvas, rect, label, dim ? const Color(0xFF9E9E9E) : const Color(0xFF6D4C41));
    }
    if (severity != 'none') {
      _badge(canvas, rect, severity == 'error' ? '!' : '?', border, right: true);
    }
  }

  void _paintPorts(Canvas canvas, BlueprintNode node, Rect rect, bool dim) {
    final count = math.max(node.inputs.length, node.outputs.length);
    for (var i = 0; i < count; i++) {
      final y = rect.top + _headerHeight + _portHeight * i + _portHeight / 2;
      if (y > rect.bottom) break;
      if (i < node.inputs.length) {
        final port = node.inputs[i];
        final slotIssues = spec.blueprint.issuesForSlot(node.id, port.name);
        final colour = slotIssues.any((issue) => issue.isError)
            ? const Color(0xFFE05252)
            : slotIssues.any((issue) => issue.isWarn)
                ? const Color(0xFFE0A32E)
                // 空着的插口画淡一点（"这里还能接东西"），接上的画实
                : spec.palette.portLabel.withValues(alpha: port.isOpenSocket ? 0.5 : 1);
        canvas.drawCircle(
          Offset(rect.left, y),
          _portRadius,
          Paint()..color = dim ? colour.withValues(alpha: 0.5) : colour,
        );
        if (spec.scale > 0.55) {
          _text(
            canvas,
            port.name,
            Offset(rect.left + 7, y - 6),
            color: spec.palette.portLabel,
            size: 9.5,
            maxWidth: rect.width / 2 - 8,
          );
        }
      }
      if (i < node.outputs.length && spec.scale > 0.55) {
        final port = node.outputs[i];
        _text(
          canvas,
          port.name,
          Offset(rect.right - rect.width / 2, y - 6),
          color: spec.palette.portLabel,
          size: 9.5,
          maxWidth: rect.width / 2 - 8,
        );
      }
    }
    // 控件（下拉框 / 数字框）：画成**名字 | 值**两列，让人一眼看出"这是什么参数、这次是多少"
    if (spec.scale > 0.45) {
      for (final widget in node.widgets) {
        // ⚠ 几何只有一份（`blueprintWidgetRowRect` 一族）：命中测试读的是同一个算式，
        // 所以"点得到"与"画得出"永远重合。
        final bar = blueprintWidgetRowRect(node, rect, widget.index);
        if (bar.top > rect.bottom - 10) break;
        canvas.drawRRect(
          RRect.fromRectAndRadius(bar, const Radius.circular(3)),
          Paint()..color = spec.palette.widgetBar,
        );
        // 名字与值**分两列**：挤成 `名字 = 值` 一行时，长值（一段提示词）会把名字挤掉，
        // 而"这是什么参数"恰恰是最不该丢的信息。
        final label = blueprintWidgetLabelRect(node, rect, widget.index);
        final value = blueprintWidgetValueRect(node, rect, widget.index);
        _text(
          canvas,
          widget.label,
          Offset(label.left + 3, label.top - 1),
          color: spec.palette.subtitle,
          size: 9,
          maxWidth: label.width - 4,
        );
        _text(
          canvas,
          _short(widget.value),
          Offset(value.left + 2, value.top - 1),
          color: spec.palette.widgetText,
          size: 9,
          maxWidth: value.width - 4,
        );
      }
    }
  }

  void _badge(Canvas canvas, Rect rect, String label, Color colour, {bool right = false}) {
    final painter = TextPainter(
      text: TextSpan(
        text: label,
        style: TextStyle(color: Colors.white, fontSize: 9, fontWeight: FontWeight.bold),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    final width = painter.width + 8;
    final origin = right
        ? Offset(rect.right - width - 3, rect.bottom - 15)
        : Offset(rect.left + 3, rect.bottom - 15);
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(origin.dx, origin.dy, width, 13),
        const Radius.circular(3),
      ),
      Paint()..color = colour,
    );
    painter.paint(canvas, Offset(origin.dx + 4, origin.dy + 1));
  }

  // -------------------------------------------------------------------------
  //  锚点与颜色
  // -------------------------------------------------------------------------
  Offset _inputAnchor(Rect box, int slot) =>
      Offset(box.left, box.top + _headerHeight + _portHeight * slot + _portHeight / 2);

  Offset _outputAnchor(Rect box, int slot) =>
      Offset(box.right, box.top + _headerHeight + _portHeight * slot + _portHeight / 2);

  // -------------------------------------------------------------------------
  //  颜色
  // -------------------------------------------------------------------------

  /// 画一行字；超出 `maxWidth` 由 TextPainter 自己省略（**不手算字符数**）。
  void _text(
    Canvas canvas,
    String text,
    Offset at, {
    required Color color,
    required double size,
    bool bold = false,
    double? maxWidth,
  }) {
    if (text.isEmpty) return;
    // ⚠ 截断**交给 TextPainter 自己按 `maxWidth` 排版**，不再手算字符数。
    //
    // 这里原来是 `limit = maxWidth / (size * 0.62)` —— `0.62em` 是拉丁字体的平均字宽，
    // 中文≈1.0em，于是**低估约 40%**：长中文参数会画到框外、压住相邻元素。
    // 而且旧写法最后是 `painter.layout()`（**没传 `maxWidth`**），
    // 所以下面那句 `maxLines: 1, ellipsis: '…'` 其实**永远不会触发**。
    //
    // 担心"逐字量太贵"是多余的：`_paintNodes` 已经做了视口剔除，
    // 要量的只有屏幕里那几十个节点；何况旧实现本来就已经在建 TextPainter 并 layout 了。
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: TextStyle(
          color: color,
          fontSize: size,
          fontWeight: bold ? FontWeight.w600 : FontWeight.normal,
        ),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: maxWidth ?? double.infinity);
    painter.paint(canvas, at);
  }

  static String _short(Object? value) {
    if (value == null) return '—';
    final text = value.toString().replaceAll('\n', ' ');
    return text.length <= 24 ? text : '${text.substring(0, 24)}…';
  }

  static Color? _parseColor(String? raw) {
    if (raw == null) return null;
    var text = raw.trim();
    if (text.startsWith('#')) text = text.substring(1);
    if (text.length == 6) text = 'FF$text';
    if (text.length != 8) return null;
    final value = int.tryParse(text, radix: 16);
    return value == null ? null : Color(value);
  }

  @override
  bool shouldRepaint(covariant BlueprintPainter old) =>
      old.spec.blueprint != spec.blueprint ||
      old.spec.layout != spec.layout ||
      old.spec.origin != spec.origin ||
      old.spec.viewport != spec.viewport ||
      old.spec.selectedId != spec.selectedId ||
      old.spec.scale != spec.scale ||
      // ⚠ 配色必须一起比：漏了的表现是"用户在设置里切深浅主题，蓝图不变色"，
      // 而**一个错都不报**。（`BlueprintPalette.of()` 回的是两个 const 单例，
      // 所以这里是身份比较，不会每帧都判"变了"。）
      old.spec.palette != spec.palette ||
      !_sameSet(old.spec.highlightedNodes, spec.highlightedNodes);

  static bool _sameSet(Set<String> a, Set<String> b) =>
      a.length == b.length && a.every(b.contains);
}

/// 画布上的一次命中（点击位置 → 节点）。
///
/// 单独抽出来是为了可测：用例不必去点像素，直接喂图坐标。
String? hitTestNode(BlueprintLayout layout, Offset point) {
  // 从后往前找（后画的在上面）
  String? hit;
  for (final entry in layout.boxes.entries) {
    if (entry.value.contains(point)) hit = entry.key;
  }
  return hit;
}

/// 视口剔除的判据（用例断言"画得比总数少"时用它算期望）。
///
/// **只有这一份**：画布的三处剔除都调它 —— 各写一遍 `overlaps` 的下场是
/// "某个方向忘了判"，而那只是多画一点、不会报错（性能回归就这么溜进来）。
bool visibleInViewport(Rect box, Rect viewport) => box.overlaps(viewport);

/// 画布当前的缩放倍数（**只取 X / Y 两轴**）。
///
/// ⚠ 别用 `Matrix4.getMaxScaleOnAxis()`：它把 **Z 轴也一起算**，而画布的矩阵是
/// `scale(fit, fit, 1)` —— Z 恒等于 1，于是**只要在缩小（fit < 1）它就永远回 1.0**。
///
/// 后果有两层，都是静默的：
/// 1. **LOD 失效**：`spec.scale > 0.22 / 0.3 / 0.45 / 0.55` 这几道"缩太小就不画正文"
///    在缩小时全部照旧通过 —— 一份 9600px 宽的工作流按 8% 打开时会**逐字画全部文本**，
///    这正是"蓝图加载/打开比 ComfyUI 慢一个量级"的观感来源之一；
/// 2. 线宽 / 网格步长按缩放倒推的地方（`1 / spec.scale`）会算错。
double blueprintScaleOf(Matrix4 transform) {
  final s = transform.storage;
  final sx = math.sqrt(s[0] * s[0] + s[1] * s[1]);
  final sy = math.sqrt(s[4] * s[4] + s[5] * s[5]);
  return math.max(sx, sy);
}

/// 当前视口落在**图坐标**下的矩形。
///
/// `transform` 是 `InteractiveViewer` 的变换（画布坐标），`origin` 是"图坐标 → 画布坐标"
/// 的平移，所以要减回去。拿不到逆矩阵时回 `Rect.largest`（= **不剔除**：
/// 宁可多画一点，也不要因为算不出来就把整张图画成空白）。
Rect visibleGraphRect(Matrix4 transform, Size viewport, Offset origin) {
  final inverse = Matrix4.tryInvert(transform);
  if (inverse == null || viewport.isEmpty) return Rect.largest;
  final a = MatrixUtils.transformPoint(inverse, Offset.zero);
  final b = MatrixUtils.transformPoint(inverse, Offset(viewport.width, viewport.height));
  return Rect.fromPoints(a, b).shift(-origin);
}
