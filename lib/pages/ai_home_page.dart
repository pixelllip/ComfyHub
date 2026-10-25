import 'dart:async';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/api_client.dart';
import '../core/file_clipboard.dart';
import '../core/file_drop.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/app_menu.dart';
import '../widgets/common.dart' show FilterChipRow;
import '../widgets/markdown_view.dart';
import '../widgets/nsfw_blur.dart';
import '../widgets/workflow_picker.dart';
import '../widgets/workflow_viewer.dart';
import '../widgets/zoomable_image_view.dart';
import 'media_detail_page.dart';
import 'train_page.dart';

/// 这个附件能不能**在本应用内**直接看（图片），而不是丢给系统默认程序。
///
/// 历史消息里的附件块**没有 modality**（后端只落 `attachmentId` 与名字），所以：
/// * 有 modality 就信它（输入区托盘是有的）；
/// * 没有就按扩展名兜底 —— 认不出来一律外跳（文档 / 音频 / 压缩包本来就该交给系统）。
///
/// 用户 bug 2026-10-18："点击会话内的图片会跳转浏览器而不是在应用内弹出小窗。"
bool canPreviewInline(String? modality, String name) {
  if (modality != null && modality.isNotEmpty) return modality == 'image';
  final lower = name.toLowerCase();
  return const [
    '.png',
    '.jpg',
    '.jpeg',
    '.webp',
    '.gif',
    '.bmp',
    '.avif',
  ].any(lower.endsWith);
}

/// AI 工作台（AIH-001 / AIH-002 / AIK-002）。
///
/// 宽屏三栏：会话列表 | 消息 + 输入 | ComfyUI 与上下文。
/// 窄屏：会话列表进抽屉，状态栏进底部 Sheet（入口在标题栏，**不用 FAB** ——
/// FAB 会盖住输入区的「发送」按钮），输入区**始终可见**。
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

  /// 左侧会话列表（宽屏左栏 / 窄屏抽屉共用同一个实例）。
  ///
  /// **必须是常驻字段，不能在 `build()` 里新建**：页面在流式回包 / 进度轮询时
  /// 每通知一次就重建一次，若每次都 `_ConversationList(...)` 造一个新 widget，
  /// Flutter 的 `canUpdate` 会认下来并调 `State.build()` —— 侧栏（几十个
  /// ListTile + 每个一个菜单锚点 + Tooltip）就跟着重排，正是 DevTools 里
  /// 那一帧 24.7ms Build 的来源。实例不变时整棵子树被跳过。
  late final Widget _conversationList;

  /// 正在把草稿写回输入框。这时**必须屏蔽** `_onInputChanged` ——
  /// 否则"清空输入框"这个动作会被当成"用户把字删光了"，把刚取回来的草稿覆盖成空串
  /// （切走再切回来字就没了，用户报的 bug ②）。
  bool _applyingDraft = false;

  /// 第几次切换会话：异步取草稿回来时用它判断"还是不是同一次切换"。
  int _draftEpoch = 0;

  /// 视口是不是贴着底部（用户建议 ③）：**贴底才自动跟随**，翻旧消息时不打扰。
  bool _atBottom = true;

  /// 上一次通知时**消息区里装的是哪条会话**。
  ///
  /// 用来识别"内容整批被换掉"这个时机（换会话 / 重新加载）——
  /// 见 [_resetScrollForConversation]：换会话要把滚动位置重置到最新一条。
  String? _shownConversationId;

  /// "已经贴底"的容差，单位像素。**只用来吸收亚像素误差**，不是"接近底部"的意思。
  ///
  /// 老代码这里是 80，那个数字就是"只滚一小段就被拽回底部"的根因 ——
  /// 完整的来龙去脉见 [_onScroll] 的注释。
  static const double _bottomEpsilon = 1.5;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_loaded) return;
    _loaded = true;
    _store = context.read<AiWorkspaceStore>();
    _conversationList = _ConversationList(onRename: _rename);
    _input.addListener(_onInputChanged);
    _scroll.addListener(_onScroll);
    _store.addListener(_onStoreChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      // `load()` 自己是幂等的：切到别的页再回来（页面会被重建）不会重新加载、
      // 更不会再新建一条会话。
      if (mounted) _store.load();
      // 预设页可能在**这个页面挂上来之前**就把文本交过来了（先设文本、再切页），
      // 那时监听器还没挂上 —— 所以这里补一次取用。
      if (mounted) _applyHandoffIfAny();
      // 回到聊天页 = 可能刚从「AI 模型与凭据」改完模型的能力声明回来：
      // 目录重读一次，改动当场生效（用户 bug 2026-09-28「模型声明要重启客户端才生效」）。
      // 冷启动那一次不用：load() 自己会读目录（loaded 还没翻真，正好跳过）。
      if (mounted && _store.loaded) _store.refreshModels();
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
    _applyHandoffIfAny();
    final id = _store.conversation?.id;
    _resetScrollForConversation(id);
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

  /// 取走预设页交过来的文本（`▶️运行`），填进输入框并聚焦。
  ///
  /// 三条别踩：
  ///  1. **取走即清空**（`takeHandoff`）—— 否则每次切回工作台都会再填一遍，
  ///     用户刚清空的输入框会"复活"上一段字；
  ///  2. **只在输入框为空时填**：用户可能已经打了一半，直接覆盖等于把他的字吃掉；
  ///     有内容时改走"追加到末尾"，不丢他的输入；
  ///  3. **不触发草稿保存**（走 [_setInputText]）—— 这段字是程序填的，
  ///     不该被当成"用户打出来的草稿"落盘。
  void _applyHandoffIfAny() {
    if (!_store.hasHandoff) return;
    final text = _store.takeHandoff();
    if (text == null || text.isEmpty) return;
    final existing = _input.text.trim();
    _setInputText(existing.isEmpty ? text : '$existing\n$text');
    _inputFocus.requestFocus();
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
  ///
  /// 判据只有一条：**是不是真的贴底**（容差 [_bottomEpsilon]，只吸收亚像素误差）。
  ///
  /// 老写法是 `distance < 80 ? true : ...` —— 那等于给"贴底"画了一个 80px 宽的笼子。
  /// 这就是用户 bug 2026-09-20 那个"两个症状、一个根因"：
  ///  ① 流式输出时往上滚一小段：内容同时在往下长，`max` 涨、`pixels` 没动多少，
  ///     算出来的距离一直落在 80 以内 → 下一帧又被判成"贴底" →
  ///     `_followIfAtBottom()` 的 `animateTo(max)` 把人拽回底部（"只滚一小段的话
  ///     界面会立即重置到底部，再继续滚才不会滚下去"）。
  ///  ② 快速拉滚动条时位置变化异常：用户一边拖，那个 180ms 的跟随动画一边在执行、
  ///     每来一段流式内容又排一次新的 —— 手指和动画各拉一个方向，位置就跳。
  ///
  /// 容差收到 1.5 之后，**用户滚一像素就脱离"贴底"**：`_followIfAtBottom()` 当场不再
  /// 排新的跟随动画，上面两条同时消失（用户滚回底部时距离归零，自然重新贴上）。
  ///
  /// 这里**不需要**再判"位置有没有变小"：内容变长时 `max` 在涨而 `pixels` 没动，
  /// 距离确实会变大，但那只是"新内容在下面"，跟随照旧；而用户真的滚上去了，
  /// 距离一定会超过这个容差。
  ///
  /// 另外**不要**在这里去动 `position`（比如"把在飞的动画按停"）：`UserScrollNotification`
  /// 是在 `ScrollDragController.update` 的中间派发的，此刻 `applyUserOffset` 正等着
  /// `setPixels`，任何 `jumpTo` / `hold` 都会把 `activity` 换成 Idle ——
  /// 结果是**用户的手指当场失效**，而且 Flutter 会直接抛
  /// `'activity!.isScrolling': is not true`（本机实测过）。
  void _onScroll() {
    if (!_scroll.hasClients) return;
    final distance = _scroll.position.maxScrollExtent - _scroll.position.pixels;
    final next = distance <= _bottomEpsilon;
    if (next != _atBottom && mounted) setState(() => _atBottom = next);
  }

  /// 只有"用户本来就在底部"才跟着新内容滚（用户建议 ③）。
  void _followIfAtBottom() {
    if (!_atBottom) return;
    _scrollToBottom();
  }

  /// 换了会话（消息内容**整批被换掉**）时，把滚动位置重置到最新一条。
  ///
  /// 用户 2026-09-20 报的第四个坑是"总长变小、`pixels` 还停在按旧总长算的位置上 ——
  /// 内容被顶到视口外，第一条消息在界面上直接消失"。触发它的时机正是这里：
  /// `openConversation` 会把 `messages` 整批换掉，而本页只是重新 `build`，
  /// **滚动位置不会自己重置** —— `pixels` 被原样带到新会话上：
  ///   · 新会话内容更短 → 被夹回/顶出视口；
  ///   · 新会话内容一样长 → 直接落在半路（连夹取都不会发生，用例里实测差 256px）。
  ///
  /// 所以"换会话落在最新一条"这条产品纪律由界面**显式**负责，不靠框架的夹取去猜。
  /// 换会话 = 用户想看最新的进展，落在半路没有任何理由。
  ///
  /// 必须放在**帧后**：这一刻新会话的内容还没布局，`maxScrollExtent` 还是旧的，
  /// 当场 `jumpTo` 会跳到一个马上就不成立的偏移上。这里用 `jumpTo`（不是动画）：
  /// 内容整批换掉是"切换"，不是"滚一段"，中间那几帧滑过旧位置没有意义。
  void _resetScrollForConversation(String? id) {
    if (id == _shownConversationId) return;
    _shownConversationId = id;
    if (id == null) return;
    if (mounted) setState(() => _atBottom = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scroll.hasClients) return;
      _scroll.jumpTo(_scroll.position.maxScrollExtent);
    });
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
    // 第三栏（ComfyUI 状态）只在够宽时才铺开。
    //
    // **900~1199px 曾经是个死角**（用户建议 ② / 需求审计 AIH-002）：那个区间
    // `wide` 成立 → 没有窄屏那个 FAB，但 `showPanel` 不成立 → 也没有第三栏，
    // 于是「ComfyUI 状态」整块内容在界面上**一个入口都没有**。
    // 当时补的是"只在 900~1199px 把入口放进 AppBar"，结果**窄屏那一支还是靠 FAB** ——
    // 而 FAB 钉在 `Scaffold` 的右下角，正好落在输入区「发送」按钮上面：
    // 窗口一窄就盖住发送按钮（用户 2026-09-20 报的 bug）。现在入口只在**一个地方**：
    // 第三栏铺不开时，一律进 AppBar（`onShowStatus`），窄屏不再有 FAB。
    final showPanel = wide && width >= 1200;

    final conversationList = _conversationList;
    // AppBar 显示**当前对话标题**（用户建议 ④）：标题是自动总结出来的，
    // 所以它得一直在视野里，用户才知道自己在哪条对话里、也可以随手改名。
    //
    // **对话主体单独抽成一个会 `watch` 的 widget**（[_Thread]）：流式回包 /
    // ComfyUI 进度轮询每秒会通知好几次，以前是整页（含左侧 240px 会话列表）
    // 跟着重建 —— DevTools 实测一帧 Build 24.7ms、侧栏每个会话行一次重建
    // （Tooltip 12 / MenuAnchor 12 / ListTile 10）。现在只有这棵树重建。
    final thread = _Thread(
      controller: _scroll,
      atBottom: _atBottom,
      input: _input,
      inputFocus: _inputFocus,
      onShowStatus: showPanel ? null : () => _showStatusSheet(context),
      onJumpToBottom: _jumpToBottom,
      onSubmitted: (store, text) async {
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
    );
  }

  void _showStatusSheet(BuildContext context) {
    final store = context.read<AiWorkspaceStore>();
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (_) => SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.6,
        child: _ContextPanel(store: store),
      ),
    );
  }

  /// 重命名一条会话（侧栏那行的「重命名」菜单递过来的动作）。
  Future<void> _rename(BuildContext context, AiConversation c) async {
    // 页面自己的 State 不再 `watch` store（见 build 的注释）：
    // 这里按需取，改完由 store 通知侧栏自己重建。
    final store = context.read<AiWorkspaceStore>();
    final controller = TextEditingController(text: c.title);
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('重命名会话'),
        content: TextField(controller: controller, autofocus: true),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, controller.text),
            child: const Text('保存'),
          ),
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
//  对话主体：AppBar + 消息列表 + 输入区
// ---------------------------------------------------------------------------

/// 会话主体整棵树。
///
/// **单独抽出来是为了把 `watch` 关在这棵子树里**：流式回包（约 60 次/秒的增量、
/// 攒到一帧仍有一二十次）与 ComfyUI 进度轮询（1.5 秒一次）都会
/// `notifyListeners()`，以前整页（含左侧 240px 会话列表）跟着重建 ——
/// DevTools 实测一帧 Build 24.7ms（UI Jank），侧栏每个会话行重建 10~12 次
/// （Tooltip / MenuAnchor / ListTile 全在表上）。
/// 侧栏与消息区本来就不相关，不该被对面的刷新拖着重建。
///
/// 回调都从外面传进来（滚动位置、输入框控制器、提交动作），
/// 这样这棵树刷新时上面的页面 State 不必跟着动。
class _Thread extends StatelessWidget {
  const _Thread({
    required this.controller,
    required this.atBottom,
    required this.input,
    required this.inputFocus,
    required this.onShowStatus,
    required this.onJumpToBottom,
    required this.onSubmitted,
  });

  final ScrollController controller;
  final bool atBottom;
  final TextEditingController input;
  final FocusNode inputFocus;
  final VoidCallback? onShowStatus;
  final VoidCallback onJumpToBottom;
  final Future<void> Function(AiWorkspaceStore store, String text) onSubmitted;

