/// 提示词 / 产物 / 标签 的数据模型
///
/// 与后端 Ktor 的 DTO 一一对应，字段名使用 camelCase。
library;

// ===========================================================================
//  后端环境（受限状态）
// ===========================================================================

/// `GET /api/system/confinement` —— **"这个后端能不能写工作区外"**。
///
/// 为什么界面上要有它（2026-09-29 用户 bug）：受限这件事原本只有 `doctor` 与工具文案会说，
/// 界面上看不到，于是模型如实转述成"input 目录权限不够"，用户去改 ACL —— 白忙一场。
/// 受限是**环境事实**，得让人一眼看见；而且必须连**判据**一起给
/// （[imageInsideProject]：后端映像落在项目内），否则用户没法自查。
class ConfinementInfo {
  final bool restricted;

  /// 后端 exe 现在在哪儿（判据的第一半）
  final String? serverImage;

  /// 后端映像是否落在**自己的运行根目录**里（判据的第二半）。
  ///
  /// ⚠ 别把这句说成"在工作区内"：源码树布局下两者相等，但发布包里根目录就是包目录、
  /// 并不是被沙箱框住的地方（实测那里 `restricted=false`）。所以文案只说"落在运行根目录里"，
  /// 让用户自己对照自己的布局。
  final bool imageInsideProjectRoot;

  /// 写不进的那些（人话形式：`产物目录 D:\…（permissiondenied（os error 5））`）
  final List<String> blockedPaths;

  /// 探过的那些
  final List<String> checkedPaths;

  /// 后端给的一行小结
  final String summary;

  /// 真受限时后端给的修法（**别在界面里另写一份**）
  final String? remedy;

  const ConfinementInfo({
    required this.restricted,
    this.serverImage,
    this.imageInsideProjectRoot = false,
    this.blockedPaths = const [],
    this.checkedPaths = const [],
    this.summary = '',
    this.remedy,
  });

  factory ConfinementInfo.fromJson(Map<String, dynamic> j) => ConfinementInfo(
    restricted: j['restricted'] == true,
    serverImage: j['serverImage'] as String?,
    imageInsideProjectRoot: j['imageInsideProjectRoot'] == true,
    blockedPaths: ((j['blockedPaths'] as List?) ?? const [])
        .map((e) => '$e')
        .toList(),
    checkedPaths: ((j['checkedPaths'] as List?) ?? const [])
        .map((e) => '$e')
        .toList(),
    summary: (j['summary'] ?? '') as String,
    remedy: j['remedy'] as String?,
  );

  /// 一句话说明"为什么" —— **判据在前、结论在后**，别反过来。
  ///
  /// 「后端被关在项目里」那种说法太含糊（听着像 ComfyUI 出问题了），
  /// 说清是**后端映像落在运行根目录里**用户才能自己去核对。
  String get headline => restricted
      ? (imageInsideProjectRoot ? '后端映像落在运行根目录里，写不进项目外的目录' : '后端写不进项目外的目录')
      : '后端可以正常读写项目外';

  /// 给用户看的细节（判据 + 写不进的目录）。
  String get detail {
    final lines = <String>[];
    if (serverImage != null && serverImage!.isNotEmpty) {
      lines.add(
        '后端映像是 $serverImage${imageInsideProjectRoot ? '（在运行根目录内）' : ''}',
      );
    }
    if (blockedPaths.isNotEmpty) {
      lines.add('写不进这些目录 ${blockedPaths.join('、')}');
    }
    return lines.join('\n');
  }
}

// ===========================================================================
//  出网代理（`GET /api/system/outbound`）
// ===========================================================================

/// **这次出网走不走代理**，连"这份设置是从哪来的"一起给。
///
/// 存在的理由（2026-11-09）：danbooru 挂维护、Cloudflare 回 503，而工具报错只写
/// "暂时不可用"，看着就像"本项目没吃代理"。真相是**路由是通的、503 是对方回的** ——
/// 这件事得在界面上看得见，用户才能自己核对，而不是靠猜。
class OutboundInfo {
  /// `proxy` = 公网走代理；`direct` = 公网直连
  final String route;

  /// 代理地址（`host:port`）；直连时为空
  final String? proxy;

  /// 这份设置来自哪里：`env:COMFYHUB_HTTPS_PROXY` / `env:HTTPS_PROXY` / `system` / `none`
  final String source;

  /// 系统代理开没开
  final bool systemEnabled;

  /// 系统代理填的地址（原样）
  final String? systemServer;

  /// `ProxyOverride` 拆开的例外表（这些地址直连）
  final List<String> overrides;

  /// 系统用的是 PAC 自动配置脚本（**本项目读不了它**，后端会写进 [warning]）
  final String? autoConfigUrl;

  /// 读得到但用不上的情况（目前只有 PAC）
  final String? warning;

  /// `?probe=1` 时后端真的发过请求的结果
  final List<OutboundProbe> probes;

  const OutboundInfo({
    required this.route,
    this.proxy,
    this.source = 'none',
    this.systemEnabled = false,
    this.systemServer,
    this.overrides = const [],
    this.autoConfigUrl,
    this.warning,
    this.probes = const [],
  });

