// ComfyHub 核心闭环的 widget 测试：
//
//   画廊里点开一张生成图 → 详情页显示它关联的提示词 → 点提示词 → 提示词详情
//
// 用 MockClient 假造后端响应，不依赖真实 MySQL / Ktor，因此可以稳定跑在 CI 上。
// 真实后端的端到端验证见 scripts\autorun-app.ps1。

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
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/media_detail_page.dart';
import 'package:viewer/pages/prompt_detail_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/common.dart';
import 'package:viewer/widgets/zoomable_image_view.dart';

// ---------------------------------------------------------------------------
//  假数据
// ---------------------------------------------------------------------------

const _positive =
    'cyberpunk city street at night, heavy rain, neon signs reflecting on wet asphalt';
const _negative = 'lowres, blurry, watermark';

final _promptJson = {
  'id': 1,
  'title': '雨夜霓虹街道 · 赛博朋克',
  'kind': 'IMAGE',
  'positivePrompt': _positive,
  'negativePrompt': _negative,
  'checkpoint': 'sd_xl_base_1.0.safetensors',
  'sampler': 'dpmpp_2m',
  'steps': 32,
  'cfgScale': 7.5,
  'seed': 884213771,
  'width': 1216,
  'height': 832,
  'favorite': true,
  'createdAt': '2026-09-11T01:04:51.489Z',
  // 一次生成挂多个 LoRA 是常态，详情页要能把每一个都列出来
  'loras': [
    {'name': 'detail-tweaker', 'weight': 0.8},
    {'name': 'anima-lineart-v2.safetensors', 'weight': 1.0},
  ],
  'tags': [
    {'id': 3, 'name': '赛博朋克', 'useCount': 1, 'color': '#9C27B0'},
    {'id': 8, 'name': '电影感', 'useCount': 1},
  ],
  'mediaCount': 1,
};

final _mediaJson = {
  'id': 1,
  'promptId': 1,
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
  'createdAt': '2026-09-11T01:04:51.489Z',
  'sha256': '3f8a1c9d2e7b4056a1b2c3d4e5f60718293a4b5c6d7e8f9012345678abcdef01',
  'promptTitle': '雨夜霓虹街道 · 赛博朋克',
  'promptPositive': _positive,
  'promptNegative': _negative,
  'checkpoint': 'sd_xl_base_1.0.safetensors',
  'seed': 884213771,
  'promptTags': ['赛博朋克', '电影感'],
  'fileUrl': '/api/media/1/file',
  'thumbUrl': '/api/media/1/thumb',
};

List<String> _log = [];

MockClient _buildMockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    // 记录完整 path + query，便于断言筛选参数（如 tags=）
    _log.add(
      '${request.method} $path'
      '${request.url.hasQuery ? '?${request.url.query}' : ''}',
    );

    Map<String, dynamic> page(List<Map<String, dynamic>> items) => {
          'items': items,
          'total': items.length,
          'page': 1,
          'size': 24,
          'pages': 1,
        };

    Object? body;
    var status = 200;

    if (path == '/api/media') {
      body = page([_mediaJson]);
    } else if (path == '/api/media/1') {
      body = _mediaJson;
    } else if (path == '/api/media/1/prompt') {
      body = _promptJson;
    } else if (path == '/api/prompts') {
      body = page([_promptJson]);
    } else if (path == '/api/prompts/1') {
      body = _promptJson;
    } else if (path == '/api/prompts/1/media') {
      body = [_mediaJson];
    } else if (path == '/api/tags') {
      body = [
        {'id': 3, 'name': '赛博朋克', 'useCount': 1},
      ];
    } else if (path == '/api/tags/categories') {
      body = ['风格'];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 1,
        'media': 1,
        'tags': 1,
        'favoritePrompts': 1,
        'favoriteMedia': 0,
        'byKind': {'IMAGE': 1},
        'byMediaKind': {'IMAGE': 1},
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

Future<LibraryStore> _makeStore() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = ApiClient(settings.baseUrl, client: _buildMockClient());
  final store = LibraryStore(settings, api: api);
  await store.refreshAll();
  return store;
}

