//! 产物详情页的**窄屏**回归（用户 2026-10-11 报的三个真问题）：
//!
//!   ① 窗口过窄时 **pixel overflow**（`RenderFlex overflowed`）；
//!   ② **方向键**在切各个按钮的选中状态，而不是切换同批产物；
//!   ③ 窄屏下**滚轮同时滚界面和缩放图片**。
//!
//! 三条都只能靠"真的摆一次窄屏"才抓得到 —— 单测与宽屏用例一条都不会红。

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
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';
import 'package:viewer/widgets/zoomable_image_view.dart';

Map<String, dynamic> _mediaJson(
  int id, {
  int? collectionId,
  int collectionSize = 1,
}) {
  final json = <String, dynamic>{
    'id': id,
    'promptId': 1,
    'kind': 'IMAGE',
    'title': '同批 $id',
    'originalName': 'a$id.png',
    'storedName': 'a$id.png',
    'sizeBytes': 100,
    'width': 1024,
    'height': 1024,
    'favorite': false,
    'nsfw': false,
    'hasWorkflow': false,
    'fileUrl': '/api/media/$id/file',
    'thumbUrl': '/api/media/$id/thumb',
    'promptTitle': '赛博朋克少女',
    'promptPositive': '(1girl:1.2), cyberpunk',
    'promptNegative': 'lowres',
    'collectionSize': collectionSize,
  };
  // 进了集合才有这两个键（单张那条根本没有 `collectionId`）
  if (collectionId != null) {
    json['collectionId'] = collectionId;
    json['collectionPreview'] = [1, 2, 3];
  }
  return json;
}

Map<String, dynamic> _promptJson() => {
  'id': 1,
  'title': '赛博朋克少女',
  'kind': 'IMAGE',
  'positivePrompt': '(1girl:1.2), cyberpunk',
  'negativePrompt': 'lowres',
  'loras': <Map<String, dynamic>>[],
  'extraParams': <String, String>{},
  'negativeMode': 'keep',
  'favorite': false,
  'hasWorkflow': false,
  'mediaCount': 3,
  'tags': <Map<String, dynamic>>[],
};

/// 三条同批产物（集合 7），外加集合成员列表 —— 详情页要靠它算上一张 / 下一张。
MockClient _client({int members = 3}) {
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;
    if (path == '/api/media/1') {
      body = _mediaJson(1, collectionId: 7, collectionSize: members);
    } else if (path == '/api/media/2') {
      body = _mediaJson(2, collectionId: 7, collectionSize: members);
    } else if (path == '/api/media/3') {
      body = _mediaJson(3, collectionId: 7, collectionSize: members);
    } else if (path == '/api/prompts/1') {
      body = _promptJson();
    } else if (path == '/api/presets/1/media') {
      body = <Map<String, dynamic>>[];
    } else if (path == '/api/media') {
      // 集合成员列表（`collectionId=7&sort=oldest`）
      body = {
        'items': [
          for (var i = 1; i <= members; i++)
            _mediaJson(i, collectionId: 7, collectionSize: members),
        ],
        'total': members,
        'page': 1,
        'size': 200,
        'pages': 1,
      };
    } else if (path == '/api/tags') {
      body = <Map<String, dynamic>>[];
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

Future<LibraryStore> _store({int members = 3}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  return LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: _client(members: members)),
  );
}

Widget _wrap(LibraryStore store) => MultiProvider(
  providers: [
    ChangeNotifierProvider<LibraryStore>.value(value: store),
    ChangeNotifierProvider<SettingsStore>.value(value: store.settings),
    ChangeNotifierProvider<NsfwStore>(create: (_) => NsfwStore()),
  ],
  child: const MaterialApp(home: MediaDetailPage(mediaId: 1)),
);

void _surface(WidgetTester tester, Size size) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

