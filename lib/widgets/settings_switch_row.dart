import 'package:material_ui/material_ui.dart';

/// 设置项里的开关行：标题 + 说明（左），开关（右）。
///
/// ⚠ 2026-11-09 起**不再用 `SwitchListTile`**：`ListTile` 的固有高度
/// （`computeMaxIntrinsicHeight`）是按"单行 / 两行 / 三行"给的常数，**不认
/// 副标题折行**。首页的 NSFW / 库统计要做成等高的并排两栏，外层 `IntrinsicHeight`
/// 量到的 NSFW 高度会比真实布局少一行，结果卡片撑不下、直接 overflow 15px。
/// 自己用 `Row` + `Column` + `Text` 拼，固有高度就按真实文字量算。
///
/// 样式对齐 M3 的 `ListTile`：标题 `bodyLarge` / 说明 `bodyMedium`（次要色），
/// 开关那一侧留 12 的间距；整行可点 = 切换开关。
class SettingsSwitchRow extends StatelessWidget {
  final bool value;
  final ValueChanged<bool>? onChanged;
  final String title;
  final String subtitle;

  const SettingsSwitchRow({
    super.key,
    required this.value,
    required this.onChanged,
    required this.title,
    required this.subtitle,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final enabled = onChanged != null;
    final titleColor = enabled
        ? theme.colorScheme.onSurface
        : theme.colorScheme.onSurface.withValues(alpha: 0.38);
    final subtitleColor = enabled
        ? theme.colorScheme.onSurfaceVariant
        : theme.colorScheme.onSurfaceVariant.withValues(alpha: 0.38);
    return InkWell(
      onTap: enabled ? () => onChanged!(!value) : null,
      borderRadius: BorderRadius.circular(12),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    title,
                    style: theme.textTheme.bodyLarge?.copyWith(
                      color: titleColor,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: subtitleColor,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Switch(value: value, onChanged: onChanged),
          ],
        ),
      ),
    );
  }
}
