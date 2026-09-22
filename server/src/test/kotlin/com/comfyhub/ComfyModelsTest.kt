package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * `comfy_fit_workflow` 的配对判据（用户建议 ①，2026-09-22）。
 *
 * 用户的原话是"可以使用 civitai 等网站查找符合要求的工作流，**优先根据本机已有的模型修改该工作流**"。
 * 这一组用例钉的就是中间那一步：**图里点名的模型 ↔ 本机已有的文件**。
 *
 * 三件事各钉一头，因为每一头错了都会让用户白等一次提交：
 *
 *  1. **清单从哪来**：`/object_info` 的枚举优先（那是"ComfyUI 能加载什么"的权威答案），
 *     拿不到才扫盘 —— 扫出来的会标 `scan`，因为"盘上有"不等于"它认得到"。
 *  2. **配对怎么打分**：词根相似度（`krea2Turbo…_int8` ↔ `krea2_turbo_int8`）要认得出来，
 *     而 `sd_xl_base` ↔ `sd_xl_refiner` 这种"字面很像、其实是两份不同模型"的**不许**当成同一份。
 *  3. **只报不改**：配对跑完，输入的那份图必须**一个字节都没变**，也不许替模型决定"就用这个替代品"。
 */
class ComfyModelsTest {

    private fun objectInfo(json: String): JsonObject = AppJson.parseToJsonElement(json) as JsonObject

    /** 一份像真的 `/object_info`：模型枚举 + 一个普通参数节点。 */
    private fun realObjectInfo() = objectInfo(
        """
        {
          "CheckpointLoaderSimple": {"input": {"required": {"ckpt_name": [["krea2_turbo_int8.safetensors", "sub\\sd_xl_base_1.0.safetensors"]]}}},
          "LoraLoader": {"input": {"required": {"lora_name": [["ink_wash.safetensors"]]}}},
          "VAELoader": {"input": {"required": {"vae_name": [["vae-ft-mse.safetensors"]]}}},
          "KSampler": {"input": {"required": {"steps": ["INT", {"default": 20}], "sampler_name": [["euler", "dpmpp_2m"]]}}},
          "MyCustomLoader": {"input": {"required": {"model_name": [["custom_thing.safetensors"]]}}}
        }
        """.trimIndent(),
    )

    // --- 清单来源 -----------------------------------------------------------

    @Test
    fun `清单优先取 object_info 的枚举 而不是扫盘`() {
        val inv = ComfyModels.inventory(realObjectInfo(), projectRoot = null)!!
        assertEquals(ComfyModels.Source.OBJECT_INFO, inv.source)
        assertTrue(inv.models.any { it.name == "krea2_turbo_int8.safetensors" && it.folder == "checkpoints" })
        assertTrue(inv.models.any { it.name == "ink_wash.safetensors" && it.folder == "loras" })
        assertTrue(inv.models.any { it.name == "vae-ft-mse.safetensors" && it.folder == "vae" })
        // 下拉预设（sampler_name 的 euler / dpmpp_2m）**不是模型文件**，不该进清单
        assertFalse(inv.models.any { it.name == "euler" }, "枚举里的非文件项不许被当成模型")
    }

    @Test
    fun `拿不到 object_info 时返回 null 让调用方去扫盘 而不是给一份空清单`() {
        // 空清单会让每一份工作流都看起来"什么模型都缺" —— 那比"没有清单"更糟
        assertEquals(null, ComfyModels.fromObjectInfo(null))
        assertEquals(null, ComfyModels.fromObjectInfo(objectInfo("""{"KSampler":{"input":{"required":{}}}}""")))
    }

    @Test
    fun `枚举后面的配置对象不会被当成文件名（线上形状：第一项是枚举、后面跟 tooltip）`() {
        // 线上真实形状：`"ckpt_name": [["a.safetensors"], {"tooltip": "..."}]` ——
        // 只看第一项才是枚举，把整条声明摊平就会把 tooltip 也读成模型名
        val inv = ComfyModels.fromObjectInfo(
            objectInfo(
                """
                {"CheckpointLoaderSimple":{"input":{"required":{
                   "ckpt_name":[["a.safetensors"],{"tooltip":"选一个底模"}]}}}}
                """.trimIndent(),
            ),
        )!!
        assertEquals(listOf("a.safetensors"), inv.models.map { it.name })
    }

