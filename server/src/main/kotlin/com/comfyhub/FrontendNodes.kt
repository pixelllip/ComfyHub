package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put

/**
 * ## 模拟用户行为 ##（用户建议 ②，2026-09-21）
 *
 * 用户的原话是：「加入 ##模拟用户行为## 模式，面对一些**仅面向前端的工作流节点**，
 * 让 AI 有能力在前端读取、编辑、在 comfy 前端提交」。
 *
 * 和真正的浏览器自动化不同（用户当面确认过走**后端 + ComfyUI HTTP API**这条线，
 * 不引入浏览器），这里做的是**把前端 JS 在排队前做的那几件事，在后端等价地做掉**：
 * ComfyUI 官方前端在 `graphToPrompt()` 之后、`POST /prompt` 之前，会调用各扩展的
 * `beforeRegisterNodeDef` / `onExecuted` 钩子去改写节点图。这一步在服务端是拿不到的
 * （那是浏览器里的 JS），所以过去我们只能如实报 `UNSUPPORTED_NODES`。
 *
 * 本文件只做**判据确定、可验证**的那几类，绝不照猫画虎：
 *
 * | 前端节点 | 前端自己干了什么 | 这里怎么等价改写 |
 * | --- | --- | --- |
 * | `Anything Everywhere` 一族（含 `?` / `!` / `Prompts` / `Seed` / 数字后缀变体） | 把上游那一路值**广播**到全图所有同类型、且没接线的输入上 | 按它声明的类型 / 名字过滤**填空**，并如实报告灌了哪几条线 |
 * | `PreviewImage` / `PreviewAudio` / `PreviewVideo` / `Preview3D` / `ShowText` / `Display Any` … | 只是"看一眼"的显示节点，不产出模型要的东西 | 丢掉（与官方前端一致），如实报告 |
 * | 没有任何输出节点 | 前端**自动挂一个 `SaveImage`**（`filename_prefix = ComfyUI`） | 补一个 `SaveImage` 接上同一个 IMAGE 来源 |
 *
 * 判据一律**保守**：说不清类型的广播不猜，照旧报 `unsupported`（出口还是那一套：
 * 用 `comfy_submit(connections=…)` 点名补线，或者在 ComfyUI 里导出 API 格式）。
 *
 * 这就是 `docs/ai-tools-and-skills.md` 里「模拟用户行为」那一节说的边界 ——
 * **改这里之前先读那一段，别把"猜语义"的手伸进来**。
 */
object FrontendNodes {

    /** 一次改写的总结（进工具结果，给模型和用户看"到底动了哪几根线"）。 */
    data class Emulation(
        /** 已经等价改写 / 丢掉的前端节点，形如 `12 Anything Everywhere（广播）` */
        val rewritten: List<String> = emptyList(),
        /** 广播灌进去的线，形如 `1.positive ← 节点 14（Anything Everywhere）` */
        val broadcast: List<String> = emptyList(),
        /** 自动补的输出节点（前端在"一个输出都没有"时也会这么做） */
        val injected: List<String> = emptyList(),
        /** 仍然改写不了的前端节点，形如 `37 Anything Everywhere` */
        val unsupported: List<String> = emptyList(),
        /** 已经等价改写 / 丢掉的前端节点 id（调用方据此从 `unsupportedNodes` 里摘出去）。 */
        val handledIds: List<String> = emptyList(),
    )

    /**
     * 只用前端 JS 实现、服务端没有节点定义的**虚拟节点**。
     *
     * 判定顺序必须是：**先看 `/object_info` 里有没有**（有的版本把其中一些做成了真节点，
     * 那时它就该原样保留），只有"查不到 + 名字在这张表里"才走这里。
     */
    private val BROADCAST_EXACT = setOf(
        "Anything Everywhere", "Anything Everywhere?",
        "Anything Everywhere3", "Anything Everywhere3?",
        "Anything Everywhere4", "Anything Everywhere4?",
        "Anything Everywhere5", "Anything Everywhere5?",
        "Prompts Everywhere", "Seed Everywhere",
    )

