// 「手机从电脑同步 Provider」的防线（`docs/pairing-design.md` §14.6）。
//
// 这段功能的价值全在**语义**上，而语义错了不会报错：多删一条本机独有的 Provider、
// 用一份空目录覆盖掉模型能力声明、拿桌面的 revision 去写本机库撞乐观锁……
// 表现都只是"同步完看起来有点不对"。所以这里一条一条钉住：
//   ① 建 / 覆盖 / 保留三态，且**一条都不删**；
//   ② 同 id 且内容一致 → **一个字节都不写**；
//   ③ 模型目录拉取失败 → **绝不用空目录覆盖**；
//   ④ 不认识的协议 / 缺字段 → 跳过并如实说，其余照常；
//   ⑤ 整趟拿不到清单 → 本机零改动；
//   ⑥ 界面上：手机有这个按钮、桌面上没有；一次点击真的走建 / 写模型并刷新清单。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/credential_store.dart';
import 'package:viewer/core/provider_store.dart';
import 'package:viewer/core/provider_sync.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

Map<String, dynamic> _providerJson({
  String id = 'desk-gw',
  String name = '桌面网关',
  String api = 'openai-completions',
  String baseURL = 'https://api.example.com/v1',
  String? credentialRef = 'DESK_API_KEY',
  String endpointTrust = 'public',
  bool enabled = true,
  int revision = 7,
}) =>
    {
      'id': id,
      'displayName': name,
      'api': api,
      'baseURL': baseURL,
      'credentialRef': ?credentialRef,
      'endpointTrust': endpointTrust,
      'enabled': enabled,
      'revision': revision,
      'createdAt': '2026-09-27T00:00:00.000Z',
      'updatedAt': '2026-09-27T00:00:00.000Z',
    };

