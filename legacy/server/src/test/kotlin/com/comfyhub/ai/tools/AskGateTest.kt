package com.comfyhub.ai.tools

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 一次问多个问题之后的两件事（用户 2026-09-22 拍的形态，照 DeepSeek Harness）：
 * **一张卡装最多 5 问、分页、可逐题跳过、最后一页「提交」、答案可为空**。
 *
 * 这里钉的是两处最容易错的地方：
 *
 *  1. **归组**：同一次调用生成的多条记录要能被按 `groupId` 拼回"同一张卡"，
 *     并且按题号排 —— 排错的话界面第 1 页显示的是第 3 题。
 *  2. **降级窗口**（[AskGate.awaitAll]）：新界面是一次提交整张卡；但**旧界面只认第一条**
 *     （它按 tool call 挂卡，多条事件会互相覆盖）。那时只有一条答复会来 ——
 *     绝不能因此把 Run 挂到 30 分钟超时。所以"任意一条到了"之后再给一个短宽限窗收剩下的，
 *     收不到就按"用户没回答"继续（宁可少答几题，也不能让用户对着卡死的 Run 干等）。
 */
class AskGateTest {

    private fun question(
        id: String,
        index: Int,
        groupId: String = "g1",
        text: String = "Q$index",
    ) = PendingQuestion(
        questionId = id,
        runId = "r1",
        conversationId = "c1",
        question = text,
        options = emptyList(),
        allowFreeText = true,
        askedAtMs = 1_000L + index,
        index = index,
        total = 3,
        groupId = groupId,
    )

    @Test
    fun `整张卡按 groupId 归组并按题号排序`() {
        val gate = AskGate()
        val asks = AskRegistry(gate)
        // 故意倒着 track：界面拿到的事件顺序不保证，卡片必须自己排对
        listOf(2, 0, 1).forEach { asks.track(question("q$it", it)) }

        assertEquals(
            listOf("Q0", "Q1", "Q2"),
            asks.pendingGroupFor("r1").map { it.question },
            "同一张卡要按 index 排，不能按事件到达顺序",
        )
        assertEquals(3, asks.group("g1").size)
        // 同一张卡里的一题也算"问过"，不许重复开卡
        assertEquals("q1", asks.findSameQuestion("r1", "Q1")?.questionId)
    }

    @Test
    fun `已经答过的那几条直接收回来`() = runBlocking {
        val gate = AskGate()
        val ids = listOf("q0", "q1", "q2")
        ids.forEach { gate.open(it) }
        // 用户手快：抢在 awaitAll 之前答完（正常竞态，邮箱必须先接住）
        assertEquals(true, gate.resolve("q0", AskedAnswer("A", 0)))
        assertEquals(true, gate.resolve("q1", AskedAnswer("B", 1)))
        assertEquals(true, gate.resolve("q2", AskedAnswer("自由文本")))

        val out = gate.awaitAll(ids)
        assertEquals("A", out["q0"]?.answer)
        assertEquals("B", out["q1"]?.answer)
        assertEquals("自由文本", out["q2"]?.answer)
    }

    @Test
    fun `只答了一部分时在宽限窗内收口 不挂到超时`() = runBlocking {
        // 超时给得很长（30 秒），但宽限窗只有 0.3 秒 —— 走的就是"旧界面只交第一条"那条路
        val gate = AskGate(timeoutMs = 30_000)
        val ids = listOf("q0", "q1", "q2")
        ids.forEach { gate.open(it) }

        val started = System.currentTimeMillis()
        val job = async(Dispatchers.Default) { gate.awaitAll(ids, graceMs = 300) }
        delay(80)
        gate.resolve("q1", AskedAnswer("只答了这一题", 1))
        val out = job.await()
        val elapsed = System.currentTimeMillis() - started

        assertEquals("只答了这一题", out["q1"]?.answer)
        assertNull(out["q0"], "没答的那几条必须是 null（= 用户没回答），不是异常、也不是空串")
        assertNull(out["q2"])
        assertTrue(elapsed < 5_000, "宽限窗没生效：整整等了 ${elapsed}ms（旧界面会把 Run 拖死）")
    }

    @Test
    fun `一条都没答时如实全部返回 null`() = runBlocking {
        val gate = AskGate(timeoutMs = 200)
        val ids = listOf("q0", "q1")
        ids.forEach { gate.open(it) }
        val out = gate.awaitAll(ids, graceMs = 100)
        assertEquals(setOf("q0", "q1"), out.keys)
        assertNull(out["q0"])
        assertNull(out["q1"])
    }

    @Test
    fun `多选题的勾选要能带回去`() = runBlocking {
        val gate = AskGate()
        val ids = listOf("q0")
        ids.forEach { gate.open(it) }
        gate.resolve("q0", AskedAnswer("Sage Attention、加速 LoRA", null, listOf(0, 2)))

        val out = gate.awaitAll(ids)
        assertEquals(listOf(0, 2), out["q0"]?.optionIndexes)
        assertEquals("Sage Attention、加速 LoRA", out["q0"]?.answer)
    }

    @Test
    fun `提问形状里带上分页与多选字段`() {
        val json = question("q0", 1).copy(multiSelect = true).toJson()
        assertEquals("1", json["index"].toString())
        assertEquals("3", json["total"].toString())
        assertEquals("g1", json["groupId"].toString().trim('"'))
        assertEquals("true", json["multiSelect"].toString())
    }
}
