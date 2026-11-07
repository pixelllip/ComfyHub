import 'package:flutter/material.dart';

import '../models/ai_models.dart';
import 'ai_provider_widgets.dart';

// ---------------------------------------------------------------------------
//  新建 / 编辑 Provider 与模型的对话框
// ---------------------------------------------------------------------------
//
// 从 `ai_provider_settings_page.dart`（当时 2,956 行）拆出来的第四块：
// 这一页的"弹窗"与"页面"本来就是两件事 —— 弹窗只跟草稿 / 校验有关，
// 不需要看见清单、密钥输入框或模型列表。

/// 由 Provider ID 推出的**凭据引用名**（API Key 的存放名）：全大写、`-` 换成 `_`、末尾加 `_API_KEY`。
///
/// 后端要求环境变量风格（大写字母/数字/下划线，且**以字母开头**）。例：`my-gateway` → `MY_GATEWAY_API_KEY`。
///
/// ⚠ **用户看不到它、也不该问他**（2026-10 用户要求：端点信任级别与凭据引用名都由软件自动决定）——
/// 所以这里必须**永远推得出一个合法值**：Provider ID 允许以数字开头（`2fast` 是合法 kebab ID），
/// 而推出来的 `2FAST_API_KEY` 不是合法引用名，前缀一个 `P` 就合法了（`P2FAST_API_KEY`）。
/// 以前那种"ID 以数字开头就请用户手填"的做法，在表单里已经没有那一栏了。
String defaultCredentialRef(String providerId) {
  final name =
      '${providerId.trim().toUpperCase().replaceAll('-', '_')}_API_KEY';
  // 合法就原样用；不合法（ID 以数字开头）补一个前缀 —— **不改**名字本身
  return isValidCredentialRef(name) ? name : 'P$name';
}

/// 合法的引用名（后端 `require_credential_ref` 同一条正则）。
bool isValidCredentialRef(String ref) =>
    RegExp(r'^[A-Z][A-Z0-9_]{0,127}$').hasMatch(ref);

class AiProviderDraft {
  final String id;
  final String displayName;
  final AiApi api;
  final String baseURL;

  /// 对话框里填的 API Key（留空 = 先不填）。
  ///
  /// 它**不进**提交给 provider 接口的那份 body（provider 行里只有"这把钥匙叫什么名字"）；
  /// 建完之后由页面调 `CredentialStore.save` 单独存（桌面 = 后端托管，手机 = 本机密钥库）。
  final String apiKey;

  const AiProviderDraft(
    this.id,
    this.displayName,
    this.api,
    this.baseURL,
    this.apiKey,
  );
}

class AiProviderDialog extends StatefulWidget {
  /// 传了就是**编辑**那一条（ID 锁住：桌面 `PUT` 的纪律是"ID 创建后不可修改"，
  /// 而桌面 `POST` 遇到已存在的 id 会报"Provider ID 已存在"）。
  final AiProvider? existing;

  const AiProviderDialog({super.key, this.existing});

  @override
  State<AiProviderDialog> createState() => _AiProviderDialogState();
}

class _AiProviderDialogState extends State<AiProviderDialog> {
  final _id = TextEditingController();
  final _name = TextEditingController();
  final _url = TextEditingController();

  /// API Key：**只写不读**（AIH-012）。对话框打开时永远是空的，
  /// 编辑时留空 = 不动已有密钥（清除只能去详情里点「移除密钥」）。
  final _key = TextEditingController();
  AiApi _api = AiApi.openaiCompletions;

