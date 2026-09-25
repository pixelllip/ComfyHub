package com.comfyhub

import com.comfyhub.ai.AiException
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.file.Files
import java.nio.file.Path
import java.security.MessageDigest

/**
 * **学习一份工作流**（W2/W3，2026-09-26）：把"某个来源"读成 API 节点图 → 归纳成配方 →（可选）落库。
 *
 * 它是 `comfy_learn_workflow`（AI 工具）与 `POST /api/ai/workflow-recipes`（界面）**共用的那一份实现**
 * —— 两处各写一遍必然漂移，而漂移的表现是"AI 学出来的配方和界面看到的不一样"（不报错）。
 *
 * ## 三种来源（各有各的幂等键，与 `ComfyWorkflowFiles` / `comfy_load_workflow` 同一把钥匙）
 *
 * | 来源 | 参数 | `source_ref` | 说明 |
 * | --- | --- | --- | --- |
 * | 本机文件 | `path` | `file:<sha256>` | 界面格式会经 `WorkflowConvert` 转成 API 图（**要 ComfyUI 在场**） |
 * | 库里已有 | `promptId` | `prompt:<id>` | 取 `capture_runs.raw` 里那份 API 图（`ComfyCapture.apiGraphOf`） |
 * | 直接给图 | `graph` | `graph:<sha256>` | 模型从别处复制来的节点图（没有文件、没有库记录） |
 *
 * **学习不改用户的文件、也不往 `prompts` 里塞行**：配方只记"这一族长什么样"。
 * 真提交仍走 `comfy_submit(promptId=…)`（R10：审批 → 提交 → 等 → 入库），
 * 需要 promptId 时用 `comfy_load_workflow(path=…)` —— 它与这里的 `file:<sha256>` 是同一把钥匙，
 * 所以"学过的文件"再去读，拿到的还是那条提示词。
 *
 * ## ⚠ 最要紧的一条纪律：**不许学一条缺了节点的骨架**
 *
 * 界面格式转 API 图时会**摘掉**它转换不了的节点（`unsupportedNodes`）并留下悬空输入。
 * 现场（2026-09-26 真机抽查）：用户有两份 Krea 2 图像编辑工作流，而**当前跑着的实例里
 * 没有那几个 Krea2 自定义节点**（`/object_info` 2817 个类里一个都没有）——
 * 转换于是把整条编辑链摘掉了，19 个节点只剩 13 个。如果就这么学下去，
 * 配方会是一条**看着像、其实缺了编辑链**的骨架，而且**没有任何报错**。
 *
 * 所以默认档是**拒学**（[tolerateUnsupported] = false）：把摘掉了哪些类如实列出来，
 * 并明确指出"先确认本机这个实例有没有这些节点"。带 `tolerateUnsupported=true` 才学，
 * 而且配方会带 `complete=false` 与 `gaps`（读的时候必须如实说"骨架缺件"）。
 */
object WorkflowRecipeLearner {

    /** 一份工作流文件最大读多少（与 `AiWorkflowSearch.MAX_WORKFLOW_FILE_BYTES` 同一档）。 */
    const val MAX_FILE_BYTES = 32L * 1024 * 1024

    /** 缺口清单最多列几条（工具结果会被截断，列表本身也不该淹掉结论）。 */
    const val MAX_GAPS = 40

    /** 带出去的转换警告最多几条。 */
    const val MAX_CONVERT_WARNINGS = 5

    /**
     * 依赖（全是从外面注入的**读**入口 —— 这一层不碰数据库、不碰网络）。
     *
     * @param objectInfo ComfyUI 的 `/object_info`（拿不到回 null）：界面格式转换**必须**用它，
     *   模型槽位也用它现算自定义加载器。
     * @param apiGraphOf 按 promptId 取 API 图（`ComfyCapture.apiGraphOf`）。
     * @param promptName 按 promptId 取标题（给配方起名用）。
     * @param findPromptIdByRunKey 按 `run_key` 反查 promptId（`CaptureRepo.findRun(...)?.promptId`）：
     *   学一份**已经读进过库**的文件时，配方就能顺手带上 `promptId`。
     */
    data class Deps(
        val objectInfo: () -> JsonObject?,
        val apiGraphOf: (Long) -> JsonObject?,
        val promptName: (Long) -> String?,
        val findPromptIdByRunKey: (String) -> Long?,
    )

