import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'backend_paths.dart';

/// 全局设置（后端地址、自动启动、项目根目录等），持久化在本地。
class SettingsStore extends ChangeNotifier {
  static const _kBaseUrl = 'comfyhub.baseUrl';
  static const _kBaseUrlHistory = 'comfyhub.baseUrlHistory';
  static const _kPageSize = 'comfyhub.pageSize';
  static const _kAutoStartBackend = 'comfyhub.autoStartBackend';
  static const _kStopServicesOnExit = 'comfyhub.stopServicesOnExit';
  static const _kAllowRemoteBackend = 'comfyhub.allowRemoteBackend';
  static const _kProjectRoot = 'comfyhub.projectRoot';

  String _baseUrl = defaultBaseUrl();
  int _pageSize = 24;
  bool _autoStartBackend = true;
  bool _stopServicesOnExit = true;
  bool _allowRemoteBackend = false;
  String? _projectRoot;
  bool _loaded = false;

  /// 用过哪些地址（最近用的在前，最多 [maxBaseUrlHistory] 条）。
  ///
  /// 为什么要有它：跨端用户至少有两条路要来回切 —— 模拟器/同机（回环）与
  /// 真机（局域网 IP / Tailscale）。以前设置页只有**一个文本框**，
  /// 换一次就得**手抄一遍 IP**，抄错一个数字就是"连不上"，而上一轮用过的地址
  /// 明明就在几分钟前。所以历史是"选择"这件事的**事实来源**（不是猜的候选）。
  List<String> _baseUrlHistory = const [];

  /// 地址是用户手填的（true）还是我们自动发现的（false，见 [baseUrlIsAuto]）。
  bool _baseUrlFromUser = false;

  String get baseUrl => _baseUrl;
  int get pageSize => _pageSize;

  /// 历史地址（最近用的在前）。界面拿它渲染"最近用过"那一组候选。
  List<String> get baseUrlHistory => List.unmodifiable(_baseUrlHistory);

  /// 历史最多留几条（再多就成了"翻半天找不到"，而不是"顺手点一下"）。
  static const int maxBaseUrlHistory = 6;

  /// 当前地址是不是"用户手填的"（false = 我们按本机 `.run\api.json` 自动定的）。
  ///
  /// 用途只有一个（2026-09-22）：后端**重启会换端口**（端口是自动挑的），
  /// 那时 App 必须能自己重新发现并改口 —— 但**绝不能**覆盖用户在设置页里填的地址
  /// （那是"我要连别的机器"，改它就是把人家的远程配置弄坏了）。
  bool get baseUrlIsAuto => !_baseUrlFromUser;

  /// App 启动时是否自动把后端拉起来
  bool get autoStartBackend => _autoStartBackend;

  /// 关闭 App 时是否把**这次由 App 启动的**本地服务一并停掉（谁起的谁关）。
  /// 关掉的话服务会常驻，下次开 App 就是热启动。
  bool get stopServicesOnExit => _stopServicesOnExit;

  /// 允许手机 / 局域网设备连接本机后端（`COMFYHUB_ALLOW_REMOTE`）。
  ///
  /// 默认关：后端只听回环。开着时后端听所有网卡 —— 真机连桌面才有得连
  /// （2026-09-28 用户 bug「手机连接桌面端失败，报 timed out」的根因就是后端
  /// 只听 127.0.0.1）。远程请求由配对闸门把关（回环放行，局域网要配对令牌）。
  bool get allowRemoteBackend => _allowRemoteBackend;

  /// 运行根目录（放 `.db\` / `storage\` / `.run\` 的那个目录）。为空表示自动探测。
  ///
  /// 判据见 `backend_paths.dart` 的 [looksLikeRoot]：源码树（`server-rs\Cargo.toml`）
  /// 与 release 产物（`viewer.exe` 同级带 `backend\`）两种布局都认。
  String? get projectRoot => _projectRoot;

  bool get loaded => _loaded;

  /// 测试用：覆盖"去哪找 `.run\api.json`"。
  ///
  /// `null`（生产默认）= 按 [_projectRootCandidates] 真实探测；
  /// 空列表 = 假装这台机器上没有端口记录（用例期望"回落到 8080"时用这个）；
  /// 给了目录 = 只在这些目录里找（用例想喂一份假的端口文件时用）。
  @visibleForTesting
  static List<String>? portFileRootsOverride;

