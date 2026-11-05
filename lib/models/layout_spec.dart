import 'dart:convert';

/// 宽容地把 JSON 里的数字读成 int（缺省 / 脏值都回 [fallback]）。
int _asInt(Object? v, [int fallback = 0]) {
  if (v is int) return v;
  if (v is num) return v.round();
  if (v is String) return int.tryParse(v) ?? fallback;
  return fallback;
}

/// 框挂在房间的哪个面上。
///
/// 只有 x/y 的时候，模型只能把所有东西摊在**一面墙**前 —— 镜面、窗、床挤成一个
/// 平面构图（用户 2026-11-01："我要的不是这种平面场景，比如镜面在一堵墙上，
/// 而床和窗户在紧挨着的另一堵墙"）。
/// 给每个框一个"属于哪个面"，"两面墙垂直"这种关系才说得出来。
enum LayoutSurface {
  none('', '未指定', ''),
  leftWall('left', '左墙', '挂在左墙'),
  rightWall('right', '右墙', '挂在右墙'),
  backWall('back', '正墙', '挂在正墙'),
  floor('floor', '地面', '立在地面'),
  table('table', '桌面', '摆在桌面');

  const LayoutSurface(this.id, this.label, this.phrase);

  final String id;
  final String label;

  /// 散文里的说法（[none] 为空，表示不提这个面）。
  final String phrase;
}

/// 机位。俯视 / 平视 / 仰视决定的是**透视**，不是构图 —— 同一份框，换个机位
/// 就该换一套空间描述（俯视才看得见桌面，平视才看得见墙面竖向关系）。
enum LayoutCamera {
  auto('auto', '不指定'),
  topdown('topdown', '俯视'),
  eye('eye', '平视'),
  low('low', '仰视');

  const LayoutCamera(this.id, this.label);

  final String id;
  final String label;

  String get phrase => this == LayoutCamera.auto ? '' : '$label视角';
}

/// 布局框的用途。
///
/// 前三种与 MOSS-Image 布局 JSON 的 `label` 一一对应：
/// ComfyUI 的 `comfy_extras/nodes_moss_image.py` 只认 `object` / `character` /
/// `text` 这三档（`character_boxes` 里点过名的算角色，其余算物体）。
///
/// [blank]（留白）是**我们加的第四档**，那套格式里没有对应值 —— 留白没有"内容"，
/// 进不了 `items`。这不是缺陷，是两种输出表达能力不同：**留白只活在散文里**，
/// 翻译时必须把它说清楚，而不是悄悄丢掉。
enum LayoutBoxKind {
  object('object', '物体'),
  character('character', '角色'),
  text('text', '文字'),
  blank('', '留白');

  const LayoutBoxKind(this.mossLabel, this.displayName);

  /// MOSS-Image 布局 JSON 里的 `label`；[blank] 没有对应值。
  final String mossLabel;

  /// 界面上的叫法。
  final String displayName;

  /// 能不能进专用输出（布局 JSON）。
  bool get isSerializable => mossLabel.isNotEmpty;
}

/// 两个框之间的关系。
///
/// 坐标只说得出"在哪里"，说不出"镜子里那个是不是同一个人" —— 那是**一致性**，
/// 不是位置。用户 2026-11-01 的实际结果是："镜中人低头拨头发，本人却在挠头"：
/// 两个框各自描述得都对，只是没人告诉模型它们是同一个主体。
///
/// 所以关系必须单独说，而且**两条出口都要说**（散文一句、JSON 里的 caption 一句）。
enum LayoutRelation {
  none('', '无'),
  mirror('mirror', '镜像 / 倒影'),
  same('same', '同一主体'),
  inFront('front', '挡在前面');

  const LayoutRelation(this.id, this.label);

  final String id;
  final String label;
}

