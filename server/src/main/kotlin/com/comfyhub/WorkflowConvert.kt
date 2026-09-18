package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * ComfyUI **界面格式工作流**（`{nodes:[…], links:[…]}`）→ **API 格式节点图**
 * （`{"<节点id>": {class_type, inputs}}`，也就是 `POST /prompt` 唯一接受的那种）。
 *
 * 为什么需要它（用户 bug ③）：用户指了一份本机的工作流文件
 * `…\user\default\workflows\krea2….json` 让 AI 去跑，而 `comfy_submit` 只认库里的 `promptId` ——
 * 那条工作流从来没被捕获过，于是整件事卡死在"我不能凭一个文件路径提交"。
 *
 * 界面格式里 `widgets_values` 默认**只有位置、没有参数名**，所以转换必须知道每个节点的
 * 输入声明顺序 —— 那份声明只有 ComfyUI 的 `/object_info` 有，
 * 因此转换是**在线**的（提交前顺手问一次 ComfyUI），不是一个纯函数。
 *
 * ## 能做到什么、做不到什么（都必须如实说）
 *
 * 能：普通 Python 节点（`/object_info` 里有定义的）—— 连线按 `links` 还原成
 * `["上游节点id", 槽位]`，控件值按声明顺序贴回名字上，`control_after_generate`
 * 多占的那个槽位（`seed` 后面那个下拉框）也照跳；
 * **首选带名字的 `widgets_values_named`**（新版前端会写一份，见 [fillWidgets]）。
 *
 * 能等价改写的界面专用节点就改写，不让用户看见"转换不了"：
 *  - `Note` / `MarkdownNote` / `PrimitiveNode`：纯界面标记，丢掉（ComfyUI 前端也把它们从提示词里剔除）；
 *  - `Reroute` / `SetNode` / `GetNode`：把连线接到真正的上游（`GetNode` 按变量名找回 `SetNode`）；
 *  - **组节点（subgraph / `definitions.subgraphs`）**：按定义**展开**成内部的真节点（见 [flatten]）；
 *  - `mode=4`（旁路 / bypass）：按 ComfyUI 的语义**把输出接到同类型的输入上**（见 [bypassRedirects]），
 *    这是实际工作流里最常见的一种状态（本机实测一份 91 节点的存档里有 61 个是旁路），
 *    直接拒绝会让这个功能对真实文件完全不可用；
 *  - `mode=2`（静音 / never）：节点不执行、也不出现在提示词里，下游会缺输入 —— **警告**如实列出。
 *
 * 不能（**一律报错，绝不猜**）：只用前端 JS 实现的"广播型"虚拟节点，典型的是
 * `Anything Everywhere`（把一路输入广播到全图所有同类型的空输入上）。ComfyUI 的官方前端在排队前
 * 自己会把它们改写掉，服务端拿不到那份 JS 也不该照猫画虎 —— 硬猜的后果是 ComfyUI 报一堆
 * 莫名其妙的节点错误，用户根本查不出来，所以这里宁可失败并说清楚出口在哪。
 *
 * 但"宁可失败"不等于"卡死"：转换结果里会带上**缺口清单**（[Result.unresolvedInputs] /
 * [Result.openInputs]），调用方（`comfy_load_workflow` + AI，见 `docs/ai-tools-and-skills.md`）
 * 可以照着这份清单把缺口补上再提交 —— 补的是**具体哪根线接哪**，不是让谁去猜语义。
 */
object WorkflowConvert {

    /**
     * 转换结果。
     *
     * [unsupportedNodes] 非空时**不许当成"能跑"** —— 调用方要么如实报错，要么让模型把缺口补齐。
     * [warnings] 是可以继续跑、但用户/模型值得知道的事（控件值个数对不上、被静音的节点…）。
     */
    data class Result(
        val graph: JsonObject,
        val nodeCount: Int,
        /** 被等价改写 / 丢掉的界面专用节点，形如 `12 Note` */
        val rewritten: List<String> = emptyList(),
        val warnings: List<String> = emptyList(),
        /** `/object_info` 里没有、也不是已知虚拟节点的类，形如 `37 Anything Everywhere` */
        val unsupportedNodes: List<String> = emptyList(),
        /**
         * 图里还**悬着**的输入：某个输入的上游节点没能被转换出来（被摘掉 / 没能展开），
         * 形如 `60.vae ← 节点 1`。这种图提交上去 ComfyUI 会报缺输入，所以必须报出来。
         */
        val unresolvedInputs: List<String> = emptyList(),
        /**
         * 现在**空着**的连线型输入，形如 `60.vae (VAE)`。
         *
         * 纯前端广播节点（`Anything Everywhere`）干的事就是往这些地方灌值 —— 所以这份清单
         * 正好是"这类节点被摘掉以后要补哪些线"的作业本。
         */
        val openInputs: List<String> = emptyList(),
    )

