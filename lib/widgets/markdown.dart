// 轻量 Markdown 解析（自研，零依赖）。
//
// 为什么不用 flutter_markdown：它已经停止维护，而且这里的需求很窄 ——
// 把模型流式吐出来的文本渲染成可读的富文本。自研就两个好处：
//   1. **流式安全**：还没闭合的 `**`、代码围栏、行内代码一律按字面量显示，
//      绝不"吞内容"。模型边生成边渲染时，这一点比功能多更重要；
//   2. 不引入依赖，Windows 离线构建的行为完全可控。
//
// 支持：标题 / 段落 / 无序列表 / 有序列表 / 引用 / 分隔线 /
//      围栏代码块 / 行内代码 / 粗体 / 斜体 / 删除线 / 链接 / 自动链接。
//
// 解析结果与 Flutter 无关，方便在纯单测里断言（test/markdown_test.dart）。

/// 行内片段：一段纯文本，或者一个带语义的区间。
class MdInline {
  const MdInline(
    this.text, {
    this.bold = false,
    this.italic = false,
    this.code = false,
    this.strike = false,
    this.link,
  });

  final String text;
  final bool bold;
  final bool italic;
  final bool code;
  final bool strike;

  /// 链接地址；非空表示这一段可点击。
  final String? link;

  /// 未传的字段沿用原值（注意 link 用"是否显式传入"判断，所以拆成两个方法）。
  MdInline withStyle({bool? bold, bool? italic, bool? code, bool? strike, String? link}) => MdInline(
        text,
        bold: bold ?? this.bold,
        italic: italic ?? this.italic,
        code: code ?? this.code,
        strike: strike ?? this.strike,
        link: link ?? this.link,
      );

  MdInline withText(String next) => MdInline(
        next,
        bold: bold,
        italic: italic,
        code: code,
        strike: strike,
        link: link,
      );

  @override
  String toString() =>
      'MdInline("$text"${bold ? ' b' : ''}${italic ? ' i' : ''}${code ? ' c' : ''}'
      '${strike ? ' s' : ''}${link != null ? ' ->$link' : ''})';

  @override
  bool operator ==(Object other) =>
      other is MdInline &&
      other.text == text &&
      other.bold == bold &&
      other.italic == italic &&
      other.code == code &&
      other.strike == strike &&
      other.link == link;

  @override
  int get hashCode => Object.hash(text, bold, italic, code, strike, link);
}

/// 块级元素。
sealed class MdBlock {
  const MdBlock();
}

/// 段落：行内片段已解析好，直接渲染。
class MdParagraph extends MdBlock {
  const MdParagraph(this.spans);
  final List<MdInline> spans;
}

class MdHeading extends MdBlock {
  const MdHeading(this.level, this.spans);

  /// 1~6
  final int level;
  final List<MdInline> spans;
}

/// 列表项：`ordered` 决定显示 "1." 还是 "•"。
class MdListItem extends MdBlock {
  const MdListItem(this.spans, {required this.ordered, this.index = 1});
  final List<MdInline> spans;
  final bool ordered;
  final int index;
}

class MdQuote extends MdBlock {
  const MdQuote(this.blocks);
  final List<MdBlock> blocks;
}

class MdCodeBlock extends MdBlock {
  const MdCodeBlock(this.code, {this.language});
  final String code;
  final String? language;
}

class MdRule extends MdBlock {
  const MdRule();
}

/// 表格列对齐（GFM 的 `:---` / `:---:` / `---:`）。
enum MdColumnAlign { none, left, center, right }

/// 表格：`header` 与每一行的单元格数都等于 `alignments.length`。
///
/// 单元格里的行内标记（粗体 / 行内代码 / 链接）照样解析 —— 助手回复里的
/// 表格经常是 `| \`list_skills\` | 列出已安装 **Skills** |`，不解析就是一坨反引号。
class MdTable extends MdBlock {
  const MdTable(this.alignments, this.header, this.rows);

  final List<MdColumnAlign> alignments;
  final List<List<MdInline>> header;
  final List<List<List<MdInline>>> rows;
}

