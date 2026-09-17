package com.comfyhub

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import org.slf4j.LoggerFactory
import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.time.Duration
import java.time.Instant
import java.util.concurrent.atomic.AtomicBoolean

/**
 * ComfyUI 自动捕获。
 *
 * 两条路都通到这里：
 *   1. **轮询**（默认开启，什么都不用装）：定时读 ComfyUI 的 `/history`，
 *      每发现一条新的已完成运行，就把「参数（API 节点图）+ 工作流 + 产物文件」整条收进来。
 *   2. **推送**：ComfyUI 里装了 `comfyhub_capture` 自定义节点时，跑完立刻 POST `/api/ingest/comfyui`。
 *
 * 3. **历史导入**：已经生成好的 PNG 里本来就嵌着 `prompt` / `workflow`，
 *    `importFolder()` 能直接把老图连着提示词一起收进库。
 *
 * 幂等靠 `capture_runs.run_key`（= ComfyUI 的 prompt_id）+ 文件 SHA-256 双重保证，
 * 重复轮询 / 重复推送 / 重复导入都不会产生重复数据。
 */
class ComfyCapture(private val cfg: AppConfig, private val storage: Storage) {

    private val log = LoggerFactory.getLogger(ComfyCapture::class.java)

    private val http: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(4))
        .followRedirects(HttpClient.Redirect.NORMAL)
        .build()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val pollInFlight = AtomicBoolean(false)

    @Volatile private var lastPollAt: Instant? = null
    @Volatile private var lastError: String? = null
    @Volatile private var comfyReachable = false
    @Volatile private var queueRunning = 0
    @Volatile private var queuePending = 0

    /** 队列里正在跑的任务名（ComfyUI 会在队列项里给 `extra_pnginfo.workflow.extra.title`）。 */
    @Volatile private var queueRunningLabel: String? = null

    /** 界面上「正在生成什么」的一句话：取不到就返回 null（界面自己说"正在生成…"）。 */
    fun runningLabel(): String? = queueRunningLabel

    /** 队列长度（最近一次刷新时的快照）。 */
    fun queueRunning(): Int = queueRunning

    fun queuePending(): Int = queuePending

    /** 最近一次轮询时 ComfyUI 是否可达。 */
    fun isReachable(): Boolean = comfyReachable

    /**
     * 立刻去问一次 ComfyUI 的队列（界面"实时进度"用）。
     *
     * 与 [pollOnce] 的区别：只刷队列计数，**不做** `/history` 的入库扫描 ——
     * 界面 1.5 秒轮询一次，不能每次都触发一遍解析与写库。
     */
    fun refreshQueueNow() {
        val conf = runCatching { SettingsRepo.captureConfig(cfg) }
            .getOrElse { CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir) }
        runCatching {
            refreshQueue(conf.comfyUrl)
            comfyReachable = true
        }.onFailure {
            comfyReachable = false
            lastError = friendlyError(it as? Exception ?: Exception(it), conf.comfyUrl)
        }
    }

    // -----------------------------------------------------------------------
    //  轮询
    // -----------------------------------------------------------------------

    fun start() {
        scope.launch {
            delay(2500)
            while (currentCoroutineContext().isActive) {
                val conf = runCatching { SettingsRepo.captureConfig(cfg) }.getOrNull()
                if (conf == null || !conf.enabled) {
                    delay(5_000)
                    continue
                }
                runCatching { pollOnce(conf) }.onFailure {
                    lastError = it.message
                    log.debug("轮询失败: {}", it.message)
                }
                delay(conf.pollSeconds.coerceIn(1, 600) * 1000L)
            }
        }
        log.info("ComfyUI 自动捕获已启动（默认地址 {}，可在 App 设置里改）", cfg.comfyUrl)
    }

    fun stop() {
        runCatching { scope.cancel() }
    }

    /** 跑一轮：把 `/history` 里还没捕获的完成项收进来 */
    fun pollOnce(conf: CaptureConfig = SettingsRepo.captureConfig(cfg)): CapturePollResult {
        if (!pollInFlight.compareAndSet(false, true)) {
            return CapturePollResult(ok = false, message = "上一次轮询还没结束")
        }
        try {
            val history = fetchJson("${conf.comfyUrl.trimEnd('/')}/history?max_items=100")
            comfyReachable = true
            val obj = history as? JsonObject
                ?: return CapturePollResult(ok = false, message = "ComfyUI /history 返回格式异常")
            runCatching { refreshQueue(conf.comfyUrl) }

            var checked = 0
            var newRuns = 0
            var newMedia = 0

            // history 按提交顺序返回（旧 -> 新），这里从最新往回处理
            for ((runKey, entryEl) in obj.entries.toList().reversed()) {
                if (checked >= conf.maxPerPoll) break
                val entry = entryEl as? JsonObject ?: continue
                val statusObj = entry["status"] as? JsonObject
                val completed = (statusObj?.get("completed") as? JsonPrimitive)?.booleanOrNull ?: false
                val statusStr = (statusObj?.get("status_str") as? JsonPrimitive)?.contentOrNull
                // status 可能是 null（例如被中断的运行），这时只要已经有产物就照样收
                val hasOutputs = (entry["outputs"] as? JsonObject)?.isNotEmpty() == true
                if (!completed && statusStr != "error" && !hasOutputs) continue

                if (!CaptureRepo.beginRun(runKey, "ComfyUI", entry).claimed) continue
                checked++

                val req = buildIngestRequest(runKey, entry, conf, statusStr == "error")
                val result = ingestClaimed(req, conf)
                if (result.imported > 0) {
                    newRuns++
                    newMedia += result.imported
                }
            }

            lastPollAt = Instant.now()
            lastError = null
            return CapturePollResult(ok = true, checked = checked, newRuns = newRuns, newMedia = newMedia)
        } catch (e: Exception) {
            comfyReachable = false
            lastError = friendlyError(e, conf.comfyUrl)
            return CapturePollResult(ok = false, message = "连不上 ComfyUI（${conf.comfyUrl}）：$lastError")
        } finally {
            pollInFlight.set(false)
        }
    }

    /** 把底层网络异常翻译成用户能看懂的一句话（原始类名对用户没意义） */
    private fun friendlyError(e: Exception, comfyUrl: String): String = when (e) {
        is java.net.ConnectException -> "连接被拒绝 —— ComfyUI 没在运行？"
        is java.net.http.HttpTimeoutException -> "连接超时"
        is java.net.SocketTimeoutException -> "读取超时"
        is java.net.UnknownHostException -> "地址无法解析：$comfyUrl"
        is java.io.IOException -> e.message?.takeIf { it.isNotBlank() } ?: "网络错误（${e::class.simpleName}）"
        else -> e.message?.takeIf { it.isNotBlank() } ?: e::class.simpleName ?: "未知错误"
    }

    private fun buildIngestRequest(
        runKey: String,
        entry: JsonObject,
        conf: CaptureConfig,
        isError: Boolean,
    ): IngestRequest {
        val history = HistoryEntry.parse(entry)
        val graph = history.graph

        val outputs = mutableListOf<IngestOutput>()
        val outputsObj = entry["outputs"] as? JsonObject
        outputsObj?.forEach nodeLoop@{ (nodeId, nodeOut) ->
            val node = nodeOut as? JsonObject ?: return@nodeLoop
            val nodeType = (graph?.get(nodeId) as? JsonObject)?.cls()
            for (key in listOf("images", "gifs", "audio", "video", "videos", "files")) {
                val arr = node[key] as? JsonArray ?: continue
                arr.forEach itemLoop@{ item ->
                    val o = item as? JsonObject ?: return@itemLoop
                    val filename = (o["filename"] as? JsonPrimitive)?.contentOrNull ?: return@itemLoop
                    outputs += IngestOutput(
                        filename = filename,
                        subfolder = (o["subfolder"] as? JsonPrimitive)?.contentOrNull,
                        type = (o["type"] as? JsonPrimitive)?.contentOrNull ?: "output",
                        nodeId = nodeId,
                        nodeType = nodeType,
                    )
                }
            }
        }

        return IngestRequest(
            runKey = runKey,
            source = "ComfyUI",
            status = if (isError) "error" else "success",
            comfyUrl = conf.comfyUrl,
            outputDir = conf.outputDir,
            prompt = graph,
            workflow = history.workflow,
            outputs = outputs,
            raw = entry,
        )
    }

    // -----------------------------------------------------------------------
    //  入库
    // -----------------------------------------------------------------------

    /** 外部（自定义节点 / 脚本）调用：自己抢运行锁 */
    fun ingest(req: IngestRequest): IngestResult {        val runKey = req.runKey.trim()
        if (runKey.isEmpty()) return IngestResult(runKey = "", message = "runKey 不能为空")

        val conf = runCatching { SettingsRepo.captureConfig(cfg) }
            .getOrElse { CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir) }

        val claim = CaptureRepo.beginRun(runKey, req.source.ifBlank { "ComfyUI" }, req.raw)
        if (!claim.claimed) {
            return IngestResult(
                runKey = runKey,
                promptId = claim.existingPromptId,
                alreadyCaptured = true,
                message = "这次运行已经捕获过了",
            )
        }
        return ingestClaimed(req, conf)
    }

    /**
     * 把**一条已经跑完的 `/history` 条目**收进库（用户建议 ①：AI 提交任务后的产出也要入库）。
     *
     * 与轮询捕获走完全同一条路（解析 → 建提示词 → 导入产物 → 完成运行记录），
     * 幂等同样靠 `capture_runs.run_key` + 文件 SHA-256：
     * 所以"AI 刚提交完就入库"和"轮询几秒后又看到它"不会变成两条数据。
     *
     * 入口是 [ComfySubmitter]：它提交后自己盯着这一次运行，跑完直接调这里，
     * 不用等后台轮询的下一个周期（用户希望"提交完就能看到结果"）。
     */
    fun captureRun(runKey: String, entry: kotlinx.serialization.json.JsonObject): IngestResult {
        val conf = runCatching { SettingsRepo.captureConfig(cfg) }
            .getOrElse { CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir) }
        val claim = CaptureRepo.beginRun(runKey, "ComfyUI", entry)
        if (!claim.claimed) {
            return IngestResult(
                runKey = runKey,
                promptId = claim.existingPromptId,
                alreadyCaptured = true,
                message = "这次运行已经捕获过了",
            )
        }
        val statusObj = entry["status"] as? kotlinx.serialization.json.JsonObject
        val isError = (statusObj?.get("status_str") as? kotlinx.serialization.json.JsonPrimitive)
            ?.contentOrNull == "error"
        val req = buildIngestRequest(runKey, entry, conf, isError)
        log.info(
            "captureRun {}：outputDir={} 产物条目={} 图节点={}",
            runKey, conf.outputDir, req.outputs.size, req.prompt?.size ?: 0,
        )
        return ingestClaimed(req, conf)
    }

    /**
     * 从一条已捕获运行的原始 `/history` 片段里取出 **API 格式节点图**。
     *
     * 为什么要有它：`prompts.workflow_json` 存的是**界面格式**（能拖回 ComfyUI 复现），
     * 而提交任务需要的是 API 格式。两者不能互相转换（界面格式里 `widgets_values` 只有位置、
     * 没有参数名，猜就会猜错），所以这里回到最原始的那份记录上取 —— 它就是"当时真正跑的东西"。
     *
     * 老记录（捕获前就存在的 import 项）可能没有，返回 null，由调用方如实说明。
     */
    fun apiGraphOf(promptId: Long): kotlinx.serialization.json.JsonObject? {
        val prompt = PromptRepo.get(promptId) ?: return null
        val runKey = prompt.sourceRef ?: return null
        val raw = CaptureRepo.rawOf(runKey) ?: return null
        val entry = runCatching {
            AppJson.parseToJsonElement(raw) as? kotlinx.serialization.json.JsonObject
        }.getOrNull() ?: return null
        return HistoryEntry.parse(entry).graph
    }

    /** 已经抢到运行锁之后的实际入库逻辑 */
    private fun ingestClaimed(req: IngestRequest, conf: CaptureConfig): IngestResult {        val runKey = req.runKey
        val source = req.source.ifBlank { "ComfyUI" }
        try {
            val parsed = GraphParse.parse(req.prompt)

            val tags = LinkedHashSet<String>()
            if (source == "ComfyUI" && conf.autoTag.isNotBlank()) tags += conf.autoTag.trim()
            req.extra?.tags?.forEach { t -> if (t.isNotBlank()) tags += t.trim() }

            val files = req.outputs
                .filter { it.filename.isNotBlank() }
                .filter { it.type.isNullOrBlank() || it.type.equals("output", ignoreCase = true) }
                .distinctBy { Triple(it.filename, it.subfolder ?: "", it.type ?: "output") }

            if (files.isEmpty()) {
                val status = if (req.status == "error") "error" else "empty"
                CaptureRepo.finishRun(runKey, null, status, 0, null, req.error)
                // 这条日志是排"产物怎么没进来"的第一现场：说清楚是 /history 里就没有 outputs，
                // 还是 outputs 里的条目被过滤掉了（type 不是 output / 文件名为空）
                log.warn(
                    "运行 {} 没有可入库的产物：/history 的 outputs 原始条目={}，过滤后={}",
                    runKey, (req.raw?.get("outputs") as? JsonElement)?.toString()?.take(400) ?: "(无 outputs)",
                    files.size,
                )
                return IngestResult(
                    runKey = runKey,
                    message = if (status == "error") "这次运行执行失败，只记录了日志" else "这次运行没有产出文件",
                )
            }

            val title = makeTitle(req, parsed, files)
            // 优先用界面格式工作流（能直接拖回 ComfyUI）；没有的话把 API 节点图原样存下来 ——
            // agent / 脚本提交的运行只有 API 图，但那份 JSON 同样能在 ComfyUI 里加载，
            // 总比"这次生成没有工作流"强（前端会按格式给出不同提示）。
            val workflowJson = (req.workflow ?: req.prompt)?.let {
                runCatching { AppJson.encodeToString(JsonObject.serializer(), it) }.getOrNull()
            }

            val promptId = Db.tx { conn ->
                PromptRepo.createCaptured(
                    conn = conn,
                    parsed = parsed,
                    title = title,
                    source = source,
                    sourceRef = runKey,
                    workflowJson = workflowJson,
                    tags = tags.toList(),
                    notes = req.extra?.notes,
                )
            }

            val comfyUrl = (req.comfyUrl ?: conf.comfyUrl).trimEnd('/')
            var imported = 0
            var duplicates = 0
            var failed = 0
            val mediaIds = mutableListOf<Long>()
            val temps = mutableListOf<Path>()

            for (f in files) {
                val local = resolveLocalOutput(req.outputDir ?: conf.outputDir, f)
                var src = local
                var isTemp = false
                if (src == null && conf.downloadFallback && comfyUrl.isNotBlank()) {
                    src = downloadOutput(comfyUrl, f, temps)
                    isTemp = src != null
                }
                if (src == null) {
                    failed++
                    log.warn("找不到 ComfyUI 产物文件: {}（本地无 outputDir 命中，HTTP 下载也没成功）", f.filename)
                    continue
                }
                log.info("捕获产物 {} -> {}", f.filename, src)

                when (val r = CaptureRepo.importFile(
                    storage = storage,
                    source = src,
                    originalName = f.filename,
                    kindHint = kindFor(f),
                    promptId = promptId,
                    title = null,
                    sourceLabel = source,
                    sourceRef = runKey,
                    workflowJson = workflowJson,
                    tags = tags.toList(),
                    move = isTemp,
                )) {
                    is CaptureRepo.Imported.Created -> {
                        imported++
                        mediaIds += r.mediaId
                    }
                    is CaptureRepo.Imported.Duplicate -> duplicates++
                    is CaptureRepo.Imported.Failed -> {
                        failed++
                        log.warn("产物入库失败 {}: {}", f.filename, r.reason)
                    }
                }
            }

            temps.forEach { runCatching { Files.deleteIfExists(it) } }
            CaptureRepo.finishRun(runKey, promptId, "success", imported, title, null)

            return IngestResult(
                runKey = runKey,
                promptId = promptId,
                created = true,
                mediaIds = mediaIds,
                imported = imported,
                duplicates = duplicates,
                failed = failed,
            )
        } catch (e: Exception) {
            log.error("捕获运行 {} 失败", runKey, e)
            runCatching { CaptureRepo.finishRun(runKey, null, "error", 0, null, e.message) }
            return IngestResult(runKey = runKey, message = "捕获失败: ${e.message}")
        }
    }

    // -----------------------------------------------------------------------
    //  历史文件导入
    // -----------------------------------------------------------------------

    private data class HistImport(
        val created: Boolean = false,
        val duplicate: Boolean = false,
        val promptId: Long? = null,
        val reason: String? = null,
    )

    fun importFolder(req: ImportFolderRequest): ImportFolderResult {
        val dir = Paths.get(req.dir)
        require(Files.isDirectory(dir)) { "目录不存在: ${req.dir}" }

        val limit = req.limit.coerceIn(1, 5000)
        val extensions = MediaFiles.IMAGE_EXT + MediaFiles.VIDEO_EXT + MediaFiles.AUDIO_EXT
        val tags = req.tags.filter { it.isNotBlank() }

        var scanned = 0
        var imported = 0
        var duplicates = 0
        var failed = 0
        var promptsCreated = 0
        val promptIds = mutableListOf<Long>()
        val errors = mutableListOf<String>()

        val candidates: List<Path> = try {
            if (req.recursive) {
                Files.walk(dir).use { s -> s.filter { Files.isRegularFile(it) }.toList() }
            } else {
                Files.list(dir).use { s -> s.filter { Files.isRegularFile(it) }.toList() }
            }
        } catch (e: Exception) {
            throw IllegalArgumentException("读取目录失败: ${e.message}")
        }

        for (path in candidates) {
            if (scanned >= limit) break
            // 别把自己 storage 里的文件再导入一遍
            if (path.startsWith(storage.mediaDir) || path.startsWith(storage.thumbDir)) continue
            val name = path.fileName.toString()
            if (name.startsWith(".")) continue
            if (MediaFiles.extensionOf(name) !in extensions) continue
            scanned++

            try {
                val r = importOneHistorical(path, req.linkWorkflow, tags)
                when {
                    r.created -> {
                        imported++
                        r.promptId?.let { promptIds += it; promptsCreated++ }
                    }
                    r.duplicate -> duplicates++
                    else -> {
                        failed++
                        r.reason?.let { errors += "$name: $it" }
                    }
                }
            } catch (e: Exception) {
                failed++
                errors += "$name: ${e.message}"
            }
        }

        return ImportFolderResult(
            dir = dir.toString(),
            scanned = scanned,
            imported = imported,
            duplicates = duplicates,
            failed = failed,
            promptsCreated = promptsCreated,
            promptIds = promptIds.distinct(),
            errors = errors.take(20),
        )
    }

    private fun importOneHistorical(path: Path, linkWorkflow: Boolean, tags: List<String>): HistImport {
        val name = path.fileName.toString()

        // 内容去重：已经入过库的文件直接跳过，也顺带避免重复建提示词
        val sha = MediaFiles.sha256(path)
        val existing = Db.withConnection { conn -> MediaRepo.findBySha(conn, sha) }
        if (existing != null) return HistImport(duplicate = true, promptId = existing.promptId)

        val isPng = MediaFiles.extensionOf(name) == "png"
        val chunks = if (isPng) PngMeta.readTextChunks(path) else emptyMap()
        val graph = chunks["prompt"]?.let { text ->
            runCatching { AppJson.parseToJsonElement(text) as? JsonObject }.getOrNull()
        }
        val workflow = chunks["workflow"]

        if (linkWorkflow && graph != null) {
            val runKey = "import:${sha.take(40)}"
            // force：上面已经确认过文件不在库里，历史运行记录不该拦住重新导入
            val claim = CaptureRepo.beginRun(runKey, "ComfyUI-Import", force = true)
            if (!claim.claimed) return HistImport(duplicate = true, promptId = claim.existingPromptId)

            val parsed = GraphParse.parse(graph)
            val title = makeImportTitle(parsed, name)
            val promptId = Db.tx { conn ->
                PromptRepo.createCaptured(
                    conn = conn,
                    parsed = parsed,
                    title = title,
                    source = "ComfyUI-Import",
                    sourceRef = runKey,
                    workflowJson = workflow,
                    tags = tags,
                    notes = "从 ${path.parent} 导入，参数取自 PNG 内嵌的 prompt 元数据",
                )
            }
            return when (val r = CaptureRepo.importFile(
                storage = storage,
                source = path,
                originalName = name,
                promptId = promptId,
                sourceLabel = "ComfyUI-Import",
                sourceRef = runKey,
                workflowJson = workflow,
                tags = tags,
            )) {
                is CaptureRepo.Imported.Created -> {
                    CaptureRepo.finishRun(runKey, promptId, "success", 1, title, null)
                    HistImport(created = true, promptId = promptId)
                }
                is CaptureRepo.Imported.Duplicate -> {
                    CaptureRepo.finishRun(runKey, promptId, "duplicate", 0, title, null)
                    HistImport(duplicate = true, promptId = promptId)
                }
                is CaptureRepo.Imported.Failed -> {
                    CaptureRepo.finishRun(runKey, null, "error", 0, title, r.reason)
                    HistImport(reason = r.reason)
                }
            }
        }

        // 没有内嵌工作流（视频 / 音频 / 手改过的图）：只当普通产物收进来
        return when (val r = CaptureRepo.importFile(
            storage = storage,
            source = path,
            originalName = name,
            promptId = null,
            sourceLabel = "ComfyUI-Import",
            workflowJson = workflow,
            tags = tags,
        )) {
            is CaptureRepo.Imported.Created -> HistImport(created = true)
            is CaptureRepo.Imported.Duplicate -> HistImport(duplicate = true, promptId = r.existingId)
            is CaptureRepo.Imported.Failed -> HistImport(reason = r.reason)
        }
    }

    // -----------------------------------------------------------------------
    //  状态
    // -----------------------------------------------------------------------

    fun status(): CaptureStatus {
        val conf = runCatching { SettingsRepo.captureConfig(cfg) }
            .getOrElse { CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir) }
        val (runs, media) = runCatching { CaptureRepo.stats() }.getOrDefault(0L to 0L)

        val reachable = runCatching {
            refreshQueue(conf.comfyUrl)
            true
        }.getOrDefault(false)
        comfyReachable = reachable

        return CaptureStatus(
            enabled = conf.enabled,
            comfyUrl = conf.comfyUrl,
            outputDir = conf.outputDir,
            comfyReachable = reachable,
            queueRunning = queueRunning,
            queuePending = queuePending,
            lastPollAt = lastPollAt?.toString(),
            lastError = lastError,
            capturedRuns = runs,
            capturedMedia = media,
            recent = runCatching { CaptureRepo.recentRuns(20) }.getOrDefault(emptyList()),
        )
    }

    // -----------------------------------------------------------------------
    //  小工具
    // -----------------------------------------------------------------------

    private fun makeTitle(req: IngestRequest, parsed: GraphParse.Parsed, files: List<IngestOutput>): String {
        val explicit = req.extra?.title?.takeIf { it.isNotBlank() }
        val prefix = explicit
            ?: parsed.titleHint
            ?: files.first().filename.substringBeforeLast('.').trimEnd('_', ' ', '-')
        val base = prefix.ifBlank { "ComfyUI 生成" }
        val seed = parsed.seed
        return (if (seed != null) "$base · seed $seed" else base).take(250)
    }

    private fun makeImportTitle(parsed: GraphParse.Parsed, filename: String): String {
        val prefix = parsed.titleHint
            ?: filename.substringBeforeLast('.').trimEnd('_', ' ', '-')
        val base = prefix.ifBlank { "导入的产物" }
        val seed = parsed.seed
        return (if (seed != null) "$base · seed $seed" else base).take(250)
    }

    private fun kindFor(f: IngestOutput): String {
        f.kind?.uppercase()?.takeIf { it in MediaRepo.KINDS }?.let { return it }
        val node = f.nodeType.orEmpty().lowercase()
        val ext = MediaFiles.extensionOf(f.filename)
        return when {
            node.contains("audio") && node.contains("save") -> "AUDIO"
            node.contains("previewaudio") -> "AUDIO"
            node.contains("videocombine") || node.contains("savevideo") || node.contains("savewebm") -> "VIDEO"
            node.contains("animated") -> "VIDEO"
            ext in MediaFiles.AUDIO_EXT -> "AUDIO"
            ext in MediaFiles.VIDEO_EXT -> "VIDEO"
            ext in MediaFiles.IMAGE_EXT -> "IMAGE"
            else -> MediaFiles.kindOf(f.filename)
        }
    }

    private fun resolveLocalOutput(outputDir: String?, f: IngestOutput): Path? {
        if (outputDir.isNullOrBlank()) return null
        return try {
            var p = Paths.get(outputDir)
            if (!f.subfolder.isNullOrBlank()) p = p.resolve(f.subfolder!!)
            p = p.resolve(f.filename).normalize()
            p.takeIf { Files.isRegularFile(it) }
        } catch (e: Exception) {
            log.debug("解析本地输出路径失败 {}: {}", f.filename, e.message)
            null
        }
    }

    private fun downloadOutput(comfyUrl: String, f: IngestOutput, temps: MutableList<Path>): Path? {
        val ext = MediaFiles.extensionOf(f.filename).ifBlank { "bin" }
        var dest: Path? = null
        return try {
            val query = buildString {
                append("filename=").append(enc(f.filename))
                if (!f.subfolder.isNullOrBlank()) append("&subfolder=").append(enc(f.subfolder!!))
                append("&type=").append(enc(f.type ?: "output"))
            }
            val uri = URI.create("$comfyUrl/view?$query")
            val req = HttpRequest.newBuilder(uri).timeout(Duration.ofMinutes(5)).GET().build()
            val res = http.send(req, HttpResponse.BodyHandlers.ofInputStream())
            if (res.statusCode() !in 200..299) {
                res.body().close()
                return null
            }
            dest = storage.tempFile(".$ext")
            val tmp = dest!!
            res.body().use { input -> Files.newOutputStream(tmp).use { out -> input.copyTo(out) } }
            temps.add(tmp)
            tmp
        } catch (e: Exception) {
            dest?.let { runCatching { Files.deleteIfExists(it) } }
            log.warn("下载 ComfyUI 产物失败 {}: {}", f.filename, e.message)
            null
        }
    }

    private fun fetchJson(url: String): JsonElement {
        val req = HttpRequest.newBuilder(URI.create(url))
            .timeout(Duration.ofSeconds(20))
            .header("Accept", "application/json")
            .GET()
            .build()
        val res = http.send(req, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8))
        if (res.statusCode() !in 200..299) throw IllegalStateException("HTTP ${res.statusCode()}")
        return AppJson.parseToJsonElement(res.body())
    }

    private fun refreshQueue(comfyUrl: String) {
        val obj = fetchJson("${comfyUrl.trimEnd('/')}/queue") as? JsonObject ?: return
        val running = obj["queue_running"] as? JsonArray
        queueRunning = running?.size ?: 0
        queuePending = (obj["queue_pending"] as? JsonArray)?.size ?: 0
        queueRunningLabel = running?.firstOrNull()?.let { labelOfQueueItem(it) }
    }

    /**
     * 队列项是 `[number, prompt_id, graph, extra_data, outputs]`：
     * 标题藏在 `extra_data.extra_pnginfo.workflow.extra.title`（我们在 ComfyUI 里给工作流起的名字）。
     * 取不到就返回 null —— 界面宁可只说「正在生成…」，也不要编一个名字出来。
     */
    private fun labelOfQueueItem(item: JsonElement): String? = runCatching {
        val row = item as? JsonArray ?: return null
        val extra = row.firstOrNull { (it as? JsonObject)?.containsKey("extra_pnginfo") == true } as? JsonObject
            ?: return null
        val workflow = (extra["extra_pnginfo"] as? JsonObject)?.get("workflow") as? JsonObject ?: return null
        val title = ((workflow["extra"] as? JsonObject)?.get("title") as? JsonPrimitive)?.contentOrNull
        title?.takeIf { it.isNotBlank() }
    }.getOrNull()

    private fun enc(value: String): String =
        URLEncoder.encode(value, StandardCharsets.UTF_8).replace("+", "%20")
}

