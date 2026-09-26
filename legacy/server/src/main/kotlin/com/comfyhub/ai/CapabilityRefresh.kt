package com.comfyhub.ai

import com.comfyhub.AppJson
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import org.slf4j.LoggerFactory
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.time.Duration
import java.time.Instant
import java.time.temporal.ChronoUnit

/**
 * 模型能力目录的**在线刷新**（2026-09-23，用户拍板"全做了"）。
 *
 * ## 数据从哪来
 *
 * [ModelCapabilityCatalog] 的内置表编在 jar 里，发版才能改 —— 新模型层出不穷，不够用。
 * 这里的补充链是（与 2026-09-21 "检测优先，安装只作兜底"同一纪律）：
 *
 *  1. **接口自述**（DISCOVERED）最可信 —— `AiUpstream.parseDeclaredCapabilities` 已经逐维度采信，
 *     不归这里管；
 *  2. **models.dev 公开数据集**（本文件）：`https://models.dev/api.json` 聚合了各家官方文档的
 *     模态 / `tool_call` / `reasoning` / **思考档位**（`reasoning_options` 里的 effort 列表），
 *     更新勤快、免密钥。拉下来规范化成 [ModelCapabilityCatalog.OverlayFile] 写进
 *     `<storage>\ai\capabilities\remote.json`，并让目录当场重读 —— **不用重启、不用发版**；
 *  3. 内置家族规则兜底（模糊匹配 + 方言），remote 不认识的维度落到它。
 *
 * ## 三条纪律
 *
 *  1. **远端数据只是数据**：我们只取几个固定字段，绝不执行其中任何内容（与工具结果同一条纪律）；
 *  2. **失败不阻塞**：出网失败 / 解析失败都只记日志、返回如实的结果，旧文件原样保留 ——
 *     "刷新失败"绝不能把本来能用的目录弄坏；
 *  3. **COMFYHUB_NO_DOWNLOAD=1 一刀关**（与 Java / MCP 运行时那条链同一个开关）。
 */
object CapabilityRefresh {
    private val log = LoggerFactory.getLogger(CapabilityRefresh::class.java)

    const val SOURCE = "models.dev"
    const val URL = "https://models.dev/api.json"

    /** remote.json 比这更旧就认为过期（启动时与后台每小时的巡检用）。 */
    const val REFRESH_INTERVAL_DAYS = 7L

    private const val HTTP_TIMEOUT_SECONDS = 30L
    private const val BACKGROUND_CHECK_PERIOD_MINUTES = 60L

    data class RefreshResult(
        val ok: Boolean,
        /** 主动跳过（NO_DOWNLOAD / 还没过期）也算"没坏"，界面上如实显示原因 */
        val skipped: Boolean,
        val message: String,
        val entryCount: Int = 0,
        val fetchedAt: String? = null,
    )

    // -----------------------------------------------------------------------
    //  刷新
    // -----------------------------------------------------------------------

    fun refresh(dir: Path, force: Boolean): RefreshResult {
        if (noDownload()) {
            return RefreshResult(false, skipped = true, message = "COMFYHUB_NO_DOWNLOAD=1 已关闭在线更新（目录仍用内置表 + 已有 remote.json）")
        }
        if (!force && !isStale(dir)) {
            return RefreshResult(true, skipped = true, message = "remote.json 还没过期（每 $REFRESH_INTERVAL_DAYS 天自动拉一次），要立刻更新请用强制刷新")
        }
        return runCatching { fetchAndWrite(dir) }
            .getOrElse {
                log.warn("能力目录在线刷新失败（保留旧文件）：{}", it.message)
                RefreshResult(false, skipped = false, message = "拉取失败：${it.message ?: it.javaClass.simpleName}（旧目录原样保留）")
            }
    }

    /** 启动巡检用：没过期就不出网。 */
    fun refreshIfNeeded(dir: Path): RefreshResult = refresh(dir, force = false)

