import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';

import '../core/backend_launcher.dart';
import '../core/backend_paths.dart';
import '../core/formatting.dart';
import '../core/layout_constants.dart';
import '../core/media_cache.dart';
import '../core/platform_caps.dart';
import '../core/settings_store.dart';
import '../state/library_store.dart';
import '../state/nsfw_store.dart';
import '../widgets/export_dialog.dart';
import '../widgets/media_cache_card.dart';
import '../widgets/scroll_linked_title.dart';
import '../widgets/settings_desktop_shell.dart';
import '../widgets/settings_scaffold.dart';
import '../widgets/settings_switch_row.dart';
import 'ai_provider_settings_page.dart';
import 'ai_tools_settings_page.dart';
import 'settings_comfy_capture_page.dart';
import 'settings_data_dir_page.dart';
import 'settings_interface_page.dart';
import 'settings_local_services_page.dart';
import 'settings_mobile_local_page.dart';
import 'settings_pairing_page.dart';
import 'settings_search_page.dart';

/// 设置首页。
///
/// 两套形态，判据是**平台 + 宽度**：
///   * 桌面端且宽度 ≥ `kWideLayoutMinWidth`：左侧只有大项主标题的分类栏，
///     右侧是该分类的主体界面，次级页面也只替换右侧（[SettingsDesktopShell]）；
///   * 其余（手机 / 平板 / 桌面窄窗口）：原来的大标题 + 分组入口行，点进子页。
///
/// 分类本身仍是八个大项，首页不再重复罗列入口：
/// 概览（NSFW / 库统计 / 导出）、本地服务、远程访问与配对、AI 模型与凭据、
/// AI 工具权限、ComfyUI 自动捕获、外观与布局、数据目录。
class SettingsPage extends StatefulWidget {
  /// 设置搜索跳转到某一项时传入（例如 `nsfw` / `cache`）。
  final String? initialSectionId;

  /// 被搜索结果当作独立页面打开时需要返回键；作为导航的一页时不需要。
  final bool showBack;

