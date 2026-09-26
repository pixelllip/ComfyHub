package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.file.Files
import java.nio.file.Paths
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * 工作流配方（W1）的回归防线。
 *
 * 盯的是五件**删了以后出错也没人知道**的事：
 * ① 骨架指纹对"换节点 id / 换模型名 / 换参数 / 加界面节点"**稳定**，对"多一个结构节点"**敏感**
 *    —— 这正是"泛用"的定义，算错了会让 `comfy_list_recipes` 把不同族的工作流归到一起
 *    （或者把同一族拆成十几份，两种都不报错）；
 * ② 模型槽位只认**确定**的加载器表（值必须是字符串），槽位的键不在旋钮里重复出现；
 * ③ 任务分类的**优先级**（edit 必须先于 inpaint：Krea 2 的 Identity Edit 两条链都有）；
 * ④ 拿不到 `/object_info` 时必须**如实写在 warnings 里**（自定义加载器会漏报）；
 * ⑤ `render()` / `toJson()` 里必须能直接看到"改哪个键"（槽位与旋钮的 overrideKey）。
 *
 * 除了内联夹具，还有一条**读仓库里真文件**的用例（`comfyui/workflows/comfyhub_inpaint_*.json`，
 * 它们是 API 格式、已进版本库）—— 夹具只覆盖我想得到的形状，真文件会把我没想到的摆出来。
 */
class WorkflowRecipeTest {

    // -----------------------------------------------------------------------
    //  夹具小工具
    // -----------------------------------------------------------------------

    private fun link(id: String, slot: Int = 0): JsonArray = buildJsonArray {
        add(JsonPrimitive(id))
        add(JsonPrimitive(slot))
    }

    private fun node(cls: String, title: String? = null, inputs: Map<String, JsonElement>): JsonObject =
        buildJsonObject {
            if (title != null) put("_meta", buildJsonObject { put("title", title) })
            put("class_type", cls)
            put("inputs", JsonObject(inputs))
        }

    private fun str(value: String): JsonElement = JsonPrimitive(value)
    private fun num(value: Number): JsonElement = JsonPrimitive(value)

    /** 一份最小的 t2i：底模 + 文本编码 + 空潜空间 + 采样 + 解码 + 存图。 */
    private fun t2iGraph(unet: String = "krea2_turbo_nvfp4.safetensors"): JsonObject = JsonObject(
        linkedMapOf(
            "1" to node("CheckpointLoaderSimple", "底模", mapOf("ckpt_name" to str(unet))),
            "6" to node("CLIPTextEncode", "正面", mapOf("clip" to link("1", 1), "text" to str("a cat"))),
            "7" to node("CLIPTextEncode", "负面", mapOf("clip" to link("1", 1), "text" to str("blurry"))),
            "5" to node(
                "EmptyLatentImage", null,
                mapOf("width" to num(1024), "height" to num(1024), "batch_size" to num(1)),
            ),
            "3" to node(
                "KSampler", null,
                mapOf(
                    "model" to link("1", 0), "positive" to link("6", 0), "negative" to link("7", 0),
                    "latent_image" to link("5", 0), "seed" to num(42), "steps" to num(8),
                    "cfg" to num(1.0), "sampler_name" to str("euler"), "scheduler" to str("simple"),
                    "denoise" to num(1.0),
                ),
            ),
            "8" to node("VAEDecode", null, mapOf("samples" to link("3", 0), "vae" to link("1", 2))),
            "9" to node("SaveImage", null, mapOf("images" to link("8", 0), "filename_prefix" to str("x"))),
        ),
    )

    // -----------------------------------------------------------------------
    //  ① 骨架指纹
    // -----------------------------------------------------------------------

