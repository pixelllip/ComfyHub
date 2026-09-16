package com.comfyhub.ai.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * 工具调用的**线格式**契约测试（M4，AIH-033~036）。
 *
 * 三家协议的差别不在字段名，而在**结构**：
 *  - OpenAI 兼容：工具调用是 assistant 消息里的 `tool_calls`，结果是一条 `role=tool` 的消息；
 *  - Anthropic：助手 `content` 变成块数组（`text` + `tool_use`），结果必须回在**紧随其后的 user 消息**里
 *    （连续的 `role=tool` 轮要合并成一条，否则上游直接 400）；
 *  - Responses：工具调用是**顶层 item**（`function_call` / `function_call_output`），用 `call_id` 关联。
 *
 * 全部离线，不访问任何真实供应商。
 */
class ToolProtocolTest {

    private val openai = OpenAiCompletionsAdapter()
    private val anthropic = AnthropicMessagesAdapter()
    private val responses = OpenAiResponsesAdapter()

    private val spec = ToolSpec(
        name = "write_file",
        description = "写文件",
        parameters = ToolSpec.EMPTY_PARAMS,
    )

    private val call = ToolCallRef("call_1", "write_file", """{"path":"comfyui/a.txt"}""")

    // --- OpenAI 兼容 Chat Completions --------------------------------------

    @Test
    fun `openai 助手轮的工具调用渲染成 tool_calls`() {
        val body = openai.buildBody(
            "gpt-4o",
            listOf(
                ChatTurn("system", "规则"),
                ChatTurn("user", "帮我写个文件"),
                ChatTurn("assistant", "", toolCalls = listOf(call)),
            ),
            stream = true,
        )
        val message = body["messages"]!!.jsonArray[2].jsonObject
        assertEquals("assistant", message["role"]!!.jsonPrimitive.content)
        val rendered = message["tool_calls"]!!.jsonArray.single().jsonObject
        assertEquals("call_1", rendered["id"]!!.jsonPrimitive.content)
        assertEquals("function", rendered["type"]!!.jsonPrimitive.content)
        val fn = rendered["function"]!!.jsonObject
        assertEquals("write_file", fn["name"]!!.jsonPrimitive.content)
        assertEquals("""{"path":"comfyui/a.txt"}""", fn["arguments"]!!.jsonPrimitive.content)
    }

    @Test
    fun `openai 工具结果渲染成 role=tool 加 tool_call_id`() {
        val body = openai.buildBody(
            "gpt-4o",
            listOf(ChatTurn("tool", "已写入 comfyui/a.txt", toolCallId = "call_1")),
            stream = false,
        )
        val message = body["messages"]!!.jsonArray.single().jsonObject
        assertEquals("tool", message["role"]!!.jsonPrimitive.content)
        assertEquals("call_1", message["tool_call_id"]!!.jsonPrimitive.content)
        assertEquals("已写入 comfyui/a.txt", message["content"]!!.jsonPrimitive.content)
    }

    @Test
    fun `openai tools 是 type=function 的嵌套结构 空列表则字段整体省略`() {
        val body = openai.buildBody("gpt-4o", listOf(ChatTurn("user", "hi")), true, tools = listOf(spec))
        val rendered = body["tools"]!!.jsonArray.single().jsonObject
        assertEquals("function", rendered["type"]!!.jsonPrimitive.content)
        val fn = rendered["function"]!!.jsonObject
        assertEquals("write_file", fn["name"]!!.jsonPrimitive.content)
        assertEquals("写文件", fn["description"]!!.jsonPrimitive.content)
        assertTrue(fn["parameters"] is JsonObject, "parameters 必须是 JSON Schema 对象")

        // 空列表 ≠ 发一个空数组：很多网关对 tools:[] 直接 400
        val empty = openai.buildBody("gpt-4o", listOf(ChatTurn("user", "hi")), true)
        assertTrue(!empty.containsKey("tools"), "没有工具时 tools 字段必须整个不出现")
    }

    // --- Anthropic Messages ------------------------------------------------

