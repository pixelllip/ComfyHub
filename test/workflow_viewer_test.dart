// 「查看工作流」的 widget 测试。
//
// ComfyUI 的工作流 JSON 是整个工作流捕获功能的最终交付物：
// 用户复制它、拖回 ComfyUI，就能复现同一次生成。
// 所以这里逐条验证弹窗的四种状态（读取中 / 有内容 / 没存过 / 读取失败）
// 以及两个详情页的接线。后端仍然用 MockClient 假造，不依赖真实 Ktor。

import 'dart:async';
import 'dart:convert';

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
import 'package:viewer/pages/prompt_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/workflow_viewer.dart';

// ---------------------------------------------------------------------------
//  假数据
// ---------------------------------------------------------------------------

/// 界面格式工作流（ComfyUI 里「保存」出来的那种），故意压缩成一行：
/// 只有真的走过 JsonEncoder 才会出现 `"key": value` 这样的空格
const _uiWorkflowJson =
    '{"last_node_id":3,"last_link_id":0,"nodes":[{"id":3,"type":"KSampler","widgets_values":[42,"fixed",32]}],'
    '"links":[],"groups":[],"config":{},"extra":{},"version":0.4}';

/// API 格式节点图：agent / 脚本直接调 `/prompt` 生图时，history 里只有这一份
const _apiGraphJson = '{"3":{"class_type":"KSampler","inputs":{"seed":42,"steps":32}},'
    '"4":{"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"sd_xl_base_1.0.safetensors"}}}';

final _promptJson = {
  'id': 1,
  'title': '雨夜霓虹街道 · 赛博朋克',
  'kind': 'IMAGE',
  'positivePrompt': 'cyberpunk city street at night, heavy rain',
  'source': 'ComfyUI',
  'sourceRef': 'run-9f3c',
  'hasWorkflow': true,
  'createdAt': '2026-09-11T04:51:00.000Z',
  'loras': <Map<String, dynamic>>[],
  'tags': <Map<String, dynamic>>[],
  'mediaCount': 0,
};

final _mediaJson = {
  'id': 1,
  'promptId': null,
  'kind': 'IMAGE',
  'title': 'neon-street.png',
  'originalName': 'neon-street.png',
  'storedName': 'abc.png',
  'mimeType': 'image/png',
  'sizeBytes': 253976,
  'width': 1216,
  'height': 832,
  'favorite': false,
  'source': 'ComfyUI',
  'hasWorkflow': true,
  'createdAt': '2026-09-11T04:51:00.000Z',
  'promptTags': <String>[],
  'fileUrl': '/api/media/1/file',
  'thumbUrl': '/api/media/1/thumb',
};

http.Response _json(Object? body, [int status = 200]) => http.Response(
      jsonEncode(body),
      status,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );

MockClient _buildMockClient({
  int promptWorkflowStatus = 200,
  int mediaWorkflowStatus = 204,
}) {
  return MockClient((request) async {
    final path = request.url.path;

    if (path == '/api/prompts/1/workflow') {
      if (promptWorkflowStatus == 204) return http.Response('', 204);
      if (promptWorkflowStatus == 200) {
        return http.Response(
          _uiWorkflowJson,
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }
      return _json({'error': '工作流读取失败'}, promptWorkflowStatus);
    }
    if (path == '/api/media/1/workflow') {
      if (mediaWorkflowStatus == 204) return http.Response('', 204);
      if (mediaWorkflowStatus == 200) {
        // 产物这条模拟 agent / 脚本提交的运行：只存得下 API 格式节点图
        return http.Response(
          _apiGraphJson,
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }
      return _json({'error': '工作流读取失败'}, mediaWorkflowStatus);
    }

    if (path == '/api/prompts/1') return _json(_promptJson);
    if (path == '/api/prompts/1/media') return _json(<Map<String, dynamic>>[]);
    if (path == '/api/media/1') return _json(_mediaJson);

    return _json({'error': 'not found: $path'}, 404);
  });
}

Future<LibraryStore> _makeStore({
  int promptWorkflowStatus = 200,
  int mediaWorkflowStatus = 204,
}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = ApiClient(
    settings.baseUrl,
    client: _buildMockClient(
      promptWorkflowStatus: promptWorkflowStatus,
      mediaWorkflowStatus: mediaWorkflowStatus,
    ),
  );
  return LibraryStore(settings, api: api);
}

/// 最小页面壳：只有一个「查看工作流」按钮。
///
/// 注意别在它外面再套一层 MaterialApp —— 嵌套 MaterialApp 会造出两个
/// ScaffoldMessenger，弹窗挂在根 Navigator 上，SnackBar 就会找不到 Scaffold。
Widget _harness(Future<String?> Function() load) => Scaffold(
      body: Center(child: WorkflowButton(title: '测试工作流', load: load)),
    );

Widget _wrapStore(LibraryStore store, Widget home) =>
    ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: MaterialApp(home: home),
    );

/// 桌面尺寸：详情页走宽屏布局，目标控件也不会落在可视区之外
void _desktopSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1600, 1100);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