Map<String, dynamic> _modelJson({
  String providerId = 'desk-gw',
  String id = 'm-text',
  String name = '纯文本模型',
}) =>
    {
      'providerId': providerId,
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

/// 本机那份清单元件（= 手机的 `DeviceProviderStore`，走桥进本机库）。
class _FakeLocalStore implements ProviderStore {
  _FakeLocalStore({
    this.local = true,
    List<AiProvider>? providers,
    Map<String, List<AiModel>>? models,
  })  : providers = providers ?? [],
        models = models ?? {};

  final bool local;
  List<AiProvider> providers;
  Map<String, List<AiModel>> models;

  final List<String> calls = [];
  final List<String> deleted = [];

  /// 每次 `updateProvider` 提交上来的 revision（乐观锁那一份必须来自**本机**）。
  final List<int> submittedRevisions = [];
  Map<String, dynamic>? lastCreateDraft;
  Object? throwOnListModels;
  Object? throwOnCreate;

  @override
  bool get isLocal => local;

  @override
  bool get supportsProbe => !local;

  @override
  Future<List<AiProvider>> listProviders() async {
    calls.add('listProviders');
    return providers;
  }

  @override
  Future<AiProvider> createProvider(Map<String, dynamic> draft) async {
    calls.add('createProvider');
    if (throwOnCreate != null) throw throwOnCreate!;
    lastCreateDraft = draft;
    final created = AiProvider.fromJson({
      ...draft,
      'revision': 1,
      'createdAt': '2026-09-27T00:00:00.000Z',
      'updatedAt': '2026-09-27T00:00:00.000Z',
    });
    providers = [...providers, created];
    return created;
  }

  @override
  Future<AiProvider> updateProvider(AiProvider value) async {
    calls.add('updateProvider');
    submittedRevisions.add(value.revision);
    final updated = AiProvider(
      id: value.id,
      displayName: value.displayName,
      api: value.api,
      baseURL: value.baseURL,
      credentialRef: value.credentialRef,
      endpointTrust: value.endpointTrust,
      enabled: value.enabled,
      revision: value.revision + 1,
      credential: value.credential,
    );
    providers = [for (final p in providers) p.id == updated.id ? updated : p];
    return updated;
  }

  @override
  Future<void> deleteProvider(String id) async {
    calls.add('deleteProvider');
    deleted.add(id);
    providers = [for (final p in providers) if (p.id != id) p];
  }

  @override
  Future<List<AiModel>> listModels(String providerId) async {
    calls.add('listModels');
    if (throwOnListModels != null) throw throwOnListModels!;
    return models[providerId] ?? const [];
  }

  @override
  Future<List<AiModel>> saveModels(String providerId, List<AiModel> value) async {
    calls.add('saveModels');
    models = {...models, providerId: value};
    return value;
  }
}

/// 桌面后端：只回**两条只读**端点（`GET /api/ai/providers` 与 `…/{id}/models`）。
///
/// 别的请求（尤其 PUT / POST / DELETE）一律 500：这条功能是"只读搬运"，
/// 谁要是往桌面上写，得在这里当场露馅。
class _FakeDesktop {
  _FakeDesktop({
    required this.providers,
    this.models = const {},
    this.failModels = const {},
    this.providersStatus = 200,
  });

  /// `GET /api/ai/providers` 回的那份清单（响应体里带桌面的 revision，本机不该用它）。
  final List<Map<String, dynamic>> providers;

  /// providerId → 模型数组。
  final Map<String, List<Map<String, dynamic>>> models;

  /// 这些 provider 的模型端点回 500。
  final Set<String> failModels;

  /// 非 200 = 整趟失败（403：没配对令牌 / 闸门挡着）。
  final int providersStatus;

  final List<String> hits = [];

  AiApiClient client() => AiApiClient(
        'http://desktop.test',
        client: MockClient((request) async {
          final path = request.url.path;
          hits.add('${request.method} $path');
          const json = {'content-type': 'application/json; charset=utf-8'};
          if (request.method != 'GET') {
            return http.Response('{"error":"同步只读，不许往桌面写"}', 500, headers: json);
          }
          if (path == '/api/ai/providers') {
            if (providersStatus != 200) {
              return http.Response('{"error":"需要配对令牌（X-ComfyHub-Token）"}', providersStatus,
                  headers: json);
            }
            return http.Response(jsonEncode(providers), 200, headers: json);
          }
          final match = RegExp(r'^/api/ai/providers/(.+)/models$').firstMatch(path);
          if (match != null) {
            final id = Uri.decodeComponent(match.group(1)!);
            if (failModels.contains(id)) {
              return http.Response('{"error":"上游超时"}', 500, headers: json);
            }
            return http.Response(jsonEncode(models[id] ?? const []), 200, headers: json);
          }
          return http.Response('{"error":"没有这条端点"}', 404, headers: json);
        }),
      );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  // ---------------------------------------------------------------------------
  //  ① 建 / 覆盖 / 保留
  // ---------------------------------------------------------------------------

  test('新建、覆盖、保留三态都如实：同 id 用桌面覆盖，本机独有的一条不删', () async {
    final desktop = _FakeDesktop(
      providers: [
        _providerJson(id: 'new-gw', name: '桌面新网关', revision: 9),
        _providerJson(
          id: 'both-gw',
          name: '桌面上的名字',
          baseURL: 'https://desk.example.com/v1',
          credentialRef: 'DESK_KEY',
          revision: 9,
        ),
      ],
      models: {
        'new-gw': [_modelJson(providerId: 'new-gw', id: 'm1', name: '模型一')],
        'both-gw': [
          _modelJson(providerId: 'both-gw', id: 'm2', name: '模型二'),
          _modelJson(providerId: 'both-gw', id: 'm3', name: '模型三'),
        ],
      },
    );
    final local = _FakeLocalStore(
      providers: [
        AiProvider.fromJson(_providerJson(
          id: 'both-gw',
          name: '手机上的名字',
          baseURL: 'http://192.168.1.9:11434/v1',
          credentialRef: null,
          revision: 3,
        )),
        AiProvider.fromJson(_providerJson(id: 'phone-only', name: '只在手机上', revision: 2)),
      ],
    );

    final report = await ProviderSync(desktop: desktop.client(), local: local).run();

    expect(report.ok, isTrue, reason: report.error);
    expect(report.created, ['new-gw']);
    expect(report.updated, ['both-gw']);
    expect(report.unchanged, isEmpty);
    expect(
      report.localOnly,
      ['phone-only'],
      reason: '桌面没有它 ≠ 本机该删它 —— 本机清单可能是用户自己配的',
    );
    expect(report.modelsWritten, 3, reason: '两份目录各写一次（1 + 2 条）');
    expect(report.problems, isEmpty);

    // 一条都不删：这是最容易"顺手"写出来、也最难被发现的一种破坏
    expect(local.deleted, isEmpty);
    expect(local.calls, isNot(contains('deleteProvider')));

    // 覆盖：桌面的字段进来了，credentialRef 也跟着来（之后取密钥才知道拿哪把）
    final both = local.providers.firstWhere((p) => p.id == 'both-gw');
    expect(both.displayName, '桌面上的名字');
    expect(both.baseURL, 'https://desk.example.com/v1');
    expect(both.credentialRef, 'DESK_KEY');

    // ★ 提交的是**本机那一版 revision**（3），不是桌面的 9：两端的库是两份
    expect(local.submittedRevisions, [3]);

    // 模型目录按 providerId + id 落进本机那份
    expect(local.models['new-gw']!.single.id, 'm1');
    expect(local.models['new-gw']!.single.providerId, 'new-gw');
    expect(local.models['both-gw']!.map((m) => m.id), ['m2', 'm3']);

    // 只读：绝不许往桌面上写
    expect(
      desktop.hits.every((h) => h.startsWith('GET ')),
      isTrue,
      reason: '这条通道是只读搬运（桌面那份是真源）：${desktop.hits}',
    );
  });

  // ---------------------------------------------------------------------------
  //  ② 内容一致 → 一个字节都不写
  // ---------------------------------------------------------------------------

  test('同 id 且内容与模型目录都一致：零写库（不白涨 revision、不白抖一次）', () async {
    final provider = _providerJson(id: 'same-gw', revision: 5);
    final desktop = _FakeDesktop(
      providers: [provider],
      models: {
        'same-gw': [_modelJson(providerId: 'same-gw')],
      },
    );
    final local = _FakeLocalStore(
      // 本机那一行的 revision 与桌面不同（两份库各算各的），但**配置内容**一致
      providers: [AiProvider.fromJson(_providerJson(id: 'same-gw', revision: 2))],
      models: {
        'same-gw': [AiModel.fromJson(_modelJson(providerId: 'same-gw'))],
      },
    );

    final report = await ProviderSync(desktop: desktop.client(), local: local).run();

    expect(report.ok, isTrue, reason: report.error);
    expect(report.unchanged, ['same-gw']);
    expect(report.touched, isFalse);
    expect(report.modelsWritten, 0);
    expect(local.calls.where((c) => c != 'listProviders' && c != 'listModels'), isEmpty,
        reason: '没有变化就不许写库：${local.calls}');
  });

  // ---------------------------------------------------------------------------
  //  ③ 模型目录拉取失败 → 本机那份绝不能被清空
  // ---------------------------------------------------------------------------

  test('模型目录拉取失败：Provider 照样同步，本机那份目录一个字节都不动', () async {
    final desktop = _FakeDesktop(
      providers: [_providerJson(id: 'flaky-gw')],
      failModels: {'flaky-gw'},
    );
    final local = _FakeLocalStore(
      providers: [AiProvider.fromJson(_providerJson(id: 'flaky-gw', revision: 1))],
      models: {
        'flaky-gw': [AiModel.fromJson(_modelJson(providerId: 'flaky-gw', id: 'keep-me'))],
      },
    );

    final report = await ProviderSync(desktop: desktop.client(), local: local).run();

    expect(report.ok, isTrue, reason: '单条失败不该把整趟判死');
    expect(report.problems, hasLength(1));
    expect('${report.problems.single}', contains('模型目录'));
    expect(local.calls, isNot(contains('saveModels')),
        reason: '拿空目录覆盖会把"上游抖了一下"变成"模型全没了"');
    expect(local.models['flaky-gw']!.single.id, 'keep-me');
  });

  // ---------------------------------------------------------------------------
  //  ④ 不认识的协议 / 缺字段：跳过并如实说，其余照常
  // ---------------------------------------------------------------------------

  test('协议不认识或缺字段的一条被跳过（不建一条本机发不出去的），其余照常同步', () async {
    final desktop = _FakeDesktop(
      providers: [
        _providerJson(id: 'acme-gw', api: 'acme-rpc'),
        _providerJson(id: 'no-url', baseURL: ''),
        _providerJson(id: 'good-gw'),
      ],
      models: {
        'good-gw': [_modelJson(providerId: 'good-gw')],
      },
    );
    final local = _FakeLocalStore();

    final report = await ProviderSync(desktop: desktop.client(), local: local).run();

    expect(report.ok, isTrue, reason: report.error);
    expect(report.created, ['good-gw']);
    expect(report.problems, hasLength(2));
    expect(report.problems.map((p) => p.providerId), containsAll(['acme-gw', 'no-url']));
    expect(local.providers.map((p) => p.id), ['good-gw']);
  });

  // ---------------------------------------------------------------------------
  //  ⑤ 整趟失败 → 本机零改动
  // ---------------------------------------------------------------------------

  test('拿不到桌面清单（403）：整趟失败、一个字都没动', () async {
    final desktop = _FakeDesktop(providers: const [], providersStatus: 403);
    final local = _FakeLocalStore(
      providers: [AiProvider.fromJson(_providerJson(id: 'phone-only', revision: 1))],
      models: {
        'phone-only': [AiModel.fromJson(_modelJson(providerId: 'phone-only'))],
      },
    );

    final report = await ProviderSync(desktop: desktop.client(), local: local).run();

    expect(report.ok, isFalse);
    expect(report.error, contains('403'));
    expect(report.touched, isFalse);
    expect(local.providers.map((p) => p.id), ['phone-only'], reason: '失败不许顺手清理本机');
    expect(local.models['phone-only'], hasLength(1));
  });

  test('桌面那一端（不是本机库）直接拒绝：没有"自己同步给自己"这回事', () async {
    final local = _FakeLocalStore(local: false);
    final report = await ProviderSync(desktop: _FakeDesktop(providers: const []).client(), local: local).run();

    expect(report.ok, isFalse);
    expect(report.error, contains('不需要从桌面同步'));
    expect(local.calls, isEmpty);
  });

  // ---------------------------------------------------------------------------
  //  ⑥ 界面
  // ---------------------------------------------------------------------------

  Future<Widget> page({
    required _FakeLocalStore store,
    required _FakeDesktop desktop,
  }) async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    return ChangeNotifierProvider<SettingsStore>.value(
      value: settings,
      child: MaterialApp(
        home: AiProviderSettingsPage(
          api: desktop.client(),
          providers: store,
          credentials: _NoCredentialStore(),
        ),
      ),
    );
  }

  testWidgets('手机上点「从电脑同步 Provider」：确认 → 建进本机 → 清单与结果都如实', (tester) async {
    tester.view.physicalSize = const Size(420, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeLocalStore();
    final desktop = _FakeDesktop(
      providers: [_providerJson(id: 'desk-gw', name: '桌面网关')],
      models: {
        'desk-gw': [_modelJson(providerId: 'desk-gw', id: 'm1', name: '桌面上的模型')],
      },
    );

    await tester.pumpWidget(await page(store: store, desktop: desktop));
    await tester.pumpAndSettle();

    expect(find.text('从电脑同步 Provider'), findsOneWidget);
    await tester.tap(find.text('从电脑同步 Provider'));
    await tester.pumpAndSettle();

    // 覆盖本机配置之前必须先问一次
    expect(find.text('从电脑同步 Provider？'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.textContaining('API Key 不在其中'),
      ),
      findsOneWidget,
    );

    await tester.tap(find.text('开始同步'));
    await tester.pumpAndSettle();

    expect(store.calls, contains('createProvider'));
    expect(store.calls, contains('saveModels'));
    expect(store.models['desk-gw']!.single.id, 'm1');

    // 结果如实报账，关掉之后清单里就有这条了
    expect(find.text('同步完成'), findsOneWidget);
    expect(find.textContaining('新建 1 个'), findsOneWidget);
    await tester.tap(find.text('好的'));
    await tester.pumpAndSettle();
    expect(find.text('桌面网关'), findsOneWidget);
  });

  testWidgets('桌面端不出现这个按钮（后端库自己就是真源）', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeLocalStore(
      local: false,
      providers: [AiProvider.fromJson(_providerJson(id: 'desk-gw'))],
    );
    await tester.pumpWidget(await page(store: store, desktop: _FakeDesktop(providers: const [])));
    await tester.pumpAndSettle();

    expect(find.text('从电脑同步 Provider'), findsNothing);
    expect(
      store.calls.every((c) => c == 'listProviders' || c == 'listModels'),
      isTrue,
      reason: '桌面端只是把清单读出来显示，不该有建 / 更 / 删 / 写目录：${store.calls}',
    );
  });

  testWidgets('320dp 的窄屏上：两个按钮与那三句说明都不许画到屏外', (tester) async {
    // 真机最窄那一档（`phone_layout_overflow_test.dart` 也钉着 320）：按钮 + 说明是
    // 这一页新加的固定高度内容，溢出在这里表现为黄黑斜纹（= 未捕获异常）。
    tester.view.physicalSize = const Size(320, 640);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final store = _FakeLocalStore(
      providers: [AiProvider.fromJson(_providerJson(id: 'desk-gw'))],
    );
    await tester.pumpWidget(await page(store: store, desktop: _FakeDesktop(providers: const [])));
    await tester.pumpAndSettle();

    expect(find.text('从电脑同步 Provider'), findsOneWidget);
    expect(find.textContaining('只在这台手机上'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}

/// 这些用例盯的是**清单怎么搬**，与凭据状态覆盖无关（`isLocal = false` = 不覆盖）。
class _NoCredentialStore implements CredentialStore {
  @override
  bool get isLocal => false;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async => AiCredentialStatus.none;

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) async => AiCredentialStatus.none;

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) async => AiCredentialStatus.none;

  @override
  Future<String?> reveal(AiProvider provider) async => null;
}
