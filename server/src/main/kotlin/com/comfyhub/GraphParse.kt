package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull

/**
 * 从 ComfyUI 的 **API 格式节点图**里把「提示词 + 参数」抽出来。
 *
 * API 格式长这样（节点 id -> {class_type, inputs}，引用是 ["节点id", 输出槽位]）：
 * ```json
 * { "3": { "class_type": "KSampler",
 *          "inputs": { "seed": 1, "steps": 20, "cfg": 7.0,
 *                      "sampler_name": "euler", "scheduler": "normal",
 *                      "positive": ["6", 0], "negative": ["7", 0],
 *                      "model": ["4", 0], "latent_image": ["5", 0] } } }
 * ```
 *
 * 这里不硬编码某一个工作流，而是**顺着引用把采样链走一遍**。实测过的两大类结构：
 *
 * 1. 经典 `KSampler`：steps / cfg / seed / sampler_name / scheduler 都在它自己身上，
 *    `positive` / `negative` 指向 `CLIPTextEncode`，`model` 指回 CheckpointLoader。
 * 2. 自定义采样链（`SamplerCustomAdvanced` + `BasicGuider` + `BasicScheduler` +
 *    `KSamplerSelect` + `RandomNoise`，MiniMax H3 这类工作流就是）：
 *    参数分散在 `sigmas` / `sampler` / `noise` / `guider` 指向的节点上，
 *    文本提示词在条件节点（如 `MiniMaxH3ReferenceToVideo` 的 `prompt`）里。
 *
 * 两条路都走不通时再退回"全图扫描"的启发式，尽量不让字段空着。
 */
object GraphParse {

    /** 解析结果；字段与 `prompts` 表一一对应 */
    data class Parsed(
        val kind: String = "IMAGE",
        val positive: String = "",
        val negative: String? = null,
        val checkpoint: String? = null,
        val loras: List<LoraRef> = emptyList(),
        val sampler: String? = null,
        val scheduler: String? = null,
        val steps: Int? = null,
        val cfg: Double? = null,
        val seed: Long? = null,
        val width: Int? = null,
        val height: Int? = null,
        val batch: Int? = null,
        /** 其它散落参数，键名形如 `KSampler.denoise` / `ResolutionSelector.aspect_ratio` */
        val extra: Map<String, String> = emptyMap(),
        /** 建议的标题前缀（取自保存节点的 filename_prefix） */
        val titleHint: String? = null,
    )

    private val VIDEO_CLASS_HINTS = listOf(
        "videocombine", "savevideo", "savewebm", "createvideo", "saveanimatedwebp",
        "vhs_", "wanvideo", "saveanimatedpng", "svd", "imgtovideo", "minimaxh3"
    )
    private val AUDIO_CLASS_HINTS = listOf(
        "saveaudio", "previewaudio", "audioupload", "saveaudiomp3", "voiceclone"
    )

    /** 节点上哪些标量输入算"提示词正文" */
    private val TEXT_INPUT_KEYS = listOf(
        "text", "prompt", "positive", "positive_prompt", "system_prompt",
        "text_g", "text_l", "caption", "prompt_text", "instruction"
    )

    private val NEGATIVE_HINT_KEYS = listOf("negative", "negative_prompt", "neg")

    /** 继续下钻时认这些"条件/文本"转发键 */
    private val CHAIN_KEYS = listOf("conditioning", "cond", "positive", "negative", "clip", "prompt")

    /** 已经单独入库的字段，不再重复塞进 extra_params */
    private val SKIP_INPUT_KEYS = setOf(
        "text", "prompt", "positive", "negative", "seed", "noise_seed", "steps", "cfg",
        "sampler_name", "scheduler", "width", "height", "batch_size", "ckpt_name", "lora_name",
        "strength_model", "model", "clip", "vae", "latent_image", "samples", "images",
        "filename_prefix", "model_name", "unet_name", "length", "batch", "audio", "video"
    )

