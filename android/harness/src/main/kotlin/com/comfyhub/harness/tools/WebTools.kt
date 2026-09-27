package com.comfyhub.harness.tools

import com.comfyhub.harness.net.HttpFailure
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.net.URLEncoder
import java.nio.charset.StandardCharsets
import java.util.Locale

/**
 * 手机侧的两支出网工具：`web_search` / `civitai_search`（2026-09-27 补齐 MOBILE 工具面）。
 *
 * ## 行为真源
 *
 * **逐条对照桌面 Rust 的 `server-rs/src/web_tools.rs` 与 `server-rs/src/civitai.rs`**
 * （Kotlin 版 `KeenableSearch.kt` / `CivitaiSearch.kt` 是第二参照）。对齐的是四件事：
 * 端点与查询串形状、参数名与默认值、**错误码**（模型按码分支，不是按那句话）、
 * 以及回给模型的那段正文的措辞。
 *
 * 于是"手机上搜一次"与"桌面上搜一次"给模型的是**同一种东西**，
 * 用户在哪一端问都不该得到两套答案。
 *
 * ## 三条不变的东西
 *
 * 1. **出网走手机已有的那条 HTTP 客户端**（`ToolContext.transport`，与 `web_fetch` 同一个实例）——
 *    不另开 HTTP 栈、不引依赖；
 * 2. **三个出网工具共用一份联网预算**（[WebQueryBudget]，`ToolContext.webQueries`）——
 *    "一次回复最多联网 8 次"这句话在手机上也是**真的拦得住**的；
 * 3. **结果是不可信数据**（红线 R7）：正文前面那段横幅**来自 run-kit**，
 *    这里只把 `{kind}` / `{query}` 替换进去（[ToolResults.untrustedBanner]）。
 *    手机源码里一个字的规则文案都没有。
 *
 * ## 凭据（M6 那条路）
 *
 * key 从 `ToolContext.lookupSecret` 取（平台通道注入的 Android Keystore 实现）。
 * ref 名与桌面**同名**：[WebCredentialRefs.KEENABLE] / [WebCredentialRefs.CIVITAI] ——
 * 界面上"这台手机上填过哪些 Key"读的就是这些名字。**用户要在手机上单独填一次**
 * （两端的密钥互相独立，`docs/mobile-harness-design.md` §8），没填时两支工具都**照搜**：
 * Keenable 走免密钥通道（官方额度每小时 1000 次）、Civitai 走匿名通道并在正文里如实写明。
 *
 * ⚠ **"没配"与"解不开"是两件事**：前者是上面那种正常状态，后者（换过系统用户 /
 * Keystore 钥匙丢了）一律**如实报码**（`SEARCH_CREDENTIAL` / 桌面没有对应的
 * `CIVITAI_CREDENTIAL`）让人去重填 —— 静默降级成"没有 key"会让用户以为自己在用登录态。
 *
 * ## 两处与桌面**有意不同**（都是平台差异，不是口径差异）
 *
 * 1. **没有"出网路径（直连 / 代理）"那句话**：桌面要解释"JVM 不走系统代理"这个坑，
 *    手机走的是系统网络，没有这个概念 —— 照抄会变成一句**假话**；
 * 2. **超时与连不上只能按异常文本分**：桌面的 `reqwest` 能把连接超时 / 读超时 /
 *    其它分别报出来，手机的 `HttpURLConnection` 在 `UrlConnectionTransport`
 *    统一折成一条 `HttpFailure`（消息里带着原始异常名）。所以这里按"消息里有没有
 *    Timeout"分 [SEARCH_TIMEOUT] 与 [SEARCH_FAILED] —— 两者对模型的含义都是
 *    "这次没查成、别硬重试"，**分错了也只是码不同，不会让它以为搜到了**。
 */

/**
 * 出网预算（三个工具**共用一份**）。
 *
 * 数值与桌面 `policy::MAX_WEB_QUERIES_PER_RUN`（= Kotlin
 * `ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN`）**同值**：系统提示里写着
 * "一次回复最多联网 N 次"，那个 N 就是它 —— 手机上不去读桌面的 policy（离线也要能跑），
 * 所以这里是"两处同值"的约定，**改一处要同时改另一处**。
 */
