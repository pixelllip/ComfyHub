// 产物详情页的**上下文一组**：
//
//   在预设库的卡片上挂了几个示例产物，点开其中一张，详情页左右切换
//   应该在这几个示例里走 —— 而不是按 `collectionId` 跑到它在画廊里
//   原本待的那个集合。
//
// 两条防线：
//   ① `MediaDetailPage(siblings: …)` 只认给定的一组，**不再请求集合成员**；
//   ② 卡片页点缩略图时真的把这组示例传下去了（漏传的话 ① 再对也没用）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/character_cards_page.dart';
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';
import 'package:viewer/models/models.dart';

Map<String, dynamic> _mediaJson(
  int id, {
  int? collectionId,
  int collectionSize = 1,
}) {
  final json = <String, dynamic>{
    'id': id,
    'kind': 'IMAGE',
    'title': '示例 $id',
    'originalName': 'sample-$id.png',
    'storedName': 'sample-$id.png',
    'sizeBytes': 100,
    'width': 1024,
    'height': 1024,
    'favorite': false,
    'nsfw': false,
    'hasWorkflow': false,
    'fileUrl': '/api/media/$id/file',
    'thumbUrl': '/api/media/$id/thumb',
    'collectionSize': collectionSize,
  };
  if (collectionId != null) {
    json['collectionId'] = collectionId;
    json['collectionPreview'] = [8, 9];
  }
  return json;
}

MediaAsset _asset(int id, {int? collectionId, int collectionSize = 1}) =>
    MediaAsset.fromJson(
      _mediaJson(
        id,
        collectionId: collectionId,
        collectionSize: collectionSize,
      ),
    );

http.Response _json(Object? body) => http.Response(
  jsonEncode(body),
  200,
  headers: {'content-type': 'application/json; charset=utf-8'},
);

Future<LibraryStore> _store(MockClient client) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  return LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: client),
  );
}

/// 画廊里的集合 7 只有 8、9 两张 —— 详情页要是按集合取成员，
/// 当前这条（1 / 2 / 3）根本不在里面，计数就会消失或变成别的数字。
MockClient _detailClient(List<String> calls) => MockClient((request) async {
  calls.add(
    '${request.method} ${request.url.path}'
    '${request.url.hasQuery ? '?${request.url.query}' : ''}',
  );
  final path = request.url.path;
  if (path == '/api/media/1' ||
      path == '/api/media/2' ||
      path == '/api/media/3') {
    final id = int.parse(path.split('/').last);
    return _json(_mediaJson(id, collectionId: 7, collectionSize: 2));
  }
  if (path == '/api/media') {
    // 真的被问到了就回一个**不包含当前三条**的集合，让断言更容易看懂。
    return _json({
      'items': [
        _mediaJson(8, collectionId: 7, collectionSize: 2),
        _mediaJson(9, collectionId: 7, collectionSize: 2),
      ],
      'total': 2,
      'page': 1,
      'size': 200,
      'pages': 1,
    });
  }
  return http.Response('{"error":"not found: $path"}', 404);
});

