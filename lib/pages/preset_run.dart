import 'package:flutter/material.dart';

import '../models/models.dart';
import '../widgets/common.dart';

/// 预设「▶️运行」：选目标产物类型 + 确认，然后把文本交给工作台。
///
/// # 为什么是弹窗而不是直接跑
///
/// 用户 2026-10-11 定的两条：
///  1. **要用户确认**（"经过用户确认后把提示词发给工作台执行"）——
///     所以这里只负责"问清楚 + 生成一段交给工作台的话"，**不自动起 Run**；
///  2. 类型**能改**：预设自己的用途（`PromptKind`）是默认值，
///     用户这次想拿它去生视频也行。
///
/// # 为什么生成的是"一段话"而不是结构化参数
///
/// 工作台那条链路的入口就是"用户发一条消息"，模型再自己调 `comfy_submit`。
/// 给它一个 `presetId` 参数就要动工具清单 + bump `SystemPrompt.VERSION`（R12），
/// 而这里要表达的语义（"用这段提示词、出这类产物"）一段话就说清了。
/// 提示词正文**原样带上**（不改一个字符），否则用户会觉得"跑的不是我存的那条"。

/// 弹窗：返回用户选定的产物类型；取消返回 `null`。
///
/// 用户 2026-10-11："发送前的弹窗再让用户看一眼提示词详情" —— 所以这里把
/// **正向 / 负向提示词正文都摊开**（可滚动），而不是只显示个标题。
Future<PromptKind?> showPresetRunDialog(BuildContext context, Prompt prompt) {
  // 预设指定了用途就默认选它；`MIXED`（混合）在"这次要出什么"上没有意义，
  // 退化成生图（与"未指定"同一个默认值）。
  final initial = prompt.kind == PromptKind.mixed ? PromptKind.image : prompt.kind;
  return showDialog<PromptKind>(
    context: context,
    builder: (context) => _PresetRunDialog(
      prompt: prompt,
      initial: initial,
      specified: prompt.kind != PromptKind.mixed,
    ),
  );
}

class _PresetRunDialog extends StatefulWidget {
  final Prompt prompt;
  final PromptKind initial;

  /// 预设自己**有没有**指定用途（决定那句提示文案怎么写）。
  final bool specified;

  const _PresetRunDialog({
    required this.prompt,
    required this.initial,
    required this.specified,
  });

  @override
  State<_PresetRunDialog> createState() => _PresetRunDialogState();
}

class _PresetRunDialogState extends State<_PresetRunDialog> {
  late PromptKind _kind = widget.initial;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = widget.prompt;
    final negative = (p.negativePrompt ?? '').trim();
    // 只发正面的预设：把负面**灰着**列出来并说明"这次不会发出去" ——
    // 直接藏掉会让用户以为"我的负面丢了"（见 `buildPresetRunText` 的同一条理由）。
    final positiveOnly = p.negativeMode == 'positive_only';
    final title = p.title.isEmpty ? '未命名预设' : p.title;

    return AlertDialog(
      title: const Text('运行预设'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  KindBadge(kind: p.kind),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleSmall,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 14),
              Text('这次要出什么？', style: theme.textTheme.labelLarge),
              const SizedBox(height: 6),
              SegmentedButton<PromptKind>(
                segments: const [
                  ButtonSegment(value: PromptKind.image, label: Text('生图'), icon: Icon(Icons.image_outlined)),
                  ButtonSegment(value: PromptKind.video, label: Text('生视频'), icon: Icon(Icons.movie_outlined)),
                  ButtonSegment(value: PromptKind.audio, label: Text('生音频'), icon: Icon(Icons.music_note_outlined)),
                ],
                selected: {_kind},
                onSelectionChanged: (s) => setState(() => _kind = s.first),
              ),
              const SizedBox(height: 6),
              Text(
                widget.specified ? '已按预设的用途选好，可以改。' : '这个预设没指定用途，选一个再继续。',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
              const SizedBox(height: 16),
              // ---- 提示词详情（发送前让用户看一眼）----
              _preview(theme, '正向提示词', p.positivePrompt.trim(), dim: false),
              if (negative.isNotEmpty) ...[
                const SizedBox(height: 10),
                _preview(
                  theme,
                  positiveOnly ? '负向提示词（这次不会发出去）' : '负向提示词',
                  negative,
                  dim: positiveOnly,
                ),
              ],
              const SizedBox(height: 12),
              Text(
                '确认后会把上面的提示词填进「AI 工作台」的输入框（会新建一条对话），还要你自己按发送。',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('取消')),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_kind),
          child: const Text('送到工作台'),
        ),
      ],
    );
  }

  Widget _preview(ThemeData theme, String label, String text, {required bool dim}) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: theme.textTheme.labelMedium),
        const SizedBox(height: 4),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
            borderRadius: BorderRadius.circular(8),
          ),
          child: SelectableText(
            text,
            style: theme.textTheme.bodySmall?.copyWith(
              color: dim ? theme.colorScheme.outline : null,
              decoration: dim ? TextDecoration.lineThrough : null,
            ),
          ),
        ),
      ],
    );
  }
}

