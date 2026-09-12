import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/formatting.dart';
import '../models/models.dart';
import '../state/library_store.dart';
import 'tag_editor.dart';

/// 弹出上传面板。返回 true 表示有文件成功入库。
Future<bool> showUploadSheet(
  BuildContext context, {
  int? promptId,
  Prompt? prompt,
}) async {
  final result = await showModalBottomSheet<bool>(
    context: context,
    isScrollControlled: true,
    showDragHandle: true,
    builder: (_) => _UploadSheet(promptId: promptId, prompt: prompt),
  );
  return result ?? false;
}

class _UploadSheet extends StatefulWidget {
  final int? promptId;
  final Prompt? prompt;

  const _UploadSheet({this.promptId, this.prompt});

  @override
  State<_UploadSheet> createState() => _UploadSheetState();
}

class _UploadSheetState extends State<_UploadSheet> {
  final List<PlatformFile> _files = [];
  final _titleController = TextEditingController();
  final _sourceController = TextEditingController(text: 'ComfyUI');
  final _notesController = TextEditingController();

  String? _kindOverride;
  List<String> _tags = [];
  bool _uploading = false;
  double _progress = 0;
  String? _status;
  int? _linkPromptId;

  @override
  void initState() {
    super.initState();
    _linkPromptId = widget.promptId;
    if (widget.prompt != null) {
      _tags = widget.prompt!.tags.map((e) => e.name).toList();
    }
  }

  @override
  void dispose() {
    _titleController.dispose();
    _sourceController.dispose();
    _notesController.dispose();
    super.dispose();
  }

  /// file_picker 12.x 起 `pickFiles` 是静态方法，直接返回 `List<PlatformFile>`。
  static const _allowedExtensions = [
    'png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp', 'avif', 'tiff',
    'mp4', 'webm', 'mov', 'mkv', 'avi', 'm4v',
    'mp3', 'wav', 'flac', 'ogg', 'm4a', 'aac', 'opus',
  ];

  Future<void> _pick() async {
    final picked = await FilePicker.pickFiles(
      dialogTitle: '选择要导入的生成产物',
      type: FileType.custom,
      allowedExtensions: _allowedExtensions,
    );
    if (picked.isEmpty) return;
    setState(() {
      _files
        ..clear()
        ..addAll(picked);
      _status = null;
    });
  }

  Future<void> _upload() async {
    if (_files.isEmpty) return;
    final store = context.read<LibraryStore>();
    setState(() {
      _uploading = true;
      _progress = 0;
      _status = '正在上传…';
    });

    final paths = _files
        .map((f) => f.path)
        .whereType<String>()
        .toList();
    if (paths.isEmpty) {
      setState(() {
        _uploading = false;
        _status = '无法读取文件路径（Web 端暂不支持上传）';
      });
      return;
    }

    try {
      final result = await store.api.uploadMedia(
        filePaths: paths,
        promptId: _linkPromptId,
        kind: _kindOverride,
        title: _titleController.text.trim().isEmpty ? null : _titleController.text.trim(),
        source: _sourceController.text.trim().isEmpty ? null : _sourceController.text.trim(),
        notes: _notesController.text.trim().isEmpty ? null : _notesController.text.trim(),
        tags: _tags,
      );

      await Future.wait([store.refreshMedia(), store.refreshPrompts(), store.refreshTags(), store.refreshStats()]);
      if (!mounted) return;

      setState(() {
        _uploading = false;
        _progress = 1;
        _status = '成功 ${result.items.length} 个'
            '${result.duplicates.isNotEmpty ? '，跳过重复 ${result.duplicates.length} 个' : ''}'
            '${result.failed.isNotEmpty ? '，失败 ${result.failed.length} 个' : ''}';
      });

      if (result.items.isNotEmpty || result.duplicates.isNotEmpty) {
        if (!mounted) return;
        Navigator.of(context).pop(true);
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(_status!)),
        );
      }
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _uploading = false;
        _status = '上传失败: $e';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = context.watch<LibraryStore>();
    final bottomInset = MediaQuery.viewInsetsOf(context).bottom;

