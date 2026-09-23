package com.comfyhub

import com.comfyhub.ai.AiAttachmentStore
import com.comfyhub.ai.AiRunRepo
import com.comfyhub.ai.AiSeeder
import com.comfyhub.ai.CredentialService
import com.comfyhub.ai.HarnessRunner
import com.comfyhub.ai.RunEventBus
import com.comfyhub.ai.aiRoutes
import com.comfyhub.ai.mcp.McpRuntimeLocator
import com.comfyhub.ai.mcp.McpStdioClient
import com.comfyhub.ai.mcp.McpToolBridge
import com.comfyhub.ai.mcp.McpToolPolicy
import com.comfyhub.ai.tools.KeenableSearch
import com.comfyhub.ai.tools.CivitaiSearch
import com.comfyhub.ai.tools.MemoryStore
import com.comfyhub.ai.tools.SkillStore
import com.comfyhub.ai.tools.ToolAccess
import com.comfyhub.ai.tools.ToolApprovalGate
import com.comfyhub.ai.tools.ToolRegistry
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpMethod
import io.ktor.http.HttpStatusCode
import io.ktor.serialization.kotlinx.json.json
import io.ktor.server.application.Application
import io.ktor.server.application.ApplicationStopped
import io.ktor.server.application.install
import io.ktor.server.engine.embeddedServer
import io.ktor.server.netty.Netty
import kotlinx.serialization.json.JsonObject
import io.ktor.server.plugins.calllogging.CallLogging
import io.ktor.server.plugins.compression.Compression
import io.ktor.server.plugins.contentnegotiation.ContentNegotiation
import io.ktor.server.plugins.cors.routing.CORS
import io.ktor.server.plugins.defaultheaders.DefaultHeaders
import io.ktor.server.plugins.partialcontent.PartialContent
import io.ktor.server.plugins.statuspages.StatusPages
import io.ktor.server.request.httpMethod
import io.ktor.server.request.path
import io.ktor.server.response.respond
import io.ktor.server.routing.get
import io.ktor.server.routing.route
import io.ktor.server.routing.routing
import kotlinx.serialization.SerializationException
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.slf4j.LoggerFactory
import org.slf4j.event.Level
import java.sql.SQLException
import java.time.Instant

/** 全局上下文，注入到各路由 */
class AppContext(val cfg: AppConfig, val storage: Storage)

const val APP_VERSION = "1.0.0"

fun main() {
    val log = LoggerFactory.getLogger("com.comfyhub.Main")
    val cfg = AppConfig.fromEnv()

    cfg.ensureStorage()

    // 数据库是后端的硬依赖。这里带重试地等它，而不是一连不上就直接退出 ——
    // 这样即便 MySQL 比后端晚几秒就绪（脚本并行拉起时很常见）也能自愈。
    if (!waitForDatabase(cfg)) {
        Db.close()
        kotlin.system.exitProcess(1)
    }

    val storage = Storage(cfg.storageDir)

    // 库结构自动补齐（新增列 / 新表），保证 App 自动拉起后端时不需要人工跑 SQL
    runCatching { Migrate.run() }.onFailure {
        log.error("数据库结构迁移失败（后端继续启动，但 ComfyUI 自动捕获可能不可用）: {}", it.message)
    }

    log.info("ComfyHub 服务启动中...\n{}", cfg.describe())

    // 端口不再写死（用户 2026-09-22）：候选端口绑不上就**换成随机端口**再来一次，
    // 绑定成功后把**真实端口**记进 `<项目根>\.run\api.json` —— 前端 / 脚本 / ComfyUI
    // 捕获扩展都读这一个文件，端口只有一个真源。详见 [ApiPortFile]。
    val actual = startServer(cfg, log)
    val portState = ApiPortState(
        port = actual.port,
        pid = ProcessHandle.current().pid(),
        host = cfg.host,
        url = ApiPortFile.urlFor(cfg.host, actual.port),
        startedAt = java.time.Instant.now().toString(),
        requestedPort = cfg.port,
        fellBack = actual.fellBack,
    )
    ApiPortFile.write(cfg.projectRoot, portState)

    // 优雅退出时清掉记录（只有还属于自己那一份才清，见 [ApiPortFile.deleteIfOwned]）
    Runtime.getRuntime().addShutdownHook(
        Thread { ApiPortFile.deleteIfOwned(cfg.projectRoot, portState.pid) },
    )

    // ⚠️ 引擎已经在 [startServer] 里 `start(wait = false)` 过了（为了回读真实端口）。
    // 这里**绝不能**再 `start(wait = true)`：ktor 的 EmbeddedServer.start() 没有"已启动"保护，
    // 每调一次都会 createApplication() 把 module 整个重跑一遍（comfy-mcp 子进程也会再起一个），
    // 然后让 Netty 重绑同一端口 —— 旧连接器还没释放，赌赢照跑、赌输 BindException
    // 直接把进程带走（2026-09-22 实测两次：启动后 ~9 秒死，栈都指向这一行）。
    // 主线程只需要停住等停机；优雅退出由 ktor 在 start() 时挂的 shutdown hook 负责
    //（上面我们自己那个 ApiPortFile 清理 hook 也在）。
    Thread.currentThread().join()
}

