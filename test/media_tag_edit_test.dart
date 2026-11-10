// 产物标签编辑的回归：详情页要能读出**产物自身标签**、能改、保存后重新加载。
//
// 这一条防的是三件容易断的事：
//   ① `MediaAsset.ownTags` 与合并的 `promptTags` 不能混（混了会把提示词标签复制成产物标签）；
//   ② PATCH 必须真的把 `tags` 发出去；
//   ③ 保存后要重新拉一遍，界面不能还显示旧标签。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';

Map<String, dynamic> _mediaJson(List<String> ownTags) => {
  'id': 1,
  'promptId': 1,
  'kind': 'IMAGE',
  'title': '带标签的产物',
  'originalName': 'a.png',
  'storedName': 'a.png',
  'sizeBytes': 100,
  'width': 512,
  'height': 512,
  'favorite': false,
  'nsfw': false,
  'hasWorkflow': false,
  'fileUrl': '/api/media/1/file',
  'thumbUrl': '/api/media/1/thumb',
  'promptTitle': '提示词',
  'promptPositive': '1girl, cyberpunk',
  'collectionSize': 1,
  'ownTags': ownTags,
  // 合并集合：产物自身 ∪ 提示词；「来自提示词」那一组由两者相减算出来
  'promptTags': [...ownTags, '提示词标签'],
};

Map<String, dynamic> _promptJson() => {
  'id': 1,
  'title': '提示词',
  'kind': 'IMAGE',
  'positivePrompt': '1girl, cyberpunk',
  'loras': <Map<String, dynamic>>[],
  'extraParams': <String, String>{},
  'negativeMode': 'keep',
  'favorite': false,
  'hasWorkflow': false,
  'mediaCount': 1,
  'tags': <Map<String, dynamic>>[],
};

void main() {
  testWidgets('详情页：能编辑产物标签，保存后重新加载', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    var ownTags = <String>['旧标签'];
    Map<String, dynamic>? patched;

    final client = MockClient((request) async {
      final path = request.url.path;
      Object? body;
      var status = 200;
      if (path == '/api/media/1' && request.method == 'PATCH') {
        patched = jsonDecode(request.body) as Map<String, dynamic>;
        ownTags = ((patched?['tags'] as List?) ?? const [])
            .map((e) => e.toString())
            .toList();
        body = _mediaJson(ownTags);
      } else if (path == '/api/media/1') {
        body = _mediaJson(ownTags);
      } else if (path == '/api/prompts/1') {
        body = _promptJson();
      } else if (path == '/api/presets/1/media') {
        body = <Map<String, dynamic>>[];
      } else if (path == '/api/media') {
        body = {
          'items': [_mediaJson(ownTags)],
          'total': 1,
          'page': 1,
          'size': 200,
          'pages': 1,
        };
      } else if (path == '/api/tags') {
        body = [
          {'id': 1, 'name': '新标签', 'useCount': 0},
        ];
      } else if (path == '/api/tags/categories') {
        body = ['风格'];
      } else if (path == '/api/stats') {
        body = {
          'prompts': 1,
          'media': 1,
          'tags': 1,
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

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = LibraryStore(
      settings,
      api: ApiClient(settings.baseUrl, client: client),
    );
    addTearDown(store.dispose);
    await store.refreshAll();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
          ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
        ],
        child: const MaterialApp(home: MediaDetailPage(mediaId: 1)),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('产物标签'), findsOneWidget);
    expect(find.text('旧标签'), findsOneWidget);
    expect(find.text('来自提示词'), findsOneWidget);
    expect(find.text('提示词标签'), findsOneWidget);

    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    expect(find.text('编辑产物标签'), findsOneWidget);

    await tester.enterText(find.byType(TextField).first, '新标签');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();

    expect(patched, isNotNull, reason: '保存必须真的发 PATCH');
    expect(patched!['tags'], ['旧标签', '新标签']);
    expect(find.text('新标签'), findsWidgets, reason: '保存后要重新拉一遍并显示新标签');
  });
}
