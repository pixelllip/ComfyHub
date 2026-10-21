// 把模型返回的 Markdown 渲染成富文本（自研，见 markdown.dart 的解析部分）。
//
// 设计取舍：
//   · **流式友好**：每次 setState 都重新解析整段文本。解析是纯函数、量级很小
//     （聊天回复单条最多几 KB），换来的好处是"半截文本也能正确显示"；
//   · **只读**：不提供编辑；
//   · **可跨段选择**（用户 bug ②："仅能选择 3 行文本"）：一段回复会被解析成很多个块
//     （段落 / 标题 / 列表项 / 表格单元格 / 代码块），而 `SelectableText` 是**每块一个选择域** ——
//     结果就是鼠标拖到这一段的末尾就再也拉不过去了（短段落正好三行）。
//     现在整块正文包在一个 `SelectionArea` 里，内部用普通的 `Text.rich`，
//     于是选择可以跨段落、跨表格连续拉下去（外层气泡还会把它和思考 / 工具卡合并成一个域）；
//   · 链接可点，走系统浏览器（url_launcher），失败只提示不抛。

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import 'markdown.dart';

/// 渲染一段 Markdown。文本为空时返回空盒子（调用方自己决定占位）。
class MarkdownText extends StatelessWidget {
  const MarkdownText(
    this.source, {
    super.key,
    this.style,
    this.onLinkTap,
    this.selectable = true,
  });

  final String source;

  /// 正文基础样式；标题 / 代码块在此基础上放大或换色。
  final TextStyle? style;

  /// 链接点击回调；默认用系统默认程序打开。
  final Future<void> Function(String url)? onLinkTap;

  /// 是否自己包一层 [SelectionArea]。
  ///
  /// 调用方**已经**在更外层包了 `SelectionArea`（聊天气泡就是：它要把思考段、正文、
  /// 工具卡连成一个选择域）时必须传 false —— 嵌套的选择域会让拖选又只能停在当前这一块里。
  final bool selectable;

