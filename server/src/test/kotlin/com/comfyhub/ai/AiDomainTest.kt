package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * 阶段 1 规则单测：Provider/URL/SSRF 校验、附件准入、凭据输入校验。
 * 这些都是纯函数，不碰数据库、不碰网络，因此可以稳定覆盖
 * "不支持的附件绝不能触发上游请求"（AIH-030）这条核心不变式。
 */
class AiDomainTest {

    // --- Provider ID / URL -------------------------------------------------

    @Test
    fun `provider id 必须是 kebab-case`() {
        assertEquals("my-gateway", AiValidation.requireProviderId("my-gateway"))
        assertFailsWith<AiException> { AiValidation.requireProviderId("MyGateway") }
        assertFailsWith<AiException> { AiValidation.requireProviderId("my_gateway") }
        assertFailsWith<AiException> { AiValidation.requireProviderId("-bad") }
        assertFailsWith<AiException> { AiValidation.requireProviderId("") }
    }

    @Test
    fun `base url 只去掉末尾斜杠 不改写路径`() {
        val url = AiValidation.normalizeBaseUrl("https://gw.example/anthropic/v1/", EndpointTrust.PUBLIC)
        assertEquals("https://gw.example/anthropic/v1", url)
    }

    @Test
    fun `公网端点必须 https 且不能内嵌账密`() {
        assertFailsWith<AiException> {
            AiValidation.normalizeBaseUrl("http://gw.example/v1", EndpointTrust.PUBLIC)
        }
        assertFailsWith<AiException> {
            AiValidation.normalizeBaseUrl("https://user:pass@gw.example/v1", EndpointTrust.PUBLIC)
        }
    }

    @Test
    fun `本机端点允许 http`() {
        val url = AiValidation.normalizeBaseUrl("http://127.0.0.1:11434/v1", EndpointTrust.LOOPBACK)
        assertEquals("http://127.0.0.1:11434/v1", url)
    }

    // --- SSRF --------------------------------------------------------------

    @Test
    fun `信任级别与地址范围必须匹配`() {
        // 公网级别不允许指向本机
        assertFailsWith<AiException> {
            AiValidation.normalizeBaseUrl("https://localhost/v1", EndpointTrust.PUBLIC)
        }
        // 回环级别不允许指向公网
        assertFailsWith<AiException> {
            AiValidation.normalizeBaseUrl("https://api.example.com/v1", EndpointTrust.LOOPBACK)
        }
        // 私网级别允许 192.168/10/172.16
        assertEquals(
            "http://192.168.1.20:8080/v1",
            AiValidation.normalizeBaseUrl("http://192.168.1.20:8080/v1", EndpointTrust.PRIVATE_NETWORK)
        )
    }

    @Test
    fun `云元数据地址永远拒绝`() {
        assertEquals(Scope.BLOCKED, AddressScope.of("169.254.169.254"))
        assertEquals(Scope.BLOCKED, AddressScope.of("metadata.google.internal"))
        assertFailsWith<AiException> {
            AiValidation.normalizeBaseUrl("http://169.254.169.254/latest", EndpointTrust.UNSAFE_ANY)
        }
    }

    @Test
    fun `IPv6 回环被识别为回环`() {
        assertEquals(Scope.LOOPBACK, AddressScope.of("[::1]"))
        assertEquals(Scope.LOOPBACK, AddressScope.of("::1"))
    }

    // --- 附件准入（AIH-028 / AIH-030） --------------------------------------

    private fun textOnlyModel() = AiModelDto(
        providerId = "p", id = "m", displayName = "纯文本模型",
        inputModalities = listOf("text"),
    )

