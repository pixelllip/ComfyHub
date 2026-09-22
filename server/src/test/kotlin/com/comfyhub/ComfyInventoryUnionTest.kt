package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import java.nio.file.Files
import java.nio.file.Path

/**
 * 清单改成**并集**之后，"盘上有、但当前实例枚举不到"这件事必须被说清楚（2026-09-22 事故）。
 *
 * 现场（会话 `0bedb3f5`）：`launch_comfyui` 直接跑 `main.py`、没带 ComfyUI Desktop 的
 * `--extra-model-paths-config`，于是 `/object_info` 里 `diffusion_models\` 只有 2 个 minimax。
 * 旧实现"问得到 ComfyUI 就只用它"，所以 `anima-base-v1.0.safetensors`（4.18 GB，
 * 就躺在 `D:\Comfy-Desktop\ComfyUI-Shared\models\diffusion_models\`）被报成**"本机没有"**，
 * 模型据此差点让用户去下载一个本来就在硬盘上的文件。
 *
 * 这一组用例钉住三件事：
 *  ① 并集：盘上多的那些要进清单，但标 `visibleToInstance = false`，并单列进 `diskOnly`；
 *  ② 配对时**只有实例能加载的同名文件**才算"不用改"；只在盘上的走 `onDiskOnly`，
 *     既不进 `suggestedOverrides`，也不许算成 `missing`（那会引导用户去下载）；
 *  ③ 正文必须给出**正确动作**：重启 / 修挂载，**不是**下载。
 */
class ComfyInventoryUnionTest {

    private fun objectInfo(json: String): JsonObject = AppJson.parseToJsonElement(json) as JsonObject

    /** 一个"实例能加载的"清单：只有 krea2。 */
    private fun instanceInfo() = objectInfo(
        """{"CheckpointLoaderSimple":{"input":{"required":{"ckpt_name":[["krea2_turbo_int8.safetensors"]]}}}}""",
    )

    /** 造一个真实存在的 models 目录（模拟 `ComfyUI-Shared\models`）。 */
    private fun diskRoot(vararg names: String): Path {
        val root = Files.createTempDirectory("comfyhub-union").toRealPath()
        val ckpt = root.resolve("checkpoints")
        Files.createDirectories(ckpt)
        names.forEach { Files.write(ckpt.resolve(it), ByteArray(16)) }
        return root
    }

    @Test
    fun `清单是并集 盘上多的那些标成实例看不到`() {
        val disk = diskRoot("anima-base-v1.0.safetensors")
        val inv = ComfyModels.inventory(instanceInfo(), projectRoot = null, modelDirs = listOf(disk))

        assertEquals(ComfyModels.Source.OBJECT_INFO, inv.source, "权威来源仍然是实例（能不能加载它说了算）")
        assertTrue(inv.merged, "两份都拿到了就该是并集")

        val anima = inv.models.first { it.name == "anima-base-v1.0.safetensors" }
        assertFalse(anima.visibleToInstance, "实例的枚举里没有它")
        assertNotNull(anima.path, "盘上的那份要带路径 —— 报告里得说清它在哪")

        val krea = inv.models.first { it.name == "krea2_turbo_int8.safetensors" }
        assertTrue(krea.visibleToInstance, "枚举里有它 = 能加载")

        assertEquals(
            listOf("anima-base-v1.0.safetensors"),
            inv.diskOnly.map { it.name },
            "只有它属于'盘上有、实例看不到'",
        )
    }

    @Test
    fun `两边都有的同名文件只留一条 并带上盘上的路径`() {
        val disk = diskRoot("krea2_turbo_int8.safetensors")
        val inv = ComfyModels.inventory(instanceInfo(), projectRoot = null, modelDirs = listOf(disk))

        val same = inv.models.filter { it.name == "krea2_turbo_int8.safetensors" }
        assertEquals(1, same.size, "同名不许出现两条：${inv.models.map { it.name }}")
        assertTrue(same.first().visibleToInstance)
        assertNotNull(same.first().path, "并集的意义之一就是给实例清单补上体积与路径")
        assertTrue(inv.diskOnly.isEmpty(), "这个文件实例看得到，不算 diskOnly")
    }

