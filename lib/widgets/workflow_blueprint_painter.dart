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

  const BlueprintPainterSpec({
    required this.blueprint,
    required this.layout,
    this.origin = Offset.zero,
    this.viewport = Rect.largest,
    this.selectedId,
    this.highlightedNodes = const {},
    this.scale = 1,
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

  static const double _portHeight = 18;
  static const double _headerHeight = 26;
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
  //  分组框
  // -------------------------------------------------------------------------
  void _paintGroups(Canvas canvas, Rect viewport) {
    for (final group in spec.blueprint.groups) {
      if (!group.valid) continue;
      final rect = Rect.fromLTWH(group.left, group.top, group.width, group.height);
      if (!visibleInViewport(rect, viewport)) continue;
      final colour = _parseColor(group.color) ?? const Color(0xFF3F789E);
      final fill = Paint()..color = colour.withValues(alpha: 0.06);
      final border = Paint()
        ..color = colour.withValues(alpha: 0.55)
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

    // 底色按"这是什么节点"分：注释 / 转发 / 广播 / 显示 / 普通
    var body = _bodyColor(node);
    if (dim) body = body.withValues(alpha: alpha);

    final border = switch (severity) {
      'error' => const Color(0xFFE53935),
      'warn' => const Color(0xFFF9A825),
      'info' => const Color(0xFF1E88E5),
      _ => _borderColor(node),
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
        Paint()..color = border.withValues(alpha: dim ? 0.18 : 0.32),
      );
      canvas.restore();
    }

    if (spec.scale > 0.22) {
      _text(
        canvas,
        node.displayTitle,
        Offset(rect.left + 6, rect.top + 5),
        color: dim ? const Color(0xFF6B6B6B) : const Color(0xFF101418),
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
          color: const Color(0xFF5A6673),
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
            ? const Color(0xFFE53935)
            : slotIssues.any((issue) => issue.isWarn)
                ? const Color(0xFFF9A825)
                : port.isOpenSocket
                    ? const Color(0xFFB0BEC5)
                    : const Color(0xFF546E7A);
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
            color: const Color(0xFF37474F),
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
          color: const Color(0xFF37474F),
          size: 9.5,
          maxWidth: rect.width / 2 - 8,
        );
      }
    }
    // 控件（下拉框 / 数字框）：画成一行行的"小格子"，让人一眼看出哪些是参数
    if (spec.scale > 0.45) {
      for (final widget in node.widgets) {
        final y = rect.top + _headerHeight + _portHeight * count + 16 * widget.index + 4;
        if (y > rect.bottom - 10) break;
        final bar = Rect.fromLTWH(rect.left + 6, y, rect.width - 12, 11);
        canvas.drawRRect(
          RRect.fromRectAndRadius(bar, const Radius.circular(3)),
          Paint()..color = const Color(0x14000000),
        );
        _text(
          canvas,
          '${widget.label} = ${_short(widget.value)}',
          Offset(rect.left + 9, y - 1),
          color: const Color(0xFF455A64),
          size: 9,
          maxWidth: rect.width - 18,
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

  Color _bodyColor(BlueprintNode node) => switch (node.kind) {
        'note' => const Color(0xFFFFF8E1),
        'reroute' => const Color(0xFFCFD8DC),
        'variable' => const Color(0xFFE1BEE7),
        'broadcast' => const Color(0xFFFFE0B2),
        'display' => const Color(0xFFB2EBF2),
        'bypasser' => const Color(0xFFD7CCC8),
        'subgraph' => const Color(0xFFC5CAE9),
        'unknown' => const Color(0xFFFFCDD2),
        _ => const Color(0xFFF3F5F7),
      };

  Color _borderColor(BlueprintNode node) => switch (node.kind) {
        'note' => const Color(0xFFFBC02D),
        'broadcast' => const Color(0xFFEF6C00),
        'display' => const Color(0xFF00838F),
        'unknown' => const Color(0xFFC62828),
        _ => const Color(0xFF90A4AE),
      };

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
    var shown = text;
    if (maxWidth != null && maxWidth > 20) {
      // 按估算截断（**不**调用 TextPainter.measure 逐字量：上千节点时那是主要开销）
      final limit = (maxWidth / (size * 0.62)).floor();
      if (shown.runes.length > limit) {
        shown = '${String.fromCharCodes(shown.runes.take(math.max(1, limit - 1)))}…';
      }
    }
    final painter = TextPainter(
      text: TextSpan(
        text: shown,
        style: TextStyle(
          color: color,
          fontSize: size,
          fontWeight: bold ? FontWeight.w600 : FontWeight.normal,
        ),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout();
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
