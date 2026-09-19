package com.comfyhub.ai

import com.comfyhub.ai.protocol.AnthropicMessagesAdapter
import com.comfyhub.ai.protocol.ChatTurn
import com.comfyhub.ai.protocol.OpenAiResponsesAdapter
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.ReasoningRequest
import com.comfyhub.ai.protocol.ThinkingFormat
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.int
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * 「输出被长度上限截断时**接着写**」（用户建议 ②：不要强行截断 AI 回复）。
 *
 * 纯逻辑部分都在这里钉住：怎么判断"这一轮是被截断的"、续写开头怎么去重、
 * 以及 Anthropic 那边 `max_tokens` 到底发多少（它以前写死 4096，长回复必然被截断）。
 */
class OutputContinuationTest {

    // --- 判据：哪些 finish_reason 表示"被输出长度截断" ------------------------

    @Test
    fun `三家协议的截断结束原因都认得出来`() {
        // OpenAI 兼容是 length，Anthropic 是 max_tokens，Responses 是 max_output_tokens
        assertTrue(HarnessRunner.isTruncatedByOutputLimit("length"))
        assertTrue(HarnessRunner.isTruncatedByOutputLimit("max_tokens"))
        assertTrue(HarnessRunner.isTruncatedByOutputLimit("max_output_tokens"))
        // 大小写不该影响判断（有的网关回 MAX_TOKENS）
        assertTrue(HarnessRunner.isTruncatedByOutputLimit("MAX_TOKENS"))
    }

    @Test
    fun `正常结束与未知原因都不算截断`() {
        assertFalse(HarnessRunner.isTruncatedByOutputLimit("stop"))
        assertFalse(HarnessRunner.isTruncatedByOutputLimit("tool_calls"))
        assertFalse(HarnessRunner.isTruncatedByOutputLimit("end_turn"))
        assertFalse(HarnessRunner.isTruncatedByOutputLimit("incomplete"))
        assertFalse(HarnessRunner.isTruncatedByOutputLimit(null))
    }

    // --- 续写开头去重 ---------------------------------------------------------

    @Test
    fun `续写开头重说了上一轮的结尾时只去掉重复的那一段`() {
        val said = "第一步先看工作流；第二步改参数"
        // 模型把最后一句重念了一遍才接着写
        val next = "第二步改参数，第三步提交"
        assertEquals("，第三步提交", next.substring(HarnessRunner.longestSuffixOverlap(said, next)))
    }

    @Test
    fun `没有重叠时一个字都不动`() {
        assertEquals(0, HarnessRunner.longestSuffixOverlap("前面说过的话", "完全不同的下文"))
        assertEquals(0, HarnessRunner.longestSuffixOverlap(null, "随便什么"))
        assertEquals(0, HarnessRunner.longestSuffixOverlap("前面说过的话", ""))
    }

    @Test
    fun `只在查重窗口内找重叠 不会为长巧合截掉正文`() {
        val long = "甲".repeat(HarnessRunner.OVERLAP_WINDOW + 40)
        // 完全重复也最多认 OVERLAP_WINDOW 个字符
        assertEquals(HarnessRunner.OVERLAP_WINDOW, HarnessRunner.longestSuffixOverlap(long, long))
    }

    // --- Anthropic 的 max_tokens（必填字段）----------------------------------

    private val anthropic = AnthropicMessagesAdapter()

    private fun anthropicMaxTokens(
        maxOutputTokens: Int?,
        reasoning: ReasoningRequest = ReasoningRequest.NONE,
    ) = anthropic
        .buildBody("claude", listOf(ChatTurn("user", "hi")), stream = true, reasoning = reasoning, maxOutputTokens = maxOutputTokens)["max_tokens"]!!
        .let { (it as JsonPrimitive).int }

    @Test
    fun `Anthropic 用模型声明的输出上限 不再写死 4096`() {
        assertEquals(8192, anthropicMaxTokens(8192))
        // 模型没声明时仍然回落 4096（保持与老行为一致，宁可被截断也不要 400）
        assertEquals(AnthropicMessagesAdapter.DEFAULT_MAX_TOKENS, anthropicMaxTokens(null))
        assertEquals(AnthropicMessagesAdapter.DEFAULT_MAX_TOKENS, anthropicMaxTokens(0))
    }

    @Test
    fun `Anthropic 开着思考时 max_tokens 必须严格大于思考预算`() {
        val thinking = ReasoningRequest(
            effort = ReasoningEffort.HIGH,
            wireValue = "high",
            format = ThinkingFormat.OPENAI,
            budgetTokens = 16_000,
        )
        val maxTokens = anthropicMaxTokens(4096, thinking)
        assertTrue(maxTokens > 16_000, "max_tokens=$maxTokens 必须严格大于思考预算，否则上游直接 400")
    }

    // --- Responses：只有声明过才带 max_output_tokens（避免被判超限 400）-------

    @Test
    fun `Responses 只在模型声明输出上限时才带 max_output_tokens`() {
        val responses = OpenAiResponsesAdapter()
        val turns = listOf(ChatTurn("user", "hi"))
        val withCap = responses.buildBody("gpt-5", turns, true, maxOutputTokens = 2048)
        assertEquals(2048, (withCap["max_output_tokens"] as JsonPrimitive).int)

        val without = responses.buildBody("gpt-5", turns, true)
        assertFalse(without.containsKey("max_output_tokens"), "没声明就别发这个字段")
    }
}


