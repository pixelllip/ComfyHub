import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../core/workflow_blueprint.dart';

/// 一次绘制实际绘制的元素数量（**供测试用例使用**，不面向界面）。
///
/// 仓库约定：界面类改动不写 golden / 不断言像素，而是把"画了什么"变成可断言的量
/// （见 `docs/archive/workflow-blueprint-research.md` §5 的 P2 验收）。视口剔除是性能的
/// 唯一防线，所以它必须有回归。
class BlueprintPaintStats {
  /// 这一帧画到的节点数
  final int drawnNodes;

  /// 这一帧跳过的节点数（视口之外）
  final int culledNodes;
  final int drawnEdges;
  final int culledEdges;

  const BlueprintPaintStats({
    this.drawnNodes = 0,
    this.culledNodes = 0,
    this.drawnEdges = 0,
    this.culledEdges = 0,
  });

  @override
  String toString() =>
      'BlueprintPaintStats(drawn=$drawnNodes, culled=$culledNodes, edges=$drawnEdges)';
}

/// WCAG 相对亮度（0 = 黑，1 = 白）。
///
/// 不只给测试用：`BlueprintPalette.isDark` 就靠它决定分组底板的 alpha 该往哪边调。
/// 顺带，调色板"刺不刺眼"因此能**算**出来 —— 仓库不做 golden / 截图回归
/// （`docs/workflow-blueprint-design.md` §6），"观感"只能变成可断言的量。
double relativeLuminance(Color color) {
  double channel(double c) =>
      c <= 0.03928 ? c / 12.92 : math.pow((c + 0.055) / 1.055, 2.4).toDouble();
  return 0.2126 * channel(color.r) +
      0.7152 * channel(color.g) +
      0.0722 * channel(color.b);
}

