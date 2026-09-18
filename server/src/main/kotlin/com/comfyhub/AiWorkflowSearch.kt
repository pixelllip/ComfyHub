package com.comfyhub

import com.comfyhub.ai.AiAttachmentRepo
import com.comfyhub.ai.AiAttachmentStore
import com.comfyhub.ai.Modality
import com.comfyhub.ai.tools.ComfySubmitOutcome
import com.comfyhub.ai.tools.ToolFailure
import com.comfyhub.ai.tools.WorkflowSearch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
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

    private val log = LoggerFactory.getLogger(AiWorkflowSearch::class.java)

    /** 从本机文件加载进来的工作流在库里的来源标记（也是自动打的标签）。 */
    const val SOURCE = "ComfyUI-File"

    /** 一份工作流文件最大读多少（本机工作流通常几十 KB，上百 MB 的一定是拿错文件了）。 */
    const val MAX_WORKFLOW_FILE_BYTES = 32L * 1024 * 1024

    /** 节点摘要最多列几个（8KB 的工具结果装不下 100+ 个节点的完整清单）。 */
    const val MAX_SUMMARY_NODES = 120

    /** 工具结果是**截断**过的（全局 8KB 上限）：每一类缺口各留多少条。 */
    const val MAX_UNSUPPORTED = 20
    const val MAX_GAPS = 40
    const val MAX_REWRITTEN = 12
    const val MAX_WARNINGS = 5


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
     * 把一份**本机工作流文件**读进库，返回一个可以交给 `comfy_submit` 的 `promptId`
     * （用户 bug ③："comfy_submit 只认库里的 promptId —— 我不能凭一个文件路径提交"）。
     *
     * 两条路都要认：
     *  - 文件是 **API 格式**（`{"1":{"class_type":…}}`，ComfyUI 的「导出（API）」产物）→ **原样用**，
     *    零转换、零猜测；
     *  - 文件是 **界面格式**（`{nodes:[…], links:[…]}`，ComfyUI 保存工作流的默认格式）→
     *    用 `/object_info` 做等价转换（[WorkflowConvert]）。转换不出来的（Anything Everywhere
     *    这类纯前端节点）**如实报错**，绝不糊一个"看起来差不多"的图 —— 猜错的代价是 ComfyUI
     *    报一堆莫名其妙的节点错误，用户根本查不出来。
     *
     * 入库是**幂等**的：`run_key = file:<文件 sha256>`，同一份文件重复加载只会得到同一个 promptId。
     */
    suspend fun loadWorkflowFromFile(
        submitter: ComfySubmitter,
        file: Path,
        title: String?,
        includeGraph: Boolean,
        /**
         * 遇到转换不了的前端节点时**是否照样入库**（默认 false = 如实报错）。
         *
         * 这是"放权给 AI"的那条通道（用户建议 ②）：点名之后，转换结果会带上
         * **缺口清单**（哪个输入还悬着、哪些连线型输入空着、缺的是哪个类），
         * 模型照着清单用 `comfy_submit(connections=…)` 把线补上再提交。
         * 补的是"哪根线接哪"，不是让谁去猜语义 —— 猜的那部分仍然由 ComfyUI 的校验兜底。
         */
        tolerateUnsupported: Boolean = false,
    ): JsonObject {
        if (!Files.isRegularFile(file)) {
            throw ToolFailure("NOT_A_FILE", "不是文件：$file")
        }
        val size = runCatching { Files.size(file) }.getOrDefault(0L)
        if (size <= 0L) throw ToolFailure("EMPTY_FILE", "文件是空的：$file")
        if (size > MAX_WORKFLOW_FILE_BYTES) {
            throw ToolFailure(
                "TOO_LARGE",
                "工作流文件太大（$size 字节 > $MAX_WORKFLOW_FILE_BYTES）：$file",
            )
        }
        val bytes = Files.readAllBytes(file)
        val root = runCatching { AppJson.parseToJsonElement(String(bytes, Charsets.UTF_8)) as? JsonObject }
            .getOrNull()
            ?: throw ToolFailure(
                "NOT_A_WORKFLOW",
                "这份文件不是 ComfyUI 工作流 JSON（解析失败）：$file。" +
                    "需要 ComfyUI 保存的工作流文件（含 nodes/links），或「导出（API）」得到的节点图。",
            )

        val apiFormat = HistoryEntry.isNodeGraph(root)
        val uiFormat = !apiFormat && WorkflowConvert.isUiWorkflow(root)
        if (!apiFormat && !uiFormat) {
            throw ToolFailure(
                "NOT_A_WORKFLOW",
                "这份 JSON 既不是 API 格式节点图，也不是界面格式工作流（没有 nodes / links）：$file",
            )
        }

        var warnings: List<String> = emptyList()
        var rewritten: List<String> = emptyList()
        var unsupported: List<String> = emptyList()
        var unresolved: List<String> = emptyList()
        var openInputs: List<String> = emptyList()
        val apiGraph: JsonObject = if (apiFormat) {
            root
        } else {
            val objectInfo = try {
                submitter.objectInfo()
            } catch (e: Exception) {
                throw ToolFailure(
                    "COMFY_UNREACHABLE",
                    "这是界面格式工作流，转换需要问 ComfyUI 要节点定义（/object_info），但连不上：" +
                        "${e.message?.take(200)}。先 comfy_get_status 看 ComfyUI 在不在跑。",
                )
            }
            val converted = try {
                WorkflowConvert.toApiGraph(root, objectInfo)
            } catch (e: IllegalArgumentException) {
                throw ToolFailure("INVALID_ARGUMENT", "转换工作流失败：${e.message}")
            }
            if (converted.unsupportedNodes.isNotEmpty() && !tolerateUnsupported) {
                throw ToolFailure(
                    "UNSUPPORTED_NODES",
                    "这条工作流用了 ComfyUI **前端**才有的节点，服务端没法等价转换：" +
                        converted.unsupportedNodes.joinToString("、").take(400) +
                        "。三个出口：① 把 tolerateUnsupported=true 再来一次 —— 我会把这些节点摘掉、" +
                        "并把**缺哪些线**列出来（openInputs / unresolvedInputs），" +
                        "你照着用 comfy_submit(connections=…) 补上就能跑；" +
                        "② 在 ComfyUI 里用「工作流菜单 → 导出（API）」存成 API 格式，再把那份文件路径给我" +
                        "（API 格式是原样提交，零转换）；" +
                        "③ 在 ComfyUI 里点一次 Queue 让它跑一次，跑完会被自动捕获，之后就能按 promptId 提交。",
                )
            }
            if (converted.graph.isEmpty()) {
                throw ToolFailure("NO_API_GRAPH", "转换之后一个可提交的节点都没有：$file")
            }
            warnings = converted.warnings.take(MAX_WARNINGS)
            rewritten = converted.rewritten.take(MAX_REWRITTEN)
            unsupported = converted.unsupportedNodes.take(MAX_UNSUPPORTED)
            unresolved = converted.unresolvedInputs.take(MAX_GAPS)
            openInputs = converted.openInputs.take(MAX_GAPS)
            converted.graph
        }

        val runKey = "file:" + sha256Hex(bytes)
        val claim = CaptureRepo.beginRun(
            runKey = runKey,
            source = SOURCE,
            raw = buildJsonObject { put("prompt", apiGraph) },
        )
        // 抢不到 + 那条记录是 `success` 但 prompt_id 已经没了 = **库里的提示词被用户删掉了**
        // （删除会把 prompt_id 置空，run_key 是文件内容算出来的，于是这份文件再也导不进来，
        // 只会永远回一句"正在被另一次调用加载"）。这种情况要能重新导入 ——
        // 正在处理中（running）的仍然让开，不然并发加载同一份文件会各建一条。
        val retried = if (!claim.claimed && claim.existingPromptId == null && claim.existingStatus == "success") {
            CaptureRepo.beginRun(
                runKey = runKey,
                source = SOURCE,
                raw = buildJsonObject { put("prompt", apiGraph) },
                force = true,
            )
        } else {
            claim
        }
        val promptId = if (!retried.claimed) {
            // 同一份文件已经加载过：直接复用（幂等，不重复建提示词）
            retried.existingPromptId ?: throw ToolFailure(
                "LOAD_IN_PROGRESS",
                "这份工作流正在被另一次调用加载，稍等一下再试（或直接用 comfy_find_workflow 找它）",
            )
        } else {
            val parsed = GraphParse.parse(apiGraph)
            val label = title?.takeIf { it.isNotBlank() }
                ?: file.fileName.toString().substringBeforeLast('.').take(200)
            val workflowJson = runCatching {
                AppJson.encodeToString(JsonObject.serializer(), if (uiFormat) root else apiGraph)
            }.getOrNull()
            val created = Db.tx { conn ->
                PromptRepo.createCaptured(
                    conn = conn,
                    parsed = parsed,
                    title = label,
                    source = SOURCE,
                    sourceRef = runKey,
                    workflowJson = workflowJson,
                    tags = listOf(SOURCE),
                    notes = "从本机工作流文件加载：$file" +
                        if (uiFormat) "（界面格式，已按 ComfyUI /object_info 转成 API 节点图）" else "（API 格式，原样使用）",
                )
            }
            CaptureRepo.finishRun(runKey, created, "success", 0, label, null)
            log.info("加载工作流文件入库：{} -> promptId={}（{} 格式，{} 个节点）", file, created, if (uiFormat) "UI" else "API", apiGraph.size)
            created
        }

        val prompt = PromptRepo.get(promptId)
        val runnable = unsupported.isEmpty() && unresolved.isEmpty()
        return buildJsonObject {
            put("promptId", promptId)
            put("runKey", runKey)
            put("title", prompt?.title ?: title ?: file.fileName.toString())
            put("path", file.toString())
            put("format", if (uiFormat) "ui" else "api")
            put("nodeCount", apiGraph.size)
            // 「能不能直接跑」是模型最需要知道的一件事：缺口的图提交上去 ComfyUI 只会报缺输入
            put("runnable", runnable)
            when {
                runnable -> put(
                    "hint",
                    "现在可以 comfy_submit(promptId=$promptId, overrides={\"<节点id>.<输入名>\": 值}) 提交它；" +
                        "参数路径见下面的 nodes（linked 里的输入是连线，不能覆盖）。",
                )
                else -> put(
                    "hint",
                    "**这条现在还不能跑**：转换时摘掉了 ${unsupported.size} 个 ComfyUI 前端节点，" +
                        "留下的缺口见 openInputs / unresolvedInputs。请照着缺口用 " +
                        "comfy_submit(promptId=$promptId, connections={\"<节点id>.<输入名>\": [\"<上游节点id>\", 槽位]}) " +
                        "把线补上再提交（要改的字面量仍走 overrides）。不确定某条线该接哪时，先问用户，别乱接。",
                )
            }
            if (rewritten.isNotEmpty()) {
                put("rewrittenNodes", buildJsonArray { rewritten.forEach { add(JsonPrimitive(it)) } })
            }
            if (unsupported.isNotEmpty()) {
                put("unsupportedNodes", buildJsonArray { unsupported.forEach { add(JsonPrimitive(it)) } })
            }
            if (unresolved.isNotEmpty()) {
                put("unresolvedInputs", buildJsonArray { unresolved.forEach { add(JsonPrimitive(it)) } })
            }
            if (openInputs.isNotEmpty()) {
                put("openInputs", buildJsonArray { openInputs.forEach { add(JsonPrimitive(it)) } })
            }
            if (warnings.isNotEmpty()) {
                put("warnings", buildJsonArray { warnings.forEach { add(JsonPrimitive(it)) } })
            }
            put("nodes", summarizeNodes(apiGraph))
            if (includeGraph) put("apiGraph", apiGraph)
        }
    }

    /**
     * 给模型看的节点摘要：每个节点的 `id` / 类型 / 标题 / 可覆盖的控件名 / 已连线的输入名。
     *
     * 为什么不直接把整张图丢过去：113 个节点的 API 图有 100+ KB，一次工具结果上限是 8KB，
     * 模型看到的会是被截断的半个 JSON —— 那比给一张清楚的"哪个节点能改什么"的表更难用。
     */
    private fun summarizeNodes(apiGraph: JsonObject): JsonArray = buildJsonArray {
        apiGraph.entries.take(MAX_SUMMARY_NODES).forEach { (id, value) ->
            val node = value as? JsonObject ?: return@forEach
            val inputs = node.inputs()
            val widgets = mutableListOf<String>()
            val linked = mutableListOf<String>()
            inputs.forEach { (name, v) ->
                if (v is JsonArray) linked += name else widgets += name
            }
            add(
                buildJsonObject {
                    put("id", id)
                    put("classType", node.cls())
                    (node["_meta"] as? JsonObject)?.get("title").asText()?.let { put("title", it) }
                    put("widgets", buildJsonArray { widgets.forEach { add(JsonPrimitive(it)) } })
                    put("linked", buildJsonArray { linked.forEach { add(JsonPrimitive(it)) } })
                },
            )
        }
    }

    private fun sha256Hex(bytes: ByteArray): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256").digest(bytes)
        return digest.joinToString("") { "%02x".format(it) }
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
        connections: JsonObject?,
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

        val (overridden, appliedOverrides) = try {
            ComfySubmitter.WorkflowEdit.applyOverrides(graph, overrides.orEmpty())
        } catch (e: IllegalArgumentException) {
            throw ToolFailure("INVALID_ARGUMENT", e.message ?: "参数覆盖不合法")
        }
        // 补线（放权通道）：界面格式里那些纯前端节点被摘掉之后，缺口由模型的 connections 填上
        val (patched, appliedConnections) = try {
            ComfySubmitter.WorkflowEdit.applyConnections(overridden, connections.orEmpty())
        } catch (e: IllegalArgumentException) {
            throw ToolFailure("INVALID_ARGUMENT", e.message ?: "补线不合法")
        }
        val applied = appliedOverrides + appliedConnections

        val label = title?.takeIf { it.isNotBlank() } ?: prompt.title
        val submission = submitter.submit(patched, label, waitSeconds = waitSeconds)

        val json = buildJsonObject {
            put("promptId", promptId)
            // 三个 id 各是什么，写清楚 —— 实测模型会拿错（它拿数字 promptId 去 comfy_get_run，
            // 而那边旧实现只认 UUID，于是"刚提交完却说查不到这次运行"）
            put("runKey", submission.promptId)
            put("comfyPromptId", submission.promptId)
            put("title", label)
            put("status", submission.status)
            put("elapsedMs", submission.elapsedMs)
            put("message", submission.message)
            put("error", submission.error)
            put("capturedPromptId", submission.capturedPromptId)
            put("idHint", "查这次运行用 comfy_get_run(runKey=\"${submission.promptId}\")；" +
                "capturedPromptId 是捕获记录编号（画廊 / 提示词用的那个是 promptId）")
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
                        "也可以隔一会儿用 comfy_get_run runKey=${submission.promptId} 查",
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
