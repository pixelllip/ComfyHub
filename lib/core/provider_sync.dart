import 'dart:convert';

import '../models/ai_models.dart';
import 'ai_api_client.dart';
import 'provider_store.dart';

/// **手机从电脑同步 Provider 清单与模型目录**（`docs/pairing-design.md` §14.6）。
///
/// # 适用范围
///
/// `provider_store.dart` 把两端的清单分开：桌面写后端库、手机写本机库
/// （`ai_providers` / `ai_models` 是**本机表**，不在同步范围内）。桌面配置好的
/// Provider（协议 / 地址 / 模型能力声明）在手机上需要重新录入；能力声明
/// （模态 / 传输方式 / 思考档位）错一个字符的表现是"附件发不出去 / 思考参数不发"，
/// 两端都不报错。此模块提供**显式**的搬运通道。
///
/// # 走的端点（**不新增服务端接口**）
///
/// 就是既有的两条只读端点：`GET /api/ai/providers` + `GET /api/ai/providers/{id}/models`。
/// 它们本来就在"已配对设备可读"的范围里（闸门见 §11.4），加一条"导出"端点等于给同一份事实
/// 再开第二个形状 —— 两个形状就是两份实现，迟早漂移。一次同步 = `1 + N` 个请求。
///
/// # 语义：显式触发的一次导入，**不是自动合并**
///
/// | 情形 | 做什么 |
/// | --- | --- |
/// | 桌面有、本机没有 | **新建**（含 `credentialRef` —— 之后「从桌面同步密钥」才知道该取哪一把） |
/// | 两边同 id、内容不同 | **桌面那份覆盖**（显示名 / 协议 / baseURL / credentialRef / 端点信任档 / enabled） |
/// | 两边同 id、内容相同 | **一个字节都不写**（不白涨 revision，也不让聊天页白抖一次） |
/// | 只在手机上有的 | **保留不动**，如实报出来 |
/// | 已删除的 | **一个都不删**（桌面没有不等于手机上应当消失：本机清单可能由本机自行配置） |
///
/// 覆盖时提交的是**本机那一版 `revision`**：两端的库是两份，桌面那份 revision 与本机毫无关系；
/// 拿桌面的 revision 去写本机库只会撞乐观锁（"配置已被其他窗口修改…"），而这里明明只有
/// 一个写者。乐观锁语义本身照旧（本机库那一版仍是权威）。
///
/// # 三条不能破的纪律
///
/// 1. **整趟拿不到清单 → 一个字都不动**（不回退成"删掉本机那些"），失败如实说；
/// 2. **模型目录拉取失败绝不用空目录覆盖**（那会把"网关抖了一下"变成"模型全没了"）；
/// 3. **密钥值一个都不经过这里**：`credential` 只是"这台机器上有没有 key"的即时状态，
///    既不会被搬、也不会被写。取值仍走 §14.1 那条要 `allowCredentials` 的闸门。
class ProviderSync {
  ProviderSync({required this.desktop, required this.local});

  /// 桌面的后端客户端（`AiApiClient`，令牌默认由 `PairingTokenCache` 带上）。
  final AiApiClient desktop;

  /// 本机那份清单的来源（手机 = `DeviceProviderStore`）。
  final ProviderStore local;