    /** 只负责"看一眼"的显示节点：丢掉安全（官方前端也不把它们写进提示词）。 */
    private val DISPLAY_VIRTUAL = setOf(
        "PreviewImage", "PreviewAudio", "PreviewVideo", "Preview3D", "PreviewMask",
        "ShowText", "ShowText|pysssss", "Display Any", "DisplayAny",
        "DisplayString", "Display Int", "Display Float",
    )

    /** 名字里带这些片段、且 `/object_info` 查不到的，当成广播型（`Anything Everywhere (3)` 这类变体）。 */
    private val BROADCAST_PATTERNS = listOf(
        Regex("^Anything\\s*Everywhere", RegexOption.IGNORE_CASE),
        Regex("^AnythingEverywhere", RegexOption.IGNORE_CASE),
        Regex("Everywhere\\b", RegexOption.IGNORE_CASE),
    )

    /** 名字里带这些片段、且 `/object_info` 查不到的，当成显示节点。 */
    private val DISPLAY_PATTERNS = listOf(
        Regex("^Preview[A-Z]", RegexOption.IGNORE_CASE),
        Regex("^Show(Text|Anything|String|Int|Float|Image)", RegexOption.IGNORE_CASE),
        Regex("^Display\\s?(Any|String|Int|Float|Text|Value)", RegexOption.IGNORE_CASE),
    )

    /** 这个类是不是"仅前端"的广播型虚拟节点。 */
    fun isBroadcast(cls: String): Boolean =
        cls in BROADCAST_EXACT || BROADCAST_PATTERNS.any { it.containsMatchIn(cls) }

    /** 这个类是不是"仅前端"的显示型虚拟节点。 */
    fun isDisplay(cls: String): Boolean =
        cls in DISPLAY_VIRTUAL || DISPLAY_PATTERNS.any { it.containsMatchIn(cls) }

    /**
     * 全图的**连出来**的输入（`节点id.输入名`），按上游类型分组。
     *
     * 广播型节点就是往这些地方灌值的 —— 它们没接线，转换出来之后是"空着的插口"。
     */
    data class OpenInput(
        val nodeId: String,
        val name: String,
        /** `/object_info` 声明的可接受类型（空 = 说不清，不参与广播）。 */
        val types: List<String>,
    )