    fun parse(graph: JsonObject?): Parsed {
        if (graph == null || graph.isEmpty()) return Parsed()
        val nodes = graph.entries.mapNotNull { (id, v) -> (v as? JsonObject)?.let { id to it } }.toMap()
        if (nodes.isEmpty()) return Parsed()

        val sampler = findSampler(nodes)
        val guider = sampler?.input("guider")?.asRef()?.let { nodes[it] }
            ?: sampler?.input("model")?.asRef()?.let { nodes[it] }?.takeIf { it.cls().contains("Guider") }

        // --- 采样参数：先看采样器自己，再看链上的兄弟节点，最后全图兜底 ---
        val sigmas = sampler?.input("sigmas")?.asRef()?.let { nodes[it] }
        val samplerSelect = sampler?.input("sampler")?.asRef()?.let { nodes[it] }
        val noise = sampler?.input("noise")?.asRef()?.let { nodes[it] }

        val stepsNode = sequenceOf(sampler, sigmas, guider)
            .firstOrNull { it?.input("steps").asInt() != null }
            ?: nodes.values.firstOrNull { it.cls().contains("Scheduler", true) && it.input("steps").asInt() != null }
            ?: nodes.values.firstOrNull { it.input("steps").asInt() != null }

        val samplerName = sampler?.input("sampler_name").asText()
            ?: samplerSelect?.input("sampler_name").asText()
            ?: nodes.values.firstOrNull { it.cls().contains("SamplerSelect", true) }?.input("sampler_name").asText()
            ?: nodes.values.firstOrNull { it.input("sampler_name").asText() != null }?.input("sampler_name").asText()

        val schedulerName = sampler?.input("scheduler").asText()
            ?: sigmas?.input("scheduler").asText()
            ?: nodes.values.firstOrNull { it.input("scheduler").asText() != null }?.input("scheduler").asText()

        val cfg = sampler?.input("cfg").asDouble()
            ?: guider?.input("cfg").asDouble()
            ?: nodes.values.firstOrNull { it.cls().contains("Guider", true) && it.input("cfg").asDouble() != null }?.input("cfg").asDouble()
            ?: nodes.values.firstOrNull { it.input("cfg").asDouble() != null }?.input("cfg").asDouble()

        val seed = sampler?.input("seed").asLong()
            ?: sampler?.input("noise_seed").asLong()
            ?: noise?.input("noise_seed").asLong()
            ?: noise?.input("seed").asLong()
            ?: nodes.values.firstOrNull { it.input("noise_seed").asLong() != null }?.input("noise_seed").asLong()
            ?: nodes.values.firstOrNull { it.input("seed").asLong() != null }?.input("seed").asLong()

        // --- 提示词：优先沿采样器/引导器的条件链找，找不到再退化 ---
        val positive = firstNonBlank(
            textFromNode(sampler, nodes, 0),
            textFromRef(sampler?.input("positive"), nodes, 0),
            textFromRef(guider?.input("conditioning"), nodes, 0),
            textFromRef(guider?.input("positive"), nodes, 0),
            textFromRef(sampler?.input("positive_cond"), nodes, 0),
        ) ?: longestTextInput(nodes)?.second

        val negative = firstNonBlank(
            textFromRef(sampler?.input("negative"), nodes, 0),
            textFromRef(guider?.input("negative"), nodes, 0),
            negativeTextInput(nodes)?.second,
        )

        val (loras, checkpoint) = walkModel(sampler, guider, nodes)
        val dims = resolveDimensions(sampler?.input("latent_image"), nodes)

        return Parsed(
            kind = detectKind(nodes),
            positive = positive.orEmpty(),
            negative = negative?.takeIf { it.isNotBlank() && it != positive },
            checkpoint = checkpoint ?: findCheckpoint(nodes),
            loras = loras,
            sampler = samplerName,
            scheduler = schedulerName,
            steps = stepsNode?.input("steps").asInt(),
            cfg = cfg,
            seed = seed,
            width = dims?.first,
            height = dims?.second,
            batch = dims?.third,
            extra = collectExtra(nodes),
            titleHint = findFilenamePrefix(nodes),
        )
    }

    // -----------------------------------------------------------------------
    //  采样器定位
    // -----------------------------------------------------------------------

    private fun findSampler(nodes: Map<String, JsonObject>): JsonObject? {
        val values = nodes.values
        // 经典 KSampler：steps + cfg 都在自己身上
        values.firstOrNull { it.input("steps").asInt() != null && it.input("cfg").asDouble() != null }?.let { return it }
        // 自定义采样链的根节点：有 sigmas 且（有 noise 或 guider）
        values.firstOrNull { it.input("sigmas") != null && (it.input("noise") != null || it.input("guider") != null) }?.let { return it }
        values.firstOrNull { it.cls().contains("SamplerCustom", true) }?.let { return it }
        // 注意 KSamplerSelect 也含 "KSampler"，但它只有 sampler_name，别优先选它
        values.firstOrNull { it.cls().contains("KSampler", true) && it.input("sampler_name") == null }?.let { return it }
        values.firstOrNull { it.cls().contains("KSampler", true) }?.let { return it }
        return values.firstOrNull { it.cls().contains("Sampler", true) }
    }

