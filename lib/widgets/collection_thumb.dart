import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/models.dart';
import '../state/library_store.dart';
import 'common.dart';
import 'media_thumb.dart' show CachedThumbImage, kThumbMaxDecodeEdge;
import 'nsfw_blur.dart';

/// 画廊里的**集合格子**：同一批微调的产物叠成一张，右下角标总数。
///
/// 形状就是用户说的 `口」」` —— 最上面那张（集合里**最早**的一张，也是列表的代表）
/// 占满格子，后面几张从右侧与下侧各露出一条边。
///
/// # 三条别踩
///
/// 1. **叠放不许把格子撑高**：格子尺寸由 `SliverGridDelegate` 决定，这里是
///    `StackFit.expand`，露边用的是 `Positioned` 的 inset，**不改变外框**。
///    改了外框 `AdaptiveColumnList` 那类"固定行高"的估算就会飘（AGENTS §6）。
/// 2. **解码尺寸仍要限制**：缩略图走的是 512px 的 `/thumb`（视频走 `/poster`），
///    不限制 `cacheWidth` 就会每格解一张大图 —— 与 `MediaThumb` 同一条纪律。
/// 3. **NSFW 打码包在最里面**，角标与标题仍然看得见（用户 2026-09-20 的要求：
///    先知道"这是什么"才谈得上决定要不要点开）。
class CollectionThumb extends StatelessWidget {
  /// 列表里代表这条（也是叠放时最上面那张）。
  final MediaAsset media;

  /// 集合**头三张**成员的 id（后端 `collectionPreview` 给的，已经按时间升序）。
  final List<int> previewIds;

  final VoidCallback onTap;
  final bool selected;
  final bool selectionMode;
  final VoidCallback? onToggleSelect;

  const CollectionThumb({
    super.key,
    required this.media,
    required this.previewIds,
    required this.onTap,
    this.selected = false,
    this.selectionMode = false,
    this.onToggleSelect,
  });

  /// 最多叠几层（再多的成员只体现在角标数字上）。
  static const int maxLayers = 3;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.read<LibraryStore>();
    final ids = previewIds.take(maxLayers).toList();
    // 至少要有代表自己；后端理论上一定给（代表是集合里最早那条），拿不到就退化成单图。
    final layers = ids.isEmpty ? [media.id] : ids;
    final count = media.collectionSize;

