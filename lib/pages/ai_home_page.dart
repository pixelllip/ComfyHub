import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/api_client.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/markdown_view.dart';

/// AI 工作台（AIH-001 / AIH-002 / AIK-002）。
///
/// 宽屏三栏：会话列表 | 消息 + 输入 | ComfyUI 与上下文。
/// 窄屏：会话列表进抽屉，状态栏进底部 Sheet，输入区**始终可见**。
class AiHomePage extends StatefulWidget {
  const AiHomePage({super.key});

  @override
  State<AiHomePage> createState() => _AiHomePageState();
}

class _AiHomePageState extends State<AiHomePage> {
  final _input = TextEditingController();
  final _inputFocus = FocusNode();
  final _scroll = ScrollController();
  bool _loaded = false;

  /// 当前输入框内容挂在哪条会话上（用来把草稿写进本地存储）。
  String? _draftConversationId;

  /// 缓存 store：`dispose()` 里不能再碰 `context`
  late AiWorkspaceStore _store;

  /// 正在把草稿写回输入框。这时**必须屏蔽** `_onInputChanged` ——
  /// 否则"清空输入框"这个动作会被当成"用户把字删光了"，把刚取回来的草稿覆盖成空串
  /// （切走再切回来字就没了，用户报的 bug ②）。
  bool _applyingDraft = false;

