package com.comfyhub.ai.mcp

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * MCP stdio 客户端的端到端回归（2026-09-21）。
 *
 * 为什么不 mock：这个客户端的坑**全在协议与进程上** —— id 对不上、通知没有 id 却去等回复、
 * stdout 混进非 JSON 的噪音行、stderr 不排空把子进程堵死、进程死了不重起……
 * 这些只有**真起一个子进程**才能抓到（与 AGENTS §10.1"假 SSE 的形状必须照着真事件抄"同一条教训）。
 *
 * 所以这里用 `src/test/resources/mcp/fake_mcp_server.py`：它的形状是照着**真 comfy-mcp 实测**
 * 抄的（`initialize` → `tools/list` → `tools/call`），连"stdout 漏一行警告"都留着。
 * 没有 Python 解释器时整类跳过（不假绿）。
 */
class McpStdioClientTest {

    private val json = kotlinx.serialization.json.Json { ignoreUnknownKeys = true }

    private fun pythonExe(): Path? {
        val candidates = listOfNotNull(
            System.getenv("COMFYHUB_MCP_PYTHON")?.takeIf { it.isNotBlank() },
            System.getenv("MCP_TEST_PYTHON")?.takeIf { it.isNotBlank() },
            // 本机装了项目自己的 MCP 运行时时，直接用它 —— 那是最接近真实的一条路
            System.getProperty("user.dir")?.let { Paths.get(it).parent?.resolve("storage/ai/mcp/python/python.exe")?.toString() },
        )
        candidates.forEach { c ->
            val p = runCatching { Paths.get(c) }.getOrNull()
            if (p != null && Files.isRegularFile(p)) return p
        }
        // PATH 上的 python（测试机一般都有）
        val fromPath = System.getenv("PATH").orEmpty().split(java.io.File.pathSeparator)
            .map { Paths.get(it, "python.exe") }
            .firstOrNull { Files.isRegularFile(it) }
        return fromPath
    }

    private fun scriptPath(): Path {
        val url = javaClass.getResource("/mcp/fake_mcp_server.py")
        assertNotNull(url, "找不到 src/test/resources/mcp/fake_mcp_server.py")
        return Paths.get(url.toURI())
    }

    private fun client(noisy: Boolean = false, timeoutMs: Long = 30_000): McpStdioClient {
        val py = pythonExe()
        org.junit.jupiter.api.Assumptions.assumeTrue(py != null, "本机没有 Python，跳过 MCP 客户端测试")
        val script = scriptPath()
        // 直接把假服务器脚本当参数传给 python（`python <script> [--noisy]`）。
        // **不走 .cmd 替身**：Windows 上 ProcessBuilder 不能直接执行 .cmd（那要先过 cmd.exe），
        // 用 extraArgs 这一条能测到全部协议行为，形状也更接近生产（python + 一个入口）。
        val args = listOf(script.toString()) + if (noisy) listOf("--noisy") else emptyList()
        return McpStdioClient(
            pythonExe = py!!,
            scriptsDir = null,
            comfyCliExe = null,
            workingDir = script.parent,
            defaultTimeoutMs = timeoutMs,
            handshakeTimeoutMs = timeoutMs,
            extraArgs = args,
        )
    }

    // -----------------------------------------------------------------------

    @Test
    fun `握手后能拉到工具清单，且描述与 schema 原样保留`() {
        client().use { c ->
            val tools = c.tools()
            assertEquals(3, tools.size, "假服务器给了 3 个工具")
            val stats = tools.first { it.name == "system_stats" }
            assertTrue(stats.description.contains("VRAM"), "描述要原样带过来")
            assertEquals("object", stats.inputSchema["type"]?.let { (it as kotlinx.serialization.json.JsonPrimitive).content })
            assertNotNull(c.instructions(), "initialize 里的 instructions 要留下来")
        }
    }

    @Test
    fun `调用工具会把参数原样透传并解出文本`() {
        client().use { c ->
            val out = c.callTool(
                "run_workflow",
                buildJsonObject {
                    put("workflow_path", "D:\\wf\\a.json")
                    put("wait", false)
                },
            )
            assertTrue(out.text.startsWith("echo:"), "服务端回显：${out.text}")
            assertTrue(out.text.contains("a.json"), "参数要原样过去")
            assertTrue(out.text.contains("\"wait\": false") || out.text.contains("\"wait\":false"))
            assertEquals(false, out.isError)
        }
    }

    @Test
    fun `服务端标了 isError 的结果要能被识别出来（不能当成成功）`() {
        client().use { c ->
            val out = c.callTool("install_node", buildJsonObject { put("names", "[]") })
            assertTrue(out.isError, "isError=true 必须被认出来")
            assertTrue(out.text.contains("refused"))
        }
    }

    @Test
    fun `stdout 里混进非 JSON 噪音行也不崩（真实服务器会漏日志）`() {
        client(noisy = true).use { c ->
            val tools = c.tools()
            assertEquals(3, tools.size, "噪音行应被跳过，工具照样拉得到")
        }
    }

    @Test
    fun `进程被关掉之后调用要报错，而不是静默成功`() {
        val py = pythonExe()
        org.junit.jupiter.api.Assumptions.assumeTrue(py != null, "本机没有 Python，跳过")
        val c = client()
        c.use {
            it.tools()
            it.close()
            val e = assertFailsWith<McpException> { it.callTool("system_stats", buildJsonObject { }) }
            assertEquals("MCP_CLOSED", e.code)
        }
    }

    @Test
    fun `Python 路径不存在时要给稳定错误码，不是抛原始异常`() {
        val missing = Files.createTempDirectory("no-python").resolve("python.exe")
        val c = McpStdioClient(missing, null, null, null, defaultTimeoutMs = 3_000, handshakeTimeoutMs = 3_000)
        c.use {
            val e = assertFailsWith<McpException> { it.tools() }
            assertTrue(e.code == "MCP_NO_PYTHON" || e.code == "MCP_SPAWN_FAILED", "实际：${e.code}")
        }
    }
}
