package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.ai.CredentialService
import com.comfyhub.ai.OutboundProxy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
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
 * [ToolRegistry] 依赖的**窄接口**：工具层只认"搜一次 Civitai"这一个动作。
 *
 * 与 [WebSearchClient] 同一个理由：ToolRegistryTest 要能塞一个**不起 HTTP 的假实现**
 * 去钉"NSFW 必须显式打开 / 没有令牌要说清楚"这些语义；
 * 真正打 HTTP 的部分（域名切换、鉴权头、错误映射）由 CivitaiSearchTest 拿本机假服务钉死。
 */
interface CivitaiClient {
    /**
     * 搜一次 Civitai 的模型 / LoRA 库。
     *
     * [allowNsfw] = true 表示调用方（模型）认为这一问需要成人内容：
     * 那会走 `.red` 域名并要求令牌，缺令牌时**报错说清楚**，不静默返回过滤后的结果。
     */
    suspend fun search(
        query: String,
        allowNsfw: Boolean = false,
        type: String? = null,
        limit: Int = 8,
    ): CivitaiOutcome
}

/**
 * Civitai 模型库搜索（用户建议 ④/⑦，2026-09-20）。
 *
 * ## 用户的原话与这条实现怎么对上
 *
 * > "设计一个工具，允许 AI 搜索 civitai（注：civitai 如要搜索 NSFW 内容需要登录或者令牌，
 * >  且域名变成 .red；这种情况下可以询问用户，如允许则模拟用户真机搜索）"
 * >
 * > "或许可以让 ai 联网搜索 comfy 库里各种模型信息（civitai，或者 hugging face 等等），
 * >  贴上简短的说明（比如它拿来做什么），登记在数据库中"
 *
 * 所以这里做三件事：
 *  1. **普通（安全）搜索**走 `civitai.com/api/v1/models`，免令牌；
 *  2. **NSFW 搜索**走 `civitai.red`，**必须**有令牌（[CREDENTIAL_REF]），
 *     且工具侧默认 `ASK` —— 也就是"这种情况下可以询问用户"；
 *     缺令牌时抛 `CIVITAI_TOKEN_REQUIRED` 并告诉用户去哪儿填，**不许静默降级**成安全搜索
 *     （那样模型会拿到一份被过滤过的结果却以为看到了全部）。
 *  3. 每条结果都带上**这个模型是干什么的**：类型 + 基础模型 + 触发词 + 简介，
 *     正是"贴上简短的说明，登记在数据库中"要的那份东西 ——
 *     模型拿到之后可以自己调 `remember` 落进长期记忆。
 *
 * ## 出网纪律（与 [KeenableSearch] 同规矩）
 *
 *  · 只打两个**写死**的域名（`civitai.com` / `civitai.red`），不接受任意 Host；
 *  · **绝不记录 / 回显令牌**：日志只写状态码与稳定错误码；
 *  · 返回的正文一律当**不可信数据**（工具侧会套 [untrustedBanner]）。
 *
 * ## 为什么域名要真的切到 `.red`
 *
 * 用户的要求是"模拟用户真机搜索"：NSFW 走成人站镜像、把 `nsfw=true` 明明白白写进查询串。
 * `.red` 是同一套 API 的成人站镜像，这条照做。
 *
 * ## ⚠ 与线上实测不一致的假设（2026-09-20 在本机实打 civitai.com / civitai.red 核对过）
 *
 * 用户原话里的前提是"NSFW 需要登录/令牌，且域名变成 `.red`"。实测 `/api/v1/models`
 * 这条**搜索**接口并不是这样：
 *
 *  · `https://civitai.com/api/v1/models?query=breasts&nsfw=true` **匿名**就返回
 *    `nsfw=true / nsfwLevel=60` 的条目（`id=7328 breasts_on_glass`），和 `.red` 上
 *    同一查询的结果**逐条一致**；
 *  · `.red` 匿名同样 200，要不要令牌都一样；
 *  · 反过来，一个**假的** `Authorization: Bearer` 也不会被拒（两个域名都照样 200）——
 *    也就是说这条接口根本不校验令牌。
 *
 * 所以本类里的"NSFW 必须有令牌"是**用户定的策略**（表达"这是登录态才做的事"，
 * 并且它天然带一次审批），**不是**上游的要求。这个区别要留着：
 * 别在注释、文档或给模型的提示里把它说成"不给令牌就搜不到" —— 那是**假话**，
 * 而模型会据此对用户撒谎（"需要配置令牌"）或者干脆放弃搜索。
 * 上游真正会 400 的是别的东西：`types` 大小写敏感、`limit` 上限 100（见下）。
 */