  /// 第几次切换会话：异步取草稿回来时用它判断"还是不是同一次切换"。
  int _draftEpoch = 0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_loaded) return;
    _loaded = true;
    _store = context.read<AiWorkspaceStore>();
    _input.addListener(_onInputChanged);
    _store.addListener(_onStoreChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      // `load()` 自己是幂等的：切到别的页再回来（页面会被重建）不会重新加载、
      // 更不会再新建一条会话。
      if (mounted) _store.load();
    });
  }

  /// 输入变化就落草稿：切会话 / 关掉 App 都不会把没发出去的文字丢掉。
  void _onInputChanged() {
    if (_applyingDraft) return;
    final id = _draftConversationId;
    if (id == null || !mounted) return;
    _store.saveDraft(id, _input.text);
  }

  /// 会话切换时把上一条的草稿存好、把新一条的草稿取回来。
  void _onStoreChanged() {
    if (!mounted) return;
    final id = _store.conversation?.id;
    if (id == _draftConversationId) return;

    final previous = _draftConversationId;
    _draftConversationId = id;
    if (previous != null) {
      // 附件托盘由 Store 随会话存走（`AiWorkspaceStore._stashAttachments`），
      // 这里只保住输入框里的文字
      _store.saveDraft(previous, _input.text);
    }
    if (id == null) return;

    // 先把输入框清干净（屏蔽回调，别把新会话的草稿抹掉），再去取这条会话的草稿
    _setInputText('');
    final epoch = ++_draftEpoch;
    _store.loadDraft(id).then((draft) {
      if (!mounted || _draftEpoch != epoch) return;
      // 取草稿期间用户已经开始打字了：以用户现在打的为准，不要回退成旧草稿
      if (_input.text.isEmpty) _setInputText(draft.text);
    });
  }

  /// 程序性地改输入框内容：**不**触发草稿保存（草稿是"用户打出来的"才算数）。
  void _setInputText(String text) {
    _applyingDraft = true;
    _input.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
    _applyingDraft = false;
  }

  @override
  void dispose() {
    _input.removeListener(_onInputChanged);
    _store.removeListener(_onStoreChanged);
    // 页面销毁（比如切到别的功能页、关窗口）之前把当前草稿落一次
    final id = _draftConversationId;
    if (id != null) {
      _store.saveDraft(id, _input.text);
    }
    _input.dispose();
    _inputFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.animateTo(
          _scroll.position.maxScrollExtent,
          duration: const Duration(milliseconds: 180),
          curve: Curves.easeOut,
        );
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AiWorkspaceStore>();
    final width = MediaQuery.sizeOf(context).width;
    final wide = width >= 900;

    final conversationList = _ConversationList(store: store);
    final thread = Column(
      children: [
        Expanded(child: _MessageList(store: store, controller: _scroll)),
        _Composer(
          store: store,
          controller: _input,
          focusNode: _inputFocus,
          onSubmit: (text) async {
            await store.send(text);
            _input.clear();
            _scrollToBottom();
          },
        ),
      ],
    );

    if (wide) {
      final showPanel = width >= 1200;
      return Scaffold(
        body: Row(
          children: [
            SizedBox(width: 240, child: conversationList),
            const VerticalDivider(width: 1),
            Expanded(child: thread),
            if (showPanel) ...[
              const VerticalDivider(width: 1),
              SizedBox(width: 260, child: _ContextPanel(store: store)),
            ],
          ],
        ),
      );
    }

    return Scaffold(
      drawer: Drawer(child: SafeArea(child: conversationList)),
      body: thread,
      floatingActionButton: FloatingActionButton.small(
        tooltip: 'ComfyUI 状态',
        onPressed: () => _showStatusSheet(context, store),
        child: const Icon(Icons.dns_outlined),
      ),
    );
  }

  void _showStatusSheet(BuildContext context, AiWorkspaceStore store) {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (_) => SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.6,
        child: _ContextPanel(store: store),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  会话列表
// ---------------------------------------------------------------------------

class _ConversationList extends StatelessWidget {
  final AiWorkspaceStore store;
  const _ConversationList({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
          child: FilledButton.icon(
            onPressed: () => store.newConversation(),
            icon: const Icon(Icons.add_comment_outlined, size: 18),
            label: const Text('新建对话'),
          ),
        ),
        const Divider(height: 1),
        Expanded(
          child: store.conversations.isEmpty
              ? Center(
                  child: Text(
                    '还没有对话',
                    style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                  ),
                )
              : ListView.builder(
                  itemCount: store.conversations.length,
                  itemBuilder: (context, i) {
                    final c = store.conversations[i];
                    final selected = store.conversation?.id == c.id;
                    return ListTile(
                      dense: true,
                      selected: selected,
                      title: Text(c.title, maxLines: 1, overflow: TextOverflow.ellipsis),
                      subtitle: Text('${c.messageCount} 条消息', style: theme.textTheme.labelSmall),
                      onTap: () {
                        store.openConversation(c.id);
                        if (Scaffold.maybeOf(context)?.isDrawerOpen == true) {
                          Navigator.of(context).pop();
                        }
                      },
                      trailing: PopupMenuButton<String>(
                        tooltip: '更多',
                        onSelected: (v) {
                          if (v == 'rename') _rename(context, c);
                          if (v == 'archive') store.archiveConversation(c.id);
                          if (v == 'delete') store.deleteConversation(c.id);
                        },
                        itemBuilder: (_) => const [
                          PopupMenuItem(value: 'rename', child: Text('重命名')),
                          PopupMenuItem(value: 'archive', child: Text('归档')),
                          PopupMenuItem(value: 'delete', child: Text('删除')),
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }

  Future<void> _rename(BuildContext context, AiConversation c) async {
    final controller = TextEditingController(text: c.title);
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('重命名会话'),
        content: TextField(controller: controller, autofocus: true),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, controller.text), child: const Text('保存')),
        ],
      ),
    );
    if (result != null && result.trim().isNotEmpty) {
      if (store.conversation?.id == c.id) {
        await store.renameConversation(result.trim());
      } else {
        await store.openConversation(c.id);
        await store.renameConversation(result.trim());
      }
    }
  }
}

// ---------------------------------------------------------------------------
//  消息区
// ---------------------------------------------------------------------------

class _MessageList extends StatelessWidget {
  final AiWorkspaceStore store;
  final ScrollController controller;
  const _MessageList({required this.store, required this.controller});

  @override
  Widget build(BuildContext context) {
    if (store.messages.isEmpty) return _EmptyThread(store: store);

    return ListView.builder(
      controller: controller,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      itemCount: store.messages.length,
      itemBuilder: (context, i) {
        final message = store.messages[i];
        // RepaintBoundary：流式生成时每一帧都会重建列表，没有它的话所有
        // Markdown 气泡都要跟着重绘一遍（长对话越聊越卡）。
        return RepaintBoundary(
          child: _MessageBubble(
            message: message,
            sending: store.sending,
            onRetry: store.retry,
            // 工具卡 / 思考都只属于助手消息
            toolCalls: message.isUser ? const [] : store.toolCallsFor(message),
            reasoning: message.isUser ? '' : store.reasoningFor(message),
            toolCategoryOf: store.toolCategoryOf,
            onApprove: store.approveToolCall,
            onDeny: store.denyToolCall,
          ),
        );
      },
    );
  }
}

/// 一条消息气泡。
///
/// 拆成独立组件（而不是塞在 `_MessageList` 的 itemBuilder 里）是为了让
/// Markdown 渲染只在**这一条**变化时重建：长回复逐字流式刷新时，
/// 早就不变的历史气泡不会被重新解析。
class _MessageBubble extends StatelessWidget {
  final AiMessage message;
  final bool sending;
  final Future<void> Function(String assistantMessageId) onRetry;

  /// 这条消息的有序工具调用（M4）。
  final List<AiToolCallState> toolCalls;

  /// 这条消息累积的思考正文（`reasoning.delta` / reasoning 块）。
  final String reasoning;

  final String Function(String toolName) toolCategoryOf;
  final Future<void> Function(String callId) onApprove;
  final Future<void> Function(String callId) onDeny;

  const _MessageBubble({
    required this.message,
    required this.sending,
    required this.onRetry,
    this.toolCalls = const [],
    this.reasoning = '',
    required this.toolCategoryOf,
    required this.onApprove,
    required this.onDeny,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final m = message;
    final align = m.isUser ? CrossAxisAlignment.end : CrossAxisAlignment.start;
    final bg = m.isUser
        ? theme.colorScheme.primaryContainer
        : theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.5);
    // 失败/取消的助手消息可以重试：新建一个 Run，用 retryOfRunId 关联回去（AIH-024）
    final canRetry = !m.isUser && (m.status == 'failed' || m.status == 'cancelled') && !sending;
    // 待批准的工具调用（M4）：气泡里要能点「批准 / 拒绝」
    final pending = toolCalls.where((c) => c.needsApproval).toList();

    return Column(
      crossAxisAlignment: align,
      children: [
        Container(
          constraints: const BoxConstraints(maxWidth: 720),
          margin: const EdgeInsets.symmetric(vertical: 6),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(12)),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // 思考过程（M4）：默认折叠，只有真有 reasoning 增量时才出现
              if (!m.isUser && reasoning.trim().isNotEmpty) _ReasoningPanel(text: reasoning),
              for (final part in m.parts)
                if (part.type == 'attachment')
                  Padding(
                    padding: const EdgeInsets.only(bottom: 4),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const Icon(Icons.attach_file, size: 14),
                        const SizedBox(width: 4),
                        Text(part.text ?? part.attachmentId ?? '附件',
                            style: theme.textTheme.labelSmall),
                      ],
                    ),
                  ),
              // 用户消息按纯文本显示（自己敲的，不需要渲染）；
              // 助手消息渲染 Markdown —— 之前把 `**加粗**` 原样吐出来，很难读。
              if (m.text.isNotEmpty)
                m.isUser
                    ? SelectableText(m.text)
                    : MarkdownText(m.text, style: theme.textTheme.bodyMedium),
              // 流式进行中且还没有内容：给一个明确的"在生成"提示，而不是空白气泡
              if (m.status == 'streaming' && m.text.isEmpty && toolCalls.isEmpty)
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const SizedBox(
                      width: 12,
                      height: 12,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                    const SizedBox(width: 8),
                    Text('正在生成…', style: theme.textTheme.bodySmall),
                  ],
                ),
              // 工具卡（M4）：按调用顺序排在正文下面
              if (toolCalls.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (final call in toolCalls)
                        _ToolCallCard(
                          key: ValueKey('${m.id}-${call.callId}'),
                          call: call,
                          category: toolCategoryOf(call.name),
                          onApprove: onApprove,
                          onDeny: onDeny,
                        ),
                    ],
                  ),
                ),
              if (pending.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(
                    '有 ${pending.length} 个工具调用在等你的批准（写盘 / 改动类操作）。',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                  ),
                ),
              if (m.status == 'cancelled')
                Text('（已停止）',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
              if (m.status == 'failed')
                Text('（生成失败）',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error)),
              if (canRetry)
                Align(
                  alignment: Alignment.centerLeft,
                  child: TextButton.icon(
                    onPressed: () => onRetry(m.id),
                    icon: const Icon(Icons.refresh, size: 16),
                    label: const Text('重试'),
                    style: TextButton.styleFrom(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      minimumSize: const Size(0, 32),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                  ),
                ),
              // token 统计（AIH-057）：只对真有 usage 的助手消息显示，没有就不占位
              if (!m.isUser && (m.usage?.isEmpty == false || (m.reasoningEffort ?? 'off') != 'off'))
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if ((m.reasoningEffort ?? 'off') != 'off') ...[
                        Icon(Icons.psychology_outlined,
                            size: 13, color: theme.colorScheme.outline),
                        const SizedBox(width: 3),
                        Text(
                          '思考 ${AiReasoningEffort.parse(m.reasoningEffort)?.label ?? m.reasoningEffort}',
                          style: theme.textTheme.labelSmall
                              ?.copyWith(color: theme.colorScheme.outline),
                        ),
                        const SizedBox(width: 10),
                      ],
                      if (m.usage?.isEmpty == false) ...[
                        Icon(Icons.data_usage, size: 13, color: theme.colorScheme.outline),
                        const SizedBox(width: 3),
                        Tooltip(
                          message: m.usage!.detailLabel,
                          child: Text(
                            m.usage!.shortLabel,
                            style: theme.textTheme.labelSmall
                                ?.copyWith(color: theme.colorScheme.outline),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
//  工具卡与思考（M4）
// ---------------------------------------------------------------------------

/// 一张工具卡：图标 + 工具名 + 状态徽标 + 耗时，详情默认折叠。
///
/// 只渲染后端给的内容（后端已经把凭据脱敏、结果截断到 8KB），前端不改写、不补全。
class _ToolCallCard extends StatefulWidget {
  final AiToolCallState call;
  final String category;
  final Future<void> Function(String callId) onApprove;
  final Future<void> Function(String callId) onDeny;

  const _ToolCallCard({
    super.key,
    required this.call,
    required this.category,
    required this.onApprove,
    required this.onDeny,
  });

  @override
  State<_ToolCallCard> createState() => _ToolCallCardState();
}

class _ToolCallCardState extends State<_ToolCallCard> {
  bool _expanded = false;
  bool _busy = false;

  IconData get _icon => switch (widget.category) {
        'skill' => Icons.auto_awesome_outlined,
        'files' => Icons.folder_outlined,
        _ => Icons.dns_outlined,
      };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final call = widget.call;
    final color = switch (call.status) {
      AiToolCallStatus.ok => theme.colorScheme.primary,
      AiToolCallStatus.pendingApproval => theme.colorScheme.tertiary,
      AiToolCallStatus.failed || AiToolCallStatus.denied => theme.colorScheme.error,
      AiToolCallStatus.running => theme.colorScheme.outline,
    };

    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.5)),
        color: theme.colorScheme.surface.withValues(alpha: 0.6),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(_icon, size: 14, color: color),
              const SizedBox(width: 6),
              Flexible(
                child: Text(
                  call.name.isEmpty ? '未知工具' : call.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelMedium,
                ),
              ),
              const SizedBox(width: 6),
              _StatusChip(label: call.status.label, color: color),
              if (call.elapsedLabel.isNotEmpty) ...[
                const SizedBox(width: 6),
                Text(call.elapsedLabel,
                    style: theme.textTheme.labelSmall
                        ?.copyWith(color: theme.colorScheme.outline)),
              ],
              const Spacer(),
              if (call.isRunning)
                const SizedBox(
                  width: 12,
                  height: 12,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              if (call.hasDetail)
                InkWell(
                  onTap: () => setState(() => _expanded = !_expanded),
                  child: Padding(
                    padding: const EdgeInsets.all(2),
                    child: Icon(
                      _expanded ? Icons.expand_less : Icons.expand_more,
                      size: 16,
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
            ],
          ),
          // 待批准：气泡里直接给按钮（后端 ToolApprovalGate 已经开着，点了才算数）
          if (call.needsApproval)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Row(
                children: [
                  FilledButton(
                    onPressed: _busy ? null : () => _resolve(approve: true),
                    style: FilledButton.styleFrom(
                      padding: const EdgeInsets.symmetric(horizontal: 12),
                      minimumSize: const Size(0, 30),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    child: const Text('批准'),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton(
                    onPressed: _busy ? null : () => _resolve(approve: false),
                    style: OutlinedButton.styleFrom(
                      padding: const EdgeInsets.symmetric(horizontal: 12),
                      minimumSize: const Size(0, 30),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    child: const Text('拒绝'),
                  ),
                ],
              ),
            ),
          if (_expanded)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 260),
                child: SingleChildScrollView(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (call.arguments.isNotEmpty) ...[
                        Text('参数', style: theme.textTheme.labelSmall),
                        SelectableText(call.arguments, style: theme.textTheme.bodySmall),
                        if (call.detailText.isNotEmpty) const SizedBox(height: 6),
                      ],
                      if (call.detailText.isNotEmpty) ...[
                        Text(
                          call.status == AiToolCallStatus.ok ? '结果' : '说明',
                          style: theme.textTheme.labelSmall,
                        ),
                        SelectableText(call.detailText, style: theme.textTheme.bodySmall),
                      ],
                    ],
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  Future<void> _resolve({required bool approve}) async {
    setState(() => _busy = true);
    try {
      await (approve ? widget.onApprove(widget.call.callId) : widget.onDeny(widget.call.callId));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

class _StatusChip extends StatelessWidget {
  final String label;
  final Color color;
  const _StatusChip({required this.label, required this.color});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(label, style: theme.textTheme.labelSmall?.copyWith(color: color)),
    );
  }
}

/// 思考过程：默认折叠，避免把气泡撑成一屏高。
class _ReasoningPanel extends StatefulWidget {
  final String text;
  const _ReasoningPanel({required this.text});

  @override
  State<_ReasoningPanel> createState() => _ReasoningPanelState();
}

class _ReasoningPanelState extends State<_ReasoningPanel> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            onTap: () => setState(() => _expanded = !_expanded),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(_expanded ? Icons.expand_less : Icons.expand_more,
                    size: 16, color: theme.colorScheme.outline),
                Icon(Icons.psychology_outlined, size: 14, color: theme.colorScheme.outline),
                const SizedBox(width: 4),
                Text('思考过程',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
              ],
            ),
          ),
          if (_expanded)
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 240),
              child: SingleChildScrollView(
                child: Text(
                  widget.text,
                  style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _EmptyThread extends StatelessWidget {
  final AiWorkspaceStore store;
  const _EmptyThread({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(Icons.auto_awesome_outlined, size: 40, color: theme.colorScheme.primary),
              const SizedBox(height: 12),
              Text('AI 工作台', style: theme.textTheme.titleLarge),
              const SizedBox(height: 6),
              Text(
                '生图 / 生视频需求的澄清、提示词设计与进度跟进。\n'
                '输入 / 可以调出 Skills 目录；也可以把文件拖进来当附件。',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.outline),
              ),
              if (!store.hasProvider) ...[
                const SizedBox(height: 16),
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(12),
                    child: Text(
                      '还没有配置 Provider。请到「设置 → AI 模型」添加协议、Base URL 和模型目录；'
                      'API Key 只写不读，不会出现在界面或日志里。',
                      style: theme.textTheme.bodySmall,
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  输入区
// ---------------------------------------------------------------------------

class _Composer extends StatefulWidget {
  final AiWorkspaceStore store;
  final TextEditingController controller;
  final FocusNode focusNode;
  final Future<void> Function(String text) onSubmit;

  const _Composer({
    required this.store,
    required this.controller,
    required this.focusNode,
    required this.onSubmit,
  });

  @override
  State<_Composer> createState() => _ComposerState();
}

class _ComposerState extends State<_Composer> {
  bool _composing = false;

  /// `/` 后面的查询词；null = 不显示 Skill 菜单。
  String? _slashQuery;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onControllerChanged);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onControllerChanged);
    super.dispose();
  }

  /// `/` 菜单：输入区里最后一段要是 `/xxx`（行首或空格后）才弹出来。
  void _onControllerChanged() {
    final text = widget.controller.text;
    final match = RegExp(r'(?:^|\s)/([^\s/]*)$').firstMatch(text);
    final next = match?.group(1);
    if (next != _slashQuery) setState(() => _slashQuery = next);
  }

  /// 菜单里列出的 skills：只列**用户可调用**且启用的（`/` 就是用户显式调用）。
  List<AiSkill> get _slashMatches {
    final query = _slashQuery;
    if (query == null) return const [];
    final all = widget.store.skills.where((s) => s.enabled && s.userInvocable).toList();
    if (query.isEmpty) return all;
    final q = query.toLowerCase();
    return all
        .where((s) =>
            s.name.toLowerCase().contains(q) || s.description.toLowerCase().contains(q))
        .toList();
  }

  /// 把 `/xxx` 换成 `/skill 名 `，模型看到的就是这个 skill 名。
  void _insertSlash(AiSkill skill) {
    final text = widget.controller.text;
    final match = RegExp(r'(?:^|\s)/([^\s/]*)$').firstMatch(text);
    if (match == null) return;
    final slash = text.lastIndexOf('/');
    final next = '${text.substring(0, slash)}/${skill.name} ';
    widget.controller.value =
        TextEditingValue(text: next, selection: TextSelection.collapsed(offset: next.length));
    setState(() => _slashQuery = null);
    widget.focusNode.requestFocus();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    final blockers = store.preflight?.blockers ?? const <String>[];
    final slashMatches = _slashMatches;

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
          if (store.error != null) _Banner(text: store.error!, icon: Icons.error_outline),
          if (blockers.isNotEmpty)
            _Banner(
              text: '附件未通过准入，发送会被阻断：\n${blockers.map((b) => '· $b').join('\n')}',
              icon: Icons.block,
              error: true,
            ),
          if (store.attachments.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (var i = 0; i < store.attachments.length; i++)
                    Chip(
                      label: Text('${store.attachments[i].name} · ${store.attachments[i].sizeLabel}',
                          style: theme.textTheme.labelSmall),
                      onDeleted: () => store.removeAttachmentAt(i),
                    ),
                ],
              ),
            ),
          // Skills 的 `/` 菜单（M5）：只列可见的几条，懒构建，点一条就写进输入框
          if (slashMatches.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 180),
                // 底色必须由 Material 给：用 Container 的 decoration 会盖掉
                // ListTile 的水波纹（Flutter 会直接抛断言）
                child: Material(
                  color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                    side: BorderSide(color: theme.dividerColor),
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: ListView.builder(
                    shrinkWrap: true,
                    padding: EdgeInsets.zero,
                    itemCount: slashMatches.length,
                    itemBuilder: (context, i) {
                      final skill = slashMatches[i];
                      return ListTile(
                        dense: true,
                        visualDensity: VisualDensity.compact,
                        title: Text('/${skill.name}', style: theme.textTheme.labelMedium),
                        subtitle: Text(
                          skill.oneLine.isEmpty ? skill.sourceLabel : skill.oneLine,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.labelSmall,
                        ),
                        trailing: _Badge(skill.sourceLabel),
                        onTap: () => _insertSlash(skill),
                      );
                    },
                  ),
                ),
              ),
            ),
          // 排版：上一行整宽输入框，下一行 [附件] [模型选择] …… [发送]
          Shortcuts(
            shortcuts: const {
              SingleActivator(LogicalKeyboardKey.enter): _SendIntent(),
            },
            child: Actions(
              actions: {
                _SendIntent: CallbackAction<_SendIntent>(
                  onInvoke: (_) {
                    // 中文输入法 composing 期间的回车是"选词"，不能当发送（AIH-053）
                    if (_composing) return null;
                    // `/` 菜单开着时回车不发送（否则会把半截 skill 名发出去）
                    if (_slashMatches.isNotEmpty) return null;
                    _submit();
                    return null;
                  },
                ),
              },
              child: TextField(
                controller: widget.controller,
                focusNode: widget.focusNode,
                minLines: 2,
                maxLines: 8,
                textInputAction: TextInputAction.newline,
                onChanged: (v) => setState(() => _composing = false),
                decoration: const InputDecoration(
                  hintText: '描述你的生图 / 生视频需求，或输入 / 调用 Skill（Enter 发送，Shift+Enter 换行）',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              IconButton(
                tooltip: '添加附件',
                onPressed: _pickFiles,
                icon: const Icon(Icons.attach_file),
              ),
              _ModelPicker(store: store),
              const SizedBox(width: 8),
              _EffortPicker(store: store),
              const Spacer(),
              // 本次对话的 token 汇总（AIH-057）：一直是可见的，不必翻设置
              if (!store.usageSummary.isEmpty)
                Padding(
                  padding: const EdgeInsets.only(right: 12),
                  child: Tooltip(
                    message: store.usageSummary.label,
                    child: Text(
                      '本对话 ↑${AiTokenUsage.compact(store.usageSummary.inputTokens)} '
                      '↓${AiTokenUsage.compact(store.usageSummary.outputTokens)}',
                      style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                    ),
                  ),
                ),
              FilledButton.icon(
                onPressed: store.sending ? () => store.stop() : (store.canSend ? _submit : null),
                icon: Icon(store.sending ? Icons.stop : Icons.send, size: 18),
                label: Text(store.sending ? '停止' : '发送'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  void _submit() {
    final text = widget.controller.text;
    widget.onSubmit(text);
  }

  Future<void> _pickFiles() async {
    // file_picker 12.x 起 pickFiles 是静态方法，直接返回 List<PlatformFile>
    final files = await FilePicker.pickFiles(dialogTitle: '选择要发给 AI 的附件');
    if (files.isEmpty) return;
    for (final f in files) {
      final size = f.lengthSync() ?? await f.length();
      widget.store.addAttachment(AiAttachment(
        name: f.name,
        // 模态由后端严格分类器判定（AIH-027）；这里只做初步提示，不当作准入结论
        modality: _guessModality(f.extension),
        mimeType: _guessMime(f.extension),
        sizeBytes: size,
      ));
    }
  }
}

class _SendIntent extends Intent {
  const _SendIntent();
}

/// 仅用于预检请求的初步类型（后端会用 magic bytes 复核，AIH-027）。
String? _guessModality(String? ext) {
  switch ((ext ?? '').toLowerCase()) {
    case 'png':
    case 'jpg':
    case 'jpeg':
    case 'webp':
    case 'gif':
    case 'bmp':
      return 'image';
    case 'mp4':
    case 'webm':
    case 'mov':
    case 'mkv':
      return 'video';
    case 'mp3':
    case 'wav':
    case 'flac':
    case 'm4a':
      return 'audio';
    case 'pdf':
    case 'txt':
    case 'md':
    case 'doc':
    case 'docx':
      return 'document';
    default:
      return null; // 未知类型交给后端判定并在预检里阻断
  }
}

String _guessMime(String? ext) {
  switch ((ext ?? '').toLowerCase()) {
    case 'png':
      return 'image/png';
    case 'jpg':
    case 'jpeg':
      return 'image/jpeg';
    case 'webp':
      return 'image/webp';
    case 'gif':
      return 'image/gif';
    case 'mp4':
      return 'video/mp4';
    case 'webm':
      return 'video/webm';
    case 'mov':
      return 'video/quicktime';
    case 'mp3':
      return 'audio/mpeg';
    case 'wav':
      return 'audio/wav';
    case 'pdf':
      return 'application/pdf';
    default:
      return 'application/octet-stream';
  }
}

class _Banner extends StatelessWidget {
  final String text;
  final IconData icon;
  final bool error;
  final VoidCallback? onClose;

  const _Banner({required this.text, required this.icon, this.error = false, this.onClose});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = error ? theme.colorScheme.error : theme.colorScheme.primary;
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 16, color: color),
          const SizedBox(width: 8),
          Expanded(child: Text(text, style: theme.textTheme.bodySmall)),
          if (onClose != null)
            InkWell(onTap: onClose, child: const Icon(Icons.close, size: 14)),
        ],
      ),
    );
  }
}

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
  const _ModelPicker({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final model = store.selectedModel;
    return InkWell(
      onTap: () => showDialog<void>(
        context: context,
        builder: (_) => _ModelPickerDialog(store: store),
      ),
      borderRadius: BorderRadius.circular(8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
        decoration: BoxDecoration(
          border: Border.all(color: theme.dividerColor),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              model == null ? Icons.error_outline : Icons.memory,
              size: 16,
              color: model == null ? theme.colorScheme.error : null,
            ),
            const SizedBox(width: 6),
            Text(model?.displayName ?? '未选择模型', style: theme.textTheme.labelMedium),
            const SizedBox(width: 6),
            for (final m in model?.modalities ?? const <AiModality>[])
              Padding(
                padding: const EdgeInsets.only(right: 4),
                child: _Badge(m.label),
              ),
            const SizedBox(width: 2),
            Icon(Icons.arrow_drop_down, size: 18, color: theme.colorScheme.outline),
          ],
        ),
      ),
    );
  }
}

/// 可搜索的模型选择对话框（懒构建：只建屏幕上看得见的那几行）。
class _ModelPickerDialog extends StatefulWidget {
  final AiWorkspaceStore store;
  const _ModelPickerDialog({required this.store});

  @override
  State<_ModelPickerDialog> createState() => _ModelPickerDialogState();
}

class _ModelPickerDialogState extends State<_ModelPickerDialog> {
  final _search = TextEditingController();
  String _query = '';

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  /// 按 id / 显示名过滤（大小写不敏感）。
  List<AiModel> get _filtered {
    final q = _query.trim().toLowerCase();
    if (q.isEmpty) return widget.store.models;
    return widget.store.models
        .where((m) => m.id.toLowerCase().contains(q) || m.displayName.toLowerCase().contains(q))
        .toList();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final models = _filtered;
    final selectedId = widget.store.selectedModel?.id;

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
              padding: const EdgeInsets.symmetric(horizontal: 12),
              child: Row(
                children: [
                  Text('共 ${widget.store.models.length} 个模型',
                      style: theme.textTheme.labelSmall
                          ?.copyWith(color: theme.colorScheme.outline)),
                  const Spacer(),
                  Text('显示 ${models.length} 个',
                      style: theme.textTheme.labelSmall
                          ?.copyWith(color: theme.colorScheme.outline)),
                ],
              ),
            ),
            const Divider(height: 12),
            // 懒构建：69 个模型也只建可见的那几行
            Expanded(
              child: models.isEmpty
                  ? Center(
                      child: Text('没有匹配的模型',
                          style: theme.textTheme.bodySmall
                              ?.copyWith(color: theme.colorScheme.outline)),
                    )
                  : ListView.builder(
                      itemCount: models.length,
                      itemBuilder: (context, i) {
                        final m = models[i];
                        return ListTile(
                          dense: true,
                          selected: m.id == selectedId,
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
                                style: theme.textTheme.labelSmall
                                    ?.copyWith(color: theme.colorScheme.outline),
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
                          // 唯一的选择入口：store.selectModel
                          onTap: () {
                            widget.store.selectModel(m);
                            Navigator.of(context).pop();
                          },
                        );
                      },
                    ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 思考强度选择器（AIH-056）。
///
/// 只显示**当前模型声明过**的档位：没声明推理能力就整块置灰并说明原因，
/// 而不是给一堆选了会被后端拒绝的选项。
class _EffortPicker extends StatelessWidget {
  final AiWorkspaceStore store;
  const _EffortPicker({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final model = store.selectedModel;
    final options = model?.selectableEfforts ?? const <AiReasoningEffort>[];
    final enabled = options.isNotEmpty;

    final label = enabled
        ? store.reasoningEffort.label
        : (model == null ? '思考强度' : '不支持思考');

    return PopupMenuButton<AiReasoningEffort>(
      tooltip: enabled
          ? '思考强度（当前模型声明：${options.map((e) => e.label).join(' / ')}）'
          : '该模型未声明可用的思考档位，请到「设置 → AI 模型」声明',
      enabled: enabled,
      onSelected: store.selectReasoningEffort,
      itemBuilder: (_) => [
        for (final e in options)
          PopupMenuItem(
            value: e,
            child: Row(
              children: [
                Icon(
                  e == store.reasoningEffort ? Icons.radio_button_checked : Icons.radio_button_off,
                  size: 16,
                ),
                const SizedBox(width: 8),
                Text(e.label),
                const Spacer(),
                if (model?.effortWireValue(e) != null)
                  Text(
                    '→ ${model!.effortWireValue(e)}',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                  ),
              ],
            ),
          ),
      ],
      child: Opacity(
        opacity: enabled ? 1 : 0.6,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            border: Border.all(color: theme.dividerColor),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.psychology_outlined,
                  size: 16,
                  color: store.reasoningEffort.isThinking && enabled
                      ? theme.colorScheme.primary
                      : null),
              const SizedBox(width: 5),
              Text(label, style: theme.textTheme.labelMedium),
            ],
          ),
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  final String text;
  const _Badge(this.text);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: theme.colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text, style: theme.textTheme.labelSmall),
    );
  }
}

// ---------------------------------------------------------------------------
//  右侧上下文：模型能力 / Skills / ComfyUI
// ---------------------------------------------------------------------------

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

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
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
    final store = widget.store;
    final model = store.selectedModel;

    // CustomScrollView + SliverList：固定几段用 SliverToBoxAdapter，
    // skills 可能有几十个，必须懒构建（`ListView(children: [...])` 会一次全建）。
    return CustomScrollView(
      slivers: [
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 0),
          sliver: SliverList.list(children: [
            Text('模型能力', style: theme.textTheme.labelLarge),
            const SizedBox(height: 6),
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
                '来源：${model.capabilitySourceLabel}'
                '${model.contextWindow != null ? ' · 上下文 ${model.contextWindow}' : ''}',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ],
            const SizedBox(height: 16),
            // 「ComfyUI」右边就是刷新按钮：它刷新的是 ComfyUI 状态，
            // 以前挂在"上下文"标题右边，看着像是刷新整个面板（用户建议第 5 条）。
            Row(
              children: [
                Text('ComfyUI', style: theme.textTheme.labelLarge),
                IconButton(
                  tooltip: '刷新 ComfyUI 状态',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                  padding: EdgeInsets.zero,
                  onPressed: _loading ? null : _refresh,
                  icon: const Icon(Icons.refresh, size: 16),
                ),
              ],
            ),
            const SizedBox(height: 6),
            if (_loading)
              const LinearProgressIndicator(minHeight: 2)
            else if (_error != null)
              Text('查询失败：$_error', style: theme.textTheme.bodySmall)
            else if (_status == null)
              Text('暂无数据', style: theme.textTheme.bodySmall)
            else ...[
              _KV('连通性', _status!.comfyReachable ? '可达' : '不可达'),
              _KV('运行中', '${_status!.queueRunning}'),
              _KV('等待中', '${_status!.queuePending}'),
              _KV('已捕获', '${_status!.capturedRuns} 次 / ${_status!.capturedMedia} 个产物'),
              if (_status!.lastError != null)
                Text(_status!.lastError!, style: theme.textTheme.labelSmall),
            ],
            const SizedBox(height: 16),
            // Skills：名字就是真源，这里只是后端的只读视图（M5）。
            // **投放口**才是"装 skill"的方式：把文件夹（或 .md）拷进下面这个目录，
            // 应用启动时会自动登记（拷进来的文件没写 frontmatter 也会被补上）。
            // 这里不再有「从 DSH 导入」按钮（用户明确要求去掉）。
            Row(
              children: [
                Text('Skills', style: theme.textTheme.labelLarge),
                const SizedBox(width: 4),
                Text('${store.skills.length}',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
                const Spacer(),
                IconButton(
                  tooltip: '重新扫描投放口（自动登记新拷进来的 skill）',
                  visualDensity: VisualDensity.compact,
                  constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
                  padding: EdgeInsets.zero,
                  onPressed: store.skillsBusy ? null : () => _rescanSkills(),
                  icon: const Icon(Icons.refresh, size: 16),
                ),
              ],
            ),
            _SkillDropIn(store: store, onOpen: () => _openSkillsFolder(), onCopy: () => _copySkillsPath()),
            if (store.skillsBusy) const LinearProgressIndicator(minHeight: 2),
            if (store.skillsError != null)
              Text('Skills 加载失败：${store.skillsError}', style: theme.textTheme.bodySmall),
          ]),
        ),
        if (!store.skillsBusy && store.skills.isEmpty && store.skillsError == null)
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 12),
            sliver: SliverToBoxAdapter(
              child: Text(
                '还没有 Skill。把 skill 文件夹（或一个 .md）拷进上面的目录再点刷新，'
                '也可以让 AI 用 register_skill 注册。',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          )
        else
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
            sliver: SliverList.builder(
              itemCount: store.skills.length,
              itemBuilder: (context, i) => _SkillRow(
                skill: store.skills[i],
                onDelete: () => _deleteSkill(store.skills[i]),
              ),
            ),
          ),
        // 长期记忆（M6）：每次对话都会带上；AI 也能用 remember 追加一条
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 16),
          sliver: SliverToBoxAdapter(
            child: _MemoryPanel(
              store: store,
              onEdit: () => _editMemory(),
            ),
          ),
        ),
      ],
    );
  }

  Future<void> _rescanSkills() async {
    final result = await widget.store.rescanSkills();
    if (!mounted) return;
    if (result.ok && widget.store.skillsError == null) {
      _snack(result.message);
    } else if (widget.store.skillsError != null) {
      _snack('Skills 加载失败：${widget.store.skillsError}', error: true);
    } else {
      _snack(result.message, error: true);
    }
  }

  /// 在资源管理器里打开投放口（打不开就退回"把路径给用户"）。
  Future<void> _openSkillsFolder() async {
    final path = widget.store.skillRoots?.userRoot ?? '';
    if (path.isEmpty) {
      _snack('还没有拿到 skills 目录位置，点一下刷新试试', error: true);
      return;
    }
    final ok = await _openFolder(path);
    if (!ok && mounted) {
      _snack('打不开文件管理器，请手动打开：$path');
    }
  }

  /// 把投放口路径放进剪贴板（打不开资源管理器时的兜底）。
  Future<void> _copySkillsPath() async {
    final path = widget.store.skillRoots?.userRoot ?? '';
    if (path.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: path));
    if (mounted) _snack('已复制 skills 目录：$path');
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
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('删除')),
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
          backgroundColor: error ? Theme.of(context).colorScheme.errorContainer : null,
        ),
      );
  }
}

