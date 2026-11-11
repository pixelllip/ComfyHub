import 'package:flutter/material.dart';

import '../widgets/export_dialog.dart';
import '../widgets/settings_scaffold.dart';
import 'ai_provider_settings_page.dart';
import 'ai_tools_settings_page.dart';
import 'settings_comfy_capture_page.dart';
import 'settings_data_dir_page.dart';
import 'settings_interface_page.dart';
import 'settings_local_services_page.dart';
import 'settings_mobile_local_page.dart';
import 'settings_pairing_page.dart';

/// 设置搜索里的一个结果。
///
/// 标题是界面上那一项的名字，副标题是它在设置里的位置（面包屑）。
/// 点进去可能是独立页面，也可能是首页的某一节（由 [destination] 决定）。
class SettingsSearchEntry {
  final String title;

  /// 这一项在哪里，例如 `设置 › AI 工具权限 › 预算`。
  final String location;

  /// 额外的匹配词（同义词、页面名、栏目标题）。用户不一定记得界面上的原词。
  final List<String> keywords;

  final IconData icon;

  /// 点开之后要去的页面。null 表示这一项用 [action]。
  final WidgetBuilder? destination;

  /// 不是页面而是动作（例如打开导出对话框）。
  final void Function(BuildContext context)? action;

  const SettingsSearchEntry({
    required this.title,
    required this.location,
    this.keywords = const [],
    this.icon = Icons.settings_outlined,
    this.destination,
    this.action,
  });
}

/// 设置里一项的位置串（`设置 › 页面 › 小节`）。
String _where(List<String> parts) => parts.join(' › ');