  static String defaultBaseUrl() {
    if (kIsWeb) return 'http://127.0.0.1:8080';
    try {
      // Android 模拟器访问宿主机需要用 10.0.2.2。
      // 刻意**不**去读本机的 `.run\api.json`：那台机器上的后端跟手机是两台设备，
      // 手机连的是"宿主机的某个端口"，由用户在设置页里填（远程场景见
      // docs/inbound-auth-design.md）。
      if (Platform.isAndroid) return 'http://10.0.2.2:8080';
    } catch (_) {
      // 平台判断在个别环境下会抛异常，忽略即可
    }
    // 桌面端：后端端口现在是**自动挑的**（2026-09-22，用户要求"不再显式指定端口"），
    // 真实端口记在 `<项目根>\.run\api.json` 里，前端必须去读它 —— 否则每次启动
    // 都在猜一个固定端口，而后端可能正跑在别的端口上。
    return _baseUrlFromPortFile() ?? 'http://127.0.0.1:8080';
  }

  /// 从 `<项目根>\.run\api.json` 读后端**实际**监听的端口（读不到返回 null）。
  ///
  /// 这是端口这件事在 Dart 侧的唯一入口，与后端 `api_port_file.rs`、
  /// `comfyui\comfyhub_capture\capture_core.py` 读的是**同一个文件**。
  ///
  /// 三条纪律：
  ///  · **读不到就用默认值**，绝不抛异常 —— 它在 App 启动路径上，崩了就是白屏；
  ///  · **只在"用户没手填地址"时才用**（[load] 里 `prefs` 有值就优先用用户的），
  ///    否则远程 / 局域网场景会被拽回本机；
  ///  · 用完**不缓存**：端口会随重启变，每次冷启动都重新读一次。
  static String? _baseUrlFromPortFile({String? projectRoot}) {
    for (final root in _projectRootCandidates(projectRoot)) {
      final state = readPortFile(root);
      final port = state?['port'];
      if (port is! int || port <= 0 || port > 65535) continue;
      final host = (state?['host'] as String?) ?? '127.0.0.1';
      // `COMFYHUB_ALLOW_REMOTE=1` 时后端听的是所有网卡，而客户端连 0.0.0.0 是不通的 → 回落回环
      final connectHost = (host.isEmpty || host == '0.0.0.0' || host == '::') ? '127.0.0.1' : host;
      return 'http://$connectHost:$port';
    }
    return null;
  }

