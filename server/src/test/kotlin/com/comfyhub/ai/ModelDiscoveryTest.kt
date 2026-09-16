package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 模型发现（AIH-009）：只产出候选，**不推断能力**。
 * 全部离线，用真实网关会返回的几种 JSON 形态做输入。
 */
class ModelDiscoveryTest {

    @Test
    fun `OpenAI 标准 data 数组`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"object":"list","data":[
              {"id":"gpt-4o-mini","object":"model","owned_by":"openai"},
              {"id":"deepseek-chat","object":"model"}
            ]}
            """.trimIndent()
        )
        assertEquals(2, candidates.size)
        assertEquals("gpt-4o-mini", candidates[0].id)
        assertEquals("gpt-4o-mini", candidates[0].displayName, "没有显示名时退回 id")
    }

    @Test
    fun `兼容 models 对象与富信息字段`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"models":[
              {"name":"claude-3-5-sonnet","display_name":"Claude 3.5 Sonnet","context_window":200000,"max_output_tokens":8192}
            ]}
            """.trimIndent()
        )
        assertEquals(1, candidates.size)
        assertEquals("claude-3-5-sonnet", candidates[0].id)
        assertEquals("Claude 3.5 Sonnet", candidates[0].displayName)
        assertEquals(200000, candidates[0].contextWindow)
        assertEquals(8192, candidates[0].maxOutputTokens)
    }

    @Test
    fun `按 id 去重 且忽略没有 id 的条目`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"data":[{"id":"a"},{"id":"a"},{"object":"model"}]}
            """.trimIndent()
        )
        assertEquals(1, candidates.size)
        assertEquals("a", candidates.single().id)
    }

    @Test
    fun `畸形或空响应不抛异常 只是没有候选`() {
        assertTrue(AiUpstream.parseCandidates("not json").isEmpty())
        assertTrue(AiUpstream.parseCandidates("").isEmpty())
        assertTrue(AiUpstream.parseCandidates("""{"data":[]}""").isEmpty())
    }

    @Test
    fun `不认识的模型不会被猜能力：只给文本并标明未识别`() {
        val candidate = AiUpstream.parseCandidates("""{"data":[{"id":"acme-mystery-9000"}]}""").single()
        assertEquals(listOf("text"), candidate.modalities)
        assertEquals("unknown", candidate.capabilitySource)
        // 工具是唯一的兜底：网关普遍支持但不声明，且请求体还不发 tools（用户要求默认给上）
        assertTrue(candidate.tools, "工具默认给上")
        assertTrue(!candidate.reasoning, "不认识的模型不该被猜成支持思考")
        assertTrue(candidate.thinkingEfforts.isEmpty())
        // 内置目录也查不到，说明确实没有"按名字硬猜"的路径
        assertNull(ModelCapabilityCatalog.lookup("acme-mystery-9000"))
    }

    @Test
    fun `接口只给 id 时回退到内置目录，而不是当成未识别`() {
        // 真实网关最常见的样子：只有 id / object / owned_by
        val candidates = AiUpstream.parseCandidates(
            """
            {"object":"list","data":[
              {"id":"claude-sonnet-5","object":"model","owned_by":"gw"},
              {"id":"gpt-5.6-sol","object":"model","owned_by":"gw"},
              {"id":"deepseek/deepseek-v4.1-flash","object":"model","owned_by":"gw"},
              {"id":"acme-mystery-9000","object":"model","owned_by":"gw"}
            ]}
            """.trimIndent()
        )
        val claude = candidates.first { it.id == "claude-sonnet-5" }
        assertEquals(listOf("text", "image"), claude.modalities)
        assertTrue(claude.reasoning)
        assertEquals(
            setOf("off", "low", "medium", "high", "xhigh", "max"),
            claude.thinkingEfforts.keys,
            "内置目录的档位要跟着下来，否则用户还得手填",
        )
        assertEquals(CapabilitySource.BUILTIN.wire, claude.capabilitySource)

        val gpt = candidates.first { it.id == "gpt-5.6-sol" }
        assertEquals(listOf("text", "image"), gpt.modalities)
        assertTrue(gpt.reasoning)

        val ds = candidates.first { it.id == "deepseek/deepseek-v4.1-flash" }
        assertEquals(listOf("text", "image"), ds.modalities)
        assertEquals("deepseek", ds.thinkingFormat)

        // 两边都没有的仍然只给文本 + 默认工具
        val mystery = candidates.first { it.id == "acme-mystery-9000" }
        assertEquals(listOf("text"), mystery.modalities)
        assertTrue(mystery.tools)
        assertEquals("unknown", mystery.capabilitySource)
    }

    @Test
    fun `接口只声明一部分维度时 其余维度仍由内置目录补`() {
        // 网关只说了"支持函数调用"，没说过模态与思考
        val c = AiUpstream.parseCandidates(
            """{"data":[{"id":"claude-sonnet-5","supports_tools":true}]}"""
        ).single()
        assertTrue(c.tools)
        assertEquals(listOf("text", "image"), c.modalities, "模态该由内置目录补上")
        assertTrue(c.reasoning, "思考支持也该由内置目录补上")
        assertEquals(CapabilitySource.DISCOVERED.wire, c.capabilitySource)
    }

    @Test
    fun `空的 capabilities 对象不再被当成接口声明`() {
        // 旧实现看到 capabilities:{} 就认定"接口说了"，于是内置目录永远轮不到 —— 这里钉死
        val c = AiUpstream.parseCandidates("""{"data":[{"id":"gpt-5.6-sol","capabilities":{}}]}""").single()
        assertEquals(CapabilitySource.BUILTIN.wire, c.capabilitySource)
        assertEquals(listOf("text", "image"), c.modalities)
        assertTrue(c.reasoning)
    }
}
