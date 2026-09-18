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
import '../widgets/app_menu.dart';
import '../widgets/markdown_view.dart';
import '../widgets/workflow_viewer.dart';
import 'media_detail_page.dart';

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

  /// 视口是不是贴着底部（用户建议 ③）：**贴底才自动跟随**，翻旧消息时不打扰。
  bool _atBottom = true;

  /// 上一次的滚动位置：用来判断"用户是不是往上翻了"。
  ///
  /// 为什么不能只看 `maxScrollExtent - pixels`：流式输出时内容一直在变长，
  /// `max` 蹭蹭往上涨而 `pixels` 没动，距离自然就超过阈值了 ——
  /// 那样会把"用户明明还在底部"误判成"用户翻上去了"，自动跟随第一帧就停。
  /// 只有**位置真的变小**（往上翻）才算用户主动离开底部。
  double _lastPixels = 0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_loaded) return;
    _loaded = true;
    _store = context.read<AiWorkspaceStore>();
    _input.addListener(_onInputChanged);
    _scroll.addListener(_onScroll);
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
    // 页面刚挂上来时 `_draftConversationId` 还是空的（要等第一次通知才同步）：
    // 这里就地认领当前会话 —— 否则这段字既不会存进草稿，发送时也不会被清掉，
    // 下一次"取回草稿"就会把它灌回输入框（等于同一条消息发两遍）。
    final id = _draftConversationId ??= _store.conversation?.id;
    if (id == null || !mounted) return;
    _store.saveDraft(id, _input.text);
  }

  /// 会话切换时把上一条的草稿存好、把新一条的草稿取回来。
  void _onStoreChanged() {
    if (!mounted) return;
    _followIfAtBottom();
    final id = _store.conversation?.id;
    if (id == _draftConversationId) return;

    final previous = _draftConversationId;
    _draftConversationId = id;
    if (previous != null) {
      // 附件托盘由 Store 随会话存走（`AiWorkspaceStore._stashAttachments`），
      // 这里只保住输入框里的文字
      _store.saveDraft(previous, _input.text);
      // **真的换了会话**才清空输入框（下面再把新会话的草稿取回来）。
      // 页面刚挂上来的那一次（previous == null）不算切换：用户可能已经打了字，
      // 清空就等于把他的字吃掉。
      _setInputText('');
    } else if (id != null && _input.text.isNotEmpty) {
      // 用户抢在第一次通知之前就打了字：这段字属于当前会话，存下来、别被草稿覆盖
      _store.saveDraft(id, _input.text);
    }
    if (id == null) return;

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
    _scroll.removeListener(_onScroll);
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

  /// 用户手动滚到底 / 翻上去时更新"贴底"状态（用户建议 ③ / ④）。
  void _onScroll() {
    if (!_scroll.hasClients) return;
    final position = _scroll.position;
    final pixels = position.pixels;
    final distance = position.maxScrollExtent - pixels;
    // 往上翻（位置真的变小）才算离开底部；内容变长导致的"距离变大"不算
    final wentUp = pixels < _lastPixels - 2;
    _lastPixels = pixels;
    final next = distance < 80 ? true : (wentUp ? false : _atBottom);
    if (next != _atBottom && mounted) setState(() => _atBottom = next);
  }

  /// 只有"用户本来就在底部"才跟着新内容滚（用户建议 ③）。
  void _followIfAtBottom() {
    if (!_atBottom) return;
    _scrollToBottom();
  }

  /// 回到最新消息（右下角按钮，用户建议 ④）。
  void _jumpToBottom() {
    if (mounted) setState(() => _atBottom = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scroll.hasClients) return;
      _scroll.animateTo(
        _scroll.position.maxScrollExtent,
        duration: const Duration(milliseconds: 220),
        curve: Curves.easeOut,
      );
    });
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
    // 第三栏（Comfy 状态）只在够宽时才铺开。
    //
    // **900~1199px 曾经是个死角**（用户建议 ② / 需求审计 AIH-002）：那个区间
    // `wide` 成立 → 没有窄屏那个 FAB，但 `showPanel` 不成立 → 也没有第三栏，
    // 于是「ComfyUI 状态」整块内容在界面上**一个入口都没有**。
    // 现在宽屏分支在这个区间把入口补到 AppBar 上（见 `onShowStatus`）。
    final showPanel = wide && width >= 1200;

    final conversationList = _ConversationList(store: store);
    // AppBar 显示**当前对话标题**（用户建议 ④）：标题是自动总结出来的，
    // 所以它得一直在视野里，用户才知道自己在哪条对话里、也可以随手改名。
    final thread = Column(
      children: [
        _ConversationAppBar(
          store: store,
          // 只有"宽屏但不够宽到铺第三栏"这一段才需要在标题栏补入口；
          // 更窄时有右下角的 FAB，更宽时有第三栏本身。
          onShowStatus: wide && !showPanel ? () => _showStatusSheet(context, store) : null,
        ),
        Expanded(
          child: Stack(
            children: [
              _MessageList(store: store, controller: _scroll),
              // 回到聊天底部（用户建议 ④）：**只在用户翻上去之后**才出现，
              // 贴在对话主体右下角，不挡输入区。
              if (!_atBottom)
                Positioned(
                  right: 16,
                  bottom: 16,
                  child: Tooltip(
                    message: '回到最新消息',
                    child: FloatingActionButton.small(
                      heroTag: 'ai-jump-bottom',
                      onPressed: _jumpToBottom,
                      child: const Icon(Icons.arrow_downward),
                    ),
                  ),
                ),
            ],
          ),
        ),
        _Composer(
          store: store,
          controller: _input,
          focusNode: _inputFocus,
          onSubmit: (text) async {
            // 空输入 + 没有附件：没什么可发的，静默返回（也别弹提示）。
            if (text.trim().isEmpty && store.attachments.isEmpty) return;

            // **只有"真的会被发出去"才清空输入框**。
            //
            // 准入判断是同步的、和 `send()` 共用同一份（`store.sendBlockReason()`）：
            // 被拒时（最常见的是"还在生成中又按了一次回车"）一个字节都不动 ——
            // 以前是"先清空、再由 send() 拒绝"，用户打的字被静默吃掉，只能自己
            // 把上一条复制一遍再发一次（用户报的 bug）。
            //
            // 清空仍然发生在等待之前：`store.send()` 要一直 await 到整轮结束，
            // 放到后面就会出现"点了发送输入框里的话还在"，看起来像没反应。
            if (store.sendBlockReason() == null) {
              _input.clear();
              // 用户刚发了消息：无论刚才翻到哪儿，都跳回底部看回复
              if (mounted) setState(() => _atBottom = true);
              _scrollToBottom();
            }
            await store.send(text);
            _scrollToBottom();
          },
        ),
      ],
    );

    if (wide) {
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
              // 「选中」和「鼠标按下 / 悬停」必须一眼分得开（用户 bug：次新那条会话
              // 顶着一块亮灰底，看起来比当前会话还像被选中）。
              // Material 默认的**按下高亮**是 ~38% 白，比选中态的存在感强得多；而且窗口在
              // 收到 mouse-up 之前失去焦点 / 被最小化时，这个按下高亮会**卡在屏幕上不动**。
              // 所以：按下 / 水波纹 / 悬停一律压到很淡，选中态改用明确底色（见下面的 selectedTileColor）。
              : Theme(
                  data: theme.copyWith(
                    highlightColor: theme.colorScheme.primary.withValues(alpha: 0.10),
                    splashColor: theme.colorScheme.primary.withValues(alpha: 0.10),
                    hoverColor: theme.colorScheme.onSurface.withValues(alpha: 0.06),
                  ),
                  child: ListView.builder(
                    itemCount: store.conversations.length,
                    itemBuilder: (context, i) {
                      final c = store.conversations[i];
                      final selected = store.conversation?.id == c.id;
                      return ListTile(
                        dense: true,
                        selected: selected,
                        // M3 的 ListTile 选中态**默认只把文字染成主题色、没有底色**，
                        // 于是"鼠标压过的那一行"看着比真选中的还显眼。这里给当前会话一个明确的底色。
                        selectedTileColor: theme.colorScheme.primary.withValues(alpha: 0.16),
                        title: Text(c.title, maxLines: 1, overflow: TextOverflow.ellipsis),
                        subtitle:
                            Text('${c.messageCount} 条消息', style: theme.textTheme.labelSmall),
                        onTap: () {
                          store.openConversation(c.id);
                          if (Scaffold.maybeOf(context)?.isDrawerOpen == true) {
                            Navigator.of(context).pop();
                          }
                        },
                        trailing: AppMenuButton<String>(
                          tooltip: '更多',
                          onSelected: (v) {
                            if (v == 'rename') _rename(context, c);
                            if (v == 'archive') store.archiveConversation(c.id);
                            if (v == 'delete') store.deleteConversation(c.id);
                          },
                          options: const [
                            MenuOption(value: 'rename', icon: Icons.edit_outlined, label: '重命名'),
                            MenuOption(value: 'archive', icon: Icons.archive_outlined, label: '归档'),
                            MenuOption(
                              value: 'delete',
                              icon: Icons.delete_outline,
                              label: '删除',
                              danger: true,
                              dividerBefore: true,
                            ),
                          ],
                          button: (context, controller, isOpen) => IconButton(
                            tooltip: '更多',
                            onPressed: () =>
                                controller.isOpen ? controller.close() : controller.open(),
                            icon: const Icon(Icons.more_vert, size: 18),
                          ),
                        ),
                      );
                    },
                  ),
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
//  AppBar：当前对话标题（用户建议 ④）
// ---------------------------------------------------------------------------

/// 顶部条：显示**当前对话标题**，可以直接改名，也能看到当前模型与消息数。
///
/// 对话标题现在是 AI 在第一次提问时自动总结出来的（用户建议 ③），
/// 用户不接受这个总结时可以在这里（或会话列表的「重命名」里）改掉。
class _ConversationAppBar extends StatelessWidget implements PreferredSizeWidget {
  final AiWorkspaceStore store;

  /// 给「ComfyUI 状态」用的入口（可空）。
  ///
  /// 只在 **900~1199px** 这一段给：那个宽度下第三栏铺不开、又没有窄屏的 FAB，
  /// 不补这个按钮的话 Comfy 状态整块内容在界面上不可达（用户建议 ②）。
  final VoidCallback? onShowStatus;

  const _ConversationAppBar({required this.store, this.onShowStatus});

  @override
  Size get preferredSize => const Size.fromHeight(56);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final conv = store.conversation;
    final title = (conv?.title ?? '').trim();
    final model = store.selectedModel?.displayName ?? store.selectedModel?.id;
    final showTitle = title.isNotEmpty && title != '新对话';

    return AppBar(
      toolbarHeight: 56,
      // 窄屏时左上角是抽屉入口（会话列表）
      automaticallyImplyLeading: MediaQuery.sizeOf(context).width < 900,
      titleSpacing: 8,
      title: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Text(
            showTitle ? title : '新对话',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.titleMedium,
          ),
          Text(
            [
              ?model,
              if (conv != null) '${conv.messageCount} 条消息',
              if (store.sending) '生成中…',
            ].join(' · '),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
          ),
        ],
      ),
      actions: [
        if (onShowStatus != null)
          IconButton(
            tooltip: 'ComfyUI 状态',
            icon: const Icon(Icons.dns_outlined, size: 18),
            onPressed: onShowStatus,
          ),
        if (conv != null)
          IconButton(
            tooltip: '重命名这条对话',
            icon: const Icon(Icons.edit_outlined, size: 18),
            onPressed: () => _renameCurrent(context),
          ),
        IconButton(
          tooltip: '新建对话',
          icon: const Icon(Icons.add_comment_outlined, size: 18),
          onPressed: () => store.newConversation(),
        ),
        const SizedBox(width: 4),
      ],
    );
  }

  Future<void> _renameCurrent(BuildContext context) async {
    final conv = store.conversation;
    if (conv == null) return;
    final controller = TextEditingController(text: conv.title);
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('重命名会话'),
        content: TextField(controller: controller, autofocus: true),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, controller.text),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    if (result != null && result.trim().isNotEmpty) {
      await store.renameConversation(result.trim());
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
            // 思考 / 正文按**流顺序**还原成段（用户要求"如实按流顺序显示"）
            segments: message.isUser ? const [] : store.segmentsFor(message),
            // 这条回复真正产出的产物（回复末尾的「画廊入口卡」）
            producedMediaIds: message.isUser ? const [] : store.producedMediaIds(message),
            // 产出这些图的那份工作流也属于"生成的产物"（用户建议 ①）
            producedPromptIds: message.isUser ? const [] : store.producedPromptIds(message),
            toolCategoryOf: store.toolCategoryOf,
            onApprove: store.approveToolCall,
            onDeny: store.denyToolCall,
            // 用户消息里的附件块：按 id 现取缩略图 / 视频预览帧（M3）
            thumbUrlOf: store.thumbUrlFor,
            fileUrlOf: store.fileUrlFor,
            // 画廊产物（媒体 id）走画廊接口，别再喂给附件接口
            mediaThumbUrlOf: store.mediaThumbUrlFor,
            mediaPosterUrlOf: store.mediaPosterUrlFor,
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

  /// 这条消息按流顺序排好的段：思考段 + 正文段（`reasoning` / `text`）。
  final List<AiMessageSegment> segments;

  /// 这次回复真正产出的画廊产物 id（`comfy_submit` 的 mediaIds）。
  final List<int> producedMediaIds;

  /// 这次回复入库的提示词 id（`comfy_submit` 的 capturedPromptId）。
  ///
  /// 用户建议 ①："「生成的产物」包括生成的工作流" —— 有了它，回复末尾那张卡
  /// 就能直接打开这次真正跑过的工作流（也能拖回 ComfyUI 复现）。
  final List<int> producedPromptIds;

  final String Function(String toolName) toolCategoryOf;
  final Future<void> Function(String callId) onApprove;
  final Future<void> Function(String callId) onDeny;

  /// 附件 id → 缩略图 / 预览帧地址（M3）。
  final String? Function(String? attachmentId) thumbUrlOf;
  final String? Function(String? attachmentId) fileUrlOf;

  /// 画廊产物（媒体 id）→ 缩略图 / 视频封面地址。
  ///
  /// 与上面那两条**必须分开**：附件接口的 id 空间和画廊 media id 完全不同。
  final String Function(int mediaId) mediaThumbUrlOf;
  final String Function(int mediaId) mediaPosterUrlOf;

  const _MessageBubble({
    required this.message,
    required this.sending,
    required this.onRetry,
    this.toolCalls = const [],
    this.segments = const [],
    this.producedMediaIds = const [],
    this.producedPromptIds = const [],
    required this.toolCategoryOf,
    required this.onApprove,
    required this.onDeny,
    required this.thumbUrlOf,
    required this.fileUrlOf,
    required this.mediaThumbUrlOf,
    required this.mediaPosterUrlOf,
  });

  /// 气泡正文的有序块：思考 / 正文 / 工具卡，顺序 = `parts` 的顺序（= 消息获取顺序）。
  ///
  /// 用户明确要求"完全按照消息获取顺序来，不要将最终回复置于最终思考的前面"：
  /// 早先这里是"所有思考与正文段先铺完、工具卡统统一坨挂在最下面"，
  /// 一轮回复里工具卡就跑到最终正文后面去了 —— 那也是重排。
  ///
  /// `parts` 还没到的流式阶段退回 `segments`（只有思考与正文），
  /// 实时累积的工具卡追加在末尾（此刻它们确实是最新的）。
  List<Widget> _orderedBody(ThemeData theme) {
    final m = message;
    final streaming = m.status == 'streaming';
    final blocks = <Widget>[];
    final placed = <String>{};

    void addSeg(String type, String text) {
      if (text.isEmpty) return;
      if (type == 'reasoning') {
        blocks.add(_ReasoningPanel(text: text, streaming: streaming));
      } else if (type == 'text' && text.trim().isNotEmpty) {
        // 用户消息是**纯文本**，不走 Markdown（用户打什么就显示什么）
        blocks.add(m.isUser
            ? Text(text)
            : MarkdownText(text, style: theme.textTheme.bodyMedium, selectable: false));
      }
    }

    if (m.parts.isNotEmpty) {
      // 相邻同类块合并成一段（免得一个气泡里冒出十几个碎段），但**不跨工具卡合并**
      var type = '';
      var buffer = StringBuffer();
      void flush() {
        if (buffer.isEmpty) {
          type = '';
          return;
        }
        addSeg(type, buffer.toString());
        buffer = StringBuffer();
        type = '';
      }

      for (final part in m.parts) {
        if (part.type == 'reasoning' || part.type == 'text') {
          if (part.type != type) {
            flush();
            type = part.type;
          }
          buffer.write(part.text ?? '');
          continue;
        }
        if (part.type != 'tool_call') continue;
        flush();
        final call = toolCalls.where((c) => c.callId == part.toolCallId).firstOrNull;
        if (call == null) continue;
        placed.add(call.callId);
        blocks.add(_toolCard(call));
      }
      flush();
    } else {
      for (final seg in segments) {
        addSeg(seg.type, seg.text);
      }
    }

    // 还没归位的工具卡（流式过程中 parts 尚未到达 / parts 里没记的）追加在末尾
    for (final call in toolCalls) {
      if (placed.contains(call.callId)) continue;
      blocks.add(_toolCard(call));
    }
    return blocks;
  }

  Widget _toolCard(AiToolCallState call) => Padding(
        padding: const EdgeInsets.only(top: 8),
        child: _ToolCallCard(
          key: ValueKey('${message.id}-${call.callId}'),
          call: call,
          category: toolCategoryOf(call.name),
          onApprove: onApprove,
          onDeny: onDeny,
        ),
      );

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
          // 整条气泡一个选择域（用户 bug ②："仅能选择 3 行文本"）：
          // 一段回复被解析成很多个块（段落 / 标题 / 列表 / 表格 / 代码块），
          // 每块一个 SelectableText 的话，鼠标拖到当前块末尾就拉不动了。
          // 包一层 SelectionArea、内部一律用 Text，选择就能跨段连续拉下去。
          child: SelectionArea(
            child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // 思考 / 正文 / 工具卡**按 parts 的真实顺序**交错渲染
              // （用户要求：完全按照消息获取顺序来，不重排）。
              // 后端给的 parts 就是权威的有序块；`segments` 是它的降级版本（只有思考与正文段），
              // 流式过程中 parts 还没到，就用它 + 末尾追加还没归位的工具卡。
              ..._orderedBody(theme),
              // 附件块（只出现在用户轮）
              for (final part in m.parts)
                if (part.type == 'attachment')
                  Padding(
                    padding: const EdgeInsets.only(bottom: 4),
                    child: _MessageAttachment(
                      name: part.text ?? part.attachmentId ?? '附件',
                      thumbUrl: thumbUrlOf(part.attachmentId),
                      fileUrl: fileUrlOf(part.attachmentId),
                    ),
                  ),
              // 没有段可用时（老数据 / 只有纯文本）退回整段正文，
              // 用户消息本来就是纯文本，不走 Markdown。
              if (segments.isEmpty && m.text.isNotEmpty)
                m.isUser
                    ? Text(m.text)
                    : MarkdownText(m.text, style: theme.textTheme.bodyMedium, selectable: false),
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
              // 工具卡已经在 [_orderedBody] 里按 parts 的真实顺序插好了 ——
              // 这里**不能再统一补一坨**，否则又变回"工具卡全排到最后"。
              if (pending.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(
                    '有 ${pending.length} 个工具调用在等你的批准（写盘 / 改动类操作）。',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                  ),
                ),
              // 画廊入口卡（用户建议 ⑤）：这次回复**真的生成了产物**时，
              // 在末尾贴一个能点开看详情的入口，而不是让用户自己去画廊里翻。
              // 用户建议 ①：这张卡里也包含"生成它的工作流"（一次运行出来的图 + 那份工作流
              // 本来就是同一批产物），所以多一个「查看工作流」按钮。
              if (producedMediaIds.isNotEmpty || producedPromptIds.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: _ProducedMediaCard(
                    mediaIds: producedMediaIds,
                    promptIds: producedPromptIds,
                    mediaThumbUrlOf: mediaThumbUrlOf,
                    mediaPosterUrlOf: mediaPosterUrlOf,
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
        // 长期记忆（M6）：记住一条事实用的是「便签」这个意象
        'memory' => Icons.push_pin_outlined,
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
                Tooltip(
                  // 带工具名的 tooltip：用例与用户都能精确定位"展开哪一个工具的结果"
                  message: '${_expanded ? '收起' : '展开'} ${call.name} 的结果',
                  child: InkWell(
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
                        Text(call.arguments, style: theme.textTheme.bodySmall),
                        if (call.detailText.isNotEmpty) const SizedBox(height: 6),
                      ],
                      if (call.detailText.isNotEmpty) ...[
                        Text(
                          call.status == AiToolCallStatus.ok ? '结果' : '说明',
                          style: theme.textTheme.labelSmall,
                        ),
                        Text(call.detailText, style: theme.textTheme.bodySmall),
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

/// 回复末尾的**画廊入口卡**（用户建议 ⑤）：
/// "如果让 AI 调用 comfy 生成了产物，可以在回复末尾贴上一个类似画廊项目的入口，点开即查看对应详情"。
///
/// 只显示**后端确认入库**的产物（媒体 id 来自工具的结构化结果），
/// 所以这里不会出现"AI 说生成了但其实没有"的假入口。
///
/// 缩略图走**画廊**的 `/api/media/{id}/thumb`（视频退到 `/poster`）：
/// 早先这里复用了附件那两条接口（`thumbUrlOf(id)`），id 空间不同，必然 404 ——
/// 用户看到的就是"生成产物后预览图不可用"。
class _ProducedMediaCard extends StatelessWidget {
  final List<int> mediaIds;

  /// 产出这些图的那份提示词 / 工作流（用户建议 ①）。
  final List<int> promptIds;

  final String Function(int mediaId) mediaThumbUrlOf;
  final String Function(int mediaId) mediaPosterUrlOf;

  const _ProducedMediaCard({
    required this.mediaIds,
    this.promptIds = const [],
    required this.mediaThumbUrlOf,
    required this.mediaPosterUrlOf,
  });

  /// 标题：图 + 工作流算同一批产物（用户建议 ①），有几个就分别说清楚。
  ///
  /// 只有图时保持原来的写法，别为了多一个维度把常见情况的标题变啰嗦。
  String get _title {
    if (promptIds.isEmpty) return '生成的产物（${mediaIds.length}）';
    if (mediaIds.isEmpty) return '生成的产物（工作流 ${promptIds.length} 份）';
    return '生成的产物（${mediaIds.length} 个 · 含 ${promptIds.length} 份工作流）';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final shown = mediaIds.take(6).toList();
    return Container(
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.45),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: theme.dividerColor),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                mediaIds.isEmpty ? Icons.account_tree_outlined : Icons.photo_library_outlined,
                size: 15,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(width: 6),
              Text(_title, style: theme.textTheme.labelMedium),
            ],
          ),
          if (shown.isNotEmpty) ...[
            const SizedBox(height: 8),
            // 横向缩略图条：懒构建，产物多的时候也不会一次全解码
            SizedBox(
              height: 72,
              child: ListView.builder(
                scrollDirection: Axis.horizontal,
                itemCount: shown.length,
                itemBuilder: (context, i) {
                  final id = shown[i];
                  return Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: InkWell(
                      onTap: () => _openDetail(context, id),
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(8),
                        child: SizedBox(
                          width: 72,
                          height: 72,
                          child: Image.network(
                            mediaThumbUrlOf(id),
                            fit: BoxFit.cover,
                            // 图片有 thumb；视频的 thumb 会回 204，退到后端抽的第一帧封面
                            errorBuilder: (_, _, _) => Image.network(
                              mediaPosterUrlOf(id),
                              fit: BoxFit.cover,
                              errorBuilder: (_, _, _) => Container(
                                color: theme.colorScheme.surfaceContainerHighest,
                                child: Icon(Icons.image_not_supported_outlined,
                                    size: 18, color: theme.colorScheme.outline),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
            ),
          ],
          const SizedBox(height: 6),
          Wrap(
            spacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              if (mediaIds.isNotEmpty)
                TextButton.icon(
                  onPressed: () => _openDetail(context, mediaIds.first),
                  icon: const Icon(Icons.open_in_new, size: 15),
                  label: const Text('查看详情'),
                  style: _buttonStyle,
                ),
              if (promptIds.isNotEmpty)
                // 「生成的产物」包括生成的工作流（用户建议 ①）：
                // 直接打开这次真正跑过的那份工作流，而不是让用户去画廊里找
                TextButton.icon(
                  onPressed: () => _openWorkflow(context),
                  icon: const Icon(Icons.account_tree_outlined, size: 15),
                  label: const Text('查看工作流'),
                  style: _buttonStyle,
                ),
              if (mediaIds.length > 1)
                Text(
                  '点缩略图可以逐个看',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
            ],
          ),
        ],
      ),
    );
  }

  static final ButtonStyle _buttonStyle = TextButton.styleFrom(
    padding: const EdgeInsets.symmetric(horizontal: 8),
    minimumSize: const Size(0, 30),
    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
  );

  void _openDetail(BuildContext context, int mediaId) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => MediaDetailPage(mediaId: mediaId)),
    );
  }

  /// 打开"生成这些图的那份工作流"。
  ///
  /// 用 `AiApiClient`（而不是 `ApiClient(base)` 现造一个）是为了**走注入的 http client** ——
  /// 仓库里现造客户端的写法在 widget 测试里会真的发 HTTP（一定 400），这条路径就测不到了。
  void _openWorkflow(BuildContext context) {
    final store = context.read<AiWorkspaceStore>();
    showWorkflowDialog(
      context,
      title: '本次生成的工作流（提示词 #${promptIds.first}）',
      load: () => store.promptWorkflow(promptIds.first),
    );
  }
}

/// 思考过程：**默认折叠**（用户要求："思考过程默认自动折叠"）。
///
/// 收起时**不是简单藏起来** —— 显示模型思考的摘要（首段，压成一行），
/// 这样一眼能看出"它想了什么"；展开后给完整正文，可滚动、可选中复制。
/// 流式生成中也**不自动展开**：思考是过程，正文才是结果，
/// 一屏里几条长思考会把真正的回答顶到看不见的地方；想看的点一下就展开。
class _ReasoningPanel extends StatefulWidget {
  final String text;

  /// 这一轮还在生成：标题栏上显示"思考中…"。
  final bool streaming;

  const _ReasoningPanel({required this.text, this.streaming = false});

  @override
  State<_ReasoningPanel> createState() => _ReasoningPanelState();
}

class _ReasoningPanelState extends State<_ReasoningPanel> {
  /// 默认收起。**组件重建（切会话 / 刷新消息）后依然收起**：
  /// 状态只活在这一个 State 里，不写全局偏好，折叠与否本来就是"临时看一眼"的动作。
  bool _expanded = false;

  /// 收起时的摘要：压掉换行、取开头一段，末尾还有内容就加省略号。
  static String summaryOf(String text, {int limit = 160}) {
    final flat = text.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (flat.length <= limit) return flat;
    return '${flat.substring(0, limit)}…';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline);
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
                Text('思考过程', style: label),
                if (widget.streaming) ...[
                  const SizedBox(width: 6),
                  // 这里**不用** CircularProgressIndicator：无限动画会让
                  // `pumpAndSettle` 永远等下去，而且一屏好几个转圈也很吵。
                  Text('思考中…', style: label),
                ],
              ],
            ),
          ),
          if (_expanded)
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 320),
              child: SingleChildScrollView(
                // 收起时**整段不建**（不是 Opacity/Offstage）：长思考的文本量不小，
                // 折叠状态不该还把它留在 widget 树里。
                child: _SelectableReasoning(text: widget.text),
              ),
            )
          else if (widget.text.trim().isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(left: 20, top: 2),
              child: Tooltip(
                message: '点击展开完整思考过程',
                child: Text(
                  summaryOf(widget.text),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline.withValues(alpha: 0.85),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// 展开后的完整思考正文：可选中复制（长度不受摘要限制）。
///
/// 单独包一层是为了让"折叠时完全不构建它"这个行为一眼可见（见 `_ReasoningPanel`）。
///
/// 用普通 `Text` 而不是 `SelectableText`：思考与正文同在一个 `SelectionArea` 里，
/// 内层再开一个选择域的话，拖选到思考段末尾就拉不到下面的正文了（用户 bug ②）。
class _SelectableReasoning extends StatelessWidget {
  final String text;
  const _SelectableReasoning({required this.text});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Text(
      text,
      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
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
  /// 中文输入法正在组字（拼音还没上屏）。
  ///
  /// 以前这里是个**永远是 false** 的布尔字段（只在 `onChanged` 里被赋 false），
  /// 等于没有这道闸：组字期间按回车会把半截拼音当消息发出去，并且顺手清空输入框 ——
  /// 在 Windows 上"清空时输入法还在组字"正是引擎把旧文本回灌/复制的触发条件
  /// （flutter/flutter#191196）。现在直接读控制器里的 composing range，是真的。
  bool get _composing => widget.controller.value.composing.isValid;

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
    // 组字状态也参与"发送按钮可不可用"，所以它一变就得重建（见 build 里的 `composing`）
    final composing = _composing;
    if (next != _slashQuery || composing != _wasComposing) {
      setState(() {
        _slashQuery = next;
        _wasComposing = composing;
      });
    }
  }

  /// 上一次重建时的组字状态：只在**变化**时 setState，免得每次按键都重建。
  bool _wasComposing = false;

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
          if (store.attachments.isNotEmpty || store.uploadingAttachments)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (var i = 0; i < store.attachments.length; i++)
                    _AttachmentTile(
                      key: ValueKey('attach-${store.attachments[i].id ?? i}'),
                      attachment: store.attachments[i],
                      // 图片给缩略图、视频给预览帧（后端同一张 /thumb 接口，抽不出来回 204）
                      thumbUrl: store.thumbUrlFor(store.attachments[i].id),
                      fileUrl: store.fileUrlFor(store.attachments[i].id),
                      blockers: store.preflight?.blockersAt(i) ?? const [],
                      onRemove: () => store.removeAttachmentAt(i),
                    ),
                  if (store.uploadingAttachments)
                    const SizedBox(
                      width: 92,
                      height: 92,
                      child: Center(
                        child: SizedBox(
                          width: 20,
                          height: 20,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                      ),
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
                    // `/` 菜单开着时回车不发送（否则会把半截 skill 名发出去）
                    if (_slashMatches.isNotEmpty) return null;
                    // 组字期间的判断在 `_submit()` 里（按钮也走同一条路）
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
                decoration: const InputDecoration(
                  hintText: '描述你的生图 / 生视频需求，或输入 / 调用 Skill（Enter 发送，Shift+Enter 换行）',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          // 底行排版（用户 bug：token 汇总被挤没 + 发送按钮不贴右）。两个症状同一个根因：
          //   ① 汇总当时是 `Flexible`（loose）—— 它用不满自己那份空间时，多出来的空白落在
          //      **发送按钮右边**，按钮就不贴右了；
          //   ② 它和 `Spacer` 各占 1 flex —— 剩余空间对半分，窗口一窄就先被省略成"本对话…"。
          // 现在的规矩：左侧操作组是**唯一的弹性位**（Expanded，内部横向可滚动：窄屏时自己滚，
          // 不把右边顶出去），汇总固定占位（≤220，只在自己太长时才省略），发送放最后且不参与压缩。
          Row(
            children: [
              Expanded(
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        tooltip: '添加附件',
                        onPressed: _pickFiles,
                        icon: const Icon(Icons.attach_file),
                      ),
                      // 权限档（用户建议 ⑤）：就在附件按钮和模型选择之间
                      _PermissionPicker(store: store),
                      _ModelPicker(store: store),
                      const SizedBox(width: 8),
                      _EffortPicker(store: store),
                      const SizedBox(width: 8),
                    ],
                  ),
                ),
              ),
              // 本次对话的 token 汇总（AIH-057）：一直是可见的，不必翻设置。
              // 固定占位（不吃 flex），所以它只会因为**自己太长**而省略，不会被别人挤掉。
              if (!store.usageSummary.isEmpty) ...[
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 220),
                  child: Tooltip(
                    message: store.usageSummary.label,
                    child: Text(
                      '本对话 ↑${AiTokenUsage.compact(store.usageSummary.inputTokens)} '
                      '↓${AiTokenUsage.compact(store.usageSummary.outputTokens)}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      textAlign: TextAlign.right,
                      style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
              ],
              FilledButton.icon(
                // 组字（拼音还没上屏）期间不给发：这时候发出去的是半截拼音，
                // 而且"清空输入框"正好踩在 Windows 输入法回灌旧文本的触发点上。
                onPressed: store.sending
                    ? () => store.stop()
                    : (store.canSend && !_wasComposing ? _submit : null),
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
    // 组字（拼音还没上屏）期间的回车是"选词 / 上屏"，不是发送（AIH-053）；
    // 按钮走同一条路，所以闸放在这里，两条入口都拦得住。
    if (_composing) return;
    final text = widget.controller.text;
    widget.onSubmit(text);
  }

  Future<void> _pickFiles() async {
    // file_picker 12.x 起 pickFiles 是静态方法，直接返回 List<PlatformFile>
    final files = await FilePicker.pickFiles(dialogTitle: '选择要发给 AI 的附件');
    if (files.isEmpty) return;
    // 上传走"路径"：不让几十 MB 的图片进 Dart 堆；类型判定、缩略图与准入都在后端做（M3）
    final picked = <({String name, String path})>[];
    final missing = <String>[];
    for (final f in files) {
      final path = f.path;
      if (path == null || path.isEmpty) {
        missing.add(f.name);
      } else {
        picked.add((name: f.name, path: path));
      }
    }
    if (missing.isNotEmpty) {
      widget.store.noteAttachmentsUnreadable(missing);
    }
    await widget.store.attachFiles(picked);
  }
}

class _SendIntent extends Intent {
  const _SendIntent();
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
//  权限档（用户建议 ⑤）
// ---------------------------------------------------------------------------

/// 权限两档：**询问**（默认）/ **自动允许（无需批准）**。
///
/// 位置就是用户指定的：输入区底部、附件按钮与模型选择之间。
/// 真源在后端（`ai.tools.policy.permissionMode`），切档写回后端 ——
/// 后端每次 Run 现读策略，于是"切到自动允许"会**即时**把那段规则注入系统提示，
/// 并且 AI 调工具不再走审批闸门。路径白名单不受影响，越界写照样被拒。
///
/// 名字来自用户建议：「完全权限」听着像"什么都能干"，其实它只免掉"问一下"，
/// 文件夹白名单一点都没放宽 —— 所以改叫「自动允许（无需批准）」。
class _PermissionPicker extends StatelessWidget {
  final AiWorkspaceStore store;
  const _PermissionPicker({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final full = store.fullPermission;
    final color = full ? theme.colorScheme.error : theme.colorScheme.outline;
    final label = full ? AiToolPolicy.modeFullLabel : AiToolPolicy.modeAskLabel;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: AppMenuButton<String>(
        tooltip: full
            ? '当前：$label —— AI 调用工具不会等你批准（可读 / 可写目录范围不变，越界仍会被拒）。点一下改回「${AiToolPolicy.modeAskLabel}」。'
            : '当前：$label —— 写盘 / 提交任务这类改动操作会先弹批准。点一下切到「${AiToolPolicy.modeFullLabel}」。',
        onSelected: store.setPermissionMode,
        options: [
          MenuOption(
            value: AiToolPolicy.modeAsk,
            icon: Icons.how_to_reg_outlined,
            label: AiToolPolicy.modeAskLabel,
            subtitle: '改动类工具先等你批准',
            trailingIcon: full ? null : Icons.check,
          ),
          MenuOption(
            value: AiToolPolicy.modeFull,
            icon: Icons.gpp_maybe_outlined,
            label: AiToolPolicy.modeFullLabel,
            subtitle: 'AI 无需批准，直接动手（目录白名单不变）',
            trailingIcon: full ? Icons.check : null,
          ),
        ],
        // 两种档位的选择放在菜单里：按钮本身体积小，不会把输入区挤爆
        button: (context, controller, isOpen) => Opacity(
          opacity: store.permissionBusy ? 0.5 : 1,
          child: InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: store.permissionBusy
                ? null
                : () => controller.isOpen ? controller.close() : controller.open(),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              decoration: BoxDecoration(
                border: Border.all(color: full ? color : theme.dividerColor),
                borderRadius: BorderRadius.circular(8),
                color: full ? color.withValues(alpha: 0.08) : null,
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    full ? Icons.gpp_maybe_outlined : Icons.how_to_reg_outlined,
                    size: 16,
                    color: color,
                  ),
                  const SizedBox(width: 6),
                  Text(label,
                      style: theme.textTheme.labelMedium?.copyWith(color: full ? color : null)),
                  Icon(Icons.arrow_drop_down, size: 18, color: theme.colorScheme.outline),
                ],
              ),
            ),
          ),
        ),
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
            // 模型名可能很长（内置目录里就有 "…-preview-2026-01" 这种）：
            // 输入区底部这一行还要塞下附件 / 权限 / 思考强度 / token 汇总 / 发送，
            // 所以这里**必须**能压缩，不能把整行撑溢出。
            Flexible(
              child: Text(
                model?.displayName ?? '未选择模型',
                style: theme.textTheme.labelMedium,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
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
/// 只显示**当前模型声明过**的档位；**「关闭」例外** —— 它不发任何思考参数、
/// 任何网关都成立，所以永远可选（用户要求"能关掉思考，且不用去改模型声明"）。
/// 模型完全没声明推理能力时整块置灰并说明原因，而不是给一堆选了会被后端拒绝的选项。
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

    return AppMenuButton<AiReasoningEffort>(
      tooltip: enabled
          ? '思考强度（「关闭」始终可选；模型声明的档位：'
              '${options.where((e) => e.isThinking).map((e) => e.label).join(' / ')}）'
          : '该模型未声明推理能力，请到「设置 → AI 模型」声明',
      onSelected: store.selectReasoningEffort,
      options: [
        for (final e in options)
          MenuOption(
            value: e,
            label: e.label,
            icon: e == store.reasoningEffort
                ? Icons.radio_button_checked
                : Icons.radio_button_off,
            // 线上表达（例如 "→ high"）放第二行，别跟标题挤在一行
            subtitle: model?.effortWireValue(e) == null ? null : '→ ${model!.effortWireValue(e)}',
          ),
      ],
      button: (context, controller, isOpen) => Opacity(
        opacity: enabled ? 1 : 0.6,
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: enabled
              ? () => controller.isOpen ? controller.close() : controller.open()
              : null,
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
      ),
    );
  }
}

/// 附件托盘里的一格：**图片显示缩略图、视频显示预览帧**（M3）。
///
/// 缩略图 / 预览帧都来自后端的 `GET /api/ai/attachments/{id}/thumb`：
/// 图片是 JPEG 缩略图，视频是 Windows 缩略图管线抽的第一帧。抽不出来（或音频 / 文档）
/// 后端回 204，这里退化成文件图标 —— 不是破图。
///
/// 与画廊同一条性能规矩：**按实际绘制像素解码**（`cacheWidth`），
/// 否则一张 4096² 的原图会以全尺寸进 ImageCache。
class _AttachmentTile extends StatelessWidget {
  final AiAttachment attachment;
  final String? thumbUrl;
  final String? fileUrl;

  /// 这张附件在当前模型下的准入问题（非空 = 打红框并说明）
  final List<String> blockers;
  final VoidCallback onRemove;

  static const double _size = 92;

  const _AttachmentTile({
    super.key,
    required this.attachment,
    required this.thumbUrl,
    required this.fileUrl,
    required this.blockers,
    required this.onRemove,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final blocked = blockers.isNotEmpty;
    final borderColor = blocked ? theme.colorScheme.error : theme.dividerColor;

    return Tooltip(
      message: blocked
          ? '这张附件发不出去：\n${blockers.map((b) => '· $b').join('\n')}'
          : '${attachment.name} · ${attachment.sizeLabel}${attachment.isVideo ? '（已取预览帧）' : ''}'
              '\n点击在新窗口打开',
      child: SizedBox(
        width: _size,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: _size,
              height: _size,
              child: Stack(
                children: [
                  Positioned.fill(
                    child: InkWell(
                      onTap: fileUrl == null ? null : () => _openAttachment(fileUrl!),
                      child: Container(
                        clipBehavior: Clip.antiAlias,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(8),
                          border: Border.all(
                            color: borderColor,
                            width: blocked ? 2 : 1,
                          ),
                        ),
                        child: _preview(context),
                      ),
                    ),
                  ),
                  // 视频：盖一个播放角标，一眼看出这是"预览帧"而不是一张图片
                  if (attachment.isVideo && thumbUrl != null)
                    const Positioned.fill(
                      child: IgnorePointer(
                        child: Center(
                          child: Icon(Icons.play_circle_fill, size: 28, color: Colors.white70),
                        ),
                      ),
                    ),
                  if (blocked)
                    const Positioned(
                      left: 3,
                      bottom: 3,
                      child: Icon(Icons.block, size: 15, color: Colors.white),
                    ),
                  Positioned(
                    top: -6,
                    right: -6,
                    child: IconButton(
                      tooltip: '移除附件',
                      iconSize: 15,
                      visualDensity: VisualDensity.compact,
                      padding: EdgeInsets.zero,
                      constraints: const BoxConstraints.tightFor(width: 24, height: 24),
                      onPressed: onRemove,
                      icon: const Icon(Icons.cancel),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 2),
            Text(
              attachment.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall,
            ),
            Text(
              attachment.sizeLabel,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ],
        ),
      ),
    );
  }

  Widget _preview(BuildContext context) {
    if (thumbUrl == null) return _iconFallback(context);
    final dpr = MediaQuery.devicePixelRatioOf(context);
    return Image.network(
      thumbUrl!,
      width: _size,
      height: _size,
      fit: BoxFit.cover,
      // 与画廊缩略图同一条规矩：解码宽度跟着绘制尺寸 × DPR，上限 512（见 media_thumb.dart）
      cacheWidth: (_size * dpr).round().clamp(1, 512),
      // 缩略图用不上 mipmap（medium 会为每张纹理生成 mipmap）
      filterQuality: FilterQuality.low,
      errorBuilder: (_, _, _) => _iconFallback(context),
      loadingBuilder: (context, child, progress) =>
          progress == null ? child : _iconFallback(context, dim: true),
    );
  }

  Widget _iconFallback(BuildContext context, {bool dim = false}) {
    final theme = Theme.of(context);
    return Container(
      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: dim ? 0.3 : 0.6),
      child: Center(
        child: Icon(
          attachmentIcon(attachment.modality),
          size: 28,
          color: theme.colorScheme.outline,
        ),
      ),
    );
  }
}

/// 消息气泡里的附件：只读的小缩略图（用户消息发出去之后回头看得到自己发了什么）。
class _MessageAttachment extends StatelessWidget {
  final String name;
  final String? thumbUrl;
  final String? fileUrl;

  const _MessageAttachment({required this.name, required this.thumbUrl, required this.fileUrl});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Tooltip(
      message: '$name\n点击在新窗口打开',
      child: InkWell(
        onTap: fileUrl == null ? null : () => _openAttachment(fileUrl!),
        child: Container(
          width: 96,
          height: 96,
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(8),
            border: Border.all(color: theme.dividerColor),
          ),
          child: thumbUrl == null
              ? Center(child: Icon(Icons.attach_file, size: 22, color: theme.colorScheme.outline))
              : Image.network(
                  thumbUrl!,
                  fit: BoxFit.cover,
                  cacheWidth: (96 * MediaQuery.devicePixelRatioOf(context)).round().clamp(1, 512),
                  filterQuality: FilterQuality.low,
                  errorBuilder: (_, _, _) =>
                      Center(child: Icon(Icons.attach_file, size: 22, color: theme.colorScheme.outline)),
                ),
        ),
      ),
    );
  }
}

/// 附件模态 → 图标（音频 / 文档 / 未知类型没有缩略图可看）。
IconData attachmentIcon(String? modality) => switch (modality) {
      'image' => Icons.image_outlined,
      'video' => Icons.movie_outlined,
      'audio' => Icons.audiotrack_outlined,
      'document' => Icons.description_outlined,
      'text' => Icons.article_outlined,
      _ => Icons.attach_file,
    };

/// 用系统默认程序打开附件原件（Windows 上落到 ShellExecute）。
Future<void> _openAttachment(String url) async {
  try {
    await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication);
  } catch (_) {
    // 打不开就什么都不做：URL 本身在 tooltip 里能看到
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

/// 右侧栏顶部的**实时进度**（用户"其他建议"第 1 条）。
///
/// 显示两件事：
///  - ComfyUI 队列（运行中 / 等待中 + 正在跑的那个任务名）；
///  - AI / 用户最近提交的任务（状态、耗时、产物数）。
///
/// 没有活动时**整块不占位** —— 右侧栏本来就窄，闲着的时候不该被一张空卡片吃掉高度。
class _ComfyProgress extends StatelessWidget {
  final AiWorkspaceStore store;
  const _ComfyProgress({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final jobs = store.comfyJobs;
    final active = jobs.submissions.where((s) => s.isActive).toList();
    final recent = jobs.submissions.where((s) => !s.isActive).take(2).toList();
    // 队列里没人、也没有最近提交过（或最近只是"不可达"）：不显示这一块
    if (!jobs.hasActivity && recent.isEmpty && jobs.comfyReachable) {
      return const SizedBox.shrink();
    }

    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(
                    jobs.hasActivity ? Icons.play_circle_outline : Icons.dns_outlined,
                    size: 16,
                    color: jobs.hasActivity ? theme.colorScheme.primary : theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 6),
                  Text('实时进度', style: theme.textTheme.labelLarge),
                  const Spacer(),
                  if (jobs.queuePending > 0)
                    Text('队列 ${jobs.queuePending}',
                        style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
                ],
              ),
              const SizedBox(height: 6),
              if (!jobs.comfyReachable)
                Text(
                  'ComfyUI 不可达：进度拿不到（它没在跑？）',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                )
              else if (jobs.queueRunning > 0)
                Text(
                  '正在生成：${jobs.runningLabel ?? '未知工作流'}'
                  '${jobs.queuePending > 0 ? '（还有 ${jobs.queuePending} 个排队）' : ''}',
                  style: theme.textTheme.bodySmall,
                ),
              for (final s in active)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: _SubmissionRow(submission: s, highlight: true),
                ),
              if (active.isEmpty && jobs.queueRunning == 0 && jobs.comfyReachable)
                Text('当前没有正在生成的任务',
                    style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
              if (recent.isNotEmpty) ...[
                const Divider(height: 14),
                Text('最近提交', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
                for (final s in recent)
                  Padding(
                    padding: const EdgeInsets.only(top: 3),
                    child: _SubmissionRow(submission: s, highlight: false),
                  ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _SubmissionRow extends StatelessWidget {
  final AiComfySubmission submission;
  final bool highlight;
  const _SubmissionRow({required this.submission, required this.highlight});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = submission.isFailed
        ? theme.colorScheme.error
        : (highlight ? theme.colorScheme.primary : theme.colorScheme.outline);
    return Row(
      children: [
        Expanded(
          child: Text(
            submission.label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(color: color),
          ),
        ),
        const SizedBox(width: 6),
        Text(
          [
            submission.statusLabel,
            if (submission.elapsedLabel.isNotEmpty) submission.elapsedLabel,
            if (submission.mediaIds.isNotEmpty) '${submission.mediaIds.length} 个产物',
          ].join(' · '),
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
      ],
    );
  }
}

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

  /// Skills 列表默认**折叠**（用户"其他建议"第 2 条）：装了几十个之后右侧栏放不下，
  /// 展开后给搜索框，找 skill 靠搜索而不是往下翻。
  bool _skillsOpen = false;
  final _skillSearch = TextEditingController();

  /// 折叠状态下的搜索结果：名称、描述、whenToUse 里任一命中就算。
  List<AiSkill> _filteredSkills(AiWorkspaceStore store) {
    final q = _skillSearch.text.trim().toLowerCase();
    if (q.isEmpty) return store.skills;
    return store.skills
        .where((s) =>
            s.name.toLowerCase().contains(q) ||
            (s.description).toLowerCase().contains(q) ||
            (s.whenToUse ?? '').toLowerCase().contains(q))
        .toList();
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _refresh();
      // 实时进度（用户"其他建议"第 1 条）：右侧栏一显示就开始轮询，
      // 这样"AI 刚提交的任务 / 用户自己在 ComfyUI 里点的任务"都能看到进度。
      widget.store.startWatchingComfy();
    });
  }

  @override
  void dispose() {
    widget.store.stopWatchingComfy();
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

    // CustomScrollView + SliverList：固定几段用 SliverToBoxAdapter，
    // skills 可能有几十个，必须懒构建（`ListView(children: [...])` 会一次全建）。
    return CustomScrollView(
      slivers: [
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 0),
          sliver: SliverList.list(children: [
            // 实时进度（用户"其他建议"第 1 条）：AI 提交的任务 / ComfyUI 队列就显示在最上面，
            // 用户在对话里说"帮我跑一张"之后，不用切到 ComfyUI 就能看到它跑到哪了。
            _ComfyProgress(store: store),
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
            //
            // 用户"其他建议"第 2 条：**列表默认折叠**（装了十几个 skill 之后，
            // 右侧栏会被它们占满），展开后才给搜索框 —— 找某个 skill 靠搜索，不靠翻。
            Row(
              children: [
                InkWell(
                  onTap: () => setState(() => _skillsOpen = !_skillsOpen),
                  child: Row(
                    children: [
                      Icon(_skillsOpen ? Icons.expand_less : Icons.expand_more,
                          size: 16, color: theme.colorScheme.outline),
                      Text('Skills', style: theme.textTheme.labelLarge),
                    ],
                  ),
                ),
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
                          onPressed: () => setState(() => _skillSearch.clear()),
                        ),
                  border: const OutlineInputBorder(),
                ),
                onChanged: (_) => setState(() {}),
              ),
            ],
            _SkillDropIn(store: store, onOpen: () => _openSkillsFolder(), onCopy: () => _copySkillsPath()),
            if (store.skillsBusy) const LinearProgressIndicator(minHeight: 2),
            if (store.skillsError != null)
              Text('Skills 加载失败：${store.skillsError}', style: theme.textTheme.bodySmall),
          ]),
        ),
        if (!_skillsOpen)
          // 折叠时不列条目，只留一句提示（用户想找 skill 就展开）
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 12),
            sliver: SliverToBoxAdapter(
              child: Text(
                store.skills.isEmpty
                    ? '还没有 Skill。把 skill 文件夹（或一个 .md）拷进上面的目录再点刷新，'
                        '也可以让 AI 用 register_skill 注册。'
                    : '列表已折叠（${store.skills.length} 个）。点上面的「Skills」展开并搜索。',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          )
        else if (_filteredSkills(store).isEmpty)
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 12),
            sliver: SliverToBoxAdapter(
              child: Text(
                store.skills.isEmpty
                    ? '还没有 Skill。把 skill 文件夹（或一个 .md）拷进上面的目录再点刷新，'
                        '也可以让 AI 用 register_skill 注册。'
                    : '没有匹配「${_skillSearch.text.trim()}」的 Skill。',
                style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              ),
            ),
          )
        else
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
            sliver: SliverList.builder(
              itemCount: _filteredSkills(store).length,
              itemBuilder: (context, i) {
                final skill = _filteredSkills(store)[i];
                return _SkillRow(
                  skill: skill,
                  onDelete: () => _deleteSkill(skill),
                );
              },
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
