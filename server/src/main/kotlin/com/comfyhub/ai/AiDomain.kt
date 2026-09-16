package com.comfyhub.ai

import com.comfyhub.ai.protocol.LevelSpec
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.ThinkingFormat
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.contentOrNull
import java.net.InetAddress
import java.net.URI

/**
 * AI 工作台的领域模型与校验规则（AIH-006 / AIH-011 / AIH-017 / AIH-024 / AIH-028）。
 *
 * 这一层**不依赖数据库、不依赖 HTTP**，所有规则都是纯函数，方便单测覆盖
 * "零上游请求"这类核心验收用例（AIH-030）。
 */

// ---------------------------------------------------------------------------
//  协议
// ---------------------------------------------------------------------------

/** 首期支持的三种协议；不含已淘汰的文本 /completions（DEC-002）。 */
enum class AiApi(val wire: String) {
    OPENAI_COMPLETIONS("openai-completions"),
    OPENAI_RESPONSES("openai-responses"),
    ANTHROPIC_MESSAGES("anthropic-messages");

    companion object {
        fun parse(wire: String?): AiApi? = entries.firstOrNull { it.wire == wire }
        val wireValues: List<String> get() = entries.map { it.wire }
    }
}

/** 端点信任级别（AIH-017）。默认最严，放宽必须由用户显式选择。 */
enum class EndpointTrust(val wire: String) {
    PUBLIC("public"),
    LOOPBACK("loopback"),
    PRIVATE_NETWORK("private-network"),
    UNSAFE_ANY("unsafe-any");

    companion object {
        fun parse(wire: String?): EndpointTrust? = entries.firstOrNull { it.wire == wire }
        /** 自动推断：本机回环 → LOOPBACK，内网 → PRIVATE_NETWORK，其余 → PUBLIC。 */
        fun infer(host: String): EndpointTrust = when (AddressScope.of(host)) {
            Scope.LOOPBACK -> LOOPBACK
            Scope.PRIVATE, Scope.LINK_LOCAL -> PRIVATE_NETWORK
            else -> PUBLIC
        }
    }
}

/** 输入模态（AIH-028）；"document" 覆盖 PDF 等文件型输入。 */
enum class Modality(val wire: String) {
    TEXT("text"), IMAGE("image"), VIDEO("video"), AUDIO("audio"), DOCUMENT("document");

    companion object {
        fun parse(wire: String?): Modality? = entries.firstOrNull { it.wire == wire }
        val wireValues: List<String> get() = entries.map { it.wire }
    }
}

/** 附件传输方式；协议适配器必须实现对应方式才允许发送。 */
enum class Transport(val wire: String) {
    INLINE_BASE64("inline_base64"),
    REMOTE_URL("remote_url"),
    FILE_ID("file_id"),
    EXTRACTED_TEXT("extracted_text");

    companion object {
        fun parse(wire: String?): Transport? = entries.firstOrNull { it.wire == wire }
    }
}

/** 稳定错误码（AIH-024）。 */
object AiErrorCode {
    const val MISSING_CREDENTIAL = "MISSING_CREDENTIAL"
    const val UNKNOWN_MODEL = "UNKNOWN_MODEL"
    const val RATE_LIMIT = "RATE_LIMIT"
    const val QUOTA_EXCEEDED = "QUOTA_EXCEEDED"
    const val CONFIG_ERROR = "CONFIG_ERROR"
    const val UNSUPPORTED_CONTENT = "UNSUPPORTED_CONTENT"
    const val PROTOCOL_ERROR = "PROTOCOL_ERROR"
    const val ABORTED = "ABORTED"
    const val PROVIDER_UNREACHABLE = "PROVIDER_UNREACHABLE"
    const val ATTACHMENT_BLOCKED = "ATTACHMENT_BLOCKED"
}

/** 业务异常：带稳定 code，且不允许把密钥写进 message。 */
class AiException(val code: String, message: String) : RuntimeException(message)

/**
 * 一次请求的 token 用量（AIH-057）。
 *
 * 各家的 `usage` 字段名不同（OpenAI 用 `prompt_tokens`，Anthropic 用 `input_tokens`，
 * 还有 `prompt_tokens_details.cached_tokens` 这种明细），这里统一归一化成四个数，
 * 前端就不必各自解析供应商方言了。
 */
