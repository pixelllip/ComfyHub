import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/credential_store.dart';
import '../core/inbound_auth_api.dart';
import '../core/provider_store.dart';
import '../core/provider_sync.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/settings_scaffold.dart';
import 'ai_provider_detail.dart';
import 'ai_provider_dialogs.dart';
import 'ai_provider_list_panel.dart';

// 这一页按「Provider 列表 / 模型目录 / 凭据」拆成了三块（2026-11-07）：
//   · 列表   —— `ai_provider_list_panel.dart`
//   · 模型目录 —— `ai_provider_models_section.dart`
//   · 凭据   —— `ai_provider_credentials_section.dart`
// 本文件只剩"页面骨架 + 清单 CRUD + 从电脑同步"这些真正属于页面的东西。
// 对话框与能力目录卡各自独立成文件；下面这两条 export 是给用例的稳定入口
// （`defaultCredentialRef` / `kModelRowHeight` 以前就住在这个库里）。
export 'ai_provider_dialogs.dart'
    show defaultCredentialRef, isValidCredentialRef;
export 'ai_provider_models_section.dart' show kModelRowHeight;

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

/// 窄屏（手机）上这一页的两块内容 —— **一次只显示一块、各自整屏**。
///
/// 理由见 `_buildNarrow` 的注释：手机上不做上下分屏，点进去就是子界面。
enum _NarrowPane { list, detail }

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

  /// 模型能力目录的在线状态（models.dev，2026-09-23）。
  ///
  /// ⚠ **全局一份，但只显示在选中的 Provider 详情里**。
  /// 此前它是页顶两张常驻卡片之一，与当前 Provider 无关，却占用半屏。
  AiCapabilityStatus? _cap;
  bool _capBusy = false;
  bool _probeBusy = false;

  /// **窄屏（手机）当前显示哪一块**。桌面用不上它（那边是左右分栏）。
  ///
  /// 见 [`_buildNarrow`]：手机上"列表 / 详情"各自整屏，一次只显示一块 ——
  /// 用 `enum` 而不是 `Navigator.push`，是为了让两块共用同一份 `State`
  /// （推成独立路由的话，父级 `setState` 不会重建它，卡片状态会停在推入那一刻）。
  _NarrowPane _pane = _NarrowPane.list;

  /// 「从电脑同步 Provider」正在跑（按钮变转圈 + 禁用；再点一次不会叠一趟）。
  bool _syncingProviders = false;

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
      _loadCapabilityCatalog();
    });
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
      if (mounted) _notify('更新能力目录失败，$e', error: true);
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
      if (mounted) _notify('更新校准开关失败，$e', error: true);
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
          '会对每个 Provider 最多 8 个支持推理的模型，从最高档往下发极小的真实请求，'
          '返回网关接受的实际档位。\n\n'
          '可能产生少量费用。手动声明过的模型不会被覆盖。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('开始校准'),
          ),
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
      if (mounted) _notify('校准失败，$e', error: true);
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
                Text(
                  response.message,
                  style: Theme.of(ctx).textTheme.bodySmall,
                ),
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
                              : (o.ok
                                    ? Icons.check_circle_outline
                                    : Icons.error_outline),
                          size: 15,
                          color: o.skipped
                              ? Theme.of(ctx).colorScheme.outline
                              : (o.ok
                                    ? Colors.green.shade600
                                    : Theme.of(ctx).colorScheme.error),
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
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('好的'),
          ),
        ],
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
            // best-effort：拿不到本机状态就沿用清单里的值（那条会显示"未配置"，
            // 至少不会把"没有密钥"说成"有"）。不在这里当成"读取成功"。
            out.add(p);
          }
        }
        _providers = out;
      }
      if (_selected != null) {
        _selected = _providers.where((p) => p.id == _selected!.id).firstOrNull;
      }
      _models = _selected == null
          ? const []
          : await _store.listModels(_selected!.id);
    } catch (e) {
      _error = '$e';
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _select(AiProvider? p) async {
    // ⚠ **使用清单中的数据，不使用调用方持有的对象**：
    // 新建时填写密钥后，provider 会显示"未填写密钥"，而实际已填入。
    //
    // 新建 / 编辑的返回值里，`credential` 是后端**回读那一刻**算出来的 ——
    // 那时密钥还没存（`insert_provider` / `update_provider` 都在 `PUT credentials` 之前
    // 就回读了）。页面拿它当"当前选中项"，于是刚填进去的 key 在详情里显示成"未配置"，
    // 而 [credential_store] 里明明有那把。`_reload()` 是在存完密钥之后跑的，
    // 所以按 id 取回来的那一份才是最新的。
    final fresh = p == null
        ? null
        : _providers.where((e) => e.id == p.id).firstOrNull ?? p;
    setState(() {
      _selected = fresh;
      _models = const [];
      _info = null;
      _error = null;
    });
    if (fresh != null) {
      try {
        _models = await _store.listModels(fresh.id);
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
    // 手机上**不做上下分屏**（上下分屏在手机上体验不佳，部分内容应作为子界面）
    if (!wide) return _buildNarrow(theme);

    final list = AiProviderListPanel(
      providers: _providers,
      selectedId: _selected?.id,
      loading: _loading,
      // 手机上要如实说一句"这份清单只在本机"（§14.4：两端各一份、不自动合并）
      local: _store.isLocal,
      onSelect: _select,
      onCreate: _createProvider,
      onDelete: _deleteProvider,
      // 只在手机上出现：桌面上后端库自己就是真源
      onSync: _store.isLocal ? _syncProvidersFromDesktop : null,
      syncing: _syncingProviders,
    );

    final detail = _selected == null
        ? Center(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Text(
                '选择一个 Provider 查看详情，或新建一个。\n填入的 API Key 保存后不会再显示。',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
          )
        : AiProviderDetail(
            key: ValueKey(_selected!.id + _selected!.revision.toString()),
            api: _api,
            credentials: _credentials,
            providers: _store,
            authApiFactory: widget.authApiFactory,
            provider: _selected!,
            models: _models,
            info: _info,
            error: _error,
            // 「模型能力目录」**嵌在该 Provider 的详情里**：
            // 它以前是与 Provider 无关的两张常驻卡，页顶就吃掉半屏。
            capability: _cap,
            capabilityBusy: _capBusy,
            probeBusy: _probeBusy,
            onRefreshCapability: _loading ? null : _loadCapabilityCatalog,
            onUpdateCapabilityRemote: _refreshCapabilityCatalog,
            onProbeToggle: _toggleProbe,
            onProbe: _probeThinkingLadders,
            onEdit: () => _editProvider(_selected!),
            onChanged: (msg) async {
              setState(() => _info = msg);
              await _reload();
            },
          );

    // ⚠ 「AI 工具权限」的入口**不在这里**：它与模型 / 凭据不构成附属关系，
    // 入口位于设置首页，与「AI 模型与凭据」并列。
    //
    // 顶栏 / 返回键 / 大标题联动**和其它设置子页完全一样**（[SettingsPageScaffold]
    // 与 [SettingsScaffold] 共用 [SettingsAppBar]）：以前这一页没有 AppBar，
    // 正文页头直接顶在 body 上，手机上会被状态栏压住。
    return SettingsPageScaffold(
      title: 'AI 模型与凭据',
      child: Column(
        children: [
          // 没有选中 Provider 时，错误也要看得见（新建失败最常发生在这种情况下）
          if (_error != null) _errorBanner(theme),
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
        Icon(
          Icons.error_outline,
          size: 18,
          color: theme.colorScheme.onErrorContainer,
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Text(
            _error!,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.onErrorContainer,
            ),
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

  /// **手机上的这一页**：两块内容各自**整屏**，带返回箭头，一次只显示一块。
  ///
  /// 上下分屏在手机上体验不佳，部分内容应作为子界面。
  /// 所以：
  ///
  /// | 面板 | 里面是什么 | 怎么进出 |
  /// | --- | --- | --- |
  /// | `list` | Provider 一览（主界面）+ 新建 | 点某一条 → `detail` |
  /// | `detail` | 密钥 / 模型目录 / 能力目录 / 连接测试 | 左上角返回 |
  ///
  /// ⚠ **刻意不 `Navigator.push` 开新路由**：两块共用同一份 `State`
  /// （`_cap` / `_models` / `_error` 全在这里）。推成独立路由之后，
  /// 父级 `setState` **不会**重建那个路由 —— 卡片上的状态会停在推入那一刻，
  /// 表现是"东西明明加载完了却还显示空的、改完不回显"。一个 `enum` 就没有这个缝。
  Widget _buildNarrow(ThemeData theme) {
    // 详情面板的前提是"有一个选中的 Provider"：删掉之后自动退回列表
    final pane = (_selected == null && _pane == _NarrowPane.detail)
        ? _NarrowPane.list
        : _pane;
    // 子面板上**接管系统返回**（否则返回手势会使整页退回设置首页）。
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
      case _NarrowPane.detail:
        final provider = _selected!;
        return withBackGuard(
          // key 跟着 Provider 走：换一条 = 换一段正文，标题进度从 0 重新开始
          // （不换 key 的话，上一条滚到一半的进度会让 AppBar 上立刻挂着新标题）。
          SettingsPageScaffold(
            key: ValueKey('ai-provider-detail-${provider.id}'),
            title: provider.displayName,
            onBack: () => setState(() => _pane = _NarrowPane.list),
            // 大标题就是这条 Provider 的名字，所以详情里不再重复一行同名标题
            // （AGENTS §6.3：同一个标题不许出现两次）；「编辑」因此挪到 AppBar。
            actions: [
              IconButton(
                tooltip: '编辑 Provider',
                icon: const Icon(Icons.edit_outlined),
                onPressed: () => _editProvider(provider),
              ),
            ],
            child: Column(
              children: [
                if (_error != null) _errorBanner(theme),
                Expanded(
                  child: AiProviderDetail(
                    key: ValueKey(provider.id + provider.revision.toString()),
                    api: _api,
                    credentials: _credentials,
                    providers: _store,
                    authApiFactory: widget.authApiFactory,
                    provider: provider,
                    models: _models,
                    info: _info,
                    error: _error,
                    showName: false,
                    // 与桌面同一处：能力目录嵌在这条 Provider 的详情里
                    capability: _cap,
                    capabilityBusy: _capBusy,
                    probeBusy: _probeBusy,
                    onRefreshCapability: _loading
                        ? null
                        : _loadCapabilityCatalog,
                    onUpdateCapabilityRemote: _refreshCapabilityCatalog,
                    onProbeToggle: _toggleProbe,
                    onProbe: _probeThinkingLadders,
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
        return SettingsPageScaffold(
          key: const ValueKey('ai-provider-list'),
          title: 'AI 模型与凭据',
          child: Column(
            children: [
              if (_error != null) _errorBanner(theme),
              Expanded(
                child: AiProviderListPanel(
                  providers: _providers,
                  selectedId: _selected?.id,
                  loading: _loading,
                  local: _store.isLocal,
                  onSelect: (p) async {
                    await _select(p);
                    // 手机上点一条 = **进子界面**（不是"在下面换个详情"）
                    if (mounted && p != null) {
                      setState(() => _pane = _NarrowPane.detail);
                    }
                  },
                  onCreate: _createProvider,
                  onDelete: _deleteProvider,
                  onSync: _store.isLocal ? _syncProvidersFromDesktop : null,
                  syncing: _syncingProviders,
                ),
              ),
            ],
          ),
        );
    }
  }

  /// 统一的界面反馈：**任何失败都要提示**。
  ///
  /// 此前的问题：错误只渲染在右侧详情面板中，而详情面板需先选中一个 Provider 才显示 ——
  /// 否则"新建 Provider 失败"在界面上无任何反馈，表现为"点击后未添加"。
  void _notify(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          // 长错误（比如"更新能力目录失败，接口不存在: /api/ai/capabilities/refresh"）
          // 在手机上会超出浮层允许的高度，画成 `BOTTOM OVERFLOWED BY 14 PIXELS`
          // 。套一层滚动容器：
          // 高度不够就**滚动**，而不是把内容画到框外面去。消息一个字都不改。
          content: SingleChildScrollView(child: Text(message)),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error
              ? Theme.of(context).colorScheme.errorContainer
              : null,
        ),
      );
  }

  Future<void> _createProvider() async {
    final draft = await showDialog<AiProviderDraft>(
      context: context,
      builder: (_) => const AiProviderDialog(),
    );
    if (draft == null) return;
    try {
      final created = await _store.createProvider({
        'id': draft.id,
        'displayName': draft.displayName,
        'api': draft.api.wire,
        'baseURL': draft.baseURL,
        // 凭据引用名**由程序决定**（不在界面上显示）：Provider ID 全大写 + `_API_KEY`。
        'credentialRef': defaultCredentialRef(draft.id),
        // ⚠ **不发端点信任级别**：它由软件按地址自己判（桌面后端 `EndpointTrust.infer`；
        //    手机侧存默认值，且手机侧根本不读这个字段 —— 见 `ProviderJson` 的注释）。
        //    实测问题：强加"仅本机"会使公网地址立即被后端拒绝，而错误不可见。
      });
      // 对话框中填写密钥后即保存（"填完即可用"）；留空不算错误，详情页仍可补充。
      final saved = await _saveDialogKey(created, draft.apiKey);
      await _reload();
      await _select(created);
      // 手机上创建后直接进入详情（否则需要在列表中再次查找新建条目）
      if (mounted && MediaQuery.sizeOf(context).width < 900) {
        setState(() => _pane = _NarrowPane.detail);
      }
      _notify(
        saved
            ? '已创建 Provider「${created.displayName}」，API Key 已保存。'
            : '已创建 Provider「${created.displayName}」，接下来可以填 API Key 和模型目录。',
      );
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('创建失败，$e', error: true);
    }
  }

  /// 对话框里那一栏密钥：填了就存，留空返回 `false`（不是错误）。
  ///
  /// 存密钥失败**只提示、不撤销 provider** —— provider 本身已经建好了，
  /// 丢弃刚填写的整份配置与 provider，后果比"密钥未保存"更严重。
  Future<bool> _saveDialogKey(AiProvider provider, String key) async {
    if (key.trim().isEmpty) return false;
    try {
      await _credentials.save(provider, key);
      return true;
    } catch (e) {
      _notify('Provider 已保存，但 API Key 没存上，$e', error: true);
      return false;
    }
  }

  /// **编辑一条已有的**（例如修改 baseURL）。
  ///
  /// 四条与桌面 POST/PUT 对齐的纪律：
  ///  1. **ID 不给改**（对话框里锁住）：桌面 `PUT` 的判据是"body 里的 id 不许与路径不同"，
  ///     `POST` 撞到已存在的 id 直接报"Provider ID 已存在"；
  ///  2. **带 revision 提交**（乐观锁 AIH-007）：手机侧库里只有一个写者，但**语义照旧** ——
  ///     桌面那份清单可能同时被别的窗口改过，而两端看到的错误要是同一句话；
  ///  3. 提交走 [_store]（手机 = 本机库，**不出网**）；
  ///  4. **凭据引用名沿用原值**（老行可能是别的名字）：它跟着 ID 重推等于换了一把钥匙的
  ///     槽位 —— 已保存的密钥会**立即失效**，界面上只显示"未配置"。
  Future<void> _editProvider(AiProvider p) async {
    final draft = await showDialog<AiProviderDraft>(
      context: context,
      builder: (_) => AiProviderDialog(existing: p),
    );
    if (draft == null) return;
    final existingRef = (p.credentialRef ?? '').trim();
    try {
      final updated = await _store.updateProvider(
        AiProvider(
          id: p.id,
          displayName: draft.displayName,
          api: draft.api.wire,
          baseURL: draft.baseURL,
          // 见第 4 条：有就沿用，没有（历史数据）才由程序按 ID 推一个
          credentialRef: existingRef.isEmpty
              ? defaultCredentialRef(p.id)
              : existingRef,
          // 端点信任级别**不由手动选择**：桌面端由后端按**新地址**重新推断
          // （`RemoteProviderStore.updateProvider` 不包含该键 —— 修改地址后旧档位可能
          //  已不适用于新地址，沿用旧档会导致界面无法修改的错误）；
          // 手机侧不读它，沿用原值。
          endpointTrust: p.endpointTrust,
          enabled: p.enabled,
          revision: p.revision,
          credential: p.credential,
        ),
      );
      final saved = await _saveDialogKey(updated, draft.apiKey);
      await _reload();
      await _select(updated);
      _notify(
        saved
            ? '已保存 Provider「${updated.displayName}」（revision ${updated.revision}），API Key 已保存。'
            : '已保存 Provider「${updated.displayName}」（revision ${updated.revision}）。',
      );
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('保存失败，$e', error: true);
    }
  }

  Future<void> _deleteProvider(AiProvider p) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('删除 Provider「${p.displayName}」？'),
        content: Text(
          p.credential.configured
              ? '已有会话会保留它的 Provider / 模型快照，但不能再继续发送。它配置的密钥会一并删除。'
              : '已有会话会保留它的 Provider / 模型快照，但不能再继续发送。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('删除'),
          ),
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
      _notify('删除失败，$e', error: true);
    }
  }

  // ---------------------------------------------------------------------------
  //  从电脑同步 Provider 清单（`docs/pairing-design.md` §14.6）
  // ---------------------------------------------------------------------------

  /// **把桌面那份清单搬到本机**（只在手机上出现）。
  ///
  /// 为什么要有这个入口：两端清单是**各一份**（本机表 / 后端库，§14.4），分开之后
  /// "在桌面上配好一条 Provider"对手机毫无帮助 —— 手机上还得从头再敲一遍 Base URL、
  /// 模型能力声明（敲错一个字符的表现是"附件发不出去 / 思考参数不发"，两端都不报错）。
  ///
  /// 三条纪律（真活在 [ProviderSync] 里，这里只管界面与反馈）：
  ///  ① **覆盖需先确认一次**（同 id 会被桌面那份覆盖，未经确认修改配置属于最差的一类"顺带改动"）；
  ///  ② 结束时**如实报账**（建了几个 / 覆盖了几个 / 本机独有几个保留 / 哪几条没成）；
  ///  ③ 完成后**让聊天页那份清单也跟上**（[AiWorkspaceStore.refreshProviders]）——
  ///     否则切回后显示"同步完成但无选项"，需要再次切换页面。
  Future<void> _syncProvidersFromDesktop() async {
    if (!_store.isLocal || _syncingProviders) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('从电脑同步 Provider？'),
        content: const SingleChildScrollView(
          child: Text(
            '把桌面上的 Provider 清单与模型目录搬到这台手机。\n\n'
            '桌面有、手机没有的会新建。同 id 的用桌面那份覆盖，包括显示名、协议、Base URL 和模型能力。'
            '只在手机上有的保留不删。\n\n'
            'API Key 不在其中，它要单独在「远程访问与配对」里取回，而且桌面要允许这台设备读密钥。',
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('开始同步'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    setState(() {
      _syncingProviders = true;
      _error = null;
    });
    try {
      // ⚠ **不在这里主动读令牌**：真源是 Android Keystore，而这一页只通过
      // `AiApiClient` 发请求 —— 它的令牌默认取自进程内缓存（`PairingTokenCache`，
      // 由 AI 工作台冷启动时读进来）。这一页自己去撞一次桥有两个坏处：
      // ① 桥不归它管（值只该经手一次）；② 真机之外的宿主（单测）上那次调用会**悬着不返回**，
      // 表现为"点了同步永远转圈"。没令牌时后端会回 401/403，下面那句人话已经足够可读。
      final report = await ProviderSync(desktop: _api, local: _store).run();
      if (!mounted) return;
      await _reload();
      if (!mounted) return;
      // 聊天页的清单是它自己读的（`AiWorkspaceStore.providers`），这里顺带刷新一次。
      // 取不到这个 store（单测 / 宿主没挂）时什么都不做 —— 不该因为"状态没挂上"就报错。
      try {
        await context.read<AiWorkspaceStore?>()?.refreshProviders();
      } catch (_) {
        // 只是聊天页的缓存没跟上，本机库已经写好了
      }
      if (!mounted) return;
      // 先收掉按钮上的转圈，再弹结果：转圈还在跑的时候 `pumpAndSettle` 永远等不到静止，
      // 而语义上"同步"这一刻也确实结束了（下面只是报账）。
      setState(() => _syncingProviders = false);
      await _showSyncReport(report);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('从电脑同步失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _syncingProviders = false);
    }
  }

  /// 同步结果的**如实报账**（+ "本机还缺哪几把 key"时给一个一键取回的出口）。
  Future<void> _showSyncReport(ProviderSyncReport report) async {
    if (!report.ok) {
      _notify(report.error!, error: true);
      return;
    }
    final lines = <String>[
      if (report.created.isNotEmpty)
        '新建 ${report.created.length} 个（${report.created.join('、')}）',
      if (report.updated.isNotEmpty)
        '覆盖 ${report.updated.length} 个（${report.updated.join('、')}）',
      if (report.unchanged.isNotEmpty) '${report.unchanged.length} 个与桌面一致，没有改动',
      '模型目录写入 ${report.modelsWritten} 条',
      if (report.localOnly.isNotEmpty)
        '只在手机上有的 ${report.localOnly.length} 个保留不动（${report.localOnly.join('、')}）',
    ];
    if (report.problems.isNotEmpty) {
      lines.add('还有 ${report.problems.length} 条没同步成。');
      for (final p in report.problems) {
        lines.add('· $p');
      }
    }
    // 缺密钥只说给**能取**的人听（本机密钥库 + 已挂上 AI 工作台）。
    final missing = [
      for (final p in _providers)
        if ((p.credentialRef ?? '').trim().isNotEmpty &&
            !p.credential.configured)
          p.displayName,
    ];
    final canPullKeys =
        _credentials.isLocal &&
        missing.isNotEmpty &&
        context.read<AiWorkspaceStore?>() != null;
    if (missing.isNotEmpty) {
      lines.add(
        canPullKeys
            ? '其中 ${missing.length} 个在本机还没有密钥（${missing.join('、')}）—— 可以接着从桌面取回'
            : '其中 ${missing.length} 个在本机还没有密钥 —— 去「远程访问与配对」点「从桌面同步密钥」取回'
                  '（桌面要允许这台设备读取密钥）',
      );
    }

    final choice = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('同步完成'),
        content: SingleChildScrollView(child: Text(lines.join('\n'))),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, 'ok'),
            child: const Text('好的'),
          ),
          if (canPullKeys)
            FilledButton(
              onPressed: () => Navigator.pop(ctx, 'keys'),
              child: const Text('再取回密钥'),
            ),
        ],
      ),
    );
    if (choice == 'keys' && mounted) await _pullKeysFromDesktop();
  }

  /// 接着把密钥取回本机（真活在 [AiWorkspaceStore.syncCredentialsFromDesktop]：
  /// 值只经手一次、只进本机密钥库，界面不碰它）。
  Future<void> _pullKeysFromDesktop() async {
    final store = context.read<AiWorkspaceStore?>();
    if (store == null) {
      _notify('AI 工作台还没准备好，稍后再试一次。', error: true);
      return;
    }
    try {
      // 它读的是自己那份 `providers` —— 刚同步来的那几条要先让它看见，否则 refs 是旧的
      await store.refreshProviders();
      final text = await store.syncCredentialsFromDesktop();
      if (mounted) await _reload();
      _notify(text);
    } catch (e) {
      _notify('从桌面同步密钥失败，$e', error: true);
    }
  }
}

extension _FirstOrNullExt<T> on Iterable<T> {
  T? get firstOrNull => isEmpty ? null : first;
}
