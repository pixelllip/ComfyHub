package com.comfyhub.harness.text

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 会话标题摘取 —— 移植自桌面 `ConversationTitleTest`（同一组用例）。
 *
 * 这些用例盯的都是**真实会出错的点**：增量被切碎、模型不听话、标记不闭合。
 * 判据只有一条：**绝不能吞掉模型真正说的话**。
 *
 * 手机侧照搬的理由：这是**同一份算法、两份实现**，而"已说的话一个字都不能丢"
 * 正是红线 R6 的第一条纪律 —— 桌面测过的边界，手机必须也过。
 */
class TitleStripperTest {

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
        assertEquals("title=$title text=[$text]", "猫娘立绘", title)
        assertEquals("title=$title text=[$text]", "好的", text)
    }

    @Test
    fun `每三个字一片时正文前的空行也被剪掉（真机就是这么发的）`() {
        val body = "[标题]赛博朋克少女[/标题]\n\n好的，我按赛博朋克少女的方向来。"
        val (text, title) = run(*body.chunked(3).toTypedArray())
        assertEquals("title=$title text=[$text]", "赛博朋克少女", title)
        assertEquals("title=$title text=[$text]", "好的，我按赛博朋克少女的方向来。", text)
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
        assertNull("不该摘到标题，text=[$text]", title)
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
        assertEquals("title=$title text=[$text]", "赛博朋克少女", title)
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

    // 用户 bug⑤："AI 回答首尾疑似又被截断"。第一层根因是"起标题指令被常驻在每轮上下文里"，
    // 第二层就是这里钉的：摘到标题之后模型又吐了一遍标记，回声同样必须摘掉。
    @Test
    fun `摘到标题之后模型再吐一遍标记，回声也要摘掉且不改标题`() {
        // 真实流是切成小片的（真机实测：`[标` / `题]另一个说法` / `[/标` / `题]`）
        val (text, title) = run(
            "[标题]初见标题[/标题]\n\n正文第一段。",
            "[标", "题]另一个说法", "[/标", "题]", "\n\n正文第二段。",
        )
        assertEquals("第一次摘到的才算数，text=[$text]", "初见标题", title)
        assertEquals("回声标记不能漏进正文，text=[$text]", "正文第一段。\n\n正文第二段。", text)
    }

    @Test
    fun `回声标记一直不闭合时一个字都不能吞`() {
        val (text, title) = run("[标题]真的标题[/标题]正文[标题]没有闭合")
        assertEquals("真的标题", title)
        // 只摘掉 `[标题]` 这个标记本身，标题内容照样是模型说过的话
        assertEquals("text=[$text]", "正文没有闭合", text)
    }

    @Test
    fun `回声标记之后正文照旧流式放行，不被压住`() {
        val body = "[标题]先定的名字[/标题]\n\n第一段。[标题]回声[/标题]\n\n第二段。"
        val (text, title) = run(*body.chunked(2).toTypedArray())
        assertEquals("title=$title text=[$text]", "先定的名字", title)
        // 回声只摘标记本身：它后面那个空行是正文的段落分隔，照旧留着（绝不吞内容）
        assertEquals("title=$title text=[$text]", "第一段。\n\n第二段。", text)
    }

    // --- 失败 / 取消时"已经说出去的话一个字都不能丢"（红线 R6 第 2 条） ----------

    @Test
    fun `R6：流断在半路时 flush 会把压住的整段放出来，一个字都不许丢`() {
        val stripper = TitleStripper(enabled = true)
        // 第一轮：模型刚开口就被掐断 —— START 状态把这一小段压在缓冲里
        assertEquals("", stripper.push("好的，我"))
        assertNull(stripper.title)
        // 失败分支必须 flush：否则用户看到的回复开头空一块
        assertEquals("好的，我", stripper.flush())
    }

    @Test
    fun `R6：标题已摘到、回声压住一半时 flush 不吞回声内容`() {
        val stripper = TitleStripper(enabled = true)
        val visible = StringBuilder()
        visible.append(stripper.push("[标题]名字[/标题]\n\n正文"))
        // 回声的第二片只到了一半（`[标`），状态机把它压在尾部等着
        visible.append(stripper.push("尾巴[标"))
        visible.append(stripper.flush())
        assertEquals("名字", stripper.title)
        assertEquals("放出来的是模型真说过的话（含没闭合的半个标记）", "正文尾巴[标", visible.toString())
    }
}