/// 拦截剪贴板写入，返回收集到的文本列表
List<String> _captureClipboard(WidgetTester tester) {
  final copied = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        copied.add((call.arguments as Map)['text'] as String? ?? '');
      }
      return null;
    },
  );
  addTearDown(() => tester.binding.defaultBinaryMessenger
      .setMockMethodCallHandler(SystemChannels.platform, null));
  return copied;
}

void main() {
  testWidgets('读取过程中先转圈，读完再显示内容', (tester) async {
    final completer = Completer<String?>();
    await tester.pumpWidget(MaterialApp(home: _harness(() => completer.future)));

    await tester.tap(find.text('查看工作流'));
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(find.text('正在读取工作流…'), findsOneWidget);

    completer.complete('{"1":{"class_type":"KSampler"}}');
    await tester.pumpAndSettle();

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(find.textContaining('"class_type": "KSampler"'), findsOneWidget);
  });

  testWidgets('接口返回界面工作流时弹窗展示格式化后的 JSON', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrapStore(store, _harness(() => store.api.promptWorkflow(1))));

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    expect(find.text('测试工作流'), findsOneWidget);
    // 后端返回的是一整行，出现 ": " 说明确实走过 JsonEncoder.withIndent
    expect(find.textContaining('"type": "KSampler"'), findsOneWidget);
    expect(find.textContaining('"widgets_values"'), findsOneWidget);
    expect(find.text('这是 ComfyUI 的工作流 JSON，拖回 ComfyUI 即可复现'), findsOneWidget);
    expect(find.text('复制'), findsOneWidget);
  });

  testWidgets('API 格式节点图会换成对应的提示语', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore(mediaWorkflowStatus: 200);
    await tester.pumpWidget(_wrapStore(store, _harness(() => store.api.mediaWorkflow(1))));

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    expect(find.textContaining('"class_type": "KSampler"'), findsOneWidget);
    expect(find.textContaining('API 格式节点图'), findsOneWidget);
    expect(find.text('这是 ComfyUI 的工作流 JSON，拖回 ComfyUI 即可复现'), findsNothing);
  });

  test('API 格式与界面格式能区分开', () {
    expect(looksLikeApiGraph(jsonDecode(_apiGraphJson)), isTrue);
    expect(looksLikeApiGraph(jsonDecode(_uiWorkflowJson)), isFalse);
    expect(looksLikeApiGraph(jsonDecode('{}')), isFalse);
    expect(looksLikeApiGraph(null), isFalse);
    expect(looksLikeApiGraph('not json'), isFalse);
  });

  testWidgets('后端没有存工作流时显示空状态', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore(mediaWorkflowStatus: 204);
    await tester.pumpWidget(_wrapStore(store, _harness(() => store.api.mediaWorkflow(1))));

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    expect(find.text('这次生成没有保存工作流'), findsOneWidget);
    expect(find.textContaining('只有 ComfyUI 自动捕获或历史导入的运行'), findsOneWidget);
    // 没有内容就没什么可复制的
    expect(find.text('复制'), findsNothing);
  });

  testWidgets('复制按钮复制的是完整原文并弹出提示', (tester) async {
    _desktopSurface(tester);
    final copied = _captureClipboard(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrapStore(store, _harness(() => store.api.promptWorkflow(1))));

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('复制'));
    await tester.pumpAndSettle();

    expect(copied.single, _uiWorkflowJson, reason: '复制的必须是后端原文，而不是格式化/截断后的展示文本');
    expect(find.text('已复制，可以直接粘回 ComfyUI'), findsOneWidget);

    // 把 SnackBar 的自动关闭计时器走完，否则测试结束时会留下 pending timer
    await tester.pump(const Duration(seconds: 2));
    await tester.pumpAndSettle();
  });

  testWidgets('读取失败时弹窗内展示错误并可重试', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore(promptWorkflowStatus: 500);
    await tester.pumpWidget(_wrapStore(store, _harness(() => store.api.promptWorkflow(1))));

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    // 异常被弹窗接住，不该把整页变成错误页
    expect(find.byType(WorkflowButton), findsOneWidget);
    expect(find.textContaining('工作流读取失败'), findsOneWidget);
    expect(find.text('重试'), findsOneWidget);

    await tester.tap(find.text('重试'));
    await tester.pumpAndSettle();
    expect(find.text('重试'), findsOneWidget);
  });

  testWidgets('提示词详情在有工作流时显示来源徽标和查看工作流', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrapStore(store, const PromptDetailPage(promptId: 1)));
    await tester.pumpAndSettle();

    expect(find.text('ComfyUI 自动捕获'), findsOneWidget);
    expect(find.text('查看工作流'), findsOneWidget);

    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();
    expect(find.textContaining('"type": "KSampler"'), findsOneWidget);

    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('产物详情在有工作流时也能打开同一个弹窗', (tester) async {
    _desktopSurface(tester);
    final store = await _makeStore(mediaWorkflowStatus: 200);
    await tester.pumpWidget(_wrapStore(store, const MediaDetailPage(mediaId: 1)));
    await tester.pumpAndSettle();

    expect(find.text('查看工作流'), findsOneWidget);

    await tester.ensureVisible(find.text('查看工作流'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('查看工作流'));
    await tester.pumpAndSettle();

    expect(find.textContaining('"class_type": "KSampler"'), findsOneWidget);
    expect(find.textContaining('API 格式节点图'), findsOneWidget);
  });
}
