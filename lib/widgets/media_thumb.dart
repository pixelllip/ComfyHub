import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import 'common.dart';

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
    final thumb = media.kind == MediaKind.image
        ? store.api.absolute(media.thumbUrl ?? media.fileUrl)
        : null;

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
              Image.network(
                thumb,
                fit: BoxFit.cover,
                filterQuality: FilterQuality.medium,
                loadingBuilder: (context, child, progress) {
                  if (progress == null) return child;
                  return const Center(
                    child: SizedBox(width: 20, height: 20, child: CircularProgressIndicator(strokeWidth: 2)),
                  );
                },
                errorBuilder: (context, error, stack) => _placeholder(theme),
              )
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
