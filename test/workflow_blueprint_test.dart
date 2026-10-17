// 工作流蓝图的回归：模型 / 布局 / 剔除 / 编辑 / 页面接线。
//
// 这里钉的都是**删了以后出错还能不能被抓住**的那些事实：
//   · 界面格式用文件里的真实坐标，API 格式的布局必须**确定性**且分层正确；
//   · 画布只画视口内的东西（上千节点时不剔除就是卡死，而"卡"不会报错）；
//   · 编辑只碰那一个字段 —— 前端 only 节点、坐标、`widgets_values_named` 一个都不能丢；
//   · 改完参数要**就地复检**（把改过的原文再交给后端算一次）。
//
// 不写 golden、不断言像素（仓库 0 处 golden，见 AGENTS 第 1 节）。

import 'dart:convert';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/workflow_blueprint.dart';
import 'package:viewer/widgets/workflow_blueprint_page.dart';
import 'package:viewer/widgets/workflow_blueprint_painter.dart';
import 'package:viewer/widgets/workflow_viewer.dart';

// ---------------------------------------------------------------------------
//  夹具
// ---------------------------------------------------------------------------

/// 一份**界面格式**工作流原文：3 个节点 + 2 条线 + 1 个 Note（前端 only）+ 1 个分组。
const _uiRaw = '''
{
  "version": 0.4,
  "nodes": [
    {"id": 1, "type": "CheckpointLoaderSimple", "pos": [0, 0], "size": [270, 98], "mode": 0,
     "inputs": [],
     "outputs": [{"name": "MODEL", "type": "MODEL", "links": [11]}],
     "widgets_values": ["krea2.safetensors"]},
    {"id": 2, "type": "KSampler", "pos": [400, 0], "size": [300, 260], "mode": 4,
     "inputs": [{"name": "model", "type": "MODEL", "link": 11},
                {"name": "seed", "type": "INT", "link": null, "widget": {"name": "seed"}}],
     "outputs": [{"name": "LATENT", "type": "LATENT", "links": [12]}],
     "widgets_values": [42, "randomize"],
     "widgets_values_named": {"seed": 42, "control_after_generate": "randomize"}},
    {"id": 3, "type": "SaveImage", "pos": [800, 0], "size": [270, 270], "mode": 0,
     "inputs": [{"name": "images", "type": "IMAGE", "link": 12}],
     "outputs": [{"name": "images", "type": "IMAGE", "links": []}],
     "widgets_values": ["test"]},
    {"id": 4, "type": "Note", "pos": [-400, 0], "size": [300, 200], "mode": 0,
     "inputs": [], "outputs": [], "widgets_values": ["备注"]}
  ],
  "links": [[11, 1, 0, 2, 0, "MODEL"], [12, 2, 0, 3, 0, "LATENT"]],
  "groups": [{"title": "主链", "bounding": [-100, -60, 1300, 400], "color": "#3f789e"}]
}
''';