/**
 * 启动 HTTP 服务，返回**真实绑定到的端口**。
 *
 * 两条纪律：
 *
 *  1. **候选端口绑不上不算致命**：`port = 0` 时操作系统会挑一个空闲端口，
 *     所以"8080 被占 / 落在 Windows 保留段里"这类问题从"起不来"变成"换一个"。
 *     用户的原话是"失败了回到随机端口"。
 *  2. **必须回读真实端口**：`cfg.port == 0` 时我们事先不知道会绑到哪个，
 *     只能从引擎的 `resolvedConnectors()` 拿 —— 拿不到就明确报错，
 *     **绝不**写一个猜出来的端口（那会让前端连到一个不存在的服务，且查不出原因）。
 */
private fun startServer(
    cfg: AppConfig,
    log: org.slf4j.Logger,
): StartedServer {
    // 模块体每次都要新建：一个 Application 实例不能挂到两个引擎上
    fun newEngine(port: Int) = embeddedServer(Netty, port = port, host = cfg.host) {
        module(AppContext(cfg, storage = Storage(cfg.storageDir)))
    }

    fun attempt(port: Int): StartAttempt {
        val engine = newEngine(port)
        // ⚠️ Netty 的绑定失败发生在**它自己的事件循环线程**里：`engine.start(wait = false)`
        // 那一句不会抛异常，异常被记在引擎内部那个 `resolvedConnectors` 的 deferred 上。
        // 所以**必须**去 await 它：成功时给出真实端口，失败时把 BindException 原样抛回来。
        // 不 await 的话"回落随机端口"就是假的 —— 进程活着、日志刷 bind 失败、却没有任何人在听
        // （2026-09-22 实测踩到：单独重启时候选端口还被上一个进程占着，回落逻辑完全没生效）。
        return try {
            engine.start(wait = false)
            // 超时必须**明显短于**脚本侧等健康的窗口（server.ps1 的 Wait-Api 是 90 秒）：
            // 这里 60 秒的话，"绑定其实成功了、只是引擎没报"这种极端情况会一直占着窗口，
            // 让脚本等到超时才开始回落（2026-09-22 实测到 90+ 秒的启动）。
            // 25 秒足够冷启动（实测 Netty 绑定在 5 秒内完成），失败也来得及回落。
            val port0 = runBlocking {
                withTimeout(25_000) { engine.engine.resolvedConnectors().firstOrNull()?.port }
            } ?: throw IllegalStateException("服务已启动但拿不到监听端口（这是本机的 bug，请附启动日志反馈）")
            StartAttempt(StartedServer(engine, port0, fellBack = false), null)
        } catch (e: TimeoutCancellationException) {
            runCatching { engine.stop(0, 0) }
            StartAttempt(null, IllegalStateException("启动 25 秒内既没绑上也没报错（引擎内部卡住）", e))
        } catch (e: Exception) {
            runCatching { engine.stop(0, 0) }
            StartAttempt(null, e)
        }
    }

    fun isBindFailure(t: Throwable?): Boolean =
        t != null && generateSequence(t) { it.cause }.any { it is java.net.BindException }

    val first = attempt(cfg.port)
    if (first.server != null) {
        val s = first.server
        log.info(
            "后端已监听 {}{}",
            ApiPortFile.urlFor(cfg.host, s.port),
            if (s.port != cfg.port) "（请求的端口是 ${cfg.port}）" else "",
        )
        return s
    }

    val failure = first.failure
    if (!isBindFailure(failure)) {
        // 不是"端口被占"（比如配置错、Netty 起不来）：**不要**用随机端口把这颗雷盖住
        throw IllegalStateException("HTTP 服务启动失败：${failure?.message}", failure)
    }

    log.warn(
        "候选端口 {} 绑不上（{}）—— 改用随机端口重试（用户要求：失败了回到随机端口）。" +
            "想固定端口请用 -Port / COMFYHUB_PORT 指定一个确实空闲的端口。",
        cfg.port,
        failure!!.message,
    )
    val retry = attempt(0)
    val s = retry.server
        ?: throw IllegalStateException("改用随机端口后仍然起不来：${retry.failure?.message}", retry.failure)
    log.warn("已改用随机端口 {}（原候选 {}）", s.port, cfg.port)
    return StartedServer(s.engine, s.port, fellBack = true)
}

