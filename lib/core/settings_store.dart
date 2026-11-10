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
/// 用户 2026-10-18："能不能让我自由选择在电脑/手机排版的画廊的集合详情行为？"
/// —— 以前是**代码**按宽度档替他定（宽屏停靠、窄屏弹窗），现在两个宽度档
/// **各自一条偏好**，选哪边由用户自己说。
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

  /// 读偏好。认不出来的（老版本写的 / 用户手改过的）回 [fallback]，**绝不抛** ——
  /// 它在界面构建路径上，抛了就是白屏（与 `AppThemeMode.parse` 同一条纪律）。
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

  /// 筛选栏折叠状态（**整体一个**，用户 2026-10-11 定的：不是每个分区各自折叠）。
  ///
  /// 默认**展开**（别一上来就把功能藏起来）；用户收起过就记住 —— 按页面各自记，
  /// 因为画廊和预设页的筛选栏内容不同，收了一个不代表想收另一个。
  static const _kGalleryFiltersCollapsed = 'comfyhub.galleryFiltersCollapsed';
  static const _kPromptFiltersCollapsed = 'comfyhub.promptFiltersCollapsed';
  static const _kAutoStartBackend = 'comfyhub.autoStartBackend';
  static const _kStopServicesOnExit = 'comfyhub.stopServicesOnExit';
  static const _kAllowRemoteBackend = 'comfyhub.allowRemoteBackend';
  static const _kProjectRoot = 'comfyhub.projectRoot';

  /// 画廊「集合详情」在**宽屏**（`lib/app.dart` 的 `kWideLayoutMinWidth`，900）下的形态。
  ///
  /// 用户 2026-10-18："能不能让我自由选择在电脑/手机排版的画廊的集合详情行为？"
  /// —— 两个宽度档各自一条偏好，形态由用户定，代码不再替他选。
  ///
  /// ⚠ 默认 `'dock'` / `'popup'` **必须等于改之前写死在代码里的行为**
  /// （宽屏停靠、窄屏居中弹窗），否则没动过设置的人看到的东西就变了。
  static const _kCollectionDetailWide = 'comfyhub.galleryCollectionDetailWide';

  /// 画廊「集合详情」在**窄屏**（`< kWideLayoutMinWidth`）下的形态。默认 `'popup'`。
  static const _kCollectionDetailNarrow =
      'comfyhub.galleryCollectionDetailNarrow';

  /// 界面外观（深色 / 浅色 / 跟随系统）。见 [AppThemeMode]。
  ///
  /// ⚠ 默认**深色**：这是 2026-10-13 之前唯一存在过的外观（`MaterialApp.themeMode`
  /// 写死 dark），换成别的默认值等于给所有存量用户换一套配色。
  static const _kThemeMode = 'comfyhub.themeMode';

  /// **Material You 取色档位**（默认 `full` = 整份配色都由系统强调色派生，
  /// 2026-11-05 用户拍板"尺度做到 A"）。
  ///
  /// 老版本只存了布尔 `comfyhub.followSystemAccent`：读不到新键时按它迁移
  /// （true → full，false → off），用户已有的选择不丢。
  static const _kAccentMode = 'comfyhub.accentMode';
  static const _kLegacyFollowSystemAccent = 'comfyhub.followSystemAccent';

  /// 手机端**产物缓存**的原件预算（MB）。见 `media_cache.dart` 的
  /// [kOriginalCacheBudgetChoicesMb] —— 档位就那几项，别让用户填任意数字
  /// （填 1MB 等于关掉缓存，填 99999 等于没有上限，两头都不是"设上限"的本意）。
  static const _kMediaCacheOrigBudgetMb = 'comfyhub.mediaCacheOrigBudgetMb';

  /// 上次后端**实际**监听的端口（0 / 缺省 = 还没记录过）。
  ///
  /// # 为什么必须记住它（2026-10-13 用户提的问题）
  ///
  /// 后端默认 `--port 0`（让操作系统挑空闲端口，避免"被占 / 落在 Windows 保留段"
  /// 起不来），**代价是重启就换端口**。而**手机那边存的是
  /// `http://<局域网IP>:<端口>`** —— 端口一变那条地址就废了，用户得跑到桌面上
  /// 重新扫一次配对二维码（用户原话："怎么保证手机稳定连接电脑，而不用每次
  /// 重新配置"）。
  ///
  /// 所以下次启动**先要回这个端口**；只有"它确实被别人占了"这一种情况才换
  /// （那时由后端自己回落到随机端口，并在 `.run\api.json` 里记 `fellBack: true`，
  /// 我们再把新端口记下来 + 在界面上如实说一声，见 `BackendLauncher.portFallbackFrom`）。
  ///
  /// ⚠ 这条偏好**不是**"用户手填的端口"：它是"上次实际用的那个"。
  /// 想改端口没有任何界面入口 —— 真源永远只有一个：后端绑好之后写的
  /// `.run\api.json`（见 [refreshBaseUrlFromPortFile]）。
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
  /// 老用户不吃"默认值漂移"的亏 —— 与 `kOriginalCacheBudgetBytes` 同一个数。
  int _mediaCacheOrigBudgetMb = kOriginalCacheBudgetBytes ~/ (1024 * 1024);

  /// 用过哪些地址（最近用的在前，最多 [maxBaseUrlHistory] 条）。
  ///
  /// 为什么要有它：跨端用户至少有两条路要来回切 —— 模拟器/同机（回环）与
  /// 真机（局域网 IP / Tailscale）。以前设置页只有**一个文本框**，
  /// 换一次就得**手抄一遍 IP**，抄错一个数字就是"连不上"，而上一轮用过的地址
  /// 明明就在几分钟前。所以历史是"选择"这件事的**事实来源**（不是猜的候选）。
  List<String> _baseUrlHistory = const [];

  /// 地址是用户手填的（true）还是我们自动发现的（false，见 [baseUrlIsAuto]）。
  ///
  /// ⚠ 它**不再单独决定"要不要听端口文件的"** —— 回环地址（[isLocalBackendUrl] 为真）
  /// 一律按自动处理，不管这里是什么。判据只有一条，见 [refreshBaseUrlFromPortFile]。
  bool _baseUrlFromUser = false;

  String get baseUrl => _baseUrl;
  int get pageSize => _pageSize;

  /// 画廊筛选栏是否收起（默认展开）。
  bool get galleryFiltersCollapsed => _galleryFiltersCollapsed;

  /// 预设页筛选栏是否收起（默认展开）。
  bool get promptFiltersCollapsed => _promptFiltersCollapsed;

  /// 历史地址（最近用的在前）。界面拿它渲染"最近用过"那一组候选。
  List<String> get baseUrlHistory => List.unmodifiable(_baseUrlHistory);

  /// 历史最多留几条（再多就成了"翻半天找不到"，而不是"顺手点一下"）。
  static const int maxBaseUrlHistory = 6;

  /// 当前地址是不是"用户手填的"（false = 我们按本机 `.run\api.json` 自动定的）。
  ///
  /// 用途有两个：后端重启后要不要**改口**（见 [refreshBaseUrlFromPortFile]），
  /// 以及手机端启动时要不要**自己挑一台桌面**（见 `app.dart` 的
  /// `_pickDesktopAutomatically`）—— 用户手填过就不许替他改。
  ///
  /// ⚠ **本机（回环）地址一律算"自动"**：它说的是"这台机器上的后端"，
  /// 而本机后端端口是 `--port 0` 自动挑的。把它当用户配置存下来，等于把
  /// "当前这个随机端口"抄了一遍 —— 下次后端一重启，界面就永远连不上
  /// （2026-10-12 用户报的"直接启动软件连接不上后端"，根因就在这）。
  bool get baseUrlIsAuto => !_baseUrlFromUser || isLocalBackendUrl(_baseUrl);

  /// **"这个地址说的是本机后端吗"** —— 回环地址判据。
  ///
  /// 端口这件事**只有这一条判据**，别在别处再写一遍"要不要听用户的"：
  ///   · 真 —— 本机后端。端口是自动挑的、真源只有 `.run\api.json`，
  ///     所以地址**永远跟着它走**（哪怕这个值先前被写进过偏好）；
  ///   · 假 —— 别的机器（局域网 IP / Tailscale / 域名）。那是用户的远程配置，
  ///     端口文件与它无关，一个字都不许改。
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
  /// 默认关：后端只听回环。开着时后端听所有网卡 —— 真机连桌面才有得连
  /// （2026-09-28 用户 bug「手机连接桌面端失败，报 timed out」的根因就是后端
  /// 只听 127.0.0.1）。远程请求由配对闸门把关（回环放行，局域网要配对令牌）。
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
  /// 只用来**要回**同一个端口，不是"用户配置"：真源永远是后端写的 `.run\api.json`。
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
  ///  · **只在"这个地址说的是本机"时才收下它**（判据只有一条：[isLocalBackendUrl]。
  ///    用户填了别的机器就永远优先用他的，否则远程 / 局域网场景会被拽回本机）；
  ///  · 用完**不缓存**：端口会随重启变，每次冷启动都重新读一次。
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
    // 存量清洗（2026-10-12）：老版本会把**自动发现来的回环地址**当"用户手填"存进
    // `_kBaseUrl`（设置页那个「保存并测试连接」把框里的值原样写回），而那份值
    // 下次启动就是死地址 —— 界面的表现是"后端明明起来了、App 却连不上"。
    // 回环 = 本机 = 自动，所以这里直接当没填过，并顺手把那条偏好删掉：
    // 留着它只会让下一个排查的人以为"用户手填过"。
    final storedIsLocal =
        stored != null && stored.isNotEmpty && isLocalBackendUrl(stored);
    if (storedIsLocal) await prefs.remove(_kBaseUrl);
    _baseUrlFromUser = stored != null && stored.isNotEmpty && !storedIsLocal;
    _baseUrl = _baseUrlFromUser ? stored! : defaultBaseUrl();
    _pageSize = prefs.getInt(_kPageSize) ?? 24;
    // 手机上筛选区默认**收起**：360dp 宽度里它一展开就占掉半屏，画廊 / 预设先看到的
    // 应该是内容而不是一排筛选 chip（用户 2026-10-31）。桌面端仍默认展开。
    // 只作为"没有偏好时"的默认值：用户自己点过展开 / 收起之后，偏好说了算。
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
    // 缓存预算：只认档位表里的值（老版本 / 手改的野值回默认），并**立刻应用到缓存**
    // —— 用户上次把 2GB 调到 512MB，下次冷启动就该按 512MB 淘汰，不能等下一次下载。
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
  /// 为什么在**读**的时候也要洗一遍：这份表是上一版程序写进去的（或者用户
  /// 手改过 SharedPreferences），界面直接渲染脏值就会出现一个点不动的候选。
  ///
  /// ⚠ **回环地址不进历史**（2026-10-12）：历史是"联**别的机器**的候选"，
  /// 而本机地址是自动的、端口每次启动都换 —— 把 `127.0.0.1:2175` 这种留在里面，
  /// 只会在界面上摆一串**永远点不通**的候选（它们正是那个 bug 留下的渣）。
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
  ///
  /// **判据只有一条**（2026-10-12 定案，别再拆成两半）：地址指向**本机**（回环，
  /// [isLocalBackendUrl]）→ 永远跟着 `.run\api.json` 走，**哪怕它先前被写进过偏好**；
  /// 指向**别的机器** → 那是用户的远程配置，什么也不做。
  ///
  /// 为什么必须这样：后端端口是 `--port 0` 自动挑的（重启就换），"把当前这个端口
  /// 存下来"等于存了一个下次启动必然失效的死地址。2026-10-12 之前这里写的是
  /// `if (_baseUrlFromUser) return false`，而设置页会把框里**自动发现来的**地址
  /// 原样存回去（`setBaseUrl`），于是 `_baseUrlFromUser` 一旦为真就再也不读端口文件 ——
  /// 后端在新端口上跑得好好的，App 却对着旧端口等满 60 秒超时。
  ///
  /// 调用时机：后端启动完成之后（`BackendLauncher.ensureRunning` / 重启动作收尾处）。
  /// 端口文件读不到（还没起来 / 已停止）时保持原样，绝不猜。
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
    // ⚠ **本机地址不是"用户配置"**（2026-10-12）：本机后端端口由它自己挑，真源只有
    // `.run\api.json`。所以回环地址与"清空"走同一条路 —— 回到自动。
    // 否则存下来的就是"当前这个随机端口"，下次后端一重启界面就永远连不上
    // （而且这条偏好还会让 [refreshBaseUrlFromPortFile] 一直提前返回）。
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
    // 顺手记住它：跨端来回切时，"上一条用过的"就是最好的候选（见 _baseUrlHistory）
    _baseUrlHistory = [
      v,
      ..._baseUrlHistory.where((e) => e != v),
    ].take(maxBaseUrlHistory).toList();
    notifyListeners();
    await prefs.setString(_kBaseUrl, v);
    await prefs.setStringList(_kBaseUrlHistory, _baseUrlHistory);
  }

  /// **自动挑到的地址**：只在本进程生效。
  ///
  /// 与 [setBaseUrl] 的区别就是"算不算用户手填"：这一条**不写 `_kBaseUrl`、也不置
  /// `_baseUrlFromUser`** —— 下次冷启动重新挑一次才是"自动"（跨端地址本来就会变：
  /// 换了 Wi-Fi、换到 Tailscale、模拟器换真机）。但它**进历史**，下次优先试它。
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

  /// 选产物缓存的原件预算（MB）。**当场生效**：缓存那边顺手淘汰一次，
  /// 用户把 2GB 调到 512MB 之后占用必须马上降下来（见 `MediaCache.setOriginalBudget`）。
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
