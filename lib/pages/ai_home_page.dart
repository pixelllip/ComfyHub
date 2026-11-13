import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;
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
import '../core/layout_constants.dart';
import '../core/pairing_token.dart';
import '../core/settings_store.dart';
import '../core/theme.dart';
import '../models/models.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/ai_pending_banner.dart';
import '../widgets/app_menu.dart';
import '../widgets/attachment_preview.dart';
import '../widgets/common.dart' show FilterChipRow;
import '../widgets/markdown_view.dart';
import '../widgets/nsfw_blur.dart';
import '../widgets/submit_result_card.dart';
import '../widgets/workflow_picker.dart';
import '../widgets/workflow_viewer.dart';
import '../widgets/zoomable_image_view.dart';
import 'media_detail_page.dart';
import 'train_page.dart';

part 'ai_home_ask_card.dart';
part 'ai_home_context_panel.dart';
part 'ai_home_model_picker.dart';

/// 后端给的相对地址（`/api/ai/attachments/...`）→ 可直接给 `Image.network` 的绝对地址。
///
/// 提问卡的裁图 / 选项示意图都是后端存的相对路径：前端**只补 baseUrl**，
/// 不自己拼第二套附件路径（那种拼法在换端口 / 换机器时一定 404）。
String absoluteBackendUrl(String base, String url) {
  final trimmed = url.trim();
  if (trimmed.isEmpty) return trimmed;
  if (trimmed.startsWith('http://') || trimmed.startsWith('https://')) {
    return trimmed;
  }
  return '$base$trimmed';
}

/// 附件预览取文本时带的头（配对令牌**现取**：空串不发，理由同 `AiApiClient._headers`）。
Map<String, String> attachmentPreviewHeaders() {
  final token = PairingTokenCache.read();
  return {if (token != null && token.isNotEmpty) 'X-ComfyHub-Token': token};
}

/// 历史附件块没有原始文件名时，界面上拿什么称呼它。
///
/// `parts[].text` 为空时调用方会退回 `attachmentId`（一串 UUID）。把 UUID 原样当
/// 标题既难读、又会让"按扩展名判类型"的兜底彻底失效 —— 所以这里换成"图片附件 / 附件"。
String attachmentDisplayName(String raw, {required bool hasPreview}) {
  final name = raw.trim();
  final looksLikeId = RegExp(
    r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
  ).hasMatch(name);
  if (name.isEmpty || looksLikeId) {
    return hasPreview ? '图片附件' : '附件';
  }
  return name;
}

/// 「从设定图生成角色卡」的固定起手流程。
///
/// 这句话只负责**把意图与顺序说清**：先逐部位问、带裁图、问完再写卡。
/// 具体规则（怎么裁、选项要不要配图、哪些不许问）在系统提示第 26 条里。
const _characterSheetPrompt =
    '这是一张角色设定图。请从上到下逐部位分析，先别急着写卡：'
    '凡是光看图会判错的地方（发型分缝 / 刘海、发饰、领口与服装分层、裙片、袜鞋、手上道具），'
    '都用 ask_user_question 一次问清 —— 每题带一块从这张图上裁下来的局部图，'
    '给几个具体候选项（有可靠示例图就贴在选项前面）；我答完你再按答案写成角色卡。'
    '能看死的颜色和明显款式不要问。';

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

  /// 正在把草稿写回输入框。此时**必须屏蔽** `_onInputChanged` ——
  /// 否则"清空输入框"会被当作"删光文字"，把刚取回的草稿覆盖为空串
  /// （切走再切回后文字丢失）。
  bool _applyingDraft = false;

  /// 第几次切换会话：异步取草稿回来时用它判断"还是不是同一次切换"。
  int _draftEpoch = 0;

  /// 视口是否贴底：**贴底才自动跟随**，翻旧消息时不跟随。
  bool _atBottom = true;

  /// 上一次通知时**消息区里装的是哪条会话**。
  ///
  /// 用来识别"内容整批被换掉"这个时机（换会话 / 重新加载）——
  /// 见 [_resetScrollForConversation]：换会话要把滚动位置重置到最新一条。
  String? _shownConversationId;

  /// 左栏（会话列表）的宽度（逻辑像素）。可拖动，下限 / 上限见
  /// `layout_constants.dart` 的 `kAiConversationPaneWidth` / `kAiSidePaneMaxWidth`。
  ///
  /// 初值来自偏好（上次松手时的宽度），拖动过程中只改这个字段，**松手才落盘**。
  double _conversationPaneWidth = kAiConversationPaneWidth;

  /// 右栏（上下文与状态）的宽度，约束同 [_conversationPaneWidth]。
  double _contextPaneWidth = kAiContextPaneWidth;

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
    // 侧栏宽度是"上次松手时"的值；越界 / 坏值在 SettingsStore.load 里已回默认，
    // 这里只负责取用（真正夹到当前窗口能容纳多少由 build 现算）。
    final settings = context.read<SettingsStore>();
    _conversationPaneWidth = settings.aiConversationPaneWidth;
    _contextPaneWidth = settings.aiContextPaneWidth;
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
      // 目录重读一次，改动立即生效（此前模型声明需重启客户端才生效）。
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
      // **确实更换会话**时才清空输入框（随后取回新会话草稿）。
      // 页面首次挂载（previous == null）不算切换：此时可能已有输入文字，
      // 清空会丢失输入。
      _setInputText('');
    } else if (id != null && _input.text.isNotEmpty) {
      // 首次通知之前已有输入：该文本属于当前会话，需保存，不能被草稿覆盖
      _store.saveDraft(id, _input.text);
    }
    if (id == null) return;

    final epoch = ++_draftEpoch;
    _store.loadDraft(id).then((draft) {
      if (!mounted || _draftEpoch != epoch) return;
      // 取草稿期间已开始输入：以当前输入为准，不回退为旧草稿
      if (_input.text.isEmpty) _setInputText(draft.text);
    });
  }

  /// 程序性地修改输入框内容：**不**触发草稿保存（仅手动输入才保存草稿）。
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
  ///  1. **取走即清空**（`takeHandoff`）—— 否则每次切回工作台都会再次填入，
  ///     使已清空的输入框重新出现上一段文本；
  ///  2. **仅在输入框为空时填入**：已有输入时直接覆盖会丢失内容；
  ///     有内容时改为"追加到末尾"，保留原输入；
  ///  3. **不触发草稿保存**（走 [_setInputText]）—— 该文本由程序填入，
  ///     不作为手动草稿落盘。
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

  /// 手动滚到底 / 翻上去时更新"贴底"状态。
  ///
  /// 判据只有一条：**是不是真的贴底**（容差 [_bottomEpsilon]，只吸收亚像素误差）。
  ///
  /// 旧写法为 `distance < 80 ? true : ...`，相当于把"贴底"的判定范围设为 80px。
  /// 该阈值导致两个现象：
  ///  ① 流式输出时向上滚动一小段：内容同时增长，`max` 增大而 `pixels` 变化很小，
  ///     计算出的距离始终在 80 以内 → 下一帧再次判定为"贴底" →
  ///     `_followIfAtBottom()` 的 `animateTo(max)` 将视口拉回底部（"只滚一小段时
  ///     界面立即重置到底部"）。
  ///  ② 快速拖动滚动条时位置异常：180ms 的跟随动画在拖动期间执行，
  ///     每段流式内容又触发一次 —— 拖动与动画方向不同，位置发生跳动。
  ///
  /// 容差改为 1.5 后，**滚动一像素即脱离"贴底"**：`_followIfAtBottom()` 不再
  /// 触发新的跟随动画，上述两个现象同时消失（滚回底部时距离归零，重新判定为贴底）。
  ///
  /// 这里**不需要**再判"位置有没有变小"：内容变长时 `max` 在涨而 `pixels` 没动，
  /// 距离确实会变大，但那只是"新内容在下方"，仍保持跟随；若主动向上滚动，
  /// 距离一定会超过这个容差。
  ///
  /// 另外**不要**在这里去动 `position`（比如"把在飞的动画按停"）：`UserScrollNotification`
  /// 是在 `ScrollDragController.update` 的中间派发的，此刻 `applyUserOffset` 正等着
  /// `setPixels`，任何 `jumpTo` / `hold` 都会把 `activity` 换成 Idle ——
  /// 结果是**拖动立即失效**，且 Flutter 会直接抛出
  /// `'activity!.isScrolling': is not true`（本机实测过）。
  void _onScroll() {
    if (!_scroll.hasClients) return;
    final distance = _scroll.position.maxScrollExtent - _scroll.position.pixels;
    final next = distance <= _bottomEpsilon;
    if (next != _atBottom && mounted) setState(() => _atBottom = next);
  }

  /// 只有"原本就在底部"时才跟随新内容滚动。
  void _followIfAtBottom() {
    if (!_atBottom) return;
    _scrollToBottom();
  }

  /// 换了会话（消息内容**整批被换掉**）时，把滚动位置重置到最新一条。
  ///
  /// 已知现象："总长变小、`pixels` 仍停留在按旧总长计算的位置 ——
  /// 内容被顶到视口外，第一条消息在界面上消失"。触发时机正是这里：
  /// `openConversation` 会把 `messages` 整批换掉，而本页只是重新 `build`，
  /// **滚动位置不会自己重置** —— `pixels` 被原样带到新会话上：
  ///   · 新会话内容更短 → 被夹回/顶出视口；
  ///   · 新会话内容一样长 → 直接落在半路（连夹取都不会发生，用例里实测差 256px）。
  ///
  /// 所以"换会话落在最新一条"这条产品纪律由界面**显式**负责，不靠框架的夹取去猜。
  /// 换会话的目的是查看最新进展，停留在中间位置没有意义。
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

  /// 回到最新消息（右下角按钮）。
  void _jumpToBottom() {
    if (mounted) setState(() => _atBottom = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scroll.hasClients) return;
      _scroll.animateTo(
        _scroll.position.maxScrollExtent,
        // M3 动效令牌（AppMotion）：同一类"回到位置"的动作全 App 用同一档。
        duration: AppMotion.medium,
        curve: AppMotion.emphasized,
      );
    });
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.animateTo(
          _scroll.position.maxScrollExtent,
          duration: AppMotion.short,
          curve: AppMotion.emphasized,
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
    // **900~1199px 区间缺少入口**：该区间内 `wide` 成立 → 无窄屏 FAB，
    // 但 `showPanel` 不成立 → 也无第三栏，
    // 因此「ComfyUI 状态」在界面上**没有入口**。
    // 仅在 900~1199px 将入口放入 AppBar 时，**窄屏仍依赖 FAB** ——
    // 而 FAB 位于 `Scaffold` 右下角，与输入区「发送」按钮重叠：
    // 窗口变窄时会覆盖发送按钮。现在入口只在**一个地方**：
    // 第三栏铺不开时，一律进 AppBar（`onShowStatus`），窄屏不再有 FAB。
    final showPanel = wide && width >= 1200;

    final conversationList = _conversationList;
    // AppBar 显示**当前对话标题**：标题自动生成，
    // 需要常驻可见，以便确认当前会话并可随时改名。
    //
    // **对话主体单独抽成一个会 `watch` 的 widget**（[_Thread]）：流式回包 /
    // ComfyUI 进度轮询每秒会通知好几次，以前是整页（含左侧 240px 会话列表）
    // 跟着重建 —— DevTools 实测一帧 Build 24.7ms、侧栏每个会话行一次重建
    // （Tooltip 12 / MenuAnchor 12 / ListTile 10）。现在只有这棵树重建。
    Future<void> submit(AiWorkspaceStore store, OutgoingMessage message) async {
      // 空输入 + 没有附件：没什么可发的，静默返回（也别弹提示）。
      if (message.text.trim().isEmpty && store.attachments.isEmpty) return;

      // **只有"真的会被发出去"才清空输入框**。
      //
      // 准入判断是同步的、和 `send()` 共用同一份（`store.sendBlockReason()`）：
      // 被拒时（最常见的是"还在生成中又按了一次回车"）一个字节都不动 ——
      // 旧实现为"先清空、再由 send() 拒绝"，输入文字被静默丢弃，
      // 只能手动复制上一条后重发。
      //
      // 清空仍然发生在等待之前：`store.send()` 要一直 await 到整轮结束，
      // 放到后面就会出现"点了发送输入框里的话还在"，看起来像没反应。
      if (store.sendBlockReason() == null) {
        _input.clear();
        // 刚发送消息：无论此前滚动到哪里，都回到底部查看回复
        if (mounted) setState(() => _atBottom = true);
        _scrollToBottom();
      }
      await store.sendMessage(message);
      _scrollToBottom();
    }

    final thread = _Thread(
      controller: _scroll,
      atBottom: _atBottom,
      input: _input,
      inputFocus: _inputFocus,
      onShowStatus: showPanel ? null : () => _showContextSheet(context),
      onJumpToBottom: _jumpToBottom,
      onSubmitted: submit,
    );

    if (wide) {
      // 三栏之间**不画分隔线**：层次由底色差表达，中间的消息区保持 `surface`。
      //
      // 两栏**不是同一档底色**：导航栏是 `surfaceContainerLow`，会话列表再抬一档
      // （`surfaceContainerHigh`），右侧状态栏回到 `surfaceContainerLow` 并把内容
      // 放进 `surfaceContainerHigh` 的卡片里。这样导航栏、会话列表、右侧栏、
      // 底部输入区四块各有各的底色，不会糊成一片。
      final scheme = Theme.of(context).colorScheme;
      final conversationPaneColor = scheme.surfaceContainerHigh;
      final contextPaneColor = scheme.surfaceContainerLow;
      return Scaffold(
        body: LayoutBuilder(
          builder: (context, constraints) {
            final bodyWidth = constraints.maxWidth;
            // 两栏宽度都是"记住的值 + 当前窗口能容纳多少"的现算结果：
            //   · 下限 = 拖动出现之前的固定宽度（240 / 260）；
            //   · 上限 = 480，并且两栏加起来要给中栏留下 kAiCenterMinWidth。
            // 先取右栏、再算左栏上限、最后回夹右栏 —— 后拖的那一栏让位给中栏，
            // 否则窗口一窄中栏就会被压成一条缝。
            final rightBase = _contextPaneWidth
                .clamp(kAiContextPaneWidth, kAiSidePaneMaxWidth)
                .toDouble();
            final leftMax = math.max(
              kAiConversationPaneWidth,
              math.min(
                kAiSidePaneMaxWidth,
                bodyWidth - (showPanel ? rightBase : 0) - kAiCenterMinWidth,
              ),
            );
            final leftWidth = _conversationPaneWidth
                .clamp(kAiConversationPaneWidth, leftMax)
                .toDouble();
            final rightMax = math.max(
              kAiContextPaneWidth,
              math.min(
                kAiSidePaneMaxWidth,
                bodyWidth - leftWidth - kAiCenterMinWidth,
              ),
            );
            final rightWidth = rightBase
                .clamp(kAiContextPaneWidth, rightMax)
                .toDouble();
            return Row(
              // 两栏与拖动条都铺满整列高度，底色才不会只盖住内容那一截。
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(
                  key: const Key('ai-conversation-pane'),
                  width: leftWidth,
                  // 用 `Material` 而不是 `ColoredBox`：会话行是 `ListTile`，
                  // 它的选中底色与水波纹要画在最近的 Material 上；中间插一层
                  // ColoredBox 会把那些效果盖掉（框架会直接断言）。
                  child: Material(
                    color: conversationPaneColor,
                    child: conversationList,
                  ),
                ),
                _PaneResizeHandle(
                  key: const Key('ai-conversation-resize'),
                  background: conversationPaneColor,
                  semanticLabel: '拖动调整会话列表宽度',
                  onDragStart: () {
                    // 从"当前实际显示宽度"开始累加：窗口变窄时显示宽度会被夹小，
                    // 直接拿记住的值累加会出现一段拖不动的死区。
                    setState(() => _conversationPaneWidth = leftWidth);
                  },
                  onDelta: (dx) {
                    setState(() {
                      _conversationPaneWidth = (_conversationPaneWidth + dx)
                          .clamp(kAiConversationPaneWidth, leftMax)
                          .toDouble();
                    });
                  },
                  onDragEnd: _savePaneWidths,
                ),
                Expanded(child: thread),
                if (showPanel) ...[
                  _PaneResizeHandle(
                    key: const Key('ai-context-resize'),
                    background: contextPaneColor,
                    semanticLabel: '拖动调整上下文栏宽度',
                    onDragStart: () {
                      setState(() => _contextPaneWidth = rightWidth);
                    },
                    onDelta: (dx) {
                      // 拖动条在右栏左侧：往左拖（dx 为负）= 右栏变宽。
                      setState(() {
                        _contextPaneWidth = (_contextPaneWidth - dx)
                            .clamp(kAiContextPaneWidth, rightMax)
                            .toDouble();
                      });
                    },
                    onDragEnd: _savePaneWidths,
                  ),
                  SizedBox(
                    key: const Key('ai-context-pane'),
                    width: rightWidth,
                    child: Material(
                      color: contextPaneColor,
                      child: _ContextPanel(store: store),
                    ),
                  ),
                ],
              ],
            );
          },
        ),
      );
    }

    return Scaffold(
      drawer: Drawer(child: SafeArea(child: conversationList)),
      body: thread,
    );
  }

  /// 拖动结束后把两栏宽度落盘（拖动过程中不写偏好，避免每帧一次磁盘写）。
  void _savePaneWidths() {
    final settings = context.read<SettingsStore>();
    settings.setAiConversationPaneWidth(_conversationPaneWidth);
    settings.setAiContextPaneWidth(_contextPaneWidth);
  }

  /// 手机上「上下文与状态」面板（模型能力 / ComfyUI / Skills / 长期记忆）。
  ///
  /// ⚠ 入口名称必须与内容一致：此前名为「ComfyUI 状态」，但面板中 ComfyUI 只占
  /// 其中一张卡，进入后与名称不符。现在顶栏菜单为「ComfyUI 与上下文」、
  /// 面板自己再标一行"上下文与状态"。
  void _showContextSheet(BuildContext context) {
    final store = context.read<AiWorkspaceStore>();
    final theme = Theme.of(context);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (_) => SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.72,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 10),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text('上下文与状态', style: theme.textTheme.titleMedium),
                  Text(
                    '模型能力 · ComfyUI · Skills · 长期记忆',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
            Expanded(child: _ContextPanel(store: store)),
          ],
        ),
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

