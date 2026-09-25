import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/pages/character_cards_page.dart';

/// 角色卡页面（O8）的回归。
///
/// 盯三件事：
///  ① 卡里的**事实**要画出来：tag 栈、LoRA 与权重、别名、来源，以及
///     「**这些 tag 没跟 danbooru 核对过**」那一行 —— 核对时间是后端的 danbooru
///     工具写的事实，界面上必须与"已核对"分得开（不然用户没法判断卡可不可信）；
///  ② 新建 / 编辑打的是哪条请求、**payload 里带没带 tagsVerifiedAt**
///     （编辑是全量替换，漏了它就会把 AI 查出来的核对时间悄悄抹掉）；
///  ③ 真机尺寸（384x853）下不溢出。
///
/// 假后端只回固定形状，**不出网**。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Map<String, dynamic> card({
    int id = 1,
    String name = 'shinku',
    List<String> tags = const ['1girl', 'red_eyes'],
    String? verifiedAt,
  }) =>
      {
        'id': id,
        'name': name,
        'nameEn': 'Shinku',
        'aliases': ['真红'],
        'characterTag': 'shinku_(nte)',
        'seriesTag': 'neverness_to_everness',
        'tags': tags,
        'loraName': 'NTE(Anima).safetensors',
        'loraWeight': 0.7,
        'source': 'danbooru',
        'tagsVerifiedAt': ?verifiedAt,
      };

  /// 假后端：记下每次请求，按路径回固定数据。
  ({MockClient client, List<String> calls, List<Map<String, dynamic>> bodies}) backend(
      List<Map<String, dynamic>> cards) {
    final calls = <String>[];
    final bodies = <Map<String, dynamic>>[];
    final client = MockClient((req) async {
      calls.add('${req.method} ${req.url.path}${req.url.query.isEmpty ? '' : '?${req.url.query}'}');
      if (req.method != 'GET') {
        bodies.add(jsonDecode(req.body) as Map<String, dynamic>);
      }
      Object body;
      if (req.method == 'GET') {
        body = cards;
      } else if (req.method == 'POST') {
        body = {...(jsonDecode(req.body) as Map<String, dynamic>), 'id': 9};
      } else if (req.method == 'PUT') {
        body = {...(jsonDecode(req.body) as Map<String, dynamic>), 'id': 1};
      } else {
        body = {'deleted': 1};
      }
      return http.Response(jsonEncode(body), req.method == 'POST' ? 201 : 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });
    return (client: client, calls: calls, bodies: bodies);
  }

  Future<void> pumpPage(WidgetTester tester, AiApiClient api) async {
    await tester.pumpWidget(MaterialApp(home: CharacterCardsPage(api: api)));
    await tester.pumpAndSettle();
  }

  testWidgets('列表画出卡里的事实 没核对过要明说', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([
      card(),
      // 第二张刻意换成另一个角色：两张卡的摘要必须各画各的（不许把第一张的内容复用出来）
      {
        ...card(id: 2, verifiedAt: '2026-09-26T00:00:00Z'),
        'name': 'iroi',
        'nameEn': 'Iroi',
        'characterTag': 'iroi_(nte)',
        'aliases': <String>[],
        'seriesTag': null,
        'loraName': null,
        'loraWeight': null,
        'tags': ['green_hair'],
      },
    ]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

    expect(b.calls.single, 'GET /api/ai/character-cards');
    expect(find.text('shinku（Shinku）'), findsOneWidget);
    expect(find.text('iroi（Iroi）'), findsOneWidget);
    expect(find.text('shinku_(nte) · neverness_to_everness · 别名 真红 · LoRA NTE(Anima).safetensors @0.7'),
        findsOneWidget);
    expect(find.text('red_eyes'), findsOneWidget);
    // 第一张没核对过、第二张核对过 → 那行**只该出现一次**（这是"事实要画出来"的判据）
    expect(find.text('这些 tag 没跟 danbooru 核对过'), findsOneWidget,
        reason: '核对过的那张卡不该画这行');
  });

  testWidgets('搜索把关键字发给后端', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

    await tester.enterText(find.byType(TextField).first, '真红');
    await tester.tap(find.widgetWithText(FilledButton, '搜索'));
    await tester.pumpAndSettle();

    expect(b.calls.last, 'GET /api/ai/character-cards?q=%E7%9C%9F%E7%BA%A2');
  });

  testWidgets('新建发 POST 编辑发 PUT 且原样带回核对时间', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card(verifiedAt: '2026-09-26T00:00:00Z')]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

    // 编辑：打开菜单 → 编辑
    await tester.tap(find.byTooltip('更多操作').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    expect(find.text('编辑角色卡'), findsOneWidget);

    await tester.enterText(find.widgetWithText(TextField, '规范 tag 栈'), '1girl, horns');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();

    expect(b.calls.where((c) => c.startsWith('PUT')), isNotEmpty);
    final sent = b.bodies.last;
    expect(sent['tags'], ['1girl', 'horns']);
    expect(sent['name'], 'shinku');
    // ★ 编辑是全量替换：核对时间不在弹窗里，但必须原样带回去（漏了就把 AI 查的事实抹了）
    expect(sent['tagsVerifiedAt'], '2026-09-26T00:00:00Z');

    // 新建：FAB → 填名字 → 保存
    await tester.tap(find.text('新建角色卡'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, '角色名 *'), 'iroi');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();
    expect(b.calls.where((c) => c.startsWith('POST')), isNotEmpty);
    expect(b.bodies.last['name'], 'iroi');
    expect(b.bodies.last['tagsVerifiedAt'], isNull, reason: '新建时没有核对时间，不许编一个');
  });

  testWidgets('真机尺寸（384x853）下不溢出', (tester) async {
    tester.view.physicalSize = const Size(384 * 2.8125, 853 * 2.8125);
    tester.view.devicePixelRatio = 2.8125;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    SharedPreferences.setMockInitialValues({});
    final b = backend([card(tags: List.generate(30, (i) => 'tag_$i'))]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));
    expect(find.text('shinku（Shinku）'), findsOneWidget);
    // 溢出会抛 RenderFlex overflowed（widget 用例里未捕获异常 = 红）
  });
}
