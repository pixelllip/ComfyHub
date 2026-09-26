package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 提交前的模型预检（用户 2026-09-22 拍板：加载不了就**当场拒**，别白烧一轮显卡）。
 *
 * 两条同样重要：
 *  ① **该拒的拒** —— 要的模型当前实例加载不了（盘上有一份但实例看不到 / 本机确实没有）时，
 *     拒绝并给出**动作**，而不是让 ComfyUI 回一句 `file not found`（那次现场白跑了 78 秒）；
 *  ② **不该拒的不拒** —— 清单不可信（ComfyUI 没跑、也没扫到任何 models 目录）时**一个都不拦**：
 *     那时的 "missing" 只是"我没看见"。误拦一个本来能跑的工作流，比白跑一次更糟。
 */
class ModelPreflightTest {

    private fun graph(json: String): JsonObject = AppJson.parseToJsonElement(json) as JsonObject

    private fun loaderGraph(name: String) =
        graph("""{"1":{"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"$name"}}}""")

    private fun inventory(
        source: ComfyModels.Source = ComfyModels.Source.OBJECT_INFO,
        models: List<ComfyModels.LocalModel> = emptyList(),
    ) = ComfyModels.Inventory(
        source = source,
        models = models,
        diskOnly = models.filter { !it.visibleToInstance },
        merged = true,
    )

    private fun visible(name: String) = ComfyModels.LocalModel(name = name, folder = "checkpoints")
    private fun hidden(name: String) = ComfyModels.LocalModel(
        name = name,
        folder = "checkpoints",
        path = "D:\\Comfy-Desktop\\ComfyUI-Shared\\models\\checkpoints\\$name",
        bytes = 4_182_218_328,
        visibleToInstance = false,
    )

    @Test
    fun `模型都在就放行`() {
        val g = loaderGraph("krea2_turbo_int8.safetensors")
        assertNull(ModelPreflight.check(g, inventory(models = listOf(visible("krea2_turbo_int8.safetensors")))))
    }

    @Test
    fun `盘上有但实例看不到时拒 并让用户重启而不是下载`() {
        val g = loaderGraph("anima-base-v1.0.safetensors")
        val refusal = assertNotNull(
            ModelPreflight.check(g, inventory(models = listOf(hidden("anima-base-v1.0.safetensors")))),
            "盘上有、实例看不到 —— 必须拒（否则白跑一次）",
        )
        assertTrue(refusal.contains("没有提交"), refusal)
        assertTrue(refusal.contains("重启 ComfyUI"), "要给动作：$refusal")
        assertTrue(refusal.contains("不要下载"), "别让用户去下载一个就在硬盘上的文件：$refusal")
        assertTrue(refusal.contains("comfy_env_report"), "要指向对账工具：$refusal")
    }

    @Test
    fun `本机确实没有时拒 并指向搜索与提问`() {
        val g = loaderGraph("some_missing_style_lora.safetensors")
        val refusal = assertNotNull(ModelPreflight.check(g, inventory(models = listOf(visible("sd15.safetensors")))))
        assertTrue(refusal.contains("本机确实没有"), refusal)
        assertTrue(refusal.contains("civitai_search"), "要给出「去哪儿找」这条路：$refusal")
        assertTrue(refusal.contains("ask_user_question"), "选择权要交给用户：$refusal")
        assertTrue(refusal.contains("comfy_prune_workflow"), "旁支上的加载器要给一条出路：$refusal")
    }

    @Test
    fun `清单不可信时一个都不拦`() {
        // ComfyUI 没跑、也没扫到 models：这时的 missing 只是"我没看见"
        val g = loaderGraph("anima-base-v1.0.safetensors")
        assertNull(
            ModelPreflight.check(g, inventory(source = ComfyModels.Source.NONE)),
            "误拦一个本来能跑的工作流，比白跑一次更糟",
        )
    }

    @Test
    fun `空图与没有模型输入的图都放行`() {
        assertNull(ModelPreflight.check(graph("{}"), inventory(models = listOf(visible("a.safetensors")))))
        assertNull(
            ModelPreflight.check(
                graph("""{"3":{"class_type":"KSampler","inputs":{"steps":20}}}"""),
                inventory(models = emptyList()),
            ),
        )
    }

    @Test
    fun `拒绝文案里要写清是哪个键要哪个文件`() {
        val g = loaderGraph("anima-base-v1.0.safetensors")
        val refusal = assertNotNull(ModelPreflight.check(g, inventory(models = listOf(hidden("anima-base-v1.0.safetensors")))))
        assertTrue(refusal.contains("1.ckpt_name"), "要说清覆盖键：$refusal")
        assertTrue(refusal.contains("anima-base-v1.0.safetensors"), refusal)
        assertTrue(refusal.contains("ComfyUI-Shared"), "要说清文件到底在哪：$refusal")
    }
}
