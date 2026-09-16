import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/common.dart';
import '../widgets/media_thumb.dart';
import 'media_detail_page.dart';
import 'prompt_detail_page.dart';

/// 单个标签的搜索结果页：同时列出该标签下的提示词与生成产物。
/// 这是「提示词可用 tag 搜索」最直观的入口。
class TagResultsPage extends StatefulWidget {
  final String tag;

  const TagResultsPage({super.key, required this.tag});

  @override
  State<TagResultsPage> createState() => _TagResultsPageState();
}

class _TagResultsPageState extends State<TagResultsPage>
    with SingleTickerProviderStateMixin {
  late final TabController _tabs = TabController(length: 2, vsync: this);

  List<Prompt> _prompts = const [];
  List<MediaAsset> _media = const [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    if (!mounted) return;
    final api = context.read<LibraryStore>().api;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final prompts = await api.listPrompts(tags: [widget.tag], size: 100);
      final media = await api.listMedia(tags: [widget.tag], size: 100);
      if (!mounted) return;
      setState(() {
        _prompts = prompts.items;
        _media = media.items;
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
    return Scaffold(
      appBar: AppBar(
        title: Row(
          children: [
            const Icon(Icons.sell_outlined, size: 18),
            const SizedBox(width: 8),
            Text(widget.tag),
          ],
        ),
        bottom: TabBar(
          controller: _tabs,
          tabs: [
            Tab(text: '提示词 (${_prompts.length})'),
            Tab(text: '产物 (${_media.length})'),
          ],
        ),
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : _error != null
              ? ErrorState(message: _error!, onRetry: _load)
              : TabBarView(
                  controller: _tabs,
                  children: [
                    _prompts.isEmpty
                        ? EmptyState(
                            icon: Icons.search_off,
                            title: '没有使用「${widget.tag}」的提示词',
                          )
                        : ListView.separated(
                            padding: const EdgeInsets.all(16),
                            addAutomaticKeepAlives: false,
                            itemCount: _prompts.length,
                            separatorBuilder: (_, _) => const SizedBox(height: 10),
                            itemBuilder: (context, i) {
                              final p = _prompts[i];
                              return Card(
                                child: ListTile(
                                  title: Row(
                                    children: [
                                      KindBadge(kind: p.kind, compact: true),
                                      const SizedBox(width: 8),
                                      Expanded(
                                        child: Text(
                                          p.title,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                        ),
                                      ),
                                    ],
                                  ),
                                  subtitle: Padding(
                                    padding: const EdgeInsets.only(top: 4),
                                    child: Text(
                                      p.positivePrompt,
                                      maxLines: 2,
                                      overflow: TextOverflow.ellipsis,
                                      style: theme.textTheme.bodySmall,
                                    ),
                                  ),
                                  trailing: const Icon(Icons.chevron_right),
                                  onTap: () async {
                                    await Navigator.of(context).push(
                                      MaterialPageRoute(
                                        builder: (_) => PromptDetailPage(promptId: p.id),
                                      ),
                                    );
                                    await _load();
                                  },
                                ),
                              );
                            },
                          ),
                    _media.isEmpty
                        ? EmptyState(
                            icon: Icons.search_off,
                            title: '没有使用「${widget.tag}」的产物',
                          )
                        : GridView.builder(
                            padding: const EdgeInsets.all(16),
                            // 同画廊网格：RepaintBoundary 由 delegate 自动加，
                            // 这里只关掉用不上的 keep-alive 包装
                            addAutomaticKeepAlives: false,
                            gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                              maxCrossAxisExtent: 240,
                              crossAxisSpacing: 10,
                              mainAxisSpacing: 10,
                              childAspectRatio: 1,
                            ),
                            itemCount: _media.length,
                            itemBuilder: (context, i) => MediaThumb(
                              media: _media[i],
                              onTap: () async {
                                await Navigator.of(context).push(
                                  MaterialPageRoute(
                                    builder: (_) => MediaDetailPage(mediaId: _media[i].id),
                                  ),
                                );
                                await _load();
                              },
                            ),
                          ),
                  ],
                ),
    );
  }
}
