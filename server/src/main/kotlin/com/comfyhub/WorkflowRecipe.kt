package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import java.security.MessageDigest

/**
 * **工作流配方**（recipe）：把一份**具体**工作流归纳成
 * 「任务 + 骨架指纹 + 模型槽位 + 参数旋钮 + 输出」。
 *
 * 用户 2026-09-26 的原话："我想可能很多模型都有 image edit 工作流 …… 甚至视频模型都有作为
 * image edit 基底模型的资质。**如果以后出了新的模型，如何让 AI 自主学习新模型泛用/特定工作流的搭建？**"
 * 设计与论证在 `docs/workflow-recipe-design.md`，这里只写实现上的四条纪律：
 *
 * ## 一、这是**归纳**，不是猜测
 *
 * 三层分解（见设计 §2）全部踩在**代码里已有的事实**上：
 *  - **模型槽位**直接复用 [ComfyModels.requirements] —— 那张"哪个类的哪个输入是一个模型文件名"
 *    的确定表（外加 `/object_info` 现算的枚举）。**这里不新建第二张角色表**；
 *    认不出的第三方加载器**不猜**（宁可漏报，也不要把 `model` 这种转发连线当成文件名）。
 *  - **骨架指纹**只由"执行相关节点类 + 类级连线"算出：节点 id、文件名、参数值**都不进指纹**
 *    （所以同一族工作流即使换了底模、换了节点编号，指纹仍然一样 —— 这就是"泛用"的可计算定义）。
 *  - **任务分类**按"输出与输入的位置"判（有视频输出 = video、有遮罩链 = inpaint、
 *    有参考图编辑链 = edit…），判据是显式类名集合，**不用子串瞎匹配**
 *    （`ResizeImageMaskNode` 里有 "Mask" 但它只是缩放，把它当 inpainting 就会骗到模型）。
 *
 * ## 二、任务分类是**提示**，不是结论
 *
 * `inpaint` / `video` / `audio` 三档的判据是硬事实（遮罩链 / 视频输出 / 音频输出），
 * `edit` 与 `i2i` 的边界本来就是连续谱（有参考图但没有遮罩）。所以：
 *  - 认不出时回 [Task.UNKNOWN] 并写进 `warnings`，**不许硬塞一档**；
 *  - `warnings` 里永远有一句"按节点形状判的、可能不准"，让模型自己复核。
 *
 * ## 三、配方**不是**可提交对象（红线 R10）
 *
 * 这里产出的只是**知识**：槽位（该改哪个键）+ 旋钮（能改什么）+ 骨架（这一族长什么样）。
 * 真正提交永远走 `comfy_submit(promptId=…, overrides=…)` 那条链路
 * （审批 → 提交 → 等 → `capture.captureRun()` 入库），配方既不生成图也不写用户的文件。
 *
 * ## 四、纯函数、可单测
 *
 * 同一个 API 节点图进来，出去的结构逐字一样（没有时间、没有随机、没有 I/O）。
 * `_meta.title` 之类的界面信息只作为**可读性**带出去，不参与指纹与判定。
 *
 * @see WorkflowRecipeRepo 落库与查询（W2）
 * @see com.comfyhub.ai.tools.ToolRegistry `comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe`
 */
object WorkflowRecipe {

    /**
     * 解析规则的版本号 —— **进指纹的输入**。
     *
     * 判据（类集合 / 前端节点名单 / 分类规则）一改，同一个图的指纹就该变，
     * 否则库里会同时存在"用旧规则算的骨架"和"用新规则算的骨架"，`group by skeleton_hash`
     * 就会把两族混在一起，而**没有任何报错**。所以它进 sha256 的正文。
     */
    const val VERSION = 1

    /** 旋钮值最长留多少字符（提示词可以很长；配方只用来"看出这是什么值"，不当作提交参数）。 */
    const val MAX_KNOB_VALUE = 600

    /** 一份配方最多留几个旋钮（113 节点的工作流有几百个字面量，全列会淹掉模型要看的几条）。 */
    const val MAX_KNOBS = 80

    /** 一条警告最长多少字（进工具结果与界面，太长会把结论挤掉）。 */
    const val MAX_WARNING_CHARS = 300

    // -----------------------------------------------------------------------
    //  任务分类
    // -----------------------------------------------------------------------

    /**
     * 这份工作流是干什么的。
     *
     * [wire] 进库、进 `comfy_list_recipes(task=…)` 的过滤参数，所以是稳定字符串。
     */
    enum class Task(val wire: String) {
        T2I("t2i"),
        I2I("i2i"),