/// 两栏之间的拖动条（宽屏才有）。
///
/// 平时与相邻栏同色、不画分隔线；鼠标移上去才出现一条主色短棒，拖动时保持显示。
/// 这样"可以拖"有一处可见的提示，同时又没有用户不喜欢的常驻竖线。
///
/// 命中区域就是 [kAiResizeHandleWidth] 宽的一整列，横向拖动交给它；
/// 左右方向由调用方决定（左栏往右拖变宽，右栏往左拖变宽）。
class _PaneResizeHandle extends StatefulWidget {
  const _PaneResizeHandle({
    super.key,
    required this.background,
    required this.semanticLabel,
    required this.onDragStart,
    required this.onDelta,
    required this.onDragEnd,
  });

  /// 与相邻栏相同的底色：拖动条看起来是那一栏的边缘，而不是另一块区域。
  final Color background;

  /// 读屏软件念出来的名字（例如"拖动调整会话列表宽度"）。
  final String semanticLabel;

  /// 按下并开始拖动：调用方用它把"当前显示宽度"写回状态。
  final VoidCallback onDragStart;

  /// 本次水平位移（向右为正），由调用方换算成宽度并夹在上下限内。
  final ValueChanged<double> onDelta;

  /// 松手 / 取消：调用方落盘。
  final VoidCallback onDragEnd;

  @override
  State<_PaneResizeHandle> createState() => _PaneResizeHandleState();
}

class _PaneResizeHandleState extends State<_PaneResizeHandle> {
  bool _hovering = false;
  bool _dragging = false;

