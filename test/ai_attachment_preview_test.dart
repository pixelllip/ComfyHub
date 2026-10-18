// 会话内图片的打开方式（用户 bug 2026-10-18）：
//
//   "点击会话内的图片会跳转浏览器而不是在应用内弹出小窗。"
//
// 成因：附件缩略图的 onTap 一律走 `_openAttachment`（`launchUrl(externalApplication)`），
// 应用内的大图查看器 `ZoomableImageView` 明明已经有了，只是聊天页从没接上。
//
// 这里钉两件事：① 判"能不能应用内看"的**按名字兜底**（历史消息没有 modality）；
// ② 弹窗真的弹的是**应用内**查看器。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/widgets/zoomable_image_view.dart';

void main() {
  group('canPreviewInline', () {
    test('有 modality 就信它', () {
      expect(canPreviewInline('image', '随便什么.bin'), isTrue);
      expect(canPreviewInline('document', 'a.png'), isFalse,
          reason: '后端说是文档就按文档走，扩展名不作为推翻依据');
      expect(canPreviewInline('video', 'a.mp4'), isFalse);
    });

    test('没有 modality（历史消息）时按扩展名兜底', () {
      expect(canPreviewInline(null, 'shot_00001_.png'), isTrue);
      expect(canPreviewInline(null, 'PHOTO.JPG'), isTrue, reason: '大小写不敏感');
      expect(canPreviewInline(null, 'a.webp'), isTrue);
      expect(canPreviewInline(null, 'note.txt'), isFalse);
      expect(canPreviewInline(null, 'voice.mp3'), isFalse);
      expect(canPreviewInline(null, '附件'), isFalse, reason: '认不出来一律外跳，不猜');
    });
  });

  testWidgets('图片走应用内弹窗；关掉之后弹窗消失', (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: ElevatedButton(
              onPressed: () => showImagePreview(
                context,
                url: 'http://127.0.0.1:1/never.png',
                imageSize: const Size(64, 64),
              ),
              child: const Text('open'),
            ),
          ),
        ),
      ),
    ));

    await tester.tap(find.text('open'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.byType(Dialog), findsOneWidget, reason: '要在应用内弹，不是跳浏览器');
    expect(find.byType(ZoomableImageView), findsOneWidget);

    await tester.tap(find.byTooltip('关闭'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.byType(ZoomableImageView), findsNothing);
  });
}
