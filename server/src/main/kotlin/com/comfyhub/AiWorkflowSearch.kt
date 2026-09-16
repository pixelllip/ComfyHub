package com.comfyhub

import com.comfyhub.ai.tools.ComfySubmitOutcome
import com.comfyhub.ai.tools.ToolFailure
import com.comfyhub.ai.tools.WorkflowSearch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * AI 工具「提交 ComfyUI 任务」的接线层（用户建议 ①）。
 *
 * 工具层（`comfyhub.ai.tools`）不直接依赖 `PromptRepo` / `ComfySubmitter` ——
 * 它只认 `Application.kt` 传进来的两个 lambda。这一层就是那两个 lambda 的实现，
 * 放在主包是为了让工具层保持"只通过注入的入口访问世界"这条约定。
 *
 * 只做三件事：
 *  1. 搜库里的工作流（能给模型看的参数摘要 + 可选 API 节点图）；
 *  2. 把参数覆盖写进节点图（[ComfySubmitter.WorkflowEdit]）；
 *  3. 交给 [ComfySubmitter] 真跑，并把结果整理成模型 / 界面都能用的形状。
 */
object AiWorkflowSearch {

    /**
     * 按关键词找库里能跑的工作流。
     *
     * 为什么不直接返回界面上那份 `workflow_json`：那是**界面格式**，`/prompt` 不接受。
     * 提交要用的是 `capture_runs.raw` 里那份 API 节点图（见 [ComfyCapture.apiGraphOf]），
     * 所以这里把 `hasApiGraph` 如实标出来 —— 没有的老记录，模型就该告诉用户"这条跑不了"，
     * 而不是硬编一个。
     */
    fun search(capture: ComfyCapture, query: String, limit: Int, includeGraph: Boolean): WorkflowSearch {
        val page = PromptRepo.search(
            PromptRepo.SearchArgs(q = query, sort = "newest", page = 1, size = limit),
        )
        val items = page.items
        val json = buildJsonObject {
            put("count", items.size)
            put("query", query)
            put(
                "workflows",
                buildJsonArray {
                    items.forEach { p ->
                        val graph = if (includeGraph) capture.apiGraphOf(p.id) else null
                        add(
                            buildJsonObject {
                                put("promptId", p.id)
                                put("title", p.title)
                                put("kind", p.kind)
                                put("positivePrompt", p.positivePrompt.take(400))
                                put("negativePrompt", p.negativePrompt?.take(200))
                                put("checkpoint", p.checkpoint)
                                put("sampler", p.sampler)
                                put("scheduler", p.scheduler)
                                put("steps", p.steps)
                                put("cfgScale", p.cfgScale)
                                put("seed", p.seed)
                                put("width", p.width)
                                put("height", p.height)
                                put("source", p.source)
                                put("hasMedia", p.mediaCount > 0)
                                // 能不能直接提交：只有拿得到 API 节点图才行
                                put("runnable", graph != null)
                                if (graph != null && includeGraph) put("apiGraph", graph)
                                if (graph == null) {
                                    put(
                                        "note",
                                        "这条没有 API 节点图（老数据 / 只导入了图片），不能直接提交；" +
                                            "可以在 ComfyUI 里跑一次让它被捕获，或者换一条 runnable=true 的",
                                    )
                                }
                            }
                        )
                    }
                },
            )
        }
        return WorkflowSearch(items.size, json)
    }

    /**
     * 提交一条库里的工作流给 ComfyUI 跑。
     *
     * 步骤：取 API 图 → 覆盖参数（类型不符当场报错）→ 提交并等待 → 返回产物 id。
     * 提示词正文变了的话，**同时**把 `positivePrompt` 一并更新到工作流里是不可能的
     * （节点图上的 `text` 输入才是真源），所以覆盖由模型的 `overrides` 明确给出 ——
     * 工具不替它猜哪个节点是提示词。
     */
    suspend fun submit(
        submitter: ComfySubmitter,
        capture: ComfyCapture,
        promptId: Long,
        overrides: JsonObject?,
        title: String?,
        waitSeconds: Int,
    ): ComfySubmitOutcome {
        val prompt = PromptRepo.get(promptId)
            ?: throw ToolFailure("NOT_FOUND", "库里没有 id=$promptId 的提示词（先用 comfy_find_workflow 找）")
        val graph = capture.apiGraphOf(promptId)
            ?: throw ToolFailure(
                "NO_API_GRAPH",
                "这条提示词（${prompt.title}）没有 API 格式节点图，不能直接提交。" +
                    "可以换一条 runnable=true 的，或者先在 ComfyUI 里手动跑一次让它被捕获。",
            )

        val (patched, applied) = try {
            ComfySubmitter.WorkflowEdit.applyOverrides(graph, overrides.orEmpty())
        } catch (e: IllegalArgumentException) {
            throw ToolFailure("INVALID_ARGUMENT", e.message ?: "参数覆盖不合法")
        }

        val label = title?.takeIf { it.isNotBlank() } ?: prompt.title
        val submission = submitter.submit(patched, label, waitSeconds = waitSeconds)

        val json = buildJsonObject {
            put("promptId", promptId)
            put("comfyPromptId", submission.promptId)
            put("title", label)
            put("status", submission.status)
            put("elapsedMs", submission.elapsedMs)
            put("message", submission.message)
            put("error", submission.error)
            put("capturedPromptId", submission.capturedPromptId)
            put(
                "appliedOverrides",
                buildJsonArray { applied.forEach { add(kotlinx.serialization.json.JsonPrimitive(it)) } },
            )
            put("mediaIds", buildJsonArray { submission.mediaIds.forEach { add(kotlinx.serialization.json.JsonPrimitive(it)) } })
            put("mediaCount", submission.mediaIds.size)
            if (submission.status == "timeout") {
                put(
                    "hint",
                    "还没跑完：产物会在跑完后自动入库，让用户稍后刷新画廊即可；" +
                        "也可以隔一会儿用 comfy_get_run 查 comfyPromptId=${submission.promptId}",
                )
            }
        }
        return ComfySubmitOutcome(
            promptId = submission.promptId,
            status = submission.status,
            title = label,
            mediaIds = submission.mediaIds,
            capturedPromptId = submission.capturedPromptId,
            json = json,
        )
    }
}