  const SettingsPage({super.key, this.initialSectionId, this.showBack = false});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  void _push(BuildContext context, Widget page) {
    Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => page));
  }

  /// 打开独立的结果页：从结果点进去的目标页返回时，先回到这一页。
  void _openSearch(BuildContext context, bool isWindowsDesktop) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => _searchPage(isWindowsDesktop),
      ),
    );
  }

  /// 搜索结果页。它被推到**当前分类的右侧 Navigator** 里（桌面端），
  /// 所以左侧分类栏保持可见；窄屏才落到整页。
  Widget _searchPage(bool isWindowsDesktop) => SettingsSearchPage(
    isWindowsDesktop: isWindowsDesktop,
    homePageBuilder: (sectionId) => PreferencesSettingsPage(
      // 顶层的「偏好」条目没有具体小节；NSFW / 缓存才按 id 定位。
      initialSectionId: sectionId == 'preferences' ? null : sectionId,
      showBack: true,
    ),
  );

  @override
  Widget build(BuildContext context) {
    // 判据只有一份：`isWindowsDesktopHost`（带单测用的平台 override）。
    final isWindowsDesktop = isWindowsDesktopHost;
    // 宽窄与导航形态用**同一个数字**：< 900 就是手机形态，电脑端窄屏也一样。
    final wide = MediaQuery.sizeOf(context).width >= kWideLayoutMinWidth;

    // 桌面端宽屏重做为"分类栏 + 右侧正文"。
    // 手机 / 平板即使窗口够宽也不换这套：大标题随滚动的形态由**平台**决定
    // （`useCollapsingPageTitle`），不是窗口宽度。
    if (wide && !useCollapsingPageTitle) {
      return SettingsDesktopShell(
        items: _desktopItems(isWindowsDesktop),
        initialId: 'overview',
        showBack: widget.showBack,
        searchPageBuilder: (_) => _searchPage(isWindowsDesktop),
        onBack: () => Navigator.of(context).maybePop(),
      );
    }

    return SettingsScaffold(
      // 首页在 `HomeShell` 的内容区里，不需要返回箭头。
      title: '设置',
      showBack: widget.showBack,
      initialSectionId: widget.initialSectionId,
      header: SettingsSearchBar(
        onTap: () => _openSearch(context, isWindowsDesktop),
      ),
      sections: _narrowSections(isWindowsDesktop),
    );
  }

  /// 桌面分类栏：**只有大项主标题**（图标 + 一行字，没有副标题）。
  ///
  /// 顺序与手机端的入口顺序一致；平台差异只在"本地服务 / 本机数据"与
  /// "数据目录"两项上（手机连的是桌面后端，本机没有可拉起的服务）。
  List<SettingsNavItem> _desktopItems(bool isWindowsDesktop) {
    return [
      SettingsNavItem(
        id: 'overview',
        label: '概览',
        icon: Icons.space_dashboard_outlined,
        builder: (_) => const _SettingsOverviewPane(),
      ),
      if (isWindowsDesktop)
        SettingsNavItem(
          id: 'services',
          label: '本地服务',
          icon: Icons.dns_outlined,
          builder: (_) => const LocalServicesSettingsPage(),
        )
      else
        SettingsNavItem(
          id: 'mobile-local',
          label: '本机数据与同步',
          icon: Icons.phonelink_setup,
          builder: (_) => const MobileLocalSettingsPage(),
        ),
      SettingsNavItem(
        id: 'pairing',
        label: '远程访问与配对',
        icon: Icons.qr_code_2,
        builder: (_) => const PairingSettingsPage(),
      ),
      SettingsNavItem(
        id: 'ai-providers',
        label: 'AI 模型与凭据',
        icon: Icons.smart_toy_outlined,
        builder: (_) => const AiProviderSettingsPage(),
      ),
      SettingsNavItem(
        id: 'ai-tools',
        label: 'AI 工具权限',
        icon: Icons.shield_outlined,
        builder: (_) => const AiToolsSettingsPage(),
      ),
      SettingsNavItem(
        id: 'capture',
        label: 'ComfyUI 自动捕获',
        icon: Icons.sync_alt,
        builder: (_) => const ComfyCaptureSettingsPage(),
      ),
      SettingsNavItem(
        id: 'interface',
        label: '外观与布局',
        icon: Icons.palette_outlined,
        builder: (_) => const InterfaceSettingsPage(),
      ),
      SettingsNavItem(
        id: 'preferences',
        label: '偏好',
        icon: Icons.tune,
        builder: (_) => const PreferencesSettingsPage(),
      ),
      if (isWindowsDesktop)
        SettingsNavItem(
          id: 'data-dir',
          label: '数据目录',
          icon: Icons.folder_open_outlined,
          builder: (_) => const DataDirSettingsPage(),
        ),
    ];
  }

  /// 窄屏 / 手机首页：**保持分组入口行**（手机端排版保持不变，
  /// 电脑端窄屏恢复为手机端排版）。
  ///
  /// ⚠ 这一支不要再改成桌面分类栏：窄屏上没有横向空间，分类栏会把入口压成
  /// 一长条，分组反而丢了。
  List<SettingsSection> _narrowSections(bool isWindowsDesktop) {
    final dataDir = context.watch<BackendLauncher>().dataDir;
    return [
      SettingsSection(
        title: '服务',
        id: 'service',
        estimatedHeight: 280,
        child: Column(
          children: [
            if (isWindowsDesktop) ...[
              SettingsEntryTile(
                icon: Icons.dns_outlined,
                title: '本地服务（后端 + 数据库）',
                subtitle: '自动拉起 / 停止、启动与重启、日志、运行根目录、后端地址',
                onTap: () => _push(context, const LocalServicesSettingsPage()),
              ),
              const SizedBox(height: 8),
            ] else ...[
              SettingsEntryTile(
                icon: Icons.phonelink_setup,
                title: '本机数据与同步',
                subtitle: '本机会话与上行队列、桌面连接、立即同步与拉取',
                onTap: () => _push(context, const MobileLocalSettingsPage()),
              ),
              const SizedBox(height: 8),
            ],
            SettingsEntryTile(
              icon: Icons.qr_code_2,
              title: '远程访问与配对',
              subtitle: '配对闸门、二维码、已授权设备、从桌面同步密钥',
              onTap: () => _push(context, const PairingSettingsPage()),
            ),
          ],
        ),
      ),
      SettingsSection(
        title: 'AI',
        id: 'ai',
        estimatedHeight: 280,
        child: Column(
          children: [
            SettingsEntryTile(
              icon: Icons.smart_toy_outlined,
              title: 'AI 模型与凭据',
              subtitle: 'Provider、Base URL、模型能力与 API Key（密钥只写不读）',
              onTap: () => _push(context, const AiProviderSettingsPage()),
            ),
            const SizedBox(height: 8),
            // 工具权限与"模型与凭据"**不构成附属关系**，
            // 入口位于设置首页，不放入模型页顶栏。
            SettingsEntryTile(
              icon: Icons.shield_outlined,
              title: 'AI 工具权限',
              subtitle: '读写白名单、逐个工具的 allow / ask / deny、调用预算',
              onTap: () => _push(context, const AiToolsSettingsPage()),
            ),
          ],
        ),
      ),
      // 「采集」与「界面」属于不同类别（不应合并为一组）。
      SettingsSection(
        title: '采集',
        id: 'capture',
        estimatedHeight: 140,
        child: SettingsEntryTile(
          icon: Icons.sync_alt,
          title: 'ComfyUI 自动捕获',
          subtitle: '地址、输出目录、轮询间隔、自动标签、立即同步与目录导入',
          onTap: () => _push(context, const ComfyCaptureSettingsPage()),
        ),
      ),
      SettingsSection(
        title: '界面',
        id: 'interface',
        estimatedHeight: 140,
        child: SettingsEntryTile(
          icon: Icons.palette_outlined,
          title: '外观与布局',
          subtitle: '明暗模式、取色档位、每页数量、集合详情形态',
          onTap: () => _push(context, const InterfaceSettingsPage()),
        ),
      ),
      // NSFW 只有两个选项，而且从属关系要一眼看见 —— 留在首页。
      const SettingsSection(
        title: 'NSFW 内容',
        id: 'nsfw',
        child: _NsfwSettingsCard(),
      ),
      if (!isWindowsDesktop && !kIsWeb)
        const SettingsSection(
          title: '产物缓存（本机）',
          id: 'cache',
          child: _MediaCacheCard(),
        ),
      const SettingsSection(
        title: '库统计',
        id: 'stats',
        child: _LibraryStatsRows(),
      ),
      // 「数据目录」是**主设置菜单**中的一项，不放在「界面」子页内。
      // 该页可查看当前位置、打开目录、更换位置（含迁移）。
      //
      // 副标题写 `<数据目录>\storage`（产物 / 附件 / AI 文件），不是运行根目录：
      // 根目录是开发概念，源码树中运行时即仓库根。
      if (isWindowsDesktop)
        SettingsSection(
          title: '目录',
          id: 'dataDir',
          estimatedHeight: 140,
          child: SettingsEntryTile(
            icon: Icons.folder_open_outlined,
            title: '数据目录',
            subtitle: dataDir == null
                ? '未定位到数据目录'
                : resolvedStorageDirOf(dataDir),
            onTap: () => _push(context, const DataDirSettingsPage()),
          ),
        ),
      if (isWindowsDesktop)
        const SettingsSection(
          title: '导出',
          id: 'export',
          estimatedHeight: 160,
          child: _ExportAllCard(),
        ),
    ];
  }
}

