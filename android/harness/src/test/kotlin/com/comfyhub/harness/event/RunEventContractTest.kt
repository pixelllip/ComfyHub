package com.comfyhub.harness.event

import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.buildJsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * 🔒 RunEvent 契约（`docs/run-event-contract.md`）与手机侧 `RunEventType` 的**清单一致**用例。
 *
 * 这是 M0 契约在**手机侧**的防线（契约 §2 的"两端各有一条用例"）：
 * 桌面那份 `RunEventContractTest` 只保证"桌面 = 契约"，保证不了"手机 = 桌面"。
 * 漂移是**静默**的 —— 手机漏一个类型，编译通过、测试全绿，
 * 直到手机上某个事件永远不被渲染才发现（而这个文件就是为那件事存在的）。
 *
 * 判据与桌面完全一致：**从文档里读**（不是把清单再抄一遍，抄一遍就等于没有防线），
 * 契约 §2 表格里反引号包起来的第一列就是权威清单。
 */
class RunEventContractTest {

    /** 仓库根下的契约文件：从测试的工作目录往上找 `docs/run-event-contract.md`。 */
    private fun contractFile(): Path {
        var dir: Path? = Path.of("").toAbsolutePath()
        while (dir != null) {
            val candidate = dir.resolve("docs").resolve("run-event-contract.md")
            if (Files.isRegularFile(candidate)) return candidate
            dir = dir.parent
        }
        error("找不到 docs/run-event-contract.md（测试工作目录=${Path.of("").toAbsolutePath()}）")
    }

    /** 契约 §2 表格里的类型清单（按出现顺序）。 */
    private fun typesInContract(): List<String> {
        // 用 readAllBytes 而不是 Files.readString：单元测试编译在 android.jar 上，
        // 那里没有 Java 11 的便捷方法。
        val text = String(Files.readAllBytes(contractFile()), Charsets.UTF_8)
        // 只取 §2 那一节（到 §3 为止），避免把别处反引号里的东西也算进来。
        val section = text.substringAfter("## 2.", "").substringBefore("\n## 3.")
        assertTrue("契约里找不到 §2 事件类型清单（章节标题改了吗？）", section.isNotBlank())
        return Regex("""^\|\s*`([a-z][a-z._]*)`\s*\|""", RegexOption.MULTILINE)
            .findAll(section)
            .map { it.groupValues[1] }
            // 表头那一行是 `` `type` | 常量 | … ``，也长得像数据行 —— 按名字排掉它。
            // ⚠ 别改成"过滤掉不含点号的"：`heartbeat` 就不含点号，那样会静默少一个。
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
    fun `契约 §2 的类型清单与手机侧 RunEventType 逐项相等`() {
        val documented = typesInContract()
        val coded = typesInCode()

        assertEquals("契约 §2 里有重复的类型行：$documented", documented.size, documented.toSet().size)
        assertEquals(
            "契约里写了、手机常量里没有（手机的常量清单会漏掉它）",
            emptyList<String>(),
            (documented.toSet() - coded).sorted(),
        )
        assertEquals(
            "手机加了新事件类型，契约 §2 没登记",
            emptyList<String>(),
            (coded - documented.toSet()).sorted(),
        )
        // 2026-11-05 桌面加了 `step.started`（环节标记），契约 §2 从 17 变 18。
        // 手机侧原先漏了这个常量 —— 这条计数断言就是那次漂移的断路器。
        assertEquals("契约的事件类型个数变了（现在是 ${documented.size} 个）", 18, documented.size)
    }

    @Test
    fun `ALL 的顺序与契约 §2 的表一致（同序才好逐行对照）`() {
        assertEquals(typesInContract(), RunEventType.ALL)
    }

    @Test
    fun `SSE 帧格式是 id-event-data-空行`() {
        val sse = RunEvent(7, RunEventType.TEXT_DELTA, buildJsonObject { }).toSse()
        assertEquals("契约 §1 的帧格式被改了", "id: 7\nevent: text.delta\ndata: {}\n\n", sse)
    }
}

/**
 * `RunEventBus` 的环形缓冲与断线重连续传（红线 **R5**，移植自桌面 `RunEventBusTest`）。
 *
 * 手机比桌面更需要这几条：**后台被杀是常态**，重连（切回前台）时拿到的事件是否完整、
 * `seq` 会不会错位，光看代码看不出来。
 */
class RunEventBusTest {

