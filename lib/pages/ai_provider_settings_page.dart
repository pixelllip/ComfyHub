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
      body: wide
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
        'endpointTrust': draft.trust.wire,
      });
      await _reload();
      await _select(created);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
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
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
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

  @override
  void initState() {
    super.initState();
    _models.addAll(widget.models);
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

    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
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
        if (_models.isEmpty)
          Text('还没有模型', style: theme.textTheme.bodySmall)
        else
          for (var i = 0; i < _models.length; i++) _modelTile(theme, i),
        const SizedBox(height: 12),
        FilledButton.icon(
          onPressed: _busy || _models.isEmpty ? null : _saveModels,
          icon: const Icon(Icons.save_outlined, size: 18),
          label: const Text('保存模型目录'),
        ),
      ],
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
                  onPressed: _busy ? null : () => setState(() => _models.removeAt(index)),
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
                        : (on) => setState(() => _models[index] = _toggleModality(m, modality, on)),
                  ),
                FilterChip(
                  label: Text('工具', style: theme.textTheme.labelSmall),
                  selected: m.tools,
                  onSelected: _busy ? null : (on) => setState(() => _models[index] = _copy(m, tools: on)),
                ),
              ],
            ),
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
    return AiModel(
      providerId: m.providerId,
      id: m.id,
      displayName: m.displayName,
      inputModalities: next,
      attachmentTransports: transports,
      mimeAllowlist: m.mimeAllowlist,
      tools: m.tools,
      parallelTools: m.parallelTools,
      reasoning: m.reasoning,
      contextWindow: m.contextWindow,
      maxOutputTokens: m.maxOutputTokens,
      maxAttachmentCount: m.maxAttachmentCount,
      capabilitySource: 'manual',
      enabled: m.enabled,
    );
  }

  AiModel _copy(AiModel m, {bool? tools}) => AiModel(
        providerId: m.providerId,
        id: m.id,
        displayName: m.displayName,
        inputModalities: m.inputModalities,
        attachmentTransports: m.attachmentTransports,
        mimeAllowlist: m.mimeAllowlist,
        tools: tools ?? m.tools,
        parallelTools: m.parallelTools,
        reasoning: m.reasoning,
        contextWindow: m.contextWindow,
        maxOutputTokens: m.maxOutputTokens,
        maxAttachmentCount: m.maxAttachmentCount,
        capabilitySource: m.capabilitySource,
        enabled: m.enabled,
      );

  Future<void> _test() async {
    setState(() {
      _busy = true;
      _testResult = null;
    });
    try {
      final result = await widget.api.testProvider(widget.provider.id);
      setState(() => _testResult = result);
    } catch (e) {
      await widget.onChanged('连接测试失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _saveKey() async {
    final value = _key.text;
    if (value.trim().isEmpty) {
      await widget.onChanged('密钥留空 = 不修改；如果要清除请点「移除密钥」。');
      return;
    }
    setState(() => _busy = true);
    try {
      final status = await widget.api.setCredential(widget.provider.id, value);
      _key.clear(); // 立刻丢弃，不在内存里多留一秒
      await widget.onChanged('密钥已保存（${status.source}）。值不会再被读回。');
    } catch (e) {
      await widget.onChanged('保存密钥失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _removeKey() async {
    setState(() => _busy = true);
    try {
      await widget.api.removeCredential(widget.provider.id);
      await widget.onChanged('密钥已移除。');
    } catch (e) {
      await widget.onChanged('移除失败：$e');
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
        await widget.onChanged('获取模型失败：${result.display}');
        return;
      }
      if (result.candidates.isEmpty) {
        await widget.onChanged('端点没有返回模型列表（${result.message}）');
        return;
      }
      final picked = await showDialog<List<AiModelCandidate>>(
        context: context,
        builder: (_) => _DiscoverDialog(result: result),
      );
      if (picked == null || picked.isEmpty) return;
      var added = 0;
      setState(() {
        for (final c in picked) {
          if (_models.any((m) => m.id == c.id)) continue;
          _models.add(AiModel(
            providerId: widget.provider.id,
            id: c.id,
            displayName: c.displayName,
            // 只声明文本；其它能力必须由用户显式勾选
            inputModalities: const ['text'],
            contextWindow: c.contextWindow,
            maxOutputTokens: c.maxOutputTokens,
            capabilitySource: 'discovered',
          ));
          added++;
        }
      });
      await widget.onChanged('已加入 $added 个候选（别忘了点「保存模型目录」）');
    } catch (e) {
      await widget.onChanged('获取模型失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _addModel() async {    final draft = await showDialog<_ModelDraft>(
      context: context,
      builder: (_) => const _ModelDialog(),
    );
    if (draft == null) return;
    setState(() {
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
                  'contextWindow': m.contextWindow,
                  'maxOutputTokens': m.maxOutputTokens,
                  'maxAttachmentCount': m.maxAttachmentCount,
                  'capabilitySource': m.capabilitySource,
                  'enabled': m.enabled,
                })
            .toList(),
      );
      await widget.onChanged('模型目录已保存（${_models.length} 个）。');
    } catch (e) {
      await widget.onChanged('保存模型目录失败：$e');
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
  final AiEndpointTrust trust;
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
  AiEndpointTrust _trust = AiEndpointTrust.loopback;

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    _url.dispose();
    _ref.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('新建 Provider'),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _id,
                decoration: const InputDecoration(
                  labelText: 'Provider ID（小写 kebab-case，创建后不可改）',
                  hintText: 'my-gateway',
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _name,
                decoration: const InputDecoration(labelText: '显示名', isDense: true),
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
                decoration: const InputDecoration(
                  labelText: 'Base URL',
                  hintText: 'https://gateway.example/v1',
                  helperText: '只去掉末尾的 /；Anthropic 兼容网关的路径原样保留',
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              DropdownButtonFormField<AiEndpointTrust>(
                initialValue: _trust,
                decoration: const InputDecoration(
                  labelText: '端点信任级别',
                  helperText: '公网只允许 https；本机/局域网要显式选择，云元数据地址一律拒绝',
                  isDense: true,
                ),
                items: [
                  for (final t in AiEndpointTrust.values)
                    DropdownMenuItem(value: t, child: Text(t.label)),
                ],
                onChanged: (v) => setState(() => _trust = v ?? _trust),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _ref,
                decoration: const InputDecoration(
                  labelText: '凭据引用名（可选）',
                  hintText: 'MY_GATEWAY_API_KEY',
                  helperText: '只是名字；密钥在保存 Provider 后单独填写',
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
          onPressed: () => Navigator.pop(
            context,
            _ProviderDraft(
              _id.text.trim(),
              _name.text.trim(),
              _api,
              _url.text.trim(),
              _ref.text.trim().isEmpty ? null : _ref.text.trim(),
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

/// 候选模型勾选框：只列身份与容量，**不显示能力猜测**。
class _DiscoverDialog extends StatefulWidget {
  final AiDiscoverResult result;
  const _DiscoverDialog({required this.result});

  @override
  State<_DiscoverDialog> createState() => _DiscoverDialogState();
}

class _DiscoverDialogState extends State<_DiscoverDialog> {
  late final Set<String> _selected = widget.result.candidates.map((c) => c.id).toSet();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: Text('发现 ${widget.result.candidates.length} 个候选模型'),
      content: SizedBox(
        width: 520,
        height: 380,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '勾选后加入模型目录（还要点「保存模型目录」才生效）。'
              '能力不会被自动推断，请在保存前按需勾选文本/图片/视频/音频/文档。',
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 8),
            Expanded(
              child: ListView.builder(
                itemCount: widget.result.candidates.length,
                itemBuilder: (context, i) {
                  final c = widget.result.candidates[i];
                  return CheckboxListTile(
                    dense: true,
                    value: _selected.contains(c.id),
                    title: Text(c.displayName),
                    subtitle: Text(c.detail, style: theme.textTheme.labelSmall),
                    onChanged: (on) => setState(() {
                      if (on == true) {
                        _selected.add(c.id);
                      } else {
                        _selected.remove(c.id);
                      }
                    }),
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
            widget.result.candidates.where((c) => _selected.contains(c.id)).toList(),
          ),
          child: Text('加入 ${_selected.length} 个'),
        ),
      ],
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