    /**
     * 对一份**已经转换好**的 API 节点图做前端等价改写。
     *
     * @param rawNodes 界面格式里的原始节点（找广播/显示节点要靠它，转出来的图里已经没有它们了）
     * @param objectInfo ComfyUI 的 `/object_info`
     * @param graph `WorkflowConvert.toApiGraph` 转出来的图（**会被复制**，不改原对象）
     * @param links linkId → (上游节点 id, 输出槽位)
     * @return 改写后的图 + 一份"动了什么"的清单
     */
    fun emulate(
        rawNodes: List<JsonObject>,
        objectInfo: JsonObject,
        graph: Map<String, JsonElement>,
        links: Map<Long, Pair<String, Int>>,
    ): Pair<Map<String, JsonElement>, Emulation> {
        val out = LinkedHashMap(graph)
        val rewritten = mutableListOf<String>()
        val broadcast = mutableListOf<String>()
        val injected = mutableListOf<String>()
        val unsupported = mutableListOf<String>()

        val broadcasters = rawNodes.filter { node ->
            val cls = node.uiClass()
            cls.isNotEmpty() && objectInfo[cls] == null && isBroadcast(cls)
        }
        val displays = rawNodes.filter { node ->
            val cls = node.uiClass()
            cls.isNotEmpty() && objectInfo[cls] == null && isDisplay(cls)
        }

        // 空输入只算一次；每灌一条线就记进 filled，后面的广播不会再重复灌同一个位置
        val allOpen = openInputs(objectInfo, out)
        val filled = mutableSetOf<Pair<String, String>>()

        // --- ① 广播：把上游那一路填进所有"同类型且空着"的输入 ---
        broadcasters.forEach { node ->
            val id = node.idOf() ?: return@forEach
            val cls = node.uiClass()
            val source = broadcastSource(node, links)?.let { resolveFor(out, it) }
            if (source == null) {
                // 它自己都没接上游：没有任何东西可广播（前端这时候也不会灌值）
                unsupported += "$id $cls（没有接上游，没有东西可以广播）"
                return@forEach
            }
            val spec = broadcastSpec(node)
            val targets = allOpen.filter { (it.nodeId to it.name) !in filled }
            val hits = targets.filter { t -> matches(spec, t) }
            if (hits.isEmpty()) {
                unsupported += "$id $cls（${spec.describe()}：全图没有对得上的空输入）"
                return@forEach
            }
            hits.forEach { t ->
                val target = (out[t.nodeId] as? JsonObject) ?: return@forEach
                val inputs = LinkedHashMap<String, JsonElement>(
                    (target["inputs"] as? JsonObject)?.toMap() ?: emptyMap(),
                )
                inputs[t.name] = JsonArray(listOf(refOf(source.first), JsonPrimitive(source.second)))
                out[t.nodeId] = buildJsonObject {
                    target.forEach { (k, v) -> if (k != "inputs") put(k, v) }
                    put("inputs", JsonObject(inputs))
                }
                filled += t.nodeId to t.name
                broadcast += "${t.nodeId}.${t.name} ← 节点 $id（$cls）"
            }
            rewritten += "$id $cls（广播 ${hits.size} 处）"
        }

        // --- ② 显示节点：丢掉（官方前端也不把这类写进提示词） ---
        displays.forEach { node ->
            val id = node.idOf() ?: return@forEach
            rewritten += "$id ${node.uiClass()}（显示节点，已忽略）"
        }

        // --- ③ 一个输出节点都没有：前端会自动挂 SaveImage，这里照做 ---
        // 只在"原本一张图都存不出来"时补，绝不覆盖用户自己的工作流（那会多写文件）。
        // 接哪一路？**按类型找产出 IMAGE 的节点**（不能看输入名：VAEDecode 的输出压根不在
        // 它的 `inputs` 里，它是把 `samples` 解成图的那个节点）。
        if (out.isNotEmpty() && out.values.none { it.isSaveLikeNode() }) {
            val imageSource = out.entries.firstNotNullOfOrNull { (nodeId, value) ->
                val node = value as? JsonObject ?: return@firstNotNullOfOrNull null
                if (!produces(objectInfo, node, "IMAGE")) return@firstNotNullOfOrNull null
                JsonArray(listOf(refOf(nodeId), JsonPrimitive(0)))
            }
            if (imageSource != null) {
                val newId = freshNodeId(out.keys)
                out[newId] = buildJsonObject {
                    put("class_type", "SaveImage")
                    put("inputs", buildJsonObject {
                        put("images", imageSource)
                        put("filename_prefix", JsonPrimitive("ComfyHub"))
                    })
                }
                injected += "$newId SaveImage（原本没有输出节点，照前端的做法补一个）"
            }
        }

        return out to Emulation(
            rewritten = rewritten,
            broadcast = broadcast,
            injected = injected,
            unsupported = unsupported,
            handledIds = (broadcasters + displays).mapNotNull { it.idOf() },
        )
    }

    /** 供 `WorkflowConvert` 在广播改写之后重算"还空着哪些连线型输入"。 */
    fun openInputsOf(objectInfo: JsonObject, graph: Map<String, JsonElement>): List<String> =
        openInputs(objectInfo, graph).map { "${it.nodeId}.${it.name} (${it.types.joinToString("/")})" }