    /** litegraph 的节点状态：0 = 正常，2 = 静音（永不执行），4 = 旁路（接过去） */
    private const val MODE_MUTED = 2
    private const val MODE_BYPASS = 4

    /**
     * 只由前端 JS 实现、后端没有节点定义的"虚拟节点"。
     *
     * 注意判定顺序：**先看 `/object_info` 里有没有**。有的版本把 `SetNode` 之类做成了
     * 真节点（那时它在提示词里就该原样保留），所以只有"查不到 + 名字在这张表里"才改写。
     */
    private val FRONTEND_VIRTUAL = setOf(
        "Note", "MarkdownNote", "PrimitiveNode", "Reroute", "SetNode", "GetNode",
    )

    /** 会被当作"控件"（有 widgets_values 槽位）的类型。其余都是连线插口。 */
    private val WIDGET_TYPES = setOf("INT", "FLOAT", "STRING", "BOOLEAN", "COMBO")

    /** 这份 JSON 是不是界面格式工作流（带 nodes / links）。 */
    fun isUiWorkflow(obj: JsonObject): Boolean = obj["nodes"] is JsonArray

    // -----------------------------------------------------------------------
    //  主流程
    // -----------------------------------------------------------------------

    fun toApiGraph(ui: JsonObject, objectInfo: JsonObject): Result {
        val flat = flatten(ui, objectInfo)
        val rawNodes = flat.nodes
        if (rawNodes.isEmpty()) throw IllegalArgumentException("工作流里一个节点都没有（nodes 是空的）")

        val links = flat.links
        // 改写表：(节点id, 输出槽位) → 真正的上游 (节点id, 槽位)。
        // Reroute / Set-Get / 旁路 / 组节点的输出都登记在这里。
        val redirects = flat.redirects
        val out = LinkedHashMap<String, JsonElement>()
        val rewritten = flat.rewritten
        val unsupported = flat.unsupported
        val warnings = flat.warnings

        // --- 第一遍：登记所有可等价改写的界面节点（必须在解析连线之前全部登记完） ---
        val setByName = mutableMapOf<String, Pair<String, Int>>()
        rawNodes.forEach { node ->
            val cls = node.uiClass()
            if (cls == "SetNode" && objectInfo[cls] == null) {
                val source = node.firstLinkedInput(links)?.let { resolve(it, redirects) }
                val name = node.widgetString()
                if (source != null && name != null) setByName[name] = source
            }
        }
        rawNodes.forEach { node ->
            val cls = node.uiClass()
            val id = node.idOf() ?: return@forEach
            if (cls.isEmpty() || objectInfo[cls] != null) return@forEach
            when (cls) {
                "Reroute", "SetNode" -> {
                    node.firstLinkedInput(links)?.let { redirects[id to 0] = resolve(it, redirects) }
                    rewritten += "$id $cls"
                }
                "GetNode" -> {
                    val name = node.widgetString()
                    val source = name?.let { setByName[it] }
                    if (source != null) {
                        redirects[id to 0] = source
                        rewritten += "$id $cls"
                    } else {
                        unsupported += "$id $cls（找不到同名的 SetNode「${name ?: "?"}」）"
                    }
                }
                in FRONTEND_VIRTUAL -> rewritten += "$id $cls"
            }
        }
        // 旁路：按输出槽位登记"接到哪个同类型输入上"
        rawNodes.forEach { node ->
            if ((node.int("mode") ?: 0) != MODE_BYPASS) return@forEach
            val id = node.idOf() ?: return@forEach
            val cls = node.uiClass()
            bypassRedirects(id, node, links, redirects, warnings)
            rewritten += "$id $cls（旁路）"
        }

        // --- 第二遍：真正转成节点 ---
        val redirectedIds = redirects.keys.map { it.first }.toSet()
        val openInputs = mutableListOf<String>()
        for (node in rawNodes) {
            val id = node.idOf() ?: continue
            val cls = node.uiClass()
            if (cls.isEmpty()) continue
            val mode = node.int("mode") ?: 0
            // 组节点实例已经被展开成内部的真节点，它自己不再是一个节点
            if (id in flat.handled) continue
            if (id in redirectedIds) continue
            if (cls in FRONTEND_VIRTUAL && objectInfo[cls] == null) continue
            if (mode == MODE_MUTED) {
                // 静音 = 不执行。它自己不进提示词；下游拿不到输入，ComfyUI 会报错，
                // 所以这里必须留一句警告，别让用户以为"转换成功了"。
                val consumers = consumersOf(id, rawNodes, links)
                warnings += "节点 $id（$cls）是静音的，它的输出不会产生" +
                    if (consumers.isEmpty()) "" else "，下游（${consumers.joinToString("、")}）可能缺输入"
                continue
            }
            if (mode == MODE_BYPASS) continue
            if (mode != 0) {
                unsupported += "$id $cls（未知的节点状态 mode=$mode）"
                continue
            }

            val info = objectInfo[cls] as? JsonObject
            if (info == null) {
                unsupported += "$id $cls"
                continue
            }

            val inputs = LinkedHashMap<String, JsonElement>()
            // 1) 连线优先：界面格式里 inputs[].link 指向 links 表
            (node["inputs"] as? JsonArray)?.forEach { entry ->
                val e = entry as? JsonObject ?: return@forEach
                val name = e["name"].asText()?.takeIf { it.isNotBlank() } ?: return@forEach
                if (inputs.containsKey(name)) return@forEach
                val linkId = e["link"].asLong() ?: return@forEach
                val ref = links[linkId] ?: return@forEach
                // 穿透 Reroute / Set-Get / 旁路的节点，拿到真正的产出节点
                val source = resolve(ref, redirects)
                inputs[name] = JsonArray(listOf(refOf(source.first), JsonPrimitive(source.second)))
            }
            // 2) 组节点把内部节点的输入暴露成外层控件时，值直接填进那个输入
            flat.forcedInputs[id]?.let { inputs.putAll(it) }
            // 3) 控件值：按 /object_info 的声明顺序贴回名字上
            fillWidgets(node, widgetSpecs(info), inputs, warnings, id, cls)

            out[id] = buildJsonObject {
                put("class_type", cls)
                put("inputs", JsonObject(inputs))
                node["title"].asText()?.takeIf { it.isNotBlank() }?.let {
                    put("_meta", buildJsonObject { put("title", it) })
                }
            }
            collectOpenInputs(info, inputs, id, openInputs)
        }

        return Result(
            graph = JsonObject(out),
            nodeCount = out.size,
            rewritten = rewritten,
            warnings = warnings,
            unsupportedNodes = unsupported,
            unresolvedInputs = danglingInputs(out),
            openInputs = openInputs,
        )
    }

