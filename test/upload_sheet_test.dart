// 「上传生成产物」面板（`lib/widgets/upload_sheet.dart`）的回归。
//
// 后端一次能回三份名单：`items`（入库成功）、`duplicates`（内容重复跳过的）、
// `failed`（这条没成的）。面板必须**三个数字都如实说**，并且按"到底有没有东西入库"
// 决定关不关面板：
//
//   · 有一条进库（items 或 duplicates 非空）→ 关面板、返回 true、弹一句汇总；
//   · 一条都没进（全是 failed）→ **面板留着**，让用户改完直接重试；
//   · 接口直接报错 → 同理留着，并说「上传失败」而不是假装成功。
//
// 另外两条：Web 端拿不到本地路径时要当场说清（不是转圈），以及"没选文件"时
// 上传按钮是灰的。

import 'dart:convert';
import 'dart:io';

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
import 'package:viewer/widgets/upload_sheet.dart';

import 'support/drain_real_io.dart';
import 'support/fake_file_picker.dart';

const _mediaJson = {
  'id': 1,
  'kind': 'IMAGE',
  'title': 'a',
  'originalName': 'a.png',
  'storedName': 'a.png',
  'sizeBytes': 100,
  'fileUrl': '/api/media/1/file',
};

class _UploadBackend {
  final List<String> calls = [];

  Map<String, dynamic> response = {
    'items': [_mediaJson],
    'duplicates': <Object>[],
    'failed': <Object>[],
  };

  bool fail = false;

  MockClient get client => MockClient((request) async {
    final path = request.url.path;
    calls.add('${request.method} $path');

    if (path == '/api/media/upload') {
      if (fail) return _error('存储目录写不进去');
      return _ok(response);
    }

    // 上传成功后刷新画廊 / 提示词 / 标签 / 统计，给一套空库。
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
  late Directory dir;
  late _UploadBackend backend;
  late FakeFilePicker picker;
  late LibraryStore store;
  late SettingsStore settings;

  late String fileA;
  late String fileB;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    dir = Directory.systemTemp.createTempSync('comfyhub-upload-sheet-');
    fileA = _makeFile(dir, 'a.png', 100);
    fileB = _makeFile(dir, 'b.png', 2048);

    backend = _UploadBackend();
    picker = FakeFilePicker(
      files: [
        FakePlatformFile.local(fileA, size: 100),
        FakePlatformFile.local(fileB, size: 2048),
      ],
    )..install();

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
    try {
      dir.deleteSync(recursive: true);
    } catch (_) {}
  });

