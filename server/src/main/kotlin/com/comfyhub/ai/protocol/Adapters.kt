package com.comfyhub.ai.protocol

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * OpenAI 兼容 Chat Completions（AIH-003）。
 *
 * 覆盖 OpenAI 官方以及绝大多数兼容网关（DeepSeek / Moonshot / vLLM / LM Studio / Ollama 的
 * OpenAI 端点等），是首期唯一"真的能聊"的协议。
 *
 * 只实现文本流；**图片等附件暂不支持**，所以 [transports] 是空集 —— 预检据此阻断，
 * 而不是让请求带着前端声明直接发出去（AIH-028）。
 */
class OpenAiCompletionsAdapter : ProtocolAdapter {
    override val api = AiApiRef.OPENAI_COMPLETIONS
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "openai-completions/1"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
    ): JsonObject =
        buildJsonObject {
            put("model", model)
            put(
                "messages",
                buildJsonArray {
                    messages.forEach { turn ->
                        add(
                            buildJsonObject {
                                put("role", turn.role)
                                put("content", turn.content)
                            }
                        )
                    }
                }
            )
            put("stream", stream)
            if (stream) {
                // 让上游在最后一个 chunk 里带上 usage（OpenAI 兼容网关不一定支持，忽略即可）
                put("stream_options", buildJsonObject { put("include_usage", true) })
            }
            applyReasoning(reasoning)
        }

    // 思考强度按方言落到不同字段（AIH-056）。关闭思考时**分两种**：`openai` 方言什么都不发
    // （最安全，很多网关不认 `reasoning_effort:"none"`），而 `deepseek`/`zai`/`openrouter` 方言
    // 必须显式发"禁用"，否则网关会默认开思考。
    private fun JsonObjectBuilder.applyReasoning(r: ReasoningRequest) {
        if (r.effort != null && r.wireValue != null) {
            when (r.format) {
                ThinkingFormat.DEEPSEEK -> {
                    put("thinking", buildJsonObject { put("type", "enabled") })
                    put("reasoning_effort", r.wireValue)
                }
                ThinkingFormat.QWEN -> {
                    put("enable_thinking", true)
                    put("reasoning_effort", r.wireValue)
                }
                ThinkingFormat.ZAI -> {
                    put("thinking", buildJsonObject { put("type", "enabled"); put("clear_thinking", false) })
                    put("reasoning_effort", r.wireValue)
                }
                ThinkingFormat.OPENROUTER -> {
                    put("reasoning", buildJsonObject { put("effort", r.wireValue) })
                }
                ThinkingFormat.OPENAI -> put("reasoning_effort", r.wireValue)
            }
            return
        }
        // 关闭：只有"默认会思考"的方言才需要显式关闭
        when (r.format) {
            ThinkingFormat.DEEPSEEK -> put("thinking", buildJsonObject { put("type", "disabled") })
            ThinkingFormat.ZAI -> put("thinking", buildJsonObject { put("type", "disabled") })
            ThinkingFormat.OPENROUTER -> put("reasoning", buildJsonObject { put("effort", "none") })
            else -> Unit
        }
    }

    override fun interpret(frame: SseAccumulator.Frame): List<StreamEvent> {
        val data = frame.data.trim()
        if (data.isEmpty()) return emptyList()
        if (data == "[DONE]") return emptyList()

        val root = runCatching { ProtocolJson.parseToJsonElement(data) }.getOrNull() ?: return emptyList()
        val out = mutableListOf<StreamEvent>()

        root.raw("usage")?.let { out += StreamEvent.Usage(it) }
        root.str("id")?.let { out += StreamEvent.ProviderId(it) }

        root.objArray("choices").forEach { choice ->
            // 兼容两种方言：delta（流式）与 message（个别网关在流里回完整消息）
            val delta = choice["delta"] ?: choice["message"]
            val content = delta.str("content")
            if (!content.isNullOrEmpty()) out += StreamEvent.TextDelta(content)
            val reasoning = delta.str("reasoning_content") ?: delta.str("reasoning")
            if (!reasoning.isNullOrEmpty()) out += StreamEvent.ReasoningDelta(reasoning)
        }
        return out
    }
}

