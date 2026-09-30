import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import 'prompts_page.dart' show confirmDialog;

/// 角色卡（O8，2026-09-26）。
///
/// 一张卡 = 一个角色的**规范 tag 栈 + 配哪条 LoRA / 权重 + 别名 + 来源 + 最后一次
/// 跟 danbooru 核对的时间**。它解决两件事：
///  1. **AI 别再凭印象编 tag**：写角色之前先 `search_character_cards`（有卡就照卡写），
///     没有就用 `danbooru_lookup` 核对写法、再 `save_character_card` 存下来；
///  2. **用户看得见、改得动**：模型存了什么、来源是什么、有没有核对过，这一页能一眼看出来
///     （`tagsVerifiedAt` 为空 = 那些 tag **没跟 danbooru 核对过**，界面上要明说）。
///
/// 位置：从「提示词」页右上角进来（`prompts_page.dart` 的 actions）。
/// **刻意不做成第六个一级导航项** —— 手机窄屏上六个图标会挤成两行标签，
/// 而角色卡天然属于"写提示词"这条工作流（`test/home_nav_test.dart` 也就一个字都不用动）。
///
/// 数据源是 `/api/ai/character-cards`（AI 域，走 [AiApiClient]）：
/// 与 AI 工具读写的是**同一张表**，所以这里看到的卡就是模型"先查三处"里查的那一份。
class CharacterCardsPage extends StatefulWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const CharacterCardsPage({super.key, this.api});

  @override
  State<CharacterCardsPage> createState() => _CharacterCardsPageState();
}

class _CharacterCardsPageState extends State<CharacterCardsPage> {
  late AiApiClient _api;
  bool _loading = true;
  String? _error;
  List<CharacterCard> _cards = const [];
  String _query = '';
  final _searchInput = TextEditingController();