/**
 * 解析 `/history` 里的一条运行记录。
 *
 * 坑点：`prompt` 字段是**队列项整条 Tuple 的拷贝**，而它的长度变过：
 *
 * | ComfyUI | 形状 |
 * | --- | --- |
 * | 0.34.2 之前 | `[节点图, extra_data, 要执行的输出节点]` |
 * | 0.34.2 起 | `[编号, prompt_id, 节点图, extra_data, 要执行的输出节点, 敏感数据]` |
 *
 * （server.py 入队的是 `(number, prompt_id, prompt, extra_data, outputs_to_execute, sensitive)`，
 * execution.py 的 `task_done` 又原样写进 history。）
 *
 * 按下标取就会在升级后**静默取空**：prompt[0] 是数字、prompt[1] 是 prompt_id 字符串，
 * 于是提示词、参数、工作流全丢 —— 只剩一个"这次运行没有产出工作流"的空壳。
 * 所以这里按内容认：值全是 `{class_type: ...}` 的对象是节点图，另一个带
 * `extra_pnginfo` / `client_id` 的对象就是 extra_data。
 */
internal object HistoryEntry {

    data class Parsed(
        val graph: JsonObject? = null,
        val extraData: JsonObject? = null,
        val workflow: JsonObject? = null,
    )