  /// 顶栏「选择现有工作流」：把路径写进输入框，让 AI 照着这一份出图。
  ///
  /// 不直接发送：用户还要补一句"想生成什么"。路径写进去之后，AI 的第一动作必然是
  /// `comfy_load_workflow(path=…)`（系统提示第 23 条 + `comfy-workflow-guide`），
  /// 不会自己编一份新的。
  Future<void> _pickWorkflow(BuildContext context) async {
    final store = context.read<AiWorkspaceStore>();
    final file = await showWorkflowPicker(
      context,
      loadList: store.listWorkflowFiles,
      loadRaw: store.readWorkflowFile,
      diagnose: store.workflowBlueprint,
      save: (raw, name) => store.saveWorkflow(raw: raw, name: name),
    );
    if (file == null || !context.mounted) return;
    final existing = input.text.trimRight();
    final hint = existing.isEmpty ? '' : '\n';
    final insert = '用这份工作流生成：${file.path}\n';
    final next = '$existing$hint$insert';
    input.value = TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: next.length),
    );
    inputFocus.requestFocus();
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AiWorkspaceStore>();
    return Column(
      // `stretch` 不能省（用户 bug 2026-09-20："聊天主体距离两侧边栏空出一大片空白"）。
      //
      // Column 的默认 `crossAxisAlignment` 是 **center**：子项拿到的是**松约束**，
      // 于是消息区（`SingleChildScrollView`）会缩到"最宽那条气泡 + 内边距"的宽度
      // （气泡上限 720 → 752），再**居中**摆在栏里 —— 实测 1600 逻辑像素的窗口下
      // 消息区只有 752 宽、两侧各空 173，滚动条也画在那条 752 的右边缘上，
      // 离边栏老远（theme 里把 `crossAxisMargin` 调成 0 治不了这个：滚动视图本身就没铺满）。
      // 输入区同理：它跟着缩成一条窄条、居中。
      // 铺开之后：消息区 = 栏宽、滚动条贴右侧边栏、输入区也是整栏宽。
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _ConversationAppBar(
          store: store,
          onShowStatus: onShowStatus,
          onPickWorkflow: () => _pickWorkflow(context),
        ),
        Expanded(
          child: Stack(
            children: [
              _MessageList(store: store, controller: controller),
              // 「AI 正在生成」的加载提示**不再做成底部浮层**（用户建议 2026-09-20：
              // "不要出现在底部（很影响观感），而是当前正在生成回复的气泡底部，
              // 并且只保留转圈圈动画"）—— 它现在是那个气泡自己的内容，
              // 见 `_MessageBubble` 里 `isStreaming` 那一段。
              // 回到聊天底部（用户建议 ④）：**只在用户翻上去之后**才出现，
              // 贴在对话主体右下角，不挡输入区。
              if (!atBottom)
                Positioned(
                  right: 16,
                  bottom: 56,
                  child: Tooltip(
                    message: '回到最新消息',
                    child: FloatingActionButton.small(
                      heroTag: 'ai-jump-bottom',
                      onPressed: onJumpToBottom,
                      child: const Icon(Icons.arrow_downward),
                    ),
                  ),
                ),
            ],
          ),
        ),
        // 输入区**被提问接管**（用户 bug 2026-09-21）：AI 正在等用户回答时，
        // 底下这块整个换成提问卡（选项 + 自己输入 + 跳过），而不是在普通输入框
        // 上面再摞一张卡 —— 那样用户会不知道"该往哪儿打字"。
        // 答完（或跳过）`pendingQuestion` 变 null，普通输入框自动回来。
        if (store.pendingQuestions case final questions
            when questions.isNotEmpty)
          _AskComposer(
            store: store,
            questions: questions,
            onSubmit: (text) => onSubmitted(store, text),
          )
        else
          _Composer(
            store: store,
            controller: input,
            focusNode: inputFocus,
            onSubmit: (text) => onSubmitted(store, text),
          ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
//  会话列表
// ---------------------------------------------------------------------------

class _ConversationList extends StatefulWidget {
  /// 重命名一条会话（对话框在页面那边，这里只负责把动作递过去）。
  final Future<void> Function(BuildContext context, AiConversation conversation)
  onRename;

  const _ConversationList({required this.onRename});

  @override
  State<_ConversationList> createState() => _ConversationListState();
}

/// 这一栏**只在"会话列表真的变了"时重建**（用户 bug 2026-09-20 的性能现场）。
///
/// 为什么不直接 `context.watch`：Provider 的 watch 是"通知一次就重建一次"，
/// 而流式回包（攒到一帧也有十几次/秒）与 ComfyUI 进度轮询（1.5 秒一次）都会
/// `notifyListeners()` —— DevTools 实测那些通知把左侧 240px 里每个 ListTile +
/// 菜单锚点 + Tooltip 全重建了一遍（Rebuild Count 10~12，一帧 Build 24.7ms）。
/// 会话列表跟"消息在长"没有关系，不该跟着刷。
///
/// 这里自己 `addListener`，比较**真正影响这一栏的两样东西**：会话列表本身
/// 与"当前选中哪一条"。都没变就什么都不做。
class _ConversationListState extends State<_ConversationList> {
  late AiWorkspaceStore _store;
  List<AiConversation> _conversations = const [];
  String? _selectedId;

  @override
  void initState() {
    super.initState();
    _store = context.read<AiWorkspaceStore>();
    _snapshot();
    _store.addListener(_onStoreChanged);
  }

  @override
  void dispose() {
    _store.removeListener(_onStoreChanged);
    super.dispose();
  }

  void _snapshot() {
    _conversations = _store.conversations;
    _selectedId = _store.conversation?.id;
  }

  void _onStoreChanged() {
    if (!mounted) return;
    final next = _store.conversations;
    final nextSelected = _store.conversation?.id;
    if (identical(next, _conversations) && nextSelected == _selectedId) return;
    setState(_snapshot);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = _store;
    sidebarBuilds++;
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
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                )
              // 「选中」和「鼠标按下 / 悬停」必须一眼分得开（用户 bug：次新那条会话
              // 顶着一块亮灰底，看起来比当前会话还像被选中）。
              // Material 默认的**按下高亮**是 ~38% 白，比选中态的存在感强得多；而且窗口在
              // 收到 mouse-up 之前失去焦点 / 被最小化时，这个按下高亮会**卡在屏幕上不动**。
              // 所以：按下 / 水波纹 / 悬停一律压到很淡，选中态改用明确底色（见下面的 selectedTileColor）。
              : Theme(
                  data: theme.copyWith(
                    highlightColor: theme.colorScheme.primary.withValues(
                      alpha: 0.10,
                    ),
                    splashColor: theme.colorScheme.primary.withValues(
                      alpha: 0.10,
                    ),
                    hoverColor: theme.colorScheme.onSurface.withValues(
                      alpha: 0.06,
                    ),
                  ),
                  child: ListView.builder(
                    // 列表整体留出内边距（用户建议 ⑥）：以前 ListTile 是**贴着**左右边缘画的，
                    // 选中底色从窗口边缘一直铺到底，看着像"一整条横幅"而不是一行一个会话。
                    padding: const EdgeInsets.symmetric(
                      horizontal: 8,
                      vertical: 4,
                    ),
                    itemCount: store.conversations.length,
                    itemBuilder: (context, i) {
                      final c = store.conversations[i];
                      final selected = store.conversation?.id == c.id;
                      return ListTile(
                        dense: true,
                        selected: selected,
                        // 有了外边距之后配圆角才像"一行一条"
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(8),
                        ),
                        // M3 的 ListTile 选中态**默认只把文字染成主题色、没有底色**，
                        // 于是"鼠标压过的那一行"看着比真选中的还显眼。这里给当前会话一个明确的底色。
                        selectedTileColor: theme.colorScheme.primary.withValues(
                          alpha: 0.16,
                        ),
                        title: Text(
                          c.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        // 条数 + **最近一次对话时间**（用户建议 ⑥）：
                        // 光有"N 条消息"看不出哪条是刚聊过的，得切进去才知道。
                        subtitle: Text(
                          '${c.messageCount} 条消息 · ${relativeTime(c.updatedAt)}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.labelSmall,
                        ),
                        onTap: () {
                          store.openConversation(c.id);
                          if (Scaffold.maybeOf(context)?.isDrawerOpen == true) {
                            Navigator.of(context).pop();
                          }
                        },
                        trailing: AppMenuButton<String>(
                          tooltip: '更多',
                          onSelected: (v) {
                            if (v == 'rename') widget.onRename(context, c);
                            if (v == 'archive') store.archiveConversation(c.id);
                            if (v == 'delete') store.deleteConversation(c.id);
                          },
                          options: const [
                            MenuOption(
                              value: 'rename',
                              icon: Icons.edit_outlined,
                              label: '重命名',
                            ),
                            MenuOption(
                              value: 'archive',
                              icon: Icons.archive_outlined,
                              label: '归档',
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
                            tooltip: '更多',
                            onPressed: () => controller.isOpen
                                ? controller.close()
                                : controller.open(),
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
}

/// 顶部条：显示**当前对话标题**，可以直接改名，也能看到当前模型与消息数。
///
/// 对话标题现在是 AI 在第一次提问时自动总结出来的（用户建议 ③），
/// 用户不接受这个总结时可以在这里（或会话列表的「重命名」里）改掉。
class _ConversationAppBar extends StatelessWidget
    implements PreferredSizeWidget {
  final AiWorkspaceStore store;

  /// 给「ComfyUI 状态」用的入口（可空）。
  ///
  /// **只在第三栏铺不开时给**（< 1200px）：那个宽度下右侧栏不存在，不补这个按钮
  /// 的话 Comfy 状态整块内容在界面上不可达（用户建议 ② / AIH-002）。
  /// 窄屏以前用的是右下角 FAB，而 FAB 会盖住输入区的发送按钮（用户 bug），
  /// 所以现在无论宽窄都走这一个入口。
  final VoidCallback? onShowStatus;

  /// 「选择现有工作流」入口（用户 2026-10-21）。
  ///
  /// 放在顶栏而不是输入区底行：320 宽下底行已经排满 4 枚图标 + 发送，
  /// 再塞一枚会溢出；顶栏这里空间稳定，且"选素材"本来就不是发送动作。
  final Future<void> Function()? onPickWorkflow;

  const _ConversationAppBar({
    required this.store,
    this.onShowStatus,
    this.onPickWorkflow,
  });

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
            [?model, if (conv != null) '${conv.messageCount} 条消息'].join(' · '),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        ],
      ),
      actions: [
        if (onPickWorkflow != null)
          IconButton(
            tooltip: '选择现有工作流（可看蓝图）',
            icon: const Icon(Icons.account_tree_outlined, size: 18),
            onPressed: onPickWorkflow,
          ),
        if (onShowStatus != null)
          IconButton(
            tooltip: 'ComfyUI 状态',
            icon: const Icon(Icons.dns_outlined, size: 18),
            onPressed: onShowStatus,
          ),
        IconButton(
          tooltip: 'LoRA 训练台（取训练集 → 筛选 → 打标 → 预处理 → 参数 → 训练 / Krea-2）',
          icon: const Icon(Icons.school_outlined, size: 18),
          onPressed: () => Navigator.of(context).push(
            MaterialPageRoute(builder: (_) => const TrainPage()),
          ),
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
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('取消'),
          ),
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

/// 消息区。
///
/// **精确总长**（用户 bug 2026-09-20，第二版）：滚动条滑块的长度与位置都是
/// `pixels / maxScrollExtent` 算出来的，而懒构建的 `ListView` 对**还没布局到的**
/// 子项只能估算总长 —— 估算每滚一段就换一个值，滑块就来回跳（用户报的
/// "在最新一条聊天记录里，滚动条会跳变 1/3 的进度"）。
///
/// 前一版把缓存窗口放到前后各 2 个视口，只是让"已建那几项的平均高度"更稳一点：
/// 实测变化 17 次 → 11 次、回缩 9 次 → 4 次，**压掉一半、压不到 0** ——
/// 因为"估"这个本质没变。
///
/// 这一版把"估"整个拿掉：`SingleChildScrollView + Column` 里，内容高度
/// **就是真实高度**（不是估算、也没有第二条"硬约束"通路），
/// `maxScrollExtent = scrollExtent − viewportDimension` 从第一帧起就精确，
/// 滑块的长度与位置**全程不变**。
///
/// 为什么不用 `ListView` 的 `itemExtentBuilder`（用户探过的那条路）：
/// 它要求**每一条**都给一个精确高度，而且那个返回值是**硬约束**，差一点就直接
/// 裁内容（用户实测 `RenderFlex overflowed by 168~512px`）。要在懒构建的前提下
/// 拿到"每一条的真值"，就得再搭一层离屏测量（量宽度还得和真实布局宽度
/// 逐像素一致）—— 拿一个必然会算错的地方，去换一个本来就有精确答案的地方，不值得。
///
/// 代价说清楚（本机实测，`flutter test` 里同一趟滚动 40 帧、视口 1100×800、
/// 每条气泡一段中长文本）：
///
/// | 会话长度 | 懒构建 + 估算（旧） | 本文这一版（精确） |
/// | --- | --- | --- |
/// | 31 条 | 168ms | 165ms（持平） |
/// | 200 条 | 294ms | 401ms |
/// | 800 条 | 293ms | 990ms |
///
/// 也就是：**常用长度持平，极端长度才变贵**。贵在"整棵树每帧要走一遍"，
/// 换来的是滚动条滑块**一次都不跳**（用例实测：滚一趟 `maxScrollExtent`
/// 变化 0 次，旧版 13~24 次），这个交换是值的。
///
/// 为什么"整棵树走一遍"没把事情弄坏：
///   · 气泡本身就是懒的：`_MessageBubbleSlot` 在内容没变时把**同一个 widget
///     实例**交回去，Flutter 连那棵子树的重建与布局都跳过；
///   · 没变过的 `RenderBox` 不会重排、不在视口里的不会重绘。
/// 所以**流式输出时每帧只有正在长的那一条要重排** —— 这一点和以前一样。
///
/// 真要再往下优化，方向是"给还没量过的行一个精确高度的懒构建"
/// （`ListView.itemExtentBuilder`）。但那要额外搭一层离屏测量，而且它的返回值是
/// **硬约束**：差一点就直接裁内容（用户实测 `RenderFlex overflowed by 168~512px`），
/// 量宽度还必须与真实布局宽度逐像素一致 —— 不该塞进这次修复里。
class _MessageList extends StatelessWidget {
  final AiWorkspaceStore store;
  final ScrollController controller;

  const _MessageList({required this.store, required this.controller});

  /// 每一条气泡的外边距（用户建议 2026-09-20："聊天气泡距离边缘 20 到 30 像素"）。
  ///
  /// 取 24：气泡铺满整栏宽度（不再有 `maxWidth` 上限），左右各留 24 ——
  /// 用户气泡贴右边栏、助手气泡贴左边栏，中间是一条读得下去的行宽。
  static const EdgeInsets _itemPadding = EdgeInsets.symmetric(
    horizontal: 24,
    vertical: 6,
  );

  @override
  Widget build(BuildContext context) {
    if (store.messages.isEmpty) return _EmptyThread(store: store);

    return SingleChildScrollView(
      // 聊天气泡列表：界面里还有别的可滚动区域（会话列表、气泡里的清单），
      // 回归用例要能精确点到**这一个**（滚动跟随 / 贴底那几条断言都靠它）。
      key: const Key('ai-message-list'),
      controller: controller,
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Column(
        // `stretch`：**每条消息占满整行**，靠左还是靠右由 `_MessageBubble`
        // 自己的 `crossAxisAlignment` 决定（用户消息靠右）。
        //
        // 这里以前是 `start`，子项因此拿到松约束、缩到"气泡自己那么宽"，
        // 而 `_MessageBubble` 那层 Column 也就跟着缩成一样宽 —— 它里面写的
        // `CrossAxisAlignment.end` 于是**完全没有效果**：用户消息实际是左对齐的，
        // 右侧再空出一大片（用户 2026-09-20 的截图）。
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final message in store.messages)
            Padding(
              key: ValueKey(message.id),
              padding: _itemPadding,
              // RepaintBoundary / 重建闸门都在 `_MessageBubbleSlot` 里：见它的注释
              // （流式输出时只有**真的变了**的那一条会重建，历史气泡整棵跳过）。
              child: _MessageBubbleSlot(store: store, message: message),
            ),
        ],
      ),
    );
  }
}

/// 聊天气泡**真正被重建**的次数（回归用例用它证明"没变化的气泡不会跟着重建"）。
///
/// 生产路径上只加一次自增，不参与任何逻辑判断。
@visibleForTesting
int aiChatBubbleBuilds = 0;

/// 侧栏整栏 build 的次数（回归用例用它证明"与侧栏无关的通知不会重建侧栏"）。
///
/// 生产路径上只加一次自增，不参与任何逻辑判断。
@visibleForTesting
int sidebarBuilds = 0;

/// 一条气泡的「重建闸门」（用户 bug：聊天快速上下滑动卡顿 / 展开流式思考后卡顿）。
///
/// 为什么需要它：列表的 `itemBuilder` 每次刷新都会重跑，如果直接建 `_MessageBubble`，
/// 一屏里的**每一条历史气泡**都会跟着重新解析 Markdown、重新排版 —— 逐字回包时
/// 一秒几十次，长对话越聊越卡，滑块也跟着抖。
///
/// 可行的前提是 Store 那边的两件事：
///  1. `_replaceMessage()` 只替换**变化的那一条** `AiMessage` 实例，其余原样复用；
///  2. 实时状态（思考增量 / 工具卡）按消息记版本号（`store.liveRevisionOf`）。
///
/// 所以"实例没变 + 发送状态没变 + 这条的实时版本没变"就等于"这条没有变化"，
/// 把上一次建好的子树原样交回去 —— Flutter 见到 identical 的 widget 会整棵跳过
/// （`Element.updateChild` 里的 `child.widget == newWidget` 快速路径）。
/// 主题 / 语言这类 InheritedWidget 依赖**不受影响**：它们走依赖通知，各自标脏。
class _MessageBubbleSlot extends StatefulWidget {
  final AiWorkspaceStore store;
  final AiMessage message;

  const _MessageBubbleSlot({required this.store, required this.message});

  @override
  State<_MessageBubbleSlot> createState() => _MessageBubbleSlotState();
}

class _MessageBubbleSlotState extends State<_MessageBubbleSlot> {
  Widget? _cached;
  AiMessage? _cachedMessage;
  bool? _cachedSending;
  int? _cachedLive;
  String? _cachedStreamingId;
  String? _cachedPermission;

  @override
  Widget build(BuildContext context) {
    final store = widget.store;
    final live = store.liveRevisionOf(widget.message.id);
    // 「是不是正在生成的那一条」也要进闸门：不然"这一轮换了哪条消息在流"
    // 时，旧气泡上的转圈动画会留在原地（父级重建了、缓存却没失效）。
    final streamingId = store.streamingMessage?.id;
    final generating =
        !widget.message.isUser &&
        streamingId != null &&
        streamingId == widget.message.id;
    // 权限档同样要进闸门（用户建议 ①）：**生成过程中切档**要当场反映到气泡里，
    // 它不在 `message` 实例里、也不是 `liveRevision`，漏掉就会"切了但字没变"。
    final permission = generating ? store.permissionMode : '';
    final unchanged =
        _cached != null &&
        identical(_cachedMessage, widget.message) &&
        _cachedSending == store.sending &&
        _cachedLive == live &&
        _cachedStreamingId == streamingId &&
        _cachedPermission == permission;
    if (unchanged) return _cached!;

    final message = widget.message;
    _cachedMessage = message;
    _cachedSending = store.sending;
    _cachedLive = live;
    _cachedStreamingId = streamingId;
    _cachedPermission = permission;
    aiChatBubbleBuilds++;
    _cached = RepaintBoundary(
      child: _MessageBubble(
        message: message,
        sending: store.sending,
        // 「AI 正在生成」的转圈动画只画在**正在流的那一条**上（用户建议 2026-09-20）
        generating: generating,
        // 本轮权限档（用户建议 ①，2026-09-20）：**实时**读 store，就在正在生成的气泡里显示。
        // 只在流式期间给值 —— 历史气泡上挂"现在的档位"是错的（后端没按轮存过权限档）。
        permissionLabel: generating ? store.permissionLabel : '',
        // 这条回复当时用的是哪个模型（用户建议 2026-09-20）
        modelLabel: message.isUser ? '' : store.modelLabelOf(message.modelId),
        onRetry: store.retry,
        // 工具卡 / 思考都只属于助手消息
        toolCalls: message.isUser ? const [] : store.toolCallsFor(message),
        // 思考 / 正文按**流顺序**还原成段（用户要求"如实按流顺序显示"）
        segments: message.isUser ? const [] : store.segmentsFor(message),
        // 这条回复真正产出的产物（回复末尾的「画廊入口卡」）
        producedMediaIds: message.isUser
            ? const []
            : store.producedMediaIds(message),
        // 其中被 AI 声明为成人内容的那些：产物卡只糊这几张（用户 bug 2026-09-20）
        producedNsfwMediaIds: message.isUser
            ? const []
            : store.producedNsfwMediaIds(message),
        // 产出这些图的那份工作流也属于"生成的产物"（用户建议 ①）
        producedPromptIds: message.isUser
            ? const []
            : store.producedPromptIds(message),
        toolCategoryOf: store.toolCategoryOf,
        onApprove: store.approveToolCall,
        onDeny: store.denyToolCall,
        // 提问（用户建议 ⑥）：答完同一个 Run 接着做，不新建消息
        onAnswerQuestion:
            (questionId, {int? optionIndex, String? text}) async =>
                (await store.answerQuestion(
                  questionId,
                  optionIndex: optionIndex,
                  text: text,
                )).ok,
        onSkipQuestion: (questionId) async =>
            (await store.skipQuestion(questionId)).ok,
        // 卡片里"自己输入答案"→ 焦点交给底部被接管的那个答案框（用户 bug 2026-09-21）
        onFocusAskAnswer: () => store.pendingQuestionFocus.requestFocus(),
        // 用户消息里的附件块：按 id 现取缩略图 / 视频预览帧（M3）
        thumbUrlOf: store.thumbUrlFor,
        fileUrlOf: store.fileUrlFor,
        // 画廊产物（媒体 id）走画廊接口，别再喂给附件接口
        mediaThumbUrlOf: store.mediaThumbUrlFor,
        mediaPosterUrlOf: store.mediaPosterUrlFor,
      ),
    );
    return _cached!;
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

  /// 这条就是**正在流式生成**的那一条（用户建议 2026-09-20）：
  /// 「AI 正在生成」只在它的底部画一个转圈动画。
  final bool generating;

  /// 正在生成期间显示的本轮权限档（用户建议 ①，2026-09-20）。
  ///
  /// 这里给的是**这一刻**的档位（`store.permissionMode` 实时读），所以用户在
  /// 生成过程中切档，气泡里的字会立刻跟着变 —— 这正是"实时权限切换并展示"要的效果。
  /// 非流式期间一律空：后端没有按轮存过权限档，历史气泡上挂"现在的档位"是假的。
  final String permissionLabel;

  /// 这条回复**当时**用的模型名（用户建议 2026-09-20）。
  ///
  /// 取的是消息自己记的 `modelId`（后端逐条落库），不是"当前选中的模型"——
  /// 所以往上翻历史时看到的是每轮各自的模型，切换模型那轮一眼就能看出来。
  /// 用户消息不需要这个（显示的是"谁问的"，不是"谁答的"）。
  final String modelLabel;

  final Future<void> Function(String assistantMessageId) onRetry;

  /// 这条消息的有序工具调用（M4）。
  final List<AiToolCallState> toolCalls;

  /// 这条消息按流顺序排好的段：思考段 + 正文段（`reasoning` / `text`）。
  final List<AiMessageSegment> segments;

  /// 这次回复真正产出的画廊产物 id（`comfy_submit` 的 mediaIds）。
  final List<int> producedMediaIds;

  /// 其中被 AI 声明为成人内容的那些（`comfy_submit` 的 `nsfwMediaIds`）。
  final List<int> producedNsfwMediaIds;

  /// 这次回复入库的提示词 id（`comfy_submit` 的 capturedPromptId）。
  ///
  /// 用户建议 ①："「生成的产物」包括生成的工作流" —— 有了它，回复末尾那张卡
  /// 就能直接打开这次真正跑过的工作流（也能拖回 ComfyUI 复现）。
  final List<int> producedPromptIds;

  final String Function(String toolName) toolCategoryOf;
  final Future<void> Function(String callId) onApprove;
  final Future<void> Function(String callId) onDeny;

  /// 回答 / 跳过 AI 的提问（用户建议 ⑥）。
  final Future<bool> Function(
    String questionId, {
    int? optionIndex,
    String? text,
  })
  onAnswerQuestion;
  final Future<bool> Function(String questionId) onSkipQuestion;

  /// "自己输入答案"：把焦点交给**底部被提问接管的那块输入区**（用户 bug 2026-09-21）。
  ///
  /// 气泡里的提问卡刻意**不带输入框**（否则一屏两个框，不知道往哪儿写），
  /// 所以这一行只做一件事：把光标送到下面那个唯一的答案框里。
  final VoidCallback onFocusAskAnswer;

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
    this.generating = false,
    this.permissionLabel = '',
    this.modelLabel = '',
    required this.onRetry,
    this.toolCalls = const [],
    this.segments = const [],
    this.producedMediaIds = const [],
    this.producedNsfwMediaIds = const [],
    this.producedPromptIds = const [],
    required this.toolCategoryOf,
    required this.onApprove,
    required this.onDeny,
    required this.onAnswerQuestion,
    required this.onSkipQuestion,
    required this.onFocusAskAnswer,
    required this.thumbUrlOf,
    required this.fileUrlOf,
    required this.mediaThumbUrlOf,
    required this.mediaPosterUrlOf,
  });

  /// 气泡正文的有序块：思考 / 正文 / 工具卡 / **环节组**，顺序 = `parts` 的顺序（= 消息获取顺序）。
  ///
  /// 用户明确要求"完全按照消息获取顺序来，不要将最终回复置于最终思考的前面"：
  /// 早先这里是"所有思考与正文段先铺完、工具卡统统一坨挂在最下面"，
  /// 一轮回复里工具卡就跑到最终正文后面去了 —— 那也是重排。
  ///
  /// `parts` 还没到的流式阶段退回 `segments`：实时段里除了思考 / 正文 / 环节，还有
  /// **工具卡到达时的位置**（`type == 'tool_call'`，`text` 是 callId，见
  /// `AiWorkspaceStore._appendLiveToolCall`）。少了后者，流式期间工具卡就只能补在末尾，
  /// "思考 → 工具 → 正文"被画成"思考 → 正文 → 工具"，直到最终答复落库才跳回正确位置
  /// （用户 2026-09-19 报的错位）。
  ///
  /// **关键步骤（环节）折叠**（用户建议 2026-09-28）：`step` 块开一组，把**紧接着**的
  /// 工具卡收进那一组（默认折叠，见 [`_StepGroup`]）。两条纪律：
  ///  ① **只收连续的工具卡** —— 中间夹了正文 / 思考（或下一个环节）就先把这一组交出去
  ///     （宁可同一个环节名多一个组头，也不把正文挪到工具卡后面去：正文与工具卡的先后
  ///     是用户 bug⑥ 专门定下来的，不能再动）；
  ///  ② 同一环节名再次开组时标「续」，让人一眼看出这是同一个环节的第二段操作。
  List<Widget> _orderedBody(ThemeData theme) {
    final m = message;
    final streaming = m.status == 'streaming';
    final blocks = <Widget>[];
    final placed = <String>{};
    final steps = _StepCollector(out: blocks, buildCard: _toolCard);

    void addSeg(String type, String text) {
      if (text.isEmpty) return;
      if (type == 'reasoning') {
        // 思考段打断"连续工具卡"这一串：它自己也是内容，必须按流顺序留在原位
        steps.breakRun();
        blocks.add(_ReasoningPanel(text: text, streaming: streaming));
      } else if (type == 'step') {
        steps.open(text);
      } else if (type == 'text' && text.trim().isNotEmpty) {
        steps.breakRun();
        // 环节标记的兜底（手机 harness 是本地跑模型的，后端那条摘取链路不在它前面；
        // 桌面这边后端已经摘过，这里是幂等的 no-op）
        final visible = stripStepMarkers(text);
        if (visible.trim().isEmpty) return;
        // 用户消息是**纯文本**，不走 Markdown（用户打什么就显示什么）
        blocks.add(
          m.isUser
              ? Text(visible)
              : MarkdownText(
                  visible,
                  style: theme.textTheme.bodyMedium,
                  selectable: false,
                ),
        );
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
        // 环节块**不参与合并**（哪怕相邻同名）：合并了就把两个环节粘成一个名字
        if (part.type == 'step') {
          flush();
          addSeg('step', part.text ?? '');
          continue;
        }
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
        final call = toolCalls
            .where((c) => c.callId == part.toolCallId)
            .firstOrNull;
        if (call == null) continue;
        placed.add(call.callId);
        steps.addCall(call);
      }
      flush();
    } else {
      for (final seg in segments) {
        // 实时段里的工具卡位置占位（`text` 是 callId）：插回它到达时的位置，
        // 而不是等所有正文铺完再补在末尾
        if (seg.type == 'tool_call') {
          final call = toolCalls.where((c) => c.callId == seg.text).firstOrNull;
          if (call == null) continue;
          placed.add(call.callId);
          steps.addCall(call);
          continue;
        }
        addSeg(seg.type, seg.text);
      }
    }

    // 还没归位的工具卡（流式过程中 parts 尚未到达 / parts 里没记的）追加在末尾
    for (final call in toolCalls) {
      if (placed.contains(call.callId)) continue;
      steps.addCall(call);
    }
    steps.flush();
    return blocks;
  }

  Widget _toolCard(AiToolCallState call) => Padding(
    padding: const EdgeInsets.only(top: 8),
    // 提问（用户建议 ⑥）画的**不是工具卡而是提问卡**：一个是"等你批准"，
    // 一个是"等你回答"，混在一起用户会以为点了就等于批准。
    child: call.allQuestions.isNotEmpty
        ? _AskCard(
            key: ValueKey('${message.id}-${call.callId}'),
            // 整张卡（同组的所有题）——多问改造后只给一题会让别的题在界面上消失
            questions: call.allQuestions,
            onAnswer: onAnswerQuestion,
            onSkip: onSkipQuestion,
            // "自己输入答案"那一行把焦点交给底部被接管的输入框（用户 bug 2026-09-21）
            onFocusAnswer: onFocusAskAnswer,
          )
        : _ToolCallCard(
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
    final canRetry =
        !m.isUser &&
        (m.status == 'failed' || m.status == 'cancelled') &&
        !sending;
    // 待批准的工具调用（M4）：气泡里要能点「批准 / 拒绝」
    final pending = toolCalls.where((c) => c.needsApproval).toList();
    // 正文块只算一次：下面"退回整段正文"的兜底要用它判断"有序块与段是不是都没产出内容"
    final body = _orderedBody(theme);

    return Column(
      crossAxisAlignment: align,
      children: [
        Container(
          // 气泡本体（内含底色那一块）在用例里要能精确取到矩形 ——
          // "转圈动画贴在正在生成的那条气泡底部"这条断言就靠它。
          key: ValueKey('ai-bubble-${m.id}'),
          // **没有 maxWidth 上限**（用户建议 2026-09-20："空间利用率低"）：
          // 以前写死 720，宽窗口下右半边整片空着，表格 / 代码块也被挤在 720 里
          // 横向溢出。现在气泡由外层 `stretch` 撑满整行，左右各留 24（见 `_itemPadding`）。
          margin: const EdgeInsets.symmetric(vertical: 6),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(12),
          ),
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
                ...body,
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
                // 上面两条路**都没产出内容**时才退回整段正文（老数据 / 只有纯文本），
                // 用户消息本来就是纯文本，不走 Markdown。
                //
                // ⚠️ 判据必须是"正文块为空"，**不能**写成 `segments.isEmpty`：
                // 用户消息的 `segments` 是刻意传空的（它走 parts 那条路），那个条件对它**永远成立**，
                // 于是正文被渲染两遍 —— 用户 2026-09-19 的截图就是它：带附件的消息里
                // 同一句话上下各一份、图片正好夹在中间（`_orderedBody` 一份 + 这里一份）。
                if (body.isEmpty && m.text.isNotEmpty)
                  m.isUser
                      ? Text(m.text)
                      : MarkdownText(
                          m.text,
                          style: theme.textTheme.bodyMedium,
                          selectable: false,
                        ),
                // 「AI 正在生成」：**画在这条气泡自己的底部**，而且只有一个转圈动画
                // （用户建议 2026-09-20："不要出现在底部（很影响观感），而是当前正在
                // 生成回复的气泡底部，并且只保留转圈圈动画"）。
                //
                // 判据是"这条就是正在流的那一条"（`generating`），不是 `status == 'streaming'`：
                // 后者在流中断 / 被打断的历史消息上也可能留着，那会让旧气泡一直转圈。
                // 不再配文字 —— 转圈本身就是"正在生成"的通用语义，多一行字只是噪声；
                // 底部浮层那条也因此整个删掉了（它还会改内容总长、把滑块顶得乱跳）。
                if (generating)
                  Padding(
                    padding: EdgeInsets.only(
                      // 首字还没来时它就是气泡里唯一的内容，不加顶部间距；
                      // 已经有内容时贴在末尾，留一点呼吸位。
                      top: (body.isEmpty && m.text.isEmpty && toolCalls.isEmpty)
                          ? 0
                          : 6,
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        SizedBox(
                          width: 13,
                          height: 13,
                          child: CircularProgressIndicator(
                            strokeWidth: 2,
                            color: theme.colorScheme.primary,
                          ),
                        ),
                        // 本轮权限档（用户建议 ①）：就贴在这个转圈旁边。
                        // 用户在生成过程中切档，这行字**立刻**跟着变 ——
                        // 不用等这一轮结束，也不用去输入区低头找那个按钮。
                        if (permissionLabel.isNotEmpty) ...[
                          const SizedBox(width: 8),
                          Icon(
                            permissionLabel == AiToolPolicy.modeFullLabel
                                ? Icons.gpp_maybe_outlined
                                : Icons.how_to_reg_outlined,
                            size: 13,
                            color: permissionLabel == AiToolPolicy.modeFullLabel
                                ? theme.colorScheme.error
                                : theme.colorScheme.outline,
                          ),
                          const SizedBox(width: 3),
                          Text(
                            '本轮权限：$permissionLabel',
                            style: theme.textTheme.labelSmall?.copyWith(
                              color:
                                  permissionLabel == AiToolPolicy.modeFullLabel
                                  ? theme.colorScheme.error
                                  : theme.colorScheme.outline,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                // 工具卡已经在 [_orderedBody] 里按 parts 的真实顺序插好了 ——
                // 这里**不能再统一补一坨**，否则又变回"工具卡全排到最后"。
                if (pending.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      '有 ${pending.length} 个工具调用在等你的批准（写盘 / 改动类操作）。',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
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
                      nsfwMediaIds: producedNsfwMediaIds,
                      promptIds: producedPromptIds,
                      mediaThumbUrlOf: mediaThumbUrlOf,
                      mediaPosterUrlOf: mediaPosterUrlOf,
                    ),
                  ),
                if (m.status == 'cancelled')
                  Text(
                    '（已停止）',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                if (m.status == 'failed')
                  Text(
                    '（生成失败）',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.error,
                    ),
                  ),
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
                // 元信息（模型 / 思考 / token）在气泡**外面**渲染 —— 见下面 `_metaRow`。
              ],
            ),
          ),
        ),
        // 这一轮是怎么答出来的：模型（用户建议 2026-09-20）+ 思考强度 + token 用量。
        //
        // 放在气泡**外面**（不是里面）：它是元信息、不属于回复内容 ——
        // 放进气泡会被 `SelectionArea` 一起复制走（用户 bug ② 就是"选中的文本里
        // 混进了不该有的东西"），挂在外面既选不中、也一眼看得出属于哪一条。
        ?_metaRow(theme),
      ],
    );
  }

  /// 元信息行：模型 + 思考强度 + token 用量。
  ///
  /// 三者挤一行是有意的：说的都是"这一轮是怎么答出来的"。分成两行会让每条回复
  /// 底下挂两排小字，长对话读起来全是噪声。
  Widget? _metaRow(ThemeData theme) {
    final m = message;
    if (m.isUser) return null;
    final dim = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.outline,
    );
    return Padding(
      padding: const EdgeInsets.only(top: 2, left: 2, right: 2),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          // 本轮用的模型（用户建议 2026-09-20）：切换模型那一轮一眼就能看出来。
          // 取的是**这条消息自己记的** modelId，所以翻历史时看到的是各轮真实的模型。
          if (modelLabel.isNotEmpty) ...[
            Icon(
              Icons.smart_toy_outlined,
              size: 13,
              color: theme.colorScheme.outline,
            ),
            const SizedBox(width: 3),
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 260),
              child: Tooltip(
                // 短名可能被截断，完整 id 放 tooltip 里可查
                message: (m.modelId ?? '').isEmpty ? modelLabel : m.modelId!,
                child: Text(
                  modelLabel,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: dim,
                ),
              ),
            ),
            if ((m.reasoningEffort ?? 'off') != 'off' ||
                m.usage?.isEmpty == false)
              const SizedBox(width: 10),
          ],
          if ((m.reasoningEffort ?? 'off') != 'off') ...[
            Icon(
              Icons.psychology_outlined,
              size: 13,
              color: theme.colorScheme.outline,
            ),
            const SizedBox(width: 3),
            Text(
              '思考 ${AiReasoningEffort.parse(m.reasoningEffort)?.label ?? m.reasoningEffort}',
              style: dim,
            ),
            if (m.usage?.isEmpty == false) const SizedBox(width: 10),
          ],
          if (m.usage?.isEmpty == false) ...[
            Icon(Icons.data_usage, size: 13, color: theme.colorScheme.outline),
            const SizedBox(width: 3),
            Tooltip(
              message: m.usage!.detailLabel,
              child: Text(m.usage!.shortLabel, style: dim),
            ),
          ],
        ],
      ),
    );
  }
}

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
        color: theme.colorScheme.surface.withValues(alpha: 0.6),
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
            Text(
              widget.questions[i].question,
              style: theme.textTheme.bodyMedium,
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
  final bool selected;
  final bool enabled;
  final Widget? trailing;
  final VoidCallback onTap;

  const _AskOptionRow({
    super.key,
    required this.index,
    required this.label,
    this.hint,
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
            : theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
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
                if (trailing != null) ...[const SizedBox(width: 6), trailing!],
              ],
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
              color: theme.colorScheme.surface.withValues(alpha: 0.6),
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
                Text(q.question, style: theme.textTheme.bodyMedium),
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
                          ? '（这一题已跳过：模型会当作"用户没有回答"）'
                          : '已作答：${q.answer}',
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.primary,
                      ),
                    ),
                  ),
                // 选项多 / 问题长时**只有选项那段**内部滚动（240 上限），
                // "自己输入答案"排在它下面、永远看得见
                ConstrainedBox(
                  constraints: const BoxConstraints(maxHeight: 240),
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
                    minimumSize: const Size(0, 32),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  child: const Text('上一题'),
                ),
              TextButton(
                key: const Key('ask-skip-current'),
                onPressed: _busy ? null : _skipCurrent,
                style: TextButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  minimumSize: const Size(0, 32),
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
                    minimumSize: const Size(0, 32),
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
                  minimumSize: const Size(0, 36),
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
      color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
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

//  工具卡与思考（M4）
// ---------------------------------------------------------------------------

/// 一张工具卡：图标 + 工具名 + 状态徽标 + 耗时，详情默认折叠。
///
/// 只渲染后端给的内容（后端已经把凭据脱敏、结果截断到 MAX_RESULT_CHARS），前端不改写、不补全。
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
      AiToolCallStatus.failed ||
      AiToolCallStatus.denied => theme.colorScheme.error,
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
                  // 主标题是"这次在干什么"（模型写的 reason，或用途名），不是机器名。
                  call.name.isEmpty ? '未知工具' : call.displayTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelMedium,
                ),
              ),
              const SizedBox(width: 6),
              _StatusChip(label: call.status.label, color: color),
              if (call.elapsedLabel.isNotEmpty) ...[
                const SizedBox(width: 6),
                Text(
                  call.elapsedLabel,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
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
                  message:
                      '${_expanded ? '收起' : '展开'} ${call.displayTitle} 的结果',
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
          // 副标题：`用途 · 机器名`。机器名留在界面上，但不再是用户第一眼看到的东西。
          if (call.displaySubtitle.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(left: 20, top: 1),
              child: Text(
                call.displaySubtitle,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
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
                constraints: const BoxConstraints(maxHeight: 320),
                child: SingleChildScrollView(
                  // 三块**各自一个容器**（用户 2026-10-25：参数 / 结果分开显示，
                  // 有说明再放第三块）。一块展开 = 三块一起展开，不各管各的。
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      _ToolSection(title: '调用参数', text: _argumentsText),
                      _ToolSection(title: '调用结果', text: _resultText),
                      if (_noteText.isNotEmpty)
                        _ToolSection(title: '说明', text: _noteText),
                    ],
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 参数：能解析就缩进成多行（长 JSON 一行根本读不了）；解析不了就原样。
  String get _argumentsText {
    final raw = widget.call.arguments.trim();
    if (raw.isEmpty) return '（没有参数）';
    try {
      return const JsonEncoder.withIndent('  ').convert(jsonDecode(raw));
    } catch (_) {
      return raw;
    }
  }

  /// 结果：工具的输出；还没结果时给一句**如实**的状态（不拿失败原因冒充结果）。
  String get _resultText {
    final text = widget.call.resultText;
    if (text.isNotEmpty) return text;
    return switch (widget.call.status) {
      AiToolCallStatus.running => '（还在执行…）',
      AiToolCallStatus.pendingApproval => '（等你批准后才会执行）',
      AiToolCallStatus.denied => '（已拒绝，没有执行）',
      AiToolCallStatus.failed => '（失败，没有输出）',
      AiToolCallStatus.ok => '（没有输出）',
    };
  }

  /// 说明：失败 / 拒绝的原因（稳定错误码或消息）。
  ///
  /// 结果正文里已经原样带着的那句就不再重复（`tool_result.text` 常常就是整段错误）。
  String get _noteText {
    final note = widget.call.noteText;
    if (note.isEmpty) return '';
    final result = widget.call.resultText;
    if (result.isNotEmpty && result.contains(note)) return '';
    return note;
  }

  Future<void> _resolve({required bool approve}) async {
    setState(() => _busy = true);
    try {
      await (approve
          ? widget.onApprove(widget.call.callId)
          : widget.onDeny(widget.call.callId));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

class _ToolSection extends StatelessWidget {
  final String title;
  final String text;

  const _ToolSection({required this.title, required this.text});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: theme.dividerColor.withValues(alpha: 0.6)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 2),
          Text(text, style: theme.textTheme.bodySmall),
        ],
      ),
    );
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
      child: Text(
        label,
        style: theme.textTheme.labelSmall?.copyWith(color: color),
      ),
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

  /// 这一批里**被标记为成人内容**的那些（`comfy_submit` 声明了 `nsfw=true`）。
  ///
  /// 用户 bug 2026-09-20：模糊开关一开，回复里的产物卡也把所有图糊了 ——
  /// 只有 AI 明说过"这次是成人内容"的那几张才该糊。
  final List<int> nsfwMediaIds;

  /// 产出这些图的那份提示词 / 工作流（用户建议 ①）。
  final List<int> promptIds;

  final String Function(int mediaId) mediaThumbUrlOf;
  final String Function(int mediaId) mediaPosterUrlOf;

  const _ProducedMediaCard({
    required this.mediaIds,
    this.nsfwMediaIds = const [],
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
        color: theme.colorScheme.surfaceContainerHighest.withValues(
          alpha: 0.45,
        ),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: theme.dividerColor),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                mediaIds.isEmpty
                    ? Icons.account_tree_outlined
                    : Icons.photo_library_outlined,
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
                      child: NsfwBlur(
                        nsfw: nsfwMediaIds.contains(id),
                        revealKey: 'media:$id',
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
                                color:
                                    theme.colorScheme.surfaceContainerHighest,
                                child: Icon(
                                  Icons.image_not_supported_outlined,
                                  size: 18,
                                  color: theme.colorScheme.outline,
                                ),
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
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
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
      MaterialPageRoute<void>(
        builder: (_) => MediaDetailPage(mediaId: mediaId),
      ),
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
      blueprint: store.workflowBlueprint,
      saveBlueprint: (raw, name) => store.saveWorkflow(raw: raw, name: name),
    );
  }
}

/// 把"环节 + 它里面的工具卡"攒成组（[`_MessageBubble._orderedBody`] 内部用）。
///
/// 为什么要它：可折叠容器必须是**连续**的一块，而流是交错的（思考 / 正文 / 工具卡轮流来）。
/// 所以攒的是"**连续**到达的工具卡"这一串：
///  * 中间夹了正文 / 思考 → 先把已经攒下的交出去，**环节仍然开着**，后面再来的工具卡
///    另起一组并标「续」—— 这样"正文与工具卡的先后"永远等于它们到达的先后
///    （用户 bug⑥ 的纪律），同时这一环节的操作**一个都不会漏在外面**；
///  * 遇到下一个环节 → 关掉上一组，开新的一组。
class _StepCollector {
  _StepCollector({required this.out, required this.buildCard});

  /// 已经排好的块（环节组按它开组的**位置**插进来）。
  final List<Widget> out;

  /// 一张工具卡怎么画（由气泡提供，工具卡自己有批准 / 拒绝的回调）。
  final Widget Function(AiToolCallState call) buildCard;

  /// 当前环节名（`null` = 没有环节，工具卡按老样子直接排在正文里）。
  String? _phase;

  /// 下一组是不是"同一环节的续"（中途夹过正文 / 思考就会是）。
  bool _continued = false;

  final List<AiToolCallState> _calls = [];

  /// 正文 / 思考段之前要调：把已经攒下的工具卡先交出去（否则它们会被排到正文后面）。
  void breakRun() {
    flush();
    if (_phase != null) _continued = true;
  }

  /// 一个环节开始（`step` 块 / `step.started` 事件）。
  void open(String phase) {
    flush();
    _phase = phase;
    _continued = false;
  }

  /// 一张工具卡：有开着的环节就收进去，否则直接排在正文里（老样子）。
  void addCall(AiToolCallState call) {
    if (_phase == null) {
      out.add(buildCard(call));
      return;
    }
    _calls.add(call);
  }

  /// 把攒着的那一组交出去（**没有工具卡的环节不画组** ——
  /// 一个空组头只会白占一行，模型"说完环节名就去写正文"是正常用法）。
  void flush() {
    final phase = _phase;
    if (phase == null || _calls.isEmpty) {
      _calls.clear();
      return;
    }
    out.add(
      _StepGroup(
        phase: phase,
        continued: _continued,
        calls: List<AiToolCallState>.of(_calls),
        buildCard: buildCard,
      ),
    );
    _calls.clear();
  }
}

/// 一个**关键步骤（环节）组**：环节头 + 这一环节里的操作（工具卡），默认折叠。
///
/// 用户建议 2026-09-28：「关键步骤之间 AI 的操作可以再折叠（关键步骤名显示这是哪个粗略环节，
/// 比如正在分析请求这种，做个约束）」。环节名由**后端**从 `[步骤]…[/步骤]` 里摘出来，
/// 只可能是那六个固定名字（见 `server-rs/src/step_mark.rs`）—— 前端这里不再做任何猜名。
///
/// 三条纪律：
///  ① **默认折叠**（与"思考过程"同一条：过程让位给正文），但收起时**不是藏起来** ——
///     标题上带"N 个操作"，一眼能看出这里做过几件事；
///  ② **有东西等用户处理时不许折叠**（等批准的 / 失败的 / 被拒的）：批准按钮躲进折叠区，
///     用户就永远不知道该点哪里（那是"AI 卡住了"的观感）；
///  ③ 同一环节名的第二次开组标「续」，让人看出这是同一个环节的第二段操作（见 [`_StepCollector`]）。
class _StepGroup extends StatefulWidget {
  final String phase;
  final bool continued;
  final List<AiToolCallState> calls;
  final Widget Function(AiToolCallState call) buildCard;

  const _StepGroup({
    required this.phase,
    required this.continued,
    required this.calls,
    required this.buildCard,
  });

  @override
  State<_StepGroup> createState() => _StepGroupState();
}

class _StepGroupState extends State<_StepGroup> {
  /// 用户自己点开的（默认 false = 折叠）。
  bool _userExpanded = false;

  /// 这一组里有没有"等用户处理"的东西（处置见类注释第 ② 条）。
  bool get _hasPendingWork => widget.calls.any(
    (c) =>
        c.needsApproval ||
        c.status == AiToolCallStatus.failed ||
        c.status == AiToolCallStatus.denied,
  );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.outline,
    );
    final expanded = _hasPendingWork || _userExpanded;
    final header = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(
          _hasPendingWork
              ? Icons.priority_high
              : (_userExpanded ? Icons.expand_less : Icons.expand_more),
          size: 16,
          color: _hasPendingWork
              ? theme.colorScheme.error
              : theme.colorScheme.outline,
        ),
        Icon(Icons.route_outlined, size: 14, color: theme.colorScheme.outline),
        const SizedBox(width: 4),
        Text(widget.phase, style: label),
        if (widget.continued) Text('（续）', style: label),
        const SizedBox(width: 6),
        Text(
          _hasPendingWork
              ? '${widget.calls.length} 个操作 · 有等你处理的'
              : '${widget.calls.length} 个操作',
          style: label?.copyWith(
            color: _hasPendingWork
                ? theme.colorScheme.error
                : theme.colorScheme.outline,
          ),
        ),
      ],
    );
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 有东西等处理时**不给点**：点也只会立刻弹回来，反而让人以为按钮坏了
          _hasPendingWork
              ? Tooltip(message: '这一组里有等你处理的调用，不能折叠', child: header)
              : InkWell(
                  onTap: () => setState(() => _userExpanded = !_userExpanded),
                  child: header,
                ),
          if (expanded)
            // 收起时**整块不建**（与"思考过程"同一条：展开的 20 张工具卡不该留在树里）
            Padding(
              padding: const EdgeInsets.only(left: 15),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (final call in widget.calls) widget.buildCard(call),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// 思考过程：**默认折叠**（用户要求："思考过程默认自动折叠"）。
///
/// 收起时**不是简单藏起来** —— 显示模型思考的摘要（压成两行）：
/// **生成中取末尾一行**（看"现在想到哪了"），**结束后取开头**（看"这段在讲什么"）。
/// 展开后给完整正文，可滚动、可选中复制。
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

  /// 收起时的摘要：压掉换行，取**开头**一段；末尾还有内容就加省略号。
  ///
  /// **只扫一小段**：思考正文可能是几万字，而收起时最多显示两行 ——
  /// 全文过一遍正则（还每帧一次）纯属白花钱，流式期间这就是"卡顿"的一部分。
  ///
  /// [fromEnd] = true 时取**末尾一行**（流式生成中用户要看"现在想到哪了"）；
  /// 生成结束后再回到开头（用户要看"这段思考在讲什么"）。取末尾同样只看最后
  /// `limit * 4` 个字符，不扫全文。
  static String summaryOf(
    String text, {
    int limit = 160,
    bool fromEnd = false,
  }) {
    const lookahead = 4; // 多扫几倍，保证压掉换行后仍够 limit 个字符
    final cut = limit * lookahead;
    if (fromEnd) {
      var tail = text;
      if (text.length > cut) {
        var start = text.length - cut;
        // 别把代理对（emoji）切成半个
        if (start > 0 && _isLowSurrogate(text.codeUnitAt(start))) start += 1;
        tail = text.substring(start);
      }
      // 只认最后一条**非空**行（流式时常见末尾就是一个还没写完的换行）
      final lines = tail
          .split('\n')
          .where((line) => line.trim().isNotEmpty)
          .toList();
      var flat = (lines.isEmpty ? tail : lines.last)
          .replaceAll(RegExp(r'\s+'), ' ')
          .trim();
      if (flat.length <= limit) return flat;
      var start = flat.length - limit;
      if (_isLowSurrogate(flat.codeUnitAt(start))) start += 1;
      return '…${flat.substring(start)}';
    }
    var head = text;
    if (text.length > cut) {
      var end = cut;
      // 别把代理对（emoji）切成半个
      if (end > 0 && _isLowSurrogate(text.codeUnitAt(end))) end -= 1;
      head = text.substring(0, end);
    }
    // 生成结束后要的是"这段在讲什么"：取**第一行**（跳过开头的空行）。
    final lines = head
        .split('\n')
        .where((line) => line.trim().isNotEmpty)
        .toList();
    final first = (lines.isEmpty ? head : lines.first)
        .replaceAll(RegExp(r'\s+'), ' ')
        .trim();
    if (first.length <= limit) return first;
    var end = limit;
    if (_isLowSurrogate(first.codeUnitAt(end))) end -= 1;
    return '${first.substring(0, end)}…';
  }

  static bool _isLowSurrogate(int unit) => unit >= 0xDC00 && unit <= 0xDFFF;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.outline,
    );
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
                Icon(
                  _expanded ? Icons.expand_less : Icons.expand_more,
                  size: 16,
                  color: theme.colorScheme.outline,
                ),
                Icon(
                  Icons.psychology_outlined,
                  size: 14,
                  color: theme.colorScheme.outline,
                ),
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
              // 收起时**整段不建**（不是 Opacity/Offstage）：长思考的文本量不小，
              // 折叠状态不该还把它留在 widget 树里。
              child: _StreamingReasoning(
                text: widget.text,
                streaming: widget.streaming,
              ),
            )
          else if (widget.text.trim().isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(left: 20, top: 2),
              child: Tooltip(
                message: '点击展开完整思考过程',
                child: Text(
                  // 生成中看"末尾一行"（现在想到哪了）；结束后看开头（这段在讲什么）。
                  summaryOf(widget.text, fromEnd: widget.streaming),
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

/// 展开后的完整思考正文：可选中复制（长度不受摘要限制）、流式期间贴底跟随。
///
/// 单独包一层是为了让"折叠时完全不构建它"这个行为一眼可见（见 `_ReasoningPanel`）。
///
/// 用普通 `Text` 而不是 `SelectableText`：思考与正文同在一个 `SelectionArea` 里，
/// 内层再开一个选择域的话，拖选到思考段末尾就拉不到下面的正文了（用户 bug ②）。
///
/// 这里解决用户 2026-09-19 报的两个问题：
///
///  ① **展开正在持续流式输出的思考过程后卡顿**：以前整段思考是一个 `Text`，
///     每来一个字都要把**几万字**重新排版一遍（一帧几十毫秒，直接掉帧）。
///     现在按固定行数切成小块、**没变过的块复用上一次的 widget**
///     （widget 相同 → Flutter 整棵子树跳过，`RenderParagraph` 连布局都不重做），
///     于是每帧只重排"最后那一块"。
///  ② **展开后无法及时刷新**：内层滚动条停在自己原来的位置，新思考长在可视区下面，
///     看起来像"不刷新"。现在流式期间和聊天列表同一套纪律 —— **贴底才跟随**，
///     用户自己往上翻了就不打扰。
class _StreamingReasoning extends StatefulWidget {
  final String text;

  /// 这一轮还在生成：贴着底就跟，不贴底就不动。
  final bool streaming;

  const _StreamingReasoning({required this.text, this.streaming = false});

  /// 每多少行合成一块。
  ///
  /// 块越大 → widget 越少，但"最后一块"越长、每帧重排越多；块越小 → 复用率越高，
  /// 但一棵几十万字符的思考会碎成上千个 `RenderParagraph`。8 行是折中值。
  static const int linesPerBlock = 8;

  /// 把思考正文切成小块（按行分组，最后一块可能还在长）。
  static List<String> splitBlocks(String text) {
    if (text.isEmpty) return const [];
    final lines = text.split('\n');
    final blocks = <String>[];
    for (var i = 0; i < lines.length; i += linesPerBlock) {
      final end = (i + linesPerBlock).clamp(0, lines.length);
      blocks.add(lines.sublist(i, end).join('\n'));
    }
    return blocks;
  }

  @override
  State<_StreamingReasoning> createState() => _StreamingReasoningState();
}

class _StreamingReasoningState extends State<_StreamingReasoning> {
  final _scroll = ScrollController();

  /// 上一帧的块划分与它们的 widget：逐块比较，只重建变过的那块。
  List<String> _blocks = const [];
  List<Widget> _rendered = const [];

  /// 上一次渲染的正文：判断"这一帧文本真的变了吗"（没变就不必再跟一次底）。
  String _lastText = '';

  /// 视口是不是贴着底部：**只有贴底才跟随**（和聊天列表同一条纪律）。
  bool _atBottom = true;

  /// 贴底容差：只吸收亚像素误差，不是"接近底部"（聊天列表那边踩过同样的坑，
  /// 见 `_AiHomePageState._bottomEpsilon` 的注释）。
  static const double _bottomEpsilon = 1.5;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_onScroll);
  }

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  void _onScroll() {
    if (!_scroll.hasClients) return;
    _atBottom =
        _scroll.position.maxScrollExtent - _scroll.position.pixels <=
        _bottomEpsilon;
  }

  /// 贴底时把内层视口拉到最新一行（`jumpTo` 而不是动画：这是"跟上"不是"滚一段"）。
  void _followIfAtBottom() {
    if (!mounted || !_atBottom || !_scroll.hasClients) return;
    _scroll.jumpTo(_scroll.position.maxScrollExtent);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.outline,
    );

    // 逐块复用：内容一样的块直接把上一帧的 widget 交回去（见类注释 ①）
    final next = _StreamingReasoning.splitBlocks(widget.text);
    final rendered = <Widget>[];
    for (var i = 0; i < next.length; i++) {
      if (i < _blocks.length && _blocks[i] == next[i] && i < _rendered.length) {
        rendered.add(_rendered[i]);
      } else {
        rendered.add(Text(next[i], style: style));
      }
    }
    _blocks = next;
    _rendered = rendered;

    if (widget.streaming && _lastText != widget.text) {
      _lastText = widget.text;
      // 布局完成后才知道新的 maxScrollExtent，所以跟随放在帧后
      WidgetsBinding.instance.addPostFrameCallback((_) => _followIfAtBottom());
    }

    return SingleChildScrollView(
      controller: _scroll,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: rendered,
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
              Icon(
                Icons.auto_awesome_outlined,
                size: 40,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(height: 12),
              Text('AI 工作台', style: theme.textTheme.titleLarge),
              const SizedBox(height: 6),
              Text(
                '生图 / 生视频需求的澄清、提示词设计与进度跟进。\n'
                '输入 / 可以调出 Skills 目录；也可以把文件拖进来当附件。',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: theme.colorScheme.outline,
                ),
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

  /// 拖进窗口的文件（原生 `WM_DROPFILES` → `comfyhub/file_drop`）。取消订阅用。
  StreamSubscription<List<String>>? _dropSub;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onControllerChanged);
    // 拖进来的文件**挂成附件**（用户 2026-10-18："实现拖拽进窗口（AI工作台）"）。
    //
    // ⚠ 听的是**全局广播流**，不是"输入框上的拖放区域"：Flutter 框架没有
    // "外部文件拖进窗口"的 API（`Draggable`/`DragTarget` 只传应用内的数据），
    // 真正收到文件的是**顶层窗口**的 `WM_DROPFILES`（原生那半边）。
    // 所以"拖到哪里"这件事在原生侧根本不区分 —— 拖进窗口任意位置都算，
    // 这正好也符合直觉（整个工作台都是投放区）。
    if (FileDrop.supported) {
      _dropSub = FileDrop.stream.listen(_attachDropped);
    }
  }

  @override
  void dispose() {
    _dropSub?.cancel();
    _cardSearch.dispose();
    widget.controller.removeListener(_onControllerChanged);
    super.dispose();
  }

  /// 一批拖进来的路径 → 挂成附件。
  ///
  /// 与 `Ctrl+V` 走的是**同一个** `attachFiles`，所以"拖进来"和"粘进来"的
  /// 附件形状、去重、上限完全一致 —— 不为拖放另开一条准入路径。
  void _attachDropped(List<String> paths) {
    if (!mounted || paths.isEmpty) return;
    unawaited(
      widget.store.attachFiles([
        for (final path in paths) (name: _fileNameOf(path), path: path),
      ]),
    );
  }

  /// `/` 菜单：输入区里最后一段要是 `/xxx`（行首或空格后）才弹出来。
  void _onControllerChanged() {
    final text = widget.controller.text;
    final match = RegExp(r'(?:^|\s)/([^\s/]*)$').firstMatch(text);
    final next = match?.group(1);
    // 键盘路径：行末输入 `@` 直接开卡片面板（按钮收在 "+" 菜单里；直接打字也该有入口）。
    if (!_cardsOpen && RegExp(r'(?:^|\s)@$').hasMatch(text)) {
      _openCards();
    }
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

  // -----------------------------------------------------------------------
  //  卡片（角色卡 / 画风卡）：输入区的 `@` 菜单
  // -----------------------------------------------------------------------

  /// 这一条消息要带上的两张卡（各一张；再选同类会替换）。
  ///
  /// 发送时被拼成两段固定标记的文本（`[STYLE SPEC]` / `[CHARACTER CARD]`），
  /// 正文**逐字**带上 —— skill `r2i-prompt` 认的就是这两个槽位。
  CharacterCard? _characterCard;
  StyleCard? _styleCard;

  /// 卡片面板是否展开；null = 收起。
  bool _cardsOpen = false;

  /// 当前页签：false = 角色卡，true = 画风卡。
  bool _cardsStyled = false;

  bool _cardsLoading = false;
  String? _cardsError;
  String _cardQuery = '';
  final _cardSearch = TextEditingController();
  List<CharacterCard> _characters = const [];
  List<StyleCard> _styles = const [];

  /// 打开面板并（第一次）拉卡片库：**一次拉两类**，页签切换不再打请求。
  ///
  /// 失败不冒充"没有卡"：面板里如实显示错误（[`_cardsError`]）。
  Future<void> _openCards({bool style = false}) async {
    setState(() {
      _cardsOpen = true;
      _cardsStyled = style;
      _cardsError = null;
    });
    if (_characters.isNotEmpty || _styles.isNotEmpty || _cardsLoading) return;
    setState(() => _cardsLoading = true);
    try {
      final characters = await widget.store.listCharacterCards();
      final styles = await widget.store.listStyleCards();
      if (!mounted) return;
      setState(() {
        _characters = characters;
        _styles = styles;
        _cardsLoading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _cardsLoading = false;
        _cardsError = '读不到卡片库：$e';
      });
    }
  }

  void _closeCards() {
    setState(() {
      _cardsOpen = false;
      _cardQuery = '';
      _cardSearch.clear();
    });
  }

  void _chooseCard(CharacterCard card) {
    if ((card.prompt ?? '').trim().isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('这张卡的正面提示词还空着：先去「预设库 → 卡片库」补上再用。'),
        ),
      );
      return;
    }
    setState(() {
      if (card is StyleCard) {
        _styleCard = card;
      } else {
        _characterCard = card;
      }
      _cardsOpen = false;
      _cardQuery = '';
      _cardSearch.clear();
    });
    // 从键盘 `@` 进来的那条路：把行末那个查询词摘掉，只留用户自己写的正文。
    final text = widget.controller.text;
    final trigger = RegExp(r'(?:^|\s)@[^\s@]*$').firstMatch(text);
    if (trigger != null) {
      final next = text.substring(0, trigger.start).trimRight();
      widget.controller.value = TextEditingValue(
        text: next,
        selection: TextSelection.collapsed(offset: next.length),
      );
    }
    widget.focusNode.requestFocus();
  }

  /// 把选中的卡拼到消息前面。
  ///
  /// 槽位名与 skill `r2i-prompt` 的输入约定逐字一致；卡正文**一个字符都不改**。
  String _composeOutgoing(String text) {
    final blocks = <String>[];
    final style = _styleCard;
    if (style != null) {
      blocks.add('[STYLE SPEC]\n${style.name}\n${style.prompt}');
    }
    final character = _characterCard;
    if (character != null) {
      blocks.add('[CHARACTER CARD]\n${character.name}\n${character.prompt}');
    }
    final body = text.trim();
    if (blocks.isEmpty) return text;
    if (body.isEmpty) return blocks.join('\n\n');
    return '${blocks.join('\n\n')}\n\n$body';
  }

  /// 菜单里列出的 skills：只列**用户可调用**且启用的（`/` 就是用户显式调用）。
  List<AiSkill> get _slashMatches {
    final query = _slashQuery;
    if (query == null) return const [];
    final all = widget.store.skills
        .where((s) => s.enabled && s.userInvocable)
        .toList();
    if (query.isEmpty) return all;
    final q = query.toLowerCase();
    return all
        .where(
          (s) =>
              s.name.toLowerCase().contains(q) ||
              s.description.toLowerCase().contains(q),
        )
        .toList();
  }

  /// 把 `/xxx` 换成 `/skill 名 `，模型看到的就是这个 skill 名。
  void _insertSlash(AiSkill skill) {
    final text = widget.controller.text;
    final match = RegExp(r'(?:^|\s)/([^\s/]*)$').firstMatch(text);
    if (match == null) return;
    final slash = text.lastIndexOf('/');
    final next = '${text.substring(0, slash)}/${skill.name} ';
    widget.controller.value = TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: next.length),
    );
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
          if (store.error != null)
            _Banner(text: store.error!, icon: Icons.error_outline),
          if (blockers.isNotEmpty)
            _Banner(
              text:
                  '附件未通过准入，发送会被阻断：\n${blockers.map((b) => '· $b').join('\n')}',
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
                  color: theme.colorScheme.surfaceContainerHighest.withValues(
                    alpha: 0.4,
                  ),
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
                        title: Text(
                          '/${skill.name}',
                          style: theme.textTheme.labelMedium,
                        ),
                        subtitle: Text(
                          skill.oneLine.isEmpty
                              ? skill.sourceLabel
                              : skill.oneLine,
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
          // 卡片面板（角色卡 / 画风卡）：在输入框上方，和 `/` 菜单同一套位置。
          if (_cardsOpen) _cardsPanel(theme),
          if (_characterCard != null || _styleCard != null) _cardChips(theme),
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
              child: Focus(
                // Ctrl+V 粘附件（用户 2026-10-18："ctrl+v不能粘贴附件"）。
                //
                // ⚠ 为什么把 handler 放在**这里**（紧贴 TextField 外面）：
                // 按键是从焦点节点**往上冒泡**的，而"粘文本"是 `DefaultTextEditingShortcuts`
                // 挂在很上面的 `Shortcuts` 里处理的。我的节点只要比它更靠近 TextField，
                // 就一定能先看到 Ctrl+V。
                //
                // `canRequestFocus: false`：它只当"按键监听器"用，**不许抢文本框的焦点**。
                canRequestFocus: false,
                skipTraversal: true,
                onKeyEvent: _onComposerKey,
                child: TextField(
                  controller: widget.controller,
                  focusNode: widget.focusNode,
                  minLines: 2,
                  maxLines: 8,
                  textInputAction: TextInputAction.newline,
                  // ⚠ 右键「粘贴」必须换成我们自己的那条，否则它只读文本、
                  // 会把"文件路径"当内容贴进来（见 `_composerContextMenu`）
                  contextMenuBuilder: _composerContextMenu,
                  decoration: const InputDecoration(
                    hintText: '描述你的生图需求；/ 调用 Skill，@ 插入角色卡 / 画风卡（Enter 发送，Shift+Enter 换行）',
                    border: OutlineInputBorder(),
                    isDense: true,
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          // 底行排版。四条用户反馈累积到这里，规矩已经收敛成两句：
          //   · **一条放不下就折行**，绝不横向滚动；
          //   · **操作控件靠左；只有发送（宽屏再加 token 汇总）贴右**。
          //
          //   ① token 汇总被挤没 + 发送按钮不贴右（2026-09-20）：汇总当时是 loose 的
          //      `Flexible`，用不满的那截空白落在**按钮右边**，按钮就不贴右；它和
          //      `Spacer` 还各占 1 flex，窗口一窄先被省略。→ 汇总固定占位（≤220，
          //      只因为自己太长才省略），发送放最后、不参与压缩。
          //   ② 控件显示不全（2026-09-20）：操作组曾经是"唯一的 `Expanded` + 横向
          //      滚动"，窄窗口下视口只剩 21px（内容 847px），权限档 / 模型选择被从
          //      **左边缘**裁掉。→ 换成折行。
          //   ③ 折行本身也得好看（用户 2026-09-21："你不觉得这个排版很奇怪吗"）：
          //      把"附件(40) / 权限档(187) / 模型(296 撑满) / 思考(70)"直接折，
          //      结果是**三行**、每行右边一大片空白、权限档孤零零挂在第一行。
          //      根因是每个控件的固有宽度差一个数量级。→ 窄的时候这四枚一律退成
          //      **同宽的图标按钮**（各 ~48px）：一行放得下就一行，放不下再折行，
          //      每一行都是整齐的等宽图标。
          //
          //   ④ 整排贴右（用户 2026-09-24，手机）：窄屏那一排曾经是**一个
          //      `Wrap(alignment: end)`**（四枚控件 + 发送挤在一起），于是整排一起贴右，
          //      "附件"左边空出一大片。→ 窄屏也拆成 `Row`：左 `Expanded`（start 对齐）、
          //      右发送，见下面那个分支的注释。
          //
          // 宽屏右侧那两件（token 汇总 + 发送）走同一个 `Wrap`（end 对齐 = 整体贴右，
          // 换行后最后一行也贴右）。
          LayoutBuilder(
            builder: (context, constraints) {
              // 三档（2026-10-11 用户："按钮太小了比较难点，也没显示出有用的关键信息；
              // 在其他特定宽度下，控件变成两行排列也很难看"）：
              //
              //   ① ≥ [_fullLabelWidth]：控件带**全称**（"自动允许（无需批准）"、模型名 + 能力徽标）
              //   ② [_shortLabelWidth] ~ [_fullLabelWidth]：控件带**短标签**
              //      （"自动"/"询问"、"默认"/"高"）+ 模型名 —— 关键信息都还在，只是压掉了长文案
              //   ③ < [_shortLabelWidth]：控件退成**图标按钮**（信息进 tooltip，一个都不丢）
              //
              // 为什么以前会难看：老版只有"全称 / 图标"两档，而四枚控件的**固有宽度差一个
              // 数量级**（44 / 187 / 296 / 70），于是中等宽度下 `Wrap` 一折就是
              // **两行 + 每行右边一大片空白 + 权限档孤零零挂一行**。
              // 短标签把四枚压到 ~300px 以内，一档就能和发送挤在一排 —— 折行那件事不发生。
              //
              // ⚠ 折行仍然保留（`FilterChipRow` 是 `Wrap`）：那是**最后一道保险**，
              // 不是主要排版手段；真折了也不会被裁掉任何控件。
              final density = constraints.maxWidth >= _fullLabelWidth
                  ? _ComposerLabel.full
                  : constraints.maxWidth >= _shortLabelWidth
                  ? _ComposerLabel.short
                  : _ComposerLabel.icon;
              // token 汇总是**纯展示项**，窄了先让它走（用户 2026-09-21：
              // "宽度太窄的时候可以不显示对话 token 用量"）—— 但它不该在中等宽度就消失，
              // 所以门限单独一档。
              final showSummary = constraints.maxWidth >= _summaryWidth;
              return Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  // 操作组自己折行（宽裕时只占一行，不会把右边的汇总 / 发送顶出去）
                  Expanded(
                    child: FilterChipRow(
                      key: const Key('ai-composer-actions'),
                      alignment: WrapAlignment.start,
                      spacing: 4,
                      runSpacing: 4,
                      children: _composerActions(density),
                    ),
                  ),
                  if (showSummary) ..._composerSummary(store, theme),
                  _sendButton(store),
                ],
              );
            },
          ),
        ],
      ),
    );
  }

  /// 换成"短标签"档的门限（**操作组自己的可用宽度**，不是窗口宽度）。
  ///
  /// 实测（本机探针，2026-10-11）：1200 窗口 → 操作组可用 **674**；960 → 595；1600 → 1074。
  /// 全称档四枚的固有宽度约 44 + 190 + 300 + 80 ≈ 614 再加组内间距 ≈ 690 ——
  /// 所以 **720** 是"放得下全称"的下界：674（1200 窗口）放不下，正好该收成短标签；
  /// 1074（1600 窗口）放得下。
  ///
  /// ⚠ 门限要跟着**实测**走：以前凭印象写 780，结果 1600 窗口被误判（好在 1074 > 780
  /// 侥幸没红），而 1200 窗口那些用例的断言全对不上 —— 判据错一点，
  /// 表现就是"某个宽度下界面突然换了个样子"，而单测只看一个宽度时抓不到。
  static const double _fullLabelWidth = 720;

  /// 换成"纯图标"档的门限：短标签档四枚约 300 + 发送 94 + 间距 ≈ 410。
  static const double _shortLabelWidth = 440;

  /// 低于这个宽度就不显示 token 汇总（纯展示项让位给能操作的控件）。
  static const double _summaryWidth = 560;

  /// 输入区底行的操作控件（不含发送）。
  ///
  /// 顺序就是用户建议 ⑤ 定的：附件 → 权限档 → 模型选择 → 思考强度。
  ///
  /// [label] 是**三档文字密度**（见 `build` 里那个 `LayoutBuilder` 的注释）：
  /// 全称 / 短标签 / 纯图标。用户 2026-10-11 的原话是
  /// "按钮太小了比较难点，也没显示出有用的关键信息" ——
  /// 所以短标签档**保留文字**（"自动"/"询问"、"默认"/"高"），只压掉长文案；
  /// 纯图标档才退回 tooltip，且图标按钮本身放大到能轻松点到。
  List<Widget> _composerActions(_ComposerLabel density) {
    return [
      // 附件 + 卡片合成一个 "+" 菜单（用户 2026-10-24 指着 DSH 的排版问"放得下实体 @ 按钮吗"：
      // 320~720 宽时底行已经有 4 枚状态控件 + 发送，再塞第 5 枚就必然折行 / 挤压）。
      // 收起一层之后底行仍是 4 枚，入口还在最左；键盘路径另外留了直接输入 `@`。
      AppMenuButton<String>(
        onSelected: (v) {
          if (v == 'attach') _pickFiles();
          if (v == 'cards') _openCards();
        },
        options: [
          const MenuOption(
            value: 'attach',
            icon: Icons.attach_file,
            label: '添加附件',
          ),
          // 卡片库只有桌面后端有（手机侧没有这两张表）。
          if (!widget.store.isLocal)
            const MenuOption(
              value: 'cards',
              icon: Icons.alternate_email,
              label: '角色卡 / 画风卡…',
              dividerBefore: true,
            ),
        ],
        button: (context, controller, isOpen) => IconButton(
          tooltip: '添加附件 / 插入卡片（@）',
          onPressed: () => controller.isOpen ? controller.close() : controller.open(),
          iconSize: 22,
          padding: const EdgeInsets.all(10),
          constraints: const BoxConstraints(minWidth: 44, minHeight: 44),
          icon: const Icon(Icons.add),
        ),
      ),
      // 权限档（用户建议 ⑤）：就在附件按钮和模型选择之间
      _PermissionPicker(store: widget.store, density: density),
      _ModelPicker(store: widget.store, density: density),
      _EffortPicker(store: widget.store, density: density),
    ];
  }

  /// 卡片面板：搜索 + 角色卡 / 画风卡两个页签 + 列表。
  ///
  /// 位置与 `/` 菜单一致（输入框上方），不铺遮罩：同一条理由 —— 弹出层不该吃掉
  /// 整页的滚轮 / 拖动（`AppMenuButton` 那条约定的同源问题）。
  Widget _cardsPanel(ThemeData theme) {
    final cards = _cardsStyled ? _styles : _characters;
    final query = _cardQuery.trim().toLowerCase();
    final shown = query.isEmpty
        ? cards
        : cards
              .where(
                (c) =>
                    '${c.name} ${c.prompt ?? ''}'.toLowerCase().contains(query),
              )
              .toList();
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Material(
        color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
          side: BorderSide(color: theme.dividerColor),
        ),
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          height: 250,
          child: Column(
            children: [
              Row(
                children: [
                  _cardsTab(theme, '角色卡', !_cardsStyled, () {
                    setState(() {
                      _cardsStyled = false;
                      _cardQuery = '';
                      _cardSearch.clear();
                    });
                  }),
                  _cardsTab(theme, '画风卡', _cardsStyled, () {
                    setState(() {
                      _cardsStyled = true;
                      _cardQuery = '';
                      _cardSearch.clear();
                    });
                  }),
                  const Spacer(),
                  if (_cardsLoading)
                    const Padding(
                      padding: EdgeInsets.only(right: 8),
                      child: SizedBox(
                        width: 14,
                        height: 14,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    ),
                  IconButton(
                    tooltip: '收起卡片面板',
                    onPressed: _closeCards,
                    icon: const Icon(Icons.close, size: 18),
                  ),
                ],
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(10, 0, 10, 6),
                child: TextField(
                  controller: _cardSearch,
                  decoration: const InputDecoration(
                    isDense: true,
                    prefixIcon: Icon(Icons.search, size: 18),
                    hintText: '搜卡片名 / 正文',
                    border: OutlineInputBorder(),
                  ),
                  onChanged: (v) => setState(() => _cardQuery = v),
                ),
              ),
              Expanded(child: _cardsBody(theme, shown)),
            ],
          ),
        ),
      ),
    );
  }

  Widget _cardsTab(
    ThemeData theme,
    String label,
    bool selected,
    VoidCallback onTap,
  ) => TextButton(
    onPressed: onTap,
    style: TextButton.styleFrom(
      minimumSize: const Size(0, 34),
      padding: const EdgeInsets.symmetric(horizontal: 12),
      foregroundColor: selected
          ? theme.colorScheme.primary
          : theme.colorScheme.outline,
      textStyle: theme.textTheme.labelMedium,
    ),
    child: Text(label),
  );

  Widget _cardsBody(ThemeData theme, List<CharacterCard> shown) {
    if (_cardsError != null) {
      return Padding(
        padding: const EdgeInsets.all(12),
        child: Text(
          _cardsError!,
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.error,
          ),
        ),
      );
    }
    if (_cardsLoading) {
      return const Center(
        child: SizedBox(
          width: 18,
          height: 18,
          child: CircularProgressIndicator(strokeWidth: 2),
        ),
      );
    }
    if (shown.isEmpty) {
      return Center(
        child: Text(
          _cardQuery.trim().isEmpty
              ? '还没有${_cardsStyled ? '画风卡' : '角色卡'}：去「预设库 → 卡片库」新建一张。'
              : '没有匹配的卡片',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
      );
    }
    return ListView.builder(
      padding: EdgeInsets.zero,
      itemCount: shown.length,
      itemBuilder: (context, i) {
        final card = shown[i];
        final selected = identical(
          card,
          _cardsStyled ? _styleCard : _characterCard,
        );
        return ListTile(
          dense: true,
          visualDensity: VisualDensity.compact,
          leading: Icon(
            _cardsStyled ? Icons.palette_outlined : Icons.badge_outlined,
            size: 18,
          ),
          title: Text(
            card.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelMedium,
          ),
          subtitle: Text(
            (card.prompt ?? '（正面提示词还空着）').replaceAll('\n', ' '),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall,
          ),
          trailing: selected
              ? Icon(Icons.check, size: 18, color: theme.colorScheme.primary)
              : null,
          onTap: () => _chooseCard(card),
        );
      },
    );
  }

  /// 已选中的卡：输入框上方一排 chip，点 × 取消。
  Widget _cardChips(ThemeData theme) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Wrap(
        spacing: 8,
        runSpacing: 6,
        children: [
          if (_styleCard != null)
            InputChip(
              avatar: Icon(
                Icons.palette_outlined,
                size: 16,
                color: theme.colorScheme.primary,
              ),
              label: Text(
                '画风卡 · ${_styleCard!.name}',
                style: theme.textTheme.labelSmall,
              ),
              visualDensity: VisualDensity.compact,
              onDeleted: () => setState(() => _styleCard = null),
            ),
          if (_characterCard != null)
            InputChip(
              avatar: Icon(
                Icons.badge_outlined,
                size: 16,
                color: theme.colorScheme.primary,
              ),
              label: Text(
                '角色卡 · ${_characterCard!.name}',
                style: theme.textTheme.labelSmall,
              ),
              visualDensity: VisualDensity.compact,
              onDeleted: () => setState(() => _characterCard = null),
            ),
        ],
      ),
    );
  }

  /// 本对话的 token 汇总（AIH-057）：一直是可见的，不必翻设置。
  ///
  /// 固定占位（不吃 flex），所以它只会因为**自己太长**而省略，不会被别人挤掉。
  List<Widget> _composerSummary(AiWorkspaceStore store, ThemeData theme) {
    if (store.usageSummary.isEmpty) return const [];
    final label =
        '本对话 ↑${AiTokenUsage.compact(store.usageSummary.inputTokens)} '
        '↓${AiTokenUsage.compact(store.usageSummary.outputTokens)}';
    return [
      ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 220),
        child: Tooltip(
          message: store.usageSummary.label,
          child: Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.right,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        ),
      ),
      const SizedBox(width: 12),
    ];
  }

  Widget _sendButton(AiWorkspaceStore store) => FilledButton.icon(
    // 组字（拼音还没上屏）期间不给发：这时候发出去的是半截拼音，
    // 而且"清空输入框"正好踩在 Windows 输入法回灌旧文本的触发点上。
    onPressed: store.sending
        ? () => store.stop()
        : (store.canSend && !_wasComposing ? _submit : null),
    icon: Icon(store.sending ? Icons.stop : Icons.send, size: 18),
    label: Text(store.sending ? '停止' : '发送'),
  );

  void _submit() {
    // 组字（拼音还没上屏）期间的回车是"选词 / 上屏"，不是发送（AIH-053）；
    // 按钮走同一条路，所以闸放在这里，两条入口都拦得住。
    if (_composing) return;
    // 选中的卡拼在正文前面（槽位标记逐字固定，卡正文不改）。
    final text = _composeOutgoing(widget.controller.text);
    final sendable = widget.store.sendBlockReason() == null;
    widget.onSubmit(text);
    // 只有"真的会被发出去"才清掉卡片：被拒时（正在生成 / 附件还在上传）卡片要留着，
    // 不能因为按了一次回车就没了。
    if (sendable && (_characterCard != null || _styleCard != null)) {
      setState(() {
        _characterCard = null;
        _styleCard = null;
        _cardsOpen = false;
      });
    }
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

  /// `Ctrl+V`：剪贴板里是**文件**或**一张图**就吃成附件，都不是就把这次粘贴还回去。
  ///
  /// 输入框的右键菜单：把内置的「粘贴」换成**我们自己的**那条。
  ///
  /// ⚠ 为什么必须有它 —— **Ctrl+V 与右键「粘贴」是两条完全不同的路**：
  ///
  /// * `Ctrl+V` 被 [`_onComposerKey`] 拦下来，读剪贴板里的 `CF_HDROP` → 挂成附件；
  /// * 右键那条走 `EditableText` 内置的粘贴动作，**只读 `CF_UNICODETEXT`**。
  ///
  /// 偏偏 [`FileClipboard.writeFiles`] 为了让记事本 / 聊天框之类也能粘，
  /// **故意**额外写了一份文本兜底 —— 而那份文本就是**路径串本身**。
  /// 于是"复制一个文件、右键粘贴"粘出来的是文件路径（用户 2026-10-18 报的）。
  /// 换掉这一项之后，两条路都走同一个 `_pasteFilesOrText`：有文件 / 图挂附件、都没有才粘文本。
  Widget _composerContextMenu(BuildContext context, EditableTextState state) {
    final items = withReplacedPaste(state.contextMenuButtonItems, () {
      state.hideToolbar();
      unawaited(_pasteFilesOrText());
    });
    return AdaptiveTextSelectionToolbar.buttonItems(
      anchors: state.contextMenuAnchors,
      buttonItems: items,
    );
  }

  /// 那条"原生通道没接上"的提示**只说一次**（别每次 Ctrl+V 都弹）。
  bool _clipboardNoticeShown = false;

  /// 用户 2026-10-18："ctrl+v不能粘贴附件。"（2026-10-24 补上"截图 / 复制图片"那一半）
  ///
  /// # 为什么这里必须异步 + 自己兜底
  ///
  /// "剪贴板里有没有文件 / 图"要问原生（`FileClipboard`），而按键回调**是同步的**
  /// —— 没法 `await` 了再决定吃不吃这个事件。所以这里的做法是：
  /// **先把事件吃掉**，然后异步判；都不是就自己把文本粘进输入框。
  ///
  /// ⚠ 兜底那一步不能省：不还回去的话，用户按 Ctrl+V 会**什么都粘不上**
  /// （事件被我们吃掉了，而 `DefaultTextEditingShortcuts` 没机会处理）。
  KeyEventResult _onComposerKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    if (!FileClipboard.supported) return KeyEventResult.ignored;
    if (event.logicalKey != LogicalKeyboardKey.keyV) {
      return KeyEventResult.ignored;
    }
    if (!HardwareKeyboard.instance.isControlPressed) {
      return KeyEventResult.ignored;
    }
    // 只认"纯 Ctrl+V"：Ctrl+Shift+V / Alt+V 有别的含义，别抢
    if (HardwareKeyboard.instance.isShiftPressed ||
        HardwareKeyboard.instance.isAltPressed) {
      return KeyEventResult.ignored;
    }
    unawaited(_pasteFilesOrText());
    return KeyEventResult.handled;
  }

  Future<void> _pasteFilesOrText() async {
    final paths = await FileClipboard.readFiles();
    if (!mounted) return;
    if (paths.isNotEmpty) {
      // 与"拖进来"共用同一条路（形状、去重、上限都一致）
      _attachDropped(paths);
      return;
    }
    // 不是文件 —— 再看是不是**一张图**（截图工具 / 浏览器里复制图片）。
    //
    // 用户 2026-10-24："允许我在 AI 工作台 Ctrl+V 粘贴附件。" 截图那条路以前是空的：
    // Flutter 的 `Clipboard` 只读文本，而截图在 `CF_DIB` / `CF_BITMAP` 里。
    final image = await FileClipboard.readImage();
    if (!mounted) return;
    if (image != null && await _attachClipboardImage(image)) return;
    if (!mounted) return;
    // 没有文件 → 先看是不是**原生那一半压根没接上**："通道没编进产物"与"剪贴板里
    // 确实没有文件"表现完全一样，静默降级的话这一档永远查不出来（只说一次）。
    if (FileClipboard.channelAlive == false && !_clipboardNoticeShown) {
      _clipboardNoticeShown = true;
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        const SnackBar(content: Text('读剪贴板（文件 / 图片）的原生通道没接上：Ctrl+V 现在只能粘文本')),
      );
    }
    // 这是一次**普通文本粘贴**，原样实现一遍交给输入框
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (!mounted || text == null || text.isEmpty) return;
    final controller = widget.controller;
    final value = controller.value;
    final selection = value.selection;
    final start = selection.isValid ? selection.start : value.text.length;
    final end = selection.isValid ? selection.end : value.text.length;
    controller.value = value.copyWith(
      text: value.text.replaceRange(start, end, text),
      selection: TextSelection.collapsed(offset: start + text.length),
      composing: TextRange.empty,
    );
  }

  /// 剪贴板里的位图 → PNG → 挂成附件（与"选文件"同一条上传链路）。
  ///
  /// 原生给 `bmp`（来自 `CF_DIB` / `CF_BITMAP`）时在这里解码再编码成 PNG：
  /// 三家上游都只收 png / jpeg / webp / gif，BMP 直接发出去会被网关拒绝。
  /// 解码走 `dart:ui`（Skia 的 BMP 解码器），不为这件事引新依赖。
  ///
  /// 返回 `false` = 这张图用不了（调用方继续走文本粘贴，别把这次 Ctrl+V 吞掉）。
  Future<bool> _attachClipboardImage(ClipboardImage image) async {
    var bytes = image.bytes;
    if (!image.isPng) {
      try {
        final codec = await ui.instantiateImageCodec(bytes);
        final frame = await codec.getNextFrame();
        final png = await frame.image.toByteData(
          format: ui.ImageByteFormat.png,
        );
        frame.image.dispose();
        codec.dispose();
        if (png == null) return false;
        bytes = png.buffer.asUint8List(png.offsetInBytes, png.lengthInBytes);
      } catch (_) {
        // 解不出来（剪贴板里是别的位图变体）—— 如实忽略，退回文本粘贴
        return false;
      }
    }
    if (!mounted) return false;
    final now = DateTime.now();
    final name =
        '剪贴板图片-${now.year}${_two(now.month)}${_two(now.day)}-'
        '${_two(now.hour)}${_two(now.minute)}${_two(now.second)}.png';
    await widget.store.attachImageBytes(bytes, name: name);
    return true;
  }

  static String _two(int value) => value.toString().padLeft(2, '0');

  /// 从磁盘路径取文件名（附件显示用）。取不到就退回整条路径 —— 界面上总得有个名字。
  static String _fileNameOf(String path) {
    final normalized = path.replaceAll('\\', '/');
    final index = normalized.lastIndexOf('/');
    final name = index < 0 ? normalized : normalized.substring(index + 1);
    return name.isEmpty ? path : name;
  }
}

