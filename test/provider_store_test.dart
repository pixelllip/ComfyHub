// Provider 清单的**来源分叉**（M6 的另一半）：
//
//   用户拍板的第 4 条是"**手机完全可以独立配置 provider，有密钥的也同步到手机端**"。
//   密钥那一半早就做完了，可清单这一半以前不成立：`AiProviderSettingsPage` 的
//   provider / 模型 CRUD 清一色打 `AiApiClient`（桌面 HTTP）—— 桌面不在线时手机上
//   连"新建一个 provider / 改个 baseURL"都做不到，而 `android/harness` 的本机库里
//   `ai_providers` / `ai_models` **本来就是本机表**。
//
// 所以这里钉四件事：
//   ① `openProviderStore` 的平台分流（手机 = 桥、桌面 = 后端）；
//   ② **手机实现一条网络请求都不打**（桥只碰本机库），桥的报错翻成同一种人话异常；
//   ③ 页面注入本地 store 时，CRUD 真的走它（不是偷偷回落 HTTP）；
//   ④ 界面上**如实**说一句"这些 provider 只在这台手机上"（§14.4：两端各一份、不自动合并），
//      并把手机上还做不到的两项（连接测试 / 获取模型）禁用掉。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/core/provider_store.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

/// 假桥：按方法名回预设的 JSON，并把 `(方法, 参数)` 记下来。
class _FakeBridge {
  final List<MapEntry<String, Map<String, dynamic>?>> calls = [];

  /// 方法名 → 回执（JSON 字符串）。没配的方法回 null。
  final Map<String, String> replies = {};

  /// 方法名 → 要抛的桥错误。
  final Map<String, PlatformException> failures = {};

  MethodChannel channel() => MethodChannel('test/provider-store');

  Future<Object?> handle(MethodCall call) async {
    calls.add(
      MapEntry(call.method, (call.arguments as Map?)?.cast<String, dynamic>()),
    );
    final failure = failures[call.method];
    if (failure != null) throw failure;
    return replies[call.method];
  }

  Map<String, dynamic>? argsOf(String method) => calls
      .lastWhere((c) => c.key == method, orElse: () => const MapEntry('', null))
      .value;
}

Map<String, dynamic> _providerJson({
  String id = 'phone-gw',
  String name = '手机网关',
  String baseURL = 'http://192.168.1.9:11434/v1',
  int revision = 1,
}) => {
  'id': id,
  'displayName': name,
  'api': 'openai-completions',
  'baseURL': baseURL,
  'credentialRef': 'PHONE_KEY',
  'endpointTrust': 'private-network',
  'enabled': true,
  'revision': revision,
  'createdAt': '2026-09-27T00:00:00.000Z',
  'updatedAt': '2026-09-27T00:00:00.000Z',
};

Map<String, dynamic> _modelJson({
  String id = 'm-text',
  String name = '纯文本模型',
}) => {
  'providerId': 'phone-gw',
  'id': id,
  'displayName': name,
  'inputModalities': ['text', 'image'],
  'attachmentTransports': {
    'image': ['inline_base64'],
  },
  'mimeAllowlist': ['image/png'],
  'tools': true,
  'parallelTools': false,
  'reasoning': false,
  'thinkingEfforts': <String, String>{},
  'contextWindow': 128000,
  'capabilitySource': 'manual',
  'enabled': true,
};

/// 页面用的假 store：记录每一次调用，回执可定制。
class _FakeProviderStore implements ProviderStore {
  _FakeProviderStore({this.local = true});

  final bool local;
  final List<String> calls = [];
  List<AiProvider> providers = const [];
  List<AiModel> models = const [];
  Map<String, dynamic>? lastCreateDraft;
  AiProvider? lastUpdate;
  String? lastDeleted;
  String? lastModelsFor;
  List<AiModel>? lastSavedModels;
  Object? throwOn;

  @override
  bool get isLocal => local;

  @override
  bool get supportsProbe => !local;

  @override
  Future<List<AiProvider>> listProviders() async {
    calls.add('listProviders');
    if (throwOn == 'listProviders') throw AiApiException(400, '本机库坏了');
    return providers;
  }

