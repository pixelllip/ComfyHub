package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * "这份工作流要的模型，本机有没有、最接近的是哪个、要改哪几个键"
 * —— 用户建议 ① 的产出（`comfy_fit_workflow`）。
 *
 * ## 它只报不改
 *
 * 这是用户建议 ① 的原话（"**只报不改**，改由 `comfy_submit` 的 overrides 做"）。
 * 结构上就这么保证：这个对象**不生成图、也不写任何文件**，它吐出来的是一份报告 +
 * 一份可以直接贴进 `comfy_submit(overrides=…)` 的**覆盖建议**。
 * 模型要真改，必须把那几个键明明白白写进调用参数（还会过审批闸门）——
 * "AI 悄悄把用户的模型换掉了"这件事因此不可能发生。
 *
 * ## 三种结论，一种都不许含糊
 *
 *  - [FitStatus.EXACT]：本机有一模一样的（归一化后同名）。**不用改**，也就不用问用户。
 *  - [FitStatus.SUBSTITUTE]：本机有更接近的（分数 ≥ [ComfyModels.MIN_SCORE]）。
 *    报出候选 + 分数 + 体积，**要不要用是模型 / 用户的事**。
 *  - [FitStatus.MISSING]：本机没有像样的替代品。这时**不许自己编一个名字**：
 *    报告里给的是"要装什么 / 大概多大"的入口（见 [missHint]），让模型去
 *    `civitai_search` 查、或者用 `ask_user_question` 把选择权交给用户。
 *
 * ## 为什么分数要跟着一起报
 *
 * 2026-09-21 的现场（`b5233fb4` seq6）里，模型是**手读 safetensors 头部**判断
 * "那个 13.49 GB 的 checkpoint 是裸 DiT、不含 CLIP/VAE"的。那种判断今天仍然要它自己做，
 * 所以报告里给的是**事实**（名字像到什么程度、多大、在哪个目录），不是结论。
 * 唯一由代码给出的结论是 [FitStatus.EXACT]（同名才叫"就是它"）。
 */
