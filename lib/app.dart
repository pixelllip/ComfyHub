import 'dart:async';
import 'dart:io' show Platform;
import 'dart:ui' show AppExitResponse;

import 'package:flutter/foundation.dart'
    show debugPrint, kIsWeb, visibleForTesting;
import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:provider/provider.dart';

import 'core/backend_launcher.dart';
import 'core/lan_discovery.dart';
import 'core/layout_constants.dart';
import 'core/pairing_token.dart';
import 'core/settings_store.dart';
import 'core/sync_endpoints.dart';
import 'core/system_accent.dart';
import 'core/theme.dart';
import 'models/models.dart';
import 'pages/ai_home_page.dart';
import 'pages/gallery_page.dart';
import 'pages/prompts_page.dart';
import 'pages/settings_page.dart';
import 'pages/tags_page.dart';
import 'state/ai_workspace_store.dart';
import 'state/library_store.dart';
import 'state/nsfw_store.dart';
import 'widgets/confinement_banner.dart';
import 'widgets/data_dir_notice_banner.dart';

class ComfyHubApp extends StatelessWidget {
  final SettingsStore settings;
  final BackendLauncher launcher;

  const ComfyHubApp({
    super.key,
    required this.settings,
    required this.launcher,
  });

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
        // 首屏不立刻拉数据：等 StartupGate 把后端拉起来之后再刷（见 _StartupGateState）
        ChangeNotifierProvider<LibraryStore>(
          create: (_) => LibraryStore(settings),
        ),
        // AI 工作台状态独立一份：画廊 / 提示词刷新不应该让聊天页整体 rebuild
        //
        // ⚠ `backendReady` 是**冷启动闸门**（2026-10-12）：AI 工作台是落地页，
        // 它在第一帧就会拉 provider / 会话，而那一刻后端还没起、端口文件还没写出来
        // （`.run\api.json` 在上一次优雅退出时被删掉了），`baseUrl` 只能是 8080 兜底值
        // —— 界面显示的就是一句 `ClientException … 127.0.0.1:8080/api/ai/providers`。
        ChangeNotifierProvider<AiWorkspaceStore>(
          create: (ctx) => AiWorkspaceStore(
            baseUrlProvider: () => ctx.read<SettingsStore>().baseUrl,
            backendReady: () => ctx.read<BackendLauncher>().settled(),
          ),
        ),
        // NSFW 开关（2026-09-20）：**全局**一份 —— 打码要同时作用于画廊、提示词、
        // 标签页与 AI 气泡里的产物卡，页面各自持有一份做不到这件事。
        // 同一个闸门：读不到时它会停在默认值上，表现是"开了模糊却不糊"（静默错）。
        ChangeNotifierProvider<NsfwStore>(
          create: (ctx) => NsfwStore(
            baseUrlProvider: () => ctx.read<SettingsStore>().baseUrl,
            backendReady: () => ctx.read<BackendLauncher>().settled(),
          )..load(silent: true),
        ),
      ],
      // 外观跟着偏好走：`settings` 是 ChangeNotifier，切一次主题只重建**这一层**
      // （MultiProvider 里各个 store、以及 `HomeShell` 的页面状态都不受影响 ——
      // 它们挂在 Provider 上，不在这棵子树的 State 里）。
      child: ListenableBuilder(
        listenable: settings,
        builder: (context, _) => MaterialApp(
          title: 'ComfyHub',
          debugShowCheckedModeBanner: false,
          // 界面里的所有系统级菜单（文本框右键的「复制 / 全选 / 粘贴」、
          // 返回按钮 tooltip、日期选择器…）都由 MaterialLocalizations 提供。
          // 不显式指定 zh_CN 的话，即使界面自身是中文，这些菜单也会是英文。
          locale: const Locale('zh', 'CN'),
          localizationsDelegates: const [
            GlobalMaterialLocalizations.delegate,
            GlobalWidgetsLocalizations.delegate,
            GlobalCupertinoLocalizations.delegate,
          ],
          supportedLocales: const [Locale('zh', 'CN'), Locale('en')],
          // 外观默认深色，可在设置页切浅色 / 跟随系统（`SettingsStore.themeMode`）
          themeMode: settings.themeMode.material,
          // 字体 / 字号 / 行高在 core/theme.dart 里统一按"中文友好"调过；
          // 种子色来自平台强调色（取不到就是默认紫，见 core/system_accent.dart）
          theme: _themeFor(Brightness.light),
          darkTheme: _themeFor(Brightness.dark),
          home: const HomeShell(),
        ),
      ),
    );
  }

  /// 两套主题**按（明暗 × 种子色）各构建一次**并缓存。
  ///
  /// ⚠ 别退回"在 builder 里现调 `AppTheme.build(...)`"：`ColorScheme.fromSeed`
  /// 每次都要跑一遍配色算法，而上面这个 builder 跟着 `settings` 的**任何**通知
  /// 重建 —— 拖一下设置页的「每页数量」滑杆就是每帧重算两套配色。
  /// `ThemeData` 是常量数据，同一种种子构建一次用到底即可。
  static final Map<String, ThemeData> _themeCache = <String, ThemeData>{};

  ThemeData _themeFor(Brightness brightness) => themeFor(settings, brightness);

  /// **外观的唯一拼装点**（静态是为了让用例直接验"系统强调色开 / 关时
  /// 到底用了哪个种子色"，不用把整个 `HomeShell` 拉起来）。
  @visibleForTesting
  static ThemeData themeFor(SettingsStore settings, Brightness brightness) {
    final mode = settings.accentMode;
    final accent = mode.usesSystemColor ? SystemAccent.color : null;
    final seed = accent ?? AppTheme.defaultSeed;
    final key = '${brightness.index}:${seed.toARGB32()}:${mode.name}';
    return _themeCache.putIfAbsent(
      key,
      () => AppTheme.build(brightness, seedColor: seed, accentMode: mode),
    );
  }
}