/** 一次启动尝试：[server] 与 [failure] 恰好有一个非空。 */
private class StartAttempt(val server: StartedServer?, val failure: Throwable?)

/** [startServer] 的结果：引擎 + 真实端口 + 有没有回落。 */
private class StartedServer(
    val engine: io.ktor.server.engine.EmbeddedServer<*, *>,
    val port: Int,
    val fellBack: Boolean,
)

/**
 * 等待数据库可用。返回 false 表示彻底失败（调用方应退出进程）。
 *
 * @param attempts 尝试次数
 * @param delayMs  每次之间的间隔
 */
private fun waitForDatabase(cfg: AppConfig, attempts: Int = 20, delayMs: Long = 1500): Boolean {
    val log = LoggerFactory.getLogger("com.comfyhub.Main")
    var lastError: Exception? = null

    repeat(attempts) { i ->
        val attempt = i + 1
        try {
            if (!Db.isInitialized) Db.init(cfg)
            // 真正打一次查询，确认库和表都在
            Db.withConnection { conn ->
                conn.queryOne("SELECT COUNT(*) FROM prompts") { it.getLong(1) }
            }
            if (attempt > 1) log.info("数据库已就绪（第 {} 次尝试）", attempt)
            return true
        } catch (e: Exception) {
            lastError = e
            if (attempt == 1) {
                log.warn("数据库暂时不可用，开始重试（最多 {} 次，每次间隔 {}ms）：{}", attempts, delayMs, e.message)
            } else {
                log.warn("第 {}/{} 次连接数据库失败: {}", attempt, attempts, e.message)
            }
            Db.close()
            Thread.sleep(delayMs)
        }
    }

    log.error(
        """
        无法访问数据库，已重试 {} 次。
        请先执行:  pwsh -File scripts\\comfyhub.ps1 up
        原因: {}
        """.trimIndent(),
        attempts, lastError?.message
    )
    return false
}