object WebQueryBudget {
    const val MAX_QUERIES_PER_RUN = 8

    /** 超限时回给模型的话（与桌面执行器 `QUERY_BUDGET_EXCEEDED` 同义）。 */
    fun exceededMessage(limit: Int): String =
        "本次回复联网查询的次数已达上限（$limit 次）；" +
            "请基于已有的搜索结果作答，需要更多资料时如实告诉用户"
}

/** 出网工具的凭据 ref 名（与桌面 `credentials::KEENABLE_REF` / `civitai::CREDENTIAL_REF` 同名）。 */
object WebCredentialRefs {
    const val KEENABLE = "keenable"
    const val CIVITAI = "civitai"
}

// ---------------------------------------------------------------------------
//  web_search（Keenable 通用搜索）
// ---------------------------------------------------------------------------

/**
 * `web_search`：搜一次网。
 *
 * 为什么是 Keenable：用户 2026-09-20 定的规则 —— **只用通用方案**（不碰各家模型自带的内置搜索）。
 * Keenable 是"给 AI Agent 重建的搜索索引"，一次调用就返回排好序的**正文**，而且默认免密钥。
 */
class WebSearchTool : MobileTool {

    override val name = "web_search"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        // 预检在**参数校验之前**（与桌面执行器的次序一致：额度用尽时先回额度那一条）
        ctx.webBudgetFailure()?.let { return it }

        val query = args.str("query") ?: return ToolOutcome.failure("INVALID_ARGUMENT", "缺少参数 query")
        // ⚠ "没配"与"解不开"必须分开（[SecretLookup]）：前者走免密钥通道照搜；
        //   后者**如实报 SEARCH_CREDENTIAL**（与桌面 `web_tools::resolve_key` 同一个码）——
        //   把"解不开"说成"没配"，用户会以为自己在用登录态，实际在裸奔（AIH-015）。
        val key = when (val found = ctx.lookupSecret(WebCredentialRefs.KEENABLE)) {
            is SecretLookup.Found -> found.value
            SecretLookup.Missing -> null
            SecretLookup.Unreadable -> return ToolOutcome.failure(
                "SEARCH_CREDENTIAL",
                "联网搜索的密钥无法解密；请在设置里重新填写，或移除它以改用免密钥模式",
            )
        }

        // 免密钥走 `/public` 后缀（带 key 时去掉它）—— 官方 SDK 的路径约定：同一个端点名两种调用面。
        // 手拼 JSON 的**键取舍**照抄桌面：可空的 site / published_* 是**省掉键**（不是写 null），
        // mode 永远在（默认 pro）。
        val path = if (key == null) "/v1/search/public" else "/v1/search"
        val body = buildJsonObject {
            put("query", query)
            put("mode", "pro")
            args.str("site")?.let { put("site", it) }
            args.str("publishedAfter")?.let { put("published_after", it) }
            args.str("publishedBefore")?.let { put("published_before", it) }
            put("snippet_max_length", SNIPPET_MAX_LENGTH)
        }.toString()
        val headers = buildMap {
            put("Accept", "application/json")
            // 缺这个头服务端直接 400（`{"error":"Missing app identifier"}`，实测）——它是归因头，不是密钥
            put("X-Keenable-Title", CLIENT_SOURCE)
            // ⚠ 只在这里出现一次：**绝不进日志、绝不进错误正文**（AIH-051）
            key?.let { put("X-API-Key", it) }
        }

        val reply = runCatching { ctx.transport.postJson(DEFAULT_BASE_URL + path, body, headers) }
            .getOrElse { e -> return networkFailure("SEARCH", e, KEENABLE_HOST) }
        if (!reply.ok) return statusFailure("SEARCH", reply.status, reply.body, keyless = key == null)