  /// 跑一趟。**不抛**：网络 / 单条失败都折成 [ProviderSyncReport] 里的事实
  /// （这件事的失败方式恰好是"一部分成了、一部分没成"，只回 ok/失败就没法如实说）。
  Future<ProviderSyncReport> run() async {
    if (!local.isLocal) {
      // 桌面上后端库本身就是真源，没有"从别处取"这回事。
      // 走到这里只可能是接线错了 —— 如实说，而不是对着自己同步一遍还报"成功"。
      return const ProviderSyncReport.failed(
        '这台设备上的 Provider 就在本机后端，不需要从桌面同步。',
      );
    }

    final List<AiProvider> remote;
    try {
      remote = await desktop.listProviders();
    } catch (e) {
      // 整趟失败（没配对令牌 / 桌面不在线 / 闸门 403）：**一条都没动**
      return ProviderSyncReport.failed('读桌面 Provider 清单失败，${_describe(e)}');
    }

    final List<AiProvider> localRows;
    try {
      localRows = await local.listProviders();
    } catch (e) {
      return ProviderSyncReport.failed('读本机 Provider 清单失败，$e');
    }
    final byId = {for (final row in localRows) row.id: row};

    final created = <String>[];
    final updated = <String>[];
    final unchanged = <String>[];
    final problems = <ProviderSyncProblem>[];
    var modelsWritten = 0;

    for (final p in remote) {
      final id = p.id.trim();
      // 桌面那一行本身不完整 / 协议是手机不认识的：**不建一条看起来对、其实发不出去的**
      final invalid = _invalidReason(p);
      if (invalid != null) {
        problems.add(ProviderSyncProblem(id.isEmpty ? '（空 id）' : id, invalid));
        continue;
      }

      final existing = byId[id];
      try {
        if (existing == null) {
          await local.createProvider({
            'id': id,
            'displayName': p.displayName,
            'api': p.api,
            'baseURL': p.baseURL,
            // 空引用名**不写这个键**（写 null 与"没给"在两端是两件事，见 `ProviderJson`）
            if ((p.credentialRef ?? '').trim().isNotEmpty)
              'credentialRef': p.credentialRef,
            'endpointTrust': p.endpointTrust,
            'enabled': p.enabled,
          });
          created.add(id);
        } else if (_sameConfig(existing, p)) {
          unchanged.add(id);
        } else {
          await local.updateProvider(
            AiProvider(
              id: id,
              displayName: p.displayName,
              api: p.api,
              baseURL: p.baseURL,
              credentialRef: p.credentialRef,
              endpointTrust: p.endpointTrust,
              enabled: p.enabled,
              // ★ 本机那一版（见类注释）：桌面的 revision 与本机库无关
              revision: existing.revision,
              // 凭据状态是本机的即时事实，同步不碰它
              credential: existing.credential,
            ),
          );
          updated.add(id);
        }
      } catch (e) {
        // 单条失败只记这一条，其余照常 —— 一条坏数据不该让整趟白跑
        problems.add(ProviderSyncProblem(id, '写本机库失败，$e'));
        continue;
      }

      // ---- 模型目录：整份替换（AIH-011），但**只在真的拿到**才写 ----
      try {
        final remoteModels = await desktop.listModels(id);
        // providerId 一律以**本条 id** 为准（桌面回执里那一列是抄来的，信它会串行）
        final wanted = [
          for (final m in remoteModels)
            AiModel.fromJson({...m.toJson(), 'providerId': id}),
        ];
        final current = await local.listModels(id);
        // 内容一样就一个字节都不写：不然每次同步白涨一次 `updated_at`，
        // 聊天页那边也会因为"清单变了"白读一遍目录
        if (!_sameModels(current, wanted)) {
          await local.saveModels(id, wanted);
          modelsWritten += wanted.length;
        }
      } catch (e) {
        // ⚠ 这里**绝不能**回落成 `saveModels(id, const [])`：网关抖一下 = 模型全没了
        problems.add(
          ProviderSyncProblem(
            id,
            'Provider 已同步，但模型目录没同步上（本机那份没动），${_describe(e)}',
          ),
        );
      }
    }

    final remoteIds = {for (final p in remote) p.id.trim()};
    final localOnly = [
      for (final row in localRows)
        if (!remoteIds.contains(row.id)) row.id,
    ];

    return ProviderSyncReport(
      created: created,
      updated: updated,
      unchanged: unchanged,
      localOnly: localOnly,
      modelsWritten: modelsWritten,
      problems: problems,
    );
  }

