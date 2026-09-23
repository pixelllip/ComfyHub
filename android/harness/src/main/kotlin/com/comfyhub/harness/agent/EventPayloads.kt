package com.comfyhub.harness.agent

import com.comfyhub.harness.store.MessagePartRecord
import com.comfyhub.harness.tools.ToolOutcome
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * 事件 payload 的组装 —— **字段名与桌面逐字一致**（🔒 `docs/run-event-contract.md` §2）。
 *
 * ## 为什么值得单独一个文件
 *
 * "UI 代码两端共享"这句话，**全靠这里的键名**（`docs/mobile-harness-design.md` §5）。
 * 漂移是**静默**的：手机把 `run.failed` 的 payload 写成 `errorCode`，桌面写的是 `code`，
 * 两端各自编译通过、各自的测试全绿，而手机上那条"生成失败"的提示**永远是空的**
 * （Flutter 的 `AiRunEvent.code` 读的是 `code`）。
 *
 * 所以：
 *  1. 每个 builder 的 KDoc 里写明它对齐的是桌面哪一处；
 *  2. `EventPayloadParityTest` 逐个断言键集合 —— 那才是真正的防线。
 *
 * 桌面出处（`server/src/main/kotlin/com/comfyhub/ai/HarnessRunner.kt`）：
 * | 事件 | 桌面行号 |
 * | --- | --- |
 * | `tool.requested` | `:724` |
 * | `tool.started` | `:736` |
 * | `tool.completed` | `:779` |
 * | `tool.failed` | `:792` |
 * | `message.completed` | `:838` |
 * | `run.completed` | `:861` |
 * | `run.failed` | `:886` |
 * | `conversation.updated` | `:224` |
 */
object EventPayloads {

    /**
     * `tool.requested`：`runId` / `callId` / `name` / `arguments` / `approval`。
     *
     * ⚠ 键是 **`arguments`**（不是 `args`）：契约文档 §2 的表把这一列写成了 `args`，
     * 而**桌面代码与 Flutter 的 `AiRunEvent.arguments` 用的都是 `arguments`** —— 以代码为准
     * （见 `EventPayloadParityTest` 与文档里同步修正的那一行）。
     */
    fun toolRequested(
        runId: String,
        callId: String,
        name: String,
        arguments: String,
        approval: String,
    ): JsonObject = buildJsonObject {
        put("runId", runId)
        put("callId", callId)
        put("name", name)
        put("arguments", arguments)
        put("approval", approval)
    }

    fun toolStarted(callId: String, name: String): JsonObject = buildJsonObject {
        put("callId", callId)
        put("name", name)
    }

    fun toolCompleted(
        callId: String,
        name: String,
        elapsedMs: Long,
        preview: String,
        result: JsonObject?,
    ): JsonObject = buildJsonObject {
        put("callId", callId)
        put("name", name)
        put("elapsedMs", elapsedMs)
        put("preview", preview)
        // 界面靠 `result.mediaIds` 贴「画廊入口卡」——**只认后端给的 mediaIds**（AGENTS §10.1）
        if (result != null) put("result", result)
    }

    fun toolFailed(
        callId: String,
        name: String,
        code: String?,
        message: String,
        approval: String,
        elapsedMs: Long,
    ): JsonObject = buildJsonObject {
        put("callId", callId)
        put("name", name)
        put("code", code)
        put("message", message)
        put("approval", approval)
        put("elapsedMs", elapsedMs)
    }

    /** [ToolOutcome] → `tool.completed` / `tool.failed` 的载荷（省得调用方自己判断 ok）。 */
    fun forToolOutcome(
        callId: String,
        name: String,
        outcome: ToolOutcome,
        elapsedMs: Long,
        approval: String = "not_required",
    ): Pair<String, JsonObject> =
        if (outcome.ok) {
            com.comfyhub.harness.event.RunEventType.TOOL_COMPLETED to toolCompleted(
                callId = callId,
                name = name,
                elapsedMs = elapsedMs,
                // 预览：压成一行、截断（与桌面 `preview()` 同语义）
                preview = previewOf(outcome.content),
                result = outcome.jsonPayload,
            )
        } else {
            com.comfyhub.harness.event.RunEventType.TOOL_FAILED to toolFailed(
                callId = callId,
                name = name,
                code = outcome.errorCode,
                message = outcome.error ?: outcome.content,
                approval = approval,
                elapsedMs = elapsedMs,
            )
        }

