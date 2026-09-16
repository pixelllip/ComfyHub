// 「产物删除后提示词标未关联 + 一键清除」的回归（用户要求）：
//
//   1) 没有关联产物的提示词，卡片上要有显眼的「未关联」标记；
//   2) 提示词库有「未关联产物」筛选，走的是后端的 hasMedia=0；
//   3) 一键清除只删未关联的那些，**有线关联的一条都不能动**；
//   4) 未关联的提示词超过一页时也要全部删掉（边删边翻页是最容易漏的写法）。

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

/// 假后端：维护一份提示词表，[mediaCount] 为 0 即"未关联产物"。
class _FakeLibrary {
  _FakeLibrary(this.prompts);

  final List<Map<String, dynamic>> prompts;
  final List<String> requests = [];
  final List<int> deleted = [];

  MockClient client() => MockClient((request) async {
        final path = request.url.path;
        final params = request.url.queryParameters;
        requests.add('${request.method} $path'
            '${request.url.hasQuery ? '?${request.url.query}' : ''}');

        Object? body;
        var status = 200;

        if (path == '/api/prompts') {
          final hasMedia = params['hasMedia'];
          var items = prompts;
          if (hasMedia == '0') {
            items = prompts.where((p) => (p['mediaCount'] as int) == 0).toList();
          } else if (hasMedia == '1') {
            items = prompts.where((p) => (p['mediaCount'] as int) > 0).toList();
          }
          final size = int.tryParse(params['size'] ?? '24') ?? 24;
          final page = int.tryParse(params['page'] ?? '1') ?? 1;
          final start = (page - 1) * size;
          final slice = start >= items.length
              ? const <Map<String, dynamic>>[]
              : items.sublist(start, (start + size).clamp(0, items.length));
          body = {
            'items': slice,
            'total': items.length,
            'page': page,
            'size': size,
            'pages': items.isEmpty ? 0 : ((items.length + size - 1) ~/ size),
          };
        } else if (RegExp(r'^/api/prompts/\d+$').hasMatch(path)) {
          final id = int.parse(path.split('/').last);
          deleted.add(id);
          prompts.removeWhere((p) => p['id'] == id);
          body = {'deleted': true, 'id': id};
        } else if (path == '/api/tags' || path == '/api/tags/categories') {
          body = <Object>[];
        } else if (path == '/api/stats') {
          body = {
            'prompts': prompts.length,
            'media': 0,
            'tags': 0,
            'favoritePrompts': 0,
            'favoriteMedia': 0,
            'byKind': <String, int>{},
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
}

Map<String, dynamic> _prompt(int id, {int mediaCount = 0}) => {
      'id': id,
      'title': '提示词 $id',
      'kind': 'IMAGE',
      'positivePrompt': 'prompt body $id',
      'favorite': false,
      'mediaCount': mediaCount,
      'tags': <Object>[],
    };

Future<(LibraryStore, _FakeLibrary)> _makeStore(List<Map<String, dynamic>> prompts) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final fake = _FakeLibrary(prompts);
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: fake.client()));
  await store.refreshAll();
  return (store, fake);
}

Widget _wrap(LibraryStore store) => ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: const MaterialApp(home: PromptsPage()),
    );

void main() {
  testWidgets('没有关联产物的提示词上要能看到「未关联」标记', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final (store, _) = await _makeStore([
      _prompt(1),
      _prompt(2, mediaCount: 3),
    ]);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(find.text('未关联'), findsOneWidget, reason: 'mediaCount=0 的那条要有标记');
    expect(find.text('3'), findsOneWidget, reason: '有关联的显示条数');
  });

  testWidgets('「未关联产物」筛选走 hasMedia=0', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final (store, fake) = await _makeStore([_prompt(1), _prompt(2, mediaCount: 3)]);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    fake.requests.clear();
    await tester.tap(find.text('未关联产物'));
    await tester.pumpAndSettle();

    expect(store.onlyUnlinked, isTrue);
    expect(fake.requests.any((r) => r.contains('hasMedia=0')), isTrue);
    // 只剩未关联那一条
    expect(find.text('提示词 1'), findsOneWidget);
    expect(find.text('提示词 2'), findsNothing);
  });

  test('一键清除只删未关联的，有关联的一条都不动', () async {
    final (store, fake) = await _makeStore([
      _prompt(1),
      _prompt(2),
      _prompt(3, mediaCount: 5),
      _prompt(4, mediaCount: 1),
    ]);

    final removed = await store.deleteUnlinkedPrompts();

    expect(removed, 2);
    expect(fake.deleted, containsAll([1, 2]));
    expect(fake.deleted, isNot(contains(3)));
    expect(fake.deleted, isNot(contains(4)));
    expect(fake.prompts.map((p) => p['id']), [3, 4]);
  });

  test('未关联提示词超过一页时也要全部清掉', () async {
    // 250 条未关联 + 1 条有关联：超过后端单页上限 200，必须靠循环兜住
    final (store, fake) = await _makeStore([
      for (var i = 1; i <= 250; i++) _prompt(i),
      _prompt(999, mediaCount: 9),
    ]);

    final removed = await store.deleteUnlinkedPrompts();

    expect(removed, 250, reason: '不能因为翻页漏掉一批');
    expect(fake.prompts.map((p) => p['id']), [999]);
  });
}
