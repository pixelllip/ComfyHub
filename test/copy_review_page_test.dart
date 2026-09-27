// 「AI 文案台」页面（`lib/pages/copy_review_page.dart`）的两条硬防线 + 一个界面纪律。
//
// 防线一 **三态徽标**：`未校验` / `已校验` / `默认值变过，请重看` ——
//   而第三态必须**压过**"已校验"（设计 §1 纪律 2：你校验的是旧那一版）。
//   这几条一旦判反，用户就会以为"我看过了"，而上游早就改了默认值。
// 防线二 **批量先看 diff**：`应用` 在看到 diff 之前点不动，而选中集里有
//   `prompt.nsfw` 时必须**再确认一次**（设计 §4 硬规矩 1 / 4）——
//   取消之后一条 `dryRun:false` 的请求都不许发出去。
// 界面纪律 **384×853 不溢出**（AGENTS §6）：真机尺寸下列表 / 详情 / 批量面板
//   都得活着；Flutter 溢出会抛异常，用例里任何未捕获异常都会让它红。
//
// 全部走 `MockClient`：**不连真后端**。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/copy_api.dart';
import 'package:viewer/pages/copy_review_page.dart';

/// 假后端：记住每一个请求，按路径回话。
class _FakeCopyBackend {
  static const entries = <Map<String, Object?>>[
    {
      'key': 'prompt.s17',
      'scope': 'prompt',
      'title': '第 17 段',
      'default': '默认的第十七段',
      'current': '默认的第十七段',
      'overridden': false,
      'baseChanged': false,
      'refs': ['server-rs/src/nsfw_prompt.rs:120'],
    },
    {
      'key': 'prompt.nsfw',
      'scope': 'prompt',
      'title': '越狱人格段',
      'default': '原始越狱段',
      'current': '原始越狱段',
      'overridden': false,
      'baseChanged': false,
    },
    {
      'key': 'tool.comfy_submit.description',
      'scope': 'tool',
      'title': 'comfy_submit 的描述',
      'default': '后端给的默认描述',
      'current': '我改过的描述',
      'overridden': true,
      'baseChanged': false,
      'verifiedAt': '2026-09-27T10:00:00Z',
    },
    {
      // 校验过、但默认值已经变过 —— 徽标必须是"默认值变过，请重看"而不是"已校验"
      'key': 'ui.settings.nsfw.note',
      'scope': 'ui',
      'title': 'NSFW 注解',
      'default': '旧的默认注解',
      'current': '旧的默认注解',
      'overridden': false,
      'baseChanged': true,
      'verifiedAt': '2026-09-20T10:00:00Z',
    },
  ];

  final List<http.Request> requests = [];

  MockClient get client => MockClient((req) async {
        requests.add(req);
        final path = req.url.path;
        Object? body;
        var status = 200;

        if (path == '/api/copy/catalog') {
          final scope = req.url.queryParameters['scope'];
          body = entries.where((e) => scope == null || e['scope'] == scope).toList();
        } else if (path == '/api/copy/batch') {
          final sent = Map<String, dynamic>.from(jsonDecode(req.body) as Map);
          final keys = (sent['keys'] as List).cast<String>();
          body = {
            'items': [
              for (final k in keys)
                {
                  'key': k,
                  'before': '默认描述',
                  'after': '新描述',
                  'diff': '- 默认描述\n+ 新描述（dryRun 预览）',
                  'changed': true,
                },
            ],
            'changed': keys.length,
          };
        } else if (req.method == 'GET' && path.startsWith('/api/copy/')) {
          final key = Uri.decodeComponent(path.substring('/api/copy/'.length));
          body = entries.firstWhere((e) => e['key'] == key, orElse: () => const <String, Object?>{});
        } else {
          status = 404;
          body = {'error': 'NOT_FOUND', 'detail': '接口不存在: $path'};
        }
        return http.Response(jsonEncode(body), status,
            headers: {'content-type': 'application/json; charset=utf-8'});
      });

  List<Map<String, dynamic>> get batchBodies => [
        for (final r in requests.where((r) => r.url.path == '/api/copy/batch'))
          Map<String, dynamic>.from(jsonDecode(r.body) as Map),
      ];
}

