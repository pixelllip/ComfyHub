package com.comfyhub.ai

import com.comfyhub.Db
import com.comfyhub.FailedInfo
import com.comfyhub.MediaFiles
import com.comfyhub.Storage
import com.comfyhub.execute
import com.comfyhub.isoTime
import com.comfyhub.queryList
import com.comfyhub.queryOne
import com.comfyhub.ai.protocol.AttachmentKindRef
import com.comfyhub.ai.protocol.ChatAttachment
import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.sql.ResultSet
import java.util.Base64
import java.util.UUID

/**
 * AI 附件（M3 / AIH-027 ~ AIH-031）。
 *
 * 三层职责分得很清楚，改的时候别混：
 *
 *  - [AiAttachmentDto]：对外的客观事实（名字 / 类型 / 大小 / 尺寸）。**不含存储路径**；
 *  - [AiAttachmentRepo]：只有数据库那一层（增删查）；
 *  - [AiAttachmentStore]：文件那一层（落盘、缩略图 / 视频预览帧、读出来做 base64 内联）。
 *
 * 关键规矩：
 *  - 类型判定**只认签名**（`FileKindDetector`）：认不出来直接拒收，不做"未知即图片"的乐观回退；
 *  - 原件与缩略图放在 `storage/ai-attachments` 与 `storage/ai-thumbs`，与画廊产物**分开**存；
 *  - 图片缩略图 = JPEG 缩略图（PNG / JPEG / GIF / BMP / TIFF / **WebP** 都能解码，
 *    WebP 由 `imageio-webp` 这个纯 Java 插件提供；解不了的格式回退发原件），
 *    视频预览帧 = Windows 缩略图管线抽的第一帧（复用画廊那套，
 *    不引入 ffmpeg）；抽不出来就返回 null，界面退化成文件图标（不是破图）。
 */
@Serializable
data class AiAttachmentDto(
    val id: String,
    /** 原始文件名（界面显示用） */
    val name: String,
    /** image / video / audio / document / text */
    val kind: String,
    /** 输入模态，与 [kind] 同义；未知类型不允许入库，所以这里不会是 null */
    val modality: String,
    val mimeType: String,
    val sizeBytes: Long = 0,
    val width: Int? = null,
    val height: Int? = null,
    val sha256: String? = null,
    /** ready / rejected / deleted */
    val status: String = "ready",
    val createdAt: String? = null,
) {
    /**
     * 界面上能不能显示成"图"（图片缩略图 / 视频预览帧）。
     * 图片与视频都会走 `GET /api/ai/attachments/{id}/thumb`，抽帧失败时后端回 204，
     * 前端据此退化成文件图标。
     */
    val hasPreview: Boolean get() = kind == "image" || kind == "video"

    fun toFact(): AttachmentFact = AttachmentFact(
        name = name,
        modality = Modality.parse(modality),
        mimeType = mimeType,
        sizeBytes = sizeBytes,
        pixels = if (width != null && height != null) width.toLong() * height.toLong() else null,
    )
}

/** 一次上传的结果：`items` 是入库成功的，`failed` 是逐个失败的原因（不静默丢弃）。 */
@Serializable
data class AiAttachmentUploadResult(
    val items: List<AiAttachmentDto> = emptyList(),
    val failed: List<FailedInfo> = emptyList(),
)

// ---------------------------------------------------------------------------
//  数据库
// ---------------------------------------------------------------------------

object AiAttachmentRepo {
    fun insert(dto: AiAttachmentDto, storedName: String): AiAttachmentDto {
        Db.withConnection { conn ->
            conn.execute(
                """
                INSERT INTO ai_attachments
                  (id, original_name, stored_name, kind, mime_type, size_bytes, width, height, sha256, status)
                VALUES (?,?,?,?,?,?,?,?,?,?)
                """.trimIndent(),
                dto.id, dto.name, storedName, dto.kind, dto.mimeType, dto.sizeBytes,
                dto.width, dto.height, dto.sha256, dto.status,
            )
        }
        return dto
    }

    fun get(id: String): AiAttachmentDto? = Db.withConnection { conn ->
        conn.queryOne("SELECT * FROM ai_attachments WHERE id = ?", id) { it.toDto() }
    }