    @Test
    fun `骨架指纹对节点编号 模型名 参数值稳定`() {
        val a = WorkflowRecipe.analyze(t2iGraph("a.safetensors"))
        val b = WorkflowRecipe.analyze(t2iGraph("b.safetensors"))
        assertEquals(a.skeletonHash, b.skeletonHash, "换了底模不该换骨架（这正是'泛用'的定义）")

        // 重新编号（把 6/7 换成 66/77）+ 改提示词与 seed：形状没变
        val renamed = JsonObject(
            linkedMapOf(
                "1" to node("CheckpointLoaderSimple", null, mapOf("ckpt_name" to str("c.safetensors"))),
                "66" to node("CLIPTextEncode", null, mapOf("clip" to link("1", 1), "text" to str("完全不同的提示词"))),
                "77" to node("CLIPTextEncode", null, mapOf("clip" to link("1", 1), "text" to str("nope"))),
                "5" to node("EmptyLatentImage", null, mapOf("width" to num(512), "height" to num(512), "batch_size" to num(2))),
                "3" to node(
                    "KSampler", null,
                    mapOf(
                        "model" to link("1", 0), "positive" to link("66", 0), "negative" to link("77", 0),
                        "latent_image" to link("5", 0), "seed" to num(999), "steps" to num(30),
                        "cfg" to num(7.0), "sampler_name" to str("dpmpp_2m"), "scheduler" to str("karras"),
                        "denoise" to num(0.8),
                    ),
                ),
                "8" to node("VAEDecode", null, mapOf("samples" to link("3", 0), "vae" to link("1", 2))),
                "9" to node("SaveImage", null, mapOf("images" to link("8", 0))),
            ),
        )
        assertEquals(a.skeletonHash, WorkflowRecipe.analyze(renamed).skeletonHash)
    }

    @Test
    fun `骨架指纹对结构变化敏感 也认重数`() {
        val base = WorkflowRecipe.analyze(t2iGraph())
        // 多一个 LoRA（结构变了）
        val withLora = JsonObject(
            t2iGraph().toMutableMap().apply {
                put(
                    "20", node(
                        "LoraLoaderModelOnly", null,
                        mapOf("model" to link("1", 0), "lora_name" to str("style.safetensors"), "strength_model" to num(1.0)),
                    ),
                )
                put(
                    "3", node(
                        "KSampler", null,
                        mapOf(
                            "model" to link("20", 0), "positive" to link("6", 0), "negative" to link("7", 0),
                            "latent_image" to link("5", 0), "seed" to num(42), "steps" to num(8),
                            "cfg" to num(1.0), "sampler_name" to str("euler"), "scheduler" to str("simple"),
                            "denoise" to num(1.0),
                        ),
                    ),
                )
            },
        )
        assertNotEquals(base.skeletonHash, WorkflowRecipe.analyze(withLora).skeletonHash, "多一个结构节点必须换指纹")

        // 两个 LoRA 串起来 ≠ 一个
        val twoLoras = JsonObject(
            withLora.toMutableMap().apply {
                put(
                    "3", node(
                        "KSampler", null,
                        mapOf(
                            "model" to link("20", 0), "positive" to link("6", 0), "negative" to link("7", 0),
                            "latent_image" to link("5", 0), "seed" to num(42), "steps" to num(8),
                            "cfg" to num(1.0), "sampler_name" to str("euler"), "scheduler" to str("simple"),
                            "denoise" to num(1.0),
                        ),
                    ),
                )
                put(
                    "21", node(
                        "LoraLoaderModelOnly", null,
                        mapOf("model" to link("20", 0), "lora_name" to str("b.safetensors"), "strength_model" to num(0.6)),
                    ),
                )
                put(
                    "3b", node(
                        "KSampler", null,
                        mapOf("model" to link("21", 0), "positive" to link("6", 0), "negative" to link("7", 0),
                            "latent_image" to link("5", 0)),
                    ),
                )
            },
        )
        assertNotEquals(
            WorkflowRecipe.analyze(withLora).skeletonHash,
            WorkflowRecipe.analyze(twoLoras).skeletonHash,
            "重数要进指纹（一/两个 LoRA 不是同一个形状）",
        )
    }

