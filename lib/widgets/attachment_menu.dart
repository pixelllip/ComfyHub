import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';

import '../core/file_clipboard.dart';
import '../state/ai_workspace_store.dart';
import 'app_menu.dart' show MenuOption;
import 'common.dart' show copyToClipboard, showContextMenuAt;

/// 附件的右键菜单（消息气泡 / 输入区托盘 / 附件详情弹窗共用）。
///
/// 与画廊详情页的图片右键**同一套动作和文案**：复制文件地址 / 复制文件 / 复制文件名。
/// 附件在气泡里只能点开预览：想把它拿到别处用（粘进资源管理器、把地址发给别人），
/// 以前没有入口。
///
/// [onRemove] 非空时多一项「移除附件」：只有**还没发出去**的托盘附件能移除。
/// [source] 的 context 必须落在 `ContextMenuScope` 之内 —— 页面里的气泡 / 输入区由页面
/// 的 scope 兜住；**弹窗（详情）必须自己再挂一个**（弹窗在根 Navigator 的 overlay 里，
/// 找不到页面那个 scope）。
///
/// 「复制文件」需要磁盘绝对路径，走 `GET /api/ai/attachments/{id}/path`（由 store 转一手）；
/// 拿不到 store（例如独立预览的单元测试）或后端回 404 时只提示原因，绝不假装复制成功。
Future<void> showAttachmentContextMenu(
  BuildContext source, {
  required Offset globalPosition,
  required String name,
  required String? attachmentId,
  required String? fileUrl,
  VoidCallback? onRemove,
}) async {
  AiWorkspaceStore? store;
  try {
    store = source.read<AiWorkspaceStore>();
  } catch (_) {
    // 没有 Provider 的场景（独立预览 / 单测）：复制文件那项给不出路径，其余照常。
    store = null;
  }
  final messenger = ScaffoldMessenger.of(source);
  final action = await showContextMenuAt<String>(
    source,
    globalPosition: globalPosition,
    items: (_) => [
      if (fileUrl != null)
        const MenuOption(value: 'copyUrl', icon: Icons.link, label: '复制文件地址'),
      // 复制**文件本身**（写 `CF_HDROP`）：资源管理器里能直接粘出文件。
      // 只有 Windows 有这条路（见 `lib/core/file_clipboard.dart`）。
      if (attachmentId != null && FileClipboard.supported)
        const MenuOption(
          value: 'copyFile',
          icon: Icons.file_copy_outlined,
          label: '复制文件',
        ),
      const MenuOption(
        value: 'copyName',
        icon: Icons.description_outlined,
        label: '复制文件名',
      ),
      if (onRemove != null)
        const MenuOption(
          value: 'remove',
          icon: Icons.close,
          label: '移除附件',
          danger: true,
          dividerBefore: true,
        ),
    ],
  );
  // 菜单是异步关掉的：先确认这个 context 还挂着（复制要拿它弹提示）。
  if (action == null || !source.mounted) return;

  switch (action) {
    case 'copyUrl':
      await copyToClipboard(source, fileUrl!, label: '文件地址');
      break;
    case 'copyFile':
      if (attachmentId == null || store == null) {
        messenger.showSnackBar(const SnackBar(content: Text('拿不到这个附件的磁盘路径')));
        return;
      }
      final String path;
      try {
        path = await store.attachmentPath(attachmentId);
      } catch (e) {
        // 后端把两种失败都说清楚了（不存在 / 文件不在盘上），原样显示。
        messenger.showSnackBar(SnackBar(content: Text('拿不到磁盘路径，$e')));
        return;
      }
      if (path.isEmpty) {
        messenger.showSnackBar(const SnackBar(content: Text('这个附件的文件不在盘上')));
        return;
      }
      final ok = await FileClipboard.writeFiles([path]);
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            ok ? '已复制文件，可以在资源管理器里 Ctrl+V 粘贴' : '复制文件失败，可以改用「复制文件地址」',
          ),
        ),
      );
      break;
    case 'copyName':
      await copyToClipboard(source, name, label: '文件名');
      break;
    case 'remove':
      onRemove?.call();
      break;
  }
}
