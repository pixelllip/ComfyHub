// 数据目录迁移（`lib\core\data_dir_migration.dart`）的回归。
//
// 用户 2026-11-06："我能在设置内修改数据目录吗？还要考虑数据迁移。"
//
// 这里钉住四件事：
//   1) 搬什么 —— 库 + storage，**不搬** tmp（中转）与 app-support（偏好）；
//   2) 什么时候不许搬 —— 同一个目录 / 目录套目录 / 目标里已有一份数据；
//   3) 校验通过才落指针；取消 / 失败都不写指针、不动旧数据；
//   4) 删旧数据只删 .db 与 storage，程序目录里的备份 / 日志 / 工作流一个都不碰。

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/data_dir_migration.dart';

void main() {
  late Directory root;
  late Directory oldData;
  late Directory newData;

  setUp(() {
    root = Directory.systemTemp.createTempSync('comfyhub-datadir-root-');
    oldData = Directory.systemTemp.createTempSync('comfyhub-datadir-old-');
    newData = Directory('${root.path}${Platform.pathSeparator}new-data');
  });

  tearDown(() {
    for (final dir in [root, oldData]) {
      try {
        dir.deleteSync(recursive: true);
      } catch (_) {}
    }
  });

  String p(List<String> parts) => parts.join(Platform.pathSeparator);

  void write(Directory base, List<String> parts, String content) {
    final file = File(p([base.path, ...parts]));
    file.parent.createSync(recursive: true);
    file.writeAsStringSync(content);
  }

  bool exists(Directory base, List<String> parts) =>
      File(p([base.path, ...parts])).existsSync();

  void seed() {
    write(oldData, ['.db', 'comfyhub.db'], 'db'); // 2 字节
    write(oldData, ['.db', 'comfyhub.db-wal'], 'wal'); // 3
    write(oldData, ['storage', 'media', 'a.png'], 'img'); // 3
    write(oldData, ['storage', 'ai', 'skills', 'x', 'SKILL.md'], 'skill'); // 5
    // 下面两个**不该搬**：tmp 是 24 小时就清的中转文件；app-support 已经搬到
    // `<根>\app-support`（偏好不跟着数据走）
    write(oldData, ['storage', 'tmp', 'junk.bin'], 'junk');
    write(oldData, ['storage', 'app-support', 'shared_preferences.json'], 'prefs');
    // 程序目录里的东西：删旧数据时一个都不许碰
    write(root, ['backups', 'comfyhub-x.db'], 'backup');
    write(root, ['.run', 'api.json'], '{}');
  }

  group('量：搬什么', () {
    test('只算库与 storage，跳过 tmp / app-support', () async {
      seed();
      final usage = await measureDataDir(oldData.path);
      expect(usage.files, 4);
      expect(usage.bytes, 2 + 3 + 3 + 5);
    });
  });

  group('查：什么目标不许用', () {
    test('同一个目录 / 当前目录里面 / 上一级都拒绝', () async {
      seed();
      expect((await checkDataDirTarget(target: oldData.path, current: oldData.path)).ok,
          isFalse);
      final inside = p([oldData.path, 'sub']);
      expect((await checkDataDirTarget(target: inside, current: oldData.path)).ok,
          isFalse);
      final parent = oldData.parent.path;
      expect((await checkDataDirTarget(target: parent, current: oldData.path)).ok,
          isFalse);
    });

    test('目标里已经有一份数据 → 拒绝（那是换库，不是搬家）', () async {
      seed();
      write(newData, ['.db', 'comfyhub.db'], 'another db');
      final check = await checkDataDirTarget(target: newData.path, current: oldData.path);
      expect(check.ok, isFalse);
      expect(check.message, contains('备份与恢复'));
    });

    test('干净的新目录 → 通过（会建出来）', () async {
      seed();
      final check = await checkDataDirTarget(target: newData.path, current: oldData.path);
      expect(check.ok, isTrue);
      expect(check.source.files, 4);
      expect(Directory(newData.path).existsSync(), isTrue);
    });
  });

  group('搬：复制 + 校验 + 落指针', () {
    test('数据搬过去、tmp 与 app-support 不搬、指针指向新目录', () async {
      seed();
      final result = await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );

      expect(result.ok, isTrue, reason: result.message);
      expect(result.pointerWritten, isTrue);
      expect(exists(newData, ['.db', 'comfyhub.db']), isTrue);
      expect(exists(newData, ['.db', 'comfyhub.db-wal']), isTrue);
      expect(exists(newData, ['storage', 'media', 'a.png']), isTrue);
      expect(exists(newData, ['storage', 'ai', 'skills', 'x', 'SKILL.md']), isTrue);
      expect(exists(newData, ['storage', 'tmp', 'junk.bin']), isFalse,
          reason: 'tmp 是中转文件，不跟着搬');
      expect(exists(newData, ['storage', 'app-support', 'shared_preferences.json']),
          isFalse, reason: '偏好已经搬到 <根>\\app-support，不跟着数据走');

      expect(readDataDirOverride(root.path), newData.path);
      expect(dataDirOf(root.path), newData.path);

      // 旧数据原样留着（用户确认后才删）
      expect(exists(oldData, ['.db', 'comfyhub.db']), isTrue);
      expect(exists(oldData, ['storage', 'media', 'a.png']), isTrue);
    });

    test('取消：不写指针、旧数据不动，并如实说新目录里有半份副本', () async {
      seed();
      var calls = 0;
      final result = await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
        isCancelled: () => ++calls > 1,
      );

      expect(result.ok, isFalse);
      expect(result.cancelled, isTrue);
      expect(result.message, contains('旧数据没动'));
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);
    });

    test('有未完成的库恢复：拒绝搬（标记里是旧路径，搬走会找不到 tmp）', () async {
      seed();
      write(oldData, ['.db', 'comfyhub.db.restore-pending.json'], '{}');
      final result = await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );
      expect(result.ok, isFalse);
      expect(result.message, contains('库恢复'));
      expect(readDataDirOverride(root.path), isNull);
    });
  });

  group('删旧数据：只删 .db 与 storage', () {
    test('备份 / 运行记录留在程序目录，旧数据目录被清空', () async {
      seed();
      await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );

      final report = await deleteOldData(oldData.path);

      expect(report.failed, isEmpty);
      expect(report.deletedFiles, greaterThan(0));
      expect(Directory(dataDbParentOf(oldData.path)).existsSync(), isFalse);
      expect(exists(oldData, ['storage', 'media', 'a.png']), isFalse);
      // 偏好文件可能还被"刚升级、没重启"的进程用着 —— 删旧数据必须留着它
      expect(exists(oldData, ['storage', 'app-support', 'shared_preferences.json']),
          isTrue);
      // 程序目录里的东西一个都没动
      expect(exists(root, ['backups', 'comfyhub-x.db']), isTrue);
      expect(exists(root, ['.run', 'api.json']), isTrue);
    });
  });
}
