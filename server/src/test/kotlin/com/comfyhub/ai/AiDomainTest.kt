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
        // 真实来源：适配器**没有**实现视频的任何传输方式（见 Adapters.attachmentTransports）
        val r = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.mp4", Modality.VIDEO, "video/mp4", 1024),
            adapterTransports = AdapterCapabilities.transportsFor(AiApi.OPENAI_COMPLETIONS, Modality.VIDEO),
        )
        assertFalse(r.allowed)
        assertTrue(r.blockers.any { it.contains("尚未实现") }, r.blockers.toString())
    }

    @Test
    fun `模型没声明传输方式时用协议实现的那种（内置目录只声明模态）`() {
        // 内置目录里的 69 个模型只声明 inputModalities，不声明 attachmentTransports；
        // 传输方式是**协议**的属性，所以这里必须回落到适配器实现的那种，否则图片永远发不出去。
        val model = textOnlyModel().copy(inputModalities = listOf("text", "image"))
        val image = AdapterCapabilities.transportsFor(AiApi.OPENAI_COMPLETIONS, Modality.IMAGE)
        assertEquals(setOf(Transport.INLINE_BASE64), image)

        val ok = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.png", Modality.IMAGE, "image/png", 1024),
            adapterTransports = image,
        )
        assertTrue(ok.allowed, ok.blockers.toString())
    }

    @Test
    fun `模型显式声明了传输方式就只认它 声明的没实现照样阻断`() {
        val model = textOnlyModel().copy(
            inputModalities = listOf("text", "image"),
            // 只声明远程 URL：当前适配器只实现了内联 base64 → 交集为空 → 阻断
            attachmentTransports = mapOf("image" to listOf("remote_url")),
        )
        val r = AttachmentPolicy.evaluate(
            model, AttachmentFact("a.png", Modality.IMAGE, "image/png", 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(r.allowed)
        assertTrue(r.blockers.any { it.contains("remote_url") }, r.blockers.toString())
    }

    @Test
    fun `内联超过 8MB 的图先压缩再发 不把网关打成 413`() {
        val model = textOnlyModel().copy(inputModalities = listOf("text", "image"))
        val r = AttachmentPolicy.evaluate(
            model, AttachmentFact("huge.png", Modality.IMAGE, "image/png", 9 * 1024 * 1024),
            adapterTransports = setOf(Transport.INLINE_BASE64),
        )
        assertFalse(r.allowed)
        assertTrue(r.blockers.any { it.contains("内联发送上限") }, r.blockers.toString())
    }

    @Test
    fun `附件内联预算：最新优先 装不下就跳过小的顶上`() {
        // 3 张：12MB（超单张上限）/ 6MB（装得下）/ 5MB（总预算 20MB 里只剩 14MB，也装得下）
        val plan = InlineBudget.plan(
            sizes = listOf(12L * 1024 * 1024, 6L * 1024 * 1024, 5L * 1024 * 1024),
            maxTotal = 20L * 1024 * 1024,
            maxSingle = AttachmentPolicy.MAX_INLINE_BYTES,
        )
        assertEquals(listOf(false, true, true), plan)

        // 总预算装不下第三张时如实标 false（调用方要把它变成一句"没随本次请求发送"）
        val tight = InlineBudget.plan(
            sizes = listOf(6L * 1024 * 1024, 6L * 1024 * 1024, 6L * 1024 * 1024),
            maxTotal = 13L * 1024 * 1024,
        )
        assertEquals(listOf(true, true, false), tight)
        assertTrue(InlineBudget.plan(listOf(0L)).none { it }, "空文件不发")
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
