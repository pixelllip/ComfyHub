// 「设置 → 数据目录」页（`lib/pages/settings_data_dir_page.dart`）的回归。
//
// 交互约束：一个按钮执行搬移，一个按钮搬回默认位置，
// 页面上不许出现他看不懂的技术词（「整理成标准布局」「把指针改到这里」都已删掉）。
//
// 用例钉住这几条：
//   1) 页面上只有「搬到别处…」和「搬回默认位置」，外加一个"打开文件夹"的小图标；
//      两条技术按钮一个都不许回来；
//   2) 点「搬到别处…」选完目录**直接开搬**（不再多问一遍），进度弹窗说清从哪到哪、
//      多少文件，可以取消；取消 = 一个字节都不动、指针不动；
//   3) 目标不可用（同一个目录 / 目录套目录）只提示一句人话，不碰数据；
//   4) 搬完一定要问一次"旧数据删不删"，默认保留；说删也只删 `.db` 与 `storage`；
//   5) 搬迁成功但后端用新目录起不来 → 指针回滚、旧目录还在、如实说。
//
// 后端的起停在这条用例里是**假的**（`_FakeLauncher`）：单元测试不该真的拉进程。
// 文件复制 / 校验 / 落指针走的是真实现（`data_dir_migration.dart`），落点在
// 系统临时目录里，用例之间互不影响。

