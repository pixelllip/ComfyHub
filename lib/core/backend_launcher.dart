import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'settings_store.dart';

/// 启动阶段
enum StartupPhase {
  idle,
  probing,
  starting,
  waiting,
  ready,
  failed,
  skipped,
}

/// 起子进程的可注入缝隙：测试里换掉它，就能断言"到底传了哪些参数"，
/// 而不用真去拉一个 pwsh（在没有 Windows / 没有 PowerShell 的环境里也能跑）。
typedef BackendProcessStarter = Future<Process> Function(
  String executable,
  List<String> arguments, {
  String? workingDirectory,
});

Future<Process> _startProcess(
  String executable,
  List<String> arguments, {
  String? workingDirectory,
  ProcessStartMode mode = ProcessStartMode.normal,
}) =>
    Process.start(
      executable,
      arguments,
      workingDirectory: workingDirectory,
      runInShell: false,
      mode: mode,
    );

/// 负责在 App 启动时把 **MySQL + Kotlin 后端** 拉起来。
///
/// 做法很直接：先探一次 `/api/health`；
///   · 健康  → 什么都不做；
///   · 不健康 / 连不上 → 调用项目里的 `scripts\comfyhub.ps1 up`
///     （它会按「MySQL 先就绪 → 后端再起」的顺序来，而且后端在跑但数据库死了会自动重启后端）；
///   · 边跑边把脚本输出回显到启动页上，最后轮询健康直到就绪。
///
/// 之所以不在 App 里直接起 java/mysqld：脚本里已经处理了一堆本机坑
/// （JDK 版本挑选、WMI 脱离进程树、存储目录、MySQL 初始化……），
/// 复用它比自己再写一遍可靠得多。
class BackendLauncher extends ChangeNotifier {
  BackendLauncher(this._settings);

  final SettingsStore _settings;

  StartupPhase phase = StartupPhase.idle;
  String message = '尚未检查本地服务';
  String? lastError;

  /// 脚本输出的最近若干行，直接显示在启动页上
  final List<String> logLines = [];

  String? projectRoot;
  String? scriptPath;
  String? shellPath;

  Process? _process;
  bool _cancelled = false;
  bool _disposed = false;

  /// 退出收尾只允许跑一次（窗口关闭 + detach 可能都触发）
  bool _releaseStarted = false;

  /// 这个 App 有没有"认领"本地服务：自己用 `up -OwnerPid` 起的，
  /// 或者给已经在跑的服务补挂了退出守护（[armOwnerWatch]）。
  ///
  /// 只有认领过，退出时才会去停它们 —— 不然「你自己在终端里 `up`、
  /// 只是开个 App 看看」这种场景会被 App 顺手带走（README 里承诺过不会）。
  bool servicesClaimed = false;

  /// 起子进程的实现，测试里替换成假的；见 [BackendProcessStarter]
  @visibleForTesting
  static BackendProcessStarter processStarter = _startProcess;

  /// 测试里覆盖 [resolveShell] 的结果，让用例不依赖本机装没装 pwsh；null = 走真实解析
  @visibleForTesting
  static String? shellPathOverride;

  static const int _maxLogLines = 200;

  /// 退出时最多等脚本多久：超时就让它在后台跑完，**关窗口绝不能卡住**
  static const Duration exitReleaseTimeout = Duration(seconds: 8);

  bool get busy =>
      phase == StartupPhase.probing ||
      phase == StartupPhase.starting ||
      phase == StartupPhase.waiting;

  bool get isReady => phase == StartupPhase.ready || phase == StartupPhase.skipped;

  // -------------------------------------------------------------------------
  //  探测
  // -------------------------------------------------------------------------

  /// 探一次后端健康检查；连不上返回 null
  Future<Map<String, dynamic>?> probe({Duration timeout = const Duration(seconds: 3)}) async {
    final uri = Uri.parse('${_settings.baseUrl}/api/health');
    // 本机连"没人监听"的端口要等 SYN 重传（这台机器上实测 ~2 秒），冷启动时服务
    // 还没起来恰是常态；先花 ≤250ms 判一下端口，省掉这段白等。
    // 只对回环地址这么做，远端地址不预检（免得把慢网络误判成"没起来"）。
    if (_isLoopback(uri.host) && !await _portOpen(uri.host, uri.port)) return null;
    try {
      final res = await http.get(uri).timeout(timeout);
      if (res.statusCode == 200) {
        final decoded = jsonDecode(utf8.decode(res.bodyBytes));
        if (decoded is Map) return decoded.cast<String, dynamic>();
      }
    } catch (_) {
      // 连不上就是连不上，交给调用方决定
    }
    return null;
  }

