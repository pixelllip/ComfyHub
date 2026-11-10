import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:url_launcher/url_launcher.dart';

import 'audio_player_view.dart';
import 'video_view.dart';
import 'zoomable_image_view.dart';

/// **在应用内**打开一条附件（不交给系统默认程序）。
///
/// 文件附件在应用内以弹窗打开，可关闭。
/// 以前的规矩是"图片内嵌预览、其它类型一律 ShellExecute"——于是点一个 PDF / 文档
/// 会把整个外部程序拉起来，回来的路还得自己找。现在所有类型都先走这个弹窗：
///
///  · 图片 → 复用聊天里那个大图查看器（[showImagePreview]）；
///  · 视频 → [PlatformVideoView]（Windows 走 Media Foundation，手机走官方播放器）；
///  · 音频 → [AudioPlayerView]；
///  · 文本 / 代码 / JSON → 取回正文，滚着看（超过上限如实标"已截断"）；
///  · 其它（PDF / Office / 压缩包…）→ **也弹窗**，说明当前不能内嵌渲染，
///    并留一个显式的「用系统程序打开」退路（不是自动外跳，也不会悄悄什么都不做）。
///
/// [headers] 只在取文本时需要（配对令牌等）；图片 / 视频 / 音频仍由各自控件直连。
Future<void> showAttachmentPreview(
  BuildContext context, {
  required String url,
  required String name,
  String? modality,
  Map<String, String>? headers,
}) async {
  final kind = _kindOf(modality: modality, name: name, url: url);
  if (kind == _PreviewKind.image) {
    await showImagePreview(context, url: url);
    return;
  }
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (_) =>
        _AttachmentDialog(url: url, name: name, kind: kind, headers: headers),
  );
}

enum _PreviewKind { image, video, audio, text, other }

const _textExtensions = {
  'txt',
  'md',
  'markdown',
  'json',
  'jsonl',
  'yaml',
  'yml',
  'csv',
  'tsv',
  'log',
  'xml',
  'html',
  'htm',
  'css',
  'js',
  'mjs',
  'ts',
  'dart',
  'py',
  'rs',
  'kt',
  'java',
  'c',
  'h',
  'cpp',
  'hpp',
  'cs',
  'go',
  'rb',
  'php',
  'sql',
  'sh',
  'ps1',
  'bat',
  'toml',
  'ini',
  'cfg',
  'conf',
  'env',
};

const _videoExtensions = {
  'mp4',
  'mov',
  'mkv',
  'webm',
  'avi',
  'm4v',
  'wmv',
  'flv',
};
const _audioExtensions = {
  'mp3',
  'wav',
  'flac',
  'ogg',
  'm4a',
  'aac',
  'opus',
  'wma',
};

String _extensionOf(String name, String url) {
  final source = name.trim().isNotEmpty ? name : url;
  final clean = source.split('?').first.split('#').first;
  final dot = clean.lastIndexOf('.');
  if (dot < 0 || dot == clean.length - 1) return '';
  return clean.substring(dot + 1).toLowerCase();
}

_PreviewKind _kindOf({
  required String? modality,
  required String name,
  required String url,
}) {
  final declared = (modality ?? '').toLowerCase().trim();
  if (declared == 'image') return _PreviewKind.image;
  if (declared == 'video') return _PreviewKind.video;
  if (declared == 'audio') return _PreviewKind.audio;
  // 历史消息里没有 modality（后端只落 attachmentId 与名字），按扩展名兜底。
  final ext = _extensionOf(name, url);
  if (const {
    'png',
    'jpg',
    'jpeg',
    'webp',
    'gif',
    'bmp',
    'avif',
  }.contains(ext)) {
    return _PreviewKind.image;
  }
  if (_videoExtensions.contains(ext)) return _PreviewKind.video;
  if (_audioExtensions.contains(ext)) return _PreviewKind.audio;
  if (_textExtensions.contains(ext) || declared == 'text') {
    return _PreviewKind.text;
  }
  return _PreviewKind.other;
}

IconData _iconOf(_PreviewKind kind) => switch (kind) {
  _PreviewKind.image => Icons.image_outlined,
  _PreviewKind.video => Icons.movie_outlined,
  _PreviewKind.audio => Icons.audiotrack_outlined,
  _PreviewKind.text => Icons.description_outlined,
  _PreviewKind.other => Icons.attach_file,
};

/// 弹窗骨架：标题栏（图标 + 文件名 + 关闭）+ 内容区。
///
/// 关闭按钮**始终在右上角**（Material 的习惯位置），Esc / 点遮罩也能关。
class _AttachmentDialog extends StatelessWidget {
  final String url;
  final String name;
  final _PreviewKind kind;
  final Map<String, String>? headers;

