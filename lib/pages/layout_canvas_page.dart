import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../app.dart' show kWideLayoutMinWidth;
import '../core/layout_preset_store.dart';
import '../models/layout_embed.dart';
import '../models/layout_spec.dart';
import '../state/ai_workspace_store.dart';
import '../widgets/common.dart';
import '../widgets/layout_thumbnail.dart' show layoutKindColor;

/// 画布预设。**尺寸都取 16 的倍数**（ComfyUI / 采样器普遍按 16 对齐），
/// 宽高比取整数比，[LayoutSpec.ratioLabel] 才拼得出"3:4 竖构图"。
const List<({String label, int w, int h})> kLayoutPresets = [
  (label: '1:1 方形', w: 1024, h: 1024),
  (label: '3:4 竖构图', w: 768, h: 1024),
  (label: '4:3 横构图', w: 1024, h: 768),
  (label: '2:3 竖构图', w: 832, h: 1248),
  (label: '3:2 横构图', w: 1248, h: 832),
  (label: '16:9 宽幅', w: 1024, h: 576),
  (label: '9:16 竖屏', w: 576, h: 1024),
];

/// 八个缩放手柄（四角 + 四边中点），坐标是相对框的 0 / 0.5 / 1。
///
/// 只给右下角一个的话，想改左边就得"先移动再改大小"，用户 2026-11-01 直接说
/// "太难用"。角和边分开是有用的：拖边只动一个轴，拖角两个轴一起动。
const List<({double ax, double ay})> kLayoutHandles = [
  (ax: 0, ay: 0),
  (ax: 0.5, ay: 0),
  (ax: 1, ay: 0),
  (ax: 0, ay: 0.5),
  (ax: 1, ay: 0.5),
  (ax: 0, ay: 1),
  (ax: 0.5, ay: 1),
  (ax: 1, ay: 1),
];

/// 布局 → 提示词。
///
/// 两条入口，**画布本身只有一份**：
///   · 从 AI 工作台来（[fromWorkspace]）：AI 已经给了一份草图（[initialSpec]），
///     用户在这里改，点「提交回工作台」把规格交回去 —— 模型按新框重写提示词
///     （2026-11-03 用户口径："AI 生成提示词的同时给出可用的 2D 布局模板，
///     我可打开预览并修改，修改后重新提交给工作台"）；
///   · 从「预设库」来（默认）：空白画布，自己框，然后把提示词复制走。
///
/// 三条设计纪律（对着这个文件看）：
///   ① **坐标只有一套单位**（0~1000，见 `LayoutRect`），画布只做乘除；
///   ② **散文是有损投影**，所以先量化成词再拼句，不把百分比写进正文；
///   ③ **翻译是纯函数**（`LayoutSpec.translate`），同一份布局翻两次必须逐字节相同 ——
///      这条是"AI 只做可选润色"的前提，也是 `test/layout_spec_test.dart` 钉住的东西。
class LayoutCanvasPage extends StatefulWidget {
  const LayoutCanvasPage({
    super.key,
    this.initialSpec,
    this.initialName,
    this.fromWorkspace = false,
  });

  /// AI 给的那份草图（或用户上次提交的那一版）；null = 空白画布。
  final LayoutSpec? initialSpec;

  /// 草图的名字（卡片标题），只用来在面板里标明"正在改哪一版"。
  final String? initialName;

  /// 是不是从 AI 工作台进来的：决定标题与「提交回工作台」那一块。
  final bool fromWorkspace;

  @override
  State<LayoutCanvasPage> createState() => _LayoutCanvasPageState();
}

/// 正在拖什么。
enum _DragMode { none, create, move, resize }

class _LayoutCanvasPageState extends State<LayoutCanvasPage> {
  final List<LayoutRegion> _regions = [];
  final TextEditingController _brief = TextEditingController();

  /// 机位。
  LayoutCamera _camera = LayoutCamera.auto;

  /// 布局库（SharedPreferences）。
  final LayoutPresetStore _presets = LayoutPresetStore();

  /// AI 译者的结果。null = 还没译过（此时界面上只有模板投影）。
  bool _translating = false;
  String? _aiPrompt;
  String? _aiModel;
  List<String> _aiIssues = const [];

  int _presetIndex = 0;
  String? _selectedId;
  int _nextId = 1;

  _DragMode _mode = _DragMode.none;
  String? _dragId;
  LayoutRect? _dragOrigin;
  (int, int)? _dragStartUnit;
  ({double ax, double ay})? _dragAnchor;

  /// 指针**真正按下**的位置。见 [_onPanStart] 里那段"为什么不用 onPanStart"。
  Offset? _downLocal;
  (int, int)? _downUnit;

  @override
  void initState() {
    super.initState();
    final spec = widget.initialSpec;
    if (spec != null) _loadSpec(spec);
  }

  @override
  void dispose() {
    _brief.dispose();
    super.dispose();
  }

  ({String label, int w, int h}) get _preset => kLayoutPresets[_presetIndex];

