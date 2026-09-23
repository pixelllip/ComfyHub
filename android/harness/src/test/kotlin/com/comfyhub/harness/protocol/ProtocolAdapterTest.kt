package com.comfyhub.harness.protocol

import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 协议契约测试（移植自桌面 `com.comfyhub.ai.protocol.ProtocolAdapterTest`）：
 * SSE 分帧、方言兼容、请求体构造。全部离线，不访问任何真实供应商。
 *
 * 为什么手机侧要**照搬同一组用例**（总纲 §6 的 R6 行）：手机有自己的一份解析实现，
 * "桌面测过的写法手机也必须认"这件事，只有同一组断言能保证。
 */
class ProtocolAdapterTest {

    private fun feed(acc: SseAccumulator, text: String): List<SseAccumulator.Frame> {
        val frames = mutableListOf<SseAccumulator.Frame>()
        text.split("\n").forEach { line -> acc.line(line)?.let { frames += it } }
        acc.flush()?.let { frames += it }
        return frames
    }

    // --- SSE 分帧 ----------------------------------------------------------

    @Test
    fun `空行结束一个事件 多行 data 用换行拼接`() {
        val acc = SseAccumulator()
        val frames = feed(
            acc,
            """
            event: message
            data: line1
            data: line2

            """.trimIndent() + "\n"
        )
        assertEquals(1, frames.size)
        assertEquals("message", frames[0].event)
        assertEquals("line1\nline2", frames[0].data)
    }

    @Test
    fun `注释行与心跳忽略 不影响事件边界`() {
        val acc = SseAccumulator()
        val frames = feed(acc, ": keep-alive\ndata: {\"a\":1}\n\n")
        assertEquals(1, frames.size)
        assertEquals("{\"a\":1}", frames[0].data)
    }

    @Test
    fun `CRLF 与无冒号字段都能处理`() {
        val acc = SseAccumulator()
        val frames = feed(acc, "data: hello\r\n\r\n")
        assertEquals(1, frames.size)
        assertEquals("hello", frames[0].data)

        // data 与值之间可以有多个空格，只去掉一个
        val acc2 = SseAccumulator()
        val frames2 = feed(acc2, "data:  two-spaces\n\n")
        assertEquals(" two-spaces", frames2[0].data)
    }

    @Test
    fun `半截事件不会提前吐出`() {
        val acc = SseAccumulator()
        assertNull(acc.line("data: {\"partial\":"))
        assertNull(acc.line("data: 1}"))
        val frame = acc.line("")
        assertEquals("{\"partial\":\n1}", frame?.data)
    }

    // --- OpenAI Chat Completions -------------------------------------------

    private val openai = OpenAiCompletionsAdapter()

    @Test
    fun `openai 请求体包含 model messages stream 与 usage 选项`() {
        val body = openai.buildBody(
            "gpt-4o-mini",
            listOf(ChatTurn("system", "你是助手"), ChatTurn("user", "你好")),
            stream = true,
        )
        assertEquals("gpt-4o-mini", body["model"]!!.jsonPrimitive.content)
        assertEquals(true, body["stream"]!!.jsonPrimitive.content.toBoolean())
        val messages = body["messages"]!!.jsonArray
        assertEquals(2, messages.size)
        assertEquals("system", messages[0].jsonObject["role"]!!.jsonPrimitive.content)
        assertEquals("你好", messages[1].jsonObject["content"]!!.jsonPrimitive.content)
        assertTrue(body.containsKey("stream_options"))
    }

    @Test
    fun `openai 解析 delta 文本 与 reasoning 方言`() {
        val frames = feed(
            SseAccumulator(),
            """
            data: {"id":"chatcmpl-1","choices":[{"delta":{"content":"你"},"finish_reason":null}]}

            data: {"choices":[{"delta":{"reasoning_content":"思考中"},"finish_reason":null}]}

            data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"total_tokens":12}}

            data: [DONE]

            """.trimIndent() + "\n"
        )
        val events = frames.flatMap { openai.interpret(it) }
        val text = events.filterIsInstance<StreamEvent.TextDelta>().joinToString("") { it.text }
        val reasoning = events.filterIsInstance<StreamEvent.ReasoningDelta>().joinToString("") { it.text }
        assertEquals("你", text)
        assertEquals("思考中", reasoning)
        assertEquals("chatcmpl-1", events.filterIsInstance<StreamEvent.ProviderId>().first().id)
        assertEquals(1, events.filterIsInstance<StreamEvent.Usage>().size)
        // [DONE] 不产生任何事件
        assertTrue(openai.interpret(SseAccumulator.Frame(null, "[DONE]")).isEmpty())
    }