  factory OutboundInfo.fromJson(Map<String, dynamic> j) {
    final system = (j['system'] is Map)
        ? Map<String, dynamic>.from(j['system'] as Map)
        : const <String, dynamic>{};
    return OutboundInfo(
      route: (j['route'] ?? 'direct') as String,
      proxy: j['proxy'] as String?,
      source: (j['source'] ?? 'none') as String,
      systemEnabled: system['enabled'] == true,
      systemServer: system['server'] as String?,
      overrides: ((system['overrides'] as List?) ?? const [])
          .map((e) => '$e')
          .toList(),
      autoConfigUrl: system['autoConfigUrl'] as String?,
      warning: j['warning'] as String?,
      probes: ((j['probes'] as List?) ?? const [])
          .whereType<Map>()
          .map((e) => OutboundProbe.fromJson(Map<String, dynamic>.from(e)))
          .toList(),
    );
  }

  bool get viaProxy => (proxy ?? '').isNotEmpty;

  /// 这份设置从哪来 —— 一句人话（别把 `env:HTTPS_PROXY` 这种内部代号甩到界面上）。
  String get sourceLabel {
    if (source.startsWith('env:')) {
      return '环境变量 ${source.substring(4)}';
    }
    return switch (source) {
      'system' => '系统的代理设置',
      'none' => '没有找到代理设置',
      _ => source,
    };
  }

  /// 一句话结论：公网到底走不走代理。
  String get headline =>
      viaProxy ? '公网请求走 HTTP 代理 $proxy' : '公网请求直连，没有走代理';
}

/// 一次探针的结果（后端写死了目标，前端只显示）。
class OutboundProbe {
  final String host;

  /// `proxy` / `direct`（这一次实际走的路）
  final String via;

  /// 人话版路径（`HTTP 代理 127.0.0.1:7890`）
  final String route;

  /// 对方回的 HTTP 状态；连不上 / 超时是 null
  final int? status;

  /// 连不上时的原因
  final String? error;

  final int elapsedMs;

  const OutboundProbe({
    required this.host,
    required this.via,
    required this.route,
    this.status,
    this.error,
    this.elapsedMs = 0,
  });

  factory OutboundProbe.fromJson(Map<String, dynamic> j) => OutboundProbe(
    host: (j['host'] ?? '') as String,
    via: (j['via'] ?? 'direct') as String,
    route: (j['route'] ?? '') as String,
    status: (j['status'] as num?)?.toInt(),
    error: j['error'] as String?,
    elapsedMs: (j['elapsedMs'] as num?)?.toInt() ?? 0,
  );

  bool get reached => status != null;

  /// 一句人话：**"对方回了"和"根本没连上"是两件事**，别混成一句。
  String get sentence {
    final seconds = (elapsedMs / 1000).toStringAsFixed(1);
    if (status != null) {
      return '$host 返回了 HTTP $status，用了 $seconds 秒（$route）';
    }
    return '$host 没连上，用了 $seconds 秒（$route）。原因是 $error';
  }
}

// ===========================================================================
//  标签
// ===========================================================================

class Tag {
  final int id;
  final String name;
  final String? category;
  final String? color;
  final String? description;
  final int useCount;

  const Tag({
    required this.id,
    required this.name,
    this.category,
    this.color,
    this.description,
    this.useCount = 0,
  });

  factory Tag.fromJson(Map<String, dynamic> j) => Tag(
    id: (j['id'] as num).toInt(),
    name: (j['name'] ?? '') as String,
    category: j['category'] as String?,
    color: j['color'] as String?,
    description: j['description'] as String?,
    useCount: (j['useCount'] as num?)?.toInt() ?? 0,
  );

  Map<String, dynamic> toJson() => {
    'name': name,
    if (category != null) 'category': category,
    if (color != null) 'color': color,
    if (description != null) 'description': description,
  };
}

// ===========================================================================
//  提示词
// ===========================================================================

enum PromptKind { image, video, audio, mixed }

PromptKind promptKindFrom(String? raw) {
  switch ((raw ?? '').toUpperCase()) {
    case 'VIDEO':
      return PromptKind.video;
    case 'AUDIO':
      return PromptKind.audio;
    case 'MIXED':
      return PromptKind.mixed;
    default:
      return PromptKind.image;
  }
}

extension PromptKindX on PromptKind {
  String get wire => name.toUpperCase();
  String get label => switch (this) {
    PromptKind.image => '生图',
    PromptKind.video => '生视频',
    PromptKind.audio => '生音频',
    PromptKind.mixed => '混合',
  };
}

class LoraRef {
  final String name;
  final double weight;

  const LoraRef({required this.name, this.weight = 1.0});

  factory LoraRef.fromJson(Map<String, dynamic> j) => LoraRef(
    name: (j['name'] ?? '') as String,
    weight: (j['weight'] as num?)?.toDouble() ?? 1.0,
  );

  Map<String, dynamic> toJson() => {'name': name, 'weight': weight};
}

class Prompt {
  final int id;
  final String title;
  final PromptKind kind;
  final String positivePrompt;
  final String? negativePrompt;
  final String? checkpoint;
  final List<LoraRef> loras;
  final String? sampler;
  final String? scheduler;
  final int? steps;
  final double? cfgScale;
  final int? seed;
  final int? width;
  final int? height;
  final int? batchSize;
  final Map<String, String> extraParams;
  final String? notes;

  /// 发送模式：`keep`（默认，照原样带上负面提示词）/ `positive_only`。
  ///
  /// **有些 turbo 模型不认负面提示词**（用户 2026-10-11），所以预设要能表达
  /// "我只发正面"。这是**预设的属性**（写提示词时的意图）——
  /// 与"目标工作流认不认负面"是两件事，后者在提交那条路上兜底检查。
  final String negativeMode;

  final bool favorite;

  /// 来源: Manual（手动新建）/ ComfyUI（自动捕获）/ ComfyUI-Import（历史导入）
  final String? source;