    private fun fetchAndWrite(dir: Path): RefreshResult {
        val client = HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(10))
            // 固定 URL（不是模型给的），跟着重定向走没有 SSRF 风险
            .followRedirects(HttpClient.Redirect.NORMAL)
            // 公网出网统一走代理选择器（本机/内网强制直连），见 OutboundProxy
            .proxy(OutboundProxy.selector())
            .build()
        val request = HttpRequest.newBuilder(URI.create(URL))
            .timeout(Duration.ofSeconds(HTTP_TIMEOUT_SECONDS))
            .header("Accept", "application/json")
            .header("User-Agent", "ComfyHub")
            .GET()
            .build()
        val response = client.send(request, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8))
        if (response.statusCode() !in 200..299) {
            return RefreshResult(false, skipped = false, message = "models.dev 返回 HTTP ${response.statusCode()}（旧目录原样保留）")
        }
        val fetchedAt = Instant.now().toString()
        val count = parseAndWrite(response.body(), dir, fetchedAt)
        log.info("能力目录已从 {} 更新：{} 条模型声明 → {}", SOURCE, count, dir.resolve(ModelCapabilityCatalog.REMOTE_FILE))
        return RefreshResult(true, skipped = false, message = "已从 $SOURCE 更新 $count 条模型声明", entryCount = count, fetchedAt = fetchedAt)
    }

    // -----------------------------------------------------------------------
    //  解析 + 落盘（离线可测：body 是注入的，不打网）
    // -----------------------------------------------------------------------

    /**
     * 解析 models.dev 的返回并写 `remote.json`，返回条目数。
     *
     * 结构（实测）：`{ provider: { models: { key: { id, modalities:{input:[…]},
     * tool_call, reasoning, reasoning_options:[{type:"effort",values:[…]}], last_updated } } } }`。
     * 多家 provider 会带同一个模型 —— 撞 id 时取 `last_updated` 更新的那份。
     */
    internal fun parseAndWrite(body: String, dir: Path, fetchedAt: String): Int {
        val root = runCatching { AppJson.parseToJsonElement(body) as? JsonObject }
            .getOrNull() ?: throw IllegalStateException("返回体不是 JSON 对象")
        val providers = root["models"] as? JsonObject ?: root
        val merged = LinkedHashMap<String, Entry>()
        for ((_, providerEl) in providers) {
            val provider = providerEl as? JsonObject ?: continue
            val models = provider["models"] as? JsonObject ?: continue
            for ((key, modelEl) in models) {
                val model = modelEl as? JsonObject ?: continue
                val entry = normalize(key, model) ?: continue
                val existing = merged[entry.id]
                merged[entry.id] = if (existing == null) entry else mergeEntries(entry, existing)
            }
        }
        if (merged.isEmpty()) throw IllegalStateException("数据集里没有解析出任何模型（结构可能变了），旧文件原样保留")

        val overlay = ModelCapabilityCatalog.OverlayFile(
            source = SOURCE,
            version = SOURCE,
            fetchedAt = fetchedAt,
            entries = merged.values.associate { it.id to ModelCapabilityCatalog.OverlayEntry(
                modalities = it.modalities,
                tools = it.tools,
                reasoning = it.reasoning,
                thinkingEfforts = it.efforts,
                // 方言刻意不填：线上数据集没有"方言"这个概念，猜错比不填更糟；
                // 方言由内置家族规则给（glm→zai / qwen→qwen / deepseek→deepseek）
                thinkingFormat = null,
            ) },
        )

        Files.createDirectories(dir)
        val target = dir.resolve(ModelCapabilityCatalog.REMOTE_FILE)
        val tmp = dir.resolve("${ModelCapabilityCatalog.REMOTE_FILE}.tmp")
        Files.writeString(tmp, AppJson.encodeToString(ModelCapabilityCatalog.OverlayFile.serializer(), overlay))
        runCatching { Files.move(tmp, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING) }
            .onFailure { Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING) }
        ModelCapabilityCatalog.reload(dir)
        return merged.size
    }

    private data class Entry(
        val id: String,
        val modalities: List<String>?,
        val tools: Boolean?,
        val reasoning: Boolean?,
        val efforts: Map<String, String>?,
        val lastUpdated: String?,
    )

    /** 一个模型条目 → 我们的覆盖层形状；一个有用维度都没有就返回 null（不占地方）。 */
    private fun normalize(key: String, model: JsonObject): Entry? {
        val id = (model.str("id") ?: key).trim().lowercase()
        if (id.isEmpty() || id.any { it.isISOControl() }) return null

        // modalities.input：["text","image","pdf"] → 我们的词表（pdf→document，与
        // AiUpstream.modalityTokens 同一套映射 —— 别再发明第三套）
        val input = (model["modalities"] as? JsonObject)?.get("input") as? JsonArray
        val modalities = input?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            ?.mapNotNull { token ->
                when (token.trim().lowercase()) {
                    "text", "prompt" -> "text"
                    "image", "vision" -> "image"
                    "video" -> "video"
                    "audio" -> "audio"
                    "pdf", "file", "document" -> "document"
                    else -> null
                }
            }
            ?.distinct()
            ?.takeIf { it.isNotEmpty() }

        val tools = model.bool("tool_call")
        val reasoning0 = model.bool("reasoning")

        // 思考档位：reasoning_options 里 type=="effort" 的 values（线上表达）。
        // 线上值 → 我们的等级键：none→off，其余同名；不认识的值跳过（绝不发明）。
        val efforts = (model["reasoning_options"] as? JsonArray)
            ?.mapNotNull { it as? JsonObject }
            ?.filter { it.str("type") == "effort" }
            ?.flatMap { opt -> (opt["values"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty() }
            ?.mapNotNull { raw ->
                when (raw.trim().lowercase()) {
                    "none" -> "off" to "none"
                    "minimal", "low", "medium", "high", "xhigh", "max" -> raw.trim().lowercase() to raw.trim().lowercase()
                    else -> null
                }
            }
            ?.toMap()
            ?.takeIf { it.isNotEmpty() }
            // reasoning 明确为 false 的条目不许带档位（validateThinkingEfforts 的纪律）
            ?.takeIf { reasoning0 != false }
        // 数据集给了档位却没给 reasoning 布尔：能给档位就说明支持推理（诚实推断，不是发明）
        val reasoning = reasoning0 ?: efforts?.let { true }

        if (modalities == null && tools == null && reasoning == null && efforts == null) return null
        return Entry(id, modalities, tools, reasoning, efforts, model.str("last_updated"))
    }

    /**
     * 多家 provider 带同一个模型：**逐维度合并，`last_updated` 新者优先、旧者补位** ——
     * 新条目没给模态不该把旧条目的模态信息挤掉。合并后 reasoning=false 时不留档位。
     */
    private fun mergeEntries(a: Entry, b: Entry): Entry {
        val (n, o) = if (newerThan(a.lastUpdated, b.lastUpdated)) a to b else b to a
        val reasoning = n.reasoning ?: o.reasoning
        return Entry(
            id = n.id,
            modalities = n.modalities ?: o.modalities,
            tools = n.tools ?: o.tools,
            reasoning = reasoning,
            efforts = (n.efforts ?: o.efforts)?.takeIf { reasoning != false },
            lastUpdated = listOfNotNull(n.lastUpdated, o.lastUpdated).maxOrNull(),
        )
    }

    private fun newerThan(a: String?, b: String?): Boolean {
        if (a == null) return false
        if (b == null) return true
        return a >= b   // ISO-8601 字典序 = 时间序
    }

    private fun JsonObject.str(key: String): String? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

    /** `false` 也是一条**声明**（"明确不支持工具"），只有字段缺失 / 类型不对才是 null。 */
    private fun JsonObject.bool(key: String): Boolean? =
        when ((this[key] as? JsonPrimitive)?.contentOrNull) {
            "true" -> true
            "false" -> false
            else -> null
        }

    // -----------------------------------------------------------------------
    //  过期判定
    // -----------------------------------------------------------------------

    fun remoteFetchedAt(dir: Path): Instant? = runCatching {
        val file = dir.resolve(ModelCapabilityCatalog.REMOTE_FILE)
        if (!Files.isRegularFile(file)) return null
        AppJson.decodeFromString(ModelCapabilityCatalog.OverlayFile.serializer(), Files.readString(file))
            .fetchedAt?.let { runCatching { Instant.parse(it) }.getOrNull() }
    }.getOrNull()

    fun remoteAgeHours(dir: Path): Long? = remoteFetchedAt(dir)?.until(Instant.now(), ChronoUnit.HOURS)

    fun isStale(dir: Path): Boolean {
        val age = remoteAgeHours(dir) ?: return true
        return age > REFRESH_INTERVAL_DAYS * 24
    }

    private fun noDownload(): Boolean =
        System.getenv("COMFYHUB_NO_DOWNLOAD")?.let { it.isNotBlank() && it != "0" } == true

    // -----------------------------------------------------------------------
    //  后台巡检（启动时一次 + 每小时看一眼有没有过期；绝不阻塞启动、绝不抛出）
    // -----------------------------------------------------------------------

    @Volatile private var stopped = false
    private var thread: Thread? = null

    fun startBackground(dir: Path) {
        if (thread != null) return
        stopped = false
        thread = Thread({
            while (!stopped) {
                try {
                    val result = refreshIfNeeded(dir)
                    if (!result.skipped) log.info("能力目录后台刷新：{}", result.message)
                } catch (e: Exception) {
                    log.warn("能力目录后台巡检出异常（忽略，下一轮再试）：{}", e.message)
                }
                try {
                    Thread.sleep(BACKGROUND_CHECK_PERIOD_MINUTES * 60 * 1000)
                } catch (_: InterruptedException) {
                    break
                }
            }
        }, "capability-refresh").apply {
            isDaemon = true
            start()
        }
    }

    fun stopBackground() {
        stopped = true
        thread?.interrupt()
        thread = null
    }
}
