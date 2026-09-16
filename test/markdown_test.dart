// Markdown 渲染回归：
//
//   1. 常见块级 / 行内标记要解析对（标题、列表、引用、代码块、粗体、链接）；
//   2. **流式安全**：模型吐到一半时（未闭合的 `**` / 代码围栏）不能吞内容，
//      否则界面会出现"字越写越少"的诡异现象。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/widgets/markdown.dart';
import 'package:viewer/widgets/markdown_view.dart';

/// 把块列表压成便于断言的字符串（行内片段只取文本 + 样式标记）。
String dump(List<MdBlock> blocks) => blocks.map((b) {
      String spans(List<MdInline> s) => s
          .map((x) =>
              '${x.bold ? 'b:' : ''}${x.italic ? 'i:' : ''}${x.code ? 'c:' : ''}'
              '${x.strike ? 's:' : ''}${x.link != null ? 'l:' : ''}${x.text}')
          .join(',');
      if (b is MdParagraph) return 'P(${spans(b.spans)})';
      if (b is MdHeading) return 'H${b.level}(${spans(b.spans)})';
      if (b is MdListItem) return 'LI${b.ordered ? '${b.index}.' : '•'}(${spans(b.spans)})';
      if (b is MdQuote) return 'Q[${dump(b.blocks)}]';
      if (b is MdCodeBlock) return 'CODE(${b.language ?? ''}){${b.code}}';
      if (b is MdRule) return 'RULE';
      if (b is MdTable) {
        String row(List<List<MdInline>> cells) => cells.map((c) => spans(c)).join('|');
        return 'TABLE[${b.alignments.map((a) => a.name).join(',')}]{'
            '${row(b.header)}${b.rows.isEmpty ? '' : ' :: ${b.rows.map(row).join(' :: ')}'}}';
      }
      return '$b';
    }).join(' | ');

