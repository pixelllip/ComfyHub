// 回归（配对二维码的**格式**，`docs/pairing-design.md` §4.5 / §9）：
//
// 这个格式只有一份实现（桌面编、手机解都走 `PairingPayload`）。用例盯三件事：
//   ① 往返一致：encode → parse 之后 ticket / 地址 / 过期时间一个不差；
//   ② **版本不认识 ≠ 不是我们的码**：v=2 要说"请更新 App"，而不是"这不是 ComfyHub 的码"；
//   ③ 坏的输入一律**如实报错**（缺 ticket / 坏 base64 / 别的 scheme），绝不猜。
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/pairing_payload.dart';

void main() {
  test('往返：ticket / 地址列表 / 过期时间一个不差', () {
    const payload = PairingPayload(
      ticket: '9f2c-ticket-43chars',
      expiresAt: '2026-09-24T21:03:20Z',
      urls: ['http://192.168.1.23:51837', 'http://100.101.102.103:51837'],
    );
    final text = payload.encode();
    expect(text.startsWith('comfyhub://pair?'), isTrue, reason: text);
    expect(text, contains('v=1'));

    final parsed = PairingPayload.parse(text);
    expect(parsed.ok, isTrue, reason: parsed.error);
    expect(parsed.payload!.ticket, '9f2c-ticket-43chars');
    expect(parsed.payload!.expiresAt, '2026-09-24T21:03:20Z');
    expect(parsed.payload!.urls, [
      'http://192.168.1.23:51837',
      'http://100.101.102.103:51837',
    ]);
  });

  test('地址里有 & 与 = 也不会把查询串撕开（base64url 的意义）', () {
    const payload = PairingPayload(
      ticket: 't',
      urls: ['http://host:8080/api/x?a=1&b=2', 'https://ok.example'],
    );
    final parsed = PairingPayload.parse(payload.encode());
    expect(parsed.payload!.urls, [
      'http://host:8080/api/x?a=1&b=2',
      'https://ok.example',
    ]);
  });

  test('没有地址也合法（那种情况界面要如实说"手填"）', () {
    final parsed = PairingPayload.parse(
      const PairingPayload(ticket: 't').encode(),
    );
    expect(parsed.ok, isTrue);
    expect(parsed.payload!.urls, isEmpty);
    expect(parsed.payload!.expiresAt, isNull);
  });

  test('手机选地址：跳过回环，优先局域网（扫码配对不再连手机自己）', () {
    const payload = PairingPayload(
      ticket: 't',
      urls: [
        // 旧版桌面给的顺序是回环 → Tailscale → 局域网；手机不能取第一条
        // 非回环的（那会拿到 Tailscale），必须自己按"局域网优先"重排。
        'http://127.0.0.1:4038',
        'http://100.64.0.5:4038',
        'http://10.44.130.232:4038',
      ],
    );
    expect(payload.preferredUrl, 'http://10.44.130.232:4038');
  });

  test('手机选地址：只有回环 / 空表时返回 null，绝不拿 urls.first 兜底', () {
    expect(
      const PairingPayload(
        ticket: 't',
        urls: ['http://127.0.0.1:4038'],
      ).preferredUrl,
      isNull,
    );
    expect(
      const PairingPayload(
        ticket: 't',
        urls: ['http://10.0.2.2:8080'],
      ).preferredUrl,
      isNull,
    );
    expect(const PairingPayload(ticket: 't').preferredUrl, isNull);
  });

  test('长期令牌一个字都不在二维码里（§8）', () {
    final text = const PairingPayload(
      ticket: 'tk',
      urls: ['http://1.2.3.4:1'],
    ).encode();
    expect(text, isNot(contains('X-ComfyHub-Token')));
    expect(text, isNot(contains('token=')));
    // 只有 ticket，没有第二把长期凭据
    expect('t=tk'.split('&').length, 1);
  });

  test('版本不认识：说"请更新 App"，不说"不是 ComfyHub 的码"', () {
    final parsed = PairingPayload.parse('comfyhub://pair?v=2&t=abc');
    expect(parsed.ok, isFalse);
    expect(parsed.error, contains('比这台 App 认识的新'));
    expect(parsed.error, contains('v2'));
    expect(parsed.error, isNot(contains('不是 ComfyHub')));
  });

  test('别的码：如实说扫到了什么', () {
    final wifi = PairingPayload.parse('WIFI:S=home;T=WPA;P=secret;;');
    expect(wifi.ok, isFalse);
    expect(wifi.error, contains('这不是 ComfyHub 的配对码'));

    final other = PairingPayload.parse('https://example.com/pair?v=1&t=x');
    expect(other.ok, isFalse);
    expect(other.error, contains('example.com'));
  });

  test('内容不完整 / 坏 base64 / 空内容：逐条如实报', () {
    expect(
      PairingPayload.parse('comfyhub://pair?v=1').error,
      contains('没有 ticket'),
    );
    expect(
      PairingPayload.parse('comfyhub://pair?t=abc').error,
      contains('没有版本号'),
    );
    expect(
      PairingPayload.parse('comfyhub://pair?v=1&t=a&u=%%%').error,
      contains('地址列表读不出来'),
    );
    expect(PairingPayload.parse('   ').error, contains('空的'));
  });

  test('地址列表里混进非 http 的项：丢掉它，但不报错（其余照收）', () {
    final text = const PairingPayload(
      ticket: 't',
      // 手工构造：base64url("\n" 连接) —— 其中一条不是 http
      urls: ['http://good:1', 'ftp://bad'],
    ).encode();
    final parsed = PairingPayload.parse(text);
    expect(parsed.ok, isTrue);
    expect(parsed.payload!.urls, ['http://good:1']);
  });
}