    @Test
    fun `optional 段里的模型枚举也要认（VHS 之类把加载器放在 optional）`() {
        // 只看 `required` 会漏掉一整类节点（缺了就是"本机明明有却说没有"）
        val inputs = ComfyModels.enumerateModelInputs(
            objectInfo(
                """
                {"SomeLoader":{"input":{"optional":{"model_name":[["x.safetensors"]]}}}}
                """.trimIndent(),
            ),
        )
        assertTrue("model_name" in inputs["SomeLoader"].orEmpty(), inputs.toString())
    }

    @Test
    fun `自定义加载器的模型枚举也认得出来（不用维护一张越来越长的白名单）`() {
        val inputs = ComfyModels.enumerateModelInputs(realObjectInfo())
        assertTrue("ckpt_name" in inputs["CheckpointLoaderSimple"].orEmpty())
        assertTrue("model_name" in inputs["MyCustomLoader"].orEmpty(), "第三方加载器也要认出来")
        assertFalse("steps" in inputs["KSampler"].orEmpty(), "INT 参数不是模型枚举")
        assertFalse("sampler_name" in inputs["KSampler"].orEmpty(), "下拉预设名不是模型文件")
    }

    // --- 图里要什么 ---------------------------------------------------------

    @Test
    fun `只认确定是文件名的输入 连线型与普通参数一律不算`() {
        val graph = objectInfo(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2_turbo_int8.safetensors"}},
              "2": {"class_type": "LoraLoader", "inputs": {"lora_name": ["1", 0], "model": ["1", 0]}},
              "3": {"class_type": "KSampler", "inputs": {"steps": 20, "seed": 1}},
              "4": {"class_type": "Note", "inputs": {"text": "说明文字"}}
            }
            """.trimIndent(),
        )
        val reqs = ComfyModels.requirements(graph)
        assertEquals(1, reqs.size, "只有 1.ckpt_name 是一个真实文件名：$reqs")
        assertEquals("1.ckpt_name", reqs.first().overrideKey)
        assertEquals("checkpoints", reqs.first().folder)
    }

    @Test
    fun `自定义加载器的输入靠 object_info 认出 内置表里没有它`() {
        val graph = objectInfo(
            """{"7": {"class_type": "MyCustomLoader", "inputs": {"model_name": "custom_thing.safetensors"}}}"""
        )
        assertEquals(0, ComfyModels.requirements(graph).size, "内置表不认识它，就不许猜")
        val withInfo = ComfyModels.requirements(graph, ComfyModels.enumerateModelInputs(realObjectInfo()))
        assertEquals(1, withInfo.size)
        assertEquals("7.model_name", withInfo.first().overrideKey)
    }

    // --- 打分 ---------------------------------------------------------------

    @Test
    fun `同名归一化后是一模一样（只差大小写与分隔符）`() {
        assertTrue(ModelName.same("krea2_turbo_int8.safetensors", "Krea2-Turbo-Int8.safetensors"))
        assertTrue(ModelName.same("a.safetensors", "a.safetensors"))
        // 分量不同 = 不是同一份（这正是"不许硬换"的那条线）
        assertFalse(ModelName.same("sd_xl_base_1.0.safetensors", "sd_xl_refiner_1.0.safetensors"))
    }

    @Test
    fun `词根对得上的分数高 但两份不同的模型不会被当成同一份`() {
        // 现场（2026-09-21 b5233fb4 seq6）：这条是模型当时靠手读 safetensors 头部判断的
        val krea = ModelName.similarity(
            "krea2TurboOfficialComfy_krea2TurboInt8.safetensors",
            "krea2_turbo_int8.safetensors",
        )
        assertTrue(krea >= 0.6, "词根全对上要给高分：$krea")

        // 字面很像，但 base 与 refiner 是两份不同的模型：分数会不低，
        // 所以**只报候选、绝不自动替换**（safeToAutoApply 只认同名那一档）
        val baseVsRefiner = ModelName.similarity(
            "sd_xl_base_1.0.safetensors",
            "sd_xl_refiner_1.0.safetensors",
        )
        assertTrue(baseVsRefiner < 1.0, "绝不许判成同一份：$baseVsRefiner")

        assertEquals(1.0, ModelName.similarity("a.safetensors", "a.safetensors"))
        assertTrue(ModelName.similarity("anima_pencil.safetensors", "flux1-dev-fp8.safetensors") < ComfyModels.MIN_SCORE)
    }

    // --- 配对（只报不改）----------------------------------------------------

    /** 一份"本机有 base、没有 refiner"的清单。 */
    private fun inventoryWith(vararg names: String): ComfyModels.Inventory = ComfyModels.Inventory(
        source = ComfyModels.Source.SCAN,
        models = names.mapIndexed { i, n ->
            ComfyModels.LocalModel(name = n, folder = "checkpoints", path = "D:\\x\\$n", bytes = (i + 1) * 1024L * 1024 * 1024)
        },
    )

    @Test
    fun `本机有同名的报 exact 且不用改任何键`() {
        val graph = objectInfo(
            """{"1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}}}"""
        )
        val report = WorkflowFit.fit(graph, inventoryWith("sd_xl_base_1.0.safetensors"))
        assertEquals(1, report.items.size)
        assertEquals(FitStatus.EXACT, report.items.first().status)
        assertTrue(report.items.first().safeToAutoApply)
        assertTrue(report.suggestedOverrides().isEmpty(), "同名就没什么要改的")
        assertTrue(report.render().contains("不用改"), report.render())
    }