/// 后端会回的那份蓝图（这里手写：模型层只认这个形状，与后端 `to_json` 逐字对应）。
Map<String, dynamic> _blueprintJson({bool api = false}) => api
    ? {
        'format': 'api',
        'objectInfo': false,
        'counts': {'nodes': 3, 'edges': 2, 'groups': 0, 'errors': 0, 'warnings': 1, 'infos': 0},
        'nodes': [
          {
            'id': '1',
            'class': 'CheckpointLoaderSimple',
            'title': 'CheckpointLoaderSimple',
            'pos': null,
            'size': null,
            'mode': 0,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [],
            'outputs': [
              {'name': '输出 0', 'type': 'MODEL', 'link': null, 'links': [], 'widget': false}
            ],
            'widgets': [],
          },
          {
            'id': '2',
            'class': 'KSampler',
            'title': 'KSampler',
            'pos': null,
            'size': null,
            'mode': 0,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [
              {'name': 'model', 'type': 'MODEL', 'link': null, 'links': [], 'widget': false},
              {'name': 'latent_image', 'type': 'LATENT', 'link': null, 'links': [], 'widget': false}
            ],
            'outputs': [
              {'name': '输出 0', 'type': 'LATENT', 'link': null, 'links': [], 'widget': false}
            ],
            'widgets': [],
          },
          {
            'id': '3',
            'class': 'EmptyLatentImage',
            'title': 'EmptyLatentImage',
            'pos': null,
            'size': null,
            'mode': 0,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [],
            'outputs': [
              {'name': '输出 0', 'type': 'LATENT', 'link': null, 'links': [], 'widget': false}
            ],
            'widgets': [],
          },
        ],
        'edges': [
          {'id': null, 'from': {'node': '1', 'slot': 0}, 'to': {'node': '2', 'slot': 0}, 'type': null, 'implicit': true},
          {'id': null, 'from': {'node': '3', 'slot': 0}, 'to': {'node': '2', 'slot': 1}, 'type': null, 'implicit': true},
        ],
        'groups': [],
        'issues': [],
        'warnings': ['API 格式节点图**没有保存画布坐标**：界面上的位置是自动排出来的'],
      }
    : {
        'format': 'ui',
        'objectInfo': true,
        'counts': {'nodes': 4, 'edges': 2, 'groups': 1, 'errors': 1, 'warnings': 1, 'infos': 0},
        'nodes': [
          {
            'id': '1',
            'class': 'CheckpointLoaderSimple',
            'title': 'CheckpointLoaderSimple',
            'pos': [0, 0],
            'size': [270, 98],
            'mode': 0,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [],
            'outputs': [
              {'name': 'MODEL', 'type': 'MODEL', 'link': null, 'links': [11], 'widget': false}
            ],
            'widgets': [
              {'index': 0, 'name': 'ckpt_name', 'value': 'krea2.safetensors'}
            ],
          },
          {
            'id': '2',
            'class': 'KSampler',
            'title': 'KSampler',
            'pos': [400, 0],
            'size': [300, 260],
            'mode': 4,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [
              {'name': 'model', 'type': 'MODEL', 'link': 11, 'links': [], 'widget': false},
              {'name': 'seed', 'type': 'INT', 'link': null, 'links': [], 'widget': true, 'value': 42}
            ],
            'outputs': [
              {'name': 'LATENT', 'type': 'LATENT', 'link': null, 'links': [12], 'widget': false}
            ],
            'widgets': [
              {'index': 0, 'name': 'seed', 'value': 42},
              {'index': 1, 'name': null, 'value': 'randomize'}
            ],
          },
          {
            'id': '3',
            'class': 'SaveImage',
            'title': 'SaveImage',
            'pos': [800, 0],
            'size': [270, 270],
            'mode': 0,
            'kind': 'normal',
            'frontendOnly': false,
            'inputs': [
              {'name': 'images', 'type': 'IMAGE', 'link': 12, 'links': [], 'widget': false}
            ],
            'outputs': [
              {'name': 'images', 'type': 'IMAGE', 'link': null, 'links': [], 'widget': false}
            ],
            'widgets': [],
          },
          {
            'id': '4',
            'class': 'Note',
            'title': 'Note',
            'pos': [-400, 0],
            'size': [300, 200],
            'mode': 0,
            'kind': 'note',
            'frontendOnly': true,
            'inputs': [],
            'outputs': [],
            'widgets': [{'index': 0, 'name': null, 'value': '备注'}],
          },
        ],
        'edges': [
          {'id': 11, 'from': {'node': '1', 'slot': 0}, 'to': {'node': '2', 'slot': 0}, 'type': 'MODEL', 'implicit': false},
          {'id': 12, 'from': {'node': '2', 'slot': 0}, 'to': {'node': '3', 'slot': 0}, 'type': 'LATENT', 'implicit': false},
        ],
        'groups': [
          {'title': '主链', 'bounding': [-100, -60, 1300, 400], 'color': '#3f789e'}
        ],
        'issues': [
          {
            'code': 'CONVERT_GAP',
            'severity': 'error',
            'title': '按提交链转换这份图会留下缺口',
            'detail': '空着的必填输入：2.seed (INT)。',
            'fix': '按提示把这些输入接上',
            'nodes': ['2'],
            'links': [12],
            'slots': ['2.seed'],
          },
          {
            'code': 'BROADCAST_STUB',
            'severity': 'warn',
            'title': '有 1 个广播节点在本机被注册成「零输出」的桩',
            'detail': '4 Anything Everywhere',
            'fix': null,
            'nodes': ['4'],
            'links': <int>[],
            'slots': <String>[],
          },
        ],
        'warnings': [],
      };

