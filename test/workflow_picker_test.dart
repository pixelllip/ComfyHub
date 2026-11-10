import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/widgets/workflow_picker.dart';

/// 「选择现有工作流」对话框：列表 → 选中。
///
/// 只喂假数据，不碰网络。
///
/// 查看蓝图入口已移除，选择器中不再有「看蓝图」，
/// 所以这里只钉"列表信息如实展示 + 点一行返回路径"。
void main() {
  ComfyWorkflowFile file() => const ComfyWorkflowFile(
    name: 'moss t2i.json',
    path: r'D:\comfy\workflows\moss t2i.json',
    bytes: 2048,
    format: 'ui',
    inLibrary: true,
  );

  Future<ComfyWorkflowFile?> open(BuildContext context) {
    return showWorkflowPicker(
      context,
      loadList: ({String? q}) async =>
          ComfyWorkflowList(files: [file()], total: 1),
    );
  }

  testWidgets('列出本机工作流：格式 / 已入库 / 路径都对', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: FilledButton(
                onPressed: () => open(context),
                child: const Text('打开选择器'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开选择器'));
    await tester.pumpAndSettle();

    expect(find.text('moss t2i.json'), findsOneWidget);
    expect(find.textContaining('界面格式'), findsWidgets);
    expect(find.textContaining('已入库'), findsOneWidget);
    // 蓝图入口已经移除：弹窗里不该再有「看蓝图」
    expect(find.text('看蓝图'), findsNothing);
  });

  testWidgets('点一行 = 选中并返回路径（写回输入框用）', (tester) async {
    ComfyWorkflowFile? picked;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: FilledButton(
                onPressed: () async {
                  picked = await open(context);
                },
                child: const Text('打开选择器'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开选择器'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('moss t2i.json'));
    await tester.pumpAndSettle();
    expect(picked?.path, r'D:\comfy\workflows\moss t2i.json');
  });
}
