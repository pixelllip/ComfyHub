import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/pages/cards_page.dart';
import 'package:viewer/pages/character_cards_page.dart';
import 'package:viewer/pages/style_cards_page.dart';

/// 卡片库（角色卡 / 画风卡）的回归。
///
/// 2026-10-21 起每张卡有**两版正面提示词**（Danbooru tag 版 + 自然语言版）与
/// 一组**示例产物**。这里盯五件事：
///  ① 两版内容都要画出来，缺哪版要**明说**；
///  ② 新建 / 编辑打哪条请求、payload 恰好四个键（全量替换）；
///  ③ 画风卡走自己的端点，且与角色卡共用同一套 UI；
///  ④ "管理示例"发的是 `PUT .../examples` + `{"mediaIds":[...]}`；
///  ⑤ 真机尺寸（384x853）下不溢出。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Map<String, dynamic> media({
    int id = 7,
    String title = '示例图',
    String kind = 'IMAGE',
  }) =>
      {
        'id': id,
        'kind': kind,
        'title': title,
        'originalName': 'sample.png',
        'storedName': 'sample.png',
        'sizeBytes': 1024,
        'nsfw': false,
        'fileUrl': '/api/media/$id/file',
        'thumbUrl': '/api/media/$id/thumb',
      };

  Map<String, dynamic> card({
    int id = 1,
    String name = 'shinku',
    String? prompt = 'A quiet girl with red eyes and silver hair.',
    String? negative,
    List<Map<String, dynamic>>? examples,
  }) => {
    'id': id,
    'name': name,
    'prompt': ?prompt,
    'negative': ?negative,
    'examples': examples ?? const [],
    'updatedAt': '2026-10-01T00:00:00Z',
  };

  /// 假后端：记下每次请求，按路径回固定数据。
  ///
  /// [mediaItems] 非空时，`GET /api/media` 回它们（示例选择器用）。
  ({MockClient client, List<String> calls, List<Map<String, dynamic>> bodies})
  backend(
    List<Map<String, dynamic>> cards, {
    List<Map<String, dynamic>> mediaItems = const [],
    String cardsPath = '/api/ai/character-cards',
  }) {
    final calls = <String>[];
    final bodies = <Map<String, dynamic>>[];
    final client = MockClient((req) async {
      calls.add('${req.method} ${req.url.path}${req.url.query.isEmpty ? '' : '?${req.url.query}'}');
      if (req.method != 'GET') {
        bodies.add(jsonDecode(req.body) as Map<String, dynamic>);
      }
      Object body;
      if (req.url.path == '/api/media') {
        body = {
          'items': mediaItems,
          'total': mediaItems.length,
          'page': 1,
          'size': 60,
          'pages': 1,
        };
      } else if (req.method == 'GET') {
        body = cards;
      } else if (req.method == 'POST') {
        body = {...(jsonDecode(req.body) as Map<String, dynamic>), 'id': 9, 'examples': []};
      } else if (req.method == 'PUT') {
        final sent = jsonDecode(req.body) as Map<String, dynamic>;
        // 示例端点回整张卡；普通编辑回改过的卡
        body = req.url.path.endsWith('/examples')
            ? {
                ...card(),
                'examples': (sent['mediaIds'] as List)
                    .map((id) => media(id: (id as num).toInt()))
                    .toList(),
              }
            : {...sent, 'id': 1, 'examples': []};
      } else {
        body = {'deleted': 1};
      }
      return http.Response(jsonEncode(body), req.method == 'POST' ? 201 : 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });
    return (client: client, calls: calls, bodies: bodies);
  }

  Future<void> pumpPage(WidgetTester tester, Widget page) async {
    await tester.pumpWidget(MaterialApp(home: page));
    await tester.pumpAndSettle();
  }

  testWidgets('列表画出正面提示词 空着就明说', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([
      card(negative: 'bad hands'),
      // 第二张刻意没有正文：两张卡必须各画各的，而且"还没有正面提示词"只该出现一次
      card(id: 2, name: 'iroi', prompt: null),
    ]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    expect(b.calls, ['GET /api/ai/character-cards'], reason: '除了列表不该再问别的端点');
    expect(find.text('shinku'), findsOneWidget);
    expect(find.text('iroi'), findsOneWidget);
    expect(find.text('A quiet girl with red eyes and silver hair.'), findsOneWidget);
    expect(find.text('负面：bad hands'), findsOneWidget);
    expect(find.textContaining('还没有正面提示词'), findsOneWidget, reason: '空着是个事实，要显式画出来');
  });

  testWidgets('点卡片本体 = 编辑（不是改示例图）', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    await tester.tap(find.text('shinku'));
    await tester.pumpAndSettle();
    expect(find.text('编辑角色卡'), findsOneWidget);
    expect(find.text('选择示例产物'), findsNothing);
  });

  testWidgets('搜索把关键字发给后端', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    await tester.enterText(find.byType(TextField).first, '真红');
    await tester.tap(find.widgetWithText(FilledButton, '搜索'));
    await tester.pumpAndSettle();

    expect(
      b.calls.where((c) => c.contains('q=')).single,
      'GET /api/ai/character-cards?q=%E7%9C%9F%E7%BA%A2',
    );
  });

  testWidgets('新建发 POST 编辑发 PUT 且 payload 恰好三个字段', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    // 编辑：打开菜单 → 编辑 → 改正文 → 保存
    await tester.tap(find.byTooltip('更多操作').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    expect(find.text('编辑角色卡'), findsOneWidget);

    await tester.enterText(
      find.widgetWithText(TextField, '正面提示词'),
      'A girl with blue eyes, every detail from head to toe.',
    );
    await tester.enterText(find.widgetWithText(TextField, '负面提示词'), 'bad hands');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();

    expect(b.calls.where((c) => c.startsWith('PUT')), isNotEmpty);
    final sent = b.bodies.last;
    expect(sent['name'], 'shinku');
    expect(sent['prompt'], 'A girl with blue eyes, every detail from head to toe.');
    expect(sent['negative'], 'bad hands');
    // ★ 全量替换的形状：**恰好**这三个键（tag 版已经彻底删掉）
    expect(sent.keys.toList()..sort(), ['name', 'negative', 'prompt']);
  });

  testWidgets('清空正文要发 null 而不是漏掉这个键', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    await tester.tap(find.byTooltip('更多操作').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();

    await tester.enterText(find.widgetWithText(TextField, '正面提示词'), '');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();

    final sent = b.bodies.last;
    expect(sent.containsKey('prompt'), isTrue, reason: '漏掉这个键 = 清空永远不生效');
    expect(sent['prompt'], isNull);

    // 新建：FAB → 填名字 → 保存
    await tester.tap(find.text('新建角色卡'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, '角色名 *'), 'iroi');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();
    expect(b.calls.where((c) => c.startsWith('POST')), isNotEmpty);
    expect(b.bodies.last['name'], 'iroi');
    expect(b.bodies.last['prompt'], isNull);
    expect(b.bodies.last.keys.toList()..sort(), ['name', 'negative', 'prompt']);
  });

  testWidgets('画风卡走自己的端点 并与角色卡共用 UI', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend(
      [card(name: '赛博国风', prompt: 'semi-thick line work')],
      cardsPath: '/api/ai/style-cards',
    );
    await pumpPage(
      tester,
      StyleCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    expect(find.text('画风卡'), findsOneWidget);
    expect(b.calls, ['GET /api/ai/style-cards']);
    expect(find.text('赛博国风'), findsOneWidget);
    expect(find.text('semi-thick line work'), findsOneWidget);

    // 新建也打 style-cards
    await tester.tap(find.text('新建画风卡'));
    await tester.pumpAndSettle();
    // FAB 与弹窗标题同名：出现两个才说明走的是同一套编辑器
    expect(find.text('新建画风卡'), findsNWidgets(2));
    await tester.enterText(find.widgetWithText(TextField, '画风名 *'), '厚涂');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();
    expect(b.calls.where((c) => c.startsWith('POST')).single, 'POST /api/ai/style-cards');
  });

  testWidgets('管理示例把最终 mediaIds 发给 examples 端点', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend(
      [card(name: '赛博国风', examples: [])],
      mediaItems: [media(id: 7, title: '示例图')],
      cardsPath: '/api/ai/style-cards',
    );
    await pumpPage(
      tester,
      StyleCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );

    await tester.tap(find.text('添加示例'));
    await tester.pumpAndSettle();
    expect(find.text('选择示例产物'), findsOneWidget);

    await tester.tap(find.text('示例图'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, '保存（1）'));
    await tester.pumpAndSettle();

    final put = b.calls.where((c) => c.startsWith('PUT')).single;
    expect(put, 'PUT /api/ai/style-cards/1/examples');
    expect(b.bodies.last, {
      'mediaIds': [7],
    });
  });

  testWidgets('卡片库顶部可在角色卡与画风卡之间切换', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final calls = <String>[];
    final client = MockClient((req) async {
      calls.add(req.url.path);
      final body = req.url.path == '/api/ai/style-cards'
          ? [card(name: '赛博国风', prompt: 'semi-thick')]
          : [card()];
      return http.Response(jsonEncode(body), 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });
    await pumpPage(
      tester,
      CardsPage(api: AiApiClient('http://127.0.0.1:8080', client: client)),
    );
    expect(find.text('shinku'), findsOneWidget);

    await tester.tap(find.text('画风卡').first);
    await tester.pumpAndSettle();
    expect(calls, contains('/api/ai/style-cards'));
    expect(find.text('赛博国风'), findsOneWidget);
  });

  testWidgets('真机尺寸（384x853）下正文与示例都不溢出', (tester) async {
    tester.view.physicalSize = const Size(384 * 2.8125, 853 * 2.8125);
    tester.view.devicePixelRatio = 2.8125;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    SharedPreferences.setMockInitialValues({});
    final b = backend([
      card(
        prompt: List.generate(120, (i) => 'detail_$i').join(' '),
        examples: [media(id: 1), media(id: 2), media(id: 3)],
      ),
    ]);
    await pumpPage(
      tester,
      CharacterCardsPage(api: AiApiClient('http://127.0.0.1:8080', client: b.client)),
    );
    expect(find.text('shinku'), findsOneWidget);
    // 弹窗在窄屏也要能开、能滚（AlertDialog 的 width 560 会被屏幕夹住）
    await tester.tap(find.text('新建角色卡'));
    await tester.pumpAndSettle();
    expect(find.text('角色名 *'), findsOneWidget);
    // 溢出会抛 RenderFlex overflowed（widget 用例里未捕获异常 = 红）
  });
}
