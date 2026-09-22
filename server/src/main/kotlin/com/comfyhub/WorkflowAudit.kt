package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * **本机工作流审核**（用户 2026-09-22：「让 AI 具有审核本地工作流的能力」）。
 *
 * 起因是一句追问：会话 `0bedb3f5` 里用户问"你有没有发现你用的 krea2 工作流本身有问题呢？"，
 * AI 承认"我上一轮确实没审这条骨架就用了"。那次踩到的东西**全是可以在跑之前查出来的**：
 *
 *  | 那次的现象 | 代价 | 对应检查项 |
 *  | --- | --- | --- |
 *  | `Krea2_spine` 固定 seed + 固定前缀 → `mediaCount: 0` | 白跑 78 秒 | [CHECK_OUTPUT_NAME] |
 *  | 20 个 rgthree 前端节点（Label / Bookmark / Fast Groups Bypasser） | 转换失败 | [CHECK_FRONTEND] |
 *  | `WidgetToString.IS_CHANGED()` / `SystemNotification` 版本不兼容 | 0.01 秒空跑 | [CHECK_NODE_VERSION] |
 *  | 用 `CheckpointLoaderSimple` 从 int8 一体包"只取 MODEL" | 与官方模板不符 | [CHECK_LOADER_FOLDER] |
 *  | 8 步 / cfg 1 被任意改 | 质量崩 | [CHECK_PARAMS] |
 *  | 1360×1024 + 4K 放大 | 8GB 显存吃紧 | [CHECK_VRAM] |
 *  | 给 Krea 2 写 Danbooru tag、给 Anima 写自然语言 | 出图跑偏 | [CHECK_PROMPT_STYLE] |
 *
 * ## 三段之间的分工（别混）
 *
 *  - **确定性检查（这里）**：只报**事实**（哪个节点、哪个值、和什么冲突），可单测、可复现；
 *  - **`skill`**（`skills/builtin/comfy-workflow-audit`）：写"所以该怎么办"的判定与修法；
 *  - **提交闸门**（[ModelPreflight] + [blockingRefusal]）：只拦**必跑不了**的那些
 *    （前端节点没摘干净 / 已知版本不兼容节点 / 本机没有的节点类），
 *    参数档位、显存、提示词风格**只提醒不拦** —— 那些是"不推荐"，不是"跑不了"。
 *
 * ## 一条纪律
 *
 * **拿不到的信息不许猜**：没有 `/object_info` 就不判"本机没有这个节点类"；
 * 不认识的模型就不判参数档位（宁可不说，也不要给一个会让人改错方向的结论）。
 */
object WorkflowAudit {

    const val CHECK_FRONTEND = "frontendNodes"
    const val CHECK_NODE_VERSION = "nodeVersions"
    const val CHECK_UNKNOWN_NODE = "unknownNodes"
    const val CHECK_OUTPUT_NAME = "outputName"
    const val CHECK_PARAMS = "params"
    const val CHECK_VRAM = "vram"
    const val CHECK_PROMPT_STYLE = "promptStyle"
    const val CHECK_LOADER_FOLDER = "loaderFolder"

    /** 结论的严重程度：只有 [BLOCK] 会被提交闸门拦下。 */
    enum class Level(val wire: String) {
        /** 必跑不了（ComfyUI 验证阶段就会整片丢节点 / 报文件不存在）。 */
        BLOCK("block"),

        /** 跑得起来，但大概率不是你要的结果（白跑一次显卡时间）。 */
        WARN("warn"),

        /** 只是背景信息。 */
        INFO("info"),
    }

    /** 一条检查结论。 */
    data class Finding(
        val level: Level,
        val check: String,
        val title: String,
        val detail: String,
        /** 该怎么办（人话，一句话）。 */
        val fix: String? = null,
        val nodes: List<String> = emptyList(),
    )

