import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:provider/provider.dart';

import 'core/backend_launcher.dart';
import 'core/settings_store.dart';
import 'core/theme.dart';
import 'pages/ai_home_page.dart';
import 'pages/gallery_page.dart';
import 'pages/prompts_page.dart';
import 'pages/settings_page.dart';
import 'pages/tags_page.dart';
import 'state/ai_workspace_store.dart';
import 'state/library_store.dart';

class ComfyHubApp extends StatelessWidget {
  final SettingsStore settings;
  final BackendLauncher launcher;

  const ComfyHubApp({super.key, required this.settings, required this.launcher});

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
        ChangeNotifierProvider<AiWorkspaceStore>(
          create: (ctx) => AiWorkspaceStore(
            baseUrlProvider: () => ctx.read<SettingsStore>().baseUrl,
          ),
        ),
      ],
      child: MaterialApp(
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
        themeMode: ThemeMode.dark,
        // 字体 / 字号 / 行高在 core/theme.dart 里统一按"中文友好"调过
        theme: AppTheme.build(Brightness.light),
        darkTheme: AppTheme.build(Brightness.dark),
        home: const StartupGate(),
      ),
    );
  }
}

/// 启动闸门：先把 MySQL + 后端拉起来，再进主界面。
///
/// 后端没起来时也能「跳过」进主界面（离线浏览最后一次的结果 / 去设置里改地址）。
class StartupGate extends StatefulWidget {
  const StartupGate({super.key});

  @override
  State<StartupGate> createState() => _StartupGateState();
}

class _StartupGateState extends State<StartupGate> {
  BackendLauncher? _launcher;
  SettingsStore? _settings;
  AppLifecycleListener? _lifecycle;
  bool _bootstrapped = false;
  bool _refreshed = false;

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
    _launcher?.removeListener(_onLauncherChanged);
    super.dispose();
  }

  /// 关窗口时把**本地服务**停掉：`scripts\comfyhub.ps1 release`（只停后端 + MySQL，
  /// 不碰 App 自己）。Windows 上这是唯一可靠的收尾时机 —— 模板 runner
  /// （windows\runner\flutter_window.cpp:50-70）把窗口消息交给引擎，
  /// WM_CLOSE 会被转成 onExitRequested；而 detach / dispose 之后 Dart 侧已经
  /// 不适合再起子进程（onDetach 在桌面端也不可靠，注册它反而可能在启动时就误触发）。
  ///
  /// 无论脚本成功、失败还是没有，都必须返回 exit：绝不能让 App 关不掉。
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
    // 后端刚就绪：把提示词 / 画廊 / 标签 / 统计拉一遍
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) context.read<LibraryStore>().refreshAll();
    });
  }

  @override
  void initState() {
    super.initState();
    // 「关 App 就停服务」的窗口关闭钩子（见 _onExitRequested）
    _lifecycle = AppLifecycleListener(onExitRequested: _onExitRequested);
    WidgetsBinding.instance.addPostFrameCallback((_) => _bootstrap());
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
    await launcher.ensureRunning();
  }

  Future<void> _retry() async {
    _refreshed = false;
    await _launcher?.ensureRunning(force: true);
  }

  @override
  Widget build(BuildContext context) {
    final launcher = context.watch<BackendLauncher>();
    if (launcher.isReady) return const HomeShell();
    return _StartupView(
      launcher: launcher,
      onRetry: _retry,
      onSkip: () => launcher.skip(),
      onCancel: () => launcher.cancel(),
    );
  }
}

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
                      Icon(Icons.error_outline, size: 20, color: theme.colorScheme.error),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(launcher.message, style: theme.textTheme.titleMedium),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                Text(
                  '正在自动准备本地服务：MySQL（127.0.0.1:3307）→ Kotlin 后端（127.0.0.1:8080）。'
                  '首次启动或需要构建后端时会慢一些。',
                  style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                ),
                const SizedBox(height: 16),
                if (tail.isNotEmpty)
                  Container(
                    height: 200,
                    width: double.infinity,
                    padding: const EdgeInsets.all(10),
                    decoration: BoxDecoration(
                      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
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
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
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

/// 主框架：宽屏用 NavigationRail，窄屏用底部导航。
class HomeShell extends StatefulWidget {
  const HomeShell({super.key});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  // 默认落在「AI 工作台」（AIH-001）—— 打开 App 先进入对话，画廊退到第二位
  int _index = 0;

  static const _destinations = [
    _Dest('AI 工作台', Icons.auto_awesome_outlined, Icons.auto_awesome),
    _Dest('画廊', Icons.photo_library_outlined, Icons.photo_library),
    _Dest('提示词', Icons.text_snippet_outlined, Icons.text_snippet),
    _Dest('标签', Icons.sell_outlined, Icons.sell),
    _Dest('设置', Icons.settings_outlined, Icons.settings),
  ];

  @override
  Widget build(BuildContext context) {
    final wide = MediaQuery.sizeOf(context).width >= 900;
    const pages = [
      AiHomePage(),
      GalleryPage(),
      PromptsPage(),
      TagsPage(),
      SettingsPage(),
    ];

    if (wide) {
      return Scaffold(
        body: Row(
          children: [
            NavigationRail(
              selectedIndex: _index,
              onDestinationSelected: (i) => setState(() => _index = i),
              labelType: NavigationRailLabelType.all,
              leading: const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: _BrandMark(),
              ),
              destinations: [
                for (final d in _destinations)
                  NavigationRailDestination(
                    icon: Icon(d.icon),
                    selectedIcon: Icon(d.selectedIcon),
                    label: Text(d.label),
                  ),
              ],
            ),
            const VerticalDivider(width: 1),
            Expanded(child: pages[_index]),
          ],
        ),
      );
    }

    return Scaffold(
      body: pages[_index],
      bottomNavigationBar: NavigationBar(
        selectedIndex: _index,
        onDestinationSelected: (i) => setState(() => _index = i),
        destinations: [
          for (final d in _destinations)
            NavigationDestination(
              icon: Icon(d.icon),
              selectedIcon: Icon(d.selectedIcon),
              label: d.label,
            ),
        ],
      ),
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
      child: const Center(
        child: Text('C', style: TextStyle(color: Colors.white, fontWeight: FontWeight.w900, fontSize: 20)),
      ),
    );
  }
}