void _surface(WidgetTester tester, Size size) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('传了示例组就只在这一组里切换，不再问画廊集合', (tester) async {
    _surface(tester, const Size(1200, 900));
    final calls = <String>[];
    final store = await _store(_detailClient(calls));
    addTearDown(store.dispose);

    // 顺序刻意与画廊集合（1、2、3）不同：按示例组走时，1 是第 2 张。
    final examples = [_asset(2), _asset(1), _asset(3)];

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
          ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
        ],
        child: MaterialApp(
          home: MediaDetailPage(mediaId: 1, siblings: examples),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(
      find.text('2 / 3'),
      findsOneWidget,
      reason: '计数必须按示例组的顺序算（1 在这一组的第 2 位）',
    );
    expect(
      calls.where((c) => c.contains('collectionId=')),
      isEmpty,
      reason: '从示例进来时不该再去列画廊集合成员',
    );

    // → 到这一组的第 3 张（id 3），不是集合里的下一张
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pumpAndSettle();
    expect(find.text('3 / 3'), findsOneWidget);
    expect(find.text('示例 3'), findsOneWidget);

    // ← 回到第 2 张（id 1），再 ← 到第 1 张（id 2）
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('2 / 3'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('1 / 3'), findsOneWidget);
    expect(find.text('示例 2'), findsOneWidget);
  });

  testWidgets('卡片上点示例缩略图：详情页拿到的是整张卡的示例组', (tester) async {
    _surface(tester, const Size(1200, 900));
    final calls = <String>[];

    // 卡片库（AiApiClient）与产物详情（LibraryStore.api）是两个客户端。
    final card = {
      'id': 1,
      'name': 'shinku',
      'prompt': 'A quiet girl with red eyes.',
      'examples': [
        _mediaJson(1, collectionId: 7, collectionSize: 2),
        _mediaJson(2, collectionId: 7, collectionSize: 2),
        _mediaJson(3, collectionId: 7, collectionSize: 2),
      ],
      'updatedAt': '2026-10-01T00:00:00Z',
    };
    final cardsClient = MockClient((request) async {
      if (request.url.path == '/api/ai/character-cards') return _json([card]);
      return http.Response('{"error":"not found"}', 404);
    });

    final store = await _store(_detailClient(calls));
    addTearDown(store.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
          ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
        ],
        child: MaterialApp(
          home: CharacterCardsPage(
            api: AiApiClient('http://127.0.0.1:8080', client: cardsClient),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // 卡片上的三张示例缩略图
    expect(find.byType(Image), findsNWidgets(3));
    await tester.tap(find.byType(Image).first);
    await tester.pumpAndSettle();

    expect(find.byType(MediaDetailPage), findsOneWidget);
    expect(
      find.text('1 / 3'),
      findsOneWidget,
      reason: '打开的是卡片这一组（3 张），不是画廊集合 7（2 张）',
    );
    expect(
      calls.where((c) => c.contains('collectionId=')),
      isEmpty,
      reason: '点示例缩略图不该去列它原本所属的画廊集合',
    );

    // 左右切换仍在这 3 张示例里
    await tester.tap(find.byTooltip('下一张（→）'));
    await tester.pumpAndSettle();
    expect(find.text('2 / 3'), findsOneWidget);
    expect(find.text('示例 2'), findsOneWidget);

    // 返回卡片列表（别把详情页留在栈上影响后续）
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('shinku'), findsOneWidget);
  });

  testWidgets('只给一组 id：详情页补齐这一组，并按这一组切换（工作台的一批产物）', (tester) async {
    _surface(tester, const Size(1200, 900));
    final calls = <String>[];
    final store = await _store(_detailClient(calls));
    addTearDown(store.dispose);

    // 顺序刻意与画廊集合（1、2、3）不同：按这一批走时，1 是第 2 张。
    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<LibraryStore>.value(value: store),
          ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
          ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
        ],
        child: MaterialApp(
          home: MediaDetailPage(mediaId: 1, siblingIds: const [2, 1, 3]),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(
      find.text('2 / 3'),
      findsOneWidget,
      reason: '计数必须按这一批 id 的顺序算（1 在这一批的第 2 位）',
    );
    expect(
      calls.where((c) => c.contains('collectionId=')),
      isEmpty,
      reason: '只给了一组 id 时不该再去列画廊集合成员',
    );
    // 整组都要补齐：翻页动画要同时画出前后两条，缺一条就翻不过去。
    expect(
      calls.where((c) => c == 'GET /api/media/2'),
      isNotEmpty,
      reason: '这一批里排在当前条前面的那条要在打开时取到',
    );
    expect(
      calls.where((c) => c == 'GET /api/media/3'),
      isNotEmpty,
      reason: '这一批里排在当前条后面的那条要在打开时取到',
    );

    // → 到这一批的第 3 张（id 3），不是集合里的下一张
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pumpAndSettle();
    expect(find.text('3 / 3'), findsOneWidget);
    expect(find.text('示例 3'), findsOneWidget);

    // ← 回到第 2 张（id 1），再 ← 到第 1 张（id 2）
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('2 / 3'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('1 / 3'), findsOneWidget);
    expect(find.text('示例 2'), findsOneWidget);
  });
}
