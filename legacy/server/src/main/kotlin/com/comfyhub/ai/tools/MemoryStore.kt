package com.comfyhub.ai.tools

import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.time.Instant
import java.time.LocalDate

/**
 * 长期记忆（M6，用户建议"引入长期记忆"）。
 *
 * 真源是一个**人类可读、可手改**的 Markdown 文件：`<storage>\ai\memory.md`，
 * 一行一条（`- 内容`）。选文件而不是表，理由和 Skills 一样：
 * 用户能直接打开看、直接删，AI 写进去的东西不会藏进某张表里。
 *
 * 三条不能破的规矩：
 *  1. 注入系统提示时**截断**到 [MAX_CHARS]，并且和工具输出一样**当数据不当指令**
 *     （记忆里若被塞进"忽略之前的规则"，模型必须当作可疑内容报告，见系统提示第 4 条）；
 *  2. 写入有硬上限，超出直接报错而不是悄悄截断 —— 悄悄截断等于悄悄丢用户的话；
 *  3. 不做缓存：AI 刚 `remember` 完，下一次 Run 渲染提示词时就要读到它（与 Skills 一致）。
 *
 * ## 条数与写入节流（2026-09-18，用户要求）
 *
 * 用户的顾虑是**用起来**的顾虑，不是存储的顾虑："条数太多，到时候想查找、改动、删除会比较困难"。
 * 所以除了原有的字符上限，再压三道闸：
 *  - **总条数**上限 [MAX_ENTRIES]（100 条）：满了**报错**让用户去清理，不静默丢；
 *  - **一次 Run** 里 AI 最多新增 [MAX_ENTRIES_PER_RUN] 条（在 `remember` 工具里查 `ctx.memoryWrites`）——
 *    不限制的话，模型很容易把整段对话都"记下来"；
 *  - AI 写的每条自动带 `[yyyy-MM-dd]` 前缀（[append] 的 `dated` 参数；**用户手敲的不带**），
 *    这样界面上能按时间看、按关键词搜，过期的也好挑出来删。
 */
@Serializable
data class MemoryDto(
    val content: String = "",
    val path: String = "",
    /** 一行一条，这里给的是条数（空行不算） */
    val entryCount: Int = 0,
    val maxChars: Int = MemoryStore.MAX_CHARS,
    /** 条数上限（界面显示"N / 100 条"用） */
    val maxEntries: Int = MemoryStore.MAX_ENTRIES,
    val updatedAt: String? = null,
)

class MemoryStore(private val dir: Path) {
    private val log = LoggerFactory.getLogger(MemoryStore::class.java)

    companion object {
        /** 能注入系统提示的正文上限（超出部分不进提示词，但文件里仍然完整保留） */
        const val MAX_CHARS = 8000

        /** 单条记忆的长度上限 */
        const val MAX_ENTRY_CHARS = 500

        /** **总条数**硬上限：条数一多，查找 / 改动 / 删除都会变难（用户要求） */
        const val MAX_ENTRIES = 100

        /** 一次 Run 里 AI 最多新增几条（用户要求）；超了 `remember` 直接拒绝并说明 */
        const val MAX_ENTRIES_PER_RUN = 4

        private const val FILE_NAME = "memory.md"

        /** 系统提示里注入的记忆正文上限（模型上下文有限，记忆只是背景资料） */
        const val PROMPT_CHARS = 4000

        /** AI 写入时的日期前缀（形如 `[2026-09-18] `） */
        private val DATE_PREFIX = Regex("""^\[\d{4}-\d{2}-\d{2}]\s*""")

        /** 列表项前面的符号（`- 内容`） */
        private val BULLET = Regex("""^[-*]\s*""")

        fun countEntries(content: String): Int =
            content.lines().count { it.trim().isNotEmpty() }

        /**
         * 一行的**正文**：去掉 `- ` 与 `[日期] ` 前缀。
         *
         * 判重就靠它 —— 否则"同一件事今天记一次、明天又记一次"会堆成两行
         * （日期不同，按字面比就不相等了）。
         */
        fun bodyOf(line: String): String =
            line.trim().replaceFirst(BULLET, "").replaceFirst(DATE_PREFIX, "").trim()
    }
    val file: Path get() = dir.resolve(FILE_NAME)

    /** 一行一条的正文（去掉空行）。界面按**这个顺序**编号做单条 / 批量删除。 */
    fun entries(): List<String> = read()
        .content
        .lines()
        .map { it.trim() }
        .filter { it.isNotEmpty() }

    /**
     * 记忆里是不是已经有这条（忽略 `- ` / `[日期] ` 前缀与大小写）。
     *
     * 给 `remember` 用：判重命中的"又说了一遍"不算新增 ——
     * 既不占每轮额度，也不该被节流挡住。
     */
    fun hasEntry(entry: String): Boolean {
        val body = bodyOf(entry)
        if (body.isEmpty()) return false
        return read().content.lines().any { bodyOf(it).equals(body, ignoreCase = true) }
    }

