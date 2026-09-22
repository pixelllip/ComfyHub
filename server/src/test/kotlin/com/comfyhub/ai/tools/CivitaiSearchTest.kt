package com.comfyhub.ai.tools

import com.comfyhub.ai.CredentialService
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking

/**
 * Civitai 客户端在**真实 HTTP** 上的行为（不联网：全部打本机临时端口）。
 *
 * 钉的是用户建议 ④ 那条原话里的两件事：
 *   · **NSFW 要显式打开、且域名变成 `.red`** —— 走安全档还是成人档，必须在请求上看得见
 *     （Host 不同、查询串里明写 `nsfw=true`）；
 *   · **令牌有就带、没有也说实话** —— 2026-09-20 核对线上后放宽：`.red` 匿名也能搜到成人结果
 *     （见 `CivitaiSearch` 的类注释），所以缺令牌**不再拒绝搜索**，
 *     但正文里必须写明"本次是匿名通道"，不许让模型以为拿到的是登录态的全量结果。
 *
 * 另外三条 HTTP 纪律与 [KeenableSearchTest] 同规矩：错误只映射成稳定码、
 * **绝不在错误正文里回显令牌**、解析出来的"用途说明"要能进给模型的文本。
 */
class CivitaiSearchTest {

    private data class Captured(
        val path: String,
        val query: String?,
        val authorization: String?,
    )

    private inline fun withCivitai(
        status: Int = 200,
        body: String = DEFAULT_BODY,
        block: (safe: String, nsfw: String, captured: () -> List<Captured>) -> Unit,
    ) {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val seen = mutableListOf<Captured>()
        server.createContext("/") { exchange ->
            seen += capture(exchange)
            val bytes = body.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        server.start()
        try {
            // 两个"域名"都指向同一个本机服务：**判据是客户端选了哪个 base**，
            // 而不是真去连 civitai.red（那才是"不出网"这条纪律要的）
            val base = "http://127.0.0.1:${server.address.port}"
            block(base, base) { synchronized(seen) { seen.toList() } }
        } finally {
            server.stop(0)
        }
    }

    private fun capture(exchange: HttpExchange): Captured {
        exchange.requestBody.readBytes()
        return Captured(
            path = exchange.requestURI.path,
            query = exchange.requestURI.rawQuery,
            authorization = exchange.requestHeaders.getFirst("Authorization"),
        )
    }

    /** 凭据服务：令牌走环境变量源（绕开 DPAPI，不碰真子系统）。 */
    private fun credentials(token: String? = null): CredentialService =
        CredentialService(
            Files.createTempDirectory("comfyhub-civitai"),
            env = if (token == null) mapOf() else mapOf("COMFYHUB_CRED_civitai" to token),
        )

    companion object {
        private val DEFAULT_BODY = """
            {"items":[
              {"id":42,"name":"水墨风 LoRA","type":"LORA","nsfw":false,
               "creator":{"username":"someone"},
               "stats":{"downloadCount":1234,"thumbsUpCount":56},
               "description":"<p>把照片转成<b>水墨画</b>风格，适合风景与人物。</p>",
               "modelVersions":[{"baseModel":"SDXL 1.0","trainedWords":["inkwash","chinese painting"]}]}
            ]}
        """.trimIndent().replace("\n", "")
    }

    // --- 安全档 ---------------------------------------------------------------

    @Test
    fun `安全搜索不带令牌 且不写 nsfw 参数`() = runBlocking {
        withCivitai { safe, _, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("水墨风", allowNsfw = false)

            val hit = captured().single()
            assertEquals("/api/v1/models", hit.path)
            assertNull(hit.authorization, "安全档不该带令牌")
            assertFalse(hit.query!!.contains("nsfw"), "没要成人内容就不要写 nsfw 参数：${hit.query}")
            assertTrue(hit.query.contains("query="), hit.query)

            assertEquals(1, outcome.models.size)
            assertEquals("水墨风 LoRA", outcome.models[0].name)
            assertEquals("SDXL 1.0", outcome.models[0].baseModel)
            // 用户建议 ⑦ 要的"简短的说明"：HTML 标签去掉、压成一行
            // （标签位置替换成空格，所以相邻文字之间会有空格）
            assertFalse(outcome.context.contains("<b>"), "简介里的 HTML 标签必须去掉：${outcome.context}")
            assertTrue(outcome.context.contains("[1] 水墨风 LoRA"), outcome.context)
            assertTrue(outcome.context.contains("用途：把照片转成 水墨画 风格"), outcome.context)
            assertTrue(outcome.context.contains("触发词：inkwash, chinese painting"), outcome.context)
        }
    }

    // --- NSFW 档 --------------------------------------------------------------

    @Test
    fun `NSFW 搜索走成人站 base 带令牌 且明写 nsfw 参数`() = runBlocking {
        withCivitai { safe, nsfw, captured ->
            val client = CivitaiSearch(credentials("civ_test"), safeBaseUrl = safe, nsfwBaseUrl = nsfw)
            val outcome = client.search("任意查询", allowNsfw = true)

            val hit = captured().single()
            assertEquals("Bearer civ_test", hit.authorization, "成人档必须带令牌")
            assertTrue(hit.query!!.contains("nsfw=true"), "成人档要明写 nsfw=true：${hit.query}")
            assertEquals(true, outcome.meta["nsfw"]?.let { it.toString().toBoolean() })
        }
    }

    /**
     * 没配令牌时的 NSFW 搜索（2026-09-20 用户决定放宽）。
     *
     * 老行为是当场抛 `CIVITAI_TOKEN_REQUIRED`、**一个请求都不发**，依据是"`.red` 只认登录态"。
     * 真机核对推翻了这个前提（`.red` 匿名 200，结果与 `.com?nsfw=true` 逐条一致），
     * 于是改成：**照搜**，但正文里如实写明这次是匿名通道 ——
     * 放宽的是"能不能搜"，没有放宽"要不要说实话"。
     */
    @Test
    fun `NSFW 没令牌也照搜 成人站 并在正文里写明是匿名通道`() = runBlocking {
        withCivitai { safe, nsfw, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = nsfw)
            val outcome = client.search("任意查询", allowNsfw = true)

            val hit = captured().single()
            assertNull(hit.authorization, "没配令牌就不带 Authorization 头")
            assertTrue(hit.query!!.contains("nsfw=true"), "成人档要明写 nsfw=true：${hit.query}")
            assertEquals(true, outcome.meta["nsfw"]?.let { it.toString().toBoolean() })
            assertEquals(true, outcome.meta["tokenless"]?.let { it.toString().toBoolean() })
            assertTrue(outcome.context.contains("匿名通道"), "要如实说明这次没带令牌：${outcome.context}")
        }
    }

