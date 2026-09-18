package com.comfyhub

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * 界面格式工作流 → API 格式节点图（用户 bug ③）。
 *
 * 这层转换的每一条都对应"猜错了用户查不出来"的风险，所以用例只钉**可验证的事实**：
 * 连线还原成 `["上游id", 槽位]`、控件值按声明顺序贴回名字上、种子后面那个
 * `control_after_generate` 槽位要跳过、纯前端节点要么被等价改写要么**明确报错**（不许猜）。
 */
class WorkflowConvertTest {

    /** 一份小号 `/object_info`：够覆盖控件顺序 / 种子附加槽位 / 强制插口三种情况。 */
    private fun objectInfo(): JsonObject = AppJson.parseToJsonElement(
        """
        {
          "CheckpointLoaderSimple": {
            "input": { "required": { "ckpt_name": [["a.safetensors","b.safetensors"]] } }
          },
          "CLIPTextEncode": {
            "input": { "required": { "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } }
          },
          "EmptyLatentImage": {
            "input": { "required": {
              "width": ["INT", {"default": 512}],
              "height": ["INT", {"default": 512}],
              "batch_size": ["INT", {"default": 1}]
            } }
          },
          "KSampler": {
            "input": { "required": {
              "model": ["MODEL"],
              "seed": ["INT", {"default": 0, "control_after_generate": true}],
              "steps": ["INT", {"default": 20}],
              "cfg": ["FLOAT", {"default": 7.0}],
              "sampler_name": [["euler","dpmpp_2m"]],
              "scheduler": [["normal","karras"]],
              "positive": ["CONDITIONING"],
              "negative": ["CONDITIONING"],
              "latent_image": ["LATENT"],
              "denoise": ["FLOAT", {"default": 1.0}]
            } }
          },
          "SaveImage": {
            "input": {
              "required": { "images": ["IMAGE"] },
              "optional": { "filename_prefix": ["STRING", {"default": "ComfyUI"}] }
            }
          },
          "ForceInputNode": {
            "input": { "required": {
              "value": ["STRING", {"forceInput": true}],
              "note": ["STRING", {"default": ""}]
            } }
          },
          "ModelPassthrough": {
            "input": { "required": { "model": ["MODEL"] } }
          }
        }
        """.trimIndent(),
    ) as JsonObject

    /**
     * 经典四节点工作流：Checkpoint → 正/负 CLIP → KSampler → SaveImage。
     *
     * 故意做成和真实文件同样的形状：`links` 是数组的数组，节点 `inputs[].link` 指链接 id。
     */
    private fun uiWorkflow(): JsonObject = AppJson.parseToJsonElement(
        """
        {
          "nodes": [
            { "id": 4, "type": "CheckpointLoaderSimple", "mode": 0,
              "widgets_values": ["a.safetensors"], "inputs": [], "outputs": [{"name":"MODEL"}] },
            { "id": 6, "type": "CLIPTextEncode", "mode": 0,
              "widgets_values": ["一只赛博朋克猫"], "inputs": [{"name":"clip","link":1}] },
            { "id": 7, "type": "CLIPTextEncode", "mode": 0,
              "widgets_values": ["worst quality"], "inputs": [{"name":"clip","link":2}] },
            { "id": 5, "type": "EmptyLatentImage", "mode": 0,
              "widgets_values": [896, 1264, 1], "inputs": [] },
            { "id": 3, "type": "KSampler", "mode": 0,
              "widgets_values": [20260918, "randomize", 30, 3.5, "euler", "normal", 1.0],
              "inputs": [
                {"name":"model","link":3},
                {"name":"positive","link":4},
                {"name":"negative","link":5},
                {"name":"latent_image","link":6}
              ] },
            { "id": 9, "type": "SaveImage", "mode": 0,
              "widgets_values": ["krea2/out"],
              "inputs": [{"name":"images","link":7}] }
          ],
          "links": [
            [1, 4, 1, 6, 0, "CLIP"],
            [2, 4, 1, 7, 0, "CLIP"],
            [3, 4, 0, 3, 0, "MODEL"],
            [4, 6, 0, 3, 1, "CONDITIONING"],
            [5, 7, 0, 3, 2, "CONDITIONING"],
            [6, 5, 0, 3, 3, "LATENT"],
            [7, 3, 0, 9, 0, "IMAGE"]
          ]
        }
        """.trimIndent(),
    ) as JsonObject

    private fun JsonObject.node(id: String) = this[id] as JsonObject
    private fun JsonObject.inputsOf(id: String) = node(id)["inputs"] as JsonObject

