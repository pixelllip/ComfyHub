// 快速滑动卡顿相关的回归测试。
//
// 要钉住的三件事（都是"滑动大量列表项"时的真实代价）：
//
//   1. 缩略图**按格子实际绘制尺寸解码**（有上限），而不是按后端那张
//      512px JPEG / 原图的分辨率解码 —— 否则每个格子都要解 + 传一张大纹理，
//      快速滑动时一帧新进几个格子就直接掉帧；
//   2. `AdaptiveColumnList` 的懒加载：500 条也只构建可见的那些行；
//      单列时不再为每行多做一次固有尺寸查询（IntrinsicHeight）；
//   3. 画廊网格是懒的：120 条产物不会一次性全建出来，且每个格子自带
//      RepaintBoundary（由 SliverChildBuilderDelegate 自动加）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/adaptive_layout.dart';
import 'package:viewer/widgets/media_thumb.dart';

// ---------------------------------------------------------------------------
//  夹具
// ---------------------------------------------------------------------------

MockClient _emptyClient({List<Map<String, dynamic>> media = const []}) {
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;
    if (path == '/api/media') {
      body = {
        'items': media,
        'total': media.length,
        'page': 1,
        'size': 24,
        'pages': 1,
      };
    } else if (path == '/api/prompts') {
      body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': media.length,
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

Future<LibraryStore> _store({List<Map<String, dynamic>> media = const []}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  return LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _emptyClient(media: media)));
}

Map<String, dynamic> _mediaJson(int id) => {
      'id': id,
      'kind': 'IMAGE',
      'title': 'shot-$id.png',
      'originalName': 'shot-$id.png',
      'storedName': 's$id.png',
      'mimeType': 'image/png',
      'sizeBytes': 1024,
      'width': 4096,
      'height': 4096,
      'favorite': false,
      'source': 'ComfyUI',
      'createdAt': '2026-09-16T01:00:00.000Z',
      'fileUrl': '/api/media/$id/file',
      'thumbUrl': '/api/media/$id/thumb',
    };

MediaAsset _asset({int? width, int? height}) => MediaAsset(
      id: 1,
      kind: MediaKind.image,
      title: 'shot.png',
      originalName: 'shot.png',
      storedName: 's.png',
      width: width,
      height: height,
      fileUrl: '/api/media/1/file',
      thumbUrl: '/api/media/1/thumb',
    );

