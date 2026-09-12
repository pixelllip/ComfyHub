package com.comfyhub

import java.sql.Connection
import java.sql.ResultSet

/**
 * 标签仓储。标签是全局词表，通过 prompt_tags / media_tags 关联到提示词与产物。
 *
 * use_count 采用「改动后整体重算」策略：数据量在个人使用的量级，
 * 这样能保证计数永远正确，也避免复杂的增量维护逻辑。
 */
object TagRepo {

    fun normalize(name: String): String = name.trim().lowercase()

    private fun mapTag(rs: ResultSet): TagDto = TagDto(
        id = rs.getLong("id"),
        name = rs.strOr("name"),
        category = rs.str("category"),
        color = rs.str("color"),
        description = rs.str("description"),
        useCount = rs.intOrNull("use_count") ?: 0,
        createdAt = rs.isoTime("created_at"),
    )

    private const val BASE_SELECT =
        "SELECT t.id, t.name, t.normalized, t.category, t.color, t.description, t.use_count, t.created_at FROM tags t"

    fun list(
        q: String? = null,
        category: String? = null,
        sort: String = "popular",
        limit: Int = 500,
    ): List<TagDto> = Db.withConnection { conn ->
        val where = mutableListOf<String>()
        val params = mutableListOf<Any?>()

        if (!q.isNullOrBlank()) {
            where += "(t.name LIKE ? OR t.normalized LIKE ? OR t.description LIKE ?)"
            val like = "%${q.trim()}%"
            params += like; params += like; params += like
        }
        if (!category.isNullOrBlank()) {
            where += "t.category = ?"
            params += category
        }

        val order = when (sort) {
            "name" -> "t.normalized ASC"
            "newest" -> "t.created_at DESC"
            "usage", "popular" -> "t.use_count DESC, t.name ASC"
            else -> "t.use_count DESC, t.name ASC"
        }

        val sql = buildString {
            append(BASE_SELECT)
            if (where.isNotEmpty()) append(" WHERE ").append(where.joinToString(" AND "))
            append(" ORDER BY ").append(order)
            append(" LIMIT ?")
        }
        conn.queryList(sql, *params.toTypedArray(), limit.coerceIn(1, 5000)) { mapTag(it) }
    }

    fun categories(): List<String> = Db.withConnection { conn ->
        conn.queryList("SELECT DISTINCT category FROM tags WHERE category IS NOT NULL AND category <> '' ORDER BY category") {
            it.getString(1)
        }
    }

    fun getById(conn: Connection, id: Long): TagDto? =
        conn.queryOne("$BASE_SELECT WHERE t.id = ?", id) { mapTag(it) }

    fun getByNormalized(conn: Connection, normalized: String): TagDto? =
        conn.queryOne("$BASE_SELECT WHERE t.normalized = ?", normalize(normalized)) { mapTag(it) }

    /** 确保标签存在并返回 id；已存在则直接返回原 id（保留原有展示名大小写） */
    fun ensure(conn: Connection, name: String, category: String? = null, color: String? = null): Long? {
        val display = name.trim().take(96)
        if (display.isEmpty()) return null
        val norm = normalize(display).take(96)

        conn.queryOne("SELECT id FROM tags WHERE normalized = ?", norm) { it.getLong(1) }?.let { return it }

        return conn.executeReturningKey(
            "INSERT INTO tags (name, normalized, category, color) VALUES (?, ?, ?, ?)",
            display, norm, category?.takeIf { it.isNotBlank() }, color?.takeIf { it.isNotBlank() }
        )
    }

    fun update(conn: Connection, id: Long, input: TagInput): Boolean {
        val display = input.name.trim().take(96)
        if (display.isEmpty()) return false
        val norm = normalize(display).take(96)
        val dup = conn.queryOne("SELECT id FROM tags WHERE normalized = ? AND id <> ?", norm, id) { it.getLong(1) }
        if (dup != null) throw IllegalArgumentException("标签「$display」已存在")
        return conn.execute(
            "UPDATE tags SET name = ?, normalized = ?, category = ?, color = ?, description = ? WHERE id = ?",
            display, norm,
            input.category?.takeIf { it.isNotBlank() },
            input.color?.takeIf { it.isNotBlank() },
            input.description?.takeIf { it.isNotBlank() },
            id
        ) > 0
    }

