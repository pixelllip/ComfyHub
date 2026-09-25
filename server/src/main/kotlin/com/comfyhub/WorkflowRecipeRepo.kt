package com.comfyhub

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import java.sql.Connection
import java.sql.ResultSet

/**
 * 工作流配方仓储（W2，2026-09-26）。
 *
 * 一行 = 一份**已经学过**的工作流：「任务 + 骨架指纹 + 模型槽位 + 参数旋钮」。
 * 归纳（`WorkflowRecipe.analyze`）是纯函数，这里只管**存**与**查**。
 *
 * ## 三条纪律
 *
 * 1. **同一份来源 = 同一条配方**（`source_ref` 唯一索引 + `ON CONFLICT(source_ref) DO UPDATE`）：
 *    模型反复"学一下这条工作流"不该攒出十几行。**不许用 `INSERT OR REPLACE`**
 *    （R3：它会点亮 DELETE 触发器往 `_sync_log` 写假 delete）。本表当前**不同步**，
 *    但这条习惯要留着 —— 哪天进同步范围时不必回头改。
 * 2. **`prompt_id` 是可空外键**（`ON DELETE SET NULL`）：用户删掉那条提示词，配方还在 ——
 *    配方记的是"这一族长什么样"，不依赖某一条具体记录（红线 R1 在这张表上的落点）。
 * 3. **`complete=0` 照样存**，但列表/详情里必须带出去：那是"骨架缺件"的配方
 *    （转换时摘掉了节点类），拿它当完整骨架用会跑出一条**看起来对、其实缺链**的图。
 *    存下来的理由：缺件本身是事实（"这台机器的实例里没有那几个自定义节点"），
 *    下次用户装上了再学一遍就能覆盖成 complete=1。
 */
object WorkflowRecipeRepo {

    /** 名字上限（与提示词标题同一量级）。 */
    const val MAX_NAME_CHARS = 200

    /** 备注上限。 */
    const val MAX_NOTES_CHARS = 2000

    /** 一次最多列几条（给模型看的，不是分页）。 */
    const val MAX_LIST = 100

    /** 一份配方里 JSON 数组文本的上限（防止模型塞进一张 500 节点的图把库撑爆）。 */
    const val MAX_JSON_CHARS = 200_000

    /** 进库 / 出库的形状（键名稳定，界面与 AI 都按它取）。 */
    @Serializable
    data class WorkflowRecipeDto(
        val id: Long,
        val name: String,
        /** t2i / i2i / edit / inpaint / upscale / video / audio / unknown */
        val task: String,
        val skeletonHash: String,
        val recipeVersion: Int,
        /** file / prompt / graph */
        val sourceKind: String,
        val sourceRef: String? = null,
        val sourcePath: String? = null,
        val promptId: Long? = null,
        val nodeCount: Int,
        val classCount: Int,
        val imageInputs: Int,
        val hasMask: Boolean,
        /** false = 这张图转换时有缺口（摘掉了节点 / 有悬空输入），**别当完整骨架用** */
        val complete: Boolean,
        val slots: JsonArray,
        val knobs: JsonArray,
        val classes: JsonArray,
        val edges: JsonArray,
        val outputs: JsonArray,
        val gaps: List<String>,
        val warnings: List<String>,
        val notes: String? = null,
        val createdAt: String? = null,
        val updatedAt: String? = null,
    ) {
        /**
         * 还原成 [WorkflowRecipe.Recipe]（`render()` 与"绑定新模型"都要用**带类型**的槽位 / 旋钮）。
         *
         * 为什么要还原而不是再存一份 render 文本：两份必然漂移，而漂移的表现是
         * "界面上看到的配方与 AI 拿到的不一样"（不报错）。已知字段解析不出来时**如实回空**
         * （一行坏数据不该让整个详情 500），缺口列表里已经带着"转换有缺口"这个事实。
         */
        fun toRecipe(): WorkflowRecipe.Recipe = WorkflowRecipe.Recipe(
            task = WorkflowRecipe.Task.parse(task) ?: WorkflowRecipe.Task.UNKNOWN,
            skeletonHash = skeletonHash,
            nodeClasses = classes.mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
            edges = edges.mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
            slots = slots.mapNotNull { raw ->
                val o = raw as? JsonObject ?: return@mapNotNull null
                val role = o.str("role") ?: return@mapNotNull null
                val nodeId = o.str("nodeId") ?: return@mapNotNull null
                val classType = o.str("classType") ?: return@mapNotNull null
                val inputName = o.str("inputName") ?: return@mapNotNull null
                val fileName = o.str("fileName") ?: return@mapNotNull null
                WorkflowRecipe.Slot(
                    role = role,
                    nodeId = nodeId,
                    classType = classType,
                    inputName = inputName,
                    fileName = fileName,
                    title = o.str("title"),
                    roleGuessed = o.str("roleGuessed") == "true",
                )
            },
            knobs = knobs.mapNotNull { raw ->
                val o = raw as? JsonObject ?: return@mapNotNull null
                val nodeId = o.str("nodeId") ?: return@mapNotNull null
                val classType = o.str("classType") ?: return@mapNotNull null
                val inputName = o.str("inputName") ?: return@mapNotNull null
                WorkflowRecipe.Knob(
                    nodeId = nodeId,
                    classType = classType,
                    inputName = inputName,
                    kind = WorkflowRecipe.KnobKind.parse(o.str("kind")) ?: WorkflowRecipe.KnobKind.OTHER,
                    value = o.str("value").orEmpty(),
                    title = o.str("title"),
                )
            },
            outputs = outputs.mapNotNull { raw ->
                val o = raw as? JsonObject ?: return@mapNotNull null
                WorkflowRecipe.Output(
                    nodeId = o.str("nodeId") ?: return@mapNotNull null,
                    classType = o.str("classType") ?: return@mapNotNull null,
                    kind = o.str("kind").orEmpty(),
                )
            },
            imageInputs = imageInputs,
            hasMask = hasMask,
            nodeCount = nodeCount,
            warnings = warnings,
        )

        /** 给模型/界面看的正文（与 `comfy_learn_workflow` 当场回的那份**跑的是同一个函数**）。 */
        fun renderText(): String = toRecipe().render(name)
    }