  /// 去哪找 `.run\api.json`：显式给的根 → 环境变量 → 从 exe / cwd 往上找运行根目录。
  ///
  /// **判据与 [BackendLauncher] 完全同源**（`backend_paths.dart` 的 [detectRoot]）——
  /// 2026-09-30 之前这里是自己再写一遍"往上找 `scripts\comfyhub.ps1`"，两处一旦漂移
  /// 就会出现"App 起了后端、却读不到它写的端口文件"。
  ///
  /// **测试里必须能关掉它**（见 [portFileRootsOverride]）：否则用例的结果会随
  /// "这台机器上后端正跑在哪个端口"变化（实测：`context_menu_test` 复制出的 URL
  /// 变成了当时后端占用的随机端口，而不是用例期望的 8080）。
  static List<String> _projectRootCandidates([String? explicit]) {
    final override = portFileRootsOverride;
    if (override != null) return override;
    final roots = <String>[];
    if (explicit != null && explicit.isNotEmpty) roots.add(explicit);
    final env = Platform.environment['COMFYHUB_PROJECT_ROOT'];
    if (env != null && env.isNotEmpty) roots.add(env);
    final detected = detectRoot();
    if (detected != null) roots.add(detected);
    return roots;
  }

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    final stored = prefs.getString(_kBaseUrl);
    _baseUrlFromUser = stored != null && stored.isNotEmpty;
    _baseUrl = _baseUrlFromUser ? stored! : defaultBaseUrl();
    _pageSize = prefs.getInt(_kPageSize) ?? 24;
    _autoStartBackend = prefs.getBool(_kAutoStartBackend) ?? true;
    _stopServicesOnExit = prefs.getBool(_kStopServicesOnExit) ?? true;
    _allowRemoteBackend = prefs.getBool(_kAllowRemoteBackend) ?? false;
    _projectRoot = prefs.getString(_kProjectRoot);
    _baseUrlHistory = _sanitizeHistory(prefs.getStringList(_kBaseUrlHistory));
    _loaded = true;
    notifyListeners();
  }

  /// 去重（保序）、丢掉空串与非 http 的脏值、截到上限。
  ///
  /// 为什么在**读**的时候也要洗一遍：这份表是上一版程序写进去的（或者用户
  /// 手改过 SharedPreferences），界面直接渲染脏值就会出现一个点不动的候选。
  static List<String> _sanitizeHistory(List<String>? raw) {
    if (raw == null) return const [];
    final out = <String>[];
    for (final item in raw) {
      final v = item.trim();
      if (v.isEmpty || !v.startsWith('http')) continue;
      if (out.contains(v)) continue;
      out.add(v);
      if (out.length >= maxBaseUrlHistory) break;
    }
    return out;
  }

  /// 这个地址在"最近用过"里的位置（-1 = 没有）。
  int baseUrlHistoryIndexOf(String url) {
    final v = url.trim();
    for (var i = 0; i < _baseUrlHistory.length; i++) {
      if (_baseUrlHistory[i] == v) return i;
    }
    return -1;
  }

  /// 从历史里去掉一条（用户点「忘记」）。
  Future<void> forgetBaseUrl(String url) async {
    final v = url.trim();
    if (!_baseUrlHistory.contains(v)) return;
    _baseUrlHistory = [..._baseUrlHistory]..remove(v);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
  }

  /// 重新发现本机后端地址（后端重启换了端口时用）。
  ///
  /// 返回 true = 地址变了（调用方要重建 api client）。
  /// **用户手填过地址时什么也不做**并返回 false —— 那是他的远程配置，不该被本机服务改掉。
  ///
  /// 调用时机：后端启动完成之后（`BackendLauncher.ensureRunning` / 重启动作收尾处）。
  /// 端口文件读不到（还没起来 / 已停止）时保持原样，绝不猜。
  Future<bool> refreshBaseUrlFromPortFile() async {
    if (_baseUrlFromUser) return false;
    final discovered = _baseUrlFromPortFile(projectRoot: _projectRoot);
    if (discovered == null || discovered == _baseUrl) return false;
    _baseUrl = discovered;
    notifyListeners();
    return true;
  }

  Future<void> setBaseUrl(String value) async {
    var v = value.trim();
    final prefs = await SharedPreferences.getInstance();
    if (v.isEmpty) {
      // 清空 = 回到"自动"：地址由 `.run\api.json` 决定（后端端口是自动挑的）
      _baseUrlFromUser = false;
      _baseUrl = defaultBaseUrl();
      notifyListeners();
      await prefs.remove(_kBaseUrl);
      return;
    }
    if (!v.startsWith('http://') && !v.startsWith('https://')) v = 'http://$v';
    while (v.endsWith('/')) {
      v = v.substring(0, v.length - 1);
    }
    _baseUrlFromUser = true;
    _baseUrl = v;
    // 顺手记住它：跨端来回切时，"上一条用过的"就是最好的候选（见 _baseUrlHistory）
    _baseUrlHistory = [v, ..._baseUrlHistory.where((e) => e != v)]
        .take(maxBaseUrlHistory)
        .toList();
    notifyListeners();
    await prefs.setString(_kBaseUrl, v);
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
  }

  Future<void> setPageSize(int value) async {
    _pageSize = value.clamp(8, 120);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kPageSize, _pageSize);
  }

  Future<void> setAutoStartBackend(bool value) async {
    _autoStartBackend = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kAutoStartBackend, value);
  }

  Future<void> setStopServicesOnExit(bool value) async {
    _stopServicesOnExit = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kStopServicesOnExit, value);
  }

  /// 允许手机 / 局域网设备连接（改完要**重启后端**才生效：监听地址起进程时定）。
  Future<void> setAllowRemoteBackend(bool value) async {
    _allowRemoteBackend = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kAllowRemoteBackend, value);
  }

  Future<void> setProjectRoot(String? value) async {
    final v = value?.trim();
    _projectRoot = (v == null || v.isEmpty) ? null : v;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    if (_projectRoot == null) {
      await prefs.remove(_kProjectRoot);
    } else {
      await prefs.setString(_kProjectRoot, _projectRoot!);
    }
  }
}
