// 产物导出弹窗的三条回归（2026-10-27）：
//
//  1. 范围 / 目录 / 两个开关**原样**变成 `POST /api/media/export` 的请求体 ——
//     后端只认 camelCase，写错一个键就等于"静默按全部导出"；
//  2. 目录空着时**一个请求都不发**，就地报错（别让用户对着转圈猜）；
//  3. 回执里的失败条目要真的显示出来 —— 后端说"有 3 条没导出去"，界面只报"完成"
//     就是骗人。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/export_dialog.dart';

/// 请求体 / 返回体由每条用例自己设。
class _Harness {
  final List<http.Request> requests = [];
  Map<String, dynamic> response = {
    'dir': r'D:\out',
    'manifest': r'D:\out\导出清单.json',
    'exported': 2,
    'collections': 0,
    'workflows': 1,
    'totalBytes': 2048,
    'byKind': {'IMAGE': 2},
    'failed': <Object>[],
    'warnings': <Object>[],
  };

  http.Client client() => MockClient((request) async {
    requests.add(request);
    return http.Response(
      jsonEncode(response),
      200,
      headers: {'content-type': 'application/json; charset=utf-8'},
    );
  });
}

Future<LibraryStore> _store(_Harness harness) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  return LibraryStore(
    settings,
    api: ApiClient(settings.baseUrl, client: harness.client()),
  );
}

Widget _host(LibraryStore store, {List<int>? mediaIds}) => MultiProvider(
  providers: [ChangeNotifierProvider<LibraryStore>.value(value: store)],
  child: MaterialApp(
    home: Builder(
      builder: (context) => Scaffold(
        body: Center(
          child: ElevatedButton(
            onPressed: () => showExportDialog(context, mediaIds: mediaIds),
            child: const Text('打开导出'),
          ),
        ),
      ),
    ),
  ),
);

void main() {
  testWidgets('选中范围与两个开关原样变成请求体', (tester) async {
    final harness = _Harness();
    final store = await _store(harness);
    await tester.pumpWidget(_host(store, mediaIds: [11, 12]));
    await tester.tap(find.text('打开导出'));
    await tester.pumpAndSettle();

    expect(find.text('选中的 2 个产物'), findsOneWidget, reason: '范围要说清楚');
    await tester.enterText(find.byType(TextField), r'D:\导出');
    await tester.tap(find.text('开始导出'));
    await tester.pumpAndSettle();

    expect(harness.requests, hasLength(1));
    expect(harness.requests.single.method, 'POST');
    expect(harness.requests.single.url.path, '/api/media/export');
    final body =
        jsonDecode(harness.requests.single.body) as Map<String, dynamic>;
    expect(body['dir'], r'D:\导出');
    expect(body['mediaIds'], [11, 12]);
    expect(body.containsKey('collectionId'), isFalse);
    expect(body['includeWorkflow'], isTrue);
    expect(body['includeApiWorkflow'], isTrue);

    // 回执：数量 + 清单路径都要看得见
    expect(find.text('导出完成'), findsOneWidget);
    expect(find.textContaining('产物 2 个'), findsOneWidget);
    expect(find.textContaining(r'D:\out\导出清单.json'), findsOneWidget);
  });

  testWidgets('目录空着时一个请求都不发', (tester) async {
    final harness = _Harness();
    final store = await _store(harness);
    await tester.pumpWidget(_host(store));
    await tester.tap(find.text('打开导出'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('开始导出'));
    await tester.pumpAndSettle();

    expect(harness.requests, isEmpty, reason: '没有目录就别去打扰后端');
    expect(find.text('先选一个导出目录。'), findsOneWidget);
  });

  testWidgets('失败条目必须显示出来，不能只报完成', (tester) async {
    final harness = _Harness();
    harness.response = {
      'dir': r'D:\out',
      'manifest': r'D:\out\导出清单.json',
      'exported': 1,
      'collections': 0,
      'workflows': 0,
      'totalBytes': 1024,
      'byKind': {'IMAGE': 1},
      'failed': [
        {'mediaId': 7, 'name': 'gone.png', 'reason': '文件不在盘上：gone.png'},
      ],
      'warnings': <Object>[],
    };
    final store = await _store(harness);
    await tester.pumpWidget(_host(store, mediaIds: [7]));
    await tester.tap(find.text('打开导出'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), r'D:\导出');
    await tester.tap(find.text('开始导出'));
    await tester.pumpAndSettle();

    expect(find.text('导出完成，但有条目没导出去'), findsOneWidget);
    expect(find.textContaining('gone.png'), findsOneWidget);
    expect(find.textContaining('文件不在盘上'), findsOneWidget);
  });

  testWidgets('窄屏 360 不溢出（弹窗要能在手机宽度下完整显示）', (tester) async {
    await tester.binding.setSurfaceSize(const Size(360, 640));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final harness = _Harness();
    final store = await _store(harness);
    await tester.pumpWidget(_host(store));
    await tester.tap(find.text('打开导出'));
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
    expect(find.text('全部产物'), findsOneWidget);
    expect(find.text('仅收藏'), findsOneWidget);
  });

  test('回执解析缺字段时回落到零，不抛', () {
    final report = MediaExportReport.fromJson({'dir': r'D:\x'});
    expect(report.exported, 0);
    expect(report.byKind, isEmpty);
    expect(report.failed, isEmpty);
    expect(report.hasFailures, isFalse);
  });
}