  const _AttachmentDialog({
    required this.url,
    required this.name,
    required this.kind,
    this.headers,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final screen = MediaQuery.sizeOf(context);
    final maxWidth = (screen.width * 0.9).clamp(320.0, 1100.0);
    final maxHeight = (screen.height * 0.9).clamp(260.0, 820.0);
    return Dialog(
      insetPadding: const EdgeInsets.all(16),
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: maxWidth, maxHeight: maxHeight),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 8, 6, 6),
              child: Row(
                children: [
                  Icon(
                    _iconOf(kind),
                    size: 18,
                    color: theme.colorScheme.primary,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleSmall,
                    ),
                  ),
                  IconButton(
                    tooltip: '关闭',
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: () => Navigator.of(context).maybePop(),
                  ),
                ],
              ),
            ),
            const Divider(height: 1),
            Flexible(child: _body(context)),
          ],
        ),
      ),
    );
  }

  Widget _body(BuildContext context) {
    switch (kind) {
      case _PreviewKind.video:
        if (!PlatformVideoView.supported) {
          return _Fallback(
            url: url,
            title: '当前平台不支持内嵌视频播放',
            detail: '可以点下面的按钮用系统播放器打开。',
          );
        }
        return Padding(
          padding: const EdgeInsets.all(10),
          child: AspectRatio(
            aspectRatio: 16 / 9,
            child: PlatformVideoView(url: url),
          ),
        );
      case _PreviewKind.audio:
        return Padding(
          padding: const EdgeInsets.all(18),
          child: AudioPlayerView(url: url, title: name),
        );
      case _PreviewKind.text:
        return _TextPreview(url: url, headers: headers);
      case _PreviewKind.image:
      case _PreviewKind.other:
        return _Fallback(
          url: url,
          title: '这种文件暂不支持在应用内预览',
          detail: '需要时可以点下面的按钮用系统程序打开（会离开本应用）。',
        );
    }
  }
}

/// 文本附件：取回正文，滚着看。
class _TextPreview extends StatefulWidget {
  final String url;
  final Map<String, String>? headers;

  const _TextPreview({required this.url, this.headers});

  @override
  State<_TextPreview> createState() => _TextPreviewState();
}

class _TextPreviewState extends State<_TextPreview> {
  static const _maxChars = 200000;

  String? _text;
  String? _error;
  bool _truncated = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final response = await http.get(
        Uri.parse(widget.url),
        headers: widget.headers ?? const {},
      );
      if (!mounted) return;
      if (response.statusCode < 200 || response.statusCode >= 300) {
        setState(() => _error = '读取失败（HTTP ${response.statusCode}）');
        return;
      }
      var text = utf8.decode(response.bodyBytes, allowMalformed: true);
      if (text.length > _maxChars) {
        text = text.substring(0, _maxChars);
        _truncated = true;
      }
      setState(() => _text = text);
    } catch (e) {
      if (mounted) setState(() => _error = '读取失败，$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Text(_error!, textAlign: TextAlign.center),
        ),
      );
    }
    if (_text == null) {
      return const Center(child: CircularProgressIndicator());
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_truncated)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
            child: Text(
              '文件太大，只显示前 $_maxChars 个字符（完整内容请用系统程序打开）。',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.tertiary,
              ),
            ),
          ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(12),
            child: SelectableText(
              _text!,
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: 'monospace',
                height: 1.45,
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// 不能内嵌渲染的类型：说清楚，并给一条显式的系统打开退路。
class _Fallback extends StatelessWidget {
  final String url;
  final String title;
  final String detail;

  const _Fallback({
    required this.url,
    required this.title,
    required this.detail,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.insert_drive_file_outlined,
            size: 48,
            color: theme.colorScheme.outline,
          ),
          const SizedBox(height: 10),
          Text(
            title,
            style: theme.textTheme.titleSmall,
            textAlign: TextAlign.center,
          ),
          const SizedBox(height: 4),
          Text(
            detail,
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
            textAlign: TextAlign.center,
          ),
          const SizedBox(height: 14),
          OutlinedButton.icon(
            onPressed: () async {
              try {
                await launchUrl(
                  Uri.parse(url),
                  mode: LaunchMode.externalApplication,
                );
              } catch (_) {
                // 打不开就什么都不做：地址在标题栏下方能看着
              }
            },
            icon: const Icon(Icons.open_in_new, size: 16),
            label: const Text('用系统程序打开'),
          ),
        ],
      ),
    );
  }
}
