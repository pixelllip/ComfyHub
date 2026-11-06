// 「根目录在哪、后端 exe 在哪」（`lib\core\backend_paths.dart`）的回归测试。
//
// 这是**全 App 唯一的一份布局解析**（源码树 / Release 两种布局、
// `COMFYHUB_SERVER_EXE` 覆盖），`scripts\backend.ps1` 是它在脚本世界的镜像 ——
// 判据漂移的表现是"App 起了 A、脚本说在 B"，所以这里把顺序钉死。

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/backend_paths.dart';

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
    // 而用户的真库一直在项目根下。判据要"往上再找一层源码树"。
    test('产物目录里有 backend\\，但上面有源码树 → 根取源码树', () {
      touch(['server-rs', 'Cargo.toml']); // 项目根
      final runner = Directory(p([root.path, 'build', 'windows', 'x64', 'runner', 'Debug']))
        ..createSync(recursive: true);
      File(p([runner.path, 'backend', kServerExeName]))
        ..createSync(recursive: true)
        ..writeAsStringSync('x');

      // 产物目录**单独看**确实像根（这条判据本身没变）
      expect(looksLikeRoot(runner.path), isTrue);
      // 但从它往上找时，**必须**拿到项目根，而不是它自己
      expect(detectRoot(from: runner.path), root.path,
          reason: '源码树在就轮不到 build\\ 里的产物目录当根');
    });

    test('整条链上没有源码树 → 回退到产物根（真正的发布包）', () {
      final pack = Directory(p([root.path, 'dist', 'ComfyHub']))..createSync(recursive: true);
      File(p([pack.path, 'backend', kServerExeName]))
        ..createSync(recursive: true)
        ..writeAsStringSync('x');
      expect(detectRoot(from: pack.path), pack.path,
          reason: '发布包里后端就在旁边，它就是根');
    });
  });

  group('detectRoot：从子目录往上找', () {
    test('命中带判据的上级', () {
      touch(['server-rs', 'Cargo.toml']);
      final deep = Directory(p([root.path, 'a', 'b', 'c']))..createSync(recursive: true);
      expect(detectRoot(from: deep.path), root.path);
    });

    test('显式给的目录实在不像根，就原样回它（用户说了算）', () {
      expect(detectRoot(from: root.path), root.path);
    });
  });

  group('serverExeCandidates：顺序即优先级', () {
    test('COMFYHUB_SERVER_EXE 永远最前，其后是 backend\\ / server\\ / target\\release / target\\debug',
        () {
      final list = serverExeCandidates(root.path, env: {'COMFYHUB_SERVER_EXE': 'X:\\custom\\exe'});

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
      expect(list[4].value, p([root.path, 'server-rs', 'target', 'release', kServerExeName]));
      expect(list[5].value, p([root.path, 'server-rs', 'target', 'debug', kServerExeName]));
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

      expect(layout!.exe, p([root.path, 'server-rs', 'target', 'release', kServerExeName]));
    });

    test('COMFYHUB_SERVER_EXE 覆盖一切（把后端映像挪出受限目录的开关）', () {
      touch(['backend', kServerExeName]);
      final custom = File(p([root.path, 'elsewhere', kServerExeName]))
        ..parent.createSync(recursive: true)
        ..writeAsStringSync('x');

      final layout = resolveLayout(projectRoot: root.path,
          env: {'COMFYHUB_SERVER_EXE': custom.path});

      expect(layout!.exe, custom.path);
      expect(layout.exeSource, 'COMFYHUB_SERVER_EXE');
    });

    test('环境变量指向不存在的路径时当没设，不崩', () {
      touch(['backend', kServerExeName]);

      final layout = resolveLayout(projectRoot: root.path,
          env: {'COMFYHUB_SERVER_EXE': p([root.path, 'nope.exe'])});

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

  // 数据目录（2026-11-06）：库 + storage 可以被用户搬到别的盘。
  // 判据是 `<根>\data-dir.json`；没有它就是运行根目录。
  group('数据目录覆盖：<根>\\data-dir.json', () {
    test('没设过 = 运行根目录（默认）', () {
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);
      expect(dataDbFileOf(root.path),
          p([root.path, '.db', 'comfyhub.db']));
      expect(dataStorageDirOf(root.path), p([root.path, 'storage']));
    });

    test('写了覆盖值就按它走，清了就回默认', () {
      final target = p([root.path, 'other-disk', 'ComfyHubData']);
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
      File(p([root.path, kDataDirFileName])).writeAsStringSync('{"dataDir": "D:\\x"');
      expect(readDataDirOverride(root.path), isNull);
      expect(dataDirOf(root.path), root.path);

      File(p([root.path, kDataDirFileName])).writeAsStringSync('{"dataDir": ""}');
      expect(readDataDirOverride(root.path), isNull);
    });
  });

  group('readPortFile：启动与退出的关键路径，绝不能抛', () {    test('没有文件回 null', () {
      expect(readPortFile(root.path), isNull);
    });

    test('半截 JSON / 非 JSON 回 null', () {
      final runDir = Directory(p([root.path, '.run']))..createSync(recursive: true);
      File(p([runDir.path, 'api.json'])).writeAsStringSync('{"port": 12');
      expect(readPortFile(root.path), isNull);
      File(p([runDir.path, 'api.json'])).writeAsStringSync('不是 json');
      expect(readPortFile(root.path), isNull);
    });

    test('正常文件解析出 port / pid', () {
      final runDir = Directory(p([root.path, '.run']))..createSync(recursive: true);
      File(p([runDir.path, 'api.json'])).writeAsStringSync(
          '{"port":6023,"pid":8884,"host":"0.0.0.0","url":"http://0.0.0.0:6023",'
          '"requestedPort":0,"fellBack":false}');

      final state = readPortFile(root.path);
      expect(state, isNotNull);
      expect(state!['port'], 6023);
      expect(state['pid'], 8884);
    });
  });
}