  /// 来源侧唯一标识（如 ComfyUI 的 prompt_id）
  final String? sourceRef;

  /// 后端是否存了完整工作流（决定要不要显示「查看工作流」）
  final bool hasWorkflow;
  final DateTime? createdAt;
  final DateTime? updatedAt;
  final List<Tag> tags;
  final int mediaCount;

  const Prompt({
    required this.id,
    required this.title,
    required this.kind,
    required this.positivePrompt,
    this.negativePrompt,
    this.checkpoint,
    this.loras = const [],
    this.sampler,
    this.scheduler,
    this.steps,
    this.cfgScale,
    this.seed,
    this.width,
    this.height,
    this.batchSize,
    this.extraParams = const {},
    this.notes,
    this.negativeMode = 'keep',
    this.favorite = false,
    this.source,
    this.sourceRef,
    this.hasWorkflow = false,
    this.createdAt,
    this.updatedAt,
    this.tags = const [],
    this.mediaCount = 0,
  });

  factory Prompt.fromJson(Map<String, dynamic> j) => Prompt(
    id: (j['id'] as num).toInt(),
    title: (j['title'] ?? '') as String,
    kind: promptKindFrom(j['kind'] as String?),
    positivePrompt: (j['positivePrompt'] ?? '') as String,
    negativePrompt: j['negativePrompt'] as String?,
    checkpoint: j['checkpoint'] as String?,
    loras: ((j['loras'] as List?) ?? const [])
        .map((e) => LoraRef.fromJson(e as Map<String, dynamic>))
        .toList(),
    sampler: j['sampler'] as String?,
    scheduler: j['scheduler'] as String?,
    steps: (j['steps'] as num?)?.toInt(),
    cfgScale: (j['cfgScale'] as num?)?.toDouble(),
    seed: (j['seed'] as num?)?.toInt(),
    width: (j['width'] as num?)?.toInt(),
    height: (j['height'] as num?)?.toInt(),
    batchSize: (j['batchSize'] as num?)?.toInt(),
    extraParams: ((j['extraParams'] as Map?) ?? const {}).map(
      (k, v) => MapEntry(k.toString(), v.toString()),
    ),
    notes: j['notes'] as String?,
    // 老后端没有这个键时退回 keep —— **绝不能默认成只发正面**
    // （那会悄悄丢掉用户的负面提示词，而 turbo 之外的模型很吃负面）
    negativeMode: (j['negativeMode'] as String?) ?? 'keep',
    favorite: (j['favorite'] as bool?) ?? false,
    source: j['source'] as String?,
    sourceRef: j['sourceRef'] as String?,
    hasWorkflow: (j['hasWorkflow'] as bool?) ?? false,
    createdAt: _dt(j['createdAt']),
    updatedAt: _dt(j['updatedAt']),
    tags: ((j['tags'] as List?) ?? const [])
        .map((e) => Tag.fromJson(e as Map<String, dynamic>))
        .toList(),
    mediaCount: (j['mediaCount'] as num?)?.toInt() ?? 0,
  );

  /// 用于 POST/PUT 的请求体
  Map<String, dynamic> toInputJson() => {
    'title': title,
    'kind': kind.wire,
    'positivePrompt': positivePrompt,
    'negativePrompt': negativePrompt,
    'checkpoint': checkpoint,
    'loras': loras.map((e) => e.toJson()).toList(),
    'sampler': sampler,
    'scheduler': scheduler,
    'steps': steps,
    'cfgScale': cfgScale,
    'seed': seed,
    'width': width,
    'height': height,
    'batchSize': batchSize,
    'extraParams': extraParams,
    'notes': notes,
    'negativeMode': negativeMode,
    'favorite': favorite,
    'tags': tags.map((e) => e.name).toList(),
  };

  Prompt copyWith({
    int? id,
    String? title,
    PromptKind? kind,
    String? positivePrompt,
    String? negativePrompt,
    String? checkpoint,
    List<LoraRef>? loras,
    String? sampler,
    String? scheduler,
    int? steps,
    double? cfgScale,
    int? seed,
    int? width,
    int? height,
    int? batchSize,
    Map<String, String>? extraParams,
    String? notes,
    String? negativeMode,
    bool? favorite,
    bool? hasWorkflow,
    List<Tag>? tags,
    int? mediaCount,
  }) => Prompt(
    id: id ?? this.id,
    title: title ?? this.title,
    kind: kind ?? this.kind,
    positivePrompt: positivePrompt ?? this.positivePrompt,
    negativePrompt: negativePrompt ?? this.negativePrompt,
    checkpoint: checkpoint ?? this.checkpoint,
    loras: loras ?? this.loras,
    sampler: sampler ?? this.sampler,
    scheduler: scheduler ?? this.scheduler,
    steps: steps ?? this.steps,
    cfgScale: cfgScale ?? this.cfgScale,
    seed: seed ?? this.seed,
    width: width ?? this.width,
    height: height ?? this.height,
    batchSize: batchSize ?? this.batchSize,
    extraParams: extraParams ?? this.extraParams,
    notes: notes ?? this.notes,
    negativeMode: negativeMode ?? this.negativeMode,
    favorite: favorite ?? this.favorite,
    source: source,
    sourceRef: sourceRef,
    hasWorkflow: hasWorkflow ?? this.hasWorkflow,
    createdAt: createdAt,
    updatedAt: updatedAt,
    tags: tags ?? this.tags,
    mediaCount: mediaCount ?? this.mediaCount,
  );

