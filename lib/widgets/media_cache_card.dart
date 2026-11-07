import 'package:flutter/material.dart';

import '../core/formatting.dart';
import '../core/media_cache.dart';

/// 设置页「产物缓存」那张卡（**手机端**）。
///
/// 抽成独立 widget 而不是写在 `settings_page.dart` 里，是为了**能被直接测**：
/// 那一节只在手机上渲染（`!isWindowsDesktop`），而 `flutter test` 跑在 Windows 上，
/// 留在页面里的实现一条用例都覆盖不到 —— 320dp 会不会溢出就没人知道了。
///
/// 这里只负责"看得见"：占用、上限、清理。真正的淘汰在 `MediaCache` 里做
/// （按总字节 LRU，超上限淘汰到 80%）。
class MediaCacheCard extends StatelessWidget {
  const MediaCacheCard({
    super.key,
    required this.stats,
    required this.budgetMb,
    required this.busy,
    required this.onBudgetChanged,
    required this.onClear,
  });

  /// 占用快照；`null` = 还没统计完。
  final MediaCacheStats? stats;

  /// 当前选中的原件上限（MB），必须是 `kOriginalCacheBudgetChoicesMb` 里的一项。
  final int budgetMb;

  /// 清理中 / 正在改设置：按钮与下拉都禁用（避免连点把两次清理交叉）。
  final bool busy;

  final ValueChanged<int> onBudgetChanged;
  final VoidCallback onClear;

  /// 档位标签：≥1024 的用 GB 显示（1.5GB 比 1536MB 好读）。
  static String budgetLabel(int mb) {
    if (mb < 1024) return '$mb MB';
    final gb = mb / 1024;
    return gb == gb.roundToDouble() ? '${gb.toInt()} GB' : '$gb GB';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final snapshot = stats;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (snapshot == null)
              const Row(
                children: [
                  SizedBox(
                    width: 16,
                    height: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                  SizedBox(width: 10),
                  Text('正在统计…'),
                ],
              )
            else ...[
              Text(
                '已用 ${formatSize(snapshot.totalBytes)} / '
                '上限 ${formatSize(snapshot.totalBudget)}',
                style: theme.textTheme.titleSmall,
              ),
              const SizedBox(height: 4),
              Text(
                '缩略图 ${formatSize(snapshot.thumbBytes)}'
                '（${snapshot.thumbCount} 张，上限 ${formatSize(snapshot.thumbBudget)}） · '
                '原件 ${formatSize(snapshot.origBytes)}（${snapshot.origCount} 条）',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              // 读不到目录时上面的数字全是 0，必须说清"不是没占空间，是读不出来"
              if (snapshot.error != null) ...[
                const SizedBox(height: 4),
                Text(
                  '缓存目录读不出来，上面的数字不可信（${snapshot.error}）。',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.error,
                  ),
                ),
              ],
            ],
            const SizedBox(height: 12),
            // `Wrap` 而不是 `Row`：320dp 上"原件上限 + 下拉 + 清理"放不下一行，
            // 折行是设计好的形态（不是溢出）。
            Wrap(
              spacing: 12,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text('原件上限', style: theme.textTheme.bodyMedium),
                    const SizedBox(width: 8),
                    DropdownButton<int>(
                      value: budgetMb,
                      isDense: true,
                      onChanged: busy
                          ? null
                          : (v) {
                              if (v != null) onBudgetChanged(v);
                            },
                      items: [
                        for (final mb in kOriginalCacheBudgetChoicesMb)
                          DropdownMenuItem(
                            value: mb,
                            child: Text(budgetLabel(mb)),
                          ),
                      ],
                    ),
                  ],
                ),
                OutlinedButton.icon(
                  onPressed: busy || snapshot == null ? null : onClear,
                  icon: busy
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.delete_sweep_outlined),
                  label: const Text('清理缓存'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              '缓存只保留看过的内容，缩略图在格子出现时下载，原件在打开详情时下载。'
              '太大的视频 / 音频不会自动下载，点「保存到手机相册」才会下。'
              '超过上限按最久未用淘汰到 80%。清理只删本机这些副本，'
              '桌面上的原件与已保存进相册的文件都不受影响。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
