package com.comfyhub.ai

import java.time.LocalDate
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/**
 * run-kit 的**版本号与签名**用例（M1 / 🔒 docs/run-kit-contract.md §3）。
 *
 * `kitVersion` 是手机端判断"我手里的规则旧了没有"的唯一依据，它的两条纪律：
 *
 *  1. **没变就绝不跳号** —— 手机每次重连都取一次 kit，版本号必须稳定，
 *     否则"命中吊销"的判断就成了摆设；
 *  2. **四类触发器任一变化就 bump**（契约 §3：提示词正文 / 工具清单结构 /
 *     横幅文案 / 预算数值），同日累加序号、跨日从 .1 重新起。
 *
 * 这里测的是纯函数（[RunKit.nextVersion] / [RunKit.hashOf]）；
 * 组装层的端点行为归 e2e（`scripts\e2e-ai-tools-test.ps1` 后续补幕）。
 */
class RunKitVersionTest {

    private fun signature(
        promptVersion: String = "v28",
        nsfw: Boolean = false,
        toolsJson: String = """[{"name":"web_search"}]""",
        banner: String = "横幅",
        budget: String = """{"perImageBytes":8388608,"totalBytes":20971520}""",
        titleInstruction: String = "起标题指令",
        continueInstruction: String = "接着写指令",
    ) = RunKit.Signature(
        promptVersion = promptVersion,
        nsfwEnabled = nsfw,
        nsfwPromptVersion = "nsfw-v2",
        toolsJson = toolsJson,
        untrustedBanner = banner,
        inlineBudgetJson = budget,
        titleInstruction = titleInstruction,
        continueInstruction = continueInstruction,
    )

    @Test
    fun `内容没变时版本号稳定`() {
        val v = RunKit.nextVersion("2026-09-23.4", changed = false, today = LocalDate.of(2026, 9, 24))
        assertEquals("2026-09-23.4", v, "没变就沿用：跨日也不动")
    }

    @Test
    fun `同日变化序号加一`() {
        assertEquals(
            "2026-09-23.2",
            RunKit.nextVersion("2026-09-23.1", changed = true, today = LocalDate.of(2026, 9, 23)),
        )
    }

    @Test
    fun `跨日变化从 1 重新起号`() {
        assertEquals(
            "2026-09-24.1",
            RunKit.nextVersion("2026-09-23.7", changed = true, today = LocalDate.of(2026, 9, 24)),
        )
    }

    @Test
    fun `第一次发 kit 从 1 起号`() {
        assertEquals(
            "2026-09-23.1",
            RunKit.nextVersion(null, changed = false, today = LocalDate.of(2026, 9, 23)),
            "previous 为空（还没发过）也要有版本号",
        )
    }

    @Test
    fun `契约的四类触发器任一变化都要换签名`() {
        val base = RunKit.hashOf(signature())
        // ① 提示词版本
        assertNotEquals(base, RunKit.hashOf(signature(promptVersion = "v29")), "SystemPrompt.VERSION 变了必须 bump")
        // ② 工具清单结构（增删工具 / 改描述 / 改 schema —— description 变也是结构变）
        assertNotEquals(
            base,
            RunKit.hashOf(signature(toolsJson = """[{"name":"web_search"},{"name":"web_fetch"}]""")),
            "新增工具必须 bump",
        )
        assertNotEquals(
            base,
            RunKit.hashOf(signature(toolsJson = """[{"name":"web_search"}]""".replace("web_search", "webSearch"))),
            "工具名变了必须 bump（名字都变了个数没变也算）",
        )
        // ③ 横幅文案
        assertNotEquals(base, RunKit.hashOf(signature(banner = "横幅改了一个字")), "R7 文案变了必须 bump")
        // ④ 预算数值
        assertNotEquals(
            base,
            RunKit.hashOf(signature(budget = """{"perImageBytes":16777216,"totalBytes":20971520}""")),
            "inlineBudget 变了必须 bump",
        )
        // NSFW 注入状态随开关变化，也算规则变了
        assertNotEquals(base, RunKit.hashOf(signature(nsfw = true)), "NSFW 开关注入状态变了必须 bump")
        // 起标题指令（M3）：手机侧要照它摘标题，改措辞也必须 bump
        assertNotEquals(
            base,
            RunKit.hashOf(signature(titleInstruction = "换个说法起标题")),
            "titleInstruction 变了必须 bump（不然手机一直用旧措辞）",
        )
        assertNotEquals(
            base,
            RunKit.hashOf(signature(continueInstruction = "换个说法接着写")),
            "continueInstruction 变了必须 bump",
        )
    }

    @Test
    fun `同一签名哈希稳定且 canonical 覆盖全部触发器`() {
        val a = RunKit.hashOf(signature())
        val b = RunKit.hashOf(signature())
        assertEquals(a, b, "同一份规则两次算出的哈希必须一致（手机端比对就靠它）")
        assertTrue(a.length == 64, "SHA-256 十六进制应是 64 位")

        val canonical = signature().canonical()
        listOf(
            "promptVersion=", "nsfw=", "nsfwPromptVersion=", "tools=", "banner=", "budget=", "titleInstruction=", "continueInstruction=",
        ).forEach { key ->
            assertTrue(canonical.contains(key), "canonical() 缺了触发器 $key —— 新增签名字段时要同步进来")
        }
    }
}