    @Test
    fun `anthropic 助手工具调用渲染成 content blocks 且 input 是解析后的对象`() {
        val body = anthropic.buildBody(
            "claude-sonnet-5",
            listOf(
                ChatTurn("system", "规则"),
                ChatTurn("user", "写文件"),
                ChatTurn("assistant", "好的", toolCalls = listOf(call)),
            ),
            stream = true,
        )
        assertEquals("规则", body["system"]!!.jsonPrimitive.content, "system 必须留在顶层")

        val turns = body["messages"]!!.jsonArray
        assertEquals(2, turns.size)
        val assistant = turns[1].jsonObject
        assertEquals("assistant", assistant["role"]!!.jsonPrimitive.content)

        val blocks = assistant["content"]!!.jsonArray
        assertEquals(2, blocks.size, "正文 + tool_use 两个块")
        val text = blocks[0].jsonObject
        assertEquals("text", text["type"]!!.jsonPrimitive.content)
        assertEquals("好的", text["text"]!!.jsonPrimitive.content)

        val use = blocks[1].jsonObject
        assertEquals("tool_use", use["type"]!!.jsonPrimitive.content)
        assertEquals("call_1", use["id"]!!.jsonPrimitive.content)
        assertEquals("write_file", use["name"]!!.jsonPrimitive.content)
        val input = use["input"]!!.jsonObject
        assertEquals("comfyui/a.txt", input["path"]!!.jsonPrimitive.content)
    }