  @override
  Widget build(BuildContext context) {
    if (source.trim().isEmpty) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final base =
        style ?? theme.textTheme.bodyMedium ?? const TextStyle(fontSize: 14);
    final blocks = parseMarkdown(source);
    final column = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [for (final b in blocks) _block(context, theme, base, b)],
    );
    return selectable ? SelectionArea(child: column) : column;
  }

  Widget _block(
    BuildContext context,
    ThemeData theme,
    TextStyle base,
    MdBlock block,
  ) {
    switch (block) {
      case MdParagraph(:final spans):
        return Text.rich(
          TextSpan(children: _spans(context, theme, base, spans)),
          style: base,
        );

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
          child: Text.rich(
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
                child: Text(
                  marker,
                  style: base.copyWith(color: theme.colorScheme.outline),
                ),
              ),
              Expanded(
                child: Text.rich(
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
              left: BorderSide(
                color: theme.colorScheme.outlineVariant,
                width: 3,
              ),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              for (final inner in blocks)
                DefaultTextStyle.merge(
                  style: base.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                  child: _block(
                    context,
                    theme,
                    base.copyWith(color: theme.colorScheme.onSurfaceVariant),
                    inner,
                  ),
                ),
            ],
          ),
        );

      case MdCodeBlock(:final code, :final language):
        // 单行代码块（模型经常把一条命令 / 一段 JSON 整段塞进 ``` 里且不换行）：
        // 横向滚动在这种场景下等于"永远看不全" —— 用户要求**超过容器宽度就换行显示**。
        // 多行代码块**保持横向滚动**：代码的换行位置本身有意义（缩进 / 对齐 / 表格状输出），
        // 自动折行反而更难读。
        final singleLine = !code.contains('\n');
        // ⚠ 代码块底色必须是**半透明**、而且要把 `selectionColor` 显式交给
        // `DefaultSelectionStyle`：外层 `SelectionArea` 的高亮由每个 `RenderParagraph`
        // 自己画，纯色块 + 主题里没给选择色时，选中区域会和底色糊在一起，看起来像"没选中"。
        final codeText = DefaultSelectionStyle(
          selectionColor: theme.colorScheme.primary.withValues(alpha: 0.45),
          child: Text(
            code,
            style: base.copyWith(
              fontFamily: 'Consolas',
              fontFamilyFallback: const ['Courier New', 'monospace'],
              fontSize: (base.fontSize ?? 14) - 0.5,
              height: 1.4,
            ),
            softWrap: singleLine,
          ),
        );
        return Container(
          width: double.infinity,
          margin: const EdgeInsets.symmetric(vertical: 6),
          decoration: BoxDecoration(
            // 0.7 → 0.42：留出透光率，选择高亮即使被画在底色下面也看得见。
            color: theme.colorScheme.surfaceContainerHighest.withValues(
              alpha: 0.42,
            ),
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: theme.colorScheme.outlineVariant.withValues(alpha: 0.5),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (language != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(10, 6, 10, 0),
                  child: Text(
                    language,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
              // 单行：直接放进有界宽度里（软换行，必要时连超长单词也会被拆开）；
              // 多行：横向滚动，保持原始排版
              if (singleLine)
                Padding(
                  padding: const EdgeInsets.fromLTRB(10, 6, 10, 8),
                  child: codeText,
                )
              else
                SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  padding: const EdgeInsets.fromLTRB(10, 6, 10, 8),
                  child: codeText,
                ),
            ],
          ),
        );

      case MdTable(:final alignments, :final header, :final rows):
        // 普通表格仍用弹性列宽（长文本换行，读起来最舒服）；
        // **超宽表格**（列多 / 单元格长）改成固有列宽 + 横向滚动，并带一条常驻滚动条 ——
        // 否则只有 5 列的表会把每一列压成"一个字一行"，根本没法读。
        final wide = _tableLooksWide(alignments, header, rows);
        return _MarkdownTable(
          alignments: alignments,
          header: header,
          rows: rows,
          theme: theme,
          base: base,
          scrollable: wide,
          cellBuilder: (spans, align, style) =>
              _cell(context, theme, style, spans, align),
        );

      case MdRule():
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: Divider(height: 1, color: theme.colorScheme.outlineVariant),
        );
    }
  }

  /// 表格的一个单元格（内容照常解析行内标记，可选中复制）。
  Widget _cell(
    BuildContext context,
    ThemeData theme,
    TextStyle style,
    List<MdInline> spans,
    MdColumnAlign align,
  ) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      child: Text.rich(
        TextSpan(children: _spans(context, theme, style, spans)),
        style: style,
        textAlign: switch (align) {
          MdColumnAlign.center => TextAlign.center,
          MdColumnAlign.right => TextAlign.right,
          _ => TextAlign.left,
        },
      ),
    );
  }

  List<InlineSpan> _spans(
    BuildContext context,
    ThemeData theme,
    TextStyle base,
    List<MdInline> spans,
  ) {
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
        // 行内代码也留透光率：纯色底色会把选中高亮盖住（用户 2026-10-21 的反馈）。
        backgroundColor: theme.colorScheme.surfaceContainerHighest.withValues(
          alpha: 0.65,
        ),
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
      final ok = await launchUrl(
        Uri.parse(url),
        mode: LaunchMode.externalApplication,
      );
      if (!ok) messenger?.showSnackBar(SnackBar(content: Text('打不开链接：$url')));
    } catch (e) {
      messenger?.showSnackBar(SnackBar(content: Text('打不开链接：$e')));
    }
  }
}

