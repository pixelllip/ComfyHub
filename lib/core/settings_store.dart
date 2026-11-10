import 'dart:convert';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'backend_paths.dart';
import 'library_identity.dart';
import 'media_cache.dart';
import 'system_accent.dart';
import 'theme.dart';

/// 画廊「集合详情」的两种形态（真源：`SettingsStore.collectionDetailWide` /
/// `collectionDetailNarrow`）。
///
/// 两个宽度档**各自一条偏好**：宽屏默认停靠、窄屏默认弹窗，形态由偏好决定，
/// 不再由代码按宽度写死。
///
/// 落盘取 [name]（`'dock'` / `'popup'`），不用 int 魔法数：偏好里躺着 `0/1`
/// 的排查成本远高于一个能直接读懂的字符串。
enum CollectionDetailStyle {
  /// 停靠面板：占**真实布局空间**，画廊网格自己收窄 —— 谁都不遮谁。
  dock('停靠面板'),

  /// 居中弹窗：浮在画廊之上、背景虚化 —— 不改变网格布局。
  popup('居中弹窗');

  /// 设置页两选一控件上的标签。
  final String label;

  const CollectionDetailStyle(this.label);

  /// 读偏好。无法识别的值（旧版本写入或手动修改）返回 [fallback]，**不抛异常** ——
  /// 它在界面构建路径上，抛异常会导致白屏（与 `AppThemeMode.parse` 约束一致）。
  static CollectionDetailStyle parse(
    String? raw, {
    required CollectionDetailStyle fallback,
  }) {
    for (final style in values) {
      if (style.name == raw) return style;
    }
    return fallback;
  }
}

/// 全局设置（后端地址、自动启动、项目根目录等），持久化在本地。
class SettingsStore extends ChangeNotifier {
  static const _kBaseUrl = 'comfyhub.baseUrl';
  static const _kBaseUrlHistory = 'comfyhub.baseUrlHistory';
  static const _kPageSize = 'comfyhub.pageSize';

  /// 筛选栏折叠状态（**整体一个**，不按分区分别折叠）。
  ///
  /// 默认**展开**；折叠状态按页面分别记录 —— 画廊与预设页的筛选栏内容不同，
  /// 收起一个不代表收起另一个。
  static const _kGalleryFiltersCollapsed = 'comfyhub.galleryFiltersCollapsed';
  static const _kPromptFiltersCollapsed = 'comfyhub.promptFiltersCollapsed';
  static const _kAutoStartBackend = 'comfyhub.autoStartBackend';
  static const _kStopServicesOnExit = 'comfyhub.stopServicesOnExit';
  static const _kAllowRemoteBackend = 'comfyhub.allowRemoteBackend';
  static const _kProjectRoot = 'comfyhub.projectRoot';

  /// 画廊「集合详情」在**宽屏**（`lib/app.dart` 的 `kWideLayoutMinWidth`，900）下的形态。
  ///
  /// 两个宽度档各自一条偏好，形态由偏好决定，代码不按宽度写死。
  ///
  /// ⚠ 默认 `'dock'` / `'popup'` **必须等于原来的固定行为**（宽屏停靠、窄屏居中弹窗），
  /// 否则未修改设置时界面表现会变化。
  static const _kCollectionDetailWide = 'comfyhub.galleryCollectionDetailWide';

  /// 画廊「集合详情」在**窄屏**（`< kWideLayoutMinWidth`）下的形态。默认 `'popup'`。
  static const _kCollectionDetailNarrow =
      'comfyhub.galleryCollectionDetailNarrow';

  /// 界面外观（深色 / 浅色 / 跟随系统）。见 [AppThemeMode]。
  ///
  /// ⚠ 默认**深色**：此前 `MaterialApp.themeMode` 固定为 dark，修改默认值会改变
  /// 已安装环境的配色。
  static const _kThemeMode = 'comfyhub.themeMode';

