// 「同步与连接」页（AIH 跨端）：候选地址分类、测试连接的真实结论、上行读数。
//
// 这一页的价值全在"如实"两个字上，所以用例盯的也是这三条：
//  ① 分类只按 IP 形状（回环 / 局域网 / Tailscale），不许按"猜"；
//  ② 测试连接**只由真实请求决定**：连不上要说清为什么，回的不是 ComfyHub 也要说；
//  ③ 上行读数不许假装：读不到就显示读不到，推了几条就报几条。
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/settings_store.dart';
import 'package:viewer/core/sync_endpoints.dart';
import 'package:viewer/pages/sync_settings_page.dart';

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('地址分类（只用于显示）', () {
    test('回环：localhost / 127.0.0.1 / 模拟器那三个特殊地址', () {
      expect(classifyEndpoint('http://127.0.0.1:8080'), SyncEndpointKind.loopback);
      expect(classifyEndpoint('http://localhost:8080'), SyncEndpointKind.loopback);
      expect(classifyEndpoint('http://10.0.2.2:8080'), SyncEndpointKind.loopback);
    });

    test('局域网：10/8、172.16-31、192.168', () {
      expect(classifyEndpoint('http://192.168.1.9:8080'), SyncEndpointKind.lan);
      expect(classifyEndpoint('http://10.1.2.3:8080'), SyncEndpointKind.lan);
      expect(classifyEndpoint('http://172.16.5.5:8080'), SyncEndpointKind.lan);
      expect(classifyEndpoint('http://172.32.5.5:8080'), SyncEndpointKind.other);
    });

    test('Tailscale：CGNAT 100.64.0.0/10', () {
      expect(classifyEndpoint('http://100.64.0.5:8080'), SyncEndpointKind.tailscale);
      expect(classifyEndpoint('http://100.127.255.1:8080'), SyncEndpointKind.tailscale);
      // 100.128 已经出了 CGNAT 段
      expect(classifyEndpoint('http://100.128.0.1:8080'), SyncEndpointKind.other);
    });

    test('其余一律"其他"，不许硬塞进某一类', () {
      expect(classifyEndpoint('http://169.254.1.1:8080'), SyncEndpointKind.other);
      expect(classifyEndpoint('http://example.com:8080'), SyncEndpointKind.other);
      expect(classifyEndpoint(''), SyncEndpointKind.other);
    });
  });

  group('本机候选', () {
    test('顺序：当前使用 → 自动发现 → 最近用过，且按 url 去重', () {
      final list = localEndpointCandidates(
        autoUrl: 'http://127.0.0.1:8080',
        current: 'http://192.168.1.9:8080',
        history: const ['http://127.0.0.1:8080', 'http://100.64.0.5:8080'],
      );
      expect(list.map((e) => e.url).toList(), [
        'http://192.168.1.9:8080',
        'http://127.0.0.1:8080',
        'http://100.64.0.5:8080',
      ]);
      expect(list[0].source, '当前使用');
      expect(list[1].source, '自动发现'); // 与历史重复时保留先出现的那条
      expect(list[2].kind, SyncEndpointKind.tailscale);
    });
  });

  group('测试连接', () {
    test('健康检查通了才叫通，并把后端自报的版本读出来', () async {
      final client = MockClient((req) async {
        if (req.url.path == '/api/health') {
          return http.Response(
            jsonEncode({
              'status': 'ok',
              'version': '0.2.0',
              'database': 'ok',
              'storageDir': 'D:/x/storage',
              'impl': 'rust',
              'schemaVersion': 3,
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        }
        if (req.url.path == '/api/sync/manifest') {
          return http.Response(
            jsonEncode({'schemaVersion': 3, 'kitVersion': '2026-09-27.2', 'cursor': 42}),
            200,
          );
        }
        return http.Response('not found', 404);
      });
      final r = await probeSyncEndpoint('http://192.168.1.9:8080', client: client);
      expect(r.ok, isTrue);
      expect(r.impl, 'rust');
      expect(r.schemaVersion, 3);
      expect(r.kitVersion, '2026-09-27.2');
      expect(r.summary, contains('连接正常'));
      expect(r.summary, contains('schema v3'));
    });

    test('manifest 要令牌（401）不算地址错：照样算通，但如实说明', () async {
      // 注意：这里的响应体用 ASCII —— `http.Response(String, …)` 没有
      // `charset=utf-8` 时按 latin1 编码，中文会**当场抛异常**（那会让用例
      // 变成"在测 MockClient"而不是在测我们的判据）。
      final client = MockClient((req) async {
        if (req.url.path == '/api/health') {
          return http.Response(jsonEncode({'status': 'ok', 'database': 'ok'}), 200);
        }
        return http.Response(jsonEncode({'error': 'token required'}), 401);
      });
      final r = await probeSyncEndpoint('http://127.0.0.1:8080', client: client);
      expect(r.ok, isTrue);
      expect(r.note, contains('配对'));
    });

    test('HTTP 500 要如实报出码，不许说"连上了"', () async {
      final client = MockClient((_) async => http.Response('boom', 500));
      final r = await probeSyncEndpoint('http://127.0.0.1:8080', client: client);
      expect(r.ok, isFalse);
      expect(r.error, contains('HTTP 500'));
      expect(r.summary, contains('连不上'));
    });

    test('200 但不是 ComfyHub：直说"回的不是 ComfyHub 后端"', () async {
      final client = MockClient((_) async => http.Response(jsonEncode({'hello': 'world'}), 200));
      final r = await probeSyncEndpoint('http://127.0.0.1:8080', client: client);
      expect(r.ok, isFalse);
      expect(r.error, contains('不是 ComfyHub'));
    });

    test('地址不以 http 开头：当场说清，不发请求', () async {
      var called = 0;
      final client = MockClient((_) async {
        called++;
        return http.Response('{}', 200);
      });
      final r = await probeSyncEndpoint('192.168.1.9:8080', client: client);
      expect(r.ok, isFalse);
      expect(called, 0);
      expect(r.error, contains('http://'));
    });
  });

  group('问对端要地址（/api/net/urls）', () {
    test('200：解析成候选，来源标"桌面报告"', () async {
      final client = MockClient((req) async {
        expect(req.url.path, '/api/net/urls');
        return http.Response(
          jsonEncode({
            'urls': ['http://192.168.1.9:8080', {'url': 'http://100.64.0.5:8080'}],
          }),
          200,
        );
      });
      final r = await discoveredEndpoints('http://127.0.0.1:8080', client: client);
      expect(r.endpoints.map((e) => e.url).toList(),
          ['http://192.168.1.9:8080', 'http://100.64.0.5:8080']);
      expect(r.endpoints.every((e) => e.source == '桌面报告'), isTrue);
      expect(r.note, isNull);
    });

    test('404（旧后端没有这条接口）不是错误：回空 + 一句如实说明', () async {
      final client = MockClient((_) async => http.Response('', 404));
      final r = await discoveredEndpoints('http://127.0.0.1:8080', client: client);
      expect(r.endpoints, isEmpty);
      expect(r.note, contains('旧版本'));
    });

    test('连不上就回空，绝不编地址', () async {
      final client = MockClient((_) async => throw const SocketExceptionLike());
      final r = await discoveredEndpoints('http://127.0.0.1:8080', client: client);
      expect(r.endpoints, isEmpty);
      expect(r.note, contains('失败'));
    });
  });

  group('页面', () {
    testWidgets('候选点一下就切过去并存进"最近用过"，测试连接显示真实结论', (tester) async {
      // 这一页是三张卡片摞起来，默认 800×600 的测试视口装不下第三张 ——
      // 而 `ListView` 是**懒构建**的：屏幕外的卡片根本不 build，
      // `find` 自然找不到它（这不是 bug，是懒构建该有的样子）。
      tester.view.physicalSize = const Size(1400, 2600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);

      final settings = SettingsStore();
      await settings.load();
      await settings.setBaseUrl('http://192.168.1.9:8080');

      final client = MockClient((req) async {
        if (req.url.path == '/api/net/urls') {
          return http.Response(
            jsonEncode({
              'urls': ['http://100.64.0.5:8080'],
            }),
            200,
          );
        }
        if (req.url.path == '/api/health') {
          return http.Response(
            jsonEncode({
              'status': 'ok',
              'version': '0.2.0',
              'database': 'ok',
              'impl': 'rust',
              'schemaVersion': 3,
            }),
            200,
          );
        }
        return http.Response(jsonEncode({'schemaVersion': 3, 'kitVersion': 'k1'}), 200);
      });

      await tester.pumpWidget(
        ChangeNotifierProvider<SettingsStore>.value(
          value: settings,
          child: MaterialApp(
            home: SyncSettingsPage(
              client: client,
              tokenProvider: () => 'tok',
              pendingProvider: () async => 7,
              syncRunner: (baseUrl, token) async =>
                  (ok: true, applied: 3, pending: 4, error: null),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // 候选：当前（局域网）、自动发现（回环）、桌面报告（Tailscale）
      expect(find.textContaining('局域网 · http://192.168.1.9:8080'), findsOneWidget);
      expect(find.textContaining('Tailscale · http://100.64.0.5:8080'), findsOneWidget);
      expect(find.text('待上行：7 条'), findsOneWidget);

      // 点 Tailscale 那条 → 地址切过去
      await tester.tap(find.textContaining('Tailscale · http://100.64.0.5:8080'));
      await tester.pumpAndSettle();
      expect(settings.baseUrl, 'http://100.64.0.5:8080');
      expect(settings.baseUrlHistory, contains('http://100.64.0.5:8080'));

      // 测试连接：真实打了一次 /api/health，结论是通
      expect(find.textContaining('连接正常'), findsOneWidget);
      expect(find.textContaining('待上行：7 条'), findsOneWidget);

      // 立即同步：报真实条数
      await tester.tap(find.text('立即同步'));
      await tester.pumpAndSettle();
      expect(find.textContaining('这次推上去 3 条，还剩 4 条'), findsOneWidget);
      expect(find.text('待上行：4 条'), findsOneWidget);
    });

    testWidgets('连不上时给红叉与原因，队列读不到就显示读不到', (tester) async {
      tester.view.physicalSize = const Size(1400, 2600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);

      final settings = SettingsStore();
      await settings.load();
      await settings.setBaseUrl('http://192.168.1.9:8080');

      final client = MockClient((req) async {
        if (req.url.path == '/api/net/urls') return http.Response('{"urls":[]}', 200);
        return http.Response('nope', 500);
      });

      await tester.pumpWidget(
        ChangeNotifierProvider<SettingsStore>.value(
          value: settings,
          child: MaterialApp(
            home: SyncSettingsPage(
              client: client,
              tokenProvider: () => null,
              pendingProvider: () async => throw StateError('库打不开'),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.textContaining('读不到'), findsWidgets);
      expect(find.textContaining('库打不开'), findsOneWidget);

      await tester.tap(find.text('测试连接'));
      await tester.pumpAndSettle();
      expect(find.textContaining('连不上：HTTP 500'), findsOneWidget);
      expect(find.byIcon(Icons.error_outline), findsOneWidget);
    });
  });
}

/// 一个"网络不可达"的替身：`MockClient` 的回调里抛异常就等价于传输层失败。
///
/// 用它而不是真的连一个不存在的端口，是为了让用例在任何机器上都稳定。
class SocketExceptionLike implements Exception {
  const SocketExceptionLike();
  @override
  String toString() => '连接被拒绝';
}
