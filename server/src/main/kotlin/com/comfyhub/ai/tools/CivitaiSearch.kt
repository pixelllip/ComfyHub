package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.ai.CredentialService
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
 * 用户观察到的现象是对的：`.com` 上匿名请求拿到的是**已被过滤**的结果集，
 * NSFW 模型根本不在里面；`.red` 是同一套 API 的成人站镜像。照用户说的"模拟用户真机搜索"
 * 就是：带令牌、打 `.red`、把 `nsfw=true` 明明白白写进查询串。
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
        .connectTimeout(Duration.ofSeconds(5))
        // 与 KeenableSearch 同规矩：不跟随重定向（目标是我们写死的域名，没必要被带走）
        .followRedirects(HttpClient.Redirect.NEVER)
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

        const val DEFAULT_TIMEOUT_SECONDS = 25L
        const val MAX_TIMEOUT_SECONDS = 60L

        /** 一次最多回几条（再多也没人看，还白烧 token）。 */
        const val MAX_LIMIT = 20

        /** 每条简介截断到多少字符 —— 用户要的是"简短的说明"，不是整页 README。 */
        const val MAX_DESCRIPTION_CHARS = 600

        /** 允许的模型类型（Civitai 的 `types` 参数取值）。 */
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
        // NSFW：用户点过头（工具默认 ASK）之后才可能走到这里，令牌仍然必须有 ——
        // `.red` 上匿名请求拿不到成人结果，与其返回一份空结果让模型瞎猜，
        // 不如当场说清楚"要令牌，去哪儿填"。
        if (allowNsfw && token == null) {
            throw ToolFailure(
                "CIVITAI_TOKEN_REQUIRED",
                "搜索 Civitai 的 NSFW 内容需要令牌（成人站 $nsfwBase 只认登录态）。" +
                    "请让用户在「设置 → AI 工具 / 凭据」里填一个 Civitai API Key" +
                    "（凭据名 $CREDENTIAL_REF），或设置环境变量 $ENV_TOKEN；" +
                    "**不要**改口说\"没搜到\"，也不要凭记忆编造模型信息。",
            )
        }

        val base = if (allowNsfw) nsfwBase else safeBase
        val url = buildString {
            append(base).append("/api/v1/models?")
            append("query=").append(enc(q))
            append("&limit=").append(limit.coerceIn(1, MAX_LIMIT))
            append("&sort=Most Downloaded&period=AllTime")
            type?.takeIf { it.isNotBlank() }?.let { append("&types=").append(enc(it.trim())) }
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
            context = render(models, q, allowNsfw),
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
        // 顶层没有 baseModel 字段，这一步不取的话界面上就是一片空白
        val firstVersion = obj["modelVersions"]?.let { el ->
            runCatching { el.jsonArray }.getOrNull()?.firstOrNull()
        }?.let { runCatching { it.jsonObject }.getOrNull() }
        val baseModel = firstVersion?.str("baseModel").orEmpty()
        val trainedWords = firstVersion?.get("trainedWords")?.let { el ->
            runCatching { el.jsonArray }.getOrNull().orEmpty().mapNotNull { w ->
                runCatching { w.jsonPrimitive.contentOrNull }.getOrNull()
            }
        }.orEmpty()
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
            pageUrl = "$safeBase/models/$id",
            description = obj.str("description").orEmpty()
                .replace(Regex("<[^>]+>"), " ")
                .replace(Regex("\\s+"), " ")
                .trim()
                .take(MAX_DESCRIPTION_CHARS),
            trainedWords = trainedWords,
        )
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    private suspend fun get(url: String, token: String?): JsonObject {
        val builder = HttpRequest.newBuilder(URI(url))
            .timeout(Duration.ofSeconds(timeoutSeconds.coerceIn(1, MAX_TIMEOUT_SECONDS)))
            .header("Accept", "application/json")
            .GET()
        // 日志与错误里**只出现状态码**，令牌一个字符都不落
        if (token != null) builder.header("Authorization", "Bearer $token")

        val response = withContext(Dispatchers.IO) {
            runCatching { client.send(builder.build(), HttpResponse.BodyHandlers.ofString()) }
        }.getOrElse { e ->
            throw when (e) {
                is java.net.http.HttpTimeoutException ->
                    ToolFailure("CIVITAI_TIMEOUT", "搜 Civitai 超时（$timeoutSeconds 秒），稍后再试")
                else ->
                    ToolFailure(
                        "CIVITAI_FAILED",
                        "连不上 Civitai（${e::class.simpleName}）；本机需要能访问 ${URI(url).host}",
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
     */
    private fun render(models: List<CivitaiModel>, query: String, nsfw: Boolean): String {
        if (models.isEmpty()) return ""
        val head = buildString {
            append("Civitai 搜索「").append(query).append("」")
            if (nsfw) append("（NSFW 通道）")
            append("：").append(models.size).append(" 条")
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
                append(m.pageUrl)
                append("（下载 ").append(m.downloads).append(" · 赞 ").append(m.thumbsUp).append('）')
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
)

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