    /**
     * 按 id 批量取，**保持传入顺序**（预检结果要跟界面的顺序一一对应）。
     * 找不到的 id 静默跳过 —— 调用方可以用"数量对不对"判断有没有已删除的附件。
     */
    fun list(ids: List<String>): List<AiAttachmentDto> {
        if (ids.isEmpty()) return emptyList()
        val found = Db.withConnection { conn ->
            conn.queryList(
                "SELECT * FROM ai_attachments WHERE id IN (${ids.joinToString(",") { "?" }})",
                *ids.toTypedArray(),
            ) { it.toDto() }
        }.associateBy { it.id }
        return ids.mapNotNull { found[it] }
    }

    /** 删除行的同时连原件与缩略图一起删（调用方负责 [Storage.deleteAiAttachmentFiles]）。 */
    fun storedName(id: String): String? = Db.withConnection { conn ->
        conn.queryOne("SELECT stored_name FROM ai_attachments WHERE id = ?", id) { it.getString(1) }
    }

    fun delete(id: String): Boolean = Db.withConnection { conn ->
        conn.execute("DELETE FROM ai_attachments WHERE id = ?", id) > 0
    }

    private fun ResultSet.toDto() = AiAttachmentDto(
        id = getString("id"),
        name = getString("original_name") ?: "附件",
        kind = getString("kind") ?: "document",
        modality = getString("kind") ?: "document",
        mimeType = getString("mime_type") ?: "application/octet-stream",
        sizeBytes = getLong("size_bytes"),
        // width / height 在库里是可空列：getInt 会回 0，这里用 getObject 区分"没有"与"0"
        width = (getObject("width") as? Number)?.toInt(),
        height = (getObject("height") as? Number)?.toInt(),
        sha256 = getString("sha256"),
        status = getString("status") ?: "ready",
        createdAt = isoTime("created_at"),
    )
}

// ---------------------------------------------------------------------------
//  文件
// ---------------------------------------------------------------------------

