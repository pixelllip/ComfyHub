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
    fun classifyStatus(status: Int): String? = when {
        status in 200..299 -> null
        status == 401 || status == 403 -> AiErrorCode.MISSING_CREDENTIAL
        status == 404 -> AiErrorCode.PROTOCOL_ERROR
        status == 408 || status == 504 -> AiErrorCode.PROVIDER_UNREACHABLE
        status == 429 -> AiErrorCode.RATE_LIMIT
        status == 402 -> AiErrorCode.QUOTA_EXCEEDED
        status in 400..499 -> AiErrorCode.CONFIG_ERROR
        status >= 500 -> AiErrorCode.PROVIDER_UNREACHABLE
        else -> AiErrorCode.PROTOCOL_ERROR
    }

    /** 列表 URL：OpenAI 兼容拼 `/models`，Anthropic 只对列表归一化 `/v1`。 */
    fun modelsUrl(baseURL: String, api: AiApi): String {
        val base = baseURL.trimEnd('/')
        return when (api) {
            AiApi.ANTHROPIC_MESSAGES ->
                if (base.endsWith("/v1")) "$base/models" else "$base/v1/models"
            AiApi.OPENAI_COMPLETIONS, AiApi.OPENAI_RESPONSES -> "$base/models"
        }
    }

    fun testConnection(provider: AiProviderDto, secret: String?): TestResult {
        val api = AiApi.parse(provider.api)
            ?: return TestResult(false, AiErrorCode.CONFIG_ERROR, "未知协议：${provider.api}")
        val trust = EndpointTrust.parse(provider.endpointTrust) ?: EndpointTrust.PUBLIC

        val uri = runCatching { URI(modelsUrl(provider.baseURL, api)) }.getOrNull()
            ?: return TestResult(false, AiErrorCode.CONFIG_ERROR, "Base URL 无法解析")
        val host = uri.host ?: return TestResult(false, AiErrorCode.CONFIG_ERROR, "Base URL 缺少主机名")

        // 先查凭据（便宜、不需要网络）：缺密钥时应该报得比"地址不可信"更准
        if (provider.credentialRef != null && secret.isNullOrEmpty()) {
            return TestResult(false, AiErrorCode.MISSING_CREDENTIAL, "该 Provider 引用了凭据 ${provider.credentialRef}，但本机没有配置")
        }

        // SSRF 复核：保存时已经查过一次，这里按真实解析结果再查一次
        runCatching { EndpointGuard.resolveAndCheck(host, trust) }.getOrElse { e ->
            return TestResult(false, AiErrorCode.CONFIG_ERROR, e.message ?: "目标地址不被信任级别允许")
        }

        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(10))
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
            val code = classifyStatus(status)
            // 只记状态与错误码，绝不记 header 或响应正文
            log.info("Provider 连接测试 provider={} endpoint={} status={} code={}", provider.id, safeEndpoint(uri), status, code)
            if (code == null) {
                TestResult(true, null, "连接正常（HTTP $status）", status, countModels(response.body(), api))
            } else {
                TestResult(false, code, explain(code, status), status)
            }
        } catch (e: java.net.http.HttpTimeoutException) {
            log.warn("Provider 连接超时 provider={} endpoint={}", provider.id, safeEndpoint(uri))
            TestResult(false, AiErrorCode.PROVIDER_UNREACHABLE, "连接超时（10 秒）")
        } catch (e: Exception) {
            log.warn("Provider 连接失败 provider={} endpoint={} reason={}", provider.id, safeEndpoint(uri), e::class.simpleName)
            TestResult(false, AiErrorCode.PROVIDER_UNREACHABLE, "无法连接：${e::class.simpleName}")
        }
    }

    /** 日志里只出现 scheme://host:port/path，不带 query / fragment / userInfo。 */
    private fun safeEndpoint(uri: URI): String = buildString {
        append(uri.scheme).append("://").append(uri.host)
        if (uri.port > 0) append(':').append(uri.port)
        append(uri.path ?: "")
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
        AiErrorCode.PROTOCOL_ERROR -> "端点不存在或协议不匹配（HTTP $status）：确认 Base URL 与所选协议"
        AiErrorCode.RATE_LIMIT -> "被限流（HTTP 429）"
        AiErrorCode.QUOTA_EXCEEDED -> "配额不足（HTTP $status）"
        AiErrorCode.PROVIDER_UNREACHABLE -> "上游不可达或返回服务端错误（HTTP $status）"
        else -> "配置有误（HTTP $status）"
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
    )

    private fun getModels(provider: AiProviderDto, secret: String?): UpstreamResponse {
        val api = AiApi.parse(provider.api)
            ?: return UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, "未知协议：${provider.api}", "")
        val trust = EndpointTrust.parse(provider.endpointTrust) ?: EndpointTrust.PUBLIC
        val uri = runCatching { URI(modelsUrl(provider.baseURL, api)) }.getOrNull()
            ?: return UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, "Base URL 无法解析", "")
        val host = uri.host
            ?: return UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, "Base URL 缺少主机名", "")

        if (provider.credentialRef != null && secret.isNullOrEmpty()) {
            return UpstreamResponse(
                false, null, AiErrorCode.MISSING_CREDENTIAL,
                "该 Provider 引用了凭据 ${provider.credentialRef}，但本机没有配置", ""
            )
        }
        runCatching { EndpointGuard.resolveAndCheck(host, trust) }.getOrElse { e ->
            return UpstreamResponse(false, null, AiErrorCode.CONFIG_ERROR, e.message ?: "目标地址不被允许", "")
        }

        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(20))
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
            val code = classifyStatus(response.statusCode())
            if (code == null) {
                UpstreamResponse(true, response.statusCode(), null, "OK", response.body())
            } else {
                UpstreamResponse(
                    false, response.statusCode(), code,
                    explain(code, response.statusCode()), ""
                )
            }
        } catch (e: java.net.http.HttpTimeoutException) {
            UpstreamResponse(false, null, AiErrorCode.PROVIDER_UNREACHABLE, "连接超时（20 秒）", "")
        } catch (e: Exception) {
            UpstreamResponse(false, null, AiErrorCode.PROVIDER_UNREACHABLE, "无法连接：${e::class.simpleName}", "")
        }
    }
}
