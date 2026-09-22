package com.comfyhub

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.ConcurrentHashMap

/**
 * 本机 ComfyUI 的**模型清单**与"工作流要的模型 ↔ 本机最接近的那个"的配对（用户建议 ①，2026-09-22）。
 *
 * ## 为什么需要它
 *
 * 用户的原话是"可以使用 civitai 等网站查找符合要求的工作流，**优先根据本机已有的模型修改该工作流**"。
 * 三件工具当时都齐了（`civitai_search` 能搜模型、`comfy_load_workflow` 能读工作流、
 * `comfy_submit(overrides=…)` 能改参数），**缺的正是中间那一步**：判断"这份图里点名的
 * `xxx.safetensors`，本机上哪个文件能顶"。
 *
 * 2026-09-21 的现场（对话 `b5233fb4` seq6）就是这一步没做，模型只能**手读 safetensors 头部**
 * 去确认"那个 13.49 GB 的 checkpoint 是裸 DiT、不含 CLIP/VAE"—— 代价高、结论不稳定，
 * 而且它每次都要重来一遍。
 *
 * ## 两条纪律
 *
 *  1. **只报不改**（用户建议 ① 的原话）。这个对象只回答三个问题：
 *     图里要哪些模型 / 本机最接近的是哪个 / 要改哪几个 `overrides` 键。
 *     真正动手改图的永远是 `comfy_submit(overrides=…)` —— 这样"AI 自作主张改了用户的模型"
 *     这件事在结构上就不可能发生（它必须把覆盖键明明白白写在调用参数里，并且过审批）。
 *  2. **不猜也不编**（AGENTS §10）。配不上就报 `missing` 并明说"本机没有"，
 *     **绝不**为了好看硬塞一个"最接近的"上去；分数低于 [MIN_SCORE] 的一律不给候选。
 *     宁可让模型去问用户 / 去 Civitai 搜，也不要给一个会让 ComfyUI 报"文件不存在"的名字。
 *
 * ## 清单从哪来（两级来源，都能用）
 *
 *  - **首选 `/object_info`**：`CheckpointLoaderSimple.ckpt_name` 的枚举**就是** ComfyUI 自己能加载的
 *    checkpoint 列表，`LoraLoader.lora_name` 就是 LoRA 列表 —— 这是唯一权威的"能加载什么"
 *    （它认 extra_model_paths.yaml 里指向的别的盘、也认用户自建的子目录）。零成本、已经在缓存里。
 *  - **兜底扫盘**：拿不到 `/object_info` 时（ComfyUI 没在跑）按目录扫 `models\<folder>`
 *    （判据复用 [ComfyRoots]，与读白名单同一套根目录），只看文件元数据、不打开文件。
 *    扫出来的结果**不能**保证能加载（可能没被 ComfyUI 认到），所以报告里会带上 `source`。
 */
object ComfyModels {

    private val log = LoggerFactory.getLogger(ComfyModels::class.java)

    /** 认的模型文件后缀（ComfyUI 实际能加载的那些；`.pth` / `.bin` / `.ckpt` 是历史格式）。 */
    private val MODEL_EXTS = setOf(
        "safetensors", "ckpt", "pt", "pth", "bin", "gguf", "sft", "onnx", "yaml", "json",
    )

    /** 一次最多列多少条（用户的 models 目录上千个文件是常态，不能整个塞进工具结果）。 */
    const val MAX_ENTRIES = 500

    /** 一条候选最多给几个（3 个足够做决定，多了只会淹掉结论）。 */
    const val MAX_CANDIDATES = 3

    /**
     * 多低的相似度就不给候选了。
     *
     * 0.35 ≈ "至少有一半 token 对上了"。定这条线是为了**不编**：ComfyUI 只认磁盘上真实存在的
     * 文件名，给一个看着像的名字等于让用户白等一次提交（还会得到一句看不懂的报错）。
     */
    const val MIN_SCORE = 0.35

