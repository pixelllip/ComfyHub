part of 'ai_home_page.dart';

class _ContextPanel extends StatefulWidget {
  final AiWorkspaceStore store;
  const _ContextPanel({required this.store});

  @override
  State<_ContextPanel> createState() => _ContextPanelState();
}

class _ContextPanelState extends State<_ContextPanel> {
  CaptureStatus? _status;
  bool _loading = false;
  String? _error;

  /// Skills 列表默认**折叠**：数量较多时右侧栏无法容纳，
  /// 展开后给搜索框，找 skill 靠搜索而不是往下翻。
  bool _skillsOpen = false;
  final _skillSearch = TextEditingController();

  /// 折叠状态下的搜索结果：名称、描述、whenToUse 里任一命中就算。
  List<AiSkill> _filteredSkills(AiWorkspaceStore store) {
    final q = _skillSearch.text.trim().toLowerCase();
    if (q.isEmpty) return store.skills;
    return store.skills
        .where(
          (s) =>
              s.name.toLowerCase().contains(q) ||
              (s.description).toLowerCase().contains(q) ||
              (s.whenToUse ?? '').toLowerCase().contains(q),
        )
        .toList();
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      // **冷启动先等后端**（2026-10-12）：这一栏一显示就打三处
      //（ComfyUI 状态 / 实时进度 / 待审批），而那一刻 `.run\api.json` 还没写出来、
      // `baseUrl` 还是 8080 兜底值 —— 三个请求必然失败，其中 `/api/capture/status`
      // 那个失败还会**留在界面上**（只有手点刷新才走）。等真有结论再打，失败也才如实。
      await widget.store.waitForBackend();
      if (!mounted) return;
      _refresh();
      // 实时进度：右侧栏显示时开始轮询，
      // "AI 提交的任务 / 在 ComfyUI 中手动提交的任务"均可查看进度。
      widget.store.startWatchingComfy();
      // 跨端待审批（M5）：手机排上来的提交在桌面上要看得见、点得动。
      // 与实时进度**同一条纪律**（只在这一栏显示时轮询，dispose 里停），
      // 外加一条：**手机上不轮询也不画那张卡** —— 审批一律归桌面。
      if (!widget.store.isLocal) widget.store.startWatchingPending();
    });
  }

  @override
  void dispose() {
    widget.store.stopWatchingComfy();
    widget.store.stopWatchingPending();
    _skillSearch.dispose();
    super.dispose();
  }

  Future<void> _refresh() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final base = context.read<SettingsStore>().baseUrl;
      final status = await ApiClient(base).captureStatus();
      if (mounted) setState(() => _status = status);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 这里 watch 一次：右侧栏要跟着"实时进度"（AI 提交的任务）自动刷新
    final store = context.watch<AiWorkspaceStore>();
    final model = store.selectedModel;
    final visibleSkills = _filteredSkills(store);

    // 右侧栏每一块**各自一个卡片容器**。此前只有"实时进度"是 Card，其余几块为
    // 直接铺在背景上的文字与列表，无法区分各块归属与边界。
    //
    // 不使用 `Card`+`Column` 整体包裹：skills 数量可达数十个，**必须懒构建**
    // （`ListView(children: [...])` 会一次性全部构建）。因此 Skills 卡使用
    // `DecoratedSliver` + `SliverMainAxisGroup`：卡片背景覆盖"标题 + 懒构建条目"，
    // 行数再多也只构建可见部分。外观与 [_PanelCard] 共用 [panelCardDecoration]。
    return CustomScrollView(
      slivers: [
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 10),
          sliver: SliverList.list(
            children: [
              // 跨端待审批（M5）：**有人等你点批准**放在最上面（比进度更需要动手），
              // 手机排上来的提交就在这里露面。只有桌面画它：手机上审批归桌面，
              // 给一个点了没用的按钮不如不给。
              // 它自己负责"没有待办时整块不占位"（含下面那道间距），所以这里不另加 SizedBox。
              if (!store.isLocal) _PendingApprovals(store: store),
              // 实时进度：AI 提交的任务 / ComfyUI 队列，
              // 提交后无需切换到 ComfyUI 即可查看运行进度。
              _ComfyProgress(store: store),
              const SizedBox(height: 10),
              _PanelCard(
                key: const Key('ai-panel-card-model'),
                title: '模型能力',
                children: [
                  if (model == null)
                    Text('未选择模型', style: theme.textTheme.bodySmall)
                  else ...[
                    Text(model.displayName, style: theme.textTheme.bodyMedium),
                    const SizedBox(height: 4),
                    Wrap(
                      spacing: 4,
                      runSpacing: 4,
                      children: [
                        for (final m in AiModality.values)
                          _CapabilityChip(
                            label: m.label,
                            // 未声明的能力一律显示为不支持（AIH-011：不猜）
                            supported: model.supports(m),
                          ),
                        _CapabilityChip(label: '工具', supported: model.tools),
                      ],
                    ),
                    const SizedBox(height: 6),
                    Text(
                      '来源是 ${model.capabilitySourceLabel}'
                      '${model.contextWindow != null ? ' · 上下文 ${model.contextWindow}' : ''}',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ],
                ],
              ),
              const SizedBox(height: 10),
              // 「ComfyUI」右边就是刷新按钮：它刷新的是 ComfyUI 状态，
              // 此前位于"上下文"标题右侧，显示为刷新整个面板。
              _PanelCard(
                key: const Key('ai-panel-card-comfy'),
                title: 'ComfyUI',
                trailing: IconButton(
                  tooltip: '刷新 ComfyUI 状态',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(
                    minWidth: 28,
                    minHeight: 28,
                  ),
                  padding: EdgeInsets.zero,
                  onPressed: _loading ? null : _refresh,
                  icon: const Icon(Icons.refresh, size: 16),
                ),
                children: [
                  if (_loading)
                    const LinearProgressIndicator(minHeight: 2)
                  else if (_error != null)
                    Text('查询失败，$_error', style: theme.textTheme.bodySmall)
                  else if (_status == null)
                    Text('暂无数据', style: theme.textTheme.bodySmall)
                  else ...[
                    _KV('连通性', _status!.comfyReachable ? '可达' : '不可达'),
                    _KV('运行中', '${_status!.queueRunning}'),
                    _KV('等待中', '${_status!.queuePending}'),
                    _KV(
                      '已捕获',
                      '${_status!.capturedRuns} 次 / ${_status!.capturedMedia} 个产物',
                    ),
                    if (_status!.lastError != null)
                      Text(
                        _status!.lastError!,
                        style: theme.textTheme.labelSmall,
                      ),
                  ],
                ],
              ),
              const SizedBox(height: 10),
            ],
          ),
        ),
        // Skills：名字就是真源，这里只是后端的只读视图（M5）。
        // **投放口**才是"装 skill"的方式：把文件夹（或 .md）拷进下面这个目录，
        // 应用启动时会自动登记（拷进来的文件没写 frontmatter 也会被补上）。
        // 此处不提供「从 DSH 导入」按钮。
        //
        // **列表默认折叠**（skill 数量较多时，
        // 右侧栏会被它们占满），展开后才给搜索框 —— 找某个 skill 靠搜索，不靠翻。
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 10),
          sliver: DecoratedSliver(
            key: const Key('ai-panel-card-skills'),
            decoration: panelCardDecoration(theme),
            sliver: SliverMainAxisGroup(
              slivers: [
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(10, 10, 10, 0),
                  sliver: SliverToBoxAdapter(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            InkWell(
                              onTap: () =>
                                  setState(() => _skillsOpen = !_skillsOpen),
                              child: Row(
                                children: [
                                  Icon(
                                    _skillsOpen
                                        ? Icons.expand_less
                                        : Icons.expand_more,
                                    size: 16,
                                    color: theme.colorScheme.outline,
                                  ),
                                  Text(
                                    'Skills',
                                    style: theme.textTheme.labelLarge,
                                  ),
                                ],
                              ),
                            ),
                            const SizedBox(width: 4),
                            Text(
                              '${store.skills.length}',
                              style: theme.textTheme.labelSmall?.copyWith(
                                color: theme.colorScheme.outline,
                              ),
                            ),
                            const Spacer(),
                            IconButton(
                              tooltip: '重新扫描投放口（自动登记新拷进来的 skill）',
                              visualDensity: VisualDensity.compact,
                              constraints: const BoxConstraints(
                                minWidth: 28,
                                minHeight: 28,
                              ),
                              padding: EdgeInsets.zero,
                              onPressed: store.skillsBusy
                                  ? null
                                  : () => _rescanSkills(),
                              icon: const Icon(Icons.refresh, size: 16),
                            ),
                          ],
                        ),
                        if (_skillsOpen) ...[
                          const SizedBox(height: 4),
                          TextField(
                            controller: _skillSearch,
                            decoration: InputDecoration(
                              isDense: true,
                              hintText: '搜索 skill（名称 / 说明）',
                              prefixIcon: const Icon(Icons.search, size: 16),
                              suffixIcon: _skillSearch.text.isEmpty
                                  ? null
                                  : IconButton(
                                      tooltip: '清空搜索',
                                      icon: const Icon(Icons.close, size: 16),
                                      onPressed: () =>
                                          setState(() => _skillSearch.clear()),
                                    ),
                              border: const OutlineInputBorder(),
                            ),
                            onChanged: (_) => setState(() {}),
                          ),
                        ],
                        _SkillDropIn(
                          store: store,
                          onOpen: () => _openSkillsFolder(),
                          onCopy: () => _copySkillsPath(),
                        ),
                        if (store.skillsBusy)
                          const LinearProgressIndicator(minHeight: 2),
                        if (store.skillsError != null)
                          Text(
                            'Skills 加载失败，${store.skillsError}',
                            style: theme.textTheme.bodySmall,
                          ),
                        // 回落取证：下面列的是**手机本机**那几份，不是桌面上的全集。
                        // 缺少该说明会被理解为电脑上的 skill 已删除。
                        if (store.skillsLocalOnly)
                          Text(
                            '桌面连不上，这里只列了手机本机的 skill。',
                            style: theme.textTheme.bodySmall,
                          ),
                      ],
                    ),
                  ),
                ),
                if (!_skillsOpen)
                  // 折叠时不列条目，仅保留一行提示（展开后可见）
                  SliverPadding(
                    padding: const EdgeInsets.fromLTRB(10, 6, 10, 10),
                    sliver: SliverToBoxAdapter(
                      child: Text(
                        store.skills.isEmpty
                            ? '还没有 Skill。把 skill 文件夹（或一个 .md）拷进上面的目录再点刷新，'
                                  '也可以让 AI 用 register_skill 注册。'
                            : '列表已折叠（${store.skills.length} 个）。点上面的「Skills」展开并搜索。',
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.outline,
                        ),
                      ),
                    ),
                  )
                else if (visibleSkills.isEmpty)
                  SliverPadding(
                    padding: const EdgeInsets.fromLTRB(10, 6, 10, 10),
                    sliver: SliverToBoxAdapter(
                      child: Text(
                        store.skills.isEmpty
                            ? '还没有 Skill。把 skill 文件夹（或一个 .md）拷进上面的目录再点刷新，'
                                  '也可以让 AI 用 register_skill 注册。'
                            : '没有匹配「${_skillSearch.text.trim()}」的 Skill。',
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.outline,
                        ),
                      ),
                    ),
                  )
                else
                  SliverPadding(
                    padding: const EdgeInsets.fromLTRB(4, 2, 4, 8),
                    sliver: SliverList.builder(
                      itemCount: visibleSkills.length,
                      itemBuilder: (context, i) => _SkillRow(
                        skill: visibleSkills[i],
                        onToggle: () => _toggleSkill(visibleSkills[i]),
                        onDelete: () => _deleteSkill(visibleSkills[i]),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
        // 长期记忆（M6）：每次对话都会带上；AI 也能用 remember 追加一条
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 16),
          sliver: SliverToBoxAdapter(
            child: _PanelCard(
              key: const Key('ai-panel-card-memory'),
              children: [
                _MemoryPanel(store: store, onEdit: () => _editMemory()),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Future<void> _toggleSkill(AiSkill skill) async {
    final result = await widget.store.setSkillEnabled(skill.name, !skill.enabled);
    if (!mounted) return;
    _snack(result.message, error: !result.ok);
  }

  Future<void> _rescanSkills() async {    final result = await widget.store.rescanSkills();
    if (!mounted) return;
    if (result.ok && widget.store.skillsError == null) {
      _snack(result.message);
    } else if (widget.store.skillsError != null) {
      _snack('Skills 加载失败，${widget.store.skillsError}', error: true);
    } else {
      _snack(result.message, error: true);
    }
  }

  /// 在资源管理器里打开投放口（失败时返回路径）。
  Future<void> _openSkillsFolder() async {
    final path = widget.store.skillRoots?.userRoot ?? '';
    if (path.isEmpty) {
      _snack('还没有拿到 skills 目录位置，点一下刷新试试', error: true);
      return;
    }
    final ok = await _openFolder(path);
    if (!ok && mounted) {
      _snack('打不开文件管理器，请手动打开 $path');
    }
  }

  /// 把投放口路径放进剪贴板（打不开资源管理器时的兜底）。
  Future<void> _copySkillsPath() async {
    final path = widget.store.skillRoots?.userRoot ?? '';
    if (path.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: path));
    if (mounted) _snack('已复制 skills 目录到 $path');
  }

  /// 长期记忆编辑器：整篇可改（真源是 memory.md，人是可以手改的）。
  Future<void> _editMemory() async {
    final saved = await showDialog<String>(
      context: context,
      builder: (_) => _MemoryEditorDialog(store: widget.store),
    );
    if (!mounted || saved == null) return;
    final result = await widget.store.saveMemory(saved);
    if (!mounted) return;
    _snack(result.message, error: !result.ok);
  }

  Future<void> _deleteSkill(AiSkill skill) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('删除 Skill「${skill.name}」？'),
        content: const Text('会从磁盘上删掉它的正文文件，模型之后不能再加载。内置 Skill 不能删除。'),
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
    final result = await widget.store.deleteSkill(skill.name);
    if (!mounted) return;
    _snack(result.message, error: !result.ok);
  }

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          content: Text(message),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error
              ? Theme.of(context).colorScheme.errorContainer
              : null,
        ),
      );
  }
}

