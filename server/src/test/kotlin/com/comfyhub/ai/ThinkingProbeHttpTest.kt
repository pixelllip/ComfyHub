package com.comfyhub.ai

import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * 思考档位校准（TESTED）的 HTTP 层，用 JDK 自带 HttpServer 起一个**本机假网关**，
 * 盯四条：
 *
 *  1. 从最高档往下试，第一个 2xx 就是"最高接受档"；写回的阶梯 = off + 已接受的各档；
 *  2. 4xx 且错误文案**提到**思考参数 → 降档；与思考参数无关的 400 → 如实中止（绝不误判）；
 *  3. 严格网关只认 `max_completion_tokens` 时按错误提示换字段重试一次；
 *  4. anthropic 协议的 Provider 诚实跳过，不装懂。
 */
class ThinkingProbeHttpTest {

    private val received = AtomicReference<List<String>>(emptyList())

    private val server: HttpServer = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0).apply {
        createContext("/") { exchange ->
            val body = exchange.requestBody.readAllBytes().decodeToString()
            val effort = Regex("\"reasoning_effort\"\\s*:\\s*\"([^\"]+)\"").find(body)?.groupValues?.get(1)
            val tokenField = listOf("max_completion_tokens", "max_tokens").firstOrNull { body.contains("\"$it\"") }
            received.getAndUpdate { it + listOf("effort=$effort token=$tokenField") }
            val (status, resp) = behavior.get()(effort, tokenField)
            val bytes = resp.toByteArray()
            exchange.sendResponseHeaders(status, if (bytes.isEmpty()) -1 else bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
        start()
    }

    /** 测试用例往这里塞行为；默认全接受。 */
    private val behavior = AtomicReference<(String?, String?) -> Pair<Int, String>> { _, _ ->
        200 to """{"id":"1"}"""
    }

    fun setBehavior(f: (String?, String?) -> Pair<Int, String>) {
        behavior.set(f)
    }

    @AfterTest
    fun stop() {
        server.stop(0)
        ModelCapabilityCatalog.reload(null)
    }

    private fun provider(api: String = "openai-completions", base: String) = AiProviderDto(
        id = "fake", displayName = "假网关", api = api, baseURL = base,
        endpointTrust = "loopback",
    )

    private fun model(efforts: Map<String, String> = mapOf("off" to "none", "low" to "low", "high" to "high", "max" to "max")) =
        AiModelDto(
            providerId = "fake", id = "test-model", displayName = "测试模型",
            reasoning = true, thinkingEfforts = efforts, capabilitySource = "builtin",
        )

    @Test
    fun `从最高档往下试，第一个通过的就是最高接受档`() {
        // 声明了 {off, low, high, max} → 只试声明过的 [max, high, low]；
        // max 不认（错误提到 reasoning_effort）→ 降档 high 通过
        setBehavior { effort, _ ->
            if (effort == "max") 400 to """{"error":{"message":"Invalid reasoning_effort: max"}}""" else 200 to """{"id":"1"}"""
        }
        val base = "http://127.0.0.1:${server.address.port}/v1"
        val outcome = AiUpstream.probeThinkingLadder(provider(base = base), model(), secret = null)
        assertTrue(outcome.ok, outcome.message)
        assertEquals("high", outcome.acceptedTop)
        // 阶梯 = off + 已接受档及其以下；被拒的 max 绝不写回
        assertEquals(setOf("off", "high", "low"), outcome.ladder!!.keys)
        assertFalse(outcome.ladder.keys.contains("max"))
        assertTrue(outcome.attempts >= 3, "基准 + max + high（${outcome.attempts}）")
    }

    @Test
    fun `与思考参数无关的 400 会如实中止而不是降档`() {
        // 基准（不带档位）放行；一带 reasoning_effort 就报"模型不存在"——
        // 这种 400 与思考参数无关，必须如实中止，绝不能当成"网关不认这一档"继续降档
        setBehavior { effort, _ ->
            if (effort == null) 200 to """{"id":"1"}""" else 400 to """{"error":{"message":"model not found: test-model"}}"""
        }
        val base = "http://127.0.0.1:${server.address.port}/v1"
        val outcome = AiUpstream.probeThinkingLadder(provider(base = base), model(), secret = null)
        assertFalse(outcome.ok)
        assertTrue(outcome.message.contains("与思考参数无关"), outcome.message)
        assertEquals(2, outcome.attempts)   // 基准 + 一次 max
    }

    @Test
    fun `严格网关只认 max_completion_tokens 时按提示换字段重试`() {
        setBehavior { _, tokenField ->
            if (tokenField == "max_tokens") {
                400 to """{"error":{"message":"Use 'max_completion_tokens' instead of 'max_tokens'."}}"""
            } else {
                200 to """{"id":"1"}"""
            }
        }
        val base = "http://127.0.0.1:${server.address.port}/v1"
        val outcome = AiUpstream.probeThinkingLadder(provider(base = base), model(), secret = null)
        assertTrue(outcome.ok, outcome.message)
        assertEquals("max", outcome.acceptedTop)
        val calls = received.get()
        assertTrue(calls.any { it.contains("token=max_tokens") }, calls.toString())
        assertTrue(calls.any { it.contains("token=max_completion_tokens") }, calls.toString())
    }

    @Test
    fun `anthropic 协议诚实跳过`() {
        val outcome = AiUpstream.probeThinkingLadder(
            provider(api = "anthropic-messages", base = "http://127.0.0.1:9"),
            model(), secret = null,
        )
        assertFalse(outcome.ok)
        assertTrue(outcome.message.contains("只支持 OpenAI 兼容"), outcome.message)
    }
}
