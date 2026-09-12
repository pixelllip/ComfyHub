// 右键菜单的回归测试：
//
//   1) 画廊里对缩略图右键 → 关联提示词 / 收藏 / 删除
//   2) 详情页里对图片右键   → 复制文件地址 / 文件名 / 正向 / 负向提示词
//
// 这两个入口都是"不用先点进详情页、不用划选文字"的快捷操作，
// 很容易在改布局时被顺手删掉，所以用测试钉住。

import 'dart:convert';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/zoomable_image_view.dart';

const _positive =
    'cyberpunk city street at night, heavy rain, neon signs reflecting on wet asphalt';
const _negative = 'lowres, blurry, watermark';

final _promptJson = {
  'id': 1,
  'title': '雨夜霓虹街道 · 赛博朋克',
  'kind': 'IMAGE',
  'positivePrompt': _positive,
  'negativePrompt': _negative,
  'tags': <Object>[],
};

final _mediaJson = {
  'id': 1,
  'promptId': 1,
  'kind': 'IMAGE',
  'title': 'neon-street.png',
  'originalName': 'neon-street.png',
  'storedName': 'abc.png',
  'mimeType': 'image/png',
  'sizeBytes': 253976,
  'width': 1216,
  'height': 832,
  'favorite': false,
  'fileUrl': '/api/media/1/file',
  'thumbUrl': '/api/media/1/thumb',
  'promptTitle': '雨夜霓虹街道 · 赛博朋克',
  'promptPositive': _positive,
  'promptNegative': _negative,
};

List<String> _log = [];

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    _log.add('${request.method} $path');

    Object? body;
    var status = 200;
    Map<String, dynamic> page(List<Map<String, dynamic>> items) =>
        {'items': items, 'total': items.length, 'page': 1, 'size': 24, 'pages': 1};

    if (path == '/api/media') {
      body = page([_mediaJson]);
    } else if (path == '/api/media/1') {
      body = _mediaJson;
    } else if (path == '/api/media/1/prompt') {
      body = _promptJson;
    } else if (path == '/api/prompts') {
      body = page([_promptJson]);
    } else if (path == '/api/prompts/1') {
      body = _promptJson;
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 1,
        'media': 1,
        'tags': 0,
        'favoritePrompts': 1,
        'favoriteMedia': 0,
        'byKind': {'IMAGE': 1},
        'byMediaKind': {'IMAGE': 1},
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

Future<LibraryStore> _makeStore() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
  await store.refreshAll();
  return store;
}

Widget _wrap(LibraryStore store) => ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: const MaterialApp(home: GalleryPage()),
    );

void _useDesktopSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1600, 1100);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

Future<void> _rightClick(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.pumpAndSettle();
  await tester.tapAt(tester.getCenter(finder), buttons: kSecondaryMouseButton);
  await tester.pumpAndSettle();
}

/// 拦下剪贴板写入，返回收集到的文本
List<String> _captureClipboard(WidgetTester tester) {
  final copied = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        copied.add((call.arguments as Map)['text'] as String);
      }
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null),
  );
  return copied;
}

bool _called(String prefix) => _log.any((e) => e.startsWith(prefix));

void main() {
  setUp(() => _log = []);

  testWidgets('画廊缩略图右键弹出：关联提示词 / 收藏 / 删除', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _rightClick(tester, find.text('neon-street.png'));

    expect(find.text('更换关联提示词…'), findsOneWidget);
    expect(find.text('收藏'), findsOneWidget);
    expect(find.text('删除'), findsOneWidget);
  });

  testWidgets('画廊右键点「收藏」直接生效，不用进详情页', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _rightClick(tester, find.text('neon-street.png'));
    await tester.tap(find.text('收藏'));
    await tester.pumpAndSettle();

    expect(_called('PATCH /api/media/1'), isTrue);
  });

  testWidgets('画廊右键点「删除」先弹确认框', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _rightClick(tester, find.text('neon-street.png'));
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();

    expect(find.text('删除产物'), findsOneWidget);
    expect(_called('DELETE /api/media/1'), isFalse, reason: '还没确认就不该删');
  });

  testWidgets('详情页右键图片弹出复制项，复制文件地址拿到完整 URL', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await tester.tap(find.text('neon-street.png'));
    await tester.pumpAndSettle();
    expect(find.byType(MediaDetailPage), findsOneWidget);

    await _rightClick(tester, find.byType(ZoomableImageView));

    expect(find.text('复制文件地址'), findsOneWidget);
    expect(find.text('复制文件名'), findsOneWidget);
    expect(find.text('复制正向提示词'), findsOneWidget);
    expect(find.text('复制负向提示词'), findsOneWidget);

    final copied = _captureClipboard(tester);
    await tester.tap(find.text('复制文件地址'));
    await tester.pumpAndSettle();
    expect(copied.single, 'http://127.0.0.1:8080/api/media/1/file');
  });

  testWidgets('详情页右键复制正向提示词，拿到的是提示词全文', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await tester.tap(find.text('neon-street.png'));
    await tester.pumpAndSettle();

    await _rightClick(tester, find.byType(ZoomableImageView));

    final copied = _captureClipboard(tester);
    await tester.tap(find.text('复制正向提示词'));
    await tester.pumpAndSettle();
    expect(copied.single, _positive);
  });
}