    @Test
    fun `openai 遇到畸形 JSON 不抛异常 只是没有内容`() {
        assertTrue(openai.interpret(SseAccumulator.Frame(null, "{not json")).isEmpty())
        assertTrue(openai.interpret(SseAccumulator.Frame(null, "")).isEmpty())
    }

    @Test
    fun `openai 兼容 message 字段的网关`() {
        val events = openai.interpret(
            SseAccumulator.Frame(null, """{"choices":[{"message":{"content":"完整消息"}}]}""")
        )
        assertEquals("完整消息", (events.single() as StreamEvent.TextDelta).text)
    }

    // --- Anthropic ---------------------------------------------------------

    private val anthropic = AnthropicMessagesAdapter()

    @Test
    fun `anthropic 把 system 提到顶层 并带上 max_tokens`() {
        val body = anthropic.buildBody(
            "claude-3-5-sonnet",
            listOf(ChatTurn("system", "规则"), ChatTurn("user", "嗨"), ChatTurn("assistant", "在")),
            stream = true,
        )
        assertEquals("规则", body["system"]!!.jsonPrimitive.content)
        assertEquals("system 不能留在 messages 里", 2, body["messages"]!!.jsonArray.size)
        assertTrue(body.containsKey("max_tokens"))
    }

    @Test
    fun `anthropic 解析 content_block_delta 与 message_start`() {
        val frames = feed(
            SseAccumulator(),
            """
            event: message_start
            data: {"type":"message_start","message":{"id":"msg_1"}}

            event: content_block_delta
            data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"你"}}

            event: message_delta
            data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

            """.trimIndent() + "\n"
        )
        val events = frames.flatMap { anthropic.interpret(it) }
        assertEquals("msg_1", events.filterIsInstance<StreamEvent.ProviderId>().single().id)
        assertEquals("你", events.filterIsInstance<StreamEvent.TextDelta>().single().text)
        assertEquals(1, events.filterIsInstance<StreamEvent.Usage>().size)
    }

    @Test
    fun `anthropic 的 error 事件转成协议错误`() {
        assertThrows(ProtocolFailure::class.java) {
            anthropic.interpret(
                SseAccumulator.Frame("error", """{"type":"error","error":{"message":"overloaded"}}""")
            )
        }
    }

    // --- OpenAI Responses ---------------------------------------------------

    @Test
    fun `openai-responses 请求结构：system 提到 instructions、输入包成 content 数组`() {
        val responses = OpenAiResponsesAdapter()
        val body = responses.buildBody(
            "gpt-5",
            listOf(
                ChatTurn("system", "你是助手"),
                ChatTurn("user", "你好"),
                ChatTurn("assistant", "在的"),
                ChatTurn("user", "继续"),
            ),
            stream = true,
            reasoning = ReasoningRequest.NONE,
        )

        // system 不能留在 input 里
        assertEquals("你是助手", body["instructions"]?.jsonPrimitive?.content)
        val input = body["input"]!!.jsonArray
        assertEquals("system 之后的三轮才进 input", 3, input.size)
        assertEquals("user", input[0].jsonObject["role"]?.jsonPrimitive?.content)
        assertEquals(
            "input_text",
            input[0].jsonObject["content"]!!.jsonArray[0].jsonObject["type"]?.jsonPrimitive?.content,
        )
        assertEquals(
            "助手历史用 output_text",
            "output_text",
            input[1].jsonObject["content"]!!.jsonArray[0].jsonObject["type"]?.jsonPrimitive?.content,
        )
        assertEquals(true, body["stream"]?.jsonPrimitive?.content?.toBoolean())
        assertEquals("不做服务端存储", "false", body["store"]?.jsonPrimitive?.content)
        // 没声明推理能力时一个思考字段都不发
        assertNull(body["reasoning"])
    }