/// 这张表是不是"超宽"——决定用固有列宽 + 横向滚动，还是弹性列宽 + 换行。
///
/// 判据刻意只看**内容规模**（列数 / 单元格字符数），不看实际像素：
/// 像素要等布局之后才知道，而这里必须在建表之前选分支。阈值取得比较保守，
/// 普通 3~4 列说明表不会误判成滚动表。
bool _tableLooksWide(
  List<MdColumnAlign> alignments,
  List<List<MdInline>> header,
  List<List<List<MdInline>>> rows,
) {
  if (alignments.length >= 6) return true;
  var maxCell = 0;
  var total = 0;
  void scan(List<List<MdInline>> row) {
    for (final cell in row) {
      final length = cell.fold<int>(0, (sum, span) => sum + span.text.length);
      if (length > maxCell) maxCell = length;
      total += length;
    }
  }

  scan(header);
  for (final row in rows) {
    scan(row);
  }
  return maxCell >= 48 || total >= 280;
}

/// 表格：普通表用 `Table` 的弹性列宽；超宽表包一层横向 `Scrollbar`。
///
/// 为什么不做"永远横向滚动"：ComfyUI 相关的回复里 3 列小表很常见，
/// 永远滚动会让它们右边空一大片、左边还要拖，反而难读。
class _MarkdownTable extends StatefulWidget {
  final List<MdColumnAlign> alignments;
  final List<List<MdInline>> header;
  final List<List<List<MdInline>>> rows;
  final ThemeData theme;
  final TextStyle base;
  final bool scrollable;
  final Widget Function(List<MdInline>, MdColumnAlign, TextStyle) cellBuilder;

  const _MarkdownTable({
    required this.alignments,
    required this.header,
    required this.rows,
    required this.theme,
    required this.base,
    required this.scrollable,
    required this.cellBuilder,
  });

  @override
  State<_MarkdownTable> createState() => _MarkdownTableState();
}

class _MarkdownTableState extends State<_MarkdownTable> {
  final ScrollController _scroll = ScrollController();

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = widget.theme;
    final table = Table(
      // 超宽表：列按内容宽度，不折行（折了就等于把代码/参数名劈开）；
      // 普通表：列宽均分，长文本正常换行。
      defaultColumnWidth: widget.scrollable
          ? const IntrinsicColumnWidth()
          : const FlexColumnWidth(),
      defaultVerticalAlignment: TableCellVerticalAlignment.top,
      border: TableBorder.symmetric(
        inside: BorderSide(color: theme.colorScheme.outlineVariant, width: 0.8),
        outside: BorderSide(color: theme.colorScheme.outlineVariant),
      ),
      children: [
        TableRow(
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainerHighest.withValues(
              alpha: 0.7,
            ),
          ),
          children: [
            for (var c = 0; c < widget.alignments.length; c++)
              widget.cellBuilder(
                widget.header[c],
                widget.alignments[c],
                widget.base.copyWith(fontWeight: FontWeight.w600),
              ),
          ],
        ),
        for (final row in widget.rows)
          TableRow(
            children: [
              for (var c = 0; c < widget.alignments.length; c++)
                widget.cellBuilder(row[c], widget.alignments[c], widget.base),
            ],
          ),
      ],
    );

    final clipped = ClipRRect(
      borderRadius: BorderRadius.circular(8),
      child: table,
    );
    if (!widget.scrollable) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: clipped,
      );
    }

    return Padding(
      padding: const EdgeInsets.only(top: 6, bottom: 10),
      child: LayoutBuilder(
        builder: (context, constraints) {
          return Scrollbar(
            controller: _scroll,
            // 常驻：用户**看得见**这里可以横向滚动（默认只在滚动时闪一下，桌面端很容易错过）。
            thumbVisibility: true,
            scrollbarOrientation: ScrollbarOrientation.bottom,
            child: SingleChildScrollView(
              controller: _scroll,
              scrollDirection: Axis.horizontal,
              child: ConstrainedBox(
                constraints: BoxConstraints(minWidth: constraints.maxWidth),
                child: clipped,
              ),
            ),
          );
        },
      ),
    );
  }
}
