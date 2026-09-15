import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../core/api_client.dart';
import '../core/settings_store.dart';
import '../models/models.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';

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

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_loaded) return;
    _loaded = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) context.read<AiWorkspaceStore>().load();
    });
  }

  @override
  void dispose() {
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
    final theme = Theme.of(context);
    if (store.messages.isEmpty) return _EmptyThread(store: store);

    return ListView.builder(
      controller: controller,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      itemCount: store.messages.length,
      itemBuilder: (context, i) {
        final m = store.messages[i];
        final align = m.isUser ? CrossAxisAlignment.end : CrossAxisAlignment.start;
        final bg = m.isUser
            ? theme.colorScheme.primaryContainer
            : theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.5);
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
                  if (m.text.isNotEmpty) SelectableText(m.text),
                ],
              ),
            ),
          ],
        );
      },
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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    final blockers = store.preflight?.blockers ?? const <String>[];

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
          Row(
            children: [
              IconButton(
                tooltip: '添加附件',
                onPressed: _pickFiles,
                icon: const Icon(Icons.attach_file),
              ),
              _ModelPicker(store: store),
              const SizedBox(width: 6),
              Expanded(
                child: Shortcuts(
                  shortcuts: const {
                    SingleActivator(LogicalKeyboardKey.enter): _SendIntent(),
                  },
                  child: Actions(
                    actions: {
                      _SendIntent: CallbackAction<_SendIntent>(
                        onInvoke: (_) {
                          // 中文输入法 composing 期间的回车是"选词"，不能当发送（AIH-053）
                          if (_composing) return null;
                          _submit();
                          return null;
                        },
                      ),
                    },
                    child: TextField(
                      controller: widget.controller,
                      focusNode: widget.focusNode,
                      minLines: 1,
                      maxLines: 6,
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
              ),
              const SizedBox(width: 8),
              FilledButton.icon(
                onPressed: store.canSend || store.sending ? _submit : null,
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

class _ModelPicker extends StatelessWidget {
  final AiWorkspaceStore store;
  const _ModelPicker({required this.store});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final model = store.selectedModel;
    return PopupMenuButton<String>(
      tooltip: '选择模型',
      onSelected: (id) {
        final m = store.models.where((x) => x.id == id).firstOrNull;
        store.selectModel(m);
      },
      itemBuilder: (_) => [
        for (final m in store.models)
          PopupMenuItem(
            value: m.id,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(m.displayName),
                Text(
                  '${m.modalities.map((e) => e.label).join(' / ')}'
                  '${m.tools ? ' · 工具' : ''} · ${m.capabilitySourceLabel}',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
                ),
              ],
            ),
          ),
        if (store.models.isEmpty)
          const PopupMenuItem(enabled: false, value: '', child: Text('还没有模型，请到设置里添加')),
      ],
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
          ],
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

    return ListView(
      padding: const EdgeInsets.all(12),
      children: [
        Row(
          children: [
            Text('上下文', style: theme.textTheme.titleSmall),
            const Spacer(),
            IconButton(
              tooltip: '刷新 ComfyUI 状态',
              onPressed: _loading ? null : _refresh,
              icon: const Icon(Icons.refresh, size: 16),
            ),
          ],
        ),
        const Divider(),
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
        Text('ComfyUI', style: theme.textTheme.labelLarge),
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
          if (_status!.lastError != null) Text(_status!.lastError!, style: theme.textTheme.labelSmall),
        ],
        const SizedBox(height: 16),
        Text('Skills 目录', style: theme.textTheme.labelLarge),
        const SizedBox(height: 4),
        Text(
          '当前只登记目录；按需加载（load_skill）尚未接通，模型还不能读取正文。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
        const SizedBox(height: 6),
        for (final entry in AiWorkspaceStore.skillCatalog.entries)
          ListTile(
            dense: true,
            contentPadding: EdgeInsets.zero,
            title: Text(entry.key, style: theme.textTheme.labelMedium),
            subtitle: Text(entry.value, style: theme.textTheme.labelSmall),
          ),
      ],
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

extension _FirstOrNullExt<T> on Iterable<T> {
  T? get firstOrNull => isEmpty ? null : first;
}