class CivitaiSearch(
    private val credentials: CredentialService,
    /** 安全搜索根地址；可用环境变量覆盖（本机假服务的口子，与 KeenableSearch 一致）。 */
    safeBaseUrl: String = defaultSafeBaseUrl(),
    /** NSFW 搜索根地址（成人站镜像，同样可覆盖）。 */
    nsfwBaseUrl: String = defaultNsfwBaseUrl(),
    private val timeoutSeconds: Long = DEFAULT_TIMEOUT_SECONDS,
) : CivitaiClient {
    private val log = LoggerFactory.getLogger(CivitaiSearch::class.java)

    private val safeBase = safeBaseUrl.trim().trimEnd('/')
    private val nsfwBase = nsfwBaseUrl.trim().trimEnd('/')

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(CONNECT_TIMEOUT_SECONDS))
        // 与 KeenableSearch 同规矩：不跟随重定向（目标是我们写死的域名，没必要被带走）
        .followRedirects(HttpClient.Redirect.NEVER)
        // 本机的代理设置要认（JVM 默认一律直连 —— 用户报的"反复超时"就是这个，见 OutboundProxy）
        .proxy(OutboundProxy.selector())
        .build()

    companion object {
        const val SAFE_BASE_URL = "https://civitai.com"
        const val NSFW_BASE_URL = "https://civitai.red"

        /**
         * 令牌在 [CredentialService] 里的引用名。
         *
         * 与 `keenable` 同一约定：受管凭据库（设置页里填）或环境变量
         * `COMFYHUB_CRED_civitai`。用户也可以直接给 `CIVITAI_API_KEY` 环境变量 ——
         * 那是 Civitai 生态里通行的名字，认它不会造成"状态说谎"
         * （`describe()` 走的仍是 [CredentialService]，界面显示的是这一路的真值）。
         */
        const val CREDENTIAL_REF = "civitai"

        /** Civitai 生态里通行的环境变量名（用户手边多半已经有一个）。 */
        const val ENV_TOKEN = "CIVITAI_API_KEY"

        /**
         * 连接超时（秒）。
         *
         * 单独拎出来是因为**错误信息必须说实话**：连接超时与"连上了但回得慢"是两回事，
         * 以前两者都报"超时（25 秒）"，用户按"稍后再试"永远好不了（实际是 5 秒连不上）。
         */
        const val CONNECT_TIMEOUT_SECONDS = 5L

        const val DEFAULT_TIMEOUT_SECONDS = 25L
        const val MAX_TIMEOUT_SECONDS = 60L

        /** 一次最多回几条（再多也没人看，还白烧 token）。 */
        const val MAX_LIMIT = 20

        /**
         * 上游自己允许的 `limit` 上限（2026-09-20 实测：`limit=100` 200、`limit=101` 400）。
         *
         * 记在这里是为了让 [MAX_LIMIT] 看起来是"我们自己的取舍"而不是"上游只给这么多"——
         * 哪天有人想放宽到 50，不必再去试探一遍线上。
         */
        const val UPSTREAM_MAX_LIMIT = 100

        /** 每条简介截断到多少字符 —— 用户要的是"简短的说明"，不是整页 README。 */
        const val MAX_DESCRIPTION_CHARS = 600

        /**
         * 允许的模型类型（Civitai 的 `types` 参数取值）。
         *
         * **线上大小写敏感**（实测：`LORA` 200，`lora` / `Lora` 400），所以这里的拼写
         * 就是官方拼写，[search] 会先把调用方给的值归一成它。[TYPES] 里每一个值都实测过 200。
         */
        val TYPES = setOf("Checkpoint", "LORA", "TextualInversion", "Controlnet", "Upscaler", "VAE", "LoCon", "DoRA")

        private fun defaultSafeBaseUrl(): String =
            System.getenv("CIVITAI_BASE_URL")?.takeIf { it.isNotBlank() } ?: SAFE_BASE_URL

        private fun defaultNsfwBaseUrl(): String =
            System.getenv("CIVITAI_NSFW_BASE_URL")?.takeIf { it.isNotBlank() } ?: NSFW_BASE_URL
    }

    /** 有没有配置令牌（工具描述与错误提示都靠它说实话）。 */
    fun hasToken(): Boolean = token() != null

    private fun token(): String? {
        // 两路都认：受管凭据库优先（界面里填的那份），环境变量兜底
        runCatching { credentials.resolve(CREDENTIAL_REF) }.getOrNull()
            ?.takeIf { it.isNotBlank() }
            ?.let { return it }
        return System.getenv(ENV_TOKEN)?.takeIf { it.isNotBlank() }
    }

    override suspend fun search(
        query: String,
        allowNsfw: Boolean,
        type: String?,
        limit: Int,
    ): CivitaiOutcome {
        val q = query.trim()
        if (q.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "缺少搜索词 query")

        val token = token()
        // NSFW 档：走成人站 `.red`，**有没有令牌都照搜**。
        //
        // 2026-09-20 之前的写法是"没令牌当场抛 CIVITAI_TOKEN_REQUIRED、一个请求都不发"，
        // 依据是"`.red` 只认登录态"。真机核对把这个前提推翻了：`.red` 匿名 200
        // （和 `.com?nsfw=true` 的结果逐条一致，见类注释）。所以那条硬门槛等于
        // "明明能搜，工具却说搜不了" —— 用户 2026-09-20 选择放宽成现在这样：
        //   · 显式 `allowNsfw=true` 仍然必须（工具默认 ASK，用户点过头才走到这里）；
        //   · 配了令牌就带上（登录态更稳，也符合"模拟用户真机搜索"的本意）；
        //   · 没配也不拒绝，但**必须如实说明这次是匿名通道**（[render] 里那行提示），
        //     不能让模型以为"没配令牌也没关系"而去建议用户省掉这一步。
        if (allowNsfw && token == null) {
            log.info("Civitai NSFW 搜索走匿名通道（没配令牌）")
        }

        // `types` 线上是**大小写敏感的枚举**：实测 `LORA` 200，而 `lora` / `Lora` 都是 400。
        // 模型把类型写成小写是很自然的（工具描述里也有 "LORA" 之外的小写词），
        // 所以这里先归一成官方拼写；真不认识的值当场报清楚，
        // 别让它变成一句"上游 400"——模型看不出自己哪里写错了，只会反复试。
        val typeFilter = type?.takeIf { it.isNotBlank() }?.trim()?.let { raw ->
            TYPES.firstOrNull { it.equals(raw, ignoreCase = true) }
                ?: throw ToolFailure(
                    "INVALID_ARGUMENT",
                    "不认识的模型类型 \"$raw\"；可选：${TYPES.joinToString(" / ")}",
                )
        }

        val base = if (allowNsfw) nsfwBase else safeBase
        val url = buildString {
            append(base).append("/api/v1/models?")
            append("query=").append(enc(q))
            append("&limit=").append(limit.coerceIn(1, MAX_LIMIT))
            // ⚠️ 这里**必须**过 enc：`Most Downloaded` 里有个空格，
            // 直接拼进 URI 会抛 `URISyntaxException: Illegal character in query`
            // （本机测试当场抓到过一次 —— 拼 URL 的每一段都得当成"用户输入"对待）
            append("&sort=").append(enc("Most Downloaded"))
            append("&period=AllTime")
            typeFilter?.let { append("&types=").append(enc(it)) }
            // 走 .red 时把 nsfw 明写进查询串：这正是"模拟用户真机搜索"要做的那一步
            if (allowNsfw) append("&nsfw=true")
        }

        val body = get(url, token)
        val items = body["items"]?.let { el ->
            runCatching { el.jsonArray }.getOrNull().orEmpty()
        }.orEmpty()

        val models = items.mapNotNull { item ->
            val obj = runCatching { item.jsonObject }.getOrNull() ?: return@mapNotNull null
            toModel(obj)
        }

        return CivitaiOutcome(
            query = q,
            models = models,
            context = render(models, q, allowNsfw, tokenless = token == null),
            meta = buildJsonObject {
                put("count", models.size)
                put("query", q)
                put("nsfw", allowNsfw)
                put("host", URI(base).host)
                put("tokenless", token == null)
            },
        )
    }

    // -----------------------------------------------------------------------
    //  取一条模型 / LoRA 的"这是什么、拿来干什么"
    // -----------------------------------------------------------------------

    private fun toModel(obj: JsonObject): CivitaiModel? {
        val id = obj["id"]?.jsonPrimitive?.intOrNull ?: return null
        val name = obj.str("name").orEmpty().ifBlank { "#$id" }
        val type = obj.str("type").orEmpty()
        // 第一个版本里第一个文件就是"主文件"，它的 baseModel 才是"这份模型跑在哪个底模上"——
        // 顶层没有 baseModel 字段，这一步不取的话界面上就是一片空白。
        //
        // 字段名与线上核对过（2026-09-20，`GET /api/v1/models?query=anime`）：
        // `items[].modelVersions[0].baseModel` = `"SD 1.5"`，与本条顶层的
        // `baseModels: ["SD 1.5"]` 一致 —— 后者是**数组**（一个模型可以跨多个底模），
        // 所以只在 `modelVersions` 缺失/为空时拿它兜底，别拿它当主来源。
        val firstVersion = obj["modelVersions"]?.let { el ->
            runCatching { el.jsonArray }.getOrNull()?.firstOrNull()
        }?.let { runCatching { it.jsonObject }.getOrNull() }
        val baseModel = firstVersion?.str("baseModel").orEmpty().ifBlank {
            obj["baseModels"]?.let { el -> runCatching { el.jsonArray }.getOrNull() }
                ?.firstOrNull()
                ?.let { runCatching { it.jsonPrimitive.contentOrNull }.getOrNull() }
                .orEmpty()
        }
        val trainedWords = firstVersion?.get("trainedWords")?.let { el ->
            runCatching { el.jsonArray }.getOrNull().orEmpty().mapNotNull { w ->
                runCatching { w.jsonPrimitive.contentOrNull }.getOrNull()
            }
        }.orEmpty()
        // 体积与下载地址（用户建议 ②，2026-09-22）：线上 `modelVersions[0].files[]` 里
        // `primary=true` 那条就是主文件，带 `sizeKB` 与 `downloadUrl`。
        //
        // 为什么要它：真实的门槛就是**体积**（昨天卡住的那份 checkpoint 一个 13.49 GB），
        // 而"要不要装"这个决定没有体积根本没法做 —— 用户会看到"要下个模型"，
        // 却不知道是 40MB 还是 14GB。
        //
        // 取舍：只报**主文件**，不在服务端做"挑最小的那个量化版"这类判断 ——
        // 那是模型该做的事（它能看到候选清单），代码只把事实摆出来。
        val files = firstVersion?.get("files")?.let { el ->
            runCatching { el.jsonArray }.getOrNull().orEmpty()
        }.orEmpty()
        val primary = files.mapNotNull { runCatching { it.jsonObject }.getOrNull() }
            .firstOrNull { f -> f["primary"]?.jsonPrimitive?.contentOrNull?.toBoolean() == true }
            ?: files.firstOrNull()?.let { runCatching { it.jsonObject }.getOrNull() }
        val sizeKb = primary?.get("sizeKB")?.jsonPrimitive?.let { p ->
            runCatching { p.content.toDouble() }.getOrNull()
        }
        // sha256（P3 `find_model` 要的第二个字段，2026-09-22）：线上形如
        // `files[primary].hashes = {"SHA256":"<64位>","AutoV2":"…"}`，大小写不定（实测就是大写）。
        // 只在 `hashes` 里找 SHA256 —— 别把 `AutoV2`（Civitai 自己的短码）当成 sha256 报出去：
        // 用户拿它去校验盘上那份文件永远对不上。
        val sha256 = primary?.get("hashes")?.let { runCatching { it.jsonObject }.getOrNull() }
        val stats = obj["stats"]?.let { runCatching { it.jsonObject }.getOrNull() }
        return CivitaiModel(
            id = id,
            name = name,
            type = type,
            baseModel = baseModel,
            nsfw = obj["nsfw"]?.jsonPrimitive?.contentOrNull?.toBoolean() ?: false,
            creator = obj["creator"]?.let { runCatching { it.jsonObject }.getOrNull() }?.str("username"),
            downloads = stats?.get("downloadCount")?.jsonPrimitive?.intOrNull ?: 0,
            thumbsUp = stats?.get("thumbsUpCount")?.jsonPrimitive?.intOrNull ?: 0,
            // 链接给**正式站**，不是本次请求用的 base：base 可以被环境变量指到本机假服务 /
            // 自建代理（测试就是这么用的），把那种地址写进给用户的链接是错的
            pageUrl = "$SAFE_BASE_URL/models/$id",
            description = obj.str("description").orEmpty()
                .replace(Regex("<[^>]+>"), " ")
                .replace(Regex("\\s+"), " ")
                .trim()
                .take(MAX_DESCRIPTION_CHARS),
            trainedWords = trainedWords,
            sizeBytes = sizeKb?.let { (it * 1024).toLong() },
            downloadUrl = primary?.str("downloadUrl")?.takeIf { it.isNotBlank() },
            fileName = primary?.str("name")?.takeIf { it.isNotBlank() },
            sha256 = primarySha256(sha256),
        )
    }

    /**
     * 从 `files[primary].hashes` 里取 SHA256。
     *
     * 键名大小写不敏感（线上给的是 `SHA256`，别的实现可能写 `sha256`）；
     * **只认 64 位十六进制**，别把 `AutoV2` 那种短码当 sha256 ——
     * 用户拿短码去校验盘上的文件永远对不上，那比"不知道"更糟。
     */
    private fun primarySha256(hashes: JsonObject?): String? {
        val raw = hashes?.entries
            ?.firstOrNull { it.key.equals("SHA256", ignoreCase = true) }
            ?.value?.jsonPrimitive?.contentOrNull
            ?: return null
        val v = raw.trim()
        return v.takeIf { it.length == 64 && it.all { c -> c.isDigit() || c in 'a'..'f' || c in 'A'..'F' } }
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    private suspend fun get(url: String, token: String?): JsonObject {
        // URL 拼错了（空格、中文没编码…）不该变成一个裸的 URISyntaxException 冒到 Run 外面：
        // 那就是一次"工具失败"，按稳定错误码如实报给模型
        val uri = runCatching { URI(url) }.getOrElse { e ->
            log.warn("Civitai 搜索 URL 不合法: {}", e::class.simpleName)
            throw ToolFailure("CIVITAI_FAILED", "搜索地址拼装失败（${e::class.simpleName}）；这是本机的 bug，请如实告诉用户")
        }
        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(timeoutSeconds.coerceIn(1, MAX_TIMEOUT_SECONDS)))
            .header("Accept", "application/json")
            .GET()
        // 日志与错误里**只出现状态码**，令牌一个字符都不落
        if (token != null) builder.header("Authorization", "Bearer $token")

        val response = withContext(Dispatchers.IO) {
            runCatching { client.send(builder.build(), HttpResponse.BodyHandlers.ofString()) }
        }.getOrElse { e ->
            val host = runCatching { URI(url).host }.getOrNull() ?: "civitai"
            // 路由也写进错误里（"直连"还是"走代理 127.0.0.1:7890"）：用户看不到日志，
            // 但这一行就能让人判断是"本机出不去"还是"对方的问题"。
            // 顺带修一个**说谎的提示**：连接超时（HttpConnectTimeoutException）以前被
            // 归到"超时（25 秒）"里，其实是 5 秒内连都连不上 —— 用户按"稍后再试"永远好不了。
            throw when (e) {
                is java.net.http.HttpConnectTimeoutException ->
                    ToolFailure(
                        "CIVITAI_TIMEOUT",
                        "连不上 Civitai：$host 在 $CONNECT_TIMEOUT_SECONDS 秒内没连上。" +
                            "本次出网路径是「${OutboundProxy.describe(host)}」。" +
                            "如果本机需要代理才能访问 civitai.com，请确认系统代理可用（或设环境变量 " +
                            "${OutboundProxy.ENV_PROXY}，写 direct 表示强制直连）；" +
                            "**不要**反复重试同一个查询，如实告诉用户「本机连不上 Civitai」。",
                    )
                is java.net.http.HttpTimeoutException ->
                    ToolFailure(
                        "CIVITAI_TIMEOUT",
                        "搜 Civitai 超时（连上了但 $timeoutSeconds 秒没回完），稍后再试；" +
                            "本次出网路径是「${OutboundProxy.describe(host)}」",
                    )
                else ->
                    ToolFailure(
                        "CIVITAI_FAILED",
                        "连不上 Civitai（${e::class.simpleName}）；本机需要能访问 $host" +
                            "（本次出网路径：${OutboundProxy.describe(host)}）",
                    )
            }
        }

        val status = response.statusCode()
        val text = response.body().orEmpty()
        if (status !in 200..299) {
            val (code, hint) = when (status) {
                401, 403 ->
                    "CIVITAI_AUTH" to
                        "Civitai 拒绝了令牌；如果开的是 NSFW 搜索，请让用户重新填一个有效的 Civitai API Key"
                429 -> "CIVITAI_RATE_LIMIT" to "Civitai 限流了（429）；等一下再试，别连着搜"
                in 500..599 -> "CIVITAI_FAILED" to "Civitai 暂时不可用（$status）"
                else -> "CIVITAI_FAILED" to "Civitai 返回 $status"
            }
            log.warn("Civitai 搜索失败: status={} code={}", status, code)
            throw ToolFailure(code, hint)
        }

        return runCatching { AppJson.parseToJsonElement(text).jsonObject }.getOrElse {
            throw ToolFailure("CIVITAI_FAILED", "Civitai 返回了无法解析的内容")
        }
    }

    private fun enc(s: String): String = URLEncoder.encode(s, StandardCharsets.UTF_8)

    // -----------------------------------------------------------------------
    //  渲染
    // -----------------------------------------------------------------------

    /**
     * 把结果渲染成"条目 + 一句话说明"的块。
     *
     * 用户要的是"**简短的说明**（比如它拿来做什么）"，所以每条固定四行：
     * 名字/类型/底模 → 干什么用（简介首句）→ 触发词 → 链接与热度。
     * 宁可每条短一点，也不要让两三行占满上下文。
     *
     * [tokenless] 且 [nsfw] 时在抬头补一句"本次是匿名通道"：这不是给用户看的装饰，
     * 而是让模型知道**这次没带登录态**（`.red` 匿名能出结果，但拿到的可能比登录态少），
     * 免得它把"匿名可见"当成"这就是全部"。
     */
    private fun render(
        models: List<CivitaiModel>,
        query: String,
        nsfw: Boolean,
        tokenless: Boolean = false,
    ): String {
        if (models.isEmpty()) return ""
        val head = buildString {
            append("Civitai 搜索「").append(query).append("」")
            if (nsfw) append("（NSFW 通道）")
            append("：").append(models.size).append(" 条")
            if (nsfw && tokenless) {
                append("\n（本次没带 Civitai 令牌，走的是匿名通道 —— 结果照常可用；")
                append("要更稳的登录态可以提醒用户去「设置 → AI 工具 / 凭据」填一个，但不必停下来等。）")
            }
        }
        val blocks = models.mapIndexed { i, m ->
            buildString {
                append('[').append(i + 1).append("] ").append(m.name)
                append("（").append(m.type.ifBlank { "未知类型" })
                if (m.baseModel.isNotBlank()) append(" · 底模 ").append(m.baseModel)
                append('）')
                if (m.nsfw) append(" [NSFW]")
                append('\n')
                append("用途：").append(m.description.ifBlank { "（作者没写简介）" })
                append('\n')
                if (m.trainedWords.isNotEmpty()) {
                    append("触发词：").append(m.trainedWords.joinToString(", "))
                    append('\n')
                }
                // 体积（用户建议 ②，2026-09-22）：没有它，"要不要装"这个选择做不了 ——
                // 用户看到"要下个模型"却不知道是 40MB 还是 14GB（昨天真实卡住的那份是 13.49GB）
                m.sizeBytes?.let { append("体积：").append(m.sizeLabel()).append('\n') }
                append(m.pageUrl)
                append("（下载 ").append(m.downloads).append(" · 赞 ").append(m.thumbsUp).append('）')
                m.downloadUrl?.let { append("\n直链：").append(it) }
            }
        }
        return head + "\n\n" + blocks.joinToString("\n\n")
    }
}