/// 一块布局区域，单位 **0~1000 的规范化坐标**（左上角为原点，不含右/下边界）。
///
/// 这个单位不是随手挑的：ComfyUI 的 `Create Bounding Boxes` 用它，
/// `MOSS-Image Layout Prompt` 也只在**输出时**才换算成像素。**内部单位 = 上游单位**，
/// 序列化就只剩一次纯整数换算，没有"0~1 / 0~1000 / 像素"三套并存的机会
/// （三套并存的那种代码，早迟会有人把 0.5 当成 500）。
class LayoutRect {
  const LayoutRect(this.x, this.y, this.w, this.h);

  final int x;
  final int y;
  final int w;
  final int h;

  int get right => x + w;
  int get bottom => y + h;
  int get centerX => x + w ~/ 2;
  int get centerY => y + h ~/ 2;

  /// 占画面面积的比例（0~1）。
  double get areaFraction => (w * h) / 1000000.0;

  bool contains(int px, int py) =>
      px >= x && px <= right && py >= y && py <= bottom;

  LayoutRect translate(int dx, int dy) => LayoutRect(x + dx, y + dy, w, h);

  /// 夹回 0~1000 的画布内。
  ///
  /// 移动与缩放都要过它：不夹的话框会飞出画布，而**画出界的框在专用 JSON 里
  /// 仍然是合法数字**（坐标是中心相对的，负得再多也不报错），只是生成出来对不上 ——
  /// 这种错误不报错、只在出图时显形，最难查。
  LayoutRect clampedInside() {
    final cw = w.clamp(1, 1000);
    final ch = h.clamp(1, 1000);
    final nx = x.clamp(0, 1000 - cw);
    final ny = y.clamp(0, 1000 - ch);
    return LayoutRect(nx, ny, cw, ch);
  }

  /// 与另一块的交集面积（0~1）。
  double intersectionFraction(LayoutRect other) {
    final left = x > other.x ? x : other.x;
    final top = y > other.y ? y : other.y;
    final r = right < other.right ? right : other.right;
    final b = bottom < other.bottom ? bottom : other.bottom;
    if (r <= left || b <= top) return 0;
    return ((r - left) * (b - top)) / 1000000.0;
  }

  /// IoU：交集 / 并集（0~1）。
  double iou(LayoutRect other) {
    final inter = intersectionFraction(other);
    if (inter <= 0) return 0;
    final union = areaFraction + other.areaFraction - inter;
    return union <= 0 ? 0 : inter / union;
  }

  @override
  bool operator ==(Object other) =>
      other is LayoutRect &&
      other.x == x &&
      other.y == y &&
      other.w == w &&
      other.h == h;

  @override
  int get hashCode => Object.hash(x, y, w, h);

  @override
  String toString() => 'LayoutRect($x,$y,$w,$h)';
}

/// 画布上的一个框。
class LayoutRegion {
  const LayoutRegion({
    required this.id,
    required this.rect,
    this.kind = LayoutBoxKind.object,
    this.label = '',
    this.caption = '',
    this.text = '',
    this.relation = LayoutRelation.none,
    this.relationTo,
    this.surface = LayoutSurface.none,
  });

  final String id;
  final LayoutRect rect;
  final LayoutBoxKind kind;

  /// 短名称（"提灯的旅人"）。散文里优先用它。
  final String label;

  /// 更长的描述（"提着一盏灯，从雾里走出来"）。留空时退回 [label]。
  final String caption;

  /// 文字框里的字（只有 [LayoutBoxKind.text] 用）。
  final String text;

  /// 与另一个框的关系。
  final LayoutRelation relation;

  /// 关系指向的框 id（[relation] 为 [LayoutRelation.none] 时无意义）。
  final String? relationTo;

  /// 挂在房间的哪个面上（[LayoutSurface.none] = 没说）。
  final LayoutSurface surface;

  /// 散文里怎么称呼它。
  String get displayName {
    final l = label.trim();
    if (l.isNotEmpty) return l;
    return kind == LayoutBoxKind.blank ? '留白' : '未命名的区域';
  }

  /// 散文里的具体描述：描述优先，退回名称。
  ///
  /// 两者都有时**名字要在场** —— 用户写"提着灯从雾里走出来"这种描述时，
  /// 十有八九没把主语再写一遍，只取描述就会把"是谁"弄丢。
  String get detail {
    final c = caption.trim();
    final l = label.trim();
    if (c.isEmpty) return l.isEmpty ? '一处未命名的物体' : l;
    if (l.isEmpty) return c;
    return c.contains(l) ? c : '$l，$c';
  }

