/// 工作流「蓝图」的客户端模型（真源是后端 `server-rs/src/workflow/workflow_blueprint.rs`）。
///
/// 这里**只做三件事**：把后端给的那份 JSON 读成对象、算一个能画的布局、以及
/// 按**节点 id** 把用户的改动写回原文。判据（哪个插口必填、类型对不对、本机有没有这个类）
/// 一条都不在这里 —— 那些要 ComfyUI 的 `/object_info`，只有后端问得到。
///
/// # 为什么编辑是"改原文"而不是"改模型"
///
/// 原文（`prompts.workflow_json` / 那份拖回 ComfyUI 的 JSON）是**唯一真源**，
/// 蓝图是它的一个**视图**。所以编辑一律走 [WorkflowEditor]：按 id 找到节点、
/// 只改那一个字段，其余字节原样保留 —— 前端 only 节点（`Note` / `Reroute` / 广播 …）
/// 因此天然不会被弄丢，也不需要在这里复刻一遍序列化规则。
library;

import 'dart:convert';
import 'dart:math' as math;
import 'dart:ui' show Rect, Size;

/// `POST /api/workflow/save` 的结果（两个客户端共用，所以放在这里而不是某个客户端里）。
class SavedWorkflow {
  final String path;
  final int bytes;
  /// 下一步该怎么办（后端给的一句人话，界面直接展示）
  final String hint;

  const SavedWorkflow({required this.path, required this.bytes, required this.hint});

  factory SavedWorkflow.fromJson(Map<String, dynamic> json) => SavedWorkflow(
        path: (json['path'] ?? '').toString(),
        bytes: (json['bytes'] as num?)?.toInt() ?? 0,
        hint: (json['hint'] ?? '').toString(),
      );
}

/// 节点状态（litegraph）：0 正常 / 2 静音 / 4 旁路。
const int kNodeModeNormal = 0;
const int kNodeModeMuted = 2;
const int kNodeModeBypassed = 4;

/// 一条插口。
class BlueprintPort {
  final String name;
  final String type;
  /// 输入：接的是哪条线（null = 空着）
  final int? link;
  /// 输出：下游的线
  final List<int> links;
  /// 这个输入是控件（下拉框 / 数字框），不是一个插口
  final bool widget;
  final Object? value;

  const BlueprintPort({
    required this.name,
    required this.type,
    this.link,
    this.links = const [],
    this.widget = false,
    this.value,
  });

  factory BlueprintPort.fromJson(Map<String, dynamic> json) => BlueprintPort(
        name: (json['name'] ?? '').toString(),
        type: (json['type'] ?? '').toString(),
        link: (json['link'] as num?)?.toInt(),
        links: ((json['links'] as List?) ?? const [])
            .map((item) => (item as num).toInt())
            .toList(growable: false),
        widget: json['widget'] == true,
        value: json['value'],
      );

  /// 空着的真插口（不是控件、也没接线）—— 广播节点就是往这些地方灌值的。
  bool get isOpenSocket => !widget && link == null;
}

/// 一个控件（按 `widgets_values` 的位置）。
class BlueprintWidget {
  final int index;
  /// null = 只有位置、没有名字（旧存档 + ComfyUI 不在线时会出现，那时**不猜名字**）
  final String? name;
  final Object? value;

  const BlueprintWidget({required this.index, this.name, this.value});

  factory BlueprintWidget.fromJson(Map<String, dynamic> json) => BlueprintWidget(
        index: (json['index'] as num?)?.toInt() ?? 0,
        name: json['name']?.toString(),
        value: json['value'],
      );

  /// 界面上给这一项起的标题：有名字用名字，没有就如实说"参数 #n"。
  String get label => (name == null || name!.isEmpty) ? '参数 #$index' : name!;
}

/// 一个节点。
class BlueprintNode {
  final String id;
  final String className;
  final String title;
  /// 界面格式才有；API 格式没有坐标（前端自动排）
  final double? x;
  final double? y;
  final double? width;
  final double? height;
  final int mode;
  /// normal / note / reroute / variable / broadcast / display / bypasser / subgraph / unknown
  final String kind;
  /// 服务端没有这个类（纯前端节点，或没装的自定义节点包）
  final bool frontendOnly;
  final List<BlueprintPort> inputs;
  final List<BlueprintPort> outputs;
  final List<BlueprintWidget> widgets;

