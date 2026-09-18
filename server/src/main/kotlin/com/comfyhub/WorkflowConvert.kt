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
 * 多占的那个槽位（`seed` 后面那个下拉框）也照跳。
 *
 * 能等价改写的界面专用节点就改写，不让用户看见"转换不了"：
 *  - `Note` / `MarkdownNote` / `PrimitiveNode`：纯界面标记，丢掉（ComfyUI 前端也把它们从提示词里剔除）；
 *  - `Reroute` / `SetNode` / `GetNode`：把连线接到真正的上游（`GetNode` 按变量名找回 `SetNode`）；
 *  - `mode=4`（旁路 / bypass）：按 ComfyUI 的语义**把输出接到同类型的输入上**（见 [bypassRedirects]），
 *    这是实际工作流里最常见的一种状态（本机实测一份 91 节点的存档里有 61 个是旁路），
 *    直接拒绝会让这个功能对真实文件完全不可用；
 *  - `mode=2`（静音 / never）：节点不执行、也不出现在提示词里，下游会缺输入 —— **警告**如实列出。
 *
 * 不能（**一律报错，绝不猜**）：只用前端 JS 实现的"广播型"虚拟节点，典型的是
 * `Anything Everywhere`（把一路输入广播到全图所有同类型的空输入上）。ComfyUI 的官方前端在排队前
 * 自己会把它们改写掉，服务端拿不到那份 JS 也不该照猫画虎 —— 硬猜的后果是 ComfyUI 报一堆
 * 莫名其妙的节点错误，用户根本查不出来，所以这里宁可失败并说清楚出口在哪。
 */
object WorkflowConvert {