        val root = parseJsonObject(reply.body)
            ?: return ToolOutcome.failure("SEARCH_FAILED", "搜索服务返回了无法解析的内容")
        val results = parseWebResults(root)
        val seenQuery = (root["query"] as? JsonPrimitive)?.content?.takeIf { it.isNotBlank() } ?: query
        if (results.isEmpty()) {
            return ToolOutcome.failure(
                "NO_RESULTS",
                "「$query」没有搜到结果。换个说法再试（描述得更像你想读的文章），" +
                    "或者告诉用户你没搜到 —— 不要凭记忆编造。",
            )
        }
        val content = ToolResults.truncate(
            ToolResults.untrustedBanner(ctx.kit.untrustedBanner, kind = BANNER_KIND_SEARCH, query = query) +
                "\n\n" + renderWebResults(results),
        )
        ctx.webQueries++
        return ToolOutcome.success(
            content,
            buildJsonObject {
                put("count", results.size)
                put("query", seenQuery)
                // "本次没带 key"是**事实**，要让界面与模型都能看到（模型据此提醒用户可以填一个）
                put("keyless", key == null)
            },
        )
    }

    companion object {
        /** 默认站点（与桌面 `web_tools::DEFAULT_BASE_URL` 同值；手机侧没有环境变量的口子）。 */
        const val DEFAULT_BASE_URL = "https://api.keenable.ai"

        /** 出问题时对方能认出流量来自本项目（与桌面 `CLIENT_SOURCE` 同值）。 */
        const val CLIENT_SOURCE = "ComfyHub"

        /** 每条结果向服务端要的正文上限；总预算由 [CONTEXT_MAX_CHARS] 兜。 */
        const val SNIPPET_MAX_LENGTH = 2_500

        /** 回给模型的搜索结果**总预算**（字符，与桌面 `CONTEXT_MAX_CHARS` 同值、按整条取舍）。 */
        const val CONTEXT_MAX_CHARS = 12_000

        /** 免密钥额度（只用于错误提示里的说明文案，与桌面同值）。 */
        const val KEYLESS_RATE_LIMIT_PER_HOUR = 1_000

        private const val KEENABLE_HOST = "api.keenable.ai"
        private const val BANNER_KIND_SEARCH = "联网搜索结果"
    }
}

// ---------------------------------------------------------------------------
//  civitai_search（Civitai 模型库检索）
// ---------------------------------------------------------------------------

/**
 * `civitai_search`：搜 Civitai 的模型 / LoRA 库，拿到**带简短说明**的结构化结果。
 *
 * 三件事（与桌面 `civitai.rs` 逐条一致）：
 *  1. 安全档走 `civitai.com`、`allowNsfw=true` 时**域名真的切到** `civitai.red`
 *     并把 `nsfw=true` 明写进查询串（"模拟用户真机搜索"就是这个意思）；
 *  2. 每条结果都带上"这个模型是干什么的"：类型 + 底模 + 触发词 + 简介 + 体积；
 *  3. **没令牌也照搜**，但正文里如实写明这次是匿名通道 —— 别让模型把"匿名可见"
 *     当成"这就是全部"。
 */
class CivitaiSearchTool : MobileTool {

    override val name = "civitai_search"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        ctx.webBudgetFailure()?.let { return it }

        val query = args.str("query") ?: return ToolOutcome.failure("INVALID_ARGUMENT", "缺少参数 query")
        // `types` 线上是**大小写敏感的枚举**（实测 `LORA` 200、`lora` 400）：先归一成官方拼写，
        // 真不认识的值**当场说清可选值** —— 否则模型只看到一句"上游 400"，看不出自己哪里错了。
        val rawType = args.str("type")
        val type = rawType?.let {
            normalizeCivitaiType(it) ?: return ToolOutcome.failure(
                "INVALID_ARGUMENT",
                "type 只能是 ${CIVITAI_TYPES.joinToString(" / ")}；收到的是「$it」",
            )
        }
        val allowNsfw = args.bool("allowNsfw") ?: false
        val limit = (args.int("limit") ?: DEFAULT_LIMIT).coerceIn(1, MAX_LIMIT)
        // 同 Keenable：解不开要**如实报**，不许静默降级成匿名通道 ——
        // ⚠ 桌面 `civitai.rs::resolve_token` 把解不开当没有（那边没有对应的码），
        //   所以 `CIVITAI_CREDENTIAL` 是**手机侧新增的码**，理由是 AIH-015
        //   （"把解不开当成没有"比"码名与桌面不一致"糟得多）。两端这里**有意不同**。
        val key = when (val found = ctx.lookupSecret(WebCredentialRefs.CIVITAI)) {
            is SecretLookup.Found -> found.value
            SecretLookup.Missing -> null
            SecretLookup.Unreadable -> return ToolOutcome.failure(
                "CIVITAI_CREDENTIAL",
                "Civitai 的密钥无法解密；请在设置里重新填写它（或移除它以走匿名通道）",
            )
        }
        val base = if (allowNsfw) NSFW_BASE_URL else SAFE_BASE_URL