  static bool _isLoopback(String host) =>
      host == '127.0.0.1' || host == 'localhost' || host == '::1' || host == '[::1]';

  /// 端口上有没有人在听（带短超时）
  static Future<bool> _portOpen(String host, int port,
      {int timeoutMs = 250}) async {
    try {
      final s = await Socket.connect(host, port,
          timeout: Duration(milliseconds: timeoutMs));
      s.destroy();
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 从可执行文件所在目录往上找 `scripts\comfyhub.ps1`，定位项目根目录
  static String? detectProjectRoot({String? from}) {
    if (kIsWeb) return null;

    final starts = <String>[];
    if (from != null && from.isNotEmpty) {
      starts.add(from);
    } else {
      try {
        starts.add(File(Platform.resolvedExecutable).parent.path);
      } catch (_) {}
      try {
        starts.add(Directory.current.path);
      } catch (_) {}
    }

    final sep = Platform.pathSeparator;
    for (final start in starts) {
      var dir = Directory(start);
      for (var depth = 0; depth < 8; depth++) {
        final marker = File([dir.path, 'scripts', 'comfyhub.ps1'].join(sep));
        if (marker.existsSync()) return dir.path;
        final parent = dir.parent;
        if (parent.path == dir.path) break;
        dir = parent;
      }
    }
    return null;
  }

  /// 解析出要执行的脚本
  File? resolveScript() {
    final root = (_settings.projectRoot?.isNotEmpty ?? false)
        ? _settings.projectRoot!
        : detectProjectRoot();
    if (root == null) return null;
    projectRoot = root;
    final sep = Platform.pathSeparator;
    final file = File('$root${sep}scripts${sep}comfyhub.ps1');
    if (!file.existsSync()) return null;
    scriptPath = file.path;
    return file;
  }

  /// 找到的是不是 PowerShell 7 的 `pwsh.exe`（Windows 自带的 `powershell.exe` 不算）
  static bool _isPwsh(String shellPath) =>
      shellPath.toLowerCase().endsWith('pwsh.exe');

  static String? resolveShell() {
    if (kIsWeb || !Platform.isWindows) return null;
    if (shellPathOverride != null) return shellPathOverride;
    final candidates = <String>[
      r'C:\Program Files\PowerShell\7\pwsh.exe',
      r'C:\Program Files\PowerShell\7-preview\pwsh.exe',
    ];
    for (final c in candidates) {
      if (File(c).existsSync()) return c;
    }
    // 在整个 PATH 里先找一遍 pwsh，再退而求其次找 powershell.exe。
    // （逐个目录地"先 pwsh 后 powershell"会让靠前目录里的 5.1 抢在靠后的 7 前面。）
    final pathEnv = Platform.environment['PATH'] ?? '';
    final dirs = pathEnv
        .split(';')
        .map((d) => d.trim())
        .where((d) => d.isNotEmpty);
    for (final exe in const ['pwsh.exe', 'powershell.exe']) {
      for (final dir in dirs) {
        final p = '$dir${Platform.pathSeparator}$exe';
        if (File(p).existsSync()) return p;
      }
    }
    // 最后兜底：Windows 自带的 5.1 一定在。返回它让启动能试一次，
    // 上层会用 _isPwsh 判出来并提示装 PowerShell 7。
    const systemPs = r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe';
    if (File(systemPs).existsSync()) return systemPs;
    return null;
  }

  /// 「谁起的谁关」：把 App 自己的 PID 交给脚本，它会挂一个守护进程盯着这个 PID，
  /// App 一退出就只停掉**这次真正启动过**的 MySQL / 后端（见 scripts\watch-owner.ps1）。
  /// 返回是否真的加上了（给日志用）。
  bool _addOwnerArgs(List<String> args, String action) {
    if (!_settings.stopServicesOnExit) return false;
    if (kIsWeb || !Platform.isWindows) return false;
    if (action != 'up' && action != 'restart') return false;
    args..add('-OwnerPid')..add('$pid');
    servicesClaimed = true;
    return true;
  }

  /// 后端是否已经构建/装配过（决定要不要给脚本加 -SkipBuild）
  ///
  /// 两种布局都要认，否则发布包里的 App 会以为后端没构建，去跑一次不存在的 Gradle：
  ///   · 开发布局：server\build\install\comfy-hub-server\bin\（gradle installDist 的产物）
  ///   · 发布包布局：server\bin\（scripts\pack-release.ps1 装配的，见 packaging\manifest.json）
  bool _hasBuiltServer() {
    final root = projectRoot ?? _settings.projectRoot ?? detectProjectRoot();
    if (root == null) return false;
    final sep = Platform.pathSeparator;
    final candidates = <String>[
      '$root${sep}server${sep}bin${sep}comfy-hub-server.bat',
      '$root${sep}server${sep}build${sep}install${sep}comfy-hub-server${sep}bin${sep}comfy-hub-server.bat',
    ];
    for (final path in candidates) {
      if (File(path).existsSync()) return true;
    }
    return false;
  }

  // -------------------------------------------------------------------------
  //  启动
  // -------------------------------------------------------------------------

  /// 确保后端可用。返回 true 表示最终健康。
  Future<bool> ensureRunning({bool force = false}) async {
    if (busy) return false;
    _cancelled = false;
    lastError = null;

    _set(StartupPhase.probing, '正在检查本地服务 ${_settings.baseUrl} …');
    final health = await probe();
    if (!force && health != null && health['database'] == 'ok') {
      _log('后端已在运行（版本 ${health['version']}，数据库 ok）');
      _set(StartupPhase.ready, '后端已就绪');
      // 走这条分支说明脚本根本没跑过 up —— 于是「关 App 停服务」的 watchdog 也没挂上，
      // 之后硬杀 App（任务管理器 / 崩溃）就会把 MySQL + 后端留在后台。
      // 这里补挂一个守护进程；它只是锦上添花，失败也绝不影响启动。
      unawaited(armOwnerWatch());
      return true;
    }
    if (health != null) {
      _log('后端进程在跑，但数据库状态是 ${health['database']}，尝试修复…');
    } else {
      _log('后端未响应，准备启动。');
    }

    if (kIsWeb || !Platform.isWindows) {
      return _fail('当前平台不支持自动启动本地服务（仅 Windows 桌面端）。');
    }

    final script = resolveScript();
    if (script == null) {
      return _fail('找不到 scripts\\comfyhub.ps1。请在「设置 → 本地服务」里指定项目根目录。');
    }
    final shell = resolveShell();
    if (shell == null) {
      return _fail('PATH 里找不到 PowerShell，无法启动本地服务。'
          '请安装 PowerShell 7（winget install --id Microsoft.PowerShell），装完重启 App。');
    }
    if (!_isPwsh(shell)) {
      // Windows 自带的 5.1 不算数：scripts\*.ps1 内部到处是 `& pwsh -NoProfile -File ...`，
      // 拿 powershell.exe 去跑，子调用会以"pwsh 不是内部或外部命令"失败。
      _log('警告: 只找到 Windows PowerShell 5.1（$shell），缺少 PowerShell 7。');
      _log('      scripts 之间互相调用 pwsh，缺了它启动多半会失败。');
      _log('      安装: winget install --id Microsoft.PowerShell --source winget');
    }
    shellPath = shell;

    final args = <String>[
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      script.path,
      'up',
    ];
    if (_hasBuiltServer()) {
      args.add('-SkipBuild');
    } else {
      _log('后端还没构建过，本次会先跑一次 gradle 构建（可能要好几分钟）…');
    }
    final dataDir = _settings.mysqlDataDir;
    if (dataDir != null && dataDir.isNotEmpty) {
      args..add('-DataDir')..add(dataDir);
      _log('MySQL 实例目录: $dataDir');
    }
    if (_addOwnerArgs(args, 'up')) {
      _log('关闭 App 时会一并停掉本次启动的本地服务（设置里可关）');
    }

    _set(StartupPhase.starting, '正在启动 MySQL + 后端…');
    _log('$shell ${args.join(' ')}');

    try {
      _process = await Process.start(
        shell,
        args,
        workingDirectory: script.parent.parent.path,
        runInShell: false,
      );
    } catch (e) {
      return _fail('启动脚本失败: $e');
    }

    // allowMalformed: 脚本侧虽然已经把输出钉成 UTF-8（见 scripts\*.ps1 顶部的
    // [Console]::OutputEncoding），但万一某个版本/某种环境下漏出 GBK 字节，
    // 也不该让整个日志流报 FormatException（原先是 Missing extension byte）。
    _process!.stdout
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen(_log, onError: (Object e) => _log('[stdout 错误] $e'));
    _process!.stderr
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen((l) => _log(l.isEmpty ? '' : l), onError: (Object e) => _log('[stderr 错误] $e'));

    final exitCode = await _process!.exitCode;
    _process = null;
    if (_cancelled) {
      _set(StartupPhase.failed, '已取消启动');
      return false;
    }
    _log('启动脚本结束，退出码 $exitCode');

    _set(StartupPhase.waiting, '等待后端就绪…');
    final deadline = DateTime.now().add(const Duration(seconds: 90));
    var attempt = 0;
    while (DateTime.now().isBefore(deadline)) {
      attempt++;
      final h = await probe(timeout: const Duration(seconds: 2));
      if (h != null && h['database'] == 'ok') {
        _log('后端已就绪：版本 ${h['version']}，存储目录 ${h['storageDir']}');
        _set(StartupPhase.ready, '后端已就绪');
        return true;
      }
      if (attempt % 5 == 0) {
        _log('仍在等待…（${attempt * 2}s）${h == null ? "后端未响应" : "数据库 ${h['database']}"}');
      }
      await Future.delayed(const Duration(seconds: 2));
    }

    return _fail('等待后端超时。可以看设置页的「查看服务日志」，或手动执行 '
        'scripts\\comfyhub.ps1 doctor 体检（缺 pwsh / VC++ 运行时 / JDK 会在那里报出来），'
        '也可以直接 scripts\\comfyhub.ps1 up 看完整输出。');
  }

  /// 停止 / 重启 / 只跑一遍脚本（不走健康等待）
  Future<void> runAction(String action) async {
    if (busy) return;
    final script = resolveScript();
    final shell = resolveShell();
    if (script == null || shell == null) {
      _fail('找不到 scripts\\comfyhub.ps1 或 pwsh。');
      return;
    }
    _set(StartupPhase.starting, '正在执行 comfyhub.ps1 $action …');
    final args = <String>[
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      script.path,
      action,
    ];
    final dataDir = _settings.mysqlDataDir;
    if (dataDir != null && dataDir.isNotEmpty && action != 'logs') {
      args..add('-DataDir')..add(dataDir);
    }
    _addOwnerArgs(args, action);
    final code = await _exec(shell, args, script.parent.parent.path);
    if (code == null) return;

    if (action == 'down' || action == 'restart') {
      final h = await probe(timeout: const Duration(seconds: 2));
      if (h != null && h['database'] == 'ok') {
        _set(StartupPhase.ready, '后端已就绪');
      } else if (action == 'down') {
        _set(StartupPhase.idle, '本地服务已停止');
      } else {
        _set(StartupPhase.failed, '重启后后端没起来，请看日志');
      }
    } else {
      _set(phase == StartupPhase.starting ? StartupPhase.idle : phase, message);
    }
  }

  // -------------------------------------------------------------------------
  //  退出收尾：「谁起的谁关」
  // -------------------------------------------------------------------------

  /// 关 App 时把**本地服务**停掉：调 `scripts\comfyhub.ps1 release`
  /// （只停后端 + MySQL，**绝不动 App 进程** —— `down` 会按进程名杀 viewer，不能在这里用）。
  ///
  /// 为什么能"活过我们自己的退出"：pwsh 是独立进程，父进程（App）退出不会带走它
  /// （Windows 不会因为父进程退出就杀子进程）。实测：父进程起来 400ms 后直接 exit(0)，
  /// 子 pwsh 的 stdout 管道虽然断了，它依然把后面的步骤全部跑完并写了收尾文件。
  ///
  /// 最多等 [timeout] 就返回，剩下的步骤在后台跑完 —— 关窗口绝不能卡住。
  /// 任何异常（找不到脚本 / 起不来 / 超时）都只记日志，绝不让 App 关不掉。
  Future<bool> releaseOnExit({
    Duration timeout = exitReleaseTimeout,
  }) async {
    if (_releaseStarted) return false;
    _releaseStarted = true;
    if (!_settings.stopServicesOnExit) {
      _log('设置里关掉了「关闭 App 时一并停止本地服务」，保留后台服务。');
      return false;
    }
    if (!servicesClaimed) {
      _log('这次运行没有启动/接管过本地服务，退出时不主动停它们。');
      return false;
    }
    final args = _buildServiceArgs('release');
    if (args == null) return false;

    final proc = await _spawnDetached(args.shell, args.args, args.workDir);
    if (proc == null) return false;

    try {
      final code = await proc.exitCode.timeout(timeout);
      _log('已停止本地服务（release 退出码 $code）');
      return true;
    } on TimeoutException {
      _log('release 超过 ${timeout.inSeconds}s 还没结束，让它在后台跑完（不影响关闭）。');
      return false;
    } catch (e) {
      // exitCode 在个别平台/模式下会直接抛（比如 detached），忽略即可
      _log('release 结果未知（$e），已放它在后台跑完。');
      return false;
    }
  }

  /// 用户把「关闭 App 时一并停止本地服务」**关掉**时调用：撤掉已挂的守护进程。
  ///
  /// 只跳过 release 是不够的 —— 守护进程还在跑，App 一死它照样停服务。
  /// 这个动作删掉认领令牌，守护下一轮（≤3 秒）就静默退出，服务交还给用户。
  Future<bool> disarmOwnerWatch() async {
    if (!servicesClaimed) return false;
    final args = _buildServiceArgs('unwatch');
    if (args == null) return false;
    final proc = await _spawnDetached(args.shell, args.args, args.workDir);
    if (proc == null) return false;
    // 撤回认领：之后退出时 release 不会再动服务
    servicesClaimed = false;
    _log('用户关掉了「关闭 App 时一并停止本地服务」：已撤销退出守护，服务继续运行。');
    return true;
  }

  /// 补挂「关 App 自动停服务」的守护进程：`comfyhub.ps1 watch -OwnerPid <自己>`。
  ///
  /// 用在 [ensureRunning] 探到后端已经健康、**根本没跑 up** 的那条路径上：
  /// 那种情况下 watchdog 没挂上，硬杀 App 就会留下没人管的 MySQL + 后端。
  /// 只等"进程起来"（几毫秒），不等它退出；出错只记日志，绝不抛。
  Future<bool> armOwnerWatch() async {
    if (!_settings.stopServicesOnExit) return false;
    final args = _buildServiceArgs('watch', withOwnerPid: true);
    if (args == null) return false;

    final proc = await _spawnDetached(args.shell, args.args, args.workDir);
    if (proc == null) return false;
    // 挂上了守护 = 我们认领了这几个服务：硬杀由守护收尾，正常关闭由 release 收尾
    servicesClaimed = true;
    _log('已补挂退出守护：App(PID $pid) 退出后会停掉正在运行的本地服务。');
    return true;
  }

  /// 给 release / watch 拼参数；返回 null 表示环境不满足（找不到脚本 / pwsh）。
  _ServiceCall? _buildServiceArgs(String action, {bool withOwnerPid = false}) {
    if (kIsWeb || !Platform.isWindows) return null;
    final script = resolveScript();
    if (script == null) {
      _log('提示: 找不到 scripts\\comfyhub.ps1，退出时不会自动停本地服务。');
      return null;
    }
    final shell = resolveShell();
    if (shell == null) {
      _log('提示: PATH 里找不到 PowerShell，退出时不会自动停本地服务。');
      return null;
    }
    final args = <String>[
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      script.path,
      action,
    ];
    if (withOwnerPid) {
      args..add('-OwnerPid')..add('$pid');
    }
    final dataDir = _settings.mysqlDataDir;
    if (dataDir != null && dataDir.isNotEmpty) {
      args..add('-DataDir')..add(dataDir);
    }
    return _ServiceCall(shell, args, script.parent.parent.path);
  }

  /// 起一个"自己活下去"的子进程并把输出回显到日志；起不来返回 null（不抛）。
  Future<Process?> _spawnDetached(
      String shell, List<String> args, String workingDirectory) async {
    try {
      _log('$shell ${args.join(' ')}');
      final proc = await processStarter(shell, args, workingDirectory: workingDirectory);
      proc.stdout
          .transform(const Utf8Decoder(allowMalformed: true))
          .transform(const LineSplitter())
          .listen(_log, onError: (_) {});
      proc.stderr
          .transform(const Utf8Decoder(allowMalformed: true))
          .transform(const LineSplitter())
          .listen(_log, onError: (_) {});
      return proc;
    } catch (e) {
      _log('执行失败（已忽略，不影响关闭）: $e');
      return null;
    }
  }

  /// 把 MySQL 的实例目录整体搬到 [newDir]（数据文件 + my.ini + 日志）。
  ///
  /// 调的是 `scripts\mysql.ps1 move`：它会先停库、robocopy 过去、
  /// 写一个 `.mysql-location.json` 记住新位置，再把库拉起来。
  /// 源目录不会被删除，确认没问题后可以自己删。
  Future<bool> moveMysqlData(String newDir) async {
    if (busy) return false;
    final root = (_settings.projectRoot?.isNotEmpty ?? false)
        ? _settings.projectRoot!
        : detectProjectRoot();
    final shell = resolveShell();
    if (kIsWeb || !Platform.isWindows || root == null || shell == null) {
      _fail('找不到项目目录或 pwsh，无法迁移。');
      return false;
    }
    final sep = Platform.pathSeparator;
    final script = File([root, 'scripts', 'mysql.ps1'].join(sep));
    if (!script.existsSync()) {
      _fail('找不到 scripts\\mysql.ps1');
      return false;
    }
    _set(StartupPhase.starting, '正在迁移 MySQL 数据目录到 $newDir …');
    final code = await _exec(
      shell,
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        script.path,
        'move',
        '-DataDir',
        newDir,
      ],
      root,
    );
    if (code == 0) {
      await _settings.setMysqlDataDir(newDir);
      _set(StartupPhase.idle, 'MySQL 数据目录已迁移到 $newDir');
      return true;
    }
    _fail('迁移失败（退出码 $code），请看下面的日志');
    return false;
  }

  /// 跑一个子进程并把输出实时回显到启动页日志；返回退出码（启动失败返回 null）
  Future<int?> _exec(String shell, List<String> args, String workingDirectory) async {
    _log('$shell ${args.join(' ')}');
    try {
      final proc = await Process.start(
        shell,
        args,
        workingDirectory: workingDirectory,
        runInShell: false,
      );
      proc.stdout
          .transform(const Utf8Decoder(allowMalformed: true))
          .transform(const LineSplitter())
          .listen(_log);
      proc.stderr
          .transform(const Utf8Decoder(allowMalformed: true))
          .transform(const LineSplitter())
          .listen(_log);
      final code = await proc.exitCode;
      _log('退出码 $code');
      return code;
    } catch (e) {
      _fail('执行失败: $e');
      return null;
    }
  }

  /// 用户在启动页点「先跳过」——不阻塞使用（离线只看本地缓存/稍后再连）
  void skip() {
    _cancelled = true;
    _set(StartupPhase.skipped, '已跳过自动启动');
  }

  void cancel() {
    _cancelled = true;
    final p = _process;
    if (p != null) {
      try {
        p.kill();
      } catch (_) {}
    }
    _set(StartupPhase.failed, '已取消');
  }

  void clearLog() {
    logLines.clear();
    _notify();
  }

  // -------------------------------------------------------------------------

  bool _fail(String msg) {
    lastError = msg;
    _log(msg);
    _set(StartupPhase.failed, msg);
    return false;
  }

  void _log(String line) {
    if (line.isEmpty) return;
    logLines.add(line);
    if (logLines.length > _maxLogLines) {
      logLines.removeRange(0, logLines.length - _maxLogLines);
    }
    _notify();
  }

  void _set(StartupPhase p, String msg) {
    phase = p;
    message = msg;
    _notify();
  }

  /// 退出收尾是异步的，可能落在 dispose 之后 —— 那时再 notifyListeners 会抛
  /// 「was used after being disposed」，会把 App 的关闭流程搅乱，所以这里兜一下。
  void _notify() {
    if (_disposed) return;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    try {
      _process?.kill();
    } catch (_) {}
    super.dispose();
  }
}

/// release / watch 一次调用的参数（shell + 参数 + 工作目录）
class _ServiceCall {
  final String shell;
  final List<String> args;
  final String workDir;

  const _ServiceCall(this.shell, this.args, this.workDir);
}