  LayoutRegion copyWith({
    LayoutRect? rect,
    LayoutBoxKind? kind,
    String? label,
    String? caption,
    String? text,
    LayoutRelation? relation,
    String? relationTo,
    bool clearRelation = false,
    LayoutSurface? surface,
  }) =>
      LayoutRegion(
        id: id,
        rect: rect ?? this.rect,
        kind: kind ?? this.kind,
        label: label ?? this.label,
        caption: caption ?? this.caption,
        text: text ?? this.text,
        relation: relation ?? this.relation,
        relationTo: clearRelation ? null : (relationTo ?? this.relationTo),
        surface: surface ?? this.surface,
      );

  Map<String, Object?> toJson() => <String, Object?>{
        'id': id,
        'x': rect.x,
        'y': rect.y,
        'w': rect.w,
        'h': rect.h,
        'kind': kind.name,
        'label': label,
        'caption': caption,
        'text': text,
        'relation': relation.name,
        'relationTo': relationTo,
        'surface': surface.id,
      };

  /// 读一份存下来的框。**认不出来的字段一律回默认，绝不抛** ——
  /// 这些 JSON 会被用户手改，也会被老版本写坏，读的时候崩掉等于布局全丢。
  static LayoutRegion fromJson(Map<String, Object?> json) => LayoutRegion(
        id: (json['id'] as String?) ?? 'r0',
        rect: LayoutRect(
          _asInt(json['x']),
          _asInt(json['y']),
          _asInt(json['w'], 200),
          _asInt(json['h'], 200),
        ).clampedInside(),
        kind: LayoutBoxKind.values.firstWhere(
          (k) => k.name == json['kind'],
          orElse: () => LayoutBoxKind.object,
        ),
        label: (json['label'] as String?) ?? '',
        caption: (json['caption'] as String?) ?? '',
        text: (json['text'] as String?) ?? '',
        relation: LayoutRelation.values.firstWhere(
          (r) => r.name == json['relation'],
          orElse: () => LayoutRelation.none,
        ),
        relationTo: json['relationTo'] as String?,
        surface: LayoutSurface.values.firstWhere(
          (s) => s.id == json['surface'],
          orElse: () => LayoutSurface.none,
        ),
      );
}

/// 翻译结果：两条出口 + 一份"我损失了什么"的清单。
class LayoutTranslation {
  const LayoutTranslation({
    required this.prose,
    required this.mossJson,
    required this.warnings,
  });

  /// 通用提示词（散文）：任何模型都能读，但有信息上限。
  final String prose;

  /// 专用提示词：MOSS-Image 布局 JSON，坐标无损。
  final String mossJson;

  /// 保真度提示。**不写进提示词正文** —— 正文是给模型看的，这些是给人看的。
  final List<String> warnings;
}

/// 一整张画布的布局。
class LayoutSpec {
  const LayoutSpec({
    required this.width,
    required this.height,
    this.brief = '',
    this.regions = const [],
    this.camera = LayoutCamera.auto,
  });

  /// 出图像素宽（不是画布控件尺寸）。专用 JSON 的坐标按它换算。
  final int width;

  /// 出图像素高。
  final int height;

  /// 整体场景描述（"雾夜废墟，一名提灯的旅人正走近教堂"）。
  final String brief;

  final List<LayoutRegion> regions;

  /// 机位（俯视 / 平视 / 仰视）。
  final LayoutCamera camera;

  /// "3:4 竖构图"这种叫法，散文第一句要用。
  String get ratioLabel {
    final g = _gcd(width, height);
    final key = '${width ~/ g}:${height ~/ g}';
    final name = _ratioNames[key];
    return name == null ? key : '$key $name';
  }

  static const Map<String, String> _ratioNames = {
    '1:1': '方形',
    '3:4': '竖构图',
    '2:3': '竖构图',
    '9:16': '竖屏',
    '4:3': '横构图',
    '3:2': '横构图',
    '16:9': '宽幅',
  };

