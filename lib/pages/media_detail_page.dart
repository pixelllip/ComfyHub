import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/file_clipboard.dart';
import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../state/nsfw_store.dart';
import '../widgets/audio_player_view.dart';
import '../widgets/common.dart';
import '../widgets/prompt_params.dart';
import '../widgets/upload_sheet.dart';
import '../widgets/video_view.dart';
import '../widgets/workflow_viewer.dart';
import '../widgets/zoomable_image_view.dart';
import 'prompt_detail_page.dart';
import 'prompts_page.dart' show confirmDialog;
import 'tag_results_page.dart';

/// 产物详情：大图预览 + **关联的提示词** + 标签 + 元信息。
///
/// 这是整个 App 的核心闭环：点开一张生成图，就能看到当初用的提示词。
class MediaDetailPage extends StatefulWidget {
  final int mediaId;

  const MediaDetailPage({super.key, required this.mediaId});

  @override
  State<MediaDetailPage> createState() => _MediaDetailPageState();
}

class _MediaDetailPageState extends State<MediaDetailPage> {
  /// **当前正在看的那一条**（可以和 `widget.mediaId` 不同 —— 见 [`_goTo`]）。
  ///
  /// 用户 2026-10-11："重新路由的实际行为是关闭旧窗口、打开新窗口。太奇怪了。
  /// 能不能改成更加舒服的过渡方式：**页面不变，但是填充内容实时改变**？"
  ///
  /// 所以左右切换**不再走 `pushReplacement`**：路由始终是同一个，
  /// 只把 `_currentId` 换掉、重新取数据，页面骨架（AppBar / 布局 / 滚动位置）原地不动。
  late int _currentId = widget.mediaId;

  MediaAsset? _media;
  Prompt? _prompt;
  bool _loading = true;
  String? _error;

  /// 当前这一条所属**集合**的成员 id（按时间升序）。
  ///
  /// 用户 2026-10-11："产物对应的工作流参数大体相同、只有一点点微调，完全可以放进同一个页面…
  /// 点进详情里可以左右切换。" 判定在后端（`media/collection.rs`），这里只按 `collectionId` 列成员。
  ///
  /// ⚠ 只有 `collectionId != null` 时才拉；**空列表 / 单成员 = 不显示箭头**（用户定的
  /// "只有一份产物时禁用箭头"）。
  List<int> _siblings = const [];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void didUpdateWidget(covariant MediaDetailPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    // 外部（比如画廊重新 push 了同一个路由）换了 id：跟着换，别停在旧内容上
    if (oldWidget.mediaId != widget.mediaId && widget.mediaId != _currentId) {
      _currentId = widget.mediaId;
      WidgetsBinding.instance.addPostFrameCallback((_) => _load());
    }
  }

