import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

/// **手机上不许画到屏幕外面去**（2026-09-24 真机验收抓到的两条溢出）。
///
/// # 这条用例在防什么
///
/// 真机（OnePlus 8T）实测「AI 模型与凭据」页上有两道黄黑斜纹：
///
/// * `RIGHT OVERFLOWED BY 2 PIXELS` —— "开关 + 说明 + 立即更新 + 校准思考档位"
///   挤在一个 `Row` 里；
/// * `BOTTOM OVERFLOWED BY 57 PIXELS` —— 两张目录卡 + 错误横幅都是**非弹性**子项，
///   加起来超过可用高度时外层 `Column` 没有余量可分。
///
/// 这两条在桌面上（窗口宽、高度够）**永远看不到**，而 `flutter analyze` 一个字都不说；
/// 所以防线只能是"**用手机尺寸把它挂起来**"。
///
/// # 为什么这样写

/// Flutter 在溢出时**会抛异常**（`A RenderFlex overflowed by …`），而 widget 用例
/// 里任何未捕获异常都会让用例红 —— 于是"有没有溢出"根本不用自己断言。
///
/// 视口取真机那一组：`1080x2400` 物理像素 / `2.8125` 缩放 = 逻辑 `384x853`
/// （`adb shell wm size` + `wm density 450`），刻意**不**用桌面尺寸，
/// 否则这个用例就只剩下"点得动"这一层保障了。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  /// 一个"什么都没搬过来"的后端：每个请求都回 `404 {error}`。
  ///
  /// 这正好复现真机上的状态 —— 灰度期的 Rust 后端对 `/api/ai/builtin/status` /
  /// `/api/ai/capabilities/status` 就是回这个（页面会把两句"读取失败"画在卡上，
  /// 而**那两行文字正是把高度顶爆的那部分**）。
  MockClient emptyBackend() => MockClient((request) async {
        final body = jsonEncode({'error': '接口不存在: ${request.url.path}'});
        return http.Response(body, 404, headers: {'content-type': 'application/json'});
      });

  /// 有**一条** Provider 的后端：用来验"手机上点一条 = 进子界面"。
  MockClient oneProviderBackend() => MockClient((request) async {
        Object body;
        if (request.url.path == '/api/ai/providers') {
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
        } else {
          body = {'error': '接口不存在: ${request.url.path}'};
          return http.Response(jsonEncode(body), 404,
              headers: {'content-type': 'application/json'});
        }
        return http.Response(jsonEncode(body), 200,
            headers: {'content-type': 'application/json'});
      });

  Future<void> pumpProviderPage(
    WidgetTester tester, {
    required Size logical,
    MockClient? backend,
  }) async {
    tester.view.physicalSize = Size(logical.width * 2.8125, logical.height * 2.8125);
    tester.view.devicePixelRatio = 2.8125;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final settings = SettingsStore();
    await tester.pumpWidget(
      MultiProvider(
        providers: [ChangeNotifierProvider<SettingsStore>.value(value: settings)],
        child: MaterialApp(
          home: AiProviderSettingsPage(
            api: AiApiClient('http://127.0.0.1:8081', client: backend ?? emptyBackend()),
          ),
        ),
      ),
    );
    // 让 initState 里那三发请求都落地（它们失败，正是要看的那个状态）
    await tester.pumpAndSettle();
  }

  testWidgets('手机尺寸下「AI 模型与凭据」页不溢出、也不抛异常（两道斜纹的防线）', (tester) async {
    await pumpProviderPage(tester, logical: const Size(384, 853));

    // 手机上是**三级面板**（用户原话："上下分屏难受，而且有些东西在手机上应该表现为子界面"）：
    // 一级 = Provider 列表，维护那两张卡**不在主屏上**，而是一个子界面。
    expect(find.text('AI 模型与凭据'), findsOneWidget);
    expect(find.text('新建 Provider'), findsOneWidget);
    expect(find.text('内置模型目录'), findsNothing, reason: '维护卡不该占主屏');
    expect(find.text('模型能力目录'), findsNothing);

    // 右上角扳手 → 维护子界面（整屏 + 返回箭头），展开的两张卡**照样不许溢出**
    await tester.tap(find.byTooltip('模型目录维护'));
    await tester.pumpAndSettle();
    expect(find.text('模型目录维护'), findsOneWidget);
    expect(find.byTooltip('返回'), findsOneWidget);
    expect(find.text('内置模型目录'), findsOneWidget);
    expect(find.text('模型能力目录'), findsOneWidget);
    expect(find.textContaining('接口不存在'), findsWidgets); // 如实写失败原因，不许静默

    // 返回 → 主屏
    await tester.tap(find.byTooltip('返回'));
    await tester.pumpAndSettle();
    expect(find.text('新建 Provider'), findsOneWidget);
  });

  testWidgets('手机上点一条 Provider = 进它的详情子界面（不是上下分屏）', (tester) async {
    await pumpProviderPage(
      tester,
      logical: const Size(384, 853),
      backend: oneProviderBackend(),
    );

    expect(find.text('Command Code Goat'), findsOneWidget);
    await tester.tap(find.text('Command Code Goat'));
    await tester.pumpAndSettle();

    // 子界面：标题就是那条 Provider，带返回箭头；列表已经不在屏幕上
    expect(find.byTooltip('返回'), findsOneWidget);
    expect(find.text('Command Code Goat'), findsWidgets);
    expect(find.text('新建 Provider'), findsNothing, reason: '进了详情就不该还留着列表');

    await tester.tap(find.byTooltip('返回'));
    await tester.pumpAndSettle();
    expect(find.text('新建 Provider'), findsOneWidget);
  });

  testWidgets('更窄的屏幕（320dp）同样不溢出', (tester) async {
    await pumpProviderPage(tester, logical: const Size(320, 640));
    expect(find.text('新建 Provider'), findsOneWidget);
    await tester.tap(find.byTooltip('模型目录维护'));
    await tester.pumpAndSettle();
    expect(find.text('内置模型目录'), findsOneWidget);
  });
}
