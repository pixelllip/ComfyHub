import 'dart:io' show Platform;

import 'package:flutter/services.dart';

import '../models/ai_models.dart';
import 'ai_api_client.dart';
import 'run_event_source.dart';

/// 凭据（API Key）的**读写来源**（M6）。
///
/// 桌面与手机的凭据**不是同一份东西**，但界面是同一套：
///
/// | | 真源 | 说明 |
/// | --- | --- | --- |
/// | 桌面 | 后端托管文件 / 环境变量（`CredentialService`） | 值不回读（AIH-012） |
/// | 手机 | **本机 Android Keystore** | 桌面的 key **不下行**（总纲 §3.3：`ai_providers` 不同步），手机上必须单独填一把 |
///
/// 这条分叉以前没有：界面清一色打 `PUT /api/ai/providers/{id}/credentials`，
/// 于是手机上**根本没有任何入口**把 key 放进 Keystore —— 手机 harness 再完整也永远发不出
/// 一条消息（上游 401），而界面只会说"密钥已保存"，保存的其实是桌面那一份。
/// 这正是"安卓侧真正跑起来"缺的那一环。
///
/// 两条纪律照旧：
/// 1. **值只写不读**：这里没有任何"取出密钥"的方法，`statusOf` 只回"有没有 / 从哪来 / 能不能改"；
/// 2. **空 ref 是合法状态**（不是错误）：它表示"这条 provider 压根不需要密钥"，
///    与"配了但没有值"是两回事（桌面 `describe` 的三分支之一）。
abstract class CredentialStore {
  /// 手机（本机密钥库）还是桌面（后端托管）。
  bool get isLocal;

  /// 这条 provider 的凭据状态。
  ///
  /// 远端实现**直接回 `provider.credential`** —— 那份是后端随清单一起下发的，
  /// 再单独问一次只会多一次请求、还可能两次不一致。
  Future<AiCredentialStatus> statusOf(AiProvider provider);

  /// 写入一把新的密钥，返回写完之后的状态。
  Future<AiCredentialStatus> save(AiProvider provider, String value);

  /// 移除密钥，返回移除之后的状态。
  Future<AiCredentialStatus> clear(AiProvider provider);
}

/// 桌面实现：转发给 `AiApiClient`（后端托管）。
class RemoteCredentialStore implements CredentialStore {
  RemoteCredentialStore(this._api);

  final AiApiClient _api;

  @override
  bool get isLocal => false;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async => provider.credential;

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) =>
      _api.setCredential(provider.id, value);

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) => _api.removeCredential(provider.id);
}

/// 手机实现：`MethodChannel` → 本地 Kotlin harness → Android Keystore。
///
/// ⚠ **值不在这里解密、也不在这里读回**：桥上只有 `hasCredential` / `putCredential` /
/// `removeCredential` 三个动作，没有"取出来"的那个（与桌面同一条纪律）。
class DeviceCredentialStore implements CredentialStore {
  DeviceCredentialStore({MethodChannel? channel})
      : _method = channel ?? const MethodChannel(LocalHarnessRunEventSource.methodChannelName);

  /// 与 Kotlin 侧 `HarnessBridge.METHOD_CHANNEL` 同名（复用本地源的常量，改一处就够）。
  final MethodChannel _method;

  @override
  bool get isLocal => true;

  @override
  Future<AiCredentialStatus> statusOf(AiProvider provider) async {
    final ref = provider.credentialRef?.trim();
    // ref 为空 = 这条 provider 不需要密钥（与桌面 `describe` 的第一个分支同义）
    if (ref == null || ref.isEmpty) {
      return const AiCredentialStatus(configured: false, source: 'none', writable: false);
    }
    final has = await _method.invokeMethod<bool>('hasCredential', {'ref': ref}) ?? false;
    return AiCredentialStatus(
      configured: has,
      // `keystore` 是**手机侧专有**的来源名：它绝不能与 `managed` 混在一起 ——
      // 混了用户就会以为"桌面上配过、手机也能用"，而实际上手机上一把都没有。
      source: has ? 'keystore' : 'none',
      // 本机密钥库永远可写（要能改、也要能清掉一把配错的）
      writable: true,
    );
  }

  @override
  Future<AiCredentialStatus> save(AiProvider provider, String value) async {
    final ref = provider.credentialRef?.trim();
    if (ref == null || ref.isEmpty) {
      // 说清楚而不是静默写进一个没有名字的槽位（那把 key 之后谁也找不到）
      throw StateError('这条 Provider 没有凭据引用名（credentialRef），无法在本机保存密钥');
    }
    if (value.trim().isEmpty) {
      throw StateError('密钥不能为空（要清除请点「移除密钥」）');
    }
    await _method.invokeMethod<bool>('putCredential', {'ref': ref, 'value': value});
    return statusOf(provider);
  }

  @override
  Future<AiCredentialStatus> clear(AiProvider provider) async {
    final ref = provider.credentialRef?.trim();
    if (ref == null || ref.isEmpty) {
      return const AiCredentialStatus(configured: false, source: 'none', writable: false);
    }
    await _method.invokeMethod<bool>('removeCredential', {'ref': ref});
    return statusOf(provider);
  }
}

/// 选一个凭据来源：手机（Android）用**本机密钥库**，其余平台用**远端后端**。
///
/// 与 [openRunEventSource] 同一条判据、同一个理由（总纲 §5.3）：
/// 事件与持久化走本地时，凭据也必须走本地，否则"能聊"永远差最后一步。
CredentialStore openCredentialStore({
  required AiApiClient api,
  MethodChannel? methodChannel,
  bool? forceLocal,
}) {
  final local = forceLocal ?? Platform.isAndroid;
  if (local) return DeviceCredentialStore(channel: methodChannel);
  return RemoteCredentialStore(api);
}
