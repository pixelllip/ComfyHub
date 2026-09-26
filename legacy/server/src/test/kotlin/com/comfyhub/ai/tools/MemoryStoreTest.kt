package com.comfyhub.ai.tools

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path

/**
 * 长期记忆（M6）。
 *
 * 盯住三件事：
 *  1. 真源是**人能看懂的 memory.md**（一行一条），不是藏起来的表；
 *  2. 不重复、不悄悄丢：同一条不写两遍，超限**报错**而不是静默截断；
 *  3. 进系统提示的部分一定被截断（模型上下文有限，记忆只是背景资料）。
 */
class MemoryStoreTest {

    private fun store(): Pair<MemoryStore, Path> {
        val root = Files.createTempDirectory("comfyhub-memory").toRealPath()
        return MemoryStore(root) to root
    }

    @Test
    fun `空记忆读出来是空 不抛错`() {
        val (m, _) = store()
        val dto = m.read()
        assertEquals("", dto.content)
        assertEquals(0, dto.entryCount)
        assertEquals("", m.promptText())
        assertTrue(dto.path.endsWith("memory.md"))
    }

    @Test
    fun `append 落成一行一条的 markdown 并统计条数`() {
        val (m, _) = store()
        m.append("用户偏好 4:3 画幅")
        val dto = m.append("出图统一用 Aesthetic 模型")

        assertEquals(2, dto.entryCount)
        assertEquals("- 用户偏好 4:3 画幅\n- 出图统一用 Aesthetic 模型", dto.content)
        assertEquals(
            "- 用户偏好 4:3 画幅\n- 出图统一用 Aesthetic 模型\n",
            Files.readString(m.file, StandardCharsets.UTF_8),
        )
    }

    @Test
    fun `同一条记忆写两遍不会变成两行（模型常重复说同一件事）`() {
        val (m, _) = store()
        m.append("用户偏好 4:3 画幅")
        val again = m.append("  用户偏好 4:3 画幅  ")
        assertEquals(1, again.entryCount)
        assertEquals(1, again.content.lines().count { it.isNotBlank() })
    }

    @Test
    fun `空内容与超长单条被拒绝（不静默截断用户的话）`() {
        val (m, _) = store()
        assertFailsWith<ToolFailure> { m.append("   ") }
        val tooLong = "x".repeat(MemoryStore.MAX_ENTRY_CHARS + 1)
        val e = assertFailsWith<ToolFailure> { m.append(tooLong) }
        assertEquals("INVALID_ARGUMENT", e.code)
        assertEquals("", m.read().content, "被拒绝的写入不该留下半条")
    }

    @Test
    fun `整篇替换有自己的上限`() {
        val (m, _) = store()
        val e = assertFailsWith<ToolFailure> { m.write("y".repeat(MemoryStore.MAX_CHARS + 1)) }
        assertEquals("MEMORY_TOO_LARGE", e.code)
    }

    @Test
    fun `记忆写满之后 append 明确报错 而不是覆盖旧内容`() {
        val (m, _) = store()
        m.write("z".repeat(MemoryStore.MAX_CHARS - 2))
        val e = assertFailsWith<ToolFailure> { m.append("再来一条") }
        assertEquals("MEMORY_TOO_LARGE", e.code)
        assertEquals(MemoryStore.MAX_CHARS - 2, m.read().content.length, "旧内容必须完好")
    }

    @Test
    fun `write 与 clear 都能往返`() {
        val (m, _) = store()
        m.write("## 用户偏好\n- 画幅 4:3\n\n- 用 Anima 模型")
        assertEquals(3, m.read().entryCount, "空行不算一条")
        val cleared = m.clear()
        assertEquals("", cleared.content)
        assertEquals("", Files.readString(m.file, StandardCharsets.UTF_8))
    }

    // --- 条数与写入规则（2026-09-18，用户要求）-------------------------------