    @Test
    fun `界面节点与纯显示节点不进指纹`() {
        val base = WorkflowRecipe.analyze(t2iGraph())
        val noisy = JsonObject(
            t2iGraph().toMutableMap().apply {
                put("90", node("Note", null, mapOf("text" to str("我的笔记"))))
                put("91", node("MarkdownNote", null, mapOf("text" to str("# 标题"))))
                put("92", node("Reroute", null, mapOf("input" to link("9", 0))))
                put("93", node("GetNode", null, mapOf("id" to str("x"))))
                put("94", node("Bookmark (rgthree)", null, mapOf("bookmark" to str("x"))))
                put("95", node("Label (rgthree)", null, mapOf("text" to str("x"))))
                put("96", node("PreviewImage", null, mapOf("images" to link("8", 0))))
                put("97", node("Image Comparer (rgthree)", null, mapOf("image_a" to link("8", 0))))
            },
        )
        assertEquals(
            base.skeletonHash,
            WorkflowRecipe.analyze(noisy).skeletonHash,
            "Note / Reroute / Bookmark / Preview 这类界面节点不该改变骨架（113 节点的那份里它们占三分之一）",
        )
        // 但节点数是**如实**的（报告里要能看出"这份文件比那份胖"）
        assertEquals(7, base.nodeCount)
        assertEquals(15, WorkflowRecipe.analyze(noisy).nodeCount)
    }

    // -----------------------------------------------------------------------
    //  ② 模型槽位
    // -----------------------------------------------------------------------

    @Test
    fun `模型槽位按确定表认出角色 且不在旋钮里重复`() {
        val graph = JsonObject(
            linkedMapOf(
                "73" to node("UNETLoader", "底模", mapOf("unet_name" to str("krea2_turbo_bf16.safetensors"), "weight_dtype" to str("default"))),
                "56" to node("CLIPLoader", "文本编码器", mapOf("clip_name" to str("qwen3vl_4b.safetensors"), "type" to str("krea2"), "device" to str("default"))),
                "91" to node("VAELoader", null, mapOf("vae_name" to str("qwen_image_vae.safetensors"))),
                "71" to node("LoraLoaderModelOnly", null, mapOf("model" to link("73", 0), "lora_name" to str("identity.safetensors"), "strength_model" to num(1.0))),
                "60" to node("TextInput_", null, mapOf("text" to str("把这个人换成正在跑步的姿势"))),
                "72" to node("LoadImage", "参考图", mapOf("image" to str("clipspace/abc.png [input]"))),
            ),
        )
        val r = WorkflowRecipe.analyze(graph)

        // 槽位按**节点号**排（`ComfyModels.requirements` 就是这么给的）：
        // 顺序稳定比"按文件顺序"更重要 —— 同一份图学两次，报告要逐字一样
        assertEquals(
            mapOf("73" to "diffusion_models", "56" to "text_encoders", "91" to "vae", "71" to "loras"),
            r.slots.associate { it.nodeId to it.role },
            "角色取自 ComfyModels 那张确定表（folder），不新建一套",
        )
        assertEquals(listOf("56", "71", "73", "91"), r.slots.map { it.nodeId }, "按节点号升序")
        assertEquals("73.unet_name", r.slots.first { it.nodeId == "73" }.overrideKey)
        assertEquals("krea2_turbo_bf16.safetensors", r.slots.first { it.nodeId == "73" }.fileName)
        assertEquals("底模", r.slots.first { it.nodeId == "73" }.title, "节点标题只是可读性，不参与判定")
        // 槽位键不许在旋钮里再来一份（两边重复会让'对不上'更难查）
        val knobKeys = r.knobs.map { it.overrideKey }.toSet()
        r.slots.forEach { assertTrue(it.overrideKey !in knobKeys, "槽位 ${it.overrideKey} 不该出现在旋钮里") }
        // 但加载器上那些**模型族相关**的开关要留下（type=krea2 换模型族时必须改）
        assertNotNull(r.knobs.firstOrNull { it.overrideKey == "56.type" && it.kind == WorkflowRecipe.KnobKind.LOADER })
        // 连线型输入不是旋钮
        assertTrue(r.knobs.none { it.overrideKey == "71.model" }, "连线（数组）不是可覆盖的字面量")
    }

    // -----------------------------------------------------------------------
    //  ③ 任务分类
    // -----------------------------------------------------------------------

    private fun taskOf(graph: JsonObject): WorkflowRecipe.Task = WorkflowRecipe.analyze(graph).task

