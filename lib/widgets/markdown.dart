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

/// 行内解析：代码 → 粗体 → 删除线 → 斜体 → 链接 → 自动链接。
List<MdInline> parseInline(String text) {
  if (text.isEmpty) return const [];
  final out = <MdInline>[];
  _scanInline(text, const MdInline(''), out);
  return out;
}

/// 找到**第一个**行内标记，切成 前 / 标记内 / 后 三段递归处理。
void _scanInline(String text, MdInline style, List<MdInline> out) {
  if (text.isEmpty) return;

  // 代码优先级最高：里面的 `*` `_` 都是字面量
  final code = _codeSpan.firstMatch(text);
  if (code != null) {
    final before = text.substring(0, code.start);
    if (before.isNotEmpty) out.add(style.withText(before));
    out.add(style.withStyle(code: true).withText(code.group(1)!));
    final after = text.substring(code.end);
    if (after.isNotEmpty) _scanInline(after, style, out);
    return;
  }

  final bold = _boldStar.firstMatch(text) ?? _boldUnder.firstMatch(text);
  if (bold != null) {
    _emit(text, bold, style, out, style.withStyle(bold: true));
    return;
  }

  final strike = _strike.firstMatch(text);
  if (strike != null) {
    _emit(text, strike, style, out, style.withStyle(strike: true));
    return;
  }

  final italic = _italicStar.firstMatch(text) ?? _italicUnder.firstMatch(text);
  if (italic != null) {
    _emit(text, italic, style, out, style.withStyle(italic: true));
    return;
  }

  // [文案](url)
  final link = _link.firstMatch(text);
  if (link != null) {
    final before = text.substring(0, link.start);
    if (before.isNotEmpty) out.add(style.withText(before));
    final label = link.group(1)!.isEmpty ? link.group(2)! : link.group(1)!;
    out.add(MdInline(label,
        bold: style.bold, italic: style.italic, code: style.code, strike: style.strike,
        link: link.group(2)));
    final after = text.substring(link.end);
    if (after.isNotEmpty) _scanInline(after, style, out);
    return;
  }

  // 裸链接自动识别（末尾的句号 / 括号不算地址的一部分）
  if (style.link == null) {
    final auto = _autoLink.firstMatch(text);
    if (auto != null) {
      // 末尾的中英文标点都不算地址的一部分（"见 https://a.example/x。" 这种很常见）
      final url = auto.group(0)!.replaceAll(RegExp(r'[.,;:!?)\]、。，；：！？）】」》]+$'), '');
      final before = text.substring(0, auto.start);
      if (before.isNotEmpty) out.add(style.withText(before));
      out.add(style.withStyle(link: url).withText(url));
      final after = text.substring(auto.start + url.length);
      if (after.isNotEmpty) _scanInline(after, style, out);
      return;
    }
  }

  out.add(style.withText(text));
}

/// 标记内递归解析：`**a *b* c**` 里的斜体要生效。
void _emit(String text, Match m, MdInline style, List<MdInline> out, MdInline inner) {
  final before = text.substring(0, m.start);
  if (before.isNotEmpty) out.add(style.withText(before));
  _scanInline(m.group(1)!, inner, out);
  final after = text.substring(m.end);
  if (after.isNotEmpty) _scanInline(after, style, out);
}
