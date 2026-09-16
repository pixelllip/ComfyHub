package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * 提交工作流前的参数覆盖（用户建议 ①：「让 AI 可以直接调用 Comfy 提交任务」）。
 *
 * 判据只有两条：**类型不能被改坏**（ComfyUI 会当场 400，用户却只看到一句莫名其妙的报错）、
 * 以及**改不了就说出来**（绝不静默忽略）。
 */
class WorkflowEditTest {

    private fun graph(): JsonObject = buildJsonObject {
        put(
            "3",
            buildJsonObject {
                put("class_type", JsonPrimitive("KSampler"))
                put(
                    "inputs",
                    buildJsonObject {
                        put("seed", JsonPrimitive(42))
                        put("steps", JsonPrimitive(20))
                        put("cfg", JsonPrimitive(7.5))
                        put("denoise", JsonPrimitive(1.0))
                        put("model", kotlinx.serialization.json.buildJsonArray {
                            add(JsonPrimitive("4"))
                            add(JsonPrimitive(0))
                        })
                    },
                )
            },
        )
        put(
            "6",
            buildJsonObject {
                put("class_type", JsonPrimitive("CLIPTextEncode"))
                put("inputs", buildJsonObject { put("text", JsonPrimitive("旧提示词")) })
            },
        )
    }

    private fun apply(vararg pairs: Pair<String, String>): Pair<JsonObject, List<String>> {
        val overrides = pairs.associate { (k, v) -> k to (JsonPrimitive(v) as kotlinx.serialization.json.JsonElement) }
        return ComfySubmitter.WorkflowEdit.applyOverrides(graph(), overrides)
    }

    private fun inputOf(g: JsonObject, node: String, field: String) =
        ((g[node] as JsonObject)["inputs"] as JsonObject)[field]

    @Test
    fun `文本提示词可以换掉`() {
        val (g, applied) = apply("6.text" to "新提示词：雨夜霓虹")
        assertEquals(JsonPrimitive("新提示词：雨夜霓虹"), inputOf(g, "6", "text"))
        assertEquals(listOf("6.text ← 新提示词：雨夜霓虹"), applied)
    }

    @Test
    fun `整数字段仍然是整数（不能变成字符串或小数）`() {
        val (g, _) = apply("3.steps" to "30")
        assertEquals(JsonPrimitive(30), inputOf(g, "3", "steps"))
        assertTrue(inputOf(g, "3", "steps").toString() == "30")
    }

    @Test
    fun `浮点字段保留小数`() {
        val (g, _) = apply("3.cfg" to "9.5")
        assertEquals(JsonPrimitive(9.5), inputOf(g, "3", "cfg"))
    }

    @Test
    fun `种子可以改（用户最常见的需求：换一张）`() {
        val (g, _) = apply("3.seed" to "123456789")
        assertEquals(JsonPrimitive(123456789L), inputOf(g, "3", "seed"))
    }

    @Test
    fun `类型不符要报错而不是静默忽略`() {
        val e = assertFailsWith<IllegalArgumentException> { apply("3.steps" to "很多步") }
        assertTrue(e.message!!.contains("3.steps"))
    }

    @Test
    fun `不存在的节点或参数要报错`() {
        assertFailsWith<IllegalArgumentException> { apply("99.steps" to "30") }
        assertFailsWith<IllegalArgumentException> { apply("3.不存在" to "1") }
    }

    @Test
    fun `路径写法不对要报错`() {
        assertFailsWith<IllegalArgumentException> { apply("steps" to "30") }
    }

    @Test
    fun `连线类输入（数组）不许覆盖`() {
        assertFailsWith<IllegalArgumentException> { apply("3.model" to "4") }
    }

    @Test
    fun `没写 overrides 时图原样不动`() {
        val (g, applied) = ComfySubmitter.WorkflowEdit.applyOverrides(graph(), emptyMap())
        assertEquals(graph().toString(), g.toString())
        assertTrue(applied.isEmpty())
    }
}