  LayoutSpec get _spec => LayoutSpec(
        width: _preset.w,
        height: _preset.h,
        brief: _brief.text,
        camera: _camera,
        regions: List.unmodifiable(_regions),
      );

  LayoutRegion? get _selected {
    for (final r in _regions) {
      if (r.id == _selectedId) return r;
    }
    return null;
  }

  LayoutRegion? _byId(String id) {
    for (final r in _regions) {
      if (r.id == id) return r;
    }
    return null;
  }

  void _replace(
    String id, {
    LayoutRect? rect,
    LayoutBoxKind? kind,
    String? label,
    String? caption,
    String? text,
    LayoutRelation? relation,
    String? relationTo,
    bool clearRelation = false,
    LayoutSurface? surface,
  }) {
    final i = _regions.indexWhere((r) => r.id == id);
    if (i < 0) return;
    _regions[i] = _regions[i].copyWith(
      rect: rect,
      kind: kind,
      label: label,
      caption: caption,
      text: text,
      relation: relation,
      relationTo: relationTo,
      clearRelation: clearRelation,
      surface: surface,
    );
  }

  void _remove(String id) {
    setState(() {
      _regions.removeWhere((r) => r.id == id);
      // 指向被删框的关系一并清掉，免得散文里冒出"与「未命名」是同一个主体"
      for (var i = 0; i < _regions.length; i++) {
        if (_regions[i].relationTo == id) {
          _regions[i] = _regions[i].copyWith(
            relation: LayoutRelation.none,
            clearRelation: true,
          );
        }
      }
      if (_selectedId == id) _selectedId = null;
    });
  }

  List<LayoutRegion> _others(LayoutRegion r) =>
      _regions.where((x) => x.id != r.id).toList();

  // ── 手势 ────────────────────────────────────────────────────────────────

  static int _unit(double v, double total) {
    final n = (v / total * 1000).round();
    return n < 0 ? 0 : (n > 1000 ? 1000 : n);
  }

  String? _hitTest(int x, int y) {
    for (var i = _regions.length - 1; i >= 0; i--) {
      if (_regions[i].rect.contains(x, y)) return _regions[i].id;
    }
    return null;
  }

  /// 手柄命中：**按屏幕像素算固定半径**（画布多大都是 16px），
  /// 按 0~1000 算的话窄画布上会小到点不着。
  ({double ax, double ay})? _handleAt(LayoutRect r, Offset local, Size size) {
    for (final h in kLayoutHandles) {
      final p = Offset(
        (r.x + r.w * h.ax) / 1000 * size.width,
        (r.y + r.h * h.ay) / 1000 * size.height,
      );
      if ((local - p).distance <= 16) return h;
    }
    return null;
  }

  void _onPanDown(Offset local, Size size) {
    _downLocal = local;
    _downUnit = (_unit(local.dx, size.width), _unit(local.dy, size.height));
  }

  void _onTapDown(Offset local, Size size) {
    final sel = _selected;
    // 压在手柄上**不改变选中**：否则一按就把选中的框取消掉，紧接着的
    // `onPanStart` 会以为在空白处起拖 —— 于是"想拉大"变成"多出一个框"。
    if (sel != null && _handleAt(sel.rect, local, size) != null) return;
    final id = _hitTest(_unit(local.dx, size.width), _unit(local.dy, size.height));
    setState(() => _selectedId = id);
  }

  void _onPanStart(Offset local, Size size) {
    // ⚠ 起点用 `onPanDown` 记下的位置，**不用 `onPanStart` 的 localPosition**：
    // 后者是"手势被识别出来时"的位置，已经吃掉了约 20px 的判定阈值（slop）。
    // 拿它当起点，拖出来的框会比按下的地方偏右下约 20px —— 用户 2026-11-01 的
    // "不准"有一半是这个（另一半是散文的方位词太粗）。
    final start = _downUnit ??
        (_unit(local.dx, size.width), _unit(local.dy, size.height));
    final downLocal = _downLocal ?? local;

    final sel = _selected;
    if (sel != null) {
      final anchor = _handleAt(sel.rect, downLocal, size);
      if (anchor != null) {
        _mode = _DragMode.resize;
        _dragId = sel.id;
        _dragOrigin = sel.rect;
        _dragStartUnit = start;
        _dragAnchor = anchor;
        return;
      }
    }

    final hit = _hitTest(start.$1, start.$2);
    if (hit != null) {
      final region = _byId(hit)!;
      setState(() => _selectedId = hit);
      _mode = _DragMode.move;
      _dragId = hit;
      _dragOrigin = region.rect;
      _dragStartUnit = start;
      _dragAnchor = null;
      return;
    }

    // 空白处起拖 = 新建
    final id = 'r${_nextId++}';
    _regions.add(LayoutRegion(id: id, rect: LayoutRect(start.$1, start.$2, 0, 0)));
    setState(() => _selectedId = id);
    _mode = _DragMode.create;
    _dragId = id;
    _dragOrigin = null;
    _dragStartUnit = start;
    _dragAnchor = null;
  }