/// 真机尺寸（OnePlus 8T：1080×2400 物理 / 2.8125 缩放 = 384×853 逻辑）。
Future<void> _pump(
  WidgetTester tester, {
  required Size size,
  _FakeCopyBackend? backend,
}) async {
  tester.view.physicalSize = Size(size.width * 2.8125, size.height * 2.8125);
  tester.view.devicePixelRatio = 2.8125;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);

  final be = backend ?? _FakeCopyBackend();
  await tester.pumpWidget(MaterialApp(
    home: CopyReviewPage(api: CopyApi('http://127.0.0.1:8080', client: be.client)),
  ));
  await tester.pumpAndSettle();
}

/// 某一条文案那一行上的勾选框。
Finder _checkboxOf(String key) => find.descendant(
      of: find.ancestor(of: find.text(key), matching: find.byType(ListTile)),
      matching: find.byType(Checkbox),
    );

Finder _dialogFields() =>
    find.descendant(of: find.byType(AlertDialog), matching: find.byType(TextField));

void main() {
  testWidgets('三态徽标：未校验 / 已校验 / 默认值变过，请重看（第三态压过已校验）', (tester) async {
    await _pump(tester, size: const Size(1400, 900));

    expect(find.text('未校验'), findsNWidgets(2)); // prompt.s17 + prompt.nsfw
    expect(find.text('已校验'), findsOneWidget); // tool.comfy_submit.description
    // ui.settings.nsfw.note 既校验过、默认值又变过 —— 必须显示第三态
    expect(find.text('默认值变过，请重看'), findsOneWidget);

    // 按 scope 分组（设计 §6 的四个分组标题）
    expect(find.text('系统提示词'), findsOneWidget);
    expect(find.text('工具描述'), findsOneWidget);
    expect(find.text('界面注解'), findsOneWidget);
  });

  testWidgets('批量：先 dryRun 看 diff，「应用」在看到 diff 之前点不动', (tester) async {
    final backend = _FakeCopyBackend();
    await _pump(tester, size: const Size(1400, 900), backend: backend);

    await tester.tap(_checkboxOf('prompt.s17'));
    await tester.pumpAndSettle();
    expect(find.text('已选 1 条'), findsOneWidget);

    await tester.tap(find.text('批量修改'));
    await tester.pumpAndSettle();
    expect(find.text('批量修改 1 条'), findsOneWidget);

    // ① 没预览之前「应用」是灰的
    expect(tester.widget<FilledButton>(find.widgetWithText(FilledButton, '应用')).onPressed, isNull);

    await tester.enterText(_dialogFields().at(0), '默认的第十七段');
    await tester.enterText(_dialogFields().at(1), '新的第十七段');
    await tester.pumpAndSettle();

    // ②「先看 diff」= 一次 dryRun
    await tester.tap(find.text('先看 diff'));
    await tester.pumpAndSettle();

    expect(backend.batchBodies.single['dryRun'], isTrue);
    expect(backend.batchBodies.single['op'], 'replace');
    expect(find.textContaining('dryRun 结果：1 条会变'), findsOneWidget);
    expect(find.textContaining('+ 新描述（dryRun 预览）'), findsOneWidget);

    // ③ 看完 diff 才能应用
    expect(tester.widget<FilledButton>(find.widgetWithText(FilledButton, '应用')).onPressed, isNotNull);

    // ④ 改了输入，上一次的预览立刻作废（不许"看到的是 A、应用的是 B"）
    await tester.enterText(_dialogFields().at(1), '又换了一个说法');
    await tester.pumpAndSettle();
    expect(find.textContaining('dryRun 结果'), findsNothing);
    expect(tester.widget<FilledButton>(find.widgetWithText(FilledButton, '应用')).onPressed, isNull);
  });

  testWidgets('批量里含 prompt.nsfw：应用前必须二次确认，点「先不改」一个字都不改', (tester) async {
    final backend = _FakeCopyBackend();
    await _pump(tester, size: const Size(1400, 900), backend: backend);

    await tester.tap(_checkboxOf('prompt.nsfw'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('批量修改'));
    await tester.pumpAndSettle();

    await tester.enterText(_dialogFields().at(0), '原始越狱段');
    await tester.enterText(_dialogFields().at(1), '新的越狱段');
    await tester.pumpAndSettle();
    await tester.tap(find.text('先看 diff'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('应用'));
    await tester.pumpAndSettle();

    // 二次确认写清了"那是你有意注入的那一段"（AGENTS §10）
    expect(find.text('这批里有 prompt.nsfw'), findsOneWidget);
    expect(find.textContaining('有意注入'), findsOneWidget);

    await tester.tap(find.text('先不改'));
    await tester.pumpAndSettle();
    expect(backend.batchBodies.every((b) => b['dryRun'] == true), isTrue,
        reason: '取消了就不该有 dryRun:false 的请求');
    expect(find.text('批量修改 1 条'), findsOneWidget, reason: '面板还开着，用户可以直接再来一次');

    await tester.tap(find.text('应用'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('仍然应用'));
    await tester.pumpAndSettle();

    final applied = backend.batchBodies.where((b) => b['dryRun'] == false).toList();
    expect(applied, hasLength(1));
    expect(applied.single['keys'], ['prompt.nsfw']);
    expect(applied.single['confirm'], isTrue, reason: '设计 §7 C2 里的 confirm 要跟着一起发');
    expect(find.text('批量修改 1 条'), findsNothing, reason: '应用成功后关闭面板');
  });

  testWidgets('384×853：列表 / 详情 / 批量面板都不溢出（真机尺寸）', (tester) async {
    await _pump(tester, size: const Size(384, 853));

    expect(tester.takeException(), isNull);
    expect(find.text('AI 文案台'), findsOneWidget);

    // 窄屏是"整屏切换"：点一条进详情，有返回箭头
    await tester.tap(find.text('第 17 段'));
    await tester.pumpAndSettle();
    expect(find.byTooltip('返回清单'), findsOneWidget);
    expect(find.text('默认值（真源，只读）'), findsOneWidget);
    // 默认值（只读那一格）与当前值（可编辑那一格）各一份：这条还没改过，两格一样
    expect(find.text('默认的第十七段'), findsNWidgets(2));
    // refs 是"点一下知道它在哪用"的唯一线索，必须画出来
    expect(find.textContaining('nsfw_prompt.rs:120'), findsOneWidget);
    expect(tester.takeException(), isNull);

    await tester.tap(find.byTooltip('返回清单'));
    await tester.pumpAndSettle();

    // 勾一条 → 批量面板在这一屏尺寸下也要活得下来
    await tester.tap(_checkboxOf('prompt.s17'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('批量修改'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);

    await tester.enterText(_dialogFields().at(0), '默认的第十七段');
    await tester.enterText(_dialogFields().at(1), '新的第十七段');
    await tester.pumpAndSettle();
    await tester.tap(find.text('先看 diff'));
    await tester.pumpAndSettle();

    expect(find.textContaining('dryRun 结果'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  group('导入 JSON 的解析（设计 §6：编辑器批改后再导入）', () {
    test('导出的原样形状', () {
      final rows = parseCopyOverridesJson(jsonEncode({
        'kind': 'comfyhub.copy-overrides',
        'version': 1,
        'overrides': [
          {'key': 'tool.x.description', 'text': '新描述', 'note': '为什么改'},
        ],
      }));

      expect(rows, hasLength(1));
      expect(rows.single.key, 'tool.x.description');
      expect(rows.single.text, '新描述');
      expect(rows.single.note, '为什么改');
    });

    test('裸映射（{key: text}）与数组都认', () {
      expect(parseCopyOverridesJson('{"ui.a":"甲"}').single.text, '甲');
      expect(parseCopyOverridesJson('[{"key":"ui.b","text":"乙"}]').single.text, '乙');
    });

    test('认不出来就报错，绝不静默当成空清单', () {
      expect(() => parseCopyOverridesJson('不是 JSON'), throwsFormatException);
      expect(() => parseCopyOverridesJson('42'), throwsFormatException);
      // 没有 key 的条目跳过（导不进去的东西不该变成一条空 key 的覆盖）
      expect(parseCopyOverridesJson('[{"text":"没有 key"}]'), isEmpty);
    });
  });
}