    /**
     * 转换结果。
     *
     * [unsupportedNodes] 非空时**不许拿去提交** —— 调用方要如实报错。
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
        val rawNodes = (ui["nodes"] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
        if (rawNodes.isEmpty()) throw IllegalArgumentException("工作流里一个节点都没有（nodes 是空的）")

        val links = parseLinks(ui)
        // 改写表：(节点id, 输出槽位) → 真正的上游 (节点id, 槽位)。Reroute / Set/Get / 旁路都登记在这里。
        val redirects = mutableMapOf<Pair<Int, Int>, Pair<Int, Int>>()
        val out = LinkedHashMap<String, JsonElement>()
        val rewritten = mutableListOf<String>()
        val unsupported = mutableListOf<String>()
        val warnings = mutableListOf<String>()

        // --- 第一遍：登记所有可等价改写的界面节点（必须在解析连线之前全部登记完） ---
        val setByName = mutableMapOf<String, Pair<Int, Int>>()
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
            val id = node.int("id") ?: return@forEach
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
            val id = node.int("id") ?: return@forEach
            val cls = node.uiClass()
            bypassRedirects(id, node, links, redirects, warnings)
            rewritten += "$id $cls（旁路）"
        }

        // --- 第二遍：真正转成节点 ---
        val redirectedIds = redirects.keys.map { it.first }.toSet()
        for (node in rawNodes) {
            val id = node.int("id") ?: continue
            val cls = node.uiClass()
            if (cls.isEmpty()) continue
            val mode = node.int("mode") ?: 0
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
                inputs[name] = JsonArray(listOf(JsonPrimitive(source.first), JsonPrimitive(source.second)))
            }
            // 2) 控件值：按 /object_info 的声明顺序贴回名字上
            fillWidgets(node, widgetSpecs(info), inputs, warnings, id, cls)

            out[id.toString()] = buildJsonObject {
                put("class_type", cls)
                put("inputs", JsonObject(inputs))
                node["title"].asText()?.takeIf { it.isNotBlank() }?.let {
                    put("_meta", buildJsonObject { put("title", it) })
                }
            }
        }

        return Result(
            graph = JsonObject(out),
            nodeCount = out.size,
            rewritten = rewritten,
            warnings = warnings,
            unsupportedNodes = unsupported,
        )
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
        id: Int,
        node: JsonObject,
        links: Map<Long, Pair<Int, Int>>,
        redirects: MutableMap<Pair<Int, Int>, Pair<Int, Int>>,
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
        val source: Pair<Int, Int>,
    )

    /** 谁引用了这个节点的输出（给"静音节点"的警告用）。 */
    private fun consumersOf(id: Int, nodes: List<JsonObject>, links: Map<Long, Pair<Int, Int>>): List<String> {
        val out = mutableListOf<String>()
        nodes.forEach { n ->
            (n["inputs"] as? JsonArray)?.forEach { entry ->
                val e = entry as? JsonObject ?: return@forEach
                val linkId = e["link"].asLong() ?: return@forEach
                if (links[linkId]?.first == id) {
                    out += "${n.int("id") ?: "?"}（${n.uiClass()}）"
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
        id: Int,
        cls: String,
    ) {
        if (specs.isEmpty()) return
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

    /** 一个节点的控件（= 有 widgets_values 槽位的输入）名字与顺序，来自 `/object_info`。 */
    private fun widgetSpecs(info: JsonObject): List<WidgetSpec> {
        val input = info["input"] as? JsonObject ?: return emptyList()
        val out = mutableListOf<WidgetSpec>()
        for (group in listOf("required", "optional")) {
            val g = input[group] as? JsonObject ?: continue
            for ((name, raw) in g) {
                val arr = raw as? JsonArray ?: continue
                val options = arr.getOrNull(1) as? JsonObject
                // forceInput：被强制成连线插口的输入没有控件
                if (options?.get("forceInput").asBool() == true) continue
                val isWidget = when (val typeEl = arr.firstOrNull()) {
                    is JsonArray -> true
                    is JsonPrimitive -> typeEl.contentOrNull?.uppercase() in WIDGET_TYPES
                    else -> false
                }
                if (!isWidget) continue
                out += WidgetSpec(name, options?.get("control_after_generate").asBool() == true)
            }
        }
        return out
    }

    // -----------------------------------------------------------------------
    //  连线
    // -----------------------------------------------------------------------

    /** `links` 表：linkId → (上游节点 id, 上游输出槽位)。新旧两种写法都认。 */
    private fun parseLinks(ui: JsonObject): Map<Long, Pair<Int, Int>> {
        val out = HashMap<Long, Pair<Int, Int>>()
        (ui["links"] as? JsonArray)?.forEach { entry ->
            when (entry) {
                // 老写法：[link_id, origin_id, origin_slot, target_id, target_slot, type]
                is JsonArray -> {
                    val linkId = entry.getOrNull(0).asLong() ?: return@forEach
                    val origin = entry.getOrNull(1).asInt() ?: return@forEach
                    val slot = entry.getOrNull(2).asInt() ?: 0
                    out[linkId] = origin to slot
                }
                // 新写法：{id, origin_id, origin_slot, …}
                is JsonObject -> {
                    val linkId = entry["id"].asLong() ?: return@forEach
                    val origin = entry["origin_id"].asInt() ?: return@forEach
                    val slot = entry["origin_slot"].asInt() ?: 0
                    out[linkId] = origin to slot
                }
                else -> Unit
            }
        }
        return out
    }

    /**
     * 把一个引用一路解到"真正的产出节点"上（穿透 `Reroute` / `Set`/`Get` / 旁路的节点）。
     * 带步数上限，防止工作流里出现环时转不出来。
     */
    private fun resolve(
        ref: Pair<Int, Int>,
        redirects: Map<Pair<Int, Int>, Pair<Int, Int>>,
    ): Pair<Int, Int> {
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
    private fun JsonObject.firstLinkedInput(links: Map<Long, Pair<Int, Int>>): Pair<Int, Int>? {
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
     * 界面格式里节点的类名在 **`type`**，不是 API 格式那个 `class_type`
     * （[JsonObject.cls] 读的是后者）。
     *
     * 这两个键名混用是**踩过的坑**：第一版这里用了 `cls()`，于是每个节点都被当成"没有类名"
     * 直接跳过，转换结果是**一张空图** —— 而空图提交上去只会得到 ComfyUI 一句
     * "Prompt has no outputs"，完全看不出真正原因。用例里因此专门盯着 nodeCount。
     */
    private fun JsonObject.uiClass(): String = this["type"].asText().orEmpty()
}
