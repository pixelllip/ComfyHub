import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/backend_launcher.dart';
import '../core/backend_paths.dart';
import '../core/formatting.dart';
import '../core/media_cache.dart';
import '../core/platform_caps.dart';
import '../core/settings_store.dart';
import '../state/library_store.dart';
import '../state/nsfw_store.dart';
import '../widgets/export_dialog.dart';
import '../widgets/media_cache_card.dart';
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

/// 设置首页。
///
/// 2026-11-05 用户："如果某个卡片有三项或以上的选项，应当把他们做进子界面，
/// 像 AI 模型与凭据那样。"
///
/// 于是首页只剩三类东西：
///   1. **入口行**（本地服务 / 配对 / 模型 / 工具权限 / 自动捕获 / 界面）；
///   2. **只有一两项、且要"一眼看到当前值"的小卡**（NSFW、产物缓存）；
///   3. **只读概览**（库统计）与**单个动作**（导出）。
///
/// 判断标准别写反：不是"看起来长就拆"，而是**选项 ≥ 3 就进子页**。NSFW 只有
/// 两个开关、缓存只有一个滑杆，它们留在首页反而更好用。
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.watch<LibraryStore>();
    final launcher = context.watch<BackendLauncher>();
    // 判据只有一份：`isWindowsDesktopHost`（带单测用的平台 override）。
    final isWindowsDesktop = isWindowsDesktopHost;

    return SettingsScaffold(
      // 首页在 `HomeShell` 的内容区里，不需要返回箭头。
      title: '设置',
      showBack: false,
      sections: [
        SettingsSection(
          title: '服务',
          estimatedHeight: 280,
          child: Column(
            children: [
              // 「本地服务（后端 + 数据库）」只在 Windows 桌面端有意义：
              // 手机连的是桌面后端，本机没有可拉起的 Rust exe，也没有
              // 盘符 / 资源管理器（用户 2026-11-08 报的就是这一项又冒出来了）。
              //
              // 手机端换成它真正拥有的那一套（用户 2026-11-08："把那些电脑才
              // 能用的功能，在手机端替换成手机端专用的功能"）：本机库 + 上行
              // 队列 + 桌面连接 + 立即同步/拉取。
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
              // 2026-11-05 用户：工具权限与"模型与凭据"**不构成附属关系**，
              // 层级提高一级 —— 入口放在设置首页，而不是塞进模型页的顶栏。
              SettingsEntryTile(
                icon: Icons.shield_outlined,
                title: 'AI 工具权限',
                subtitle: '读写白名单、逐个工具的 allow / ask / deny、调用预算',
                onTap: () => _push(context, const AiToolsSettingsPage()),
              ),
            ],
          ),
        ),
        // 「采集」与「界面」是两件不相干的事（用户 2026-11-05：不该合成一组）。
        SettingsSection(
          title: '采集',
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
          estimatedHeight: 140,
          child: SettingsEntryTile(
            icon: Icons.palette_outlined,
            title: '外观与布局',
            subtitle: '明暗模式、取色档位、每页数量、集合详情形态',
            onTap: () => _push(context, const InterfaceSettingsPage()),
          ),
        ),
        // NSFW 只有两个选项，而且从属关系要一眼看见 —— 留在首页。
        SettingsSection(
          title: 'NSFW 内容',
          estimatedHeight: 340,
          child: _nsfwCard(theme),
        ),
        if (!isWindowsDesktop && !kIsWeb)
          SettingsSection(
            title: '产物缓存（本机）',
            estimatedHeight: 280,
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
          estimatedHeight: 360,
          child: _statsCard(theme, store),
        ),
        // 「数据目录」是**主设置菜单**里的一项（用户 2026-11-05：不该藏在
        // 「界面」子页里）。2026-11-06 起它变成子页入口：那里能看当前位置、
        // 打开目录、换位置（带迁移）—— 不再是一个"点开就跳走"的单动作。
        //
        // 副标题写 `<数据目录>\storage`（产物 / 附件 / AI 文件），不是运行根目录：
        // 根目录是开发概念，源码树里跑它就是仓库根（用户 2026-11-06）。
        if (isWindowsDesktop)
          SettingsSection(
            title: '目录',
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
      ],
    );
  }

  Widget _statsCard(ThemeData theme, LibraryStore store) {
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
  //  NSFW 内容（2026-09-20 用户要求）
  //
  //  两个选项**放一起**（用户原话）。真源在后端的 `ai.nsfw`，这里只是读写通道：
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
            if (nsfw.promptVersion.isNotEmpty)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
                child: Text(
                  '注入正文版本 ${nsfw.promptVersion}',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ),
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
