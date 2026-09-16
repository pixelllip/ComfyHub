package com.comfyhub.ai.tools

import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.time.Instant

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
 */
@Serializable
data class MemoryDto(
    val content: String = "",
    val path: String = "",
    /** 一行一条，这里给的是条数（空行不算） */
    val entryCount: Int = 0,
    val maxChars: Int = MemoryStore.MAX_CHARS,
    val updatedAt: String? = null,
)

class MemoryStore(private val dir: Path) {
    private val log = LoggerFactory.getLogger(MemoryStore::class.java)

    companion object {
        /** 能注入系统提示的正文上限（超出部分不进提示词，但文件里仍然完整保留） */
        const val MAX_CHARS = 8000

        /** 单条记忆的长度上限 */
        const val MAX_ENTRY_CHARS = 500

        private const val FILE_NAME = "memory.md"

        /** 系统提示里注入的记忆正文上限（模型上下文有限，记忆只是背景资料） */
        const val PROMPT_CHARS = 4000

        fun countEntries(content: String): Int =
            content.lines().count { it.trim().isNotEmpty() }
    }

    val file: Path get() = dir.resolve(FILE_NAME)

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
     * 追加一条（AI 的 `remember` 工具走这里）。
     *
     * 已存在同样的内容（忽略大小写与首尾空白）时**不重复添加**，直接返回现状 ——
     * 模型有时会把同一件事说两遍，不该在记忆里堆成两行。
     */
    fun append(entry: String): MemoryDto {
        val line = entry.replace("\r\n", "\n").replace('\n', ' ').trim()
        if (line.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "记的内容不能为空")
        if (line.length > MAX_ENTRY_CHARS) {
            throw ToolFailure("INVALID_ARGUMENT", "一条记忆太长（${line.length} > $MAX_ENTRY_CHARS 字符），拆成几条")
        }
        val current = read().content.trim()
        val bullet = "- $line"
        val exists = current.lines().any { it.trim().equals(bullet, ignoreCase = true) }
        if (exists) return dto(current)
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
        updatedAt = runCatching {
            if (Files.isRegularFile(file)) Files.getLastModifiedTime(file).toInstant().toString()
            else Instant.now().toString()
        }.getOrNull(),
    )
}