    return Material(
      color: Colors.transparent,
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: selectionMode ? (onToggleSelect ?? onTap) : onTap,
        child: Stack(
          // 外框尺寸**由父级决定**，这里只负责在框内摆放（不许撑高）
          fit: StackFit.expand,
          clipBehavior: Clip.none,
          children: [
            // 从**最里层**开始画：露得最多的那张先画，最上面那张最后画。
            // `layers` 是时间升序，所以最后画的是最早那张 —— 它盖在最上面，
            // 与"列表代表 = 最早那条"一致（点开详情也是从它开始往右切）。
            for (var i = layers.length - 1; i >= 0; i--)
              Positioned(
                left: 0,
                top: 0,
                // 第 0 层（最上面那张）不露边；后面的每层各往右下缩一点
                right: i == 0 ? 0 : 4.0 * i + 2,
                bottom: i == 0 ? 0 : 4.0 * i + 2,
                child: _layer(context, theme, store, layers[i], isTop: i == 0),
              ),
            _countBadge(theme, count),
            if (selectionMode)
              Positioned(top: 6, left: 6, child: _check(theme)),
          ],
        ),
      ),
    );
  }

  /// 一层封面。最上面那层带角标、标题与打码；露边的那几层只有图。
  Widget _layer(
    BuildContext context,
    ThemeData theme,
    LibraryStore store,
    int mediaId, {
    required bool isTop,
  }) {
    // 封面地址统一从后端给的 `thumbUrl` 模板拼（**前端不许自己拼媒体路径**的例外只有
    // "叠放要看的是兄弟产物的缩略图"这一处：代表那条用 DTO 里现成的 URL，
    // 其余成员按同一个约定换成它们的 id）。视频的 `/thumb` 会回 204，
    // 所以视频那几层退到 `/poster`（与 `MediaThumb` 同一套退化）。
    final String url;
    if (mediaId == media.id) {
      url = media.kind == MediaKind.video
          ? store.api.mediaPosterUrl(media.id)
          : store.api.absolute(media.thumbUrl ?? media.fileUrl);
    } else {
      url = store.api.absolute('/api/media/$mediaId/thumb');
    }

    final image = CachedThumbImage(
      mediaId: mediaId,
      url: url,
      api: store.api,
      cacheWidth: kThumbMaxDecodeEdge,
      theme: theme,
      loading: () => Center(
        // 与 `MediaThumb` 同一条纪律：占位图标压在 **surface** 上，
        // 颜色必须从主题取 —— 写死白色在浅色主题下等于看不见。
        child: Icon(
          Icons.image_outlined,
          size: 26,
          color: theme.colorScheme.onSurfaceVariant.withValues(alpha: 0.35),
        ),
      ),
      failed: () => Center(
        child: Icon(
          Icons.image_not_supported_outlined,
          size: 22,
          color: theme.colorScheme.onSurfaceVariant.withValues(alpha: 0.5),
        ),
      ),
    );

    return Material(
      color: theme.colorScheme.surfaceContainerHighest,
      borderRadius: BorderRadius.circular(12),
      clipBehavior: Clip.antiAlias,
      child: Stack(
        fit: StackFit.expand,
        children: [
          if (isTop)
            NsfwBlur(
              nsfw: media.nsfw,
              revealKey: 'media:${media.id}',
              borderRadius: BorderRadius.circular(12),
              child: image,
            )
          else
            // 露边的那几层**不打码**：它们只露出一条边，糊上去反而像渲染故障。
            // 真正要打码的是最上面那张（用户看到的就是它）。
            image,
          if (isTop) ...[
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
                    colors: [
                      Colors.black.withValues(alpha: 0.55),
                      Colors.transparent,
                    ],
                  ),
                ),
              ),
            ),
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
                    colors: [
                      Colors.black.withValues(alpha: 0.72),
                      Colors.transparent,
                    ],
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
              Positioned(
                top: 6,
                right: 6,
                child: Icon(
                  Icons.star,
                  size: 16,
                  color: theme.colorScheme.tertiary,
                  shadows: const [Shadow(blurRadius: 3, color: Colors.black54)],
                ),
              ),
            Positioned(
              left: 8,
              // 右下角留给角标，所以标题往左收一点
              right: 46,
              bottom: 8,
              child: Text(
                media.title.isEmpty ? media.originalName : media.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                  shadows: [Shadow(blurRadius: 3, color: Colors.black87)],
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  /// 右下角的总数角标。**只在成员多于 1 时显示**（1 张的集合不显示"1"）。
  Widget _countBadge(ThemeData theme, int count) {
    if (count <= 1) return const SizedBox.shrink();
    return Positioned(
      right: 6,
      bottom: 6,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
        decoration: BoxDecoration(
          color: Colors.black.withValues(alpha: 0.66),
          borderRadius: BorderRadius.circular(9),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(
              Icons.filter_none_rounded,
              size: 12,
              color: Colors.white,
            ),
            const SizedBox(width: 3),
            Text(
              '$count',
              style: const TextStyle(
                color: Colors.white,
                fontSize: 11.5,
                fontWeight: FontWeight.w700,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _check(ThemeData theme) => Container(
    width: 22,
    height: 22,
    decoration: BoxDecoration(
      shape: BoxShape.circle,
      color: selected ? theme.colorScheme.primary : Colors.black45,
      border: Border.all(
        color: selected
            ? theme.colorScheme.onPrimary
            : theme.colorScheme.onInverseSurface,
        width: 1.5,
      ),
    ),
    child: selected
        ? Icon(
            Icons.check,
            size: 14,
            color: theme.colorScheme.onPrimary,
          )
        : null,
  );
}

/// 叠放时**每层的最大内缩**（像素）—— 抽出来是为了能被测试钉住"格子不被撑高"。
double collectionLayerInset(int layer) => layer == 0 ? 0 : 4.0 * layer + 2;

/// 叠放需要几个成员才有意义（少于 2 个就不叠）。
bool needsStacking(int collectionSize) => collectionSize > 1;

/// 解码尺寸夹取（与 `MediaThumb` 同一条纪律：别按原图分辨率解码）。
int collectionDecodeWidth(double cellEdge, double dpr) {
  final needed = math.max(cellEdge, 1.0) * dpr;
  if (!needed.isFinite || needed <= 0) return kThumbMaxDecodeEdge;
  return needed.round().clamp(1, kThumbMaxDecodeEdge);
}
