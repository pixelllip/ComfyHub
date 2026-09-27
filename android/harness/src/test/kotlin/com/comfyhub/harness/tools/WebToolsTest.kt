package com.comfyhub.harness.tools

import com.comfyhub.harness.kit.RunKit
import com.comfyhub.harness.kit.RunKitCodec
import com.comfyhub.harness.net.HttpReply
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessDatabase
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.JdbcSqlSession
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * 手机侧的两支出网工具：`web_search` / `civitai_search`（2026-09-27 补齐 MOBILE 工具面）。
 *
 * 走**假 HTTP 客户端**（宿主 JVM，不出网），逐条钉住与桌面 `web_tools.rs` / `civitai.rs`
 * 对齐的那四件事：**端点与查询串形状**、**参数与默认值**、**稳定错误码**、
 * 以及**回给模型的那段正文**（含只做 `{kind}` / `{query}` 替换的不可信横幅，R7）。
 *
 * 另外三件在别处最容易静默出错的：
 *  - 三个出网工具**共用一份联网预算**（各写一份的下场是"某一条的额度不算数"）；
 *  - **没 key 也要如实**（Keenable 免密钥通道 / Civitai 匿名通道，正文里写明白）；
 *  - **key 一个字符都不许进结果或错误正文**（AIH-051）。
 */
class WebToolsTest {

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore
    private lateinit var files: DefaultHarnessFiles
    private lateinit var kit: RunKit

    /** 一条被记下来的出网请求（形状断言全靠它）。 */
    private data class Recorded(
        val method: String,
        val url: String,
        val body: String?,
        val headers: Map<String, String>,
    )

    /** 假出网：所有请求都回同一条 `reply`，但把请求原样记下来。 */
    private class FakeNet : HttpTransport {
        val requests = mutableListOf<Recorded>()
        var reply: HttpReply = HttpReply(200, "{}")

        override fun get(url: String, headers: Map<String, String>): HttpReply {
            requests += Recorded("GET", url, null, headers)
            return reply
        }

        override fun postJson(url: String, body: String, headers: Map<String, String>): HttpReply {
            requests += Recorded("POST", url, body, headers)
            return reply
        }

        override fun postJsonStreaming(
            url: String,
            body: String,
            headers: Map<String, String>,
            onLine: (String) -> Unit,
        ): Int = 404

        val last: Recorded get() = requests.last()
    }