    @Test
    fun `anthropic 连续的 tool 轮合并成一条 user 消息`() {
        val body = anthropic.buildBody(
            "claude-sonnet-5",
            listOf(
                ChatTurn("user", "并行做两件事"),
                ChatTurn(
                    "assistant", "",
                    toolCalls = listOf(
                        ToolCallRef("t1", "list_dir", "{}"),
                        ToolCallRef("t2", "read_file", "{}"),
                    ),
                ),
                ChatTurn("tool", "结果一", toolCallId = "t1"),
                ChatTurn("tool", "结果二", toolCallId = "t2"),
            ),
            stream = true,
        )
        val turns = body["messages"]!!.jsonArray
        assertEquals(3, turns.size, "两条 tool 轮必须合并，否则会出现连续两条 user 消息")

        val merged = turns[2].jsonObject
        assertEquals("user", merged["role"]!!.jsonPrimitive.content)
        val blocks = merged["content"]!!.jsonArray
        assertEquals(2, blocks.size)
        assertEquals("tool_result", blocks[0].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals("t1", blocks[0].jsonObject["tool_use_id"]!!.jsonPrimitive.content)
        assertEquals("结果一", blocks[0].jsonObject["content"]!!.jsonPrimitive.content)
        assertEquals("tool_result", blocks[1].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals("t2", blocks[1].jsonObject["tool_use_id"]!!.jsonPrimitive.content)
        assertEquals("结果二", blocks[1].jsonObject["content"]!!.jsonPrimitive.content)
    }

    @Test
    fun `anthropic tools 用 input_schema 且空列表整体省略`() {
        val body = anthropic.buildBody("claude", listOf(ChatTurn("user", "hi")), true, tools = listOf(spec))
        val rendered = body["tools"]!!.jsonArray.single().jsonObject
        assertEquals("write_file", rendered["name"]!!.jsonPrimitive.content)
        assertEquals("写文件", rendered["description"]!!.jsonPrimitive.content)
        assertTrue(rendered["input_schema"] is JsonObject, "Anthropic 的 schema 字段叫 input_schema")
        assertTrue(!rendered.containsKey("function"))
        assertTrue(!rendered.containsKey("parameters"))

        assertTrue(!anthropic.buildBody("claude", listOf(ChatTurn("user", "hi")), true).containsKey("tools"))
    }

    // --- OpenAI Responses --------------------------------------------------

    @Test
    fun `responses 助手工具调用是消息加顶层 function_call 项`() {
        val body = responses.buildBody(
            "gpt-5",
            listOf(
                ChatTurn("system", "规则"),
                ChatTurn("user", "写文件"),
                ChatTurn("assistant", "好的", toolCalls = listOf(call)),
            ),
            stream = true,
        )
        val input = body["input"]!!.jsonArray
        assertEquals(3, input.size, "user 消息 + assistant 消息 + 顶层 function_call")
        assertEquals("user", input[0].jsonObject["role"]!!.jsonPrimitive.content)

        val message = input[1].jsonObject
        assertEquals("assistant", message["role"]!!.jsonPrimitive.content)
        assertEquals("好的", message["content"]!!.jsonArray[0].jsonObject["text"]!!.jsonPrimitive.content)
        assertEquals(
            "output_text",
            message["content"]!!.jsonArray[0].jsonObject["type"]!!.jsonPrimitive.content,
        )

        val fnCall = input[2].jsonObject
        assertEquals("function_call", fnCall["type"]!!.jsonPrimitive.content)
        assertEquals("call_1", fnCall["call_id"]!!.jsonPrimitive.content)
        assertEquals("write_file", fnCall["name"]!!.jsonPrimitive.content)
        assertEquals("""{"path":"comfyui/a.txt"}""", fnCall["arguments"]!!.jsonPrimitive.content)
        assertEquals("false", body["store"]!!.jsonPrimitive.content, "store:false 必须保留")
    }

    @Test
    fun `responses 工具结果渲染成 function_call_output`() {
        val body = responses.buildBody(
            "gpt-5",
            listOf(ChatTurn("tool", "已写入", toolCallId = "call_1")),
            stream = true,
        )
        val item = body["input"]!!.jsonArray.single().jsonObject
        assertEquals("function_call_output", item["type"]!!.jsonPrimitive.content)
        assertEquals("call_1", item["call_id"]!!.jsonPrimitive.content)
        assertEquals("已写入", item["output"]!!.jsonPrimitive.content)
        assertEquals("false", body["store"]!!.jsonPrimitive.content)
    }

    @Test
    fun `responses tools 是扁平结构 空列表整体省略`() {
        val body = responses.buildBody("gpt-5", listOf(ChatTurn("user", "hi")), true, tools = listOf(spec))
        val rendered = body["tools"]!!.jsonArray.single().jsonObject
        assertEquals("function", rendered["type"]!!.jsonPrimitive.content)
        assertEquals("write_file", rendered["name"]!!.jsonPrimitive.content)
        assertEquals("写文件", rendered["description"]!!.jsonPrimitive.content)
        assertTrue(rendered["parameters"] is JsonObject)
        assertTrue(!rendered.containsKey("function"), "Responses 的工具定义是扁平的")

        assertTrue(!responses.buildBody("gpt-5", listOf(ChatTurn("user", "hi")), true).containsKey("tools"))
    }

    // --- interpret：三家流里的工具分片 --------------------------------------

    @Test
    fun `openai interpret 把 delta 里的 tool_calls 转成 ToolCallDelta`() {
        val events = listOf(
            SseAccumulator.Frame(
                null,
                """{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write_file","arguments":"{\"pa"}}]}}]}""",
            ),
            SseAccumulator.Frame(
                null,
                """{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]}}]}""",
            ),
        ).flatMap { openai.interpret(it) }

        val deltas = events.filterIsInstance<StreamEvent.ToolCallDelta>()
        assertEquals(2, deltas.size)
        assertEquals(0, deltas[0].index)
        assertEquals("call_1", deltas[0].key)
        assertEquals("call_1", deltas[0].callId)
        assertEquals("write_file", deltas[0].name)
        assertEquals("{\"pa", deltas[0].argumentsFragment)
        assertNull(deltas[1].key, "后续分片只有 index + arguments")
        assertNull(deltas[1].name)
        assertEquals("th\":\"a\"}", deltas[1].argumentsFragment)
    }

    @Test
    fun `anthropic interpret 处理 content_block_start 与 input_json_delta`() {
        val events = listOf(
            SseAccumulator.Frame(
                "content_block_start",
                """{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"write_file","input":{}}}""",
            ),
            SseAccumulator.Frame(
                "content_block_delta",
                """{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\""}}""",
            ),
            SseAccumulator.Frame(
                "content_block_delta",
                """{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":":\"a\"}"}}""",
            ),
        ).flatMap { anthropic.interpret(it) }

        val deltas = events.filterIsInstance<StreamEvent.ToolCallDelta>()
        assertEquals(3, deltas.size)
        assertEquals(1, deltas[0].index)
        assertEquals("toolu_1", deltas[0].key)
        assertEquals("toolu_1", deltas[0].callId)
        assertEquals("write_file", deltas[0].name)
        assertNull(deltas[0].argumentsFragment, "start 里的空 input 不当成参数片段")
        assertNull(deltas[1].key, "参数分片只有 index")
        assertEquals("{\"path\"", deltas[1].argumentsFragment)
        assertEquals(":\"a\"}", deltas[2].argumentsFragment)
    }

    @Test
    fun `anthropic start 块已经给全 input 时直接当参数`() {
        val deltas = anthropic.interpret(
            SseAccumulator.Frame(
                "content_block_start",
                """{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_2","name":"read_file","input":{"path":"comfyui/a.txt"}}}""",
            )
        ).filterIsInstance<StreamEvent.ToolCallDelta>()
        assertEquals(1, deltas.size)
        val fragment = deltas.single().argumentsFragment
        assertTrue(fragment != null && fragment.contains("\"path\""), "整块 input 要当参数片段传下去：$fragment")
    }

    @Test
    fun `responses interpret 处理 output_item-added 与 arguments-delta`() {
        val events = listOf(
            SseAccumulator.Frame(
                "response.output_item.added",
                """{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"write_file","arguments":""}}""",
            ),
            SseAccumulator.Frame(
                "response.function_call_arguments.delta",
                """{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":"}""",
            ),
            SseAccumulator.Frame(
                "response.function_call_arguments.delta",
                """{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"\"a\"}"}""",
            ),
        ).flatMap { responses.interpret(it) }

        val deltas = events.filterIsInstance<StreamEvent.ToolCallDelta>()
        assertEquals(3, deltas.size)
        assertEquals("fc_1", deltas[0].key, "流内键是 item.id")
        assertEquals("call_1", deltas[0].callId, "回传给上游的是 item.call_id")
        assertEquals("write_file", deltas[0].name)
        assertNull(deltas[0].argumentsFragment, "空 arguments 不算分片")
        assertEquals("fc_1", deltas[1].key)
        assertNull(deltas[1].callId, "分片不再重复 call_id")
        assertEquals("{\"path\":", deltas[1].argumentsFragment)
        assertEquals("\"a\"}", deltas[2].argumentsFragment)

        // 三家分片都能被同一个累加器拼回来
        val acc = ToolCallAccumulator()
        deltas.forEach { acc.apply(it) }
        val draft = acc.drafts().single()
        assertEquals("call_1", draft.callId)
        assertEquals("write_file", draft.name)
        assertEquals("""{"path":"a"}""", draft.arguments)
    }

    // --- ToolCallAccumulator -----------------------------------------------

    @Test
    fun `累加器：OpenAI 风格按 index 归并`() {
        val acc = ToolCallAccumulator()
        acc.apply(
            StreamEvent.ToolCallDelta(
                index = 0, key = "call_1", callId = "call_1",
                name = "write_file", argumentsFragment = "{\"path\"",
            )
        )
        acc.apply(StreamEvent.ToolCallDelta(index = 0, argumentsFragment = ":\"comfyui/a.txt\"}"))

        assertTrue(!acc.isEmpty())
        val draft = acc.drafts().single()
        assertEquals("call_1", draft.callId)
        assertEquals("write_file", draft.name)
        assertEquals("""{"path":"comfyui/a.txt"}""", draft.arguments)
    }

    @Test
    fun `累加器：Anthropic 风格只有 start 块带 id`() {
        val acc = ToolCallAccumulator()
        acc.apply(StreamEvent.ToolCallDelta(index = 1, key = "toolu_1", callId = "toolu_1", name = "read_file"))
        acc.apply(StreamEvent.ToolCallDelta(index = 1, argumentsFragment = "{\"pa"))
        acc.apply(StreamEvent.ToolCallDelta(index = 1, argumentsFragment = "th\":\"a.txt\"}"))

        val draft = acc.drafts().single()
        assertEquals("toolu_1", draft.callId)
        assertEquals("read_file", draft.name)
        assertEquals("""{"path":"a.txt"}""", draft.arguments)
    }

    @Test
    fun `累加器：Responses 风格以 item_id 为键 callId 只给一次`() {
        val acc = ToolCallAccumulator()
        acc.apply(
            StreamEvent.ToolCallDelta(
                index = 0, key = "fc_1", callId = "call_1", name = "write_file",
            )
        )
        acc.apply(StreamEvent.ToolCallDelta(index = 0, key = "fc_1", argumentsFragment = "{\"path\":\"a\"}"))

        val draft = acc.drafts().single()
        assertEquals("call_1", draft.callId)
        assertEquals("write_file", draft.name)
        assertEquals("""{"path":"a"}""", draft.arguments)
    }

    @Test
    fun `累加器：两个并行调用互不串味且保持出现顺序`() {
        val acc = ToolCallAccumulator()
        acc.apply(StreamEvent.ToolCallDelta(index = 0, key = "call_a", callId = "call_a", name = "list_dir", argumentsFragment = "{"))
        acc.apply(StreamEvent.ToolCallDelta(index = 1, key = "call_b", callId = "call_b", name = "read_file", argumentsFragment = "{"))
        acc.apply(StreamEvent.ToolCallDelta(index = 0, argumentsFragment = "}"))
        acc.apply(StreamEvent.ToolCallDelta(index = 1, argumentsFragment = "}"))

        val drafts = acc.drafts()
        assertEquals(2, drafts.size)
        assertEquals(listOf("call_a", "call_b"), drafts.map { it.callId })
        assertEquals(listOf("list_dir", "read_file"), drafts.map { it.name })
        assertEquals(listOf("{}", "{}"), drafts.map { it.arguments })
    }

    @Test
    fun `累加器：字符串里的花括号 换行 引号原样保留`() {
        val payload = "{\"content\":\"a } b\\nc \\\"d\\\" e\"}"
        val acc = ToolCallAccumulator()
        acc.apply(StreamEvent.ToolCallDelta(index = 0, key = "c", callId = "c", name = "write_file", argumentsFragment = payload.take(7)))
        acc.apply(StreamEvent.ToolCallDelta(index = 0, argumentsFragment = payload.substring(7, 20)))
        acc.apply(StreamEvent.ToolCallDelta(index = 0, argumentsFragment = payload.substring(20)))

        assertEquals(payload, acc.drafts().single().arguments)
    }

    @Test
    fun `累加器：callId 缺失时退回流内键 不丢这次调用`() {
        val acc = ToolCallAccumulator()
        acc.apply(StreamEvent.ToolCallDelta(index = 2, name = "list_dir", argumentsFragment = "{}"))
        val fallback = acc.drafts().single()
        assertEquals("idx:2", fallback.callId)
        assertEquals("list_dir", fallback.name)

        // 有流内键、但没有上游 callId（个别网关不回 id）→ 用流内键顶上
        val acc2 = ToolCallAccumulator()
        acc2.apply(StreamEvent.ToolCallDelta(index = 0, key = "item_9", name = "read_file"))
        assertEquals("item_9", acc2.drafts().single().callId)
    }

    // --- 版本号 -------------------------------------------------------------

    @Test
    fun `三种协议的适配器都升到 2 且都在 supported 名单里`() {
        assertEquals("openai-completions/2", openai.adapterVersion)
        assertEquals("anthropic-messages/2", anthropic.adapterVersion)
        assertEquals("openai-responses/2", responses.adapterVersion)
        assertEquals(
            setOf(AiApiRef.OPENAI_COMPLETIONS, AiApiRef.ANTHROPIC_MESSAGES, AiApiRef.OPENAI_RESPONSES),
            Adapters.supported().toSet(),
        )
    }
}
