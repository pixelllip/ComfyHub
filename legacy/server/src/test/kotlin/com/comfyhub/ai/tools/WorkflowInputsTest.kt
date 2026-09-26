package com.comfyhub.ai.tools

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * `comfy_check_inputs` 的扫描器（用户建议 ⑤，2026-09-20）。
 *
 * 用户的现场是："**用户没有提供参考图，但是他引导的工作流的节点用到参考图**"。
 * 那时候模型如果直接提交，ComfyUI 只会回一句缺输入 —— 用户看到的是"AI 说生成了、
 * 其实没跑起来"。所以这一组用例钉的是**"还等着外部给的位置有没有被如实列出来"**，
 * 以及两个方向都不能过头：
 *   · 该报的不能漏（漏一条模型就以为齐了）；
 *   · 已经填好值的不能报（全报出来等于没报，模型会被淹掉）。
 */
class WorkflowInputsTest {

    private fun node(classType: String, vararg inputs: Pair<String, Any?>) = buildJsonObject {
        put("class_type", classType)
        put(
            "inputs",
            kotlinx.serialization.json.buildJsonObject {
                inputs.forEach { (k, v) ->
                    when (v) {
                        // null 要真的写成 JSON null（不是"这个输入不存在"）：
                        // "值缺失"正是被测的那种缺口
                        null -> put(k, kotlinx.serialization.json.JsonNull)
                        is String -> put(k, v)
                        is Int -> put(k, v)
                        is List<*> -> put(
                            k,
                            kotlinx.serialization.json.buildJsonArray {
                                v.forEach { item ->
                                    add(kotlinx.serialization.json.JsonPrimitive(item.toString()))
                                }
                            },
                        )
                        else -> put(k, v.toString())
                    }
                }
            },
        )
    }

    private fun graph(vararg nodes: Pair<String, JsonObject>) = buildJsonObject {
        nodes.forEach { (id, n) -> put(id, n) }
    }

    // --- 用户现场：参考图 ---------------------------------------------------

    @Test
    fun `用户没给图时 工作流里的 LoadImage 必须报成缺图`() {
        val g = graph(
            "1" to node("CheckpointLoaderSimple", "ckpt_name" to "sdxl.safetensors"),
            // 绑着上次捕获时的那个 jpg —— 模型改不了它，也不该假装能用
            "89" to node("LoadImage", "image" to "4789521234.jpg", "upload" to "image"),
            "3" to node("KSampler", "seed" to 42, "steps" to 20),
            "9" to node("SaveImage", "images" to listOf("3", 0), "filename_prefix" to "ComfyUI"),
        )

        val report = WorkflowInputs.scan(g)

        assertTrue(report.hasExternalAssetGap, "有图片缺口时必须报出来")
        val img = report.missing.single { it.inputName == "image" }
        assertEquals("89", img.nodeId)
        assertEquals("LoadImage", img.classType)
        assertEquals(InputKind.image, img.kind)
        assertTrue(img.hint.contains("comfy_use_attachment"), img.hint)
        assertTrue(img.hint.contains("不要硬提交"), "要明确挡住「带着缺口提交」这条路：${img.hint}")
        // SaveImage 的 images 是"结果去哪儿"，不是"等谁给输入"
        assertTrue(report.missing.none { it.classType == "SaveImage" })
    }

    @Test
    fun `本次带了图附件时 图片位置记为可由附件解决 而不是要用户补`() {
        val g = graph(
            "89" to node("LoadImage", "image" to "old.jpg"),
            "3" to node("KSampler", "seed" to 1),
        )

        val withImage = WorkflowInputs.scan(g, hasImageAttachment = true)
        assertFalse(withImage.hasExternalAssetGap, "用户刚发了图：不该再去要一张")
        assertTrue(withImage.missing.none { it.kind == InputKind.image })
        assertEquals(listOf("89.image（本次附件可以投放）"), withImage.suppliedByAttachment)
        assertTrue(withImage.render().contains("不必问用户"), withImage.render())

        // 反面对照：没带图就必须报（否则这条用例证明不了什么）
        val without = WorkflowInputs.scan(g, hasImageAttachment = false)
        assertTrue(without.hasExternalAssetGap)
    }

