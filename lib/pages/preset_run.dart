import 'package:material_ui/material_ui.dart';

import '../models/models.dart';
import '../widgets/common.dart';

/// 预设「▶️运行」：选目标产物类型 + 确认，然后把文本交给工作台。
///
/// # 弹窗形式
///
/// 两条约束：
///  1. **需要确认** —— 这里只负责询问并生成一段交给工作台的文本，**不自动启动 Run**；
///  2. 类型**可修改**：预设自身的用途（`PromptKind`）为默认值，
///     也可以改为生成视频。
///
/// # 生成文本而非结构化参数
///
/// 工作台的入口是发送一条消息，模型再调用 `comfy_submit`。
/// 增加 `presetId` 参数需要修改工具清单并 bump `SystemPrompt.VERSION`（R12），
/// 而这里表达的语义（"使用这段提示词、生成这类产物"）用一段文本即可说明。
/// 提示词正文**原样带上**（不修改任何字符），否则实际执行的与保存内容不一致。

/// 弹窗：返回选定的产物类型；取消返回 `null`。
///
/// 发送前的弹窗显示提示词详情：**正向 / 负向提示词正文均展开**（可滚动），
/// 不只显示标题。
Future<PromptKind?> showPresetRunDialog(BuildContext context, Prompt prompt) {
  // 预设指定了用途就默认选它；`MIXED`（混合）在"这次要出什么"上没有意义，
  // 退化成生图（与"未指定"同一个默认值）。
  final initial = prompt.kind == PromptKind.mixed
      ? PromptKind.image
      : prompt.kind;
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
    // 直接隐藏会被理解为"负面提示词丢失"（见 `buildPresetRunText` 的同一条约束）。
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
                  ButtonSegment(
                    value: PromptKind.image,
                    label: Text('生图'),
                    icon: Icon(Icons.image_outlined),
                  ),
                  ButtonSegment(
                    value: PromptKind.video,
                    label: Text('生视频'),
                    icon: Icon(Icons.movie_outlined),
                  ),
                  ButtonSegment(
                    value: PromptKind.audio,
                    label: Text('生音频'),
                    icon: Icon(Icons.music_note_outlined),
                  ),
                ],
                selected: {_kind},
                onSelectionChanged: (s) => setState(() => _kind = s.first),
              ),
              const SizedBox(height: 6),
              Text(
                widget.specified ? '已按预设的用途选好，可以改。' : '这个预设没指定用途，选一个再继续。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 16),
              // ---- 提示词详情（发送前查看）----
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
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_kind),
          child: const Text('送到工作台'),
        ),
      ],
    );
  }

  Widget _preview(
    ThemeData theme,
    String label,
    String text, {
    required bool dim,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: theme.textTheme.labelMedium),
        const SizedBox(height: 4),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainerHighest.withValues(
              alpha: 0.4,
            ),
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
/// 四条约束：
///  1. **提示词正文逐字带上**（不修改、不截断）—— 执行内容与保存内容一致；
///  2. **说明"要生成什么"**（图片 / 视频 / 音频）—— 否则模型可能用图片提示词执行视频工作流；
///  3. **不带【已存参数】**——
///     底模 / 采样器 / 步数 / 尺寸**不属于这段提示词**，
///     它们属于"当初那份工作流"。将参数直接交给模型会代替其判断，
///     使其跳过"该预设应配哪个模型"这一步。
///     正确行为是：**模型发现未被指定生图模型时，主动询问**
///     （见下方"要求"部分）。
///  4. **说明"先查找可用工作流"** —— 库中存在工作流时按其执行，不另行搭建
///     （先侦察再执行，不从零构建）。
String buildPresetRunText(Prompt prompt, PromptKind kind) {
  final buffer = StringBuffer()
    ..writeln('用下面这个预设去${_kindAction(kind)}：')
    ..writeln()
    ..writeln('【提示词】')
    ..writeln(prompt.positivePrompt.trim());

  final negative = (prompt.negativePrompt ?? '').trim();
  // **发送模式**：预设标记"只发正面"时**不带**负面提示词。
  // 部分 turbo 模型不支持负面提示词，带上会产生干扰；因此这里不输出该行，
  // 并在末尾说明"本次不使用负面提示词"，避免模型自行补充。
  final positiveOnly = prompt.negativeMode == 'positive_only';
  if (negative.isNotEmpty && !positiveOnly) {
    buffer
      ..writeln()
      ..writeln('【负面提示词】')
      ..writeln(negative);
  }

  // ⚠ **不提供生成参数**（底模 / 采样器 / 步数 / 尺寸）：见上面第 3 条。
  // 这些参数可在关联产物的详情页中查看，即"当初那次生成"的实际配置。

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