/// 主框架：宽屏用 NavigationRail，窄屏用底部导航。
///
/// ★ **它位于 widget 树最高层**（"只替换右侧内容区，左侧侧边栏常驻，
/// 你把侧边栏改成最高级，然后窗口宽度低于阈值的时候换掉不就得了？"）。
///
/// 以前 `StartupGate` 把 `HomeShell` 包在里面，于是**后端一重启**（`isReady` 变假）
/// 整个 `HomeShell` 被换成加载页 —— 侧边栏连同五页状态全被销毁，看着就是"前端热重启了"。
/// 现在顺序反过来：
///
///   * `HomeShell`（含侧边栏 / 底部导航）**永远挂着**，是整棵树的最外层；
///   * 启动中的加载页退化成它上面的一层**覆盖物**（[`_StartupOverlay`]）；
///   * 窗口宽度低于阈值时，换的是**导航形态**（NavigationRail ↔ NavigationBar），
///     不是"换掉壳子"。
class HomeShell extends StatefulWidget {
  const HomeShell({super.key});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

/// 启动闸门：先把后端拉起来，再进主界面。
///
/// 后端没起来时也能「跳过」进主界面（离线浏览最后一次的结果 / 去设置里改地址）。
///
/// ⚠ 2026-10-11 起**这些职责都在 `HomeShell` 里**（见那里的类注释）：
/// 壳子必须是最高级，加载页只能是它上面的一层覆盖物。
class _StartupView extends StatelessWidget {
  final BackendLauncher launcher;
  final Future<void> Function() onRetry;
  final VoidCallback onSkip;
  final VoidCallback onCancel;

