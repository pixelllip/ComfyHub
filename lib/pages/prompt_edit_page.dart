import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../models/models.dart';
import '../state/library_store.dart';
import '../widgets/tag_editor.dart';

/// 新建 / 编辑提示词
class PromptEditPage extends StatefulWidget {
  final Prompt? prompt;

  /// 打开时预填的标签（例如从某个标签筛选里直接新建）
  final List<String> initialTags;

  const PromptEditPage({super.key, this.prompt, this.initialTags = const []});

  @override
  State<PromptEditPage> createState() => _PromptEditPageState();
}

class _PromptEditPageState extends State<PromptEditPage> {
  final _formKey = GlobalKey<FormState>();

  late final TextEditingController _title;
  late final TextEditingController _positive;
  late final TextEditingController _negative;
  late final TextEditingController _checkpoint;
  late final TextEditingController _sampler;
  late final TextEditingController _scheduler;
  late final TextEditingController _steps;
  late final TextEditingController _cfg;
  late final TextEditingController _seed;
  late final TextEditingController _width;
  late final TextEditingController _height;
  late final TextEditingController _batch;
  late final TextEditingController _notes;

  late PromptKind _kind;
  late bool _favorite;
  late List<String> _tags;
  late List<LoraRef> _loras;
  bool _saving = false;

  bool get isNew => widget.prompt == null;

  @override
  void initState() {
    super.initState();
    final p = widget.prompt;
    _title = TextEditingController(text: p?.title ?? '');
    _positive = TextEditingController(text: p?.positivePrompt ?? '');
    _negative = TextEditingController(text: p?.negativePrompt ?? '');
    _checkpoint = TextEditingController(text: p?.checkpoint ?? '');
    _sampler = TextEditingController(text: p?.sampler ?? '');
    _scheduler = TextEditingController(text: p?.scheduler ?? '');
    _steps = TextEditingController(text: p?.steps?.toString() ?? '');
    _cfg = TextEditingController(text: p?.cfgScale?.toString() ?? '');
    _seed = TextEditingController(text: p?.seed?.toString() ?? '');
    _width = TextEditingController(text: p?.width?.toString() ?? '');
    _height = TextEditingController(text: p?.height?.toString() ?? '');
    _batch = TextEditingController(text: p?.batchSize?.toString() ?? '');
    _notes = TextEditingController(text: p?.notes ?? '');
    _kind = p?.kind ?? PromptKind.image;
    _favorite = p?.favorite ?? false;
    _tags = [
      ...(p?.tags.map((e) => e.name) ?? const <String>[]),
      ...widget.initialTags.where((t) => !(p?.tags.any((e) => e.name == t) ?? false)),
    ];
    _loras = List.of(p?.loras ?? const <LoraRef>[]);
  }

  @override
  void dispose() {
    for (final c in [
      _title, _positive, _negative, _checkpoint, _sampler, _scheduler,
      _steps, _cfg, _seed, _width, _height, _batch, _notes,
    ]) {
      c.dispose();
    }
    super.dispose();
  }

  int? _intOf(TextEditingController c) => int.tryParse(c.text.trim());
  double? _doubleOf(TextEditingController c) => double.tryParse(c.text.trim());

