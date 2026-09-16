package com.comfyhub.ai.protocol

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * SSE 解析（AIH-021 / AIH-024）。
 *
 * 供应商的流是**字节流**，一行可能被 TCP 拆开，也可能一次到好几行；这里只做
 * "按行喂进来、遇到空行出一个 frame" 的简单状态机，再由各协议适配器解释 data。
 * 畸形数据必须归到 `PROTOCOL_ERROR`，不能把异常直接漏给上层。
 */
class SseAccumulator {
    private val data = StringBuilder()
    private var event: String? = null

    data class Frame(val event: String?, val data: String)

    /** 喂一行（不含换行符）。返回非 null 表示一个完整事件结束。 */
    fun line(raw: String): Frame? {
        val line = raw.removeSuffix("\r")
        if (line.isEmpty()) return flush()
        if (line.startsWith(":")) return null // 注释/心跳，忽略
        val idx = line.indexOf(':')
        val field: String
        val value: String
        if (idx < 0) {
            field = line
            value = ""
        } else {
            field = line.substring(0, idx)
            value = line.substring(idx + 1).removePrefix(" ")
        }
        return when (field) {
            "data" -> {
                if (data.isNotEmpty()) data.append('\n')
                data.append(value)
                null
            }
            "event" -> {
                event = value
                null
            }
            else -> null // id / retry 等首期不用
        }
    }

    fun flush(): Frame? {
        if (data.isEmpty() && event == null) return null
        val frame = Frame(event, data.toString())
        data.setLength(0)
        event = null
        return frame
    }
}

/** 一次流里累积出来的结果（适配器 → Harness）。 */
data class StreamOutcome(
    val text: String,
    val reasoning: String?,
    val finishReason: String?,
    val usage: JsonElement?,
    val providerResponseId: String?,
)

/** 流式过程中推给 Harness 的增量。 */
sealed interface StreamEvent {
    data class TextDelta(val text: String) : StreamEvent
    data class ReasoningDelta(val text: String) : StreamEvent
    data class Usage(val usage: JsonElement) : StreamEvent
    data class ProviderId(val id: String) : StreamEvent

    /**
     * 工具调用的**增量片段**（M4）。三家协议的分片方式不同，这里统一成"索引 + 键 + 名称 + 参数片段"：
     *
     *  - OpenAI 兼容：`choices[].delta.tool_calls[] = {index, id, function:{name, arguments}}`，
     *    id/name 只在第一片出现，之后只有 index + arguments 片段；
     *  - Anthropic：`content_block_start(tool_use)` 给 `index/id/name`，
     *    `content_block_delta(input_json_delta)` 只给 `index` + `partial_json`；
     *  - Responses：`output_item.added` 给 `item.id` / `item.call_id` / `name`，
     *    `function_call_arguments.delta` 只给 `item_id` + `delta`。
     *
     * 所以 [key] 是**流内关联键**（Responses 用 item.id，其余用 id），[callId] 是**回传给上游的 id**
     * （`tool_call_id` / `tool_use_id` / `call_id`）。两者在 OpenAI / Anthropic 下相同。
     */
    data class ToolCallDelta(
        val index: Int,
        val key: String? = null,
        val callId: String? = null,
        val name: String? = null,
        val argumentsFragment: String? = null,
    ) : StreamEvent
}

/**
 * 把 [StreamEvent.ToolCallDelta] 分片拼成完整工具调用。
 *
 * 与适配器一样是无状态协议解析的一部分，独立成类是为了**可单测**：
 * 三家分片方式都能用同一组断言覆盖（见 `ProtocolAdapterTest`）。
 */
class ToolCallAccumulator {
    data class Draft(val callId: String, val name: String, val arguments: String)

    private class Partial(val key: String, var callId: String?, var name: String?) {
        val arguments = StringBuilder()
    }

    private val order = mutableListOf<String>()
    private val parts = LinkedHashMap<String, Partial>()
    private val indexToKey = mutableMapOf<Int, String>()

    fun apply(delta: StreamEvent.ToolCallDelta) {
        val key = delta.key ?: indexToKey[delta.index] ?: "idx:${delta.index}"
        if (delta.key != null) indexToKey.putIfAbsent(delta.index, delta.key)
        val part = parts.getOrPut(key) {
            order += key
            Partial(key, null, null)
        }
        if (!delta.callId.isNullOrEmpty()) part.callId = delta.callId
        if (!delta.name.isNullOrEmpty()) part.name = delta.name
        delta.argumentsFragment?.let { part.arguments.append(it) }
    }

    fun isEmpty(): Boolean = parts.isEmpty()

    /** 按出现顺序返回；`callId` 缺失时退回流内键（宁可能用也不要丢一次调用）。 */
    fun drafts(): List<Draft> = order.mapNotNull { key ->
        val part = parts[key] ?: return@mapNotNull null
        Draft(callId = part.callId ?: key, name = part.name.orEmpty(), arguments = part.arguments.toString())
    }
}