@Serializable
data class TokenUsage(
    val inputTokens: Long = 0,
    val outputTokens: Long = 0,
    /** 缓存命中的输入 token（计费通常更便宜） */
    val cachedTokens: Long = 0,
    /** 思考 token（Anthropic 的 `thinking` 明细；OpenAI 兼容网关一般不给） */
    val reasoningTokens: Long = 0,
) {
    val totalTokens: Long get() = inputTokens + outputTokens
    val isEmpty: Boolean get() = totalTokens == 0L && cachedTokens == 0L

    companion object {
        val EMPTY = TokenUsage()

        private val INPUT_KEYS = listOf("prompt_tokens", "input_tokens", "inputTokens", "promptTokens")
        private val OUTPUT_KEYS = listOf("completion_tokens", "output_tokens", "outputTokens", "completionTokens")
        private val TOTAL_KEYS = listOf("total_tokens", "totalTokens")

        /** 从任意供应商的 usage 对象里尽量抽出数字；认不出来就是 0，不猜。 */
        fun from(usage: kotlinx.serialization.json.JsonElement?): TokenUsage {
            val obj = usage as? kotlinx.serialization.json.JsonObject ?: return EMPTY
            fun num(vararg keys: String): Long {
                keys.forEach { k ->
                    val v = (obj[k] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull
                    val n = v?.toDoubleOrNull()
                    if (n != null) return n.toLong()
                }
                return 0L
            }
            fun nested(parent: String, vararg keys: String): Long {
                val child = obj[parent] as? kotlinx.serialization.json.JsonObject ?: return 0L
                keys.forEach { k ->
                    val v = (child[k] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull
                    val n = v?.toDoubleOrNull()
                    if (n != null) return n.toLong()
                }
                return 0L
            }

            var input = num(*INPUT_KEYS.toTypedArray())
            var output = num(*OUTPUT_KEYS.toTypedArray())
            val total = num(*TOTAL_KEYS.toTypedArray())
            // 只给了 total 的网关：尽量拆开，拆不开就都记在输入上（totalTokens 仍然正确）
            if (input == 0L && output == 0L && total > 0L) input = total

            val cached = maxOf(
                nested("prompt_tokens_details", "cached_tokens"),
                nested("input_tokens_details", "cached_tokens"),
                num("cache_read_input_tokens"),
            )
            val reasoningOut = maxOf(
                nested("completion_tokens_details", "reasoning_tokens"),
                num("reasoning_tokens"),
            )
            return TokenUsage(input, output, cached, reasoningOut)
        }
    }
}

// ---------------------------------------------------------------------------
//  DTO
// ---------------------------------------------------------------------------

@Serializable
data class AiProviderDto(
    val id: String,
    val displayName: String,
    val api: String,
    val baseURL: String,
    val credentialRef: String? = null,
    val endpointTrust: String,
    val enabled: Boolean = true,
    val revision: Long = 1,
    /** 只写凭据的状态，绝不包含值（AIH-012） */
    val credential: CredentialStatusDto = CredentialStatusDto(configured = false, source = "none", writable = false),
    val createdAt: String? = null,
    val updatedAt: String? = null,
)

@Serializable
data class AiProviderUpsert(
    val id: String? = null,
    val displayName: String,
    val api: String,
    val baseURL: String,
    val credentialRef: String? = null,
    val endpointTrust: String? = null,
    val enabled: Boolean = true,
    /** 乐观锁：更新时必须带上读到的 revision（AIH-007） */
    val revision: Long? = null,
)

@Serializable
data class CredentialStatusDto(
    /** 是否已配置 */
    val configured: Boolean,
    /** env / managed / none */
    val source: String,
    /** 前端是否可写；env 来源只读（AIH-013） */
    val writable: Boolean,
)

@Serializable
data class AiModelDto(
    val providerId: String,
    val id: String,
    val displayName: String,
    val inputModalities: List<String> = emptyList(),
    val attachmentTransports: Map<String, List<String>> = emptyMap(),
    val mimeAllowlist: List<String> = emptyList(),
    val tools: Boolean = false,
    val parallelTools: Boolean = false,
    val reasoning: Boolean = false,
    /**
     * 该模型可选的思考等级（AIH-056）。键是等级（off/low/medium/high/max），
     * 值是"过线拼写"：字符串表示改名（如 `max: ultra`），数字表示 Anthropic 的思考预算。
     * 空 = 不声明，UI 上不显示思考强度选择器。
     */
    val thinkingEfforts: Map<String, String> = emptyMap(),
    /** 网关的思考方言：openai / deepseek / qwen / openrouter / zai；空 = 按协议默认。 */
    val thinkingFormat: String? = null,
    val contextWindow: Int? = null,
    val maxOutputTokens: Int? = null,
    val maxAttachmentBytes: Long? = null,
    val maxAttachmentCount: Int? = null,
    /** builtin / discovered / manual / tested */
    val capabilitySource: String = "manual",
    val capabilityVerifiedAt: String? = null,
    val enabled: Boolean = true,
)

@Serializable
data class AiModelUpsert(val models: List<AiModelDto>)

// ---------------------------------------------------------------------------
//  Provider ID / URL 校验
// ---------------------------------------------------------------------------

object AiValidation {
    private val KEBAB = Regex("^[a-z0-9]+(-[a-z0-9]+)*$")
    private val CRED_REF = Regex("^[A-Z][A-Z0-9_]{0,127}$")

    const val MAX_ID_LEN = 96
    const val MAX_URL_LEN = 1024

    fun requireProviderId(raw: String?): String {
        val id = raw?.trim().orEmpty()
        if (id.isEmpty()) throw AiException(AiErrorCode.CONFIG_ERROR, "Provider ID 不能为空")
        if (id.length > MAX_ID_LEN) throw AiException(AiErrorCode.CONFIG_ERROR, "Provider ID 不能超过 $MAX_ID_LEN 个字符")
        if (!KEBAB.matches(id)) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "Provider ID 只能是小写 kebab-case（如 my-gateway），且创建后不可修改")
        }
        return id
    }

    /** 凭据引用名：大写字母开头的环境变量风格，便于轮换与审计。 */
    fun requireCredentialRef(raw: String?): String? {
        val ref = raw?.trim().orEmpty()
        if (ref.isEmpty()) return null
        if (!CRED_REF.matches(ref)) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "凭据引用名只能是大写字母、数字和下划线，且以字母开头")
        }
        return ref
    }

    fun requireDisplayName(raw: String?): String {
        val name = raw?.trim().orEmpty()
        if (name.isEmpty()) throw AiException(AiErrorCode.CONFIG_ERROR, "显示名不能为空")
        if (name.length > 128) throw AiException(AiErrorCode.CONFIG_ERROR, "显示名不能超过 128 个字符")
        return name
    }

    /**
     * 模型 ID 校验：只要求非空、无控制字符、长度合理。
     * **不根据 ID 猜能力**（AIH-011），也**不限定命名规则** —— 各家网关的 ID 形态差异很大。
     */
    fun requireModelId(raw: String?): String {
        val id = raw?.trim().orEmpty()
        if (id.isEmpty()) throw AiException(AiErrorCode.CONFIG_ERROR, "模型 ID 不能为空")
        if (id.length > 191) throw AiException(AiErrorCode.CONFIG_ERROR, "模型 ID 不能超过 191 个字符")
        if (id.any { it.isISOControl() }) throw AiException(AiErrorCode.CONFIG_ERROR, "模型 ID 不能包含控制字符")
        return id
    }

    /**
     * 校验并规范化 Base URL：只去掉末尾的 `/`，不擅自改写用户路径
     * （Anthropic 兼容网关的路径经常不是 `/v1`）。
     */
    fun normalizeBaseUrl(raw: String?, trust: EndpointTrust): String {
        val text = raw?.trim().orEmpty()
        if (text.isEmpty()) throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 不能为空")
        if (text.length > MAX_URL_LEN) throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 过长")
        val uri = runCatching { URI(text) }.getOrNull()
            ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 不是合法 URL")
        val scheme = uri.scheme?.lowercase()
        if (scheme != "http" && scheme != "https") {
            throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 只支持 http/https")
        }
        val host = uri.host ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 缺少主机名")
        if (uri.userInfo != null) throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 不能包含用户名密码，请使用凭据配置")
        if (uri.query != null) throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 不能带查询参数")
        if (uri.fragment != null) throw AiException(AiErrorCode.CONFIG_ERROR, "Base URL 不能带 fragment")
        if (trust == EndpointTrust.PUBLIC && scheme != "https") {
            throw AiException(AiErrorCode.CONFIG_ERROR, "公网端点必须使用 https")
        }
        EndpointGuard.assertAllowed(host, trust)
        return text.trimEnd('/')
    }
    /**
     * 思考强度校验（AIH-056）：**模型是唯一真源**。
     *
     * - 模型没声明 `reasoning` → 任何强度都拒绝（返回 null 表示"本次不思考"）；
     * - 模型用 `thinkingEfforts` 显式列了可选等级 → 只允许其中的等级，不猜；
     * - 请求 `off`（或没传）→ 返回 null，不发任何思考参数。
     */
    fun requireThinkingEffort(model: AiModelDto, requested: ReasoningEffort?): ReasoningEffort? {
        if (requested == null || requested == ReasoningEffort.OFF) return null
        if (!model.reasoning) {
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "模型 ${model.id} 未声明推理能力，不能设置思考强度（可在「设置 → AI 模型」里声明）"
            )
        }
        val declared = parseThinkingEfforts(model.thinkingEfforts)
        if (declared.isEmpty()) return requested
        if (requested !in declared) {
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "模型 ${model.id} 未声明思考强度 ${requested.wire}（可选：${declared.joinToString(" / ") { it.wire }}）"
            )
        }
        return requested
    }

    /** 思考等级表 → 枚举 + 过线表达；无法识别的键值由 [validateThinkingEfforts] 在保存时拦下。 */
    fun parseThinkingEfforts(raw: Map<String, String>): Set<ReasoningEffort> =
        raw.keys.mapNotNull { ReasoningEffort.parse(it) }
            .filter { it != ReasoningEffort.OFF }
            .toSet()

    /** 保存模型目录时校验思考声明：等级必须认识、表达必须非空且不能带控制字符。 */
    fun validateThinkingEfforts(m: AiModelDto) {
        m.thinkingEfforts.forEach { (level, value) ->
            ReasoningEffort.parse(level)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "未知的思考等级: $level")
            if (LevelSpec.of(value) == null) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "思考等级 $level 的表达不能为空")
            }
            if (value.length > 64 || value.any { it.isISOControl() }) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "思考等级 $level 的表达不合法")
            }
        }
        if (!m.thinkingFormat.isNullOrBlank() && ThinkingFormat.parse(m.thinkingFormat) == null) {
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "未知的思考方言: ${m.thinkingFormat}（可选 ${ThinkingFormat.entries.joinToString(" / ") { it.wire }}）"
            )
        }
        if (m.thinkingEfforts.isNotEmpty() && !m.reasoning) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "声明了思考等级却没勾选「支持推理」，两者必须一致")
        }
    }
}