/** 一条 Civitai 模型 / LoRA（只留"用得上"的那几个字段）。 */
data class CivitaiModel(
    val id: Int,
    val name: String,
    val type: String,
    val baseModel: String,
    val nsfw: Boolean,
    val creator: String?,
    val downloads: Int,
    val thumbsUp: Int,
    val pageUrl: String,
    /** 去掉 HTML 标签、压成一行、截断后的简介 —— 就是"它拿来做什么"那句话。 */
    val description: String,
    val trainedWords: List<String>,
    /**
     * 主文件的字节数（用户建议 ②，2026-09-22）。
     *
     * 来自线上 `modelVersions[0].files[primary].sizeKB`；**拿不到就是 null**
     * （老版本 / 结构变了 / 假服务）—— 那时界面与正文都不写这一行，
     * 绝不用一个猜出来的数字去撑场面（用户会按这个数字决定要不要下 14GB）。
     */
    val sizeBytes: Long? = null,
    /** 主文件直链（有就给出来，方便用户自己下或让脚本下）。 */
    val downloadUrl: String? = null,
    /** 主文件名（上游 `files[primary].name`；拿不到就是 null）。 */
    val fileName: String? = null,
    /**
     * 主文件的 **sha256**（上游 `files[primary].hashes.SHA256`，P3 `find_model` 要的）。
     *
     * 用途不是"好看"：用户手动下完之后，拿它比对盘上那份文件，判断"是不是同一个模型"。
     * 今天 `ModelName.similarity` 只是**字面**相似（`sd_xl_base_1.0` 与 `sd_xl_refiner_1.0`
     * 能到 0.63 分却是两份不同的模型），所以字面相似不足以确认身份，sha256 才是。
     * 上游没给就是 null —— **不编**。
     */
    val sha256: String? = null,
) {
    /**
     * 给人看的体积标签。
     *
     * 单位分档是有意的：LoRA 常见 100~300MB，用 GB 显示会变成 "0.1GB"（看不出差别）；
     * 而 checkpoints 动辄 6~14GB，用 MB 显示就是 "13490MB"（数不清有几个零）。
     * **拿不到体积时返回"未知（作者没给）"**，不是空串 —— 空着会让模型自己编一个。
     */
    fun sizeLabel(): String = sizeBytes?.let {
        val gb = it / 1024.0 / 1024.0 / 1024.0
        if (gb >= 1.0) "%.2f GB".format(gb) else "%.0f MB".format(it / 1024.0 / 1024.0)
    } ?: "未知（作者没给文件体积）"
}

