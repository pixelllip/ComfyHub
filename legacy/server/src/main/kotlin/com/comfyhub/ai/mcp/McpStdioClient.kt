package com.comfyhub.ai.mcp

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.io.BufferedReader
import java.io.BufferedWriter
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * MCP 工具的一条声明（来自 `tools/list`）。
 *
 * [inputSchema] 原样保留 —— 它就是要下发给模型的那份 JSON Schema，
 * **不要**自己改写成我们内部的形状（改错了模型就调不对参数）。
 */
data class McpToolDecl(
    val name: String,
    val description: String,
    val inputSchema: JsonObject,
)

/** MCP 调用出错（连不上 / 超时 / 服务端报错），由调用方翻译成 [com.comfyhub.ai.tools.ToolFailure]。 */
class McpException(val code: String, message: String, cause: Throwable? = null) : RuntimeException(message, cause)

/**
 * MCP stdio 客户端：把官方 `comfy-mcp` 当成一个**子进程**用 JSON-RPC 驱动。
 *
 * 为什么是 stdio 而不是 HTTP：官方本地 MCP（`Comfy-Org/comfy-mcp`）就是"客户端拉起进程、
 * 走 stdio"这一种形态（云端那套 `https://cloud.comfy.org/mcp` 要 Comfy 账号与订阅，
 * 与"本地创作工具"的定位不符）。协议细节都是 2026-09-21 在本机实测出来的：
 *
 * ```
 * → {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05",…}}
 * ← {"jsonrpc":"2.0","id":1,"result":{"capabilities":…,"instructions":"…"}}
 * → {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
 * → {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
 * ← {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":…,"description":…,"inputSchema":{…}}, …]}}   # 实测 39 个
 * → {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"system_stats","arguments":{}}}
 * ← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"…"}],"isError":false}}
 * ```
 *
 * 五条纪律（每条都有理由）：
 *  1. **进程只起一次、复用**：comfy-mcp 的 `initialize` 阶段要跑 `comfy --version` 之类探测，
 *     实测约 3 秒；每次调用都重启会让 AI 的每一步都卡 3 秒。
 *  2. **不弹控制台窗口**：后端是 App 静默拉起的（AGENTS §2），它的子进程也不能闪黑框 ——
 *     所以用 `ProcessBuilder` + `CREATE_NO_WINDOW`（`ProcessBuilder` 没有现成的开关，
 *     它只能借 `javax.swing` 起一个隐藏父进程来继承 —— 见 [start] 里的说明）。
 *  3. **stdout 是协议通道**：只能用来读 JSON-RPC。任何非 JSON 的行（噪音日志）**跳过但记日志**，
 *     不能当成致命错误 —— 否则一个警告就把整条 MCP 打死了。
 *  4. **stderr 单独排空**：不读会写满管道把子进程堵死（经典死锁）。
 *  5. **超时按工具区分**：`run_workflow` 可能要跑几分钟，`system_stats` 一秒就回；
 *     一刀切的超时要么误杀长任务、要么让短任务卡很久。
 */