/// 右侧栏里的一行 Skill：名称 + 一行描述 + 来源徽标 + 警告 + 删除（仅自定义来源）。
class _SkillRow extends StatelessWidget {
  final AiSkill skill;
  final VoidCallback onDelete;
  final VoidCallback onToggle;

  const _SkillRow({
    required this.skill,
    required this.onDelete,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ListTile(
      dense: true,
      contentPadding: EdgeInsets.zero,
      title: Row(
        children: [
          Flexible(
            child: Text(
              skill.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelMedium?.copyWith(
                // 不合法的 skill 不进系统提示，界面上标红（不静默忽略）
                color: skill.validationError != null
                    ? theme.colorScheme.error
                    : null,
              ),
            ),
          ),
          const SizedBox(width: 4),
          _Badge(skill.sourceLabel),
          if (!skill.enabled) ...[const SizedBox(width: 4), _Badge('停用')],
        ],
      ),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            skill.oneLine.isEmpty ? '（没有描述）' : skill.oneLine,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall,
          ),
          if (skill.hasWarning)
            Tooltip(
              message: skill.warningText,
              child: Row(
                children: [
                  Icon(
                    Icons.warning_amber_rounded,
                    size: 13,
                    color: theme.colorScheme.error,
                  ),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Text(
                      '有问题，${skill.validationError != null ? '格式不合法' : '有冲突'}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            tooltip: skill.enabled ? '停用 Skill' : '启用 Skill',
            icon: Icon(
              skill.enabled
                  ? Icons.visibility_outlined
                  : Icons.visibility_off_outlined,
              size: 16,
            ),
            visualDensity: VisualDensity.compact,
            constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
            padding: EdgeInsets.zero,
            onPressed: onToggle,
          ),
          if (skill.isBuiltin)
            Tooltip(
              message: '内置 Skill 只读（可在磁盘上改）',
              child: Icon(
                Icons.lock_outline,
                size: 14,
                color: theme.colorScheme.outline,
              ),
            )
          else
            IconButton(
              tooltip: '删除 Skill',
              icon: const Icon(Icons.delete_outline, size: 16),
              visualDensity: VisualDensity.compact,
              constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
              padding: EdgeInsets.zero,
              onPressed: onDelete,
            ),
        ],
      ),
    );
  }
}

class _KV extends StatelessWidget {
  final String k;
  final String v;
  const _KV(this.k, this.v);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            k,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(width: 8),
          // 值可能很长（例如"已捕获"那一行的次数与产物数）。这里给弹性宽度并右对齐：
          // 放得下时仍贴右侧，放不下时换行显示完整内容 —— 侧栏只有 260 宽，
          // 用 `Spacer` + 不可换行的 Text 会直接画出卡片边缘。
          Expanded(
            child: Text(
              v,
              textAlign: TextAlign.right,
              style: theme.textTheme.labelSmall,
            ),
          ),
        ],
      ),
    );
  }
}

