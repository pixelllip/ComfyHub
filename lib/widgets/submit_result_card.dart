import 'package:flutter/material.dart';

import '../models/ai_models.dart';

/// 「手机排队提交」的回执卡。
///
/// # 它解决的是"静默失败"
///
/// 桌面 `submit_queue.rs` 在每条终态都会往会话里插一条 id 为
/// `submit-reply-<queueId>` 的助手消息：正文是一句人话，另有一个
/// `tool_result` 块（`ok` / `code` / `mediaIds` / `queueId`）。以前这条消息
/// 只会按普通 Markdown 正文显示，和聊天里的一句解释长得一模一样 ——
/// 已批准但提交失败时，界面显示为"没有反应"（2026-11-08 真机）。
///
/// 这里把它画成**独立的提交结果卡**：
///  * 成功：绿色对勾 + 产物缩略图（点开进详情）；
///  * 失败：红色错误图标 + **原因原文**（可滚动、不截断、不吞）；
///  * 没有结果标记的旧数据：如实说"没有结果标记"，不猜成功还是失败。
///
/// 判据只有消息 id 前缀 `submit-reply-`（后端 `reply_message_id()` 的
/// 确定性 id）；正文/内容里出现"失败"两个字**不算**判据。
class SubmitResultCard extends StatelessWidget {
  final AiMessage message;

  /// 画廊产物 id → 缩略图地址（与 `_ProducedMediaCard` 同一个来源）。
  final String Function(int mediaId) mediaThumbUrlOf;

  /// 拉缩略图时带的请求头（配对令牌）。
  ///
  /// 缩略图地址里**没有**令牌（令牌不进 URL），而手机读的是电脑的后端：闸门开着时
  /// 不带这个头的请求一律 403，界面上的表现就是"回执卡是破图"（2026-11-09 真机）。
  /// 桌面上它恒为空 Map，请求与以前逐字节相同。
  final Map<String, String> mediaHeaders;

  /// 点缩略图 / 「查看详情」时打开产物详情；为空时不给入口。
  final void Function(int mediaId)? onOpenMedia;

  const SubmitResultCard({
    super.key,
    required this.message,
    required this.mediaThumbUrlOf,
    this.mediaHeaders = const {},
    this.onOpenMedia,
  });

  /// 回执的 `tool_result` 块（后端 `compose_reply()` 写的那一份）。
  ///
  /// 只认 `tool_result`：`tool_call` 块是模型请求提交的参数，不是结果。
  Map<String, dynamic>? get _payload {
    for (final part in message.parts) {
      if (part.type == 'tool_result' && part.payload != null) {
        return part.payload;
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final payload = _payload;
    final ok = payload?['ok'] == true;
    final failed = payload != null && !ok;
    final code = (payload?['code'] ?? '').toString().trim();
    final mediaIds =
        (payload?['mediaIds'] as List?)
            ?.whereType<num>()
            .map((e) => e.toInt())
            .toList() ??
        const <int>[];

    final color = failed
        ? theme.colorScheme.error
        : ok
        ? theme.colorScheme.primary
        : theme.colorScheme.outline;
    final icon = failed
        ? Icons.error_outline
        : ok
        ? Icons.check_circle_outline
        : Icons.info_outline;
    final title = failed
        ? '提交失败'
        : ok
        ? '提交完成'
        : '提交结果';

    final body = message.text.trim();

    return Container(
      key: ValueKey('submit-result-${message.id}'),
      width: double.infinity,
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(icon, size: 18, color: color),
              const SizedBox(width: 6),
              Text(
                title,
                style: theme.textTheme.labelLarge?.copyWith(
                  color: color,
                  fontWeight: FontWeight.w600,
                ),
              ),
              if (code.isNotEmpty) ...[
                const SizedBox(width: 8),
                Flexible(
                  child: Text(
                    code,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
              ],
            ],
          ),
          if (body.isNotEmpty) ...[
            const SizedBox(height: 6),
            // 失败原因可能是一整段 HTTP 400 的 JSON：给一个可滚动的框，
            // **不截断、不省略**（"原因原文，不要吞"）。
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 220),
              child: SingleChildScrollView(
                child: SelectableText(
                  body,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: failed
                        ? theme.colorScheme.onErrorContainer
                        : theme.colorScheme.onSurface,
                  ),
                ),
              ),
            ),
          ],
          if (mediaIds.isNotEmpty) ...[
            const SizedBox(height: 8),
            SizedBox(
              height: 72,
              child: ListView.builder(
                scrollDirection: Axis.horizontal,
                itemCount: mediaIds.length,
                itemBuilder: (context, i) {
                  final id = mediaIds[i];
                  return Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: InkWell(
                      key: ValueKey('submit-result-media-$id'),
                      onTap: onOpenMedia == null
                          ? null
                          : () => onOpenMedia!(id),
                      borderRadius: BorderRadius.circular(8),
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(8),
                        child: SizedBox(
                          width: 72,
                          height: 72,
                          child: Image.network(
                            mediaThumbUrlOf(id),
                            headers: mediaHeaders,
                            fit: BoxFit.cover,
                            errorBuilder: (_, _, _) => Container(
                              color: theme.colorScheme.surfaceContainerHighest,
                              child: Icon(
                                Icons.image_not_supported_outlined,
                                size: 18,
                                color: theme.colorScheme.outline,
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
            ),
            if (onOpenMedia != null)
              TextButton.icon(
                key: const Key('submit-result-open'),
                onPressed: () => onOpenMedia!(mediaIds.first),
                icon: const Icon(Icons.open_in_new, size: 15),
                label: const Text('查看产物'),
              ),
          ],
          if (payload == null) ...[
            const SizedBox(height: 6),
            Text(
              '这条回执没有结果标记（旧数据），不知道成功还是失败。',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ],
      ),
    );
  }
}