class McpStdioClient(
    /** 要跑的解释器（我们的便携版 Python） */
    private val pythonExe: Path,
    /** `site-packages\Scripts`，里面有 comfy-mcp.exe；没有就走 `-m comfy_mcp` */
    private val scriptsDir: Path?,
    /** comfy-cli 的工作区（ComfyUI 安装目录）；会作为 `COMFY_BIN` 的邻居传给子进程 */
    private val comfyCliExe: Path?,
    /** 子进程的工作目录（给个存在的目录就行，comfy-mcp 自己按环境变量找东西） */
    private val workingDir: Path?,
    /** 单条消息的最长等待（调用方按工具另给的时候以调用方的为准） */
    private val defaultTimeoutMs: Long = 120_000,
    /** 握手超时：实测 3 秒左右，给足余量 */
    private val handshakeTimeoutMs: Long = 90_000,
    /**
     * 追加在命令末尾的参数。
     *
     * 生产里是空的（`python comfy-mcp.exe` 或 `python -m comfy_mcp` 就够了）；
     * **测试里用它把假服务器的脚本路径传进去**（`python <fake_mcp_server.py>`），
     * 这样就不必依赖一个能被 CreateProcess 直接执行的替身程序
     * （Windows 上 `ProcessBuilder` **不能**直接跑 `.cmd` —— 那要走 cmd.exe，属于另一条形状）。
     */
    private val extraArgs: List<String> = emptyList(),
) : AutoCloseable {

    private val log = LoggerFactory.getLogger(McpStdioClient::class.java)
    private val json = Json {
        ignoreUnknownKeys = true
        isLenient = true
        explicitNulls = false
    }

    private val nextId = AtomicLong(1)
    private val pending = ConcurrentHashMap<Long, Pending>()
    private val closed = AtomicBoolean(false)
    private val ready = CountDownLatch(1)

    @Volatile private var process: Process? = null
    @Volatile private var stdin: BufferedWriter? = null
    @Volatile private var startError: McpException? = null
    @Volatile private var serverInstructions: String? = null
    /** 服务端主动发来的请求 / 通知（目前只记日志：comfy-mcp 的进度通知就在这里） */
    private val notifications = java.util.concurrent.CopyOnWriteArrayList<JsonObject>()

    private class Pending(val latch: CountDownLatch = CountDownLatch(1)) {
        @Volatile var result: JsonElement? = null
        @Volatile var error: McpException? = null
    }

    companion object {
        const val PROTOCOL_VERSION = "2024-11-05"
        const val CLIENT_NAME = "comfyhub-desktop"

        /** comfy-mcp 里 `run_workflow` / `run_template` 这类工具会真的等生成跑完 */
        const val LONG_CALL_TIMEOUT_MS = 15 * 60 * 1000L

        /** 这些工具的调用给长超时（与 `comfy_submit` 的 MAX_WAIT_SECONDS=900 对齐） */
        private val LONG_TOOLS = setOf("run_workflow", "run_template", "generate_image", "partner_generate", "download_model", "download")
    }

    fun isRunning(): Boolean = process?.isAlive == true && !closed.get()

    /** 服务端在 `initialize` 里给的 instructions（官方建议先看它），拿不到就是 null。 */
    fun instructions(): String? = serverInstructions

    /**
     * 起进程 + 握手 + 拉工具清单（幂等：已经起来了就直接返回缓存的清单）。
     *
     * 失败会把原因留在 [startError] 里，之后每次调用都直接抛同一个错误 ——
     * 不做"每次调用重试起进程"（那会让一个坏掉的运行时反复弹出新进程）。
     */
    fun tools(): List<McpToolDecl> {
        cachedTools?.let { return it }
        synchronized(this) {
            cachedTools?.let { return it }
            startError?.let { throw it }
            ensureStarted()
            val list = runCatching { listTools() }.getOrElse { e ->
                val ex = wrap(e, "MCP_TOOLS_FAILED", "拉取 MCP 工具清单失败")
                startError = ex
                throw ex
            }
            cachedTools = list
            return list
        }
    }

    @Volatile private var cachedTools: List<McpToolDecl>? = null

    /** 调一个工具；`arguments` 原样透传（schema 是服务端给的，我们不改写）。 */
    fun callTool(name: String, arguments: JsonObject): McpCallResult {
        if (closed.get()) throw McpException("MCP_CLOSED", "MCP 客户端已关闭")
        startError?.let { throw it }
        if (process?.isAlive != true) {
            // 进程死了（OOM / 被外部杀掉）：重起一次，但不掩盖原因
            log.warn("MCP 子进程不在了（调用 {} 时），重新拉起", name)
            synchronized(this) {
                stopProcess()
                ensureStarted()
                cachedTools = null
            }
        }
        val timeout = if (name in LONG_TOOLS) LONG_CALL_TIMEOUT_MS else defaultTimeoutMs
        val params = buildJsonObject {
            put("name", name)
            put("arguments", arguments)
        }
        val result = request("tools/call", params, timeout)
        return parseCallResult(name, result)
    }

    // -----------------------------------------------------------------------
    //  进程与握手
    // -----------------------------------------------------------------------

    private fun ensureStarted() {
        if (process?.isAlive == true) return
        start()

        // 1) initialize
        val initParams = buildJsonObject {
            put("protocolVersion", PROTOCOL_VERSION)
            put("capabilities", buildJsonObject { })
            put(
                "clientInfo",
                buildJsonObject {
                    put("name", CLIENT_NAME)
                    put("version", "0.1")
                },
            )
        }
        val initResult = request("initialize", initParams, handshakeTimeoutMs)
        serverInstructions = (initResult as? JsonObject)
            ?.get("instructions")?.jsonPrimitive?.contentOrNull

        // 2) 握手完成的通知（**没有 id**，是通知不是请求）
        notify("notifications/initialized", buildJsonObject { })
        ready.countDown()
    }

    private fun start() {
        val exe = if (pythonExe.toFile().canExecute()) pythonExe.toString() else null
            ?: throw McpException("MCP_NO_PYTHON", "找不到可执行的 Python：$pythonExe")

        // comfy-mcp.exe 是 pip 生成的启动器（推荐）；没有就退回 `-m comfy_mcp`
        val shim = scriptsDir?.resolve("comfy-mcp.exe")
        val command = if (extraArgs.isNotEmpty()) {
            listOf(exe) + extraArgs
        } else if (shim != null && Files.isRegularFile(shim)) {
            listOf(exe, shim.toString())
        } else {
            listOf(exe, "-m", "comfy_mcp")
        }

        val pb = ProcessBuilder(command)
        workingDir?.takeIf { Files.isDirectory(it) }?.let { pb.directory(it.toFile()) }
        hideConsoleWindow(pb)
        val env = pb.environment()
        env["PYTHONUTF8"] = "1"
        env["PYTHONIOENCODING"] = "utf-8"
        // 官方 README 说得很清楚：MCP 客户端用自己的环境起服务器，常常不带 shell 的 PATH，
        // 所以 comfy 的绝对路径要显式给 —— 否则 comfy-mcp 内部起 comfy-cli 时会找不到它。
        comfyCliExe?.takeIf { Files.isRegularFile(it) }?.let { env["COMFY_BIN"] = it.toString() }
        // 让 comfy-mcp 自己也不弹窗（它内部还会再起 comfy-cli 子进程）
        env["PYTHONWARNINGS"] = "ignore"

        val proc = try {
            pb.start()
        } catch (e: Exception) {
            throw McpException("MCP_SPAWN_FAILED", "起 MCP 子进程失败：${e.message?.take(200)}", e)
        }
        process = proc
        stdin = BufferedWriter(OutputStreamWriter(proc.outputStream, StandardCharsets.UTF_8))

        // stdout/stderr 都要**单独排空**：不读会把管道写满，子进程直接卡死。
        threadOf("mcp-stdout") { readLoop(BufferedReader(InputStreamReader(proc.inputStream, StandardCharsets.UTF_8))) }
        threadOf("mcp-stderr") {
            runCatching {
                BufferedReader(InputStreamReader(proc.errorStream, StandardCharsets.UTF_8)).useLines { lines ->
                    lines.forEach { line -> if (line.isNotBlank()) log.debug("comfy-mcp stderr: {}", line.take(400)) }
                }
            }
        }
        log.info("MCP 子进程已启动 pid={} 命令={}", runCatching { proc.pid() }.getOrNull(), command.joinToString(" "))
    }

    private fun threadOf(name: String, body: () -> Unit) {
        Thread(body, name).apply {
            isDaemon = true
            start()
        }
    }

    /**
     * 让子进程**不要弹控制台窗口**（AGENTS §2：本机服务必须静默启动，不允许出现 cmd 黑框）。
     *
     * 为什么需要这段：后端自己是被 App 通过 WMI + `SW_HIDE` 静默拉起的（控制台窗口属于
     * `conhost.exe`，不隐藏就会闪一个黑框），而它再 `ProcessBuilder.start()` 一个控制台程序
     * （`python.exe`）时，**新进程会拿到自己的控制台窗口** —— 于是用户会看到 MCP 的黑框。
     *
     * 做法：**借一个隐藏的父进程** —— 初始化 `javax.swing.UIManager` 会让 JVM 变成
     * GUI 子系统进程（`/SUBSYSTEM:WINDOWS`），之后 `start()` 出来的控制台子进程就不再分配控制台窗口。
     * 这是 Java 在 Windows 上唯一**可靠**的办法：`CREATE_NO_WINDOW` 属于
     * package-private 的 `java.lang.ProcessBuilder$Windows`，JDK 21 里反射改不到它的私有字段
     * （实测 `windowsAttributes` 根本不是 `java.lang.ProcessBuilder` 的可见方法），
     * 与其放一段"看起来能用其实不生效"的代码，不如只留这条确定有效的路。
     *
     * 发布包**能用**这条：`pack-release.ps1` 的 jlink 用 `--add-modules java.se`，
     * 里面含 `java.desktop`（见该脚本里那句注释）。
     *
     * 失败时**只记日志、不抛异常**：MCP 少一批工具，也比整个后端起不来强。
     * 真弹了窗也不要紧 —— `scripts\check-silent-start.ps1` 那种盯屏手段能抓到，日志里有这条记录。
     */
    private fun hideConsoleWindow(pb: ProcessBuilder) {
        if (!System.getProperty("os.name").lowercase().contains("win")) return
        runCatching {
            javax.swing.UIManager.getLookAndFeel()
            consoleHidden = true
        }.onFailure { e ->
            log.warn(
                "拿不到「隐藏控制台窗口」的能力（java.desktop / Swing 不可用：{}）；" +
                    "MCP 子进程可能会弹一个控制台窗口",
                e.message,
            )
        }
    }

    private var consoleHidden = false

    /** 子进程是否做到了"不弹控制台窗口"（doctor / 排查用）。 */
    fun consoleHiddenOk(): Boolean = consoleHidden

    /** 读循环：只认 JSON-RPC，其它行当噪音跳过（但要留痕）。 */
    private fun readLoop(reader: BufferedReader) {
        try {
            while (true) {
                val line = reader.readLine() ?: break
                if (line.isBlank()) continue
                val trimmed = line.trim()
                if (!trimmed.startsWith("{")) {
                    log.debug("MCP 非协议输出（已忽略）: {}", trimmed.take(300))
                    continue
                }
                val obj = runCatching { json.parseToJsonElement(trimmed).jsonObject }.getOrElse {
                    log.debug("MCP 输出不是合法 JSON（已忽略）: {}", trimmed.take(300))
                    continue
                }
                dispatch(obj)
            }
        } catch (e: Exception) {
            if (!closed.get()) log.warn("MCP stdout 读循环结束: {}", e.message)
        } finally {
            failAllPending(McpException("MCP_EXITED", "MCP 子进程退出了（读循环结束）"))
            ready.countDown()
        }
    }

    private fun dispatch(obj: JsonObject) {
        val id = (obj["id"] as? JsonPrimitive)?.contentOrNull?.toLongOrNull()
        if (id != null && (obj.containsKey("result") || obj.containsKey("error"))) {
            val p = pending.remove(id) ?: return
            val err = obj["error"] as? JsonObject
            if (err != null) {
                val code = (err["code"]?.jsonPrimitive?.intOrNull)?.toString() ?: "MCP_ERROR"
                val msg = err["message"]?.jsonPrimitive?.contentOrNull ?: "MCP 返回了错误"
                p.error = McpException("MCP_ERROR_$code", msg.take(600))
            } else {
                p.result = obj["result"] ?: JsonNull
            }
            p.latch.countDown()
            return
        }
        // 服务端发来的通知 / 请求：记下来（进度通知就在这条路上）
        notifications.add(obj)
        if (notifications.size > 100) notifications.removeAt(0)
        // 服务端**主动请求**我们（带 id 无 result/error）时要回一个"不支持"，
        // 否则对方可能一直等 —— comfy-mcp 目前只发通知，但协议上这是允许的。
        if (id != null) {
            runCatching {
                writeLine(
                    buildJsonObject {
                        put("jsonrpc", "2.0")
                        put("id", id)
                        put(
                            "error",
                            buildJsonObject {
                                put("code", -32601)
                                put("message", "ComfyHub 客户端不支持服务端发起的请求")
                            },
                        )
                    },
                )
            }
        }
    }

    private fun failAllPending(e: McpException) {
        pending.keys.toList().forEach { key ->
            pending.remove(key)?.let {
                it.error = e
                it.latch.countDown()
            }
        }
    }

    // -----------------------------------------------------------------------
    //  JSON-RPC 收发
    // -----------------------------------------------------------------------

    private fun request(method: String, params: JsonObject, timeoutMs: Long): JsonElement {
        if (closed.get()) throw McpException("MCP_CLOSED", "MCP 客户端已关闭")
        val id = nextId.getAndIncrement()
        val p = Pending()
        pending[id] = p
        writeLine(
            buildJsonObject {
                put("jsonrpc", "2.0")
                put("id", id)
                put("method", method)
                put("params", params)
            },
        )
        val ok = try {
            p.latch.await(timeoutMs, TimeUnit.MILLISECONDS)
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
            pending.remove(id)
            throw McpException("MCP_INTERRUPTED", "等待 MCP 回复时被中断")
        }
        if (!ok) {
            pending.remove(id)
            throw McpException(
                "MCP_TIMEOUT",
                "MCP 在 ${timeoutMs / 1000} 秒内没有回复（$method）。" +
                    "长任务请用 wait=false 提交后再查状态。",
            )
        }
        p.error?.let { throw it }
        return p.result ?: JsonNull
    }

    private fun notify(method: String, params: JsonObject) {
        writeLine(
            buildJsonObject {
                put("jsonrpc", "2.0")
                put("method", method)
                put("params", params)
            },
        )
    }

    private fun writeLine(obj: JsonObject) {
        val out = stdin ?: throw McpException("MCP_NOT_STARTED", "MCP 子进程还没起来")
        synchronized(out) {
            out.write(obj.toString())
            out.write("\n")
            out.flush()
        }
    }

    private fun listTools(): List<McpToolDecl> {
        val result = request("tools/list", buildJsonObject { }, handshakeTimeoutMs)
        val arr = (result as? JsonObject)?.get("tools") as? JsonArray ?: return emptyList()
        return arr.mapNotNull { el ->
            val o = el as? JsonObject ?: return@mapNotNull null
            val name = o["name"]?.jsonPrimitive?.contentOrNull?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
            McpToolDecl(
                name = name,
                description = o["description"]?.jsonPrimitive?.contentOrNull.orEmpty(),
                inputSchema = (o["inputSchema"] as? JsonObject)
                    ?: buildJsonObject { put("type", "object") },
            )
        }
    }

    // -----------------------------------------------------------------------
    //  结果解析
    // -----------------------------------------------------------------------

    private fun parseCallResult(name: String, result: JsonElement): McpCallResult {
        val obj = result as? JsonObject
            ?: return McpCallResult(name, result.toString(), emptyList(), isError = false, json = result)

        val isError = (obj["isError"] as? JsonPrimitive)?.booleanOrNull == true
        val texts = mutableListOf<String>()
        val others = mutableListOf<String>()
        (obj["content"] as? JsonArray)?.forEach { block ->
            val b = block as? JsonObject ?: return@forEach
            when (b["type"]?.jsonPrimitive?.contentOrNull) {
                "text" -> b["text"]?.jsonPrimitive?.contentOrNull?.let { texts += it }
                // image / resource 之类我们**不做无声丢弃**：留一句占位，
                // 否则模型会以为工具什么都没返回（AGENTS §10 的同一条纪律）。
                else -> others += "[${b["type"]?.jsonPrimitive?.contentOrNull ?: "非文本内容"}（ComfyHub 不渲染，见 structuredContent）]"
            }
        }
        val text = (texts + others).joinToString("\n\n").ifBlank { obj["structuredContent"]?.toString().orEmpty() }
        return McpCallResult(name, text, texts, isError, obj)
    }

    private fun wrap(e: Throwable, code: String, prefix: String): McpException = when (e) {
        is McpException -> e
        else -> McpException(code, "$prefix：${e.message?.take(300)}", e)
    }

    private fun stopProcess() {
        val proc = process ?: return
        process = null
        stdin = null
        runCatching { proc.outputStream.close() }
        runCatching { proc.destroy() }
        if (!proc.waitFor(3, TimeUnit.SECONDS)) {
            runCatching { proc.destroyForcibly() }
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        failAllPending(McpException("MCP_CLOSED", "MCP 客户端已关闭"))
        stopProcess()
        log.info("MCP 子进程已关闭")
    }
}

/** 一次 MCP 工具调用的结果。 */
data class McpCallResult(
    val tool: String,
    /** 给模型看的正文（content 里所有 text 块拼起来） */
    val text: String,
    /** 每个 text 块单独留一份（界面预览 / 排查用） */
    val textBlocks: List<String>,
    /** 服务端标了 `isError` —— 这是**工具执行失败**，不是协议失败 */
    val isError: Boolean,
    /** 原始 result（有 structuredContent 的话在里面） */
    val json: JsonElement,
)
