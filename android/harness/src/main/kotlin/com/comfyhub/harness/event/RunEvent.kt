package com.comfyhub.harness.event

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.concurrent.ConcurrentHashMap

/**
 * 手机端 harness 的 Run 事件信封（🔒 `docs/run-event-contract.md`）。
 *
 * 这是"**UI 代码两端共享、零分叉**"的全部依据（`docs/mobile-harness-design.md` §5）：
 * 手机上事件的**生产者**是本地 harness（platform channel），桌面上是远端 SSE，
 * 但**信封与类型逐字一致** —— 于是 `lib/` 下那套 Run 渲染代码一行都不用为手机重写。
 *
 * ⚠ 本文件是**移植**，不是重写：字段名、常量值、`toSse()` 的字节序列都与桌面
 * `server/src/main/kotlin/com/comfyhub/ai/RunEventBus.kt` 逐字对齐，改一边必须改另一边。
 * 防线：`RunEventContractTest`（两端各一份，都从 `docs/run-event-contract.md` 读）。
 */
data class RunEvent(val seq: Int, val type: String, val payload: JsonObject) {

    /**
     * 契约 §1 的帧格式（`id:` / `event:` / `data:` / 空行）。
     *
     * 手机走 platform channel、不真的发 SSE，但这个方法**照样保留**：
     * 它是"信封长什么样"的**可执行定义**，契约 §1 的一致性用例就断言它。
     */
    fun toSse(): String = buildString {
        append("id: ").append(seq).append('\n')
        append("event: ").append(type).append('\n')
        append("data: ").append(payload.toString()).append("\n\n")
    }
}

/**
 * 事件类型常量（17 个）—— 唯一真源是契约 §2 的表，**桌面 `RunEventType` 必须同值同序**。
 *
 * 命名与顺序照搬桌面，方便逐行对照；`RunEventContractTest` 用反射把这里的常量
 * 全部读出来与契约比对（所以新增常量会被自动纳入防线，不会漏）。
 */
object RunEventType {
    const val RUN_STARTED = "run.started"
    const val MESSAGE_STARTED = "message.started"
    const val REASONING_DELTA = "reasoning.delta"
    const val TEXT_DELTA = "text.delta"
    const val TOOL_REQUESTED = "tool.requested"
    const val TOOL_STARTED = "tool.started"
    const val TOOL_COMPLETED = "tool.completed"
    const val TOOL_FAILED = "tool.failed"
    /**
     * AI 向用户提问（**与 `tool.requested` 分开是故意的**，契约 §3）：
     * 那个是"等你点批准"，这个是"等你答一段文字"，而且 Run **不会因此结束**。
     */
    const val USER_QUESTION = "user.question"
    const val USER_QUESTION_ANSWERED = "user.question.answered"
    const val USAGE_UPDATED = "usage.updated"
    const val MESSAGE_COMPLETED = "message.completed"
    const val CONVERSATION_UPDATED = "conversation.updated"
    const val RUN_COMPLETED = "run.completed"
    const val RUN_FAILED = "run.failed"
    const val RUN_CANCELLED = "run.cancelled"
    const val HEARTBEAT = "heartbeat"

    /** 全部类型（顺序与契约 §2 的表一致）。 */
    val ALL: List<String> = listOf(
        RUN_STARTED, MESSAGE_STARTED, REASONING_DELTA, TEXT_DELTA,
        TOOL_REQUESTED, TOOL_STARTED, TOOL_COMPLETED, TOOL_FAILED,
        USER_QUESTION, USER_QUESTION_ANSWERED, USAGE_UPDATED,
        MESSAGE_COMPLETED, CONVERSATION_UPDATED,
        RUN_COMPLETED, RUN_FAILED, RUN_CANCELLED, HEARTBEAT,
    )
}

