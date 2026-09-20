// 窗口很窄（320 逻辑像素，**产品约定的最小宽度**）时，画廊 / 提示词库 / 标签页
// 顶部那排筛选控件的排版回归（用户要求 2026-09-21）：
//
//   原来它们都是 `SingleChildScrollView(scrollDirection: horizontal)` ——
//   窗口一窄，右边的「仅收藏 / 未关联 / 一键清除未关联 / 排序」就被切在屏幕外，
//   用户得横向拖才看得见。用户明确说这样**不符合直觉**：
//   「一行显示不完的控件用两行显示，两行还不行就三行」。
//
// 现在统一走 `FilterChipRow`（`Wrap`）。这组用例盯的就是：
//   ① 一行放不下时**真的折行**（不是被裁）；
//   ② 每个控件都完整落在页面里（没有被任何裁剪祖先切掉）；
//   ③ 全程没有 `RenderFlex overflow`。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/gallery_page.dart';
import 'package:viewer/pages/prompts_page.dart';
import 'package:viewer/pages/tags_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/common.dart';

/// 320 逻辑像素：`windows/runner/win32_window.cpp` 里 `kMinWindowWidthLogical` 的值，
/// 也就是用户**能拖到的最窄**。比这更窄的窗口不存在，不必测。
const _minWidth = 320.0;

final _mediaJson = {
  'id': 1,
  'kind': 'IMAGE',
  'title': 'neon-street.png',
  'originalName': 'neon-street.png',
  'storedName': 'abc.png',
  'mimeType': 'image/png',
  'sizeBytes': 253976,
  'width': 1216,
  'height': 832,
  'favorite': false,
  'createdAt': '2026-09-16T01:00:00.000Z',
  'source': 'ComfyUI',
};

final _promptJson = {
  'id': 1,
  'title': '雨夜霓虹街道 · 赛博朋克',
  'kind': 'IMAGE',
  'positivePrompt': 'neon street, rain, cyberpunk, masterpiece, best quality',
  'favorite': false,
  'mediaCount': 1,
  // 注意：tags 是**对象数组**（`Prompt.fromJson` 会按 `Tag.fromJson` 解析），
  // 写成字符串数组会抛异常 → 整页提示词变成"共 0 条"，用例就测不到筛选行了
  'tags': [
    {'id': 1, 'name': '赛博朋克'},
  ],
};

/// 长分类名：就是它们把筛选行撑到第二、第三行的
const _categories = ['风格与画质调校', '角色与服装设定', '负面与排除词'];

MockClient _mockClient() => MockClient((request) async {
      final path = request.url.path;
      Object? body;
      var status = 200;
      Map<String, dynamic> page(List<Map<String, dynamic>> items) =>
          {'items': items, 'total': items.length, 'page': 1, 'size': 24, 'pages': 1};

      if (path == '/api/media') {
        body = page([_mediaJson]);
      } else if (path == '/api/prompts') {
        body = page([_promptJson]);
      } else if (path == '/api/tags') {
        body = [
          {'id': 1, 'name': '赛博朋克', 'useCount': 12, 'category': _categories[0]},
          {'id': 2, 'name': '电影感', 'useCount': 7, 'category': _categories[1]},
          {'id': 3, 'name': '8K', 'useCount': 3, 'category': _categories[2]},
        ];
      } else if (path == '/api/tags/categories') {
        body = _categories;
      } else if (path == '/api/stats') {
        body = {
          'prompts': 1,
          'media': 1,
          'tags': 3,
          'favoritePrompts': 1,
          'favoriteMedia': 0,
          'byKind': {'IMAGE': 1},
          'byMediaKind': {'IMAGE': 1},
        };
      } else {
        body = <Object>[];
      }

      return http.Response(
        jsonEncode(body),
        status,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    });

Future<LibraryStore> _store() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: _mockClient()));
  await store.refreshAll();
  return store;
}

Future<void> _pump(WidgetTester tester, Widget page, LibraryStore store) async {
  // 只给最小宽度，高度给足：这一页本身是可滚的，这里盯的是**横向**放不下时怎么办
  tester.view.physicalSize = const Size(_minWidth, 2000);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    ChangeNotifierProvider<LibraryStore>.value(
      value: store,
      child: MaterialApp(home: page),
    ),
  );
  await tester.pumpAndSettle();
}