class _CapabilityChip extends StatelessWidget {
  final String label;
  final bool supported;
  const _CapabilityChip({required this.label, required this.supported});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = supported
        ? theme.colorScheme.primary
        : theme.colorScheme.outline;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: color.withValues(alpha: 0.6)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(supported ? Icons.check : Icons.close, size: 11, color: color),
          const SizedBox(width: 3),
          Text(
            label,
            style: theme.textTheme.labelSmall?.copyWith(color: color),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  Skills 投放口 + 长期记忆（右侧栏）
// ---------------------------------------------------------------------------

/// 显示 skills 投放口的绝对路径，并提供「打开文件夹 / 复制路径」。
///
/// **不提供「从 DSH 导入」按钮**，改为一个可复制内容进去的目录。
/// 路径由后端算（两种运行布局都对），界面只负责显示 —— 别在前端拼路径。
class _SkillDropIn extends StatelessWidget {
  final AiWorkspaceStore store;
  final VoidCallback onOpen;
  final VoidCallback onCopy;

  const _SkillDropIn({
    required this.store,
    required this.onOpen,
    required this.onCopy,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final path = store.skillRoots?.userRoot ?? '';
    if (path.isEmpty) return const SizedBox.shrink();
    final style = TextButton.styleFrom(
      padding: const EdgeInsets.symmetric(horizontal: 6),
      minimumSize: const Size(0, 28),
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 2),
        Text(
          '把 skill 拷进下面这个目录就算装好，启动或刷新时会自动登记。',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        const SizedBox(height: 2),
        Tooltip(
          message: path,
          child: Text(
            path,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(fontFamily: 'Consolas'),
          ),
        ),
        Row(
          children: [
            TextButton.icon(
              onPressed: store.skillsBusy ? null : onOpen,
              icon: const Icon(Icons.folder_open, size: 14),
              label: const Text('打开文件夹'),
              style: style,
            ),
            TextButton.icon(
              onPressed: store.skillsBusy ? null : onCopy,
              icon: const Icon(Icons.copy, size: 14),
              label: const Text('复制路径'),
              style: style,
            ),
          ],
        ),
      ],
    );
  }
}

/// 右侧栏的「长期记忆」：条数 + 说明 + 编辑入口。
///
/// **不在此处显示记忆正文**：记忆内容属于偏好与上下文，而右侧栏
/// **常驻屏幕** —— 显示正文会使内容对屏幕前的所有人可见。
/// 要看内容就点右边的编辑按钮进弹窗（`_MemoryEditorDialog`）。
class _MemoryPanel extends StatelessWidget {
  final AiWorkspaceStore store;
  final VoidCallback onEdit;

  const _MemoryPanel({required this.store, required this.onEdit});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final memory = store.memory;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('长期记忆', style: theme.textTheme.labelLarge),
            const SizedBox(width: 4),
            Text(
              '${memory.entryCount} 条',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            const Spacer(),
            if (store.memoryBusy)
              const SizedBox(
                width: 12,
                height: 12,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            else
              IconButton(
                tooltip: '查看 / 编辑长期记忆',
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                padding: EdgeInsets.zero,
                onPressed: onEdit,
                icon: const Icon(Icons.edit_note, size: 16),
              ),
          ],
        ),
        Text(
          '每次对话都会带上它，AI 也能用 remember 追加一条。点右上角的图标查看 / 编辑内容。',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        if (store.memoryError != null)
          Text(
            '长期记忆不可用，${store.memoryError}',
            style: theme.textTheme.bodySmall,
          ),
      ],
    );
  }
}