  static int _gcd(int a, int b) {
    var x = a.abs();
    var y = b.abs();
    while (y != 0) {
      final t = y;
      y = x % y;
      x = t;
    }
    return x == 0 ? 1 : x;
  }

  /// 存盘用的形状（也是"发给 AI 的布局规格"的形状）。
  Map<String, Object?> toJson() => <String, Object?>{
        'v': 1,
        'width': width,
        'height': height,
        'brief': brief,
        'camera': camera.id,
        'regions': [for (final r in regions) r.toJson()],
      };

  static LayoutSpec fromJson(Map<String, Object?> json) {
    final regions = <LayoutRegion>[];
    final raw = json['regions'];
    if (raw is List) {
      for (final e in raw) {
        if (e is Map) {
          regions.add(LayoutRegion.fromJson(Map<String, Object?>.from(e)));
        }
      }
    }
    return LayoutSpec(
      width: _asInt(json['width'], 1024),
      height: _asInt(json['height'], 1024),
      brief: (json['brief'] as String?) ?? '',
      camera: LayoutCamera.values.firstWhere(
        (c) => c.id == json['camera'],
        orElse: () => LayoutCamera.auto,
      ),
      regions: regions,
    );
  }

  /// 翻译成两条出口。纯函数：同一份 spec 调两次，输出逐字节相同。
  LayoutTranslation translate() => LayoutTranslation(
        prose: _prose(),
        mossJson: _mossJson(),
        warnings: _warnings(),
      );

  // ── 散文投影 ────────────────────────────────────────────────────────────
  //
  // 散文是**有损投影**：它只有离散的方位词和几档大小，没有百分比。
  // 所以这里先把框量化成词（[ _positionPhrase ] / [ _scalePhrase ]），
  // 再拼句子 —— 而不是把坐标原样写进去。
  // 写 "主体位于 x=312..688" 是一句模型读不懂、人也不想看的话。

  String _prose() {
    final b = StringBuffer();
    // 比例名里有的已经带"构图"（"3:4 竖构图"），再拼一次就是"竖构图构图"。
    final ratio = ratioLabel;
    b.write(ratio.endsWith('构图') ? '$ratio。' : '$ratio构图。');
    final cam = camera.phrase;
    if (cam.isNotEmpty) {
      // 机位决定透视：俯视才看得见桌面，平视才看得出墙面竖向关系
      b.write('机位是 $cam。');
    }
    final intro = brief.trim();
    if (intro.isNotEmpty) {
      b.write(intro.endsWith('。') || intro.endsWith('.') ? intro : '$intro。');
    }
    if (regions.isEmpty) {
      b.write('（还没有框选任何区域。）');
      return b.toString();
    }

    // 面积大的先写：读者（和模型）都会把先出现的当成主体。
    final sorted = [...regions]..sort((a, c) {
        final byArea = c.rect.areaFraction.compareTo(a.rect.areaFraction);
        if (byArea != 0) return byArea;
        final byY = a.rect.y.compareTo(c.rect.y);
        if (byY != 0) return byY;
        return a.rect.x.compareTo(c.rect.x);
      });

    for (final r in sorted) {
      b.write(_sentence(r));
    }
    return b.toString();
  }

  String _sentence(LayoutRegion r) {
    final pos = _positionPhrase(r.rect);
    final edge = _edgeHint(r.rect);
    final size = _extentPhrase(r.rect);
    final head = <String>[
      pos,
      if (edge.isNotEmpty) edge,
      if (r.surface.phrase.isNotEmpty) r.surface.phrase,
    ].join('、');
    final rel = _relationClause(r);
    switch (r.kind) {
      case LayoutBoxKind.blank:
        return '$head（$size）是留白，不安排内容。';
      case LayoutBoxKind.text:
        final t = r.text.trim();
        if (t.isEmpty) return '$head（$size）是一块文字区域，内容待定。';
        return '$head（$size）里有一行文字「$t」$rel。';
      case LayoutBoxKind.object:
      case LayoutBoxKind.character:
        return '$head（$size）是${r.detail}$rel。';
    }
  }

