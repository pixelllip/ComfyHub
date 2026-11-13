import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/api_client.dart';
import 'package:viewer/models/train_models.dart';
import 'package:viewer/pages/train_page.dart';

/// 训练台页面：**手机宽度不溢出** + 三种状态（没装 / 没起 / 离线）如实画出来。
///
/// WebView 本身不参与：这里验证外层容器 —— 在没有引擎、WebUI 未启动、
/// 后端连不上时看到的东西。真机 / Windows 的 WebView 渲染由 `flutter build` 把关。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  ApiClient statusClient(Map<String, dynamic> status, {int code = 200}) {
    return ApiClient(
      'http://127.0.0.1:8240',
      client: MockClient((request) async {
        if (request.url.path == '/api/train/status') {
          return http.Response(
            jsonEncode(status),
            code,
            headers: {'content-type': 'application/json; charset=utf-8'},
          );
        }
        return http.Response(
          jsonEncode({'error': 'not found'}),
          404,
          headers: {'content-type': 'application/json'},
        );
      }),
    );
  }

  Map<String, dynamic> statusJson({
    required bool installed,
    required bool daemonUp,
    required bool webuiUp,
    String? webuiPort,
    List<String> problems = const [],
  }) {
    return {
      'installed': installed,
      'engineDir': installed ? r'D:\myProject\anima-lora-next' : null,
      'python': r'D:\myProject\anima-lora-next\.venv\Scripts\python.exe',
      'webuiSource': installed ? r'D:\myProject\anima-lora-next' : null,
      'daemon': {
        'up': daemonUp,
        'port': daemonUp ? 8765 : null,
        'url': daemonUp ? 'http://127.0.0.1:8765' : null,
      },
      'webui': {
        'up': webuiUp,
        'port': webuiPort,
        'url': webuiUp ? 'http://127.0.0.1:$webuiPort' : null,
        'token': webuiUp ? 'tok' : null,
      },
      'projects': 0,
      'problems': problems,
    };
  }

  Future<void> pump(WidgetTester tester, ApiClient client, Size logical) async {
    tester.view.physicalSize = Size(logical.width * 2, logical.height * 2);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(home: TrainPage(apiFactory: (_) => client)),
    );
    await tester.pumpAndSettle();
  }

  test('状态 JSON 的端口 / 在线标志按后端原样解析', () {
    final status = TrainEngineStatus.fromJson(
      statusJson(
        installed: true,
        daemonUp: true,
        webuiUp: true,
        webuiPort: '2746',
      ),
    );
    expect(status.installed, isTrue);
    expect(status.daemon.up, isTrue);
    expect(status.daemon.port, 8765);
    expect(status.webui.port, 2746);
    expect(status.webui.token, 'tok');
    expect(status.ready, isTrue);
  });

  testWidgets('手机宽度：没装引擎时显示说明，不溢出', (tester) async {
    await pump(
      tester,
      statusClient(
        statusJson(
          installed: false,
          daemonUp: false,
          webuiUp: false,
          problems: ['没有找到 anima-lora-next 引擎'],
        ),
      ),
      const Size(384, 853),
    );
    expect(find.text('还没装训练引擎'), findsOneWidget);
    expect(find.textContaining('没有找到 anima-lora-next 引擎'), findsOneWidget);
    expect(find.text('重新探测'), findsOneWidget);
  });

  testWidgets('桌面宽度：装了但 WebUI 没起时给启动按钮', (tester) async {
    await pump(
      tester,
      statusClient(statusJson(installed: true, daemonUp: true, webuiUp: false)),
      const Size(1280, 800),
    );
    expect(find.text('训练引擎已安装，WebUI 没起'), findsOneWidget);
    expect(find.text('启动训练台'), findsOneWidget);
    expect(
      find.textContaining(r'D:\myProject\anima-lora-next'),
      findsOneWidget,
    );
  });

  testWidgets('后端连不上时如实报错并可重试', (tester) async {
    final client = ApiClient(
      'http://127.0.0.1:8240',
      client: MockClient((request) async => http.Response('boom', 500)),
    );
    await pump(tester, client, const Size(384, 853));
    expect(find.text('连不上训练引擎'), findsOneWidget);
    expect(find.text('重试'), findsOneWidget);
  });

  // ---- WebView 的导航判据（2026-10-23 从 inappwebview 换到 webview_all） ----
  //
  // 这一组钉的是"哪些导航留在 WebView、哪些交给系统浏览器"。旧实现用
  // `startsWith` 判同源，端口前缀相同的地址会被误吃进 WebView（8240 vs 82401）。
  group('训练台导航判据', () {
    const base = 'http://127.0.0.1:8240/?token=abc';

    test('同源（含不同 path / query）留在 WebView', () {
      for (final url in [
        'http://127.0.0.1:8240/',
        'http://127.0.0.1:8240/studio/projects',
        'http://127.0.0.1:8240/?token=xyz',
      ]) {
        expect(
          decideTrainNavigation(
            baseUrl: base,
            targetUrl: url,
            isMainFrame: true,
          ),
          TrainLinkAction.allow,
          reason: url,
        );
      }
    });

    test('端口只是前缀相同也不算同源（旧 startsWith 的坑）', () {
      expect(
        decideTrainNavigation(
          baseUrl: 'http://127.0.0.1:8240/',
          targetUrl: 'http://127.0.0.1:82401/',
          isMainFrame: true,
        ),
        TrainLinkAction.external,
      );
    });

    test('外链交给系统浏览器', () {
      expect(
        decideTrainNavigation(
          baseUrl: base,
          targetUrl: 'https://github.com/',
          isMainFrame: true,
        ),
        TrainLinkAction.external,
      );
    });

    test('子 frame 一律放行（studio 图片 / iframe 不能被拦）', () {
      expect(
        decideTrainNavigation(
          baseUrl: base,
          targetUrl: 'https://example.com/pic.png',
          isMainFrame: false,
        ),
        TrainLinkAction.allow,
      );
    });

    test('about:blank / data: / blob: 是 WebView 内部导航，不许踢去浏览器', () {
      for (final url in [
        'about:blank',
        'data:text/html,hi',
        'blob:http://127.0.0.1:8240/3f2a',
      ]) {
        expect(
          decideTrainNavigation(
            baseUrl: base,
            targetUrl: url,
            isMainFrame: true,
          ),
          TrainLinkAction.allow,
          reason: url,
        );
      }
    });
  });
}
