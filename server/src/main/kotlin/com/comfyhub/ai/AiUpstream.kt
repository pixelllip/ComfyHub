package com.comfyhub.ai

import org.slf4j.LoggerFactory
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.time.Duration
import kotlinx.serialization.json.contentOrNull

/**
 * 上游 Provider 的连接测试与（后续）请求发送（AIH-008）。
 *
 * 四条硬规则：
 *  1. **绝不记录 Authorization / x-api-key**：日志只写 Provider ID、脱敏后的端点、状态码和稳定错误码；
 *  2. **不跟随重定向**（`Redirect.NEVER`），避免密钥被带到别的域名；
 *  3. 请求前用 [EndpointGuard] **再复核一次**目标地址（防 DNS rebinding）；
 *  4. 错误必须归到稳定错误码，前端和 Harness 都不去解析供应商文案。
 */
object AiUpstream {
    private val log = LoggerFactory.getLogger(AiUpstream::class.java)

    data class TestResult(
        val ok: Boolean,
        val errorCode: String?,
        val message: String,
        val httpStatus: Int? = null,
        val modelCount: Int? = null,
    )

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(5))
        .followRedirects(HttpClient.Redirect.NEVER)
        .build()

    /** 状态码 → 稳定错误码（AIH-024）。纯函数，便于单测。 */
    fun classifyStatus(status: Int): String? = when {        status in 200..299 -> null
        status == 401 || status == 403 -> AiErrorCode.MISSING_CREDENTIAL
        status == 404 -> AiErrorCode.PROTOCOL_ERROR
        status == 408 || status == 504 -> AiErrorCode.PROVIDER_UNREACHABLE
        status == 429 -> AiErrorCode.RATE_LIMIT
        status == 402 -> AiErrorCode.QUOTA_EXCEEDED
        status in 400..499 -> AiErrorCode.CONFIG_ERROR
        status >= 500 -> AiErrorCode.PROVIDER_UNREACHABLE
        else -> AiErrorCode.PROTOCOL_ERROR
    }

    /**
     * 列表 URL：OpenAI 兼容拼 `/models`，Anthropic 只对列表归一化 `/v1`。
     *
     * 这是**首选**地址；探测失败时会依次回退到 [modelsUrlCandidates] 里的其它写法。
     */
    fun modelsUrl(baseURL: String, api: AiApi): String {
        val base = baseURL.trimEnd('/')
        return when (api) {
            AiApi.ANTHROPIC_MESSAGES ->
                if (base.endsWith("/v1")) "$base/models" else "$base/v1/models"
            AiApi.OPENAI_COMPLETIONS, AiApi.OPENAI_RESPONSES -> "$base/models"
        }
    }

    /**
     * 模型列表地址的候选序列（按可能性从高到低）。
     *
     * 为什么需要它：同一个 Key，Base URL 填 `https://api.openai.com` 和
     * `https://api.openai.com/v1` 是两个不同的 URL —— 前者拼出来是
     * `https://api.openai.com/models`（**404**），用户看到的却是"连接失败"。
     * 官方的对话 / 响应端点路径里带 `/v1`，而 `/models` 也有不带 `/v1` 的网关，
     * 所以这里两种都试，试到能通为止，并把"实际用的是哪个 URL"回报给界面。
     */
    fun modelsUrlCandidates(baseURL: String, api: AiApi): List<String> {
        val base = baseURL.trimEnd('/')
        val hasV1 = base.endsWith("/v1")
        val stripped = if (hasV1) base.dropLast(3) else "$base/v1"
        // 另一种写法：Base URL 带 /v1 就去掉试，不带就补上试。
        // 两种协议都用同一条规则 —— 区别只在首选是哪一个（见 [modelsUrl]）。
        val alternate = "$stripped/models"
        return (listOf(modelsUrl(baseURL, api)) + alternate).distinct()
    }

    private data class Probe(
        val ok: Boolean,
        val status: Int?,
        val code: String,
        val detail: String,
        val modelCount: Int? = null,
        /** 只有确实需要正文时（取模型列表）才带回来，避免白读一遍 */
        val body: String? = null,
    )

    /** 发一次 `GET` 探测；`/models` 拿到 404 单独标出来（见 [testConnection]）。 */
    private fun httpGet(
        url: String,
        api: AiApi,
        secret: String?,
        trust: EndpointTrust,
        timeoutSeconds: Long,
        withBody: Boolean = false,
    ): Probe {
        val uri = runCatching { URI(url) }.getOrNull()
            ?: return Probe(false, null, AiErrorCode.CONFIG_ERROR, "Base URL 无法解析")
        val host = uri.host ?: return Probe(false, null, AiErrorCode.CONFIG_ERROR, "Base URL 缺少主机名")
        runCatching { EndpointGuard.resolveAndCheck(host, trust) }.getOrElse { e ->
            return Probe(false, null, AiErrorCode.CONFIG_ERROR, e.message ?: "目标地址不被允许")
        }

        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(timeoutSeconds))
            .GET()
            .header("Accept", "application/json")
        when (api) {
            AiApi.OPENAI_COMPLETIONS, AiApi.OPENAI_RESPONSES ->
                if (!secret.isNullOrEmpty()) builder.header("Authorization", "Bearer $secret")
            AiApi.ANTHROPIC_MESSAGES -> {
                if (!secret.isNullOrEmpty()) builder.header("x-api-key", secret)
                builder.header("anthropic-version", "2023-06-01")
            }
        }

        return try {
            val response = client.send(builder.build(), HttpResponse.BodyHandlers.ofString())
            val status = response.statusCode()
            val body = response.body().orEmpty()
            val base = classifyStatus(status)
                ?: return Probe(true, status, "", "", countModels(body, api), body.takeIf { withBody })
            val code = refineFromBody(base, body) ?: base
            if (status == 404) {
                // 鉴权不在这里判断：有些网关根本没有 /models，给 404 不代表 Key 不对
                Probe(false, status, MODEL_LIST_STATUS, explain(code, status))
            } else {
                Probe(false, status, code, explain(code, status) + "｜上游返回：" + redact(body, secret))
            }
        } catch (e: java.net.http.HttpTimeoutException) {
            Probe(false, null, AiErrorCode.PROVIDER_UNREACHABLE, "连接超时（${timeoutSeconds}秒）")
        } catch (e: Exception) {
            Probe(false, null, AiErrorCode.PROVIDER_UNREACHABLE, "无法连接：${e::class.simpleName}")
        }
    }

    /** `/models` 返回 404 时的内部标记：**鉴权已经过了**，只是拿不到模型列表。 */
    private const val MODEL_LIST_STATUS = "MODEL_LIST_UNAVAILABLE"

    /**
     * 连接测试：不看 `Bearer` 是否被接受，只看**这个 Base URL 到底能不能用**。
     *
     * Base URL 填 `.../v1` 与不填是两个不同的地址，所以候选地址依次试；
     * 全都拿不到列表时，只要有一个地址**不是鉴权失败**（401/403），
     * 就认为连通（Key 是对的，只是这份端点不提供 /models）。
     */
    fun testConnection(provider: AiProviderDto, secret: String?): TestResult {
        val api = AiApi.parse(provider.api)
            ?: return TestResult(false, AiErrorCode.CONFIG_ERROR, "未知协议：${provider.api}")
        val trust = EndpointTrust.parse(provider.endpointTrust) ?: EndpointTrust.PUBLIC

        // 先查凭据（便宜、不需要网络）：缺密钥时应该报得比"地址不可信"更准
        if (provider.credentialRef != null && secret.isNullOrEmpty()) {
            return TestResult(false, AiErrorCode.MISSING_CREDENTIAL, "该 Provider 引用了凭据 ${provider.credentialRef}，但本机没有配置")
        }

        val candidates = modelsUrlCandidates(provider.baseURL, api)
        var firstProbe: Probe? = null
        for (url in candidates) {
            val probe = httpGet(url, api, secret, trust, timeoutSeconds = 10)
            if (firstProbe == null) firstProbe = probe
            log.info(
                "Provider 连接测试 provider={} endpoint={} status={} code={}",
                provider.id, url, probe.status, probe.code,
            )
            if (probe.ok) {
                val via = if (url == candidates.first()) "" else "（实际可用地址 $url）"
                return TestResult(
                    true, null,
                    "连接正常（HTTP ${probe.status}）$via",
                    probe.status, probe.modelCount,
                )
            }
            // 鉴权失败：换地址也没用，直接报（这才是真正的"API Key 报错"）
            if (probe.code == AiErrorCode.MISSING_CREDENTIAL) {
                return TestResult(false, probe.code, probe.detail, probe.status)
            }
        }

        val first = firstProbe ?: return TestResult(false, AiErrorCode.CONFIG_ERROR, "Base URL 无法解析")
        // 所有候选都拿不到模型列表：只要不是鉴权问题，就说明端点本身是通的
        if (first.code == MODEL_LIST_STATUS) {
            return TestResult(
                true, null,
                "连接正常（HTTP ${first.status}）：该端点不提供模型列表（/models 返回 404），"
                    + "因此「获取可用模型」可能为空，手工添加模型即可。",
                first.status,
            )
        }
        return TestResult(false, first.code, first.detail, first.status)
    }

    /** 日志里只出现 scheme://host:port/path，不带 query / fragment / userInfo。 */
    private fun safeEndpoint(uri: URI): String = buildString {
        append(uri.scheme).append("://").append(uri.host)
        if (uri.port > 0) append(':').append(uri.port)
        append(uri.path ?: "")
    }

    /**
     * 上游报错文本脱敏后回显：**必须**先把密钥抹掉再截断（AIH-051）。
     * 有些网关会把请求体/请求头原样回显，直接透传等于把 Key 写进界面和日志。
     */
    fun redact(raw: String, secret: String?): String {
        var text = raw
        if (!secret.isNullOrEmpty()) text = text.replace(secret, "***")
        text = text.replace(Regex("(?i)(bearer\\s+)[A-Za-z0-9._\\-]{6,}"), "$1***")
        text = text.replace(
            Regex("(?i)((?:api[-_]?key|x-api-key|authorization)\"?\\s*[:=]\\s*\"?)[^\",}\\s]{6,}"),
            "$1***"
        )
        return text.replace(Regex("\\s+"), " ").trim().take(400)
    }

    fun countModels(body: String, api: AiApi): Int? = runCatching {
        val root = com.comfyhub.AppJson.parseToJsonElement(body)
        val array = when (root) {
            is kotlinx.serialization.json.JsonArray -> root
            is kotlinx.serialization.json.JsonObject ->
                // OpenAI 标准 data[]，也兼容富信息 models{}
                (root["data"] ?: root["models"]) as? kotlinx.serialization.json.JsonArray
            else -> null
        }
        array?.size
    }.getOrNull()

    fun explain(code: String, status: Int?): String = when (code) {
        AiErrorCode.MISSING_CREDENTIAL -> "鉴权失败（HTTP $status）：请检查 API Key 是否正确、是否已过期"
        AiErrorCode.PROTOCOL_ERROR ->
            "端点或协议不匹配（HTTP $status）：确认 Base URL 与所选协议；" +
                "若网关同时提供 OpenAI 与 Anthropic 两种端点，需要为不同协议的模型各建一条 Provider"
        AiErrorCode.RATE_LIMIT -> "被限流（HTTP 429）"
        AiErrorCode.QUOTA_EXCEEDED -> "配额/套餐不足或该模型不在你的套餐内（HTTP $status）"
        AiErrorCode.PROVIDER_UNREACHABLE -> "上游不可达或返回服务端错误（HTTP $status）"
        else -> "配置有误（HTTP $status）"
    }

    /**
     * 用上游返回的正文**细化**错误码（AIH-024）。
     *
     * 真实网关的 400/403 语义差别很大，只看状态码会把
     * "套餐里没有这个模型" 误报成 "密钥不对"，把 "这个模型该走另一个端点"
     * 误报成 "配置随便写错了"。这里按正文里的关键字纠正。
     */
    fun refineFromBody(baseCode: String?, body: String): String? {
        val b = body.lowercase()
        return when {
            // 套餐 / 额度类：403 也可能是"模型不在套餐内"
            b.contains("not_in_plan") || b.contains("not in plan") ||
                b.contains("quota") || b.contains("insufficient_quota") ||
                b.contains("billing") || b.contains("credit") -> AiErrorCode.QUOTA_EXCEEDED
            // 混合网关最常见的坑：模型不属于当前协议的端点
            b.contains("not supported on this endpoint") || b.contains("unsupported_model") ||
                b.contains("use /") && b.contains("provider/v1") -> AiErrorCode.PROTOCOL_ERROR
            b.contains("invalid api key") || b.contains("authentication_error") ||
                b.contains("unauthorized") -> AiErrorCode.MISSING_CREDENTIAL
            b.contains("rate limit") || b.contains("rate_limit") -> AiErrorCode.RATE_LIMIT
            else -> baseCode
        }
    }

    // -----------------------------------------------------------------------
    //  模型发现（AIH-009）：只产生候选，**不落库**
    // -----------------------------------------------------------------------

    data class ModelCandidate(
        val id: String,
        val displayName: String,
        val contextWindow: Int? = null,
        val maxOutputTokens: Int? = null,
        /** 预处理好的输入模态：接口声明 > 内置目录 > 保守的仅文本 */
        val modalities: List<String> = listOf("text"),
        val tools: Boolean = false,
        val reasoning: Boolean = false,
        /**
         * 预填的思考档位（AIH-056）：等级 → 线上表达。
         *
         * 只来自内置目录（接口一般不声明这个）。用户可以改；
         * 保存时后端会校验"声明了档位就必须勾推理"。
         */
        val thinkingEfforts: Map<String, String> = emptyMap(),
        /** 预填的思考方言：deepseek / qwen / zai / openrouter；null = 按协议默认。 */
        val thinkingFormat: String? = null,
        /** discovered / builtin / unknown，界面上要显示给用户看 */
        val capabilitySource: String = "unknown",
        /** 人类可读的来源说明，例如「命中内置规则 claude-3」 */
        val capabilityNote: String? = null,
    )

    data class DiscoveryResult(
        val ok: Boolean,
        val errorCode: String? = null,
        val message: String,
        val candidates: List<ModelCandidate> = emptyList(),
    )

    fun discoverModels(provider: AiProviderDto, secret: String?): DiscoveryResult {
        val response = getModels(provider, secret)
        if (!response.ok) {
            return DiscoveryResult(false, response.errorCode, response.message)
        }
        val candidates = parseCandidates(response.body)
        val declared = candidates.count { it.capabilitySource == CapabilitySource.DISCOVERED.wire }
        val builtin = candidates.count { it.capabilitySource == CapabilitySource.BUILTIN.wire }
        val unknown = candidates.count { it.capabilitySource == "unknown" }
        return DiscoveryResult(
            ok = true,
            message = if (candidates.isEmpty()) {
                "未发现模型（端点返回空列表）"
            } else {
                "发现 ${candidates.size} 个候选：接口声明 $declared 个、内置目录 $builtin 个、未识别 $unknown 个"
            },
            candidates = candidates,
        )
    }

    /**
     * 解析模型列表。
     *
     * 能力按**可信度从高到低**处理（AIH-011 的"不猜能力"依然成立）：
     *  1. 接口自己声明的能力（OpenRouter 的 `architecture.input_modalities`、
     *     各类 `capabilities` / `supports_vision` 字段）→ 标记为「接口声明」；
     *  2. 接口没说，但命中了[ModelCapabilityCatalog]这张按公开文档整理的离线表
     *     → 标记为「内置目录」，界面上可见、可改；
     *  3. 两者都没有 → **只给文本**，标记为「未识别」，绝不按名字猜图片能力。
     */
    fun parseCandidates(body: String): List<ModelCandidate> = runCatching {
        val root = com.comfyhub.AppJson.parseToJsonElement(body)
        val array = when (root) {
            is kotlinx.serialization.json.JsonArray -> root
            is kotlinx.serialization.json.JsonObject ->
                (root["data"] ?: root["models"]) as? kotlinx.serialization.json.JsonArray
            else -> null
        } ?: return emptyList()

        array.mapNotNull { element ->
            val obj = element as? kotlinx.serialization.json.JsonObject ?: return@mapNotNull null
            fun text(vararg keys: String): String? = keys.firstNotNullOfOrNull { key ->
                (obj[key] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
            }
            fun number(vararg keys: String): Int? = keys.firstNotNullOfOrNull { key ->
                (obj[key] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull?.toIntOrNull()
            }
            val id = text("id", "name", "model") ?: return@mapNotNull null
            val declared = parseDeclaredCapabilities(obj)
            val fromCatalog = if (declared == null) ModelCapabilityCatalog.lookup(id) else null

            ModelCandidate(
                id = id,
                displayName = text("display_name", "displayName", "name") ?: id,
                contextWindow = number("context_window", "contextWindow", "context_length", "max_context_tokens"),
                maxOutputTokens = number("max_output_tokens", "maxOutputTokens", "max_tokens"),
                modalities = declared?.modalities ?: fromCatalog?.modalities ?: listOf("text"),
                tools = declared?.tools ?: fromCatalog?.tools ?: false,
                reasoning = declared?.reasoning ?: fromCatalog?.reasoning ?: false,
                // 思考档位只可能来自内置目录：接口一般不下发这个信息。
                // 只有真的声明了推理能力才带档位 —— 否则保存时会被
                // validateThinkingEfforts 拒绝（"声明了思考等级却没勾支持推理"）。
                thinkingEfforts = if (declared?.reasoning ?: fromCatalog?.reasoning ?: false) {
                    fromCatalog?.thinkingEfforts ?: emptyMap()
                } else {
                    emptyMap()
                },
                thinkingFormat = if (declared?.reasoning ?: fromCatalog?.reasoning ?: false) {
                    fromCatalog?.thinkingFormat
                } else {
                    null
                },
                capabilitySource = when {
                    declared != null -> CapabilitySource.DISCOVERED.wire
                    fromCatalog != null -> CapabilitySource.BUILTIN.wire
                    else -> "unknown"
                },
                capabilityNote = when {
                    declared != null -> "接口在下发的模型信息里声明了输入模态"
                    fromCatalog != null -> "命中内置目录规则「${fromCatalog.matchedBy}」（可能过期，可手工修改）"
                    else -> "接口未声明、内置目录也没有 → 按仅文本处理，需要图片请手工勾选"
                },
            )
        }.distinctBy { it.id }
    }.getOrDefault(emptyList())

    private data class DeclaredCapabilities(
        val modalities: List<String>,
        val tools: Boolean,
        val reasoning: Boolean,
    )

    /**
     * 读取接口**自己声明**的能力。只要看到任何一个已知字段就认为"接口说了"，
     * 没看到的维度按 false / 无处理；一个字段都没有则返回 null（交给内置目录）。
     */
    private fun parseDeclaredCapabilities(obj: kotlinx.serialization.json.JsonObject): DeclaredCapabilities? {
        val modalities = linkedSetOf<String>()
        var sawAnything = false

        fun addModality(raw: String?) {
            val token = raw?.trim()?.lowercase() ?: return
            if (token.isEmpty()) return
            sawAnything = true
            // "text+image->text" / "image->text" 这类 OpenRouter 写法
            token.split('+', ',', '/', '|', ' ', '>')
                .map { it.trim() }
                .filter { it.isNotEmpty() }
                .forEach { part ->
                    when {
                        part.startsWith("image") || part == "vision" -> modalities += "image"
                        part.startsWith("video") -> modalities += "video"
                        part.startsWith("audio") -> modalities += "audio"
                        part.startsWith("file") || part.startsWith("pdf") -> modalities += "document"
                        part.startsWith("text") || part == "prompt" -> modalities += "text"
                    }
                }
        }

        fun readArrayOf(target: kotlinx.serialization.json.JsonObject, key: String) {
            val arr = target[key] as? kotlinx.serialization.json.JsonArray ?: return
            sawAnything = true
            arr.forEach { element ->
                when (element) {
                    is kotlinx.serialization.json.JsonPrimitive -> addModality(element.contentOrNull)
                    is kotlinx.serialization.json.JsonObject -> element.keys.forEach { addModality(it) }
                    else -> {}
                }
            }
        }

        fun readArray(key: String) = readArrayOf(obj, key)

        // 1) 数组形态：input_modalities / modalities / capabilities: ["vision","tools"]
        listOf("input_modalities", "inputModalities", "modalities", "supported_modalities", "capabilities")
            .forEach { readArray(it) }

        // 2) OpenRouter：architecture.input_modalities / architecture.modality
        (obj["architecture"] as? kotlinx.serialization.json.JsonObject)?.let { arch ->
            readArrayOf(arch, "input_modalities")
            addModality((arch["modality"] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull)
        }

        // 3) 布尔开关形态
        fun boolOf(vararg keys: String): Boolean? = keys.firstNotNullOfOrNull { key ->
            when (val v = obj[key]) {
                is kotlinx.serialization.json.JsonPrimitive -> v.contentOrNull?.toBooleanStrictOrNull()
                is kotlinx.serialization.json.JsonObject -> {
                    sawAnything = true
                    null
                }
                else -> null
            }
        }

        boolOf("supports_vision", "vision", "supports_image", "multimodal", "image_input")?.let {
            sawAnything = true
            if (it) modalities += "image"
        }
        val tools = boolOf("supports_tools", "function_calling", "tool_call", "tools") ?: false
        if (obj.keys.any { it in setOf("supports_tools", "function_calling", "tool_call", "tools") }) {
            sawAnything = true
        }
        // capabilities: {"vision": true, "function_calling": true}
        var toolsFromCaps = false
        (obj["capabilities"] as? kotlinx.serialization.json.JsonObject)?.let { caps ->
            sawAnything = true
            fun flag(key: String) =
                (caps[key] as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull?.toBooleanStrictOrNull()
            if (flag("vision") == true || flag("image") == true) modalities += "image"
            if (flag("function_calling") == true || flag("tools") == true || flag("tool_call") == true) {
                toolsFromCaps = true
            }
        }
        val reasoning = boolOf("supports_reasoning", "reasoning", "thinking", "supports_thinking") ?: false

        if (!sawAnything) return null
        if (modalities.isEmpty()) modalities += "text"
        return DeclaredCapabilities(modalities.toList(), tools || toolsFromCaps, reasoning)
    }

    // --- 内部：发一次 GET /models ------------------------------------------

    private data class UpstreamResponse(
        val ok: Boolean,
        val status: Int?,
        val errorCode: String?,
        val message: String,
        val body: String,
        /** 真正取到列表的那个地址（候选回退后可能不是 Base URL 直接拼出来的那个） */
        val usedUrl: String? = null,
    )

    /**
     * 取模型列表：候选地址依次试（`{base}/models` ↔ `{base}/v1/models`），
     * 第一个返回 2xx 的就算数。这样 Base URL 填不填 `/v1` 都能用。
     */
    private fun getModels(provider: AiProviderDto, secret: String?): UpstreamResponse {
        val api = AiApi.parse(provider.api)
            ?: return UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, "未知协议：${provider.api}", "")
        val trust = EndpointTrust.parse(provider.endpointTrust) ?: EndpointTrust.PUBLIC

        if (provider.credentialRef != null && secret.isNullOrEmpty()) {
            return UpstreamResponse(
                false, null, AiErrorCode.MISSING_CREDENTIAL,
                "该 Provider 引用了凭据 ${provider.credentialRef}，但本机没有配置", ""
            )
        }

        var last: UpstreamResponse? = null
        for (url in modelsUrlCandidates(provider.baseURL, api)) {
            val probe = httpGet(url, api, secret, trust, timeoutSeconds = 20, withBody = true)
            val attempt = UpstreamResponse(
                ok = probe.ok,
                status = probe.status,
                errorCode = probe.code.takeIf { !probe.ok },
                message = if (probe.ok) "OK" else probe.detail,
                body = probe.body.orEmpty(),
                usedUrl = url,
            )
            if (attempt.ok) return attempt
            // 鉴权失败换地址也没用，直接返回（别把"Key 不对"掩盖成"地址不对"）
            if (probe.code == AiErrorCode.MISSING_CREDENTIAL) return attempt
            last = attempt
        }
        return last ?: UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, "Base URL 无法解析", "")
    }
}
