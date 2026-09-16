package com.comfyhub

import com.comfyhub.ai.AiRunRepo
import com.comfyhub.ai.AiSeeder
import com.comfyhub.ai.CredentialService
import com.comfyhub.ai.HarnessRunner
import com.comfyhub.ai.RunEventBus
import com.comfyhub.ai.aiRoutes
import com.comfyhub.ai.tools.MemoryStore
import com.comfyhub.ai.tools.SkillStore
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

    embeddedServer(Netty, port = cfg.port, host = cfg.host) {
        module(AppContext(cfg, storage))
    }.start(wait = true)
}

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

    // 工具层（M4）：出厂只能写 <根>\comfyui，只读 <根>\comfyui + <根>\storage（见 ToolPolicy）
    val approvals = ToolApprovalGate()
    val toolRegistry = ToolRegistry(
        projectRoot = ctx.cfg.projectRoot,
        skills = skills,
        approvals = approvals,
        comfyStatus = { AppJson.encodeToJsonElement(CaptureStatus.serializer(), capture.status()) },
        comfyFindRun = { runKey ->
            CaptureRepo.findRun(runKey)?.let { AppJson.encodeToJsonElement(CaptureRunInfo.serializer(), it) }
        },
        comfySync = { AppJson.encodeToJsonElement(CapturePollResult.serializer(), capture.pollOnce()) },
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
            captureRoutes(ctx, capture)
            aiRoutes(credentials, runner, runBus, skills, memory, toolRegistry, approvals, ctx.cfg.projectRoot)
        }
    }
}