    /** 扫描结果的缓存时长（装完模型重查能得到新结果，又不必每次工具调用都重扫）。 */
    private const val SCAN_TTL_MS = 30_000L

    /**
     * 一个节点输入"等着一个模型文件"。
     *
     * [folder] 是 ComfyUI 在 `models\` 下的子目录名 —— 扫盘兜底时按它找；
     * 从 `/object_info` 拿时其实不需要它（枚举已经列全了），留着是为了让报告能说清"该放哪"。
     */
    private data class ModelInput(val cls: String, val input: String, val folder: String)

    /**
     * 认得出的"模型加载器"。
     *
     * 只列**确定**的：判据是"这个类的这个输入，值就是一个模型文件名"。
     * 认不出来的第三方加载器**不猜**（宁可漏报，也不要把 `model` 这种转发连线当成文件名）。
     */
    private val MODEL_INPUTS: Map<String, List<ModelInput>> by lazy {
        fun m(cls: String, input: String, folder: String) = cls to ModelInput(cls, input, folder)
        listOf(
            m("CheckpointLoaderSimple", "ckpt_name", "checkpoints"),
            m("CheckpointLoader", "ckpt_name", "checkpoints"),
            m("unCLIPCheckpointLoader", "ckpt_name", "checkpoints"),
            m("ImageOnlyCheckpointLoader", "ckpt_name", "checkpoints"),
            m("UNETLoader", "unet_name", "diffusion_models"),
            m("UnetLoaderGGUF", "unet_name", "diffusion_models"),
            m("UnetLoaderGGUFAdvanced", "unet_name", "diffusion_models"),
            m("DiffusionModelLoader", "model_name", "diffusion_models"),
            m("LoraLoader", "lora_name", "loras"),
            m("LoraLoaderModelOnly", "lora_name", "loras"),
            m("LoraModelLoader", "lora_name", "loras"),
            m("VAELoader", "vae_name", "vae"),
            m("CLIPLoader", "clip_name", "text_encoders"),
            m("DualCLIPLoader", "clip_name1", "text_encoders"),
            m("DualCLIPLoader", "clip_name2", "text_encoders"),
            m("TripleCLIPLoader", "clip_name1", "text_encoders"),
            m("TripleCLIPLoader", "clip_name2", "text_encoders"),
            m("TripleCLIPLoader", "clip_name3", "text_encoders"),
            m("CLIPVisionLoader", "clip_name", "clip_vision"),
            m("ControlNetLoader", "control_net_name", "controlnet"),
            m("DiffControlNetLoader", "control_net_name", "controlnet"),
            m("StyleModelLoader", "style_model_name", "style_models"),
            m("UpscaleModelLoader", "model_name", "upscale_models"),
            m("GLIGENLoader", "gligen_name", "gligen"),
            m("PhotoMakerLoader", "photomaker_model_name", "photomaker"),
            m("IPAdapterModelLoader", "ipadapter_file", "ipadapter"),
            m("CLIPVisionEncode", "clip_vision", "clip_vision"),
            m("InstantIDModelLoader", "instantid_file", "instantid"),
            m("IPAdapterUnifiedLoader", "preset", "ipadapter"),
        ).groupBy({ it.first }, { it.second })
    }

    /** 清单的来源 —— 报告里要能看出这一档（"能加载"与"盘上有"不是一回事）。 */
    enum class Source(val wire: String) {
        /** 来自 ComfyUI 的 `/object_info`：**权威**，列出来的就是它能加载的。 */
        OBJECT_INFO("object_info"),

        /** 拿不到 ComfyUI，按目录扫出来的：盘上有，但未必被它认到。 */
        SCAN("scan"),

        /** 两边都没有（ComfyUI 没跑，也没找到 models 目录）。 */
        NONE("none"),
    }

