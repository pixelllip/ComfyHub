// 会话内附件的打开方式：
//
//   ① 现象："点击会话内的图片会跳转浏览器，而不是在应用内弹出小窗。"
//      → 图片走应用内的大图查看器；
//   ② 文件附件在应用内以弹窗打开，可关闭。
//      → **非图片**（PDF / 文档 / 压缩包…）也必须弹应用内的窗，不再直接 ShellExecute；
//   ③ 详情弹窗里右键 = 同一套附件菜单（复制文件 / 地址 / 文件名）：
//      缩略图上有的操作，点进来看大图之后也必须还有。
//
// 这里钉三件事：① 弹窗真的是应用内的（`Dialog`）；② 右上角的关闭按钮能把它关掉；
// ③ 弹窗内容上右键能弹出复制菜单（图片与非图片两条路各一例）。

import 'package:flutter/gestures.dart';
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/attachment_preview.dart';
import 'package:viewer/pages/ai_home_page.dart' show attachmentDisplayName;
import 'package:viewer/widgets/zoomable_image_view.dart';

/// 拦下剪贴板写入，返回收集到的文本。
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
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
  return copied;
}

Future<void> _rightClick(WidgetTester tester, Finder finder) async {
  await tester.tapAt(tester.getCenter(finder), buttons: kSecondaryMouseButton);
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 250));
}

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

  testWidgets('图片详情里右键：弹出附件菜单，复制文件名可用', (tester) async {
    final copied = _captureClipboard(tester);
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => showAttachmentPreview(
                  context,
                  url: 'http://127.0.0.1:1/never.png',
                  name: '参考图.png',
                  attachmentId: 'att-1',
                  modality: 'image',
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
    expect(find.byType(ZoomableImageView), findsOneWidget);

    await _rightClick(tester, find.byType(ZoomableImageView));
    expect(find.text('复制文件地址'), findsOneWidget);
    expect(find.text('复制文件名'), findsOneWidget);
    // 有 id 时「复制文件」也在（真正取路径要后端，这里只钉菜单项）
    expect(find.text('复制文件'), findsOneWidget);

    await tester.tap(find.text('复制文件名'));
    await tester.pump();
    expect(copied.last, '参考图.png');
  });

  testWidgets('非图片详情里右键：也弹同一套菜单', (tester) async {
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

    await _rightClick(tester, find.text('角色设定.pdf'));
    expect(find.text('复制文件地址'), findsOneWidget);
    expect(find.text('复制文件名'), findsOneWidget);
    // 没给 attachmentId：拿不到磁盘路径，这一项就不出现（不摆一个点了必失败的按钮）
    expect(find.text('复制文件'), findsNothing);
  });
}
