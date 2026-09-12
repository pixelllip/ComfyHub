// 大图查看器的交互测试：
//
//   · 滚轮缩放（以指针为中心）—— 没放大时画面正好铺满，放大之后才有可拖动的余量
//   · 鼠标 / 触摸拖动平移
//   · 右下角缩略图只在放大后出现，高亮框跟着视野走
//
// 断言直接读 InteractiveViewer 上的 TransformationController（就是本组件传进去的那个），
// 加上两个纯几何函数的单测，不依赖真实图片解码。

import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/zoomable_image_view.dart';

/// 1x1 透明 PNG：够 Image 解码成功，测试里不碰网络
final Uint8List _pngBytes = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==',
);

const Size _viewport = Size(400, 300);
const Size _image = Size(800, 600);

Future<void> _pumpViewer(WidgetTester tester) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: Center(
          child: SizedBox(
            width: _viewport.width,
            height: _viewport.height,
            child: ZoomableImageView(
              url: 'http://localhost/none.png',
              imageSize: _image,
              imageProvider: MemoryImage(_pngBytes),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

/// 取组件内部用的变换矩阵（InteractiveViewer 收到哪个 controller 就用哪个）
Matrix4 _transform(WidgetTester tester) {
  final viewer = tester.widget<InteractiveViewer>(find.byType(InteractiveViewer));
  return viewer.transformationController!.value;
}

Future<void> _scrollWheel(WidgetTester tester, Offset at, double dy) async {
  final pointer = TestPointer(1, PointerDeviceKind.mouse);
  pointer.hover(at);
  await tester.sendEventToBinding(pointer.scroll(Offset(0, dy)));
  await tester.pumpAndSettle();
}

void main() {
  group('几何换算', () {
    test('contain 之后图片居中', () {
      // 800x600 的图放进 400x300：正好等比铺满
      expect(fittedImageRect(const Size(800, 600), const Size(400, 300)),
          const Rect.fromLTWH(0, 0, 400, 300));
      // 正方形图放进宽视口：左右留白，居中
      expect(fittedImageRect(const Size(400, 400), const Size(400, 300)),
          const Rect.fromLTWH(50, 0, 300, 300));
    });

    test('放大 2 倍时，视野只占整张图的一半', () {
      final rect = fittedImageRect(const Size(800, 600), const Size(400, 300));
      final matrix = Matrix4.identity()..scaleByDouble(2, 2, 1, 1);
      final fraction = visibleImageFraction(matrix, const Size(400, 300), rect);
      expect(fraction.width, closeTo(0.5, 1e-9));
      expect(fraction.height, closeTo(0.5, 1e-9));
    });

    test('向左平移之后，高亮框向右移', () {
      final rect = fittedImageRect(const Size(800, 600), const Size(400, 300));
      final matrix = Matrix4.identity()
        ..translateByDouble(-200, 0, 0, 1)
        ..scaleByDouble(2, 2, 1, 1);
      final fraction = visibleImageFraction(matrix, const Size(400, 300), rect);
      // 画面被拖到右边（场景坐标 100~300），比例就是 0.25 起步
      expect(fraction.left, closeTo(0.25, 1e-9));
    });
  });

  group('大图查看器', () {
    testWidgets('默认适应窗口：没有缩略图，也拖不动', (tester) async {
      await _pumpViewer(tester);

      expect(find.byKey(zoomableMinimapKey), findsNothing);
      expect(find.byKey(zoomableZoomBarKey), findsOneWidget);
      expect(find.text('100%'), findsOneWidget);

      await tester.drag(find.byType(ZoomableImageView), const Offset(-60, -40));
      await tester.pumpAndSettle();
      // scale = 1 时画面正好铺满，位移会被夹回 0
      expect(_transform(tester).getTranslation().x, closeTo(0, 1e-6));
      expect(_transform(tester).getTranslation().y, closeTo(0, 1e-6));
    });

    testWidgets('滚轮放大 + 出现缩略图 + 可以拖动', (tester) async {
      await _pumpViewer(tester);
      final center = tester.getCenter(find.byType(ZoomableImageView));

      await _scrollWheel(tester, center, -120); // 向上滚 = 放大
      final zoomed = _transform(tester).getMaxScaleOnAxis();
      expect(zoomed, greaterThan(1.0));
      expect(find.byKey(zoomableMinimapKey), findsOneWidget);
      expect(find.text('${(zoomed * 100).round()}%'), findsOneWidget);

      // 拖动：画面跟着走
      final before = _transform(tester).getTranslation();
      await tester.drag(find.byType(ZoomableImageView), const Offset(-40, -30));
      await tester.pumpAndSettle();
      final after = _transform(tester).getTranslation();
      expect(after.x, lessThan(before.x));
      expect(after.y, lessThan(before.y));

      // 再滚回去：缩略图消失，回到适应窗口
      await _scrollWheel(tester, center, 400);
      expect(_transform(tester).getMaxScaleOnAxis(), closeTo(1.0, 1e-6));
      expect(find.byKey(zoomableMinimapKey), findsNothing);
    });

    testWidgets('缩放不超出上下限', (tester) async {
      await _pumpViewer(tester);
      final center = tester.getCenter(find.byType(ZoomableImageView));

      for (var i = 0; i < 12; i++) {
        await _scrollWheel(tester, center, -200);
      }
      expect(_transform(tester).getMaxScaleOnAxis(), lessThanOrEqualTo(8.0 + 1e-6));

      for (var i = 0; i < 40; i++) {
        await _scrollWheel(tester, center, 200);
      }
      expect(_transform(tester).getMaxScaleOnAxis(), greaterThanOrEqualTo(1.0 - 1e-6));
    });

    testWidgets('「适应窗口」按钮把画面复位', (tester) async {
      await _pumpViewer(tester);
      final center = tester.getCenter(find.byType(ZoomableImageView));

      await _scrollWheel(tester, center, -240);
      await tester.drag(find.byType(ZoomableImageView), const Offset(-50, -50));
      await tester.pumpAndSettle();
      expect(_transform(tester).getMaxScaleOnAxis(), greaterThan(1.0));

      await tester.tap(find.byTooltip('适应窗口'));
      await tester.pumpAndSettle();

      expect(_transform(tester).getMaxScaleOnAxis(), closeTo(1.0, 1e-6));
      expect(find.byKey(zoomableMinimapKey), findsNothing);
    });

    testWidgets('点缩略图可以把视野挪过去', (tester) async {
      await _pumpViewer(tester);
      final center = tester.getCenter(find.byType(ZoomableImageView));

      await _scrollWheel(tester, center, -300);
      await tester.pumpAndSettle();
      // 以指针（画面正中心）为中心放大：视野停在图片中央
      final before = _transform(tester).getTranslation().x;
      expect(before, lessThan(0));

      // 点缩略图右半边 → 视野往画面右侧移动
      final minimap = tester.getRect(find.byKey(zoomableMinimapKey));
      await tester.tapAt(Offset(minimap.right - 6, minimap.center.dy));
      await tester.pumpAndSettle();

      expect(_transform(tester).getTranslation().x, lessThan(before));
    });

    testWidgets('图片加载失败时给出提示而不是崩掉', (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: _viewport.width,
              height: _viewport.height,
              child: ZoomableImageView(
                url: 'http://localhost/broken.png',
                imageSize: _image,
                imageProvider: MemoryImage(Uint8List.fromList(const [1, 2, 3, 4])),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('图片加载失败'), findsOneWidget);
    });
  });
}