/// 生成设置索引。
///
/// [isWindowsDesktop] 决定是否收录只在桌面端存在的项（本地服务 / 数据目录 / 导出）。
/// [homePageBuilder] 用来打开设置首页的某一节（NSFW、产物缓存），由设置页传入 ——
/// 搜索页不直接依赖首页，避免两个页面互相 import。
List<SettingsSearchEntry> buildSettingsSearchIndex({
  required bool isWindowsDesktop,
  required Widget Function(String? sectionId) homePageBuilder,
}) {
  final entries = <SettingsSearchEntry>[];

  void add({
    required String title,
    required String location,
    List<String> keywords = const [],
    IconData icon = Icons.settings_outlined,
    WidgetBuilder? destination,
    void Function(BuildContext context)? action,
  }) {
    entries.add(
      SettingsSearchEntry(
        title: title,
        location: location,
        keywords: keywords,
        icon: icon,
        destination: destination,
        action: action,
      ),
    );
  }

  // ---- 设置首页的入口 -----------------------------------------------------
  if (isWindowsDesktop) {
    add(
      title: '本地服务（后端 + 数据库）',
      location: _where(['设置']),
      keywords: const ['后端', '数据库', '启动', '停止', '日志', '服务'],
      icon: Icons.dns_outlined,
      destination: (_) => const LocalServicesSettingsPage(),
    );
  } else {
    add(
      title: '本机数据与同步',
      location: _where(['设置']),
      keywords: const ['手机', '本机会话', '上行', '桌面连接', '同步'],
      icon: Icons.phonelink_setup,
      destination: (_) => const MobileLocalSettingsPage(),
    );
  }
  add(
    title: '远程访问与配对',
    location: _where(['设置']),
    keywords: const ['二维码', '设备', '同步密钥', '配对闸门', '手机连接'],
    icon: Icons.qr_code_2,
    destination: (_) => const PairingSettingsPage(),
  );
  add(
    title: 'AI 模型与凭据',
    location: _where(['设置']),
    keywords: const ['Provider', '模型', 'API Key', '密钥', '协议', '能力'],
    icon: Icons.smart_toy_outlined,
    destination: (_) => const AiProviderSettingsPage(),
  );
  add(
    title: 'AI 工具权限',
    location: _where(['设置']),
    keywords: const ['工具', '允许', '询问', '拒绝', '白名单', '预算', '审批'],
    icon: Icons.shield_outlined,
    destination: (_) => const AiToolsSettingsPage(),
  );
  add(
    title: 'ComfyUI 自动捕获',
    location: _where(['设置']),
    keywords: const ['ComfyUI', '自动捕获', '输出目录', '轮询', '导入工作流'],
    icon: Icons.sync_alt,
    destination: (_) => const ComfyCaptureSettingsPage(),
  );
  add(
    title: '外观与布局',
    location: _where(['设置']),
    keywords: const ['主题', '明暗', '颜色', '每页数量', '集合详情'],
    icon: Icons.palette_outlined,
    destination: (_) => const InterfaceSettingsPage(),
  );
  if (isWindowsDesktop) {
    add(
      title: '数据目录',
      location: _where(['设置']),
      keywords: const ['存放位置', '搬家', '迁移', '对话', '图片', '角色卡'],
      icon: Icons.folder_open_outlined,
      destination: (_) => const DataDirSettingsPage(),
    );
    add(
      title: '导出全部产物',
      location: _where(['设置']),
      keywords: const ['导出', '图片', '视频', '音频', '备份产物'],
      icon: Icons.save_alt,
      action: (context) => showExportDialog(context),
    );
  }

  // ---- 设置首页里的状态块 -------------------------------------------------
  add(
    title: 'NSFW 模式',
    location: _where(['设置', 'NSFW 内容']),
    keywords: const ['成人内容', '越狱', '提示词', 'NSFW', '开关'],
    icon: Icons.no_adult_content_outlined,
    destination: (_) => homePageBuilder('nsfw'),
  );
  add(
    title: 'NSFW 内容模糊显示',
    location: _where(['设置', 'NSFW 内容']),
    keywords: const ['打码', '模糊', '预览图', 'NSFW'],
    icon: Icons.blur_on_outlined,
    destination: (_) => homePageBuilder('nsfw'),
  );
  if (!isWindowsDesktop) {
    add(
      title: '产物缓存（本机）',
      location: _where(['设置']),
      keywords: const ['缓存', '缩略图', '原件', '占用', '清理'],
      icon: Icons.photo_library_outlined,
      destination: (_) => homePageBuilder('cache'),
    );
  }

  // ---- AI 工具权限 -------------------------------------------------------
  add(
    title: '自动提交工作流次数',
    location: _where(['设置', 'AI 工具权限', '预算']),
    keywords: const ['自动提交', '工作流', '次数', '上限', '提交', '预算', 'GPU'],
    icon: Icons.playlist_add_check_circle_outlined,
    destination: (_) => const AiToolsSettingsPage(initialSectionId: 'budget'),
  );
  add(
    title: '工具轮数上限',
    location: _where(['设置', 'AI 工具权限', '预算']),
    keywords: const ['轮数', '预算', '工具', '步数'],
    icon: Icons.repeat,
    destination: (_) => const AiToolsSettingsPage(initialSectionId: 'budget'),
  );
  add(
    title: '调用总数上限',
    location: _where(['设置', 'AI 工具权限', '预算']),
    keywords: const ['调用', '次数', '预算', '上限'],
    icon: Icons.numbers,
    destination: (_) => const AiToolsSettingsPage(initialSectionId: 'budget'),
  );
  add(
    title: '写白名单',
    location: _where(['设置', 'AI 工具权限']),
    keywords: const ['写入', '目录', '白名单', '文件'],
    icon: Icons.edit_outlined,
    destination: (_) =>
        const AiToolsSettingsPage(initialSectionId: 'writeRoots'),
  );
  add(
    title: '读白名单',
    location: _where(['设置', 'AI 工具权限']),
    keywords: const ['读取', '目录', '白名单', '文件'],
    icon: Icons.folder_outlined,
    destination: (_) =>
        const AiToolsSettingsPage(initialSectionId: 'readRoots'),
  );
  add(
    title: '联网搜索（Keenable）',
    location: _where(['设置', 'AI 工具权限']),
    keywords: const ['联网', '搜索', '密钥', 'web_search', 'web_fetch'],
    icon: Icons.travel_explore_outlined,
    destination: (_) =>
        const AiToolsSettingsPage(initialSectionId: 'webSearch'),
  );
  add(
    title: '参考图 / 截图缓存',
    location: _where(['设置', 'AI 工具权限']),
    keywords: const ['附件', '缓存', '截图', '参考图', '清理'],
    icon: Icons.photo_library_outlined,
    destination: (_) =>
        const AiToolsSettingsPage(initialSectionId: 'attachmentCache'),
  );
  add(
    title: '逐工具权限',
    location: _where(['设置', 'AI 工具权限']),
    keywords: const ['工具', '允许', '询问', '拒绝', '逐个工具'],
    icon: Icons.tune,
    destination: (_) => const AiToolsSettingsPage(initialSectionId: 'tools'),
  );

  // ---- 外观与布局 -------------------------------------------------------
  add(
    title: '明暗模式',
    location: _where(['设置', '外观与布局', '主题']),
    keywords: const ['深色', '浅色', '跟随系统', '主题'],
    icon: Icons.brightness_6_outlined,
    destination: (_) => const InterfaceSettingsPage(initialSectionId: 'theme'),
  );
  add(
    title: '取色档位',
    location: _where(['设置', '外观与布局', '系统主题色']),
    keywords: const ['系统主题色', '强调色', '颜色', 'Material You'],
    icon: Icons.color_lens_outlined,
    destination: (_) => const InterfaceSettingsPage(initialSectionId: 'accent'),
  );
  add(
    title: '每页数量',
    location: _where(['设置', '外观与布局', '列表']),
    keywords: const ['分页', '条数', '列表'],
    icon: Icons.view_list_outlined,
    destination: (_) => const InterfaceSettingsPage(initialSectionId: 'list'),
  );
  add(
    title: '宽屏下的集合详情',
    location: _where(['设置', '外观与布局', '画廊']),
    keywords: const ['集合', '详情', '停靠面板', '弹窗', '画廊'],
    icon: Icons.photo_library_outlined,
    destination: (_) =>
        const InterfaceSettingsPage(initialSectionId: 'gallery'),
  );
  add(
    title: '窄屏下的集合详情',
    location: _where(['设置', '外观与布局', '画廊']),
    keywords: const ['集合', '详情', '停靠面板', '弹窗', '画廊'],
    icon: Icons.photo_library_outlined,
    destination: (_) =>
        const InterfaceSettingsPage(initialSectionId: 'gallery'),
  );

  // ---- 本地服务 / 桌面端 --------------------------------------------------
  if (isWindowsDesktop) {
    add(
      title: '启动 App 时自动拉起服务',
      location: _where(['设置', '本地服务', '开关']),
      keywords: const ['自动启动', '后端', '本地服务'],
      icon: Icons.power_settings_new,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'switches'),
    );
    add(
      title: '关闭 App 时一并停止本地服务',
      location: _where(['设置', '本地服务', '开关']),
      keywords: const ['停止服务', '退出', '后端'],
      icon: Icons.power_off,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'switches'),
    );
    add(
      title: '允许手机 / 局域网设备连接',
      location: _where(['设置', '本地服务', '开关']),
      keywords: const ['局域网', '手机', '远程', '监听'],
      icon: Icons.lan_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'switches'),
    );
    add(
      title: '服务状态',
      location: _where(['设置', '本地服务']),
      keywords: const ['启动', '重启', '停止', '日志', '后端'],
      icon: Icons.dns_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'status'),
    );
    add(
      title: '联网代理',
      location: _where(['设置', '本地服务']),
      keywords: const ['代理', '出网', 'HTTPS_PROXY', '系统代理'],
      icon: Icons.travel_explore_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'proxy'),
    );
    add(
      title: '备份与恢复',
      location: _where(['设置', '本地服务']),
      keywords: const ['备份', '恢复', '数据库'],
      icon: Icons.backup_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'backup'),
    );
    add(
      title: '存储占用',
      location: _where(['设置', '本地服务', '存储']),
      keywords: const ['存储', '占用', '清理', '孤儿', '事件归档'],
      icon: Icons.storage_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'storage'),
    );
    add(
      title: '运行根目录',
      location: _where(['设置', '本地服务']),
      keywords: const ['项目目录', '根目录', '路径'],
      icon: Icons.folder_open_outlined,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'root'),
    );
    add(
      title: '后端地址',
      location: _where(['设置', '本地服务']),
      keywords: const ['地址', '端口', '连接', 'Base URL'],
      icon: Icons.link,
      destination: (_) =>
          const LocalServicesSettingsPage(initialSectionId: 'url'),
    );
  }

  // ---- ComfyUI 自动捕获 ---------------------------------------------------
  add(
    title: '开启自动捕获',
    location: _where(['设置', 'ComfyUI 自动捕获', '捕获配置']),
    keywords: const ['自动捕获', 'ComfyUI', '轮询'],
    icon: Icons.sync_alt,
    destination: (_) =>
        const ComfyCaptureSettingsPage(initialSectionId: 'capture'),
  );
  add(
    title: 'ComfyUI 地址',
    location: _where(['设置', 'ComfyUI 自动捕获', '地址与目录']),
    keywords: const ['地址', '端口', '8188', 'ComfyUI'],
    icon: Icons.link,
    destination: (_) =>
        const ComfyCaptureSettingsPage(initialSectionId: 'address'),
  );
  add(
    title: 'ComfyUI 输出目录',
    location: _where(['设置', 'ComfyUI 自动捕获', '地址与目录']),
    keywords: const ['输出目录', 'output', '本地文件', 'ComfyUI'],
    icon: Icons.folder_open_outlined,
    destination: (_) =>
        const ComfyCaptureSettingsPage(initialSectionId: 'address'),
  );
  add(
    title: '轮询间隔',
    location: _where(['设置', 'ComfyUI 自动捕获', '轮询与操作']),
    keywords: const ['轮询', '间隔', '秒', '自动捕获'],
    icon: Icons.timer_outlined,
    destination: (_) =>
        const ComfyCaptureSettingsPage(initialSectionId: 'polling'),
  );
  add(
    title: '自动标签',
    location: _where(['设置', 'ComfyUI 自动捕获', '轮询与操作']),
    keywords: const ['标签', '自动标签', '捕获'],
    icon: Icons.sell_outlined,
    destination: (_) =>
        const ComfyCaptureSettingsPage(initialSectionId: 'polling'),
  );

  // ---- 数据目录 -----------------------------------------------------------
  if (isWindowsDesktop) {
    add(
      title: '数据目录当前位置',
      location: _where(['设置', '数据目录']),
      keywords: const ['数据目录', '存放位置', '对话', '图片', '角色卡'],
      icon: Icons.folder_open_outlined,
      destination: (_) =>
          const DataDirSettingsPage(initialSectionId: 'current'),
    );
    add(
      title: '搬到别处',
      location: _where(['设置', '数据目录', '当前位置']),
      keywords: const ['搬家', '迁移', '数据目录', '换位置'],
      icon: Icons.drive_file_move_outlined,
      destination: (_) =>
          const DataDirSettingsPage(initialSectionId: 'current'),
    );
    add(
      title: '搬回默认位置',
      location: _where(['设置', '数据目录', '当前位置']),
      keywords: const ['搬家', '默认位置', '数据目录'],
      icon: Icons.settings_backup_restore,
      destination: (_) =>
          const DataDirSettingsPage(initialSectionId: 'current'),
    );
  }

  // ---- 手机端本机数据 -----------------------------------------------------
  if (!isWindowsDesktop) {
    add(
      title: '桌面连接',
      location: _where(['设置', '本机数据与同步']),
      keywords: const ['电脑', '地址', '配对', '连接'],
      icon: Icons.desktop_windows_outlined,
      destination: (_) =>
          const MobileLocalSettingsPage(initialSectionId: 'connection'),
    );
    add(
      title: '立即同步到桌面',
      location: _where(['设置', '本机数据与同步', '本机库']),
      keywords: const ['同步', '上行', '推送', '桌面'],
      icon: Icons.cloud_upload_outlined,
      destination: (_) =>
          const MobileLocalSettingsPage(initialSectionId: 'library'),
    );
    add(
      title: '从桌面拉取',
      location: _where(['设置', '本机数据与同步', '本机库']),
      keywords: const ['同步', '下行', '拉取', '桌面'],
      icon: Icons.cloud_download_outlined,
      destination: (_) =>
          const MobileLocalSettingsPage(initialSectionId: 'library'),
    );
  }

  // ---- 远程访问与配对 -----------------------------------------------------
  add(
    title: '配对闸门',
    location: _where(['设置', '远程访问与配对']),
    keywords: const ['配对', '令牌', '局域网', '安全'],
    icon: Icons.qr_code_2,
    destination: (_) => const PairingSettingsPage(initialSectionId: 'pairing'),
  );
  add(
    title: '已授权设备',
    location: _where(['设置', '远程访问与配对']),
    keywords: const ['设备', '手机', '授权', '令牌'],
    icon: Icons.devices_outlined,
    destination: (_) => const PairingSettingsPage(initialSectionId: 'pairing'),
  );
  add(
    title: '同步密钥',
    location: _where(['设置', '远程访问与配对']),
    keywords: const ['密钥', '同步', '配对', '凭据'],
    icon: Icons.key_outlined,
    destination: (_) => const PairingSettingsPage(initialSectionId: 'pairing'),
  );

  return entries;
}

