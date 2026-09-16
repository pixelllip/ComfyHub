package com.comfyhub

import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.util.UUID

/**
 * 生成产物的磁盘存储。目录结构：
 *
 *   storage/
 *     media/   <- 原始文件  <uuid>.<ext>
 *     thumbs/  <- 图片缩略图 <mediaId>.jpg
 *     tmp/     <- 上传中转
 */
class Storage(private val root: Path) {
    private val log = LoggerFactory.getLogger(Storage::class.java)

    val mediaDir: Path = root.resolve("media")
    val thumbDir: Path = root.resolve("thumbs")
    val tmpDir: Path = root.resolve("tmp")

    init {
        Files.createDirectories(mediaDir)
        Files.createDirectories(thumbDir)
        Files.createDirectories(tmpDir)
        log.info("存储目录: {}", root)
    }

    fun newStoredName(originalName: String): String {
        val ext = MediaFiles.extensionOf(originalName)
        val id = UUID.randomUUID().toString().replace("-", "")
        return if (ext.isEmpty()) id else "$id.$ext"
    }

    fun resolveMedia(storedName: String): Path? {
        if (storedName.isBlank()) return null
        val p = mediaDir.resolve(storedName).normalize()
        // 防目录穿越
        if (!p.startsWith(mediaDir)) return null
        return p.takeIf { Files.isRegularFile(it) }
    }

    fun thumbPath(mediaId: Long): Path = thumbDir.resolve("$mediaId.jpg")

    /** 视频封面缓存：thumbs/<mediaId>.poster.png（与图片缩略图分开存，互不覆盖）。 */
    fun posterPath(mediaId: Long): Path = thumbDir.resolve("$mediaId.poster.png")

    fun tempFile(suffix: String = ".part"): Path = Files.createTempFile(tmpDir, "upload-", suffix)

    fun deleteMediaFile(storedName: String) {
        runCatching { resolveMedia(storedName)?.let { Files.deleteIfExists(it) } }
    }

    fun deleteThumb(mediaId: Long) {
        runCatching { Files.deleteIfExists(thumbPath(mediaId)) }
        runCatching { Files.deleteIfExists(posterPath(mediaId)) }
    }

    fun sizeOf(storedName: String): Long =
        resolveMedia(storedName)?.let { runCatching { Files.size(it) }.getOrDefault(0L) } ?: 0L
}