/// 这个控件被自己祖先里的裁剪区切掉了吗？（被切 = 用户看不见）
bool _clipped(WidgetTester tester, Finder finder) {
  final rect = tester.getRect(finder.first);
  for (var node = tester.renderObject(finder.first).parent; node != null; node = node.parent) {
    if (node is RenderClipRect || node is RenderClipRRect) {
      final box = node as RenderBox;
      final origin = box.localToGlobal(Offset.zero);
      final viewport = Rect.fromLTWH(origin.dx, origin.dy, box.size.width, box.size.height);
      if (!viewport.contains(rect.topLeft) ||
          !viewport.contains(rect.bottomRight - const Offset(0.01, 0.01))) {
        return true;
      }
    }
  }
  return false;
}

/// 共同断言：所有控件都在页面上、都完整可见、筛选行不再靠横向滚动、没有 overflow。
Future<void> _expectAllVisible(WidgetTester tester, List<String> labels) async {
  // 横向 `Scrollable` 一个都不该有 —— 那正是用户不要的交互。
  // 注意 `TextField` 内部也有一个横向 `Scrollable`（`restorationId: editable`，
  // 是它自己的光标滚动），要把它排掉，否则用例会在无关的东西上红。
  final horizontal = find.byType(Scrollable).evaluate().where((e) {
    final s = e.widget as Scrollable;
    if (s.axisDirection != AxisDirection.right && s.axisDirection != AxisDirection.left) {
      return false;
    }
    return s.restorationId != 'editable';
  }).toList();
  expect(horizontal, isEmpty, reason: '筛选行不许再靠横向滚动兜底');
  expect(find.byType(FilterChipRow), findsWidgets, reason: '筛选行要走会折行的 FilterChipRow');

  for (final label in labels) {
    final finder = find.text(label);
    expect(finder, findsWidgets, reason: '「$label」必须还在页面上');
    expect(_clipped(tester, finder), isFalse, reason: '「$label」不能被裁掉');
  }
  expect(tester.takeException(), isNull, reason: '最小宽度下不许 RenderFlex overflow');
}

void main() {
  testWidgets('画廊：320px 下筛选行折行，每个控件都完整可见', (tester) async {
    final store = await _store();
    addTearDown(store.dispose);
    await _pump(tester, const GalleryPage(), store);

    // 前提：这一排确实放不下一行（否则用例没测到折行）
    final firstRow = tester.getRect(find.text('全部')).top;
    expect(tester.getRect(find.text('清除未关联产物')).top, greaterThan(firstRow),
        reason: '「清除未关联产物」被挤到后面的行 = 真的折行了');

    await _expectAllVisible(tester, const [
      '全部', '图片', '视频', '音频', '仅收藏', '未关联', '清除未关联产物', '最新加入',
    ]);

    // 排序按钮 + 统计也在
    expect(find.byTooltip('排序'), findsWidgets);
    expect(find.textContaining('共 1 个'), findsOneWidget);
  });

  testWidgets('提示词库：320px 下筛选行折行，每个控件都完整可见', (tester) async {
    final store = await _store();
    addTearDown(store.dispose);
    await _pump(tester, const PromptsPage(), store);

    final firstRow = tester.getRect(find.text('全部')).top;
    expect(tester.getRect(find.text('一键清除未关联')).top, greaterThan(firstRow),
        reason: '「一键清除未关联」被挤到后面的行 = 真的折行了');

    await _expectAllVisible(tester, const [
      '全部', '生图', '生视频', '生音频', '混合', '仅收藏', '未关联产物', '一键清除未关联', '最新创建',
    ]);
    expect(find.textContaining('共 1 条'), findsOneWidget);
  });

  testWidgets('标签页：320px 下分类行折行，长分类名不被裁', (tester) async {
    final store = await _store();
    addTearDown(store.dispose);
    await _pump(tester, const TagsPage(), store);

    final firstRow = tester.getRect(find.text('全部分类')).top;
    // 分类名在**卡片**里也会出现（同一个分类），所以取第一个（筛选行那个）
    expect(tester.getRect(find.text(_categories.last).first).top, greaterThan(firstRow),
        reason: '最后一个分类被挤到后面的行 = 真的折行了');

    await _expectAllVisible(tester, [
      '全部分类',
      ..._categories,
      '按使用频率',
    ]);
  });

  // 宽窗口不受影响：还是老老实实一行（折行只在放不下时发生）
  testWidgets('画廊：宽窗口下筛选行仍然是一行', (tester) async {
    final store = await _store();
    addTearDown(store.dispose);
    tester.view.physicalSize = const Size(1600, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: const MaterialApp(home: GalleryPage()),
      ),
    );
    await tester.pumpAndSettle();

    final top = tester.getRect(find.text('全部')).top;
    expect(tester.getRect(find.text('清除未关联产物')).top, closeTo(top, 1),
        reason: '宽窗口下这一排还是一行');
  });
}
