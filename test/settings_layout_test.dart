// 设置页的回归（2026-11-05 起首页只放入口，配置项进子页）：
//
//   1) 选项 ≥ 3 的配置不在首页摊开，首页是可点的入口行；
//   2) 「AI 工具权限」与「AI 模型与凭据」是**平级**入口，不在模型页里面；
//   3) 子页里的开关行不能顶到卡片边缘；窄窗口下按钮组折行、不溢出。

import 'dart:convert';

import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride, TargetPlatform;
import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/theme.dart';
import 'package:viewer/pages/settings_page.dart';
import 'package:viewer/pages/settings_search_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';
import 'package:viewer/widgets/settings_scaffold.dart';
import 'package:viewer/widgets/settings_switch_row.dart';

MockClient _mockClient() {
  return MockClient((request) async {
    final path = request.url.path;
    Object? body;
    var status = 200;

    if (path == '/api/capture/config') {
      body = {
        'enabled': true,
        'comfyUrl': 'http://127.0.0.1:8188',
        'pollSeconds': 4,
        'autoTag': 'ComfyUI',
        'maxPerPoll': 20,
        'downloadFallback': true,
      };
    } else if (path == '/api/capture/status') {
      body = {
        'enabled': true,
        'comfyUrl': 'http://127.0.0.1:8188',
        'comfyReachable': true,
        'queueRunning': 0,
        'queuePending': 0,
        'capturedRuns': 0,
        'capturedMedia': 0,
        'recent': <Object>[],
      };
    } else if (path == '/api/ai/nsfw') {
      // NSFW 开关（两个选项放一起）：默认全关
      body = {
        'enabled': false,
        'blur': false,
        'effectiveBlur': false,
        'promptVersion': 'nsfw-v1',
      };
    } else if (path == '/api/tags' || path == '/api/tags/categories') {
      body = <Object>[];
    } else if (path == '/api/stats') {
      body = {
        'prompts': 0,
        'media': 0,
        'tags': 0,
        'favoritePrompts': 0,
        'favoriteMedia': 0,
        'byKind': <String, int>{},
        'byMediaKind': <String, int>{},
      };
    } else if (path == '/api/prompts' || path == '/api/media') {
      body = {
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 24,
        'pages': 0,
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

/// 设置页现在还要 `NsfwStore`（NSFW 两个开关的真源在后端），返回挂好 provider 的那棵树
/// （返回 launcher 是为了让用例能摆出"后端已经就绪 / 端口回落过"这些状态）。
Future<BackendLauncher> _pumpSettings(
  WidgetTester tester, {
  int? portFallbackFrom,
  BackendLayout? layout,
}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final client = _mockClient();
  final store = LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: client),
  );
  // 「端口没能要回来」是后端写在 .run\api.json 里的事实，由 launcher 读懂后交给界面。
  // 这里直接把它摆出来（真值的来源在 backend_launcher_test.dart 里验）。
  final launcher = BackendLauncher(settings)
    ..portFallbackFrom = portFallbackFrom
    ..layout = layout;
  final nsfw = NsfwStore(api: AiApiClient(settings.baseUrl, client: client));
  addTearDown(store.dispose);
  addTearDown(launcher.dispose);
  addTearDown(nsfw.dispose);
  await store.refreshAll();
  await nsfw.load(silent: true);

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
        ChangeNotifierProvider<LibraryStore>.value(value: store),
        ChangeNotifierProvider<NsfwStore>.value(value: nsfw),
      ],
      // ⚠ `UniqueKey` 不能省：同一个测试里第二次 `_pumpSettings` 时，若沿用同一个
      // `MaterialApp` 元素，Navigator 的**路由栈会保留**（上一轮 push 的子页还在），
      // 新树仍停在那条子页上。换 key 强制重建整棵树。
      child: MaterialApp(key: UniqueKey(), home: const SettingsPage()),
    ),
  );
  await tester.pumpAndSettle();
  return launcher;
}

