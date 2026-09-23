package com.comfyhub.ai

import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.buildJsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * `RunEventBus` 的环形缓冲（P1 / 总纲 §5.1）与断线重连续传（红线 **R5**）。
 *
 * 为什么这里值得有用例：环形缓冲的**收益**是省内存，**风险**是"重连时少了一段"。
 * 而它少的那一段会不会被用户看见、`seq` 会不会因此错位，光看代码看不出来 ——
 * 尤其是"`seq` 绝不回退/复用"这条：内存里丢老事件是对的，
 * 但要是顺手把 `nextSeq` 也重置了，客户端的游标就会永久错位（那才是真丢数据）。
 */
class RunEventBusTest {

    private fun drain(ch: ReceiveChannel<RunEvent>): List<RunEvent> = runBlocking {
        val out = mutableListOf<RunEvent>()
        for (e in ch) out += e
        out
    }

    @Test
    fun `超过内存上限只丢最老的、seq 绝不回退`() {
        val bus = RunEventBus()
        bus.open("r1")
        val total = RunEventBus.MEMORY_KEEP + 50
        repeat(total) { i -> bus.emit("r1", RunEventType.TEXT_DELTA, buildJsonObject { }) }

        val history = bus.history("r1")
        assertEquals(RunEventBus.MEMORY_KEEP, history.size, "内存里必须只剩上限那么多条")
        // 丢的是最老的、留的是最新的
        assertEquals(total - RunEventBus.MEMORY_KEEP + 1, history.first().seq, "应该丢最老的")
        assertEquals(total, history.last().seq, "最新的必须还在")
        // 丢老事件**不许**让 seq 回退：继续发必须接在 total 后面
        val next = bus.emit("r1", RunEventType.HEARTBEAT, buildJsonObject { })
        assertEquals(total + 1, next?.seq, "seq 回退/复用会让客户端游标永久错位")
    }

    @Test
    fun `R5：发行中的 Run 断线重连能拿全 —— 订阅者拿到的是实时那份，不受环形上限影响`() {
        val bus = RunEventBus()
        bus.open("r2")
        // 先灌满并超过上限：订阅者仍然应该一条不落地收到**之后的**每一条
        repeat(RunEventBus.MEMORY_KEEP + 10) { bus.emit("r2", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        val ch = assertNotNull(bus.subscribe("r2", after = RunEventBus.MEMORY_KEEP + 10), "Run 在内存里就必须能订阅")
        repeat(5) { bus.emit("r2", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        bus.close("r2")
        val got = drain(ch)
        assertEquals(5, got.size, "订阅之后发的每一条都要收到")
        assertEquals(
            (RunEventBus.MEMORY_KEEP + 11..RunEventBus.MEMORY_KEEP + 15).toList(),
            got.map { it.seq },
            "序号要连续，不能因为缓冲截断而跳",
        )
    }

    @Test
    fun `重连时 after 早于窗口起点，拿到的是窗口内的（不假装全都有）`() {
        val bus = RunEventBus()
        bus.open("r3")
        repeat(RunEventBus.MEMORY_KEEP + 10) { bus.emit("r3", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        // 客户端只有很老的游标（比如它离线很久了）→ 只能给它窗口内的，从窗口起点开始
        val ch = assertNotNull(bus.subscribe("r3", after = 1))
        bus.close("r3")
        val got = drain(ch)
        assertEquals(RunEventBus.MEMORY_KEEP, got.size)
        assertEquals(RunEventBus.MEMORY_KEEP + 10, got.last().seq, "最新的那条必须在")
        assertTrue(got.first().seq > 1, "老于窗口的补不回来 —— 这就是库侧保留窗口存在的理由")
    }

    @Test
    fun `Run 不在内存里时订阅返回 null，由调用方改从库里回放`() {
        val bus = RunEventBus()
        assertNull(bus.subscribe("nope", after = 0), "后端重启过的 Run 必须回落到数据库回放")
    }
}
