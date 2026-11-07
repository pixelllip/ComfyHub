import 'package:flutter/material.dart';

import '../models/ai_models.dart';

/// 一条说明 / 错误：浅色底 + 同色描边。
///
/// 列表、详情、「凭据」与对话框共用这一份 —— 以前它是页面文件里的私有
/// `_Note`，拆文件之后只能提成公开组件（视觉与文案一个字都没改）。
class AiNote extends StatelessWidget {
  final String text;
  final bool error;
  const AiNote({super.key, required this.text, required this.error});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = error ? theme.colorScheme.error : theme.colorScheme.primary;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Text(text, style: theme.textTheme.bodySmall),
    );
  }
}

/// 绿点 = 已配置，红点 = 缺失，灰点 = 由环境变量提供（只读）。
class AiCredentialDot extends StatelessWidget {
  final AiCredentialStatus status;
  const AiCredentialDot({super.key, required this.status});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = !status.configured
        ? theme.colorScheme.error
        : (status.source == 'env' ? theme.colorScheme.outline : Colors.green);
    return Tooltip(
      message: status.label,
      child: Icon(Icons.circle, size: 10, color: color),
    );
  }
}
