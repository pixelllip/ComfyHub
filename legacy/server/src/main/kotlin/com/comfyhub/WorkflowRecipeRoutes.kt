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
 * 工作流配方接口（W2/W3，2026-09-26）。
 *
 * ```
 *  GET    /api/ai/workflow-recipes           列表（?q= &task= &skeleton= &complete=true &limit=）
 *  POST   /api/ai/workflow-recipes           学一份工作流（path / promptId / graph 三选一）
 *  GET    /api/ai/workflow-recipes/{id}      详情（含配方正文与 render 文本）
 *  POST   /api/ai/workflow-recipes/{id}/bind 把配方绑到一个（新）模型上（**只报不改**）
 *  DELETE /api/ai/workflow-recipes/{id}      删除
 * ```
 *
 * 路径挂在 `/api/ai/` 下、错误体是 `{error, detail}`（AI 域口径）：读写它的两方都是 AI 域的东西
 * —— 界面（`AiApiClient`，先读 `detail`）与 AI 工具（`comfy_learn_workflow` / `comfy_bind_recipe`）。
 *
 * **学习与绑定都走传进来的那两个 lambda**（`Application` 里建的那一份），
 * 与 AI 工具**是同一个实例**：分头写必然漂移，而漂移的表现是"AI 学出来的配方和界面看到的不一样"（不报错）。
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

/** 绑定的请求体（与 `comfy_bind_recipe` 工具的参数一一对应）。 */
@Serializable
data class WorkflowRecipeBindInput(
    /** 要绑上去的模型文件名 */
    val model: String,
    /** 可选：绑到哪一类槽位（不给就按清单里该文件所在的目录推） */
    val role: String? = null,
    /** 可选：直接点名槽位的覆盖键（最准） */
    val overrideKey: String? = null,
    /** 默认 false：没验证过的名字不写进覆盖键 */
    val force: Boolean = false,
    /** 可选：还要一起绑的其它槽位，键是 role */
    val extra: Map<String, String> = emptyMap(),
)

/** 绑定响应：报告 JSON + 给模型/界面看的那段正文。 */
@Serializable
data class WorkflowRecipeBindDto(
    val recipeId: Long,
    val report: kotlinx.serialization.json.JsonObject,
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

fun Route.workflowRecipeRoutes(
    ctx: AppContext,
    learnRecipe: (WorkflowRecipeLearner.LearnArgs) -> Pair<Long, WorkflowRecipeLearner.Learned>,
    bindRecipe: (Long, List<WorkflowRecipeBinder.Request>, Boolean) -> WorkflowRecipeBinder.Report?,
) {
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
            // 学 + 落库都在这个 lambda 里（与 comfy_learn_workflow 同一个实例）
            val (id, learned) = learnRecipe(args)
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

            // 绑定：**只报不改**（与 comfy_bind_recipe 同一个实现）。
            // 界面上"这条骨架能不能换这个新模型"就是问这个端点。
            post("/bind") {
                val id = call.requireId() ?: return@post call.respondBadRequest("非法的 id")
                val input = call.receive<WorkflowRecipeBindInput>()
                val requests = buildList {
                    add(
                        WorkflowRecipeBinder.Request(
                            fileName = input.model,
                            role = input.role,
                            overrideKey = input.overrideKey,
                        ),
                    )
                    input.extra.forEach { (role, name) -> add(WorkflowRecipeBinder.Request(fileName = name, role = role)) }
                }
                val report = bindRecipe(id, requests, input.force)
                    ?: return@post call.respondNotFound("配方 $id 不存在")
                call.respond(WorkflowRecipeBindDto(recipeId = id, report = report.toJson(), render = report.render()))
            }
        }
    }
}
