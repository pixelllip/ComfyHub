package com.comfyhub.ai.tools

import com.comfyhub.ai.QuestionStore
import com.comfyhub.ai.StoredQuestion
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 提问闸门的**落库**语义（M1，2026-09-23：总纲「AskGate 落库 + 可跨端查询 + 超时恢复」，
 * R8 防线"跨端可见 + 后端重启不丢"的一半）。
 *
 * 按 `AiSeedCatalogTest` 的既有约定，Gradle 测试**不连数据库** —— 所以这里用一块内存
 * [MemoryQuestionStore] 钉住 [AskRegistry] 与 [QuestionStore] 之间的契约：
 * 真正的 SQL 实现（`AiQuestionRepo`，`ai_questions` 表）逐字对齐这几个方法。
 * 防的是这三类回归：
 *
 *  1. **answered 被 Run 收尾覆盖**（finalize 只许动 pending —— 状态只能前进）；
 *  2. **重启后提问凭空消失**（get 必须回落 store）；
 *  3. **跨端总览把已回答的也算成"在等"**（allPending 的口径）。
 */
class AskPersistenceTest {

    /** 内存版 QuestionStore：与 AiQuestionRepo 同一套方法契约，行为对齐它的 SQL 语义。 */
    private class MemoryQuestionStore : QuestionStore {
        val rows = LinkedHashMap<String, StoredQuestion>()

        override fun insert(q: PendingQuestion) {
            rows[q.questionId] = StoredQuestion(q, "pending")
        }

        override fun recordAnswer(
            questionId: String,
            answer: String,
            optionIndex: Int?,
            optionIndexes: List<Int>,
            status: String,
        ) {
            val current = rows.getValue(questionId)
            rows[questionId] = current.copy(
                question = current.question.copy(
                    answer = answer,
                    optionIndex = optionIndex,
                    optionIndexes = optionIndexes,
                ),
                status = status,
            )
        }

        override fun finalizeIfPending(questionId: String, status: String) {
            val current = rows[questionId] ?: return
            if (current.status != "pending") return // 状态只能前进：answered/skipped 不许被覆盖
            rows[questionId] = current.copy(status = status)
        }

        override fun finalizeRun(runId: String, status: String) {
            rows.values.toList().forEach { row ->
                if (row.question.runId == runId) finalizeIfPending(row.question.questionId, status)
            }
        }

        override fun find(questionId: String): StoredQuestion? = rows[questionId]
    }

    private fun question(
        id: String,
        runId: String = "r1",
        text: String = "用哪张参考图？",
    ) = PendingQuestion(
        questionId = id,
        runId = runId,
        conversationId = "c1",
        question = text,
        options = listOf("A 图", "B 图"),
        allowFreeText = true,
        askedAtMs = 1_000L,
        index = 0,
        total = 1,
        groupId = id,
    )

    @Test
    fun `track 时就落一行 pending 不等 Run 收尾`() {
        val store = MemoryQuestionStore()
        val asks = AskRegistry(AskGate(), store)

        asks.track(question("q1"))

        assertEquals("pending", store.rows.getValue("q1").status, "提问开出来那一刻库里就该有它")
        assertTrue(asks.allPending().any { it.questionId == "q1" })
    }

    @Test
    fun `回答写终值 之后的收尾不许覆盖它`() {
        val store = MemoryQuestionStore()
        val asks = AskRegistry(AskGate(), store)
        asks.track(question("q1"))

        assertTrue(asks.answer("q1", "A 图", 0), "回答必须被采纳")
        assertEquals("answered", store.rows.getValue("q1").status)
        assertEquals("A 图", store.rows.getValue("q1").question.answer)

        // 模型拿到答案后的收尾（clear 传 storeStatus）不许把 answered 打回失效
        asks.clear("q1", "expired")
        assertEquals("answered", store.rows.getValue("q1").status, "状态只能前进：answered 不许被收尾覆盖")
    }

    @Test
    fun `用户跳过按 skipped 收尾`() {
        val store = MemoryQuestionStore()
        val asks = AskRegistry(AskGate(), store)
        asks.track(question("q1"))

        asks.clear("q1", "skipped")
        assertEquals("skipped", store.rows.getValue("q1").status)
        assertFalse(asks.allPending().any { it.questionId == "q1" }, "跳过之后不算'还在等'")
    }

    @Test
    fun `Run 收尾只把还在 pending 的行按失效收尾`() {
        val store = MemoryQuestionStore()
        val asks = AskRegistry(AskGate(), store)
        asks.track(question("q1"))
        asks.track(question("q2", runId = "r1"))

        assertTrue(asks.answer("q1", "A 图", 0))
        asks.discardRun("r1") // Run 结束（正常/失败/取消都走这里）

        assertEquals("answered", store.rows.getValue("q1").status)
        assertEquals("expired", store.rows.getValue("q2").status, "没人回答的那条随 Run 收尾失效")
    }

    @Test
    fun `后端重启后 get 回落查库 内存与跨端口径清零`() {
        val store = MemoryQuestionStore()
        val first = AskRegistry(AskGate(), store)
        first.track(question("q1"))
        assertTrue(first.answer("q1", "自由文本", null))

        // "重启"：全新的闸门与登记表，库还在
        val second = AskRegistry(AskGate(), store)

        assertNull(second.getLive("q1"), "内存里不该再有旧进程的活提问")
        val restored = second.get("q1")
        assertEquals("q1", restored?.questionId, "get 必须回落 store：问过什么不能凭空消失")
        assertEquals("自由文本", restored?.answer, "答了什么也不能丢")
        assertFalse(second.allPending().contains(restored), "重放的记录不算'还在等'")
    }

    @Test
    fun `跨端总览只含还在等的提问并按提问时间排序`() {
        val store = MemoryQuestionStore()
        val asks = AskRegistry(AskGate(), store)
        asks.track(question("q-new", runId = "r1").copy(askedAtMs = 2_000L))
        asks.track(question("q-old", runId = "r2").copy(askedAtMs = 1_000L))
        asks.track(question("q-done", runId = "r3").copy(askedAtMs = 3_000L))
        asks.answer("q-done", "B 图", 1)

        assertEquals(
            listOf("q-old", "q-new"),
            asks.allPending().map { it.questionId },
            "跨端总览 = 还在等的全部提问，按 askedAt 排；已回答的不出现",
        )
    }
}