    // -----------------------------------------------------------------------
    //  读
    // -----------------------------------------------------------------------

    private const val COLS =
        "id, name, task, skeleton_hash, recipe_version, source_kind, source_ref, source_path, prompt_id, " +
            "node_count, class_count, image_inputs, has_mask, complete, slots_json, knobs_json, " +
            "classes_json, edges_json, outputs_json, warnings_json, gaps_json, notes, created_at, updated_at"

    /** JSON 里取字符串（槽位 / 旋钮的字段都是字符串或布尔，`content` 就够用）。 */
    private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.contentOrNull

    private fun mapRow(rs: ResultSet): WorkflowRecipeDto = WorkflowRecipeDto(
        id = rs.getLong("id"),
        name = rs.strOr("name"),
        task = rs.strOr("task", "unknown"),
        skeletonHash = rs.strOr("skeleton_hash"),
        recipeVersion = rs.intOrNull("recipe_version") ?: WorkflowRecipe.VERSION,
        sourceKind = rs.strOr("source_kind", "file"),
        sourceRef = rs.str("source_ref"),
        sourcePath = rs.str("source_path"),
        promptId = rs.longOrNull("prompt_id"),
        nodeCount = rs.intOrNull("node_count") ?: 0,
        classCount = rs.intOrNull("class_count") ?: 0,
        imageInputs = rs.intOrNull("image_inputs") ?: 0,
        hasMask = rs.boolOr("has_mask"),
        complete = rs.boolOr("complete", true),
        slots = jsonArray(rs.str("slots_json")),
        knobs = jsonArray(rs.str("knobs_json")),
        classes = jsonArray(rs.str("classes_json")),
        edges = jsonArray(rs.str("edges_json")),
        outputs = jsonArray(rs.str("outputs_json")),
        gaps = jsonArray(rs.str("gaps_json")).mapNotNull { (it as? kotlinx.serialization.json.JsonPrimitive)?.content },
        warnings = jsonArray(rs.str("warnings_json"))
            .mapNotNull { (it as? kotlinx.serialization.json.JsonPrimitive)?.content },
        notes = rs.str("notes"),
        createdAt = rs.isoTime("created_at"),
        updatedAt = rs.isoTime("updated_at"),
    )

    /** 坏 JSON 按空数组处理：一条配方的一格坏掉不该让整个列表 500。 */
    private fun jsonArray(raw: String?): JsonArray {
        if (raw.isNullOrBlank()) return JsonArray(emptyList())
        return runCatching { AppJson.parseToJsonElement(raw) as? JsonArray }.getOrNull() ?: JsonArray(emptyList())
    }

    /**
     * 列配方。
     *
     * @param q 关键字：名字 / 骨架指纹 / 源路径 / 模型文件名（JSON 文本里搜）
     * @param task 任务过滤（`t2i` / `edit` …），空 = 不过滤
     * @param skeletonHash 只看某一族（同一骨架）
     * @param onlyComplete true = 只列"没有转换缺口"的（默认 **false**：缺件的也要看得见，只是带上标记）
     */
    fun list(
        q: String? = null,
        task: String? = null,
        skeletonHash: String? = null,
        onlyComplete: Boolean = false,
        limit: Int = MAX_LIST,
    ): List<WorkflowRecipeDto> = Db.withConnection { conn ->
        list(conn, q, task, skeletonHash, onlyComplete, limit)
    }