        // ⚠ `sort=Most Downloaded` 那个空格**必须过编码**（Java URLEncoder 口径：空格变 `+`）——
        //   直接拼进 URI 会抛 `Illegal character in query`（本机实测踩过一次）。
        val url = buildString {
            append(base).append("/api/v1/models?query=").append(urlEncode(query.trim()))
            append("&limit=").append(limit)
            append("&sort=").append(urlEncode(SORT))
            append("&period=AllTime")
            type?.let { append("&types=").append(urlEncode(it)) }
            if (allowNsfw) append("&nsfw=true")
        }
        val headers = buildMap {
            put("Accept", "application/json")
            // 日志与错误里**只允许出现状态码**，令牌一个字符都不落（AIH-051）
            key?.let { put("Authorization", "Bearer $it") }
        }

        val reply = runCatching { ctx.transport.get(url, headers) }
            .getOrElse { e -> return networkFailure("CIVITAI", e, civitaiHost(base)) }
        if (!reply.ok) return statusFailure("CIVITAI", reply.status, reply.body, keyless = key == null)

        val root = parseJsonObject(reply.body)
            ?: return ToolOutcome.failure("CIVITAI_FAILED", "Civitai 返回了无法解析的内容")
        val models = parseCivitaiModels(root)
        if (models.isEmpty()) {
            return ToolOutcome.failure(
                "NO_RESULTS",
                "Civitai 上没搜到「$query」。换个关键词再试一次；" +
                    "如果用户要的是成人内容，确认一下 allowNsfw 打开了 —— " +
                    "仍然没有就如实告诉用户没搜到，不要编造模型名。",
            )
        }
        val tokenless = key == null
        val content = ToolResults.truncate(
            ToolResults.untrustedBanner(ctx.kit.untrustedBanner, kind = BANNER_KIND_CIVITAI, query = query) +
                "\n\n" + renderCivitaiModels(models, query, allowNsfw, tokenless),
        )
        ctx.webQueries++
        return ToolOutcome.success(
            content,
            buildJsonObject {
                put("query", query)
                put("count", models.size)
                put("nsfw", allowNsfw)
                put("tokenless", tokenless)
                put("models", buildJsonArray { models.forEach { add(it.toJson()) } })
            },
        )
    }

    companion object {
        /** 安全档 / NSFW 档的根地址（与桌面 `SAFE_BASE_URL` / `NSFW_BASE_URL` 同值且**写死**）。 */
        const val SAFE_BASE_URL = "https://civitai.com"
        const val NSFW_BASE_URL = "https://civitai.red"

        /** 一次最多回几条（与桌面 `MAX_LIMIT` 同值；上游自己允许 100，这里是我们自己的取舍）。 */
        const val MAX_LIMIT = 20
        const val DEFAULT_LIMIT = 8

        /** 每条简介截断到多少字符（用户要的是"简短的说明"，不是整页 README）。 */
        const val MAX_DESCRIPTION_CHARS = 600

        private const val SORT = "Most Downloaded"
        private const val BANNER_KIND_CIVITAI = "Civitai 模型搜索结果"
    }
}

/** 允许的模型类型（Civitai `types` 参数的官方拼写；顺序就是报错文案里列出来的顺序）。 */
internal val CIVITAI_TYPES: List<String> =
    listOf("Checkpoint", "LORA", "TextualInversion", "Controlnet", "Upscaler", "VAE", "LoCon", "DoRA")

/** 大小写不敏感地归一成官方拼写；不认识回 `null`（调用方据此当场报错并列出可选值）。 */
internal fun normalizeCivitaiType(raw: String): String? =
    CIVITAI_TYPES.firstOrNull { it.equals(raw.trim(), ignoreCase = true) }

