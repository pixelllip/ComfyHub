// 提示词库的回归测试：
//
//   1) 宽窗口按「宽度 / 550」排多列（以前是单列，右边空一大片）
//   2) 批量管理：进入多选 → 勾几条 → 一次收藏 / 加标签 / 删除

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/state/library_store.dart';

List<String> _log = [];

Map<String, dynamic> _prompt(int id, String title) => {
      'id': id,
      'title': title,
      'kind': 'IMAGE',
      'positivePrompt': 'prompt body $id',
      'favorite': false,
      'mediaCount': 0,
      'tags': <Object>[],
    };

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    _log.add('${request.method} $path');

    Object? body;
    var status = 200;
    if (path == '/api/prompts') {
      body = {
        'items': [_prompt(1, '雨夜霓虹'), _prompt(2, '黄昏海岸'), _prompt(3, '清晨薄雾')],
        'total': 3,
        'page': 1,
        'size': 24,
        'pages': 1,
      };
    } else if (RegExp(r'^/api/prompts/\d+$').hasMatch(path)) {
      body = _prompt(int.parse(path.split('/').last), '雨夜霓虹');
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 3,
        'media': 0,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': {'IMAGE': 3},
        'byMediaKind': <String, int>{},
      };
    } else if (path.endsWith('/favorite') || path.endsWith('/tags')) {
      body = _prompt(1, '雨夜霓虹');
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

Future<LibraryStore> _makeStore() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
  await store.refreshAll();
  return store;
}

Future<void> _pump(WidgetTester tester, LibraryStore store) async {
  tester.view.physicalSize = const Size(1600, 1100);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: const MaterialApp(home: PromptsPage()),
    ),
  );
  await tester.pumpAndSettle();
}

bool _called(String prefix) => _log.any((e) => e.startsWith(prefix));

void main() {
  setUp(() => _log = []);

  testWidgets('提示词页：1600 宽的窗口排成两列', (tester) async {
    final store = await _makeStore();
    await _pump(tester, store);

    final cards = find.byType(PromptCard);
    expect(cards, findsNWidgets(3));

    final r0 = tester.getRect(cards.at(0));
    final r1 = tester.getRect(cards.at(1));
    final r2 = tester.getRect(cards.at(2));
    expect(r1.top, closeTo(r0.top, 0.5), reason: '前两张应该在同一行');
    expect(r1.left, greaterThan(r0.right - 1));
    expect(r2.top, greaterThan(r0.bottom + 1), reason: '第三张换行');
    expect(r0.width, lessThan(900), reason: '一列不该被拉到整个窗口那么宽');
  });

  testWidgets('批量管理：勾两条后一次收藏', (tester) async {
    final store = await _makeStore();
    await _pump(tester, store);

    await tester.tap(find.byTooltip('批量管理'));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(PromptCard).at(0));
    await tester.tap(find.byType(PromptCard).at(1));
    await tester.pumpAndSettle();
    expect(find.text('已选 2 项'), findsOneWidget);

    await tester.tap(find.text('收藏'));
    await tester.pumpAndSettle();

    expect(_called('POST /api/prompts/1/favorite'), isTrue);
    expect(_called('POST /api/prompts/2/favorite'), isTrue);
    expect(_called('POST /api/prompts/3/favorite'), isFalse);
  });

  testWidgets('批量管理：批量加标签走的是追加标签接口', (tester) async {
    final store = await _makeStore();
    await _pump(tester, store);

    await tester.tap(find.byTooltip('批量管理'));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(PromptCard).at(0));
    await tester.pumpAndSettle();

    await tester.tap(find.text('加标签'));
    await tester.pumpAndSettle();
    expect(find.text('给 1 条提示词加标签'), findsOneWidget);

    await tester.enterText(find.byType(TextField).last, '赛博朋克');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    await tester.tap(find.text('添加标签'));
    await tester.pumpAndSettle();

    expect(_called('POST /api/prompts/1/tags'), isTrue);
  });

  testWidgets('批量管理：删除要先确认，确认后逐条 DELETE', (tester) async {
    final store = await _makeStore();
    await _pump(tester, store);

    await tester.tap(find.byTooltip('批量管理'));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(PromptCard).at(0));
    await tester.tap(find.byType(PromptCard).at(1));
    await tester.pumpAndSettle();

    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(find.text('删除 2 条提示词'), findsOneWidget);
    expect(_called('DELETE /api/prompts/1'), isFalse);

    await tester.tap(find.widgetWithText(FilledButton, '删除'));
    await tester.pumpAndSettle();

    expect(_called('DELETE /api/prompts/1'), isTrue);
    expect(_called('DELETE /api/prompts/2'), isTrue);
  });
}