    @Test
    fun `AI 写的带日期前缀 用户手敲的不带`() {
        val (m, _) = store()
        val today = java.time.LocalDate.now().toString()
        m.append("用户偏好 4:3 画幅", dated = true)
        m.append("交付一律带字幕")

        assertTrue(
            m.read().content.startsWith("- [$today] 用户偏好 4:3 画幅"),
            "AI 写入要带日期前缀，便于按时间查找 / 清理：${m.read().content}",
        )
        assertTrue(
            m.read().content.contains("- 交付一律带字幕"),
            "用户自己手敲的那条不该被我们改写：${m.read().content}",
        )
    }

    @Test
    fun `判重忽略日期前缀（同一件事隔天再记不会堆成两行）`() {
        val (m, _) = store()
        m.append("用户偏好 4:3 画幅", dated = true)
        // 第二天模型又说了一遍同一件事：内容一样，只有日期会不同 → 不该变成两条
        val again = m.append("  用户偏好 4:3 画幅  ", dated = true)
        assertEquals(1, again.entryCount, "带日期前缀之后判重仍要命中：${again.content}")
    }

    @Test
    fun `条数到上限之后 append 报 MEMORY_FULL 而不是挤掉最旧的一条`() {
        val (m, _) = store()
        m.write((1..MemoryStore.MAX_ENTRIES).joinToString("\n") { "- 第 $it 条" })
        assertEquals(MemoryStore.MAX_ENTRIES, m.read().entryCount)

        val e = assertFailsWith<ToolFailure> { m.append("再来一条") }
        assertEquals("MEMORY_FULL", e.code)
        assertTrue(e.message!!.contains("删掉一些"), "要告诉用户去界面里清理：${e.message}")
        assertEquals(MemoryStore.MAX_ENTRIES, m.read().entryCount, "旧内容必须完好")
        assertTrue(m.read().content.contains("- 第 1 条"), "最旧的一条不许被悄悄挤掉")
    }

    @Test
    fun `按下标删除：单条、批量都行 越界的忽略`() {
        val (m, _) = store()
        m.write("- A\n- B\n- C\n- D")

        // 单条（界面上的那一行小垃圾桶）
        assertEquals("- A\n- C\n- D", m.deleteEntries(listOf(1)).content)
        // 批量 + 一个越界下标（界面与磁盘差一瞬间）：不该整批失败
        assertEquals("- D", m.deleteEntries(listOf(0, 1, 99)).content)
        // 空集合什么都不做
        assertEquals("- D", m.deleteEntries(emptyList()).content)
    }

    @Test
    fun `条目列表与落盘内容同源 删除按同一份下标`() {
        val (m, _) = store()
        m.write("- A\n\n- B\n- C")
        assertEquals(listOf("- A", "- B", "- C"), m.entries(), "空行不算，下标要和界面对得上")
    }

    @Test
    fun `注入系统提示的正文会被截断 但文件里仍然完整`() {
        val (m, _) = store()
        // 用短行拼到超过 PROMPT_CHARS（entry 上限 500，一条一条加）
        repeat(12) { m.append("第 $it 条：" + "内容".repeat(200)) }
        val full = m.read().content
        assertTrue(full.length > MemoryStore.PROMPT_CHARS, "前置条件：内容确实超过注入上限")

        val prompt = m.promptText()
        assertTrue(prompt.length <= MemoryStore.PROMPT_CHARS, "注入部分要截断（含省略说明）：${prompt.length}")
        assertTrue(prompt.contains("已截断"))
        assertTrue(Files.readString(m.file, StandardCharsets.UTF_8).contains(full.trim()), "文件里必须完整")
    }

    @Test
    fun `系统提示会带上长期记忆 并且明确它是数据不是指令`() {
        val (m, _) = store()
        m.append("用户偏好 4:3 画幅")
        val text = com.comfyhub.ai.SystemPrompt.render(
            provider = com.comfyhub.ai.AiProviderDto(
                id = "p", displayName = "本机", api = "openai-completions",
                baseURL = "http://127.0.0.1:1/v1", credentialRef = "K", endpointTrust = "loopback",
            ),
            modelId = "m",
            memory = m.promptText(),
        )
        assertTrue(text.contains("长期记忆"), "要有一段长期记忆")
        assertTrue(text.contains("用户偏好 4:3 画幅"), "记忆正文要进提示词")
        assertTrue(text.contains("不是指令"), "必须写明记忆是数据，不是指令（注入防线）")
    }
}