class _SendIntent extends Intent {
  const _SendIntent();
}

/// 把右键菜单里的「粘贴」换成 `onPaste`（**位置不变**；内置项里没有就补在末尾）。
///
/// 抽成公开的纯函数只为一件事：**让它可测**。它是"右键粘贴别把文件路径当文本贴进来"
/// 的**唯一**防线，而这种"换掉一个内置菜单项"的改动一旦被谁改回去，
/// 界面上不会报任何错、只会安安静静粘出一串 `D:\...`（见 `_composerContextMenu`）。
///
/// 为什么要补末尾那一手：光标落在只读位置之类的场景下，内置项里**压根没有**「粘贴」，
/// 不补的话用户在这一档就没有任何粘贴入口了。
@visibleForTesting
List<ContextMenuButtonItem> withReplacedPaste(
  List<ContextMenuButtonItem> items,
  VoidCallback onPaste,
) {
  // ⚠ `type` 必须显式写回 `paste`：`ContextMenuButtonItem` 不传 `type` 时默认是
  // `custom`，而平台相关的**排序与文案**是看 `type` 的 —— 丢了类型会变成
  // "菜单位置对但样式/排序跟别处不一样"，且不报错。
  final paste = ContextMenuButtonItem(
    type: ContextMenuButtonType.paste,
    label: '粘贴',
    onPressed: onPaste,
  );
  final out = <ContextMenuButtonItem>[];
  var replaced = false;
  for (final item in items) {
    if (item.type == ContextMenuButtonType.paste) {
      out.add(paste);
      replaced = true;
    } else {
      out.add(item);
    }
  }
  if (!replaced) out.add(paste);
  return out;
}

