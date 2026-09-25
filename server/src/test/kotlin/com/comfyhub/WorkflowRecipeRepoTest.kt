package com.comfyhub

import kotlinx.serialization.json.JsonPrimitive
import org.junit.jupiter.api.AfterAll
import java.nio.file.Files
import java.nio.file.Paths
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * 工作流配方仓储（W2）的回归防线。
 *
 * 盯的是五件**删了以后出错也没人知道**的事：
 * ① 同一份来源 = 同一条配方（幂等），而且**行 id 不变**（界面上的链接不会失效）；
 * ② 重新学一份会**整行更新**（不是留着旧槽位）；
 * ③ `prompt_id` 是 `ON DELETE SET NULL`：用户删掉那条提示词，配方还在（红线 R1 在这张表上的落点）；
 * ④ 这张表**不进同步账本**（`_sync_log` 里一行都不许有它）；
 * ⑤ DTO → `Recipe` 还原后 `render()` 还能给出"改哪个键"（详情页与 AI 拿到的是同一份正文）。
 *
 * 连真库（[TestDb] 是临时文件库，理由见 `SqliteFoundationTest.kt`）。
 */
class WorkflowRecipeRepoTest {

    private fun sampleRecipe(): WorkflowRecipe.Recipe = WorkflowRecipe.analyze(
        AppJson.parseToJsonElement(
            """
            {
              "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"a.safetensors"}},
              "6": {"class_type":"CLIPTextEncode","inputs":{"clip":["1",1],"text":"a cat"}},
              "5": {"class_type":"EmptyLatentImage","inputs":{"width":1024,"height":1024,"batch_size":1}},
              "3": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["6",0],"latent_image":["5",0],
                     "seed":42,"steps":8,"cfg":1.0,"sampler_name":"euler","scheduler":"simple","denoise":1.0}},
              "8": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
              "9": {"class_type":"SaveImage","inputs":{"images":["8",0]}}
            }
            """.trimIndent(),
        ) as kotlinx.serialization.json.JsonObject,
    )

    private fun write(
        name: String = "krea2 编辑",
        sourceRef: String? = "file:abc",
        promptId: Long? = null,
        complete: Boolean = true,
        gaps: List<String> = emptyList(),
        task: String? = null,
    ): WorkflowRecipeRepo.Write {
        val recipe = sampleRecipe()
        return WorkflowRecipeRepo.writeOf(
            recipe = recipe,
            name = name,
            sourceKind = "file",
            sourceRef = sourceRef,
            sourcePath = "D:/wf/$name.json",
            promptId = promptId,
            complete = complete,
            gaps = gaps,
        ).let { if (task == null) it else it.copy(task = task) }
    }

    // -----------------------------------------------------------------------

