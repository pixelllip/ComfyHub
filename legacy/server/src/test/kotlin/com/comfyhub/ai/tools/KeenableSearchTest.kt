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
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking

/**
 * Keenable 客户端在**真实 HTTP** 上的行为（不联网：全部打本机临时端口）。
 *
 * 钉的都是踩过/实测过的坑：
 *  - 免密钥请求**必须**带 `X-Keenable-Title`（2026-09-20 实测：缺它服务端直接
 *    `400 {"error":"Missing app identifier"}`）；
 *  - 免密钥走 `/v1/search/public`，带 key 走 `/v1/search`（官方 SDK 的路径约定）；
 *  - 429 归到 `SEARCH_RATE_LIMIT` 并说清免密钥额度，且**绝不回显密钥**；
 *  - `web_fetch` 的目标是内网 / 本机地址时，必须在出网**之前**被拒。
 */
class KeenableSearchTest {

    /** 一次请求的关键事实（路径 / 头 / 正文），给断言用。 */
    private data class Captured(
        val method: String,
        val path: String,
        val query: String?,
        val keenableTitle: String?,
        val apiKey: String?,
        val body: String,
    )

    // 必须是 inline：block 里要直接调 suspend 的 search()/page()
    private inline fun withKeenable(
        searchStatus: Int = 200,
        searchBody: String = DEFAULT_SEARCH_BODY,
        fetchStatus: Int = 200,
        fetchBody: String = DEFAULT_FETCH_BODY,
        block: (base: String, captured: () -> List<Captured>) -> Unit,
    ) {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val seen = mutableListOf<Captured>()
        server.createContext("/") { exchange ->
            seen += capture(exchange)
            val isFetch = exchange.requestURI.path.endsWith("/fetch") ||
                exchange.requestURI.path.endsWith("/fetch/public")
            val status = when {
                isFetch -> fetchStatus
                else -> searchStatus
            }
            val body = when {
                isFetch -> fetchBody
                else -> searchBody
            }
            val bytes = body.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        server.start()
        try {
            block("http://127.0.0.1:${server.address.port}") { synchronized(seen) { seen.toList() } }
        } finally {
            server.stop(0)
        }
    }

    private fun capture(exchange: HttpExchange): Captured {
        val bodyBytes = exchange.requestBody.readBytes()
        return Captured(
            method = exchange.requestMethod,
            path = exchange.requestURI.path,
            query = exchange.requestURI.rawQuery,
            keenableTitle = exchange.requestHeaders.getFirst("X-Keenable-Title"),
            apiKey = exchange.requestHeaders.getFirst("X-API-Key"),
            body = String(bodyBytes, StandardCharsets.UTF_8),
        )
    }

    /** 凭据服务：keyless 测试不带 env；带 key 的测试用环境变量源（绕开 DPAPI，不碰真子系统）。 */
    private fun credentials(key: String? = null): CredentialService =
        CredentialService(
            Files.createTempDirectory("comfyhub-keenable"),
            env = if (key == null) mapOf() else mapOf("COMFYHUB_CRED_keenable" to key),
        )

    companion object {
        private val DEFAULT_SEARCH_BODY = """
            {"query":"flux 提示词技巧","mode":"pro","results":[
              {"title":"Flux 提示词指南","url":"https://example.com/flux","snippet":"正文片段一","published_at":"2026-08-01"},
              {"title":"第二条","url":"https://example.com/2","snippet":"片段二"}
            ]}
        """.trimIndent().replace("\n", "")

        private val DEFAULT_FETCH_BODY = """
            {"url":"https://example.com/flux","title":"Flux 指南","content":"# 全文\n正文内容"}
        """.trimIndent().replace("\n", "")
    }

    // --- 搜索 -----------------------------------------------------------------

    @Test
    fun `免密钥搜索打 v1_search_public 并带 X_Keenable_Title 头`() = runBlocking {
        withKeenable { base, captured ->
            val client = KeenableSearch(credentials(), baseUrl = base)
            val outcome = client.search("flux 提示词技巧")

            val hits = captured().filter { it.path.endsWith("/search/public") }
            assertEquals(1, hits.size, "免密钥必须走 /public 路径：${captured().map { it.path }}")
            val hit = hits.single()
            assertEquals("POST", hit.method)
            assertTrue(!hit.keenableTitle.isNullOrBlank(), "缺 X-Keenable-Title 会被服务端 400 掉")
            assertEquals(null, hit.apiKey, "免密钥请求不该带 X-API-Key")
            assertTrue(hit.body.contains("\"query\""), hit.body)
            assertTrue(hit.body.contains("flux"), hit.body)

            assertEquals(2, outcome.results.size)
            assertTrue(outcome.context.contains("[1] Flux 提示词指南 (https://example.com/flux)"), outcome.context)
            assertTrue(outcome.context.contains("正文片段一"), outcome.context)
            assertEquals(true, (outcome.meta["keyless"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toBoolean())
        }
    }

    @Test
    fun `配置密钥后走 v1_search 并带 X_API_Key`() = runBlocking {
        withKeenable { base, captured ->
            val client = KeenableSearch(credentials("keen_test"), baseUrl = base)
            val outcome = client.search("任意查询")

            val hit = captured().single()
            assertEquals("/v1/search", hit.path, "带 key 时不能走 /public")
            assertEquals("keen_test", hit.apiKey)
            assertEquals(false, (outcome.meta["keyless"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toBoolean())
        }
    }

    @Test
    fun `429 免密钥时提示额度与填 key 的出路`() = runBlocking {
        withKeenable(searchStatus = 429, searchBody = """{"error":"quota"}""") { base, _ ->
            val client = KeenableSearch(credentials(), baseUrl = base)
            val err = runCatching { client.search("q") }.exceptionOrNull()

            assertTrue(err is ToolFailure, "应当抛 ToolFailure：$err")
            assertEquals("SEARCH_RATE_LIMIT", err!!.code)
            assertTrue(err.message!!.contains("1000"), "要说明免密钥额度：${err.message}")
        }
    }

    @Test
    fun `429 带密钥时报额度用尽 且绝不回显密钥`() = runBlocking {
        withKeenable(searchStatus = 429, searchBody = """{"error":"quota"}""") { base, _ ->
            val client = KeenableSearch(credentials("keen_secret_value"), baseUrl = base)
            val err = runCatching { client.search("q") }.exceptionOrNull()

            assertTrue(err is ToolFailure, "应当抛 ToolFailure：$err")
            assertEquals("SEARCH_RATE_LIMIT", err!!.code)
            assertTrue(err.message!!.contains("额度"), "带 key 时要说清是密钥额度用尽：${err.message}")
            assertTrue(!err.message!!.contains("keen_secret_value"), "绝不回显密钥：${err.message}")
        }
    }

    // --- 取正文 ---------------------------------------------------------------

    @Test
    fun `web_fetch 打 v1_fetch_public 并把目标 URL 编码进查询串`() = runBlocking {
        withKeenable { base, captured ->
            val client = KeenableSearch(credentials(), baseUrl = base)
            // 注意：URI 不接受字面空格/非 ASCII —— 真实场景里 URL 来自搜索结果（已编码）
            val page = client.page("https://example.com/page?id=42&lang=zh")

            val hit = captured().single()
            assertEquals("/v1/fetch/public", hit.path)
            assertEquals("GET", hit.method)
            assertTrue(!hit.keenableTitle.isNullOrBlank())
            assertTrue(hit.query!!.startsWith("url=https%3A%2F%2Fexample.com%2Fpage"), hit.query)
            assertEquals("Flux 指南", page.title)
            assertTrue(page.content.contains("正文内容"), page.content)
        }
    }

    @Test
    fun `web_fetch 拒绝内网与本机目标 且一个请求都不发`() = runBlocking {
        withKeenable { base, captured ->
            val client = KeenableSearch(credentials(), baseUrl = base)
            for (target in listOf("http://127.0.0.1:9222/x", "http://192.168.1.1/x", "http://169.254.169.254/x")) {
                val err = runCatching { client.page(target) }.exceptionOrNull()
                assertTrue(err is ToolFailure, "$target 应当被拒：$err")
                assertEquals("FETCH_TARGET_DENIED", (err as ToolFailure).code)
            }
            assertEquals(0, captured().size, "被拒的目标一个请求都不该发出去")
        }
    }
}