  void _onPanUpdate(Offset local, Size size) {
    final id = _dragId;
    final start = _dragStartUnit;
    if (id == null || start == null || _mode == _DragMode.none) return;
    final x = _unit(local.dx, size.width);
    final y = _unit(local.dy, size.height);

    setState(() {
      switch (_mode) {
        case _DragMode.create:
          {
            final left = math.min(start.$1, x);
            final top = math.min(start.$2, y);
            final w = math.max((x - start.$1).abs(), 1);
            final h = math.max((y - start.$2).abs(), 1);
            _replace(
              id,
              rect: LayoutRect(left, top, w, h).clampedInside(),
            );
          }
        case _DragMode.move:
          {
            final origin = _dragOrigin;
            if (origin == null) return;
            _replace(
              id,
              rect: origin.translate(x - start.$1, y - start.$2).clampedInside(),
            );
          }
        case _DragMode.resize:
          {
            final origin = _dragOrigin;
            final anchor = _dragAnchor;
            if (origin == null || anchor == null) return;
            _replace(id, rect: _resized(origin, anchor, x, y));
          }
        case _DragMode.none:
          break;
      }
    });
  }

  /// 按抓到的那个手柄改矩形：只有被抓住的那条边跟着走，其余不动。
  ///
  /// 越过对边就交换（允许"翻面"）—— 拖过界时框突然消失比翻面更让人困惑。
  LayoutRect _resized(
    LayoutRect origin,
    ({double ax, double ay}) anchor,
    int x,
    int y,
  ) {
    var l = origin.x.toDouble();
    var t = origin.y.toDouble();
    var r = origin.right.toDouble();
    var b = origin.bottom.toDouble();

    if (anchor.ax == 0) l = x.toDouble();
    if (anchor.ax == 1) r = x.toDouble();
    if (anchor.ay == 0) t = y.toDouble();
    if (anchor.ay == 1) b = y.toDouble();

    if (r < l) {
      final s = l;
      l = r;
      r = s;
    }
    if (b < t) {
      final s = t;
      t = b;
      b = s;
    }
    if (r - l < 20) {
      if (anchor.ax == 0) {
        l = r - 20;
      } else {
        r = l + 20;
      }
    }
    if (b - t < 20) {
      if (anchor.ay == 0) {
        t = b - 20;
      } else {
        b = t + 20;
      }
    }
    return LayoutRect(l.round(), t.round(), (r - l).round(), (b - t).round())
        .clampedInside();
  }

  void _onPanEnd() {
    final id = _dragId;
    if (_mode == _DragMode.create && id != null) {
      final r = _byId(id);
      // 只点了一下（或拖得很小）：按一个默认大小的框收尾，
      // 免得留下一个 3×4 的、看不见也选不中的框。
      if (r != null && (r.rect.w < 40 || r.rect.h < 40)) {
        const side = 260;
        final x = r.rect.x.clamp(0, 1000 - side).toInt();
        final y = r.rect.y.clamp(0, 1000 - side).toInt();
        setState(() => _replace(id, rect: LayoutRect(x, y, side, side)));
      }
    }
    _mode = _DragMode.none;
    _dragId = null;
    _dragOrigin = null;
    _dragStartUnit = null;
    _dragAnchor = null;
    _downLocal = null;
    _downUnit = null;
  }

