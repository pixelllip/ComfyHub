// AI 工作台的模型选择器：按 Provider 分组（用户 2026-10 的两条要求）。
//
//   1. 「来自不同 provider 的模型要套在一个 padding 里面区分，**padding 外标注模型来源**」
//      —— 布局是**硬要求**，不是配色偏好：一眼要能分清哪个模型是谁家的。
//   2. 顺带修掉「新加入的 provider 无法在工作台中选用（不加载）」：
//      以前这个对话框只有**当前那一条**的模型，界面上又没有别的地方能换 Provider，
//      于是刚建好的那一条永远选不上。
//
// 真后端那份清单是按 display_name 排序的，这里照抄 ——
// 断言"哪一组在上面"才有意义（否则用例会随字典序抖动）。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_home_page.dart';
import 'package:viewer/state/ai_workspace_store.dart';

Map<String, dynamic> _provider(String id, String name, {bool credential = true}) => {
      'id': id,
      'displayName': name,
      'api': 'openai-completions',
      'baseURL': 'https://api.$id.example/v1',
      'credentialRef': '${id.toUpperCase()}_KEY',
      'endpointTrust': 'public',
      'enabled': true,
      'revision': 1,
      'credential': {
        'configured': credential,
        'source': credential ? 'managed' : 'none',
        'writable': true,
      },
    };

Map<String, dynamic> _model(String providerId, String id, String name) => {
      'providerId': providerId,
      'id': id,
      'displayName': name,
      'inputModalities': ['text'],
      'tools': false,
      'capabilitySource': 'manual',
      'enabled': true,
    };

/// 两条 Provider，各有一个模型；「DeepSeek」那把 key 还没配。
MockClient _backend() => MockClient((request) async {
      final path = request.url.path;
      Object body;
      if (path == '/api/ai/providers') {
        body = [
          _provider('deepseek', 'DeepSeek', credential: false),
          _provider('local-gw', '本机网关'),
        ];
      } else if (path == '/api/ai/providers/deepseek/models') {
        body = [_model('deepseek', 'deepseek-chat', 'DeepSeek Chat')];
      } else if (path == '/api/ai/providers/local-gw/models') {
        body = [_model('local-gw', 'qwen3', 'Qwen3')];
      } else if (path == '/api/ai/conversations' && request.method == 'POST') {
        // 冷启动会顺手建一条空会话（`_startFreshConversation`）；回的必须是**一个对象**，
        // 回 `[]` 会让 `load()` 整个失败，页面停在"连接后端失败"。
        body = {
          'id': 'c1',
          'title': '新对话',
          'providerId': 'deepseek',
          'modelId': 'deepseek-chat',
          'messageCount': 0,
        };
      } else if (path == '/api/ai/conversations') {
        body = <Object>[];
      } else {
        body = <Object>[];
      }
      return http.Response(jsonEncode(body), 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });

Future<AiWorkspaceStore> _pump(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);

  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final store = AiWorkspaceStore(
    api: AiApiClient(settings.baseUrl, client: _backend()),
  );
  await tester.pumpWidget(MultiProvider(
    providers: [
      ChangeNotifierProvider<SettingsStore>.value(value: settings),
      ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
    ],
    child: const MaterialApp(home: AiHomePage()),
  ));
  await tester.pumpAndSettle();
  return store;
}

/// 选择器对话框里的**搜索框**。
///
/// ⚠ 不能用 `find.byType(TextField).first`：输入区的输入框也是 TextField，
/// 而对话框是叠在页面之上的路由（遍历顺序在页面之后），`.first` 拿到的是输入框那个。
Finder _searchBox() =>
    find.descendant(of: find.byType(Dialog), matching: find.byType(TextField));