class AiAttachmentStore(
    private val storage: Storage,
    /** 项目根：视频预览帧的脚本 `<根>\scripts\video-poster.ps1` 从这里往上找 */
    private val projectRoot: Path,
) {
    private val log = LoggerFactory.getLogger(AiAttachmentStore::class.java)

    companion object {
        /** 单个附件的上传硬上限（再大就不该走"内联进请求体"这条路）。 */
        const val MAX_UPLOAD_BYTES: Long = 32L * 1024 * 1024

        /**
         * 内联 base64 的**原始字节**上限。
         *
         * base64 会把体积放大 1/3，而各网关的请求体上限普遍在 20MB 量级（Anthropic 单图 5MB、
         * OpenAI 单图 20MB）。8MB 原图 → ~10.7MB 请求体，是"能发出去"和"别把网关打挂"之间的平衡点。
         */
        const val MAX_INLINE_BYTES: Long = 8L * 1024 * 1024
    }

    /**
     * 把一个已落到临时文件的上传收进附件库。
     *
     * 失败一律抛 [AiException]（带稳定 code）并**删掉临时文件**，绝不留半份：
     *  - 空文件 / 超大 → `UNSUPPORTED_CONTENT`；
     *  - 签名认不出来 → `UNSUPPORTED_CONTENT`（前端声明的 MIME 只是线索，AIH-027）。
     */
    fun save(name: String, tmp: Path, declaredMime: String?): AiAttachmentDto {
        val fileName = name.trim().ifEmpty { "attachment.bin" }.take(255)
        try {
            val size = Files.size(tmp)
            if (size <= 0L) {
                throw AiException(AiErrorCode.UNSUPPORTED_CONTENT, "文件 $fileName 是空的")
            }
            if (size > MAX_UPLOAD_BYTES) {
                throw AiException(
                    AiErrorCode.UNSUPPORTED_CONTENT,
                    "文件 $fileName 超过单附件上限（${MAX_UPLOAD_BYTES / 1024 / 1024} MB）",
                )
            }
            val head = Files.newInputStream(tmp).use { it.readNBytes(StrictIntake.MAX_HEAD_BYTES) }
            val detected = FileKindDetector.detect(head, fileName, declaredMime)
            if (detected.blocked) {
                throw AiException(
                    AiErrorCode.UNSUPPORTED_CONTENT,
                    "无法识别文件 $fileName 的真实类型（签名与声明的 ${declaredMime ?: "未知 MIME"} 不符），已拒绝上传",
                )
            }
            val id = UUID.randomUUID().toString()
            val stored = storage.newAiStoredName(fileName)
            val dest = storage.aiAttachmentDir.resolve(stored)
            Files.move(tmp, dest, StandardCopyOption.REPLACE_EXISTING)

            val dims = if (detected.kind == FileKind.IMAGE) MediaFiles.probeImageSize(dest) else null
            val sha = runCatching { MediaFiles.sha256(dest) }.getOrNull()
            val dto = AiAttachmentDto(
                id = id,
                name = fileName,
                kind = detected.kind.name.lowercase(),
                modality = (detected.modality ?: Modality.DOCUMENT).wire,
                mimeType = detected.mimeType ?: (declaredMime ?: "application/octet-stream"),
                sizeBytes = size,
                width = dims?.first,
                height = dims?.second,
                sha256 = sha,
            )
            AiAttachmentRepo.insert(dto, stored)
            log.info("AI 附件已入库 id={} kind={} size={}", id, dto.kind, size)
            return dto
        } catch (t: Throwable) {
            runCatching { Files.deleteIfExists(tmp) }
            throw t
        }
    }

    /** 上传中转文件（放在 storage/tmp，与画廊上传共用同一套约定）。 */
    fun tempFile(): Path = storage.tempFile()

    /** 原件路径（找不到文件返回 null：库里有行、盘上没文件时要如实降级）。 */
    fun fileOf(id: String): Path? {
        val stored = AiAttachmentRepo.storedName(id) ?: return null
        return storage.resolveAiAttachment(stored)
    }

    /**
     * 缩略图 / 视频预览帧。返回 `(路径, Content-Type)`；没有可看的图就返回 null
     * （调用方回 204，界面退化成文件图标）。
     *
     * 生成是**惰性**的：上传时只落原件，第一次请求缩略图时才解码 —— 上传路径不该为一张
     * 可能永远没人看的缩略图停下来。
     *
     * 图片分支有**两级降级**（与画廊 `/thumb` 同一条规矩）：
     *  1. 先尝试 JPEG 缩略图（png / jpg / gif / bmp / tiff / **webp**，webp 靠
     *     `imageio-webp` 这个纯 Java 插件，见 `build.gradle.kts`）；
     *  2. 解不了（AVIF / HEIC / 动图 webp 这类没有解码器的）就**把原件当预览发出去**
     *     （带它自己的 MIME）—— Flutter 侧本来就认得 WebP/AVIF，
     *     断在这里的话用户只会看到一个文件图标，而"图明明在那儿"。
     */
    fun thumbnailOf(id: String): Pair<Path, String>? {
        val dto = AiAttachmentRepo.get(id) ?: return null
        val src = fileOf(id) ?: return null
        return when (dto.kind) {
            "image" -> {
                val dest = storage.aiThumbPath(id)
                if (Files.isRegularFile(dest) || MediaFiles.writeThumbnail(src, dest)) {
                    dest to "image/jpeg"
                } else {
                    src to dto.mimeType
                }
            }
            "video" -> {
                val dest = storage.aiPosterPath(id)
                if (!Files.isRegularFile(dest) &&
                    !MediaFiles.writeVideoPoster(src, dest, roots = listOf(projectRoot))
                ) {
                    return null
                }
                if (Files.isRegularFile(dest)) dest to "image/png" else null
            }
            // 音频 / 文档 / 文本没有"画面"可看：界面显示文件图标就够了
            else -> null
        }
    }

    fun delete(id: String): Boolean {
        val stored = AiAttachmentRepo.storedName(id)
        val removed = AiAttachmentRepo.delete(id)
        if (stored != null) storage.deleteAiAttachmentFiles(stored, id)
        return removed
    }

    /**
     * 读成"可以直接内联给模型"的附件：图片才需要 base64，其他模态目前适配器都不支持
     * （预检会在更早一步阻断，这里是编程不变式）。
     *
     * 读不到文件或超过内联上限时返回 null —— 调用方负责把它变成一条**明确说明**
     * （"这个附件没随本次请求发送"），而不是静默丢掉。
     */
    fun toChatAttachment(dto: AiAttachmentDto): ChatAttachment? {
        val kind = when (dto.modality) {
            "image" -> AttachmentKindRef.IMAGE
            "video" -> AttachmentKindRef.VIDEO
            "audio" -> AttachmentKindRef.AUDIO
            "document" -> AttachmentKindRef.DOCUMENT
            else -> AttachmentKindRef.TEXT
        }
        val path = fileOf(dto.id) ?: return null
        val size = runCatching { Files.size(path) }.getOrDefault(0L)
        if (size <= 0L || size > MAX_INLINE_BYTES) return null
        val base64 = runCatching {
            Base64.getEncoder().encodeToString(Files.readAllBytes(path))
        }.getOrNull() ?: return null
        return ChatAttachment(
            kind = kind,
            mimeType = dto.mimeType,
            base64 = base64,
            fileName = dto.name,
        )
    }
}
