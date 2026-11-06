import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/settings_store.dart';
import '../core/system_accent.dart';
import '../core/theme.dart';
import '../state/library_store.dart';
import '../widgets/settings_scaffold.dart';

/// 「外观与布局」子页：主题 / 取色 / 每页数量 / 集合详情。
///
/// 2026-11-05 从设置首页搬出来（用户：一张卡里超过两项的配置收进子界面）。
/// 后来又按用户"能拆开的卡片要拆开"的意见，把原来一张"外观"大卡拆成
/// **主题 / 系统主题色 / 列表** 三块 —— 宽窗口下 [SettingsScaffold] 才能把它们
/// 分到不同列里，而不是一整张长卡占住一列。
class InterfaceSettingsPage extends StatelessWidget {
  const InterfaceSettingsPage({super.key});

  /// 重新读一次平台强调色：用户在系统里换了颜色，不用重启 App 就能看到。
  Future<void> _redetectAccent(BuildContext context) async {
    final messenger = ScaffoldMessenger.of(context);
    await context.read<SettingsStore>().refreshSystemAccent();
    if (!context.mounted) return;
    messenger.showSnackBar(SnackBar(content: Text(SystemAccent.sourceLabel)));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final store = context.watch<LibraryStore>();

    return SettingsScaffold(
      title: '外观与布局',
      sections: [
        SettingsSection(
          title: '主题',
          estimatedHeight: 150,
          child: Card(
            child: ListTile(
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 8,
              ),
              title: const Text('明暗模式'),
              subtitle: const Text('深色 / 浅色 / 跟随系统'),
              trailing: SegmentedButton<AppThemeMode>(
                segments: [
                  for (final mode in AppThemeMode.values)
                    ButtonSegment(value: mode, label: Text(mode.label)),
                ],
                selected: {settings.themeMode},
                onSelectionChanged: (picked) =>
                    settings.setThemeMode(picked.first),
                showSelectedIcon: false,
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
              ),
            ),
          ),
        ),
        // 系统主题色 / 取色档位：Material You 的"个人化"就在这里。
        // 点标题那一行 = 重新检测（系统里换了颜色不用重启 App）。
        SettingsSection(
          title: '系统主题色',
          estimatedHeight: 270,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  ListTile(
                    contentPadding: EdgeInsets.zero,
                    onTap: () => _redetectAccent(context),
                    leading: Container(
                      width: 12,
                      height: 12,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: SystemAccent.color ?? AppTheme.defaultSeed,
                        border: Border.all(
                          color: theme.colorScheme.outlineVariant,
                        ),
                      ),
                    ),
                    title: const Text('取色档位'),
                    subtitle: Text(
                      SystemAccent.available
                          ? '${SystemAccent.sourceLabel}。点这一行重新检测。'
                          : '${SystemAccent.sourceLabel}（换台设备 / 升级系统后点这一行再试）。',
                    ),
                  ),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: SegmentedButton<AppAccentMode>(
                      segments: [
                        for (final mode in AppAccentMode.values)
                          ButtonSegment(value: mode, label: Text(mode.label)),
                      ],
                      selected: {settings.accentMode},
                      onSelectionChanged: (picked) =>
                          settings.setAccentMode(picked.first),
                      showSelectedIcon: false,
                      style: const ButtonStyle(
                        visualDensity: VisualDensity.compact,
                      ),
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    settings.accentMode.description,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '列表',
          estimatedHeight: 150,
          child: Card(
            child: ListTile(
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 8,
              ),
              title: const Text('每页数量'),
              subtitle: Text('当前 ${settings.pageSize} 条'),
              trailing: SizedBox(
                width: 180,
                child: Slider(
                  value: settings.pageSize.toDouble(),
                  min: 8,
                  max: 96,
                  divisions: 11,
                  label: '${settings.pageSize}',
                  onChanged: (v) => settings.setPageSize(v.round()),
                  onChangeEnd: (_) => store.refreshAll(),
                ),
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '画廊',
          estimatedHeight: 280,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Column(
                children: [
                  ListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('宽屏下的集合详情'),
                    subtitle: const Text(
                      '窗口宽度到 900 以上时用哪种。停靠面板会占住真实布局，画廊网格自己收窄，'
                      '谁也不遮谁。居中弹窗浮在画廊上，背景虚化。',
                    ),
                    trailing: SegmentedButton<CollectionDetailStyle>(
                      segments: [
                        for (final style in CollectionDetailStyle.values)
                          ButtonSegment(value: style, label: Text(style.label)),
                      ],
                      selected: {settings.collectionDetailWide},
                      onSelectionChanged: (picked) =>
                          settings.setCollectionDetailWide(picked.first),
                      showSelectedIcon: false,
                      style: const ButtonStyle(
                        visualDensity: VisualDensity.compact,
                      ),
                    ),
                  ),
                  const Divider(height: 1),
                  ListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('窄屏下的集合详情'),
                    subtitle: const Text(
                      '窗口宽度不到 900 时用哪种。停靠面板会占住真实布局，画廊网格自己收窄，'
                      '窗口很窄时会被挤得很小。居中弹窗浮在画廊上，背景虚化。',
                    ),
                    trailing: SegmentedButton<CollectionDetailStyle>(
                      segments: [
                        for (final style in CollectionDetailStyle.values)
                          ButtonSegment(value: style, label: Text(style.label)),
                      ],
                      selected: {settings.collectionDetailNarrow},
                      onSelectionChanged: (picked) =>
                          settings.setCollectionDetailNarrow(picked.first),
                      showSelectedIcon: false,
                      style: const ButtonStyle(
                        visualDensity: VisualDensity.compact,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}
