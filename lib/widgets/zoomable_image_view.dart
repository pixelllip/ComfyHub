import 'dart:math' as math;

import 'package:flutter/material.dart';

/// 缩略图（右下角）与倍数条（左下角）的 key，测试里用来定位
const Key zoomableMinimapKey = Key('zoomable-minimap');
const Key zoomableZoomBarKey = Key('zoomable-zoom-bar');

/// 图片按 `BoxFit.contain` 铺进 [viewport] 之后实际占的矩形（居中）。
///
/// 缩略图上的高亮框、以及"是否已经拖到边界"都靠它换算。拆成纯函数方便单测。
@visibleForTesting
Rect fittedImageRect(Size intrinsic, Size viewport) {
  if (intrinsic.isEmpty || viewport.isEmpty) return Offset.zero & viewport;
  final fitted = applyBoxFit(BoxFit.contain, intrinsic, viewport).destination;
  return Rect.fromLTWH(
    (viewport.width - fitted.width) / 2,
    (viewport.height - fitted.height) / 2,
    fitted.width,
    fitted.height,
  );
}

/// 当前视野落在整张图里的比例（0~1），缩略图用它画高亮框。
@visibleForTesting
Rect visibleImageFraction(Matrix4 transform, Size viewport, Rect imageRect) {
  final inverse = Matrix4.tryInvert(transform);
  if (inverse == null || imageRect.isEmpty) return const Rect.fromLTWH(0, 0, 1, 1);
  final a = MatrixUtils.transformPoint(inverse, Offset.zero);
  final b = MatrixUtils.transformPoint(inverse, Offset(viewport.width, viewport.height));
  final visible = Rect.fromPoints(a, b).intersect(imageRect);
  if (visible.isEmpty) return Rect.zero;
  return Rect.fromLTWH(
    (visible.left - imageRect.left) / imageRect.width,
    (visible.top - imageRect.top) / imageRect.height,
    visible.width / imageRect.width,
    visible.height / imageRect.height,
  );
}

/// 大图查看器。
///
/// 交互约定（和常见的看图工具一致）：
///   · **滚轮**：以鼠标位置为中心放大 / 缩小（`InteractiveViewer` 自带，鼠标滚轮不会被外层滚动视图抢走）；
///   · **按住拖动**：平移画面（只有放大之后才拖得动 —— 适应窗口时画面正好铺满，没有可移动的余量）；
///   · **右下角缩略图**：整张图缩略显示 + 高亮当前视野，一眼看出"现在看的是哪一块"，
///     在缩略图上点 / 拖可以直接跳过去；
///   · 左下角显示当前倍数，放大后出现「适应窗口」按钮，一键回到 1:1。
///
/// 尺寸基准：图片按 `BoxFit.contain` 铺进整个视口，`scale = 1` 就是"整张图都在视野里"，
/// 所以缩放倍数就是相对"适应窗口"的倍数，而不是相对原始像素。
class ZoomableImageView extends StatefulWidget {
  final String url;

  /// 原始像素尺寸。详情页从后端元数据直接拿得到，就不用等图片解码完成才能画缩略图。
  final Size? imageSize;

  /// 测试用：替换掉默认的 `NetworkImage(url)`。
  final ImageProvider? imageProvider;

  final double minScale;
  final double maxScale;
  final Color background;

  const ZoomableImageView({
    super.key,
    required this.url,
    this.imageSize,
    this.imageProvider,
    this.minScale = 1,
    this.maxScale = 8,
    this.background = Colors.black,
  });

  @override
  State<ZoomableImageView> createState() => _ZoomableImageViewState();
}

class _ZoomableImageViewState extends State<ZoomableImageView> {
  final TransformationController _tc = TransformationController();

  ImageProvider? _provider;
  ImageStream? _stream;
  ImageStreamListener? _listener;

  /// 图片原始像素尺寸：优先用入参，其次靠解码回调补上（入参缺失时才挂监听）
  Size? _intrinsic;
  Size _viewport = Size.zero;
  bool _dragging = false;

  @override
  void initState() {
    super.initState();
    _intrinsic = _validSize(widget.imageSize);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _provider ??= widget.imageProvider ?? NetworkImage(widget.url);
    if (_intrinsic == null) _listenToImageStream();
  }