  /// 空白草稿
  static Prompt draft() => const Prompt(
    id: 0,
    title: '',
    kind: PromptKind.image,
    positivePrompt: '',
  );
}

// ===========================================================================
//  生成产物
// ===========================================================================

enum MediaKind { image, video, audio }

MediaKind mediaKindFrom(String? raw) {
  switch ((raw ?? '').toUpperCase()) {
    case 'VIDEO':
      return MediaKind.video;
    case 'AUDIO':
      return MediaKind.audio;
    default:
      return MediaKind.image;
  }
}

extension MediaKindX on MediaKind {
  String get wire => name.toUpperCase();
  String get label => switch (this) {
    MediaKind.image => '图片',
    MediaKind.video => '视频',
    MediaKind.audio => '音频',
  };
}

class MediaAsset {
  final int id;
  final int? promptId;
  final MediaKind kind;
  final String title;
  final String originalName;
  final String storedName;
  final String? mimeType;
  final int sizeBytes;
  final int? width;
  final int? height;
  final int? durationMs;
  final String? sha256;
  final String? source;
  final String? sourceRef;
  final bool hasWorkflow;
  final bool favorite;

  /// 是否成人内容（用户 bug 2026-09-20）。
  ///
  /// **只决定预览图打不打码**，不影响能不能看：AI 用 `comfy_submit` 的 `nsfw`
  /// 声明过、或用户手动标记过才为 true。默认 false —— 以前没有这一项，
  /// 「NSFW 预览图模糊」开关一开会把风景图、甚至音频封面全都糊住。
  final bool nsfw;
  final String? notes;
  final DateTime? createdAt;
  final DateTime? updatedAt;

  /// 所属**集合**（同源微调的产物归到一组）。
  ///
  /// `null` = 没进组（没关联提示词的孤儿产物，或规则判定它自成一类）。
  /// 后端判定真源是 `server-rs/src/media/collection.rs`，**前端不做任何判定**。
  final int? collectionId;

  /// 这个集合有几个成员。**后端保证 ≥ 1**（没进组的产物是 1）。
  /// 画廊靠它决定要不要叠放封面 / 显示角标；`<= 1` 与 `collectionId == null` 一样按普通格子显示。
  final int collectionSize;

  /// 集合**头三张**成员的 id（叠放封面用）。空 = 没进组。
  ///
  /// 后端在列表 SQL 里一次算好（`collection_preview`），所以画廊**不必**为每个集合格子
  /// 再发一次"列成员"的请求 —— 一页 24 格就是 24 次。
  final List<int> collectionPreview;

  // 关联提示词摘要
  final String? promptTitle;
  final String? promptPositive;
  final String? promptNegative;
  final String? checkpoint;
  final int? seed;
  final List<String> promptTags;

  // 相对路径
  final String fileUrl;
  final String? thumbUrl;

  const MediaAsset({
    required this.id,
    this.promptId,
    required this.kind,
    required this.title,
    required this.originalName,
    required this.storedName,
    this.mimeType,
    this.sizeBytes = 0,
    this.width,
    this.height,
    this.durationMs,
    this.sha256,
    this.source,
    this.sourceRef,
    this.hasWorkflow = false,
    this.favorite = false,
    this.nsfw = false,
    this.notes,
    this.createdAt,
    this.updatedAt,
    this.collectionId,
    this.collectionSize = 1,
    this.collectionPreview = const [],
    this.promptTitle,
    this.promptPositive,
    this.promptNegative,
    this.checkpoint,
    this.seed,
    this.promptTags = const [],
    required this.fileUrl,
    this.thumbUrl,
  });

  factory MediaAsset.fromJson(Map<String, dynamic> j) => MediaAsset(
    id: (j['id'] as num).toInt(),
    promptId: (j['promptId'] as num?)?.toInt(),
    kind: mediaKindFrom(j['kind'] as String?),
    title: (j['title'] ?? '') as String,
    originalName: (j['originalName'] ?? '') as String,
    storedName: (j['storedName'] ?? '') as String,
    mimeType: j['mimeType'] as String?,
    sizeBytes: (j['sizeBytes'] as num?)?.toInt() ?? 0,
    width: (j['width'] as num?)?.toInt(),
    height: (j['height'] as num?)?.toInt(),
    durationMs: (j['durationMs'] as num?)?.toInt(),
    sha256: j['sha256'] as String?,
    source: j['source'] as String?,
    sourceRef: j['sourceRef'] as String?,
    hasWorkflow: (j['hasWorkflow'] as bool?) ?? false,
    favorite: (j['favorite'] as bool?) ?? false,
    nsfw: (j['nsfw'] as bool?) ?? false,
    notes: j['notes'] as String?,
    createdAt: _dt(j['createdAt']),
    updatedAt: _dt(j['updatedAt']),
    promptTitle: j['promptTitle'] as String?,
    promptPositive: j['promptPositive'] as String?,
    promptNegative: j['promptNegative'] as String?,
    checkpoint: j['checkpoint'] as String?,
    seed: (j['seed'] as num?)?.toInt(),
    promptTags: ((j['promptTags'] as List?) ?? const [])
        .map((e) => e.toString())
        .toList(),
    collectionId: (j['collectionId'] as num?)?.toInt(),
    // 后端保证 ≥ 1；老后端没有这个键时退回 1（按普通格子显示，不会误叠放）
    collectionSize: (j['collectionSize'] as num?)?.toInt() ?? 1,
    collectionPreview: ((j['collectionPreview'] as List?) ?? const [])
        .map((e) => (e as num).toInt())
        .toList(),
    fileUrl: (j['fileUrl'] ?? '') as String,
    thumbUrl: j['thumbUrl'] as String?,
  );

