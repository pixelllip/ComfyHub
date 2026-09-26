package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.file.Paths
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * 摘节点 / 拼节点（用户建议 ②，2026-09-20）。
 *
 * 用户的两个现场：
 *  - "我没给参考图，但是给了个需要参考图的工作流，要能识别出该工作流的骨干部分，
 *    把无关内容临时摘掉"；
 *  - "显存不足但是硬要跑，这时候可能需要考虑向工作流加入 Sage Attention"。
 *
 * 这里盯的就是这两件事**真的做到**，而不是"报了一句成功"：
 * 摘掉一条分支之后，下游那些指着它的线必须**接回它自己的上游**（否则 ComfyUI 报缺输入），
 * 输出节点必须仍然连通；插节点必须插在正确的那根线上。
 */
class WorkflowPruneTest {

    private fun graph(json: String): JsonObject = AppJson.parseToJsonElement(json) as JsonObject

    private fun nodeOf(graph: JsonObject, id: String): JsonObject = graph[id] as JsonObject

    private fun inputsOf(graph: JsonObject, id: String): JsonObject = nodeOf(graph, id)["inputs"] as JsonObject

    private fun textOf(element: JsonElement?): String = (element as? JsonPrimitive)?.content.orEmpty()

    /** 读一条连线 ["上游节点", 槽位]；不是连线就报错（断言消息里能看出问题）。 */
    private fun refOf(element: JsonElement?): Pair<String, String> {
        val arr = element as? JsonArray ?: error("不是一条连线")
        return textOf(arr.getOrNull(0)) to textOf(arr.getOrNull(1))
    }

    /** 经典的"文生图 + 参考图分支"：KSampler.positive 接在 ControlNet 上。 */
    private val withReferenceBranch = graph(
        """
        {
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "4": {"class_type":"LoraLoader","inputs":{"model":["1",0],"clip":["1",1],"lora_name":"x.safetensors"}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["4",1]}},
          "7": {"class_type":"CLIPTextEncode","inputs":{"text":"bad","clip":["4",1]}},
          "89": {"class_type":"LoadImage","inputs":{"image":"478952.jpg","upload":"image"}},
          "92": {"class_type":"ControlNetApplyAdvanced","inputs":{"positive":["6",0],"negative":["7",0],"control_net":["91",0],"image":["89",0],"strength":0.8}},
          "91": {"class_type":"ControlNetLoader","inputs":{"control_net_name":"canny.safetensors"}},
          "3": {"class_type":"KSampler","inputs":{"model":["4",0],"positive":["92",0],"negative":["92",1],"latent_image":["5",0],"seed":1,"steps":20,"cfg":7.0}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024,"batch_size":1}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
          "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}}
        }
        """.trimIndent()
    )

    @Test
    fun `摘掉参考图分支 ControlNet 与 LoadImage 一起走 采样器的线接回文本编码器`() {
        val r = WorkflowPrune.apply(withReferenceBranch, removeFrom = listOf("89.image"))

        assertTrue(
            r.warnings.any { it.contains("92.image") },
            "哪根线断了要说清楚：${r.warnings}",
        )
        // 采样器本来指着 92；92 还在（它还有消费者），所以采样器不动 ——
        // 想彻底不参考图，就把 92 也一起摘掉（见下一条用例）。
        val positive = refOf(inputsOf(r.graph, "3")["positive"])
        assertEquals("92", positive.first, "还有消费者的节点只被摘掉那根线，节点本身保留")
        assertTrue(r.unresolved.isEmpty(), "不该留下悬空的线：${r.unresolved}")
        assertTrue(r.runnable, "摘完应当能跑：alive=${r.outputsAlive}/${r.outputsTotal} ${r.warnings}")
    }

    @Test
    fun `摘掉一个共享节点不会误伤别人 文本编码器被两个分支共用`() {
        val g = graph(
            """
            {
              "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["1",1]}},
              "89": {"class_type":"LoadImage","inputs":{"image":"a.jpg"}},
              "92": {"class_type":"ControlNetApplyAdvanced","inputs":{"positive":["6",0],"image":["89",0]}},
              "3": {"class_type":"KSampler","inputs":{"positive":["92",0],"steps":20}},
              "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0]}},
              "10": {"class_type":"SaveImage","inputs":{"images":["9",0]}}
            }
            """.trimIndent()
        )
        val r = WorkflowPrune.apply(g, removeFrom = listOf("89"))
        assertTrue(r.graph.containsKey("6"), "共享的文本编码器不能被误摘")
        assertTrue(!inputsOf(r.graph, "92").containsKey("image"), "被摘的那根线要真的没了")
        assertTrue(!r.graph.containsKey("89"), "点名的 LoadImage 摘掉了")
        assertTrue(r.graph.containsKey("92"), "92 还有消费者（采样器），保留")
        assertTrue(r.graph.containsKey("3") && r.graph.containsKey("10"))
    }

