package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * 工作流的**待补输入诊断**（用户建议 ⑤，2026-09-20）。
 *
 * ## 用户的原话
 *
 * > "可能要设计工具/skill，让 AI 能够基于基本工作流，摘除无关节点或者拼装必要节点
 * >  （比如这种情况：用户没有提供参考图，但是他引导的工作流的节点用到参考图）"
 *
 * ## 这条实现解决"识别"，不假装解决"改写"
 *
 * 摘节点 / 拼节点这件事，本机**已经有一半**了：`comfy_load_workflow(tolerateUnsupported=true)`
 * 会把转换不了的前端节点摘掉并给出 `openInputs` / `unresolvedInputs`，
 * `comfy_submit(connections=…)` 能把线补上（见系统提示第 15 条）。缺的是另一半 ——
 * **AI 手里有一份能跑的工作流时，怎么知道"这份图还需要用户给点什么"**。
 *
 * 比如用户拿了张文生图工作流却说要"参考这张图改"：工作流里那个 `LoadImage`
 * 要么绑着上次捕获时的某个 jpg（跟你无关的文件），要么压根没连——
 * 模型不看清楚就会硬提交，ComfyUI 那边只回一句缺输入，用户看到的是"AI 说生成了、其实没跑起来"。
 *
 * 所以这个扫描器只做一件事：**把"还等着外部输入的位置"逐个列清楚**，
 * 每一条都带 节点 id / 节点类 / 输入名 / 缺什么类型 / 该去哪儿找。
 * 剩下的判断（要不要问用户、要不要换一条工作流）是模型的事，
 * 而它现在有了事实可依据 —— 这正是"不许瞎猜"的前提。
 *
 * ## 判据（刻意保守）
 *
 * 一个输入算"待补"，当且仅当它：
 *   · **是连线型**（值是数组），或值缺失 / 是空串；
 *   · 且**不属于**模型可控的输出（`OUTPUT_NODE` 的类）；
 *   · 且不是 ComfyUI 的虚拟节点（`Note` / `Reroute` 之类，它们没有真实语义）。
 *
 * 已经填了真实值（文本、数字、下拉默认值）的输入**不算缺口** —— 那是工作流本来的设定，
 * 报告里把它们也算上只会淹掉真正要看的那几条。
 */
object WorkflowInputs {

    /**
     * 连线型输入里，**必须由外部提供**的那些类。
     *
     * 判据取的是位置语义（这个节点在这一格等的是"一张图 / 一段视频"），不是具体节点名：
     * `LoadImage`、`IPAdapter` 的 `image`、`VHS_LoadVideo` 的 `video`、蒙版类都落在这里。
     * 没被认出来的（第三方自定义节点）照样列进 [MissingInput.kind] = "unknown"，
     * **不静默丢弃** —— 列表少一条，模型就会以为那份工作流已经齐了。
     */
    private val IMAGE_INPUTS = setOf("image", "images", "mask", "masks", "image1", "image2", "image_b")
    private val VIDEO_INPUTS = setOf("video", "videos", "frames", "frame_load_cap")

    /** 没有真实语义的虚拟节点：它们不该进"待补输入"清单。 */
    private val VIRTUAL_NODES = setOf("Note", "MarkdownNote", "Reroute", "PrimitiveNode", "GetNode", "SetNode")

    /**
     * 扫一份 **API 格式**节点图（`{"1": {"class_type": "...", "inputs": {...}}, ...}`）。
     *
     * @param graph URL 或路径无关的纯数据：`comfy_get_run` / `comfy_load_workflow` 的
     *   `raw` / `apiGraph` 字段原样传进来即可。
     * @param hasImageAttachment 本次请求是否带了图片附件。带了的话，图片类缺口不算"缺"——
     *   模型手里就有图，下一步是 `comfy_use_attachment` 投放，不该被报成"要用户补"。
     */
    fun scan(graph: JsonObject, hasImageAttachment: Boolean = false): WorkflowInputReport {
        val missing = mutableListOf<MissingInput>()
        val known = mutableListOf<String>()

        graph.forEach { (nodeId, raw) ->
            val node = runCatching { raw.jsonObject }.getOrNull() ?: return@forEach
            val classType = node.str("class_type").orEmpty()
            if (classType.isBlank()) return@forEach
            if (classType in VIRTUAL_NODES) return@forEach

            val inputs = runCatching { node["inputs"]?.jsonObject }.getOrNull() ?: return@forEach
            val title = node["_meta"]?.let { runCatching { it.jsonObject }.getOrNull() }?.str("title")

            inputs.forEach { (name, raw) ->
                // 捕获成局部变量：下面的判据会在 lambda 里用，直接引用 `value` 参数会拿不到
                val value: JsonElement? = raw
                val gap = gapOf(classType, name, value) ?: return@forEach
                if (gap == InputKind.image && hasImageAttachment) {
                    // 本次带了图：图片缺口由 comfy_use_attachment 投放解决，
                    // 记成"已知道"而不是"要用户补" —— 两条混在一起模型就会去问用户要图。
                    known += "$nodeId.$name（本次附件可以投放）"
                    return@forEach
                }
                missing += MissingInput(
                    nodeId = nodeId,
                    classType = classType,
                    inputName = name,
                    kind = gap,
                    title = title,
                    currentValue = describeValue(value),
                    hint = hintFor(gap),
                )
            }
        }

        return WorkflowInputReport(
            missing = missing,
            suppliedByAttachment = known,
            nodeCount = graph.size,
        )
    }