  Future<void> _load({bool showSpinner = true}) async {
    if (!mounted) return;
    final store = context.read<LibraryStore>();
    // 打开详情 = 用户主动要看这一条（用户 2026-09-20：「打开详情之后就没必要模糊了」）：
    // 详情页自己不打码，顺手把"这一条看过了"记下来 —— 回到列表时它的缩略图也不再糊。
    // 只记在内存里（`NsfwStore`），关掉 App 再打开照旧打码。
    //
    // ⚠ 每次**切换**也要对新的那一条 reveal（`_goTo` 会再调一次 `_load`）——
    // 漏掉的表现是"看完第 1 张，第 2 张又糊上了"。
    context.read<NsfwStore?>()?.reveal('media:$_currentId');
    setState(() {
      if (showSpinner) _loading = true;
      _error = null;
    });
    try {
      final media = await store.api.getMedia(_currentId);
      Prompt? prompt;
      if (media.promptId != null) {
        try {
          prompt = await store.api.getPrompt(media.promptId!);
        } catch (_) {
          prompt = null;
        }
      }
      // 集合成员：拿到失败**不影响详情页**（没有箭头而已，不是错误）
      var siblings = const <int>[];
      if (media.collectionId != null) {
        try {
          final page = await store.api.listMedia(
            collectionId: media.collectionId,
            sort: 'oldest',
            size: 200,
          );
          siblings = page.items.map((e) => e.id).toList();
        } catch (_) {
          siblings = const [];
        }
      }
      if (!mounted) return;
      setState(() {
        _media = media;
        _prompt = prompt;
        _siblings = siblings;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
    }
  }

  /// 当前这条在集合里的下标（不在集合里 = -1）。
  int get _indexInSiblings => _siblings.indexOf(_currentId);

  /// 上一条 / 下一条的 id（没有就返回 null → 按钮禁用、快捷键不响应）。
  int? get _prevId {
    final i = _indexInSiblings;
    return (i > 0) ? _siblings[i - 1] : null;
  }

  int? get _nextId {
    final i = _indexInSiblings;
    return (i >= 0 && i + 1 < _siblings.length) ? _siblings[i + 1] : null;
  }

  /// 切到集合里的另一条：**原地换内容，路由不动**（用户 2026-10-11）。
  ///
  /// 为什么不 `pushReplacement`：那在 Flutter 里等于"**旧页销毁 + 新页进场**"——
  /// 用户看到的是"窗口关了一下又开"，而且页面骨架（AppBar / 布局 / 滚动位置）全重建。
  /// 用户原话："太奇怪了。能不能改成更加舒服的过渡方式：页面不变，但是填充内容实时改变？"
  ///
  /// 所以这里只改 `_currentId` 再取一次数据：
  ///  * **路由始终是同一个**（后退一次就回画廊，不会翻十张要按十次）；
  ///  * 页面骨架原地不动，只有内容区换；
  ///  * `showSpinner: false` —— 切换时不闪加载圈，旧图一直显示到新图数据到位
  ///    （`getMedia` 是本机后端，通常几十毫秒）。
  ///
  /// ⚠ **不许清 `_prompt`**：清掉会让"关联的提示词"那张卡**空一下**再填上，
  /// 看起来就是整块在闪（用户 2026-10-11 报的"图片切换时整个界面闪烁"的一半）。
  /// 留着旧值也不会串 —— `_media` 与 `_prompt` 是**同一次 setState** 一起换的，
  /// 所以"图换了词还是旧的"这个中间态根本不存在。
  void _goTo(int id) {
    if (id == _currentId) return;
    setState(() => _currentId = id);
    _load(showSpinner: false);
  }

  /// 「复制文件本身」：把磁盘上的原件放进系统剪贴板（资源管理器里能直接粘出文件）。
  ///
  /// 三条纪律：
  /// 1. 磁盘路径**由后端给**（`GET /api/media/{id}/path`）—— 前端不拼路径（AGENTS §7）；
  /// 2. 路径拿不到（文件不在盘上 / 产物不存在）就**如实说**，不假装成功；
  /// 3. 写剪贴板失败也**如实说**，并给出退路（「复制文件地址」）。
  Future<void> _copyFileToClipboard(
    LibraryStore store,
    int mediaId,
    ScaffoldMessengerState messenger,
  ) async {
    final String path;
    try {
      path = await store.api.mediaPath(mediaId);
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('拿不到磁盘路径：$e')));
      return;
    }
    if (path.isEmpty) {
      messenger.showSnackBar(const SnackBar(content: Text('这条产物的文件不在盘上')));
      return;
    }
    final ok = await FileClipboard.writeFiles([path]);
    messenger.showSnackBar(SnackBar(
      content: Text(ok
          ? '已复制文件，可以在资源管理器里 Ctrl+V 粘贴'
          : '复制文件失败，可以改用「复制文件地址」'),
    ));
  }