  bool get hasPrompt => promptId != null;

  /// 这一条在画廊里该不该显示成"一组"（叠放封面 + 角标）。
  ///
  /// 判据与后端一致：**有集合且成员多于一个**。没进组的（孤儿产物）是 `null` + `1`，
  /// 按普通格子显示。
  bool get isCollection => collectionId != null && collectionSize > 1;
}

/// 一个**集合**的摘要（后端 `GET /api/media/collections`，2026-10-18 手动集合）。
///
/// 两种集合本来就是两种东西，界面上必须分得清：
/// * `auto` = 算法按提示词分出来的（**没有名字**，可以重新算）；
/// * `manual` = 用户建的、或者被用户改过的（有名字，算法不再动它）。
class MediaCollection {
  final int id;
  final String kind;

  /// `auto` / `manual`
  final String origin;

  /// 用户起的名字。算法分出来的为 null。
  final String? title;
  final int size;

  /// 选择器里给这一组显示一张封面（第一张成员的 media id）。
  ///
  /// 没有它时选择器只能显示"未命名集合（3 张）"——几个未命名集合长得一模一样，
  /// 用户根本分不出要把图移进哪一个（2026-11-07 用户："至少给我一个预览吧？"）。
  final int? coverMediaId;

  /// 封面是不是 NSFW（选择器里照样打码，别为了"看一眼封面"把规则破了）。
  final bool coverNsfw;

  const MediaCollection({
    required this.id,
    required this.kind,
    required this.origin,
    this.title,
    this.size = 0,
    this.coverMediaId,
    this.coverNsfw = false,
  });

  bool get isManual => origin == 'manual';

  /// 界面上显示的名字：手动集合用人起的名字；没有名字时——
  /// * `manual`（老版本建的 / 后端还没补默认名）：如实说「未命名集合」；
  /// * `auto`（算法分组）：说「算法分组」，别让人以为是用户起的。
  String get label {
    final named = title?.trim() ?? '';
    if (named.isNotEmpty) return named;
    return isManual ? '未命名集合（$size 张）' : '算法分组（$size 张）';
  }

  /// 装不装得下 [mediaKind]（一个集合只装一种，用户 2026-10-18 拍板）。
  bool accepts(String mediaKind) =>
      kind.toUpperCase() == mediaKind.toUpperCase();

  factory MediaCollection.fromJson(Map<String, dynamic> json) =>
      MediaCollection(
        id: (json['id'] as num).toInt(),
        kind: (json['kind'] ?? 'IMAGE').toString(),
        origin: (json['origin'] ?? 'auto').toString(),
        title: json['title']?.toString(),
        size: (json['size'] as num?)?.toInt() ?? 0,
        coverMediaId: (json['coverMediaId'] as num?)?.toInt(),
        coverNsfw: json['coverNsfw'] == true,
      );
}

/// `POST /api/media/export` 的结果（2026-10-27）。
///
/// 后端把原件 + 工作流 / 提示词信息按「类型目录 / 集合文件夹」写进用户选的目录，
/// 这里只带**回执**：多少条成功、多少条失败、清单落在哪。界面据此说人话，
/// **不自己猜成功失败**。
class MediaExportReport {
  final String dir;
  final String manifest;
  final int exported;
  final int collections;
  final int workflows;
  final int totalBytes;
  final Map<String, int> byKind;
  final List<MediaExportFailure> failed;
  final List<String> warnings;

  const MediaExportReport({
    required this.dir,
    required this.manifest,
    required this.exported,
    required this.collections,
    required this.workflows,
    required this.totalBytes,
    required this.byKind,
    required this.failed,
    required this.warnings,
  });

  bool get hasFailures => failed.isNotEmpty;

  factory MediaExportReport.fromJson(Map<String, dynamic> json) =>
      MediaExportReport(
        dir: (json['dir'] ?? '').toString(),
        manifest: (json['manifest'] ?? '').toString(),
        exported: (json['exported'] as num?)?.toInt() ?? 0,
        collections: (json['collections'] as num?)?.toInt() ?? 0,
        workflows: (json['workflows'] as num?)?.toInt() ?? 0,
        totalBytes: (json['totalBytes'] as num?)?.toInt() ?? 0,
        byKind: ((json['byKind'] as Map?) ?? const {}).map(
          (key, value) =>
              MapEntry(key.toString(), (value as num?)?.toInt() ?? 0),
        ),
        failed: ((json['failed'] as List?) ?? const [])
            .map(
              (e) => MediaExportFailure.fromJson(
                Map<String, dynamic>.from(e as Map),
              ),
            )
            .toList(growable: false),
        warnings: ((json['warnings'] as List?) ?? const [])
            .map((e) => e.toString())
            .toList(growable: false),
      );
}

/// 一条没导出的产物：`mediaId` / 原始文件名可能为空（例如整个清单写入失败）。
class MediaExportFailure {
  final int? mediaId;
  final String? name;
  final String reason;

  const MediaExportFailure({this.mediaId, this.name, required this.reason});

  factory MediaExportFailure.fromJson(Map<String, dynamic> json) =>
      MediaExportFailure(
        mediaId: (json['mediaId'] as num?)?.toInt(),
        name: json['name']?.toString(),
        reason: (json['reason'] ?? '').toString(),
      );
}

// ===========================================================================
//  分页
// ===========================================================================

