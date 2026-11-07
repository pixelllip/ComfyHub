// 「设置 → 数据目录」页（`lib/pages/settings_data_dir_page.dart`）的回归。
//
// 用例针对的是**破坏性的那几步**（用户 2026-11-06："还要考虑数据迁移"）：
//   1) 搬之前必须先弹确认弹窗，说清"从哪到哪、多少文件、旧目录不删"；取消 = 一个字节都不动；
//   2) 目标不可用（同一个目录 / 目录套目录 / 目标里已经有一份数据）只提示、不进确认弹窗；
//   3) 搬完一定要问一次"旧数据删不删"，默认保留；说删也只删 `.db` 与 `storage`；
//   4) 搬迁成功但后端用新目录起不来 → 指针回滚、旧目录还在、如实说；
//   5) 没定位到数据目录时按钮是灰的，不是"点了没反应"。
//
// 后端的起停在这条用例里是**假的**（`_FakeLauncher`）：单元测试不该真的拉进程。
// 文件复制 / 校验 / 落指针走的是真实现（`data_dir_migration.dart`），落点在
// 系统临时目录里，用例之间互不影响。

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/settings_store.dart';
import 'package:viewer/pages/settings_data_dir_page.dart';

import 'support/drain_real_io.dart';
import 'support/fake_file_picker.dart';

/// 只替换「起停后端」，其余（projectRoot / dataDir / layout）走真实现。
class _FakeLauncher extends BackendLauncher {
  _FakeLauncher(super.settings, {List<bool> ensureResults = const [true]})
    : _ensureResults = [...ensureResults];

  /// 依次返回给 `ensureRunning` 的结果（用完就一直用最后一个）。
  List<bool> _ensureResults;

  int stopCalls = 0;
  final List<bool> ensureForced = [];

  @override
  Future<bool> stopBackend() async {
    stopCalls++;
    return true;
  }

  @override
  Future<bool> ensureRunning({bool force = false}) async {
    ensureForced.add(force);
    if (_ensureResults.isEmpty) return true;
    final value = _ensureResults.first;
    if (_ensureResults.length > 1) _ensureResults = _ensureResults.sublist(1);
    return value;
  }
}

