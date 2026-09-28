// 退出收尾（release / watch）的参数回归测试。
//
// 这里**不真的起进程**：`BackendLauncher.processStarter` 是留给测试的缝隙，
// 换成假的之后就能断言"到底用哪个 shell、传了哪些参数、工作目录是哪儿"，
// 用例因此在没有 pwsh 的机器上也能跑。
//
// 背景（修的是哪个 bug）：App 退出时没人停它冷启动拉起来的 MySQL + 后端。
// 窗口关闭那一刀走 `comfyhub.ps1 release`（只停后端 + MySQL，不碰 App 自己，
// 所以不能用会按进程名杀 viewer 的 `down`）；App 启动时探到"后端已经健康"就直接
// 返回、没跑过 up 的那条路，则补挂一次 `comfyhub.ps1 watch -OwnerPid <自己>`。

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:viewer/core/backend_launcher.dart';
import 'package:viewer/core/settings_store.dart';

/// 一次子进程调用（假 starter 记下来的）
class _Call {
  final String exe;
  final List<String> args;
  final String? workDir;
  final Map<String, String>? environment;

  _Call(this.exe, this.args, this.workDir, [this.environment]);
}

/// 假的 Process：只提供 [_spawnDetached] 会用到的那几个成员
class _FakeProcess implements Process {
  _FakeProcess({int? exitCode, Completer<int>? never})
      : _exit = never ?? (Completer<int>()..complete(exitCode ?? 0));

  final Completer<int> _exit;

  @override
  int get pid => 4242;

  @override
  Stream<List<int>> get stdout => const Stream<List<int>>.empty();

  @override
  Stream<List<int>> get stderr => const Stream<List<int>>.empty();

  @override
  IOSink get stdin => throw UnimplementedError();

  @override
  Future<int> get exitCode => _exit.future;

  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) => true;
}

