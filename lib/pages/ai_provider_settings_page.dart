import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/api_client.dart' show ApiException;
import '../core/credential_store.dart';
import '../core/inbound_auth_api.dart';
import '../core/pairing_token.dart';
import '../core/provider_store.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import 'ai_tools_settings_page.dart';

/// AI 模型设置（AIH-006 / AIH-007 / AIH-010 / AIH-011 / AIH-012 / AIH-013）。
///
/// 三段式：Provider 列表 → 基本信息 → 凭据 + 模型目录。
/// 两条不能破的规则：
///  1. **API Key 输入框每次打开都是空的**，界面上只显示"已配置 / 未配置 / 来源"；
///     留空保存表示"不改已有密钥"，清除必须点「移除密钥」。
///  2. **能力必须显式声明**：不认识 model id，也不根据名字猜模态。
class AiProviderSettingsPage extends StatefulWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  /// 凭据来源。正式运行时按平台选（手机 = 本机密钥库）；测试注入假的。
  final CredentialStore? credentials;

  /// **Provider 清单与模型目录**的来源（M6 的另一半）。
  ///
  /// 正式运行时按平台选：**手机 = 本机库**（走桥，一条网络请求都不打）、桌面 = 后端库。
  /// 测试注入假的。
  ///
  /// ⚠ 它只管**这一页的清单 CRUD**；[api] 仍要传 —— 内置目录 / 能力目录两张维护卡，
  /// 以及桌面上才有的「连接测试 / 获取模型」都还走它。
  final ProviderStore? providers;

  /// `InboundAuthApi` 的工厂：**密钥上行**（「推给桌面」）要打 `/api/ai/credentials`。
  /// 测试注入带 MockClient 的那个，正式运行时按后端地址现建。
  final InboundAuthApi Function(String baseUrl)? authApiFactory;

  const AiProviderSettingsPage({
    super.key,
    this.api,
    this.credentials,
    this.providers,
    this.authApiFactory,
  });

  @override
  State<AiProviderSettingsPage> createState() => _AiProviderSettingsPageState();
}

/// 窄屏（手机）上这一页的三块内容 —— **一次只显示一块、各自整屏**。
///
/// 理由见 `_buildNarrow` 的注释：手机上不做上下分屏，点进去就是子界面。
enum _NarrowPane { list, detail, maintenance }

class _AiProviderSettingsPageState extends State<AiProviderSettingsPage> {
  late AiApiClient _api;

  /// **清单的来源**（provider / 模型目录）：手机 = 本机库（桥），桌面 = 后端库。
  ///
  /// 以前这一页的 CRUD 清一色打 `_api`（桌面 HTTP）—— 桌面不在线时手机上连
  /// "新建一个 provider / 改个 baseURL"都做不到，而本机库里那两张表**本来就是本机表**。
  late ProviderStore _store;

  /// 凭据的读写来源（M6）：桌面 = 后端托管；手机 = **本机密钥库**。
  ///
  /// 以前这里清一色打 `PUT /api/ai/providers/{id}/credentials`，于是手机上
  /// **没有任何入口**把 key 放进 Keystore —— harness 再完整也发不出消息，而界面还说
  /// "密钥已保存"（保存的是桌面那一份）。
  late CredentialStore _credentials;

  List<AiProvider> _providers = const [];
  List<AiModel> _models = const [];
  AiProvider? _selected;
  bool _loading = true;
  String? _error;
  String? _info;

  /// 内置模型目录的状态（只读预览，打开页面时拉一次）。
  AiBuiltinCatalogStatus? _builtin;
  bool _builtinBusy = false;

  /// 模型能力目录的在线更新状态（custom > remote > 内置，2026-09-23）。
  AiCapabilityStatus? _cap;
  bool _capBusy = false;
  bool _probeBusy = false;

  /// **窄屏（手机）当前显示哪一块**。桌面用不上它（那边是左右分栏）。
  ///
  /// 见 [`_buildNarrow`]：手机上"列表 / 详情 / 维护"各自整屏，一次只显示一块 ——
  /// 用 `enum` 而不是 `Navigator.push`，是为了让三块共用同一份 `State`
  /// （推成独立路由的话，父级 `setState` 不会重建它，卡片状态会停在推入那一刻）。
  _NarrowPane _pane = _NarrowPane.list;