    fun list(
        conn: Connection,
        q: String? = null,
        task: String? = null,
        skeletonHash: String? = null,
        onlyComplete: Boolean = false,
        limit: Int = MAX_LIST,
    ): List<WorkflowRecipeDto> {
        val where = mutableListOf<String>()
        val params = mutableListOf<Any?>()
        if (!q.isNullOrBlank()) {
            val like = "%${q.trim()}%"
            where += "(name LIKE ? OR skeleton_hash LIKE ? OR source_ref LIKE ? OR source_path LIKE ? " +
                "OR slots_json LIKE ? OR notes LIKE ?)"
            repeat(6) { params += like }
        }
        if (!task.isNullOrBlank()) {
            where += "task = ?"
            params += task.trim().lowercase()
        }
        if (!skeletonHash.isNullOrBlank()) {
            where += "skeleton_hash = ?"
            params += skeletonHash.trim()
        }
        if (onlyComplete) where += "complete = 1"
        val sql = buildString {
            append("SELECT $COLS FROM workflow_recipes")
            if (where.isNotEmpty()) append(" WHERE ").append(where.joinToString(" AND "))
            append(" ORDER BY updated_at DESC, id DESC LIMIT ?")
        }
        return conn.queryList(sql, *params.toTypedArray(), limit.coerceIn(1, 500)) { mapRow(it) }
    }

    fun get(id: Long): WorkflowRecipeDto? = Db.withConnection { conn -> get(conn, id) }

    fun get(conn: Connection, id: Long): WorkflowRecipeDto? =
        conn.queryOne("SELECT $COLS FROM workflow_recipes WHERE id = ?", id) { mapRow(it) }

    fun getBySourceRef(conn: Connection, sourceRef: String): WorkflowRecipeDto? =
        conn.queryOne("SELECT $COLS FROM workflow_recipes WHERE source_ref = ?", sourceRef) { mapRow(it) }

    /**
     * 同一族（同一骨架）的配方有几条 —— "这条骨架我学过几份"。
     *
     * 不返回 `GROUP BY` 的大表：给模型看的是"有几条 + 那几条是谁"，
     * 而列出一族时本来就会按 `skeletonHash` 过滤。
     */
    fun countBySkeleton(conn: Connection, skeletonHash: String): Int =
        conn.queryOne("SELECT COUNT(*) FROM workflow_recipes WHERE skeleton_hash = ?", skeletonHash) { it.getInt(1) } ?: 0

    // -----------------------------------------------------------------------
    //  写
    // -----------------------------------------------------------------------

    /** 进库用的字段（**不含** id / 时间戳：那两个由库定）。 */
    data class Write(
        val name: String,
        val task: String,
        val skeletonHash: String,
        val recipeVersion: Int = WorkflowRecipe.VERSION,
        val sourceKind: String,
        val sourceRef: String?,
        val sourcePath: String? = null,
        val promptId: Long? = null,
        val nodeCount: Int = 0,
        val classCount: Int = 0,
        val imageInputs: Int = 0,
        val hasMask: Boolean = false,
        val complete: Boolean = true,
        val slotsJson: String = "[]",
        val knobsJson: String = "[]",
        val classesJson: String = "[]",
        val edgesJson: String = "[]",
        val outputsJson: String = "[]",
        val warningsJson: String = "[]",
        val gapsJson: String = "[]",
        val notes: String? = null,
    )