  /// **Material You 取色档位**（默认 `full`，整份配色由系统强调色派生）。
  ///
  /// 旧版本只存布尔 `comfyhub.followSystemAccent`：读不到新键时按它迁移
  /// （true → full，false → off），保留已有选择。
  static const _kAccentMode = 'comfyhub.accentMode';
  static const _kLegacyFollowSystemAccent = 'comfyhub.followSystemAccent';

  /// 手机端**产物缓存**的原件预算（MB）。见 `media_cache.dart` 的
  /// [kOriginalCacheBudgetChoicesMb] —— 只提供固定档位，不接受任意数字
  /// （1MB 等同于关闭缓存，99999 等同于无上限）。
  static const _kMediaCacheOrigBudgetMb = 'comfyhub.mediaCacheOrigBudgetMb';

  /// 上次后端**实际**监听的端口（0 / 缺省 = 尚未记录）。
  ///
  /// 后端默认 `--port 0`（由操作系统选择空闲端口，避免被占用或落在 Windows
  /// 保留段），重启后端口会变化。手机保存的地址是 `http://<局域网IP>:<端口>`，
  /// 端口变化会使该地址失效，需要重新扫描配对二维码。
  ///
  /// 下次启动**优先请求该端口**；仅当端口已被其他进程占用时才更换
  /// （后端回落到随机端口，并在 `.run\api.json` 记录 `fellBack: true`，
  /// 随后记录新端口并在界面提示，见 `BackendLauncher.portFallbackFrom`）。
  ///
  /// ⚠ 这条偏好表示"上次实际使用的端口"，不是手动填写的端口。修改端口没有界面
  /// 入口 —— 唯一真源是后端绑定后写入的 `.run\api.json`（见 [refreshBaseUrlFromPortFile]）。
  static const _kLastApiPort = 'comfyhub.lastApiPort';

  String _baseUrl = defaultBaseUrl();
  int _pageSize = 24;
  bool _galleryFiltersCollapsed = false;
  bool _promptFiltersCollapsed = false;
  bool _autoStartBackend = true;
  bool _stopServicesOnExit = true;
  bool _allowRemoteBackend = false;
  String? _projectRoot;
  AppThemeMode _themeMode = AppThemeMode.dark;
  AppAccentMode _accentMode = AppAccentMode.full;
  CollectionDetailStyle _collectionDetailWide = CollectionDetailStyle.dock;
  CollectionDetailStyle _collectionDetailNarrow = CollectionDetailStyle.popup;
  int? _lastApiPort;

  /// 上次用的是**哪一份库**（身份 + 位置），见 `library_identity.dart`。
  ///
  /// 为什么记在偏好里：库可能被搬走 / 拔盘 / 复制出一份新的，目录字符串靠不住；
  /// "上次用的是这一份"是判断"这次打开的是不是你的数据"的唯一事实来源。
  LibraryRef? _lastLibrary;

  bool _loaded = false;

  /// 手机端产物缓存的原件预算（MB）。默认取 `media_cache.dart` 的常量，
  /// 与 `kOriginalCacheBudgetBytes` 保持同一个数值，避免默认值漂移。
  int _mediaCacheOrigBudgetMb = kOriginalCacheBudgetBytes ~/ (1024 * 1024);

  /// 用过哪些地址（最近使用的在前，最多 [maxBaseUrlHistory] 条）。
  ///
  /// 跨端场景需要在回环（模拟器 / 同机）与局域网 IP / Tailscale 之间切换。
  /// 历史记录是地址选择的**事实来源**，不是推测出的候选。
  List<String> _baseUrlHistory = const [];

  /// 地址来自手动填写（true）或自动发现（false，见 [baseUrlIsAuto]）。
  ///
  /// ⚠ 该标志**不单独决定是否采用端口文件**：回环地址（[isLocalBackendUrl] 为真）
  /// 一律按自动处理。判据只有一条，见 [refreshBaseUrlFromPortFile]。
  bool _baseUrlFromUser = false;

  String get baseUrl => _baseUrl;
  int get pageSize => _pageSize;