    @Test
    fun `连线还原成上游节点与槽位 控件值按声明顺序贴回名字`() {
        val result = WorkflowConvert.toApiGraph(uiWorkflow(), objectInfo())

        assertTrue(result.unsupportedNodes.isEmpty(), "不该有转换不了的节点：${result.unsupportedNodes}")
        assertEquals(6, result.nodeCount)

        // 连线：KSampler.model ← 节点 4 的 0 号输出
        assertEquals(
            JsonArray(listOf(JsonPrimitive(4), JsonPrimitive(0))),
            result.graph.inputsOf("3")["model"],
        )
        assertEquals(
            JsonArray(listOf(JsonPrimitive(6), JsonPrimitive(0))),
            result.graph.inputsOf("3")["positive"],
        )
        // 控件值：seed 之后跳过 control_after_generate 那个槽位，steps 才拿到 30
        assertEquals(JsonPrimitive(20260918L), result.graph.inputsOf("3")["seed"])
        assertEquals(JsonPrimitive(30), result.graph.inputsOf("3")["steps"])
        assertEquals(JsonPrimitive(3.5), result.graph.inputsOf("3")["cfg"])
        assertEquals(JsonPrimitive("euler"), result.graph.inputsOf("3")["sampler_name"])
        assertEquals(JsonPrimitive("normal"), result.graph.inputsOf("3")["scheduler"])
        assertEquals(JsonPrimitive(1.0), result.graph.inputsOf("3")["denoise"])

        // 宽高（三个控件按顺序对齐）
        assertEquals(JsonPrimitive(896), result.graph.inputsOf("5")["width"])
        assertEquals(JsonPrimitive(1264), result.graph.inputsOf("5")["height"])
        assertEquals(JsonPrimitive(1), result.graph.inputsOf("5")["batch_size"])

        // 文本节点的正文
        assertEquals(JsonPrimitive("一只赛博朋克猫"), result.graph.inputsOf("6")["text"])

        // 类名与标题
        assertEquals("KSampler", (result.graph.node("3")["class_type"] as JsonPrimitive).content)
    }

