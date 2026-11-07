// 「设置 → ComfyUI 自动捕获」页（`lib/pages/settings_comfy_capture_page.dart`）的回归。
//
// 这一页几乎全是"调一次后端、成功说一句、失败说另一句"，所以用例按**四条失败分支**
// 各钉一条（加载 / 保存 / 探测 / 导入），再加上"成功时界面真的变成了新的那份配置"。
//
// 三条容易漏的：
//   1) 加载失败时「保存配置」必须是灰的 —— 手里没有 `_capture`，拿空配置去覆盖
//      用户设置比"保存不了"糟得多；
//   2) 探测是**只读**的（"自动查找 ComfyUI"只显示结果），点了「使用这个目录」才写；
//      找到了但和当前配置一样时不给按钮，写一句"已经在用这个目录了"；
//   3) 保存时把输出目录清空 = 显式清掉（`clearOutputDir`），不是"没改"。

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/api_client.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/settings_comfy_capture_page.dart';
import 'package:viewer/state/library_store.dart';

import 'support/fake_file_picker.dart';

class _CaptureBackend {
  final List<String> calls = [];
  Map<String, dynamic>? lastConfigBody;
  String? lastImportDir;

  bool failConfigGet = false;
  bool failConfigPut = false;
  bool failStatus = false;
  bool failLocate = false;
  bool failApply = false;
  bool failImport = false;
  bool failImportWorkflows = false;
  bool failPoll = false;

  Map<String, dynamic> config = {
    'enabled': true,
    'comfyUrl': 'http://127.0.0.1:8188',
    'outputDir': r'D:\Comfy\output',
    'pollSeconds': 6,
    'autoTag': 'ComfyUI',
    'maxPerPoll': 20,
    'downloadFallback': true,
  };

  Map<String, dynamic> status = {
    'enabled': true,
    'comfyUrl': 'http://127.0.0.1:8188',
    'comfyReachable': true,
    'queueRunning': 2,
    'queuePending': 1,
    'capturedRuns': 3,
    'capturedMedia': 5,
    'lastError': '上一次连不上 127.0.0.1:8188',
    'recent': [
      {
        'runKey': 'run-1',
        'status': 'success',
        'mediaCount': 2,
        'title': '一只猫',
        'capturedAt': '2026-11-07T03:00:00Z',
      },
      {
        'runKey': 'run-2',
        'status': 'error',
        'mediaCount': 0,
        'capturedAt': '2026-11-07T03:05:00Z',
      },
    ],
  };

  Map<String, dynamic> locate = {
    'home': r'D:\Comfy',
    'outputDir': r'D:\Comfy\output',
    'source': '注册表',
    'configuredOutputDir': null,
  };

  MockClient get client => MockClient((request) async {
    final path = request.url.path;
    calls.add('${request.method} $path');

    if (path == '/api/capture/config' && request.method == 'GET') {
      if (failConfigGet) return _error('配置读不出来');
      return _ok(config);
    }
    if (path == '/api/capture/config' && request.method == 'PUT') {
      if (failConfigPut) return _error('配置写不进库');
      lastConfigBody = (jsonDecode(request.body) as Map)
          .cast<String, dynamic>();
      config = {...config, ...lastConfigBody!};
      return _ok(config);
    }
    if (path == '/api/capture/status') {
      if (failStatus) return _error('状态读不出来');
      return _ok(status);
    }
    if (path == '/api/capture/locate' && request.method == 'GET') {
      if (failLocate) return _error('探测挂了');
      return _ok(locate);
    }
    if (path == '/api/capture/locate/apply') {
      if (failApply) return _error('写不进配置');
      final dir = ((jsonDecode(request.body) as Map)['outputDir'] ?? '')
          .toString();
      config = {...config, 'outputDir': dir};
      return _ok(config);
    }
    if (path == '/api/capture/poll') {
      if (failPoll) return _error('轮询请求被拒');
      return _ok({'ok': true, 'checked': 3, 'newRuns': 1, 'newMedia': 2});
    }
    if (path == '/api/capture/import') {
      if (failImport) return _error('这个目录读不动');
      lastImportDir = ((jsonDecode(request.body) as Map)['dir'] ?? '')
          .toString();
      return _ok({
        'dir': lastImportDir,
        'scanned': 3,
        'imported': 2,
        'duplicates': 1,
        'failed': 0,
        'promptsCreated': 1,
        'errors': <String>[],
      });
    }
    if (path == '/api/capture/import-workflows') {
      if (failImportWorkflows) return _error('workflows 目录读不动');
      return _ok({
        'dirs': [r'D:\Comfy\user\default\workflows'],
        'scanned': 4,
        'imported': 3,
        'duplicates': 1,
        'failed': 0,
        'promptIds': [11, 12, 13],
      });
    }

    // `_syncNow` / `_importFolder` 成功后会 `refreshAll()`，给它一套空库。
    if (path == '/api/prompts' || path == '/api/media') {
      return _ok({
        'items': <Object>[],
        'total': 0,
        'page': 1,
        'size': 20,
        'pages': 0,
      });
    }
    if (path == '/api/tags' || path == '/api/tags/categories') {
      return _ok(<Object>[]);
    }
    if (path == '/api/stats') return _ok(<String, Object>{});
    return _error('接口不存在: $path', 404);
  });