  void _end() {
    if (_dragging && mounted) setState(() => _dragging = false);
    widget.onDragEnd();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final active = _hovering || _dragging;
    return Semantics(
      label: widget.semanticLabel,
      child: MouseRegion(
        cursor: SystemMouseCursors.resizeLeftRight,
        onEnter: (_) => setState(() => _hovering = true),
        onExit: (_) => setState(() => _hovering = false),
        child: Listener(
          // 用原始指针事件而不是 `GestureDetector`：拖动条只需要"按住后横向跟手"，
          // 不参与手势竞技场，所以没有 slop —— 按下即可拖动，边界与指针 1:1。
          onPointerDown: (_) {
            setState(() => _dragging = true);
            widget.onDragStart();
          },
          onPointerMove: (event) {
            if (_dragging) widget.onDelta(event.delta.dx);
          },
          onPointerUp: (_) {
            if (_dragging) _end();
          },
          onPointerCancel: (_) {
            if (_dragging) _end();
          },
          child: Container(
            width: kAiResizeHandleWidth,
            color: widget.background,
            alignment: Alignment.center,
            child: AnimatedOpacity(
              opacity: active ? 1 : 0,
              duration: AppMotion.short,
              child: Container(
                width: 2,
                height: 32,
                decoration: BoxDecoration(
                  color: theme.colorScheme.primary,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  对话主体：AppBar + 消息列表 + 输入区
// ---------------------------------------------------------------------------/// 会话主体整棵树。
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
  final Future<void> Function(AiWorkspaceStore store, OutgoingMessage message)
  onSubmitted;

  /// 输入框 `+` 菜单里的「选择现有工作流」：把路径写进输入框，让 AI 照着这一份出图。
  ///
  /// 不直接发送：还需要补充"想生成什么"。路径写入后，AI 的第一动作必然是
  /// `comfy_load_workflow(path=…)`（系统提示第 23 条 + `comfy-workflow-guide`），
  /// 不会自己编一份新的。
  Future<void> _pickWorkflow(BuildContext context) async {
    final store = context.read<AiWorkspaceStore>();
    // 选择器**只做选择**：不提供蓝图入口。
    final file = await showWorkflowPicker(
      context,
      loadList: store.listWorkflowFiles,
    );
    if (file == null || !context.mounted) return;
    final existing = input.text.trimRight();
    final hint = existing.isEmpty ? '' : '\n';
    final insert = '用这份工作流生成 ${file.path}\n';
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
    final questions = store.pendingQuestions;
    return LayoutBuilder(
      builder: (context, constraints) {
        // ★ 输入区**有上限**（治本）：面板 / 已选卡片 / 多行输入加起来若超过
        //   页面剩下的高度，Colum 会直接 `BOTTOM OVERFLOWED`。给它一个上限，
        //   里面的弹层用 `Flexible` 自己收 —— 面板永远不会把输入框和底行顶出去。
        //   （桌面没有虚拟键盘，140 只是留给消息区的最小高度，正常永远用不满。）
        final composerMax = (constraints.maxHeight * 0.85)
            .clamp(180.0, constraints.maxHeight)
            .toDouble();
        return Column(
          // `stretch` 不能省略（否则聊天主体与两侧边栏之间出现大片空白）。
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
              // 工作流库只在桌面后端有：手机上不给这个入口（给了也只会弹"读不到"）。
              onPickWorkflow: store.isLocal
                  ? null
                  : () => _pickWorkflow(context),
            ),
            // 「有人等你点批准」的全局提示：钉在对话顶部，只有桌面画
            // （手机上审批归桌面）。没有待办时它自己返回空，不占位。
            if (!store.isLocal) _PendingApprovalBanner(store: store),
            Expanded(
              child: Stack(
                children: [
                  _MessageList(store: store, controller: controller),
                  // 「AI 正在生成」的加载提示**不做成底部浮层**：
                  // 显示在正在生成回复的气泡底部，且只保留转圈动画。
                  // 它现在是该气泡自身的内容，
                  // 见 `_MessageBubble` 里 `isStreaming` 那一段。
                  // 回到聊天底部：**仅在向上滚动之后**才出现，
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
            // 输入区**由提问接管**：AI 等待回答时，
            // 该区域整体替换为提问卡（选项 + 自由输入 + 跳过），不在普通输入框
            // 上叠加卡片，以避免输入位置不明确。
            // 答完（或跳过）`pendingQuestion` 变 null，普通输入框自动回来。
            ConstrainedBox(
              constraints: BoxConstraints(maxHeight: composerMax),
              // 不套外层滚动：需要在**输入框内部**滑动（2026-11-08），
              // 外层滚动会把那次拖动抢走。高度由输入框自己的 `maxLines`
              // 跟着"屏幕 - 键盘 - 卡片 - 底行"算出来（见 `_composerInputMaxLines`）。
              child: questions.isNotEmpty
                  ? _AskComposer(
                      store: store,
                      questions: questions,
                      onSubmit: (text) =>
                          onSubmitted(store, OutgoingMessage.plain(text)),
                    )
                  : _Composer(
                      store: store,
                      controller: input,
                      focusNode: inputFocus,
                      onSubmit: (message) => onSubmitted(store, message),
                      maxHeight: composerMax,
                      // 顶栏精简后，工作流选择从输入框的 `+` 菜单进（功能不丢）。
                      onPickWorkflow: store.isLocal
                          ? null
                          : () => _pickWorkflow(context),
                    ),
            ),
          ],
        );
      },
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

/// 这一栏**仅在"会话列表实际变化"时重建**。
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
        // 按钮与列表之间**不画横线**：这里留白本身就够分开，
        // 一条贯穿整栏的线只会把侧栏切成两块。
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
              // 「选中」与「鼠标按下 / 悬停」必须可区分（次新会话曾显示亮灰底，
              // 比当前会话更接近选中态）。
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
                    // 列表整体保留内边距：此前 ListTile **紧贴**左右边缘，
                    // 选中底色从窗口边缘铺到底，呈现为整条横幅而非单行会话。
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
                        // M3 的选中角色是 `secondaryContainer`，不是"primary 叠 16%"。
                        selectedTileColor: theme.colorScheme.secondaryContainer,
                        selectedColor: theme.colorScheme.onSecondaryContainer,
                        title: Text(
                          c.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        // 条数 + **最近一次对话时间**：
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
/// 对话标题在首次提问时由 AI 自动生成，
/// 如需修改可在此处（或会话列表的「重命名」）编辑。
class _ConversationAppBar extends StatelessWidget
    implements PreferredSizeWidget {
  final AiWorkspaceStore store;

  /// 给「上下文与状态」面板用的入口（可空）。
  ///
  /// **只在第三栏铺不开时给**（< 1200px）：那个宽度下右侧栏不存在，不补这个按钮
  /// 时 Comfy 状态整块内容在界面上无法访问。
  /// 窄屏此前使用右下角 FAB，而 FAB 会覆盖输入区的发送按钮，
  /// 所以现在无论宽窄都走这一个入口。
  final VoidCallback? onShowStatus;

  /// 「选择现有工作流」入口。
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
      // **一个主操作 + 一个 overflow**（AppBar 按钮数量过多）。
      //
      // 以前这里平铺 5 枚图标（工作流 / ComfyUI 状态 / 训练台 / 重命名 / 新建），
      // 窄屏挤、宽屏也在抢标题宽度。M3 的做法是主操作留在外面、其余收进菜单：
      //   · 「新建对话」是这一页唯一的主动作 → `IconButton.filledTonal`；
      //   · 工作流 / 状态 / 重命名 / 训练台 → 「更多」菜单。
      //
      // ⚠ 「选择现有工作流（可看蓝图）」**从顶栏移除**；
      //   工作流选择改从输入框的 `+` 菜单进入。
      actions: [
        AppMenuButton<String>(
          tooltip: '更多操作',
          onSelected: (v) {
            if (v == 'rename') _renameCurrent(context);
            if (v == 'context') onShowStatus?.call();
            if (v == 'workflow') onPickWorkflow?.call();
            if (v == 'train') {
              Navigator.of(context)
                  .push(MaterialPageRoute(builder: (_) => const TrainPage()));
            }
          },
          options: [
            if (conv != null)
              const MenuOption(
                value: 'rename',
                icon: Icons.edit_outlined,
                label: '重命名本对话',
              ),
            if (onShowStatus != null)
              const MenuOption(
                value: 'context',
                icon: Icons.dns_outlined,
                label: '上下文与状态',
              ),
            if (onPickWorkflow != null)
              const MenuOption(
                value: 'workflow',
                icon: Icons.account_tree_outlined,
                label: '选择现有工作流…',
              ),
            const MenuOption(
              value: 'train',
              icon: Icons.school_outlined,
              label: 'LoRA 训练台',
              dividerBefore: true,
            ),
          ],
          // 不再给按钮单独的 tooltip：`AppMenuButton` 已经用 `tooltip` 包了一层，
          // 两处都给会让 `find.byTooltip('更多操作')` 命中两个节点。
          button: (context, controller, isOpen) => IconButton(
            onPressed: () =>
                controller.isOpen ? controller.close() : controller.open(),
            icon: const Icon(Icons.more_vert),
          ),
        ),
        const SizedBox(width: 4),
        IconButton.filledTonal(
          tooltip: '新建对话',
          onPressed: () => store.newConversation(),
          icon: const Icon(Icons.add_comment_outlined, size: 20),
        ),
        const SizedBox(width: 8),
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
/// **精确总长**：滚动条滑块的长度与位置由
/// `pixels / maxScrollExtent` 计算，而懒构建的 `ListView` 对**尚未布局的**
/// 子项只能估算总长 —— 每次滚动都会改变估算值，滑块位置随之跳动
/// （在最新一条聊天记录中，滚动条进度会跳变约 1/3）。
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
/// 不使用 `ListView` 的 `itemExtentBuilder`：
/// 它要求**每一条**都给一个精确高度，而且那个返回值是**硬约束**，差一点就直接
/// 裁内容（`RenderFlex overflowed by 168~512px`）。要在懒构建的前提下
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
/// **硬约束**：差一点就会裁内容（`RenderFlex overflowed by 168~512px`），
/// 量宽度还必须与真实布局宽度逐像素一致 —— 不该塞进这次修复里。
class _MessageList extends StatelessWidget {
  final AiWorkspaceStore store;
  final ScrollController controller;

  const _MessageList({required this.store, required this.controller});

  /// 每一条气泡的外边距。
  ///
  /// 取 24：气泡铺满整栏宽度（不再有 `maxWidth` 上限），左右各留 24 ——
  /// 请求气泡靠右栏、助手气泡靠左栏，中间为可读行宽。
  static const EdgeInsets _itemPadding = EdgeInsets.symmetric(
    horizontal: 24,
    vertical: 6,
  );

  @override
  Widget build(BuildContext context) {
    if (store.messages.isEmpty) return _EmptyThread(store: store);

    // 最后一条用户消息的位置：它**之后**的助手消息还"有效"，之前的助手消息
    // 已经被用户的新提问顶掉 —— 结尾提问卡要据此自动收起（见 `_AskFollowUpCard`）。
    final messages = store.messages;
    var lastUser = -1;
    for (var i = messages.length - 1; i >= 0; i--) {
      if (messages[i].isUser) {
        lastUser = i;
        break;
      }
    }

    return SingleChildScrollView(
      // 聊天气泡列表：界面里还有别的可滚动区域（会话列表、气泡里的清单），
      // 回归用例要能精确点到**这一个**（滚动跟随 / 贴底那几条断言都靠它）。
      key: const Key('ai-message-list'),
      controller: controller,
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Column(
        // `stretch`：**每条消息占满整行**，靠左还是靠右由 `_MessageBubble`
        // 自身的 `crossAxisAlignment` 决定（请求消息靠右）。
        //
        // 这里以前是 `start`，子项因此拿到松约束、缩到"气泡自己那么宽"，
        // 而 `_MessageBubble` 那层 Column 也就跟着缩成一样宽 —— 它里面写的
        // `CrossAxisAlignment.end` 因此**没有效果**：请求消息实际为左对齐，
        // 右侧出现大片空白。
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < messages.length; i++)
            Padding(
              key: ValueKey(messages[i].id),
              padding: _itemPadding,
              // RepaintBoundary / 重建闸门都在 `_MessageBubbleSlot` 里：见它的注释
              // （流式输出时只有**真的变了**的那一条会重建，历史气泡整棵跳过）。
              child: _MessageBubbleSlot(
                store: store,
                message: messages[i],
                superseded: !messages[i].isUser && i < lastUser,
              ),
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

/// 一条气泡的「重建闸门」（避免快速滑动 / 展开流式思考时卡顿）。
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

  /// 这条助手消息是否已被**后来的用户消息**顶掉（见 `_MessageList.build`）。
  ///
  /// 只影响结尾提问卡的默认折叠状态；但它必须进重建闸门 —— 用户发出新提问时
  /// 消息实例本身没变，只看实例会一直用缓存，卡片就不会自动收起。
  final bool superseded;

  const _MessageBubbleSlot({
    required this.store,
    required this.message,
    this.superseded = false,
  });

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
  bool? _cachedSuperseded;

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
    // 权限档同样纳入闸门：**生成过程中切换档位**需立即反映到气泡中，
    // 它不在 `message` 实例里、也不是 `liveRevision`，漏掉就会"切了但字没变"。
    final permission = generating ? store.permissionMode : '';
    final unchanged =
        _cached != null &&
        identical(_cachedMessage, widget.message) &&
        _cachedSending == store.sending &&
        _cachedLive == live &&
        _cachedStreamingId == streamingId &&
        _cachedPermission == permission &&
        _cachedSuperseded == widget.superseded;
    if (unchanged) return _cached!;

    final message = widget.message;
    _cachedMessage = message;
    _cachedSending = store.sending;
    _cachedLive = live;
    _cachedStreamingId = streamingId;
    _cachedPermission = permission;
    _cachedSuperseded = widget.superseded;
    aiChatBubbleBuilds++;
    _cached = RepaintBoundary(
      child: _MessageBubble(
        message: message,
        store: store,
        sending: store.sending,
        // 结尾提问卡：被后来的用户消息顶掉时默认收起
        superseded: widget.superseded,
        // 「AI 正在生成」的转圈动画只绘制在**正在流式生成的那一条**上
        generating: generating,
        // 本轮权限档：**实时**读取 store，显示在正在生成的气泡中。
        // 只在流式期间给值 —— 历史气泡上挂"现在的档位"是错的（后端没按轮存过权限档）。
        permissionLabel: generating ? store.permissionLabel : '',
        // 该回复使用的模型
        modelLabel: message.isUser ? '' : store.modelLabelOf(message.modelId),
        onRetry: store.retry,
        // 工具卡 / 思考都只属于助手消息
        toolCalls: message.isUser ? const [] : store.toolCallsFor(message),
        // 思考 / 正文按**流顺序**还原为段（按到达顺序显示）
        segments: message.isUser ? const [] : store.segmentsFor(message),
        // 这条回复真正产出的产物（回复末尾的「画廊入口卡」）
        producedMediaIds: message.isUser
            ? const []
            : store.producedMediaIds(message),
        // 其中被 AI 声明为成人内容的：产物卡仅对这些打码
        producedNsfwMediaIds: message.isUser
            ? const []
            : store.producedNsfwMediaIds(message),
        // 产出这些图的工作流也属于"生成的产物"
        producedPromptIds: message.isUser
            ? const []
            : store.producedPromptIds(message),
        toolCategoryOf: store.toolCategoryOf,
        onApprove: store.approveToolCall,
        onDeny: store.denyToolCall,
        // 提问：回答后同一 Run 继续执行，不新建消息
        onAnswerQuestion:
            (questionId, {int? optionIndex, String? text}) async =>
                (await store.answerQuestion(
                  questionId,
                  optionIndex: optionIndex,
                  text: text,
                )).ok,
        onSkipQuestion: (questionId) async =>
            (await store.skipQuestion(questionId)).ok,
        // 卡片中"自己输入答案"→ 焦点交给底部被接管的答案框
        onFocusAskAnswer: () => store.pendingQuestionFocus.requestFocus(),
        // 提问卡里的裁图是后端给的相对路径（`/api/ai/attachments/{id}/file`）
        apiBase: store.baseUrl,
        // 消息中的附件块：按 id 获取缩略图 / 视频预览帧（M3）
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

  /// 会话状态：结尾提问卡要在这里记录作答并发出下一条消息。
  final AiWorkspaceStore store;
  final bool sending;

  /// 表示**正在流式生成**的那一条：
  /// 「AI 正在生成」只在它的底部画一个转圈动画。
  final bool generating;

  /// 生成期间显示的本轮权限档。
  ///
  /// 该值取**当前时刻**的档位（实时读取 `store.permissionMode`），因此生成过程中
  /// 切换档位时，气泡中的文字会立即更新。
  /// 非流式期间为空：后端未按轮保存权限档，在历史气泡上显示"当前档位"不准确。
  final String permissionLabel;

  /// 该回复**当时**使用的模型名。
  ///
  /// 取的是消息自己记的 `modelId`（后端逐条落库），不是"当前选中的模型"——
  /// 所以往上翻历史时看到的是每轮各自的模型，切换模型那轮一眼就能看出来。
  /// 请求消息不需要该字段（显示的是"谁问的"，不是"谁答的"）。
  final String modelLabel;

  final Future<void> Function(String assistantMessageId) onRetry;

  /// 这条消息的有序工具调用（M4）。
  final List<AiToolCallState> toolCalls;

  /// 这条助手消息是否已经被**后来的用户消息**顶掉（见 `_MessageList.build`）。
  ///
  /// 只用于结尾提问卡的默认折叠：用户已经往下问了，旧提问不该继续摊开占版面。
  final bool superseded;

  /// 这条消息按流顺序排好的段：思考段 + 正文段（`reasoning` / `text`）。
  final List<AiMessageSegment> segments;

  /// 这次回复真正产出的画廊产物 id（`comfy_submit` 的 mediaIds）。
  final List<int> producedMediaIds;

  /// 其中被 AI 声明为成人内容的那些（`comfy_submit` 的 `nsfwMediaIds`）。
  final List<int> producedNsfwMediaIds;

  /// 这次回复入库的提示词 id（`comfy_submit` 的 capturedPromptId）。
  ///
  /// "生成的产物"包括生成的工作流 —— 借助该字段，回复末尾的卡片
  /// 可以直接打开本次实际运行的工作流（也可拖回 ComfyUI 复现）。
  final List<int> producedPromptIds;

  final String Function(String toolName) toolCategoryOf;
  final Future<void> Function(String callId) onApprove;
  final Future<void> Function(String callId) onDeny;

  /// 回答 / 跳过 AI 的提问。
  final Future<bool> Function(
    String questionId, {
    int? optionIndex,
    String? text,
  })
  onAnswerQuestion;
  final Future<bool> Function(String questionId) onSkipQuestion;

  /// "自己输入答案"：将焦点交给**底部被提问接管的输入区**。
  ///
  /// 气泡里的提问卡刻意**不带输入框**（否则一屏两个框，不知道往哪儿写），
  /// 所以这一行只做一件事：把光标送到下面那个唯一的答案框里。
  final VoidCallback onFocusAskAnswer;

  /// 后端根地址（提问卡里的裁图是相对路径，显示前补它）。
  final String apiBase;

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
    required this.store,
    required this.sending,
    this.generating = false,
    this.permissionLabel = '',
    this.modelLabel = '',
    required this.onRetry,
    this.toolCalls = const [],
    this.superseded = false,
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
    this.apiBase = '',
    required this.thumbUrlOf,
    required this.fileUrlOf,
    required this.mediaThumbUrlOf,
    required this.mediaPosterUrlOf,
  });

  /// 气泡正文的有序块：思考 / 正文 / 工具卡 / **环节组**，顺序 = `parts` 的顺序（= 消息获取顺序）。
  ///
  /// **严格按照消息到达顺序**排列，不将最终回复放在最终思考之前：
  /// 早先这里是"所有思考与正文段先铺完、工具卡统统一坨挂在最下面"，
  /// 一轮回复里工具卡就跑到最终正文后面去了 —— 那也是重排。
  ///
  /// `parts` 还没到的流式阶段退回 `segments`：实时段里除了思考 / 正文 / 环节，还有
  /// **工具卡到达时的位置**（`type == 'tool_call'`，`text` 是 callId，见
  /// `AiWorkspaceStore._appendLiveToolCall`）。少了后者，流式期间工具卡就只能补在末尾，
  /// "思考 → 工具 → 正文"被画成"思考 → 正文 → 工具"，直到最终答复落库才跳回正确位置
  /// （曾出现顺序错位）。
  ///
  /// **关键步骤（环节）折叠**：`step` 块开一组，把**紧接着**的
  /// 工具卡收进那一组（默认折叠，见 [`_StepGroup`]）。两条纪律：
  ///  ① **只收连续的工具卡** —— 中间夹了正文 / 思考（或下一个环节）就先把这一组交出去
  ///     （宁可同一个环节名多一个组头，也不把正文挪到工具卡后面去：正文与工具卡的先后
  ///     为固定行为，不能修改）；
  ///  ② 同一环节名再次开组时标「续」，让人一眼看出这是同一个环节的第二段操作。
  List<Widget> _orderedBody(BuildContext context, ThemeData theme) {
    final m = message;
    // 手机排队提交的回执（后端 id 形如 `submit-reply-<queueId>`）：
    // 画成**独立的提交结果卡**（成功带产物缩略图、失败带原因原文），
    // 不再按普通 Markdown 正文混在聊天里 —— 否则失败和一句解释长得一样，
    // 已批准但界面显示为"没有反应"（2026-11-08 真机）。
    if (m.id.startsWith('submit-reply-')) {
      return [
        SubmitResultCard(
          message: m,
          mediaThumbUrlOf: mediaThumbUrlOf,
          // 同 `_ProducedMediaCard`：手机上的缩略图要靠这个头才不是破图
          mediaHeaders: attachmentPreviewHeaders(),
          onOpenMedia: (mediaId, mediaIds) => Navigator.of(context).push(
            MaterialPageRoute<void>(
              builder: (_) => MediaDetailPage(
                mediaId: mediaId,
                // 回执上的这一批就是详情页里的"一组"：只在这几张里左右切换，
                // 不退回它们在画廊各自所属的分组。
                siblingIds: mediaIds,
              ),
            ),
          ),
        ),
      ];
    }
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
        // 请求消息是**纯文本**，不走 Markdown（按输入原样显示）。
        if (m.isUser) {
          blocks.add(Text(visible));
        } else {
          blocks.add(
            MarkdownText(
              visible,
              style: theme.textTheme.bodyMedium,
              selectable: false,
            ),
          );
        }
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
        if (part.type == 'mention') {
          flush();
          final chip = _mentionChip(context, part);
          if (chip != null) blocks.add(chip);
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

  /// 消息中的 `@` 卡片 / `/技能`：一个小卡片，点开查看**发送那一刻**的快照。
  Widget? _mentionChip(BuildContext context, AiMessagePart part) {
    final payload = part.payload;
    if (payload == null) return null;
    final kind = payload['kind'] as String? ?? '';
    final name = payload['name'] as String? ?? '';
    if (name.isEmpty) return null;
    late final String label;
    late final IconData icon;
    if (kind == 'style_card') {
      label = '画风卡';
      icon = Icons.palette_outlined;
    } else if (kind == 'character_card') {
      label = '角色卡';
      icon = Icons.person_outline;
    } else if (kind == 'skill') {
      label = '技能';
      icon = Icons.auto_awesome_outlined;
    } else {
      label = '引用';
      icon = Icons.link;
    }
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Align(
        alignment: Alignment.centerLeft,
        child: InputChip(
          key: ValueKey('mention-$kind-$name'),
          avatar: Icon(icon, size: 16),
          label: Text('$label · $name'),
          visualDensity: VisualDensity.compact,
          onPressed: () => _showMentionSnapshot(context, kind, name, payload),
        ),
      ),
    );
  }

  /// 快照弹窗：卡看正面 / 负面提示词，技能看这次调用时的正文与版本。
  void _showMentionSnapshot(
    BuildContext context,
    String kind,
    String name,
    Map<String, dynamic> payload,
  ) {
    final snapshot = payload['snapshot'] is Map
        ? Map<String, dynamic>.from(payload['snapshot'] as Map)
        : const <String, dynamic>{};
    final title = kind == 'skill'
        ? '技能 · $name'
        : kind == 'style_card'
        ? '画风卡 · $name'
        : '角色卡 · $name';
    Widget section(String label, String? value) {
      if (value == null || value.trim().isEmpty) {
        return const SizedBox.shrink();
      }
      return Padding(
        padding: const EdgeInsets.only(bottom: 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(label, style: const TextStyle(fontWeight: FontWeight.w600)),
            const SizedBox(height: 4),
            SelectableText(
              value,
              style: const TextStyle(fontFamily: 'Consolas', fontSize: 12),
            ),
          ],
        ),
      );
    }

    showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 560),
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (kind == 'skill') ...[
                  if ((payload['version'] as String?)?.isNotEmpty ?? false)
                    Text('版本 ${payload['version']}'),
                  const SizedBox(height: 8),
                  section(
                    '调用时的技能正文',
                    payload['body'] as String? ?? '（这条消息发送时没有留下正文快照）',
                  ),
                ] else ...[
                  section('正面提示词', snapshot['prompt'] as String?),
                  section('负面提示词', snapshot['negative'] as String?),
                ],
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('关闭'),
          ),
        ],
      ),
    );
  }

  Widget _toolCard(AiToolCallState call) => Padding(
    padding: const EdgeInsets.only(top: 8),
    // 提问渲染为**提问卡而非工具卡**：一个是"等待批准"，
    // 一个是"等待回答"，混在一起会被理解为点击即批准。
    child: call.allQuestions.isNotEmpty
        // 结尾提问（`ask_follow_up`）：非阻塞的变种 —— 问题贴在这一轮末尾，
        // 卡片自己就是作答入口；提交后拼一条用户消息、开下一轮。
        ? (call.name == 'ask_follow_up'
              ? _AskFollowUpCard(
                  key: ValueKey('ask-follow-up-${message.id}-${call.callId}'),
                  questions: call.allQuestions,
                  apiBase: apiBase,
                  store: store,
                  submitText: call.submitText,
                  // 这一轮还在生成 / 还在发送时不能提交（要等它结束）
                  enabled: !sending,
                  // 用户已经往下问了：旧提问默认收成一行
                  superseded: superseded,
                )
              : _AskCard(
                  key: ValueKey('${message.id}-${call.callId}'),
                  // 整张卡（同组的所有题）——多问改造后只给一题会让别的题在界面上消失
                  questions: call.allQuestions,
                  apiBase: apiBase,
                  onAnswer: onAnswerQuestion,
                  onSkip: onSkipQuestion,
                  // "自己输入答案"一行将焦点交给底部被接管的输入框
                  onFocusAnswer: onFocusAskAnswer,
                ))
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
    // 请求气泡 = primaryContainer（M3 的"我"侧），助手气泡 = surfaceContainerHigh
    // （实色，不再叠 alpha：叠色在深色下会发灰、在动态取色下还会串色）。
    final bg = m.isUser
        ? theme.colorScheme.primaryContainer
        : theme.colorScheme.surfaceContainerHigh;
    // M3 表达性气泡：大圆角 + 靠近说话人的一角收小（请求消息靠右、助手靠左）。
    final bubbleShape = BorderRadius.only(
      topLeft: const Radius.circular(16),
      topRight: const Radius.circular(16),
      bottomLeft: Radius.circular(m.isUser ? 16 : 4),
      bottomRight: Radius.circular(m.isUser ? 4 : 16),
    );
    // 失败/取消的助手消息可以重试：新建一个 Run，用 retryOfRunId 关联回去（AIH-024）
    final canRetry =
        !m.isUser &&
        (m.status == 'failed' || m.status == 'cancelled') &&
        !sending;
    // 待批准的工具调用（M4）：气泡里要能点「批准 / 拒绝」。
    // ⚠ 提问卡也用 `pendingApproval` 这个状态（"等你回答"），但它**不是审批**：
    // 混入后顶部会多出"有 1 个工具调用等待批准"，而找不到批准按钮。
    final pending = toolCalls
        .where((c) => c.needsApproval && c.allQuestions.isEmpty)
        .toList();
    // 正文块只算一次：下面"退回整段正文"的兜底要用它判断"有序块与段是不是都没产出内容"
    final body = _orderedBody(context, theme);

    return Column(
      crossAxisAlignment: align,
      children: [
        Container(
          // 气泡本体（内含底色那一块）在用例里要能精确取到矩形 ——
          // "转圈动画贴在正在生成的那条气泡底部"这条断言就靠它。
          key: ValueKey('ai-bubble-${m.id}'),
          // **助手气泡铺满整行，请求气泡按内容收缩**。
          //
          // 助手这一侧是"内容面板"（正文 / 工具卡 / 产物卡 / 思考过程…），宽度
          // 只能由版面决定：一旦让 `Container` 收缩到内容宽度，流式期间气泡宽度
          // 就会跟着"当前最后一行有多长"一抽一抽地跳 —— 思考过程最明显，它折叠时
          // 显示的是**最后一行**，换行时气泡右边缘随之变化。
          // 请求侧为"消息气泡"：按内容收缩并靠右。
          //
          // 此前依赖外层 `stretch` 撑满（空间利用率低），
          // 但 `stretch` 只作用到"每条消息占满整行"那一层：这一层 Column 的
          // `crossAxisAlignment` 是 start/end，气泡拿到的是**松约束**、又缩回内容
          // 宽度了（这也是 057c47b 那句"气泡由 stretch 撑满整行"没兑现的原因）。
          // 所以 `width: double.infinity` 不能省：它把助手气泡钉在整行宽上。
          width: m.isUser ? null : double.infinity,
          margin: const EdgeInsets.symmetric(vertical: 6),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          decoration: BoxDecoration(color: bg, borderRadius: bubbleShape),
          // 整条气泡为一个选择域：
          // 一段回复被解析成很多个块（段落 / 标题 / 列表 / 表格 / 代码块），
          // 每块一个 SelectableText 的话，鼠标拖到当前块末尾就拉不动了。
          // 包一层 SelectionArea、内部一律用 Text，选择就能跨段连续拉下去。
          child: SelectionArea(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // 思考 / 正文 / 工具卡**按 parts 的真实顺序**交错渲染
                // （严格按消息到达顺序排列，不重排）。
                // 后端给的 parts 就是权威的有序块；`segments` 是它的降级版本（只有思考与正文段），
                // 流式过程中 parts 还没到，就用它 + 末尾追加还没归位的工具卡。
                ...body,
                // 附件块（只出现在请求轮）
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
                // 请求消息为纯文本，不走 Markdown。
                //
                // ⚠️ 判据必须是"正文块为空"，**不能**写成 `segments.isEmpty`：
                // 请求消息的 `segments` 刻意传空（走 parts 路径），该条件对它**始终成立**，
                // 导致正文被渲染两遍 —— 带附件的消息中
                // 同一句话上下各一份、图片正好夹在中间（`_orderedBody` 一份 + 这里一份）。
                if (body.isEmpty && m.text.isNotEmpty)
                  m.isUser
                      ? Text(m.text)
                      : MarkdownText(
                          m.text,
                          style: theme.textTheme.bodyMedium,
                          selectable: false,
                        ),
                // 「AI 正在生成」：**显示在该气泡自身的底部**，且只有一个转圈动画
                // （不放在页面底部，只显示在正在生成的气泡底部）。
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
                        // 本轮权限档：显示在转圈动画旁。
                        // 生成过程中切换档位时，该行文字**立即**更新 ——
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
                            '本轮权限是 $permissionLabel',
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
                // 画廊入口卡：本次回复**实际生成产物**时，
                // 在末尾提供可打开详情的入口，不需要到画廊中查找。
                // 该卡也包含"生成它的工作流"（一次运行产生的图 + 该工作流
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
                      // 手机读的是电脑的后端：缩略图地址里没有令牌，得靠这个头
                      mediaHeaders: attachmentPreviewHeaders(),
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
                        // M3 的最小触控高度是 48；聊天里的次要按钮压到 44，
                        // 既不再出现"28~32 高、点不准"的小按钮，也不把气泡撑得太胖。
                        minimumSize: const Size(0, 44),
                        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      ),
                    ),
                  ),
                // 元信息（模型 / 思考 / token）在气泡**外面**渲染 —— 见下面 `_metaRow`。
              ],
            ),
          ),
        ),
        // 本轮的生成信息：模型 + 思考强度 + token 用量。
        //
        // 放在气泡**外面**：它属于元信息，不属于回复内容 ——
        // 放入气泡会被 `SelectionArea` 一并复制（选中的文本中会混入非正文内容），
        // 放在外面不可选中，也能看出属于哪一条。
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
          // 本轮使用的模型：切换模型的那一轮可直接辨认。
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
        color: theme.colorScheme.surface,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 展开判定区：**整条标题行 + 副标题**都能点。
          //
          // 以前只有箭头那个 16px 图标接手势，命中区小到"点了没反应"；
          // 现在标题 / 状态 / 耗时 / 副标题整块都是同一块命中区。
          // 没有明细的卡不接手势：点了也不会展开，比不能点更让人困惑。
          _expandTarget(
            Tooltip(
              // 带工具名的 tooltip：便于定位"展开哪一个工具的结果"
              message: '${_expanded ? '收起' : '展开'} ${call.displayTitle} 的结果',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(_icon, size: 14, color: color),
                      const SizedBox(width: 6),
                      // 标题独占"扣掉状态、耗时、展开键之后"的整行宽度。
                      // 标题与 `Spacer` 同为 flex 1 时，剩余空间按 1:1 分给两者，
                      // 长标题只能用到一半就被省略号截断，右边却空着一大片；
                      // 去掉 `Spacer` 后展开键也回到耗时旁边，不再被推到行的另一端。
                      Expanded(
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
                      if (call.isRunning) ...[
                        const SizedBox(width: 6),
                        const SizedBox(
                          width: 12,
                          height: 12,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                      ],
                      if (call.hasDetail) ...[
                        const SizedBox(width: 4),
                        Padding(
                          padding: const EdgeInsets.all(2),
                          child: Icon(
                            _expanded ? Icons.expand_less : Icons.expand_more,
                            size: 16,
                            color: theme.colorScheme.outline,
                          ),
                        ),
                      ],
                    ],
                  ),
                  // 副标题：`用途 · 机器名`。机器名保留在界面上，但不是首要信息。
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
                ],
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
                      minimumSize: const Size(0, 44),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                    ),
                    child: const Text('批准'),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton(
                    onPressed: _busy ? null : () => _resolve(approve: false),
                    style: OutlinedButton.styleFrom(
                      padding: const EdgeInsets.symmetric(horizontal: 12),
                      minimumSize: const Size(0, 44),
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
                  // 三块**各自一个容器**（参数 / 结果分开显示，
                  // 有说明时放第三块）。一块展开即三块一起展开。
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

  /// 把"可展开的头部"包成整块可点：命中区从 16px 的箭头扩大到整行。
  ///
  /// 只有真有明细（`hasDetail`）时才接手势：没有明细的卡点了也不会展开，
  /// 那比不能点更让人困惑。
  Widget _expandTarget(Widget child) {
    if (!widget.call.hasDetail) return child;
    return Material(
      type: MaterialType.transparency,
      child: InkWell(
        onTap: () => setState(() => _expanded = !_expanded),
        borderRadius: BorderRadius.circular(6),
        child: child,
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
        color: theme.colorScheme.surfaceContainerHighest,
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

/// 回复末尾的**画廊入口卡**：
/// "如果让 AI 调用 comfy 生成了产物，可以在回复末尾贴上一个类似画廊项目的入口，点开即查看对应详情"。
///
/// 只显示**后端确认入库**的产物（媒体 id 来自工具的结构化结果），
/// 所以这里不会出现"AI 说生成了但其实没有"的假入口。
///
/// 缩略图走**画廊**的 `/api/media/{id}/thumb`（视频退到 `/poster`）：
/// 早先这里复用了附件那两条接口（`thumbUrlOf(id)`），id 空间不同，必然 404 ——
/// 表现为"生成产物后预览图不可用"。
class _ProducedMediaCard extends StatelessWidget {
  final List<int> mediaIds;

  /// 这一批里**被标记为成人内容**的那些（`comfy_submit` 声明了 `nsfw=true`）。
  ///
  /// 模糊开关开启时，回复中的产物卡曾把所有图打码 ——
  /// 只有 AI 明说过"这次是成人内容"的那几张才该糊。
  final List<int> nsfwMediaIds;

  /// 产出这些图的提示词 / 工作流。
  final List<int> promptIds;

  final String Function(int mediaId) mediaThumbUrlOf;
  final String Function(int mediaId) mediaPosterUrlOf;

  /// 拉缩略图 / 封面时带的请求头（配对令牌；桌面上恒为空）。
  ///
  /// 媒体地址里不带令牌，而手机读的是电脑的后端 —— 闸门开着时没有这个头一律 403，
  /// 表现是"产物卡是破图"（2026-11-09 真机）。与 `SubmitResultCard` 同一个来源。
  final Map<String, String> mediaHeaders;

  const _ProducedMediaCard({
    required this.mediaIds,
    this.nsfwMediaIds = const [],
    this.promptIds = const [],
    required this.mediaThumbUrlOf,
    required this.mediaPosterUrlOf,
    this.mediaHeaders = const {},
  });

  /// 标题：图 + 工作流属于同一批产物，按数量分别说明。
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
    // 全部列出。此前使用 `take(6)`，一批 10 张时
    // 后 4 张在卡上根本看不到；这里是横向懒构建，列多少都不会一次全解码。
    final shown = mediaIds;
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
                      key: ValueKey('produced-media-$id'),
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
                            headers: mediaHeaders,
                            fit: BoxFit.cover,
                            // 图片有 thumb；视频的 thumb 会回 204，退到后端抽的第一帧封面
                            errorBuilder: (_, _, _) => Image.network(
                              mediaPosterUrlOf(id),
                              headers: mediaHeaders,
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
                // "生成的产物"包括生成的工作流：
                // 直接打开本次实际运行的工作流，不需要到画廊中查找
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
    minimumSize: const Size(0, 44),
    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
  );

  void _openDetail(BuildContext context, int mediaId) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => MediaDetailPage(
          mediaId: mediaId,
          // 这一批产物就是详情页里的"一组"：卡上能看到的几张，左右切换也只在这几张里走。
          // 只有一张时也传（这一组就一张，不再展开到画廊里的分组）。
          //
          // 只传 id（缩略图条里只有 id，元数据由详情页补齐）。不传的话详情页
          // 会按 `collectionId` 去列画廊集合 —— 一批产物可能来自多次提交，
          // 在画廊里落在各自的分组里，于是点开哪一张就进它自己那个分组。
          siblingIds: mediaIds,
        ),
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
///    （固定行为），同时该环节的操作**不会遗漏在外**；
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
/// 关键步骤之间的操作可折叠，环节名用于说明当前步骤。
/// 环节名由**后端**从 `[步骤]…[/步骤]` 中提取，
/// 只可能是那六个固定名字（见 `server-rs/src/step_mark.rs`）—— 前端这里不再做任何猜名。
///
/// 三条约束：
///  ① **默认折叠**（与"思考过程"同一条：过程让位给正文），但收起时**不是藏起来** ——
///     标题上带"N 个操作"，一眼能看出这里做过几件事；
///  ② **有待处理项时不得折叠**（待批准 / 失败 / 被拒）：批准按钮位于折叠区时，
///     无法确定点击位置（呈现为"AI 卡住"）；
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
  /// 是否由手动展开（默认 false = 折叠）。
  bool _userExpanded = false;

  /// 该组是否包含"待处理"项（处置见类注释第 ② 条）。
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

/// 思考过程：**默认折叠**。
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
  /// [fromEnd] = true 时取**末尾一行**（流式生成中显示当前进度）；
  /// 生成结束后回到开头（显示思考内容）。取末尾同样只看最后
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
    final summary = widget.text.trim().isEmpty
        ? ''
        // 生成中看"末尾一行"（现在想到哪了）；结束后看开头（这段在讲什么）。
        : summaryOf(widget.text, fromEnd: widget.streaming);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 整行可点：命中区从"思考过程 思考中…"这几个字扩大到整行 + 摘要。
          // 以前只有那一小截文字接手势，下面写着"点击展开完整思考过程"却点不动。
          InkWell(
            onTap: () => setState(() => _expanded = !_expanded),
            borderRadius: BorderRadius.circular(6),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 4),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    mainAxisSize: MainAxisSize.max,
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
                  if (!_expanded && summary.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(left: 20, top: 2),
                      child: Text(
                        summary,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.outline.withValues(
                            alpha: 0.85,
                          ),
                        ),
                      ),
                    ),
                ],
              ),
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
/// 内层再开一个选择域时，拖选到思考段末尾无法继续选中下方正文。
///
/// 这里解决两个问题：
///
///  ① **展开正在持续流式输出的思考过程后卡顿**：以前整段思考是一个 `Text`，
///     每来一个字都要把**几万字**重新排版一遍（一帧几十毫秒，直接掉帧）。
///     现在按固定行数切成小块、**没变过的块复用上一次的 widget**
///     （widget 相同 → Flutter 整棵子树跳过，`RenderParagraph` 连布局都不重做），
///     于是每帧只重排"最后那一块"。
///  ② **展开后无法及时刷新**：内层滚动条停在自己原来的位置，新思考长在可视区下面，
///     看起来像"不刷新"。现在流式期间和聊天列表同一套纪律 —— **贴底才跟随**，
///     主动向上滚动时不打扰。
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

  /// 贴底容差：只吸收亚像素误差，不是"接近底部"（聊天列表中也出现过同类问题，
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
    // 键盘弹起时可用高度会缩到一半 —— 直接 `Center` 一个固定内容会
    // `BOTTOM OVERFLOWED`（真机 2026-11-08 实测 55px）。放不下就让它滚。
    return LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        child: ConstrainedBox(
          constraints: BoxConstraints(minHeight: constraints.maxHeight),
          child: Center(
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
                      '输入 / 可以调出 Skills 目录，也可以把文件拖进来当附件。',
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
                            '还没有配置 Provider。请到「设置 → AI 模型」添加协议、Base URL 和模型目录，'
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
  final Future<void> Function(OutgoingMessage message) onSubmit;

  /// 「选择现有工作流…」（从顶栏搬到 `+` 菜单）。null = 这个端没有工作流库。
  final Future<void> Function()? onPickWorkflow;

  /// 输入区**能用的最大高度**（页面按"body 高度 × 0.85"算好传进来）。
  ///
  /// 输入框行数就按它减去卡片 / 底行来算，保证任何键盘高度下都不会顶到 AppBar。
  final double maxHeight;

  const _Composer({
    required this.store,
    required this.controller,
    required this.focusNode,
    required this.onSubmit,
    this.onPickWorkflow,
    required this.maxHeight,
  });

  @override
  State<_Composer> createState() => _ComposerState();
}

class _ComposerState extends State<_Composer> {
  /// 中文输入法正在组字（拼音还没上屏）。
  ///
  /// 以前这里是个**永远是 false** 的布尔字段（只在 `onChanged` 里被赋 false），
  /// 等于没有这道闸：组字期间按回车会把半截拼音当消息发出，并同时清空输入框 ——
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
    // 拖入的文件**作为附件**挂载。
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
    //
    // ⚠ 只处理**文本确实变化**的那一次：关闭弹层后焦点回到输入框，
    // controller 的 selection 变化同样会触发监听；仅判断"行末为 @"时会
    // **再次弹出**，需要取消两次才能继续输入。用 `_atHandled` 记录该次 @ 已处理。
    final atEnd = RegExp(r'(?:^|\s)@$').hasMatch(text);
    if (!_cardsOpen && atEnd && text != _atHandled) {
      _atHandled = text;
      _openCards();
    } else if (!atEnd) {
      _atHandled = null;
    }
    // ★ 两个弹层**互斥**（同时开启会叠加并溢出）。
    //   输入 `/` 表示需要 skill 菜单，卡片面板立即收起。
    if (next != null && _cardsOpen) {
      _cardsOpen = false;
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

  /// 这一条消息要带上的卡：**角色卡可以多张**（多角色同框），画风卡最多一张。
  ///
  /// 发送时每张卡各拼一段固定标记的文本（`[STYLE SPEC]` / `[CHARACTER CARD]`），
  /// 正文**逐字**带上 —— skill `r2i-prompt` 认的就是这些槽位。
  /// 角色卡按**选择顺序**排列（从 @ 面板逐个点选）。
  final List<CharacterCard> _characterCards = [];
  StyleCard? _styleCard;

  /// 卡片面板是否展开。
  ///
  /// 选择一张后收起；选择多张时再次输入 `@`。
  /// 面板高度也按可用高度收（键盘弹起时不顶出屏幕）。
  bool _cardsOpen = false;

  /// 已经处理过的"行末 `@`"文本。用来防止焦点回到输入框时**重复弹层**。
  String? _atHandled;

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
    final phone =
        Theme.of(context).platform == TargetPlatform.android ||
        Theme.of(context).platform == TargetPlatform.iOS;
    if (phone) {
      // 手机：**底部弹层**。面板不参与输入区排版，键盘弹起也不会把
      // 元素挤压成一条（窗口高度不足时难以操作）。
      FocusManager.instance.primaryFocus?.unfocus();
      await _ensureCards(refresh: true);
      if (!mounted) return;
      var picked = false;
      await showModalBottomSheet<void>(
        context: context,
        isScrollControlled: true,
        useSafeArea: true,
        backgroundColor: Colors.transparent,
        builder: (ctx) {
          final media = MediaQuery.of(ctx);
          final height = ((media.size.height - media.viewInsets.bottom) * 0.66)
              .clamp(280.0, media.size.height * 0.82)
              .toDouble();
          return Padding(
            padding: EdgeInsets.only(bottom: media.viewInsets.bottom),
            child: SizedBox(
              height: height,
              child: _CardPickerSheet(
                characters: _characters,
                styles: _styles,
                selectedCharacterIds: _characterCards.map((c) => c.id).toSet(),
                selectedStyleId: _styleCard?.id,
                initialStyleTab: style,
                loading: _cardsLoading,
                error: _cardsError,
                onPickCharacter: (card) {
                  picked = true;
                  _chooseCard(card);
                  Navigator.of(ctx).pop();
                },
                onPickStyle: (card) {
                  picked = true;
                  _chooseCard(card);
                  Navigator.of(ctx).pop();
                },
              ),
            ),
          );
        },
      );
      if (!mounted) return;
      if (!picked) {
        // 取消选择：把行末那个 `@` 摘掉，再把焦点还给输入框。
        // 不摘除时继续输入会变成"@ 后接正文"，且下次点击会再次弹出面板
        //（取消后需再次取消才能输入）。
        final text = widget.controller.text;
        final match = RegExp(r'(?:^|\s)@$').firstMatch(text);
        if (match != null) {
          final next = text.substring(0, match.end - 1);
          widget.controller.value = TextEditingValue(
            text: next,
            selection: TextSelection.collapsed(offset: next.length),
          );
        }
      }
      widget.focusNode.requestFocus();
      return;
    }

    setState(() {
      _cardsOpen = true;
      // 与 `/` 菜单互斥：同一时刻只留一个弹层（否则两个叠在一起必溢出）
      _slashQuery = null;
      _cardsStyled = style;
      _cardsError = null;
    });
    await _ensureCards(refresh: true);
  }

  /// 拉一次卡片库（两类一起）。失败把原因写进 [\_cardsError]，不抛。
  ///
  /// **每次打开面板都重拉**（[refresh]）：卡片可能在「预设库」里刚改过，
  /// 也可能刚被 AI 的 `save_*_card` 覆盖过。只拉一次会让「`@` 面板里的卡」
  /// 和「卡片库里的卡」不是同一版 —— 用户看到的正文与模型拿到的正文不一致。
  Future<void> _ensureCards({bool refresh = false}) async {
    if (_cardsLoading) return;
    if (!refresh && (_characters.isNotEmpty || _styles.isNotEmpty)) return;
    setState(() => _cardsLoading = true);
    try {
      final characters = await widget.store.listCharacterCards();
      final styles = await widget.store.listStyleCards();
      if (!mounted) return;
      setState(() {
        _characters = characters;
        _styles = styles;
        // 已选中的卡按 id 换成刚拉到的那一份：卡被改过时，下一次发送用新正文，
        // 不再拿打开面板之前的旧对象（同一张卡的对象可能已经换了）。
        final style = _styleCard;
        if (style != null) {
          _styleCard = styles.firstWhere(
            (c) => c.id == style.id,
            orElse: () => style,
          );
        }
        for (var i = 0; i < _characterCards.length; i++) {
          final card = _characterCards[i];
          _characterCards[i] = characters.firstWhere(
            (c) => c.id == card.id,
            orElse: () => card,
          );
        }
        _cardsLoading = false;
        _cardsError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _cardsLoading = false;
        _cardsError = '读不到卡片库，$e';
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

  /// 点一张卡 = **切换选中**；面板不收起，方便接着选第二、第三张。
  ///
  /// 角色卡为多选（同框 / 对话均依赖它），再次点击同一张即取消。
  /// 画风卡只有一张（一张图只有一套渲染规格），选择新的替换旧的，再次点击取消。
  void _chooseCard(CharacterCard card) {
    if ((card.prompt ?? '').trim().isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('这张卡的正面提示词还空着，先去「预设库 → 卡片库」补上再用。')),
      );
      return;
    }
    setState(() {
      if (card is StyleCard) {
        _styleCard = identical(_styleCard, card) ? null : card;
      } else {
        final index = _characterCards.indexWhere((c) => c.id == card.id);
        if (index >= 0) {
          _characterCards.removeAt(index);
        } else {
          _characterCards.add(card);
        }
      }
      // 选完清掉搜索词：下一张常常是另一个名字，不用先手动删一遍。
      _cardQuery = '';
      _cardSearch.clear();
      // ★ 选择后收起。
      //   要再选一张就再打 `@` 或点 `+`；面板常驻会把输入区顶出屏幕。
      _cardsOpen = false;
    });
    // 面板收起、键盘已收起（打开时），将焦点还给输入框，以便继续输入正文。
    widget.focusNode.requestFocus();
    // 从键盘 `@` 进入的路径：移除行末查询词，只保留已输入的正文。
    final text = widget.controller.text;
    final trigger = RegExp(r'(?:^|\s)@[^\s@]*$').firstMatch(text);
    if (trigger != null) {
      final next = text.substring(0, trigger.start).trimRight();
      widget.controller.value = TextEditingValue(
        text: next,
        selection: TextSelection.collapsed(offset: next.length),
      );
    }
    // 焦点交回输入框：选完就能直接写要求（老手感），面板留着 —— 想再加一张点一下就行。
    widget.focusNode.requestFocus();
  }

  /// 行首的 `/技能名`（与后端 `valid_name` 同一条字符集）。
  static final RegExp _skillPrefix = RegExp(
    r'^\s*/([a-z0-9][a-z0-9-]{0,63})(?:\s+|$)',
  );

  /// 把选中的卡拼到消息前面（画风卡一段；角色卡**一张一段**，按选中顺序）。
  ///
  /// 槽位名与 skill `r2i-prompt` 的输入约定逐字一致；卡正文**一个字符都不改**。
  /// 多段 `[CHARACTER CARD]` = 多角色同框，skill 里写清了怎么处理。
  ///
  /// 同时把每张卡的**发送时快照**与 `/技能` 一起交给后端：界面气泡按 mention 块画小卡片，
  /// 模型仍拿到同样顺序的卡正文。
  OutgoingMessage _composeOutgoingMessage(String raw) {
    var bodyText = raw;
    AiSkill? invoked;
    final match = _skillPrefix.firstMatch(raw);
    if (match != null) {
      final name = match.group(1)!;
      for (final skill in widget.store.skills) {
        if (skill.name == name &&
            skill.enabled &&
            skill.userInvocable &&
            skill.validationError == null) {
          invoked = skill;
          bodyText = raw.substring(match.end);
          break;
        }
      }
    }

    final blocks = <String>[];
    final mentions = <OutgoingMention>[];
    final style = _styleCard;
    if (style != null) {
      final block = _cardBlock(
        marker: '[STYLE SPEC]',
        negativeMarker: '[STYLE NEGATIVE]',
        name: style.name,
        prompt: style.prompt,
        negative: style.negative,
      );
      blocks.add(block);
      mentions.add(
        OutgoingMention(
          kind: 'style_card',
          name: style.name,
          modelText: block,
          snapshot: _cardSnapshot(style),
        ),
      );
    }
    for (final character in _characterCards) {
      final block = _cardBlock(
        marker: '[CHARACTER CARD]',
        negativeMarker: '[CHARACTER NEGATIVE]',
        name: character.name,
        prompt: character.prompt,
        negative: character.negative,
      );
      blocks.add(block);
      mentions.add(
        OutgoingMention(
          kind: 'character_card',
          name: character.name,
          modelText: block,
          snapshot: _cardSnapshot(character),
        ),
      );
    }
    if (invoked != null) {
      mentions.add(OutgoingMention(kind: 'skill', name: invoked.name));
    }

    final displayText = bodyText.trim();
    if (blocks.isEmpty && invoked == null) {
      return OutgoingMessage.plain(raw);
    }
    final String full;
    if (blocks.isEmpty) {
      // 只点了技能、没写正文：还要给标题 / 审计留一行东西（模型看到的是技能正文）
      full = displayText.isEmpty ? '/${invoked!.name}' : displayText;
    } else if (displayText.isEmpty) {
      full = blocks.join('\n\n');
    } else {
      full = '${blocks.join('\n\n')}\n\n$displayText';
    }
    return OutgoingMessage(
      text: full,
      displayText: displayText,
      mentions: mentions,
    );
  }

  /// 一张卡投给模型的文本：正文逐字，负面单独一段。
  ///
  /// 负面**不拼进正文**：skill 要求 `[STYLE SPEC]` / `[CHARACTER CARD]` 的正文
  /// 当固定规格逐字读，混进去会被当成规格的一部分。单独一段，模型才分得清
  /// "要什么"和"不要什么"；空负面不写这一段（与没有负面时的旧文本逐字一致）。
  String _cardBlock({
    required String marker,
    required String negativeMarker,
    required String name,
    required String? prompt,
    required String? negative,
  }) {
    final buffer = StringBuffer('$marker\n$name\n${prompt ?? ''}');
    final neg = (negative ?? '').trim();
    if (neg.isNotEmpty) {
      buffer.write('\n\n$negativeMarker\n$neg');
    }
    return buffer.toString();
  }

  /// 发送时把卡的可读字段抄一份（之后卡被改 / 删，弹窗里仍是当时那一版）。
  Map<String, dynamic> _cardSnapshot(CharacterCard card) => {
    'id': card.id,
    'name': card.name,
    'prompt': card.prompt,
    'negative': card.negative,
    'updatedAt': card.updatedAt,
  };

  /// 菜单中列出的 skills：只列**可调用**且已启用的（`/` 表示显式调用）。
  List<AiSkill> get _slashMatches {
    final query = _slashQuery;
    if (query == null) return const [];
    final all = widget.store.skills
        .where((s) => s.enabled && s.userInvocable && s.validationError == null)
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
    setState(() {
      _slashQuery = null;
      // 与卡片面板互斥；插入 skill 之后也收起卡片面板（只留输入框）
      _cardsOpen = false;
    });
    widget.focusNode.requestFocus();
  }

  /// 键盘实际占的高度（逻辑像素）。
  ///
  /// ⚠ 不能用 `MediaQuery.viewInsetsOf(context).bottom`：`Scaffold` 的
  /// `resizeToAvoidBottomInset` 会把它消费掉，body 里读出来是 0 —— 真机实测
  /// 弹层仍按"全屏高度"排版，于是 `BOTTOM OVERFLOWED`。`View.viewInsets` 是
  /// 原始物理像素，不会被 Scaffold 改。
  double _keyboardInset(BuildContext context) {
    final view = View.of(context);
    final dpr = view.devicePixelRatio;
    if (dpr <= 0) return 0;
    return view.viewInsets.bottom / dpr;
  }

  /// 输入框最多能长几行：**屏幕 - 键盘 - 已选卡片 - 底行**剩下多少算多少。
  ///
  /// 超过该行数时文本在输入框**内部**滚动，
  /// 整个输入区不会顶到 AppBar 上 —— 键盘高度可变，所以每次 build 都现算。
  int get _inputMaxLines {
    final cards = _characterCards.length + (_styleCard != null ? 1 : 0);
    // 一行大约放两张 chip；留一点余量。
    final chipRows = cards == 0 ? 0 : (cards + 1) ~/ 2;
    // 底行 76 + chip 每行 52 + 输入区上下留白 20 + 余量 20
    final reserved = 76.0 + chipRows * 52.0 + 20.0 + 20.0;
    final available = widget.maxHeight - reserved;
    final lines = (available / 32).floor();
    if (lines < 1) return 1;
    if (lines > 8) return 8;
    return lines;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    final blockers = store.preflight?.blockers ?? const <String>[];
    final slashMatches = _slashMatches;
    // 输入框最多能长几行：**屏幕 - 键盘 - 已选卡片 - 底行** 里剩下的高度。
    // 这样键盘高度怎么变，输入区都不会顶到 AppBar 上；超出这个行数时
    // 文本在**输入框内部**滚动。
    final inputMax = _inputMaxLines;

    return Container(
      // 输入区与消息区之间**不画横线**：换成比消息区高两档的底色，
      // 让"这里开始是输入区"由色阶表达（Material 3 的分层方式）。
      // 比左右两栏再高一档，避免和导航栏 / 会话列表糊成同一个灰块。
      color: theme.colorScheme.surfaceContainer,
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
      child: Column(
        // 输入区按内容取高（上限由页面那层 ConstrainedBox 给）：写成 max 会把
        // 上限当成实际高度，桌面端输入区一下吃掉 960px、消息区只剩一条缝。
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
          if (blockers.isNotEmpty)
            _Banner(
              text:
                  '附件未通过准入，发送会被阻断。\n${blockers.map((b) => '· $b').join('\n')}',
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
                // 键盘弹起时窗口只剩一半：固定 180 会把输入区顶出屏幕
                //（真机 2026-11-08 实测 `BOTTOM OVERFLOWED BY 17 PIXELS`）。
                // 跟着可用高度收一收，最多仍是 180。
                constraints: BoxConstraints(
                  maxHeight:
                      ((MediaQuery.sizeOf(context).height -
                                  _keyboardInset(context)) *
                              0.28)
                          .clamp(110.0, 180.0)
                          .toDouble(),
                ),
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
          // 手机走底部弹层（见 `_openCards`），这里只剩电脑端的内嵌面板。
          if (_cardsOpen) _cardsPanel(theme),
          // 排版：上一行整宽输入框，下一行 [附件] [模型选择] …… [发送]
          Shortcuts(
            // 回车 = 发送只在**电脑端**。手机软键盘的回车为 `TextInputAction.newline`，
            // 但个别输入法会将其作为原始 Enter 键事件上报 —— 在此绑定会直接发送。
            // 手机端不绑定，回车一律走输入框自身的换行行为。
            shortcuts:
                (theme.platform == TargetPlatform.android ||
                    theme.platform == TargetPlatform.iOS)
                ? const <ShortcutActivator, Intent>{}
                : const {
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
                // Ctrl+V 粘贴附件。
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
                  // 行数上限按"屏幕 - 键盘 - 卡片 - 底行"现算（见 build 里的
                  // `inputMax`）；超出时文本在输入框**内部**滚动，不顶 AppBar。
                  minLines: inputMax >= 2 ? 2 : 1,
                  maxLines: inputMax,
                  textInputAction: TextInputAction.newline,
                  // ⚠ 右键「粘贴」必须换成我们自己的那条，否则它只读文本、
                  // 会把"文件路径"当内容贴进来（见 `_composerContextMenu`）
                  contextMenuBuilder: _composerContextMenu,
                  decoration: const InputDecoration(
                    hintText: '描述你的生图需求。/ 可以调用 Skill，@ 可以插入角色卡（可多张）或画风卡。Enter 发送，Shift+Enter 换行。',
                    border: OutlineInputBorder(),
                    isDense: true,
                  ),
                ),
              ),
            ),
          ),
          // 已选卡片放在**输入框下面**：输入区为"底部锚定 + 内部滚动"（reverse），
          // 放在输入框上方的 chip 会被长文本顶出可视区；放在下方则始终贴着
          // 底行，长文本只在输入框内部滚动。
          if (_characterCards.isNotEmpty || _styleCard != null)
            _cardChips(theme),
          const SizedBox(height: 8),
          // 底行排版约束：
          //   · **一行放不下时折行**，不使用横向滚动；
          //   · **操作控件靠左；发送（宽屏另含 token 汇总）靠右**。
          //
          //   ① token 汇总被挤没 + 发送按钮不贴右（2026-09-20）：汇总当时是 loose 的
          //      `Flexible`，用不满的那截空白落在**按钮右边**，按钮就不贴右；它和
          //      `Spacer` 还各占 1 flex，窗口一窄先被省略。→ 汇总固定占位（≤220，
          //      只因为自己太长才省略），发送放最后、不参与压缩。
          //   ② 控件显示不全（2026-09-20）：操作组曾经是"唯一的 `Expanded` + 横向
          //      滚动"，窄窗口下视口只剩 21px（内容 847px），权限档 / 模型选择被从
          //      **左边缘**裁掉。→ 换成折行。
          //   ③ 折行后的排版：将"附件(40) / 权限档(187) / 模型(296 撑满) / 思考(70)"直接折行，
          //      结果为**三行**、每行右侧大片空白、权限档单独位于第一行。
          //      原因是各控件的固有宽度相差一个数量级。→ 窄屏时这四项统一退为
          //      **等宽图标按钮**（各约 48px）：一行放得下则一行，放不下再折行，
          //      每行均为整齐的等宽图标。
          //
          //   ④ 整排靠右：窄屏该排此前为**一个 `Wrap(alignment: end)`**（四项控件 + 发送），
          //      整排一起靠右，"附件"左侧留出大片空白。→ 窄屏同样拆为 `Row`：
          //      左侧 `Expanded`（start 对齐）、右侧发送，见下方分支注释。
          //
          // 宽屏右侧那两件（token 汇总 + 发送）走同一个 `Wrap`（end 对齐 = 整体贴右，
          // 换行后最后一行也贴右）。
          LayoutBuilder(
            builder: (context, constraints) {
              // 三档（按钮过小时难以点击，且不显示关键信息；
              // 特定宽度下控件变为两行排列）：
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
              // token 汇总属于**纯展示项**，宽度不足时优先隐藏，
              // 但不应在中等宽度就消失，因此单独设置门限。
              final showSummary = constraints.maxWidth >= _summaryWidth;
              return Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  // 操作组自己折行（宽裕时只占一行，不会把右边的汇总 / 发送顶出去）
                  Expanded(
                    child: FilterChipRow(
                      key: const Key('ai-composer-actions'),
                      alignment: WrapAlignment.start,
                      // 四枚按钮的间距**只由这一个值决定**（各按钮自己不再带
                      // 左右 padding，见 `_PermissionPicker` 与 "+" 的注释）。
                      spacing: 6,
                      runSpacing: 6,
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
  /// 顺序：附件 → 权限档 → 模型选择 → 思考强度。
  ///
  /// [label] 是**三档文字密度**（见 `build` 里那个 `LayoutBuilder` 的注释）：
  /// 全称 / 短标签 / 纯图标。按钮过小时难以点击，且不显示关键信息，
  /// 因此短标签档**保留文字**（"自动"/"询问"、"默认"/"高"），仅省略长文案；
  /// 纯图标档退回 tooltip，图标按钮本身放大到便于点击。
  List<Widget> _composerActions(_ComposerLabel density) {
    return [
      // 附件 + 卡片合并为一个 "+" 菜单（320~720 宽时底行已有 4 个状态控件 + 发送，
      // 增加第 5 个会导致折行 / 挤压）。
      // 合并后底行仍为 4 个，入口位于最左；键盘路径另保留直接输入 `@`。
      AppMenuButton<String>(
        onSelected: (v) {
          if (v == 'attach') _pickFiles();
          if (v == 'cards') _openCards();
          if (v == 'sheet') _startCardFromSheet();
          if (v == 'workflow') widget.onPickWorkflow?.call();
        },
        options: [
          const MenuOption(
            value: 'attach',
            icon: Icons.attach_file,
            label: '添加附件',
          ),
          // 「选择现有工作流」从顶栏搬到这里（2026-11-05 顶栏精简）。
          // 只有桌面后端才有 ComfyUI 工作流库；手机上给了也没东西可选。
          if (widget.onPickWorkflow != null)
            const MenuOption(
              value: 'workflow',
              icon: Icons.account_tree_outlined,
              label: '选择现有工作流…',
              dividerBefore: true,
            ),
          // 卡片库走桌面后端的表：**手机也能用**（连的就是桌面，`listCharacterCards`
          // 本来就是 HTTP）—— 以前这里按 `isLocal` 藏掉，手机上 `+` 菜单里根本没有
          // 卡片入口，只有打 `@` 才进得去（2026-11-08 真机发现）。
          const MenuOption(
            value: 'cards',
            icon: Icons.alternate_email,
            label: '角色卡 / 画风卡…',
            dividerBefore: true,
          ),
          // 从设定图建卡（2026-11-04）：选图 → 新会话 → AI 逐部位带图来问。
          if (!widget.store.isLocal)
            const MenuOption(
              value: 'sheet',
              icon: Icons.badge_outlined,
              label: '从设定图生成角色卡…',
            ),
        ],
        // 与右边三枚一样是 44×44 的描边方块：图标按钮光秃秃地待在一排
        // 描边按钮最左边时，它自带的 10px 内边距会让"+"离下一枚明显更远
        // （尺寸不统一）。统一外形后，间距只由
        // `FilterChipRow.spacing` 一个值决定。
        button: (context, controller, isOpen) => Tooltip(
          message: '添加附件 / 插入卡片（@）',
          child: InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: () =>
                controller.isOpen ? controller.close() : controller.open(),
            child: Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                border: Border.all(color: Theme.of(context).dividerColor),
                borderRadius: BorderRadius.circular(8),
              ),
              child: const Icon(Icons.add, size: 22),
            ),
          ),
        ),
      ),
      // 权限档：位于附件按钮与模型选择之间
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
    // 按空白分词：输入 "波普 平涂" 时两段都要命中，而不是拿整串去 contains
    //（整串 contains 在词之间隔了空格时永远查不到）。
    final tokens = query.isEmpty
        ? const <String>[]
        : query.split(RegExp(r'\s+')).where((t) => t.isNotEmpty).toList();
    final shown = tokens.isEmpty
        ? cards
        : cards.where((c) {
            final haystack = '${c.name} ${c.prompt ?? ''} ${c.negative ?? ''}'
                .toLowerCase();
            return tokens.every((token) => haystack.contains(token));
          }).toList();
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Material(
        color: theme.colorScheme.surfaceContainerHigh,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(12),
          side: BorderSide(color: theme.colorScheme.outlineVariant),
        ),
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          // 手机上按"可用高度"给（键盘弹起时收、收起时放大；打开面板时我们
          // 已经把键盘收掉了，所以这里通常拿到的是整屏）；电脑端没有虚拟键盘
          // 那套问题，固定给一个舒服的高度。
          height:
              (theme.platform == TargetPlatform.android ||
                  theme.platform == TargetPlatform.iOS)
              ? ((MediaQuery.sizeOf(context).height - _keyboardInset(context)) *
                        0.46)
                    .clamp(220.0, 360.0)
                    .toDouble()
              : 260.0,
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
                  // 多选时把"已经选了几张"说清楚（面板不会自动收起）。
                  if (_characterCards.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Text(
                        '角色卡 ${_characterCards.length} 张',
                        style: theme.textTheme.labelSmall?.copyWith(
                          color: theme.colorScheme.primary,
                        ),
                      ),
                    ),
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
                  style: theme.textTheme.bodySmall,
                  decoration: InputDecoration(
                    isDense: true,
                    // 手机上这个框原来又高又宽，把整个面板吃掉一大截
                    //（搜索框占用空间过大）。
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: 8,
                      vertical: 6,
                    ),
                    prefixIcon: const Icon(Icons.search, size: 16),
                    prefixIconConstraints: const BoxConstraints(
                      minWidth: 30,
                      minHeight: 30,
                    ),
                    hintText: '搜卡片名 / 正文',
                    hintStyle: theme.textTheme.bodySmall,
                    border: const OutlineInputBorder(),
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
      minimumSize: const Size(0, 44),
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
              ? '还没有${_cardsStyled ? '画风卡' : '角色卡'}，去「预设库 → 卡片库」新建一张。'
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
        // 角色卡是多选：只要在选中列表里就算选中（不按对象身份，按 id —— 同一次
        // 打开面板里列表可能重建过）。
        final selected = _cardsStyled
            ? identical(card, _styleCard)
            : _characterCards.any((c) => c.id == card.id);
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

  /// 已选中的卡：输入框上方一排 chip，点 × 取消（角色卡一张一个 chip）。
  Widget _cardChips(ThemeData theme) {
    return Padding(
      // 与输入框间距收紧：上面 4、下面 2，
      // 行高总预算在 `_inputMaxLines` 里按 52/行算过，收一点不会引出溢出。
      padding: const EdgeInsets.only(top: 4, bottom: 2),
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
              // 点 chip 看这张卡的正文（只读）—— 选中的卡是"发出去的样子"，
              // 改它要去卡片库，所以这里只给看。
              onPressed: () => _showCardDetail(
                title: '画风卡 · ${_styleCard!.name}',
                prompt: _styleCard!.prompt ?? '',
              ),
              onDeleted: () => setState(() => _styleCard = null),
            ),
          for (final card in _characterCards)
            InputChip(
              key: ValueKey('character-card-chip-${card.id}'),
              avatar: Icon(
                Icons.badge_outlined,
                size: 16,
                color: theme.colorScheme.primary,
              ),
              label: Text(
                '角色卡 · ${card.name}',
                style: theme.textTheme.labelSmall,
              ),
              visualDensity: VisualDensity.compact,
              onPressed: () => _showCardDetail(
                title: '角色卡 · ${card.name}',
                prompt: card.prompt ?? '',
                negative: card.negative,
              ),
              onDeleted: () => setState(
                () => _characterCards.removeWhere((c) => c.id == card.id),
              ),
            ),
        ],
      ),
    );
  }

  /// 点已选卡片 → **只读**详情弹窗（正面 / 负面提示词原样，可选中复制）。
  void _showCardDetail({
    required String title,
    required String prompt,
    String? negative,
  }) {
    showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: SizedBox(
          width: 560,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('正面提示词', style: Theme.of(ctx).textTheme.labelMedium),
                const SizedBox(height: 4),
                SelectableText(prompt),
                if ((negative ?? '').trim().isNotEmpty) ...[
                  const SizedBox(height: 12),
                  Text('负面提示词', style: Theme.of(ctx).textTheme.labelMedium),
                  const SizedBox(height: 4),
                  SelectableText(negative!),
                ],
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('关闭'),
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
    // 选中的卡 / `/技能` 拼在正文前面（槽位标记逐字固定，卡正文不改）。
    final message = _composeOutgoingMessage(widget.controller.text);
    final sendable = widget.store.sendBlockReason() == null;
    widget.onSubmit(message);
    // 只有"真的会被发出去"才清掉卡片：被拒时（正在生成 / 附件还在上传）卡片要留着，
    // 不能因为按了一次回车就没了。
    if (sendable && (_characterCards.isNotEmpty || _styleCard != null)) {
      setState(() {
        _characterCards.clear();
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

  /// 「从设定图生成角色卡」：选一张设定图 → 开一条新会话 → 附上它并直接发问。
  ///
  /// 流程：不直接生成角色卡，而是**自上而下逐项确认不确定的部位**
  /// （题目带裁图、选项带候选 / 示例图）。此处只负责送入图片并发起，
  /// 具体提问方式由系统提示第 26 条约束。
  ///
  /// 只在桌面后端上出现入口：手机本地 harness 没有卡片库，也没有后端的裁图能力。
  Future<void> _startCardFromSheet() async {
    if (widget.store.isLocal) return;
    final files = await FilePicker.pickFiles(
      type: FileType.image,
      dialogTitle: '选择角色设定图',
    );
    if (files.isEmpty) return;
    final file = files.first;
    final path = file.path;
    if (path == null || path.isEmpty) {
      widget.store.noteAttachmentsUnreadable([file.name]);
      return;
    }
    // 设定图不该接在上一段聊天后面：当前会话非空就新开一条
    if (widget.store.messages.isNotEmpty) {
      await widget.store.newConversation();
    }
    await widget.store.attachFiles([(name: file.name, path: path)]);
    if (!mounted) return;
    // 上传失败时 store 已经写过 error / notice；这里不再发一条"没有图"的消息
    if (widget.store.attachments.isEmpty) return;
    await widget.onSubmit(OutgoingMessage.plain(_characterSheetPrompt));
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
  /// 因此"复制文件后右键粘贴"得到的是文件路径。
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

  /// Ctrl+V 粘贴附件（含"截图 / 复制图片"两种来源）。
  ///
  /// # 为什么这里必须异步 + 自己兜底
  ///
  /// "剪贴板里有没有文件 / 图"要问原生（`FileClipboard`），而按键回调**是同步的**
  /// —— 没法 `await` 了再决定吃不吃这个事件。所以这里的做法是：
  /// **先把事件吃掉**，然后异步判；都不是就自己把文本粘进输入框。
  ///
  /// ⚠ 兜底步骤不可省略：不归还事件时，按 Ctrl+V 会**无法粘贴任何内容**
  /// （事件已被拦截，`DefaultTextEditingShortcuts` 无法处理）。
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
    // Ctrl+V 粘贴附件：截图此前无法粘贴，
    // Flutter 的 `Clipboard` 只读文本，而截图位于 `CF_DIB` / `CF_BITMAP` 中。
    final image = await FileClipboard.readImage();
    if (!mounted) return;
    if (image != null && await _attachClipboardImage(image)) return;
    if (!mounted) return;
    // 没有文件 → 先看是不是**原生那一半未接通**："通道没编进产物"与"剪贴板里
    // 确实没有文件"表现完全一样，静默降级的话这一档永远查不出来（只说一次）。
    if (FileClipboard.channelAlive == false && !_clipboardNoticeShown) {
      _clipboardNoticeShown = true;
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        const SnackBar(content: Text('读剪贴板（文件 / 图片）的原生通道没接上，Ctrl+V 现在只能粘文本')),
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
/// 补充末尾项的用途：光标落在只读位置等场景下，内置项里**没有**「粘贴」，
/// 不补充时该档位没有任何粘贴入口。
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
/// 按钮过小时难以点击，且不显示关键信息；特定宽度下控件变为两行排列。
///
/// 旧版只有两档（全称 / 纯图标），中间宽度没有对应档位 —— 文字被压缩，
/// 或退化为四个无信息的图标。现在增加中间的**短标签**档：
/// 关键信息（权限档、思考强度、模型）**在任何宽度都可见**。
enum _ComposerLabel {
  /// 全称：`自动允许（无需批准）`、模型名 + 能力徽标、思考强度全名。
  full,

  /// 短标签：`自动` / `询问`、`默认` / `高` + 模型名。**关键信息仍在**。
  short,

  /// 纯图标：文字进 tooltip（信息一个都不丢，只是要点开/悬停才看见）。
  icon,
}

// ---------------------------------------------------------------------------
//  权限档
// ---------------------------------------------------------------------------

/// 权限两档：**询问**（默认）/ **自动允许（无需批准）**。
///
/// 位置：输入区底部、附件按钮与模型选择之间。
/// 真源在后端（`ai.tools.policy.permissionMode`），切档写回后端 ——
/// 后端每次 Run 现读策略，于是"切到自动允许"会**即时**把那段规则注入系统提示，
/// 并且 AI 调工具不再走审批闸门。路径白名单不受影响，越界写照样被拒。
///
/// 「完全权限」这一名称不准确：它只免除审批，
/// 文件夹白名单没有任何放宽 —— 因此改为「自动允许（无需批准）」。
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
    // 退回纯图标后点击精度不足，也无法判断当前档位。
    final shortText = full ? '自动' : AiToolPolicy.modeAskLabel;
    // ⚠ 不要在这里再包一层 `Padding(horizontal: 4)`：那会让"附件 → 权限"
    // 的间距变成 4（行间距）+4（这里）+4（权限自己）＝ 12，而"模型 → 思考"
    // 只有 4。整排间距只由 `FilterChipRow.spacing` 决定
    // （此前各控件尺寸不统一）。
    return AppMenuButton<String>(
      tooltip: full
          ? '当前是$text，AI 调用工具不会等你批准。可读 / 可写目录范围不变，越界仍会被拒。点一下改回「${AiToolPolicy.modeAskLabel}」。'
          : '当前是$text，写盘 / 提交任务这类改动操作会先弹批准。点一下切到「${AiToolPolicy.modeFullLabel}」。',
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
              : () =>
                    controller.isOpen ? controller.close() : controller.open(),
          child: Container(
            // 三档均保证 ≥44 高
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
    );
  }
}

/// 思考强度选择器（AIH-056）。
///
/// 只显示**当前模型声明过**的档位；两个例外总是可选或按声明出现：
/// · 「默认」—— 不发任何思考参数、任何网关都成立，所以永远可选
///   （默认不指定思考强度，且无需修改模型声明）；
/// · 「关闭」—— **实际发送关闭参数**，
///   只在模型目录里声明了关闭取值（`off` / `disabled`）时出现。
/// **「默认」不承诺"不会思考"**（2026-09-20 改名的原因，见 [AiReasoningEffort.off]：
/// 本机在用的聚合网关关不掉思考，叫「关闭」就是撒谎）—— 所以两档必须分开列。
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
        ? '思考强度（当前是 ${store.reasoningEffort.label}）。「默认」不发思考参数，由网关决定。'
              '「关闭」会真的发关闭参数，模型不思考。'
              '模型声明的档位有 ${options.where((e) => e.isThinking).map((e) => e.label).join(' / ')}'
        : (model == null ? '还没选模型' : '$text。该模型未声明推理能力，请到「设置 → AI 模型」声明。');

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
            // 该行用于说明"选择关闭即不思考"这一行为。
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
            // 三档均保证 ≥44 高
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
                // "当前思考强度"属于关键信息）
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

    return Tooltip(
      message: blocked
          ? '这张附件发不出去。\n${blockers.map((b) => '· $b').join('\n')}'
          : '${attachment.name} · ${attachment.sizeLabel}${attachment.isVideo ? '（已取预览帧）' : ''}'
                '\n点击在应用内预览（弹窗）',
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
                          : () => showAttachmentPreview(
                              context,
                              url: fileUrl!,
                              name: attachment.name,
                              modality: attachment.modality,
                              headers: attachmentPreviewHeaders(),
                            ),
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
    // **附件不打码**：附件是主动选择上传的文件，
    // 库里没有、也不该有"这条是不是 NSFW"的标记 —— 按"没标记就不糊"的判据，
    // 这里不添加打码层（此前无条件打码，上传的参考图会整体打码）。
    return Image.network(
      thumbUrl!,
      width: _size,
      height: _size,
      fit: BoxFit.cover,
      // 附件缩略图同样是后端给的地址（地址里不带令牌）：手机走局域网时必须带头，
      // 否则自己的参考图也是破图（与下面那条 `attachmentPreviewHeaders()` 一致）。
      headers: attachmentPreviewHeaders(),
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

/// 消息气泡里的附件：只读的小缩略图（发送后仍可回看）。
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
    // 历史消息里的附件块**只有 attachmentId**（没有原始文件名 / modality），
    // 所以 `name` 经常就是一串 UUID。别把它当扩展名判据（那会把图片判成"其它"，
    // 会被转到浏览器打开）。
    // 有缩略图 = 后端认得出这是图片，直接按图片预览。
    final isImage = thumbUrl != null;
    final label = attachmentDisplayName(name, hasPreview: isImage);
    return Tooltip(
      message: '$label\n点击在应用内预览（弹窗）',
      child: InkWell(
        onTap: fileUrl == null
            ? null
            : () => showAttachmentPreview(
                context,
                url: fileUrl!,
                name: name,
                // 缩略图存在就是图片：别让"没有扩展名"把它判成未知类型
                modality: isImage ? 'image' : null,
                headers: attachmentPreviewHeaders(),
              ),
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
                  headers: attachmentPreviewHeaders(),
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

/// 右侧栏顶部的**实时进度**。
///
/// 显示两件事：
///  - ComfyUI 队列（运行中 / 等待中 + 正在跑的那个任务名）；
///  - 最近提交的任务（状态、耗时、产物数）。
///
/// 没有活动时**整块不占位** —— 右侧栏本来就窄，闲着的时候不该被一张空卡片吃掉高度。
class _ComfyProgress extends StatelessWidget {
  final AiWorkspaceStore store;
  const _ComfyProgress({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final jobs = store.comfyJobs;
    final jobsError = store.comfyJobsError;
    final active = jobs.submissions.where((s) => s.isActive).toList();
    final recent = jobs.submissions.where((s) => !s.isActive).take(2).toList();
    // 队列里没人、也没有最近提交过（或最近只是"不可达"）：不显示这一块。
    // ⚠ 拉取失败时**照样占一块**：否则"拉不到进度"看起来就是"没有任务"。
    if (!jobs.hasActivity &&
        recent.isEmpty &&
        jobs.comfyReachable &&
        jobsError == null) {
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
            if (jobsError != null)
              Text(
                '拉不到实时进度，本地服务没有回话。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              )
            else if (!jobs.comfyReachable)
              Text(
                'ComfyUI 连不上，进度拿不到（它没在跑？）',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              )
            else if (jobs.queueRunning > 0)
              Text(
                '正在生成「${jobs.runningLabel ?? '未知工作流'}」'
                '${jobs.queuePending > 0 ? '（还有 ${jobs.queuePending} 个排队）' : ''}',
                style: theme.textTheme.bodySmall,
              ),
            for (final s in active)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: _SubmissionRow(submission: s, highlight: true),
              ),
            if (active.isEmpty &&
                jobs.queueRunning == 0 &&
                jobs.comfyReachable &&
                jobsError == null)
              Text(
                '当前没有正在生成的任务',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            if (recent.isNotEmpty) ...[
              // 同一张卡里分"正在跑"与"最近提交"两组：用留白分开，
              // 不画横线（卡片本身已经是独立的一块）。
              const SizedBox(height: 10),
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
/// 与 `cardTheme` 对齐的"填充卡"：实心 surfaceContainerHigh + 24 圆角，无描边。
BoxDecoration panelCardDecoration(ThemeData theme) => BoxDecoration(
  color: theme.colorScheme.surfaceContainerHigh,
  borderRadius: BorderRadius.circular(24),
);

/// 右侧栏里的一块：统一的卡片容器 + 一行标题（可选右侧动作）。
///
/// 右侧栏除实时进度外的内容也使用相同容器分组。
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
        // 名字独占一行（最多两行）。右侧栏只有三百来像素宽，
        // 名字与"状态 · 耗时 · 产物数"挤在同一行时，长名字只剩半截（`test · …`）。
        Text(
          submission.label,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.labelSmall?.copyWith(color: color),
        ),
        const SizedBox(height: 1),
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
            // 步骤名本身不短（"正在执行 自定义采样器（高级）（7/8）"），
            // 一行装不下就折到第二行，别把括号里的步数和百分比一起截掉。
            maxLines: 2,
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
              '待审批列表取不到，$error',
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
                // 卡片说明：手机点过提交**不算**批准
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

/// 手机上的卡片选择器（**底部弹层**）。
///
/// 放回输入区会与键盘争用高度，只能二选一。弹层是独立路由：自有高度（约 2/3 屏）、
/// 独立滚动，键盘弹起时整体上移，列表始终可见、可点击。
class _CardPickerSheet extends StatefulWidget {
  final List<CharacterCard> characters;
  final List<StyleCard> styles;
  final Set<int> selectedCharacterIds;
  final int? selectedStyleId;
  final bool initialStyleTab;
  final bool loading;
  final String? error;
  final ValueChanged<CharacterCard> onPickCharacter;
  final ValueChanged<StyleCard> onPickStyle;

  const _CardPickerSheet({
    required this.characters,
    required this.styles,
    required this.selectedCharacterIds,
    required this.selectedStyleId,
    required this.initialStyleTab,
    required this.loading,
    required this.error,
    required this.onPickCharacter,
    required this.onPickStyle,
  });

  @override
  State<_CardPickerSheet> createState() => _CardPickerSheetState();
}

class _CardPickerSheetState extends State<_CardPickerSheet> {
  late bool _styled = widget.initialStyleTab;
  final TextEditingController _search = TextEditingController();
  String _query = '';

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  /// 按空白分词；每段都要命中（与桌面 `@` 面板的过滤同一套规则）。
  ///
  /// 整串 `contains` 在词之间隔了空格时永远查不到（"波普 平涂" 就是一个例子）。
  List<T> _filter<T extends CharacterCard>(List<T> cards) {
    final tokens = _query
        .trim()
        .toLowerCase()
        .split(RegExp(r'\s+'))
        .where((t) => t.isNotEmpty)
        .toList();
    if (tokens.isEmpty) return cards;
    return cards.where((c) {
      final haystack = '${c.name} ${c.prompt ?? ''} ${c.negative ?? ''}'
          .toLowerCase();
      return tokens.every((token) => haystack.contains(token));
    }).toList();
  }

  List<CharacterCard> get _characters => _filter(widget.characters);

  List<StyleCard> get _styles => _filter(widget.styles);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final count = widget.selectedCharacterIds.length;
    final shown = _styled ? _styles.length : _characters.length;
    return Material(
      color: theme.colorScheme.surfaceContainerHigh,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 4, 0),
            child: Row(
              children: [
                _tab(
                  theme,
                  '角色卡',
                  !_styled,
                  () => setState(() => _styled = false),
                ),
                _tab(
                  theme,
                  '画风卡',
                  _styled,
                  () => setState(() => _styled = true),
                ),
                const Spacer(),
                if (count > 0)
                  Text(
                    '已选 $count 张',
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.primary,
                    ),
                  ),
                IconButton(
                  tooltip: '收起卡片面板',
                  onPressed: () => Navigator.of(context).maybePop(),
                  icon: const Icon(Icons.close, size: 18),
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
            child: TextField(
              controller: _search,
              style: theme.textTheme.bodySmall,
              decoration: InputDecoration(
                isDense: true,
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 8,
                  vertical: 8,
                ),
                prefixIcon: const Icon(Icons.search, size: 16),
                prefixIconConstraints: const BoxConstraints(
                  minWidth: 30,
                  minHeight: 30,
                ),
                hintText: '搜卡片名 / 正文',
                hintStyle: theme.textTheme.bodySmall,
                border: const OutlineInputBorder(),
              ),
              onChanged: (v) => setState(() => _query = v),
            ),
          ),
          Expanded(child: _body(theme, shown)),
        ],
      ),
    );
  }

  Widget _tab(ThemeData theme, String label, bool active, VoidCallback onTap) {
    return TextButton(
      onPressed: onTap,
      style: TextButton.styleFrom(
        visualDensity: VisualDensity.compact,
        foregroundColor: active
            ? theme.colorScheme.primary
            : theme.colorScheme.onSurfaceVariant,
      ),
      child: Text(label),
    );
  }

  Widget _body(ThemeData theme, int shown) {
    if (widget.loading) {
      return const Center(child: CircularProgressIndicator());
    }
    if (widget.error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Text(
            widget.error!,
            textAlign: TextAlign.center,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.error,
            ),
          ),
        ),
      );
    }
    if (shown == 0) {
      return Center(
        child: Text(
          _query.trim().isEmpty ? '卡片库还是空的' : '没有匹配的卡片',
          style: theme.textTheme.bodySmall,
        ),
      );
    }
    if (_styled) {
      final items = _styles;
      return ListView.builder(
        padding: const EdgeInsets.fromLTRB(4, 0, 4, 16),
        itemCount: items.length,
        itemBuilder: (context, i) {
          final card = items[i];
          return _tile(
            theme,
            name: card.name,
            subtitle: card.prompt ?? '',
            selected: widget.selectedStyleId == card.id,
            icon: Icons.palette_outlined,
            onTap: () => widget.onPickStyle(card),
          );
        },
      );
    }
    final items = _characters;
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(4, 0, 4, 16),
      itemCount: items.length,
      itemBuilder: (context, i) {
        final card = items[i];
        return _tile(
          theme,
          name: card.name,
          subtitle: card.prompt ?? '',
          selected: widget.selectedCharacterIds.contains(card.id),
          icon: Icons.badge_outlined,
          onTap: () => widget.onPickCharacter(card),
        );
      },
    );
  }

  Widget _tile(
    ThemeData theme, {
    required String name,
    required String subtitle,
    required bool selected,
    required IconData icon,
    required VoidCallback onTap,
  }) {
    return ListTile(
      leading: Icon(icon, size: 20),
      title: Text(name),
      subtitle: Text(
        subtitle,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: theme.textTheme.bodySmall,
      ),
      trailing: selected
          ? Icon(Icons.check, color: theme.colorScheme.primary)
          : null,
      onTap: onTap,
    );
  }
}

/// 「待审批」横幅的**轮询壳**：自己每 5 秒拉一次 `GET /api/ai/pending`。
///
/// 不依赖右侧栏的 watcher：窄屏 / 上下文面板收起时它不会启动，
/// 因此看不到"有待审批项"（该情况下会错过 300 秒等待，提交超时被拒）。
///
/// 只读 store 的公开字段（`pendingOverview` / `refreshPending`），
/// **不改 store**（另一个子代理正在改它的会话列表）。
class _PendingApprovalBanner extends StatefulWidget {
  final AiWorkspaceStore store;

  const _PendingApprovalBanner({required this.store});

  @override
  State<_PendingApprovalBanner> createState() => _PendingApprovalBannerState();
}

class _PendingApprovalBannerState extends State<_PendingApprovalBanner> {
  /// 这一帧是不是已经把轮询挂上了（`initState` 里要等后端，不是同步完成）。
  bool _watching = false;

  @override
  void initState() {
    super.initState();
    unawaited(_start());
  }

  /// 与右侧栏**同一条冷启动纪律**（`_ContextPanel.initState`）：先等后端有个结论，
  /// 再去问待批清单。冷启动那一刻 `baseUrl` 还是 8080 兜底值，这一问必然失败，
  /// 而失败会**留在界面上**（`pendingError` 只有手点刷新才走）—— 横幅比那张卡还显眼。
  Future<void> _start() async {
    await widget.store.waitForBackend();
    if (!mounted) return;
    _watching = true;
    // 轮询交给 store（与右侧栏那张卡**共用一份** 5 秒定时器）：
    // 两边各起一个就是每 5 秒问后端两遍（2026-11-09 抓到的重复拉取）。
    widget.store.startWatchingPending();
  }

  @override
  void dispose() {
    if (_watching) widget.store.stopWatchingPending();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final store = context.watch<AiWorkspaceStore>();
    return AiPendingBanner(
      approvals: store.pendingOverview.approvals,
      onOpen: (a) => unawaited(store.openConversation(a.conversationId)),
    );
  }
}
