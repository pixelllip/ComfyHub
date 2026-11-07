import 'package:flutter/material.dart';

import '../models/ai_models.dart';
import 'ai_provider_widgets.dart';

/// 「Provider 列表」子页：新建 / 从电脑同步 / 一览。
///
/// 从 `ai_provider_settings_page.dart`（当时 2,956 行）拆出来的第一块。
/// 桌面端它是左栏，手机端它自己就是一整屏 —— 两处共用这一个组件。
class AiProviderListPanel extends StatelessWidget {
  final List<AiProvider> providers;
  final String? selectedId;
  final bool loading;

  /// 这份清单是不是**本机**的（手机）：是的话要说一句"桌面看不到它"。
  final bool local;
  final Future<void> Function(AiProvider?) onSelect;
  final Future<void> Function() onCreate;
  final Future<void> Function(AiProvider) onDelete;

  /// 「从电脑同步 Provider」（`docs/pairing-design.md` §14.6）。
  ///
  /// **`null` = 这一端不该有它**（桌面上后端库自己就是真源）—— 按钮与那句说明一起不渲染，
  /// 而不是渲染一个点了没反应的按钮。
  final Future<void> Function()? onSync;

  /// 同步正在跑：按钮转圈 + 禁用（再点一次不叠第二趟）。
  final bool syncing;

  const AiProviderListPanel({
    super.key,
    required this.providers,
    required this.selectedId,
    required this.loading,
    required this.local,
    required this.onSelect,
    required this.onCreate,
    required this.onDelete,
    this.onSync,
    this.syncing = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              FilledButton.icon(
                onPressed: () => onCreate(),
                icon: const Icon(Icons.add, size: 18),
                label: const Text('新建 Provider'),
              ),
              if (onSync != null) ...[
                const SizedBox(height: 8),
                OutlinedButton.icon(
                  // 同步进行中禁用：再点一次会在本机库上叠第二趟写
                  onPressed: syncing ? null : () => onSync!(),
                  icon: syncing
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.cloud_download_outlined, size: 18),
                  label: const Text('从电脑同步 Provider'),
                ),
              ],
            ],
          ),
        ),
        // ⚠ **如实**：手机上这一页管的是**本机那份清单**（`ai_providers` / `ai_models`
        //    是手机的本机表），它与桌面那份**各归各、不自动合并** —— 同步的只有密钥明文
        //    （用户显式点「推给桌面」/「从桌面同步密钥」那些动作，`docs/pairing-design.md` §14）。
        //    不写这一句，用户建完会以为桌面也能用（而桌面那边压根没有这条 provider）。
        //    §14.6 又加了一条**显式**的搬运通道，所以这句要说清"要手动点"，免得它被读成"会自动同步"。
        if (local)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
            child: AiNote(
              text:
                  '这些 Provider 只在这台手机上，存在本机库里，桌面看不到它们。'
                  '两端的清单各归各，不会自动合并。桌面那份要手动点「从电脑同步 Provider」搬过来，'
                  '同 id 会被覆盖，只在手机上有的保留不动，API Key 不在其中。'
                  '桌面不在线时也能在这里新建、编辑、删除和改模型目录。',
              error: false,
            ),
          ),
        if (loading) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: providers.isEmpty
              ? Center(
                  child: Text(
                    '还没有 Provider',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                )
              : ListView.builder(
                  itemCount: providers.length,
                  itemBuilder: (context, i) {
                    final p = providers[i];
                    return ListTile(
                      dense: true,
                      selected: p.id == selectedId,
                      leading: AiCredentialDot(status: p.credential),
                      title: Text(
                        p.displayName,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: Text(
                        '${p.apiLabel} · ${p.endpointTrust}',
                        style: theme.textTheme.labelSmall,
                      ),
                      onTap: () => onSelect(p),
                      trailing: IconButton(
                        tooltip: '删除',
                        icon: const Icon(Icons.delete_outline, size: 18),
                        onPressed: () => onDelete(p),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}
