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

  const AiMessage({
    required this.id,
    required this.conversationId,
    required this.seq,
    required this.role,
    required this.status,
    required this.text,
    this.modelId,
    this.parts = const [],
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
      );

  bool get isUser => role == 'user';
}

/// 附件（AIH-029）：准入结论由后端 preflight 给出，前端不自行放行。
class AiAttachment {
  final String name;
  final String? modality;
  final String mimeType;
  final int sizeBytes;

  const AiAttachment({
    required this.name,
    required this.modality,
    required this.mimeType,
    required this.sizeBytes,
  });

  Map<String, dynamic> toJson() => {
        'name': name,
        if (modality != null) 'modality': modality,
        'mimeType': mimeType,
        'sizeBytes': sizeBytes,
      };

  String get sizeLabel {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) return '${(sizeBytes / 1024).toStringAsFixed(0)} KB';
    return '${(sizeBytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }
}

/// 预检结果：`allowed=false` 时必须阻断发送，不能"先发再说"（AIH-030）。
class AiPreflightResult {
  final bool allowed;
  final List<String> blockers;

  const AiPreflightResult({required this.allowed, required this.blockers});

  static const allow = AiPreflightResult(allowed: true, blockers: []);

  factory AiPreflightResult.fromJson(Map<String, dynamic> json) => AiPreflightResult(
        allowed: json['allowed'] == true,
        blockers: (json['blockers'] as List?)?.map((e) => e.toString()).toList() ?? const [],
      );
}