    @Test
    fun `本机没有同名的报 substitute 并把候选与体积列出来 但绝不自动替换`() {
        val graph = objectInfo(
            """{"1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2_turbo_int8_v2.safetensors"}}}"""
        )
        val report = WorkflowFit.fit(graph, inventoryWith("krea2_turbo_int8.safetensors"))
        val item = report.items.first()
        assertEquals(FitStatus.SUBSTITUTE, item.status)
        assertFalse(item.safeToAutoApply, "只有同名才允许不问用户就换")
        assertTrue(report.suggestedOverrides().isEmpty(), "替代品不进 suggestedOverrides")
        val text = report.render()
        assertTrue(text.contains("krea2_turbo_int8.safetensors"), text)
        assertTrue(text.contains("GB"), "体积要报出来（昨天真实的门槛就是 13.49GB）：$text")
        assertTrue(text.contains("先问用户") || text.contains("先跟用户确认"), text)
    }

    @Test
    fun `本机什么都没有时报 missing 且明说不要编一个文件名`() {
        val graph = objectInfo(
            """{"1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}}}"""
        )
        val report = WorkflowFit.fit(graph, inventoryWith("sd15.safetensors", "flux1-dev.safetensors"))
        assertEquals(FitStatus.MISSING, report.items.first().status)
        assertTrue(report.allMissing)
        val text = report.render()
        assertTrue(text.contains("不要编一个文件名"), text)
        assertTrue(text.contains("civitai_search"), "缺模型要指向「去搜」这条路：$text")
        assertTrue(text.contains("ask_user_question"), "选择权要交给用户：$text")
    }

