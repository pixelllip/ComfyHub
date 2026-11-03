import 'package:flutter/material.dart';

import '../models/layout_embed.dart';
import 'layout_thumbnail.dart';

/// 回复末尾（或工具卡后面）那排**候选布局**。
///
/// 用户 2026-11-03 的口径："AI 生成提示词的同时给出可用的 2D 布局模板，
/// 我可打开预览并修改，修改后重新提交给工作台。"
/// 所以这张卡只做两件事：把模板画出来、把「打开画布」做成一个明确的动作。
/// **不在这里直接"应用"** —— 改不改、改哪儿，必须由用户进画布决定。
class LayoutTemplatesCard extends StatelessWidget {
  const LayoutTemplatesCard({
    super.key,
    required this.candidates,
    required this.onOpen,
  });

  final List<LayoutCandidate> candidates;

  /// 打开画布（带上这一份规格）。
  final void Function(LayoutCandidate candidate) onOpen;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      key: const Key('layout-templates-card'),
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest.withValues(
          alpha: 0.45,
        ),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: theme.dividerColor),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                Icons.dashboard_customize_outlined,
                size: 15,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  '2D 布局模板（${candidates.length}）',
                  style: theme.textTheme.labelMedium,
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 21, top: 2),
            child: Text(
              '点一条打开画布，改完提交回对话 —— AI 会按新构图重写提示词。',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ),
          const SizedBox(height: 6),
          for (var i = 0; i < candidates.length; i++)
            _LayoutCandidateRow(
              // ⚠ 用序号而不是标题：模型完全可能给两个同名模板，
              // 同层重复 Key 会直接抛 "Duplicate keys found"。
              key: ValueKey('layout-candidate-$i'),
              candidate: candidates[i],
              onTap: () => onOpen(candidates[i]),
            ),
        ],
      ),
    );
  }
}

class _LayoutCandidateRow extends StatelessWidget {
  const _LayoutCandidateRow({
    super.key,
    required this.candidate,
    required this.onTap,
  });

  final LayoutCandidate candidate;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final spec = candidate.spec;
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.all(4),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                LayoutThumbnail(spec: spec),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        candidate.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelLarge,
                      ),
                      if (candidate.note.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(top: 2),
                          child: Text(
                            candidate.note,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: theme.textTheme.labelSmall?.copyWith(
                              color: theme.colorScheme.outline,
                            ),
                          ),
                        ),
                      Padding(
                        padding: const EdgeInsets.only(top: 4),
                        child: Text(
                          '${spec.ratioLabel} · ${spec.regions.length} 个框',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.labelSmall?.copyWith(
                            color: theme.colorScheme.outline,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 4),
                Padding(
                  padding: const EdgeInsets.only(top: 18),
                  child: Icon(
                    Icons.chevron_right,
                    size: 18,
                    color: theme.colorScheme.outline,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// 用户消息里的**已提交布局**：把那条长文本画成一张卡。
///
/// 没有它，重新提交的那条消息就是一屏 JSON；有了它，「再次修改」能直接接着改
/// **这一版**（而不是回头去改 AI 最初给的那一版）。
class LayoutResubmitCard extends StatelessWidget {
  const LayoutResubmitCard({
    super.key,
    required this.resubmit,
    required this.onEdit,
  });

  final LayoutResubmit resubmit;
  final VoidCallback onEdit;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final spec = resubmit.spec;
    return Container(
      key: const Key('layout-resubmit-card'),
      constraints: const BoxConstraints(maxWidth: 420),
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surface.withValues(alpha: 0.55),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: theme.dividerColor),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                Icons.dashboard_customize_outlined,
                size: 15,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(width: 6),
              Expanded(
                child: Text('已提交的 2D 布局', style: theme.textTheme.labelMedium),
              ),
            ],
          ),
          if (resubmit.note.trim().isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                resubmit.note.trim(),
                style: theme.textTheme.bodySmall,
              ),
            ),
          const SizedBox(height: 6),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              LayoutThumbnail(spec: spec, height: 56),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '${spec.ratioLabel} · ${spec.regions.length} 个框',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                    TextButton.icon(
                      onPressed: onEdit,
                      icon: const Icon(Icons.edit_outlined, size: 15),
                      label: const Text('再次修改'),
                      style: TextButton.styleFrom(
                        padding: const EdgeInsets.symmetric(horizontal: 6),
                        minimumSize: const Size(0, 30),
                        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