/// 把 MediaThumb 放进一个**固定尺寸**的盒子里，模拟网格格子。
Future<void> _pumpThumb(
  WidgetTester tester,
  LibraryStore store,
  MediaAsset media, {
  required Size cell,
  double dpr = 1.0,
}) async {
  tester.view.devicePixelRatio = dpr;
  tester.view.physicalSize = Size(cell.width * dpr + 400, cell.height * dpr + 400);
  addTearDown(tester.view.reset);

  await tester.pumpWidget(
    ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox.fromSize(
              size: cell,
              child: MediaThumb(media: media, onTap: () {}),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pump();
}

ResizeImage _thumbProvider(WidgetTester tester) {
  final image = tester.widget<Image>(find.byType(Image));
  expect(image.image, isA<ResizeImage>(),
      reason: '缩略图必须带 cacheWidth，否则会按原图分辨率解码');
  return image.image as ResizeImage;
}

// ---------------------------------------------------------------------------
//  固有尺寸查询探针
// ---------------------------------------------------------------------------

class _IntrinsicCounter {
  int calls = 0;
}

class _IntrinsicProbe extends SingleChildRenderObjectWidget {
  const _IntrinsicProbe({required this.counter, required super.child});

  final _IntrinsicCounter counter;

  @override
  _ProbeRender createRenderObject(BuildContext context) => _ProbeRender(counter);

  @override
  void updateRenderObject(BuildContext context, covariant _ProbeRender renderObject) {
    renderObject.counter = counter;
  }
}

class _ProbeRender extends RenderProxyBox {
  _ProbeRender(this.counter);

  _IntrinsicCounter counter;

  @override
  double computeMinIntrinsicHeight(double width) {
    counter.calls++;
    return super.computeMinIntrinsicHeight(width);
  }

  @override
  double computeMaxIntrinsicHeight(double width) {
    counter.calls++;
    return super.computeMaxIntrinsicHeight(width);
  }
}

void main() {
  group('缩略图按实际绘制尺寸解码', () {
    testWidgets('正方格子：解码尺寸 = 格子边长 × devicePixelRatio', (tester) async {
      final store = await _store();
      addTearDown(store.dispose);

      await _pumpThumb(tester, store, _asset(width: 4096, height: 4096),
          cell: const Size(100, 100), dpr: 2.0);
      expect(_thumbProvider(tester).width, 200);
    });

    testWidgets('竖图 / 横图：按 BoxFit.cover 需要的尺寸解码，长宽比不变形', (tester) async {
      final store = await _store();
      addTearDown(store.dispose);

      // 竖图（832×1216）盖满 100×100：宽度就是瓶颈 → 100
      await _pumpThumb(tester, store, _asset(width: 832, height: 1216),
          cell: const Size(100, 100));
      expect(_thumbProvider(tester).width, 100);

      // 横图（4096×1024）盖满 100×100：高度是瓶颈 → 需要 400 宽才够（< 512 上限）
      await _pumpThumb(tester, store, _asset(width: 4096, height: 1024),
          cell: const Size(100, 100));
      expect(_thumbProvider(tester).width, 400);
    });

    testWidgets('解码尺寸有上限：格子再大也不超过 kThumbMaxDecodeEdge', (tester) async {
      final store = await _store();
      addTearDown(store.dispose);

      await _pumpThumb(tester, store, _asset(width: 4096, height: 4096),
          cell: const Size(2000, 2000), dpr: 2.0);
      expect(_thumbProvider(tester).width, kThumbMaxDecodeEdge);
    });

    testWidgets('没有原始尺寸信息时退化成"格子长边 × dpr"，仍然有界', (tester) async {
      final store = await _store();
      addTearDown(store.dispose);

      await _pumpThumb(tester, store, _asset(), cell: const Size(120, 90), dpr: 3.0);
      expect(_thumbProvider(tester).width, 360);
    });

    testWidgets('缩略图用 FilterQuality.low（不生成 / 采样 mipmap）', (tester) async {
      final store = await _store();
      addTearDown(store.dispose);

      await _pumpThumb(tester, store, _asset(width: 4096, height: 4096),
          cell: const Size(200, 200), dpr: 1.0);
      expect(tester.widget<Image>(find.byType(Image)).filterQuality, FilterQuality.low);
    });
  });

  group('AdaptiveColumnList', () {
    testWidgets('500 条也只构建可见的那些行（懒加载没有退化成 children: [...]）', (tester) async {
      tester.view.physicalSize = const Size(1200, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final built = <int>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AdaptiveColumnList(
              itemCount: 500,
              itemBuilder: (context, i) {
                built.add(i);
                return Container(key: ValueKey('item$i'), height: 60, color: Colors.red);
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(built, isNotEmpty);
      expect(built.length, lessThan(60),
          reason: '800 高的窗口 + 250px cacheExtent 只该建几十个，不能把 500 条全建出来');
      expect(built.length, lessThan(500));
    });

    testWidgets('单列（窄窗口）不再为每行做固有尺寸查询，多列仍然等高', (tester) async {
      // ---- 窄窗口：列数 = 1 ----
      tester.view.physicalSize = const Size(900, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final single = _IntrinsicCounter();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AdaptiveColumnList(
              itemCount: 6,
              itemBuilder: (context, i) => _IntrinsicProbe(
                counter: single,
                child: Container(key: ValueKey('s$i'), height: 40, color: Colors.blue),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(single.calls, 0,
          reason: '只有一个条目时没有"等高"可言，不该付 IntrinsicHeight 的那次查询');

      // ---- 宽窗口：列数 = 2，仍然等高 ----
      tester.view.physicalSize = const Size(1200, 800);
      final multi = _IntrinsicCounter();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AdaptiveColumnList(
              itemCount: 4,
              itemBuilder: (context, i) => _IntrinsicProbe(
                counter: multi,
                child: Container(
                  key: ValueKey('m$i'),
                  // 同一行里故意做出高度差，检查是否被拉齐
                  height: i.isEven ? 40 : 90,
                  color: Colors.green,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(multi.calls, greaterThan(0), reason: '并排两个条目时仍需量一次高度来对齐');
      final r0 = tester.getRect(find.byKey(const ValueKey('m0')));
      final r1 = tester.getRect(find.byKey(const ValueKey('m1')));
      expect(r1.height, closeTo(r0.height, 0.5), reason: '同一行的卡片仍然等高');
      expect(r0.height, 90, reason: '对齐到最高的那一张');
    });
  });

  group('画廊网格', () {
    testWidgets('120 条产物只建可见的那些格子，且每个格子自带 RepaintBoundary', (tester) async {
      tester.view.physicalSize = const Size(1600, 1100);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final store = await _store(media: [for (var i = 1; i <= 120; i++) _mediaJson(i)]);
      addTearDown(store.dispose);
      await store.refreshAll();

      await tester.pumpWidget(
        ChangeNotifierProvider<LibraryStore>.value(
          value: store,
          child: const MaterialApp(home: GalleryPage()),
        ),
      );
      await tester.pumpAndSettle();

      final thumbs = find.byType(MediaThumb);
      expect(thumbs, findsWidgets);
      expect(tester.widgetList(thumbs).length, lessThan(80),
          reason: '120 条里只有可见的（+ cacheExtent 内一行的）该被建出来');

      // 网格自己不包 RepaintBoundary：SliverChildBuilderDelegate 会自动加一个
      expect(
        find.ancestor(of: thumbs.first, matching: find.byType(RepaintBoundary)),
        findsWidgets,
        reason: '每个格子要被 RepaintBoundary 隔离开',
      );

      // keep-alive 对媒体格子没用，已经关掉，省一层包装
      expect(find.byType(AutomaticKeepAlive), findsNothing);
    });

    testWidgets('视频格子请求的是 /poster 封面帧，不是回 204 的 /thumb（用户 bug ③）', (tester) async {
      tester.view.physicalSize = const Size(800, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final store = await _store();
      addTearDown(store.dispose);

      await _pumpThumb(
        tester,
        store,
        const MediaAsset(
          id: 7,
          kind: MediaKind.video,
          title: 'clip.mp4',
          originalName: 'clip.mp4',
          storedName: 'c.mp4',
          fileUrl: '/api/media/7/file',
          // 后端对视频根本不返回 thumbUrl；就算返回了也不能用
          thumbUrl: '/api/media/7/thumb',
        ),
        cell: const Size(220, 220),
      );

      final image = tester.widget<Image>(find.byType(Image));
      final provider = image.image;
      expect(provider, isA<ResizeImage>(),
          reason: '封面帧同样要按绘制像素解码，别整帧进 ImageCache');
      final inner = (provider as ResizeImage).imageProvider;
      expect(inner, isA<NetworkImage>());
      final url = (inner as NetworkImage).url;
      expect(url, endsWith('/api/media/7/poster'),
          reason: '视频要走抽帧接口；/thumb 对视频回 204，用它就永远没有预览图');
      // 播放角标仍然要在封面之上
      expect(find.byIcon(Icons.play_arrow_rounded), findsOneWidget);
    });
  });
}