    return Padding(
      padding: EdgeInsets.fromLTRB(20, 0, 20, 20 + bottomInset),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('上传生成产物', style: theme.textTheme.titleLarge),
            const SizedBox(height: 4),
            Text(
              '支持图片 / 视频 / 音频，类型会按扩展名自动识别。相同内容的文件不会被重复入库。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
            const SizedBox(height: 16),

            OutlinedButton.icon(
              onPressed: _uploading ? null : _pick,
              icon: const Icon(Icons.folder_open),
              label: Text(_files.isEmpty ? '选择文件…' : '重新选择（已选 ${_files.length} 个）'),
            ),
            if (_files.isNotEmpty) ...[
              const SizedBox(height: 10),
              Container(
                constraints: const BoxConstraints(maxHeight: 140),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(10),
                  color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
                ),
                child: ListView.builder(
                  shrinkWrap: true,
                  itemCount: _files.length,
                  itemBuilder: (_, i) {
                    final f = _files[i];
                    final bytes = f.lengthSync();
                    return ListTile(
                      dense: true,
                      leading: const Icon(Icons.insert_drive_file_outlined),
                      title: Text(f.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                      trailing: Text(
                        bytes == null ? '' : formatSize(bytes),
                        style: Theme.of(context).textTheme.labelSmall,
                      ),
                    );
                  },
                ),
              ),
            ],
            const SizedBox(height: 16),

            // 关联提示词
            Text('关联提示词', style: theme.textTheme.labelLarge),
            const SizedBox(height: 6),
            _PromptSelector(
              promptId: _linkPromptId,
              initialTitle: widget.prompt?.title,
              onChanged: (id, title) => setState(() => _linkPromptId = id),
            ),
            const SizedBox(height: 16),

            Text('类型（默认按扩展名自动识别）', style: theme.textTheme.labelLarge),
            const SizedBox(height: 6),
            Wrap(
              spacing: 6,
              children: [
                ChoiceChip(
                  label: const Text('自动'),
                  selected: _kindOverride == null,
                  onSelected: (_) => setState(() => _kindOverride = null),
                ),
                for (final k in MediaKind.values)
                  ChoiceChip(
                    label: Text(k.label),
                    selected: _kindOverride == k.wire,
                    onSelected: (_) => setState(() => _kindOverride = k.wire),
                  ),
              ],
            ),
            const SizedBox(height: 16),

            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _titleController,
                    decoration: const InputDecoration(labelText: '标题（可选，默认用文件名）'),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: TextField(
                    controller: _sourceController,
                    decoration: const InputDecoration(labelText: '来源'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _notesController,
              maxLines: 2,
              decoration: const InputDecoration(labelText: '备注（可选）'),
            ),
            const SizedBox(height: 16),

            TagEditor(
              label: '给这些产物额外打标签',
              value: _tags,
              onChanged: (v) => setState(() => _tags = v),
            ),
            const SizedBox(height: 20),

            if (_uploading) ...[
              LinearProgressIndicator(value: _progress == 0 ? null : _progress),
              const SizedBox(height: 8),
            ],
            if (_status != null) ...[
              Text(_status!, style: theme.textTheme.bodySmall),
              const SizedBox(height: 8),
            ],

            Row(
              children: [
                TextButton(
                  onPressed: _uploading ? null : () => Navigator.pop(context, false),
                  child: const Text('取消'),
                ),
                const Spacer(),
                FilledButton.icon(
                  onPressed: (_files.isEmpty || _uploading) ? null : _upload,
                  icon: const Icon(Icons.cloud_upload_outlined),
                  label: Text('上传 ${_files.isEmpty ? '' : _files.length} 个文件'),
                ),
              ],
            ),
            if (store.tags.isEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(
                  '提示：标签词表为空，先去「标签」页创建几个吧。',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// 选择要关联的提示词
class _PromptSelector extends StatelessWidget {
  final int? promptId;
  final String? initialTitle;
  final void Function(int?, String?) onChanged;

  const _PromptSelector({
    required this.promptId,
    required this.initialTitle,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      children: [
        Expanded(
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(10),
              color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
            ),
            child: Text(
              promptId == null
                  ? '未关联（稍后可在详情里关联）'
                  : '已关联 #$promptId ${initialTitle ?? ''}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodyMedium,
            ),
          ),
        ),
        const SizedBox(width: 8),
        TextButton(
          onPressed: () async {
            final picked = await showPromptPicker(context);
            if (picked != null) onChanged(picked.id, picked.title);
          },
          child: const Text('选择'),
        ),
        if (promptId != null)
          IconButton(
            tooltip: '取消关联',
            icon: const Icon(Icons.link_off),
            onPressed: () => onChanged(null, null),
          ),
      ],
    );
  }
}

/// 提示词选择弹窗
Future<Prompt?> showPromptPicker(BuildContext context) {
  return showDialog<Prompt>(
    context: context,
    builder: (_) => const _PromptPickerDialog(),
  );
}

class _PromptPickerDialog extends StatefulWidget {
  const _PromptPickerDialog();

  @override
  State<_PromptPickerDialog> createState() => _PromptPickerDialogState();
}

class _PromptPickerDialogState extends State<_PromptPickerDialog> {
  final _controller = TextEditingController();
  List<Prompt> _results = const [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _search(''));
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _search(String q) async {
    if (!mounted) return;
    final api = context.read<LibraryStore>().api;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final page = await api.listPrompts(q: q.trim().isEmpty ? null : q.trim(), size: 30);
      if (!mounted) return;
      setState(() {
        _results = page.items;
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
    return AlertDialog(
      title: const Text('选择要关联的提示词'),
      content: SizedBox(
        width: 480,
        height: 440,
        child: Column(
          children: [
            TextField(
              controller: _controller,
              autofocus: true,
              decoration: const InputDecoration(
                hintText: '搜索标题或提示词内容…',
                prefixIcon: Icon(Icons.search),
              ),
              onSubmitted: _search,
            ),
            const SizedBox(height: 10),
            Expanded(
              child: _loading
                  ? const Center(child: CircularProgressIndicator())
                  : _error != null
                      ? Center(child: Text('加载失败: $_error', textAlign: TextAlign.center))
                      : _results.isEmpty
                          ? const Center(child: Text('没有找到提示词'))
                          : ListView.builder(
                              itemCount: _results.length,
                              itemBuilder: (_, i) {
                                final p = _results[i];
                                return ListTile(
                                  dense: true,
                                  leading: const Icon(Icons.text_snippet_outlined),
                                  title: Text(
                                    p.title,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                  subtitle: Text(
                                    p.positivePrompt,
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                  onTap: () => Navigator.pop(context, p),
                                );
                              },
                            ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('取消')),
      ],
    );
  }
}