  /// 画廊筛选栏是否收起（默认展开）。
  bool get galleryFiltersCollapsed => _galleryFiltersCollapsed;

  /// 预设页筛选栏是否收起（默认展开）。
  bool get promptFiltersCollapsed => _promptFiltersCollapsed;

  /// 历史地址（最近用的在前）。界面拿它渲染"最近用过"那一组候选。
  List<String> get baseUrlHistory => List.unmodifiable(_baseUrlHistory);

  /// 历史最多保留几条（再多会难以查找，而不是可快速点击）。
  static const int maxBaseUrlHistory = 6;

  /// 当前地址是否来自手动填写（false 表示按本机 `.run\api.json` 自动确定）。
  ///
  /// 两个用途：后端重启后是否**改写地址**（见 [refreshBaseUrlFromPortFile]），
  /// 以及手机端启动时是否**自动选择一台桌面**（见 `app.dart` 的
  /// `_pickDesktopAutomatically`）—— 手动填写的地址不得改写。
  ///
  /// ⚠ **本机（回环）地址一律视为自动**：它指向本机后端，而本机后端端口由
  /// `--port 0` 自动分配。把随机端口当作配置保存后，后端重启会使界面无法连接。
  bool get baseUrlIsAuto => !_baseUrlFromUser || isLocalBackendUrl(_baseUrl);

  /// **该地址是否指向本机后端** —— 回环地址判据。
  ///
  /// 端口判定只使用这一条判据，不在其他地方重复判断"是否采用手动配置"：
  ///   · 真 —— 本机后端。端口由系统分配，唯一真源是 `.run\api.json`，
  ///     地址**始终跟随端口文件**（即使该值曾被写入偏好）；
  ///   · 假 —— 其他机器（局域网 IP / Tailscale / 域名），属于远程配置，
  ///     与端口文件无关，不得改写。
  ///
  /// `0.0.0.0` 也算"本机"：后端开了局域网监听时它写在端口文件里，
  /// 而客户端连通配地址是连不上的（读侧会回落回环，见 [_baseUrlFromPortFile]）。
  static bool isLocalBackendUrl(String url) {
    final v = url.trim();
    if (v.isEmpty) return false;
    var host = '';
    try {
      host = Uri.parse(v.contains('://') ? v : 'http://$v').host;
    } catch (_) {
      // 解析不了就走下面的正则兜底
    }
    if (host.isEmpty) {
      final m = RegExp(r'^(?:[a-zA-Z][a-zA-Z0-9+.-]*://)?([^/:?#]+)')
          .firstMatch(v);
      host = m?.group(1) ?? '';
    }
    host = host.toLowerCase();
    return host == '127.0.0.1' ||
        host == 'localhost' ||
        host == '::1' ||
        host == '[::1]' ||
        host == '0.0.0.0';
  }

  /// App 启动时是否自动把后端拉起来
  bool get autoStartBackend => _autoStartBackend;

  /// 关闭 App 时是否把**这次由 App 启动的**本地服务一并停掉（谁起的谁关）。
  /// 关掉的话服务会常驻，下次开 App 就是热启动。
  bool get stopServicesOnExit => _stopServicesOnExit;

  /// 允许手机 / 局域网设备连接本机后端（`COMFYHUB_ALLOW_REMOTE`）。
  ///
  /// 默认关闭：后端只监听回环地址。开启后后端监听所有网卡，真机才能连接桌面
  /// （后端仅监听 127.0.0.1 时，手机连接会超时）。远程请求由配对闸门校验
  /// （回环放行，局域网需要配对令牌）。
  bool get allowRemoteBackend => _allowRemoteBackend;

  /// 运行根目录（放 `.db\` / `storage\` / `.run\` 的那个目录）。为空表示自动探测。
  ///
  /// 判据见 `backend_paths.dart` 的 [looksLikeRoot]：源码树（`server-rs\Cargo.toml`）
  /// 与 release 产物（`viewer.exe` 同级带 `backend\`）两种布局都认。
  String? get projectRoot => _projectRoot;

