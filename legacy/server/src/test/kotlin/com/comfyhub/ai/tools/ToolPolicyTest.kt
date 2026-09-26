package com.comfyhub.ai.tools

import com.comfyhub.ai.protocol.ToolSpec
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlinx.serialization.json.buildJsonObject
import org.junit.jupiter.api.Assumptions.assumeTrue
import java.nio.file.Files
import java.nio.file.Path

/**
 * 工具权限策略（AGENTS.md §10：默认只能写 `<项目根>\comfyui`）。
 *
 * 三条硬规矩一条都不能松：
 *  1. 写只允许落在 writeRoots（出厂 = `<根>\comfyui`）；
 *  2. 读只允许落在 readRoots（出厂 = `comfyui` + `storage`，storage 只读）；
 *  3. `.git` / `.mysql` / `.run` / `node_modules` **永远不可写**，即使用户把白名单放宽到项目根。
 *
 * 路径判定要给 `..`、绝对路径、符号链接都判死刑。纯文件系统，不碰数据库也不联网。
 */
class ToolPolicyTest {

    /** 一个"像项目根"的临时目录：comfyui / storage / other / 运行期数据目录都建好。 */
    private fun newRoot(): Path {
        val root = Files.createTempDirectory("comfyhub-policy").toRealPath()
        listOf("comfyui", "storage", "other", ".git", ".mysql", ".run", "node_modules").forEach {
            Files.createDirectories(root.resolve(it))
        }
        return root
    }

    private fun fakeTool(name: String, access: ToolAccess) = AgentTool(
        name = name,
        description = "测试工具",
        parameters = ToolSpec.EMPTY_PARAMS,
        category = ToolCategory.FILES,
        mutating = false,
        defaultAccess = access,
    ) { _, _ -> ToolOutput("ok") }

    private fun registryOf(root: Path) = ToolRegistry(
        projectRoot = root,
        skills = SkillStore(root.resolve("builtin"), root.resolve("user")),
        approvals = ToolApprovalGate(),
        comfyStatus = { buildJsonObject { } },
        comfyFindRun = { _: String -> null },
        comfySync = { buildJsonObject { } },
    )

    // --- 写根 ---------------------------------------------------------------

    @Test
    fun `默认写根就是 comfyui 内部放行 兄弟目录与别处的绝对路径都拒绝`() {
        val root = newRoot()
        val policy = ToolPolicy(root, ToolPolicyConfig())

        assertEquals(listOf(root.resolve("comfyui")), policy.writeRoots)
        assertEquals(root.resolve("comfyui").resolve("x.txt"), policy.resolveWrite("comfyui/x.txt"))
        assertEquals(
            root.resolve("comfyui").resolve("nested/deep/y.txt"),
            policy.resolveWrite("comfyui/nested/deep/y.txt"),
            "还不存在的子目录要沿存在的祖先解析出来",
        )

        val sibling = assertFailsWith<ToolFailure> { policy.resolveWrite("other/x.txt") }
        assertEquals("PATH_DENIED", sibling.code)

        val absolute = assertFailsWith<ToolFailure> {
            policy.resolveWrite(root.resolve("other/abs.txt").toString())
        }
        assertEquals("PATH_DENIED", absolute.code)

        val parent = assertFailsWith<ToolFailure> { policy.resolveWrite("../outside.txt") }
        assertEquals("PATH_DENIED", parent.code)
    }

    @Test
    fun `穿越路径 comfyui 上跳 other 会被拒`() {
        val root = newRoot()
        val policy = ToolPolicy(root, ToolPolicyConfig())
        val denied = assertFailsWith<ToolFailure> { policy.resolveWrite("comfyui/../other/x.txt") }
        assertEquals("PATH_DENIED", denied.code)
        val deniedRead = assertFailsWith<ToolFailure> { policy.resolveRead("comfyui/../other/x.txt") }
        assertEquals("PATH_DENIED", deniedRead.code)
    }

    @Test
    fun `comfyui 里指向外面的符号链接不能当成写通道`() {
        val root = newRoot()
        val policy = ToolPolicy(root, ToolPolicyConfig())
        val link = root.resolve("comfyui/link-out")
        val created = runCatching { Files.createSymbolicLink(link, root.resolve("other")) }.isSuccess
        // Windows 上创建符号链接需要开发者模式 / 管理员：不支持时如实标成 skipped，不静默变绿
        assumeTrue(created, "本机无法创建符号链接，跳过链接逃逸用例")

        val denied = assertFailsWith<ToolFailure> { policy.resolveWrite("comfyui/link-out/x.txt") }
        assertEquals("PATH_DENIED", denied.code, "链接必须解成真实路径再判白名单")
        val readDenied = assertFailsWith<ToolFailure> { policy.resolveRead("comfyui/link-out/x.txt") }
        assertEquals("PATH_DENIED", readDenied.code)
    }