        /**
         * **参考图编辑**（instruction edit）：有参考图编辑链 ——
         * Krea 2 的 `Krea2EditModelPatch` / `Krea2OstrisEditModelPatch` / `TextEncodeKrea2OstrisEdit*`、
         * Flux Kontext 风格的多参考潜空间方法等。
         *
         * 它**优先于** [INPAINT]：Krea 2 的 Identity Edit 同时带遮罩链（`ImageToMask` / `GrowMask`），
         * 但它的能力是"照参考图改" —— 把它归成 inpaint 的话，
         * `comfy_list_recipes(task="edit")` 永远找不到它，而那正是用户说的"图像编辑工作流"。
         */
        EDIT("edit"),

        /** 遮罩局部重绘（`LoadImageMask` / `SetLatentNoiseMask` / `InpaintModelConditioning` …）。 */
        INPAINT("inpaint"),

        /** 只有放大模型、没有采样器。 */
        UPSCALE("upscale"),

        VIDEO("video"),
        AUDIO("audio"),

        /** 认不出（空图 / 既不生成也不吃图）。**如实说，不硬塞**。 */
        UNKNOWN("unknown");

        companion object {
            fun parse(wire: String?): Task? = entries.firstOrNull { it.wire == wire }
        }
    }

    /** 旋钮的种类（只为让模型一眼看出"哪些是提示词、哪些是采样参数"）。 */
    enum class KnobKind(val wire: String) {
        /** 文本编码节点的正文（提示词）。 */
        PROMPT("prompt"),

        /** 图片 / 遮罩 / 视频文件名（`LoadImage.image`、`LoadImageMask.image`…）。 */
        IMAGE("image"),

        /** 采样参数（`steps` / `cfg` / `sampler_name` / `scheduler` / `denoise`…）。 */
        SAMPLING("sampling"),

        /** 随机种子。 */
        SEED("seed"),

        /** 尺寸（`width` / `height` / `batch_size`…）。 */
        SIZE("size"),

        /**
         * **加载器上那些"模型族相关"的开关**（`CLIPLoader.type=krea2`、`UNETLoader.weight_dtype`、
         * LoRA 权重…）。
         *
         * 单独一类的原因：它们不像文件名那样显眼，但换模型族时**照样要改**
         * （`type=qwen_image` 与 `type=krea2` 不是一回事），漏掉它就会出现
         * "底模换对了、文本编码器还是按老模型解"这种**不报错但结果不对**的情形。
         */
        LOADER("loader"),

        OTHER("other");

        companion object {
            fun parse(wire: String?): KnobKind? = entries.firstOrNull { it.wire == wire }
        }
    }

    /** 一个模型槽位（= 一个"从磁盘加载模型文件"的输入，可换件）。 */
    data class Slot(
        /** 角色/该放哪个目录：`checkpoints` / `diffusion_models` / `text_encoders` / `vae` / `loras` … */
        val role: String,
        val nodeId: String,
        val classType: String,
        val inputName: String,
        val fileName: String,
        /** 节点在文件里的标题（`_meta.title`，可读性用，不参与判定） */
        val title: String? = null,
        /**
         * [role] 是**猜的**：这个槽位来自 `/object_info` 现算的自定义加载器
         * （[ComfyModels.MODEL_INPUTS] 那张确定表里没有它），所以只知道"它要一个模型文件"，
         * **不知道**该放哪个目录。
         *
         * 如实标出来而不是硬塞一个目录名：`UltralyticsDetectorProvider.model_name` 的真身是
         * `models/ultralytics/bbox` 下的检测器权重，猜成 `diffusion_models` 会把人指到错的地方
         * （真机探针上就这么显示过）。
         */
        val roleGuessed: Boolean = false,
    ) {
        /** `comfy_submit(overrides=…)` 的键。 */
        val overrideKey: String get() = "$nodeId.$inputName"
    }

    /** 一个可覆盖的**字面量**输入。 */
    data class Knob(
        val nodeId: String,
        val classType: String,
        val inputName: String,
        val kind: KnobKind,
        /** 当前值（数字/布尔按文本给；超长已按 [MAX_KNOB_VALUE] 截断） */
        val value: String,
        val title: String? = null,
    ) {
        val overrideKey: String get() = "$nodeId.$inputName"
    }

    /** 一个输出节点（图跑到哪儿结束）。 */
    data class Output(val nodeId: String, val classType: String, val kind: String)

