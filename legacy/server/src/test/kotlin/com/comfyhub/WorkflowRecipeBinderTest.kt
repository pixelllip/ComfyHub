package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 配方绑定（W3）的回归防线：**"把新模型接到旧骨架上"这件事只报不改**。
 *
 * 盯的是六件**删了以后出错也没人知道**的事：
 * ① 只有"同名且当前实例能加载"（`exact`）才自动写覆盖键 —— 这一档写错就是"AI 悄悄换掉了用户的模型"；
 * ② 盘上有、实例看不到（`onDiskOnly`）**不许**写键，而且要说"重启/修挂载，别下载"；
 * ③ 只有字面相似（`substitute`）**不许**写键，候选给出来但"先问用户"；
 * ④ 本机没有（`missing`）**不许编文件名**；
 * ⑤ 同角色多槽位（两条 LoRA）→ `ambiguous`，**不许**把一个文件写给两个槽位；
 * ⑥ 整包 checkpoint 接到"分离式加载"的骨架上时，要明说"结构上接不上、要改写骨架"，
 *    而不是糊一个看着像的覆盖键上去。
 */
class WorkflowRecipeBinderTest {

    /** 一条"分离式加载 + 两条 LoRA"的参考图编辑骨架（形状照 Krea 2 那类工作流抄）。 */
    private fun editRecipe(): WorkflowRecipe.Recipe = WorkflowRecipe.analyze(
        AppJson.parseToJsonElement(
            """
            {
              "55": {"class_type":"UNETLoader","inputs":{"unet_name":"krea2_turbo_bf16.safetensors","weight_dtype":"default"}},
              "56": {"class_type":"CLIPLoader","inputs":{"clip_name":"qwen3vl_4b.safetensors","type":"krea2"}},
              "57": {"class_type":"VAELoader","inputs":{"vae_name":"wan_2.1_vae.safetensors"}},
              "71": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["55",0],"lora_name":"identity_edit.safetensors","strength_model":1.0}},
              "72": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["71",0],"lora_name":"detail.safetensors","strength_model":0.6}},
              "80": {"class_type":"LoadImage","inputs":{"image":"ref.png"}},
              "81": {"class_type":"CLIPTextEncode","inputs":{"clip":["56",0],"text":"换成跑步"}},
              "82": {"class_type":"KSampler","inputs":{"model":["72",0],"positive":["81",0],"latent_image":["80",0],
                     "seed":1,"steps":8,"cfg":1.0,"sampler_name":"euler","scheduler":"simple","denoise":1.0}},
              "83": {"class_type":"VAEDecode","inputs":{"samples":["82",0],"vae":["57",0]}},
              "84": {"class_type":"SaveImage","inputs":{"images":["83",0]}}
            }
            """.trimIndent(),
        ) as JsonObject,
    )

    private fun inventory(vararg models: ComfyModels.LocalModel): ComfyModels.Inventory =
        ComfyModels.Inventory(source = ComfyModels.Source.OBJECT_INFO, models = models.toList())

    private fun local(name: String, folder: String, visible: Boolean = true) =
        ComfyModels.LocalModel(name = name, folder = folder, path = "D:/models/$folder/$name", visibleToInstance = visible)

    // -----------------------------------------------------------------------

