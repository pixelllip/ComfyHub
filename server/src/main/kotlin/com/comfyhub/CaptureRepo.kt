package com.comfyhub

import kotlinx.serialization.json.JsonObject
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

/**
 * 捕获相关的落库逻辑：产物文件入库（SHA-256 去重）+ 运行记录。
 *
 * 手动上传、ComfyUI 推送捕获、历史目录导入都走这里，
 * 保证「同一份文件不会入两次库」「同一次生成不会建两条提示词」。
 */
object CaptureRepo {
    private val log = LoggerFactory.getLogger(CaptureRepo::class.java)

    /** 单次导入的结果 */
    sealed interface Imported {
        data class Created(val mediaId: Long, val kind: String) : Imported
        data class Duplicate(val existingId: Long) : Imported
        data class Failed(val reason: String) : Imported
    }

    // -----------------------------------------------------------------------
    //  文件入库
    // -----------------------------------------------------------------------

    /**
     * 把一个已经存在于磁盘上的文件收进 storage。
     *
     * @param move true 表示源文件是临时文件，入库后不再需要（会被移动/删除）；
     *             false 表示源文件属于用户（例如 ComfyUI 的 output 目录），只复制。
     */
    fun importFile(
        storage: Storage,
        source: Path,
        originalName: String,
        kindHint: String? = null,
        promptId: Long? = null,
        title: String? = null,
        sourceLabel: String = "ComfyUI",
        sourceRef: String? = null,
        workflowJson: String? = null,
        notes: String? = null,
        tags: List<String> = emptyList(),
        move: Boolean = false,
    ): Imported {
        if (!Files.isRegularFile(source)) return Imported.Failed("文件不存在: $source")

        val kind = kindHint?.uppercase()?.takeIf { it in MediaRepo.KINDS }
            ?: MediaFiles.kindOf(originalName)
        val sha = try {
            MediaFiles.sha256(source)
        } catch (e: Exception) {
            return Imported.Failed("读取文件失败: ${e.message}")
        }

        // 内容去重：同一张图重复导入时直接复用已有记录
        val existing = Db.withConnection { conn -> MediaRepo.findBySha(conn, sha) }
        if (existing != null) {
            if (move) runCatching { Files.deleteIfExists(source) }
            return Imported.Duplicate(existing.id)
        }

        val storedName = storage.newStoredName(originalName)
        val dest = storage.mediaDir.resolve(storedName)
        try {
            if (move) {
                try {
                    Files.move(source, dest, StandardCopyOption.REPLACE_EXISTING)
                } catch (_: Exception) {
                    // 跨盘 move 可能失败，退回复制
                    Files.copy(source, dest, StandardCopyOption.REPLACE_EXISTING)
                    runCatching { Files.deleteIfExists(source) }
                }
            } else {
                Files.copy(source, dest, StandardCopyOption.REPLACE_EXISTING)
            }
        } catch (e: Exception) {
            return Imported.Failed("写入存储目录失败: ${e.message}")
        }

        return try {
            val size = Files.size(dest)
            val mime = MediaFiles.detectMime(originalName)
            val dims = if (kind == "IMAGE") MediaFiles.probeImageSize(dest) else null

            val id = Db.tx { conn ->
                val newId = MediaRepo.insert(
                    conn = conn,
                    kind = kind,
                    title = title?.take(255) ?: originalName,
                    originalName = originalName,
                    storedName = storedName,
                    mimeType = mime,
                    sizeBytes = size,
                    width = dims?.first,
                    height = dims?.second,
                    durationMs = null,
                    sha256 = sha,
                    source = sourceLabel,
                    promptId = promptId,
                    notes = notes,
                    sourceRef = sourceRef,
                    workflowJson = workflowJson,
                )
                if (tags.isNotEmpty()) TagRepo.setMediaTags(conn, newId, tags)
                TagRepo.refreshUseCounts(conn)
                newId
            }
            if (kind == "IMAGE") MediaFiles.writeThumbnail(dest, storage.thumbPath(id))
            Imported.Created(id, kind)
        } catch (e: Exception) {
            log.error("产物入库失败 {}", originalName, e)
            runCatching { Files.deleteIfExists(dest) }
            Imported.Failed(e.message ?: "入库失败")
        }
    }

    // -----------------------------------------------------------------------
    //  运行记录
    // -----------------------------------------------------------------------

    data class RunClaim(
        val claimed: Boolean,
        val existingPromptId: Long? = null,
        val existingStatus: String? = null,
    )

    /**
     * 抢占一次运行。run_key 唯一 —— 抢不到说明这次生成已经处理过（或正在处理）。
     *
     * @param force 用于「目录导入」这条路：那边在调用之前已经用 SHA-256 确认过文件不在库里，
     *              所以历史运行记录只是个日志，不该拦住重新导入（否则用户删掉产物之后
     *              再点一次「导入已有产物」会永远提示重复）。**正在处理中的**运行不会被抢。
     *
     * 卡在 `running` 超过 10 分钟的记录会被重新抢占，避免后端中途被杀之后再也补不上。
     */
    /**
     * 已经存在一条记录时，这次调用能不能抢占它。
     *
     * 抽成纯函数是为了能单测（`CaptureRunRetryTest`）—— 这里的每一条都是实测踩出来的：
     *  - `running` 且没过 10 分钟：别人正在收，让开；
     *  - **`empty` / `error` 可以重试**：`/history` 先有记录、产物文件晚到是常态，
     *    当终态就会出现"AI 说生成了、画廊里却没有"这种最难查的问题；
     *  - `success` 是唯一不再重复收的终态（幂等靠它）；
     *  - `force` 给「目录导入」用：那边已经用 SHA-256 确认文件不在库里。
     */
    internal fun canReclaim(status: String, ageSeconds: Int, force: Boolean): Boolean = when {
        status == "running" && ageSeconds <= 600 -> false
        status == "empty" || status == "error" || force -> true
        status == "running" -> true
        else -> false
    }