  /// 启用数 + token 口径（**后端算的**，前端只显示）——生图角色卡框架那行「本 X · 总 Y/512」。
  CharacterSummary _summary = const CharacterSummary();

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
    _searchInput.dispose();
    super.dispose();
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final cards = await _api.listCharacterCards(query: _query);
      // 汇总口径单独一条：拿不到不该拦列表（那只是顶部那行数字）
      CharacterSummary summary = _summary;
      try {
        summary = await _api.characterSummary();
      } catch (_) {}
      if (!mounted) return;
      setState(() {
        _cards = cards;
        _summary = summary;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  /// 列表上的 ✓ 启用开关：**全量替换**那份卡（只发 enabled 会把别的字段清掉）。
  ///
  /// 超了 6 个上限时**后端报错、原样弹给用户**（它的话里写着"先关掉一个再启用"）——
  /// 前端不自己判上限，免得两处判据漂。
  Future<void> _toggleEnabled(CharacterCard card, bool on) async {
    try {
      final saved = await _api.updateCharacterCard(card.id, card.copyWith(enabled: on).toPayload());
      if (!mounted) return;
      setState(() {
        _cards = [for (final c in _cards) if (c.id == saved.id) saved else c];
      });
      await _reload(); // 拿新的启用数 / token 口径
    } catch (e) {
      _snack('$e');
    }
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _edit(CharacterCard? card) async {
    final saved = await showDialog<bool>(
      context: context,
      builder: (_) => _CardEditorDialog(api: _api, card: card),
    );
    if (saved == true) await _reload();
  }

  Future<void> _delete(CharacterCard card) async {
    final ok = await confirmDialog(
      context,
      title: '删除角色卡',
      message: '确定删除「${card.name}」？\n\n'
          '删掉之后 AI 写这个角色时就没有卡可查了 —— 它得重新去 danbooru 核对一遍。',
    );
    if (!ok) return;
    try {
      await _api.deleteCharacterCard(card.id);
      _snack('已删除「${card.name}」');
      await _reload();
    } catch (e) {
      _snack('删除失败：$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('角色卡'),
        actions: [
          IconButton(tooltip: '刷新', icon: const Icon(Icons.refresh), onPressed: _reload),
          const SizedBox(width: 4),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(1),
          child: Divider(height: 1, color: Theme.of(context).dividerColor),
        ),
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _edit(null),
        icon: const Icon(Icons.add),
        label: const Text('新建角色卡'),
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _searchInput,
                    decoration: InputDecoration(
                      isDense: true,
                      prefixIcon: const Icon(Icons.search, size: 18),
                      hintText: '搜角色名 / 别名 / 角色 tag / LoRA 名',
                      border: const OutlineInputBorder(),
                      suffixIcon: _query.isEmpty
                          ? null
                          : IconButton(
                              tooltip: '清除',
                              icon: const Icon(Icons.close, size: 18),
                              onPressed: () {
                                _searchInput.clear();
                                setState(() => _query = '');
                                _reload();
                              },
                            ),
                    ),
                    onSubmitted: (v) {
                      setState(() => _query = v.trim());
                      _reload();
                    },
                  ),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  onPressed: () {
                    setState(() => _query = _searchInput.text.trim());
                    _reload();
                  },
                  child: const Text('搜索'),
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 6),
            child: Row(
              children: [
                Icon(Icons.groups_outlined, size: 14, color: Theme.of(context).colorScheme.outline),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    _summary.line,
                    style: Theme.of(context)
                        .textTheme
                        .bodySmall
                        ?.copyWith(color: Theme.of(context).colorScheme.outline),
                  ),
                ),
                Text(
                  _summary.full ? '已满 6 个' : '最多同时启用 6 个',
                  style: Theme.of(context)
                      .textTheme
                      .bodySmall
                      ?.copyWith(color: Theme.of(context).colorScheme.outline),
                ),
              ],
            ),
          ),
          Expanded(child: _body()),
        ],
      ),
    );
  }

  Widget _body() {
    if (_loading) return const Center(child: CircularProgressIndicator());
    if (_error != null) return ErrorState(message: _error!, onRetry: _reload);
    if (_cards.isEmpty) {
      return EmptyState(
        icon: Icons.badge_outlined,
        title: _query.isEmpty ? '还没有角色卡' : '没有匹配的角色卡',
        subtitle: _query.isEmpty
            ? '角色卡存的是"这个角色的规范 tag 栈 + 配哪条 LoRA"。\n'
                '也可以让 AI 写角色时自己核对 danbooru 并存一张 —— 你在这一页能看到它存了什么。'
            : '换个词再搜（角色名 / 别名 / 角色 tag / LoRA 名）。',
      );
    }
    return AdaptiveColumnList(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
      runSpacing: 10,
      itemCount: _cards.length,
      itemBuilder: (context, i) => _CardTile(
        card: _cards[i],
        onEdit: () => _edit(_cards[i]),
        onDelete: () => _delete(_cards[i]),
        onToggle: (on) => _toggleEnabled(_cards[i], on),
      ),
    );
  }
}

/// 一张角色卡（列表里的一项）。
class _CardTile extends StatelessWidget {
  final CharacterCard card;
  final VoidCallback onEdit;
  final VoidCallback onDelete;
  final ValueChanged<bool> onToggle;

