// 设置页的两条回归：
//
//   1) 宽窗口按「宽度 / 550」分列（以前一路单列往下滚）
//   2) 开关行（「开启自动捕获」这些）不能顶到卡片边缘 —— 它们用的是
//      contentPadding 为零的 SwitchListTile，必须自己套一层内边距

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/theme.dart';
import 'package:viewer/pages/settings_page.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/state/nsfw_store.dart';

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
      body = {'enabled': false, 'blur': false, 'effectiveBlur': false, 'promptVersion': 'nsfw-v1'};
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
      body = {'items': <Object>[], 'total': 0, 'page': 1, 'size': 24, 'pages': 0};
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
}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final client = _mockClient();
  final store = LibraryStore(settings, api: ApiClient(settings.baseUrl, client: client));
  // 「端口没能要回来」是后端写在 .run\api.json 里的事实，由 launcher 读懂后交给界面。
  // 这里直接把它摆出来（真值的来源在 backend_launcher_test.dart 里验）。
  final launcher = BackendLauncher(settings)..portFallbackFrom = portFallbackFrom;
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
      child: const MaterialApp(home: SettingsPage()),
    ),
  );
  await tester.pumpAndSettle();
  return launcher;
}

void main() {
  testWidgets('设置页：宽窗口分列，开关行左右都留出了内边距', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    // 1) 分列：自动捕获排到了右边那一列
    final left = tester.getTopLeft(find.text('本地服务（后端 + 数据库）'));
    final right = tester.getTopLeft(find.text('ComfyUI 自动捕获'));
    expect(right.dx, greaterThan(left.dx + 100), reason: '宽窗口应该分两列');

    // 2) 开关行不顶到卡片边缘（16 卡片内边距 + 10 自己套的内边距）
    final tile = find
        .ancestor(of: find.text('开启自动捕获'), matching: find.byType(SwitchListTile))
        .first;
    final card = find.ancestor(of: find.text('开启自动捕获'), matching: find.byType(Card)).first;
    final tileRect = tester.getRect(tile);
    final cardRect = tester.getRect(card);
    expect(tileRect.left - cardRect.left, greaterThanOrEqualTo(20),
        reason: '左侧要有留白，不能贴到卡片边缘');
    expect(cardRect.right - tileRect.right, greaterThanOrEqualTo(20),
        reason: '右侧（开关那一侧）也要有留白');
  });

  // 用户 2026-10-13：「主题（浅色/深色）那里排版奇怪，不应该一行搞定吗」。
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

    final title = tester.getRect(find.text('主题'));
    final picker = tester.getRect(find.byType(SegmentedButton<AppThemeMode>));
    expect(picker.left, greaterThan(title.right), reason: '选择器要在标题右边');
    expect((picker.center.dy - title.center.dy).abs(), lessThan(8),
        reason: '两者必须在同一行（纵向中心基本重合），不能另起一行');
  });

  // 「AI 模型与凭据」是这一页最常去的入口，必须排在前面的列里，
  // 不能压在「ComfyUI 自动捕获 / 库统计」下面（用户反馈过要往下翻很久）。
  testWidgets('设置页：AI 模型与凭据排在 ComfyUI 自动捕获前面', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    final local = tester.getTopLeft(find.text('本地服务（后端 + 数据库）'));
    final ai = tester.getTopLeft(find.text('AI 模型与凭据'));
    final capture = tester.getTopLeft(find.text('ComfyUI 自动捕获'));

    // 排在「自动捕获」前面（同一列更靠上，或者前一个列）
    final beforeCapture = ai.dx < capture.dx - 100 || ai.dy < capture.dy;
    expect(beforeCapture, isTrue,
        reason: 'AI 入口不能排在 ComfyUI 自动捕获之后（实际 ai=$ai capture=$capture）');
    expect(ai.dy, greaterThan(local.dy - 1), reason: '本地服务仍然排在最前');
  });

  // NSFW 两个开关放一起（用户要求），而且模糊选项**从属于**开关：
  // 主开关没开时它是置灰的 —— 没开 NSFW 就谈不上"给 NSFW 内容打码"。
  testWidgets('设置页：NSFW 开关与模糊选项放一起，后者从属于前者', (tester) async {
    tester.view.physicalSize = const Size(1700, 1200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    expect(find.text('NSFW 内容'), findsOneWidget);
    final master = find.ancestor(
      of: find.text('NSFW 模式'),
      matching: find.byType(SwitchListTile),
    );
    final blur = find.ancestor(
      of: find.text('NSFW 内容模糊显示'),
      matching: find.byType(SwitchListTile),
    );
    expect(master, findsOneWidget);
    expect(blur, findsOneWidget);

    // 同一张卡片里（"放一起"）
    final card = find.ancestor(of: find.text('NSFW 模式'), matching: find.byType(Card)).first;
    expect(
      find.descendant(of: card, matching: find.text('NSFW 内容模糊显示')),
      findsOneWidget,
      reason: '两个选项必须在同一张卡片里',
    );

    // 主开关关着 → 模糊那一项不可点
    expect(tester.widget<SwitchListTile>(master).value, isFalse);
    expect(tester.widget<SwitchListTile>(blur).onChanged, isNull,
        reason: 'NSFW 模式没开时模糊选项应当置灰');
  });

  // 用户 bug（窄窗口）："控件显示不全"。
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

    expect(tester.takeException(), isNull, reason: '窄窗口下设置页不许 RenderFlex overflow');
    for (final label in const [
      '自动探测',
      '保存并测试连接',
      '恢复默认',
    ]) {
      expect(find.text(label), findsOneWidget, reason: '$label 不能被裁掉');
    }

    // 「主题」那一行是**标题 + 三段选择器挤在一行**里（用户 2026-10-13 报过
    // "排版奇怪，不应该一行搞定吗"，所以它不能退回"另起一行居中"）。
    // 一行放三个中文标签最容易挤爆的就是这个宽度 —— 三段一个都不许被裁掉。
    expect(find.text('主题'), findsOneWidget);
    for (final label in const ['深色', '浅色', '跟随系统']) {
      expect(find.text(label), findsOneWidget, reason: '主题的「$label」不能被挤掉');
    }
  });

  // 用户 2026-10-13 问："「允许手机 / 局域网设备连接」和「允许移动端连接」
  // 是不是重复了？"——**不是**同一个值，但名字像、后果都是"手机能不能连"：
  //   · 这个开关 = 后端**听不听**局域网（起进程时定，要重启后端）；
  //   · 那个（在「远程访问与配对」里）= **配对闸门**：局域网请求要不要带令牌。
  // 所以两边的说明里都要有"①/②"和对方的去处 —— 少了它，用户遇到
  // "我明明开了，手机还是连不上"就只能靠猜是卡在哪一道。
  testWidgets('设置页：局域网开关要写明它和"配对闸门"是两道不同的闸', (tester) async {
    tester.view.physicalSize = const Size(1700, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpSettings(tester);

    expect(find.text('允许手机 / 局域网设备连接'), findsOneWidget);
    expect(find.textContaining('① 让后端监听局域网'), findsOneWidget);
    expect(find.textContaining('② 配对闸门'), findsOneWidget,
        reason: '必须指出另一道闸在哪，否则两个开关看着就是重复的');
  });

  // 「端口没能要回上次那个」必须显示出来（2026-10-13，与「端口粘住」一起加的）。
  //
  // 后端平时会要回上一次的端口，手机上存的 `http://<局域网IP>:<端口>` 一直有效；
  // 一旦那个端口被别的程序占了，后端只能回落 —— **手机保存的地址当场失效**。
  // 这条提示是用户唯一的线索：少了它，用户遇到的只是"手机突然连不上"，
  // 猜不到该去重扫二维码（本仓 §12.3）。
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

      expect(find.textContaining('6430 这次没能用上'), findsOneWidget,
          reason: '${width}dp：端口回落必须写出来');
      expect(find.textContaining('重新扫一次'), findsOneWidget);
      expect(tester.takeException(), isNull, reason: '${width}dp 下这条提示不许溢出');
    }

    // 正常要回来（portFallbackFrom 为 null）→ 一个字都不该出现
    tester.view.physicalSize = const Size(1700, 1200);
    await _pumpSettings(tester);
    expect(find.textContaining('这次没能用上'), findsNothing,
        reason: '端口正常时不许常年亮一条警告');
  });
}
