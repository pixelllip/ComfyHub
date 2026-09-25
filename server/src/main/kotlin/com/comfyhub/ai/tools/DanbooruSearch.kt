package com.comfyhub.ai.tools

import com.comfyhub.ai.OutboundProxy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.add
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
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
import java.time.Instant

/**
 * Danbooru 查询（O8，2026-09-25，用户要求："对于支持 tag 的模型（比如 Anima），
 * 除了专门 LoRA 内置的提示词外**也要查查 danbooru**"）。
 *
 * # 为什么要有这条工具
 *
 * Anima / SD 这类 **Danbooru tag 模型**的提示词是有"正确写法"的：
 * 角色 tag 必须与 danbooru 上的**同一个 tag**（`shinku_(nte)` 而不是 `shinku` 或 `真红`），
 * 外观 / 服装 / 动作要写成 danbooru 认的那几个词。模型凭印象编 tag 的后果不是报错，
 * 是**出一张别人**（实测现场：shinku 被当成 iroi 写了两版废图）。
 *
 * LoRA 的触发词只覆盖"这个 LoRA 自己训出来的那部分"，它既不告诉你角色的
 * **其他规范 tag**，也不告诉你 danbooru 上这个角色**有没有别名/换装版本**。
 *
 * # 三条实测事实（2026-09-25 本机量到的）
 *
 * 1. **必须带 User-Agent**：不带 UA 时 danbooru 直接回 **403**（用 curl 与
 *    `Invoke-WebRequest` 都复现过）。所以这个客户端把 UA 写死成常量，
 *    别"顺手"删掉它。
 * 2. **本机必须走代理**：`danbooru.donmai.us` 直连不稳定（大量 connect timeout），
 *    经 `127.0.0.1:7890` 稳定 200。走 [OutboundProxy.selector]（公网走系统代理、
 *    本机/内网强制直连），与 `CivitaiSearch` 同一条路。
 * 3. **`wiki_pages/<tag>.json` 404 是正常情况**（这个 tag 没有 wiki 页，不是错误）：
 *    照旧把 tags / related 给出去，只在正文里说明"这个 tag 没有 wiki 页"。
 *
 * # 一次查询最多 4 个请求（有界）
 *
 * ① 精确 `name_matches=<tag>`（查这个 tag 本身，含改名/弃用标记）
 * ② 模糊 `name_matches=*<q>*`（拿换装版本 / 同名角色的候选，也用来判断"你写的 tag 不存在"）
 * ③ `wiki_pages/<tag>.json`（别名 `other_names` + 简介）
 * ④ `related_tag.json?query=<tag>`（**同现 tag**：danbooru 上这个角色最常用的外观/服装 tag，
 *    按 frequency 排序 —— 这就是"规范 tag 栈"的来源）
 *
 * ③④ 只在②里真的解析出一个角色 tag 时才发。工具层的联网预算是 8 次/回复，
 * 所以一次查询 4 个请求是能接受的上限；**不要**在这里做"多轮追问式"查询。
 */
interface DanbooruClient {
    /**
     * 查一个角色 / tag。
     *
     * @param query 用户或模型给的名字（中文、罗马字、带/不带括号都行）
     * @param limit 候选 tag 最多几条（1..20）
     * @param relatedLimit 同现 tag 最多几条（1..50）
     */
    suspend fun lookup(query: String, limit: Int = 12, relatedLimit: Int = 20): DanbooruOutcome
}

/** 一次 danbooru 查询的结果：`text` 给模型看，`meta` 给界面与"写进角色卡"用。 */
data class DanbooruOutcome(
    val text: String,
    val meta: JsonObject,
)