    @Test
    fun `openai-responses 思考强度落到 reasoning-effort，不偷偷降级`() {
        val responses = OpenAiResponsesAdapter()
        fun effortOf(r: ReasoningRequest) = responses.buildBody("gpt-5", emptyList(), true, r)["reasoning"]

        val high = effortOf(
            ReasoningRequest(ReasoningEffort.HIGH, "high", ThinkingFormat.OPENAI)
        )!!.jsonObject
        assertEquals("high", high["effort"]?.jsonPrimitive?.content)
        assertEquals(
            "不带 summary 网关不下发思考过程",
            "auto",
            high["summary"]?.jsonPrimitive?.content,
        )

        // 低/中/极高都按原样发（"极高"是前沿模型真实存在的档位）
        assertEquals(
            "minimal",
            effortOf(ReasoningRequest(ReasoningEffort.MINIMAL, "minimal", ThinkingFormat.OPENAI))!!
                .jsonObject["effort"]?.jsonPrimitive?.content,
        )
        assertEquals(
            "xhigh",
            effortOf(ReasoningRequest(ReasoningEffort.XHIGH, "xhigh", ThinkingFormat.OPENAI))!!
                .jsonObject["effort"]?.jsonPrimitive?.content,
        )
        // max 也照发：只有模型真的声明过 max 才会走到这里，写错就让上游明确拒绝，
        // 总好过把用户的"最大"悄悄降成"高"
        assertEquals(
            "max",
            effortOf(ReasoningRequest(ReasoningEffort.MAX, "max", ThinkingFormat.OPENAI))!!
                .jsonObject["effort"]?.jsonPrimitive?.content,
        )

        // 关闭思考 = 不带 reasoning 字段
        assertNull(effortOf(ReasoningRequest.NONE))
    }

    @Test
    fun `openai-responses 解析 output_text 增量、思考摘要与 completed 里的用量`() {
        val responses = OpenAiResponsesAdapter()
        val events = listOf(
            SseAccumulator.Frame(
                "response.created",
                """{"type":"response.created","response":{"id":"resp_1"}}""",
            ),
            SseAccumulator.Frame(
                "response.output_text.delta",
                """{"type":"response.output_text.delta","delta":"你"}""",
            ),
            SseAccumulator.Frame(
                "response.reasoning_summary_text.delta",
                """{"type":"response.reasoning_summary_text.delta","delta":"想一下"}""",
            ),
            SseAccumulator.Frame(
                "response.completed",
                """{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":5,"output_tokens":7}}}""",
            ),
        ).flatMap { responses.interpret(it) }

        assertEquals("你", events.filterIsInstance<StreamEvent.TextDelta>().single().text)
        assertEquals("想一下", events.filterIsInstance<StreamEvent.ReasoningDelta>().single().text)
        assertEquals("resp_1", events.filterIsInstance<StreamEvent.ProviderId>().last().id)
        assertEquals(1, events.filterIsInstance<StreamEvent.Usage>().size)
    }

    @Test
    fun `openai-responses 的失败事件转成协议错误`() {
        val responses = OpenAiResponsesAdapter()
        assertThrows(ProtocolFailure::class.java) {
            responses.interpret(
                SseAccumulator.Frame(
                    "response.failed",
                    """{"type":"response.failed","response":{"error":{"message":"boom"}}}""",
                )
            )
        }
        // 只有 type、没有 event 行的网关也要认
        assertThrows(ProtocolFailure::class.java) {
            responses.interpret(
                SseAccumulator.Frame(null, """{"type":"error","message":"rate limited"}""")
            )
        }
    }

    @Test
    fun `openai-responses 已实现：进入 supported 名单`() {
        assertTrue(Adapters.supported().contains(AiApiRef.OPENAI_RESPONSES))
        assertTrue(Adapters.supported().contains(AiApiRef.OPENAI_COMPLETIONS))
        assertEquals("openai-responses/3", Adapters.of(AiApiRef.OPENAI_RESPONSES)!!.adapterVersion)
    }

    @Test
    fun `三种协议都只声明了图片内联 视频音频文档仍然阻断`() {
        listOf(
            AiApiRef.OPENAI_COMPLETIONS,
            AiApiRef.ANTHROPIC_MESSAGES,
            AiApiRef.OPENAI_RESPONSES,
        ).forEach { api ->
            val adapter = Adapters.of(api)!!
            val image = adapter.attachmentTransports[AttachmentKindRef.IMAGE].orEmpty()
            assertEquals("$api 的图片内联", setOf(TransportRef.INLINE_BASE64), image)
            // 视频 / 音频 / 文档：一个都没实现，预检据此阻断（不做不透明的抽帧降级）
            listOf(AttachmentKindRef.VIDEO, AttachmentKindRef.AUDIO, AttachmentKindRef.DOCUMENT).forEach { kind ->
                assertTrue(
                    "$api 不应该声称实现了 ${kind.wire}",
                    adapter.attachmentTransports[kind].orEmpty().isEmpty(),
                )
            }
        }
        assertNull("未知协议不能被解析成任何适配器", AiApiRef.parse("nope"))
    }

