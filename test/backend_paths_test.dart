// 「根目录在哪、后端 exe 在哪」（`lib\core\backend_paths.dart`）的回归测试。
//
// 这是**全 App 唯一的一份布局解析**（源码树 / Release 两种布局、
// `COMFYHUB_SERVER_EXE` 覆盖），`scripts\backend.ps1` 是它在脚本世界的镜像 ——
// 判据漂移的表现是"App 起了 A、脚本说在 B"，所以这里把顺序钉死。

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/library_identity.dart';

void main() {
  late Directory root;

  setUp(() {
    root = Directory.systemTemp.createTempSync('comfyhub-paths-');
  });

  tearDown(() {
    try {
      root.deleteSync(recursive: true);
    } catch (_) {}
  });

  String p(List<String> parts) => parts.join(Platform.pathSeparator);

  void touch(List<String> parts) {
    final file = File(p([root.path, ...parts]));
    file.parent.createSync(recursive: true);
    file.writeAsStringSync('x');
  }

  group('looksLikeRoot：三种判据命中任意一个就算', () {
    test('源码树判据 server-rs\\Cargo.toml', () {
      touch(['server-rs', 'Cargo.toml']);
      expect(looksLikeRoot(root.path), isTrue);
    });

    test('两种布局都有的 db\\schema.sqlite.sql', () {
      touch(['db', 'schema.sqlite.sql']);
      expect(looksLikeRoot(root.path), isTrue);
    });

    test('schema 与 backend\\exe 同时存在 = 产物目录，不算源码树', () {
      // 现场（2026-10-11）：`flutter build windows --debug` 会把 DDL 和后端一起装进
      // `build\...\Debug\`，于是它同时命中"源码树"和"产物"两条判据 ——
      // 被当成源码树的话，Debug 版 App 就会用它自己那份空库 + 旧后端（界面报 HTTP 400）。
      touch(['db', 'schema.sqlite.sql']);
      touch(['backend', kServerExeName]);
      // 它**仍然**是个合法的"产物根"（真发布包就长这样），但**不是**源码树；
      // 判据细节见下面那组用例：往上找根时源码树优先。
      expect(looksLikeRoot(root.path), isTrue);
    });

    test('Release 产物判据 backend\\comfyhub-server-rs.exe', () {
      touch(['backend', kServerExeName]);
      expect(looksLikeRoot(root.path), isTrue);
    });

    test('空目录不算', () {
      expect(looksLikeRoot(root.path), isFalse);
      expect(looksLikeRoot(''), isFalse);
    });
  });

  group('源码树优先于产物根（2026-10-11 真 bug）', () {
    // 现场：`flutter build windows --debug` 会把后端装进
    // `build\windows\x64\runner\Debug\backend\`，于是那个产物目录命中了产物判据、
    // 自己成了"根" —— Debug 版 App 去用它的空库 + 旧后端 exe（界面报 HTTP 400），
    // 而实际库位于项目根目录。判据需"向上再查找一层源码树"。
    test('产物目录里有 backend\\，但上面有源码树 → 根取源码树', () {
      touch(['server-rs', 'Cargo.toml']); // 项目根
      final runner = Directory(
        p([root.path, 'build', 'windows', 'x64', 'runner', 'Debug']),
      )..createSync(recursive: true);
      File(p([runner.path, 'backend', kServerExeName]))
        ..createSync(recursive: true)
        ..writeAsStringSync('x');

      // 产物目录**单独看**确实像根（这条判据本身没变）
      expect(looksLikeRoot(runner.path), isTrue);
      // 但从它往上找时，**必须**拿到项目根，而不是它自己
      expect(
        detectRoot(from: runner.path),
        root.path,
        reason: '源码树在就轮不到 build\\ 里的产物目录当根',
      );
    });

    test('整条链上没有源码树 → 回退到产物根（真正的发布包）', () {
      final pack = Directory(p([root.path, 'dist', 'ComfyHub']))
        ..createSync(recursive: true);
      File(p([pack.path, 'backend', kServerExeName]))
        ..createSync(recursive: true)
        ..writeAsStringSync('x');
      expect(detectRoot(from: pack.path), pack.path, reason: '发布包里后端就在旁边，它就是根');
    });
  });

  group('detectRoot：从子目录往上找', () {
    test('命中带判据的上级', () {
      touch(['server-rs', 'Cargo.toml']);
      final deep = Directory(p([root.path, 'a', 'b', 'c']))
        ..createSync(recursive: true);
      expect(detectRoot(from: deep.path), root.path);
    });

    test('显式给的目录实在不像根，就原样回它（用户说了算）', () {
      expect(detectRoot(from: root.path), root.path);
    });
  });

  group('serverExeCandidates：顺序即优先级', () {
    test('COMFYHUB_SERVER_EXE 永远最前，其后是 backend\\ / server\\ / target\\release / target\\debug', () {
      final list = serverExeCandidates(
        root.path,
        env: {'COMFYHUB_SERVER_EXE': 'X:\\custom\\exe'},
      );

      // 顺序即优先级：覆盖 > App 自带 backend\ > 根下 backend\ > 旧发布包 server\ > 源码树
      expect(list.map((e) => e.key).toList(), [
        'COMFYHUB_SERVER_EXE',
        'App 自带 backend\\',
        'backend\\',
        'server\\（旧发布包布局）',
        'server-rs\\target\\release',
        'server-rs\\target\\debug',
      ]);
      expect(list[0].value, 'X:\\custom\\exe');
      // 相对 root 的候选都落在 root 下
      expect(list[2].value, p([root.path, 'backend', kServerExeName]));
      expect(
        list[4].value,
        p([root.path, 'server-rs', 'target', 'release', kServerExeName]),
      );
      expect(
        list[5].value,
        p([root.path, 'server-rs', 'target', 'debug', kServerExeName]),
      );
    });

    test('没设环境变量时第一个候选就是 backend\\', () {
      final list = serverExeCandidates(root.path, env: const {});
      expect(list.first.key, contains('backend'));
      expect(list.any((e) => e.key == 'COMFYHUB_SERVER_EXE'), isFalse);
    });
  });

  group('resolveLayout：', () {
    test('release 布局（backend\\）优先于源码树 target\\release', () {
      touch(['backend', kServerExeName]);
      touch(['server-rs', 'target', 'release', kServerExeName]);

      final layout = resolveLayout(projectRoot: root.path, env: const {});

      expect(layout, isNotNull);
      expect(layout!.root, root.path);
      expect(layout.exe, p([root.path, 'backend', kServerExeName]));
      expect(layout.exeSource, contains('backend'));
    });

    test('只有源码树产物时回落到 target\\release', () {
      touch(['server-rs', 'target', 'release', kServerExeName]);

      final layout = resolveLayout(projectRoot: root.path, env: const {});

      expect(
        layout!.exe,
        p([root.path, 'server-rs', 'target', 'release', kServerExeName]),
      );
    });

    test('COMFYHUB_SERVER_EXE 覆盖一切（把后端映像挪出受限目录的开关）', () {
      touch(['backend', kServerExeName]);
      final custom = File(p([root.path, 'elsewhere', kServerExeName]))
        ..parent.createSync(recursive: true)
        ..writeAsStringSync('x');

      final layout = resolveLayout(
        projectRoot: root.path,
        env: {'COMFYHUB_SERVER_EXE': custom.path},
      );

      expect(layout!.exe, custom.path);
      expect(layout.exeSource, 'COMFYHUB_SERVER_EXE');
    });

    test('环境变量指向不存在的路径时当没设，不崩', () {
      touch(['backend', kServerExeName]);

      final layout = resolveLayout(
        projectRoot: root.path,
        env: {
          'COMFYHUB_SERVER_EXE': p([root.path, 'nope.exe']),
        },
      );

      expect(layout!.exe, p([root.path, 'backend', kServerExeName]));
    });

    test('什么产物都没有时 exe 为 null，但根目录照常给出', () {
      touch(['server-rs', 'Cargo.toml']); // 只给根判据，不给 exe

      final layout = resolveLayout(projectRoot: root.path, env: const {});

      expect(layout!.root, root.path);
      expect(layout.exe, isNull);
      expect(layout.hasExe, isFalse);
    });
  });

  // 数据目录：库 + storage 可以移动到其他磁盘。
  // 判据是 `<根>\data-dir.json`；没有它就是运行根目录。
  group('数据目录覆盖：<根>\\data-dir.json', () {
    test('没设过 = 运行根目录（默认）', () {
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);
      expect(dataDbFileOf(root.path), p([root.path, '.db', 'comfyhub.db']));
      expect(dataStorageDirOf(root.path), p([root.path, 'storage']));
    });

    test('写了覆盖值就按它走，清了就回默认', () {
      final target = p([root.path, 'other-disk', 'ComfyHubData']);
      // 目录得先在（2026-11-10 起：指针指的目录不在了会**回落到运行根目录**并说出来，
      // 免得照着不存在的路径建一个空库、界面看起来像"数据全没了"）
      Directory(p([target, 'storage', 'media'])).createSync(recursive: true);
      expect(writeDataDirOverride(root.path, target), isTrue);
      expect(readDataDirOverride(root.path), target);
      expect(dataDirOf(root.path), target);
      expect(dataDbFileOf(target), p([target, '.db', 'comfyhub.db']));
      expect(dataStorageDirOf(target), p([target, 'storage']));

      expect(writeDataDirOverride(root.path, null), isTrue);
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);
    });

    test('半截 JSON / 坏值当没设过，绝不抛', () {
      File(p([root.path, kDataDirFileName]))
          .writeAsStringSync('{"dataDir": "D:\\x"');
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);

      File(p([root.path, kDataDirFileName]))
          .writeAsStringSync('{"dataDir": ""}');
      expect(readDataDirOverride(root.path), isNull);
    });
  });

  // 产物目录的实际位置（2026-11-09）：
  //   · 标准 = `<数据目录>\storage`；
  //   · 扁平 = 数据目录本身（里层 storage 内容被移动到上一级）；
  //   · 双层 = `<数据目录>\storage\storage`（旧版把 storage 当成数据目录）。
  // 判据只能是"哪个目录里真的有内容"，空壳不算。
  group('resolvedStorageDirOf：三种布局都认', () {
    void writeFile(String base, List<String> parts) {
      final file = File(p([base, ...parts]));
      file.parent.createSync(recursive: true);
      file.writeAsStringSync('x');
    }

    test('空目录 → 标准路径（由调用方去建）', () {
      final data = p([root.path, 'data']);
      Directory(data).createSync();
      expect(resolvedStorageDirOf(data), dataStorageDirOf(data));
      expect(isFlatStorageLayout(data), isFalse);
      expect(looksLikeStorageDir(data), isFalse);
    });

    test('扁平：产物直接在数据目录里（本例就是用户手挪后的样子）', () {
      final data = p([root.path, 'data']);
      writeFile(data, ['media', 'a.png']);
      expect(resolvedStorageDirOf(data), data);
      expect(isFlatStorageLayout(data), isTrue);
      expect(looksLikeStorageDir(data), isTrue);
    });

    test('标准布局有内容时优先（哪怕旁边还躺着扁平的那一套）', () {
      final data = p([root.path, 'data']);
      writeFile(data, ['media', 'old.png']);
      writeFile(data, ['storage', 'media', 'a.png']);
      expect(resolvedStorageDirOf(data), dataStorageDirOf(data));
      expect(isFlatStorageLayout(data), isFalse);
    });

    test('空的标准目录 + 扁平内容：认扁平，不被空壳挡住', () {
      final data = p([root.path, 'data']);
      Directory(p([data, 'storage', 'media'])).createSync(recursive: true);
      writeFile(data, ['media', 'a.png']);
      expect(resolvedStorageDirOf(data), data);
      expect(isFlatStorageLayout(data), isTrue);
    });

    test('双层：旧版套出来的 storage\\storage 也认', () {
      final data = p([root.path, 'data']);
      writeFile(data, ['storage', 'storage', 'media', 'a.png']);
      expect(
        resolvedStorageDirOf(data),
        p([dataStorageDirOf(data), 'storage']),
      );
      expect(isFlatStorageLayout(data), isFalse);
    });

    test('只有 tmp / 空目录不算内容', () {
      final data = p([root.path, 'data']);
      writeFile(data, ['storage', 'tmp', 'junk.bin']);
      Directory(p([data, 'media'])).createSync(recursive: true);
      expect(resolvedStorageDirOf(data), dataStorageDirOf(data));
    });
  });

  group('readPortFile：启动与退出的关键路径，绝不能抛', () {
    test('没有文件回 null', () {
      expect(readPortFile(root.path), isNull);
    });

    test('半截 JSON / 非 JSON 回 null', () {
      final runDir = Directory(p([root.path, '.run']))
        ..createSync(recursive: true);
      File(p([runDir.path, 'api.json'])).writeAsStringSync('{"port": 12');
      expect(readPortFile(root.path), isNull);
      File(p([runDir.path, 'api.json'])).writeAsStringSync('不是 json');
      expect(readPortFile(root.path), isNull);
    });

    test('正常文件解析出 port / pid', () {
      final runDir = Directory(p([root.path, '.run']))
        ..createSync(recursive: true);
      File(p([runDir.path, 'api.json'])).writeAsStringSync(
        '{"port":6023,"pid":8884,"host":"0.0.0.0","url":"http://0.0.0.0:6023",'
        '"requestedPort":0,"fellBack":false}',
      );

      final state = readPortFile(root.path);
      expect(state, isNotNull);
      expect(state!['port'], 6023);
      expect(state['pid'], 8884);
    });
  });

  group('数据目录：靠"库的身份"认，不靠路径猜（2026-11-10）', () {
    // 现场：`data-dir.json` 写的是 `D:\dist\ComfyHub`，真正的库在
    // `D:\dist\ComfyHub\storage\.db\comfyhub.db`（57 个会话）；指针那一级也有一份库，
    // 但是上一份构建残留（3 个会话、2 张角色卡）。照着指针开 → "对话不见了、角色卡是
    // 旧版"，而磁盘上一个字节都没丢。
    //
    // 修法不是"猜哪份更大"，而是让**库自己带身份**（`library_identity.dart`）：
    //   · `<数据目录>\.db\library.json` 跟着库走（复制 / 搬家 / 迁移都带着它）；
    //   · 偏好 `comfyhub.library.last` 记住"上次用的是哪一份、在哪"。
    // 于是"这次打开的是不是你的库"变成能问出来的事实。大小只在**没有记录**的老库上
    // 用一次（过渡），认完就写身份 + 记偏好，从此不再靠猜。

    /// 造一份库（按字节数造大小，内容无所谓）。
    void writeDb(String base, {required int bytes}) {
      final file = File(p([base, '.db', 'comfyhub.db']));
      file.parent.createSync(recursive: true);
      file.writeAsBytesSync(List<int>.filled(bytes, 0));
    }

    /// 造一份**有身份**的库，返回它的 id。
    String writeIdentifiedDb(String base, {required int bytes}) {
      writeDb(base, bytes: bytes);
      final id = ensureLibraryId(base);
      expect(id, isNotEmpty, reason: '身份文件必须写得进去（否则用例的前提不成立）');
      return id;
    }

    void pointAt(String dir) {
      expect(writeDataDirOverride(root.path, dir), isTrue);
    }

    test('没指针、没记录 → 运行根目录，什么都不提示', () {
      final info = resolveDataDir(root.path);
      expect(info.dataDir, root.path);
      expect(info.pointer, isNull);
      expect(info.pointerMissing, isFalse);
      expect(info.pick, DataDirPick.chosen);
      expect(info.notice, isNull);
    });

    test('指针指的目录不在了 → 回运行根目录，并且说出来（绝不照着空路径建库）', () {
      pointAt(p([root.path, '拔掉的移动盘']));
      final info = resolveDataDir(root.path);
      expect(info.dataDir, root.path);
      expect(info.pointerMissing, isTrue);
      expect(info.notice, contains('已不存在'));
      // 一句提示，不提"移动盘"这类未定义的概念
      expect(info.notice, contains('这次使用的是'));
    });

    test('没有记录时不猜：用指针这一级，把别的库列出来让用户选', () {
      final hub = p([root.path, 'hub']);
      writeDb(hub, bytes: 1024); // 上一级：上一份构建的残留
      writeDb(p([hub, 'storage']), bytes: 64 * 1024); // storage 里：另一份库
      final mediaDir = Directory(p([hub, 'storage', 'media']))
        ..createSync(recursive: true);
      File(p([mediaDir.path, 'a.png'])).writeAsStringSync('x');
      pointAt(hub);

      final info = resolveDataDir(root.path);
      // ⚠ 这里**不许**按大小自动换（2026-11-10 复查）：用指针那一级，
      // 另一份只在 `otherCandidates` 中报告，是否切换由界面决定。
      expect(info.pick, DataDirPick.chosen);
      expect(info.dataDir, hub);
      expect(
        info.otherCandidates.map((c) => c.path),
        contains(p([hub, 'storage'])),
      );
      // 这不属于故障：不应显示横幅（否则每次启动都会出现提示条）。
      expect(info.notice, isNull);
    });

    test('有记录：复制出去的构建里没有库 → 按记录打开上次那份（这就是一劳永逸）', () {
      final lib = p([root.path, 'lib']);
      final id = writeIdentifiedDb(lib, bytes: 4096);
      // 程序被复制到别处：那个目录里既没有库，也没有指针
      final copy = Directory(p([root.path, 'copy']))..createSync();

      final info = resolveDataDir(
        copy.path,
        knownLibraryId: id,
        knownLibraryPath: lib,
      );
      expect(info.dataDir, lib);
      expect(info.pick, DataDirPick.remembered);
      expect(info.matchesRemembered, isTrue);
      expect(info.usedLibraryId, id);
      expect(info.notice, contains('上次使用的那份'));
    });

    test('有库有身份、但还没有记录 → 信它，不按大小乱换', () {
      final small = p([root.path, 'a']);
      final id = writeIdentifiedDb(small, bytes: 1024);
      Directory(p([small, 'storage'])).createSync(); // 产物目录在（不是"只复制了程序"）
      writeDb(p([root.path, 'hub', 'storage']), bytes: 64 * 1024); // 旁边一份更大的
      pointAt(small);

      final info = resolveDataDir(root.path);
      expect(info.dataDir, small);
      expect(info.pick, DataDirPick.chosen);
      expect(info.usedLibraryId, id);
      expect(info.notice, isNull);
    });

    test('有记录：指针里是另一份有身份的库 → 尊重指针，但要说出来', () {
      final a = p([root.path, 'a']);
      writeIdentifiedDb(a, bytes: 2048);
      final b = p([root.path, 'b']);
      final bId = writeIdentifiedDb(b, bytes: 4096);
      pointAt(a);

      final info = resolveDataDir(
        root.path,
        knownLibraryId: bId,
        knownLibraryPath: b,
      );
      expect(info.dataDir, a, reason: '用户显式选过的位置不能被静默覆盖');
      expect(info.pick, DataDirPick.chosen);
      expect(info.pointerHasOtherLibrary, isTrue);
      expect(info.matchesRemembered, isFalse);
      expect(info.notice, contains('不是上次使用的那一份'));
    });

    test('有记录但哪儿都找不到 → 用指针那份，并说出来', () {
      final data = p([root.path, 'only']);
      writeDb(data, bytes: 2048);
      pointAt(data);

      final info = resolveDataDir(
        root.path,
        knownLibraryId: 'gone-id',
        knownLibraryPath: p([root.path, 'gone']),
      );
      expect(info.dataDir, data);
      expect(info.rememberedMissing, isTrue);
      expect(info.matchesRemembered, isFalse);
      expect(info.notice, contains('已找不到'));
    });

    test('库在、产物目录整个不在 → 说出来（只复制了程序没复制 storage）', () {
      final data = p([root.path, 'only-db']);
      writeDb(data, bytes: 1024 * 1024);
      pointAt(data);

      final info = resolveDataDir(root.path);
      expect(info.storageMissing, isTrue);
      expect(info.notice, contains('图片'));
    });

    test('空库目录不算数据（刚装好不该报噪音）', () {
      final data = p([root.path, 'fresh']);
      Directory(p([data, 'storage'])).createSync(recursive: true);
      pointAt(data);
      final info = resolveDataDir(root.path);
      expect(info.storageMissing, isFalse);
      expect(info.notice, isNull);
    });

    test('身份文件坏了 → 当"这次没有身份"，不崩也不假装同一个库', () {
      final data = p([root.path, 'broken']);
      writeDb(data, bytes: 1024);
      File(p([data, '.db', kLibraryIdFileName])).writeAsStringSync('{"id": ');
      expect(readLibraryId(data), isNull);
      // 有记录但那份库认不出来 → 不能声称"就是上次那份"
      final info = resolveDataDir(
        root.path,
        knownLibraryId: 'some-id',
        knownLibraryPath: data,
      );
      expect(info.usedLibraryId, isNull);
      expect(info.matchesRemembered, isFalse);
    });
  });

  group('库的身份：跟着库走，读写都宽容（library_identity.dart）', () {
    test('同一个库读两次是同一个 id（幂等）', () {
      final data = p([root.path, 'lib']);
      final first = ensureLibraryId(data);
      expect(first, isNotEmpty);
      expect(ensureLibraryId(data), first);
      expect(readLibraryId(data), first);
    });

    test('没有库就拍不出快照（不会给空目录发身份）', () {
      final data = p([root.path, 'empty']);
      Directory(data).createSync(recursive: true);
      expect(snapshotLibrary(data), isNull);
      expect(
        File(p([data, '.db', kLibraryIdFileName])).existsSync(),
        isFalse,
        reason: '只在真的用到这份库时才写身份',
      );
    });

    test('快照带位置、大小与时间', () {
      final data = p([root.path, 'lib']);
      final db = File(p([data, '.db', 'comfyhub.db']))
        ..createSync(recursive: true)
        ..writeAsBytesSync(List<int>.filled(2048, 0));
      final ref = snapshotLibrary(data);
      expect(ref, isNotNull);
      expect(ref!.path, data);
      expect(ref.sizeBytes, db.lengthSync());
      expect(ref.usable, isTrue);
      expect(ref.seenAt.year, greaterThanOrEqualTo(2026));
    });

    test('偏好里写的引用能被宽容解析（缺字段 / 脏值回 null）', () {
      expect(LibraryRef.fromJson(null), isNull);
      expect(LibraryRef.fromJson(<String, Object?>{}), isNull);
      expect(LibraryRef.fromJson(<String, Object?>{'id': 'x'}), isNull);
      final ref = LibraryRef.fromJson(<String, Object?>{
        'id': 'x',
        'path': r'D:\data',
        'seenAt': '2026-11-10T08:00:00.000',
        'sizeBytes': 42,
      });
      expect(ref, isNotNull);
      expect(ref!.id, 'x');
      expect(ref.sizeBytes, 42);
      expect(
        LibraryRef.fromJson(<String, Object?>{'id': 'x', 'path': 'p'})!
            .seenAt
            .year,
        1970,
      );
    });
  });
}
