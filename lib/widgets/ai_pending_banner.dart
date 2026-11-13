import 'package:material_ui/material_ui.dart';

import '../models/ai_models.dart';

/// 「待审批」全局横幅（桌面端 AI 工作台）。
///
/// # 用途
///
/// 手机排上来的提交（`submit:<queueId>`）不在任何本机 Run 的 SSE 里，只能靠
/// `GET /api/ai/pending` 获取。右侧栏「待审批」卡仅在**上下文面板打开时**
/// 可见；窄屏 / 面板收起时无法得知有待批准的提交 —— 该情况下会错过 300 秒等待，
/// 提交超时被拒，界面表现为"点击无反应"。
///
/// 这条横幅钉在对话顶部：**有 approvals 就出现**（条数 + 其中几项是手机排队
/// 的提交），点「去处理」跳到对应会话；**没有就整块不占位**（不画空条）。
///
/// 它只负责"画"：轮询与跳转由页面里的 `_PendingApprovalBanner` 负责，
/// 这样这个组件在 widget 测试里不碰网络。
class AiPendingBanner extends StatelessWidget {
  final List<AiPendingApproval> approvals;

  /// 点「去处理」时跳到**第一条**待审批所在的会话。
  final ValueChanged<AiPendingApproval> onOpen;

  const AiPendingBanner({
    super.key,
    required this.approvals,
    required this.onOpen,
  });

  @override
  Widget build(BuildContext context) {
    // 没有待办：整块不占位（连间距都不留）。
    if (approvals.isEmpty) return const SizedBox.shrink();

    final theme = Theme.of(context);
    final queued = approvals.where((a) => a.isQueuedSubmit).length;
    final label = queued > 0
        ? '${approvals.length} 项待审批，其中 $queued 项是手机排队的提交'
        : '${approvals.length} 项待审批';

    return Material(
      key: const Key('ai-pending-banner'),
      color: theme.colorScheme.errorContainer.withValues(alpha: 0.55),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
        child: Row(
          children: [
            Icon(
              Icons.pending_actions,
              size: 18,
              color: theme.colorScheme.onErrorContainer,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.labelMedium?.copyWith(
                  color: theme.colorScheme.onErrorContainer,
                ),
              ),
            ),
            TextButton(
              key: const Key('ai-pending-banner-open'),
              onPressed: () => onOpen(approvals.first),
              style: TextButton.styleFrom(
                foregroundColor: theme.colorScheme.onErrorContainer,
                visualDensity: VisualDensity.compact,
              ),
              child: const Text('去处理'),
            ),
          ],
        ),
      ),
    );
  }
}
