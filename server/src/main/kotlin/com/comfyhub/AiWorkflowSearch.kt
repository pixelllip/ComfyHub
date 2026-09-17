package com.comfyhub

import com.comfyhub.ai.AiAttachmentRepo
import com.comfyhub.ai.AiAttachmentStore
import com.comfyhub.ai.Modality
import com.comfyhub.ai.tools.ComfySubmitOutcome
import com.comfyhub.ai.tools.ToolFailure
import com.comfyhub.ai.tools.WorkflowSearch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.nio.file.StandardCopyOption

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

    /**
     * 把用户发来的**图片附件**投放进 ComfyUI 的 `input/` 目录（用户 bug：
     * "工作流的 LoadImage 读的是它自己被捕获时绑定的那张 jpg，我只能改文本，改不了这个文件名"）。
     *
     * 这正是那个死结的唯一解法：LoadImage 的 `image` 输入只能是 ComfyUI **input 目录里真实存在的文件名**，
     * 所以图生图 / 参考图 / 结构参考必须先"把文件放进去"，再拿返回的文件名去覆盖节点输入。
     *
     * 三条纪律：
     *  1. **文件名由我们算**：`<原文件名>-<附件id前8位>.<扩展名>` —— 确定、可重复（同一个附件重复调用
     *     得到同一个名字，重跑工作流不会指向一个消失的文件），又不会因为两个用户都叫 `image.png`
     *     而互相覆盖；
     *  2. **只投图片**：视频 / 音频 / 文本附件放进去 ComfyUI 也读不了，直接如实报错；
     *  3. **找不到 ComfyUI 就说找不到**：不猜路径、不建目录到随机位置。
     */
    fun useAttachment(
        cfg: AppConfig,
        attachments: AiAttachmentStore,
        attachmentId: String,
        wantedName: String?,
    ): JsonObject {
        val dto = AiAttachmentRepo.get(attachmentId)
            ?: throw ToolFailure("ATTACHMENT_NOT_FOUND", "没有 id=$attachmentId 的附件（附件 id 见用户消息里的附件说明）")
        val modality = dto.toFact().modality
        if (modality != Modality.IMAGE) {
            throw ToolFailure(
                "NOT_AN_IMAGE",
                "附件「${dto.name}」是 ${modality?.wire ?: "未知类型"}，不是图片。" +
                    "只有图片能放进 ComfyUI 的 input 目录当作图生图 / 参考图。",
            )
        }
        val source = attachments.fileOf(attachmentId)
            ?: throw ToolFailure("ATTACHMENT_NOT_FOUND", "附件「${dto.name}」的原件已经不在了（可能被清理过）")
        if (!Files.isRegularFile(source)) {
            // 库里有行、盘上没文件：如实说，别走到拷贝那一步再报一个看不懂的 IO 错
            throw ToolFailure("ATTACHMENT_NOT_FOUND", "附件「${dto.name}」的原件文件不在了：$source")
        }

        val inputDir = resolveComfyInputDir(cfg)
            ?: throw ToolFailure(
                "COMFY_NOT_FOUND",
                "没找到 ComfyUI 的目录，没法把图片放进它的 input 目录。" +
                    "让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置后再试。",
            )

        val filename = inputFilenameFor(dto.name, attachmentId, wantedName)
        val target = inputDir.resolve(filename)
        try {
            Files.createDirectories(inputDir)
            // 同一个附件重复调用时目标已存在（内容相同）：直接复用，省一次几 MB 的拷贝
            val same = Files.isRegularFile(target) && Files.size(target) == Files.size(source)
            if (!same) Files.copy(source, target, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: Exception) {
            throw ToolFailure("COMFY_INPUT_FAILED", "写不进 ComfyUI 的 input 目录（${inputDir}）：${e.message?.take(200)}")
        }

        return buildJsonObject {
            put("attachmentId", attachmentId)
            put("filename", filename)
            put("inputDir", inputDir.toString())
            put(
                "hint",
                "用 comfy_submit 覆盖工作流里 LoadImage 节点的 image 输入：" +
                    """{"<节点id>.image":"$filename"}（节点 id 用 comfy_find_workflow 的 includeGraph=true 查）。""",
            )
        }
    }

    /**
     * ComfyUI 的 `input` 目录。
     *
     * 优先从**已经配置好的输出目录**推（capture 配置里的 outputDir 与 input 是兄弟目录，
     * 这是 ComfyUI 的固定布局）；推不出来再走 [ComfyLocator] 的探测。
     * 探测**只读**，找不到就返回 null（绝不在随机位置建目录）。
     */
    private fun resolveComfyInputDir(cfg: AppConfig): Path? {
        val configured = runCatching { SettingsRepo.captureConfig(cfg).outputDir }.getOrNull()
            ?.takeIf { it.isNotBlank() }
            ?.let { runCatching { Paths.get(it) }.getOrNull() }
        if (configured != null) {
            val home = if (configured.fileName?.toString().equals("output", ignoreCase = true)) {
                configured.parent
            } else {
                configured
            }
            home?.resolve("input")?.let { if (Files.isDirectory(it)) return it }
        }
        val home = ComfyLocator.find(cfg).home?.let { runCatching { Paths.get(it) }.getOrNull() } ?: return null
        val input = home.resolve("input")
        return if (Files.isDirectory(input)) input else null
    }

    /**
     * 投放进 input 目录的文件名。
     *
     * 名字里带附件 id 的前 8 位：同一个附件永远得到同一个名字（重跑工作流不会指向一个已经没有的文件），
     * 不同附件即使原文件名相同也不会互相覆盖。ComfyUI 的 LoadImage 只认文件名本身，
     * 所以路径分隔符之类的非法字符一律清掉。
     */
    internal fun inputFilenameFor(originalName: String, attachmentId: String, wantedName: String?): String {
        val raw = (wantedName?.takeIf { it.isNotBlank() } ?: originalName).trim()
        val safe = raw.map { if (it.isLetterOrDigit() || it == '.' || it == '-' || it == '_') it else '_' }
            .joinToString("")
            .trimStart('.')
            .takeLast(120)
            .ifBlank { "image.png" }
        val dot = safe.lastIndexOf('.')
        val stem = (if (dot > 0) safe.substring(0, dot) else safe).ifBlank { "image" }
        val ext = (if (dot > 0) safe.substring(dot) else ".png").take(10)
        val tag = attachmentId.filter { it.isLetterOrDigit() }.take(8).ifBlank { "ref" }
        return "$stem-$tag$ext"
    }
}