/// 把 Markdown 文本解析成块列表。
///
/// **流式安全**：未闭合的围栏代码块会把"剩下的内容"当代码继续渲染；
/// 未闭合的行内标记（`**`、`*`、`` ` ``）一律按字面量显示，不会吞内容。
List<MdBlock> parseMarkdown(String source) {
  final lines = source.replaceAll('\r\n', '\n').replaceAll('\r', '\n').split('\n');
  final blocks = <MdBlock>[];
  final paragraph = <String>[];

  void flushParagraph() {
    if (paragraph.isEmpty) return;
    // 段落内部的单个换行按软换行处理
    blocks.add(MdParagraph(parseInline(paragraph.join('\n'))));
    paragraph.clear();
  }

  var i = 0;
  while (i < lines.length) {
    final line = lines[i];

    // --- 围栏代码块 ---------------------------------------------------
    final fence = _fenceOf(line);
    if (fence != null) {
      flushParagraph();
      final body = <String>[];
      // 注意：`RegExp.escape('```')` 里含有 `$`，走 `String.replaceAll` 会被当成
      // 替换引用而炸掉（返回空串）——这里只对单个字符做转义。
      final closePattern =
          RegExp(r'^\s{0,3}' + RegExp.escape(fence.marker[0]) + r'{3,}[ \t]*$');
      i++;
      var closed = false;
      while (i < lines.length) {
        if (closePattern.hasMatch(lines[i])) {
          closed = true;
          i++;
          break;
        }
        body.add(lines[i]);
        i++;
      }
      // 代码块内容不带尾部空行；未闭合时也保留已收到的内容
      while (body.isNotEmpty && _isBlank(body.last)) {
        body.removeLast();
      }
      // 未闭合（还在流式输出）也照样渲染成代码块
      blocks.add(MdCodeBlock(body.join('\n'), language: fence.language));
      if (!closed) break; // 后面的内容都属于这个还没结束的代码块
      continue;
    }

    // --- 标题 ---------------------------------------------------------
    final heading = RegExp(r'^\s{0,3}(#{1,6})\s+(.*)$').firstMatch(line);
    if (heading != null) {
      flushParagraph();
      blocks.add(MdHeading(heading.group(1)!.length, parseInline(heading.group(2)!.trim())));
      i++;
      continue;
    }

    // --- 分隔线 -------------------------------------------------------
    if (RegExp(r'^\s{0,3}([-*_])(\s*\1){2,}\s*$').hasMatch(line)) {
      flushParagraph();
      blocks.add(const MdRule());
      i++;
      continue;
    }

    // --- 引用 ---------------------------------------------------------
    if (RegExp(r'^\s{0,3}>').hasMatch(line)) {
      flushParagraph();
      final quoted = <String>[];
      while (i < lines.length && RegExp(r'^\s{0,3}>').hasMatch(lines[i])) {
        quoted.add(lines[i].replaceFirst(RegExp(r'^\s{0,3}>\s?'), ''));
        i++;
      }
      blocks.add(MdQuote(parseMarkdown(quoted.join('\n'))));
      continue;
    }

    // --- 列表 ---------------------------------------------------------
    final item = _listItemOf(line);
    if (item != null) {
      flushParagraph();
      var ordinal = item.ordered ? (int.tryParse(item.marker) ?? 1) : 1;
      while (i < lines.length) {
        final current = _listItemOf(lines[i]);
        if (current == null) {
          // 列表项里的悬挂续行（缩进 ≥ 2、且上一块是本列表的一项）
          if (!_isBlank(lines[i]) &&
              RegExp(r'^\s{2,}\S').hasMatch(lines[i]) &&
              blocks.isNotEmpty &&
              blocks.last is MdListItem) {
            final prev = blocks.removeLast() as MdListItem;
            blocks.add(MdListItem(
              [...prev.spans, const MdInline('\n'), ...parseInline(lines[i].trim())],
              ordered: prev.ordered,
              index: prev.index,
            ));
            i++;
            continue;
          }
          break;
        }
        blocks.add(MdListItem(
          parseInline(current.text),
          ordered: current.ordered,
          index: ordinal,
        ));
        if (current.ordered) ordinal++;
        i++;
      }
      continue;
    }

    // --- 表格（GFM）-----------------------------------------------------
    // 必须在"段落"之前：表格的头部行本身长得就像普通段落。
    // 判定要求**下一行是分隔行**（`|---|:--:|`），所以流式输出到一半
    // （只吐了表头、分隔行还没来）时仍按普通文本显示，不会误吞内容。
    final table = _tableAt(lines, i);
    if (table != null) {
      flushParagraph();
      blocks.add(table.block);
      i = table.next;
      continue;
    }

    // --- 空行 ---------------------------------------------------------
    if (_isBlank(line)) {
      flushParagraph();
      i++;
      continue;
    }

    paragraph.add(line);
    i++;
  }

  flushParagraph();
  return blocks;
}

bool _isBlank(String line) => line.trim().isEmpty;

// ---------------------------------------------------------------------------
//  表格
// ---------------------------------------------------------------------------

