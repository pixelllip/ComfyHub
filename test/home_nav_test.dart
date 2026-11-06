// 首页落地页的回归测试：
//
//   需求（AIH-001 / DEC-001）是「打开 App 就落在 AI 工作台」，导航顺序
//   AI 工作台 → 画廊 → 提示词 → 标签 → 设置。
//   这里不依赖真实后端（用 MockClient 返回空数据），断言的是 lib/app.dart 里
//   _destinations 与 pages 两个列表的顺序和默认选中项。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/app.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/state/library_store.dart';

/// 空后端：AI 侧返回空列表，画廊 / 提示词返回空分页。
MockClient _emptyBackend() {
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path.startsWith('/api/ai/')) {
      body = <Object>[];
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': 0,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': <String, int>{},
        'byMediaKind': <String, int>{},
      };
    } else {
      body = {
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 24,
        'pages': 0,
      };
    }
    return http.Response(
      jsonEncode(body),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Future<Widget> _makeShell() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final mock = _emptyBackend();
  final library = LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: mock),
  );
  await library.refreshAll();
  final ai = AiWorkspaceStore(api: AiApiClient(settings.baseUrl, client: mock));

  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<LibraryStore>.value(value: library),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: ai),
      // ⚠ 2026-10-11 起 `HomeShell` **自己**负责拉起后端（原 `StartupGate` 的职责并进来了，
      // 因为壳子必须是最高级：后端重启不许把壳子换掉）。所以它要 `BackendLauncher`。
      // 用 `skip()` 的实例：测试里不真的起后端，也不显示加载页。
      ChangeNotifierProvider<BackendLauncher>(
        create: (_) => BackendLauncher(settings)..skip(),
      ),
    ],
    child: const MaterialApp(home: HomeShell()),
  );
}