    @Test
    fun `forceInput 的输入不算控件 位置不会被它挤位`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 1, "type": "ForceInputNode", "mode": 0,
                           "widgets_values": ["hello"],
                           "inputs": [{"name":"value","link":9}] } ],
              "links": [[9, 2, 0, 1, 0, "STRING"]] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        // value 是 forceInput（插口）→ 走连线；唯一的控件 note 拿到 "hello"
        assertEquals(
            JsonArray(listOf(JsonPrimitive(2), JsonPrimitive(0))),
            result.graph.inputsOf("1")["value"],
        )
        assertEquals(JsonPrimitive("hello"), result.graph.inputsOf("1")["note"])
    }

    @Test
    fun `备注与原始值节点被丢掉 不会进提示词`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "Note", "mode": 0, "widgets_values": ["写点说明"] },
                { "id": 2, "type": "MarkdownNote", "mode": 0, "widgets_values": ["# 标题"] },
                { "id": 3, "type": "PrimitiveNode", "mode": 0, "widgets_values": [42] },
                { "id": 4, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512, 1] }
              ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(1, result.nodeCount, "界面专用节点不该进 API 图")
        assertTrue(result.graph.containsKey("4"))
        assertTrue(!result.graph.containsKey("1"))
        assertTrue(result.rewritten.any { it.contains("Note") })
    }

    @Test
    fun `静音的节点被摘掉 并且留下警告`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "CLIPTextEncode", "mode": 2, "widgets_values": ["不跑这句"], "inputs": [] },
                { "id": 2, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512, 1] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"positive","type":"CONDITIONING","link":8}] }
              ],
              "links": [[8, 1, 0, 3, 1, "CONDITIONING"]] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(!result.graph.containsKey("1"))
        assertTrue(
            result.warnings.any { it.contains("静音") && it.contains("3") },
            "静音节点的下游可能缺输入，必须留警告：${result.warnings}",
        )
    }

    @Test
    fun `被旁路的节点把输出接到同类型的输入上 这是真实存档里最常见的状态`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [],
                  "outputs": [{"name":"MODEL","type":"MODEL"}] },
                { "id": 2, "type": "ModelPassthrough", "mode": 4, "widgets_values": [],
                  "inputs": [{"name":"model","type":"MODEL","link":1}],
                  "outputs": [{"name":"MODEL","type":"MODEL"}] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"model","type":"MODEL","link":2}] }
              ],
              "links": [
                [1, 1, 0, 2, 0, "MODEL"],
                [2, 2, 0, 3, 0, "MODEL"]
              ] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(result.unsupportedNodes.isEmpty(), "${result.unsupportedNodes}")
        assertTrue(!result.graph.containsKey("2"), "旁路的节点自己不执行，不进提示词")
        assertEquals(
            JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(0))),
            result.graph.inputsOf("3")["model"],
            "旁路要穿透：下游拿到的是上游真正的输出",
        )
    }

    @Test
    fun `旁路的输出找不到同类型输入时留警告 不静默`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 2, "type": "ModelPassthrough", "mode": 4, "widgets_values": [],
                  "inputs": [], "outputs": [{"name":"MODEL","type":"MODEL"}] }
              ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(
            result.warnings.any { it.contains("旁路") && it.contains("同类型") },
            "接不过去必须说清楚：${result.warnings}",
        )
    }

    @Test
    fun `Reroute 与 SetNode GetNode 把连线接到真正的上游`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [] },
                { "id": 2, "type": "Reroute", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"","link":10}] },
                { "id": 3, "type": "SetNode", "mode": 0, "widgets_values": ["MODEL_BASE"],
                  "inputs": [{"name":"MODEL","link":11}] },
                { "id": 4, "type": "GetNode", "mode": 0, "widgets_values": ["MODEL_BASE"],
                  "inputs": [] },
                { "id": 5, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"model","link":12}] }
              ],
              "links": [
                [10, 1, 0, 2, 0, "MODEL"],
                [11, 2, 0, 3, 0, "MODEL"],
                [12, 4, 0, 5, 0, "MODEL"]
              ] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(result.unsupportedNodes.isEmpty(), "${result.unsupportedNodes}")
        // GetNode → SetNode → Reroute → 节点 1，一路穿透
        assertEquals(
            JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(0))),
            result.graph.inputsOf("5")["model"],
        )
        assertTrue(!result.graph.containsKey("2"))
        assertTrue(!result.graph.containsKey("3"))
        assertTrue(!result.graph.containsKey("4"))
    }

    @Test
    fun `纯前端节点转换不了就如实报错 不猜`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "Anything Everywhere", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"anything","link":1}] },
                { "id": 2, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512, 1] }
              ],
              "links": [[1, 2, 0, 1, 0, "LATENT"]] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(1, result.unsupportedNodes.size)
        assertTrue(result.unsupportedNodes.first().contains("Anything Everywhere"), result.unsupportedNodes.toString())
        // 转不出来的节点**不进**图（宁可整体失败，也不要半个提示词）
        assertTrue(!result.graph.containsKey("1"))
    }

    @Test
    fun `没有节点定义的自定义节点也如实报错`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 7, "type": "MyCustomNode", "mode": 0, "widgets_values": [1] } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(listOf("7 MyCustomNode"), result.unsupportedNodes)
    }

    @Test
    fun `未知的节点状态明确拒绝`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 3, "type": "KSampler", "mode": 9, "widgets_values": [1] } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(result.unsupportedNodes.single().contains("mode=9"))
    }

    @Test
    fun `控件值个数对不上时给警告 但仍然转出来`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                           "widgets_values": [512, 512, 1, "多出来的一个"] } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(1, result.nodeCount)
        assertTrue(result.warnings.any { it.contains("控件值") }, result.warnings.toString())
    }

    @Test
    fun `控件值存成对象时按名字取`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                           "widgets_values": {"width": 1024, "height": 1024, "batch_size": 2} } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(JsonPrimitive(1024), result.graph.inputsOf("1")["width"])
        assertEquals(JsonPrimitive(2), result.graph.inputsOf("1")["batch_size"])
    }

    @Test
    fun `新写法 links 对象也认`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 6, "type": "CLIPTextEncode", "mode": 0, "widgets_values": ["hi"], "inputs": [] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"positive","link":4}] }
              ],
              "links": [ {"id": 4, "origin_id": 6, "origin_slot": 0, "target_id": 3, "target_slot": 1} ] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(
            JsonArray(listOf(JsonPrimitive(6), JsonPrimitive(0))),
            result.graph.inputsOf("3")["positive"],
        )
    }

    @Test
    fun `isUiWorkflow 只认带 nodes 的界面格式`() {
        assertTrue(WorkflowConvert.isUiWorkflow(uiWorkflow()))
        assertTrue(!WorkflowConvert.isUiWorkflow(buildJsonObject { put("1", buildJsonObject { put("class_type", "KSampler") }) }))
    }

    // --- 2026-09-18：两条"不用猜"的增强 + 缺口清单 -------------------------

    @Test
    fun `带名字的 widgets_values_named 优先 且按输入声明过滤（用户 bug：参数对不齐）`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                           "widgets_values": [512, 512, 1],
                           "widgets_values_named": {"width":1024,"height":768,"batch_size":2,"bogus":9} } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())

        // 有名字就用名字：位置式那份完全被忽略（这正是"对不齐"的来源）
        assertEquals(JsonPrimitive(1024), result.graph.inputsOf("1")["width"])
        assertEquals(JsonPrimitive(768), result.graph.inputsOf("1")["height"])
        assertEquals(JsonPrimitive(2), result.graph.inputsOf("1")["batch_size"])
        // 声明里没有的键不能塞进去（ComfyUI 会当未知参数拒掉整张图），但要留一句话
        assertTrue(!result.graph.inputsOf("1").containsKey("bogus"))
        assertTrue(result.warnings.any { it.contains("bogus") }, result.warnings.toString())
        assertTrue(
            !result.warnings.any { it.contains("控件值有") },
            "带名字的那份不该再报「个数对不上」：${result.warnings}",
        )
    }

    @Test
    fun `组节点按定义展开：内部节点换新号 连线与控件值都接对 输出穿透到真正的产出节点`() {
        val result = WorkflowConvert.toApiGraph(subgraphWorkflow(), objectInfo())

        assertTrue(result.unsupportedNodes.isEmpty(), "组节点该被展开而不是报错：${result.unsupportedNodes}")
        // 顶层 1（CheckpointLoaderSimple）+ 20（KSampler）+ 展开出来的两个内部节点
        assertEquals(4, result.nodeCount)
        assertTrue(!result.graph.containsKey("10"), "被展开的组节点实例自己不该再出现")
        assertTrue(result.rewritten.any { it.contains("组节点") }, result.rewritten.toString())

        // 内部节点拿到的是没人用过的新号（内部 id 与主图撞号，必须重编）
        val innerLatent = result.graph.entries.first { (k, v) ->
            k != "1" && k != "20" && (v as JsonObject)["class_type"].asText() == "EmptyLatentImage"
        }
        val innerModel = result.graph.entries.first { (k, v) ->
            (v as JsonObject)["class_type"].asText() == "ModelPassthrough"
        }

        // ① 实例上的控件值（width=768）要填进内部节点的对应输入，而不是走控件顺序
        assertEquals(
            JsonPrimitive(768),
            (innerLatent.value as JsonObject).inputsOf()["width"],
            "组节点把 width 暴露到了外层，值必须原样传进去",
        )
        // ② 组节点的"连线型输入"要接上主图那边真正的来源
        assertEquals(
            JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(0))),
            (innerModel.value as JsonObject).inputsOf()["model"],
            "组节点输入喂进来的 MODEL 应该接回主图的 CheckpointLoaderSimple",
        )
        // ③ 组节点的输出要穿透到内部真正产出它的节点
        assertEquals(
            JsonArray(listOf(JsonPrimitive(innerLatent.key.toInt()), JsonPrimitive(0))),
            result.graph.inputsOf("20")["latent_image"],
            "下游拿到的必须是内部那个 EmptyLatentImage",
        )
        assertTrue(result.unresolvedInputs.isEmpty(), result.unresolvedInputs.toString())
    }

    @Test
    fun `组节点里的组节点这一版不展开 如实报错（不猜）`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 10, "type": "aaaaaaaa-0000-0000-0000-000000000000", "mode": 0,
                           "widgets_values": [], "inputs": [] } ],
              "links": [],
              "definitions": { "subgraphs": [
                { "id": "aaaaaaaa-0000-0000-0000-000000000000",
                  "inputNode": {"id": -10}, "outputNode": {"id": -20},
                  "inputs": [], "outputs": [],
                  "nodes": [ { "id": 1, "type": "bbbbbbbb-0000-0000-0000-000000000000" } ],
                  "links": [] },
                { "id": "bbbbbbbb-0000-0000-0000-000000000000",
                  "inputNode": {"id": -10}, "outputNode": {"id": -20},
                  "inputs": [], "outputs": [], "nodes": [], "links": [] }
              ] } }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertTrue(
            result.unsupportedNodes.any { it.contains("还套着一层组节点") },
            result.unsupportedNodes.toString(),
        )
    }

    @Test
    fun `静音节点留下的悬挂输入要点名报出来（不然 ComfyUI 只会说节点不存在）`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [
                { "id": 1, "type": "CLIPTextEncode", "mode": 2, "widgets_values": ["不跑"], "inputs": [] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"positive","type":"CONDITIONING","link":8}] }
              ],
              "links": [[8, 1, 0, 3, 1, "CONDITIONING"]] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        assertEquals(listOf("3.positive ← 节点 1（没有转换出来）"), result.unresolvedInputs)
    }

    @Test
    fun `空着的连线型输入列成清单（纯前端广播节点原来就是往这些地方灌值的）`() {
        val ui = AppJson.parseToJsonElement(
            """
            { "nodes": [ { "id": 1, "type": "ForceInputNode", "mode": 0,
                           "widgets_values": ["hello"], "inputs": [] } ],
              "links": [] }
            """.trimIndent(),
        ) as JsonObject
        val result = WorkflowConvert.toApiGraph(ui, objectInfo())
        // value 是 forceInput（插口）：空着 = 缺输入，要列出来
        assertTrue(result.openInputs.contains("1.value (STRING)"), result.openInputs.toString())
        // note 是普通控件：空着时 ComfyUI 用默认值，不算缺口
        assertTrue(!result.openInputs.any { it.contains("note") }, result.openInputs.toString())
    }

    @Test
    fun `空工作流报错 而不是转出一张空图`() {
        val empty = AppJson.parseToJsonElement("""{"nodes":[],"links":[]}""") as JsonObject
        val e = kotlin.runCatching { WorkflowConvert.toApiGraph(empty, objectInfo()) }.exceptionOrNull()
        assertTrue(e is IllegalArgumentException, "空工作流必须报错：$e")
    }

    /**
     * 一份带**组节点**的界面格式工作流，结构照着本机真实存档（krea2 那份）抄：
     * 主图 → 组节点实例（type 是 UUID）→ `definitions.subgraphs` 里的定义，
     * 定义用 `inputNode.id = -10` / `outputNode.id = -20` 两个代理节点表示接口。
     *
     * 这里刻意让内部节点 id 与主图**撞号**（内部也有 1 号节点）、
     * 内部连线 id 与主图也不共用一套编号 —— 这两件事真实文件里都会发生。
     */
    private fun subgraphWorkflow(): JsonObject = AppJson.parseToJsonElement(
        """
        {
          "nodes": [
            { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
              "widgets_values": ["a.safetensors"], "inputs": [],
              "outputs": [{"name":"MODEL","type":"MODEL"}] },
            { "id": 10, "type": "11111111-2222-3333-4444-555555555555", "mode": 0,
              "widgets_values_named": {"width": 768},
              "inputs": [
                {"name":"model","type":"MODEL","link":1},
                {"name":"width","type":"INT","widget":{"name":"width"},"link":null}
              ],
              "outputs": [{"name":"LATENT","type":"LATENT","links":[2]}] },
            { "id": 20, "type": "KSampler", "mode": 0,
              "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
              "inputs": [
                {"name":"model","type":"MODEL","link":3},
                {"name":"latent_image","type":"LATENT","link":2}
              ] }
          ],
          "links": [
            [1, 1, 0, 10, 0, "MODEL"],
            [2, 10, 0, 20, 1, "LATENT"],
            [3, 1, 0, 20, 0, "MODEL"]
          ],
          "definitions": { "subgraphs": [
            { "id": "11111111-2222-3333-4444-555555555555", "name": "新组节点",
              "inputNode": {"id": -10}, "outputNode": {"id": -20},
              "inputs": [
                {"name":"model","type":"MODEL","linkIds":[1401]},
                {"name":"width","type":"INT","linkIds":[1402]}
              ],
              "outputs": [ {"name":"LATENT","type":"LATENT","linkIds":[1403]} ],
              "nodes": [
                { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                  "widgets_values": [512, 512, 1],
                  "inputs": [{"name":"width","type":"INT","widget":{"name":"width"},"link":1402}],
                  "outputs": [{"name":"LATENT","type":"LATENT","links":[1403]}] },
                { "id": 2, "type": "ModelPassthrough", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"model","type":"MODEL","link":1401}] }
              ],
              "links": [
                {"id":1401,"origin_id":-10,"origin_slot":0,"target_id":2,"target_slot":0,"type":"MODEL"},
                {"id":1402,"origin_id":-10,"origin_slot":1,"target_id":1,"target_slot":0,"type":"INT"},
                {"id":1403,"origin_id":1,"origin_slot":0,"target_id":-20,"target_slot":0,"type":"LATENT"}
              ] }
          ] }
        }
        """.trimIndent(),
    ) as JsonObject

    /** 取一个已转出节点的 inputs（名字短一点，断言好读）。 */
    private fun JsonObject.inputsOf() = this["inputs"] as JsonObject
}