/// 从第 [i] 行开始识别一张表；不是表就返回 null。
///
/// 判定标准（GFM）：第 i 行含 `|`，第 i+1 行是**分隔行**且每个单元格都是 `:?-+:?`。
/// 单元格数一律以分隔行为准 —— 表头多写的列会被丢掉、少写的补空。
({MdTable block, int next})? _tableAt(List<String> lines, int i) {
  if (i + 1 >= lines.length) return null;
  final head = lines[i];
  if (!head.contains('|')) return null;
  final aligns = _alignmentsOf(lines[i + 1]);
  if (aligns == null) return null;

  final rows = <List<List<MdInline>>>[];
  var j = i + 2;
  while (j < lines.length) {
    final line = lines[j];
    if (_isBlank(line) || !line.contains('|')) break;
    // 又出现一行分隔行 = 另一张表开始了，停在这里
    if (_alignmentsOf(line) != null) break;
    rows.add(_cells(_splitRow(line), aligns.length));
    j++;
  }
  return (block: MdTable(aligns, _cells(_splitRow(head), aligns.length), rows), next: j);
}

/// 分隔行 → 每列的对齐方式；不是分隔行返回 null。
List<MdColumnAlign>? _alignmentsOf(String line) {
  if (!line.contains('-')) return null;
  final raw = _splitRow(line);
  if (raw.isEmpty) return null;
  final out = <MdColumnAlign>[];
  for (final cell in raw) {
    final c = cell.trim();
    if (!RegExp(r'^:?-+:?$').hasMatch(c)) return null;
    final left = c.startsWith(':');
    final right = c.endsWith(':');
    out.add(left && right
        ? MdColumnAlign.center
        : left
            ? MdColumnAlign.left
            : right
                ? MdColumnAlign.right
                : MdColumnAlign.none);
  }
  return out;
}

/// 按未转义的 `|` 切一行，并去掉首尾那两条边框竖线。
List<String> _splitRow(String line) {
  var s = line.trim();
  if (s.startsWith('|')) s = s.substring(1);
  if (s.endsWith('|') && !s.endsWith(r'\|')) s = s.substring(0, s.length - 1);
  final cells = <String>[];
  final buf = StringBuffer();
  for (var k = 0; k < s.length; k++) {
    final ch = s[k];
    if (ch == '\\' && k + 1 < s.length && s[k + 1] == '|') {
      buf.write('|'); // `\|` 是字面竖线，不是分列符
      k++;
      continue;
    }
    if (ch == '|') {
      cells.add(buf.toString().trim());
      buf.clear();
      continue;
    }
    buf.write(ch);
  }
  cells.add(buf.toString().trim());
  return cells;
}

/// 把 [raw] 规整成 [columns] 列（多的丢、少的补空），并解析行内标记。
List<List<MdInline>> _cells(List<String> raw, int columns) => [
      for (var k = 0; k < columns; k++)
        parseInline(k < raw.length ? raw[k] : ''),
    ];

class _Fence {
  const _Fence(this.marker, this.language);
  final String marker;
  final String? language;
}

_Fence? _fenceOf(String line) {
  // 结尾必须是行尾（允许尾随空格），不能把 "```\n" 这种"围栏后面还有内容"的
  // 情形当成开围栏 —— 否则开头的空行会让一个正常的代码块被当成未闭合。
  final m = RegExp(r'^\s{0,3}(`{3,}|~{3,})[ \t]*([A-Za-z0-9_+#.-]*)[ \t]*$').firstMatch(line);
  if (m == null) return null;
  final marker = m.group(1)!;
  final lang = m.group(2);
  return _Fence(marker[0] * 3, (lang == null || lang.isEmpty) ? null : lang);
}

class _ListItem {
  const _ListItem(this.text, {required this.ordered, required this.marker});
  final String text;
  final bool ordered;
  final String marker;
}

_ListItem? _listItemOf(String line) {
  final ordered = RegExp(r'^\s{0,6}(\d{1,9})[.)]\s+(.*)$').firstMatch(line);
  if (ordered != null) {
    return _ListItem(ordered.group(2)!, ordered: true, marker: ordered.group(1)!);
  }
  final bullet = RegExp(r'^\s{0,6}[-*+]\s+(.*)$').firstMatch(line);
  if (bullet != null) {
    return _ListItem(bullet.group(1)!, ordered: false, marker: '-');
  }
  return null;
}

