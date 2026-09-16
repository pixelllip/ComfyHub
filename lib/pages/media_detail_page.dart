import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';

import '../core/formatting.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/audio_player_view.dart';
import '../widgets/common.dart';
import '../widgets/prompt_params.dart';
import '../widgets/upload_sheet.dart';
import '../widgets/win_video_view.dart';
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
  MediaAsset? _media;
  Prompt? _prompt;
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  Future<void> _load() async {
    if (!mounted) return;
    final store = context.read<LibraryStore>();
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final media = await store.api.getMedia(widget.mediaId);
      Prompt? prompt;
      if (media.promptId != null) {
        try {
          prompt = await store.api.getPrompt(media.promptId!);
        } catch (_) {
          prompt = null;
        }
      }
      if (!mounted) return;
      setState(() {
        _media = media;
        _prompt = prompt;
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

    return Scaffold(
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
          PopupMenuButton<String>(
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
            itemBuilder: (_) => const [
              PopupMenuItem(value: 'rename', child: ListTile(leading: Icon(Icons.drive_file_rename_outline), title: Text('重命名'))),
              PopupMenuItem(value: 'notes', child: ListTile(leading: Icon(Icons.notes), title: Text('编辑备注'))),
              PopupMenuItem(value: 'open', child: ListTile(leading: Icon(Icons.open_in_new), title: Text('用系统播放器打开'))),
              PopupMenuItem(value: 'copyUrl', child: ListTile(leading: Icon(Icons.link), title: Text('复制文件地址'))),
              PopupMenuItem(value: 'delete', child: ListTile(leading: Icon(Icons.delete_outline), title: Text('删除'))),
            ],
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _load,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final wide = constraints.maxWidth >= 1000;
            final info = _InfoPanel(
              media: m,
              prompt: _prompt,
              onChanged: _load,
            );
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
    final preview = GestureDetector(
      // translucent：视频/音频四周的留白也要右键
      behavior: HitTestBehavior.translucent,
      onSecondaryTapDown: (d) => _showPreviewMenu(m, url, d.globalPosition),
      child: _Preview(media: m, url: url, posterUrl: _posterUrl(m)),
    );

    if (m.kind == MediaKind.image) {
      return SizedBox(
        width: double.infinity,
        height: wide ? null : height, // 宽屏时高度由 Row 的 stretch 给
        child: preview,
      );
    }
    if (m.kind == MediaKind.audio) {
      // 音频没有画面，不需要占满整栏：限宽 + 居中，剩下的空间留给提示词
      final centered = Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 560),
          child: preview,
        ),
      );
      return wide ? centered : preview;
    }
    // 视频：宽屏铺满整栏（长边铺满靠播放器自己算），窄屏给固定高度、
    // 不放进滚动容器 —— 播放器的控制条和全屏按钮需要一块稳定可见的区域。
    return SizedBox(
      width: double.infinity,
      height: wide ? null : (height ?? 420),
      child: preview,
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
  Future<void> _showPreviewMenu(MediaAsset m, String url, Offset globalPosition) async {
    final positive = (_prompt?.positivePrompt ?? m.promptPositive ?? '').trim();
    final negative = (_prompt?.negativePrompt ?? m.promptNegative ?? '').trim();

    final action = await showContextMenuAt<String>(
      context,
      globalPosition: globalPosition,
      items: [
        contextMenuItem(context, value: 'copyUrl', icon: Icons.link, label: '复制文件地址'),
        contextMenuItem(
          context,
          value: 'copyName',
          icon: Icons.description_outlined,
          label: '复制文件名',
        ),
        const PopupMenuDivider(),
        contextMenuItem(
          context,
          value: 'copyPositive',
          icon: Icons.article_outlined,
          label: '复制正向提示词',
          enabled: positive.isNotEmpty,
        ),
        contextMenuItem(
          context,
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
          child: WinVideoView.supported
              ? WinVideoView(url: url, posterUrl: posterUrl)
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

  const _InfoPanel({required this.media, required this.prompt, required this.onChanged});

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
                Row(
                  children: [
                    Icon(Icons.link, size: 18, color: theme.colorScheme.primary),
                    const SizedBox(width: 6),
                    Text('关联的提示词', style: theme.textTheme.titleSmall),
                    const Spacer(),
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
                        MaterialPageRoute(builder: (_) => PromptDetailPage(promptId: p.id)),
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
                  Row(
                    children: [
                      Icon(Icons.tune, size: 18, color: theme.colorScheme.primary),
                      const SizedBox(width: 6),
                      Text('生成参数', style: theme.textTheme.titleSmall),
                      const Spacer(),
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