class _Banner extends StatelessWidget {
  final String text;
  final IconData icon;
  final bool error;
  final VoidCallback? onClose;

  const _Banner({
    required this.text,
    required this.icon,
    this.error = false,
    this.onClose,
  });

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
//  输入区底行的文字密度（2026-10-11）
// ---------------------------------------------------------------------------

/// 输入区底行控件的**三档文字密度**。
///
/// 用户 2026-10-11："AI 工作台界面下方的控件，有时候按钮太小了比较难点，
/// 也没显示出有用的关键信息；在其他特定宽度下，控件变成两行排列也很难看。"
///
/// 老版只有两档（全称 / 纯图标），中间那一大片宽度没档位 —— 于是要么文字被挤爆，
/// 要么退成四个没有信息的小图标。现在补上中间的**短标签**档：
/// 关键信息（哪一档权限、哪个思考强度、哪个模型）**在任何宽度都看得见**。
enum _ComposerLabel {
  /// 全称：`自动允许（无需批准）`、模型名 + 能力徽标、思考强度全名。
  full,

  /// 短标签：`自动` / `询问`、`默认` / `高` + 模型名。**关键信息仍在**。
  short,

  /// 纯图标：文字进 tooltip（信息一个都不丢，只是要点开/悬停才看见）。
  icon,
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

