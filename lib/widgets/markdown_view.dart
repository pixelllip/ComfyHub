// 把模型返回的 Markdown 渲染成富文本（自研，见 markdown.dart 的解析部分）。
//
// 设计取舍：
//   · **流式友好**：每次 setState 都重新解析整段文本。解析是纯函数、量级很小
//     （聊天回复单条最多几 KB），换来的好处是"半截文本也能正确显示"；
//   · **只读**：不提供编辑，所以直接用 SelectableText.rich 保证可以选中复制；
//   · 链接可点，走系统浏览器（url_launcher），失败只提示不抛。

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import 'markdown.dart';

/// 渲染一段 Markdown。文本为空时返回空盒子（调用方自己决定占位）。
class MarkdownText extends StatelessWidget {
  const MarkdownText(this.source, {super.key, this.style, this.onLinkTap});

  final String source;

  /// 正文基础样式；标题 / 代码块在此基础上放大或换色。
  final TextStyle? style;

  /// 链接点击回调；默认用系统默认程序打开。
  final Future<void> Function(String url)? onLinkTap;

  @override
  Widget build(BuildContext context) {
    if (source.trim().isEmpty) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final base = style ?? theme.textTheme.bodyMedium ?? const TextStyle(fontSize: 14);
    final blocks = parseMarkdown(source);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final b in blocks) _block(context, theme, base, b),
      ],
    );
  }

  Widget _block(BuildContext context, ThemeData theme, TextStyle base, MdBlock block) {
    switch (block) {
      case MdParagraph(:final spans):
        return SelectableText.rich(TextSpan(children: _spans(context, theme, base, spans)),
            style: base);

      case MdHeading(:final level, :final spans):
        final size = switch (level) {
          1 => 22.0,
          2 => 19.0,
          3 => 17.0,
          _ => 15.0,
        };
        final heading = base.copyWith(
          fontSize: size,
          fontWeight: FontWeight.w600,
          height: 1.35,
        );
        return Padding(
          padding: EdgeInsets.only(top: level <= 2 ? 10 : 6, bottom: 4),
          child: SelectableText.rich(
            TextSpan(children: _spans(context, theme, heading, spans)),
            style: heading,
          ),
        );

      case MdListItem(:final spans, :final ordered, :final index):
        final marker = ordered ? '$index.' : '•';
        return Padding(
          padding: const EdgeInsets.only(left: 4, top: 2, bottom: 2),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: ordered ? 26 : 16,
                child: Text(marker, style: base.copyWith(color: theme.colorScheme.outline)),
              ),
              Expanded(
                child: SelectableText.rich(
                  TextSpan(children: _spans(context, theme, base, spans)),
                  style: base,
                ),
              ),
            ],
          ),
        );

      case MdQuote(:final blocks):
        return Container(
          margin: const EdgeInsets.symmetric(vertical: 4),
          padding: const EdgeInsets.only(left: 10),
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(color: theme.colorScheme.outlineVariant, width: 3),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              for (final inner in blocks)
                DefaultTextStyle.merge(
                  style: base.copyWith(color: theme.colorScheme.onSurfaceVariant),
                  child: _block(context, theme, base.copyWith(color: theme.colorScheme.onSurfaceVariant), inner),
                ),
            ],
          ),
        );

      case MdCodeBlock(:final code, :final language):
        return Container(
          width: double.infinity,
          margin: const EdgeInsets.symmetric(vertical: 6),
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.7),
            borderRadius: BorderRadius.circular(8),
            border: Border.all(color: theme.colorScheme.outlineVariant.withValues(alpha: 0.5)),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (language != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(10, 6, 10, 0),
                  child: Text(language,
                      style: theme.textTheme.labelSmall
                          ?.copyWith(color: theme.colorScheme.outline)),
                ),
              SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                padding: const EdgeInsets.fromLTRB(10, 6, 10, 8),
                child: SelectableText(
                  code,
                  style: base.copyWith(
                    fontFamily: 'Consolas',
                    fontFamilyFallback: const ['Courier New', 'monospace'],
                    fontSize: (base.fontSize ?? 14) - 0.5,
                    height: 1.4,
                  ),
                ),
              ),
            ],
          ),
        );

      case MdRule():
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: Divider(height: 1, color: theme.colorScheme.outlineVariant),
        );
    }
  }

  List<InlineSpan> _spans(
      BuildContext context, ThemeData theme, TextStyle base, List<MdInline> spans) {
    return [
      for (final s in spans)
        TextSpan(
          text: s.text,
          style: _style(theme, base, s),
          recognizer: s.link == null
              ? null
              : (TapGestureRecognizer()..onTap = () => _open(context, s.link!)),
        ),
    ];
  }

  TextStyle _style(ThemeData theme, TextStyle base, MdInline s) {
    var out = base;
    if (s.bold) out = out.copyWith(fontWeight: FontWeight.w700);
    if (s.italic) out = out.copyWith(fontStyle: FontStyle.italic);
    if (s.strike) out = out.copyWith(decoration: TextDecoration.lineThrough);
    if (s.code) {
      out = out.copyWith(
        fontFamily: 'Consolas',
        fontFamilyFallback: const ['Courier New', 'monospace'],
        backgroundColor: theme.colorScheme.surfaceContainerHighest,
      );
    }
    if (s.link != null) {
      out = out.copyWith(
        color: theme.colorScheme.primary,
        decoration: TextDecoration.underline,
        decorationColor: theme.colorScheme.primary,
      );
    }
    return out;
  }

  Future<void> _open(BuildContext context, String url) async {
    if (onLinkTap != null) return onLinkTap!(url);
    final messenger = ScaffoldMessenger.maybeOf(context);
    try {
      final ok = await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication);
      if (!ok) messenger?.showSnackBar(SnackBar(content: Text('打不开链接：$url')));
    } catch (e) {
      messenger?.showSnackBar(SnackBar(content: Text('打不开链接：$e')));
    }
  }
}
