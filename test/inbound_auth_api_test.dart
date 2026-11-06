// 回归（配对令牌这条线，`docs/pairing-design.md` §11）：`lib/core/inbound_auth_api.dart`
// 是手机与桌面之间**唯一**一次交接秘密的地方，所以这里钉住的每条都是"错了以后不会报错"
// 的那一类：
//
//   ① `pair/request` 的响应里**不能**有 ticket —— 有它手机就能自己算出 6 位码，
//      "必须看到桌面屏幕"这条安全属性当场失效，而流程照样跑通、别的测试一个都不会红；
//   ② 长期令牌只在 `claim` 的响应里出现一次，解析错了就是"每次启动都要重配"；
//   ③ 错误信封 `{error, detail}` 必须映射成 `ApiException`：界面靠 `message` / `detail`
//      显示人话，靠状态码分清 410（会话失效，要重新举手）与 429（太频繁）；
//   ④ 密钥下行只认响应里真出现过的 ref（未授权的不报错、也绝不编一个空值出来）；
//   ⑤ `X-ComfyHub-Token`：null / 空串时**一个头都不发**，非空时每个请求都带 ——
//      桌面三条 e2e 与既有用例都不带任何头，这条形状不能被顺手改掉。

import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/api_client.dart' show ApiException;
import 'package:viewer/core/inbound_auth_api.dart';

/// 统一的 JSON 响应（**必须带 charset=utf-8**：`http.Response` 默认按 Latin-1 编码 body，
/// 中文 `error` / `detail` 会在 http 层就抛 "Contains invalid characters"）。
http.Response _json(Object? body, [int status = 200]) => http.Response(
  jsonEncode(body),
  status,
  headers: const {'content-type': 'application/json; charset=utf-8'},
);

String? _tokenOf(http.BaseRequest request) {
  for (final e in request.headers.entries) {
    if (e.key.toLowerCase() == 'x-comfyhub-token') return e.value;
  }
  return null;
}

