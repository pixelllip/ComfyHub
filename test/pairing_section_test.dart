// 回归（配对设置卡，`docs/pairing-design.md` §11）：这块界面最容易悄悄做错的两件事 ——
//
//   ① 开关**照请求画**而不是**照后端回的画**：后端回 `ok:false`（enabled 仍是 false）时
//      界面却显示"已打开"，用户以为开了、手机却连不上，界面上一点线索都没有；
//   ② 待配对请求必须画成**能念出来的 6 位码 + 来源 + 剩余时间**：这一块的全部价值就是
//      "人看着桌面屏幕把码敲进手机"，码画不出来（或藏在收起的折叠里）等于配对做不了。
//
// 手机那一支的判据是 `Platform.isAndroid`（测试进程恒为 false），所以这里只覆盖桌面支；
// 手机支的行为由 `test/inbound_auth_api_test.dart` 与 `test/ai_offline_source_test.dart`
// 从 API / 令牌接线两侧钉住。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/inbound_auth_api.dart';
import 'package:viewer/pages/pairing_section.dart';

http.Response _json(Object? body) => http.Response(
      jsonEncode(body),
      200,
      headers: const {'content-type': 'application/json; charset=utf-8'},
    );

Widget _host({required MockClient client}) => MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: PairingSection(
            baseUrl: 'http://127.0.0.1:8081',
            apiFactory: (base) => InboundAuthApi(base, client: client),
          ),
        ),
      ),
    );

Finder _masterSwitch() => find.ancestor(
      of: find.text('允许移动端连接'),
      matching: find.byType(SwitchListTile),
    );

void main() {
  testWidgets('桌面：后端没接受开关时，开关仍是"关"，并且如实说为什么', (tester) async {
    tester.view.physicalSize = const Size(900, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final client = MockClient((request) async {
      if (request.method == 'PUT') {
        // 后端**没有接受**：闸门还是关的（旧构建 / 拒绝时就是这样，
        // 而且 HTTP 是 200 —— 照"我请求成了"来画的话，这里不会有任何报错）
        return _json({
          'ok': false,
          'enabled': false,
          'hasToken': false,
          'devices': <Object>[],
          'pending': <Object>[],
        });
      }
      return _json({
        'enabled': false,
        'hasToken': false,
        'devices': <Object>[],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    expect(tester.widget<SwitchListTile>(_masterSwitch()).value, isFalse);

    await tester.tap(_masterSwitch());
    await tester.pump();
    await tester.pump();

    expect(
      tester.widget<SwitchListTile>(_masterSwitch()).value,
      isFalse,
      reason: '后端回 ok:false 时绝不能画成"已打开"',
    );
    expect(find.textContaining('后端没有接受'), findsOneWidget, reason: '要如实说，不能静默');

    // 收尾：把定时器一起随 State 销毁（否则测试结束时会报"还有 timer 没取消"）
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：待配对请求画成 6 位码 + 来源 + 剩余时间；设备行带授权开关与吊销', (tester) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final expire = DateTime.now().toUtc().add(const Duration(minutes: 3)).toIso8601String();
    final pending = {
      'requestId': 'req-1',
      'code': '428193',
      'ip': '10.44.33.71',
      'deviceName': 'OnePlus 8T',
      'codeExpiresAt': expire,
      'expiresAt': expire,
    };
    final device = {
      'id': 'phone-1',
      'name': 'OnePlus 8T',
      'pairedAt': '2026-09-24T08:00:00Z',
      'lastSeenAt': '2026-09-24T09:00:00Z',
      'lastIp': '10.44.33.71',
      'allowCredentials': false,
    };

    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': [pending]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': [device],
        'pending': [pending],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    // 有人举手了就必须看得见码（自动展开）：藏在收起的折叠里 = 桌面上找不到码
    final codes = tester
        .widgetList<SelectableText>(find.byType(SelectableText))
        .map((w) => w.data)
        .toList();
    expect(codes, contains('428193'), reason: '6 位码要能选中、能念出来');
    expect(find.textContaining('OnePlus 8T'), findsWidgets);
    expect(find.textContaining('剩余'), findsOneWidget);
    expect(find.textContaining('10.44.33.71'), findsWidgets);

    // 已配对设备行：设备名 / 最后活跃 / 最后 IP / 授权开关（默认关）/ 吊销
    expect(find.text('允许把密钥发给这台设备'), findsOneWidget);
    final allow = find.ancestor(
      of: find.text('允许把密钥发给这台设备'),
      matching: find.byType(SwitchListTile),
    );
    expect(tester.widget<SwitchListTile>(allow).value, isFalse, reason: '默认不允许下行密钥');
    expect(find.textContaining('手机拿不到桌面上的 API Key'), findsOneWidget);
    expect(find.text('吊销'), findsOneWidget);

    // 收尾：销毁 State（连同 2 秒轮询的定时器）
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });
}