void main() {
  testWidgets('App 默认落在 AI 工作台，导航顺序是 AI 工作台 → 画廊 → 提示词 → 标签 → 设置', (
    tester,
  ) async {
    // 宽屏（>=900）才会走 NavigationRail 布局，和真实桌面端一致
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _makeShell());
    await tester.pumpAndSettle();

    // 1) 首屏就是 AI 工作台，不是画廊
    expect(find.byType(AiHomePage), findsOneWidget);
    expect(find.byType(GalleryPage), findsNothing);

    // 2) 导航：默认选中第 0 个，顺序以 AI 工作台打头
    final rail = tester.widget<NavigationRail>(find.byType(NavigationRail));
    expect(rail.selectedIndex, 0);
    final labels = rail.destinations
        .map((d) => (d.label as Text).data)
        .toList();
    // 「提示词」2026-10-11 改组成「预设」（用户：预设 = 可复用的一段提示词 + 用途）
    expect(labels, ['AI 工作台', '画廊', '预设', '标签', '设置']);

    // 3) 点第二个才进画廊。
    //    ⚠ 判据是 `findsNothing` **且这不是"被销毁了"** —— `IndexedStack` 把没选中的页
    //    放进 `Offstage`，而 finder 默认跳过 offstage 子树，所以"找不到"= "没在显示"。
    //    页面本身仍然挂在树上（状态、滚动位置都保住了），见第 5 条的侧边栏断言。
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    expect(find.byType(GalleryPage), findsOneWidget);
    expect(
      tester.getRect(find.byType(GalleryPage)).height,
      greaterThan(0),
      reason: '画廊必须真的占着内容区',
    );
    expect(find.byType(AiHomePage), findsNothing, reason: '工作台被藏起来了（不是被销毁）');

    // 4) 第三个是预设库
    await tester.tap(find.text('预设'));
    await tester.pumpAndSettle();
    expect(find.byType(PromptsPage), findsOneWidget);
    expect(tester.getRect(find.byType(PromptsPage)).height, greaterThan(0));

    // 5) ★ **侧边栏必须常驻**（用户 2026-10-11："侧边栏没有常驻显示"）。
    //
    // 判据用**几何**：换页前后 `NavigationRail` 的矩形必须逐像素相同。
    // 光看 `findsOneWidget` 抓不到这个 bug —— 每次 build 现拼一个导航栏也只有一个，
    // 但它会重新挂载（视觉上闪、滚动位置/动画状态全丢）。
    final railRect = tester.getRect(find.byType(NavigationRail));
    expect(railRect.width, greaterThan(0), reason: '侧边栏要有宽度');
    // ★ 更要紧的是 **State 的 identity**：几何相同不代表没被重建。
    // "每次 build 现拼一个 NavigationRail" 时矩形一模一样，但它其实被重新挂载了
    // （这就是用户看到的"侧边栏闪一下 / 没有常驻"）。
    final railState = tester.state(find.byType(NavigationRail));

    await tester.tap(find.text('标签'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    expect(
      tester.getRect(find.byType(NavigationRail)),
      railRect,
      reason: '来回换页时侧边栏的位置/尺寸一个字都不该动',
    );
    expect(
      identical(tester.state(find.byType(NavigationRail)), railState),
      isTrue,
      reason: '侧边栏必须是**同一个** State —— 换页不许把它重建掉',
    );
    // 而且它必须真的在屏幕左边、铺满整个窗口高度（"常驻在最外层"）
    expect(railRect.left, 0, reason: '侧边栏贴着窗口左边');
    expect(
      railRect.height,
      tester.view.physicalSize.height / tester.view.devicePixelRatio,
      reason: '侧边栏铺满整个窗口高度（不是跟着内容一起换）',
    );
  });

  // ★ **后端重启时壳子不许被换掉**（用户 2026-10-11："让后端能够静默重启，
  // 不显示那个加载中界面，导致整个前端热重启；我要的桌面端侧边栏常驻呢？"）
  //
  // 老结构是 `StartupGate` 用 `return HomeShell()` / `return _StartupView()` **二选一**，
  // 所以 `isReady` 一变假，整个壳子（含侧边栏 + 五页状态）就被换成加载页。
  // 现在壳子是最高级、加载页只是覆盖物 —— 这条用例盯的就是"重启前后是同一个 State"。
  testWidgets('后端重启：侧边栏与五页状态原地不动，不再弹加载页', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final mock = _emptyBackend();
    final library = LibraryStore(
      settings,
      api: ApiClient(settings.baseUrl, client: mock),
    );
    await library.refreshAll();
    final ai = AiWorkspaceStore(
      api: AiApiClient(settings.baseUrl, client: mock),
    );
    final launcher = BackendLauncher(settings)..skip();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<LibraryStore>.value(value: library),
          ChangeNotifierProvider<AiWorkspaceStore>.value(value: ai),
          ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
        ],
        child: const MaterialApp(home: HomeShell()),
      ),
    );
    await tester.pumpAndSettle();

    // 先逛一圈，让"访问过的页"有状态可丢
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('预设'));
    await tester.pumpAndSettle();
    final railState = tester.state(find.byType(NavigationRail));
    final galleryState = tester.state(
      find.byType(GalleryPage, skipOffstage: false),
    );
    expect(find.byType(NavigationRail), findsOneWidget);

    // 模拟后端**重启**：isReady 变假（真去 kill 进程在单测里做不到）
    launcher.debugSetPhase(StartupPhase.starting, message: '正在重启本地服务…');
    await tester.pumpAndSettle();

    // ① 壳子还在、而且是**同一个 State**
    expect(
      find.byType(NavigationRail),
      findsOneWidget,
      reason: '后端重启不许把侧边栏换掉（老结构就是在这里换成加载页的）',
    );
    expect(
      identical(tester.state(find.byType(NavigationRail)), railState),
      isTrue,
      reason: '侧边栏必须是同一个 State —— 重建就是用户看到的"前端热重启"',
    );
    // ② 已经逛过的页也还在（状态没丢）
    expect(
      identical(
        tester.state(find.byType(GalleryPage, skipOffstage: false)),
        galleryState,
      ),
      isTrue,
      reason: '逛过的页面必须留着（换页/重启都不许销毁它）',
    );
    // ③ 不该出现启动页的那些文案
    expect(find.text('跳过，先进入界面'), findsNothing, reason: '重启时不该再弹冷启动的加载页');
    expect(find.text('重试'), findsNothing);

    // ④ 后端回来之后仍然一切照旧
    launcher.debugSetPhase(StartupPhase.ready, message: '已就绪');
    await tester.pumpAndSettle();
    expect(
      identical(tester.state(find.byType(NavigationRail)), railState),
      isTrue,
    );
    expect(find.text('跳过，先进入界面'), findsNothing);
  });
}