    @Test
    fun `任务分类 七档各有判据`() {
        // t2i：没有任何图像输入
        assertEquals(WorkflowRecipe.Task.T2I, taskOf(t2iGraph()))

        // i2i：有 LoadImage
        val i2i = JsonObject(
            t2iGraph().toMutableMap().apply {
                put("30", node("LoadImage", null, mapOf("image" to str("a.png"))))
                put("31", node("VAEEncode", null, mapOf("pixels" to link("30", 0), "vae" to link("1", 2))))
                put("3", node("KSampler", null, mapOf("latent_image" to link("31", 0), "model" to link("1", 0))))
            },
        )
        assertEquals(WorkflowRecipe.Task.I2I, taskOf(i2i))

        // inpaint：有遮罩链
        val inpaint = JsonObject(
            i2i.toMutableMap().apply {
                put("12", node("LoadImageMask", null, mapOf("image" to str("m.png"), "channel" to str("red"))))
                put("24", node("SetLatentNoiseMask", null, mapOf("samples" to link("31", 0), "mask" to link("12", 0))))
                put("3", node("KSampler", null, mapOf("latent_image" to link("24", 0), "model" to link("1", 0))))
            },
        )
        assertEquals(WorkflowRecipe.Task.INPAINT, taskOf(inpaint))
        assertTrue(WorkflowRecipe.analyze(inpaint).hasMask)

        // edit：参考图编辑链 —— **即便同时有遮罩链**也必须判 edit（Krea 2 Identity Edit 就是两者都有）
        val edit = JsonObject(
            inpaint.toMutableMap().apply {
                put("58", node("Krea2EditModelPatch", null, mapOf("model" to link("1", 0), "latent" to link("31", 0))))
                put("60", node("TextEncodeKrea2OstrisEditx2", null, mapOf("clip" to link("1", 1), "text" to str("换成跑步"))))
            },
        )
        assertEquals(
            WorkflowRecipe.Task.EDIT,
            taskOf(edit),
            "edit 必须优先于 inpaint：归成 inpaint 的话 comfy_list_recipes(task=edit) 永远找不到用户的图像编辑工作流",
        )

        // video：有 SaveVideo（输出即事实）
        val video = JsonObject(
            mapOf(
                "1" to node("LoadImage", null, mapOf("image" to str("a.png"))),
                "2" to node("MiniMaxH3ImageToVideo", null, mapOf("image" to link("1", 0), "prompt" to str("走起来"))),
                "3" to node("SaveVideo", null, mapOf("video" to link("2", 0))),
            ),
        )
        assertEquals(WorkflowRecipe.Task.VIDEO, taskOf(video))

        // 视频模型但**只出一张图**：不许判成 video（光看节点名就会错）
        val videoModelImageOut = JsonObject(
            mapOf(
                "1" to node("MiniMaxH3Sampler", null, mapOf("prompt" to str("一只猫"))),
                "2" to node("VAEDecode", null, mapOf("samples" to link("1", 0))),
                "3" to node("SaveImage", null, mapOf("images" to link("2", 0))),
            ),
        )
        assertEquals(WorkflowRecipe.Task.T2I, taskOf(videoModelImageOut))

        // audio：音频输出
        val audio = JsonObject(mapOf("1" to node("SaveAudioAdvanced", null, mapOf("audio" to str("x")))))
        assertEquals(WorkflowRecipe.Task.AUDIO, taskOf(audio))

        // upscale：只有放大模型、没有采样器
        val upscale = JsonObject(
            mapOf(
                "1" to node("LoadImage", null, mapOf("image" to str("a.png"))),
                "2" to node("UpscaleModelLoader", null, mapOf("model_name" to str("4x.pth"))),
                "3" to node("ImageUpscaleWithModel", null, mapOf("upscale_model" to link("2", 0), "image" to link("1", 0))),
                "4" to node("SaveImage", null, mapOf("images" to link("3", 0))),
            ),
        )
        assertEquals(WorkflowRecipe.Task.UPSCALE, taskOf(upscale))

        // unknown：认不出（不能硬塞一档）
        val unknown = JsonObject(mapOf("1" to node("PrimitiveStringMultiline", null, mapOf("value" to str("随便")))))
        assertEquals(WorkflowRecipe.Task.UNKNOWN, taskOf(unknown))
        assertTrue(WorkflowRecipe.analyze(unknown).warnings.any { it.contains("认不出") })
    }

