package com.comfyhub.ai.protocol

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
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
 *
 * M4 起支持工具调用：`tools[].function` 下发定义，`delta.tool_calls[]` 收分片。
 */
class OpenAiCompletionsAdapter : ProtocolAdapter {
    override val api = AiApiRef.OPENAI_COMPLETIONS
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "openai-completions/2"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
        tools: List<ToolSpec>,
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
                                when {
                                    // 工具结果：role=tool + tool_call_id
                                    turn.role == "tool" -> {
                                        put("tool_call_id", turn.toolCallId.orEmpty())
                                        put("content", turn.content)
                                    }
                                    // 助手发起工具调用：content 用空串（比 null 更广的兼容性），
                                    // 再带 tool_calls
                                    turn.toolCalls.isNotEmpty() -> {
                                        put("content", turn.content)
                                        put("tool_calls", buildJsonArray {
                                            turn.toolCalls.forEach { call ->
                                                add(
                                                    buildJsonObject {
                                                        put("id", call.id)
                                                        put("type", "function")
                                                        put(
                                                            "function",
                                                            buildJsonObject {
                                                                put("name", call.name)
                                                                put("arguments", call.arguments)
                                                            },
                                                        )
                                                    }
                                                )
                                            }
                                        })
                                    }
                                    else -> put("content", turn.content)
                                }
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
            if (tools.isNotEmpty()) put("tools", openAiTools(tools))
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

            // 工具调用分片：第一片带 id/name，之后只有 index + arguments 片段
            delta.objArray("tool_calls").forEachIndexed { fallbackIndex, call ->
                val index = call.int("index") ?: fallbackIndex
                val fn = call.raw("function")
                out += StreamEvent.ToolCallDelta(
                    index = index,
                    key = call.str("id"),
                    callId = call.str("id"),
                    name = fn.str("name"),
                    argumentsFragment = fn.str("arguments"),
                )
            }
        }
        return out
    }
}

/**
 * Anthropic Messages 兼容协议（AIH-005）。
 *
 * 请求体与 OpenAI 不同（system 独立字段、必须给 max_tokens），SSE 事件类型也不同
 * （`content_block_delta` 的 `delta.text`）。这里实现文本流，附件同样暂不支持。
 *
 * 工具调用用 Anthropic 自己的结构：助手 `content` 里是 `tool_use` 块，工具结果必须回在
 * **紧随其后的 user 消息**里、用 `tool_result` 块（`tool_use_id` 对应）—— 所以这里会把
 * 连续的 `role=tool` 轮**合并成一条 user 消息**，否则上游会因为"连续两条 user 消息"而报错。
 */
class AnthropicMessagesAdapter : ProtocolAdapter {
    override val api = AiApiRef.ANTHROPIC_MESSAGES
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "anthropic-messages/2"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
        tools: List<ToolSpec>,
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
            put("messages", anthropicMessages(turns))
            put("stream", stream)
            if (tools.isNotEmpty()) put("tools", anthropicTools(tools))
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

    private fun anthropicMessages(turns: List<ChatTurn>): JsonArray = buildJsonArray {
        var i = 0
        while (i < turns.size) {
            val turn = turns[i]
            if (turn.role == "tool") {
                // 连续的 tool_result 合并成一条 user 消息（Anthropic 要求 tool_result 紧跟 tool_use）
                val blocks = buildJsonArray {
                    while (i < turns.size && turns[i].role == "tool") {
                        val t = turns[i]
                        add(
                            buildJsonObject {
                                put("type", "tool_result")
                                put("tool_use_id", t.toolCallId.orEmpty())
                                put("content", t.content)
                            }
                        )
                        i++
                    }
                }
                add(buildJsonObject { put("role", "user"); put("content", blocks) })
                continue
            }
            val assistant = turn.role == "assistant"
            if (assistant && turn.toolCalls.isNotEmpty()) {
                val blocks = buildJsonArray {
                    if (turn.content.isNotEmpty()) {
                        add(buildJsonObject { put("type", "text"); put("text", turn.content) })
                    }
                    turn.toolCalls.forEach { call ->
                        add(
                            buildJsonObject {
                                put("type", "tool_use")
                                put("id", call.id)
                                put("name", call.name)
                                put("input", parseArguments(call.arguments))
                            }
                        )
                    }
                }
                add(buildJsonObject { put("role", "assistant"); put("content", blocks) })
            } else {
                add(
                    buildJsonObject {
                        put("role", if (assistant) "assistant" else "user")
                        put("content", turn.content)
                    }
                )
            }
            i++
        }
    }