/**
 * Anthropic Messages 兼容协议（AIH-005）。
 *
 * 请求体与 OpenAI 不同（system 独立字段、必须给 max_tokens），SSE 事件类型也不同
 * （`content_block_delta` 的 `delta.text`）。这里实现文本流，附件同样暂不支持。
 */
class AnthropicMessagesAdapter : ProtocolAdapter {
    override val api = AiApiRef.ANTHROPIC_MESSAGES
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "anthropic-messages/1"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
    ): JsonObject {
        // Anthropic 的 system 是顶层字段，不能混在 messages 里
        val system = messages.filter { it.role == "system" }.joinToString("\n\n") { it.content }
        val turns = messages.filter { it.role != "system" }
        // 开启思考时 max_tokens 必须严格大于思考预算，否则上游直接 400
        val budget = if (reasoning.enabled) {
            (reasoning.budgetTokens ?: ThinkingLevels.ANTHROPIC_BUDGET.getValue(ReasoningEffort.MEDIUM))
                .coerceAtLeast(ThinkingLevels.ANTHROPIC_MIN_BUDGET)
        } else 0
        return buildJsonObject {
            put("model", model)
            put("max_tokens", if (budget > 0) budget + 4096 else 4096)
            if (system.isNotBlank()) put("system", system)
            put(
                "messages",
                buildJsonArray {
                    turns.forEach { turn ->
                        add(
                            buildJsonObject {
                                put("role", if (turn.role == "assistant") "assistant" else "user")
                                put("content", turn.content)
                            }
                        )
                    }
                }
            )
            put("stream", stream)
            if (budget > 0) {
                put(
                    "thinking",
                    buildJsonObject {
                        put("type", "enabled")
                        put("budget_tokens", budget)
                    }
                )
            }
        }
    }

    override fun interpret(frame: SseAccumulator.Frame): List<StreamEvent> {
        val data = frame.data.trim()
        if (data.isEmpty() || data == "[DONE]") return emptyList()
        val root = runCatching { ProtocolJson.parseToJsonElement(data) }.getOrNull() ?: return emptyList()
        val out = mutableListOf<StreamEvent>()
        val type = frame.event ?: root.str("type")

        when (type) {
            "message_start" -> root.raw("message")?.str("id")?.let { out += StreamEvent.ProviderId(it) }
            "content_block_delta" -> {
                val delta = root.raw("delta")
                delta.str("text")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.TextDelta(it) }
                delta.str("thinking")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.ReasoningDelta(it) }
            }
            "message_delta" -> root.raw("usage")?.let { out += StreamEvent.Usage(it) }
            "error" -> {
                val message = root.raw("error").str("message") ?: "上游返回错误事件"
                throw ProtocolFailure(message)
            }
        }
        return out
    }
}

/** 上游流里明确报错（区别于网络层异常）。 */
class ProtocolFailure(message: String) : RuntimeException(message)

/**
 * OpenAI Responses 协议（AIH-004）。
 *
 * 首期尚未实现：请求/事件结构与 Chat Completions 差异较大（`output_text.delta` 等），
 * 与其半吊子实现，不如**明确拒绝**并让用户改用 openai-completions。
 */
class OpenAiResponsesAdapter : ProtocolAdapter {
    override val api = AiApiRef.OPENAI_RESPONSES
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "openai-responses/0"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
    ): JsonObject =
        throw ProtocolFailure("openai-responses 协议尚未实现，请先改用 openai-completions")

    override fun interpret(frame: SseAccumulator.Frame): List<StreamEvent> = emptyList()
}

object Adapters {
    private val all: Map<AiApiRef, ProtocolAdapter> = listOf(
        OpenAiCompletionsAdapter(),
        AnthropicMessagesAdapter(),
        OpenAiResponsesAdapter(),
    ).associateBy { it.api }

    fun of(api: AiApiRef): ProtocolAdapter? = all[api]

    fun supported(): List<AiApiRef> = all.filterValues { it.adapterVersion.endsWith("/1") }.keys.toList()
}

/** 供 `JsonPrimitive` 之外的判空使用。 */
internal fun JsonPrimitive?.orNull(): String? = this?.contentOrNullValue()

private fun JsonPrimitive.contentOrNullValue(): String? = runCatching { content }.getOrNull()