// 行内标记：**粗** / __粗__ / *斜* / _斜_ / ~~删~~ / `代码` / [文案](url)
// 注意 `**` 要排在 `*` 前面匹配，否则 `**a**` 会被斜体规则先吃掉一半。
final _boldStar = RegExp(r'\*\*(?!\s)(.+?)(?<!\s)\*\*', dotAll: true);
// 下划线强调必须落在词边界上，否则 snake_case_name 会被误判（CommonMark 同款规则）
final _boldUnder = RegExp(r'(?<![\w])__(?![\s_])(.+?)(?<![\s_])__(?![\w])', dotAll: true);
final _strike = RegExp(r'~~(?!\s)(.+?)(?<!\s)~~', dotAll: true);
final _italicStar = RegExp(r'(?<!\*)\*(?!\*)(?!\s)([^*]+?)(?<!\s)\*(?!\*)');
final _italicUnder = RegExp(r'(?<![\w_])_(?![\s_])([^_]+?)(?<![\s_])_(?![\w_])');
final _codeSpan = RegExp(r'`([^`]+)`');
final _link = RegExp(r'!?\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)');
final _autoLink = RegExp(r'https?://[^\s<>()\[\]]*[^\s<>()\[\].,;:!?]');

/// 行内解析：**位置最靠前的标记先处理**（同一位置才按 代码 > 粗体 > 删除线 > 斜体 > 链接）。
///
/// 为什么不是"代码永远优先"：`**重点看 \`read_file\` 这一行**` 这种句子（助手回复里很常见）
/// 里 `**` 出现在行内代码**前面**，先切代码就会把 `**重点看 ` 当纯文本吐出来，
/// 结尾的 `**` 也成了孤零零的字面量 —— 用户看到的正是"粗体渲染不正常"。
List<MdInline> parseInline(String text) {
  if (text.isEmpty) return const [];
  final out = <MdInline>[];
  _scanInline(text, const MdInline(''), out);
  return out;
}

/// 找到**第一个**行内标记，切成 前 / 标记内 / 后 三段递归处理。
void _scanInline(String text, MdInline style, List<MdInline> out) {
  if (text.isEmpty) return;

  // 候选：位置最靠前者胜；位置相同按优先级（数字小的先处理）
  final candidates = <({int start, int priority, String kind, Match m})>[];
  void add(String kind, int priority, Match? m) {
    if (m != null) candidates.add((start: m.start, priority: priority, kind: kind, m: m));
  }

  add('code', 0, _codeSpan.firstMatch(text));
  add('bold', 1, _boldStar.firstMatch(text) ?? _boldUnder.firstMatch(text));
  add('strike', 2, _strike.firstMatch(text));
  add('italic', 3, _italicStar.firstMatch(text) ?? _italicUnder.firstMatch(text));
  add('link', 4, _link.firstMatch(text));
  // 裸链接：已经在链接标记内部时不再识别（否则会套娃）
  if (style.link == null) add('auto', 5, _autoLink.firstMatch(text));

  if (candidates.isEmpty) {
    out.add(style.withText(text));
    return;
  }
  candidates.sort((a, b) => a.start != b.start ? a.start - b.start : a.priority - b.priority);
  final pick = candidates.first;

  switch (pick.kind) {
    case 'code':
      final before = text.substring(0, pick.m.start);
      if (before.isNotEmpty) out.add(style.withText(before));
      out.add(style.withStyle(code: true).withText(pick.m.group(1)!));
      _scanInline(text.substring(pick.m.end), style, out);
    case 'bold':
      _emit(text, pick.m, style, out, style.withStyle(bold: true));
    case 'strike':
      _emit(text, pick.m, style, out, style.withStyle(strike: true));
    case 'italic':
      _emit(text, pick.m, style, out, style.withStyle(italic: true));
    case 'link':
      final before = text.substring(0, pick.m.start);
      if (before.isNotEmpty) out.add(style.withText(before));
      final label = pick.m.group(1)!.isEmpty ? pick.m.group(2)! : pick.m.group(1)!;
      out.add(MdInline(label,
          bold: style.bold,
          italic: style.italic,
          code: style.code,
          strike: style.strike,
          link: pick.m.group(2)));
      _scanInline(text.substring(pick.m.end), style, out);
    default: // auto
      // 末尾的中英文标点都不算地址的一部分（"见 https://a.example/x。" 这种很常见）
      final url = pick.m
          .group(0)!
          .replaceAll(RegExp(r'[.,;:!?)\]、。，；：！？）】」》]+$'), '');
      final before = text.substring(0, pick.m.start);
      if (before.isNotEmpty) out.add(style.withText(before));
      out.add(style.withStyle(link: url).withText(url));
      _scanInline(text.substring(pick.m.start + url.length), style, out);
  }
}

/// 标记内递归解析：`**a *b* c**` 里的斜体要生效。
void _emit(String text, Match m, MdInline style, List<MdInline> out, MdInline inner) {
  final before = text.substring(0, m.start);
  if (before.isNotEmpty) out.add(style.withText(before));
  _scanInline(m.group(1)!, inner, out);
  final after = text.substring(m.end);
  if (after.isNotEmpty) _scanInline(after, style, out);
}
