// 「存储」卡的回归（2026-11-06）。
//
// 钉四件事：
//   1) 各目录占用与事件条数真的画出来（后端字段名写错会当场空掉）；
//   2) 清理孤儿**先扫描、再确认**，确认之后才发 `apply: true`；
//   3) 没有孤儿时只给一句人话，不弹确认框；
//   4) 归档旧事件要先确认，归档完把条数与文件名说出来。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/state/library_store.dart';
import 'package:viewer/widgets/storage_card.dart';

class _Backend {
  int orphanCount = 2;
  int orphanBytes = 2048;
  final List<String> calls = [];
  final List<String> bodies = [];

  Map<String, dynamic> get _usage => {
    'root': r'C:\ComfyHub',
    'totalBytes': 5 * 1024 * 1024,
    'dirs': [
      {
        'key': 'db',
        'label': '数据库',
        'path': r'C:\ComfyHub\.db',
        'bytes': 1024,
        'files': 2,
      },
      {
        'key': 'media',
        'label': '产物原图',
        'path': r'C:\ComfyHub\storage\media',
        'bytes': 4 * 1024 * 1024,
        'files': 12,
      },
    ],
    'events': {
      'rows': 1234,
      'oldest': '2026-09-01T00:00:00Z',
      'newest': '2026-11-06T00:00:00Z',
    },
  };

  MockClient get client => MockClient((request) async {
    final path = request.url.path;
    calls.add('${request.method} $path');
    if (request.body.isNotEmpty) bodies.add(request.body);
    Object? body;
    var status = 200;

    if (path == '/api/system/storage' && request.method == 'GET') {
      body = _usage;
    } else if (path == '/api/system/storage/orphans') {
      final apply =
          request.body.isNotEmpty &&
          (jsonDecode(request.body) as Map)['apply'] == true;
      if (apply) {
        final deleted = orphanCount;
        final bytes = orphanBytes;
        orphanCount = 0;
        orphanBytes = 0;
        body = {
          'deleted': deleted,
          'bytes': bytes,
          'failed': <Object>[],
          'remaining': 0,
        };
      } else {
        body = {
          'items': [
            for (var i = 0; i < orphanCount; i++)
              {
                'kind': 'media',
                'name': 'orphan-$i.png',
                'bytes': 1024,
                'modifiedAt': '2026-09-01T00:00:00Z',
              },
          ],
          'count': orphanCount,
          'bytes': orphanBytes,
          'truncated': false,
        };
      }
    } else if (path == '/api/system/events/archive') {
      body = {
        'report': {
          'archived': 120,
          'bytes': 4096,
          'file':
              r'C:\ComfyHub\storage\ai\run-events\run-events-20261106-220000.jsonl',
          'cutoff': '2026-10-07T00:00:00Z',
          'remaining': 0,
        },
      };
    } else {
      status = 404;
      body = {'error': 'not found'};
    }
    return http.Response(
      jsonEncode(body),
      status,
      headers: const {'content-type': 'application/json'},
    );
  });
}

Future<void> _pump(WidgetTester tester, _Backend backend) async {
  SharedPreferences.setMockInitialValues({});
  final settings = SettingsStore();
  await settings.load();
  final api = ApiClient('http://127.0.0.1:9999', client: backend.client);
  final store = LibraryStore(settings, api: api);
  final launcher = BackendLauncher(settings);
  addTearDown(launcher.dispose);

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<SettingsStore>.value(value: settings),
        ChangeNotifierProvider<BackendLauncher>.value(value: launcher),
        ChangeNotifierProvider<LibraryStore>.value(value: store),
      ],
      child: MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            padding: const EdgeInsets.all(12),
            child: const StorageCard(),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('占用与事件条数都画出来', (tester) async {
    await _pump(tester, _Backend());

    expect(find.text('产物原图'), findsOneWidget);
    expect(find.text('4.0 MB · 12 个文件'), findsOneWidget);
    expect(find.text('1.0 KB · 2 个文件'), findsOneWidget);
    expect(find.text('5.0 MB'), findsOneWidget);
    expect(find.textContaining('1234 条'), findsOneWidget);
    expect(find.textContaining('2026-09-01'), findsOneWidget);
  });

  testWidgets('清理孤儿要先扫描、再确认', (tester) async {
    final backend = _Backend();
    await _pump(tester, backend);

    await tester.tap(find.text('清理孤儿文件…'));
    await tester.pumpAndSettle();

    expect(find.text('清理孤儿文件？'), findsOneWidget);
    expect(find.textContaining('2 个文件'), findsWidgets);
    expect(find.textContaining('2.0 KB'), findsOneWidget);

    await tester.tap(find.widgetWithText(FilledButton, '删掉'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/system/storage/orphans'));
    expect(
      backend.bodies.first,
      '{}',
      reason: '扫描也要带 JSON body，后端是 Json<OrphanInput>',
    );
    expect(
      backend.bodies.any((b) => b.contains('"apply":true')),
      isTrue,
      reason: '确认之后才允许 apply: true',
    );
    expect(find.textContaining('删掉了 2 个文件'), findsOneWidget);
    expect(find.textContaining('2.0 KB'), findsWidgets);
  });

  testWidgets('没有孤儿时只给一句人话，不弹确认框', (tester) async {
    final backend = _Backend()
      ..orphanCount = 0
      ..orphanBytes = 0;
    await _pump(tester, backend);

    await tester.tap(find.text('清理孤儿文件…'));
    await tester.pumpAndSettle();

    expect(find.text('清理孤儿文件？'), findsNothing);
    expect(find.text('没有孤儿文件，磁盘上的东西在库里都有记录。'), findsOneWidget);
    expect(
      backend.bodies.any((b) => b.contains('"apply":true')),
      isFalse,
      reason: '没东西可删就不该发删除请求',
    );
  });

  testWidgets('归档旧事件要先确认，并把条数与文件名说出来', (tester) async {
    final backend = _Backend();
    await _pump(tester, backend);

    await tester.tap(find.text('归档旧事件…'));
    await tester.pumpAndSettle();

    expect(find.text('归档旧事件？'), findsOneWidget);
    expect(find.textContaining('1234 条'), findsWidgets);

    await tester.tap(find.widgetWithText(FilledButton, '归档'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/system/events/archive'));
    expect(find.textContaining('归档了 120 条事件'), findsOneWidget);
    expect(find.textContaining('run-events-20261106-220000.jsonl'), findsOneWidget);
  });

  testWidgets('320 宽不溢出', (tester) async {
    tester.view.physicalSize = const Size(320, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pump(tester, _Backend());
    expect(tester.takeException(), isNull);
    expect(find.text('刷新'), findsOneWidget);
  });
}
