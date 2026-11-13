// 回归（配对设置卡，`docs/pairing-design.md` §11）：这块界面最容易悄悄做错的两件事 ——
//
//   ① 开关**照请求画**而不是**照后端回的画**：后端回 `ok:false`（enabled 仍是 false）时
//      界面却显示"已打开"，而手机无法连接，界面上没有提示；
//   ② 待配对请求必须画成**能念出来的 6 位码 + 来源 + 剩余时间**：这一块的全部价值就是
//      "人看着桌面屏幕把码敲进手机"，码画不出来（或藏在收起的折叠里）等于配对做不了。
//
// 手机那一支的判据是 `Platform.isAndroid`（测试进程恒为 false），所以这里只覆盖桌面支；
// 手机支的行为由 `test/inbound_auth_api_test.dart` 与 `test/ai_offline_source_test.dart`
// 从 API / 令牌接线两侧钉住。

import 'dart:convert';

import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/inbound_auth_api.dart';
import 'package:viewer/core/pairing_payload.dart';
import 'package:viewer/pages/pairing_section.dart';
import 'package:viewer/widgets/pairing_qr.dart';

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
  // 「允许移动端连接」与「本地服务」那张卡里的「允许手机 / 局域网设备连接」
  // **不是同一件事**，两个名称都近似"允许手机连接"（
  // "是不是重复了"）：
  //   · 那个 = 后端**听不听**局域网（起进程时定，要重启）；
  //   · 这个 = **配对闸门**：局域网请求要不要带令牌。
  // 关掉这个不是"不许连"，而是"局域网内任何设备都能连、不用令牌"（更松）。
  // 两边都必须写出顺序，否则无法判断"已开启但手机连不上"发生在哪一步。
  testWidgets('闸门这一项要写明它与「本地服务」那个开关的关系（防"看着像重复"）', (tester) async {
    tester.view.physicalSize = const Size(900, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final client = MockClient(
      (request) async => _json({
        'enabled': true,
        'hasToken': true,
        'devices': <Object>[],
        'pending': <Object>[],
      }),
    );

    await tester.pumpWidget(_host(client: client));
    await tester.pumpAndSettle();

    expect(find.textContaining('② 已开启'), findsOneWidget);
    expect(
      find.textContaining('「本地服务」'),
      findsOneWidget,
      reason: '必须指出另一道闸在哪，否则两个开关看着就是重复的',
    );
  });

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

    final expire = DateTime.now()
        .toUtc()
        .add(const Duration(minutes: 3))
        .toIso8601String();
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
        return _json({
          'pending': [pending],
        });
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
    expect(
      tester.widget<SwitchListTile>(allow).value,
      isFalse,
      reason: '默认不允许下行密钥',
    );
    expect(find.textContaining('手机拿不到桌面上的 API Key'), findsOneWidget);
    expect(find.text('吊销'), findsOneWidget);

    // 收尾：销毁 State（连同 2 秒轮询的定时器）
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：密钥的两个开关分开 —— 勾上行不许把下行那个关掉，旧后端要如实说', (tester) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 下行已开启、上行仍关闭（接下来需勾选上行开关）
    var allow = true;
    var allowUpload = false;
    final bodies = <String>[];
    // 旧后端：响应里**没有** allowCredentialsUpload 这个键
    var oldBackend = false;

    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/devices/phone-1/credentials') {
        bodies.add(request.body);
        final sent = jsonDecode(request.body) as Map<String, dynamic>;
        allow = sent['allow'] as bool? ?? allow;
        if (sent.containsKey('allowUpload')) {
          allowUpload = sent['allowUpload'] as bool;
        }
        return _json({
          'id': 'phone-1',
          'name': 'OnePlus 8T',
          'allowCredentials': allow,
          if (!oldBackend) 'allowCredentialsUpload': allowUpload,
        });
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': [
          {
            'id': 'phone-1',
            'name': 'OnePlus 8T',
            'pairedAt': '2026-09-24T08:00:00Z',
            'allowCredentials': allow,
            if (!oldBackend) 'allowCredentialsUpload': allowUpload,
          },
        ],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    Finder switchOf(String title) => find.ancestor(
      of: find.text(title),
      matching: find.byType(SwitchListTile),
    );

    final down = switchOf('允许把密钥发给这台设备');
    final up = switchOf('允许这台设备改桌面上的密钥');
    expect(tester.widget<SwitchListTile>(down).value, isTrue);
    expect(tester.widget<SwitchListTile>(up).value, isFalse, reason: '上行默认关');

    // 勾上行：请求里必须有 allowUpload，**且下行那个当前值也照样带着**（后端按"传了就改"处理）
    await tester.tap(up);
    await tester.pumpAndSettle();
    expect(bodies.single, contains('"allowUpload":true'));
    expect(bodies.single, contains('"allow":true'));
    // ★ 本地重建设备行时**不许把下行那个开关擦掉**（漏了就是"点上行、下行被悄悄关掉"）
    expect(
      tester.widget<SwitchListTile>(down).value,
      isTrue,
      reason: '勾上行不该动到下行',
    );
    expect(tester.widget<SwitchListTile>(up).value, isTrue);

    // 收尾：销毁 State（连同 2 秒轮询的定时器）
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：旧后端不认识上行开关 → 不许假装开了，并说清为什么不生效', (tester) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    // 老后端：设备行与回执里**都没有** allowCredentialsUpload 这个键，
    // 因此"本地开启"与"回读为关"必然矛盾 —— 此时的处理方式是
    // 将开关显示为"关"并说明后端版本问题（不显示为开启）。
    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/devices/phone-1/credentials') {
        return _json({
          'id': 'phone-1',
          'name': 'OnePlus 8T',
          'allowCredentials': false,
        });
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': [
          {
            'id': 'phone-1',
            'name': 'OnePlus 8T',
            'pairedAt': '2026-09-24T08:00:00Z',
            'allowCredentials': false,
          },
        ],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    final up = find.ancestor(
      of: find.text('允许这台设备改桌面上的密钥'),
      matching: find.byType(SwitchListTile),
    );
    expect(tester.widget<SwitchListTile>(up).value, isFalse);

    await tester.tap(up);
    await tester.pumpAndSettle();

    expect(
      tester.widget<SwitchListTile>(up).value,
      isFalse,
      reason: '回读为 false 就不许显示成开',
    );
    expect(find.textContaining('这台后端还不支持'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：扫码配对的二维码里带的是 ticket + 地址，6 位码照样显示给人敲', (tester) async {
    tester.view.physicalSize = const Size(900, 2200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    var expand = DateTime.now()
        .toUtc()
        .add(const Duration(minutes: 2))
        .toIso8601String();
    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/pair/start') {
        return _json({
          'ticket': 'tk-42',
          'code': '428193',
          'expiresAt': expand,
          'codeExpiresAt': expand,
          'urls': ['http://192.168.1.23:51837', 'http://100.101.102.103:51837'],
        });
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': <Object>[],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    // 二维码在「配对新手机」那张展开区里（收起时不该占位）
    expect(find.byType(PairingQrImage), findsNothing);
    await tester.tap(find.text('配对新手机'));
    await tester.pump();

    await tester.tap(find.text('生成二维码'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    // ★ 画出来的二维码内容必须**能解回**同一份事实（不是随便画一个方块）
    final qr = tester.widget<PairingQrImage>(find.byType(PairingQrImage));
    final parsed = PairingPayload.parse(qr.payload.encode());
    expect(parsed.ok, isTrue, reason: parsed.error);
    expect(parsed.payload!.ticket, 'tk-42');
    expect(parsed.payload!.urls, [
      'http://192.168.1.23:51837',
      'http://100.101.102.103:51837',
    ]);
    // 长期令牌不许出现在二维码里
    expect(qr.payload.encode().contains('X-ComfyHub-Token'), isFalse);

    // 6 位码 + 地址照样显示（人敲码那一步不能省）
    expect(find.text('428193'), findsOneWidget);
    expect(find.textContaining('192.168.1.23:51837'), findsWidgets);
    // 2026-09-30 合并后只剩**一个**按钮：有码时它提示"重新生成二维码"
    //（原来二维码那条路另有一个「重新生成」TextButton，与"生成"按钮重复）
    expect(find.text('重新生成二维码'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：旧后端没有扫码接口 → 如实说，并指向手敲码那条路', (tester) async {
    tester.view.physicalSize = const Size(900, 2200);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/pair/start') {
        return http.Response(
          jsonEncode({'error': '接口不存在: /api/auth/pair/start'}),
          501,
          headers: const {'content-type': 'application/json; charset=utf-8'},
        );
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': <Object>[],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();
    await tester.tap(find.text('配对新手机'));
    await tester.pump();
    await tester.tap(find.text('生成二维码'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.byType(PairingQrImage), findsNothing);
    expect(find.textContaining('还没有"扫码配对"接口'), findsOneWidget);
    // ⚠ 文案已精简（「手机举手 + 敲码」→「手动敲验证码」）——
    //   这一条钉的是**指路**本身（不许只说失败、不说另一条路怎么走），不钉具体措辞。
    expect(find.textContaining('手动敲验证码'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：自动提交开关默认关，打开后走 auto-approve 且不擦掉别的开关', (tester) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    var auto = false;
    var allow = true; // 已允许取密钥：重建行时不许被擦掉
    final bodies = <String>[];

    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/devices/phone-1/auto-approve') {
        bodies.add(request.body);
        final sent = jsonDecode(request.body) as Map<String, dynamic>;
        auto = sent['enabled'] as bool? ?? auto;
        return _json({
          'id': 'phone-1',
          'name': 'OnePlus 8T',
          'allowCredentials': allow,
          'allowCredentialsUpload': false,
          'autoApproveSubmits': auto,
        });
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': [
          {
            'id': 'phone-1',
            'name': 'OnePlus 8T',
            'pairedAt': '2026-09-24T08:00:00Z',
            'allowCredentials': allow,
            'autoApproveSubmits': auto,
          },
        ],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    Finder switchOf(String title) => find.ancestor(
      of: find.text(title),
      matching: find.byType(SwitchListTile),
    );

    final autoSwitch = switchOf('允许这台手机自动提交（免桌面批准）');
    expect(tester.widget<SwitchListTile>(autoSwitch).value, isFalse);
    expect(find.textContaining('都要在桌面点一次批准'), findsOneWidget);

    await tester.tap(autoSwitch);
    await tester.pumpAndSettle();
    expect(bodies.single, contains('"enabled":true'));
    expect(tester.widget<SwitchListTile>(autoSwitch).value, isTrue);
    // ★ 打开自动提交时重建行，不许把「允许取密钥」那个开关擦掉
    expect(
      tester.widget<SwitchListTile>(switchOf('允许把密钥发给这台设备')).value,
      isTrue,
      reason: '重建行要原样带上已有的两个密钥开关',
    );
    // 打开后必须把代价写出来（不只是"更省事"）
    expect(find.textContaining('消耗 GPU'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('桌面：旧后端不认识自动提交 → 开关画回关，并说清是版本问题', (tester) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final client = MockClient((request) async {
      if (request.url.path == '/api/auth/devices/phone-1/auto-approve') {
        // 旧后端：回执里没有 autoApproveSubmits 这个键
        return _json({
          'id': 'phone-1',
          'name': 'OnePlus 8T',
          'allowCredentials': false,
        });
      }
      if (request.url.path == '/api/auth/pair/pending') {
        return _json({'pending': <Object>[]});
      }
      return _json({
        'enabled': true,
        'hasToken': true,
        'devices': [
          {
            'id': 'phone-1',
            'name': 'OnePlus 8T',
            'pairedAt': '2026-09-24T08:00:00Z',
            'allowCredentials': false,
          },
        ],
        'pending': <Object>[],
      });
    });

    await tester.pumpWidget(_host(client: client));
    await tester.pump();

    Finder switchOf(String title) => find.ancestor(
      of: find.text(title),
      matching: find.byType(SwitchListTile),
    );

    final autoSwitch = switchOf('允许这台手机自动提交（免桌面批准）');
    await tester.tap(autoSwitch);
    await tester.pumpAndSettle();

    expect(
      tester.widget<SwitchListTile>(autoSwitch).value,
      isFalse,
      reason: '回读没有这个键 = 没生效，不许假装开了',
    );
    expect(find.textContaining('还不支持"自动提交"'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });
}
