package com.comfyhub.harness.agent

import com.comfyhub.harness.net.HttpFailure
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.protocol.AiApiRef
import com.comfyhub.harness.protocol.ProtocolAdapter
import com.comfyhub.harness.protocol.ProtocolFailure
import com.comfyhub.harness.protocol.SseAccumulator
import com.comfyhub.harness.protocol.StreamEvent
import com.comfyhub.harness.protocol.StreamOutcome
import com.comfyhub.harness.protocol.ToolCallAccumulator
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.contentOrNull

/**
 * 一次上游调用的稳定错误码（移植自桌面 `AiErrorCode` 的子集 + `AiUpstream.classifyStatus`）。
 *
 * 为什么要稳定错误码：前端与模型都**不去解析供应商文案**（文案千奇百怪、还会带上配额信息）。
 */
object UpstreamErrorCode {
    const val MISSING_CREDENTIAL = "MISSING_CREDENTIAL"
    const val RATE_LIMIT = "RATE_LIMIT"
    const val QUOTA_EXCEEDED = "QUOTA_EXCEEDED"
    const val CONFIG_ERROR = "CONFIG_ERROR"
    const val PROVIDER_UNREACHABLE = "PROVIDER_UNREACHABLE"
    const val PROTOCOL_ERROR = "PROTOCOL_ERROR"
}

class UpstreamFailure(val code: String, message: String) : RuntimeException(message)

/**
 * 上游流式调用（移植自桌面 `HarnessRunner` 里那段 `streamUpstream` + `AiUpstream` 的两个纯函数）。
 *
 * 四条硬规则与桌面一致：
 *  1. **绝不记录 Authorization / x-api-key**（本类不写日志，异常里也不带请求头）；
 *  2. 不跟随重定向（由 `HttpTransport` 的实现保证）；
 *  3. 状态码归到**稳定错误码**；
 *  4. 畸形 SSE 帧**不许把异常漏给上层**（适配器内部 `runCatching`）。
 */
class UpstreamClient(private val transport: HttpTransport) {

    /** 一轮流式调用的累积结果。 */
    data class RoundStream(
        val outcome: StreamOutcome,
        val toolCalls: ToolCallAccumulator,
        val idle: Boolean,
    )

    /**
     * 发一次流式请求。
     *
     * @param onEvent 每个增量/用量/id 事件都会回调（`text.delta` 的落库与推送由调用方决定）
     * @return 这一轮累积出来的正文/思考/结束原因/用量 + 工具调用分片
     */
    fun stream(
        baseUrl: String,
        api: AiApiRef,
        secret: String?,
        body: String,
        adapter: ProtocolAdapter,
        onEvent: (StreamEvent) -> Unit,
    ): RoundStream {
        val text = StringBuilder()
        val reasoning = StringBuilder()
        val toolCalls = ToolCallAccumulator()
        var finishReason: String? = null
        var usage: JsonElement? = null
        var providerId: String? = null
        // 每次调用一个新状态机（它是有状态的：多行 data 要拼、空行才出 frame）
        val sse = SseAccumulator()
        var sawAnything = false

        fun handleFrame(frame: SseAccumulator.Frame?) {
            if (frame == null) return
            adapter.interpret(frame).forEach { event ->
                sawAnything = true
                when (event) {
                    is StreamEvent.TextDelta -> text.append(event.text)
                    is StreamEvent.ReasoningDelta -> reasoning.append(event.text)
                    is StreamEvent.Usage -> usage = event.usage
                    is StreamEvent.ProviderId -> providerId = event.id
                    is StreamEvent.ToolCallDelta -> toolCalls.apply(event)
                }
                onEvent(event)
            }
            // finish_reason 只在 choices[].finish_reason 里，适配器刻意不透传它 → 从原始 data 里取
            extractFinishReason(frame.data)?.let { finishReason = it }
        }

        // OpenAI 兼容网关的 Base URL 填不填 /v1 是两个不同地址，候选依次试（与桌面同一条规则）
        val candidates = urlCandidates(baseUrl, api)
        var lastFailure: UpstreamFailure? = null
        for (url in candidates) {
            try {
                transport.postJsonStreaming(url, body, headers(api, secret)) { line ->
                    handleFrame(sse.line(line))
                }
                // 流结束时收尾：最后一段可能没有空行收尾
                handleFrame(sse.flush())
                return RoundStream(
                    outcome = StreamOutcome(
                        text = text.toString(),
                        reasoning = reasoning.toString().takeIf { it.isNotEmpty() },
                        finishReason = finishReason,
                        usage = usage,
                        providerResponseId = providerId,
                    ),
                    toolCalls = toolCalls,
                    // "一个事件都没收到" = 空跑（网关 200 但流是空的），调用方据此决定要不要报错
                    idle = !sawAnything,
                )
            } catch (e: HttpFailure) {
                // 404 说明这个候选地址拼得不对，换下一个；其余错误直接判定（别把 401 掩盖成 404）
                if (e.status == 404 && url != candidates.last()) {
                    lastFailure = UpstreamFailure(UpstreamErrorCode.PROTOCOL_ERROR, e.message.orEmpty())
                    continue
                }
                throw UpstreamFailure(classify(e.status), explain(classify(e.status), e.status) + "｜" + e.message)
            } catch (e: ProtocolFailure) {
                throw UpstreamFailure(UpstreamErrorCode.PROTOCOL_ERROR, "上游返回了错误事件：${e.message}")
            }
        }
        throw lastFailure ?: UpstreamFailure(UpstreamErrorCode.CONFIG_ERROR, "Base URL 无法解析")
    }