    @Test
    fun `输出节点不会被摘 点名摘 SaveImage 要如实拒绝`() {
        val r = WorkflowPrune.apply(withReferenceBranch, removeFrom = listOf("10"))
        assertTrue(r.graph.containsKey("10"), "输出节点不许被摘掉（摘了就什么都出不来）")
        assertTrue(r.warnings.any { it.contains("保留") }, "要如实说明为什么没摘：${r.warnings}")
    }

    @Test
    fun `keep 里的节点不会被摘`() {
        val r = WorkflowPrune.apply(
            withReferenceBranch,
            removeFrom = listOf("89"),
            keep = setOf("89"),
        )
        assertTrue(r.graph.containsKey("92"), "keep 明确要求保留的节点不能摘")
        assertTrue(r.graph.containsKey("89"), "keep 明确要求保留的节点不能摘")
        assertTrue(!r.graph.containsKey("89") || r.graph.containsKey("92"), "上游留着，下游也就还在")
    }

    @Test
    fun `插节点 Sage Attention 接在模型链上 原来的上游接到它自己`() {
        val objectInfo = graph(
            """
            {
              "SageAttention": {"input":{"required":{"model":["MODEL"],"dtype":["COMBO",{"default":"fp16"}]}}}
            }
            """.trimIndent()
        )
        val r = WorkflowPrune.apply(
            withReferenceBranch,
            injections = listOf(WorkflowPrune.Injection(at = "3.model", classType = "SageAttention")),
            objectInfo = objectInfo,
        )
        assertEquals(1, r.injected.size, "要插进去一个节点：${r.injected} ${r.warnings}")
        val newId = r.graph.keys.first { textOf((r.graph[it] as JsonObject)["class_type"]) == "SageAttention" }
        val modelRef = refOf(inputsOf(r.graph, "3")["model"])
        assertEquals(newId, modelRef.first, "采样器的 model 要改成补丁节点")
        val patchModel = refOf(inputsOf(r.graph, newId)["model"])
        assertEquals("4", patchModel.first, "补丁的 model 要接回原来的上游 LoraLoader")
        assertTrue(r.runnable, "插完应当仍然能跑：${r.unresolved}")
    }

    @Test
    fun `没有节点定义表时照样插 但不猜接线之外的输入`() {
        val r = WorkflowPrune.apply(
            withReferenceBranch,
            injections = listOf(WorkflowPrune.Injection(at = "3.model", classType = "SageAttention")),
            objectInfo = null,
        )
        // 插入这件事本身是确定的（classType + 插在哪根线），所以定义表缺失不阻止插入；
        // 缺的是"它还有哪些输入"这类信息 —— 那种情况下宁可少插，也不乱接。
        assertTrue(r.injected.isNotEmpty(), "有明确的插入位置就该插：${r.injected}")
        assertTrue(r.graph.containsKey("3"), "采样器还在")
    }

    @Test
    fun `节点定义里没有这个类时只警告不强插`() {
        val objectInfo = graph("""{"SomeOtherNode":{"input":{"required":{"x":["INT"]}}}}""")
        val r = WorkflowPrune.apply(
            withReferenceBranch,
            injections = listOf(WorkflowPrune.Injection(at = "3.model", classType = "SageAttention")),
            objectInfo = objectInfo,
        )
        assertTrue(r.injected.isEmpty(), "这台 ComfyUI 没有的节点类不该硬插：${r.injected}")
        assertTrue(
            r.warnings.any { it.contains("SageAttention") },
            "要如实说明少了什么：${r.warnings}",
        )
    }

    @Test
    fun `摘掉之后留下空输入要列出来而不是假装没事`() {
        val g = graph(
            """
            {
              "89": {"class_type":"LoadImage","inputs":{"image":"a.jpg"}},
              "3": {"class_type":"KSampler","inputs":{"positive":["89",0],"steps":20}},
              "10": {"class_type":"SaveImage","inputs":{"images":["3",0]}}
            }
            """.trimIndent()
        )
        val r = WorkflowPrune.apply(g, removeFrom = listOf("89"))
        assertTrue(r.graph.containsKey("3"), "采样器还在")
        assertTrue(!inputsOf(r.graph, "3").containsKey("positive"), "没有可用的上游 → 这个输入被摘掉")
        assertTrue(
            r.warnings.any { it.contains("现在空着") },
            "必须如实说清哪根线没了：${r.warnings}",
        )
    }