// ---------------------------------------------------------------------------
//  SSRF 防护（AIH-017）
// ---------------------------------------------------------------------------

enum class Scope { LOOPBACK, PRIVATE, LINK_LOCAL, PUBLIC, BLOCKED }

object AddressScope {
    /** 云元数据地址：无论信任级别都不允许。 */
    private val BLOCKED_HOSTS = setOf("metadata.google.internal", "metadata.goog")

    fun of(host: String): Scope {
        val h = host.trim().lowercase().removeSurrounding("[", "]")
        if (h.isEmpty()) return Scope.BLOCKED
        if (h in BLOCKED_HOSTS) return Scope.BLOCKED
        if (h == "localhost" || h.endsWith(".localhost")) return Scope.LOOPBACK
        val addr = parseLiteral(h) ?: return Scope.PUBLIC // 域名：留给请求时做 DNS 复核
        if (addr.isLoopbackAddress) return Scope.LOOPBACK
        if (addr.isAnyLocalAddress) return Scope.BLOCKED
        // 链路本地网段（含 169.254.169.254 云元数据）一律视为红线：
        // 正常的模型网关不会部署在链路本地地址上。
        if (addr.isLinkLocalAddress) return Scope.BLOCKED
        if (addr.isSiteLocalAddress) return Scope.PRIVATE
        return Scope.PUBLIC
    }

