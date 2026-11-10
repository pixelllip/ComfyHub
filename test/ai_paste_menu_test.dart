import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/pages/ai_home_page.dart';

/// 把三个有代表性的内置项做成夹具：剪切 / 粘贴 / 全选。
List<ContextMenuButtonItem> _builtin({required VoidCallback onPaste}) => [
  ContextMenuButtonItem(type: ContextMenuButtonType.cut, onPressed: () {}),
  ContextMenuButtonItem(type: ContextMenuButtonType.paste, onPressed: onPaste),
  ContextMenuButtonItem(
    type: ContextMenuButtonType.selectAll,
    onPressed: () {},
  ),
];

void main() {
  // 现象："右键粘贴得到的是文件路径。"
  //
  // 真因：`Ctrl+V` 与右键「粘贴」是**两条路**。前者被我们拦下、读剪贴板里的
  // `CF_HDROP` 挂成附件；后者走 `EditableText` 内置的粘贴动作，**只读
  // `CF_UNICODETEXT`** —— 而 `FileClipboard.writeFiles` 为了让记事本之类也能粘，
  // 特意额外写了一份文本兜底，那份文本就是**路径串本身**。
  //
  // 所以这条纯函数是那件事的**唯一**防线：谁把"换掉右键粘贴项"改回去，
  // 界面上不会报任何错，只会安安静静地粘出一串 `D:\...`。用例就是为这个存在的。
  group('右键菜单的粘贴项', () {
    test('内置的「粘贴」被换成我们自己的，而且**位置不变**', () {
      var ours = 0;
      var builtin = 0;
      final items = withReplacedPaste(
        _builtin(onPaste: () => builtin++),
        () => ours++,
      );

      expect(items, hasLength(3), reason: '不许悄悄多出一项或少一项');
      expect(items.map((item) => item.type).toList(), [
        ContextMenuButtonType.cut,
        ContextMenuButtonType.paste,
        ContextMenuButtonType.selectAll,
      ], reason: '三个内置项的**顺序**要原样保留（用户看惯的位置不该跳）');

      items[1].onPressed!();
      expect(ours, 1, reason: '★ 点「粘贴」必须走我们那条（读文件 → 挂附件）');
      expect(builtin, 0, reason: '★ 内置那条**一次都不许**被调用，它就是粘路径的元凶');
    });

    test('内置项里本来没有「粘贴」时，补一个在末尾（别让用户没有粘贴入口）', () {
      var ours = 0;
      final items = withReplacedPaste([
        ContextMenuButtonItem(
          type: ContextMenuButtonType.copy,
          onPressed: () {},
        ),
      ], () => ours++);

      expect(items, hasLength(2));
      expect(items.last.label, '粘贴');
      items.last.onPressed!();
      expect(ours, 1);
    });

    test('只有「粘贴」一项时也要换掉（别原样放行）', () {
      var builtin = 0;
      final items = withReplacedPaste(
        _builtin(onPaste: () => builtin++).sublist(1, 2),
        () {},
      );

      expect(items, hasLength(1));
      items.single.onPressed!();
      expect(builtin, 0);
    });
  });
}
