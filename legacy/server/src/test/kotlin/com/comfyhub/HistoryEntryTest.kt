package com.comfyhub

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * `/history` 运行记录的解析测试。
 *
 * 这里钉的是一个**真实踩过的坑**：ComfyUI 0.34.2 把入队的 `prompt` 从 3 元组扩成了 6 元组
 * `[编号, prompt_id, 节点图, extra_data, 要执行的输出节点, 敏感数据]`，
 * 而 history 存的就是这一整条。
 * 旧实现按 `prompt[0]` / `prompt[1]` 取节点图和 extra_data —— 升级后取到的是数字和字符串，
 * 于是轮询捕获出来的提示词、参数、工作流**全空**（agent / 脚本提交的运行尤其明显）。
 *
 * fixture 直接抄自本机 0.34.2 的真实 `/history`（anima 文生图那两条运行）。
 */
class HistoryEntryTest {

    private fun obj(json: String): JsonObject = Json.parseToJsonElement(json) as JsonObject

    private fun arr(json: String): JsonArray = Json.parseToJsonElement(json) as JsonArray

    private val graphJson = """
        {
          "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "anima-base-v1.0.safetensors", "weight_dtype": "default"}},
          "4": {"class_type": "CLIPTextEncode", "inputs": {"text": "score_9, light blue hair", "clip": ["2", 0]}},
          "8": {"class_type": "KSampler", "inputs": {"seed": 771234, "steps": 30, "cfg": 4.0,
                "sampler_name": "er_sde", "scheduler": "simple", "denoise": 1.0,
                "model": ["1", 0], "positive": ["4", 0], "negative": ["5", 0], "latent_image": ["6", 0]}},
          "10": {"class_type": "SaveImage", "inputs": {"images": ["9", 0], "filename_prefix": "anima_snowgirl_c1"}}
        }
    """.trimIndent()

    @Test
    fun `0_34_2 的六元组能取到节点图与 extra_data`() {
        val entry = obj(
            """
            {
              "prompt": [1, "e38d31e3-7f0f-48be-a872-c442903d6885", $graphJson,
                         {"client_id": "anima", "create_time": 1789133696509}, ["10"]],
              "outputs": {"10": {"images": [{"filename": "anima_snowgirl_c1_00001_.png", "type": "output"}]}},
              "status": {"status_str": "success", "completed": true, "messages": []}
            }
            """.trimIndent()
        )

        val parsed = HistoryEntry.parse(entry)
        assertNotNull(parsed.graph, "节点图必须能取到（按下标取会拿到数字 1）")
        assertTrue(HistoryEntry.isNodeGraph(parsed.graph!!))
        assertEquals("UNETLoader", parsed.graph!!["1"]!!.let { (it as JsonObject).cls() })
        assertEquals("anima", parsed.extraData?.get("client_id").toString().trim('"'))
        assertNull(parsed.workflow, "agent 提交时本来就没带界面工作流")
    }

    @Test
    fun `六元组里带界面工作流时能取到`() {
        val entry = obj(
            """
            {
              "prompt": [2, "abc-123", $graphJson,
                         {"client_id": "web", "extra_pnginfo": {"workflow": {"nodes": [], "links": [], "version": 0.4}}},
                         ["10"], {}],
              "outputs": {}, "status": {"status_str": "success", "completed": true}
            }
            """.trimIndent()
        )

        val parsed = HistoryEntry.parse(entry)
        assertNotNull(parsed.workflow)
        assertEquals(0.4, parsed.workflow!!["version"].toString().toDouble())
    }

    @Test
    fun `老版本的三元组同样能取到`() {
        val entry = obj(
            """
            {
              "prompt": [$graphJson,
                         {"client_id": "web", "extra_pnginfo": {"workflow": {"nodes": [], "version": 0.4}}},
                         ["10"]],
              "outputs": {}, "status": {"status_str": "success", "completed": true}
            }
            """.trimIndent()
        )

        val parsed = HistoryEntry.parse(entry)
        assertNotNull(parsed.graph)
        assertNotNull(parsed.workflow)
        assertEquals("web", parsed.extraData?.get("client_id").toString().trim('"'))
    }

    @Test
    fun `坏数据不炸：空对象 缺字段 类型乱来`() {
        assertNull(HistoryEntry.parse(obj("""{}""")).graph)
        assertNull(HistoryEntry.parse(obj("""{"prompt": null}""")).graph)
        assertNull(HistoryEntry.parse(obj("""{"prompt": "oops"}""")).graph)
        // 只有数字和字符串的元组：认不出节点图，但也不能抛异常
        val weird = HistoryEntry.parse(obj("""{"prompt": [1, "id", [], {}]}"""))
        assertNull(weird.graph)
        assertNull(weird.workflow)
    }

    @Test
    fun `节点图判定不会把 extra_data 或空对象误认成图`() {
        assertTrue(HistoryEntry.isNodeGraph(obj(graphJson)))
        // extra_data 的值是标量 → 不是图
        assertTrue(!HistoryEntry.isNodeGraph(obj("""{"client_id": "anima", "create_time": 1}""")))
        // 空对象不算图（ComfyUI 的 sensitive 字段常常就是 {}）
        assertTrue(!HistoryEntry.isNodeGraph(obj("""{}""")))
    }

    @Test
    fun `元组里的敏感数据不会被当成 extra_data`() {
        val entry = obj(
            """
            {
              "prompt": [3, "abc", $graphJson, {"client_id": "web"}, ["10"], {"auth_token": "secret"}],
              "outputs": {}, "status": {"status_str": "success", "completed": true}
            }
            """.trimIndent()
        )
        val parsed = HistoryEntry.parse(entry)
        assertEquals("web", parsed.extraData?.get("client_id").toString().trim('"'))
    }
}
