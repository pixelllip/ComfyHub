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

        post("/import") {
            val body = call.receive<ImportFolderRequest>()
            if (body.dir.isBlank()) return@post call.respondBadRequest("dir 不能为空")
            call.respond(capture.importFolder(body))
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

    get("/media/{id}/workflow") {
        val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
        if (MediaRepo.get(id) == null) return@get call.respondNotFound("产物 $id 不存在")
        val json = MediaRepo.workflowJson(id) ?: return@get call.respond(HttpStatusCode.NoContent)
        call.respondText(json, ContentType.Application.Json)
    }
}
