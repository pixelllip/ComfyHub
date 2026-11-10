import 'package:flutter/material.dart';

import '../core/ai_api_client.dart';
import '../core/credential_store.dart';
import '../core/inbound_auth_api.dart';
import '../core/provider_store.dart';
import '../models/ai_models.dart';
import 'ai_provider_credentials_section.dart';
import 'ai_provider_models_section.dart';
import 'ai_provider_widgets.dart';

/// 一条 Provider 的详情（桌面右栏 / 手机详情子页）。
///
/// 从 `ai_provider_settings_page.dart`（当时 2,956 行）拆出来的装配层：
/// 它只负责"信息头 + 凭据 + 模型目录"三块的顺序，真正的状态各自在
/// [AiProviderCredentialsSection] 与 [AiProviderModelsSection] 里。
///
/// 以前这三块挤在同一个 800 行的 `State` 里，密钥输入框、连接测试、模型目录的
/// `_busy` 与 `_dirty` 互相牵连；拆开之后"改密钥"不会再把模型列表重建一遍。
class AiProviderDetail extends StatelessWidget {
  final AiApiClient api;
  final CredentialStore credentials;

  /// 清单来源（手机 = 本机库）：**模型目录的保存**走它，连接的测试/发现看它的 [ProviderStore.supportsProbe]。
  final ProviderStore providers;
  final InboundAuthApi Function(String baseUrl)? authApiFactory;
  final AiProvider provider;
  final List<AiModel> models;
  final String? info;
  final String? error;

  /// 正文里要不要再摆一行 Provider 名字 + 「编辑 Provider」。
  ///
  /// 手机上这一条 Provider 的**页面大标题**就是它的名字（`_buildNarrow` 的详情
  /// 面板），正文再摆一次 = 同一个标题出现两次；那里改由 AppBar 的编辑图标负责。
  /// 桌面是双栏，右边这不是页面标题，所以照旧保留。
  final bool showName;

  /// 「模型能力目录」那一块（在线数据集 = **models.dev**）。
  final AiCapabilityStatus? capability;
  final bool capabilityBusy;
  final bool probeBusy;
  final VoidCallback? onRefreshCapability;
  final VoidCallback? onUpdateCapabilityRemote;
  final ValueChanged<bool>? onProbeToggle;
  final VoidCallback? onProbe;

  /// 「编辑 Provider」（改显示名 / 协议 / baseURL / API Key）。
  /// **不带 revision 的修改不在这里** —— 编辑时用的是这一行的 revision（乐观锁）。
  final Future<void> Function() onEdit;
  final Future<void> Function(String message) onChanged;

  const AiProviderDetail({
    super.key,
    required this.api,
    required this.credentials,
    required this.providers,
    this.authApiFactory,
    required this.provider,
    required this.models,
    required this.info,
    required this.error,
    this.showName = true,
    this.capability,
    this.capabilityBusy = false,
    this.probeBusy = false,
    this.onRefreshCapability,
    this.onUpdateCapabilityRemote,
    this.onProbeToggle,
    this.onProbe,
    required this.onEdit,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = provider;

    final header = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (showName)
          Row(
            children: [
              Expanded(
                child: Text(p.displayName, style: theme.textTheme.titleLarge),
              ),
              // 「修改 baseURL / 协议」的路径（独立配置 provider）：
              // 两端都走同一条 store（手机 = 本机库、桌面 = 后端），提交时带 revision（乐观锁）。
              OutlinedButton.icon(
                onPressed: () => onEdit(),
                icon: const Icon(Icons.edit_outlined, size: 18),
                label: const Text('编辑 Provider'),
              ),
            ],
          ),
        if (showName) const SizedBox(height: 4),
        Text(
          '${p.id} · ${p.apiLabel} · revision ${p.revision}',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        const SizedBox(height: 6),
        SelectableText(p.baseURL, style: theme.textTheme.bodySmall),
        const SizedBox(height: 4),
        Text(
          '端点信任级别是 ${p.endpointTrust}（保存时校验，公网必须是 https）',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        if (info != null) ...[
          const SizedBox(height: 10),
          AiNote(text: info!, error: false),
        ],
        if (error != null) ...[
          const SizedBox(height: 10),
          AiNote(text: error!, error: true),
        ],
        const SizedBox(height: 16),
        AiProviderCredentialsSection(
          api: api,
          credentials: credentials,
          providers: providers,
          authApiFactory: authApiFactory,
          provider: p,
          onChanged: onChanged,
        ),
      ],
    );

    return AiProviderModelsSection(
      header: header,
      api: api,
      providers: providers,
      provider: p,
      models: models,
      capability: capability,
      capabilityBusy: capabilityBusy,
      probeBusy: probeBusy,
      onRefreshCapability: onRefreshCapability,
      onUpdateCapabilityRemote: onUpdateCapabilityRemote,
      onProbeToggle: onProbeToggle,
      onProbe: onProbe,
      onChanged: onChanged,
    );
  }
}
