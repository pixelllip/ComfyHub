package com.comfyhub

import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import java.sql.Connection
import java.sql.ResultSet

/**
 * 角色卡仓储（O8，2026-09-25）。
 *
 * 一张卡 = 一个角色的**规范 tag 栈 + 来源 + 配哪条 LoRA / 权重**。它要解决的现场问题是
 * "AI 每次写角色都靠印象编 tag"：库里明明有 `shinku_(nte)` 的完整 tag 栈（prompts 261/310），
 * 模型却把 shinku 当成 iroi 写了两版废图（总纲 §10.3 O7/O8）。
 *
 * ## 三条纪律
 *
 * 1. **同名 = 同一张卡**（`normalized` 唯一索引 + `ON CONFLICT(normalized) DO UPDATE`）：
 *    模型反复"记一下这个角色"不该攒出三四份；也**不许**用 `INSERT OR REPLACE`
 *    （R3：它会点亮 DELETE 触发器往 `_sync_log` 写假 delete）。本表当前**不同步**，
 *    但这条习惯要留着 —— 哪天进同步范围时不必回头改。
 * 2. **来源必须如实**（[CharacterCardDto.source] / [CharacterCardDto.tagsVerifiedAt]）：
 *    "我从 danbooru 查到的" 与 "我按印象写的" 是两件事，界面上要能分开看。
 *    所以核对时间由**调 danbooru 的那条路**写，不由模型自己填（见 `DanbooruClient`）。
 * 3. **写不进去就报错，不许静默截断**：字段超长/名字为空一律抛 `IllegalArgumentException`
 *    （→ 400 + 人话），与 `MemoryStore` 的取向一致（悄悄丢用户的话更糟）。
 */
object CharacterCardRepo {

    /** 名字上限（与 tags.name 的 96 同一档）。 */
    const val MAX_NAME_CHARS = 96

    /** 一张卡的 tag 栈上限：100 个 tag 已经远超任何角色（再多是模型跑飞了）。 */
    const val MAX_TAGS = 100

    /** 单个 tag 上限。 */
    const val MAX_TAG_CHARS = 96

    /** 备注上限（够写一整段"这个角色怎么画才对"）。 */
    const val MAX_NOTES_CHARS = 4000

    /** 搜索一次最多回几张卡（给模型看的，不是分页）。 */
    const val MAX_SEARCH = 50

    fun normalize(name: String): String = name.trim().lowercase()

    // -----------------------------------------------------------------------
    //  值转换
    // -----------------------------------------------------------------------

    private fun decodeList(raw: String?): List<String> {
        if (raw.isNullOrBlank()) return emptyList()
        return runCatching { AppJson.decodeFromString(ListSerializer(String.serializer()), raw) }
            .getOrDefault(emptyList())
    }

    private fun encodeList(value: List<String>): String? =
        if (value.isEmpty()) null
        else runCatching { AppJson.encodeToString(ListSerializer(String.serializer()), value) }.getOrNull()

    /**
     * 把模型/用户写的一串 tag 拆成一项一项。
     *
     * 分隔符认三种：英文逗号、中文逗号、换行（模型三种都会写；`derpibooru` 风格的空格分隔
     * **不认** —— 那会把 `red eyes` 拆成两个不存在的 tag，比不拆更糟）。
     * 去重保序（同一批里重复的 tag 没有意义，还会稀释权重）。
     */
    fun splitTags(text: String): List<String> {
        val out = LinkedHashSet<String>()
        text.split(',', '，', '\n', '\r', ';', '；')
            .asSequence()
            .map { normalizeTag(it) }
            .filter { it.isNotEmpty() }
            .forEach { out += it }
        return out.toList()
    }

    /** 单个 tag 的清洗：压空白、去掉列表符号（`- ` / `* ` / 行首数字序号）。 */
    fun normalizeTag(raw: String): String {
        var tag = raw.trim().replace(Regex("\\s+"), " ")
        tag = tag.removePrefix("- ").removePrefix("* ").trim()
        tag = tag.replace(Regex("^\\d+[.、)]\\s*"), "").trim()
        return tag.take(MAX_TAG_CHARS)
    }

    private fun cleanTags(tags: List<String>): List<String> =
        tags.asSequence()
            .map { normalizeTag(it) }
            .filter { it.isNotEmpty() }
            .distinct()
            .take(MAX_TAGS)
            .toList()

    private fun cleanAliases(aliases: List<String>): List<String> =
        aliases.asSequence().map { it.trim() }.filter { it.isNotEmpty() }.distinct().take(20).toList()

    /** 字段超长**报错**而不是截断（第 3 条纪律）。 */
    private fun requireFits(value: String?, limit: Int, what: String): String? {
        val v = value?.trim()?.takeIf { it.isNotEmpty() } ?: return null
        require(v.length <= limit) { "$what 超出上限（${v.length} > $limit 字符）；请拆成多条，不要指望被悄悄截断" }
        return v
    }

