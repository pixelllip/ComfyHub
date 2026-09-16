package com.comfyhub

import kotlinx.coroutines.delay
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.time.Duration
import java.time.Instant
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * 一次提交给 ComfyUI 的运行（用户建议 ①：「让 AI 可以直接调用 Comfy 提交任务」）。
 *
 * 这是**内存里**的进度快照：AI 工作台右侧栏用它显示实时进度，
 * 产物落库之后由捕获那套（`capture_runs` / `media_assets`）接管，这里只保留最近若干条。
 */
@Serializable
data class ComfySubmission(
    /** ComfyUI 的 prompt_id（提交成功后才有） */
    val promptId: String,
    /** 谁提交的：ai / user */
    val submittedBy: String = "ai",
    /** 提交时用的提示词标题（给界面一个好认的名字） */
    val title: String? = null,
    /** queued / running / success / error / empty / timeout */
    val status: String = "queued",
    val submittedAt: String = Instant.now().toString(),
    /** 结束时刻（还在跑时为 null） */
    val finishedAt: String? = null,
    val elapsedMs: Long = 0,
    /** 已经捕获入库的产物 id（完成后才有） */
    val mediaIds: List<Long> = emptyList(),
    /** 这次运行对应的提示词 id（捕获后才有） */
    val capturedPromptId: Long? = null,
    val error: String? = null,
    /** 人类可读的一句话进度说明 */
    val message: String? = null,
)

/**
 * ComfyUI 任务提交器（用户建议 ①）。
 *
 * 与 [ComfyCapture] 的分工：
 *  - 这里只负责**把工作流送进 ComfyUI 的队列**，并盯着这一次运行看它跑完没有；
 *  - 产物入库仍然走捕获那套（`/history` → `PromptRepo.createCaptured` → `MediaRepo`），
 *    所以"AI 提交的产出"和"用户手点的产出"在库里是同一种东西，画廊里都能看到。
 *
 * 三条纪律：
 *  1. **不接受任意 URL**：地址只来自应用配置，和查询类工具同源；
 *  2. **不做无限等待**：有明确上限（默认 4 分钟、最多 15 分钟），超时如实报 `timeout`，
 *     并说明"任务可能还在 ComfyUI 里跑，跑完会自动入库"；
 *  3. **不谎报进度**：只有 `/history` 真的给出结果才算完成。
 */
