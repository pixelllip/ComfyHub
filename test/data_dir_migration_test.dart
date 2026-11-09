// 数据目录迁移（`lib\core\data_dir_migration.dart`）的回归。
//
// 用户 2026-11-06："我能在设置内修改数据目录吗？还要考虑数据迁移。"
// 用户 2026-11-09：搬过去之后变成 `storage\storage`、手动挪平后软件不认，
// 想搬回上一级 / 程序目录又都被"目标里已经有一份数据"挡回来。
//
// 这里钉住这些事：
//   1) 搬什么 —— 库 + 产物，**不搬** tmp（中转）与 app-support（偏好）；
//   2) 三种产物布局都认（标准 / 扁平 / 双层），搬过去一律写成标准布局；
//   3) 什么时候不许搬 —— 同一个目录 / 目录套目录 / 更上一级 / 上一级里有别的数据；
//   4) 目标里已经有一份库时**先备份再覆盖**，不再把人踢去「备份与恢复」；
//   5) 校验通过才落指针；取消 / 失败都不写指针、不动旧数据；
//   6) 删旧数据只删 `.db` 与 old storage，程序目录里的备份 / 日志 / 工作流一个都不碰，
//      搬到上一级时更不许碰"原地留着"的产物。

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/data_dir_migration.dart';