    /**
     * 状态码 → 稳定错误码（纯函数，与桌面 `AiUpstream.classifyStatus` 同表）。
     *
     * ⚠ `null` 状态码 = 网络层异常（连不上 / 超时）→ `PROVIDER_UNREACHABLE`。
     */
    fun classify(status: Int?): String = when {
        status == null -> UpstreamErrorCode.PROVIDER_UNREACHABLE
        status == 401 || status == 403 -> UpstreamErrorCode.MISSING_CREDENTIAL
        status == 404 -> UpstreamErrorCode.PROTOCOL_ERROR
        status == 408 || status == 504 -> UpstreamErrorCode.PROVIDER_UNREACHABLE
        status == 429 -> UpstreamErrorCode.RATE_LIMIT
        status == 402 -> UpstreamErrorCode.QUOTA_EXCEEDED
        status in 400..499 -> UpstreamErrorCode.CONFIG_ERROR
        status >= 500 -> UpstreamErrorCode.PROVIDER_UNREACHABLE
        else -> UpstreamErrorCode.PROTOCOL_ERROR
    }

    fun explain(code: String, status: Int?): String = when (code) {
        UpstreamErrorCode.MISSING_CREDENTIAL -> "鉴权失败（HTTP $status）：检查 API Key 是否正确、是否已过期"
        UpstreamErrorCode.RATE_LIMIT -> "被限流（HTTP 429）"
        UpstreamErrorCode.QUOTA_EXCEEDED -> "配额/套餐不足（HTTP $status）"
        UpstreamErrorCode.PROVIDER_UNREACHABLE -> "上游不可达或返回服务端错误（HTTP $status）"
        UpstreamErrorCode.PROTOCOL_ERROR -> "端点或协议不匹配（HTTP $status）"
        else -> "配置有误（HTTP $status）"
    }

    companion object {
        /** 请求头（**只在这里拼一次**：密钥不进日志、不进异常）。 */
        fun headers(api: AiApiRef, secret: String?): Map<String, String> {
            val base = mutableMapOf("Accept" to "text/event-stream")
            when (api) {
                AiApiRef.OPENAI_COMPLETIONS, AiApiRef.OPENAI_RESPONSES ->
                    if (!secret.isNullOrEmpty()) base["Authorization"] = "Bearer $secret"
                AiApiRef.ANTHROPIC_MESSAGES -> {
                    if (!secret.isNullOrEmpty()) base["x-api-key"] = secret
                    base["anthropic-version"] = "2023-06-01"
                }
            }
            return base
        }

        /**
         * chat / responses / messages 端点的候选地址（Base URL 带不带 `/v1` 两种写法都试）。
         *
         * 与桌面 `AiUpstream.chatUrlCandidates` 同一条规则：同一个 Key，Base URL 填
         * `https://api.x.com` 与 `https://api.x.com/v1` 是两个不同的 URL，
         * 前者拼出来 404，用户看到的却是"连接失败"。
         */
        fun urlCandidates(baseUrl: String, api: AiApiRef): List<String> {
            val base = baseUrl.trimEnd('/')
            val stripped = if (base.endsWith("/v1")) base.dropLast(3) else "$base/v1"
            val suffix = when (api) {
                AiApiRef.OPENAI_COMPLETIONS -> "/chat/completions"
                AiApiRef.OPENAI_RESPONSES -> "/responses"
                AiApiRef.ANTHROPIC_MESSAGES -> "/messages"
            }
            return listOf("$base$suffix", "$stripped$suffix").distinct()
        }

        /** 从原始 SSE data 里取 `choices[].finish_reason`（适配器刻意不透传它）。 */
        fun extractFinishReason(data: String): String? {
            val trimmed = data.trim()
            if (trimmed.isEmpty() || trimmed == "[DONE]") return null
            val root = runCatching {
                com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(trimmed)
            }.getOrNull() as? kotlinx.serialization.json.JsonObject ?: return null
            val choices = (root["choices"] as? kotlinx.serialization.json.JsonArray) ?: return null
            choices.forEach { el ->
                val reason = ((el as? kotlinx.serialization.json.JsonObject)?.get("finish_reason")
                    as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull
                if (!reason.isNullOrBlank()) return reason
            }
            // Responses 协议的结束原因在 response.status / incomplete_details
            val response = root["response"] as? kotlinx.serialization.json.JsonObject
            val status = (response?.get("status") as? kotlinx.serialization.json.JsonPrimitive)?.contentOrNull
            if (status == "incomplete") return "max_output_tokens"
            return null
        }
    }
}