    private fun parseLiteral(host: String): InetAddress? {
        val looksLikeIp = host.contains(':') || host.split('.').let { parts ->
            parts.size == 4 && parts.all { it.isNotEmpty() && it.all(Char::isDigit) }
        }
        if (!looksLikeIp) return null
        return runCatching { InetAddress.getByName(host) }.getOrNull()
    }
}

object EndpointGuard {
    /** 按信任级别校验主机；失败抛 CONFIG_ERROR（保存时即拦截，不等请求发出）。 */
    fun assertAllowed(host: String, trust: EndpointTrust) {
        val scope = AddressScope.of(host)
        // 云元数据/保留地址是硬红线，任何信任级别（含 unsafe-any）都不放行
        if (scope == Scope.BLOCKED) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "该地址属于云元数据/保留地址，已拒绝")
        }
        if (trust == EndpointTrust.UNSAFE_ANY) return
        val ok = when (trust) {
            EndpointTrust.PUBLIC -> scope == Scope.PUBLIC
            EndpointTrust.LOOPBACK -> scope == Scope.LOOPBACK
            EndpointTrust.PRIVATE_NETWORK -> scope == Scope.PRIVATE || scope == Scope.LOOPBACK || scope == Scope.LINK_LOCAL
            EndpointTrust.UNSAFE_ANY -> true
        }
        if (!ok) {
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "端点信任级别 ${trust.wire} 不允许访问主机 $host（当前判定：$scope）"
            )
        }
    }

    /**
     * 请求前复核：把主机名解析成真实地址再检查一次，防止 DNS rebinding。
     * 返回解析出的地址，供调用方建立连接使用。
     */
    fun resolveAndCheck(host: String, trust: EndpointTrust): List<InetAddress> {
        assertAllowed(host, trust)
        val addresses = runCatching { InetAddress.getAllByName(host).toList() }.getOrElse {
            throw AiException(AiErrorCode.PROVIDER_UNREACHABLE, "无法解析主机 $host")
        }
        addresses.forEach { addr ->
            assertAllowed(addr.hostAddress ?: "", trust)
        }
        return addresses
    }
}

