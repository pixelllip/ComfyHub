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
 * 运行中的 Run 事件总线：内存里保留全部事件 + 订阅通道。
 * 事件同时会落库（[AiRunRepo.appendEvent]），因此后端重启后仍能回放已完成的事件。
 */
class RunEventBus {
    private class Stream {
        val events = mutableListOf<RunEvent>()
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
        stream.events += event
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
    const val USAGE_UPDATED = "usage.updated"
    const val MESSAGE_COMPLETED = "message.completed"
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