    /**
     * 广播的目标类型 / 名字过滤。
     *
     * `Anything Everywhere` 把自己的设定存在节点的 `properties` 里
     * （`"Input Type"` / `"Input Name"` / `"Prefix"` / `"Filter"`），不同版本键名不一样，
     * 所以这里逐个认；一个都认不出来时**返回 null 类型** = 不猜，由调用方报 unsupported。
     */
    private data class BroadcastSpec(
        val type: String?,
        val nameFilter: String?,
        val groupMode: String?,
        val minCount: Int?,
    ) {
        fun describe(): String = buildString {
            append("类型 ")
            append(type ?: "未声明")
            nameFilter?.let { append("，名字含「").append(it).append("」") }
            groupMode?.let { append("，分组 ").append(it) }
            minCount?.let { append("，至少 ").append(it).append(" 处") }
        }
    }

    private fun broadcastSpec(node: JsonObject): BroadcastSpec {
        val props = node["properties"] as? JsonObject
        fun prop(vararg keys: String): String? {
            for (k in keys) {
                val v = props?.get(k) ?: continue
                if (v is JsonNull) continue
                val text = (v as? JsonPrimitive)?.contentOrNull?.trim()
                if (!text.isNullOrEmpty()) return text
            }
            return null
        }
        val rawType = prop("Input Type", "inputType", "Type", "type")
        // 前端把"不限类型"写成 `*` / `*?` / `anything`
        val type = rawType?.takeIf { it != "*" && it != "*?" && !it.equals("anything", true) }
        return BroadcastSpec(
            type = type,
            nameFilter = prop("Input Name", "inputName", "Prefix", "Filter", "filter"),
            groupMode = prop("Group Mode", "group_mode", "Mode", "mode"),
            minCount = prop("Broadcast", "broadcast")?.toIntOrNull(),
        )
    }

    /** 这条空输入接不接受这次广播。 */
    private fun matches(spec: BroadcastSpec, target: OpenInput): Boolean {
        // 名字过滤优先（Prompts Everywhere 就是靠它只灌提示词输入）
        if (spec.nameFilter != null) {
            if (!target.name.contains(spec.nameFilter, ignoreCase = true)) return false
            // 带了过滤词就不再要求类型对得上：过滤词本身就是更强的约束
            return true
        }
        val type = spec.type ?: return false
        return target.types.any { it.equals(type, ignoreCase = true) }
    }

    /** 广播节点的上游：它自己的那个连线型输入接的是谁。 */
    private fun broadcastSource(node: JsonObject, links: Map<Long, Pair<String, Int>>): Pair<String, Int>? {
        (node["inputs"] as? JsonArray)?.forEach { entry ->
            val e = entry as? JsonObject ?: return@forEach
            val linkId = linkIdOf(e) ?: return@forEach
            links[linkId]?.let { return it }
        }
        // 有的版本写成 `widgets_values` 里的上游名字，认不出来就不广播（不猜）
        return null
    }

    private fun linkIdOf(e: JsonObject): Long? {
        val raw = e["link"] ?: return null
        val text = (raw as? JsonPrimitive)?.contentOrNull ?: return null
        return text.toLongOrNull()
    }

    /**
     * 穿透 Reroute / 旁路 / 组节点输出留下的改写表，拿到真正的产出节点。
     *
     * 广播是在**转完之后**做的，改写表已经不在手上了，所以这里只在图里找：
     * 图里没有那个 id 就一直顺着"输出改写"的痕迹找 —— 找不到就如实放弃。
     */
    private fun resolveFor(graph: Map<String, JsonElement>, ref: Pair<String, Int>): Pair<String, Int>? {
        if (graph.containsKey(ref.first)) return ref
        // 摘掉的前端节点留下的悬挂引用：图里没有就不能广播（前端也灌不出值）
        return null
    }