    override fun interpret(frame: SseAccumulator.Frame): List<StreamEvent> {
        val data = frame.data.trim()
        if (data.isEmpty() || data == "[DONE]") return emptyList()
        val root = runCatching { ProtocolJson.parseToJsonElement(data) }.getOrNull() ?: return emptyList()
        val out = mutableListOf<StreamEvent>()
        val type = frame.event ?: root.str("type")
        val blockIndex = root.int("index") ?: 0

        when (type) {
            "message_start" -> root.raw("message")?.str("id")?.let { out += StreamEvent.ProviderId(it) }
            "content_block_start" -> {
                val block = root.raw("content_block")
                if (block.str("type") == "tool_use") {
                    out += StreamEvent.ToolCallDelta(
                        index = blockIndex,
                        key = block.str("id"),
                        callId = block.str("id"),
                        name = block.str("name"),
                        // 有的网关在 start 里就把整个 input 给全了
                        argumentsFragment = block.raw("input")?.let { input ->
                            if (input is JsonObject && input.isNotEmpty()) input.toString() else null
                        },
                    )
                }
            }
            "content_block_delta" -> {
                val delta = root.raw("delta")
                delta.str("text")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.TextDelta(it) }
                delta.str("thinking")?.takeIf { it.isNotEmpty() }?.let { out += StreamEvent.ReasoningDelta(it) }
                // 工具参数是 partial_json 片段
                delta.str("partial_json")?.let { fragment ->
                    out += StreamEvent.ToolCallDelta(
                        index = blockIndex,
                        argumentsFragment = fragment,
                    )
                }
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
 *
 * 工具调用是**顶层 item**（不是消息里的字段）：`function_call` 与 `function_call_output`
 * 各自作为 `input[]` 的一项，用 `call_id` 关联。
 */
class OpenAiResponsesAdapter : ProtocolAdapter {
    override val api = AiApiRef.OPENAI_RESPONSES
    override val transports: Set<TransportRef> = emptySet()
    override val adapterVersion = "openai-responses/2"

    override fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest,
        tools: List<ToolSpec>,
    ): JsonObject {
        // system 提示词在 Responses 里是顶层 instructions，不能留在 input 里
        val instructions = messages.filter { it.role == "system" }
            .joinToString("\n\n") { it.content }
        val turns = messages.filter { it.role != "system" }

        return buildJsonObject {
            put("model", model)
            if (instructions.isNotBlank()) put("instructions", instructions)
            put("input", responsesInput(turns))
            put("stream", stream)
            // 不做服务端存储：和 Chat Completions 的行为对齐，避免把用户内容留在上游
            put("store", false)
            if (tools.isNotEmpty()) put("tools", responsesTools(tools))
            applyReasoning(reasoning)
        }
    }

    private fun responsesInput(turns: List<ChatTurn>): JsonArray = buildJsonArray {
        turns.forEach { turn ->
            if (turn.role == "tool") {
                add(
                    buildJsonObject {
                        put("type", "function_call_output")
                        put("call_id", turn.toolCallId.orEmpty())
                        put("output", turn.content)
                    }
                )
                return@forEach
            }
            val assistant = turn.role == "assistant"
            if (turn.content.isNotEmpty()) {
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
            turn.toolCalls.forEach { call ->
                add(
                    buildJsonObject {
                        put("type", "function_call")
                        put("call_id", call.id)
                        put("name", call.name)
                        put("arguments", call.arguments)
                    }
                )
            }
        }
    }

    /**
     * Responses 的思考字段是 `reasoning: {effort, summary}`。
     *
     * 两点与 Chat Completions 不同：
     *  1. **只有 `reasoning.effort` 一种写法**，没有 deepseek/qwen/zai 那些方言字段 ——
     *     那些网关在 Responses 协议下同样认这个字段，所以这里不需要 `thinkingFormat` 分支；
     *  2. 官方文档里 `effort` 的可选值是 `minimal / low / medium / high`（不同模型还会多出
     *     `xhigh` / `max`）。这里**显式映射**而不是 `else -> medium`：写错时宁可让上游
     *     明确拒绝，也不要偷偷把用户的"最大"降成"中"。
     */
    private fun JsonObjectBuilder.applyReasoning(r: ReasoningRequest) {
        if (r.effort == null || r.wireValue == null) return
        val effort = when (r.wireValue) {
            "minimal" -> "minimal"
            "low" -> "low"
            "medium" -> "medium"
            "high" -> "high"
            "xhigh" -> "xhigh"
            // 只有"用户确实声明过 max"才会走到这里；官方若不支持会 400 —— 那也比降级好
            "max" -> "max"
            else -> r.wireValue
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
            // 工具调用出现：item.id 是流内关联键，item.call_id 才是要回传的 id
            type == "response.output_item.added" || type == "response.output_item.done" -> {
                val item = root.raw("item") ?: return emptyList()
                if (item.str("type") == "function_call") {
                    out += StreamEvent.ToolCallDelta(
                        index = root.int("output_index") ?: 0,
                        key = item.str("id"),
                        callId = item.str("call_id"),
                        name = item.str("name"),
                        argumentsFragment = item.str("arguments")?.takeIf { it.isNotEmpty() },
                    )
                }
            }
            // 工具参数分片：只给 item_id + delta
            type == "response.function_call_arguments.delta" -> {
                root.str("delta")?.let {
                    out += StreamEvent.ToolCallDelta(
                        index = root.int("output_index") ?: 0,
                        key = root.str("item_id"),
                        argumentsFragment = it,
                    )
                }
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

    /**
     * 真正实现了完整对话的协议。
     *
     * 早期这里用 `adapterVersion.endsWith("/1")` 当"文本已实现"的标记，M4 把版本号升到 `/2`
     * 之后那个判定就失效了（会把三种协议全部隐藏），所以改成显式列出 —— 新增协议时，
     * 只有在这里登记过的才会出现在界面上。
     */
    fun supported(): List<AiApiRef> = all.keys.toList()
}

/** 供 `JsonPrimitive` 之外的判空使用。 */
internal fun JsonPrimitive?.orNull(): String? = this?.contentOrNullValue()

private fun JsonPrimitive.contentOrNullValue(): String? = runCatching { content }.getOrNull()

// ---------------------------------------------------------------------------
//  工具定义 / 参数
// ---------------------------------------------------------------------------

/** OpenAI 兼容与 Responses 的 `tools` 结构（`{type:"function", function:{…}}` vs 扁平）。 */
private fun openAiTools(tools: List<ToolSpec>): JsonArray = buildJsonArray {
    tools.forEach { tool ->
        add(
            buildJsonObject {
                put("type", "function")
                put(
                    "function",
                    buildJsonObject {
                        put("name", tool.name)
                        put("description", tool.description)
                        put("parameters", tool.parameters)
                    },
                )
            }
        )
    }
}

private fun responsesTools(tools: List<ToolSpec>): JsonArray = buildJsonArray {
    tools.forEach { tool ->
        add(
            buildJsonObject {
                put("type", "function")
                put("name", tool.name)
                put("description", tool.description)
                put("parameters", tool.parameters)
            }
        )
    }
}

private fun anthropicTools(tools: List<ToolSpec>): JsonArray = buildJsonArray {
    tools.forEach { tool ->
        add(
            buildJsonObject {
                put("name", tool.name)
                put("description", tool.description)
                put("input_schema", tool.parameters)
            }
        )
    }
}

/**
 * 回传给上游的工具参数。
 *
 * 上游要的是 **JSON 对象**而不是字符串，而模型给的是字符串；解析失败时**不能抛**
 * （抛了就整轮失败），退化成 `{}` 并把原文丢掉 —— 参数错误由工具层报给模型更合适。
 */
internal fun parseArguments(raw: String): JsonObject {
    val text = raw.trim()
    if (text.isEmpty()) return JsonObject(emptyMap())
    return runCatching { ProtocolJson.parseToJsonElement(text) as? JsonObject }.getOrNull()
        ?: JsonObject(emptyMap())
}

/** 显式表达"这里确实没有内容"（`content: null`）。 */
internal val JsonNullValue: JsonNull = JsonNull
