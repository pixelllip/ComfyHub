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
      body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
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
  final library = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: mock));
  await library.refreshAll();
  final ai = AiWorkspaceStore(api: AiApiClient(settings.baseUrl, client: mock));

  return MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<LibraryStore>.value(value: library),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: ai),
    ],
    child: const MaterialApp(home: HomeShell()),
  );
}

void main() {
  testWidgets('App 默认落在 AI 工作台，导航顺序是 AI 工作台 → 画廊 → 提示词 → 标签 → 设置', (tester) async {
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
    expect(labels, ['AI 工作台', '画廊', '提示词', '标签', '设置']);

    // 3) 点第二个才进画廊
    await tester.tap(find.text('画廊'));
    await tester.pumpAndSettle();
    expect(find.byType(GalleryPage), findsOneWidget);
    expect(find.byType(AiHomePage), findsNothing);

    // 4) 第三个是提示词库
    await tester.tap(find.text('提示词'));
    await tester.pumpAndSettle();
    expect(find.byType(PromptsPage), findsOneWidget);
  });
}