  const BlueprintNode({
    required this.id,
    required this.className,
    required this.title,
    this.x,
    this.y,
    this.width,
    this.height,
    this.mode = kNodeModeNormal,
    this.kind = 'normal',
    this.frontendOnly = false,
    this.inputs = const [],
    this.outputs = const [],
    this.widgets = const [],
  });

  factory BlueprintNode.fromJson(Map<String, dynamic> json) {
    List<double>? pair(Object? raw) {
      if (raw is! List || raw.length < 2) return null;
      final a = (raw[0] as num?)?.toDouble();
      final b = (raw[1] as num?)?.toDouble();
      if (a == null || b == null) return null;
      return [a, b];
    }

    final pos = pair(json['pos']);
    final size = pair(json['size']);
    return BlueprintNode(
      id: (json['id'] ?? '').toString(),
      className: (json['class'] ?? '').toString(),
      title: (json['title'] ?? '').toString(),
      x: pos?[0],
      y: pos?[1],
      width: size?[0],
      height: size?[1],
      mode: (json['mode'] as num?)?.toInt() ?? kNodeModeNormal,
      kind: (json['kind'] ?? 'normal').toString(),
      frontendOnly: json['frontendOnly'] == true,
      inputs: ((json['inputs'] as List?) ?? const [])
          .map((item) => BlueprintPort.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      outputs: ((json['outputs'] as List?) ?? const [])
          .map((item) => BlueprintPort.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      widgets: ((json['widgets'] as List?) ?? const [])
          .map((item) => BlueprintWidget.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
    );
  }

  bool get bypassed => mode == kNodeModeBypassed;
  bool get muted => mode == kNodeModeMuted;
  /// 不参与执行的那几种（注释框 / 转发点 / 纯前端）
  bool get decorative =>
      kind == 'note' || kind == 'reroute' || kind == 'variable' || frontendOnly;

  /// 界面上显示的标题（空标题回落到类名）。
  String get displayTitle => title.trim().isEmpty ? className : title;
}

/// 一条连线。
class BlueprintEdge {
  final int? id;
  final String fromNode;
  final int fromSlot;
  final String toNode;
  final int toSlot;
  final String? type;
  /// API 格式的边没有 id（是从 `inputs` 反推出来的）
  final bool implicit;

  const BlueprintEdge({
    this.id,
    required this.fromNode,
    required this.fromSlot,
    required this.toNode,
    required this.toSlot,
    this.type,
    this.implicit = false,
  });

  factory BlueprintEdge.fromJson(Map<String, dynamic> json) {
    final from = Map<String, dynamic>.from(json['from'] as Map? ?? const {});
    final to = Map<String, dynamic>.from(json['to'] as Map? ?? const {});
    return BlueprintEdge(
      id: (json['id'] as num?)?.toInt(),
      fromNode: (from['node'] ?? '').toString(),
      fromSlot: (from['slot'] as num?)?.toInt() ?? 0,
      toNode: (to['node'] ?? '').toString(),
      toSlot: (to['slot'] as num?)?.toInt() ?? 0,
      type: json['type']?.toString(),
      implicit: json['implicit'] == true,
    );
  }
}

/// 一个分组框（界面格式才有）。
class BlueprintGroup {
  final String title;
  final List<double> bounding;
  final String? color;

  const BlueprintGroup({required this.title, this.bounding = const [], this.color});

  factory BlueprintGroup.fromJson(Map<String, dynamic> json) => BlueprintGroup(
        title: (json['title'] ?? '').toString(),
        bounding: ((json['bounding'] as List?) ?? const [])
            .map((item) => (item as num).toDouble())
            .toList(growable: false),
        color: json['color']?.toString(),
      );

  bool get valid => bounding.length >= 4;
  double get left => bounding[0];
  double get top => bounding[1];
  double get width => bounding[2];
  double get height => bounding[3];
}

/// 一条接线体检结论。
class BlueprintIssue {
  final String code;
  /// error / warn / info
  final String severity;
  final String title;
  final String detail;
  final String? fix;
  final List<String> nodes;
  final List<int> links;
  /// `33.model` 这种"节点.输入名"，界面据此把标记画到具体插口上
  final List<String> slots;

  const BlueprintIssue({
    required this.code,
    required this.severity,
    required this.title,
    required this.detail,
    this.fix,
    this.nodes = const [],
    this.links = const [],
    this.slots = const [],
  });

  factory BlueprintIssue.fromJson(Map<String, dynamic> json) => BlueprintIssue(
        code: (json['code'] ?? '').toString(),
        severity: (json['severity'] ?? 'info').toString(),
        title: (json['title'] ?? '').toString(),
        detail: (json['detail'] ?? '').toString(),
        fix: json['fix']?.toString(),
        nodes: ((json['nodes'] as List?) ?? const []).map((e) => e.toString()).toList(growable: false),
        links: ((json['links'] as List?) ?? const [])
            .map((e) => (e as num).toInt())
            .toList(growable: false),
        slots: ((json['slots'] as List?) ?? const []).map((e) => e.toString()).toList(growable: false),
      );

  bool get isError => severity == 'error';
  bool get isWarn => severity == 'warn';
}

/// 一份蓝图。
class WorkflowBlueprint {
  /// ui / api / unknown
  final String format;
  final String? title;
  /// 后端有没有拿到 ComfyUI 的 `/object_info`（拿不到时体检的覆盖面小一圈）
  final bool objectInfo;
  final List<BlueprintNode> nodes;
  final List<BlueprintEdge> edges;
  final List<BlueprintGroup> groups;
  final List<BlueprintIssue> issues;
  final List<String> warnings;
  final int errorCount;
  final int warnCount;

  WorkflowBlueprint({
    required this.format,
    this.title,
    this.objectInfo = false,
    required this.nodes,
    required this.edges,
    required this.groups,
    required this.issues,
    required this.warnings,
    required this.errorCount,
    required this.warnCount,
  });

  factory WorkflowBlueprint.fromJson(Map<String, dynamic> json) {
    final counts = Map<String, dynamic>.from(json['counts'] as Map? ?? const {});
    return WorkflowBlueprint(
      format: (json['format'] ?? 'unknown').toString(),
      title: json['title']?.toString(),
      objectInfo: json['objectInfo'] == true,
      nodes: ((json['nodes'] as List?) ?? const [])
          .map((item) => BlueprintNode.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      edges: ((json['edges'] as List?) ?? const [])
          .map((item) => BlueprintEdge.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      groups: ((json['groups'] as List?) ?? const [])
          .map((item) => BlueprintGroup.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      issues: ((json['issues'] as List?) ?? const [])
          .map((item) => BlueprintIssue.fromJson(Map<String, dynamic>.from(item as Map)))
          .toList(growable: false),
      warnings: ((json['warnings'] as List?) ?? const [])
          .map((item) => item.toString())
          .toList(growable: false),
      errorCount: (counts['errors'] as num?)?.toInt() ?? 0,
      warnCount: (counts['warnings'] as num?)?.toInt() ?? 0,
    );
  }

  bool get isUi => format == 'ui';
  bool get isApi => format == 'api';
  bool get known => format != 'unknown';

  BlueprintNode? nodeById(String id) {
    for (final node in nodes) {
      if (node.id == id) return node;
    }
    return null;
  }

  /// 挂在某个节点上的结论（界面用它给节点框上色 / 打角标）。
  List<BlueprintIssue> issuesForNode(String id) =>
      issues.where((issue) => issue.nodes.contains(id)).toList(growable: false);

  /// 某个节点的某个插口上有没有结论（`33.model`）。
  List<BlueprintIssue> issuesForSlot(String nodeId, String name) => issues
      .where((issue) => issue.slots.contains('$nodeId.$name'))
      .toList(growable: false);

  /// 最严重的一档（error > warn > info > none）。
  String severityForNode(String id) {
    final hit = issuesForNode(id);
    if (hit.any((issue) => issue.isError)) return 'error';
    if (hit.any((issue) => issue.isWarn)) return 'warn';
    if (hit.isNotEmpty) return 'info';
    return 'none';
  }

  /// 位置是不是后端**算**出来的（API 格式没有坐标）。
  bool get autoLayout => isApi;
}

/// 布局结果：每个节点一个矩形（图坐标）。
class BlueprintLayout {
  final Map<String, Rect> boxes;
  final Rect bounds;

  const BlueprintLayout({required this.boxes, required this.bounds});

  Rect? boxOf(String id) => boxes[id];

  /// 计算布局：界面格式用**文件里的真实坐标**，API 格式按最长路径分层排。
  ///
  /// 判据与取舍（研究文档 §3.4）：
  /// * 界面格式的坐标可能很大（实测包围盒 9600×2400）也可能为负 —— 原样用，
  ///   由视口去适配（**不许**在画布上假装"它就是 ComfyUI 里的样子"以外的东西）；
  /// * API 格式**没有坐标**：分层（`layer = 1 + max(上游)`）+ 同层按上游重心排序，
  ///   结果必须是**确定性的**（同一输入两次得到相同坐标），界面上要如实标注"位置是排出来的"。
  static BlueprintLayout of(WorkflowBlueprint blueprint) {
    if (blueprint.nodes.isEmpty) {
      return const BlueprintLayout(boxes: {}, bounds: Rect.zero);
    }
    return blueprint.isApi ? _layered(blueprint) : _fromPositions(blueprint);
  }

  static BlueprintLayout _fromPositions(WorkflowBlueprint blueprint) {
    final boxes = <String, Rect>{};
    var left = double.infinity;
    var top = double.infinity;
    var right = -double.infinity;
    var bottom = -double.infinity;
    for (final node in blueprint.nodes) {
      final size = _boxSize(node);
      // 没有坐标的节点（理论上不该有）排到原点右边，免得整个画布塌成一点
      final x = node.x ?? (boxes.length * 40.0);
      final y = node.y ?? 0;
      final rect = Rect.fromLTWH(x, y, size.width, size.height);
      boxes[node.id] = rect;
      left = math.min(left, rect.left);
      top = math.min(top, rect.top);
      right = math.max(right, rect.right);
      bottom = math.max(bottom, rect.bottom);
    }
    return BlueprintLayout(
      boxes: boxes,
      bounds: Rect.fromLTRB(left, top, right, bottom),
    );
  }

  static BlueprintLayout _layered(WorkflowBlueprint blueprint) {
    final order = <String, int>{};
    for (var i = 0; i < blueprint.nodes.length; i++) {
      order[blueprint.nodes[i].id] = i;
    }
    final upstream = <String, List<String>>{};
    final downstream = <String, List<String>>{};
    for (final edge in blueprint.edges) {
      if (!order.containsKey(edge.fromNode) || !order.containsKey(edge.toNode)) continue;
      upstream.putIfAbsent(edge.toNode, () => []).add(edge.fromNode);
      downstream.putIfAbsent(edge.fromNode, () => []).add(edge.toNode);
    }

    // 分层：Kahn 拓扑序，`layer = 1 + max(上游)`；有环时把环上的节点放到最后一层（不死循环）
    final layer = <String, int>{};
    final pending = <String, int>{};
    for (final node in blueprint.nodes) {
      pending[node.id] = (upstream[node.id] ?? const []).length;
      // ⚠ 先给**所有**节点一个 0：队列头（没有上游的那些）如果不定层，
      // 后面那条"环上剩下的补到最后一层"会把它们当成环上的节点推到最右边 ——
      // 表现是"上游跑到下游右边"，而布局是算出来的、不会报错。
      layer[node.id] = 0;
    }
    final queue = <String>[
      for (final node in blueprint.nodes)
        if (pending[node.id] == 0) node.id,
    ];
    var head = 0;
    while (head < queue.length) {
      final id = queue[head++];
      for (final next in downstream[id] ?? const <String>[]) {
        final candidate = (layer[id] ?? 0) + 1;
        if (candidate > (layer[next] ?? 0)) layer[next] = candidate;
        pending[next] = (pending[next] ?? 1) - 1;
        if (pending[next] == 0) queue.add(next);
      }
    }
    // 环上剩下的（没进过队列）按文档顺序补到最后一层
    final maxLayer = layer.values.fold<int>(0, math.max);
    for (final node in blueprint.nodes) {
      layer.putIfAbsent(node.id, () => maxLayer + 1);
    }

    final byLayer = <int, List<String>>{};
    for (final node in blueprint.nodes) {
      byLayer.putIfAbsent(layer[node.id]!, () => []).add(node.id);
    }
    final layers = byLayer.keys.toList()..sort();
    final boxes = <String, Rect>{};
    // 层内位置用上游的**平均 y**（重心）排一遍，减少明显交叉
    final centre = <String, double>{};
    for (final id in order.keys) {
      centre[id] = 0;
    }
    const layerGap = 320.0;
    const rowGap = 120.0;
    var x = 0.0;
    var maxY = 0.0;
    for (final key in layers) {
      final ids = byLayer[key]!;
      ids.sort((a, b) {
        final ca = _barycentre(a, upstream, centre, order);
        final cb = _barycentre(b, upstream, centre, order);
        if (ca != cb) return ca.compareTo(cb);
        return (order[a] ?? 0).compareTo(order[b] ?? 0);
      });
      var y = 0.0;
      var widest = 0.0;
      for (final id in ids) {
        final node = blueprint.nodes[order[id]!];
        final size = _boxSize(node);
        boxes[id] = Rect.fromLTWH(x, y, size.width, size.height);
        centre[id] = y + size.height / 2;
        y += size.height + rowGap;
        widest = math.max(widest, size.width);
      }
      maxY = math.max(maxY, y);
      x += widest + layerGap;
    }
    return BlueprintLayout(
      boxes: boxes,
      bounds: Rect.fromLTRB(0, 0, x, maxY),
    );
  }

  static double _barycentre(
    String id,
    Map<String, List<String>> upstream,
    Map<String, double> centre,
    Map<String, int> order,
  ) {
    final parents = upstream[id];
    if (parents == null || parents.isEmpty) return (order[id] ?? 0).toDouble();
    var sum = 0.0;
    var count = 0;
    for (final parent in parents) {
      final value = centre[parent];
      if (value == null) continue;
      sum += value;
      count++;
    }
    if (count == 0) return (order[id] ?? 0).toDouble();
    return sum / count;
  }

  /// 节点框的尺寸：界面格式有真实 `size`（实测 140×46 ~ 7547×300），
  /// 没有就按标题长度估一个（API 格式只能估 —— 界面上要如实标注）。
  static Size _boxSize(BlueprintNode node) {
    var width = node.width ?? 0;
    var height = node.height ?? 0;
    if (width <= 0) {
      // 标题按**字符数**估宽（中文比英文宽，取个折中）
      final chars = node.displayTitle.runes.length;
      width = (chars * 8.5 + 40).clamp(160.0, 460.0).toDouble();
    }
    if (height <= 0) {
      final rows = math.max(node.inputs.length, node.outputs.length);
      height = (30 + rows * 20 + node.widgets.length * 18).clamp(50.0, 400.0).toDouble();
    }
    return Size(width, height);
  }
}

/// 在原文上做**按 id 定位**的小改动。
///
/// 只有三种改动（改控件值 / 改节点状态 / 改标题），每一种都只碰那一个字段：
/// 工作流原文里还有一大堆我们**不该动**的东西（前端 only 节点、坐标、分组、flags、
/// `widgets_values_named`…），整份重新序列化一次就可能把它们弄丢。
class WorkflowEditor {
  /// 原文解析出来的 JSON（顶层必须是对象）
  final Map<String, dynamic> root;

  WorkflowEditor._(this.root);

  /// 解析原文；不是对象 / 不是 JSON 就回 null（界面据此禁用编辑）。
  static WorkflowEditor? parse(String raw) {
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return null;
      return WorkflowEditor._(Map<String, dynamic>.from(decoded));
    } catch (_) {
      return null;
    }
  }

  bool get isUiFormat => root['nodes'] is List;

  /// 这份原文能不能改（只有界面格式有 `nodes` 数组；API 格式的参数键名不在这里）。
  bool get editable => isUiFormat;

  /// 节点数（界面上"这份图有几个节点"用它）。
  int get nodeCount {
    final raw = root['nodes'];
    return raw is List ? raw.length : 0;
  }

  int? _indexOf(String id) {
    final raw = root['nodes'];
    if (raw is! List) return null;
    for (var i = 0; i < raw.length; i++) {
      final node = raw[i];
      if (node is Map && node['id'].toString() == id) return i;
    }
    return null;
  }

  /// 改一个控件值（按 `widgets_values` 的位置）。
  ///
  /// ⚠ 位置那份**永远改**；带名字那份（`widgets_values_named`，新版前端会存、
  /// 而且后端**优先读它**）能对上名字就一起改。对不上名字时把它整份删掉 ——
  /// 留着旧值会让这次编辑**静默失效**（界面看着改了、转出来还是旧值），
  /// 删掉之后后端会退回按 `/object_info` 的声明顺序读位置那份，而那份刚改过。
  bool setWidgetValue(String nodeId, int index, Object? value) {
    final at = _indexOf(nodeId);
    if (at == null) return false;
    final node = (root['nodes'] as List)[at] as Map;
    final values = node['widgets_values'];
    if (values is! List || index < 0 || index >= values.length) return false;
    values[index] = value;
    final named = node['widgets_values_named'];
    if (named is Map && named.isNotEmpty) {
      final names = _widgetNamesOf(node, named, values.length);
      final name = names == null ? null : names[index];
      if (name != null && named.containsKey(name)) {
        named[name] = value;
      } else {
        node.remove('widgets_values_named');
      }
    }
    return true;
  }

  /// 位置 → 控件名（对不上就回 `null`，调用方据此删掉带名字的那份）。
  ///
  /// 两个来源，按可靠性排：
  ///  1. **`widgets_values_named` 自己的键序**（前端的插入序就是控件顺序，长度对得上才敢用）；
  ///  2. 节点 `inputs[].widget.name` 的顺序（`control_after_generate` 那种"多占一位"的
  ///     下拉框不在 `inputs` 里，所以长度常常对不上 —— 那时回 `null`，**不猜**）。
  List<String?>? _widgetNamesOf(Map node, Map named, int count) {
    if (named.length == count) {
      return named.keys.map((key) => key.toString()).toList(growable: false);
    }
    final inputs = node['inputs'];
    if (inputs is! List) return null;
    final names = <String?>[];
    for (final entry in inputs) {
      if (entry is! Map) continue;
      final widget = entry['widget'];
      if (widget is Map) {
        names.add((widget['name'] ?? entry['name'])?.toString());
      }
    }
    return names.length == count ? names : null;
  }

  /// 改节点状态（0 正常 / 2 静音 / 4 旁路）。
  bool setNodeMode(String nodeId, int mode) {
    final at = _indexOf(nodeId);
    if (at == null) return false;
    final node = (root['nodes'] as List)[at] as Map;
    if (node['mode'] == mode) return false;
    node['mode'] = mode;
    return true;
  }

  /// 改节点标题（空串 = 删掉自定义标题，回落到类名）。
  bool setNodeTitle(String nodeId, String title) {
    final at = _indexOf(nodeId);
    if (at == null) return false;
    final node = (root['nodes'] as List)[at] as Map;
    final trimmed = title.trim();
    if (trimmed.isEmpty) {
      if (!node.containsKey('title')) return false;
      node.remove('title');
      return true;
    }
    if (node['title'] == trimmed) return false;
    node['title'] = trimmed;
    return true;
  }

  /// 改节点坐标（在蓝图上拖动节点）。
  bool setNodePosition(String nodeId, double x, double y) {
    final at = _indexOf(nodeId);
    if (at == null) return false;
    final node = (root['nodes'] as List)[at] as Map;
    final pos = node['pos'];
    if (pos is List && pos.length >= 2) {
      if (pos[0] == x && pos[1] == y) return false;
      pos[0] = x;
      pos[1] = y;
      return true;
    }
    node['pos'] = [x, y];
    return true;
  }

  /// 序列化回原文（缩进与后端存的那份一致：两格）。
  String toJson() => const JsonEncoder.withIndent('  ').convert(root);
}