/**
 * 一次 Run 的事件总线（手机侧）：**内存里只留最近 [MEMORY_KEEP] 条** + 订阅通道。
 *
 * 与桌面 [RunEventBus] 的三条语义逐条对齐（契约 §4 / 红线 R5）：
 *  1. `seq` 在**一次 Run 内**从 1 起单调递增，**绝不回退、绝不复用** ——
 *     内存里丢最老的只是"回放不到那么老"，而 `nextSeq` 一旦重置，客户端游标会永久错位；
 *  2. 环形缓冲用 `ArrayDeque`（`removeFirst()` O(1)），不是 List（那是"滑长列表卡顿"的服务端版本）；
 *  3. 订阅者拿到的是**实时**那份，不受环形上限影响 —— 手机切后台再回来时靠这条续传。
 *
 * 与桌面的唯一差别：手机侧事件**同时落本地库**由 `com.comfyhub.harness.store.LocalRunStore` 负责
 *（桌面是 `AiRunRepo.EventBatcher`），进程被杀后重启从本地库回放。
 */
class RunEventBus {

    private class Stream {
        val events = ArrayDeque<RunEvent>()
        val subscribers = mutableListOf<Channel<RunEvent>>()
        var closed = false
        var nextSeq = 1
    }

    private val streams = ConcurrentHashMap<String, Stream>()

    fun open(runId: String) {
        streams[runId] = Stream()
    }

    @Synchronized
    fun emit(runId: String, type: String, payload: JsonObject = buildJsonObject { }): RunEvent? {
        val stream = streams[runId] ?: return null
        if (stream.closed) return null
        val event = RunEvent(stream.nextSeq++, type, payload)
        stream.events.addLast(event)
        while (stream.events.size > MEMORY_KEEP) stream.events.removeFirst()
        stream.subscribers.forEach { it.trySend(event) }
        return event
    }

    @Synchronized
    fun close(runId: String) {
        val stream = streams[runId] ?: return
        stream.closed = true
        stream.subscribers.forEach { it.close() }
        stream.subscribers.clear()
    }

    fun history(runId: String): List<RunEvent> = streams[runId]?.events?.toList() ?: emptyList()

    fun isClosed(runId: String): Boolean = streams[runId]?.closed ?: true

    /**
     * 订阅某个 Run：先把历史事件推一遍（按 `after` 过滤），再接收后续事件。
     * Run 不在内存里（进程被杀过）时返回 null，由调用方改从**本地库**回放。
     */
    @Synchronized
    fun subscribe(runId: String, after: Int): ReceiveChannel<RunEvent>? {
        val stream = streams[runId] ?: return null
        val channel = Channel<RunEvent>(Channel.UNLIMITED)
        stream.events.filter { it.seq > after }.forEach { channel.trySend(it) }
        if (stream.closed) {
            channel.close()
        } else {
            stream.subscribers += channel
        }
        return channel
    }

    @Synchronized
    fun unsubscribe(runId: String, channel: ReceiveChannel<RunEvent>) {
        val stream = streams[runId] ?: return
        stream.subscribers.removeAll { it === channel }
    }

    companion object {
        /**
         * 内存里每个 Run 最多留多少条事件（与桌面同值，契约 §4）。
         *
         * ⚠ 这是**内存**上限：一次 Run 里 `text.delta` 一分钟就上万条（实测单 Run 4.7 万）。
         * 手机比桌面更该省内存（后台被杀的配额），所以这里**只许对齐、不许调大**。
         */
        const val MEMORY_KEEP = 2000
    }
}

/** 组装 payload 的小工具（与桌面 `payload(...)` 同语义：null → JSON null，其余原样）。 */
fun payload(vararg pairs: Pair<String, Any?>): JsonObject = buildJsonObject {
    pairs.forEach { (k, v) ->
        when (v) {
            null -> put(k, JsonNull)
            is String -> put(k, v)
            is Int -> put(k, v)
            is Long -> put(k, v)
            is Boolean -> put(k, v)
            is JsonObject -> put(k, v)
            is JsonElement -> put(k, v)
            else -> put(k, v.toString())
        }
    }
}
