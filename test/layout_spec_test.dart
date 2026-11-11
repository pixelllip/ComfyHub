import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/models/layout_spec.dart';
import 'package:viewer/pages/layout_canvas_page.dart';

void main() {
  group('专用投影：MOSS-Image 布局 JSON', () {
    test('整幅画布 → 四边各半幅', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [LayoutRegion(id: 'a', rect: LayoutRect(0, 0, 1000, 1000))],
      );
      expect(spec.mossBbox(const LayoutRect(0, 0, 1000, 1000)), [
        -512,
        -512,
        512,
        512,
      ]);
    });

    test('起点 floor、终点 ceil，且换成以中心为原点的像素', () {
      const spec = LayoutSpec(width: 768, height: 1024);
      // 与 nodes_moss_image.py 的 math.floor(...) - height // 2 手工核对过：
      //   x0 = 250*768//1000 - 384 = 192-384 = -192
      //   x1 = ceil(750*768/1000) - 384 = 576-384 = 192
      //   y0 = 0-512 = -512 ; y1 = ceil(250*1024/1000)-512 = 256-512 = -256
      expect(spec.mossBbox(const LayoutRect(250, 0, 500, 250)), [
        -512,
        -192,
        -256,
        192,
      ]);
    });

    test('整段 JSON 与节点输出同构（golden）', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        brief: '雾夜',
        regions: [
          LayoutRegion(
            id: 'a',
            rect: LayoutRect(250, 500, 250, 250),
            label: '旅人',
          ),
        ],
      );
      // ComfyUI 那边是 json.dumps(..., separators=(",", ":"))，即紧凑无空格、键序如下。
      // 这条断言的意义：我们的画布能产出**和节点逐字节相同**的文本，
      // 也就是说它可以直接当提示词喂给 MOSS-Image，坐标不会被二次解释。
      expect(
        spec.translate().mossJson,
        '{"summary":{"caption":"雾夜"},'
        '"items":[{"label":"object","caption":"旅人","bbox":[0,-256,256,0]}],'
        '"bbox_coord_space":"abs_centralized","bbox_format":"yxyx"}',
      );
    });

    test('角色 / 文字 / 留白三档的差别', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'c',
            rect: LayoutRect(0, 0, 400, 400),
            kind: LayoutBoxKind.character,
            label: '骑士',
          ),
          LayoutRegion(
            id: 't',
            rect: LayoutRect(0, 600, 900, 200),
            kind: LayoutBoxKind.text,
            label: '标题',
            text: 'MOSS',
          ),
          LayoutRegion(
            id: 'b',
            rect: LayoutRect(600, 0, 400, 400),
            kind: LayoutBoxKind.blank,
          ),
        ],
      );
      final items = spec.mossItems();
      // 留白不进 items —— 那套格式只认有内容的框
      expect(items.length, 2);
      expect(items[0]['label'], 'character');
      expect(items[1]['label'], 'text');
      expect(items[1]['text'], 'MOSS');
      // 物体/角色没有 text 字段，文字框才有
      expect(items[0].containsKey('text'), isFalse);
    });
  });

  group('通用投影：散文', () {
    test('位置被量化成方位词，不是百分比', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        brief: '雾夜废墟',
        regions: [
          LayoutRegion(id: 'a', rect: LayoutRect(0, 0, 300, 300), label: '教堂'),
        ],
      );
      final prose = spec.translate().prose;
      expect(prose, startsWith('1:1 方形构图。雾夜废墟。'));
      expect(prose, contains('画面左上'));
      expect(prose, contains('教堂'));
      expect(prose, isNot(contains('%')));
      // 同一格里两个框必须能被区分开：宽高各报几成，而不是只给"小块"
      expect(prose, contains('宽约三成、高约三成'));
      expect(prose, contains('贴左缘'));
    });

    test('两个同格的框不会得到一模一样的句子', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'a',
            rect: LayoutRect(80, 190, 180, 130),
            label: '镜子',
          ),
          LayoutRegion(
            id: 'b',
            rect: LayoutRect(0, 256, 340, 104),
            label: '桌子',
          ),
        ],
      );
      final prose = spec.translate().prose;
      expect(prose, contains('镜子'));
      expect(prose, contains('桌子'));
      // 两个框的方位词相同（都在左上），但宽高不同
      expect(prose, contains('宽约两成、高约一成半'));
      expect(prose, contains('宽约三成半、高约一成'));
    });

    test('留白只活在散文里', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'b',
            rect: LayoutRect(0, 0, 1000, 250),
            kind: LayoutBoxKind.blank,
          ),
        ],
      );
      expect(spec.translate().prose, contains('留白'));
      expect(spec.mossItems(), isEmpty);
    });

    test('同一份 spec 翻译两次逐字节相同', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        brief: '测试',
        regions: [
          LayoutRegion(id: 'a', rect: LayoutRect(10, 20, 300, 400), label: '甲'),
          LayoutRegion(
            id: 'b',
            rect: LayoutRect(500, 500, 200, 200),
            label: '乙',
          ),
        ],
      );
      final first = spec.translate();
      final second = spec.translate();
      expect(first.prose, second.prose);
      expect(first.mossJson, second.mossJson);
    });

    test('主>5 个区域时报损失（散文承载不了）', () {
      final spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          for (var i = 0; i < 7; i++)
            LayoutRegion(
              id: 'r$i',
              rect: const LayoutRect(0, 0, 100, 100),
              label: 'x',
            ),
        ],
      );
      final t = spec.translate();
      expect(t.warnings.join(), contains('5 个约束'));
    });

    test('大面积重叠只有"没说谁挡谁"才算损失', () {
      // 前后遮挡就该重叠：标了「挡在前面」是**表达纵深**，不是翻译损失。
      const ordered = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'a',
            rect: LayoutRect(100, 100, 600, 600),
            label: '钢琴演奏者',
            relation: LayoutRelation.inFront,
            relationTo: 'b',
          ),
          LayoutRegion(id: 'b', rect: LayoutRect(250, 150, 600, 600), label: '合唱团'),
        ],
      );
      expect(
        ordered.translate().warnings.join(),
        isNot(contains('没说谁挡谁')),
      );

      // 叠在一起却没有任何遮挡关系 → 散文确实说不清谁在前。
      const unexplained = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(id: 'a', rect: LayoutRect(100, 100, 600, 600), label: '甲'),
          LayoutRegion(id: 'b', rect: LayoutRect(250, 150, 600, 600), label: '乙'),
        ],
      );
      expect(
        unexplained.translate().warnings.join(),
        contains('没说谁挡谁'),
      );
    });

    test('比例名不重复叠"构图"，且名称不会因为写了描述就被丢掉', () {
      const spec = LayoutSpec(
        width: 768,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'a',
            rect: LayoutRect(0, 0, 300, 300),
            label: '旅人',
            caption: '提着灯从雾里走出来',
          ),
        ],
      );
      final prose = spec.translate().prose;
      expect(prose, startsWith('3:4 竖构图。'));
      expect(prose, isNot(contains('构图构图')));
      expect(prose, contains('旅人，提着灯从雾里走出来'));
    });

    test('镜像关系会同时进散文和 JSON（坐标说不出"是不是同一个人"）', () {
      const spec = LayoutSpec(
        width: 832,
        height: 1248,
        regions: [
          LayoutRegion(
            id: 'p',
            rect: LayoutRect(250, 400, 500, 500),
            kind: LayoutBoxKind.character,
            label: '女生',
          ),
          LayoutRegion(
            id: 'm',
            rect: LayoutRect(150, 80, 400, 300),
            label: '镜子',
            caption: '镜中映出正面朝向的她',
            relation: LayoutRelation.mirror,
            relationTo: 'p',
          ),
        ],
      );
      final t = spec.translate();
      expect(t.prose, contains('与「女生」是同一个人的镜像'));
      final mirror = spec.mossItems().firstWhere(
        (i) => '${i['caption']}'.contains('镜像'),
      );
      expect(mirror['caption'], contains('姿势必须一致'));
    });

    test('关系指向不存在的框时不生成残句', () {
      const spec = LayoutSpec(
        width: 1024,
        height: 1024,
        regions: [
          LayoutRegion(
            id: 'a',
            rect: LayoutRect(0, 0, 300, 300),
            label: '甲',
            relation: LayoutRelation.same,
            relationTo: 'gone',
          ),
        ],
      );
      final prose = spec.translate().prose;
      expect(prose, isNot(contains('同一个主体')));
      expect(prose, isNot(contains('null')));
    });

    test('宽高比标签', () {
      expect(const LayoutSpec(width: 768, height: 1024).ratioLabel, '3:4 竖构图');
      expect(const LayoutSpec(width: 1024, height: 768).ratioLabel, '4:3 横构图');
      expect(const LayoutSpec(width: 1024, height: 1024).ratioLabel, '1:1 方形');
    });
  });

  group('页面', () {
    testWidgets('窄屏 360 不溢出，宽屏 1440 是左右分栏', (tester) async {
      tester.view.physicalSize = const Size(360, 640);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(const MaterialApp(home: LayoutCanvasPage()));
      await tester.pumpAndSettle();
      expect(find.text('布局 → 提示词'), findsOneWidget);
      // 面板是懒构建的 ListView：只断言首屏一定看得见的那几行
      expect(find.text('场景大概画什么'), findsOneWidget);
      expect(tester.takeException(), isNull);

      tester.view.physicalSize = const Size(1440, 900);
      await tester.pumpAndSettle();
      expect(find.text('让 AI 写'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('两个框以上才出现"关系"编辑区', (tester) async {
      tester.view.physicalSize = const Size(1400, 1800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(const MaterialApp(home: LayoutCanvasPage()));
      await tester.pumpAndSettle();

      final canvas = tester.renderObject<RenderBox>(
        find.byKey(const Key('layout-canvas')),
      );
      final o = canvas.localToGlobal(Offset.zero);
      final s = canvas.size;

      await tester.dragFrom(
        o + Offset(s.width * 0.05, s.height * 0.05),
        Offset(s.width * 0.2, s.height * 0.2),
      );
      await tester.pumpAndSettle();
      expect(find.text('与其他框的关系'), findsNothing);

      await tester.dragFrom(
        o + Offset(s.width * 0.5, s.height * 0.5),
        Offset(s.width * 0.2, s.height * 0.2),
      );
      await tester.pumpAndSettle();
      expect(find.text('与其他框的关系'), findsOneWidget);
      // 选中"镜像"后要能挑目标框
      await tester.tap(find.text('镜像 / 倒影'));
      await tester.pumpAndSettle();
      expect(find.text('未命名的区域'), findsWidgets);
      expect(tester.takeException(), isNull);
    });

    testWidgets('拖出来的框，起点就是按下的点（不吃手势 slop）', (tester) async {
      tester.view.physicalSize = const Size(1200, 1600);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await tester.pumpWidget(const MaterialApp(home: LayoutCanvasPage()));
      await tester.pumpAndSettle();

      final canvas = tester.renderObject<RenderBox>(
        find.byKey(const Key('layout-canvas')),
      );
      final origin = canvas.localToGlobal(Offset.zero);
      final size = canvas.size;

      // 点都按"画布内的相对位置"取，窗口高度一变也不会跑出画布
      final from = origin + Offset(size.width * 0.1, size.height * 0.1);
      final to = origin + Offset(size.width * 0.4, size.height * 0.5);
      await tester.dragFrom(from, to - from);
      await tester.pumpAndSettle();

      int ux(double dx) => ((dx - origin.dx) / size.width * 1000).round();
      int uy(double dy) => ((dy - origin.dy) / size.height * 1000).round();

      final json = tester
          .widgetList<SelectableText>(find.byType(SelectableText))
          .map((w) => w.data ?? '')
          .firstWhere((t) => t.contains('bbox_coord_space'));
      final decoded = jsonDecode(json) as Map<String, dynamic>;
      final items = decoded['items'] as List<dynamic>;
      final bbox =
          (items.first as Map<String, dynamic>)['bbox'] as List<dynamic>;

      // 1024x1024：起点 floor、终点 ceil，再各减半幅。
      // 这条用例盯的是"onPanStart 的位置已经吃掉 20px slop"那个 bug ——
      // 症状是拖出来的框比按下的地方整体偏右下。
      const h = 1024;
      expect(bbox[0], uy(from.dy) * h ~/ 1000 - h ~/ 2);
      expect(bbox[1], ux(from.dx) * h ~/ 1000 - h ~/ 2);
      expect(bbox[2], (uy(to.dy) * h + 999) ~/ 1000 - h ~/ 2);
      expect(bbox[3], (ux(to.dx) * h + 999) ~/ 1000 - h ~/ 2);
    });
  });
}