class Paged<T> {
  final List<T> items;
  final int total;
  final int page;
  final int size;
  final int pages;

  const Paged({
    required this.items,
    required this.total,
    required this.page,
    required this.size,
    required this.pages,
  });

  factory Paged.fromJson(
    Map<String, dynamic> j,
    T Function(Map<String, dynamic>) item,
  ) => Paged(
    items: ((j['items'] as List?) ?? const [])
        .map((e) => item(e as Map<String, dynamic>))
        .toList(),
    total: (j['total'] as num?)?.toInt() ?? 0,
    page: (j['page'] as num?)?.toInt() ?? 1,
    size: (j['size'] as num?)?.toInt() ?? 20,
    pages: (j['pages'] as num?)?.toInt() ?? 0,
  );

  static Paged<T> empty<T>() =>
      Paged<T>(items: const [], total: 0, page: 1, size: 20, pages: 0);
}

class UploadResult {
  final List<MediaAsset> items;
  final List<Map<String, dynamic>> duplicates;
  final List<Map<String, dynamic>> failed;
  final int? promptId;

  const UploadResult({
    this.items = const [],
    this.duplicates = const [],
    this.failed = const [],
    this.promptId,
  });

  factory UploadResult.fromJson(Map<String, dynamic> j) => UploadResult(
    items: ((j['items'] as List?) ?? const [])
        .map((e) => MediaAsset.fromJson(e as Map<String, dynamic>))
        .toList(),
    duplicates: ((j['duplicates'] as List?) ?? const [])
        .map((e) => (e as Map).cast<String, dynamic>())
        .toList(),
    failed: ((j['failed'] as List?) ?? const [])
        .map((e) => (e as Map).cast<String, dynamic>())
        .toList(),
    promptId: (j['promptId'] as num?)?.toInt(),
  );
}

class LibraryStats {
  final int prompts;
  final int media;
  final int tags;
  final int favoritePrompts;
  final int favoriteMedia;
  final Map<String, int> byKind;
  final Map<String, int> byMediaKind;

  const LibraryStats({
    this.prompts = 0,
    this.media = 0,
    this.tags = 0,
    this.favoritePrompts = 0,
    this.favoriteMedia = 0,
    this.byKind = const {},
    this.byMediaKind = const {},
  });

  factory LibraryStats.fromJson(Map<String, dynamic> j) => LibraryStats(
    prompts: (j['prompts'] as num?)?.toInt() ?? 0,
    media: (j['media'] as num?)?.toInt() ?? 0,
    tags: (j['tags'] as num?)?.toInt() ?? 0,
    favoritePrompts: (j['favoritePrompts'] as num?)?.toInt() ?? 0,
    favoriteMedia: (j['favoriteMedia'] as num?)?.toInt() ?? 0,
    byKind: ((j['byKind'] as Map?) ?? const {}).map(
      (k, v) => MapEntry(k.toString(), (v as num).toInt()),
    ),
    byMediaKind: ((j['byMediaKind'] as Map?) ?? const {}).map(
      (k, v) => MapEntry(k.toString(), (v as num).toInt()),
    ),
  );
}

// ===========================================================================
//  ComfyUI 自动捕获
// ===========================================================================

/// 自动捕获配置（存在后端数据库里，App 与后端共用一份）
class CaptureConfig {
  final bool enabled;
  final String comfyUrl;

  /// ComfyUI 输出目录；填了就能直接读本地文件，省掉一次 HTTP 下载
  final String? outputDir;
  final int pollSeconds;

  /// 捕获到的提示词自动打上的标签
  final String autoTag;

  /// 一轮轮询最多处理几条未捕获的运行
  final int maxPerPoll;

  /// 本地文件找不到时是否回退到 HTTP 下载
  final bool downloadFallback;

  const CaptureConfig({
    this.enabled = true,
    this.comfyUrl = 'http://127.0.0.1:8188',
    this.outputDir,
    this.pollSeconds = 4,
    this.autoTag = 'ComfyUI',
    this.maxPerPoll = 20,
    this.downloadFallback = true,
  });

  factory CaptureConfig.fromJson(Map<String, dynamic> j) => CaptureConfig(
    enabled: (j['enabled'] as bool?) ?? true,
    comfyUrl: (j['comfyUrl'] ?? 'http://127.0.0.1:8188') as String,
    outputDir: (j['outputDir'] as String?)?.trim().isEmpty ?? true
        ? null
        : (j['outputDir'] as String).trim(),
    pollSeconds: (j['pollSeconds'] as num?)?.toInt() ?? 4,
    autoTag: (j['autoTag'] ?? 'ComfyUI') as String,
    maxPerPoll: (j['maxPerPoll'] as num?)?.toInt() ?? 20,
    downloadFallback: (j['downloadFallback'] as bool?) ?? true,
  );

  Map<String, dynamic> toJson() => {
    'enabled': enabled,
    'comfyUrl': comfyUrl,
    'outputDir': outputDir,
    'pollSeconds': pollSeconds,
    'autoTag': autoTag,
    'maxPerPoll': maxPerPoll,
    'downloadFallback': downloadFallback,
  };

  CaptureConfig copyWith({
    bool? enabled,
    String? comfyUrl,
    String? outputDir,
    bool clearOutputDir = false,
    int? pollSeconds,
    String? autoTag,
    int? maxPerPoll,
    bool? downloadFallback,
  }) => CaptureConfig(
    enabled: enabled ?? this.enabled,
    comfyUrl: comfyUrl ?? this.comfyUrl,
    outputDir: clearOutputDir ? null : (outputDir ?? this.outputDir),
    pollSeconds: pollSeconds ?? this.pollSeconds,
    autoTag: autoTag ?? this.autoTag,
    maxPerPoll: maxPerPoll ?? this.maxPerPoll,
    downloadFallback: downloadFallback ?? this.downloadFallback,
  );
}

