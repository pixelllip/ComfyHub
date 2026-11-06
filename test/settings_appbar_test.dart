// 设置家族的顶栏 / 大标题回归（2026-11-06 用户报的两个 bug）：
//
//   1) 设置首页与子页：大标题滚走时**要逐渐消失**（高度收掉 + 淡出），
//      同一个标题在 AppBar 里按同一个进度淡入；
//   2) 「AI 模型与凭据」「AI 工具权限」以前**没有 AppBar**，正文页头直接顶在
//      body 上 —— 手机上会被状态栏压住，而且和其它设置子页长得不一样。
//
// 两个页面都改成 `SettingsPageScaffold`（与 `SettingsScaffold` 共用 `SettingsAppBar`），
// 所以这里钉的是"有没有 AppBar / 返回键在不在栏里 / 内容在不在栏下面"。
//
// ⚠ 手机形态用 `debugDefaultTargetPlatformOverride` 模拟：`flutter test` 跑在
// Windows 上，真实平台永远是 Windows。`useCollapsingPageTitle` 认这个开关。

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';
import 'package:viewer/pages/ai_tools_settings_page.dart';
import 'package:viewer/widgets/scroll_linked_title.dart';
import 'package:viewer/widgets/settings_scaffold.dart';

/// 真机那一组：`1080x2400` / `2.8125` = 逻辑 `384x853`（OnePlus 8T）。
const _phone = Size(384, 853);

/// 状态栏高度（逻辑像素）。`tester.view.padding` 收的是物理像素，dpr 取 1。
const _statusBar = 24.0;

void _phoneView(WidgetTester tester) {
  tester.view.physicalSize = _phone;
  tester.view.devicePixelRatio = 1.0;
  tester.view.padding = const FakeViewPadding(top: _statusBar);
  addTearDown(tester.view.reset);
}