    @Test
    fun `text-only 模型必须阻断图片且给出原因`() {
        val r = AttachmentPolicy.evaluate(
            model = textOnlyModel(),
            attachment = AttachmentFact("a.png", Modality.IMAGE, "image/png", 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(r.allowed, "text-only 模型不允许发送图片")
        assertTrue(r.blockers.any { it.contains("未声明支持") }, "应说明模型未声明该模态: ${r.blockers}")
    }

    @Test
    fun `模型声明支持但适配器未实现 也要阻断`() {
        val model = textOnlyModel().copy(
            inputModalities = listOf("text", "image"),
            attachmentTransports = mapOf("image" to listOf("inline_base64")),
        )
        val r = AttachmentPolicy.evaluate(
            model = model,
            attachment = AttachmentFact("a.png", Modality.IMAGE, "image/png", 1024),
            adapterTransports = emptySet(), // 适配器还没实现
        )
        assertFalse(r.allowed)
        assertTrue(r.blockers.any { it.contains("适配器尚未实现") }, r.blockers.toString())
    }

    @Test
    fun `能力未知默认拒绝`() {
        val r = AttachmentPolicy.evaluate(
            model = textOnlyModel(),
            attachment = AttachmentFact("weird.bin", null, "application/octet-stream", 10),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(r.allowed, "无法识别类型必须阻断，不能乐观发送")
    }

    @Test
    fun `全部条件满足才放行`() {
        val model = textOnlyModel().copy(
            inputModalities = listOf("text", "image"),
            attachmentTransports = mapOf("image" to listOf("inline_base64")),
            mimeAllowlist = listOf("image/png", "image/jpeg"),
            maxAttachmentBytes = 5 * 1024 * 1024,
            maxAttachmentCount = 3,
        )
        val ok = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.png", Modality.IMAGE, "image/png", 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertTrue(ok.allowed, ok.blockers.toString())

        val tooBig = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.png", Modality.IMAGE, "image/png", 9 * 1024 * 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(tooBig.allowed)
        assertTrue(tooBig.blockers.any { it.contains("大小上限") }, tooBig.blockers.toString())

        val badMime = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.webp", Modality.IMAGE, "image/webp", 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(badMime.allowed)
        assertTrue(badMime.blockers.any { it.contains("不接受") }, badMime.blockers.toString())
    }

    @Test
    fun `视频在未实现时被阻断 绝不静默抽帧`() {
        val model = textOnlyModel().copy(inputModalities = listOf("text", "video"))
        val r = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.mp4", Modality.VIDEO, "video/mp4", 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(r.allowed)
    }

    // --- 凭据 --------------------------------------------------------------

    @Test
    fun `粘贴 NAME=value 或带引号要报格式错误`() {
        val svc = CredentialService(java.nio.file.Path.of(System.getProperty("java.io.tmpdir"), "ch-cred-test"))
        // 正常值
        assertEquals("sk-abc123", svc.sanitize("sk-abc123"))
        assertFailsWith<AiException> { svc.sanitize("OPENAI_API_KEY=sk-abc") }
        assertFailsWith<AiException> { svc.sanitize("\"sk-abc\"") }
        assertFailsWith<AiException> { svc.sanitize("sk abc") }
        assertFailsWith<AiException> { svc.sanitize("sk\nabc") }
        assertFailsWith<AiException> { svc.sanitize("   ") }
    }

    @Test
    fun `env 来源的凭据只读 且 describe 不含值`() {
        val svc = CredentialService(
            java.nio.file.Path.of(System.getProperty("java.io.tmpdir"), "ch-cred-test"),
            env = mapOf("MY_KEY" to "sk-secret"),
        )
        val status = svc.describe("MY_KEY")
        assertTrue(status.configured)
        assertEquals("env", status.source)
        assertFalse(status.writable)
        // DTO 里没有任何字段能装密钥
        assertFalse(status.toString().contains("sk-secret"), "状态 DTO 不能泄漏密钥值")
    }

    @Test
    fun `未配置的凭据状态为 none`() {
        val svc = CredentialService(
            java.nio.file.Path.of(System.getProperty("java.io.tmpdir"), "ch-cred-test"),
            env = emptyMap(),
        )
        val status = svc.describe("NOT_SET_REF")
        assertFalse(status.configured)
        assertEquals("none", status.source)
    }

    // --- 协议枚举 ----------------------------------------------------------

    @Test
    fun `首期只认三种协议 且不含已淘汰的 completions`() {
        assertEquals(AiApi.OPENAI_COMPLETIONS, AiApi.parse("openai-completions"))
        assertEquals(AiApi.OPENAI_RESPONSES, AiApi.parse("openai-responses"))
        assertEquals(AiApi.ANTHROPIC_MESSAGES, AiApi.parse("anthropic-messages"))
        assertEquals(null, AiApi.parse("openai"))
        assertFalse(AiApi.wireValues.contains("completions"))
    }
}
