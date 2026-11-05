// 标签页的回归：卡片宽度**最多 275**（= 通用列宽的 half），
// 长文案不会把固定高度的卡片撑破。
//
// 为什么是 275（用户 2026-10-12："标签页的卡片宽度过大，缩小为原来一半"）：
// 标签卡里只有一个名字 + 一行分类/说明 + 一个计数 + 两个按钮，按通用的 550 分列时
// 1440 窗口上每张 ≈656 宽 —— 名字只占左边一小截、右边全空着。回归见 `_cardWidth`。
//
// 卡片走的是固定高度的 GridView（mainAxisExtent），中文字号偏大 +
// 分类/说明很长时最容易"RenderFlex overflowed"，所以这里专门用长文案压一压。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/tags_page.dart';
import 'package:viewer/state/library_store.dart';

const _longCategory = '风格分类里最长的那个分类名字';
const _longDescription = '这条说明特别长，长到单行绝对放不下，用来验证卡片不会被文字撑破';

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;

    if (path == '/api/tags') {
      body = [
        {'id': 1, 'name': '赛博朋克', 'useCount': 12, 'color': '#9C27B0'},
        {
          'id': 2,
          'name': '电影感',
          'useCount': 7,
          'category': _longCategory,
          'description': _longDescription,
        },
        {'id': 3, 'name': '8K', 'useCount': 3},
        // 16 个英文字符：最小窗口（360）下必须完整显示
        {'id': 4, 'name': 'abcdefghijklmnop', 'useCount': 1},
      ];
    } else if (path == '/api/tags/categories') {
      body = [_longCategory];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': 0,
        'tags': 3,
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

    return http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

void main() {
  Future<void> pumpTags(WidgetTester tester, Size size) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
    addTearDown(store.dispose);
    await store.refreshAll();

    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: const MaterialApp(home: TagsPage()),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('标签页：宽窗口按最小卡宽分列，长文案不撑破', (tester) async {
    await pumpTags(tester, const Size(1600, 900));

    final cards = find.byType(Card);
    expect(cards, findsNWidgets(4), reason: '四个标签各一张卡');

    final r0 = tester.getRect(cards.at(0));
    final r1 = tester.getRect(cards.at(1));
    final r2 = tester.getRect(cards.at(2));
    final r3 = tester.getRect(cards.at(3));

    // 1600 - 左右各 16 的 padding = 1568 → 1568/328 = 4 列（上限也是 4）
    expect(r0.width, greaterThanOrEqualTo(kTagCardMinWidth),
        reason: '宽窗口的每一列都不能比 360 窗口下那张卡更窄');
    expect(r1.top, closeTo(r0.top, 0.5), reason: '前两张在同一行');
    expect(r1.left, greaterThan(r0.right - 1), reason: '第二张在第一张右边');
    expect(r2.top, closeTo(r0.top, 0.5), reason: '四列正好放四张，第三张也在同一行');
    expect(r3.top, closeTo(r0.top, 0.5), reason: '第四张同样在第一行');

    // 长文案被截断成一行（有 overflow: ellipsis），卡片高度保持一致
    expect(r1.height, closeTo(r0.height, 0.5));
    expect(find.textContaining(_longDescription), findsOneWidget);
  });

  testWidgets('标签页：360 宽下卡片就是这个下限，16 个英文字符完整显示', (tester) async {
    await pumpTags(tester, const Size(360, 800));

    final cards = find.byType(Card);
    final r0 = tester.getRect(cards.at(0));
    final r1 = tester.getRect(cards.at(1));

    // 360 - 左右各 16 的 padding = 328 —— 这就是 [kTagCardMinWidth]
    expect(r0.width, closeTo(kTagCardMinWidth, 1),
        reason: '最小窗口下一列满宽，且等于标签卡的最小宽度');
    expect(r1.top, greaterThan(r0.bottom + 1), reason: '一张一行');
    expect(find.textContaining(_longDescription), findsOneWidget);

    // 16 个英文字符的标签名要有足够的位置。
    //
    // ⚠ 这里不能断言 `RenderParagraph.didExceedMaxLines == false`：widget 测试
    // 用的是 Ahem 字体，每个字符都是 fontSize 宽（16 × 14.5 = 232px），
    // 真实中文字体 / 英文字体下同样的 16 个字符只有一半宽。所以按"标题拿到的
    // 可用宽度"来钉 —— 16 个英文字符按 8px/字符估算约 128，留到 140 即安全。
    final titleWidth = tester.getSize(find.text('abcdefghijklmnop')).width;
    expect(
      titleWidth,
      greaterThanOrEqualTo(140),
      reason: '360 宽下，标题至少要留出 16 个英文字符的位置',
    );
  });
}