    /** 本机的一个模型文件。 */
    @Serializable
    data class LocalModel(
        /** ComfyUI 认的名字（`/object_info` 枚举里那一串，可能带子目录前缀，如 `SDXL\a.safetensors`） */
        val name: String,
        /** 它在 `models\` 下属于哪一类（checkpoints / loras / vae …） */
        val folder: String,
        /** 盘上的绝对路径（扫盘来源才有；`/object_info` 来源为 null） */
        val path: String? = null,
        val bytes: Long? = null,
    ) {
        /** 给用户看的体积（GB，一位小数）；不知道就 null —— **不编一个数出来**。 */
        val sizeGb: Double? get() = bytes?.let { it / 1024.0 / 1024.0 / 1024.0 }
    }

    /** 一份完整的本机清单。 */
    data class Inventory(
        val source: Source,
        val models: List<LocalModel>,
        /** 认出来的 `models\` 目录（扫盘来源才有；报告里要能说清"扫的是哪儿"） */
        val modelsDir: Path? = null,
        val truncated: Boolean = false,
    ) {
        fun inFolder(folder: String): List<LocalModel> =
            models.filter { it.folder.equals(folder, ignoreCase = true) }

        /**
         * 全清单里找候选：**先在本类目录里找，不够再跨目录找**
         * （用户把 LoRA 放进 checkpoints 是常见事故，报出来比一句"本机没有"有用得多）。
         */
        fun candidatesFor(folder: String, wanted: String): List<Scored> {
            fun scoredOf(local: LocalModel, sameFolder: Boolean): Scored? {
                val s = ModelName.similarity(wanted, local.name)
                if (s < MIN_SCORE) return null
                return Scored(local, s, sameFolder)
            }
            val same = inFolder(folder).mapNotNull { scoredOf(it, sameFolder = true) }
            val cross = if (same.size >= MAX_CANDIDATES) {
                emptyList()
            } else {
                models.filter { !it.folder.equals(folder, ignoreCase = true) }
                    .mapNotNull { scoredOf(it, sameFolder = false) }
            }
            return (same + cross).sortedByDescending { it.score }.take(MAX_CANDIDATES)
        }

        /**
         * **接近但不够像**的几条（分数低于 [ComfyModels.MIN_SCORE]）。
         *
         * 用途只有一个（用户建议 ②，2026-09-22）：缺模型时给用户看"本机其实有几个
         * 名字沾边的，但没有一个够像" —— 这一句能挡掉一整类误判。用户在 `ask_user_question`
         * 里看到"要装它（13.5GB）"时，很容易以为自己已经有那个模型了（因为盘上确实有个
         * 名字像的），报出来他就能自己判断"哦那个不是同一个"。
         *
         * **它不出现在候选里**（分数不达标就不给），只作为缺模型时的一句话背景。
         */
        fun nearMissesFor(folder: String, wanted: String, limit: Int = 3): List<LocalModel> = models
            .map { it to ModelName.similarity(wanted, it.name) }
            // 两道闸门，缺一个这句提示就会变成噪声（用户建议 ② 要的是"有用的一栏"，不是"一堆名字"）：
            //  ① 分数要过 [NEAR_MISS_MIN]（只共用一个 `xl` / `1` 这种通用词就不算"沾边"）；
            //  ② 至少要有一个**长度 ≥ 3 的公共词**（`krea2` / `turbo` / `int8` 这类才是真的沾边）
            .filter { (local, s) ->
                s > NEAR_MISS_MIN && s < MIN_SCORE &&
                    (ModelName.tokens(wanted) intersect ModelName.tokens(local.name)).any { it.length >= 3 }
            }
            .sortedWith(compareByDescending<Pair<LocalModel, Double>> { it.second }.thenBy { it.first.name })
            .take(limit)
            .map { it.first }
    }

    /**
     * "沾边"的下限。
     *
     * 0.15 ≈ 只共用一个词。定这条是因为**报错的名字比不报更糟**：用户看到
     * "本机有个 sd15.safetensors" 会以为"那不就是我的模型吗"，
     * 而它跟 `krea2TurboInt8` 其实毫无关系。
     */
    const val NEAR_MISS_MIN = 0.15