    /**
     * 图里有没有"指向一个不存在的节点"的输入。
     *
     * 这不是吹毛求疵：被摘掉的节点（静音的、未知类的、旁路又找不到同类型输入的）留下的
     * **悬挂引用**提交上去，ComfyUI 只会回一句"节点 xxx 不存在"，用户完全看不出是哪一步坏的。
     */
    private fun danglingInputs(out: Map<String, JsonElement>): List<String> {
        val dangling = mutableListOf<String>()
        out.forEach { (id, value) ->
            val node = value as? JsonObject ?: return@forEach
            (node["inputs"] as? JsonObject)?.forEach { (name, v) ->
                val ref = v as? JsonArray ?: return@forEach
                val source = ref.firstOrNull().asText() ?: return@forEach
                if (source !in out) dangling += "$id.$name ← 节点 $source（没有转换出来）"
            }
        }
        return dangling
    }

    /**
     * 列出这个节点**现在空着的连线型输入**（广播型虚拟节点原来就是往这些地方灌值的）。
     *
     * 只算"插口型"：控件型（INT / FLOAT / STRING / BOOLEAN / COMBO）不算 —— 它们空着时
     * ComfyUI 用默认值，不是缺输入。
     */
    private fun collectOpenInputs(
        info: JsonObject,
        inputs: Map<String, JsonElement>,
        id: String,
        into: MutableList<String>,
    ) {
        val input = info["input"] as? JsonObject ?: return
        for (group in listOf("required", "optional")) {
            val g = input[group] as? JsonObject ?: continue
            for ((name, raw) in g) {
                if (name in inputs) continue
                val arr = raw as? JsonArray ?: continue
                // 控件型（含 COMBO）空着时 ComfyUI 用默认值，不算缺口；
                // forceInput 是**插口**，哪怕类型是 STRING 也算缺口
                if (isWidgetInput(arr)) continue
                val type = (arr.firstOrNull() as? JsonPrimitive)?.contentOrNull ?: continue
                into += "$id.$name ($type)"
            }
        }
    }