// ---------------------------------------------------------------------------
//  共用的出网细节
// ---------------------------------------------------------------------------

/** Java `URLEncoder` 的口径（空格 → `+`，保留 `-_.*`）——与桌面 `tools::url_encode` 同一套。 */
internal fun urlEncode(value: String): String = URLEncoder.encode(value, StandardCharsets.UTF_8)

/** 解析成 JSON **对象**；不是对象（HTML 挑战页 / 数组 / 乱码）回 `null`，由调用方如实报错。 */
internal fun parseJsonObject(text: String): JsonObject? =
    runCatching { com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(text) as? JsonObject }.getOrNull()

/**
 * 网络层失败 → 稳定错误码（`prefix` 是 `SEARCH` 或 `CIVITAI`）。
 *
 * 两条纪律：
 *  1. **不许把"没查成"说成"没搜到"** —— 错误码与文案都要指回"这次请求失败了"；
 *  2. 错误正文里**一个字符的 key 都不许出现**：这里只带异常自己的消息
 *     （`UrlConnectionTransport` 的 `HttpFailure` 里只有 URL 与状态码）。
 */
private fun networkFailure(prefix: String, error: Throwable, host: String): ToolOutcome {
    val detail = (error as? HttpFailure)?.message ?: "${error::class.simpleName}: ${error.message}"
    val timeout = detail.contains("timeout", ignoreCase = true) || detail.contains("timed out", ignoreCase = true)
    return if (timeout) {
        ToolOutcome.failure("${prefix}_TIMEOUT", "联网请求超时（连上了但没在超时时间内回完）；稍后再试，别连着搜")
    } else {
        ToolOutcome.failure("${prefix}_FAILED", "连不上服务（$detail）；本机需要能访问 $host")
    }
}

/** 上游状态码 → 稳定错误码（两端逐条一致；模型按码分支，不按那句话）。 */
private fun statusFailure(prefix: String, status: Int, body: String, keyless: Boolean): ToolOutcome {
    val service = if (prefix == "SEARCH") "搜索服务" else "Civitai"
    val (code, hint) = when (status) {
        401, 403 -> if (prefix == "SEARCH") {
            "${prefix}_AUTH" to "密钥被拒绝；请检查设置里的 Keenable 密钥"
        } else {
            "${prefix}_AUTH" to "Civitai 拒绝了令牌；如果开的是 NSFW 搜索，请让用户重新填一个有效的 Civitai API Key"
        }
        402 -> "${prefix}_QUOTA" to "搜索服务余额不足"
        429 -> if (prefix == "SEARCH") {
            if (keyless) {
                "${prefix}_RATE_LIMIT" to
                    "免密钥模式每小时 ${WebSearchTool.KEYLESS_RATE_LIMIT_PER_HOUR} 次，已超出；" +
                    "稍后再试，或在设置里填一个 Keenable 密钥提高额度"
            } else {
                "${prefix}_RATE_LIMIT" to "该密钥的搜索额度已用尽（429）；稍后再试"
            }
        } else {
            "${prefix}_RATE_LIMIT" to "Civitai 限流了（429）；等一下再试，别连着搜"
        }
        400 -> if (prefix == "SEARCH") {
            "${prefix}_BAD_REQUEST" to "请求被拒绝（多数是查询本身不合法）"
        } else {
            "${prefix}_FAILED" to "$service 返回 400"
        }
        in 500..599 -> "${prefix}_FAILED" to "$service 暂时不可用（$status）"
        else -> "${prefix}_FAILED" to "$service 返回 $status"
    }
    // 服务端自己的 message（**绝不回显我们发出去的 key**）；没有就只留 hint
    val detail = errorDetail(body)
    return ToolOutcome.failure(code, if (detail.isEmpty()) hint else "$hint｜返回：$detail")
}

/** 服务端错误正文里的 `message` / `error` / `detail`，截断后用；解析不出来就原样截断。 */
internal fun errorDetail(text: String): String {
    val obj = parseJsonObject(text)
    if (obj != null) {
        for (key in listOf("message", "error", "detail")) {
            obj.str(key)?.let { return takeUtf16(it, 300) }
        }
        return ""
    }
    return takeUtf16(text, 300)
}