    // --- 工具调用分片（R11/R12 的下游：工具循环靠它） -----------------------

    @Test
    fun `三种协议的工具调用分片都能拼成完整调用`() {
        // OpenAI：第一片带 id/name，第二片只有 arguments
        val openAiAcc = ToolCallAccumulator()
        openAiAcc.apply(StreamEvent.ToolCallDelta(0, "call_1", "call_1", "web_search"))
        openAiAcc.apply(StreamEvent.ToolCallDelta(0, null, null, null, "{\"q\":"))
        openAiAcc.apply(StreamEvent.ToolCallDelta(0, null, null, null, "\"x\"}"))
        val openAiDraft = openAiAcc.drafts().single()
        assertEquals("call_1", openAiDraft.callId)
        assertEquals("web_search", openAiDraft.name)
        assertEquals("{\"q\":\"x\"}", openAiDraft.arguments)

        // Anthropic：start 给 id/name，delta 只给 partial_json（且不带 key）
        val anthropicAcc = ToolCallAccumulator()
        anthropicAcc.apply(StreamEvent.ToolCallDelta(1, "toolu_1", "toolu_1", "remember"))
        anthropicAcc.apply(StreamEvent.ToolCallDelta(1, null, null, null, "{\"text\":\"hi\"}"))
        val anthropicDraft = anthropicAcc.drafts().single()
        assertEquals("toolu_1", anthropicDraft.callId)
        assertEquals("remember", anthropicDraft.name)
        assertEquals("{\"text\":\"hi\"}", anthropicDraft.arguments)

        // Responses：流内键是 item.id，回传的是 call_id
        val responsesAcc = ToolCallAccumulator()
        responsesAcc.apply(StreamEvent.ToolCallDelta(0, "item_1", "call_9", "web_fetch"))
        responsesAcc.apply(StreamEvent.ToolCallDelta(0, "item_1", null, null, "{}"))
        val responsesDraft = responsesAcc.drafts().single()
        assertEquals("回传给上游的必须是 call_id", "call_9", responsesDraft.callId)
        assertEquals("web_fetch", responsesDraft.name)

        assertTrue(ToolCallAccumulator().isEmpty())
    }

    @Test
    fun `附件内联：图片按协议各自的结构发出去，未实现的模态直接抛`() {
        val image = ChatAttachment(AttachmentKindRef.IMAGE, "image/png", "QUJD")
        val video = ChatAttachment(AttachmentKindRef.VIDEO, "video/mp4", "QUJD")

        val openAiTurn = ChatTurn("user", "看图", attachments = listOf(image))
        val openAiBody = openai.buildBody("m", listOf(openAiTurn), true)
        val content = openAiBody["messages"]!!.jsonArray[0].jsonObject["content"]!!.jsonArray
        assertEquals("image_url", content[1].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals(
            "data:image/png;base64,QUJD",
            content[1].jsonObject["image_url"]!!.jsonObject["url"]!!.jsonPrimitive.content,
        )

        val anthropicBody = anthropic.buildBody(
            "m", listOf(ChatTurn("user", "看图", attachments = listOf(image))), true,
        )
        val blocks = anthropicBody["messages"]!!.jsonArray[0].jsonObject["content"]!!.jsonArray
        assertEquals("image", blocks[0].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals(
            "Anthropic 要裸 base64，不带 data: 前缀",
            "QUJD",
            blocks[0].jsonObject["source"]!!.jsonObject["data"]!!.jsonPrimitive.content,
        )

        // 未实现的模态：宁可整轮失败，也不许静默丢掉后假装发成功（红线 R9）
        assertThrows(UnsupportedContentFailure::class.java) {
            openai.buildBody("m", listOf(ChatTurn("user", "看视频", attachments = listOf(video))), true)
        }
        assertThrows(UnsupportedContentFailure::class.java) {
            anthropic.buildBody("m", listOf(ChatTurn("user", "看视频", attachments = listOf(video))), true)
        }
        assertThrows(UnsupportedContentFailure::class.java) {
            OpenAiResponsesAdapter().buildBody(
                "m", listOf(ChatTurn("user", "看视频", attachments = listOf(video))), true,
            )
        }
    }
}