    // -----------------------------------------------------------------------
    //  组节点（subgraph）展开
    // -----------------------------------------------------------------------

    /** 展开组节点之后得到的图。 */
    private class Flat(
        val nodes: List<JsonObject>,
        /** linkId → (上游节点 id, 槽位)。节点 id 统一是字符串（UI 格式里是数字，转成文本）。 */
        val links: Map<Long, Pair<String, Int>>,
        /** 预置的改写表：组节点的输出槽位 → 内部真正的产出节点 */
        val redirects: MutableMap<Pair<String, Int>, Pair<String, Int>>,
        /** 已经处理过的组节点实例 id：要么展开了，要么已经带着**具体原因**报过错了 */
        val handled: Set<String>,
        /** 内部节点的某个输入由"实例上的控件"提供时，直接填进去的值 */
        val forcedInputs: MutableMap<String, MutableMap<String, JsonElement>>,
        val rewritten: MutableList<String>,
        val unsupported: MutableList<String>,
        val warnings: MutableList<String>,
    )

    /** 界面格式里的一条连线（两种写法都归一成这个）。 */
    private data class RawLink(
        val origin: String,
        val originSlot: Int,
        val target: String,
        val targetSlot: Int,
    )

    /**
     * 把界面格式的图**摊平**：组节点就地展开成内部的真节点，连线合并成一张表。
     *
     * 组节点（新版 ComfyUI 的 subgraph / group node）在存档里是这样的：
     *  - `definitions.subgraphs[]` 每个定义有 `id`(UUID) / `nodes` / `links` / `inputs` / `outputs`
     *    以及两个**代理节点** `inputNode.id`（本机实测是 -10）与 `outputNode.id`（-20）；
     *  - 主图里出现一个 `type` 等于那个 UUID 的节点，它的 `inputs` 与定义里的 `inputs`
     *    **按顺序一一对应**（实测 6 个组节点全部如此）。
     *
     * 服务端拿不到前端那份 JS，但这些是**数据**，可以照实展开：
     *  - 内部节点会与主图**撞号**（内部 id 也是 1、2、3…）→ 重新分配一批没人用过的新号；
     *  - 内部连线的 id 也是局部编号（本机实测出现过 14717 这种与主图重号的）→ 整表重新编号；
     *  - `origin_id = inputNode` 的连线 = "从实例的第 k 个输入进来"：接上主图那边的真实来源；
     *    那个输入没连线、而是外层控件时（分辨率 / 方向这类），把实例的控件值
     *    直接填给内部节点的对应输入（**不能走控件顺序**，那是另一套编号）；
     *  - `target_id = outputNode` 的连线 = "实例的第 k 个输出"：登记成改写，
     *    让下游拿到内部真正产出那个节点；
     *  - `mode != 0` 的实例**不展开**：它自己不执行，交给旁路 / 静音那套逻辑按类型接过去。
     */
    private fun flatten(ui: JsonObject, objectInfo: JsonObject): Flat {
        val defs = subgraphDefs(ui)
        val topNodes = (ui["nodes"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
        val topLinks = parseRawLinks(ui["links"] as? JsonArray)

        val nodes = mutableListOf<JsonObject>()
        val links = LinkedHashMap<Long, Pair<String, Int>>()
        val redirects = mutableMapOf<Pair<String, Int>, Pair<String, Int>>()
        val forcedInputs = mutableMapOf<String, MutableMap<String, JsonElement>>()
        val handled = mutableSetOf<String>()
        val rewritten = mutableListOf<String>()
        val unsupported = mutableListOf<String>()
        val warnings = mutableListOf<String>()

        // 新号从"现有最大值 + 1"开始，保证不与任何已有编号冲突
        var nextLinkId = (topLinks.keys.maxOrNull() ?: 0L) + 1L
        var nextNodeId = (topNodes.mapNotNull { it.idOf()?.toLongOrNull() }.maxOrNull() ?: 0L) + 1L

        topLinks.forEach { (id, l) -> links[id] = l.origin to l.originSlot }

        topNodes.forEach { node ->
            val id = node.idOf()
            val cls = node.uiClass()
            val def = if (cls.isNotEmpty() && objectInfo[cls] == null) defs[cls] else null
            val mode = node.int("mode") ?: 0
            if (def == null || id == null || mode != 0) {
                nodes += node
                return@forEach
            }

            val short = cls.take(8)
            val inputProxy = (def["inputNode"] as? JsonObject)?.idOf()
            val outputProxy = (def["outputNode"] as? JsonObject)?.idOf()
            val iface = (def["inputs"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
            val instInputs = (node["inputs"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
            val innerNodes = (def["nodes"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }

            if (inputProxy == null || outputProxy == null || innerNodes.isEmpty()) {
                unsupported += "$id $short…（组节点：定义里缺少输入 / 输出代理，不敢猜着展开）"
                handled += id
                nodes += node
                return@forEach
            }
            if (iface.size != instInputs.size) {
                unsupported += "$id $short…（组节点实例有 ${instInputs.size} 个输入，" +
                    "定义里是 ${iface.size} 个，对不上，不敢猜着展开）"
                handled += id
                nodes += node
                return@forEach
            }
            if (innerNodes.any { defs.containsKey(it.uiClass()) }) {
                unsupported += "$id $short…（组节点里还套着一层组节点，这一版不展开）"
                handled += id
                nodes += node
                return@forEach
            }

            val innerLinks = parseRawLinks(def["links"] as? JsonArray)
            val renumber = HashMap<Long, Long>()
            innerLinks.keys.forEach { renumber[it] = nextLinkId++ }
            val idMap = HashMap<String, String>()
            innerNodes.forEach { inner -> inner.idOf()?.let { idMap[it] = (nextNodeId++).toString() } }

            // 实例的每个输入：接了线 → 真实来源；没接线但是控件 → 值（等会儿填给内部节点）
            val sourceOfSlot = arrayOfNulls<Pair<String, Int>>(iface.size)
            val widgetOfSlot = arrayOfNulls<JsonElement>(iface.size)
            instInputs.forEachIndexed { k, e ->
                val src = e["link"].asLong()?.let { topLinks[it] }
                if (src != null) {
                    sourceOfSlot[k] = src.origin to src.originSlot
                } else {
                    val name = iface.getOrNull(k)?.get("name").asText()
                    val named = node["widgets_values_named"] as? JsonObject
                    val positional = (node["widgets_values"] as? JsonArray)?.getOrNull(k)
                    val v = name?.let { named?.get(it) } ?: positional
                    if (v != null && v !is JsonNull) widgetOfSlot[k] = v
                }
            }

            // 内部连线：输入代理换成真实来源；输出代理登记成实例输出的改写
            val widgetFix = mutableMapOf<Long, Pair<String, JsonElement>>()
            innerLinks.forEach { (oldId, l) ->
                val newId = renumber.getValue(oldId)
                if (l.origin == inputProxy) {
                    val src = sourceOfSlot.getOrNull(l.originSlot)
                    val w = widgetOfSlot.getOrNull(l.originSlot)
                    when {
                        src != null -> links[newId] = src
                        // 值来自外层控件：记下"哪个内部节点的哪个输入"，等会儿直接填
                        w != null -> {
                            val targetId = idMap[l.target]
                            if (targetId != null) widgetFix[oldId] = targetId to w
                        }
                        // 都没有：这个输入本来就空着 —— 号不登记，内部那个输入就是"悬着"的
                    }
                } else {
                    links[newId] = (idMap[l.origin] ?: l.origin) to l.originSlot
                }
                if (l.target == outputProxy) {
                    redirects[id to l.targetSlot] = (idMap[l.origin] ?: l.origin) to l.originSlot
                }
            }

            // 内部节点本体：换号、重指连线、把外层控件的值填进对应输入
            innerNodes.forEach { inner ->
                val raw = inner.idOf() ?: return@forEach
                val newId = idMap[raw] ?: return@forEach
                val ins = (inner["inputs"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
                val fixed = mutableMapOf<String, JsonElement>()
                val newIns = ins.map { e ->
                    val old = e["link"].asLong() ?: return@map e
                    val fix = widgetFix[old]?.takeIf { it.first == newId }
                    if (fix != null) {
                        val name = e["name"].asText()
                        if (name != null) fixed[name] = fix.second
                        return@map JsonObject(e + ("link" to JsonNull))
                    }
                    val nid = renumber[old] ?: return@map e
                    JsonObject(e + ("link" to JsonPrimitive(nid)))
                }
                if (fixed.isNotEmpty()) forcedInputs[newId] = fixed
                nodes += JsonObject(
                    inner + mapOf("id" to JsonPrimitive(newId), "inputs" to JsonArray(newIns)),
                )
            }
            handled += id
            rewritten += "$id $short…（组节点，展开成 ${innerNodes.size} 个节点）"
        }

        return Flat(nodes, links, redirects, handled, forcedInputs, rewritten, unsupported, warnings)
    }

    private fun subgraphDefs(ui: JsonObject): Map<String, JsonObject> {
        val subs = ((ui["definitions"] as? JsonObject)?.get("subgraphs") as? JsonArray)
            ?: return emptyMap()
        return subs.mapNotNull { it as? JsonObject }
            .mapNotNull { d -> d["id"].asText()?.let { it to d } }
            .toMap()
    }

    /** `links` 表：linkId → 连线两端。数组写法与对象写法都认。 */
    private fun parseRawLinks(links: JsonArray?): Map<Long, RawLink> {
        val out = HashMap<Long, RawLink>()
        links.orEmpty().forEach { entry ->
            when (entry) {
                // 老写法：[link_id, origin_id, origin_slot, target_id, target_slot, type]
                is JsonArray -> {
                    val linkId = entry.getOrNull(0).asLong() ?: return@forEach
                    val origin = entry.getOrNull(1).asText() ?: return@forEach
                    out[linkId] = RawLink(
                        origin = origin,
                        originSlot = entry.getOrNull(2).asInt() ?: 0,
                        target = entry.getOrNull(3).asText().orEmpty(),
                        targetSlot = entry.getOrNull(4).asInt() ?: 0,
                    )
                }
                // 新写法：{id, origin_id, origin_slot, target_id, target_slot, …}
                is JsonObject -> {
                    val linkId = entry["id"].asLong() ?: return@forEach
                    val origin = entry["origin_id"].asText() ?: return@forEach
                    out[linkId] = RawLink(
                        origin = origin,
                        originSlot = entry["origin_slot"].asInt() ?: 0,
                        target = entry["target_id"].asText().orEmpty(),
                        targetSlot = entry["target_slot"].asInt() ?: 0,
                    )
                }
                else -> Unit
            }
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  旁路（mode=4）
    // -----------------------------------------------------------------------

    /**
     * 旁路的语义：这个节点不执行，但"信号"要穿过去 —— 它的每个输出接到**同类型的某个输入**上。
     *
     * 规则与 ComfyUI 前端一致的地方是"同类型"；槽位对应关系上，先试**同名下标**的输入，
     * 不行再退到"第一个同类型且有连线的输入"（多输入同类型时这是最接近前端行为的近似）。
     * 一个输出找不到可对应的输入时不静默放过：那时下游会缺输入，如实写进警告。
     */
    private fun bypassRedirects(
        id: String,
        node: JsonObject,
        links: Map<Long, Pair<String, Int>>,
        redirects: MutableMap<Pair<String, Int>, Pair<String, Int>>,
        warnings: MutableList<String>,
    ) {
        val outputs = (node["outputs"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
        val inputs = (node["inputs"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
        val linkedInputs = inputs.mapIndexedNotNull { index, e ->
            val linkId = e["link"].asLong() ?: return@mapIndexedNotNull null
            val source = links[linkId] ?: return@mapIndexedNotNull null
            LinkedInput(index, e["type"].asText(), e["name"].asText(), source)
        }
        outputs.forEachIndexed { slot, output ->
            val type = output["type"].asText()
            val sameSlot = linkedInputs.firstOrNull { it.index == slot && it.type == type }
            val sameType = sameSlot ?: linkedInputs.firstOrNull { it.type == type }
            val source = sameType?.source
            if (source == null) {
                warnings += "节点 $id（${node.uiClass()}）是旁路的，但输出「${output["name"].asText() ?: slot}」" +
                    "（${type ?: "?"}）没有同类型的已连接输入可以接过去 —— 下游可能缺输入"
                return@forEachIndexed
            }
            redirects[id to slot] = resolve(source, redirects)
        }
    }

    private data class LinkedInput(
        val index: Int,
        val type: String?,
        val name: String?,
        val source: Pair<String, Int>,
    )

    /** 谁引用了这个节点的输出（给"静音节点"的警告用）。 */
    private fun consumersOf(
        id: String,
        nodes: List<JsonObject>,
        links: Map<Long, Pair<String, Int>>,
    ): List<String> {
        val out = mutableListOf<String>()
        nodes.forEach { n ->
            (n["inputs"] as? JsonArray)?.forEach { entry ->
                val e = entry as? JsonObject ?: return@forEach
                val linkId = e["link"].asLong() ?: return@forEach
                if (links[linkId]?.first == id) {
                    out += "${n.idOf() ?: "?"}（${n.uiClass()}）"
                }
            }
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  控件值
    // -----------------------------------------------------------------------

    private fun fillWidgets(
        node: JsonObject,
        specs: List<WidgetSpec>,
        inputs: MutableMap<String, JsonElement>,
        warnings: MutableList<String>,
        id: String,
        cls: String,
    ) {
        if (specs.isEmpty()) return
        // 0) **首选带名字的那份**（`widgets_values_named`）。
        //
        // 新版 ComfyUI 前端会顺便写一份 `{控件名: 值}`。有它就不必按位置猜了 ——
        // 位置式那份之所以要猜，是因为自定义节点可能多几个界面专用控件，
        // 一旦个数对不上，后面所有参数就整体错位（只能给一句"留意报错"的警告）。
        //
        // 只认 `/object_info` **声明过**的控件名：手上的前端版本与后端节点版本不一致时，
        // 名字表里可能多出后端不认的键，直接塞进去 ComfyUI 会当成未知参数拒掉整张图。
        val named = node["widgets_values_named"] as? JsonObject
        if (named != null && named.isNotEmpty()) {
            val declared = specs.map { it.name }.toSet()
            named.forEach { (name, v) ->
                if (name in inputs || v is JsonNull) return@forEach
                if (name in declared) {
                    inputs[name] = v
                } else {
                    warnings += "节点 $id（$cls）的控件值里有 `$name`，但节点的输入声明里没有它 —— 已忽略" +
                        "（多半是界面版本与后端节点版本不一致）"
                }
            }
            return
        }
        when (val values = node["widgets_values"]) {
            // 少数节点（自定义节点）把控件存成对象：按名字取，最稳
            is JsonObject -> {
                specs.forEach { spec ->
                    val v = values[spec.name] ?: return@forEach
                    if (spec.name !in inputs && v !is JsonNull) inputs[spec.name] = v
                }
            }
            is JsonArray -> {
                // 已经被连线占掉的输入不重复写（ComfyUI 会当未知参数拒掉）
                val pending = specs.filter { it.name !in inputs }
                val expected = pending.size + pending.count { it.extraSlot }
                if (values.size != expected) {
                    warnings += "节点 $id（$cls）的控件值有 ${values.size} 个，按输入声明应该是 $expected 个" +
                        "（多半是自定义节点多了界面专用的控件）—— 参数可能对不齐，跑之前留意 ComfyUI 的报错"
                }
                var i = 0
                for (spec in pending) {
                    val v = values.getOrNull(i) ?: break
                    i++
                    if (v !is JsonNull) inputs[spec.name] = v
                    // 种子后面跟的那个「control_after_generate」下拉框也占一个槽位
                    if (spec.extraSlot) i++
                }
            }
            else -> return
        }
    }

    private data class WidgetSpec(val name: String, val extraSlot: Boolean)

    /**
     * 一个输入声明算不算"控件"（= 有 widgets_values 槽位）。
     *
     * `forceInput` 的例外很关键：它被**强制成连线插口**了，哪怕类型是 STRING 也没有控件槽位 ——
     * 把它当控件会让后面所有控件的取值整体错位。
     */
    private fun isWidgetInput(decl: JsonArray): Boolean {
        val options = decl.getOrNull(1) as? JsonObject
        if (options?.get("forceInput").asBool() == true) return false
        return when (val typeEl = decl.firstOrNull()) {
            is JsonArray -> true
            is JsonPrimitive -> typeEl.contentOrNull?.uppercase() in WIDGET_TYPES
            else -> false
        }
    }

    /** 一个节点的控件（= 有 widgets_values 槽位的输入）名字与顺序，来自 `/object_info`。 */
    private fun widgetSpecs(info: JsonObject): List<WidgetSpec> {
        val input = info["input"] as? JsonObject ?: return emptyList()
        val out = mutableListOf<WidgetSpec>()
        for (group in listOf("required", "optional")) {
            val g = input[group] as? JsonObject ?: continue
            for ((name, raw) in g) {
                val arr = raw as? JsonArray ?: continue
                if (!isWidgetInput(arr)) continue
                val options = arr.getOrNull(1) as? JsonObject
                out += WidgetSpec(name, options?.get("control_after_generate").asBool() == true)
            }
        }
        return out
    }

    /**
     * 把一个引用一路解到"真正的产出节点"上（穿透 `Reroute` / `Set`/`Get` / 旁路 / 组节点输出）。
     * 带步数上限，防止工作流里出现环时转不出来。
     */
    private fun resolve(
        ref: Pair<String, Int>,
        redirects: Map<Pair<String, Int>, Pair<String, Int>>,
    ): Pair<String, Int> {
        var current = ref
        var guard = 0
        while (guard++ < 64) {
            val next = redirects[current] ?: return current
            if (next == current) return current
            current = next
        }
        return current
    }

    /** 节点第一个有连线的输入 → (上游节点, 槽位)。 */
    private fun JsonObject.firstLinkedInput(links: Map<Long, Pair<String, Int>>): Pair<String, Int>? {
        (this["inputs"] as? JsonArray)?.forEach { entry ->
            val e = entry as? JsonObject ?: return@forEach
            val linkId = e["link"].asLong() ?: return@forEach
            links[linkId]?.let { return it }
        }
        return null
    }

    /** `SetNode` / `GetNode` 的"变量名"（唯一那个控件值）。 */
    private fun JsonObject.widgetString(): String? {
        val values = this["widgets_values"] ?: return null
        return when (values) {
            is JsonArray -> values.firstOrNull().asText()
            is JsonObject -> values.values.firstOrNull().asText()
            else -> null
        }?.takeIf { it.isNotBlank() }
    }

    private fun JsonObject.int(field: String): Int? = this[field].asInt()

    /**
     * 节点引用写成**数字**（节点 id 都是数字，与 ComfyUI 自己导出的一致）。
     * 万一以后出现非数字 id（理论上可能），退回文本 —— 两边的查表用的都是对象键，都认。
     */
    private fun refOf(id: String): JsonPrimitive =
        id.toLongOrNull()?.let { JsonPrimitive(it) } ?: JsonPrimitive(id)

    /** 节点 id 统一当**文本**用（界面格式里是数字，API 图里是对象键）。 */
    private fun JsonObject.idOf(): String? = this["id"].asText()?.takeIf { it.isNotBlank() }

    /**
     * 界面格式里节点的类名在 **`type`**，不是 API 格式那个 `class_type`
     * （[JsonObject.cls] 读的是后者）。
     *
     * 这两个键名混用是**踩过的坑**：第一版这里用了 `cls()`，于是每个节点都被当成"没有类名"
     * 直接跳过，转换结果是**一张空图** —— 而空图提交上去只会得到 ComfyUI 一句
     * "Prompt has no outputs"，完全看不出真正原因。用例里因此专门盯着 nodeCount。
     */
    private fun JsonObject.uiClass(): String = this["type"].asText().orEmpty()
}
