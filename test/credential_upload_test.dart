// 回归（密钥上行，「推给桌面」——`docs/pairing-design.md` §14）：
//
//   手机上的 key 保存在本机密钥库，桌面那份是**另一份**。支持"双向"后，
//   手机上多了一个显式动作：把本机这一把推给桌面。这一批钉住四件事：
//   ① **上行请求的形状**（`PUT /api/ai/credentials` + 配对令牌 + `credentials` 映射）；
//   ② **两个开关分开**（`allow` 与 `allowUpload` 各自独立，谁都不许顺手带上另一个）；
//   ③ **`reveal` 是唯一读密钥的口子**：桌面侧（Remote）直接抛、空 ref 连通道都不打；
//   ④ 界面**如实**：本机没有这把 → 不发请求（别拿空串覆盖桌面上那把好的）；
//      回执里没有这一条 → 不当它成功；旧后端 404 → 直说"还没有这条接口"。
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/api_client.dart' show ApiException;
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/core/inbound_auth_api.dart';
import 'package:viewer/core/run_event_source.dart'
    show LocalHarnessRunEventSource;
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const harnessChannel = MethodChannel(
    LocalHarnessRunEventSource.methodChannelName,
  );
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  final store = <String, String>{};
  final channelCalls = <MethodCall>[];

  /// 让 `getCredential` 回 null 而 `hasCredential` 仍为 true ——
  /// 这是真实存在的一种时序：**检查时有、取的时候没了/解不开**（密钥库被清、条目坏了）。
  var hideRevealed = false;

  setUp(() {
    store.clear();
    channelCalls.clear();
    hideRevealed = false;
    messenger.setMockMethodCallHandler(harnessChannel, (call) async {
      channelCalls.add(call);
      final args = Map<String, dynamic>.from(
        (call.arguments as Map?) ?? const {},
      );
      switch (call.method) {
        case 'hasCredential':
          return store.containsKey(args['ref']);
        case 'getCredential':
          return hideRevealed ? null : store[args['ref']];
      }
      return null;
    });
  });

  tearDown(() => messenger.setMockMethodCallHandler(harnessChannel, null));

  AiProvider provider({String? ref = 'PHONE_KEY'}) => AiProvider(
    id: 'p1',
    displayName: '本机网关',
    api: 'openai-completions',
    baseURL: 'http://127.0.0.1:11434/v1',
    credentialRef: ref,
    endpointTrust: 'loopback',
    enabled: true,
    revision: 1,
    credential: const AiCredentialStatus(
      configured: true,
      source: 'keystore',
      writable: true,
    ),
  );

  group('上行请求的形状', () {
    test(
      'PUT /api/ai/credentials：带令牌、body 是 {credentials, rev}，回执逐条解析',
      () async {
        late http.Request seen;
        final api = InboundAuthApi(
          'http://192.168.1.9:8080',
          client: MockClient((request) async {
            seen = request;
            // 响应体用 ASCII：`http.Response(String, …)` 没有 charset 时按 latin1 编码，
            // 中文会当场抛异常（那样测的就不是我们的判据了）。
            return http.Response(
              '{"applied":["PHONE_KEY"],"rejected":[{"ref":"OTHER","code":"STALE_IGNORED","message":"stale"}],"rev":13}',
              200,
            );
          }),
        );

        final r = await api.uploadCredentials(
          {'PHONE_KEY': 'sk-1'},
          rev: 12,
          token: 'tok-1',
        );

        expect(seen.method, 'PUT');
        expect(seen.url.path, '/api/ai/credentials');
        expect(seen.headers['X-ComfyHub-Token'], 'tok-1');
        final body = seen.body;
        expect(body, contains('PHONE_KEY'));
        expect(body, contains('sk-1'));
        expect(body, contains('"rev":12'));
        expect(r.applied, ['PHONE_KEY']);
        expect(r.rejected.single.code, 'STALE_IGNORED');
        expect(r.rev, 13);
      },
    );

    test('不带 rev 时**不发这个键**（= 用户显式点的强制覆盖）', () async {
      late http.Request seen;
      final api = InboundAuthApi(
        'http://x',
        client: MockClient((request) async {
          seen = request;
          return http.Response('{"applied":["A"]}', 200);
        }),
      );
      await api.uploadCredentials({'A': 'v'});
      expect(seen.body.contains('"rev"'), isFalse);
    });

    test('旧后端 404 要抛 ApiException（界面据此说"还没有这条接口"）', () async {
      final api = InboundAuthApi(
        'http://x',
        client: MockClient((_) async => http.Response('', 404)),
      );
      await expectLater(
        () => api.uploadCredentials({'A': 'v'}),
        throwsA(
          isA<ApiException>().having((e) => e.statusCode, 'statusCode', 404),
        ),
      );
    });

    test('被拒的三种码各有各的人话（不许只说一个码）', () {
      expect(
        const CredentialRejection(ref: 'A', code: 'FORBIDDEN').hint,
        contains('没有允许这台设备改密钥'),
      );
      expect(
        const CredentialRejection(ref: 'A', code: 'STALE_IGNORED').hint,
        contains('先从桌面同步'),
      );
      expect(
        const CredentialRejection(
          ref: 'A',
          code: 'BAD_PAYLOAD',
          message: '名字太长',
        ).hint,
        contains('名字太长'),
      );
    });
  });

  group('两个开关分开（§14.1）', () {
    test('allowUpload=true 时两个键都在；不传时**不许**出现 allowUpload', () async {
      final bodies = <String>[];
      final api = InboundAuthApi(
        'http://x',
        client: MockClient((request) async {
          bodies.add(request.body);
          return http.Response(
            '{"id":"dev-1","name":"KB2000","allowCredentials":true,"allowCredentialsUpload":true}',
            200,
          );
        }),
      );

      final d = await api.setAllowCredentials('dev-1', true, allowUpload: true);
      expect(d.allowCredentials, isTrue);
      expect(d.allowCredentialsUpload, isTrue);
      expect(bodies.last.contains('allowUpload'), isTrue);

      await api.setAllowCredentials('dev-1', false);
      expect(
        bodies.last.contains('allowUpload'),
        isFalse,
        reason: '只动下行开关时多发一个键，旧后端会当成"用户没开上行"，那种静默最坑',
      );
    });

    test('旧后端没有 allowCredentialsUpload 字段 → 恒 false（不是"用户没开"）', () {
      final d = PairedDevice.fromJson(const {'id': 'dev-1', 'name': 'KB2000'});
      expect(d.allowCredentialsUpload, isFalse);
    });
  });

  group('reveal：唯一读密钥的口子', () {
    test('手机：走桥的 getCredential；空 ref 连通道都不打', () async {
      store['PHONE_KEY'] = 'sk-phone';
      final device = DeviceCredentialStore(channel: harnessChannel);

      expect(await device.reveal(provider()), 'sk-phone');
      expect(channelCalls.map((c) => c.method), contains('getCredential'));

      channelCalls.clear();
      expect(await device.reveal(provider(ref: '')), isNull);
      expect(channelCalls, isEmpty, reason: '没有 ref 就没有"这把"，不该白打一次通道');
    });

    test('桌面：直接抛（桌面自己就是真源）', () async {
      final remote = RemoteCredentialStore(
        AiApiClient('http://127.0.0.1:8080'),
      );
      await expectLater(() => remote.reveal(provider()), throwsStateError);
    });
  });

  group('界面：推给桌面', () {
    MockClient backend({
      int uploadStatus = 200,
      String uploadBody = '{"applied":["PHONE_KEY"]}',
    }) {
      return MockClient((request) async {
        final path = request.url.path;
        if (path == '/api/ai/credentials' && request.method == 'PUT') {
          return http.Response(uploadBody, uploadStatus);
        }
        final body = switch (path) {
          '/api/ai/providers' => [
            {
              'id': 'p1',
              'displayName': '本机网关',
              'api': 'openai-completions',
              'baseURL': 'http://127.0.0.1:11434/v1',
              'credentialRef': 'PHONE_KEY',
              'endpointTrust': 'loopback',
              'enabled': true,
              'revision': 1,
              'credential': {
                'configured': true,
                'source': 'keystore',
                'writable': true,
              },
            },
          ],
          _ => <Object>[],
        };
        return http.Response(
          _json(body),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      });
    }

    Future<Widget> page(MockClient client) async {
      SharedPreferences.setMockInitialValues({});
      final settings = SettingsStore();
      await settings.load();
      final api = AiApiClient(settings.baseUrl, client: client);
      return ChangeNotifierProvider<SettingsStore>.value(
        value: settings,
        child: MaterialApp(
          home: AiProviderSettingsPage(
            api: api,
            credentials: DeviceCredentialStore(channel: harnessChannel),
            authApiFactory: (base) => InboundAuthApi(base, client: client),
          ),
        ),
      );
    }

    testWidgets('确认之后才推；成功了要如实说推到哪一把', (tester) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);
      store['PHONE_KEY'] = 'sk-phone';

      await tester.pumpWidget(await page(backend()));
      await tester.pumpAndSettle();
      await tester.tap(find.text('本机网关'));
      await tester.pumpAndSettle();

      // 按钮必须是**可用**的：点一个 onPressed=null 的按钮不会报错，
      // 于是"没反应"这种失败会被误诊成"弹窗没出来"。
      final pushButton = find.widgetWithText(OutlinedButton, '推给桌面');
      expect(pushButton, findsOneWidget);
      expect(tester.widget<OutlinedButton>(pushButton).onPressed, isNotNull);
      await tester.tap(pushButton);
      await tester.pumpAndSettle();
      // 先弹出确认：该值会离开本机
      expect(find.text('推上去'), findsOneWidget);

      await tester.tap(find.text('推上去'));
      await tester.pumpAndSettle();
      expect(find.textContaining('已推给桌面'), findsWidgets);
    });

    testWidgets('本机取不到明文 → 不发请求，并说清为什么不发', (tester) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);
      // ⚠ 这里要的是"**按钮是亮的**、但取的时候没了"那种时序：
      // 页面用 `statusOf`（= hasCredential）决定按钮亮不亮，所以 hasCredential 得为 true，
      // 而 getCredential 回 null。两者都 false 的话按钮本来就点不动，测不到这条路径。
      store['PHONE_KEY'] = 'x';
      hideRevealed = true;

      var uploadCalls = 0;
      final client = MockClient((request) async {
        if (request.url.path == '/api/ai/credentials' &&
            request.method == 'PUT') {
          uploadCalls++;
          return http.Response('{"applied":["PHONE_KEY"]}', 200);
        }
        final body = switch (request.url.path) {
          '/api/ai/providers' => [
            {
              'id': 'p1',
              'displayName': '本机网关',
              'api': 'openai-completions',
              'baseURL': 'http://127.0.0.1:11434/v1',
              'credentialRef': 'PHONE_KEY',
              'endpointTrust': 'loopback',
              'enabled': true,
              'revision': 1,
              'credential': {
                'configured': true,
                'source': 'keystore',
                'writable': true,
              },
            },
          ],
          _ => <Object>[],
        };
        return http.Response(
          _json(body),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      });

      await tester.pumpWidget(await page(client));
      await tester.pumpAndSettle();
      await tester.tap(find.text('本机网关'));
      await tester.pumpAndSettle();
      final pushButton = find.widgetWithText(OutlinedButton, '推给桌面');
      expect(tester.widget<OutlinedButton>(pushButton).onPressed, isNotNull);
      await tester.tap(pushButton);
      await tester.pumpAndSettle();
      expect(find.text('推上去'), findsOneWidget);
      await tester.tap(find.text('推上去'));
      await tester.pumpAndSettle();

      expect(uploadCalls, 0, reason: '推空值会把桌面上那把好的覆盖掉 —— 所以这里必须一个请求都不发');
      expect(find.textContaining('本机没有这把密钥'), findsWidgets);
    });

    testWidgets('旧后端 404 → 直说"还没有这条接口"，不许假装成功', (tester) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);
      store['PHONE_KEY'] = 'sk-phone';

      await tester.pumpWidget(
        await page(backend(uploadStatus: 404, uploadBody: '')),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('本机网关'));
      await tester.pumpAndSettle();
      final pushButton = find.widgetWithText(OutlinedButton, '推给桌面');
      expect(tester.widget<OutlinedButton>(pushButton).onPressed, isNotNull);
      await tester.tap(pushButton);
      await tester.pumpAndSettle();
      expect(find.text('推上去'), findsOneWidget);
      await tester.tap(find.text('推上去'));
      await tester.pumpAndSettle();

      expect(find.textContaining('还没有"上行密钥"的接口'), findsWidgets);
    });

    // 真机 2026-11-08 实测（PJX110，约 361dp 宽）：三个按钮排一行放不下，
    // 「推给桌面」被裁掉 52px（界面右下角一条 RIGHT OVERFLOWED）。改成 Wrap 折行。
    testWidgets('360dp 窄屏：三个密钥按钮折行，一个都不许被裁掉', (tester) async {
      tester.view.physicalSize = const Size(360, 1600);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.reset);
      store['PHONE_KEY'] = 'sk-phone';

      await tester.pumpWidget(await page(backend()));
      await tester.pumpAndSettle();
      await tester.tap(find.text('本机网关'));
      await tester.pumpAndSettle();

      expect(
        tester.takeException(),
        isNull,
        reason: '360dp 下密钥区不许 RenderFlex overflow',
      );
      expect(find.widgetWithText(FilledButton, '保存密钥'), findsOneWidget);
      expect(find.widgetWithText(OutlinedButton, '移除密钥'), findsOneWidget);
      expect(find.widgetWithText(OutlinedButton, '推给桌面'), findsOneWidget);
    });
  });
}

String _json(Object body) => jsonEncode(body);
