package com.comfyhub.ai.protocol

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * 附件内联的协议契约（M3 / AIH-028 / AIH-030）。
 *
 * 三家协议**真的把图片发出去**的写法完全不同，而且都是"发错了就 400"的类型：
 *
 *  | 协议 | 图片写法 |
 *  | --- | --- |
 *  | `openai-completions` | `content: [{type:"text"},{type:"image_url",image_url:{url:"data:…"}}]` |
 *  | `anthropic-messages` | `content: [{type:"image",source:{type:"base64",media_type,data}}]` |
 *  | `openai-responses` | `input[].content[] = {type:"input_image",image_url:"data:…"}` |
 *
 * 另外两条不变式也在这一层钉住：**文本轮仍然是字符串**（最广兼容），
 * **没实现的模态抛 [UnsupportedContentFailure]**（绝不静默丢弃后假装发成功）。
 */
class AttachmentProtocolTest {

    private val pngBase64 = "iVBORw0KGgoAAAANSUhEUg=="

    private fun image(name: String = "a.png"): ChatAttachment =
        ChatAttachment(AttachmentKindRef.IMAGE, "image/png", pngBase64, name)

    private val openai = OpenAiCompletionsAdapter()
    private val anthropic = AnthropicMessagesAdapter()
    private val responses = OpenAiResponsesAdapter()

    // --- data URL ---------------------------------------------------------

    @Test
    fun `data url 的形状：data 前缀加 mime 加 base64`() {
        assertEquals("data:image/png;base64,$pngBase64", image().dataUrl)
    }

    // --- openai-completions ------------------------------------------------

    @Test
    fun `openai：图片走 image_url 的 data url，文本在前`() {
        val body = openai.buildBody(
            model = "m",
            messages = listOf(ChatTurn("user", "看看这张图", attachments = listOf(image()))),
            stream = true,
        )
        val content = body["messages"]!!.jsonArray[0].jsonObject["content"]!!.jsonArray
        assertEquals(2, content.size)
        assertEquals("text", content[0].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals("看看这张图", content[0].jsonObject["text"]!!.jsonPrimitive.content)
        assertEquals("image_url", content[1].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals(
            "data:image/png;base64,$pngBase64",
            content[1].jsonObject["image_url"]!!.jsonObject["url"]!!.jsonPrimitive.content,
        )
    }

    @Test
    fun `openai：只发图不打字也成立；纯文本轮仍然是字符串`() {
        val body = openai.buildBody(
            model = "m",
            messages = listOf(
                ChatTurn("user", "上一轮"),
                ChatTurn("assistant", "好的"),
                ChatTurn("user", "", attachments = listOf(image())),
            ),
            stream = true,
        )
        val messages = body["messages"]!!.jsonArray
        // 前三轮（没有附件）保持最朴素的字符串 content —— 兼容性最好
        assertTrue(messages[0].jsonObject["content"] is JsonPrimitive)
        assertTrue(messages[1].jsonObject["content"] is JsonPrimitive)
        val withImage = messages[2].jsonObject["content"]!!.jsonArray
        assertEquals(1, withImage.size, "没有文字时就只有图片块")
        assertEquals("image_url", withImage[0].jsonObject["type"]!!.jsonPrimitive.content)
    }

    // --- anthropic ---------------------------------------------------------

    @Test
    fun `anthropic：图片在前文本在后，source 用裸 base64`() {
        val body = anthropic.buildBody(
            model = "m",
            messages = listOf(ChatTurn("user", "看看这张图", attachments = listOf(image()))),
            stream = true,
        )
        val content = body["messages"]!!.jsonArray[0].jsonObject["content"]!!.jsonArray
        assertEquals(2, content.size)
        val block = content[0].jsonObject
        assertEquals("image", block["type"]!!.jsonPrimitive.content)
        val source = block["source"]!!.jsonObject
        assertEquals("base64", source["type"]!!.jsonPrimitive.content)
        assertEquals("image/png", source["media_type"]!!.jsonPrimitive.content)
        assertEquals(pngBase64, source["data"]!!.jsonPrimitive.content)
        assertEquals("text", content[1].jsonObject["type"]!!.jsonPrimitive.content)
        assertEquals("看看这张图", content[1].jsonObject["text"]!!.jsonPrimitive.content)
    }

    // --- openai-responses --------------------------------------------------

    @Test
    fun `responses：input_image 用 data url，和 input_text 并列`() {
        val body = responses.buildBody(
            model = "m",
            messages = listOf(ChatTurn("user", "看看这张图", attachments = listOf(image()))),
            stream = true,
        )
        val content = body["input"]!!.jsonArray[0].jsonObject["content"]!!.jsonArray
        assertEquals(2, content.size)
        val img = content[0].jsonObject
        assertEquals("input_image", img["type"]!!.jsonPrimitive.content)
        assertEquals("data:image/png;base64,$pngBase64", img["image_url"]!!.jsonPrimitive.content)
        assertEquals("input_text", content[1].jsonObject["type"]!!.jsonPrimitive.content)
    }

    // --- 未实现的模态 -------------------------------------------------------

    @Test
    fun `视频音频文档：三种协议都抛 UnsupportedContentFailure 而不是静默丢掉`() {
        val video = ChatAttachment(AttachmentKindRef.VIDEO, "video/mp4", "AAAA", "a.mp4")
        val turns = listOf(ChatTurn("user", "看视频", attachments = listOf(video)))
        listOf(openai, anthropic, responses).forEach { adapter ->
            assertFailsWith<UnsupportedContentFailure>("${adapter.api} 应当明确失败") {
                adapter.buildBody("m", turns, stream = true)
            }
        }
    }

    /** 顺带钉住 JSON 访问辅助函数的行为（上面几个断言全靠它）。 */
    @Test
    fun `工具轮与附件轮互不干扰`() {
        val body = openai.buildBody(
            model = "m",
            messages = listOf(
                ChatTurn(
                    "assistant",
                    "我来查一下",
                    toolCalls = listOf(ToolCallRef("call_1", "comfy_get_status", "{}")),
                ),
                ChatTurn("tool", "{\"ok\":true}", toolCallId = "call_1"),
                ChatTurn("user", "谢谢", attachments = listOf(image())),
            ),
            stream = true,
        )
        val messages = body["messages"]!!.jsonArray
        assertEquals(3, messages.size)
        assertEquals("call_1", messages[0].jsonObject["tool_calls"]!!.jsonArray[0].jsonObject["id"]!!.jsonPrimitive.content)
        assertEquals("call_1", messages[1].jsonObject["tool_call_id"]!!.jsonPrimitive.content)
        assertEquals(2, (messages[2].jsonObject["content"] as JsonArray).size)
    }
}
