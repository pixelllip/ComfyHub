package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 严格分类器回归（AIH-027）。
 *
 * 最重要的一条：**认不出来必须是 UNKNOWN**，绝不能像画廊老逻辑那样回退成 IMAGE ——
 * 那等于把未知文件当图片发给上游模型。
 */
class FileKindDetectorTest {

    private fun bytes(vararg v: Int) = v.map { it.toByte() }.toByteArray()

    private fun head(prefix: ByteArray, size: Int = 64): ByteArray =
        prefix + ByteArray((size - prefix.size).coerceAtLeast(0))

    @Test
    fun `按魔数识别常见图片`() {
        assertEquals(
            "image/png",
            FileKindDetector.detect(head(bytes(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)), "a.png").mimeType
        )
        assertEquals(
            FileKind.IMAGE,
            FileKindDetector.detect(head(bytes(0xFF, 0xD8, 0xFF, 0xE0)), "a.jpg").kind
        )
        assertEquals(
            "image/gif",
            FileKindDetector.detect(head("GIF89a".toByteArray()), "a.gif").mimeType
        )
        assertEquals(
            "image/webp",
            FileKindDetector.detect(head("RIFF____WEBPVP8 ".toByteArray()), "a.webp").mimeType
        )
        assertEquals(
            FileKind.IMAGE,
            FileKindDetector.detect(head(bytes(0x42, 0x4D)), "a.bmp").kind
        )
    }

    @Test
    fun `扩展名撒谎也没用 签名优先`() {
        // 内容是 PNG，名字却叫 .txt —— 以内容为准
        val png = head(bytes(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A))
        val detected = FileKindDetector.detect(png, "definitely-not-an-image.txt")
        assertEquals(FileKind.IMAGE, detected.kind)
        assertEquals("image/png", detected.mimeType)

        // 内容是纯文本，名字却叫 .png —— 不按扩展名当图片
        val text = "hello, this is just text\n".repeat(4).toByteArray()
        val asPng = FileKindDetector.detect(text, "sneaky.png", "image/png")
        assertEquals(FileKind.UNKNOWN, asPng.kind, "签名对不上时不能因为扩展名就相信它是图片")
    }

    @Test
    fun `ISO BMFF 按品牌区分图片和视频`() {
        val mp4 = head("\u0000\u0000\u0000\u0018ftypisom".toByteArray())
        assertEquals(FileKind.VIDEO, FileKindDetector.detect(mp4, "a.mp4").kind)
        assertEquals("video/mp4", FileKindDetector.detect(mp4, "a.mp4").mimeType)

        val avif = head("\u0000\u0000\u0000\u0018ftypavif".toByteArray())
        assertEquals(FileKind.IMAGE, FileKindDetector.detect(avif, "a.avif").kind)
        assertEquals("image/avif", FileKindDetector.detect(avif, "a.avif").mimeType)
    }

    @Test
    fun `识别音频与文档`() {
        assertEquals(FileKind.AUDIO, FileKindDetector.detect(head("ID3\u0003".toByteArray()), "a.mp3").kind)
        assertEquals(FileKind.AUDIO, FileKindDetector.detect(head("fLaC".toByteArray()), "a.flac").kind)
        assertEquals(FileKind.AUDIO, FileKindDetector.detect(head("OggS".toByteArray()), "a.ogg").kind)
        assertEquals(
            "audio/wav",
            FileKindDetector.detect(head("RIFF____WAVEfmt ".toByteArray()), "a.wav").mimeType
        )
        assertEquals(
            "application/pdf",
            FileKindDetector.detect(head("%PDF-1.7".toByteArray()), "a.pdf").mimeType
        )
    }

    @Test
    fun `文本类需要内容嗅探 且带 NUL 的文本扩展名不放过`() {
        // 纯文本不能补零填充（NUL 会被正确地判成二进制）
        val md = "hello world\n".repeat(8).toByteArray()
        assertEquals(FileKind.TEXT, FileKindDetector.detect(md, "notes.md").kind)
        // 二进制内容 + 文本扩展名 → 不认
        val binary = head(bytes(0x00, 0x01, 0x02, 0x03), 64)
        assertEquals(FileKind.UNKNOWN, FileKindDetector.detect(binary, "notes.txt").kind)
    }

    @Test
    fun `未知类型一律 UNKNOWN 且 modality 为空`() {
        val weird = head(bytes(0x13, 0x37, 0x7A, 0x01))
        val detected = FileKindDetector.detect(weird, "mystery.bin", "application/octet-stream")
        assertEquals(FileKind.UNKNOWN, detected.kind)
        assertNull(detected.modality, "未知类型的模态必须是空，调用方据此阻断")
        assertTrue(detected.blocked)
    }

    @Test
    fun `空文件不崩 且判为 UNKNOWN`() {
        val detected = FileKindDetector.detect(ByteArray(0), "empty.png", "image/png")
        assertEquals(FileKind.UNKNOWN, detected.kind)
    }

    @Test
    fun `前端谎报 image 也骗不过准入 判定权在后端`() {
        // 前端说这是 image/png，但真实内容是纯文本
        val text = "just some text, not an image\n".repeat(4).toByteArray()
        val resolved = StrictIntake.resolve(
            name = "fake.png",
            declaredModality = "image",
            declaredMime = "image/png",
            sizeBytes = text.size.toLong(),
            pixels = null,
            head = text,
        )
        assertTrue(resolved.blockers.isNotEmpty(), "签名不匹配时必须给出阻断理由")
        assertNull(resolved.fact.modality, "不能沿用前端声明的 image")

        val model = AiModelDto(
            providerId = "p", id = "m", displayName = "视觉模型",
            inputModalities = listOf("text", "image"),
            attachmentTransports = mapOf("image" to listOf("inline_base64")),
        )
        val admission = AttachmentPolicy.evaluate(
            model, resolved.fact, setOf(Transport.INLINE_BASE64)
        )
        assertTrue(!admission.allowed, "未知类型即使模型支持图片也必须阻断")
    }

    @Test
    fun `有文件头时以签名判定为准 前端声明只是线索`() {
        val png = head(bytes(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A))
        val resolved = StrictIntake.resolve(
            name = "whatever.dat",
            declaredModality = "document", // 前端声明错了
            declaredMime = "application/octet-stream",
            sizeBytes = 2048,
            pixels = null,
            head = png,
        )
        assertTrue(resolved.blockers.isEmpty())
        assertEquals(Modality.IMAGE, resolved.fact.modality, "以签名判定为准")
        assertEquals("image/png", resolved.fact.mimeType)
    }

    @Test
    fun `没有文件头时按声明放行但不下结论`() {
        val resolved = StrictIntake.resolve(
            name = "a.png",
            declaredModality = "image",
            declaredMime = "image/png",
            sizeBytes = 100,
            pixels = null,
            head = null,
        )
        assertEquals(Modality.IMAGE, resolved.fact.modality)
        assertTrue(resolved.blockers.isEmpty())
    }

    @Test
    fun `分类器产出的模态能直接喂给附件准入`() {
        val model = AiModelDto(
            providerId = "p", id = "m", displayName = "纯文本模型",
            inputModalities = listOf("text"),
        )
        val png = FileKindDetector.detect(
            head(bytes(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)), "a.png"
        )
        val fact = AttachmentFact("a.png", png.modality, png.mimeType!!, 2048)
        val result = AttachmentPolicy.evaluate(model, fact, setOf(Transport.INLINE_BASE64))
        assertTrue(!result.allowed, "纯文本模型 + 图片必须被阻断")
    }
}
