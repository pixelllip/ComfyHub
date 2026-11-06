import 'package:flutter/foundation.dart';

import '../core/ai_api_client.dart';
import '../models/ai_models.dart';

/// NSFW 内容开关（2026-09-20 用户要求）。
///
/// 两个选项**放一起**（用户原话）：① 是否注入那段"越狱"系统提示词；
/// ② 预览图要不要模糊显示。真源在后端（`app_settings` 的 `ai.nsfw`），
/// 这个 store 只是它的**只读镜像 + 写入通道**：
///
///  - 后端在拼系统提示词时要读同一份设置 —— 前端自己存一份（`shared_preferences`）
///    就会出现"界面显示开着、后端其实没注入"这种自相矛盾的状态；
///  - [effectiveBlur] 也以后端算好的为准，前端不在别处再推一遍。
///
/// 它是**全局**的（挂在 `MultiProvider` 根部）：打码要同时作用于画廊、提示词、
/// 标签页和 AI 气泡里的产物卡 —— 只在某个页面里持有的状态做不到这一点。
class NsfwStore extends ChangeNotifier {
  final String Function()? baseUrlProvider;
  final AiApiClient? _injected;

  /// 冷启动闸门：等本地服务有个结论再读设置（`BackendLauncher.settled`）。
  ///
  /// 它在这里**不只是"少一句报错"**：读失败时这个 store 保留的是默认值
  /// （`blurEnabled == false`），于是"用户开了预览图模糊、冷启动却不糊"——
  /// 同一个根因（端口文件还没写出来就发请求）在 NSFW 这条路上是**静默错**。
  final Future<void> Function()? backendReady;

  AiApiClient? _cached;
  String? _cachedFor;

  NsfwStore({this.baseUrlProvider, AiApiClient? api, this.backendReady})
    : _injected = api;

  AiApiClient get _api {
    final injected = _injected;
    if (injected != null) return injected;
    final base = baseUrlProvider!();
    if (_cached == null || _cachedFor != base) {
      _cached?.dispose();
      // 令牌由 `AiApiClient` 默认带上（进程内那一份配对令牌）—— 见 `PairingTokenCache`
      _cached = AiApiClient(base);
      _cachedFor = base;
    }
    return _cached!;
  }

  AiNsfwSettings settings = const AiNsfwSettings();

  bool _loading = false;
  String? error;

  /// 用户在这次会话里"点开看过"的图（点一下打码的图就解开）。
  ///
  /// **刻意只放内存**：关掉 App 再打开时重新打码 —— 用户开"模糊"就是不想一进来
  /// 就被糊一脸，而不是"永久解锁某几张图"。
  final Set<String> _revealed = <String>{};

  bool get loading => _loading;

  /// 要不要打码（= 后端算好的 `effectiveBlur`）。
  bool get blurEnabled => settings.effectiveBlur;

  /// NSFW 开关本身开着没有（设置页显示用）。
  bool get nsfwEnabled => settings.enabled;

  bool get blurOptionEnabled => settings.blur;

  /// 注入正文的版本号（后端 `NsfwPrompt.VERSION`）。
  String get promptVersion => settings.promptVersion;

  /// 这一张图现在要不要打码（[key] 通常是 `media:<id>`）。
  ///
  /// **调用方必须先确认这一条是 NSFW**（`NsfwBlur` 的 `nsfw` 参数）：
  /// 这个方法只回答"用户开着模糊开关、且这一条这次还没被看过吗"。
  /// 以前它就是全部判据，于是开关一开**所有**内容都被糊住（用户 bug 2026-09-20）。
  bool shouldBlur(String? key) {
    if (!blurEnabled) return false;
    if (key == null || key.isEmpty) return true;
    return !_revealed.contains(key);
  }

  /// 这一条已经被看过（打开了详情页）：本次会话不再糊它。
  bool isRevealed(String? key) =>
      key != null && key.isNotEmpty && _revealed.contains(key);

  /// 用户点开了这一条（详情页）：把它解开（只对本次会话有效）。
  void reveal(String? key) {
    if (key == null || key.isEmpty) return;
    if (_revealed.add(key)) notifyListeners();
  }

  Future<void> load({bool silent = false}) async {
    if (!silent) {
      _loading = true;
      notifyListeners();
    }
    try {
      // 冷启动那一次要等后端起来（见 [backendReady]）—— 否则读到的是 8080 兜底地址，
      // 失败之后 `settings` 停在默认值上（"模糊显示"于是静默失效）。
      await backendReady?.call();
      final next = await _api.nsfwSettings();
      settings = next;
      error = null;
    } catch (e) {
      // 后端没起来时**不当作"开了"**：宁可显示原图，也不要让人以为应用坏了
      error = '读取 NSFW 设置失败，$e';
    } finally {
      _loading = false;
      notifyListeners();
    }
  }

  /// 写设置：`enabled` / `blur` 一起发（两个选项本来就放一起）。
  Future<({bool ok, String message})> save({
    required bool enabled,
    required bool blur,
  }) async {
    try {
      final saved = await _api.updateNsfwSettings(enabled: enabled, blur: blur);
      settings = saved;
      // 关掉打码（或整个 NSFW）时把"看过"的记录清掉：再打开时应当是打码状态
      if (!saved.effectiveBlur) _revealed.clear();
      error = null;
      notifyListeners();
      return (ok: true, message: '已保存');
    } catch (e) {
      error = '$e';
      notifyListeners();
      return (ok: false, message: '保存失败，$e');
    }
  }

  @override
  void dispose() {
    _cached?.dispose();
    super.dispose();
  }
}