    // --- 工具层接线（comfy_prune_workflow）-----------------------------------

    private val tmp = Paths.get(System.getProperty("java.io.tmpdir"))

    private fun skills() = SkillStore(
        builtinRoot = tmp.resolve("no-builtin"),
        userRoot = tmp.resolve("no-user"),
    )

    private fun toolEnv(comfyObjectInfo: (() -> JsonObject?)? = null): ToolRegistry =
        ToolRegistry(
            projectRoot = tmp,
            skills = skills(),
            approvals = ToolApprovalGate(),
            comfyStatus = { buildJsonObject { } },
            comfyFindRun = { null },
            comfySync = { buildJsonObject { } },
            comfyObjectInfo = comfyObjectInfo,
        )

    private fun ctx() = ToolContext(
        runId = "r1",
        policy = ToolPolicy(tmp, ToolPolicyConfig()),
        skills = skills(),
    )

    private fun args(vararg pairs: Pair<String, Any?>): String = buildJsonObject {
        pairs.forEach { (k, v) ->
            when (v) {
                null -> put(k, JsonNull)
                is String -> put(k, v)
                is Boolean -> put(k, v)
                is Int -> put(k, v)
                is JsonObject -> put(k, v)
                is JsonArray -> put(k, v)
                else -> put(k, v.toString())
            }
        }
    }.toString()

    @Test
    fun `工具层 给一份图与要摘的位置 返回改好的图与审计清单`() = runBlocking {
        val reg = toolEnv()
        val record = reg.invoke(
            "c1",
            "comfy_prune_workflow",
            args(
                "graph" to withReferenceBranch,
                "removeFrom" to buildJsonArray { add(JsonPrimitive("89.image")) },
            ),
            ctx(),
        )
        assertEquals("ok", record.status, record.error)
        val json = record.resultJson!!
        assertEquals("true", textOf(json["runnable"]))
        assertTrue((json["removed"] as JsonArray).isNotEmpty(), "摘了什么要如实列出来")
        val graphOut = json["graph"] as JsonObject
        assertTrue(!graphOut.containsKey("89"))
        assertTrue(graphOut.containsKey("10"))
    }

    // --- 无用节点分析（用户建议 ④，2026-09-22）-------------------------------
    //
    // 用户的原话："当用户提出类似'删除无用节点'时，分析出本地用不上的节点并向用户再次确认删除，
    // 然后执行操作。" 所以这里钉两件事：① 分析得准（哪条用不上、哪条只是没接完）；
    // ② 分析**一个字都不改**（确认之前不许动用户的工作流）。

    /** 一份骨干齐全、另加两条**用不上**的分支（放大链 + 换脸链）+ 一个没接完的孤立节点。 */
    private val withDeadBranches = graph(
        """
        {
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["1",1]}},
          "3": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["6",0],"latent_image":["5",0],"seed":1}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
          "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}},
          "70": {"class_type":"UpscaleModelLoader","inputs":{"model_name":"4x.pth"}},
          "71": {"class_type":"ImageUpscaleWithModel","inputs":{"upscale_model":["70",0],"image":["9",0]}},
          "80": {"class_type":"FaceSwap","inputs":{"image":["71",0],"face":["81",0]}},
          "81": {"class_type":"LoadImage","inputs":{"image":"face.png"}},
          "99": {"class_type":"Note","inputs":{"text":"还没接完的备注"}}
        }
        """.trimIndent()
    )