fun Application.module(ctx: AppContext) {
    val log = LoggerFactory.getLogger("com.comfyhub.Module")

    // ComfyUI 自动捕获：轮询 /history，把「参数 + 工作流 + 产物」整条收进库
    val capture = ComfyCapture(ctx.cfg, ctx.storage)
    capture.start()
    monitor.subscribe(ApplicationStopped) { capture.stop() }

    // AI 凭据：值只存在 DPAPI 加密文件里，DB / 日志 / 接口都拿不到明文（AIH-012 / AIH-015）
    val credentials = CredentialService(ctx.cfg.storageDir.resolve("ai"))

    // Skills（M5）：**磁盘是正文真源**，AI 注册 / 用户删除都立刻生效，不需要重启应用。
    // 用户投放口 = `<storage>\ai\skills`：把 skill 文件夹（或 .md）拷进去就算装好，
    // 启动时自动补 frontmatter 登记（见 SkillStore.autoRegister）。
    val skills = SkillStore(
        builtinRoot = ctx.cfg.projectRoot.resolve("skills").resolve("builtin"),
        userRoot = ctx.cfg.storageDir.resolve("ai").resolve("skills"),
    )
    runCatching { skills.autoRegister() }
        .onSuccess {
            if (it.registered > 0) log.info("投放口自动登记了 {} 个 skill：{}", it.registered, it.names.joinToString("、"))
            it.errors.forEach { msg -> log.warn("投放口有没法自动登记的条目：{}", msg) }
        }
        .onFailure { log.warn("扫描 skills 投放口失败（不影响启动）：{}", it.message) }
    log.info("skills 投放口: {}", skills.roots().userRoot)

    // 长期记忆（M6）：一个人类可读的 memory.md，注入系统提示 + AI 可用 remember 追加
    val memory = MemoryStore(ctx.cfg.storageDir.resolve("ai"))

    // 能力目录覆盖层（2026-09-23"全做了"）：custom/remote.json 在 <storage>\ai\capabilities，
    // 启动时读一次让 lookup 立刻生效；后台 daemon 每小时看一眼 remote.json 过没过期
    // （>7 天才真出网拉 models.dev，COMFYHUB_NO_DOWNLOAD=1 一刀关）。
    val capabilitiesDir = ctx.cfg.storageDir.resolve("ai").resolve("capabilities")
    runCatching { com.comfyhub.ai.ModelCapabilityCatalog.reload(capabilitiesDir) }
        .onFailure { log.warn("能力覆盖层读不了（只用内置表）：{}", it.message) }
    com.comfyhub.ai.CapabilityRefresh.startBackground(capabilitiesDir)
    monitor.subscribe(ApplicationStopped) { com.comfyhub.ai.CapabilityRefresh.stopBackground() }

    // AI 附件（M3）：原件与缩略图都落在 storage 下（与画廊产物分开），
    // 视频预览帧复用画廊那套 Windows 缩略图管线（不引入 ffmpeg）。
    val aiAttachments = AiAttachmentStore(ctx.storage, ctx.cfg.projectRoot)

    // 工具层（M4）：出厂只能写 <根>\comfyui，只读 <根>\comfyui + <根>\storage（见 ToolPolicy）
    val approvals = ToolApprovalGate()
    // 提问闸门（用户建议 ⑥）：AI 可以当场问一句、**不中断**这一次回答
    val askGate = com.comfyhub.ai.tools.AskGate()
    val asks = com.comfyhub.ai.tools.AskRegistry(askGate)
    // 提交任务（用户建议 ①）：AI 可以把库里的工作流真的送进 ComfyUI 队列
    val submitter = ComfySubmitter(ctx.cfg, capture)

    // Civitai 客户端：`civitai_search` 与 `find_model`（P3）**共用同一个实例** ——
    // 域名切换 / 令牌 / 错误映射都在它里面，两处各造一个只会漂移。
    val civitaiSearch = CivitaiSearch(credentials)

    // 官方 comfy-mcp（用户建议 ④，2026-09-21 拍板接入）：把它的 39 个工具当成
    // **第二来源**接进工具表 —— 工作流执行、节点/模型查询、显存这些交给它，
    // 我们的 comfy_* 保留"产物必须入库"的那条链路（同名时我们的胜出，见 ToolRegistry.tools）。
    //
    // 三条边界：
    //  1. 运行时不在（没装配 / 没 Python）→ 一个 MCP 工具都不加，**不影响启动**（可选能力）；
    //  2. `tools/list` 要真的起一次子进程（约 3 秒），所以**放在启动时做一次**并缓存 ——
    //     不能等到用户发第一条消息才做（那会把首字延迟拖到 3 秒以上）；
    //  3. 起不来只记日志，App 照常能用。
    val mcpClient = runCatching { McpRuntimeLocator.discover(ctx.cfg.projectRoot) }
        .getOrNull()
        ?.takeIf { it.available }
        ?.let { paths ->
            McpStdioClient(
                pythonExe = paths.pythonExe!!,
                scriptsDir = paths.scriptsDir,
                comfyCliExe = paths.comfyExe,
                workingDir = paths.dir,
            )
        }
    val mcpTools: List<com.comfyhub.ai.tools.AgentTool> = if (mcpClient != null) {
        runCatching {
            val decls = mcpClient.tools()
            val bridge = com.comfyhub.ai.mcp.McpToolBridge(mcpClient, ToolRegistry.MAX_RESULT_CHARS)
            log.info(
                "官方 comfy-mcp 已接入：{} 个工具（{} 个默认 deny / {} 个默认 ask）",
                decls.size,
                decls.count { com.comfyhub.ai.mcp.McpToolPolicy.accessFor(it.name) == com.comfyhub.ai.tools.ToolAccess.DENY },
                decls.count { com.comfyhub.ai.mcp.McpToolPolicy.accessFor(it.name) == com.comfyhub.ai.tools.ToolAccess.ASK },
            )
            decls.map { bridge.build(it) }
        }.getOrElse { e ->
            log.warn("comfy-mcp 已装上但这次没拉起来（{}）：AI 少一批 ComfyUI 工具，App 照常可用", e.message)
            emptyList()
        }
    } else {
        log.info("comfy-mcp 运行时不可用（没装配 / 没 Python）：跳过 MCP 工具。装它: pwsh -File scripts\\mcp-runtime.ps1 install")
        emptyList()
    }

    // 本机模型清单（用户建议 ①，2026-09-22）：**`/object_info` 与盘扫的并集**。
    // 只有并集才说得清"盘上有、但当前实例看不到"——那次事故里模型据此让用户去下载
    // 一个本来就在硬盘上的 4GB 文件（见 ComfyModels.inventory 的注释）。
    // 抽成一个 val 是为了让 `comfy_submit` 的**提交前预检**用同一份清单（不另算一份、也不另写一份判据）。
    val modelInventory: (JsonObject?) -> com.comfyhub.ComfyModels.Inventory = { objectInfo ->
        com.comfyhub.ComfyModels.inventory(
            objectInfo ?: runCatching { submitter.objectInfo() }.getOrNull(),
            ctx.cfg.projectRoot,
            // 用户**真实的模型根**：ComfyUI Desktop 的共享目录、别的盘上的模型库。
            // 这一项必须传 —— 少了它，盘上事实里就没有 Shared，那句诊断无从谈起。
            com.comfyhub.ComfyEnv.desktop()?.modelsDirs.orEmpty(),
        )
    }

    val toolRegistry = ToolRegistry(
        projectRoot = ctx.cfg.projectRoot,
        skills = skills,
        approvals = approvals,
        // 状态里额外带上"最近提交的任务"：模型查状态时就能看到自己刚提交的那个跑到哪了
        comfyStatus = {
            val base = AppJson.encodeToJsonElement(CaptureStatus.serializer(), capture.status()) as JsonObject
            JsonObject(
                base + mapOf(
                    "submissions" to AppJson.encodeToJsonElement(
                        kotlinx.serialization.builtins.ListSerializer(ComfySubmission.serializer()),
                        submitter.submissions(5),
                    ),
                )
            )
        },
        comfyFindRun = { key ->
            // 两个都认：ComfyUI 的 UUID（runKey）与捕获记录里的数字 prompt_id。
            // 只认 UUID 的话，模型拿 comfy_submit 结果里的数字来查就会"刚提交完却说没这次运行"。
            val info = CaptureRepo.findRun(key)
                ?: key.trim().toLongOrNull()?.let { CaptureRepo.findRunByPromptId(it) }
            info?.let { AppJson.encodeToJsonElement(CaptureRunInfo.serializer(), it) }
        },
        comfySync = { AppJson.encodeToJsonElement(CapturePollResult.serializer(), capture.pollOnce()) },
        comfyFindWorkflow = { query, limit, includeGraph ->
            AiWorkflowSearch.search(capture, query, limit, includeGraph)
        },
        // 用户 bug ⑤：库只收"捕获过的运行"，所以首次使用时库里是空的 —— 本机 ComfyUI 里
        // 已经保存的工作流文件（user\<用户>\workflows\*.json）要能直接被列出来（只读），
        // 并标出哪几份已经在本项目库里（run_key = file:<sha256>，与 comfy_load_workflow 同一把钥匙）。
        comfyListWorkflows = { query, limit ->
            ComfyWorkflowFiles.listJson(ComfyWorkflowFiles.dirs(ctx.cfg), query, limit) { sha ->
                CaptureRepo.findRun("file:$sha")?.promptId
            }
        },
        comfySubmit = { promptId, overrides, connections, title, waitSeconds, nsfw ->
            AiWorkflowSearch.submit(
                submitter,
                capture,
                promptId,
                overrides,
                connections,
                title,
                waitSeconds,
                nsfw,
                // 提交前预检（用户 2026-09-22 拍板）：要的模型当前实例加载不了就**当场拒**，
                // 并给出动作（重启 / 修挂载 / 装它 / 把用不上的分支摘掉）。取清单失败时不拦 ——
                // 误拦一个本来能跑的工作流，比白跑一次更糟。
                //
                // 两道闸门，都是"必跑不了"才拦（用户拍板的档位）：
                //  ① 审核里那三项 —— 纯前端节点 / 版本不兼容节点 / 本机没有的节点类；
                //  ② 模型那一项 —— 盘上有但实例看不到 / 本机确实没有。
                // 参数档位、显存、提示词风格**只提醒不拦**（那是"不推荐"，不是"跑不了"）。
                preflight = { graph ->
                    runCatching {
                        val info = runCatching { submitter.objectInfo() }.getOrNull()
                        val inv = modelInventory(info)
                        com.comfyhub.WorkflowAudit.audit(graph, info, inv).blockingRefusal()
                            ?: com.comfyhub.ModelPreflight.check(graph, inv)
                    }.getOrNull()
                },
            )
        },
        // 用户 bug ③：给一个工作流文件路径就能读进库、拿到 promptId（界面格式在这里被转成 API 图）
        // tolerateUnsupported（用户建议 ②）：转换不了的前端节点摘掉并给出缺口清单，交给模型补线
        // emulateFrontend（用户建议 ②「##模拟用户行为##」）：把纯前端节点照前端排队前的行为
        // 等价改写掉（广播填空 / 丢显示节点 / 补 SaveImage），改了什么如实回给模型
        comfyLoadWorkflow = { path, title, includeGraph, tolerateUnsupported, emulateFrontend ->
            AiWorkflowSearch.loadWorkflowFromFile(
                submitter,
                java.nio.file.Paths.get(path),
                title,
                includeGraph,
                tolerateUnsupported,
                emulateFrontend,
            )
        },
        // 图生图：把用户发来的图投放进 ComfyUI 的 input 目录（用户 bug：
        // "LoadImage 读的是捕获时绑定的那张 jpg，我只能改文本，改不了文件名"）
        comfyUseAttachment = { attachmentId, filename ->
            AiWorkflowSearch.useAttachment(ctx.cfg, aiAttachments, attachmentId, filename)
        },
        // 联网搜索（2026-09-20，用户定的规则：只用通用方案）：本项目**唯一**会出网的工具类别。
        // 免密钥默认可用；可选 key 走 CredentialService（DPAPI），设置页里填。
        search = KeenableSearch(credentials),
        // Civitai 模型库搜索（用户建议 ④/⑦）：安全档免令牌走 civitai.com；
        // NSFW 档走 civitai.red 且必须令牌（设置页里的凭据名 civitai，
        // 或环境变量 CIVITAI_API_KEY）。工具默认 ask —— 用户原话就是"这种情况可以询问用户"。
        civitai = civitaiSearch,
        // 定向查询（用户建议 P3，2026-09-22）：一条 `find_model` 内部按
        // Civitai → HuggingFace → ModelScope 顺序查，每个源都报可达性；
        // **绝不代下** —— 只贴链接 + sha256 + 目标目录（`download_model` 保持 deny）。
        // Civitai 那一半**复用上面同一个客户端**；HF 与 ModelScope 是这里的真实检索实现，
        // 全部走 OutboundProxy（公网走代理、本机/内网强制直连）。
        finder = com.comfyhub.ai.tools.HttpModelFinder(
            civitai = civitaiSearch,
            credentials = credentials,
        ),
        // 提问（用户建议 ⑥）：把问题递到用户面前、在原地等回答，
        // 答完作为工具结果喂回模型，**同一个 Run 接着做**。
        ask = askGate,
        askRegistry = asks,
        // 摘/拼节点（用户建议 ②）：拼装要照着 ComfyUI 的节点定义算，
        // 拿不到就只摘不拼（宁可不做，也不乱接一根线）
        comfyObjectInfo = { runCatching { submitter.objectInfo() }.getOrNull() },
        // 本机模型清单（用户建议 ①）：**`/object_info` 与盘扫的并集**（2026-09-22 改）。
        // 与上面对 `comfy_submit` 预检用的是**同一份**（`modelInventory`）——
        // 两处判据分家就会出现"预检说能用、fit 说没有"这种自相矛盾。
        comfyModels = modelInventory,
        // 官方 comfy-mcp 拉来的工具（运行时不在了就是空列表）
        extraTools = mcpTools,
    )

    // 内置模型目录：项目内置的冻结副本（classpath）里那份，缺哪个补哪个；已存在的一律不覆盖
    val seed = AiSeeder.syncBuiltinProvider()
    if (seed.added > 0) log.info("内置模型目录已登记：新增 {} 个模型（{}）", seed.added, seed.version)
    if (seed.divergent.isNotEmpty()) log.info("有 {} 个模型的能力声明与内置目录不同（保持用户当前设置）", seed.divergent.size)
    if (seed.error != null) log.warn("内置模型目录登记失败（不影响启动）：{}", seed.error)

    // Run 事件总线 + 后台执行器（AIH-020/021，M4 工具循环）
    val runBus = RunEventBus()
    val runner = HarnessRunner(
        credentials = credentials,
        bus = runBus,
        tools = toolRegistry,
        skills = skills,
        approvals = approvals,
        projectRoot = ctx.cfg.projectRoot,
        memory = memory,
        attachments = aiAttachments,
        askRegistry = asks,
    )
    monitor.subscribe(ApplicationStopped) { runner.shutdown() }
    // 上次进程退出时还在 running 的 Run 不可能再继续：标成失败，而不是让界面永远转圈
    runCatching { AiRunRepo.failStaleRunning() }
        .onSuccess { if (it > 0) log.warn("有 {} 个 Run 因后端重启被标记为失败", it) }

    if (!ctx.cfg.loopbackOnly) {
        log.warn(
            "后端监听 {}（非回环）：AI 接口会代替用户调用上游模型并产生费用，请确认局域网可信",
            ctx.cfg.host
        )
    }

    install(DefaultHeaders) {
        header("X-App", "ComfyHub/$APP_VERSION")
    }

    install(CallLogging) {
        level = Level.INFO
        format { call ->
            "${call.request.httpMethod.value} ${call.request.path()} -> ${call.response.status()?.value ?: "-"}"
        }
    }

    install(ContentNegotiation) {
        json(AppJson)
    }

    install(Compression)

    // 视频拖动进度条依赖 Range 支持
    install(PartialContent) {
        maxRangeCount = 20
    }

    // CORS 收紧（AIH-016）：原来是 anyHost()，任何网页都能调用本机后端。
    // 在 AI 接口会拿着用户的 API Key 代替用户请求上游之后，这等于把额度和密钥暴露给
    // 任意一个本机打开的网页，所以改成"只允许本机来源 + 显式配置的白名单"。
    install(CORS) {
        val schemes = listOf("http", "https")
        allowHost("localhost", schemes)
        allowHost("127.0.0.1", schemes)
        allowHost("[::1]", schemes)
        ctx.cfg.corsOrigins.forEach { origin ->
            runCatching {
                val uri = java.net.URI(origin)
                val h = uri.host ?: return@runCatching
                val scheme = uri.scheme ?: "https"
                allowHost(h, listOf(scheme))
            }.onFailure {
                log.warn("忽略无法解析的 CORS Origin 配置: {}", origin)
            }
        }
        allowHeader(HttpHeaders.ContentType)
        allowHeader(HttpHeaders.Authorization)
        allowHeader(HttpHeaders.Range)
        exposeHeader(HttpHeaders.ContentLength)
        exposeHeader(HttpHeaders.ContentRange)
        exposeHeader(HttpHeaders.AcceptRanges)
        allowMethod(HttpMethod.Get)
        allowMethod(HttpMethod.Post)
        allowMethod(HttpMethod.Put)
        allowMethod(HttpMethod.Patch)
        allowMethod(HttpMethod.Delete)
        allowMethod(HttpMethod.Options)
    }

    install(StatusPages) {
        // AI 领域异常统一成**稳定错误码**（AIH-024）：前端拿 `error` 判断、拿 `detail` 给用户看。
        // 不这样做的话它们会落到下面的 Throwable 分支，变成 500 + "AiException"，用户只看到"服务器错误"。
        exception<com.comfyhub.ai.AiException> { call, cause ->
            call.respond(HttpStatusCode.BadRequest, ApiError(cause.code, cause.message))
        }
        exception<IllegalArgumentException> { call, cause ->
            call.respond(HttpStatusCode.BadRequest, ApiError(cause.message ?: "请求参数不合法"))
        }
        exception<SerializationException> { call, cause ->
            call.respond(HttpStatusCode.BadRequest, ApiError("请求体 JSON 解析失败", cause.message))
        }
        exception<SQLException> { call, cause ->
            log.error("数据库错误: {}", cause.message, cause)
            call.respond(HttpStatusCode.InternalServerError, ApiError("数据库错误", cause.message))
        }
        exception<Throwable> { call, cause ->
            log.error("未处理异常: {}", cause.message, cause)
            call.respond(
                HttpStatusCode.InternalServerError,
                ApiError(cause::class.simpleName ?: "UnknownError", cause.message)
            )
        }
        status(HttpStatusCode.NotFound) { call, _ ->
            call.respond(ApiError("接口不存在: ${call.request.path()}"))
        }
    }

    routing {
        get("/") {
            call.respond(
                mapOf(
                    "app" to "ComfyHub",
                    "version" to APP_VERSION,
                    "docs" to "/api/health",
                )
            )
        }

        route("/api") {
            get("/health") {
                val dbState = runCatching {
                    Db.withConnection { conn -> conn.queryOne("SELECT 1") { it.getInt(1) } }
                }.fold({ "ok" }, { "error: ${it.message}" })

                call.respond(
                    HealthDto(
                        status = if (dbState == "ok") "ok" else "degraded",
                        version = APP_VERSION,
                        database = dbState,
                        storageDir = ctx.cfg.storageDir.toString(),
                        serverTime = Instant.now().toString(),
                    )
                )
            }

            get("/stats") {
                call.respond(Db.withConnection { MediaRepo.stats(it) })
            }

            promptRoutes()
            tagRoutes()
            mediaRoutes(ctx)
            captureRoutes(ctx, capture, submitter)
            aiRoutes(credentials, runner, runBus, skills, memory, toolRegistry, approvals, ctx.cfg.projectRoot, aiAttachments, capabilitiesDir, asks)
        }
    }
}