    @Test
    fun `运行期数据目录即使白名单放宽到项目根也不能写`() {
        val root = newRoot()
        val wide = ToolPolicy(root, ToolPolicyConfig(writeRoots = listOf(root.toString())))

        // 放宽之后普通目录确实能写了……
        assertEquals(root.resolve("other/x.txt"), wide.resolveWrite("other/x.txt"))

        // ……但这四个是"内脏"，任何工具都不许碰
        listOf(
            ".git/config",
            ".mysql/my.ini",
            ".run/app.log",
            "node_modules/pkg/index.js",
        ).forEach { rel ->
            val denied = assertFailsWith<ToolFailure>("$rel 必须被拒绝") { wide.resolveWrite(rel) }
            assertEquals("PATH_DENIED", denied.code, rel)
        }
    }

    // --- 读根 ---------------------------------------------------------------

    @Test
    fun `读根默认是 comfyui 加 storage 且 storage 只读`() {
        val root = newRoot()
        Files.writeString(root.resolve("storage/a.txt"), "产物")
        Files.writeString(root.resolve("other/a.txt"), "别的")
        val policy = ToolPolicy(root, ToolPolicyConfig())

        assertEquals(listOf(root.resolve("comfyui"), root.resolve("storage")), policy.readRoots)
        assertEquals(root.resolve("storage/a.txt"), policy.resolveRead("storage/a.txt"))
        assertEquals(root.resolve("comfyui/x.txt"), policy.resolveRead("comfyui/x.txt"))

        val denied = assertFailsWith<ToolFailure> { policy.resolveRead("other/a.txt") }
        assertEquals("PATH_DENIED", denied.code)

        // storage 是产物目录：能读不能写
        val writeDenied = assertFailsWith<ToolFailure> { policy.resolveWrite("storage/a.txt") }
        assertEquals("PATH_DENIED", writeDenied.code)
        assertEquals("产物", Files.readString(root.resolve("storage/a.txt")), "被拒的写不能动到原文件")
    }

    @Test
    fun `自动发现的 ComfyUI 目录只放宽读 不放宽写`() {
        val root = newRoot()
        // 模拟"用户机器上的 ComfyUI 在工作流目录那边"（真实形状见 ComfyRootsTest）
        val comfy = Files.createTempDirectory("comfyhub-auto-comfy").toRealPath()
        Files.createDirectories(comfy.resolve("user/default/workflows"))
        val workflow = comfy.resolve("user/default/workflows/krea2.json")
        Files.writeString(workflow, """{"nodes":[],"links":[]}""")

        // 没有自动发现时：读不到（用户报的 PATH_DENIED 就是这个）
        val plain = ToolPolicy(root, ToolPolicyConfig())
        assertEquals(
            "PATH_DENIED",
            assertFailsWith<ToolFailure> { plain.resolveRead(workflow.toString()) }.code,
        )

        // 自动发现之后：读得到，而且**用户配的那一份没被改**
        val auto = ToolPolicy(root, ToolPolicyConfig(), autoReadRoots = listOf(comfy))
        assertEquals(workflow, auto.resolveRead(workflow.toString()))
        assertEquals(listOf(root.resolve("comfyui"), root.resolve("storage")), auto.readRoots)
        assertEquals(
            listOf(root.resolve("comfyui"), root.resolve("storage"), comfy),
            auto.effectiveReadRoots,
        )

        // 写仍然只允许项目内的 comfyui：放行读不等于放行写
        assertEquals(
            "PATH_DENIED",
            assertFailsWith<ToolFailure> { auto.resolveWrite(workflow.toString()) }.code,
        )
    }

    @Test
    fun `空路径与非法字符报自己的错误码`() {
        val root = newRoot()
        val policy = ToolPolicy(root, ToolPolicyConfig())
        assertEquals(
            "INVALID_ARGUMENT",
            assertFailsWith<ToolFailure> { policy.resolveRead("   ") }.code,
        )
        assertEquals(
            "PATH_DENIED",
            assertFailsWith<ToolFailure> { policy.resolveWrite("comfyui/\u0000x") }.code,
        )
    }

    // --- 权限覆盖 -----------------------------------------------------------

    @Test
    fun `accessFor 认用户覆盖 isOverridden 如实报告`() {
        val root = newRoot()
        val tool = fakeTool("write_file", ToolAccess.ALLOW)

        val plain = ToolPolicy(root, ToolPolicyConfig())
        assertEquals(ToolAccess.ALLOW, plain.accessFor(tool))
        assertTrue(!plain.isOverridden(tool))

        val ask = ToolPolicy(root, ToolPolicyConfig(overrides = mapOf("write_file" to "ask")))
        assertEquals(ToolAccess.ASK, ask.accessFor(tool))
        assertTrue(ask.isOverridden(tool))

        val deny = ToolPolicy(root, ToolPolicyConfig(overrides = mapOf("write_file" to "deny")))
        assertEquals(ToolAccess.DENY, deny.accessFor(tool))
        assertTrue(deny.isOverridden(tool))

        // 取值非法时**回退出厂档**，不能当成"没配过"以外的任何东西，更不能放开
        val junk = ToolPolicy(root, ToolPolicyConfig(overrides = mapOf("write_file" to "nope")))
        assertEquals(ToolAccess.ALLOW, junk.accessFor(tool))
        assertTrue(!junk.isOverridden(tool))
    }

