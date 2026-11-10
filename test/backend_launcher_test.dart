// 后端直启（`lib\core\backend_launcher.dart`）的参数与收尾回归测试。
//
// 2026-09-30 起 App **不再经过 pwsh**：`ensureRunning` 直接执行
// `<运行根>\backend\…` 或 `server-rs\target\…\comfyhub-server-rs.exe`，
// 关窗时 kill 自己认领过的那个进程。这里**不真的起进程**：
// `BackendLauncher.processStarter` / `toolRunnerOverride` / `killPidOverride`
// 是留给测试的缝隙，换掉之后就能断言"到底起的是哪个 exe、传了哪些参数、
// 杀了哪个 pid"，在没有任何后端二进制的机器上也能跑。
//
// 背景（修的是哪类 bug）：
//  · 以前走 `pwsh → comfyhub.ps1 → server.ps1 → WMI → cmd → exe` 五层，
//    缺 pwsh / WMI 被拒 / 脚本卡住，任何一环失败 App 都只能干等 90 秒；
//  · 退出收尾以前靠 `watch-owner.ps1` 守护进程盯 App 的 pid，现在后端自己
//    盯 stdin 管道（`--watch-stdin`），App 这边只需记下"谁是我的人"。

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/backend_paths.dart';
import 'package:viewer/core/library_identity.dart';
import 'package:viewer/core/settings_store.dart';

/// 一次子进程调用（假 starter 记下来的）
class _Call {
  final String exe;
  final List<String> args;
  final String? workDir;
  final Map<String, String>? environment;

  _Call(this.exe, this.args, this.workDir, [this.environment]);
}

/// 假的 Process：launcher 会订阅 stdout/stderr、挂 exitCode、在收尾时 kill
class _FakeProcess implements Process {
  _FakeProcess({int pid = 4242, Completer<int>? exit})
    : fakePid = pid,
      _exit = exit ?? (Completer<int>()..complete(0));

  final int fakePid;
  final Completer<int> _exit;
  int killCalls = 0;

  @override
  int get pid => fakePid;

  @override
  Stream<List<int>> get stdout => const Stream<List<int>>.empty();

  @override
  Stream<List<int>> get stderr => const Stream<List<int>>.empty();

  @override
  IOSink get stdin => throw UnimplementedError();

  @override
  Future<int> get exitCode => _exit.future;

  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) {
    killCalls++;
    return true;
  }
}

