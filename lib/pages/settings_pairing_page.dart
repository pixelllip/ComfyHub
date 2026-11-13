import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';

import '../core/settings_store.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/settings_scaffold.dart';
import 'pairing_section.dart';

/// 「远程访问与配对」子页。
///
/// 2026-11-05 从设置首页搬出来（它一面墙有：配对闸门、二维码、设备列表、
/// 同步密钥…… 远超"一张卡的容量"）。内容本身仍然是 [PairingSection]，
/// 这里只负责给它一个独立的页面与顶栏。
class PairingSettingsPage extends StatelessWidget {
  /// 设置搜索跳转到的区块（当前只有 `pairing`）。null = 从页首开始。
  final String? initialSectionId;

  const PairingSettingsPage({super.key, this.initialSectionId});

  @override
  Widget build(BuildContext context) {
    final settings = context.watch<SettingsStore>();

    return SettingsScaffold(
      title: '远程访问与配对',
      initialSectionId: initialSectionId,
      sections: [
        SettingsSection(
          title: '配对',
          id: 'pairing',
          estimatedHeight: 720,
          child: PairingSection(
            baseUrl: settings.baseUrl,
            // 配对成功 / 忘记这台电脑之后：让 AI 工作台重新读一次令牌，
            // 再取一次规则工件（配对前那次失败很可能就是因为没有令牌）。
            //
            // ⚠ `read<AiWorkspaceStore?>()`（可空）：设置页的用例不一定注册这个
            // store —— 取不到就什么也不做，配对本身照样成功。
            onPaired: () async {
              final store = context.read<AiWorkspaceStore?>();
              await store?.reloadPairingToken();
            },
            onSyncCredentials: () async {
              final store = context.read<AiWorkspaceStore?>();
              if (store == null) return 'AI 工作台还没准备好，稍后再试一次。';
              return store.syncCredentialsFromDesktop();
            },
            onSaveBaseUrl: settings.setBaseUrl,
          ),
        ),
      ],
    );
  }
}
