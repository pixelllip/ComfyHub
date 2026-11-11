import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
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
import '../widgets/settings_scaffold.dart';
import '../widgets/settings_switch_row.dart';
import '../widgets/settings_tile.dart';
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
/// 卡片中有三项及以上选项时收进子界面（例如 AI 模型与凭据）。
///
/// 于是首页只剩三类东西：
///   1. **入口行**（本地服务 / 配对 / 模型 / 工具权限 / 自动捕获 / 界面）；
///   2. **只有一两项、且要"一眼看到当前值"的小卡**（NSFW、产物缓存）；
///   3. **只读概览**（库统计）与**单个动作**（导出）。
///
/// 判断标准别写反：不是"看起来长就拆"，而是**选项 ≥ 3 就进子页**。NSFW 只有
/// 两个开关、缓存只有一个滑杆，它们留在首页反而更好用。
class SettingsPage extends StatefulWidget {
  /// 设置搜索跳转到首页某一节时传入（例如 `nsfw` / `cache`）。
  final String? initialSectionId;

  /// 被搜索结果当作独立页面打开时需要返回键；作为导航的一页时不需要。
  final bool showBack;

  const SettingsPage({super.key, this.initialSectionId, this.showBack = false});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  /// NSFW 两个开关正在写库（避免连点把两次写入交叉）。
  bool _nsfwBusy = false;