  /// 界面外观（默认深色）。[AppThemeMode.material] 才是给 `MaterialApp` 的那个值。
  AppThemeMode get themeMode => _themeMode;

  /// **Material You 取色档位**（见 [AppAccentMode]）。
  AppAccentMode get accentMode => _accentMode;

  /// 兼容旧接口：等价于"档位不是 off"。
  bool get followSystemAccent => _accentMode != AppAccentMode.off;

  /// 宽屏（`>= kWideLayoutMinWidth`）下集合详情的形态。默认 [CollectionDetailStyle.dock]。
  CollectionDetailStyle get collectionDetailWide => _collectionDetailWide;

  /// 窄屏（`< kWideLayoutMinWidth`）下集合详情的形态。默认 [CollectionDetailStyle.popup]。
  CollectionDetailStyle get collectionDetailNarrow => _collectionDetailNarrow;

  /// 上次后端实际监听的端口（null = 还没记录过 → 这一轮还是让系统挑）。
  ///
  /// 只用于**请求**同一端口，不是配置项：唯一真源是后端写入的 `.run\api.json`。
  int? get lastApiPort => _lastApiPort;

  /// 上次用的是哪一份库（null = 还没记录过 / 记录坏了）。
  ///
  /// 判据点只有一个：**库自己的身份**（`<数据目录>\.db\library.json`），不是目录字符串。
  LibraryRef? get lastLibrary => _lastLibrary;

  /// 手机端产物缓存的原件预算（MB，默认 1536 = 1.5GB）。
  ///
  /// 只认 [kOriginalCacheBudgetChoicesMb] 里的档位：手改偏好写进来的野值一律回默认，
  /// 否则会出现"下拉框选不中任何一项"的界面。
  int get mediaCacheOrigBudgetMb => _mediaCacheOrigBudgetMb;

  bool get loaded => _loaded;

  /// 测试用：覆盖"去哪找 `.run\api.json`"。
  ///
  /// `null`（生产默认）= 按 [_projectRootCandidates] 真实探测；
  /// 空列表 = 假装这台机器上没有端口记录（用例期望"回落到 8080"时用这个）；
  /// 给了目录 = 只在这些目录里找（用例想喂一份假的端口文件时用）。
  @visibleForTesting
  static List<String>? portFileRootsOverride;

  /// 测试用：覆盖"手机默认收起筛选栏"这条平台判据（见 [load]）。
  /// 生产恒为 `null`，由真实平台决定。
  @visibleForTesting
  static bool? mobileFiltersDefaultOverride;

  static bool get _mobileFiltersDefault {
    final override = mobileFiltersDefaultOverride;
    if (override != null) return override;
    if (kIsWeb) return false;
    try {
      return Platform.isAndroid || Platform.isIOS;
    } catch (_) {
      return false;
    }
  }

  static String defaultBaseUrl() {
    if (kIsWeb) return 'http://127.0.0.1:8080';
    try {
      // Android 模拟器访问宿主机需要用 10.0.2.2。
      // 刻意**不**去读本机的 `.run\api.json`：那台机器上的后端跟手机是两台设备，
      // 手机连接的是宿主机端口，由设置页填写（远程场景见
      // docs/inbound-auth-design.md）。
      if (Platform.isAndroid) return 'http://10.0.2.2:8080';
    } catch (_) {
      // 平台判断在个别环境下会抛异常，忽略即可
    }
    // 桌面端：后端端口由系统**自动分配**，真实端口记录在 `<项目根>\.run\api.json`，
    // 前端必须读取该文件，否则会连接到一个固定端口，而后端可能运行在其他端口。
    return _baseUrlFromPortFile() ?? 'http://127.0.0.1:8080';
  }

