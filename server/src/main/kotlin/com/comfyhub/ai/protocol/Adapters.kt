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
 * 与 Chat Completions 的差别是结构性的，不是改个字段名：
 *
 * | | Chat Completions | Responses |
 * | --- | --- | --- |
 * | 路径 | `/chat/completions` | `/responses` |
 * | 系统提示词 | `messages[0].role=system` | 顶层 `instructions` |
 * | 输入 | `messages[{role,content:String}]` | `input[{role,content:[{type:input_text,text}]}]` |
 * | 流事件 | `choices[].delta.content` | `response.output_text.delta` |
 * | 用量 | 最后一个 chunk 的 `usage` | `response.completed.response.usage` |
 *
 * 只实现文本流（附件传输仍为空集，预检据此阻断，AIH-028）。
 */
class OpenAiResponsesAdapter : ProtocolAdapter {
    override val api = AiApiRef.OPENAI_RESPONSES
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "openai-responses/1"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
    ): JsonObject {
        // system 提示词在 Responses 里是顶层 instructions，不能留在 input 里
        val instructions = messages.filter { it.role == "system" }
            .joinToString("\n\n") { it.content }
        val turns = messages.filter { it.role != "system" }

        return buildJsonObject {
            put("model", model)
            if (instructions.isNotBlank()) put("instructions", instructions)
            put(
                "input",
                buildJsonArray {
                    turns.forEach { turn ->
                        val assistant = turn.role == "assistant"
                        add(
                            buildJsonObject {
                                put("role", if (assistant) "assistant" else "user")
                                put(
                                    "content",
                                    buildJsonArray {
                                        add(
                                            buildJsonObject {
                                                // 助手历史用 output_text，用户输入用 input_text
                                                put("type", if (assistant) "output_text" else "input_text")
                                                put("text", turn.content)
                                            }
                                        )
                                    }
                                )
                            }
                        )
                    }
                }
            )
            put("stream", stream)
            // 不做服务端存储：和 Chat Completions 的行为对齐，避免把用户内容留在上游
            put("store", false)
            applyReasoning(reasoning)
        }
    }

    /**
     * Responses 的思考字段是 `reasoning: {effort, summary}`。
     *
     * 两点与 Chat Completions 不同：
     *  1. **只有 `reasoning.effort` 一种写法**，没有 deepseek/qwen/zai 那些方言字段 ——
     *     那些网关在 Responses 协议下同样认这个字段，所以这里不需要 `thinkingFormat` 分支；
     *  2. `effort` 只接受 minimal/low/medium/high（Responses 早期只有 low/medium/high，
     *     后来加了 minimal）。我们的 `MAX` 落成 `high`，`OFF` 就**不带**这个字段。
     *
     * 另外开 `summary: "auto"`：不带摘要时多数网关不会下发思考过程，
     * 聊天框里的"思考中"就会一直空着。
     */
    private fun JsonObjectBuilder.applyReasoning(r: ReasoningRequest) {
        if (r.effort == null || r.wireValue == null) return
        val effort = when (r.wireValue) {
            "max", "xhigh" -> "high"
            "minimal" -> "minimal"
            "low", "medium", "high" -> r.wireValue
            else -> "medium"
        }
        put(
            "reasoning",
            buildJsonObject {
                put("effort", effort)
                put("summary", "auto")
            }
        )
    }

    override fun interpret(frame: SseAccumulator.Frame): List<StreamEvent> {
        val data = frame.data.trim()
        if (data.isEmpty() || data == "[DONE]") return emptyList()
        val root = runCatching { ProtocolJson.parseToJsonElement(data) }.getOrNull() ?: return emptyList()
        val out = mutableListOf<StreamEvent>()
        // 事件类型：有的网关发 SSE `event:` 行，有的只在 body 的 type 里
        val type = frame.event ?: root.str("type")

        when {
            // 正文增量
            type == "response.output_text.delta" -> {
                root.str("delta")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.TextDelta(it) }
            }
            // 思考过程（推理摘要 / 推理正文，两种事件名都见过）
            type == "response.reasoning_summary_text.delta" ||
                type == "response.reasoning_text.delta" -> {
                root.str("delta")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.ReasoningDelta(it) }
            }
            // 流结束：用量与上游响应 id 都在这里
            type == "response.completed" || type == "response.incomplete" -> {
                val response = root.raw("response") ?: root
                response.str("id")?.let { out += StreamEvent.ProviderId(it) }
                response.raw("usage")?.let { out += StreamEvent.Usage(it) }
            }
            // 单条消息创建（拿 id 用）
            type == "response.created" || type == "response.in_progress" -> {
                root.raw("response")?.str("id")?.let { out += StreamEvent.ProviderId(it) }
            }
            // 明确的失败事件
            type == "response.failed" || type == "error" -> {
                val message = root.raw("response").str("error", "message")
                    ?: root.raw("error").str("message")
                    ?: root.str("message")
                    ?: "上游返回错误事件"
                throw ProtocolFailure(message)
            }
        }
        return out
    }
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