/// WCAG 对比度（1:1 ~ 21:1）。
@visibleForTesting
double contrastRatio(Color a, Color b) {
  final la = relativeLuminance(a);
  final lb = relativeLuminance(b);
  final hi = math.max(la, lb);
  final lo = math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

/// 蓝图配色。**两套主题各一份，均需成立**。
///
/// # 用途
///
/// 已知问题："蓝图对比度过高。" 原因是画布把**一整套浅色**写死在
/// 画笔中（节点底 `#F3F5F7` + 近黑正文），而 App **默认深色主题** ——
/// 深底上覆盖近白的大色块，对比度约 17:1，高于 WCAG AAA 的 7:1。
/// 切换到浅色主题时，节点底与浅色画布接近同色、分组底板 `alpha 0.06` 也不可见。
/// 即该组颜色**只对其未适配的主题成立**，两种主题均不正确。
///
/// 现在的规矩两条：
/// 1. **节点底跟画布同一侧**（深色主题下也是深色），只靠**一档明度差 + 一圈描边**分层
///    （AGENTS §6.1：层次用色彩表达，不用 elevation / 阴影）。
/// 2. `kind` 的语义色**只上边框**，不再整块铺底、也不整条染标题 —— 这是 ComfyUI 的观感来源
///    （标题条是中性灰），也是"大面积高对比"被消掉的直接原因。
class BlueprintPalette {
  /// 画布底色。**画布必须有自己的底色**，别再借 `Scaffold` 的 surface
  /// （借了就没法保证"节点底与画布底同侧"这件事）。
  final Color canvas;

  /// 网格线（很淡，只帮着看清平移了多少）。
  final Color grid;

  /// 普通节点底色。
  final Color body;

  /// 普通节点描边。
  final Color border;

  /// 节点标题条底色。
  ///
  /// ComfyUI 的标题条是**中性灰**（不是按节点类别上色）：截图实测深色主题下
  /// 标题条约 `#3E3E3E`、节点底 `#313131`、画布 `#282828`。语义色只留在描边上
  /// —— 整条标题按类别染成蓝 / 紫 / 绿反而比 ComfyUI 更花。
  final Color header;

  /// 节点标题文字。
  final Color title;

  /// 节点类名（次要文字）。
  final Color subtitle;

  /// 插口名文字。
  final Color portLabel;

  /// 参数行的小格子底。
  final Color widgetBar;

  /// 参数行的文字。
  final Color widgetText;

  /// `kind` 的语义强调色（**只用在边框上**）。
  final Map<String, Color> accents;

  /// ComfyUI 的**插口 / 连线**类型色（真源：ComfyUI 前端 `link_type_colors`）。
  ///
  /// 不同输入/输出的节点与连线配色取自 ComfyUI。
  /// 深色一套直接取 ComfyUI 的色值；浅色一套保持同一个色相、压暗明度 ——
  /// 把 `#FFD500` 这种高亮黄原样铺在浅色画布上等于看不见。
  final Map<String, Color> typeColors;

  /// 认不出的类型用中性色（ComfyUI 的 `default_link_color`）。
  final Color unknownType;

  const BlueprintPalette({
    required this.canvas,
    required this.grid,
    required this.body,
    required this.border,
    required this.header,
    required this.title,
    required this.subtitle,
    required this.portLabel,
    required this.widgetBar,
    required this.widgetText,
    required this.accents,
    required this.typeColors,
    required this.unknownType,
  });

  /// 某个插口 / 连线的颜色。
  ///
  /// 认不出的类型**按名字稳定地散列**成一种颜色，而不是一律灰：
  /// ComfyUI 里自定义节点的类型（`SEGS` / `PIPE_LINE` / `COMFY_DYNAMICCOMBO_V3` …）
  /// 非常多，全画成灰色就回到了"所有线一个样"。
  Color typeColor(String? type) {
    final key = (type ?? '').trim().toUpperCase();
    if (key.isEmpty) return unknownType;
    final known = typeColors[key];
    if (known != null) return known;
    var hash = 0;
    for (final unit in key.codeUnits) {
      hash = (hash * 31 + unit) & 0x7fffffff;
    }
    return HSLColor.fromAHSL(
      1,
      (hash % 360).toDouble(),
      isDark ? 0.55 : 0.62,
      isDark ? 0.68 : 0.40,
    ).toColor();
  }

  static BlueprintPalette of(Brightness brightness) =>
      brightness == Brightness.dark ? dark : light;

  /// 画布底色是不是深色一侧 —— 用它决定分组底板的 alpha 该往哪边调
  /// （深色下 0.06 的底色等于没有，浅色下 0.3 又会压住内容）。
  bool get isDark => relativeLuminance(canvas) < 0.5;

  /// 这一类节点的强调色（认不出来回普通描边色 —— **不猜**）。
  Color accentOf(String kind) => accents[kind] ?? border;

  /// 深色：一切都在深色一侧，靠 `body` 比 `canvas` 亮一档来分层。
  ///
  /// 灰阶取自 ComfyUI（Obsidian 主题）实测：画布 `#282828` / 节点 `#313131` /
  /// 标题 `#3E3E3E` / 参数行 `#242427`。原来那套偏蓝紫，和 ComfyUI 摆在一起
  /// 一眼就看得出不是同一个东西。
  static const BlueprintPalette dark = BlueprintPalette(
    canvas: Color(0xFF222224),
    grid: Color(0xFF2D2D2F),
    body: Color(0xFF313133),
    border: Color(0xFF4A4A4F),
    header: Color(0xFF3E3E42),
    title: Color(0xFFE6E6E8),
    subtitle: Color(0xFFA0A0A8),
    portLabel: Color(0xFFD2D2D6),
    widgetBar: Color(0xFF242426),
    widgetText: Color(0xFFD4D4D8),
    accents: {
      'note': Color(0xFFC9A227),
      'reroute': Color(0xFF7F8C9B),
      'variable': Color(0xFFA56BC4),
      'broadcast': Color(0xFFD08A3E),
      'display': Color(0xFF3FA9B8),
      'bypasser': Color(0xFF9A8577),
      'subgraph': Color(0xFF8A93D6),
      'unknown': Color(0xFFE06C63),
    },
    typeColors: {
      'MODEL': Color(0xFFB39DDB),
      'CLIP': Color(0xFFFFD500),
      'CLIP_VISION': Color(0xFFA8DADC),
      'CLIP_VISION_OUTPUT': Color(0xFFAD7452),
      'CONDITIONING': Color(0xFFFFA931),
      'CONTROL_NET': Color(0xFF6EE7B7),
      'IMAGE': Color(0xFF64B5F6),
      'LATENT': Color(0xFFFF9CF9),
      'MASK': Color(0xFF81C784),
      'STYLE_MODEL': Color(0xFFC2FFAE),
      'VAE': Color(0xFFFF6E6E),
      'TAESD': Color(0xFFDCC274),
      'AUDIO': Color(0xFFE0E0E0),
      'VIDEO': Color(0xFF00BCD4),
      // 截图实测：STRING 是亮绿 `#7CFF7C`（ComfyUI 主题里没有列在基础 Palette 中）
      'STRING': Color(0xFF7CFF7C),
      'INT': Color(0xFF9CC3E5),
      'FLOAT': Color(0xFF9CC3E5),
      'BOOLEAN': Color(0xFF9CC3E5),
      'COMBO': Color(0xFFB0BEC5),
      'LORA_STACK': Color(0xFF94DCCD),
      'CONTROL_NET_STACK': Color(0xFF94DCCD),
      'PIPE_LINE': Color(0xFF7737AA),
      'XYPLOT': Color(0xFF74DA5D),
      'SAMPLING': Color(0xFF60A5FA),
    },
    unknownType: Color(0xFF9A9A9A),
  );

  /// 浅色：同一套规矩反过来 —— 节点底比画布**白**一档。
  static const BlueprintPalette light = BlueprintPalette(
    canvas: Color(0xFFEFEFF3),
    grid: Color(0xFFE3E3E9),
    body: Color(0xFFFFFFFF),
    border: Color(0xFFC6C6D0),
    header: Color(0xFFF1F1F5),
    title: Color(0xFF1B1B1F),
    subtitle: Color(0xFF6C6C78),
    portLabel: Color(0xFF3A3A44),
    widgetBar: Color(0xFFECECF1),
    widgetText: Color(0xFF3A3A45),
    accents: {
      'note': Color(0xFFB58900),
      'reroute': Color(0xFF607D8B),
      'variable': Color(0xFF8E44AD),
      'broadcast': Color(0xFFC1671B),
      'display': Color(0xFF00707C),
      'bypasser': Color(0xFF795548),
      'subgraph': Color(0xFF5C6BC0),
      'unknown': Color(0xFFC62828),
    },
    typeColors: {
      'MODEL': Color(0xFF6A4C93),
      'CLIP': Color(0xFF9A7400),
      'CLIP_VISION': Color(0xFF2A7F8E),
      'CLIP_VISION_OUTPUT': Color(0xFF8D5524),
      'CONDITIONING': Color(0xFFB45F00),
      'CONTROL_NET': Color(0xFF0F766E),
      'IMAGE': Color(0xFF1565C0),
      'LATENT': Color(0xFFAD1457),
      'MASK': Color(0xFF2E7D32),
      'STYLE_MODEL': Color(0xFF558B2F),
      'VAE': Color(0xFFC62828),
      'TAESD': Color(0xFF8D6E00),
      'AUDIO': Color(0xFF455A64),
      'VIDEO': Color(0xFF006064),
      'STRING': Color(0xFF2E7D32),
      'INT': Color(0xFF1F4E79),
      'FLOAT': Color(0xFF1F4E79),
      'BOOLEAN': Color(0xFF1F4E79),
      'COMBO': Color(0xFF546E7A),
      'LORA_STACK': Color(0xFF00695C),
      'CONTROL_NET_STACK': Color(0xFF00695C),
      'PIPE_LINE': Color(0xFF4A148C),
      'XYPLOT': Color(0xFF33691E),
      'SAMPLING': Color(0xFF1D4ED8),
    },
    unknownType: Color(0xFF77777F),
  );
}

/// 节点内的几何常量 —— **只有这一份**：绘制与命中测试都读它。
///
/// 为什么必须共用：一旦绘制和命中各写一套，表现就是"点得到的地方与画出来的地方
/// 不重合"，而且**一个错都不报**（点击落空只会让人以为"这里不能点"）。
///
/// 尺寸按 ComfyUI 截图反推：标题条 28、每个插口 18、参数行 20（格高 16）。
/// 旧值是 26/18/16/11 —— 小字挤在一起，长提示词只能显示一行，与 ComfyUI 差得远。
const double kBlueprintHeaderHeight = 28;
const double kBlueprintPortHeight = 18;

/// 参数行之间的步距（含小格子本体的高度）。
const double kBlueprintWidgetRowStep = 20;

/// 参数小格子的高度。
const double kBlueprintWidgetBarHeight = 16;

/// 参数行文字大小 / 行高（多行文本按它算高度）。
const double kBlueprintWidgetTextSize = 10.5;
const double kBlueprintWidgetLineHeight = 13;

/// 一个**参数行**的完整几何（绘制、命中和就地编辑共用）。
class BlueprintWidgetRow {
  final int index;

  /// 整行（含两列）
  final Rect rect;

  /// 名字列（多行文本框没有名字列，宽度为 0）
  final Rect label;

  /// 值列（多行文本框 = 整行）
  final Rect value;

  /// 是不是多行文本框（Note / 提示词这类）
  final bool multiline;

  /// 是否落在节点底边以内（节点太矮时后面的行不画、也点不到）
  final bool visible;

  /// 画几行文字（仅多行有意义）
  final int lines;

  const BlueprintWidgetRow({
    required this.index,
    required this.rect,
    required this.label,
    required this.value,
    required this.multiline,
    required this.visible,
    required this.lines,
  });
}

/// 这个节点是不是「注释框」（`Note` / `MarkdownNote` 及其变体）。
///
/// 注释框的 `widgets_values[0]` 是一整段正文，不该按"参数名 | 值"那一行画。
bool isBlueprintNote(BlueprintNode node) {
  final cls = node.className.toLowerCase();
  return node.kind == 'note' || cls == 'note' || cls.contains('markdownnote');
}

/// 真正占一行端口的输入数。
///
/// 纯控件输入（`widget: true` 且没有接线）在 ComfyUI 里显示为参数行、**不是**一个
/// 左侧插口；把它算进端口行数会把参数区一路往下推（TextGenerate 有 20 个输入，
/// 其中一半是控件，旧算法直接吃掉 360px 高度）。
int blueprintInputSocketCount(BlueprintNode node) =>
    node.inputs.where((port) => !port.widget || port.link != null).length;

/// 端口区一共几行（左右取较大者，与 ComfyUI 的 litegraph 一致）。
int blueprintPortRowCount(BlueprintNode node) =>
    math.max(blueprintInputSocketCount(node), node.outputs.length);

/// 某个输入插口画在第几行；纯控件且没接线的输入回 `null`（它不画端口）。
int? blueprintInputRowIndex(BlueprintNode node, int slot) {
  if (slot < 0 || slot >= node.inputs.length) return null;
  var row = 0;
  for (var i = 0; i < node.inputs.length; i++) {
    final port = node.inputs[i];
    final visible = !port.widget || port.link != null;
    if (i == slot) return visible ? row : null;
    if (visible) row++;
  }
  return null;
}

/// 参数值 → 画到画布上的一行文字（`null` 如实显示成破折号）。
String blueprintWidgetText(Object? value) {
  if (value == null) return '—';
  if (value is List) return value.join(', ');
  return value.toString();
}

/// 这个参数是不是该用**多行文本框**画。
///
/// 判据刻意保守：只有「本来就是多行编辑框的节点」「值里带换行」「够长的文本类参数」
/// 三条。像 `lora_name` 这种长文件名要保持在**一行内省略** —— 给它撑五行只会
/// 把后面的参数顶出节点。
bool isBlueprintMultilineWidget(BlueprintNode node, BlueprintWidget widget) {
  if (isBlueprintNote(node)) return true;
  final value = widget.value;
  if (value is! String) return false;
  if (value.contains('\n') || value.contains('\r')) return true;
  final cls = node.className.toLowerCase();
  if (cls.contains('multiline') || cls.contains('markdown')) return true;
  final name = (widget.name ?? '').toLowerCase();
  final textLike =
      cls.contains('textencode') ||
      cls.contains('textgenerate') ||
      cls.contains('primitivestring') ||
      cls.contains('stringconcatenate') ||
      cls.contains('savetext') ||
      name == 'text' ||
      name.endsWith('_text') ||
      name.contains('prompt');
  return textLike && value.runes.length > 48;
}

/// 估算一段文字在 [width] 宽里要折成几行（只用于算高度，不参与排版）。
///
/// 不引 `TextPainter` 是刻意的：几何要在纯 `test`（没有 widget binding）里也能算，
/// 而且绘制与命中必须读到同一个值。误差由画笔那一侧的 `maxLines` + 省略号兜底。
int estimateBlueprintTextLines(String text, double width, double fontSize) {
  if (text.isEmpty || width <= 0) return 1;
  final wide = fontSize;
  final narrow = fontSize * 0.56;
  var lines = 1;
  var lineWidth = 0.0;
  for (final rune in text.runes) {
    if (rune == 0x0A || rune == 0x0D) {
      if (rune == 0x0D) continue;
      lines++;
      lineWidth = 0;
      continue;
    }
    final unit = _isWideRune(rune) ? wide : narrow;
    if (lineWidth > 0 && lineWidth + unit > width) {
      lines++;
      lineWidth = 0;
    }
    lineWidth += unit;
  }
  return lines;
}

bool _isWideRune(int rune) =>
    (rune >= 0x1100 && rune <= 0x115F) ||
    (rune >= 0x2E80 && rune <= 0xA4CF) ||
    (rune >= 0xAC00 && rune <= 0xD7A3) ||
    (rune >= 0xF900 && rune <= 0xFAFF) ||
    (rune >= 0xFE30 && rune <= 0xFE6F) ||
    (rune >= 0xFF00 && rune <= 0xFF60) ||
    (rune >= 0xFFE0 && rune <= 0xFFE6) ||
    (rune >= 0x1F300 && rune <= 0x1FAFF);

/// 一个节点里**所有**参数行的几何（包含画不下的行，用 `visible` 标出来）。
///
/// 多行文本框会按剩余高度撑高：同一节点的后续行先预留 `kBlueprintWidgetRowStep`，
/// 所以"一段长提示词"不会把下面的参数顶出节点。
List<BlueprintWidgetRow> blueprintWidgetRows(BlueprintNode node, Rect box) {
  final rows = <BlueprintWidgetRow>[];
  if (node.widgets.isEmpty) return rows;
  final note = isBlueprintNote(node);
  final bodyTop =
      box.top +
      kBlueprintHeaderHeight +
      blueprintPortRowCount(node) * kBlueprintPortHeight;
  // 留 1px 给节点底边框：CLIPLoader 这种"3 行刚好塞满 106px"的节点，
  // 多扣 3px 会把最后一行裁掉一条。
  final bottom = box.bottom - 1;
  final textWidth = math.max(20.0, box.width - 16);
  var top = bodyTop + 2;

  for (var i = 0; i < node.widgets.length; i++) {
    final widget = node.widgets[i];
    final remaining = node.widgets.length - i - 1;
    final reserve = remaining * kBlueprintWidgetRowStep;
    final maxHeight = bottom - top;
    final multiline = note || isBlueprintMultilineWidget(node, widget);
    var lines = 1;
    var height = kBlueprintWidgetBarHeight;
    if (multiline) {
      final limit = math.max(kBlueprintWidgetBarHeight, maxHeight - reserve);
      final maxLines = math.max(
        1,
        ((limit - 6) / kBlueprintWidgetLineHeight).floor(),
      );
      lines = math.min(
        maxLines,
        estimateBlueprintTextLines(
          blueprintWidgetText(widget.value),
          textWidth,
          kBlueprintWidgetTextSize,
        ),
      );
      height = limit < kBlueprintWidgetBarHeight
          ? kBlueprintWidgetBarHeight
          : math.min(
              limit,
              math.max(
                kBlueprintWidgetBarHeight,
                lines * kBlueprintWidgetLineHeight + 6,
              ),
            );
    }
    final visible = top < bottom - 4 && maxHeight >= 8;
    final drawnHeight = visible
        ? math.min(height, maxHeight).toDouble()
        : height;
    final rect = Rect.fromLTWH(box.left + 6, top, box.width - 12, drawnHeight);
    final labelWidth = multiline ? 0.0 : _blueprintLabelWidth(rect);
    rows.add(
      BlueprintWidgetRow(
        index: widget.index,
        rect: rect,
        label: Rect.fromLTWH(rect.left, rect.top, labelWidth, rect.height),
        value: multiline
            ? rect
            : Rect.fromLTWH(
                rect.left + labelWidth,
                rect.top,
                math.max(1, rect.width - labelWidth).toDouble(),
                rect.height,
              ),
        multiline: multiline,
        visible: visible,
        lines: lines,
      ),
    );
    // 单行参数按固定 20px 步距（ComfyUI 的行高）；多行按实际高度 + 4px 间隔。
    top += multiline ? drawnHeight + 4 : kBlueprintWidgetRowStep;
  }
  return rows;
}

/// 参数行里"名字"那一格（**固定不变**的那一半）。
///
/// 宽度取行宽的 38%，并夹在 28~96 之间 —— 太窄名字看不出是什么，太宽值没地方了。
double _blueprintLabelWidth(Rect row) => (row.width * 0.38).clamp(28.0, 96.0);

BlueprintWidgetRow? _widgetRowOf(BlueprintNode node, Rect box, int index) {
  for (final row in blueprintWidgetRows(node, box)) {
    if (row.index == index) return row;
  }
  return null;
}

/// 一个**参数行**在画布坐标下的矩形（兼容旧调用：内部已改成按内容撑高）。
Rect blueprintWidgetRowRect(BlueprintNode node, Rect box, int index) =>
    _widgetRowOf(node, box, index)?.rect ?? Rect.zero;

Rect blueprintWidgetLabelRect(BlueprintNode node, Rect box, int index) =>
    _widgetRowOf(node, box, index)?.label ?? Rect.zero;

/// 参数行里"值"那一格（**可变**的那一半）。
///
/// 显示形式为：text（不变） | xxx...（可变），点击后右侧出现可编辑 textfield。
/// 就地编辑器位于该矩形上。
Rect blueprintWidgetValueRect(BlueprintNode node, Rect box, int index) =>
    _widgetRowOf(node, box, index)?.value ?? Rect.zero;

/// 点击位置（**图坐标**）落在哪一行参数上；不在任何一行上回 `null`。
///
/// 命中区比画出来的格子**四周放宽 3px**：多行文本行很高、单行行又只有 16px，
/// 统一放宽一点对手感和用例都更稳。
int? hitTestWidgetRow(BlueprintNode node, Rect box, Offset point) {
  for (final row in blueprintWidgetRows(node, box)) {
    if (!row.visible) break;
    if (row.rect.inflate(3).contains(point)) return row.index;
  }
  return null;
}

/// 蓝图绘制器的输入（**刻意与 widget 解耦**：用例可以直接调它，不用 pump 整个页面）。
class BlueprintPainterSpec {
  final WorkflowBlueprint blueprint;
  final BlueprintLayout layout;

  /// 图坐标 → 画布坐标的平移（画布左上角留白 + 包围盒可能为负）
  final Offset origin;

  /// **当前视口**（图坐标）。剔除就靠它 —— 上千节点的图只画看得见的那几个。
  final Rect viewport;
  final String? selectedId;
  final Set<String> highlightedNodes;

  /// 低于这个缩放就不画正文（LOD）：缩到 5% 还逐字排版会让整帧卡住
  final double scale;

  /// 配色（深浅两套之一）。**必须由页面按 `Theme.of(context).brightness` 传进来** ——
  /// 画笔里再写死一套颜色就会重演"深色主题下压一片近白"那个 bug。
  final BlueprintPalette palette;

  const BlueprintPainterSpec({
    required this.blueprint,
    required this.layout,
    this.origin = Offset.zero,
    this.viewport = Rect.largest,
    this.selectedId,
    this.highlightedNodes = const {},
    this.scale = 1,
    this.palette = BlueprintPalette.light,
  });
}

/// 把蓝图**画出来**：分组底板 → 连线 → 节点框 → 选中/问题标记。
///
/// 为什么用 `CustomPainter` 而不是给每个节点建 widget：实测一份真存档可以到
/// 91 个节点 / 7547px 宽的注释框，而上千节点的图用 widget 重建会很贵。
/// 画布只画**视口内**的东西（[BlueprintPaintStats] 钉住这一条）。
class BlueprintPainter extends CustomPainter {
  final BlueprintPainterSpec spec;
  final void Function(BlueprintPaintStats stats)? onPainted;

  BlueprintPainter(this.spec, {this.onPainted});

  static const double _portHeight = kBlueprintPortHeight;
  static const double _headerHeight = kBlueprintHeaderHeight;

  @override
  void paint(Canvas canvas, Size size) {
    // 视口是**图坐标**下那一块（由页面按当前变换算好传进来）：
    // 用 `size`（画布整个尺寸 = 整张图）去剔除等于一条都剔不掉。
    final viewport = spec.viewport;
    var drawnNodes = 0;
    var culledNodes = 0;
    var drawnEdges = 0;
    var culledEdges = 0;

    // 画布**自己的**底色：铺满整个控件。
    // ⚠ 必须在 `translate` **之前**画：平移量是"图坐标 → 画布坐标"，
    // 图坐标的原点常常离包围盒很远（实测工作流 x≈3550），平移之后再画
    // `Offset.zero & size` 会落到画面外 —— 表现是画布透明 / 只剩 Scaffold 底色。
    // 别再借 `Scaffold` 的 surface：借了就没法保证"节点底与画布底同侧"。
    canvas.drawRect(Offset.zero & size, Paint()..color = spec.palette.canvas);
    canvas.save();
    canvas.translate(spec.origin.dx, spec.origin.dy);
    _paintGrid(canvas, viewport);
    _paintGroups(canvas, viewport);
    final edges = _paintEdges(canvas, viewport);
    drawnEdges = edges.$1;
    culledEdges = edges.$2;
    final nodes = _paintNodes(canvas, viewport);
    drawnNodes = nodes.$1;
    culledNodes = nodes.$2;
    canvas.restore();

    onPainted?.call(
      BlueprintPaintStats(
        drawnNodes: drawnNodes,
        culledNodes: culledNodes,
        drawnEdges: drawnEdges,
        culledEdges: culledEdges,
      ),
    );
  }

  // -------------------------------------------------------------------------
  //  画布网格
  // -------------------------------------------------------------------------

  /// 网格步长（图坐标）。
  static const double _gridStep = 48;

  /// 很淡的网格：**只画视口内那一片**，而且线宽按缩放倒推，屏幕上永远是 1px。
  ///
  /// ⚠ 两道护栏缺一不可：`_canvasArea` 在拿不到逆矩阵时会把视口回成 `Rect.largest`
  /// （`visibleGraphRect` 的兜底），那种视口上按步长循环等于死循环。
  void _paintGrid(Canvas canvas, Rect viewport) {
    if (spec.scale < 0.15) return;
    if (!viewport.isFinite || viewport.isEmpty) return;
    if ((viewport.width / _gridStep).ceil() > 200 ||
        (viewport.height / _gridStep).ceil() > 200) {
      return;
    }
    final paint = Paint()
      ..color = spec.palette.grid
      ..strokeWidth = 1 / math.max(spec.scale, 0.05);
    final left = (viewport.left / _gridStep).floorToDouble() * _gridStep;
    final top = (viewport.top / _gridStep).floorToDouble() * _gridStep;
    for (var x = left; x <= viewport.right; x += _gridStep) {
      canvas.drawLine(
        Offset(x, viewport.top),
        Offset(x, viewport.bottom),
        paint,
      );
    }
    for (var y = top; y <= viewport.bottom; y += _gridStep) {
      canvas.drawLine(
        Offset(viewport.left, y),
        Offset(viewport.right, y),
        paint,
      );
    }
  }

  // -------------------------------------------------------------------------
  //  分组框
  // -------------------------------------------------------------------------
  void _paintGroups(Canvas canvas, Rect viewport) {
    for (final group in spec.blueprint.groups) {
      if (!group.valid) continue;
      final rect = Rect.fromLTWH(
        group.left,
        group.top,
        group.width,
        group.height,
      );
      if (!visibleInViewport(rect, viewport)) continue;
      final colour = _parseColor(group.color) ?? const Color(0xFF3F789E);
      // 深色画布下 alpha 0.06 的底色**等于没画**，所以按明暗各给一档
      final dark = spec.palette.isDark;
      final fill = Paint()
        ..color = colour.withValues(alpha: dark ? 0.12 : 0.07);
      final border = Paint()
        ..color = colour.withValues(alpha: dark ? 0.75 : 0.55)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2;
      final rounded = RRect.fromRectAndRadius(rect, const Radius.circular(8));
      canvas.drawRRect(rounded, fill);
      canvas.drawRRect(rounded, border);
      if (spec.scale > 0.25) {
        _text(
          canvas,
          group.title,
          Offset(rect.left + 10, rect.top + 6),
          color: colour,
          size: 13,
          bold: true,
        );
      }
    }
  }

  // -------------------------------------------------------------------------
  //  连线
  // -------------------------------------------------------------------------
  (int, int) _paintEdges(Canvas canvas, Rect viewport) {
    var drawn = 0;
    var culled = 0;
    for (final edge in spec.blueprint.edges) {
      final from = spec.layout.boxOf(edge.fromNode);
      final to = spec.layout.boxOf(edge.toNode);
      if (from == null || to == null) continue;
      final fromNode = spec.blueprint.nodeById(edge.fromNode);
      final toNode = spec.blueprint.nodeById(edge.toNode);
      final start = _outputAnchor(fromNode, from, edge.fromSlot);
      final end = _inputAnchor(toNode, to, edge.toSlot);
      // 剔除：整条贝塞尔都在视口外就跳过（用两头的包围盒近似，够用且便宜）
      if (!visibleInViewport(
        Rect.fromPoints(start, end).inflate(40),
        viewport,
      )) {
        culled++;
        continue;
      }
      drawn++;
      final severity = _edgeSeverity(edge);
      // 普通连线按**类型**上色（ComfyUI 的 link_type_colors）；错误 / 警告优先。
      final type = _edgeType(edge, fromNode, toNode);
      final colour = severity == 'error'
          ? const Color(0xFFE53935)
          : severity == 'warn'
          ? const Color(0xFFF9A825)
          : spec.palette.typeColor(type);
      final paint = Paint()
        ..color = colour.withValues(alpha: severity == 'none' ? 0.92 : 1)
        ..style = PaintingStyle.stroke
        ..strokeWidth = severity == 'none' ? 2.2 : 3.2
        ..strokeCap = StrokeCap.round;
      // 控制点按水平距离走：ComfyUI 的线也是「先横着出来、再横着进去」。
      final reach = (end.dx - start.dx).abs().clamp(50.0, 220.0).toDouble();
      final path = Path()
        ..moveTo(start.dx, start.dy)
        ..cubicTo(
          start.dx + reach,
          start.dy,
          end.dx - reach,
          end.dy,
          end.dx,
          end.dy,
        );
      canvas.drawPath(path, paint);
    }
    return (drawn, culled);
  }

  /// 连线类型：优先用连线自己声明的，其次上游输出 / 下游输入声明的。
  String? _edgeType(
    BlueprintEdge edge,
    BlueprintNode? fromNode,
    BlueprintNode? toNode,
  ) {
    final declared = edge.type;
    if (declared != null && declared.trim().isNotEmpty) return declared;
    if (fromNode != null &&
        edge.fromSlot >= 0 &&
        edge.fromSlot < fromNode.outputs.length) {
      final type = fromNode.outputs[edge.fromSlot].type;
      if (type.isNotEmpty) return type;
    }
    if (toNode != null &&
        edge.toSlot >= 0 &&
        edge.toSlot < toNode.inputs.length) {
      final type = toNode.inputs[edge.toSlot].type;
      if (type.isNotEmpty) return type;
    }
    return null;
  }

  /// 这条线自己有没有结论（类型不匹配 / 引用了不存在的节点）。
  String _edgeSeverity(BlueprintEdge edge) {
    for (final issue in spec.blueprint.issues) {
      if (issue.links.isNotEmpty &&
          edge.id != null &&
          issue.links.contains(edge.id)) {
        return issue.severity;
      }
    }
    return 'none';
  }

  // -------------------------------------------------------------------------
  //  节点
  // -------------------------------------------------------------------------
  (int, int) _paintNodes(Canvas canvas, Rect viewport) {
    var drawn = 0;
    var culled = 0;
    for (final node in spec.blueprint.nodes) {
      final rect = spec.layout.boxOf(node.id);
      if (rect == null) continue;
      if (!visibleInViewport(rect, viewport)) {
        culled++;
        continue;
      }
      drawn++;
      _paintNode(canvas, node, rect);
    }
    return (drawn, culled);
  }

  void _paintNode(Canvas canvas, BlueprintNode node, Rect rect) {
    final severity = spec.blueprint.severityForNode(node.id);
    final selected = spec.selectedId == node.id;
    final dim = node.bypassed || node.muted;
    final palette = spec.palette;

    // 底色**不按 kind 变**：大面积铺语义色正是"刺眼"的来源。
    // 语义区分只保留在边框上（ComfyUI 的标题条也是中性灰）。
    final body = dim ? palette.body.withValues(alpha: 0.55) : palette.body;

    final border = switch (severity) {
      'error' => const Color(0xFFE05252),
      'warn' => const Color(0xFFE0A32E),
      'info' => const Color(0xFF4C9BE8),
      _ => palette.accentOf(node.kind),
    };

    final rounded = RRect.fromRectAndRadius(rect, const Radius.circular(6));
    canvas.drawRRect(rounded, Paint()..color = body);
    canvas.drawRRect(
      rounded,
      Paint()
        ..color = selected
            ? const Color(0xFF00B0FF)
            : border.withValues(alpha: dim ? 0.5 : 1)
        ..style = PaintingStyle.stroke
        ..strokeWidth = selected ? 3 : (severity == 'none' ? 1.2 : 2.4),
    );

    // 标题条：中性灰，与 ComfyUI 一致
    final headerHeight = math.min(_headerHeight, rect.height);
    if (headerHeight > 0) {
      canvas.save();
      canvas.clipRRect(rounded);
      canvas.drawRect(
        Rect.fromLTWH(rect.left, rect.top, rect.width, headerHeight),
        Paint()..color = palette.header.withValues(alpha: dim ? 0.5 : 1),
      );
      if (rect.height > _headerHeight) {
        canvas.drawLine(
          Offset(rect.left, rect.top + _headerHeight),
          Offset(rect.right, rect.top + _headerHeight),
          Paint()
            ..color = palette.border.withValues(alpha: dim ? 0.25 : 0.55)
            ..strokeWidth = 1,
        );
      }
      canvas.restore();
    }

    // 节点 id：画在框的**右上角外面**（ComfyUI 也这么标），右对齐。
    if (spec.scale > 0.35 && rect.width > 70) {
      _textRight(
        canvas,
        '#${node.id}',
        rect.right - 2,
        rect.top - 14,
        color: palette.subtitle.withValues(alpha: dim ? 0.45 : 0.9),
        size: 9.5,
        maxWidth: 90,
      );
    }

    if (spec.scale > 0.18) {
      // 标题被改过时类名才是"这到底是什么节点"：标题上、类名下，两行都塞进 28px。
      final showClass =
          spec.scale > 0.5 &&
          node.title.trim().isNotEmpty &&
          node.title != node.className &&
          rect.height > _headerHeight + 4;
      final iconSpace = rect.width > 80 ? 15.0 : 0.0;
      if (iconSpace > 0) {
        canvas.drawCircle(
          Offset(rect.left + 9, rect.top + (showClass ? 9 : _headerHeight / 2)),
          3,
          Paint()..color = palette.subtitle.withValues(alpha: dim ? 0.3 : 0.65),
        );
      }
      _text(
        canvas,
        node.displayTitle,
        Offset(rect.left + 6 + iconSpace, rect.top + (showClass ? 2.5 : 6)),
        color: palette.title.withValues(alpha: dim ? 0.45 : 1),
        size: 12.5,
        bold: true,
        maxWidth: rect.width - 12 - iconSpace,
      );
      if (showClass) {
        _text(
          canvas,
          node.className,
          Offset(rect.left + 6 + iconSpace, rect.top + 15.5),
          color: palette.subtitle.withValues(alpha: dim ? 0.45 : 0.95),
          size: 9.5,
          maxWidth: rect.width - 12 - iconSpace,
        );
      }
    }

    // 参数区要裁在圆角里（长文本、超出的参数行不许画到框外）。
    if (spec.scale > 0.3) {
      canvas.save();
      canvas.clipRRect(rounded);
      _paintWidgets(canvas, node, rect, dim);
      canvas.restore();
      // 插口画在裁剪之外：圆点要压在边框上、可以露出去半个
      _paintPorts(canvas, node, rect, dim);
    }

    // 角标：旁路 / 静音 / 纯前端
    if (node.bypassed || node.muted || node.frontendOnly) {
      final label = node.muted ? '静音' : (node.bypassed ? '旁路' : '前端');
      _badge(
        canvas,
        rect,
        label,
        dim ? const Color(0xFF9E9E9E) : const Color(0xFF6D4C41),
      );
    }
    if (severity != 'none') {
      _badge(
        canvas,
        rect,
        severity == 'error' ? '!' : '?',
        border,
        right: true,
      );
    }
  }

  void _paintPorts(Canvas canvas, BlueprintNode node, Rect rect, bool dim) {
    for (var slot = 0; slot < node.inputs.length; slot++) {
      final row = blueprintInputRowIndex(node, slot);
      if (row == null) continue;
      final y = rect.top + _headerHeight + _portHeight * row + _portHeight / 2;
      if (y > rect.bottom - 1) break;
      final port = node.inputs[slot];
      _paintPortDot(canvas, Offset(rect.left, y), port, node, dim);
      if (spec.scale > 0.45) {
        _text(
          canvas,
          port.name,
          Offset(rect.left + 9, y - 7),
          color: spec.palette.portLabel.withValues(alpha: dim ? 0.45 : 1),
          size: 10,
          maxWidth: math.max(20.0, rect.width * 0.5 - 14),
        );
      }
    }
    for (var slot = 0; slot < node.outputs.length; slot++) {
      final y = rect.top + _headerHeight + _portHeight * slot + _portHeight / 2;
      if (y > rect.bottom - 1) break;
      final port = node.outputs[slot];
      _paintPortDot(canvas, Offset(rect.right, y), port, node, dim);
      if (spec.scale > 0.45) {
        // **右对齐**：输出名贴着右边的圆点，像 ComfyUI 那样从右往左排。
        _textRight(
          canvas,
          port.name,
          rect.right - 9,
          y - 7,
          color: spec.palette.portLabel.withValues(alpha: dim ? 0.45 : 1),
          size: 10,
          maxWidth: math.max(20.0, rect.width * 0.5 - 14),
        );
      }
    }
  }

  /// 一个插口圆点：**类型色**填充 + 深色外圈；有问题时再套一圈错误 / 警告色。
  void _paintPortDot(
    Canvas canvas,
    Offset at,
    BlueprintPort port,
    BlueprintNode node,
    bool dim,
  ) {
    final typeColor = spec.palette.typeColor(port.type);
    final issues = spec.blueprint.issuesForSlot(node.id, port.name);
    final issueColor = issues.any((issue) => issue.isError)
        ? const Color(0xFFE05252)
        : issues.any((issue) => issue.isWarn)
        ? const Color(0xFFE0A32E)
        : null;
    // 外圈用画布底色：圆点压在节点边框上时边界才清楚
    canvas.drawCircle(
      at,
      4.8,
      Paint()..color = spec.palette.canvas.withValues(alpha: dim ? 0.4 : 1),
    );
    canvas.drawCircle(
      at,
      3.5,
      Paint()
        ..color = typeColor.withValues(
          alpha: dim ? 0.4 : (port.isOpenSocket ? 0.75 : 1),
        ),
    );
    if (issueColor != null) {
      canvas.drawCircle(
        at,
        5.5,
        Paint()
          ..color = issueColor.withValues(alpha: dim ? 0.5 : 1)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.6,
      );
    }
  }

  /// 参数区：单行 = `名字 | 值`（值**右对齐**）；长文本 = 多行文本框。
  void _paintWidgets(Canvas canvas, BlueprintNode node, Rect rect, bool dim) {
    final rows = blueprintWidgetRows(node, rect);
    for (final row in rows) {
      if (!row.visible) break;
      final widget = _widgetAt(node, row.index);
      if (widget == null) continue;
      final bar = RRect.fromRectAndRadius(row.rect, const Radius.circular(3));
      canvas.drawRRect(
        bar,
        Paint()
          ..color = spec.palette.widgetBar.withValues(alpha: dim ? 0.45 : 1),
      );
      if (row.multiline) {
        canvas.drawRRect(
          bar,
          Paint()
            ..color = spec.palette.border.withValues(alpha: dim ? 0.2 : 0.5)
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1,
        );
        _textBlock(
          canvas,
          blueprintWidgetText(widget.value),
          Offset(row.rect.left + 6, row.rect.top + 3),
          color: spec.palette.widgetText.withValues(alpha: dim ? 0.5 : 1),
          size: kBlueprintWidgetTextSize,
          maxWidth: row.rect.width - 12,
          maxLines: row.lines,
        );
        continue;
      }

      final textTop = row.rect.top + (row.rect.height - 13) / 2;
      _text(
        canvas,
        widget.label,
        Offset(row.label.left + 6, textTop),
        color: spec.palette.subtitle.withValues(alpha: dim ? 0.45 : 1),
        size: 10,
        maxWidth: math.max(12.0, row.label.width - 12),
      );

      final value = widget.value;
      if (value is bool) {
        _switch(canvas, row.rect, value, dim);
        continue;
      }
      final text = blueprintWidgetText(value)
          .replaceAll(RegExp(r'[\r\n]+'), ' ');
      final swatch = _hexColor(text);
      if (swatch != null) {
        final chip = Rect.fromLTWH(
          row.rect.right - 17,
          row.rect.top + 3,
          10,
          10,
        );
        canvas.drawRRect(
          RRect.fromRectAndRadius(chip, const Radius.circular(2)),
          Paint()..color = swatch,
        );
        canvas.drawRRect(
          RRect.fromRectAndRadius(chip, const Radius.circular(2)),
          Paint()
            ..color = spec.palette.border
            ..style = PaintingStyle.stroke
            ..strokeWidth = 0.8,
        );
      }
      _textRight(
        canvas,
        text,
        row.rect.right - (swatch != null ? 22 : 6),
        textTop,
        color: spec.palette.widgetText.withValues(alpha: dim ? 0.5 : 1),
        size: 10,
        // ⚠ 只能占"值"那一列：给整行宽度会让长值左移压住名字列。
        maxWidth: math.max(
          10.0,
          row.value.width - 10 - (swatch != null ? 14 : 0),
        ),
      );
    }
  }

  BlueprintWidget? _widgetAt(BlueprintNode node, int index) {
    for (final widget in node.widgets) {
      if (widget.index == index) return widget;
    }
    return null;
  }

  /// 布尔值画成一个开关（ComfyUI 在这个位置也是开关，而不是 `true` / `false`）。
  void _switch(Canvas canvas, Rect row, bool value, bool dim) {
    const width = 26.0;
    const height = 13.0;
    final track = RRect.fromRectAndRadius(
      Rect.fromLTWH(
        row.right - width - 6,
        row.top + (row.height - height) / 2,
        width,
        height,
      ),
      const Radius.circular(height / 2),
    );
    canvas.drawRRect(
      track,
      Paint()
        ..color =
            (value ? spec.palette.typeColor('BOOLEAN') : spec.palette.subtitle)
                .withValues(alpha: dim ? 0.35 : (value ? 0.9 : 0.4)),
    );
    canvas.drawCircle(
      Offset(
        value ? track.right - height / 2 : track.left + height / 2,
        track.center.dy,
      ),
      height / 2 - 2,
      Paint()..color = spec.palette.title.withValues(alpha: dim ? 0.5 : 1),
    );
  }

  void _badge(
    Canvas canvas,
    Rect rect,
    String label,
    Color colour, {
    bool right = false,
  }) {
    final painter = TextPainter(
      text: TextSpan(
        text: label,
        style: TextStyle(
          color: Colors.white,
          fontSize: 9,
          fontWeight: FontWeight.bold,
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    final width = painter.width + 8;
    final origin = right
        ? Offset(rect.right - width - 3, rect.bottom - 15)
        : Offset(rect.left + 3, rect.bottom - 15);
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(origin.dx, origin.dy, width, 13),
        const Radius.circular(3),
      ),
      Paint()..color = colour,
    );
    painter.paint(canvas, Offset(origin.dx + 4, origin.dy + 1));
  }

  // -------------------------------------------------------------------------
  //  锚点
  // -------------------------------------------------------------------------

  /// 输入锚点：按**真实插口行**算（纯控件输入不占行，见 [blueprintInputRowIndex]）。
  Offset _inputAnchor(BlueprintNode? node, Rect box, int slot) {
    var row = math.max(0, slot);
    if (node != null) {
      final mapped = blueprintInputRowIndex(node, slot);
      if (mapped != null) row = mapped;
    }
    return Offset(
      box.left,
      box.top + _headerHeight + _portHeight * row + _portHeight / 2,
    );
  }

  /// 输出锚点：输出永远是插口，行号就是 slot。
  Offset _outputAnchor(BlueprintNode? node, Rect box, int slot) {
    final count = node?.outputs.length ?? 0;
    final row = count > 0 ? slot.clamp(0, count - 1) : math.max(0, slot);
    return Offset(
      box.right,
      box.top + _headerHeight + _portHeight * row + _portHeight / 2,
    );
  }

  // -------------------------------------------------------------------------
  //  文字
  // -------------------------------------------------------------------------

  TextStyle _style(Color color, double size, {bool bold = false}) => TextStyle(
    color: color,
    fontSize: size,
    fontWeight: bold ? FontWeight.w600 : FontWeight.normal,
    height: 1.22,
  );

  /// 画一行字；超出 `maxWidth` 由 TextPainter 自己省略（**不手算字符数**）。
  void _text(
    Canvas canvas,
    String text,
    Offset at, {
    required Color color,
    required double size,
    bool bold = false,
    double? maxWidth,
  }) {
    if (text.isEmpty) return;
    // ⚠ 截断**交给 TextPainter 自己按 `maxWidth` 排版**，不再手算字符数。
    //
    // 这里原来是 `limit = maxWidth / (size * 0.62)` —— `0.62em` 是拉丁字体的平均字宽，
    // 中文≈1.0em，于是**低估约 40%**：长中文参数会画到框外、压住相邻元素。
    // 而且旧写法最后是 `painter.layout()`（**没传 `maxWidth`**），
    // 所以下面那句 `maxLines: 1, ellipsis: '…'` 其实**永远不会触发**。
    //
    // 担心"逐字量太贵"是多余的：`_paintNodes` 已经做了视口剔除，
    // 要量的只有屏幕里那几十个节点；何况旧实现本来就已经在建 TextPainter 并 layout 了。
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: _style(color, size, bold: bold),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: math.max(1, maxWidth ?? double.infinity));
    painter.paint(canvas, at);
  }

  /// 右对齐画一行字（输出名、数值、参数值都用它）。
  void _textRight(
    Canvas canvas,
    String text,
    double right,
    double top, {
    required Color color,
    required double size,
    bool bold = false,
    double? maxWidth,
  }) {
    if (text.isEmpty) return;
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: _style(color, size, bold: bold),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: math.max(1, maxWidth ?? double.infinity));
    painter.paint(canvas, Offset(right - painter.width, top));
  }

  /// 多行文字块（Note 正文 / 长提示词）：行数按 [`blueprintWidgetRows`] 算好的来。
  void _textBlock(
    Canvas canvas,
    String text,
    Offset at, {
    required Color color,
    required double size,
    required double maxWidth,
    required int maxLines,
  }) {
    if (text.isEmpty) return;
    final painter = TextPainter(
      text: TextSpan(text: text, style: _style(color, size)),
      textDirection: TextDirection.ltr,
      maxLines: math.max(1, maxLines),
      ellipsis: '…',
    )..layout(maxWidth: math.max(1, maxWidth));
    painter.paint(canvas, at);
  }

  /// `#222222` 这种颜色值 → 色块；不是合法颜色回 null。
  static Color? _hexColor(String text) {
    final match = RegExp(r'^#([0-9a-fA-F]{6})$').firstMatch(text.trim());
    if (match == null) return null;
    final value = int.tryParse(match.group(1)!, radix: 16);
    return value == null ? null : Color(0xFF000000 | value);
  }

  static Color? _parseColor(String? raw) {
    if (raw == null) return null;
    var text = raw.trim();
    if (text.startsWith('#')) text = text.substring(1);
    if (text.length == 6) text = 'FF$text';
    if (text.length != 8) return null;
    final value = int.tryParse(text, radix: 16);
    return value == null ? null : Color(value);
  }

  @override
  bool shouldRepaint(covariant BlueprintPainter old) =>
      old.spec.blueprint != spec.blueprint ||
      old.spec.layout != spec.layout ||
      old.spec.origin != spec.origin ||
      old.spec.viewport != spec.viewport ||
      old.spec.selectedId != spec.selectedId ||
      old.spec.scale != spec.scale ||
      // ⚠ 配色必须一起比较：遗漏时表现为"切换深浅主题后蓝图不变色"，
      // 而**一个错都不报**。（`BlueprintPalette.of()` 回的是两个 const 单例，
      // 所以这里是身份比较，不会每帧都判"变了"。）
      old.spec.palette != spec.palette ||
      !_sameSet(old.spec.highlightedNodes, spec.highlightedNodes);

  static bool _sameSet(Set<String> a, Set<String> b) =>
      a.length == b.length && a.every(b.contains);
}

