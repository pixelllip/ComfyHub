// 「新建 / 编辑提示词」页（`lib/pages/prompt_edit_page.dart`）的回归。
//
// 这一页最容易出的两类事：
//   1) **数字框**：留空 / 只填一个减号或点号，`int.tryParse` / `double.tryParse`
//      回 null —— 必须原样存 null（"没填"），不能抛，也不能悄悄变成 0；
//   2) **LoRA 权重的回落**：权重写不了数字时 `double.tryParse(v) ?? 1.0`，
//      要钉住"这一条还在、权重回到 1.0"，而不是"整条 LoRA 被丢掉"。
//
// 另外钉住：保存失败时页面不关、错误说人话、可以重试；编辑走 PUT 且
// `extraParams` / `negativeMode` / `favorite` / 标签原样带回去（这些不是本页的输入框，
// 丢失即静默修改数据）。

import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/pages/prompt_edit_page.dart';
import 'package:viewer/state/library_store.dart';

class _PromptBackend {
  final List<String> calls = [];
  String? lastMethodPath;
  Map<String, dynamic>? lastBody;

  bool failSave = false;
  int nextId = 7;

  MockClient get client => MockClient((request) async {
    final path = request.url.path;
    calls.add('${request.method} $path');

    if (path == '/api/prompts' && request.method == 'POST') {
      if (failSave) return _error('磁盘空间不够');
      lastMethodPath = 'POST /api/prompts';
      lastBody = (jsonDecode(request.body) as Map).cast<String, dynamic>();
      return _ok(_savedJson(nextId, lastBody!));
    }
    if (path.startsWith('/api/prompts/') && request.method == 'PUT') {
      if (failSave) return _error('磁盘空间不够');
      lastMethodPath = 'PUT $path';
      lastBody = (jsonDecode(request.body) as Map).cast<String, dynamic>();
      final id = int.parse(path.split('/').last);
      return _ok(_savedJson(id, lastBody!));
    }

    // 保存成功后会 `refreshAll()`，给它一套空库。
    if (path == '/api/prompts' || path == '/api/media') {
      return _ok({
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 20,
        'pages': 0,
      });
    }
    if (path == '/api/tags' || path == '/api/tags/categories') {
      return _ok(<Object>[]);
    }
    if (path == '/api/stats') return _ok(<String, Object>{});
    return _error('接口不存在: $path', 404);
  });

  /// 把请求体回成一条完整的 Prompt（`toInputJson` 的 tags 是字符串数组，
  /// 而 `Prompt.fromJson` 要的是对象数组，这里转一下）。
  static Map<String, dynamic> _savedJson(int id, Map<String, dynamic> body) => {
    ...body,
    'id': id,
    'tags': [
      for (final name in (body['tags'] as List? ?? const []))
        {'id': 0, 'name': name, 'useCount': 0},
    ],
    'source': 'Manual',
    'hasWorkflow': false,
    'mediaCount': 0,
    'createdAt': '2026-11-07T03:00:00Z',
    'updatedAt': '2026-11-07T03:00:00Z',
  };

  static http.Response _ok(Object? body) => http.Response(
    jsonEncode(body),
    200,
    headers: const {'content-type': 'application/json; charset=utf-8'},
  );

  static http.Response _error(String message, [int code = 500]) =>
      http.Response(
        jsonEncode({'error': message}),
        code,
        headers: const {'content-type': 'application/json; charset=utf-8'},
      );
}