data class FitReport(
    val inventorySource: ComfyModels.Source,
    val modelsDir: String?,
    val localCount: Int,
    val truncated: Boolean,
    val missingNodes: List<String>,
    val items: List<FitItem>,
    /**
     * 本机清单里"**盘上有、但当前跑的实例枚举不到**"的条数（2026-09-22 事故的诊断口）。
     *
     * \> 0 就说明这个实例的模型根没挂全（`--extra-model-paths-config` 没生效 /
     * 实例不是从 ComfyUI Desktop 那套配置起的）——报告里会明说这一句。
     */
    val inventoryDiskOnly: Int = 0,
) {
    /** 有没有"必须动手"的项（缺模型，或者要改键）。 */
    val needsAction: Boolean get() = items.any { it.status != FitStatus.EXACT }

    /** 一条"一模一样、不用改"的都没有（= 一份工作流都跑不起来）。 */
    val allMissing: Boolean get() = items.isNotEmpty() && items.all { it.status == FitStatus.MISSING }

    fun toJson(): JsonObject = buildJsonObject {
        put("inventorySource", inventorySource.wire)
        modelsDir?.let { put("modelsDir", it) }
        put("localModels", localCount)
        if (truncated) put("inventoryTruncated", true)
        put("required", items.size)
        put("exact", items.count { it.status == FitStatus.EXACT })
        put("substitute", items.count { it.status == FitStatus.SUBSTITUTE })
        // 盘上有、但这个实例枚举不到（2026-09-22）：单列一档，别混进 missing ——
        // 混进去的后果是模型让用户去下载一个本来就在硬盘上的文件
        put("onDiskOnly", items.count { it.status == FitStatus.ON_DISK_ONLY })
        put("missing", items.count { it.status == FitStatus.MISSING })
        if (inventoryDiskOnly > 0) put("invisibleToInstance", inventoryDiskOnly)
        if (missingNodes.isNotEmpty()) {
            put("missingNodes", buildJsonArray { missingNodes.forEach { add(JsonPrimitive(it)) } })
        }
        put(
            "models",
            buildJsonArray {
                items.forEach { item ->
                    add(
                        buildJsonObject {
                            put("nodeId", item.requirement.nodeId)
                            put("classType", item.requirement.classType)
                            put("inputName", item.requirement.inputName)
                            put("wanted", item.requirement.fileName)
                            put("folder", item.requirement.folder)
                            item.requirement.title?.let { put("title", it) }
                            put("overrideKey", item.requirement.overrideKey)
                            put("status", item.status.wire)
                            put("safeToAutoApply", item.safeToAutoApply)
                            if (item.candidates.isNotEmpty()) {
                                put(
                                    "candidates",
                                    buildJsonArray {
                                        item.candidates.forEach { c ->
                                            add(
                                                buildJsonObject {
                                                    put("name", c.model.name)
                                                    put("folder", c.model.folder)
                                                    put("score", kotlin.math.round(c.score * 100) / 100.0)
                                                    put("sameFolder", c.sameFolder)
                                                    // 这个候选**当前实例能不能加载**：false 的不能拿去 overrides
                                                    // （ComfyUI 只会回一句文件不存在）
                                                    put("visibleToInstance", c.model.visibleToInstance)
                                                    c.model.path?.let { put("path", it) }
                                                    c.model.sizeGb?.let {
                                                        put("sizeGB", kotlin.math.round(it * 100) / 100.0)
                                                    }
                                                },
                                            )
                                        }
                                    },
                                )
                            }
                            item.suggestedOverride?.let { put("suggestedOverride", it) }
                            // 缺模型时给"这个缺口该怎么补"的可执行事实（用户建议 ②）：
                            // 查什么词、装到哪个目录、以及**本机有多少看起来沾边的替代品**
                            if (item.status == FitStatus.MISSING) {
                                put(
                                    "installProposal",
                                    buildJsonObject {
                                        put(
                                            "lookupQuery",
                                            ModelName.lookupQuery(item.requirement.fileName),
                                        )
                                        put("installInto", item.requirement.folder)
                                        put(
                                            "note",
                                            "先去 civitai_search(query=<lookupQuery>) 拿**体积与下载链接**，" +
                                                "再把\"装它 / 换工作流 / 放弃\"作为选项问用户。" +
                                                "本机那种\"沾一点边但不够像\"的近似文件：" +
                                                (item.nearMisses.ifEmpty { listOf("没有") }).joinToString("、"),
                                        )
                                    },
                                )
                            }
                            put("hint", item.hint())
                        },
                    )
                }
            },
        )
        // 直接可用的覆盖建议（**只报**：模型要把它贴进 comfy_submit 的 overrides 才会生效）
        val suggested = suggestedOverrides()
        if (suggested.isNotEmpty()) {
            put(
                "suggestedOverrides",
                buildJsonObject { suggested.forEach { (k, v) -> put(k, v) } },
            )
        }
        put("hint", renderHint())
    }

    /**
     * 可以**直接**贴进 `comfy_submit(overrides=…)` 的那几个键。
     *
     * 只收 [FitStatus.EXACT]（同名，换上去一定对）—— 替代品**不进这里**：
     * 那是"看着像"，该由模型判断 + 用户点头，不该由代码替它们决定（见 [FitItem.status] 的注释）。
     */
    fun suggestedOverrides(): Map<String, String> = items
        .filter { it.status == FitStatus.EXACT }
        .mapNotNull { item ->
            // 同名时**不需要覆盖**（图里那个名字本机就有）→ 只有归一化后同名但字面不同才要改
            val local = item.candidates.firstOrNull()?.model?.name ?: return@mapNotNull null
            if (local == item.requirement.fileName) null else item.requirement.overrideKey to local
        }
        .toMap()

    /** 给模型看的一小段：**结论先行**，然后是逐条。 */
    fun render(): String = buildString {
        val required = items.size
        if (required == 0) {
            append("这份工作流**没有点名任何模型文件**（连 checkpoint / LoRA / VAE 加载器都没有），")
            append("所以没有模型要对。")
            appendMissingNodes(this)
            return@buildString
        }
        val exact = items.count { it.status == FitStatus.EXACT }
        val sub = items.count { it.status == FitStatus.SUBSTITUTE }
        val diskOnly = items.count { it.status == FitStatus.ON_DISK_ONLY }
        val miss = items.count { it.status == FitStatus.MISSING }
        append("这份工作流要 ").append(required).append(" 个模型：")
        append("本机有同名的 ").append(exact).append(" 个");
        if (sub > 0) append("、有可替代的 ").append(sub).append(" 个")
        if (diskOnly > 0) {
            append("、**硬盘上有同名但当前实例看不到的 ").append(diskOnly).append(" 个**")
        }
        if (miss > 0) append("、**本机没有的 ").append(miss).append(" 个**")
        append("。\n")
        append("（本机清单来自 ").append(if (inventorySource == ComfyModels.Source.OBJECT_INFO) {
            "ComfyUI 的 /object_info，共 $localCount 条 —— 列出来的就是它能加载的"
        } else {
            "目录扫描，共 $localCount 条" + (modelsDir?.let { "，扫的是 $it" } ?: "")
        })
        append("）\n")
        if (inventoryDiskOnly > 0) {
            append("⚠ 本机清单里有 ").append(inventoryDiskOnly)
            append(" 个文件**只出现在硬盘上、当前跑的 ComfyUI 实例枚举不到**——")
            append("说明这个实例的模型根没挂全（`--extra-model-paths-config` 没生效）。\n")
            append("  这类文件**不要拿去 overrides**（提交只会报文件不存在），也**不要下载**（它就在硬盘上）：")
            append("先让用户重启 ComfyUI（ComfyHub 起实例时会自动挂上 ComfyUI Desktop 的模型根，见 `comfy_env_report`）再对一次。\n")
        }

        items.forEach { item ->
            append("\n· ").append(item.requirement.overrideKey)
            append("（").append(item.requirement.classType)
            item.requirement.title?.let { append(" / ").append(it) }
            append("，" ).append(item.requirement.folder).append("）要：")
            append(item.requirement.fileName).append('\n')
            append("  ").append(
                when (item.status) {
                    FitStatus.EXACT -> {
                        val local = item.candidates.first().model.name
                        if (local == item.requirement.fileName) "✅ 本机有同名文件，**不用改**。"
                        else "✅ 本机有同名文件（写法不同：$local），覆盖成它即可。"
                    }
                    FitStatus.SUBSTITUTE ->
                        "⚠️ 本机没有同名的。最接近的：" +
                            item.candidates.joinToString("；") { c ->
                                "${c.model.name}（像 ${(c.score * 100).toInt()}%" +
                                    (c.model.sizeGb?.let { "，${"%.1f".format(it)}GB" } ?: "") +
                                    (if (c.sameFolder) "" else "，在 ${c.model.folder}/ 里") +
                                    (if (c.model.visibleToInstance) "" else "，**当前实例看不到它**") + "）"
                            } +
                            " —— 像不像、能不能顶要你自己判断（分数只是字面相似度，" +
                            "**不保证架构兼容**）；拿不准就问用户。" +
                            if (item.candidates.none { it.model.visibleToInstance }) {
                                "\n  ⚠️ 这些候选**当前实例都枚举不到**（在硬盘上但没挂上）：先让用户重启 ComfyUI 再提交，" +
                                    "别拿它们去 overrides（只会得到一句文件不存在）。"
                            } else {
                                ""
                            }
                    FitStatus.ON_DISK_ONLY -> {
                        val local = item.candidates.first().model
                        "🟡 **硬盘上有这个文件**（${local.name}" +
                            (local.sizeGb?.let { "，${"%.1f".format(it)}GB" } ?: "") +
                            (local.path?.let { "，在 $it" } ?: "") +
                            "），但**当前跑的 ComfyUI 实例枚举不到它**" +
                            "—— 说明这次启动没挂上那套模型根（共享目录 / 别的盘）。\n" +
                            "  **先修挂载再提交，不要下载**（文件本来就在硬盘上），也**不要**改 overrides " +
                            "（改成它也一样加载不了）：让用户重启 ComfyUI（ComfyHub 起实例时会自动挂上），" +
                            "或先 `comfy_env_report` 看三个根到底差在哪。"
                    }
                    FitStatus.MISSING ->
                        "❌ 本机没找到像样的替代品。**不要编一个文件名**去提交" +
                            "（ComfyUI 只会报文件不存在）。" +
                            "先去 `civitai_search` 查它" +
                            "（建议查询词：`" + ModelName.lookupQuery(item.requirement.fileName) + "`）" +
                            "—— 拿到**体积**（几 GB）与下载链接，" +
                            "再用 `ask_user_question` 让用户选：装它 / 换一条只用本机已有模型的工作流 / 放弃。" +
                            "提问的选项里**把体积写进去**（用户就是按这个数决定装不装的）。" +
                            if (item.nearMisses.isEmpty()) {
                                "（本机连名字沾边的都没有。）"
                            } else {
                                "（本机有几个**名字沾边但不是一个东西**的：" +
                                    item.nearMisses.joinToString("、") +
                                    " —— 别拿它们顶，报给用户时也可以提一句。）"
                            }
                },
            )
            append('\n')
        }

        val suggested = suggestedOverrides()
        if (suggested.isNotEmpty()) {
            append("\n要改的键（**同名但写法不同**，直接贴进 comfy_submit 的 overrides 即可）：")
            suggested.forEach { (k, v) -> append("\n  \"").append(k).append("\": \"").append(v).append('"') }
            append('\n')
        } else if (exact == required && required > 0) {
            append("\n**这份工作流的模型本机全都有，不用改任何键。**\n")
        }
        appendMissingNodes(this)
        append(
            "\n（这个工具**只报不改**：上面那些键要真生效，得由你写进 `comfy_submit(overrides=…)`；" +
                "替代品尤其要**先跟用户确认**再换 —— 那是他的模型。）\n",
        )
    }

    private fun renderHint(): String = when {
        items.isEmpty() ->
            "这份工作流没有点名模型文件" +
                (if (missingNodes.isEmpty()) "。" else "，但有 " + missingNodes.size +
                    " 个节点类本机 ComfyUI 里没有（见 missingNodes）——先用 comfy_check_inputs 看缺口。")
        items.any { it.status == FitStatus.ON_DISK_ONLY } ->
            "要的模型**硬盘上就有**，只是当前这个 ComfyUI 实例枚举不到它们（模型根没挂全）。" +
                "**不要去下载**，也别改 overrides：先让用户重启 ComfyUI" +
                "（ComfyHub 起实例时会自动挂上 ComfyUI Desktop 的模型根），或用 comfy_env_report 看根目录差在哪。"
        allMissing ->
            "本机没有这份工作流要的模型。**不要硬提交**：先去 `civitai_search` 查这些模型" +
                "（拿体积与下载链接），再用 `ask_user_question` 给用户 2~4 个具体选项" +
                "（自己装 / 换用本机已有模型的等价工作流 / 放弃），让他选。"
        needsAction ->
            "有可替代的模型：要不要换由你判断 + 用户点头（overrides 写进 comfy_submit）。" +
                "同名的那几个不用动。"
        else -> "模型全都有同名的，可以直接提交。"
    }

    private fun appendMissingNodes(sb: StringBuilder) {
        if (missingNodes.isEmpty()) return
        sb.append("\n本机 ComfyUI 里**没有这些节点类**（先 comfy_get_status 确认它在跑；" +
            "确实是缺节点时，缺的节点要另装，这个工具不负责装）：")
        sb.append(missingNodes.joinToString("、")).append('\n')
    }
}

