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
 * 四个状态（[scanAndStrip] 是"扫描 + 摘标记"的循环，START / STREAM / PASSTHROUGH 都走它）：
 * ```
 * START       还没看见 `[标题]` → 先压着（最多压 MAX_START_LOOKAHEAD 个字符），
 *             免得把切成两片的 `[标` + `题]` 漏半个给用户看
 * STREAM      已经在放行寒暄了；尾部最多留 OPEN/CLOSE 那么长的尾巴，防止标记被切开时漏出去
 * IN_TITLE    见到 `[标题]`，正在等 `[/标题]`（标题本身不放进正文）
 * PASSTHROUGH 已经摘到标题：正文正常放行，但**继续盯着**完整标记对 ——
 *             模型在后续轮次里可能把 `[标题]…[/标题]` 再吐一遍（回声），
 *             那同样是标记、不是正文。回声只摘标记，不改已定的会话名（见 [scanAndStrip]）。
 * ```
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

    /**
     * 刚摘掉标题标记：接下来放出去的那一小段要先剪掉前导空白。
     *
     * 为什么需要：模型被要求"标题之后空一行再写正文"，于是标记后面紧跟的是 `\n\n`，
     * 摘掉标记之后正文就会以两个空行开头（真机实测过一次，气泡顶部空一块）。
     */
    private var trimLeadingBlank = false

    /** 摘出来的标题（还没摘到 / 不需要摘时为 null）。 */
    var title: String? = null
        private set

    /**
     * 标记已经处理完（标题摘到了 / 确认没有）。
     *
     * ⚠️ **不等于"后面不会再有标记"**：回声照样会被摘掉（见 [scanAndStrip]）。
     */
    val finished: Boolean get() = !enabled || state == PASSTHROUGH

    /**
     * 喂一片增量，返回**应当进入正文**的文本（可能为空串）。
     *
     * 调用方只需把返回值原样发给界面 / 追加到落库文本，不要另外拼接缓冲。
     */
    fun push(chunk: String): String {
        if (!enabled || chunk.isEmpty()) return chunk
        buffer.append(chunk)

        // START：还没看见 `[标题]`，先按住（最多 MAX_START_LOOKAHEAD），
        // 免得把切成两片的 `[标` + `题]` 漏半个给用户看。
        if (state == START) {
            if (buffer.indexOf(OPEN) < 0 && buffer.length <= MAX_START_LOOKAHEAD) return ""
            // 压得太久说明模型根本没打算给标题：放行（只留可能是标记开头的那几个字符）
            state = STREAM
        }
        return scanAndStrip()
    }

    /**
     * 扫描缓冲：见到完整标记就摘掉，其余原样放行。
     *
     * **一片里可能藏着好几个标记**，所以是一个循环而不是"摘一个就返回"：
     * 摘到标题之后模型可能又把 `[标题]…[/标题]` 吐一遍（回声，真机实测过），
     * 那一次也得摘掉 —— 否则它就以正文的样子出现在回答里
     * （用户 bug⑤："AI 回答首尾疑似又被截断"，实据是回复里那行
     * `[标题] Krea2 初音画风迁移出图[/标题]`）。
     *
     * 铁律不变 —— **绝不吞内容**：标题一直不闭合 / 长得离谱时，剩下的内容照样放行
     * （缓冲区里那些还会由 [flush] 在流结束时兜底放出去）。
     */
    private fun scanAndStrip(): String {
        val out = StringBuilder()
        while (true) {
            // 1) 正在等 `[/标题]`
            if (state == IN_TITLE) {
                val closeAt = buffer.indexOf(CLOSE)
                if (closeAt < 0) {
                    // 标题本身长得离谱 → 认输：`[标题]` 之后那一段当正文放行（模型没按格式来）
                    if (buffer.length > MAX_TITLE_CHARS + 40) {
                        state = PASSTHROUGH
                        out.append(cleanLeading(takeBuffer()))
                    }
                    return out.toString() // 还在等 `[/标题]`
                }
                val found = clean(buffer.substring(0, closeAt))
                // **第一次**摘到的那个才算数：后面那些是回声，只摘标记，不改已定的会话名
                if (title == null) {
                    title = found
                    // 标题之后紧跟的那一个空行剪掉（"标题之后空一行再写正文"是格式要求）。
                    // 只对第一次这样做：回声前后一概不碰，一个字都不多吞。
                    trimLeadingBlank = true
                }
                buffer.delete(0, closeAt + CLOSE.length)
                state = PASSTHROUGH
                continue
            }

            // 2) 找下一个 `[标题]`
            val openAt = buffer.indexOf(OPEN)
            if (openAt < 0) {
                // 尾部可能藏着切了一半的标记（`[` / `[标` / `[/标`），留到下一片再说
                out.append(cleanLeading(dropBefore(lastPartialMarkerStart())))
                return out.toString()
            }
            // 标记之前的正文（寒暄）是模型真的说了的话，原样放行
            out.append(cleanLeading(dropBefore(openAt)))
            buffer.delete(0, OPEN.length)
            state = IN_TITLE
        }
    }

    /** 流结束时收尾：没闭合的缓冲一律当正文放出去（宁可多显示，也不吞内容）。 */
    fun flush(): String {
        if (buffer.isEmpty()) return ""
        state = PASSTHROUGH
        return takeBuffer()
    }

    // -----------------------------------------------------------------------

    /** 取走缓冲里的全部内容（并清空缓冲）。 */
    private fun takeBuffer(): String {
        val out = buffer.toString()
        buffer.clear()
        return out
    }

    /** 放行 `[0, end)`，把它们从缓冲里去掉（[end] 之前的都确定不是标记的一部分）。 */
    private fun dropBefore(end: Int): String {
        if (end <= 0) return ""
        val out = buffer.substring(0, end)
        buffer.delete(0, end)
        return out
    }

    /** 标题标记之后的第一段内容：剪掉前导空白（见 [trimLeadingBlank]）。 */
    private fun cleanLeading(text: String): String {
        if (!trimLeadingBlank || text.isEmpty()) return text
        val trimmed = text.trimStart()
        if (trimmed.isNotEmpty()) trimLeadingBlank = false
        return trimmed
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
