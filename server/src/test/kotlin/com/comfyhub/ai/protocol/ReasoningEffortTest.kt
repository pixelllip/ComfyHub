package com.comfyhub.ai.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * 思考强度（AIH-056）的协议契约测试。
 *
 * 这里守的是三条底线：
 *  1. **模型没声明推理能力 → 一个思考字段都不发**（否则网关 400）；
 *  2. 同一个等级在不同方言落到**各自正确的字段**；
 *  3. 关闭思考时，只有"默认会思考"的方言才显式发禁用，OpenAI 方言什么都不发。
 */
class ReasoningEffortTest {

    private val openai = OpenAiCompletionsAdapter()
    private val anthropic = AnthropicMessagesAdapter()

    private fun turns() = listOf(ChatTurn("user", "你好"))

    private fun req(
        effort: ReasoningEffort?,
        format: ThinkingFormat = ThinkingFormat.OPENAI,
        levelMap: Map<String, LevelSpec> = emptyMap(),
        declared: Boolean = true,
    ) = ReasoningRequest.of(effort, declaredReasoning = declared, levelMap = levelMap, format = format)

    // --- 等级 / 方言解析 ---------------------------------------------------

    @Test
    fun `等级解析大小写不敏感 未知值返回 null 而不是猜`() {
        assertEquals(ReasoningEffort.HIGH, ReasoningEffort.parse("HIGH"))
        assertEquals(ReasoningEffort.OFF, ReasoningEffort.parse(" off "))
        assertNull(ReasoningEffort.parse("ultra"))
        assertNull(ReasoningEffort.parse(null))
        assertNull(ReasoningEffort.parse(""))
        assertNull(ThinkingFormat.parse("nope"))
        assertEquals(ThinkingFormat.DEEPSEEK, ThinkingFormat.parse("DeepSeek"))
    }

    @Test
    fun `模型没声明推理能力时退回 NONE 一个字段都不发`() {
        val r = req(ReasoningEffort.HIGH, format = ThinkingFormat.DEEPSEEK, declared = false)
        assertEquals(ReasoningRequest.NONE, r)
        assertFalse(r.enabled)

        // 连"禁用"都不发：不支持思考的模型上不该出现任何思考字段
        val body = openai.buildBody("m", turns(), true, r)
        assertFalse(body.containsKey("reasoning_effort"))
        assertFalse(body.containsKey("thinking"))
        assertFalse(body.containsKey("enable_thinking"))
    }

    @Test
    fun `请求 off 或 null 等于不思考 但保留方言信息`() {
        assertNull(req(ReasoningEffort.OFF).effort)
        assertNull(req(null).effort)
        assertFalse(req(ReasoningEffort.OFF).enabled)
        // 方言不能丢：DeepSeek 这类网关靠它才能发"禁用"
        assertEquals(ThinkingFormat.DEEPSEEK, req(ReasoningEffort.OFF, ThinkingFormat.DEEPSEEK).format)
    }

    // --- OpenAI 兼容方言 ---------------------------------------------------