    @Test
    fun `盘上有同名但实例加载不了时报 onDiskOnly 并明说不要去下载`() {
        val path = "D:\\Comfy-Desktop\\ComfyUI-Shared\\models\\diffusion_models\\anima-base-v1.0.safetensors"
        val model = ComfyModels.LocalModel(
            name = "anima-base-v1.0.safetensors",
            folder = "diffusion_models",
            path = path,
            bytes = 4_182_218_328,
            visibleToInstance = false,
        )
        val inv = ComfyModels.Inventory(
            source = ComfyModels.Source.OBJECT_INFO,
            models = listOf(model),
            diskOnly = listOf(model),
            merged = true,
        )
        val graph = objectInfo(
            """{"79":{"class_type":"UNETLoader","inputs":{"unet_name":"anima-base-v1.0.safetensors"}}}""",
        )

        val report = WorkflowFit.fit(graph, inv)
        val item = report.items.single()
        assertEquals(FitStatus.ON_DISK_ONLY, item.status)
        assertFalse(item.safeToAutoApply, "实例加载不了，绝不许自动替换")
        assertTrue(report.suggestedOverrides().isEmpty(), "改了也是文件不存在，不许进 overrides")

        val text = report.render()
        assertTrue(text.contains("不要下载") || text.contains("不要去下载"), "必须挡住'去下载'这条路：$text")
        assertTrue(text.contains("重启"), "要给出正确动作（重启 / 修挂载）：$text")
        assertTrue(text.contains(path), "要说清文件到底在哪：$text")

        val json = report.toJson()
        assertEquals("1", (json["onDiskOnly"] as JsonPrimitive).content)
        assertEquals("0", (json["missing"] as JsonPrimitive).content, "不许算成 missing（那会把用户引去下载）")
        assertEquals("1", (json["invisibleToInstance"] as JsonPrimitive).content)
    }

    @Test
    fun `候选全都看不到时 明说别拿它们去 overrides`() {
        val inv = ComfyModels.Inventory(
            source = ComfyModels.Source.OBJECT_INFO,
            models = listOf(
                ComfyModels.LocalModel(
                    name = "krea2_turbo_int8_v2.safetensors",
                    folder = "checkpoints",
                    visibleToInstance = false,
                ),
            ),
            diskOnly = listOf(
                ComfyModels.LocalModel(
                    name = "krea2_turbo_int8_v2.safetensors",
                    folder = "checkpoints",
                    visibleToInstance = false,
                ),
            ),
            merged = true,
        )
        val graph = objectInfo(
            """{"1":{"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2_turbo_int8.safetensors"}}}""",
        )

        val report = WorkflowFit.fit(graph, inv)
        assertEquals(FitStatus.SUBSTITUTE, report.items.single().status)
        assertEquals(
            "false",
            ((report.toJson()["models"] as kotlinx.serialization.json.JsonArray)[0]
                .let { it as JsonObject }["candidates"] as kotlinx.serialization.json.JsonArray)[0]
                .let { it as JsonObject }["visibleToInstance"]!!.let { (it as JsonPrimitive).content },
            "候选必须带上'实例能不能加载'这一位",
        )
        val text = report.render()
        assertTrue(text.contains("看不到"), "全看不到时要说清：$text")
        assertTrue(text.contains("重启"), "正确动作是重启，不是换模型：$text")
    }

    @Test
    fun `并集为空时不硬凑 diskOnly`() {
        val inv = ComfyModels.inventory(instanceInfo(), projectRoot = null, modelDirs = emptyList())
        assertFalse(inv.merged, "没扫到盘就不算并集")
        assertTrue(inv.diskOnly.isEmpty())
        assertEquals(
            listOf("krea2_turbo_int8.safetensors"),
            inv.models.map { it.name },
            "拿不到盘就还是实例那一份（既有的行为不许变）",
        )
    }
}
