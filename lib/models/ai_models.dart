/// AI 工作台的前端模型（AIH-006 / AIH-010 / AIH-012 / AIH-018 / AIH-019）。
///
/// 注意：这里**没有**任何保存 API Key 的字段 —— 密钥只写不读（AIH-012），
/// 前端能拿到的只有 [AiCredentialStatus]。
library;

import 'dart:convert';

import 'layout_embed.dart';
import 'models.dart';

/// 凭据状态：只有"有没有 / 从哪来 / 能不能改"，没有值。
class AiCredentialStatus {
  final bool configured;

  /// `env` / `managed` / `none`（桌面）与 `keystore`（手机本机密钥库）。
  ///
  /// ⚠ `keystore` **只在手机上出现**，而且它绝不能与 `managed` 混为一谈：
  /// 桌面的密钥**不下行**（总纲 §3.3：`ai_providers` 不同步），
  /// 混了用户就会以为"桌面配过、手机也能用" —— 而手机上其实一把都没有。
  final String source;
  final bool writable;

  const AiCredentialStatus({
    required this.configured,
    required this.source,
    required this.writable,
  });

  static const none = AiCredentialStatus(
    configured: false,
    source: 'none',
    writable: false,
  );

  factory AiCredentialStatus.fromJson(Map<String, dynamic> json) =>
      AiCredentialStatus(
        configured: json['configured'] == true,
        source: (json['source'] ?? 'none').toString(),
        writable: json['writable'] == true,
      );

  String get label => switch (source) {
    'env' => configured ? '已配置（环境变量，只读）' : '未配置（环境变量）',
    'managed' => '已配置',
    'keystore' => '已配置（本机密钥库）',
    _ => '未配置',
  };
}

/// 输入模态。未知即不支持，不猜（AIH-011）。
enum AiModality {
  text('text', '文本'),
  image('image', '图片'),
  video('video', '视频'),
  audio('audio', '音频'),
  document('document', '文档');

  const AiModality(this.wire, this.label);
  final String wire;
  final String label;

  static AiModality? parse(String? wire) {
    for (final m in values) {
      if (m.wire == wire) return m;
    }
    return null;
  }
}

/// 协议：首期三种，不含已淘汰的旧 `/completions`（DEC-002）。
enum AiApi {
  openaiCompletions('openai-completions', 'OpenAI Chat Completions'),
  openaiResponses('openai-responses', 'OpenAI Responses'),
  anthropicMessages('anthropic-messages', 'Anthropic Messages');

  const AiApi(this.wire, this.label);
  final String wire;
  final String label;

  static AiApi? parse(String? wire) {
    for (final a in values) {
      if (a.wire == wire) return a;
    }
    return null;
  }
}

/// 思考强度（AIH-056）。
///
/// "等级"是给用户看的，**能不能选、发什么值由模型目录决定**：
/// 模型声明了 `thinkingEfforts` 就只显示其中的等级；没声明推理能力就完全不给选。
///
/// 等级表与 pi-ai / DSH 对齐：`默认 → 极低 → 低 → 中 → 高 → 极高 → 最大`。
/// 前沿模型常常只声明到"极高"（没有 `max`），少了这一档就只能把用户的选择压回"高"。
enum AiReasoningEffort {
  /// 最低档：**不指定思考等级**，交给网关自己决定。
  ///
  /// 2026-09-20 由「关闭」改名而来（用户报的 bug：选了「关闭」却仍然在思考）。
  /// 真机核对（本机在用的 command-code-goat 网关）表明这个前提是错的：
  ///
  ///  · 它的 `reasoning_effort` 只认 `low|medium|high|xhigh|max`（报错原文），**没有 off**；
  ///  · `thinking{type:disabled}` / `enable_thinking:false` / `reasoning{enabled:false}` /
  ///    `thinking_budget:0` / `/no_think` / JSON null **全被忽略**，照样返回 reasoning；
  ///  · 不止 DeepSeek：MiMo V2.5、Kimi K2.6、Qwen3.7-Flash、MiniMax M2.5 也在思考。
  ///
  /// 也就是说"关掉思考"在这类网关上**做不到**，叫「关闭」就是承诺一件做不到的事。
  /// 叫「默认」才是真话 —— 我们不发任何思考参数，思考与否由网关决定
  /// （DSH 同一个档位也叫 `Default`）。要真正不思考，只能换一个不思考的模型/网关。
  off('off', '默认', hint: '不发思考参数，由网关决定，有的网关（如聚合站）仍会思考'),
  minimal('minimal', '极低'),
  low('low', '低'),
  medium('medium', '中'),
  high('high', '高'),
  xhigh('xhigh', '极高'),
  max('max', '最大');

  const AiReasoningEffort(this.wire, this.label, {this.hint});

  /// 线上 / 入库用的值。**`off` 这个名字保留**：它是数据库与 API 里的既有取值，
  /// 改它要动历史 Run 与各地图里的 `off` 键，而用户看到的是 [label]「默认」。
  final String wire;
  final String label;

  /// 一档的补充说明（可选）：菜单里当副标题、按钮上当 tooltip。
  final String? hint;

  static AiReasoningEffort? parse(String? wire) {
    for (final e in values) {
      if (e.wire == wire) return e;
    }
    return null;
  }

  /// 默认以外的等级（模型可选的"真的在指定思考强度"的档位）
  bool get isThinking => this != AiReasoningEffort.off;
}

/// 网关的思考方言：同一个"高"在 DeepSeek / Qwen / OpenRouter / Z.AI 上落到的字段不同。
enum AiThinkingFormat {
  openai('openai', 'OpenAI 风格（reasoning_effort）'),
  deepseek('deepseek', 'DeepSeek（thinking + reasoning_effort）'),
  qwen('qwen', 'Qwen（enable_thinking + reasoning_effort）'),
  openrouter('openrouter', 'OpenRouter（reasoning.effort）'),
  zai('zai', 'Z.AI（thinking + reasoning_effort）');

  const AiThinkingFormat(this.wire, this.label);
  final String wire;
  final String label;

  static AiThinkingFormat? parse(String? wire) {
    if (wire == null || wire.isEmpty) return null;
    for (final f in values) {
      if (f.wire == wire) return f;
    }
    return null;
  }
}

/// 端点信任级别（AIH-017）。
enum AiEndpointTrust {
  public_('public', '公网（仅 https）'),
  loopback('loopback', '仅本机'),
  privateNetwork('private-network', '局域网'),
  unsafeAny('unsafe-any', '不限制（慎用）');

  const AiEndpointTrust(this.wire, this.label);
  final String wire;
  final String label;

  static AiEndpointTrust parse(String? wire) => values.firstWhere(
    (t) => t.wire == wire,
    orElse: () => AiEndpointTrust.public_,
  );
}

class AiProvider {
  final String id;
  final String displayName;
  final String api;
  final String baseURL;
  final String? credentialRef;
  final String endpointTrust;
  final bool enabled;
  final int revision;
  final AiCredentialStatus credential;

  const AiProvider({
    required this.id,
    required this.displayName,
    required this.api,
    required this.baseURL,
    required this.credentialRef,
    required this.endpointTrust,
    required this.enabled,
    required this.revision,
    required this.credential,
  });

  factory AiProvider.fromJson(Map<String, dynamic> json) => AiProvider(
    id: (json['id'] ?? '').toString(),
    displayName: (json['displayName'] ?? json['id'] ?? '').toString(),
    api: (json['api'] ?? '').toString(),
    baseURL: (json['baseURL'] ?? '').toString(),
    credentialRef: json['credentialRef']?.toString(),
    endpointTrust: (json['endpointTrust'] ?? 'public').toString(),
    enabled: json['enabled'] != false,
    revision: (json['revision'] as num?)?.toInt() ?? 1,
    credential: json['credential'] is Map
        ? AiCredentialStatus.fromJson(
            Map<String, dynamic>.from(json['credential'] as Map),
          )
        : AiCredentialStatus.none,
  );

  AiApi? get apiEnum => AiApi.parse(api);
  String get apiLabel => apiEnum?.label ?? api;

  /// 落本地缓存用（M6）。`credential` **不进缓存**：它是"这台机器上有没有 key"的
  /// 即时状态，缓存下来的那一刻就可能已经过期了（手机上是本机密钥库说了算）。
  Map<String, dynamic> toJson() => {
    'id': id,
    'displayName': displayName,
    'api': api,
    'baseURL': baseURL,
    if (credentialRef != null) 'credentialRef': credentialRef,
    'endpointTrust': endpointTrust,
    'enabled': enabled,
    'revision': revision,
  };

  /// 只换凭据状态。
  ///
  /// 手机上清单来自**桌面**（`GET /api/ai/providers`），而"这台手机有没有 key"这件事
  /// 只有本机密钥库知道 —— 所以拿到的清单要用本机状态**覆盖** `credential` 再交给界面，
  /// 否则手机上会显示"已配置"，点发送却 401（而界面从头到尾没说过一句实话）。
  AiProvider withCredential(AiCredentialStatus status) => AiProvider(
    id: id,
    displayName: displayName,
    api: api,
    baseURL: baseURL,
    credentialRef: credentialRef,
    endpointTrust: endpointTrust,
    enabled: enabled,
    revision: revision,
    credential: status,
  );
}

class AiModel {
  final String providerId;
  final String id;
  final String displayName;
  final List<String> inputModalities;
  final Map<String, List<String>> attachmentTransports;
  final List<String> mimeAllowlist;
  final bool tools;
  final bool parallelTools;
  final bool reasoning;

  /// 该模型可选的思考等级（AIH-056）；空 = 不声明，UI 上不给选。
  final Map<String, String> thinkingEfforts;

  /// 网关思考方言：openai / deepseek / qwen / openrouter / zai；null = 按协议默认。
  final String? thinkingFormat;
  final int? contextWindow;
  final int? maxOutputTokens;
  final int? maxAttachmentCount;
  final String capabilitySource;
  final bool enabled;

  const AiModel({
    required this.providerId,
    required this.id,
    required this.displayName,
    this.inputModalities = const [],
    this.attachmentTransports = const {},
    this.mimeAllowlist = const [],
    this.tools = false,
    this.parallelTools = false,
    this.reasoning = false,
    this.thinkingEfforts = const {},
    this.thinkingFormat,
    this.contextWindow,
    this.maxOutputTokens,
    this.maxAttachmentCount,
    this.capabilitySource = 'manual',
    this.enabled = true,
  });