    /** 学习结果（还没落库）。 */
    data class Learned(
        val recipe: WorkflowRecipe.Recipe,
        val name: String,
        val sourceKind: String,
        val sourceRef: String?,
        val sourcePath: String?,
        val promptId: Long?,
        /** false = 这张图转换时有缺口（见文件头那条纪律） */
        val complete: Boolean,
        val gaps: List<String>,
        /** 格式：`ui` / `api` */
        val format: String,
        /** 归纳本身的警告 + 转换过程中的警告 */
        val warnings: List<String>,
    ) {
        fun toJson(id: Long? = null): JsonObject = buildJsonObject {
            id?.let { put("recipeId", it) }
            put("name", name)
            put("task", recipe.task.wire)
            put("skeletonHash", recipe.skeletonHash)
            put("complete", complete)
            put("format", format)
            put("sourceKind", sourceKind)
            sourceRef?.let { put("sourceRef", it) }
            sourcePath?.let { put("sourcePath", it) }
            promptId?.let { put("promptId", it) }
            put("nodeCount", recipe.nodeCount)
            put("slotCount", recipe.slots.size)
            put("knobCount", recipe.knobs.size)
            put("gaps", buildJsonArray { gaps.forEach { add(JsonPrimitive(it)) } })
            put("render", recipe.render(name))
            put("recipe", recipe.toJson())
        }
    }

    // -----------------------------------------------------------------------
    //  三种来源
    // -----------------------------------------------------------------------

    /** 从本机工作流文件学（界面格式要 `/object_info` 才转得动）。 */
    fun learnFile(
        path: Path,
        name: String? = null,
        tolerateUnsupported: Boolean = false,
        emulateFrontend: Boolean = true,
        deps: Deps,
    ): Learned {
        if (!Files.isRegularFile(path)) {
            throw AiException("NOT_FOUND", "不是文件：$path")
        }
        val size = runCatching { Files.size(path) }.getOrDefault(0L)
        if (size <= 0L) throw AiException("EMPTY_FILE", "文件是空的：$path")
        if (size > MAX_FILE_BYTES) {
            throw AiException("TOO_LARGE", "工作流文件太大（$size 字节 > $MAX_FILE_BYTES）：$path")
        }
        val bytes = Files.readAllBytes(path)
        val root = runCatching { AppJson.parseToJsonElement(String(bytes, Charsets.UTF_8)) as? JsonObject }
            .getOrNull()
            ?: throw AiException(
                "NOT_A_WORKFLOW",
                "这份文件不是 ComfyUI 工作流 JSON（解析失败）：$path。" +
                    "需要 ComfyUI 保存的工作流文件（含 nodes/links），或「导出（API）」得到的节点图。",
            )

        val apiFormat = HistoryEntry.isNodeGraph(root)
        val uiFormat = !apiFormat && WorkflowConvert.isUiWorkflow(root)
        if (!apiFormat && !uiFormat) {
            throw AiException(
                "NOT_A_WORKFLOW",
                "这份 JSON 既不是 API 格式节点图，也不是界面格式工作流（没有 nodes / links）：$path",
            )
        }

        val runKey = "file:" + sha256Hex(bytes)
        var gaps = emptyList<String>()
        var convertWarnings = emptyList<String>()
        // `/object_info` 只取一次：转换与"现算自定义加载器"用的是同一份
        // （两次 HTTP 取到的可能是两个版本，而"槽位少了几个"正是这种不一致的后果）
        val info = deps.objectInfo()
        val graph: JsonObject = if (apiFormat) {
            root
        } else {
            val info2 = info ?: throw AiException(
                "COMFY_UNREACHABLE",
                "这是界面格式工作流，归纳它需要问 ComfyUI 要节点定义（/object_info），但连不上。" +
                    "先 `comfy_get_status` 看 ComfyUI 在不在跑；或者在 ComfyUI 里用「工作流菜单 → 导出（API）」" +
                    "存一份 API 格式的文件，再把那份路径给我。",
            )
            val converted = runCatching { WorkflowConvert.toApiGraph(root, info2, emulateFrontend) }
                .getOrElse { throw AiException("INVALID_ARGUMENT", "转换工作流失败：${it.message?.take(400)}") }
            gaps = (converted.unsupportedNodes.map { "摘掉节点类：$it" } +
                converted.unresolvedInputs.map { "悬空输入：$it" } +
                converted.openInputs.map { "空着的连线输入：$it" }).distinct().take(MAX_GAPS)
            convertWarnings = converted.warnings.take(MAX_CONVERT_WARNINGS)
            if (gaps.isNotEmpty() && !tolerateUnsupported) {
                throw AiException(
                    "UNSUPPORTED_NODES",
                    "这份工作流转成可提交的节点图时**有缺口**，直接学下来会得到一条缺了链的骨架（而不会报错），" +
                        "所以先停下：\n" + gaps.joinToString("\n") { "  · $it" } + "\n" +
                        "先确认**本机这个实例**有没有这些节点类（`GET /object_info/<类名>`）：" +
                        "如果它们是你自己装过的自定义节点（Krea2 / rgthree 这类），" +
                        "那多半是**这个实例没装 / 没启用那个节点包**，或者实例的模型根没挂全 —— " +
                        "**不是工作流坏了**，也不是让你去改它。\n" +
                        "四个出口：① 让用户在 ComfyUI 里启停一次 / 换用装了那些节点的实例，再学一次；" +
                        "② 在 ComfyUI 里「导出（API）」，把那份 API 格式文件给我（零转换）；" +
                        "③ 确实只想学个大概：带 `tolerateUnsupported=true` 再来一次 —— " +
                        "我会照样学，但配方上标 `complete=false` 与这些缺口，**不许**拿它当完整骨架用；" +
                        "④ 换一条不用那些节点的工作流。",
                )
            }
            if (converted.graph.isEmpty()) {
                throw AiException("NO_API_GRAPH", "转换之后一个可提交的节点都没有：$path")
            }
            converted.graph
        }

        val name2 = name?.takeIf { it.isNotBlank() }
            ?: path.fileName.toString().substringBeforeLast('.').take(WorkflowRecipeRepo.MAX_NAME_CHARS)
        val learned = analyze(graph, name2, "file", runKey, path.toString(), info)
        return learned.copy(
            promptId = deps.findPromptIdByRunKey(runKey),
            complete = gaps.isEmpty(),
            gaps = gaps,
            format = if (uiFormat) "ui" else "api",
            warnings = (learned.warnings + convertWarnings).distinct(),
        )
    }