    // -----------------------------------------------------------------------
    //  读
    // -----------------------------------------------------------------------

    private const val BASE_SELECT =
        "SELECT id, name, normalized, name_en, aliases, character_tag, series_tag, tags, negative, " +
            "lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, source, " +
            "tags_verified_at, notes, created_at, updated_at FROM character_cards"

    private fun mapRow(rs: ResultSet): CharacterCardDto = CharacterCardDto(
        id = rs.getLong("id"),
        name = rs.strOr("name"),
        nameEn = rs.str("name_en"),
        aliases = decodeList(rs.str("aliases")),
        characterTag = rs.str("character_tag"),
        seriesTag = rs.str("series_tag"),
        tags = decodeList(rs.str("tags")),
        negative = rs.str("negative"),
        loraName = rs.str("lora_name"),
        loraWeight = rs.doubleOrNull("lora_weight"),
        loraTrigger = rs.str("lora_trigger"),
        checkpoint = rs.str("checkpoint"),
        referenceMediaId = rs.longOrNull("reference_media_id"),
        source = rs.str("source"),
        tagsVerifiedAt = rs.isoTime("tags_verified_at"),
        notes = rs.str("notes"),
        createdAt = rs.isoTime("created_at"),
        updatedAt = rs.isoTime("updated_at"),
    )

    /**
     * 按关键字搜索：名字 / 别名 / 角色 tag / 作品 tag / tag 栈 / LoRA 名 / 备注 全都命中。
     *
     * `q` 为空 = 列全部（界面首屏）。
     */
    fun list(q: String? = null, limit: Int = MAX_SEARCH): List<CharacterCardDto> = Db.withConnection { conn ->
        list(conn, q, limit)
    }

    fun list(conn: Connection, q: String? = null, limit: Int = MAX_SEARCH): List<CharacterCardDto> {
        val where = mutableListOf<String>()
        val params = mutableListOf<Any?>()
        if (!q.isNullOrBlank()) {
            val like = "%${q.trim()}%"
            where += "(name LIKE ? OR normalized LIKE ? OR name_en LIKE ? OR aliases LIKE ? " +
                "OR character_tag LIKE ? OR series_tag LIKE ? OR tags LIKE ? OR lora_name LIKE ? " +
                "OR lora_trigger LIKE ? OR notes LIKE ?)"
            repeat(10) { params += like }
        }
        val sql = buildString {
            append(BASE_SELECT)
            if (where.isNotEmpty()) append(" WHERE ").append(where.joinToString(" AND "))
            append(" ORDER BY updated_at DESC, id DESC LIMIT ?")
        }
        return conn.queryList(sql, *params.toTypedArray(), limit.coerceIn(1, 500)) { mapRow(it) }
    }

    fun get(id: Long): CharacterCardDto? = Db.withConnection { conn -> get(conn, id) }

    /** 事务内复用（**不要**在 `Db.tx` 里再调 `withConnection`：会再借一条连接撞写锁）。 */
    fun get(conn: Connection, id: Long): CharacterCardDto? =
        conn.queryOne("$BASE_SELECT WHERE id = ?", id) { mapRow(it) }

    fun getByNormalized(conn: Connection, name: String): CharacterCardDto? =
        conn.queryOne("$BASE_SELECT WHERE normalized = ?", normalize(name)) { mapRow(it) }

    // -----------------------------------------------------------------------
    //  写
    // -----------------------------------------------------------------------

    private fun validate(input: CharacterCardInput): String {
        val name = input.name.trim()
        require(name.isNotEmpty()) { "角色名不能为空" }
        require(name.length <= MAX_NAME_CHARS) { "角色名太长（${name.length} > $MAX_NAME_CHARS 字符）" }
        return name
    }

    /** 参考图必须真的存在（外键会拦，但那样报出来的是 500 + SQL 原文，看不懂）。 */
    private fun requireMediaExists(conn: Connection, mediaId: Long?) {
        if (mediaId == null) return
        val exists = conn.queryOne("SELECT 1 FROM media_assets WHERE id = ?", mediaId) { it.getInt(1) } != null
        require(exists) { "参考图（产物 $mediaId）不存在；先确认它在画廊里，再填 referenceMediaId" }
    }

    fun create(conn: Connection, input: CharacterCardInput): Long {
        val name = validate(input)
        requireMediaExists(conn, input.referenceMediaId)
        return conn.executeReturningKey(
            "INSERT INTO character_cards (name, normalized, name_en, aliases, character_tag, series_tag, " +
                "tags, negative, lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, " +
                "source, tags_verified_at, notes) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            name,
            normalize(name),
            requireFits(input.nameEn, MAX_NAME_CHARS, "英文名"),
            encodeList(cleanAliases(input.aliases)),
            requireFits(input.characterTag, MAX_TAG_CHARS, "角色 tag"),
            requireFits(input.seriesTag, MAX_TAG_CHARS, "作品 tag"),
            encodeList(cleanTags(input.tags)),
            requireFits(input.negative, MAX_NOTES_CHARS, "负面词"),
            requireFits(input.loraName, 256, "LoRA 文件名"),
            input.loraWeight,
            requireFits(input.loraTrigger, MAX_NOTES_CHARS, "LoRA 触发词"),
            requireFits(input.checkpoint, 256, "底模名"),
            input.referenceMediaId,
            requireFits(input.source, 500, "来源"),
            requireFits(input.tagsVerifiedAt, 64, "核对时间"),
            requireFits(input.notes, MAX_NOTES_CHARS, "备注"),
        )
    }

