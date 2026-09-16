// 「AI 模型与凭据」长列表的两个回归（用户报的 bug ③ 卡顿 / bug ④ 滚动条位置不准）：
//
//   ① **滚动条**：懒构建的 ListView 只能用"已布局子项的平均高度"估算 maxScrollExtent，
//      卡片高矮不一时（支持推理的卡片多出 7 个档位 chip），越往下滚估算值越大 ——
//      实测 69 个模型从 3512 涨到 21318，滑块从 17.9% 缩到 3.5%。
//      现在用固定行高的 `SliverFixedExtentList`，滚动范围是常数，滑块不跳。
//   ② **卡顿**：每行不再是"7 个 FilterChip + 下拉框"，而是固定高度的一行 + 编辑弹窗；
//      一趟滚动只构建视口附近的那几行。
//
// 能力编辑没有因此丢：点一行（或右侧调节按钮）打开弹窗，改完点「确定」写回列表，
// 再点「保存模型目录」落库 —— 这两条也一并钉在这里。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/ai_api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/ai_provider_settings_page.dart';

/// 保存模型目录的请求体（按顺序）。
final List<String> modelSaves = [];

Map<String, dynamic> _model(int i) {
  // 前 35 个"不支持推理"（矮卡片），后面 34 个"支持推理且声明 7 档"（高卡片）——
  // Command Code Goat 那 69 个模型的实际形态，也是估算误差最大的形态
  final tall = i >= 35;
  return {
    'providerId': 'goat',
    'id': 'model-$i',
    'displayName': 'Model $i',
    'inputModalities': ['text', if (i % 3 == 0) 'image'],
    'tools': true,
    'reasoning': tall,
    'thinkingEfforts': tall
        ? {
            for (final e in ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']) e: e,
          }
        : <String, String>{},
    'thinkingFormat': tall ? 'openai' : null,
    'capabilitySource': 'builtin',
    'enabled': true,
  };
}

MockClient _backend() => MockClient((request) async {
      final path = request.url.path;
      Object body;
      if (path == '/api/ai/providers') {
        body = [
          {
            'id': 'goat',
            'displayName': 'Command Code Goat',
            'api': 'openai-completions',
            'baseURL': 'https://api.commandcode.ai/provider/v1/',
            'credentialRef': 'GOAT_API_KEY',
            'endpointTrust': 'public',
            'enabled': true,
            'revision': 1,
            'credential': {'configured': true, 'source': 'managed', 'writable': true},
          }
        ];
      } else if (path == '/api/ai/providers/goat/models') {
        if (request.method == 'PUT') {
          modelSaves.add(request.body);
          body = (jsonDecode(request.body) as Map<String, dynamic>)['models'];
        } else {
          body = [for (var i = 0; i < 69; i++) _model(i)];
        }
      } else if (path.endsWith('/builtin-catalog')) {
        body = {
          'ok': true,
          'version': '2026-09b',
          'providerCount': 1,
          'modelCount': 69,
          'divergentCount': 0,
        };
      } else {
        body = <Object>[];
      }
      return http.Response(jsonEncode(body), 200,
          headers: {'content-type': 'application/json; charset=utf-8'});
    });

Future<Widget> _page() async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = AiApiClient(settings.baseUrl, client: _backend());
  return ChangeNotifierProvider<SettingsStore>.value(
    value: settings,
    child: MaterialApp(home: AiProviderSettingsPage(api: api)),
  );
}

/// 打开页面并选中那个 69 个模型的 Provider。
Future<void> _open(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1400, 1000);
  tester.view.devicePixelRatio = 1.0;
  await tester.pumpWidget(await _page());
  await tester.pumpAndSettle();
  await tester.tap(find.text('Command Code Goat'));
  await tester.pumpAndSettle();
}

/// 模型目录那张列表的 ScrollPosition（页面里最长的那个可滚动区域）。
ScrollPosition _modelListPosition(WidgetTester tester) => tester
    .stateList<ScrollableState>(find.byType(Scrollable))
    .map((s) => s.position)
    .reduce((a, b) => a.maxScrollExtent >= b.maxScrollExtent ? a : b);

/// 某一行模型的 Finder（用显示名定位，再往上找那一行的 InkWell）。
Finder _row(String displayName) =>
    find.ancestor(of: find.text(displayName), matching: find.byType(InkWell)).first;

/// 行上的能力图标（tooltip = 模态名）——**只看这一行**：
/// 别的行也有同样的图标，全局找会误判。
Finder _iconIn(Finder row, String label) =>
    find.descendant(of: row, matching: find.byTooltip(label));