class CaptureRunInfo {
  final String runKey;
  final int? promptId;
  final String status;
  final int mediaCount;
  final String? title;
  final String? error;
  final DateTime? capturedAt;

  const CaptureRunInfo({
    required this.runKey,
    this.promptId,
    this.status = 'success',
    this.mediaCount = 0,
    this.title,
    this.error,
    this.capturedAt,
  });

  factory CaptureRunInfo.fromJson(Map<String, dynamic> j) => CaptureRunInfo(
    runKey: (j['runKey'] ?? '') as String,
    promptId: (j['promptId'] as num?)?.toInt(),
    status: (j['status'] ?? 'success') as String,
    mediaCount: (j['mediaCount'] as num?)?.toInt() ?? 0,
    title: j['title'] as String?,
    error: j['error'] as String?,
    capturedAt: _dt(j['capturedAt']),
  );
}

class CaptureStatus {
  final bool enabled;
  final String comfyUrl;
  final String? outputDir;
  final bool comfyReachable;
  final int queueRunning;
  final int queuePending;
  final DateTime? lastPollAt;
  final String? lastError;
  final int capturedRuns;
  final int capturedMedia;
  final List<CaptureRunInfo> recent;

  const CaptureStatus({
    this.enabled = false,
    this.comfyUrl = '',
    this.outputDir,
    this.comfyReachable = false,
    this.queueRunning = 0,
    this.queuePending = 0,
    this.lastPollAt,
    this.lastError,
    this.capturedRuns = 0,
    this.capturedMedia = 0,
    this.recent = const [],
  });

  factory CaptureStatus.fromJson(Map<String, dynamic> j) => CaptureStatus(
    enabled: (j['enabled'] as bool?) ?? false,
    comfyUrl: (j['comfyUrl'] ?? '') as String,
    outputDir: j['outputDir'] as String?,
    comfyReachable: (j['comfyReachable'] as bool?) ?? false,
    queueRunning: (j['queueRunning'] as num?)?.toInt() ?? 0,
    queuePending: (j['queuePending'] as num?)?.toInt() ?? 0,
    lastPollAt: _dt(j['lastPollAt']),
    lastError: j['lastError'] as String?,
    capturedRuns: (j['capturedRuns'] as num?)?.toInt() ?? 0,
    capturedMedia: (j['capturedMedia'] as num?)?.toInt() ?? 0,
    recent: ((j['recent'] as List?) ?? const [])
        .map((e) => CaptureRunInfo.fromJson((e as Map).cast<String, dynamic>()))
        .toList(),
  );
}

/// ComfyUI 安装位置探测结果（用户"其他建议"第 3 条）。
///
/// 发布包是便携式的：用户把包解压到哪、ComfyUI 装在哪，两边都不知道。
/// 后端按特征文件（`main.py` + `comfy/` 或 `output/` + `models/`）探测，
/// 探测结果**不会自动生效** —— 用户点「使用这个目录」才会写进配置。
class ComfyLocation {
  /// 探测到的 ComfyUI 根目录（找不到为 null）。
  final String? home;

  /// 大概率可直接读产物的输出目录。
  final String? outputDir;

  /// 从哪里找到的（给用户看的一句话）。
  final String? source;

  /// 当前设置里已经生效的输出目录。
  final String? configuredOutputDir;

  /// 没找到时给用户的建议。
  final String? note;

  const ComfyLocation({
    this.home,
    this.outputDir,
    this.source,
    this.configuredOutputDir,
    this.note,
  });

  factory ComfyLocation.fromJson(Map<String, dynamic> j) => ComfyLocation(
    home: j['home'] as String?,
    outputDir: j['outputDir'] as String?,
    source: j['source'] as String?,
    configuredOutputDir: j['configuredOutputDir'] as String?,
    note: j['note'] as String?,
  );

  /// 探测到了可以直接用的输出目录，且和当前配置不一样。
  bool get canApply =>
      outputDir != null &&
      outputDir!.isNotEmpty &&
      outputDir != configuredOutputDir;
}

class CapturePollResult {
  final bool ok;
  final int checked;
  final int newRuns;
  final int newMedia;
  final String? message;

  const CapturePollResult({
    this.ok = false,
    this.checked = 0,
    this.newRuns = 0,
    this.newMedia = 0,
    this.message,
  });

  factory CapturePollResult.fromJson(Map<String, dynamic> j) =>
      CapturePollResult(
        ok: (j['ok'] as bool?) ?? false,
        checked: (j['checked'] as num?)?.toInt() ?? 0,
        newRuns: (j['newRuns'] as num?)?.toInt() ?? 0,
        newMedia: (j['newMedia'] as num?)?.toInt() ?? 0,
        message: j['message'] as String?,
      );

  String get summary => ok
      ? '检查 $checked 次运行，新捕获 $newRuns 次（$newMedia 个产物）'
      : '同步失败，${message ?? "未知原因"}';
}

class ImportFolderResult {
  final String dir;
  final int scanned;
  final int imported;
  final int duplicates;
  final int failed;
  final int promptsCreated;
  final List<String> errors;

  const ImportFolderResult({
    this.dir = '',
    this.scanned = 0,
    this.imported = 0,
    this.duplicates = 0,
    this.failed = 0,
    this.promptsCreated = 0,
    this.errors = const [],
  });

