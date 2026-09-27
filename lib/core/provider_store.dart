import 'dart:convert';
import 'dart:io' show Platform;

import 'package:flutter/services.dart';

import '../models/ai_models.dart';
import 'ai_api_client.dart';
import 'run_event_source.dart' show LocalHarnessRunEventSource;

/// Provider 与模型目录的**读写来源**（M6 的另一半）。
///
/// 用户拍板的第 4 条原话是"**手机完全可以独立配置 provider，有密钥的也同步到手机端**"：
/// 密钥那一半（Keystore + 下行 `GET /api/ai/credentials` + 上行 `PUT`）早就做完了，
/// 但**清单这一半一直不成立** —— 这一页的 provider / 模型 CRUD 清一色打 `AiApiClient`
/// （桌面 HTTP），桌面不在线时手机上连"新建一个 provider / 改个 baseURL"都做不到，
/// 而 `android/harness` 的本地库里 `ai_providers` / `ai_models` **本来就是本机表**
/// （`MobileSchema` 的同步范围之外）。
///
/// | | 真源 | 说明 |
/// | --- | --- | --- |
/// | 桌面 | 后端库（`/api/ai/providers`） | 唯一真源：清单与模型目录都在那儿 |
/// | 手机 | **本机库**（Kotlin `HarnessStore` 的 `ai_providers` / `ai_models`） | 桌面不在线也能建、能改；**这些 provider 只在这台手机上** |
///
/// ⚠ **两条清单不自动合并**（`docs/pairing-design.md` §14.4）：同步的只有**密钥的明文**
/// （用户显式点「推给桌面」那一个动作），清单各归各 —— 所以界面上必须**如实说一句**
/// "这些 provider 只在这台手机上，桌面看不到它"，否则用户会以为建完桌面也能用。
///
/// 三条纪律与 `CredentialStore` 同一套：
///  1. **手机上一条网络请求都不打**：走桥 → 本机库（`dart:io` 的网络栈在测试里也不会被碰）；
///  2. **错误形状两端一致**：桥的 `PlatformException` 在这里翻成 [AiApiException]
///     （页面上的 `'$e'` 因此在两端长得一样，都是人话）；
///  3. **清单里没有密钥**（AIH-012）：DTO 里的 `credential` 只是"这台机器上有没有 key"的状态，
///     手机上它由本机密钥库说了算（页面用 `CredentialStore.statusOf` 现问）。
abstract class ProviderStore {
  /// 手机（本机库）还是桌面（后端库）。
  bool get isLocal;

  /// 这一端能不能**自己**探上游（连接测试 / 获取模型）。
  ///
  /// 手机侧暂时**不能**：那两步要**由这一端自己**向上游发请求（还要用本机密钥 + 端点信任档
  /// 那套），而手机侧还没有 `provider_probe` 那一份实现。界面据此把这两个按钮**禁用并说明** ——
  /// 不这么做的话用户点下去会拿到"Provider 不存在: x"（桌面根本不认识手机上这条 id），
  /// 那是最难解释的一种失败。
  bool get supportsProbe;

  Future<List<AiProvider>> listProviders();

  /// 新建一条。参数就是 `POST /api/ai/providers` 的**请求体形状**
  /// （`id` / `displayName` / `api` / `baseURL` / `credentialRef` / `endpointTrust`）——
  /// 与页面里已经拼好的那份 draft 逐字一致，两端实现都不翻译。
  ///
  /// id 已存在时**报错**（"Provider ID 已存在: x（ID 创建后不可修改）"，桌面同句），
  /// 不静默覆盖 —— 覆盖会把用户正在用的那条的地址一起改掉。
  Future<AiProvider> createProvider(Map<String, dynamic> draft);

  /// 更新一条已有的。**乐观锁**（AIH-007）：`value.revision` 必须与库里那一行相等，
  /// 否则报"配置已被其他窗口修改…请刷新后重试"（桌面同句）。
  Future<AiProvider> updateProvider(AiProvider value);

  Future<void> deleteProvider(String id);

  Future<List<AiModel>> listModels(String providerId);

  /// **全量替换**某条 provider 的模型目录（AIH-011）：界面上的列表就是全部。
  Future<List<AiModel>> saveModels(String providerId, List<AiModel> models);
}

/// 桌面实现：转发给后端（`/api/ai/providers*`）。
class RemoteProviderStore implements ProviderStore {
  RemoteProviderStore(this._api);

  final AiApiClient _api;

  @override
  bool get isLocal => false;

  /// 桌面就是探上游的那一端（`provider_probe.rs` 在它后面）。
  @override
  bool get supportsProbe => true;

  @override
  Future<List<AiProvider>> listProviders() => _api.listProviders();

  @override
  Future<AiProvider> createProvider(Map<String, dynamic> draft) => _api.createProvider(draft);

  /// ⚠ 传 `toJson()`（**带 revision**）：后端靠它做乐观锁，少了它后端会拒
  /// （"缺少 revision，请刷新后重试"）。
  @override
  Future<AiProvider> updateProvider(AiProvider value) =>
      _api.updateProvider(value.id, value.toJson());

  @override
  Future<void> deleteProvider(String id) => _api.deleteProvider(id);

  @override
  Future<List<AiModel>> listModels(String providerId) => _api.listModels(providerId);