/// 交给工作台的那段话。
///
/// 四条纪律：
///  1. **提示词正文逐字带上**（不改写、不截断）—— 用户存的什么就跑什么；
///  2. **说清"要出什么"**（生图/生视频/生音频）—— 否则模型可能拿一句生图提示词去跑视频工作流；
///  3. **不带【已存参数】**（用户 2026-10-11 明确要求）——
///     底模 / 采样器 / 步数 / 尺寸这些**不是这段提示词的一部分**，
///     它们属于"当初那份工作流"。把参数塞给模型等于替它做决定，
///     反而让它跳过"这份预设该配什么模型"这一步。
///     正确行为是：**模型自己发觉没被要求用哪个生图模型时，主动问用户**
///     （这条写在下面的"要求"里）。
///  4. **说清"先找能跑的工作流"** —— 库里有工作流就照它跑，别自己另搭一份
///     （这是 2026-09-28 那条"模型不知道先生成替换目标"的同类教训：
///     让它先侦察再动手，而不是从零造图）。
String buildPresetRunText(Prompt prompt, PromptKind kind) {
  final buffer = StringBuffer()
    ..writeln('用下面这个预设去${_kindAction(kind)}：')
    ..writeln()
    ..writeln('【提示词】')
    ..writeln(prompt.positivePrompt.trim());

  final negative = (prompt.negativePrompt ?? '').trim();
  // **发送模式**（用户 2026-10-11）：预设标了"只发正面"就**不带**负面提示词。
  // 这不是"忘了写" —— 有些 turbo 模型不认负面，带上反而干扰；所以这里连一行都不提，
  // 并在末尾的要求里说清"这次不要负面提示词"，免得模型自己又补一段上去。
  final positiveOnly = prompt.negativeMode == 'positive_only';
  if (negative.isNotEmpty && !positiveOnly) {
    buffer
      ..writeln()
      ..writeln('【负面提示词】')
      ..writeln(negative);
  }

  // ⚠ **刻意不给生成参数**（底模 / 采样器 / 步数 / 尺寸）：见上面第 3 条。
  // 用户要看那些参数，去关联产物的详情页看 —— 那是"当初那次生成"的事实。

  buffer
    ..writeln()
    ..writeln(
      '要求：先用 comfy_find_workflow / comfy_list_workflows 找一份能跑的工作流（优先库里已有的），'
      '把上面这段提示词按它的输入结构填进去再提交，别自己从零搭一份新工作流。'
      '如果缺模型或节点，如实说缺什么，不要假装跑过了。',
    )
    ..writeln(
      '另外，上面没有指定用哪个生图模型。你发觉这一点时要主动问用户'
      '（这次用哪个底模 / 有没有偏好），别自己替他挑一个就开跑 —— '
      '但也别只是干等：可以同时把找到的候选工作流与它们的模型列给他看。',
    );
  if (positiveOnly) {
    buffer.writeln(
      '还有：这个预设设成了「只发正面提示词」（目标模型不认负面），'
      '所以不要再补任何负面提示词上去。',
    );
  }
  return buffer.toString();
}

String _kindAction(PromptKind kind) => switch (kind) {
      PromptKind.image => '生图',
      PromptKind.video => '生视频',
      PromptKind.audio => '生音频',
      PromptKind.mixed => '生成内容',
    };
