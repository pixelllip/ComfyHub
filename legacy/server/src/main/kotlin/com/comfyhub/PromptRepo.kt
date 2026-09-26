package com.comfyhub

import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer
import java.sql.Connection
import java.sql.ResultSet

object PromptRepo {

    val KINDS = setOf("IMAGE", "VIDEO", "AUDIO", "MIXED")

    private const val COLUMNS = """
        p.id, p.title, p.kind, p.positive_prompt, p.negative_prompt, p.checkpoint, p.loras,
        p.sampler, p.scheduler, p.steps, p.cfg_scale, p.seed, p.width, p.height, p.batch_size,
        p.extra_params, p.notes, p.favorite, p.created_at, p.updated_at,
        p.source, p.source_ref, (p.workflow_json IS NOT NULL) AS has_workflow
    """

    // -----------------------------------------------------------------------
    //  映射
    // -----------------------------------------------------------------------

    private fun decodeLoras(raw: String?): List<LoraRef> {
        if (raw.isNullOrBlank()) return emptyList()
        return runCatching {
            AppJson.decodeFromString(ListSerializer(LoraRef.serializer()), raw)
        }.getOrDefault(emptyList())
    }

    private fun decodeExtra(raw: String?): Map<String, String> {
        if (raw.isNullOrBlank()) return emptyMap()
        return runCatching {
            AppJson.decodeFromString(MapSerializer(String.serializer(), String.serializer()), raw)
        }.getOrDefault(emptyMap())
    }

    // 注意：这里必须显式给出序列化器。
    // 写成 `encode(value: Any)` 时 reified T 会退化成 Any，运行时找不到序列化器，
    // 异常又被 runCatching 吞掉 —— 结果就是 loras / extra_params 静默存成 NULL。
    private fun encodeLoras(value: List<LoraRef>): String? =
        runCatching { AppJson.encodeToString(ListSerializer(LoraRef.serializer()), value) }.getOrNull()

    private fun encodeExtra(value: Map<String, String>): String? =
        runCatching {
            AppJson.encodeToString(MapSerializer(String.serializer(), String.serializer()), value)
        }.getOrNull()

    fun mapPrompt(rs: ResultSet, tags: List<TagDto> = emptyList(), mediaCount: Int = 0): PromptDto = PromptDto(
        id = rs.getLong("id"),
        title = rs.strOr("title"),
        kind = rs.strOr("kind", "IMAGE"),
        positivePrompt = rs.strOr("positive_prompt"),
        negativePrompt = rs.str("negative_prompt"),
        checkpoint = rs.str("checkpoint"),
        loras = decodeLoras(rs.str("loras")),
        sampler = rs.str("sampler"),
        scheduler = rs.str("scheduler"),
        steps = rs.intOrNull("steps"),
        cfgScale = rs.doubleOrNull("cfg_scale"),
        seed = rs.longOrNull("seed"),
        width = rs.intOrNull("width"),
        height = rs.intOrNull("height"),
        batchSize = rs.intOrNull("batch_size"),
        extraParams = decodeExtra(rs.str("extra_params")),
        notes = rs.str("notes"),
        favorite = rs.boolOr("favorite"),
        source = rs.str("source"),
        sourceRef = rs.str("source_ref"),
        hasWorkflow = rs.boolOr("has_workflow"),
        createdAt = rs.isoTime("created_at"),
        updatedAt = rs.isoTime("updated_at"),
        tags = tags,
        mediaCount = mediaCount,
    )

    private fun attachRelations(conn: Connection, prompts: List<PromptDto>): List<PromptDto> {
        if (prompts.isEmpty()) return prompts
        val ids = prompts.map { it.id }
        val tagMap = TagRepo.tagsForPrompts(conn, ids)
        val marks = ids.joinToString(",") { "?" }
        val counts = conn.queryList(
            "SELECT prompt_id, COUNT(*) AS c FROM media_assets WHERE prompt_id IN ($marks) GROUP BY prompt_id",
            *ids.toTypedArray()
        ) { rs -> rs.getLong("prompt_id") to rs.getInt("c") }.toMap()
        return prompts.map { it.copy(tags = tagMap[it.id].orEmpty(), mediaCount = counts[it.id] ?: 0) }
    }

    // -----------------------------------------------------------------------
    //  查询
    // -----------------------------------------------------------------------

    data class SearchArgs(
        val q: String? = null,
        val tags: List<String> = emptyList(),
        val tagMode: String = "any",
        val kind: String? = null,
        val favorite: Boolean? = null,
        val hasMedia: Boolean? = null,
        val sort: String = "newest",
        val page: Int = 1,
        val size: Int = 20,
    )

