import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import 'prompts_page.dart' show confirmDialog;

/// 角色卡（O8，2026-09-26；2026-10 收敛成三个字段）。
///
/// 一张卡 = 一个角色的**名称 + 正面提示词 + 负面提示词**（用户原话："角色卡只保留：
/// 名称、正面提示词、负面提示词"）。它解决两件事：
///  1. **AI 别再凭印象编 tag**：写角色之前先 `search_character_cards`（有卡就照卡写），
///     没有就用 `danbooru_lookup` 核对写法，再把要给这个角色的提示词存下来；
///  2. **用户看得见、改得动**：模型存了什么，这一页能一眼看出来、能直接改。
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
      if (!mounted) return;
      setState(() {
        _cards = cards;
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
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _edit(null),
        icon: const Icon(Icons.add),
        label: const Text('新建角色卡'),
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _searchInput,
                    decoration: InputDecoration(
                      isDense: true,
                      prefixIcon: const Icon(Icons.search, size: 18),
                      hintText: '搜角色名 / 提示词',
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
            ? '一张卡 = 一个角色的名称 + 正面提示词 + 负面提示词。\n'
                '也可以让 AI 写角色时自己核对 danbooru 并存一张 —— 你在这一页能看到它存了什么。'
            : '换个词再搜（角色名 / 提示词里的词）。',
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
      ),
    );
  }
}

/// 一张角色卡（列表里的一项）：**名称 / 正面提示词 / 负面提示词**，就这三样。
class _CardTile extends StatelessWidget {
  final CharacterCard card;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  const _CardTile({
    required this.card,
    required this.onEdit,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final prompt = card.prompt;
    final negative = card.negative;
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
                  child: Text(
                    card.name,
                    style: theme.textTheme.titleSmall,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
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
            // 正面提示词：卡片上最主要的内容，**空着就明说空着**（用户要能一眼看出哪张还没写完）
            if (prompt == null)
              Text(
                '还没有正面提示词',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              )
            else
              Text(
                prompt,
                style: theme.textTheme.bodySmall,
                maxLines: 4,
                overflow: TextOverflow.ellipsis,
              ),
            if (negative != null) ...[
              const SizedBox(height: 6),
              Text(
                '负面：$negative',
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

/// 新建 / 编辑角色卡：**只有名称 / 正面提示词 / 负面提示词**三个输入框。
///
/// 字段与后端 `CharacterCardInput` 一一对应；**保存是全量替换**（后端 `PUT` 语义），
/// 所以在这里清空一个字段就真的清掉它 —— 用户改完看到的就是卡里的全部内容。
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

  @override
  void initState() {
    super.initState();
    final card = widget.card;
    _c = {
      'name': TextEditingController(text: card?.name ?? ''),
      'prompt': TextEditingController(text: card?.prompt ?? ''),
      'negative': TextEditingController(text: card?.negative ?? ''),
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

  Future<void> _save() async {
    final name = _c['name']!.text.trim();
    if (name.isEmpty) {
      setState(() => _error = '角色名不能为空');
      return;
    }
    setState(() {
      _saving = true;
      _error = null;
    });
    // 全量替换：空字段发 null（后端落 NULL），用户在弹窗里清空就真的清掉
    final payload = <String, dynamic>{
      'name': name,
      'prompt': _text('prompt'),
      'negative': _text('negative'),
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
            alignLabelWithHint: maxLines > 1,
            isDense: true,
            border: const OutlineInputBorder(),
          ),
        ),
      );

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.card == null ? '新建角色卡' : '编辑角色卡'),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _field('name', '角色名 *', hint: '用户叫的名字，例如 shinku'),
              _field(
                'prompt',
                '正面提示词',
                hint: '这个角色的规范 tag 栈 / 外观服饰基线，例如 shinku_(nte), red_eyes, dragon_girl',
                maxLines: 6,
              ),
              _field(
                'negative',
                '负面提示词',
                hint: '这个角色常踩的那些词',
                maxLines: 3,
              ),
              Text(
                '提示词会被 AI 直接照抄去出图（`search_character_cards` 查到的就是它），'
                '所以写清楚比写多更重要 —— 拿不准的 tag 先让 AI 用 danbooru_lookup 核对。',
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
