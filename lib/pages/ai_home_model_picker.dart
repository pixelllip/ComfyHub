part of 'ai_home_page.dart';

// ---------------------------------------------------------------------------
//  模型选择（聊天框内直接切换，AIK-002）
// ---------------------------------------------------------------------------

/// 模型选择（聊天框内直接切换，AIK-002）。
///
/// 内置目录有 **69 个模型**，所以这里是"搜索框 + 懒构建列表"的对话框，
/// 不是把每个模型都塞进 PopupMenu 的 `for (...)` —— 那样每次打开都要
/// 一次性建 69 行（每行还带徽标），弹出明显卡顿。
class _ModelPicker extends StatelessWidget {
  final AiWorkspaceStore store;

  /// 文字密度（见 [`_ComposerLabel`]）：`icon` 档才把模型名收进 tooltip。
  final _ComposerLabel density;

  const _ModelPicker({required this.store, this.density = _ComposerLabel.full});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final model = store.selectedModel;
    final name = model?.displayName ?? '未选择模型';
    final modalities = (model?.modalities ?? const <AiModality>[])
        .map((m) => m.label)
        .join(' / ');
    final button = InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: () => showDialog<void>(
        context: context,
        builder: (_) => _ModelPickerDialog(store: store),
      ),
      child: Container(
        // 三档都保证 ≥44 高（用户："按钮太小了比较难点"）
        constraints: const BoxConstraints(minHeight: 44),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
        decoration: BoxDecoration(
          border: Border.all(
            color: model == null ? theme.colorScheme.error : theme.dividerColor,
          ),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              model == null ? Icons.error_outline : Icons.memory,
              size: 18,
              color: model == null ? theme.colorScheme.error : null,
            ),
            if (density != _ComposerLabel.icon) ...[
              const SizedBox(width: 6),
              // 模型名可能很长（内置目录里就有 "…-preview-2026-01" 这种）：
              // 输入区底部这一行还要塞下附件 / 权限 / 思考强度 / token 汇总 / 发送，
              // 所以这里**必须**能压缩，不能把整行撑溢出。
              Flexible(
                child: Text(
                  name,
                  style: theme.textTheme.labelMedium,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              // 能力徽标只在**全称**档给：短标签档的空间要留给模型名本身
              // （用户要的是"显示出有用的关键信息"，模型名比"文本/图片"那两枚小徽标更关键）。
              if (density == _ComposerLabel.full) ...[
                const SizedBox(width: 6),
                for (final m in model?.modalities ?? const <AiModality>[])
                  Padding(
                    padding: const EdgeInsets.only(right: 4),
                    child: _Badge(m.label),
                  ),
              ],
              const SizedBox(width: 2),
              Icon(
                Icons.arrow_drop_down,
                size: 18,
                color: theme.colorScheme.outline,
              ),
            ],
          ],
        ),
      ),
    );
    // 全称档名字与徽标都在按钮上，不用 tooltip（空 message 会画出空气泡）
    if (density == _ComposerLabel.full) return button;
    // 另外两档补一句 Provider —— 换过 Provider 之后光看模型名认不出它是谁家的
    final provider = store.selectedProvider?.displayName;
    return Tooltip(
      message:
          '当前模型是 $name${modalities.isEmpty ? '' : '（$modalities）'}'
          '${provider == null ? '' : '，来自 $provider'}'
          '。点一下换一个',
      child: button,
    );
  }
}

/// 可搜索的模型选择对话框 —— **按 Provider 分组**（懒构建：只建屏幕上看得见的那几行）。
///
/// 分组是用户 2026-10 提的要求，原话两条：
///  1. 「来自不同 provider 的模型要套在一个 padding 里面区分，**padding 外标注模型来源**」
///     —— 所以布局是：**来源那一行在容器外面**，模型行**套在同一个 padding 里面**；
///  2. 顺带把「换 Provider」这件事补上：以前这个对话框只有**当前那一条**的模型，
///     界面上又没有别的地方能换，于是刚建好的 Provider 在工作台里永远选不上。
class _ModelPickerDialog extends StatefulWidget {
  final AiWorkspaceStore store;
  const _ModelPickerDialog({required this.store});

