import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/common.dart';
import '../widgets/media_thumb.dart';
import '../widgets/prompt_params.dart';
import '../widgets/tag_editor.dart';
import '../widgets/upload_sheet.dart';
import '../widgets/workflow_viewer.dart';
import 'media_detail_page.dart';
import 'prompt_edit_page.dart';
import 'prompts_page.dart' show confirmDialog;

/// 提示词详情：完整提示词 + 参数 + 标签 + 关联的生成产物
class PromptDetailPage extends StatefulWidget {
  final int promptId;

  const PromptDetailPage({super.key, required this.promptId});

  @override
  State<PromptDetailPage> createState() => _PromptDetailPageState();
}

class _PromptDetailPageState extends State<PromptDetailPage> {
  Prompt? _prompt;
  List<MediaAsset> _media = const [];
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
      final prompt = await store.api.getPrompt(widget.promptId);
      final media = await store.api.promptMedia(widget.promptId);
      if (!mounted) return;
      setState(() {
        _prompt = prompt;
        _media = media;
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
    final theme = Theme.of(context);

    if (_loading && _prompt == null) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    if (_error != null || _prompt == null) {
      return Scaffold(
        appBar: AppBar(),
        body: ErrorState(message: _error ?? '提示词不存在', onRetry: _load),
      );
    }

    final p = _prompt!;
    final store = context.read<LibraryStore>();

    return Scaffold(
      appBar: AppBar(
        title: Text(
          p.title.isEmpty ? '未命名提示词' : p.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        actions: [
          IconButton(
            tooltip: p.favorite ? '取消收藏' : '收藏',
            icon: Icon(
              p.favorite ? Icons.star : Icons.star_border,
              color: p.favorite ? Colors.amber : null,
            ),
            onPressed: () async {
              await store.api.setPromptFavorite(p.id, !p.favorite);
              await _load();
              if (mounted) store.refreshAll();
            },
          ),
          IconButton(
            tooltip: '编辑',
            icon: const Icon(Icons.edit_outlined),
            onPressed: () async {
              await Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => PromptEditPage(prompt: p)),
              );
              await _load();
              if (mounted) store.refreshAll();
            },
          ),
          AppMenuButton<String>(
            tooltip: '更多',
            onSelected: (v) async {
              final messenger = ScaffoldMessenger.of(context);
              final navigator = Navigator.of(context);
              if (v == 'duplicate') {
                await store.api.duplicatePrompt(p.id);
                await store.refreshAll();
                messenger.showSnackBar(const SnackBar(content: Text('已复制')));
              } else if (v == 'delete') {
                final ok = await confirmDialog(
                  context,
                  title: '删除提示词',
                  message: '确定删除「${p.title}」吗？\n关联的产物会保留，但会解除关联。',
                );
                if (ok) {
                  await store.deletePrompt(p.id);
                  navigator.pop();
                }
              }
            },
            options: const [
              MenuOption(value: 'duplicate', icon: Icons.copy, label: '复制一份'),
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
              onPressed: () => controller.isOpen ? controller.close() : controller.open(),
              icon: const Icon(Icons.more_vert),
            ),
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _load,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 48),
          children: [
            // 用 Wrap 而不是 Row：窄窗口下这些小块会自动换行，不会溢出
            Wrap(
              crossAxisAlignment: WrapCrossAlignment.center,
              spacing: 10,
              runSpacing: 8,
              children: [
                KindBadge(kind: p.kind),
                if ((p.source ?? '').isNotEmpty && p.source != 'Manual')
                  _SourceBadge(source: p.source!, sourceRef: p.sourceRef),
                Text(
                  '创建于 ${formatDateTime(p.createdAt)}',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
                if (p.hasWorkflow)
                  WorkflowButton(
                    title: p.title.isEmpty ? '未命名提示词' : p.title,
                    load: () => store.api.promptWorkflow(p.id),
                  ),
              ],
            ),
            const SizedBox(height: 16),

            CopyableText(label: '正向提示词', text: p.positivePrompt),
            const SizedBox(height: 16),
            if ((p.negativePrompt ?? '').isNotEmpty) ...[
              CopyableText(label: '负向提示词', text: p.negativePrompt),
              const SizedBox(height: 16),
            ],

            if (PromptParams.hasAny(p)) ...[
              Text('生成参数', style: theme.textTheme.titleSmall),
              const SizedBox(height: 8),
              PromptParams(prompt: p),
              const SizedBox(height: 8),
            ],

            if ((p.notes ?? '').isNotEmpty) ...[
              Text('备注', style: theme.textTheme.titleSmall),
              const SizedBox(height: 6),
              Text(p.notes!, style: theme.textTheme.bodyMedium?.copyWith(height: 1.5)),
              const SizedBox(height: 16),
            ],

            Row(
              children: [
                Text('标签', style: theme.textTheme.titleSmall),
                const Spacer(),
                TextButton.icon(
                  onPressed: () => _editTags(p),
                  icon: const Icon(Icons.local_offer_outlined, size: 16),
                  label: const Text('管理标签'),
                ),
              ],
            ),
            const SizedBox(height: 4),
            if (p.tags.isEmpty)
              Text(
                '还没有标签 —— 打上标签后就能在提示词库里按标签搜索',
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
              )
            else
              Wrap(
                children: [
                  for (final tag in p.tags)
                    TagChip(
                      label: tag.name,
                      count: tag.useCount,
                      color: parseHexColor(tag.color),
                      onTap: () async {
                        await store.api.removePromptTag(p.id, tag.id);
                        await _load();
                        if (mounted) store.refreshAll();
                      },
                      onDeleted: () async {
                        await store.api.removePromptTag(p.id, tag.id);
                        await _load();
                        if (mounted) store.refreshAll();
                      },
                    ),
                ],
              ),

            const SizedBox(height: 24),
            Row(
              children: [
                Text('关联产物 (${_media.length})', style: theme.textTheme.titleSmall),
                const Spacer(),
                FilledButton.tonalIcon(
                  onPressed: () async {
                    final ok = await showUploadSheet(context, promptId: p.id, prompt: p);
                    if (ok) await _load();
                  },
                  icon: const Icon(Icons.upload_file, size: 18),
                  label: const Text('上传产物'),
                ),
              ],
            ),
            const SizedBox(height: 12),
            if (_media.isEmpty)
              Container(
                padding: const EdgeInsets.symmetric(vertical: 28),
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(color: theme.dividerColor),
                ),
                child: Column(
                  children: [
                    Icon(Icons.image_not_supported_outlined, color: theme.colorScheme.outline),
                    const SizedBox(height: 8),
                    Text('还没有关联的产物', style: theme.textTheme.bodySmall),
                  ],
                ),
              )
            else
              _MediaGrid(media: _media, onChanged: _load),
          ],
        ),
      ),
    );
  }

