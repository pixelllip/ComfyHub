import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/settings_store.dart';
import '../models/ai_models.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/adaptive_layout.dart';
import '../widgets/common.dart';
import '../widgets/nsfw_blur.dart';
import 'media_detail_page.dart';

/// 卡片种类。角色卡与画风卡**同等地位**，只是语义与文案不同。
enum CardKind {
  character,
  style;

  String get label => switch (this) {
    CardKind.character => '角色卡',
    CardKind.style => '画风卡',
  };

  String get nameLabel => switch (this) {
    CardKind.character => '角色名',
    CardKind.style => '画风名',
  };

  String get newLabel => switch (this) {
    CardKind.character => '新建角色卡',
    CardKind.style => '新建画风卡',
  };

  String get editTitle => switch (this) {
    CardKind.character => '编辑角色卡',
    CardKind.style => '编辑画风卡',
  };

  String get searchHint => switch (this) {
    CardKind.character => '搜角色名 / 正面提示词',
    CardKind.style => '搜画风名 / 正面提示词',
  };

  String get emptyTitle => switch (this) {
    CardKind.character => '还没有角色卡',
    CardKind.style => '还没有画风卡',
  };

  String get emptySubtitle => switch (this) {
    CardKind.character =>
      '一张角色卡 = 名称 + 一版自然语言正面提示词 + 负面提示词，\n'
          '还可以挂几张产物当示例。角色卡管"画的是谁"。',
    CardKind.style =>
      '一张画风卡 = 名称 + 一版自然语言正面提示词 + 负面提示词，\n'
          '还可以挂几张产物当示例。画风卡管"画成什么样"。',
  };

  String get editorHint => switch (this) {
    CardKind.character =>
      '正面提示词写成一整段自然语言（600+ token、英文约 450~600 词），从头到脚写全。\n'
          '要 Danbooru tag 版时让 AI 当场另写一版给你看 —— 卡片里只存自然语言。',
    CardKind.style =>
      '画风卡与角色卡同等地位，只写"画成什么样"（画师 / 媒介 / 上色 / 打光 / 材质 / 后期）。\n'
          '正面提示词至少 100~200 token（英文约 75~150 词），这是下限。复杂画风可以按'
          '面部 / 头发 / 衣物 / 其他物体分开写材质，该长就长。要 tag 版时让 AI 当场另写。',
  };

  /// 图标（空状态等地方用）。
  IconData get icon => switch (this) {
    CardKind.character => Icons.badge_outlined,
    CardKind.style => Icons.palette_outlined,
  };
}

/// 卡片视图（角色卡 / 画风卡）：搜索栏 + 列表 + 空 / 错 / loading。
///
/// **没有 Scaffold / AppBar / FAB**：预设页把它当作「角色卡」「画风卡」两个
/// 同级页签的内容，顶部导航与新建按钮属于宿主页面（跟着页签换）。
/// [CardsPage] 是给"单独打开一类卡"的老入口 / 用例用的壳子。
class CardsView extends StatefulWidget {
  final CardKind kind;

  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const CardsView({super.key, required this.kind, this.api});

  @override
  State<CardsView> createState() => CardsViewState();
}

class CardsViewState extends State<CardsView> {
  late AiApiClient _api;
  bool _loading = true;
  String? _error;
  List<CharacterCard> _cards = const [];
  String _query = '';
  final _searchInput = TextEditingController();

  /// 切种类 / 连按搜索时旧请求可能晚到；只认最后一次。
  int _loadSeq = 0;

  CardKind get _kind => widget.kind;