// ---------------------------------------------------------------------------
//  web_search：结果解析与渲染（纯函数，单测直接盯）
// ---------------------------------------------------------------------------

/** 一条搜索结果（`snippet` 是页面正文抽取，不是 meta description）。 */
internal data class WebResult(
    val title: String,
    val url: String,
    val snippet: String,
    val publishedAt: String?,
)

internal fun parseWebResults(root: JsonObject): List<WebResult> =
    (root["results"] as? JsonArray).orEmpty().mapNotNull { element ->
        val obj = element as? JsonObject ?: return@mapNotNull null
        WebResult(
            title = obj.str("title").orEmpty(),
            url = obj.str("url").orEmpty(),
            snippet = obj.str("snippet").orEmpty(),
            publishedAt = obj.str("published_at"),
        )
    }

/**
 * 结果 → "编号 + 标题 + 链接 + 正文"的块（形状照抄官方 SDK 的 `to_context()`，与桌面同一份口径）。
 *
 * 两条别改：
 *  - 编号按**实际保留的条数**走：中间某条超预算被跳过时不能留下 `[1][2][4]` 这种空洞 ——
 *    模型引用的是这里的编号，空洞会让"第 3 条"指向不存在的结果；
 *  - 以**整条**为单位取舍：宁可少给一条，也不给模型半句话。
 */
internal fun renderWebResults(results: List<WebResult>): String {
    val kept = mutableListOf<String>()
    var used = 0
    results.forEach { r ->
        val header = buildString {
            append("[").append(kept.size + 1).append("] ")
            append(if (r.title.trim().isEmpty()) "(无标题)" else r.title.trim())
            append(" (").append(r.url).append(")")
            r.publishedAt?.let { append(" - 发布 $it") }
        }
        val body = collapseSpaces(r.snippet)
        val block = if (body.isEmpty()) header else "$header\n$body"
        val cost = block.length + if (kept.isEmpty()) 0 else 2
        if (kept.isNotEmpty() && used + cost > WebSearchTool.CONTEXT_MAX_CHARS) return@forEach
        kept += block
        used += cost
    }
    return kept.joinToString("\n\n")
}

// ---------------------------------------------------------------------------
//  civitai_search：结果解析与渲染
// ---------------------------------------------------------------------------

/** 一条 Civitai 模型 / LoRA（只留"用得上"的那几个字段）。 */
internal data class CivitaiModel(
    val id: Long,
    val name: String,
    val type: String,
    val baseModel: String,
    val nsfw: Boolean,
    val creator: String?,
    val downloads: Long,
    val thumbsUp: Long,
    val pageUrl: String,
    val description: String,
    val trainedWords: List<String>,
    /** 拿不到就是 `null` —— 那时正文与界面都**不写这一行**，绝不用猜出来的数字撑场面。 */
    val sizeBytes: Long?,
    val downloadUrl: String?,
    val fileName: String?,
    val sha256: String?,
) {
    /** 给人看的体积标签（单位分档与桌面 `size_label` 同：LoRA 常见 100~300MB，checkpoint 6~14GB）。 */
    fun sizeLabel(): String {
        val bytes = sizeBytes ?: return "未知（作者没给文件体积）"
        val gb = bytes.toDouble() / 1024 / 1024 / 1024
        return if (gb >= 1.0) {
            String.format(Locale.US, "%.2f GB", gb)
        } else {
            String.format(Locale.US, "%.0f MB", bytes.toDouble() / 1024 / 1024)
        }
    }

    /** 结构化结果（键的顺序与取舍照抄桌面 `to_json`：可空键**省掉**而不是写 null）。 */
    fun toJson(): JsonObject = buildJsonObject {
        put("id", id)
        put("name", name)
        put("type", type)
        put("baseModel", baseModel)
        put("nsfw", nsfw)
        creator?.let { put("creator", it) }
        put("downloads", downloads)
        put("thumbsUp", thumbsUp)
        put("pageUrl", pageUrl)
        put("description", description)
        sizeBytes?.let {
            put("sizeBytes", it)
            put("sizeLabel", sizeLabel())
        }
        downloadUrl?.let { put("downloadUrl", it) }
        fileName?.let { put("fileName", it) }
        sha256?.let { put("sha256", it) }
        if (trainedWords.isNotEmpty()) {
            put("trainedWords", buildJsonArray { trainedWords.forEach { add(JsonPrimitive(it)) } })
        }
    }
}