  @override
  State<_ModelPickerDialog> createState() => _ModelPickerDialogState();
}

/// 选择器里的**一组**：一条 Provider + 它名下（命中搜索词的）那些模型。
class _ProviderGroup {
  final AiProvider provider;
  final List<AiModel> all;
  final List<AiModel> hits;
  final String? issue;

  const _ProviderGroup(this.provider, this.all, this.hits, this.issue);
}

class _ModelPickerDialogState extends State<_ModelPickerDialog> {
  final _search = TextEditingController();
  String _query = '';

  @override
  void initState() {
    super.initState();
    // 打开选择器 = "我要挑一个" = 清单必须是最新的（总纲 §27.2 第 12 条）：
    // 手机上 Provider 的真源是本机库，刚在「AI 模型与凭据」页建好的那一条，
    // 不重读一次就不会出现在这里（以前要切走再切回来才看得到）。
    // 模型目录同理（用户 bug「模型声明要重启客户端才生效」）：改的是模型的能力声明，
    // provider 行不动，refreshProviders 的"清单变没变"判不出来 —— 目录要单独重读。
    unawaited(widget.store.refreshProviders());
    unawaited(widget.store.refreshModels());
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  /// 按 id / 显示名过滤（大小写不敏感）。
  bool _matches(AiModel m) {
    final q = _query.trim().toLowerCase();
    if (q.isEmpty) return true;
    return m.id.toLowerCase().contains(q) ||
        m.displayName.toLowerCase().contains(q);
  }

  /// 全部 Provider 的目录，**每条一组**；一条都搜不到的整组不列（否则满屏空标题）。
  ///
  /// 目录还没拉到（[AiWorkspaceStore.catalogByProvider] 是空的）时，**当前那条**仍用
  /// `store.models` 顶上 —— 那样至少不会在"正在读"的那一瞬间把已有的模型清空。
  List<_ProviderGroup> get _groups {
    final store = widget.store;
    final out = <_ProviderGroup>[];
    for (final p in store.providers) {
      final list =
          store.catalogByProvider[p.id] ??
          (p.id == store.selectedProvider?.id
              ? store.models
              : const <AiModel>[]);
      final hits = list.where(_matches).toList();
      if (hits.isEmpty) continue;
      out.add(_ProviderGroup(p, list, hits, store.catalogIssues[p.id]));
    }
    return out;
  }

  /// **来源标注**（在 padding **外面**的那一行）。
  Widget _groupHeader(_ProviderGroup g, ThemeData theme, bool current) {
    final p = g.provider;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 10, 16, 4),
      child: Row(
        children: [
          Icon(
            current ? Icons.radio_button_checked : Icons.cloud_outlined,
            size: 14,
            color: current
                ? theme.colorScheme.primary
                : theme.colorScheme.outline,
          ),
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              '来自 ${p.displayName}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelMedium?.copyWith(
                color: current
                    ? theme.colorScheme.primary
                    : theme.colorScheme.onSurface,
                fontWeight: current ? FontWeight.w600 : null,
              ),
            ),
          ),
          const SizedBox(width: 6),
          Text(
            p.id,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const Spacer(),
          if (!p.enabled) const _Badge('已停用'),
          if (!p.credential.configured) const _Badge('未配置密钥'),
          _Badge('${g.all.length} 个'),
        ],
      ),
    );
  }

  /// 一行模型（**在 padding 里面**）。
  Widget _modelTile(
    AiProvider p,
    AiModel m,
    ThemeData theme,
    String? selectedId,
  ) {
    return ListTile(
      dense: true,
      selected: p.id == widget.store.selectedProvider?.id && m.id == selectedId,
      title: Text(m.displayName, maxLines: 1, overflow: TextOverflow.ellipsis),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${m.modalities.map((e) => e.label).join(' / ')}'
            '${m.tools ? ' · 工具' : ''} · ${m.capabilitySourceLabel}'
            ' · ${m.id}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 2),
          Wrap(
            spacing: 4,
            runSpacing: 2,
            children: [
              for (final modality in m.modalities) _Badge(modality.label),
              if (m.tools) const _Badge('工具'),
              if (m.reasoning) const _Badge('思考'),
            ],
          ),
        ],
      ),
      // 唯一的选择入口：跨 Provider 时先把 Provider 切过去（见 store.selectModelFrom）
      onTap: () {
        unawaited(widget.store.selectModelFrom(p, m));
        Navigator.of(context).pop();
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      // 目录是打开这一刻现拉的（`refreshProviders`）—— 不听通知的话，
      // 拉回来之后这个对话框还是一张空列表。
      listenable: widget.store,
      builder: (context, _) => _buildDialog(context),
    );
  }

  Widget _buildDialog(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    final groups = _groups;
    final total = store.catalogByProvider.values.fold<int>(
      0,
      (a, b) => a + b.length,
    );
    final shown = groups.fold<int>(0, (a, g) => a + g.hits.length);
    final selectedId = store.selectedModel?.id;
    // 有 Provider 却一个目录都没读到 = 正在拉（`refreshProviders` 是异步的）
    final loading =
        store.providers.isNotEmpty &&
        store.catalogByProvider.isEmpty &&
        store.catalogIssues.isEmpty;

    return Dialog(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560, maxHeight: 520),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
              child: TextField(
                controller: _search,
                autofocus: true,
                onChanged: (v) => setState(() => _query = v),
                decoration: const InputDecoration(
                  hintText: '搜索模型 id 或名称',
                  prefixIcon: Icon(Icons.search, size: 18),
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: Row(
                children: [
                  Text(
                    loading ? '正在读取各 Provider 的模型目录…' : '共 $total 个模型',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                  const Spacer(),
                  Text(
                    '显示 $shown 个',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ],
              ),
            ),
            const Divider(height: 12),
            Expanded(child: _body(theme, groups, loading, selectedId)),
          ],
        ),
      ),
    );
  }

  /// 三种状态各说各的话（用户原话纪律：看不见的现象只能靠猜，所以都要**如实说**）。
  Widget _body(
    ThemeData theme,
    List<_ProviderGroup> groups,
    bool loading,
    String? selectedId,
  ) {
    if (widget.store.providers.isEmpty) {
      return _hint(theme, '还没有配置 Provider。\n去「设置 → AI 模型与凭据」建一条，并给它配好模型目录。');
    }
    if (loading) {
      return const Center(
        child: SizedBox(
          width: 18,
          height: 18,
          child: CircularProgressIndicator(strokeWidth: 2),
        ),
      );
    }
    if (groups.isEmpty) {
      return _hint(
        theme,
        _query.trim().isEmpty
            ? '这些 Provider 下一条模型都没有。\n去「设置 → AI 模型与凭据」用「获取可用模型」或「手动添加」补上。'
            : '没有匹配的模型',
      );
    }
    // 每组两个 sliver：**标题在 padding 外，模型列表整体套在一个 padding 里**。
    // 组内是 `SliverList`（懒构建）——69 个模型一次也只建视口附近那几行。
    return CustomScrollView(
      slivers: [
        for (final g in groups) ...[
          SliverToBoxAdapter(
            child: _groupHeader(
              g,
              theme,
              g.provider.id == widget.store.selectedProvider?.id,
            ),
          ),
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 10),
            sliver: SliverList(
              delegate: SliverChildBuilderDelegate(
                (context, i) =>
                    _modelTile(g.provider, g.hits[i], theme, selectedId),
                childCount: g.hits.length,
              ),
            ),
          ),
        ],
        const SliverToBoxAdapter(child: SizedBox(height: 8)),
      ],
    );
  }

  Widget _hint(ThemeData theme, String text) => Center(
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: Text(
        text,
        textAlign: TextAlign.center,
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      ),
    ),
  );
}