    /** 工具结果的单行预览（与桌面 `ToolRegistry.preview` 同口径）。 */
    fun previewOf(text: String, max: Int = 200): String {
        val flat = text.replace(Regex("\\s+"), " ").trim()
        return if (flat.length <= max) flat else flat.take(max) + "…"
    }

    /**
     * `message.completed`：`messageId` / `text` / `reasoning` / `finishReason` / `elapsedMs` /
     * `reasoningEffort` / `steps` / `parts` / `usage`。
     *
     * ⚠ `parts` 是**权威**（Flutter 在 parts 非空时只渲染 parts）：
     * 少发它，手机上那条消息就会"有正文、没工具卡"。
     */
    fun messageCompleted(
        messageId: String,
        text: String,
        reasoning: String?,
        finishReason: String?,
        elapsedMs: Long,
        reasoningEffort: String?,
        steps: Int,
        parts: List<MessagePartRecord>,
        usage: JsonObject,
    ): JsonObject = buildJsonObject {
        put("messageId", messageId)
        put("text", text)
        put("reasoning", reasoning)
        put("finishReason", finishReason)
        put("elapsedMs", elapsedMs)
        put("reasoningEffort", reasoningEffort)
        put("steps", steps)
        put("parts", partsJson(parts))
        put("usage", usage)
    }

    /** 有序块的 JSON（字段名与桌面 `AiMessagePartDto` 一致）。 */
    fun partsJson(parts: List<MessagePartRecord>) = buildJsonArray {
        parts.forEach { part ->
            add(
                buildJsonObject {
                    put("type", part.type)
                    put("text", part.text)
                    put("toolCallId", part.toolCallId)
                    put("jsonPayload", part.jsonPayload)
                }
            )
        }
    }

    fun runCompleted(runId: String, elapsedMs: Long): JsonObject = buildJsonObject {
        put("runId", runId)
        put("elapsedMs", elapsedMs)
    }

    /** `run.failed`：键是 **`code` / `message`**（不是 `errorCode` / `error`）。 */
    fun runFailed(runId: String, code: String?, message: String?): JsonObject = buildJsonObject {
        put("runId", runId)
        put("code", code)
        put("message", message)
    }

    fun runCancelled(runId: String): JsonObject = buildJsonObject { put("runId", runId) }

    /** token 用量（与桌面 `usage.updated` / `message.completed.usage` 同键）。 */
    fun usage(
        inputTokens: Long,
        outputTokens: Long,
        cachedTokens: Long = 0,
        reasoningTokens: Long = 0,
    ): JsonObject = buildJsonObject {
        put("inputTokens", inputTokens)
        put("outputTokens", outputTokens)
        put("cachedTokens", cachedTokens)
        put("reasoningTokens", reasoningTokens)
    }

    /** 从上游给的 usage JSON 里读 token 数（认 OpenAI / Anthropic 两家的字段名）。 */
    fun usageOf(raw: kotlinx.serialization.json.JsonElement?): JsonObject {
        val obj = raw as? JsonObject ?: return usage(0, 0)
        fun long(vararg keys: String): Long = keys.firstNotNullOfOrNull { key ->
            (obj[key] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull()
        } ?: 0L
        return usage(
            inputTokens = long("input_tokens", "prompt_tokens"),
            outputTokens = long("output_tokens", "completion_tokens"),
            cachedTokens = long("cached_tokens", "cache_read_input_tokens"),
            reasoningTokens = long("reasoning_tokens"),
        )
    }
}