class ComfySubmitter(
    private val cfg: AppConfig,
    private val capture: ComfyCapture,
) {
    private val log = LoggerFactory.getLogger(ComfySubmitter::class.java)

    private val http: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(4))
        .followRedirects(HttpClient.Redirect.NORMAL)
        .build()

    /** 最近提交的运行（prompt_id → 进度）。界面右侧栏与 `comfy_get_status` 都读它。 */
    private val submissions = ConcurrentHashMap<String, ComfySubmission>()

    companion object {
        /** 内存里最多保留多少条提交记录（只有"最近进度"这一个用途） */
        const val MAX_TRACKED = 30

        /** 等一次运行跑完的默认 / 最大时长 */
        const val DEFAULT_WAIT_SECONDS = 240
        const val MAX_WAIT_SECONDS = 900
    }

    /** 最近的提交记录，新的在前。 */
    fun submissions(limit: Int = 10): List<ComfySubmission> =
        submissions.values.sortedByDescending { it.submittedAt }.take(limit.coerceIn(1, MAX_TRACKED))

    fun find(promptId: String): ComfySubmission? = submissions[promptId.trim()]

    /** 只提交、不等：适合"先排上队，我继续做别的"。 */
    suspend fun submitDetached(
        graph: JsonObject,
        title: String?,
        submittedBy: String = "ai",
    ): ComfySubmission = submit(graph, title, waitSeconds = 0, submittedBy = submittedBy)

    /**
     * 提交一个 **API 格式节点图** 到 ComfyUI 并等它跑完。
     *
     * @param graph ComfyUI `/prompt` 需要的 API 格式（节点 id → {class_type, inputs}）。
     *              界面格式 workflow（带 nodes/links）**不能**直接提交，见 [ComfyCapture.apiGraphOf]。
     * @param waitSeconds 等多久（0 = 不等，只提交）。**超时不代表失败**。
     */
    suspend fun submit(
        graph: JsonObject,
        title: String?,
        waitSeconds: Int = DEFAULT_WAIT_SECONDS,
        submittedBy: String = "ai",
    ): ComfySubmission {
        require(graph.isNotEmpty()) { "工作流是空的：不能提交一个没有节点的图" }
        val conf = SettingsRepo.captureConfig(cfg)
        val base = conf.comfyUrl.trimEnd('/')
        val clientId = UUID.randomUUID().toString()

        val promptId = postPrompt(base, graph, clientId)
        val started = System.currentTimeMillis()
        val initial = ComfySubmission(
            promptId = promptId,
            submittedBy = submittedBy,
            title = title,
            status = "queued",
            message = "已提交到 ComfyUI 队列",
        )
        track(initial)
        log.info("已提交 ComfyUI 任务 promptId={} 节点数={}", promptId, graph.size)

        val waitMs = waitSeconds.coerceIn(0, MAX_WAIT_SECONDS) * 1000L
        if (waitMs == 0L) return initial

        val deadline = System.currentTimeMillis() + waitMs
        var queueNote: String? = null
        while (System.currentTimeMillis() < deadline) {
            delay(1500)
            val entry = historyEntry(base, promptId)
            if (entry == null) {
                // 还没出现在 /history 里：可能还在队列 / 正在跑，顺手看一下队列位置
                queueNote = runCatching { queueNote(base, promptId) }.getOrNull() ?: queueNote
                track(initial.copy(status = "running", message = queueNote ?: "正在生成…"))
                continue
            }
            val statusObj = entry["status"] as? JsonObject
            val completed = (statusObj?.get("completed") as? JsonPrimitive)?.booleanOrNull ?: false
            val statusStr = (statusObj?.get("status_str") as? JsonPrimitive)?.contentOrNull
            val hasOutputs = (entry["outputs"] as? JsonObject)?.isNotEmpty() == true
            if (!completed && statusStr != "error" && !hasOutputs) {
                track(initial.copy(status = "running", message = "正在生成…"))
                continue
            }

            // 跑完了：交给捕获那套入库（与轮询捕获走同一条路，幂等靠 prompt_id）
            val failed = statusStr == "error"
            val result = runCatching { capture.captureRun(promptId, entry) }.getOrElse { e ->
                log.warn("提交的任务 {} 入库失败: {}", promptId, e.message)
                val broken = initial.copy(
                    status = if (failed) "error" else "success",
                    finishedAt = Instant.now().toString(),
                    elapsedMs = System.currentTimeMillis() - started,
                    error = e.message,
                    message = "运行结束，但产物入库失败：${e.message}",
                )
                track(broken)
                return broken
            }

            val done = initial.copy(
                status = if (failed) "error" else if (result.imported > 0) "success" else "empty",
                finishedAt = Instant.now().toString(),
                elapsedMs = System.currentTimeMillis() - started,
                mediaIds = result.mediaIds,
                capturedPromptId = result.promptId,
                error = result.message.takeIf { failed },
                message = when {
                    failed -> "ComfyUI 报错，这次没有产物"
                    result.imported > 0 -> "完成，产物已入库"
                    result.duplicates > 0 -> "完成（产物与已有文件相同，未重复入库）"
                    else -> "完成，但没有产物文件"
                },
            )
            track(done)
            log.info("ComfyUI 任务 {} 结束：status={} 产物={}", promptId, done.status, result.imported)
            return done
        }

        // 超时：**如实说**，不假装失败也不假装完成
        val timedOut = initial.copy(
            status = "timeout",
            elapsedMs = System.currentTimeMillis() - started,
            message = "等待超过 ${waitSeconds} 秒，任务可能还在 ComfyUI 里跑；" +
                "跑完之后会自动被捕获，稍后可以在画廊里看到。",
        )
        track(timedOut)
        return timedOut
    }

    /** 重新去 ComfyUI 看一次某次提交的进度（界面刷新用）。 */
    fun refresh(promptId: String): ComfySubmission? {
        val current = submissions[promptId] ?: return null
        if (current.status == "success" || current.status == "error" || current.status == "empty") return current
        val conf = SettingsRepo.captureConfig(cfg)
        val base = conf.comfyUrl.trimEnd('/')
        val note = runCatching { queueNote(base, promptId) }.getOrNull()
        val next = current.copy(
            elapsedMs = System.currentTimeMillis() - parseInstant(current.submittedAt),
            message = note ?: current.message,
        )
        track(next)
        return next
    }

    // -----------------------------------------------------------------------

    private fun track(s: ComfySubmission) {
        submissions[s.promptId] = s
        if (submissions.size > MAX_TRACKED) {
            submissions.values.sortedBy { it.submittedAt }
                .take(submissions.size - MAX_TRACKED)
                .forEach { submissions.remove(it.promptId) }
        }
    }

    private fun parseInstant(iso: String): Long =
        runCatching { Instant.parse(iso).toEpochMilli() }.getOrDefault(System.currentTimeMillis())

    /** `POST /prompt`：把节点图放进队列，拿回 prompt_id。 */
    private fun postPrompt(base: String, graph: JsonObject, clientId: String): String {
        val payload = AppJson.encodeToString(
            JsonElement.serializer(),
            buildJsonObject {
                put("prompt", graph)
                put("client_id", clientId)
            },
        )
        val req = HttpRequest.newBuilder(URI.create("$base/prompt"))
            .timeout(Duration.ofSeconds(30))
            .header("Content-Type", "application/json")
            .POST(HttpRequest.BodyPublishers.ofString(payload, StandardCharsets.UTF_8))
            .build()
        val res = http.send(req, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8))
        val body = res.body().orEmpty()
        if (res.statusCode() !in 200..299) {
            // ComfyUI 的 400 会说明哪个节点参数不对，这段信息对用户很有用
            throw IllegalStateException("ComfyUI 拒绝了这次提交（HTTP ${res.statusCode()}）：${body.take(600)}")
        }
        val root = runCatching { AppJson.parseToJsonElement(body) as? JsonObject }.getOrNull()
            ?: throw IllegalStateException("ComfyUI 返回了无法解析的内容：${body.take(300)}")
        return (root["prompt_id"] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
            ?: throw IllegalStateException("ComfyUI 没有返回 prompt_id：${body.take(300)}")
    }

    /** 查这一次运行在 `/history` 里的条目（还没有就返回 null）。 */
    private fun historyEntry(base: String, promptId: String): JsonObject? =
        runCatching {
            val obj = fetchJson("$base/history/$promptId") as? JsonObject ?: return null
            obj[promptId] as? JsonObject
        }.getOrNull()

    /** 队列里的一次运行在什么位置（给界面一句人话）。 */
    private fun queueNote(base: String, promptId: String): String? = runCatching {
        val obj = fetchJson("$base/queue") as? JsonObject ?: return null
        val running = obj["queue_running"] as? JsonArray
        val pending = obj["queue_pending"] as? JsonArray
        if (indexOfPromptId(running, promptId) >= 0) return "正在生成…"
        val index = indexOfPromptId(pending, promptId)
        if (index >= 0) return "排队中（前面还有 $index 个任务）"
        val total = (pending?.size ?: 0) + (running?.size ?: 0)
        if (total > 0) "队列里还有 $total 个任务" else null
    }.getOrNull()

    /** 队列项是 `[number, prompt_id, graph, extra, outputs]`，按内容找 prompt_id。 */
    private fun indexOfPromptId(arr: JsonArray?, promptId: String): Int {
        arr ?: return -1
        arr.forEachIndexed { i, item ->
            val row = item as? JsonArray ?: return@forEachIndexed
            if (row.any { (it as? JsonPrimitive)?.contentOrNull == promptId }) return i
        }
        return -1
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

    // -----------------------------------------------------------------------
    //  工作流改写（提交前的参数覆盖）
    // -----------------------------------------------------------------------

    /**
     * 把一处参数覆盖写进 API 格式节点图。
     *
     * `overrides` 的键是 `节点id.输入名`（例如 `3.steps`、`6.text`），与工作流里的
     * 节点编号一一对应 —— ComfyUI 界面上每个节点的标题里就写着这个编号，用户能自己核对。
     *
     * 三条规矩：
     *  1. **只在类型兼容时改写**：原来存的是数字就不能塞字符串（ComfyUI 会当场 400，
     *     而用户看到的是一句莫名其妙的报错），所以按原值类型转换；
     *  2. 转换不了 / 路径不存在 → **抛错**，让模型自己纠正（绝不静默忽略）；
     *  3. 返回实际改掉的清单，工具如实告诉用户"改了哪几个参数"。
     */
    object WorkflowEdit {

        fun applyOverrides(
            graph: JsonObject,
            overrides: Map<String, JsonElement>,
        ): Pair<JsonObject, List<String>> {
            val applied = mutableListOf<String>()
            val nodes: MutableMap<String, JsonElement> = LinkedHashMap(graph)
            overrides.forEach { (path, value) ->
                val dot = path.lastIndexOf('.')
                require(dot > 0 && dot < path.length - 1) {
                    "参数路径要写成 节点id.输入名（例如 3.steps），收到的是「$path」"
                }
                val nodeId = path.substring(0, dot)
                val field = path.substring(dot + 1)
                val node = nodes[nodeId] as? JsonObject
                    ?: throw IllegalArgumentException("工作流里没有节点 $nodeId（参数路径 $path）")
                val inputs = (node["inputs"] as? JsonObject)?.toMutableMap()
                    ?: throw IllegalArgumentException("节点 $nodeId 没有 inputs（参数路径 $path）")
                // 只允许覆盖**这个节点本来就有的输入**：凭空加一个字段 ComfyUI 会当未知参数拒掉，
                // 报错却只会说"节点参数不对"，用户根本查不出来
                if (!inputs.containsKey(field)) {
                    throw IllegalArgumentException(
                        "节点 $nodeId 没有名为「$field」的输入（现有：${inputs.keys.joinToString("、").take(200)}）"
                    )
                }
                inputs[field] = coerce(inputs[field], value, path)
                nodes[nodeId] = JsonObject(node + mapOf("inputs" to JsonObject(inputs)))
                applied += "$path ← ${(value as? JsonPrimitive)?.contentOrNull ?: value}"
            }
            return JsonObject(nodes) to applied
        }

        /** 按原值类型转换；原来没有这个字段时按值的字面量类型落下去。 */
        private fun coerce(previous: JsonElement?, value: JsonElement, path: String): JsonElement {
            val text = (value as? JsonPrimitive)?.contentOrNull
                ?: throw IllegalArgumentException("参数 $path 只支持标量（数字 / 文本 / 布尔）")
            if (!value.isString) return value
            return when (previous) {
                is JsonPrimitive -> when {
                    previous.isString -> JsonPrimitive(text)
                    previous.intOrNull != null -> text.toLongOrNull()?.let { JsonPrimitive(it) }
                        ?: throw IllegalArgumentException("参数 $path 原来是整数，给的值「$text」不是整数")
                    previous.contentOrNull?.toDoubleOrNull() != null ->
                        text.toDoubleOrNull()?.let { JsonPrimitive(it) }
                            ?: throw IllegalArgumentException("参数 $path 原来是数字，给的值「$text」不是数字")
                    previous.contentOrNull?.toBooleanStrictOrNull() != null ->
                        text.toBooleanStrictOrNull()?.let { JsonPrimitive(it) }
                            ?: throw IllegalArgumentException("参数 $path 原来是布尔，给的值「$text」不是布尔")
                    else -> JsonPrimitive(text)
                }
                // 原来没有这个字段：数字就当数字，其余当文本
                null -> text.toLongOrNull()?.let { JsonPrimitive(it) }
                    ?: text.toDoubleOrNull()?.let { JsonPrimitive(it) }
                    ?: JsonPrimitive(text)
                else -> throw IllegalArgumentException("参数 $path 不是标量，不能覆盖")
            }
        }
    }
}
