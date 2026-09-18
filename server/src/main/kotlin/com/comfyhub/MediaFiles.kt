package com.comfyhub

import org.slf4j.LoggerFactory
import java.awt.RenderingHints
import java.awt.image.BufferedImage
import java.io.BufferedInputStream
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.security.MessageDigest
import javax.imageio.IIOImage
import javax.imageio.ImageIO
import javax.imageio.ImageWriteParam
import kotlin.math.max

/**
 * 文件层面的事情：类型识别、尺寸探测、哈希、缩略图。
 * 不依赖外部二进制（不调用 ffmpeg），保证部署零依赖。
 *
 * 关于 WebP：JDK 自带的 ImageIO **读不了 WebP**，所以 `build.gradle.kts` 里挂了
 * TwelveMonkeys 的 `imageio-webp` —— 纯 Java 的 ImageReaderSpi 插件，不解码就没人能解码，
 * 结果就是"附件/画廊里的 webp 有图没缩略图"（2026-09 用户要求补上 WebP 支持）。
 * 没有解码器的格式（AVIF / HEIC）**不在这里硬造**：调用方按"回退原件"处理
 * （`MediaRoutes` 的 `/thumb`、`AiAttachmentStore.thumbnailOf`），让 Flutter 自己解码。
 */
object MediaFiles {
    private val log = LoggerFactory.getLogger(MediaFiles::class.java)

    init {
        System.setProperty("java.awt.headless", "true")
    }

