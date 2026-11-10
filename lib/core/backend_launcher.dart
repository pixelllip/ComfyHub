import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'backend_paths.dart';
import 'library_identity.dart';
import 'settings_store.dart';

/// 启动阶段
enum StartupPhase { idle, probing, starting, waiting, ready, failed, skipped }

/// 起子进程的可注入缝隙：测试里换掉它，就能断言"到底起的是哪个 exe、传了哪些参数"，
/// 而不用真去拉一个后端进程（在没有 Windows / 没有后端二进制的机器上也能跑）。
typedef BackendProcessStarter = Future<Process> Function(
  String executable,
  List<String> arguments, {
  String? workingDirectory,
  Map<String, String>? environment,
});

/// 跑"小工具"进程（`icacls` / `taskkill`）的缝隙，测试里替换掉。
typedef ToolRunner = Future<ProcessResult?> Function(
  String executable,
  List<String> arguments,
);

Future<Process> _startProcess(
  String executable,
  List<String> arguments, {
  String? workingDirectory,
  Map<String, String>? environment,
  ProcessStartMode mode = ProcessStartMode.normal,
}) => Process.start(
  executable,
  arguments,
  workingDirectory: workingDirectory,
  environment: environment,
  runInShell: false,
  mode: mode,
);

/// **App 自己把 Rust 后端拉起来 / 关掉**（2026-09-30 起不再经过 pwsh）。
///
/// # 为什么改成这样
///
/// 以前这条路是 `pwsh → comfyhub.ps1 → server.ps1 → WMI/cmd → 后端`：五层、要装
/// PowerShell 7、要躲 WMI 的拒绝访问、要靠 `watch-owner.ps1` 看着 App 的 pid。
/// 现在 App 直接 `Process.start(<根>\backend\comfyhub-server-rs.exe)`：
///
///  1. **不会有黑框**：Dart 的 `Process.start`（attached 模式）自己就带
///     `CREATE_NO_WINDOW` —— 这正是它注释里写的那个场景（"for gui applications
///     (flutter on windows) console might be absent, will create by CreateProcessW"）。
///     所以 `scripts\silent-process.ps1` 那套 WMI/SW_HIDE 可以整体退场；
///  2. **谁起的谁关**：进程句柄在我们手里，退出时 `kill` 一下就行 ——
///     `scripts\watch-owner.ps1` 那个"盯着 App pid 的守护进程"也不需要了；
///  3. **强杀 App 也不会留孤儿**：起后端时带 `--watch-stdin`，后端的 stdin 是
///     我们给它的那根管道，写端**只有我们持有**；App 一死（任务管理器 / 崩溃）
///     管道即关，后端读到 EOF 就自己优雅退出（见 `server-rs/src/main.rs`）。
///
/// ⚠ **`_process` 这个引用必须一直留着**：stdin 那根管道的写端挂在这个 Dart
/// `Process` 对象上，一旦被 GC 回收，后端就会以为"App 没了"而退出。
/// 所以启动成功后**绝不能**像以前那样把它置空。
///
/// # 起哪一份、怎么找
///
/// 见 [`BackendLayout`] / [`serverExeCandidates`]：release 产物里是
/// `<viewer.exe 同级>\backend\comfyhub-server-rs.exe`（构建时由
/// `windows\CMakeLists.txt` 装配进去），源码树里是
/// `server-rs\target\{release,debug}\…`，`COMFYHUB_SERVER_EXE` 永远最高优先。
class BackendLauncher extends ChangeNotifier {
  BackendLauncher(this._settings);

  final SettingsStore _settings;

  StartupPhase phase = StartupPhase.idle;
  String message = '尚未检查本地服务';
  String? lastError;

  /// 子进程输出的最近若干行，直接显示在启动页 / 设置页上
  final List<String> logLines = [];

  /// 解析出来的布局（根目录 + 后端 exe）
  BackendLayout? layout;

  String? serverExe;
  String? serverExeSource;

  String? get projectRoot => layout?.root;

  /// 上次解析出来的数据目录结论（指针 / 实际数据不一致、产物目录不见了都在里面）。
  ///
  /// 界面（启动横幅、设置 → 数据目录）**只读这一份** —— 别各自拿 `dataDirOf()`
  /// 再判一遍，两边漂移的表现就是"设置里说 A、实际用的是 B"。
  DataDirResolution? dataDirInfo;

  /// 这次真正用的数据目录（库 + `storage`；默认就是运行根目录，可被
  /// `<根>\data-dir.json` 指到别的盘，见 `backend_paths.dart` 的 `resolveDataDir`）。
  String? get dataDir {
    final info = dataDirInfo;
    if (info != null) return info.dataDir;
    final root = layout?.root;
    return root == null ? null : dataDirOf(root);
  }

  /// 数据目录这件事上有异常时的一句人话（正常时 null，横幅不显示）。
  String? get dataDirNotice => dataDirInfo?.notice;