    /**
     * 归纳结果（**配方正文**）。
     *
     * [skeletonHash] 是"泛用"的键：同一族工作流（换了底模、换了节点编号、换了参数）指纹相同，
     * 于是 `comfy_list_recipes(task="edit")` 能把它们归到一族里让模型挑。
     */
    data class Recipe(
        val task: Task,
        val skeletonHash: String,
        /** 执行相关节点类（**带重数**、已排序，进指纹） */
        val nodeClasses: List<String>,
        /** 类级连线（`下游类.输入 <- 上游类[槽位]`，带重数、已排序，进指纹） */
        val edges: List<String>,
        /** 模型槽位（**按节点号升序**，沿用 [ComfyModels.requirements] 的顺序 —— 报告要逐字可复现） */
        val slots: List<Slot>,
        val knobs: List<Knob>,
        val outputs: List<Output>,
        /** 图里"从磁盘读一张图/一段视频"的节点个数（判 i2i 的依据，也是给模型看的规模） */
        val imageInputs: Int,
        val hasMask: Boolean,
        val nodeCount: Int,
        /** 旋钮是不是被 [MAX_KNOBS] 截断了 */
        val knobsTruncated: Boolean = false,
        val warnings: List<String> = emptyList(),
    ) {
        /** 类的种类数（去重后的规模，报告里比节点数更能说明形状）。 */
        val classCount: Int get() = nodeClasses.distinct().size

        /**
         * 给模型看的正文。
         *
         * 刻意做成"一份能直接抄进 `comfy_submit(overrides=…)` 的清单"：
         * 槽位在前（换模型就改这几个键）、旋钮按种类分组、输出在末尾。
         */
        fun render(name: String? = null): String = buildString {
            append("【工作流配方】")
            append(name ?: "（未命名）")
            append('\n')
            append("任务：").append(task.wire)
            if (task == Task.UNKNOWN) append("（**认不出**）") else append("（按节点形状判的）")
            append("　骨架：").append(skeletonHash)
            append("（").append(nodeClasses.size).append(" 个类 / ").append(edges.size).append(" 条类级线）")
            append('\n')

            append("模型槽位（").append(slots.size).append("）")
            if (slots.isEmpty()) {
                append("：**一个都没认出**")
                append('\n')
            } else {
                append(" —— 换模型/换 LoRA 就改这几个键：\n")
                slots.forEach { s ->
                    append("  · ").append(s.role).append("  ")
                    append(s.overrideKey).append(" = ").append(s.fileName)
                    if (!s.title.isNullOrBlank()) append("　（").append(s.title).append("）")
                    append('\n')
                }
                val guessed = slots.count { it.roleGuessed }
                if (guessed > 0) {
                    append("  （其中 ").append(guessed)
                        .append(" 个的**目录名是猜的**：它们来自 /object_info 现算的自定义加载器，")
                        .append("只知道那里要一个模型文件，不知道该放哪个目录 —— 别拿目录名当结论。）\n")
                }
            }

            if (knobs.isNotEmpty()) {
                append("参数旋钮（").append(knobs.size)
                if (knobsTruncated) append("，只列了前 ").append(MAX_KNOBS).append(" 个")
                append("）：\n")
                KnobKind.entries.forEach { kind ->
                    val group = knobs.filter { it.kind == kind }
                    if (group.isEmpty()) return@forEach
                    append("  · ").append(kindLabel(kind)).append("：")
                    append(
                        group.joinToString(" · ") { k ->
                            val v = if (k.value.length > 80) k.value.take(80) + "…" else k.value
                            "${k.overrideKey}=$v"
                        }
                    )
                    append('\n')
                }
            }

            append("输出：")
            append(
                if (outputs.isEmpty()) "**没有输出节点**（这张图跑完不会留下东西）"
                else outputs.joinToString(" · ") { "#${it.nodeId} ${it.classType}（${it.kind}）" }
            )
            append('\n')
            append("图像输入：").append(imageInputs).append(" 个")
            append("　遮罩链：").append(if (hasMask) "有" else "没有")
            append("　节点数：").append(nodeCount)
            append('\n')

            if (warnings.isNotEmpty()) {
                append("提醒：\n")
                warnings.forEach { append("  · ").append(it).append('\n') }
            }
        }.trimEnd()

        /** 落库 / 进接口的 JSON 形状（键名稳定，界面与 AI 都按它取）。 */
        fun toJson(): JsonObject = buildJsonObject {
            put("task", task.wire)
            put("skeletonHash", skeletonHash)
            put("recipeVersion", VERSION)
            put("nodeCount", nodeCount)
            put("classCount", classCount)
            put("edgeCount", edges.size)
            put("imageInputs", imageInputs)
            put("hasMask", hasMask)
            put("knobsTruncated", knobsTruncated)
            put("nodeClasses", buildJsonArray { nodeClasses.distinct().forEach { add(JsonPrimitive(it)) } })
            put("edges", buildJsonArray { edges.forEach { add(JsonPrimitive(it)) } })
            put(
                "slots",
                buildJsonArray {
                    slots.forEach { s ->
                        add(
                            buildJsonObject {
                                put("role", s.role)
                                put("nodeId", s.nodeId)
                                put("classType", s.classType)
                                put("inputName", s.inputName)
                                put("fileName", s.fileName)
                                put("overrideKey", s.overrideKey)
                                put("roleGuessed", s.roleGuessed)
                                s.title?.let { put("title", it) }
                            }
                        )
                    }
                },
            )
            put(
                "knobs",
                buildJsonArray {
                    knobs.forEach { k ->
                        add(
                            buildJsonObject {
                                put("nodeId", k.nodeId)
                                put("classType", k.classType)
                                put("inputName", k.inputName)
                                put("kind", k.kind.wire)
                                put("value", k.value)
                                put("overrideKey", k.overrideKey)
                                k.title?.let { put("title", it) }
                            }
                        )
                    }
                },
            )
            put(
                "outputs",
                buildJsonArray {
                    outputs.forEach { o ->
                        add(
                            buildJsonObject {
                                put("nodeId", o.nodeId)
                                put("classType", o.classType)
                                put("kind", o.kind)
                            }
                        )
                    }
                },
            )
            put("warnings", buildJsonArray { warnings.forEach { add(JsonPrimitive(it)) } })
        }
    }

