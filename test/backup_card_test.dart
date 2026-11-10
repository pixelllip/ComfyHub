// 「备份与恢复」卡的回归（2026-11-06）。
//
// 钉四件事：
//   1) 没有备份时给一句人话，不是空卡片；
//   2) 「立即备份」真的 POST，然后把新备份显示出来（列表刷新过）；
//   3) 「恢复」必须先确认，确认之后 POST /api/system/restore，**再重启服务**；
//   4) 后端留下的 `pending` / `lastRestore` 要如实画出来 —— 恢复到底成没成，
//      仅这两处会显示该信息。

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
import 'package:viewer/widgets/backup_card.dart';

const _backup = {
  'name': 'comfyhub-20261106-131211.db',
  'bytes': 4096,
  'createdAt': '2026-11-06T13:12:11Z',
  'kind': 'auto',
};

const _safety = {
  'name': 'before-restore-20261106-131500.db',
  'bytes': 8192,
  'createdAt': '2026-11-06T13:15:00Z',
  'kind': 'beforeRestore',
};

class _Backend {
  _Backend({List<Map<String, dynamic>>? items}) : items = [...?items];

  final List<Map<String, dynamic>> items;
  final List<String> calls = [];
  String? pending;
  Map<String, dynamic>? lastRestore;

  MockClient get client => MockClient((request) async {
    final path = request.url.path;
    calls.add('${request.method} $path');
    Object? body;
    var status = 200;

    if (path == '/api/system/backups' && request.method == 'GET') {
      body = {
        'dir': r'C:\ComfyHub\backups',
        'items': items,
        'pending': pending,
        'lastRestore': lastRestore,
      };
    } else if (path == '/api/system/backups' && request.method == 'POST') {
      final item = {
        'name': 'comfyhub-20261106-131211.db',
        'bytes': 4096,
        'createdAt': '2026-11-06T13:12:11Z',
        'kind': 'auto',
      };
      items.add(Map<String, dynamic>.from(item));
      status = 201;
      body = {'item': item};
    } else if (path.startsWith('/api/system/backups/') &&
        request.method == 'DELETE') {
      final name = path.split('/').last;
      items.removeWhere((e) => e['name'] == name);
      body = {'deleted': name};
    } else if (path == '/api/system/restore' && request.method == 'POST') {
      final name = (jsonDecode(request.body) as Map)['name'] as String;
      pending = name;
      lastRestore = {
        'ok': true,
        'name': name,
        'message': '已恢复这份备份。恢复前的库另存了一份 before-restore 备份。',
        'at': '2026-11-06T13:16:00Z',
      };
      body = {
        'ok': true,
        'name': name,
        'safety': 'before-restore-20261106-131500.db',
        'restartRequired': true,
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

Future<void> _pump(
  WidgetTester tester,
  _Backend backend, {
  Future<void> Function()? onRestart,
}) async {
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
            child: BackupCard(onRestart: onRestart),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('没有备份时给一句人话', (tester) async {
    final backend = _Backend();
    await _pump(tester, backend);

    expect(find.text('还没有备份。点「立即备份」拍一张。'), findsOneWidget);
    expect(find.text('立即备份'), findsOneWidget);
    expect(find.text('打开备份目录'), findsOneWidget);
  });

  testWidgets('立即备份会 POST，并把新备份显示出来', (tester) async {
    final backend = _Backend();
    await _pump(tester, backend);

    await tester.tap(find.text('立即备份'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/system/backups'));
    expect(find.text('comfyhub-20261106-131211.db'), findsOneWidget);
    expect(find.textContaining('4.0 KB'), findsOneWidget);
    // 列表刷新过（POST 之后又读了一次）
    expect(
      backend.calls.where((c) => c == 'GET /api/system/backups').length,
      greaterThanOrEqualTo(2),
    );
  });

  testWidgets('恢复要先确认，确认之后重启服务', (tester) async {
    var restarted = 0;
    final backend = _Backend(items: [Map<String, dynamic>.from(_backup)]);
    await _pump(tester, backend, onRestart: () async => restarted++);

    await tester.tap(find.widgetWithText(TextButton, '恢复').first);
    await tester.pumpAndSettle();
    expect(find.text('恢复这份备份？'), findsOneWidget);

    // 取消：一个请求都不该发
    await tester.tap(find.widgetWithText(TextButton, '取消'));
    await tester.pumpAndSettle();
    expect(
      backend.calls.where((c) => c.startsWith('POST /api/system/restore')),
      isEmpty,
    );
    expect(restarted, 0);

    await tester.tap(find.widgetWithText(TextButton, '恢复').first);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, '恢复'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/system/restore'));
    expect(restarted, 1, reason: '落完标记必须重启一次，换库发生在启动时');
    // 原样展示后端写入的 lastRestore
    expect(find.textContaining('已恢复这份备份'), findsWidgets);
  });

  testWidgets('待完成的恢复会挂在卡片顶部，可以一键补上最后一步', (tester) async {
    final backend = _Backend(items: [Map<String, dynamic>.from(_backup)])
      ..pending = 'comfyhub-20261106-131211.db';
    var restarted = 0;
    await _pump(tester, backend, onRestart: () async => restarted++);

    expect(find.textContaining('重启本地服务后生效'), findsOneWidget);
    await tester.tap(find.text('重启并完成'));
    await tester.pumpAndSettle();
    expect(restarted, 1);
  });

  testWidgets('320 宽也不溢出（手机上的设置页）', (tester) async {
    tester.view.physicalSize = const Size(320, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final backend = _Backend(
      items: [
        Map<String, dynamic>.from(_backup),
        Map<String, dynamic>.from(_safety),
      ],
    )..pending = 'comfyhub-20261106-131211.db';
    await _pump(tester, backend);

    expect(tester.takeException(), isNull);
    expect(find.text('立即备份'), findsOneWidget);
  });

  testWidgets('删除要确认，删完列表里就没有了', (tester) async {
    final backend = _Backend(
      items: [
        Map<String, dynamic>.from(_backup),
        Map<String, dynamic>.from(_safety),
      ],
    );
    await _pump(tester, backend);
    expect(find.text(_backup['name'] as String), findsOneWidget);
    expect(find.text(_safety['name'] as String), findsOneWidget);

    await tester.tap(find.byTooltip('删掉这份备份').first);
    await tester.pumpAndSettle();
    expect(find.text('删掉这份备份？'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, '删掉'));
    await tester.pumpAndSettle();

    expect(
      backend.calls.where((c) => c.startsWith('DELETE /api/system/backups/')),
      isNotEmpty,
    );
    expect(find.text(_backup['name'] as String), findsNothing);
  });
}