    @Test
    fun `openai 方言直接发 reasoning_effort`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.HIGH))
        assertEquals("high", body["reasoning_effort"]!!.jsonPrimitive.content)
        assertFalse(body.containsKey("thinking"))
    }

    @Test
    fun `等级可以在模型目录里改名`() {
        val body = openai.buildBody(
            "m", turns(), true,
            req(ReasoningEffort.MAX, levelMap = mapOf("max" to LevelSpec.Name("ultra")))
        )
        assertEquals("ultra", body["reasoning_effort"]!!.jsonPrimitive.content)
    }

    @Test
    fun `deepseek 方言同时发 thinking 与 reasoning_effort 关闭时发 disabled`() {
        val on = openai.buildBody("m", turns(), true, req(ReasoningEffort.MEDIUM, ThinkingFormat.DEEPSEEK))
        assertEquals("enabled", on["thinking"]!!.jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals("medium", on["reasoning_effort"]!!.jsonPrimitive.content)

        val off = openai.buildBody("m", turns(), true, req(ReasoningEffort.OFF, ThinkingFormat.DEEPSEEK))
        assertEquals("disabled", off["thinking"]!!.jsonObject["type"]!!.jsonPrimitive.content)
        assertFalse(off.containsKey("reasoning_effort"), "关闭时不该带 effort 值")
    }

    @Test
    fun `qwen 方言用 enable_thinking`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.LOW, ThinkingFormat.QWEN))
        assertEquals(true, on(body, "enable_thinking"))
        assertEquals("low", body["reasoning_effort"]!!.jsonPrimitive.content)
    }

    @Test
    fun `zai 方言带 clear_thinking 关闭时把 thinking 置为 disabled`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.MAX, ThinkingFormat.ZAI))
        val thinking = body["thinking"]!!.jsonObject
        assertEquals("enabled", thinking["type"]!!.jsonPrimitive.content)
        assertEquals(false, thinking["clear_thinking"]!!.jsonPrimitive.content.toBoolean())

        val off = openai.buildBody("m", turns(), true, req(ReasoningEffort.OFF, ThinkingFormat.ZAI))
        assertEquals("disabled", off["thinking"]!!.jsonObject["type"]!!.jsonPrimitive.content)
    }

    @Test
    fun `openrouter 方言用 reasoning 对象 关闭时是 none`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.HIGH, ThinkingFormat.OPENROUTER))
        assertEquals("high", body["reasoning"]!!.jsonObject["effort"]!!.jsonPrimitive.content)

        val off = openai.buildBody("m", turns(), true, req(ReasoningEffort.OFF, ThinkingFormat.OPENROUTER))
        assertEquals("none", off["reasoning"]!!.jsonObject["effort"]!!.jsonPrimitive.content)
    }

    @Test
    fun `openai 方言关闭思考时什么都不发 不冒险发 none`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.OFF, ThinkingFormat.OPENAI))
        assertFalse(body.containsKey("reasoning_effort"))
        assertFalse(body.containsKey("reasoning"))
        assertFalse(body.containsKey("thinking"))
        assertFalse(body.containsKey("enable_thinking"))
    }

    // --- Anthropic ---------------------------------------------------------

    @Test
    fun `anthropic 用 thinking 预算 且 max_tokens 必须大于预算`() {
        val body = anthropic.buildBody("claude", turns(), true, req(ReasoningEffort.HIGH))
        val thinking = body["thinking"]!!.jsonObject
        assertEquals("enabled", thinking["type"]!!.jsonPrimitive.content)
        val budget = thinking["budget_tokens"]!!.jsonPrimitive.content.toInt()
        val maxTokens = body["max_tokens"]!!.jsonPrimitive.content.toInt()
        assertEquals(16384, budget)
        assertTrue(maxTokens > budget, "max_tokens($maxTokens) 必须严格大于预算($budget)")

        // 数字形式的等级表达可以直接指定预算
        val custom = anthropic.buildBody(
            "claude", turns(), true,
            req(ReasoningEffort.MEDIUM, levelMap = mapOf("medium" to LevelSpec.Budget(4000)))
        )
        assertEquals(4000, custom["thinking"]!!.jsonObject["budget_tokens"]!!.jsonPrimitive.content.toInt())
        assertTrue(custom["max_tokens"]!!.jsonPrimitive.content.toInt() > 4000)
    }

    @Test
    fun `anthropic 关闭思考时不带 thinking 且 max_tokens 回到默认`() {
        val body = anthropic.buildBody("claude", turns(), true, req(ReasoningEffort.OFF))
        assertFalse(body.containsKey("thinking"))
        assertEquals(4096, body["max_tokens"]!!.jsonPrimitive.content.toInt())
    }

    @Test
    fun `anthropic 预算过小时被抬到最小值 避免上游直接拒绝`() {
        val body = anthropic.buildBody(
            "claude", turns(), true,
            req(ReasoningEffort.LOW, levelMap = mapOf("low" to LevelSpec.Budget(16)))
        )
        assertEquals(1024, body["thinking"]!!.jsonObject["budget_tokens"]!!.jsonPrimitive.content.toInt())
    }

    // --- 等级表达解析 ------------------------------------------------------

    @Test
    fun `等级表达可以是名字或正数 负数与空值都不接受`() {
        assertEquals(LevelSpec.Name("ultra"), LevelSpec.of("ultra"))
        assertEquals(LevelSpec.Budget(2048), LevelSpec.of(" 2048 "))
        assertNull(LevelSpec.of("0"))
        assertNull(LevelSpec.of("-5"))
        assertNull(LevelSpec.of("  "))
        assertNull(LevelSpec.of(null))
    }

    @Test
    fun `默认等级表与实现一致`() {
        assertEquals("none", ThinkingLevels.DEFAULT[ReasoningEffort.OFF])
        assertEquals("high", ThinkingLevels.DEFAULT[ReasoningEffort.MAX])
        assertEquals(2048, ThinkingLevels.ANTHROPIC_BUDGET[ReasoningEffort.LOW])
    }

    private fun on(body: kotlinx.serialization.json.JsonObject, key: String): Boolean =
        body[key]!!.jsonPrimitive.content.toBoolean()

    @Test
    fun `消息体本身不受思考设置影响`() {
        val body = openai.buildBody("m", turns(), true, req(ReasoningEffort.HIGH))
        assertEquals(1, body["messages"]!!.jsonArray.size)
        assertEquals(true, body["stream"]!!.jsonPrimitive.content.toBoolean())
    }
}