class DanbooruSearch(
    baseUrl: String = defaultBaseUrl(),
    private val timeoutSeconds: Long = DEFAULT_TIMEOUT_SECONDS,
    /** 这次出网用的 User-Agent（默认 [USER_AGENT]，可用 [ENV_UA] 覆盖）。 */
    private val userAgent: String = userAgent(),
) : DanbooruClient {

    private val log = LoggerFactory.getLogger(DanbooruSearch::class.java)
    private val base = baseUrl.trim().trimEnd('/')

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(CONNECT_TIMEOUT_SECONDS))
        .followRedirects(HttpClient.Redirect.NEVER)
        .proxy(OutboundProxy.selector())
        .build()

    companion object {
        const val DEFAULT_BASE_URL = "https://danbooru.donmai.us"

        /**
         * User-Agent **必须带**，而且**必须短**（实测：不带就 403）。
         *
         * 2026-09-25 本机实测（同一个 URL、同一个代理，只换 UA）：
         *
         * | UA | 结果 |
         * | --- | --- |
         * | `ComfyHub/1.0 (+local)` | **200** |
         * | `ComfyHub/1.0` | 200 |
         * | `comfyhub/1.0 (+local)` | 200 |
         * | `ComfyHub/1.0 (local ComfyUI desktop app; tag lookup)` | 403 + Cloudflare 挑战页 |
         * | `ComfyHub/0.1 (local ComfyUI desktop app)` | 403 + Cloudflare 挑战页 |
         * | `Mozilla/5.0 (Windows NT 10.0; Win64; x64)` | 403 + Cloudflare 挑战页 |
         *
         * 也就是说 danbooru 前面挂着 **Cloudflare 的 managed challenge**，它会按 UA 的形状挑人，
         * 而"写得越长越像真人/越像脚本"反而越容易被拦。所以这里**刻意取最短的那一种**，
         * 并且给用户留一个环境变量口子（[ENV_UA]）—— 哪天它又变了，不必改代码重发版本。
         */
        const val USER_AGENT = "ComfyHub/1.0 (+local)"

        /** 覆盖 User-Agent（被 Cloudflare 拦时用户自己能换一个，不用等我们发版）。 */
        const val ENV_UA = "COMFYHUB_DANBOORU_UA"

        fun userAgent(env: (String) -> String? = System::getenv): String =
            env(ENV_UA)?.trim()?.takeIf { it.isNotEmpty() } ?: USER_AGENT

        const val CONNECT_TIMEOUT_SECONDS = 5L
        const val DEFAULT_TIMEOUT_SECONDS = 20L
        const val MAX_TIMEOUT_SECONDS = 60L

        /** 一次最多回几条候选 tag（再多对"挑对那个 tag"没有帮助，只烧 token）。 */
        const val MAX_LIMIT = 20

        /** 同现 tag 最多回几条（20 条已经覆盖"外观 + 服装 + 常见动作"）。 */
        const val MAX_RELATED = 50

        /** wiki 正文截断（wiki 里有大量 `!post #…` 的图例，正文本身很短）。 */
        const val MAX_WIKI_CHARS = 1200

        /** 环境变量覆盖（本机假服务的口子，与 KeenableSearch / CivitaiSearch 同一约定）。 */
        fun defaultBaseUrl(): String =
            System.getenv("DANBOORU_BASE_URL")?.trim()?.takeIf { it.isNotEmpty() } ?: DEFAULT_BASE_URL

        /**
         * danbooru 的 tag 分类码 → 人话。
         *
         * 0 普通 / 1 画师 / 3 作品（版权）/ 4 角色 / 5 meta。**2 目前没有定义**，
         * 所以别写成 `0..5` 的数组下标（那会把 2 说成 copyright）。
         */
        fun categoryName(code: Int?): String = when (code) {
            0 -> "general"
            1 -> "artist"
            3 -> "copyright"
            4 -> "character"
            5 -> "meta"
            else -> "unknown"
        }

        /**
         * 把用户/模型的写法归一成 danbooru 的 tag 写法。
         *
         * 空格 → 下划线（danbooru 的 tag 不允许空格；`shinku (nte)` 与
         * `shinku_(nte)` 是同一个 tag）、全角括号 → 半角、小写。
         * **不动下划线本身**（`shinku_(nte)` 里的下划线是 tag 的一部分）。
         */
        fun normalizeTag(raw: String): String = raw.trim()
            .lowercase()
            // 空白 → 下划线（danbooru 的 tag 不允许空格）
            .replace(Regex("\\s+"), "_")
            // 括号：统一成半角，且前面**保证有一个下划线** —— danbooru 的消歧写法是
            // `name_(work)`；用户写 `shinku（nte）` 时不能变成 `shinku(nte)`（那是个不存在的 tag）。
            .replace(Regex("_*[（(]"), "_(")
            .replace(Regex("[）)]"), ")")
            .trim('_')
    }

    override suspend fun lookup(query: String, limit: Int, relatedLimit: Int): DanbooruOutcome {
        val q = normalizeTag(query)
        require(q.isNotEmpty()) { "查询词不能为空" }
        val lim = limit.coerceIn(1, MAX_LIMIT)
        val relLim = relatedLimit.coerceIn(1, MAX_RELATED)

        val exact = fetchTags(q, fuzzy = false, limit = lim)
        // 模糊那一趟**总是要发**：换装版本（`shinku_(dawn_patrol)_(nte)`）只出现在模糊结果里，
        // 而"这个角色有哪些版本"正是"别自己编 tag"最需要的信息。
        val fuzzy = fetchTags(q, fuzzy = true, limit = lim)

        val all = (exact + fuzzy).distinctBy { it.name }
        val character = all.filter { it.category == 4 }.maxByOrNull { it.postCount }
        val variants = all.filter { it.category == 4 && it.name != character?.name }.map { it.name }

        var wiki: JsonObject? = null
        var related: List<RelatedTag> = emptyList()
        if (character != null) {
            wiki = fetchWiki(character.name)
            related = fetchRelated(character.name, relLim)
        }

        val verifiedAt = Instant.now().toString()
        val meta = buildJsonObject {
            put("query", query)
            put("normalizedTag", q)
            put("exact", tagsJson(exact))
            if (character == null) put("candidates", tagsJson(all.take(lim)))
            character?.let { c ->
                put(
                    "character",
                    buildJsonObject {
                        put("tag", c.name)
                        put("postCount", c.postCount)
                        put("deprecated", c.isDeprecated)
                        if (variants.isNotEmpty()) put("variants", buildJsonArray { variants.forEach { add(it) } })
                        if (wiki != null) {
                            put("otherNames", wiki["other_names"] ?: JsonArray(emptyList()))
                            put("wiki", wiki["body"]?.let { bodyText(it.jsonPrimitive.content) } ?: "")
                            put("wikiMissing", false)
                        } else {
                            put("wikiMissing", true)
                        }
                        put("relatedTags", relatedJson(related))
                    },
                )
                put("wikiUrl", "$base/wiki_pages/${encode(c.name)}")
            }
            put("verifiedAt", verifiedAt)
            put("provider", "danbooru")
        }

        return DanbooruOutcome(
            text = render(query, q, exact, all, character, variants, wiki, related),
            meta = meta,
        )
    }

    // -----------------------------------------------------------------------
    //  正文（给模型看的那一份）
    // -----------------------------------------------------------------------

    private fun render(
        query: String,
        q: String,
        exact: List<DanbooruTag>,
        all: List<DanbooruTag>,
        character: DanbooruTag?,
        variants: List<String>,
        wiki: JsonObject?,
        related: List<RelatedTag>,
    ): String = buildString {
        appendLine("danbooru 查询：$query（归一成 `$q`）")
        appendLine("分类码：0=general 1=artist 3=copyright 4=character 5=meta")

        if (exact.isEmpty()) {
            appendLine("· 精确 tag `$q` **不存在**（danbooru 上没这个 tag）——别照这个写法出图。")
        } else {
            appendLine("· 精确命中：" + exact.joinToString("、") { "`${it.name}`（${categoryName(it.category)}，${it.postCount} 张）" })
        }
        val others = all.filterNot { e -> exact.any { it.name == e.name } }
        if (others.isNotEmpty()) {
            appendLine("· 名字相近的 tag（可能是同一角色的换装版本 / 同名角色）：")
            others.take(12).forEach {
                appendLine("   - `${it.name}`（${categoryName(it.category)}，${it.postCount} 张）")
            }
        }

        if (character == null) {
            appendLine()
            appendLine("**没有解析出角色 tag**：上面没有 category=4 的命中。")
            appendLine("下一步：拿一个真正的角色 tag 再查一次（`角色名_(作品名)` 那种），或者问用户这个角色出自哪部作品。")
            return@buildString
        }

        appendLine()
        appendLine("★ 角色 tag：`${character.name}`（${character.postCount} 张${if (character.isDeprecated) "，**已弃用**" else ""}）")
        val otherNames = wiki?.get("other_names")?.let { el ->
            runCatching { el.jsonArray }.getOrNull()?.mapNotNull { it.jsonPrimitive.contentOrNull }
        }.orEmpty()
        if (otherNames.isNotEmpty()) appendLine("· 别名（danbooru 记的 other names）：" + otherNames.joinToString("、"))
        val body = wiki?.get("body")?.jsonPrimitive?.contentOrNull
        if (body.isNullOrBlank()) {
            appendLine("· wiki：这个 tag 没有 wiki 页（正常情况，不影响 tag 本身可用）。")
        } else {
            appendLine("· wiki 正文（截断）：")
            appendLine("```")
            appendLine(bodyText(body))
            appendLine("```")
        }
        if (variants.isNotEmpty()) {
            appendLine("· 同角色的**换装 / 其他版本 tag**（要换衣服就换这个 tag，别自己编）：" + variants.joinToString("、"))
        }
        if (related.isNotEmpty()) {
            appendLine("· 同现 tag（danbooru 上这个角色最常配的 tag，按频率降序）——**这就是规范 tag 栈的来源**：")
            appendLine("   " + related.joinToString(" ") { it.name })
        } else {
            appendLine("· 同现 tag：没拿到（该 tag 的图太少或接口不可用）——这时才凭 LoRA 触发词 + 常识写。")
        }
        appendLine()
        appendLine(
            "写角色卡时：`characterTag` = `${character.name}`，" +
                "`aliases` = 上面的别名，`tags` = 上面挑出来的同现 tag（外观/服装/动作那几类），" +
                "`source` = `$base/wiki_pages/${encode(character.name)}`。"
        )
    }

    private fun bodyText(body: String): String {
        val cleaned = body.replace("\r\n", "\n").trim()
        return if (cleaned.length <= MAX_WIKI_CHARS) cleaned else cleaned.take(MAX_WIKI_CHARS) + "\n…（wiki 过长已截断）"
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    private data class DanbooruTag(
        val name: String,
        val category: Int?,
        val postCount: Int,
        val isDeprecated: Boolean,
    )

    private data class RelatedTag(
        val name: String,
        val category: Int?,
        val postCount: Int,
        val frequency: Double,
    )

    private suspend fun fetchTags(query: String, fuzzy: Boolean, limit: Int): List<DanbooruTag> {
        val pattern = if (fuzzy) "*$query*" else query
        val url = "$base/tags.json?search%5Bname_matches%5D=${encode(pattern)}" +
            "&search%5Border%5D=count&limit=$limit"
        // ⚠ `tags.json` 回的是**数组**（`wiki_pages` / `related_tag` 回的是对象）——
        // 一律当 JsonObject 解析会在第一个请求上就炸（"返回了无法解析的内容"，
        // 而那看着像"danbooru 挂了"，其实是我们解析错了形状）。
        val body = getJson(url) ?: return emptyList()
        val arr = runCatching { body.jsonArray }.getOrNull() ?: return emptyList()
        return arr.mapNotNull { el ->
            val obj = runCatching { el.jsonObject }.getOrNull() ?: return@mapNotNull null
            val name = obj.str("name") ?: return@mapNotNull null
            DanbooruTag(
                name = name,
                category = obj.int("category"),
                postCount = obj.int("post_count") ?: 0,
                isDeprecated = obj["is_deprecated"]?.jsonPrimitive?.booleanOrNull == true,
            )
        }
    }

    /** wiki 页：404 是**正常情况**（没有 wiki 页），回 null 而不是报错。 */
    private suspend fun fetchWiki(tag: String): JsonObject? {
        val url = "$base/wiki_pages/${encode(tag)}.json"
        return getJson(url, notFoundOk = true)?.let { runCatching { it.jsonObject }.getOrNull() }
    }

    private suspend fun fetchRelated(tag: String, limit: Int): List<RelatedTag> {
        val url = "$base/related_tag.json?query=${encode(tag)}&category=General&limit=$limit"
        // 404 也当"没有同现 tag"：这一步是**增值信息**，它挂了不该把已经解析出来的
        // 角色 tag / 别名一起丢掉（正文里会如实写"同现 tag 没拿到"）。
        // 429 / 5xx 仍然抛错 —— 那是"没查成"，与"没有"是两回事。
        val body = getJson(url, notFoundOk = true)?.let { runCatching { it.jsonObject }.getOrNull() }
            ?: return emptyList()
        val arr = body["related_tags"]?.let { el -> runCatching { el.jsonArray }.getOrNull() } ?: return emptyList()
        return arr.mapNotNull { el ->
            val obj = runCatching { el.jsonObject }.getOrNull() ?: return@mapNotNull null
            val tagObj = obj.obj("tag") ?: return@mapNotNull null
            val name = tagObj.str("name") ?: return@mapNotNull null
            RelatedTag(
                name = name,
                category = tagObj.int("category"),
                postCount = tagObj.int("post_count") ?: 0,
                frequency = obj["frequency"]?.jsonPrimitive?.contentOrNull?.toDoubleOrNull() ?: 0.0,
            )
        }
    }

    /**
     * 发一次 GET 并解析 JSON（**形状由调用方判**：`tags.json` 是数组、其余是对象）。
     *
     * 404 在 [notFoundOk] 下回 null（wiki 页没有就是没有）；其余非 2xx 一律
     * 抛 [ToolFailure]（带稳定码），**绝不把"没查成"说成"没有这个 tag"**。
     */
    private suspend fun getJson(url: String, notFoundOk: Boolean = false): JsonElement? {
        val uri = runCatching { URI(url) }.getOrElse {
            throw ToolFailure("DANBOORU_FAILED", "查询地址拼装失败（这是本机的 bug，请如实告诉用户）")
        }
        val request = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(timeoutSeconds.coerceIn(1, MAX_TIMEOUT_SECONDS)))
            .header("Accept", "application/json")
            .header("User-Agent", userAgent)
            .GET()
            .build()

        val response = withContext(Dispatchers.IO) {
            runCatching { client.send(request, HttpResponse.BodyHandlers.ofString()) }
        }.getOrElse { e ->
            val host = runCatching { URI(url).host }.getOrNull() ?: "danbooru.donmai.us"
            val route = OutboundProxy.describe(host)
            throw when (e) {
                is java.net.http.HttpConnectTimeoutException ->
                    ToolFailure(
                        "DANBOORU_TIMEOUT",
                        "连不上 $host（$CONNECT_TIMEOUT_SECONDS 秒内没连上；本次出网路径「$route」）。" +
                            "danbooru 在国内通常**必须走代理** —— 如实告诉用户，别反复重试，" +
                            "也不要凭印象编 tag。",
                    )
                is java.net.http.HttpTimeoutException ->
                    ToolFailure("DANBOORU_TIMEOUT", "连上了 $host 但 $timeoutSeconds 秒没回完（路径「$route」）；稍后再试")
                else ->
                    ToolFailure("DANBOORU_FAILED", "连不上 $host（${e::class.simpleName}；路径「$route」）")
            }
        }

        val status = response.statusCode()
        val text = response.body().orEmpty()
        if (status == 404 && notFoundOk) return null
        if (status !in 200..299) {
            // Cloudflare 的 managed challenge 长这样：状态 403 + 一整页 HTML（标题 "Just a moment..."）。
            // 必须**单独认出来**：把它报成"UA 被拒"或"没有这个 tag"都会把用户带偏
            // （前者让人以为换个 UA 就好，后者直接让人以为角色不存在）。
            if (status == 403 && looksLikeCloudflareChallenge(text)) {
                throw ToolFailure(
                    "DANBOORU_CLOUDFLARE",
                    "danbooru 前面的 Cloudflare 拦住了这次请求（403 + 挑战页，不是 danbooru 自己回的）。" +
                        "本机实测**短 UA 才过得去**（当前用的是 `$userAgent`）。" +
                        "换一个 User-Agent（环境变量 ${ENV_UA}）再试，或者让用户自己在浏览器里查这个 tag —— " +
                        "**不要**把这个结果说成「danbooru 上没有这个角色」。",
                )
            }
            val (code, hint) = when (status) {
                403 -> "DANBOORU_UA_REJECTED" to
                    "danbooru 拒绝了这次请求（403）：它的机器人政策要求带说明身份的 User-Agent（当前用的是 `$userAgent`）"
                404 -> "DANBOORU_NOT_FOUND" to "danbooru 上没有这个 tag（404）——**别照这个写法出图**"
                429 -> "DANBOORU_RATE_LIMIT" to "danbooru 限流了（429）；等一下再查，不要连着查"
                in 500..599 -> "DANBOORU_FAILED" to "danbooru 暂时不可用（$status）"
                else -> "DANBOORU_FAILED" to "danbooru 返回 $status"
            }
            log.warn("danbooru 查询失败：status={} url={}", status, uri.path)
            throw ToolFailure(code, hint)
        }
        return runCatching { com.comfyhub.AppJson.parseToJsonElement(text) }.getOrElse {
            // 2xx 也可能是挑战页（Cloudflare 有时用 200 送挑战页）——同样别报成"内容坏了"
            if (looksLikeCloudflareChallenge(text)) {
                throw ToolFailure(
                    "DANBOORU_CLOUDFLARE",
                    "danbooru 前面的 Cloudflare 送回了挑战页（HTTP $status 但不是 JSON）。" +
                        "可用环境变量 ${ENV_UA} 换一个 User-Agent 再试；**不要**说成「没有这个角色」。",
                )
            }
            throw ToolFailure("DANBOORU_FAILED", "danbooru 返回了无法解析的内容（status=$status）")
        }
    }

    /** Cloudflare 挑战页的判据：HTML 里的那两个标记（比"看 Content-Type"更耐改版）。 */
    private fun looksLikeCloudflareChallenge(body: String): Boolean =
        body.contains("Just a moment", ignoreCase = true) ||
            body.contains("challenge-platform") ||
            body.contains("Enable JavaScript and cookies to continue")

    private fun encode(value: String): String =
        URLEncoder.encode(value, StandardCharsets.UTF_8).replace("+", "%20")

    private fun tagsJson(tags: List<DanbooruTag>): JsonArray = buildJsonArray {
        tags.forEach { t ->
            add(
                buildJsonObject {
                    put("name", t.name)
                    put("category", categoryName(t.category))
                    put("postCount", t.postCount)
                    if (t.isDeprecated) put("deprecated", true)
                }
            )
        }
    }

    private fun relatedJson(tags: List<RelatedTag>): JsonArray = buildJsonArray {
        tags.forEach { t ->
            add(
                buildJsonObject {
                    put("name", t.name)
                    put("postCount", t.postCount)
                    put("frequency", t.frequency)
                }
            )
        }
    }
}