  /// 从 `<项目根>\.run\api.json` 读后端**实际**监听的端口（读不到返回 null）。
  ///
  /// 这是端口这件事在 Dart 侧的唯一入口，与后端 `api_port_file.rs`、
  /// `comfyui\comfyhub_capture\capture_core.py` 读的是**同一个文件**。
  ///
  /// 三条约束：
  ///  · **读取失败时返回默认值**，不抛异常 —— 它在 App 启动路径上，异常会导致白屏；
  ///  · **仅当地址指向本机时才采用**（判据只有一条：[isLocalBackendUrl]；
  ///    手动填写的远程地址优先，否则远程 / 局域网场景会被强制切回本机）；
  ///  · **不缓存**：端口随重启变化，每次冷启动重新读取。
  static String? _baseUrlFromPortFile({String? projectRoot}) {
    for (final root in _projectRootCandidates(projectRoot)) {
      final state = readPortFile(root);
      final port = state?['port'];
      if (port is! int || port <= 0 || port > 65535) continue;
      final host = (state?['host'] as String?) ?? '127.0.0.1';
      // `COMFYHUB_ALLOW_REMOTE=1` 时后端听的是所有网卡，而客户端连 0.0.0.0 是不通的 → 回落回环
      final connectHost = (host.isEmpty || host == '0.0.0.0' || host == '::')
          ? '127.0.0.1'
          : host;
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
    // 存量清洗：旧版本会把**自动发现的回环地址**当作手动填写值存入 `_kBaseUrl`
    // （设置页「保存并测试连接」原样写回输入框）。该值在下次启动时成为失效地址，
    // 表现为"后端已启动、App 无法连接"。回环地址一律视为自动，因此按未填写处理，
    // 并删除该偏好，避免后续排查误判为手动配置。
    final storedIsLocal =
        stored != null && stored.isNotEmpty && isLocalBackendUrl(stored);
    if (storedIsLocal) await prefs.remove(_kBaseUrl);
    _baseUrlFromUser = stored != null && stored.isNotEmpty && !storedIsLocal;
    _baseUrl = _baseUrlFromUser ? stored! : defaultBaseUrl();
    _pageSize = prefs.getInt(_kPageSize) ?? 24;
    // 手机上筛选区默认**收起**：360dp 宽度下展开会占用半屏，画廊 / 预设应优先
    // 显示内容。桌面端仍默认展开。仅在无偏好时使用；设置后以偏好为准。
    //
    // ⚠ 判据用 `Platform.isAndroid`（真实运行平台），**不是** `defaultTargetPlatform`：
    // Flutter 测试进程里后者默认就是 android，用它会让所有 Windows 用例的筛选栏
    // 突然默认收起（2026-10-31 实测 4 条布局用例一起红）。
    final mobileDefault = _mobileFiltersDefault;
    _galleryFiltersCollapsed =
        prefs.getBool(_kGalleryFiltersCollapsed) ?? mobileDefault;
    _promptFiltersCollapsed =
        prefs.getBool(_kPromptFiltersCollapsed) ?? mobileDefault;
    _autoStartBackend = prefs.getBool(_kAutoStartBackend) ?? true;
    _stopServicesOnExit = prefs.getBool(_kStopServicesOnExit) ?? true;
    _allowRemoteBackend = prefs.getBool(_kAllowRemoteBackend) ?? false;
    _projectRoot = prefs.getString(_kProjectRoot);
    // 认不出来的值（老版本写的 / 手改过的）回默认，不抛：见 AppThemeMode.parse
    _themeMode = AppThemeMode.parse(prefs.getString(_kThemeMode));
    // 新键优先；没有就按老布尔迁移（true → accent，false → off）。
    final storedAccentMode = prefs.getString(_kAccentMode);
    _accentMode = storedAccentMode != null
        ? AppAccentMode.parse(storedAccentMode)
        : AppAccentMode.fromLegacyBool(
            prefs.getBool(_kLegacyFollowSystemAccent),
          );
    // 同上：回默认值就是"改之前写死在代码里的行为"（宽屏停靠 / 窄屏居中弹窗）
    _collectionDetailWide = CollectionDetailStyle.parse(
      prefs.getString(_kCollectionDetailWide),
      fallback: CollectionDetailStyle.dock,
    );
    _collectionDetailNarrow = CollectionDetailStyle.parse(
      prefs.getString(_kCollectionDetailNarrow),
      fallback: CollectionDetailStyle.popup,
    );
    _lastApiPort = _sanitizePort(prefs.getInt(_kLastApiPort));
    // "上次用的是哪份库"：坏记录当没记录（`LibraryRef.fromJson` 是宽容解析）。
    final rawLibrary = prefs.getString(kLastLibraryPrefsKey);
    if (rawLibrary != null && rawLibrary.isNotEmpty) {
      try {
        _lastLibrary = LibraryRef.fromJson(jsonDecode(rawLibrary));
      } catch (e) {
        debugPrint('[settings] 上次用的库记录读不懂，当没记录：$e');
        _lastLibrary = null;
      }
    }
    // 缓存预算：只认档位表里的值（旧版本或手动修改的异常值回默认），并**立刻应用到缓存**
    // —— 上次把 2GB 调整为 512MB 后，下次冷启动应按 512MB 淘汰，不等待下一次下载。
    final rawBudgetMb = prefs.getInt(_kMediaCacheOrigBudgetMb);
    _mediaCacheOrigBudgetMb =
        kOriginalCacheBudgetChoicesMb.contains(rawBudgetMb)
        ? rawBudgetMb!
        : kOriginalCacheBudgetBytes ~/ (1024 * 1024);
    MediaCache.instance.setOriginalBudget(
      _mediaCacheOrigBudgetMb * 1024 * 1024,
    );
    final rawHistory = prefs.getStringList(_kBaseUrlHistory);
    _baseUrlHistory = _sanitizeHistory(rawHistory);
    // 洗出来的和历史里躺着的不一样 → 把干净的**写回去**（同上：留着脏的只会误导排查，
    // 而且"最近用过"那一组候选会一直摆着几条永远点不通的本机地址）。
    if (rawHistory != null && !listEquals(rawHistory, _baseUrlHistory)) {
      await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
    }
    _loaded = true;
    notifyListeners();
  }

  /// 去重（保序）、丢掉空串与非 http 的脏值、截到上限。
  ///
  /// 读取时清洗：该表由上一版本程序写入，也可能被手动修改；直接渲染脏值
  /// 会出现无法点击的候选。
  ///
  /// ⚠ **回环地址不进入历史**：历史保存的是连接**其他机器**的候选；本机地址由
  /// 系统自动确定，端口每次启动都会变化。保留 `127.0.0.1:2175` 这类地址会在界面上
  /// 产生**无法连接**的候选。
  static List<String> _sanitizeHistory(List<String>? raw) {
    if (raw == null) return const [];
    final out = <String>[];
    for (final item in raw) {
      final v = item.trim();
      if (v.isEmpty || !v.startsWith('http')) continue;
      if (isLocalBackendUrl(v)) continue;
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

  /// 从历史中移除一条（点「忘记」时调用）。
  Future<void> forgetBaseUrl(String url) async {
    final v = url.trim();
    if (!_baseUrlHistory.contains(v)) return;
    _baseUrlHistory = [..._baseUrlHistory]..remove(v);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
  }

  /// 重新发现本机后端地址（后端重启更换端口时调用）。
  ///
  /// 返回 true 表示地址已变化（调用方需要重建 api client）。
  ///
  /// **判据只有一条**：地址指向**本机**（回环，[isLocalBackendUrl]）→ 始终跟随
  /// `.run\api.json`，即使该值曾被写入偏好；指向**其他机器** → 属于远程配置，不做改动。
  ///
  /// 当前端口不能当作配置保存：后端端口由 `--port 0` 自动分配，重启即变化，
  /// 保存的端口在下次启动时必然失效。早期实现使用 `if (_baseUrlFromUser) return false`，
  /// 而设置页会把自动发现的地址原样存回（`setBaseUrl`），导致 `_baseUrlFromUser`
  /// 为真后不再读取端口文件，App 对旧端口等待 60 秒超时。
  ///
  /// 调用时机：后端启动完成后（`BackendLauncher.ensureRunning` / 重启动作收尾）。
  /// 端口文件不可读（未启动 / 已停止）时保持原值，不做推测。
  Future<bool> refreshBaseUrlFromPortFile() async {
    if (_baseUrlFromUser && !isLocalBackendUrl(_baseUrl)) return false;
    final discovered = _baseUrlFromPortFile(projectRoot: _projectRoot);
    if (discovered == null || discovered == _baseUrl) return false;
    _baseUrl = discovered;
    notifyListeners();
    return true;
  }

  Future<void> setBaseUrl(String value) async {
    var v = value.trim();
    final prefs = await SharedPreferences.getInstance();
    // ⚠ **本机地址不属于手动配置**：本机后端端口由系统分配，唯一真源是 `.run\api.json`。
    // 回环地址与"清空"走同一分支 —— 恢复自动。否则会保存当前随机端口，后端重启后
    // 界面无法连接，且该偏好会使 [refreshBaseUrlFromPortFile] 提前返回。
    if (v.isEmpty || isLocalBackendUrl(v)) {
      _baseUrlFromUser = false;
      _baseUrl = defaultBaseUrl();
      notifyListeners();
      await prefs.remove(_kBaseUrl);
      return;
    }
    v = _normalizeUrl(v);
    _baseUrlFromUser = true;
    _baseUrl = v;
    // 同时记录该值：跨端切换时"上一条使用过的"是最佳候选（见 _baseUrlHistory）
    _baseUrlHistory = [
      v,
      ..._baseUrlHistory.where((e) => e != v),
    ].take(maxBaseUrlHistory).toList();
    notifyListeners();
    await prefs.setString(_kBaseUrl, v);
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
  }

  /// **自动选择的地址**：只在本进程生效。
  ///
  /// 与 [setBaseUrl] 的区别是**是否计为手动填写**：该方法**不写 `_kBaseUrl`、不置
  /// `_baseUrlFromUser`** —— 下次冷启动重新选择才属于"自动"（跨端地址会变化：
  /// 切换 Wi-Fi、切换到 Tailscale、模拟器换真机）。该地址**进入历史**，下次优先尝试。
  ///
  /// 返回 true = 地址真的变了（调用方要重建 api client）。
  Future<bool> adoptAutoBaseUrl(String value) async {
    final v = _normalizeUrl(value);
    if (v.isEmpty || v == _baseUrl) return false;
    _baseUrl = v;
    _baseUrlHistory = [
      v,
      ..._baseUrlHistory.where((e) => e != v),
    ].take(maxBaseUrlHistory).toList();
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
    return true;
  }

  /// 地址归一化（补 scheme、去掉尾部斜杠）——[setBaseUrl] 与 [adoptAutoBaseUrl] 共用一份。
  static String _normalizeUrl(String value) {
    var v = value.trim();
    if (v.isEmpty) return '';
    if (!v.startsWith('http://') && !v.startsWith('https://')) v = 'http://$v';
    while (v.endsWith('/')) {
      v = v.substring(0, v.length - 1);
    }
    return v;
  }

  Future<void> setPageSize(int value) async {
    _pageSize = value.clamp(8, 120);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kPageSize, _pageSize);
  }

  /// 记住筛选栏的折叠状态（按页面各自记）。
  Future<void> setGalleryFiltersCollapsed(bool value) async {
    _galleryFiltersCollapsed = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kGalleryFiltersCollapsed, value);
  }

  Future<void> setPromptFiltersCollapsed(bool value) async {
    _promptFiltersCollapsed = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(_kPromptFiltersCollapsed, value);
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

  /// 端口合法区间内的值才收；其余（0 / 负数 / 超过 65535 / 手改坏的）一律当"没记录过"。
  ///
  /// **不许把它拼进 argv 之前才校验**：`--port 99999` 会让后端直接退出
  /// （`main.rs` 里 "端口必须是 1~65535" 那条分支），而"App 记着一条脏值"
  /// 这件事本身就不该存在。
  static int? _sanitizePort(int? value) =>
      (value != null && value >= 1 && value <= 65535) ? value : null;

  /// 把后端这次实际监听的端口落盘（下次启动要回它）。见 [_kLastApiPort]。
  Future<void> setLastApiPort(int? value) async {
    final clean = _sanitizePort(value);
    if (_lastApiPort == clean) return;
    _lastApiPort = clean;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    if (clean == null) {
      await prefs.remove(_kLastApiPort);
    } else {
      await prefs.setInt(_kLastApiPort, clean);
    }
  }

  /// 记下"这次用的是哪一份库"。见 `library_identity.dart`。
  ///
  /// 只由启动流程（`BackendLauncher`）在**真的用上这份库**之后调用 —— 绝不因为
  /// "扫到一个 `.db`"就写。坏了就当没记录，所以这里只写合法引用。
  Future<void> setLastLibrary(LibraryRef? value) async {
    final same =
        (value == null && _lastLibrary == null) ||
        (value != null &&
            _lastLibrary != null &&
            value.id == _lastLibrary!.id &&
            value.path == _lastLibrary!.path);
    if (same) return;
    _lastLibrary = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    if (value == null) {
      await prefs.remove(kLastLibraryPrefsKey);
    } else {
      await prefs.setString(kLastLibraryPrefsKey, jsonEncode(value.toJson()));
    }
  }

  /// 选产物缓存的原件预算（MB）。**立即生效**：缓存随即淘汰一次，
  /// 把 2GB 调整为 512MB 后，占用必须立即下降（见 `MediaCache.setOriginalBudget`）。
  Future<void> setMediaCacheOrigBudgetMb(int mb) async {
    if (!kOriginalCacheBudgetChoicesMb.contains(mb)) return;
    if (_mediaCacheOrigBudgetMb == mb) return;
    _mediaCacheOrigBudgetMb = mb;
    MediaCache.instance.setOriginalBudget(mb * 1024 * 1024);
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kMediaCacheOrigBudgetMb, mb);
  }

  /// 切换界面外观。**立刻 `notifyListeners()` 再落盘**：换主题是"看得见"的操作，
  /// 不能等磁盘（`ComfyHubApp` 就挂着这个 notifier 重建 `MaterialApp`）。
  Future<void> setThemeMode(AppThemeMode value) async {
    if (_themeMode == value) return;
    _themeMode = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kThemeMode, value.name);
  }