    /** 全量替换（界面上的"编辑保存"用它：弹窗里所有字段都在）。 */
    fun update(conn: Connection, id: Long, input: CharacterCardInput): Boolean {
        val name = validate(input)
        requireMediaExists(conn, input.referenceMediaId)
        return conn.execute(
            "UPDATE character_cards SET name = ?, normalized = ?, name_en = ?, aliases = ?, " +
                "character_tag = ?, series_tag = ?, tags = ?, negative = ?, lora_name = ?, " +
                "lora_weight = ?, lora_trigger = ?, checkpoint = ?, reference_media_id = ?, " +
                "source = ?, tags_verified_at = ?, notes = ? WHERE id = ?",
            name,
            normalize(name),
            requireFits(input.nameEn, MAX_NAME_CHARS, "英文名"),
            encodeList(cleanAliases(input.aliases)),
            requireFits(input.characterTag, MAX_TAG_CHARS, "角色 tag"),
            requireFits(input.seriesTag, MAX_TAG_CHARS, "作品 tag"),
            encodeList(cleanTags(input.tags)),
            requireFits(input.negative, MAX_NOTES_CHARS, "负面词"),
            requireFits(input.loraName, 256, "LoRA 文件名"),
            input.loraWeight,
            requireFits(input.loraTrigger, MAX_NOTES_CHARS, "LoRA 触发词"),
            requireFits(input.checkpoint, 256, "底模名"),
            input.referenceMediaId,
            requireFits(input.source, 500, "来源"),
            requireFits(input.tagsVerifiedAt, 64, "核对时间"),
            requireFits(input.notes, MAX_NOTES_CHARS, "备注"),
            id,
        ) > 0
    }

    fun delete(conn: Connection, id: Long): Boolean =
        conn.execute("DELETE FROM character_cards WHERE id = ?", id) > 0

    /**
     * 合并（**AI 保存角色卡时用这个**，不是全量替换）。
     *
     * 为什么：模型一次只会说一件事（"这个角色的 LoRA 是 iroi-anima"），
     * 全量替换会把它上一轮存下来的 tag 栈、来源、别名一起抹掉 —— 那是**悄悄丢用户的数据**。
     * 所以：[incoming] 里为空的字段一律保留 [existing] 的值；只有明确给了值才覆盖。
     *
     * `tags` 是**追加去重**（不是覆盖）：模型常常一次补一两个 tag
     * （"再想想，她还戴蝴蝶结"），覆盖同样会丢东西。要去掉某个 tag 就整串重给一次
     * 并在界面里删 —— 这条写在工具描述里。
     */
    fun merge(existing: CharacterCardDto?, incoming: CharacterCardInput): CharacterCardInput {
        if (existing == null) return incoming
        return incoming.copy(
            name = incoming.name.trim().ifEmpty { existing.name },
            nameEn = incoming.nameEn ?: existing.nameEn,
            aliases = (existing.aliases + incoming.aliases).distinct(),
            characterTag = incoming.characterTag ?: existing.characterTag,
            seriesTag = incoming.seriesTag ?: existing.seriesTag,
            tags = (existing.tags + incoming.tags).distinct(),
            negative = incoming.negative ?: existing.negative,
            loraName = incoming.loraName ?: existing.loraName,
            loraWeight = incoming.loraWeight ?: existing.loraWeight,
            loraTrigger = incoming.loraTrigger ?: existing.loraTrigger,
            checkpoint = incoming.checkpoint ?: existing.checkpoint,
            referenceMediaId = incoming.referenceMediaId ?: existing.referenceMediaId,
            source = incoming.source ?: existing.source,
            tagsVerifiedAt = incoming.tagsVerifiedAt ?: existing.tagsVerifiedAt,
            notes = incoming.notes ?: existing.notes,
        )
    }

    /**
     * 按名字幂等保存：没有就建、有就合并。返回 `(id, created)`。
     *
     * 用 `Db.tx` 的调用方拿 `conn` 版本；这是给工具/路由用的便利包装。
     */
    fun save(conn: Connection, input: CharacterCardInput): Pair<Long, Boolean> {
        val name = validate(input)
        val existing = getByNormalized(conn, name)
        if (existing == null) return create(conn, input) to true
        val merged = merge(existing, input)
        update(conn, existing.id, merged)
        return existing.id to false
    }
}
