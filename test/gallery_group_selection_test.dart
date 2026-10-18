// 画廊"整组"语义的回归（用户 bug 2026-10-18）：
//
//   "我右键删除画廊选中的图片的时候，一次只能删一张，应当区分删一张和删集合内所有照片的行为。"
//
// 成因：多选态只存**代表那条**的 media id，集合格子和单图在 `Set<int>` 里长得一模一样，
// 于是批量删/收藏/关联都只动了封面那一张。
//
// 这里钉的是最便宜、也最容易再退回去的那一半：**长按集合格子要按整组计入**。
// （"删一张 / 删整组"的菜单分支需要 dialog + 假后端联动，另见 gallery_archive 的说明。）

import 'dart:convert';

import 'package:flutter/gestures.dart' show kLongPressTimeout;
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

/// 一条产物的 DTO。id=1 是散图，id=2 是「3 张一组」的集合格子（成员 2/3/4）。
Map<String, dynamic> _media({
  required int id,
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
      'favorite': false,
      'source': 'ComfyUI',
      'createdAt': '2026-10-18T01:00:00.000Z',
      'fileUrl': '/api/media/$id/file',
      'thumbUrl': '/api/media/$id/thumb',
      'collectionId': collectionId,
      'collectionSize': collectionSize,
      'collectionPreview': preview,
    };

/// 测试里记下"新建集合"这一路请求（拖拽落点的断言吃它）。
final List<Map<String, dynamic>> createdCollections = [];

/// 记下每一次 `PATCH /api/media/{id}`：`(id, body)`。拖拽落点最关键的证据在这里。
final List<({int id, Map<String, dynamic> body})> patchedMedia = [];

http.Client _client() => MockClient((request) async {
      final path = request.url.path;
      Object? body;
      var status = 200;
      final patch = RegExp(r'^/api/media/(\d+)$').firstMatch(path);
      if (request.method == 'PATCH' && patch != null) {
        patchedMedia.add((
          id: int.parse(patch.group(1)!),
          body: Map<String, dynamic>.from(jsonDecode(request.body) as Map),
        ));
        body = _media(id: int.parse(patch.group(1)!));
      } else if (request.method == 'POST' && path == '/api/media/collections') {
        final sent = Map<String, dynamic>.from(jsonDecode(request.body) as Map);
        createdCollections.add(sent);
        status = 201;
        body = {
          'id': 77,
          'kind': 'IMAGE',
          'origin': 'manual',
          'size': (sent['mediaIds'] as List).length,
        };
      } else if (path == '/api/media') {
        // 按 collectionId 列成员要**如实**回那一组的成员：集合 9 是 {2,3,4}。
        // 假后端在这里含糊，用例就会对着一个不真实的成员表下断言。
        if (request.url.queryParameters['collectionId'] == '9') {
          body = {
            'items': [_media(id: 2), _media(id: 3), _media(id: 4)],
            'total': 3,
            'page': 1,
            'size': 500,
            'pages': 1,
          };
          return http.Response(
            jsonEncode(body),
            status,
            headers: {'content-type': 'application/json; charset=utf-8'},
          );
        }
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
      } else if (path == '/api/tags' || path == '/api/tags/categories') {
        body = <Object>[];
      } else if (path == '/api/stats') {
        body = {
          'prompts': 0,
          'media': 2,
          'tags': 0,
          'favoritePrompts': 0,
          'favoriteMedia': 0,
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
  setUp(() {
    createdCollections.clear();
    patchedMedia.clear();
    debugMouseFirstOverride = null;
  });

  tearDown(() => debugMouseFirstOverride = null);
  testWidgets('长按集合格子按**整组**计入（3 张），不是一个格子一张', (tester) async {
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.longPress(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    expect(
      find.text('已选 3 张'),
      findsOneWidget,
      reason: '集合的 3 张要一起被选中 —— 否则批量删只会删掉封面那一张（2026-10-18 的 bug）',
    );
  });

  testWidgets('长按散图只选 1 张', (tester) async {
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.longPress(find.byType(MediaThumb));
    await tester.pumpAndSettle();

    expect(find.text('已选 1 张'), findsOneWidget);
  });

  testWidgets('再点一次同一格 = 取消选中，整组一起退出', (tester) async {
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.longPress(find.byType(CollectionThumb));
    await tester.pumpAndSettle();
    expect(find.text('已选 3 张'), findsOneWidget);

    // 多选态下再点一格 = 取消
    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    expect(find.text('已选 0 张'), findsOneWidget, reason: '整组要一起退出，不能只退一张');
  });

  testWidgets('点集合格子 = **展开列出成员**，不是直接跳封面详情', (tester) async {
    // 用户 2026-10-18："手机系统管理图标的逻辑是：点进文件夹（在弹窗展开文件夹列出
    // 所有图片，再选择其一进入它的详情。"（并要求能把成员拖出去）
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    expect(find.textContaining('这一组'), findsNothing, reason: '前提：没展开');

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    expect(find.textContaining('这一组（3 张）'), findsOneWidget, reason: '要点开文件夹列成员');
    expect(
      find.textContaining('长按可多选'),
      findsOneWidget,
      reason: '集合内要能多选（用户 2026-10-18："在集合内我不能多选操作"）',
    );

    // 集合内多选：长按一张 → 出现多选操作栏
    await tester.longPress(find.byKey(const Key('folder-member-thumb')).first);
    await tester.pumpAndSettle();
    expect(find.text('全选'), findsOneWidget);
    expect(find.text('移出集合'), findsOneWidget);

    // 收起
    await tester.tap(find.byTooltip('关闭'));
    await tester.pumpAndSettle();
    expect(find.textContaining('这一组'), findsNothing);
  });

  testWidgets('桌面：**直接左键拖**一格到另一格 = 并进它所在的集合（不用先进模式）', (tester) async {
    // 用户 2026-10-18："电脑端画廊并不支持拖动行为。"（真因是以前只做了长按拖，
    // 而长按拖在鼠标上既不直观、又会被父级的长按"进模式"抢走手势）
    debugMouseFirstOverride = true;
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    final from = tester.getCenter(find.byType(MediaThumb));
    final to = tester.getCenter(find.byType(CollectionThumb));

    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(24, 0));
    await tester.pump();
    await gesture.moveTo(to);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    final moved = patchedMedia.where((call) => call.body['collectionId'] == 9).toList();
    expect(moved, isNotEmpty, reason: '★ 松手必须真的把这一格并进目标所在的集合');
    expect(moved.first.id, 1);
    expect(store.media.items.any((m) => m.id == 1), isTrue, reason: '移动的是归属，不是删东西');
  });

  testWidgets('桌面：拖**集合格子**到散图上 = 当场新建一组，且含**整组**', (tester) async {
    debugMouseFirstOverride = true;
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    final from = tester.getCenter(find.byType(CollectionThumb));
    final to = tester.getCenter(find.byType(MediaThumb));

    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(-24, 0));
    await tester.pump();
    await gesture.moveTo(to);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    expect(createdCollections, hasLength(1), reason: '目标没有集合 → 当场新建一个');
    final ids = (createdCollections.single['mediaIds'] as List).cast<int>().toSet();
    expect(
      ids,
      {1, 2, 3, 4},
      reason: '★ 新建的组要含"拖过来的整组（2/3/4）"加上目标（1）—— 动整组，不是只动封面',
    );
  });

  testWidgets('弹窗里把成员**拖出弹窗外**：弹窗让开，松手落到底部条 = 移出集合', (tester) async {
    // 用户 2026-10-18："打开集合应该是出一个弹窗，我把内容移出到弹窗外、弹窗关闭，
    // 再移动到某个元素上，松手完成移动操作。"
    debugMouseFirstOverride = true;
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();
    expect(find.textContaining('这一组（3 张）'), findsOneWidget);

    final from = tester.getCenter(find.byKey(const Key('folder-member-thumb')).first);

    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump();
    // 先移到弹窗**矩形之外**（左下角）—— 弹窗要让开，底下的落点才收得到
    await gesture.moveTo(const Offset(30, 570));
    await tester.pump();
    expect(
      find.textContaining('这一组（3 张）'),
      findsNothing,
      reason: '★ 拖出弹窗外之后弹窗必须让开，否则底下的格子根本收不到落点',
    );

    // 再落到"移出集合"条上（它只在拖动中才出现）
    final strip = tester.getCenter(find.text('拖到这里 = 移出集合'));
    await gesture.moveTo(Offset(strip.dx, strip.dy - 8));
    await tester.pump();
    await gesture.moveTo(strip);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    expect(patchedMedia, isNotEmpty, reason: '松手有没有真的发出请求');
    expect(
      patchedMedia.where((call) => call.body['clearCollection'] == true),
      isNotEmpty,
      reason: '★ 松手必须真的把这一张移出集合',
    );
    expect(find.textContaining('这一组'), findsNothing, reason: '拖完弹窗收起');
  });

  testWidgets('弹窗里多选之后拖其中一张 = **整批一起走**', (tester) async {
    // 用户 2026-10-18："全选、移出集合、删除。同时要做到可以多选拖拽。"
    // ⚠ 弹窗里的多选与画廊主界面那个是**两套**状态，只认后者的话这里只会搬走一张。
    debugMouseFirstOverride = true;
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    final thumbs = find.byKey(const Key('folder-member-thumb'));
    // 长按第一张 → 进集合内多选；再点第二张一起选中
    await tester.longPress(thumbs.first);
    await tester.pumpAndSettle();
    await tester.tap(thumbs.at(1));
    await tester.pumpAndSettle();
    expect(find.text('已选 2 张'), findsWidgets, reason: '前提：勾了两张（标题与操作栏各一处）');

    final from = tester.getCenter(thumbs.first);
    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump();
    await gesture.moveTo(const Offset(30, 570));
    await tester.pump();
    final strip = tester.getCenter(find.text('拖到这里 = 移出集合'));
    await gesture.moveTo(strip);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    final cleared = patchedMedia
        .where((call) => call.body['clearCollection'] == true)
        .map((call) => call.id)
        .toSet();
    expect(
      cleared,
      {2, 3},
      reason: '★ 勾了两张，拖其中一张要**两张一起**移出集合（不是只搬被拖的那一张）',
    );
  });

  testWidgets('宽屏：停靠面板**不会让开**；拖到空地上松手 = 直接移出集合', (tester) async {
    // 用户 2026-10-18："停靠面板适合电脑 UI，弹窗适合手机 UI" + "我还希望…拖动到弹窗外，
    // 能够直接解除移出集合，而不再只能拖动到屏幕底"。
    // 宽屏是**停靠**形态：面板占真实布局空间、根本不需要"让开"，拖出去随便松手就算移出。
    debugMouseFirstOverride = true;
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();
    expect(find.textContaining('这一组（3 张）'), findsOneWidget, reason: '前提：面板开着');

    final from = tester.getCenter(find.byKey(const Key('folder-member-thumb')).first);
    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump();
    // 面板在右侧 380 宽；左下角是"面板之外、格子之外、底部条之上"的空地
    await gesture.moveTo(const Offset(40, 900));
    await tester.pump();
    expect(
      find.textContaining('这一组（3 张）'),
      findsOneWidget,
      reason: '★ 停靠面板**不该**因为拖出去就消失（它本来就不挡路）',
    );

    await gesture.up();
    await tester.pumpAndSettle();

    expect(
      patchedMedia.where((call) => call.body['clearCollection'] == true),
      isNotEmpty,
      reason: '★ 松手在空地上就必须算"移出集合"，不该要求用户把指针挪到底部那条上',
    );
  });

  testWidgets('窄屏：居中弹窗，拖出弹窗外它就彻底让开（不再回来挡住）', (tester) async {
    // 窄屏才是弹窗形态。⚠ 让开之后**不许**再"拖回来就重新挡住" ——
    // 那会让窗口中间那一片格子永远落不到（用户 2026-10-18 报的正是这个）。
    debugMouseFirstOverride = true;
    tester.view.physicalSize = const Size(820, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();
    expect(find.textContaining('这一组（3 张）'), findsOneWidget);

    final from = tester.getCenter(find.byKey(const Key('folder-member-thumb')).first);
    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump();

    // 移到弹窗之外，再**移回弹窗正中间**：不许因此把弹窗放回来
    await gesture.moveTo(const Offset(20, 880));
    await tester.pump();
    expect(find.textContaining('这一组（3 张）'), findsNothing, reason: '拖出去就该让开');

    final center = Offset(tester.view.physicalSize.width / 2, 450);
    await gesture.moveTo(center);
    await tester.pump();
    expect(
      find.textContaining('这一组（3 张）'),
      findsNothing,
      reason: '★ 让开之后**不许**再"拖回来就重新挡住"，否则中间那些格子永远落不到',
    );

    await gesture.up();
    await tester.pumpAndSettle();
  });

  testWidgets('把**外面的**一张拖进面板里 = 加进这一组（面板整体都是落点）', (tester) async {
    // 用户 2026-10-18："B 有个 bug：我不能把目标拖进停靠面板内，实现集合加入操作。"
    // 真因：只把**成员格子**做成了落点，面板的标题 / 空白 / 成员之间的缝都是死的。
    debugMouseFirstOverride = true;
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    await tester.tap(find.byType(CollectionThumb));
    await tester.pumpAndSettle();

    // 拖画廊里的散图（id=1）到面板**标题**上（不是成员格子）
    final from = tester.getCenter(find.byType(MediaThumb));
    final to = tester.getCenter(find.textContaining('这一组（3 张）'));

    final gesture = await tester.startGesture(from);
    await tester.pump();
    await gesture.moveBy(const Offset(24, 0));
    await tester.pump();
    await gesture.moveTo(to);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    final into = patchedMedia.where((call) => call.body['collectionId'] == 9).toList();
    expect(into, isNotEmpty, reason: '★ 拖到面板上必须真的加进这一组');
    expect(into.first.id, 1, reason: '被加进去的是拖过来的那一张');
  });

  testWidgets('触屏：长按进模式后**长按拖**同样能移动（这条路径别被改坏）', (tester) async {
    // 触屏上必须长按才起拖：否则轻轻一划就变成拖图，跟 `GridView` 的滚动抢手势。
    debugMouseFirstOverride = false;
    final (store, settings) = await _seed();
    await tester.pumpWidget(_app(store, settings));
    await tester.pumpAndSettle();

    final from = tester.getCenter(find.byType(MediaThumb));
    final to = tester.getCenter(find.byType(CollectionThumb));

    // 1) 模式外：长按 = 进整理模式
    await tester.longPress(find.byType(MediaThumb));
    await tester.pumpAndSettle();
    expect(find.text('已选 1 张'), findsOneWidget);

    // 2) 模式内：长按拖
    final gesture = await tester.startGesture(from);
    await tester.pump(kLongPressTimeout + const Duration(milliseconds: 60));
    await gesture.moveTo(to);
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    expect(
      patchedMedia.where((call) => call.body['collectionId'] == 9),
      isNotEmpty,
      reason: '触屏那条路也要真的能移动',
    );
  });
}