    fun parse(entry: JsonObject): Parsed {
        when (val field = entry["prompt"]) {
            // 万一哪天改成具名形式 {prompt: …, extra_data: …}
            is JsonObject -> {
                val graph = (field["prompt"] as? JsonObject) ?: field.takeIf { isNodeGraph(it) }
                val extra = (field["extra_data"] as? JsonObject) ?: field.takeIf { looksLikeExtraData(it) }
                return Parsed(graph, extra, workflowOf(extra))
            }
            is JsonArray -> {
                val objects = field.mapNotNull { it as? JsonObject }
                val graph = objects.firstOrNull { isNodeGraph(it) }
                val extra = objects.firstOrNull { it !== graph && looksLikeExtraData(it) }
                return Parsed(graph, extra, workflowOf(extra))
            }
            else -> return Parsed()
        }
    }

    /** API 格式节点图：每个值都是带 class_type 的对象 */
    fun isNodeGraph(obj: JsonObject): Boolean =
        obj.isNotEmpty() && obj.values.all { (it as? JsonObject)?.containsKey("class_type") == true }

    private fun looksLikeExtraData(obj: JsonObject): Boolean =
        obj.containsKey("extra_pnginfo") || obj.containsKey("client_id") || obj.containsKey("create_time")

    /** 界面格式工作流藏在 extra_data.extra_pnginfo.workflow 里 */
    fun workflowOf(extraData: JsonObject?): JsonObject? =
        (extraData?.get("extra_pnginfo") as? JsonObject)?.get("workflow") as? JsonObject
}