  /// 产物缓存占用（手机端；null = 还没统计完）。
  MediaCacheStats? _cacheStats;
  bool _cacheBusy = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _loadCacheStats());
  }

  Future<void> _loadCacheStats() async {
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() => _cacheStats = stats);
  }

  /// 换档位：写偏好（[SettingsStore] 会当场把新预算交给缓存并淘汰一次），
  /// 再等一拍重新统计 —— 淘汰是异步的，紧接着读会读到淘汰前的数字。
  Future<void> _onBudgetChanged(int mb) async {
    final settings = context.read<SettingsStore>();
    await settings.setMediaCacheOrigBudgetMb(mb);
    await Future<void>.delayed(const Duration(milliseconds: 120));
    await _loadCacheStats();
  }

  Future<void> _clearCache() async {
    setState(() => _cacheBusy = true);
    final freed = await MediaCache.instance.clear();
    final failures = MediaCache.instance.lastClearFailures;
    final listError = MediaCache.instance.lastClearError;
    final stats = await MediaCache.instance.stats();
    if (!mounted) return;
    setState(() {
      _cacheStats = stats;
      _cacheBusy = false;
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

  void _push(BuildContext context, Widget page) {
    Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => page));
  }

  /// 打开独立的结果页：从结果点进去的目标页返回时，先回到这一页。
  void _openSearch(BuildContext context, bool isWindowsDesktop) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => SettingsSearchPage(
          isWindowsDesktop: isWindowsDesktop,
          homePageBuilder: (sectionId) =>
              SettingsPage(initialSectionId: sectionId, showBack: true),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.watch<LibraryStore>();
    final launcher = context.watch<BackendLauncher>();
    // 判据只有一份：`isWindowsDesktopHost`（带单测用的平台 override）。
    final isWindowsDesktop = isWindowsDesktopHost;
    // 宽窄与导航形态用**同一个数字**：< 900 就是手机形态，电脑端窄屏也一样。
    // 电脑端窄屏恢复为手机端排版。
    final wide = MediaQuery.sizeOf(context).width >= kWideLayoutMinWidth;

    return SettingsScaffold(
      // 首页在 `HomeShell` 的内容区里，不需要返回箭头。
      title: '设置',
      showBack: widget.showBack,
      initialSectionId: widget.initialSectionId,
      header: SettingsSearchBar(
        onTap: () => _openSearch(context, isWindowsDesktop),
      ),
      sections: wide
          ? _wideSections(context, theme, store, launcher, isWindowsDesktop)
          : _narrowSections(context, theme, store, launcher, isWindowsDesktop),
    );
  }

  /// 宽屏首页：**等高磁贴** + 单列状态卡。
  ///
  /// 磁贴自己就写着是哪一项，不再加"服务 / AI / 采集 / 界面"这层分类标题 ——
  /// 加了只会把首页拉长。分类只在窄 / 手机那套里保留（见 [_narrowSections]）。
  List<SettingsSection> _wideSections(
    BuildContext context,
    ThemeData theme,
    LibraryStore store,
    BackendLauncher launcher,
    bool isWindowsDesktop,
  ) {
    final entries = <Widget>[
      // 「本地服务（后端 + 数据库）」仅在 Windows 桌面端适用：
      // 手机连接的是桌面后端，本机没有可启动的 Rust exe，也没有
      // 盘符 / 资源管理器。
      //
      // 手机端使用对应功能：本机库 + 上行队列 + 桌面连接 + 立即同步/拉取。
      if (isWindowsDesktop)
        SettingsTile(
          icon: Icons.dns_outlined,
          title: '本地服务（后端 + 数据库）',
          subtitle: '启动、停止、日志与后端地址',
          onTap: () => _push(context, const LocalServicesSettingsPage()),
        )
      else
        SettingsTile(
          icon: Icons.phonelink_setup,
          title: '本机数据与同步',
          subtitle: '本机会话、上行队列、桌面连接',
          onTap: () => _push(context, const MobileLocalSettingsPage()),
        ),
      SettingsTile(
        icon: Icons.qr_code_2,
        title: '远程访问与配对',
        subtitle: '二维码、已授权设备、同步密钥',
        onTap: () => _push(context, const PairingSettingsPage()),
      ),
      SettingsTile(
        icon: Icons.smart_toy_outlined,
        title: 'AI 模型与凭据',
        subtitle: 'Provider、模型能力与 API Key',
        onTap: () => _push(context, const AiProviderSettingsPage()),
      ),
      // 工具权限与"模型与凭据"**不构成附属关系**，
      // 入口位于设置首页，不放入模型页顶栏。
      SettingsTile(
        icon: Icons.shield_outlined,
        title: 'AI 工具权限',
        subtitle: '读写白名单、批准方式、调用预算',
        onTap: () => _push(context, const AiToolsSettingsPage()),
      ),
      SettingsTile(
        icon: Icons.sync_alt,
        title: 'ComfyUI 自动捕获',
        subtitle: '地址、输出目录、轮询与目录导入',
        onTap: () => _push(context, const ComfyCaptureSettingsPage()),
      ),
      SettingsTile(
        icon: Icons.palette_outlined,
        title: '外观与布局',
        subtitle: '明暗模式、取色档位、每页数量',
        onTap: () => _push(context, const InterfaceSettingsPage()),
      ),
      // 「数据目录」是**主设置菜单**中的一项，不放在「界面」子页内。
      // 该页可查看当前位置、打开目录、更换位置（含迁移）。
      //
      // 副标题写 `<数据目录>\storage`（产物 / 附件 / AI 文件），不是运行根目录：
      // 根目录是开发概念，源码树中运行时即仓库根。
      if (isWindowsDesktop)
        SettingsTile(
          icon: Icons.folder_open_outlined,
          title: '数据目录',
          subtitle: launcher.dataDir == null
              ? '未定位到数据目录'
              : resolvedStorageDirOf(launcher.dataDir!),
          onTap: () => _push(context, const DataDirSettingsPage()),
        ),
      if (isWindowsDesktop)
        SettingsTile(
          icon: Icons.save_alt,
          title: '导出全部产物…',
          subtitle: '按图片、视频、音频分目录导出',
          onTap: () => showExportDialog(context),
        ),
    ];

    // 状态区：响应式规则（判据是**可用内容宽度**）：
    //   · 宽屏（≥1200）：NSFW 40% + 库统计 60%；
    //   · 中等（≥820）：两个等宽面板；
    //   · 窄：上下堆叠。
    // 两栏时用 `IntrinsicHeight` + `SettingsSection.fill` 把下边缘拉齐。
    final cacheCard = (!isWindowsDesktop && !kIsWeb)
        ? SettingsSection(
            title: '产物缓存（本机）',
            id: 'cache',
            child: MediaCacheCard(
              stats: _cacheStats,
              budgetMb: context.watch<SettingsStore>().mediaCacheOrigBudgetMb,
              busy: _cacheBusy,
              onBudgetChanged: _onBudgetChanged,
              onClear: _clearCache,
            ),
          )
        : null;

    return [
      // 第一组不带小标题：它就是"全部入口"。
      SettingsSection(
        title: '',
        id: 'entries',
        child: SettingsTileGrid(maxColumns: 4, tiles: entries),
      ),
      SettingsSection(
        title: '',
        id: 'nsfw',
        child: _StatusArea(
          nsfw: _nsfwCard(theme),
          stats: _statsStrip(theme, store),
        ),
      ),
      ?cacheCard,
    ];
  }

  /// 窄屏 / 手机首页：**保持分组入口行**（手机端排版保持不变，
  /// 电脑端窄屏恢复为手机端排版）。
  ///
  /// ⚠ 这一支别再换成磁贴：磁贴是宽屏用来"利用横向空间"的；窄屏上它只会
  /// 把三四个分组压成一长条，分类反而丢了。
  List<SettingsSection> _narrowSections(
    BuildContext context,
    ThemeData theme,
    LibraryStore store,
    BackendLauncher launcher,
    bool isWindowsDesktop,
  ) {
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
      SettingsSection(title: 'NSFW 内容', id: 'nsfw', child: _nsfwCard(theme)),
      if (!isWindowsDesktop && !kIsWeb)
        SettingsSection(
          title: '产物缓存（本机）',
          id: 'cache',
          child: MediaCacheCard(
            stats: _cacheStats,
            budgetMb: context.watch<SettingsStore>().mediaCacheOrigBudgetMb,
            busy: _cacheBusy,
            onBudgetChanged: _onBudgetChanged,
            onClear: _clearCache,
          ),
        ),
      SettingsSection(
        title: '库统计',
        id: 'stats',
        child: _statsRows(theme, store),
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
            subtitle: launcher.dataDir == null
                ? '未定位到数据目录'
                : resolvedStorageDirOf(launcher.dataDir!),
            onTap: () => _push(context, const DataDirSettingsPage()),
          ),
        ),
      if (isWindowsDesktop)
        SettingsSection(
          title: '导出',
          id: 'export',
          estimatedHeight: 160,
          child: Card(
            clipBehavior: Clip.antiAlias,
            child: ListTile(
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 4,
              ),
              leading: const Icon(Icons.save_alt),
              title: const Text('导出全部产物…'),
              subtitle: const Text(
                '按 图片 / 视频 / 音频 分目录，同一集合单独成夹，'
                '工作流与提示词信息一起带走（可先在画廊多选只导一部分）',
              ),
              trailing: const Icon(Icons.chevron_right),
              onTap: () => showExportDialog(context),
            ),
          ),
        ),
    ];
  }

  /// 宽屏库统计：**2×2 的数字格**（状态区右侧那一栏）。
  ///
  /// 数字短、彼此独立，排成 2×2 既省高度也用上了宽度；四项都在**同一张卡**里，
  /// 不存在"两列高低不一"的问题。窄屏 / 手机仍用 [_statsRows]。
  Widget _statsStrip(ThemeData theme, LibraryStore store) {
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

  /// 窄屏 / 手机库统计：保持 2026-11-05 的四行竖排（标签在左、数字在右）。
  Widget _statsRows(ThemeData theme, LibraryStore store) {
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

  // -------------------------------------------------------------------------
  //  NSFW 内容
  //
  //  两个选项**放在一起**。真源在后端的 `ai.nsfw`，这里只是读写通道：
  //  ① 开关：开 = 系统提示词里注入那段"越狱"提示词，关 = 注入一句"本机没开"；
  //  ② 模糊：开 = 预览图打码（点一下解开这一张），且**只有 ① 也开着才生效**。
  // -------------------------------------------------------------------------

  Widget _nsfwCard(ThemeData theme) {
    final nsfw = context.watch<NsfwStore>();
    final busy = _nsfwBusy;
    final enabled = nsfw.nsfwEnabled;
    final blur = nsfw.blurOptionEnabled;

    Future<void> apply({bool? nextEnabled, bool? nextBlur}) async {
      setState(() => _nsfwBusy = true);
      final next = await nsfw.save(
        enabled: nextEnabled ?? enabled,
        // 关掉开关时模糊也一并落成"关"：不然下次再打开会莫名其妙地全糊
        blur: (nextEnabled == false) ? false : (nextBlur ?? blur),
      );
      if (!mounted) return;
      setState(() => _nsfwBusy = false);
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

/// 设置首页的"状态区"：NSFW 开关 + 库统计。
///
/// 响应式规则，判据是**可用内容宽度**（不是窗口宽度 ——
/// 正文本身还有 1680 的上限）：
///   · ≥1200：NSFW 40% + 库统计 60%；
///   · ≥820：两个等宽面板；
///   · 更窄：上下堆叠。
///
/// 两栏时两张卡的下边缘必须齐平：[IntrinsicHeight] 先量出这一行的高度，
/// [SettingsSection.fill] 再把两张卡各自撑满 —— 不会再出现"一列长一列短"。
class _StatusArea extends StatelessWidget {
  final Widget nsfw;
  final Widget stats;

  const _StatusArea({required this.nsfw, required this.stats});

  /// 4:6 不等宽的下限。
  static const double wideMinWidth = 1200;

  /// 两个等宽面板的下限。
  static const double evenMinWidth = 820;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        if (width >= wideMinWidth) return _panels(4, 6);
        if (width >= evenMinWidth) return _panels(1, 1);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SettingsSection(title: 'NSFW 内容', child: nsfw),
            const SizedBox(height: 24),
            SettingsSection(title: '库统计', child: stats),
          ],
        );
      },
    );
  }

  Widget _panels(int leftFlex, int rightFlex) => IntrinsicHeight(
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          flex: leftFlex,
          child: SettingsSection(fill: true, title: 'NSFW 内容', child: nsfw),
        ),
        const SizedBox(width: 24),
        Expanded(
          flex: rightFlex,
          child: SettingsSection(fill: true, title: '库统计', child: stats),
        ),
      ],
    ),
  );
}