  @override
  Future<List<AiModel>> saveModels(String providerId, List<AiModel> models) =>
      _api.saveModels(providerId, models.map((m) => m.toJson()).toList());
}

/// 手机实现：`MethodChannel` → 本地 Kotlin harness → **本机库**（`ai_providers` / `ai_models`）。
///
/// ⚠ **一条网络请求都不打**：这几个动作只读写本机库，桌面不在线照样能用。
/// 与手机侧的凭据实现同一个通道（复用常量，改一处就够）。
class DeviceProviderStore implements ProviderStore {
  DeviceProviderStore({MethodChannel? channel})
      : _method = channel ?? const MethodChannel(LocalHarnessRunEventSource.methodChannelName);

  final MethodChannel _method;

  @override
  bool get isLocal => true;

  /// 见 [ProviderStore.supportsProbe]：手机上还没实现探上游那一套。
  @override
  bool get supportsProbe => false;

  @override
  Future<List<AiProvider>> listProviders() async =>
      _rows(await _invoke('listProviders')).map(AiProvider.fromJson).toList();

  @override
  Future<AiProvider> createProvider(Map<String, dynamic> draft) async {
    // draft 里没有 revision → 桥按"新建"处理（与桌面 POST 同：revision 由库给 1）
    final raw = await _invoke('saveProvider', {'provider': jsonEncode(draft)});
    return AiProvider.fromJson(_row(raw, 'saveProvider'));
  }

  @override
  Future<AiProvider> updateProvider(AiProvider value) async {
    // toJson() 带 revision → 桥走乐观锁更新（与桌面 PUT 同）
    final raw = await _invoke('saveProvider', {'provider': jsonEncode(value.toJson())});
    return AiProvider.fromJson(_row(raw, 'saveProvider'));
  }

  @override
  Future<void> deleteProvider(String id) async {
    try {
      await _method.invokeMethod<bool>('deleteProvider', {'id': id});
    } on PlatformException catch (e) {
      throw AiApiException(400, e.message ?? e.code);
    } on MissingPluginException {
      throw AiApiException(400, '本机 harness 还没接上（deleteProvider）');
    }
  }

  @override
  Future<List<AiModel>> listModels(String providerId) async =>
      _rows(await _invoke('listModels', {'providerId': providerId})).map(AiModel.fromJson).toList();

  @override
  Future<List<AiModel>> saveModels(String providerId, List<AiModel> models) async {
    final raw = await _invoke('saveModels', {
      'providerId': providerId,
      // 键名与后端 DTO 逐字一致：同一个 toJson() 走两端
      'models': jsonEncode(models.map((m) => m.toJson()).toList()),
    });
    return _rows(raw).map(AiModel.fromJson).toList();
  }

  /// 桥调用（回一段 JSON 字符串）。
  ///
  /// **异常翻译在这一处**：桥的 `result.error('CONFIG_ERROR', 人话)` 到这里是
  /// `PlatformException(code, message)`，翻成 [AiApiException] 之后与远端那条路
  /// **同一种类型、同一种人话** —— 页面上的 `'$e'` 因此不需要分叉。
  /// 状态码填 400 只是"配置类错误"的记号（AI 领域的配置错误在桌面上也是 400）。
  Future<String?> _invoke(String method, [Map<String, dynamic>? args]) async {
    try {
      return await _method.invokeMethod<String>(method, args);
    } on PlatformException catch (e) {
      throw AiApiException(400, e.message ?? e.code);
    } on MissingPluginException {
      throw AiApiException(400, '本机 harness 还没接上（$method）');
    }
  }

  /// 桥回的是 JSON **数组**（provider 清单 / 模型目录）。
  static List<Map<String, dynamic>> _rows(String? raw) {
    if (raw == null || raw.trim().isEmpty) return const [];
    final decoded = jsonDecode(raw);
    if (decoded is! List) return const [];
    return decoded.whereType<Map>().map((e) => Map<String, dynamic>.from(e)).toList();
  }

  /// 桥回的是单个 JSON **对象**（保存后的那一条）。
  static Map<String, dynamic> _row(String? raw, String method) {
    if (raw == null || raw.trim().isEmpty) {
      throw AiApiException(400, '本机 harness 的 $method 没有回内容');
    }
    final decoded = jsonDecode(raw);
    if (decoded is! Map) {
      throw AiApiException(400, '本机 harness 的 $method 回的形状不对（期望一个 JSON 对象）');
    }
    return Map<String, dynamic>.from(decoded);
  }
}

/// 选一个 Provider 来源：手机（Android）用**本机库**，其余平台用**后端库**。
///
/// 与 [openCredentialStore] / [openRunEventSource] 同一条判据、同一个理由（总纲 §5.3）：
/// 凭据走本机时，**清单也必须走本机** —— 否则手机上"能在本机配一条 provider"这件事
/// 永远差最后一步（清单还在桌面手上，离线时连建都建不了）。
ProviderStore openProviderStore({
  required AiApiClient api,
  MethodChannel? methodChannel,
  bool? forceLocal,
}) {
  final local = forceLocal ?? Platform.isAndroid;
  if (local) return DeviceProviderStore(channel: methodChannel);
  return RemoteProviderStore(api);
}