  @override
  void initState() {
    super.initState();
    // ⚠ 这三个**必须在 initState 里就绪**（不能等 post-frame）：
    //   `build()` 第一帧就要读 `_store.isLocal` —— 手机上那句"只在这台手机上"的提示、
    //   以及"连接测试 / 获取模型"两个按钮的禁用都靠它。放进 post-frame 回调里的后果是
    //   首帧 `LateInitializationError`（而 `context.read` 在 initState 里是安全的：
    //   它不注册依赖，只是取一次值）。
    _api = widget.api ?? AiApiClient(context.read<SettingsStore>().baseUrl);
    _credentials = widget.credentials ?? openCredentialStore(api: _api);
    // 手机上这份清单**只在本机**（不出网）；桌面那份仍然在后端。
    _store = widget.providers ?? openProviderStore(api: _api);
    // 真正的加载仍然放到首帧之后（与以前一致：不在 build 期间 setState）
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _reload();
      _loadBuiltin();
      _loadCapabilityCatalog();
    });
  }

  @override
  void dispose() {
    super.dispose();
  }

  /// 内置目录状态：拉不到只影响这张卡（要如实显示失败原因，不能静默）。
  Future<void> _loadBuiltin() async {
    try {
      final status = await _api.builtinStatus();
      if (mounted) setState(() => _builtin = status);
    } catch (e) {
      if (mounted) {
        setState(() => _builtin = AiBuiltinCatalogStatus(error: '$e'));
      }
    }
  }

  /// 能力目录状态：拉不到如实显示在卡上。
  Future<void> _loadCapabilityCatalog() async {
    try {
      final status = await _api.capabilityStatus();
      if (mounted) setState(() => _cap = status);
    } catch (e) {
      if (mounted) setState(() => _cap = AiCapabilityStatus(error: '$e'));
    }
  }

  /// 立即更新在线能力目录（models.dev）。失败不是异常：后端会把原因放在 lastMessage。
  Future<void> _refreshCapabilityCatalog() async {
    setState(() => _capBusy = true);
    try {
      final status = await _api.refreshCapabilityCatalog();
      if (!mounted) return;
      setState(() => _cap = status);
      _notify(status.lastMessage ?? '已请求更新能力目录。');
    } catch (e) {
      if (mounted) _notify('更新能力目录失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _capBusy = false);
    }
  }

  /// 开/关实测校准。
  Future<void> _toggleProbe(bool on) async {
    setState(() => _capBusy = true);
    try {
      final status = await _api.setProbeEnabled(enabled: on);
      if (!mounted) return;
      setState(() => _cap = status);
      _notify(status.lastMessage ?? (on ? '已允许实测校准' : '已关闭实测校准'));
    } catch (e) {
      if (mounted) _notify('更新校准开关失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _capBusy = false);
    }
  }

  /// 校准思考档位：**先确认再发请求**（校准发真实请求，可能产生少量费用）。
  Future<void> _probeThinkingLadders() async {
    if (_cap?.probeEnabled != true) {
      _notify('实测校准未开启（先打开下面的开关）。', error: true);
      return;
    }
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('校准思考档位？'),
        content: const Text(
          '会对每个 Provider 最多 8 个支持推理的模型，从最高档往下发极小的真实请求'
          '返回网关接受的实际挡位。\n\n'
          '可能产生少量费用；手动声明过的模型不会被覆盖。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('开始校准')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    setState(() => _probeBusy = true);
    try {
      final response = await _api.probeCapabilities();
      if (!mounted) return;
      await _showProbeResults(response);
      if (!response.ok) _notify(response.message, error: true);
    } catch (e) {
      if (mounted) _notify('校准失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _probeBusy = false);
    }
  }

  /// 校准结果逐行摆出来（成功 / 跳过 / 失败都说清楚，不美化）。
  Future<void> _showProbeResults(AiCapabilityProbeResponse response) async {
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('思考档位校准结果'),
        content: SizedBox(
          width: 420,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(response.message, style: Theme.of(ctx).textTheme.bodySmall),
                const SizedBox(height: 8),
                for (final o in response.outcomes)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(
                          o.skipped
                              ? Icons.skip_next_outlined
                              : (o.ok ? Icons.check_circle_outline : Icons.error_outline),
                          size: 15,
                          color: o.skipped
                              ? Theme.of(ctx).colorScheme.outline
                              : (o.ok ? Colors.green.shade600 : Theme.of(ctx).colorScheme.error),
                        ),
                        const SizedBox(width: 6),
                        Expanded(
                          child: Text(
                            '${o.modelId.isEmpty ? o.providerId : o.modelId} — ${o.message}'
                            '${o.acceptedTop == null ? '' : '（最高 ${o.acceptedTop}）'}',
                            style: Theme.of(ctx).textTheme.bodySmall,
                          ),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
        ),
        actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('好的'))],
      ),
    );
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      _providers = await _store.listProviders();
      // 手机上"这台设备有没有 key"只有本机密钥库知道：不覆盖就会显示桌面那份状态
      if (_credentials.isLocal) {
        final out = <AiProvider>[];
        for (final p in _providers) {
          try {
            out.add(p.withCredential(await _credentials.statusOf(p)));
          } catch (_) {
            out.add(p);
          }
        }
        _providers = out;
      }
      if (_selected != null) {
        _selected = _providers.where((p) => p.id == _selected!.id).firstOrNull;
      }
      _models = _selected == null ? const [] : await _store.listModels(_selected!.id);
    } catch (e) {
      _error = '$e';
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _select(AiProvider? p) async {
    setState(() {
      _selected = p;
      _models = const [];
      _info = null;
      _error = null;
    });
    if (p != null) {
      try {
        _models = await _store.listModels(p.id);
      } catch (e) {
        setState(() => _error = '$e');
      }
      if (mounted) setState(() {});
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final wide = MediaQuery.sizeOf(context).width >= 900;
    // 手机上**不做上下分屏**（用户反馈原话："上下分屏难受，而且有些东西在手机上应该表现为子界面"）
    if (!wide) return _buildNarrow(theme);

    final list = _ProviderList(
      providers: _providers,
      selectedId: _selected?.id,
      loading: _loading,
      // 手机上要如实说一句"这份清单只在本机"（§14.4：两端各一份、不自动合并）
      local: _store.isLocal,
      onSelect: _select,
      onCreate: _createProvider,
      onDelete: _deleteProvider,
    );

    final detail = _selected == null
        ? Center(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Text(
                '选择一个 **Provider** 查看详情，或新建一个。\n填入的 API KEY 将不会再次显示。',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          )
        : _ProviderDetail(
            key: ValueKey(_selected!.id + _selected!.revision.toString()),
            api: _api,
            credentials: _credentials,
            providers: _store,
            authApiFactory: widget.authApiFactory,
            provider: _selected!,
            models: _models,
            info: _info,
            error: _error,
            onEdit: () => _editProvider(_selected!),
            onChanged: (msg) async {
              setState(() => _info = msg);
              await _reload();
            },
          );

    return Scaffold(
      appBar: AppBar(
        title: const Text('AI 模型与凭据'),
        actions: [
          // 工具权限（M4）单独一页：白名单 / 逐工具 allow-ask-deny / 预算
          TextButton.icon(
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute<void>(builder: (_) => const AiToolsSettingsPage()),
            ),
            icon: const Icon(Icons.shield_outlined, size: 18),
            label: const Text('AI 工具权限'),
          ),
          const SizedBox(width: 8),
        ],
      ),
      body: Column(
        children: [
          // 没有选中 Provider 时，错误也要看得见（新建失败最常发生在这种情况下）
          if (_error != null) _errorBanner(theme),
          // 内置模型目录（M6）：冻结副本 → 库里的对齐入口，永远可见
          //
          // ⚠ 这两张卡（+ 上面的错误横幅）在手机上会占掉大半屏。桌面窗口够高，
          // 但为了不让"窗口被拖小"时也画到屏幕外，这里给一个**高度上限**并让它自己滚
          // （真机验收实测过 `BOTTOM OVERFLOWED BY 57 PIXELS` 就是这么来的）。
          ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.sizeOf(context).height * 0.42,
            ),
            child: SingleChildScrollView(
              child: Column(
                children: [
                  _builtinCard(),
                  _capabilityCard(),
                ],
              ),
            ),
          ),
          Expanded(
            child: Row(
              children: [
                SizedBox(width: 280, child: list),
                const VerticalDivider(width: 1),
                Expanded(child: detail),
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// 顶部那条错误横幅（窄屏三个面板共用同一份渲染）。
  Widget _errorBanner(ThemeData theme) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        color: theme.colorScheme.errorContainer,
        child: Row(
          children: [
            Icon(Icons.error_outline, size: 18, color: theme.colorScheme.onErrorContainer),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                _error!,
                style: theme.textTheme.bodySmall
                    ?.copyWith(color: theme.colorScheme.onErrorContainer),
              ),
            ),
            IconButton(
              tooltip: '关闭',
              icon: const Icon(Icons.close, size: 16),
              onPressed: () => setState(() => _error = null),
            ),
          ],
        ),
      );

  /// **手机上的这一页**：三块内容各自**整屏**，带返回箭头，一次只显示一块。
  ///
  /// 用户反馈原话："上下分屏难受，而且有些东西在手机上应该表现为子界面。"
  /// 所以：
  ///
  /// | 面板 | 里面是什么 | 怎么进出 |
  /// | --- | --- | --- |
  /// | `list` | Provider 一览（主界面）+ 新建 | 点某一条 → `detail` |
  /// | `detail` | 密钥 / 模型目录 / 连接测试 | 左上角返回 |
  /// | `maintenance` | 「模型目录维护」那两张卡 | 右上角扳手 → 返回 |
  ///
  /// ⚠ **刻意不 `Navigator.push` 开新路由**：三块共用同一份 `State`
  /// （`_builtin` / `_cap` / `_models` / `_error` 全在这里）。推成独立路由之后，
  /// 父级 `setState` **不会**重建那个路由 —— 卡片上的状态会停在推入那一刻，
  /// 表现是"东西明明加载完了却还显示空的、改完不回显"。一个 `enum` 就没有这个缝。
  Widget _buildNarrow(ThemeData theme) {
    // 详情面板的前提是"有一个选中的 Provider"：删掉之后自动退回列表
    final pane = (_selected == null && _pane == _NarrowPane.detail) ? _NarrowPane.list : _pane;
    // 子面板上**接管系统返回**（用户实测报的 bug：划一下返回手势，整页直接退回设置首页了）。
    // 面板是同一个 State 里换 enum，不是路由 —— 所以系统返回默认弹的是"整个页面"。
    // `canPop: false` 把这一层拦住，由我们退回列表；在列表上时照旧放行（返回 = 退出这一页）。
    final interceptBack = pane != _NarrowPane.list;
    Widget withBackGuard(Widget child) => PopScope(
          canPop: !interceptBack,
          onPopInvokedWithResult: (didPop, _) {
            if (didPop) return;
            setState(() => _pane = _NarrowPane.list);
          },
          child: child,
        );

    switch (pane) {
      case _NarrowPane.maintenance:
        return withBackGuard(
          Scaffold(
            appBar: AppBar(
              leading: IconButton(
                tooltip: '返回',
                icon: const Icon(Icons.arrow_back),
                onPressed: () => setState(() => _pane = _NarrowPane.list),
              ),
              title: const Text('模型目录维护'),
            ),
            body: SingleChildScrollView(
              child: Column(
                children: [
                  const Padding(
                    padding: EdgeInsets.fromLTRB(16, 12, 16, 0),
                    child: Text('这两件事都在电脑上做更合适：给模型补能力声明、实测校准思考档位（后者会发真实请求、可能产生少量费用）。'),
                  ),
                  _builtinCard(),
                  _capabilityCard(),
                ],
              ),
            ),
          ),
        );
      case _NarrowPane.detail:
        final provider = _selected!;
        return withBackGuard(
          Scaffold(
            appBar: AppBar(
              leading: IconButton(
                tooltip: '返回',
                icon: const Icon(Icons.arrow_back),
                onPressed: () => setState(() => _pane = _NarrowPane.list),
              ),
              title: Text(provider.displayName, maxLines: 1, overflow: TextOverflow.ellipsis),
            ),
            body: Column(
              children: [
                if (_error != null) _errorBanner(theme),
                Expanded(
                  child: _ProviderDetail(
                    key: ValueKey(provider.id + provider.revision.toString()),
                    api: _api,
                    credentials: _credentials,
                    providers: _store,
                    authApiFactory: widget.authApiFactory,
                    provider: provider,
                    models: _models,
                    info: _info,
                    error: _error,
                    onEdit: () => _editProvider(provider),
                    onChanged: (msg) async {
                      setState(() => _info = msg);
                      await _reload();
                    },
                  ),
                ),
              ],
            ),
          ),
        );
      case _NarrowPane.list:
        return Scaffold(
          appBar: AppBar(
            title: const Text('AI 模型与凭据'),
            actions: [
              IconButton(
                tooltip: '模型目录维护',
                icon: const Icon(Icons.tune),
                onPressed: () => setState(() => _pane = _NarrowPane.maintenance),
              ),
              IconButton(
                tooltip: 'AI 工具权限',
                icon: const Icon(Icons.shield_outlined),
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute<void>(builder: (_) => const AiToolsSettingsPage()),
                ),
              ),
            ],
          ),
          body: Column(
            children: [
              if (_error != null) _errorBanner(theme),
              Expanded(
                child: _ProviderList(
                  providers: _providers,
                  selectedId: _selected?.id,
                  loading: _loading,
                  local: _store.isLocal,
                  onSelect: (p) async {
                    await _select(p);
                    // 手机上点一条 = **进子界面**（不是"在下面换个详情"）
                    if (mounted && p != null) setState(() => _pane = _NarrowPane.detail);
                  },
                  onCreate: _createProvider,
                  onDelete: _deleteProvider,
                ),
              ),
            ],
          ),
        );
    }
  }

  /// 「内置模型目录」卡片（桌面平铺 / 手机折叠项里都用这一个，免得两处漂移）。
  Widget _builtinCard() => _BuiltinCatalogCard(
        status: _builtin,
        busy: _builtinBusy,
        onRefresh: _loading ? null : _loadBuiltin,
        onAlignCapabilities: _alignBuiltinCapabilities,
        onAddMissing: _addMissingBuiltinModels,
      );

  /// 「模型能力目录」卡片（同上）。
  Widget _capabilityCard() => _CapabilityCatalogCard(
        status: _cap,
        busy: _capBusy,
        probeBusy: _probeBusy,
        onRefreshStatus: _loading ? null : _loadCapabilityCatalog,
        onUpdateRemote: _refreshCapabilityCatalog,
        onProbeToggle: _toggleProbe,
        onProbe: _probeThinkingLadders,
      );

  /// 内置模型目录（M6）。
  ///
  /// 顺序不能颠倒：**先 GET status 预览 → 让用户确认 → 再 POST sync**。
  /// 直接 POST 等于不问就改库里的能力声明。
  Future<void> _alignBuiltinCapabilities() async {
    final AiBuiltinCatalogStatus status;
    try {
      status = await _api.builtinStatus();
    } catch (e) {
      _notify('读取内置目录状态失败：$e', error: true);
      return;
    }
    if (!mounted) return;
    if (!status.ok) {
      _notify('内置目录不可用：${status.error}', error: true);
      return;
    }
    final count = status.divergentCount;
    if (count == 0) {
      _notify('内置目录与库里的能力声明已经一致，不需要对齐。');
      setState(() => _builtin = status);
      return;
    }
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('用内置目录对齐模型能力？'),
        content: Text(
          '会按内置目录（版本 ${status.version}）重写 $count 个模型的能力声明：'
          '输入模态、工具 / 推理能力、思考档位、思考方言与上下文。\n\n'
          '不会新增或删除模型，也不会改展示名与启用状态。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('对齐')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    await _syncBuiltin('refresh-capabilities');
  }

  /// 补齐缺失模型：纯新增，不改已有行，所以不必弹确认框。
  Future<void> _addMissingBuiltinModels() => _syncBuiltin('add-missing');

  Future<void> _syncBuiltin(String mode) async {
    setState(() => _builtinBusy = true);
    try {
      final result = await _api.syncBuiltinCatalog(mode: mode);
      if (!mounted) return;
      setState(() => _builtin = result);
      if (!result.ok) {
        _notify('同步失败：${result.error}', error: true);
      } else if (mode == 'add-missing') {
        _notify('已补齐 ${result.added} 个缺失模型（保留 ${result.kept} 个已有一致的模型）。');
      } else {
        _notify('已对齐 ${result.updated} 个模型的能力声明。');
      }
      // 库里的模型变了，右侧的模型目录要跟着刷新
      await _reload();
    } catch (e) {
      if (!mounted) return;
      setState(() => _builtin = AiBuiltinCatalogStatus(error: '$e'));
      _notify('同步失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _builtinBusy = false);
    }
  }

  /// 统一的用户可见反馈：**任何失败都要弹出来**。
  ///
  /// 之前的坑：错误只渲染在右侧详情面板里，而详情面板要先选中一个 Provider 才显示 ——
  /// 于是"新建 Provider 失败"在界面上完全没有反应，用户看到的就是"点了没加上"。
  void _notify(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          // 长错误（比如"更新能力目录失败：接口不存在: /api/ai/capabilities/refresh"）
          // 在手机上会超出浮层允许的高度，画成 `BOTTOM OVERFLOWED BY 14 PIXELS`
          // （2026-09-24 真机验收用户报的就是它）。套一层滚动容器：
          // 高度不够就**滚动**，而不是把内容画到框外面去。消息一个字都不改。
          content: SingleChildScrollView(child: Text(message)),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error ? Theme.of(context).colorScheme.errorContainer : null,
        ),
      );
  }

  Future<void> _createProvider() async {
    final draft = await showDialog<_ProviderDraft>(
      context: context,
      builder: (_) => const _ProviderDialog(),
    );
    if (draft == null) return;
    try {
      final created = await _store.createProvider({
        'id': draft.id,
        'displayName': draft.displayName,
        'api': draft.api.wire,
        'baseURL': draft.baseURL,
        'credentialRef': draft.credentialRef,
        // 不选就是 null，交给后端按地址推断（公网 / 本机），避免"默认只允许本机"把公网地址挡掉
        'endpointTrust': draft.trust?.wire,
      });
      await _reload();
      await _select(created);
      // 手机上建完直接进它的详情（不然用户还得在列表里再找一遍刚建的那条）
      if (mounted && MediaQuery.sizeOf(context).width < 900) {
        setState(() => _pane = _NarrowPane.detail);
      }
      _notify('已创建 Provider「${created.displayName}」，接下来可以填 API Key 和模型目录。');
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('创建失败：$e', error: true);
    }
  }

  /// **编辑一条已有的**（用户要的"改个 baseURL"）。
  ///
  /// 三条与桌面 POST/PUT 对齐的纪律：
  ///  1. **ID 不给改**（对话框里锁住）：桌面 `PUT` 的判据是"body 里的 id 不许与路径不同"，
  ///     `POST` 撞到已存在的 id 直接报"Provider ID 已存在"；
  ///  2. **带 revision 提交**（乐观锁 AIH-007）：手机侧库里只有一个写者，但**语义照旧** ——
  ///     桌面那份清单可能同时被别的窗口改过，而两端看到的错误要是同一句话；
  ///  3. 提交走 [_store]（手机 = 本机库，**不出网**）。
  Future<void> _editProvider(AiProvider p) async {
    final draft = await showDialog<_ProviderDraft>(
      context: context,
      builder: (_) => _ProviderDialog(existing: p),
    );
    if (draft == null) return;
    try {
      final updated = await _store.updateProvider(AiProvider(
        id: p.id,
        displayName: draft.displayName,
        api: draft.api.wire,
        baseURL: draft.baseURL,
        credentialRef: draft.credentialRef,
        // 用户在对话框里选了信任档就用它；选「自动」就保持原来那个值
        // （手机侧**不做推断**，见 `HarnessStore.saveProvider` 的注释）
        endpointTrust: draft.trust?.wire ?? p.endpointTrust,
        enabled: p.enabled,
        revision: p.revision,
        credential: p.credential,
      ));
      await _reload();
      await _select(updated);
      _notify('已保存 Provider「${updated.displayName}」（revision ${updated.revision}）。');
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('保存失败：$e', error: true);
    }
  }

  Future<void> _deleteProvider(AiProvider p) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('删除 Provider「${p.displayName}」？'),
        content: const Text('已有会话会保留它的 Provider / 模型快照，但不能再继续发送。密钥不会被自动删除。'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('删除')),
        ],
      ),
    );
    if (ok != true) return;
    try {
      await _store.deleteProvider(p.id);
      if (_selected?.id == p.id) _selected = null;
      await _reload();
      _notify('已删除 Provider「${p.displayName}」');
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('删除失败：$e', error: true);
    }
  }
}