  @override
  Future<AiProvider> createProvider(Map<String, dynamic> draft) async {
    calls.add('createProvider');
    lastCreateDraft = draft;
    final created = AiProvider.fromJson({
      ..._providerJson(
        id: draft['id'] as String,
        name: draft['displayName'] as String,
      ),
      'baseURL': draft['baseURL'],
    });
    // 真 store 的行为：建完就能在清单里查到（页面会 `_reload()` 再选中它）
    providers = [...providers, created];
    return created;
  }

  @override
  Future<AiProvider> updateProvider(AiProvider value) async {
    calls.add('updateProvider');
    lastUpdate = value;
    return value;
  }

  @override
  Future<void> deleteProvider(String id) async {
    calls.add('deleteProvider');
    lastDeleted = id;
  }

  @override
  Future<List<AiModel>> listModels(String providerId) async {
    calls.add('listModels');
    return models;
  }

  @override
  Future<List<AiModel>> saveModels(
    String providerId,
    List<AiModel> models,
  ) async {
    calls.add('saveModels');
    lastModelsFor = providerId;
    lastSavedModels = models;
    return models;
  }
}

/// 后端：**任何** provider 请求都记下来（用来证明"本地实现没有偷偷回落 HTTP"）。
class _Backend {
  final List<String> hits = [];

  AiApiClient client() => AiApiClient(
    'http://127.0.0.1:8080',
    client: MockClient((request) async {
      hits.add('${request.method} ${request.url.path}');
      return http.Response(
        jsonEncode(<String, dynamic>{}),
        200,
        headers: {'content-type': 'application/json; charset=utf-8'},
      );
    }),
  );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  // -------------------------------------------------------------------------
  //  ① 分流
  // -------------------------------------------------------------------------

  test('平台分流：手机（forceLocal）走桥，桌面走后端客户端', () {
    final api = _Backend().client();
    final local = openProviderStore(api: api, forceLocal: true);
    final remote = openProviderStore(api: api, forceLocal: false);

    expect(local, isA<DeviceProviderStore>());
    expect(local.isLocal, isTrue);
    expect(
      local.supportsProbe,
      isFalse,
      reason: '手机上"连接测试 / 获取模型"要由本机自己探上游 —— 还没实现，界面据此禁用（不看这一位就会去撞桌面那条不存在的 id）',
    );

    expect(remote, isA<RemoteProviderStore>());
    expect(remote.isLocal, isFalse);
    expect(remote.supportsProbe, isTrue);
  });

  // -------------------------------------------------------------------------
  //  ② 手机实现：只碰本机库（桥），一条网络请求都不打
  // -------------------------------------------------------------------------