  /// 桌面那一侧的错误：`AiApiException.toString()` 只有人话（**状态码不在里面**），
  /// 而 401 / 403 / 429 在这里的处置完全不同（令牌不对 / 闸门挡着 / 限速），
  /// 所以状态码要一起说。本机库那侧的 `AiApiException(400, …)` 里的 400 只是
  /// "配置类错误"的记号（不是 HTTP），所以**只在桌面调用处**用它。
  static String _describe(Object e) =>
      e is AiApiException ? 'HTTP ${e.statusCode}：${e.message}' : '$e';

  /// 桌面那一行能不能在本机上真的跑起来；不能就回一句人话（`null` = 可以）。
  static String? _invalidReason(AiProvider p) {
    if (p.id.trim().isEmpty) return '桌面那条没有 id';
    if (p.displayName.trim().isEmpty) return '桌面那条没有显示名';
    if (p.api.trim().isEmpty) return '桌面那条没有协议';
    if (p.baseURL.trim().isEmpty) return '桌面那条没有 Base URL';
    // 手机 harness 只实现这三种协议（`AiApi` / Kotlin `AiApiRef`）。不认识的**跳过**：
    // 建一条本机无法使用的 Provider，只会在发送时变成难以理解的上游错误。
    if (AiApi.parse(p.api) == null) return '手机不支持这个协议 ${p.api}';
    return null;
  }

  /// 两行的"配置"是否一致（**不含** revision / credential / 时间戳）。
  static bool _sameConfig(AiProvider a, AiProvider b) =>
      a.displayName == b.displayName &&
      a.api == b.api &&
      a.baseURL == b.baseURL &&
      (a.credentialRef ?? '').trim() == (b.credentialRef ?? '').trim() &&
      a.endpointTrust == b.endpointTrust &&
      a.enabled == b.enabled;

  /// 两份模型目录是否一致（按与后端逐字相同的 DTO 形状比对）。
  static bool _sameModels(List<AiModel> a, List<AiModel> b) {
    if (a.length != b.length) return false;
    final left = jsonEncode([for (final m in a) m.toJson()]);
    final right = jsonEncode([for (final m in b) m.toJson()]);
    return left == right;
  }
}

/// 一条"没同步成"的如实记录：哪个 Provider、为什么。
class ProviderSyncProblem {
  const ProviderSyncProblem(this.providerId, this.reason);

  final String providerId;
  final String reason;

  @override
  String toString() => '$providerId，$reason';
}

/// 一趟同步的结果（形状取自 `docs/pairing-design.md` §14.6）。
///
/// [error] 非空 = **整趟没开始就失败了**（那时 [created] / [updated] / [localOnly] 全空，
/// 本机一个字节都没动）。逐条失败进 [problems]，不影响其它条目。
class ProviderSyncReport {
  const ProviderSyncReport({
    this.error,
    this.created = const [],
    this.updated = const [],
    this.unchanged = const [],
    this.localOnly = const [],
    this.modelsWritten = 0,
    this.problems = const [],
  });

  const ProviderSyncReport.failed(String message) : this(error: message);

  final String? error;

  /// 本机新建的 id（桌面有、本机没有）。
  final List<String> created;

  /// 被桌面那份覆盖的 id（两边同 id、内容不同）。
  final List<String> updated;

  /// 两边同 id、内容一致 —— **一个字节都没写**。
  final List<String> unchanged;

  /// 只在手机上有的 id（**保留不动**；桌面没有 ≠ 本机该删）。
  final List<String> localOnly;

  /// 真正写进本机库的模型条数（内容没变的目录不重复写）。
  final int modelsWritten;

  /// 逐条失败 / 跳过（未知协议、写库失败、模型目录拉取失败…）。
  final List<ProviderSyncProblem> problems;

  bool get ok => error == null;

  /// 有没有真的动过本机库（界面据此决定要不要提示"刷新"）。
  bool get touched =>
      created.isNotEmpty || updated.isNotEmpty || modelsWritten > 0;
}