/// 按关键词过滤并排序。
///
/// 空格分隔的多个词要求**全部命中**（标题 / 位置 / 关键词任意一处）；
/// 排序优先命中标题，其次关键词，最后才是"位置里带着这个词"。
List<SettingsSearchEntry> searchSettingsEntries(
  List<SettingsSearchEntry> entries,
  String query,
) {
  final tokens = query
      .trim()
      .toLowerCase()
      .split(RegExp(r'\s+'))
      .where((token) => token.isNotEmpty)
      .toList();
  if (tokens.isEmpty) return entries;

  final matches = <({SettingsSearchEntry entry, int score, int index})>[];
  for (var index = 0; index < entries.length; index++) {
    final entry = entries[index];
    final title = entry.title.toLowerCase();
    final location = entry.location.toLowerCase();
    final keywords = entry.keywords.join(' ').toLowerCase();
    var score = 0;
    var all = true;
    for (final token in tokens) {
      if (title == token) {
        score += 1000;
      } else if (title.startsWith(token)) {
        score += 600;
      } else if (title.contains(token)) {
        score += 400;
      } else if (keywords.contains(token)) {
        score += 200;
      } else if (location.contains(token)) {
        score += 100;
      } else {
        all = false;
        break;
      }
    }
    if (all) matches.add((entry: entry, score: score, index: index));
  }
  matches.sort((a, b) {
    final byScore = b.score.compareTo(a.score);
    if (byScore != 0) return byScore;
    return a.index.compareTo(b.index);
  });
  return [for (final match in matches) match.entry];
}

