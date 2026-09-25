package com.comfyhub.ai.tools

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking

/**
 * Danbooru 客户端在**真实 HTTP** 上的行为（不联网：全部打本机临时端口）。
 *
 * 钉的是三条**实测出来、错了会静默误导模型**的事实：
 *  ① **必须带 User-Agent** —— 不带就 403（2026-09-25 用 curl / Invoke-WebRequest 都复现过），
 *     而 403 会被用户读成"danbooru 上没有这个角色"；
 *  ② **一次查询打哪几个请求**（精确 tag → 模糊 tag → wiki → 同现 tag），
 *     以及**wiki 404 是正常情况**（这个 tag 没 wiki 页，不是查失败）；
 *  ③ **404 / 403 / 429 各有各的稳定码**，正文不许把"没查成"说成"没有这个 tag"。
 *
 * 同现 tag（`related_tag.json`）是"规范 tag 栈"的真正来源：danbooru 上这个角色
 * 最常用的外观 / 服装 tag，按 frequency 排序 —— 用户要的就是它
 * （"除了专门 LoRA 内置的提示词外也要查查 danbooru"）。
 */
class DanbooruSearchTest {

    private data class Captured(
        val path: String,
        val query: String?,
        val userAgent: String?,
    )

    private suspend fun withDanbooru(
        handler: (path: String, query: String?) -> Pair<Int, String>,
        block: suspend (base: String, captured: () -> List<Captured>) -> Unit,
    ) {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val seen = mutableListOf<Captured>()
        server.createContext("/") { exchange: HttpExchange ->
            exchange.requestBody.readBytes()
            synchronized(seen) {
                seen += Captured(
                    path = exchange.requestURI.path,
                    query = exchange.requestURI.rawQuery,
                    userAgent = exchange.requestHeaders.getFirst("User-Agent"),
                )
            }
            val (status, body) = handler(exchange.requestURI.path, exchange.requestURI.rawQuery)
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

    @Test
    fun `查一个角色会打四类请求 每个都带 User-Agent`() = runBlocking {
        withDanbooru({ path, query ->
            when {
                path == "/tags.json" && query?.contains("*") == true -> 200 to FUZZY_TAGS
                path == "/tags.json" -> 200 to EXACT_TAG
                path.startsWith("/wiki_pages/") -> 200 to WIKI
                path == "/related_tag.json" -> 200 to RELATED
                else -> 404 to "{}"
            }
        }) { base, captured ->
            val client = DanbooruSearch(baseUrl = base, userAgent = DanbooruSearch.USER_AGENT)
            val outcome = client.lookup("shinku (nte)")

            val requests = captured()
            assertEquals(4, requests.size, "精确 + 模糊 + wiki + 同现，别多打")
            assertTrue(requests.any { it.path == "/tags.json" && it.query?.contains("shinku_%28nte%29") == true })
            assertTrue(requests.any { it.path == "/tags.json" && it.query?.contains("*shinku") == true }, "模糊那一趟要发（换装版本只在里面）")
            assertTrue(requests.any { it.path == "/wiki_pages/shinku_(nte).json" })
            assertTrue(requests.any { it.path == "/related_tag.json" && it.query?.contains("query=shinku_%28nte%29") == true })
            // ★ 这条是"不带 UA 就 403"的防线：任何一次请求漏了 UA 都会红
            assertTrue(
                requests.all { it.userAgent == DanbooruSearch.USER_AGENT },
                "每一次请求都必须带 User-Agent（不带被 403）：${requests.map { it.userAgent }}",
            )

            val character = outcome.meta.obj("character")!!
            assertEquals("shinku_(nte)", character.str("tag"))
            val variants = character["variants"]!!.toString()
            assertTrue(variants.contains("shinku_(dawn_patrol)_(nte)"), variants)
            assertTrue(outcome.text.contains("真红"), "别名要进正文（那是角色卡的 aliases 来源）")
            assertTrue(outcome.text.contains("horns"), "同现 tag 要进正文")
            assertTrue(outcome.text.contains("规范 tag 栈"), outcome.text)
            assertTrue(outcome.text.contains("换装"), outcome.text)
            // 核对时间：`save_character_card` 靠"这一轮真的查过"决定写不写它
            assertTrue(outcome.meta.str("verifiedAt") != null)
        }
    }

    @Test
    fun `精确 tag 不存在时说清楚 并给候选`() = runBlocking {
        // 顺带钉一条纪律：`related_tag` 404 只是"没有同现 tag"，不该把已经解析出来的
        // 角色 tag 一起丢掉（这里刻意让 related 走 404 分支）
        withDanbooru({ path, query ->
            when {
                path == "/tags.json" && query?.contains("*") == true -> 200 to FUZZY_TAGS
                path == "/tags.json" -> 200 to "[]"
                else -> 404 to "{}"
            }
        }) { base, _ ->
            val outcome = DanbooruSearch(baseUrl = base).lookup("shinku")
            assertTrue(outcome.text.contains("不存在"), outcome.text)
            assertTrue(outcome.text.contains("shinku_(nte)"), "候选要列出来：${outcome.text}")
            assertEquals("shinku_(nte)", outcome.meta.obj("character")!!.str("tag"))
            assertTrue(outcome.text.contains("同现 tag：没拿到"), "同现那一趟失败要如实说：${outcome.text}")
        }
    }

    @Test
    fun `一个角色都没解析出来时如实说 而不是编一个`() = runBlocking {
        withDanbooru({ _, _ -> 200 to GENERAL_ONLY }) { base, _ ->
            val outcome = DanbooruSearch(baseUrl = base).lookup("red_eyes")
            assertTrue(outcome.text.contains("没有解析出角色 tag"), outcome.text)
            assertTrue(outcome.text.contains("red_eyes"), outcome.text)
            assertTrue(outcome.meta.obj("character") == null)
        }
    }

    @Test
    fun `wiki 404 是正常情况 不算查失败`() = runBlocking {
        withDanbooru({ path, query ->
            when {
                path == "/tags.json" && query?.contains("*") == true -> 200 to FUZZY_TAGS
                path == "/tags.json" -> 200 to EXACT_TAG
                path.startsWith("/wiki_pages/") -> 404 to ""
                path == "/related_tag.json" -> 200 to RELATED
                else -> 404 to "{}"
            }
        }) { base, _ ->
            val outcome = DanbooruSearch(baseUrl = base).lookup("shinku_(nte)")
            assertEquals(true, outcome.meta.obj("character")!!["wikiMissing"]!!.toString().toBoolean())
            assertTrue(outcome.text.contains("没有 wiki 页"), outcome.text)
        }
    }

    @Test
    fun `403 与 429 映射成各自的稳定码 正文不许把没查成说成没有`() = runBlocking {
        withDanbooru({ _, _ -> 403 to "" }) { base, _ ->
            val e = assertFailsWith<ToolFailure> { DanbooruSearch(baseUrl = base).lookup("shinku") }
            assertEquals("DANBOORU_UA_REJECTED", e.code)
            assertTrue(e.message!!.contains("User-Agent"), e.message!!)
        }
        withDanbooru({ _, _ -> 429 to "" }) { base, _ ->
            val e = assertFailsWith<ToolFailure> { DanbooruSearch(baseUrl = base).lookup("shinku") }
            assertEquals("DANBOORU_RATE_LIMIT", e.code)
        }
    }

    /**
     * Cloudflare 挑战页必须**单独认出来**。
     *
     * 实测（2026-09-25）：UA 写得长一点就被 Cloudflare 用 403 + "Just a moment..." 挑战页拦下。
     * 报成 `DANBOORU_UA_REJECTED` 会让人以为"danbooru 不收我们的 UA"，
     * 报成 `DANBOORU_NOT_FOUND` 更糟 —— 直接让人以为**这个角色不存在**。
     */
    @Test
    fun `Cloudflare 挑战页有自己的错误码 不许说成没有这个 tag`() = runBlocking {
        val challenge = """
            <!DOCTYPE html><html><head><title>Just a moment...</title></head>
            <body><script src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1"></script>
            Enable JavaScript and cookies to continue</body></html>
        """.trimIndent()
        withDanbooru({ _, _ -> 403 to challenge }) { base, _ ->
            val e = assertFailsWith<ToolFailure> { DanbooruSearch(baseUrl = base).lookup("shinku") }
            assertEquals("DANBOORU_CLOUDFLARE", e.code)
            assertTrue(e.message!!.contains(DanbooruSearch.ENV_UA), "要告诉用户怎么自救：${e.message}")
            assertTrue(e.message!!.contains("不要"), e.message!!)
        }
        // 200 也可能是挑战页（Cloudflare 两种都出现过）
        withDanbooru({ _, _ -> 200 to challenge }) { base, _ ->
            val e = assertFailsWith<ToolFailure> { DanbooruSearch(baseUrl = base).lookup("shinku") }
            assertEquals("DANBOORU_CLOUDFLARE", e.code)
        }
    }

    @Test
    fun `User-Agent 默认取短的常量 但用户能用自己的环境变量覆盖`() {
        assertEquals(DanbooruSearch.USER_AGENT, DanbooruSearch.userAgent { null })
        assertEquals("MyApp/9.9", DanbooruSearch.userAgent { if (it == DanbooruSearch.ENV_UA) " MyApp/9.9 " else null })
        // 空串 = 没设（别把 UA 发成空字符串，那等于不带）
        assertEquals(DanbooruSearch.USER_AGENT, DanbooruSearch.userAgent { "   " })
    }

    /**
     * **真站抽查（opt-in）**：默认跳过（测试不出网、也不去打扰 danbooru）。
     *
     * ```powershell
     * $env:JAVA_HOME='D:\Program Files\Android Studio\jbr'
     * $env:COMFYHUB_LIVE_DANBOORU='1'
     * $env:COMFYHUB_HTTPS_PROXY='http://127.0.0.1:7890'   # 本机直连 danbooru 不稳，必须走代理
     * cd server; gradle test --tests "com.comfyhub.ai.tools.DanbooruSearchTest" --console=plain
     * ```
     *
     * 它钉的是夹具**永远看不出来**的那一类：User-Agent 被拒（403）、字段改名、
     * `related_tags` / `other_names` 没了 —— 接口形状一变，夹具照样全绿。
     */
    @Test
    fun `真站抽查 角色 tag 别名与同现 tag 都拿得到（opt-in）`() = runBlocking {
        org.junit.jupiter.api.Assumptions.assumeTrue(
            System.getenv("COMFYHUB_LIVE_DANBOORU") == "1",
            "默认不跑真站：设 COMFYHUB_LIVE_DANBOORU=1",
        )
        val outcome = DanbooruSearch().lookup("shinku (nte)", relatedLimit = 10)
        val character = outcome.meta.obj("character")
        assertEquals("shinku_(nte)", character?.str("tag"), outcome.text)
        assertTrue(
            character!!["relatedTags"]!!.toString().contains("horns"),
            "同现 tag 拿不到说明接口形状变了：${character["relatedTags"]}",
        )
        assertTrue(outcome.text.contains("Neverness"), "wiki 正文要能拿到：${outcome.text}")
    }

    @Test
    fun `tag 写法归一：空格变下划线 全角括号变半角 但不动下划线本身`() {        assertEquals("shinku_(nte)", DanbooruSearch.normalizeTag("  Shinku (NTE) "))
        assertEquals("shinku_(nte)", DanbooruSearch.normalizeTag("shinku（nte）"))
        assertEquals("shinku_(nte)", DanbooruSearch.normalizeTag("shinku_(nte)"))
        assertEquals("真红", DanbooruSearch.normalizeTag("真红"))
        // 分类码 2 目前没有定义 —— 别写成 0..5 的数组下标（会把 2 说成 copyright）
        assertEquals("character", DanbooruSearch.categoryName(4))
        assertEquals("copyright", DanbooruSearch.categoryName(3))
        assertEquals("unknown", DanbooruSearch.categoryName(2))
        assertEquals("unknown", DanbooruSearch.categoryName(null))
    }

    companion object {
        private val EXACT_TAG = """
            [{"id":2626738,"name":"shinku_(nte)","post_count":340,"category":4,"is_deprecated":false}]
        """.trimIndent()

        private val FUZZY_TAGS = """
            [
              {"id":2626738,"name":"shinku_(nte)","post_count":340,"category":4,"is_deprecated":false},
              {"id":2673775,"name":"shinku_(dawn_patrol)_(nte)","post_count":20,"category":4,"is_deprecated":false},
              {"id":1,"name":"shinku","post_count":5,"category":0,"is_deprecated":false}
            ]
        """.trimIndent()

        private val GENERAL_ONLY = """
            [
              {"id":2785,"name":"red_eyes","post_count":999999,"category":0,"is_deprecated":false},
              {"id":999,"name":"red_eyes_glow","post_count":12,"category":0,"is_deprecated":true}
            ]
        """.trimIndent()

        private val WIKI = """
            {
              "id":287381,
              "title":"shinku_(nte)",
              "body":"A character from [[Neverness to Everness]].\r\n\r\nh4. Appearance\r\n\r\n* horns",
              "other_names":["真红","真紅(NTE)"]
            }
        """.trimIndent()

        private val RELATED = """
            {
              "query":"shinku_(nte)",
              "post_count":340,
              "related_tags":[
                {"tag":{"id":2785,"name":"horns","post_count":695529,"category":0},"frequency":0.9735},
                {"tag":{"id":390728,"name":"multicolored_hair","post_count":1199367,"category":0},"frequency":0.9712}
              ]
            }
        """.trimIndent()
    }
}