    @Test
    fun `分析：用不上的分支连成一片 孤立节点单独列出且不建议删`() {
        val report = WorkflowPrune.unusedBranches(withDeadBranches)

        // 80 / 81 / 70 / 71 谁都不喂给出图链路 —— 连成一片 = 一条分支
        assertEquals(4, report.totalUnused, "用不上的应当是换脸+放大那四个")
        assertEquals(1, report.branches.size, "它们连在一起，算一条：${report.branches.map { it.nodes.map { n -> n.id } }}")
        val branch = report.branches.first()
        assertEquals(setOf("70", "71", "80", "81"), branch.nodes.map { it.id }.toSet())
        // 入口 = 这条分支里谁也不引用的那个（换脸节点），摘它就是整条一起走
        assertEquals("80", branch.entry, "入口应当是分支的梢（下游那个）")
        assertEquals(setOf("70", "71", "80", "81"), branch.suggestRemove().toSet())
        // 人话标签（用户要照着它做决定，不能只给节点 id）
        assertTrue(branch.label.isNotBlank())
        assertTrue(branch.kinds.any { it.contains("放大") }, "节点种类要说人话：${branch.kinds}")

        // 99（没接任何线、也没人用）是"还没接完"，不归"用不上"
        assertEquals(listOf("99"), report.isolated.map { it.id })
        assertEquals(1, report.totalIsolated)
        assertEquals(11, report.totalNodes)
    }

    @Test
    fun `分析：干净的工作流一条都不报（宁可少报也不误报）`() {
        val report = WorkflowPrune.unusedBranches(withReferenceBranch)
        assertEquals(0, report.totalUnused, "这份图每个节点都在链路里")
        assertTrue(report.branches.isEmpty())
        assertTrue(report.toJson()["hint"].toString().contains("没有可摘"), "要明确说没有，而不是给一份空清单")
    }

    @Test
    fun `分析结果可以直接拿去摘：建议的入口能整条摘掉`() {
        val report = WorkflowPrune.unusedBranches(withDeadBranches)
        val removed = report.branches.flatMap { it.suggestRemove() }
        val r = WorkflowPrune.apply(withDeadBranches, removeFrom = removed)
        assertTrue(r.removed.any { it.startsWith("80") }, "摘除清单里要有换脸：${r.removed}")
        assertTrue(r.removed.any { it.startsWith("70") }, "放大节点也要跟着走：${r.removed}")
        assertTrue(r.graph.containsKey("10"), "出图链路必须还在")
        assertTrue(r.graph.containsKey("9"))
        assertTrue(r.runnable, "${r.unresolved} ${r.warnings}")
    }

    @Test
    fun `工具层 analyze 只分析不改图 并且要求先问用户再动手`() = runBlocking {
        val reg = toolEnv()
        val record = reg.invoke(
            "c1",
            "comfy_prune_workflow",
            args("graph" to withDeadBranches, "analyze" to true),
            ctx(),
        )
        assertEquals("ok", record.status, record.error)
        val json = record.resultJson!!
        // **一个字都不改**：没有 graph 字段（那是真摘才有的东西）
        assertTrue(json["graph"] == null, "analyze 绝不能带回改过的图")
        assertEquals("4", textOf(json["totalUnused"]))
        val branches = json["branches"] as JsonArray
        assertEquals("80", textOf((branches.first() as JsonObject)["removeFrom"]))
        val hint = textOf(json["hint"])
        assertTrue(hint.contains("先问用户"), "必须要求先确认：$hint")
        assertTrue(hint.contains("isolated") || hint.contains("还没接完"), "要提醒孤立节点别删：$hint")
    }

    @Test
    fun `工具层 什么都不要求也没开 analyze 时 报错并指路 analyze`() = runBlocking {
        val reg = toolEnv()
        val record = reg.invoke(
            "c1",
            "comfy_prune_workflow",
            args("graph" to withDeadBranches),
            ctx(),
        )
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
        assertTrue(record.error!!.contains("analyze"), "要告诉模型下一步怎么走：${record.error}")
    }

    @Test
    fun `工具层 什么都没要求就报 INVALID_ARGUMENT 不假装做了事`() = runBlocking {
        val reg = toolEnv()
        val record = reg.invoke(
            "c1",
            "comfy_prune_workflow",
            args("graph" to withReferenceBranch),
            ctx(),
        )
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
    }

    @Test
    fun `工具层 拿不到节点定义时只摘不拼 并说明原因`() = runBlocking {
        val reg = toolEnv(comfyObjectInfo = { null })
        val record = reg.invoke(
            "c1",
            "comfy_prune_workflow",
            args(
                "graph" to withReferenceBranch,
                "injections" to buildJsonArray {
                    add(
                        buildJsonObject {
                            put("at", "3.model")
                            put("classType", "SageAttention")
                        }
                    )
                },
            ),
            ctx(),
        )
        assertEquals("ok", record.status, record.error)
        val json = record.resultJson!!
        assertEquals("false", textOf(json["ok"]))
        assertEquals("COMFY_UNREACHABLE", textOf(json["reason"]))
        assertNotNull(json["detachedGraph"], "摘除部分的结果要照样给出来")
    }
}
