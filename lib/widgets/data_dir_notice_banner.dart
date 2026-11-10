/// **"库不在你以为的地方"** 的常驻横幅（2026-11-10 用户报的那次事故）。
///
/// # 为什么要有它
///
/// 用户的 `data-dir.json` 指着 `D:\dist\ComfyHub`，真正的库却在
/// `D:\dist\ComfyHub\storage\.db\comfyhub.db`；指针那一级也有一份库，但是上一份构建
/// 残留的**旧库**。App 照着指针打开旧库 —— 界面表现是"对话不见了、角色卡是旧版"，
/// 而磁盘上一个字节都没丢。
///
/// `backend_paths.dart` 的 `resolveDataDir` 已经把目录**校正**过来了；但"校正过"这件事
/// 必须看得见（AGENTS §12.3：界面上看不见的现象，用户和模型就只能靠猜）。所以：
///
/// * 只在校正过 / 产物目录不见了 / 指针失效时出现，正常时一个像素都不占；
/// * 文案来自 `DataDirResolution.notice`（**唯一一份**，界面不另写）；
/// * 可以关掉（同一条问题关一次就够；换成别的问题会再出现）；
/// * 给一个「去设置」的出口 —— 那里能看实际路径、也能把指针改到实际数据上。
library;

import 'package:flutter/material.dart';

class DataDirNoticeBanner extends StatefulWidget {
  /// 要说的那句话。null / 空 = 什么都不显示。
  final String? notice;

  /// 点「去设置」时的回调（一般是切到设置页）。
  final VoidCallback? onOpenSettings;

  const DataDirNoticeBanner({
    super.key,
    required this.notice,
    this.onOpenSettings,
  });

  @override
  State<DataDirNoticeBanner> createState() => _DataDirNoticeBannerState();
}

class _DataDirNoticeBannerState extends State<DataDirNoticeBanner> {
  /// 已经关掉的是哪一句话。换了内容就重新出现 —— 关的只是"这一次的问题"。
  String? _dismissed;

  @override
  Widget build(BuildContext context) {
    final notice = widget.notice;
    if (notice == null || notice.isEmpty || notice == _dismissed) {
      return const SizedBox.shrink();
    }

    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final onContainer = scheme.onTertiaryContainer;
    return Material(
      color: scheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(Icons.folder_off_outlined, size: 20, color: onContainer),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    '数据目录提醒',
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: onContainer,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    notice,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: onContainer,
                    ),
                  ),
                ],
              ),
            ),
            if (widget.onOpenSettings != null)
              TextButton(
                onPressed: widget.onOpenSettings,
                child: Text('去设置', style: TextStyle(color: onContainer)),
              ),
            IconButton(
              tooltip: '知道了',
              icon: Icon(Icons.close, size: 18, color: onContainer),
              onPressed: () => setState(() => _dismissed = notice),
            ),
          ],
        ),
      ),
    );
  }
}
