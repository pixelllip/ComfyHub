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
    fun `发现不推断能力 候选里根本没有能力字段`() {
        val candidate = AiUpstream.ModelCandidate("m1", "M1")
        assertNull(candidate.contextWindow)
        assertNull(candidate.maxOutputTokens)
        // 能力必须由用户在模型目录里显式声明（AIH-011）：候选结构里不该出现任何能力字段
        val names = AiUpstream.ModelCandidate::class.members.map { it.name }.toSet()
        assertTrue(
            names.none { it.contains("modal", true) || it.contains("tool", true) || it.contains("image", true) },
            "候选模型不应携带能力字段：$names"
        )
    }
}