void main() {
  late Directory root;
  late List<_Call> calls;
  late BackendProcessStarter realStarter;

  const fakeShell = r'C:\fake\pwsh.exe';
  const dataDir = r'D:\mysql\comfyhub-data';

  setUp(() {
    root = Directory.systemTemp.createTempSync('comfyhub-launcher-');
    // 认得出来的项目布局：<root>\scripts\comfyhub.ps1
    Directory('${root.path}${Platform.pathSeparator}scripts').createSync();
    File('${root.path}${Platform.pathSeparator}scripts'
            '${Platform.pathSeparator}comfyhub.ps1')
        .writeAsStringSync('# stub');

    calls = [];
    realStarter = BackendLauncher.processStarter;
    BackendLauncher.shellPathOverride = fakeShell;
    // 把"去哪找 .run\api.json"钉在临时根上：否则 load/refresh 会读到**真项目**
    // 的端口文件，用例结果随"这台机器的后端正跑在哪个端口"漂移
    // （settings_store.dart 里那条注释就是这个坑的现场记录）。
    SettingsStore.portFileRootsOverride = [root.path];
  });

  tearDown(() {
    BackendLauncher.processStarter = realStarter;
    BackendLauncher.shellPathOverride = null;
    BackendLauncher.waitDeadlineOverride = null;
    SettingsStore.portFileRootsOverride = null;
    try {
      root.deleteSync(recursive: true);
    } catch (_) {}
  });

  /// 装一个假的进程启动器
  void installStarter({bool throwError = false, bool neverExit = false}) {
    BackendLauncher.processStarter = (exe, args, {workingDirectory, environment}) async {
      calls.add(_Call(exe, List<String>.of(args), workingDirectory, environment));
      if (throwError) throw const ProcessException('pwsh', [], 'boom');
      return _FakeProcess(never: neverExit ? Completer<int>() : null);
    };
  }

  Future<SettingsStore> loadSettings({
    String? projectRoot,
    String? mysqlDataDir,
    bool stopServicesOnExit = true,
  }) async {
    final values = <String, Object>{
      'comfyhub.projectRoot': projectRoot ?? root.path,
      'comfyhub.stopServicesOnExit': stopServicesOnExit,
      'comfyhub.autoStartBackend': false,
    };
    if (mysqlDataDir != null) {
      values['comfyhub.mysqlDataDir'] = mysqlDataDir;
    }
    SharedPreferences.setMockInitialValues(values);
    final settings = SettingsStore();
    await settings.load();
    return settings;
  }

  /// 模拟"这个 App 认领了本地服务"：真实流程里是启动时 `up -OwnerPid`
  /// 或探到后端已健康后补挂 `watch` 做的（两者都会先起一个子进程）。
  Future<void> claim(BackendLauncher launcher) async {
    expect(await launcher.armOwnerWatch(), isTrue);
    calls.clear();
  }

  test('releaseOnExit：调 release（不是 down），把 -DataDir 原样传下去', () async {
    final settings = await loadSettings(mysqlDataDir: dataDir);
    final launcher = BackendLauncher(settings);
    installStarter();
    await claim(launcher);

    final ok = await launcher.releaseOnExit();

    expect(ok, isTrue);
    expect(calls, hasLength(1));
    expect(calls.single.exe, fakeShell);
    expect(calls.single.workDir, root.path);
    expect(calls.single.args, [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      '${root.path}${Platform.pathSeparator}scripts'
          '${Platform.pathSeparator}comfyhub.ps1',
      'release',
      '-DataDir',
      dataDir,
    ]);
    // 一定要走 release：down 会按进程名把 App 自己杀掉
    expect(calls.single.args, isNot(contains('down')));
    launcher.dispose();
  });

  test('releaseOnExit：没设 MySQL 目录时不传 -DataDir', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();
    await claim(launcher);

    await launcher.releaseOnExit();

    expect(calls.single.args.sublist(5), ['release']);
    launcher.dispose();
  });

  test('releaseOnExit：没认领过服务（你自己在终端里 up 的）就不动它们', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.releaseOnExit(), isFalse);
    expect(calls, isEmpty);
    launcher.dispose();
  });

  test('releaseOnExit：设置里关掉"退出停服务"就什么都不做', () async {
    final settings = await loadSettings(stopServicesOnExit: false);
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.releaseOnExit(), isFalse);
    expect(calls, isEmpty);
    launcher.dispose();
  });

  test('armOwnerWatch：补挂守护进程，带上自己的 PID', () async {
    final settings = await loadSettings(mysqlDataDir: dataDir);
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.armOwnerWatch(), isTrue);

    expect(calls, hasLength(1));
    expect(calls.single.exe, fakeShell);
    expect(calls.single.args, [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      '${root.path}${Platform.pathSeparator}scripts'
          '${Platform.pathSeparator}comfyhub.ps1',
      'watch',
      '-OwnerPid',
      '$pid',
      '-DataDir',
      dataDir,
    ]);
    launcher.dispose();
  });

  test('releaseOnExit：脚本卡住也不会一直等（超时返回，让它后台跑完）', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter(neverExit: true);
    await claim(launcher);

    final sw = Stopwatch()..start();
    final ok = await launcher.releaseOnExit(timeout: const Duration(milliseconds: 120));
    sw.stop();

    expect(ok, isFalse);
    expect(sw.elapsedMilliseconds, lessThan(3000));
    expect(calls, hasLength(1)); // 进程已经起出去了
    launcher.dispose();
  });

  test('releaseOnExit：起不来 / 找不到脚本都只是返回 false，不抛异常', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter(throwError: true);

    // 脚本都起不来 → 认领不了，退出时自然也不会去停（返回 false，不抛）
    expect(await launcher.releaseOnExit(), isFalse);
    launcher.dispose();

    // 找不到 scripts\comfyhub.ps1（用一个空目录当项目根）
    final empty = Directory.systemTemp.createTempSync('comfyhub-empty-');
    addTearDown(() => empty.deleteSync(recursive: true));
    final settings2 = await loadSettings(projectRoot: empty.path);
    final launcher2 = BackendLauncher(settings2);
    calls = [];
    expect(await launcher2.releaseOnExit(), isFalse);
    expect(calls, isEmpty);
    launcher2.dispose();
  });

  test('releaseOnExit：只跑一次（关窗口 + detach 都触发也不会重复停服务）', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();
    await claim(launcher);

    expect(await launcher.releaseOnExit(), isTrue);
    expect(await launcher.releaseOnExit(), isFalse);
    expect(calls, hasLength(1));
    launcher.dispose();
  });

  test('退出收尾在 dispose 之后跑也不会抛（日志那条链路）', () async {
    final settings = await loadSettings();
    final launcher = BackendLauncher(settings);
    installStarter();
    await claim(launcher);
    // 让 starter 在返回前先 dispose 掉 launcher，模拟"关窗口时 widget 树已经拆了"
    BackendLauncher.processStarter = (exe, args, {workingDirectory, environment}) async {
      calls.add(_Call(exe, List<String>.of(args), workingDirectory));
      launcher.dispose();
      return _FakeProcess();
    };
    expect(await launcher.releaseOnExit(), isTrue);
  });

  test('release/watch 的日志会被记下来（方便排查"到底停没停"）', () async {
    final settings = await loadSettings(mysqlDataDir: dataDir);
    final launcher = BackendLauncher(settings);
    installStarter();
    await claim(launcher);

    await launcher.releaseOnExit();
    expect(launcher.logLines.any((l) => l.contains('release')), isTrue);
    launcher.dispose();
  });

  test('ensureRunning：后端本来就健康时补挂 watch，而不是什么都不干', () async {
    // 真起一个本地 HTTP 服务当"后端"，让 probe() 走 health.ok 那条早返回分支
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((req) {
      req.response
        ..statusCode = 200
        ..headers.contentType = ContentType.json
        ..write(jsonEncode({'version': 'test', 'database': 'ok'}));
      req.response.close();
    });

    final settings = await loadSettings();
    await settings.setBaseUrl('http://127.0.0.1:${server.port}');
    final launcher = BackendLauncher(settings);
    installStarter();

    expect(await launcher.ensureRunning(), isTrue);

    // armOwnerWatch 是 unawaited 的（不能拖慢启动），等它把守护挂上
    for (var i = 0; i < 50 && !launcher.servicesClaimed; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(calls, hasLength(1));
    expect(calls.single.args.sublist(5), ['watch', '-OwnerPid', '$pid']);
    // 补挂成功 = 认领了服务：关窗口时 release 才有资格停它们
    expect(launcher.servicesClaimed, isTrue);
    launcher.dispose();
  });

  /// 写一份 `.run\api.json`（后端绑定成功后落盘的那份端口记录）
  void writeApiJson(Directory root, int port) {
    final runDir = Directory('${root.path}${Platform.pathSeparator}.run');
    runDir.createSync(recursive: true);
    File('${runDir.path}${Platform.pathSeparator}api.json').writeAsStringSync(
        '{"port":$port,"pid":0,"host":"127.0.0.1",'
        '"url":"http://127.0.0.1:$port","requestedPort":0,"fellBack":false}');
  }

  test('ensureRunning：up 把后端起到了新端口时，等待中会按 .run\\api.json 重新发现', () async {
    // 先占一个端口再放掉 = 一个保证没人听的"死端口"（App 启动时读到的旧门牌）
    final dead = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final deadPort = dead.port;
    await dead.close();
    writeApiJson(root, deadPort);

    // 假后端：起来之后 /api/health 回 200 ok
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((req) {
      req.response
        ..statusCode = 200
        ..headers.contentType = ContentType.json
        ..write(jsonEncode({'version': 'test', 'database': 'ok'}));
      req.response.close();
    });

    final settings = await loadSettings();
    // 此时 settings.baseUrl 还是死端口（load 时读的旧 api.json）
    final launcher = BackendLauncher(settings);
    // 假的 up：退出前把 api.json 改写成**新端口**（后端绑上后自己会写）
    BackendLauncher.processStarter = (exe, args, {workingDirectory, environment}) async {
      calls.add(_Call(exe, List<String>.of(args), workingDirectory));
      writeApiJson(root, server.port);
      return _FakeProcess();
    };
    BackendLauncher.waitDeadlineOverride = const Duration(seconds: 10);

    expect(await launcher.ensureRunning(), isTrue);

    // 关键断言：地址跟着 api.json 走到了新端口，而不是对着旧端口等满 90 秒
    expect(settings.baseUrl, 'http://127.0.0.1:${server.port}');
    expect(launcher.logLines.any((l) => l.contains('后端换了端口')), isTrue);
    expect(launcher.phase, StartupPhase.ready);
    launcher.dispose();
  });

  test('允许局域网连接打开时，up 带上 COMFYHUB_ALLOW_REMOTE=1；关着时不带', () async {
    // 用户 bug（2026-09-28）「手机连接桌面端失败，报 timed out」的根因是后端只听
    // 127.0.0.1；开关就是靠这个环境变量一路传到 server.ps1 / Rust main 的。
    // 指到一个必死的地址，逼 ensureRunning 真的去跑 up（假 starter 秒退，
    // 等待期截短到 2 秒 —— 我们只看传了什么环境变量，不关心最终起没起来）。
    Future<SettingsStore> deadSettings() async {
      final settings = await loadSettings();
      await settings.setBaseUrl('http://127.0.0.1:1');
      return settings;
    }

    BackendLauncher.waitDeadlineOverride = const Duration(seconds: 2);
    addTearDown(() => BackendLauncher.waitDeadlineOverride = null);

    // 先关着：不带环境变量
    final launcher = BackendLauncher(await deadSettings());
    installStarter();
    expect(await launcher.ensureRunning(), isFalse);
    final off = calls.firstWhere((c) => c.args.contains('up'));
    expect(off.environment ?? const {}, isEmpty);
    launcher.dispose();

    // 再打开：环境变量必须跟着脚本下去（关不掉它真机永远连不上桌面）
    final settings2 = await deadSettings();
    await settings2.setAllowRemoteBackend(true);
    final launcher2 = BackendLauncher(settings2);
    calls.clear();
    installStarter();
    expect(await launcher2.ensureRunning(), isFalse);
    final on = calls.firstWhere((c) => c.args.contains('up'));
    expect(on.environment, containsPair('COMFYHUB_ALLOW_REMOTE', '1'));
    launcher2.dispose();
  });
}