    /** 顺着 model 引用往回走，收集 LoRA 链和模型名 */
    private fun walkModel(
        sampler: JsonObject?,
        guider: JsonObject?,
        nodes: Map<String, JsonObject>,
    ): Pair<List<LoraRef>, String?> {
        val loras = LinkedHashMap<String, LoraRef>()
        var checkpoint: String? = null

        val starts = listOfNotNull(
            sampler?.input("model")?.asRef(),
            guider?.input("model")?.asRef(),
            sampler?.input("guider")?.asRef(),
        )

        for (start in starts) {
            var ref: String? = start
            var guard = 0
            while (ref != null && guard++ < 40) {
                val node = nodes[ref] ?: break
                val cls = node.cls()

                if (cls.contains("Lora", true)) {
                    val raw = node.input("lora_name") ?: node.input("lora") ?: node.input("lora_path")
                    val names = when (raw) {
                        is JsonPrimitive -> listOfNotNull(raw.contentOrNull)
                        is JsonArray -> raw.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                        else -> emptyList()
                    }
                    val weight = node.input("strength_model").asDouble()
                        ?: node.input("strength").asDouble()
                        ?: node.input("weight").asDouble()
                        ?: 1.0
                    names.filter { it.isNotBlank() }.forEach { name ->
                        val short = name.substringAfterLast('/').substringAfterLast('\\')
                        loras.putIfAbsent(short, LoraRef(short, weight))
                    }
                }

                if (checkpoint == null) {
                    for (key in listOf("ckpt_name", "model_name", "unet_name", "gguf_name", "diffusion_model")) {
                        node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { checkpoint = it; break }
                    }
                }

                ref = node.input("model").asRef()
                    ?: node.input("MODEL").asRef()
                    ?: node.input("guider")?.asRef()?.let { nodes[it] }?.input("model")?.asRef()
            }
        }
        return loras.values.toList() to checkpoint
    }

    private fun findCheckpoint(nodes: Map<String, JsonObject>): String? {
        nodes.values.forEach { n ->
            if (n.cls().contains("CheckpointLoader", true)) {
                n.input("ckpt_name").asText()?.takeIf { it.isNotBlank() }?.let { return it }
            }
        }
        for (key in listOf("ckpt_name", "model_name", "unet_name", "gguf_name", "diffusion_model")) {
            nodes.values.forEach { n ->
                val c = n.cls()
                if (c.contains("Loader", true) || c.contains("UNET", true)) {
                    n.input(key).asText()?.takeIf { it.isNotBlank() }?.let { return it }
                }
            }
        }
        return null
    }

    // -----------------------------------------------------------------------
    //  文本
    // -----------------------------------------------------------------------

    /** 节点自身的文本输入；没有就顺着条件引用往下找 */
    private fun textFromNode(node: JsonObject?, nodes: Map<String, JsonObject>, depth: Int): String? {
        if (node == null || depth > 6) return null
        for (key in TEXT_INPUT_KEYS) {
            node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { return it }
        }
        for (key in CHAIN_KEYS) {
            textFromRef(node.input(key), nodes, depth + 1)?.let { return it }
        }
        return null
    }

    /** 把 ["节点id", 槽位] 解析成文本，能穿透转发节点与原始值节点 */
    private fun textFromRef(ref: JsonElement?, nodes: Map<String, JsonObject>, depth: Int): String? {
        if (ref == null || depth > 6) return null
        if (ref is JsonPrimitive) return ref.contentOrNull?.takeIf { it.isNotBlank() }
        val id = ref.asRef() ?: return null
        val node = nodes[id] ?: return null

        for (key in TEXT_INPUT_KEYS) {
            node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { return it }
        }
        // 上游是 String / Primitive 之类的原始值节点
        val cls = node.cls()
        if (cls.contains("Primitive", true) || cls.contains("String", true) || cls.contains("Text", true)) {
            for (key in listOf("value", "string", "text", "prompt")) {
                node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { return it }
            }
        }
        for (key in CHAIN_KEYS) {
            textFromRef(node.input(key), nodes, depth + 1)?.let { return it }
        }
        return null
    }

    /** 全图兜底：最长的那个文本输入通常就是正向提示词 */
    private fun longestTextInput(nodes: Map<String, JsonObject>): Pair<String, String>? =
        nodes.values
            .mapNotNull { node ->
                TEXT_INPUT_KEYS.firstNotNullOfOrNull { key ->
                    node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { key to it }
                }
            }
            .maxByOrNull { it.second.length }

    /** 负向提示词兜底：键名或类名里带 negative 的文本输入 */
    private fun negativeTextInput(nodes: Map<String, JsonObject>): Pair<String, String>? =
        nodes.values
            .mapNotNull { node ->
                val byKey = NEGATIVE_HINT_KEYS.firstNotNullOfOrNull { key ->
                    node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { key to it }
                }
                byKey ?: if (node.cls().contains("Negative", true)) {
                    TEXT_INPUT_KEYS.firstNotNullOfOrNull { key ->
                        node.input(key).asText()?.takeIf { it.isNotBlank() }?.let { key to it }
                    }
                } else {
                    null
                }
            }
            .maxByOrNull { it.second.length }