    @Test
    fun `被 deny 的工具不会出现在下发给模型的 specs 里`() {
        val root = newRoot()
        val registry = registryOf(root)
        val denied = ToolPolicy(
            root,
            ToolPolicyConfig(overrides = mapOf("write_file" to "deny", "comfy_sync_history" to "deny")),
        )
        val names = registry.specs(denied).map { it.name }
        assertTrue(!names.contains("write_file"), "deny 的写工具不能下发给模型")
        assertTrue(!names.contains("comfy_sync_history"))
        assertTrue(names.contains("read_file"), "没被覆盖的工具照常下发")
    }

    @Test
    fun `写目录根本身不能当写入目标`() {
        val root = newRoot()
        val policy = ToolPolicy(root, ToolPolicyConfig())
        val denied = assertFailsWith<ToolFailure> { policy.resolveWrite("comfyui") }
        assertEquals("PATH_DENIED", denied.code)
    }

    // --- 权限档（用户建议 ⑤）-------------------------------------------------

    @Test
    fun `自动允许档只把 ask 变 allow deny 与路径白名单都不放宽`() {
        val root = newRoot()
        val askTool = fakeTool("comfy_submit", ToolAccess.ASK)
        val allowTool = fakeTool("read_file", ToolAccess.ALLOW)
        val denyTool = fakeTool("write_file", ToolAccess.DENY)

        val ask = ToolPolicy(root, ToolPolicyConfig())
        assertTrue(!ask.fullPermission)
        assertEquals(ToolAccess.ASK, ask.accessFor(askTool))

        val full = ToolPolicy(root, ToolPolicyConfig(permissionMode = ToolPolicyConfig.PERMISSION_FULL))
        assertTrue(full.fullPermission)
        assertEquals(ToolAccess.ALLOW, full.accessFor(askTool), "自动允许档下不必再等批准")
        assertEquals(ToolAccess.ALLOW, full.accessFor(allowTool))
        assertEquals(ToolAccess.DENY, full.accessFor(denyTool), "deny 是「不许用」，不是「要不要问一下」")

        // 关键：路径边界一点都没松 —— 越界写照样拒绝（"自动允许"不是"随便写"）
        val escaped = assertFailsWith<ToolFailure> { full.resolveWrite("other/x.txt") }
        assertEquals("PATH_DENIED", escaped.code)
        val forbidden = assertFailsWith<ToolFailure> { full.resolveWrite(".git/config") }
        assertEquals("PATH_DENIED", forbidden.code)
    }

    @Test
    fun `权限档取值非法时回落询问 绝不当成自动允许`() {
        val root = newRoot()
        val bogus = ToolPolicy(root, ToolPolicyConfig(permissionMode = "yes-please"))
        assertEquals(ToolPolicyConfig.PERMISSION_ASK, bogus.permissionMode)
        assertTrue(!bogus.fullPermission)
        assertEquals(ToolAccess.ASK, bogus.accessFor(fakeTool("comfy_submit", ToolAccess.ASK)))
    }

    // --- 内置预算（查 ComfyUI 的次数）----------------------------------------

    @Test
    fun `查 ComfyUI 的预算默认 9 次`() {
        assertEquals(9, ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN)
        assertEquals(
            ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN,
            ToolPolicyConfig().maxComfyQueriesPerRun,
        )
    }

    @Test
    fun `库里冻着的旧查询预算不生效 一律以代码常量为准`() {
        val root = newRoot()
        // 老库里的那份 JSON 会写着当年的出厂值（用户切一次权限档就会写进去）
        val stored = ToolPolicyConfig(maxComfyQueriesPerRun = 3, permissionMode = ToolPolicyConfig.PERMISSION_FULL)

        val normalized = ToolPolicyConfig.normalizeStored(stored)
        assertEquals(9, normalized.maxComfyQueriesPerRun, "只改代码默认值不够，读库时要规范化")
        // 用户真的能改的项必须原样保留，不能被顺手冲掉
        assertEquals(ToolPolicyConfig.PERMISSION_FULL, normalized.permissionMode)

        val policy = ToolPolicy(root, normalized)
        assertEquals(9, policy.config.maxComfyQueriesPerRun)
        assertTrue(policy.fullPermission)
    }
}
