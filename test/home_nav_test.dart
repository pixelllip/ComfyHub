// 首页落地页的回归测试：
//
//   需求是「打开 App 就落在画廊（产物 / 图片）页」，导航顺序 画廊 → 提示词 → 标签 → 设置。
//   这里不依赖真实后端（用 MockClient 返回空列表），断言的是 lib/app.dart 里
//   _destinations 与 pages 两个列表的顺序和默认选中项。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/app.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/state/library_store.dart';

MockClient _emptyBackend() {
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path == '/api/tags' || path == '/api/tags/categories') {
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

Future<LibraryStore> _makeStore() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _emptyBackend()));
  await store.refreshAll();
  return store;
}

void main() {
  testWidgets('App 默认落在画廊页，导航顺序是 画廊 → 提示词 → 标签 → 设置', (tester) async {
    // 宽屏（>=900）才会走 NavigationRail 布局，和真实桌面端一致
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = await _makeStore();
    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: const MaterialApp(home: HomeShell()),
      ),
    );
    await tester.pumpAndSettle();

    // 1) 首屏就是画廊，不是提示词库
    expect(find.byType(GalleryPage), findsOneWidget);
    expect(find.byType(PromptsPage), findsNothing);

    // 2) 导航：默认选中第 0 个，且顺序是 画廊 → 提示词 → 标签 → 设置
    final rail = tester.widget<NavigationRail>(find.byType(NavigationRail));
    expect(rail.selectedIndex, 0);
    final labels = rail.destinations
        .map((d) => (d.label as Text).data)
        .toList();
    expect(labels, ['画廊', '提示词', '标签', '设置']);

    // 3) 点第二个才进提示词库
    await tester.tap(find.text('提示词'));
    await tester.pumpAndSettle();
    expect(find.byType(PromptsPage), findsOneWidget);
    expect(find.byType(GalleryPage), findsNothing);
  });
}