/// 长期记忆编辑器：**搜索 + 单条 / 批量删除** + 加一条 + 整篇编辑。
///
/// 真源仍然是 `memory.md`（直接修改该文件同样生效），因此没有另外实现存储 ——
/// 列表视图只是"一行一条"的入口。搜索与删除为必需功能：
/// 他的顾虑不是存不下，而是"条数太多，想查找、改动、删除会比较困难"。
///
/// 下标语义：列表按 `memory.entries` 的顺序（0 起）编号，与后端的行序**同源** ——
/// 所以勾选之后把下标原样发给后端就行，不需要把内容传回去比对。
class _MemoryEditorDialog extends StatefulWidget {
  final AiWorkspaceStore store;
  const _MemoryEditorDialog({required this.store});

  @override
  State<_MemoryEditorDialog> createState() => _MemoryEditorDialogState();
}

class _MemoryEditorDialogState extends State<_MemoryEditorDialog> {
  /// 整篇编辑用的文本框（只在「整篇编辑」模式下可见 / 可保存）
  late final TextEditingController _text = TextEditingController(
    text: widget.store.memory.content,
  );
  final TextEditingController _entry = TextEditingController();
  final TextEditingController _search = TextEditingController();
  bool _busy = false;

  /// 勾选中的条目下标（指向 `memory.entries` 的**原始**顺序，不是过滤后的）
  final Set<int> _selected = {};