  /// 切换取色档位。**先通知再落盘**（`MaterialApp` 挂着这个 notifier，
  /// 主题要当场换，不该等磁盘）。
  Future<void> setAccentMode(AppAccentMode value) async {
    if (_accentMode == value) return;
    _accentMode = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kAccentMode, value.name);
  }

  /// 兼容旧接口：开 = [AppAccentMode.accent]，关 = [AppAccentMode.off]。
  Future<void> setFollowSystemAccent(bool value) =>
      setAccentMode(value ? AppAccentMode.accent : AppAccentMode.off);

  /// 重新检测一次系统强调色，然后让主题重建（设置页的「重新检测」）。
  Future<void> refreshSystemAccent() async {
    await SystemAccent.detect();
    notifyListeners();
  }

  /// 选宽屏下集合详情的形态。**先 `notifyListeners()` 再落盘**：画廊监听着它，
  /// 改完当场换形态（不许要求重启），磁盘那一步不该挡在界面响应前面。
  Future<void> setCollectionDetailWide(CollectionDetailStyle value) async {
    if (_collectionDetailWide == value) return;
    _collectionDetailWide = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kCollectionDetailWide, value.name);
  }

  /// 选窄屏下集合详情的形态（同上：当场生效）。
  Future<void> setCollectionDetailNarrow(CollectionDetailStyle value) async {
    if (_collectionDetailNarrow == value) return;
    _collectionDetailNarrow = value;
    notifyListeners();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kCollectionDetailNarrow, value.name);
  }
}