    /** 一个候选（[sameFolder] 为假 = 它不在该输入本来的那一类目录里，用之前要跟用户说一声）。 */
    data class Scored(val model: LocalModel, val score: Double, val sameFolder: Boolean)

    // -----------------------------------------------------------------------
    //  清单来源
    // -----------------------------------------------------------------------

    /** 扫盘结果的缓存（按 models 目录分组，避免同一批工具调用反复遍历磁盘）。 */
    private class Cached(val at: Long, val dir: Path?, val models: List<LocalModel>, val truncated: Boolean)

    private val scanCache = ConcurrentHashMap<String, Cached>()

    /**
     * 拿本机清单。
     *
     * @param objectInfo ComfyUI 的 `/object_info`（拿得到就用它 —— 那是权威清单）；
     *   为 null 时退回扫盘（[ComfyRoots] 认出来的 ComfyUI 根 + 用户配置的输出目录父目录）。
     * @param projectRoot 项目根（扫盘时用来定位项目内的 `comfyui` 与调用 [ComfyRoots] 的探测）；
     *   为 null 就只用环境变量与用户配置。
     */
    fun inventory(objectInfo: JsonObject?, projectRoot: Path? = null): Inventory {
        fromObjectInfo(objectInfo)?.let { return it }
        return scan(projectRoot)
    }