void main() {
  test('粗体 / 斜体 / 删除线 / 行内代码', () {
    expect(parseInline('这是**粗体**结束'),
        [const MdInline('这是'), const MdInline('粗体', bold: true), const MdInline('结束')]);

    expect(parseInline('*斜体*'), [const MdInline('斜体', italic: true)]);
    expect(parseInline('~~删掉~~'), [const MdInline('删掉', strike: true)]);
    expect(parseInline('`a*b`'), [const MdInline('a*b', code: true)]);

    // 粗体里的斜体要生效
    final nested = parseInline('**a *b* c**');
    expect(nested.map((s) => s.text).join(), 'a b c');
    expect(nested.every((s) => s.bold), isTrue);
    expect(nested.firstWhere((s) => s.text == 'b').italic, isTrue);
  });

  test('链接：显式链接与裸链接', () {
    expect(parseInline('看[这里](https://a.example/x)吧'),
        [const MdInline('看'), const MdInline('这里', link: 'https://a.example/x'), const MdInline('吧')]);

    // 裸链接自动识别，末尾句号不算地址的一部分
    final auto = parseInline('见 https://a.example/x。');
    expect(auto.firstWhere((s) => s.link != null).text, 'https://a.example/x');
    expect(auto.last.text, '。');
  });

  test('块级：标题 / 列表 / 引用 / 分隔线', () {
    final blocks = parseMarkdown('''
# 标题

- 第一项
- 第二项

1. 甲
2. 乙

> 引用内容

---
''');
    expect(dump(blocks), contains('H1(标题)'));
    expect(dump(blocks), contains('LI•(第一项)'));
    expect(dump(blocks), contains('LI1.(甲)'));
    expect(dump(blocks), contains('LI2.(乙)'));
    expect(dump(blocks), contains('Q[P(引用内容)]'));
    expect(dump(blocks), contains('RULE'));
  });

  test('围栏代码块保留原文，不被行内规则拆开', () {
    final blocks = parseMarkdown('说明：\n\n```dart\nfinal a = **b**;\n```\n');
    expect(dump(blocks), contains('CODE(dart){final a = **b**;}'));
  });

  test('流式安全：未闭合的粗体按字面量显示，不吞内容', () {
    // 模型刚吐到 "**生" 的时候，界面必须还能看到 "**生"
    expect(parseInline('**生'), [const MdInline('**生')]);
    expect(parseInline('**生图'), [const MdInline('**生图')]);
    // 闭合之后才变成粗体
    expect(parseInline('**生图**'), [const MdInline('生图', bold: true)]);
  });

  test('流式安全：未闭合的代码围栏把余下内容当代码', () {
    final blocks = parseMarkdown('看这个：\n\n```python\nprint(1)\n');
    expect(dump(blocks), contains('CODE(python){print(1)}'));
  });

  test('裸星号与下划线不会被误判', () {
    // 3 * 4 = 12 里的星号不能变成斜体
    expect(parseInline('3 * 4 = 12').every((s) => !s.italic), isTrue);
    // snake_case 里的下划线不能变成斜体
    expect(parseInline('snake_case_name').every((s) => !s.italic), isTrue);
  });

  test('真实助手回复的渲染（截图里那段）', () {
    const reply = '''
你好！ 👋

我是 ComfyHub AI 工台的创作顾问，可以帮你完成以下工作：

- **生图** —— 根据文字描述生成图片
- **生视频** —— 制作动态视频内容
- **音乐创作** —— 生成配乐或音效
- **内容生产** —— 相关的提示词设计、方案拆解等

请问你今天想做什么创作？
''';
    final blocks = parseMarkdown(reply);
    final li = blocks.whereType<MdListItem>().toList();
    expect(li, hasLength(4));
    expect(li.first.spans.first.text, '生图');
    expect(li.first.spans.first.bold, isTrue);
    // 列表项里的破折号说明要跟着走，不能丢
    expect(li.first.spans.map((s) => s.text).join(), contains('根据文字描述生成图片'));
  });

  group('粗体里嵌行内代码（用户报的"粗体渲染不正常"）', () {
    test('粗体从行内代码**前面**开始时也要生效', () {
      final spans = parseInline('**Anima 生图体系（冲突以 `anima-prompt` 为准）**');
      // 以前这里会吐成 "**Anima…以 " + code + " 为准）**"，两头的 `**` 原样显示
      expect(spans.first.text, startsWith('Anima'));
      expect(spans.first.bold, isTrue);
      expect(spans.any((s) => s.text == '**'), isFalse, reason: '不能有裸露的 ** 字面量');
      final code = spans.firstWhere((s) => s.code);
      expect(code.text, 'anima-prompt');
      expect(code.bold, isTrue, reason: '代码片段还在粗体里面，样式要继承');
      expect(spans.last.text, ' 为准）');
      expect(spans.last.bold, isTrue, reason: '结尾的说明文字还在粗体里');
    });

    test('行内代码在粗体前面时，后面的粗体照样解析', () {
      final spans = parseInline('用 `read_file` 读，**只读**是默认');
      expect(spans.firstWhere((s) => s.code).text, 'read_file');
      expect(spans.firstWhere((s) => s.text == '只读').bold, isTrue);
    });

    test('标点夹在中间也成立（中文场景最常见）', () {
      final spans = parseInline('**重点：`a`、`b` 都要过**。');
      // 收尾的句号在 `**` 外面，按 Markdown 规则不该加粗；里面的部分全都要粗体
      expect(spans.last.text, '。');
      expect(spans.last.bold, isFalse);
      expect(spans.take(spans.length - 1).every((s) => s.bold), isTrue);
    });
  });

  group('表格（GFM）', () {
    test('表头 + 分隔行 + 数据行解析成 MdTable', () {
      final blocks = parseMarkdown('''
| 工具 | 说明 | 备注 |
|---|---|---|
| `list_skills` | 列出已安装 **Skills** | 只读 |
| `load_skill` | 按需加载 | 只读 |
''');
      final t = blocks.whereType<MdTable>().toList();
      expect(t, hasLength(1), reason: '不能退化成一堆带竖线的段落');
      expect(t.first.alignments, [MdColumnAlign.none, MdColumnAlign.none, MdColumnAlign.none]);
      expect(t.first.header.map((c) => c.first.text).toList(), ['工具', '说明', '备注']);
      expect(t.first.rows, hasLength(2));
      // 单元格里的行内标记照常解析
      expect(t.first.rows.first[0].first.code, isTrue);
      expect(t.first.rows.first[1].firstWhere((s) => s.text == 'Skills').bold, isTrue);
      expect(dump(blocks), contains('TABLE[', ), reason: 'dump 也要能表示表格');
    });

    test('对齐标记被识别', () {
      final blocks = parseMarkdown('| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n');
      final t = blocks.single as MdTable;
      expect(t.alignments, [MdColumnAlign.left, MdColumnAlign.center, MdColumnAlign.right]);
    });

    test('没有分隔行就不是表格（普通段落里的竖线不能变形）', () {
      final blocks = parseMarkdown('这里 A | B 只是普通文字\n第二行也一样\n');
      expect(blocks.whereType<MdTable>(), isEmpty);
      expect(blocks.single, isA<MdParagraph>());
    });

    test('流式安全：只吐了表头、分隔行还没来 → 仍然按段落显示', () {
      final blocks = parseMarkdown('| 工具 | 说明 |\n');
      expect(blocks.whereType<MdTable>(), isEmpty);
      expect(blocks.single, isA<MdParagraph>());
    });

    test('列数不齐的表格不会崩：以分隔行为准，多的丢、少的补空', () {
      final blocks = parseMarkdown('| a | b |\n|---|---|\n| 1 | 2 | 3 |\n| 9 |\n');
      final t = blocks.single as MdTable;
      expect(t.header, hasLength(2));
      expect(t.rows.first, hasLength(2));
      expect(t.rows.last.map((c) => c.map((s) => s.text).join()).toList(), ['9', '']);
    });

    test('表格后面紧跟的正文不会被吃进表格', () {
      final blocks = parseMarkdown('| a |\n|---|\n| 1 |\n\n后面这段话是正文\n');
      expect(blocks.whereType<MdTable>(), hasLength(1));
      final p = blocks.whereType<MdParagraph>().toList();
      expect(p.map((x) => x.spans.map((s) => s.text).join()).join(), contains('后面这段话是正文'));
    });
  });

  group('真正渲染出来（气泡里的宽度约束下不能崩）', () {
    testWidgets('表格渲染成 Table，单元格里的行内代码不再是反引号', (tester) async {
      // 助手回复的真实形态：表格 + 单元格里的 `工具名` 与 **粗体**
      const reply = '''
**Skill 类**
| 工具 | 说明 | 备注 |
|---|---|---|
| `list_skills` | 列出已安装 **Skills** | 只读 |
| `load_skill` | 按需加载 | 只读 |

看完这张表就知道有哪些工具了。
''';
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 560, // 聊天气泡的实际可用宽度量级
              child: SingleChildScrollView(child: MarkdownText(reply)),
            ),
          ),
        ),
      ));
      await tester.pumpAndSettle();

      expect(tester.takeException(), isNull);
      expect(find.byType(Table), findsOneWidget);
      expect(find.textContaining('list_skills', findRichText: true), findsOneWidget);
      expect(find.textContaining('`', findRichText: true), findsNothing, reason: '反引号不该出现在界面上');
      expect(find.textContaining('**', findRichText: true), findsNothing, reason: '星号不该出现在界面上');
    });

    testWidgets('宽 4 列的表格在窄气泡里也不会溢出', (tester) async {
      const reply = '| 一 | 二 | 三 | 四 |\n|---|---|---|---|\n'
          '| 很长的中文内容会自己换行 | 内容 | 内容 | 内容 |\n';
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: Center(child: SizedBox(width: 320, child: MarkdownText(reply))),
        ),
      ));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: 'RenderFlex overflow 会在这里冒出来');
    });
  });
}
