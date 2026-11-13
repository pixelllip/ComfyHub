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

import 'package:material_ui/material_ui.dart';
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
        'counts': {
          'nodes': 3,
          'edges': 2,
          'groups': 0,
          'errors': 0,
          'warnings': 1,
          'infos': 0,
        },
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
              {
                'name': '输出 0',
                'type': 'MODEL',
                'link': null,
                'links': [],
                'widget': false,
              },
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
              {
                'name': 'model',
                'type': 'MODEL',
                'link': null,
                'links': [],
                'widget': false,
              },
              {
                'name': 'latent_image',
                'type': 'LATENT',
                'link': null,
                'links': [],
                'widget': false,
              },
            ],
            'outputs': [
              {
                'name': '输出 0',
                'type': 'LATENT',
                'link': null,
                'links': [],
                'widget': false,
              },
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
              {
                'name': '输出 0',
                'type': 'LATENT',
                'link': null,
                'links': [],
                'widget': false,
              },
            ],
            'widgets': [],
          },
        ],
        'edges': [
          {
            'id': null,
            'from': {'node': '1', 'slot': 0},
            'to': {'node': '2', 'slot': 0},
            'type': null,
            'implicit': true,
          },
          {
            'id': null,
            'from': {'node': '3', 'slot': 0},
            'to': {'node': '2', 'slot': 1},
            'type': null,
            'implicit': true,
          },
        ],
        'groups': [],
        'issues': [],
        'warnings': ['API 格式节点图**没有保存画布坐标**：界面上的位置是自动排出来的'],
      }
    : {
        'format': 'ui',
        'objectInfo': true,
        'counts': {
          'nodes': 4,
          'edges': 2,
          'groups': 1,
          'errors': 1,
          'warnings': 1,
          'infos': 0,
        },
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
              {
                'name': 'MODEL',
                'type': 'MODEL',
                'link': null,
                'links': [11],
                'widget': false,
              },
            ],
            'widgets': [
              {'index': 0, 'name': 'ckpt_name', 'value': 'krea2.safetensors'},
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
              {
                'name': 'model',
                'type': 'MODEL',
                'link': 11,
                'links': [],
                'widget': false,
              },
              {
                'name': 'seed',
                'type': 'INT',
                'link': null,
                'links': [],
                'widget': true,
                'value': 42,
              },
            ],
            'outputs': [
              {
                'name': 'LATENT',
                'type': 'LATENT',
                'link': null,
                'links': [12],
                'widget': false,
              },
            ],
            'widgets': [
              {'index': 0, 'name': 'seed', 'value': 42},
              {'index': 1, 'name': null, 'value': 'randomize'},
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
              {
                'name': 'images',
                'type': 'IMAGE',
                'link': 12,
                'links': [],
                'widget': false,
              },
            ],
            'outputs': [
              {
                'name': 'images',
                'type': 'IMAGE',
                'link': null,
                'links': [],
                'widget': false,
              },
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
            'widgets': [
              {'index': 0, 'name': null, 'value': '备注'},
            ],
          },
        ],
        'edges': [
          {
            'id': 11,
            'from': {'node': '1', 'slot': 0},
            'to': {'node': '2', 'slot': 0},
            'type': 'MODEL',
            'implicit': false,
          },
          {
            'id': 12,
            'from': {'node': '2', 'slot': 0},
            'to': {'node': '3', 'slot': 0},
            'type': 'LATENT',
            'implicit': false,
          },
        ],
        'groups': [
          {
            'title': '主链',
            'bounding': [-100, -60, 1300, 400],
            'color': '#3f789e',
          },
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

/// 把当前**渲染出来**的文字全收集起来（含 `RichText`）。
///
/// 为什么要它：`find.text` 只认 `Text.data`，而 markdown 渲染出来的是 `RichText` ——
/// 用它才能断言"标记没有原样露出来"（否则断言会假绿）。
String _renderedText(WidgetTester tester) {
  final buffer = StringBuffer();
  for (final widget in tester.allWidgets) {
    if (widget is RichText) {
      buffer.writeln(widget.text.toPlainText());
    } else if (widget is Text && widget.data != null) {
      buffer.writeln(widget.data);
    }
  }
  return buffer.toString();
}

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
      expect(
        loader.left,
        lessThan(sampler.left),
        reason: 'CheckpointLoaderSimple 在 KSampler 左边',
      );
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
      expect(
        stats.drawnEdges,
        lessThan(blueprint.edges.length),
        reason: '视口外的线不许画',
      );
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
      final rect = visibleGraphRect(
        transform,
        const Size(400, 300),
        Offset.zero,
      );
      expect(rect.left, closeTo(50, 0.01));
      expect(rect.top, closeTo(25, 0.01));
      expect(rect.width, closeTo(200, 0.01));
      // origin 是"图坐标 → 画布坐标"，所以要减回去
      final shifted = visibleGraphRect(
        transform,
        const Size(400, 300),
        const Offset(10, 10),
      );
      expect(shifted.left, closeTo(40, 0.01));
    });

    test('命中测试取最上面那个', () {
      final layout = BlueprintLayout.of(_blueprint());
      expect(hitTestNode(layout, const Offset(10, 10)), '1');
      expect(hitTestNode(layout, const Offset(-390, 10)), '4');
      expect(hitTestNode(layout, const Offset(5000, 5000)), isNull);
    });

    test('参数行的几何只有一份：画出来的矩形，点它的中心必须命中它自己', () {
      // 在蓝图上直接修改参数。命中与绘制**共用同一个算式**，
      // 否则表现是"点得到的地方和画出来的地方不重合"，而且一个错都不报。
      final blueprint = _blueprint();
      final layout = BlueprintLayout.of(blueprint);
      var checked = 0;
      for (final node in blueprint.nodes) {
        final box = layout.boxOf(node.id);
        if (box == null) continue;
        for (final widget in node.widgets) {
          final row = blueprintWidgetRowRect(node, box, widget.index);
          // 超出节点底边的行根本不画（画笔里 `break`），也就不该被点到
          if (row.top > box.bottom - 10) continue;
          expect(
            hitTestWidgetRow(node, box, row.center),
            widget.index,
            reason: '节点 ${node.id} 第 ${widget.index} 行的中心必须命中它自己',
          );
          checked++;
        }
      }
      expect(checked, greaterThan(0), reason: '夹具里至少要有一行画得出来的参数');

      // 标题条上点一下：**不该**命中任何参数行（那是"选中节点"的手势）
      final node = blueprint.nodes.firstWhere((n) => n.widgets.isNotEmpty);
      final box = layout.boxOf(node.id)!;
      expect(
        hitTestWidgetRow(node, box, Offset(box.left + 4, box.top + 4)),
        isNull,
      );
      // 节点外面更不该命中
      expect(
        hitTestWidgetRow(node, box, Offset(box.left, box.bottom + 40)),
        isNull,
      );
    });

    test('参数行是"名字 | 值"两列：不重叠、不留缝、宽度有上下限', () {
      // 显示形式为：text（不变） | xxx...（可变）。
      final blueprint = _blueprint();
      final layout = BlueprintLayout.of(blueprint);
      var checked = 0;
      for (final node in blueprint.nodes) {
        final box = layout.boxOf(node.id);
        if (box == null) continue;
        for (final row in blueprintWidgetRows(node, box)) {
          // 多行文本框（Note / 长提示词）本来就没有"名字列"，
          // 它该占整行 —— 这里只钉单行参数的"名字 | 值"两列。
          if (!row.visible || row.multiline) continue;
          expect(
            row.label.right,
            closeTo(row.value.left, 0.001),
            reason: '两列必须紧挨着，不留缝',
          );
          expect(row.label.left, closeTo(row.rect.left, 0.001));
          expect(row.value.right, closeTo(row.rect.right, 0.001));
          expect(
            row.label.width,
            greaterThanOrEqualTo(28),
            reason: '名字太窄就认不出是什么参数',
          );
          expect(row.label.width, lessThanOrEqualTo(96), reason: '名字太宽值就没地方了');
          expect(row.value.width, greaterThan(0), reason: '值那一列必须真的存在');
          expect(
            row.rect.height,
            greaterThanOrEqualTo(kBlueprintWidgetBarHeight),
          );
          checked++;
        }
      }
      expect(checked, greaterThan(0), reason: '夹具里至少要有一行画得出来的参数');
    });

    test('长文本 / Note 按多行撑高，后面的参数与节点底边都不被顶穿', () {
      // 需要多行显示的内容使用多行显示。
      // 以前所有参数都是 11px 高的一行，一段提示词只露前 24 个字。
      final node = BlueprintNode(
        id: 'text',
        className: 'PrimitiveStringMultiline',
        title: 'Text String',
        widgets: const [
          BlueprintWidget(
            index: 0,
            name: 'value',
            value: '第一行提示词\n第二行提示词\n第三行提示词',
          ),
          BlueprintWidget(index: 1, name: 'seed', value: 42),
        ],
      );
      final box = const Rect.fromLTWH(0, 0, 320, 260);
      final rows = blueprintWidgetRows(node, box);

      expect(rows.length, 2);
      expect(rows[0].multiline, isTrue);
      expect(
        rows[0].rect.height,
        greaterThan(kBlueprintWidgetRowStep),
        reason: '多行文本必须真的占多行，而不是压成一行',
      );
      expect(rows[0].lines, 3);
      // 后面的参数仍然要留出位置，不许被长文本顶到框外
      expect(rows[1].multiline, isFalse);
      expect(rows[1].rect.bottom, lessThanOrEqualTo(box.bottom));
      // 命中测试读的是同一份几何：点多行文本中间必须命中它自己
      expect(hitTestWidgetRow(node, box, rows[0].rect.center), 0);
      expect(hitTestWidgetRow(node, box, rows[1].rect.center), 1);
    });

    test('纯控件输入不占端口行：参数区不会被 20 个输入一路往下推', () {
      // TextGenerate 有 6 个真插口 + 一堆 widget 输入；旧算法按 inputs.length 算端口，
      // 直接吃掉 360px。ComfyUI 里这些控件显示成参数行，不画左侧圆点。
      final blueprint = _blueprint();
      final sampler = blueprint.nodeById('2')!;
      // 夹具里 KSampler 的 seed 是 widget 输入、没接线 ⇒ 只有 model 占一行
      expect(blueprintInputSocketCount(sampler), 1);
      expect(blueprintInputRowIndex(sampler, 0), 0);
      expect(blueprintInputRowIndex(sampler, 1), isNull);
      expect(blueprintPortRowCount(sampler), 1);
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
      final node =
          (json['nodes'] as List).firstWhere((n) => n['id'] == 2) as Map;
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
      final node =
          (json['nodes'] as List).firstWhere((n) => n['id'] == 2) as Map;
      expect(node['mode'], 0);
      expect(node['title'], '采样器');
      expect(node['pos'], [123.5, -9.0]);
      // 前端 only 的 Note 与别的节点一个字节都没动
      final note =
          (json['nodes'] as List).firstWhere((n) => n['id'] == 4) as Map;
      expect(note['type'], 'Note');
      expect(note['widgets_values'], ['备注']);
      expect((json['groups'] as List).length, 1);
      expect((json['links'] as List).length, 2);
    });

    test('不存在的节点 / 同样的值：如实回 false（不算改过）', () {
      final editor = WorkflowEditor.parse(_uiRaw)!;
      expect(editor.setWidgetValue('99', 0, 1), isFalse);
      expect(
        editor.setNodeMode('2', kNodeModeBypassed),
        isFalse,
        reason: '本来就是旁路',
      );
      expect(editor.setNodeTitle('2', ''), isFalse, reason: '本来就没有自定义标题');
    });

    test('API 格式不能这样改（参数键名不在这份 JSON 里）', () {
      final editor = WorkflowEditor.parse(
        '{"1":{"class_type":"KSampler","inputs":{"seed":1}}}',
      )!;
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
      Future<Map<String, dynamic>> diagnose(
        String raw, {
        bool diagnose = true,
      }) async {
        seen.add(raw);
        return _blueprintJson();
      }

      await tester.pumpWidget(
        MaterialApp(
          home: WorkflowBlueprintPage(
            title: '测试',
            raw: _uiRaw,
            diagnose: diagnose,
          ),
        ),
      );
      await tester.pumpAndSettle();

      // 打开时并发两条：结构快照（diagnose=false）+ 全量体检（diagnose=true）。
      expect(seen.length, 2);
      expect(find.textContaining('4 节点'), findsOneWidget);
      expect(find.textContaining('2 连线'), findsOneWidget);
      expect(find.textContaining('1 分组'), findsOneWidget);

      // 体检页：错误条目 + 修法都在
      await tester.tap(find.textContaining('体检 2'));
      await tester.pumpAndSettle();
      expect(find.text('按提交链转换这份图会留下缺口'), findsOneWidget);
      expect(find.textContaining('修法是'), findsWidgets);

      // 从结论跳到节点
      await tester.tap(find.widgetWithText(ActionChip, '#2'));
      await tester.pumpAndSettle();
      expect(find.text('KSampler · 节点 #2 · 旁路'), findsOneWidget);

      // 编辑：改 seed（第一个参数）—— 面板里每个参数是"名字 | 值"两列，
      // 值那一格是 `_PanelWidgetField` 里的 TextField，用它的稳定 key 定位。
      await tester.tap(find.byIcon(Icons.edit_outlined));
      await tester.pumpAndSettle();
      final field = find.byKey(const Key('blueprint-panel-widget-field')).first;
      await tester.enterText(field, '777');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pumpAndSettle();

      expect(seen.length, greaterThanOrEqualTo(2), reason: '改完要就地复检');
      expect(seen.last, contains('"seed": 777'), reason: '交回去的是改过的那份原文');
      // 原文里别的东西一个都没动
      expect(seen.last, contains('"type": "Note"'));
    });

    testWidgets('打开就把内容缩放对齐到画布中心 —— 不是跑到右上角', (tester) async {
      // 现象："蓝图打开后默认位于右上角，应缩放对齐窗口。"
      // 真因：适配用的是**整个 body** 的尺寸（含右侧 380 面板与上方横幅），
      // 比画布宽一截，算出来的平移把内容推到了右边。
      //
      // 断言是**精确**的：适配之后，图包围盒的中心在屏幕上必须正好落在视口中心
      // （数学上 `screen = p*fit + dx` 里 `dx` 恰好把 `bounds` 居中，与缩放无关）。
      tester.view.physicalSize = const Size(1200, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(
        MaterialApp(
          home: WorkflowBlueprintPage(
            title: '测试',
            raw: _uiRaw,
            diagnose: (raw, {bool diagnose = true}) async => _blueprintJson(),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final viewer = tester.widget<InteractiveViewer>(
        find.byType(InteractiveViewer),
      );
      final matrix = viewer.transformationController!.value;
      final viewport = tester.getSize(find.byType(InteractiveViewer));

      final blueprint = WorkflowBlueprint.fromJson(_blueprintJson());
      final bounds = BlueprintLayout.of(blueprint).bounds;
      // 图坐标 → 画布坐标（与页面里的 `origin` 同一套算法，padding = 40）
      const padding = 40.0;
      final origin = Offset(padding - bounds.left, padding - bounds.top);
      final onScreen = MatrixUtils.transformPoint(
        matrix,
        bounds.center + origin,
      );

      expect(
        (onScreen.dx - viewport.width / 2).abs(),
        lessThan(0.5),
        reason: '横向必须居中；偏到右边就是"跑到右上角"那个 bug',
      );
      expect(
        (onScreen.dy - viewport.height / 2).abs(),
        lessThan(0.5),
        reason: '纵向必须居中',
      );
      // 缩放要真的"适应窗口"（不是 1:1 原样铺开）
      final scale = blueprintScaleOf(matrix);
      expect(scale, lessThanOrEqualTo(1.0));
      expect(scale, greaterThan(0.0));
      // ⚠ 判据必须是 blueprintScaleOf，不是 `getMaxScaleOnAxis()`：
      // 后者把 Z=1 也算进去，缩小的时候永远回 1.0，LOD 会整个失效。
      final expectedFit = (viewport.width - 48) / bounds.width;
      expect(
        (scale - expectedFit).abs(),
        lessThan(0.001),
        reason: '按窗口宽度适应：$scale vs $expectedFit',
      );
    });

    testWidgets('面板能折叠：收起之后画布变宽，再展开回来', (tester) async {
      // 该内容不能折叠。
      tester.view.physicalSize = const Size(1200, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(
        MaterialApp(
          home: WorkflowBlueprintPage(
            title: '测试',
            raw: _uiRaw,
            diagnose: (raw, {bool diagnose = true}) async => _blueprintJson(),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final openWidth = tester.getSize(find.byType(InteractiveViewer)).width;
      expect(find.textContaining('节点 /'), findsOneWidget, reason: '前提：面板开着');

      await tester.tap(find.byKey(const Key('blueprint-panel-toggle')));
      await tester.pumpAndSettle();

      expect(find.textContaining('节点 /'), findsNothing, reason: '收起之后面板该整个消失');
      final closedWidth = tester.getSize(find.byType(InteractiveViewer)).width;
      expect(closedWidth, greaterThan(openWidth), reason: '收起面板的意义就是画布变宽');

      await tester.tap(find.byKey(const Key('blueprint-panel-toggle')));
      await tester.pumpAndSettle();
      expect(find.textContaining('节点 /'), findsOneWidget, reason: '要能再展开回来');
      expect(tester.getSize(find.byType(InteractiveViewer)).width, openWidth);
    });

    testWidgets('编辑模式下点画布上的参数行 = 就地改，不用去右侧面板', (tester) async {
      // 编辑时可在蓝图上直接操作，不需要在右侧侧边栏选择。
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      final seen = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          home: WorkflowBlueprintPage(
            title: '测试',
            raw: _uiRaw,
            diagnose: (raw, {bool diagnose = true}) async {
              seen.add(raw);
              return _blueprintJson();
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byIcon(Icons.edit_outlined));
      await tester.pumpAndSettle();

      final blueprint = WorkflowBlueprint.fromJson(_blueprintJson());
      final layout = BlueprintLayout.of(blueprint);
      final viewer = tester.widget<InteractiveViewer>(
        find.byType(InteractiveViewer),
      );
      final matrix = viewer.transformationController!.value;
      final canvasTopLeft = tester.getTopLeft(find.byType(InteractiveViewer));
      const padding = 40.0;
      final origin = Offset(
        padding - layout.bounds.left,
        padding - layout.bounds.top,
      );

      // 找一行"屏幕上画得出来、而且宽度够摆输入框"的参数（与生产判据同一个阈值）
      Object? original;
      Offset? target;
      for (final node in blueprint.nodes) {
        final box = layout.boxOf(node.id);
        if (box == null) continue;
        for (final widget in node.widgets) {
          final row = blueprintWidgetRowRect(node, box, widget.index);
          if (row.top > box.bottom - 10) continue;
          final centre = MatrixUtils.transformPoint(
            matrix,
            row.center + origin,
          );
          final width = row.width * blueprintScaleOf(matrix);
          if (width < 40) continue;
          original = widget.value;
          target = canvasTopLeft + centre;
          break;
        }
        if (target != null) break;
      }
      expect(target, isNotNull, reason: '夹具里至少要有一行能就地编辑的参数');

      await tester.tapAt(target!);
      await tester.pumpAndSettle();
      expect(
        find.byKey(const Key('blueprint-inline-widget-field')),
        findsOneWidget,
        reason: '点了参数行就该在**画布上**出现输入框',
      );

      // 数字按数字改，字符串按字符串改（类型从当前值推，见 coerceWidgetValue）
      final probe = original is num ? '424242' : 'blueprint-inline-probe';
      await tester.enterText(
        find.byKey(const Key('blueprint-inline-widget-field')),
        probe,
      );
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pumpAndSettle();

      expect(
        find.byKey(const Key('blueprint-inline-widget-field')),
        findsNothing,
        reason: '提交之后就地编辑器要收起来',
      );
      expect(seen.length, greaterThanOrEqualTo(2), reason: '改完要就地复检');
      expect(seen.last, contains(probe), reason: '复检用的是**改过之后**的原文');
    });

    testWidgets('体检里的 markdown 真的被渲染（不许露出反引号与井号）', (tester) async {
      // 现象：文案包含 Markdown 格式，但相关组件不支持渲染。
      // 后端那几段文案里带 `` `代码` ``、`**加粗**`，以及本仓库约定的 `##强调##`。
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(
        MaterialApp(
          home: WorkflowBlueprintPage(
            title: '测试',
            raw: _uiRaw,
            diagnose: (raw, {bool diagnose = true}) async {
              final json = _blueprintJson();
              final issues = (json['issues'] as List)
                  .cast<Map<String, dynamic>>();
              issues.first['detail'] =
                  '开 ##模拟用户行为## 就行（`comfy_load_workflow`，**默认开着**）';
              return json;
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.textContaining('体检 2'));
      await tester.pumpAndSettle();

      final text = _renderedText(tester);
      expect(text, contains('模拟用户行为'), reason: '内容本身要还在');
      expect(text, isNot(contains('##')), reason: '井号必须被渲染掉，不能原样露出来');
      expect(text, isNot(contains('`')), reason: '反引号必须被渲染掉');
      expect(text, isNot(contains('**')), reason: '星号必须被渲染掉');
    });

    testWidgets('「看蓝图」只在给了蓝图取数时才出现', (tester) async {
      // 这条钉的是"蓝图是可选的"：`WorkflowButton` 不传 blueprint 时，
      // 弹窗与以前**逐字相同**（详情页与老用例因此一条都不用改）
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Center(
              child: WorkflowButton(title: '测试', load: () async => _uiRaw),
            ),
          ),
        ),
      );
      await tester.tap(find.text('查看工作流'));
      await tester.pumpAndSettle();
      expect(find.text('看蓝图'), findsNothing);
      expect(find.text('复制'), findsOneWidget, reason: '老入口一个都不许少');
      await tester.tap(find.text('关闭'));
      await tester.pumpAndSettle();

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Center(
              child: WorkflowButton(
                title: '测试',
                load: () async => _uiRaw,
                blueprint: (raw, {bool diagnose = true}) async =>
                    _blueprintJson(),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('查看工作流'));
      await tester.pumpAndSettle();
      expect(find.text('看蓝图'), findsOneWidget);
    });
  });

  // ---------------------------------------------------------------------------
  //  配色
  //
  //  现象：蓝图对比度过高。
  //  真因：配色是**一整套浅色**写死在画笔里（节点底 #F3F5F7 + 近黑正文），
  //  而 App 默认深色主题 ⇒ 深底上压一片近白，对比约 17:1。
  //
  //  仓库不做 golden / 截图回归，所以"观感"只能**算**出来 —— 这几条就是那个量。
  // ---------------------------------------------------------------------------
  group('配色', () {
    /// 近白：老实现那个 `#F3F5F7` 就落在这里面。
    bool looksNearWhite(Color c) => relativeLuminance(c) > 0.7;

    test('深色主题下节点底不许是近白色（老实现就是这么刺眼的）', () {
      final dark = BlueprintPalette.dark;
      expect(
        looksNearWhite(dark.body),
        isFalse,
        reason: '深色主题的节点底一旦接近白色，大面积高对比就会刺眼（2026-10-18 的 bug）',
      );
      expect(
        relativeLuminance(dark.canvas) < relativeLuminance(dark.body),
        isTrue,
        reason: '节点底要比画布**亮一档**才分得出层（不许同色）',
      );
    });

    test('两套主题下"节点底 vs 画布底"都看得清、但不刺眼', () {
      for (final palette in [BlueprintPalette.dark, BlueprintPalette.light]) {
        final ratio = contrastRatio(palette.body, palette.canvas);
        expect(
          ratio,
          greaterThanOrEqualTo(1.1),
          reason: '太低就分不出节点边界（浅色主题曾经就是这样）',
        );
        expect(
          ratio,
          lessThanOrEqualTo(2.5),
          reason: '太大就是"一大片高对比色块"，正是用户说的刺眼',
        );
      }
    });

    test('两套主题下节点标题都读得清（正文级对比 ≥ 7:1）', () {
      for (final palette in [BlueprintPalette.dark, BlueprintPalette.light]) {
        expect(
          contrastRatio(palette.title, palette.body),
          greaterThanOrEqualTo(7.0),
        );
        expect(
          contrastRatio(palette.widgetText, palette.widgetBar),
          greaterThanOrEqualTo(4.5),
          reason: '参数行是小字，至少要 AA 的 4.5:1',
        );
      }
    });

    test('语义色只上强调，不铺满整块底', () {
      final dark = BlueprintPalette.dark;
      expect(dark.accentOf('note'), isNot(dark.body));
      expect(dark.accentOf('unknown'), isNot(dark.body));
      // 认不出来的 kind 回普通描边色 —— **不猜**
      expect(dark.accentOf('没见过的类'), dark.border);
      // 深浅两套都要给同名的 kind 留位置，否则某个主题下会静默退化成"没上色"
      expect(
        dark.accents.keys.toSet(),
        BlueprintPalette.light.accents.keys.toSet(),
      );
    });

    test('两套配色必须是稳定单例（shouldRepaint 靠身份比较）', () {
      expect(
        identical(BlueprintPalette.of(Brightness.dark), BlueprintPalette.dark),
        isTrue,
      );
      expect(
        identical(
          BlueprintPalette.of(Brightness.light),
          BlueprintPalette.light,
        ),
        isTrue,
      );
      expect(BlueprintPalette.dark.isDark, isTrue);
      expect(BlueprintPalette.light.isDark, isFalse);
    });

    test('插口 / 连线取 ComfyUI 的类型色：同类同色、异类异色', () {
      // 不同输入/输出节点与连线的配色取自 ComfyUI。
      final dark = BlueprintPalette.dark;
      expect(dark.typeColor('MODEL'), const Color(0xFFB39DDB));
      expect(dark.typeColor('CLIP'), const Color(0xFFFFD500));
      expect(dark.typeColor('IMAGE'), const Color(0xFF64B5F6));
      expect(dark.typeColor('LATENT'), const Color(0xFFFF9CF9));
      expect(dark.typeColor('STRING'), const Color(0xFF7CFF7C));
      // 大小写 / 空白不该影响判据（后端给的类型大小写并不总是统一）
      expect(dark.typeColor(' model '), dark.typeColor('MODEL'));
      // 四种最常见的类型必须互相区分开 —— 全画成灰色就回到了"所有线一个样"
      final common = {
        dark.typeColor('MODEL'),
        dark.typeColor('CLIP'),
        dark.typeColor('IMAGE'),
        dark.typeColor('LATENT'),
        dark.typeColor('CONDITIONING'),
      };
      expect(common.length, 5);
      // 认不出的类型按名字稳定散列，但又不能撞回灰色默认值
      expect(dark.typeColor('SEGS'), dark.typeColor('SEGS'));
      expect(dark.typeColor('SEGS'), isNot(dark.unknownType));
      expect(dark.typeColor(null), dark.unknownType);
      // 深浅两套都要把这套类型表带全，否则某个主题下会静默退化成随机色
      expect(
        dark.typeColors.keys.toSet(),
        BlueprintPalette.light.typeColors.keys.toSet(),
      );
    });

    test('标题条是中性的，不按节点类别整片染色（与 ComfyUI 一致）', () {
      final dark = BlueprintPalette.dark;
      expect(dark.header, isNot(dark.accentOf('note')));
      expect(dark.header, isNot(dark.accentOf('unknown')));
      expect(
        relativeLuminance(dark.header),
        greaterThan(relativeLuminance(dark.body)),
        reason: '标题条要比节点底亮一档才分得开',
      );
    });
  });
}