  /// 文字密度（见 [`_ComposerLabel`]）。
  final _ComposerLabel density;

  const _PermissionPicker({
    required this.store,
    this.density = _ComposerLabel.full,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final full = store.fullPermission;
    final color = full ? theme.colorScheme.error : theme.colorScheme.outline;
    final text = full ? AiToolPolicy.modeFullLabel : AiToolPolicy.modeAskLabel;
    // 短标签档：`自动允许（无需批准）` → `自动`；`询问` 本来就短，原样用。
    // ⚠ 短标签**必须仍然能区分两档**（"自动" vs "询问"）——
    // 退回纯图标之后，用户点不准又看不出现在是哪一档，那正是用户抱怨的那件事。
    final shortText = full ? '自动' : AiToolPolicy.modeAskLabel;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: AppMenuButton<String>(
        tooltip: full
            ? '当前：$text —— AI 调用工具不会等你批准（可读 / 可写目录范围不变，越界仍会被拒）。点一下改回「${AiToolPolicy.modeAskLabel}」。'
            : '当前：$text —— 写盘 / 提交任务这类改动操作会先弹批准。点一下切到「${AiToolPolicy.modeFullLabel}」。',
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
                : () => controller.isOpen
                      ? controller.close()
                      : controller.open(),
            child: Container(
              // 三档都保证 ≥44 高（用户："按钮太小了比较难点"）
              constraints: const BoxConstraints(minHeight: 44),
              padding: EdgeInsets.symmetric(
                horizontal: density == _ComposerLabel.full ? 10 : 12,
                vertical: 8,
              ),
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
                    size: 18,
                    color: color,
                  ),
                  if (density != _ComposerLabel.icon) ...[
                    const SizedBox(width: 6),
                    Text(
                      density == _ComposerLabel.full ? text : shortText,
                      style: theme.textTheme.labelMedium?.copyWith(
                        color: full ? color : null,
                      ),
                    ),
                  ],
                  // 纯图标档不画箭头（那是展开暗示；图标按钮 + tooltip 已经够）。
                  if (density != _ComposerLabel.icon)
                    Icon(
                      Icons.arrow_drop_down,
                      size: 18,
                      color: theme.colorScheme.outline,
                    ),
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
          '当前模型：$name${modalities.isEmpty ? '' : '（$modalities）'}'
          '${provider == null ? '' : '\n来源：$provider'}'
          ' —— 点一下换一个',
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
              '来源：${p.displayName}',
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

/// 思考强度选择器（AIH-056）。
///
/// 只显示**当前模型声明过**的档位；**「默认」例外** —— 它不发任何思考参数、
/// 任何网关都成立，所以永远可选（用户要求"能默认不指定思考强度，且不用去改模型声明"）。
/// **但它不承诺"不会思考"**（2026-09-20 改名的原因，见 [AiReasoningEffort.off]：
/// 本机在用的聚合网关关不掉思考，叫「关闭」就是撒谎）。
/// 模型完全没声明推理能力时整块置灰并说明原因，而不是给一堆选了会被后端拒绝的选项。
class _EffortPicker extends StatelessWidget {
  final AiWorkspaceStore store;

  /// 文字密度（见 [`_ComposerLabel`]）。
  final _ComposerLabel density;

  const _EffortPicker({
    required this.store,
    this.density = _ComposerLabel.full,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final model = store.selectedModel;
    final options = model?.selectableEfforts ?? const <AiReasoningEffort>[];
    final enabled = options.isNotEmpty;

    final text = enabled
        ? store.reasoningEffort.label
        : (model == null ? '思考强度' : '不支持思考');
    // 短标签档：`默认` 本来就短；`高` / `中` / `低` 也短。
    // 只有 `不支持思考` 那种长文案需要压 —— 压成"无"并让 tooltip 说清原因。
    final shortText = enabled
        ? store.reasoningEffort.label
        : (model == null ? '思考' : '不支持');

    // 纯图标档用 tooltip 兜住信息（tooltip 里带着档位说明与"为什么置灰"）
    final tooltip = enabled
        ? '思考强度（当前：${store.reasoningEffort.label}）'
              '：「默认」始终可选：不发思考参数、由网关决定；'
              '模型声明的档位：'
              '${options.where((e) => e.isThinking).map((e) => e.label).join(' / ')}'
        : (model == null ? '还没选模型' : '$text —— 该模型未声明推理能力，请到「设置 → AI 模型」声明');

    return AppMenuButton<AiReasoningEffort>(
      tooltip: tooltip,
      onSelected: store.selectReasoningEffort,
      options: [
        for (final e in options)
          MenuOption(
            value: e,
            label: e.label,
            icon: e == store.reasoningEffort
                ? Icons.radio_button_checked
                : Icons.radio_button_off,
            // 线上表达（例如 "→ high"）放第二行，别跟标题挤在一行。
            // 「默认」没有"线上表达"（我们什么都不发），就把它的真实含义写在那儿 ——
            // 用户报的 bug 正是"以为选了关闭就不会思考"，这一行是那个 bug 的正面回答。
            subtitle:
                e.hint ??
                (model?.effortWireValue(e) == null
                    ? null
                    : '→ ${model!.effortWireValue(e)}'),
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
            // 三档都保证 ≥44 高（用户："按钮太小了比较难点"）
            constraints: const BoxConstraints(minHeight: 44),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
            decoration: BoxDecoration(
              border: Border.all(
                color: enabled && store.reasoningEffort.isThinking
                    ? theme.colorScheme.primary
                    : theme.dividerColor,
              ),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.psychology_outlined,
                  size: 18,
                  color: store.reasoningEffort.isThinking && enabled
                      ? theme.colorScheme.primary
                      : null,
                ),
                // 纯图标档才把文字收进 tooltip（短标签档**保留档位名** ——
                // "现在是什么思考强度"正是用户要看见的关键信息）
                if (density != _ComposerLabel.icon) ...[
                  const SizedBox(width: 5),
                  Text(
                    density == _ComposerLabel.full ? text : shortText,
                    style: theme.textTheme.labelMedium,
                  ),
                ],
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
    // 输入区托盘有 modality，直接信它（见 [canPreviewInline]）
    final preview = canPreviewInline(attachment.modality, attachment.name);

    return Tooltip(
      message: blocked
          ? '这张附件发不出去：\n${blockers.map((b) => '· $b').join('\n')}'
          : '${attachment.name} · ${attachment.sizeLabel}${attachment.isVideo ? '（已取预览帧）' : ''}'
                '\n${preview ? '点击预览' : '点击用系统程序打开'}',
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
                      onTap: fileUrl == null
                          ? null
                          : () => preview
                                ? showImagePreview(context, url: fileUrl!)
                                : _openAttachment(fileUrl!),
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
                          child: Icon(
                            Icons.play_circle_fill,
                            size: 28,
                            color: Colors.white70,
                          ),
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
                      constraints: const BoxConstraints.tightFor(
                        width: 24,
                        height: 24,
                      ),
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
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _preview(BuildContext context) {
    if (thumbUrl == null) return _iconFallback(context);
    final dpr = MediaQuery.devicePixelRatioOf(context);
    // **附件不打码**（用户 bug 2026-09-20 之后定的规矩）：附件是用户自己挑的文件，
    // 库里没有、也不该有"这条是不是 NSFW"的标记 —— 按"没标记就不糊"的判据，
    // 这里本来就不该再包一层（以前无脑糊，用户自己的参考图一上传就是一片马赛克）。
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
      color: theme.colorScheme.surfaceContainerHighest.withValues(
        alpha: dim ? 0.3 : 0.6,
      ),
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

  const _MessageAttachment({
    required this.name,
    required this.thumbUrl,
    required this.fileUrl,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // 历史消息没有 modality，只能按名字判（见 [canPreviewInline]）
    final preview = canPreviewInline(null, name);
    return Tooltip(
      message: preview ? '$name\n点击预览' : '$name\n点击用系统程序打开',
      child: InkWell(
        onTap: fileUrl == null
            ? null
            : () => preview
                  ? showImagePreview(context, url: fileUrl!)
                  : _openAttachment(fileUrl!),
        child: Container(
          width: 96,
          height: 96,
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(8),
            border: Border.all(color: theme.dividerColor),
          ),
          child: thumbUrl == null
              ? Center(
                  child: Icon(
                    Icons.attach_file,
                    size: 22,
                    color: theme.colorScheme.outline,
                  ),
                )
              : Image.network(
                  thumbUrl!,
                  fit: BoxFit.cover,
                  cacheWidth: (96 * MediaQuery.devicePixelRatioOf(context))
                      .round()
                      .clamp(1, 512),
                  filterQuality: FilterQuality.low,
                  errorBuilder: (_, _, _) => Center(
                    child: Icon(
                      Icons.attach_file,
                      size: 22,
                      color: theme.colorScheme.outline,
                    ),
                  ),
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

    return DecoratedBox(
      decoration: panelCardDecoration(theme),
      child: Padding(
        padding: const EdgeInsets.all(10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  jobs.hasActivity
                      ? Icons.play_circle_outline
                      : Icons.dns_outlined,
                  size: 16,
                  color: jobs.hasActivity
                      ? theme.colorScheme.primary
                      : theme.colorScheme.outline,
                ),
                const SizedBox(width: 6),
                Text('实时进度', style: theme.textTheme.labelLarge),
                const Spacer(),
                if (jobs.queuePending > 0)
                  Text(
                    '队列 ${jobs.queuePending}',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
              ],
            ),
            const SizedBox(height: 6),
            if (!jobs.comfyReachable)
              Text(
                'ComfyUI 不可达：进度拿不到（它没在跑？）',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
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
              Text(
                '当前没有正在生成的任务',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            if (recent.isNotEmpty) ...[
              const Divider(height: 14),
              Text(
                '最近提交',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
              for (final s in recent)
                Padding(
                  padding: const EdgeInsets.only(top: 3),
                  child: _SubmissionRow(submission: s, highlight: false),
                ),
            ],
          ],
        ),
      ),
    );
  }
}

/// 右侧栏卡片的统一外观。
///
/// 刻意**手写**而不是直接 `Card`：Skills 那一块要罩住"标题 + 懒构建的条目列表"，
/// 只能用 `DecoratedSliver`（它只吃 `Decoration`，给不了 `Card`）。
/// 两处共用这一个函数，就不会出现"只有实时进度有卡片"或者几张卡长得不一样。
/// 数值与主题里的 `cardTheme` 对齐（M3 的 surfaceContainerLow、14 圆角、
/// outlineVariant@0.5 描边）。
BoxDecoration panelCardDecoration(ThemeData theme) => BoxDecoration(
  color: theme.colorScheme.surfaceContainerLow,
  borderRadius: BorderRadius.circular(14),
  border: Border.all(
    color: theme.colorScheme.outlineVariant.withValues(alpha: 0.5),
  ),
);

/// 右侧栏里的一块：统一的卡片容器 + 一行标题（可选右侧动作）。
///
/// 用户建议（2026-09-20）：「给右边栏除了实时进度以外的内容也拿类似容器分别包裹起来」。
/// 包裹之后每块自己成一张卡，"哪几行属于同一块、到哪里算下一块"一眼可见。
class _PanelCard extends StatelessWidget {
  /// 标题行左侧的文字（为空则只画 [children]，由内容自带标题）。
  final String? title;

  /// 标题行右侧的动作（例如 ComfyUI 的刷新按钮）。
  final Widget? trailing;

  final List<Widget> children;

  const _PanelCard({
    super.key,
    this.title,
    this.trailing,
    required this.children,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return DecoratedBox(
      decoration: panelCardDecoration(theme),
      child: Padding(
        padding: const EdgeInsets.all(10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (title != null) ...[
              Row(
                children: [
                  Text(title!, style: theme.textTheme.labelLarge),
                  if (trailing != null) ...[const Spacer(), trailing!],
                ],
              ),
              const SizedBox(height: 6),
            ],
            ...children,
          ],
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
    final p = submission.progress;
    // 进度条只在"**正在跑** + 后端真的给了这一条的进度"时出现。
    // 拿不到进度就不画（不画一条 0% 的假进度 —— 那会让人以为卡住了）。
    final showProgress = highlight && submission.isActive && p != null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
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
                if (submission.mediaIds.isNotEmpty)
                  '${submission.mediaIds.length} 个产物',
              ].join(' · '),
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
        if (showProgress) ...[
          const SizedBox(height: 3),
          // `percent` 为空 = **进度未知**（ComfyUI 没发 value/max）→ 不确定态进度条。
          // 绝不拿"已执行节点数/总节点数"自己凑一个比例（后端也刻意不这么做）。
          LinearProgressIndicator(
            value: p.percent == null ? null : (p.percent!.clamp(0, 100)) / 100,
            minHeight: 3,
          ),
          const SizedBox(height: 2),
          Text(
            [p.stepLabel, if (p.percent != null) '${p.percent}%'].join(' · '),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        ],
      ],
    );
  }
}

/// 跨端待审批（M5）：**手机排上来的提交**在桌面上长出来的那张卡。
///
/// # 为什么要有它
///
/// 桌面上那张"等你批准"的卡平时是跟着 **Run 的 SSE** 长的，而手机排上来的提交
/// （契约 §4.1 的 `submit_job`，`callId` 形如 `submit:<queueId>`）**不在任何本机 Run
/// 的事件流里** —— 没有这张卡，"手机排的队"在桌面上既看不见也点不了，只能等它超时。
/// 数据源只有 `GET /api/ai/pending` 一个（`asks` 那一份不在这里画：提问卡跟着会话走，
/// 由消息列表里的 `_AskCard` 管）。
///
/// # 几条纪律
///
///  * **只有桌面画它**（手机上审批归桌面 —— 给一个点了没用的按钮不如不给）；
///  * 一屏最多 [_PendingApprovals.maxVisible] 条 + 一行"还有 N 条"（右侧栏本来就窄，
///    排队多了不能撑爆它）；
///  * **列表为空时整块不占位**（与 [_ComfyProgress] 同一条纪律，连下面那道间距都不留）；
///  * 拉不到列表**如实说一句**（一行小字），但别把整页搞得像坏了；
///  * 点完**按返回值如实更新**：`accepted=false` 时那行不许消失，还要说清"桌面没接受"。
class _PendingApprovals extends StatefulWidget {
  final AiWorkspaceStore store;

  /// 一屏最多画几条（剩下的折成一行"还有 N 条"）。
  static const maxVisible = 5;

  const _PendingApprovals({required this.store});

  @override
  State<_PendingApprovals> createState() => _PendingApprovalsState();
}

class _PendingApprovalsState extends State<_PendingApprovals> {
  /// 正在处理的那一条（处理期间按钮禁用，防重复点）。
  String? _busyCallId;

  /// 上一次动作的结果（人话）。`accepted=false` 时用错误色显示 —— 不许悄悄吞掉。
  String? _message;
  bool _failed = false;

  /// 参数按后端给的原文显示（**不解析、不猜**），压成一行免得把窄栏撑开。
  static String _oneLineArgs(String raw) =>
      raw.replaceAll(RegExp(r'\s+'), ' ').trim();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final approvals = widget.store.pendingOverview.approvals;
    final error = widget.store.pendingError;

    // 没有待办、也没出错：整块不占位。
    if (approvals.isEmpty && error == null) return const SizedBox.shrink();

    final visible = approvals.take(_PendingApprovals.maxVisible).toList();
    final hidden = approvals.length - visible.length;

    return Padding(
      // 间距放在这里面：空的时候整块（含间距）一起消失
      padding: const EdgeInsets.only(bottom: 10),
      child: _PanelCard(
        key: const Key('ai-panel-card-pending'),
        title: '待审批',
        // 条数只在**真有**待办时显示：列表空着的时候这一行整块只因"拉不到"而存在，
        // 那时写一个"0 条"既没意义，又和右侧栏别处的条数（长期记忆的"N 条"）撞脸。
        trailing: approvals.isEmpty
            ? null
            : Text(
                '${approvals.length} 项待批',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
        children: [
          if (error != null)
            Text(
              '待审批列表取不到：$error',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.error,
              ),
            ),
          for (final a in visible) _row(theme, a),
          if (hidden > 0)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                '还有 $hidden 条（先处理上面这几条）',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
          if (visible.any((a) => a.isQueuedSubmit))
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                // 用户拍板过的那条规矩，写在卡片上：手机点过提交**不算**批准
                '手机上点「提交」只是排队，真正提交要在这里点「批准」。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
          if (_message != null)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                _message!,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: _failed
                      ? theme.colorScheme.error
                      : theme.colorScheme.primary,
                ),
              ),
            ),
        ],
      ),
    );
  }

  /// 一行 = 固定行高的"这是什么 + 批准 / 拒绝"。
  ///
  /// 行高是常数（AGENTS §6）：右侧栏是懒构建的滚动区，行自己撑高会让滚动条越滚越短。
  Widget _row(ThemeData theme, AiPendingApproval a) {
    final label = a.isQueuedSubmit
        ? '手机排队的提交'
        : (a.name.isEmpty ? '未知工具' : a.name);
    final detail = _oneLineArgs(a.argumentsJson);
    final busy = _busyCallId != null;
    const padding = EdgeInsets.symmetric(horizontal: 10);
    const minSize = Size(0, 28);
    const tapTarget = MaterialTapTargetSize.shrinkWrap;
    return SizedBox(
      height: 38,
      child: Row(
        children: [
          Expanded(
            child: Text(
              detail.isEmpty ? label : '$label · $detail',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall,
            ),
          ),
          const SizedBox(width: 4),
          FilledButton(
            key: Key('ai-pending-approve-${a.callId}'),
            onPressed: busy ? null : () => _resolve(a, approve: true),
            style: FilledButton.styleFrom(
              padding: padding,
              minimumSize: minSize,
              tapTargetSize: tapTarget,
            ),
            child: const Text('批准'),
          ),
          const SizedBox(width: 4),
          OutlinedButton(
            key: Key('ai-pending-deny-${a.callId}'),
            onPressed: busy ? null : () => _resolve(a, approve: false),
            style: OutlinedButton.styleFrom(
              padding: padding,
              minimumSize: minSize,
              tapTargetSize: tapTarget,
            ),
            child: const Text('拒绝'),
          ),
        ],
      ),
    );
  }

  /// 点「批准 / 拒绝」：立刻发出去，**按返回值如实更新界面**。
  Future<void> _resolve(AiPendingApproval a, {required bool approve}) async {
    setState(() {
      _busyCallId = a.callId;
      _message = null;
    });
    final result = await widget.store.resolvePendingApproval(
      a.callId,
      approve: approve,
    );
    if (!mounted) return;
    setState(() {
      _busyCallId = null;
      _failed = !result.accepted;
      _message = result.message;
    });
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
      // 实时进度（用户"其他建议"第 1 条）：右侧栏一显示就开始轮询，
      // 这样"AI 刚提交的任务 / 用户自己在 ComfyUI 里点的任务"都能看到进度。
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

    // 右侧栏每一块**各自一个卡片容器**（用户建议 2026-09-20：「给右边栏除了实时进度
    // 以外的内容也拿类似容器分别包裹起来」）。以前只有"实时进度"是 Card，其余几块是
    // 直接摊在背景上的文字与列表，一眼看不出哪几行属于同一块、到哪里算下一块。
    //
    // 为什么不用 `Card`+`Column` 一把包住：skills 可能有几十个，**必须懒构建**
    // （`ListView(children: [...])` 会一次全建）。所以 Skills 那张卡走
    // `DecoratedSliver` + `SliverMainAxisGroup`：卡片背景罩住"标题 + 懒构建的条目"，
    // 行数再多也只建看得见的那几条。外观与 [_PanelCard] 共用 [panelCardDecoration]。
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
              // 实时进度（用户"其他建议"第 1 条）：AI 提交的任务 / ComfyUI 队列，
              // 用户在对话里说"帮我跑一张"之后，不用切到 ComfyUI 就能看到它跑到哪了。
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
                      '来源：${model.capabilitySourceLabel}'
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
              // 以前挂在"上下文"标题右边，看着像是刷新整个面板（用户建议第 5 条）。
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
                    Text('查询失败：$_error', style: theme.textTheme.bodySmall)
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
        // 这里不再有「从 DSH 导入」按钮（用户明确要求去掉）。
        //
        // 用户"其他建议"第 2 条：**列表默认折叠**（装了十几个 skill 之后，
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
                            'Skills 加载失败：${store.skillsError}',
                            style: theme.textTheme.bodySmall,
                          ),
                      ],
                    ),
                  ),
                ),
                if (!_skillsOpen)
                  // 折叠时不列条目，只留一句提示（用户想找 skill 就展开）
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
                      '有问题：${skill.validationError != null ? '格式不合法' : '有冲突'}',
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
      trailing: skill.isBuiltin
          ? Tooltip(
              message: '内置 Skill 只读（可在磁盘上改）',
              child: Icon(
                Icons.lock_outline,
                size: 14,
                color: theme.colorScheme.outline,
              ),
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
          Text(
            k,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
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
/// 用户在建议里要的就是这个：**不要「从 DSH 导入」按钮**，改成一个能拷东西进去的目录。
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
          '把 skill 拷进这个目录即装好（启动/刷新时自动登记）：',
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
/// **刻意不在这里印记忆正文**（用户建议 2026-09-20：「右边栏长期记忆，
/// 不显示记忆内容的预览」）：记忆里存的多半是私下的偏好与上下文，而右侧栏是
/// **一直摊在屏幕上**的一面墙 —— 把正文印上去，任何路过屏幕的人都看得到。
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
          '每次对话都会带上它；AI 也能用 remember 追加一条。点右上角的图标查看 / 编辑内容。',
          style: theme.textTheme.labelSmall?.copyWith(
            color: theme.colorScheme.outline,
          ),
        ),
        if (store.memoryError != null)
          Text(
            '长期记忆不可用：${store.memoryError}',
            style: theme.textTheme.bodySmall,
          ),
      ],
    );
  }
}

/// 长期记忆编辑器：**搜索 + 单条 / 批量删除** + 加一条 + 整篇编辑。
///
/// 真源仍然是 `memory.md`（用户直接改那个文件也认），所以这里没有另造一套存储 ——
/// 列表视图只是"一行一条"的一个方便入口。加搜索与删除是用户的明确要求：
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
    // 搜索时把已有的勾选收拢到"还看得见的那几条"：否则"删除选中"会删掉用户
    // 当前看不到的东西 —— 那是很坏的一种惊喜。
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
                          ? '一行一条；AI 的 remember 工具会追加到这里。'
                          : '真源是 $path（也可以直接改那个文件）。',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
                  // 条数占用（用户要求：条数要可控，界面上得看得见还剩多少）
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
/// 由调用方把路径显示给用户 —— 不能假装打开了。
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