/// AI 工具权限页要的四条只读端点（其余一律 404，页面会把失败如实画出来）。
MockClient _toolsBackend() => MockClient((request) async {
      final path = request.url.path;
      Object body;
      if (path == '/api/ai/tools') {
        body = const <Object>[];
      } else if (path == '/api/ai/tools/policy' ||
          path == '/api/ai/web-search/credential' ||
          path == '/api/ai/attachments/stats') {
        body = const <String, Object>{};
      } else {
        body = {'error': '接口不存在: $path'};
        return http.Response(jsonEncode(body), 404,
            headers: {'content-type': 'application/json'});
      }
      return http.Response(jsonEncode(body), 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });

/// 一条 Provider：用来进"详情面板"（手机上是子界面，标题是这条 Provider 的名字）。
MockClient _providerBackend() => MockClient((request) async {
      final path = request.url.path;
      Object body;
      if (path == '/api/ai/providers') {
        body = [
          {
            'id': 'command-code-goat',
            'displayName': 'Command Code Goat',
            'api': 'openai-completions',
            'baseURL': 'https://api.commandcode.ai/provider/v1',
            'credentialRef': 'COMMAND_CODE_GOAT',
            'endpointTrust': 'public',
            'enabled': true,
            'revision': 1,
            'credential': {'configured': true, 'source': 'keystore', 'writable': true},
          }
        ];
      } else if (path == '/api/ai/providers/command-code-goat/models') {
        body = const <Object>[];
      } else {
        body = {'error': '接口不存在: $path'};
        return http.Response(jsonEncode(body), 404,
            headers: {'content-type': 'application/json'});
      }
      return http.Response(jsonEncode(body), 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });

/// 手机形态（Android）：判据只看 `useCollapsingPageTitle`，不按窗口宽度。
///
/// ⚠ 这个 foundation 调试变量必须在**用例体内**清回 null：`flutter_test` 在
/// 用例结束时（`addTearDown` 之前）就会检查"测试有没有留下改过的调试变量"。
Future<void> _asPhone(Future<void> Function() body) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.android;
  try {
    await body();
  } finally {
    debugDefaultTargetPlatformOverride = null;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('设置子页：滚动后大标题收起、AppBar 标题补上', (tester) {
    return _asPhone(() async {
      _phoneView(tester);

      await tester.pumpWidget(
        MaterialApp(
          home: SettingsScaffold(
            title: '外观与布局',
            sections: [
              for (var i = 0; i < 12; i++)
                SettingsSection(
                  title: '第 $i 块',
                  child: Card(child: SizedBox(height: 200, child: Text('内容 $i'))),
                ),
            ],
          ),
        ),
      );
      await tester.pumpAndSettle();

      // 顶部：只有正文里的大标题（AppBar 里那个在进度 0 时整个不建）；
      // 返回键在 AppBar 里，不在正文页头。
      expect(find.byType(AppBar), findsOneWidget);
      expect(find.text('外观与布局'), findsOneWidget);
      expect(
        find.descendant(of: find.byType(AppBar), matching: find.byType(BackButton)),
        findsOneWidget,
        reason: '返回键必须待在 AppBar 里',
      );

      // 往下滚过 fadeDistance（56）→ 大标题块的高度要收掉，AppBar 标题出现。
      await tester.drag(find.byType(SingleChildScrollView), const Offset(0, -120));
      await tester.pumpAndSettle();

      final collapsing = find.byType(CollapsingPageTitle);
      expect(collapsing, findsOneWidget);
      expect(tester.getSize(collapsing).height, 0,
          reason: '大标题滚走之后不许在页首留一条空白');
      expect(find.text('外观与布局'), findsNWidgets(2),
          reason: '正文里那份已透明，AppBar 里补上同一个名字');

      // 滚回顶部：AppBar 标题收回，正文大标题回来。
      await tester.drag(find.byType(SingleChildScrollView), const Offset(0, 200));
      await tester.pumpAndSettle();
      expect(find.text('外观与布局'), findsOneWidget);
      expect(tester.getSize(collapsing).height, greaterThan(0));
    });
  });

  testWidgets('AI 工具权限：AppBar 在、返回键在栏里、正文在状态栏下面', (tester) {
    return _asPhone(() async {
      _phoneView(tester);

      await tester.pumpWidget(
        MaterialApp(
          home: AiToolsSettingsPage(
            api: AiApiClient('http://127.0.0.1:8080', client: _toolsBackend()),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.byType(AppBar), findsOneWidget,
          reason: '没有 AppBar 时页头会顶进状态栏');
      expect(
        find.descendant(of: find.byType(AppBar), matching: find.byType(BackButton)),
        findsOneWidget,
        reason: '和其它设置子页一样，返回键在 AppBar 里',
      );
      // 正文（大标题块）必须落在状态栏 + AppBar 下面。
      final appBarBottom = tester.getBottomLeft(find.byType(AppBar)).dy;
      expect(appBarBottom, greaterThanOrEqualTo(_statusBar));
      final bodyTop = tester.getTopLeft(find.byType(CollapsingPageTitle)).dy;
      expect(bodyTop, greaterThanOrEqualTo(appBarBottom),
          reason: '正文不许盖到状态栏 / 顶栏上');

      // 手机端大标题就是页面名，滚动后同一套联动要生效。
      expect(find.text('AI 工具权限'), findsOneWidget);
      await tester.drag(find.byType(SingleChildScrollView), const Offset(0, -120));
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(CollapsingPageTitle)).height, 0);
      expect(find.text('AI 工具权限'), findsNWidgets(2));
    });
  });

  testWidgets('AI 模型与凭据：列表与详情都有自己的 AppBar，详情标题是 Provider 名', (tester) {
    return _asPhone(() async {
      _phoneView(tester);

      await tester.pumpWidget(
        MaterialApp(
          home: AiProviderSettingsPage(
            api: AiApiClient('http://127.0.0.1:8081', client: _providerBackend()),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.byType(AppBar), findsOneWidget);
      expect(
        find.descendant(of: find.byType(AppBar), matching: find.byType(BackButton)),
        findsOneWidget,
        reason: '列表这一层是"退出整个页面"，返回键在 AppBar 里',
      );
      expect(
        tester.getTopLeft(find.byType(CollapsingPageTitle)).dy,
        greaterThanOrEqualTo(tester.getBottomLeft(find.byType(AppBar)).dy),
        reason: '正文不许盖到状态栏 / 顶栏上',
      );

      // 点一条 = 进详情子界面：标题换成这条 Provider，返回键**还在 AppBar 里**
      // （以前它在正文页头里，而且详情正文还会再重复一次同名标题）。
      await tester.tap(find.text('Command Code Goat'));
      await tester.pumpAndSettle();

      expect(find.byType(AppBar), findsOneWidget);
      expect(
        find.descendant(of: find.byType(AppBar), matching: find.byType(BackButton)),
        findsOneWidget,
      );
      expect(find.text('Command Code Goat'), findsOneWidget,
          reason: '页面大标题是 Provider 名，正文里不许再重复一行');
      expect(find.text('新建 Provider'), findsNothing, reason: '进了详情就不该还留着列表');

      // 详情里的 `CustomScrollView` 也要接上同一套联动（这页的滚动在子组件里，
      // 进度由本骨架包住最外层滚动换算）。
      // 真机竖屏（853）上这段详情刚好一屏放得下、滚不动，所以把视口压矮一档
      // 再滚 —— 滚得动才验得到进度。
      // ⚠ 拖拽点要选**不吃纵向手势**的东西：落在 API Key 输入框上时，
      //    这一拖会被文本框自己的选择手势吃掉，页面根本不动。
      tester.view.physicalSize = const Size(384, 480);
      await tester.pumpAndSettle();
      await tester.drag(find.text('连接测试'), const Offset(0, -160));
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(CollapsingPageTitle)).height, 0);
      expect(find.text('Command Code Goat'), findsNWidgets(2));

      // 返回 → 列表（同一份 State 里换面板，不是路由弹出）。
      await tester.tap(
        find.descendant(of: find.byType(AppBar), matching: find.byType(BackButton)),
      );
      await tester.pumpAndSettle();
      expect(find.text('新建 Provider'), findsOneWidget);
      expect(find.text('AI 模型与凭据'), findsOneWidget);
    });
  });
}