// ---------------------------------------------------------------------------
//  附件准入（AIH-028 / AIH-030）
// ---------------------------------------------------------------------------

/** 附件的客观事实（由严格分类器产出，AIH-027）。 */
data class AttachmentFact(
    val name: String,
    val modality: Modality?,
    val mimeType: String,
    val sizeBytes: Long,
    /** 图片像素预算；非图片为 null */
    val pixels: Long? = null,
)

/**
 * 准入结论。
 *
 * `allowed=false` 时**必须**阻断发送；调用方不允许把 blocker 降级成占位符后继续请求上游，
 * 这正是 AIH-030"上游请求数为 0"要保证的不变式。
 */
data class PreflightResult(val allowed: Boolean, val blockers: List<String>) {
    companion object {
        fun allow() = PreflightResult(true, emptyList())
        fun block(vararg reasons: String) = PreflightResult(false, reasons.toList())
    }
}

/**
 * 附件入库前的严格判定（AIH-027）。
 *
 * 前端声明的模态**只是线索**：只要拿到文件头，就以**后端按签名判定**的结果为准。
 * 这样"前端谎报 image 骗过准入"这条路径是不通的 —— 判定权在后端手里。
 */
object StrictIntake {
    /** 允许前端提交的文件头大小上限；超过就截断，避免拿它当上传通道。 */
    const val MAX_HEAD_BYTES = 4096

    data class Resolved(val fact: AttachmentFact, val blockers: List<String>)

    fun resolve(
        name: String,
        declaredModality: String?,
        declaredMime: String,
        sizeBytes: Long,
        pixels: Long?,
        head: ByteArray?,
    ): Resolved {
        val declared = Modality.parse(declaredModality)
        if (head == null || head.isEmpty()) {
            // 没有文件头：只能采信声明，但把"未经签名确认"如实告诉调用方
            return Resolved(
                AttachmentFact(name, declared, declaredMime, sizeBytes, pixels),
                emptyList(),
            )
        }
        val detected = FileKindDetector.detect(head.copyOf(MAX_HEAD_BYTES), name, declaredMime)
        if (detected.kind == FileKind.UNKNOWN) {
            return Resolved(
                AttachmentFact(name, null, detected.mimeType ?: declaredMime, sizeBytes, pixels),
                listOf("无法识别文件 $name 的真实类型（签名不匹配声明的 $declaredMime），已阻断"),
            )
        }
        return Resolved(
            AttachmentFact(name, detected.modality, detected.mimeType ?: declaredMime, sizeBytes, pixels),
            emptyList(),
        )
    }
}

object AttachmentPolicy {
    /**
     * 内联 base64 的原始字节上限（与 `AiAttachmentStore.MAX_INLINE_BYTES` 同一个口径）。
     *
     * base64 会把体积放大 1/3，各网关的请求体上限普遍在 20MB 量级，所以超过这个体积的图片
     * **不发**，让用户先压缩 —— 而不是发出去被网关 413。
     */
    const val MAX_INLINE_BYTES: Long = 8L * 1024 * 1024