void main() {
  testWidgets('桌面设置：左侧只有大项主标题，点开后在右侧显示主体界面', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    final nav = find.byKey(const Key('settings-nav'));
    Finder navLabel(String label) =>
        find.descendant(of: nav, matching: find.text(label));

    // 1) 分类栏只有九个**大项**，每项只有主标题，没有副标题。
    for (final entry in const [
      '概览',
      '本地服务',
      '远程访问与配对',
      'AI 模型与凭据',
      'AI 工具权限',
      'ComfyUI 自动捕获',
      '外观与布局',
      '偏好',
      '数据目录',
    ]) {
      expect(navLabel(entry), findsOneWidget, reason: '分类栏应该有「$entry」');
    }
    final navRows = tester.widgetList<ListTile>(
      find.descendant(of: nav, matching: find.byType(ListTile)),
    );
    expect(navRows.length, 9);
    expect(
      navRows.every((row) => row.subtitle == null),
      isTrue,
      reason: '分类栏只放主标题，不放说明',
    );
    expect(
      find.byType(SettingsEntryTile),
      findsNothing,
      reason: '桌面端用分类栏，不再用入口行',
    );

    // 2) 点进「本地服务」：正文在右侧，开关行不顶到卡片边缘。
    await tester.tap(navLabel('本地服务'));
    await tester.pumpAndSettle();

    const switchTitle = '启动 App 时自动拉起服务';
    final row = find
        .ancestor(
          of: find.text(switchTitle),
          matching: find.byType(SettingsSwitchRow),
        )
        .first;
    final card = find
        .ancestor(of: find.text(switchTitle), matching: find.byType(Card))
        .first;
    final titleRect = tester.getRect(find.text(switchTitle));
    final switchRect = tester.getRect(
      find.descendant(of: row, matching: find.byType(Switch)),
    );
    final cardRect = tester.getRect(card);
    expect(
      titleRect.left - cardRect.left,
      greaterThanOrEqualTo(20),
      reason: '左侧要有留白，不能贴到卡片边缘',
    );
    expect(
      cardRect.right - switchRect.right,
      greaterThanOrEqualTo(20),
      reason: '右侧（开关那一侧）也要有留白',
    );
  });

  // 设置首页该项此前打开的是**运行根目录**：源码树中运行时即仓库根，
  // 打开后显示的是开发者的项目目录。
  // 现在它是「数据目录」子页的入口（副标题为数据目录），
  // 更换位置（含迁移）在子页中执行。
  testWidgets('设置：分类栏的「数据目录」点开后在右侧显示当前位置与迁移入口', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(
      tester,
      layout: const BackendLayout(root: r'D:\demo\viewer', exe: null),
    );

    final nav = find.byKey(const Key('settings-nav'));
    expect(
      find.descendant(of: nav, matching: find.text('数据目录')),
      findsOneWidget,
      reason: '分类栏只有主标题，路径放在右侧正文里',
    );
    expect(
      find.descendant(of: nav, matching: find.byType(ListTile)),
      findsNWidgets(9),
    );

    // 点进去是右侧正文：当前位置、两个一键搬家的按钮
    await tester.tap(find.descendant(of: nav, matching: find.text('数据目录')));
    await tester.pumpAndSettle();
    expect(find.text('当前位置'), findsOneWidget);
    expect(find.text('搬到别处…'), findsOneWidget);
    expect(find.textContaining('原来文件夹中的内容不会被删除'), findsOneWidget);
    expect(find.text(r'D:\demo\viewer'), findsOneWidget);
  });

  // 主题（浅色 / 深色）的排版：应在一行内完成。
  //
  // 当时的写法是 `ListTile(标题) + 另起一行 Wrap(三个 chip)`：这张卡片的 `Column`
  // 是默认的 `crossAxisAlignment: center`，而 `Wrap` 只占内容宽度 —— 于是控件
  // **在整行里居中**，看着就是"标题独在左边、控件飘在中间"。现在改成一行：
  // 标题在左、三段选择器在右。
  //
  // 判据用**几何**：纵向中心必须基本重合（"同一行"）；退回"另起一行"时
  // 纵向中心会差出几十像素，这条立刻红。
  testWidgets('设置页：主题是"标题在左、选择器在右"的一行，不是另起一行居中', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    await tester.tap(
      find.descendant(
        of: find.byKey(const Key('settings-nav')),
        matching: find.text('外观与布局'),
      ),
    );
    await tester.pumpAndSettle();

    final title = tester.getRect(find.text('明暗模式'));
    final picker = tester.getRect(find.byType(SegmentedButton<AppThemeMode>));
    expect(picker.left, greaterThan(title.right), reason: '选择器要在标题右边');
    // 标题行带一句副标题（两行），选择器在右侧垂直居中 —— 两者中心允许差半个
    // 行高；真正"另起一行"时差值会跳到 40 以上。
    expect(
      (picker.center.dy - title.center.dy).abs(),
      lessThan(20),
      reason: '两者必须在同一行，不能另起一行',
    );
  });

  // 桌面设置的层次**只用底色深浅表达**（与 AI 工作台同一做法）：
  // 左侧分类栏高一档、右侧正文回到 surface、正文卡片再高一档；不画分隔线。
  // 分类项竖排、同一列宽，选中项只有一个。
  testWidgets('桌面设置：分类栏与正文分属两档底色，分类项竖排且只选中一个', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    final nav = find.byKey(const Key('settings-nav'));
    final pane = find.byKey(const Key('settings-pane'));
    final theme = Theme.of(tester.element(nav));
    final scheme = theme.colorScheme;
    expect(
      tester.widget<Material>(nav).color,
      scheme.surfaceContainerHigh,
      reason: '分类栏是二级面板那一档',
    );
    expect(
      tester.widget<ColoredBox>(pane).color,
      scheme.surface,
      reason: '正文回到 surface',
    );

    // 左栏顶部的「设置」与右侧各分类的 AppBar 标题必须是同一套字级，
    // 不能一边大一边小。
    final settingsTitle = tester.widget<Text>(
      find.descendant(of: nav, matching: find.text('设置')),
    );
    final titleStyle = theme.appBarTheme.titleTextStyle ?? theme.textTheme.titleLarge;
    expect(
      settingsTitle.style?.fontSize,
      titleStyle?.fontSize,
      reason: '左栏标题与右侧 AppBar 标题同字号',
    );
    expect(
      settingsTitle.style?.fontWeight,
      titleStyle?.fontWeight,
      reason: '左栏标题与右侧 AppBar 标题同字重',
    );

    final navRect = tester.getRect(nav);
    final paneRect = tester.getRect(pane);
    expect(
      paneRect.left,
      closeTo(navRect.right, 0.5),
      reason: '正文紧挨分类栏，不再画一道线',
    );
    expect(navRect.width, closeTo(264, 0.5));
    expect(paneRect.width, closeTo(1700 - 264, 0.5));

    final rows = find.descendant(of: nav, matching: find.byType(ListTile));
    expect(rows, findsNWidgets(9));
    final first = tester.getRect(rows.at(0));
    final second = tester.getRect(rows.at(1));
    expect(second.left, closeTo(first.left, 0.5), reason: '分类项在同一列');
    expect(second.top, greaterThan(first.top), reason: '分类项竖排，不是磁贴');
    expect(second.width, closeTo(first.width, 0.5), reason: '每项同宽');
    expect(
      first.height,
      closeTo(44, 1),
      reason: '分类项比右侧 56 高的 AppBar 矮一档，不能比页头还大',
    );
    expect(tester.widget<ListTile>(rows.at(0)).selected, isTrue);
    expect(tester.widget<ListTile>(rows.at(1)).selected, isFalse);
    expect(
      find.byType(BackButton),
      findsNothing,
      reason: '分栏根页面没有上一页，不该出现返回箭头',
    );
  });

  // NSFW 属于「偏好」，不再混进「概览」：
  // 概览只回答"库里有什么"，偏好才是改内容策略的地方。
  testWidgets('概览不混入 NSFW；NSFW 只在偏好里', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    final nav = find.byKey(const Key('settings-nav'));
    Finder navLabel(String label) =>
        find.descendant(of: nav, matching: find.text(label));

    expect(find.text('NSFW 模式'), findsNothing, reason: '概览不该再放内容策略');
    expect(find.text('库统计'), findsOneWidget);

    await tester.tap(navLabel('偏好'));
    await tester.pumpAndSettle();
    expect(find.text('NSFW 模式'), findsOneWidget);
    expect(find.text('NSFW 内容模糊显示'), findsOneWidget);
    expect(find.text('库统计'), findsNothing, reason: '库统计留在概览');
  });

  // 分栏不是"窗口够宽就一定能放下"：900 宽时分类栏占掉 264，右侧正文只有 636。
  // 这一档要逐项点过去，确认没有任何一页被挤到溢出（正文按自己那一格的宽度排版）。
  testWidgets('窗口刚过 900：每个分类都能在右侧渲染，不横向溢出', (tester) async {
    tester.view.physicalSize = const Size(900, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    final nav = find.byKey(const Key('settings-nav'));
    Finder navLabel(String label) =>
        find.descendant(of: nav, matching: find.text(label));

    for (final label in const [
      '本地服务',
      '远程访问与配对',
      'AI 模型与凭据',
      'AI 工具权限',
      'ComfyUI 自动捕获',
      '外观与布局',
      '偏好',
      '数据目录',
    ]) {
      await tester.tap(navLabel(label));
      // 这一档不能用 pumpAndSettle：加载中的转圈是无限动画。
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(
        tester.takeException(),
        isNull,
        reason: '900 宽下「$label」的正文不许 RenderFlex overflow',
      );
    }
  });

  // 从搜索点进「NSFW 内容」时，这一页是叠在搜索结果之上的独立路由。
  // 分栏骨架自己没有 AppBar，必须在分类栏顶部补一个返回入口；
  // 没有它，桌面上只能靠系统返回，等于"进去出不来"。
  testWidgets('桌面设置：从搜索进入 NSFW 后有返回入口，返回回到搜索结果', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    await tester.tap(find.text('搜索设置'));
    await tester.pumpAndSettle();
    expect(find.byType(SettingsSearchPage), findsOneWidget);
    // 搜索页在右侧这一格里打开，左侧分类栏仍然在。
    expect(find.byKey(const Key('settings-desktop-shell')), findsOneWidget);
    expect(find.byKey(const Key('settings-nav')), findsOneWidget);

    await tester.enterText(find.byType(TextField), 'NSFW 模式');
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(ListTile, 'NSFW 模式'));
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('settings-desktop-shell')), findsOneWidget);
    expect(find.byType(PreferencesSettingsPage), findsOneWidget);
    expect(find.byType(BackButton), findsOneWidget, reason: '叠在搜索结果上的设置页要能退回去');

    await tester.tap(find.byType(BackButton));
    await tester.pumpAndSettle();
    expect(find.byType(SettingsSearchPage), findsOneWidget);
    expect(find.byType(PreferencesSettingsPage), findsNothing);
    // 退回的是右侧那一格，设置分栏与分类栏都还在。
    expect(find.byKey(const Key('settings-desktop-shell')), findsOneWidget);
    expect(find.byKey(const Key('settings-nav')), findsOneWidget);
  });

  // 手机端排版保持不变，电脑端窄屏恢复为手机端排版。
  // 判据与导航形态同一个数字（kWideLayoutMinWidth = 900）：低于它就是手机形态，
  // 电脑窗口拉窄也一样 —— 分组入口行 + 单列卡片，不是磁贴。
  testWidgets('电脑端窄屏（< 900）用手机那套分组入口，不是磁贴', (tester) async {
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    expect(find.text('服务'), findsOneWidget);
    expect(find.text('AI'), findsOneWidget);
    expect(find.byType(SettingsEntryTile), findsWidgets);
    expect(
      find.byKey(const Key('settings-nav')),
      findsNothing,
      reason: '窄屏不该出现桌面端的分类栏',
    );
  });

  // NSFW 两个开关放在一起，模糊选项**从属于**主开关：
  // 主开关没开时它是置灰的 —— 没开 NSFW 就谈不上"给 NSFW 内容打码"。
  testWidgets('设置页：NSFW 开关与模糊选项放一起，后者从属于前者', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    await tester.tap(
      find.descendant(
        of: find.byKey(const Key('settings-nav')),
        matching: find.text('偏好'),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('NSFW 内容'), findsOneWidget);
    final master = find.ancestor(
      of: find.text('NSFW 模式'),
      matching: find.byType(SettingsSwitchRow),
    );
    final blur = find.ancestor(
      of: find.text('NSFW 内容模糊显示'),
      matching: find.byType(SettingsSwitchRow),
    );
    expect(master, findsOneWidget);
    expect(blur, findsOneWidget);

    // 同一张卡片里（"放一起"）
    final card = find
        .ancestor(of: find.text('NSFW 模式'), matching: find.byType(Card))
        .first;
    expect(
      find.descendant(of: card, matching: find.text('NSFW 内容模糊显示')),
      findsOneWidget,
      reason: '两个选项必须在同一张卡片里',
    );

    // 主开关关着 → 模糊那一项不可点
    expect(tester.widget<SettingsSwitchRow>(master).value, isFalse);
    expect(
      tester.widget<SettingsSwitchRow>(blur).onChanged,
      isNull,
      reason: 'NSFW 模式没开时模糊选项应当置灰',
    );
  });

  // 窄窗口下控件显示不全。
  //
  // 本地服务那张卡片里有**按钮组**（原来是裸 `Row`）：[自动探测][保存]、
  // [保存并测试连接][恢复默认]。320px 窗口下卡片内容宽 288，最宽的那组
  // 自然宽度就有 ~295 → `Row` 直接 `RenderFlex overflowed`（实测溢出 7.1px），
  // 右边那枚按钮被裁掉一半。现在换 `Wrap`：放不下就折行，按钮一个都不少。
  // （原第三组 [保存][迁移老库到该目录…] 已随 2026-09-29 MySQL 退役删除。）
  testWidgets('设置页：窗口很窄时按钮组折行，不溢出（用户 bug：控件显示不全）', (tester) async {
    // 高度给足：各组按钮一次全在树上（这一页本身是可滚的，这里要盯的是**横向**溢出）
    tester.view.physicalSize = const Size(320, 2600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    await tester.tap(find.text('本地服务（后端 + 数据库）'));
    await tester.pumpAndSettle();

    expect(
      tester.takeException(),
      isNull,
      reason: '窄窗口下设置页不许 RenderFlex overflow',
    );
    for (final label in const ['自动探测', '保存并测试连接', '恢复默认']) {
      expect(find.text(label), findsOneWidget, reason: '$label 不能被裁掉');
    }

    // 回首页 → 进「界面」：三段主题选择器在最窄的 320dp 上一个都不许被裁掉。
    await tester.pageBack();
    await tester.pumpAndSettle();
    await tester.tap(find.text('外观与布局'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull, reason: '窄窗口下界面子页不许溢出');
    expect(find.text('明暗模式'), findsOneWidget);
    for (final label in const ['深色', '浅色', '跟随系统']) {
      expect(find.text(label), findsOneWidget, reason: '主题的「$label」不能被挤掉');
    }
  });

  // 「允许手机 / 局域网设备连接」与「允许移动端连接」不是同一个值，
  // 但名称相近、结果都是"手机能否连接"：
  //   · 这个开关 = 后端**听不听**局域网（起进程时定，要重启后端）；
  //   · 那个（在「远程访问与配对」里）= **配对闸门**：局域网请求要不要带令牌。
  // 所以两边的说明中都要有"①/②"与对方的入口 —— 否则无法判断
  // "已开启但手机连不上"发生在哪一步。
  testWidgets('设置页：局域网开关要写明它和"配对闸门"是两道不同的闸', (tester) async {
    tester.view.physicalSize = const Size(1700, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);
    await tester.tap(
      find.descendant(
        of: find.byKey(const Key('settings-nav')),
        matching: find.text('本地服务'),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('允许手机 / 局域网设备连接'), findsOneWidget);
    expect(find.textContaining('① 让后端监听局域网'), findsOneWidget);
    expect(
      find.textContaining('② 配对闸门'),
      findsOneWidget,
      reason: '必须指出另一道闸在哪，否则两个开关看着就是重复的',
    );
  });

  // 「端口没能要回上次那个」必须显示出来（2026-10-13，与「端口粘住」一起加的）。
  //
  // 后端平时会要回上一次的端口，手机上存的 `http://<局域网IP>:<端口>` 一直有效；
  // 一旦那个端口被别的程序占了，后端只能回落 —— **手机保存的地址当场失效**。
  // 这条提示是唯一线索：缺少它时，只会遇到"手机突然连不上"，
  // 无法判断需要重新扫码（本仓 §12.3）。
  //
  // 一句话两半：**该亮的时候要亮**，**正常的时候不许常年亮着吓人**。
  testWidgets('设置页：端口回落时提示"手机要重扫"，正常时不显示（窄屏也不溢出）', (tester) async {
    // 真机那两档宽度：384（OnePlus 8T）/ 320（最窄）—— 这句话很长，
    // 它必须自己折行而不是把卡片顶爆。
    for (final width in const [384.0, 320.0]) {
      tester.view.physicalSize = Size(width, 1500);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await _pumpSettings(tester, portFallbackFrom: 6430);
      await tester.tap(find.text('本地服务（后端 + 数据库）'));
      await tester.pumpAndSettle();

      expect(
        find.textContaining('6430 这次没能用上'),
        findsOneWidget,
        reason: '${width}dp：端口回落必须写出来',
      );
      expect(find.textContaining('重新扫一次'), findsOneWidget);
      expect(tester.takeException(), isNull, reason: '${width}dp 下这条提示不许溢出');
    }

    // 正常要回来（portFallbackFrom 为 null）→ 一个字都不该出现
    tester.view.physicalSize = const Size(1700, 1200);
    await _pumpSettings(tester);
    // 正常时**首页**就不该出现这条警告（子页里也不该有，但先守住第一道）
    expect(
      find.textContaining('这次没能用上'),
      findsNothing,
      reason: '端口正常时不许常年亮一条警告',
    );
  });

  // 手机设置中不应出现仅电脑可用的功能。
  // 2026-11-05 把设置页拆成子界面时，「本地服务（后端 + 数据库）」这一项又变回
  // 无条件渲染了（它在 2026-09-28 就被 `isWindowsDesktop` 挡过一次）。
  // 判据是**平台**，不是窗口宽度：手机连的是桌面后端，本机没有可拉起的 Rust exe，
  // 也没有盘符 / 资源管理器。
  testWidgets('手机：设置首页没有只有电脑端才有的入口', (tester) async {
    // ⚠ override 必须在用例体内恢复（`_verifyInvariants` 会断言它已清零），
    // 不能只用 `addTearDown` —— 那时断言已经跑过了。
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    try {
      tester.view.physicalSize = const Size(720, 3600);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);

      await _pumpSettings(tester);

      expect(
        find.text('本地服务（后端 + 数据库）'),
        findsNothing,
        reason: '手机上不该出现本地服务入口（本机没有后端可拉）',
      );
      expect(
        find.text('本机数据与同步'),
        findsOneWidget,
        reason: '手机端要有自己的那一项（本机库 + 同步），不是把整组藏空了事',
      );
      expect(
        find.text('远程访问与配对'),
        findsOneWidget,
        reason: '配对是手机端真正要用的那一项，不能跟着一起藏掉',
      );
      expect(find.text('数据目录'), findsNothing, reason: '数据目录是桌面盘符的事');
      expect(find.text('导出全部产物…'), findsNothing, reason: '导出写的是桌面磁盘路径');
      expect(find.text('产物缓存（本机）'), findsOneWidget, reason: '手机专属的那一节要还在');
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