    private fun buildWhere(args: SearchArgs): Pair<String, List<Any?>> {
        val where = mutableListOf<String>()
        val params = mutableListOf<Any?>()

        if (!args.q.isNullOrBlank()) {
            // 按空白拆词，每个词都要命中（AND），兼容中文场景（用 LIKE 而非 FULLTEXT）
            val terms = args.q.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.take(8)
            terms.forEach { term ->
                where += """
                    (p.title LIKE ? OR p.positive_prompt LIKE ? OR p.negative_prompt LIKE ?
                     OR p.notes LIKE ? OR p.checkpoint LIKE ?)
                """.trimIndent()
                val like = "%$term%"
                repeat(5) { params += like }
            }
        }

        TagRepo.promptTagFilter(args.tags, args.tagMode, params)?.let { where += it }

        if (!args.kind.isNullOrBlank() && args.kind != "ALL") {
            where += "p.kind = ?"
            params += args.kind.uppercase()
        }
        if (args.favorite == true) where += "p.favorite = 1"
        if (args.hasMedia == true) where += "EXISTS (SELECT 1 FROM media_assets m WHERE m.prompt_id = p.id)"
        if (args.hasMedia == false) where += "NOT EXISTS (SELECT 1 FROM media_assets m WHERE m.prompt_id = p.id)"

        val clause = if (where.isEmpty()) "" else "WHERE " + where.joinToString(" AND ")
        return clause to params
    }

    private fun orderBy(sort: String): String = when (sort) {
        "oldest" -> "p.created_at ASC"
        "updated" -> "p.updated_at DESC"
        "title" -> "p.title ASC, p.id DESC"
        "favorite" -> "p.favorite DESC, p.created_at DESC"
        else -> "p.created_at DESC"
    }

    fun search(args: SearchArgs): PageDto<PromptDto> = Db.withConnection { conn ->
        val (clause, params) = buildWhere(args)
        val size = args.size.coerceIn(1, 200)
        val page = args.page.coerceAtLeast(1)
        val offset = (page - 1) * size

        val total = conn.queryOne("SELECT COUNT(*) FROM prompts p $clause", *params.toTypedArray()) {
            it.getLong(1)
        } ?: 0L

        val rows = conn.queryList(
            "SELECT $COLUMNS FROM prompts p $clause ORDER BY ${orderBy(args.sort)} LIMIT ? OFFSET ?",
            *params.toTypedArray(), size, offset
        ) { mapPrompt(it) }

        PageDto(
            items = attachRelations(conn, rows),
            total = total,
            page = page,
            size = size,
            pages = if (total == 0L) 0 else ((total + size - 1) / size).toInt(),
        )
    }

    fun get(id: Long): PromptDto? = Db.withConnection { conn -> get(conn, id) }

    fun get(conn: Connection, id: Long): PromptDto? {
        val dto = conn.queryOne("SELECT $COLUMNS FROM prompts p WHERE p.id = ?", id) { mapPrompt(it) } ?: return null
        return attachRelations(conn, listOf(dto)).first()
    }

    fun allTagsOf(conn: Connection): List<String> =
        conn.queryList("SELECT name FROM tags ORDER BY name") { it.getString(1) }

    // -----------------------------------------------------------------------
    //  写入
    // -----------------------------------------------------------------------

    fun create(conn: Connection, input: PromptInput): Long {
        require(input.positivePrompt.isNotBlank() || input.title.isNotBlank()) { "提示词内容不能为空" }
        val kind = input.kind.uppercase().takeIf { it in KINDS } ?: "IMAGE"

        val id = conn.executeReturningKey(
            """
            INSERT INTO prompts
              (title, kind, positive_prompt, negative_prompt, checkpoint, loras, sampler, scheduler,
               steps, cfg_scale, seed, width, height, batch_size, extra_params, notes, favorite)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            """.trimIndent(),
            input.title.trim().ifBlank { autoTitle(input.positivePrompt) },
            kind,
            input.positivePrompt,
            input.negativePrompt?.takeIf { it.isNotBlank() },
            input.checkpoint?.takeIf { it.isNotBlank() },
            encodeLoras(input.loras),
            input.sampler?.takeIf { it.isNotBlank() },
            input.scheduler?.takeIf { it.isNotBlank() },
            input.steps,
            input.cfgScale,
            input.seed,
            input.width,
            input.height,
            input.batchSize,
            encodeExtra(input.extraParams),
            input.notes?.takeIf { it.isNotBlank() },
            if (input.favorite) 1 else 0,
        )

        if (input.tags.isNotEmpty()) TagRepo.addPromptTags(conn, id, input.tags)
        TagRepo.refreshUseCounts(conn)
        return id
    }

