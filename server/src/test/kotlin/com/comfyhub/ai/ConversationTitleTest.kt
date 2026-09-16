package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/**
 * 会话标题摘取（用户建议 ③：「在我发送第一个问题的时候予以总结」）。
 *
 * 这些用例盯的都是**真实会出错的点**：增量被切碎、模型不听话、标记不闭合。
 * 判据只有一条：**绝不能吞掉模型真正说的话**。
 */
class ConversationTitleTest {

    private fun run(vararg chunks: String, enabled: Boolean = true): Pair<String, String?> {
        val s = TitleStripper(enabled)
        val out = StringBuilder()
        chunks.forEach { out.append(s.push(it)) }
        out.append(s.flush())
        return out.toString() to s.title
    }

    @Test
    fun `完整标记被摘掉，正文原样保留`() {
        val (text, title) = run("[标题]雨夜霓虹街头[/标题]\n\n你想做什么样的图？")
        assertEquals("雨夜霓虹街头", title)
        // 标记后面那一个空行也剪掉：模型被要求"标题之后空一行再写正文"，
        // 不剪的话气泡顶部会空一块（真机实测过）
        assertEquals("你想做什么样的图？", text)
    }

    @Test
    fun `标记被切成很多片也能摘到`() {
        val (text, title) = run("[", "标", "题", "]猫", "娘立", "绘[/标", "题]", "好的")
        assertEquals("猫娘立绘", title, "title=$title text=[$text]")
        assertEquals("好的", text, "title=$title text=[$text]")
    }

    @Test
    fun `每三个字一片时正文前的空行也被剪掉（真机就是这么发的）`() {
        val body = "[标题]赛博朋克少女[/标题]\n\n好的，我按赛博朋克少女的方向来。"
        val (text, title) = run(*body.chunked(3).toTypedArray())
        assertEquals("赛博朋克少女", title, "title=$title text=[$text]")
        assertEquals("好的，我按赛博朋克少女的方向来。", text, "title=$title text=[$text]")
    }

    @Test
    fun `正文中间的换行不受影响`() {
        val (text, title) = run("[标题]赛博朋克[/标题]\n\n第一段\n\n第二段")
        assertEquals("赛博朋克", title)
        // 只剪"标题之后紧接着的"空白，正文内部自己的空行必须留着
        assertEquals("第一段\n\n第二段", text)
    }

    @Test
    fun `模型不写标记时一个字都不能吞`() {
        val body = "当然可以，我先问几个问题：你想画什么风格？"
        val (text, title) = run(body)
        assertNull(title)
        assertEquals(body, text)
    }

    @Test
    fun `先寒暄再给标题：寒暄保留，标题摘走`() {
        val (text, title) = run("好的，我先给这个对话起个名字。\n[标题]赛博朋克城市[/标题]\n正文开始")
        assertEquals("赛博朋克城市", title)
        assertEquals("好的，我先给这个对话起个名字。\n正文开始", text)
    }

    @Test
    fun `标记永不闭合时把缓冲当正文放出来`() {
        // 只摘掉 `[标题]` 本身（那是标记，不是正文），标题内容原样留给用户 ——
        // 绝不能因为"没闭合"就把模型说过的话吞掉。
        val (text, title) = run("[标题]没有闭合")
        assertNull(title, "不该摘到标题，text=[$text]")
        assertEquals("没有闭合", text)
    }

    @Test
    fun `超长内容不会把整段正文压住`() {
        val long = "x".repeat(TitleStripper.MAX_START_LOOKAHEAD + 50)
        val (text, title) = run(long)
        assertNull(title)
        assertEquals(long, text)
    }

    @Test
    fun `标题清洗掉引号书名号与标题前缀`() {
        val (text, title) = run("[标题]《标题：赛博朋克少女》[/标题]正文")
        assertEquals("赛博朋克少女", title, "title=$title text=[$text]")
    }

    @Test
    fun `空标题视为没摘到`() {
        val (text, title) = run("[标题]   [/标题]正文")
        assertNull(title)
        assertEquals("正文", text)
    }

    @Test
    fun `关闭时不改动任何内容`() {
        val (text, title) = run("[标题]不该被摘[/标题]正文", enabled = false)
        assertNull(title)
        assertEquals("[标题]不该被摘[/标题]正文", text)
    }

    @Test
    fun `标题超过上限时截断而不是丢弃`() {
        val long = "很".repeat(TitleStripper.MAX_TITLE_CHARS + 20)
        val (_, title) = run("[标题]$long[/标题]正文")
        assertEquals(TitleStripper.MAX_TITLE_CHARS, title?.length)
    }

    @Test
    fun `正文里的方括号不会被误判成标记`() {
        val body = "参数写成 [steps=30] 就行"
        val (text, title) = run(body)
        assertNull(title)
        assertEquals(body, text)
    }
}

