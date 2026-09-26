package com.comfyhub

import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.server.request.receive
import io.ktor.server.response.respond
import io.ktor.server.response.respondText
import io.ktor.server.routing.Route
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.put
import io.ktor.server.routing.route

/**
 * ComfyUI 联动接口。
 *
 * ```
 *  GET    /api/capture/config        读自动捕获配置
 *  PUT    /api/capture/config        整体覆盖自动捕获配置
 *  GET    /api/capture/status        状态 + 最近捕获的若干次运行
 *  POST   /api/capture/poll          立刻轮询一次 ComfyUI /history
 *  POST   /api/capture/import        导入某个目录里已经生成好的产物（含 PNG 内嵌工作流）
 *  GET    /api/capture/workflows     列出本机 ComfyUI 已保存的工作流文件（只读，用户 bug ⑤）
 *  POST   /api/capture/import-workflows  把这些工作流文件批量读进本项目库
 *  POST   /api/ingest/comfyui        捕获入口（ComfyUI 自定义节点 / 外部脚本推送）
 *  GET    /api/prompts/{id}/workflow 该提示词对应的完整工作流 JSON
 *  GET    /api/media/{id}/workflow   该产物对应的完整工作流 JSON
 * ```
 */
fun Route.captureRoutes(ctx: AppContext, capture: ComfyCapture, submitter: ComfySubmitter) {

    route("/capture") {

        get("/config") {
            call.respond(SettingsRepo.captureConfig(ctx.cfg))
        }

        // 整体覆盖（App 的设置页总是回传完整配置，不做字段级合并，语义更简单）
        put("/config") {
            val body = call.receive<CaptureConfig>()
            call.respond(SettingsRepo.saveCaptureConfig(body))
        }

        get("/status") {
            call.respond(capture.status())
        }

        /**
         * 实时进度（用户"其他建议"第 1 条）：AI 工作台右侧栏轮询它。
         *
         * 与 `/status` 的区别：这里**每次都真的去问一次 ComfyUI 的队列**，
         * 而且带上 AI / 用户最近提交的任务进度 —— 界面要的就是"现在到底在跑什么"。
         */
        get("/jobs") {
            val enabled = SettingsRepo.captureConfig(ctx.cfg).enabled
            capture.refreshQueueNow()
            val reachable = capture.isReachable()
            call.respond(
                CaptureJobSnapshot(
                    queueRunning = capture.queueRunning(),
                    queuePending = capture.queuePending(),
                    comfyReachable = reachable,
                    runningLabel = capture.runningLabel(),
                    submissions = submitter.submissions(10),
                )
            )
        }

        post("/poll") {
            call.respond(capture.pollOnce())
        }

        /**
         * 探测 ComfyUI 装在哪（用户"其他建议"第 3 条）。**只读**：
         * 探测结果不会自动生效，用户点「使用这个目录」才会写进配置。
         */
        get("/locate") {
            call.respond(ComfyLocator.find(ctx.cfg))
        }

        /** 把探测到的输出目录写进配置（用户点了「使用这个目录」）。 */
        post("/locate/apply") {
            val body = call.receive<LocateApplyRequest>()
            val dir = body.outputDir?.trim().orEmpty()
            if (dir.isEmpty()) return@post call.respondBadRequest("outputDir 不能为空")
            if (!java.nio.file.Files.isDirectory(java.nio.file.Paths.get(dir))) {
                return@post call.respondBadRequest("目录不存在：$dir")
            }
            val current = SettingsRepo.captureConfig(ctx.cfg)
            call.respond(SettingsRepo.saveCaptureConfig(current.copy(outputDir = dir)))
        }

        post("/import") {
            val body = call.receive<ImportFolderRequest>()
            if (body.dir.isBlank()) return@post call.respondBadRequest("dir 不能为空")
            call.respond(capture.importFolder(body))
        }

        /**
         * **本机 ComfyUI 里已经保存的工作流文件**（`user\<用户>\workflows\*.json`，用户 bug ⑤）。
         *
         * 只读：只列文件名 / 路径 / 格式 / 是否已入库，不改用户的文件、不写库。
         * 为什么需要它：`prompts` 库只收"捕获过的运行"与"手动读进来的文件"，
         * 所以**首次使用时库里是空的**，而用户机器上早就存着工作流 —— 这个接口把它们摆出来。
         */
        get("/workflows") {
            val query = call.request.queryParameters["q"]
            val limit = call.request.queryParameters["limit"]?.toIntOrNull() ?: 50
            val json = ComfyWorkflowFiles.listJson(ComfyWorkflowFiles.dirs(ctx.cfg), query, limit) { sha ->
                CaptureRepo.findRun("file:$sha")?.promptId
            }
            call.respondText(
                AppJson.encodeToString(kotlinx.serialization.json.JsonObject.serializer(), json),
                ContentType.Application.Json,
            )
        }

        /**
         * 把这些工作流文件**批量读进本项目库**（界面上的「导入工作流文件…」按钮）。
         *
         * 写的是我们自己的库，用户机器上的文件一个字节都不动；幂等靠
         * `run_key = file:<sha256>`（与 `comfy_load_workflow` 同一把钥匙）。
         */
        post("/import-workflows") {
            val body = call.receive<ImportWorkflowsRequest>()
            val dirs = body.dir?.takeIf { it.isNotBlank() }?.let { raw ->
                val p = runCatching { java.nio.file.Paths.get(raw) }.getOrNull()
                    ?: return@post call.respondBadRequest("目录不合法：$raw")
                if (!java.nio.file.Files.isDirectory(p)) return@post call.respondBadRequest("目录不存在：$raw")
                listOf(p)
            } ?: ComfyWorkflowFiles.dirs(ctx.cfg)
            val json = ComfyWorkflowFiles.importAll(
                cfg = ctx.cfg,
                submitter = submitter,
                dirs = dirs,
                limit = body.limit,
                query = body.query,
            )
            call.respondText(
                AppJson.encodeToString(kotlinx.serialization.json.JsonObject.serializer(), json),
                ContentType.Application.Json,
            )
        }
    }

    // --- 推送式捕获（ComfyUI 自定义节点跑完直接打过来） ---
    post("/ingest/comfyui") {
        val body = call.receive<IngestRequest>()
        if (body.runKey.isBlank()) return@post call.respondBadRequest("runKey 不能为空")
        call.respond(capture.ingest(body))
    }

    // --- 工作流原文（前端按需拉取，避免列表接口变重） ---
    get("/prompts/{id}/workflow") {
        val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
        if (Db.withConnection { PromptRepo.get(it, id) } == null) return@get call.respondNotFound("提示词 $id 不存在")
        val json = PromptRepo.workflowJson(id) ?: return@get call.respond(HttpStatusCode.NoContent)
        call.respondText(json, ContentType.Application.Json)
    }

    /**
     * **API 格式**节点图（提交给 ComfyUI `/prompt` 用的那一份）。
     *
     * 与上面的 `/workflow` 不是一回事：那是界面格式（能拖回 ComfyUI 复现），
     * 这是"当时真正跑的东西"。`comfy_submit` 与端到端自测都用它；
     * 老数据没有就 204（调用方如实说明"这条跑不了"，不猜着转换）。
     */
    get("/prompts/{id}/api-graph") {
        val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
        if (Db.withConnection { PromptRepo.get(it, id) } == null) return@get call.respondNotFound("提示词 $id 不存在")
        val graph = capture.apiGraphOf(id) ?: return@get call.respond(HttpStatusCode.NoContent)
        call.respondText(
            AppJson.encodeToString(kotlinx.serialization.json.JsonObject.serializer(), graph),
            ContentType.Application.Json,
        )
    }

    get("/media/{id}/workflow") {
        val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
        if (MediaRepo.get(id) == null) return@get call.respondNotFound("产物 $id 不存在")
        val json = MediaRepo.workflowJson(id) ?: return@get call.respond(HttpStatusCode.NoContent)
        call.respondText(json, ContentType.Application.Json)
    }
}