  @override
  void didUpdateWidget(covariant ZoomableImageView oldWidget) {
    super.didUpdateWidget(oldWidget);
    final changed = oldWidget.url != widget.url || oldWidget.imageProvider != widget.imageProvider;
    if (!changed) return;
    _detachStream();
    _provider = widget.imageProvider ?? NetworkImage(widget.url);
    _intrinsic = _validSize(widget.imageSize);
    if (_intrinsic == null) _listenToImageStream();
    // 换图时可能正处在 build 阶段，直接改 notifier 会把已经建好的监听者标脏，挪到帧后
    _afterFrame(_reset);
  }

  @override
  void dispose() {
    _detachStream();
    _tc.dispose();
    super.dispose();
  }

  static Size? _validSize(Size? size) =>
      (size != null && size.width > 0 && size.height > 0) ? size : null;

  // -------------------------------------------------------------------------
  //  图片尺寸
  // -------------------------------------------------------------------------

  void _listenToImageStream() {
    final provider = _provider;
    if (provider == null) return;
    final stream = provider.resolve(createLocalImageConfiguration(context));
    if (stream.key == _stream?.key) return;
    _detachStream();
    final listener = ImageStreamListener(
      (info, _) {
        if (!mounted) return;
        setState(() {
          _intrinsic = Size(info.image.width.toDouble(), info.image.height.toDouble());
        });
      },
      onError: (_, _) {},
    );
    stream.addListener(listener);
    _stream = stream;
    _listener = listener;
  }

  void _detachStream() {
    final stream = _stream;
    final listener = _listener;
    if (stream != null && listener != null) stream.removeListener(listener);
    _stream = null;
    _listener = null;
  }

  // -------------------------------------------------------------------------
  //  几何计算
  // -------------------------------------------------------------------------

  /// 图片在"子坐标系"（= 视口大小的那个盒子）里实际占的矩形：居中 + contain
  Rect _imageRect(Size viewport) {
    final intrinsic = _intrinsic;
    if (intrinsic == null) return Offset.zero & viewport;
    return fittedImageRect(intrinsic, viewport);
  }

  double get _scale => _tc.value.getMaxScaleOnAxis();

  /// 把变换限制在"画面始终铺满视口"的范围内（子盒子大小 == 视口大小）
  Matrix4 _clampedTransform(double scale, Offset translation, Size viewport) {
    final tx = translation.dx.clamp(viewport.width * (1 - scale), 0.0);
    final ty = translation.dy.clamp(viewport.height * (1 - scale), 0.0);
    return Matrix4.identity()
      ..translateByDouble(tx, ty, 0, 1)
      ..scaleByDouble(scale, scale, 1, 1);
  }

  void _reset() {
    if (_tc.value == Matrix4.identity()) return;
    _tc.value = Matrix4.identity();
  }

