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
     */
    fun buildBody(
        model: String,
        messages: List<ChatTurn>,
        stream: Boolean,
        reasoning: ReasoningRequest = ReasoningRequest.NONE,
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

/** 对话中的一轮。附件（图片等）暂未实现，见适配器的 transports。 */
data class ChatTurn(val role: String, val content: String)

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

internal fun JsonElement?.raw(field: String): JsonElement? = (this as? JsonObject)?.get(field)