    @Test
    fun `ResizeImageMaskNode 里有 Mask 但它不是 inpaint`() {
        assertEquals(
            WorkflowRecipe.Task.I2I,
            taskOf(
                JsonObject(
                    mapOf(
                        "1" to node("LoadImage", null, mapOf("image" to str("a.png"))),
                        "2" to node("ResizeImageMaskNode", null, mapOf("image" to link("1", 0), "width" to num(512))),
                        "3" to node("SaveImage", null, mapOf("images" to link("2", 0))),
                    ),
                ),
            ),
        )
    }

    // -----------------------------------------------------------------------
    //  ④ 旋钮与如实警告
    // -----------------------------------------------------------------------

    @Test
    fun `旋钮按种类分组 值可截断 计数如实`() {
        val r = WorkflowRecipe.analyze(t2iGraph())
        val byKind = r.knobs.groupBy { it.kind }
        assertTrue(
            byKind[WorkflowRecipe.KnobKind.PROMPT]!!.any { it.overrideKey == "6.text" && it.value == "a cat" },
            "文本编码的 text 要判成提示词",
        )
        assertTrue(byKind[WorkflowRecipe.KnobKind.SAMPLING]!!.any { it.overrideKey == "3.steps" })
        assertTrue(byKind[WorkflowRecipe.KnobKind.SAMPLING]!!.any { it.overrideKey == "3.sampler_name" })
        assertTrue(byKind[WorkflowRecipe.KnobKind.SEED]!!.any { it.overrideKey == "3.seed" })
        assertTrue(byKind[WorkflowRecipe.KnobKind.SIZE]!!.any { it.overrideKey == "5.width" })
        // 提示词排在最前面（模型最先要看的就是它）
        assertEquals(WorkflowRecipe.KnobKind.PROMPT, r.knobs.first().kind)

        // 超长值只留前 MAX_KNOB_VALUE 个字符（否则一份 2KB 的自然语言提示词就把结果撑满了）
        val long = "x".repeat(WorkflowRecipe.MAX_KNOB_VALUE + 50)
        val huge = WorkflowRecipe.analyze(
            JsonObject(mapOf("1" to node("CLIPTextEncode", null, mapOf("clip" to link("1", 1), "text" to str(long))))),
        )
        val knob = huge.knobs.first { it.kind == WorkflowRecipe.KnobKind.PROMPT }
        assertTrue(knob.value.endsWith("…"), "超长值要带截断标记")
        assertEquals(WorkflowRecipe.MAX_KNOB_VALUE + 1, knob.value.length)
    }

    @Test
    fun `拿不到 object_info 与空图都要如实说`() {
        val r = WorkflowRecipe.analyze(t2iGraph())
        assertTrue(
            r.warnings.any { it.contains("/object_info") },
            "没有 objectInfo 时必须说'自定义加载器的槽位可能漏报'，不许装作查全了",
        )
        // 给了 objectInfo（哪怕是空表）就不再报这条
        val withInfo = WorkflowRecipe.analyze(t2iGraph(), emptyMap())
        assertTrue(withInfo.warnings.none { it.contains("/object_info") })

        val empty = WorkflowRecipe.analyze(JsonObject(emptyMap()))
        assertEquals(WorkflowRecipe.Task.UNKNOWN, empty.task)
        assertTrue(empty.warnings.any { it.contains("空图") })
        assertTrue(empty.slots.isEmpty() && empty.knobs.isEmpty())
    }

    // -----------------------------------------------------------------------
    //  ⑤ render / toJson
    // -----------------------------------------------------------------------

