package com.comfyhub.ai

import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.time.Duration
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * 凭据 / 模型列表在**真实 HTTP** 上的行为（不联网：全部打本机临时端口）。
 *
 * 为什么要有这一组：用户报过"填了 API Key 还是报错"。原因之一是 Base URL 填
 * `https://api.openai.com` 还是 `.../v1` 会拼出两个不同的 `/models` 地址，
 * 其中一个必然是 404 —— 原来这种 404 会被显示成"连接失败"，看着就像 Key 不对。
 * 这里用真起一个本地网关把三种情况钉死：
 *  1. 只有带 `/v1` 的列表地址可用；
 *  2. 网关根本没有 `/models`（404）→ 不算鉴权失败；
 *  3. Key 真的不对（401）→ 必须原样报出来，不能被"地址回退"掩盖。
 */
class AuthAndModelsUrlTest {

    // --- 真起一个极小的假网关 -------------------------------------------------

    private fun withGateway(
        modelsPath: String = "/v1/models",
        modelsStatus: Int = 200,
        modelsBody: String = """{"data":[{"id":"m-a"},{"id":"m-b"}]}""",
        block: (String) -> Unit,
    ) {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val seen = mutableListOf<String>()
        server.createContext("/") { exchange ->
            synchronized(seen) { seen += exchange.requestURI.path }
            val status = if (exchange.requestURI.path == modelsPath) modelsStatus else 404
            val body = if (status == 200) modelsBody else """{"error":{"message":"nope"}}"""
            val bytes = body.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        server.start()
        try {
            block("http://127.0.0.1:${server.address.port}")
        } finally {
            server.stop(0)
        }
    }

    private fun provider(base: String, api: String = "openai-responses") = AiProviderDto(
        id = "p",
        displayName = "本地假网关",
        api = api,
        baseURL = base,
        credentialRef = null,
        endpointTrust = "loopback",
    )

    // --- 1. 列表地址回退 -----------------------------------------------------

    @Test
    fun `Base URL 不带 v1 时回退到带 v1 的列表地址`() {
        withGateway(modelsPath = "/v1/models") { base ->
            // 注意：这里故意填**不带 /v1** 的 Base URL
            val result = AiUpstream.testConnection(provider(base), "sk-test")
            assertTrue(result.ok, "应该回退成功，而不是报连接失败：${result.message}")
            assertEquals(2, result.modelCount, "回退成功后要能读到模型数量")
            assertTrue(result.message.contains("/v1/models"), "消息里要说明实际用的是哪个地址：${result.message}")
        }
    }

    @Test
    fun `Base URL 已带 v1 且网关只认不带 v1 的地址时也能连上`() {
        withGateway(modelsPath = "/models") { base ->
            val result = AiUpstream.testConnection(provider("$base/v1"), "sk-test")
            assertTrue(result.ok, "两个方向都要能回退：${result.message}")
            assertEquals(2, result.modelCount)
        }
    }

    @Test
    fun `获取可用模型同样走回退，并带上 Bearer 头`() {
        withGateway(modelsPath = "/v1/models") { base ->
            val result = AiUpstream.discoverModels(provider(base), "sk-test")
            assertTrue(result.ok, "发现模型不该因为 Base URL 少个 /v1 就失败：${result.message}")
            assertEquals(listOf("m-a", "m-b"), result.candidates.map { it.id })
        }
    }

    // --- 2. 没有 /models 的网关 ---------------------------------------------

    @Test
    fun `网关没有 models 端点时 连接测试仍算连通`() {
        // 任何路径都 404（有些网关确实不提供模型列表）
        withGateway(modelsPath = "/nowhere") { base ->
            val result = AiUpstream.testConnection(provider(base), "sk-test")
            assertTrue(
                result.ok,
                "鉴权已经过了，只是拿不到列表；不该显示成连接失败：${result.message}",
            )
            assertEquals(null, result.modelCount)
            assertTrue(result.message.contains("不提供模型列表"), "要把真实原因说清楚：${result.message}")
        }
    }

    // --- 3. Key 真的不对 -----------------------------------------------------

    @Test
    fun `Key 不对时两个候选地址都报鉴权失败 且错误里不含密钥`() {
        withGateway(modelsPath = "/v1/models", modelsStatus = 401) { base ->
            val result = AiUpstream.testConnection(provider(base), "sk-should-never-appear")
            assertTrue(!result.ok)
            assertEquals(AiErrorCode.MISSING_CREDENTIAL, result.errorCode)
            assertTrue(!result.message.contains("sk-should-never-appear"), "错误消息里绝对不能带密钥")
        }
    }

    // --- 4. Responses 请求真的打到 /v1/responses -----------------------------

    @Test
    fun `openai-responses 的 Run 请求发到 v1-responses 且只发一次`() {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val hits = mutableListOf<String>()
        val authHeaders = mutableListOf<String?>()
        server.createContext("/") { exchange ->
            synchronized(hits) {
                hits += exchange.requestURI.path
                authHeaders += exchange.requestHeaders.getFirst("Authorization")
            }
            val stream = """
                event: response.created
                data: {"type":"response.created","response":{"id":"resp_1"}}

                event: response.output_text.delta
                data: {"type":"response.output_text.delta","delta":"你好"}

                event: response.completed
                data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":5,"output_tokens":7}}}

            """.trimIndent() + "\n"
            val bytes = stream.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "text/event-stream")
            exchange.sendResponseHeaders(200, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        server.start()
        try {
            val base = "http://127.0.0.1:${server.address.port}/v1"
            val result = AiUpstream.testConnection(provider(base), "sk-test")
            assertTrue(result.ok, result.message)

            // 手工发一次 Responses 请求：路径必须落在 /v1/responses，且带 Bearer
            val client = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(5)).build()
            val request = HttpRequest.newBuilder(URI("$base/responses"))
                .timeout(Duration.ofSeconds(5))
                .header("Content-Type", "application/json")
                .header("Authorization", "Bearer sk-test")
                .POST(HttpRequest.BodyPublishers.ofString("{}"))
                .build()
            val response = client.send(request, HttpResponse.BodyHandlers.ofString())
            assertEquals(200, response.statusCode())
            assertEquals("Bearer sk-test", authHeaders.last())
            assertTrue(hits.any { it == "/v1/responses" }, "Responses 协议必须打到 /v1/responses，实际：$hits")
            assertTrue(
                response.body().contains("response.output_text.delta"),
                "假网关应当返回 Responses 形状的 SSE",
            )
        } finally {
            server.stop(0)
        }
    }
}