Widget _wrap(LibraryStore store) => ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: const MaterialApp(home: GalleryPage()),
    );

/// 把测试画布调大，让详情页走"宽屏分栏"布局（和真实桌面端一致），
/// 同时避免目标控件落在可视区域之外导致 tap 打空。
void _useDesktopSurface(WidgetTester tester) {
  tester.view.physicalSize = const Size(1600, 1100);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
}

Future<void> _tapVisible(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.pumpAndSettle();
  await tester.tap(finder);
  await tester.pumpAndSettle();
}

/// 是否发出过以 [prefix] 开头的请求（日志里带 query，不能直接用 contains 比对整串）
bool _called(String prefix) => _log.any((e) => e.startsWith(prefix));

void main() {
  setUp(() => _log = []);

  testWidgets('画廊能渲染出后端返回的产物', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    expect(find.text('neon-street.png'), findsOneWidget);
    // 缩略图上的关联提示词标题
    expect(find.text('雨夜霓虹街道 · 赛博朋克'), findsOneWidget);
    expect(_called('GET /api/media'), isTrue);

    // 左上角类型徽标压在图片上，必须是"实心"的（以前 alpha 0.14 基本看不清）
    final badge = find.descendant(
      of: find.byType(KindBadge).first,
      matching: find.byType(Container),
    ).first;
    final decoration = tester.widget<Container>(badge).decoration! as BoxDecoration;
    expect(decoration.color!.a, greaterThan(0.9),
        reason: '缩略图上的类型徽标要够不透明才读得出来');
  });

  testWidgets('点开产物详情能看到关联提示词全文', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));

    // 进了媒体详情页
    expect(find.byType(MediaDetailPage), findsOneWidget);
    expect(_called('GET /api/media/1'), isTrue);
    expect(_called('GET /api/prompts/1'), isTrue);

    // 详情页上能看到提示词正文 + 负向提示词 + 参数
    expect(find.text('关联的提示词'), findsOneWidget);
    expect(find.text(_positive), findsOneWidget);
    expect(find.text(_negative), findsOneWidget);
    expect(find.textContaining('sd_xl_base_1.0.safetensors'), findsWidgets);
    expect(find.textContaining('884213771'), findsWidgets);
  });

  testWidgets('产物详情：生成参数放在单独的卡片里，不再混进「关联的提示词」', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));

    // 参数有自己的一张卡
    expect(find.text('生成参数'), findsOneWidget);
    final paramsCard = find.ancestor(of: find.text('生成参数'), matching: find.byType(Card)).first;
    expect(
      find.descendant(of: paramsCard, matching: find.textContaining('sd_xl_base_1.0.safetensors')),
      findsOneWidget,
      reason: '模型应该落在「生成参数」卡片里',
    );
    expect(
      find.descendant(of: paramsCard, matching: find.textContaining('884213771')),
      findsOneWidget,
      reason: 'Seed 也应该落在「生成参数」卡片里',
    );

    // 「关联的提示词」卡片里只剩提示词本身
    final promptCard = find.ancestor(of: find.text('关联的提示词'), matching: find.byType(Card)).first;
    expect(find.descendant(of: promptCard, matching: find.text(_positive)), findsOneWidget);
    expect(
      find.descendant(of: promptCard, matching: find.textContaining('sd_xl_base_1.0.safetensors')),
      findsNothing,
      reason: '模型不该再出现在提示词卡片里',
    );
    expect(
      find.descendant(of: promptCard, matching: find.textContaining('884213771')),
      findsNothing,
      reason: 'Seed 不该再出现在提示词卡片里',
    );
  });

  testWidgets('产物详情：多个 LoRA 逐个列出，点一下复制 <lora:名字:权重>', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));

    // 两个 LoRA 都列出来了（各自一个胶囊，而不是一排都写着「LoRA」）
    expect(find.text('LoRA (2)'), findsOneWidget);
    expect(find.text('detail-tweaker'), findsOneWidget);
    expect(find.text('anima-lineart-v2.safetensors'), findsOneWidget);

    final paramsCard = find.ancestor(of: find.text('生成参数'), matching: find.byType(Card)).first;
    expect(find.descendant(of: paramsCard, matching: find.text('0.8')), findsOneWidget);
    expect(find.descendant(of: paramsCard, matching: find.text('1')), findsOneWidget);

    // 点 LoRA 胶囊 → 复制提示词标签写法（拦下 Clipboard 通道看真实写入的内容）
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
      () => tester.binding.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await _tapVisible(tester, find.text('detail-tweaker'));
    expect(find.text('LoRA 标签已复制'), findsOneWidget);
    expect(copied, ['<lora:detail-tweaker:0.8>']);
  });

  testWidgets('从产物详情可以跳到提示词详情', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));
    await _tapVisible(tester, find.text('雨夜霓虹街道 · 赛博朋克'));

    expect(find.byType(PromptDetailPage), findsOneWidget);
    expect(find.text('正向提示词'), findsOneWidget);
    expect(find.text(_positive), findsOneWidget);
    // 提示词详情里的「生成参数」也是同一份渲染，多个 LoRA 同样逐个列出
    expect(find.text('LoRA (2)'), findsOneWidget);
    expect(find.text('detail-tweaker'), findsOneWidget);
    expect(find.text('anima-lineart-v2.safetensors'), findsOneWidget);

    // 「标签」在 ListView 下方，需要滚动过去（ListView 懒加载）
    await tester.dragUntilVisible(
      find.text('标签'),
      find.byType(ListView),
      const Offset(0, -220),
    );
    await tester.pumpAndSettle();
    expect(find.text('标签'), findsOneWidget);
    // 标签来自后端
    expect(find.text('赛博朋克'), findsWidgets);
  });

  testWidgets('产物详情点标签会进入该标签的搜索页', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));
    await _tapVisible(tester, find.text('赛博朋克').first);

    expect(_log.any((e) => e.contains('tags=')), isTrue,
        reason: '按标签检索时应该带上 tags 参数');
  });

  testWidgets('产物详情：大图用可缩放查看器，文件信息铺满整栏', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));
    expect(find.byType(MediaDetailPage), findsOneWidget);

    // 图片走的是带拖动 / 滚轮缩放 / 缩略图的大图查看器
    expect(find.byType(ZoomableImageView), findsOneWidget);

    // 「文件信息」卡片要占满右侧整栏（右边缘贴着面板的内边距，而不是缩在左半边）
    final card = find.ancestor(of: find.text('文件名'), matching: find.byType(Card)).first;
    final cardRect = tester.getRect(card);
    expect(cardRect.width, greaterThan(500));
    expect(cardRect.right, greaterThan(tester.view.physicalSize.width - 40));

    // 键值对排成两列铺满卡片：第二列落在卡片右半边，且和左列同一行
    final typePos = tester.getTopLeft(find.text('类型'));
    final sizePos = tester.getTopLeft(find.text('大小'));
    expect(sizePos.dy, closeTo(typePos.dy, 1));
    expect(sizePos.dx, greaterThan(cardRect.center.dx));
  });

  testWidgets('文件信息：被截断的值（SHA-256）能用 tooltip 看全', (tester) async {
    _useDesktopSurface(tester);
    final store = await _makeStore();
    await tester.pumpWidget(_wrap(store));
    await tester.pumpAndSettle();

    await _tapVisible(tester, find.text('neon-street.png'));

    final full = _mediaJson['sha256']! as String;
    // 面板里只显示前 16 位
    expect(find.text('${full.substring(0, 16)}…'), findsOneWidget);
    expect(find.text(full), findsNothing);

    // 完整值挂在 tooltip 上（鼠标停上去就能看全）
    final tips = tester
        .widgetList<Tooltip>(find.byType(Tooltip))
        .where((t) => t.message == full)
        .toList();
    expect(tips, hasLength(1), reason: '完整 SHA-256 要能通过 tooltip 看到');
  });
}
