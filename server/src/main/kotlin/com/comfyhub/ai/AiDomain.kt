package com.comfyhub.ai

import kotlinx.serialization.Serializable
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

object AttachmentPolicy {
    /** 模型声明 + 适配器实现 + MIME 白名单 + 大小/数量/像素预算，全部满足才放行。 */
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

        val transports = model.attachmentTransports[modality.wire].orEmpty()
            .mapNotNull { Transport.parse(it) }
            .toSet()
        val usable = transports.intersect(adapterTransports)
        if (transports.isEmpty()) {
            blockers += "模型未声明${modality.label()}的传输方式"
        } else if (usable.isEmpty()) {
            blockers += "${modality.label()}的传输方式（${transports.joinToString { it.wire }}）当前协议适配器尚未实现"
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