    /** 安全档不许出现"匿名通道"那句（那是 NSFW 档的提示，别到处贴）。 */
    @Test
    fun `安全档的正文里不写匿名通道提示`() = runBlocking {
        withCivitai { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("水墨风")
            assertFalse(outcome.context.contains("匿名通道"), outcome.context)
        }
    }

    @Test
    fun `配置了令牌时 hasToken 为真`() {
        assertFalse(CivitaiSearch(credentials()).hasToken())
        assertTrue(CivitaiSearch(credentials("civ_test")).hasToken())
    }

    // --- 错误映射与令牌保密 -----------------------------------------------------

    @Test
    fun `401 归到 CIVITAI_AUTH 且错误正文里绝不回显令牌`() = runBlocking {
        withCivitai(status = 401, body = """{"message":"unauthorized"}""") { safe, _, _ ->
            val client = CivitaiSearch(credentials("super-secret-token"), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val e = assertFailsWith<ToolFailure> { client.search("任意查询", allowNsfw = false) }

            assertEquals("CIVITAI_AUTH", e.code)
            assertFalse(e.message!!.contains("super-secret-token"), "错误里不能出现令牌：${e.message}")
        }
    }

    @Test
    fun `429 归到 CIVITAI_RATE_LIMIT 并提醒别连着搜`() = runBlocking {
        withCivitai(status = 429, body = """{"error":"rate limited"}""") { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val e = assertFailsWith<ToolFailure> { client.search("任意查询") }
            assertEquals("CIVITAI_RATE_LIMIT", e.code)
            assertTrue(e.message!!.contains("429"), e.message)
        }
    }

    @Test
    fun `type 与 limit 会进查询串 limit 有上限`() = runBlocking {
        withCivitai { safe, _, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            client.search("任意查询", type = "LORA", limit = 999)

            val q = captured().single().query!!
            assertTrue(q.contains("types=LORA"), q)
            assertTrue(q.contains("limit=${CivitaiSearch.MAX_LIMIT}"), "limit 必须被夹到上限：$q")
        }
    }

    /**
     * `types` 线上**大小写敏感**（2026-09-20 实测：`LORA` 200，`lora` / `Lora` 400）：
     * 小写很自然（模型自己写工具参数时不会抄大小写），直接透传就会变成一句没头没脑的
     * "上游 400"，所以本地先归一成官方拼写再发。
     */
    @Test
    fun `type 的大小写会被归一成官方拼写`() = runBlocking {
        withCivitai { safe, _, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            client.search("任意查询", type = "lora")
            assertTrue(captured().single().query!!.contains("types=LORA"), captured().single().query!!)
        }
    }

    @Test
    fun `不认识的 type 当场报清楚 不发请求`() = runBlocking {
        withCivitai { safe, _, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val e = assertFailsWith<ToolFailure> { client.search("任意查询", type = "Loraa") }

            assertEquals("INVALID_ARGUMENT", e.code)
            // 报错要把**可选值**列出来，模型才能自己改对；只说"400"它就只会反复试
            assertTrue(e.message!!.contains("LORA"), e.message)
            assertTrue(captured().isEmpty(), "本地就能判错的东西不该出网")
        }
    }

    /**
     * 线上字段核对（2026-09-20 实打 `GET /api/v1/models`）：`baseModel` 在
     * `modelVersions[0]` 里，顶层另有一个 `baseModels` **数组**。前者缺失时用后者兜底 ——
     * 兜底那一条要有用例盯着，否则它只是"看起来很稳"的一行代码。
     */
    @Test
    fun `modelVersions 缺失时用顶层 baseModels 兜底`() = runBlocking {
        val body = """
            {"items":[{"id":7,"name":"只有顶层底模","type":"LORA","nsfw":false,
              "baseModels":["Illustrious","SDXL 1.0"],
              "stats":{"downloadCount":5,"thumbsUpCount":1},
              "description":"测试用"}]}
        """.trimIndent().replace("\n", "")

        withCivitai(body = body) { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("任意查询")
            assertEquals("Illustrious", outcome.models[0].baseModel)
        }
    }

    // --- 体积（用户建议 ②，2026-09-22）-----------------------------------------

    /**
     * 线上 `modelVersions[0].files[]` 里 `primary=true` 那条带 `sizeKB` / `downloadUrl`。
     *
     * 为什么要钉它：用户建议 ② 的原话是"把缺口升级成缺口 + 需要装什么 + **大概多大**"，
     * 而昨天真实卡住的那份 checkpoint 是 **13.49 GB** —— 没有体积，
     * 用户在"装它 / 换工作流 / 放弃"这三条路之间根本没法选。
     */
    @Test
    fun `主文件体积与直链一路进到正文与结构化字段`() = runBlocking {
        // 13.49 GB ≈ 14145495 KB（与昨天那份 krea2TurboInt8 同量级）
        val body = """
            {"items":[{"id":9,"name":"krea2 Turbo Int8","type":"Checkpoint","nsfw":false,
              "stats":{"downloadCount":10,"thumbsUpCount":2},
              "description":"大底模",
              "modelVersions":[{"baseModel":"Flux.1 D","trainedWords":[],
                "files":[{"name":"krea2.safetensors","primary":true,"sizeKB":14145495.0,
                          "downloadUrl":"https://civitai.com/api/download/models/9"}]}]}]}
        """.trimIndent().replace("\n", "")

        withCivitai(body = body) { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("krea2 turbo")
            val m = outcome.models.single()
            assertTrue(m.sizeBytes != null)
            assertEquals("13.49 GB", m.sizeLabel())
            assertTrue(outcome.context.contains("体积：13.49 GB"), outcome.context)
            assertTrue(outcome.context.contains("直链：https://civitai.com/api/download/models/9"), outcome.context)
            // 结构化字段：界面与模型都能直接用，不用去解析正文
            val json = outcome.toJson()["models"].toString()
            assertTrue(json.contains("sizeLabel"), json)
            assertTrue(json.contains("13.49 GB"), json)
        }
    }

    @Test
    fun `作者没给体积时不编一个数：正文不写那一行 结构化字段也不写这个键`() = runBlocking {
        withCivitai { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("水墨风")
            val m = outcome.models.single()
            assertNull(m.sizeBytes, "上游没给就是 null，不许猜")
            assertEquals("未知（作者没给文件体积）", m.sizeLabel())
            assertFalse(outcome.context.contains("体积："), "拿不到就别写这一行：${outcome.context}")
            assertFalse(outcome.toJson()["models"].toString().contains("sizeLabel"), "别写 0 / null")
        }
    }

    @Test
    fun `空查询词当场报 INVALID_ARGUMENT 不发请求`() = runBlocking {
        withCivitai { safe, _, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val e = assertFailsWith<ToolFailure> { client.search("   ") }
            assertEquals("INVALID_ARGUMENT", e.code)
            assertTrue(captured().isEmpty())
        }
    }

    // --- 结构化结果（用户建议 ⑦："登记在数据库中"） -------------------------------

    @Test
    fun `结构化结果带 id 名字 说明 触发词与链接`() = runBlocking {
        withCivitai { safe, _, _ ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = safe)
            val outcome = client.search("水墨风")
            val json = outcome.toJson()
            val model = json["models"]!!.let {
                (it as kotlinx.serialization.json.JsonArray)[0].let { m ->
                    m as kotlinx.serialization.json.JsonObject
                }
            }

            assertEquals("42", model["id"].toString())
            assertEquals("水墨风 LoRA", model["name"].toString().trim('"'))
            // 说明进的是"结构化字段"：标签已去掉、能直接被 remember 落库
            assertTrue(
                model["description"].toString().contains("把照片转成"),
                "description 要带上用途：${model["description"]}",
            )
            assertEquals("https://civitai.com/models/42", model["pageUrl"].toString().trim('"'))
            assertTrue(model["trainedWords"].toString().contains("inkwash"))
        }
    }
}