/** 图里点名的一个模型 vs 本机清单的配对结果。 */
data class FitItem(
    val requirement: ComfyModels.Requirement,
    val status: FitStatus,
    val candidates: List<ComfyModels.Scored>,
    /**
     * 本机那几个"名字沾点边、但不够像"的文件（分数低于阈值的）。
     *
     * 只在 [FitStatus.MISSING] 时说给用户听：他看到"要装它"时，盘上那个像的名字
     * 很容易让他以为已经有了（用户建议 ② 要的"本机已有替代品"这一栏）。
     */
    val nearMisses: List<String> = emptyList(),
) {
    /** 唯一可以"不问用户就直接换"的情形：同名（归一化后一致）。 */
    val safeToAutoApply: Boolean get() = status == FitStatus.EXACT

    /** 建议写进 overrides 的值（同名但字面不同时才有；完全一样时为 null）。 */
    val suggestedOverride: String?
        get() = if (status == FitStatus.EXACT) {
            candidates.firstOrNull()?.model?.name?.takeIf { it != requirement.fileName }
        } else {
            null
        }

    fun hint(): String = when (status) {
        FitStatus.EXACT -> "本机有同名文件，不用改。"
        FitStatus.SUBSTITUTE ->
            "本机没有同名的；候选见 candidates。换之前**先问用户**（那是他的模型），" +
                "确认后写 comfy_submit(overrides={\"${requirement.overrideKey}\":\"<候选名>\"})。"
        FitStatus.ON_DISK_ONLY ->
            "盘上有同名的（见 candidates[0].path），但**当前实例加载不了**：先让用户重启 ComfyUI / 修好模型根，" +
                "**不要下载**（文件就在硬盘上），也不要改 overrides（改了同样报文件不存在）。"
        FitStatus.MISSING ->
            "本机没有。先去 civitai_search 查它，再用 ask_user_question 让用户决定装 / 换工作流 / 放弃。"
    }
}