  /// 「存为预设」：把这条产物当初用的提示词**复制**成一条新预设。
  ///
  /// 为什么是复制、不是就地改：自动捕获的提示词行是**运行记录**
  /// （`capture_runs.prompt_id` 指向它），改它等于篡改台账（R10 那条链）。
  /// "我想以后反复用这套词"是另一件事 —— 所以另起一条 `Manual` 来源的预设。
  Future<void> _saveAsPreset(BuildContext context, Prompt source) async {
    final store = context.read<LibraryStore>();
    final messenger = ScaffoldMessenger.of(context);
    final name = await _promptText(
      title: '存为预设',
      label: '预设名',
      initial: source.title.isEmpty ? '未命名预设' : '${source.title}（预设）',
    );
    if (name == null || !mounted) return;
    try {
      await store.api.createPrompt(source.copyWith(id: 0, title: name));
      await store.refreshAll();
      messenger.showSnackBar(SnackBar(content: Text('已存为预设「$name」')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('存为预设失败: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = context.read<LibraryStore>();

    if (_loading && _media == null) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    if (_error != null || _media == null) {
      return Scaffold(
        appBar: AppBar(),
        body: ErrorState(message: _error ?? '产物不存在', onRetry: _load),
      );
    }

    final m = _media!;
    final fileUrl = store.api.absolute(m.fileUrl);

    // ContextMenuScope：右键菜单的宿主（不铺 ModalBarrier，菜单开着也能滚，用户 bug ②）
    // Builder 让下面的 context 落在 Scope **之内**，showAppContextMenu 才找得到宿主。
    return ContextMenuScope(
      child: Builder(
        builder: (context) => Scaffold(
      appBar: AppBar(
        title: Text(
          m.title.isEmpty ? m.originalName : m.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        actions: [
          IconButton(
            tooltip: m.favorite ? '取消收藏' : '收藏',
            icon: Icon(m.favorite ? Icons.star : Icons.star_border, color: m.favorite ? Colors.amber : null),
            onPressed: () async {
              await store.api.updateMedia(m.id, favorite: !m.favorite);
              await _load();
              if (mounted) store.refreshAll();
            },
          ),
          AppMenuButton<String>(
            tooltip: '更多',
            onSelected: (v) async {
              final messenger = ScaffoldMessenger.of(context);
              final navigator = Navigator.of(context);
              switch (v) {
                case 'rename':
                  final name = await _promptText(
                    title: '重命名',
                    label: '标题',
                    initial: m.title.isEmpty ? m.originalName : m.title,
                  );
                  if (name != null) {
                    await store.api.updateMedia(m.id, title: name);
                    await _load();
                  }
                  break;
                case 'notes':
                  final notes = await _promptText(
                    title: '编辑备注',
                    label: '备注',
                    initial: m.notes ?? '',
                    multiline: true,
                  );
                  if (notes != null) {
                    await store.api.updateMedia(m.id, notes: notes);
                    await _load();
                  }
                  break;
                case 'open':
                  final uri = Uri.parse(fileUrl);
                  if (await canLaunchUrl(uri)) {
                    await launchUrl(uri, mode: LaunchMode.externalApplication);
                  } else {
                    messenger.showSnackBar(const SnackBar(content: Text('无法用系统播放器打开')));
                  }
                  break;
                case 'copyUrl':
                  await copyToClipboard(context, fileUrl, label: '文件地址');
                  break;
                case 'detach':
                  // 移出之后要**重新拉一遍**：`_siblings` 少一个，左右箭头跟着收起
                  await store.api.updateMedia(m.id, clearCollection: true);
                  await _load();
                  messenger.showSnackBar(const SnackBar(content: Text('已移出集合')));
                  break;
                case 'copyFile':
                  await _copyFileToClipboard(store, m.id, messenger);
                  break;
                case 'nsfw':
                  // 手动标记 / 取消标记（用户 bug 2026-09-20）：AI 没声明过的内容
                  // （自己导入的、或老数据）由用户自己说了算 —— 打码只作用于标记过的条目。
                  await store.api.updateMedia(m.id, nsfw: !m.nsfw);
                  await _load();
                  if (mounted) store.refreshAll();
                  break;
                case 'delete':
                  final ok = await confirmDialog(
                    context,
                    title: '删除产物',
                    message: '确定删除「${m.originalName}」吗？磁盘上的文件也会被删除。',
                  );
                  if (ok) {
                    await store.deleteMedia(m.id);
                    navigator.pop();
                  }
                  break;
              }
            },
            options: [
              const MenuOption(value: 'rename', icon: Icons.drive_file_rename_outline, label: '重命名'),
              const MenuOption(value: 'notes', icon: Icons.notes, label: '编辑备注'),
              MenuOption(
                value: 'nsfw',
                icon: m.nsfw ? Icons.visibility : Icons.visibility_off_outlined,
                label: m.nsfw ? '取消 NSFW 标记' : '标记为 NSFW',
              ),
              const MenuOption(value: 'open', icon: Icons.open_in_new, label: '用系统播放器打开'),
              // 归类纠错（2026-10-18）：只有真在某个集合里才给这一项。
              // 后端会同时记下"用户已移出"，所以拆出来之后回填不会再吸回去。
              if (m.isCollection)
                MenuOption(
                  value: 'detach',
                  icon: Icons.call_split,
                  label: '移出集合（共 ${m.collectionSize} 张里的这一张）',
                  dividerBefore: true,
                ),
              const MenuOption(
                value: 'copyUrl',
                icon: Icons.link,
                label: '复制文件地址',
                dividerBefore: true,
              ),
              // 「复制文件本身」（2026-10-18）：写 CF_HDROP，资源管理器里能直接粘出文件。
              // 只有 Windows 有这条路（见 `lib/core/file_clipboard.dart`）。
              if (FileClipboard.supported)
                const MenuOption(
                  value: 'copyFile',
                  icon: Icons.file_copy_outlined,
                  label: '复制文件（可在资源管理器粘贴）',
                ),
              const MenuOption(
                value: 'delete',
                icon: Icons.delete_outline,
                label: '删除',
                danger: true,
                dividerBefore: true,
              ),
            ],
            button: (context, controller, isOpen) => IconButton(
              tooltip: '更多',
              onPressed: () => controller.isOpen ? controller.close() : controller.open(),
              icon: const Icon(Icons.more_vert),
            ),
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _load,
        // 左右方向键切换同批产物（用户 2026-10-11 报的"方向键在切按钮选中状态"）。
        //
        // 为什么要 `CallbackShortcuts` + `Focus`（而不是靠按钮自带的焦点遍历）：
        // 方向键默认是 Flutter 的**焦点遍历**快捷键 —— 一按就在页面上的按钮之间跳，
        // 用户按左键看到的是"某个按钮被选中了"，而不是"上一张图"。
        // 这里用一个**自动获得焦点**的 `Focus` 把 ←/→ 接管掉：
        //   · 只在有上一条/下一条时响应（边界上按了什么都不做，不会误触发别的）；
        //   · 其他键（Tab / Enter / 空格）**原样放行**，不影响键盘操作按钮。
        child: CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.arrowLeft): () {
              final id = _prevId;
              if (id != null) _goTo(id);
            },
            const SingleActivator(LogicalKeyboardKey.arrowRight): () {
              final id = _nextId;
              if (id != null) _goTo(id);
            },
          },
          child: Focus(
            autofocus: true,
            child: LayoutBuilder(
              builder: (context, constraints) {
            final wide = constraints.maxWidth >= 1000;
            final info = _InfoPanel(
              media: m,
              prompt: _prompt,
              onChanged: _load,
              onSaveAsPreset: (p) => _saveAsPreset(context, p),
            );
            // ⚠ **不要在这里包 `AnimatedSwitcher`**（2026-10-11 用户报的"切换时整个界面闪烁"）。
            //
            // 曾经把整页（`Row` / `ListView`）包进一个 key 跟着 `_currentId` 变的
            // `AnimatedSwitcher`：那样每切一张图，**整页骨架**都要重建一遍
            // （`LayoutBuilder` → `Row` → 两个 `Expanded` → 滚动容器…），
            // 而旧的那份在 `layoutBuilder` 里还占着位置 —— 于是整块闪。
            //
            // 正确做法：**布局原地不动，只让真正变的那块（图片）自己淡入**。
            // 过渡在 `_previewArea` 里做（见那里的 `AnimatedSwitcher`）。
            if (wide) {
              return Row(
                // stretch：左栏的大图查看器要铺满整屏高度（自己处理拖动 / 缩放）
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(flex: 3, child: _previewArea(m, fileUrl, wide: true)),
                  const VerticalDivider(width: 1),
                  Expanded(
                    flex: 2,
                    child: SingleChildScrollView(
                      padding: const EdgeInsets.fromLTRB(20, 16, 20, 48),
                      child: info,
                    ),
                  ),
                ],
              );
            }
            // 窄屏：图片查看器给一块固定的高度，剩下的空间留给提示词
            final previewHeight = (constraints.maxHeight * 0.62).clamp(240.0, 620.0);
            return ListView(
              children: [
                _previewArea(m, fileUrl, wide: false, height: previewHeight),
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 16, 16, 48),
                  child: info,
                ),
              ],
            );
          },
            ),
          ),
        ),
      ),
        ),
      ),
    );
  }

  /// 预览区。
  ///
  /// 图片自带「拖动 + 滚轮缩放 + 缩略图」，必须拿到一块完整、有界的高度，
  /// 所以它不跟外层一起滚；视频 / 音频还是普通内容，交给外层滚动。
  ///
  /// 整块都能**右键**：弹出复制相关的快捷项（地址 / 文件名 / 提示词），
  /// 不用先划选文字再复制。
  Widget _previewArea(MediaAsset m, String url, {required bool wide, double? height}) {
    // Builder 取一个**在 ContextMenuScope 里面**的 context 传给右键菜单：
    // 页面自己的 `State.context` 在 Scope 外面，用它找不到菜单宿主。
    return Builder(
      builder: (menuContext) {
        final preview = GestureDetector(
          // translucent：视频/音频四周的留白也要右键
          behavior: HitTestBehavior.translucent,
          onSecondaryTapDown: (d) =>
              _showPreviewMenu(menuContext, m, url, d.globalPosition),
          // ⚠ **滚轮冲突**（2026-10-11 用户报的"窄屏下滚轮同时滚界面和缩放图片"）：
          //
          // 窄屏时详情页是 `ListView`，而大图查看器的滚轮缩放（`InteractiveViewer` 自带）
          // 与列表滚动**同时响应同一个事件** —— 结果滚一下既翻页又缩放。
          //
          // 修法：在预览区外面包一层 `Listener`，把滚轮事件**登记进手势竞技场**，
          // 外层 `Scrollable` 的处理器就会跳过它（这正是 Flutter 自己的
          // `Scrollable` 处理滚轮的方式：`GestureBinding.instance.pointerSignalResolver`，
          // 谁先注册谁赢，而命中测试是**自下而上**的 —— `InteractiveViewer` 在里层先拿到）。
          //
          // 代价（刻意接受）：鼠标停在图上时**不能**用滚轮翻页，得把鼠标移开图再滚。
          // 这是"图上的滚轮 = 缩放"这条通用约定的必然结果 —— 看图工具都这么做。
          child: Listener(
            onPointerSignal: (event) {
              if (event is PointerScrollEvent) {
                GestureBinding.instance.pointerSignalResolver.register(event, (_) {});
              }
            },
            // **过渡只做在图片上**（用户 2026-10-11："切换时整个界面闪烁"）：
            // 布局原地不动，只有这一块淡入。key 跟着 `_currentId` 变 ——
            // 同一个 id 重建时（比如下拉刷新）不重放动画。
            //
            // ⚠ 别把这个 `AnimatedSwitcher` 提到外层：提到外层就是"整页骨架重建"，
            // 那正是闪烁的来源（`LayoutBuilder`/`Row`/滚动容器全都要重来一遍）。
            child: AnimatedSwitcher(
              duration: const Duration(milliseconds: 140),
              // ⚠ **别用默认的 `layoutBuilder`**：它把新旧两份摆进一个 `Stack`
              // 同时布局 —— 预览区是"由外层 `Row` 的 stretch 给高度"的，
              // 两份同时存在时其中一份会拿到 unbounded 高度（溢出/闪烁）。
              // 这里只留"新的那一份"，过渡就只剩淡入，布局始终只有一份。
              layoutBuilder: (current, previous) => current ?? const SizedBox.shrink(),
              child: KeyedSubtree(
                key: ValueKey<int>(_currentId),
                child: _Preview(media: m, url: url, posterUrl: _posterUrl(m)),
              ),
            ),
          ),
        );

        // 同批产物：左右箭头 + "第几张 / 共几张"（用户 2026-10-11）。
        //
        // **只给图片**：视频 / 音频的"上一个 / 下一个"在**播放控件那一行**
        // （用户 2026-10-11 明确分开的），所以那两种 kind 直接就是 `preview`。
        // 只有**多于一张**时才叠 —— 单张（或没进组）时连位置都不占
        // （用户定的"只有一份产物时禁用箭头"）。
        final withNav = (m.kind == MediaKind.image && _siblings.length > 1)
            ? Stack(
                fit: StackFit.expand,
                children: [
                  preview,
                  Positioned(
                    left: 4,
                    top: 0,
                    bottom: 0,
                    child: Center(
                      child: _NavArrow(
                        icon: Icons.chevron_left,
                        tooltip: '上一张（←）',
                        onPressed: _prevId == null ? null : () => _goTo(_prevId!),
                      ),
                    ),
                  ),
                  Positioned(
                    right: 4,
                    top: 0,
                    bottom: 0,
                    child: Center(
                      child: _NavArrow(
                        icon: Icons.chevron_right,
                        tooltip: '下一张（→）',
                        onPressed: _nextId == null ? null : () => _goTo(_nextId!),
                      ),
                    ),
                  ),
                  Positioned(
                    top: 8,
                    left: 0,
                    right: 0,
                    child: Center(
                      child: _NavCounter(
                        index: _indexInSiblings + 1,
                        total: _siblings.length,
                      ),
                    ),
                  ),
                ],
              )
            : preview;

        if (m.kind == MediaKind.image) {
          return SizedBox(
            width: double.infinity,
            height: wide ? null : height, // 宽屏时高度由 Row 的 stretch 给
            child: withNav,
          );
        }
        if (m.kind == MediaKind.audio) {
          // 音频没有画面，不需要占满整栏：限宽 + 居中，剩下的空间留给提示词
          final centered = Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 560),
              child: withNav,
            ),
          );
          return wide ? centered : withNav;
        }
        // 视频：宽屏铺满整栏（长边铺满靠播放器自己算），窄屏给固定高度、
        // 不放进滚动容器 —— 播放器的控制条和全屏按钮需要一块稳定可见的区域。
        return SizedBox(
          width: double.infinity,
          height: wide ? null : (height ?? 420),
          child: withNav,
        );
      },
    );
  }

  /// 视频封面（预览图）：后端用 ffmpeg 抽的第一帧；拿不到就返回 null，
  /// 播放器会退化成转圈，不影响播放。
  String? _posterUrl(MediaAsset m) {
    if (m.kind != MediaKind.video) return null;
    final base = context.read<SettingsStore>().baseUrl;
    return '$base/api/media/${m.id}/poster';
  }

  /// 详情页里对媒体本身右键：只列"复制"相关的操作。
  ///
  /// [source] 必须是**在 `ContextMenuScope` 里面**的 context（`_previewArea` 里用
  /// `Builder` 取的那一个），否则找不到右键菜单宿主。
  Future<void> _showPreviewMenu(
    BuildContext source,
    MediaAsset m,
    String url,
    Offset globalPosition,
  ) async {
    final positive = (_prompt?.positivePrompt ?? m.promptPositive ?? '').trim();
    final negative = (_prompt?.negativePrompt ?? m.promptNegative ?? '').trim();

    final action = await showContextMenuAt<String>(
      source,
      globalPosition: globalPosition,
      items: (_) => [
        const MenuOption(value: 'copyUrl', icon: Icons.link, label: '复制文件地址'),
        // 复制**文件本身**（写 CF_HDROP）：资源管理器里能直接粘出文件
        if (FileClipboard.supported)
          const MenuOption(
            value: 'copyFile',
            icon: Icons.file_copy_outlined,
            label: '复制文件',
          ),
        const MenuOption(
          value: 'copyName',
          icon: Icons.description_outlined,
          label: '复制文件名',
        ),
        MenuOption(
          value: 'copyPositive',
          icon: Icons.article_outlined,
          label: '复制正向提示词',
          enabled: positive.isNotEmpty,
          dividerBefore: true,
        ),
        MenuOption(
          value: 'copyNegative',
          icon: Icons.block_outlined,
          label: '复制负向提示词',
          enabled: negative.isNotEmpty,
        ),
      ],
    );
    if (action == null || !mounted) return;

    switch (action) {
      case 'copyUrl':
        await copyToClipboard(context, url, label: '文件地址');
        break;
      case 'copyFile':
        await _copyFileToClipboard(
          context.read<LibraryStore>(),
          m.id,
          ScaffoldMessenger.of(context),
        );
        break;
      case 'copyName':
        await copyToClipboard(context, m.originalName, label: '文件名');
        break;
      case 'copyPositive':
        await copyToClipboard(context, positive, label: '正向提示词');
        break;
      case 'copyNegative':
        await copyToClipboard(context, negative, label: '负向提示词');
        break;
    }
  }

  Future<String?> _promptText({
    required String title,
    required String label,
    required String initial,
    bool multiline = false,
  }) async {
    final controller = TextEditingController(text: initial);
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: SizedBox(
          width: 420,
          child: TextField(
            controller: controller,
            autofocus: true,
            maxLines: multiline ? 5 : 1,
            decoration: InputDecoration(labelText: label),
          ),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, controller.text.trim()),
            child: const Text('确定'),
          ),
        ],
      ),
    );
    controller.dispose();
    return result;
  }
}

