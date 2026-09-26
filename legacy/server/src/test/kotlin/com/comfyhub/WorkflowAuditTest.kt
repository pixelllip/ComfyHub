package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 工作流审核（用户 2026-09-22：「让 AI 具有审核本地工作流的能力」）。
 *
 * 每一项都对应会话 `0bedb3f5` 里**真实踩到**的一个坑（见 [WorkflowAudit] 的类注释那张表）。
 * 两条纪律各钉一头：
 *  - **跑不了的必须拦**（纯前端节点 / 版本不兼容 / 本机没有的节点类）——
 *    带着它们提交就是 0.01 秒空跑；
 *  - **"不推荐"的绝不拦**（参数档位 / 显存 / 提示词风格）——
 *    那是替用户做决定，比白跑一次更让人恼火。
 */
class WorkflowAuditTest {

    private fun graph(json: String): JsonObject = AppJson.parseToJsonElement(json) as JsonObject

    /** 一份干净的最小骨架（Krea2 Turbo 的官方档位）。 */
    private fun cleanGraph() = graph(
        """
        {
          "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}},
          "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3VL4BAbliterated.safetensors", "type": "krea2"}},
          "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "A candid night photograph of a young woman leaning on a railing, warm promenade lights reflecting on the water, medium shot, vertical framing, soft directional lighting"}},
          "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
          "5": {"class_type": "KSampler", "inputs": {"steps": 8, "cfg": 1.0, "sampler_name": "euler", "seed": 20260922}},
          "6": {"class_type": "VAEDecode", "inputs": {}},
          "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "Krea2_run_a"}}
        }
        """.trimIndent(),
    )

    private fun find(report: WorkflowAudit.Report, check: String) = report.findings.filter { it.check == check }

    @Test
    fun `官方档位的骨架不报必跑不了的问题`() {
        val report = WorkflowAudit.audit(cleanGraph())
        assertTrue(report.blocking.isEmpty(), "这份骨架是官方档位、无前端节点：不该有 BLOCK：${report.render()}")
        assertNull(report.blockingRefusal())
        // 唯一会报的是"固定 seed + 固定前缀"那条提醒 —— 重复跑同一份图会落同一个文件名，
        // 被去重吞掉（那次白跑 78 秒）。它是提醒，不是拦。
        assertEquals(
            listOf(WorkflowAudit.CHECK_OUTPUT_NAME),
            report.findings.map { it.check },
            report.render(),
        )
    }

