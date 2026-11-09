import 'package:flutter/material.dart';

import '../core/theme.dart';
import '../models/models.dart';

/// 设置 → 本地服务 → 「联网代理」那张卡。
///
/// 为什么界面上要有它（2026-11-09 用户 bug）：danbooru 挂维护、Cloudflare 回 503，
/// 工具报错只写"暂时不可用（503）"，看着就像"本项目没吃代理"。
/// 真相是**路由是通的、503 是对方回的**。这件事得一眼看见；
/// 光看"配置"还不够 —— 系统代理开着也可能代理本身挂了，所以另给一个「测试出网」。
///
/// 文案纪律：别把 `env:HTTPS_PROXY` / `via:proxy` 这类内部代号甩到界面上，
/// 也别说成"代理已生效"这种没验证过的话（只有探针回来才算验证）。
class OutboundCard extends StatelessWidget {
  final OutboundInfo? info;
  final bool loading;
  final bool probing;
  final String? error;
  final VoidCallback? onRefresh;
  final VoidCallback? onProbe;

  const OutboundCard({
    super.key,
    this.info,
    this.loading = false,
    this.probing = false,
    this.error,
    this.onRefresh,
    this.onProbe,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = AppColors.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );

    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                const Icon(Icons.travel_explore, size: 18),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    '出网路径',
                    style: theme.textTheme.titleSmall?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                IconButton(
                  onPressed: loading ? null : onRefresh,
                  tooltip: '重新读取',
                  icon: const Icon(Icons.refresh, size: 18),
                ),
              ],
            ),
            if (error != null && error!.isNotEmpty) ...[
              const SizedBox(height: 4),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    Icons.error_outline,
                    size: 18,
                    color: theme.colorScheme.error,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      '读取失败，$error',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ),
                ],
              ),
            ],
            if (info == null) ...[
              const SizedBox(height: 8),
              Text(loading ? '正在读取…' : '还没读到出网设置。', style: muted),
            ] else ...[
              Text(
                info!.headline,
                style: theme.textTheme.bodyMedium?.copyWith(
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                info!.viaProxy
                    ? '这个地址来自${info!.sourceLabel}。本机与局域网地址永远直连，'
                          'ComfyUI 和手机连过来都不受影响。'
                    : '本机与局域网地址永远直连；公网没设代理时也是直连。'
                          '如果 Civitai、Danbooru 或模型服务连不上，'
                          '请打开系统代理，或设环境变量 COMFYHUB_HTTPS_PROXY。',
                style: muted,
              ),
              if (info!.warning != null && info!.warning!.isNotEmpty) ...[
                const SizedBox(height: 6),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(
                      Icons.warning_amber_rounded,
                      size: 18,
                      color: theme.colorScheme.error,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        info!.warning!,
                        style: theme.textTheme.bodySmall?.copyWith(
                          color: theme.colorScheme.error,
                        ),
                      ),
                    ),
                  ],
                ),
              ],
              if (info!.overrides.isNotEmpty) ...[
                const SizedBox(height: 6),
                Text(
                  '系统代理的例外列表（这些地址直连）${_shortOverrides(info!.overrides)}',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ],
              const Divider(height: 20),
              Wrap(
                spacing: 10,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  OutlinedButton.icon(
                    onPressed: probing ? null : onProbe,
                    icon: probing
                        ? const SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.wifi_tethering, size: 18),
                    label: Text(probing ? '正在测试…' : '测试出网'),
                  ),
                  Text(
                    '会向 Civitai 与 Danbooru 各发一次请求。',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
              if (info!.probes.isNotEmpty) ...[
                const SizedBox(height: 10),
                for (final probe in info!.probes)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          _probeIcon(probe),
                          size: 16,
                          color: _probeColor(probe, theme, colors),
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            probe.sentence,
                            style: theme.textTheme.bodySmall,
                          ),
                        ),
                      ],
                    ),
                  ),
                Text(
                  '收到 4xx / 5xx 说明网络是通的，是对方回的；连不上或超时才说明这条出网路径有问题。',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ],
            ],
          ],
        ),
      ),
    );
  }

  static IconData _probeIcon(OutboundProbe probe) {
    if (!probe.reached) return Icons.error_outline;
    final status = probe.status ?? 0;
    if (status >= 200 && status < 300) return Icons.check_circle_outline;
    return Icons.info_outline;
  }

  static Color _probeColor(
    OutboundProbe probe,
    ThemeData theme,
    AppColors colors,
  ) {
    if (!probe.reached) return theme.colorScheme.error;
    final status = probe.status ?? 0;
    return (status >= 200 && status < 300)
        ? colors.success
        : theme.colorScheme.onSurfaceVariant;
  }

  /// 例外列表可能很长（Windows 默认就十几条）—— 只列前几条，别把卡片撑爆。
  static String _shortOverrides(List<String> list) {
    const max = 6;
    final shown = list.take(max).join('、');
    return list.length <= max ? ' $shown' : ' $shown 等 ${list.length} 项';
  }
}