  Future<void> _save() async {
    if (!_formKey.currentState!.validate()) return;
    if (_positive.text.trim().isEmpty && _title.text.trim().isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('正向提示词和标题至少填一个')),
      );
      return;
    }

    setState(() => _saving = true);
    final store = context.read<LibraryStore>();
    final draft = Prompt(
      id: widget.prompt?.id ?? 0,
      title: _title.text.trim(),
      kind: _kind,
      positivePrompt: _positive.text.trim(),
      negativePrompt: _negative.text.trim().isEmpty ? null : _negative.text.trim(),
      checkpoint: _checkpoint.text.trim().isEmpty ? null : _checkpoint.text.trim(),
      loras: _loras,
      sampler: _sampler.text.trim().isEmpty ? null : _sampler.text.trim(),
      scheduler: _scheduler.text.trim().isEmpty ? null : _scheduler.text.trim(),
      steps: _intOf(_steps),
      cfgScale: _doubleOf(_cfg),
      seed: _intOf(_seed),
      width: _intOf(_width),
      height: _intOf(_height),
      batchSize: _intOf(_batch),
      extraParams: widget.prompt?.extraParams ?? const {},
      notes: _notes.text.trim().isEmpty ? null : _notes.text.trim(),
      favorite: _favorite,
      tags: _tags.map((e) => Tag(id: 0, name: e)).toList(),
    );

    try {
      final saved = isNew ? await store.api.createPrompt(draft) : await store.api.updatePrompt(draft);
      if (!mounted) return;
      await store.refreshAll();
      if (!mounted) return;
      Navigator.of(context).pop(saved);
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('保存失败: $e')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: Text(isNew ? '新建提示词' : '编辑提示词'),
        actions: [
          TextButton.icon(
            onPressed: _saving ? null : _save,
            icon: _saving
                ? const SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2))
                : const Icon(Icons.check),
            label: const Text('保存'),
          ),
          const SizedBox(width: 8),
        ],
      ),
      body: Form(
        key: _formKey,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 48),
          children: [
            TextFormField(
              controller: _title,
              decoration: const InputDecoration(
                labelText: '标题',
                hintText: '留空时会自动取正向提示词的开头',
              ),
            ),
            const SizedBox(height: 16),

            Text('生成类型', style: theme.textTheme.labelLarge),
            const SizedBox(height: 6),
            SegmentedButton<PromptKind>(
              segments: const [
                ButtonSegment(value: PromptKind.image, label: Text('生图'), icon: Icon(Icons.image_outlined)),
                ButtonSegment(value: PromptKind.video, label: Text('生视频'), icon: Icon(Icons.movie_outlined)),
                ButtonSegment(value: PromptKind.audio, label: Text('生音频'), icon: Icon(Icons.music_note_outlined)),
                ButtonSegment(value: PromptKind.mixed, label: Text('混合'), icon: Icon(Icons.auto_awesome_mosaic_outlined)),
              ],
              selected: {_kind},
              onSelectionChanged: (s) => setState(() => _kind = s.first),
            ),
            const SizedBox(height: 18),

            TextFormField(
              controller: _positive,
              maxLines: 6,
              minLines: 4,
              decoration: const InputDecoration(
                labelText: '正向提示词 *',
                alignLabelWithHint: true,
                hintText: 'masterpiece, best quality, ...',
              ),
            ),
            const SizedBox(height: 14),
            TextFormField(
              controller: _negative,
              maxLines: 4,
              minLines: 2,
              decoration: const InputDecoration(
                labelText: '负向提示词',
                alignLabelWithHint: true,
              ),
            ),
            const SizedBox(height: 20),

            _section(theme, '生成参数'),
            const SizedBox(height: 10),
            TextFormField(
              controller: _checkpoint,
              decoration: const InputDecoration(labelText: '模型 / Checkpoint'),
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(child: TextFormField(controller: _sampler, decoration: const InputDecoration(labelText: '采样器 Sampler'))),
                const SizedBox(width: 12),
                Expanded(child: TextFormField(controller: _scheduler, decoration: const InputDecoration(labelText: '调度器 Scheduler'))),
              ],
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(child: _numField(_steps, '步数 Steps', integer: true)),
                const SizedBox(width: 12),
                Expanded(child: _numField(_cfg, 'CFG')),
                const SizedBox(width: 12),
                Expanded(child: _numField(_seed, 'Seed', integer: true)),
              ],
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(child: _numField(_width, '宽', integer: true)),
                const SizedBox(width: 12),
                Expanded(child: _numField(_height, '高', integer: true)),
                const SizedBox(width: 12),
                Expanded(child: _numField(_batch, '批量', integer: true)),
              ],
            ),
            const SizedBox(height: 20),

            _section(theme, 'LoRA'),
            const SizedBox(height: 6),
            ..._buildLoras(theme),
            const SizedBox(height: 20),

            _section(theme, '标签'),
            const SizedBox(height: 6),
            TagEditor(
              value: _tags,
              onChanged: (v) => setState(() => _tags = v),
            ),
            const SizedBox(height: 20),

            TextFormField(
              controller: _notes,
              maxLines: 3,
              decoration: const InputDecoration(
                labelText: '备注',
                alignLabelWithHint: true,
                hintText: '记录踩坑 / 参数心得',
              ),
            ),
            const SizedBox(height: 12),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              value: _favorite,
              onChanged: (v) => setState(() => _favorite = v),
              title: const Text('加入收藏'),
              secondary: const Icon(Icons.star_outline),
            ),
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: _saving ? null : _save,
              icon: const Icon(Icons.save_outlined),
              label: Text(isNew ? '创建' : '保存修改'),
            ),
          ],
        ),
      ),
    );
  }

  Widget _section(ThemeData theme, String title) => Row(
        children: [
          Text(title, style: theme.textTheme.titleSmall?.copyWith(color: theme.colorScheme.primary)),
          const SizedBox(width: 8),
          Expanded(child: Divider(color: theme.dividerColor)),
        ],
      );

  Widget _numField(TextEditingController controller, String label, {bool integer = false}) {
    return TextFormField(
      controller: controller,
      keyboardType: TextInputType.number,
      inputFormatters: [
        FilteringTextInputFormatter.allow(integer ? RegExp(r'[0-9\-]') : RegExp(r'[0-9\.\-]')),
      ],
      decoration: InputDecoration(labelText: label),
    );
  }

  List<Widget> _buildLoras(ThemeData theme) {
    final widgets = <Widget>[];
    for (var i = 0; i < _loras.length; i++) {
      final lora = _loras[i];
      widgets.add(
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Row(
            children: [
              Expanded(
                flex: 3,
                child: TextFormField(
                  initialValue: lora.name,
                  decoration: const InputDecoration(labelText: 'LoRA 名称'),
                  onChanged: (v) => _loras[i] = LoraRef(name: v, weight: lora.weight),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: TextFormField(
                  initialValue: lora.weight.toString(),
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(labelText: '权重'),
                  onChanged: (v) => _loras[i] = LoraRef(
                    name: lora.name,
                    weight: double.tryParse(v) ?? 1.0,
                  ),
                ),
              ),
              IconButton(
                icon: const Icon(Icons.remove_circle_outline),
                onPressed: () => setState(() => _loras.removeAt(i)),
              ),
            ],
          ),
        ),
      );
    }
    widgets.add(
      Align(
        alignment: Alignment.centerLeft,
        child: TextButton.icon(
          onPressed: () => setState(() => _loras.add(const LoraRef(name: '', weight: 1.0))),
          icon: const Icon(Icons.add, size: 18),
          label: const Text('添加 LoRA'),
        ),
      ),
    );
    return widgets;
  }
}
