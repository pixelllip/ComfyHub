package com.comfyhub.ai

/**
 * 会话标题的**自动总结**（用户建议 ③：「对话记录的标题，在我发送第一个问题的时候予以总结」）。
 *
 * 为什么是"让模型在正文最前面带一行标题"，而不是**再发一次上游请求**去总结：
 *  - 用户的第一条提问本来就要走一次上游，多问一次就是多花一次钱、多等一次网络；
 *  - 有些网关对"只要标题"的短请求会走另一套限流/计费，行为不可预期。
 * 所以第一轮请求里附一句很短的要求，让模型先输出 `[标题]…[/标题]`，
 * 后端在流式过程中把它摘掉、写进会话，**用户看不到这行标记**。
 *
 * 这个类就是"摘标记"的状态机，刻意做成**纯逻辑、可单测**。两条铁律：
 *  1. **绝不吞内容** —— 模型不按格式来时，说过的话一个字都不能少；
 *  2. **绝不重复内容** —— 寒暄可能分好几片到达，已放行的部分不能再放一遍。
 *
 * 三个状态：
 * ```
 * START    只可能有 `[` / `[标` 这种没收全的标记 → 压着（最多压 MAX_START_LOOKAHEAD 个字符）
 * STREAM   已经在放行寒暄了；尾部最多留 OPEN/CLOSE 那么长的尾巴，防止标记被切开时漏出去
 * IN_TITLE 见到 `[标题]`，正在等 `[/标题]`（标题本身不放进正文）
 * ```
 * 摘到标题、或确认没有标题之后进入 `PASSTHROUGH`，后续增量原样透传。
 */
class TitleStripper(private val enabled: Boolean) {

    companion object {
        /** 标题标记：模型被告知只输出这一个格式。 */
        const val OPEN = "[标题]"
        const val CLOSE = "[/标题]"

        /** 标题长度上限。 */
        const val MAX_TITLE_CHARS = 60

        /**
         * START 状态最多压多少字符。
         *
         * 选 200：足够覆盖"模型先啰嗦两句再给标题"的常见偏差，
         * 又不会让第一句话迟迟不出现在界面上（流式体验）。
         */
        const val MAX_START_LOOKAHEAD = 200

        private const val START = 0
        private const val STREAM = 1
        private const val IN_TITLE = 2
        private const val PASSTHROUGH = 3
    }

    private val buffer = StringBuilder()
    private var state = START

    /** 摘出来的标题（还没摘到 / 不需要摘时为 null）。 */
    var title: String? = null
        private set

    /** 标记已经处理完（成功摘到或确认没有），后续增量直接透传。 */
    val finished: Boolean get() = !enabled || state == PASSTHROUGH

    /**
     * 喂一片增量，返回**应当进入正文**的文本（可能为空串）。
     *
     * 调用方只需把返回值原样发给界面 / 追加到落库文本，不要另外拼接缓冲。
     */
    fun push(chunk: String): String {
        if (!enabled || state == PASSTHROUGH || chunk.isEmpty()) return chunk
        buffer.append(chunk)

        if (state == START) {
            val openAt = buffer.indexOf(OPEN)
            if (openAt >= 0) return takeTitle(openAt)

            // 还没到上限：押着，等下一片把标记补全（`[` + `标题]`）
            if (buffer.length <= MAX_START_LOOKAHEAD) return ""

            // 已经压得太久，模型显然没按格式来：先把确定不是标记的部分放行。
            // 只保留"可能是标记开头"的最后几个字符，避免把切开的 `[标` 漏给用户看。
            state = STREAM
            return dropBefore(lastPartialMarkerStart())
        }

        if (state == STREAM) {
            val openAt = buffer.indexOf(OPEN)
            if (openAt >= 0) return takeTitle(openAt)
            // 尾部可能藏着切了一半的标记，留到最后再说
            return dropBefore(lastPartialMarkerStart())
        }

        // IN_TITLE：已经在标记里了，只等闭合
        return drainTitle()
    }

    /** 流结束时收尾：没闭合的缓冲一律当正文放出去（宁可多显示，也不吞内容）。 */
    fun flush(): String {
        if (buffer.isEmpty()) return ""
        val out = buffer.toString()
        buffer.clear()
        state = PASSTHROUGH
        return out
    }

    // -----------------------------------------------------------------------

    /** 确认 `[标题]` 出现在标记 [openAt]，把标记之前的正文放出去，然后开始等闭合。 */
    private fun takeTitle(openAt: Int): String {
        val preamble = buffer.substring(0, openAt)
        buffer.delete(0, openAt + OPEN.length)
        state = IN_TITLE
        // 标记之前的寒暄是模型真的说了的话，原样放行
        return preamble + drainTitle()
    }

    /** 处理 `[标题]` 之后的内容：闭合了就摘标题，否则继续等。 */
    private fun drainTitle(): String {
        val closeAt = buffer.indexOf(CLOSE)
        if (closeAt >= 0) {
            title = clean(buffer.substring(0, closeAt))
            val rest = buffer.substring(closeAt + CLOSE.length)
            buffer.clear()
            state = PASSTHROUGH
            return rest
        }
        // 标题本身长得离谱 → 认输，把整段当正文放行（模型没按格式来）
        if (buffer.length > MAX_TITLE_CHARS + 40) {
            state = PASSTHROUGH
            return flush()
        }
        return "" // 还在等 `[/标题]`
    }

    /** 放行 `[0, end)`，把它们从缓冲里去掉（[end] 之前的都确定不是标记的一部分）。 */
    private fun dropBefore(end: Int): String {
        if (end <= 0) return ""
        val out = buffer.substring(0, end)
        buffer.delete(0, end)
        return out
    }

    /**
     * 缓冲末尾那段"可能还没收全的标记"从哪个下标开始。
     *
     * 只有两种情况要留：`[标`（OPEN 的前缀）、`[/标`（CLOSE 的前缀）。
     * 其余一律可以立刻放行 —— 否则模型一开口就被按住，流式看起来像卡住。
     */
    private fun lastPartialMarkerStart(): Int {
        val text = buffer.toString()
        val bracket = text.lastIndexOf(OPEN[0])
        if (bracket < 0) return text.length
        val tail = text.substring(bracket)
        val isPartial = (tail.length < OPEN.length && OPEN.startsWith(tail)) ||
            (tail.length < CLOSE.length && CLOSE.startsWith(tail))
        return if (isPartial) bracket else text.length
    }

    private fun clean(raw: String): String? {
        var t = raw.replace(Regex("\\s+"), " ").trim()
        // 去掉模型爱加的前缀与包裹：`标题：xxx`、`《xxx》`、`"xxx"`…
        t = t.trim('：', ':', '"', '\'', '“', '”', '‘', '’', '《', '》', '【', '】', '「', '」', '[', ']', '#', '*')
        if (t.startsWith("标题")) {
            t = t.removePrefix("标题").trimStart('：', ':', ' ', '\t')
        }
        t = t.trim('：', ':', '"', '\'', '“', '”', '‘', '’', '《', '》', '【', '】', '「', '」', '[', ']', '#', '*')
        if (t.length > MAX_TITLE_CHARS) t = t.take(MAX_TITLE_CHARS)
        return t.takeIf { it.isNotBlank() }
    }
}
