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
 *   · **NSFW 要令牌、且域名变成 `.red`** —— 走安全档还是成人档，必须在请求上看得见
 *     （Host 不同、查询串里明写 `nsfw=true`、带不带 Bearer 令牌）；
 *   · **缺令牌时不许静默降级** —— 必须报 `CIVITAI_TOKEN_REQUIRED` 并且**一个请求都不发**，
 *     否则模型会拿到一份被过滤过的结果却以为看到了全部。
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

    @Test
    fun `NSFW 缺令牌时直接 CIVITAI_TOKEN_REQUIRED 且一个请求都不发`() = runBlocking {
        withCivitai { safe, nsfw, captured ->
            val client = CivitaiSearch(credentials(), safeBaseUrl = safe, nsfwBaseUrl = nsfw)
            val e = assertFailsWith<ToolFailure> { client.search("任意查询", allowNsfw = true) }

            assertEquals("CIVITAI_TOKEN_REQUIRED", e.code)
            assertTrue(e.message!!.contains("令牌"), e.message)
            // 关键：**不许静默降级成安全搜索**（那会让模型以为看到了全部）
            assertTrue(captured().isEmpty(), "缺令牌时不该发任何请求：${captured()}")
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
