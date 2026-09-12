import 'package:flutter/material.dart';

import '../models/models.dart';
import 'common.dart';

/// 提示词的「生成参数」：模型 / 采样器 / 调度器 / 步数 / CFG / Seed / 尺寸 / 批量 + LoRA。
///
/// 产物详情和提示词详情共用这一份渲染，两处字段永远不会对不上。
///
/// LoRA 刻意**不**跟其它参数一起挤进 Wrap：一次生成挂两个以上 LoRA 是常态，
/// 全塞成 `LoRA: xxx` 的胶囊后会变成一排一模一样的「LoRA」前缀，认不出谁是谁。
/// 所以它单独占一行，每个 LoRA 一个胶囊（名字 + 权重），点一下复制
/// `<lora:名字:权重>` 标签。
class PromptParams extends StatelessWidget {
  final Prompt prompt;

  const PromptParams({super.key, required this.prompt});

  /// 这几个参数一个都没有时，调用方直接别渲染这块（免得出现空卡片）
  static bool hasAny(Prompt p) =>
      p.checkpoint != null ||
      p.sampler != null ||
      p.scheduler != null ||
      p.steps != null ||
      p.cfgScale != null ||
      p.seed != null ||
      p.width != null ||
      p.height != null ||
      p.batchSize != null ||
      p.loras.any((e) => e.name.trim().isNotEmpty);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 名字为空的 LoRA 是编辑页里加了还没填的半成品，不当参数展示
    final loras = prompt.loras.where((e) => e.name.trim().isNotEmpty).toList();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          children: [
            MetaChip(label: '模型', value: prompt.checkpoint, icon: Icons.memory),
            MetaChip(label: '采样器', value: prompt.sampler, icon: Icons.tune),
            MetaChip(label: '调度器', value: prompt.scheduler, icon: Icons.schedule),
            MetaChip(label: '步数', value: prompt.steps?.toString(), icon: Icons.stairs),
            MetaChip(label: 'CFG', value: prompt.cfgScale?.toString(), icon: Icons.equalizer),
            MetaChip(label: 'Seed', value: prompt.seed?.toString(), icon: Icons.casino_outlined),
            MetaChip(
              label: '尺寸',
              value: (prompt.width != null && prompt.height != null)
                  ? '${prompt.width}×${prompt.height}'
                  : null,
              icon: Icons.aspect_ratio,
            ),
            MetaChip(label: '批量', value: prompt.batchSize?.toString(), icon: Icons.grid_view),
          ],
        ),
        if (loras.isNotEmpty) ...[
          const SizedBox(height: 2),
          Row(
            children: [
              Icon(Icons.extension, size: 13, color: theme.colorScheme.outline),
              const SizedBox(width: 4),
              Text(
                'LoRA (${loras.length})',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
              const SizedBox(width: 4),
              // 多个 LoRA 时，一次复制出整串标签，省得一个个点
              if (loras.length > 1)
                TextButton.icon(
                  onPressed: () => copyToClipboard(
                    context,
                    loras.map(LoraChip.tagOf).join(', '),
                    label: '全部 LoRA 标签',
                  ),
                  icon: const Icon(Icons.copy_all_outlined, size: 14),
                  label: const Text('复制全部'),
                  style: TextButton.styleFrom(
                    visualDensity: VisualDensity.compact,
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                    minimumSize: Size.zero,
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Wrap(children: [for (final lora in loras) LoraChip(lora: lora)]),
        ],
      ],
    );
  }
}

/// 单个 LoRA：名字 + 权重，点一下把 `<lora:名字:权重>` 复制走
class LoraChip extends StatelessWidget {
  final LoraRef lora;

  const LoraChip({super.key, required this.lora});

  /// 复制到提示词里的写法（A1111 / ComfyUI 的 LoRA 标签语法）
  static String tagOf(LoraRef l) => '<lora:${l.name}:${formatWeight(l.weight)}>';

  /// 1.0 → 1，0.8 → 0.8；权重是整数时不补 `.0`，看着更像提示词里的写法
  static String formatWeight(double w) =>
      w == w.roundToDouble() ? w.toStringAsFixed(0) : w.toString();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = theme.colorScheme.primary;
    final tag = tagOf(lora);

    return Padding(
      padding: const EdgeInsets.only(right: 8, bottom: 8),
      child: Tooltip(
        message: '$tag\n点击复制这个 LoRA 标签',
        child: Material(
          color: color.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(8),
          child: InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: () => copyToClipboard(context, tag, label: 'LoRA 标签'),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.extension, size: 13, color: color),
                  const SizedBox(width: 5),
                  // LoRA 名字经常很长（带 .safetensors），限宽 + 省略号，
                  // 完整写法挂在 tooltip 上
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 230),
                    child: Text(
                      lora.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelMedium?.copyWith(fontWeight: FontWeight.w600),
                    ),
                  ),
                  const SizedBox(width: 6),
                  Text(
                    formatWeight(lora.weight),
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: color,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