WorkflowBlueprint _blueprint({bool api = false}) =>
    WorkflowBlueprint.fromJson(_blueprintJson(api: api));

// ---------------------------------------------------------------------------
//  模型
// ---------------------------------------------------------------------------

void main() {
  group('模型', () {
    test('界面格式：节点 / 连线 / 分组 / 问题都读得出来', () {
      final blueprint = _blueprint();
      expect(blueprint.format, 'ui');
      expect(blueprint.nodes.length, 4);
      expect(blueprint.edges.length, 2);
      expect(blueprint.groups.single.title, '主链');
      expect(blueprint.errorCount, 1);
      expect(blueprint.warnCount, 1);

      final note = blueprint.nodeById('4')!;
      expect(note.kind, 'note');
      expect(note.frontendOnly, isTrue);
      expect(note.x, -400);
      expect(note.width, 300);

      final sampler = blueprint.nodeById('2')!;
      expect(sampler.bypassed, isTrue, reason: 'mode=4 就是旁路');
      expect(sampler.widgets.length, 2);
      expect(sampler.widgets[0].label, 'seed');
      // 没有名字的那一位**不许猜**：如实显示"参数 #1"
      expect(sampler.widgets[1].name, isNull);
      expect(sampler.widgets[1].label, '参数 #1');

      // 插口：空着的是"真插口"，控件不算
      final seed = sampler.inputs.firstWhere((port) => port.name == 'seed');
      expect(seed.widget, isTrue);
      expect(seed.isOpenSocket, isFalse);
    });

    test('问题能定位到节点与插口', () {
      final blueprint = _blueprint();
      expect(blueprint.severityForNode('2'), 'error');
      expect(blueprint.severityForNode('4'), 'warn');
      expect(blueprint.severityForNode('1'), 'none');
      expect(blueprint.issuesForSlot('2', 'seed').single.code, 'CONVERT_GAP');
    });
  });

  // -------------------------------------------------------------------------
  //  布局
  // -------------------------------------------------------------------------
  group('布局', () {
    test('界面格式用文件里的真实坐标', () {
      final blueprint = _blueprint();
      final layout = BlueprintLayout.of(blueprint);
      expect(layout.boxOf('1'), const Rect.fromLTWH(0, 0, 270, 98));
      expect(layout.boxOf('4'), const Rect.fromLTWH(-400, 0, 300, 200));
      expect(layout.bounds.left, -400);
      expect(layout.bounds.right, 1070);
    });

    test('API 格式分层排：上游在左边的层，且两次结果完全相同', () {
      final blueprint = _blueprint(api: true);
      final first = BlueprintLayout.of(blueprint);
      final second = BlueprintLayout.of(blueprint);
      // 确定性：同一份输入两次得到相同坐标（界面上写着"位置是排出来的"，那就必须稳定）
      for (final node in blueprint.nodes) {
        expect(first.boxOf(node.id), second.boxOf(node.id), reason: node.id);
      }
      final loader = first.boxOf('1')!;
      final sampler = first.boxOf('2')!;
      final latent = first.boxOf('3')!;
      expect(loader.left, lessThan(sampler.left), reason: 'CheckpointLoaderSimple 在 KSampler 左边');
      expect(latent.left, lessThan(sampler.left));
      // 同层的两个上游必须**错开**排（同一列里上下分开），不能叠在一起
      expect(loader.left, latent.left);
      expect(loader.top, isNot(latent.top));
    });
  });

  // -------------------------------------------------------------------------
  //  画布剔除
  // -------------------------------------------------------------------------
  group('画布', () {
    test('只画视口内的节点，视口外的连线也要剔掉', () {
      final blueprint = _blueprint();
      final layout = BlueprintLayout.of(blueprint);
      var stats = const BlueprintPaintStats();
      final painter = BlueprintPainter(
        BlueprintPainterSpec(
          blueprint: blueprint,
          layout: layout,
          // 只看得见 1 号节点那一小块
          viewport: const Rect.fromLTWH(-20, -20, 320, 140),
          scale: 1,
        ),
        onPainted: (value) => stats = value,
      );
      final recorder = ui.PictureRecorder();
      painter.paint(Canvas(recorder), const Size(340, 160));
      recorder.endRecording();

      expect(stats.drawnNodes, 1, reason: '视口里只有 1 号');
      expect(stats.culledNodes, 3);
      expect(stats.drawnEdges, lessThan(blueprint.edges.length), reason: '视口外的线不许画');
    });

    test('缩放很小时不画正文（LOD），但节点框仍然要画', () {
      final blueprint = _blueprint();
      final layout = BlueprintLayout.of(blueprint);
      var stats = const BlueprintPaintStats();
      final painter = BlueprintPainter(
        BlueprintPainterSpec(
          blueprint: blueprint,
          layout: layout,
          viewport: layout.bounds.inflate(50),
          scale: 0.1,
        ),
        onPainted: (value) => stats = value,
      );
      final recorder = ui.PictureRecorder();
      // 缩小到 0.1 时画布只有整张图的 1/10 大 —— 不该崩、也不该把节点全丢掉
      painter.paint(Canvas(recorder), const Size(400, 200));
      recorder.endRecording();
      expect(stats.drawnNodes, blueprint.nodes.length);
      expect(stats.culledNodes, 0);
    });

    test('视口矩形换算：缩放平移之后要跟着变', () {
      final transform = Matrix4.identity()
        ..translateByDouble(-100, -50, 0, 1)
        ..scaleByDouble(2, 2, 1, 1);
      final rect = visibleGraphRect(transform, const Size(400, 300), Offset.zero);
      expect(rect.left, closeTo(50, 0.01));
      expect(rect.top, closeTo(25, 0.01));
      expect(rect.width, closeTo(200, 0.01));
      // origin 是"图坐标 → 画布坐标"，所以要减回去
      final shifted = visibleGraphRect(transform, const Size(400, 300), const Offset(10, 10));
      expect(shifted.left, closeTo(40, 0.01));
    });

    test('命中测试取最上面那个', () {
      final layout = BlueprintLayout.of(_blueprint());
      expect(hitTestNode(layout, const Offset(10, 10)), '1');
      expect(hitTestNode(layout, const Offset(-390, 10)), '4');
      expect(hitTestNode(layout, const Offset(5000, 5000)), isNull);
    });
  });

  // -------------------------------------------------------------------------
  //  编辑（按 id 改原文）
  // -------------------------------------------------------------------------
  group('编辑', () {
    test('改控件值：位置那份与带名字那份一起改', () {
      final editor = WorkflowEditor.parse(_uiRaw)!;
      expect(editor.editable, isTrue);
      expect(editor.setWidgetValue('2', 0, 777), isTrue);
      final json = jsonDecode(editor.toJson()) as Map<String, dynamic>;
      final node = (json['nodes'] as List).firstWhere((n) => n['id'] == 2) as Map;
      expect((node['widgets_values'] as List)[0], 777);
      // ⚠ 新版前端存的那一份**优先**：只改位置那份会让"界面看着改了、转出来还是旧值"
      expect((node['widgets_values_named'] as Map)['seed'], 777);
    });

    test('改状态 / 标题 / 坐标，且只碰那一个字段', () {
      final editor = WorkflowEditor.parse(_uiRaw)!;
      expect(editor.setNodeMode('2', kNodeModeNormal), isTrue);
      expect(editor.setNodeTitle('2', '采样器'), isTrue);
      expect(editor.setNodePosition('2', 123.5, -9.0), isTrue);

      final json = jsonDecode(editor.toJson()) as Map<String, dynamic>;
      final node = (json['nodes'] as List).firstWhere((n) => n['id'] == 2) as Map;
      expect(node['mode'], 0);
      expect(node['title'], '采样器');
      expect(node['pos'], [123.5, -9.0]);
      // 前端 only 的 Note 与别的节点一个字节都没动
      final note = (json['nodes'] as List).firstWhere((n) => n['id'] == 4) as Map;
      expect(note['type'], 'Note');
      expect(note['widgets_values'], ['备注']);
      expect((json['groups'] as List).length, 1);
      expect((json['links'] as List).length, 2);
    });

    test('不存在的节点 / 同样的值：如实回 false（不算改过）', () {
      final editor = WorkflowEditor.parse(_uiRaw)!;
      expect(editor.setWidgetValue('99', 0, 1), isFalse);
      expect(editor.setNodeMode('2', kNodeModeBypassed), isFalse, reason: '本来就是旁路');
      expect(editor.setNodeTitle('2', ''), isFalse, reason: '本来就没有自定义标题');
    });

    test('API 格式不能这样改（参数键名不在这份 JSON 里）', () {
      final editor = WorkflowEditor.parse('{"1":{"class_type":"KSampler","inputs":{"seed":1}}}')!;
      expect(editor.editable, isFalse);
      expect(editor.setWidgetValue('1', 0, 2), isFalse);
    });

    test('不是 JSON 就回 null，界面据此禁用编辑', () {
      expect(WorkflowEditor.parse('这不是 JSON'), isNull);
      expect(WorkflowEditor.parse('[1,2,3]'), isNull);
    });
  });

  // -------------------------------------------------------------------------
  //  页面
  // -------------------------------------------------------------------------
  group('页面', () {
    testWidgets('算出蓝图并显示节点/连线/分组数，改参数会拿改过的原文复检', (tester) async {
      tester.view.physicalSize = const Size(1600, 1100);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final seen = <String>[];
      Future<Map<String, dynamic>> diagnose(String raw) async {
        seen.add(raw);
        return _blueprintJson();
      }

      await tester.pumpWidget(MaterialApp(
        home: WorkflowBlueprintPage(title: '测试', raw: _uiRaw, diagnose: diagnose),
      ));
      await tester.pumpAndSettle();

      expect(seen.length, 1);
      expect(find.textContaining('4 节点'), findsOneWidget);
      expect(find.textContaining('2 连线'), findsOneWidget);
      expect(find.textContaining('1 分组'), findsOneWidget);

      // 体检页：错误条目 + 修法都在
      await tester.tap(find.textContaining('体检 2'));
      await tester.pumpAndSettle();
      expect(find.text('按提交链转换这份图会留下缺口'), findsOneWidget);
      expect(find.textContaining('修法：'), findsWidgets);

      // 从结论跳到节点
      await tester.tap(find.widgetWithText(ActionChip, '#2'));
      await tester.pumpAndSettle();
      expect(find.text('KSampler · 节点 #2 · 旁路'), findsOneWidget);

      // 编辑：改 seed（第一个参数）
      await tester.tap(find.byIcon(Icons.edit_outlined));
      await tester.pumpAndSettle();
      final field = find.byType(TextFormField).at(1); // 0 是标题
      await tester.enterText(field, '777');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pumpAndSettle();

      expect(seen.length, greaterThanOrEqualTo(2), reason: '改完要就地复检');
      expect(seen.last, contains('"seed": 777'), reason: '交回去的是改过的那份原文');
      // 原文里别的东西一个都没动
      expect(seen.last, contains('"type": "Note"'));
    });

    testWidgets('「看蓝图」只在给了蓝图取数时才出现', (tester) async {
      // 这条钉的是"蓝图是可选的"：`WorkflowButton` 不传 blueprint 时，
      // 弹窗与以前**逐字相同**（详情页与老用例因此一条都不用改）
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: Center(child: WorkflowButton(title: '测试', load: () async => _uiRaw)),
        ),
      ));
      await tester.tap(find.text('查看工作流'));
      await tester.pumpAndSettle();
      expect(find.text('看蓝图'), findsNothing);
      expect(find.text('复制'), findsOneWidget, reason: '老入口一个都不许少');
      await tester.tap(find.text('关闭'));
      await tester.pumpAndSettle();

      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: Center(
            child: WorkflowButton(
              title: '测试',
              load: () async => _uiRaw,
              blueprint: (raw) async => _blueprintJson(),
            ),
          ),
        ),
      ));
      await tester.tap(find.text('查看工作流'));
      await tester.pumpAndSettle();
      expect(find.text('看蓝图'), findsOneWidget);
    });
  });
}