// ---------------------------------------------------------------------------
//  预览区
// ---------------------------------------------------------------------------

class _Preview extends StatelessWidget {
  final MediaAsset media;
  final String url;
  final String? posterUrl;

  const _Preview({required this.media, required this.url, this.posterUrl});

  @override
  Widget build(BuildContext context) {
    switch (media.kind) {
      case MediaKind.image:
        // 鼠标拖动平移、滚轮以指针为中心缩放，右下角缩略图指示当前视野
        return ZoomableImageView(
          url: url,
          imageSize: (media.width != null && media.height != null)
              ? Size(media.width!.toDouble(), media.height!.toDouble())
              : null,
        );

      case MediaKind.video:
        return Container(
          color: Colors.black,
          width: double.infinity,
          child: PlatformVideoView.supported
              ? PlatformVideoView(url: url, posterUrl: posterUrl)
              : _Unsupported(
                  icon: Icons.movie_outlined,
                  message: '当前平台不支持内嵌视频播放',
                  hint: '可以用右上角菜单里的「用系统播放器打开」',
                ),
        );

      case MediaKind.audio:
        return Padding(
          padding: const EdgeInsets.all(12),
          child: AudioPlayerView(url: url, title: media.title.isEmpty ? media.originalName : media.title),
        );
    }
  }
}

