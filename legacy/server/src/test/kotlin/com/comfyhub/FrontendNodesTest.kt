package com.comfyhub

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/**
 * ## 模拟用户行为 ##（用户建议 ②，2026-09-21）。
 *
 * 用户的原话：「加入 ##模拟用户行为## 模式，面对一些**仅面向前端的工作流节点**，
 * 让 AI 有能力在前端读取、编辑、在 comfy 前端提交」。
 *
 * 走的路线（用户当面确认）：**后端 + ComfyUI HTTP API**，不做浏览器自动化 ——
 * 也就是"把前端 JS 在排队前会做的那几件事，在后面等价地做掉"。
 *
 * 这里钉的每一条都是"弄错了用户查不出来"的地方：
 *  · 广播型节点（`Anything Everywhere`）只灌**同类型且空着**的输入，已接好的绝不覆盖；
 *  · **说不清类型就不猜**（照旧报 unsupported，出口还是那条"自己补线"的路）；
 *  · 显示节点丢掉、一个输出都没有时照前端的做法补 `SaveImage`；
 *  · 不开这个开关时**行为与过去一模一样**（默认 false，老用例一条都不动）。
 */
class FrontendNodesTest {

    private fun objectInfo(): JsonObject = AppJson.parseToJsonElement(
        """
        {
          "CheckpointLoaderSimple": {
            "input": { "required": { "ckpt_name": [["a.safetensors","b.safetensors"]] } },
            "output": ["MODEL", "CLIP", "VAE"]
          },
          "KSampler": {
            "input": { "required": {
              "model": ["MODEL"],
              "positive": ["CONDITIONING"],
              "negative": ["CONDITIONING"],
              "latent_image": ["LATENT"]
            } }
          },
          "CLIPTextEncode": {
            "input": { "required": { "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } }
          },
          "EmptyLatentImage": {
            "input": { "required": { "width": ["INT", {"default": 512}], "height": ["INT", {"default": 512}] } }
          },
          "SaveImage": {
            "input": { "required": { "images": ["IMAGE"] } }
          },
          "VAEDecode": {
            "input": { "required": { "samples": ["LATENT"], "vae": ["VAE"] } }
          }
        }
        """.trimIndent(),
    ) as JsonObject

    /**
     * 一份"用了广播节点"的界面格式工作流：
     *
     * ```
     * 1 Checkpoint ──MODEL──▶ 2 KSampler.model   （**已经接好**，广播不许再动它）
     * 4 Anything Everywhere（类型 VAE）──┐
     *                                   └─▶ 3 VAEDecode.vae   （空着，该被灌进来）
     * ```
     *
     * 广播声明的是 **VAE**：全图只有 `3.vae` 是 VAE 插口，而 `2.model`（MODEL）本来也接着线 ——
     * 两个条件同时成立才说明"只灌同类型 + 不碰已接好的"这两条都对。
     */
    private fun uiWithBroadcast(): JsonObject = AppJson.parseToJsonElement(
        """
        {
          "nodes": [
            { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
              "widgets_values": ["a.safetensors"], "inputs": [], "outputs": [{"name":"MODEL"}] },
            { "id": 2, "type": "KSampler", "mode": 0, "widgets_values": [],
              "inputs": [{"name":"model","link":1}] },
            { "id": 3, "type": "VAEDecode", "mode": 0, "widgets_values": [],
              "inputs": [{"name":"samples","link":2}] },
            { "id": 5, "type": "EmptyLatentImage", "mode": 0,
              "widgets_values": [512, 512], "inputs": [] },
            { "id": 4, "type": "Anything Everywhere", "mode": 0, "widgets_values": [],
              "properties": { "Input Type": "VAE" },
              "inputs": [{"name":"anything","link":3}] }
          ],
          "links": [
            [1, 1, 0, 2, 0, "MODEL"],
            [2, 5, 0, 3, 0, "LATENT"],
            [3, 1, 0, 4, 0, "MODEL"]
          ]
        }
        """.trimIndent(),
    ) as JsonObject

    private fun JsonObject.node(id: String) = this[id] as JsonObject
    private fun JsonObject.inputsOf(id: String) = node(id)["inputs"] as JsonObject

    @Test
    fun `不开模拟用户行为时行为与过去一模一样`() {
        val result = WorkflowConvert.toApiGraph(uiWithBroadcast(), objectInfo())
        assertEquals(1, result.unsupportedNodes.size, result.unsupportedNodes.toString())
        assertTrue(result.unsupportedNodes.single().contains("Anything Everywhere"))
        assertTrue(result.frontendRewritten.isEmpty())
        assertTrue(!result.graph.containsKey("4"))
    }