    data class Report(
        val nodeCount: Int,
        val findings: List<Finding>,
    ) {
        val blocking: List<Finding> get() = findings.filter { it.level == Level.BLOCK }

        /**
         * 提交闸门要的那句话：有"必跑不了"的项就返回一段说明，否则 null。
         *
         * 只收 [Level.BLOCK] —— 参数档位 / 显存 / 提示词风格那些**不许**拦
         * （它们是"不推荐"，不是"跑不了"；拦了就是替用户做决定）。
         */
        fun blockingRefusal(): String? {
            if (blocking.isEmpty()) return null
            return buildString {
                append("**没有提交**：审核发现 ").append(blocking.size)
                append(" 处\"跑不起来\"的问题（ComfyUI 验证阶段就会整片丢节点或直接报错）：\n")
                blocking.forEach { f ->
                    append("\n· [").append(f.check).append("] ").append(f.title).append('\n')
                    append("  ").append(f.detail).append('\n')
                    f.fix?.let { append("  → ").append(it).append('\n') }
                }
            }
        }

        fun toJson(): JsonObject = buildJsonObject {
            put("nodeCount", nodeCount)
            put("block", blocking.size)
            put("warn", findings.count { it.level == Level.WARN })
            put("info", findings.count { it.level == Level.INFO })
            put(
                "findings",
                buildJsonArray {
                    findings.forEach { f ->
                        add(
                            buildJsonObject {
                                put("level", f.level.wire)
                                put("check", f.check)
                                put("title", f.title)
                                put("detail", f.detail)
                                f.fix?.let { put("fix", it) }
                                if (f.nodes.isNotEmpty()) {
                                    put("nodes", buildJsonArray { f.nodes.forEach { add(JsonPrimitive(it)) } })
                                }
                            },
                        )
                    }
                },
            )
        }

        /** 给模型看的正文：结论先行，然后逐条。 */
        fun render(): String {
            if (findings.isEmpty()) {
                return "审核通过：${nodeCount} 个节点，七项检查都没发现问题（前端节点 / 节点版本 / " +
                    "输出文件名 / 参数档位 / 显存 / 提示词风格 / 加载器目录）。\n" +
                    "（审核只看**图的结构**：模型在不在本机请用 comfy_fit_workflow；这份图的输入齐不齐" +
                    "用 comfy_check_inputs。）\n"
            }
            return buildString {
                append("审核结论（").append(nodeCount).append(" 个节点）：")
                append("**跑不起来 ").append(blocking.size).append(" 处**、")
                append("不建议 ").append(findings.count { it.level == Level.WARN }).append(" 处")
                if (findings.any { it.level == Level.INFO }) {
                    append("、另有 ").append(findings.count { it.level == Level.INFO }).append(" 条背景")
                }
                append("。\n")
                findings.forEach { f ->
                    val mark = when (f.level) {
                        Level.BLOCK -> "🔴"
                        Level.WARN -> "🟠"
                        Level.INFO -> "🔵"
                    }
                    append("\n").append(mark).append(" [").append(f.check).append("] ")
                        .append(f.title).append('\n')
                    append("  ").append(f.detail).append('\n')
                    if (f.nodes.isNotEmpty()) {
                        append("  涉及节点：").append(f.nodes.joinToString("、")).append('\n')
                    }
                    f.fix?.let { append("  → ").append(it).append('\n') }
                }
                append("\n（审核**只报不改**：要摘节点用 `comfy_prune_workflow`，要改参数用 " +
                    "`comfy_submit(overrides=…)`；我不动你的工作流。）\n")
            }
        }
    }

    // -----------------------------------------------------------------------
    //  知识表（都是有出处的；拿不准的一律不写进来）
    // -----------------------------------------------------------------------

    /**
     * **纯前端**节点：ComfyUI 服务端根本没有这些类，图里带着它们 = 验证阶段整片失败。
     *
     * rgthree 那一族来自 2026-09-22 的现场（`krea2SFWNSFWUncensoredImageTo_v10.json`：
     * 10 Label + Fast Groups Bypasser + 8 Bookmark + …共 20 个）。
     * `Anything Everywhere` 与显示节点由 [FrontendNodes] 自己认得（模式匹配），这里不重复列。
     */
    private val KNOWN_FRONTEND_ONLY = setOf(
        "Label (rgthree)", "Bookmark (rgthree)", "Fast Groups Bypasser (rgthree)",
        "Fast Groups Muter (rgthree)", "Image Comparer (rgthree)", "Context (rgthree)",
        "Context Big (rgthree)", "SetNode", "GetNode", "Reroute", "Note", "PrimitiveNode",
    )