/**
 * 一次 Civitai 搜索的结果。
 *
 * [context] 是**已经渲染好**的、给模型看的块；界面要用就读 [meta] / [models]，
 * 不要去解析 [context] 的文本（与 [WebSearchOutcome] 同约定）。
 */
data class CivitaiOutcome(
    val query: String,
    val models: List<CivitaiModel>,
    val context: String,
    val meta: JsonObject,
) {
    val isEmpty: Boolean get() = models.isEmpty()

    /** 给前端 / 入库用的结构化结果：摘要 + 每条模型的 id / 名字 / 说明。 */
    fun toJson(): JsonObject = buildJsonObject {
        put("query", query)
        put("count", models.size)
        put(
            "models",
            buildJsonArray {
                models.forEach { m ->
                    add(
                        buildJsonObject {
                            put("id", m.id)
                            put("name", m.name)
                            put("type", m.type)
                            put("baseModel", m.baseModel)
                            put("nsfw", m.nsfw)
                            m.creator?.let { put("creator", it) }
                            put("downloads", m.downloads)
                            put("thumbsUp", m.thumbsUp)
                            put("pageUrl", m.pageUrl)
                            put("description", m.description)
                            // 体积（用户建议 ②）：结构化字段给界面与模型都能直接用；
                            // 拿不到就**不写这个键**（比写一个 null / 0 更不容易被误读成"很小"）
                            m.sizeBytes?.let {
                                put("sizeBytes", it)
                                put("sizeLabel", m.sizeLabel())
                            }
                            m.downloadUrl?.let { put("downloadUrl", it) }
                            // sha256（P3）：结构化字段给界面 / 模型都能直接用；
                            // 拿不到就**不写这个键**（比写一个 null 更不容易被当成"校验过了"）
                            m.fileName?.let { put("fileName", it) }
                            m.sha256?.let { put("sha256", it) }
                            if (m.trainedWords.isNotEmpty()) {
                                put(
                                    "trainedWords",
                                    buildJsonArray { m.trainedWords.forEach { add(JsonPrimitive(it)) } },
                                )
                            }
                        },
                    )
                }
            },
        )
    }
}