    fun beginRun(runKey: String, source: String, raw: JsonObject? = null, force: Boolean = false): RunClaim =
        Db.withConnection { conn ->
            val rawText = raw?.let { runCatching { AppJson.encodeToString(JsonObject.serializer(), it) }.getOrNull() }

            val inserted = conn.execute(
                "INSERT IGNORE INTO capture_runs (run_key, source, status, raw) VALUES (?,?,?,?)",
                runKey, source, "running", rawText
            )
            if (inserted > 0) return@withConnection RunClaim(true)

            val existing = conn.queryOne(
                "SELECT prompt_id, status, TIMESTAMPDIFF(SECOND, created_at, NOW(3)) AS age FROM capture_runs WHERE run_key = ?",
                runKey
            ) { Triple(it.longOrNull("prompt_id"), it.strOr("status"), it.intOrNull("age") ?: 0) }

            val (promptId, status, age) = existing ?: return@withConnection RunClaim(true)

            if (!canReclaim(status, age, force)) return@withConnection RunClaim(false, promptId, status)

            conn.execute(
                "UPDATE capture_runs SET status = 'running', error = NULL, created_at = CURRENT_TIMESTAMP(3) WHERE run_key = ?",
                runKey
            )
            RunClaim(true)
        }

    fun finishRun(
        runKey: String,
        promptId: Long?,
        status: String,
        mediaCount: Int,
        title: String?,
        error: String?,
    ) = Db.withConnection { conn ->
        conn.execute(
            """
            UPDATE capture_runs
               SET prompt_id = ?, status = ?, media_count = ?, title = ?, error = ?
             WHERE run_key = ?
            """.trimIndent(),
            promptId, status, mediaCount, title?.take(255),
            error?.take(2000), runKey
        )
        Unit
    }

    fun recentRuns(limit: Int = 20): List<CaptureRunInfo> = Db.withConnection { conn ->
        conn.queryList(
            """
            SELECT run_key, prompt_id, status, media_count, title, error, created_at
              FROM capture_runs ORDER BY id DESC LIMIT ?
            """.trimIndent(),
            limit.coerceIn(1, 200)
        ) { rs ->
            CaptureRunInfo(
                runKey = rs.strOr("run_key"),
                promptId = rs.longOrNull("prompt_id"),
                status = rs.strOr("status", "success"),
                mediaCount = rs.intOrNull("media_count") ?: 0,
                title = rs.str("title"),
                error = rs.str("error"),
                capturedAt = rs.isoTime("created_at"),
            )
        }
    }

    /** 按 runKey 查一条记录（AIH-034：`comfy_get_run` 工具用）。 */
    fun findRun(runKey: String): CaptureRunInfo? = Db.withConnection { conn ->        conn.queryOne(
            """
            SELECT run_key, prompt_id, status, media_count, title, error, created_at
              FROM capture_runs WHERE run_key = ?
            """.trimIndent(),
            runKey,
        ) { rs -> rs.toRunInfo() }
    }

    /**
     * 按**捕获记录里的数字 prompt_id** 查一条记录。
     *
     * 为什么需要（实测踩到的坑）：`comfy_submit` 的结果里同时有
     * `promptId`（库里 prompts 表的 id）、`comfyPromptId`（ComfyUI 的 UUID）、
     * `capturedPromptId`（`capture_runs.prompt_id`）三个"id"，模型很自然地会拿数字那个
     * 去 `comfy_get_run`，而那边只认 runKey（UUID）→ 直接 `NOT_FOUND`，
     * 表现就是"刚提交完，它却说查不到这次运行"（真实对话里发生过）。
     * 所以查询入口要把数字也认下来。
     */
    fun findRunByPromptId(promptId: Long): CaptureRunInfo? = Db.withConnection { conn ->        conn.queryOne(
            """
            SELECT run_key, prompt_id, status, media_count, title, error, created_at
              FROM capture_runs WHERE prompt_id = ? ORDER BY id DESC LIMIT 1
            """.trimIndent(),
            promptId,
        ) { rs -> rs.toRunInfo() }
    }

    private fun java.sql.ResultSet.toRunInfo() = CaptureRunInfo(
        runKey = strOr("run_key"),
        promptId = longOrNull("prompt_id"),
        status = strOr("status", "success"),
        mediaCount = intOrNull("media_count") ?: 0,
        title = str("title"),
        error = str("error"),
        capturedAt = isoTime("created_at"),
    )

    /** 原始 `/history` 片段（`raw` 列）—— 提交任务时要从中取回 API 格式节点图。 */
    fun rawOf(runKey: String): String? = Db.withConnection { conn ->
        conn.queryOne("SELECT raw FROM capture_runs WHERE run_key = ?", runKey) { it.getString(1) }
    }

    /** 返回 (运行数, 产物数) */
    fun stats(): Pair<Long, Long> = Db.withConnection { conn ->
        val runs = conn.queryOne("SELECT COUNT(*) FROM capture_runs") { it.getLong(1) } ?: 0L
        val media = conn.queryOne("SELECT COALESCE(SUM(media_count), 0) FROM capture_runs") { it.getLong(1) } ?: 0L
        runs to media
    }
}