    /**
     * **已知版本不兼容**的节点类（装了、但一跑就崩）。
     *
     * 2026-09-22 实测：这两族在 IS_CHANGED 上签名对不上，验证阶段整片节点被丢，
     * `Prompt executed in 0.01 seconds`、零产物。**可随节点包更新而失效** ——
     * 所以文案里写的是"先更新节点包；更新完还报就摘掉"，而不是"永远别用"。
     */
    private val KNOWN_INCOMPATIBLE = mapOf(
        "WidgetToString" to "IS_CHANGED() missing 1 required positional argument",
        "SystemNotification" to "IS_CHANGED() missing 1 required keyword-only argument",
        "SystemNotification|pysssss" to "IS_CHANGED() missing 1 required keyword-only argument",
    )

    /** 认得出档位的模型（判据是**文件名里的词**，不是猜）。 */
    private data class ModelProfile(
        val label: String,
        val steps: IntRange?,
        val cfg: ClosedFloatingPointRange<Double>?,
        val samplers: Set<String>,
        val note: String,
    )

    private val KREA2 = ModelProfile(
        label = "Krea 2（Turbo 档）",
        steps = 4..8,
        cfg = 0.0..1.5,
        samplers = setOf("euler", "er_sde", "dpmpp_2m", "dpmpp_2m_sde"),
        note = "官方 Turbo 部署参数是 8 步 / cfg 1 / mu 1.15（4-step 蒸馏 LoRA 时 4 步 / cfg 1）",
    )

    private val ANIMA = ModelProfile(
        label = "Anima",
        steps = 4..32,
        cfg = 0.0..4.5,
        samplers = setOf("er_sde", "euler", "euler_ancestral", "dpmpp_2m"),
        note = "官方推荐 er_sde / euler + cfg 1（合并底模常见 30 步 / cfg 4）",
    )

    /** 采样器节点（值里有 seed 与 steps / cfg 的那些）。 */
    private val SAMPLER_CLASSES = setOf(
        "KSampler", "KSamplerAdvanced", "SamplerCustom", "SamplerCustomAdvanced",
        "KSamplerSelect", "RandomNoise",
    )

    /** 保存节点（值里有 filename_prefix 的那些）。 */
    private val SAVE_PREFIX_KEYS = listOf("filename_prefix", "filename", "prefix")

    // -----------------------------------------------------------------------
    //  入口
    // -----------------------------------------------------------------------

    /**
     * 审一份 **API 格式**节点图。
     *
     * @param objectInfo ComfyUI 的 `/object_info`（**null = ComfyUI 没跑**）。
     *   为 null 时**不判**"本机没有这个节点类"——宁可少报，也不要凭"我没问到"下结论。
     * @param inventory 本机模型清单（[ComfyModels.inventory]），用来判"加载器与目录不匹配"。
     */
    fun audit(
        graph: JsonObject,
        objectInfo: JsonObject? = null,
        inventory: ComfyModels.Inventory? = null,
    ): Report {
        val nodes = graph.mapNotNull { (id, raw) ->
            val node = raw as? JsonObject ?: return@mapNotNull null
            val cls = node.str("class_type").orEmpty()
            if (cls.isBlank()) null else id to (cls to node)
        }.toMap()

        val findings = mutableListOf<Finding>()
        findings += checkFrontendNodes(nodes, objectInfo)
        findings += checkNodeVersions(nodes)
        findings += checkOutputName(graph)
        findings += checkParams(graph)
        findings += checkVram(graph)
        findings += checkPromptStyle(graph)
        findings += checkLoaderFolder(graph, inventory)
        return Report(nodeCount = nodes.size, findings = findings)
    }

    // -----------------------------------------------------------------------
    //  ① 前端外观节点
    // -----------------------------------------------------------------------