void main() {
  setUp(() {
    modelSaves.clear();
  });

  testWidgets('69 个模型：滚动条长度与位置全程稳定（bug ④）', (tester) async {
    addTearDown(tester.view.reset);
    await _open(tester);

    final pos = _modelListPosition(tester);
    double thumbRatio() => pos.viewportDimension / (pos.maxScrollExtent + pos.viewportDimension);

    final atTop = pos.maxScrollExtent;
    final ratioAtTop = thumbRatio();
    expect(atTop, greaterThan(0));

    // 从页顶一路滚到底：滚动范围（也就是滑块长度）必须一直是同一个数
    for (final target in [500.0, 2000.0, 4000.0]) {
      if (target > pos.maxScrollExtent) break;
      pos.jumpTo(target);
      await tester.pump();
      expect(pos.maxScrollExtent, closeTo(atTop, 0.5),
          reason: '滚到 $target 时滚动范围不该变（以前会从 3512 涨到 21318）');
      expect(thumbRatio(), closeTo(ratioAtTop, 0.001));
    }
    pos.jumpTo(pos.maxScrollExtent);
    await tester.pumpAndSettle();
    expect(pos.maxScrollExtent, closeTo(atTop, 0.5), reason: '滚到底也不该变');
    expect(pos.pixels, closeTo(pos.maxScrollExtent, 0.5), reason: '能真的滚到底');
  });

  testWidgets('固定行高 + 懒构建：一趟只建视口附近的行（bug ③）', (tester) async {
    addTearDown(tester.view.reset);
    await _open(tester);

    // 每行一个「编辑能力」按钮；69 行不可能全建出来
    final built = tester.widgetList(find.byTooltip('编辑能力')).length;
    expect(built, greaterThan(0));
    expect(built, lessThan(30), reason: '69 个模型一次只该建出视口附近的那几行，实际建了 $built 行');

    // 行高是常数：随便挑两行量一下
    expect(tester.getSize(_row('Model 0')).height, kModelRowHeight);

    // 后半段（那几个"支持推理"的高卡片）行高也必须一样
    final pos = _modelListPosition(tester);
    pos.jumpTo(pos.maxScrollExtent);
    await tester.pumpAndSettle();
    expect(tester.getSize(_row('Model 68')).height, kModelRowHeight);
  });

  testWidgets('能力编辑搬进弹窗：改完点确定写回行，保存后落库', (tester) async {
    addTearDown(tester.view.reset);
    await _open(tester);

    // 第 0 个模型：text + image + 工具（没有推理）
    expect(_iconIn(_row('Model 0'), '图片'), findsOneWidget);
    expect(_iconIn(_row('Model 0'), '推理'), findsNothing);

    await tester.tap(find.text('Model 0'));
    await tester.pumpAndSettle();
    expect(find.text('模型能力：Model 0'), findsOneWidget, reason: '点一行要能打开编辑弹窗');

    // 打开"思考强度"，勾上「高」；顺手把「图片」关掉
    await tester.tap(find.widgetWithText(FilterChip, '图片'));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilterChip, '高'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();

    // 行上的能力图标跟着变：图片没了、推理来了（这些图标是 hover tooltip）
    expect(_iconIn(_row('Model 0'), '图片'), findsNothing, reason: '取消勾选的模态要立刻反映到行上');
    expect(_iconIn(_row('Model 0'), '推理'), findsOneWidget);

    await tester.tap(find.text('保存模型目录'));
    await tester.pumpAndSettle();

    expect(modelSaves, isNotEmpty, reason: '点保存要真的 PUT');
    final saved = jsonDecode(modelSaves.last) as Map<String, dynamic>;
    final models = (saved['models'] as List).cast<Map<String, dynamic>>();
    final first = models.firstWhere((m) => m['id'] == 'model-0');
    expect(first['inputModalities'], isNot(contains('image')));
    expect(first['reasoning'], isTrue);
    expect((first['thinkingEfforts'] as Map).keys, contains('high'));
    expect(first['capabilitySource'], 'manual', reason: '手工改过就要标成 manual');
    expect(models, hasLength(69), reason: '改一个模型不能把别的模型弄丢');
  });

  testWidgets('编辑弹窗点「取消」不留痕迹', (tester) async {
    addTearDown(tester.view.reset);
    await _open(tester);

    await tester.tap(find.text('Model 0'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilterChip, '图片'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();

    // 行上仍然是"有图片"
    expect(_iconIn(_row('Model 0'), '图片'), findsOneWidget);

    await tester.tap(find.text('保存模型目录'));
    await tester.pumpAndSettle();
    final saved = jsonDecode(modelSaves.last) as Map<String, dynamic>;
    final first = (saved['models'] as List)
        .cast<Map<String, dynamic>>()
        .firstWhere((m) => m['id'] == 'model-0');
    expect(first['inputModalities'], contains('image'));
  });
}