/// 桌面设置「偏好」：NSFW 与（手机端）产物缓存。
///
/// NSFW 不再混在「概览」里：概览只回答"库里有什么"，偏好才是改内容策略的地方。
/// 这一页同时被设置搜索当作目标页使用（带返回键）。
class PreferencesSettingsPage extends StatelessWidget {
  /// 从搜索结果进入时定位到哪一节（`nsfw` / `cache`）。
  final String? initialSectionId;

  final bool showBack;

  const PreferencesSettingsPage({
    super.key,
    this.initialSectionId,
    this.showBack = false,
  });

  @override
  Widget build(BuildContext context) {
    final isWindowsDesktop = isWindowsDesktopHost;
    return SettingsScaffold(
      title: '偏好',
      showBack: showBack,
      initialSectionId: initialSectionId,
      sections: [
        const SettingsSection(
          title: 'NSFW 内容',
          id: 'nsfw',
          child: _NsfwSettingsCard(),
        ),
        if (!isWindowsDesktop && !kIsWeb)
          const SettingsSection(
            title: '产物缓存（本机）',
            id: 'cache',
            child: _MediaCacheCard(),
          ),
      ],
    );
  }
}

/// 桌面设置「概览」：库统计与导出。
///
/// 它挂在右侧分栏自己的 `Navigator` 里，不会因为 `SettingsPage` 重建而重建；
/// 所以数据必须由**子树自己** `watch`，否则统计会停在打开那一刻。
class _SettingsOverviewPane extends StatelessWidget {
  const _SettingsOverviewPane();

