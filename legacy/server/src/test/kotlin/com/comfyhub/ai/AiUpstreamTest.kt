package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * Provider 连接测试与错误分类（AIH-008 / AIH-024）。
 * 常规测试**不访问真实供应商**：状态码映射是纯函数，网络那一条只打本机必然拒绝的端口。
 */
class AiUpstreamTest {

    @Test
    fun `状态码映射到稳定错误码`() {
        assertNull(AiUpstream.classifyStatus(200))
        assertNull(AiUpstream.classifyStatus(204))
        assertEquals(AiErrorCode.MISSING_CREDENTIAL, AiUpstream.classifyStatus(401))
        assertEquals(AiErrorCode.MISSING_CREDENTIAL, AiUpstream.classifyStatus(403))
        assertEquals(AiErrorCode.PROTOCOL_ERROR, AiUpstream.classifyStatus(404))
        assertEquals(AiErrorCode.RATE_LIMIT, AiUpstream.classifyStatus(429))
        assertEquals(AiErrorCode.QUOTA_EXCEEDED, AiUpstream.classifyStatus(402))
        assertEquals(AiErrorCode.CONFIG_ERROR, AiUpstream.classifyStatus(422))
        assertEquals(AiErrorCode.PROVIDER_UNREACHABLE, AiUpstream.classifyStatus(500))
        assertEquals(AiErrorCode.PROVIDER_UNREACHABLE, AiUpstream.classifyStatus(504))
    }

    @Test
    fun `列表 URL 按协议构造 不擅自改写用户路径`() {
        assertEquals(
            "https://gw.example/v1/models",
            AiUpstream.modelsUrl("https://gw.example/v1", AiApi.OPENAI_COMPLETIONS)
        )
        assertEquals(
            "https://gw.example/anthropic/v1/models",
            AiUpstream.modelsUrl("https://gw.example/anthropic/v1", AiApi.ANTHROPIC_MESSAGES)
        )
        // Anthropic 兼容网关常常不给 /v1，列表 URL 才补上
        assertEquals(
            "https://api.anthropic.com/v1/models",
            AiUpstream.modelsUrl("https://api.anthropic.com", AiApi.ANTHROPIC_MESSAGES)
        )
    }

    @Test
    fun `模型列表地址会回退到另一种 v1 写法`() {
        // Base URL 填不填 /v1 都能用：两个候选都要在，首选仍然是原来那个
        assertEquals(
            listOf("https://api.openai.com/v1/models", "https://api.openai.com/models"),
            AiUpstream.modelsUrlCandidates("https://api.openai.com/v1", AiApi.OPENAI_RESPONSES)
        )
        assertEquals(
            listOf("https://api.openai.com/models", "https://api.openai.com/v1/models"),
            AiUpstream.modelsUrlCandidates("https://api.openai.com", AiApi.OPENAI_COMPLETIONS)
        )
        assertEquals(
            listOf("https://gw.example/v1/models", "https://gw.example/models"),
            AiUpstream.modelsUrlCandidates("https://gw.example/v1", AiApi.ANTHROPIC_MESSAGES)
        )
    }

    @Test
    fun `模型数量解析兼容 data 数组与 models 对象`() {
        assertEquals(2, AiUpstream.countModels("""{"data":[{"id":"a"},{"id":"b"}]}""", AiApi.OPENAI_COMPLETIONS))
        assertEquals(1, AiUpstream.countModels("""{"models":[{"id":"a"}]}""", AiApi.ANTHROPIC_MESSAGES))
        assertNull(AiUpstream.countModels("not json", AiApi.OPENAI_COMPLETIONS))
    }

    @Test
    fun `引用了凭据但本机没配置时 直接报 MISSING_CREDENTIAL 不发请求`() {
        val provider = AiProviderDto(
            id = "p", displayName = "p", api = "openai-completions",
            baseURL = "http://127.0.0.1:1/v1", credentialRef = "MY_KEY",
            endpointTrust = "loopback",
        )
        val result = AiUpstream.testConnection(provider, null)
        assertTrue(!result.ok)
        assertEquals(AiErrorCode.MISSING_CREDENTIAL, result.errorCode)
        assertNull(result.httpStatus)
    }

    @Test
    fun `不可达端点归到 PROVIDER_UNREACHABLE 且消息里没有密钥`() {
        // 本机 1 端口必然拒绝连接；不会真的碰到外网
        val provider = AiProviderDto(
            id = "p", displayName = "p", api = "openai-completions",
            baseURL = "http://127.0.0.1:1/v1", credentialRef = null,
            endpointTrust = "loopback",
        )
        val result = AiUpstream.testConnection(provider, "sk-should-never-appear")
        assertTrue(!result.ok)
        assertEquals(AiErrorCode.PROVIDER_UNREACHABLE, result.errorCode)
        assertTrue(!result.message.contains("sk-should-never-appear"), "错误消息里绝对不能带密钥")
        assertTrue(!result.message.contains("Bearer"), "错误消息里不能带认证头")
    }

    @Test
    fun `SSRF：公网级别不允许指向本机`() {
        val provider = AiProviderDto(
            id = "p", displayName = "p", api = "openai-completions",
            baseURL = "http://localhost:8080/v1", credentialRef = null,
            endpointTrust = "public",
        )
        val result = AiUpstream.testConnection(provider, "sk-x")
        assertTrue(!result.ok)
        assertEquals(AiErrorCode.CONFIG_ERROR, result.errorCode)
    }
}
