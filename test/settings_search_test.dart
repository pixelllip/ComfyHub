// 设置搜索的回归：
//
//   1) 索引覆盖各子页的设置项，结果的标题是那一项、副标题是它的位置；
//   2) 从搜索结果点进目标页之后，返回**先回到搜索结果页**（查询词还在），
//      再返回才回设置首页。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/settings_page.dart';
import 'package:viewer/pages/settings_search_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    Object body;
    if (path == '/api/ai/nsfw') {
      body = {
        'enabled': false,
        'blur': false,
        'effectiveBlur': false,
        'promptVersion': 'test',
      };
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
    } else if (path == '/api/prompts' || path == '/api/media') {
      body = {
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 24,
        'pages': 0,
      };
    } else {
      body = <Object>[];
    }
    return http.Response(
      jsonEncode(body),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Widget _homeBuilder(String? sectionId) =>
    Scaffold(body: Center(child: Text('首页区块 $sectionId')));

void main() {
  test('设置搜索结果的标题是匹配项，副标题是所在位置', () {
    final entries = buildSettingsSearchIndex(
      isWindowsDesktop: true,
      homePageBuilder: _homeBuilder,
    );

    final auto = searchSettingsEntries(entries, '自动提交');
    expect(auto, isNotEmpty);
    expect(auto.first.title, '自动提交工作流次数');
    expect(auto.first.location, '设置 › AI 工具权限 › 预算');

    // 多个词要求全部命中，顺序不影响
    final multi = searchSettingsEntries(entries, '工作流 次数');
    expect(multi.map((e) => e.title), contains('自动提交工作流次数'));

    // 位置也参与匹配：搜页面名能找到该页里的项
    final budget = searchSettingsEntries(entries, '预算');
    expect(budget.map((e) => e.title), contains('工具轮数上限'));

    // 桌面端有本地服务 / 数据目录，手机端换成本机数据与同步
    expect(
      entries.map((e) => e.title),
      containsAll(<String>['本地服务（后端 + 数据库）', '数据目录']),
    );
    final mobile = buildSettingsSearchIndex(
      isWindowsDesktop: false,
      homePageBuilder: _homeBuilder,
    );
    expect(mobile.map((e) => e.title), contains('本机数据与同步'));
    expect(mobile.map((e) => e.title), isNot(contains('本地服务（后端 + 数据库）')));
  });

  testWidgets('从搜索结果进入目标页，返回时先回到搜索结果页', (tester) async {
    tester.view.physicalSize = const Size(1500, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final client = _mockClient();
    final library = LibraryStore(
      settings,
      api: ApiClient(settings.baseUrl, client: client),
    );
    final nsfw = NsfwStore(api: AiApiClient(settings.baseUrl, client: client));
    final launcher = BackendLauncher(settings)..skip();
    addTearDown(library.dispose);
    addTearDown(nsfw.dispose);
    addTearDown(launcher.dispose);
    await library.refreshAll();
    await nsfw.load(silent: true);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
          ChangeNotifierProvider<LibraryStore>.value(value: library),
          ChangeNotifierProvider<NsfwStore>.value(value: nsfw),
        ],
        child: const MaterialApp(home: SettingsPage()),
      ),
    );
    await tester.pumpAndSettle();

    // 设置页顶部的搜索入口
    expect(find.text('搜索设置'), findsOneWidget);
    await tester.tap(find.text('搜索设置'));
    await tester.pumpAndSettle();
    expect(find.byType(SettingsSearchPage), findsOneWidget);

    // 结果：标题是设置项，副标题是位置
    await tester.enterText(find.byType(TextField), '明暗模式');
    await tester.pumpAndSettle();
    final result = find.widgetWithText(ListTile, '明暗模式');
    expect(result, findsOneWidget);
    expect(find.text('设置 › 外观与布局 › 主题'), findsOneWidget);

    // 进目标页
    await tester.tap(result);
    await tester.pumpAndSettle();
    expect(find.text('系统主题色'), findsOneWidget);
    expect(find.byType(SettingsSearchPage), findsNothing);

    // 第一次返回：回到搜索结果页，查询词与位置都还在
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.byType(SettingsSearchPage), findsOneWidget);
    expect(find.widgetWithText(ListTile, '明暗模式'), findsOneWidget);
    expect(find.text('设置 › 外观与布局 › 主题'), findsOneWidget);

    // 第二次返回：回设置首页
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.byType(SettingsSearchPage), findsNothing);
    expect(find.text('搜索设置'), findsOneWidget);
  });
}