    @Test
    fun `广播节点把上游那一路灌进同类型且空着的输入 已接好的绝不覆盖`() {
        val result = WorkflowConvert.toApiGraph(uiWithBroadcast(), objectInfo(), emulateFrontend = true)

        assertTrue(
            result.unsupportedNodes.isEmpty(),
            "广播该被等价改写掉，不该再报转换不了：${result.unsupportedNodes}",
        )
        // VAEDecode.vae 被灌上 Checkpoint 的 MODEL 输出
        assertEquals(
            JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(0))),
            result.graph.inputsOf("3")["vae"],
        )
        // KSampler.model 本来就接着线，**一个字都不能动**
        assertEquals(
            JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(0))),
            result.graph.inputsOf("2")["model"],
        )
        // 广播节点自己不进图
        assertTrue(!result.graph.containsKey("4"))
        // "动了什么"要看得见
        assertTrue(result.broadcastLines.any { it.contains("3.vae") }, result.broadcastLines.toString())
        assertTrue(result.frontendRewritten.any { it.contains("Anything Everywhere") })
    }

    @Test
    fun `广播灌完之后 openInputs 要重算 不能留着已经接好的位置`() {
        val result = WorkflowConvert.toApiGraph(uiWithBroadcast(), objectInfo(), emulateFrontend = true)
        assertTrue(
            result.openInputs.none { it.startsWith("3.vae") },
            "3.vae 已经接好了，不该再挂在缺口清单里：${result.openInputs}",
        )
        // 说不清类型（没声明 Input Type）的广播不猜：照旧报 unsupported
        val vague = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [] },
                { "id": 3, "type": "VAEDecode", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"samples","link":2}] },
                { "id": 5, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512], "inputs": [] },
                { "id": 4, "type": "Anything Everywhere", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"anything","link":3}] }
              ],
              "links": [[2, 5, 0, 3, 0, "LATENT"], [3, 1, 0, 4, 0, "CLIP"]] }
            """.trimIndent(),
        ) as JsonObject
        val vagueResult = WorkflowConvert.toApiGraph(vague, objectInfo(), emulateFrontend = true)
        assertTrue(
            vagueResult.unsupportedNodes.any { it.contains("未声明") },
            "没声明类型就不许猜：${vagueResult.unsupportedNodes}",
        )
        assertTrue(vagueResult.broadcastLines.isEmpty())
    }

    @Test
    fun `显示节点丢掉 一个输出都没有时照前端的做法补 SaveImage`() {
        // 图里只有"生成"，一个 Save/Preview 都没有 —— 前端这时会自己挂一个 SaveImage
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512], "inputs": [] },
                { "id": 2, "type": "KSampler", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"latent_image","link":1}] },
                { "id": 3, "type": "VAEDecode", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"samples","link":2}] },
                { "id": 4, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [8, 8], "inputs": [] },
                { "id": 7, "type": "PreviewImage", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"images","link":3}] }
              ],
              "links": [
                [1, 1, 0, 2, 3, "LATENT"],
                [2, 2, 0, 3, 0, "LATENT"],
                [3, 3, 0, 7, 0, "IMAGE"]
              ] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo(), emulateFrontend = true)

        assertTrue(
            result.unsupportedNodes.isEmpty(),
            "显示节点该被丢掉而不是报错：${result.unsupportedNodes}",
        )
        assertTrue(result.frontendRewritten.any { it.contains("PreviewImage") })
        val injected = result.graph.entries.firstOrNull {
            ((it.value as? JsonObject)?.get("class_type") as? JsonPrimitive)?.contentOrNull == "SaveImage"
        }
        assertTrue(injected != null, "该补一个 SaveImage")
        val inputs = (injected!!.value as JsonObject)["inputs"] as JsonObject
        assertEquals(
            JsonArray(listOf(JsonPrimitive(3), JsonPrimitive(0))),
            inputs["images"],
            "补的那个 SaveImage 要接在真正的 IMAGE 来源上",
        )
    }

    @Test
    fun `已经有输出节点时不补 SaveImage`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512], "inputs": [] },
                { "id": 3, "type": "VAEDecode", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"samples","link":1}] },
                { "id": 9, "type": "SaveImage", "mode": 0, "widgets_values": ["x"],
                  "inputs": [{"name":"images","link":2}] }
              ],
              "links": [[1, 1, 0, 3, 0, "LATENT"], [2, 3, 0, 9, 0, "IMAGE"]] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo(), emulateFrontend = true)
        assertTrue(result.injectedNodes.isEmpty(), result.injectedNodes.toString())
        assertEquals(3, result.nodeCount)
    }
}