    // -----------------------------------------------------------------------
    //  判据表（全部是**显式类名**，不做子串猜测）
    // -----------------------------------------------------------------------

    /**
     * 没有执行语义、或只影响显示的节点。
     *
     * 它们**不进指纹**：一份 113 节点的工作流里，`Note` / `Bookmark` / `Label` / `GetNode` / `SetNode`
     * 这类界面节点能占三分之一（实测：krea2SFWNSFW 那份 41 个类里有 6 类是这样、共 66 个节点）。
     * 把它们算进指纹，"同一族工作流"就永远不会相等 —— 而那正是配方要回答的问题。
     *
     * ⚠ 这只是**兜底**：正常路径上界面节点已经在 `WorkflowConvert` 那一步被展开/摘掉了
     * （见 `AiWorkflowSearch.loadWorkflowFromFile`）。真正进到这里的，是"导出（API）"时
     * 用户自己带上的、或者老捕获记录里残留的。
     */
    private val NON_EXECUTING = setOf(
        "Note", "MarkdownNote", "Reroute", "PrimitiveNode", "GetNode", "SetNode",
        "Bookmark (rgthree)", "Label (rgthree)", "Fast Groups Bypasser (rgthree)",
        "Anything Everywhere", "Anything Everywhere3", "Seed (rgthree)", "easy showAnything",
    )

    /** 只显示、不产出产物的节点（`SaveImage` **不在**这里 —— 它是真输出）。 */
    private val DISPLAY_ONLY = setOf(
        "PreviewImage", "PreviewAny", "MaskPreview+", "Image Comparer (rgthree)",
        "CR Simple Image Compare", "ImageCompare", "ShowText|pysssss", "easy showAnything",
        "WidgetToString", "SystemNotification|pysssss", "FancyTimerNode",
    )

    /**
     * 从磁盘读一张图的节点（含遮罩图 —— 它也是一路图像输入）。
     *
     * ⚠ 刻意**不含** `ImageOnlyCheckpointLoader`（它是模型加载器，不是"图从外部来"）
     * 也不含 `LoadAudio`（那是音频输入）：把它们算进来会把 t2i 误判成 i2i。
     */
    private val IMAGE_LOADERS = setOf(
        "LoadImage", "LoadImageMask", "LoadImageOutput", "LoadImagesFromDirectory",
        "VHS_LoadVideo", "LoadVideo", "LoadImageSetNode",
    )

    /** "这里有一张遮罩在决定重绘区"的显式节点。 */
    private val MASK_NODES = setOf(
        "LoadImageMask", "ImageToMask", "MaskToImage", "SetLatentNoiseMask",
        "InpaintModelConditioning", "InpaintModelConditioningAdvanced", "GrowMask", "MaskBlur",
        "FeatherMask", "SolidMask", "MaskComposite", "DifferentialDiffusion",
    )