  bool get _editing => widget.existing != null;

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
      _api = AiApi.parse(existing.api) ?? AiApi.openaiCompletions;
    }
    // 输入时就实时校验，按钮状态跟着变
    _id.addListener(_revalidate);
    _url.addListener(_revalidate);
    _name.addListener(_revalidate);
  }

  void _revalidate() => setState(() {});

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    _url.dispose();
    _key.dispose();
    super.dispose();
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
    if (uri == null ||
        (uri.scheme != 'http' && uri.scheme != 'https') ||
        uri.host.isEmpty) {
      return '要写成完整的 http(s)://主机/路径';
    }
    return null;
  }

  /// 提交条件：**只有用户真的能填错的那几项**。
  ///
  /// 端点信任级别与凭据引用名都**不在表单里**（软件自动决定），所以这里不再校验它们 ——
  /// 引用名由 [`defaultCredentialRef`] 保证合法，信任档由后端按地址推断。
  bool get _canSubmit =>
      _kebab.hasMatch(_id.text.trim()) &&
      _name.text.trim().isNotEmpty &&
      _url.text.trim().isNotEmpty &&
      _urlError == null;

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
                decoration: const InputDecoration(
                  labelText: '显示名',
                  hintText: '公司网关',
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              DropdownButtonFormField<AiApi>(
                initialValue: _api,
                decoration: const InputDecoration(
                  labelText: 'API 协议',
                  isDense: true,
                ),
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
                  helperText: '只去掉末尾的 /，兼容网关的路径原样保留。',
                  errorText: _urlError,
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              // API Key：**填了就一起保存**（不用"先建 provider、再进去填一次"两步）。
              // 只写不读 —— 编辑时它永远是空的，留空表示不修改已有密钥。
              TextField(
                controller: _key,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: InputDecoration(
                  labelText: 'API Key（可选）',
                  hintText: 'sk-…',
                  helperText: _editing
                      ? '密钥只写不读，留空表示不修改已有密钥，要清除请到详情里点「移除密钥」。'
                      : '密钥只写不读，填了就一起保存，留空可以先建好、稍后在详情里填。',
                  isDense: true,
                ),
              ),
              if (!_editing) ...[
                const SizedBox(height: 8),
                const AiNote(
                  text:
                      '端点信任级别（公网 / 本机 / 局域网）与凭据引用名都由程序按地址与 ID 自动决定，'
                      '不用你选。地址是本机就按本机放行，是公网就要求 https。',
                  error: false,
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: !_canSubmit
              ? null
              : () => Navigator.pop(
                  context,
                  AiProviderDraft(
                    _id.text.trim(),
                    _name.text.trim(),
                    _api,
                    _url.text.trim(),
                    _key.text,
                  ),
                ),
          child: Text(_editing ? '保存' : '创建'),
        ),
      ],
    );
  }
}

class AiModelDraft {
  final String id;
  final String displayName;
  final List<AiModality> modalities;
  final bool tools;
  const AiModelDraft(this.id, this.displayName, this.modalities, this.tools);
}

/// 候选模型：**自动预填**能力（逐维度：接口声明 > 内置目录 > 兜底；
/// 工具默认给上），并把来源标出来；加入前可以逐个调整。
class AiDiscoverDialog extends StatefulWidget {
  final AiDiscoverResult result;
  const AiDiscoverDialog({super.key, required this.result});

  @override
  State<AiDiscoverDialog> createState() => _AiDiscoverDialogState();
}

