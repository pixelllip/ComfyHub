package com.comfyhub

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import java.nio.file.Files
import java.nio.file.Path

/**
 * 本机 ComfyUI 已保存工作流文件的发现与列出（用户 bug ⑤）。
 *
 * 现场：用户说"基于 `krea2SFWNSFWUncensoredImageTo_v10` 生成"，AI 回"库里搜不到，
 * 要么把路径发我，要么你在 ComfyUI 里点一次 Queue 让它被自动捕获"。
 * 而那份文件一直躺在
 * `…\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2….json` 里 ——
 * `prompts` 库过去只收"捕获过的运行"，所以**首次使用时库里是空的**。
 *
 * 这里钉住"能不能看见"以及**只看见该看见的**：
 *  - Desktop 的安装目录（程序与共享数据分家）底下的 `user\default\workflows` 要能找到；
 *  - 多个用户目录都认；
 *  - 非 `.json`、空文件、坏 JSON 不进列表（不让 AI 拿到一个"看起来像工作流"的垃圾）；
 *  - 界面格式 / API 格式如实标出来（判据与加载那边同一份）。
 */
class ComfyWorkflowFilesTest {

    private fun tmp(): Path = Files.createTempDirectory("comfyhub-wf-files").toRealPath()

    /** ComfyUI Desktop 的真实布局：程序装在 Installs 下，工作流在**安装目录**的 user 目录里。 */
    private fun installWithWorkflows(root: Path): Path {
        val install = root.resolve("ComfyUI-Installs/ComfyUI/ComfyUI")
        Files.createDirectories(install.resolve("comfy"))
        Files.writeString(install.resolve("main.py"), "# comfyui")
        Files.createDirectories(install.resolve("user/default/workflows"))
        return install
    }

    private fun uiWorkflow(text: String) = """
        {"last_node_id": 1, "nodes": [{"id": 6, "type": "CLIPTextEncode", "widgets_values": ["$text"], "inputs": []}],
         "links": []}
    """.trimIndent()

    private fun apiWorkflow() = """
        {"3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20}}}
    """.trimIndent()

    @Test
    fun `安装目录底下的 user workflows 能被找到`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        val wf = install.resolve("user/default/workflows")
        Files.writeString(wf.resolve("krea2SFWNSFWUncensoredImageTo_v10.json"), uiWorkflow("cat"))

        val dirs = ComfyWorkflowFiles.dirsFromHomes(listOf(install))
        assertEquals(listOf(wf.toRealPath()), dirs, "工作流目录没被认出来：$dirs")
    }

    @Test
    fun `多个用户目录都认 没有 workflows 的目录不算`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        Files.createDirectories(install.resolve("user/another/workflows"))
        Files.createDirectories(install.resolve("user/__manager"))

        val names = ComfyWorkflowFiles.dirsFromHomes(listOf(install)).map { it.fileName.toString() }
        assertEquals(2, names.size, "default 与 another 两个用户目录都该认：$names")
        assertTrue(ComfyWorkflowFiles.dirsFromHomes(listOf(root.resolve("nothing-here"))).isEmpty())
    }

    @Test
    fun `列出文件名 格式 修改时间 与内容指纹`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        val wf = install.resolve("user/default/workflows")
        Files.writeString(wf.resolve("krea2_v10.json"), uiWorkflow("cat"))
        Files.writeString(wf.resolve("api_export.json"), apiWorkflow())

        val files = ComfyWorkflowFiles.scan(ComfyWorkflowFiles.dirsFromHomes(listOf(install)))
        assertEquals(2, files.size)
        assertEquals(setOf("krea2_v10.json", "api_export.json"), files.map { it.name }.toSet())
        assertEquals("ui", files.first { it.name == "krea2_v10.json" }.format, "nodes/links 就是界面格式")
        assertEquals("api", files.first { it.name == "api_export.json" }.format, "class_type 就是 API 格式")
        files.forEach {
            assertTrue(it.sha256.length == 64, "要有内容指纹（入库的 run_key 靠它）：$it")
            assertTrue(it.bytes > 0)
            assertTrue(it.modifiedAt != null)
        }
    }

    @Test
    fun `非 json 空文件 坏 json 都不进列表`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        val wf = install.resolve("user/default/workflows")
        Files.writeString(wf.resolve("readme.txt"), "not a workflow")
        Files.writeString(wf.resolve("empty.json"), "")
        Files.writeString(wf.resolve("broken.json"), "{ not json")
        Files.writeString(wf.resolve(".hidden.json"), uiWorkflow("cat"))
        Files.writeString(wf.resolve("good.json"), uiWorkflow("cat"))

        val files = ComfyWorkflowFiles.scan(ComfyWorkflowFiles.dirsFromHomes(listOf(install)))
        assertEquals(listOf("good.json"), files.map { it.name }, "只该留下能用的那一份：$files")
    }

    @Test
    fun `query 按文件名过滤（AI 说 krea2 就能直接命中）`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        val wf = install.resolve("user/default/workflows")
        Files.writeString(wf.resolve("krea2SFWNSFWUncensoredImageTo_v10.json"), uiWorkflow("cat"))
        Files.writeString(wf.resolve("MiaoMiao Harem.json"), uiWorkflow("cat"))

        val dirs = ComfyWorkflowFiles.dirsFromHomes(listOf(install))
        val hit = ComfyWorkflowFiles.scan(dirs, query = "KREA2")
        assertEquals(1, hit.size, "大小写不敏感的子串匹配：$hit")
        assertEquals("krea2SFWNSFWUncensoredImageTo_v10.json", hit.first().name)
        assertTrue(ComfyWorkflowFiles.scan(dirs, query = "没有这个名字").isEmpty())
    }

    @Test
    fun `listJson 把已入库的那份标出来 并说明下一步怎么做`() {
        val root = tmp()
        val install = installWithWorkflows(root)
        val wf = install.resolve("user/default/workflows")
        Files.writeString(wf.resolve("krea2_v10.json"), uiWorkflow("cat"))
        Files.writeString(wf.resolve("other.json"), uiWorkflow("dog"))
        val dirs = ComfyWorkflowFiles.dirsFromHomes(listOf(install))
        // 假装 krea2 那份已经在库里（真源是 run_key = file:<sha256>）
        val kreaSha = ComfyWorkflowFiles.scan(dirs).first { it.name == "krea2_v10.json" }.sha256

        val json = ComfyWorkflowFiles.listJson(dirs, query = null, limit = 20) { sha ->
            if (sha == kreaSha) 42L else null
        }
        val files = (json["files"] as JsonArray).map { it as JsonObject }
        assertEquals(2, files.size)
        val krea = files.first { (it["name"] as JsonPrimitive).content == "krea2_v10.json" }
        assertEquals(42L, (krea["promptId"] as JsonPrimitive).content.toLong())
        assertEquals("true", (krea["inLibrary"] as JsonPrimitive).content)
        val other = files.first { it !== krea }
        assertEquals("false", (other["inLibrary"] as JsonPrimitive).content)
        assertTrue((json["hint"] as JsonPrimitive).content.contains("comfy_load_workflow"))
    }

    @Test
    fun `一个目录都没有时如实说没有 而不是谎报空清单`() {
        val json = ComfyWorkflowFiles.listJson(emptyList(), query = null, limit = 20)
        assertEquals("0", (json["count"] as JsonPrimitive).content)
        val message = (json["message"] as JsonPrimitive).content
        assertTrue(message.contains("没找到"), message)
        assertTrue(message.contains("ComfyUI"), message)
    }
}