/** 配对结论（四档，见 [FitReport] 的注释）。 */
enum class FitStatus(val wire: String) {
    /** 本机有一模一样的（归一化后同名）**且当前实例能加载** —— 唯一能自动替换的一档。 */
    EXACT("exact"),

    /** 本机有更接近的（分数 ≥ 阈值）：候选给出来，用不用由模型 / 用户定。 */
    SUBSTITUTE("substitute"),

    /**
     * **硬盘上有同名的，但当前跑着的那个实例枚举不到它**（2026-09-22 新加，那次事故的直接产物）。
     *
     * 机制：`launch_comfyui` 直接跑 `main.py`，ComfyUI Desktop 那套"模型在共享目录 / 别的盘"
     * 的配置一个都没传 → `/object_info` 里没有这些文件 → 只报"本机没有"会让模型
     * **让用户去下载一个本来就在硬盘上的文件**（现场：`anima-base-v1.0` 4.18 GB）。
     * 这一档的正确动作是**修挂载 / 重启**，不是下载。
     */
    ON_DISK_ONLY("onDiskOnly"),

    /** 本机没有像样的替代品：该去装 / 该让用户选，**不许编名字**。 */
    MISSING("missing"),
}

/**
 * 把"图里要的模型"与"本机清单"配起来（`comfy_fit_workflow` 的全部逻辑，纯函数、可单测）。
 *
 * @param graph API 格式节点图
 * @param inventory [ComfyModels.inventory] 的结果
 * @param knownNodes `/object_info` 的 **节点类名集合**，用来报"本机没有这些节点"
 *   （而"没点名模型"与"缺节点"是两件事，报告里分开说）；null = 不检查。
 * @param modelInputs `/object_info` 现算的模型枚举输入（`ComfyModels.enumerateModelInputs`），
 *   用来认出自定义加载器；null = 只用内置那张表。
 */