class _ProviderList extends StatelessWidget {
  final List<AiProvider> providers;
  final String? selectedId;
  final bool loading;

  /// 这份清单是不是**本机**的（手机）：是的话要说一句"桌面看不到它"。
  final bool local;
  final Future<void> Function(AiProvider?) onSelect;
  final Future<void> Function() onCreate;
  final Future<void> Function(AiProvider) onDelete;

  const _ProviderList({
    required this.providers,
    required this.selectedId,
    required this.loading,
    required this.local,
    required this.onSelect,
    required this.onCreate,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
          child: FilledButton.icon(
            onPressed: () => onCreate(),
            icon: const Icon(Icons.add, size: 18),
            label: const Text('新建 Provider'),
          ),
        ),
        // ⚠ **如实**：手机上这一页管的是**本机那份清单**（`ai_providers` / `ai_models`
        //    是手机的本机表），它与桌面那份**各归各、不自动合并** —— 同步的只有密钥明文
        //    （用户显式点「推给桌面」那一个动作，`docs/pairing-design.md` §14.4）。
        //    不写这一句，用户建完会以为桌面也能用（而桌面那边压根没有这条 provider）。
        if (local)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
            child: _Note(
              text: '这些 Provider 只在这台手机上（存在本机库里），桌面看不到它们；'
                  '两端的清单各归各，不会自动合并。桌面不在线时也能在这里新建 / 编辑 / 删除、改模型目录。',
              error: false,
            ),
          ),
        if (loading) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: providers.isEmpty
              ? Center(
                  child: Text('还没有 Provider',
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline)),
                )
              : ListView.builder(
                  itemCount: providers.length,
                  itemBuilder: (context, i) {
                    final p = providers[i];
                    return ListTile(
                      dense: true,
                      selected: p.id == selectedId,
                      leading: _CredentialDot(status: p.credential),
                      title: Text(p.displayName, maxLines: 1, overflow: TextOverflow.ellipsis),
                      subtitle: Text('${p.apiLabel} · ${p.endpointTrust}',
                          style: theme.textTheme.labelSmall),
                      onTap: () => onSelect(p),
                      trailing: IconButton(
                        tooltip: '删除',
                        icon: const Icon(Icons.delete_outline, size: 18),
                        onPressed: () => onDelete(p),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

/// 绿点 = 已配置，红点 = 缺失，灰点 = 由环境变量提供（只读）。
class _CredentialDot extends StatelessWidget {
  final AiCredentialStatus status;
  const _CredentialDot({required this.status});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = !status.configured
        ? theme.colorScheme.error
        : (status.source == 'env' ? theme.colorScheme.outline : Colors.green);
    return Tooltip(
      message: status.label,
      child: Icon(Icons.circle, size: 10, color: color),
    );
  }
}

class _ProviderDetail extends StatefulWidget {
  final AiApiClient api;
  final CredentialStore credentials;

  /// 清单来源（手机 = 本机库）：**模型目录的保存**走它，连接的测试/发现看它的 [ProviderStore.supportsProbe]。
  final ProviderStore providers;
  final InboundAuthApi Function(String baseUrl)? authApiFactory;
  final AiProvider provider;
  final List<AiModel> models;
  final String? info;
  final String? error;

  /// 「编辑 Provider」（改显示名 / 协议 / baseURL / 信任档 / 凭据引用名）。
  /// **不带 revision 的修改不在这里** —— 编辑时用的是这一行的 revision（乐观锁）。
  final Future<void> Function() onEdit;
  final Future<void> Function(String message) onChanged;

  const _ProviderDetail({
    super.key,
    required this.api,
    required this.credentials,
    required this.providers,
    this.authApiFactory,
    required this.provider,
    required this.models,
    required this.info,
    required this.error,
    required this.onEdit,
    required this.onChanged,
  });

  @override
  State<_ProviderDetail> createState() => _ProviderDetailState();
}

class _ProviderDetailState extends State<_ProviderDetail> {
  /// 密钥输入框**永远是空字符串**：只写不读，不留任何回显可能（AIH-012）。
  final _key = TextEditingController();
  bool _busy = false;
  AiProviderTestResult? _testResult;
  final _models = <AiModel>[];

  /// 本组件内的反馈统一走 SnackBar：任何一步失败都要看得见。
  void _toast(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          content: Text(message),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error ? Theme.of(context).colorScheme.errorContainer : null,
        ),
      );
  }

  @override
  void initState() {
    super.initState();
    _models.addAll(widget.models);
  }

  /// 父级把模型目录加载完之后会重建本组件（key 带 revision）。
  ///
  /// **必须有这一条**：只靠 `initState` 初始化的话，父级异步补上目录时
  /// 本地 `_models` 还停在空列表，界面就一直显示"还没有模型" ——
  /// 这正是"获取完可用模型并加入后，退出重进看不到模型"的成因。
  /// 用户在本地已经改过（还没保存）时**不要**用父级数据覆盖。
  @override
  void didUpdateWidget(_ProviderDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (_dirty) return;
    if (widget.provider.id != oldWidget.provider.id ||
        !_sameModels(widget.models, oldWidget.models)) {
      setState(() {
        _models
          ..clear()
          ..addAll(widget.models);
      });
    }
  }

  /// 本地有未保存的改动（增删模型、改能力勾选）。
  bool _dirty = false;

  /// 任何本地模型改动都要置脏：否则父级重建时会把用户没保存的编辑冲掉。
  void _markDirty() => _dirty = true;

  static bool _sameModels(List<AiModel> a, List<AiModel> b) {
    if (identical(a, b)) return true;
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i].id != b[i].id) return false;
    }
    return true;
  }

  @override
  void dispose() {
    _key.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = widget.provider;

    // 头部（固定几条）+ 模型卡片。**必须用 builder 懒构建**：
    // 以前是 `children: [..., for (var i...) _modelTile(...)]`，
    // 每个模型卡里有 6 个 FilterChip + 一个下拉框，几十个模型全在首帧就建出来，
    // 滚动时整个列表跟着重建 —— 这就是"AI 模型与凭据界面滑动卡顿"的主因。
    final header = <Widget>[
      Row(
        children: [
          Expanded(child: Text(p.displayName, style: theme.textTheme.titleLarge)),
          // 「改个 baseURL / 协议 / 信任档」的路（用户要的"独立配置 provider"）：
          // 两端都走同一条 store（手机 = 本机库、桌面 = 后端），提交时带 revision（乐观锁）。
          OutlinedButton.icon(
            onPressed: _busy ? null : () => widget.onEdit(),
            icon: const Icon(Icons.edit_outlined, size: 18),
            label: const Text('编辑 Provider'),
          ),
        ],
      ),
      const SizedBox(height: 4),
      Text('${p.id} · ${p.apiLabel} · revision ${p.revision}',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
      const SizedBox(height: 6),
      SelectableText(p.baseURL, style: theme.textTheme.bodySmall),
      const SizedBox(height: 4),
      Text('端点信任级别：${p.endpointTrust}（保存时校验，公网必须是 https）',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
      if (widget.info != null) ...[
        const SizedBox(height: 10),
        _Note(text: widget.info!, error: false),
      ],
      if (widget.error != null) ...[
        const SizedBox(height: 10),
        _Note(text: widget.error!, error: true),
      ],
      const SizedBox(height: 16),
      Row(
        children: [
          Text('连接测试', style: theme.textTheme.titleSmall),
          const SizedBox(width: 10),
          OutlinedButton.icon(
            // 手机上**还不能**：这一步要由本机自己向上游发请求（用本机密钥 + 端点信任档那套），
            // 而桌面那一份 `provider_probe` 手机侧还没实现。禁用 + 下面那句话，
            // 比让用户点下去拿到一句"Provider 不存在: x"（桌面不认识手机这条 id）诚实得多。
            onPressed: (_busy || !widget.providers.supportsProbe) ? null : _test,
            icon: const Icon(Icons.network_check, size: 18),
            label: const Text('测试连接'),
          ),
        ],
      ),
      if (!widget.providers.supportsProbe)
        Text(
          '连接测试与「获取模型」目前只在桌面可用（要由那一端自己向上游发请求）；'
          '这台手机上还做不到，所以这两个按钮先禁用着。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        )
      else
        Text(
          '测试只发一次请求，**10 秒**超时，且密钥不进日志。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
      if (_testResult != null) ...[
        const SizedBox(height: 8),
        _Note(text: _testResult!.display, error: !_testResult!.ok),
      ],
      const SizedBox(height: 16),
      Text('API Key', style: theme.textTheme.titleSmall),
      const SizedBox(height: 6),
      Row(
        children: [
          _CredentialDot(status: p.credential),
          const SizedBox(width: 6),
          Text(p.credential.label, style: theme.textTheme.bodySmall),
        ],
      ),
      const SizedBox(height: 8),
      if (p.credential.source == 'env')
        Text('该凭据由环境变量 ${p.credentialRef} 提供，是只读的，不能在应用内修改或移除。',
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline))
      else ...[
        // 手机上密钥存在**本机**（Android Keystore），桌面那份不会同步过来 ——
        // 不说清楚，用户会以为"桌面配过就够了"，然后每发一条都失败（M6）
        if (widget.credentials.isLocal)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Text(
              '这一条保存在本机密钥库，**只在这台手机上生效**。想让桌面也用这一把，点下面的「推给桌面」（值会离开本机，桌面必须已允许这台设备改密钥）。反过来「从桌面同步密钥」在「远程访问与配对」那张卡里。',
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
        TextField(
          controller: _key,
          obscureText: true,
          autocorrect: false,
          enableSuggestions: false,
          decoration: const InputDecoration(
            labelText: '粘贴密钥（留空表示不修改已有密钥）。',
            helperText: '粘贴您的**密钥**',
            border: OutlineInputBorder(),
            isDense: true,
          ),
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            FilledButton.icon(
              onPressed: _busy ? null : _saveKey,
              icon: const Icon(Icons.key, size: 18),
              label: const Text('保存密钥'),
            ),
            const SizedBox(width: 8),
            OutlinedButton.icon(
              onPressed: _busy || !p.credential.configured ? null : _removeKey,
              icon: const Icon(Icons.delete_outline, size: 18),
              label: const Text('移除密钥'),
            ),
            // 密钥上行（§14.2）：只有"手机上、且有 ref、且本机真有这把"才出现。
            // 桌面自己就是真源，没有"推给桌面"这回事（`RemoteCredentialStore.reveal` 直接抛）。
            if (widget.credentials.isLocal && (p.credentialRef ?? '').trim().isNotEmpty) ...[
              const SizedBox(width: 8),
              OutlinedButton.icon(
                onPressed: _busy || !p.credential.configured ? null : _pushKeyToDesktop,
                icon: const Icon(Icons.upload_outlined, size: 18),
                label: const Text('推给桌面'),
              ),
            ],
          ],
        ),
      ],
      const Divider(height: 32),
      Row(
        children: [
          Text('模型目录', style: theme.textTheme.titleSmall),
          const Spacer(),
          TextButton.icon(
            // 同上：手机上还做不到（要本机自己去拉上游的 /models）
            onPressed: (_busy || !widget.providers.supportsProbe) ? null : _discover,
            icon: const Icon(Icons.cloud_download_outlined, size: 18),
            label: const Text('获取可用模型'),
          ),
          TextButton.icon(
            onPressed: _busy ? null : _addModel,
            icon: const Icon(Icons.add, size: 18),
            label: const Text('手动添加'),
          ),
        ],
      ),
      Text(
        '本 harness 不按模型名字猜能力。',
        style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
      ),
      const SizedBox(height: 8),
      if (_models.isEmpty) Text('还没有模型', style: theme.textTheme.bodySmall),
    ];
    final count = _models.length;

    // 列表结构改成 **固定行高 + 编辑弹窗**（用户报的 bug ③ 卡顿 / ④ 滚动条不准）：
    //
    //   · 懒构建的 `ListView` / `SliverList` 只能用"**已布局**子项的平均高度"估算
    //     `maxScrollExtent`。卡片高矮不一时（支持推理的卡片多出 7 个档位 chip），
    //     越往下滚估算值越大 —— 实测 69 个模型时从 3512 一路涨到 21318，
    //     滚动条滑块于是从 17.9% 缩到 3.5%，位置也全错。
    //   · `SliverFixedExtentList` 的 extent 是**算出来的常数**，首帧就精确，
    //     滚动条不再跳；每行也不再是"7 个 FilterChip + 下拉框"，快速滑动不掉帧。
    //
    // 能力编辑没有丢：点一行（或右侧的调节按钮）打开编辑弹窗，见 [_ModelEditorDialog]。
    return Column(
      children: [
        Expanded(
          child: CustomScrollView(
            slivers: [
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
                sliver: SliverToBoxAdapter(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: header),
                ),
              ),
              SliverFixedExtentList(
                itemExtent: kModelRowHeight,
                delegate: SliverChildBuilderDelegate(
                  (context, i) => RepaintBoundary(child: _modelRow(theme, i)),
                  childCount: count,
                ),
              ),
              const SliverToBoxAdapter(child: SizedBox(height: 16)),
            ],
          ),
        ),
        // 保存按钮常驻底部：69 个模型不用一路滚到底才找得到它
        Container(
          decoration: BoxDecoration(
            border: Border(top: BorderSide(color: theme.dividerColor)),
            color: theme.colorScheme.surface,
          ),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          child: Row(
            children: [
              if (_dirty)
                Text('有未保存的改动', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.primary))
              else
                Text('模型目录已与后端一致',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
              const Spacer(),
              FilledButton.icon(
                onPressed: _busy || _models.isEmpty ? null : _saveModels,
                icon: const Icon(Icons.save_outlined, size: 18),
                label: const Text('保存模型目录'),
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// 一行模型 = **固定高度**（[kModelRowHeight]），能力概览用图标表达。
  Widget _modelRow(ThemeData theme, int index) {
    final m = _models[index];
    return InkWell(
      onTap: _busy ? null : () => _editModel(index),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16),
        child: Row(
          children: [
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(m.displayName,
                      maxLines: 1, overflow: TextOverflow.ellipsis, style: theme.textTheme.titleSmall),
                  const SizedBox(height: 2),
                  Text(m.id,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
                ],
              ),
            ),
            const SizedBox(width: 8),
            _capabilityIcons(theme, m),
            IconButton(
              tooltip: '编辑能力',
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.tune, size: 18),
              onPressed: _busy ? null : () => _editModel(index),
            ),
            IconButton(
              tooltip: '移除',
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.close, size: 16),
              onPressed: _busy ? null : () => setState(() { _models.removeAt(index); _markDirty(); }),
            ),
          ],
        ),
      ),
    );
  }

  /// 能力一览：只画**声明过**的（未知即不支持，宁可不画也不猜）。
  Widget _capabilityIcons(ThemeData theme, AiModel m) {
    final items = <(IconData, String)>[
      for (final modality in AiModality.values)
        if (m.supports(modality)) (_modalityIcons[modality]!, modality.label),
      if (m.tools) (Icons.handyman_outlined, '工具'),
      if (m.reasoning) (Icons.psychology_outlined, '推理'),
    ];
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final (icon, label) in items)
          Tooltip(
            message: label,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2),
              child: Icon(icon, size: 15, color: theme.colorScheme.onSurfaceVariant),
            ),
          ),
      ],
    );
  }

  /// 打开某个模型的能力编辑弹窗；点了「确定」才写回列表（仍是本地改动，等「保存模型目录」落库）。
  Future<void> _editModel(int index) async {
    final edited = await showDialog<AiModel>(
      context: context,
      builder: (_) => _ModelEditorDialog(model: _models[index], busy: _busy),
    );
    if (edited == null || !mounted) return;
    setState(() {
      _models[index] = edited;
      _markDirty();
    });
  }

  // 模型卡的"能力 chips + 思考强度"编辑界面搬去了 [_ModelEditorDialog]，
  // 列表里只留固定高度的一行（[_modelRow]）—— 这正是滚动条不再乱跳、
  // 快速滑动不掉帧的原因。

  Future<void> _test() async {
    if (!widget.providers.supportsProbe) {
      // 兜底（按钮已经禁用了）：真被调到也要说人话，别去撞桌面那条不存在的 id
      _toast('手机上还没有"连接测试"：这一步要由本机自己向上游发请求，本批未实现。', error: true);
      return;
    }
    setState(() {
      _busy = true;
      _testResult = null;
    });
    try {
      final result = await widget.api.testProvider(widget.provider.id);
      if (mounted) setState(() => _testResult = result);
      _toast(result.ok ? '连接正常：${result.message}' : '连接失败：${result.display}', error: !result.ok);
    } catch (e) {
      _toast('连接测试失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _saveKey() async {
    final value = _key.text;
    if (value.trim().isEmpty) {
      _toast('密钥留空 = 不修改；如果要清除请点「移除密钥」。');
      return;
    }
    setState(() => _busy = true);
    try {
      final status = await widget.credentials.save(widget.provider, value);
      _key.clear(); // 立刻丢弃，不在内存里多留一秒
      final where = widget.credentials.isLocal ? '本机密钥库' : status.source;
      await widget.onChanged('密钥已保存（$where）。值不会再被读回。');
      _toast('密钥已保存（$where），值不会再被读回。');
    } catch (e) {
      _toast('保存密钥失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _removeKey() async {
    setState(() => _busy = true);
    try {
      await widget.credentials.clear(widget.provider);
      await widget.onChanged('密钥已移除。');
      _toast('密钥已移除。');
    } catch (e) {
      _toast('移除失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// **把本机这一把推给桌面**（`docs/pairing-design.md` §14.2 密钥上行）。
  ///
  /// 四步，一步都不能省：
  ///  ① **先问一次**：值会离开本机，这是用户该知情的事；
  ///  ② `reveal` 取明文（**全仓唯一允许读密钥的地方**）：本机没有就如实说"没有可推的"，
  ///     **绝不拿空串去覆盖桌面上那把好的**；
  ///  ③ `PUT /api/ai/credentials`，带上配对令牌；`rev` **不带** = 用户显式点的强制覆盖
  ///     （带 rev 才是"等值才应用"，见 §14.3）；
  ///  ④ 回执**逐条如实**：`applied` 说成功、`rejected` 说清是哪一种（开关没开 / 桌面更新过 /
  ///     名字不合法），旧后端（404/501）直说"还没有这条接口"。
  Future<void> _pushKeyToDesktop() async {
    final provider = widget.provider;
    final ref = provider.credentialRef?.trim() ?? '';
    if (ref.isEmpty) return;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('把这一把密钥推给桌面？'),
        content: Text(
          '「$ref」这把 key 会离开本机，写到桌面那一份凭据里 —— 桌面以后就用它发请求。\n\n'
          '桌面必须已经允许这台设备改密钥，否则会被拒（在桌面上的设备卡片里勾选）。',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('推上去')),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    setState(() => _busy = true);
    InboundAuthApi? owned;
    try {
      final value = await widget.credentials.reveal(provider);
      if (value == null || value.isEmpty) {
        _toast('本机没有这把密钥（没有可推的）—— 推空值会把桌面上那把好的覆盖掉，所以这里不发请求。',
            error: true);
        return;
      }
      final factory = widget.authApiFactory;
      final auth = factory != null ? factory(widget.api.baseUrl) : (owned = InboundAuthApi(widget.api.baseUrl));
      final CredentialUploadResult result;
      try {
        result = await auth.uploadCredentials({ref: value}, token: PairingTokenCache.read());
      } finally {
        // 自己建的客户端自己收；注入进来的（测试/MockClient）不许关
        owned?.dispose();
      }
      if (result.applied.contains(ref)) {
        await widget.onChanged('「$ref」已推到桌面。');
        _toast('已推给桌面：$ref');
      } else if (result.rejected.isNotEmpty) {
        final r = result.rejected.firstWhere((e) => e.ref == ref, orElse: () => result.rejected.first);
        _toast('桌面拒收「${r.ref}」：${r.hint}', error: true);
      } else {
        // 既没说成功也没说被拒 —— 如实说"桌面没回这一条"，别假装成功
        _toast('桌面既没回成功也没回被拒（回执里没有 $ref）—— 别当它成了。', error: true);
      }
    } on ApiException catch (e) {
      if (e.statusCode == 404 || e.statusCode == 501) {
        _toast('这台后端还没有"上行密钥"的接口（旧版本）—— 升级桌面后端再试。', error: true);
      } else {
        _toast('推给桌面失败：${e.message}', error: true);
      }
    } catch (e) {
      _toast('推给桌面失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 拉取候选模型。**发现只是候选**：勾选后先进本地列表，仍需点「保存模型目录」才落库；
  /// 能力（图片/视频/工具…）不会因为发现而被推断，一律保持"未声明"（AIH-009 / AIH-011）。
  Future<void> _discover() async {
    if (!widget.providers.supportsProbe) {
      // 兜底（按钮已经禁用了）：说清"这一端还做不到"，而不是去问桌面（它不认识本机这条 id）
      _toast('手机上还没有"获取模型"：这一步要由本机自己去拉上游的 /models，本批未实现。', error: true);
      return;
    }
    setState(() => _busy = true);
    try {
      final result = await widget.api.discoverModels(widget.provider.id);
      if (!mounted) return;
      if (!result.ok) {
        _toast('获取模型失败：${result.display}', error: true);
        return;
      }
      if (result.candidates.isEmpty) {
        _toast('端点没有返回模型列表（${result.message}）', error: true);
        return;
      }
      final picked = await showDialog<List<AiModelCandidate>>(
        context: context,
        builder: (_) => _DiscoverDialog(result: result),
      );
      if (picked == null || picked.isEmpty) return;
      var added = 0;
      setState(() {
        _markDirty();
        for (final c in picked) {
          if (_models.any((m) => m.id == c.id)) continue;
          _models.add(AiModel(
            providerId: widget.provider.id,
            id: c.id,
            displayName: c.displayName,
            // 用发现阶段预填/用户确认过的能力，而不是无脑"仅文本"
            inputModalities: c.modalities.map((m) => m.wire).toList(),
            attachmentTransports: {
              for (final m in c.modalities)
                if (m != AiModality.text) m.wire: const ['inline_base64'],
            },
            tools: c.tools,
            reasoning: c.reasoning,
            // 思考档位/方言也从内置目录预填（AIH-056）：用户能在模型卡片上改
            thinkingEfforts: c.thinkingEfforts,
            thinkingFormat: c.thinkingFormat,
            contextWindow: c.contextWindow,
            maxOutputTokens: c.maxOutputTokens,
            // 来源如实记录：接口声明 → discovered；内置目录 → builtin；未识别 → manual（用户接受默认）
            capabilitySource: switch (c.capabilitySource) {
              'discovered' => 'discovered',
              'builtin' => 'builtin',
              'tested' => 'tested',
              _ => 'manual',
            },
          ));
          added++;
        }
      });
      if (added == 0) {
        _toast('这些候选已经在目录里了。');
        return;
      }
      // 立刻落库：不要让用户以为"加了但没生效"
      await _saveModels();
      _toast('已加入并保存 $added 个模型');
    } catch (e) {
      _toast('获取模型失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _addModel() async {
    final draft = await showDialog<_ModelDraft>(
      context: context,
      builder: (_) => const _ModelDialog(),
    );
    if (draft == null) return;
    if (draft.id.isEmpty) {
      _toast('模型 ID 不能为空', error: true);
      return;
    }
    if (_models.any((m) => m.id == draft.id)) {
      _toast('模型 ${draft.id} 已经在目录里了');
      return;
    }
    setState(() {
      _markDirty();
      _models.add(AiModel(
        providerId: widget.provider.id,
        id: draft.id,
        displayName: draft.displayName,
        inputModalities: draft.modalities.map((m) => m.wire).toList(),
        attachmentTransports: {
          for (final m in draft.modalities)
            if (m != AiModality.text) m.wire: const ['inline_base64'],
        },
        tools: draft.tools,
        capabilitySource: 'manual',
      ));
    });
    // 点一次「添加」就应该真的加上：立刻落库
    await _saveModels();
    _toast('已添加并保存模型「${draft.displayName}」');
  }

  Future<void> _saveModels() async {
    setState(() => _busy = true);
    try {
      // 手机走本机库（一条网络请求都不打）；桌面走后端库。**同一份 toJson()**，两端不翻译。
      await widget.providers.saveModels(widget.provider.id, _models);
      _dirty = false; // 已经落库，之后父级重建可以直接覆盖本地副本
      await widget.onChanged('模型目录已保存（${_models.length} 个）。');
    } catch (e) {
      _toast('保存模型目录失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

class _Note extends StatelessWidget {
  final String text;
  final bool error;
  const _Note({required this.text, required this.error});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = error ? theme.colorScheme.error : theme.colorScheme.primary;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Text(text, style: theme.textTheme.bodySmall),
    );
  }
}

// ---------------------------------------------------------------------------
//  新建 Provider / 新建模型 对话框
// ---------------------------------------------------------------------------

class _ProviderDraft {
  final String id;
  final String displayName;
  final AiApi api;
  final String baseURL;
  final String? credentialRef;
  final AiEndpointTrust? trust;
  const _ProviderDraft(this.id, this.displayName, this.api, this.baseURL, this.credentialRef, this.trust);
}

class _ProviderDialog extends StatefulWidget {
  /// 传了就是**编辑**那一条（ID 锁住：桌面 `PUT` 的纪律是"ID 创建后不可修改"，
  /// 而桌面 `POST` 遇到已存在的 id 会报"Provider ID 已存在"）。
  final AiProvider? existing;

  const _ProviderDialog({this.existing});

  @override
  State<_ProviderDialog> createState() => _ProviderDialogState();
}

class _ProviderDialogState extends State<_ProviderDialog> {
  final _id = TextEditingController();
  final _name = TextEditingController();
  final _url = TextEditingController();
  final _ref = TextEditingController();
  AiApi _api = AiApi.openaiCompletions;

  bool get _editing => widget.existing != null;

  /// null = 自动（由后端按地址判断公网 / 本机）。
  ///
  /// 之前默认是"仅本机"，用户填了公网地址却忘了改这里，保存被后端拒绝，
  /// 而且错误看不见 —— 就是"配好了点添加没反应"。
  AiEndpointTrust? _trust;

  static final _kebab = RegExp(r'^[a-z0-9]+(-[a-z0-9]+)*$');

  @override
  void initState() {
    super.initState();
    // 编辑：把现有那一行填进来（ID 锁住，其余都能改 —— 用户要的"改个 baseURL"）
    final existing = widget.existing;
    if (existing != null) {
      _id.text = existing.id;
      _name.text = existing.displayName;
      _url.text = existing.baseURL;
      _ref.text = existing.credentialRef ?? defaultCredentialRef(existing.id);
      _api = AiApi.parse(existing.api) ?? AiApi.openaiCompletions;
      _trust = AiEndpointTrust.parse(existing.endpointTrust);
      _refTouched = true; // 已有值，别再跟着 ID 自动改
    }
    // 输入时就实时校验，按钮状态跟着变
    _id.addListener(_onIdChanged);
    _url.addListener(_revalidate);
    _name.addListener(_revalidate);
    // 凭据引用名默认跟着 ID 走（provider id 全大写），用户手动改过就不再覆盖
    _ref.addListener(_onRefChanged);
  }

  /// 用户手动改过凭据引用名 → 不再自动同步。
  bool _refTouched = false;

  /// 正在按 ID 自动填引用名：此时不把 `_ref` 的变化当成"用户手动改过"。
  bool _syncingRef = false;

  final _refFocus = FocusNode(debugLabel: 'credential-ref');

  /// 由 Provider ID 推出的默认凭据引用名：**全大写、`-` 换成 `_`、末尾加 `_API_KEY`**
  /// （后端要求环境变量风格：大写字母/数字/下划线，且以字母开头）。
  ///
  /// 例：`my-gateway` → `MY_GATEWAY_API_KEY`。
  /// 凭据引用名实际上不能空着（没有它就没法保存 API Key），但用户不该为它操心，
  /// 所以默认自动生成、跟着 ID 走，只有主动改过才停。
  static String defaultCredentialRef(String providerId) =>
      '${providerId.trim().toUpperCase().replaceAll('-', '_')}_API_KEY';

  /// 默认值合不合法（ID 以数字开头时会推不出合法名字）。
  static bool isValidCredentialRef(String ref) => RegExp(r'^[A-Z][A-Z0-9_]{0,127}$').hasMatch(ref);

  void _revalidate() => setState(() {});

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    _url.dispose();
    _ref.dispose();
    _refFocus.dispose();
    super.dispose();
  }

  void _onIdChanged() {
    // 用户没主动接管过、或者把框清空了 → 继续跟着 ID 自动填
    if (!_refTouched || _ref.text.trim().isEmpty) {
      final next = defaultCredentialRef(_id.text);
      if (_ref.text != next) {
        _syncingRef = true;
        _ref.text = next;
        _syncingRef = false;
      }
    }
    _revalidate();
  }

  /// 引用名变化：只有**用户自己**改的才算"手动接管"（程序自动填的不算）。
  void _onRefChanged() {
    if (!_syncingRef && !_refTouched) _refTouched = true;
    _revalidate();
  }

  String? get _idError {
    final v = _id.text.trim();
    if (v.isEmpty) return null;
    if (!_kebab.hasMatch(v)) return '只能小写字母/数字，用 - 连接（如 my-gateway）';
    if (v.length > 96) return '不能超过 96 个字符';
    return null;
  }

  String? get _urlError {
    final v = _url.text.trim();
    if (v.isEmpty) return null;
    final uri = Uri.tryParse(v);
    if (uri == null || (uri.scheme != 'http' && uri.scheme != 'https') || uri.host.isEmpty) {
      return '要写成完整的 http(s)://主机/路径';
    }
    return null;
  }

  /// 凭据引用名：**自动处理**。
  ///
  /// 它是 API Key 的存放名，所以不能空着；但用户不该为它操心 —— 默认值直接由
  /// Provider ID 推出来（全大写、`-` 换成 `_`），并且跟着 ID 实时变；
  /// 只有用户**主动改过**它之后才停止同步（见 [_refTouched]）。
  String? get _refError {
    final v = _ref.text.trim();
    if (v.isEmpty) return null;
    if (!isValidCredentialRef(v)) {
      return '只能是大写字母、数字和下划线，且以字母开头（Provider ID 以数字开头时要手改一个）';
    }
    if (v.length > 128) return '不能超过 128 个字符';
    return null;
  }

  /// 提交时把"用户没动过、值为空"的情况补上默认值，而不是拦着他（数据没错就行）。
  ///
  /// **用户手动清空也算"没填"**：那就用默认值，不去追究他为什么清空。
  String get _effectiveRef {
    final v = _ref.text.trim();
    if (v.isNotEmpty) return v;
    return defaultCredentialRef(_id.text);
  }

  bool get _canSubmit {
    final ref = _effectiveRef;
    if (ref.isEmpty || !isValidCredentialRef(ref)) return false;
    return _kebab.hasMatch(_id.text.trim()) &&
        _name.text.trim().isNotEmpty &&
        _urlError == null &&
        _url.text.trim().isNotEmpty;
  }

  /// 自动模式下的提示：让用户知道后端会怎么判断。
  String get _autoTrustHint {
    final host = Uri.tryParse(_url.text.trim())?.host ?? '';
    if (host.isEmpty) return '按地址自动判断';
    final isLocal = host == 'localhost' || host == '127.0.0.1' || host == '::1' || host.startsWith('192.168.') || host.startsWith('10.');
    return isLocal ? '自动：识别为本机/局域网地址' : '自动：识别为公网（必须 https）';
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(_editing ? '编辑 Provider' : '新建 Provider'),
      content: SizedBox(
        width: 480,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _id,
                // 编辑时 ID 只读：桌面两侧都是"创建后不可修改"（POST 撞 id 报错、PUT 拒绝改 id）
                readOnly: _editing,
                decoration: InputDecoration(
                  labelText: 'Provider ID（创建后不可改）',
                  hintText: 'my-gateway',
                  errorText: _idError,
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _name,
                decoration: const InputDecoration(labelText: '显示名', hintText: '公司网关', isDense: true),
              ),
              const SizedBox(height: 10),
              DropdownButtonFormField<AiApi>(
                initialValue: _api,
                decoration: const InputDecoration(labelText: 'API 协议', isDense: true),
                items: [
                  for (final a in AiApi.values)
                    DropdownMenuItem(value: a, child: Text(a.label)),
                ],
                onChanged: (v) => setState(() => _api = v ?? _api),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _url,
                decoration: InputDecoration(
                  labelText: 'Base URL',
                  hintText: 'https://api.deepseek.com/v1',
                  helperText: '只去掉末尾的 /；兼容网关的路径原样保留',
                  errorText: _urlError,
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              DropdownButtonFormField<AiEndpointTrust?>(
                initialValue: _trust,
                decoration: InputDecoration(
                  labelText: '端点信任级别',
                  helperText: _autoTrustHint,
                  isDense: true,
                ),
                items: [
                  const DropdownMenuItem<AiEndpointTrust?>(value: null, child: Text('自动（按地址判断）')),
                  for (final t in AiEndpointTrust.values)
                    DropdownMenuItem<AiEndpointTrust?>(value: t, child: Text(t.label)),
                ],
                onChanged: (v) => setState(() => _trust = v),
              ),
              const SizedBox(height: 10),
              // 凭据引用名：正常不用用户操心 —— 默认跟着 Provider ID 走
              // （全大写、`-` 换成 `_`），只有主动改过才停。提交时若还空着就自动补上默认值。
              TextField(
                controller: _ref,
                focusNode: _refFocus,
                textCapitalization: TextCapitalization.characters,
                decoration: InputDecoration(
                  labelText: '凭据引用名（可选）',
                  hintText: 'DEEPSEEK_API_KEY',
                  helperText: _refTouched
                      ? '只是名字；密钥在创建之后单独填写（只写不读）'
                      : '默认跟随 Provider ID：全大写、- 换成 _、末尾加 _API_KEY',
                  errorText: _refError,
                  isDense: true,
                ),
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(
          onPressed: !_canSubmit
              ? null
              : () => Navigator.pop(
                    context,
                    _ProviderDraft(
                      _id.text.trim(),
                      _name.text.trim(),
                      _api,
                      _url.text.trim(),
                      // 空着也没关系：这里补上由 Provider ID 推出来的默认名
                      _effectiveRef,
                      _trust,
                    ),
                  ),
          child: Text(_editing ? '保存' : '创建'),
        ),
      ],
    );
  }
}

class _ModelDraft {
  final String id;
  final String displayName;
  final List<AiModality> modalities;
  final bool tools;
  const _ModelDraft(this.id, this.displayName, this.modalities, this.tools);
}

/// 候选模型：**自动预填**能力（逐维度：接口声明 > 内置目录 > 兜底；
/// 工具默认给上），并把来源标出来；加入前可以逐个调整。
class _DiscoverDialog extends StatefulWidget {
  final AiDiscoverResult result;
  const _DiscoverDialog({required this.result});

  @override
  State<_DiscoverDialog> createState() => _DiscoverDialogState();
}

class _DiscoverDialogState extends State<_DiscoverDialog> {
  /// id -> 用户确认后的能力（默认取预填值）
  late final Map<String, AiModelCandidate> _drafts = {
    for (final c in widget.result.candidates) c.id: c,
  };
  late final Set<String> _selected = widget.result.candidates.map((c) => c.id).toSet();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final declared = widget.result.candidates.where((c) => c.capabilitySource == 'discovered').length;
    final builtin = widget.result.candidates.where((c) => c.capabilitySource == 'builtin').length;

    return AlertDialog(
      title: Text('发现 ${widget.result.candidates.length} 个候选模型'),
      content: SizedBox(
        width: 620,
        height: 460,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '能力已按「接口声明（$declared 个）→ 内置目录（$builtin 个）→ 仅文本」自动预填，'
              '徽标可以逐个点开修改。确认后写入模型目录，随时可以在列表里再改。',
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 8),
            Expanded(
              child: ListView.builder(
                itemCount: widget.result.candidates.length,
                itemBuilder: (context, i) {
                  final id = widget.result.candidates[i].id;
                  final c = _drafts[id]!;
                  final checked = _selected.contains(id);
                  return Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        CheckboxListTile(
                          dense: true,
                          contentPadding: EdgeInsets.zero,
                          value: checked,
                          title: Row(
                            children: [
                              Flexible(child: Text(c.displayName, overflow: TextOverflow.ellipsis)),
                              const SizedBox(width: 8),
                              _SourceChip(source: c.capabilitySource, label: c.sourceLabel),
                            ],
                          ),
                          subtitle: Text(c.detail, style: theme.textTheme.labelSmall),
                          onChanged: (on) => setState(() {
                            if (on == true) {
                              _selected.add(id);
                            } else {
                              _selected.remove(id);
                            }
                          }),
                        ),
                        if (checked)
                          Padding(
                            padding: const EdgeInsets.only(left: 8, bottom: 4),
                            child: Wrap(
                              spacing: 6,
                              children: [
                                for (final m in AiModality.values)
                                  FilterChip(
                                    label: Text(m.label, style: theme.textTheme.labelSmall),
                                    selected: c.modalities.contains(m),
                                    visualDensity: VisualDensity.compact,
                                    onSelected: (on) => setState(() {
                                      final next = [...c.modalities];
                                      if (on) {
                                        next.add(m);
                                      } else {
                                        next.remove(m);
                                      }
                                      _drafts[id] = c.copyWith(modalities: next);
                                    }),
                                  ),
                                FilterChip(
                                  label: Text('工具', style: theme.textTheme.labelSmall),
                                  selected: c.tools,
                                  visualDensity: VisualDensity.compact,
                                  onSelected: (on) =>
                                      setState(() => _drafts[id] = c.copyWith(tools: on)),
                                ),
                              ],
                            ),
                          ),
                        if (checked && c.capabilityNote != null)
                          Padding(
                            padding: const EdgeInsets.only(left: 8, bottom: 4),
                            child: Text(
                              c.capabilityNote!,
                              style: theme.textTheme.labelSmall
                                  ?.copyWith(color: theme.colorScheme.outline),
                            ),
                          ),
                      ],
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(
          onPressed: () => Navigator.pop(
            context,
            _selected.map((id) => _drafts[id]!).toList(),
          ),
          child: Text('加入 ${_selected.length} 个'),
        ),
      ],
    );
  }
}

/// 能力来源徽标：接口声明（可信）/ 内置目录（离线表，可能过期）/ 未识别。
class _SourceChip extends StatelessWidget {
  final String source;
  final String label;
  const _SourceChip({required this.source, required this.label});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (bg, fg) = switch (source) {
      'discovered' => (theme.colorScheme.primaryContainer, theme.colorScheme.onPrimaryContainer),
      'builtin' => (theme.colorScheme.tertiaryContainer, theme.colorScheme.onTertiaryContainer),
      'tested' => (theme.colorScheme.secondaryContainer, theme.colorScheme.onSecondaryContainer),
      _ => (theme.colorScheme.surfaceContainerHighest, theme.colorScheme.outline),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(6)),
      child: Text(label, style: theme.textTheme.labelSmall?.copyWith(color: fg)),
    );
  }
}

class _ModelDialog extends StatefulWidget {
  const _ModelDialog();

  @override
  State<_ModelDialog> createState() => _ModelDialogState();
}

class _ModelDialogState extends State<_ModelDialog> {
  final _id = TextEditingController();
  final _name = TextEditingController();
  final _modalities = <AiModality>{AiModality.text};

  /// 工具默认给上：网关多半不声明工具能力但实际支持，而当前请求体还不发 `tools`，
  /// 勾着不会让请求失败（用户随时能取消）。
  bool _tools = true;

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('添加模型'),
      content: SizedBox(
        width: 460,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _id,
              decoration: const InputDecoration(
                labelText: '模型 ID（请求里用的那个）',
                isDense: true,
              ),
            ),
            const SizedBox(height: 10),
            TextField(
              controller: _name,
              decoration: const InputDecoration(labelText: '显示名', isDense: true),
            ),
            const SizedBox(height: 12),
            const Text('输入能力（不勾 = 不支持；不猜）'),
            const SizedBox(height: 6),
            Wrap(
              spacing: 6,
              children: [
                for (final m in AiModality.values)
                  FilterChip(
                    label: Text(m.label),
                    selected: _modalities.contains(m),
                    onSelected: (on) => setState(() {
                      if (on) {
                        _modalities.add(m);
                      } else {
                        _modalities.remove(m);
                      }
                    }),
                  ),
                FilterChip(
                  label: const Text('工具'),
                  selected: _tools,
                  onSelected: (on) => setState(() => _tools = on),
                ),
              ],
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(
          onPressed: () => Navigator.pop(
            context,
            _ModelDraft(
              _id.text.trim(),
              _name.text.trim().isEmpty ? _id.text.trim() : _name.text.trim(),
              _modalities.toList(),
              _tools,
            ),
          ),
          child: const Text('添加'),
        ),
      ],
    );
  }
}

extension _FirstOrNullExt<T> on Iterable<T> {
  T? get firstOrNull => isEmpty ? null : first;
}

/// 「内置模型目录」卡片（M6）。
///
/// 内置目录是 `%USERPROFILE%\.dsh\settings.yaml` 的**冻结副本**
/// （`server/src/main/resources/ai/builtin-catalog.json`）—— 运行时**绝不读 YAML**，
/// 装我们项目的人没装 DSH 也能用；这里只是把冻结副本与库里的差异摆出来，让用户决定要不要对齐。
class _BuiltinCatalogCard extends StatelessWidget {
  final AiBuiltinCatalogStatus? status;
  final bool busy;
  final VoidCallback? onRefresh;
  final Future<void> Function() onAlignCapabilities;
  final Future<void> Function() onAddMissing;

  const _BuiltinCatalogCard({
    required this.status,
    required this.busy,
    required this.onRefresh,
    required this.onAlignCapabilities,
    required this.onAddMissing,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = status;
    final divergent = s?.divergentCount ?? 0;

    return Card(
      margin: const EdgeInsets.fromLTRB(12, 12, 12, 4),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.inventory_2_outlined, size: 18, color: theme.colorScheme.primary),
                const SizedBox(width: 6),
                Text('内置模型目录', style: theme.textTheme.titleSmall),
                if (busy) ...[
                  const SizedBox(width: 10),
                  const SizedBox(
                    width: 12,
                    height: 12,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                ],
                const Spacer(),
                IconButton(
                  tooltip: '刷新内置目录状态',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                  padding: EdgeInsets.zero,
                  onPressed: busy ? null : onRefresh,
                  icon: const Icon(Icons.refresh, size: 16),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              r'来自 %USERPROFILE%\.dsh\settings.yaml 的冻结副本（内置目录），运行时不会去读 YAML；'
              '已自动登记进数据库，这里只做「按需对齐」。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 8),
            if (s == null)
              Text('正在读取内置目录状态…', style: theme.textTheme.bodySmall)
            else if (!s.ok)
              Text('读取失败：${s.error}',
                  style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.error))
            else ...[
              Wrap(
                spacing: 6,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  _Tag('版本 ${s.version}'),
                  _Tag('目录模型 ${s.modelCount}'),
                  _Tag('库里一致 ${s.kept}'),
                  if (divergent > 0)
                    Container(
                      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                      decoration: BoxDecoration(
                        color: theme.colorScheme.errorContainer,
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Icon(Icons.warning_amber_rounded,
                              size: 13, color: theme.colorScheme.onErrorContainer),
                          const SizedBox(width: 4),
                          Text(
                            s.divergenceLabel,
                            style: theme.textTheme.labelSmall
                                ?.copyWith(color: theme.colorScheme.onErrorContainer),
                          ),
                        ],
                      ),
                    )
                  else
                    const _Tag('能力声明一致'),
                  if (s.providerCreated) const _Tag('Provider 按内置目录创建'),
                ],
              ),
              if (divergent > 0)
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Text(
                    '分歧的模型：${s.divergent.take(6).join('、')}'
                    '${divergent > 6 ? ' 等 $divergent 个' : ''}',
                    style: theme.textTheme.labelSmall
                        ?.copyWith(color: theme.colorScheme.outline),
                  ),
                ),
            ],
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 4,
              children: [
                FilledButton.icon(
                  onPressed: busy ? null : () => onAlignCapabilities(),
                  icon: const Icon(Icons.tune, size: 16),
                  label: const Text('用内置目录对齐模型能力'),
                ),
                OutlinedButton.icon(
                  onPressed: busy ? null : () => onAddMissing(),
                  icon: const Icon(Icons.playlist_add, size: 16),
                  // 纯新增：不改任何已有行，所以不弹确认框
                  label: const Text('补齐缺失模型'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// 「模型能力目录」卡片（2026-09-23"全做了"）。
///
/// 展示三层目录（手工 custom > 在线 remote > 内置）的新鲜度；「立即更新」拉
/// models.dev；实测校准默认关，打开后才能发真实请求校准思考档位。
/// 所有状态都从后端读（`/api/ai/capabilities/*`），前端不存一份自己的。
class _CapabilityCatalogCard extends StatelessWidget {
  final AiCapabilityStatus? status;
  final bool busy;
  final bool probeBusy;
  final VoidCallback? onRefreshStatus;
  final VoidCallback? onUpdateRemote;
  final ValueChanged<bool>? onProbeToggle;
  final VoidCallback? onProbe;

  const _CapabilityCatalogCard({
    required this.status,
    required this.busy,
    required this.probeBusy,
    required this.onRefreshStatus,
    required this.onUpdateRemote,
    required this.onProbeToggle,
    required this.onProbe,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = status;
    final remoteStale = s?.remoteStale ?? false;
    // 闭包里访问可空函数字段无法被 Dart 的流分析提升，先取局部变量
    final update = onUpdateRemote;
    final probe = onProbe;

    return Card(
      margin: const EdgeInsets.fromLTRB(12, 8, 12, 4),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.auto_awesome_motion_outlined, size: 18, color: theme.colorScheme.primary),
                const SizedBox(width: 6),
                Text('模型能力目录', style: theme.textTheme.titleSmall),
                if (busy || probeBusy) ...[
                  const SizedBox(width: 10),
                  const SizedBox(
                    width: 12,
                    height: 12,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                ],
                const Spacer(),
                IconButton(
                  tooltip: '刷新目录状态',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                  padding: EdgeInsets.zero,
                  onPressed: busy ? null : onRefreshStatus,
                  icon: const Icon(Icons.refresh, size: 16),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              '预填来源：手工目录 > 在线目录（models.dev 自动更新）> 内置规则；接口自己声明的永远优先。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 6),
            if (s == null)
              Text('正在读取能力目录状态…', style: theme.textTheme.bodySmall)
            else if (s.error != null)
              Text('读取失败：${s.error}',
                  style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.error))
            else ...[
              Wrap(
                spacing: 6,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  _Tag('内置 v${s.version}（${s.builtinRules} 条规则）'),
                  if (s.customEntries != null) _Tag('手工 ${s.customEntries} 条'),
                  Container(
                    padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                    decoration: BoxDecoration(
                      color: remoteStale
                          ? theme.colorScheme.errorContainer
                          : theme.colorScheme.surfaceContainerHighest,
                      borderRadius: BorderRadius.circular(6),
                    ),
                    child: Text(
                      s.remoteLabel + (s.remoteStale ? ' · 已过期' : ''),
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: remoteStale
                            ? theme.colorScheme.onErrorContainer
                            : theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                  if (s.lastMessage != null) _Tag(s.lastMessage!),
                ],
              ),
            ],
            const SizedBox(height: 6),
            // 开关 + 说明一行，两个动作一行（窄屏换行）。
            //
            // ⚠ 这里踩了两次（2026-09-24 真机验收）：
            //   ① 原来是一个 `Row` 塞四样东西 → `RIGHT OVERFLOWED BY 2 PIXELS`；
            //   ② 改成 `Wrap([Row(Switch+Text), 按钮…])` 之后**还是**溢出 10px ——
            //      因为里层那个 `Row` 自己是 `mainAxisSize.min` + 不换行的 `Text`，
            //      它一宽就顶出去了。所以文字必须是 `Expanded` + 省略号，
            //      动作按钮才是那个"窄了就换行"的 `Wrap`。
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Switch(
                      value: s?.probeEnabled ?? false,
                      onChanged: busy ? null : (v) => onProbeToggle?.call(v),
                      materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    const SizedBox(width: 2),
                    Expanded(
                      child: Text(
                        '允许实测校准（发真实请求，可能产生少量费用）',
                        style: theme.textTheme.bodySmall,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                Wrap(
                  spacing: 8,
                  runSpacing: 6,
                  children: [
                    FilledButton.icon(
                      onPressed: (busy || update == null) ? null : () => update(),
                      icon: const Icon(Icons.cloud_download_outlined, size: 16),
                      label: const Text('立即更新'),
                    ),
                    OutlinedButton.icon(
                      onPressed: (busy || probeBusy || probe == null || s?.probeEnabled != true)
                          ? null
                          : () => probe(),
                      icon: const Icon(Icons.fact_check_outlined, size: 16),
                      label: const Text('校准思考档位'),
                    ),
                  ],
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _Tag extends StatelessWidget {
  final String text;
  const _Tag(this.text);
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: theme.colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text, style: theme.textTheme.labelSmall),
    );
  }
}

/// 一行模型的高度（常数）。
///
/// **别改成按内容自适应**：`SliverFixedExtentList` 就是靠这个常数精确算出
/// `maxScrollExtent` 的，一旦不固定，滚动条滑块又会随滚动变短（用户报的 bug ④）。
const double kModelRowHeight = 64;

/// 模态 → 图标（模型行右侧的能力一览）。
const Map<AiModality, IconData> _modalityIcons = {
  AiModality.text: Icons.notes_outlined,
  AiModality.image: Icons.image_outlined,
  AiModality.video: Icons.movie_outlined,
  AiModality.audio: Icons.graphic_eq,
  AiModality.document: Icons.description_outlined,
};

/// 模型能力编辑的公共规则：**列表页与编辑弹窗共用这一份**，
/// 免得"点一下某个徽标把别的声明抹掉"这类问题在两边各写一遍。
class _ModelEdit {
  const _ModelEdit._();

  /// 唯一的模型复制入口：**新增字段必须在这里带上**。
  static AiModel copy(
    AiModel m, {
    bool? tools,
    bool? reasoning,
    List<String>? inputModalities,
    Map<String, List<String>>? attachmentTransports,
    Map<String, String>? thinkingEfforts,
    String? thinkingFormat,
    bool clearThinkingFormat = false,
    bool manual = false,
  }) =>
      AiModel(
        providerId: m.providerId,
        id: m.id,
        displayName: m.displayName,
        inputModalities: inputModalities ?? m.inputModalities,
        attachmentTransports: attachmentTransports ?? m.attachmentTransports,
        mimeAllowlist: m.mimeAllowlist,
        tools: tools ?? m.tools,
        parallelTools: m.parallelTools,
        reasoning: reasoning ?? m.reasoning,
        thinkingEfforts: thinkingEfforts ?? m.thinkingEfforts,
        thinkingFormat: clearThinkingFormat ? null : (thinkingFormat ?? m.thinkingFormat),
        contextWindow: m.contextWindow,
        maxOutputTokens: m.maxOutputTokens,
        maxAttachmentCount: m.maxAttachmentCount,
        // 用户手工动过能力就标成 manual（AIH-011：声明从哪来要看得见）
        capabilitySource: manual ? 'manual' : m.capabilitySource,
        enabled: m.enabled,
      );

  static AiModel toggleModality(AiModel m, AiModality modality, bool on) {
    final next = [...m.inputModalities];
    if (on) {
      if (!next.contains(modality.wire)) next.add(modality.wire);
    } else {
      next.remove(modality.wire);
    }
    // 传输方式跟着模态一起改：这里只登记"模型声明支持"，适配器是否实现了由后端另判（AIH-028）
    final transports = Map<String, List<String>>.from(m.attachmentTransports);
    if (on && modality != AiModality.text && !transports.containsKey(modality.wire)) {
      transports[modality.wire] = const ['inline_base64'];
    }
    if (!on) transports.remove(modality.wire);
    return copy(m, inputModalities: next, attachmentTransports: transports, manual: true);
  }

  /// 切换"支持推理"：关掉时必须一并清掉思考档位声明（后端也会校验两者一致）。
  static AiModel toggleReasoning(AiModel m, bool on) =>
      copy(m, reasoning: on, thinkingEfforts: on ? m.thinkingEfforts : const {}, manual: true);

  /// 各档位的默认"过线拼写"（与后端 ThinkingLevels.DEFAULT 对齐）。
  static String defaultEffortWire(AiReasoningEffort e) => switch (e) {
        AiReasoningEffort.off => 'none',
        AiReasoningEffort.minimal => 'minimal',
        AiReasoningEffort.low => 'low',
        AiReasoningEffort.medium => 'medium',
        AiReasoningEffort.high => 'high',
        AiReasoningEffort.xhigh => 'xhigh',
        AiReasoningEffort.max => 'high',
      };
}

/// 模型能力编辑弹窗：模态 / 工具 / 思考强度与方言。
///
/// 点「确定」把改好的模型返回给调用方（**不落库** —— 落库仍然要点列表底部的
/// 「保存模型目录」，这条规矩没变）。
class _ModelEditorDialog extends StatefulWidget {
  const _ModelEditorDialog({required this.model, required this.busy});

  final AiModel model;
  final bool busy;

  @override
  State<_ModelEditorDialog> createState() => _ModelEditorDialogState();
}

class _ModelEditorDialogState extends State<_ModelEditorDialog> {
  late AiModel _m = widget.model;

  void _update(AiModel next) => setState(() => _m = next);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final busy = widget.busy;
    return AlertDialog(
      title: Text('模型能力：${_m.displayName}'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              SelectableText(_m.id,
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
              const SizedBox(height: 10),
              Text('输入模态与工具', style: theme.textTheme.titleSmall),
              const SizedBox(height: 6),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final modality in AiModality.values)
                    FilterChip(
                      label: Text(modality.label, style: theme.textTheme.labelSmall),
                      selected: _m.supports(modality),
                      onSelected: busy
                          ? null
                          : (on) => _update(_ModelEdit.toggleModality(_m, modality, on)),
                    ),
                  FilterChip(
                    label: Text('工具', style: theme.textTheme.labelSmall),
                    selected: _m.tools,
                    onSelected: busy ? null : (on) => _update(_ModelEdit.copy(_m, tools: on, manual: true)),
                  ),
                ],
              ),
              const Divider(height: 24),
              Row(
                children: [
                  Expanded(child: Text('思考强度', style: theme.textTheme.titleSmall)),
                  Switch(
                    value: _m.reasoning,
                    onChanged: busy ? null : (on) => _update(_ModelEdit.toggleReasoning(_m, on)),
                  ),
                ],
              ),
              Text(
                _m.reasoning
                    ? '勾选该模型真正支持的档位（「默认」不在这个列表里 —— 它不发任何思考参数、'
                        '任何模型都成立，聊天界面永远可选）。留空 = 不声明；真正的思考档位宁可不给选，'
                        '也不要发出去被上游 400。'
                    : '该模型不支持推理；勾选后可以逐档声明。',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
              if (_m.reasoning) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    // 「默认」不列在这里：它是"什么都不声明"的那一档，不是模型能力。
                    for (final e in AiReasoningEffort.values.where((e) => e.isThinking))
                      FilterChip(
                        label: Text(e.label, style: theme.textTheme.labelSmall),
                        selected: _m.thinkingEfforts.containsKey(e.wire),
                        onSelected: busy
                            ? null
                            : (on) {
                                final next = Map<String, String>.from(_m.thinkingEfforts);
                                if (on) {
                                  next[e.wire] = _ModelEdit.defaultEffortWire(e);
                                } else {
                                  next.remove(e.wire);
                                }
                                _update(_ModelEdit.copy(_m, thinkingEfforts: next, manual: true));
                              },
                      ),
                  ],
                ),
                const SizedBox(height: 8),
                DropdownButtonFormField<String>(
                  initialValue: _m.thinkingFormatEnum?.wire,
                  // 弹窗比原来的卡片窄得多：不设 isExpanded 时选中项会按原样撑开
                  // （实测窄到 492px 时 "spaceBetween" 那一行溢出 154px）
                  isExpanded: true,
                  isDense: true,
                  decoration: const InputDecoration(
                    labelText: '网关思考方言（同一个"高"落到哪个字段）',
                    helperText:
                        'OpenAI 风格用 reasoning_effort；DeepSeek / Qwen / Z.AI / OpenRouter 各有自己的字段组合',
                    isDense: true,
                  ),
                  items: [
                    const DropdownMenuItem(value: null, child: Text('按协议默认（OpenAI 风格）')),
                    for (final f in AiThinkingFormat.values)
                      DropdownMenuItem(value: f.wire, child: Text(f.label)),
                  ],
                  onChanged: busy
                      ? null
                      : (v) => _update(v == null
                          ? _ModelEdit.copy(_m, clearThinkingFormat: true, manual: true)
                          : _ModelEdit.copy(_m, thinkingFormat: v, manual: true)),
                ),
                if (_m.thinkingEfforts.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 6),
                    child: Text(
                      '实际发送：${_m.thinkingEfforts.entries.map((e) => '${e.key}→${e.value}').join('  ')}',
                      style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                    ),
                  ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(onPressed: () => Navigator.pop(context, _m), child: const Text('确定')),
      ],
    );
  }
}
