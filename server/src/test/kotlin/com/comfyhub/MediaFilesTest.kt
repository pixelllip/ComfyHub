package com.comfyhub

import java.awt.image.BufferedImage
import java.nio.file.Files
import java.nio.file.Path
import java.util.Base64
import javax.imageio.ImageIO
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * WebP 支持回归（2026-09 用户要求：附件要能收 webp）。
 *
 * 为什么必须有这条用例：JDK 自带的 ImageIO **读不了 WebP**，所以
 * `build.gradle.kts` 挂了 TwelveMonkeys 的 `imageio-webp`。那是个 ServiceLoader 插件 ——
 * 一旦依赖被删掉、版本回退、或者打 fatJar 时把 `META-INF/services/javax.imageio.spi.ImageReaderSpi`
 * 合并丢了，**编译照样通过**，只是悄悄退化成"webp 没有缩略图"。
 * 这条用例是唯一能抓住那种退化的防线。
 */
class MediaFilesTest {

    /** 2×2 无损 WebP（VP8L），带 alpha —— Pillow 生成，字节写死在用例里。 */
    private val webpLossless2x2 =
        "UklGRjAAAABXRUJQVlA4TCMAAAAvAUAAEB8gEEjeHzqN+RcQFPk/moCg6LrlIk8Owg0YIvofAgA="

    /** 64×64 有损 WebP（VP8），用来验证"会缩放"而不是原样搬运。 */
    private val webpLossy64x64 =
        "UklGRuoAAABXRUJQVlA4IN4AAACQCACdASpAAEAAPm0wkkayIyGhLAgCQA2JYjONegSAAFLTZ+qf5n7AAJJ/waDCo4kd4G3mQ4ftx///U6nCgIpl//99xQExTbx5N92fMAD+/6DU6mCVjkfOhi0I8uNUKj2DdJnq/rAPrkVF243S7MPMrGu8Ul80qyiVfB8x8Hnunp8OP5rWxLluq4jfQAz49c78P9t9P94sotsk6aZc9g6mR4CWpx6trTZcpJYfCfztO7j2rgviRmudIeJaevtpnAKp4RL+rOYWD1eaTGcxWXBtVBitBN4RFD4bIgiYwAA="

    private fun write(name: String, base64: String): Path {
        val dir = Files.createTempDirectory("comfyhub-media-test")
        val path = dir.resolve(name)
        Files.write(path, Base64.getDecoder().decode(base64))
        return path
    }

    private fun isJpeg(path: Path): Boolean {
        val head = Files.newInputStream(path).use { it.readNBytes(2) }
        return head.size == 2 && head[0] == 0xFF.toByte() && head[1] == 0xD8.toByte()
    }

    @Test
    fun `webp 能探测尺寸（无损与有损两种头都认）`() {
        assertEquals(2 to 2, MediaFiles.probeImageSize(write("a.webp", webpLossless2x2)))
        assertEquals(64 to 64, MediaFiles.probeImageSize(write("b.webp", webpLossy64x64)))
    }

    @Test
    fun `webp 能生成 JPEG 缩略图`() {
        val src = write("a.webp", webpLossless2x2)
        val dest = src.resolveSibling("a-thumb.jpg")
        assertTrue(MediaFiles.writeThumbnail(src, dest), "webp 解不了 = ImageIO 插件没生效")
        assertTrue(Files.size(dest) > 0)
        assertTrue(isJpeg(dest), "缩略图必须是 JPEG（前端按 image/jpeg 收）")
    }

    @Test
    fun `webp 缩略图会按 maxEdge 缩放`() {
        val src = write("b.webp", webpLossy64x64)
        val dest = src.resolveSibling("b-thumb.jpg")
        assertTrue(MediaFiles.writeThumbnail(src, dest, maxEdge = 16))
        // 缩略图确实是 JPEG，且按 16 这条边缩到了 16×16（不是把 64×64 原样搬过去）
        assertTrue(isJpeg(dest))
        assertEquals(16 to 16, MediaFiles.probeImageSize(dest))
    }

    /**
     * JPEG 的尺寸探测（2026-09 顺手修掉的老 bug）。
     *
     * 旧实现把 SOI（`FFD8`）也当成"带长度字段的段"：读出长度 `0xFFE0` 后要跳六万多字节，
     * 于是**任何 JPEG 的尺寸探测都返回 null** —— 症状是 jpg 附件的宽高永远是空的
     * （`AttachmentFact.pixels` 也就一直是 null），而 png / webp 却好端端的，很难联想到解析器。
     */
    @Test
    fun `jpeg 尺寸探测（SOI 不是带长度的段）`() {
        val dir = Files.createTempDirectory("comfyhub-media-test")
        val jpg = dir.resolve("a.jpg")
        assertTrue(ImageIO.write(BufferedImage(200, 100, BufferedImage.TYPE_INT_RGB), "jpeg", jpg.toFile()))
        assertEquals(200 to 100, MediaFiles.probeImageSize(jpg))
    }

    @Test
    fun `认不出来的字节不会假装成功`() {
        val dir = Files.createTempDirectory("comfyhub-media-test")
        val src = dir.resolve("broken.webp")
        Files.write(src, "not really a webp".toByteArray())
        val dest = dir.resolve("broken-thumb.jpg")
        assertFalse(MediaFiles.writeThumbnail(src, dest), "解不了必须返回 false，由调用方回退原件")
        assertFalse(Files.exists(dest))
    }
}
