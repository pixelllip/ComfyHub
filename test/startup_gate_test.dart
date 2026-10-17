import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';
import 'package:viewer/state/nsfw_store.dart';

/// **冷启动闸门**：后端还没"有结论"之前，首屏一个请求都不许发（2026-10-12）。
///
/// 现场（用户贴的那句）：
///
/// ```
/// ClientException with SocketException: Connection attempt cancelled,
/// host: 127.0.0.1, port: 8080, uri=http://127.0.0.1:8080/api/ai/providers
/// ```
///
/// 两件事叠在一起：① `.run\api.json` 在上一次优雅退出时被后端删掉了，冷启动那一刻
/// `baseUrl` 只能是 8080 那个兜底值 —— 请求**注定失败**；② 后端 1 秒后就绪、地址一变，
/// store 把旧 client `dispose()` 掉，于是那个还在连 8080 的请求报的是 **cancelled**
/// 而不是 refused（这就是"连接尝试被取消"的来路）。
///
/// 修法是"等出结论再发"（`BackendLauncher.settled`）：闸门本身（等 / 超时放行 /
/// 失败也放行）在 `test\backend_launcher_test.dart`，这里钉的是**两个 store 有没有真的
/// 站在闸门后面** —— 把 `await backendReady?.call()` 删掉，这两条立刻红。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  /// 记录发出去的请求路径；返回的东西不重要（这两个用例只关心"发没发"）。
  ({MockClient client, List<String> paths}) recorder() {
    final paths = <String>[];
    final client = MockClient((request) async {
      paths.add(request.url.path);
      return http.Response('[]', 200, headers: {'content-type': 'application/json'});
    });
    return (client: client, paths: paths);
  }

  test('AI 工作台：闸门没开之前不发请求，开了才去拉 provider', () async {
    SharedPreferences.setMockInitialValues({});
    final rec = recorder();
    final gate = Completer<void>();
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080', client: rec.client),
      backendReady: () => gate.future,
    );
    addTearDown(store.dispose);

    final loading = store.load();
    await Future<void>.delayed(const Duration(milliseconds: 30));
    expect(rec.paths, isEmpty,
        reason: '后端还没起，这会儿发出去只能是打 8080 兜底地址（必失败，用户看到的就是那句 ClientException）');

    gate.complete();
    await loading;
    expect(rec.paths, contains('/api/ai/providers'),
        reason: '闸门开了之后必须真的去拉 —— 否则等于"等完什么都不做"');
  });

  test('NSFW 设置：同样站在闸门后面（读不到会静默停在默认值）', () async {
    final rec = recorder();
    final gate = Completer<void>();
    final store = NsfwStore(
      api: AiApiClient('http://127.0.0.1:8080', client: rec.client),
      backendReady: () => gate.future,
    );
    addTearDown(store.dispose);

    final loading = store.load(silent: true);
    await Future<void>.delayed(const Duration(milliseconds: 30));
    expect(rec.paths, isEmpty, reason: '一刻都不许提前打（打了必失败，而它会静默停在默认值上）');

    gate.complete();
    await loading;
    expect(rec.paths, contains('/api/ai/nsfw'));
  });

  testWidgets('AI 工作台右侧栏：闸门没开之前不打 ComfyUI 状态（那个失败会留在界面上）', (tester) async {
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();

    final paths = <String>[];
    final client = MockClient((request) async {
      paths.add(request.url.path);
      return http.Response('[]', 200, headers: {'content-type': 'application/json'});
    });
    final gate = Completer<void>();
    final store = AiWorkspaceStore(
      api: AiApiClient('http://127.0.0.1:8080', client: client),
      backendReady: () => gate.future,
    );

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<SettingsStore>.value(value: settings),
          ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
        ],
        child: const MaterialApp(home: AiHomePage()),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    // 右侧栏（1600 宽会铺开）一显示就打这三处 —— 冷启动那一刻它们都会打到 8080。
    // ⚠ 只看得见走 store 客户端的那两个：`/api/capture/status` 由这一栏自己 new 了一个
    // `ApiClient`（不经过注入的 MockClient），这一层观察不到 —— 但它与这两个在**同一个
    // await 之后**（见 `_ContextPanel.initState`），闸门挪走它一样会提前打。
    for (final path in ['/api/capture/jobs', '/api/ai/pending']) {
      expect(paths.where((p) => p == path), isEmpty, reason: '闸门没开就别打 $path');
    }

    gate.complete();
    // ⚠ 不能用 pumpAndSettle：这一栏起来之后有 Timer.periodic（实时进度 / 待审批）
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));
    expect(paths, contains('/api/capture/jobs'), reason: '闸门开了之后要真的去拉');
    expect(paths, contains('/api/ai/pending'));

    // 把页面拆掉，让那一栏 dispose（否则周期定时器会留到测试之外）
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
