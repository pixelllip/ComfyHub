/// AI 工作台的前端模型（AIH-006 / AIH-010 / AIH-012 / AIH-018 / AIH-019）。
///
/// 注意：这里**没有**任何保存 API Key 的字段 —— 密钥只写不读（AIH-012），
/// 前端能拿到的只有 [AiCredentialStatus]。
library;

/// 凭据状态：只有"有没有 / 从哪来 / 能不能改"，没有值。
class AiCredentialStatus {
  final bool configured;
  final String source; // env / managed / none
  final bool writable;

  const AiCredentialStatus({
    required this.configured,
    required this.source,
    required this.writable,
  });

  static const none = AiCredentialStatus(configured: false, source: 'none', writable: false);

  factory AiCredentialStatus.fromJson(Map<String, dynamic> json) => AiCredentialStatus(
        configured: json['configured'] == true,
        source: (json['source'] ?? 'none').toString(),
        writable: json['writable'] == true,
      );

  String get label => switch (source) {
        'env' => configured ? '已配置（环境变量，只读）' : '未配置（环境变量）',
        'managed' => '已配置',
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
/// 等级表与 pi-ai / DSH 对齐：`关闭 → 极低 → 低 → 中 → 高 → 极高 → 最大`。
/// 前沿模型常常只声明到"极高"（没有 `max`），少了这一档就只能把用户的选择压回"高"。
enum AiReasoningEffort {
  off('off', '关闭'),
  minimal('minimal', '极低'),
  low('low', '低'),
  medium('medium', '中'),
  high('high', '高'),
  xhigh('xhigh', '极高'),
  max('max', '最大');

  const AiReasoningEffort(this.wire, this.label);
  final String wire;
  final String label;

  static AiReasoningEffort? parse(String? wire) {
    for (final e in values) {
      if (e.wire == wire) return e;
    }
    return null;
  }

  /// 关闭以外的等级（模型可选的"真的在思考"的档位）
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

  static AiEndpointTrust parse(String? wire) =>
      values.firstWhere((t) => t.wire == wire, orElse: () => AiEndpointTrust.public_);
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
            ? AiCredentialStatus.fromJson(Map<String, dynamic>.from(json['credential'] as Map))
            : AiCredentialStatus.none,
      );

  AiApi? get apiEnum => AiApi.parse(api);
  String get apiLabel => apiEnum?.label ?? api;
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
            (json['inputModalities'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        attachmentTransports: (json['attachmentTransports'] as Map?)?.map(
              (k, v) => MapEntry(
                k.toString(),
                (v as List?)?.map((e) => e.toString()).toList() ?? const <String>[],
              ),
            ) ??
            const {},
        mimeAllowlist: (json['mimeAllowlist'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        tools: json['tools'] == true,
        parallelTools: json['parallelTools'] == true,
        reasoning: json['reasoning'] == true,
        thinkingEfforts: (json['thinkingEfforts'] as Map?)?.map(
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

  /// 模型选择器上的能力徽标（AIH-048）：只显示能力目录里的声明，不猜测。
  List<AiModality> get modalities =>
      inputModalities.map(AiModality.parse).whereType<AiModality>().toList();

  bool supports(AiModality m) => inputModalities.contains(m.wire);

  /// 该模型能不能调思考强度：必须声明推理能力，且**至少声明了一个具体档位**。
  bool get supportsReasoningEffort =>
      reasoning && thinkingEfforts.keys.any((k) => k != AiReasoningEffort.off.wire);

  /// 可选的思考档位（固定顺序：关闭 → 极低 → 低 → 中 → 高 → 极高 → 最大）。
  ///
  /// **「关闭」永远可选**（用户要求："允许我关闭模型思考，不影响模型声明"）：
  /// 它不发任何思考参数、任何网关都成立，所以不要求模型在 `thinkingEfforts` 里声明 `off`
  /// —— 否则"只想安静地问一句"的用户得先去改模型声明，这是本末倒置。
  /// 真正的思考档位仍然**只列模型声明过的**，不猜、不降级（AIH-056 第 2 条）。
  List<AiReasoningEffort> get selectableEfforts {
    if (!reasoning) return const [];
    final declared = thinkingEfforts.keys.toSet();
    return [
      AiReasoningEffort.off,
      ...AiReasoningEffort.values.where((e) => e.isThinking && declared.contains(e.wire)),
    ];
  }

  /// 该等级的线上表达（网关改名时显示给用户看）。数字表示 Anthropic 的思考预算。
  String? effortWireValue(AiReasoningEffort effort) => thinkingEfforts[effort.wire];

  AiThinkingFormat? get thinkingFormatEnum => AiThinkingFormat.parse(thinkingFormat);

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

  factory AiProviderTestResult.fromJson(Map<String, dynamic> json) => AiProviderTestResult(
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

  factory AiModelCandidate.fromJson(Map<String, dynamic> json) => AiModelCandidate(
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
        thinkingEfforts: (json['thinkingEfforts'] as Map?)?.map(
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

  AiModelCandidate copyWith({List<AiModality>? modalities, bool? tools}) => AiModelCandidate(
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

  factory AiDiscoverResult.fromJson(Map<String, dynamic> json) => AiDiscoverResult(
        ok: json['ok'] == true,
        errorCode: json['errorCode']?.toString(),
        message: (json['message'] ?? '').toString(),
        candidates: (json['candidates'] as List?)
                ?.whereType<Map>()
                .map((e) => AiModelCandidate.fromJson(Map<String, dynamic>.from(e)))
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
      type == 'run.completed' || type == 'run.failed' || type == 'run.cancelled';
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
        updatedAt: DateTime.tryParse((json['updatedAt'] ?? '').toString()),
      );

  AiConversation copyWith({String? title, int? messageCount}) => AiConversation(
        id: id,
        title: title ?? this.title,
        providerId: providerId,
        modelId: modelId,
        archived: archived,
        messageCount: messageCount ?? this.messageCount,
        updatedAt: updatedAt,
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
    if (input == 0 && output == 0) input = pick(['total_tokens', 'totalTokens']);
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

  bool get isEmpty => totalTokens == 0 && cachedTokens == 0 && reasoningTokens == 0;

  /// `↑1.2k ↓340` —— 输入/输出，聊天里最常看的一对。
  String get shortLabel => '↑${compact(inputTokens)} ↓${compact(outputTokens)}';

  String get detailLabel {
    final parts = <String>['输入 $inputTokens', '输出 $outputTokens'];
    if (cachedTokens > 0) parts.add('缓存命中 $cachedTokens');
    if (reasoningTokens > 0) parts.add('思考 $reasoningTokens');
    parts.add('合计 $totalTokens');
    return parts.join(' · ');
  }

  /// 大数字压缩成 `1.2k` / `15k`，聊天里不占地方。
  static String compact(int n) =>
      n < 1000 ? '$n' : '${(n / 1000).toStringAsFixed(n < 10000 ? 1 : 0)}k';
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
    final buf = StringBuffer('$requests 次请求 · ↑${AiTokenUsage.compact(inputTokens)} '
        '↓${AiTokenUsage.compact(outputTokens)} · 合计 ${AiTokenUsage.compact(totalTokens)}');
    if (cachedTokens > 0) buf.write(' · 缓存命中 ${AiTokenUsage.compact(cachedTokens)}');
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
        payload: json['jsonPayload'] is Map
            ? Map<String, dynamic>.from(json['jsonPayload'] as Map)
            : null,
      );

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

/// 一条助手消息里**按流顺序**排好的一段内容：思考段或正文段。
///
/// 为什么需要它：工具循环会让一次回复变成
/// `思考 → 正文 → 工具 → 思考 → 正文 …`，而人想看到的就是这个顺序。
/// 后端给的 `parts` 已经是权威的有序块，这里只是把**相邻的同类块合并**成段，
/// 免得一个气泡里冒出十几个碎段。
class AiMessageSegment {
  /// reasoning / text
  final String type;
  final String text;

  const AiMessageSegment(this.type, this.text);

  bool get isReasoning => type == 'reasoning';

  /// 从有序块还原成段（相邻同类合并）。没有 parts 时返回空列表，调用方回退到实时状态。
  static List<AiMessageSegment> fromParts(List<AiMessagePart> parts) {
    final out = <AiMessageSegment>[];
    for (final p in parts) {
      if (p.type != 'reasoning' && p.type != 'text') continue;
      final text = p.text ?? '';
      if (text.isEmpty) continue;
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
        parts: (json['parts'] as List?)
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
  }) =>
      AiMessage(
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

// ---------------------------------------------------------------------------
//  内置模型目录（M6：冻结副本 + 对齐）
// ---------------------------------------------------------------------------

/// 内置模型目录的状态 / 同步结果（`GET /api/ai/builtin/status`、
/// `POST /api/ai/builtin/sync`）。
///
/// 内置目录是 `%USERPROFILE%\.dsh\settings.yaml` 的**冻结副本**
/// （`resources/ai/builtin-catalog.json`）：运行时绝不读 YAML，
/// 只按用户点的那一下把差异对齐进库。
class AiBuiltinCatalogStatus {
  /// add-missing / refresh-capabilities
  final String mode;
  final String version;

  /// 内置目录对应的 Provider id。
  final String? providerId;

  /// 这次会补 / 已补的模型数。
  final int added;

  /// 这次对齐 / 已对齐的能力条数。
  final int updated;

  /// 库里已有、且与内置目录一致（不用动）的条数。
  final int kept;

  /// 库里已有、但能力声明与内置目录**不一致**的 model_id。
  final List<String> divergent;

  /// 内置目录里一共有多少个模型。
  final int modelCount;
  final bool providerCreated;

  /// 非空就是出错（预览也可能因为目录缺失而报错）。
  final String? error;

  const AiBuiltinCatalogStatus({
    this.mode = 'add-missing',
    this.version = '',
    this.providerId,
    this.added = 0,
    this.updated = 0,
    this.kept = 0,
    this.divergent = const [],
    this.modelCount = 0,
    this.providerCreated = false,
    this.error,
  });

  factory AiBuiltinCatalogStatus.fromJson(Map<String, dynamic> json) => AiBuiltinCatalogStatus(
        mode: (json['mode'] ?? 'add-missing').toString(),
        version: (json['version'] ?? '').toString(),
        providerId: json['providerId']?.toString(),
        added: (json['added'] as num?)?.toInt() ?? 0,
        updated: (json['updated'] as num?)?.toInt() ?? 0,
        kept: (json['kept'] as num?)?.toInt() ?? 0,
        divergent:
            (json['divergent'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        modelCount: (json['modelCount'] as num?)?.toInt() ?? 0,
        providerCreated: json['providerCreated'] == true,
        error: json['error']?.toString(),
      );

  bool get ok => (error ?? '').isEmpty;

  /// 分歧条数（也就是"对齐"这次会改多少条）。
  int get divergentCount => divergent.length;

  bool get hasDivergence => divergent.isNotEmpty;

  /// 醒目提示的原文。
  String get divergenceLabel => '有 $divergentCount 个模型的能力声明与内置目录不同';
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
    if (sizeBytes < 1024 * 1024) return '${(sizeBytes / 1024).toStringAsFixed(0)} KB';
    return '${(sizeBytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }
}

/// 一次附件上传的结果：成功的 + 逐个失败的原因（不静默丢弃）。
class AiAttachmentUploadResult {
  final List<AiAttachment> items;
  final List<({String fileName, String reason})> failed;

  const AiAttachmentUploadResult({this.items = const [], this.failed = const []});

  factory AiAttachmentUploadResult.fromJson(Map<String, dynamic> json) => AiAttachmentUploadResult(
        items: (json['items'] as List?)
                ?.whereType<Map>()
                .map((e) => AiAttachment.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
        failed: (json['failed'] as List?)
                ?.whereType<Map>()
                .map((e) => (
                      fileName: (e['fileName'] ?? '附件').toString(),
                      reason: (e['reason'] ?? '上传失败').toString(),
                    ))
                .toList() ??
            const [],
      );

  String get failedLabel =>
      failed.map((f) => '${f.fileName}：${f.reason}').join('；');
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

  factory AiPreflightItem.fromJson(Map<String, dynamic> json) => AiPreflightItem(
        index: (json['index'] as num?)?.toInt() ?? 0,
        name: (json['name'] ?? '附件').toString(),
        allowed: json['allowed'] == true,
        blockers: (json['blockers'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        attachmentId: json['attachmentId']?.toString(),
      );
}

/// 预检结果：`allowed=false` 时必须阻断发送，不能"先发再说"（AIH-030）。
class AiPreflightResult {
  final bool allowed;
  final List<String> blockers;

  /// 逐个附件的结论（顺序与请求一致）：托盘里给被拦下的那张打标记
  final List<AiPreflightItem> items;

  const AiPreflightResult({required this.allowed, required this.blockers, this.items = const []});

  static const allow = AiPreflightResult(allowed: true, blockers: []);

  factory AiPreflightResult.fromJson(Map<String, dynamic> json) => AiPreflightResult(
        allowed: json['allowed'] == true,
        blockers: (json['blockers'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        items: (json['items'] as List?)
                ?.whereType<Map>()
                .map((e) => AiPreflightItem.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
      );

  /// 第 [index] 个附件被拦下的原因（空列表 = 没问题）。
  List<String> blockersAt(int index) =>
      items.where((it) => it.index == index).expand((it) => it.blockers).toList();
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
  bool get hasWarning => (validationError ?? '').isNotEmpty || (conflict ?? '').isNotEmpty;

  /// 悬浮提示上的完整说明：问题优先，其次冲突，最后来源。
  String get warningText {
    if ((validationError ?? '').isNotEmpty) return '不合法：$validationError';
    if ((conflict ?? '').isNotEmpty) return conflict!;
    return origin == null ? sourceLabel : '$sourceLabel · $origin';
  }

  String get sizeLabel {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) return '${(sizeBytes / 1024).toStringAsFixed(1)} KB';
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

  factory AiSkillRescanResult.fromJson(Map<String, dynamic> json) => AiSkillRescanResult(
        registered: (json['registered'] as num?)?.toInt() ?? 0,
        names: (json['names'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        errors: (json['errors'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        skills: (json['skills'] as List?)
                ?.whereType<Map>()
                .map((e) => AiSkill.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
      );

  /// 要给用户看的话：**失败条目也要说**，不能只报"成功 N 个"。
  String get summary {
    final parts = <String>['扫描到 ${skills.length} 个 skill'];
    if (registered > 0) parts.add('自动登记 $registered 个（${names.take(3).join('、')}）');
    if (errors.isNotEmpty) parts.add('${errors.length} 个没法用：${errors.take(2).join('；')}');
    return parts.join(' · ');
  }
}

/// 长期记忆（M6）：真源是 `<storage>\ai\memory.md`。
class AiMemory {
  final String content;
  final String path;
  final int entryCount;
  final int maxChars;

  const AiMemory({
    this.content = '',
    this.path = '',
    this.entryCount = 0,
    this.maxChars = 8000,
  });

  factory AiMemory.fromJson(Map<String, dynamic> json) => AiMemory(
        content: (json['content'] ?? '').toString(),
        path: (json['path'] ?? '').toString(),
        entryCount: (json['entryCount'] as num?)?.toInt() ?? 0,
        maxChars: (json['maxChars'] as num?)?.toInt() ?? 8000,
      );

  /// 面板上的一行预览（第一条记忆）。
  String get preview {
    final first = content
        .split('\n')
        .map((l) => l.trim())
        .firstWhere((l) => l.isNotEmpty, orElse: () => '');
    return first.replaceFirst(RegExp(r'^[-*]\s*'), '');
  }
}

// ---------------------------------------------------------------------------
//  工具与权限（M4 / AIH-031 ~ AIH-036 / AIH-049）
// ---------------------------------------------------------------------------

/// 一个工具的生效权限（`GET /api/ai/tools`）。
class AiToolInfo {
  final String name;
  final String description;

  /// skill / files / comfy —— 界面按它选图标。
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
    this.maxToolSteps = 8,
    this.maxCallsPerRun = 16,
    this.maxReadBytes = 0,
    this.maxWriteBytes = 0,
    this.defaultWriteRoot,
    this.permissionMode = modeAsk,
  });

  factory AiToolPolicy.fromJson(Map<String, dynamic> json) => AiToolPolicy(
        writeRoots: (json['writeRoots'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        readRoots: (json['readRoots'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        autoReadRoots:
            (json['autoReadRoots'] as List?)?.map((e) => e.toString()).toList() ?? const [],
        overrides: (json['overrides'] as Map?)?.map(
              (k, v) => MapEntry(k.toString(), v.toString()),
            ) ??
            const {},
        maxToolSteps: (json['maxToolSteps'] as num?)?.toInt() ?? 8,
        maxCallsPerRun: (json['maxCallsPerRun'] as num?)?.toInt() ?? 16,
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
  }) =>
      AiToolPolicy(
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
  String get defaultWriteHint => defaultWriteRoot == null || defaultWriteRoot!.isEmpty
      ? '默认只能写后端预设的产物目录，其它位置一律拒绝。'
      : '默认只能写 $defaultWriteRoot，其它位置一律拒绝。';
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
  final AiToolCallStatus status;

  /// 结果预览（后端已按 8KB 截断）。
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

  /// 这次工具调用入库的**提示词 id**（`comfy_submit` 的 `capturedPromptId`）。
  ///
  /// 产物只是"结果图"，**生成它的那份工作流同样属于本次的产物**（用户建议 ①）：
  /// 有了它就能在回复末尾直接打开这次真正跑过的工作流 / 把它拖回 ComfyUI 复现。
  final int? promptId;

  const AiToolCallState({
    required this.callId,
    required this.name,
    this.arguments = '',
    this.status = AiToolCallStatus.running,
    this.preview,
    this.error,
    this.elapsedMs = 0,
    this.approval,
    this.mediaIds = const [],
    this.promptId,
  });

  factory AiToolCallState.fromJson(Map<String, dynamic> json) => AiToolCallState(
        callId: (json['callId'] ?? '').toString(),
        name: (json['name'] ?? '').toString(),
        arguments: (json['arguments'] ?? '').toString(),
        status: AiToolCallStatus.parse(json['status']?.toString()),
        preview: json['preview']?.toString(),
        error: json['error']?.toString(),
        elapsedMs: (json['elapsedMs'] as num?)?.toInt() ?? 0,
        approval: json['approval']?.toString(),
        mediaIds: (json['mediaIds'] as List?)?.whereType<num>().map((e) => e.toInt()).toList() ?? const [],
        promptId: (json['promptId'] as num?)?.toInt(),
      );

  /// 从工具结构化结果里取出产物 id（`comfy_submit` 的 `mediaIds`）。
  static List<int> mediaIdsOf(Object? resultJson) {
    if (resultJson is! Map) return const [];
    final raw = resultJson['mediaIds'];
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
        out.add(AiToolCallState(
          callId: id,
          name: (map['name'] ?? '').toString(),
          arguments: (map['arguments'] ?? '').toString(),
        ));
      } else if (part.type == 'tool_result') {
        final id = part.toolCallId ?? '';
        final map = part.payload ?? const <String, dynamic>{};
        final ok = map['ok'] == true;
        final approval = (map['approval'] ?? '').toString();
        final i = indexOf[id];
        final prev = i == null ? null : out[i];
        final next = AiToolCallState(
          callId: id,
          name: (map['name'] ?? prev?.name ?? '').toString(),
          arguments: prev?.arguments ?? '',
          status: ok
              ? AiToolCallStatus.ok
              : (approval == 'denied' ? AiToolCallStatus.denied : AiToolCallStatus.failed),
          // tool_result.text 就是完整输出（后端已截断到 8KB）
          preview: part.text,
          error: ok ? null : (map['code']?.toString() ?? map['error']?.toString()),
          elapsedMs: (map['elapsedMs'] as num?)?.toInt() ?? 0,
          approval: approval.isEmpty ? null : approval,
          mediaIds: AiToolCallState.mediaIdsOf(map),
          promptId: AiToolCallState.promptIdOf(map),
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
    AiToolCallStatus? status,
    String? preview,
    String? error,
    int? elapsedMs,
    String? approval,
    List<int>? mediaIds,
    int? promptId,
  }) =>
      AiToolCallState(
        callId: callId,
        name: name ?? this.name,
        arguments: arguments ?? this.arguments,
        status: status ?? this.status,
        preview: preview ?? this.preview,
        error: error ?? this.error,
        elapsedMs: elapsedMs ?? this.elapsedMs,
        approval: approval ?? this.approval,
        mediaIds: mediaIds ?? this.mediaIds,
        promptId: promptId ?? this.promptId,
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

  bool get hasDetail => detailText.isNotEmpty || arguments.isNotEmpty;

  String get elapsedLabel => elapsedMs <= 0
      ? ''
      : (elapsedMs < 1000 ? '$elapsedMs ms' : '${(elapsedMs / 1000).toStringAsFixed(1)} s');
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
  });

  factory AiComfySubmission.fromJson(Map<String, dynamic> json) => AiComfySubmission(
        promptId: (json['promptId'] ?? '').toString(),
        submittedBy: (json['submittedBy'] ?? 'ai').toString(),
        title: json['title']?.toString(),
        status: (json['status'] ?? 'queued').toString(),
        submittedAt: (json['submittedAt'] ?? '').toString(),
        finishedAt: json['finishedAt']?.toString(),
        elapsedMs: (json['elapsedMs'] as num?)?.toInt() ?? 0,
        mediaIds: (json['mediaIds'] as List?)?.whereType<num>().map((e) => e.toInt()).toList() ?? const [],
        capturedPromptId: (json['capturedPromptId'] as num?)?.toInt(),
        error: json['error']?.toString(),
        message: json['message']?.toString(),
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
    return s < 60 ? '${s.toStringAsFixed(0)} 秒' : '${(s / 60).toStringAsFixed(1)} 分钟';
  }

  /// 界面上显示的名字（标题 → promptId 前 8 位）。
  String get label => (title == null || title!.trim().isEmpty)
      ? '任务 ${promptId.length > 8 ? promptId.substring(0, 8) : promptId}'
      : title!;
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

  const AiComfyJobs({
    this.queueRunning = 0,
    this.queuePending = 0,
    this.comfyReachable = false,
    this.runningLabel,
    this.submissions = const [],
  });

  static const empty = AiComfyJobs();

  factory AiComfyJobs.fromJson(Map<String, dynamic> json) => AiComfyJobs(
        queueRunning: (json['queueRunning'] as num?)?.toInt() ?? 0,
        queuePending: (json['queuePending'] as num?)?.toInt() ?? 0,
        comfyReachable: json['comfyReachable'] == true,
        runningLabel: json['runningLabel']?.toString(),
        submissions: (json['submissions'] as List?)
                ?.whereType<Map>()
                .map((e) => AiComfySubmission.fromJson(Map<String, dynamic>.from(e)))
                .toList() ??
            const [],
      );

  /// 有没有"正在跑"的东西 —— 界面据此决定要不要显示进度块、要不要继续轮询。
  bool get hasActivity => queueRunning > 0 || queuePending > 0 || submissions.any((s) => s.isActive);
}