  const _CardTile({
    required this.card,
    required this.onEdit,
    required this.onDelete,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final verified = card.tagsVerifiedAt != null;
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        card.nameEn == null ? card.name : '${card.name}（${card.nameEn}）',
                        style: theme.textTheme.titleSmall,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      if (card.summary.isNotEmpty) ...[
                        const SizedBox(height: 2),
                        Text(
                          card.summary,
                          style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ],
                    ],
                  ),
                ),
                AppMenuButton<String>(
                  tooltip: '更多操作',
                  onSelected: (v) => v == 'edit' ? onEdit() : onDelete(),
                  options: const [
                    MenuOption(value: 'edit', icon: Icons.edit_outlined, label: '编辑'),
                    MenuOption(
                      value: 'delete',
                      icon: Icons.delete_outline,
                      label: '删除',
                      danger: true,
                      dividerBefore: true,
                    ),
                  ],
                  button: (context, controller, isOpen) => IconButton(
                    tooltip: '更多操作',
                    icon: const Icon(Icons.more_vert, size: 18),
                    onPressed: () => controller.isOpen ? controller.close() : controller.open(),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 6),
            // ✓ 启用开关 + 位置：框架里那两件（"最多同时启用 6 个"与"5×5 网格定位"）。
            // 未启用 = 留在库里待命、不占 token，所以这里说的是"参与本次生成"而不是"删除"。
            Row(
              children: [
                SizedBox(
                  height: 32,
                  child: Checkbox(
                    value: card.enabled,
                    visualDensity: VisualDensity.compact,
                    materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    onChanged: (v) => onToggle(v ?? false),
                  ),
                ),
                const SizedBox(width: 4),
                Text(
                  card.enabled ? '参与本次生成' : '未启用（不占 token）',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: card.enabled ? theme.colorScheme.primary : theme.colorScheme.outline,
                  ),
                ),
                const Spacer(),
                Tooltip(
                  message: '位置：5×5 网格（A1~E5）；「自动」= 让 AI 自己摆',
                  child: Chip(
                    visualDensity: VisualDensity.compact,
                    label: Text('位置 ${card.position ?? '自动'}', style: theme.textTheme.bodySmall),
                    avatar: const Icon(Icons.grid_on, size: 14),
                  ),
                ),
              ],
            ),
            if (!verified) ...[
              const SizedBox(height: 8),
              // "没核对过"是个**事实**，必须显式画出来（用户才能判断要不要让 AI 去查一遍）
              Row(
                children: [
                  Icon(Icons.info_outline, size: 14, color: theme.colorScheme.outline),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      '这些 tag 没跟 danbooru 核对过',
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                    ),
                  ),
                ],
              ),
            ],
            if (card.tags.isNotEmpty) ...[
              const SizedBox(height: 10),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final t in card.tags.take(12))
                    Container(
                      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                      decoration: BoxDecoration(
                        color: theme.colorScheme.surfaceContainerHighest,
                        borderRadius: BorderRadius.circular(10),
                      ),
                      child: Text(t, style: theme.textTheme.bodySmall),
                    ),
                  if (card.tags.length > 12)
                    Text('…共 ${card.tags.length} 个', style: theme.textTheme.bodySmall),
                ],
              ),
            ],
            if (card.notes != null) ...[
              const SizedBox(height: 10),
              Text(
                card.notes!,
                style: theme.textTheme.bodySmall,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
              ),
            ],
            if (card.source != null) ...[
              const SizedBox(height: 8),
              Text(
                '来源：${card.source}',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// 新建 / 编辑角色卡。
///
/// 字段与后端 `CharacterCardInput` 一一对应；**保存是全量替换**（后端 `PUT` 语义），
/// 所以在这里清空一个字段就真的清掉它 —— 用户改完看到的就是卡里的全部内容。
/// `tagsVerifiedAt` 不在这里改（那是 danbooru 那条工具写的事实），保存时原样带回。
class _CardEditorDialog extends StatefulWidget {
  final AiApiClient api;
  final CharacterCard? card;

  const _CardEditorDialog({required this.api, this.card});

  @override
  State<_CardEditorDialog> createState() => _CardEditorDialogState();
}

class _CardEditorDialogState extends State<_CardEditorDialog> {
  late final Map<String, TextEditingController> _c;
  bool _saving = false;
  String? _error;

  /// 位置：`null` = 自动（让 AI 摆）；否则是 5×5 里的一个格子（默认 `C3` 居中）。
  String? _position;
  /// 是否参与本次生成。
  bool _enabled = false;

  @override
  void initState() {
    super.initState();
    final card = widget.card;
    _position = card?.position;
    _enabled = card?.enabled ?? false;
    _c = {
      'name': TextEditingController(text: card?.name ?? ''),
      'nameEn': TextEditingController(text: card?.nameEn ?? ''),
      'characterTag': TextEditingController(text: card?.characterTag ?? ''),
      'seriesTag': TextEditingController(text: card?.seriesTag ?? ''),
      'aliases': TextEditingController(text: card?.aliases.join('、') ?? ''),
      'tags': TextEditingController(text: card?.tags.join(', ') ?? ''),
      'loraName': TextEditingController(text: card?.loraName ?? ''),
      'loraWeight': TextEditingController(text: card?.loraWeight?.toString() ?? ''),
      'loraTrigger': TextEditingController(text: card?.loraTrigger ?? ''),
      'checkpoint': TextEditingController(text: card?.checkpoint ?? ''),
      'source': TextEditingController(text: card?.source ?? ''),
      'negative': TextEditingController(text: card?.negative ?? ''),
      'notes': TextEditingController(text: card?.notes ?? ''),
    };
  }

  @override
  void dispose() {
    for (final c in _c.values) {
      c.dispose();
    }
    super.dispose();
  }

  String? _text(String key) {
    final v = _c[key]!.text.trim();
    return v.isEmpty ? null : v;
  }

  List<String> _split(String key) => _c[key]!
      .text
      .split(RegExp(r'[,，\n]'))
      .map((e) => e.trim())
      .where((e) => e.isNotEmpty)
      .toList();

  Future<void> _save() async {
    final name = _c['name']!.text.trim();
    if (name.isEmpty) {
      setState(() => _error = '角色名不能为空');
      return;
    }
    final weight = _c['loraWeight']!.text.trim();
    setState(() {
      _saving = true;
      _error = null;
    });
    final payload = <String, dynamic>{
      'name': name,
      'nameEn': _text('nameEn'),
      'aliases': _split('aliases'),
      'characterTag': _text('characterTag'),
      'seriesTag': _text('seriesTag'),
      'tags': _split('tags'),
      'negative': _text('negative'),
      'loraName': _text('loraName'),
      'loraWeight': weight.isEmpty ? null : double.tryParse(weight),
      'loraTrigger': _text('loraTrigger'),
      'checkpoint': _text('checkpoint'),
      'source': _text('source'),
      // 只有 AI 的 danbooru 工具写过它；界面不改、原样带回
      'tagsVerifiedAt': widget.card?.tagsVerifiedAt,
      'notes': _text('notes'),
      // 生图角色卡框架那两件
      'position': _position,
      'enabled': _enabled,
    };
    try {
      if (widget.card == null) {
        await widget.api.createCharacterCard(payload);
      } else {
        await widget.api.updateCharacterCard(widget.card!.id, payload);
      }
      if (!mounted) return;
      Navigator.pop(context, true);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _saving = false;
        _error = e.toString();
      });
    }
  }

  Widget _field(String key, String label, {String? hint, int maxLines = 1}) => Padding(
        padding: const EdgeInsets.only(bottom: 10),
        child: TextField(
          controller: _c[key],
          maxLines: maxLines,
          decoration: InputDecoration(
            labelText: label,
            hintText: hint,
            isDense: true,
            border: const OutlineInputBorder(),
          ),
        ),
      );

  /// 位置：**自动** 或 5×5 网格里的一个格子（默认 `C3` 居中）。
  ///
  /// 为什么是网格而不是"随便填个坐标"：框架给的就是 5 行（A~E）× 5 列（1~5）共 25 格，
  /// 它只是给模型的**构图提示**（谁在左、谁在右），不是像素坐标。
  Widget _positionPicker() {
    final theme = Theme.of(context);
    const rows = ['A', 'B', 'C', 'D', 'E'];
    const cols = [1, 2, 3, 4, 5];
    const defaultPosition = 'C3';
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text('位置', style: theme.textTheme.bodyMedium),
              const SizedBox(width: 8),
              Text(
                _position == null ? '自动（让 AI 自己摆）' : _position!,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              ),
              const Spacer(),
              TextButton(
                onPressed: () => setState(() => _position = null),
                child: const Text('自动'),
              ),
            ],
          ),
          // 5×5 网格：点哪格就是哪格；再点一次取消（回到自动）
          for (final row in rows)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final col in cols)
                  Padding(
                    padding: const EdgeInsets.only(right: 4, bottom: 4),
                    child: _GridCell(
                      label: '$row$col',
                      selected: _position == '$row$col',
                      onTap: () => setState(
                        () => _position = _position == '$row$col' ? null : '$row$col',
                      ),
                    ),
                  ),
              ],
            ),
          Text(
            '点了格子就是"这个人摆在那儿"（A~E 是行、1~5 是列，$defaultPosition 居中）；'
            '留「自动」让 AI 自己安排构图。',
            style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.card == null ? '新建角色卡' : '编辑角色卡'),
      content: SizedBox(
        width: 460,
        // 手机上字段多，必须能滚（否则直接 RenderFlex overflow）
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _field('name', '角色名 *', hint: '用户叫的名字，例如 shinku'),
              _field('nameEn', '英文 / 罗马字', hint: 'danbooru 上的写法，例如 Shinku'),
              _field('characterTag', 'danbooru 角色 tag', hint: '例如 shinku_(nte)'),
              _field('seriesTag', '作品 tag', hint: '例如 neverness_to_everness'),
              _field('aliases', '别名', hint: '用、或逗号分隔；danbooru 的 other names 可照抄'),
              _field('tags', '规范 tag 栈', hint: '逗号或换行分隔：1girl, red_eyes, dragon_girl…', maxLines: 4),
              _field('loraName', '角色 LoRA 文件名', hint: 'ComfyUI models/loras 下的名字'),
              _field('loraWeight', 'LoRA 权重', hint: '例如 0.7（留空 = 用工作流里的值）'),
              _field('loraTrigger', 'LoRA 触发词', hint: '例如 @nte'),
              _field('checkpoint', '建议底模', hint: '例如 anima-base-v1.0.safetensors'),
              _field('source', '来源', hint: '链接 / 「用户口述」 / danbooru'),
              _field('negative', '常踩的负面词', maxLines: 2),
              _field('notes', '备注', hint: '这个角色怎么画才对', maxLines: 3),
              _positionPicker(),
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                value: _enabled,
                onChanged: (v) => setState(() => _enabled = v),
                title: const Text('参与本次生成'),
                subtitle: Text(
                  _enabled
                      ? '会计入 token；单次生成最多同时启用 6 个'
                      : '留在角色库里待命：不参与生成、不占 token',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ),
              if (widget.card?.tagsVerifiedAt != null)
                Text(
                  'danbooru 核对时间：${widget.card!.tagsVerifiedAt}（由 AI 的 danbooru 查询写下，这里不改动）',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              if (_error != null) ...[
                const SizedBox(height: 10),
                Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _saving ? null : () => Navigator.pop(context, false),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: _saving ? null : _save,
          child: _saving
              ? const SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2))
              : const Text('保存'),
        ),
      ],
    );
  }
}

/// 5×5 网格里的一格（位置选择器用）。
///
/// 尺寸固定：25 格排下来必须放得进 460 宽的弹窗，且**窄屏也不溢出**
/// （`test/character_cards_test.dart` 在 384 宽下盯着）。
class _GridCell extends StatelessWidget {
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _GridCell({required this.label, required this.selected, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Container(
        width: 40,
        height: 30,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: selected ? theme.colorScheme.primaryContainer : theme.colorScheme.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(6),
          border: Border.all(
            color: selected ? theme.colorScheme.primary : theme.dividerColor,
            width: selected ? 1.5 : 0.5,
          ),
        ),
        child: Text(
          label,
          style: theme.textTheme.bodySmall?.copyWith(
            color: selected ? theme.colorScheme.onPrimaryContainer : theme.colorScheme.outline,
            fontWeight: selected ? FontWeight.w600 : null,
          ),
        ),
      ),
    );
  }
}
