package com.comfyhub.ai.tools

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 🔒 工具分端契约（总纲 §3.5，红线 R11/R12）。
 *
 * 钉两件事：
 *
 * 1. **手机可达的工具就是那 7 个**（`docs/tool-scope-contract.md` §2）。
 *    漂移在这里是静默的：往手机清单里多塞一个 `comfy_submit`，
 *    模型就会一本正经地汇报"我提交了"，而桌面根本不在线（这正是 R11 要防的）。
 *    反过来少一个，表现为"手机上功能莫名缺失"，也查不出原因。
 *
 * 2. **桌面默认路径（`ToolScope.ALL`）一个工具都没少** —— 加 `ToolScope` 这一步
 *    必须是纯加字段、零行为变化；这条断言就是那个"零变化"的证据。
 */
class ToolScopeTest {

    private fun env() = ToolRegistryTest.Env(Files.createTempDirectory("comfyhub-scope").toRealPath())

    /** `docs/tool-scope-contract.md` §2 的"MOBILE 可执行"表（7 个）。 */
    private val mobileTools = setOf(
        "web_search", "web_fetch", "civitai_search",
        "list_skills", "load_skill", "remember", "ask_user_question",
    )

    @Test
    fun `手机可执行的工具正好是契约里那 7 个`() {
        val mobile = env().registry.toolsFor(ToolScope.MOBILE).map { it.name }.toSet()
        assertEquals(mobileTools.sorted(), mobile.sorted(), "手机侧工具清单与契约不一致")
    }

    @Test
    fun `桌面专属的工具一个都不许出现在手机清单里`() {
        val registry = env().registry
        // DESKTOP 与 MOBILE 两个过滤结果必须**互补且不重叠**：并集 = 全集，交集 = 空。
        // （`BOTH` 那 7 个会同时出现在两边，这正是"两端都能做"的意思。）
        val desktopSide = registry.toolsFor(ToolScope.DESKTOP).map { it.name }.toSet()
        val mobileSide = registry.toolsFor(ToolScope.MOBILE).map { it.name }.toSet()
        assertEquals(
            registry.tools.map { it.name }.toSet(), desktopSide + mobileSide,
            "有工具既不在手机清单也不在桌面清单",
        )
        assertTrue(
            registry.tools.filter { it.scope == ToolScope.DESKTOP }.map { it.name }.none { it in mobileSide },
            "桌面专属工具漏进了手机清单：${registry.tools.filter { it.scope == ToolScope.DESKTOP }.map { it.name }.filter { it in mobileSide }}",
        )
    }

    @Test
    fun `R11：手机侧不存在叫 comfy_submit 的工具`() {
        val mobile = env().registry.toolsFor(ToolScope.MOBILE).map { it.name }
        assertTrue(
            "comfy_submit" !in mobile,
            "手机清单里出现了 comfy_submit —— 桌面离线时它无法履行'提交→等生成→返回 mediaIds'的契约，模型会误报成功（R11）",
        )
    }

    @Test
    fun `桌面默认路径零变化：ALL 就是全量工具`() {
        val registry = env().registry
        assertEquals(
            registry.tools.map { it.name }.sorted(),
            registry.toolsFor(ToolScope.ALL).map { it.name }.sorted(),
            "ToolScope.ALL 必须等于全量 —— 桌面自己的 Run 行为不能因此改变",
        )
    }

    @Test
    fun `新增工具时忘了写 scope 会落在桌面（宁可手机上少一个，也不能多一个调不到的）`() {
        assertEquals(
            ToolScope.DESKTOP,
            AgentTool(
                name = "brand_new_tool",
                description = "d",
                parameters = kotlinx.serialization.json.buildJsonObject { },
                category = ToolCategory.FILES,
                mutating = false,
                defaultAccess = ToolAccess.ALLOW,
            ) { _, _ -> ToolOutput("") }.scope,
        )
    }

    @Test
    fun `契约文件在、且 §2 的 MOBILE 表与这里的断言是同一份清单`() {
        val doc: Path = Path.of("..", "docs", "tool-scope-contract.md").normalize()
        assertTrue(Files.isRegularFile(doc), "契约文件不在：${doc.toAbsolutePath()}")
        val text = Files.readString(doc)
        val section = text.substringAfter("## 2.", "").substringBefore("\n## 3.")
        val documented = Regex("""`([a-z_]+)`""").findAll(section).map { it.groupValues[1] }.toSet()
        assertEquals(mobileTools.sorted(), documented.sorted(), "契约文档的 MOBILE 表与本用例的清单不一致")
    }
}
