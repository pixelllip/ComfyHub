import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/models/layout_embed.dart';
import 'package:viewer/models/layout_spec.dart';
import 'package:viewer/pages/layout_canvas_page.dart';
import 'package:viewer/widgets/layout_templates_card.dart';

/// 一份**后端规范化之后**的候选（`ai/layout_proposal.rs` 的 payload 形状）。
Map<String, dynamic> _payload() => {
  'layoutTemplates': [
    {
      'title': '三分法：旅人压左',
      'note': '主体压左三分线，右侧留出教堂',
      'spec': {
        'v': 1,
        'width': 768,
        'height': 1024,
        'brief': '雾夜废墟，一名提灯的旅人正走近教堂',
        'camera': 'eye',
        'regions': [
          {
            'id': 'r1',
            'x': 40,
            'y': 300,
            'w': 260,
            'h': 640,
            'kind': 'character',
            'label': '提灯的旅人',
            'caption': '灯是唯一暖色',
            'text': '',
            'relation': 'none',
            'relationTo': null,
            'surface': 'floor',
          },
          {
            'id': 'r2',
            'x': 620,
            'y': 120,
            'w': 360,
            'h': 760,
            'kind': 'object',
            'label': '教堂',
            'caption': '',
            'text': '',
            'relation': 'inFront',
            'relationTo': 'r1',
            'surface': 'back',
          },
        ],
      },
    },
  ],
  'issues': [],
};