  static http.Response _ok(Object? body) => http.Response(
    jsonEncode(body),
    200,
    headers: const {'content-type': 'application/json; charset=utf-8'},
  );

  static http.Response _error(String message, [int code = 500]) =>
      http.Response(
        jsonEncode({'error': message}),
        code,
        headers: const {'content-type': 'application/json; charset=utf-8'},
      );
}

void main() {
  late _CaptureBackend backend;
  late FakeFilePicker picker;
  late LibraryStore store;
  late SettingsStore settings;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    backend = _CaptureBackend();
    picker = FakeFilePicker(directory: r'D:\Comfy\output')..install();
    settings = SettingsStore();
    await settings.load();
    store = LibraryStore(
      settings,
      api: ApiClient('http://127.0.0.1:9999', client: backend.client),
    );
  });

  tearDown(() {
    picker.restore();
    store.dispose();
    settings.dispose();
  });

  Future<void> pumpPage(WidgetTester tester) async {
    tester.view.physicalSize = const Size(900, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: const MaterialApp(home: ComfyCaptureSettingsPage()),
      ),
    );
    await tester.pumpAndSettle();
  }

  String fieldText(WidgetTester tester, int index) => tester
      .widget<TextField>(find.byType(TextField).at(index))
      .controller!
      .text;

  testWidgets('加载成功：状态、最近捕获、上一次错误、三项配置都如实显示', (tester) async {
    await pumpPage(tester);

    expect(
      find.text('已连上 ComfyUI http://127.0.0.1:8188（队列 运行中 2 / 等待 1）'),
      findsOneWidget,
    );
    expect(
      find.textContaining('最近一次错误是 上一次连不上 127.0.0.1:8188'),
      findsOneWidget,
    );
    expect(find.text('最近捕获'), findsOneWidget);
    expect(find.text('一只猫'), findsOneWidget);
    expect(find.textContaining('2 个产物'), findsOneWidget);
    expect(find.textContaining('run-2'), findsOneWidget);

    expect(fieldText(tester, 0), 'http://127.0.0.1:8188');
    expect(fieldText(tester, 1), r'D:\Comfy\output');
    expect(fieldText(tester, 2), 'ComfyUI');
  });

  testWidgets('加载失败：如实报错，且「保存配置」是灰的（不许拿空配置盖用户设置）', (tester) async {
    backend.failConfigGet = true;
    await pumpPage(tester);

    expect(find.textContaining('配置读不出来'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, '保存配置'))
          .onPressed,
      isNull,
    );
    // 还能重新加载
    expect(find.byTooltip('刷新状态'), findsOneWidget);
  });

  testWidgets('保存：清空输出目录是显式清掉，关掉开关如实写进配置', (tester) async {
    await pumpPage(tester);

    await tester.enterText(
      find.byType(TextField).at(0),
      '  http://127.0.0.1:9000  ',
    );
    await tester.enterText(find.byType(TextField).at(1), '   ');
    await tester.tap(find.text('开启自动捕获'));
    await tester.pumpAndSettle();

    await tester.tap(find.widgetWithText(FilledButton, '保存配置'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('PUT /api/capture/config'));
    final body = backend.lastConfigBody!;
    expect(body['comfyUrl'], 'http://127.0.0.1:9000', reason: '两端空格要去掉');
    expect(body['outputDir'], isNull, reason: '清空 = 显式清掉，不是"没改"');
    expect(body['enabled'], isFalse);
    expect(body['pollSeconds'], 6, reason: '没动过的项原样带回去');
    expect(find.text('自动捕获已关闭'), findsOneWidget);
  });

  testWidgets('保存失败：说清是哪一步失败，不假装成功', (tester) async {
    backend.failConfigPut = true;
    await pumpPage(tester);

    await tester.tap(find.widgetWithText(FilledButton, '保存配置'));
    await tester.pumpAndSettle();

    expect(find.textContaining('保存失败'), findsOneWidget);
    expect(find.textContaining('配置写不进库'), findsOneWidget);
    expect(find.text('自动捕获已开启'), findsNothing);
  });

  testWidgets('自动查找：只显示探测结果，点「使用这个目录」才写配置', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.text('自动查找 ComfyUI'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('GET /api/capture/locate'));
    expect(find.text(r'找到 ComfyUI，位置在 D:\Comfy'), findsOneWidget);
    expect(find.textContaining(r'输出目录是 D:\Comfy\output（注册表）'), findsOneWidget);
    expect(
      backend.calls.where(
        (c) => c.startsWith('POST /api/capture/locate/apply'),
      ),
      isEmpty,
      reason: '探测只读：没点「使用这个目录」不许改配置',
    );

    await tester.tap(find.text('使用这个目录'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/capture/locate/apply'));
    expect(find.textContaining('已使用探测到的输出目录'), findsOneWidget);
    expect(fieldText(tester, 1), r'D:\Comfy\output');
  });

  testWidgets('自动查找：没找到时给出该怎么填，不给「使用这个目录」', (tester) async {
    backend.locate = {
      'home': null,
      'outputDir': null,
      'note': '没找到 ComfyUI。请手工填上面的输出目录。',
    };
    await pumpPage(tester);

    await tester.tap(find.text('自动查找 ComfyUI'));
    await tester.pumpAndSettle();

    expect(find.text('没找到 ComfyUI 的安装位置。'), findsOneWidget);
    expect(find.text('没找到 ComfyUI。请手工填上面的输出目录。'), findsOneWidget);
    expect(find.text('使用这个目录'), findsNothing);
  });

  testWidgets('自动查找：已经在用这个目录时只说一句，不给重复写入的按钮', (tester) async {
    backend.locate = {
      'home': r'D:\Comfy',
      'outputDir': r'D:\Comfy\output',
      'configuredOutputDir': r'D:\Comfy\output',
    };
    await pumpPage(tester);

    await tester.tap(find.text('自动查找 ComfyUI'));
    await tester.pumpAndSettle();

    expect(find.text('已经在用这个目录了。'), findsOneWidget);
    expect(find.text('使用这个目录'), findsNothing);
  });

  testWidgets('自动查找失败：说「查找失败」并把原因带上', (tester) async {
    backend.failLocate = true;
    await pumpPage(tester);

    await tester.tap(find.text('自动查找 ComfyUI'));
    await tester.pumpAndSettle();

    expect(find.textContaining('查找失败'), findsOneWidget);
    expect(find.textContaining('探测挂了'), findsOneWidget);
  });

  testWidgets('导入目录：成功显示汇总，失败说「导入失败」', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.text('导入已有产物…'));
    await tester.pumpAndSettle();

    expect(picker.directoryCalls, 1);
    expect(backend.calls, contains('POST /api/capture/import'));
    expect(backend.lastImportDir, r'D:\Comfy\output');
    expect(find.textContaining('扫描 3 个文件，新导入 2'), findsOneWidget);

    backend.failImport = true;
    await tester.tap(find.text('导入已有产物…'));
    await tester.pumpAndSettle();

    expect(find.textContaining('导入失败'), findsOneWidget);
    expect(find.textContaining('这个目录读不动'), findsOneWidget);
  });

  testWidgets('导入本机工作流：成功显示汇总，失败说「导入工作流失败」', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.text('导入本机工作流'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/capture/import-workflows'));
    expect(find.textContaining('扫描 4 份工作流文件，新导入 3'), findsOneWidget);

    backend.failImportWorkflows = true;
    await tester.tap(find.text('导入本机工作流'));
    await tester.pumpAndSettle();

    expect(find.textContaining('导入工作流失败'), findsOneWidget);
    expect(find.textContaining('workflows 目录读不动'), findsOneWidget);
  });

  testWidgets('立即同步：成功说清检查了几次 / 新增几条，失败说「同步失败」', (tester) async {
    await pumpPage(tester);

    await tester.tap(find.text('立即同步'));
    await tester.pumpAndSettle();

    expect(backend.calls, contains('POST /api/capture/poll'));
    expect(find.text('检查 3 次运行，新捕获 1 次（2 个产物）'), findsOneWidget);

    backend.failPoll = true;
    await tester.tap(find.text('立即同步'));
    await tester.pumpAndSettle();

    expect(find.textContaining('同步失败'), findsOneWidget);
    expect(find.textContaining('轮询请求被拒'), findsOneWidget);
  });
}