    private fun drain(ch: ReceiveChannel<RunEvent>): List<RunEvent> = runBlocking {
        val out = mutableListOf<RunEvent>()
        for (e in ch) out += e
        out
    }

    @Test
    fun `超过内存上限只丢最老的、seq 绝不回退`() {
        val bus = RunEventBus()
        bus.open("r1")
        val total = RunEventBus.MEMORY_KEEP + 50
        repeat(total) { bus.emit("r1", RunEventType.TEXT_DELTA, buildJsonObject { }) }

        val history = bus.history("r1")
        assertEquals("内存里必须只剩上限那么多条", RunEventBus.MEMORY_KEEP, history.size)
        assertEquals("应该丢最老的", total - RunEventBus.MEMORY_KEEP + 1, history.first().seq)
        assertEquals("最新的必须还在", total, history.last().seq)
        val next = bus.emit("r1", RunEventType.HEARTBEAT, buildJsonObject { })
        assertEquals("seq 回退/复用会让客户端游标永久错位", total + 1, next?.seq)
    }

    @Test
    fun `R5：切后台再回来能拿全 —— 订阅者拿到的是实时那份，不受环形上限影响`() {
        val bus = RunEventBus()
        bus.open("r2")
        repeat(RunEventBus.MEMORY_KEEP + 10) { bus.emit("r2", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        val ch = bus.subscribe("r2", after = RunEventBus.MEMORY_KEEP + 10)
        assertNotNull("Run 在内存里就必须能订阅", ch)
        repeat(5) { bus.emit("r2", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        bus.close("r2")
        val got = drain(ch!!)
        assertEquals("订阅之后发的每一条都要收到", 5, got.size)
        assertEquals(
            "序号要连续，不能因为缓冲截断而跳",
            (RunEventBus.MEMORY_KEEP + 11..RunEventBus.MEMORY_KEEP + 15).toList(),
            got.map { it.seq },
        )
    }

    @Test
    fun `重连时 after 早于窗口起点，拿到的是窗口内的（不假装全都有）`() {
        val bus = RunEventBus()
        bus.open("r3")
        repeat(RunEventBus.MEMORY_KEEP + 10) { bus.emit("r3", RunEventType.TEXT_DELTA, buildJsonObject { }) }
        val ch = bus.subscribe("r3", after = 1)
        assertNotNull("Run 在内存里就必须能订阅", ch)
        bus.close("r3")
        val got = drain(ch!!)
        assertEquals("内存窗口有多大就只给多少", RunEventBus.MEMORY_KEEP, got.size)
        assertEquals("最新的那条必须在", RunEventBus.MEMORY_KEEP + 10, got.last().seq)
        assertTrue("老于窗口的补不回来 —— 这就是本地库保留窗口存在的理由", got.first().seq > 1)
    }

    @Test
    fun `Run 不在内存里时订阅返回 null，由调用方改从本地库回放`() {
        val bus = RunEventBus()
        assertNull("进程被杀过的 Run 必须回落到本地库回放", bus.subscribe("nope", after = 0))
    }

    @Test
    fun `Run 收尾后再发事件不发出去（闭流后不许再进订阅队列）`() {
        val bus = RunEventBus()
        bus.open("r4")
        bus.emit("r4", RunEventType.RUN_STARTED, buildJsonObject { })
        bus.close("r4")
        assertNull(bus.emit("r4", RunEventType.TEXT_DELTA, buildJsonObject { }))
        assertTrue(bus.isClosed("r4"))
    }
}
