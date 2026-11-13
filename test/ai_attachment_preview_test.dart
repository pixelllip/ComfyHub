// 会话内附件的打开方式：
//
//   ① 现象："点击会话内的图片会跳转浏览器，而不是在应用内弹出小窗。"
//      → 图片走应用内的大图查看器；
//   ② 文件附件在应用内以弹窗打开，可关闭。
//      → **非图片**（PDF / 文档 / 压缩包…）也必须弹应用内的窗，不再直接 ShellExecute。
//
// 这里钉两件事：① 弹窗真的是应用内的（`Dialog`）；② 右上角的关闭按钮能把它关掉。

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/attachment_preview.dart';
import 'package:viewer/pages/ai_home_page.dart' show attachmentDisplayName;
import 'package:viewer/widgets/zoomable_image_view.dart';

void main() {
  test('历史附件没有原始文件名时，不把 UUID 当标题', () {
    const id = '00000000-000c-47c1-aa7c-15eb5f8cdcdc';
    expect(attachmentDisplayName(id, hasPreview: true), '图片附件');
    expect(attachmentDisplayName(id, hasPreview: false), '附件');
    expect(attachmentDisplayName('角色设定.png', hasPreview: true), '角色设定.png');
    expect(attachmentDisplayName('  ', hasPreview: false), '附件');
  });

  testWidgets('图片走应用内弹窗；关掉之后弹窗消失', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
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
      ),
    );

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

  testWidgets('非图片附件也在应用内弹窗，且可以关闭', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => showAttachmentPreview(
                  context,
                  url: 'http://127.0.0.1:1/never.pdf',
                  name: '角色设定.pdf',
                  modality: 'document',
                ),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('open'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.byType(Dialog), findsOneWidget, reason: '不许再把用户丢给系统程序');
    expect(find.text('角色设定.pdf'), findsOneWidget);
    expect(find.textContaining('暂不支持在应用内预览'), findsOneWidget);
    expect(find.byTooltip('关闭'), findsOneWidget);

    await tester.tap(find.byTooltip('关闭'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.byType(Dialog), findsNothing);
  });
}