  factory AiModel.fromJson(Map<String, dynamic> json) => AiModel(
    providerId: (json['providerId'] ?? '').toString(),
    id: (json['id'] ?? '').toString(),
    displayName: (json['displayName'] ?? json['id'] ?? '').toString(),
    inputModalities:
        (json['inputModalities'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    attachmentTransports:
        (json['attachmentTransports'] as Map?)?.map(
          (k, v) => MapEntry(
            k.toString(),
            (v as List?)?.map((e) => e.toString()).toList() ?? const <String>[],
          ),
        ) ??
        const {},
    mimeAllowlist:
        (json['mimeAllowlist'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    tools: json['tools'] == true,
    parallelTools: json['parallelTools'] == true,
    reasoning: json['reasoning'] == true,
    thinkingEfforts:
        (json['thinkingEfforts'] as Map?)?.map(
          (k, v) => MapEntry(k.toString(), (v ?? '').toString()),
        ) ??
        const {},
    thinkingFormat: json['thinkingFormat']?.toString(),
    contextWindow: (json['contextWindow'] as num?)?.toInt(),
    maxOutputTokens: (json['maxOutputTokens'] as num?)?.toInt(),
    maxAttachmentCount: (json['maxAttachmentCount'] as num?)?.toInt(),
    capabilitySource: (json['capabilitySource'] ?? 'manual').toString(),
    enabled: json['enabled'] != false,
  );

  /// 落本地缓存用（M6：手机把桌面下发的模型目录存下来，离线时接着用）。
  ///
  /// **键名与后端逐字一致**，所以缓存读回来走的还是 [AiModel.fromJson] 这一条路 ——
  /// 另写一套"缓存格式"就会与后端漂移，而漂移的表现是"离线时能力声明少了几项"
  /// （附件发不出去、工具卡不显示），且没有任何东西会报错。
  Map<String, dynamic> toJson() => {
    'providerId': providerId,
    'id': id,
    'displayName': displayName,
    'inputModalities': inputModalities,
    'attachmentTransports': attachmentTransports,
    'mimeAllowlist': mimeAllowlist,
    'tools': tools,
    'parallelTools': parallelTools,
    'reasoning': reasoning,
    'thinkingEfforts': thinkingEfforts,
    if (thinkingFormat != null) 'thinkingFormat': thinkingFormat,
    if (contextWindow != null) 'contextWindow': contextWindow,
    if (maxOutputTokens != null) 'maxOutputTokens': maxOutputTokens,
    if (maxAttachmentCount != null) 'maxAttachmentCount': maxAttachmentCount,
    'capabilitySource': capabilitySource,
    'enabled': enabled,
  };

  /// 模型选择器上的能力徽标（AIH-048）：只显示能力目录里的声明，不猜测。
  List<AiModality> get modalities =>
      inputModalities.map(AiModality.parse).whereType<AiModality>().toList();

  bool supports(AiModality m) => inputModalities.contains(m.wire);

  /// 该模型能不能调思考强度：必须声明推理能力，且**至少声明了一个具体档位**。
  bool get supportsReasoningEffort =>
      reasoning &&
      thinkingEfforts.keys.any((k) => k != AiReasoningEffort.off.wire);

  /// 可选的思考档位（固定顺序：默认 → 极低 → 低 → 中 → 高 → 极高 → 最大）。
  ///
  /// **「默认」永远可选**（用户要求："允许我默认不指定思考强度，不影响模型声明"）：
  /// 它不发任何思考参数、任何网关都成立，所以不要求模型在 `thinkingEfforts` 里声明 `off`
  /// —— 否则"只想安静地问一句"的用户得先去改模型声明，这是本末倒置。
  /// **但它不承诺"不会思考"**：网关若默认开思考，它照样会思考（看 [AiReasoningEffort.off]）。
  /// 真正的思考档位仍然**只列模型声明过的**，不猜、不降级（AIH-056 第 2 条）。
  List<AiReasoningEffort> get selectableEfforts {
    if (!reasoning) return const [];
    final declared = thinkingEfforts.keys.toSet();
    return [
      AiReasoningEffort.off,
      ...AiReasoningEffort.values.where(
        (e) => e.isThinking && declared.contains(e.wire),
      ),
    ];
  }

  /// 该等级的线上表达（网关改名时显示给用户看）。数字表示 Anthropic 的思考预算。
  String? effortWireValue(AiReasoningEffort effort) =>
      thinkingEfforts[effort.wire];

  AiThinkingFormat? get thinkingFormatEnum =>
      AiThinkingFormat.parse(thinkingFormat);

  /// 能力来源提示：让用户知道这个声明是人填的还是探测来的（AIH-011）。
  String get capabilitySourceLabel => switch (capabilitySource) {
    'builtin' => '内置',
    'discovered' => '探测所得',
    'tested' => '已实测',
    _ => '手工声明',
  };
}

/// 连接测试结果（AIH-008）：只有状态与稳定错误码，没有任何凭据内容。
class AiProviderTestResult {
  final bool ok;
  final String? errorCode;
  final String message;
  final int? httpStatus;
  final int? modelCount;

  const AiProviderTestResult({
    required this.ok,
    required this.message,
    this.errorCode,
    this.httpStatus,
    this.modelCount,
  });

  factory AiProviderTestResult.fromJson(Map<String, dynamic> json) =>
      AiProviderTestResult(
        ok: json['ok'] == true,
        errorCode: json['errorCode']?.toString(),
        message: (json['message'] ?? '').toString(),
        httpStatus: (json['httpStatus'] as num?)?.toInt(),
        modelCount: (json['modelCount'] as num?)?.toInt(),
      );

  /// 给用户看的完整说明：稳定错误码在前，便于报障时对齐。
  String get display => errorCode == null ? message : '[$errorCode] $message';
}

/// 模型发现候选（AIH-009）。
///
/// 能力字段是**预填建议**而非断言：`capabilitySource` 说明判断从哪来
/// （接口声明 / 内置目录 / 未识别），用户加入前可以改。
class AiModelCandidate {
  final String id;
  final String displayName;
  final int? contextWindow;
  final int? maxOutputTokens;
  final List<AiModality> modalities;
  final bool tools;
  final bool reasoning;

  /// 预填的思考档位（来自内置目录；接口一般不声明这个）。
  final Map<String, String> thinkingEfforts;

  /// 预填的思考方言；null = 按协议默认。
  final String? thinkingFormat;

  /// discovered / builtin / unknown
  final String capabilitySource;
  final String? capabilityNote;

  const AiModelCandidate({
    required this.id,
    required this.displayName,
    this.contextWindow,
    this.maxOutputTokens,
    this.modalities = const [AiModality.text],
    this.tools = false,
    this.reasoning = false,
    this.thinkingEfforts = const {},
    this.thinkingFormat,
    this.capabilitySource = 'unknown',
    this.capabilityNote,
  });

  factory AiModelCandidate.fromJson(Map<String, dynamic> json) =>
      AiModelCandidate(
        id: (json['id'] ?? '').toString(),
        displayName: (json['displayName'] ?? json['id'] ?? '').toString(),
        contextWindow: (json['contextWindow'] as num?)?.toInt(),
        maxOutputTokens: (json['maxOutputTokens'] as num?)?.toInt(),
        modalities: ((json['modalities'] as List?) ?? const ['text'])
            .map((e) => AiModality.parse(e.toString()))
            .whereType<AiModality>()
            .toList(),
        tools: json['tools'] == true,
        reasoning: json['reasoning'] == true,
        thinkingEfforts:
            (json['thinkingEfforts'] as Map?)?.map(
              (k, v) => MapEntry(k.toString(), (v ?? '').toString()),
            ) ??
            const {},
        thinkingFormat: json['thinkingFormat']?.toString(),
        capabilitySource: (json['capabilitySource'] ?? 'unknown').toString(),
        capabilityNote: json['capabilityNote']?.toString(),
      );

  /// 能力来源的中文标签 —— 必须让用户看得出"这是谁说的"。
  String get sourceLabel => switch (capabilitySource) {
    'discovered' => '接口声明',
    'builtin' => '内置目录',
    'tested' => '已实测',
    'manual' => '手工声明',
    _ => '未识别（默认仅文本）',
  };

  bool get sourceIsGuess => capabilitySource == 'builtin';

  String get detail {
    final parts = <String>[];
    if (contextWindow != null) parts.add('上下文 $contextWindow');
    if (maxOutputTokens != null) parts.add('输出上限 $maxOutputTokens');
    return parts.isEmpty ? id : '$id · ${parts.join(' · ')}';
  }

  AiModelCandidate copyWith({List<AiModality>? modalities, bool? tools}) =>
      AiModelCandidate(
        id: id,
        displayName: displayName,
        contextWindow: contextWindow,
        maxOutputTokens: maxOutputTokens,
        modalities: modalities ?? this.modalities,
        tools: tools ?? this.tools,
        reasoning: reasoning,
        capabilitySource: capabilitySource,
        capabilityNote: capabilityNote,
      );
}

class AiDiscoverResult {
  final bool ok;
  final String? errorCode;
  final String message;
  final List<AiModelCandidate> candidates;

  const AiDiscoverResult({
    required this.ok,
    required this.message,
    this.errorCode,
    this.candidates = const [],
  });

  factory AiDiscoverResult.fromJson(Map<String, dynamic> json) =>
      AiDiscoverResult(
        ok: json['ok'] == true,
        errorCode: json['errorCode']?.toString(),
        message: (json['message'] ?? '').toString(),
        candidates:
            (json['candidates'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) =>
                      AiModelCandidate.fromJson(Map<String, dynamic>.from(e)),
                )
                .toList() ??
            const [],
      );

  String get display => errorCode == null ? message : '[$errorCode] $message';
}

/// Run 启动结果（AIH-020）：POST 返回 202 + runId，执行在后台。
class AiRunStart {
  final String runId;
  final String assistantMessageId;
  final String? userMessageId;

  const AiRunStart({
    required this.runId,
    required this.assistantMessageId,
    this.userMessageId,
  });

  factory AiRunStart.fromJson(Map<String, dynamic> json) => AiRunStart(
    runId: (json['runId'] ?? '').toString(),
    assistantMessageId: (json['assistantMessageId'] ?? '').toString(),
    userMessageId: json['userMessageId']?.toString(),
  );
}

/// 统一 Harness 事件（AIH-021）。Flutter 只认这一套，不解析供应商 SSE。
class AiRunEvent {
  final String type;
  final Map<String, dynamic> data;

  const AiRunEvent(this.type, this.data);

  String? get text => data['text']?.toString();
  String? get code => data['code']?.toString();
  String? get message => data['message']?.toString();
  String? get messageId => data['messageId']?.toString();

  /// 工具调用相关（M4）：`tool.*` 事件共用这几个字段。
  String? get callId => data['callId']?.toString();
  String? get toolName => data['name']?.toString();
  String? get arguments => data['arguments']?.toString();

  /// 模型写的一句"为什么调用"（`tool.requested` 里的可选字段，只给界面看）。
  String? get reason => data['reason']?.toString();
  String? get approval => data['approval']?.toString();
  String? get preview => data['preview']?.toString();
  int? get elapsedMs => (data['elapsedMs'] as num?)?.toInt();

  /// 工具轮数（`message.completed` 新增）。
  int? get steps => (data['steps'] as num?)?.toInt();

  /// 这一轮累计的思考正文（`message.completed` 里的 `reasoning`）。
  String? get reasoning => data['reasoning']?.toString();

  /// `message.completed` 的有序 parts（权威）；不是列表就是 null（老后端没有）。
  List<AiMessagePart>? get parts {
    final raw = data['parts'];
    if (raw is! List) return null;
    return raw
        .whereType<Map>()
        .map((e) => AiMessagePart.fromJson(Map<String, dynamic>.from(e)))
        .toList();
  }

  bool get isTerminal =>
      type == 'run.completed' ||
      type == 'run.failed' ||
      type == 'run.cancelled';
}

class AiConversation {
  final String id;
  final String title;
  final String? providerId;
  final String? modelId;
  final bool archived;
  final int messageCount;
  final DateTime? updatedAt;

  const AiConversation({
    required this.id,
    required this.title,
    this.providerId,
    this.modelId,
    this.archived = false,
    this.messageCount = 0,
    this.updatedAt,
  });

  factory AiConversation.fromJson(Map<String, dynamic> json) => AiConversation(
    id: (json['id'] ?? '').toString(),
    title: (json['title'] ?? '新对话').toString(),
    providerId: json['providerId']?.toString(),
    modelId: json['modelId']?.toString(),
    archived: json['archived'] == true,
    messageCount: (json['messageCount'] as num?)?.toInt() ?? 0,
    // 后端给的是 `Instant.toString()`（UTC，带 Z）；不转本地时区的话，
    // 列表里那个"最近一次对话"时间会整体偏掉一个时差（本机是 +08:00）。
    updatedAt: DateTime.tryParse((json['updatedAt'] ?? '').toString())
        ?.toLocal(),
  );

  AiConversation copyWith({
    String? title,
    int? messageCount,
    DateTime? updatedAt,
  }) => AiConversation(
    id: id,
    title: title ?? this.title,
    providerId: providerId,
    modelId: modelId,
    archived: archived,
    messageCount: messageCount ?? this.messageCount,
    updatedAt: updatedAt ?? this.updatedAt,
  );
}

/// 一次请求的 token 用量（AIH-057）。
///
/// 后端已经把各家 `usage` 方言归一化成这四个数，前端**不要再解析供应商字段**。
class AiTokenUsage {
  final int inputTokens;
  final int outputTokens;
  final int cachedTokens;
  final int reasoningTokens;

  const AiTokenUsage({
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.cachedTokens = 0,
    this.reasoningTokens = 0,
  });

  static const empty = AiTokenUsage();

  factory AiTokenUsage.fromJson(Map<String, dynamic> json) => AiTokenUsage(
    inputTokens: (json['inputTokens'] as num?)?.toInt() ?? 0,
    outputTokens: (json['outputTokens'] as num?)?.toInt() ?? 0,
    cachedTokens: (json['cachedTokens'] as num?)?.toInt() ?? 0,
    reasoningTokens: (json['reasoningTokens'] as num?)?.toInt() ?? 0,
  );

  /// 兼容后端历史数据：某些老消息的 `usage` 还是供应商原始对象（OpenAI/Anthropic 两种方言）。
  factory AiTokenUsage.fromRaw(Object? raw) {
    final map = raw is Map ? Map<String, dynamic>.from(raw) : null;
    if (map == null) return empty;
    int pick(List<String> keys) {
      for (final k in keys) {
        final v = map[k];
        if (v is num) return v.toInt();
        if (v is String) {
          final n = int.tryParse(v);
          if (n != null) return n;
        }
      }
      return 0;
    }

    int nested(String parent, String key) {
      final child = map[parent];
      if (child is Map) {
        final v = child[key];
        if (v is num) return v.toInt();
      }
      return 0;
    }

    var input = pick(['inputTokens', 'prompt_tokens', 'input_tokens']);
    final output = pick(['outputTokens', 'completion_tokens', 'output_tokens']);
    if (input == 0 && output == 0) {
      input = pick(['total_tokens', 'totalTokens']);
    }
    return AiTokenUsage(
      inputTokens: input,
      outputTokens: output,
      cachedTokens: [
        nested('prompt_tokens_details', 'cached_tokens'),
        nested('input_tokens_details', 'cached_tokens'),
        pick(['cachedTokens']),
      ].reduce((a, b) => a > b ? a : b),
      reasoningTokens: [
        nested('completion_tokens_details', 'reasoning_tokens'),
        pick(['reasoningTokens']),
      ].reduce((a, b) => a > b ? a : b),
    );
  }

  int get totalTokens => inputTokens + outputTokens;

  bool get isEmpty =>
      totalTokens == 0 && cachedTokens == 0 && reasoningTokens == 0;

  /// `↑1.2k ↓340` —— 输入/输出，聊天里最常看的一对。
  String get shortLabel => '↑${compact(inputTokens)} ↓${compact(outputTokens)}';

  String get detailLabel {
    final parts = <String>['输入 $inputTokens', '输出 $outputTokens'];
    if (cachedTokens > 0) parts.add('缓存命中 $cachedTokens');
    if (reasoningTokens > 0) parts.add('思考 $reasoningTokens');
    parts.add('合计 $totalTokens');
    return parts.join(' · ');
  }

  /// 大数字压缩成 `1.2K` / `15K` / `2.5M` / `1.1B`，聊天里不占地方。
  ///
  /// **单位逐级上台阶 K → M → B**（用户 2026-09-24："当数 token 用量字过大时，
  /// 切换 K/M/B 的单位"）。老实现只有 `k` 一档，`1_500_000` 会写成 **`1500k`** ——
  /// 四位数还带单位，比不压缩还长，正是用户看到的那一幕。
  ///
  /// 小数位口径沿用老实现：**换算后整数位不到两位才留一位小数**（`1.2K` / `15K`），
  /// 正好整数的尾巴（`2.0K`）去掉，别写成 `2.0K` 白占一个字。
  static String compact(int n) {
    final abs = n.abs();
    final (double scaled, String unit) = abs < 1000
        ? (n.toDouble(), '')
        : abs < 1000000
        ? (n / 1000, 'K')
        : abs < 1000000000
        ? (n / 1000000, 'M')
        : (n / 1000000000, 'B');
    if (unit.isEmpty) return '$n';
    final digits = scaled.abs() < 10
        ? scaled.toStringAsFixed(1)
        : scaled.toStringAsFixed(0);
    final trimmed = digits.endsWith('.0')
        ? digits.substring(0, digits.length - 2)
        : digits;
    return '$trimmed$unit';
  }
}

/// 会话级的 token 汇总（AIH-057）：列表里一眼看出"这个对话花了多少"。
class AiUsageSummary {
  final int inputTokens;
  final int outputTokens;
  final int cachedTokens;
  final int reasoningTokens;
  final int requests;

  const AiUsageSummary({
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.cachedTokens = 0,
    this.reasoningTokens = 0,
    this.requests = 0,
  });

  static const empty = AiUsageSummary();

  int get totalTokens => inputTokens + outputTokens;

  bool get isEmpty => requests == 0;

  /// 把一个会话里的消息用量加起来；没有 usage 的消息不计入 requests。
  factory AiUsageSummary.of(Iterable<AiMessage> messages) {
    var input = 0, output = 0, cached = 0, reasoning = 0, requests = 0;
    for (final m in messages) {
      final u = m.usage;
      if (u == null || u.isEmpty) continue;
      input += u.inputTokens;
      output += u.outputTokens;
      cached += u.cachedTokens;
      reasoning += u.reasoningTokens;
      requests++;
    }
    return AiUsageSummary(
      inputTokens: input,
      outputTokens: output,
      cachedTokens: cached,
      reasoningTokens: reasoning,
      requests: requests,
    );
  }

  /// `3 次请求 · ↑12.4k ↓3.1k · 合计 15.5k`
  String get label {
    if (isEmpty) return '本对话暂无 token 统计';
    final buf = StringBuffer(
      '$requests 次请求 · ↑${AiTokenUsage.compact(inputTokens)} '
      '↓${AiTokenUsage.compact(outputTokens)} · 合计 ${AiTokenUsage.compact(totalTokens)}',
    );
    if (cachedTokens > 0) {
      buf.write(' · 缓存命中 ${AiTokenUsage.compact(cachedTokens)}');
    }
    return buf.toString();
  }
}

/// 有序消息块（AIH-019）：正文、附件、工具调用、工具结果各占一块。
class AiMessagePart {
  final String type;
  final String? text;
  final String? attachmentId;
  final String? toolCallId;
  final Map<String, dynamic>? payload;

  const AiMessagePart({
    required this.type,
    this.text,
    this.attachmentId,
    this.toolCallId,
    this.payload,
  });

  factory AiMessagePart.fromJson(Map<String, dynamic> json) => AiMessagePart(
    type: (json['type'] ?? 'text').toString(),
    text: json['text']?.toString(),
    attachmentId: json['attachmentId']?.toString(),
    toolCallId: json['toolCallId']?.toString(),
    // `jsonPayload` 有**两种来路**，必须都认：
    // ① 桌面后端（Kotlin/JSON 对象）→ 直接就是 Map；
    // ② 手机桥（`MethodChannel` 的字符串载荷）→ 是一段 JSON 文本。
    // 只认 Map 的话，手机上每条 `tool_call` 的 `payload` 都是 null，
    // 表现是**工具卡上的工具名与参数永远空着**（正文还在，所以看着像"只是没显示全"）。
    payload: _payloadOf(json['jsonPayload']),
  );

  static Map<String, dynamic>? _payloadOf(Object? raw) {
    if (raw is Map) return Map<String, dynamic>.from(raw);
    if (raw is String && raw.trim().isNotEmpty) {
      try {
        final decoded = jsonDecode(raw);
        if (decoded is Map) return Map<String, dynamic>.from(decoded);
      } catch (_) {
        // 坏 payload 不能影响整条消息的解析
      }
    }
    return null;
  }

  /// `tool_call` 部分里模型给的工具名（其它类型返回 null）。
  String? get toolName {
    if (type != 'tool_call') return null;
    final map = payload;
    return map == null ? null : map['name']?.toString();
  }

  /// `tool_call` 部分里模型给的参数（JSON 文本，原样展示，前端不解析）。
  String? get toolArguments {
    if (type != 'tool_call') return null;
    final map = payload;
    return map == null ? null : map['arguments']?.toString();
  }
}

/// 六个**固定环节名**（与后端 `server-rs/src/step_mark.rs` 的 `PHASES` 逐字一致）。
///
/// 系统提示第 21 条只让模型用这六个；后端摘标记时也只认这六个（枚举外的名字会被摘掉、
/// 但**不记环节**）。这里再列一份是为了**手机端**：手机 harness 是本地跑模型的，
/// 后端那条摘取链路不在它前面 —— 不兜这一下，`[步骤]正在查资料[/步骤]` 就会原样显示在手机上。
const List<String> kStepPhases = [
  '正在分析请求',
  '正在查资料',
  '正在读文件',
  '正在生成图片',
  '正在写文件',
  '正在收尾',
];

/// 摘掉正文里的环节标记（`[步骤]正在查资料[/步骤]`）。
///
/// **只摘"名字恰好是那六个"的**：别的方括号文本（`[1]`、代码块里的 `[步骤]xxx`）一个字都不动
/// —— 与后端的"绝不吞内容"同一条纪律。桌面后端已经摘过一遍，这里是个幂等的兜底。
String stripStepMarkers(String text) {
  if (text.isEmpty || !text.contains('[步骤]')) return text;
  final pattern = RegExp(
    r'\[步骤\]\s*(' + kStepPhases.join('|') + r')\s*\[/步骤\]',
  );
  return text.replaceAll(pattern, '');
}

/// 一条助手消息里**按流顺序**排好的一段内容：思考段 / 正文段 / 环节段。
///
/// 为什么需要它：工具循环会让一次回复变成
/// `思考 → 正文 → 工具 → 思考 → 正文 …`，而人想看到的就是这个顺序。
/// 后端给的 `parts` 已经是权威的有序块，这里只是把**相邻的同类块合并**成段，
/// 免得一个气泡里冒出十几个碎段。
///
/// `step` 段（用户建议 2026-09-28）的 `text` 是**环节名**（后端从 `[步骤]…[/步骤]` 里摘的，
/// 只可能是那六个之一）：界面按它把这一环节里的工具卡折进一组。
class AiMessageSegment {
  /// reasoning / text / step（工具卡的位置占位是 `tool_call`，`text` 是 callId）
  final String type;
  final String text;

  const AiMessageSegment(this.type, this.text);

  bool get isReasoning => type == 'reasoning';

  /// 关键步骤（环节）段：`text` 是环节名。
  bool get isStep => type == 'step';

  /// 从有序块还原成段（相邻同类合并）。没有 parts 时返回空列表，调用方回退到实时状态。
  static List<AiMessageSegment> fromParts(List<AiMessagePart> parts) {
    final out = <AiMessageSegment>[];
    for (final p in parts) {
      if (p.type != 'reasoning' && p.type != 'text' && p.type != 'step') {
        continue;
      }
      final text = p.text ?? '';
      if (text.isEmpty) continue;
      // ⚠ 环节段**不合并**（哪怕相邻同名）：分不清"一个环节里的两段操作"与"两个环节"
      // —— 与实时段那边 `_appendLiveStep` 同一条纪律。
      if (p.type == 'step') {
        out.add(AiMessageSegment('step', text));
        continue;
      }
      if (out.isNotEmpty && out.last.type == p.type) {
        out[out.length - 1] = AiMessageSegment(p.type, out.last.text + text);
      } else {
        out.add(AiMessageSegment(p.type, text));
      }
    }
    return out;
  }

  /// 反过来：把段落列表拼回有序块（测试与排错时用）。
  static List<AiMessagePart> toParts(List<AiMessageSegment> segments) => [
    for (final s in segments) AiMessagePart(type: s.type, text: s.text),
  ];
}

class AiMessage {
  final String id;
  final String conversationId;
  final int seq;
  final String role; // user / assistant / tool / system_note
  final String status;
  final String text;
  final String? modelId;
  final List<AiMessagePart> parts;

  /// token 用量（AIH-057）：只对助手消息有值；老数据可能是供应商原始对象，已兼容解析。
  final AiTokenUsage? usage;

  /// 本消息实际使用的思考强度（AIH-056），来自 `message.completed` 事件。
  final String? reasoningEffort;

  const AiMessage({
    required this.id,
    required this.conversationId,
    required this.seq,
    required this.role,
    required this.status,
    required this.text,
    this.modelId,
    this.parts = const [],
    this.usage,
    this.reasoningEffort,
  });

  factory AiMessage.fromJson(Map<String, dynamic> json) => AiMessage(
    id: (json['id'] ?? '').toString(),
    conversationId: (json['conversationId'] ?? '').toString(),
    seq: (json['seq'] as num?)?.toInt() ?? 0,
    role: (json['role'] ?? 'assistant').toString(),
    status: (json['status'] ?? 'complete').toString(),
    text: (json['text'] ?? '').toString(),
    modelId: json['modelId']?.toString(),
    parts:
        (json['parts'] as List?)
            ?.whereType<Map>()
            .map((e) => AiMessagePart.fromJson(Map<String, dynamic>.from(e)))
            .toList() ??
        const [],
    usage: json['usage'] == null ? null : AiTokenUsage.fromRaw(json['usage']),
    reasoningEffort: json['reasoningEffort']?.toString(),
  );

  AiMessage copyWith({
    String? text,
    String? status,
    AiTokenUsage? usage,
    String? reasoningEffort,
    List<AiMessagePart>? parts,
  }) => AiMessage(
    id: id,
    conversationId: conversationId,
    seq: seq,
    role: role,
    status: status ?? this.status,
    text: text ?? this.text,
    modelId: modelId,
    parts: parts ?? this.parts,
    usage: usage ?? this.usage,
    reasoningEffort: reasoningEffort ?? this.reasoningEffort,
  );

  bool get isUser => role == 'user';
}

/// 附件（AIH-029 / M3）：准入结论由后端 preflight 给出，前端不自行放行。
///
/// 上传成功后后端会返回 `id`：界面靠它取**缩略图 / 视频预览帧**
/// （`GET /api/ai/attachments/{id}/thumb`），发送时也只传 id —— 文件在后端手里，
/// 前端不再需要把字节读进内存。
class AiAttachment {
  /// 后端附件 id；还没上传成功时为 null（本地占位 / 老用例直接构造）
  final String? id;
  final String name;

  /// image / video / audio / document / text（真源是后端的签名判定，AIH-027）
  final String? modality;
  final String mimeType;
  final int sizeBytes;

  /// 图片像素尺寸（后端探测；非图片为 null）
  final int? width;
  final int? height;

  const AiAttachment({
    this.id,
    required this.name,
    required this.modality,
    required this.mimeType,
    required this.sizeBytes,
    this.width,
    this.height,
  });

  /// 后端 `AiAttachmentDto` → 前端模型。
  factory AiAttachment.fromJson(Map<String, dynamic> json) => AiAttachment(
    id: json['id']?.toString(),
    name: (json['name'] ?? '附件').toString(),
    modality: json['modality']?.toString(),
    mimeType: (json['mimeType'] ?? 'application/octet-stream').toString(),
    sizeBytes: (json['sizeBytes'] as num?)?.toInt() ?? 0,
    width: (json['width'] as num?)?.toInt(),
    height: (json['height'] as num?)?.toInt(),
  );

  /// 预检用的"线索"（后端有 id 时以库里的判定为准，这里只是兼容老路径与草稿存储）
  Map<String, dynamic> toJson() => {
    if (id != null) 'id': id,
    'name': name,
    if (modality != null) 'modality': modality,
    'mimeType': mimeType,
    'sizeBytes': sizeBytes,
  };

  bool get isImage => modality == 'image';
  bool get isVideo => modality == 'video';
  bool get hasId => id != null && id!.isNotEmpty;

  String get sizeLabel {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) {
      return '${(sizeBytes / 1024).toStringAsFixed(0)} KB';
    }
    return '${(sizeBytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }
}

/// 一次附件上传的结果：成功的 + 逐个失败的原因（不静默丢弃）。
class AiAttachmentUploadResult {
  final List<AiAttachment> items;
  final List<({String fileName, String reason})> failed;

  const AiAttachmentUploadResult({
    this.items = const [],
    this.failed = const [],
  });

  factory AiAttachmentUploadResult.fromJson(Map<String, dynamic> json) =>
      AiAttachmentUploadResult(
        items:
            (json['items'] as List?)
                ?.whereType<Map>()
                .map((e) => AiAttachment.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
        failed:
            (json['failed'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) => (
                    fileName: (e['fileName'] ?? '附件').toString(),
                    reason: (e['reason'] ?? '上传失败').toString(),
                  ),
                )
                .toList() ??
            const [],
      );

  String get failedLabel =>
      failed.map((f) => '${f.fileName}，${f.reason}').join('。');
}

/// AI 附件缓存统计（`GET /api/ai/attachments/stats`）。
///
/// 裁图 / 搜图 / 用户上传都落成附件；这里只做展示，**清理规则在后端**
/// （被消息 / 提问引用的一个都不删，未引用的留 `graceDays` 天，超 `maxBytes` 淘汰）。
class AiAttachmentCacheStats {
  final int count;
  final int bytes;
  final int maxBytes;
  final int graceDays;
  final List<AiAttachmentOriginStats> byOrigin;

  const AiAttachmentCacheStats({
    this.count = 0,
    this.bytes = 0,
    this.maxBytes = 0,
    this.graceDays = 0,
    this.byOrigin = const [],
  });

  factory AiAttachmentCacheStats.fromJson(Map<String, dynamic> json) =>
      AiAttachmentCacheStats(
        count: (json['count'] as num?)?.toInt() ?? 0,
        bytes: (json['bytes'] as num?)?.toInt() ?? 0,
        maxBytes: (json['maxBytes'] as num?)?.toInt() ?? 0,
        graceDays: (json['graceDays'] as num?)?.toInt() ?? 0,
        byOrigin:
            (json['byOrigin'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) => AiAttachmentOriginStats.fromJson(
                    Map<String, dynamic>.from(e),
                  ),
                )
                .toList() ??
            const [],
      );

  int bytesOf(String origin) => byOrigin
      .where((o) => o.origin == origin)
      .fold(0, (sum, o) => sum + o.bytes);
}

class AiAttachmentOriginStats {
  final String origin;
  final int count;
  final int bytes;

  const AiAttachmentOriginStats({
    required this.origin,
    required this.count,
    required this.bytes,
  });

  factory AiAttachmentOriginStats.fromJson(Map<String, dynamic> json) =>
      AiAttachmentOriginStats(
        origin: (json['origin'] ?? 'upload').toString(),
        count: (json['count'] as num?)?.toInt() ?? 0,
        bytes: (json['bytes'] as num?)?.toInt() ?? 0,
      );

  String get label => switch (origin) {
    'crop' => '设定图裁片',
    'search' => '网络参考图',
    _ => '用户上传',
  };
}

/// 一次附件缓存 GC 的结果（`POST /api/ai/attachments/gc`）。
class AiAttachmentGcResult {
  final int removed;
  final int freedBytes;
  final int keptReferenced;
  final int orphanFiles;
  final int remaining;
  final int remainingBytes;

  const AiAttachmentGcResult({
    this.removed = 0,
    this.freedBytes = 0,
    this.keptReferenced = 0,
    this.orphanFiles = 0,
    this.remaining = 0,
    this.remainingBytes = 0,
  });

  factory AiAttachmentGcResult.fromJson(Map<String, dynamic> json) =>
      AiAttachmentGcResult(
        removed: (json['removed'] as num?)?.toInt() ?? 0,
        freedBytes: (json['freedBytes'] as num?)?.toInt() ?? 0,
        keptReferenced: (json['keptReferenced'] as num?)?.toInt() ?? 0,
        orphanFiles: (json['orphanFiles'] as num?)?.toInt() ?? 0,
        remaining: (json['remaining'] as num?)?.toInt() ?? 0,
        remainingBytes: (json['remainingBytes'] as num?)?.toInt() ?? 0,
      );
}

/// 单个附件的准入结论：界面用它给对应的缩略图打红框 / 悬浮显示原因。
class AiPreflightItem {
  final int index;
  final String name;
  final bool allowed;
  final List<String> blockers;
  final String? attachmentId;

  const AiPreflightItem({
    required this.index,
    required this.name,
    required this.allowed,
    this.blockers = const [],
    this.attachmentId,
  });

  factory AiPreflightItem.fromJson(Map<String, dynamic> json) =>
      AiPreflightItem(
        index: (json['index'] as num?)?.toInt() ?? 0,
        name: (json['name'] ?? '附件').toString(),
        allowed: json['allowed'] == true,
        blockers:
            (json['blockers'] as List?)?.map((e) => e.toString()).toList() ??
            const [],
        attachmentId: json['attachmentId']?.toString(),
      );
}

/// 预检结果：`allowed=false` 时必须阻断发送，不能"先发再说"（AIH-030）。
class AiPreflightResult {
  final bool allowed;
  final List<String> blockers;

  /// 逐个附件的结论（顺序与请求一致）：托盘里给被拦下的那张打标记
  final List<AiPreflightItem> items;

  const AiPreflightResult({
    required this.allowed,
    required this.blockers,
    this.items = const [],
  });

  static const allow = AiPreflightResult(allowed: true, blockers: []);

  factory AiPreflightResult.fromJson(Map<String, dynamic> json) =>
      AiPreflightResult(
        allowed: json['allowed'] == true,
        blockers:
            (json['blockers'] as List?)?.map((e) => e.toString()).toList() ??
            const [],
        items:
            (json['items'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) => AiPreflightItem.fromJson(Map<String, dynamic>.from(e)),
                )
                .toList() ??
            const [],
      );

  /// 第 [index] 个附件被拦下的原因（空列表 = 没问题）。
  List<String> blockersAt(int index) => items
      .where((it) => it.index == index)
      .expand((it) => it.blockers)
      .toList();
}

// ---------------------------------------------------------------------------
//  Skills（M5 / AIH-037 ~ AIH-045）
// ---------------------------------------------------------------------------

/// 一个 Skill 的元数据。
///
/// 正文**不在列表里**（可能几十 KB）：列表只给元数据，要正文得单独
/// [AiSkillDetail] 请求。`validationError` 非空表示磁盘上这份不合法 ——
/// 照样列出来（不静默忽略），但不能参与对话。
class AiSkill {
  final String name;
  final String description;
  final String? whenToUse;
  final String? version;

  /// builtin（项目自带、只读）/ user（用户或 AI 注册、可删）
  final String source;
  final bool enabled;
  final bool userInvocable;
  final bool modelInvocable;

  /// 正文 sha256 前 16 位（变更审计用），后端没给就是 null。
  final String? digest;
  final int sizeBytes;
  final int fileCount;
  final DateTime? updatedAt;

  /// 非空 = 这份 skill 不合法，界面要标红并按 [validationError] 说明原因。
  final String? validationError;

  /// 非空的"提示"类信息（例如覆盖了同名内置 skill）：不影响可用性。
  final String? conflict;

  /// 来源说明（例如 `dsh:%USERPROFILE%\.dsh\skills`）。
  final String? origin;

  const AiSkill({
    required this.name,
    this.description = '',
    this.whenToUse,
    this.version,
    this.source = 'user',
    this.enabled = true,
    this.userInvocable = true,
    this.modelInvocable = true,
    this.digest,
    this.sizeBytes = 0,
    this.fileCount = 1,
    this.updatedAt,
    this.validationError,
    this.conflict,
    this.origin,
  });

  factory AiSkill.fromJson(Map<String, dynamic> json) => AiSkill(
    name: (json['name'] ?? '').toString(),
    description: (json['description'] ?? '').toString(),
    whenToUse: json['whenToUse']?.toString(),
    version: json['version']?.toString(),
    source: (json['source'] ?? 'user').toString(),
    // 后端缺字段时按"可用"处理，不要凭空把 skill 变成停用
    enabled: json['enabled'] != false,
    userInvocable: json['userInvocable'] != false,
    modelInvocable: json['modelInvocable'] != false,
    digest: json['digest']?.toString(),
    sizeBytes: (json['sizeBytes'] as num?)?.toInt() ?? 0,
    fileCount: (json['fileCount'] as num?)?.toInt() ?? 1,
    updatedAt: DateTime.tryParse((json['updatedAt'] ?? '').toString()),
    validationError: json['validationError']?.toString(),
    conflict: json['conflict']?.toString(),
    origin: json['origin']?.toString(),
  );

  bool get isBuiltin => source == 'builtin';

  /// 列表上的来源徽标：内置 / 用户。
  String get sourceLabel => isBuiltin ? '内置' : '用户';

  /// 有没有需要提醒用户的问题（不合法或冲突）。
  bool get hasWarning =>
      (validationError ?? '').isNotEmpty || (conflict ?? '').isNotEmpty;

  /// 悬浮提示上的完整说明：问题优先，其次冲突，最后来源。
  String get warningText {
    if ((validationError ?? '').isNotEmpty) return '不合法，$validationError';
    if ((conflict ?? '').isNotEmpty) return conflict!;
    return origin == null ? sourceLabel : '$sourceLabel · $origin';
  }

  String get sizeLabel {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) {
      return '${(sizeBytes / 1024).toStringAsFixed(1)} KB';
    }
    return '${(sizeBytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }

  /// 一行副标题：描述写不下时用 whenToUse 兜底。
  String get oneLine {
    final text = description.isNotEmpty ? description : (whenToUse ?? '');
    return text.replaceAll('\n', ' ');
  }
}

/// skill 详情：元数据 + 正文（打开详情时才请求）。
class AiSkillDetail {
  final AiSkill skill;
  final String content;

  const AiSkillDetail({required this.skill, required this.content});

  factory AiSkillDetail.fromJson(Map<String, dynamic> json) => AiSkillDetail(
    skill: json['skill'] is Map
        ? AiSkill.fromJson(Map<String, dynamic>.from(json['skill'] as Map))
        : const AiSkill(name: ''),
    content: (json['content'] ?? '').toString(),
  );
}

/// skills 投放口的位置（用户建议：不要「从 DSH 导入」按钮，改成"往文件夹里拷"）。
///
/// 两种运行布局都由后端算好：源码树是 `<项目根>\storage\ai\skills`，
/// 发布包是 `<根>\storage\ai\skills`（便携式）。
class AiSkillRoots {
  final String userRoot;
  final String builtinRoot;
  final bool userRootExists;

  const AiSkillRoots({
    this.userRoot = '',
    this.builtinRoot = '',
    this.userRootExists = false,
  });

  factory AiSkillRoots.fromJson(Map<String, dynamic> json) => AiSkillRoots(
    userRoot: (json['userRoot'] ?? '').toString(),
    builtinRoot: (json['builtinRoot'] ?? '').toString(),
    userRootExists: json['userRootExists'] == true,
  );
}

/// 「重新扫描投放口」的结果：这次自动登记了什么 + 扫描后的完整清单。
class AiSkillRescanResult {
  final int registered;
  final List<String> names;
  final List<String> errors;
  final List<AiSkill> skills;

  const AiSkillRescanResult({
    this.registered = 0,
    this.names = const [],
    this.errors = const [],
    this.skills = const [],
  });

  factory AiSkillRescanResult.fromJson(Map<String, dynamic> json) =>
      AiSkillRescanResult(
        registered: (json['registered'] as num?)?.toInt() ?? 0,
        names:
            (json['names'] as List?)?.map((e) => e.toString()).toList() ??
            const [],
        errors:
            (json['errors'] as List?)?.map((e) => e.toString()).toList() ??
            const [],
        skills:
            (json['skills'] as List?)
                ?.whereType<Map>()
                .map((e) => AiSkill.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
      );

  /// 要给用户看的话：**失败条目也要说**，不能只报"成功 N 个"。
  String get summary {
    final parts = <String>['扫描到 ${skills.length} 个 skill'];
    if (registered > 0) {
      parts.add('自动登记 $registered 个（${names.take(3).join('、')}）');
    }
    if (errors.isNotEmpty) {
      parts.add('${errors.length} 个没法用，${errors.take(2).join('。')}');
    }
    return parts.join(' · ');
  }
}

/// 长期记忆（M6）：真源是 `<storage>\ai\memory.md`。
class AiMemory {
  final String content;
  final String path;
  final int entryCount;
  final int maxChars;

  /// 条数上限（用户要求：条数太多，查找 / 改动 / 删除都会变难）
  final int maxEntries;

  const AiMemory({
    this.content = '',
    this.path = '',
    this.entryCount = 0,
    this.maxChars = 8000,
    this.maxEntries = 100,
  });

  factory AiMemory.fromJson(Map<String, dynamic> json) => AiMemory(
    content: (json['content'] ?? '').toString(),
    path: (json['path'] ?? '').toString(),
    entryCount: (json['entryCount'] as num?)?.toInt() ?? 0,
    maxChars: (json['maxChars'] as num?)?.toInt() ?? 8000,
    maxEntries: (json['maxEntries'] as num?)?.toInt() ?? 100,
  );

  /// 一行一条的正文（去掉空行）。**下标与后端 `MemoryStore.entries()` 同源** ——
  /// 界面按这份列表做单条 / 批量删除，顺手把 `- ` 与 `[日期] ` 前缀剥掉只用于展示。
  List<String> get entries => content
      .split('\n')
      .map((l) => l.trim())
      .where((l) => l.isNotEmpty)
      .map(
        (l) => l
            .replaceFirst(RegExp(r'^[-*]\s*'), '')
            .replaceFirst(_datePrefix, '')
            .trim(),
      )
      .toList();

  static final _datePrefix = RegExp(r'^\[\d{4}-\d{2}-\d{2}]\s*');
}

// ---------------------------------------------------------------------------
//  卡片库：角色卡 / 画风卡（2026-10-21）
// ---------------------------------------------------------------------------

/// 一张卡（角色卡 / 画风卡）的公共形状。
///
/// 每张卡只有**一版正面提示词** [prompt]：整段自然语言。
/// 2026-10-24 用户口径：**不再保留 Danbooru tag 版** —— 一句话描述比 tag 栈更耐改
/// （换底模 / 换 tag 体系都不用重写），要 tag 版时让 AI 按用户要求当场另翻一版即可。
/// 老库里存着的 tag 版由后端迁移转成自然语言灌进 [prompt]（已有正文的卡不动）。
///
/// [examples] 是这张卡的**示例产物**（画廊里的 mediaId）：
/// 界面上画成缩略图，AI 那边用 `#id「标题」` 的形式看到。
class CharacterCard {
  final int id;
  final String name;

  /// 正面提示词（整段自然语言）。
  final String? prompt;

  /// 负面提示词（整段文本）。
  final String? negative;

  /// 示例产物（按加入顺序）。
  final List<MediaAsset> examples;

  final String? updatedAt;

  const CharacterCard({
    required this.id,
    required this.name,
    this.prompt,
    this.negative,
    this.examples = const [],
    this.updatedAt,
  });

  static String? _text(dynamic raw) {
    final s = raw?.toString().trim();
    return (s == null || s.isEmpty) ? null : s;
  }

  factory CharacterCard.fromJson(Map<String, dynamic> json) => CharacterCard(
    id: (json['id'] as num?)?.toInt() ?? 0,
    name: (json['name'] ?? '').toString(),
    prompt: _text(json['prompt']),
    negative: _text(json['negative']),
    examples: ((json['examples'] as List?) ?? const [])
        .whereType<Map>()
        .map((e) => MediaAsset.fromJson(Map<String, dynamic>.from(e)))
        .toList(),
    updatedAt: _text(json['updatedAt']),
  );

  /// 正文写完了才算完整（前端只做提示，不阻止保存）。
  bool get hasPrompt => prompt != null;
}

/// 画风卡：字段与 [CharacterCard] 逐项同形，只是语义是"画成什么样"。
///
/// 单独一个类是为了让调用点读起来是画风卡（`StyleCard`），
/// 而不是在 UI 里到处传一个叫 `CharacterCard` 的东西。
class StyleCard extends CharacterCard {
  const StyleCard({
    required super.id,
    required super.name,
    super.prompt,
    super.negative,
    super.examples,
    super.updatedAt,
  });

  factory StyleCard.fromJson(Map<String, dynamic> json) {
    final card = CharacterCard.fromJson(json);
    return StyleCard(
      id: card.id,
      name: card.name,
      prompt: card.prompt,
      negative: card.negative,
      examples: card.examples,
      updatedAt: card.updatedAt,
    );
  }
}

// ---------------------------------------------------------------------------
//  工具与权限（M4 / AIH-031 ~ AIH-036 / AIH-049）
// ---------------------------------------------------------------------------
/// 一个工具的生效权限（`GET /api/ai/tools`）。
class AiToolInfo {
  final String name;
  final String description;

  /// skill / files / comfy / memory / web —— 界面按它选图标。
  final String category;

  /// 会不会写盘（界面要标出来）。
  final bool mutating;

  /// allow / ask / deny
  final String access;

  /// 是否被用户设置覆盖过（没覆盖 = 出厂默认）。
  final bool overridden;

  const AiToolInfo({
    required this.name,
    this.description = '',
    this.category = 'comfy',
    this.mutating = false,
    this.access = 'ask',
    this.overridden = false,
  });

  factory AiToolInfo.fromJson(Map<String, dynamic> json) => AiToolInfo(
    name: (json['name'] ?? '').toString(),
    description: (json['description'] ?? '').toString(),
    category: (json['category'] ?? 'comfy').toString(),
    mutating: json['mutating'] == true,
    access: (json['access'] ?? 'ask').toString(),
    overridden: json['overridden'] == true,
  );

  String get categoryLabel => switch (category) {
    'skill' => 'Skills',
    'files' => '文件',
    'comfy' => 'ComfyUI',
    // 本机 LoRA 训练引擎（anima-lora-next）：项目 / 数据集 / 打标 / 预处理 / 训练
    'train' => '训练',
    'memory' => '记忆',
    // 联网搜索（2026-09-20）：唯一会把数据送出本机的类别
    'web' => '联网搜索',
    // O7/O8（2026-09-25）：搜自己的提示词库 / 读写角色卡
    'library' => '资料库',
    // 2D 布局模板（2026-11-03）：写提示词时同轮给的构图草稿
    'layout' => '布局',
    _ => category,
  };

  String get accessLabel => AiToolPolicy.accessLabel(access);

  bool get isDenied => access == 'deny';
}

/// 工具权限策略（`GET/PUT /api/ai/tools/policy`）。
///
/// 界面只需要"写/读白名单 + 逐工具覆盖 + 预算"这几项；字节上限后端自己管，
/// 只读展示，不在这里改。
class AiToolPolicy {
  final List<String> writeRoots;
  final List<String> readRoots;

  /// **自动发现**的只读目录（本机 ComfyUI 的安装 / 共享目录）。
  ///
  /// 与 [readRoots] 分开：那一份是用户配的（可加可删），这一份是后端每次现算的探测结果
  /// （用户 bug ④："应该自动发现 comfy 目录，给 comfy 的目录默认白名单"）。
  /// 界面上只展示、不编辑。
  final List<String> autoReadRoots;

  /// toolName → allow / ask / deny
  final Map<String, String> overrides;
  final int maxToolSteps;
  final int maxCallsPerRun;
  final int maxReadBytes;
  final int maxWriteBytes;

  /// 出厂默认的写入目录：界面上要大声说明"默认只能写这里"。
  final String? defaultWriteRoot;

  /// 权限档（用户建议 ⑤）：`ask` 默认，需要审批的工具要用户点批准；
  /// `full` 自动允许（无需批准），见 [modeFullLabel]。
  final String permissionMode;

  static const modeAsk = 'ask';
  static const modeFull = 'full';

  /// 界面上给两档起的名字。
  ///
  /// `full` 以前叫「完全权限」，用户指出这个名字不准确：它只免掉"问一下"，
  /// 并没有放开文件夹白名单。所以改叫「自动允许（无需批准）」——
  /// 名字直接说清它做了什么、没做什么（用户建议）。
  static const modeAskLabel = '询问';
  static const modeFullLabel = '自动允许（无需批准）';

  const AiToolPolicy({
    this.writeRoots = const [],
    this.readRoots = const [],
    this.autoReadRoots = const [],
    this.overrides = const {},
    this.maxToolSteps = 25,
    this.maxCallsPerRun = 48,
    this.maxReadBytes = 0,
    this.maxWriteBytes = 0,
    this.defaultWriteRoot,
    this.permissionMode = modeAsk,
  });

  factory AiToolPolicy.fromJson(Map<String, dynamic> json) => AiToolPolicy(
    writeRoots:
        (json['writeRoots'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    readRoots:
        (json['readRoots'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    autoReadRoots:
        (json['autoReadRoots'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    overrides:
        (json['overrides'] as Map?)?.map(
          (k, v) => MapEntry(k.toString(), v.toString()),
        ) ??
        const {},
    maxToolSteps: (json['maxToolSteps'] as num?)?.toInt() ?? 25,
    maxCallsPerRun: (json['maxCallsPerRun'] as num?)?.toInt() ?? 48,
    maxReadBytes: (json['maxReadBytes'] as num?)?.toInt() ?? 0,
    maxWriteBytes: (json['maxWriteBytes'] as num?)?.toInt() ?? 0,
    defaultWriteRoot: json['defaultWriteRoot']?.toString(),
    // 认不出的档位一律当「询问」：权限这种事不猜
    permissionMode: json['permissionMode'] == modeFull ? modeFull : modeAsk,
  );

  static const accessOptions = <String, String>{
    'allow': '允许',
    'ask': '询问',
    'deny': '拒绝',
  };

  static String accessLabel(String? access) => accessOptions[access] ?? '询问';

  AiToolPolicy copyWith({
    List<String>? writeRoots,
    List<String>? readRoots,
    List<String>? autoReadRoots,
    Map<String, String>? overrides,
    int? maxToolSteps,
    int? maxCallsPerRun,
  }) => AiToolPolicy(
    writeRoots: writeRoots ?? this.writeRoots,
    readRoots: readRoots ?? this.readRoots,
    autoReadRoots: autoReadRoots ?? this.autoReadRoots,
    overrides: overrides ?? this.overrides,
    maxToolSteps: maxToolSteps ?? this.maxToolSteps,
    maxCallsPerRun: maxCallsPerRun ?? this.maxCallsPerRun,
    maxReadBytes: maxReadBytes,
    maxWriteBytes: maxWriteBytes,
    defaultWriteRoot: defaultWriteRoot,
    permissionMode: permissionMode,
  );

  /// 界面上那句"默认只能写哪里"。
  String get defaultWriteHint =>
      defaultWriteRoot == null || defaultWriteRoot!.isEmpty
      ? '默认只能写后端预设的产物目录，其它位置一律拒绝。'
      : '默认只能写 $defaultWriteRoot，其它位置一律拒绝。';
}

/// NSFW 内容设置（2026-09-20 用户要求：把那段"越狱"系统提示词做成开关 + 预览图模糊选项）。
///
/// 两个开关**放一起**（用户原话），而且**真源在后端**（`app_settings` 的 `ai.nsfw`）：
/// [enabled] 决定后端拼系统提示词时注不注入那一段，前端自己存一份就会出现
/// "界面显示开着、后端其实没注入"这种自相矛盾的状态。
///
/// [effectiveBlur] 由后端算好（= `enabled && blur`）：关了 NSFW 开关就不存在
/// "NSFW 内容"这个概念，那时把整页图打码只会让人以为应用坏了。
class AiNsfwSettings {
  /// 是否注入 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 那段越狱提示词。
  final bool enabled;

  /// 是否把预览图模糊显示（只有 [enabled] 也打开时才生效）。
  final bool blur;

  /// 真正生效的"打不打码" = `enabled && blur`。**前端不要自己再算一遍。**
  final bool effectiveBlur;

  /// 注入正文的版本号（后端 `NsfwPrompt.VERSION`），界面用它说明"这版是哪一版"。
  final String promptVersion;

  const AiNsfwSettings({
    this.enabled = false,
    this.blur = false,
    this.effectiveBlur = false,
    this.promptVersion = '',
  });

  factory AiNsfwSettings.fromJson(Map<String, dynamic> json) => AiNsfwSettings(
    enabled: json['enabled'] == true,
    blur: json['blur'] == true,
    effectiveBlur: json['effectiveBlur'] == true,
    promptVersion: (json['promptVersion'] ?? '').toString(),
  );

  AiNsfwSettings copyWith({bool? enabled, bool? blur}) => AiNsfwSettings(
    enabled: enabled ?? this.enabled,
    blur: blur ?? this.blur,
    // 本地先按同一套规则预示；写回后以后端返回的为准
    effectiveBlur: (enabled ?? this.enabled) && (blur ?? this.blur),
    promptVersion: promptVersion,
  );
}

/// AI 向用户提的一个问题（用户建议 ⑥，2026-09-20："让 AI 像 deepseek harness 一样
/// 支持提问功能，回答结束时不中断当前对话"）。
///
/// 它与「需要批准的工具」长得像、语义完全不同：批准是"允不允许"，提问是"缺哪条信息"；
/// 而且**这次 Run 不会因此结束** —— 用户答完，答案作为工具结果喂回模型，同一个回复接着做。
class AiQuestion {
  final String questionId;
  final String runId;
  final String conversationId;
  final String question;

  /// **发起这次提问的工具调用号**（`tool.requested` 里的 `callId`，形如 `call_00_…`）。
  ///
  /// 必须与 [questionId] 分开存（用户 bug 2026-09-21 的根因）：
  /// `user.question` 事件里只有 `questionId`（后端生成的 UUID），**没有 `callId`**；
  /// 而气泡里那张卡的"槽位"是按 `callId` 排的（`tool.requested` 先到）。
  /// 两者混用就会出现"工具卡在跑、提问卡消失、输入区不接管"——
  /// 所以调用方（`AiWorkspaceStore`）在收到 `user.question` 时把当前那条
  /// `ask_user_question` 调用的 `callId` 贴进来，答题与切状态都按它定位。
  ///
  /// 历史消息（`parts`）里没有这个字段，那时按 [questionId] 反查即可。
  final String? callId;

  /// 界面画成按钮的候选答案（可能为空，那就只能自己敲）。
  final List<String> options;

  /// 与 [options] 一一对应的选项示意图（空串 = 这一项没有图）。
  ///
  /// 2026-11-04「按设定图逐部位确认」：AI 从设定图上裁一块当题目配图，
  /// 候选款式有可靠示例图时贴在**选项文字前面**。长度可能比 [options] 短
  /// （旧数据 / 手机回的），读之前先按下标判空。
  final List<String> optionImages;

  /// 题目配图（相对后端根的地址；界面补 baseUrl 之后显示）。
  ///
  /// 裁图是后端做的（`ask_user_question` 的 `image.attachmentId` + `region`），
  /// 这里只拿结果地址 —— 前端不碰解码、也不自己拼附件路径。
  final String? imageUrl;

  /// 允许自己敲一段（后端目前恒为 true —— 选项不一定对得上用户的需求）。
  final bool allowFreeText;

  /// 可选小标题（例如「参考图」「输出尺寸」）。
  final String? header;

  /// 用户已经答过（界面切成"已回答"）。
  final bool answered;
  final String? answer;
  final int? optionIndex;

  /// 同一张提问卡里的第几题（0 基）与总题数；单问时是 `0 / 1`。
  ///
  /// 2026-09-22 用户拍板的形态（照 DeepSeek Harness）：**一张卡装多个问题、分页、
  /// 可逐题跳过、最后一页按钮变「提交」、答案可为空**。同一次 `ask_user_question`
  /// 调用生成的多条共享 [groupId]，界面按它把这几条画成同一张卡的 N 页。
  final int index;
  final int total;

  /// 这张卡的分组号（单问时是空串 —— 用 [key] 归一）。
  final String groupId;

  /// 多选题：选项可以多选（界面画复选框）。后端上限 5 个选项。
  final bool multiSelect;

  /// 多选题用户勾了哪几个（自己敲的自由文本仍走 [answer]）。
  final List<int> optionIndexes;

  /// 分组键：老数据 / 单问没有 `groupId`，就用 `questionId` 归一组。
  String get key => groupId.isNotEmpty ? groupId : questionId;

  /// 这张卡不止一题（界面要画分页与进度）。
  bool get isPaged => total > 1;

  /// 第 [index] 个选项的示意图地址（没有 / 越界都回 null）。
  String? optionImageAt(int index) {
    if (index < 0 || index >= optionImages.length) return null;
    final url = optionImages[index].trim();
    return url.isEmpty ? null : url;
  }

  const AiQuestion({
    required this.questionId,
    this.runId = '',
    this.conversationId = '',
    required this.question,
    this.callId,
    this.options = const [],
    this.optionImages = const [],
    this.imageUrl,
    this.allowFreeText = true,
    this.header,
    this.answered = false,
    this.answer,
    this.optionIndex,
    this.index = 0,
    this.total = 1,
    this.groupId = '',
    this.multiSelect = false,
    this.optionIndexes = const [],
  });

  factory AiQuestion.fromJson(Map<String, dynamic> json) => AiQuestion(
    questionId: (json['questionId'] ?? '').toString(),
    runId: (json['runId'] ?? '').toString(),
    conversationId: (json['conversationId'] ?? '').toString(),
    question: (json['question'] ?? '').toString(),
    callId: json['callId']?.toString(),
    options:
        (json['options'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    optionImages:
        (json['optionImages'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    imageUrl: (json['imageUrl']?.toString().trim().isEmpty ?? true)
        ? null
        : json['imageUrl'].toString(),
    allowFreeText: json['allowFreeText'] != false,
    header: json['header']?.toString(),
    answered: json['answered'] == true,
    answer: json['answer']?.toString(),
    optionIndex: (json['optionIndex'] as num?)?.toInt(),
    index: (json['index'] as num?)?.toInt() ?? 0,
    total: (json['total'] as num?)?.toInt() ?? 1,
    groupId: (json['groupId'] ?? '').toString(),
    multiSelect: json['multiSelect'] == true,
    optionIndexes:
        (json['optionIndexes'] as List?)
            ?.whereType<num>()
            .map((e) => e.toInt())
            .toList() ??
        const [],
  );

  AiQuestion copyWith({
    String? callId,
    bool? answered,
    String? answer,
    int? optionIndex,
    List<int>? optionIndexes,
  }) => AiQuestion(
    questionId: questionId,
    runId: runId,
    conversationId: conversationId,
    question: question,
    callId: callId ?? this.callId,
    options: options,
    optionImages: optionImages,
    imageUrl: imageUrl,
    allowFreeText: allowFreeText,
    header: header,
    answered: answered ?? this.answered,
    answer: answer ?? this.answer,
    optionIndex: optionIndex ?? this.optionIndex,
    index: index,
    total: total,
    groupId: groupId,
    multiSelect: multiSelect,
    optionIndexes: optionIndexes ?? this.optionIndexes,
  );
}

/// 一次工具调用的状态机。
enum AiToolCallStatus {
  running('running', '运行中'),
  pendingApproval('pending_approval', '待批准'),
  ok('ok', '已完成'),
  failed('failed', '失败'),
  denied('denied', '已拒绝');

  const AiToolCallStatus(this.wire, this.label);
  final String wire;
  final String label;

  static AiToolCallStatus parse(String? wire) {
    for (final s in values) {
      if (s.wire == wire) return s;
    }
    return AiToolCallStatus.running;
  }
}

/// 一条工具调用（气泡里的工具卡）。
///
/// 两种来源共用这一个结构：
///  - **流式**：`tool.requested / started / completed / failed` 事件逐条更新；
///  - **历史**：`message.parts` 里的 `tool_call` + `tool_result` 配对还原（见 [listFromParts]）。
class AiToolCallState {
  final String callId;
  final String name;

  /// 模型给的参数原文（JSON 文本）。**原样展示**，前端不解析、不执行。
  final String arguments;

  /// 模型写的一句"为什么调用"（后端从参数里摘出来，**不会传给工具**）。
  ///
  /// 用户 2026-10-21：工具卡不要只显示 `comfy_load_workflow` 这种机器名。
  /// 有它就把它当标题（一眼看出这次在干什么），工具名退到副标题。
  final String? reason;
  final AiToolCallStatus status;

  /// 结果预览（后端已按 MAX_RESULT_CHARS 截断）。
  final String? preview;

  /// 失败 / 拒绝的原因（稳定错误码或消息）。
  final String? error;
  final int elapsedMs;

  /// not_required / pending / approved / denied
  final String? approval;

  /// 这次工具调用**产出的画廊产物 id**（`comfy_submit` 的 mediaIds）。
  ///
  /// 用于在回复末尾贴「画廊入口卡」（用户建议 ⑤）：AI 真的生成了东西，
  /// 就该在回复下面给一个能点开看详情的入口，而不是让用户自己去画廊里翻。
  final List<int> mediaIds;

  /// 这次工具调用**声明为成人内容**的那几个产物 id（`comfy_submit` 的 `nsfwMediaIds`）。
  ///
  /// 用户 bug 2026-09-20：模糊开关一开，回复里的产物卡把**所有**缩略图都糊了。
  /// 现在只糊 AI 明说过的那几张（它提交时写了 `nsfw=true`），其余正常内容一律不糊。
  final List<int> nsfwMediaIds;

  /// 这次工具调用入库的**提示词 id**（`comfy_submit` 的 `capturedPromptId`）。
  ///
  /// 产物只是"结果图"，**生成它的那份工作流同样属于本次的产物**（用户建议 ①）：
  /// 有了它就能在回复末尾直接打开这次真正跑过的工作流 / 把它拖回 ComfyUI 复现。
  final int? promptId;

  /// `ask_user_question` 提的那个问题（用户建议 ⑥）；别的工具为 null。
  ///
  /// 有值时气泡里画的**不是工具卡而是提问卡**（带按钮 / 输入框），
  /// 因为"等你回答"和"等你批准"是两件事，UI 不能混。
  final AiQuestion? question;

  /// 同一张提问卡里的**所有**题（多问改造，2026-09-22）。
  ///
  /// 后端的 `user.question` 是**一题一条事件**、都挂在同一个 `callId` 上：
  /// 只留 [question] 的话第 2 题起会把前一题顶掉（那张卡就只剩最后一题，
  /// 而模型还在等那几题 —— Run 会一直挂到 30 分钟超时）。
  /// [question] 仍是"第一条"：历史回放与旧用例照旧读它。
  final List<AiQuestion> questions;

  /// `layout_propose` 给出的候选布局（2026-11-03）。
  ///
  /// 与 [mediaIds] 同源：只认工具**结构化结果**（`jsonPayload.layoutTemplates`），
  /// 不解析正文。回复末尾因此能画一张"2D 布局模板"卡，而不是让用户去别处找。
  final List<LayoutCandidate> layoutCandidates;

  /// 这张卡要画几题：优先用 [questions]，没有就退回 [question]。
  List<AiQuestion> get allQuestions => questions.isNotEmpty
      ? questions
      : (question == null ? const <AiQuestion>[] : [question!]);

  /// 工具名 → 用户看得懂的"用途"（没有 `reason` 时至少不显示机器名）。
  ///
  /// ⚠ 这张表只是**界面兜底**：模型写了 `reason` 就用它，那张表不参与任何执行 / 安全判定。
  /// 新增工具时在这里补一行即可；漏了会退回工具名（不会崩）。
  String get purposeLabel => switch (name) {
    'comfy_load_workflow' => '读取工作流',
    'comfy_list_workflows' => '查看本机工作流',
    'comfy_find_workflow' => '查找工作流',
    'comfy_submit' => '提交出图',
    'comfy_get_status' => '查看 ComfyUI 状态',
    'comfy_get_run' => '查看这次运行',
    'comfy_sync_history' => '同步生成记录',
    'comfy_use_attachment' => '投放参考图',
    'comfy_use_media' => '投放画廊图片',
    // 2026-11-04：按 Danbooru tag 找真实参考图（选项配图用）
    'reference_image_search' => '查找参考图',
    // 看画廊最近的产物（2026-10-29）：AI 主动"看图"的那一步
    'comfy_view_outputs' => '查看生成产物',
    'comfy_make_mask' => '制作遮罩',
    'comfy_prune_workflow' => '摘改工作流',
    'comfy_audit_workflow' => '审核工作流',
    'comfy_diagnose_workflow' => '检查接线',
    'comfy_check_inputs' => '检查工作流输入',
    'comfy_fit_workflow' => '核对本机模型',
    'comfy_learn_workflow' => '学习工作流',
    'comfy_list_recipes' => '查看工作流配方',
    'comfy_bind_recipe' => '匹配模型配方',
    'comfy_start' => '启动 ComfyUI',
    'comfy_stop' => '停止 ComfyUI',
    'comfy_env_report' => '检查运行环境',
    'read_file' => '读取文件',
    'write_file' => '写入文件',
    'list_dir' => '查看目录',
    'list_skills' => '查看技能',
    'load_skill' => '加载技能',
    'register_skill' => '注册技能',
    'delete_skill' => '删除技能',
    'remember' => '记一条长期记忆',
    'search_prompts' => '搜索提示词',
    'search_character_cards' => '搜索角色卡',
    'save_character_card' => '保存角色卡',
    'danbooru_lookup' => '查 Danbooru 标签',
    'civitai_search' => '搜模型库',
    'find_model' => '查找模型',
    'web_search' => '联网搜索',
    'web_fetch' => '读取网页',
    'ask_user_question' => '询问你',
    'layout_propose' => '生成 2D 布局模板',
    'todo_write' => '更新任务清单',
    _ => name.isEmpty ? '工具调用' : name,
  };

  /// 工具卡的**主标题**：模型写了原因就显示原因（"先确认本机有没有这个模型"），
  /// 否则显示用途（"读取工作流"）。机器名退到副标题。
  String get displayTitle {
    final text = reason?.trim();
    return (text == null || text.isEmpty) ? purposeLabel : text;
  }

  /// 副标题：`用途 · 机器名`；没有 `reason` 时只给机器名（与用途相同的就不重复）。
  String get displaySubtitle {
    final label = purposeLabel;
    if (reason?.trim().isNotEmpty ?? false) {
      return label == name ? name : '$label · $name';
    }
    return label == name ? '' : name;
  }

  /// 把新到的一条提问并进这张卡：同 `questionId` 覆盖，否则追加，按题号排。
  static List<AiQuestion> mergeQuestions(
    List<AiQuestion> prev,
    AiQuestion next,
  ) {
    final out = <AiQuestion>[
      ...prev.where((q) => q.questionId != next.questionId),
      next,
    ]..sort((a, b) => a.index.compareTo(b.index));
    return out;
  }

  const AiToolCallState({
    required this.callId,
    required this.name,
    this.arguments = '',
    this.reason,
    this.status = AiToolCallStatus.running,
    this.preview,
    this.error,
    this.elapsedMs = 0,
    this.approval,
    this.mediaIds = const [],
    this.nsfwMediaIds = const [],
    this.promptId,
    this.question,
    this.questions = const [],
    this.layoutCandidates = const [],
  });

  factory AiToolCallState.fromJson(Map<String, dynamic> json) =>
      AiToolCallState(
        callId: (json['callId'] ?? '').toString(),
        name: (json['name'] ?? '').toString(),
        arguments: (json['arguments'] ?? '').toString(),
        reason: json['reason']?.toString(),
        status: AiToolCallStatus.parse(json['status']?.toString()),
        preview: json['preview']?.toString(),
        error: json['error']?.toString(),
        elapsedMs: (json['elapsedMs'] as num?)?.toInt() ?? 0,
        approval: json['approval']?.toString(),
        mediaIds:
            (json['mediaIds'] as List?)
                ?.whereType<num>()
                .map((e) => e.toInt())
                .toList() ??
            const [],
        nsfwMediaIds:
            (json['nsfwMediaIds'] as List?)
                ?.whereType<num>()
                .map((e) => e.toInt())
                .toList() ??
            const [],
        promptId: (json['promptId'] as num?)?.toInt(),
        question: json['question'] is Map
            ? AiQuestion.fromJson(
                Map<String, dynamic>.from(json['question'] as Map),
              )
            : null,
        questions: _questionsOf(json['questions']),
        layoutCandidates: LayoutCandidate.listFromPayload(json),
      );

  /// 解析一组提问（`questions[]`；没有就是空表）。
  static List<AiQuestion> _questionsOf(Object? raw) {
    if (raw is! List) return const [];
    return raw
        .whereType<Map>()
        .map((e) => AiQuestion.fromJson(Map<String, dynamic>.from(e)))
        .toList();
  }

  /// 从工具结构化结果里取出产物 id（`comfy_submit` 的 `mediaIds`）。
  static List<int> mediaIdsOf(Object? resultJson) {
    if (resultJson is! Map) return const [];
    final raw = resultJson['mediaIds'];
    if (raw is! List) return const [];
    return raw.whereType<num>().map((e) => e.toInt()).toList();
  }

  /// 从工具结构化结果里取出**被标记为成人内容**的产物 id
  /// （`comfy_submit` 的 `nsfwMediaIds`；没声明过就是空表 → 一张都不糊）。
  static List<int> nsfwMediaIdsOf(Object? resultJson) {
    if (resultJson is! Map) return const [];
    final raw = resultJson['nsfwMediaIds'];
    if (raw is! List) return const [];
    return raw.whereType<num>().map((e) => e.toInt()).toList();
  }

  /// 从工具结构化结果里取出**本次入库**的提示词 id（`comfy_submit` 的 `capturedPromptId`）。
  ///
  /// 刻意**不**回落到 `promptId`：那个是"提交时用的库里的提示词"（源工作流），
  /// 而这次真正跑的是**改过参数之后**的节点图；捕获入库的 `capturedPromptId` 才是它。
  static int? promptIdOf(Object? resultJson) {
    if (resultJson is! Map) return null;
    final raw = resultJson['capturedPromptId'];
    return raw is num ? raw.toInt() : null;
  }

  /// 由 `message.parts` 还原：`tool_call` 给名字与参数，`tool_result` 补结果与结论。
  ///
  /// 顺序就是 parts 的顺序（= 模型实际调用的顺序），不能被 map 打乱。
  static List<AiToolCallState> listFromParts(List<AiMessagePart> parts) {
    final out = <AiToolCallState>[];
    final indexOf = <String, int>{};
    for (final part in parts) {
      if (part.type == 'tool_call') {
        final id = part.toolCallId ?? '';
        final map = part.payload ?? const <String, dynamic>{};
        indexOf[id] = out.length;
        out.add(
          AiToolCallState(
            callId: id,
            name: (map['name'] ?? '').toString(),
            arguments: (map['arguments'] ?? '').toString(),
            reason: map['reason']?.toString(),
          ),
        );
      } else if (part.type == 'tool_result') {
        final id = part.toolCallId ?? '';
        final map = part.payload ?? const <String, dynamic>{};
        final ok = map['ok'] == true;
        final approval = (map['approval'] ?? '').toString();
        final i = indexOf[id];
        final prev = i == null ? null : out[i];
        // 提问工具的结果里记着"问了什么、答了什么"：重开 App 之后那张卡要照原样显示
        // （已完成状态），不能变成一张空白卡片（用户建议 ⑥）。
        final asked = map['question'] is Map
            ? AiQuestion.fromJson(
                Map<String, dynamic>.from(map['question'] as Map),
              )
            : null;
        // 整组提问（多问改造）：后端在工具结果里落 `askedAll`；
        // 老结果只有 `asked` 一条 —— 那就退化成单题，界面照样画得出来。
        final askedAll = _questionsOf(map['askedAll']);
        final askedGroup = askedAll.isNotEmpty
            ? askedAll
            : (asked == null
                  ? (prev?.questions ?? const <AiQuestion>[])
                  : AiToolCallState.mergeQuestions(
                      prev?.questions ?? const [],
                      asked,
                    ));
        final next = AiToolCallState(
          callId: id,
          name: (map['name'] ?? prev?.name ?? '').toString(),
          arguments: prev?.arguments ?? '',
          reason: prev?.reason,
          status: ok
              ? AiToolCallStatus.ok
              : (approval == 'denied'
                    ? AiToolCallStatus.denied
                    : AiToolCallStatus.failed),
          // tool_result.text 就是完整输出（后端已截断到 MAX_RESULT_CHARS）
          preview: part.text,
          error: ok
              ? null
              : (map['code']?.toString() ?? map['error']?.toString()),
          elapsedMs: (map['elapsedMs'] as num?)?.toInt() ?? 0,
          approval: approval.isEmpty ? null : approval,
          mediaIds: AiToolCallState.mediaIdsOf(map),
          nsfwMediaIds: AiToolCallState.nsfwMediaIdsOf(map),
          promptId: AiToolCallState.promptIdOf(map),
          question: asked ?? prev?.question,
          questions: askedGroup,
          layoutCandidates: LayoutCandidate.listFromPayload(map),
        );
        if (i == null) {
          indexOf[id] = out.length;
          out.add(next);
        } else {
          out[i] = next;
        }
      }
    }
    return out;
  }

  AiToolCallState copyWith({
    String? name,
    String? arguments,
    String? reason,
    AiToolCallStatus? status,
    String? preview,
    String? error,
    int? elapsedMs,
    String? approval,
    List<int>? mediaIds,
    List<int>? nsfwMediaIds,
    int? promptId,
    AiQuestion? question,
    List<AiQuestion>? questions,
    List<LayoutCandidate>? layoutCandidates,
  }) => AiToolCallState(
    callId: callId,
    name: name ?? this.name,
    arguments: arguments ?? this.arguments,
    reason: reason ?? this.reason,
    status: status ?? this.status,
    preview: preview ?? this.preview,
    error: error ?? this.error,
    elapsedMs: elapsedMs ?? this.elapsedMs,
    approval: approval ?? this.approval,
    mediaIds: mediaIds ?? this.mediaIds,
    nsfwMediaIds: nsfwMediaIds ?? this.nsfwMediaIds,
    promptId: promptId ?? this.promptId,
    question: question ?? this.question,
    questions: questions ?? this.questions,
    layoutCandidates: layoutCandidates ?? this.layoutCandidates,
  );

  /// 待用户批准：气泡上要出「批准 / 拒绝」。
  bool get needsApproval => status == AiToolCallStatus.pendingApproval;

  bool get isRunning => status == AiToolCallStatus.running;

  /// 展开后能看到的详情（没有就不给展开箭头）。
  String get detailText {
    if ((error ?? '').isNotEmpty) return error!;
    if ((preview ?? '').isNotEmpty) return preview!;
    return '';
  }

  /// 工具的**结果正文**（`preview`）。
  ///
  /// 与 [noteText] 分开是用户 2026-10-25 的要求：工具卡里"参数"和"结果"各占一块，
  /// 失败原因作为第三块"说明"—— 不许像以前那样把 `error` 顶到结果的位置上。
  String get resultText => (preview ?? '').trim();

  /// 失败 / 拒绝的**说明**（稳定错误码或消息）；成功时为空。
  String get noteText => (error ?? '').trim();

  bool get hasDetail => detailText.isNotEmpty || arguments.isNotEmpty;

  String get elapsedLabel => elapsedMs <= 0
      ? ''
      : (elapsedMs < 1000
            ? '$elapsedMs ms'
            : '${(elapsedMs / 1000).toStringAsFixed(1)} s');
}

// ---------------------------------------------------------------------------
//  ComfyUI 提交任务与实时进度（用户建议 ① 与"其他建议"第 1 条）
// ---------------------------------------------------------------------------

/// 一次提交给 ComfyUI 的运行（后端 `ComfySubmission`）。
class AiComfySubmission {
  final String promptId;
  final String submittedBy;

  /// 提交时用的标题（能给界面一个好认的名字）。
  final String? title;

  /// queued / running / success / error / empty / timeout
  final String status;
  final String submittedAt;
  final String? finishedAt;
  final int elapsedMs;

  /// 已经入库的产物 id（完成后才有）—— 画廊入口卡就是用它跳过去的。
  final List<int> mediaIds;
  final int? capturedPromptId;
  final String? error;
  final String? message;

  /// 这一次的**实时进度**（后端从 ComfyUI 的 WebSocket 帧里拿的）。
  ///
  /// 只在"我们自己提交、且进度 WS 连着"时才有；拿不到就是 `null`
  /// （= **进度未知**，不是 0%）。别拿它跟 `status` 混：`status` 是"跑到哪一档"，
  /// 这个是"这一档里面跑到哪一步了"。
  final AiComfyProgress? progress;

  const AiComfySubmission({
    required this.promptId,
    this.submittedBy = 'ai',
    this.title,
    this.status = 'queued',
    this.submittedAt = '',
    this.finishedAt,
    this.elapsedMs = 0,
    this.mediaIds = const [],
    this.capturedPromptId,
    this.error,
    this.message,
    this.progress,
  });

  factory AiComfySubmission.fromJson(Map<String, dynamic> json) =>
      AiComfySubmission(
        promptId: (json['promptId'] ?? '').toString(),
        submittedBy: (json['submittedBy'] ?? 'ai').toString(),
        title: json['title']?.toString(),
        status: (json['status'] ?? 'queued').toString(),
        submittedAt: (json['submittedAt'] ?? '').toString(),
        finishedAt: json['finishedAt']?.toString(),
        elapsedMs: (json['elapsedMs'] as num?)?.toInt() ?? 0,
        mediaIds:
            (json['mediaIds'] as List?)
                ?.whereType<num>()
                .map((e) => e.toInt())
                .toList() ??
            const [],
        capturedPromptId: (json['capturedPromptId'] as num?)?.toInt(),
        error: json['error']?.toString(),
        message: json['message']?.toString(),
        progress: json['progress'] is Map
            ? AiComfyProgress.fromJson(
                Map<String, dynamic>.from(json['progress'] as Map),
              )
            : null,
      );

  /// 还在跑（界面据此显示"实时进度"）。
  bool get isActive => status == 'queued' || status == 'running';

  bool get isFailed => status == 'error';

  /// 界面上的状态文案。
  String get statusLabel => switch (status) {
    'queued' => '排队中',
    'running' => '生成中',
    'success' => '已完成',
    'empty' => '已完成（无产物）',
    'error' => '失败',
    'timeout' => '仍在运行',
    _ => status,
  };

  String get elapsedLabel {
    if (elapsedMs <= 0) return '';
    final s = elapsedMs / 1000;
    return s < 60
        ? '${s.toStringAsFixed(0)} 秒'
        : '${(s / 60).toStringAsFixed(1)} 分钟';
  }

  /// 界面上显示的名字（标题 → promptId 前 8 位）。
  String get label => (title == null || title!.trim().isEmpty)
      ? '任务 ${promptId.length > 8 ? promptId.substring(0, 8) : promptId}'
      : title!;
}

/// 一次生成的**实时进度**（后端 `comfy_progress.rs` 维护，来自 ComfyUI 的 WS 帧）。
///
/// 三个"不许骗人"的地方，模型这层就把它表达清楚：
///  · [percent] 为空 = **进度未知**（ComfyUI 没发 `value/max`）—— 界面**不许**画成 0%，
///    也不许自己拿"已执行节点数/总节点数"凑一个；
///  · [nodeName] 为空时只有 [nodeId]：如实说"正在执行节点 9"，而不是编一个名字；
///  · 终态的 [percent] 保持最后一次真实读数（后端刻意不抬到 100），
///    "完成没完成"由 `status` 说，不由进度条说。
class AiComfyProgress {
  /// 这次提交在 ComfyUI 里的 prompt id。
  final String promptId;

  /// `queued` / `running` / `success` / `error` / `cancelled`（后端给的原值）。
  final String state;

  final double? value;
  final double? max;

  /// 0~100（整数，后端算好的高水位）；为空 = 未知。
  final int? percent;

  final String? nodeId;

  /// 当前步骤的可读名（提交时登记过的 API 图里 `_meta.title` → `class_type`）。
  final String? nodeName;

  final String? error;
  final String? updatedAt;

  const AiComfyProgress({
    required this.promptId,
    this.state = 'queued',
    this.value,
    this.max,
    this.percent,
    this.nodeId,
    this.nodeName,
    this.error,
    this.updatedAt,
  });

  factory AiComfyProgress.fromJson(Map<String, dynamic> json) =>
      AiComfyProgress(
        promptId: (json['promptId'] ?? '').toString(),
        state: (json['state'] ?? 'queued').toString(),
        value: (json['value'] as num?)?.toDouble(),
        max: (json['max'] as num?)?.toDouble(),
        percent: (json['percent'] as num?)?.toInt(),
        nodeId: json['nodeId']?.toString(),
        nodeName: json['nodeName']?.toString(),
        error: json['error']?.toString(),
        updatedAt: json['updatedAt']?.toString(),
      );

  bool get isRunning => state == 'running';

  /// **当前步骤**那一句人话（没有名字就只说节点 id；什么都没有就说"进度未知"）。
  ///
  /// `value/max` 有值时附带 `12/30` —— 那是 ComfyUI 自己发的采样步数，
  /// 不是我们数的节点数。
  String get stepLabel {
    final name = (nodeName != null && nodeName!.trim().isNotEmpty)
        ? nodeName!.trim()
        : (nodeId != null && nodeId!.trim().isNotEmpty
              ? '节点 ${nodeId!.trim()}'
              : null);
    final steps = (value != null && max != null && max! > 0)
        ? '${value!.round()}/${max!.round()}'
        : null;
    if (name == null && steps == null) return '进度未知';
    if (name == null) return '步骤 $steps';
    if (steps == null) return '正在执行 $name';
    return '正在执行 $name（$steps）';
  }
}

/// 实时进度快照（后端 `GET /api/capture/jobs`）。
class AiComfyJobs {
  final int queueRunning;
  final int queuePending;
  final bool comfyReachable;

  /// 队列里正在跑的那一个的名字（ComfyUI 里给工作流起的标题）。
  final String? runningLabel;

  /// 最近提交的任务（新的在前）。
  final List<AiComfySubmission> submissions;

  /// 后端现在**正在跑**的那一条（没有就是 null）。
  ///
  /// 与 `submissions[].progress` 是同一份数据源（`comfy_progress` 的 `current`），
  /// 放在顶层只是为了界面不必先找到"哪条在跑"。
  final AiComfyProgress? current;

  const AiComfyJobs({
    this.queueRunning = 0,
    this.queuePending = 0,
    this.comfyReachable = false,
    this.runningLabel,
    this.submissions = const [],
    this.current,
  });

  static const empty = AiComfyJobs();

  factory AiComfyJobs.fromJson(Map<String, dynamic> json) => AiComfyJobs(
    queueRunning: (json['queueRunning'] as num?)?.toInt() ?? 0,
    queuePending: (json['queuePending'] as num?)?.toInt() ?? 0,
    comfyReachable: json['comfyReachable'] == true,
    runningLabel: json['runningLabel']?.toString(),
    submissions:
        (json['submissions'] as List?)
            ?.whereType<Map>()
            .map(
              (e) => AiComfySubmission.fromJson(Map<String, dynamic>.from(e)),
            )
            .toList() ??
        const [],
    current: json['current'] is Map
        ? AiComfyProgress.fromJson(
            Map<String, dynamic>.from(json['current'] as Map),
          )
        : null,
  );

  /// 有没有"正在跑"的东西 —— 界面据此决定要不要显示进度块、要不要继续轮询。
  bool get hasActivity =>
      queueRunning > 0 ||
      queuePending > 0 ||
      submissions.any((s) => s.isActive);
}

// ---------------------------------------------------------------------------
//  模型能力目录的在线更新（2026-09-23"全做了"）
//
//  链路：custom.json（手工）> remote.json（models.dev 拉取）> 内置表，逐维度合并。
//  状态 / 手动刷新 / 实测校准开关与触发都从后端读 —— 前端不存一份自己的，
//  否则就会出现"界面说更新了、后端其实没刷新"（与 NSFW 开关同一条纪律）。
// ---------------------------------------------------------------------------

/// `GET /api/ai/capabilities/status`（也用于刷新 / 开关的回读）。
class AiCapabilityStatus {
  final String version;
  final int builtinRules;
  final String? customSource;
  final String? customFetchedAt;
  final int? customEntries;
  final String? remoteSource;
  final String? remoteFetchedAt;
  final int? remoteEntries;

  /// 在线目录的年龄（小时）；null = 没有 remote.json
  final int? remoteAgeHours;
  final bool remoteStale;
  final int refreshIntervalDays;

  /// 是否允许实测校准（发真实请求；默认关）
  final bool probeEnabled;

  /// 最近一次（手动）操作的人话结果
  final String? lastMessage;

  /// 拉状态失败时的原因（界面如实显示，不静默）
  final String? error;

  const AiCapabilityStatus({
    this.version = '',
    this.builtinRules = 0,
    this.customSource,
    this.customFetchedAt,
    this.customEntries,
    this.remoteSource,
    this.remoteFetchedAt,
    this.remoteEntries,
    this.remoteAgeHours,
    this.remoteStale = false,
    this.refreshIntervalDays = 7,
    this.probeEnabled = false,
    this.lastMessage,
    this.error,
  });

  factory AiCapabilityStatus.fromJson(Map<String, dynamic> json) =>
      AiCapabilityStatus(
        version: json['version'] as String? ?? '',
        builtinRules: (json['builtinRules'] as num?)?.toInt() ?? 0,
        customSource: json['customSource'] as String?,
        customFetchedAt: json['customFetchedAt'] as String?,
        customEntries: (json['customEntries'] as num?)?.toInt(),
        remoteSource: json['remoteSource'] as String?,
        remoteFetchedAt: json['remoteFetchedAt'] as String?,
        remoteEntries: (json['remoteEntries'] as num?)?.toInt(),
        remoteAgeHours: (json['remoteAgeHours'] as num?)?.toInt(),
        remoteStale: json['remoteStale'] == true,
        refreshIntervalDays:
            (json['refreshIntervalDays'] as num?)?.toInt() ?? 7,
        probeEnabled: json['probeEnabled'] == true,
        lastMessage: json['lastMessage'] as String?,
      );

  /// 在线目录的一句话描述（界面直接显示）。
  String get remoteLabel {
    if (remoteSource == null) return '还没有在线目录';
    final h = remoteAgeHours;
    final age = h == null ? '' : (h < 24 ? '$h小时前' : '${(h / 24).round()}天前');
    return '$remoteSource · $remoteEntries 条 · $age';
  }
}

/// 单个模型的校准结果（`POST /api/ai/capabilities/probe`）。
class AiCapabilityProbeOutcome {
  final String providerId;
  final String modelId;
  final bool ok;
  final String? acceptedTop;
  final Map<String, String>? ladder;
  final bool written;
  final bool skipped;
  final String message;

  const AiCapabilityProbeOutcome({
    required this.providerId,
    required this.modelId,
    required this.ok,
    this.acceptedTop,
    this.ladder,
    this.written = false,
    this.skipped = false,
    required this.message,
  });

  factory AiCapabilityProbeOutcome.fromJson(Map<String, dynamic> json) =>
      AiCapabilityProbeOutcome(
        providerId: json['providerId'] as String? ?? '',
        modelId: json['modelId'] as String? ?? '',
        ok: json['ok'] == true,
        acceptedTop: json['acceptedTop'] as String?,
        ladder: (json['ladder'] as Map?)?.map(
          (k, v) => MapEntry(k.toString(), v.toString()),
        ),
        written: json['written'] == true,
        skipped: json['skipped'] == true,
        message: json['message'] as String? ?? '',
      );
}

class AiCapabilityProbeResponse {
  final bool ok;
  final String message;
  final List<AiCapabilityProbeOutcome> outcomes;

  const AiCapabilityProbeResponse({
    required this.ok,
    required this.message,
    this.outcomes = const [],
  });

  factory AiCapabilityProbeResponse.fromJson(Map<String, dynamic> json) =>
      AiCapabilityProbeResponse(
        ok: json['ok'] == true,
        message: json['message'] as String? ?? '',
        outcomes:
            (json['outcomes'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) => AiCapabilityProbeOutcome.fromJson(
                    Map<String, dynamic>.from(e),
                  ),
                )
                .toList() ??
            const [],
      );
}

// ---------------------------------------------------------------------------
//  跨端待办总览（`GET /api/ai/pending`，M1 定的契约；M5 起 approvals 里并进
//  手机排队的提交）
//
//  它是给"另一台设备刚连上来"用的：手机、第二个窗口、刚重启完的桌面 App ——
//  它们**没有经历**那条 SSE 事件流，所以"正在等人"的卡片（AI 的提问、需要批准的
//  调用）靠这一个接口一次补齐，而不是逐个 Run 去问。
//
//  ⚠ 键名逐字对齐 `server-rs/src/pending.rs`（那里少一个键，界面上就少一块内容）：
//    * `approvals[].askedAt` 是 **ISO 文本**；
//    * `asks[].askedAt` 是 **epoch 毫秒**（数字）。
//  两个 `askedAt` 类型不同，别想着"统一一下"。
// ---------------------------------------------------------------------------

/// `GET /api/ai/pending` 的响应体：`{ asks: [...], approvals: [...] }`。
class AiPendingOverview {
  /// AI 还在等的提问（Rust 侧直接读 `ai_questions` 里 `status='pending'` 的行）。
  final List<AiPendingAsk> asks;

  /// 还在等批准的调用 —— **两类合成一个列表**：[AiPendingApproval] 那一份注释写了为什么。
  final List<AiPendingApproval> approvals;

  const AiPendingOverview({this.asks = const [], this.approvals = const []});

  static const empty = AiPendingOverview();

  factory AiPendingOverview.fromJson(Map<String, dynamic> json) =>
      AiPendingOverview(
        asks:
            (json['asks'] as List?)
                ?.whereType<Map>()
                .map((e) => AiPendingAsk.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
        approvals:
            (json['approvals'] as List?)
                ?.whereType<Map>()
                .map(
                  (e) =>
                      AiPendingApproval.fromJson(Map<String, dynamic>.from(e)),
                )
                .toList() ??
            const [],
      );

  /// 一件待办都没有（界面上"整块不占位"用得上）。
  bool get isEmpty => asks.isEmpty && approvals.isEmpty;

  /// 摘掉一条已被后端接受的审批（返回新对象，不改原对象）。
  ///
  /// 点了「批准 / 拒绝」且 `accepted=true` 之后就地摘掉它：后端那一刻**确实**
  /// 把它移出了待办（`submit_jobs` / `ai_tool_calls` 的 `approval` 当场置位），
  /// 不该等"下一次轮询刚好拉回来"才从界面上消失。
  AiPendingOverview withoutApproval(String callId) => AiPendingOverview(
    asks: asks,
    approvals: approvals.where((a) => a.callId != callId).toList(),
  );
}

/// `approvals[]` 的一条 —— 正在等人批准的调用。
///
/// **两类东西共用这一个形状**（`pending.rs` 的文件头）：
///   1. 桌面 Run 的工具调用（`ai_tool_calls.status='pending'`）；
///   2. **手机排队的提交**（M5，契约 §4.1）：`callId` 形如 `submit:<queueId>`，
///      后端把 `name` 写成它真正要干的事（`comfy_submit`）。
///
/// 为什么并成一个列表：**审批在桌面上必须看得见**，而复用的是同一条通道
/// （同一个 `POST /api/ai/tool-calls/{callId}/approve|deny`）——
/// 客户端因此不必分两路处理。
class AiPendingApproval {
  final String callId;
  final String runId;
  final String conversationId;

  /// 工具名。手机排队的提交这里是 `comfy_submit`。
  final String name;

  /// 参数原文（**JSON 文本**，不是对象）。界面只把它当一行字显示，不在这里解析。
  final String argumentsJson;

  /// **ISO 文本**（`rs.isoTime` 的口径）；后端拿不到时间时是 null。
  ///
  /// ⚠ 与 [AiPendingAsk.askedAt] 的类型不同，别互换。
  final String? askedAt;

  const AiPendingApproval({
    required this.callId,
    this.runId = '',
    this.conversationId = '',
    this.name = '',
    this.argumentsJson = '',
    this.askedAt,
  });

  factory AiPendingApproval.fromJson(Map<String, dynamic> json) =>
      AiPendingApproval(
        callId: (json['callId'] ?? '').toString(),
        runId: (json['runId'] ?? '').toString(),
        conversationId: (json['conversationId'] ?? '').toString(),
        name: (json['name'] ?? '').toString(),
        argumentsJson: (json['argumentsJson'] ?? '').toString(),
        askedAt: json['askedAt']?.toString(),
      );

  /// 这条是不是**手机排上来的提交**（桌面 Run 的工具调用是 `call_00_…`）。
  ///
  /// 判据就是后端的 `call_id_for()`：`submit:<queueId>`（`submit_queue.rs`）。
  /// 界面靠它说人话（"手机排队的提交"）并提醒"手机点的那一下只是排队"。
  bool get isQueuedSubmit => callId.startsWith('submit:');
}

/// `asks[]` 的一条（提问）。
///
/// 形状与 `questions.rs::row_to_json` 逐字一致 —— 它与界面上的 [AiQuestion] 是
/// **同一份 wire 形状的两个用处**：`AiQuestion` 是"气泡里那张正在被 SSE 打的卡"
/// （会被 `copyWith` 就地改状态），而这一份是 `GET /api/ai/pending` 的**只读快照**
/// （"另一台设备刚连上来"的补卡场景，还多带一个 `askedAt`）。
class AiPendingAsk {
  final String questionId;
  final String runId;
  final String conversationId;
  final String question;
  final List<String> options;
  final bool allowFreeText;
  final bool answered;

  /// **epoch 毫秒**（数字）。别当成 ISO 文本 —— 审批那一份才是 ISO。
  final int askedAt;

  final int index;
  final int total;
  final String groupId;
  final bool multiSelect;

  /// 下面这几个是**可空键**：后端没有就整个键不出现（`explicitNulls = false`）。
  final String? header;
  final String? answer;
  final int? optionIndex;
  final List<int> optionIndexes;

  const AiPendingAsk({
    required this.questionId,
    this.runId = '',
    this.conversationId = '',
    this.question = '',
    this.options = const [],
    this.allowFreeText = true,
    this.answered = false,
    this.askedAt = 0,
    this.index = 0,
    this.total = 1,
    this.groupId = '',
    this.multiSelect = false,
    this.header,
    this.answer,
    this.optionIndex,
    this.optionIndexes = const [],
  });

  factory AiPendingAsk.fromJson(Map<String, dynamic> json) => AiPendingAsk(
    questionId: (json['questionId'] ?? '').toString(),
    runId: (json['runId'] ?? '').toString(),
    conversationId: (json['conversationId'] ?? '').toString(),
    question: (json['question'] ?? '').toString(),
    options:
        (json['options'] as List?)?.map((e) => e.toString()).toList() ??
        const [],
    allowFreeText: json['allowFreeText'] != false,
    answered: json['answered'] == true,
    askedAt: (json['askedAt'] as num?)?.toInt() ?? 0,
    index: (json['index'] as num?)?.toInt() ?? 0,
    total: (json['total'] as num?)?.toInt() ?? 1,
    groupId: (json['groupId'] ?? '').toString(),
    multiSelect: json['multiSelect'] == true,
    header: json['header']?.toString(),
    answer: json['answer']?.toString(),
    optionIndex: (json['optionIndex'] as num?)?.toInt(),
    optionIndexes:
        (json['optionIndexes'] as List?)
            ?.whereType<num>()
            .map((e) => e.toInt())
            .toList() ??
        const [],
  );
}