  /// 关系短语（两条出口共用）。指不到的框直接忽略 —— 关系框被删掉时
  /// 宁可少一句，也不要生成"与「undefined」"这种句子。
  String _relationClause(LayoutRegion r) {
    if (r.relation == LayoutRelation.none) return '';
    final target = _byId(r.relationTo);
    if (target == null || identical(target, r)) return '';
    final name = target.displayName;
    return switch (r.relation) {
      LayoutRelation.none => '',
      LayoutRelation.mirror =>
        '，与「$name」是同一个人的镜像，姿势必须一致、仅左右翻转',
      LayoutRelation.same => '，与「$name」是同一个主体',
      LayoutRelation.inFront => '，挡在「$name」前面',
    };
  }

  LayoutRegion? _byId(String? id) {
    if (id == null) return null;
    for (final r in regions) {
      if (r.id == id) return r;
    }
    return null;
  }

  /// 把一个矩形压成 3×3 方位词。
  ///
  /// 用**中心点**判方位：一个横贯画面的盒子中心必然在中间，这时 [ _scalePhrase ]
  /// 会补一句"横贯画面"，两者合起来才是它的真实位置。
  String _positionPhrase(LayoutRect r) {
    final cx = r.centerX;
    final cy = r.centerY;
    final h = cx < 334
        ? '左'
        : cx < 667
            ? ''
            : '右';
    final v = cy < 334
        ? '上'
        : cy < 667
            ? ''
            : '下';
    if (h.isEmpty && v.isEmpty) return '画面中央';
    if (v.isEmpty) return '画面$h侧';
    if (h.isEmpty) return '画面$v方';
    return '画面$h$v';
  }

  /// 贴边提示。构图里"贴着右边缘的一条"和"右上角的一块"是两回事，
  /// 光靠方位词区分不出来，所以边缘单独说。
  String _edgeHint(LayoutRect r) {
    if (r.w >= 950 && r.h >= 950) return '';
    final hints = <String>[];
    if (r.x <= 15) hints.add('贴左缘');
    if (r.right >= 985) hints.add('贴右缘');
    if (r.y <= 15) hints.add('贴顶部');
    if (r.bottom >= 985) hints.add('贴底部');
    if (hints.length == 2 && r.w >= 950) return '横贯画面';
    if (hints.length == 2 && r.h >= 950) return '纵向贯穿';
    return hints.join('、');
  }

  /// 宽高各占几成。
  ///
  /// 这里**必须给数字**：只写"左上方"的话，同一格里两个框会得到一模一样的
  /// 句子（用户 2026-11-01 的反馈就是"桌子"和"镜子"都被写成"画面左上（小块）"）。
  /// 面积分档（小块 / 大面积）损失的信息比"宽约三成、高约一成"多得多 ——
  /// 两个面积相同但一个横条一个竖条的框，在前者眼里是一样的。
  String _extentPhrase(LayoutRect r) =>
      '宽约${_tenths(r.w / 1000)}、高约${_tenths(r.h / 1000)}';

  /// 0~1 → "三成 / 三成半 / 半成 / 整幅"（取最近的 5%）。
  static String _tenths(double v) {
    final n = (v * 20).round() / 2;
    if (n >= 10) return '整幅';
    if (n <= 0) return '极小';
    if (n < 1) return '半成';
    const digits = ['', '一', '两', '三', '四', '五', '六', '七', '八', '九'];
    final whole = n.floor();
    return (n - whole) >= 0.5 ? '${digits[whole]}成半' : '${digits[whole]}成';
  }

  // ── 专用投影（MOSS-Image 布局 JSON）─────────────────────────────────────
  //
  // 结构与 ComfyUI `nodes_moss_image.py` 的 `MOSSImageLayoutPrompt.execute`
  // **逐字节对齐**（紧凑 JSON、同样的键序、同样的整数取整），所以：
  //   · 我们生成的这段可以直接当提示词文本喂给 MOSS-Image；
  //   · 可以用它和节点输出做 golden 对比 —— 坐标错位一眼就能看出来。