    /**
     * **参考图编辑**链的显式节点。
     *
     * 名单来自用户机器上真有的工作流（只读扫过，见设计 §5），**逐字对着真文件的
     * `class_type` 抄的** —— 第一版照着统计表写成了 `…Editx2`（那是"出现 2 次"的后缀），
     * 结果 Krea 2 的两份编辑工作流被归成了 `inpaint` / `i2i`（探针当场照出来的）：
     * - Krea 2：`Krea2EditModelPatch` / `Krea2OstrisEditModelPatch` / `Krea2EditGroundedEncode` /
     *   `TextEncodeKrea2OstrisEdit` / `FluxKontextMultiReferenceLatentMethod`
     * - Flux Kontext / Qwen-Edit 一类：`ReferenceLatent` / `KontextSwitch` / `TextEncodeQwenImageEdit`
     */
    private val EDIT_NODES = setOf(
        "Krea2EditModelPatch", "Krea2OstrisEditModelPatch", "Krea2EditGroundedEncode",
        "TextEncodeKrea2OstrisEdit", "FluxKontextMultiReferenceLatentMethod",
        "ReferenceLatent", "KontextSwitch",
        "TextEncodeQwenImageEdit", "TextEncodeQwenImageEditPlus",
    )

    /** 输出节点 → 产物类别。 */
    private val OUTPUT_KINDS: Map<String, String> = mapOf(
        "SaveImage" to "image", "SaveImageWithMetadata" to "image", "ImageSave" to "image",
        "SaveVideo" to "video", "VHS_VideoCombine" to "video", "SaveWEBM" to "video",
        "SaveAnimatedWEBP" to "video", "SaveAnimatedPNG" to "video",
        "SaveAudio" to "audio", "SaveAudioAdvanced" to "audio", "SaveAudioMP3" to "audio",
        "SaveAudioOpus" to "audio", "PreviewAudio" to "audio",
    )

    /** 一看就是视频链的类名（**子串**匹配是刻意的：第三方视频节点前缀换得很勤）。 */
    private val VIDEO_HINTS = listOf(
        "SaveVideo", "VideoCombine", "VHS_", "WanImageToVideo", "WanVideo", "WanFirstLastFrame",
        "MiniMaxH3", "MiniMax", "HunyuanVideo", "LTXV", "LTXVideo", "Mochi", "CogVideo",
        "SVD_", "SVDImg2Vid", "ImageToVideo", "Kling", "Seedance", "Veo",
    )

    /** 音频链的类名子串。 */
    private val AUDIO_HINTS = listOf("SaveAudio", "PreviewAudio", "MiniMaxMusic", "MusicGen", "AudioEncoder")

    /** 放大链（"没有采样器 + 有放大模型" = upscale）。 */
    private val UPSCALE_NODES = setOf(
        "UpscaleModelLoader", "ImageUpscaleWithModel", "UpscaleImage", "UpscaleImageByUsingModel",
        "ImageScaleBy", "UltimateSDUpscale", "ImageScale",
    )

    /** 采样器（判 upscale 用 + 报告里给模型看的"有没有采样"）。 */
    private fun looksLikeSampler(cls: String): Boolean =
        cls == "KSampler" || cls == "KSamplerAdvanced" || cls.contains("SamplerCustom") ||
            cls.contains("KSampler") || cls.contains("Sampler_") || cls == "SamplerCustom" ||
            cls == "ClownsharKSampler_Beta" || cls.contains("Sampler")

    /** 文本编码节点（这些类上的 `text` 就是提示词）。 */
    private val PROMPT_CLASSES = setOf(
        "CLIPTextEncode", "CLIPTextEncodeSDXL", "CLIPTextEncodeSDXLRefiner", "CLIPTextEncodeFlux",
        "TextEncodeKrea2OstrisEditx2", "TextEncodeQwenImageEdit", "TextEncodeQwenImageEditPlus",
        "T5TextEncode", "TextInput_", "PrimitiveStringMultiline", "String Literal",
        "BNK_CLIPTextEncodeAdvanced", "smZ CLIPTextEncode", "easy text",
    )

    private val SAMPLING_INPUTS = setOf(
        "steps", "cfg", "sampler_name", "scheduler", "denoise", "guidance", "shift",
        "start_at_step", "end_at_step", "add_noise", "return_with_leftover_noise", "sigma_max",
        "sigma_min", "eta", "noise_type", "sampler", "sigmas",
    )