  @override
  void initState() {
    super.initState();
    // `context.read` 在 initState 里是允许的（不订阅变化）
    _api = widget.api ?? AiApiClient(context.read<SettingsStore>().baseUrl);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _reload();
    });
  }

  @override
  void dispose() {
    _searchInput.dispose();
    super.dispose();
  }

  /// 供宿主页的「新建」按钮调用（预设页的 FAB 会按当前页签调它）。
  Future<void> createNew() => _edit(null);

  /// 供宿主页的「刷新」按钮调用。
  Future<void> reload() => _reload();

  Future<void> _reload() async {
    final seq = ++_loadSeq;
    final kind = _kind;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final cards = kind == CardKind.character
          ? await _api.listCharacterCards(query: _query)
          : await _api.listStyleCards(query: _query);
      if (!mounted || seq != _loadSeq || kind != _kind) return;
      setState(() {
        _cards = cards;
        _loading = false;
      });
    } catch (e) {
      if (!mounted || seq != _loadSeq || kind != _kind) return;
      setState(() {
        _error = e.toString();
        _loading = false;
      });
    }
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _edit(CharacterCard? card) async {
    final saved = await showDialog<bool>(
      context: context,
      builder: (_) => _CardEditorDialog(kind: _kind, api: _api, card: card),
    );
    if (saved == true) await _reload();
  }

  Future<void> _delete(CharacterCard card) async {
    final ok = await _confirm(
      title: '删除${_kind.label}',
      message:
          '确定删除「${card.name}」？\n\n'
          '删掉之后 AI 写这个${_kind == CardKind.character ? "角色" : "画风"}时就没有卡可查了。',
    );
    if (!ok) return;
    try {
      if (_kind == CardKind.character) {
        await _api.deleteCharacterCard(card.id);
      } else {
        await _api.deleteStyleCard(card.id);
      }
      _snack('已删除「${card.name}」');
      await _reload();
    } catch (e) {
      _snack('删除失败，$e');
    }
  }

  /// 管理示例产物：选择器返回**最终**的 mediaId 列表（空 = 全部移除）。
  Future<void> _manageExamples(CharacterCard card) async {
    final picked = await showDialog<List<int>>(
      context: context,
      builder: (_) => _ExamplePickerDialog(api: _api, initial: card.examples),
    );
    if (picked == null || !mounted) return;
    try {
      if (_kind == CardKind.character) {
        await _api.replaceCharacterCardExamples(card.id, picked);
      } else {
        await _api.replaceStyleCardExamples(card.id, picked);
      }
      _snack(picked.isEmpty ? '已清空示例产物' : '已保存 ${picked.length} 个示例产物');
      await _reload();
    } catch (e) {
      _snack('保存示例失败，$e');
    }
  }

  Future<bool> _confirm({
    required String title,
    required String message,
  }) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: Text(message),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: Theme.of(ctx).colorScheme.error,
            ),
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    return ok == true;
  }

  @override
  Widget build(BuildContext context) {
    // 只有搜索栏 + 列表：页面骨架（AppBar / FAB / 页签）属于宿主
    return Column(
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
                    hintText: _kind.searchHint,
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
    );
  }

  Widget _body() {
    if (_loading) return const Center(child: CircularProgressIndicator());
    if (_error != null) return ErrorState(message: _error!, onRetry: _reload);
    if (_cards.isEmpty) {
      return EmptyState(
        icon: _kind.icon,
        title: _query.isEmpty ? _kind.emptyTitle : '没有匹配的${_kind.label}',
        subtitle: _query.isEmpty
            ? '${_kind.emptySubtitle}\n也可以让 AI 写完自己存 —— 你在这一页能看到它存了什么。'
            : '换个词再搜（名字 / 正面提示词里的词）。',
      );
    }
    return AdaptiveColumnList(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
      runSpacing: 10,
      itemCount: _cards.length,
      itemBuilder: (context, i) => _CardTile(
        card: _cards[i],
        kind: _kind,
        apiBase: _api.baseUrl,
        mediaPosterUrl: _api.mediaPosterUrl,
        onEdit: () => _edit(_cards[i]),
        onDelete: () => _delete(_cards[i]),
        onManageExamples: () => _manageExamples(_cards[i]),
        onOpenExample: (media) => _openExample(_cards[i], media),
      ),
    );
  }

  void _openExample(CharacterCard card, MediaAsset media) {
    // 测试里可能没有 LibraryStore；那时缩略图只做展示，点开无动作（不崩）。
    // ⚠ 别用 `read<LibraryStore?>()` —— provider 注册的是非空类型，问可空类型
    // 找不到同一个 InheritedWidget（会永远返回 null，真机上点缩略图就"没反应"）。
    final LibraryStore store;
    try {
      store = context.read<LibraryStore>();
    } catch (_) {
      return;
    }
    if (store.api.baseUrl.isEmpty) return;
    Navigator.of(context).push(
      MaterialPageRoute(
        // 详情页的左右切换只在这张卡挂的示例里走 —— 不传的话它会按
        // `collectionId` 把用户带回画廊里原本那一批（用户 2026-11-07 报的 bug）。
        builder: (_) =>
            MediaDetailPage(mediaId: media.id, siblings: card.examples),
      ),
    );
  }
}