void main() {
  test('pair/request 的响应不含 ticket：手机只拿得到 requestId（§11.1 的实现纪律）', () async {
    final claimBodies = <Map<String, dynamic>>[];
    final api = InboundAuthApi(
      'http://10.0.0.5:8081',
      client: MockClient((request) async {
        if (request.url.path == '/api/auth/pair/request') {
          // 刻意把 ticket 也塞进响应：手机侧**没有**承载它的字段 —— 解析出来只有 requestId，
          // 这正是"必须看到桌面屏幕"那条属性的形状（手机自己算不出码）。
          return _json({
            'requestId': 'req-1',
            'ticket': 'T-LEAK',
            'expiresAt': '2026-09-24T10:00:00Z',
            'codeExpiresAt': '2026-09-24T09:05:00Z',
            'serverTime': '2026-09-24T09:00:00Z',
          });
        }
        claimBodies.add(
          Map<String, dynamic>.from(jsonDecode(request.body) as Map),
        );
        return _json({'deviceId': 'phone-1', 'token': 'tok-1'});
      }),
    );

    final req = await api.requestPairing(
      deviceId: 'phone-1',
      deviceName: 'OnePlus 8T',
    );
    expect(req.requestId, 'req-1');
    expect(req.codeExpiresAt, '2026-09-24T09:05:00Z');

    await api.claim(
      code: '123456',
      deviceId: 'phone-1',
      deviceName: 'OnePlus 8T',
      requestId: req.requestId,
    );
    expect(claimBodies.single['requestId'], 'req-1');
    // 手机**不能**把 ticket 交回去（把它当 ticket 用 = 手机自己就能算码）
    expect(claimBodies.single.containsKey('ticket'), isFalse);
    expect(jsonEncode(claimBodies.single), isNot(contains('T-LEAK')));
  });

  test('claim：长期令牌必须被解析出来（它只在这个响应里出现这一次）', () async {
    late Map<String, dynamic> body;
    final api = InboundAuthApi(
      'http://10.0.0.5:8081',
      client: MockClient((request) async {
        body = Map<String, dynamic>.from(jsonDecode(request.body) as Map);
        return _json({
          'deviceId': 'phone-1',
          'token': 'tok-abc',
          'schemaVersion': 2,
          'serverTime': '2026-09-24T09:00:00Z',
        });
      }),
    );

    final claim = await api.claim(
      code: '428193',
      deviceId: 'phone-1',
      deviceName: 'OnePlus 8T',
      requestId: 'req-1',
    );

    expect(claim.token, 'tok-abc');
    expect(claim.deviceId, 'phone-1');
    expect(claim.schemaVersion, 2);
    expect(body['code'], '428193');
    expect(body['requestId'], 'req-1');
  });

  test(
    '错误信封 {error, detail} 映射成 ApiException：410=会话失效、429=太频繁、2xx 带 error 也算失败',
    () async {
      for (final (status, error) in const [
        (410, '配对会话已失效，请在电脑上重新发起'),
        (429, '请求太频繁，请稍后再试'),
        // ⚠ 灰度期桌面 App 连的还是 Kotlin 后端，而它那个 404 处理器会把状态码重置成 200
        // （`{"error":"接口不存在: …"}`）—— 不认它的话，`PUT /api/auth` 会"成功"返回一个
        // enabled=false 的状态，界面上的表现就是**开关弹回去、一个字都不解释**。
        (200, '接口不存在: /api/auth'),
      ]) {
        final api = InboundAuthApi(
          'http://10.0.0.5:8081',
          client: MockClient(
            (request) async =>
                _json({'error': error, 'detail': 'trace-$status'}, status),
          ),
        );
        await expectLater(
          api.claim(code: '000000', deviceId: 'p', deviceName: 'n'),
          throwsA(
            isA<ApiException>()
                .having((e) => e.statusCode, 'statusCode', status)
                // 人话不能被吞掉：界面上显示的就是它
                .having((e) => e.message, 'message', error)
                .having((e) => e.detail, 'detail', 'trace-$status'),
          ),
        );
      }
    },
  );

  test('fetchCredentials：只返回响应里真出现过的 ref（未授权的不报错、也不编）', () async {
    late Uri url;
    final api = InboundAuthApi(
      'http://10.0.0.5:8081',
      client: MockClient((request) async {
        url = request.url;
        return _json({
          'credentials': {'A_KEY': 'sk-a'},
        });
      }),
    );

    final got = await api.fetchCredentials([
      'A_KEY',
      'A_KEY',
      'B_KEY',
      '',
    ], token: 'tok-1');

    expect(got.keys.toList(), ['A_KEY']);
    expect(got['A_KEY'], 'sk-a');
    expect(got.containsKey('B_KEY'), isFalse, reason: '未授权的 ref 不该凭空出现');
    expect(
      url.queryParameters['refs'],
      'A_KEY,B_KEY',
      reason: '空 ref 不进查询串，重复的只发一次',
    );
  });

  test('X-ComfyHub-Token：token 为 null / 空串时一个头都不发，非空时每个请求都带', () async {
    final seen = <http.BaseRequest>[];
    final api = InboundAuthApi(
      'http://10.0.0.5:8081',
      client: MockClient((request) async {
        seen.add(request);
        return _json({'enabled': false, 'hasToken': false});
      }),
    );

    await api.status(); // GET，无令牌
    await api.status(token: ''); // GET，空令牌（与"没带"同义）
    await api.status(token: 'tok-1'); // GET，带令牌
    await api.setEnabled(true, token: 'tok-1'); // PUT（_send 那条路），带令牌

    expect(seen, hasLength(4));
    // 空令牌**不发这个头**：空串会被当成"带了个错的令牌"（403 vs 401 的诊断会指错方向）
    expect(_tokenOf(seen[0]), isNull);
    expect(_tokenOf(seen[1]), isNull);
    expect(_tokenOf(seen[2]), 'tok-1');
    expect(_tokenOf(seen[3]), 'tok-1');
    // 不带令牌时请求里不能多出任何自定义头（桌面 e2e 靠这一点保持"今天的行为"）
    expect(
      seen[0].headers.keys.map((k) => k.toLowerCase()),
      isNot(contains('x-comfyhub-token')),
    );
  });
}