/// 同批产物之间切换的左右箭头。
///
/// `onPressed == null` = **禁用**（不是隐藏）—— 用户 2026-10-11 明确要的是
/// "只有一份产物时禁用箭头"，所以边界上那个箭头仍在，只是按不动。
class _NavArrow extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback? onPressed;

  const _NavArrow({required this.icon, required this.tooltip, this.onPressed});

  @override
  Widget build(BuildContext context) {
    final enabled = onPressed != null;
    return Tooltip(
      message: tooltip,
      child: Material(
        color: Colors.black.withValues(alpha: enabled ? 0.45 : 0.18),
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onPressed,
          child: Padding(
            padding: const EdgeInsets.all(6),
            child: Icon(
              icon,
              size: 26,
              color: enabled ? Colors.white : Colors.white38,
            ),
          ),
        ),
      ),
    );
  }
}

/// "第几张 / 共几张"的小胶囊。
class _NavCounter extends StatelessWidget {
  final int index;
  final int total;

  const _NavCounter({required this.index, required this.total});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Text(
        '$index / $total',
        style: const TextStyle(color: Colors.white, fontSize: 11.5, fontWeight: FontWeight.w600),
      ),
    );
  }
}

class _Unsupported extends StatelessWidget {
  final IconData icon;
  final String message;
  final String? hint;