/// 设置首页顶部的搜索入口。
///
/// 点击进入独立的结果页；从结果点进目标页之后，返回会先回到结果页。
class SettingsSearchBar extends StatelessWidget {
  final VoidCallback onTap;

  const SettingsSearchBar({super.key, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
          child: Row(
            children: [
              Icon(
                Icons.search,
                size: 20,
                color: theme.colorScheme.onSurfaceVariant,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  '搜索设置',
                  style: theme.textTheme.bodyLarge?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ),
              Icon(
                Icons.chevron_right,
                size: 20,
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 设置搜索结果页。
///
/// 结果标题是那一项的名字，副标题是它的位置。点击后把目标页**压在这条路由之上**，
/// 因此从目标页返回时先回到这里（查询词与滚动位置都还在），再返回才回设置首页。
class SettingsSearchPage extends StatefulWidget {
  final bool isWindowsDesktop;

  /// 打开设置首页某一节（NSFW / 产物缓存）时用，由设置页注入。
  final Widget Function(String? sectionId) homePageBuilder;

  const SettingsSearchPage({
    super.key,
    required this.isWindowsDesktop,
    required this.homePageBuilder,
  });

  @override
  State<SettingsSearchPage> createState() => _SettingsSearchPageState();
}

class _SettingsSearchPageState extends State<SettingsSearchPage> {
  final _controller = TextEditingController();
  late final List<SettingsSearchEntry> _entries;

  @override
  void initState() {
    super.initState();
    _entries = buildSettingsSearchIndex(
      isWindowsDesktop: widget.isWindowsDesktop,
      homePageBuilder: widget.homePageBuilder,
    );
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  List<SettingsSearchEntry> get _shown =>
      searchSettingsEntries(_entries, _controller.text);

  void _open(SettingsSearchEntry entry) {
    if (entry.action != null) {
      entry.action!(context);
      return;
    }
    final destination = entry.destination;
    if (destination == null) return;
    Navigator.of(context).push(MaterialPageRoute<void>(builder: destination));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final shown = _shown;
    final query = _controller.text.trim();
    return SettingsPageScaffold(
      title: '搜索设置',
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 6),
            child: TextField(
              controller: _controller,
              autofocus: true,
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              decoration: InputDecoration(
                hintText: '搜索设置项、功能名或说明',
                prefixIcon: const Icon(Icons.search),
                suffixIcon: query.isEmpty
                    ? null
                    : IconButton(
                        tooltip: '清空',
                        icon: const Icon(Icons.clear),
                        onPressed: () {
                          _controller.clear();
                          setState(() {});
                        },
                      ),
                border: const OutlineInputBorder(),
                isDense: true,
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                query.isEmpty
                    ? '共 ${_entries.length} 项设置，输入关键词可以缩小范围。'
                    : '找到 ${shown.length} 项。点一项可以打开对应的设置。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
          ),
          Expanded(
            child: shown.isEmpty
                ? Center(
                    child: Text(
                      '没有匹配的设置项。',
                      style: theme.textTheme.bodyMedium?.copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                  )
                : ListView.separated(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 32),
                    itemCount: shown.length,
                    separatorBuilder: (_, _) => const SizedBox(height: 8),
                    itemBuilder: (context, index) {
                      final entry = shown[index];
                      return Card(
                        clipBehavior: Clip.antiAlias,
                        child: ListTile(
                          leading: Icon(entry.icon),
                          title: Text(entry.title),
                          subtitle: Text(entry.location),
                          trailing: const Icon(Icons.chevron_right),
                          onTap: () => _open(entry),
                        ),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}
