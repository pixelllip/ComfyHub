import 'package:material_ui/material_ui.dart';

import '../models/ai_models.dart';

/// 「模型能力目录」卡片（挪进 Provider 详情中）。
///
/// 能力的预填来源是**在线数据集 models.dev**（「立即更新」拉取）；手工目录排在它前面，
/// 接口自身声明优先。**内置目录卡片已删除**：该目录是 `%USERPROFILE%\.dsh\settings.yaml`
/// 的冻结副本，而 AGENTS §10 规定"运行期不得依赖 `.dsh`"——
/// 安装本项目时可能没有安装 DSH。
/// 剩余 `内置规则` 是后端的兜底推断表（`capability_catalog.rs`），
/// 作为"网关未声明"时的最后一层，不需要手动维护。
/// 实测校准默认关，打开后才能发真实请求校准思考档位。
/// 所有状态都从后端读（`/api/ai/capabilities/*`），前端不存一份自己的。
class AiModelCapabilityCard extends StatelessWidget {
  final AiCapabilityStatus? status;
  final bool busy;
  final bool probeBusy;
  final VoidCallback? onRefreshStatus;
  final VoidCallback? onUpdateRemote;
  final ValueChanged<bool>? onProbeToggle;
  final VoidCallback? onProbe;

  /// 外边距。嵌在折叠区里时传 [EdgeInsets.zero]（外面已经有一圈留白了）。
  final EdgeInsetsGeometry margin;

  const AiModelCapabilityCard({
    super.key,
    required this.status,
    required this.busy,
    required this.probeBusy,
    required this.onRefreshStatus,
    required this.onUpdateRemote,
    required this.onProbeToggle,
    required this.onProbe,
    this.margin = const EdgeInsets.fromLTRB(12, 8, 12, 4),
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = status;
    final remoteStale = s?.remoteStale ?? false;
    // 闭包里访问可空函数字段无法被 Dart 的流分析提升，先取局部变量
    final update = onUpdateRemote;
    final probe = onProbe;

    return Card(
      margin: margin,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.auto_awesome_motion_outlined,
                  size: 18,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(width: 6),
                Text('模型能力目录', style: theme.textTheme.titleSmall),
                if (busy || probeBusy) ...[
                  const SizedBox(width: 10),
                  const SizedBox(
                    width: 12,
                    height: 12,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                ],
                const Spacer(),
                IconButton(
                  tooltip: '刷新目录状态',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(
                    minWidth: 28,
                    minHeight: 28,
                  ),
                  padding: EdgeInsets.zero,
                  onPressed: busy ? null : onRefreshStatus,
                  icon: const Icon(Icons.refresh, size: 16),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              '能力先按在线目录 models.dev 预填，点「立即更新」可以拉取。其次才是手工目录。'
              '接口自己声明的永远优先，网关什么都不说时才轮到最后那层兜底推断。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 6),
            if (s == null)
              Text('正在读取能力目录状态…', style: theme.textTheme.bodySmall)
            else if (s.error != null)
              Text(
                '读取失败，${s.error}',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              )
            else ...[
              Wrap(
                spacing: 6,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  if (s.customEntries != null) _Tag('手工 ${s.customEntries} 条'),
                  Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 2,
                    ),
                    decoration: BoxDecoration(
                      color: remoteStale
                          ? theme.colorScheme.errorContainer
                          : theme.colorScheme.surfaceContainerHighest,
                      borderRadius: BorderRadius.circular(6),
                    ),
                    child: Text(
                      s.remoteLabel + (s.remoteStale ? ' · 已过期' : ''),
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: remoteStale
                            ? theme.colorScheme.onErrorContainer
                            : theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                  if (s.lastMessage != null) _Tag(s.lastMessage!),
                ],
              ),
            ],
            const SizedBox(height: 6),
            // 开关 + 说明一行，两个动作一行（窄屏换行）。
            //
            // ⚠ 这里踩了两次（2026-09-24 真机验收）：
            //   ① 原来是一个 `Row` 塞四样东西 → `RIGHT OVERFLOWED BY 2 PIXELS`；
            //   ② 改成 `Wrap([Row(Switch+Text), 按钮…])` 之后**还是**溢出 10px ——
            //      因为里层那个 `Row` 自己是 `mainAxisSize.min` + 不换行的 `Text`，
            //      它一宽就顶出去了。所以文字必须是 `Expanded` + 省略号，
            //      动作按钮才是那个"窄了就换行"的 `Wrap`。
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Switch(
                      value: s?.probeEnabled ?? false,
                      onChanged: busy ? null : (v) => onProbeToggle?.call(v),
                      materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    const SizedBox(width: 2),
                    Expanded(
                      child: Text(
                        '允许实测校准（发真实请求，可能产生少量费用）',
                        style: theme.textTheme.bodySmall,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                Wrap(
                  spacing: 8,
                  runSpacing: 6,
                  children: [
                    FilledButton.icon(
                      onPressed: (busy || update == null)
                          ? null
                          : () => update(),
                      icon: const Icon(Icons.cloud_download_outlined, size: 16),
                      label: const Text('立即更新'),
                    ),
                    OutlinedButton.icon(
                      onPressed:
                          (busy ||
                              probeBusy ||
                              probe == null ||
                              s?.probeEnabled != true)
                          ? null
                          : () => probe(),
                      icon: const Icon(Icons.fact_check_outlined, size: 16),
                      label: const Text('校准思考档位'),
                    ),
                  ],
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _Tag extends StatelessWidget {
  final String text;
  const _Tag(this.text);
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: theme.colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text, style: theme.textTheme.labelSmall),
    );
  }
}
