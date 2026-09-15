package com.comfyhub.ai

import org.slf4j.LoggerFactory
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.time.Duration

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
}