void main() {
  late Directory base;
  late Directory root;
  late Directory oldData;
  late Directory newData;

  String p(List<String> parts) => parts.join(Platform.pathSeparator);

  setUp(() {
    base = Directory.systemTemp.createTempSync('comfyhub-datadir-');
    root = Directory(p([base.path, 'root']))..createSync();
    oldData = Directory(p([base.path, 'old-data']))..createSync();
    newData = Directory(p([base.path, 'new-data']));
  });

  tearDown(() {
    try {
      base.deleteSync(recursive: true);
    } catch (_) {}
  });

  void write(Directory base, List<String> parts, String content) {
    final file = File(p([base.path, ...parts]));
    file.parent.createSync(recursive: true);
    file.writeAsStringSync(content);
  }

  bool exists(Directory base, List<String> parts) =>
      File(p([base.path, ...parts])).existsSync();

  /// 标准布局（`<数据>\.db` + `<数据>\storage`）
  void seed() {
    write(oldData, ['.db', 'comfyhub.db'], 'db'); // 2 字节
    write(oldData, ['.db', 'comfyhub.db-wal'], 'wal'); // 3
    write(oldData, ['storage', 'media', 'a.png'], 'img'); // 3
    write(oldData, ['storage', 'ai', 'skills', 'x', 'SKILL.md'], 'skill'); // 5
    // 下面两个**不该搬**：tmp 是 24 小时就清的中转文件；app-support 已经搬到
    // `<根>\app-support`（偏好不跟着数据走）
    write(oldData, ['storage', 'tmp', 'junk.bin'], 'junk');
    write(oldData, [
      'storage',
      'app-support',
      'shared_preferences.json',
    ], 'prefs');
    // 程序目录里的东西：删旧数据时一个都不许碰
    write(root, ['backups', 'comfyhub-x.db'], 'backup');
    write(root, ['.run', 'api.json'], '{}');
  }

  /// 扁平布局：用户把里层 `storage` 的内容挪到了数据目录本身。
  void seedFlat(Directory data) {
    write(data, ['.db', 'comfyhub.db'], 'db');
    write(data, ['media', 'a.png'], 'img');
    write(data, ['ai', 'skills', 'x', 'SKILL.md'], 'skill');
    write(data, ['tmp', 'junk.bin'], 'junk');
  }

  group('量：搬什么', () {
    test('标准布局只算库与 storage，跳过 tmp / app-support', () async {
      seed();
      final usage = await measureDataDir(oldData.path);
      expect(usage.files, 4);
      expect(usage.bytes, 2 + 3 + 3 + 5);
    });

    test('扁平布局把数据目录里的产物算进来，tmp 不算', () async {
      seedFlat(oldData);
      final usage = await measureDataDir(oldData.path);
      expect(usage.files, 3);
      expect(usage.bytes, 2 + 3 + 5);
    });
  });

  group('目录归一：选到 storage 文件夹时按上一级算', () {
    test('storage / Storage 都归一到上一级，普通名字不动', () {
      expect(
        normalizeDataDirTarget(p([base.path, 'ComfyHub', 'storage'])),
        p([base.path, 'ComfyHub']),
      );
      expect(
        normalizeDataDirTarget(p([base.path, 'ComfyHub', 'Storage'])),
        p([base.path, 'ComfyHub']),
      );
      expect(
        normalizeDataDirTarget(p([base.path, 'ComfyHub', 'data'])),
        p([base.path, 'ComfyHub', 'data']),
      );
      expect(isStorageNamedDir(p([base.path, 'ComfyHub', 'Storage'])), isTrue);
      expect(isStorageNamedDir(p([base.path, 'ComfyHub', 'data'])), isFalse);
    });
  });

  group('查：什么目标不许用', () {
    test('同一个目录 / 当前目录里面拒绝', () async {
      seed();
      expect(
        (await checkDataDirTarget(
          target: oldData.path,
          current: oldData.path,
        )).ok,
        isFalse,
      );
      final inside = p([oldData.path, 'sub']);
      expect(
        (await checkDataDirTarget(target: inside, current: oldData.path)).ok,
        isFalse,
      );
    });

    test('上一级是干净目录 → 允许（"搬回程序目录"就是这个形状）', () async {
      final parent = Directory(p([base.path, 'plain-parent']))..createSync();
      final data = Directory(p([parent.path, 'data']))..createSync();
      write(data, ['.db', 'comfyhub.db'], 'db');
      write(data, ['storage', 'media', 'a.png'], 'img');

      final check = await checkDataDirTarget(
        target: parent.path,
        current: data.path,
      );
      expect(check.ok, isTrue, reason: check.message);
    });

    test('上一级里已经有一份别的数据 → 拒绝（不能混在一起）', () async {
      final parent = Directory(p([base.path, 'busy-parent']))..createSync();
      final data = Directory(p([parent.path, 'data']))..createSync();
      write(data, ['.db', 'comfyhub.db'], 'db');
      write(data, ['storage', 'media', 'a.png'], 'img');
      write(parent, ['storage', 'media', 'other.png'], 'other');

      final check = await checkDataDirTarget(
        target: parent.path,
        current: data.path,
      );
      expect(check.ok, isFalse);
      expect(check.message, contains('上一级'));
    });

    test('更上一级 → 拒绝（那是个大目录，不是数据目录）', () async {
      final mid = Directory(p([base.path, 'mid']))..createSync();
      final data = Directory(p([mid.path, 'data']))..createSync();
      write(data, ['.db', 'comfyhub.db'], 'db');
      write(data, ['storage', 'media', 'a.png'], 'img');

      final check = await checkDataDirTarget(
        target: base.path,
        current: data.path,
      );
      expect(check.ok, isFalse);
      expect(check.message, contains('更上一级'));
    });

    test('目标里已经有一份数据 → 允许，但标明会备份+覆盖', () async {
      seed();
      write(newData, ['.db', 'comfyhub.db'], 'another db');

      final check = await checkDataDirTarget(
        target: newData.path,
        current: oldData.path,
      );
      expect(check.ok, isTrue);
      expect(check.replacesTarget, isTrue);
      expect(check.message, contains('备份'));
      expect(check.message, contains('覆盖'));
    });

    test('干净的新目录 → 通过（会建出来）', () async {
      seed();
      final check = await checkDataDirTarget(
        target: newData.path,
        current: oldData.path,
      );
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
      expect(
        exists(newData, ['storage', 'ai', 'skills', 'x', 'SKILL.md']),
        isTrue,
      );
      expect(
        exists(newData, ['storage', 'tmp', 'junk.bin']),
        isFalse,
        reason: 'tmp 是中转文件，不跟着搬',
      );
      expect(
        exists(newData, ['storage', 'app-support', 'shared_preferences.json']),
        isFalse,
        reason: '偏好已经搬到 <根>\\app-support，不跟着数据走',
      );

      expect(readDataDirOverride(root.path), newData.path);
      expect(dataDirOf(root.path), newData.path);

      // 旧数据原样留着（用户确认后才删）
      expect(exists(oldData, ['.db', 'comfyhub.db']), isTrue);
      expect(exists(oldData, ['storage', 'media', 'a.png']), isTrue);
    });

    test('扁平布局：产物搬进目标的 storage，不再跟着数据目录平铺', () async {
      seedFlat(oldData);
      final result = await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );

      expect(result.ok, isTrue, reason: result.message);
      expect(exists(newData, ['.db', 'comfyhub.db']), isTrue);
      expect(exists(newData, ['storage', 'media', 'a.png']), isTrue);
      expect(
        exists(newData, ['storage', 'ai', 'skills', 'x', 'SKILL.md']),
        isTrue,
      );
      expect(exists(newData, ['storage', 'tmp', 'junk.bin']), isFalse);
      expect(exists(newData, ['media', 'a.png']), isFalse);
      expect(readDataDirOverride(root.path), newData.path);
    });

    test('搬到上一级：产物原地不动、库挪上去，旧库先备份到 backups', () async {
      final parent = Directory(p([base.path, 'ComfyHub']))..createSync();
      final storage = Directory(p([parent.path, 'storage']))..createSync();
      write(storage, ['.db', 'comfyhub.db'], 'new-db');
      write(storage, ['media', 'a.png'], 'img');
      write(parent, ['.db', 'comfyhub.db'], 'old-db');

      final check = await checkDataDirTarget(
        target: parent.path,
        current: storage.path,
      );
      expect(check.ok, isTrue, reason: check.message);
      expect(check.replacesTarget, isTrue);

      final result = await migrateDataDir(
        source: storage.path,
        target: parent.path,
        root: root.path,
      );

      expect(result.ok, isTrue, reason: result.message);
      expect(result.backupPath, isNotNull);
      expect(
        File(p([parent.path, '.db', 'comfyhub.db'])).readAsStringSync(),
        'new-db',
      );
      expect(
        File(p([parent.path, 'storage', 'media', 'a.png'])).existsSync(),
        isTrue,
      );
      expect(
        Directory(p([parent.path, 'storage', 'storage'])).existsSync(),
        isFalse,
        reason: '不能再套一层 storage',
      );
      expect(readDataDirOverride(root.path), parent.path);

      final backups = Directory(p([root.path, 'backups']))
          .listSync()
          .whereType<Directory>()
          .toList();
      expect(backups, hasLength(1));
      expect(
        File(p([backups.single.path, 'comfyhub.db'])).readAsStringSync(),
        'old-db',
      );

      // 搬完之后"旧数据目录"就是新产物目录：只清旧库，产物一个都不许动。
      final report = await deleteOldData(
        storage.path,
        newStorageDir: resolvedStorageDirOf(parent.path),
      );
      expect(report.failed, isEmpty);
      expect(
        File(p([storage.path, '.db', 'comfyhub.db'])).existsSync(),
        isFalse,
      );
      expect(
        File(p([storage.path, 'media', 'a.png'])).existsSync(),
        isTrue,
        reason: '这是新产物目录，删旧数据时不许碰',
      );
    });

    test('双层布局搬到上一级：把 storage\\storage 压平成标准的 storage', () async {
      final parent = Directory(p([base.path, 'Nested']))..createSync();
      final data = Directory(p([parent.path, 'storage']))..createSync();
      write(data, ['.db', 'comfyhub.db'], 'new-db');
      write(data, ['storage', 'storage', 'media', 'a.png'], 'img');
      write(parent, ['.db', 'comfyhub.db'], 'old-db');

      final check = await checkDataDirTarget(
        target: parent.path,
        current: data.path,
      );
      expect(check.ok, isTrue, reason: check.message);

      final result = await migrateDataDir(
        source: data.path,
        target: parent.path,
        root: root.path,
      );
      expect(result.ok, isTrue, reason: result.message);
      expect(
        File(p([parent.path, 'storage', 'media', 'a.png'])).existsSync(),
        isTrue,
        reason: '里层 storage 的内容要被压到上一层的 storage 里',
      );
      expect(
        Directory(p([parent.path, 'storage', 'storage'])).existsSync(),
        isTrue,
        reason: '旧的双层目录还在（删旧数据时才清）',
      );
      expect(
        File(p([parent.path, '.db', 'comfyhub.db'])).readAsStringSync(),
        'new-db',
      );

      final report = await deleteOldData(
        data.path,
        newStorageDir: resolvedStorageDirOf(parent.path),
      );
      expect(report.failed, isEmpty);
      expect(
        File(p([data.path, 'media', 'a.png'])).existsSync(),
        isTrue,
        reason: '压平后的产物就在这里是新 storage，不许删',
      );
      expect(
        Directory(p([data.path, 'storage'])).existsSync(),
        isFalse,
        reason: '旧的双层副本要清掉',
      );
    });

    test('目标已有库：先备份再覆盖，产物同名覆盖、旧文件留着', () async {
      seed();
      write(newData, ['.db', 'comfyhub.db'], 'other-db');
      write(newData, ['storage', 'media', 'b.png'], 'other');

      final result = await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );

      expect(result.ok, isTrue, reason: result.message);
      expect(result.backupPath, isNotNull);
      expect(
        File(p([newData.path, '.db', 'comfyhub.db'])).readAsStringSync(),
        'db',
      );
      expect(exists(newData, ['storage', 'media', 'a.png']), isTrue);
      expect(
        exists(newData, ['storage', 'media', 'b.png']),
        isTrue,
        reason: '目标里多出来的旧产物不删，只是同名覆盖',
      );
      expect(
        File(p([result.backupPath!, 'comfyhub.db'])).readAsStringSync(),
        'other-db',
      );
      expect(readDataDirOverride(root.path), newData.path);
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

  group('删旧数据：只删 .db 与产物，程序目录原样', () {
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
      expect(
        exists(oldData, ['storage', 'app-support', 'shared_preferences.json']),
        isTrue,
      );
      // 程序目录里的东西一个都没动
      expect(exists(root, ['backups', 'comfyhub-x.db']), isTrue);
      expect(exists(root, ['.run', 'api.json']), isTrue);
    });

    test('扁平布局搬到别处：旧的扁平产物要清掉，但不碰新产物目录', () async {
      seedFlat(oldData);
      await migrateDataDir(
        source: oldData.path,
        target: newData.path,
        root: root.path,
      );

      final report = await deleteOldData(
        oldData.path,
        newStorageDir: resolvedStorageDirOf(newData.path),
      );

      expect(report.failed, isEmpty);
      expect(exists(oldData, ['media', 'a.png']), isFalse);
      expect(exists(oldData, ['ai', 'skills', 'x', 'SKILL.md']), isFalse);
      expect(exists(newData, ['storage', 'media', 'a.png']), isTrue);
      expect(
        exists(newData, ['storage', 'ai', 'skills', 'x', 'SKILL.md']),
        isTrue,
      );
    });
  });
}