class _AiDiscoverDialogState extends State<AiDiscoverDialog> {
  /// id -> 用户确认后的能力（默认取预填值）
  late final Map<String, AiModelCandidate> _drafts = {
    for (final c in widget.result.candidates) c.id: c,
  };
  late final Set<String> _selected = widget.result.candidates
      .map((c) => c.id)
      .toSet();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final declared = widget.result.candidates
        .where((c) => c.capabilitySource == 'discovered')
        .length;
    final builtin = widget.result.candidates
        .where((c) => c.capabilitySource == 'builtin')
        .length;

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
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
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
                              Flexible(
                                child: Text(
                                  c.displayName,
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                              const SizedBox(width: 8),
                              AiSourceChip(
                                source: c.capabilitySource,
                                label: c.sourceLabel,
                              ),
                            ],
                          ),
                          subtitle: Text(
                            c.detail,
                            style: theme.textTheme.labelSmall,
                          ),
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
                                    label: Text(
                                      m.label,
                                      style: theme.textTheme.labelSmall,
                                    ),
                                    selected: c.modalities.contains(m),
                                    visualDensity: VisualDensity.compact,
                                    onSelected: (on) => setState(() {
                                      final next = [...c.modalities];
                                      if (on) {
                                        next.add(m);
                                      } else {
                                        next.remove(m);
                                      }
                                      _drafts[id] = c.copyWith(
                                        modalities: next,
                                      );
                                    }),
                                  ),
                                FilterChip(
                                  label: Text(
                                    '工具',
                                    style: theme.textTheme.labelSmall,
                                  ),
                                  selected: c.tools,
                                  visualDensity: VisualDensity.compact,
                                  onSelected: (on) => setState(
                                    () => _drafts[id] = c.copyWith(tools: on),
                                  ),
                                ),
                              ],
                            ),
                          ),
                        if (checked && c.capabilityNote != null)
                          Padding(
                            padding: const EdgeInsets.only(left: 8, bottom: 4),
                            child: Text(
                              c.capabilityNote!,
                              style: theme.textTheme.labelSmall?.copyWith(
                                color: theme.colorScheme.outline,
                              ),
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
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
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
class AiSourceChip extends StatelessWidget {
  final String source;
  final String label;
  const AiSourceChip({super.key, required this.source, required this.label});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (bg, fg) = switch (source) {
      'discovered' => (
        theme.colorScheme.primaryContainer,
        theme.colorScheme.onPrimaryContainer,
      ),
      'builtin' => (
        theme.colorScheme.tertiaryContainer,
        theme.colorScheme.onTertiaryContainer,
      ),
      'tested' => (
        theme.colorScheme.secondaryContainer,
        theme.colorScheme.onSecondaryContainer,
      ),
      _ => (
        theme.colorScheme.surfaceContainerHighest,
        theme.colorScheme.outline,
      ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(
        label,
        style: theme.textTheme.labelSmall?.copyWith(color: fg),
      ),
    );
  }
}

class AiModelDialog extends StatefulWidget {
  const AiModelDialog({super.key});

  @override
  State<AiModelDialog> createState() => _AiModelDialogState();
}

class _AiModelDialogState extends State<AiModelDialog> {
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
              decoration: const InputDecoration(
                labelText: '显示名',
                isDense: true,
              ),
            ),
            const SizedBox(height: 12),
            const Text('输入能力（不勾就是不支持，也不做猜测）'),
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
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(
            context,
            AiModelDraft(
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

/// 模型能力编辑的公共规则：**列表页与编辑弹窗共用这一份**，
/// 免得"点一下某个徽标把别的声明抹掉"这类问题在两边各写一遍。
class AiModelEdit {
  const AiModelEdit._();

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
  }) => AiModel(
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
    thinkingFormat: clearThinkingFormat
        ? null
        : (thinkingFormat ?? m.thinkingFormat),
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
    if (on &&
        modality != AiModality.text &&
        !transports.containsKey(modality.wire)) {
      transports[modality.wire] = const ['inline_base64'];
    }
    if (!on) transports.remove(modality.wire);
    return copy(
      m,
      inputModalities: next,
      attachmentTransports: transports,
      manual: true,
    );
  }

  /// 切换"支持推理"：关掉时必须一并清掉思考档位声明（后端也会校验两者一致）。
  static AiModel toggleReasoning(AiModel m, bool on) => copy(
    m,
    reasoning: on,
    thinkingEfforts: on ? m.thinkingEfforts : const {},
    manual: true,
  );

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
class AiModelEditorDialog extends StatefulWidget {
  const AiModelEditorDialog({
    super.key,
    required this.model,
    required this.busy,
  });

  final AiModel model;
  final bool busy;

  @override
  State<AiModelEditorDialog> createState() => _AiModelEditorDialogState();
}

class _AiModelEditorDialogState extends State<AiModelEditorDialog> {
  late AiModel _m = widget.model;

  void _update(AiModel next) => setState(() => _m = next);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final busy = widget.busy;
    return AlertDialog(
      title: Text('模型能力（${_m.displayName}）'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              SelectableText(
                _m.id,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 10),
              Text('输入模态与工具', style: theme.textTheme.titleSmall),
              const SizedBox(height: 6),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final modality in AiModality.values)
                    FilterChip(
                      label: Text(
                        modality.label,
                        style: theme.textTheme.labelSmall,
                      ),
                      selected: _m.supports(modality),
                      onSelected: busy
                          ? null
                          : (on) => _update(
                              AiModelEdit.toggleModality(_m, modality, on),
                            ),
                    ),
                  FilterChip(
                    label: Text('工具', style: theme.textTheme.labelSmall),
                    selected: _m.tools,
                    onSelected: busy
                        ? null
                        : (on) => _update(
                            AiModelEdit.copy(_m, tools: on, manual: true),
                          ),
                  ),
                ],
              ),
              const Divider(height: 24),
              Row(
                children: [
                  Expanded(
                    child: Text('思考强度', style: theme.textTheme.titleSmall),
                  ),
                  Switch(
                    value: _m.reasoning,
                    onChanged: busy
                        ? null
                        : (on) => _update(AiModelEdit.toggleReasoning(_m, on)),
                  ),
                ],
              ),
              Text(
                _m.reasoning
                    ? '勾选该模型真正支持的档位。「默认」不在这个列表里，它不发任何思考参数，'
                          '任何模型都成立，聊天界面永远可选。留空就是不声明。'
                          '真正的思考档位宁可不给选，也不要发出去被上游 400。'
                    : '该模型不支持推理。勾选后可以逐档声明。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              if (_m.reasoning) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    // 「默认」不列在这里：它是"什么都不声明"的那一档，不是模型能力。
                    for (final e in AiReasoningEffort.values.where(
                      (e) => e.isThinking,
                    ))
                      FilterChip(
                        label: Text(e.label, style: theme.textTheme.labelSmall),
                        selected: _m.thinkingEfforts.containsKey(e.wire),
                        onSelected: busy
                            ? null
                            : (on) {
                                final next = Map<String, String>.from(
                                  _m.thinkingEfforts,
                                );
                                if (on) {
                                  next[e.wire] = AiModelEdit.defaultEffortWire(
                                    e,
                                  );
                                } else {
                                  next.remove(e.wire);
                                }
                                _update(
                                  AiModelEdit.copy(
                                    _m,
                                    thinkingEfforts: next,
                                    manual: true,
                                  ),
                                );
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
                    helperText: 'OpenAI 风格用 reasoning_effort，DeepSeek / Qwen / Z.AI / OpenRouter 各有自己的字段组合。',
                    isDense: true,
                  ),
                  items: [
                    const DropdownMenuItem(
                      value: null,
                      child: Text('按协议默认（OpenAI 风格）'),
                    ),
                    for (final f in AiThinkingFormat.values)
                      DropdownMenuItem(value: f.wire, child: Text(f.label)),
                  ],
                  onChanged: busy
                      ? null
                      : (v) => _update(
                          v == null
                              ? AiModelEdit.copy(
                                  _m,
                                  clearThinkingFormat: true,
                                  manual: true,
                                )
                              : AiModelEdit.copy(
                                  _m,
                                  thinkingFormat: v,
                                  manual: true,
                                ),
                        ),
                ),
                if (_m.thinkingEfforts.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 6),
                    child: Text(
                      '实际发送的值是 ${_m.thinkingEfforts.entries.map((e) => '${e.key}→${e.value}').join('  ')}',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, _m),
          child: const Text('确定'),
        ),
      ],
    );
  }
}
