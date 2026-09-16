import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';

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

  const AiProviderSettingsPage({super.key, this.api});

  @override
  State<AiProviderSettingsPage> createState() => _AiProviderSettingsPageState();
}

class _AiProviderSettingsPageState extends State<AiProviderSettingsPage> {
  late AiApiClient _api;

  List<AiProvider> _providers = const [];
  List<AiModel> _models = const [];
  AiProvider? _selected;
  bool _loading = true;
  String? _error;
  String? _info;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _api = widget.api ?? AiApiClient(context.read<SettingsStore>().baseUrl);
      _reload();
    });
  }

  @override
  void dispose() {
    super.dispose();
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      _providers = await _api.listProviders();
      if (_selected != null) {
        _selected = _providers.where((p) => p.id == _selected!.id).firstOrNull;
      }
      _models = _selected == null ? const [] : await _api.listModels(_selected!.id);
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
        _models = await _api.listModels(p.id);
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

    final list = _ProviderList(
      providers: _providers,
      selectedId: _selected?.id,
      loading: _loading,
      onSelect: _select,
      onCreate: _createProvider,
      onDelete: _deleteProvider,
    );

    final detail = _selected == null
        ? Center(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Text(
                '选择一个 Provider 查看详情，或新建一个。\n'
                'API Key 只写不读：填进去之后界面、数据库普通字段和日志都不会再出现它。',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          )
        : _ProviderDetail(
            key: ValueKey(_selected!.id + _selected!.revision.toString()),
            api: _api,
            provider: _selected!,
            models: _models,
            info: _info,
            error: _error,
            onChanged: (msg) async {
              setState(() => _info = msg);
              await _reload();
            },
          );

    return Scaffold(
      appBar: AppBar(title: const Text('AI 模型与凭据')),
      body: Column(
        children: [
          // 没有选中 Provider 时，错误也要看得见（新建失败最常发生在这种情况下）
          if (_error != null)
            Container(
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
            ),
          Expanded(
            child: wide
                ? Row(
                    children: [
                      SizedBox(width: 280, child: list),
                      const VerticalDivider(width: 1),
                      Expanded(child: detail),
                    ],
                  )
                : Column(
                    children: [
                      SizedBox(height: 200, child: list),
                      const Divider(height: 1),
                      Expanded(child: detail),
                    ],
                  ),
          ),
        ],
      ),
    );
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
          content: Text(message),
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
      final created = await _api.createProvider({
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
      _notify('已创建 Provider「${created.displayName}」，接下来可以填 API Key 和模型目录。');
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
      _notify('创建失败：$e', error: true);
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
      await _api.deleteProvider(p.id);
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
  final Future<void> Function(AiProvider?) onSelect;
  final Future<void> Function() onCreate;
  final Future<void> Function(AiProvider) onDelete;

  const _ProviderList({
    required this.providers,
    required this.selectedId,
    required this.loading,
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
  final AiProvider provider;
  final List<AiModel> models;
  final String? info;
  final String? error;
  final Future<void> Function(String message) onChanged;

  const _ProviderDetail({
    super.key,
    required this.api,
    required this.provider,
    required this.models,
    required this.info,
    required this.error,
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
      Text(p.displayName, style: theme.textTheme.titleLarge),
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
            onPressed: _busy ? null : _test,
            icon: const Icon(Icons.network_check, size: 18),
            label: const Text('测试连接'),
          ),
        ],
      ),
      Text(
        '测试只发一次 GET 请求：不跟随重定向、10 秒超时，密钥不会出现在日志里。',
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
        TextField(
          controller: _key,
          obscureText: true,
          autocorrect: false,
          enableSuggestions: false,
          decoration: const InputDecoration(
            labelText: '粘贴密钥（留空表示不修改已有密钥）',
            helperText: '只粘贴值本身：不要带引号，也不要粘贴 NAME=value',
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
          ],
        ),
      ],
      const Divider(height: 32),
      Row(
        children: [
          Text('模型目录', style: theme.textTheme.titleSmall),
          const Spacer(),
          TextButton.icon(
            onPressed: _busy ? null : _discover,
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
        '能力必须显式声明：这里填了什么就是什么，Harness 不会根据模型名字猜（未知即不支持）。',
        style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
      ),
      const SizedBox(height: 8),
      if (_models.isEmpty) Text('还没有模型', style: theme.textTheme.bodySmall),
    ];
    final count = _models.length;
    final tail = <Widget>[
      const SizedBox(height: 12),
      FilledButton.icon(
        onPressed: _busy || _models.isEmpty ? null : _saveModels,
        icon: const Icon(Icons.save_outlined, size: 18),
        label: const Text('保存模型目录'),
      ),
    ];

    return ListView.builder(
      padding: const EdgeInsets.all(16),
      itemCount: header.length + count + tail.length,
      itemBuilder: (context, i) {
        if (i < header.length) return header[i];
        if (i < header.length + count) {
          // RepaintBoundary：勾选某个能力时不会把旁边已经画好的卡片一起重绘
          return RepaintBoundary(child: _modelTile(theme, i - header.length));
        }
        return tail[i - header.length - count];
      },
    );
  }

  Widget _modelTile(ThemeData theme, int index) {
    final m = _models[index];
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(m.displayName, style: theme.textTheme.titleSmall),
                ),
                IconButton(
                  tooltip: '移除',
                  icon: const Icon(Icons.close, size: 16),
                  onPressed: _busy ? null : () => setState(() { _models.removeAt(index); _markDirty(); }),
                ),
              ],
            ),
            Text(m.id, style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
            const SizedBox(height: 6),
            Wrap(
              spacing: 6,
              children: [
                for (final modality in AiModality.values)
                  FilterChip(
                    label: Text(modality.label, style: theme.textTheme.labelSmall),
                    selected: m.supports(modality),
                    onSelected: _busy
                        ? null
                        : (on) => setState(() { _models[index] = _toggleModality(m, modality, on); _markDirty(); }),
                  ),
                FilterChip(
                  label: Text('工具', style: theme.textTheme.labelSmall),
                  selected: m.tools,
                  onSelected: _busy ? null : (on) => setState(() { _models[index] = _copy(m, tools: on); _markDirty(); }),
                ),
              ],
            ),
            // 思考强度（AIH-056）：模型级声明，聊天框里的思考强度选择器就读这里
            _thinkingEditor(theme, index),
          ],
        ),
      ),
    );
  }

  AiModel _toggleModality(AiModel m, AiModality modality, bool on) {
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
    return _copy(m, inputModalities: next, attachmentTransports: transports);
  }

  /// 唯一的模型复制入口：**新增字段必须在这里带上**，否则切换某个徽标会把别的声明悄悄抹掉。
  AiModel _copy(
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

  /// 切换"支持推理"：关掉时必须一并清掉思考档位声明（后端也会校验两者一致）。
  AiModel _toggleReasoning(AiModel m, bool on) =>
      _copy(m, reasoning: on, thinkingEfforts: on ? m.thinkingEfforts : const {}, manual: true);

  /// 展开的模型编辑：思考档位 + 网关方言（AIH-056）。
  Widget _thinkingEditor(ThemeData theme, int index) {
    final m = _models[index];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Divider(height: 20),
        Row(
          children: [
            Expanded(
              child: Text('思考强度', style: theme.textTheme.labelLarge),
            ),
            Switch(
              value: m.reasoning,
              onChanged: _busy ? null : (on) => setState(() { _models[index] = _toggleReasoning(m, on); _markDirty(); }),
            ),
          ],
        ),
        Text(
          m.reasoning
              ? '勾选该模型真正支持的档位。留空 = 不声明，聊天界面不显示思考强度选择器（宁可不给选，也不要发出去被上游 400）。'
              : '该模型不支持推理；勾选后可以逐档声明。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
        if (m.reasoning) ...[
          const SizedBox(height: 8),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final e in AiReasoningEffort.values)
                FilterChip(
                  label: Text(e.label, style: theme.textTheme.labelSmall),
                  selected: m.thinkingEfforts.containsKey(e.wire),
                  onSelected: _busy
                      ? null
                      : (on) => setState(() {
                            final next = Map<String, String>.from(m.thinkingEfforts);
                            if (on) {
                              next[e.wire] = _defaultEffortWire(e);
                            } else {
                              next.remove(e.wire);
                            }
                            _models[index] = _copy(m, thinkingEfforts: next, manual: true);
                          }),
                ),
            ],
          ),
          const SizedBox(height: 8),
          DropdownButtonFormField<String>(
            initialValue: m.thinkingFormatEnum?.wire,
            isDense: true,
            decoration: const InputDecoration(
              labelText: '网关思考方言（同一个"高"落到哪个字段）',
              helperText: 'OpenAI 风格用 reasoning_effort；DeepSeek / Qwen / Z.AI / OpenRouter 各有自己的字段组合',
              isDense: true,
            ),
            items: [
              const DropdownMenuItem(value: null, child: Text('按协议默认（OpenAI 风格）')),
              for (final f in AiThinkingFormat.values)
                DropdownMenuItem(value: f.wire, child: Text(f.label)),
            ],
            onChanged: _busy
                ? null
                : (v) => setState(() => _models[index] = v == null
                    ? _copy(m, clearThinkingFormat: true, manual: true)
                    : _copy(m, thinkingFormat: v, manual: true)),
          ),
          if (m.thinkingEfforts.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '实际发送：${m.thinkingEfforts.entries.map((e) => '${e.key}→${e.value}').join('  ')}',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
        ],
      ],
    );
  }

  /// 各档位的默认"过线拼写"（与后端 ThinkingLevels.DEFAULT 对齐）。
  static String _defaultEffortWire(AiReasoningEffort e) => switch (e) {
        AiReasoningEffort.off => 'none',
        AiReasoningEffort.minimal => 'minimal',
        AiReasoningEffort.low => 'low',
        AiReasoningEffort.medium => 'medium',
        AiReasoningEffort.high => 'high',
        AiReasoningEffort.xhigh => 'xhigh',
        AiReasoningEffort.max => 'high',
      };

  Future<void> _test() async {
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
      final status = await widget.api.setCredential(widget.provider.id, value);
      _key.clear(); // 立刻丢弃，不在内存里多留一秒
      await widget.onChanged('密钥已保存（${status.source}）。值不会再被读回。');
      _toast('密钥已保存（${status.source}），值不会再被读回。');
    } catch (e) {
      _toast('保存密钥失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _removeKey() async {
    setState(() => _busy = true);
    try {
      await widget.api.removeCredential(widget.provider.id);
      await widget.onChanged('密钥已移除。');
      _toast('密钥已移除。');
    } catch (e) {
      _toast('移除失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 拉取候选模型。**发现只是候选**：勾选后先进本地列表，仍需点「保存模型目录」才落库；
  /// 能力（图片/视频/工具…）不会因为发现而被推断，一律保持"未声明"（AIH-009 / AIH-011）。
  Future<void> _discover() async {
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
        _toast('这些候选已经在目录里了');
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
      await widget.api.saveModels(
        widget.provider.id,
        _models
            .map((m) => {
                  'providerId': widget.provider.id,
                  'id': m.id,
                  'displayName': m.displayName,
                  'inputModalities': m.inputModalities,
                  'attachmentTransports': m.attachmentTransports,
                  'mimeAllowlist': m.mimeAllowlist,
                  'tools': m.tools,
                  'parallelTools': m.parallelTools,
                  'reasoning': m.reasoning,
                  // 思考强度声明（AIH-056）：空表 / null 都不发没用的字段
                  if (m.thinkingEfforts.isNotEmpty) 'thinkingEfforts': m.thinkingEfforts,
                  if (m.thinkingFormat != null) 'thinkingFormat': m.thinkingFormat,
                  'contextWindow': m.contextWindow,
                  'maxOutputTokens': m.maxOutputTokens,
                  'maxAttachmentCount': m.maxAttachmentCount,
                  'capabilitySource': m.capabilitySource,
                  'enabled': m.enabled,
                })
            .toList(),
      );
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
  const _ProviderDialog();

  @override
  State<_ProviderDialog> createState() => _ProviderDialogState();
}

class _ProviderDialogState extends State<_ProviderDialog> {
  final _id = TextEditingController();
  final _name = TextEditingController();
  final _url = TextEditingController();
  final _ref = TextEditingController();
  AiApi _api = AiApi.openaiCompletions;

  /// null = 自动（由后端按地址判断公网 / 本机）。
  ///
  /// 之前默认是"仅本机"，用户填了公网地址却忘了改这里，保存被后端拒绝，
  /// 而且错误看不见 —— 就是"配好了点添加没反应"。
  AiEndpointTrust? _trust;

  static final _kebab = RegExp(r'^[a-z0-9]+(-[a-z0-9]+)*$');

  @override
  void initState() {
    super.initState();
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
      title: const Text('新建 Provider'),
      content: SizedBox(
        width: 480,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _id,
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
          child: const Text('创建'),
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

/// 候选模型：**自动预填**输入模态（接口声明 > 内置目录 > 仅文本），
/// 并把来源标出来；加入前可以逐个调整。
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
  bool _tools = false;

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