    fun update(conn: Connection, id: Long, input: PromptInput): Boolean {
        val kind = input.kind.uppercase().takeIf { it in KINDS } ?: "IMAGE"
        val affected = conn.execute(
            """
            UPDATE prompts SET
              title = ?, kind = ?, positive_prompt = ?, negative_prompt = ?, checkpoint = ?, loras = ?,
              sampler = ?, scheduler = ?, steps = ?, cfg_scale = ?, seed = ?, width = ?, height = ?,
              batch_size = ?, extra_params = ?, notes = ?, favorite = ?
            WHERE id = ?
            """.trimIndent(),
            input.title.trim().ifBlank { autoTitle(input.positivePrompt) },
            kind,
            input.positivePrompt,
            input.negativePrompt?.takeIf { it.isNotBlank() },
            input.checkpoint?.takeIf { it.isNotBlank() },
            encodeLoras(input.loras),
            input.sampler?.takeIf { it.isNotBlank() },
            input.scheduler?.takeIf { it.isNotBlank() },
            input.steps,
            input.cfgScale,
            input.seed,
            input.width,
            input.height,
            input.batchSize,
            encodeExtra(input.extraParams),
            input.notes?.takeIf { it.isNotBlank() },
            if (input.favorite) 1 else 0,
            id
        )
        if (affected > 0) {
            TagRepo.replacePromptTags(conn, id, input.tags)
            TagRepo.refreshUseCounts(conn)
        }
        return affected > 0
    }

    fun setFavorite(conn: Connection, id: Long, favorite: Boolean): Boolean =
        conn.execute("UPDATE prompts SET favorite = ? WHERE id = ?", if (favorite) 1 else 0, id) > 0

    /** 删除提示词：关联产物保留但解除关联（FK ON DELETE SET NULL） */
    fun delete(conn: Connection, id: Long): Boolean {
        val ok = conn.execute("DELETE FROM prompts WHERE id = ?", id) > 0
        if (ok) TagRepo.refreshUseCounts(conn)
        return ok
    }

    /** 复制一条提示词 */
    fun duplicate(conn: Connection, id: Long): Long? {
        val src = get(conn, id) ?: return null
        return create(
            conn,
            PromptInput(
                title = "${src.title} 副本",
                kind = src.kind,
                positivePrompt = src.positivePrompt,
                negativePrompt = src.negativePrompt,
                checkpoint = src.checkpoint,
                loras = src.loras,
                sampler = src.sampler,
                scheduler = src.scheduler,
                steps = src.steps,
                cfgScale = src.cfgScale,
                seed = src.seed,
                width = src.width,
                height = src.height,
                batchSize = src.batchSize,
                extraParams = src.extraParams,
                notes = src.notes,
                favorite = false,
                tags = src.tags.map { it.name },
            )
        )
    }

    /**
     * 自动捕获专用：直接用解析好的结果建一条提示词。
     *
     * 比 `create()` 多带三样东西：`source`（来源）、`source_ref`（幂等键）、
     * `workflow_json`（界面格式工作流快照，随时能拖回 ComfyUI 复现）。
     */
    fun createCaptured(
        conn: Connection,
        parsed: GraphParse.Parsed,
        title: String,
        source: String,
        sourceRef: String?,
        workflowJson: String?,
        tags: List<String>,
        notes: String? = null,
    ): Long {
        val id = conn.executeReturningKey(
            """
            INSERT INTO prompts
              (title, kind, positive_prompt, negative_prompt, checkpoint, loras, sampler, scheduler,
               steps, cfg_scale, seed, width, height, batch_size, extra_params, notes, favorite,
               source, source_ref, workflow_json)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            """.trimIndent(),
            title.trim().take(255).ifBlank { autoTitle(parsed.positive) },
            parsed.kind.uppercase().takeIf { it in KINDS } ?: "IMAGE",
            parsed.positive,
            parsed.negative?.takeIf { it.isNotBlank() },
            parsed.checkpoint?.takeIf { it.isNotBlank() },
            encodeLoras(parsed.loras),
            parsed.sampler?.takeIf { it.isNotBlank() },
            parsed.scheduler?.takeIf { it.isNotBlank() },
            parsed.steps,
            parsed.cfg,
            parsed.seed,
            parsed.width,
            parsed.height,
            parsed.batch,
            encodeExtra(parsed.extra),
            notes?.takeIf { it.isNotBlank() },
            0,
            source.take(32),
            sourceRef?.take(128),
            workflowJson,
        )
        if (tags.isNotEmpty()) TagRepo.addPromptTags(conn, id, tags)
        TagRepo.refreshUseCounts(conn)
        return id
    }

    /** 工作流原文（可能有几百 KB，只在详情页按需拉取） */
    fun workflowJson(id: Long): String? = Db.withConnection { conn ->
        conn.queryOne("SELECT workflow_json FROM prompts WHERE id = ?", id) { it.getString(1) }
    }

    private fun autoTitle(positive: String): String {
        val first = positive.trim().lineSequence().firstOrNull().orEmpty()
        val head = first.split(',').firstOrNull()?.trim().orEmpty()
        return head.take(60).ifBlank { "未命名提示词" }
    }
}