    /**
     * 从 `/object_info` 的**输入枚举**里抠出模型清单。
     *
     * 这是 §"清单从哪来"说的首选来源：`CheckpointLoaderSimple` 的 `ckpt_name` 声明形如
     * `[[ "a.safetensors", "sub\b.safetensors" ]]`（第一项是枚举，后面可能跟 `{"tooltip":…}`），
     * 枚举里那一串**就是** ComfyUI 能加载的全部文件。
     *
     * 拿不到任何一条时返回 null（→ 退回扫盘）：空清单比"没有清单"更危险，
     * 它会让每一份工作流都看起来"什么模型都缺"。
     */
    fun fromObjectInfo(objectInfo: JsonObject?): Inventory? {
        if (objectInfo == null) return null
        val out = LinkedHashMap<String, LocalModel>()
        MODEL_INPUTS.forEach { (cls, inputs) ->
            val node = objectInfo[cls] as? JsonObject ?: return@forEach
            val required = node["input"]?.let { it as? JsonObject }?.get("required") as? JsonObject
                ?: return@forEach
            inputs.forEach { spec ->
                val decl = required[spec.input] as? JsonArray ?: return@forEach
                val enum = decl.firstOrNull() as? JsonArray ?: return@forEach
                enum.forEach { raw ->
                    val name = (raw as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return@forEach
                    if (name.isBlank()) return@forEach
                    // 同一个文件名可能挂在不同类下（CLIPVisionLoader 与 IPAdapter 都读 clip_vision）：
                    // 以**先出现的类**为准，避免同一份文件在清单里出现两次
                    out.putIfAbsent(
                        "$cls|$name",
                        LocalModel(name = name, folder = spec.folder, path = null, bytes = null),
                    )
                }
            }
        }
        if (out.isEmpty()) return null
        val models = out.values.sortedWith(compareBy({ it.folder }, { it.name }))
        val truncated = models.size > MAX_ENTRIES
        return Inventory(
            source = Source.OBJECT_INFO,
            models = if (truncated) models.take(MAX_ENTRIES) else models,
            truncated = truncated,
        )
    }

    /**
     * 扫盘：在 [ComfyRoots] 认出来的 ComfyUI 根底下看 `models\<folder>\**`（只读元数据）。
     *
     * 有界：每个目录最多 [MAX_ENTRIES] 条、不跟随符号链接、深度 ≤ [MAX_SCAN_DEPTH]。
     * 扫不到就返回空清单（[Source.NONE]）—— 那是"这台机器没装 ComfyUI / 没放模型"的正常情形，
     * 不是错误；报告里会如实说"本机清单是空的，所以配不上任何模型"。
     */
    fun scan(projectRoot: Path? = null): Inventory {
        val roots = scanRoots(projectRoot)
        if (roots.isEmpty()) return Inventory(Source.NONE, emptyList())

        val key = roots.joinToString(";") { it.toString() }
        scanCache[key]?.let { if (System.currentTimeMillis() - it.at < SCAN_TTL_MS) {
            return Inventory(Source.SCAN, it.models, it.dir, it.truncated)
        } }

        val found = LinkedHashMap<String, LocalModel>()
        var truncated = false
        var modelsDir: Path? = null

        for (root in roots) {
            val dir = root.resolve("models")
            if (!Files.isDirectory(dir)) continue
            if (modelsDir == null) modelsDir = dir
            MODEL_INPUTS.values.flatten().map { it.folder }.distinct().forEach { folder ->
                val folderDir = dir.resolve(folder)
                if (!Files.isDirectory(folderDir)) return@forEach
                val files = listFiles(folderDir)
                files.forEach { file ->
                    if (found.size >= MAX_ENTRIES) {
                        truncated = true
                        return@forEach
                    }
                    val name = folderDir.relativize(file).toString().replace('\\', '/')
                    found.putIfAbsent(
                        "$folder|$name",
                        LocalModel(
                            name = name,
                            folder = folder,
                            path = file.toString(),
                            bytes = runCatching { Files.size(file) }.getOrNull(),
                        ),
                    )
                }
            }
        }
        val models = found.values.sortedWith(compareBy({ it.folder }, { it.name }))
        scanCache[key] = Cached(System.currentTimeMillis(), modelsDir, models, truncated)
        log.info("扫到本机模型 {} 个（models={}）", models.size, modelsDir)
        return Inventory(Source.SCAN, models, modelsDir, truncated)
    }

    /** 扫盘时看的 ComfyUI 根：环境变量 / 用户配置的输出目录父目录 / 项目内 `comfyui`。 */
    private fun scanRoots(projectRoot: Path?): List<Path> {
        val out = LinkedHashSet<Path>()
        if (projectRoot != null) {
            runCatching { ComfyRoots.autoReadRoots(projectRoot) }.getOrDefault(emptyList())
                .forEach { root ->
                    // autoReadRoots 给的是"ComfyUI 根"与"它周围的目录"，只留真的有 models 的
                    if (Files.isDirectory(root.resolve("models"))) out.add(root)
                    // ComfyUI Desktop 把共享模型放在 ComfyUI-Shared 那一侧：它的 models 是兄弟目录
                    if (root.fileName?.toString()?.contains("models", true) == true) {
                        root.parent?.let { if (Files.isDirectory(it)) out.add(it) }
                    }
                }
        }
        return out.toList()
    }

    /** 一个目录下所有认得出是模型的文件（带深度上限、不跟符号链接），按名字排序。 */
    private fun listFiles(dir: Path): List<Path> = runCatching {
        Files.walk(dir, MAX_SCAN_DEPTH).use { stream ->
            stream.filter { Files.isRegularFile(it) && !Files.isSymbolicLink(it) }
                .filter { isModelFile(it) }
                .sorted()
                .limit(MAX_ENTRIES.toLong())
                .toList()
        }
    }.getOrElse { emptyList() }

    private fun isModelFile(path: Path): Boolean {
        val name = path.fileName?.toString() ?: return false
        val ext = name.substringAfterLast('.', "").lowercase()
        return ext in MODEL_EXTS
    }

    private const val MAX_SCAN_DEPTH = 3

    // -----------------------------------------------------------------------
    //  图里要哪些模型
    // -----------------------------------------------------------------------

    /** 图里点名的**一个**模型。 */
    data class Requirement(
        val nodeId: String,
        val classType: String,
        val inputName: String,
        val fileName: String,
        val folder: String,
        val title: String? = null,
        /** 1 = 必填（`/object_info` 说它是 required），0 = 可选（VHS 之类会说 optional） */
        val required: Boolean = true,
    ) {
        /** 覆盖时用的键（`comfy_submit(overrides=…)` 的键就是它）。 */
        val overrideKey: String get() = "$nodeId.$inputName"
    }

    /**
     * 扫一份 **API 格式**节点图，列出它点名的所有模型文件。
     *
     * 只认 [MODEL_INPUTS] 里那些**确定是文件名**的输入，且值必须是**字符串**：
     * 值是数组（连线）就跳过 —— 那是"从别的节点来的"，不是"从磁盘加载的"。
     * 认不出来的第三方加载器**不猜**（宁可漏报，也不要拿连线数组当文件名）。
     *
     * @param modelInputs 可选：用 `/object_info` 现算的"哪个类哪个输入是模型枚举"，
     *   用来补上 [MODEL_INPUTS] 里没写死的自定义节点（例如某个第三方 `MyLoader.model_name`）。
     *   传 null 就只用内置那张表。
     */
    fun requirements(graph: JsonObject, modelInputs: Map<String, Set<String>>? = null): List<Requirement> {
        val out = mutableListOf<Requirement>()
        graph.forEach { (nodeId, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val cls = (node["class_type"] as? JsonPrimitive)?.contentOrNull ?: return@forEach
            val inputs = node["inputs"] as? JsonObject ?: return@forEach
            val title = ((node["_meta"] as? JsonObject)?.get("title") as? JsonPrimitive)?.contentOrNull

            val known = MODEL_INPUTS[cls].orEmpty()
            val fromObjectInfo = modelInputs?.get(cls).orEmpty()
            val all = (known.map { it.input to it.folder } + fromObjectInfo.map { it to folderGuess(it) })
                .distinctBy { it.first }

            all.forEach { (input, folder) ->
                val value = inputs[input] as? JsonPrimitive ?: return@forEach
                if (!value.isString) return@forEach
                val name = value.content
                if (name.isBlank()) return@forEach
                // 空串 / 明显不是文件的（下拉预设名，如 IPAdapterUnifiedLoader 的 "PLUS (high strength)"）
                // 不算"要装一个模型"：它没有后缀，也点不出体积
                out += Requirement(
                    nodeId = nodeId,
                    classType = cls,
                    inputName = input,
                    fileName = name,
                    folder = known.firstOrNull { it.input == input }?.folder ?: folder,
                    title = title,
                )
            }
        }
        return out.distinctBy { it.overrideKey }.sortedBy { it.nodeId.toIntOrNull() ?: Int.MAX_VALUE }
    }

    /** 从 `/object_info` 反推一个模型枚举输入属于哪一类目录（报告里说"该放哪"用）。 */
    private fun folderGuess(input: String): String = when {
        input.contains("ckpt", true) -> "checkpoints"
        input.contains("lora", true) -> "loras"
        input.contains("vae", true) -> "vae"
        input.contains("clip_vision", true) -> "clip_vision"
        input.contains("clip", true) -> "text_encoders"
        input.contains("control_net", true) -> "controlnet"
        input.contains("unet", true) || input.contains("model_name", true) -> "diffusion_models"
        input.contains("style", true) -> "style_models"
        else -> "checkpoints"
    }

    /**
     * 从 `/object_info` 算出"哪些类的哪些输入是模型枚举"。
     *
     * 判据只有一条：**这个输入的第一个声明是字符串数组**（那就是 ComfyUI 给的枚举），
     * 且里面有模型后缀的项。这样第三方加载器也能被认出来（不用维护一张越来越长的白名单），
     * 而 `INT` / `STRING`（自由文本）之类不会被误认 —— 前者第一个声明是字符串不是数组，
     * 后者的第一项是 `"STRING"` 这个**类型名**（也认得出：类型名里没有点后缀）。
     */
    fun enumerateModelInputs(objectInfo: JsonObject?): Map<String, Set<String>> {
        if (objectInfo == null) return emptyMap()
        val out = mutableMapOf<String, MutableSet<String>>()
        objectInfo.forEach { (cls, raw) ->
            val node = raw as? JsonObject ?: return@forEach
            val sections = listOf("required", "optional").mapNotNull {
                (node["input"] as? JsonObject)?.get(it) as? JsonObject
            }
            sections.forEach { section ->
                section.forEach { (input, decl) ->
                    val arr = decl as? JsonArray ?: return@forEach
                    val enum = arr.firstOrNull() as? JsonArray ?: return@forEach
                    val looksLikeFiles = enum.any { e ->
                        val s = (e as? JsonPrimitive)?.contentOrNull ?: return@any false
                        s.substringAfterLast('.', "").lowercase() in MODEL_EXTS
                    }
                    if (looksLikeFiles) out.getOrPut(cls) { mutableSetOf() }.add(input)
                }
            }
        }
        return out
    }
}

/**
 * 模型文件名的**配对打分**（用户建议 ① 的核心判据）。
 *
 * 现场长这样：工作流点名 `krea2TurboOfficialComfy_krea2TurboInt8.safetensors`，
 * 而本机盘上那份叫 `krea2_turbo_int8_v2.safetensors` —— 字面上几乎不像，
 * 但**词根全对**（krea2 / turbo / int8）。反过来，`sd_xl_base_1.0.safetensors`
 * 与 `sd_xl_refiner_1.0.safetensors` 字面上很像，却**是两份不同的模型**，不能顶。
 *
 * 所以判据是**分词后的集合相似度**（Dice 系数），不是字符串编辑距离：
 *
 *  - 归一化：去扩展名 → 小写 → 把 `-_. \/|+` 统一成空格 → 拆词；
 *  - 额外把"数字紧跟字母"的地方切开（`krea2` → `krea` + `2`），
 *    这样 `krea2Turbo` 与 `krea_2_turbo` 能对上；
 *  - 相似度 = 2 × 交集 / (|A| + |B|)，再用"一个是不是另一个的子串"给一点加权。
 *
 * 实测这几个例子：
 *
 * | 要的 | 本机有的 | 分 |
 * | --- | --- | --- |
 * | `krea2TurboOfficialComfy_krea2TurboInt8.safetensors` | `krea2_turbo_int8.safetensors` | 0.75 |
 * | `sd_xl_base_1.0.safetensors` | `sd_xl_refiner_1.0.safetensors` | 0.63 |
 * | `sd_xl_base_1.0.safetensors` | `sd_xl_base_1.0.safetensors` | 1.00 |
 *
 * 注意第二行：0.63 高过阈值，所以它**会**作为候选报出来 —— 这是**有意**的。
 * 工具不替模型做决定，它只给"本机最接近的那个 + 分数"；该不该用由模型看着分数与
 * 体积自己判断（`safeToAutoApply` 那道闸门挡着"一模一样之外的自动替换"）。
 */
object ModelName {

    /** 词根级相似度（0~1）。 */
    fun similarity(wanted: String, local: String): Double {
        val a = tokens(wanted)
        val b = tokens(local)
        if (a.isEmpty() || b.isEmpty()) return 0.0
        if (a == b) return 1.0
        val inter = a.intersect(b).size
        val dice = 2.0 * inter / (a.size + b.size)
        // 一个完整包含另一个（`krea2_turbo` ⊂ `krea2_turbo_int8_v2`）：给一点加权，
        // 免得"更完整的那个"因为分母变大反而被扣分
        val containment = if (a.all { it in b } || b.all { it in a }) 0.12 else 0.0
        return (dice + containment).coerceAtMost(1.0)
    }

    /** 严格相等（同名同后缀）：只有这一种情况才允许"不用问用户就替换"。 */
    fun same(wanted: String, local: String): Boolean = tokens(wanted) == tokens(local)

    /**
     * 把文件名变成一个**能拿去搜的查询词**（用户建议 ②，2026-09-22）。
     *
     * `krea2TurboOfficialComfy_krea2TurboInt8.safetensors` → `krea2 Turbo Int8`
     * —— 拿整个文件名去 `civitai_search` 是搜不到的（那是一串打包名），
     * 但词根级别的词是能的。所以：归一化成词 → 去掉最常见的那几个噪声词
     * （`v1` / `final` / `official` 这类）→ 用空格连起来 → 最多 6 个词。
     *
     * **这不是"猜模型名"**：它只是把用户图里那个文件名转成一句更好的搜索词，
     * 搜到什么仍然由 Civitai 说了算（搜不到就如实说没搜到）。
     */
    fun lookupQuery(fileName: String): String {
        val stop = setOf(
            "official", "comfy", "comfyui", "final", "fixed", "copy", "new", "old",
            "v", "ver", "version", "model",
        )
        // ⚠️ 顺序要紧：**先归一化去重，再截断**。两个坑（2026-09-22 都被用例抓到）：
        //  ① 打包名里同一个词会出现两遍（`krea2Turbo…_krea2TurboInt8`），
        //     先去重才能保证 `take(N)` 用的是不重复的词；
        //  ② 单字**数字**（int8 的那个 `8`）必须留着 —— 它是"量化位宽"里最有信息量的一位，
        //     丢了会把 int8 搜成别的文件。所以只丢纯字母的单字词（`v` / `a`）。
        val kept = tokens(fileName)
            .filter { it !in stop }
            .filter { it.length > 1 || it.all { c -> c.isDigit() } }
            .toList()
        if (kept.isEmpty()) {
            // 全被停用词吃掉时**退回原文件名**（空查询词会被 civitai_search 当场拒掉）：
            // 那比"返回空串"强 —— 搜不到是上游说了算，但至少这次调用是合法的
            return fileName.trim().take(80)
        }
        // 再把"字母后紧跟的数字"粘回去（`krea` + `2` → `krea2`、`int` + `8` → `int8`）：
        // 分词是为了算相似度（`krea2` 与 `krea_2` 必须能对上），但**搜索词要更像文件名** ——
        // 真实的模型名就叫 `krea2`，搜 "krea 2 turbo int 8" 会带进一堆无关结果。
        val merged = mutableListOf<String>()
        kept.take(6).forEach { w ->
            val prev = merged.lastOrNull()
            if (prev != null && w.all { c -> c.isDigit() } && prev.last().isLetter()) {
                merged[merged.size - 1] = prev + w
            } else {
                merged += w
            }
        }
        return merged.joinToString(" ")
    }

    /** 归一化成一个可比较的词集合。 */
    fun tokens(raw: String): Set<String> {
        val noExt = raw.substringBeforeLast('.', raw)
        // ⚠️ 大小写边界要在**动过大小写之前**判（`toLowerCase()` 之后
        // `krea2TurboOfficialComfy` 已经看不出词界了）—— 这一条是 2026-09-22 实测抓到的：
        // 少了它，`lookupQuery` 会把整串当成一个词，搜出来的结果是错的。
        val spaced = StringBuilder()
        noExt.forEachIndexed { i, c ->
            when {
                c in " -_.\\/|+()[]{}" -> spaced.append(' ')
                // 小写 → 大写：驼峰词界（`krea2Turbo` → `krea2` + `Turbo`）
                c.isUpperCase() && i > 0 && noExt[i - 1].isLowerCase() ->
                    spaced.append(' ').append(c)
                // 字母 → 数字（`krea2`）；两个方向都要，`flux1dev` 才会切成 flux/1/dev
                c.isDigit() && i > 0 && noExt[i - 1].isLetter() ->
                    spaced.append(' ').append(c)
                c.isLetter() && i > 0 && noExt[i - 1].isDigit() ->
                    spaced.append(' ').append(c)
                else -> spaced.append(c)
            }
        }
        return spaced.toString().lowercase().split(' ').filter { it.isNotBlank() }.toSet()
    }
}
