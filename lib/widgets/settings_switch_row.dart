import 'package:flutter/material.dart';

/// 设置项里的开关行。
///
/// `SwitchListTile` 自己的 `contentPadding` 是零，直接放进卡片里的话，
/// 标题会顶到卡片左边缘、开关顶到右边缘，跟同卡片里输入框的留白对不上 ——
/// 所以统一给一点内边距。
///
/// ⚠ 不要再往它外面套一层带底色的 `Material` / `Container`：M3 的 ListTile 本来
/// 就该直接坐在卡片上，套一层会变成"卡中卡"（2026-11-05 已去掉）。
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
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: SwitchListTile(
        contentPadding: const EdgeInsets.symmetric(horizontal: 4),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(12),
        ),
        value: value,
        onChanged: onChanged,
        title: Text(title),
        subtitle: Text(subtitle),
      ),
    );
  }
}