    fun delete(conn: Connection, id: Long): Boolean = conn.execute("DELETE FROM tags WHERE id = ?", id) > 0

    /** 批量取提示词的标签，避免 N+1 查询 */
    fun tagsForPrompts(conn: Connection, promptIds: List<Long>): Map<Long, List<TagDto>> {
        if (promptIds.isEmpty()) return emptyMap()
        val marks = promptIds.joinToString(",") { "?" }
        val sql = """
            SELECT pt.prompt_id, t.id, t.name, t.normalized, t.category, t.color, t.description, t.use_count, t.created_at
            FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
            WHERE pt.prompt_id IN ($marks)
            ORDER BY t.name
        """.trimIndent()
        return conn.queryList(sql, *promptIds.toTypedArray()) { rs ->
            rs.getLong("prompt_id") to mapTag(rs)
        }.groupBy({ it.first }, { it.second })
    }

    /** 批量取产物自身的标签名 */
    fun tagsForMedia(conn: Connection, mediaIds: List<Long>): Map<Long, List<String>> {
        if (mediaIds.isEmpty()) return emptyMap()
        val marks = mediaIds.joinToString(",") { "?" }
        val sql = """
            SELECT mt.media_id, t.name
            FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
            WHERE mt.media_id IN ($marks)
            ORDER BY t.name
        """.trimIndent()
        return conn.queryList(sql, *mediaIds.toTypedArray()) { rs ->
            rs.getLong("media_id") to rs.strOr("name")
        }.groupBy({ it.first }, { it.second })
    }

    fun refreshUseCounts(conn: Connection) {
        conn.execute(
            """
            UPDATE tags t SET t.use_count = (
                (SELECT COUNT(*) FROM prompt_tags pt WHERE pt.tag_id = t.id)
              + (SELECT COUNT(*) FROM media_tags  mt WHERE mt.tag_id = t.id)
            )
            """.trimIndent()
        )
    }

    // -----------------------------------------------------------------------
    //  关联维护
    // -----------------------------------------------------------------------

    fun replacePromptTags(conn: Connection, promptId: Long, tagNames: List<String>) {
        conn.execute("DELETE FROM prompt_tags WHERE prompt_id = ?", promptId)
        addPromptTags(conn, promptId, tagNames)
    }

    fun addPromptTags(conn: Connection, promptId: Long, tagNames: List<String>) {
        tagNames.mapNotNull { ensure(conn, it) }.distinct().forEach { tagId ->
            conn.execute("INSERT IGNORE INTO prompt_tags (prompt_id, tag_id) VALUES (?, ?)", promptId, tagId)
        }
    }

    fun removePromptTag(conn: Connection, promptId: Long, tagId: Long) {
        conn.execute("DELETE FROM prompt_tags WHERE prompt_id = ? AND tag_id = ?", promptId, tagId)
    }

    fun setMediaTags(conn: Connection, mediaId: Long, tagNames: List<String>) {
        conn.execute("DELETE FROM media_tags WHERE media_id = ?", mediaId)
        tagNames.mapNotNull { ensure(conn, it) }.distinct().forEach { tagId ->
            conn.execute("INSERT IGNORE INTO media_tags (media_id, tag_id) VALUES (?, ?)", mediaId, tagId)
        }
    }

    /** 解析标签过滤参数：逗号分隔，统一小写去空格 */
    fun parseTagParam(raw: String?): List<String> =
        raw?.split(',')
            ?.map { normalize(it) }
            ?.filter { it.isNotEmpty() }
            ?.distinct()
            ?: emptyList()

    /**
     * 构造「提示词必须拥有这些标签」的 SQL 片段。
     * @param mode any = 命中任意一个；all = 必须全部命中
     */
    fun promptTagFilter(tagNormals: List<String>, mode: String, params: MutableList<Any?>): String? {
        if (tagNormals.isEmpty()) return null
        val marks = tagNormals.joinToString(",") { "?" }
        params.addAll(tagNormals)
        return if (mode == "all") {
            """
            p.id IN (
              SELECT pt.prompt_id FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
              WHERE t.normalized IN ($marks)
              GROUP BY pt.prompt_id HAVING COUNT(DISTINCT t.normalized) = ${tagNormals.size}
            )
            """.trimIndent()
        } else {
            """
            p.id IN (
              SELECT pt.prompt_id FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
              WHERE t.normalized IN ($marks)
            )
            """.trimIndent()
        }
    }
}