    /**
     * 幂等保存（同一份 `source_ref` 只有一行）。返回 `(id, created)`。
     *
     * 用 `ON CONFLICT(source_ref) DO UPDATE`：**不是** `INSERT OR REPLACE`（R3），
     * 而且语义上也对 —— 重新学一份工作流应该是"把这一行更新成刚学到的样子"，
     * 行 id 保持不变（界面上的链接不会失效）。
     */
    fun save(conn: Connection, w: Write): Pair<Long, Boolean> {
        val name = w.name.trim().takeIf { it.isNotEmpty() } ?: "未命名工作流"
        require(name.length <= MAX_NAME_CHARS) { "配方名太长（${name.length} > $MAX_NAME_CHARS 字符）" }
        require(w.task.isNotBlank()) { "task 不能为空（认不出就写 unknown，**不要**编一个）" }
        require(w.skeletonHash.isNotBlank()) { "骨架指纹不能为空" }
        listOf(
            "slotsJson" to w.slotsJson, "knobsJson" to w.knobsJson, "classesJson" to w.classesJson,
            "edgesJson" to w.edgesJson, "outputsJson" to w.outputsJson,
            "warningsJson" to w.warningsJson, "gapsJson" to w.gapsJson,
        ).forEach { (what, value) ->
            require(value.length <= MAX_JSON_CHARS) { "$what 太大（${value.length} > $MAX_JSON_CHARS 字符）" }
        }
        val notes = w.notes?.trim()?.takeIf { it.isNotEmpty() }
        require(notes == null || notes.length <= MAX_NOTES_CHARS) { "备注太长（> $MAX_NOTES_CHARS 字符）" }

        // 有 prompt_id 时先确认它真的在（外键会拦，但那样报出来的是 500 + SQL 原文）
        if (w.promptId != null) {
            val exists = conn.queryOne("SELECT 1 FROM prompts WHERE id = ?", w.promptId) { it.getInt(1) } != null
            require(exists) { "promptId=${w.promptId} 在库里不存在（配方指向它没有意义）" }
        }

        val existing = w.sourceRef?.let { getBySourceRef(conn, it) }
        if (existing != null) {
            conn.execute(
                "UPDATE workflow_recipes SET name=?, task=?, skeleton_hash=?, recipe_version=?, source_kind=?, " +
                    "source_path=?, prompt_id=?, node_count=?, class_count=?, image_inputs=?, has_mask=?, " +
                    "complete=?, slots_json=?, knobs_json=?, classes_json=?, edges_json=?, outputs_json=?, " +
                    "warnings_json=?, gaps_json=?, notes=? WHERE id=?",
                name, w.task, w.skeletonHash, w.recipeVersion, w.sourceKind, w.sourcePath, w.promptId,
                w.nodeCount, w.classCount, w.imageInputs, if (w.hasMask) 1 else 0,
                if (w.complete) 1 else 0, w.slotsJson, w.knobsJson, w.classesJson, w.edgesJson, w.outputsJson,
                w.warningsJson, w.gapsJson, notes, existing.id,
            )
            return existing.id to false
        }

        val id = conn.executeReturningKey(
            "INSERT INTO workflow_recipes (name, task, skeleton_hash, recipe_version, source_kind, source_ref, " +
                "source_path, prompt_id, node_count, class_count, image_inputs, has_mask, complete, " +
                "slots_json, knobs_json, classes_json, edges_json, outputs_json, warnings_json, gaps_json, notes) " +
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            name, w.task, w.skeletonHash, w.recipeVersion, w.sourceKind, w.sourceRef, w.sourcePath, w.promptId,
            w.nodeCount, w.classCount, w.imageInputs, if (w.hasMask) 1 else 0, if (w.complete) 1 else 0,
            w.slotsJson, w.knobsJson, w.classesJson, w.edgesJson, w.outputsJson, w.warningsJson, w.gapsJson, notes,
        )
        return id to true
    }

    fun delete(conn: Connection, id: Long): Boolean =
        conn.execute("DELETE FROM workflow_recipes WHERE id = ?", id) > 0

    /** 把一份 `Recipe` 摊成 [Write] 要的那几个字段（JSON 片段一律由 `Recipe.toJson()` 现取）。 */
    fun writeOf(
        recipe: WorkflowRecipe.Recipe,
        name: String,
        sourceKind: String,
        sourceRef: String?,
        sourcePath: String? = null,
        promptId: Long? = null,
        complete: Boolean = true,
        gaps: List<String> = emptyList(),
        extraWarnings: List<String> = emptyList(),
        notes: String? = null,
    ): Write {
        val json = recipe.toJson()
        // 槽位 / 旋钮 / 输出是**对象数组**，按 JSON 原文存；类清单与边是**字符串数组**
        val slotsJson = encode(json["slots"] as? JsonArray ?: JsonArray(emptyList()))
        val knobsJson = encode(json["knobs"] as? JsonArray ?: JsonArray(emptyList()))
        val outputsJson = encode(json["outputs"] as? JsonArray ?: JsonArray(emptyList()))
        return Write(
            name = name,
            task = recipe.task.wire,
            skeletonHash = recipe.skeletonHash,
            recipeVersion = WorkflowRecipe.VERSION,
            sourceKind = sourceKind,
            sourceRef = sourceRef,
            sourcePath = sourcePath,
            promptId = promptId,
            nodeCount = recipe.nodeCount,
            classCount = recipe.classCount,
            imageInputs = recipe.imageInputs,
            hasMask = recipe.hasMask,
            complete = complete,
            slotsJson = slotsJson,
            knobsJson = knobsJson,
            classesJson = encodeStrings(recipe.nodeClasses),
            edgesJson = encodeStrings(recipe.edges),
            outputsJson = outputsJson,
            warningsJson = encodeStrings(recipe.warnings + extraWarnings),
            gapsJson = encodeStrings(gaps),
            notes = notes,
        )
    }

    private fun encode(array: JsonArray): String =
        runCatching { AppJson.encodeToString(JsonArray.serializer(), array) }.getOrDefault("[]")

    private fun encodeStrings(values: List<String>): String =
        encode(JsonArray(values.map { kotlinx.serialization.json.JsonPrimitive(it) }))
}
