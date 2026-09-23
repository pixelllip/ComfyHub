package com.comfyhub.ai

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.concurrent.ConcurrentHashMap

/**
 * 单调递增序号的 Run 事件（AIH-021）。
 *
 * Flutter 只消费这一套统一事件，不去解析供应商 SSE；断线后用 `after=<seq>` 续传。
 */
data class RunEvent(val seq: Int, val type: String, val payload: JsonObject) {
    fun toSse(): String = buildString {
        append("id: ").append(seq).append('\n')
        append("event: ").append(type).append('\n')
        append("data: ").append(payload.toString()).append("\n\n")
    }
}

/**
 * 运行中的 Run 事件总线：内存里保留**最近 [MEMORY_KEEP] 条**事件 + 订阅通道。
 * 事件同时会落库（[AiRunRepo.EventBatcher]），因此后端重启后仍能回放已完成的事件。
 *
 * ⚠ **内存缓冲是有上限的（P1，2026-09-23）**：以前是 `mutableListOf` 全量留，
 * 而实测单 Run 4.7 万个 `text.delta` —— 那是纯浪费的常驻内存（大 Run 能占几十 MB），
 * 也是总纲 §5.1 "RunEventBus 改环形缓冲"要治的东西。
 *
 * 保留策略两侧对齐、但**不共用同一个数**：
 *  - 内存 [MEMORY_KEEP]：够"页面重新订阅 / 滚回看最近这些"；
 *  - 库里 [AiRunRepo.pruneEvents] 的 keep：够"后端重启后回放"。
 * 两侧都是"留尾部"，因为**断线重连永远只看最新的一段**（`?after=<seq>`）。
 */
class RunEventBus {
    private class Stream {
        /**
         * 环形缓冲：只留最近 [MEMORY_KEEP] 条。
         *
         * 用 ArrayDeque 而不是 List：`removeFirst()` 是 O(1)，List 那个是 O(n)
         *（逐字流式时每条都要挪整个列表，正是"滑长列表卡顿"的服务端版本）。
         */
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
        // 环形：超上限就从最老的开始丢。**seq 本身绝不回退/复用** ——
        // 内存里少一条只是"回放不到那么老"，而 seq 复用会让客户端的游标错位（那才是真丢数据）。
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

    companion object {
        /**
         * 内存里每个 Run 最多留多少条事件（P1 / 总纲 §5.1 的"环形缓冲"）。
         *
         * 2000 的依据：一次 Run 里前端真正会回看的只有最近这一段（用户看到的是一屏聊天），
         * 而逐字流式的 `text.delta` 一分钟就上万条 —— 全量留实测单 Run 4.7 万条。
         * ⚠ 这是**内存**上限，与库里 `HarnessRunner.EVENT_KEEP`(4000) 是**两件事**：
         * 前者管"进程活着时能重放多少"，后者管"后端重启后能回放多少"。
         */
        const val MEMORY_KEEP = 2000
    }

    /**
     * 订阅某个 Run：先把历史事件推一遍（按 `after` 过滤），再接收后续事件。
     * Run 不在内存里（后端重启过）时返回 null，由调用方改从数据库回放。
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
        stream.subscribers.removeIf { it === channel }
    }
}

/** 事件类型常量（与实施方案 9.2 一致）。 */
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
     * AI 向用户提问（用户建议 ⑥，2026-09-20）。
     *
     * 与 `tool.requested` 分开是**有意的**：那个是"等你点批准"，这个是"等你回答"。
     * 界面上两者长得像，语义完全不同 —— 批准是允许/不允许，回答是一段文字，
     * 而且**这次 Run 不会因此结束**：`ask_user_question` 会挂在这里等，
     * 用户答完把回答作为工具结果喂回模型，同一个 Run 接着往下做。
     */
    const val USER_QUESTION = "user.question"
    /** 用户答完了：界面据此把提问卡切成"已回答"（`POST /api/ai/questions/{id}/answer`）。 */
    const val USER_QUESTION_ANSWERED = "user.question.answered"
    const val USAGE_UPDATED = "usage.updated"
    const val MESSAGE_COMPLETED = "message.completed"
    /** 会话标题被自动总结出来了（用户建议 ③）：前端据此刷新标题与对话列表 */
    const val CONVERSATION_UPDATED = "conversation.updated"
    const val RUN_COMPLETED = "run.completed"
    const val RUN_FAILED = "run.failed"
    const val RUN_CANCELLED = "run.cancelled"
    const val HEARTBEAT = "heartbeat"
}

internal fun payload(vararg pairs: Pair<String, Any?>): JsonObject = buildJsonObject {
    pairs.forEach { (k, v) ->
        when (v) {
            null -> put(k, kotlinx.serialization.json.JsonNull)
            is String -> put(k, v)
            is Int -> put(k, v)
            is Long -> put(k, v)
            is Boolean -> put(k, v)
            is JsonObject -> put(k, v)
            is kotlinx.serialization.json.JsonElement -> put(k, v)
            else -> put(k, v.toString())
        }
    }
}
