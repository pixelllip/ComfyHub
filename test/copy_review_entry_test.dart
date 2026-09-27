// 设置页 →「AI 文案台」的入口（任务书第 4 条：只加这一处）。
//
// 为什么值得单独一条用例：文案台自己的用例全都**直接构造** `CopyReviewPage`，
// 绕过了设置页那一跳 —— 入口被顺手删掉、或指向别的页面，没有任何别的防线。

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
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';

/// 设置页要的那些端点（与 `settings_layout_test.dart` 同一份最小假后端）。
MockClient _mockClient() => MockClient((request) async {
      final path = request.url.path;
      Object? body;
      var status = 200;

      if (path == '/api/capture/config') {
        body = {
          'enabled': true,
          'comfyUrl': 'http://127.0.0.1:8188',
          'pollSeconds': 4,
          'autoTag': 'ComfyUI',
          'maxPerPoll': 20,
          'downloadFallback': true,
        };
      } else if (path == '/api/capture/status') {
        body = {
          'enabled': true,
          'comfyUrl': 'http://127.0.0.1:8188',
          'comfyReachable': true,
          'queueRunning': 0,
          'queuePending': 0,
          'capturedRuns': 0,
          'capturedMedia': 0,
          'recent': <Object>[],
        };
      } else if (path == '/api/ai/nsfw') {
        body = {'enabled': false, 'blur': false, 'effectiveBlur': false, 'promptVersion': 'nsfw-v1'};
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
        body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
      } else {
        status = 404;
        body = {'error': 'not found: $path'};
      }

      return http.Response(jsonEncode(body), status,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });

void main() {
  testWidgets('设置页有「AI 文案台」入口，点进去就是文案台', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final client = _mockClient();
    final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: client));
    final launcher = BackendLauncher(settings);
    final nsfw = NsfwStore(api: AiApiClient(settings.baseUrl, client: client));
    addTearDown(store.dispose);
    addTearDown(launcher.dispose);
    addTearDown(nsfw.dispose);
    await store.refreshAll();
    await nsfw.load(silent: true);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<NsfwStore>.value(value: nsfw),
        ],
        child: const MaterialApp(home: SettingsPage()),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('AI 文案台'), findsOneWidget, reason: '设置页要有这一张卡');

    await tester.tap(find.text('人工校验 / 批量修改 AI 文案'));
    await tester.pumpAndSettle();

    // 推上去的是文案台：标题在它自己的 AppBar 上（设置页那张卡的标题还留在树里，
    // 所以这里不能只按文字找）。
    expect(find.widgetWithText(AppBar, 'AI 文案台'), findsOneWidget);
    expect(find.byTooltip('导出 / 导入'), findsOneWidget);
    // 文案台自己建的客户端在用例里打不到真后端（flutter_test 让真实请求一律失败），
    // 页面必须**如实**说出"没读到清单"，而不是假装清单是空的。
    expect(find.textContaining('还没读到清单'), findsOneWidget);
    expect(find.textContaining('没有匹配的文案'), findsNothing);
  });
}