void main() {
  testWidgets('模型按 Provider 分组：来源在 padding 外，模型套在 padding 里', (tester) async {
    await _pump(tester);

    // 打开选择器（打开那一刻会重读清单与各家的目录）
    await tester.tap(find.text('DeepSeek Chat').first);
    await tester.pumpAndSettle();

    expect(find.text('来自 DeepSeek'), findsOneWidget);
    expect(find.text('来自 本机网关'), findsOneWidget);

    // 布局的硬要求：**模型在 padding 里面，来源那一行在 padding 外面**
    expect(
      find.ancestor(of: find.text('DeepSeek Chat'), matching: find.byType(SliverPadding)),
      findsWidgets,
      reason: '模型行必须套在同一个 padding 里（用户原话）',
    );
    expect(
      find.ancestor(of: find.text('来自 DeepSeek'), matching: find.byType(SliverPadding)),
      findsNothing,
      reason: '来源标注必须在 padding 外面（用户原话）',
    );

    // 没配密钥的那条要**标出来**：不然用户点进去发消息才撞 401
    expect(find.text('未配置密钥'), findsOneWidget);
    expect(find.text('已配置密钥'), findsNothing);
  });

  testWidgets('点另一条 Provider 的模型 = 真的把 Provider 也切过去', (tester) async {
    final store = await _pump(tester);
    expect(store.selectedProvider?.id, 'deepseek',
        reason: '按显示名排序，DeepSeek 是第一条');

    await tester.tap(find.text('DeepSeek Chat').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Qwen3'));
    await tester.pumpAndSettle();

    expect(store.selectedProvider?.id, 'local-gw',
        reason: '选谁家的模型就用谁家的 Provider（用户 bug：新加入的 provider 选不上）');
    expect(store.selectedModel?.id, 'qwen3');

    // 反方向也验一次：换回另一条
    await tester.tap(find.text('Qwen3').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('DeepSeek Chat'));
    await tester.pumpAndSettle();
    expect(store.selectedProvider?.id, 'deepseek');
    expect(store.selectedModel?.id, 'deepseek-chat');
  });

  testWidgets('搜索按组过滤：搜不到的那一组整组不出现', (tester) async {
    await _pump(tester);

    await tester.tap(find.text('DeepSeek Chat').first);
    await tester.pumpAndSettle();

    // ⚠ 搜索框要**圈在对话框里**：输入区也有一个 TextField，`.first` 会点到它头上
    // （对话框是叠在页面之上的路由，遍历顺序在页面之后）。
    await tester.enterText(_searchBox(), 'Qwen');
    await tester.pumpAndSettle();

    expect(find.text('来自 本机网关'), findsOneWidget);
    expect(find.text('Qwen3'), findsOneWidget);
    // 圈在对话框里找：右侧栏「模型能力」那张卡也写着当前模型的名字
    expect(find.descendant(of: find.byType(Dialog), matching: find.text('来自 DeepSeek')),
        findsNothing,
        reason: '一条都搜不到的组不许留一个空标题占屏');
    expect(find.descendant(of: find.byType(Dialog), matching: find.text('DeepSeek Chat')),
        findsNothing);
  });

  testWidgets('一条 Provider 也没有时，说清楚去哪儿建', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    final store = AiWorkspaceStore(
      api: AiApiClient(settings.baseUrl, client: MockClient((request) async {
            return http.Response(jsonEncode(<Object>[]), 200,
                headers: {'content-type': 'application/json; charset=utf-8'});
          })),
    );
    await tester.pumpWidget(MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<AiWorkspaceStore>.value(value: store),
      ],
      child: const MaterialApp(home: AiHomePage()),
    ));
    await tester.pumpAndSettle();

    // 没有 Provider 时输入区那一条就是入口（`_ModelPicker` 的按钮；宽屏与右侧栏各有一个）
    await tester.tap(find.text('未选择模型').first);
    await tester.pumpAndSettle();
    expect(find.descendant(of: find.byType(Dialog), matching: find.textContaining('还没有配置 Provider')),
        findsOneWidget);
  });
}