void main() {
  group('LayoutCandidate：读工具结构化结果', () {
    test('正常 payload → 一条候选，坐标 / 关系 / brief 都在', () {
      final list = LayoutCandidate.listFromPayload(_payload());
      expect(list, hasLength(1));
      final c = list.first;
      expect(c.title, '三分法：旅人压左');
      expect(c.note, '主体压左三分线，右侧留出教堂');
      expect(c.spec.width, 768);
      expect(c.spec.height, 1024);
      expect(c.spec.brief, contains('提灯的旅人'));
      expect(c.spec.camera, LayoutCamera.eye);
      expect(c.spec.regions, hasLength(2));
      expect(c.spec.regions[1].relation, LayoutRelation.inFront);
      expect(c.spec.regions[1].relationTo, 'r1');
      expect(c.spec.regions[0].surface, LayoutSurface.floor);
    });

    test('tool_result 的 jsonPayload 还原成候选（重开 App 走的就是这条路）', () {
      final parts = <AiMessagePart>[
        const AiMessagePart(
          type: 'tool_call',
          toolCallId: 'call-1',
          payload: {'name': 'layout_propose'},
        ),
        AiMessagePart(
          type: 'tool_result',
          toolCallId: 'call-1',
          payload: {'name': 'layout_propose', 'ok': true, ..._payload()},
        ),
      ];
      final calls = AiToolCallState.listFromParts(parts);
      expect(calls, hasLength(1));
      // ⚠ 后端 `push_tool_result` 是**白名单**：`layoutTemplates` 没被放行的话
      //   （或者这里读错了键）历史里就一张卡都没有，而当轮却是好的。
      expect(calls.single.layoutCandidates, hasLength(1));
      expect(calls.single.layoutCandidates.single.title, '三分法：旅人压左');
    });

    test('没有 spec / 空 regions / 不是 Map 的条目一律跳过（绝不抛）', () {
      final list = LayoutCandidate.listFromPayload({
        'layoutTemplates': [
          'not a map',
          {'title': '没有 spec'},
          {
            'title': '空框',
            'spec': {'width': 768, 'height': 1024, 'regions': []},
          },
          {
            'title': '',
            'spec': {
              'width': 768,
              'height': 1024,
              'regions': [
                {'id': 'r1', 'label': '甲', 'x': 0, 'y': 0, 'w': 100, 'h': 100},
              ],
            },
          },
        ],
      });
      expect(list, hasLength(1));
      // 没标题的给个兜底名字，而不是空白卡
      expect(list.first.title, '布局 1');
      expect(LayoutCandidate.listFromPayload(null), isEmpty);
      expect(
        LayoutCandidate.listFromPayload({'layoutTemplates': 'x'}),
        isEmpty,
      );
    });
  });

  group('LayoutResubmit：提交 → 读回', () {
    const spec = LayoutSpec(
      width: 768,
      height: 1024,
      brief: '雾夜废墟',
      camera: LayoutCamera.eye,
      regions: [
        LayoutRegion(
          id: 'r1',
          rect: LayoutRect(40, 300, 260, 640),
          kind: LayoutBoxKind.character,
          label: '提灯的旅人',
          surface: LayoutSurface.floor,
        ),
        LayoutRegion(
          id: 'r2',
          rect: LayoutRect(620, 120, 360, 760),
          kind: LayoutBoxKind.object,
          label: '教堂',
          relation: LayoutRelation.inFront,
          relationTo: 'r1',
        ),
      ],
    );

    test('toMessage 带标记 / 散文 / 带包装的 JSON', () {
      const resubmit = LayoutResubmit(spec: spec, note: '把教堂再压低一点');
      final text = resubmit.toMessage();
      expect(text.startsWith(LayoutResubmit.header), isTrue);
      expect(text, contains('把教堂再压低一点'));
      expect(text, contains('雾夜废墟'));
      expect(text, contains('```json'));
      expect(text, contains('"comfyhubLayout"'));
      // 用户 2026-11-03："强调这是用户的实际用意" —— 回传时要表明
      // 这是他自己定下的图，不是 AI 的建议稿。
      expect(text, contains('亲手定下'));
      expect(text, contains('不是建议稿'));
      // 散文是纯函数投影：即使模型读不懂 JSON，"谁在哪"也在正文里
      expect(text, contains('画面'));
      expect(text, contains('提灯的旅人'));
    });

    test('toMessage → tryParse 能原样读回（含关系与坐标）', () {
      const resubmit = LayoutResubmit(spec: spec, note: '把教堂再压低一点');
      final back = LayoutResubmit.tryParse(resubmit.toMessage());
      expect(back, isNotNull);
      expect(back!.note, '把教堂再压低一点');
      expect(back.spec.toJson(), spec.toJson());
    });

    test('默认要求不会被当成用户备注再回显一遍', () {
      const resubmit = LayoutResubmit(spec: spec);
      final back = LayoutResubmit.tryParse(resubmit.toMessage());
      expect(back, isNotNull);
      expect(back!.note, isEmpty);
    });

    test('普通文本 / 坏 JSON / 缺 comfyhubLayout → null', () {
      expect(LayoutResubmit.tryParse('帮我写一段提示词'), isNull);
      expect(
        LayoutResubmit.tryParse('${LayoutResubmit.header}坏的\n```json\n{'),
        isNull,
      );
      expect(
        LayoutResubmit.tryParse(
          '${LayoutResubmit.header}x\n```json\n{"other": {}}\n```',
        ),
        isNull,
      );
    });
  });

  group('控件', () {
    testWidgets('模板卡在 320 宽下不溢出，点一条回调那一条', (tester) async {
      tester.view.physicalSize = const Size(320, 640);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final candidates = LayoutCandidate.listFromPayload(_payload());
      LayoutCandidate? opened;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: LayoutTemplatesCard(
              candidates: candidates,
              onOpen: (c) => opened = c,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('2D 布局模板（1）'), findsOneWidget);
      expect(tester.takeException(), isNull);

      await tester.tap(find.text('三分法：旅人压左'));
      await tester.pumpAndSettle();
      expect(opened?.title, '三分法：旅人压左');
    });

    testWidgets('两个同名模板也不会因重复 Key 崩', (tester) async {
      tester.view.physicalSize = const Size(320, 640);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final candidate = LayoutCandidate.listFromPayload(_payload()).first;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: LayoutTemplatesCard(
              // 模型完全可能给两个同名模板（标题是它自己写的）
              candidates: [candidate, candidate],
              onOpen: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('2D 布局模板（2）'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('已提交布局卡在 320 宽下不溢出', (tester) async {
      tester.view.physicalSize = const Size(320, 640);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      const spec = LayoutSpec(
        width: 768,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'r1',
            rect: LayoutRect(40, 300, 260, 640),
            label: '提灯的旅人',
          ),
        ],
      );
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: LayoutResubmitCard(
              resubmit: LayoutResubmit(spec: spec, note: '压低教堂'),
              onEdit: _noop,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('已提交的 2D 布局'), findsOneWidget);
      expect(find.text('再次修改'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('画布带 AI 草图进来：框已经在了，并有提交入口', (tester) async {
      tester.view.physicalSize = const Size(1440, 900);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      const spec = LayoutSpec(
        width: 768,
        height: 1024,
        brief: '雾夜废墟',
        regions: [
          LayoutRegion(
            id: 'r1',
            rect: LayoutRect(40, 300, 260, 640),
            label: '提灯的旅人',
          ),
        ],
      );
      await tester.pumpWidget(
        const MaterialApp(
          home: LayoutCanvasPage(
            initialSpec: spec,
            initialName: '三分法：旅人压左',
            fromWorkspace: true,
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('调整布局'), findsOneWidget);
      expect(find.text('正在修改 三分法：旅人压左'), findsOneWidget);
      expect(find.text('提交回工作台'), findsOneWidget);
      // 草图灌进画布了：面板里数得出 1 个框，场景描述也在
      expect(find.text('框（1）'), findsOneWidget);
      expect(find.text('雾夜废墟'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });
}

void _noop() {}