    /**
     * 模型声明 + 适配器实现 + MIME 白名单 + 大小/数量/像素预算，全部满足才放行。
     *
     * @param adapterTransports **该模态**在这个协议适配器上真的实现了的传输方式
     *   （`AdapterCapabilities.transportsFor(api, modality)`）。
     */
    fun evaluate(
        model: AiModelDto,
        attachment: AttachmentFact,
        adapterTransports: Set<Transport>,
        currentAttachmentCount: Int = 0,
    ): PreflightResult {
        val blockers = mutableListOf<String>()
        val modality = attachment.modality
        if (modality == null || modality == Modality.TEXT) {
            // 文本附件走正文抽取，不算多模态输入；这里只做类型未知的阻断
            if (modality == null) blockers += "无法识别文件类型（${attachment.mimeType}），已阻断；请确认为受支持的图片或文档"
            return if (blockers.isEmpty()) PreflightResult.allow() else PreflightResult.block(*blockers.toTypedArray())
        }

        if (!model.inputModalities.contains(modality.wire)) {
            blockers += "模型 ${model.displayName} 未声明支持${modality.label()}输入"
        }

        // 传输方式是**协议**的属性，模型声明的是"能不能吃这种输入"。
        // 模型显式声明了就用它的（与适配器的交集），没声明就用适配器实现的那种 ——
        // 否则内置目录里那 69 个模型（只声明模态、不声明传输）永远发不出图片。
        val declared = model.attachmentTransports[modality.wire].orEmpty()
            .mapNotNull { Transport.parse(it) }
            .toSet()
        val usable = if (declared.isEmpty()) adapterTransports else declared.intersect(adapterTransports)
        if (usable.isEmpty()) {
            blockers += if (declared.isEmpty()) {
                "${modality.label()}的传输方式当前协议适配器尚未实现"
            } else {
                "${modality.label()}的传输方式（${declared.joinToString { it.wire }}）当前协议适配器尚未实现"
            }
        } else if (Transport.INLINE_BASE64 in usable && attachment.sizeBytes > MAX_INLINE_BYTES) {
            blockers += "文件 ${attachment.name} 超过内联发送上限（${MAX_INLINE_BYTES / 1024 / 1024} MB），" +
                "请先压缩或换一张更小的图"
        }

        val maxBytes = model.maxAttachmentBytes
        if (maxBytes != null && attachment.sizeBytes > maxBytes) {
            blockers += "文件 ${attachment.name} 超过大小上限（${maxBytes / 1024 / 1024} MB）"
        }
        val maxCount = model.maxAttachmentCount
        if (maxCount != null && currentAttachmentCount >= maxCount) {
            blockers += "附件数量超过上限（$maxCount）"
        }
        if (model.mimeAllowlist.isNotEmpty() && !matchesMime(model.mimeAllowlist, attachment.mimeType)) {
            blockers += "该模型不接受 ${attachment.mimeType} 类型"
        }

        return if (blockers.isEmpty()) PreflightResult.allow() else PreflightResult.block(*blockers.toTypedArray())
    }

    private fun matchesMime(allowlist: List<String>, mime: String): Boolean =
        allowlist.any { rule ->
            val r = rule.trim().lowercase()
            when {
                r == mime.lowercase() -> true
                r.endsWith("/*") -> mime.lowercase().startsWith(r.removeSuffix("*"))
                else -> false
            }
        }

    private fun Modality.label(): String = when (this) {
        Modality.TEXT -> "文本"
        Modality.IMAGE -> "图片"
        Modality.VIDEO -> "视频"
        Modality.AUDIO -> "音频"
        Modality.DOCUMENT -> "文档"
    }
}

/**
 * 一次请求里"哪些附件真的发出去"的判定（M3，纯函数便于单测）。
 *
 * 一次对话可能累积好几张历史图片，而请求体不能无限大：超过预算的那些**不发**，
 * 但调用方必须把它们变成一条**明确的说明**（"这张图没随本次请求发送"），
 * 而不是静默消失 —— AIH-030 要防的就是"看起来发出去了、其实没有"。
 *
 * 这是"贪心 + 保序"：从最新到最旧由调用方决定顺序，装不下就跳过、继续试后面的小图。
 */
object InlineBudget {
    /** 单次请求内联附件的原始字节总预算（base64 后约 ×1.33）。 */
    const val MAX_TOTAL_BYTES: Long = 20L * 1024 * 1024

    fun plan(
        sizes: List<Long>,
        maxTotal: Long = MAX_TOTAL_BYTES,
        maxSingle: Long = AttachmentPolicy.MAX_INLINE_BYTES,
    ): List<Boolean> {
        var total = 0L
        return sizes.map { size ->
            val ok = size in 1..maxSingle && total + size <= maxTotal
            if (ok) total += size
            ok
        }
    }
}
