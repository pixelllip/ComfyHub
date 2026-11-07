part of 'ai_home_page.dart';

// ---------------------------------------------------------------------------
//  提问卡（用户建议 ⑥，2026-09-20）
// ---------------------------------------------------------------------------

/// 一张提问卡的**历史记录形态**：AI 当场问过什么、用户答了什么。
///
/// 与 [_ToolCallCard] 刻意分开画：
///  - 工具卡是"我在干什么"（参数 + 结果预览，读的多、点的少）；
///  - 提问卡是"我需要你告诉我一件事"（问题 + 可点的选项）。
///
/// **不在这里放输入框**（用户 bug 2026-09-21）：AI 提问期间，输入区整块被
/// [_AskComposer] 接管，打字的地方只有底下一个；卡片里再放一个只会让人不知道
/// 该往哪儿写。所以这里是"问题 + 选项按钮"，要自己写的就点最后那行进底部输入框。
///
/// 用户答完之后卡片**不消失**，而是切成"已回答"—— 历史里一眼能看出这一轮
/// 是基于什么信息做的（与工具卡一样，都是这条回复的上下文）。
class _AskCard extends StatefulWidget {
  /// **整张卡**：同一次 `ask_user_question` 问的所有题（至少一条，按题号排）。
  ///
  /// 多问改造（2026-09-22）之前这里只有一条；只画第一条会让剩下的题在界面上
  /// 凭空消失（用户以为只问了一件事，模型却在等 N 个答案）。
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

  /// 用户手动点开来过 —— 之后**不再**跟着状态自动折回去。
  ///
  /// "等回答 → 已回答"会让卡片从"可点选项"变成"记着答案"；要是每次都替用户决定
  /// 展开还是收起，他刚点开看清楚、答案一到就被折回去，很难受。
  bool _expandedByUser = false;

  /// 用户手动折起来了 —— 已回答时**不再**自动展开。
  bool _collapsedByUser = false;

  /// 这一帧该展开吗。
  ///
  /// 折叠是**默认**（用户 bug 2026-09-21：气泡里那张卡和底部被接管的输入区把整组选项
  /// 画了两遍）。回答完又默认展开成"问题 + 答案"两行 —— 那才是历史里要留的信息。
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
                Flexible(
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
                const Spacer(),
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
          // 折起来也不能让某一题消失，否则用户根本不知道模型问了 N 件事。
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
            // （用户 2026-11-04 的现场）。等回答时看底部那张当前题的图就够；
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
            // 两边都放会让人不知道该往哪儿点（用户 bug 2026-09-21 的原话）。
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
            // 用户 bug 2026-09-21 的原话就是"最末尾选项固定让用户决定（输入你的答案）"。
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
                  // 跳过 = 如实告诉模型"用户没有回答"，它会基于已有信息继续
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
                      // 图片在**选项文字前面**（用户 2026-11-04 的口径：
                      // "如果有能力找到这种发型的示例图要贴到选项描述开头中"）。
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
/// 用户 2026-11-04 的现场：图铺满整行时上下都是黑边，鼠标一停还弹出「点击放大」
/// 把脸挡住；改成"左侧问题、右侧截图"之后，图只占一条窄栏，问题仍能一眼读完。
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
/// 鼠标停在图上时那句「点击放大」会正好盖住要看的细节（用户 2026-11-04）；
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
//  提问输入区（用户 bug 2026-09-21：AI 提问时**接管**聊天下部的输入模块）
// ---------------------------------------------------------------------------

/// AI 提问期间**替换掉普通输入框**的那一块。
///
/// 参考 DeepSeek Harness 的做法（`dsh-client-ui-user-questions` 的 composer takeover）：
/// 提问时编辑器整块变成答题卡 —— 选项点一下就答，要自己写的写在最后那一行，
/// 底下给"跳过 / 停止"。它是**唯一的**作答入口，所以不会出现
/// "气泡里一个输入框、底下又一个输入框，不知道往哪儿写"。
///
/// 多问改造（2026-09-22，用户拍板的形态）：一次 `ask_user_question` 可以问**最多 5 题**，
/// 界面是**一张卡分页**：
///  · 顶上一行写「第 N/M 题」；
///  · 底行是「上一题 / 跳过本题 / 下一题」，**最后一页**那个按钮变成「提交」；
///  · `multiSelect` 的题画复选框（选项上限 5），每题都保留"自己输入"；
///  · **答案可以为空**：整卡提交时没作答的题按"跳过"发（= 告诉模型"用户没回答"，
///    不是"没问过"）。一题一页地答完才收卡片 —— 只答一题就走会让模型白等剩下的。
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
  /// 而模型拿到的本来就是"用户说了什么"这段文字 —— 所以多选拼成「A、B」发，
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

  /// 跳过**当前这一题**（后端当作"用户没有回答"放行，模型基于已有信息继续）。
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
        // 这一题用户什么也没选：按"跳过"发 —— 模型会如实说缺什么，不会假装他答了
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
  /// （`onSubmitted` 在 `maxLines > 1` 时根本不会触发），而用户对答题框的直觉
  /// 就是"打完按回车发出去"（DSH 的提问框也是这个规矩）。
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
      decoration: BoxDecoration(
        color: theme.colorScheme.surface,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
      child: Column(
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
                    Flexible(
                      child: Text(
                        q.header?.isNotEmpty == true ? q.header! : 'AI 想确认一件事',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelMedium?.copyWith(
                          color: theme.colorScheme.tertiary,
                        ),
                      ),
                    ),
                    const Spacer(),
                    // 多题卡要一眼看出"还有几题"（用户给的 DSH 形态就是「第 N/M 题」）
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
                            // 用例靠 key 区分"选项到底画了几份"（用户 bug 2026-09-21）
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
                // --- 最后一行：固定"自己输入答案"（用户 bug 2026-09-21）---
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
              // 最后一页就是「提交」（用户给的形态）；中间页是「下一题」。
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