    @Test
    fun `同名且实例能加载才自动写覆盖键`() {
        val recipe = editRecipe()
        val report = WorkflowRecipeBinder.bind(
            recipe = recipe,
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "krea2_turbo_v11.safetensors")),
            inventory = inventory(local("krea2_turbo_v11.safetensors", "diffusion_models")),
            recipeId = 7,
            recipeName = "krea2 编辑",
        )
        assertEquals(WorkflowRecipeBinder.Status.EXACT, report.bindings.single().status)
        assertEquals(mapOf("55.unet_name" to "krea2_turbo_v11.safetensors"), report.overrides)
        assertTrue(report.ready)
        // 角色是**按清单里那个文件所在的目录推**的（不用调用方猜）
        assertEquals("diffusion_models", report.bindings.single().role)
        // 没动的槽位照样列出来（用户看得见"还有哪些是原值"）
        assertTrue(report.untouched.any { it.startsWith("56.clip_name") })
        // 旋钮提示里有提示词与采样键（模型据此拼下一次提交）
        assertTrue(report.knobHints.contains("81.text"))
        assertTrue(report.knobHints.contains("82.steps"))
        assertTrue(report.render().contains("可以直接跑"), report.render())
    }

    @Test
    fun `盘上有但实例看不到 不许写键 也不许让人去下载`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "krea2_turbo_bf16.safetensors")),
            inventory = inventory(local("krea2_turbo_bf16.safetensors", "diffusion_models", visible = false)),
        )
        assertEquals(WorkflowRecipeBinder.Status.ON_DISK_ONLY, report.bindings.single().status)
        assertTrue(report.overrides.isEmpty(), "实例加载不了的文件写进覆盖键 = 下一次提交必然报文件不存在")
        assertFalse(report.ready)
        val note = report.bindings.single().note
        assertTrue(note.contains("重启"), note)
        assertTrue(note.contains("不要下载"), note)
    }

    @Test
    fun `只有字面相似时给候选但先问用户`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "krea2_turbo_v11.safetensors")),
            inventory = inventory(local("krea2_turbo_v10.safetensors", "diffusion_models")),
        )
        val b = report.bindings.single()
        assertEquals(WorkflowRecipeBinder.Status.SUBSTITUTE, b.status)
        assertTrue(report.overrides.isEmpty())
        assertTrue(b.candidates.first().contains("krea2_turbo_v10.safetensors"))
        assertTrue(b.note.contains("先问用户"), b.note)
    }

    @Test
    fun `本机没有时绝不编文件名 force 才写得进去`() {
        val inv = inventory(local("anima-base-v1.0.safetensors", "diffusion_models"))
        val strict = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "totally_new_model.safetensors")),
            inventory = inv,
        )
        assertEquals(WorkflowRecipeBinder.Status.MISSING, strict.bindings.single().status)
        assertTrue(strict.overrides.isEmpty())
        assertTrue(strict.bindings.single().note.contains("不要自己编"))

        val forced = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "totally_new_model.safetensors")),
            inventory = inv,
            force = true,
        )
        assertEquals(mapOf("55.unet_name" to "totally_new_model.safetensors"), forced.overrides)
        assertTrue(forced.warnings.any { it.contains("没验证过") }, forced.warnings.toString())
        assertFalse(forced.ready, "force 写进去不等于 ready（提交前预检还会再拦一次）")
    }

    @Test
    fun `拿不到清单时如实说没验证过`() {
        val recipe = editRecipe()
        val unchecked = WorkflowRecipeBinder.bind(
            recipe = recipe,
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "x.safetensors", role = "diffusion_models")),
            inventory = null,
        )
        assertEquals(WorkflowRecipeBinder.Status.UNCHECKED, unchecked.bindings.single().status)
        assertTrue(unchecked.overrides.isEmpty())

        val forced = WorkflowRecipeBinder.bind(
            recipe = recipe,
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "x.safetensors", role = "diffusion_models")),
            inventory = null,
            force = true,
        )
        assertEquals(mapOf("55.unet_name" to "x.safetensors"), forced.overrides)
    }

    @Test
    fun `同角色多槽位要报 ambiguous 不许一个文件写给两个槽位`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "new_lora.safetensors", role = "loras")),
            inventory = inventory(local("new_lora.safetensors", "loras")),
        )
        val b = report.bindings.single()
        assertEquals(WorkflowRecipeBinder.Status.AMBIGUOUS, b.status)
        assertTrue(report.overrides.isEmpty())
        assertTrue(b.note.contains("71.lora_name") && b.note.contains("72.lora_name"), b.note)

        // 点明 overrideKey 就绑得上
        val one = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(
                WorkflowRecipeBinder.Request(fileName = "new_lora.safetensors", overrideKey = "71.lora_name"),
            ),
            inventory = inventory(local("new_lora.safetensors", "loras")),
        )
        assertEquals(mapOf("71.lora_name" to "new_lora.safetensors"), one.overrides)
    }

    @Test
    fun `整包 checkpoint 接分离式骨架要明说结构上接不上`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "anima-base-v1.0.safetensors")),
            inventory = inventory(local("anima-base-v1.0.safetensors", "checkpoints")),
        )
        val b = report.bindings.single()
        assertEquals(WorkflowRecipeBinder.Status.NOT_IN_RECIPE, b.status)
        assertTrue(report.overrides.isEmpty())
        assertTrue(b.note.contains("分离式加载"), b.note)
        assertTrue(b.note.contains("checkpoint"), b.note)
        assertTrue(b.note.contains("改写"), b.note)
        // 同样的结论也要能从"给了一个不存在的 role"那条路得到
        val byRole = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "x.safetensors", role = "checkpoints")),
            inventory = null,
        )
        assertTrue(byRole.bindings.single().note.contains("没有 role=checkpoints"), byRole.bindings.single().note)
    }

    @Test
    fun `一次绑好几个角色 缺件配方要提醒先补齐`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(
                WorkflowRecipeBinder.Request(fileName = "new_unet.safetensors", role = "diffusion_models"),
                WorkflowRecipeBinder.Request(fileName = "new_clip.safetensors", role = "text_encoders"),
                WorkflowRecipeBinder.Request(fileName = "new_vae.safetensors", role = "vae"),
            ),
            inventory = inventory(
                local("new_unet.safetensors", "diffusion_models"),
                local("new_clip.safetensors", "text_encoders"),
                local("new_vae.safetensors", "vae"),
            ),
            complete = false,
        )
        assertEquals(
            mapOf(
                "55.unet_name" to "new_unet.safetensors",
                "56.clip_name" to "new_clip.safetensors",
                "57.vae_name" to "new_vae.safetensors",
            ),
            report.overrides,
        )
        assertFalse(report.ready, "complete=false 的配方不许报 ready")
        assertTrue(report.hints.any { it.contains("complete=false") }, report.hints.toString())
    }

    @Test
    fun `sidecar 元数据不算模型 也不许混进候选`() {
        // 真机探针抓到的形状：磁盘兜底清单里有 anima-base-v1.0.metadata.json
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(
                WorkflowRecipeBinder.Request(
                    fileName = "anima-base-v1.0.metadata.json",
                    role = "diffusion_models",
                ),
            ),
            inventory = inventory(
                local("anima-base-v1.0.metadata.json", "diffusion_models"),
                local("anima-base-v1.0.safetensors", "diffusion_models"),
            ),
        )
        val b = report.bindings.single()
        assertEquals(WorkflowRecipeBinder.Status.NOT_A_MODEL, b.status)
        assertTrue(report.overrides.isEmpty())
        assertTrue(b.note.contains("safetensors"), b.note)

        // 反过来：sidecar 也不该出现在候选里（它会把真模型顶掉）
        val near = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(
                WorkflowRecipeBinder.Request(fileName = "anima-base-v1.1.safetensors", role = "diffusion_models"),
            ),
            inventory = inventory(local("anima-base-v1.0.metadata.json", "diffusion_models")),
        )
        assertFalse(
            near.bindings.single().candidates.any { it.contains("metadata.json") },
            "候选里不许有 sidecar：${near.bindings.single().candidates}",
        )
    }

    @Test
    fun `没给 role 时按底模那一格兜底 但要把假设说出来`() {
        val report = WorkflowRecipeBinder.bind(
            recipe = editRecipe(),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "x.safetensors")),
            inventory = null,
        )
        val b = report.bindings.single()
        assertEquals("diffusion_models", b.role, "参数名叫 model，兜底就该落在底模那一格")
        assertEquals(WorkflowRecipeBinder.Status.UNCHECKED, b.status)
        assertTrue(
            report.warnings.any { it.contains("按**底模**那一格") },
            "兜底是个假设，必须说出来：${report.warnings}",
        )
    }

    @Test
    fun `配方连底模槽位都没有时要说清楚要带什么`() {
        // 只有一条 LoRA + 存图的骨架（没有底模槽位）
        val loraOnly = WorkflowRecipe.analyze(
            AppJson.parseToJsonElement(
                """
                {
                  "71": {"class_type":"LoraLoaderModelOnly","inputs":{"model":["70",0],"lora_name":"a.safetensors","strength_model":1.0}},
                  "70": {"class_type":"UNETLoader","inputs":{"unet_name":""}},
                  "84": {"class_type":"SaveImage","inputs":{"images":["70",0]}}
                }
                """.trimIndent(),
            ) as JsonObject,
        )
        val report = WorkflowRecipeBinder.bind(
            recipe = loraOnly.copy(slots = loraOnly.slots.filter { it.role == "loras" }),
            requests = listOf(WorkflowRecipeBinder.Request(fileName = "x.safetensors")),
            inventory = null,
        )
        val b = report.bindings.single()
        assertEquals(WorkflowRecipeBinder.Status.NOT_IN_RECIPE, b.status)
        assertTrue(b.note.contains("overrideKey"), b.note)
        assertTrue(b.note.contains("71.lora_name"), "要把槽位清单列出来：${b.note}")
        assertNull(b.overrideKey)
    }
}
