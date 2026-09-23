package com.comfyhub.harness.agent

import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.MessagePartRecord

/**
 * 一条助手消息的"有序块（parts）"落库器 —— 移植自桌面 `HarnessRunner.PartSink`。
 *
 * 为什么自成一个类，而不是 `execute` 里的几个局部变量：失败 / 取消的兜底分支在 `try`
 * **外面**，而那里同样要能把"已经流出去、还没落成 part"的内容补上（见 [flushInflight]）。
 *
 * 顺序纪律（用户要求"完全按照消息获取顺序来，不要将最终回复置于最终思考的前面"）：
 * 思考 / 正文按**到达顺序**进 [inflight]（相邻同类合并成一段），一轮跑完再原样落库；
 * 工具卡由调用方在轮次边界上直接用 [add] 插进去，于是 parts 就是真实的时间顺序。
 */
class PartSink(private val store: HarnessStore, private val assistantId: String) {

    /** 已经落库的块（按落库顺序）。 */
    val parts = mutableListOf<MessagePartRecord>()

    /** 下一个 ordinal（从库里已有块之后接，**重试 / 恢复时不会覆盖旧块**）。 */
    private var ordinal = store.maxPartOrdinal(assistantId) + 1

    /**
     * 这一轮已经流出去、但**还没落成 part** 的思考 / 正文（顺序 = 到达顺序）。
     *
     * 它是"失败也不能丢内容"的那一半：正文早就写进了 `message.text`（界面正在流式显示），
     * 块却要等这一轮跑完才落。中间被 429 / 网络断掉时这一轮永远跑不完，
     * 不补落的话库里就只有正文没有块 —— 而前端在 **parts 非空时只渲染 parts**，
     * 于是用户"直接看不到它已经回复完的内容"（桌面 2026-09-19 的 429 现场）。
     */
    private val inflight = mutableListOf<Pair<String, String>>()

    fun add(
        type: String,
        text: String?,
        toolCallId: String? = null,
        jsonPayload: String? = null,
        queue: Boolean = true,
    ) {
        store.appendPart(assistantId, ordinal++, type, text, toolCallId, jsonPayload, queue = queue)
        parts += MessagePartRecord(assistantId, ordinal - 1, type, text, toolCallId, jsonPayload)
    }

    /** 记一片"还没落块"的增量（相邻同类型合并成一段）。 */
    fun buffer(type: String, chunk: String) {
        if (chunk.isEmpty()) return
        val last = inflight.lastOrNull()
        if (last != null && last.first == type) {
            inflight[inflight.size - 1] = type to (last.second + chunk)
        } else {
            inflight += type to chunk
        }
    }

    /**
     * 把缓冲里的思考 / 正文按到达顺序落成有序块并清空。
     *
     * 两个调用点：一轮**正常跑完**（把这一轮的块插进 parts），
     * 以及失败 / 取消的兜底分支（把已经流出去的那部分补上，一个字都不许丢）。
     */
    fun flushInflight() {
        if (inflight.isEmpty()) return
        val batch = inflight.toList()
        inflight.clear()
        batch.forEach { (type, chunk) -> add(type, chunk) }
    }

    fun pendingCount(): Int = inflight.size
}
