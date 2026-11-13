part of 'ai_home_page.dart';

// ---------------------------------------------------------------------------
//  提问卡
// ---------------------------------------------------------------------------

/// 一张提问卡的**历史记录形态**：AI 的提问与对应的回答。
///
/// 与 [_ToolCallCard] 刻意分开画：
///  - 工具卡是"我在干什么"（参数 + 结果预览，读的多、点的少）；
///  - 提问卡是"我需要你告诉我一件事"（问题 + 可点的选项）。
///
/// **不在此处放输入框**：AI 提问期间，输入区整块由
/// [_AskComposer] 接管，输入位置只有底部一个；卡片内再放一个会导致输入位置不明确。
/// 因此这里是"问题 + 选项按钮"，需要自行输入时点击最后一行进入底部输入框。
///
/// 回答之后卡片**不消失**，而是切换为"已回答"—— 历史中可辨认这一轮的
/// 输入信息（与工具卡一样，都属于该回复的上下文）。
class _AskCard extends StatefulWidget {
  /// **整张卡**：同一次 `ask_user_question` 问的所有题（至少一条，按题号排）。
  ///
  /// 此前此处只有一条；只渲染第一条会使其余问题在界面上
  /// 消失（否则会误认为只问了一件事，而模型在等待 N 个答案）。
  final List<AiQuestion> questions;

  /// 后端根地址（裁图 / 选项图的相对路径补它）。
  final String apiBase;

  final Future<bool> Function(
    String questionId, {
    int? optionIndex,
    String? text,
  })
  onAnswer;
  final Future<bool> Function(String questionId) onSkip;

  /// "去下面那个输入框里写"：把焦点交给输入区被接管后的那个文本框。
  final VoidCallback onFocusAnswer;

  const _AskCard({
    super.key,
    required this.questions,
    this.apiBase = '',
    required this.onAnswer,
    required this.onSkip,
    required this.onFocusAnswer,
  });

  /// 第一条：单题形态与标题都读它。
  AiQuestion get question => questions.first;

  @override
  State<_AskCard> createState() => _AskCardState();
}

class _AskCardState extends State<_AskCard> {
  bool _busy = false;

  /// 已手动展开 —— 之后**不再**随状态自动折回。
  ///
  /// "等回答 → 已回答"会让卡片从"可点选项"变为"记录答案"；若每次都自动决定
  /// 展开还是收起，他刚点开看清楚、答案一到就被折回去，很难受。
  bool _expandedByUser = false;

  /// 已手动折起 —— 已回答时**不再**自动展开。
  bool _collapsedByUser = false;

  /// 这一帧该展开吗。
  ///
  /// 折叠是**默认**（气泡中的卡与被接管的输入区会把整组选项
  /// 渲染两遍）。回答后默认展开为"问题 + 答案"两行，即历史中需要保留的信息。
  bool get _expanded {
    if (_expandedByUser) return true;
    if (_collapsedByUser) return false;
    return widget.question.answered;
  }

  void _toggle() => setState(() {
    final next = !_expanded;
    _expandedByUser = next;
    _collapsedByUser = !next;
  });