  const _StartupView({
    required this.launcher,
    required this.onRetry,
    required this.onSkip,
    required this.onCancel,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final failed = launcher.phase == StartupPhase.failed;
    final tail = launcher.logLines.length > 40
        ? launcher.logLines.sublist(launcher.logLines.length - 40)
        : launcher.logLines;

    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 720),
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    const _BrandMark(),
                    const SizedBox(width: 14),
                    Text('ComfyHub', style: theme.textTheme.headlineSmall),
                  ],
                ),
                const SizedBox(height: 20),
                Row(
                  children: [
                    if (!failed)
                      const SizedBox(
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    else
                      Icon(
                        Icons.error_outline,
                        size: 20,
                        color: theme.colorScheme.error,
                      ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        launcher.message,
                        style: theme.textTheme.titleMedium,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                Text(
                  '正在自动准备本地服务，包括本地 SQLite 数据库和 Rust 后端。'
                  '后端由 App 自己直接启动，不再需要 PowerShell。',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
                const SizedBox(height: 16),
                if (tail.isNotEmpty)
                  Container(
                    height: 200,
                    width: double.infinity,
                    padding: const EdgeInsets.all(10),
                    decoration: BoxDecoration(
                      color: theme.colorScheme.surfaceContainerHighest
                          .withValues(alpha: 0.35),
                      borderRadius: BorderRadius.circular(8),
                      border: Border.all(color: theme.dividerColor),
                    ),
                    child: ListView.builder(
                      reverse: true,
                      itemCount: tail.length,
                      itemBuilder: (_, i) {
                        final line = tail[tail.length - 1 - i];
                        return Text(
                          line,
                          style: theme.textTheme.labelSmall?.copyWith(
                            fontFamily: 'monospace',
                            color: theme.colorScheme.onSurfaceVariant,
                          ),
                        );
                      },
                    ),
                  ),
                const SizedBox(height: 16),
                Row(
                  children: [
                    if (!failed)
                      OutlinedButton.icon(
                        onPressed: onCancel,
                        icon: const Icon(Icons.stop_circle_outlined),
                        label: const Text('取消'),
                      ),
                    if (failed)
                      FilledButton.icon(
                        onPressed: () => onRetry(),
                        icon: const Icon(Icons.refresh),
                        label: const Text('重试'),
                      ),
                    const SizedBox(width: 10),
                    TextButton(
                      onPressed: onSkip,
                      child: const Text('跳过，先进入界面'),
                    ),
                  ],
                ),
                if (launcher.projectRoot != null) ...[
                  const SizedBox(height: 4),
                  Text(
                    '项目目录: ${launcher.projectRoot}',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _HomeShellState extends State<HomeShell> {
  // 默认落在「AI 工作台」（AIH-001）—— 打开 App 先进入对话，画廊退到第二位
  int _index = 0;

  // ---------------------------------------------------------------------------
  //  启动 / 后端拉起（原 `StartupGate` 的职责，2026-10-11 并进来）
  // ---------------------------------------------------------------------------

  BackendLauncher? _launcher;
  SettingsStore? _settings;
  AppLifecycleListener? _lifecycle;
  bool _bootstrapped = false;
  bool _refreshed = false;

  /// **曾经就绪过**（一旦为真就再也不回落）。
  ///
  /// 约束："后端需静默重启，不显示加载中界面，避免整个前端热重启；
  /// 我要的桌面端侧边栏常驻呢？"
  ///
  /// 真因是以前 `StartupGate` 把 `HomeShell` 包在里面：后端**重启**
  /// （换端口 / 手动重启 / 崩溃重拉）时 `isReady` 变回 false，于是整个 `HomeShell`
  /// 被替换成加载页 —— 侧边栏连同五页状态**全被销毁**，看着就是"前端热重启了"。
  ///
  /// 现在壳子是最高级，加载页只是它上面的一层覆盖物：一旦进过主界面，
  /// 后端再怎么起落都只在**内容区**里体现（各页面自己会如实报"连不上"）。
  bool _everReady = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _settings = context.read<SettingsStore>();
    final launcher = context.read<BackendLauncher>();
    if (!identical(launcher, _launcher)) {
      _launcher?.removeListener(_onLauncherChanged);
      _launcher = launcher..addListener(_onLauncherChanged);
    }
  }

  @override
  void dispose() {
    _lifecycle?.dispose();
    _discovery?.cancel();
    _discovery = null;
    _launcher?.removeListener(_onLauncherChanged);
    super.dispose();
  }

  /// 关窗口时把**本地服务**停掉：`BackendLauncher.releaseOnExit()` ——
  /// 它 kill 的是**这次由 App 拉起来的那个后端进程**（以前是调
  /// `scripts\comfyhub.ps1 release`，那条脚本链路 2026-09-30 已删）。
  /// Windows 上这是唯一可靠的收尾时机 —— 模板 runner
  /// （windows\runner\flutter_window.cpp:50-70）把窗口消息交给引擎，
  /// WM_CLOSE 会被转成 onExitRequested；而 detach / dispose 之后 Dart 侧已经
  /// 不适合再起子进程（onDetach 在桌面端也不可靠，注册它反而可能在启动时就误触发）。
  ///
  /// 无论停没停成，都必须返回 exit：绝不能让 App 关不掉。
  Future<AppExitResponse> _onExitRequested() async {
    final launcher = _launcher;
    if (launcher != null && (_settings?.stopServicesOnExit ?? false)) {
      try {
        await launcher.releaseOnExit();
      } catch (_) {
        // 收尾失败也要照常关闭
      }
    }
    return AppExitResponse.exit;
  }

  void _onLauncherChanged() {
    final launcher = _launcher;
    if (launcher == null || !launcher.isReady || _refreshed) return;
    _refreshed = true;
    // 后端刚就绪（可能是**重启**后的新端口）：先按 `.run\api.json` 重新发现一次地址，
    // 再拉数据。
    //
    // 为什么必须重新发现（2026-09-22）：后端端口现在是**自动挑的**（默认 0），
    // 重启会换一个端口。App 冷启动时读到的可能还是上一轮那个（或者默认 8080），
    // 不重新读取时会持续连接无人监听的端口，界面显示"启动页提示后端已就绪，
    // 界面却无法获取数据"。手动填写过地址时该步骤不执行（属于远程配置）。
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      final settings = context.read<SettingsStore>();
      await settings.refreshBaseUrlFromPortFile();
      if (!mounted) return;
      context.read<LibraryStore>().refreshAll();
      // 受限状态也**这时候**才探（2026-10-12）：以前它在 `initState` 里直接打，
      // 而那一刻后端还没起来、地址还是 8080 —— 探测必然失败，被静默吞掉之后
      // 「后端受限」那条常驻横幅**冷启动永远不出现**（AGENTS §12 的"看得见"就落空了）。
      await _probeConfinement();
    });
  }

  Future<void> _bootstrap() async {
    if (_bootstrapped || !mounted) return;
    _bootstrapped = true;
    final launcher = context.read<BackendLauncher>();
    final settings = context.read<SettingsStore>();
    if (!settings.autoStartBackend) {
      launcher.skip();
      return;
    }
    // 安卓（以及任何非 Windows 平台）**没有"本地服务"这回事**：手机连的是桌面端
    // 的后端（配对而来），桌面离线时聊天页自己会如实报错。把 Windows 那套
    // "App 启动后端"的启动页搬上来只会遮挡一屏（现象：
    // "安卓启动时会显示 Windows 的加载界面，但是没有用"）—— 直接跳过进主界面。
    if (kIsWeb || !Platform.isWindows) {
      launcher.skip();
      return;
    }
    await launcher.ensureRunning();
  }

  Future<void> _retry() async {
    _refreshed = false;
    await _launcher?.ensureRunning(force: true);
  }

  /// 后端"受不受限"（写不写得进项目外）。**受限时横幅常驻** —— 受限是环境事实，
  /// 界面上不显示时仅工具文案与 doctor 会说明，模型只能推测。
  ConfinementInfo? _confinement;

  /// 局域网桌面发现（Android）。换 Wi-Fi / 桌面换 IP 时靠它自动改地址；
  /// **不改配对令牌** —— 只把 baseUrl 换成探测通过的那一条。
  StreamSubscription<Uri>? _discovery;

  static const _destinations = [
    _Dest('AI 工作台', Icons.auto_awesome_outlined, Icons.auto_awesome),
    _Dest('画廊', Icons.photo_library_outlined, Icons.photo_library),
    _Dest('预设', Icons.text_snippet_outlined, Icons.text_snippet),
    _Dest('标签', Icons.sell_outlined, Icons.sell),
    _Dest('设置', Icons.settings_outlined, Icons.settings),
  ];

  /// 五页**只建一次**，之后一直挂着（见 [build] 里的 `IndexedStack`）。
  ///
  /// ⚠ 为什么不能像以前那样"在 `build` 里现拼一个 `pages` 列表"：
  /// 那样每次 `setState` 都产生**新的 widget 实例**，Flutter 于是把旧页面
  /// **销毁重建**（`initState` 再跑一遍、滚动位置归零、正在输入的框被清空），
  /// 而且整棵树（含左侧 `NavigationRail`）一起重建 —— 此前报告的问题为
  /// "侧边栏没有置顶、换页时右边被整个换掉"就是这个。
  ///
  /// 每一项都套一个 [`_ContentNavigator`]（**内容区自己的路由栈**）——
  /// 见那个类的注释：次级页面（产物详情 / 预设详情 / 设置子页…）**只替换右边**，
  /// 不许盖住侧边栏。
  late final List<Widget> _pages = [
    const _ContentNavigator(child: AiHomePage()),
    const _ContentNavigator(child: GalleryPage()),
    _ContentNavigator(
      child: PromptsPage(onSendToWorkspace: _handoffToWorkspace),
    ),
    const _ContentNavigator(child: TagsPage()),
    const _ContentNavigator(child: SettingsPage()),
  ];

  /// 已经访问过的页（首次访问才真的建，之后常驻）。
  ///
  /// 为什么不是"一上来五页全建"：`IndexedStack` 会把**所有** children 都挂载，
  /// 于是设置页在启动时就要 `BackendLauncher`、画廊在启动时就要拉列表 ——
  /// 一个页面缺依赖会使整个外壳报错（`home_nav_test` 即如此失败）。
  /// 而且"没点过的页面也去拉数据"纯属浪费。
  ///
  /// 所以：**点过就留下，没点过就是空位**。既拿到了"换页不重建"，
  /// 又没有"启动时全量初始化"的代价。
  final Set<int> _visited = {0};

  /// 未访问页的占位（`IndexedStack` 要求每个位置都有 child）。
  static const _placeholder = SizedBox.shrink();

  /// 切页：**唯一的入口**（导航栏 / 横幅 / 预设"运行"都走它）。
  ///
  /// 同时把目标页标记为"已访问"—— 这样它才会被实际创建（见 [`_visited`]）。
  /// 散着写 `setState(() => _index = i)` 的后果是：页面切过去了，但那一格还是占位。
  void _goToPage(int index) {
    setState(() {
      _index = index;
      _visited.add(index);
    });
  }

  @override
  void initState() {
    super.initState();
    // 「关 App 就停服务」的窗口关闭钩子（见 _onExitRequested）
    _lifecycle = AppLifecycleListener(onExitRequested: _onExitRequested);
    WidgetsBinding.instance.addPostFrameCallback((_) => _bootstrap());
    // ⚠ 受限探测**不在这儿打**：这一刻后端还没起来（见 `_onLauncherChanged` 里的调用）
    _pickDesktopAutomatically();
  }

  /// 「预设 ▶️运行」的落点：把文本交给工作台的输入框，然后**切到工作台那一页**。
  ///
  /// 三条纪律：
  ///  1. **只交接、不自动发送** —— 仍需手动按发送（约束：
  ///     不产生"无人值守的提交"，`comfy_submit` 那道审批闸门一个字都不改）；
  ///  2. **当前会话非空时先新建一条对话** ——
  ///     否则"运行预设"会把新提示词接在上一段聊天后面，上下文串味。
  ///     当前会话本来就是空的（刚开的"新对话"）就直接用，**不白建一条**；
  ///  3. 交接走 [`AiWorkspaceStore.handoffToComposer`]（取走即清空），
  ///     页面自己不 push 第二个工作台实例。
  Future<void> _handoffToWorkspace(String text) async {
    final store = context.read<AiWorkspaceStore>();
    // 先切页：点击"运行"后立即显示工作台，新建会话在后台完成后填入输入框
    _goToPage(0);
    if (store.messages.isNotEmpty) {
      await store.newConversation();
    }
    if (!mounted) return;
    store.handoffToComposer(text);
  }

  /// **自动选择一台可连接的桌面**（跨端处理应自动完成）。
  ///
  /// 只在手机上做：桌面的地址由 `.run\api.json` 自动决定（[SettingsStore.defaultBaseUrl]），
  /// 且**手动填写过地址时不进行任何尝试**（[SettingsStore.baseUrlIsAuto] 为假 = 远程配置）。
  ///
  /// 静默失败，一条都不通时保持现状、不弹任何东西 —— 真正要用到后端时，
  /// 各页面本就会如实报告"连不上"（该路径更贴近当时的实际操作）。
  Future<void> _pickDesktopAutomatically() async {
    if (kIsWeb) return;
    try {
      if (!Platform.isAndroid) return;
    } catch (_) {
      return;
    }
    final settings = context.read<SettingsStore>();
    if (settings.baseUrlIsAuto) {
      final picked = await autoPickBaseUrl(
        autoUrl: SettingsStore.defaultBaseUrl(),
        current: settings.baseUrl,
        history: settings.baseUrlHistory,
      );
      if (picked.baseUrl != null) {
        await settings.adoptAutoBaseUrl(picked.baseUrl!);
      }
    }
    // 手填过地址也要挂发现：换 Wi-Fi / 桌面换 IP 后，旧地址会失效，
    // 但**配对令牌仍然有效** —— 发现到新地址只改 baseUrl，不重新配对。
    _startDiscovery(settings);
  }

  bool _discoveryBusy = false;

  /// 听桌面广播；只接受**当前令牌探测通过**的那一条，绝不让广播改掉令牌。
  void _startDiscovery(SettingsStore settings) {
    if (_discovery != null || !LanDiscovery.supported) return;
    _discovery = LanDiscovery.endpoints.listen(
      (uri) async {
        if (!mounted || _discoveryBusy) return;
        final token = PairingTokenCache.value;
        // 未配对 = 尚不具备自动切换地址的条件；需先扫码 / 手动填写。
        if (token == null || token.isEmpty) return;
        _discoveryBusy = true;
        try {
          // 当前地址还能用就不动它 —— 发现是"救失联"，不是"每次广播都重连"。
          final current = await probeSyncEndpoint(
            settings.baseUrl,
            token: token,
            timeout: const Duration(milliseconds: 1500),
          );
          if (current.ok || !mounted) return;
          final probe = await probeSyncEndpoint(
            uri.toString(),
            token: token,
            timeout: const Duration(seconds: 2),
          );
          if (!probe.ok || !mounted) return;
          final changed = await settings.adoptAutoBaseUrl(uri.toString());
          if (!changed || !mounted) return;
          // 地址换了，规则工件 / 同步也要按新地址重来一次；令牌仍是同一把。
          await context.read<AiWorkspaceStore>().reloadPairingToken();
          if (mounted) context.read<LibraryStore>().refreshAll();
        } catch (_) {
          // 发现失败不能影响任何页面：下一次广播还会再来
        } finally {
          _discoveryBusy = false;
        }
      },
      onError: (Object error, StackTrace stack) {
        // UDP 被系统回收 / 端口被占：静默降级成"手填地址"，不弹错误。
        debugPrint('[discovery] $error');
      },
    );
  }

  /// 只问一次。问不到（后端还没起 / 老版本没有这个端点）就当"不知道"：
  /// **别亮横幅** —— 那不是受限的证据。
  Future<void> _probeConfinement() async {
    try {
      final info = await context.read<LibraryStore>().api.confinement();
      if (!mounted) return;
      setState(() => _confinement = info);
    } catch (_) {
      // 同上：静默。真正撞到时工具文案与 `doctor` 会说。
    }
  }

  @override
  Widget build(BuildContext context) {
    final wide = MediaQuery.sizeOf(context).width >= kWideLayoutMinWidth;
    // 每页数量：电脑端"自动"按窗口大小排满一页。先更新尺寸再建页面，
    // 页面第一次请求就会用上算出来的数量，不会先用固定值拉一遍再重拉。
    context.read<LibraryStore>().updateViewport(MediaQuery.sizeOf(context));
    // 横幅在**内容之上**（不是浮层）：它是常驻状态，不是转瞬的提示
    final body = Column(
      children: [
        ConfinementBanner(
          info: _confinement,
          onOpenSettings: () => _goToPage(4),
        ),
        // "库不在你以为的地方"（指针 ≠ 实际数据 / 产物目录没跟过来）：同样是常驻状态，
        // 摆在内容之上。正常时它是 `SizedBox.shrink()`，不占高度（2026-11-10）。
        DataDirNoticeBanner(
          notice: context.watch<BackendLauncher>().dataDirNotice,
          onOpenSettings: () => _goToPage(4),
        ),
        // **页面常驻、仅切换可见性**（"桌面端 UI 应始终保留主界面
        // 侧边栏置顶，更换页面的时候把右边界面换掉即可"）。
        //
        // `IndexedStack` 把五页**全部保持挂载**，只显示 `_index` 那一页：
        //   · 换页**不重建**任何页面 —— 画廊的滚动位置、预设的搜索词、设置里填了一半的
        //     表单都不会丢失（这是同一问题的另一部分）；
        //   · 代价：五页的 `initState` 在启动时就都跑了（各自拉一次数据）。
        //     这是可接受的 —— 切到任意页都需要拉取，而"换页丢状态"属于明确缺陷。
        Expanded(
          child: IndexedStack(
            index: _index,
            sizing: StackFit.expand,
            children: [
              for (var i = 0; i < _pages.length; i++)
                // 每页各自一层缓存：来回切页时直接复用上一帧的图层，不把整页
                // （画廊几十张卡片）重新画一遍。
                RepaintBoundary(
                  child: _visited.contains(i) ? _pages[i] : _placeholder,
                ),
            ],
          ),
        ),
      ],
    );

    // **导航形态**由窗口宽度决定（不是平台）：宽了用侧边栏、窄了用底部导航。
    // ⚠ 换的是"形态"，不是"壳子" —— `HomeShell` 本身永远是同一个 State。
    final Widget shell = wide
        ? Scaffold(
            // 侧边栏在 `Row` 里**稳定存在**：`_index` 变化只让 `IndexedStack` 换一页，
            // 导航栏这一侧不参与重建（`_railDestinations` 是 `static final`，
            // 只求值一次）。
            body: Row(
              children: [
                NavigationRail(
                  selectedIndex: _index,
                  onDestinationSelected: _goToPage,
                  labelType: NavigationRailLabelType.all,
                  leading: const Padding(
                    padding: EdgeInsets.symmetric(vertical: 12),
                    child: _BrandMark(),
                  ),
                  destinations: _railDestinations,
                ),
                // 侧边栏与内容区之间**不画分隔线**：侧边栏底色是
                // `surfaceContainerLow`（见主题的 `navigationRailTheme`），内容区是
                // `surface` —— 层次由底色差表达，与 Material 3 的分区方式一致。
                Expanded(child: body),
              ],
            ),
          )
        : Scaffold(
            body: body,
            bottomNavigationBar: NavigationBar(
              selectedIndex: _index,
              onDestinationSelected: _goToPage,
              destinations: _barDestinations,
            ),
          );

    // **冷启动那一次**才盖加载页（见 `_everReady`）：
    // 后端重启时不盖 —— 壳子与五页状态原地不动，各页面自己会如实报"连不上"。
    final launcher = context.watch<BackendLauncher>();
    if (launcher.isReady) _everReady = true;
    if (_everReady || launcher.isReady) return shell;

    return _StartupOverlay(
      startup: _StartupView(
        launcher: launcher,
        onRetry: _retry,
        onSkip: () => launcher.skip(),
        onCancel: () => launcher.cancel(),
      ),
      child: shell,
    );
  }

  /// 导航栏的两份目标列表 —— 从 [`_destinations`] **派生一次**就定下来，不随 build 重建。
  ///
  /// ⚠ 为什么不直接在 `build` 里现拼：那样每次 `setState` 都产生新的目标 widget，
  /// Flutter 会重新挂载导航栏（"侧边栏闪一下"的来源）。
  /// ⚠ 也不能写成 `static const` + `for` 元素 —— Dart 的常量表达式不支持 `for`
  /// （报 `Constant expressions don't support 'for' elements`）。
  /// `static final` 只在类初始化时求值一次，之后每次 build 拿到的是**同一批实例**，
  /// 对 element 树来说效果一样。顺序必须与 [`_destinations`] 一致。
  static final _railDestinations = [
    for (final d in _destinations)
      NavigationRailDestination(
        icon: Icon(d.icon),
        selectedIcon: Icon(d.selectedIcon),
        label: Text(d.label),
      ),
  ];

  static final _barDestinations = [
    for (final d in _destinations)
      NavigationDestination(
        icon: Icon(d.icon),
        selectedIcon: Icon(d.selectedIcon),
        label: d.label,
      ),
  ];
}

/// **内容区自己的路由栈**：让次级页面只替换右边，不许盖住侧边栏。
///
/// 两条相关约束：
///   ① "桌面端 UI 应该始终把主界面侧边栏置顶，更换页面的时候把右边界面换掉即可"；
///   ② 看到产物详情页（`test_00350_.png` 那张）之后："侧边栏哪去了？次级菜单还是没有
///      只替换右边的内容？"
///
/// 根因：详情页走的是 `Navigator.of(context).push(...)` —— 那是 `MaterialApp` 的
/// **根 Navigator**，它的路由铺满**整个窗口**，于是连侧边栏一起盖住。
///
/// 修法：每个主页面各套一个 `Navigator`（就是这一层）。页面里的
/// `Navigator.of(context).push(...)` 于是落在**自己这一格**里：
///   · 侧边栏 / 底部导航完全不受影响（它们在 `Navigator` 外面）；
///   · 每页的路由栈互相独立 —— 画廊翻到详情、切去预设再切回来，
///     画廊仍停留在详情页（符合预期：未按返回）；
///   · 对话框（`showDialog` / `showModalBottomSheet`）默认走**根** navigator，
///     仍然铺满整个窗口 —— 那是对的，弹窗本来就该盖住一切。
///
/// ⚠ `onGenerateRoute` 里必须把 `settings` **原样带下去**：路由名是 Flutter 用来
/// 认"同一个路由"的，丢了它会让 `pop` 的结果匹配不上（`showDialog` 的 future 挂住）。
class _ContentNavigator extends StatelessWidget {
  final Widget child;

  const _ContentNavigator({required this.child});

  @override
  Widget build(BuildContext context) {
    // ⚠ **窄屏（移动端形态）不套这一层 Navigator**：
    //
    //   "移动端 UI 中，底边栏不是像桌面端 UI 那样侧边栏常驻的，次级菜单应该覆盖全部界面。"
    //
    // 于是次级页（产物详情 / 预设详情 / 标签结果…）里那句 `Navigator.of(context).push`
    // 落到 `MaterialApp` 的**根** Navigator 上 —— 路由铺满整个窗口，
    // **底部导航栏也被盖上**（手机上"进详情页"就该是整屏换页，而不是"半屏下面还挂着导航栏"）。
    //
    // 宽屏相反：侧边栏常驻，次级页只替换右边那一格（见上面那段注释）。
    // 两种形态由**同一个宽度阈值**决定（[kWideLayoutMinWidth]），别在别处再写一个数字。
    if (MediaQuery.sizeOf(context).width < kWideLayoutMinWidth) return child;
    return Navigator(
      onGenerateRoute: (settings) =>
          MaterialPageRoute<void>(settings: settings, builder: (_) => child),
    );
  }
}

/// 把加载页盖在壳子**上面**（而不是替换壳子）。
///
/// 约束："侧边栏改为最高级，窗口宽度低于阈值时替换
/// 不就得了？" —— 对。以前 `StartupGate` 用 `return HomeShell()` / `return _StartupView()`
/// **二选一**，后端一重启就整棵树换掉。现在壳子始终是底，加载页只是上面一层。
///
/// 用 `Stack` + 不透明底色（不是 `Dialog` / 路由）：冷启动时它必须**盖住整个窗口**
/// （包括侧边栏），因为那时侧边栏里的每一页都还没数据、点也没用。
/// 而一旦 `_everReady` 为真就整个不再挂 —— 后端重启不会有任何遮挡。
class _StartupOverlay extends StatelessWidget {
  final Widget startup;
  final Widget child;

  const _StartupOverlay({required this.startup, required this.child});

  @override
  Widget build(BuildContext context) {
    return Stack(
      children: [
        child,
        Positioned.fill(
          child: Material(
            color: Theme.of(context).colorScheme.surface,
            child: startup,
          ),
        ),
      ],
    );
  }
}

class _Dest {
  final String label;
  final IconData icon;
  final IconData selectedIcon;
  const _Dest(this.label, this.icon, this.selectedIcon);
}

class _BrandMark extends StatelessWidget {
  const _BrandMark();

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      width: 40,
      height: 40,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(12),
        gradient: LinearGradient(
          colors: [scheme.primary, scheme.tertiary],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
      ),
      // 文字颜色跟着主色走（`onPrimary`）：深色主题的主色是浅色调，白字会糊；
      // 浅色主题的主色是深色调，深字会糊 —— 只有 on 角色在两套里都成立。
      child: Center(
        child: Text(
          'C',
          style: TextStyle(
            color: scheme.onPrimary,
            fontWeight: FontWeight.w900,
            fontSize: 20,
          ),
        ),
      ),
    );
  }
}
