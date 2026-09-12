package com.comfyhub

import java.sql.Connection
import java.sql.ResultSet

object MediaRepo {

    val KINDS = setOf("IMAGE", "VIDEO", "AUDIO")

    private const val COLS = """
        m.id, m.prompt_id, m.kind, m.title, m.original_name, m.stored_name, m.mime_type,
        m.size_bytes, m.width, m.height, m.duration_ms, m.sha256, m.source, m.favorite, m.notes,
        m.created_at, m.updated_at, (m.workflow_json IS NOT NULL) AS has_workflow,
        p.title AS prompt_title, p.positive_prompt AS prompt_positive,
        p.negative_prompt AS prompt_negative, p.checkpoint, p.seed
    """

    private const val FROM = "FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id"

    fun mapMedia(rs: ResultSet, ownTags: List<String> = emptyList()): MediaDto {
        val id = rs.getLong("id")
        return MediaDto(
            id = id,
            promptId = rs.longOrNull("prompt_id"),
            kind = rs.strOr("kind", "IMAGE"),
            title = rs.strOr("title"),
            originalName = rs.strOr("original_name"),
            storedName = rs.strOr("stored_name"),
            mimeType = rs.str("mime_type"),
            sizeBytes = rs.longOrNull("size_bytes") ?: 0L,
            width = rs.intOrNull("width"),
            height = rs.intOrNull("height"),
            durationMs = rs.longOrNull("duration_ms"),
            sha256 = rs.str("sha256"),
            source = rs.str("source"),
            favorite = rs.boolOr("favorite"),
            notes = rs.str("notes"),
            createdAt = rs.isoTime("created_at"),
            updatedAt = rs.isoTime("updated_at"),
            hasWorkflow = rs.boolOr("has_workflow"),
            promptTitle = rs.str("prompt_title"),
            promptPositive = rs.str("prompt_positive"),
            promptNegative = rs.str("prompt_negative"),
            checkpoint = rs.str("checkpoint"),
            seed = rs.longOrNull("seed"),
            promptTags = ownTags,
            fileUrl = "/api/media/$id/file",
            thumbUrl = "/api/media/$id/thumb",
        )
    }

    /**
     * 给产物补上标签：产物自身的标签 + 所关联提示词的标签（去重合并）。
     * 这样在画廊里点开/筛选时，看到的是一致的标签集合。
     */
    private fun attachOwnTags(conn: Connection, rows: List<MediaDto>): List<MediaDto> {
        if (rows.isEmpty()) return rows
        val ownTags = TagRepo.tagsForMedia(conn, rows.map { it.id })
        val promptIds = rows.mapNotNull { it.promptId }.distinct()
        val promptTags = TagRepo.tagsForPrompts(conn, promptIds)
        return rows.map { row ->
            val fromPrompt = row.promptId?.let { promptTags[it] }?.map { it.name }.orEmpty()
            val merged = (ownTags[row.id].orEmpty() + fromPrompt).distinct().sorted()
            row.copy(promptTags = merged)
        }
    }

    data class SearchArgs(
        val q: String? = null,
        val tags: List<String> = emptyList(),
        val tagMode: String = "any",
        val kind: String? = null,
        val promptId: Long? = null,
        val favorite: Boolean? = null,
        val untagged: Boolean = false,
        val sort: String = "newest",
        val page: Int = 1,
        val size: Int = 24,
    )

