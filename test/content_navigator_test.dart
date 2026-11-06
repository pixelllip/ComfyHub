//! 次级页面（产物详情 / 预设详情…）**只替换右边**，不许盖住侧边栏。
//!
//! 用户 2026-10-11 两条连着的反馈：
//!   ① "桌面端 UI 应该始终把主界面侧边栏置顶，更换页面的时候把右边界面换掉即可"；
//!   ② 看到产物详情页之后："侧边栏哪去了？次级菜单还是没有只替换右边的内容？"
//!
//! 根因：详情页走 `Navigator.of(context).push(...)`，那是 `MaterialApp` 的**根**
//! Navigator，路由铺满整个窗口。修法是每个主页面各套一个内容区 Navigator
//! （`lib/app.dart` 的 `_ContentNavigator`）—— 这组用例盯的就是它。

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
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';

Map<String, dynamic> _mediaJson() => {
  'id': 1,
  'kind': 'IMAGE',
  'title': '海边回眸',
  'originalName': 'test_00350_.png',
  'storedName': 'test_00350_.png',
  'sizeBytes': 100,
  'width': 1024,
  'height': 1024,
  'favorite': false,
  'nsfw': false,
  'hasWorkflow': false,
  'fileUrl': '/api/media/1/file',
  'thumbUrl': '/api/media/1/thumb',
  'collectionSize': 1,
};

MockClient _backend() => MockClient((request) async {
  final path = request.url.path;
  Object body;
  if (path.startsWith('/api/ai/')) {
    body = <Object>[];
  } else if (path == '/api/media/1') {
    body = _mediaJson();
  } else if (path == '/api/media') {
    body = {
      'items': [_mediaJson()],
      'total': 1,
      'page': 1,
      'size': 24,
      'pages': 1,
    };
  } else if (path == '/api/tags' || path == '/api/tags/categories') {
    body = <Object>[];
  } else if (path == '/api/stats') {
    body = {
      'prompts': 0,
      'media': 1,
      'tags': 0,
      'favoritePrompts': 0,
      'favoriteMedia': 0,
      'byKind': <String, int>{},
      'byMediaKind': <String, int>{'IMAGE': 1},
    };
  } else {
    body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
  }
  return http.Response(
    jsonEncode(body),
    200,
    headers: {'content-type': 'application/json; charset=utf-8'},
  );
});

Future<Widget> _shell() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final mock = _backend();
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
      ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
      ChangeNotifierProvider<BackendLauncher>(
        create: (_) => BackendLauncher(settings)..skip(),
      ),
    ],
    child: const MaterialApp(home: HomeShell()),
  );
}

void main() {
  testWidgets('产物详情压在内容区里：侧边栏仍在、位置不变', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _shell());
    await tester.pumpAndSettle();

    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    final railRect = tester.getRect(find.byType(NavigationRail));
    final railState = tester.state(find.byType(NavigationRail));

    // 从**画廊这一格内部**的 context 压详情页 —— 正是页面代码里
    // `Navigator.of(context).push(...)` 走的同一条路。
    final galleryContext = tester.element(find.byType(GalleryPage));
    Navigator.of(galleryContext).push(
      MaterialPageRoute<void>(
        builder: (_) => const MediaDetailPage(mediaId: 1),
      ),
    );
    await tester.pumpAndSettle();

    // ① 详情页出来了
    expect(find.byType(MediaDetailPage), findsOneWidget);
    // ② ★ 侧边栏**还在**，而且几何与 State 都没动
    expect(
      find.byType(NavigationRail),
      findsOneWidget,
      reason: '次级页面不许盖住侧边栏（用户："侧边栏哪去了？"）',
    );
    expect(
      tester.getRect(find.byType(NavigationRail)),
      railRect,
      reason: '侧边栏位置/尺寸一个字都不该动',
    );
    expect(
      identical(tester.state(find.byType(NavigationRail)), railState),
      isTrue,
      reason: '侧边栏还是同一个 State',
    );
    // ③ 详情页只占**右边**（左边缘在侧边栏右边）
    final detail = tester.getRect(find.byType(MediaDetailPage));
    expect(
      detail.left,
      greaterThanOrEqualTo(railRect.right),
      reason: '详情页的左边从侧边栏右边开始，不是从窗口左边',
    );

    // ④ 返回之后画廊还在（没被销毁）
    Navigator.of(tester.element(find.byType(MediaDetailPage))).pop();
    await tester.pumpAndSettle();
    expect(find.byType(GalleryPage), findsOneWidget);
    expect(find.byType(MediaDetailPage), findsNothing);
    expect(find.byType(NavigationRail), findsOneWidget);
  });

  testWidgets('每页各有独立路由栈：切走再切回来，那一页还停在详情', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _shell());
    await tester.pumpAndSettle();
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();

    Navigator.of(tester.element(find.byType(GalleryPage))).push(
      MaterialPageRoute<void>(
        builder: (_) => const MediaDetailPage(mediaId: 1),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(MediaDetailPage), findsOneWidget);

    // 切去预设：画廊那一格被藏起来，但**它的路由栈还在**
    await tester.tap(find.text('预设'));
    await tester.pumpAndSettle();
    expect(find.byType(PromptsPage), findsOneWidget);
    expect(find.byType(MediaDetailPage), findsNothing, reason: '别的页不该看到画廊的详情页');

    // 切回画廊：还停在详情页（用户没按返回）
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    expect(
      find.byType(MediaDetailPage),
      findsOneWidget,
      reason: '每页路由栈独立 —— 切回来该还在原来那一层',
    );
    expect(find.byType(NavigationRail), findsOneWidget, reason: '侧边栏始终在');
  });

  // ---------------------------------------------------------------------------
  //  窄屏（移动端形态）：次级页要**铺满整个界面**（用户 2026-10-12）
  //
  //  "移动端 UI 中，底边栏不是像桌面端 UI 那样侧边栏常驻的，
  //   次级菜单应该覆盖全部界面。"
  //
  // 桌面端把次级页关在内容区（上一组用例），移动端相反：手机上"进详情"就该整屏换页，
  // 底下还挂着一条底部导航栏会让人以为"还有一半界面能用"。
  // ---------------------------------------------------------------------------

  testWidgets('窄屏：次级页落在根 Navigator 上，铺满整屏（底边栏被盖住）', (tester) async {
    tester.view.physicalSize = const Size(400, 800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(await _shell());
    await tester.pumpAndSettle();

    // 窄屏是底部导航形态
    expect(find.byType(NavigationBar), findsOneWidget);
    expect(find.byType(NavigationRail), findsNothing);

    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();

    Navigator.of(tester.element(find.byType(GalleryPage))).push(
      MaterialPageRoute<void>(
        builder: (_) => const MediaDetailPage(mediaId: 1),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byType(MediaDetailPage), findsOneWidget);
    // ★ 整屏：从 (0,0) 起、占满窗口 —— 而不是"从底边栏上面开始"
    final detail = tester.getRect(find.byType(MediaDetailPage));
    expect(detail.left, 0, reason: '窄屏次级页铺满整屏（左边不再是侧边栏右边）');
    expect(detail.top, 0);
    expect(detail.width, 400);
    expect(detail.height, 800, reason: '底边栏也在它下面被盖住 —— 手机上"进详情"就是整屏换页');

    Navigator.of(tester.element(find.byType(MediaDetailPage))).pop();
    await tester.pumpAndSettle();
    expect(find.byType(MediaDetailPage), findsNothing);
    expect(find.byType(GalleryPage), findsOneWidget, reason: '返回后画廊还在');
    expect(find.byType(NavigationBar), findsOneWidget);
  });
}
