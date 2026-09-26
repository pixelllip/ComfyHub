package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * 思考强度声明的领域校验 + token 用量归一化（AIH-056 / AIH-057）。
 *
 * 这两件事都在"用户能看到什么"和"上游收到什么"之间，出错的代价分别是 400 和账单对不上，
 * 所以规则都放在纯函数里单独测。
 */
class ThinkingAndUsageTest {

    private fun model(
        reasoning: Boolean = true,
        efforts: Map<String, String> = emptyMap(),
        format: String? = null,
    ) = AiModelDto(
        providerId = "p",
        id = "m",
        displayName = "M",
        reasoning = reasoning,
        thinkingEfforts = efforts,
        thinkingFormat = format,
    )

    // --- 思考等级校验 ------------------------------------------------------

    @Test
    fun `off 与空请求都返回 null 不发思考参数`() {
        assertNull(AiValidation.requireThinkingEffort(model(), null))
        assertNull(AiValidation.requireThinkingEffort(model(), com.comfyhub.ai.protocol.ReasoningEffort.OFF))
    }

    @Test
    fun `模型没声明推理能力时拒绝任何强度`() {
        val e = assertFailsWith<AiException> {
            AiValidation.requireThinkingEffort(
                model(reasoning = false),
                com.comfyhub.ai.protocol.ReasoningEffort.HIGH,
            )
        }
        assertEquals(AiErrorCode.CONFIG_ERROR, e.code)
        assertTrue(e.message!!.contains("未声明推理能力"))
    }

    @Test
    fun `声明了推理能力但没列档位时 允许标准档位`() {
        val r = AiValidation.requireThinkingEffort(
            model(efforts = emptyMap()),
            com.comfyhub.ai.protocol.ReasoningEffort.MEDIUM,
        )
        assertEquals(com.comfyhub.ai.protocol.ReasoningEffort.MEDIUM, r)
    }

    @Test
    fun `列了档位时只允许列出来的那几个`() {
        val m = model(efforts = mapOf("off" to "none", "low" to "low", "high" to "high"))
        assertEquals(
            com.comfyhub.ai.protocol.ReasoningEffort.LOW,
            AiValidation.requireThinkingEffort(m, com.comfyhub.ai.protocol.ReasoningEffort.LOW),
        )
        val e = assertFailsWith<AiException> {
            AiValidation.requireThinkingEffort(m, com.comfyhub.ai.protocol.ReasoningEffort.MAX)
        }
        assertTrue(e.message!!.contains("未声明思考强度 max"))
    }

    @Test
    fun `off 不算可选档位`() {
        assertEquals(
            setOf(com.comfyhub.ai.protocol.ReasoningEffort.LOW),
            AiValidation.parseThinkingEfforts(mapOf("off" to "none", "low" to "low")),
        )
    }

    // --- 目录保存校验 ------------------------------------------------------

    @Test
    fun `未知等级或空表达在保存时就被拦下`() {
        val bad1 = model(efforts = mapOf("ultra" to "ultra"))
        assertFailsWith<AiException> { AiValidation.validateThinkingEfforts(bad1) }

        val bad2 = model(efforts = mapOf("high" to "   "))
        assertFailsWith<AiException> { AiValidation.validateThinkingEfforts(bad2) }

        val bad3 = model(format = "nope")
        assertFailsWith<AiException> { AiValidation.validateThinkingEfforts(bad3) }
    }

    @Test
    fun `声明了档位却没勾推理 两者必须一致`() {
        val bad = model(reasoning = false, efforts = mapOf("high" to "high"))
        val e = assertFailsWith<AiException> { AiValidation.validateThinkingEfforts(bad) }
        assertTrue(e.message!!.contains("支持推理"))
    }

    @Test
    fun `合法声明可以通过 数字表达也行`() {
        AiValidation.validateThinkingEfforts(
            model(
                efforts = mapOf("low" to "low", "medium" to "4096"),
                format = "deepseek",
            )
        )
    }

    // --- token 用量归一化 --------------------------------------------------

    private fun usage(json: String) =
        TokenUsage.from(Json.parseToJsonElement(json))

    @Test
    fun `openai 方言的 usage 被归一化`() {
        val u = usage(
            """{"prompt_tokens":120,"completion_tokens":34,"total_tokens":154,
                "prompt_tokens_details":{"cached_tokens":64},
                "completion_tokens_details":{"reasoning_tokens":12}}"""
        )
        assertEquals(120, u.inputTokens)
        assertEquals(34, u.outputTokens)
        assertEquals(64, u.cachedTokens)
        assertEquals(12, u.reasoningTokens)
        assertEquals(154, u.totalTokens)
    }

    @Test
    fun `anthropic 方言的 usage 也能认出来`() {
        val u = usage("""{"input_tokens":88,"output_tokens":9,"cache_read_input_tokens":30}""")
        assertEquals(88, u.inputTokens)
        assertEquals(9, u.outputTokens)
        assertEquals(30, u.cachedTokens)
        assertEquals(97, u.totalTokens)
    }

    @Test
    fun `只给 total 的网关不会把总数算丢`() {
        val u = usage("""{"total_tokens":500}""")
        assertEquals(500, u.totalTokens)
    }

    @Test
    fun `没有 usage 或坏数据都当成 0 而不是抛异常`() {
        assertTrue(TokenUsage.from(null).isEmpty)
        assertTrue(usage("""{"prompt_tokens":"abc"}""").isEmpty)
        assertTrue(TokenUsage.from(JsonPrimitive("nope")).isEmpty)
    }

    @Test
    fun `旧消息里存的供应商原始对象也能回算`() {
        // 后端历史上是把供应商原始 usage 原样存库的，这里保证历史会话的统计不会变成 0
        val raw: JsonObject = buildJsonObject {
            put("prompt_tokens", 10)
            put("completion_tokens", 20)
        }
        val u = TokenUsage.from(raw)
        assertEquals(30, u.totalTokens)
    }
}
