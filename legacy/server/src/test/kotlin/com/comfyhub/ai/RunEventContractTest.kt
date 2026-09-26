package com.comfyhub.ai

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 🔒 RunEvent 契约（`docs/run-event-contract.md`）与 `RunEventType` 的**清单一致**用例（M0 交付物）。
 *
 * 为什么值得存在：这份契约是"UI 代码两端共享、零分叉"的全部依据（总纲 §3.2）——
 * 桌面走远端 SSE、手机走本地 platform channel，**事件类型逐字一致**才谈得上共用一套渲染。
 * 而漂移是**静默**的：改了一个常量名、或者文档里漏写一个类型，两端各自编译通过、测试全绿，
 * 直到手机上某个事件永远不被渲染才发现。
 *
 * 所以这里**从文档里读**（不是把清单再抄一遍 —— 抄一遍就等于没有防线）：
 * 契约 §2 表格里反引号包起来的第一列就是权威清单。
 */
class RunEventContractTest {

    /** 仓库根：测试的工作目录就是 `server/`。 */
    private val contract: Path = Path.of("..", "docs", "run-event-contract.md").normalize()

    /** 契约 §2 表格里的类型清单（按出现顺序）。 */
    private fun typesInContract(): List<String> {
        assertTrue(Files.isRegularFile(contract), "契约文件不在：${contract.toAbsolutePath()}（M0 的冻结物之一）")
        val text = Files.readString(contract)
        // 只取 §2 那一节（到 §3 为止），避免把别处反引号里的东西也算进来。
        val section = text.substringAfter("## 2.", "").substringBefore("\n## 3.")
        assertTrue(section.isNotBlank(), "契约里找不到 §2 事件类型清单（章节标题改了吗？）")
        return Regex("""^\|\s*`([a-z][a-z._]*)`\s*\|""", RegexOption.MULTILINE)
            .findAll(section)
            .map { it.groupValues[1] }
            // 表头那一行是 `` `type` | 常量 | … ``，也长得像数据行 —— 按名字排掉它。
            // ⚠ 别改成"过滤掉不含点号的"：`heartbeat` 就不含点号，那样会静默少一个
            //（2026-09-23 写这条用例时真的踩到了，表现是"个数 16≠17"）。
            .filter { it != "type" }
            .toList()
    }

    /** `RunEventType` 里所有事件类型常量（反射读，免得再加一处手工清单）。 */
    private fun typesInCode(): Set<String> =
        RunEventType::class.java.declaredFields
            .filter { java.lang.reflect.Modifier.isStatic(it.modifiers) && it.type == String::class.java }
            .mapNotNull { f ->
                runCatching { f.isAccessible = true; f.get(null) as? String }.getOrNull()
            }
            .toSet()

    @Test
    fun `契约 §2 的类型清单与 RunEventType 逐项相等`() {
        val documented = typesInContract()
        val coded = typesInCode()

        assertEquals(
            documented.size, documented.toSet().size,
            "契约 §2 里有重复的类型行：$documented",
        )
        assertEquals(
            (documented.toSet() - coded).sorted(), emptyList<String>(),
            "契约里写了、代码里没有（文档先行没关系，但要一起改）",
        )
        assertEquals(
            (coded - documented.toSet()).sorted(), emptyList<String>(),
            "代码里加了新事件类型，契约 §2 没登记 —— 手机的常量清单会漏掉它",
        )
        assertEquals(17, documented.size, "契约的事件类型个数变了（现在是 ${documented.size} 个）")
    }

    @Test
    fun `SSE 帧格式是 id-event-data-空行`() {
        val sse = RunEvent(7, RunEventType.TEXT_DELTA, kotlinx.serialization.json.buildJsonObject { }).toSse()
        assertEquals("id: 7\nevent: text.delta\ndata: {}\n\n", sse, "契约 §1 的帧格式被改了")
    }
}