    /** 从库里已有的提示词学（那份 API 图在 `capture_runs.raw` 里）。 */
    fun learnPrompt(promptId: Long, name: String? = null, deps: Deps): Learned {
        val graph = deps.apiGraphOf(promptId) ?: throw AiException(
            "NO_API_GRAPH",
            "库里 id=$promptId 这条提示词没有 API 格式节点图（老数据 / 只导入了图片），学不了。" +
                "可以换一条（`comfy_find_workflow` 里 runnable=true 的），或者把它的工作流文件路径给我。",
        )
        val name2 = name?.takeIf { it.isNotBlank() }
            ?: deps.promptName(promptId)?.takeIf { it.isNotBlank() }
            ?: "prompt-$promptId"
        return analyze(graph, name2.take(WorkflowRecipeRepo.MAX_NAME_CHARS), "prompt", "prompt:$promptId", null, deps.objectInfo())
            .copy(promptId = promptId, format = "api")
    }

    /** 直接给一张 API 节点图（模型从别处复制来的）。 */
    fun learnGraph(graph: JsonObject, name: String? = null, deps: Deps): Learned {
        if (!HistoryEntry.isNodeGraph(graph)) {
            throw AiException(
                "INVALID_ARGUMENT",
                "graph 不是一份 API 格式节点图（应当形如 " +
                    """{"1":{"class_type":"KSampler","inputs":{…}}}）。界面格式（nodes/links）请给文件路径。""",
            )
        }
        val compact = runCatching { AppJson.encodeToString(JsonObject.serializer(), graph) }.getOrDefault("")
        val ref = "graph:" + sha256Hex(compact.toByteArray(Charsets.UTF_8))
        val name2 = (name?.takeIf { it.isNotBlank() } ?: "节点图").take(WorkflowRecipeRepo.MAX_NAME_CHARS)
        return analyze(graph, name2, "graph", ref, null, deps.objectInfo()).copy(format = "api")
    }

    // -----------------------------------------------------------------------
    //  落库
    // -----------------------------------------------------------------------

    /**
     * 学完顺手落库（幂等：同一来源只有一行）。返回 `(id, created)`。
     *
     * 走 `Db.tx`（`BEGIN IMMEDIATE`，红线 R2）—— 与其它仓储一致。
     */
    fun save(learned: Learned, notes: String? = null): Pair<Long, Boolean> = Db.tx { conn ->
        WorkflowRecipeRepo.save(
            conn,
            WorkflowRecipeRepo.writeOf(
                recipe = learned.recipe,
                name = learned.name,
                sourceKind = learned.sourceKind,
                sourceRef = learned.sourceRef,
                sourcePath = learned.sourcePath,
                promptId = learned.promptId,
                complete = learned.complete,
                gaps = learned.gaps,
                extraWarnings = learned.warnings.filter { it !in learned.recipe.warnings },
                notes = notes,
            ),
        )
    }

    /** 学 + 存（工具与界面端点都调这一个）。 */
    fun learnAndSave(learned: Learned, notes: String? = null): Pair<Long, Learned> {
        val (id, _) = save(learned, notes)
        return id to learned
    }

    // -----------------------------------------------------------------------
    //  内部
    // -----------------------------------------------------------------------

    private fun analyze(
        graph: JsonObject,
        name: String,
        sourceKind: String,
        sourceRef: String?,
        sourcePath: String?,
        info: JsonObject?,
    ): Learned {
        val recipe = WorkflowRecipe.analyze(graph, ComfyModels.enumerateModelInputs(info).ifEmpty { null })
        return Learned(
            recipe = recipe,
            name = name,
            sourceKind = sourceKind,
            sourceRef = sourceRef,
            sourcePath = sourcePath,
            promptId = null,
            complete = true,
            gaps = emptyList(),
            format = "api",
            warnings = recipe.warnings,
        )
    }

    private fun sha256Hex(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
}
