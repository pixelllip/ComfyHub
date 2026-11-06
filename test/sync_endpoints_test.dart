// 自动挑桌面（2026-09-30 用户要求：跨端这件事"改成自动处理而不用用户管理"）。
//
// 原来这件事是一页「同步与连接」：列候选、点一下切过去、手动测连接、手动上行。
// 现在只剩一个函数 `autoPickBaseUrl`，所以这一组用例就是那条路的回归线：
// 挑得对（顺序）、挑不到时**保持现状不清空**、以及"绝不猜地址"。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/sync_endpoints.dart';

/// 只有 `ok` 里的那些地址会回 200 的假后端。
MockClient _backend(
  Set<String> ok, {
  void Function(String base, String path)? onCall,
}) {
  return MockClient((request) async {
    final base =
        '${request.url.scheme}://${request.url.host}:${request.url.port}';
    onCall?.call(base, request.url.path);
    if (!ok.contains(base)) {
      return http.Response('connection refused', 503);
    }
    return http.Response(
      jsonEncode({
        'status': 'ok',
        'impl': 'rust',
        'version': '1.2.3',
        'database': 'sqlite',
        'schemaVersion': 7,
      }),
      200,
      headers: {'content-type': 'application/json'},
    );
  });
}

void main() {
  test('按"当前 → 自动发现 → 最近用过"的顺序挑，第一个通的就赢', () async {
    final called = <String>[];
    final picked = await autoPickBaseUrl(
      autoUrl: 'http://127.0.0.1:8080',
      current: 'http://10.0.2.2:8080',
      history: ['http://192.168.1.9:8080', 'http://100.64.0.7:8080'],
      client: _backend(
        {'http://192.168.1.9:8080'},
        onCall: (b, p) {
          if (p == '/api/health') called.add(b);
        },
      ),
    );

    // 前两条（当前 / 自动发现）都不通 → 第三条（历史里的局域网）通
    expect(picked.baseUrl, 'http://192.168.1.9:8080');
    expect(called, [
      'http://10.0.2.2:8080',
      'http://127.0.0.1:8080',
      'http://192.168.1.9:8080',
    ]);
    expect(picked.tried, 3);
    expect(picked.probe?.ok, isTrue, reason: '挑中那次的探测结论要留着（界面要显示版本/耗时）');
  });

  test('当前地址就通时只试一次（不为一条已知可用的地址白跑三条）', () async {
    final called = <String>[];
    final picked = await autoPickBaseUrl(
      autoUrl: 'http://127.0.0.1:8080',
      current: 'http://192.168.1.9:8080',
      history: const [],
      client: _backend(
        {'http://192.168.1.9:8080'},
        onCall: (b, p) {
          if (p == '/api/health') called.add(b);
        },
      ),
    );
    expect(picked.baseUrl, 'http://192.168.1.9:8080');
    expect(called.length, 1);
  });

  test('一条都不通 → baseUrl 为 null，并留下最后一次的真实原因（不许清空地址）', () async {
    final picked = await autoPickBaseUrl(
      autoUrl: 'http://127.0.0.1:8080',
      current: 'http://10.0.2.2:8080',
      client: _backend(const {}),
    );
    expect(picked.baseUrl, isNull, reason: '调用方据此保持现状，绝不把地址清成空的');
    expect(picked.tried, 2);
    expect(picked.probe?.ok, isFalse);
    expect(picked.probe?.error, isNotNull);
    expect(picked.summary, contains('都没连上'));
  });

  test('候选里去重、且顺序稳定（同一条别试两遍）', () async {
    final called = <String>[];
    final picked = await autoPickBaseUrl(
      autoUrl: 'http://192.168.1.9:8080',
      current: 'http://192.168.1.9:8080',
      history: ['http://192.168.1.9:8080', 'http://10.0.2.2:8080'],
      client: _backend(
        const {},
        onCall: (b, p) {
          if (p == '/api/health') called.add(b);
        },
      ),
    );
    expect(picked.baseUrl, isNull);
    expect(called, [
      'http://192.168.1.9:8080',
      'http://10.0.2.2:8080',
    ], reason: '去重后只试两条');
    expect(picked.candidates.length, 2);
  });

  test('地址不完整（没有 http://）时不发请求，直接如实报', () async {
    var calls = 0;
    final picked = await autoPickBaseUrl(
      autoUrl: '',
      current: '192.168.1.9:8080',
      client: _backend(const {}, onCall: (_, _) => calls++),
    );
    expect(picked.baseUrl, isNull);
    expect(calls, 0, reason: '连不上就是连不上，别为一个畸形地址发请求');
    expect(picked.probe?.error, contains('http://'));
  });
}