/// 画布上的一次命中（点击位置 → 节点）。
///
/// 单独抽出来是为了可测：用例不必去点像素，直接喂图坐标。
String? hitTestNode(BlueprintLayout layout, Offset point) {
  // 从后往前找（后画的在上面）
  String? hit;
  for (final entry in layout.boxes.entries) {
    if (entry.value.contains(point)) hit = entry.key;
  }
  return hit;
}

/// 视口剔除的判据（用例断言"画得比总数少"时用它算期望）。
///
/// **只有这一份**：画布的三处剔除都调它 —— 各写一遍 `overlaps` 的下场是
/// "某个方向忘了判"，而那只是多画一点、不会报错（性能回归就这么溜进来）。
bool visibleInViewport(Rect box, Rect viewport) => box.overlaps(viewport);

/// 画布当前的缩放倍数（**只取 X / Y 两轴**）。
///
/// ⚠ 别用 `Matrix4.getMaxScaleOnAxis()`：它把 **Z 轴也一起算**，而画布的矩阵是
/// `scale(fit, fit, 1)` —— Z 恒等于 1，于是**只要在缩小（fit < 1）它就永远回 1.0**。
///
/// 后果有两层，都是静默的：
/// 1. **LOD 失效**：`spec.scale > 0.22 / 0.3 / 0.45 / 0.55` 这几道"缩太小就不画正文"
///    在缩小时全部照旧通过 —— 一份 9600px 宽的工作流按 8% 打开时会**逐字画全部文本**，
///    这正是"蓝图加载/打开比 ComfyUI 慢一个量级"的观感来源之一；
/// 2. 线宽 / 网格步长按缩放倒推的地方（`1 / spec.scale`）会算错。
double blueprintScaleOf(Matrix4 transform) {
  final s = transform.storage;
  final sx = math.sqrt(s[0] * s[0] + s[1] * s[1]);
  final sy = math.sqrt(s[4] * s[4] + s[5] * s[5]);
  return math.max(sx, sy);
}

/// 当前视口落在**图坐标**下的矩形。
///
/// `transform` 是 `InteractiveViewer` 的变换（画布坐标），`origin` 是"图坐标 → 画布坐标"
/// 的平移，所以要减回去。拿不到逆矩阵时回 `Rect.largest`（= **不剔除**：
/// 宁可多画一点，也不要因为算不出来就把整张图画成空白）。
Rect visibleGraphRect(Matrix4 transform, Size viewport, Offset origin) {
  final inverse = Matrix4.tryInvert(transform);
  if (inverse == null || viewport.isEmpty) return Rect.largest;
  final a = MatrixUtils.transformPoint(inverse, Offset.zero);
  final b = MatrixUtils.transformPoint(
    inverse,
    Offset(viewport.width, viewport.height),
  );
  return Rect.fromPoints(a, b).shift(-origin);
}