void main() {
  late Directory root;
  late Directory target;
  late _FakeLauncher launcher;
  late FakeFilePicker picker;
  late SettingsStore settings;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    root = Directory.systemTemp.createTempSync('comfyhub-datadir-page-root-');
    target = Directory.systemTemp.createTempSync(
      'comfyhub-datadir-page-target-',
    );
    _write(root, ['.db', 'comfyhub.db'], 'db');
    _write(root, ['storage', 'media', 'a.png'], 'img');
    _write(root, ['backups', 'comfyhub-old.db'], 'backup');
    _write(root, ['.run', 'api.json'], '{}');

    settings = SettingsStore();
    await settings.load();
    launcher = _FakeLauncher(settings)..layout = BackendLayout(root: root.path);

    picker = FakeFilePicker(directory: target.path)..install();
  });

  tearDown(() {
    picker.restore();
    launcher.dispose();
    for (final dir in [root, target]) {
      try {
        dir.deleteSync(recursive: true);
      } catch (_) {}
    }
  });

  Future<void> pumpPage(WidgetTester tester) async {
    tester.view.physicalSize = const Size(900, 1600);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      ChangeNotifierProvider<BackendLauncher>.value(
        value: launcher,
        child: const MaterialApp(home: DataDirSettingsPage()),
      ),
    );
    await tester.pumpAndSettle();
  }

  /// 点一下，然后把**真磁盘 IO** 推完（量目录大小 / 复制 / 删文件都是真 IO）。
  Future<void> tapWithIo(WidgetTester tester, Finder target) async {
    await tester.tap(target);
    await drainRealIo(tester);
    await tester.pumpAndSettle();
  }

  /// 点「更改位置…」并等目标检查（真磁盘 IO）跑完、弹窗画出来。
  Future<void> pickTarget(WidgetTester tester) =>
      tapWithIo(tester, find.text('更改位置…'));

  testWidgets('默认目录：说清库和产物在哪，不给「搬回程序目录」', (tester) async {
    await pumpPage(tester);

    expect(find.text('数据目录'), findsOneWidget);
    expect(find.text(root.path), findsOneWidget);
    expect(find.text('产物在 ${dataStorageDirOf(root.path)}'), findsOneWidget);
    expect(find.textContaining('库和产物都在程序目录里'), findsOneWidget);
    expect(find.text('打开'), findsOneWidget);
    expect(find.text('更改位置…'), findsOneWidget);
    expect(
      find.text('搬回程序目录'),
      findsNothing,
      reason: '本来就在默认目录，不该给一个"搬回自己"的按钮',
    );
  });

  testWidgets('数据目录搬走过之后：给「搬回程序目录」，说明里如实区分', (tester) async {
    expect(writeDataDirOverride(root.path, target.path), isTrue);
    await pumpPage(tester);

    expect(find.text(target.path), findsOneWidget);
    expect(find.textContaining('库和产物在这个目录里'), findsOneWidget);
    expect(find.text('搬回程序目录'), findsOneWidget);
  });

  testWidgets('没定位到数据目录：按钮是灰的，不是点了没反应', (tester) async {
    launcher.layout = null;
    await pumpPage(tester);

    expect(find.text('还没定位到数据目录'), findsOneWidget);
    expect(
      tester
          .widget<TextButton>(find.widgetWithText(TextButton, '打开'))
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<TextButton>(find.widgetWithText(TextButton, '更改位置…'))
          .onPressed,
      isNull,
    );
  });

  testWidgets('确认弹窗：说清从哪到哪、多少文件，取消则一个字节都不动', (tester) async {
    await pumpPage(tester);

    await pickTarget(tester);

    expect(find.text('把数据搬到新位置？'), findsOneWidget);
    expect(find.text('从  ${root.path}'), findsOneWidget);
    expect(
      find.text('搬到  ${Directory(target.path).absolute.path}'),
      findsOneWidget,
    );
    expect(find.textContaining('一共 2 个文件'), findsOneWidget);
    expect(find.textContaining('搬的时候本地服务会先停下来，搬完自动重启'), findsOneWidget);

    await tester.tap(find.widgetWithText(TextButton, '取消'));
    await tester.pumpAndSettle();

    expect(readDataDirOverride(root.path), isNull, reason: '取消后不能落指针');
    expect(
      Directory(dataStorageDirOf(target.path)).existsSync(),
      isFalse,
      reason: '取消后新目录里不许出现半份副本',
    );
    expect(launcher.stopCalls, 0, reason: '用户没确认，不许先把本地服务停掉');
    expect(find.text('搬好了'), findsNothing);
  });

  testWidgets('还没有数据时，确认弹窗换一句话（不是"0 个文件"）', (tester) async {
    Directory(dataDbParentOf(root.path)).deleteSync(recursive: true);
    Directory(dataStorageDirOf(root.path)).deleteSync(recursive: true);
    await pumpPage(tester);

    await pickTarget(tester);

    expect(find.text('现在还没有数据，切换过去就是空的。'), findsOneWidget);
  });

  testWidgets('目标不可用：只提示一句人话，不进确认弹窗、不碰数据', (tester) async {
    picker.directory = [root.path, 'sub'].join(Platform.pathSeparator);
    await pumpPage(tester);

    await pickTarget(tester);

    expect(find.text('不能搬到这个目录'), findsOneWidget);
    expect(find.textContaining('新目录不能放在当前数据目录里面'), findsOneWidget);
    expect(find.text('把数据搬到新位置？'), findsNothing);

    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();

    expect(launcher.stopCalls, 0);
    expect(readDataDirOverride(root.path), isNull);
  });

  testWidgets('目标里已经有一份数据：拒绝并指向「备份与恢复」，那是换库不是搬家', (tester) async {
    _write(target, ['.db', 'comfyhub.db'], 'another db');
    await pumpPage(tester);

    await pickTarget(tester);

    expect(find.text('不能搬到这个目录'), findsOneWidget);
    expect(find.textContaining('备份与恢复'), findsOneWidget);
    expect(launcher.stopCalls, 0);
  });

  testWidgets('搬成功：复制 + 落指针 + 问旧数据删不删；默认「先留着」就一个都不删', (tester) async {
    await pumpPage(tester);

    await pickTarget(tester);
    await tapWithIo(tester, find.widgetWithText(FilledButton, '开始搬'));
    await tester.pumpAndSettle();

    // 新位置里真的有了这两样，指针也指过去了
    expect(
      File([target.path, '.db', 'comfyhub.db'].join(Platform.pathSeparator))
          .existsSync(),
      isTrue,
    );
    expect(
      File(
        [target.path, 'storage', 'media', 'a.png'].join(Platform.pathSeparator),
      ).existsSync(),
      isTrue,
    );
    expect(readDataDirOverride(root.path), target.path);
    expect(launcher.stopCalls, 1, reason: '搬之前必须先停本地服务');
    expect(launcher.ensureForced, contains(true));

    // 搬完必须问一次（用户 2026-11-06："搬完会问你一次"）
    expect(find.text('搬好了'), findsOneWidget);
    expect(find.textContaining('旧目录还在：${root.path}'), findsOneWidget);
    await tester.tap(find.widgetWithText(TextButton, '先留着'));
    await tester.pumpAndSettle();

    expect(
      File([root.path, '.db', 'comfyhub.db'].join(Platform.pathSeparator))
          .existsSync(),
      isTrue,
      reason: '默认保留旧数据',
    );
    expect(
      File(
        [root.path, 'storage', 'media', 'a.png'].join(Platform.pathSeparator),
      ).existsSync(),
      isTrue,
    );
  });

  testWidgets('搬成功后确认删除：只删旧目录里的 .db 与 storage，备份 / 运行记录不动', (tester) async {
    await pumpPage(tester);

    await pickTarget(tester);
    await tapWithIo(tester, find.widgetWithText(FilledButton, '开始搬'));
    await tapWithIo(tester, find.widgetWithText(FilledButton, '删掉旧数据'));

    expect(Directory(dataDbParentOf(root.path)).existsSync(), isFalse);
    expect(
      File(
        [root.path, 'storage', 'media', 'a.png'].join(Platform.pathSeparator),
      ).existsSync(),
      isFalse,
    );
    // 程序目录里的东西一个都不许碰
    expect(
      File(
        [root.path, 'backups', 'comfyhub-old.db'].join(Platform.pathSeparator),
      ).existsSync(),
      isTrue,
    );
    expect(
      File([root.path, '.run', 'api.json'].join(Platform.pathSeparator))
          .existsSync(),
      isTrue,
    );
    expect(find.textContaining('旧数据已删除'), findsOneWidget);
  });

  testWidgets('搬完但后端用新目录起不来：指针回滚、旧目录还在、如实告诉用户', (tester) async {
    // 第一次（新目录）起不来；回滚之后（旧目录）能起来
    launcher.dispose();
    launcher = _FakeLauncher(settings, ensureResults: const [false, true])
      ..layout = BackendLayout(root: root.path);
    await pumpPage(tester);

    await pickTarget(tester);
    await tapWithIo(tester, find.widgetWithText(FilledButton, '开始搬'));
    expect(find.text('搬完了，但后端用新目录没起来'), findsOneWidget);
    expect(find.textContaining('已经改回原来的目录并重启'), findsOneWidget);
    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();

    expect(
      readDataDirOverride(root.path),
      isNull,
      reason: '后端起不来就必须把指针改回去，否则下次启动还是坏的目录',
    );
    expect(launcher.ensureForced.where((f) => f).length, 2);
    // 旧数据一直没动过
    expect(
      File([root.path, '.db', 'comfyhub.db'].join(Platform.pathSeparator))
          .existsSync(),
      isTrue,
    );
    // 新目录里的副本留着（用户自己决定删不删），界面说清了它在哪
    expect(readDataDirOverride(root.path), isNull);
    expect(Directory(dataStorageDirOf(target.path)).existsSync(), isTrue);
  });
}

void _write(Directory base, List<String> parts, String content) {
  final file = File([base.path, ...parts].join(Platform.pathSeparator));
  file.parent.createSync(recursive: true);
  file.writeAsStringSync(content);
}