  Process? _process;
  bool _cancelled = false;
  bool _disposed = false;

  /// 退出收尾只允许跑一次（窗口关闭 + dispose 可能都触发）
  bool _releaseStarted = false;

  /// 这次要停掉的后端 pid。
  ///
  /// 包含两种：**我们自己起的**，以及**启动时探到已经在跑、被我们接管的**
  /// （后者是设置里那句"包括启动 App 之前就已经在跑的"；以前靠
  /// `watch -OwnerPid` 挂守护进程实现，现在就是记下 pid）。
  final Set<int> _claimedPids = {};

  /// 这个 App 有没有"认领"本地服务：自己起的，或者接管了已经在跑的那个。
  /// 只有认领过，退出时才会去停它们 —— 不然「别人起的后端、只是开个 App 看看」
  /// 会被顺手带走。
  bool servicesClaimed = false;

  /// 这次后端**没能用回**我们想要的端口时，记下当初想要的那个（null = 一切正常）。
  ///
  /// 判据**不是我猜的**，是后端自己写的 `.run\api.json`：`fellBack: true` +
  /// `requestedPort: <我们给的>`。为什么要让界面看得见（`settings_page.dart` 的
  /// 「本地服务」卡片会显示它）：**端口一换，手机上保存的那个地址就失效了** ——
  /// 用户得回桌面上重新扫一次配对二维码。不写出来的话，手机"突然连不上"
  /// 就只剩用户自己猜（本仓 §12.3「界面上看不见的现象，用户只能靠猜」）。
  int? portFallbackFrom;

  /// 起子进程的实现，测试里替换成假的；见 [BackendProcessStarter]
  @visibleForTesting
  static BackendProcessStarter processStarter = _startProcess;

  /// 跑 `icacls` / `taskkill` 的实现，测试里替换掉（不然用例会真的去拉进程）
  @visibleForTesting
  static ToolRunner? toolRunnerOverride;

  /// 杀 pid 的实现，测试里替换掉（不然用例会真的去杀系统里那个 pid）
  @visibleForTesting
  static Future<bool> Function(int pid)? killPidOverride;

  /// 测试里把「等后端就绪」的 90 秒截短；null = 走真实值
  @visibleForTesting
  static Duration? waitDeadlineOverride;

  static const int _maxLogLines = 200;

  /// 退出时最多等后端多久：超时就让它在后台收尾，**关窗口绝不能卡住**
  static const Duration exitReleaseTimeout = Duration(seconds: 8);

  /// 等后端就绪的上限。比脚本时代（90 秒）短得多：不再有 pwsh 冷启动 / cargo 构建，
  /// 后端从进程起来到监听端口实测在 1 秒内；MCP 预热是后台的，不挡端口。
  static const Duration readyTimeout = Duration(seconds: 60);

  bool get busy =>
      phase == StartupPhase.probing ||
      phase == StartupPhase.starting ||
      phase == StartupPhase.waiting;

  bool get isReady =>
      phase == StartupPhase.ready || phase == StartupPhase.skipped;

  /// 给后端的额外环境变量。端口文件与工具权限的基准是 `COMFYHUB_PROJECT_ROOT`；
  /// 开了「允许手机 / 局域网设备连接」时再带上 `COMFYHUB_ALLOW_REMOTE`
  /// （不开时后端只听 127.0.0.1，真机连不上 —— 2026-09-28 用户 bug）。
  Map<String, String>? _childEnvironment(String root) {
    final env = <String, String>{'COMFYHUB_PROJECT_ROOT': root};
    if (_settings.allowRemoteBackend) env['COMFYHUB_ALLOW_REMOTE'] = '1';
    return env;
  }

  // -------------------------------------------------------------------------
  //  探测
  // -------------------------------------------------------------------------