void main() {
  late Directory root;
  late List<_Call> spawns;
  late List<_Call> tools;
  late List<int> killedPids;
  late BackendProcessStarter realStarter;

  /// 源码树布局里后端 exe 的位置（setUp 造出来）
  String devExe() => [
    root.path,
    'server-rs',
    'target',
    'release',
    kServerExeName,
  ].join(Platform.pathSeparator);

  setUp(() {
    root = Directory.systemTemp.createTempSync('comfyhub-launcher-');
    // 源码树布局的判据（backend_paths.dart 的 looksLikeRoot）+ 一份假 exe
    Directory('${root.path}${Platform.pathSeparator}server-rs').createSync();
    File(
      '${root.path}${Platform.pathSeparator}server-rs'
      '${Platform.pathSeparator}Cargo.toml',
    ).writeAsStringSync('[package]\nname = "x"\n');
    Directory(
      '${root.path}${Platform.pathSeparator}server-rs'
      '${Platform.pathSeparator}target${Platform.pathSeparator}release',
    ).createSync(recursive: true);
    File(devExe()).writeAsStringSync('fake exe');

    spawns = [];
    tools = [];
    killedPids = [];
    realStarter = BackendLauncher.processStarter;
    BackendLauncher.toolRunnerOverride = (exe, args) async {
      tools.add(_Call(exe, List<String>.of(args), null));
      return ProcessResult(0, 0, 'processed file: x', '');
    };
    BackendLauncher.killPidOverride = (pid) async {
      killedPids.add(pid);
      return true;
    };
    BackendLauncher.waitDeadlineOverride = const Duration(seconds: 1);
    // 把"去哪找 .run\api.json"钉在临时根上：否则 load/refresh 会读到**真项目**
    // 的端口文件，用例结果随"这台机器的后端正跑在哪个端口"漂移
    // （settings_store.dart 里那条注释就是这个坑的现场记录）。
    SettingsStore.portFileRootsOverride = [root.path];
  });

  tearDown(() {
    BackendLauncher.processStarter = realStarter;
    BackendLauncher.toolRunnerOverride = null;
    BackendLauncher.killPidOverride = null;
    BackendLauncher.waitDeadlineOverride = null;
    SettingsStore.portFileRootsOverride = null;
    try {
      root.deleteSync(recursive: true);
    } catch (_) {}
  });

  /// 装一个假的进程启动器
  void installStarter({
    bool throwError = false,
    Completer<int>? exit,
    int pid = 555000,
    void Function()? onSpawn,
  }) {
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(
            _Call(exe, List<String>.of(args), workingDirectory, environment),
          );
          onSpawn?.call();
          if (throwError) throw const ProcessException('exe', [], 'boom');
          return _FakeProcess(pid: pid, exit: exit);
        };
  }

  Future<SettingsStore> loadSettings({
    String? projectRoot,
    bool stopServicesOnExit = true,
    bool allowRemote = false,
    int? lastApiPort,
  }) async {
    final values = <String, Object>{
      'comfyhub.projectRoot': projectRoot ?? root.path,
      'comfyhub.stopServicesOnExit': stopServicesOnExit,
      'comfyhub.autoStartBackend': false,
      if (allowRemote) 'comfyhub.allowRemoteBackend': true,
    };
    if (lastApiPort != null) values['comfyhub.lastApiPort'] = lastApiPort;
    SharedPreferences.setMockInitialValues(values);
    final settings = SettingsStore();
    await settings.load();
    return settings;
  }

  /// 写一份 `.run\api.json`（后端绑定成功后落盘的那份端口记录）
  ///
  /// `requestedPort` / `fellBack` 是后端自己写的两个键：前者是"我们想让它听的端口"，
  /// 后者是"那个端口没绑上、它回落到随机端口了"。它们正是"端口粘住"这件事的
  /// 全部事实来源（我们**不猜**）。
  void writeApiJson(
    int port, {
    int pid = 987654,
    int requestedPort = 0,
    bool fellBack = false,
  }) {
    final runDir = Directory('${root.path}${Platform.pathSeparator}.run');
    runDir.createSync(recursive: true);
    File('${runDir.path}${Platform.pathSeparator}api.json').writeAsStringSync(
      '{"port":$port,"pid":$pid,"host":"127.0.0.1",'
      '"url":"http://127.0.0.1:$port","requestedPort":$requestedPort,'
      '"fellBack":$fellBack}',
    );
  }

  /// 起一个本地 HTTP 服务当"后端"（health 可控）
  Future<HttpServer> startFakeBackend({String database = 'ok'}) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((req) {
      req.response
        ..statusCode = 200
        ..headers.contentType = ContentType.json
        ..write(jsonEncode({'version': 'test', 'database': database}));
      req.response.close();
    });
    return server;
  }

  /// 死端口：先占住再放掉 = 一个保证没人听的端口
  Future<int> deadPort() async {
    final s = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final p = s.port;
    await s.close();
    return p;
  }

  // -------------------------------------------------------------------------
  //  启动参数
  // -------------------------------------------------------------------------

  test(
    '启动：直启 exe，参数是 --port 0 / --watch-stdin / COMFYHUB_PROJECT_ROOT，起之前先归一标签',
    () async {
      writeApiJson(await deadPort()); // App 启动时读到的"旧门牌"（没人听）
      final settings = await loadSettings();
      final launcher = BackendLauncher(settings);
      installStarter();

      await launcher.ensureRunning();

      // icacls（标签归一）在起进程**之前**跑
      expect(tools, hasLength(1));
      expect(tools.single.exe, 'icacls');
      expect(tools.single.args, [devExe(), '/setintegritylevel', 'Medium']);

      expect(spawns, hasLength(1));
      expect(spawns.single.exe, devExe());
      expect(spawns.single.workDir, root.path);
      expect(spawns.single.args, [
        '--db',
        '${root.path}${Platform.pathSeparator}.db'
            '${Platform.pathSeparator}comfyhub.db',
        '--storage', '${root.path}${Platform.pathSeparator}storage',
        '--host', '127.0.0.1',
        '--port', '0',
        // 强杀 App 时不留孤儿的唯一保障（后端盯着这根管道）
        '--watch-stdin',
      ]);
      // 端口文件与工具权限的基准必须显式给（发布包里 backend\ 与根差一层，纯推断不可靠）
      expect(
        spawns.single.environment,
        containsPair('COMFYHUB_PROJECT_ROOT', root.path),
      );
      // 没开"允许局域网"时不带这个变量（后端只听回环）
      expect(
        spawns.single.environment ?? const {},
        isNot(contains('COMFYHUB_ALLOW_REMOTE')),
      );
      // 起了 = 认领了：关窗时才有资格停它
      expect(launcher.servicesClaimed, isTrue);
      launcher.dispose();
    },
  );

  test('启动：数据目录被指到别处时 --db / --storage 跟着走，根目录基准不变', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final data = Directory(
      '${root.path}${Platform.pathSeparator}elsewhere-data',
    )..createSync(recursive: true);
    expect(writeDataDirOverride(root.path, data.path), isTrue);
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    final sep = Platform.pathSeparator;
    expect(spawns, hasLength(1));
    expect(spawns.single.args, [
      '--db',
      '${data.path}$sep.db${sep}comfyhub.db',
      '--storage',
      '${data.path}${sep}storage',
      '--host',
      '127.0.0.1',
      '--port',
      '0',
      '--watch-stdin',
    ]);
    // 端口文件 / 备份 / 工具白名单的基准仍是运行根目录（数据搬了，程序没搬）
    expect(spawns.single.workDir, root.path);
    expect(
      spawns.single.environment,
      containsPair('COMFYHUB_PROJECT_ROOT', root.path),
    );
    expect(launcher.dataDir, data.path);
    launcher.dispose();
  });

  test('启动：数据目录本身就是产物目录时 --storage 指它，不再硬拼 storage', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final data = Directory('${root.path}${Platform.pathSeparator}flat-data')
      ..createSync(recursive: true);
    // 扁平布局：产物直接位于数据目录（手动移动后的形态）
    final media = Directory('${data.path}${Platform.pathSeparator}media')
      ..createSync();
    File('${media.path}${Platform.pathSeparator}a.png').writeAsStringSync('x');
    expect(writeDataDirOverride(root.path, data.path), isTrue);
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    final sep = Platform.pathSeparator;
    expect(spawns.single.args, [
      '--db',
      '${data.path}$sep.db${sep}comfyhub.db',
      '--storage',
      data.path,
      '--host',
      '127.0.0.1',
      '--port',
      '0',
      '--watch-stdin',
    ]);
    expect(
      Directory('${data.path}${sep}storage').existsSync(),
      isFalse,
      reason: '认了扁平布局就不该再建一个空的 storage 子目录',
    );
    launcher.dispose();
  });

  test('启动：指针停在 storage 的上一级、记录里是 storage 那份 → 按记录用它', () async {
    // 2026-11-10 真 bug：`data-dir.json` 写 `…\hub`，库却在 `…\hub\storage\.db`,
    // 而 `…\hub\.db` 是上一份构建的残留。现在靠**身份记录**决定用哪份
    // （不按大小猜）：记着 storage 那份，就打开 storage 那份。
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final sep = Platform.pathSeparator;
    final hub = Directory('${root.path}${sep}hub')..createSync(recursive: true);
    void writeDb(Directory base, int bytes) {
      final file = File('${base.path}$sep.db${sep}comfyhub.db');
      file.parent.createSync(recursive: true);
      file.writeAsBytesSync(List<int>.filled(bytes, 0));
    }

    writeDb(hub, 1024); // 上一级：残留的旧库
    final real = Directory('${hub.path}${sep}storage')..createSync();
    writeDb(real, 64 * 1024); // storage 里：正在用的那份
    // 真库旁边就是产物（扁平布局）：现场 `D:\dist\ComfyHub\storage\media` 正是这样
    final mediaDir = Directory('${real.path}${sep}media')..createSync();
    File('${mediaDir.path}${sep}a.png').writeAsStringSync('x');
    final id = ensureLibraryId(real.path);
    expect(id, isNotEmpty);
    await settings.setLastLibrary(
      LibraryRef(id: id, path: real.path, seenAt: DateTime.now()),
    );
    expect(writeDataDirOverride(root.path, hub.path), isTrue);

    final launcher = BackendLauncher(settings);
    installStarter();
    await launcher.ensureRunning();

    expect(spawns.single.args, [
      '--db',
      '${real.path}$sep.db${sep}comfyhub.db',
      '--storage',
      real.path,
      '--host',
      '127.0.0.1',
      '--port',
      '0',
      '--watch-stdin',
    ]);
    // 界面要能说清"为什么换了一份"
    expect(launcher.dataDir, real.path);
    expect(launcher.dataDirInfo?.pick, DataDirPick.remembered);
    expect(launcher.dataDirNotice, contains('上次使用的那份'));
    // `.run` 与工作目录的基准仍然是运行根目录（数据在哪不影响端口文件在哪）
    expect(spawns.single.workDir, root.path);
    launcher.dispose();
  });

  test('启动：程序被复制到别处时，按"上次用的库"的记录打开原来那份', () async {
    // 将构建复制到其他位置打开后，使用的是另一份旧库。
    // 现在每个库都带身份，偏好里记着"上次用的是哪一份、在哪" —— 复制出去的构建
    // 照样打开你的库（并且这件事在界面能看见）。
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final sep = Platform.pathSeparator;
    final lib = Directory('${root.path}${sep}library')
      ..createSync(recursive: true);
    final dbFile = File('${lib.path}$sep.db${sep}comfyhub.db');
    dbFile.parent.createSync(recursive: true);
    dbFile.writeAsBytesSync(List<int>.filled(4096, 0));
    Directory('${lib.path}${sep}storage').createSync();
    final id = ensureLibraryId(lib.path);
    expect(id, isNotEmpty);
    await settings.setLastLibrary(
      LibraryRef(id: id, path: lib.path, seenAt: DateTime.now()),
    );

    // 这次从"另一个目录"启动（那里没有库、也没有指针）
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    expect(spawns.single.args, [
      '--db',
      '${lib.path}$sep.db${sep}comfyhub.db',
      '--storage',
      '${lib.path}${sep}storage',
      '--host',
      '127.0.0.1',
      '--port',
      '0',
      '--watch-stdin',
    ]);
    expect(launcher.dataDir, lib.path);
    expect(launcher.dataDirInfo?.pick, DataDirPick.remembered);
    expect(launcher.dataDirNotice, contains('上次使用的那份'));
    launcher.dispose();
  });

  test('启动：开了「允许局域网」就 --host 0.0.0.0 + COMFYHUB_ALLOW_REMOTE=1', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings(allowRemote: true);
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    expect(spawns.single.args, contains('0.0.0.0'));
    expect(
      spawns.single.environment,
      containsPair('COMFYHUB_ALLOW_REMOTE', '1'),
    );
    launcher.dispose();
  });

  test('启动：后端进程提前退出时当场报失败，不等满超时', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter(exit: Completer<int>()..complete(3));

    final sw = Stopwatch()..start();
    final ok = await launcher.ensureRunning();
    sw.stop();

    expect(ok, isFalse);
    expect(sw.elapsedMilliseconds, lessThan(3000), reason: '进程都退了还白等就是退步');
    expect(launcher.phase, StartupPhase.failed);
    expect(launcher.lastError, contains('退出码 3'));
    launcher.dispose();
  });

  test('启动：找不到后端二进制时给修法，而不是猜一个路径去起', () async {
    File(devExe()).deleteSync();
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();

    final ok = await launcher.ensureRunning();

    expect(ok, isFalse);
    expect(spawns, isEmpty);
    expect(launcher.lastError, contains('cargo build --release'));
    expect(launcher.lastError, contains('COMFYHUB_SERVER_EXE'));
    launcher.dispose();
  });

  test('启动：端口文件被后端改写后，等待中会重新发现新端口', () async {
    final dead = await deadPort();
    writeApiJson(dead); // 旧门牌
    // 假后端：health 一直 ok
    final server = await startFakeBackend();
    addTearDown(() => server.close(force: true));
    final settings = await loadSettings();
    // 此时 settings.baseUrl 还是死端口（load 时读的旧 api.json）
    final launcher = BackendLauncher(settings);
    // 假的"后端"：起出去之后把 api.json 改写成**新端口**（真后端绑上后自己会写）
    installStarter(onSpawn: () => writeApiJson(server.port, pid: 555000));

    expect(await launcher.ensureRunning(), isTrue);

    // 关键断言：地址跟着 api.json 走到了新端口，而不是对着旧端口等满超时
    expect(settings.baseUrl, 'http://127.0.0.1:${server.port}');
    expect(launcher.logLines.any((l) => l.contains('后端换了端口')), isTrue);
    expect(launcher.phase, StartupPhase.ready);
    launcher.dispose();
  });

  test('启动：后端在跑但数据库不 ok 时，先停掉端口记录里那个 pid 再重起', () async {
    final server = await startFakeBackend(database: 'broken');
    addTearDown(() => server.close(force: true));
    writeApiJson(server.port, pid: 987654);
    final settings = await loadSettings();
    await settings.setBaseUrl('http://127.0.0.1:${server.port}');
    final launcher = BackendLauncher(settings);
    installStarter();

    final ok = await launcher.ensureRunning();

    // 真正停掉的是端口文件里记的那个 pid，而不是碰系统里别的进程
    expect(killedPids, contains(987654));
    expect(spawns, hasLength(1), reason: '旧的送走了，新的要起来');
    expect(ok, isFalse, reason: '假后端永远回 broken，重起后仍然不健康是如实结果');
    launcher.dispose();
  });

  test('启动：后端本来就健康时直接复用，不起新进程', () async {
    final server = await startFakeBackend();
    addTearDown(() => server.close(force: true));
    writeApiJson(server.port, pid: 987654);
    final settings = await loadSettings();
    await settings.setBaseUrl('http://127.0.0.1:${server.port}');
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.ensureRunning(), isTrue);
    expect(spawns, isEmpty);
    expect(launcher.phase, StartupPhase.ready);
    // 复用 ≠ 不认领：关窗时按设置把它一并停掉
    expect(launcher.servicesClaimed, isTrue);
    launcher.dispose();
  });

  // -------------------------------------------------------------------------
  //  首屏闸门（settled）—— "冷启动那一刻打 8080 兜底地址"那一类报错的防线
  // -------------------------------------------------------------------------

  test('settled：出结论之前不放行，结论一出（哪怕是失败）就放行', () async {
    writeApiJson(await deadPort()); // 旧门牌没人听 → 这一轮必然去起新的
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();

    // ⚠ 拿闸门的时机就是真实顺序：`HomeShell` 的 postFrame 先把后端拉起来
    // （**同步**翻成 busy —— 这一步挪到第一个 await 之后，下面这条断言立刻红），
    // 各页面的首屏请求随后才发。
    final running = launcher.ensureRunning();
    var settled = false;
    unawaited(launcher.settled().then((_) => settled = true));

    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(settled, isFalse, reason: '后端还没结论，这时候发首屏请求只能是打 8080 那个兜底地址');

    await running; // 假后端永远不健康 → 这一轮以 failed 收场
    await Future<void>.delayed(Duration.zero);
    expect(settled, isTrue, reason: '失败也要放行：各页面要如实报"连不上"，不许静默重试到底');
    launcher.dispose();
  });

  test('settled：没什么可等时立刻返回（不许把首屏卡住）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    // 什么都没在跑（phase = idle）：立刻放行，不是"等满超时"
    await launcher.settled(timeout: const Duration(milliseconds: 200));
    expect(launcher.phase, StartupPhase.idle);
    launcher.dispose();
  });

  // -------------------------------------------------------------------------
  //  退出收尾
  // -------------------------------------------------------------------------

  test('releaseOnExit：kill 自己认领过的那个后端，而且不碰别人', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    final proc = _FakeProcess(pid: 555000, exit: Completer<int>()..complete(0));
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(
            _Call(exe, List<String>.of(args), workingDirectory, environment),
          );
          return proc;
        };
    await launcher.ensureRunning(); // 起了（等待超时也无妨，认领已发生）
    expect(launcher.servicesClaimed, isTrue);

    expect(await launcher.releaseOnExit(), isTrue);
    expect(proc.killCalls, 1, reason: '句柄在手上，优先用它收');
    expect(killedPids, contains(555000));
    // 收尾之后不再认领：dispose 的兜底不会再杀一遍
    expect(launcher.servicesClaimed, isFalse);
    launcher.dispose();
  });

  test('releaseOnExit：这次没起过 / 没接管过服务就不动它们', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.releaseOnExit(), isFalse);
    expect(killedPids, isEmpty);
    launcher.dispose();
  });

  test('releaseOnExit：设置里关掉"退出停服务"就什么都不做（服务常驻）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings(stopServicesOnExit: false);
    final launcher = BackendLauncher(settings);
    final proc = _FakeProcess(pid: 555000);
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(_Call(exe, List.of(args), workingDirectory, environment));
          return proc;
        };
    await launcher.ensureRunning();

    expect(await launcher.releaseOnExit(), isFalse);
    expect(proc.killCalls, 0);
    expect(killedPids, isEmpty);
    launcher.dispose();
  });

  test('releaseOnExit：只跑一次（关窗口 + dispose 都触发也不会重复停）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    final proc = _FakeProcess(pid: 555000, exit: Completer<int>()..complete(0));
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(_Call(exe, List.of(args), workingDirectory, environment));
          return proc;
        };
    await launcher.ensureRunning();

    expect(await launcher.releaseOnExit(), isTrue);
    expect(await launcher.releaseOnExit(), isFalse);
    expect(proc.killCalls, 1);
    launcher.dispose();
  });

  test('releaseOnExit：后端迟迟不退也不卡住关闭（超时就放它在后台收尾）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    // 永远不退的假进程 = 收尾一直等不到
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(_Call(exe, List.of(args), workingDirectory, environment));
          return _FakeProcess(pid: 555000, exit: Completer<int>());
        };
    await launcher.ensureRunning();

    final sw = Stopwatch()..start();
    final ok = await launcher.releaseOnExit(
      timeout: const Duration(milliseconds: 200),
    );
    sw.stop();

    expect(ok, isTrue, reason: 'kill 已经发出去了，退出码没等到也算停过');
    expect(sw.elapsedMilliseconds, lessThan(3000), reason: '关窗口绝不能被收尾卡住');
    launcher.dispose();
  });

  test('disarm：用户关掉"退出停服务"后撤销认领，退出时不再停', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    final proc = _FakeProcess(pid: 555000);
    BackendLauncher.processStarter =
        (exe, args, {workingDirectory, environment}) async {
          spawns.add(_Call(exe, List.of(args), workingDirectory, environment));
          return proc;
        };
    await launcher.ensureRunning();
    expect(launcher.servicesClaimed, isTrue);

    expect(await launcher.disarm(), isTrue);
    expect(await launcher.releaseOnExit(), isFalse);
    expect(proc.killCalls, 0);
    launcher.dispose();
  });

  // -------------------------------------------------------------------------
  //  设置页按钮
  // -------------------------------------------------------------------------

  test('runAction(down)：按端口记录收 pid，收完不再认领', () async {
    // 端口指向一个没人听的位置：收尾的探活循环第一轮就该确认"端口没了"
    writeApiJson(await deadPort(), pid: 987654);
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);

    await launcher.runAction('down');

    expect(killedPids, contains(987654));
    expect(launcher.servicesClaimed, isFalse);
    expect(launcher.phase, StartupPhase.idle);
    launcher.dispose();
  });

  test('runAction(restart)：先停后起（起参数与直启完全一致）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.runAction('restart');

    expect(spawns, hasLength(1));
    expect(spawns.single.args, contains('--watch-stdin'));
    expect(spawns.single.args, contains('--port'));
    launcher.dispose();
  });

  // -------------------------------------------------------------------------
  //  端口"粘住"：重启不要换端口，手机存的那个地址才一直有效
  //
  //  背景：后端默认 `--port 0`（系统分配，避免被占用 /
  //  落在 Windows 保留段起不来），代价是**重启就换端口**；而手机存的是
  //  `http://<局域网IP>:<端口>` —— 端口一变就得回桌面重扫二维码。
  //  现在 App 会把上次实际用的端口记住，下次**先要回它**；只有"确实被占了"
  //  这一种情况才换（由后端自己回落并在 api.json 里记 `fellBack`）。
  // -------------------------------------------------------------------------

  test('启动：有记录时先要回上次那个端口（不再是 --port 0）', () async {
    writeApiJson(await deadPort());
    final settings = await loadSettings(lastApiPort: 45678);
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    expect(spawns, hasLength(1));
    final i = spawns.single.args.indexOf('--port');
    expect(i, greaterThanOrEqualTo(0));
    expect(
      spawns.single.args[i + 1],
      '45678',
      reason: '要回旧端口，手机存的 http://<IP>:45678 才继续有效',
    );
    launcher.dispose();
  });

  test('启动：没记录过（或记录是脏值）时仍然是 --port 0，让系统挑', () async {
    writeApiJson(await deadPort());
    // 脏值（0 / 越界）必须当"没记录过"：`--port 99999` 会让后端直接退出
    SharedPreferences.setMockInitialValues({
      'comfyhub.projectRoot': root.path,
      'comfyhub.autoStartBackend': false,
      'comfyhub.lastApiPort': 99999,
    });
    final settings = SettingsStore();
    await settings.load();
    expect(settings.lastApiPort, isNull, reason: '越界值不许拼进 argv');
    final launcher = BackendLauncher(settings);
    installStarter();

    await launcher.ensureRunning();

    final i = spawns.single.args.indexOf('--port');
    expect(spawns.single.args[i + 1], '0');
    launcher.dispose();
  });

  test('启动：后端回落时记下新端口，并亮出"手机要重扫"（不许静默）', () async {
    writeApiJson(await deadPort()); // 旧门牌没人听 → 这一轮会去起新的
    final server = await startFakeBackend();
    addTearDown(() => server.close(force: true));
    final settings = await loadSettings(lastApiPort: 45678);
    final launcher = BackendLauncher(settings);
    // 假后端：绑定成功后写 api.json —— 想要 45678 没绑上，回落到 server.port
    installStarter(
      onSpawn: () => writeApiJson(
        server.port,
        pid: 555000,
        requestedPort: 45678,
        fellBack: true,
      ),
    );

    expect(await launcher.ensureRunning(), isTrue);

    // ① 新端口要记下来：下次先要回它，别每次重启都换
    expect(settings.lastApiPort, server.port);
    // ② 回落这件事要能被界面看见（设置页「本地服务」卡片据此显示那行提示）——
    //    否则只会遇到"手机突然连不上"，无法判断需要重新扫码
    expect(launcher.portFallbackFrom, 45678);
    expect(launcher.logLines.any((l) => l.contains('没能用上')), isTrue);
    launcher.dispose();
  });

  test('启动：要回成功时不亮回落标记（界面上不该有多余的警告）', () async {
    writeApiJson(await deadPort());
    final server = await startFakeBackend();
    addTearDown(() => server.close(force: true));
    final settings = await loadSettings(lastApiPort: 45678);
    final launcher = BackendLauncher(settings);
    installStarter(
      onSpawn: () =>
          writeApiJson(server.port, pid: 555000, requestedPort: server.port),
    );

    expect(await launcher.ensureRunning(), isTrue);

    expect(settings.lastApiPort, server.port);
    expect(launcher.portFallbackFrom, isNull);
    expect(launcher.logLines.any((l) => l.contains('端口沿用上次的')), isTrue);
    launcher.dispose();
  });

  test('接管已经在跑的后端时，也把它的端口记下来（下次就能要回它）', () async {
    final server = await startFakeBackend();
    addTearDown(() => server.close(force: true));
    writeApiJson(server.port, pid: 987654, requestedPort: server.port);
    final settings = await loadSettings();
    await settings.setBaseUrl('http://127.0.0.1:${server.port}');
    final launcher = BackendLauncher(settings);

    expect(await launcher.ensureRunning(), isTrue);

    expect(spawns, isEmpty, reason: '本来就健康，不该再起一个');
    expect(
      settings.lastApiPort,
      server.port,
      reason: '接管来的那个也要记住：关掉 App 再开时要能要回同一个端口',
    );
    expect(launcher.portFallbackFrom, isNull);
    launcher.dispose();
  });

  test('端口偏好：落盘读得回来，脏值一律当没记录过', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    expect(settings.lastApiPort, isNull, reason: '第一次运行还没有记录');

    await settings.setLastApiPort(5000);
    expect(
      (await SharedPreferences.getInstance()).getInt('comfyhub.lastApiPort'),
      5000,
    );

    final again = SettingsStore();
    await again.load();
    expect(again.lastApiPort, 5000);

    await again.setLastApiPort(0);
    expect(again.lastApiPort, isNull, reason: '0 不是端口（--port 0 的含义是"让系统挑"）');
  });
}