  /// 帧后再执行（build / layout 期间不能改 transformationController）
  void _afterFrame(VoidCallback action) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) action();
    });
  }

  /// 缩略图上点 / 拖：把那个位置挪到视口中心
  void _seekTo(Offset fraction, Rect imageRect) {
    final viewport = _viewport;
    if (viewport.isEmpty) return;
    final scale = _scale;
    if (scale <= widget.minScale) return;
    final target = Offset(
      imageRect.left + fraction.dx.clamp(0.0, 1.0) * imageRect.width,
      imageRect.top + fraction.dy.clamp(0.0, 1.0) * imageRect.height,
    );
    final desired = Offset(
      viewport.width / 2 - scale * target.dx,
      viewport.height / 2 - scale * target.dy,
    );
    _tc.value = _clampedTransform(scale, desired, viewport);
  }

  void _zoomBy(double factor) {
    final viewport = _viewport;
    if (viewport.isEmpty) return;
    final scale = _scale;
    final target = (scale * factor).clamp(widget.minScale, widget.maxScale);
    if (target == scale) return;
    final center = Offset(viewport.width / 2, viewport.height / 2);
    final sceneCenter = MatrixUtils.transformPoint(Matrix4.tryInvert(_tc.value)!, center);
    final desired = Offset(
      center.dx - target * sceneCenter.dx,
      center.dy - target * sceneCenter.dy,
    );
    _tc.value = _clampedTransform(target, desired, viewport);
  }

  // -------------------------------------------------------------------------
  //  构建
  // -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    return Container(
      color: widget.background,
      child: LayoutBuilder(
        builder: (context, constraints) {
          final width = constraints.maxWidth;
          // 高度无界时（例如塞进 ListView 且没给高度）退一个 3:2 的默认高度，别让布局崩掉
          final height = constraints.hasBoundedHeight ? constraints.maxHeight : width * 2 / 3;
          final viewport = Size(width, height);
          if (viewport != _viewport) {
            // 视口变了（窗口缩放）：缩放系数保留，但平移要重新夹回合法范围。
            // 这里还在 layout 阶段，改 notifier 会踩"build 期间标脏"，所以挪到帧后。
            _viewport = viewport;
            if (_scale > widget.minScale) {
              _afterFrame(() {
                final t = _tc.value.getTranslation();
                _tc.value = _clampedTransform(_scale, Offset(t.x, t.y), viewport);
              });
            }
          }
          final imageRect = _imageRect(viewport);

          return ClipRect(
            child: Stack(
              fit: StackFit.expand,
              children: [
                MouseRegion(
                  cursor: _dragging ? SystemMouseCursors.grabbing : SystemMouseCursors.grab,
                  child: InteractiveViewer(
                    transformationController: _tc,
                    minScale: widget.minScale,
                    maxScale: widget.maxScale,
                    // 画面永远铺满视口，所以零边距就是"不能拖出边界"
                    boundaryMargin: EdgeInsets.zero,
                    onInteractionStart: (_) => setState(() => _dragging = true),
                    onInteractionEnd: (_) => setState(() => _dragging = false),
                    child: SizedBox.fromSize(
                      size: viewport,
                      child: _image(viewport),
                    ),
                  ),
                ),
                ValueListenableBuilder<Matrix4>(
                  valueListenable: _tc,
                  builder: (context, matrix, _) {
                    final zoomed = matrix.getMaxScaleOnAxis() > widget.minScale + 0.001;
                    return Stack(
                      fit: StackFit.expand,
                      children: [
                        if (zoomed)
                          Positioned(
                            right: 12,
                            bottom: 12,
                            child: _Minimap(
                              key: zoomableMinimapKey,
                              provider: _provider,
                              imageRect: imageRect,
                              fraction: visibleImageFraction(matrix, viewport, imageRect),
                              intrinsic: _intrinsic,
                              onSeek: (f) => _seekTo(f, imageRect),
                            ),
                          ),
                        Positioned(
                          left: 12,
                          bottom: 12,
                          child: _ZoomBar(
                            key: zoomableZoomBarKey,
                            scale: matrix.getMaxScaleOnAxis(),
                            zoomed: zoomed,
                            onZoomIn: () => _zoomBy(1.25),
                            onZoomOut: () => _zoomBy(1 / 1.25),
                            onReset: _reset,
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ],
            ),
          );
        },
      ),
    );
  }

  Widget _image(Size viewport) {
    final provider = _provider;
    if (provider == null) return const SizedBox.shrink();
    return Image(
      image: provider,
      fit: BoxFit.contain,
      width: viewport.width,
      height: viewport.height,
      gaplessPlayback: true,
      loadingBuilder: (context, child, progress) => progress == null
          ? child
          : Center(
              child: CircularProgressIndicator(
                value: progress.expectedTotalBytes == null
                    ? null
                    : progress.cumulativeBytesLoaded / progress.expectedTotalBytes!,
              ),
            ),
      errorBuilder: (context, e, s) => const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.broken_image_outlined, size: 42, color: Colors.white54),
            SizedBox(height: 8),
            Text('图片加载失败', style: TextStyle(color: Colors.white70)),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  右下角缩略图
// ---------------------------------------------------------------------------

/// 缩略图 + 当前视野高亮框；点 / 拖可以跳到对应位置。
class _Minimap extends StatelessWidget {
  final ImageProvider? provider;
  final Rect imageRect;
  final Rect fraction;
  final Size? intrinsic;
  final ValueChanged<Offset> onSeek;

  const _Minimap({
    super.key,
    required this.provider,
    required this.imageRect,
    required this.fraction,
    required this.intrinsic,
    required this.onSeek,
  });

  static const double _maxWidth = 176;
  static const double _maxHeight = 132;

  @override
  Widget build(BuildContext context) {
    final size = _minimapSize();
    return Tooltip(
      message: '缩略图：亮框是当前看到的位置，点一下可以跳过去',
      child: Container(
        width: size.width,
        height: size.height,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: Colors.black.withValues(alpha: 0.55),
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: Colors.white24),
          boxShadow: const [BoxShadow(color: Colors.black54, blurRadius: 12)],
        ),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: (d) => onSeek(_toImageFraction(d.localPosition, size)),
          onPanUpdate: (d) => onSeek(_toImageFraction(d.localPosition, size)),
          child: Stack(
            fit: StackFit.expand,
            children: [
              if (provider != null)
                Image(image: provider!, fit: BoxFit.contain, gaplessPlayback: true),
              CustomPaint(painter: _MinimapPainter(fraction: fraction)),
            ],
          ),
        ),
      ),
    );
  }

  Size _minimapSize() {
    final source = intrinsic;
    if (source == null) return const Size(_maxWidth, _maxHeight);
    final fitted = applyBoxFit(
      BoxFit.contain,
      source,
      const Size(_maxWidth, _maxHeight),
    ).destination;
    return Size(math.max(fitted.width, 48), math.max(fitted.height, 48));
  }

  /// 缩略图里的坐标 → 整张图的比例坐标
  Offset _toImageFraction(Offset local, Size minimap) {
    final shown = _imageBoxIn(minimap);
    if (shown.isEmpty) return Offset.zero;
    return Offset(
      ((local.dx - shown.left) / shown.width).clamp(0.0, 1.0),
      ((local.dy - shown.top) / shown.height).clamp(0.0, 1.0),
    );
  }

  /// 缩略图里图片实际占的那块（contain 之后居中）
  Rect _imageBoxIn(Size minimap) {
    final source = intrinsic;
    if (source == null) return Offset.zero & minimap;
    final fitted = applyBoxFit(BoxFit.contain, source, minimap).destination;
    return Rect.fromLTWH(
      (minimap.width - fitted.width) / 2,
      (minimap.height - fitted.height) / 2,
      fitted.width,
      fitted.height,
    );
  }
}

class _MinimapPainter extends CustomPainter {
  final Rect fraction;

  const _MinimapPainter({required this.fraction});

  @override
  void paint(Canvas canvas, Size size) {
    if (fraction.isEmpty) return;
    final box = Rect.fromLTWH(
      fraction.left * size.width,
      fraction.top * size.height,
      fraction.width * size.width,
      fraction.height * size.height,
    );
    // 视野之外压暗，视野之内留亮
    final outside = Path.combine(
      PathOperation.difference,
      Path()..addRect(Offset.zero & size),
      Path()..addRect(box),
    );
    canvas.drawPath(outside, Paint()..color = Colors.black.withValues(alpha: 0.45));
    canvas.drawRect(
      box,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.6
        ..color = Colors.white,
    );
  }

  @override
  bool shouldRepaint(covariant _MinimapPainter oldDelegate) => oldDelegate.fraction != fraction;
}

// ---------------------------------------------------------------------------
//  左下角倍数 / 缩放按钮
// ---------------------------------------------------------------------------

class _ZoomBar extends StatelessWidget {
  final double scale;
  final bool zoomed;
  final VoidCallback onZoomIn;
  final VoidCallback onZoomOut;
  final VoidCallback onReset;

  const _ZoomBar({
    super.key,
    required this.scale,
    required this.zoomed,
    required this.onZoomIn,
    required this.onZoomOut,
    required this.onReset,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(20),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _button(Icons.remove, '缩小', onZoomOut),
          SizedBox(
            width: 52,
            child: Text(
              '${(scale * 100).round()}%',
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: Colors.white,
                fontSize: 12,
                fontWeight: FontWeight.w600,
                letterSpacing: 0,
              ),
            ),
          ),
          _button(Icons.add, '放大', onZoomIn),
          if (zoomed) _button(Icons.fit_screen_outlined, '适应窗口', onReset),
        ],
      ),
    );
  }

  Widget _button(IconData icon, String tooltip, VoidCallback onTap) => IconButton(
        tooltip: tooltip,
        onPressed: onTap,
        icon: Icon(icon, size: 16, color: Colors.white),
        visualDensity: VisualDensity.compact,
        padding: EdgeInsets.zero,
        constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
      );
}