    /** 故意与桌面那份**不一样的**横幅模板：只要输出里出现它，就说明文案确实来自 kit（R7）。 */
    private val banner = "‹{kind}·来自工件›\n查询：{query}"

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("harness-web-tools-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
        files = DefaultHarnessFiles(dir.toFile())
        kit = RunKitCodec.parse(
            """
            {"kitVersion":"2026-09-27.1","schemaVersion":3,"systemPrompt":"系统提示","promptVersion":"v30",
             "nsfwEnabled":false,"tools":[],"untrustedBanner":${"\"" + banner.replace("\n", "\\n") + "\""},
             "titleInstruction":"起标题指令","models":[],
             "inlineBudget":{"perImageBytes":8388608,"totalBytes":20971520}}
            """.trimIndent(),
        )
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    private fun context(
        net: HttpTransport,
        secrets: Map<String, String> = emptyMap(),
        /** 这些 ref "配了但解不开"（换过系统用户 / Keystore 钥匙丢了）—— 与"没配"必须分开。 */
        unreadable: Set<String> = emptySet(),
        webLimit: Int = WebQueryBudget.MAX_QUERIES_PER_RUN,
    ) = ToolContext(
        runId = "r1",
        conversationId = "c1",
        kit = kit,
        store = store,
        transport = net,
        files = files,
        askGate = MobileAskGate(),
        emit = { _, _ -> },
        lookupSecret = { ref ->
            when {
                ref == null -> SecretLookup.Missing
                ref in unreadable -> SecretLookup.Unreadable
                secrets[ref] != null -> SecretLookup.Found(secrets.getValue(ref))
                else -> SecretLookup.Missing
            }
        },
    ).also { it.webQueriesLimit = webLimit }

    // --- web_search ---------------------------------------------------------

    private val searchBody = """
        {"query":"flux 提示词技巧","mode":"pro","results":[
          {"title":"Flux 提示词指南","url":"https://example.com/flux","snippet":"正文片段一","published_at":"2026-08-01"},
          {"title":"第二条","url":"https://example.com/2","snippet":"片段二"}]}
    """.trimIndent()

    @Test
    fun `web_search 免密钥：打 public、带标题头、结果带 kit 的横幅`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, searchBody) }
        val ctx = context(net)

        val out = WebSearchTool().invoke(buildJsonObject { put("query", "flux 提示词技巧") }, ctx)

        assertTrue(out.ok)
        val req = net.last
        assertEquals("POST", req.method)
        assertEquals(
            "免密钥必须走 /public（官方 SDK 的路径约定）",
            "https://api.keenable.ai/v1/search/public",
            req.url,
        )
        assertEquals("缺这个头服务端直接 400", "ComfyHub", req.headers["X-Keenable-Title"])
        assertNull("免密钥请求不该带 X-API-Key", req.headers["X-API-Key"])
        val body = req.body!!
        assertTrue("mode 默认 pro：$body", body.contains("\"mode\":\"pro\""))
        assertTrue("总预算靠它兜：$body", body.contains("\"snippet_max_length\":2500"))
        assertFalse("没给的键不许出现（省键，不是写 null）：$body", body.contains("\"site\""))

        // ★ R7：横幅文案只做 {kind} / {query} 替换，模板来自 kit（不是手机里写死的）
        assertTrue("横幅必须来自 run-kit：\n${out.content}", out.content.startsWith("‹联网搜索结果·来自工件›\n查询：flux 提示词技巧"))
        assertTrue(out.content.contains("[1] Flux 提示词指南 (https://example.com/flux) - 发布 2026-08-01"))
        assertTrue(out.content.contains("正文片段一"))

        val payload = out.jsonPayload!!
        assertEquals("2", payload["count"].toString())
        assertEquals("true", payload["keyless"].toString())
        assertEquals("一次成功搜索占一次额度", 1, ctx.webQueries)
    }

    @Test
    fun `web_search 带密钥：走非 public 路径，且密钥不出现在结果里`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, searchBody) }
        val out = WebSearchTool().invoke(
            buildJsonObject { put("query", "q") },
            context(net, secrets = mapOf(WebCredentialRefs.KEENABLE to "keen_secret_value")),
        )

        assertTrue(out.ok)
        assertEquals("带 key 时不能走 /public", "https://api.keenable.ai/v1/search", net.last.url)
        assertEquals("keen_secret_value", net.last.headers["X-API-Key"])
        assertFalse("绝不回显密钥", out.content.contains("keen_secret_value"))
        assertEquals("false", out.jsonPayload!!["keyless"].toString())
    }

    @Test
    fun `web_search 没搜到就如实说没搜到，不许编造`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, """{"results":[]}""") }
        val ctx = context(net)
        val out = WebSearchTool().invoke(buildJsonObject { put("query", "不存在的词") }, ctx)

        assertFalse(out.ok)
        assertEquals("NO_RESULTS", out.errorCode)
        assertTrue(out.content.contains("没有搜到"))
        assertEquals("没搜到不算一次额度", 0, ctx.webQueries)
    }

    @Test
    fun `web_search 状态码映射到稳定错误码（模型按码分支）`() = runBlocking {
        for ((status, expected) in listOf(
            401 to "SEARCH_AUTH",
            403 to "SEARCH_AUTH",
            402 to "SEARCH_QUOTA",
            429 to "SEARCH_RATE_LIMIT",
            400 to "SEARCH_BAD_REQUEST",
            503 to "SEARCH_FAILED",
            418 to "SEARCH_FAILED",
        )) {
            val net = FakeNet().also { it.reply = HttpReply(status, """{"message":"why"}""") }
            val out = WebSearchTool().invoke(buildJsonObject { put("query", "q") }, context(net))
            assertEquals("status=$status", expected, out.errorCode)
            assertTrue("服务端自己的 message 要带上：${out.content}", out.content.contains("｜返回：why"))
        }
    }

    @Test
    fun `web_search 限流：免密钥说清额度与出路，带密钥说清是密钥额度且绝不回显`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(429, """{"error":"quota"}""") }
        val keyless = WebSearchTool().invoke(buildJsonObject { put("query", "q") }, context(net))
        assertTrue("要说明免密钥额度：${keyless.content}", keyless.content.contains("1000"))
        assertTrue(keyless.content.contains("填一个 Keenable 密钥"))

        val withKey = WebSearchTool().invoke(
            buildJsonObject { put("query", "q") },
            context(net, secrets = mapOf(WebCredentialRefs.KEENABLE to "keen_secret_value")),
        )
        assertTrue("带 key 时要说清是密钥额度用尽", withKey.content.contains("额度"))
        assertFalse("绝不回显密钥", withKey.content.contains("keen_secret_value"))
    }

    /**
     * **密钥解不开 ≠ 没配**（AIH-015）：解不开要**如实报码**让人去重填，
     * 而且**一个请求都不发**（免密钥 / 匿名通道不是遮羞布：用户以为自己在用登录态）。
     */
    @Test
    fun `密钥解不开要如实报码，不许静默降级成无 key`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, searchBody) }

        val search = WebSearchTool().invoke(
            buildJsonObject { put("query", "q") },
            context(net, unreadable = setOf(WebCredentialRefs.KEENABLE)),
        )
        assertFalse(search.ok)
        assertEquals("SEARCH_CREDENTIAL", search.errorCode)
        assertTrue("要说清怎么办：${search.content}", search.content.contains("重新填写"))
        assertEquals("解不开时一个请求都不发", 0, net.requests.size)

        val civitai = CivitaiSearchTool().invoke(
            buildJsonObject { put("query", "q") },
            context(net, unreadable = setOf(WebCredentialRefs.CIVITAI)),
        )
        assertFalse(civitai.ok)
        assertEquals("CIVITAI_CREDENTIAL", civitai.errorCode)
        assertEquals(0, net.requests.size)
    }

    /** 三个出网工具共用一份预算（见下面那条用例；这条钉住"没配 ≠ 解不开"的反面）。 */
    @Test
    fun `没有 key（Missing）是正常状态：照搜，只是如实标出没带登录态`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, searchBody) }
        val ctx = context(net)
        val out = WebSearchTool().invoke(buildJsonObject { put("query", "q") }, ctx)

        assertTrue("没配 key 不该失败：免密钥通道照常可用", out.ok)
        assertEquals("true", out.jsonPayload!!["keyless"].toString())
        assertTrue(net.last.url.endsWith("/v1/search/public"))
    }

    // --- civitai_search -----------------------------------------------------

    private val sha = "a".repeat(64)
    private val civitaiBody = """
        {"items":[
          {"id":42,"name":"水墨风 LoRA","type":"LORA","nsfw":false,
           "creator":{"username":"someone"},
           "stats":{"downloadCount":1234,"thumbsUpCount":56},
           "description":"<p>把照片转成<b>水墨画</b>风格，适合风景与人物。</p>",
           "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":["inkwash","chinese painting"],
             "files":[{"primary":true,"sizeKB":262144,"downloadUrl":"https://civitai.com/api/download/models/1",
                       "name":"ink.safetensors","hashes":{"SHA256":"$sha","AutoV2":"ABCDEF"}}]}]}
        ]}
    """.trimIndent()

    @Test
    fun `civitai_search 查询串与结构化结果：编码、默认 limit、类型归一`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, civitaiBody) }
        val out = CivitaiSearchTool().invoke(
            buildJsonObject {
                put("query", "水墨风 style")
                put("type", "lora") // 小写：线上大小写敏感，必须归一成官方拼写
            },
            context(net),
        )

        assertTrue(out.ok)
        val url = net.last.url
        assertTrue("空格按 Java URLEncoder 口径编成 +：$url", url.startsWith("https://civitai.com/api/v1/models?query="))
        assertTrue("中文按 UTF-8 百分号编码、空格编成 +（Java URLEncoder 口径）：$url", url.contains("query=%E6%B0%B4%E5%A2%A8%E9%A3%8E+style"))
        assertTrue("默认 8 条：$url", url.contains("&limit=8"))
        assertTrue("sort 的空格必须过编码（拼错会抛 Illegal character in query）：$url", url.contains("&sort=Most+Downloaded"))
        assertTrue(url.contains("&period=AllTime"))
        assertTrue("类型要归一成官方拼写：$url", url.contains("&types=LORA"))
        assertFalse("安全档不许带 nsfw：$url", url.contains("nsfw=true"))
        assertNull("没配令牌就不带 Authorization", net.last.headers["Authorization"])

        assertTrue(out.content.startsWith("‹Civitai 模型搜索结果·来自工件›\n查询：水墨风 style"))
        assertTrue("简介要去标签压空白：${out.content}", out.content.contains("用途：把照片转成 水墨画 风格，适合风景与人物。"))
        assertTrue(out.content.contains("触发词：inkwash, chinese painting"))
        assertTrue(out.content.contains("体积：256 MB"))
        assertTrue("链接给正式站：${out.content}", out.content.contains("https://civitai.com/models/42"))
        assertTrue(out.content.contains("直链：https://civitai.com/api/download/models/1"))

        val model = (out.jsonPayload!!["models"] as kotlinx.serialization.json.JsonArray).single().let { it as kotlinx.serialization.json.JsonObject }
        assertEquals("42", model["id"].toString())
        assertEquals("\"LORA\"", model["type"].toString())
        assertEquals("\"SDXL 1.0\"", model["baseModel"].toString())
        assertEquals("262144 KB × 1024 = 256MB", "268435456", model["sizeBytes"].toString())
        assertEquals("\"256 MB\"", model["sizeLabel"].toString())
        assertEquals("\"$sha\"", model["sha256"].toString())
    }

    @Test
    fun `civitai_search 允许 NSFW：域名真的切到 red 并明写 nsfw=true，匿名时如实说明`() = runBlocking {
        val net = FakeNet().also {
            it.reply = HttpReply(
                200,
                """{"items":[{"id":7,"name":"成人模型","type":"LORA","nsfw":true,
                     "description":"x","modelVersions":[{"baseModel":"SD 1.5"}]}]}""",
            )
        }
        val out = CivitaiSearchTool().invoke(
            buildJsonObject {
                put("query", "nsfw test")
                put("allowNsfw", true)
            },
            context(net),
        )

        assertTrue(out.ok)
        assertTrue("NSFW 档要走 .red（模拟用户真机搜索）：${net.last.url}", net.last.url.startsWith("https://civitai.red/api/v1/models?"))
        assertTrue("并把 nsfw 明写进查询串：${net.last.url}", net.last.url.contains("&nsfw=true"))
        assertTrue("没令牌时正文要如实写明匿名通道：${out.content}", out.content.contains("匿名通道"))
        assertTrue("正文里要标出这条是 NSFW", out.content.contains("[NSFW]"))
        assertTrue("链接仍给正式站", out.content.contains("https://civitai.com/models/7"))
    }

    @Test
    fun `civitai_search 类型写错当场列可选值，一个请求都不发`() = runBlocking {
        val net = FakeNet()
        val out = CivitaiSearchTool().invoke(
            buildJsonObject {
                put("query", "q")
                put("type", "lora2")
            },
            context(net),
        )
        assertEquals("INVALID_ARGUMENT", out.errorCode)
        assertTrue("要说清可选值：${out.content}", out.content.contains("Checkpoint / LORA"))
        assertTrue("一个请求都不该发出去", net.requests.isEmpty())
    }

    @Test
    fun `civitai_search 没搜到与状态码`() = runBlocking {
        val empty = FakeNet().also { it.reply = HttpReply(200, """{"items":[]}""") }
        val none = CivitaiSearchTool().invoke(buildJsonObject { put("query", "q") }, context(empty))
        assertEquals("NO_RESULTS", none.errorCode)
        assertTrue(none.content.contains("不要编造模型名"))

        for ((status, expected) in listOf(
            401 to "CIVITAI_AUTH",
            429 to "CIVITAI_RATE_LIMIT",
            500 to "CIVITAI_FAILED",
            418 to "CIVITAI_FAILED",
        )) {
            val net = FakeNet().also { it.reply = HttpReply(status, "") }
            val out = CivitaiSearchTool().invoke(buildJsonObject { put("query", "q") }, context(net))
            assertEquals("status=$status", expected, out.errorCode)
        }
    }

    // --- 共用预算 / 形状细节 -------------------------------------------------

    /**
     * **三个出网工具共用一份预算**（与桌面执行器同一纪律：预检在调用之前、计数只在成功之后）。
     *
     * 各写一份的下场：系统提示里那句"一次回复最多联网 8 次"在手机上变成空话，
     * 而不会有任何用例变红。
     */
    @Test
    fun `联网预算：三支共用一份，超限一个请求都不发，失败不占额度`() = runBlocking {
        val net = FakeNet().also { it.reply = HttpReply(200, searchBody) }
        val ctx = context(net, webLimit = 1)

        assertTrue(WebSearchTool().invoke(buildJsonObject { put("query", "q") }, ctx).ok)
        assertEquals(1, ctx.webQueries)
        assertEquals(1, net.requests.size)

        // 第二支工具（web_fetch）：额度已满 → 明确失败，**且一个请求都不发**
        val second = WebFetchTool().invoke(buildJsonObject { put("url", "https://example.com/a") }, ctx)
        assertFalse(second.ok)
        assertEquals("QUERY_BUDGET_EXCEEDED", second.errorCode)
        assertTrue(second.content.contains("已达上限"))
        assertEquals("超限那次不能真发出去", 1, net.requests.size)

        // 失败不占额度：一次 500 之后额度还是 0
        val failing = FakeNet().also { it.reply = HttpReply(500, "boom") }
        val fresh = context(failing)
        assertFalse(CivitaiSearchTool().invoke(buildJsonObject { put("query", "q") }, fresh).ok)
        assertEquals("失败 / 上游报错都不占联网额度", 0, fresh.webQueries)
    }

    /**
     * 渲染是**按整条取舍**的：超预算的那条整个跳过，编号不许留空洞
     * （模型引用的是这里的编号，`[1][2][4]` 会让"第 3 条"指向不存在的结果）。
     */
    @Test
    fun `搜索结果按整条取舍且编号无空洞`() {
        val short1 = WebResult("标题一", "https://a/1", "片段一", "2026-08-01")
        val short2 = WebResult("", "https://a/2", " 多  空白\n片段 ", null)
        assertEquals(
            "[1] 标题一 (https://a/1) - 发布 2026-08-01\n片段一\n\n[2] (无标题) (https://a/2)\n多 空白 片段",
            renderWebResults(listOf(short1, short2)),
        )

        val huge = WebResult("巨大", "https://a/huge", "字".repeat(WebSearchTool.CONTEXT_MAX_CHARS), null)
        val skipped = renderWebResults(listOf(short1, huge, short2))
        assertTrue("超预算的那条整个不给：$skipped", !skipped.contains("巨大"))
        assertTrue("后面的编号接着数（[2] 而不是 [3]）：$skipped", skipped.contains("[2] (无标题) (https://a/2)"))

        val only = renderWebResults(listOf(huge))
        assertTrue("第一条自己超预算时照样留着（与官方 SDK 同）", only.startsWith("[1] 巨大 (https://a/huge)"))
    }

    /** 最终还有一道 `ToolResults.truncate`：超长要**如实说明**截断了（不许假装完整）。 */
    @Test
    fun `结果总长超限要截断并如实说明`() {
        val long = "x".repeat(ToolResults.MAX_RESULT_CHARS + 10)
        val cut = ToolResults.truncate(long)
        assertTrue("保留的是前 MAX_RESULT_CHARS 个字符", cut.startsWith(long.take(ToolResults.MAX_RESULT_CHARS)))
        assertTrue("要如实说清截断了、一共多少字符：$cut", cut.endsWith("字符）"))
        assertEquals("短的原样返回", "短", ToolResults.truncate("短"))
    }

    /** 文本口径（与桌面逐条对齐）：去标签、压空白、Java `trim`、UTF-16 截断。 */
    @Test
    fun `文本口径与桌面一致`() {
        assertEquals(" 把照片转成 水墨画 风格 ", stripTags("<p>把照片转成<b>水墨画</b>风格</p>"))
        assertEquals("a", collapseSpaces(stripTags("<p>a</p>")))
        // `[^>]` 含换行（标签可以跨行），`<>` 不算标签（`+` 至少一个字符）
        assertEquals("a d", stripTags("a<b\nc>d"))
        assertEquals("<>", stripTags("<>"))
        // Java 的 trim 只去 <= U+0020：全角空格**不去**
        assertEquals("x\u3000", javaTrim("  x\u3000  "))
        // UTF-16 单元数（中文 1、emoji 2），且不切开代理对
        assertEquals("a", takeUtf16("a🙂b", 2))
        assertEquals("a🙂", takeUtf16("a🙂b", 3))
        assertEquals("水墨", takeUtf16("水墨风", 2))
        assertEquals("abc", takeUtf16("abc", 9))
    }

    /** `sha256` 只认 64 位十六进制（`AutoV2` 那种短码不能当 sha256 —— 拿它去校验永远对不上）。 */
    @Test
    fun `短码不许当 sha256`() {
        val model = toCivitaiModel(
            kotlinx.serialization.json.Json.parseToJsonElement(
                """{"id":1,"name":"n","modelVersions":[{"files":[{"primary":true,"hashes":{"SHA256":"ABCDEF"}}]}]}""",
            ) as kotlinx.serialization.json.JsonObject,
        )!!
        assertNull(model.sha256)

        val upper = toCivitaiModel(
            kotlinx.serialization.json.Json.parseToJsonElement(
                """{"id":1,"name":"n","modelVersions":[{"files":[{"primary":true,"hashes":{"SHA256":"${"A".repeat(64)}"}}]}]}""",
            ) as kotlinx.serialization.json.JsonObject,
        )!!
        assertEquals("A".repeat(64), upper.sha256)
    }
}