    private fun buildWhere(args: SearchArgs): Pair<String, List<Any?>> {
        val where = mutableListOf<String>()
        val params = mutableListOf<Any?>()

        if (!args.q.isNullOrBlank()) {
            val terms = args.q.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.take(8)
            terms.forEach { term ->
                where += """
                    (m.title LIKE ? OR m.original_name LIKE ? OR m.notes LIKE ?
                     OR p.title LIKE ? OR p.positive_prompt LIKE ? OR p.negative_prompt LIKE ?)
                """.trimIndent()
                val like = "%$term%"
                repeat(6) { params += like }
            }
        }

        if (args.tags.isNotEmpty()) {
            // 产物命中条件：产物自己的标签 或 其关联提示词的标签
            val marks = args.tags.joinToString(",") { "?" }
            if (args.tagMode == "all") {
                where += """
                    m.id IN (
                      SELECT x.media_id FROM (
                        SELECT mt.media_id AS media_id, t.normalized AS n
                          FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
                         WHERE t.normalized IN ($marks)
                        UNION
                        SELECT m2.id AS media_id, t2.normalized AS n
                          FROM media_assets m2
                          JOIN prompt_tags pt ON pt.prompt_id = m2.prompt_id
                          JOIN tags t2 ON t2.id = pt.tag_id
                         WHERE t2.normalized IN ($marks)
                      ) x GROUP BY x.media_id HAVING COUNT(DISTINCT x.n) = ${args.tags.size}
                    )
                """.trimIndent()
                params.addAll(args.tags)
                params.addAll(args.tags)
            } else {
                where += """
                    (
                      EXISTS (
                        SELECT 1 FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
                        WHERE mt.media_id = m.id AND t.normalized IN ($marks)
                      )
                      OR EXISTS (
                        SELECT 1 FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
                        WHERE pt.prompt_id = m.prompt_id AND t.normalized IN ($marks)
                      )
                    )
                """.trimIndent()
                params.addAll(args.tags)
                params.addAll(args.tags)
            }
        }

        if (!args.kind.isNullOrBlank() && args.kind != "ALL") {
            where += "m.kind = ?"
            params += args.kind.uppercase()
        }
        args.promptId?.let {
            where += "m.prompt_id = ?"
            params += it
        }
        if (args.favorite == true) where += "m.favorite = 1"
        if (args.untagged) where += "m.prompt_id IS NULL"

        return (if (where.isEmpty()) "" else "WHERE " + where.joinToString(" AND ")) to params
    }

    private fun orderBy(sort: String): String = when (sort) {
        "oldest" -> "m.created_at ASC"
        "updated" -> "m.updated_at DESC"
        "name" -> "m.original_name ASC"
        "largest" -> "m.size_bytes DESC"
        "favorite" -> "m.favorite DESC, m.created_at DESC"
        else -> "m.created_at DESC"
    }

    fun search(args: SearchArgs): PageDto<MediaDto> = Db.withConnection { conn ->
        val (clause, params) = buildWhere(args)
        val size = args.size.coerceIn(1, 200)
        val page = args.page.coerceAtLeast(1)
        val offset = (page - 1) * size

        val total = conn.queryOne(
            "SELECT COUNT(*) $FROM $clause", *params.toTypedArray()
        ) { it.getLong(1) } ?: 0L

        val rows = conn.queryList(
            "SELECT $COLS $FROM $clause ORDER BY ${orderBy(args.sort)} LIMIT ? OFFSET ?",
            *params.toTypedArray(), size, offset
        ) { mapMedia(it) }

        PageDto(
            items = attachOwnTags(conn, rows),
            total = total,
            page = page,
            size = size,
            pages = if (total == 0L) 0 else ((total + size - 1) / size).toInt(),
        )
    }

    fun get(id: Long): MediaDto? = Db.withConnection { conn -> get(conn, id) }

    fun get(conn: Connection, id: Long): MediaDto? {
        val dto = conn.queryOne("SELECT $COLS $FROM WHERE m.id = ?", id) { mapMedia(it) } ?: return null
        return attachOwnTags(conn, listOf(dto)).first()
    }

    fun listByPrompt(promptId: Long): List<MediaDto> = Db.withConnection { conn ->
        val rows = conn.queryList(
            "SELECT $COLS $FROM WHERE m.prompt_id = ? ORDER BY m.created_at DESC", promptId
        ) { mapMedia(it) }
        attachOwnTags(conn, rows)
    }

    /** 检查是否已有相同内容的文件（sha256 去重） */
    fun findBySha(conn: Connection, sha: String): MediaDto? =
        conn.queryOne("SELECT $COLS $FROM WHERE m.sha256 = ? LIMIT 1", sha) { mapMedia(it) }

