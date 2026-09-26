package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * 上游报错的**细化**与**脱敏**（AIH-024 / AIH-051）。
 *
 * 这两件事都是被真实网关逼出来的：
 *  - 同一个 403，可能是"密钥不对"，也可能是"这个模型不在你的套餐里"；
 *  - 同一个 400，可能是"这个模型要走另一个协议端点"（混合网关很常见）；
 *  - 而且网关常常把请求原文回显，必须先把密钥抹掉再展示。
 */
class UpstreamErrorTest {

    @Test
    fun `套餐里没有这个模型 要报配额而不是密钥`() {
        val body = """{"type":"error","error":{"type":"permission_error","message":"MODEL_NOT_IN_PLAN: Claude Sonnet available in Pro and above plans"}}"""
        val base = AiUpstream.classifyStatus(403)
        assertEquals(AiErrorCode.MISSING_CREDENTIAL, base, "只看状态码会误判成密钥问题")
        assertEquals(AiErrorCode.QUOTA_EXCEEDED, AiUpstream.refineFromBody(base, body))
    }

    @Test
    fun `模型不属于当前协议端点 要报协议错误并提示分协议建 Provider`() {
        val body = """{"error":{"message":"Model \"x\" is not supported on this endpoint. Use /provider/v1/chat/completions for OpenAI"}}"""
        assertEquals(
            AiErrorCode.PROTOCOL_ERROR,
            AiUpstream.refineFromBody(AiUpstream.classifyStatus(400), body)
        )
        assertTrue(AiUpstream.explain(AiErrorCode.PROTOCOL_ERROR, 400).contains("各建一条 Provider"))
    }

    @Test
    fun `真的密钥错误仍然是密钥错误`() {
        val body = """{"error":{"message":"invalid api key"}}"""
        assertEquals(
            AiErrorCode.MISSING_CREDENTIAL,
            AiUpstream.refineFromBody(AiUpstream.classifyStatus(401), body)
        )
    }

    @Test
    fun `看不懂的正文就保留原错误码`() {
        assertEquals(
            AiErrorCode.CONFIG_ERROR,
            AiUpstream.refineFromBody(AiErrorCode.CONFIG_ERROR, """{"error":{"message":"some new thing"}}""")
        )
        assertEquals(AiErrorCode.RATE_LIMIT, AiUpstream.refineFromBody(AiErrorCode.RATE_LIMIT, "{}"))
    }

    @Test
    fun `回显正文里的密钥必须被抹掉`() {
        val secret = "sk-live-abcdef1234567890"
        val raw = """{"error":{"message":"bad key sk-live-abcdef1234567890","authorization":"Bearer $secret"}}"""
        val out = AiUpstream.redact(raw, secret)
        assertFalse(out.contains(secret), "脱敏后不能还带着密钥：$out")
        assertTrue(out.contains("***"))
        assertTrue(out.length <= 400)
    }

    @Test
    fun `没给密钥时也要按模式抹掉 Bearer 与 api key`() {
        val out = AiUpstream.redact("""Authorization: Bearer sk-abcdefghijklmnop api_key=zxcvbnmasdfghjkl""", null)
        assertFalse(out.contains("sk-abcdefghijklmnop"))
        assertFalse(out.contains("zxcvbnmasdfghjkl"))
    }

    @Test
    fun `超长正文会被截断 且压成一行`() {
        val out = AiUpstream.redact("x".repeat(2000) + "\n\n  y", null)
        assertTrue(out.length <= 400)
        assertFalse(out.contains("\n"))
    }

    @Test
    fun `403 的默认说明仍然提醒检查密钥 但配额类会换成套餐文案`() {
        assertTrue(AiUpstream.explain(AiErrorCode.MISSING_CREDENTIAL, 403).contains("API Key"))
        assertTrue(AiUpstream.explain(AiErrorCode.QUOTA_EXCEEDED, 403).contains("套餐"))
    }
}