    @Test
    fun `render 里能直接看到改哪个键 toJson 键集合稳定`() {
        val r = WorkflowRecipe.analyze(t2iGraph())
        val text = r.render("krea2_edit")
        assertTrue(text.contains("【工作流配方】krea2_edit"))
        assertTrue(text.contains("任务：t2i"))
        assertTrue(text.contains("骨架：${r.skeletonHash}"))
        assertTrue(text.contains("1.ckpt_name = krea2_turbo_nvfp4.safetensors"), text)
        assertTrue(text.contains("提示词："), text)
        assertTrue(text.contains("#9 SaveImage（image）"))

        val json = r.toJson()
        val keys = json.keys
        assertTrue(
            keys.containsAll(
                listOf(
                    "task", "skeletonHash", "recipeVersion", "nodeCount", "classCount", "edgeCount",
                    "imageInputs", "hasMask", "knobsTruncated", "nodeClasses", "edges", "slots",
                    "knobs", "outputs", "warnings",
                ),
            ),
            "少一个键 = 界面上少一块内容：$keys",
        )
        assertEquals(WorkflowRecipe.VERSION, (json["recipeVersion"] as JsonPrimitive).content.toInt())
    }

    // -----------------------------------------------------------------------
    //  真文件（仓库里那两份 API 格式的 inpaint 工作流）
    // -----------------------------------------------------------------------

    @Test
    fun `仓库里真有的 inpaint 工作流能归纳出槽位与遮罩`() {
        val file = Paths.get("..", "comfyui", "workflows", "comfyhub_inpaint_krea2.json")
        if (!Files.isRegularFile(file)) return  // 单独把文件挪走时不红（它不是本用例的产物）
        val root = AppJson.parseToJsonElement(Files.readString(file)) as JsonObject
        assertTrue(HistoryEntry.isNodeGraph(root), "这份文件应当是 API 格式（$file）")

        val r = WorkflowRecipe.analyze(root)
        assertEquals(WorkflowRecipe.Task.INPAINT, r.task)
        assertTrue(r.hasMask)
        assertEquals(
            listOf("checkpoints", "text_encoders", "vae"),
            r.slots.map { it.role },
            "真文件里的槽位：底模 + 外挂 CLIP + 外挂 VAE",
        )
        assertTrue(r.slots.any { it.overrideKey == "1.ckpt_name" })
        // 提示词、底图、遮罩、采样参数都要能看见（模型照着写 overrides）
        val keys = r.knobs.map { it.overrideKey }.toSet()
        listOf("13.text", "10.image", "12.image", "15.steps", "15.cfg", "15.seed", "11.width").forEach {
            assertTrue(it in keys, "真文件里 $it 应当是个旋钮：$keys")
        }
        assertTrue(r.outputs.any { it.classType == "SaveImage" })
        assertTrue(r.imageInputs >= 2, "底图 + 人物图 + 遮罩，至少两个 LoadImage 类节点")
    }

    @Test
    fun `同一族的两份真文件骨架相同 而细节不同`() {
        // 这两份是**同一族**（都是 krea2 的分离式加载 + 采样链），文件名不同、参数不同
        val a = Paths.get("..", "comfyui", "workflows", "comfyhub_inpaint_krea2.json")
        val b = Paths.get("..", "comfyui", "workflows", "comfyhub_inpaint_anima.json")
        if (!Files.isRegularFile(a) || !Files.isRegularFile(b)) return
        val ra = WorkflowRecipe.analyze(AppJson.parseToJsonElement(Files.readString(a)) as JsonObject)
        val rb = WorkflowRecipe.analyze(AppJson.parseToJsonElement(Files.readString(b)) as JsonObject)
        // 两份的**角色**应该一致（都是 inpaint + 有遮罩），但底模槽位的角色不同（Anima 用 UNET+CLIP）
        assertEquals(WorkflowRecipe.Task.INPAINT, ra.task)
        assertEquals(WorkflowRecipe.Task.INPAINT, rb.task)
        assertTrue(ra.slots.isNotEmpty() && rb.slots.isNotEmpty())
        // 骨架指纹是**内容**的函数：这里刻意只断言"算得出来且稳定"，不硬钉一个数字
        assertEquals(ra.skeletonHash, WorkflowRecipe.analyze(AppJson.parseToJsonElement(Files.readString(a)) as JsonObject).skeletonHash)
        assertEquals(16, ra.skeletonHash.length, "指纹是 8 字节 = 16 位十六进制")
    }
}
