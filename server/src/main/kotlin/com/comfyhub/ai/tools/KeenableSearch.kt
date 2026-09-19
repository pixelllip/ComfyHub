package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.ai.CredentialService
import com.comfyhub.ai.EndpointGuard
import com.comfyhub.ai.EndpointTrust
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.time.Duration

/**
 * [ToolRegistry] 依赖的**窄接口**：工具层只认这两个动作，不认 Keenable 本身。
 *
 * 为什么要抽它：ToolRegistryTest 要能塞一个**不起 HTTP 的假实现**去钉预算 / 审批 /
 * 不可信横幅这些语义；真正打 HTTP 的部分（头、路径、错误映射）由 [KeenableSearchTest]
 * 拿本机假服务单独钉死。两层测试各管各的，谁也不用联网。
 */
interface WebSearchClient {
    suspend fun search(
        query: String,
        site: String? = null,
        publishedAfter: String? = null,
        publishedBefore: String? = null,
        mode: String? = null,
    ): WebSearchOutcome

    suspend fun page(url: String, maxChars: Int = KeenableSearch.DEFAULT_FETCH_CHARS): WebPage
}

/**
 * Keenable 联网搜索客户端（`web_search` / `web_fetch` 的**唯一**出网口）。
 *
 * ## 为什么是 Keenable
 *
 * 用户 2026-09-20 定的规则：**只用通用方案**（不碰各家的原生内置搜索 ——
 * OpenAI 的 `web_search`、Anthropic 的 `web_search_20250305` 那些）。
 * Keenable 是"给 AI Agent 重建的搜索索引"，一次调用就返回排好序的**正文**，
 * 关键在于**默认免密钥**：用户零配置就能用，不必先去申请 key。
 *
 * ## 三条硬约束
 *
 *  1. **免密钥请求必须带 `X-Keenable-Title`**：缺这个头服务端直接 400
 *     （`{"error":"Missing app identifier"}`，实测）。它是"谁在调我"的归因头，
 *     所以填的是我们的应用名，不是密钥。
 *  2. **绝不记录 / 回显密钥**：日志只写状态码与稳定错误码，跟 [com.comfyhub.ai.AiUpstream]
 *     同一套纪律。
 *  3. **不跟随重定向**（`Redirect.NEVER`）：`web_fetch` 的目标是模型给的 URL，
 *     照着重定向走等于让它把请求带去任意地方。
 *
 * ## 免密钥额度
 *
 * 官方口径是 **1000 次/小时**。单机创作工具远远够用，所以出厂不带 key；
 * 撞上 429 时如实报错并提示可以填 key（见 [CREDENTIAL_REF]），**不许静默降级**。
 */