    /**
     * 这个输入算不算"还等着外部给"。
     *
     * 返回 null = 不用管（有真实值、或是输出节点/模型可控的东西）。
     */
    private fun gapOf(classType: String, name: String, value: JsonElement?): InputKind? {
        // 输出节点（SaveImage / PreviewImage…）是"结果去哪儿"，不是"等谁给输入"
        if (classType.contains("Save") || classType.contains("Preview")) return null

        val isWire = value is JsonArray
        val isEmpty = value == null ||
            value is JsonNull ||
            (value is JsonPrimitive && value.isString && value.content.isBlank())

        // LoadImage 的 image：就算绑着上次捕获的那个 jpg 也是一条"你要自己决定用哪张"的缺口。
        // 这是用户建议 ⑤ 那个现场的核心一条 —— 不报出来，模型就会硬提交一个不存在的文件。
        if (classType == "LoadImage" && name == "image") return InputKind.image
        if (classType.startsWith("VHS_LoadVideo") && name in VIDEO_INPUTS) return InputKind.video

        if (!isWire && !isEmpty) return null

        // 连线型输入空着，或者值缺失：看名字判它等的是什么
        return when {
            name.lowercase() in IMAGE_INPUTS -> InputKind.image
            name.lowercase() in VIDEO_INPUTS -> InputKind.video
            isWire -> InputKind.connection
            else -> InputKind.parameter
        }
    }

    private fun hintFor(kind: InputKind): String = when (kind) {
        InputKind.image ->
            "缺一张图。用户给了附件就走 comfy_use_attachment 投放拿 filename，" +
                "再覆盖这个输入；用户**没给图**时不要硬提交（ComfyUI 只会报缺输入）——" +
                "问用户要一张，或换一条不需要参考图的工作流。"
        InputKind.video ->
            "缺一段视频。本机没有「投放视频」的工具：必须让用户自己在 ComfyUI 里选好，或换工作流。"
        InputKind.connection ->
            "这个输入是**连线**，现在空着。要么照 openInputs 补线" +
                "（comfy_submit 的 connections），要么就是这份图还没转完 —— 不要猜着接一根线上去。"
        InputKind.parameter ->
            "这个输入没有值。多数是控件型输入：覆盖成用户要的参数即可，不确定就问用户。"
    }

    private fun describeValue(value: JsonElement?): String? = when {
        value == null || value is JsonNull -> null
        value is JsonArray ->
            if (value.isEmpty()) "空连线" else "连线 → ${value.jsonArray.joinToString { it.toString() }}"
        value is JsonPrimitive && value.isString && value.content.isBlank() -> "空字符串"
        else -> value.toString().take(120)
    }
}

/** 待补输入的种类 —— 决定"下一步该干什么"，所以单独列出来。 */
enum class InputKind(val wire: String) {
    /** 等一张图（附件投放 / 问用户要） */
    image("image"),

    /** 等一段视频（本机没有投放通道） */
    video("video"),

    /** 连线型输入空着（照 openInputs 补线，别猜） */
    connection("wire"),

    /** 控件型输入没值（覆盖参数或问用户） */
    parameter("value"),
}

/** 一条待补输入。 */
data class MissingInput(
    val nodeId: String,
    val classType: String,
    val inputName: String,
    val kind: InputKind,
    val title: String?,
    val currentValue: String?,
    val hint: String,
)

