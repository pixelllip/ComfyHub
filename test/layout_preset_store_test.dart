import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/layout_preset_store.dart';
import 'package:viewer/models/layout_spec.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const spec = LayoutSpec(
    width: 768,
    height: 1024,
    brief: '雾夜废墟',
    regions: [
      LayoutRegion(
        id: 'r1',
        rect: LayoutRect(0, 0, 330, 800),
        label: '教堂',
        caption: '没入雾中的轮廓',
      ),
      LayoutRegion(
        id: 'r2',
        rect: LayoutRect(560, 620, 280, 300),
        kind: LayoutBoxKind.character,
        label: '旅人',
        text: '',
        relation: LayoutRelation.mirror,
        relationTo: 'r1',
      ),
    ],
  );

  test('规格的 JSON 往返不丢东西（含关系与文字）', () {
    final back = LayoutSpec.fromJson(spec.toJson());
    expect(back.width, spec.width);
    expect(back.height, spec.height);
    expect(back.brief, spec.brief);
    expect(back.regions.length, 2);
    expect(back.regions[1].kind, LayoutBoxKind.character);
    expect(back.regions[1].relation, LayoutRelation.mirror);
    expect(back.regions[1].relationTo, 'r1');
    expect(back.regions[0].rect, const LayoutRect(0, 0, 330, 800));
    // 存回来再翻译一遍，结果要和没存过的一样
    expect(back.translate().prose, spec.translate().prose);
    expect(back.translate().mossJson, spec.translate().mossJson);
  });

  test('存 / 读布局库', () async {
    SharedPreferences.setMockInitialValues({});
    final store = LayoutPresetStore(prefs: await SharedPreferences.getInstance());
    expect(await store.load(), isEmpty);

    await store.saveAll([
      LayoutPreset(
        id: 'b',
        name: '旧的',
        updatedAt: DateTime(2026, 1, 1),
        spec: spec,
      ),
      LayoutPreset(
        id: 'a',
        name: '新的',
        updatedAt: DateTime(2026, 11, 1),
        spec: spec,
      ),
    ]);

    final loaded = await store.load();
    expect(loaded.length, 2);
    // 按时间倒序：最近保存的在最上面
    expect(loaded.first.name, '新的');
    expect(loaded.first.spec.regions.length, 2);
  });

  test('坏数据只丢那一条，不炸整个库', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(
      LayoutPresetStore.prefsKey,
      '[{"id":"ok","name":"好","spec":{"width":1024,"height":1024,"regions":[]}},'
      '{"name":"缺 id"}, 42, "不是对象"]',
    );
    final loaded = await LayoutPresetStore(prefs: prefs).load();
    expect(loaded.length, 1);
    expect(loaded.first.name, '好');
  });

  test('整段 JSON 烂掉时回空列表', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(LayoutPresetStore.prefsKey, '{不是数组');
    expect(await LayoutPresetStore(prefs: prefs).load(), isEmpty);
  });
}