  // ── 构建 ────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    final wide = MediaQuery.sizeOf(context).width >= kWideLayoutMinWidth;
    return Scaffold(
      appBar: AppBar(
        title: Text(widget.fromWorkspace ? '调整布局' : '布局 → 提示词'),
        actions: [_ratioMenu(context), _moreMenu(context)],
      ),
      body: wide
          ? Row(
              children: [
                Expanded(child: _canvasArea(context)),
                const VerticalDivider(width: 1),
                SizedBox(width: 400, child: _panel(context)),
              ],
            )
          : Column(
              children: [
                Expanded(child: _canvasArea(context)),
                const Divider(height: 1),
                SizedBox(height: 300, child: _panel(context)),
              ],
            ),
    );
  }

  Widget _ratioMenu(BuildContext context) {
    return AppMenuButton<int>(
      tooltip: '画布尺寸',
      options: [
        for (var i = 0; i < kLayoutPresets.length; i++)
          MenuOption(
            value: i,
            label: kLayoutPresets[i].label,
            trailingIcon: i == _presetIndex ? Icons.check : null,
          ),
      ],
      onSelected: (i) => setState(() => _presetIndex = i),
      button: (context, controller, isOpen) => TextButton.icon(
        onPressed: () {
          if (controller.isOpen) {
            controller.close();
          } else {
            controller.open();
          }
        },
        icon: const Icon(Icons.aspect_ratio, size: 18),
        label: Text(kLayoutPresets[_presetIndex].label),
      ),
    );
  }

  Widget _moreMenu(BuildContext context) {
    return AppMenuButton<String>(
      tooltip: '更多',
      options: const [
        MenuOption(value: 'save', label: '保存布局', icon: Icons.save_outlined),
        MenuOption(value: 'open', label: '打开布局', icon: Icons.folder_open_outlined),
        MenuOption(
          value: 'clear',
          label: '清空画布',
          icon: Icons.delete_outline,
          danger: true,
          dividerBefore: true,
        ),
      ],
      onSelected: (v) {
        if (v == 'save') {
          _saveLayout();
        } else if (v == 'open') {
          _openLayout();
        } else if (v == 'clear') {
          _confirmClear();
        }
      },
      button: (context, controller, isOpen) => IconButton(
        tooltip: '更多',
        icon: const Icon(Icons.more_vert),
        onPressed: () {
          if (controller.isOpen) {
            controller.close();
          } else {
            controller.open();
          }
        },
      ),
    );
  }

  /// 保存当前布局。存的是**规格**（框 / 坐标 / 名称 / 关系），不是生成好的提示词 ——
  /// 提示词是投影，改一次措辞就过期。
  /// 让 AI 译者把这份布局编译成提示词。
  ///
  /// 用工作台**当前选中的** Provider / 模型：不另开一套配置，想换便宜模型就换选中项。
  /// 失败或校验不过都不影响模板 —— 模板永远在下面，它是质量下限。
  Future<void> _translate() async {
    if (_regions.isEmpty) {
      _snack('先框几个区域，再让 AI 写');
      return;
    }
    final store = context.read<AiWorkspaceStore>();
    if (store.selectedProvider == null || store.selectedModel == null) {
      _snack('还没选模型：先去 AI 工作台挑一个');
      return;
    }
    setState(() => _translating = true);
    try {
      final res = await store.translateLayout(
        spec: _spec.toJson(),
        intent: _brief.text.trim(),
      );
      if (!mounted) return;
      setState(() {
        _aiPrompt = (res['prompt'] as String?)?.trim();
        _aiModel = res['model'] as String?;
        _aiIssues = ((res['issues'] as List?) ?? const [])
            .map((e) => '$e')
            .toList();
      });
    } catch (e) {
      if (!mounted) return;
      _snack('翻译失败：$e');
    } finally {
      if (mounted) setState(() => _translating = false);
    }
  }

  void _snack(String text) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(text),
        duration: const Duration(milliseconds: 1800),
        behavior: SnackBarBehavior.floating,
        width: 300,
      ),
    );
  }

  /// 把当前画布交回 AI 工作台（2026-11-03 的主流程）。
  ///
  /// 返回值是 [LayoutResubmit]：工作台拿它拼一条用户消息，模型按新框重写提示词。
  /// 空画布不许提交 —— 交回一份"什么都没有"只会让模型问一句"你要画什么"。
  Future<void> _submitToWorkspace() async {
    if (_regions.isEmpty) {
      _snack('先框出至少一个区域，再提交');
      return;
    }
    final note = TextEditingController(text: LayoutResubmit.defaultNote);
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('提交回工作台'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('AI 会按这份布局重写提示词。可以补一句要求：'),
            const SizedBox(height: 10),
            TextField(
              controller: note,
              minLines: 2,
              maxLines: 3,
              autofocus: true,
              decoration: const InputDecoration(
                isDense: true,
                border: OutlineInputBorder(),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('提交'),
          ),
        ],
      ),
    );
    final noteText = note.text.trim();
    note.dispose();
    if (ok != true || !mounted) return;
    Navigator.of(context).pop(LayoutResubmit(spec: _spec, note: noteText));
  }

  Future<void> _saveLayout() async {
    final nameCtl = TextEditingController(text: _suggestName());
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('保存布局'),
        content: TextField(
          controller: nameCtl,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: '名称',
            isDense: true,
            border: OutlineInputBorder(),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    final name = nameCtl.text.trim().isEmpty ? '未命名布局' : nameCtl.text.trim();
    final list = await _presets.load();
    list.insert(
      0,
      LayoutPreset(
        id: DateTime.now().microsecondsSinceEpoch.toString(),
        name: name,
        updatedAt: DateTime.now(),
        spec: _spec,
      ),
    );
    await _presets.saveAll(list);
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('已保存「$name」'),
        duration: const Duration(milliseconds: 1400),
        behavior: SnackBarBehavior.floating,
        width: 240,
      ),
    );
  }

  String _suggestName() {
    final brief = _brief.text.trim();
    if (brief.isNotEmpty) {
      return brief.length > 14 ? '${brief.substring(0, 14)}…' : brief;
    }
    return '${_preset.label} 布局';
  }

  Future<void> _openLayout() async {
    final list = await _presets.load();
    if (!mounted) return;
    if (list.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('还没有保存过布局'),
          duration: Duration(milliseconds: 1400),
          behavior: SnackBarBehavior.floating,
          width: 240,
        ),
      );
      return;
    }
    final picked = await showDialog<LayoutPreset>(
      context: context,
      builder: (ctx) {
        var items = list;
        return StatefulBuilder(
          builder: (ctx, setLocal) => AlertDialog(
            title: const Text('打开布局'),
            contentPadding: const EdgeInsets.only(top: 12),
            content: SizedBox(
              width: 460,
              height: 360,
              child: ListView.builder(
                itemCount: items.length,
                itemBuilder: (ctx, i) {
                  final p = items[i];
                  return ListTile(
                    dense: true,
                    title: Text(p.name),
                    subtitle: Text(
                      '${p.spec.ratioLabel} · ${p.spec.regions.length} 个框 · '
                      '${_shortDate(p.updatedAt)}',
                    ),
                    onTap: () => Navigator.of(ctx).pop(p),
                    trailing: IconButton(
                      tooltip: '删除',
                      icon: const Icon(Icons.delete_outline, size: 18),
                      onPressed: () async {
                        final next = [...items]..removeAt(i);
                        await _presets.saveAll(next);
                        if (ctx.mounted) setLocal(() => items = next);
                      },
                    ),
                  );
                },
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(ctx).pop(),
                child: const Text('关闭'),
              ),
            ],
          ),
        );
      },
    );
    if (picked == null || !mounted) return;
    _applySpec(picked.spec);
  }

  static String _shortDate(DateTime t) {
    String two(int v) => v.toString().padLeft(2, '0');
    return '${t.month}-${t.day} ${two(t.hour)}:${two(t.minute)}';
  }

  /// 把一份规格灌回画布（**不** setState；initState 里也用它）。
  void _loadSpec(LayoutSpec spec) {
    var index =
        kLayoutPresets.indexWhere((p) => p.w == spec.width && p.h == spec.height);
    if (index < 0) {
      // 尺寸不在预设里（老版本存的 / 手改过的 / AI 给的）：按比例找最接近的一档，
      // 坐标是 0~1000 归一的，换档不会错位。
      final ratio = spec.width / spec.height;
      var best = 0;
      var bestDiff = double.infinity;
      for (var i = 0; i < kLayoutPresets.length; i++) {
        final d = (kLayoutPresets[i].w / kLayoutPresets[i].h - ratio).abs();
        if (d < bestDiff) {
          bestDiff = d;
          best = i;
        }
      }
      index = best;
    }
    _presetIndex = index;
    _regions
      ..clear()
      ..addAll(spec.regions);
    _brief.text = spec.brief;
    _camera = spec.camera;
    _selectedId = null;
    _nextId = _maxId() + 1;
    _mode = _DragMode.none;
    _dragId = null;
  }

  /// 用户从布局库打开一份 → 原地替换。
  void _applySpec(LayoutSpec spec) => setState(() => _loadSpec(spec));

  /// 载入后重新起编号，否则新建框会和旧 id 撞（撞了以后 `_replace` 会改错框）。
  int _maxId() {
    var max = 0;
    for (final r in _regions) {
      final n = int.tryParse(r.id.replaceFirst(RegExp('^r'), ''));
      if (n != null && n > max) max = n;
    }
    return max;
  }

  Future<void> _confirmClear() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('清空画布'),
        content: const Text('框和场景描述都会被清掉。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('清空'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    setState(() {
      _regions.clear();
      _selectedId = null;
      _brief.clear();
    });
  }

  Widget _canvasArea(BuildContext context) {
    final theme = Theme.of(context);
    return LayoutBuilder(
      builder: (context, c) {
        const pad = 20.0;
        final maxW = math.max(c.maxWidth - pad * 2, 80.0);
        final maxH = math.max(c.maxHeight - pad * 2, 80.0);
        final ratio = _preset.w / _preset.h;
        var w = maxW;
        var h = w / ratio;
        if (h > maxH) {
          h = maxH;
          w = h * ratio;
        }
        final size = Size(w, h);
        return Stack(
          children: [
            Positioned.fill(
              child: Center(
                child: SizedBox(
                  width: w,
                  height: h,
                  child: GestureDetector(
                    key: const Key('layout-canvas'),
                    behavior: HitTestBehavior.opaque,
                    onTapDown: (d) => _onTapDown(d.localPosition, size),
                    onPanDown: (d) => _onPanDown(d.localPosition, size),
                    onPanStart: (d) => _onPanStart(d.localPosition, size),
                    onPanUpdate: (d) => _onPanUpdate(d.localPosition, size),
                    onPanEnd: (_) => _onPanEnd(),
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        border: Border.all(color: theme.colorScheme.outlineVariant),
                      ),
                      child: CustomPaint(
                        painter: _BoxPainter(
                          regions: _regions,
                          selectedId: _selectedId,
                          gridColor: theme.colorScheme.outlineVariant.withValues(alpha: 0.35),
                          thirdColor: theme.colorScheme.primary.withValues(alpha: 0.28),
                          background: theme.colorScheme.surfaceContainerLowest,
                          textColor: theme.colorScheme.onSurface,
                          surface: theme.colorScheme.surface,
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
            // 操作提示贴着画布上方，不占布局空间
            Positioned(
              left: 0,
              right: 0,
              top: 0,
              child: IgnorePointer(
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                  child: Text(
                    '空白处拖动 = 新建框 · 拖框体 = 移动 · 拖四角 / 四边 = 改大小',
                    textAlign: TextAlign.center,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }

  /// 从工作台进来时面板最上面那一块：正在改哪一版 + 提交入口。
  Widget _workspaceSubmit(BuildContext context) {
    final theme = Theme.of(context);
    final name = widget.initialName?.trim() ?? '';
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: theme.colorScheme.primaryContainer.withValues(alpha: 0.35),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            name.isEmpty ? '从 AI 工作台打开的布局' : '正在修改：$name',
            style: theme.textTheme.labelMedium,
          ),
          const SizedBox(height: 4),
          Text(
            '改完点下面「提交回工作台」，AI 会按这份框重写提示词。',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 8),
          FilledButton.icon(
            onPressed: _submitToWorkspace,
            icon: const Icon(Icons.send, size: 16),
            label: const Text('提交回工作台'),
          ),
        ],
      ),
    );
  }

  Widget _panel(BuildContext context) {
    final theme = Theme.of(context);
    final selected = _selected;
    final translation = _spec.translate();

    return ListView(
      padding: const EdgeInsets.fromLTRB(14, 14, 14, 28),
      children: [
        // 从工作台进来时，第一件事是"怎么交回去" —— 它比"场景大概画什么"更要紧。
        if (widget.fromWorkspace) ...[
          _workspaceSubmit(context),
          const SizedBox(height: 16),
        ],
        _sectionTitle(context, '场景大概画什么'),
        const SizedBox(height: 6),
        TextField(
          controller: _brief,
          minLines: 2,
          maxLines: 4,
          decoration: const InputDecoration(
            border: OutlineInputBorder(),
            isDense: true,
            hintText: '例：雾夜废墟，一名提灯的旅人正走近教堂',
          ),
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: 10),
        // 机位决定透视，不是构图：俯视才看得见桌面，平视才看得出两面墙的竖向关系
        Wrap(
          spacing: 6,
          runSpacing: 6,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            Text(
              '机位',
              style: theme.textTheme.labelSmall
                  ?.copyWith(color: theme.colorScheme.outline),
            ),
            for (final c in LayoutCamera.values)
              ChoiceChip(
                label: Text(c.label),
                selected: _camera == c,
                onSelected: (_) => setState(() => _camera = c),
              ),
          ],
        ),
        const SizedBox(height: 16),
        _sectionTitle(context, '框（${_regions.length}）'),
        if (_regions.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text(
              '在左边空白处拖出一个框，再回来给它起名字。',
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
            ),
          )
        else ...[
          const SizedBox(height: 6),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (var i = 0; i < _regions.length; i++)
                ChoiceChip(
                  label: Text('${i + 1}. ${_regions[i].displayName}'),
                  selected: _regions[i].id == _selectedId,
                  onSelected: (_) => setState(() => _selectedId = _regions[i].id),
                ),
            ],
          ),
        ],
        if (selected != null) ...[
          const SizedBox(height: 14),
          _regionEditor(context, selected),
        ],
        const SizedBox(height: 20),
        _sectionTitle(context, '输出'),
        const SizedBox(height: 8),
        Wrap(
          spacing: 10,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            FilledButton.tonalIcon(
              onPressed: _translating ? null : _translate,
              icon: _translating
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.auto_awesome, size: 18),
              label: Text(_translating ? '正在编译…' : '让 AI 写'),
            ),
            Text(
              _aiModel == null ? '译者：工作台当前模型' : '译者：$_aiModel',
              style: theme.textTheme.labelSmall
                  ?.copyWith(color: theme.colorScheme.outline),
            ),
          ],
        ),
        if (_aiPrompt != null) ...[
          const SizedBox(height: 12),
          if (_aiIssues.isNotEmpty) ...[
            _warningBox(context, _aiIssues),
            const SizedBox(height: 10),
          ],
          CopyableText(text: _aiPrompt, label: 'AI 提示词'),
        ],
        const SizedBox(height: 14),
        if (translation.warnings.isNotEmpty) ...[
          _warningBox(context, translation.warnings),
          const SizedBox(height: 12),
        ],
        CopyableText(text: translation.prose, label: '通用提示词（模板，兜底）'),
        const SizedBox(height: 14),
        CopyableText(
          text: translation.mossJson,
          label: 'MOSS 布局 JSON（专用）',
          monospace: true,
        ),
        const SizedBox(height: 10),
        Text(
          '专用 JSON 与 ComfyUI 的 MOSS-Image Layout Prompt 节点输出同构，'
          '可以直接当提示词文本喂给 MOSS-Image。\n'
          '通用提示词里，模型基本吃得下的是"在哪里"；"占多大"只是形容词，'
          '"镜中是不是同一个人"也只能靠关系字段明说 —— 这两类约束它可能不照做。',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
      ],
    );
  }

  Widget _regionEditor(BuildContext context, LayoutRegion r) {
    final index = _regions.indexWhere((x) => x.id == r.id) + 1;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text('第 $index 个框', style: Theme.of(context).textTheme.labelLarge),
              const Spacer(),
              IconButton(
                tooltip: '删除这个框',
                visualDensity: VisualDensity.compact,
                icon: const Icon(Icons.close, size: 18),
                onPressed: () => _remove(r.id),
              ),
            ],
          ),
          const SizedBox(height: 4),
          TextFormField(
            key: ValueKey('label-${r.id}'),
            initialValue: r.label,
            decoration: const InputDecoration(
              labelText: '名称',
              hintText: '例：提灯的旅人',
              isDense: true,
              border: OutlineInputBorder(),
            ),
            onChanged: (v) => _replace(r.id, label: v),
          ),
          const SizedBox(height: 10),
          TextFormField(
            key: ValueKey('caption-${r.id}'),
            initialValue: r.caption,
            minLines: 1,
            maxLines: 3,
            decoration: const InputDecoration(
              labelText: '描述（可留空，留空就用名称）',
              hintText: '例：提着一盏灯，从雾里走出来，灯光是唯一的暖色',
              isDense: true,
              border: OutlineInputBorder(),
            ),
            onChanged: (v) => _replace(r.id, caption: v),
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final k in LayoutBoxKind.values)
                ChoiceChip(
                  label: Text(k.displayName),
                  selected: r.kind == k,
                  onSelected: (_) => setState(() => _replace(r.id, kind: k)),
                ),
            ],
          ),
          // 关系：坐标说得出"在哪"，说不出"镜里那个是不是同一个人"。
          // 两个框都画对了、描述也都对，模型照样会画出两个人 —— 只能明说。
          const SizedBox(height: 12),
          Text(
            '挂在哪个面',
            style: Theme.of(context).textTheme.labelSmall?.copyWith(
                  color: Theme.of(context).colorScheme.outline,
                ),
          ),
          const SizedBox(height: 4),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final s in LayoutSurface.values)
                ChoiceChip(
                  label: Text(s.label),
                  selected: r.surface == s,
                  onSelected: (_) =>
                      setState(() => _replace(r.id, surface: s)),
                ),
            ],
          ),
          if (_regions.length > 1) ...[
            const SizedBox(height: 12),
            Text(
              '与其他框的关系',
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                    color: Theme.of(context).colorScheme.outline,
                  ),
            ),
            const SizedBox(height: 4),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final rel in LayoutRelation.values)
                  ChoiceChip(
                    label: Text(rel.label),
                    selected: r.relation == rel,
                    onSelected: (_) => setState(() {
                      if (rel == LayoutRelation.none) {
                        _replace(
                          r.id,
                          relation: LayoutRelation.none,
                          clearRelation: true,
                        );
                      } else {
                        final target =
                            r.relationTo ?? _others(r).first.id;
                        _replace(r.id, relation: rel, relationTo: target);
                      }
                    }),
                  ),
              ],
            ),
            if (r.relation != LayoutRelation.none) ...[
              const SizedBox(height: 6),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final o in _others(r))
                    ChoiceChip(
                      label: Text(o.displayName),
                      selected: r.relationTo == o.id,
                      onSelected: (_) =>
                          setState(() => _replace(r.id, relationTo: o.id)),
                    ),
                ],
              ),
            ],
          ],
          // 精确数值：一来能核对"我拖的地方"和"它记的位置"对不对，
          // 二来散文里的"宽约三成"就是从这几个数来的。
          Padding(
            padding: const EdgeInsets.only(top: 8),
            child: Text(
              'x ${(r.rect.x / 10).toStringAsFixed(1)}% · '
              'y ${(r.rect.y / 10).toStringAsFixed(1)}% · '
              '宽 ${(r.rect.w / 10).toStringAsFixed(1)}% · '
              '高 ${(r.rect.h / 10).toStringAsFixed(1)}%',
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                    color: Theme.of(context).colorScheme.outline,
                    fontFamily: 'monospace',
                  ),
            ),
          ),
          if (r.kind == LayoutBoxKind.text) ...[
            const SizedBox(height: 10),
            TextFormField(
              key: ValueKey('text-${r.id}'),
              initialValue: r.text,
              decoration: const InputDecoration(
                labelText: '文字内容',
                isDense: true,
                border: OutlineInputBorder(),
              ),
              onChanged: (v) => _replace(r.id, text: v),
            ),
          ],
          if (r.kind == LayoutBoxKind.blank)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                '留白不会进入 MOSS 布局 JSON，只会写进通用提示词。',
                style: Theme.of(context).textTheme.labelSmall?.copyWith(
                      color: Theme.of(context).colorScheme.outline,
                    ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _warningBox(BuildContext context, List<String> warnings) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: theme.colorScheme.tertiaryContainer.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.info_outline, size: 16, color: theme.colorScheme.onTertiaryContainer),
              const SizedBox(width: 6),
              Text(
                '翻译损失（${warnings.length}）',
                style: theme.textTheme.labelMedium?.copyWith(
                  color: theme.colorScheme.onTertiaryContainer,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          for (final w in warnings)
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Text(
                '· $w',
                style: theme.textTheme.bodySmall?.copyWith(
                  height: 1.45,
                  color: theme.colorScheme.onTertiaryContainer,
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _sectionTitle(BuildContext context, String text) {
    final theme = Theme.of(context);
    return Text(
      text,
      style: theme.textTheme.labelLarge?.copyWith(
        color: theme.colorScheme.primary,
        fontWeight: FontWeight.w700,
      ),
    );
  }
}

/// 把 0~1000 的框画出来。
///
/// 为什么用 `CustomPainter` 而不是给每个框建 widget（仓库里
/// `workflow_blueprint_painter.dart` 的那条结论）：框是纯图形，拖动时每帧都变，
/// 建 widget 等于每帧重建一棵子树；画布上还有个位数以上的框时，手感会先垮。
class _BoxPainter extends CustomPainter {
  _BoxPainter({
    required this.regions,
    required this.selectedId,
    required this.gridColor,
    required this.thirdColor,
    required this.background,
    required this.textColor,
    required this.surface,
  });

  final List<LayoutRegion> regions;
  final String? selectedId;
  final Color gridColor;
  final Color thirdColor;
  final Color background;
  final Color textColor;
  final Color surface;

  static Color _kindColor(LayoutBoxKind k) => layoutKindColor(k);

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = background);

    // 十分格 + 三分线：三分线是构图的实际参照，得比网格重一点
    final grid = Paint()
      ..color = gridColor
      ..strokeWidth = 1;
    for (var i = 1; i < 10; i++) {
      final dx = size.width * i / 10;
      final dy = size.height * i / 10;
      canvas.drawLine(Offset(dx, 0), Offset(dx, size.height), grid);
      canvas.drawLine(Offset(0, dy), Offset(size.width, dy), grid);
    }
    final third = Paint()
      ..color = thirdColor
      ..strokeWidth = 1.2;
    for (final f in [1 / 3, 2 / 3]) {
      canvas.drawLine(Offset(size.width * f, 0), Offset(size.width * f, size.height), third);
      canvas.drawLine(Offset(0, size.height * f), Offset(size.width, size.height * f), third);
    }

    for (var i = 0; i < regions.length; i++) {
      final r = regions[i];
      final selected = r.id == selectedId;
      final color = _kindColor(r.kind);
      final rect = Rect.fromLTWH(
        r.rect.x / 1000 * size.width,
        r.rect.y / 1000 * size.height,
        r.rect.w / 1000 * size.width,
        r.rect.h / 1000 * size.height,
      );

      canvas.drawRect(rect, Paint()..color = color.withValues(alpha: selected ? 0.22 : 0.12));
      canvas.drawRect(
        rect,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = selected ? 2.5 : 1.5
          ..color = color.withValues(alpha: selected ? 1 : 0.7),
      );

      // 序号徽标
      final badge = TextPainter(
        text: TextSpan(
          text: '${i + 1}',
          style: const TextStyle(fontSize: 11, fontWeight: FontWeight.w700, color: Colors.white),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      final badgeRect = Rect.fromLTWH(rect.left + 4, rect.top + 4, badge.width + 8, badge.height + 2);
      canvas.drawRRect(
        RRect.fromRectAndRadius(badgeRect, const Radius.circular(4)),
        Paint()..color = color,
      );
      badge.paint(canvas, Offset(badgeRect.left + 4, badgeRect.top + 1));

      // 名称
      final name = r.displayName;
      if (name.isNotEmpty && rect.width > 40) {
        final label = TextPainter(
          text: TextSpan(
            text: name,
            style: TextStyle(
              fontSize: 11.5,
              color: textColor,
              fontWeight: FontWeight.w600,
            ),
          ),
          textDirection: TextDirection.ltr,
          maxLines: 1,
          ellipsis: '…',
        )..layout(maxWidth: math.max(rect.width - 12, 10));
        label.paint(canvas, Offset(rect.left + 6, rect.top + 6 + badge.height + 4));
      }

      // 八个缩放手柄（四角 + 四边中点）
      if (selected) {
        for (final h in kLayoutHandles) {
          final p = Offset(
            (r.rect.x + r.rect.w * h.ax) / 1000 * size.width,
            (r.rect.y + r.rect.h * h.ay) / 1000 * size.height,
          );
          final radius = (h.ax == 0.5 || h.ay == 0.5) ? 4.5 : 5.5;
          canvas.drawCircle(p, radius, Paint()..color = surface);
          canvas.drawCircle(
            p,
            radius,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 2
              ..color = color,
          );
        }
      }
    }
  }

  /// ⚠ 必须恒为 true：`regions` 是页面里**原地修改**的那同一个 List，
  /// 拖框时引用没变、内容变了。按引用比较的表现是"框拖了不动，松手才出现"
  /// （松手那次因为 `selectedId` 变化碰巧重绘了）。
  @override
  bool shouldRepaint(_BoxPainter old) => true;
}
