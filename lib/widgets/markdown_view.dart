// 把模型返回的 Markdown 渲染成富文本（自研，见 markdown.dart 的解析部分）。
//
// 设计取舍：
//   · **流式友好**：每次 setState 都重新解析整段文本。解析是纯函数、量级很小
//     （聊天回复单条最多几 KB），换来的好处是"半截文本也能正确显示"；
//   · **只读**：不提供编辑；
//   · **可跨段选择**：一段回复会被解析为多个块
//     （段落 / 标题 / 列表项 / 表格单元格 / 代码块），而 `SelectableText` 是**每块一个选择域** ——
//     结果就是鼠标拖到这一段的末尾就再也拉不过去了（短段落正好三行）。
//     现在整块正文包在一个 `SelectionArea` 里，内部用普通的 `Text.rich`，
//     于是选择可以跨段落、跨表格连续拉下去（外层气泡还会把它和思考 / 工具卡合并成一个域）；
//   · 链接可点，走系统浏览器（url_launcher），失败只提示不抛。

import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
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
        return _CodeBlock(
          code: code,
          language: language,
          theme: theme,
          base: base,
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
        // 行内代码也保留透明度：纯色底色会覆盖选中高亮。
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
      if (!ok) messenger?.showSnackBar(SnackBar(content: Text('打不开链接，$url')));
    } catch (e) {
      messenger?.showSnackBar(SnackBar(content: Text('打不开链接，$e')));
    }
  }
}

/// 一张代码块：`[类型]` + 一键复制 一行，分隔线，正文。
///
/// 类型行（含「复制」按钮）包在 [SelectionContainer.disabled] 里：整条回复全选复制时，
/// 不该把「复制」两个字也带进剪贴板。
class _CodeBlock extends StatefulWidget {
  const _CodeBlock({
    required this.code,
    required this.language,
    required this.theme,
    required this.base,
  });

  final String code;
  final String? language;
  final ThemeData theme;
  final TextStyle base;

  @override
  State<_CodeBlock> createState() => _CodeBlockState();
}

class _CodeBlockState extends State<_CodeBlock> {
  bool _copied = false;
  Timer? _resetTimer;

  /// 横向滚动条要靠它（`thumbVisibility` 需要一个自己的 controller）。
  final ScrollController _hScroll = ScrollController();

  /// 上一次算好的正文（按 code + language 缓存）：流式期间 code 每帧都变，
  /// 但「复制成功」之类的 setState 不该把同一段正文再并一遍。
  String? _bodyCache;
  String? _bodyCacheCode;
  String? _bodyCacheLanguage;

  /// 展示 / 复制用的正文：模型硬折行的散文先并回段落，真代码原样。
  String get _body {
    if (_bodyCache != null &&
        _bodyCacheCode == widget.code &&
        _bodyCacheLanguage == widget.language) {
      return _bodyCache!;
    }
    final body =
        reflowWrappedProse(widget.code, widget.language) ?? widget.code;
    _bodyCacheCode = widget.code;
    _bodyCacheLanguage = widget.language;
    _bodyCache = body;
    return body;
  }

  /// 类型标签：代码块的语言标记；没有就写「文本」。
  String get _typeLabel {
    final lang = (widget.language ?? '').trim();
    return lang.isEmpty ? '文本' : lang;
  }

  @override
  void dispose() {
    _resetTimer?.cancel();
    _hScroll.dispose();
    super.dispose();
  }