  const _Unsupported({required this.icon, required this.message, this.hint});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 46, color: Colors.white54),
          const SizedBox(height: 10),
          Text(message, style: const TextStyle(color: Colors.white70)),
          if (hint != null) ...[
            const SizedBox(height: 4),
            Text(hint!, style: const TextStyle(color: Colors.white38, fontSize: 12)),
          ],
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
//  信息面板：提示词 + 标签 + 元信息
// ---------------------------------------------------------------------------

class _InfoPanel extends StatelessWidget {
  final MediaAsset media;
  final Prompt? prompt;
  final Future<void> Function() onChanged;

  /// 「存为预设」：由页面状态实现（它要拿 LibraryStore 与弹窗）。
  /// `null` = 不显示这个按钮（用例里直接起这个面板时）。
  final void Function(Prompt prompt)? onSaveAsPreset;

  const _InfoPanel({
    required this.media,
    required this.prompt,
    required this.onChanged,
    this.onSaveAsPreset,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.read<LibraryStore>();
    final p = prompt;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // ---------------- 关联的提示词 ----------------
        // 这里只放"提示词本身"（标题 + 正向 / 负向全文）；
        // 模型 / Seed 这些生成参数归下面单独的卡片，两类信息别混在一张卡里。
        Card(
          child: Padding(
            padding: const EdgeInsets.all(14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // ⚠ 用 `Wrap` 而不是 `Row`（2026-10-11 用户报的窄屏 pixel overflow）：
                // 这一行是"图标 + 固定宽标题 + 两个按钮"，窄屏（320/384dp）下
                // 固定部分加起来就超过卡片宽度 → `RenderFlex overflowed`。
                // `Wrap` 放不下就把按钮整组换到下一行，**一个控件都不会被裁**。
                Wrap(
                  spacing: 6,
                  runSpacing: 0,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Icon(Icons.link, size: 18, color: theme.colorScheme.primary),
                    Text('关联的提示词', style: theme.textTheme.titleSmall),
                    TextButton.icon(
                      onPressed: () => _changeLink(context),
                      icon: const Icon(Icons.sync_alt, size: 16),
                      label: Text(media.hasPrompt ? '更换' : '关联'),
                    ),
                    if (media.hasPrompt)
                      TextButton.icon(
                        onPressed: () => _unlink(context),
                        icon: const Icon(Icons.link_off, size: 16),
                        label: const Text('解除'),
                      ),
                  ],
                ),
                const SizedBox(height: 6),
                if (p == null)
                  Text(
                    '这条产物还没有关联提示词。关联之后，点开图片就能直接看到当初用的提示词。',
                    style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                  )
                else ...[
                  InkWell(
                    borderRadius: BorderRadius.circular(8),
                    onTap: () async {
                      await Navigator.of(context).push(
                        MaterialPageRoute(
                          builder: (_) => PromptDetailPage(promptId: p.id, showParams: true),
                        ),
                      );
                      await onChanged();
                    },
                    child: Padding(
                      padding: const EdgeInsets.symmetric(vertical: 6),
                      child: Row(
                        children: [
                          KindBadge(kind: p.kind),
                          const SizedBox(width: 8),
                          Expanded(
                            child: Text(
                              p.title,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: theme.textTheme.titleSmall?.copyWith(fontWeight: FontWeight.w600),
                            ),
                          ),
                          const Icon(Icons.chevron_right, size: 18),
                        ],
                      ),
                    ),
                  ),
                  const SizedBox(height: 8),
                  // 「存为预设」（用户 2026-10-11）：把这条产物当初用的提示词
                  // **复制**成一条新预设，而不是就地改这条自动捕获的提示词。
                  //
                  // 为什么是复制：自动捕获的提示词行是**运行记录**（`capture_runs.prompt_id`
                  // 指向它），改它等于篡改台账；而"我想以后反复用这套词"是另一件事。
                  Align(
                    alignment: Alignment.centerLeft,
                    child: TextButton.icon(
                      onPressed: () => onSaveAsPreset?.call(p),
                      icon: const Icon(Icons.bookmark_add_outlined, size: 16),
                      label: const Text('存为预设'),
                    ),
                  ),
                  CopyableText(label: '正向提示词', text: p.positivePrompt),
                  if ((p.negativePrompt ?? '').isNotEmpty) ...[
                    const SizedBox(height: 10),
                    CopyableText(label: '负向提示词', text: p.negativePrompt),
                  ],
                ],
              ],
            ),
          ),
        ),