  factory ImportFolderResult.fromJson(Map<String, dynamic> j) =>
      ImportFolderResult(
        dir: (j['dir'] ?? '') as String,
        scanned: (j['scanned'] as num?)?.toInt() ?? 0,
        imported: (j['imported'] as num?)?.toInt() ?? 0,
        duplicates: (j['duplicates'] as num?)?.toInt() ?? 0,
        failed: (j['failed'] as num?)?.toInt() ?? 0,
        promptsCreated: (j['promptsCreated'] as num?)?.toInt() ?? 0,
        errors: ((j['errors'] as List?) ?? const [])
            .map((e) => e.toString())
            .toList(),
      );

  String get summary =>
      '扫描 $scanned 个文件，新导入 $imported，跳过重复 $duplicates，失败 $failed。'
      '其中 $promptsCreated 个带内嵌工作流，已自动建提示词';
}

/// 「导入本机工作流文件」的结果（用户 bug ⑤）。
///
/// 现场：`prompts` 库过去只有两个入口 —— 自动捕获（轮询 ComfyUI `/history`）与 AI 手动按路径读，
/// 所以**首次使用时库里是空的**，而用户机器上早就存着一堆工作流（`user\<用户>\workflows`）。
/// 设置页这颗按钮就是"一次把它们读进库"。
class ImportWorkflowsResult {
  final List<String> dirs;
  final int scanned;
  final int imported;
  final int duplicates;
  final int failed;
  final List<int> promptIds;

  /// 后端给的一句话（"没找到 workflows 目录"这类情况也走它，所以界面直接显示）
  final String? message;

  const ImportWorkflowsResult({
    this.dirs = const [],
    this.scanned = 0,
    this.imported = 0,
    this.duplicates = 0,
    this.failed = 0,
    this.promptIds = const [],
    this.message,
  });

  factory ImportWorkflowsResult.fromJson(Map<String, dynamic> j) =>
      ImportWorkflowsResult(
        dirs: ((j['dirs'] as List?) ?? const [])
            .map((e) => e.toString())
            .toList(),
        scanned: (j['scanned'] as num?)?.toInt() ?? 0,
        imported: (j['imported'] as num?)?.toInt() ?? 0,
        duplicates: (j['duplicates'] as num?)?.toInt() ?? 0,
        failed: (j['failed'] as num?)?.toInt() ?? 0,
        promptIds: ((j['promptIds'] as List?) ?? const [])
            .map((e) => (e as num).toInt())
            .toList(),
        message: j['message'] as String?,
      );

  String get summary {
    final base =
        message ??
        '扫描 $scanned 份工作流文件，新导入 $imported，已在库里 $duplicates，失败 $failed';
    return dirs.isEmpty ? base : '$base（目录是 ${dirs.join("、")}）';
  }
}

/// 本机 ComfyUI 里已保存的一份工作流文件（`GET /api/capture/workflows`）。
///
/// **只读**：列表来自磁盘扫描，`path` 是「看蓝图 / 让 AI 用它」的唯一钥匙。
class ComfyWorkflowFile {
  final String name;
  final String path;
  final int bytes;
  final String modifiedAt;

  /// `ui`（界面格式） / `api`（API 节点图） / `unknown`
  final String format;
  final bool inLibrary;
  final int? promptId;

  const ComfyWorkflowFile({
    required this.name,
    required this.path,
    this.bytes = 0,
    this.modifiedAt = '',
    this.format = 'unknown',
    this.inLibrary = false,
    this.promptId,
  });

  factory ComfyWorkflowFile.fromJson(Map<String, dynamic> j) =>
      ComfyWorkflowFile(
        name: (j['name'] ?? '').toString(),
        path: (j['path'] ?? '').toString(),
        bytes: (j['bytes'] as num?)?.toInt() ?? 0,
        modifiedAt: (j['modifiedAt'] ?? '').toString(),
        format: (j['format'] ?? 'unknown').toString(),
        inLibrary: j['inLibrary'] == true,
        promptId: (j['promptId'] as num?)?.toInt(),
      );

  String get sizeLabel => bytes >= 1024 * 1024
      ? '${(bytes / 1024 / 1024).toStringAsFixed(1)} MB'
      : '${(bytes / 1024).toStringAsFixed(0)} KB';

  String get formatLabel => switch (format) {
    'ui' => '界面格式',
    'api' => 'API 格式',
    _ => '未知格式',
  };
}

/// `GET /api/capture/workflows` 的结果（文件清单 + 一句给用户看的话）。
class ComfyWorkflowList {
  final List<ComfyWorkflowFile> files;
  final List<String> dirs;
  final int total;
  final String? message;

  const ComfyWorkflowList({
    this.files = const [],
    this.dirs = const [],
    this.total = 0,
    this.message,
  });

  factory ComfyWorkflowList.fromJson(
    Map<String, dynamic> j,
  ) => ComfyWorkflowList(
    files: ((j['files'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => ComfyWorkflowFile.fromJson(Map<String, dynamic>.from(e)))
        .toList(),
    dirs: ((j['dirs'] as List?) ?? const []).map((e) => e.toString()).toList(),
    total: (j['total'] as num?)?.toInt() ?? 0,
    message: j['message']?.toString(),
  );

  bool get isEmpty => files.isEmpty;
}

// ===========================================================================

DateTime? _dt(Object? v) {
  if (v == null) return null;
  return DateTime.tryParse(v.toString())?.toLocal();
}
