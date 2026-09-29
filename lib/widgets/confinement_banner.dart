/// **"后端被关在项目里了"** 的常驻横幅。
///
/// # 为什么要有它
///
/// 2026-09-29 用户 bug：AI 助手如实转述 `comfy_make_mask` 的失败
/// （`写停放好的人物图失败：拒绝访问。 (os error 5)`），并**据此诊断成**
/// "要解决 input 目录对摆放文件的写入权限" —— 那个诊断是错的：真因是"映像在工作区内的
/// 后端写不了工作区外"（AGENTS §12）。用户于是去改 ACL，白忙。
///
/// 受限是**环境事实**，界面上一处都看不到 = 用户只能靠猜。所以：
///
/// * 横幅**在受限时才出现**，不受限时一个像素都不占（不制造噪音）；
/// * 判据（后端映像在不在工作区内）与结论**一起说** —— 只说"受限"用户没法自查；
/// * **修法文案取后端给的那一份**（`ConfinementInfo.remedy`），界面不另写一份，
///   免得 doctor / 工具文案 / 界面三处说法漂移。
///
/// 数据从哪来：`GET /api/system/confinement`（`ApiClient.confinement()`），
/// 由 `HomeShell` 在挂载时问一次 —— 不是实时监控；真要立刻反映"重起后端解除了"，
/// 回到设置页点一次「保存并测试连接」就会重新问。
library;

import 'package:flutter/material.dart';

import '../models/models.dart';

/// 受限时显示的那一条；`info` 为 null 或不受限时**什么都不显示**。
class ConfinementBanner extends StatelessWidget {
  final ConfinementInfo? info;

  /// 点横幅或「去设置」时的回调（一般是切到设置页）。
  final VoidCallback? onOpenSettings;

  const ConfinementBanner({super.key, required this.info, this.onOpenSettings});

  @override
  Widget build(BuildContext context) {
    final info = this.info;
    if (info == null || !info.restricted) return const SizedBox.shrink();

    final scheme = Theme.of(context).colorScheme;
    final onError = scheme.onErrorContainer;
    final text = Theme.of(context).textTheme;
    final detail = info.detail;
    final remedy = info.remedy;
    return Material(
      color: scheme.errorContainer,
      child: InkWell(
        onTap: onOpenSettings,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(Icons.lock_outline, size: 20, color: onError),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      info.headline,
                      style: text.titleSmall?.copyWith(color: onError, fontWeight: FontWeight.w600),
                    ),
                    if (detail.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: Text(detail, style: text.bodySmall?.copyWith(color: onError)),
                      ),
                    if (remedy != null && remedy.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.only(top: 4),
                        child: Text(remedy, style: text.bodySmall?.copyWith(color: onError)),
                      ),
                  ],
                ),
              ),
              if (onOpenSettings != null)
                TextButton(
                  onPressed: onOpenSettings,
                  child: Text('去设置', style: TextStyle(color: onError)),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