/// 右侧栏里的一行 Skill：名字 + 一行描述 + 来源徽标 + 警告 + 删除（仅用户来源）。
class _SkillRow extends StatelessWidget {
  final AiSkill skill;
  final VoidCallback onDelete;

  const _SkillRow({required this.skill, required this.onDelete});

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
                color: skill.validationError != null ? theme.colorScheme.error : null,
              ),
            ),
          ),
          const SizedBox(width: 4),
          _Badge(skill.sourceLabel),
          if (!skill.enabled) ...[
            const SizedBox(width: 4),
            _Badge('停用'),
          ],
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
                  Icon(Icons.warning_amber_rounded, size: 13, color: theme.colorScheme.error),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Text(
                      '有问题：${skill.validationError != null ? '格式不合法' : '有冲突'}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall
                          ?.copyWith(color: theme.colorScheme.error),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
      trailing: skill.isBuiltin
          ? Tooltip(
              message: '内置 Skill 只读（可在磁盘上改）',
              child: Icon(Icons.lock_outline, size: 14, color: theme.colorScheme.outline),
            )
          : IconButton(
              tooltip: '删除 Skill',
              icon: const Icon(Icons.delete_outline, size: 16),
              visualDensity: VisualDensity.compact,
              constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
              padding: EdgeInsets.zero,
              onPressed: onDelete,
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
        children: [
          Text(k, style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
          const Spacer(),
          Text(v, style: theme.textTheme.labelSmall),
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
    final color = supported ? theme.colorScheme.primary : theme.colorScheme.outline;
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
          Text(label, style: theme.textTheme.labelSmall?.copyWith(color: color)),
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
/// 用户在建议里要的就是这个：**不要「从 DSH 导入」按钮**，改成一个能拷东西进去的目录。
/// 路径由后端算（两种运行布局都对），界面只负责显示 —— 别在前端拼路径。
class _SkillDropIn extends StatelessWidget {
  final AiWorkspaceStore store;
  final VoidCallback onOpen;
  final VoidCallback onCopy;

  const _SkillDropIn({required this.store, required this.onOpen, required this.onCopy});

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
        Text('把 skill 拷进这个目录即装好（启动/刷新时自动登记）：',
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
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

/// 右侧栏的「长期记忆」：条数 + 第一条预览 + 编辑入口。
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
            Text('${memory.entryCount} 条',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
            const Spacer(),
            if (store.memoryBusy)
              const SizedBox(width: 12, height: 12, child: CircularProgressIndicator(strokeWidth: 2))
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
          '每次对话都会带上它；AI 也能用 remember 追加一条。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
        if (memory.entryCount > 0)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              memory.preview,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodySmall,
            ),
          ),
        if (store.memoryError != null)
          Text('长期记忆不可用：${store.memoryError}', style: theme.textTheme.bodySmall),
      ],
    );
  }
}

/// 长期记忆编辑器：整篇可改、可以加一条、可以清空。
///
/// 真源是 `memory.md`，所以这里就是一个普通的文本框 —— 不做"结构化条目"的花活，
/// 用户手改文件的内容也能原样读回来。
class _MemoryEditorDialog extends StatefulWidget {
  final AiWorkspaceStore store;
  const _MemoryEditorDialog({required this.store});

  @override
  State<_MemoryEditorDialog> createState() => _MemoryEditorDialogState();
}

class _MemoryEditorDialogState extends State<_MemoryEditorDialog> {
  late final TextEditingController _text =
      TextEditingController(text: widget.store.memory.content);
  final TextEditingController _entry = TextEditingController();
  bool _busy = false;

  @override
  void dispose() {
    _text.dispose();
    _entry.dispose();
    super.dispose();
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
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(SnackBar(content: Text(result.message)));
    }
  }

  Future<void> _clear() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('清空长期记忆？'),
        content: const Text('会把 memory.md 里的内容全部删掉，不能撤销。'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('清空')),
        ],
      ),
    );
    if (ok != true) return;
    setState(() => _busy = true);
    final result = await widget.store.clearMemory();
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (result.ok) _text.clear();
    });
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(SnackBar(content: Text(result.message)));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final path = widget.store.memory.path;
    return AlertDialog(
      title: const Text('长期记忆'),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                path.isEmpty
                    ? '一行一条；AI 的 remember 工具会追加到这里。'
                    : '一行一条，真源是 $path（也可以直接改那个文件）。',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
              const SizedBox(height: 8),
              TextField(
                controller: _text,
                minLines: 8,
                maxLines: 14,
                style: theme.textTheme.bodySmall?.copyWith(fontFamily: 'Consolas'),
                decoration: const InputDecoration(
                  border: OutlineInputBorder(),
                  hintText: '- 用户偏好 4:3 画幅\n- 出图统一用 Anima 模型',
                  isDense: true,
                ),
              ),
              const SizedBox(height: 10),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: _entry,
                      decoration: const InputDecoration(
                        labelText: '再加一条',
                        hintText: '例如：交付一律 16:9、带字幕',
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
        TextButton(
          onPressed: _busy ? null : _clear,
          child: const Text('清空'),
        ),
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
        FilledButton(
          onPressed: _busy ? null : () => Navigator.pop(context, _text.text),
          child: const Text('保存'),
        ),
      ],
    );
  }
}

/// 在系统文件管理器里打开一个目录。
///
/// 走 `url_launcher` 的 `file:` 协议（Windows 上落到 ShellExecute）；**打不开就返回 false**,
/// 由调用方把路径显示给用户 —— 不能假装打开了。
Future<bool> _openFolder(String path) async {
  final base = Uri.file(path, windows: true);
  for (final uri in [base, Uri.parse('${base.toString()}/')]) {
    try {
      if (await launchUrl(uri, mode: LaunchMode.externalApplication)) return true;
    } catch (_) {
      // 试下一个形式
    }
  }
  return false;
}