  Future<void> _editTags(Prompt p) async {
    var working = p.tags.map((e) => e.name).toList();
    final result = await showDialog<List<String>>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('管理标签'),
        content: SizedBox(
          width: 440,
          child: StatefulBuilder(
            builder: (ctx, setState) => SingleChildScrollView(
              child: TagEditor(
                value: working,
                onChanged: (v) => setState(() => working = v),
              ),
            ),
          ),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, working), child: const Text('保存')),
        ],
      ),
    );
    if (result == null || !mounted) return;

    final store = context.read<LibraryStore>();
    // 后端只提供"追加 + 单个移除"，这里做一次差集同步
    final before = p.tags.map((e) => e.name).toSet();
    final after = result.toSet();
    final toAdd = after.difference(before).toList();
    final toRemove = before.difference(after)
        .map((name) => p.tags.firstWhere((t) => t.name == name).id)
        .toList();

    if (toAdd.isNotEmpty) await store.api.addPromptTags(p.id, toAdd);
    for (final tagId in toRemove) {
      await store.api.removePromptTag(p.id, tagId);
    }
    await _load();
    if (mounted) store.refreshAll();
  }
}

/// 提示词来源徽标：只对非手动的来源显示，悬停能看到来源侧的唯一 ID
class _SourceBadge extends StatelessWidget {
  final String source;
  final String? sourceRef;

  const _SourceBadge({required this.source, this.sourceRef});

  static String _labelOf(String source) => switch (source) {
        'ComfyUI' => 'ComfyUI 自动捕获',
        'ComfyUI-Import' => '历史导入',
        // 后端以后加了新来源，原样显示总比显示"未知"有用
        _ => source,
      };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final badge = Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: theme.colorScheme.primary.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.cloud_download_outlined, size: 12, color: theme.colorScheme.primary),
          const SizedBox(width: 4),
          Text(
            _labelOf(source),
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.primary,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );

    final ref = sourceRef;
    if (ref == null || ref.isEmpty) return badge;
    return Tooltip(message: '来源 ID：$ref', child: badge);
  }
}

class _MediaGrid extends StatelessWidget {
  final List<MediaAsset> media;
  final Future<void> Function() onChanged;

  const _MediaGrid({required this.media, required this.onChanged});

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final columns = constraints.maxWidth > 900
            ? 4
            : constraints.maxWidth > 620
                ? 3
                : 2;
        return GridView.builder(
          shrinkWrap: true,
          physics: const NeverScrollableScrollPhysics(),
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: columns,
            crossAxisSpacing: 10,
            mainAxisSpacing: 10,
            childAspectRatio: 1,
          ),
          itemCount: media.length,
          itemBuilder: (context, i) => MediaThumb(
            media: media[i],
            onTap: () async {
              await Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => MediaDetailPage(mediaId: media[i].id)),
              );
              await onChanged();
            },
          ),
        );
      },
    );
  }
}