  test('手机实现：调桥的方法名与参数形状都对，且零网络请求', () async {
    final bridge = _FakeBridge();
    final backend = _Backend();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(bridge.channel(), bridge.handle);
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(bridge.channel(), null),
    );

    bridge.replies['listProviders'] = jsonEncode([_providerJson()]);
    bridge.replies['saveProvider'] = jsonEncode(
      _providerJson(id: 'new-gw', name: '新建的'),
    );
    bridge.replies['listModels'] = jsonEncode([_modelJson()]);
    bridge.replies['saveModels'] = jsonEncode([
      _modelJson(id: 'm2', name: '改过的'),
    ]);

    // ⚠ 这个 api 是**故意给错的**：本地实现根本不该碰它（碰了就说明回落了 HTTP）
    final store = openProviderStore(
      api: backend.client(),
      forceLocal: true,
      methodChannel: bridge.channel(),
    ) as DeviceProviderStore;

    final listed = await store.listProviders();
    expect(listed.single.id, 'phone-gw');
    expect(
      listed.single.displayName,
      '手机网关',
      reason: 'DTO 键名逐字对齐才能直接 fromJson',
    );
    expect(bridge.argsOf('listProviders'), isNull, reason: '读清单不带参数');

    final created = await store.createProvider({
      'id': 'new-gw',
      'displayName': '新建的',
      'api': 'openai-completions',
      'baseURL': 'https://api.example.com/v1',
      'credentialRef': 'NEW_GW_API_KEY',
      'endpointTrust': 'public',
    });
    expect(created.id, 'new-gw');
    final createArgs = bridge.argsOf('saveProvider')!;
    final sentDraft =
        jsonDecode(createArgs['provider'] as String) as Map<String, dynamic>;
    expect(sentDraft['id'], 'new-gw');
    expect(sentDraft['baseURL'], 'https://api.example.com/v1');
    expect(
      sentDraft.containsKey('revision'),
      isFalse,
      reason: '新建不带 revision —— 桥按桌面 POST 的语义处理（revision 由库给 1）',
    );

    final stored = AiProvider.fromJson(_providerJson(revision: 3));
    await store.updateProvider(stored);
    final updateArgs = jsonDecode(
      bridge.argsOf('saveProvider')!['provider'] as String,
    ) as Map<String, dynamic>;
    expect(
      updateArgs['revision'],
      3,
      reason: '更新必须带 revision（桌面靠它做乐观锁 AIH-007）',
    );

    await store.deleteProvider('phone-gw');
    expect(bridge.argsOf('deleteProvider')!['id'], 'phone-gw');

    final models = await store.listModels('phone-gw');
    expect(models.single.id, 'm-text');
    expect(models.single.inputModalities, ['text', 'image']);
    expect(bridge.argsOf('listModels')!['providerId'], 'phone-gw');

    final saved = await store.saveModels('phone-gw', [
      const AiModel(
        providerId: 'phone-gw',
        id: 'm2',
        displayName: '改过的',
        tools: true,
      ),
    ]);
    expect(saved.single.id, 'm2');
    final saveArgs = bridge.argsOf('saveModels')!;
    expect(saveArgs['providerId'], 'phone-gw');
    final sentModels = jsonDecode(saveArgs['models'] as String) as List;
    expect((sentModels.single as Map)['id'], 'm2');
    expect((sentModels.single as Map)['providerId'], 'phone-gw');

    expect(backend.hits, isEmpty, reason: '手机实现只碰本机库：一个远端请求都不许打（桌面不在线也要能用）');
  });

  test('手机实现：桥报错要翻成同一种人话异常（页面不用分叉）', () async {
    final bridge = _FakeBridge()
      ..failures['saveProvider'] = PlatformException(
        code: 'CONFIG_ERROR',
        message: '配置已被其他窗口修改（当前 revision=1，提交的=6），请刷新后重试',
      );
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(bridge.channel(), bridge.handle);
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(bridge.channel(), null),
    );

    final store = openProviderStore(
      api: _Backend().client(),
      forceLocal: true,
      methodChannel: bridge.channel(),
    );

    await expectLater(
      store.createProvider({
        'id': 'x',
        'displayName': 'x',
        'api': 'openai-completions',
        'baseURL': 'http://a/v1',
      }),
      throwsA(
        isA<AiApiException>().having(
          (e) => '$e',
          'toString',
          '配置已被其他窗口修改（当前 revision=1，提交的=6），请刷新后重试',
        ),
      ),
    );
  });

  // -------------------------------------------------------------------------
  //  ③ 桌面实现：转发给后端（带 revision 的 PUT）
  // -------------------------------------------------------------------------

  test('桌面实现：六个动作都转发给后端，更新带 revision', () async {
    final seen = <String>[];
    late Map<String, dynamic> lastBody;

    /// 单独留一份 **provider 的 PUT body**：`lastBody` 最后会被模型目录那条 PUT 覆盖，
    /// 而这里要看的是 provider 那一份（`endpointTrust` 刻意不带）。
    Map<String, dynamic>? providerPut;
    final api = AiApiClient(
      'http://127.0.0.1:8080',
      client: MockClient((request) async {
        seen.add('${request.method} ${request.url.path}');
        if (request.body.isNotEmpty) {
          final decoded = jsonDecode(request.body) as Map<String, dynamic>;
          lastBody = decoded;
          if (request.method == 'PUT' &&
              request.url.path == '/api/ai/providers/phone-gw') {
            providerPut = decoded;
          }
        }
        final body = request.url.path.endsWith('/models')
            ? [_modelJson()]
            : (request.method == 'GET' &&
                      request.url.path == '/api/ai/providers'
                  ? [_providerJson()]
                  : _providerJson());
        return http.Response(
          jsonEncode(body),
          200,
          headers: {'content-type': 'application/json; charset=utf-8'},
        );
      }),
    );
    final store = openProviderStore(api: api, forceLocal: false);

    await store.listProviders();
    await store.createProvider({
      'id': 'p',
      'displayName': 'P',
      'api': 'openai-completions',
      'baseURL': 'http://a/v1',
    });
    await store.updateProvider(AiProvider.fromJson(_providerJson(revision: 4)));
    await store.deleteProvider('p');
    await store.listModels('p');
    await store.saveModels('p', [
      const AiModel(providerId: 'p', id: 'm', displayName: 'M'),
    ]);

    expect(seen, [
      'GET /api/ai/providers',
      'POST /api/ai/providers',
      'PUT /api/ai/providers/phone-gw',
      'DELETE /api/ai/providers/p',
      'GET /api/ai/providers/p/models',
      'PUT /api/ai/providers/p/models',
    ]);
    expect(
      lastBody['models'],
      isA<List>(),
      reason: '模型目录是整份替换（PUT body 里是 models 数组）',
    );

    // provider 那一份 PUT：**一个字节的 endpointTrust 都不带**
    // （2026-10 用户要求：信任档由软件按地址决定）。带了旧档的话，"把公网 Provider 的地址
    // 改成本机"会拿着 public 去校验，当场被后端拒 —— 而表单里已经没有这一栏可改了。
    expect(providerPut, isNotNull, reason: '更新必须真的 PUT 出去');
    expect(
      providerPut!.containsKey('endpointTrust'),
      isFalse,
      reason: '不带信任档 → 后端按这次的新 baseURL 重新推断（`routes::update_provider`）',
    );
    expect(providerPut!['revision'], 4, reason: '乐观锁那一份照旧要带');
  });

  // -------------------------------------------------------------------------
  //  ④ 页面：注入本地 store 时 CRUD 真的走它
  // -------------------------------------------------------------------------

  Future<Widget> page(_FakeProviderStore store, _Backend backend) async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    return ChangeNotifierProvider<SettingsStore>.value(
      value: settings,
      child: MaterialApp(
        home: AiProviderSettingsPage(
          api: backend.client(),
          providers: store,
          credentials: _NoCredentialStore(),
        ),
      ),
    );
  }

  testWidgets('页面注入本地 store：清单与模型都从它来，后端一个 provider 请求都没收到', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeProviderStore()
      ..providers = [AiProvider.fromJson(_providerJson())]
      ..models = [AiModel.fromJson(_modelJson())];
    final backend = _Backend();

    await tester.pumpWidget(await page(store, backend));
    await tester.pumpAndSettle();

    expect(store.calls, contains('listProviders'));
    await tester.tap(find.text('手机网关'));
    await tester.pumpAndSettle();
    expect(store.calls, contains('listModels'));
    expect(find.text('纯文本模型'), findsWidgets);
    expect(
      backend.hits.where((h) => h.contains('/api/ai/providers')),
      isEmpty,
      reason: '清单走本机 store 之后，页面不该再去问后端要清单',
    );
  });

  testWidgets('页面：手机上如实说"只在这台手机上"，并把探上游那两项禁用', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeProviderStore()
      ..providers = [AiProvider.fromJson(_providerJson())];
    await tester.pumpWidget(await page(store, _Backend()));
    await tester.pumpAndSettle();

    // §14.4：两端各一份清单、不自动合并 —— 不说这句用户会以为桌面也能用
    expect(find.textContaining('只在这台手机上'), findsOneWidget);
    expect(find.textContaining('不会自动合并'), findsOneWidget);

    await tester.tap(find.text('手机网关'));
    await tester.pumpAndSettle();

    final test = tester.widget<OutlinedButton>(
      find.ancestor(
        of: find.text('测试连接'),
        matching: find.byType(OutlinedButton),
      ),
    );
    expect(test.onPressed, isNull, reason: '手机上还做不到：要由本机自己探上游');

    final discover = tester.widget<TextButton>(
      find.ancestor(of: find.text('获取可用模型'), matching: find.byType(TextButton)),
    );
    expect(discover.onPressed, isNull);
    expect(find.textContaining('目前只在桌面可用'), findsOneWidget);
  });

  testWidgets('页面：桌面 store 上那两个按钮照旧可用（本地才禁用）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeProviderStore(local: false)
      ..providers = [AiProvider.fromJson(_providerJson())];
    await tester.pumpWidget(await page(store, _Backend()));
    await tester.pumpAndSettle();

    expect(find.textContaining('只在这台手机上'), findsNothing);
    await tester.tap(find.text('手机网关'));
    await tester.pumpAndSettle();
    final test = tester.widget<OutlinedButton>(
      find.ancestor(
        of: find.text('测试连接'),
        matching: find.byType(OutlinedButton),
      ),
    );
    expect(test.onPressed, isNotNull);
  });

  testWidgets('页面：新建与删除都走注入的 store（不是 HTTP）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeProviderStore();
    final backend = _Backend();
    await tester.pumpWidget(await page(store, backend));
    await tester.pumpAndSettle();

    // 新建：对话框 →「创建」
    await tester.tap(find.text('新建 Provider'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.ancestor(
        of: find.text('Provider ID（创建后不可改）'),
        matching: find.byType(TextField),
      ),
      'phone-new',
    );
    await tester.enterText(
      find.ancestor(of: find.text('显示名'), matching: find.byType(TextField)),
      '手机新建的',
    );
    await tester.enterText(
      find.ancestor(
        of: find.text('Base URL'),
        matching: find.byType(TextField),
      ),
      'https://api.example.com/v1',
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('创建'));
    await tester.pumpAndSettle();

    expect(store.calls, contains('createProvider'));
    expect(store.lastCreateDraft!['id'], 'phone-new');
    expect(store.lastCreateDraft!['baseURL'], 'https://api.example.com/v1');

    // 删除：列表行的垃圾桶 → 确认
    await tester.tap(find.byIcon(Icons.delete_outline).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(store.calls, contains('deleteProvider'));
    expect(
      backend.hits.where((h) => h.contains('/api/ai/providers')),
      isEmpty,
      reason: '手机上建/删一条 provider 都不出网 —— 桌面不在线也能做',
    );
  });

  testWidgets('页面：编辑一条已有的走 store，且带 revision（乐观锁）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeProviderStore()
      ..providers = [AiProvider.fromJson(_providerJson(revision: 2))];
    final backend = _Backend();
    await tester.pumpWidget(await page(store, backend));
    await tester.pumpAndSettle();
    await tester.tap(find.text('手机网关'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('编辑 Provider'));
    await tester.pumpAndSettle();

    // 对话框要**预填**现有那一行（用户一眼看得出在改什么）
    final urlField = find.ancestor(
      of: find.text('Base URL'),
      matching: find.byType(TextField),
    );
    expect(
      tester.widget<TextField>(urlField).controller!.text,
      'http://192.168.1.9:11434/v1',
    );
    // ID 锁住：两端都是"创建后不可修改"
    final idField = find.ancestor(
      of: find.text('Provider ID（创建后不可改）'),
      matching: find.byType(TextField),
    );
    expect(tester.widget<TextField>(idField).readOnly, isTrue);

    // 改 baseURL（用户原话里那一件事）→ 保存
    await tester.enterText(urlField, 'http://192.168.1.20:11434/v1');
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();

    expect(store.calls, contains('updateProvider'));
    expect(store.lastUpdate!.id, 'phone-gw');
    expect(store.lastUpdate!.baseURL, 'http://192.168.1.20:11434/v1');
    expect(
      store.lastUpdate!.revision,
      2,
      reason: '更新必须带库里那一版 revision（AIH-007）',
    );
    expect(
      backend.hits.where((h) => h.contains('/api/ai/providers')),
      isEmpty,
      reason: '手机上改地址也不出网 —— 桌面不在线时这正是用户要的那件事',
    );
  });
}

/// 只要"不是本机密钥库"这一位：这些用例盯的是**清单**走哪条路，
/// 凭据状态覆盖那一半与它无关（`isLocal = false` 就是"别覆盖"）。
class _NoCredentialStore implements CredentialStore {
  @override
  bool get isLocal => false;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async =>
      AiCredentialStatus.none;

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) async =>
      AiCredentialStatus.none;

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) async =>
      AiCredentialStatus.none;

  @override
  Future<String?> reveal(AiProvider provider) async => null;
}