/**
 * 协议适配器。一个 Provider 固定一种协议（AIH-006），因此适配器是**无状态**的。
 *
 * `transports` 表示"这个适配器真的实现了哪种附件传输"——预检和 Run 准入都以此为准，
 * 不是根据模型声明的能力猜（AIH-028）。
 */
interface ProtocolAdapter {
    val api: AiApiRef
    val transports: Set<TransportRef>
    val adapterVersion: String

    /**
     * 组装请求体（纯函数，便于单测断言"到底发出去了什么"）。
     *
     * [reasoning] 是**已过滤**的思考设置：模型没声明推理能力时调用方传 [ReasoningRequest.NONE]，
     * 适配器只管按方言落到正确的字段上（AIH-056）。
     *
     * [tools] 为空表示本次不提供工具（模型未声明工具能力，或工具层被关闭）。空列表**不等于**
     * 发一个空 `tools: []` —— 很多网关对空数组直接 400，所以适配器必须整体省略该字段。
     */
    fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest = ReasoningRequest.NONE,
        tools: List<ToolSpec> = emptyList(),
    ): JsonObject

    /** 解释一行 SSE frame；返回 null 表示该 frame 不产生任何内容。 */
    fun interpret(frame: SseAccumulator.Frame): List<StreamEvent>
}

/** 与 `com.comfyhub.ai.AiApi` 对应的协议标识（避免领域层反向依赖）。 */
enum class AiApiRef(val wire: String) {
    OPENAI_COMPLETIONS("openai-completions"),
    OPENAI_RESPONSES("openai-responses"),
    ANTHROPIC_MESSAGES("anthropic-messages");

    companion object {
        fun parse(wire: String?): AiApiRef? = entries.firstOrNull { it.wire == wire }
    }
}

enum class TransportRef(val wire: String) {
    INLINE_BASE64("inline_base64"),
    REMOTE_URL("remote_url"),
    FILE_ID("file_id"),
    EXTRACTED_TEXT("extracted_text");

    companion object {
        fun parse(wire: String?): TransportRef? = entries.firstOrNull { it.wire == wire }
    }
}

/**
 * 对话中的一轮。
 *
 * M4 之后一轮不再只有正文：[toolCalls] 是助手发起的工具调用，[toolCallId] 是**工具结果**这一轮
 * 回填的调用 id。适配器负责把它翻译成各家协议的结构（`tool_calls` / `tool_use` /
 * `function_call_output`）。
 */
data class ChatTurn(
    val role: String,
    val content: String,
    val toolCalls: List<ToolCallRef> = emptyList(),
    val toolCallId: String? = null,
)

/** 助手发起的一次工具调用（已拼好参数 JSON 文本，原样回传给上游）。 */
data class ToolCallRef(val id: String, val name: String, val arguments: String)

/**
 * 提供给模型的工具定义（M4 / AIH-033~036）。
 *
 * [parameters] 是 JSON Schema 对象；[ToolSpec.EMPTY_PARAMS] 用于无参工具。
 */
data class ToolSpec(
    val name: String,
    val description: String,
    val parameters: JsonObject,
) {
    companion object {
        /** 无参工具的 JSON Schema（`{"type":"object","properties":{}}`）。 */
        val EMPTY_PARAMS: JsonObject = Json.parseToJsonElement(
            """{"type":"object","properties":{},"additionalProperties":false}"""
        ).jsonObject
    }
}

internal val ProtocolJson = Json { ignoreUnknownKeys = true; isLenient = true }

internal fun JsonElement?.str(field: String, nested: String? = null): String? {
    val obj = (this as? JsonObject) ?: return null
    val target = if (nested == null) obj else (obj[nested] as? JsonObject) ?: return null
    return (target[field] as? JsonPrimitive)?.contentOrNull
}

internal fun JsonElement?.objArray(field: String): List<JsonObject> =
    runCatching { (this as? JsonObject)?.get(field)?.jsonArray?.map { it.jsonObject } }.getOrNull() ?: emptyList()

internal fun JsonElement?.prim(field: String): JsonPrimitive? =
    runCatching { (this as? JsonObject)?.get(field)?.jsonPrimitive }.getOrNull()

/** 取整数（有的网关把 index 写成 number，有的写成字符串）。 */
internal fun JsonElement?.int(field: String): Int? =
    (this as? JsonObject)?.get(field)?.let { el ->
        runCatching { el.jsonPrimitive.content.toInt() }.getOrNull()
    }

/** 取布尔（`true` / `"true"` 都认）。 */
internal fun JsonElement?.bool(field: String): Boolean? =
    (this as? JsonObject)?.get(field)?.let { el ->
        runCatching { el.jsonPrimitive.content.toBooleanStrictOrNull() }.getOrNull()
    }

internal fun JsonElement?.raw(field: String): JsonElement? = (this as? JsonObject)?.get(field)