    fun read(): MemoryDto {
        val text = runCatching {
            if (Files.isRegularFile(file)) Files.readString(file, StandardCharsets.UTF_8) else ""
        }.getOrElse {
            log.warn("读取长期记忆失败: {}", it.message)
            ""
        }
        // 落盘时统一以换行结尾（人看起来才像正常文件），读回来时去掉 —— 免得界面里
        // 每次保存都多攒一个空行，也让"读 → 改 → 写"是一次幂等的往返。
        return dto(text.trim())
    }

    /** 给系统提示用的正文（截断到 [PROMPT_CHARS]，省略说明也算在里面）。 */
    fun promptText(): String {
        val text = read().content
        if (text.length <= PROMPT_CHARS) return text
        val note = "\n…（长期记忆过长已截断，完整内容见右侧栏「长期记忆」）"
        return text.take(PROMPT_CHARS - note.length) + note
    }

    /** 整篇替换（界面上的「保存」走这里）。 */
    fun write(content: String): MemoryDto {
        val text = content.replace("\r\n", "\n").trim()
        if (text.length > MAX_CHARS) {
            throw ToolFailure("MEMORY_TOO_LARGE", "记忆太长（${text.length} > $MAX_CHARS 字符），请删掉一些再保存")
        }
        persist(text)
        return dto(text)
    }

    /**
     * 追加一条。
     *
     * [dated] 为 true 时给正文加上 `[yyyy-MM-dd]` 前缀 —— **只有 AI 的 `remember` 传 true**，
     * 用户在手敲「+ 添加一条」时传 false（用户自己写的东西不该被我们改写）。
     *
     * 已存在同样的**正文**（忽略 `- ` / `[日期] ` 前缀、忽略大小写与首尾空白）时
     * **不重复添加**，直接返回现状 —— 模型有时会把同一件事说两遍，不该在记忆里堆成两行。
     */
    fun append(entry: String, dated: Boolean = false): MemoryDto {
        val line = entry.replace("\r\n", "\n").replace('\n', ' ').trim()
        val body = line.replaceFirst(BULLET, "").trim()
        if (body.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "记的内容不能为空")
        val stamped = if (dated) "[${LocalDate.now()}] $body" else body
        if (stamped.length > MAX_ENTRY_CHARS) {
            throw ToolFailure(
                "INVALID_ARGUMENT",
                "一条记忆太长（${stamped.length} > $MAX_ENTRY_CHARS 字符），拆成几条",
            )
        }
        val current = read().content.trim()
        if (hasEntry(body)) return dto(current)
        // 条数上限：满了**报错**，让用户去界面里清理；不许静默丢、也不许悄悄挤掉最旧的一条
        val count = countEntries(current)
        if (count >= MAX_ENTRIES) {
            throw ToolFailure(
                "MEMORY_FULL",
                "长期记忆已经有 $count 条（上限 $MAX_ENTRIES 条）。请用户到 AI 工作台右侧栏的" +
                    "「长期记忆」里删掉一些再记 —— 不要丢内容，也不要自己改写已有的条目。",
            )
        }
        val bullet = "- $stamped"
        val next = if (current.isEmpty()) bullet else "$current\n$bullet"
        if (next.length > MAX_CHARS) {
            throw ToolFailure(
                "MEMORY_TOO_LARGE",
                "长期记忆已满（$MAX_CHARS 字符上限）；告诉用户去右侧栏的「长期记忆」里清理，不要丢内容",
            )
        }
        persist(next)
        log.info("长期记忆 +1 条（共 {} 条）", countEntries(next))
        return dto(next)
    }

    /**
     * 按下标删除若干条（界面的单条 / 批量删除）。
     *
     * 下标是 [entries] 的顺序（0 起）。越界的**忽略掉**：界面手里的列表与磁盘可能只差
     * 一瞬间（AI 刚好在另一头 remember 了一条），不该因为一条对不上就整批失败。
     */
    fun deleteEntries(indices: Collection<Int>): MemoryDto {
        val want = indices.toSet()
        if (want.isEmpty()) return read()
        val kept = mutableListOf<String>()
        entries().forEachIndexed { i, line -> if (i !in want) kept += line }
        val next = kept.joinToString("\n")
        persist(next)
        log.info("长期记忆删除 {} 条（剩 {} 条）", want.size, countEntries(next))
        return dto(next)
    }

    fun clear(): MemoryDto {
        persist("")
        return dto("")
    }

    private fun persist(text: String) {
        Files.createDirectories(dir)
        Files.writeString(file, if (text.isEmpty()) "" else "$text\n", StandardCharsets.UTF_8)
    }

    private fun dto(text: String): MemoryDto = MemoryDto(
        content = text,
        path = file.toString(),
        entryCount = countEntries(text),
        maxChars = MAX_CHARS,
        maxEntries = MAX_ENTRIES,
        updatedAt = runCatching {
            if (Files.isRegularFile(file)) Files.getLastModifiedTime(file).toInstant().toString()
            else Instant.now().toString()
        }.getOrNull(),
    )
}