  Future<void> _send({int? optionIndex, String? text}) async {
    if (_busy) return;
    setState(() => _busy = true);
    final ok = await widget.onAnswer(
      widget.question.questionId,
      optionIndex: optionIndex,
      text: text,
    );
    if (!mounted) return;
    setState(() => _busy = false);
    if (!ok) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('这条提问已经结束了（超时或本次回答已停止），回答没有被采纳。')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final q = widget.question;
    // 整张卡都答完了才算"已回答"（一卡多题时逐题答，只答一题还写着已回答会误导）
    final answered = widget.questions.every((x) => x.answered);
    final color = answered
        ? theme.colorScheme.primary
        : theme.colorScheme.tertiary;
    // 一卡多题：气泡里这张卡只当**记录**（题目 + 答案），作答入口是底部那块。
    final paged = widget.questions.length > 1;

    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: color.withValues(alpha: 0.55)),
        color: theme.colorScheme.surface,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 头部整行可点 = 折叠 / 展开。刻意**不**用 InkWell 包整张卡：
          // 展开时选项行自己就是按钮，套在一起会互相抢点击。
          InkWell(
            onTap: _toggle,
            borderRadius: BorderRadius.circular(8),
            child: Row(
              children: [
                Icon(
                  answered
                      ? Icons.question_answer_outlined
                      : Icons.help_outline,
                  size: 15,
                  color: color,
                ),
                const SizedBox(width: 6),
                // 标题用满"扣掉状态徽标与展开键"之后的整行宽度：
                // 标题与 `Spacer` 同为 flex 1 时，剩余空间按 1:1 分给两者，
                // 长标题只能用到一半就被省略号截断。
                Expanded(
                  child: Text(
                    q.header?.isNotEmpty == true
                        ? q.header!
                        : (paged
                              ? 'AI 想问 ${widget.questions.length} 件事'
                              : 'AI 想确认一件事'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelMedium?.copyWith(color: color),
                  ),
                ),
                const SizedBox(width: 6),
                // 状态徽标：**等你回答**与**已回答**一眼分得开。
                // 等回答时它顺便当"去下面作答"的指标（真正能点的选项在输入区里）。
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 6,
                    vertical: 1,
                  ),
                  decoration: BoxDecoration(
                    color: color.withValues(alpha: 0.15),
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: Text(
                    answered ? '已回答' : '等你回答 ↓',
                    style: theme.textTheme.labelSmall?.copyWith(color: color),
                  ),
                ),
                const SizedBox(width: 2),
                Icon(
                  _expanded ? Icons.expand_less : Icons.expand_more,
                  size: 16,
                  color: theme.colorScheme.outline,
                ),
              ],
            ),
          ),
          const SizedBox(height: 6),
          // 折叠态 = 图二那种"一条记录"：头部 + 问题（+ 答案）就够，
          // **不再把选项画一遍** —— 提问期间能点的那份在底部被接管的输入区里。
          // 一卡多题时**逐题**列出来（题号 + 文字，答过的把答案跟在后面）：
          // 折起后也不能让某一题消失，否则无法知道模型问了 N 件事。
          for (var i = 0; i < widget.questions.length; i++) ...[
            if (i > 0) const SizedBox(height: 6),
            if (paged) ...[
              Text(
                '第 ${i + 1} 题',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 2),
            ],
            // 裁图只在**展开**时画：一卡 5 题时各贴一张图会把气泡撑成一整屏
            // 。等待回答时查看底部当前题的图即可；
            // 答完之后卡片默认展开，历史里再连着答案一起看。
            _AskQuestionHead(
              question: widget.questions[i].question,
              imageUrl: _expanded && widget.questions[i].imageUrl != null
                  ? absoluteBackendUrl(
                      widget.apiBase,
                      widget.questions[i].imageUrl!,
                    )
                  : null,
              imageKey: Key('ask-question-image-$i'),
            ),
            if (widget.questions[i].answered) ...[
              const SizedBox(height: 2),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    Icons.subdirectory_arrow_right,
                    size: 15,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Text(
                      (widget.questions[i].answer ?? '').isEmpty
                          ? '（已跳过，没有回答）'
                          : widget.questions[i].answer!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
                ],
              ),
            ],
          ],
          if (_expanded && paged) ...[
            const SizedBox(height: 6),
            // 多题卡不在气泡里再画一遍选项：作答入口只有底部那一块（一张卡分页），
            // 两处都放会导致点击位置不明确。
            Text(
              '在下面的答题卡里按「第 N/M 题」逐题作答，也可以逐题跳过。',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ] else if (_expanded) ...[
            // 展开态（只放**慢答案**：选项按钮 + 跳过）。
            // 打字那条路永远在底部输入区，这里不重复放输入框。
            for (var i = 0; i < q.options.length; i++) ...[
              const SizedBox(height: 6),
              _AskOptionRow(
                index: i,
                label: q.options[i],
                imageUrl: q.optionImageAt(i) == null
                    ? null
                    : absoluteBackendUrl(widget.apiBase, q.optionImageAt(i)!),
                selected: q.optionIndex == i,
                enabled: !_busy && !answered,
                onTap: () => _send(optionIndex: i),
              ),
            ],
            const SizedBox(height: 6),
            // --- 最后一行：固定留给"自己决定" ---
            // 末尾选项固定为"由自己输入答案"。
            _AskOptionRow(
              index: q.options.length,
              label: '自己输入答案',
              hint: answered ? '已经在下方输入框里回答过了' : '在下方输入框里写下你的答案',
              selected: false,
              enabled: !_busy && !answered,
              trailing: Icon(
                Icons.keyboard_alt_outlined,
                size: 15,
                color: theme.colorScheme.outline,
              ),
              onTap: widget.onFocusAnswer,
            ),
            if (!answered) ...[
              const SizedBox(height: 2),
              Align(
                alignment: Alignment.centerRight,
                child: TextButton(
                  // 跳过 = 如实告知模型"未回答"，模型基于已有信息继续
                  onPressed: _busy ? null : () => widget.onSkip(q.questionId),
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    minimumSize: const Size(0, 28),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  child: const Text('跳过这个问题'),
                ),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  结尾提问卡（`ask_follow_up`，非阻塞的变种）
// ---------------------------------------------------------------------------

/// 回复末尾的**分页提问卡**：问题贴在这一轮最后，提交后把选择拼成一条用户消息发出。
///
/// 与 [`_AskCard`]（阻塞式 `ask_user_question`）的区别：
///  · 这里是**作答入口本身**（气泡里那张卡直接点选），不接管底部输入框；
///  · 提交 = 记录答案 + **发出一条用户消息**（新一轮），不依赖模型原地等待；
///  · 每题都可以不选，最后一页的「提交」始终可点（没选的按"没有选择"发回去）。
class _AskFollowUpCard extends StatefulWidget {
  /// 整张卡（同一 `groupId` 的所有题，按题号排）。
  final List<AiQuestion> questions;

  /// 后端根地址（题目 / 选项图是相对路径）。
  final String apiBase;

  final AiWorkspaceStore store;

  /// 模型写的"提交后我会……"（可空）。
  final String? submitText;

  /// 本轮还在生成 / 正在发送时禁用（提交要在这一轮结束之后）。
  final bool enabled;

  /// 这张卡是否已经被**后来的用户消息**顶掉。
  ///
  /// 判据由 `_MessageBubble` 按会话消息顺序算：这张卡所在的那条助手消息之后
  /// 又出现了用户消息（无论用户是直接打字发的，还是点了这张卡提交的），
  /// 这张卡就只是历史记录 —— 默认收起，不占地方。
  final bool superseded;

  const _AskFollowUpCard({
    super.key,
    required this.questions,
    required this.apiBase,
    required this.store,
    this.submitText,
    this.enabled = true,
    this.superseded = false,
  });

  @override
  State<_AskFollowUpCard> createState() => _AskFollowUpCardState();
}

class _AskFollowUpCardState extends State<_AskFollowUpCard> {
  int _page = 0;
  bool _busy = false;

  /// 是否收起（只保留标题行）。**默认展开**；这张卡已经被后来的用户消息顶掉时
  /// 默认收起 —— 用户已经往下问了，旧提问不该继续占着版面。
  ///
  /// 用户手动点箭头展开 / 收起之后以手动状态为准：自动收起只在
  /// `superseded` **由假变真**的那一次发生（`didUpdateWidget`）。
  bool _collapsed = false;

  /// 回答记录卡是否展开（默认收起，展示"当时有哪些选项"时展开）。
  bool _recordOpen = false;

  /// 单选选了第几个 / 多选勾了哪几个 / 自己写的字（都按 questionId）。
  final Map<String, int> _single = {};
  final Map<String, Set<int>> _multi = {};
  final Map<String, TextEditingController> _texts = {};
  final TextEditingController _supplement = TextEditingController();

  @override
  void initState() {
    super.initState();
    _collapsed = widget.superseded;
  }

  @override
  void didUpdateWidget(covariant _AskFollowUpCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    // 用户发起了新提问（打字发的或点卡片提交的，两种情况都会多出一条用户消息）：
    // 这张旧提问自动收起。已经收起 / 用户刚手动展开过都不再重复处理 ——
    // 判据是"由假变真"这一次。
    if (!oldWidget.superseded && widget.superseded) {
      _collapsed = true;
    }
  }

  @override
  void dispose() {
    for (final controller in _texts.values) {
      controller.dispose();
    }
    _supplement.dispose();
    super.dispose();
  }

  TextEditingController _textOf(AiQuestion q) =>
      _texts.putIfAbsent(q.questionId, TextEditingController.new);

  int get _pageIndex => _page.clamp(0, widget.questions.length - 1);

  bool get _isLast => _pageIndex >= widget.questions.length - 1;

  bool get _allAnswered => widget.questions.every((q) => q.answered);

  /// 当前这份本地选择（每题一条；`null` = 这题之前已经答过）。
  List<AiFollowUpChoice?> _choices() => [
    for (final q in widget.questions)
      if (q.answered)
        null
      else
        AiFollowUpChoice(
          optionIndex: _single[q.questionId],
          optionIndexes: _multi[q.questionId] ?? const {},
          text: _texts[q.questionId]?.text ?? '',
        ),
  ];

  void _go(int delta) {
    final next = (_pageIndex + delta).clamp(0, widget.questions.length - 1);
    if (next != _pageIndex) setState(() => _page = next);
  }

  /// 跳过本题：清掉这一页的选择，翻到下一题（最后一页就停在原地）。
  void _skipPage() {
    final q = widget.questions[_pageIndex];
    _single.remove(q.questionId);
    _multi.remove(q.questionId);
    _textOf(q).clear();
    setState(() {
      if (!_isLast) _page = _pageIndex + 1;
    });
  }

  Future<void> _submit() async {
    if (_busy || !widget.enabled) return;
    setState(() => _busy = true);
    final r = await widget.store.submitFollowUp(
      widget.questions,
      _choices(),
      supplement: _supplement.text,
      submitText: widget.submitText ?? '',
    );
    if (!mounted) return;
    setState(() => _busy = false);
    if (!r.ok) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(r.message)));
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (_allAnswered) return _recordCard(theme);
    final q = widget.questions[_pageIndex];
    final total = widget.questions.length;
    final paged = total > 1;
    final answered = q.answered;
    final picked = _multi[q.questionId] ?? const <int>{};
    final single = _single[q.questionId];
    final controller = _textOf(q);
    final color = theme.colorScheme.tertiary;

    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: color.withValues(alpha: 0.55)),
        color: theme.colorScheme.surface,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Icon(Icons.help_outline, size: 15, color: color),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  q.header?.isNotEmpty == true
                      ? q.header!
                      : (paged ? 'AI 想确认 $total 件事' : 'AI 想确认一件事'),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelMedium?.copyWith(color: color),
                ),
              ),
              Text(
                paged ? '第 ${_pageIndex + 1}/$total 题' : '选完提交',
                key: const Key('ask-follow-up-page'),
                style: theme.textTheme.labelSmall?.copyWith(color: color),
              ),
              // 收起 / 展开：非阻塞提问贴在回复末尾，用户可能选择先不回答、
              // 直接往下问 —— 允许把整张卡收成一行标题。
              IconButton(
                key: const Key('ask-follow-up-toggle'),
                tooltip: _collapsed ? '展开提问' : '收起提问',
                onPressed: () => setState(() => _collapsed = !_collapsed),
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                padding: EdgeInsets.zero,
                icon: Icon(
                  _collapsed ? Icons.expand_more : Icons.expand_less,
                  size: 18,
                  color: color,
                ),
              ),
            ],
          ),
          if (!_collapsed) ...[
            const SizedBox(height: 6),
            _AskQuestionHead(
              question: q.question,
              imageUrl: q.imageUrl == null
                  ? null
                  : absoluteBackendUrl(widget.apiBase, q.imageUrl!),
              compact: MediaQuery.sizeOf(context).height < 700,
            ),
            if (answered)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  (q.answer ?? '').isEmpty
                      ? '（这一题已跳过，模型会当作"用户没有回答"）'
                      : '已作答，${q.answer}',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                ),
              )
            else
              ConstrainedBox(
                // 选项多时**只有选项区**内部滚动；下面的"自己输入答案"永远看得见。
                constraints: BoxConstraints(
                  maxHeight: MediaQuery.sizeOf(context).height < 700
                      ? 130
                      : 240,
                ),
                child: SingleChildScrollView(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      for (var i = 0; i < q.options.length; i++) ...[
                        const SizedBox(height: 6),
                        _AskOptionRow(
                          key: Key('ask-follow-up-option-$i'),
                          index: i,
                          label: q.options[i],
                          imageUrl: q.optionImageAt(i) == null
                              ? null
                              : absoluteBackendUrl(
                                  widget.apiBase,
                                  q.optionImageAt(i)!,
                                ),
                          selected: q.multiSelect
                              ? picked.contains(i)
                              : single == i,
                          enabled: !_busy && !answered,
                          trailing: q.multiSelect
                              ? Icon(
                                  picked.contains(i)
                                      ? Icons.check_box
                                      : Icons.check_box_outline_blank,
                                  size: 18,
                                  color: picked.contains(i)
                                      ? theme.colorScheme.primary
                                      : theme.colorScheme.outline,
                                )
                              : null,
                          onTap: () {
                            setState(() {
                              if (q.multiSelect) {
                                final set = _multi.putIfAbsent(
                                  q.questionId,
                                  () => <int>{},
                                );
                                set.contains(i) ? set.remove(i) : set.add(i);
                              } else {
                                _single[q.questionId] = i;
                              }
                              // 两条路只能走一条：选了项就把手打的字清掉
                              controller.clear();
                            });
                          },
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            // --- 自己输入答案（固定最后一行） ---
            if (!answered) ...[
              const SizedBox(height: 6),
              _customAnswerRow(theme, q, controller),
            ],
            // --- 最后一页：补充一句 + 模型写的"提交后我会……" ---
            if (_isLast && !answered) ...[
              const SizedBox(height: 8),
              TextField(
                key: const Key('ask-follow-up-supplement'),
                controller: _supplement,
                enabled: !_busy,
                minLines: 1,
                maxLines: 2,
                decoration: InputDecoration(
                  isDense: true,
                  border: const OutlineInputBorder(),
                  labelText: '还要交代什么（可选）',
                  hintText: '例如：给我具体的 YAML 配置',
                  hintStyle: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
              if ((widget.submitText ?? '').trim().isNotEmpty) ...[
                const SizedBox(height: 4),
                Text(
                  '提交后会把以上选择发出去，并附上：${widget.submitText!.trim()}',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ],
            ],
            const SizedBox(height: 4),
            Row(
              children: [
                if (paged)
                  TextButton(
                    key: const Key('ask-follow-up-prev'),
                    onPressed: (_busy || _pageIndex == 0)
                        ? null
                        : () => _go(-1),
                    style: TextButton.styleFrom(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      minimumSize: const Size(0, 36),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    child: const Text('上一题'),
                  ),
                TextButton(
                  key: const Key('ask-follow-up-skip'),
                  onPressed: _busy ? null : _skipPage,
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    minimumSize: const Size(0, 36),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  child: const Text('跳过本题'),
                ),
                const Spacer(),
                FilledButton(
                  key: const Key('ask-follow-up-submit'),
                  onPressed: (_busy || !widget.enabled)
                      ? null
                      : () {
                          if (_isLast) {
                            unawaited(_submit());
                          } else {
                            _go(1);
                          }
                        },
                  style: FilledButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 14),
                    minimumSize: const Size(0, 36),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  child: Text(_isLast ? '提交' : '下一题'),
                ),
              ],
            ),
          ], // if (!_collapsed)
        ],
      ),
    );
  }

  /// 已回答：收成一行标题，展开后**连同当时给的选项**一起回看。
  ///
  /// 默认收起：答完的结尾提问不该继续摊开占版面（用户 2026-11-13）。
  /// 展开时把每题当时的选项都列出来、并标出选中的那一个 ——
  /// 只回放答案文字的话，用户没法确认"当时到底还有哪些选项"。
  Widget _recordCard(ThemeData theme) {
    final color = theme.colorScheme.primary;
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: color.withValues(alpha: 0.55)),
        color: theme.colorScheme.surface,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Icon(Icons.question_answer_outlined, size: 15, color: color),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  'AI 问过 ${widget.questions.length} 件事',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelMedium?.copyWith(color: color),
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: Text(
                  '已回答',
                  style: theme.textTheme.labelSmall?.copyWith(color: color),
                ),
              ),
              IconButton(
                key: const Key('ask-follow-up-record-toggle'),
                tooltip: _recordOpen ? '收起选项' : '展开选项',
                onPressed: () => setState(() => _recordOpen = !_recordOpen),
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                padding: EdgeInsets.zero,
                icon: Icon(
                  _recordOpen ? Icons.expand_less : Icons.expand_more,
                  size: 18,
                  color: color,
                ),
              ),
            ],
          ),
          if (_recordOpen)
            for (var i = 0; i < widget.questions.length; i++) ...[
              const SizedBox(height: 8),
              Text(
                '第 ${i + 1} 题',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              const SizedBox(height: 2),
              Text(
                widget.questions[i].question,
                style: theme.textTheme.bodyMedium,
              ),
              // 当时的选项（只读）：选中的那个仍然用主色高亮。
              for (var j = 0; j < widget.questions[i].options.length; j++) ...[
                const SizedBox(height: 6),
                IgnorePointer(
                  child: _AskOptionRow(
                    key: Key('ask-follow-up-record-option-$i-$j'),
                    index: j,
                    label: widget.questions[i].options[j],
                    imageUrl: widget.questions[i].optionImageAt(j) == null
                        ? null
                        : absoluteBackendUrl(
                            widget.apiBase,
                            widget.questions[i].optionImageAt(j)!,
                          ),
                    selected: _wasChosen(widget.questions[i], j),
                    enabled: true,
                    onTap: () {},
                  ),
                ),
              ],
              const SizedBox(height: 6),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    Icons.subdirectory_arrow_right,
                    size: 15,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Text(
                      (widget.questions[i].answer ?? '').isEmpty
                          ? '（已跳过，没有回答）'
                          : widget.questions[i].answer!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
                ],
              ),
            ],
        ],
      ),
    );
  }

  /// 回看时判断某个选项当时是否被选中。
  ///
  /// 两条来源都认：结构化下标（`optionIndex`，本地作答时记的）与答案文字
  /// （后端回传的历史记录只有文字；多选是「A、B」拼接的）。
  bool _wasChosen(AiQuestion q, int index) {
    if (q.optionIndex == index) return true;
    final answer = (q.answer ?? '').trim();
    if (answer.isEmpty) return false;
    final label = q.options[index];
    if (q.multiSelect) {
      return answer.split('、').map((s) => s.trim()).contains(label);
    }
    return answer == label;
  }

  /// "自己输入答案"：序号接着选项走，输入框就在卡片里（不接管底部）。
  Widget _customAnswerRow(
    ThemeData theme,
    AiQuestion q,
    TextEditingController controller,
  ) {
    return Material(
      color: theme.colorScheme.surfaceContainerHighest,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(color: theme.dividerColor),
      ),
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              width: 20,
              height: 20,
              margin: const EdgeInsets.only(top: 6),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(
                '${q.options.length + 1}',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: TextField(
                key: Key('ask-follow-up-text-${q.questionId}'),
                controller: controller,
                enabled: !_busy,
                minLines: 1,
                maxLines: 3,
                onChanged: (_) => setState(() {
                  // 手打答案与勾选项互斥
                  _single.remove(q.questionId);
                  _multi.remove(q.questionId);
                }),
                decoration: InputDecoration(
                  isDense: true,
                  border: InputBorder.none,
                  hintText: '自己输入答案（可选）',
                  hintStyle: theme.textTheme.bodyMedium?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 提问卡 / 提问输入区里的一行选项。
///
/// 抽出来是为了两处（气泡里的卡片、底部被接管的输入区）**长得一样**：
/// 序号徽标 + 标签，点一下即答；最后一行固定是"自己输入答案"。
class _AskOptionRow extends StatelessWidget {
  final int index;
  final String label;
  final String? hint;

  /// 选项示意图（示例款式 / 从设定图裁的局部）；没有就是纯文字。
  final String? imageUrl;
  final bool selected;
  final bool enabled;
  final Widget? trailing;
  final VoidCallback onTap;

  const _AskOptionRow({
    super.key,
    required this.index,
    required this.label,
    this.hint,
    this.imageUrl,
    this.selected = false,
    this.enabled = true,
    this.trailing,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = selected
        ? theme.colorScheme.primary
        : theme.colorScheme.outline;
    return Opacity(
      opacity: enabled ? 1 : 0.5,
      child: Material(
        // 底色必须由 Material 给（用 Container 的 decoration 会盖掉水波纹，
        // Flutter 会直接抛断言，见 AGENTS.md 第 6 节）
        color: selected
            ? color.withValues(alpha: 0.10)
            : theme.colorScheme.surfaceContainerHighest,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
          side: BorderSide(
            color: selected ? color.withValues(alpha: 0.6) : theme.dividerColor,
          ),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: enabled ? onTap : null,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(8, 8, 10, 8),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: 20,
                  height: 20,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: theme.colorScheme.surfaceContainerHighest,
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: Text(
                    '${index + 1}',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      // 图片位于**选项文字前面**。
                      if (imageUrl != null) ...[
                        _AskOptionThumb(
                          key: Key('ask-option-image-$index'),
                          url: imageUrl!,
                        ),
                        const SizedBox(width: 8),
                      ],
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(label, style: theme.textTheme.bodyMedium),
                            if (hint != null)
                              Text(
                                hint!,
                                style: theme.textTheme.labelSmall?.copyWith(
                                  color: theme.colorScheme.outline,
                                ),
                              ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                if (trailing != null) ...[const SizedBox(width: 6), trailing!],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// 提问卡的**题干 + 配图**：问题在左（占大部宽度）、部位截图在右。
///
/// 图铺满整行时上下出现黑边，鼠标停留时「点击放大」会遮挡内容；
/// 改为"左侧问题、右侧截图"后，图只占一条窄栏，问题可完整阅读。
/// 窄窗口（< 560）放不下两栏，退回上下排。
class _AskQuestionHead extends StatelessWidget {
  final String question;

  /// 已补好 baseUrl 的图片地址；null = 这一题没有配图。
  final String? imageUrl;

  /// 矮窗口里再收一档（配图 110 高）。
  final bool compact;

  /// 配图那一块的 key（用例要能精确找到"当前题的图"）。
  final Key? imageKey;

  const _AskQuestionHead({
    required this.question,
    required this.imageUrl,
    this.compact = false,
    this.imageKey,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final text = Text(question, style: theme.textTheme.bodyMedium);
    final imageUrl = this.imageUrl;
    if (imageUrl == null) return text;
    return LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth < 560) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              text,
              const SizedBox(height: 5),
              _AskQuestionImage(
                key: imageKey,
                url: imageUrl,
                height: compact ? 120 : 160,
              ),
            ],
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(child: text),
            const SizedBox(width: 14),
            _AskQuestionImage(
              key: imageKey,
              url: imageUrl,
              width: compact ? 150 : 220,
              height: compact ? 110 : 150,
            ),
          ],
        );
      },
    );
  }
}

/// 提问卡里的**题目配图**：AI 从设定图上裁下来的那一块。
///
/// 点一下走应用内大图（与聊天里的附件同一条路）。**刻意不加 Tooltip**：
/// 鼠标停在图上时「点击放大」会遮挡细节；
/// 只在右下角放一个很小的放大镜图标。
class _AskQuestionImage extends StatelessWidget {
  final String url;

  /// 展示宽度 / 高度。宽度 `double.infinity` = 占满一行（窄屏上下排时用）。
  final double width;
  final double height;

  const _AskQuestionImage({
    super.key,
    required this.url,
    this.width = double.infinity,
    this.height = 180,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.surfaceContainerHighest,
      borderRadius: BorderRadius.circular(8),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: () => showImagePreview(context, url: url),
        child: SizedBox(
          width: width,
          height: height,
          child: Stack(
            fit: StackFit.expand,
            children: [
              Image.network(
                url,
                fit: BoxFit.contain,
                // 裁图是局部放大图：按 960 解码足够看清楚细节，又不至于把原图整张拉进内存
                cacheWidth: 960,
                filterQuality: FilterQuality.medium,
                errorBuilder: (_, _, _) => Center(
                  child: Icon(
                    Icons.broken_image_outlined,
                    size: 22,
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
              Positioned(
                right: 5,
                bottom: 5,
                child: IgnorePointer(
                  child: Container(
                    padding: const EdgeInsets.all(3),
                    decoration: BoxDecoration(
                      color: Colors.black.withValues(alpha: 0.5),
                      borderRadius: BorderRadius.circular(5),
                    ),
                    child: const Icon(
                      Icons.zoom_in,
                      size: 14,
                      color: Colors.white,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 选项前面的小示意图（示例款式 / 局部裁图）。
class _AskOptionThumb extends StatelessWidget {
  final String url;

  const _AskOptionThumb({super.key, required this.url});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ClipRRect(
      borderRadius: BorderRadius.circular(6),
      child: SizedBox(
        width: 56,
        height: 56,
        child: Image.network(
          url,
          fit: BoxFit.cover,
          cacheWidth: 168,
          filterQuality: FilterQuality.low,
          errorBuilder: (_, _, _) => Container(
            color: theme.colorScheme.surfaceContainerHighest,
            alignment: Alignment.center,
            child: Icon(
              Icons.image_not_supported_outlined,
              size: 18,
              color: theme.colorScheme.outline,
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  提问输入区（AI 提问时**接管**聊天下部的输入模块）
// ---------------------------------------------------------------------------

/// AI 提问期间**替换掉普通输入框**的那一块。
///
/// 参考 DeepSeek Harness 的做法（`dsh-client-ui-user-questions` 的 composer takeover）：
/// 提问时编辑器整块变成答题卡 —— 选项点一下就答，要自己写的写在最后那一行，
/// 底下给"跳过 / 停止"。它是**唯一的**作答入口，所以不会出现
/// "气泡里一个输入框、底下又一个输入框，不知道往哪儿写"。
///
/// 多问形态：一次 `ask_user_question` 可以问**最多 5 题**，
/// 界面是**一张卡分页**：
///  · 顶上一行写「第 N/M 题」；
///  · 底行是「上一题 / 跳过本题 / 下一题」，**最后一页**那个按钮变成「提交」；
///  · `multiSelect` 的题画复选框（选项上限 5），每题都保留"自己输入"；
///  · **答案可以为空**：整卡提交时未作答的题按"跳过"发送（= 告知模型"未回答"，
///    而非"未提问"）。需逐题答完后才收起卡片 —— 只答一题会使模型继续等待其余问题。
///
/// 与气泡里那张提问卡的分工：
///  · 卡片 = 记录（问过什么、答了什么，跟着这条回复走）；
///  · 这里 = 作答（现在就要做的决定，问完自动让位给普通输入框）。
class _AskComposer extends StatefulWidget {
  final AiWorkspaceStore store;

  /// 整张卡：同一组（同 `groupId`）的**所有**题，按题号排。至少一条。
  final List<AiQuestion> questions;

  /// 答完之后把这句当普通消息发出去（可选题外补充）；返回是否真的发出去了。
  final Future<void> Function(String text) onSubmit;

  const _AskComposer({
    required this.store,
    required this.questions,
    required this.onSubmit,
  });

  @override
  State<_AskComposer> createState() => _AskComposerState();
}

class _AskComposerState extends State<_AskComposer> {
  /// 每一题一个文本框（翻页来回切也不丢已打的字）。
  final Map<String, TextEditingController> _texts = {};

  /// 单选题选了第几个（按 questionId）。
  final Map<String, int> _single = {};

  /// 多选题勾了哪几个（按 questionId）。
  final Map<String, Set<int>> _multi = {};

  /// 当前第几页（0 基）。
  int _page = 0;
  bool _busy = false;

  /// **本地**这一轮已经处理过的题（跳过 / 提交过的）。
  ///
  /// 父层拿新状态重建本组件要到下一帧；`_skipCurrent` 里立刻问 `widget.questions`
  /// 会读到**还没更新**的那份（刚跳过的那题还写着"没答"），于是"下一道没作答的题"
  /// 又算回它自己 —— 实测就是这个：跳完停在原页不动。所以自己记一份。
  final Set<String> _done = {};

  @override
  void dispose() {
    for (final c in _texts.values) {
      c.dispose();
    }
    super.dispose();
  }

  TextEditingController _controllerOf(AiQuestion q) =>
      _texts.putIfAbsent(q.questionId, TextEditingController.new);

  /// 当前页（自动夹在当前题数里：跳过的题会让整卡变短，`_page` 可能越界）。
  int get _pageIndex => _page.clamp(0, widget.questions.length - 1);

  bool get _isLast => _pageIndex >= widget.questions.length - 1;

  /// 多选题的答案拼成一段文字发回去。
  ///
  /// 后端那条回答接口只收**一个** `optionIndex`（多选的 `optionIndexes` 还没开在 HTTP 上），
  /// 而模型收到的是回答文字 —— 因此多选拼接为「A、B」发送，
  /// 不为了一个结构化字段去改已经跑通的后端合同。
  String _joinedOptions(AiQuestion q, Set<int> picked) {
    final idx = picked.toList()..sort();
    return idx
        .where((i) => i >= 0 && i < q.options.length)
        .map((i) => q.options[i])
        .join('、');
  }

  /// 单问时点选项**当场作答**（保持 2026-09-21 定下的手感，也是老用例钉住的）。
  Future<void> _answerNow(
    AiQuestion q, {
    int? optionIndex,
    String? text,
  }) async {
    if (_busy) return;
    setState(() => _busy = true);
    final r = await widget.store.answerQuestion(
      q.questionId,
      optionIndex: optionIndex,
      text: text,
    );
    if (!mounted) return;
    setState(() => _busy = false);
    if (!r.ok) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(r.message)));
      return;
    }
    widget.store.pendingQuestionFocus.unfocus();
  }

  /// 跳过**当前这一题**（后端按"未回答"放行，模型基于已有信息继续）。
  Future<void> _skipCurrent() async {
    if (_busy) return;
    final q = widget.questions[_pageIndex];
    setState(() => _busy = true);
    final r = await widget.store.skipQuestion(q.questionId);
    if (!mounted) return;
    setState(() => _busy = false);
    if (!r.ok) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(r.message)));
      return;
    }
    // 跳到下一道**还没作答**的题；没有了就停在原地（整卡答完时这块会被收起）
    _done.add(q.questionId);
    final next = widget.questions.indexWhere(
      (x) => !x.answered && !_done.contains(x.questionId),
    );
    if (next >= 0) setState(() => _page = next);
  }

  /// 翻页。
  void _go(int delta) {
    final next = (_pageIndex + delta).clamp(0, widget.questions.length - 1);
    if (next != _pageIndex) setState(() => _page = next);
  }

  /// 提交整张卡：逐题把答案发回去；**没作答的题按"跳过"发**（答案可以为空）。
  ///
  /// 顺序发（不并发）：后端那几题的等待是同一组，逐条发才有"已答 / 已跳过"的确定顺序，
  /// 出了错也知道停在哪一题。
  Future<void> _submit() async {
    if (_busy) return;
    setState(() => _busy = true);
    final store = widget.store;
    String? failed;
    for (final q in widget.questions) {
      if (q.answered) continue; // 之前已经跳过 / 答过的题不再发一遍
      final text = (_texts[q.questionId]?.text ?? '').trim();
      final picked = _multi[q.questionId] ?? const <int>{};
      final single = _single[q.questionId];
      final bool ok;
      if (text.isNotEmpty) {
        ok = (await store.answerQuestion(q.questionId, text: text)).ok;
      } else if (q.multiSelect && picked.isNotEmpty) {
        ok = (await store.answerQuestion(
          q.questionId,
          text: _joinedOptions(q, picked),
        )).ok;
      } else if (single != null) {
        ok = (await store.answerQuestion(q.questionId, optionIndex: single)).ok;
      } else {
        // 该题未选择：按"跳过"发送 —— 模型会说明缺失信息，不显示为已回答
        ok = (await store.skipQuestion(q.questionId)).ok;
      }
      if (!ok) {
        failed ??= '有一条回答没有被采纳（超时或本次回答已停止）。';
      } else {
        _done.add(q.questionId);
      }
    }
    if (!mounted) return;
    setState(() => _busy = false);
    if (failed != null) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(failed)));
      return;
    }
    store.pendingQuestionFocus.unfocus();
  }

  /// Enter：**最后一页**提交整卡，中间页翻到下一题；Shift+Enter 换行。
  ///
  /// 必须拦在**文本字段外面**：多行 `TextField` 自己没有"Enter 提交"这个动作
  /// （`onSubmitted` 在 `maxLines > 1` 时不触发），而答题框的常见操作
  /// 是"输入后按回车发送"（DSH 的提问框同样如此）。
  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final isEnter =
        event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter;
    if (!isEnter) return KeyEventResult.ignored;
    final shift = HardwareKeyboard.instance.isShiftPressed;
    if (shift) return KeyEventResult.ignored; // Shift+Enter：交给文本框换行
    if (_isLast) {
      unawaited(_submit());
    } else {
      _go(1);
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    final qs = widget.questions;
    final page = _pageIndex;
    final q = qs[page];
    final total = qs.length;
    final paged = total > 1;
    // 矮窗口（< 700）里把配图与选项区一起收一收：180 的图 + 240 的选项 +
    // 输入行 + 底行会把 500 高的窗口撑破（Flutter 会直接画黄黑条纹）。
    final compact = MediaQuery.sizeOf(context).height < 700;
    final answered = q.answered;
    final picked = _multi[q.questionId] ?? const <int>{};
    final single = _single[q.questionId];
    final controller = _controllerOf(q);
    final hasText = controller.text.trim().isNotEmpty;
    // 最后一页那个按钮：多题卡**随时可交**（没作答的题按跳过发，答案可以为空）；
    // 单题卡保持老手感 —— 得先打点字 / 勾一个才让它变亮。
    final canSubmit = paged
        ? true
        : (hasText || (q.multiSelect && picked.isNotEmpty));
    // "自己输入答案"那一行本身就是个输入框：点它 = 把焦点给它。
    final focus = store.pendingQuestionFocus;

    return Container(
      // 与普通输入区同一套分层：底色高一档，不画顶边横线。
      color: theme.colorScheme.surfaceContainer,
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
      child: Column(
        // 与普通输入区同一条：写在页面那层 `ConstrainedBox` 里的上限（页面高度的 85%）
        // 只是**上限**。写成 max 会把它当成实际高度，提问一出现输入区就吃掉大半屏，
        // 消息区只剩一条缝，底行下面拖着一大片空白。
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (store.notice != null)
            _Banner(
              text: store.notice!,
              icon: Icons.info_outline,
              onClose: store.clearNotice,
            ),
          if (store.error != null)
            _Banner(text: store.error!, icon: Icons.error_outline),
          // 卡片高度**由内容决定**（不给它设上限）：里面的选项区自己有 240 上限、
          // 内部滚动，"自己输入答案"那行始终排在它下面、永远看得见。
          // 试过给整张卡设"可用高度的 60%"再让选项区 Flexible —— 矮窗口下那张卡会缩到
          // 只剩一行（Flexible 在紧高度里先被压扁），反而把作答入口挤没了，所以退回这个。
          Container(
            padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                color: theme.colorScheme.tertiary.withValues(alpha: 0.55),
              ),
              color: theme.colorScheme.surface,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Row(
                  children: [
                    Icon(
                      Icons.help_outline,
                      size: 15,
                      color: theme.colorScheme.tertiary,
                    ),
                    const SizedBox(width: 6),
                    // 与上面的折叠头一样：标题独占整行剩余宽度，不再与 `Spacer` 平分。
                    Expanded(
                      child: Text(
                        q.header?.isNotEmpty == true ? q.header! : 'AI 想确认一件事',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelMedium?.copyWith(
                          color: theme.colorScheme.tertiary,
                        ),
                      ),
                    ),
                    const SizedBox(width: 6),
                    // 多题卡需显示"还有几题"（参照 DSH 的「第 N/M 题」）
                    Text(
                      paged ? '第 ${page + 1}/$total 题' : '等你回答',
                      key: const Key('ask-page-indicator'),
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.tertiary,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                _AskQuestionHead(
                  question: q.question,
                  imageUrl: q.imageUrl == null
                      ? null
                      : absoluteBackendUrl(store.baseUrl, q.imageUrl!),
                  compact: compact,
                  imageKey: const Key('ask-composer-question-image'),
                ),
                if (q.multiSelect)
                  Text(
                    '可多选${q.options.isEmpty ? '' : '（选完按「${paged ? (_isLast ? '提交' : '下一题') : '提交'}」）'}',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                if (answered)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      (q.answer ?? '').isEmpty
                          ? '（这一题已跳过，模型会当作"用户没有回答"）'
                          : '已作答，${q.answer}',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.primary,
                      ),
                    ),
                  ),
                // 选项多 / 问题长时**只有选项那段**内部滚动（240 上限），
                // "自己输入答案"排在它下面、永远看得见
                ConstrainedBox(
                  constraints: BoxConstraints(maxHeight: compact ? 130 : 240),
                  child: SingleChildScrollView(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        for (var i = 0; i < q.options.length; i++) ...[
                          const SizedBox(height: 6),
                          _AskOptionRow(
                            // 带 key：气泡里那张提问卡**折叠时不该有**这些行，
                            // 测试用例依靠 key 区分"选项渲染份数"
                            key: Key('ask-composer-option-$i'),
                            index: i,
                            label: q.options[i],
                            imageUrl: q.optionImageAt(i) == null
                                ? null
                                : absoluteBackendUrl(
                                    store.baseUrl,
                                    q.optionImageAt(i)!,
                                  ),
                            // 多选题画复选框（勾了哪几个），单选画单选高亮
                            selected: q.multiSelect
                                ? picked.contains(i)
                                : single == i,
                            enabled: !_busy && !answered,
                            trailing: q.multiSelect
                                ? Icon(
                                    picked.contains(i)
                                        ? Icons.check_box
                                        : Icons.check_box_outline_blank,
                                    size: 18,
                                    color: picked.contains(i)
                                        ? theme.colorScheme.primary
                                        : theme.colorScheme.outline,
                                  )
                                : null,
                            onTap: () {
                              // 单问单选的**老手感**：点一下即答，不再要求按提交
                              if (!paged && !q.multiSelect) {
                                unawaited(_answerNow(q, optionIndex: i));
                                return;
                              }
                              setState(() {
                                if (q.multiSelect) {
                                  final set = _multi.putIfAbsent(
                                    q.questionId,
                                    () => <int>{},
                                  );
                                  set.contains(i) ? set.remove(i) : set.add(i);
                                  // 多选题：勾了选项就把手打的字清掉（两条路只能走一条）
                                  _controllerOf(q).clear();
                                } else {
                                  _single[q.questionId] = i;
                                  _controllerOf(q).clear();
                                }
                              });
                            },
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
                // --- 最后一行：固定"自己输入答案" ---
                const SizedBox(height: 6),
                _customAnswerRow(theme, focus, controller, q, paged),
              ],
            ),
          ),
          const SizedBox(height: 8),
          // 底行：左"跳过本题"（多题卡再加"上一题"），右"下一题 / 提交" + "停止生成"。
          // 两条退路都必须有：跳过让模型带着已有信息继续；停止则直接收掉这一轮。
          Row(
            children: [
              if (paged)
                TextButton(
                  key: const Key('ask-prev-page'),
                  onPressed: (_busy || page == 0) ? null : () => _go(-1),
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    minimumSize: const Size(0, 44),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  child: const Text('上一题'),
                ),
              TextButton(
                key: const Key('ask-skip-current'),
                onPressed: _busy ? null : _skipCurrent,
                style: TextButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  minimumSize: const Size(0, 44),
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
                child: const Text('跳过本题'),
              ),
              const Spacer(),
              if (store.running)
                TextButton.icon(
                  onPressed: () => store.stop(),
                  icon: const Icon(Icons.stop_circle_outlined, size: 16),
                  label: const Text('停止生成'),
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    minimumSize: const Size(0, 44),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                ),
              const SizedBox(width: 4),
              // 最后一页为「提交」；中间页为「下一题」。
              // ⚠ key 固定为 `ask-answer-submit`：它在单题卡上仍然就是"提交"那个按钮。
              FilledButton(
                key: const Key('ask-answer-submit'),
                onPressed: (_busy || !canSubmit)
                    ? null
                    : () {
                        if (_isLast) {
                          unawaited(_submit());
                        } else {
                          _go(1);
                        }
                      },
                style: FilledButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 14),
                  minimumSize: const Size(0, 44),
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
                child: Text(_isLast ? '提交' : '下一题'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// 最后一行：序号接着选项往下数，标签固定"自己输入答案"。
  Widget _customAnswerRow(
    ThemeData theme,
    FocusNode focus,
    TextEditingController controller,
    AiQuestion q,
    bool paged,
  ) {
    final outline = theme.colorScheme.outline;
    return Material(
      color: theme.colorScheme.surfaceContainerHighest,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(
          color: focus.hasFocus
              ? theme.colorScheme.tertiary
              : theme.dividerColor,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              width: 20,
              height: 20,
              margin: const EdgeInsets.only(top: 6),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(
                '${q.options.length + 1}',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              // 焦点事件拦在文本框外面（见 [_onKey]：Enter 提交、Shift+Enter 换行）
              child: Focus(
                onKeyEvent: _onKey,
                child: TextField(
                  key: const Key('ask-answer-field'),
                  controller: controller,
                  focusNode: focus,
                  enabled: !_busy && !q.answered,
                  minLines: 1,
                  maxLines: 4,
                  onChanged: (_) => setState(() {}),
                  decoration: InputDecoration(
                    isDense: true,
                    border: InputBorder.none,
                    // 单题卡与最后一页都是"Enter 提交"；中间页是"Enter 下一题"
                    hintText: _isLast
                        ? '自己输入答案（Enter 提交）'
                        : '自己输入答案（Enter 下一题）',
                    hintStyle: theme.textTheme.bodyMedium?.copyWith(
                      color: outline,
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