    // -----------------------------------------------------------------------
    //  尺寸 / 类型 / 其它参数
    // -----------------------------------------------------------------------

    /** 顺着 latent_image 找宽高与批量；解析不出来就返回 null（宽高会在 extra 里留线索） */
    private fun resolveDimensions(ref: JsonElement?, nodes: Map<String, JsonObject>): Triple<Int, Int, Int?>? {
        val candidates = mutableListOf<JsonObject?>()
        ref?.asRef()?.let { nodes[it] }?.let { candidates += it }
        // 常见 latent 节点兜底
        nodes.values.firstOrNull {
            val c = it.cls()
            (c.contains("EmptyLatent") || c.contains("EmptySD3") || c.contains("EmptyHunyuan") ||
                c.contains("EmptyMochi") || c.contains("EmptyImage")) &&
                it.input("width").asInt() != null
        }?.let { candidates += it }

        for (node in candidates) {
            if (node == null) continue
            val w = node.input("width").asInt()
            val h = node.input("height").asInt()
            val b = node.input("batch_size").asInt() ?: node.input("batch").asInt()
            if (w != null && h != null) return Triple(w, h, b)
        }
        return null
    }

    private fun detectKind(nodes: Map<String, JsonObject>): String {
        var video = false
        var audio = false
        var image = false
        nodes.values.forEach { n ->
            val c = n.cls().lowercase()
            when {
                VIDEO_CLASS_HINTS.any { c.contains(it) } -> video = true
                AUDIO_CLASS_HINTS.any { c.contains(it) } -> audio = true
                else -> image = true
            }
        }
        return when {
            video && audio -> "MIXED"
            video -> "VIDEO"
            audio && !image -> "AUDIO"
            else -> "IMAGE"
        }
    }

    private fun findFilenamePrefix(nodes: Map<String, JsonObject>): String? {
        val prefixes = nodes.values.mapNotNull { n ->
            val c = n.cls()
            val isSaver = c.contains("Save", true) || c.contains("VideoCombine", true)
            if (!isSaver) null else n.input("filename_prefix").asText()
        }.filter { !it.isNullOrBlank() }

        val raw = prefixes.firstOrNull() ?: return null
        val base = raw.substringAfterLast('/').substringAfterLast('\\')
        return base.takeIf { it.isNotBlank() && !it.equals("ComfyUI", ignoreCase = true) }
    }

    private fun collectExtra(nodes: Map<String, JsonObject>): Map<String, String> {
        val out = LinkedHashMap<String, String>()
        outer@ for ((id, node) in nodes) {
            val cls = node.cls().substringBefore('(').trim()
            for ((key, value) in node.inputs()) {
                if (key in SKIP_INPUT_KEYS) continue
                val text = when (value) {
                    is JsonPrimitive -> value.contentOrNull
                    else -> null
                } ?: continue
                if (text.isBlank() || text.length > 400) continue
                out["$cls.$key"] = text
                if (out.size >= 60) break@outer
            }
            if (out.size < 60 && cls.isNotEmpty()) out.putIfAbsent("$cls.__node_id", id)
        }
        return out
    }

    private fun firstNonBlank(vararg values: String?): String? =
        values.firstOrNull { !it.isNullOrBlank() }
}

// ---------------------------------------------------------------------------
//  JsonElement 小工具（对 null / 类型不符都安全）
// ---------------------------------------------------------------------------

internal fun JsonObject.cls(): String = (this["class_type"] as? JsonPrimitive)?.contentOrNull.orEmpty()

internal fun JsonObject.inputs(): JsonObject = this["inputs"] as? JsonObject ?: JsonObject(emptyMap())

internal fun JsonObject.input(key: String): JsonElement? = inputs()[key]

internal fun JsonElement?.asText(): String? = (this as? JsonPrimitive)?.contentOrNull

internal fun JsonElement?.asDouble(): Double? = asText()?.toDoubleOrNull()

internal fun JsonElement?.asInt(): Int? = asText()?.let { t ->
    t.toDoubleOrNull()?.toInt() ?: t.toIntOrNull()
}

internal fun JsonElement?.asLong(): Long? = asText()?.let { t ->
    t.toLongOrNull() ?: t.toDoubleOrNull()?.toLong() ?: t.toULongOrNull()?.toLong()
}

internal fun JsonElement?.asBool(): Boolean? = (this as? JsonPrimitive)?.booleanOrNull

/** 取 `["节点id", 槽位]` 里的节点 id */
internal fun JsonElement?.asRef(): String? =
    (this as? JsonArray)?.firstOrNull()?.let { (it as? JsonPrimitive)?.contentOrNull }
