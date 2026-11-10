import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/state/library_store.dart';

/// 预设库的**同级页签**：
///  ① 普通预设 / 角色卡 / 画风卡在**同一条导航**上，不再是"卡片库子页"；
///  ② 卡片页签**第一次点开才拉数据**（别在进预设页时白拉两个端点）；
///  ③ 新建按钮跟着页签换；
///  ④ 点画风卡卡片本体 = **编辑**（不是改示例图片）。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Map<String, dynamic> prompt(int id, String title) => {
    'id': id,
    'title': title,
    'kind': 'IMAGE',
    'positivePrompt': 'prompt body $id',
    'favorite': false,
    'mediaCount': 0,
    'tags': <Object>[],
  };

  Map<String, dynamic> card({
    int id = 1,
    String name = 'shinku',
    String promptText = 'A quiet girl with red eyes.',
  }) => {
    'id': id,
    'name': name,
    'prompt': promptText,
    'examples': <Object>[],
    'updatedAt': '2026-10-01T00:00:00Z',
  };

  MockClient libraryClient() => MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;
    if (path == '/api/prompts') {
      body = {
        'items': [prompt(1, '雨夜霓虹'), prompt(2, '黄昏海岸')],
        'total': 2,
        'page': 1,
        'size': 24,
        'pages': 1,
      };
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 2,
        'media': 0,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': {'IMAGE': 2},
        'byMediaKind': <String, int>{},
      };
    } else {
      status = 404;
      body = {'error': 'not found: $path'};
    }
    return http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });

  Future<LibraryStore> makeStore() async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = LibraryStore(
      settings,
      api: ApiClient(settings.baseUrl, client: libraryClient()),
    );
    await store.refreshAll();
    return store;
  }

  Future<void> pump(
    WidgetTester tester,
    LibraryStore store,
    AiApiClient cardsApi, {
    Size size = const Size(1600, 1100),
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
        ],
        child: MaterialApp(home: PromptsPage(cardsApi: cardsApi)),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('三个页签同级；卡片第一次点开才拉、按钮跟着换、点卡片=编辑', (tester) async {
    final store = await makeStore();
    final cardCalls = <String>[];
    final cardsClient = MockClient((request) async {
      cardCalls.add(request.url.path);
      final body = request.url.path == '/api/ai/style-cards'
          ? [card(id: 2, name: '赛博国风', promptText: 'semi-thick line work')]
          : [card()];
      return http.Response(
        jsonEncode(body),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });
    final cardsApi = AiApiClient('http://127.0.0.1:8080', client: cardsClient);

    await pump(tester, store, cardsApi);

    // 三个同级页签都在；右上角「卡片库」入口已经移除
    expect(find.text('常规预设'), findsOneWidget);
    expect(find.text('角色卡'), findsOneWidget);
    expect(find.text('画风卡'), findsOneWidget);
    expect(find.byTooltip('卡片库（角色卡 / 画风卡）'), findsNothing);
    expect(cardCalls, isEmpty, reason: '还没点卡片页签，不该先拉两个卡片端点');
    expect(find.text('雨夜霓虹'), findsOneWidget, reason: '默认仍显示常规预设');
    expect(find.text('新建提示词'), findsOneWidget);

    // 角色卡页签
    await tester.tap(find.text('角色卡'));
    await tester.pumpAndSettle();
    expect(cardCalls, ['/api/ai/character-cards']);
    expect(find.text('shinku'), findsOneWidget);
    expect(find.text('新建角色卡'), findsOneWidget);
    expect(find.text('新建提示词'), findsNothing);

    // 画风卡页签
    await tester.tap(find.text('画风卡'));
    await tester.pumpAndSettle();
    expect(cardCalls, contains('/api/ai/style-cards'));
    expect(find.text('赛博国风'), findsOneWidget);
    expect(find.text('新建画风卡'), findsOneWidget);

    // 点卡片本体 = 编辑
    await tester.tap(find.text('赛博国风'));
    await tester.pumpAndSettle();
    expect(find.text('编辑画风卡'), findsOneWidget);
  });

  testWidgets('窄屏（360）上三个页签不溢出', (tester) async {
    final store = await makeStore();
    final cardsClient = MockClient((request) async {
      return http.Response(
        jsonEncode([card(name: '赛博国风')]),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });
    final cardsApi = AiApiClient('http://127.0.0.1:8080', client: cardsClient);

    await pump(tester, store, cardsApi, size: const Size(360, 800));

    await tester.tap(find.text('角色卡'));
    await tester.pumpAndSettle();
    expect(find.text('赛博国风'), findsOneWidget);
    // 溢出会抛 RenderFlex overflowed（widget 用例里未捕获异常 = 红）
  });
}