internal fun parseCivitaiModels(root: JsonObject): List<CivitaiModel> =
    (root["items"] as? JsonArray).orEmpty().mapNotNull { element ->
        toCivitaiModel(element as? JsonObject ?: return@mapNotNull null)
    }

/**
 * 一个 Civitai 条目 → [CivitaiModel]（认不出 id 就丢掉这一条）。
 *
 * 字段路径与线上核对过（与桌面 `to_model` 一致）：
 *  - **底模**在 `modelVersions[0].baseModel`（顶层只有 `baseModels` 数组，拿它当主来源
 *    会在"一个模型跨多个底模"时给出含糊结果，所以只在 `modelVersions` 缺失时兜底）；
 *  - **体积 / 下载地址 / sha256** 在 `modelVersions[0].files[]` 里 `primary=true` 那条
 *    （没有 primary 标记就取第一个）；sha256 **只认 64 位十六进制**，别把 `AutoV2` 那种短码
 *    当 sha256 —— 用户拿短码去校验盘上的文件永远对不上，那比"不知道"更糟；
 *  - `pageUrl` 给**正式站**，不是本次请求用的 base（base 可能是本机假服务 / 自建代理）。
 */
internal fun toCivitaiModel(obj: JsonObject): CivitaiModel? {
    val id = obj.longField("id") ?: return null
    val name = obj.str("name") ?: "#$id"
    val firstVersion = (obj["modelVersions"] as? JsonArray)?.firstOrNull() as? JsonObject
    val baseModel = firstVersion?.str("baseModel")
        ?: (obj["baseModels"] as? JsonArray)?.firstOrNull()?.let { (it as? JsonPrimitive)?.content }
        ?: ""
    val trainedWords = (firstVersion?.get("trainedWords") as? JsonArray)
        .orEmpty()
        .mapNotNull { (it as? JsonPrimitive)?.content?.takeIf { word -> word.isNotBlank() } }

    val files = (firstVersion?.get("files") as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }
    val primary = files.firstOrNull { looseBool(it["primary"]) } ?: files.firstOrNull()
    val hashes = primary?.get("hashes") as? JsonObject
    val sha256 = hashes?.entries
        ?.firstOrNull { it.key.equals("SHA256", ignoreCase = true) }
        ?.value?.let { (it as? JsonPrimitive)?.content?.trim() }
        ?.takeIf { it.length == 64 && it.all { c -> c.isDigit() || c.lowercaseChar() in 'a'..'f' } }

    val stats = obj["stats"] as? JsonObject
    return CivitaiModel(
        id = id,
        name = name,
        type = obj.str("type").orEmpty(),
        baseModel = baseModel,
        nsfw = looseBool(obj["nsfw"]),
        creator = (obj["creator"] as? JsonObject)?.str("username"),
        downloads = stats?.longField("downloadCount") ?: 0,
        thumbsUp = stats?.longField("thumbsUpCount") ?: 0,
        pageUrl = "${CivitaiSearchTool.SAFE_BASE_URL}/models/$id",
        description = takeUtf16(
            javaTrim(collapseSpaces(stripTags(obj.str("description").orEmpty()))),
            CivitaiSearchTool.MAX_DESCRIPTION_CHARS,
        ),
        trainedWords = trainedWords,
        sizeBytes = primary?.doubleField("sizeKB")?.takeIf { it.isFinite() }?.let { (it * 1024).toLong() },
        downloadUrl = primary?.str("downloadUrl"),
        fileName = primary?.str("name"),
        sha256 = sha256,
    )
}

/**
 * 结果 → "条目 + 一句话说明"的块（给模型看的那一份）。
 *
 * `tokenless` 且 `nsfw` 时在抬头补一句"本次是匿名通道"：这不是装饰，
 * 而是让模型知道**这次没带登录态**，免得它把"匿名可见"当成"这就是全部"。
 */