  /// 想一次改很多条 / 粘贴一大段时用整篇编辑；平时用列表视图。
  bool _rawMode = false;

  @override
  void dispose() {
    _text.dispose();
    _entry.dispose();
    _search.dispose();
    super.dispose();
  }

  /// 记忆列表 + 关键词过滤：返回 (原始下标, 正文)。
  List<(int, String)> get _visible {
    final all = widget.store.memory.entries;
    final q = _search.text.trim().toLowerCase();
    final out = <(int, String)>[];
    for (var i = 0; i < all.length; i++) {
      if (q.isEmpty || all[i].toLowerCase().contains(q)) out.add((i, all[i]));
    }
    // 搜索时将已有勾选收拢到"仍可见的条目"：否则"删除选中"会删除
    // 当前不可见的条目。
    return out;
  }

  Future<void> _add() async {
    final entry = _entry.text.trim();
    if (entry.isEmpty) return;
    setState(() => _busy = true);
    final result = await widget.store.addMemory(entry);
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (result.ok) {
        _text.text = widget.store.memory.content;
        _entry.clear();
      }
    });
    if (!result.ok) {
      ScaffoldMessenger.maybeOf(context)
          ?.showSnackBar(SnackBar(content: Text(result.message)));
    }
  }

  Future<void> _deleteSelected() async {
    final indices = _selected.toList()..sort();
    if (indices.isEmpty) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('删除这 ${indices.length} 条记忆？'),
        content: const Text('删掉之后就找不回来了。'),
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
    setState(() => _busy = true);
    final result = await widget.store.deleteMemoryEntries(indices);
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (result.ok) {
        _selected.clear();
        _text.text = widget.store.memory.content;
      }
    });
    ScaffoldMessenger.maybeOf(context)
        ?.showSnackBar(SnackBar(content: Text(result.message)));
  }

  Future<void> _deleteOne(int index) async {
    setState(() => _busy = true);
    final result = await widget.store.deleteMemoryEntries([index]);
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (result.ok) {
        // 删掉一行之后，后面所有下标都要往前挪一位
        final shifted = {
          for (final i in _selected)
            if (i < index) i else if (i > index) i - 1,
        };
        _selected
          ..clear()
          ..addAll(shifted);
        _text.text = widget.store.memory.content;
      }
    });
    if (!result.ok) {
      ScaffoldMessenger.maybeOf(context)
          ?.showSnackBar(SnackBar(content: Text(result.message)));
    }
  }

  Future<void> _clear() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('清空长期记忆？'),
        content: const Text('会把 memory.md 里的内容全部删掉，不能撤销。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('清空'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    setState(() => _busy = true);
    final result = await widget.store.clearMemory();
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (result.ok) {
        _text.clear();
        _selected.clear();
      }
    });
    ScaffoldMessenger.maybeOf(context)
        ?.showSnackBar(SnackBar(content: Text(result.message)));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final memory = widget.store.memory;
    final path = memory.path;
    return AlertDialog(
      title: const Text('长期记忆'),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      path.isEmpty
                          ? '一行一条，AI 的 remember 工具会追加到这里。'
                          : '真源是 $path（也可以直接改那个文件）。',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
                  // 条数占用（界面上需显示剩余条数）
                  Text(
                    '${memory.entryCount} / ${memory.maxEntries} 条',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: memory.entryCount >= memory.maxEntries
                          ? theme.colorScheme.error
                          : theme.colorScheme.outline,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              if (_rawMode) ...[
                TextField(
                  controller: _text,
                  minLines: 8,
                  maxLines: 14,
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'Consolas',
                  ),
                  decoration: const InputDecoration(
                    border: OutlineInputBorder(),
                    hintText: '- 用户偏好 4:3 画幅\n- 出图统一用 Anima 模型',
                    isDense: true,
                  ),
                ),
              ] else ...[
                TextField(
                  controller: _search,
                  onChanged: (_) => setState(() {}),
                  decoration: InputDecoration(
                    prefixIcon: const Icon(Icons.search, size: 18),
                    suffixIcon: _search.text.isEmpty
                        ? null
                        : IconButton(
                            tooltip: '清空搜索',
                            icon: const Icon(Icons.close, size: 16),
                            onPressed: () => setState(() => _search.clear()),
                          ),
                    hintText: '搜索记忆（按关键词过滤）',
                    border: const OutlineInputBorder(),
                    isDense: true,
                  ),
                ),
                const SizedBox(height: 8),
                _MemoryEntryList(
                  entries: _visible,
                  selected: _selected,
                  busy: _busy,
                  emptyHint: widget.store.memory.entryCount == 0
                      ? '还没有记忆。AI 在你说出跨对话的偏好时会用 remember 记一条。'
                      : '没有匹配「${_search.text.trim()}」的记忆。',
                  onToggle: (i) => setState(() {
                    if (!_selected.remove(i)) _selected.add(i);
                  }),
                  onDelete: _busy ? null : _deleteOne,
                ),
              ],
              const SizedBox(height: 10),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: _entry,
                      decoration: const InputDecoration(
                        labelText: '再加一条',
                        hintText: '例如 交付一律 16:9、带字幕',
                        border: OutlineInputBorder(),
                        isDense: true,
                      ),
                      onSubmitted: (_) => _add(),
                    ),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton(
                    onPressed: _busy ? null : _add,
                    child: const Text('添加'),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton.icon(
          onPressed: _busy
              ? null
              : () => setState(() {
                  _rawMode = !_rawMode;
                  if (_rawMode) _text.text = widget.store.memory.content;
                }),
          icon: Icon(
            _rawMode ? Icons.list_alt_outlined : Icons.edit_note,
            size: 18,
          ),
          label: Text(_rawMode ? '列表视图' : '整篇编辑'),
        ),
        if (!_rawMode && _selected.isNotEmpty)
          TextButton.icon(
            onPressed: _busy ? null : _deleteSelected,
            icon: const Icon(Icons.delete_outline, size: 18),
            label: Text('删除选中（${_selected.length}）'),
            style: TextButton.styleFrom(
              foregroundColor: theme.colorScheme.error,
            ),
          ),
        TextButton(onPressed: _busy ? null : _clear, child: const Text('清空')),
        if (_rawMode) ...[
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: _busy ? null : () => Navigator.pop(context, _text.text),
            child: const Text('保存'),
          ),
        ] else
          FilledButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('关闭'),
          ),
      ],
    );
  }
}

