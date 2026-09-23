package com.comfyhub

import java.math.BigDecimal
import java.sql.Timestamp
import java.time.LocalDateTime
import java.time.ZoneId
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/**
 * 搬迁的取值 / 核对规则（P2，`MigrateData`）。
 *
 * 为什么这些必须有用例：**搬错这三类值都不会报错**，只是数据悄悄变样 ——
 * 时间整体偏一个时区、`TINYINY(1)` 的 true 变成字符串 "true"、`7.50` 变成 `7.5`
 * 导致核对假红。核对算法本身也要两边同一套，否则"哈希不一致"到底是搬错了还是算错了分不清。
 */
class MigrateDataTest {

    @Test
    fun `时间按 JVM 默认时区解释成 ISO-8601 UTC（与老代码 getTimestamp 同口径）`() {
        val local = LocalDateTime.of(2026, 9, 23, 20, 30, 15, 123_000_000)
        val expected = local.atZone(ZoneId.systemDefault()).toInstant()

        assertEquals(isoUtc(expected), MigrateData.fromMySql(local), "DATETIME 走 LocalDateTime 那条路")
        assertEquals(
            isoUtc(expected),
            MigrateData.fromMySql(Timestamp.valueOf(local)),
            "DATETIME 走 Timestamp 那条路 —— 两条必须同一个结果",
        )
        val text = MigrateData.fromMySql(local) as String
        assertTrue(Regex("""^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$""").matches(text), "实际：$text")
        assertEquals(expected, parseDbInstant(text), "写进去的文本必须能原样解析回来")
    }

    @Test
    fun `TINYINT(1) 布尔落成 1 0，JSON 的 byte 数组落成 UTF-8 文本`() {
        assertEquals(1, MigrateData.fromMySql(true))
        assertEquals(0, MigrateData.fromMySql(false))
        assertEquals("""{"a":1}""", MigrateData.fromMySql("""{"a":1}""".toByteArray(Charsets.UTF_8)))
        assertEquals(null, MigrateData.fromMySql(null))
    }

    @Test
    fun `DECIMAL 的 7_50 与 REAL 的 7_5 必须算成同一行（否则核对假红）`() {
        assertEquals(MigrateData.canonicalField(7.5), MigrateData.canonicalField(BigDecimal("7.50")))
        assertEquals(MigrateData.canonicalField(7.5), MigrateData.canonicalField(7.5f))
        assertEquals("7.5", MigrateData.canonicalField(BigDecimal("7.50")))
        assertEquals("0", MigrateData.canonicalField(BigDecimal("0.00")))
    }

    @Test
    fun `NULL 与空串要能区分（哨兵不能和合法值撞车）`() {
        assertNotEquals(MigrateData.canonicalField(null), MigrateData.canonicalField(""))
        assertEquals(MigrateData.canonicalField(""), "")
    }

    @Test
    fun `布尔与整数的规范文本一致：true 和 1 是同一个值`() {
        assertEquals(MigrateData.canonicalField(1), MigrateData.canonicalField(true))
        assertEquals(MigrateData.canonicalField(0), MigrateData.canonicalField(false))
    }

    @Test
    fun `搬迁表清单与依赖顺序：父表在子表前面，且事件日志默认跳过`() {
        val order = MigrateData.TABLES
        fun before(parent: String, child: String) =
            assertTrue(order.indexOf(parent) < order.indexOf(child), "$parent 必须排在 $child 前面（外键）")

        before("prompts", "prompt_tags")
        before("tags", "prompt_tags")
        before("prompts", "media_assets")
        before("media_assets", "media_tags")
        before("tags", "media_tags")
        before("prompts", "capture_runs")
        before("ai_providers", "ai_models")
        before("ai_conversations", "ai_messages")
        before("ai_messages", "ai_message_parts")
        before("ai_runs", "ai_run_events")
        before("ai_runs", "ai_tool_calls")
        before("ai_runs", "ai_questions")
        before("ai_conversations", "ai_questions")

        assertEquals(setOf("ai_run_events"), MigrateData.SKIPPED_BY_DEFAULT)
    }
}