    /** 现在**还空着**的连线型输入（转出来的图里没有这个键 = 空着）。 */
    private fun openInputs(
        objectInfo: JsonObject,
        graph: Map<String, JsonElement>,
    ): List<OpenInput> {
        val out = mutableListOf<OpenInput>()
        graph.forEach { (nodeId, value) ->
            val node = value as? JsonObject ?: return@forEach
            val cls = (node["class_type"] as? JsonPrimitive)?.contentOrNull ?: return@forEach
            // 广播节点自己不能进图（`/object_info` 里查得到才进得来），所以这里只认有定义的类
            val info = objectInfo[cls] as? JsonObject ?: return@forEach
            val input = info["input"] as? JsonObject ?: return@forEach
            val present = (node["inputs"] as? JsonObject)?.keys ?: emptySet()
            for (group in listOf("required", "optional")) {
                val g = input[group] as? JsonObject ?: continue
                for ((name, raw) in g) {
                    if (name in present) continue
                    val arr = raw as? JsonArray ?: continue
                    // 控件型（含 COMBO）空着时 ComfyUI 用默认值，不是缺口；forceInput 是插口
                    if (isWidgetSpec(arr)) continue
                    val accepted = acceptedTypes(arr)
                    if (accepted.isEmpty()) continue
                    out += OpenInput(nodeId, name, accepted)
                }
            }
        }
        return out
    }

    /** `/object_info` 里一个输入声明的可接受类型（`["MODEL"]` / `[[…]]` / `["INT",{…}]`）。 */
    private fun acceptedTypes(spec: JsonArray): List<String> {
        val head = spec.firstOrNull() ?: return emptyList()
        return when (head) {
            is JsonArray -> emptyList() // COMBO 之类的枚举 = 控件型，上面已经挡掉
            is JsonPrimitive -> {
                val s = head.contentOrNull?.trim().orEmpty()
                // 以 `*` 结尾的老写法（`"MODEL,CLIP"` / `"*"`）在 /object_info 里很少见，认不出来就不猜
                if (s.isEmpty() || s.startsWith("*")) emptyList() else listOf(s)
            }
            else -> emptyList()
        }
    }

    private fun isWidgetSpec(spec: JsonArray): Boolean {
        val head = (spec.firstOrNull() as? JsonPrimitive)?.contentOrNull ?: return true
        // forceInput 明确说"这是插口"（STRING 也能是插口）
        val opts = spec.getOrNull(1) as? JsonObject
        if (opts?.get("forceInput")?.let { (it as? JsonPrimitive)?.booleanOrNull } == true) return false
        return head in WIDGET_TYPES
    }

    /**
     * 这个节点会不会产出一条 `IMAGE`。
     *
     * 判据按可靠性分三级（**只在能确定时才认**）：
     *  1. `/object_info` 的 `output` 声明里有 IMAGE（最准）；
     *  2. 没有 `output` 声明时，退回"类名像解码 / 合成图"的那几个（`VAEDecode` / `SaveImage` …）；
     *  3. 都认不出来 = 不认（宁可少补一个输出节点，也不能把错的线接上去）。
     */
    private fun produces(objectInfo: JsonObject, node: JsonObject, type: String): Boolean {
        val cls = (node["class_type"] as? JsonPrimitive)?.contentOrNull ?: return false
        val info = objectInfo[cls] as? JsonObject
        val declared = info?.get("output") as? JsonArray
        if (declared != null) {
            return declared.any {
                ((it as? JsonPrimitive)?.contentOrNull ?: "").equals(type, ignoreCase = true)
            }
        }
        return if (type == "IMAGE") IMAGE_MAKERS.any { cls.startsWith(it) } else false
    }

    private val IMAGE_MAKERS = listOf("VAEDecode", "ImageScale", "ImageBlend", "ImageComposite")

    private fun JsonElement.isSaveLikeNode(): Boolean {        val cls = ((this as? JsonObject)?.get("class_type") as? JsonPrimitive)?.contentOrNull ?: return false
        return cls.startsWith("Save") || cls.startsWith("Preview")
    }

    private fun freshNodeId(existing: Set<String>): String {
        var n = (existing.mapNotNull { it.toIntOrNull() }.maxOrNull() ?: 0) + 1
        while (existing.contains(n.toString())) n++
        return n.toString()
    }

    private fun refOf(id: String): JsonElement =
        id.toLongOrNull()?.let { JsonPrimitive(it) } ?: JsonPrimitive(id)

    private val WIDGET_TYPES = setOf("INT", "FLOAT", "STRING", "BOOLEAN", "COMBO")
}