    private val SEED_INPUTS = setOf("seed", "noise_seed", "rand_seed", "seed_num")

    private val SIZE_INPUTS = setOf(
        "width", "height", "batch_size", "resolution", "megapixels", "aspect_ratio",
        "target_width", "target_height", "scale_by",
    )

    private val IMAGE_INPUTS = setOf("image", "images", "mask", "masks", "image1", "image2", "image_b", "video")

    /** 加载器节点（它上面那些非文件输入 = [KnobKind.LOADER]）。 */
    private fun looksLikeLoader(cls: String): Boolean =
        cls.contains("Loader") || cls.contains("Load") && !cls.contains("LoadImage")

    // -----------------------------------------------------------------------
    //  归纳
    // -----------------------------------------------------------------------

    /**
     * 归纳一份 **API 格式**节点图。
     *
     * @param graph API 节点图（`{"<id>": {"class_type": …, "inputs": {…}}}`）。
     *   界面格式（`nodes`/`links`）**不能**直接传进来 —— 先用
     *   `comfy_load_workflow` / `WorkflowConvert.toApiGraph` 转（子图展开、旁路、前端模拟都在那一步）。
     * @param modelInputs 可选：`/object_info` 现算的"哪个类哪个输入是模型枚举"
     *   （见 `ComfyModels.enumerateModelInputs`）。为 null 时只用内置那张表，并**如实写在 warnings 里**
     *   —— 自定义加载器的槽位会漏报，模型必须知道这一点。
     */
    fun analyze(graph: JsonObject, modelInputs: Map<String, Set<String>>? = null): Recipe {
        val nodes = graph.entries.mapNotNull { (id, raw) ->
            val node = raw as? JsonObject ?: return@mapNotNull null
            val cls = (node["class_type"] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
                ?: return@mapNotNull null
            val inputs = node["inputs"] as? JsonObject ?: JsonObject(emptyMap())
            val title = ((node["_meta"] as? JsonObject)?.get("title") as? JsonPrimitive)?.contentOrNull
            Node(id, cls, inputs, title)
        }

        val exec = nodes.filter { it.cls !in NON_EXECUTING }
        val byId = exec.associateBy { it.id }

        // --- 骨架：执行相关类（带重数）+ 类级连线（带重数）---------------
        val fingerprintClasses = exec.filter { it.cls !in DISPLAY_ONLY }.map { it.cls }
        val edges = mutableListOf<String>()
        exec.forEach { node ->
            // 纯显示节点的**出边**也不算：`Image Comparer` / `PreviewImage` 会挂到 SaveImage 上，
            // 把它们算进来，"多插了一个对比节点"就会换指纹（而它一个字都不影响这次跑的结果）
            if (node.cls in DISPLAY_ONLY) return@forEach
            node.inputs.forEach { (name, value) ->
                val link = value as? JsonArray ?: return@forEach
                val upstreamId = (link.getOrNull(0) as? JsonPrimitive)?.contentOrNull ?: return@forEach
                val slot = (link.getOrNull(1) as? JsonPrimitive)?.contentOrNull ?: "0"
                val upstream = byId[upstreamId] ?: return@forEach
                if (upstream.cls in DISPLAY_ONLY) return@forEach
                edges += "${node.cls}.$name<-${upstream.cls}[$slot]"
            }
        }
        val sortedClasses = fingerprintClasses.sorted()
        val sortedEdges = edges.sorted()

        val outputs = exec.mapNotNull { node ->
            OUTPUT_KINDS[node.cls]?.let { Output(node.id, node.cls, it) }
        }
        // 槽位两遍算：`modelInputs` 那一遍是**全量**（含 /object_info 现算的自定义加载器），
        // `null` 那一遍只走内置确定表 —— 差集就是"目录名是猜的"那些（见 Slot.roleGuessed）
        val allRequirements = ComfyModels.requirements(graph, modelInputs)
        val knownSlotKeys = ComfyModels.requirements(graph, null).map { it.overrideKey }.toSet()
        val slotKeys = allRequirements.map { it.overrideKey }.toSet()
        val slots = allRequirements.map { req ->
            val node = byId[req.nodeId]
            Slot(
                role = req.folder,
                nodeId = req.nodeId,
                classType = req.classType,
                inputName = req.inputName,
                fileName = req.fileName,
                title = node?.title ?: req.title,
                roleGuessed = req.overrideKey !in knownSlotKeys,
            )
        }
        val imageInputs = exec.count { it.cls in IMAGE_LOADERS }
        val hasMask = exec.any { it.cls in MASK_NODES } ||
            exec.any { node -> node.inputs["mask"] != null }

        // 模型槽位的键不进旋钮：它们在 slots 里已经有一份（重复列只会让两边对不上时更难查）
        val allKnobs = mutableListOf<Knob>()
        exec.forEach { node ->
            node.inputs.forEach { (name, value) ->
                if ("${node.id}.$name" in slotKeys) return@forEach
                val prim = value as? JsonPrimitive ?: return@forEach
                if (prim is JsonNull) return@forEach
                val text = prim.contentOrNull ?: return@forEach
                allKnobs += Knob(
                    nodeId = node.id,
                    classType = node.cls,
                    inputName = name,
                    kind = knobKind(node.cls, name, text),
                    value = if (text.length > MAX_KNOB_VALUE) text.take(MAX_KNOB_VALUE) + "…" else text,
                    title = node.title,
                )
            }
        }
        // 稳定排序：先按种类（提示词 / 图像 / 采样 / 尺寸 / 种子 / 加载器 / 其它），同种内保持文件顺序
        val ordered = allKnobs.sortedBy { KnobKind.entries.indexOf(it.kind) }
        val knobsTruncated = ordered.size > MAX_KNOBS
        val knobs = ordered.take(MAX_KNOBS)

        val task = classify(exec.map { it.cls }, outputs, imageInputs, hasMask)
        val warnings = buildList {
            if (nodes.isEmpty()) add("这张图里一个节点都没有（空图）：要么文件选错了，要么转换时被摘空了。")
            if (task == Task.UNKNOWN && nodes.isNotEmpty()) {
                add(
                    "认不出这是什么任务（既没有输出节点、也没有图像输入）—— 配方里 task=unknown 是**如实回答**，" +
                        "不要当成某一类去用。",
                )
            } else if (task == Task.EDIT || task == Task.I2I) {
                add("task 是按节点形状判的（${task.wire}）：edit 与 i2i 的边界是连续谱，拿不准就按槽位与旋钮自己复核。")
            }
            if (modelInputs == null) {
                add(
                    "拿不到 ComfyUI 的 /object_info：**自定义加载器**的模型槽位可能漏报" +
                        "（内置那张表只覆盖常见节点）。要对齐模型就用 comfy_fit_workflow 再核一遍。",
                )
            } else if (slots.isEmpty() && exec.any { looksLikeLoader(it.cls) }) {
                add("图里有加载器节点，但没有任何**已知**的模型槽位：可能是自定义加载器，别自己编文件名。")
            }
            if (knobsTruncated) {
                add("旋钮只列了前 $MAX_KNOBS 个（按种类优先），要看全就用 comfy_get_run / includeGraph 拿原图。")
            }
            if (exec.isEmpty() && nodes.isNotEmpty()) {
                add("图里只有界面节点（Note / Reroute 之类），没有可执行内容。")
            }
        }.map { it.take(MAX_WARNING_CHARS) }

        return Recipe(
            task = task,
            skeletonHash = skeletonHash(sortedClasses, sortedEdges),
            nodeClasses = sortedClasses,
            edges = sortedEdges,
            slots = slots,
            knobs = knobs,
            outputs = outputs,
            imageInputs = imageInputs,
            hasMask = hasMask,
            nodeCount = nodes.size,
            knobsTruncated = knobsTruncated,
            warnings = warnings,
        )
    }

    /**
     * 骨架指纹：`sha256("v<规则版本>|<类集合>|<类级连线>")` 取前 16 位十六进制。
     *
     * 刻意**不含**：节点 id、模型文件名、参数值、`_meta` 标题、纯界面/显示节点。
     * 含**重数**（两个 `LoraLoader` 与一个的骨架不是同一个形状）。
     */
    fun skeletonHash(sortedClasses: List<String>, sortedEdges: List<String>): String {
        val text = "v$VERSION|" + sortedClasses.joinToString(",") + "|" + sortedEdges.joinToString(";")
        val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
        return digest.take(8).joinToString("") { "%02x".format(it) }
    }

    /**
     * 任务分类（顺序即优先级，见 [Task] 各档注释）。
     *
     * ⚠ 顺序不能随手改：`audio`/`video` 必须在最前（输出即事实），
     * `edit` 必须在 `inpaint` 前（Krea 2 的 Identity Edit 两条链都有）。
     *
     * @param hasMask 与 [Recipe.hasMask] **同一个判据**（`analyze` 传同一个值进来）：
     *   两处各算一遍就会出现"报告说'遮罩链：有'、任务却判成 i2i"这种自相矛盾
     *   （真机上实测到过：`MiaoMiao Harem.json` 就是这么被判成 i2i 的）。
     */
    fun classify(classes: List<String>, outputs: List<Output>, imageInputs: Int, hasMask: Boolean): Task {
        val set = classes.toSet()
        val hasImageOut = outputs.any { it.kind == "image" }
        if (outputs.any { it.kind == "audio" } || classes.any { c -> AUDIO_HINTS.any { c.contains(it) } }) {
            return Task.AUDIO
        }
        // 视频：**输出是事实**；只有"有视频节点 + 一个图像输出都没有"才用节点名兜底。
        // 这一条是刻意加的：MiniMax H3 这类视频模型也有"只出一张图"的工作流，
        // 光看节点名会把出图的工作流判成 video，模型就会按"要等视频"去用。
        if (outputs.any { it.kind == "video" } ||
            (!hasImageOut && classes.any { c -> VIDEO_HINTS.any { c.contains(it) } })
        ) {
            return Task.VIDEO
        }
        if (set.any { it in EDIT_NODES }) return Task.EDIT
        if (hasMask) return Task.INPAINT
        val hasSampler = classes.any { looksLikeSampler(it) }
        if (!hasSampler && set.any { it in UPSCALE_NODES }) return Task.UPSCALE
        if (imageInputs >= 1) return Task.I2I
        if (outputs.isNotEmpty()) return Task.T2I
        return Task.UNKNOWN
    }

    private fun knobKind(cls: String, inputName: String, value: String): KnobKind = when {
        // 叫 `text` / `prompt` 的输入一律当提示词 —— **不看节点类**：
        // 自定义编辑节点（`Krea2EditGroundedEncode.text`）的正文也在这种输入里，
        // 只按类名认的话这些工作流的提示词旋钮会**一个都列不出来**（真机探针照出来的）。
        inputName in setOf("text", "prompt") -> KnobKind.PROMPT
        cls in PROMPT_CLASSES && inputName in setOf("string", "value", "positive", "negative") -> KnobKind.PROMPT
        inputName in IMAGE_INPUTS && looksLikeFileName(value) -> KnobKind.IMAGE
        inputName in SAMPLING_INPUTS -> KnobKind.SAMPLING
        inputName in SEED_INPUTS || inputName.startsWith("seed") -> KnobKind.SEED
        inputName in SIZE_INPUTS -> KnobKind.SIZE
        looksLikeLoader(cls) -> KnobKind.LOADER
        else -> KnobKind.OTHER
    }

    /**
     * "这个字面量看起来像一个文件名"。
     *
     * 只在**已知是文件语义的输入名**（[IMAGE_INPUTS]）上用它，所以判据可以宽松：
     * 带后缀，或者像 ComfyUI 的 input 路径（`clipspace/xxx.png [input]` / `pasted/xxx.png`）。
     */
    private fun looksLikeFileName(value: String): Boolean {
        if (value.isBlank()) return false
        val lower = value.lowercase()
        if (lower.startsWith("clipspace/") || lower.startsWith("pasted/")) return true
        return FILE_EXTENSIONS.any { lower.endsWith(it) } || lower.contains(".png [input]") ||
            lower.contains(".jpg [input]") || lower.contains(".webp [input]")
    }

    private val FILE_EXTENSIONS = listOf(
        ".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif", ".tiff", ".tif", ".exr",
        ".mp4", ".webm", ".mkv", ".mov", ".avi", ".gif", ".wav", ".mp3", ".flac", ".ogg",
    )

    /** 旋钮分组在 [Recipe.render] 里的中文标签。 */
    private fun kindLabel(kind: KnobKind): String = when (kind) {
        KnobKind.PROMPT -> "提示词"
        KnobKind.IMAGE -> "图像/遮罩"
        KnobKind.SAMPLING -> "采样"
        KnobKind.SEED -> "种子"
        KnobKind.SIZE -> "尺寸"
        KnobKind.LOADER -> "加载器参数（换模型族时也要看）"
        KnobKind.OTHER -> "其它"
    }

    /** 归纳时用的中间结构（不对外）。 */
    private data class Node(
        val id: String,
        val cls: String,
        val inputs: JsonObject,
        val title: String?,
    )
}