  @override
  Widget build(BuildContext context) {
    return SettingsScaffold(
      title: '概览',
      showBack: false,
      sections: [
        const SettingsSection(
          title: '库统计',
          id: 'stats',
          child: _LibraryStatsStrip(),
        ),
        if (isWindowsDesktopHost)
          const SettingsSection(
            title: '导出',
            id: 'export',
            child: _ExportAllCard(),
          ),
      ],
    );
  }
}

/// 宽屏库统计：**2×2 的数字格**（「概览」正文）。
///
/// 数字短、彼此独立，排成 2×2 既省高度也用上了宽度；四项都在**同一张卡**里，
/// 不存在"两列高低不一"的问题。窄屏 / 手机仍用 [_LibraryStatsRows]。
class _LibraryStatsStrip extends StatelessWidget {
  const _LibraryStatsStrip();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.watch<LibraryStore>();
    final cells = <Widget>[
      _statCell(
        theme,
        '提示词',
        '${store.stats.prompts}',
        '生图 ${store.stats.byKind['IMAGE'] ?? 0} · 生视频 ${store.stats.byKind['VIDEO'] ?? 0} · 生音频 ${store.stats.byKind['AUDIO'] ?? 0}',
      ),
      _statCell(
        theme,
        '生成产物',
        '${store.stats.media}',
        '图片 ${store.stats.byMediaKind['IMAGE'] ?? 0} · 视频 ${store.stats.byMediaKind['VIDEO'] ?? 0} · 音频 ${store.stats.byMediaKind['AUDIO'] ?? 0}',
      ),
      _statCell(theme, '标签', '${store.stats.tags}', null),
      _statCell(
        theme,
        '收藏',
        '${store.stats.favoritePrompts} 提示词 / ${store.stats.favoriteMedia} 产物',
        null,
      ),
    ];