  String _mossJson() {
    final layout = <String, Object>{
      'summary': <String, Object>{'caption': brief.trim()},
      'items': mossItems(),
      'bbox_coord_space': 'abs_centralized',
      'bbox_format': 'yxyx',
    };
    return jsonEncode(layout);
  }

  /// 专用输出里的 `items`（留白框不在其中）。
  List<Map<String, Object>> mossItems() {
    final items = <Map<String, Object>>[];
    for (final r in regions) {
      if (!r.kind.isSerializable) continue;
      final base =
          r.caption.trim().isNotEmpty ? r.caption.trim() : r.label.trim();
      // 关系也进 caption：MOSS 只按框读 caption，框外没有别的地方说"这是同一个人"。
      final caption = '$base${_relationClause(r)}';
      final item = <String, Object>{
        'label': r.kind.mossLabel,
        'caption': caption,
      };
      if (r.kind == LayoutBoxKind.text) item['text'] = r.text;
      item['bbox'] = mossBbox(r.rect);
      items.add(item);
    }
    return items;
  }

  /// 0~1000 的左上宽高 → **以画面中心为原点的像素** `[y0, x0, y1, x1]`。
  ///
  /// 起点向下取整、终点向上取整（框要"包住"内容），再减半幅 —— 与
  /// `nodes_moss_image.py` 的 `math.floor(...) - height // 2` 一致。
  /// 全程整数运算，避免 `1024.0` 被算成 `1023.9999` 之后 floor 成 1023。
  List<int> mossBbox(LayoutRect r) {
    final y0 = (r.y * height) ~/ 1000 - height ~/ 2;
    final x0 = (r.x * width) ~/ 1000 - width ~/ 2;
    final y1 = ((r.bottom * height) + 999) ~/ 1000 - height ~/ 2;
    final x1 = ((r.right * width) + 999) ~/ 1000 - width ~/ 2;
    return [y0, x0, y1, x1];
  }

  // ── 保真度提示 ──────────────────────────────────────────────────────────
  //
  // 一个翻译器值不值得信，取决于**它敢不敢报自己的损失**。
  // 这里每一条都对应散文真的会丢的东西，不是凑数的提醒。

  List<String> _warnings() {
    final out = <String>[];
    final active = regions.where((r) => r.kind.isSerializable).toList();

    if (regions.isEmpty) {
      out.add('还没有框选任何区域。');
      return out;
    }

    if (active.length > 5) {
      out.add(
        '当前 ${active.length} 个区域。一段散文大约只能稳定承载 5 个约束，'
        '超出的位置模型会随机忽略。建议改用下面的 MOSS 布局 JSON。',
      );
    }

    final unnamed = active
        .where((r) => r.label.trim().isEmpty && r.caption.trim().isEmpty)
        .length;
    if (unnamed > 0) {
      out.add('有 $unnamed 个框既没名称也没描述，散文里只能写成"一处未命名的物体"。');
    }

    final tiny = active.where((r) => r.rect.areaFraction < 0.01).length;
    if (tiny > 0) {
      out.add('有 $tiny 个框小于画面的 1%，这个尺度在散文里表达不出来。');
    }

    final emptyText =
        regions.where((r) => r.kind == LayoutBoxKind.text && r.text.trim().isEmpty).length;
    if (emptyText > 0) {
      out.add('有 $emptyText 个文字框还没填内容。');
    }

    final blanks = regions.length - active.length;
    if (blanks > 0) {
      out.add('有 $blanks 个留白框，它们进不了 MOSS 布局 JSON（那套格式只认有内容的框），'
          '这些信息只保留在通用提示词里。');
    }

    var overlaps = 0;
    for (var i = 0; i < active.length; i++) {
      for (var j = i + 1; j < active.length; j++) {
        if (active[i].rect.iou(active[j].rect) > 0.35) overlaps++;
      }
    }
    if (overlaps > 0) {
      out.add('有 $overlaps 组区域大面积重叠，散文里说不清它们的前后遮挡关系。');
    }

    return out;
  }
}
