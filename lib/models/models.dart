/// 提示词 / 产物 / 标签 的数据模型
///
/// 与后端 Ktor 的 DTO 一一对应，字段名使用 camelCase。
library;

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
        extraParams: ((j['extraParams'] as Map?) ?? const {})
            .map((k, v) => MapEntry(k.toString(), v.toString())),
        notes: j['notes'] as String?,
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
    bool? favorite,
    bool? hasWorkflow,
    List<Tag>? tags,
    int? mediaCount,
  }) =>
      Prompt(
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
  final String? notes;
  final DateTime? createdAt;
  final DateTime? updatedAt;

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
    this.notes,
    this.createdAt,
    this.updatedAt,
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
        fileUrl: (j['fileUrl'] ?? '') as String,
        thumbUrl: j['thumbUrl'] as String?,
      );

  bool get hasPrompt => promptId != null;
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

  factory Paged.fromJson(Map<String, dynamic> j, T Function(Map<String, dynamic>) item) =>
      Paged(
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
        byKind: ((j['byKind'] as Map?) ?? const {})
            .map((k, v) => MapEntry(k.toString(), (v as num).toInt())),
        byMediaKind: ((j['byMediaKind'] as Map?) ?? const {})
            .map((k, v) => MapEntry(k.toString(), (v as num).toInt())),
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
  }) =>
      CaptureConfig(
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
      outputDir != null && outputDir!.isNotEmpty && outputDir != configuredOutputDir;
}

class CapturePollResult {
  final bool ok;  final int checked;
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

  factory CapturePollResult.fromJson(Map<String, dynamic> j) => CapturePollResult(
        ok: (j['ok'] as bool?) ?? false,
        checked: (j['checked'] as num?)?.toInt() ?? 0,
        newRuns: (j['newRuns'] as num?)?.toInt() ?? 0,
        newMedia: (j['newMedia'] as num?)?.toInt() ?? 0,
        message: j['message'] as String?,
      );

  String get summary => ok
      ? '检查 $checked 次运行，新捕获 $newRuns 次（$newMedia 个产物）'
      : '同步失败：${message ?? "未知原因"}';
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

  factory ImportFolderResult.fromJson(Map<String, dynamic> j) => ImportFolderResult(
        dir: (j['dir'] ?? '') as String,
        scanned: (j['scanned'] as num?)?.toInt() ?? 0,
        imported: (j['imported'] as num?)?.toInt() ?? 0,
        duplicates: (j['duplicates'] as num?)?.toInt() ?? 0,
        failed: (j['failed'] as num?)?.toInt() ?? 0,
        promptsCreated: (j['promptsCreated'] as num?)?.toInt() ?? 0,
        errors: ((j['errors'] as List?) ?? const []).map((e) => e.toString()).toList(),
      );

  String get summary =>
      '扫描 $scanned 个文件：新导入 $imported，跳过重复 $duplicates，失败 $failed；'
      '其中 $promptsCreated 个带内嵌工作流已自动建提示词';
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

  factory ImportWorkflowsResult.fromJson(Map<String, dynamic> j) => ImportWorkflowsResult(
        dirs: ((j['dirs'] as List?) ?? const []).map((e) => e.toString()).toList(),
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
    final base = message ??
        '扫描 $scanned 份工作流文件：新导入 $imported，已在库里 $duplicates，失败 $failed';
    return dirs.isEmpty ? base : '$base（目录：${dirs.join("、")}）';
  }
}

// ===========================================================================

DateTime? _dt(Object? v) {
  if (v == null) return null;
  return DateTime.tryParse(v.toString())?.toLocal();
}