/// 长期记忆的条目列表（搜索过滤后的结果 + 勾选 + 单条删除）。
///
/// 用 `ListView.builder` 懒构建：记忆上限 100 条，一次全建出来没必要
/// （AGENTS.md §6：长列表一律懒构建，别用 `children: [for (...) ...]`）。
class _MemoryEntryList extends StatelessWidget {
  final List<(int, String)> entries;
  final Set<int> selected;
  final bool busy;
  final String emptyHint;
  final void Function(int index) onToggle;
  final void Function(int index)? onDelete;

  const _MemoryEntryList({
    required this.entries,
    required this.selected,
    required this.busy,
    required this.emptyHint,
    required this.onToggle,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (entries.isEmpty) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 24),
        child: Text(emptyHint, style: theme.textTheme.bodySmall),
      );
    }
    return SizedBox(
      height: 260,
      child: ListView.builder(
        itemCount: entries.length,
        itemBuilder: (context, i) {
          final (index, text) = entries[i];
          return ListTile(
            dense: true,
            contentPadding: EdgeInsets.zero,
            visualDensity: VisualDensity.compact,
            leading: Checkbox(
              value: selected.contains(index),
              onChanged: busy ? null : (_) => onToggle(index),
            ),
            title: Text(text, style: theme.textTheme.bodySmall),
            onTap: busy ? null : () => onToggle(index),
            trailing: IconButton(
              tooltip: '删掉这一条',
              icon: const Icon(Icons.delete_outline, size: 16),
              onPressed: onDelete == null ? null : () => onDelete!(index),
            ),
          );
        },
      ),
    );
  }
}

/// 在系统文件管理器里打开一个目录。
///
/// 走 `url_launcher` 的 `file:` 协议（Windows 上落到 ShellExecute）；**打不开就返回 false**,
/// 由调用方显示路径 —— 不得显示为已打开。
Future<bool> _openFolder(String path) async {
  final base = Uri.file(path, windows: true);
  for (final uri in [base, Uri.parse('${base.toString()}/')]) {
    try {
      if (await launchUrl(uri, mode: LaunchMode.externalApplication)) {
        return true;
      }
    } catch (_) {
      // 试下一个形式
    }
  }
  return false;
}
