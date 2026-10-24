// 画廊「收藏」相关的两条回归（用户 2026-10-24）：
//
//  1. **展开的集合**里，成员被收藏时也要看得见星标 —— 与画廊里散图一致。
//     以前文件夹弹窗里完全不画收藏状态，展开之后就"收藏丢了"。
//
//  2. **筛选「仅收藏」时无视集合**：每张被收藏的图按**单张**列出。
//     以前 `collapse=1` 与 `favorite=1` 一起发，后端只认每个集合最早那张代表 ——
//     收藏了非代表的那张就永远看不到，代表没被收藏时整组直接消失。
//
// 所以这里钉三件事：
//  * 仅收藏那一档请求**不带** `collapse`，并且格子里没有 `CollectionThumb`；
//  * 仅收藏那一档长按一格 = **按一张**计入（不是整组 N 张）；
//  * 集合展开后，被收藏的成员身上有 `Icons.star`。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/collection_thumb.dart';
import 'package:viewer/widgets/media_thumb.dart';

/// 一条产物的 DTO。集合 9 = {2, 3, 4}；其中 **3 被收藏**（不是最早的代表 2）。
Map<String, dynamic> _media({
  required int id,
  bool favorite = false,
  int? collectionId,
  int collectionSize = 1,
  List<int> preview = const [],
}) =>
    {
      'id': id,
      'kind': 'IMAGE',
      'title': 'shot-$id.png',
      'originalName': 'shot-$id.png',
      'storedName': 's$id.png',
      'mimeType': 'image/png',
      'sizeBytes': 1024,
      'favorite': favorite,
      'source': 'ComfyUI',
      'createdAt': '2026-10-18T01:00:00.000Z',
      'fileUrl': '/api/media/$id/file',
      'thumbUrl': '/api/media/$id/thumb',
      'collectionId': collectionId,
      'collectionSize': collectionSize,
      'collectionPreview': preview,
    };

/// 记下每一次 `GET /api/media`（断言 collapse / favorite 参数用）。
final List<Uri> mediaRequests = [];

http.Client _client() => MockClient((request) async {
      final path = request.url.path;
      Object? body;
      var status = 200;
      if (path == '/api/media') {
        mediaRequests.add(request.url);
        final params = request.url.queryParameters;
        if (params['collectionId'] == '9') {
          // 展开集合：成员 2/3/4，**只有 3 被收藏**
          body = {
            'items': [
              _media(id: 2),
              _media(id: 3, favorite: true),
              _media(id: 4),
            ],
            'total': 3,
            'page': 1,
            'size': 500,
            'pages': 1,
          };
        } else if (params['favorite'] == '1') {
          // 仅收藏（collapse 已关）：收藏过的单张，哪怕它在集合里
          body = {
            'items': [
              _media(
                id: 3,
                favorite: true,
                collectionId: 9,
                collectionSize: 3,
                preview: [2, 3, 4],
              ),
              _media(id: 5, favorite: true),
            ],
            'total': 2,
            'page': 1,
            'size': 24,
            'pages': 1,
          };
        } else {
          body = {
            'items': [
              _media(id: 1),
              _media(id: 2, collectionId: 9, collectionSize: 3, preview: [2, 3, 4]),
            ],
            'total': 2,
            'page': 1,
            'size': 24,
            'pages': 1,
          };
        }
      } else if (path == '/api/tags' || path == '/api/tags/categories') {
        body = <Object>[];
      } else if (path == '/api/stats') {
        body = {
          'prompts': 0,
          'media': 5,
          'tags': 0,
          'favoritePrompts': 0,
          'favoriteMedia': 2,
          'byKind': <String, int>{},
          'byMediaKind': <String, int>{},
        };
      } else if (path == '/api/prompts') {
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

Future<(LibraryStore, SettingsStore)> _seed() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _client()));
  await store.refreshMedia();
  return (store, settings);
}

Widget _app(LibraryStore store, SettingsStore settings) => MultiProvider(
      providers: [
        ChangeNotifierProvider<LibraryStore>.value(value: store),
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ],
      child: const MaterialApp(home: GalleryPage()),
    );

void main() {
  setUp(() => mediaRequests.clear());

  testWidgets('默认（未筛选收藏）仍然折叠集合，请求带 collapse=1', (tester) async {
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    expect(store.mediaFavoritesFlat, isFalse, reason: '前提：没开「仅收藏」');
    expect(
      mediaRequests.last.queryParameters['collapse'],
      '1',
      reason: '默认画廊必须折叠集合（同一批微调显示成一个格子）',
    );
    expect(find.byType(CollectionThumb), findsOneWidget, reason: '前提：集合折叠成一个格子');
    expect(find.byType(MediaThumb), findsOneWidget);
  });

  testWidgets('「仅收藏」无视集合：请求不折叠，收藏过的单张图直接显示', (tester) async {
    final (store, settings) = await _seed();
    store.setMediaOnlyFavorite(true);
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    expect(store.mediaFavoritesFlat, isTrue);
    expect(
      mediaRequests.last.queryParameters['favorite'],
      '1',
      reason: '前提：确实在按收藏筛选',
    );
    expect(
      mediaRequests.last.queryParameters.containsKey('collapse'),
      isFalse,
      reason: '★ 仅收藏时必须关掉折叠 —— 否则集合里非代表的那几张收藏永远看不到',
    );
    expect(
      find.byType(CollectionThumb),
      findsNothing,
      reason: '★ 仅收藏时无视集合：收藏过的图要显示成单张，不是一个文件夹',
    );
    expect(
      find.byType(MediaThumb),
      findsNWidgets(2),
      reason: '两张收藏都要以单张图出现（含集合成员 3）',
    );
    expect(store.media.items.map((m) => m.id), containsAll(<int>[3, 5]));
  });

  testWidgets('「仅收藏」里长按一格按**一张**计入，不是整组 N 张', (tester) async {
    final (store, settings) = await _seed();
    store.setMediaOnlyFavorite(true);
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    final member = find.byWidgetPredicate(
      (w) => w is MediaThumb && w.media.id == 3,
    );
    expect(member, findsOneWidget, reason: '前提：集合成员 3 以单张出现');

    await tester.longPress(member);
    await tester.pumpAndSettle();

    expect(
      find.text('已选 1 张'),
      findsOneWidget,
      reason: '★ 界面上是一张单图，批量操作就必须只动这一张（不能悄悄动整组 3 张）',
    );
  });

  testWidgets('展开的集合里，被收藏的成员也显示星标（与散图一致）', (tester) async {
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    // 前提：折叠态下没有任何星标（代表 2 没被收藏，收藏的 3 藏在集合里）
    expect(find.byIcon(Icons.star), findsNothing);

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    expect(find.textContaining('这一组（3 张）'), findsOneWidget, reason: '前提：集合已展开');
    expect(
      find.byIcon(Icons.star),
      findsOneWidget,
      reason: '★ 集合里收藏过的那一张（3）必须带星标 —— 展开不该把收藏状态弄丢',
    );
  });
}