    @Test
    fun `缺节点与缺模型分开报 不混成一件事`() {
        val graph = objectInfo(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}},
              "9": {"class_type": "SomeMissingNode", "inputs": {}}
            }
            """.trimIndent(),
        )
        val report = WorkflowFit.fit(
            graph,
            inventoryWith("sd_xl_base_1.0.safetensors"),
            knownNodes = setOf("CheckpointLoaderSimple", "KSampler"),
        )
        assertEquals(listOf("SomeMissingNode"), report.missingNodes)
        assertEquals(FitStatus.EXACT, report.items.first().status, "模型齐了就是齐了")
        val text = report.render()
        assertTrue(text.contains("SomeMissingNode"), text)
        assertTrue(text.contains("节点类"), "要说清缺的是节点不是模型：$text")
    }

    @Test
    fun `没有点名任何模型时如实说没有 而不是硬凑几条`() {
        val graph = objectInfo("""{"3": {"class_type": "KSampler", "inputs": {"steps": 20}}}""")
        val report = WorkflowFit.fit(graph, inventoryWith("sd_xl_base_1.0.safetensors"))
        assertTrue(report.items.isEmpty())
        assertFalse(report.needsAction)
        assertTrue(report.render().contains("没有点名"), report.render())
    }

    // --- 缺口 → 该搜什么 / 装到哪 / 本机沾边的有哪些（用户建议 ②）----------------

    @Test
    fun `文件名的打包串要转成能搜的查询词`() {
        // 拿整个文件名去 civitai_search 是搜不到的（那是一串打包名），词根才搜得到。
        // 分词是为了算相似度，搜索词则要**更像文件名**（`krea2` 粘回去）——
        // 这条断言就是钉"两种用法分开"的：相似度里 krea/2 必须是两个词，搜索词里得粘回来。
        assertEquals(
            "krea2 turbo int8",
            ModelName.lookupQuery("krea2TurboOfficialComfy_krea2TurboInt8.safetensors"),
        )
        assertEquals("ink wash2", ModelName.lookupQuery("ink_wash_v2_final.safetensors"))
        // `int8` 的那个 8 **必须留着**：int8 / fp8 是两份不同的文件，丢了就搜成别的模型
        assertTrue(ModelName.lookupQuery("krea2TurboInt8.safetensors").contains("int8"))
        // 一个词都没有时不许返回空串（空查询词会被 civitai_search 当场拒掉）：
        // 退化情况退回原文件名，宁可搜不到也不能给出一次非法调用
        assertTrue(ModelName.lookupQuery(".safetensors").isNotBlank(), ModelName.lookupQuery(".safetensors"))
        // 纯数字文件名也要能搜
        assertTrue(ModelName.lookupQuery("2.safetensors").isNotBlank(), ModelName.lookupQuery("2.safetensors"))
    }

    @Test
    fun `缺模型时给出该搜什么 装到哪 以及本机沾边但不够像的那几个`() {
        val graph = objectInfo(
            """{"1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}}}"""
        )
        // 盘上有几个"名字沾边"的：够不上候选阈值，但用户最容易以为"我明明有这个"
        val report = WorkflowFit.fit(graph, inventoryWith("sd15.safetensors", "sd_xl_base_1.0.safetensors"))
        val item = report.items.single()
        assertEquals(FitStatus.MISSING, item.status)
        val json = report.toJson()
        assertTrue(json.toString().contains("lookupQuery"), json.toString())
        // 直接读字段，别用"整串包含"来断言 —— 这条用例要钉的正是**具体是哪个词**
        val proposal = (json["models"] as kotlinx.serialization.json.JsonArray)[0]
            .let { it as kotlinx.serialization.json.JsonObject }["installProposal"]!!
        assertEquals(
            "krea2 turbo int8",
            ((proposal as kotlinx.serialization.json.JsonObject)["lookupQuery"]
                as kotlinx.serialization.json.JsonPrimitive).content,
            proposal.toString(),
        )
        assertTrue(proposal.toString().contains("installInto"), proposal.toString())
        val text = report.render()
        assertTrue(text.contains("krea2 turbo int8"), "要把建议的查询词写出来：$text")
        assertTrue(text.contains("体积"), "提问选项里要带体积：$text")
    }

    @Test
    fun `沾边但不报——只共用一个通用词的文件不算替代品`() {
        // 报错的名字比不报更糟：用户看到"本机有个 sd15.safetensors"会以为那就是他的模型，
        // 而它跟 krea2TurboInt8 毫无关系。所以"沾边"要有下限（分数 + 至少一个 ≥3 字的公共词）。
        val inv = ComfyModels.Inventory(
            ComfyModels.Source.SCAN,
            listOf(
                ComfyModels.LocalModel("sd15.safetensors", "checkpoints", null, null),
                ComfyModels.LocalModel("xl_base_1.0.safetensors", "checkpoints", null, null),
            ),
        )
        val near = inv.nearMissesFor("checkpoints", "krea2TurboInt8.safetensors")
        assertTrue(near.isEmpty(), "只共用一个通用词的不该被报成替代品：$near")

        // 真的沾边的（词根对得上、只是不够像）要报出来：
        // 这里刻意选了一个分数落在 [0.15, 0.35) 的例子 —— `krea2_turbo_fp8` 那种其实是
        // **候选**（分数更高），不会走 nearMiss 这条路
        val inv2 = ComfyModels.Inventory(
            ComfyModels.Source.SCAN,
            listOf(ComfyModels.LocalModel("krea_upscale_helper.safetensors", "checkpoints", null, null)),
        )
        assertEquals(
            listOf("krea_upscale_helper.safetensors"),
            inv2.nearMissesFor("checkpoints", "krea2TurboInt8.safetensors").map { it.name },
        )
    }
}