    @Test
    fun `同一份来源学两次只有一行 且 id 不变`() {
        TestDb.reset()
        val (first, createdFirst) = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write()) }
        assertTrue(createdFirst)
        val (second, createdSecond) = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write(name = "改了个名字")) }
        assertEquals(first, second, "同一份来源必须复用同一行")
        assertFalse(createdSecond)
        assertEquals(1, WorkflowRecipeRepo.list().size)
        assertEquals("改了个名字", assertNotNull(WorkflowRecipeRepo.get(first)).name)
    }

    @Test
    fun `重新学一份会整行更新 不留旧槽位`() {
        TestDb.reset()
        val id = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write()).first }
        // 第二遍"学到了别的形状"（换了骨架、槽位只剩一个）
        val leaner = WorkflowRecipeRepo.Write(
            name = "krea2 编辑",
            task = "edit",
            skeletonHash = "ffff000011112222",
            sourceKind = "file",
            sourceRef = "file:abc",
            slotsJson = """[{"role":"loras","nodeId":"71","classType":"LoraLoaderModelOnly","inputName":"lora_name","fileName":"x.safetensors","overrideKey":"71.lora_name","roleGuessed":false}]""",
            knobsJson = "[]",
            classesJson = """["LoraLoaderModelOnly"]""",
            edgesJson = "[]",
            outputsJson = "[]",
        )
        Db.tx { conn -> WorkflowRecipeRepo.save(conn, leaner) }
        val row = assertNotNull(WorkflowRecipeRepo.get(id))
        assertEquals("ffff000011112222", row.skeletonHash)
        assertEquals("edit", row.task)
        assertEquals(1, row.slots.size, "旧的四个槽位必须被覆盖掉，不许残留")
        assertEquals("71.lora_name", (row.slots[0] as kotlinx.serialization.json.JsonObject).let {
            (it["overrideKey"] as JsonPrimitive).content
        })
    }

    @Test
    fun `删掉提示词之后配方还在 只是 promptId 变空`() {
        TestDb.reset()
        val promptId = Db.tx { conn ->
            conn.executeReturningKey(
                "INSERT INTO prompts (title, positive_prompt, source) VALUES ('工作流','x','ComfyUI-File')",
            )
        }
        val id = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write(promptId = promptId)).first }
        assertEquals(promptId, assertNotNull(WorkflowRecipeRepo.get(id)).promptId)

        Db.tx { conn -> conn.execute("DELETE FROM prompts WHERE id = ?", promptId) }
        val row = assertNotNull(WorkflowRecipeRepo.get(id), "删提示词不该把配方一起删掉")
        assertEquals(null, row.promptId, "外键 ON DELETE SET NULL（R1：每条连接都要开 foreign_keys）")
    }

    @Test
    fun `配方不进同步账本`() {
        TestDb.reset()
        val id = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write()).first }
        Db.tx { conn -> WorkflowRecipeRepo.save(conn, write(name = "又学了一遍")) }
        Db.tx { conn -> WorkflowRecipeRepo.delete(conn, id) }
        val logged = Db.withConnection { conn ->
            conn.queryOne("SELECT COUNT(*) FROM _sync_log WHERE tbl = 'workflow_recipes'") { it.getLong(1) }
        }
        assertEquals(0L, logged, "这张表刻意不同步（总纲 §3.3）；要同步必须连触发器与 sync.rs 一起改")
    }

    @Test
    fun `缺件的配方照样存 但要能按 complete 过滤出来`() {
        TestDb.reset()
        Db.tx { conn ->
            WorkflowRecipeRepo.save(conn, write(name = "完整的", sourceRef = "file:ok"))
        }
        Db.tx { conn ->
            WorkflowRecipeRepo.save(
                conn,
                write(name = "缺件的", sourceRef = "file:gap", complete = false, gaps = listOf("摘掉节点类：Krea2EditModelPatch")),
            )
        }
        assertEquals(2, WorkflowRecipeRepo.list().size, "默认要能看见缺件的（只是带着标记）")
        assertEquals(listOf("完整的"), WorkflowRecipeRepo.list(onlyComplete = true).map { it.name })
        val gap = WorkflowRecipeRepo.list().first { it.name == "缺件的" }
        assertFalse(gap.complete)
        assertEquals(listOf("摘掉节点类：Krea2EditModelPatch"), gap.gaps)
    }

    @Test
    fun `列表能按 task 与骨架过滤 关键字能搜到模型文件名`() {
        TestDb.reset()
        val editWrite = write(name = "编辑流", sourceRef = "file:e", task = "edit").copy(
            slotsJson = """[{"role":"loras","nodeId":"71","classType":"LoraLoaderModelOnly","inputName":"lora_name","fileName":"krea2_identity_edit.safetensors","overrideKey":"71.lora_name","roleGuessed":false}]""",
        )
        Db.tx { conn -> WorkflowRecipeRepo.save(conn, editWrite) }
        Db.tx { conn ->
            // 换一个骨架指纹：这一条是"另一族"（同族 = 同一个 skeleton_hash）
            WorkflowRecipeRepo.save(
                conn,
                write(name = "文生图", sourceRef = "file:t", task = "t2i").copy(skeletonHash = "0000111122223333"),
            )
        }
        assertEquals(listOf("编辑流"), WorkflowRecipeRepo.list(task = "edit").map { it.name })
        val skeleton = WorkflowRecipeRepo.list(task = "edit").first().skeletonHash
        assertEquals(1, WorkflowRecipeRepo.list(skeletonHash = skeleton).size)
        // 关键字的匹配面要盖住槽位 JSON（"这个模型在哪条配方里"是模型最常问的一件事）
        assertEquals(listOf("编辑流"), WorkflowRecipeRepo.list(q = "identity_edit").map { it.name })
        assertEquals(1, Db.withConnection { conn -> WorkflowRecipeRepo.countBySkeleton(conn, skeleton) })
    }

    @Test
    fun `详情能从落库的形状还原出可渲染的配方`() {
        TestDb.reset()
        val id = Db.tx { conn -> WorkflowRecipeRepo.save(conn, write()).first }
        val row = assertNotNull(WorkflowRecipeRepo.get(id))

        val recipe = row.toRecipe()
        assertEquals(WorkflowRecipe.Task.T2I, recipe.task)
        assertEquals(16, recipe.skeletonHash.length, "指纹是 8 字节 = 16 位十六进制")
        assertEquals(listOf("checkpoints"), recipe.slots.map { it.role })
        assertTrue(recipe.knobs.any { it.overrideKey == "6.text" })
        assertTrue(recipe.outputs.any { it.classType == "SaveImage" })

        val text = row.renderText()
        assertTrue(text.contains("1.ckpt_name = a.safetensors"), text)
        assertTrue(text.contains("提示词："), text)
        assertEquals(row.name, text.substringAfter("【工作流配方】").substringBefore('\n'))
    }

    @Test
    fun `写不进去就报错 不静默截断`() {
        TestDb.reset()
        val e = assertFailsWith<IllegalArgumentException> {
            Db.tx { conn -> WorkflowRecipeRepo.save(conn, write(name = "x".repeat(WorkflowRecipeRepo.MAX_NAME_CHARS + 1))) }
        }
        assertTrue(e.message!!.contains("太长"), e.message!!)

        // 指向一条不存在的提示词：报人话，不是 500 + SQL 原文
        val e2 = assertFailsWith<IllegalArgumentException> {
            Db.tx { conn -> WorkflowRecipeRepo.save(conn, write(promptId = 999_999)) }
        }
        assertTrue(e2.message!!.contains("不存在"), e2.message!!)
        assertTrue(WorkflowRecipeRepo.list().isEmpty(), "报错的行不许半截写进去")

        // task 不许为空（认不出就写 unknown，别编）
        assertFailsWith<IllegalArgumentException> {
            Db.tx { conn -> WorkflowRecipeRepo.save(conn, write().copy(task = " ")) }
        }
    }

    // -----------------------------------------------------------------------

    @Test
    fun `真文件学一遍能落库并读回同一份配方`() {
        val file = Paths.get("..", "comfyui", "workflows", "comfyhub_inpaint_krea2.json")
        if (!Files.isRegularFile(file)) return
        TestDb.reset()
        val deps = WorkflowRecipeLearner.Deps(
            objectInfo = { null },
            apiGraphOf = { null },
            promptName = { null },
            findPromptIdByRunKey = { null },
        )
        val learned = WorkflowRecipeLearner.learnFile(file, deps = deps)
        assertEquals(WorkflowRecipe.Task.INPAINT, learned.recipe.task)
        assertTrue(learned.complete, "API 格式零转换，不该有缺口")
        assertEquals("file:" + sha256Of(file), learned.sourceRef)

        val (id, _) = WorkflowRecipeLearner.learnAndSave(learned)
        val row = assertNotNull(WorkflowRecipeRepo.get(id))
        assertEquals("inpaint", row.task)
        assertEquals(learned.recipe.skeletonHash, row.skeletonHash)
        assertEquals(3, row.slots.size)
        // 再学一遍：还是同一行
        val (again, _) = WorkflowRecipeLearner.learnAndSave(WorkflowRecipeLearner.learnFile(file, deps = deps))
        assertEquals(id, again)
        assertEquals(1, WorkflowRecipeRepo.list().size, "幂等：同一份文件学两次只有一行")
    }

    private fun sha256Of(path: java.nio.file.Path): String =
        java.security.MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(path))
            .joinToString("") { "%02x".format(it) }

    companion object {
        @AfterAll
        @JvmStatic
        fun cleanup() = TestDb.cleanup()
    }
}