    @Test
    fun `纯前端节点必须拦 那次是 20 个`() {
        val g = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}},
              "10": {"class_type": "Label (rgthree)", "inputs": {}},
              "15": {"class_type": "Fast Groups Bypasser (rgthree)", "inputs": {}},
              "163": {"class_type": "Bookmark (rgthree)", "inputs": {}},
              "20": {"class_type": "GetNode", "inputs": {}}
            }
            """.trimIndent(),
        )
        val report = WorkflowAudit.audit(g)
        val f = find(report, WorkflowAudit.CHECK_FRONTEND).single()
        assertEquals(WorkflowAudit.Level.BLOCK, f.level)
        assertTrue(f.detail.contains("0.01"), "要说清后果（0.01 秒空跑）：${f.detail}")
        assertTrue(f.nodes.containsAll(listOf("10", "15", "20", "163")), "${f.nodes}")
        assertNotNull(report.blockingRefusal())
    }

    @Test
    fun `版本不兼容的节点必须拦 并指向更新节点包`() {
        val g = graph(
            """
            {
              "94": {"class_type": "WidgetToString", "inputs": {}},
              "74": {"class_type": "SystemNotification|pysssss", "inputs": {}}
            }
            """.trimIndent(),
        )
        val f = find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_NODE_VERSION).single()
        assertEquals(WorkflowAudit.Level.BLOCK, f.level)
        assertTrue(assertNotNull(f.fix).contains("更新"), "要给出路：不是「这工作流坏了」而是「更新节点包」：${f.fix}")
        assertTrue(f.detail.contains("IS_CHANGED"), f.detail)
    }

    @Test
    fun `固定 seed 加固定前缀必须提醒 那次白跑 78 秒`() {
        val g = graph(
            """
            {
              "5": {"class_type": "KSampler", "inputs": {"steps": 8, "cfg": 1.0, "sampler_name": "euler", "seed": 20260921}},
              "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "Krea2_spine"}}
            }
            """.trimIndent(),
        )
        val f = find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_OUTPUT_NAME).single()
        assertEquals(WorkflowAudit.Level.WARN, f.level, "不是「跑不了」，不许拦提交")
        assertTrue(f.detail.contains("未重复入库") || f.detail.contains("mediaCount"), f.detail)
        assertTrue(assertNotNull(f.fix).contains("overrides"), "要给可直接用的修法：${f.fix}")
        assertNull(WorkflowAudit.audit(g).blockingRefusal(), "只提醒不拦")
    }

    @Test
    fun `seed 是随机占位符时不提醒`() {
        val g = graph(
            """
            {
              "5": {"class_type": "KSampler", "inputs": {"seed": "randomize", "steps": 8}},
              "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "x"}}
            }
            """.trimIndent(),
        )
        assertTrue(find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_OUTPUT_NAME).isEmpty())
    }

    @Test
    fun `参数偏离该模型的官方档位要提醒`() {
        val g = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}},
              "5": {"class_type": "KSampler", "inputs": {"steps": 30, "cfg": 4.0, "sampler_name": "dpmpp_2m_sde"}}
            }
            """.trimIndent(),
        )
        val f = find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_PARAMS).single()
        assertEquals(WorkflowAudit.Level.WARN, f.level)
        assertTrue(f.detail.contains("steps = 30"), f.detail)
        assertTrue(f.detail.contains("8"), "要说清该是什么：${f.detail}")
    }

    @Test
    fun `不认识的模型不判参数`() {
        val g = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "someUnknownModel.safetensors"}},
              "5": {"class_type": "KSampler", "inputs": {"steps": 999, "cfg": 42.0, "sampler_name": "weird"}}
            }
            """.trimIndent(),
        )
        assertTrue(find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_PARAMS).isEmpty(), "不知道就别猜")
    }

    @Test
    fun `大尺寸加放大加多 LoRA 要提醒显存`() {
        val g = graph(
            """
            {
              "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 2048, "height": 2048}},
              "8": {"class_type": "ImageUpscaleWithModel", "inputs": {}},
              "9": {"class_type": "LoraLoader", "inputs": {"lora_name": "a"}},
              "10": {"class_type": "LoraLoader", "inputs": {"lora_name": "b"}},
              "11": {"class_type": "LoraLoader", "inputs": {"lora_name": "c"}}
            }
            """.trimIndent(),
        )
        val f = find(WorkflowAudit.audit(g), WorkflowAudit.CHECK_VRAM).single()
        assertEquals(WorkflowAudit.Level.WARN, f.level)
        assertTrue(f.detail.contains("8GB"), f.detail)
    }

    @Test
    fun `给 Krea 2 堆 tag 要提醒 给 Anima 写整段自然语言也要提醒`() {
        val krea = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2Turbo_something.safetensors"}},
              "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "masterpiece, best quality, 1girl, solo, long hair, blue eyes, white dress, school uniform, blush, smile, looking at viewer"}}
            }
            """.trimIndent(),
        )
        val kreaFinding = find(WorkflowAudit.audit(krea), WorkflowAudit.CHECK_PROMPT_STYLE).single()
        assertEquals(WorkflowAudit.Level.WARN, kreaFinding.level)
        assertTrue(kreaFinding.fix!!.contains("krea-2"), "要指向对应的 skill：${kreaFinding.fix}")

        // 一段"逗号很少、但很长"的自然语言 —— 那正是给 Anima 写错了的样子
        val long = "A young woman with short dark blue-purple hair leans down over a young man " +
            "while her right hand rests gently on the top of his head and she looks down at him " +
            "with a soft warm teasing smile and a light blush across her cheeks and sweat on her forehead"
        val anima = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "anima-base-v1.0.safetensors"}},
              "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "$long"}}
            }
            """.trimIndent(),
        )
        val animaFinding = find(WorkflowAudit.audit(anima), WorkflowAudit.CHECK_PROMPT_STYLE).single()
        assertEquals(WorkflowAudit.Level.INFO, animaFinding.level, "Anima 这条是背景提示，不是警告")
        assertTrue(animaFinding.fix!!.contains("anima-prompt"), animaFinding.fix)
    }

    @Test
    fun `加载器读错目录要提醒 那次是 UNETLoader 读 checkpoints`() {
        val g = graph(
            """{"79": {"class_type": "UNETLoader", "inputs": {"unet_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"}}}""",
        )
        val inv = ComfyModels.Inventory(
            source = ComfyModels.Source.OBJECT_INFO,
            models = listOf(
                ComfyModels.LocalModel(
                    name = "krea2TurboOfficialComfy_krea2TurboInt8.safetensors",
                    folder = "checkpoints",
                ),
            ),
        )
        val f = find(WorkflowAudit.audit(g, inventory = inv), WorkflowAudit.CHECK_LOADER_FOLDER).single()
        assertEquals(WorkflowAudit.Level.WARN, f.level)
        assertTrue(f.detail.contains("checkpoints"), f.detail)
        assertTrue(f.detail.contains("diffusion_models"), f.detail)
    }

    @Test
    fun `拿不到 object_info 时不判本机缺节点`() {
        val g = graph("""{"9": {"class_type": "SomeThirdPartyNode", "inputs": {}}}""")
        val report = WorkflowAudit.audit(g, objectInfo = null)
        assertTrue(
            find(report, WorkflowAudit.CHECK_UNKNOWN_NODE).isEmpty(),
            "ComfyUI 没跑（拿不到 /object_info）时不许说「本机没有这个节点类」",
        )
        // 拿得到时才判
        val withInfo = WorkflowAudit.audit(g, objectInfo = graph("""{"KSampler": {}}"""))
        assertEquals(1, find(withInfo, WorkflowAudit.CHECK_UNKNOWN_NODE).size)
        assertEquals(WorkflowAudit.Level.BLOCK, find(withInfo, WorkflowAudit.CHECK_UNKNOWN_NODE).single().level)
    }

    @Test
    fun `闸门只拦必跑不了的三项 参数与显存不拦`() {
        val g = graph(
            """
            {
              "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "krea2Turbo_x.safetensors"}},
              "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 2048, "height": 2048}},
              "5": {"class_type": "KSampler", "inputs": {"steps": 40, "cfg": 8.0, "sampler_name": "ddim", "seed": 1}},
              "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "same"}}
            }
            """.trimIndent(),
        )
        val report = WorkflowAudit.audit(g)
        assertTrue(report.findings.isNotEmpty(), "参数/显存/前缀都该报")
        assertNull(report.blockingRefusal(), "但没有一项是「跑不了」—— 不许拦：${report.render()}")
    }

    @Test
    fun `JSON 形状要能直接被界面与模型用`() {
        val g = graph("""{"10": {"class_type": "Label (rgthree)", "inputs": {}}}""")
        val json = WorkflowAudit.audit(g).toJson()
        assertEquals("1", (json["block"] as kotlinx.serialization.json.JsonPrimitive).content)
        assertEquals("1", (json["nodeCount"] as kotlinx.serialization.json.JsonPrimitive).content)
        val finding = (json["findings"] as kotlinx.serialization.json.JsonArray)[0] as JsonObject
        assertEquals(WorkflowAudit.CHECK_FRONTEND, (finding["check"] as kotlinx.serialization.json.JsonPrimitive).content)
        assertFalse(finding["fix"] == null, "修法要跟着走")
    }
}