    fun insert(
        conn: Connection,
        kind: String,
        title: String,
        originalName: String,
        storedName: String,
        mimeType: String?,
        sizeBytes: Long,
        width: Int?,
        height: Int?,
        durationMs: Long?,
        sha256: String?,
        source: String?,
        promptId: Long?,
        notes: String?,
        sourceRef: String? = null,
        workflowJson: String? = null,
    ): Long = conn.executeReturningKey(
        """
        INSERT INTO media_assets
          (prompt_id, kind, title, original_name, stored_name, mime_type, size_bytes,
           width, height, duration_ms, sha256, source, notes, source_ref, workflow_json)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """.trimIndent(),
        promptId, kind, title, originalName, storedName, mimeType, sizeBytes,
        width, height, durationMs, sha256, source, notes, sourceRef, workflowJson
    )

    /** 该产物的工作流快照（自动捕获时会一并存下来） */
    fun workflowJson(id: Long): String? = Db.withConnection { conn ->
        conn.queryOne("SELECT workflow_json FROM media_assets WHERE id = ?", id) { it.getString(1) }
    }

    fun update(conn: Connection, id: Long, input: MediaUpdate): Boolean {
        val sets = mutableListOf<String>()
        val params = mutableListOf<Any?>()

        if (input.clearPrompt) {
            sets += "prompt_id = ?"; params += null
        } else if (input.promptId != null) {
            sets += "prompt_id = ?"; params += input.promptId
        }
        input.title?.let { sets += "title = ?"; params += it }
        input.notes?.let { sets += "notes = ?"; params += it }
        input.source?.let { sets += "source = ?"; params += it }
        input.favorite?.let { sets += "favorite = ?"; params += if (it) 1 else 0 }

        if (sets.isEmpty()) return false
        params += id
        return conn.execute("UPDATE media_assets SET ${sets.joinToString(", ")} WHERE id = ?", *params.toTypedArray()) > 0
    }

    fun setFavorite(conn: Connection, id: Long, favorite: Boolean): Boolean =
        conn.execute("UPDATE media_assets SET favorite = ? WHERE id = ?", if (favorite) 1 else 0, id) > 0

    fun delete(conn: Connection, id: Long): Boolean {
        val ok = conn.execute("DELETE FROM media_assets WHERE id = ?", id) > 0
        if (ok) TagRepo.refreshUseCounts(conn)
        return ok
    }

    /** 把某提示词下的全部产物改挂到另一个提示词 */
    fun reassign(conn: Connection, fromPrompt: Long, toPrompt: Long): Int =
        conn.execute("UPDATE media_assets SET prompt_id = ? WHERE prompt_id = ?", toPrompt, fromPrompt)

    fun stats(conn: Connection): StatsDto {
        val prompts = conn.queryOne("SELECT COUNT(*) FROM prompts") { it.getLong(1) } ?: 0
        val media = conn.queryOne("SELECT COUNT(*) FROM media_assets") { it.getLong(1) } ?: 0
        val tags = conn.queryOne("SELECT COUNT(*) FROM tags") { it.getLong(1) } ?: 0
        val favP = conn.queryOne("SELECT COUNT(*) FROM prompts WHERE favorite = 1") { it.getLong(1) } ?: 0
        val favM = conn.queryOne("SELECT COUNT(*) FROM media_assets WHERE favorite = 1") { it.getLong(1) } ?: 0
        val byKind = conn.queryList("SELECT kind, COUNT(*) c FROM prompts GROUP BY kind") { rs ->
            rs.getString(1) to rs.getLong(2)
        }.toMap()
        val byMedia = conn.queryList("SELECT kind, COUNT(*) c FROM media_assets GROUP BY kind") { rs ->
            rs.getString(1) to rs.getLong(2)
        }.toMap()
        return StatsDto(prompts, media, tags, favP, favM, byKind, byMedia)
    }
}
