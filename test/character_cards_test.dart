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
/// 2026-10 起一张卡**只有三个字段**：名称 / 正面提示词 / 负面提示词。这里盯三件事：
///  ① 卡里的内容要画出来，**没有正面提示词的那张要明说**（用户得一眼看出哪张还没写完）；
///  ② 新建 / 编辑打的是哪条请求、**payload 里恰好只有这三个键**
///     （编辑是全量替换：多带一个后端不认识的键就是形状漂了，少带一个就清不掉内容）；
///  ③ 真机尺寸（384x853）下不溢出。
///
/// 假后端只回固定形状，**不出网**。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Map<String, dynamic> card({
    int id = 1,
    String name = 'shinku',
    String? prompt = 'shinku_(nte), red_eyes',
    String? negative,
  }) =>
      {
        'id': id,
        'name': name,
        'prompt': ?prompt,
        'negative': ?negative,
        'updatedAt': '2026-10-01T00:00:00Z',
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

  testWidgets('列表画出名称与提示词 没写提示词的明说没有', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([
      card(negative: 'bad hands'),
      // 第二张刻意只有名字：两张卡必须各画各的，而且"还没有正面提示词"只该出现一次
      card(id: 2, name: 'iroi', prompt: null),
    ]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

    expect(b.calls, ['GET /api/ai/character-cards'], reason: '除了列表不该再问别的端点');
    expect(find.text('shinku'), findsOneWidget);
    expect(find.text('iroi'), findsOneWidget);
    expect(find.text('shinku_(nte), red_eyes'), findsOneWidget);
    expect(find.text('负面：bad hands'), findsOneWidget);
    expect(find.text('还没有正面提示词'), findsOneWidget, reason: '空提示词是个事实，要显式画出来');
  });

  testWidgets('搜索把关键字发给后端', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

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
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

    // 编辑：打开菜单 → 编辑 → 改正面提示词 → 保存
    await tester.tap(find.byTooltip('更多操作').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    expect(find.text('编辑角色卡'), findsOneWidget);

    await tester.enterText(find.widgetWithText(TextField, '正面提示词'), 'shinku_(nte), blue eyes');
    await tester.enterText(find.widgetWithText(TextField, '负面提示词'), 'bad hands');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();

    expect(b.calls.where((c) => c.startsWith('PUT')), isNotEmpty);
    final sent = b.bodies.last;
    expect(sent['name'], 'shinku');
    expect(sent['prompt'], 'shinku_(nte), blue eyes');
    expect(sent['negative'], 'bad hands');
    // ★ 全量替换的形状：**恰好**这三个键（多一个是形状漂了，少一个就清不掉内容）
    expect(sent.keys.toList()..sort(), ['name', 'negative', 'prompt']);
  });

  testWidgets('清空提示词要发 null 而不是漏掉这个键', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final b = backend([card()]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));

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
    expect(b.bodies.last['prompt'], isNull, reason: '没填就是 null，不许编一个');
  });

  testWidgets('真机尺寸（384x853）下不溢出', (tester) async {
    tester.view.physicalSize = const Size(384 * 2.8125, 853 * 2.8125);
    tester.view.devicePixelRatio = 2.8125;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    SharedPreferences.setMockInitialValues({});
    final b = backend([card(prompt: List.generate(40, (i) => 'tag_$i').join(', '))]);
    await pumpPage(tester, AiApiClient('http://127.0.0.1:8080', client: b.client));
    expect(find.text('shinku'), findsOneWidget);
    // 溢出会抛 RenderFlex overflowed（widget 用例里未捕获异常 = 红）
  });
}