void main() {
  late _PromptBackend backend;
  late LibraryStore store;
  late SettingsStore settings;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    backend = _PromptBackend();
    settings = SettingsStore();
    await settings.load();
    store = LibraryStore(
      settings,
      api: ApiClient('http://127.0.0.1:9999', client: backend.client),
    );
  });

  tearDown(() {
    store.dispose();
    settings.dispose();
  });

  /// 从一个宿主页 push 进去：这样才能验到"保存成功 → 关页面并把结果带回去"。
  Future<void> openEditor(
    WidgetTester tester, {
    Prompt? prompt,
    List<String> initialTags = const [],
    void Function(Prompt? saved)? onResult,
  }) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => Center(
                child: TextButton(
                  onPressed: () async {
                    final saved = await Navigator.of(context).push<Prompt>(
                      MaterialPageRoute(
                        builder: (_) => PromptEditPage(
                          prompt: prompt,
                          initialTags: initialTags,
                        ),
                      ),
                    );
                    onResult?.call(saved);
                  },
                  child: const Text('打开编辑器'),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开编辑器'));
    await tester.pumpAndSettle();
  }

  Future<void> save(WidgetTester tester) async {
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
  }

  Finder field(String label) =>
      find.ancestor(of: find.text(label), matching: find.byType(TextFormField));

  testWidgets('新建：数字框留空 / 只填减号 → 存 null，合法数字原样带上', (tester) async {
    await openEditor(tester);

    await tester.enterText(field('标题'), '一只猫');
    await tester.enterText(field('正向提示词 *'), 'masterpiece, best quality');
    await tester.enterText(field('步数 Steps'), '20');
    await tester.enterText(field('CFG'), '7.5');
    await tester.enterText(field('Seed'), '-1');
    await tester.enterText(field('宽'), '-'); // 只填了一个减号：解析不了
    // 高留空
    await tester.enterText(field('批量'), '4');
    await save(tester);

    expect(backend.lastMethodPath, 'POST /api/prompts');
    final body = backend.lastBody!;
    expect(body['title'], '一只猫');
    expect(body['steps'], 20);
    expect(body['cfgScale'], 7.5);
    expect(body['seed'], -1, reason: '负数种子是合法的');
    expect(body['width'], isNull, reason: '只有减号 = 解析不了，存 null');
    expect(body['height'], isNull, reason: '留空 = 没填，存 null 而不是 0');
    expect(body['batchSize'], 4);
    expect(find.text('新建提示词'), findsNothing, reason: '保存成功要关掉编辑页');
  });

  testWidgets('数字框只填点号：解析不了就存 null，不抛异常', (tester) async {
    await openEditor(tester);

    await tester.enterText(field('标题'), 'x');
    await tester.enterText(field('步数 Steps'), '-');
    await tester.enterText(field('CFG'), '.');
    await save(tester);

    expect(tester.takeException(), isNull);
    expect(backend.lastBody!['steps'], isNull);
    expect(backend.lastBody!['cfgScale'], isNull);
  });

  testWidgets('只填标题也行：正向留空不算错', (tester) async {
    await openEditor(tester);

    await tester.enterText(field('标题'), '只有标题');
    await save(tester);

    expect(backend.lastMethodPath, 'POST /api/prompts');
    expect(backend.lastBody!['title'], '只有标题');
    expect(backend.lastBody!['positivePrompt'], '');
  });

  testWidgets('正向和标题都空：提示一句，一个请求都不发', (tester) async {
    await openEditor(tester);

    await save(tester);

    expect(find.text('正向提示词和标题至少填一个'), findsOneWidget);
    expect(
      backend.calls.where((c) => c.startsWith('POST /api/prompts')),
      isEmpty,
    );
    expect(find.text('新建提示词'), findsOneWidget, reason: '没存成就别关页面');
  });

  testWidgets('保存失败：说人话、页面不关、改好之后能重试成功', (tester) async {
    backend.failSave = true;
    Prompt? saved;
    await openEditor(tester, onResult: (p) => saved = p);

    await tester.enterText(field('标题'), '会失败的猫');
    await tester.enterText(field('正向提示词 *'), 'cat');
    await save(tester);

    expect(find.textContaining('保存失败'), findsOneWidget);
    expect(find.textContaining('磁盘空间不够'), findsOneWidget);
    expect(find.text('新建提示词'), findsOneWidget);
    expect(saved, isNull);

    // 后端恢复之后重试：同一页、同一份输入，能存成并带结果回来
    backend.failSave = false;
    await save(tester);

    expect(saved?.id, 7);
    expect(find.text('新建提示词'), findsNothing);
  });

  testWidgets('LoRA 权重解析不了 → 这一条留着、权重回落 1.0', (tester) async {
    const prompt = Prompt(
      id: 3,
      title: '带 LoRA 的预设',
      kind: PromptKind.image,
      positivePrompt: 'detail',
      loras: [LoraRef(name: 'add_detail', weight: 0.8)],
    );
    await openEditor(tester, prompt: prompt);

    await tester.enterText(field('权重'), 'abc');
    await save(tester);

    expect(backend.lastMethodPath, 'PUT /api/prompts/3');
    final loras = backend.lastBody!['loras'] as List;
    expect(loras, hasLength(1), reason: '权重写坏了也不许把这条 LoRA 丢掉');
    expect(loras.single['name'], 'add_detail');
    expect(loras.single['weight'], 1.0);
  });

  testWidgets('LoRA 权重合法时原样保存；删掉的那条不再提交', (tester) async {
    const prompt = Prompt(
      id: 4,
      title: '两条 LoRA',
      kind: PromptKind.image,
      positivePrompt: 'x',
      loras: [
        LoraRef(name: 'style_a', weight: 0.6),
        LoraRef(name: 'style_b', weight: 0.4),
      ],
    );
    await openEditor(tester, prompt: prompt);

    await tester.enterText(field('权重').first, '0.65');
    await tester.tap(find.byIcon(Icons.remove_circle_outline).last);
    await tester.pumpAndSettle();
    await save(tester);

    final loras = backend.lastBody!['loras'] as List;
    expect(loras, hasLength(1));
    expect(loras.single['name'], 'style_a');
    expect(loras.single['weight'], 0.65);
  });

  testWidgets('编辑已有：PUT 到原 id，extraParams / 只发正面 / 收藏 / 标签原样带回', (
    tester,
  ) async {
    const prompt = Prompt(
      id: 5,
      title: '夜景',
      kind: PromptKind.image,
      positivePrompt: 'night city',
      negativePrompt: 'blurry',
      seed: -1,
      extraParams: {'workflow': 'krea'},
      negativeMode: 'positive_only',
      favorite: true,
      tags: [Tag(id: 1, name: '夜景')],
    );
    Prompt? saved;
    await openEditor(tester, prompt: prompt, onResult: (p) => saved = p);

    final negativeSwitch = find.descendant(
      of: find.widgetWithText(SwitchListTile, '只发正面提示词'),
      matching: find.byType(Switch),
    );
    expect(tester.widget<Switch>(negativeSwitch).value, isTrue);

    await save(tester);

    expect(backend.lastMethodPath, 'PUT /api/prompts/5');
    final body = backend.lastBody!;
    expect(body['extraParams'], {'workflow': 'krea'});
    expect(body['negativeMode'], 'positive_only');
    expect(body['favorite'], isTrue);
    expect(body['tags'], ['夜景']);
    expect(body['seed'], -1);
    expect(saved?.id, 5);
  });

  testWidgets('从标签筛选里新建：initialTags 与预设标签合并去重', (tester) async {
    const prompt = Prompt(
      id: 6,
      title: '预设',
      kind: PromptKind.image,
      positivePrompt: 'x',
      tags: [Tag(id: 1, name: '夜景')],
    );
    await openEditor(tester, prompt: prompt, initialTags: ['夜景', '人像']);

    await save(tester);

    expect(backend.lastBody!['tags'], ['夜景', '人像']);
  });
}