object WorkflowFit {

    fun fit(
        graph: JsonObject,
        inventory: ComfyModels.Inventory,
        knownNodes: Set<String>? = null,
        modelInputs: Map<String, Set<String>>? = null,
    ): FitReport {
        val requirements = ComfyModels.requirements(graph, modelInputs)
        val items = requirements.map { req ->
            val candidates = inventory.candidatesFor(req.folder, req.fileName)
            // 同名要分两种：**实例能加载**的才算"不用改"；只在盘上的要走 ON_DISK_ONLY
            // （不然就是"AI 说模型齐了、提交却报文件不存在"的反向版本）
            val exactVisible = candidates.firstOrNull {
                it.model.visibleToInstance && ModelName.same(req.fileName, it.model.name)
            }
            val exactAny = candidates.firstOrNull { ModelName.same(req.fileName, it.model.name) }
            when {
                // 同名优先：就算还有别的更高分的候选，"一模一样"永远是最好的答案
                exactVisible != null ->
                    FitItem(req, FitStatus.EXACT, listOf(exactVisible) + candidates.filter { it !== exactVisible })
                exactAny != null ->
                    FitItem(req, FitStatus.ON_DISK_ONLY, listOf(exactAny) + candidates.filter { it !== exactAny })
                candidates.isNotEmpty() -> FitItem(req, FitStatus.SUBSTITUTE, candidates)
                else -> FitItem(
                    req,
                    FitStatus.MISSING,
                    emptyList(),
                    // 缺模型时把"沾边但不够像"的也报出来：用户盘上那个像的名字最容易
                    // 让他以为"我明明有这个模型"（用户建议 ② 的"本机已有替代品"那一栏）
                    nearMisses = inventory.nearMissesFor(req.folder, req.fileName).map { it.name },
                )

            }
        }
        val missingNodes = if (knownNodes == null) {
            emptyList()
        } else {
            graph.values.mapNotNull { raw ->
                val cls = ((raw as? JsonObject)?.get("class_type") as? JsonPrimitive)?.contentOrNull
                cls?.takeIf { it.isNotBlank() && it !in knownNodes }
            }.distinct()
        }
        return FitReport(
            inventorySource = inventory.source,
            modelsDir = inventory.modelsDir?.toString(),
            localCount = inventory.models.size,
            truncated = inventory.truncated,
            missingNodes = missingNodes,
            items = items,
            inventoryDiskOnly = inventory.diskOnly.size,
        )
    }
}