/** 一次扫描的结果。 */
data class WorkflowInputReport(
    val missing: List<MissingInput>,
    /** 本次附件就能解决的位置（不列进"要用户补"）。 */
    val suppliedByAttachment: List<String>,
    val nodeCount: Int,
) {
    val isEmpty: Boolean get() = missing.isEmpty()

    /** 有没有"必须用户出手"的（图 / 视频）：这两类硬提交必然失败。 */
    val hasExternalAssetGap: Boolean
        get() = missing.any { it.kind == InputKind.image || it.kind == InputKind.video }

    fun toJson(): JsonObject = buildJsonObject {
        put("nodeCount", nodeCount)
        put("missingCount", missing.size)
        put("needsExternalAsset", hasExternalAssetGap)
        put(
            "missing",
            buildJsonArray {
                missing.forEach { m ->
                    add(
                        buildJsonObject {
                            put("nodeId", m.nodeId)
                            put("classType", m.classType)
                            put("inputName", m.inputName)
                            put("kind", m.kind.wire)
                            m.title?.let { put("title", it) }
                            m.currentValue?.let { put("currentValue", it) }
                            put("hint", m.hint)
                            put("ref", "${m.nodeId}.${m.inputName}")
                        },
                    )
                }
            },
        )
        if (suppliedByAttachment.isNotEmpty()) {
            put(
                "suppliedByAttachment",
                buildJsonArray { suppliedByAttachment.forEach { add(JsonPrimitive(it)) } },
            )
        }
    }

    /**
     * 给模型看的一小段：**每条一行**，带头部结论。
     *
     * 结论先行（"这份工作流还等着外部给输入"）是有意的：模型最容易漏掉的正是这一句，
     * 而它决定了后面该问用户还是该直接提交。
     */
    fun render(): String {
        if (missing.isEmpty()) {
            return buildString {
                append("这份工作流没有待补输入（").append(nodeCount)
                append(" 个节点，参数都有值或都已连线）—— 可以直接提交。")
                // 全部缺口都由本次附件解决时也要说一句：否则模型会以为"什么都没有"，
                // 转头又把 LoadImage 那个绑死的旧文件名当成能用的东西
                appendAttachmentNote(this)
            }
        }
        val asset = if (hasExternalAssetGap) "其中**有图 / 视频类的缺口**：不解决就会提交失败。" else ""
        return buildString {
            append("这份工作流有 ").append(missing.size).append(" 处**等着外部输入**：")
            if (asset.isNotEmpty()) append(asset)
            append('\n')
            missing.forEach { m ->
                append("· ").append(m.nodeId).append('.').append(m.inputName)
                append("（").append(m.classType)
                m.title?.let { append(" / ").append(it) }
                append("，缺 ").append(m.kind.wire)
                m.currentValue?.let { append("，现在：").append(it) }
                append("）\n  ").append(m.hint).append('\n')
            }
            appendAttachmentNote(this)
        }
    }

    /** 附件能解决的那些位置（有才写）。 */
    private fun appendAttachmentNote(sb: StringBuilder) {
        if (suppliedByAttachment.isEmpty()) return
        sb.append("（以下位置本次附件可以解决，不必问用户：")
        sb.append(suppliedByAttachment.joinToString("、")).append("）\n")
    }
}

/**
 * 从工具结果里挑出"API 节点图"那一段。
 *
 * 三种来源共用（`comfy_get_run.raw`、`comfy_load_workflow.apiGraph`、
 * `comfy_find_workflow` 的结果），所以判据只有一条：**是个对象、且值是带 class_type 的对象**。
 * 认不出来就返回 null —— 宁可说"给的那段不是节点图"，也不要对着一份界面格式硬扫。
 */
fun apiGraphOf(element: JsonElement?): JsonObject? {
    val obj = when (element) {
        is JsonObject -> element
        is JsonPrimitive -> element.contentOrNull
            ?.let { runCatching { AppJson.parseToJsonElement(it) as? JsonObject }.getOrNull() }
        else -> null
    } ?: return null
    if (obj.isEmpty()) return null
    val looksLikeGraph = obj.values.all { v ->
        val n = runCatching { v.jsonObject }.getOrNull() ?: return@all false
        n["class_type"] is JsonPrimitive
    }
    return if (looksLikeGraph) obj else null
}

/** 图里所有节点 id（补线时要照它核对"接的是不是真实存在的节点"）。 */
internal fun graphNodeIds(graph: JsonObject): List<String> =
    graph.keys.filter { id ->
        runCatching { graph[id]?.jsonObject?.get("class_type") }.getOrNull() is JsonPrimitive
    }

/** 一个节点在 API 图里声明的输入名（覆盖参数 / 补线时用得上）。 */
internal fun graphInputNames(graph: JsonObject, nodeId: String): List<String> =
    runCatching { graph[nodeId]?.jsonObject?.get("inputs")?.jsonObject?.keys }
        .getOrNull()?.toList().orEmpty()

/** `intOrNull` 的转发：本包别处取值习惯用 `obj.int(...)`，这里保持一致的可读性。 */
internal fun JsonObject.intOrNullField(field: String): Int? =
    (this[field] as? JsonPrimitive)?.intOrNull