/// 卡片库的独立页面（老入口 / 用例用）。
///
/// 内部就是 [CardsView]；**预设页不再走这里** —— 它把两个 [CardsView] 直接当作
/// 「角色卡」「画风卡」两个同级页签（用户 2026-10-24 的要求），顶部只有一条导航。
class CardsPage extends StatefulWidget {
  final CardKind initialKind;

  /// 是否显示顶部的"角色卡 / 画风卡"切换。独立入口用 true；
  /// 旧的两个包装页（[CharacterCardsPage] / [StyleCardsPage]）用 false。
  final bool switchable;

  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final AiApiClient? api;

  const CardsPage({
    super.key,
    this.initialKind = CardKind.character,
    this.switchable = true,
    this.api,
  });

  @override
  State<CardsPage> createState() => _CardsPageState();
}

class _CardsPageState extends State<CardsPage> {
  late CardKind _kind = widget.initialKind;

  /// 两种卡各留一个 key：切过去时**新实例**自己加载，切回来也还在（搜索词不丢）。
  final Map<CardKind, GlobalKey<CardsViewState>> _viewKeys = {
    for (final kind in CardKind.values) kind: GlobalKey<CardsViewState>(),
  };

  @override
  Widget build(BuildContext context) {
    final key = _viewKeys[_kind]!;
    return Scaffold(
      appBar: AppBar(
        title: Text(_kind.label),
        actions: [
          IconButton(
            tooltip: '刷新',
            icon: const Icon(Icons.refresh),
            onPressed: () => key.currentState?.reload(),
          ),
          const SizedBox(width: 4),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => key.currentState?.createNew(),
        icon: const Icon(Icons.add),
        label: Text(_kind.newLabel),
      ),
      body: Column(
        children: [
          if (widget.switchable)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 10, 16, 0),
              child: Align(
                alignment: Alignment.centerLeft,
                child: SegmentedButton<CardKind>(
                  showSelectedIcon: false,
                  style: const ButtonStyle(
                    visualDensity: VisualDensity.compact,
                  ),
                  segments: const [
                    ButtonSegment(
                      value: CardKind.character,
                      icon: Icon(Icons.badge_outlined, size: 18),
                      label: Text('角色卡'),
                    ),
                    ButtonSegment(
                      value: CardKind.style,
                      icon: Icon(Icons.palette_outlined, size: 18),
                      label: Text('画风卡'),
                    ),
                  ],
                  selected: {_kind},
                  onSelectionChanged: (s) => setState(() => _kind = s.first),
                ),
              ),
            ),
          Expanded(
            child: CardsView(key: key, kind: _kind, api: widget.api),
          ),
        ],
      ),
    );
  }
}

/// 一张卡（列表里的一项）：名称、正面提示词、示例产物。
class _CardTile extends StatelessWidget {
  final CharacterCard card;
  final CardKind kind;
  final String apiBase;
  final String Function(int) mediaPosterUrl;
  final VoidCallback onEdit;
  final VoidCallback onDelete;
  final VoidCallback onManageExamples;
  final void Function(MediaAsset) onOpenExample;