    private fun checkFrontendNodes(
        nodes: Map<String, Pair<String, JsonObject>>,
        objectInfo: JsonObject?,
    ): List<Finding> {
        val uiOnly = nodes.filter { (_, v) ->
            val cls = v.first
            cls in KNOWN_FRONTEND_ONLY || FrontendNodes.isBroadcast(cls) || FrontendNodes.isDisplay(cls)
        }
        val unknown = if (objectInfo == null) {
            emptyMap()
        } else {
            nodes.filter { (_, v) -> objectInfo[v.first] == null && v.first !in KNOWN_FRONTEND_ONLY }
        }

        val out = mutableListOf<Finding>()
        if (uiOnly.isNotEmpty()) {
            out += Finding(
                level = Level.BLOCK,
                check = CHECK_FRONTEND,
                title = "图里还有 ${uiOnly.size} 个纯前端节点（服务端根本没有这些类）",
                detail = "这些类只存在于 ComfyUI **前端**（rgthree 的 Label / Bookmark / " +
                    "Fast Groups Bypasser、Set/Get、Anything Everywhere、显示节点…）。" +
                    "服务端拿到它们会在**验证阶段整片丢掉节点**，表现是跑完 0.01 秒、零产物。" +
                    "这次现场（`krea2SFWNSFWUncensoredImageTo_v10.json`）就是 20 个这类节点。",
                fix = "让 `comfy_load_workflow` 读原文件（它会照前端行为等价改写），" +
                    "或用 `comfy_prune_workflow` 把这些节点摘掉并补上缺口。",
                nodes = uiOnly.keys.sortedBy { it.toIntOrNull() ?: Int.MAX_VALUE },
            )
        }
        if (unknown.isNotEmpty()) {
            out += Finding(
                level = Level.BLOCK,
                check = CHECK_UNKNOWN_NODE,
                title = "本机 ComfyUI 里没有这 ${unknown.size} 个节点类",
                detail = "`/object_info` 里查不到它们，所以提交一定会被验证拒绝：" +
                    unknown.entries.joinToString("、") { (id, v) -> "$id ${v.first}" } + "。",
                fix = "要么装对应的自定义节点包（ComfyUI-Manager），要么把用到它们的分支摘掉" +
                    "（`comfy_prune_workflow(analyze=true)` 先看清楚）。",
                nodes = unknown.keys.sortedBy { it.toIntOrNull() ?: Int.MAX_VALUE },
            )
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  ② 自定义节点版本不兼容
    // -----------------------------------------------------------------------

    private fun checkNodeVersions(nodes: Map<String, Pair<String, JsonObject>>): List<Finding> {
        val hits = nodes.filter { (_, v) -> v.first in KNOWN_INCOMPATIBLE }
        if (hits.isEmpty()) return emptyList()
        return listOf(
            Finding(
                level = Level.BLOCK,
                check = CHECK_NODE_VERSION,
                title = "有 ${hits.size} 个节点与当前节点包版本不兼容",
                detail = hits.entries.joinToString("；") { (id, v) ->
                    "$id ${v.first}（${KNOWN_INCOMPATIBLE[v.first]}）"
                } + "。这些类**装了但一跑就崩**：验证阶段整片节点被丢，`Prompt executed in 0.01 seconds`、零产物。",
                fix = "先在 ComfyUI-Manager 里**更新这几个节点包**再试；更新后还报就摘掉这条链，" +
                    "换等效节点（例如用 `easy showAnything` / `PreviewAny` 代替 `WidgetToString`）。",
                nodes = hits.keys.sortedBy { it.toIntOrNull() ?: Int.MAX_VALUE },
            ),
        )
    }

    // -----------------------------------------------------------------------
    //  ③ 输出文件名 + 固定 seed（那次白跑 78 秒的根因）
    // -----------------------------------------------------------------------

    private fun checkOutputName(graph: JsonObject): List<Finding> {
        val prefixes = mutableListOf<String>()
        graph.forEach { (id, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            SAVE_PREFIX_KEYS.forEach { key ->
                val v = (inputs[key] as? JsonPrimitive)?.contentOrNull
                if (!v.isNullOrBlank()) prefixes += "$id.$key = $v"
            }
        }
        val seeds = mutableListOf<String>()
        graph.forEach { (id, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val cls = node.str("class_type").orEmpty()
            if (cls !in SAMPLER_CLASSES) return@forEach
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            listOf("seed", "noise_seed").forEach { key ->
                val v = inputs[key]
                if (v is JsonPrimitive && v.isNumberLike()) seeds += "$id.$key = ${v.contentOrNull}"
            }
        }
        if (prefixes.isEmpty() || seeds.isEmpty()) return emptyList()
        return listOf(
            Finding(
                level = Level.WARN,
                check = CHECK_OUTPUT_NAME,
                title = "输出前缀与种子都写死了 —— 每次跑都会落同一个文件名",
                detail = "保存节点：" + prefixes.joinToString("、") + "；采样器：" + seeds.joinToString("、") +
                    "。ComfyUI 会覆盖同名文件，本项目的捕获按 `prompt_id` + 文件 SHA-256 判重，" +
                    "于是**跑完 mediaCount: 0、提示\"与已有文件相同，未重复入库\"** —— 等于白跑。" +
                    "（现场：`Krea2_spine` 沿用 seed 20260921 + 同前缀，78 秒后什么都没入库。）" +
                    "注意**抽卡是常态**：用户要连出几张挑一张时，靠的就是\"**换 seed、不换前缀**\"来攒同一批图。",
                fix = "看用户要什么：**抽卡**（\"再出几张\"\"这张不行\"）→ **保留 filename_prefix、只换 seed**" +
                    "（用户说\"随机\"就把 seed 换成随机值，否则给一个新数字）；" +
                    "**只跑一次定稿** → 换 seed 或换 prefix 都行。" +
                    "`overrides={\"<采样器>.seed\":<新种子>}`（或 `{\"<保存节点>.filename_prefix\":\"<新前缀>\"}`）。",
                nodes = (prefixes + seeds).map { it.substringBefore('.') }.distinct(),
            ),
        )
    }

    // -----------------------------------------------------------------------
    //  ④ 参数档位是否匹配该模型
    // -----------------------------------------------------------------------

    private fun checkParams(graph: JsonObject): List<Finding> {
        val modelName = graph.values.mapNotNull { raw ->
            val node = raw as? JsonObject ?: return@mapNotNull null
            val cls = node.str("class_type").orEmpty()
            if (!cls.contains("Loader", ignoreCase = true)) return@mapNotNull null
            val inputs = node["inputs"] as? JsonObject ?: return@mapNotNull null
            listOf("ckpt_name", "unet_name", "model_name", "lora_name")
                .firstNotNullOfOrNull { (inputs[it] as? JsonPrimitive)?.contentOrNull }
        }.firstOrNull { it.isNotBlank() } ?: return emptyList()

        val profile = when {
            modelName.contains("krea", true) -> KREA2
            modelName.contains("anima", true) -> ANIMA
            else -> return emptyList() // 不认识的模型**不判**（宁可不说，也不要给错方向）
        }

        val out = mutableListOf<Finding>()
        graph.forEach { (id, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            if (node.str("class_type").orEmpty() !in SAMPLER_CLASSES) return@forEach
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            val steps = (inputs["steps"] as? JsonPrimitive)?.intOrNull()
            val cfg = (inputs["cfg"] as? JsonPrimitive)?.doubleOrNull()
            val sampler = (inputs["sampler_name"] as? JsonPrimitive)?.contentOrNull
            val bad = mutableListOf<String>()
            if (steps != null && profile.steps != null && steps !in profile.steps) {
                bad += "steps = $steps（该模型这一档是 ${profile.steps.first}~${profile.steps.last}）"
            }
            if (cfg != null && profile.cfg != null && cfg !in profile.cfg) {
                bad += "cfg = $cfg（该模型这一档是 ${profile.cfg.start}~${profile.cfg.endInclusive}）"
            }
            if (sampler != null && profile.samplers.isNotEmpty() && sampler !in profile.samplers) {
                bad += "sampler = $sampler（常用 ${profile.samplers.joinToString("/")}）"
            }
            if (bad.isNotEmpty()) {
                out += Finding(
                    level = Level.WARN,
                    check = CHECK_PARAMS,
                    title = "参数与 $modelName（${profile.label}）的官方档位不符",
                    detail = "节点 $id：" + bad.joinToString("；") + "。${profile.note}。",
                    fix = "按官方档位改（`comfy_submit(overrides=…)`），或者确认你就是要偏离这个档位。",
                    nodes = listOf(id),
                )
            }
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  ⑤ 显存风险（只估、不拦）
    // -----------------------------------------------------------------------

    private fun checkVram(graph: JsonObject): List<Finding> {
        var maxPixels = 0L
        var hasUpscale = false
        var loras = 0
        graph.forEach { (_, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val cls = node.str("class_type").orEmpty()
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            if (cls.contains("EmptyLatentImage") || cls.contains("EmptySD3LatentImage")) {
                val w = (inputs["width"] as? JsonPrimitive)?.intOrNull() ?: 0
                val h = (inputs["height"] as? JsonPrimitive)?.intOrNull() ?: 0
                if (w.toLong() * h > maxPixels) maxPixels = w.toLong() * h
            }
            if (cls.contains("Upscale", true) || cls.contains("ImageScale", true)) hasUpscale = true
            if (cls.contains("Lora", true)) loras++
        }
        val risks = mutableListOf<String>()
        if (maxPixels >= 2048L * 2048L) risks += "出图尺寸 ${maxPixels / 1_000_000}MP（≥4MP）"
        if (hasUpscale) risks += "带放大/重采样节点（放大那一步显存峰值最高）"
        if (loras >= 3) risks += "挂了 $loras 个 LoRA（每个都要额外显存与显存带宽）"
        if (risks.isEmpty()) return emptyList()
        return listOf(
            Finding(
                level = Level.WARN,
                check = CHECK_VRAM,
                title = "显存可能不够（本机实测是 RTX 4060 Laptop / 8GB）",
                detail = "风险点：" + risks.joinToString("；") + "。8GB 是小卡，这几个因素叠加时" +
                    "ComfyUI 会 OOM 或退化到共享内存（慢十倍以上）。",
                fix = "先在 `comfy_get_status` / `server_info` 看当前显存占用；真要跑就降一档" +
                    "（尺寸、放大倍数、LoRA 数量三者择一），或加 `comfy_prune_workflow` 的 Sage Attention 补丁省显存。",
            ),
        )
    }

    // -----------------------------------------------------------------------
    //  ⑥ 提示词风格与模型是否匹配
    // -----------------------------------------------------------------------

    private fun checkPromptStyle(graph: JsonObject): List<Finding> {
        val modelName = graph.values.mapNotNull { raw ->
            val node = raw as? JsonObject ?: return@mapNotNull null
            val inputs = node["inputs"] as? JsonObject ?: return@mapNotNull null
            listOf("ckpt_name", "unet_name")
                .firstNotNullOfOrNull { (inputs[it] as? JsonPrimitive)?.contentOrNull }
        }.firstOrNull() ?: return emptyList()

        val positives = graph.values.mapNotNull { raw ->
            val node = raw as? JsonObject ?: return@mapNotNull null
            if (node.str("class_type").orEmpty() !in setOf("CLIPTextEncode", "CLIPTextEncodeSDXL", "TextEncodeQwenImageEdit")) {
                return@mapNotNull null
            }
            val inputs = node["inputs"] as? JsonObject ?: return@mapNotNull null
            (inputs["text"] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
        }
        if (positives.isEmpty()) return emptyList()
        val text = positives.maxByOrNull { it.length } ?: return emptyList()
        val commas = text.count { it == ',' }
        val hasSentence = Regex("""[.!?]|\bwith\b|\bwearing\b|\bstanding\b""", RegexOption.IGNORE_CASE)
            .containsMatchIn(text)

        return when {
            modelName.contains("krea", true) && commas >= 8 && !hasSentence -> listOf(
                Finding(
                    level = Level.WARN,
                    check = CHECK_PROMPT_STYLE,
                    title = "给 Krea 2 写的是 Danbooru tag 堆，而它要自然语言",
                    detail = "这条正向提示词有 $commas 个逗号、几乎没有成句结构（前 80 字：${
                        text.take(80).replace('\n', ' ')
                    }…）。Krea 2 官方明确推荐 natural language，" + "\"masterpiece, best quality\" 这类咒语没有意义。",
                    fix = "按 `krea-2` skill 的官方规则重写（主体+属性+动作 → 场景 → 构图 → 光线 → 媒介），" +
                        "tag 版留给 Anima；或者明确告诉用户你要用 tag 写法。",
                ),
            )
            modelName.contains("anima", true) && commas <= 2 && text.length > 200 -> listOf(
                Finding(
                    level = Level.INFO,
                    check = CHECK_PROMPT_STYLE,
                    title = "给 Anima 写的是一整段自然语言",
                    detail = "Anima 是 Danbooru tag 模型：外观/动作/表情这些关键特征写 tag 更稳，" +
                        "自然语言拿来补氛围。这条正向提示词 $commas 个逗号、${text.length} 字。",
                    fix = "按 `anima-prompt` skill 转成 tag 为主（NL 只补氛围）；纯场景图可以照旧走" +
                        "`anima-scene-prompt`。",
                ),
            )
            else -> emptyList()
        }
    }

    // -----------------------------------------------------------------------
    //  ⑦ 加载器与目录不匹配
    // -----------------------------------------------------------------------

    private fun checkLoaderFolder(graph: JsonObject, inventory: ComfyModels.Inventory?): List<Finding> {
        if (inventory == null || inventory.models.isEmpty()) return emptyList()
        // 某个文件名在本机哪些目录里出现过
        fun foldersOf(name: String): Set<String> = inventory.models
            .filter { ModelName.same(name, it.name) || it.name.equals(name, ignoreCase = true) }
            .map { it.folder }
            .toSet()

        val out = mutableListOf<Finding>()
        graph.forEach { (id, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val cls = node.str("class_type").orEmpty()
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            // UNETLoader 只读 diffusion_models/（unet/），拿 checkpoints 里的文件一定报不存在
            val wanted = when {
                cls.startsWith("UNETLoader") || cls == "DiffusionModelLoader" ->
                    (inputs["unet_name"] ?: inputs["model_name"]).asText() to "diffusion_models"
                cls.contains("CheckpointLoader") -> inputs["ckpt_name"].asText() to "checkpoints"
                else -> null
            } ?: return@forEach
            val (file, expected) = wanted
            if (file.isBlank()) return@forEach
            val folders = foldersOf(file)
            if (folders.isEmpty() || expected in folders) return@forEach
            out += Finding(
                level = Level.WARN,
                check = CHECK_LOADER_FOLDER,
                title = "$cls 读的目录不对：$file 不在 $expected/ 里",
                detail = "本机这份文件在 " + folders.joinToString("、") { "$it/" } + "，" +
                    "而 $cls 只认 $expected/（枚举里看不到它 → 提交会报文件不存在）。",
                fix = "换成该目录里真实存在的文件（`comfy_fit_workflow` 会给候选），" +
                    "或者换用读那个目录的加载器（例如 `CheckpointLoaderSimple` ← → `UNETLoader`）。",
                nodes = listOf(id),
            )
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  小工具
    // -----------------------------------------------------------------------

    private fun JsonObject.str(key: String): String? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

    private fun JsonElement?.asText(): String = (this as? JsonPrimitive)?.contentOrNull.orEmpty()

    /** 数值（整数）。JSON 里 `20` 是数字；前端有时写成 `"20"`，两种都认。 */
    private fun JsonPrimitive.intOrNull(): Int? = contentOrNull?.trim()?.toIntOrNull()

    private fun JsonPrimitive.doubleOrNull(): Double? = contentOrNull?.trim()?.toDoubleOrNull()

    /** 固定种子（数字或数字串）；`"randomize"` 这种前端占位符不算。 */
    private fun JsonPrimitive.isNumberLike(): Boolean = contentOrNull?.trim()?.toDoubleOrNull() != null
}
