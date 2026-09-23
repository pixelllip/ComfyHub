package com.comfyhub.harness.attachment

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 附件内联预算（红线 **R9** 的第三层准入在手机侧的对应物）。
 *
 * 这里守的是"**既不能静默丢弃，也不能因为历史里有张大图就把请求打成 400**"：
 * 超预算的那些不发，但必须留下一条明确的说明（[InlineBudget.skippedNote]）。
 */
class InlineBudgetTest {

    @Test
    fun `按顺序贪心装填：装不下的跳过，继续试后面的小图`() {
        val sizes = listOf(6L, 6L, 1L) // 总计 13 > 10
        val plan = InlineBudget.plan(sizes, maxTotal = 10L, maxSingle = 8L)
        assertEquals(listOf(true, false, true), plan)
    }

    @Test
    fun `单张超上限的一律不发，且不占用总额度`() {
        val plan = InlineBudget.plan(listOf(9L, 1L), maxTotal = 10L, maxSingle = 8L)
        assertEquals(listOf(false, true), plan)
    }

    @Test
    fun `非正数的尺寸视为无效（不许拿 0 字节去骗预算）`() {
        assertEquals(listOf(false, false, true), InlineBudget.plan(listOf(0L, -5L, 3L), 10L, 8L))
    }

    @Test
    fun `没发出去的附件必须有一句明确说明（不许静默丢弃）`() {
        assertEquals("", InlineBudget.skippedNote(emptyList()))
        assertEquals("（以下附件未随本次请求发送：a.png、b.jpg）", InlineBudget.skippedNote(listOf("a.png", "b.jpg")))
    }

    @Test
    fun `run-kit 下发值优先于常量（两端预算必须一致）`() {
        val limits = InlineBudget.Limits(perImageBytes = 1024L, totalBytes = 2048L)
        assertTrue(limits.plan(listOf(1024L)).single())
        assertFalse("超过下发的单图上限就不发", limits.plan(listOf(2048L)).single())
        assertEquals(InlineBudget.MAX_INLINE_BYTES, InlineBudget.Limits.FALLBACK.perImageBytes)
        assertEquals(InlineBudget.MAX_TOTAL_BYTES, InlineBudget.Limits.FALLBACK.totalBytes)
    }
}