  /// 探一次后端健康检查；连不上返回 null
  Future<Map<String, dynamic>?> probe({
    Duration timeout = const Duration(seconds: 3),
  }) async {
    final uri = Uri.parse('${_settings.baseUrl}/api/health');
    // 本机连"没人监听"的端口要等 SYN 重传（这台机器上实测 ~2 秒），冷启动时服务
    // 还没起来恰是常态；先花 ≤250ms 判一下端口，省掉这段白等。
    // 只对回环地址这么做，远端地址不预检（免得把慢网络误判成"没起来"）。
    if (_isLoopback(uri.host) && !await _portOpen(uri.host, uri.port)) {
      return null;
    }
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
      host == '127.0.0.1' ||
      host == 'localhost' ||
      host == '::1' ||
      host == '[::1]';

  /// 端口上有没有人在听（带短超时）
  static Future<bool> _portOpen(
    String host,
    int port, {
    int timeoutMs = 250,
  }) async {
    try {
      final s = await Socket.connect(
        host,
        port,
        timeout: Duration(milliseconds: timeoutMs),
      );
      s.destroy();
      return true;
    } catch (_) {
      return false;
    }
  }

  /// **首屏拉数据的闸门**：等"这一轮启动/探测有没有结论"（就绪 / 失败 / 跳过）。
  ///
  /// # 为什么必须有它
  ///
  /// 冷启动时 `<根>\.run\api.json` **还不存在**（后端优雅退出时会把它删掉），
  /// 于是 `SettingsStore.baseUrl` 只能是 `http://127.0.0.1:8080` 那个兜底值 ——
  /// 那一刻发出去的任何请求都**注定失败**，而失败的样子还很吓人：
  /// `ClientException … Connection attempt cancelled, host: 127.0.0.1, port: 8080`
  /// （2026-10-12 用户报的"刚启动 AI 工作台就显示这个"）。
  ///
  /// 更阴的是**半截失败**：后端绑好端口后我们一 `notifyListeners`，store 就按新地址
  /// 把旧 client `dispose()` 掉，那个还在连 8080 的请求于是被"取消"（而不是"拒绝"）——
  /// 所以消息里写的是 cancelled。等出结论再发，这一整类现象就都没有了。
  ///
  /// # 三条纪律
  ///
  ///  1. **只等"有没有结论"，不保证结论是好的** —— 后端起不来时照样放行，
  ///     让各页面如实报"连不上"（不许替用户静默重试到底）；
  ///  2. **不许把首屏卡死**：超时就放行（那时请求失败是如实的结果）；
  ///  3. **它不阻塞用户动作** —— 只有"冷启动第一波"该等（`AiWorkspaceStore.load` /
  ///     `NsfwStore.load`），用户点出来的动作照旧直接发。
  Future<void> settled({Duration? timeout}) async {
    if (!busy) return;
    final completer = Completer<void>();
    void check() {
      if (!busy && !completer.isCompleted) completer.complete();
    }

    addListener(check);
    check(); // 加监听与首次判断之间可能刚好翻了档，补一次
    try {
      await completer.future.timeout(
        timeout ?? readyTimeout + const Duration(seconds: 5),
      );
    } on TimeoutException {
      _log('等后端启动结论超时，先按现状继续（该报的错照样会报）。');
    } finally {
      removeListener(check);
    }
  }

  /// 从可执行文件所在目录往上找运行根目录（判据与两种布局见 `backend_paths.dart`）。
  ///
  /// 保留这个静态入口：设置页的「项目根目录」输入框与提示都用它。
  static String? detectProjectRoot({String? from}) => detectRoot(from: from);

  /// 算一次数据目录结论 —— **唯一入口**：启动、起后端、设置页都走它。
  ///
  /// 判据只有 `resolveDataDir` 一份，"上次用的是哪份库"从偏好里取（身份，不是路径）。
  DataDirResolution _computeDataDir(String root) {
    final known = _settings.lastLibrary;
    return resolveDataDir(
      root,
      knownLibraryId: known?.id,
      knownLibraryPath: known?.path,
    );
  }

  /// 重新算一次并通知界面（改过指针 / 切换过库之后调）。
  DataDirResolution? refreshDataDirInfo() {
    final root = layout?.root;
    if (root == null) return null;
    final info = _computeDataDir(root);
    dataDirInfo = info;
    notifyListeners();
    return info;
  }

  /// 把"这次真的用上了哪份库"记进偏好。
  ///
  /// 只认**真有库**的目录：`snapshotLibrary` 顺手给那份库补上身份文件
  /// （2026-11-10：从此"这次打开的是不是你的库"是可以问出来的，不是猜的）。
  Future<void> _rememberLibrary(DataDirResolution info) async {
    final ref = snapshotLibrary(info.dataDir);
    if (ref == null) return;
    await _settings.setLastLibrary(ref);
  }

  /// 把数据目录切到 [dir] 并重启后端 —— **只改指针、一个文件都不搬**。
  ///
  /// 数据本来就在那儿，缺的只是"让程序下次打开它"。2026-11-09 那次"搬来搬去被挡
  /// 回来"的死锁，就是因为以前只有"整份迁移"这一条路。
  ///
  /// 返回 false = 指针没写进去（目录只读之类），调用方要如实说。
  Future<bool> switchDataDir(String dir) async {
    final root = layout?.root;
    if (root == null) return false;
    if (!writeDataDirOverride(root, dir)) return false;
    // 换了库就得重起后端（它拿着 --db 参数，不会自己换）
    await ensureRunning(force: true);
    return true;
  }

  /// 解析一次布局并记进 [layout] / [serverExe]。
  bool _resolve({bool log = true}) {
    final resolved = resolveLayout(projectRoot: _settings.projectRoot);
    if (resolved == null) {
      if (log) _log('定位不到运行根目录（这台机器上既没有源码树，也没有装配好的 release 产物）。');
      return false;
    }
    layout = resolved;
    serverExe = resolved.exe;
    serverExeSource = resolved.exeSource;
    // 数据目录的结论只在这里算一次（**唯一入口**：设置页也读这份，别自己再算一遍）：
    // 打开的是不是上次那份库、产物目录在不在，都记下来给界面用。
    final info = _computeDataDir(resolved.root);
    dataDirInfo = info;
    unawaited(_rememberLibrary(info));
    if (log) {
      _log('运行根目录: ${resolved.root}');
      if (info.dataDir != resolved.root) _log('数据目录:   ${info.dataDir}');
      if (info.pick == DataDirPick.remembered) {
        _log('按"上次用的库"打开：${info.dataDir}（指针写的是 ${info.chosen}）');
      }
      if (info.otherCandidates.isNotEmpty) {
        // 只是报告：同一棵树里还有别的库。换哪份由用户决定（设置 → 数据目录）
        _log(
          '还发现别的库：${info.otherCandidates.map((c) => '${c.path}'
              '（${sizeText(c.bytes)}）').join('、')}',
        );
      }
      if (info.pointerHasOtherLibrary) {
        _log(
          '指针那里是另一份库（${info.dataDir}），和你上次用的 '
          '${info.knownLibraryPath} 不是同一份。',
        );
      }
      if (info.usedLibraryId != null) _log('库身份:     ${info.usedLibraryId}');
      if (info.pointerMissing) {
        _log('data-dir.json 指的 ${info.pointer} 不在了，按运行根目录处理。');
      }
      if (info.storageMissing) {
        _log('产物目录不在：${info.storageDir}（只复制了程序、没复制 storage？）');
      }
      final storage = info.storageDir;
      if (storage != dataStorageDirOf(info.dataDir)) {
        _log('产物目录:   $storage（按实际布局识别，不是标准的 storage 子目录）');
      }
      if (resolved.exe != null) {
        _log('后端程序:   ${resolved.exe}（来源：${resolved.exeSource}）');
      } else {
        _log('后端程序:   没找到 $kServerExeName');
      }
    }
    return true;
  }

  String _missingExeMessage() =>
      '没找到后端程序 $kServerExeName。'
      '发布包或 Release 产物里，它应该跟 viewer.exe 同级放在 backend\\ 里，'
      'release 构建时会由 windows\\CMakeLists.txt 自动装配（见 README「构建」一节）。'
      '源码树里可以先运行 cargo build --release --manifest-path server-rs\\Cargo.toml。'
      '已有别处的构建产物时，设环境变量 $kServerExeEnv 指过去，'
      '把后端映像挪出受限目录也是用这个开关。';

  // -------------------------------------------------------------------------
  //  启动
  // -------------------------------------------------------------------------

  /// 确保后端可用。返回 true 表示最终健康。
  Future<bool> ensureRunning({bool force = false}) async {
    if (busy) return false;
    _cancelled = false;
    lastError = null;

    // ⚠ **先翻成"忙"，再做第一个 await**（2026-10-12）：首屏各页的 postFrame 回调与
    // 我们在**同一帧**里跑，而这里第一件事（读端口文件）是异步的 —— 中间那段空窗
    // 会被 [settled] 的调用方当成"没人起后端、可以发请求了"，于是它们拿着 8080 那个
    // 兜底地址发出去，冷启动**必然**报一次"连接后端失败"（用户贴的那个
    // `ClientException … 127.0.0.1:8080/api/ai/providers`）。
    _set(StartupPhase.probing, '正在检查本地服务…');

    // 后端端口是自动挑的：App 开着的时候后端可能重启到**新端口**，
    // 探活前先按 .run\api.json 重新发现一次（用户手填过地址时它什么也不做）。
    await _settings.refreshBaseUrlFromPortFile();
    _set(StartupPhase.probing, '正在检查本地服务 ${_settings.baseUrl} …');
    final health = await probe();

    if (!force && health != null && health['database'] == 'ok') {
      _log('后端已在运行（版本 ${health['version']}，数据库 ok）');
      // 探到已经在跑 = 这次没起过进程，但要**认领**它：
      // 「关 App 时一并停止本地服务」这句承诺包含"启动 App 之前就已经在跑的"那个。
      _claimRunningBackend();
      // 顺手把它的端口记下来：下次我们启动时就能要回同一个（这是"手机不用重新配"的
      // 前提），而且它要是当初回落过，这里也要如实说出来。
      await _recordActualPort(null, spawned: false);
      _set(StartupPhase.ready, '后端已就绪');
      return true;
    }
    if (health != null) {
      _log('后端进程在跑，但数据库状态是 ${health['database']} —— 先把它停掉再重新起一个。');
    } else {
      _log('后端未响应，准备启动。');
    }

    if (kIsWeb || !Platform.isWindows) {
      return _fail('当前平台不支持自动启动本地服务（仅 Windows 桌面端）。');
    }

    if (!_resolve()) {
      return _fail('定位不到运行根目录。请在「设置 → 本地服务」里指定项目根目录。');
    }
    if (serverExe == null) return _fail(_missingExeMessage());

    // 库里那半死不活的那个先送走（它是我们或上一次运行留下的，端口文件里有它的 pid）
    if (health != null) await _stopUnhealthy();

    return _spawnAndWait();
  }

  /// 后端进程在、但数据库不 ok：把端口文件里记着的那个 pid 收掉再重起。
  ///
  /// 这一步**不看 `_claimedPids`**（那时还没认领任何东西），判据只有一条：
  /// "`.run\api.json` 里那个 pid 还活着、而且它占着的端口回的 health 是坏的"。
  Future<void> _stopUnhealthy() async {
    final pid = _portFilePid();
    if (pid == null || pid <= 0) return;
    if (await _killPid(pid)) {
      _log('已停掉状态异常的后端（pid $pid）');
      // 让它把端口放掉再起新的（最多等 3 秒）
      for (var i = 0; i < 15; i++) {
        if (await probe(timeout: const Duration(milliseconds: 300)) == null) {
          break;
        }
        await Future.delayed(const Duration(milliseconds: 200));
      }
    }
  }

  /// 起后端并等它健康。
  Future<bool> _spawnAndWait() async {
    final root = layout!.root;
    // 数据目录（库 + storage）可以搬出程序目录；`.run` / `backups` / `comfyui`
    // 仍按运行根目录算 —— 端口文件的位置不能跟着数据走，脚本与捕获扩展都读它。
    //
    // `_resolve()` 就在起后端之前跑过，结论直接复用 —— 别在这里再算一遍：再算会把
    // "第一次认下这份库"变成"按上次那份库打开"，提示文案就自相矛盾了。
    final info = dataDirInfo ?? _computeDataDir(root);
    dataDirInfo = info;
    // 记下"这次用的是这份库"（后端起来之后库才存在，所以这里也要记一次）
    unawaited(_rememberLibrary(info));
    final data = info.dataDir;
    final exe = serverExe!;

    // 产物目录按**磁盘上的实际布局**认：标准是 `<数据目录>\storage`，
    // 但也可能是旧版套出来的 `storage\storage`，或用户手动挪成的扁平布局
    // （产物直接在数据目录里）。别在这里硬拼 `storage`，否则界面能看到库、
    // 却读不到任何一张图（2026-11-09 用户报的"软件不认这个目录的内容"）。
    final storage = info.storageDir;

    // 目录先备好：后端会自己建库目录，但 `.run` / 产物目录缺失会让它多打几句警告
    for (final dir in [runDirOf(root), storage, dataDbParentOf(data)]) {
      try {
        Directory(dir).createSync(recursive: true);
      } catch (_) {}
    }

    await _normalizeIntegrity(exe);

    final dbFile = info.dbFile;
    final bindHost = _settings.allowRemoteBackend ? '0.0.0.0' : '127.0.0.1';
    // **先要回上一次那个端口**（2026-10-13）：`--port 0` 让系统挑，代价是重启就换
    // 端口，而**手机存的是 `http://<局域网IP>:<端口>`** —— 端口一变那条地址就废了，
    // 用户得重新扫一次配对二维码。要回旧端口之后，"端口变了"就只剩
    // "它确实被别人占了"这一种情况（那时后端自己回落到随机端口并记 `fellBack`）。
    //
    // ⚠ 要回失败**不影响启动**：`main.rs` 绑不上就自动回落（且不会退出），
    // 所以这条优化不会把"起不来"引进来（那正是当初选 `--port 0` 的理由）。
    final preferredPort = _settings.lastApiPort;
    final args = <String>[
      '--db', dbFile,
      '--storage', storage,
      '--host', bindHost,
      // 端口只有一个真源：真实端口由后端写进 .run\api.json，我们只读它
      '--port', '${preferredPort ?? 0}',
      // 强杀 App 时不留孤儿：见类注释与 server-rs/src/main.rs 的 spawn_stdin_watchdog
      '--watch-stdin',
    ];

    _set(StartupPhase.starting, '正在启动后端…');
    _log('$exe ${args.join(' ')}');
    _log('       工作目录 $root；后端来源：$serverExeSource');

    Process proc;
    try {
      proc = await processStarter(
        exe,
        args,
        workingDirectory: root,
        environment: _childEnvironment(root),
      );
    } catch (e) {
      return _fail('启动后端失败: $e');
    }
    // ⚠ 这个引用要一直留着（stdin 的写端挂在它上面，GC 掉后端就会自己退）
    _process = proc;
    _claim(proc.pid);

    // 输出回显。allowMalformed：后端偶尔会漏出非 UTF-8 字节（比如 Windows 的
    // 本地化错误串），不该让整条日志流报 FormatException。
    proc.stdout
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen(_log, onError: (Object e) => _log('[stdout 错误] $e'));
    proc.stderr
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter())
        .listen(
          (l) => _log(l.isEmpty ? '' : l),
          onError: (Object e) => _log('[stderr 错误] $e'),
        );

    // 进程提前退出要能当场看见（不再像脚本时代那样白等满 90 秒）
    var exited = false;
    int? exitCode;
    unawaited(
      proc.exitCode
          .then((code) {
            exited = true;
            exitCode = code;
            _log('[后端已退出] 退出码 $code');
            _notify();
          })
          .catchError((Object _) {}),
    );

    _set(StartupPhase.waiting, '等待后端就绪…');
    final deadline = DateTime.now().add(waitDeadlineOverride ?? readyTimeout);
    var attempt = 0;
    while (DateTime.now().isBefore(deadline)) {
      attempt++;
      // ⚠️ 每轮都重读端口文件：后端绑好端口后自己写它，而我们探活用的地址
      // 可能还是启动时读到的那个（用户手填过地址时这一步什么也不做）。
      if (await _settings.refreshBaseUrlFromPortFile()) {
        _log('后端换了端口，改探 ${_settings.baseUrl}（读自 .run\\api.json）');
      }
      final h = await probe(timeout: const Duration(seconds: 2));
      if (h != null && h['database'] == 'ok') {
        _log('后端已就绪：版本 ${h['version']}，存储目录 ${h['storageDir']}');
        servicesClaimed = true;
        // 把这次实际用的端口记下来（下次先要回它），并如实报告"没要回来"
        await _recordActualPort(root);
        _set(StartupPhase.ready, '后端已就绪');
        return true;
      }
      if (_cancelled) return _fail('已取消启动');
      if (exited) {
        return _fail(
          '后端启动失败，进程已退出（退出码 $exitCode）。日志见上。'
          '常见原因可能是数据库文件被别的进程占着、端口绑不上，或 exe 与源码不匹配。',
        );
      }
      if (attempt % 10 == 0) {
        _log(
          '仍在等待…（${attempt}s）'
          '${h == null ? "后端未响应" : "数据库 ${h['database']}"}',
        );
      }
      await Future.delayed(const Duration(milliseconds: 1000));
    }

    return _fail(
      '等待后端超时（${(waitDeadlineOverride ?? readyTimeout).inSeconds} 秒）。'
      '后端日志见上方输出，也可以在「设置 → 本地服务」里点「停止」再点「启动 / 修复服务」。',
    );
  }

  /// 停下"我们认领过的"后端（自己起的 + 接管来的），不动别人的。
  Future<bool> _stopClaimed({bool log = false}) async {
    var killed = false;
    final pids = _claimedPids.toList();
    for (final pid in pids) {
      if (await _killPid(pid)) killed = true;
    }
    // 句柄还在手上时优先用它（进程已经退出时 kill 返回 false，不算错）
    final proc = _process;
    if (proc != null) {
      try {
        if (proc.kill()) killed = true;
      } catch (_) {}
    }
    _claimedPids.clear();
    _process = null;
    if (log && killed) _log('已停掉后端进程（pid ${pids.join('、')}）');
    return killed;
  }

  /// 停止本地服务（用户点「停止」/「重启」时走这条）。
  ///
  /// 与退出收尾的区别：这里是**明确的用户意图**，所以除了认领过的那个，
  /// 还会把端口文件里记着的、以及按映像名找到的后端一起收掉
  /// （`server.ps1 stop` 以前就是这么干的）。
  Future<bool> stopBackend() async {
    if (busy) return false;
    _set(StartupPhase.starting, '正在停止后端…');
    var killed = await _stopClaimed(log: true);

    final pid = _portFilePid();
    if (pid != null && pid > 0 && await _killPid(pid)) killed = true;

    if (!killed) {
      final byName = await _taskkillByName();
      killed = byName;
    }

    servicesClaimed = false;
    if (killed) {
      // 让收尾跑完：后端要删端口记录、写完 WAL 再退出（最多等 5 秒）
      for (var i = 0; i < 25; i++) {
        if (await probe(timeout: const Duration(milliseconds: 300)) == null) {
          break;
        }
        await Future.delayed(const Duration(milliseconds: 200));
      }
      _set(StartupPhase.idle, '本地服务已停止');
    } else {
      _set(StartupPhase.idle, '本地服务本来就没在跑');
    }
    return killed;
  }

  /// 停止 / 重启（设置页按钮的共同出口）
  Future<void> runAction(String action) async {
    if (busy) return;
    switch (action) {
      case 'down':
      case 'stop':
        await stopBackend();
        return;
      case 'restart':
        await stopBackend();
        await ensureRunning(force: true);
        return;
      default:
        _fail('不认识的本地服务动作：$action');
    }
  }

  // -------------------------------------------------------------------------
  //  退出收尾：「谁起的谁关」
  // -------------------------------------------------------------------------

  /// 关 App 时把**本地服务**停掉（只停我们认领过的那些）。
  ///
  /// 三条纪律：
  ///   · 设置里关掉「关闭 App 时一并停止本地服务」→ 什么都不做（服务常驻）；
  ///   · 这次没起过 / 没接管过 → 什么都不做（别人终端里起的后端不归我们管）；
  ///   · 最多等 [timeout] 就返回 —— **关窗口绝不能卡住**，剩下的收尾后端自己会跑完
  ///     （它还握着 `--watch-stdin` 那根管道：我们一退出，它照样会优雅退出）。
  Future<bool> releaseOnExit({Duration timeout = exitReleaseTimeout}) async {
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

    _log('关闭 App：正在停掉本次启动的本地服务…');
    // ⚠ 先把句柄取出来：`_stopClaimed` 会把 `_process` 置空，之后再读就晚了
    final proc = _process;
    final stopped = await _stopClaimed();
    servicesClaimed = false;

    if (proc != null) {
      try {
        final code = await proc.exitCode.timeout(timeout);
        _log('本地服务已停止（后端退出码 $code）');
      } on TimeoutException {
        _log(
          '后端 ${timeout.inSeconds}s 内还没退干净，放它在后台收尾'
          '（stdin 一关它也会自己退，不影响关闭）。',
        );
      } catch (e) {
        _log('后端退出结果未知（$e），已放它在后台跑完。');
      }
    }
    return stopped;
  }

  /// 用户把「关闭 App 时一并停止本地服务」**关掉**时调用：撤销认领。
  ///
  /// 不需要像以前那样再去撤一个 `watch-owner` 守护进程 —— 那份守护已经由
  /// `--watch-stdin`（后端自己盯着这根管道）取代了。
  Future<bool> disarm() async {
    if (!servicesClaimed) return false;
    servicesClaimed = false;
    _claimedPids.clear();
    _log('用户关掉了「关闭 App 时一并停止本地服务」：退出时不再停服务。');
    return true;
  }

  // -------------------------------------------------------------------------
  //  认领 / 杀进程 / 权限标签
  // -------------------------------------------------------------------------

  /// 记下"这个 pid 是我们的人"。
  void _claim(int? pid) {
    if (pid != null && pid > 0) {
      _claimedPids.add(pid);
      servicesClaimed = true;
    }
  }

  /// 探到"后端已经在跑"时把它认领下来：pid 取自后端自己写的 `.run\api.json`。
  void _claimRunningBackend() {
    final pid = _portFilePid();
    if (pid != null && pid > 0) {
      _claim(pid);
      _log('已认领正在运行的后端（pid $pid）：关 App 时会按设置把它一并停掉。');
    } else {
      _log('提示：端口记录里没有后端 pid（.run\\api.json 缺 pid），退出时不会主动停它。');
    }
  }

  int? _portFilePid() {
    final state = readPortFile(_portFileRoot() ?? '');
    final pid = state?['pid'];
    return pid is int ? pid : null;
  }

  /// 去哪个根目录找 `.run\api.json`（解析出来的布局优先，其次按偏好 / 真实现场探）。
  String? _portFileRoot() =>
      layout?.root ?? detectRoot(from: _settings.projectRoot);

  /// 把**这次实际**监听的端口记进偏好（下次启动先要回它），并如实报告"没要回来"。
  ///
  /// 三条纪律：
  ///  1. **只认 `.run\api.json`**（后端自己写的真源）—— 读不到就什么都不做，
  ///     绝不按"我们请求的端口"猜一个记下来；
  ///  2. **端口没变时一个字都不写**（[SettingsStore.setLastApiPort] 内部会 early return，
  ///     这里再判一次是为了连通知都不发）；
  ///  3. **回落必须说出来**：这是"手机连不上"的唯一线索，不能只躺在日志里
  ///     （[portFallbackFrom] + 设置页那行提示）。
  Future<void> _recordActualPort(String? root, {bool spawned = true}) async {
    final resolved = root ?? _portFileRoot();
    if (resolved == null) return;
    final state = readPortFile(resolved);
    final port = state?['port'];
    if (port is! int || port <= 0) return;

    final requested = state?['requestedPort'];
    final wanted = (requested is int && requested > 0) ? requested : null;
    portFallbackFrom = (state?['fellBack'] == true) ? wanted : null;
    await _settings.setLastApiPort(port);

    if (portFallbackFrom != null) {
      _log(
        '⚠ 端口 $portFallbackFrom 这次没能用上（被别的程序占了，或落在系统保留段），'
        '后端改听 $port。手机上保存的地址会失效，'
        '请在「设置 → 远程访问与配对」里重新扫一次二维码。',
      );
    } else if (spawned && wanted != null) {
      _log('端口沿用上次的 $port（手机存的地址不用改）。');
    }
  }

  static Future<bool> _killPid(int pid) async {
    if (pid <= 0) return false;
    final over = killPidOverride;
    if (over != null) return over(pid);
    try {
      return Process.killPid(pid);
    } catch (_) {
      return false; // 进程已经没了 / 没权限：都不算异常
    }
  }

  /// 按映像名收后端（用户点「停止」时的兜底：端口文件可能缺 pid 或指向了旧进程）。
  Future<bool> _taskkillByName() async {
    _log('按映像名收一遍 $kServerExeName …');
    final result = await _runTool('taskkill', ['/F', '/IM', kServerExeName]);
    if (result == null) return false;
    // taskkill 找不到进程时回 128（"没有找到"），那不是错误
    return result.exitCode == 0;
  }

  static Future<ProcessResult?> _runTool(
    String executable,
    List<String> arguments,
  ) async {
    try {
      // Process.run 同样走 CREATE_NO_WINDOW（见类注释），不会闪黑框
      return await Process.run(executable, arguments);
    } catch (_) {
      return null;
    }
  }

  Future<ProcessResult?> _tool(String exe, List<String> args) =>
      (toolRunnerOverride ?? _runTool)(exe, args);

  /// 把后端 exe 的**完整性标签**设回 `Medium`（best-effort，失败只记日志）。
  ///
  /// # 为什么要这一步（2026-09-30 用户实测的两条事实）
  ///
  /// 1. **写限制是"完整性级别（IL）"判的，不是路径 / 用户 / 会话**：清掉 exe 上的
  ///    `Low` 标签，后端立刻就能写 ComfyUI 目录（`%TEMP%`、用户自己的安装目录同理）。
  /// 2. **Windows 取 `min(父进程, 映像)`**：链路上**任何一环**是 Low，子进程照样掉回 Low
  ///    —— 而"在工作区里构建出来的产物普遍带着 Low 标签"，`viewer.exe` 也不例外。
  ///
  /// 所以这一步只能救一半，而且救的正是常见的那一半：**App 自己是 Medium
  /// （用户双击、从普通终端起），只是后端的 exe 上还留着构建时带的 Low 标签**。
  /// 两种情况下它一定救不了，也只能如实记一句：
  ///   · App 自己就是 Low（`viewer.exe` 也带着 Low 标签）—— Low 改不动向上的标签；
  ///   · 没有 `icacls`（非 Windows / 精简系统）。
  /// 真正的判断在**后端自己**的写探针上（`GET /api/system/confinement`，界面上有横幅），
  /// 这里只做"顺手把标签归一"。
  Future<void> _normalizeIntegrity(String exe) async {
    if (kIsWeb || !Platform.isWindows) return;
    final result = await _tool('icacls', [exe, '/setintegritylevel', 'Medium']);
    if (result == null) {
      _log('[权限] 没能执行 icacls（跳过完整性标签归一，后端照样会起）。');
      return;
    }
    final lines = result.stdout.toString().trim().split('\n');
    final first = lines.isEmpty ? '' : lines.first.trim();
    if (result.exitCode == 0) {
      _log(
        '[权限] 已把后端 exe 的完整性标签设回 Medium（Low 标签是"写不进工作区外"的根因，'
        '判据见 GET /api/system/confinement）。',
      );
    } else {
      _log(
        '[权限] 设完整性标签失败（icacls 退出码 ${result.exitCode}）$first —— '
        'App 自己多半也跑在 Low 上（链路上任何一环是 Low，子进程就掉回 Low）。'
        '那种情况只能把整个产物拷到工作区外，或在不受限的终端里重新构建一次。',
      );
    }
  }

  // -------------------------------------------------------------------------

  /// 用户在启动页点「先跳过」——不阻塞使用（离线只看本地缓存/稍后再连）
  void skip() {
    _cancelled = true;
    _set(StartupPhase.skipped, '已跳过自动启动');
  }

  void cancel() {
    _cancelled = true;
    unawaited(_stopClaimed());
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

  /// **测试专用**：把状态直接推到某一档（不碰进程）。
  ///
  /// 为什么需要它：2026-10-11 起 `HomeShell` 是最高级、加载页只是覆盖物，
  /// 而"后端重启时壳子不许被换掉"这条纪律**只能靠模拟 isReady 变假来验证** ——
  /// 真去 kill 一个后端进程在单测里做不到。生产代码一行都不用它。
  @visibleForTesting
  void debugSetPhase(StartupPhase p, {String message = ''}) => _set(p, message);

  /// 退出收尾是异步的，可能落在 dispose 之后 —— 那时再 notifyListeners 会抛
  /// 「was used after being disposed」，会把 App 的关闭流程搅乱，所以这里兜一下。
  void _notify() {
    if (_disposed) return;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    // 兜底：窗口关闭那刀（`AppLifecycleListener.onExitRequested`）没走成时，
    // 至少别把这个后端留成孤儿。设了"保留服务"就什么都不做。
    if (servicesClaimed && _settings.stopServicesOnExit) {
      unawaited(_stopClaimed());
    }
    super.dispose();
  }
}