class KeenableSearch(
    private val credentials: CredentialService,
    /**
     * 服务根地址。可用环境变量 `KEENABLE_API_URL` 覆盖 —— 这条**不只是给测试用的**：
     * 官方 SDK 也认这个变量，企业自建/代理的场景同样需要。
     */
    baseUrl: String = defaultBaseUrl(),
    /** 归因用的应用名（`X-Keenable-Title`），见约束 ①。 */
    private val clientSource: String = CLIENT_SOURCE,
    private val timeoutSeconds: Long = DEFAULT_TIMEOUT_SECONDS,
) : WebSearchClient {
    private val log = LoggerFactory.getLogger(KeenableSearch::class.java)

    /** 末尾斜杠一律去掉：否则拼出来是 `//v1/search/public`。 */
    private val base: String = baseUrl.trim().trimEnd('/')

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(5))
        // 约束 ③：模型给的 URL 不能被重定向带跑
        .followRedirects(HttpClient.Redirect.NEVER)
        .build()

    companion object {
        const val DEFAULT_BASE_URL = "https://api.keenable.ai"

        /**
         * 可选密钥在 [CredentialService] 里的引用名。
         *
         * 两种填法：受管凭据库（DPAPI，设置页里填），或环境变量
         * `COMFYHUB_CRED_keenable`（[CredentialService] 对所有引用名统一的环境变量约定）。
         *
         * 注意**不认**官方 SDK 的 `KEENABLE_API_KEY`：密钥入口只留一个，
         * 界面上的"已配置/未配置"状态必须与实际行为一致 —— 多认一路环境变量，
         * 状态就会说谎（显示未配置却在用 key）。
         */
        const val CREDENTIAL_REF = "keenable"

        /** `X-Keenable-Title` 的值：出问题时对方能认出流量来自本项目。 */
        const val CLIENT_SOURCE = "ComfyHub"

        /** 官方免密钥额度，只用于错误提示里的说明文案。 */
        const val KEYLESS_RATE_LIMIT_PER_HOUR = 1000

        const val DEFAULT_TIMEOUT_SECONDS = 25L
        const val MAX_TIMEOUT_SECONDS = 60L

        /**
         * 回给模型的搜索结果**总预算**（字符）。
         *
         * 取官方 `to_context()` 的默认值 12000：它在那边是经过验证的"够模型引用、
         * 又不至于把上下文塞爆"的档位。[ToolRegistry.MAX_RESULT_CHARS] 是最后一道闸，
         * 我们在这里先按预算整块取舍 —— 宁可不给某一条，也不要给半句话。
         */
        const val CONTEXT_MAX_CHARS = 12_000

        /** 每条结果向服务端要的正文上限；总预算由 [CONTEXT_MAX_CHARS] 兜。 */
        const val SNIPPET_MAX_LENGTH = 2_500

        /** 单次 `web_fetch` 返回正文的默认/最大字符数。 */
        const val DEFAULT_FETCH_CHARS = 20_000
        const val MAX_FETCH_CHARS = 60_000

        private fun defaultBaseUrl(): String =
            System.getenv("KEENABLE_API_URL")?.takeIf { it.isNotBlank() } ?: DEFAULT_BASE_URL
    }

    // -----------------------------------------------------------------------
    //  搜索
    // -----------------------------------------------------------------------

    /**
     * 搜一次网。
     *
     * [query] 用**自然语言描述你想要的那一页**（"对比 React 和 Vue 性能的博客"），
     * 不要堆关键词 —— Keenable 的索引是语义的，这条写在官方文档里，也写进了工具描述。
     */
    override suspend fun search(
        query: String,
        site: String?,
        publishedAfter: String?,
        publishedBefore: String?,
        mode: String?,
    ): WebSearchOutcome {
        val payload = buildJsonObject {
            put("query", query)
            put("mode", mode?.takeIf { it.isNotBlank() } ?: "pro")
            site?.takeIf { it.isNotBlank() }?.let { put("site", it) }
            publishedAfter?.takeIf { it.isNotBlank() }?.let { put("published_after", it) }
            publishedBefore?.takeIf { it.isNotBlank() }?.let { put("published_before", it) }
            put("snippet_max_length", SNIPPET_MAX_LENGTH)
        }

        val key = resolveKey()
        val body = request(
            endpoint = "search",
            method = "POST",
            key = key,
            body = AppJson.encodeToString(JsonObject.serializer(), payload),
        )

        val results = body["results"]?.let { el ->
            runCatching { el.jsonArray }.getOrNull().orEmpty().mapNotNull { item ->
                val obj = runCatching { item.jsonObject }.getOrNull() ?: return@mapNotNull null
                WebResult(
                    title = obj.str("title").orEmpty(),
                    url = obj.str("url").orEmpty(),
                    snippet = obj.str("snippet").orEmpty(),
                    publishedAt = obj.str("published_at"),
                )
            }
        }.orEmpty()

        val context = render(results)
        return WebSearchOutcome(
            query = body.str("query") ?: query,
            results = results,
            context = context,
            meta = buildJsonObject {
                put("count", results.size)
                put("query", body.str("query") ?: query)
                put("keyless", key == null)
            },
        )
    }

    // -----------------------------------------------------------------------
    //  取正文
    // -----------------------------------------------------------------------

    /**
     * 取一个 URL 的正文（服务端已去掉样板、转成 markdown）。
     *
     * 目标 URL 由模型给，所以发出去之前**先过 [EndpointGuard]**：内网 / 回环 /
     * 云元数据地址一律拒绝。虽然这一跳只是让 Keenable 去抓，本机并没有真的连过去，
     * 但"拿着第三方服务去探用户内网"本身就不该由我们发起 —— 官方 SDK 也做同样的拦截。
     */
    override suspend fun page(url: String, maxChars: Int): WebPage {
        val target = runCatching { URI(url.trim()) }.getOrNull()
            ?: throw ToolFailure("INVALID_ARGUMENT", "url 无法解析：$url")
        val scheme = target.scheme?.lowercase()
        if (scheme != "http" && scheme != "https") {
            throw ToolFailure("INVALID_ARGUMENT", "url 必须是 http/https 链接")
        }
        val host = target.host
            ?: throw ToolFailure("INVALID_ARGUMENT", "url 里没有主机名：$url")
        guardPublicHost(host)

        val limit = maxChars.coerceIn(1, MAX_FETCH_CHARS)
        val encoded = URLEncoder.encode(url.trim(), StandardCharsets.UTF_8)
        val body = request(
            endpoint = "fetch",
            method = "GET",
            key = resolveKey(),
            queryString = "url=$encoded",
        )

        val content = body.str("content").orEmpty()
        val cut = content.length > limit
        val text = if (cut) content.take(limit) else content
        return WebPage(
            url = body.str("url") ?: url.trim(),
            title = body.str("title").orEmpty(),
            content = text,
            publishedAt = body.str("published_at"),
            truncated = cut,
        )
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    /**
     * 发一次请求并解析 JSON。
     *
     * 免密钥走 `/public` 后缀（带 key 时去掉它），这是官方 SDK 的路径约定：
     * 同一个端点名，两种调用面。
     */
    private suspend fun request(
        endpoint: String,
        method: String,
        key: String?,
        body: String? = null,
        queryString: String? = null,
    ): JsonObject {
        val path = "/v1/$endpoint" + if (key == null) "/public" else ""
        val full = base + path + (queryString?.let { "?$it" } ?: "")

        val uri = runCatching { URI(full) }.getOrNull()
            ?: throw ToolFailure("SEARCH_FAILED", "搜索服务地址无法解析：$base")
        uri.host?.let { guardBaseHost(it) }

        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(timeoutSeconds.coerceIn(1, MAX_TIMEOUT_SECONDS)))
            .header("Accept", "application/json")
            // 约束 ①：免密钥请求没有这个头会被服务端 400 掉
            .header("X-Keenable-Title", clientSource)
        if (key != null) builder.header("X-API-Key", key)
        if (body != null) {
            builder.header("Content-Type", "application/json")
            builder.method(method, HttpRequest.BodyPublishers.ofString(body, StandardCharsets.UTF_8))
        } else {
            builder.method(method, HttpRequest.BodyPublishers.noBody())
        }

        // HttpClient.send 是阻塞的：放到 IO 线程，别占住 Run 的调度线程
        val response = withContext(Dispatchers.IO) {
            runCatching { client.send(builder.build(), HttpResponse.BodyHandlers.ofString()) }
        }.getOrElse { e ->
            throw when (e) {
                is java.net.http.HttpTimeoutException ->
                    ToolFailure("SEARCH_TIMEOUT", "联网搜索超时（${timeoutSeconds} 秒），稍后再试")
                else ->
                    ToolFailure("SEARCH_FAILED", "连不上搜索服务（${e::class.simpleName}）；" +
                        "本机需要能访问 $base")
            }
        }

        val status = response.statusCode()
        val text = response.body().orEmpty()
        if (status !in 200..299) {
            // 约束 ②：只带服务端自己的 message，绝不回显我们发出去的 key
            val detail = detailOf(text)
            val (code, hint) = when (status) {
                401, 403 -> "SEARCH_AUTH" to "密钥被拒绝；请检查设置里的 Keenable 密钥"
                402 -> "SEARCH_QUOTA" to "搜索服务余额不足"
                429 ->
                    if (key == null) {
                        "SEARCH_RATE_LIMIT" to
                            "免密钥模式每小时 $KEYLESS_RATE_LIMIT_PER_HOUR 次，已超出；" +
                                "稍后再试，或在设置里填一个 Keenable 密钥提高额度"
                    } else {
                        "SEARCH_RATE_LIMIT" to "该密钥的搜索额度已用尽（429）；稍后再试"
                    }
                400 -> "SEARCH_BAD_REQUEST" to "请求被拒绝（多数是查询本身不合法）"
                in 500..599 -> "SEARCH_FAILED" to "搜索服务暂时不可用（$status）"
                else -> "SEARCH_FAILED" to "搜索服务返回 $status"
            }
            log.warn("Keenable {} 失败: status={} code={}", endpoint, status, code)
            throw ToolFailure(code, "$hint${if (detail.isEmpty()) "" else "｜返回：$detail"}")
        }

        return runCatching { AppJson.parseToJsonElement(text).jsonObject }.getOrElse {
            log.warn("Keenable {} 返回了非 JSON 正文", endpoint)
            throw ToolFailure("SEARCH_FAILED", "搜索服务返回了无法解析的内容")
        }
    }

    /** 服务端错误正文里的 `error` / `message` / `detail`，截断后用。 */
    private fun detailOf(text: String): String = runCatching {
        val obj = AppJson.parseToJsonElement(text).jsonObject
        listOf("message", "error", "detail")
            .firstNotNullOfOrNull { obj.str(it) }
            .orEmpty()
            .take(300)
    }.getOrDefault(text.take(300))

    /**
     * 解析可选密钥：**每个公开调用只解一次** —— DPAPI 解密要起一个 powershell 子进程，
     * 一次搜索里既算 `keyless` 又发请求就会解两遍，纯属浪费。
     *
     * 解密失败（换过 Windows 用户等）不能被当成"没有 key"：那样会静默降级成免密钥，
     * 用户以为在用 key，实际在裸奔。
     */
    private fun resolveKey(): String? = runCatching { credentials.resolve(CREDENTIAL_REF) }.getOrElse { e ->
        throw ToolFailure(
            "SEARCH_CREDENTIAL",
            "联网搜索的密钥无法解密（${e.message}）；请在设置里重新填写，或移除它以改用免密钥模式",
        )
    }

    // -----------------------------------------------------------------------
    //  渲染：编号可引用的块
    // -----------------------------------------------------------------------

    /**
     * 把结果渲染成"编号 + 标题 + 链接 + 正文"的块。
     *
     * 这个形状**照抄官方 SDK 的 `to_context()`**，两个理由：
     *  1. 编号引用是模型的天性 —— `[1]` 比裸 URL 更容易被正确引用回正文；
     *  2. 它以**整条**为单位取舍，宁可少给一条，也不会给模型半句话。
     */
    private fun render(results: List<WebResult>): String {
        val kept = ArrayList<String>()
        var used = 0
        results.forEach { r ->
            // 编号按**实际保留的条数**走：中间某条超预算被跳过时不能留下 [1][2][4] 这种空洞，
            // 模型引用的是这里的编号，空洞会让"第 3 条"指向不存在的结果
            val header = buildString {
                append('[').append(kept.size + 1).append("] ")
                append(r.title.ifBlank { "(无标题)" })
                append(" (").append(r.url).append(')')
                r.publishedAt?.let { append(" - 发布 ").append(it) }
            }
            val bodyText = r.snippet.replace(Regex("\\s+"), " ").trim()
            val block = if (bodyText.isEmpty()) header else "$header\n$bodyText"
            val cost = block.length + if (kept.isEmpty()) 0 else 2
            if (kept.isNotEmpty() && used + cost > CONTEXT_MAX_CHARS) return@forEach
            kept += block
            used += cost
        }
        return kept.joinToString("\n\n")
    }

    // -----------------------------------------------------------------------
    //  地址校验
    // -----------------------------------------------------------------------

    /** 搜索服务自身地址：按地址推断信任级别（回环是给本机假服务/自建代理留的口子）。 */
    private fun guardBaseHost(host: String) {
        runCatching { EndpointGuard.assertAllowed(host, EndpointTrust.infer(host)) }.getOrElse { e ->
            throw ToolFailure("SEARCH_FAILED", "搜索服务地址不被允许：${e.message}")
        }
    }

    /** `web_fetch` 的目标一律按 PUBLIC 判：回环 / 内网 / 云元数据全部拒绝。 */
    private fun guardPublicHost(host: String) {
        runCatching { EndpointGuard.assertAllowed(host, EndpointTrust.PUBLIC) }.getOrElse {
            throw ToolFailure(
                "FETCH_TARGET_DENIED",
                "拒绝抓取 $host：只允许公开网络地址（内网 / 本机 / 云元数据地址会被拒绝）",
            )
        }
    }
}

/** 一条搜索结果（`snippet` 是页面正文抽取，不是 meta description）。 */
data class WebResult(
    val title: String,
    val url: String,
    val snippet: String,
    val publishedAt: String? = null,
)

/**
 * 一次搜索的结果。
 *
 * [context] 是**已经渲染好**的、给模型看的块；界面要用的话读 [meta] / [results]，
 * 不要去解析 [context] 的文本。
 */
data class WebSearchOutcome(
    val query: String,
    val results: List<WebResult>,
    val context: String,
    val meta: JsonObject,
) {
    val isEmpty: Boolean get() = results.isEmpty()
}

/** 一个被抓取的页面。 */
data class WebPage(
    val url: String,
    val title: String,
    val content: String,
    val publishedAt: String? = null,
    val truncated: Boolean = false,
)

// ---------------------------------------------------------------------------
//  JSON 取值：直接用本包 ToolRegistry.kt 里的 `internal fun JsonObject.str`，
//  这里**不要再写一份** —— 同包同签名会变成 Conflicting overloads（编译期就炸）。
// ---------------------------------------------------------------------------
