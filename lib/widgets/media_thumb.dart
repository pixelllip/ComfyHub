import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import 'common.dart';

/// 缩略图解码的最大边长（**设备像素**）。
///
/// 网格里的格子最多 240 逻辑像素宽，而后端 `GET /api/media/{id}/thumb` 给的是一张
/// 512px 的 JPEG、生成失败/没有缩略图时 `thumbUrl` 还会退回原图（几 MB、4096px）。
/// 不限制解码尺寸的话，每个格子都要：
///   ① 在 IO 线程把整张位图解码出来（4096² × 4B ≈ 64MB）；
///   ② 往 GPU 上传一张同样大的纹理；
///   ③ 挤爆 ImageCache（默认 100MB / 1000 张），回头滑动只能重新解码。
/// 快速滑动时一帧要新进好几个格子，这三件事叠起来就是"卡顿"。
const int kThumbMaxDecodeEdge = 512;

/// 画廊 / 详情页里的媒体缩略格子。
class MediaThumb extends StatelessWidget {
  final MediaAsset media;
  final VoidCallback onTap;
  final bool selected;
  final bool selectionMode;
  final VoidCallback? onToggleSelect;
  final String? overlayText;

  const MediaThumb({
    super.key,
    required this.media,
    required this.onTap,
    this.selected = false,
    this.selectionMode = false,
    this.onToggleSelect,
    this.overlayText,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.read<LibraryStore>();
    // 图片走 /thumb；**视频走 /poster**（用户 bug ③：视频格子一直没有预览图）。
    //
    // `/api/media/{id}/thumb` 对非图片直接回 204，所以以前视频格子只能显示一个
    // 电影图标占位。后端其实有 `/api/media/{id}/poster`：用 Windows 缩略图管线抽第一帧
    // 并缓存在 `storage/thumbs/<id>.poster.png`（详情页的封面预览一直用的就是它）。
    // 抽不出来（缺解码器）回 204 —— 那时仍退化成占位图标，不是破图。
    final image = media.kind == MediaKind.image
        ? store.api.absolute(media.thumbUrl ?? media.fileUrl)
        : null;
    final thumb = image ?? (media.kind == MediaKind.video ? store.api.mediaPosterUrl(media.id) : null);

    return Material(
      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
      borderRadius: BorderRadius.circular(12),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: selectionMode ? (onToggleSelect ?? onTap) : onTap,
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (thumb != null)
              // 缩略图单独占一层 RepaintBoundary：解码完成 / 加载进度变化时只重绘这一层，
              // 格子上的渐变和带投影的文字不会跟着重新栅格化
              // （文字阴影是每层最贵的一笔，现在每张图只会白付一次）
              RepaintBoundary(child: _networkThumb(context, thumb, theme))
            else
              _placeholder(theme),

            // 顶部渐变，保证角标可读
            Positioned(
              top: 0,
              left: 0,
              right: 0,
              height: 46,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    colors: [Colors.black.withValues(alpha: 0.55), Colors.transparent],
                  ),
                ),
              ),
            ),
            // 底部渐变，放文件名
            Positioned(
              bottom: 0,
              left: 0,
              right: 0,
              height: 56,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [Colors.black.withValues(alpha: 0.72), Colors.transparent],
                  ),
                ),
              ),
            ),

            Positioned(
              top: 6,
              left: 6,
              child: MediaKindBadge(kind: media.kind),
            ),
            if (media.favorite)
              const Positioned(
                top: 6,
                right: 6,
                child: Icon(Icons.star, size: 16, color: Colors.amber),
              ),
            if (media.durationMs != null)
              Positioned(
                bottom: 8,
                right: 8,
                child: Text(
                  formatDuration(media.durationMs),
                  style: const TextStyle(color: Colors.white, fontSize: 11),
                ),
              ),
            Positioned(
              left: 8,
              right: 8,
              bottom: 8,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    overlayText ?? media.title.ifEmpty(media.originalName),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: Colors.white,
                      fontSize: 12.5,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0,
                      // 缩略图底色深浅不定，加个投影保证文字始终可读
                      shadows: [Shadow(blurRadius: 3, color: Colors.black87)],
                    ),
                  ),
                  if (media.hasPrompt && overlayText == null)
                    Text(
                      media.promptTitle ?? '',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: Colors.white.withValues(alpha: 0.9),
                        fontSize: 11.5,
                        fontWeight: FontWeight.w500,
                        letterSpacing: 0,
                        shadows: const [Shadow(blurRadius: 3, color: Colors.black87)],
                      ),
                    ),
                ],
              ),
            ),
            if (media.kind != MediaKind.image)
              Center(
                child: Container(
                  padding: const EdgeInsets.all(10),
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: Colors.black.withValues(alpha: 0.42),
                  ),
                  child: Icon(
                    media.kind == MediaKind.video ? Icons.play_arrow_rounded : Icons.graphic_eq,
                    color: Colors.white,
                    size: 26,
                  ),
                ),
              ),
            if (selectionMode)
              Positioned(
                top: 6,
                left: 6,
                child: Container(
                  width: 22,
                  height: 22,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: selected ? theme.colorScheme.primary : Colors.black45,
                    border: Border.all(color: Colors.white70, width: 1.5),
                  ),
                  child: selected
                      ? const Icon(Icons.check, size: 14, color: Colors.white)
                      : null,
                ),
              ),
          ],
        ),
      ),
    );
  }

  /// 网络缩略图：**按实际绘制出来的像素解码**，而不是按后端那张图的原始分辨率。
  ///
  /// 只给 `cacheWidth`（不给 `cacheHeight`）——引擎会按原比例缩放，非正方形的图不会
  /// 被拉变形；宽度按 `BoxFit.cover` 反推：源图要盖满格子，所以
  /// `解码宽度 = 源图宽 × max(格宽/源图宽, 格高/源图高)`，再乘 devicePixelRatio，
  /// 最后夹到 [kThumbMaxDecodeEdge]。没有原始尺寸信息时退化成"格子的长边"。
  Widget _networkThumb(BuildContext context, String url, ThemeData theme) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final dpr = MediaQuery.devicePixelRatioOf(context);
        return Image.network(
          url,
          fit: BoxFit.cover,
          // 缩略图用不上 mipmap：medium 会为每张纹理额外生成并采样 mipmap，
          // 缩略图这种"小于等于绘制尺寸"的场景 low（双线性）就够了
          filterQuality: FilterQuality.low,
          cacheWidth: _decodeWidth(constraints, dpr),
          loadingBuilder: (context, child, progress) =>
              progress == null ? child : _loadingPlaceholder(),
          errorBuilder: (context, error, stack) => _placeholder(theme),
        );
      },
    );
  }

  /// 需要解码成多少设备像素宽（长宽比由引擎保持）。抽出来是为了能被测试直接钉住。
  int _decodeWidth(BoxConstraints constraints, double dpr) {
    final fallbackEdge = kThumbMaxDecodeEdge.toDouble();
    final cellW = constraints.hasBoundedWidth ? constraints.maxWidth : fallbackEdge;
    final cellH = constraints.hasBoundedHeight ? constraints.maxHeight : fallbackEdge;
    final naturalW = media.width;
    final naturalH = media.height;

    double needed;
    if (naturalW != null && naturalH != null && naturalW > 0 && naturalH > 0) {
      final scale = math.max(cellW / naturalW, cellH / naturalH);
      needed = naturalW * scale;
    } else {
      needed = math.max(cellW, cellH);
    }
    if (!needed.isFinite || needed <= 0) return kThumbMaxDecodeEdge;
    return (needed * dpr).round().clamp(1, kThumbMaxDecodeEdge);
  }

  /// 加载中的静置占位。
  ///
  /// 这里以前放的是 `CircularProgressIndicator`：它内部是一个无限循环的
  /// AnimationController，**每个还在加载的格子都会挂一个 ticker**，
  /// 快速滑动时几十个格子同时在加载 = 每帧几十个 ticker + 几十次图层重绘。
  /// 缩略图走本机后端、通常几十毫秒就好，换个不会动的骨架完全够用
  /// （详情页大图查看器仍保留转圈进度）。
  Widget _loadingPlaceholder() => const Center(
        child: Icon(Icons.image_outlined, size: 30, color: Colors.white24),
      );

  Widget _placeholder(ThemeData theme) {
    if (media.kind == MediaKind.audio) {
      return Container(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            colors: [
              theme.colorScheme.tertiary.withValues(alpha: 0.35),
              theme.colorScheme.primary.withValues(alpha: 0.25),
            ],
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
          ),
        ),
        child: const Center(
          child: Icon(Icons.graphic_eq, size: 40, color: Colors.white70),
        ),
      );
    }
    return Container(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [
            theme.colorScheme.surfaceContainerHighest,
            theme.colorScheme.surfaceContainerHigh,
          ],
        ),
      ),
      child: const Center(child: Icon(Icons.movie_outlined, size: 34, color: Colors.white54)),
    );
  }
}

extension _IfEmpty on String {
  String ifEmpty(String fallback) => isEmpty ? fallback : this;
}
