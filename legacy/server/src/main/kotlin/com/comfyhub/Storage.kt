package com.comfyhub

import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.util.UUID

/**
 * 生成产物的磁盘存储。目录结构：
 *
 *   storage/
 *     media/          <- 画廊产物原件  <uuid>.<ext>
 *     thumbs/         <- 图片缩略图 / 视频封面 <mediaId>.jpg | <mediaId>.poster.png
 *     ai-attachments/ <- 发给 AI 的附件原件（M3）  <uuid>.<ext>
 *     ai-thumbs/      <- AI 附件缩略图 / 视频预览帧  <attachmentId>.jpg | .poster.png
 *     tmp/            <- 上传中转
 *
 * AI 附件**刻意与画廊产物分开**：两者生命周期不同（附件跟着会话走，产物是用户资产），
 * 混在一起以后做"清孤儿"时会互相误删。
 */
class Storage(private val root: Path) {
    private val log = LoggerFactory.getLogger(Storage::class.java)

    val mediaDir: Path = root.resolve("media")
    val thumbDir: Path = root.resolve("thumbs")
    val aiAttachmentDir: Path = root.resolve("ai-attachments")
    val aiThumbDir: Path = root.resolve("ai-thumbs")
    val tmpDir: Path = root.resolve("tmp")

    init {
        Files.createDirectories(mediaDir)
        Files.createDirectories(thumbDir)
        Files.createDirectories(aiAttachmentDir)
        Files.createDirectories(aiThumbDir)
        Files.createDirectories(tmpDir)
        log.info("存储目录: {}", root)
    }

    fun newStoredName(originalName: String): String {
        val ext = MediaFiles.extensionOf(originalName)
        val id = UUID.randomUUID().toString().replace("-", "")
        return if (ext.isEmpty()) id else "$id.$ext"
    }

    /** AI 附件原件的落盘名（与画廊产物同一套命名规则，但落在另一个目录）。 */
    fun newAiStoredName(originalName: String): String = newStoredName(originalName)

    fun resolveMedia(storedName: String): Path? {
        if (storedName.isBlank()) return null
        val p = mediaDir.resolve(storedName).normalize()
        // 防目录穿越
        if (!p.startsWith(mediaDir)) return null
        return p.takeIf { Files.isRegularFile(it) }
    }

    fun resolveAiAttachment(storedName: String): Path? {
        if (storedName.isBlank()) return null
        val p = aiAttachmentDir.resolve(storedName).normalize()
        if (!p.startsWith(aiAttachmentDir)) return null
        return p.takeIf { Files.isRegularFile(it) }
    }

    fun thumbPath(mediaId: Long): Path = thumbDir.resolve("$mediaId.jpg")

    /** 视频封面缓存：thumbs/<mediaId>.poster.png（与图片缩略图分开存，互不覆盖）。 */
    fun posterPath(mediaId: Long): Path = thumbDir.resolve("$mediaId.poster.png")

    /** AI 附件的图片缩略图：ai-thumbs/<attachmentId>.jpg */
    fun aiThumbPath(attachmentId: String): Path = aiThumbDir.resolve("$attachmentId.jpg")

    /** AI 附件的视频预览帧：ai-thumbs/<attachmentId>.poster.png */
    fun aiPosterPath(attachmentId: String): Path = aiThumbDir.resolve("$attachmentId.poster.png")

    fun tempFile(suffix: String = ".part"): Path = Files.createTempFile(tmpDir, "upload-", suffix)

    fun deleteMediaFile(storedName: String) {
        runCatching { resolveMedia(storedName)?.let { Files.deleteIfExists(it) } }
    }

    fun deleteThumb(mediaId: Long) {
        runCatching { Files.deleteIfExists(thumbPath(mediaId)) }
        runCatching { Files.deleteIfExists(posterPath(mediaId)) }
    }

    /** 删掉一个 AI 附件的原件与缩略图；任何失败都只记日志（删除不该把请求打挂）。 */
    fun deleteAiAttachmentFiles(storedName: String, attachmentId: String) {
        runCatching { resolveAiAttachment(storedName)?.let { Files.deleteIfExists(it) } }
            .onFailure { log.debug("删除 AI 附件原件失败 {}: {}", storedName, it.message) }
        runCatching { Files.deleteIfExists(aiThumbPath(attachmentId)) }
        runCatching { Files.deleteIfExists(aiPosterPath(attachmentId)) }
    }

    fun sizeOf(storedName: String): Long =
        resolveMedia(storedName)?.let { runCatching { Files.size(it) }.getOrDefault(0L) } ?: 0L
}

