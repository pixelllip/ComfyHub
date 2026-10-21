import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/widgets/workflow_blueprint_page.dart';
import 'package:viewer/widgets/workflow_picker.dart';

/// 「选择现有工作流」对话框：列表 → 看蓝图 / 选中。
///
/// 只喂假数据，不碰网络；解析仍走真实页面，所以"点开蓝图"这条接线是真的。
void main() {
  const raw = '{"nodes":[],"links":[],"version":0.4}';

  Future<Map<String, dynamic>> diagnose(
    String raw, {
    bool diagnose = true,
  }) async => {
    'format': 'ui',
    'objectInfo': true,
    'counts': {'nodes': 1, 'edges': 0, 'groups': 0, 'errors': 0, 'warnings': 0},
    'nodes': [
      {
        'id': '1',
        'class': 'KSampler',
        'title': 'KSampler',
        'pos': [0, 0],
        'size': [180, 80],
        'mode': 0,
        'kind': 'normal',
        'frontendOnly': false,
        'inputs': [],
        'outputs': [],
        'widgets': [],
      },
    ],
    'edges': [],
    'groups': [],
    'issues': [],
    'warnings': [],
  };

  ComfyWorkflowFile file() => const ComfyWorkflowFile(
    name: 'moss t2i.json',
    path: r'D:\comfy\workflows\moss t2i.json',
    bytes: 2048,
    format: 'ui',
    inLibrary: true,
  );

  Future<ComfyWorkflowFile?> open(
    BuildContext context, {
    required bool Function(String) onRaw,
  }) {
    return showWorkflowPicker(
      context,
      loadList: ({String? q}) async =>
          ComfyWorkflowList(files: [file()], total: 1),
      loadRaw: (path) async {
        onRaw(path);
        return raw;
      },
      diagnose: diagnose,
    );
  }

  testWidgets('列出本机工作流；点「看蓝图」真的打开蓝图页', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: FilledButton(
                onPressed: () => open(context, onRaw: (_) => true),
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

    await tester.tap(find.text('看蓝图'));
    await tester.pumpAndSettle();
    expect(
      find.byType(WorkflowBlueprintPage),
      findsOneWidget,
      reason: '「看蓝图」必须真的进蓝图页（不是只关掉弹窗）',
    );
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
                  picked = await open(context, onRaw: (_) => true);
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