    val IMAGE_EXT = setOf("png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "tiff", "tif")
    val VIDEO_EXT = setOf("mp4", "webm", "mov", "mkv", "avi", "m4v", "flv", "gifv")
    val AUDIO_EXT = setOf("mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma")

    private val MIME = mapOf(
        "png" to "image/png", "jpg" to "image/jpeg", "jpeg" to "image/jpeg",
        "webp" to "image/webp", "gif" to "image/gif", "bmp" to "image/bmp",
        "avif" to "image/avif", "tif" to "image/tiff", "tiff" to "image/tiff",
        "mp4" to "video/mp4", "webm" to "video/webm", "mov" to "video/quicktime",
        "mkv" to "video/x-matroska", "avi" to "video/x-msvideo", "m4v" to "video/x-m4v",
        "mp3" to "audio/mpeg", "wav" to "audio/wav", "flac" to "audio/flac",
        "ogg" to "audio/ogg", "m4a" to "audio/mp4", "aac" to "audio/aac",
        "opus" to "audio/opus", "wma" to "audio/x-ms-wma",
    )

    fun extensionOf(filename: String): String =
        filename.substringAfterLast('.', "").lowercase().take(12)

    fun detectMime(filename: String, provided: String? = null): String {
        if (!provided.isNullOrBlank() && provided != "application/octet-stream") return provided
        return MIME[extensionOf(filename)] ?: "application/octet-stream"
    }

    fun kindOf(filename: String, providedMime: String? = null): String {
        val ext = extensionOf(filename)
        val mime = detectMime(filename, providedMime)
        return when {
            mime.startsWith("image/") || ext in IMAGE_EXT -> "IMAGE"
            mime.startsWith("video/") || ext in VIDEO_EXT -> "VIDEO"
            mime.startsWith("audio/") || ext in AUDIO_EXT -> "AUDIO"
            else -> "IMAGE"
        }
    }

    fun sha256(path: Path): String {
        val md = MessageDigest.getInstance("SHA-256")
        Files.newInputStream(path).use { input ->
            val buf = ByteArray(1 shl 16)
            while (true) {
                val n = input.read(buf)
                if (n <= 0) break
                md.update(buf, 0, n)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }

    // -----------------------------------------------------------------------
    //  图片尺寸（解析文件头，不解码整张图）
    // -----------------------------------------------------------------------

    fun probeImageSize(path: Path): Pair<Int, Int>? = try {
        Files.newInputStream(path).use { raw ->
            val input = BufferedInputStream(raw, 1 shl 16)
            input.mark(64)
            val head = ByteArray(16)
            val n = input.readNBytes(head, 0, 16)
            input.reset()
            if (n < 8) null else when {
                head[0] == 0x89.toByte() && head[1] == 'P'.code.toByte() -> pngSize(input)
                head[0] == 0xFF.toByte() && head[1] == 0xD8.toByte() -> jpegSize(input)
                head[0] == 'G'.code.toByte() && head[1] == 'I'.code.toByte() -> gifSize(input)
                head[0] == 'B'.code.toByte() && head[1] == 'M'.code.toByte() -> bmpSize(input)
                head[0] == 'R'.code.toByte() && head[1] == 'I'.code.toByte() -> webpSize(input)
                else -> null
            }
        }
    } catch (e: Exception) {
        log.debug("尺寸探测失败 {}: {}", path.fileName, e.message)
        null
    }

    private fun readInt32LE(b: ByteArray, off: Int): Int =
        (b[off].toInt() and 0xFF) or ((b[off + 1].toInt() and 0xFF) shl 8) or
            ((b[off + 2].toInt() and 0xFF) shl 16) or ((b[off + 3].toInt() and 0xFF) shl 24)

    private fun readInt16LE(b: ByteArray, off: Int): Int =
        (b[off].toInt() and 0xFF) or ((b[off + 1].toInt() and 0xFF) shl 8)

    private fun be32(b: ByteArray, off: Int): Int =
        ((b[off].toInt() and 0xFF) shl 24) or ((b[off + 1].toInt() and 0xFF) shl 16) or
            ((b[off + 2].toInt() and 0xFF) shl 8) or (b[off + 3].toInt() and 0xFF)

    private fun pngSize(input: InputStream): Pair<Int, Int>? {
        val b = ByteArray(24)
        if (input.readNBytes(b, 0, 24) < 24) return null
        val w = be32(b, 16)
        val h = be32(b, 20)
        return if (w > 0 && h > 0) w to h else null
    }

    private fun gifSize(input: InputStream): Pair<Int, Int>? {
        val b = ByteArray(10)
        if (input.readNBytes(b, 0, 10) < 10) return null
        val w = readInt16LE(b, 6)
        val h = readInt16LE(b, 8)
        return if (w > 0 && h > 0) w to h else null
    }

    private fun bmpSize(input: InputStream): Pair<Int, Int>? {
        val b = ByteArray(26)
        if (input.readNBytes(b, 0, 26) < 26) return null
        val w = readInt32LE(b, 18)
        val h = readInt32LE(b, 22)
        return if (w > 0 && h != 0) w to kotlin.math.abs(h) else null
    }

    private fun webpSize(input: InputStream): Pair<Int, Int>? {
        val b = ByteArray(30)
        if (input.readNBytes(b, 0, 30) < 30) return null
        val fourcc = String(b, 12, 4, Charsets.US_ASCII)
        return when (fourcc) {
            "VP8 " -> {
                val w = readInt16LE(b, 26) and 0x3FFF
                val h = readInt16LE(b, 28) and 0x3FFF
                if (w > 0 && h > 0) w to h else null
            }
            "VP8L" -> {
                val bits = (b[21].toInt() and 0xFF) or ((b[22].toInt() and 0xFF) shl 8) or
                    ((b[23].toInt() and 0xFF) shl 16) or ((b[24].toInt() and 0xFF) shl 24)
                val w = (bits and 0x3FFF) + 1
                val h = ((bits shr 14) and 0x3FFF) + 1
                w to h
            }
            "VP8X" -> {
                val w = 1 + ((b[24].toInt() and 0xFF) or ((b[25].toInt() and 0xFF) shl 8) or ((b[26].toInt() and 0xFF) shl 16))
                val h = 1 + ((b[27].toInt() and 0xFF) or ((b[28].toInt() and 0xFF) shl 8) or ((b[29].toInt() and 0xFF) shl 16))
                w to h
            }
            else -> null
        }
    }

    /**
     * JPEG 尺寸：一路跳段，遇到 SOF（`C0`~`CF`，去掉 `C4`/`C8`/`CC` 这三个）读宽高。
     *
     * ⚠ 2026-09 修的老 bug：旧实现把 **SOI（`FFD8`）也当成"带长度字段的段"** ——
     * 它读到 `FFD8` 后紧接着把下一个段的 `FFE0` 当成"长度 = 0xFFE0"，要跳六万多字节，
     * 于是**任何 JPEG 都返回 null**（jpg 附件的宽高一直是空的，png/webp 却正常）。
     * SOI / EOI / TEM / RSTn 都是**没有长度字段的独立标记**，必须在读长度之前先认出来。
     */
    private fun jpegSize(input: InputStream): Pair<Int, Int>? {
        if (input.read() != 0xFF) return null
        while (true) {
            var marker = input.read()
            if (marker == -1) return null
            // 标记前面允许有任意多个填充 0xFF
            while (marker == 0xFF) {
                marker = input.read()
                if (marker == -1) return null
            }
            // 没有长度字段的独立标记：SOI(FFD8) / EOI(FFD9) / TEM(FF01) / RSTn(FFD0~FFD7)
            if (marker == 0xD8 || marker == 0xD9 || marker == 0x01 || marker in 0xD0..0xD7) continue

            val lenHi = input.read(); val lenLo = input.read()
            if (lenHi == -1 || lenLo == -1) return null
            val len = (lenHi shl 8) or lenLo
            if (len < 2) return null

            val isSof = marker in 0xC0..0xCF && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
            if (isSof) {
                val data = ByteArray(5)
                if (input.readNBytes(data, 0, 5) < 5) return null
                val h = (data[1].toInt() and 0xFF shl 8) or (data[2].toInt() and 0xFF)
                val w = (data[3].toInt() and 0xFF shl 8) or (data[4].toInt() and 0xFF)
                return if (w > 0 && h > 0) w to h else null
            }

            var skipped = 0L
            val need = (len - 2).toLong()
            while (skipped < need) {
                val s = input.skip(need - skipped)
                if (s <= 0) {
                    if (input.read() == -1) return null
                    skipped += 1
                } else skipped += s
            }
        }
    }

    // -----------------------------------------------------------------------
    //  缩略图（ImageIO，仅用于图片）
    // -----------------------------------------------------------------------

    /**
     * 生成 JPEG 缩略图。支持 ImageIO 能读的一切（png / jpg / gif / bmp / tiff，
     * 以及 TwelveMonkeys 插件补上的 **webp**）。
     *
     * 返回 false 表示"这个格式解不了"（例如 AVIF / HEIC）—— 这**不是错误**，
     * 调用方必须走"回退原件"的降级路径，而不是给用户一个破图或干脆什么都不显示。
     */
    fun writeThumbnail(source: Path, dest: Path, maxEdge: Int = 512): Boolean {
        return try {
            val img: BufferedImage = ImageIO.read(source.toFile()) ?: return false
            val w0 = img.width
            val h0 = img.height
            if (w0 <= 0 || h0 <= 0) return false

            val scale = minOf(1.0, maxEdge.toDouble() / max(w0, h0))
            val w = max(1, (w0 * scale).toInt())
            val h = max(1, (h0 * scale).toInt())

            val out = BufferedImage(w, h, BufferedImage.TYPE_INT_RGB)
            val g = out.createGraphics()
            try {
                g.setRenderingHint(RenderingHints.KEY_INTERPOLATION, RenderingHints.VALUE_INTERPOLATION_BILINEAR)
                g.setRenderingHint(RenderingHints.KEY_RENDERING, RenderingHints.VALUE_RENDER_QUALITY)
                g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
                // 用浅灰填充透明区域，避免 PNG 转 JPEG 后出现黑块
                g.color = java.awt.Color(0x1E, 0x1E, 0x22)
                g.fillRect(0, 0, w, h)
                g.drawImage(img, 0, 0, w, h, null)
            } finally {
                g.dispose()
            }

            Files.createDirectories(dest.parent)
            val writer = ImageIO.getImageWritersByFormatName("jpeg").next() ?: return false
            ImageIO.createImageOutputStream(dest.toFile()).use { ios ->
                writer.output = ios
                val param = writer.defaultWriteParam.apply {
                    if (canWriteCompressed()) {
                        compressionMode = ImageWriteParam.MODE_EXPLICIT
                        compressionQuality = 0.82f
                    }
                }
                writer.write(null, IIOImage(out, null, null), param)
                writer.dispose()
            }
            true
        } catch (e: IOException) {
            log.debug("缩略图生成失败 {}: {} {}", source.fileName, e.javaClass.simpleName, e.message)
            false
        } catch (e: Exception) {
            // 带上异常类名：有些 IIOException / NoSuchElementException 的 message 是 null，
            // 只打 message 会得到"缩略图生成失败 x.webp: null"这种没法查的日志。
            log.debug("缩略图生成失败 {}: {} {}", source.fileName, e.javaClass.simpleName, e.message)
            false
        }
    }

    // -----------------------------------------------------------------------
    //  视频封面（预览图）
    // -----------------------------------------------------------------------

    /**
     * 抽视频封面，走 `scripts\video-poster.ps1`（Windows 资源管理器的缩略图管线）。
     *
     * 为什么不自己解码：JVM 侧没有随包的解码器，而引入 ffmpeg 意味着要分发一个
     * 几十 MB 的二进制。系统缩略图对"本机能播的视频"都能出图，零额外依赖。
     *
     * 约定：成功返回 true 且 [dest] 存在；任何失败都返回 false 并**删掉半成品**，
     * 让调用方走"没有封面"的降级路径（播放器转圈），不影响播放。脚本用
     * `OK` / `FAIL` 前缀报告结果，这里只看文件是否真的生成了。
     */
    fun writeVideoPoster(source: Path, dest: Path, size: Int = 640, roots: List<Path> = emptyList()): Boolean {
        val script = locatePosterScript(roots) ?: run {
            log.debug("找不到 video-poster.ps1，跳过封面抽取")
            return false
        }
        return try {
            Files.createDirectories(dest.parent)
            Files.deleteIfExists(dest)
            val pwsh = locatePwsh() ?: "pwsh"
            val process = ProcessBuilder(
                pwsh, "-NoProfile", "-NonInteractive", "-File", script.toString(),
                "-Source", source.toAbsolutePath().toString(),
                "-Dest", dest.toAbsolutePath().toString(),
                "-Size", size.toString(),
                "-Force",
            )
                .redirectErrorStream(true)
                .start()

            val output = process.inputStream.bufferedReader().use { it.readText() }
            val finished = process.waitFor(60, java.util.concurrent.TimeUnit.SECONDS)
            if (!finished) {
                process.destroyForcibly()
                log.debug("封面抽取超时: {}", source.fileName)
                return false
            }
            val ok = Files.isRegularFile(dest) && Files.size(dest) > 0
            if (!ok) {
                Files.deleteIfExists(dest)
                log.debug("封面抽取失败 {}: {}", source.fileName, output.trim().take(200))
            }
            ok
        } catch (e: Exception) {
            runCatching { Files.deleteIfExists(dest) }
            log.debug("封面抽取异常 {}: {}", source.fileName, e.message)
            false
        }
    }

    /** 两种布局都认：源码树 `<根>\scripts`，发布包 `<根>\scripts`（根由 storage 往上找）。 */
    private fun locatePosterScript(roots: List<Path>): Path? {
        val seeds = mutableListOf<Path>()
        seeds.addAll(roots)
        val cwd = Paths.get("").toAbsolutePath().normalize()
        seeds.add(cwd)
        cwd.parent?.let { seeds.add(it) }
        // 再补上 cwd 往上的两级（运行目录可能是 server/ 或发布包根）
        var up = cwd.parent
        repeat(2) {
            up?.parent?.let { seeds.add(it) }
            up = up?.parent
        }
        for (root in seeds) {
            var cursor: Path? = root
            // 往上最多找三层：viewer/ → server/ → 项目根
            repeat(3) {
                val candidate = cursor?.resolve("scripts")?.resolve("video-poster.ps1")
                if (candidate != null && Files.isRegularFile(candidate)) return candidate
                cursor = cursor?.parent
            }
        }
        return null
    }

    /** 优先用 PATH 里的 pwsh；找不到就交给 ProcessBuilder 报错并降级。 */
    private fun locatePwsh(): String? {
        val explicit = System.getenv("COMFYHUB_PWSH")
        if (!explicit.isNullOrBlank() && Files.isRegularFile(Paths.get(explicit))) return explicit
        val home = System.getProperty("user.home") ?: return null
        val candidates = listOf(
            Paths.get("C:", "Program Files", "PowerShell", "7", "pwsh.exe"),
            Paths.get(home, "scoop", "shims", "pwsh.exe"),
            Paths.get("C:", "Program Files", "PowerShell", "7-preview", "pwsh.exe"),
        )
        return candidates.firstOrNull { Files.isRegularFile(it) }?.toString()
    }
}