import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';
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

  /// 停服务要等这个闸门（测试用它把"进度弹窗开着、还没开始复制"这一刻钉住）。
  /// `null` = 立即返回。
  Completer<void>? stopGate;

  @override
  Future<bool> stopBackend() async {
    stopCalls++;
    final gate = stopGate;
    if (gate != null) await gate.future;
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

  /// 点「搬到别处…」，等目标检查跑完、进度弹窗画出来（此时还没开始复制）。
  Future<void> startMove(WidgetTester tester) async {
    await tester.tap(find.text('搬到别处…'));
    await drainRealIo(tester);
    await tester.pump();
  }

  testWidgets('默认目录：只有「搬到别处…」，两条技术按钮都不在', (tester) async {
    await pumpPage(tester);

    expect(find.text('数据目录'), findsOneWidget);
    expect(find.text(root.path), findsOneWidget);
    expect(find.text('对话、图片和角色卡都保存在程序所在的文件夹里。'), findsOneWidget);
    expect(find.text('搬到别处…'), findsOneWidget);
    expect(find.text('搬回默认位置'), findsNothing);
    expect(find.byTooltip('打开文件夹'), findsOneWidget);
    // 这两个技术按钮已删除
    expect(find.text('整理成标准布局'), findsNothing);
    expect(find.text('把指针改到这里'), findsNothing);
    expect(find.textContaining('同一棵树'), findsNothing);
  });

  testWidgets('数据搬到别处之后：多一个「搬回默认位置」', (tester) async {
    expect(writeDataDirOverride(root.path, target.path), isTrue);
    await pumpPage(tester);

    expect(find.text(target.path), findsOneWidget);
    expect(find.textContaining('搬到别处'), findsOneWidget);
    expect(find.textContaining('程序所在的文件夹中不再保存这些内容'), findsOneWidget);
    expect(find.text('搬回默认位置'), findsOneWidget);
  });

  testWidgets('没定位到数据目录：两个按钮都是灰的，不是点了没反应', (tester) async {
    launcher.layout = null;
    await pumpPage(tester);

    expect(find.text('尚未确定数据的存放位置'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, '搬到别处…'))
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<OutlinedButton>(
            find.widgetWithText(OutlinedButton, '搬回默认位置'),
          )
          .onPressed,
      isNull,
    );
  });

  testWidgets('点一次就开始搬：进度里说清从哪到哪、多少文件，取消则一个字节都不动', (tester) async {
    launcher.stopGate = Completer<void>();
    await pumpPage(tester);

    await startMove(tester);

    expect(find.text('正在搬运'), findsOneWidget);
    expect(find.text('现在的位置是 ${root.path}'), findsOneWidget);
    expect(
      find.text('新的位置是 ${Directory(target.path).absolute.path}'),
      findsOneWidget,
    );
    expect(find.textContaining('共 2 个文件'), findsOneWidget);
    expect(find.widgetWithText(TextButton, '取消'), findsOneWidget);

    await tester.tap(find.widgetWithText(TextButton, '取消'));
    launcher.stopGate!.complete();
    launcher.stopGate = null;
    await drainRealIo(tester);
    await tester.pumpAndSettle();

    expect(find.text('已取消'), findsOneWidget);
    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();

    expect(readDataDirOverride(root.path), isNull, reason: '取消后不能落指针');
    expect(
      Directory(dataStorageDirOf(target.path)).existsSync(),
      isFalse,
      reason: '取消后新目录里不许出现半份副本',
    );
    expect(find.text('搬运完成'), findsNothing);
  });

  testWidgets('还没有数据时：进度里换一句话，不是"0 个文件"', (tester) async {
    Directory(dataDbParentOf(root.path)).deleteSync(recursive: true);
    Directory(dataStorageDirOf(root.path)).deleteSync(recursive: true);
    launcher.stopGate = Completer<void>();
    await pumpPage(tester);

    await startMove(tester);

    expect(find.text('当前的位置没有数据，搬运后的新位置同样是空的。'), findsOneWidget);
    await tester.tap(find.widgetWithText(TextButton, '取消'));
    launcher.stopGate!.complete();
    launcher.stopGate = null;
    await drainRealIo(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();
  });

  testWidgets('目标不可用：只提示一句人话，不碰数据', (tester) async {
    picker.directory = [root.path, 'sub'].join(Platform.pathSeparator);
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬到别处…'));

    expect(find.text('不能搬到这个文件夹'), findsOneWidget);
    expect(find.textContaining('新的位置不能位于当前位置的内部'), findsOneWidget);
    expect(find.text('正在搬运'), findsNothing);

    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();

    expect(launcher.stopCalls, 0);
    expect(readDataDirOverride(root.path), isNull);
  });

  testWidgets('目标里已经有一份数据：搬完说清已经备份+覆盖', (tester) async {
    _write(target, ['.db', 'comfyhub.db'], 'another db');
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬到别处…'));

    expect(find.text('搬运完成'), findsOneWidget);
    expect(find.textContaining('这份数据已保存到'), findsOneWidget);
    expect(
      File([target.path, '.db', 'comfyhub.db'].join(Platform.pathSeparator))
          .readAsStringSync(),
      'db',
    );
    final backups = Directory(
      [root.path, 'backups'].join(Platform.pathSeparator),
    ).listSync().whereType<Directory>().toList();
    expect(backups, hasLength(1));
    expect(
      File([backups.single.path, 'comfyhub.db'].join(Platform.pathSeparator))
          .readAsStringSync(),
      'another db',
      reason: '覆盖之前必须把目标原来那份库备份下来',
    );

    await tester.tap(find.widgetWithText(TextButton, '暂不删除'));
    await tester.pumpAndSettle();
  });

  testWidgets('数据在别处：仍然说出"在别处"，并给搬回默认位置的入口', (tester) async {
    expect(writeDataDirOverride(root.path, target.path), isTrue);
    _write(target, ['.db', 'comfyhub.db'], 'db');
    _write(target, ['media', 'a.png'], 'img');
    await pumpPage(tester);

    expect(find.text(target.path), findsOneWidget);
    expect(find.textContaining('原来的文件夹仍然存在'), findsNothing);
    expect(find.text('搬到别处…'), findsOneWidget, reason: '在别处也能再搬一次');
    expect(find.text('搬回默认位置'), findsOneWidget);
  });

  testWidgets('搬成功：复制 + 落指针 + 问旧数据删不删；默认「先留着」就一个都不删', (tester) async {
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬到别处…'));

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

    // 搬移完成后必须询问一次
    expect(find.text('搬运完成'), findsOneWidget);
    expect(find.textContaining('原来的文件夹仍然存在：${root.path}'), findsOneWidget);
    await tester.tap(find.widgetWithText(TextButton, '暂不删除'));
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

  testWidgets('「搬回默认位置」：一键搬回程序目录', (tester) async {
    expect(writeDataDirOverride(root.path, target.path), isTrue);
    _write(target, ['.db', 'comfyhub.db'], 'db');
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬回默认位置'));
    await tester.tap(find.widgetWithText(TextButton, '暂不删除'));
    await tester.pumpAndSettle();

    expect(
      readDataDirOverride(root.path),
      anyOf(isNull, root.path),
      reason: '搬回默认位置：指针要么清掉、要么就写默认目录，反正不再是别处',
    );
    expect(
      File([root.path, '.db', 'comfyhub.db'].join(Platform.pathSeparator))
          .readAsStringSync(),
      'db',
      reason: '旧目录里那份一个字节都没动',
    );
  });

  testWidgets('搬成功后确认删除：只删旧目录里的 .db 与 storage，备份 / 运行记录不动', (tester) async {
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬到别处…'));
    await tapWithIo(tester, find.widgetWithText(FilledButton, '删除原来的数据'));

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
    expect(find.textContaining('原来的数据已删除'), findsOneWidget);
  });

  testWidgets('搬完但后端用新目录起不来：指针回滚、旧目录还在、如实告诉用户', (tester) async {
    // 第一次（新目录）起不来；回滚之后（旧目录）能起来
    launcher.dispose();
    launcher = _FakeLauncher(settings, ensureResults: const [false, true])
      ..layout = BackendLayout(root: root.path);
    await pumpPage(tester);

    await tapWithIo(tester, find.text('搬到别处…'));

    expect(find.text('搬运完成，但程序无法从新位置启动'), findsOneWidget);
    expect(find.textContaining('已恢复原来的位置并重新启动'), findsOneWidget);
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
    // 新目录中的副本保留（是否删除由界面决定），界面说明其位置
    expect(Directory(dataStorageDirOf(target.path)).existsSync(), isTrue);
  });
}

void _write(Directory base, List<String> parts, String content) {
  final file = File([base.path, ...parts].join(Platform.pathSeparator));
  file.parent.createSync(recursive: true);
  file.writeAsStringSync(content);
}