void main() {
  // -------------------------------------------------------------------------
  //  ① 窄屏不许 overflow
  // -------------------------------------------------------------------------

  for (final width in [320.0, 384.0, 420.0]) {
    testWidgets('窄屏 ${width.toInt()}dp：详情页不许 RenderFlex overflow', (
      tester,
    ) async {
      _surface(tester, Size(width, 900));
      final store = await _store();
      addTearDown(store.dispose);
      await tester.pumpWidget(_wrap(store));
      await tester.pumpAndSettle();

      // 「关联的提示词」与「生成参数」两张卡的表头原来是固定宽的 `Row`
      // （图标 + 标题 + 两个按钮 / 一句说明），窄屏下必然挤爆 —— 现在是 `Wrap`。
      expect(
        tester.takeException(),
        isNull,
        reason: '${width.toInt()}dp 下不许有 overflow',
      );

      // 关键内容仍然在（折行不等于丢东西）
      expect(find.text('关联的提示词'), findsOneWidget);
      expect(find.text('正向提示词'), findsWidgets);
    });
  }

  // -------------------------------------------------------------------------
  //  ② 方向键切图（而不是切按钮焦点）
  // -------------------------------------------------------------------------

  testWidgets('方向键切换同批产物：→ 到第 2 张，← 回第 1 张', (tester) async {
    _surface(tester, const Size(1200, 900));
    final store = await _store();
    addTearDown(store.dispose);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(find.text('1 / 3'), findsOneWidget, reason: '进的是集合里最早那条');

    // ★ **页面本身不许被重建**（用户 2026-10-11："重新路由的实际行为是关闭旧窗口、
    // 打开新窗口。太奇怪了…能不能改成页面不变、内容实时改变？"）
    //
    // 判据用 `State` 的 identity：`pushReplacement` 会换掉整个 `State` 对象，
    // 而"原地换内容"必须还是**同一个** State 实例。`findsOneWidget` 抓不到这个差别
    // —— 换了路由之后新页面也只有一个。
    final stateBefore = tester.state(find.byType(MediaDetailPage));

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));
    // 过渡动画里**新旧两页同时在树里**：左右各露出半张图，动画结束才注销旧页。
    expect(
      find.byType(ZoomableImageView),
      findsNWidgets(2),
      reason: '翻图动画中应该同时看到新旧两张图',
    );
    await tester.pumpAndSettle();
    expect(find.text('2 / 3'), findsOneWidget, reason: '→ 该切到下一张');
    expect(
      find.byType(MediaDetailPage),
      findsOneWidget,
      reason: '**替换**路由，不是又压一层',
    );
    expect(
      identical(tester.state(find.byType(MediaDetailPage)), stateBefore),
      isTrue,
      reason: '切换必须是"原地换内容"，State 不许被重建（重建 = 关旧窗口开新窗口）',
    );
    expect(
      find.byType(ZoomableImageView),
      findsOneWidget,
      reason: '动画结束，旧页注销，只留当前这一张',
    );

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('1 / 3'), findsOneWidget, reason: '← 该回上一张');
    expect(
      identical(tester.state(find.byType(MediaDetailPage)), stateBefore),
      isTrue,
      reason: '来回切也始终是同一个页面',
    );
  });

  testWidgets('边界上方向键不响应（不会跳出去）', (tester) async {
    _surface(tester, const Size(1200, 900));
    final store = await _store();
    addTearDown(store.dispose);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    // 已经在第一张，再按 ← 还是第一张（不报错、不跳走）
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(find.text('1 / 3'), findsOneWidget);
  });

  testWidgets('图还没放大时横向拖动 = 翻页，拖动中就同时露出两张', (tester) async {
    _surface(tester, const Size(1200, 900));
    final store = await _store();
    addTearDown(store.dispose);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();
    expect(find.text('1 / 3'), findsOneWidget);

    final gesture = await tester.startGesture(
      tester.getCenter(find.byType(ZoomableImageView)),
    );
    await gesture.moveBy(const Offset(-220, 0));
    await tester.pump();

    // 拖到一半：旧图 + 新图都在树里（左右各半张），不是等松手才建新页。
    expect(
      find.byType(ZoomableImageView),
      findsNWidgets(2),
      reason: '拖动过程中要同时显示新旧两页',
    );

    await gesture.up();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('2 / 3'), findsOneWidget, reason: '松手后页码立刻跟上');
    await tester.pumpAndSettle();
    expect(find.text('2 / 3'), findsOneWidget);
    expect(find.byType(ZoomableImageView), findsOneWidget, reason: '动画结束，旧页注销');
  });

  testWidgets('只有一张（没进组）：不显示箭头与计数', (tester) async {
    _surface(tester, const Size(1200, 900));
    final store = await _store(members: 1);
    addTearDown(store.dispose);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(find.textContaining(' / '), findsNothing, reason: '单张不该有"第几张/共几张"');
    expect(find.byTooltip('上一张（←）'), findsNothing);
    expect(find.byTooltip('下一张（→）'), findsNothing);
  });

  // -------------------------------------------------------------------------
  //  ③ 图上的滚轮只缩放，不滚页面
  // -------------------------------------------------------------------------

  testWidgets('鼠标停在图上时滚轮不滚页面（交给缩放）', (tester) async {
    _surface(tester, const Size(420, 900)); // 窄屏 = 详情页外层是 ListView
    final store = await _store();
    addTearDown(store.dispose);
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    final listFinder = find.byType(Scrollable).first;
    final before =
        tester.widget<Scrollable>(listFinder).controller?.position.pixels ??
        Scrollable.of(tester.element(find.byType(ZoomableImageView)))
            .position
            .pixels;

    // 把指针放到图上再滚
    final center = tester.getCenter(find.byType(ZoomableImageView));
    final pointer = TestPointer(1, PointerDeviceKind.mouse);
    pointer.hover(center);
    await tester.sendEventToBinding(pointer.scroll(const Offset(0, 120)));
    await tester.pumpAndSettle();

    final after = Scrollable.of(tester.element(find.byType(ZoomableImageView)))
        .position
        .pixels;
    expect(after, before, reason: '图上的滚轮该被缩放吃掉（否则"滚一下既翻页又缩放"）');
  });
}
