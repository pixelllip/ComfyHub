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

MockClient _client({int total = 60}) {
  return MockClient((request) async {
    final path = request.url.path;
    final page = int.tryParse(request.url.queryParameters['page'] ?? '1') ?? 1;
    final size =
        int.tryParse(request.url.queryParameters['size'] ?? '24') ?? 24;
    Object? body;
    var status = 200;
    if (path == '/api/media') {
      final start = (page - 1) * size;
      final end = (start + size).clamp(0, total);
      body = {
        'items': [
          for (var i = start; i < end; i++)
            {
              'id': i + 1,
              'kind': 'IMAGE',
              'title': 'shot-${i + 1}.png',
              'originalName': 'shot-${i + 1}.png',
              'storedName': 's${i + 1}.png',
              'mimeType': 'image/png',
              'sizeBytes': 1024,
              'favorite': false,
              'source': 'ComfyUI',
              'createdAt': '2026-11-07T01:00:00.000Z',
              'fileUrl': '/api/media/${i + 1}/file',
              'thumbUrl': '/api/media/${i + 1}/thumb',
            },
        ],
        'total': total,
        'page': page,
        'size': size,
        'pages': (total + size - 1) ~/ size,
      };
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': total,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': <String, int>{},
        'byMediaKind': <String, int>{},
      };
    } else if (path == '/api/prompts') {
      body = {
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 24,
        'pages': 0,
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

Future<LibraryStore> _store() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: _client()),
  );
  await store.refreshAll();
  return store;
}

void main() {
  testWidgets('桌面画廊有多页时必须显示显式翻页按钮', (tester) async {
    tester.view.physicalSize = const Size(1600, 1100);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = await _store();
    addTearDown(store.dispose);
    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
        ],
        child: const MaterialApp(home: GalleryPage()),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('第 1 / 3 页'), findsOneWidget);
    expect(find.byTooltip('下一页'), findsOneWidget);
    expect(find.byTooltip('上一页'), findsOneWidget);

    // 不只是"在树里"：整条翻页栏必须真的落在窗口可视区内。
    final bar = tester.getRect(find.text('第 1 / 3 页'));
    expect(bar.top, greaterThan(0));
    expect(bar.bottom, lessThanOrEqualTo(1100));

    await tester.tap(find.byTooltip('下一页'));
    await tester.pumpAndSettle();
    expect(find.text('第 2 / 3 页'), findsOneWidget);
  });
}