    Widget row(int left, int right) => Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: cells[left]),
        _statDivider(theme),
        Expanded(child: cells[right]),
      ],
    );

    return Card(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Column(
          // 被拉伸到和左栏一样高时，两行均匀铺开；纵向叠放时就是自然高度。
          mainAxisAlignment: MainAxisAlignment.spaceEvenly,
          children: [row(0, 1), const Divider(height: 25), row(2, 3)],
        ),
      ),
    );
  }

  Widget _statDivider(ThemeData theme) => Container(
    width: 1,
    height: 56,
    margin: const EdgeInsets.symmetric(horizontal: 12),
    color: theme.colorScheme.outlineVariant,
  );

  Widget _statCell(ThemeData theme, String label, String value, String? sub) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 2),
        Text(
          value,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.headlineSmall?.copyWith(
            fontWeight: FontWeight.w700,
          ),
        ),
        if (sub != null && sub.isNotEmpty)
          Text(
            sub,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
      ],
    );
  }
}

/// 窄屏 / 手机库统计：保持 2026-11-05 的四行竖排（标签在左、数字在右）。
class _LibraryStatsRows extends StatelessWidget {
  const _LibraryStatsRows();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.watch<LibraryStore>();
    return Card(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Column(
          children: [
            _statRow(
              theme,
              '提示词',
              '${store.stats.prompts}',
              '生图 ${store.stats.byKind['IMAGE'] ?? 0} · 生视频 ${store.stats.byKind['VIDEO'] ?? 0} · 生音频 ${store.stats.byKind['AUDIO'] ?? 0}',
            ),
            const Divider(height: 20),
            _statRow(
              theme,
              '生成产物',
              '${store.stats.media}',
              '图片 ${store.stats.byMediaKind['IMAGE'] ?? 0} · 视频 ${store.stats.byMediaKind['VIDEO'] ?? 0} · 音频 ${store.stats.byMediaKind['AUDIO'] ?? 0}',
            ),
            const Divider(height: 20),
            _statRow(theme, '标签', '${store.stats.tags}', null),
            const Divider(height: 20),
            _statRow(
              theme,
              '收藏',
              '${store.stats.favoritePrompts} 提示词 / ${store.stats.favoriteMedia} 产物',
              null,
            ),
          ],
        ),
      ),
    );
  }

  Widget _statRow(ThemeData theme, String label, String value, String? sub) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 90,
          child: Text(label, style: theme.textTheme.bodyMedium),
        ),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                value,
                style: theme.textTheme.titleMedium?.copyWith(
                  fontWeight: FontWeight.w700,
                ),
              ),
              if (sub != null && sub.isNotEmpty)
                Text(
                  sub,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

// -------------------------------------------------------------------------
//  NSFW 内容
//
//  两个选项**放在一起**。真源在后端的 `ai.nsfw`，这里只是读写通道：
//  ① 开关：开 = 系统提示词里注入那段"越狱"提示词，关 = 注入一句"本机没开"；
//  ② 模糊：开 = 预览图打码（点一下解开这一张），且**只有 ① 也开着才生效**。
//
//  自持 State 是为了能挂在桌面分栏的 Navigator 里：弹层里重建不会丢开关状态。
// -------------------------------------------------------------------------

class _NsfwSettingsCard extends StatefulWidget {
  const _NsfwSettingsCard();

  @override
  State<_NsfwSettingsCard> createState() => _NsfwSettingsCardState();
}

class _NsfwSettingsCardState extends State<_NsfwSettingsCard> {
  bool _busy = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final nsfw = context.watch<NsfwStore>();
    final busy = _busy;
    final enabled = nsfw.nsfwEnabled;
    final blur = nsfw.blurOptionEnabled;

    Future<void> apply({bool? nextEnabled, bool? nextBlur}) async {
      setState(() => _busy = true);
      final next = await nsfw.save(
        enabled: nextEnabled ?? enabled,
        // 关掉开关时模糊也一并落成"关"：不然下次再打开会莫名其妙地全糊
        blur: (nextEnabled == false) ? false : (nextBlur ?? blur),
      );
      if (!context.mounted) return;
      setState(() => _busy = false);
      if (!next.ok) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(next.message)));
      }
    }

    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 8, 8, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 4, 8, 8),
              child: Text(
                '成人内容相关。改完下一次回复立刻生效。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
            SettingsSwitchRow(
              title: 'NSFW 模式',
              subtitle: enabled
                  ? '已开启，但 AI 工作台的对话有可能被审查严格的模型拦截。'
                  : '未开启，模型会按常规内容策略作答。',
              value: enabled,
              onChanged: busy ? null : (v) => apply(nextEnabled: v),
            ),
            const Divider(height: 1),
            SettingsSwitchRow(
              title: 'NSFW 内容模糊显示',
              subtitle: !enabled
                  ? '先开启上面的「NSFW 模式」才有意义。'
                  : (blur
                        ? '已开启，画廊 / 提示词 / 标签页与 AI 回复里的预览图会模糊'
                        : '未开启，预览图原样显示。'),
              value: blur,
              // 开关没开时这一项置灰：两个选项的从属关系要一眼看得出来
              onChanged: (busy || !enabled) ? null : (v) => apply(nextBlur: v),
            ),
            // 版本号（`注入正文版本 vNN`）已去掉：
            // 它属于实现细节，与开关状态无关。
            if (nsfw.error != null)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 6, 16, 0),
                child: Text(
                  nsfw.error!,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.error,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// 手机端产物缓存（占用、预算、清空）。
///
/// 独立持有统计状态：宽屏分栏把它挂在右侧 Navigator 里，父页面重建不会波及它。
class _MediaCacheCard extends StatefulWidget {
  const _MediaCacheCard();

  @override
  State<_MediaCacheCard> createState() => _MediaCacheCardState();
}

class _MediaCacheCardState extends State<_MediaCacheCard> {
  /// 产物缓存占用（null = 还没统计完）。
  MediaCacheStats? _stats;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadStats());
  }

  Future<void> _loadStats() async {
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() => _stats = stats);
  }

  /// 换档位：写偏好（[SettingsStore] 会当场把新预算交给缓存并淘汰一次），
  /// 再等一拍重新统计 —— 淘汰是异步的，紧接着读会读到淘汰前的数字。
  Future<void> _onBudgetChanged(int mb) async {
    final settings = context.read<SettingsStore>();
    await settings.setMediaCacheOrigBudgetMb(mb);
    await Future<void>.delayed(const Duration(milliseconds: 120));
    await _loadStats();
  }

  Future<void> _clear() async {
    setState(() => _busy = true);
    final freed = await MediaCache.instance.clear();
    final failures = MediaCache.instance.lastClearFailures;
    final listError = MediaCache.instance.lastClearError;
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() {
      _stats = stats;
      _busy = false;
    });
    // **不把"没删掉"说成"清干净了"**：列目录失败 / 有文件删不掉都要如实写。
    final String message;
    if (listError != null) {
      message = '没能列完缓存目录，只清掉了 ${formatSize(freed)}。';
    } else if (failures > 0) {
      message = '已清理 ${formatSize(freed)}，还有 $failures 个文件删不掉（可能正被播放器占用）。';
    } else if (freed > 0) {
      message = '已清理 ${formatSize(freed)}';
    } else {
      message = '缓存本来就是空的';
    }
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    return MediaCacheCard(
      stats: _stats,
      budgetMb: context.watch<SettingsStore>().mediaCacheOrigBudgetMb,
      busy: _busy,
      onBudgetChanged: _onBudgetChanged,
      onClear: _clear,
    );
  }
}

/// 「导出全部产物…」入口（桌面端）。
class _ExportAllCard extends StatelessWidget {
  const _ExportAllCard();

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
        leading: const Icon(Icons.save_alt),
        title: const Text('导出全部产物…'),
        subtitle: const Text(
          '按 图片 / 视频 / 音频 分目录，同一集合单独成夹，'
          '工作流与提示词信息一起带走（可先在画廊多选只导一部分）',
        ),
        trailing: const Icon(Icons.chevron_right),
        onTap: () => showExportDialog(context),
      ),
    );
  }
}