  /// 打开面板；面板关掉时把 `showUploadSheet` 的结果交给 [onResult]。
  Future<void> openSheet(
    WidgetTester tester, {
    int? promptId,
    Prompt? prompt,
    void Function(bool? result)? onResult,
  }) async {
    tester.view.physicalSize = const Size(900, 1800);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ChangeNotifierProvider<LibraryStore>.value(
        value: store,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => Center(
                child: TextButton(
                  onPressed: () async {
                    final ok = await showUploadSheet(
                      context,
                      promptId: promptId,
                      prompt: prompt,
                    );
                    onResult?.call(ok);
                  },
                  child: const Text('打开上传'),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开上传'));
    await tester.pumpAndSettle();
  }

  Future<void> pick(WidgetTester tester) async {
    await tester.tap(find.text('选择文件…'));
    await tester.pumpAndSettle();
  }

  Future<void> upload(WidgetTester tester) async {
    await tester.tap(find.byType(FilledButton).last);
    // 上传要读磁盘上的文件（`MultipartFile.fromPath`），假时钟推不动
    await drainRealIo(tester);
    await tester.pumpAndSettle();
  }

  testWidgets('没选文件时上传是灰的；选完显示文件名与大小；带进来的提示词已关联', (tester) async {
    await openSheet(
      tester,
      promptId: 9,
      prompt: const Prompt(
        id: 9,
        title: '夜景',
        kind: PromptKind.image,
        positivePrompt: 'night',
      ),
    );

    final emptyButton = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, '上传  个文件'),
    );
    expect(emptyButton.onPressed, isNull);
    expect(find.text('已关联 #9 夜景'), findsOneWidget);

    await pick(tester);

    expect(picker.pickFilesCalls, 1);
    expect(find.text('重新选择（已选 2 个）'), findsOneWidget);
    expect(find.text('a.png'), findsOneWidget);
    expect(find.text('b.png'), findsOneWidget);
    expect(find.text('100 B'), findsOneWidget);
    expect(find.text('2.0 KB'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, '上传 2 个文件'))
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('部分失败：成功 / 跳过重复 / 失败 三个数字都如实说，面板关掉并返回 true', (tester) async {
    backend.response = {
      'items': [_mediaJson],
      'duplicates': [
        {'name': 'dup.png'},
      ],
      'failed': [
        {'name': 'broken.png', 'error': '读不动'},
      ],
    };

    await openSheet(tester);
    await pick(tester);
    await upload(tester);

    expect(backend.calls, contains('POST /api/media/upload'));
    expect(find.text('成功 1 个，跳过重复 1 个，失败 1 个'), findsOneWidget);
    expect(find.text('上传生成产物'), findsNothing, reason: '有东西入库就该关面板');
  });

  testWidgets('只有重复也算处理过：关面板并返回 true，不说成"成功 0 个"就完了', (tester) async {
    backend.response = {
      'items': <Object>[],
      'duplicates': [
        {'name': 'a.png'},
        {'name': 'b.png'},
      ],
      'failed': <Object>[],
    };

    bool? result;
    await openSheet(tester, onResult: (ok) => result = ok);
    await pick(tester);
    await upload(tester);

    expect(result, isTrue);
    expect(find.text('成功 0 个，跳过重复 2 个'), findsOneWidget);
    expect(find.text('上传生成产物'), findsNothing);
  });

  testWidgets('全部失败：面板不关，留在原地可以改完重试', (tester) async {
    backend.response = {
      'items': <Object>[],
      'duplicates': <Object>[],
      'failed': [
        {'name': 'broken.png', 'error': '不是图片'},
      ],
    };

    bool? result;
    await openSheet(tester, onResult: (ok) => result = ok);
    await pick(tester);
    await upload(tester);

    expect(find.text('成功 0 个，失败 1 个'), findsOneWidget);
    expect(find.text('上传生成产物'), findsOneWidget, reason: '一条都没进库，面板要留着');
    expect(result, isNull);

    // 改好之后重试：同一份选择，直接能成
    backend.response = {
      'items': [_mediaJson],
      'duplicates': <Object>[],
      'failed': <Object>[],
    };
    await upload(tester);

    expect(result, isTrue);
    expect(find.text('上传生成产物'), findsNothing);
  });

  testWidgets('接口报错：显示「上传失败」并把原因带上，面板不关', (tester) async {
    backend.fail = true;

    await openSheet(tester);
    await pick(tester);
    await upload(tester);

    expect(find.textContaining('上传失败'), findsOneWidget);
    expect(find.textContaining('存储目录写不进去'), findsOneWidget);
    expect(find.text('上传生成产物'), findsOneWidget);
  });

  testWidgets('Web 端拿不到本地路径：当场说清，不发请求', (tester) async {
    picker.files = [
      FakePlatformFile(
        'a.png',
        Uri.parse('http://example.com/a.png'),
        size: 100,
      ),
    ];

    await openSheet(tester);
    await pick(tester);
    await upload(tester);

    expect(find.text('无法读取文件路径（Web 端暂不支持上传）'), findsOneWidget);
    expect(
      backend.calls.where((c) => c == 'POST /api/media/upload'),
      isEmpty,
      reason: '没有本地文件可读，不该把空请求打出去',
    );
  });

  testWidgets('取消：关面板并返回 false', (tester) async {
    bool? result;
    await openSheet(tester, onResult: (ok) => result = ok);

    await tester.tap(find.widgetWithText(TextButton, '取消'));
    await tester.pumpAndSettle();

    expect(result, isFalse);
  });
}

String _makeFile(Directory dir, String name, int bytes) {
  final file = File('${dir.path}${Platform.pathSeparator}$name');
  file.writeAsBytesSync(List<int>.filled(bytes, 65));
  return file.path;
}