internal fun renderCivitaiModels(
    models: List<CivitaiModel>,
    query: String,
    nsfw: Boolean,
    tokenless: Boolean,
): String {
    if (models.isEmpty()) return ""
    val head = buildString {
        append("Civitai 搜索「").append(query).append("」")
        if (nsfw) append("（NSFW 通道）")
        append("：").append(models.size).append(" 条")
        if (nsfw && tokenless) {
            append(
                "\n（本次没带 Civitai 令牌，走的是匿名通道 —— 结果照常可用；" +
                    "要更稳的登录态可以提醒用户去「设置 → AI 工具 / 凭据」填一个，但不必停下来等。）",
            )
        }
    }
    val blocks = models.mapIndexed { index, m ->
        buildString {
            append("[").append(index + 1).append("] ").append(m.name)
            append("（").append(if (m.type.trim().isEmpty()) "未知类型" else m.type)
            if (m.baseModel.trim().isNotEmpty()) append(" · 底模 ").append(m.baseModel)
            append("）")
            if (m.nsfw) append(" [NSFW]")
            append('\n')
            append("用途：").append(if (m.description.trim().isEmpty()) "（作者没写简介）" else m.description)
            append('\n')
            if (m.trainedWords.isNotEmpty()) {
                append("触发词：").append(m.trainedWords.joinToString(", ")).append('\n')
            }
            if (m.sizeBytes != null) append("体积：").append(m.sizeLabel()).append('\n')
            append(m.pageUrl).append("（下载 ").append(m.downloads).append(" · 赞 ").append(m.thumbsUp).append("）")
            m.downloadUrl?.let { append("\n直链：").append(it) }
        }
    }
    return "$head\n\n${blocks.joinToString("\n\n")}"
}

// ---------------------------------------------------------------------------
//  文本口径（JVM 那几条，与桌面逐条对齐）
// ---------------------------------------------------------------------------

/**
 * Kotlin `Regex("<[^>]+>").replace(text, " ")` 的对应物。
 *
 * 判据要留住两条细节：`[^>]` **含换行**（标签可以跨行匹配），而 `+` 要求**至少一个字符**
 * （`<>` 不算标签）。
 */
internal fun stripTags(text: String): String = Regex("<[^>]+>", RegexOption.DOT_MATCHES_ALL).replace(text, " ")

/** 空白压平（与桌面 `collapse_spaces` 同：所有空白序列 → 一个空格、两端去掉）。 */
internal fun collapseSpaces(text: String): String = Regex("\\s+").replace(text, " ").trim()

/**
 * Java `String.trim()` 的对应物：**只去 `<= U+0020`**（全角空格不去）。
 *
 * 为什么要单独写一个：`trim()` 在 Kotlin 里去的是 `Char.isWhitespace()`，两者对
 * `\u3000`（全角空格）的处理不同 —— 简介的两端全角空格去不去，正文里看得出来。
 */
internal fun javaTrim(text: String): String {
    var start = 0
    var end = text.length
    while (start < end && text[start] <= ' ') start++
    while (end > start && text[end - 1] <= ' ') end--
    return text.substring(start, end)
}

/** 取前 [limit] 个 **UTF-16 单元**（Java `String.length` 的口径），**不切开代理对**。 */
internal fun takeUtf16(text: String, limit: Int): String {
    if (limit <= 0) return ""
    if (text.length <= limit) return text
    val end = if (Character.isHighSurrogate(text[limit - 1])) limit - 1 else limit
    return text.substring(0, end)
}

/** `JsonObject.str`（MobileTool.kt 里那份）的窄用版：只按字面量取，空白算没给。 */
private fun JsonObject.longField(key: String): Long? = str(key)?.toLongOrNull()

private fun JsonObject.doubleField(key: String): Double? = str(key)?.toDoubleOrNull()

/** Kotlin `content.toBoolean()`（**宽松**：忽略大小写的 `"true"` 才算真）。 */
private fun looseBool(value: kotlinx.serialization.json.JsonElement?): Boolean =
    (value as? JsonPrimitive)?.content?.equals("true", ignoreCase = true) ?: false

/** 两端写死的两个域名之一（只用于错误正文里那句"本机需要能访问谁"）。 */
private fun civitaiHost(base: String): String =
    if (base == CivitaiSearchTool.NSFW_BASE_URL) "civitai.red" else "civitai.com"
