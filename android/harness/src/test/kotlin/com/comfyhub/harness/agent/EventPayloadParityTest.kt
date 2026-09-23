package com.comfyhub.harness.agent

import com.comfyhub.harness.event.RunEventType
import com.comfyhub.harness.store.MessagePartRecord
import com.comfyhub.harness.tools.ToolOutcome
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * **事件 payload 的两端一致性**（🔒 `docs/run-event-contract.md` §2 的下游）。
 *
 * 为什么这条用例最值钱：手机上跑一次 Run，事件由**手机**产生，却由**桌面那套 Flutter 界面**渲染
 * （"UI 代码两端共享、零分叉"）。payload 的键名一旦对不上，两端各自编译通过、各自的用例全绿，
 * 而界面上那个字段**永远是空的** —— 例如：
 *
 *  - `run.failed` 桌面发的是 `code` / `message`，手机若发 `errorCode` / `error`，
 *    手机上"生成失败"的提示就只剩一个空字符串；
 *  - `tool.requested` 桌面发 `arguments`，手机若发 `args`，工具卡里看不到参数；
 *  - `message.completed` 少了 `parts`，界面在 parts 非空时只渲染 parts → 消息"有正文没工具卡"。
 *
 * 这里的期望值**逐条抄自桌面代码**（`server/.../ai/HarnessRunner.kt` 的行号写在
 * [EventPayloads] 的 KDoc 表里），改任何一边都会让这条用例红。
 */
class EventPayloadParityTest {

    private fun keys(obj: JsonObject): Set<String> = obj.keys

    @Test
    fun `tool_requested 的键与桌面一致：arguments 而不是 args，且带 approval`() {
        val payload = EventPayloads.toolRequested("r1", "call_1", "web_fetch", "{\"url\":\"x\"}", "not_required")
        assertEquals(setOf("runId", "callId", "name", "arguments", "approval"), keys(payload))
        assertFalse("桌面与 Flutter 的 AiRunEvent.arguments 都读 arguments", keys(payload).contains("args"))
    }

    @Test
    fun `tool_started 的键与桌面一致`() {
        assertEquals(setOf("callId", "name"), keys(EventPayloads.toolStarted("call_1", "web_fetch")))
    }

    @Test
    fun `tool_completed 的键与桌面一致：elapsedMs 与 preview 不能少`() {
        val ok = EventPayloads.toolCompleted("call_1", "remember", 12L, "已记住", buildJsonObject { })
        assertEquals(setOf("callId", "name", "elapsedMs", "preview", "result"), keys(ok))
        // 没有结构化结果时**不发** result（而不是发个 null）
        assertEquals(
            setOf("callId", "name", "elapsedMs", "preview"),
            keys(EventPayloads.toolCompleted("call_1", "remember", 12L, "已记住", null)),
        )
    }

    @Test
    fun `tool_failed 的键与桌面一致：code 与 message`() {
        val bad = EventPayloads.toolFailed("call_1", "load_skill", "SKILL_NOT_FOUND", "没有这个 skill", "not_required", 3L)
        assertEquals(setOf("callId", "name", "code", "message", "approval", "elapsedMs"), keys(bad))
    }

    @Test
    fun `forToolOutcome 按 ok 分流，且失败信息带稳定错误码`() {
        val (okType, okBody) = EventPayloads.forToolOutcome(
            "call_1", "remember", ToolOutcome.success("已记住", buildJsonObject { put("queueId", "q1") }), 5L,
        )
        assertEquals(RunEventType.TOOL_COMPLETED, okType)
        assertTrue(okBody.containsKey("result"))

        val (failType, failBody) = EventPayloads.forToolOutcome(
            "call_2", "load_skill", ToolOutcome.failure("SKILL_NOT_FOUND", "没有这个 skill"), 7L,
        )
        assertEquals(RunEventType.TOOL_FAILED, failType)
        assertEquals("SKILL_NOT_FOUND", failBody["code"]!!.toString().trim('"'))
        assertEquals("没有这个 skill", failBody["message"]!!.toString().trim('"'))
    }

    @Test
    fun `message_completed 带 parts 与 usage（界面在 parts 非空时只渲染 parts）`() {
        val parts = listOf(
            MessagePartRecord("m1", 0, "reasoning", "想一想", null, null),
            MessagePartRecord("m1", 1, "text", "正文", null, null),
            MessagePartRecord("m1", 2, "tool_call", null, "call_1", "[{\"id\":\"call_1\"}]"),
            MessagePartRecord("m1", 3, "tool_result", "结果", "call_1", "{\"mediaIds\":[7]}"),
        )
        val payload = EventPayloads.messageCompleted(
            messageId = "m1",
            text = "正文",
            reasoning = "想一想",
            finishReason = "stop",
            elapsedMs = 100,
            reasoningEffort = "high",
            steps = 2,
            parts = parts,
            usage = EventPayloads.usage(10, 20),
        )
        assertEquals(
            setOf("messageId", "text", "reasoning", "finishReason", "elapsedMs", "reasoningEffort", "steps", "parts", "usage"),
            keys(payload),
        )
        // parts 的字段名与桌面 AiMessagePartDto 一致
        val first = (payload["parts"] as kotlinx.serialization.json.JsonArray)[0] as JsonObject
        assertEquals(setOf("type", "text", "toolCallId", "jsonPayload"), keys(first))
        assertEquals(4, (payload["parts"] as kotlinx.serialization.json.JsonArray).size)
    }

    @Test
    fun `run_failed 用 code 与 message（不是 errorCode 与 error）`() {
        val payload = EventPayloads.runFailed("r1", "RATE_LIMIT", "被限流（HTTP 429）")
        assertEquals(setOf("runId", "code", "message"), keys(payload))
    }

    @Test
    fun `run_completed 带 elapsedMs，run_cancelled 只带 runId`() {
        assertEquals(setOf("runId", "elapsedMs"), keys(EventPayloads.runCompleted("r1", 42L)))
        assertEquals(setOf("runId"), keys(EventPayloads.runCancelled("r1")))
    }

    @Test
    fun `usage 同时认 OpenAI 与 Anthropic 的字段名`() {
        val openAi = EventPayloads.usageOf(
            kotlinx.serialization.json.Json.parseToJsonElement(
                """{"prompt_tokens":11,"completion_tokens":22,"reasoning_tokens":3}"""
            )
        )
        assertEquals(setOf("inputTokens", "outputTokens", "cachedTokens", "reasoningTokens"), keys(openAi))
        assertEquals("11", openAi["inputTokens"]!!.toString())
        assertEquals("22", openAi["outputTokens"]!!.toString())
        assertEquals("3", openAi["reasoningTokens"]!!.toString())

        val anthropic = EventPayloads.usageOf(
            kotlinx.serialization.json.Json.parseToJsonElement(
                """{"input_tokens":5,"output_tokens":7,"cache_read_input_tokens":2}"""
            )
        )
        assertEquals("5", anthropic["inputTokens"]!!.toString())
        assertEquals("7", anthropic["outputTokens"]!!.toString())
        assertEquals("2", anthropic["cachedTokens"]!!.toString())

        // 上游没给 usage：如实给 0，不许编
        assertEquals("0", EventPayloads.usageOf(null)["inputTokens"]!!.toString())
    }

    @Test
    fun `工具预览压成一行并截断（与桌面 preview 同口径）`() {
        assertEquals("a b c", EventPayloads.previewOf("a\n  b\tc"))
        val long = EventPayloads.previewOf("字".repeat(300), max = 200)
        assertEquals(201, long.length)
        assertTrue(long.endsWith("…"))
    }
}
