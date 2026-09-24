// 回归（M6「手机侧的凭据」）：
//
// 手机上的 Run 由**本地 Kotlin harness** 执行、会话落在本地库，所以"这把 key 在不在手上"
// 也必须问**本机**（Android Keystore）。以前界面清一色打
// `PUT /api/ai/providers/{id}/credentials`，于是：
//   ① 手机上**没有任何入口**把 key 放进 Keystore —— harness 再完整也永远发不出消息；
//   ② 界面还显示"密钥已保存"，保存的其实是**桌面**那一份；
//   ③ provider 清单里的 `credential` 来自桌面，手机上看是"已配置"，点发送却 401。
//
// 三条都不报错，所以这里用假通道把**通道协议**与**分流判据**钉住。

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/models/ai_models.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const methodChannel = MethodChannel(LocalHarnessRunEventSourceNames.methodChannel);
  final messenger = TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final calls = <MethodCall>[];

  /// 本机密钥库里"已经"有这些 ref（假 Keystore）。
  var stored = <String, String>{};

  setUp(() {
    calls.clear();
    stored = {};
    messenger.setMockMethodCallHandler(methodChannel, (call) async {
      calls.add(call);
      final args = Map<String, dynamic>.from((call.arguments as Map?) ?? const {});
      switch (call.method) {
        case 'hasCredential':
          return stored.containsKey(args['ref']);
        case 'putCredential':
          stored[args['ref'] as String] = args['value'] as String;
          return true;
        case 'removeCredential':
          stored.remove(args['ref']);
          return true;
      }
      return null;
    });
  });

  tearDown(() => messenger.setMockMethodCallHandler(methodChannel, null));

  DeviceCredentialStore device() => DeviceCredentialStore(channel: methodChannel);

  AiProvider provider({String? ref = 'PHONE_KEY'}) => AiProvider(
        id: 'p1',
        displayName: '本机网关',
        api: 'openai-completions',
        baseURL: 'http://127.0.0.1:11434/v1',
        credentialRef: ref,
        endpointTrust: 'loopback',
        enabled: true,
        revision: 1,
        credential: const AiCredentialStatus(configured: true, source: 'managed', writable: true),
      );

  test('平台默认值：Android 用本机密钥库，其余平台用后端托管', () {
    final api = AiApiClient('http://127.0.0.1:8080');
    // `forceLocal` 就是 Android 那条分支的开关（测试进程里 Platform.isAndroid 恒为 false）
    expect(openCredentialStore(api: api, forceLocal: true), isA<DeviceCredentialStore>());
    expect(openCredentialStore(api: api, forceLocal: false), isA<RemoteCredentialStore>());
    expect(openCredentialStore(api: api, forceLocal: true).isLocal, isTrue);
  });

  test('本机状态：没有 ref 就**不问通道**，有 ref 也不混进桌面的 managed', () async {
    final store = device();

    // ref 为空 = 这条 provider 本来就不需要密钥（与桌面 describe 的第一个分支同义）
    final blank = await store.statusOf(provider(ref: null));
    expect(blank.configured, isFalse);
    expect(blank.source, 'none');
    expect(blank.writable, isFalse);
    expect(calls, isEmpty, reason: '没 ref 就不该去问通道');

    // 本机没有那把 key —— 哪怕桌面说"已配置"，这里也必须如实说没有
    final missing = await store.statusOf(provider());
    expect(missing.configured, isFalse);
    expect(missing.source, 'none', reason: '绝不能借桌面的 managed 冒充');
    expect(missing.writable, isTrue, reason: '本机永远可写（要能填、也要能清）');
    expect(missing.label, '未配置');

    stored['PHONE_KEY'] = 'sk-x';
    final ok = await store.statusOf(provider());
    expect(ok.configured, isTrue);
    expect(ok.source, 'keystore');
    expect(ok.label, '已配置（本机密钥库）');
  });

  test('保存 / 移除都只碰本机密钥库，且**只写不读**', () async {
    final store = device();

    final saved = await store.save(provider(), 'sk-secret');
    // save 的最后一步会回读一次状态（`statusOf` → hasCredential），所以这里看**第一条**
    expect(calls.first.method, 'putCredential');
    expect((calls.first.arguments as Map)['ref'], 'PHONE_KEY');
    expect((calls.first.arguments as Map)['value'], 'sk-secret');
    expect(saved.configured, isTrue);
    expect(saved.source, 'keystore');

    final cleared = await store.clear(provider());
    // clear 的最后一步同样会回读状态，所以这里找"有没有发生过移除"，而不是看最后一条
    expect(calls.map((c) => c.method), contains('removeCredential'));
    expect(cleared.configured, isFalse);

    // 整个协议里**没有**"把值取出来"的动作（AIH-012：值不回读）
    final methods = calls.map((c) => c.method).toSet();
    expect(methods, {'putCredential', 'removeCredential', 'hasCredential'});
  });

  test('没有 ref / 空值时**说清楚**，不许静默写进一个没有名字的槽位', () async {
    final store = device();
    await expectLater(store.save(provider(ref: '   '), 'sk-x'), throwsA(isA<StateError>()));
    await expectLater(
      store.clear(provider(ref: null)),
      completes,
      reason: '没 ref 的"清除"是幂等的空操作，不是错误',
    );
    expect(calls.where((c) => c.method == 'putCredential'), isEmpty);
  });

  test('远端实现：状态直接取清单里那份，写/删走 HTTP', () async {
    final seen = <String>[];
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        seen.add('${request.method} ${request.url.path}');
        return http.Response(
          '{"configured":true,"source":"managed","writable":true}',
          200,
        );
      }),
    );
    final store = RemoteCredentialStore(api);

    expect(store.isLocal, isFalse);
    final status = await store.statusOf(provider());
    expect(status.source, 'managed', reason: '桌面的状态就来自清单，不必再多问一次');
    expect(seen, isEmpty, reason: 'statusOf 不该产生任何请求');

    await store.save(provider(), 'sk-x');
    await store.clear(provider());
    expect(seen, [
      'PUT /api/ai/providers/p1/credentials',
      'DELETE /api/ai/providers/p1/credentials',
    ]);
  });

  test('模型目录的本地缓存必须原样往返（丢了能力声明 = 附件/工具静默失效）', () {
    const model = AiModel(
      providerId: 'p1',
      id: 'm-all',
      displayName: '全能模型',
      inputModalities: ['text', 'image'],
      attachmentTransports: {
        'image': ['inline_base64']
      },
      mimeAllowlist: ['image/png'],
      tools: true,
      parallelTools: true,
      reasoning: true,
      thinkingEfforts: {'low': 'low', 'max': '4096'},
      thinkingFormat: 'openai',
      contextWindow: 200000,
      maxOutputTokens: 8192,
      maxAttachmentCount: 4,
      capabilitySource: 'builtin',
      enabled: false,
    );

    final back = AiModel.fromJson(model.toJson());
    expect(back.providerId, model.providerId);
    expect(back.id, model.id);
    expect(back.displayName, model.displayName);
    expect(back.inputModalities, model.inputModalities);
    expect(back.attachmentTransports, model.attachmentTransports);
    expect(back.mimeAllowlist, model.mimeAllowlist);
    expect(back.tools, isTrue);
    expect(back.parallelTools, isTrue);
    expect(back.reasoning, isTrue);
    expect(back.thinkingEfforts, model.thinkingEfforts);
    expect(back.thinkingFormat, 'openai');
    expect(back.contextWindow, 200000);
    expect(back.maxOutputTokens, 8192);
    expect(back.maxAttachmentCount, 4);
    expect(back.capabilitySource, 'builtin');
    expect(back.enabled, isFalse);
  });

  test('provider 缓存的往返：凭据状态不进缓存（它是"这一刻"的事实）', () {
    final p = provider();
    final back = AiProvider.fromJson(p.toJson());
    expect(back.id, 'p1');
    expect(back.baseURL, 'http://127.0.0.1:11434/v1');
    expect(back.credentialRef, 'PHONE_KEY');
    expect(back.api, 'openai-completions');
    // 缓存里没有 credential → 读回来是 none，随后由本机密钥库覆盖
    expect(p.toJson().containsKey('credential'), isFalse);
    expect(back.credential.configured, isFalse);
  });
}

/// 只为了在测试里写死通道名（避免 import 整个 run_event_source 只为拿一个常量）。
class LocalHarnessRunEventSourceNames {
  static const methodChannel = 'comfyhub/harness';
}
