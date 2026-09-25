package com.comfyhub

import com.comfyhub.ai.tools.ToolPolicy
import io.ktor.http.HttpStatusCode
import io.ktor.server.request.receive
import io.ktor.server.response.respond
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.route
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject

/**
 * 工作流配方接口（W2，2026-09-26）。
 *
 * ```
 *  GET    /api/ai/workflow-recipes           列表（?q= &task= &skeleton= &complete=true &limit=）
 *  POST   /api/ai/workflow-recipes           学一份工作流（path / promptId / graph 三选一）
 *  GET    /api/ai/workflow-recipes/{id}      详情（含配方正文与 render 文本）
 *  DELETE /api/ai/workflow-recipes/{id}      删除
 * ```
 *
 * 路径挂在 `/api/ai/` 下、错误体是 `{error, detail}`（AI 域口径）：读写它的两方都是 AI 域的东西
 * —— 界面（`AiApiClient`，先读 `detail`）与 AI 工具（`comfy_learn_workflow`）。
 *
 * **`POST` 就是 `comfy_learn_workflow` 的那条实现**（[WorkflowRecipeLearner]），
 * 两者共用一份代码：分头写必然漂移，而漂移的表现是"AI 学出来的配方和界面看到的不一样"（不报错）。
 *
 * ⚠ 文件路径走 [ToolPolicy.resolveRead]（与 `comfy_load_workflow` 同一条准入）：
 * 界面 / AI 都不能借这个端点读白名单以外的文件。
 */
@Serializable
data class WorkflowRecipeLearnInput(
    /** 本机工作流文件路径（界面格式也行 —— 会用 ComfyUI 的 `/object_info` 转） */
    val path: String? = null,
    /** 或者：库里已有 API 图的提示词 id */
    val promptId: Long? = null,
    /** 或者：直接给一张 API 节点图 */
    val graph: JsonObject? = null,
    val name: String? = null,
    /** 转换有缺口时是否照样学（默认 **false = 拒学**，见 `WorkflowRecipeLearner` 文件头那条纪律） */
    val tolerateUnsupported: Boolean = false,
    /** 界面格式转换时是否照 ComfyUI 前端行为等价改写（默认 true，与 `comfy_load_workflow` 一致） */
    val emulateFrontend: Boolean = true,
    val notes: String? = null,
)

/** 详情响应：配方本身 + 给模型看的那段正文（一个渲染实现，两处共用）。 */
@Serializable
data class WorkflowRecipeDetailDto(
    val recipe: WorkflowRecipeRepo.WorkflowRecipeDto,
    val render: String,
)

/**
 * 读路径准入：走工具层同一条白名单（本机 ComfyUI 的目录自动在里面），
 * 并把工具层的 `ToolFailure` 翻成 AI 域的 `AiException`（400 + 稳定码，而不是 500 + 类名）。
 */
private fun resolveReadPath(ctx: AppContext, raw: String): java.nio.file.Path = try {
    ToolPolicy.load(ctx.cfg.projectRoot).resolveRead(raw)
} catch (e: com.comfyhub.ai.tools.ToolFailure) {
    throw com.comfyhub.ai.AiException(e.code, e.message ?: "这个路径不允许读")
}

fun Route.workflowRecipeRoutes(ctx: AppContext, deps: WorkflowRecipeLearner.Deps) {
    route("/ai/workflow-recipes") {

        get {
            val p = call.request.queryParameters
            call.respond(
                WorkflowRecipeRepo.list(
                    q = p.str("q"),
                    task = p.str("task"),
                    skeletonHash = p.str("skeleton"),
                    onlyComplete = p.bool("complete") == true,
                    limit = p.int("limit", WorkflowRecipeRepo.MAX_LIST),
                )
            )
        }

        post {
            val input = call.receive<WorkflowRecipeLearnInput>()
            // 与 comfy_load_workflow 同一条准入：只读白名单（本机 ComfyUI 的目录已在里面）。
            // ⚠ `resolveRead` 抛的是工具层的 `ToolFailure` —— 不转成 AiException 的话
            // 它会落到 StatusPages 的通用分支，变成 **500 + error:"ToolFailure"**
            // （人话还在 detail 里，但状态码说成"服务器错误"会把人往错方向带）。
            val args = WorkflowRecipeLearner.LearnArgs(
                path = input.path?.let { resolveReadPath(ctx, it).toString() },
                promptId = input.promptId,
                graph = input.graph,
                name = input.name,
                tolerateUnsupported = input.tolerateUnsupported,
                emulateFrontend = input.emulateFrontend,
                notes = input.notes,
            )
            val learned = WorkflowRecipeLearner.learn(args, deps)
            val (id, _) = WorkflowRecipeLearner.learnAndSave(learned, input.notes)
            call.respond(HttpStatusCode.Created, learned.toJson(id))
        }

        route("/{id}") {
            get {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val row = WorkflowRecipeRepo.get(id)
                    ?: return@get call.respondNotFound("配方 $id 不存在")
                // 详情里带上 render 正文：与 comfy_learn_workflow 当场回的那份**跑的是同一个函数**
                call.respond(WorkflowRecipeDetailDto(recipe = row, render = row.renderText()))
            }

            delete {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val ok = Db.tx { conn -> WorkflowRecipeRepo.delete(conn, id) }
                if (!ok) return@delete call.respondNotFound("配方 $id 不存在")
                call.respond(mapOf("deleted" to id))
            }
        }
    }
}