        // ---------------- 生成参数 ----------------
        // 单独一张卡：模型 / 采样器 / Seed / LoRA 这些是"当时怎么跑的"，
        // 和提示词正文是两类东西，混在一个卡片里既难扫也容易看漏。
        if (p != null && PromptParams.hasAny(p)) ...[
          const SizedBox(height: 16),
          Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(14, 12, 14, 2),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // 同上一处：`Wrap` 而不是 `Row` —— 窄屏下"生成参数"与右边那句说明
                  // 加起来会超过卡片宽度（用户 2026-10-11 报的 overflow 之一）。
                  Wrap(
                    spacing: 6,
                    runSpacing: 2,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Icon(Icons.tune, size: 18, color: theme.colorScheme.primary),
                      Text('生成参数', style: theme.textTheme.titleSmall),
                      Text(
                        '来自关联的提示词',
                        style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  PromptParams(prompt: p),
                ],
              ),
            ),
          ),
        ],
        const SizedBox(height: 16),

        // ---------------- 标签 ----------------
        Text('标签', style: theme.textTheme.titleSmall),
        const SizedBox(height: 6),
        if (media.promptTags.isEmpty)
          Text(
            '暂无标签',
            style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
          )
        else
          Wrap(
            children: [
              for (final t in media.promptTags)
                TagChip(
                  label: t,
                  onTap: () {
                    Navigator.of(context).push(
                      MaterialPageRoute(builder: (_) => TagResultsPage(tag: t)),
                    );
                  },
                ),
            ],
          ),
        const SizedBox(height: 6),
        Text(
          '点击标签可以查看该标签下的全部提示词与产物',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
        const SizedBox(height: 20),

        // ---------------- 元信息 ----------------
        Text('文件信息', style: theme.textTheme.titleSmall),
        const SizedBox(height: 8),
        Card(
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 6),
            child: LayoutBuilder(
              builder: (context, constraints) {
                // 面板够宽就排成两列，键值对铺满整张卡片；窄了退回单列
                final half = (constraints.maxWidth - 18) / 2;
                final twoColumns = constraints.maxWidth >= 460;
                return Wrap(
                  spacing: 18,
                  children: [
                    for (final item in _fileInfo(media))
                      SizedBox(
                        width: (twoColumns && !item.wide) ? half : constraints.maxWidth,
                        child: _kv(theme, item.key, item.value, full: item.full),
                      ),
                  ],
                );
              },
            ),
          ),
        ),
        const SizedBox(height: 16),

        // 和「继续上传」「刷新」放在同一行；窄屏用 Wrap 换行，避免挤出边界
        Wrap(
          spacing: 10,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            if (media.hasWorkflow)
              WorkflowButton(
                title: media.title.isEmpty ? media.originalName : media.title,
                load: () => store.api.mediaWorkflow(media.id),
                blueprint: store.api.workflowBlueprint,
                saveBlueprint: (raw, name) => store.api.saveWorkflow(raw: raw, name: name),
              ),
            OutlinedButton.icon(
              onPressed: () async {
                final ok = await showUploadSheet(context, promptId: media.promptId);
                if (ok) await onChanged();
              },
              icon: const Icon(Icons.upload_file, size: 18),
              label: const Text('继续上传'),
            ),
            TextButton.icon(
              onPressed: () => store.refreshAll(),
              icon: const Icon(Icons.refresh, size: 18),
              label: const Text('刷新'),
            ),
          ],
        ),
      ],
    );
  }

  /// 文件信息的一行。`wide` 表示这一行的值通常很长（文件名 / 备注），
  /// 让它自己占一整行，而不是挤进半列里折行。
  static List<_FileInfoItem> _fileInfo(MediaAsset media) => [
        _FileInfoItem('文件名', media.originalName, wide: true),
        _FileInfoItem(
          '类型',
          '${media.kind.label}${media.mimeType != null ? ' · ${media.mimeType}' : ''}',
        ),
        _FileInfoItem('大小', formatSize(media.sizeBytes)),
        if (media.width != null && media.height != null)
          _FileInfoItem('尺寸', '${media.width} × ${media.height}'),
        _FileInfoItem('来源', media.source ?? ''),
        _FileInfoItem('加入时间', formatDateTime(media.createdAt)),
        if ((media.sha256 ?? '').isNotEmpty)
          // 显示前 16 位，完整值放到 tooltip 里（鼠标停上去就能看全）
          _FileInfoItem('SHA-256', '${media.sha256!.substring(0, 16)}…', full: media.sha256),
        if ((media.notes ?? '').isNotEmpty) _FileInfoItem('备注', media.notes!, wide: true),
      ].where((e) => e.value.isNotEmpty).toList();

  Widget _kv(ThemeData theme, String key, String? value, {String? full}) {
    if (value == null || value.isEmpty) return const SizedBox.shrink();
    // 显示值可能是被截断的（SHA-256 只留前 16 位），鼠标停到文字上给完整内容
    final complete = (full == null || full.isEmpty) ? value : full;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 78,
            child: Text(
              key,
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
          Expanded(
            child: Tooltip(
              message: complete,
              waitDuration: const Duration(milliseconds: 250),
              child: SelectableText(value, style: theme.textTheme.bodySmall),
            ),
          ),
        ],
      ),
    );
  }

  Future<void> _changeLink(BuildContext context) async {
    final store = context.read<LibraryStore>();
    final picked = await showPromptPicker(context);
    if (picked == null) return;
    await store.api.updateMedia(media.id, promptId: picked.id);
    await onChanged();
    if (context.mounted) context.read<LibraryStore>().refreshAll();
  }

  Future<void> _unlink(BuildContext context) async {
    final store = context.read<LibraryStore>();
    await store.api.updateMedia(media.id, clearPrompt: true);
    await onChanged();
    if (context.mounted) context.read<LibraryStore>().refreshAll();
  }
}

/// 「文件信息」里的一行键值对
class _FileInfoItem {
  final String key;

  /// 面板里显示的值（可能会被截断）
  final String value;

  /// 完整值；和 [value] 不同时，鼠标悬停展示这个
  final String? full;

  final bool wide;

  const _FileInfoItem(this.key, this.value, {this.full, this.wide = false});
}
