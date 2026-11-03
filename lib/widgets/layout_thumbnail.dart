import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../models/layout_spec.dart';

/// 框的颜色（画布与缩略图共用一份；按 `kind` 区分）。
Color layoutKindColor(LayoutBoxKind kind) => switch (kind) {
  LayoutBoxKind.object => const Color(0xFF4CA1FF),
  LayoutBoxKind.character => const Color(0xFFFFA24C),
  LayoutBoxKind.text => const Color(0xFF4CD29A),
  LayoutBoxKind.blank => const Color(0xFF9E9E9E),
};

/// 一张布局的缩略图（宽高比跟着 [LayoutSpec] 走）。
///
/// 为什么用 `CustomPainter` 而不是给每个框建 widget：与画布同一条理由
/// （`layout_canvas_page.dart` 里 `_BoxPainter` 的注释）—— 列表里一次要画两三张，
/// 每张七八个框，建 widget 只是徒增子树。
class LayoutThumbnail extends StatelessWidget {
  const LayoutThumbnail({
    super.key,
    required this.spec,
    this.height = 68,
    this.borderRadius = 8,
  });

  final LayoutSpec spec;
  final double height;
  final double borderRadius;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 尺寸按比例算；极端比例（16:9 与 9:16）也不会翘出卡片。
    final ratio = spec.height <= 0 ? 1.0 : spec.width / spec.height;
    final width = (height * ratio).clamp(height * 0.5, height * 1.9).toDouble();
    return ClipRRect(
      borderRadius: BorderRadius.circular(borderRadius),
      child: SizedBox(
        width: width,
        height: height,
        child: CustomPaint(
          painter: LayoutThumbnailPainter(
            spec: spec,
            background: theme.colorScheme.surfaceContainerLowest,
            grid: theme.colorScheme.outlineVariant.withValues(alpha: 0.45),
            text: theme.colorScheme.onSurface,
          ),
        ),
      ),
    );
  }
}

/// 缩略图的画家。框已经由后端夹在 0~1000 内，这里只做乘除。
class LayoutThumbnailPainter extends CustomPainter {
  LayoutThumbnailPainter({
    required this.spec,
    required this.background,
    required this.grid,
    required this.text,
  });

  final LayoutSpec spec;
  final Color background;
  final Color grid;
  final Color text;

  @override
  void paint(Canvas canvas, Size size) {
    final frame = Offset.zero & size;
    canvas.drawRect(frame, Paint()..color = background);

    // 三分线：构图里最常被引用的一根参照
    final third = Paint()
      ..color = grid
      ..strokeWidth = 0.8;
    for (final f in [1 / 3, 2 / 3]) {
      canvas.drawLine(
        Offset(size.width * f, 0),
        Offset(size.width * f, size.height),
        third,
      );
      canvas.drawLine(
        Offset(0, size.height * f),
        Offset(size.width, size.height * f),
        third,
      );
    }

    for (var i = 0; i < spec.regions.length; i++) {
      final region = spec.regions[i];
      final color = layoutKindColor(region.kind);
      final rect = Rect.fromLTWH(
        region.rect.x / 1000 * size.width,
        region.rect.y / 1000 * size.height,
        region.rect.w / 1000 * size.width,
        region.rect.h / 1000 * size.height,
      );
      canvas.drawRect(rect, Paint()..color = color.withValues(alpha: 0.18));
      canvas.drawRect(
        rect,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2
          ..color = color.withValues(alpha: 0.85),
      );
      // 小图里画名字会糊成一团，只标序号；名字在卡片正文里。
      if (rect.width > 18 && rect.height > 18) {
        final label = TextPainter(
          text: TextSpan(
            text: '${i + 1}',
            style: TextStyle(
              fontSize: math.min(10.0, rect.height / 2.4),
              fontWeight: FontWeight.w700,
              color: text.withValues(alpha: 0.75),
            ),
          ),
          textDirection: TextDirection.ltr,
        )..layout();
        label.paint(canvas, rect.topLeft + const Offset(3, 2));
      }
    }
  }

  @override
  bool shouldRepaint(LayoutThumbnailPainter old) =>
      old.spec != spec ||
      old.background != background ||
      old.grid != grid ||
      old.text != text;
}