  Future<void> _copy() async {
    await Clipboard.setData(ClipboardData(text: _body));
    if (!mounted) return;
    setState(() => _copied = true);
    // 只在原位闪一下「已复制」：不弹 SnackBar，免得每复制一次都挡一下正文。
    // 用可取消的 Timer（不是 Future.delayed）：代码块随流式文本反复重建 / 被滚出
    // 屏幕时，不能留一个 1.4 秒后还在跑的定时器。
    _resetTimer?.cancel();
    _resetTimer = Timer(const Duration(milliseconds: 1400), () {
      if (mounted) setState(() => _copied = false);
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = widget.theme;
    final base = widget.base;
    final body = _body;
    final reflowed = body != widget.code;
    // 单行代码 / 硬折行散文 / 纯文本块：软换行（超过容器宽度就折）；
    // 真·多行代码：横向滚动（缩进 / 对齐本身有意义，自动折行反而更难读），
    // 并且**常驻横向滚动条** —— 否则无法判断右侧还有内容。
    final softWrap = codeBlockWraps(
      widget.code,
      widget.language,
      reflowed: reflowed,
    );
    // ⚠ 代码块底色必须是**半透明**、而且要把 `selectionColor` 显式交给
    // `DefaultSelectionStyle`：外层 `SelectionArea` 的高亮由每个 `RenderParagraph`
    // 自己画，纯色块 + 主题里没给选择色时，选中区域会和底色糊在一起，看起来像"没选中"。
    final codeText = DefaultSelectionStyle(
      selectionColor: theme.colorScheme.primary.withValues(alpha: 0.45),
      child: Text(
        body,
        style: base.copyWith(
          fontFamily: 'Consolas',
          fontFamilyFallback: const ['Courier New', 'monospace'],
          fontSize: (base.fontSize ?? 14) - 0.5,
          height: 1.4,
        ),
        softWrap: softWrap,
      ),
    );
    const codePadding = EdgeInsets.fromLTRB(10, 6, 10, 8);
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.symmetric(vertical: 6),
      clipBehavior: Clip.antiAlias,
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
          SelectionContainer.disabled(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(10, 2, 4, 2),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      _typeLabel,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                        fontFamily: 'Consolas',
                      ),
                    ),
                  ),
                  TextButton.icon(
                    onPressed: _copy,
                    icon: Icon(_copied ? Icons.check : Icons.copy, size: 14),
                    label: Text(_copied ? '已复制' : '复制'),
                    style: TextButton.styleFrom(
                      visualDensity: VisualDensity.compact,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      minimumSize: const Size(0, 32),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      textStyle: theme.textTheme.labelSmall,
                      foregroundColor: _copied
                          ? theme.colorScheme.primary
                          : theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ),
          Divider(
            height: 1,
            thickness: 1,
            color: theme.colorScheme.outlineVariant.withValues(alpha: 0.5),
          ),
          // 单行 / 折行散文 / 纯文本：直接放进有界宽度里（软换行，必要时连超长单词也会被拆开）；
          // 多行代码：横向滚动，保持原始排版，并配一条看得见的滚动条。
          if (softWrap)
            Padding(padding: codePadding, child: codeText)
          else
            Scrollbar(
              controller: _hScroll,
              // 常驻：**可看见**右侧还有内容（默认仅在滚动时短暂显示，桌面端容易错过）。
              // 内容不超宽时 Flutter 自己不会画（`maxScrollExtent == 0`），不会白占一条。
              thumbVisibility: true,
              scrollbarOrientation: ScrollbarOrientation.bottom,
              child: SingleChildScrollView(
                controller: _hScroll,
                scrollDirection: Axis.horizontal,
                // 底部留出滚动条的位置，别让它压住最后一行
                padding: const EdgeInsets.fromLTRB(10, 6, 10, 16),
                child: codeText,
              ),
            ),
        ],
      ),
    );
  }
}

/// 这张表是不是"超宽"——决定用固有列宽 + 横向滚动，还是弹性列宽 + 换行。
///
/// 判据刻意只看**内容规模**（列数 / 单元格字符数），不看实际像素：
/// 像素要等布局之后才知道，而这里必须在建表之前选分支。阈值取得比较保守，
/// 普通 3~4 列说明表不会误判成滚动表；**两列一律不滚动**（见下面的分支）。
bool _tableLooksWide(
  List<MdColumnAlign> alignments,
  List<List<MdInline>> header,
  List<List<List<MdInline>>> rows,
) {
  // **两列表格始终铺满容器、自动换行**（优先铺满对话容器，
  // 而不是向右延伸）。两列的表大多是"参数 → 说明"这种说明表，横向拖读很难受。
  if (alignments.length <= 2) return false;
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
            // 常驻：**可看见**此处可以横向滚动（默认仅在滚动时短暂显示，桌面端容易错过）。
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