    // --- 不能过头：已经填好的不许报 ------------------------------------------

    @Test
    fun `参数都有值的工作流报告为空 直接说可以提交`() {
        val g = graph(
            "1" to node("CheckpointLoaderSimple", "ckpt_name" to "sdxl.safetensors"),
            "3" to node("KSampler", "seed" to 42, "steps" to 20, "cfg" to 7),
            "9" to node("SaveImage", "images" to listOf("3", 0), "filename_prefix" to "out"),
        )

        val report = WorkflowInputs.scan(g)
        assertTrue(report.isEmpty, "没有缺口的图不该报任何东西：${report.missing}")
        assertFalse(report.hasExternalAssetGap)
        assertTrue(report.render().contains("可以直接提交"), report.render())
    }

    @Test
    fun `虚拟节点 Note 与 Reroute 不进清单`() {
        val g = graph(
            "1" to node("Note", "text" to "这里是说明"),
            "2" to node("Reroute", "input" to null),
            "3" to node("KSampler", "seed" to 5),
        )
        assertTrue(WorkflowInputs.scan(g).isEmpty)
    }

    @Test
    fun `空连线型输入报成 connection 并明确不要猜着接线`() {
        val g = graph(
            "5" to node("IPAdapterApply", "ipadapter" to listOf("7", 0)),
            "7" to node("IPAdapterUnifiedLoader", "preset" to null),
            "3" to node("KSampler", "seed" to 1),
        )

        val report = WorkflowInputs.scan(g)
        val preset = report.missing.single { it.nodeId == "7" }
        assertEquals(InputKind.parameter, preset.kind, "控件型没值 = 覆盖参数或问用户")
        val wire = report.missing.single { it.inputName == "ipadapter" }
        assertEquals(InputKind.connection, wire.kind)
        assertTrue(wire.hint.contains("不要猜着接一根线"), wire.hint)
    }

    @Test
    fun `视频类输入单独标 video 并说明本机没有投放通道`() {
        val g = graph("12" to node("VHS_LoadVideo", "video" to "old.mp4"))
        val report = WorkflowInputs.scan(g)
        val v = report.missing.single()
        assertEquals(InputKind.video, v.kind)
        assertTrue(v.hint.contains("投放视频"), v.hint)
    }

    // --- 图/视频缺口是"硬提交必然失败"的那一类 --------------------------------

    @Test
    fun `只有控件缺口时 needsExternalAsset 为假`() {
        val g = graph("7" to node("SomeCustomNode", "prompt" to null))
        val report = WorkflowInputs.scan(g)
        assertTrue(report.missing.isNotEmpty())
        assertFalse(report.hasExternalAssetGap, "没有图/视频缺口时不该把它说成「提交必然失败」")
    }

    // --- apiGraphOf：三种来源共用同一条判据 ----------------------------------

    @Test
    fun `apiGraphOf 认 API 图 不认界面格式与空对象`() {
        val api = buildJsonObject {
            put("1", buildJsonObject { put("class_type", "KSampler") })
        }
        assertNotNull(apiGraphOf(api))

        // 界面格式（nodes / links）**不是** API 图：得先 comfy_load_workflow 读进库
        val ui = buildJsonObject {
            put("nodes", kotlinx.serialization.json.buildJsonArray { })
            put("links", kotlinx.serialization.json.buildJsonArray { })
        }
        assertNull(apiGraphOf(ui), "界面格式不能被当成 API 图硬扫")

        assertNull(apiGraphOf(buildJsonObject { }), "空对象不是图")
        assertNull(apiGraphOf(null))
    }

    @Test
    fun `扫描报告 render 结论先行 把图类缺口点出来`() {
        val g = graph("89" to node("LoadImage", "image" to "old.jpg"))
        val text = WorkflowInputs.scan(g).render()
        assertTrue(text.contains("等着外部输入"), text)
        assertTrue(text.contains("图 / 视频类的缺口"), "结论要先说清「硬提交会失败」：$text")
        assertTrue(text.contains("89.image"), text)
    }
}