  const _CardTile({
    required this.card,
    required this.kind,
    required this.apiBase,
    required this.mediaPosterUrl,
    required this.onEdit,
    required this.onDelete,
    required this.onManageExamples,
    required this.onOpenExample,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        // 点卡片本体 = **编辑**（用户 2026-10-24："点击画风卡 卡片的行为是编辑，
        // 不是修改示例图片"）。示例缩略图与「管理示例」按钮有自己的点击区域，
        // 点它们仍然是看示例 / 管理示例。
        onTap: onEdit,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 12, 8, 10),
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
                      MenuOption(
                        value: 'edit',
                        icon: Icons.edit_outlined,
                        label: '编辑',
                      ),
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
                      onPressed: () => controller.isOpen
                          ? controller.close()
                          : controller.open(),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              _VersionBlock(
                label: '正面提示词',
                value: card.prompt,
                emptyText: '还没有正面提示词',
                maxLines: kind == CardKind.character ? 8 : 5,
              ),
              if (card.negative != null) ...[
                const SizedBox(height: 6),
                Text(
                  '负面提示词是 ${card.negative}',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
              ],
              if (!card.hasPrompt) ...[
                const SizedBox(height: 8),
                Row(
                  children: [
                    Icon(
                      Icons.info_outline,
                      size: 14,
                      color: theme.colorScheme.tertiary,
                    ),
                    const SizedBox(width: 4),
                    Expanded(
                      child: Text(
                        '正面提示词还空着 —— 可以让 AI 写一版，或点这张卡自己补。',
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.tertiary,
                        ),
                      ),
                    ),
                  ],
                ),
              ],
              const SizedBox(height: 8),
              Row(
                children: [
                  Icon(
                    Icons.photo_library_outlined,
                    size: 15,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 5),
                  Text(
                    card.examples.isEmpty
                        ? '示例产物'
                        : '示例产物 ${card.examples.length}',
                    style: theme.textTheme.labelMedium?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                  const Spacer(),
                  TextButton.icon(
                    onPressed: onManageExamples,
                    icon: const Icon(
                      Icons.add_photo_alternate_outlined,
                      size: 17,
                    ),
                    label: Text(card.examples.isEmpty ? '添加示例' : '管理示例'),
                  ),
                ],
              ),
              if (card.examples.isNotEmpty)
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    for (final media in card.examples.take(6))
                      _ExampleThumb(
                        media: media,
                        apiBase: apiBase,
                        posterUrl: mediaPosterUrl(media.id),
                        onTap: () => onOpenExample(media),
                      ),
                    if (card.examples.length > 6)
                      Container(
                        width: 64,
                        height: 64,
                        alignment: Alignment.center,
                        decoration: BoxDecoration(
                          color: theme.colorScheme.surfaceContainerHighest,
                          borderRadius: BorderRadius.circular(8),
                        ),
                        child: Text(
                          '+${card.examples.length - 6}',
                          style: theme.textTheme.labelMedium,
                        ),
                      ),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class _VersionBlock extends StatelessWidget {
  final String label;
  final String? value;
  final String emptyText;
  final int maxLines;

  const _VersionBlock({
    required this.label,
    required this.value,
    required this.emptyText,
    required this.maxLines,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (value == null) {
      return Text(
        emptyText,
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 1),
        Text(
          value!,
          style: theme.textTheme.bodySmall,
          maxLines: maxLines,
          overflow: TextOverflow.ellipsis,
        ),
      ],
    );
  }
}

/// 卡片上的示例缩略图（图片走 /thumb、视频走 /poster、音频给占位）。
class _ExampleThumb extends StatelessWidget {
  final MediaAsset media;
  final String apiBase;
  final String posterUrl;
  final VoidCallback onTap;

  const _ExampleThumb({
    required this.media,
    required this.apiBase,
    required this.posterUrl,
    required this.onTap,
  });

  String _absolute(String path) =>
      path.startsWith('http') ? path : '$apiBase$path';

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final imageUrl = switch (media.kind) {
      MediaKind.image => media.thumbUrl ?? media.fileUrl,
      MediaKind.video => posterUrl,
      MediaKind.audio => null,
    };
    final child = imageUrl == null
        ? Container(
            color: theme.colorScheme.surfaceContainerHighest,
            alignment: Alignment.center,
            child: Icon(
              Icons.graphic_eq,
              color: theme.colorScheme.onSurfaceVariant,
            ),
          )
        : Image.network(
            _absolute(imageUrl),
            fit: BoxFit.cover,
            cacheWidth: 128,
            filterQuality: FilterQuality.low,
            errorBuilder: (_, _, _) => Container(
              color: theme.colorScheme.surfaceContainerHighest,
              alignment: Alignment.center,
              child: Icon(
                Icons.image_not_supported_outlined,
                size: 18,
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          );
    return SizedBox(
      width: 64,
      height: 64,
      child: Material(
        borderRadius: BorderRadius.circular(8),
        clipBehavior: Clip.antiAlias,
        color: theme.colorScheme.surfaceContainerHighest,
        child: InkWell(
          onTap: onTap,
          child: Stack(
            fit: StackFit.expand,
            children: [
              NsfwBlur(
                nsfw: media.nsfw,
                revealKey: 'media:${media.id}',
                borderRadius: BorderRadius.circular(8),
                child: child,
              ),
              if (media.kind != MediaKind.image)
                Center(
                  child: Icon(
                    media.kind == MediaKind.video
                        ? Icons.play_arrow_rounded
                        : Icons.graphic_eq,
                    color: Colors.white,
                    size: 22,
                    shadows: const [
                      Shadow(blurRadius: 3, color: Colors.black87),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 新建 / 编辑一张卡：名称 + 正面提示词（自然语言）+ 负面提示词。
///
/// 字段与后端 `CardInput` 一一对应；**保存是全量替换**（后端 `PUT` 语义），
/// 所以在这里清空一个字段就真的清掉它。
class _CardEditorDialog extends StatefulWidget {
  final CardKind kind;
  final AiApiClient api;
  final CharacterCard? card;

  const _CardEditorDialog({required this.kind, required this.api, this.card});

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
      setState(() => _error = '${widget.kind.nameLabel}不能为空');
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
      if (widget.kind == CardKind.character) {
        if (widget.card == null) {
          await widget.api.createCharacterCard(payload);
        } else {
          await widget.api.updateCharacterCard(widget.card!.id, payload);
        }
      } else {
        if (widget.card == null) {
          await widget.api.createStyleCard(payload);
        } else {
          await widget.api.updateStyleCard(widget.card!.id, payload);
        }
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

  Widget _field(String key, String label, {String? hint, int maxLines = 1}) =>
      Padding(
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
    final kind = widget.kind;
    return AlertDialog(
      title: Text(widget.card == null ? kind.newLabel : kind.editTitle),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _field(
                'name',
                '${kind.nameLabel} *',
                hint: kind == CardKind.character
                    ? '用户叫的名字，例如 shinku'
                    : '用户叫的画风名，例如 赛博国风 / semi-thick_anime',
              ),
              _field(
                'prompt',
                '正面提示词',
                hint: kind == CardKind.character
                    ? '从头到脚写全，包括脸、眼睛、头发、皮肤、身体、服装分层、鞋袜、饰品、体态……600+ token'
                    : '写成一段话，按媒介 → 上色风格 → 打光习惯 → 材质（面部 / 头发 / 衣物 / 其他物体）→ 画面维度与后期。100~200 token 起，复杂画风按需写长',
                maxLines: kind == CardKind.character ? 12 : 8,
              ),
              _field(
                'negative',
                '负面提示词',
                hint: kind == CardKind.character
                    ? '这个角色常踩的那些词'
                    : '会破坏这套画风的词（写实、3d、照片、脏色……）',
                maxLines: 3,
              ),
              Text(
                '${kind.editorHint}\n'
                '示例产物在卡片上点「添加示例」管理（最多 24 条，新建的卡保存后就能加）。',
                style: Theme.of(context).textTheme.bodySmall,
              ),
              if (_error != null) ...[
                const SizedBox(height: 10),
                Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
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
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Text('保存'),
        ),
      ],
    );
  }
}

/// 示例产物选择器：从画廊最近产物里勾选，返回**最终**的 mediaId 顺序。
class _ExamplePickerDialog extends StatefulWidget {
  final AiApiClient api;
  final List<MediaAsset> initial;

  const _ExamplePickerDialog({required this.api, required this.initial});

  @override
  State<_ExamplePickerDialog> createState() => _ExamplePickerDialogState();
}

class _ExamplePickerDialogState extends State<_ExamplePickerDialog> {
  final _search = TextEditingController();
  final List<int> _order = [];
  List<MediaAsset> _items = const [];
  bool _loading = true;
  String? _error;

  /// 与后端 `cards::MAX_EXAMPLES` 同值；后端是权威校验，这里只负责提前提示。
  static const _maxExamples = 24;

  @override
  void initState() {
    super.initState();
    for (final m in widget.initial) {
      _order.add(m.id);
    }
    _load();
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final items = await widget.api.searchMediaForExamples(
        query: _search.text,
      );
      if (!mounted) return;
      // 默认视图把卡片当前的示例也放进来（它们可能早于最近 60 条，否则用户
      // 打开"管理示例"会看不到、也就没法取消勾选）。
      final seen = <int>{};
      final merged = <MediaAsset>[
        if (_search.text.trim().isEmpty) ...widget.initial,
        ...items,
      ].where((m) => seen.add(m.id)).toList();
      setState(() {
        _items = merged;
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

  void _toggle(MediaAsset media) {
    setState(() {
      if (!_order.remove(media.id)) {
        if (_order.length >= _maxExamples) {
          _error = '示例产物最多 $_maxExamples 条';
          return;
        }
        _order.add(media.id);
      }
      _error = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: const Text('选择示例产物'),
      content: SizedBox(
        width: 720,
        height: 560,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _search,
                    decoration: const InputDecoration(
                      isDense: true,
                      prefixIcon: Icon(Icons.search, size: 18),
                      hintText: '搜产物标题 / 文件名 / 提示词',
                      border: OutlineInputBorder(),
                    ),
                    onSubmitted: (_) => _load(),
                  ),
                ),
                const SizedBox(width: 8),
                FilledButton(onPressed: _load, child: const Text('搜索')),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              '已选 ${_order.length} 条（最多 $_maxExamples 条）。示例是给用户和 AI 看的样本，不会改动产物本身。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const SizedBox(height: 8),
            if (_error != null) ...[
              Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
              const SizedBox(height: 6),
            ],
            Expanded(child: _grid()),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: _loading
              ? null
              : () => Navigator.pop(context, List<int>.from(_order)),
          child: Text('保存（${_order.length}）'),
        ),
      ],
    );
  }

  Widget _grid() {
    if (_loading) return const Center(child: CircularProgressIndicator());
    if (_error != null && _items.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(_error!, textAlign: TextAlign.center),
            const SizedBox(height: 8),
            OutlinedButton(onPressed: _load, child: const Text('重试')),
          ],
        ),
      );
    }
    if (_items.isEmpty) {
      return Center(
        child: Text(
          '画廊里还没有产物。\n先去生成 / 上传几张，再回来把它们加成示例。',
          textAlign: TextAlign.center,
          style: Theme.of(context).textTheme.bodySmall,
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        final columns = (constraints.maxWidth / 116).floor().clamp(2, 6);
        return GridView.builder(
          itemCount: _items.length,
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: columns,
            crossAxisSpacing: 8,
            mainAxisSpacing: 8,
            childAspectRatio: 1,
          ),
          itemBuilder: (context, i) {
            final media = _items[i];
            final selected = _order.contains(media.id);
            return _PickerTile(
              media: media,
              apiBase: widget.api.baseUrl,
              posterUrl: widget.api.mediaPosterUrl(media.id),
              selected: selected,
              onTap: () => _toggle(media),
            );
          },
        );
      },
    );
  }
}

class _PickerTile extends StatelessWidget {
  final MediaAsset media;
  final String apiBase;
  final String posterUrl;
  final bool selected;
  final VoidCallback onTap;

  const _PickerTile({
    required this.media,
    required this.apiBase,
    required this.posterUrl,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final imageUrl = switch (media.kind) {
      MediaKind.image => media.thumbUrl ?? media.fileUrl,
      MediaKind.video => posterUrl,
      MediaKind.audio => null,
    };
    return Material(
      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
      borderRadius: BorderRadius.circular(10),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (imageUrl == null)
              Center(
                child: Icon(
                  Icons.graphic_eq,
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              )
            else
              NsfwBlur(
                nsfw: media.nsfw,
                revealKey: 'media:${media.id}',
                borderRadius: BorderRadius.circular(10),
                child: Image.network(
                  imageUrl.startsWith('http') ? imageUrl : '$apiBase$imageUrl',
                  fit: BoxFit.cover,
                  cacheWidth: 256,
                  filterQuality: FilterQuality.low,
                  errorBuilder: (_, _, _) => Center(
                    child: Icon(
                      Icons.image_not_supported_outlined,
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
              ),
            if (media.kind != MediaKind.image)
              Center(
                child: Icon(
                  media.kind == MediaKind.video
                      ? Icons.play_arrow_rounded
                      : Icons.graphic_eq,
                  color: Colors.white,
                  size: 26,
                  shadows: const [Shadow(blurRadius: 3, color: Colors.black87)],
                ),
              ),
            Positioned(
              left: 4,
              right: 4,
              bottom: 4,
              child: Text(
                media.title.isEmpty ? media.originalName : media.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 10.5,
                  fontWeight: FontWeight.w600,
                  shadows: [Shadow(blurRadius: 3, color: Colors.black87)],
                ),
              ),
            ),
            Positioned(
              top: 4,
              right: 4,
              child: Container(
                width: 20,
                height: 20,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: selected ? theme.colorScheme.primary : Colors.black45,
                  border: Border.all(color: Colors.white70, width: 1.2),
                ),
                child: selected
                    ? const Icon(Icons.check, size: 13, color: Colors.white)
                    : null,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
