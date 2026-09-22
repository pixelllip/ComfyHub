package com.comfyhub.ai.mcp

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * MCP 运行时（Python + comfy-cli + comfy-mcp）的状态。
 *
 * 真源是 `scripts\mcp-runtime.ps1` 装出来的那份东西（脚本"检测优先，安装只作兜底"：
 * 先用机器上已有的 Python，都没有才在包内下便携版）。位置见下：
 *
 * ```
 * <根>\storage\ai\mcp\venv\Scripts\python.exe        借机器上的 Python 建的隔离环境（正常走这条）
 * <根>\storage\ai\mcp\python\python.exe              便携版 Python（兜底那份）
 * <根>\storage\ai\mcp\python\Scripts\comfy-mcp.exe   pip 装的启动器
 * <根>\storage\ai\mcp\python\Scripts\comfy.exe       comfy-cli
 * <根>\storage\ai\mcp\state.json                     装配记录（版本 / 工作区 / 这份 Python 哪来的）
 * ```
 *
 * ⚠ **`comfy-mcp.exe` 不一定在 `<python 的父目录>\Scripts` 下**：venv 的解释器本身就在
 * `Scripts\` 里（再套一层就成了 `venv\Scripts\Scripts`）。所以统一走 [scriptsDirOf]。
 *
 * 后端**不自己装配**（那是脚本的活，AGENTS §5：脚本是唯一入口），只做两件事：
 *  1. 找到它、读出状态（[discover]）；
 *  2. 把 `comfy-mcp` 当子进程驱动（[McpStdioClient]）。
 */
@Serializable
data class McpRuntimeState(
    val schema: String? = null,
    val installedAt: String? = null,
    val pythonExe: String? = null,
    val pythonVersion: String? = null,
    /** 这份 Python 是哪来的：`system` / `venv` / `portable`（老记录里没有这个字段 → null）。 */
    val pythonSource: String? = null,
    val comfyMcpExe: String? = null,
    val comfyMcpVersion: String? = null,
    val comfyExe: String? = null,
    val comfyCliVersion: String? = null,
    val workspace: String? = null,
    val workspaceOk: Boolean = false,
)

/**
 * pip 的 console script（`comfy-mcp.exe`）在哪个目录。
 *
 * 两种布局都要认（与 `scripts\mcp-runtime.ps1` 的 `Get-PythonScriptsDir` 是同一套判据）：
 *  · 便携版 / 系统 Python —— `<python 的父目录>\Scripts`
 *  · venv               —— `<python 的父目录>` 本身就是 `Scripts`
 * 判错的表现是"运行时明明装好了，却报缺 comfy-mcp.exe"。
 */
internal fun scriptsDirOf(pythonExe: Path): Path? {
    val parent = pythonExe.parent ?: return null
    return if (parent.fileName?.toString()?.equals("Scripts", ignoreCase = true) == true) parent
    else parent.resolve("Scripts")
}

/** 发现结果：路径 + 状态 + 一句人话（缺什么）。 */
data class McpRuntimePaths(
    val dir: Path,
    val pythonExe: Path?,
    val comfyMcpExe: Path?,
    val comfyExe: Path?,
    val scriptsDir: Path?,
    val state: McpRuntimeState?,
    val problems: List<String>,
) {
    val available: Boolean get() = pythonExe != null && problems.isEmpty()
    val workspace: String? get() = state?.workspace
}

/**
 * 找 MCP 运行时。
 *
 * 顺序（与 `scripts\mcp-runtime.ps1` 的 `Get-McpRuntimeDir` 一致）：
 *  1. 环境变量 `COMFYHUB_MCP_DIR`（`server.ps1` 会显式给，指向脚本算出来的那个目录）
 *  2. `<根>\storage\ai\mcp` —— 发布包与源码树同构
 *  3. `%LOCALAPPDATA%\ComfyHub\mcp` —— 根目录不可写时脚本退到那里
 *
 * **刻意不读 `%USERPROFILE%\.dsh`**（AGENTS §10：任何运行期代码都不许依赖 DSH 的目录）。
 */
object McpRuntimeLocator {
    private val log = LoggerFactory.getLogger(McpRuntimeLocator::class.java)
    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    fun discover(projectRoot: Path): McpRuntimePaths {
        val env = System.getenv("COMFYHUB_MCP_DIR")?.takeIf { it.isNotBlank() }
        val candidates = buildList {
            env?.let { add(runCatching { Paths.get(it) }.getOrNull()) }
            add(projectRoot.resolve("storage").resolve("ai").resolve("mcp"))
            System.getenv("LOCALAPPDATA")?.takeIf { it.isNotBlank() }
                ?.let { add(runCatching { Paths.get(it, "ComfyHub", "mcp") }.getOrNull()) }
        }.filterNotNull()

        // env 指的那个优先（哪怕它还不存在也先认它，报"没装配"比报"找不到"更有用）
        val dir = candidates.firstOrNull { Files.isDirectory(it) }
            ?: candidates.firstOrNull()
            ?: projectRoot.resolve("storage").resolve("ai").resolve("mcp")

        val state = readState(dir)
        val python = sequenceOf(
            state?.pythonExe?.takeIf { it.isNotBlank() }?.let { runCatching { Paths.get(it) }.getOrNull() },
            // 顺序与脚本的 Get-PythonRuntimePath 一致：先 venv（正常路径），再便携版（兜底）
            dir.resolve("venv").resolve("Scripts").resolve("python.exe"),
            dir.resolve("python").resolve("python.exe"),
        ).filterNotNull().firstOrNull { Files.isRegularFile(it) }

        val scriptsDir = python?.let { scriptsDirOf(it) }
        val comfyMcp = scriptsDir?.resolve("comfy-mcp.exe")?.takeIf { Files.isRegularFile(it) }
        val comfyExe = scriptsDir?.resolve("comfy.exe")?.takeIf { Files.isRegularFile(it) }

        val problems = mutableListOf<String>()
        if (python == null) problems += "没有可用的 Python（机器上没有，包内也没装配过：跑 `pwsh -File scripts\\mcp-runtime.ps1 install`）"
        if (python != null && comfyMcp == null) problems += "缺 comfy-mcp.exe（pip 没装成？重跑 mcp-runtime.ps1 install）"
        if (python != null && comfyExe == null) problems += "缺 comfy.exe（comfy-cli 没装成？重跑 mcp-runtime.ps1 install）"
        if (state?.workspace.isNullOrBlank()) problems += "comfy-cli 还没设工作区（它不知道用户的 ComfyUI 在哪）"

        val paths = McpRuntimePaths(dir, python, comfyMcp, comfyExe, scriptsDir, state, problems)
        if (!paths.available) log.info("MCP 运行时不可用：{}（目录 {}）", problems.joinToString("；"), dir)
        return paths
    }

    private fun readState(dir: Path): McpRuntimeState? {
        val file = dir.resolve("state.json")
        if (!Files.isRegularFile(file)) return null
        return runCatching {
            val text = Files.readString(file, StandardCharsets.UTF_8)
            json.decodeFromString(McpRuntimeState.serializer(), text)
        }.onFailure { log.debug("读 MCP 装配记录失败 {}: {}", file, it.message) }.getOrNull()
    }

    /** 给界面 / doctor 用的一句话状态（后端不生成界面文案，这里只给事实）。 */
    fun summarize(projectRoot: Path): JsonObject {
        val p = discover(projectRoot)
        return buildJsonObject {
            put("available", p.available)
            put("dir", p.dir.toString())
            put("python", p.pythonExe?.toString())
            put("pythonVersion", p.state?.pythonVersion)
            // 这份 Python 是哪来的（system / venv / portable）—— 界面上能说清"我们没乱装东西"
            p.state?.pythonSource?.let { put("pythonSource", it) }
            put("comfyMcp", p.comfyMcpExe?.toString())
            put("comfyMcpVersion", p.state?.comfyMcpVersion)
            put("comfyCli", p.comfyExe?.toString())
            put("comfyCliVersion", p.state?.comfyCliVersion)
            put("workspace", p.state?.workspace)
            put("problems", JsonArray(p.problems.map { JsonPrimitive(it) }))
        }
    }
}

/** `state.json` 里没记工作区时，从 comfy-cli 自己的 config.ini 兜底读一眼（**只读，不写**）。 */
internal fun readComfyCliWorkspace(): String? {
    val local = System.getenv("LOCALAPPDATA") ?: return null
    return runCatching {
        val ini = Paths.get(local, "comfy-cli", "config.ini")
        if (!Files.isRegularFile(ini)) return null
        Files.readAllLines(ini, StandardCharsets.UTF_8)
            .firstOrNull { it.trim().startsWith("default_workspace") }
            ?.substringAfter('=')
            ?.trim()
            ?.takeIf { it.isNotBlank() && it != "None" }
    }.getOrNull()
}
