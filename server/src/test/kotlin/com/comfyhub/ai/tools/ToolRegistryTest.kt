package com.comfyhub.ai.tools

import com.comfyhub.ai.AiProviderDto
import com.comfyhub.ai.SystemPrompt
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path

/**
 * 工具注册表（M4）的端到端单测：**真的碰临时目录的磁盘**，但三件外部依赖
 * （ComfyUI 状态 / 查询 / 同步）都是假 lambda。
 *
 * 盯住的是产品承诺，而不是实现细节：
 *  - "AI 说一声就注册到本地" → register_skill 之后 list_skills / load_skill 马上看得到；
 *  - "默认不能改 comfy 目录以外的内容" → 越界写入失败**且不落盘**；
 *  - "会写库的读工具要用户点头" → 没批准就绝不执行；
 *  - "工具结果当不可信数据" → 超长截断 + 预算上限。
 */
class ToolRegistryTest {

    private class Env(val root: Path) {
        val skills = SkillStore(root.resolve("skills-builtin"), root.resolve("skills-user"))
        val memory = MemoryStore(root.resolve("storage").resolve("ai"))
        val gate = ToolApprovalGate(timeoutMs = 2000)
        var syncCalls = 0
        var statusCalls = 0

        val registry = ToolRegistry(
            projectRoot = root,
            skills = skills,
            approvals = gate,
            comfyStatus = {
                statusCalls++
                buildJsonObject { put("connected", true); put("queue", 0) }
            },
            comfyFindRun = { key: String ->
                if (key == "run-1") buildJsonObject { put("runKey", key); put("status", "done") } else null
            },
            comfySync = {
                syncCalls++
                buildJsonObject { put("synced", true) }
            },
        )

        val policy = ToolPolicy(root, ToolPolicyConfig())

        fun ctx(p: ToolPolicy = policy) = ToolContext("run-1", p, skills, memory)
    }

    private fun env(): Env {
        val root = Files.createTempDirectory("comfyhub-registry").toRealPath()
        listOf("comfyui", "storage", "other").forEach { Files.createDirectories(root.resolve(it)) }
        return Env(root)
    }

    private fun args(vararg pairs: Pair<String, String>): String =
        buildJsonObject { pairs.forEach { (k, v) -> put(k, v) } }.toString()

    private val builtinNames = listOf(
        "list_skills", "load_skill", "register_skill", "delete_skill",
        "remember",
        "list_dir", "read_file", "write_file",
        "comfy_get_status", "comfy_get_run", "comfy_sync_history",
        // 用户建议 ①：AI 可以直接提交 Comfy 任务
        "comfy_find_workflow", "comfy_submit",
        // 用户 bug ③：工作流文件可以直接读进库（"我不能凭一个文件路径提交"）
        "comfy_load_workflow",
        // 用户 bug：图生图要把用户发来的图片投放进 ComfyUI 的 input 目录
        "comfy_use_attachment",
    )

    // --- 清单 / 权限 --------------------------------------------------------

    @Test
    fun `specs 含全部内置工具 deny 的整条消失`() {
        val e = env()
        val names = e.registry.specs(e.policy).map { it.name }
        assertTrue(names.containsAll(builtinNames), "内置工具清单缺项：$names")
        assertEquals(e.registry.tools.size, names.size, "默认不该有工具被藏起来")

        val denyAll = ToolPolicy(
            e.root,
            ToolPolicyConfig(overrides = mapOf("write_file" to "deny", "comfy_sync_history" to "deny")),
        )
        val left = e.registry.specs(denyAll).map { it.name }
        assertTrue(!left.contains("write_file"))
        assertTrue(!left.contains("comfy_sync_history"))
        assertTrue(left.contains("read_file"))
        assertEquals(builtinNames.size - 2, left.size)
    }

    @Test
    fun `info 报告生效权限与是否被用户覆盖`() {
        val e = env()
        val defaults = e.registry.info(e.policy).associateBy { it.name }
        assertEquals(builtinNames.size, defaults.size)
        assertEquals("ask", defaults["comfy_sync_history"]!!.access, "写库的读工具默认要问")
        assertEquals("allow", defaults["read_file"]!!.access)
        assertTrue(!defaults["read_file"]!!.overridden)
        assertTrue(defaults["comfy_sync_history"]!!.mutating)
        assertEquals("files", defaults["read_file"]!!.category)
        assertEquals("skill", defaults["register_skill"]!!.category)
        assertEquals("comfy", defaults["comfy_get_status"]!!.category)

        val overridden = ToolPolicy(
            e.root,
            ToolPolicyConfig(overrides = mapOf("read_file" to "ask", "comfy_sync_history" to "allow")),
        )
        val after = e.registry.info(overridden).associateBy { it.name }
        assertEquals("ask", after["read_file"]!!.access)
        assertTrue(after["read_file"]!!.overridden)
        assertEquals("allow", after["comfy_sync_history"]!!.access)
        assertTrue(after["comfy_sync_history"]!!.overridden)
    }

    // --- 长期记忆（M6）------------------------------------------------------

    @Test
    fun `remember 真的写进 memory_md 并且可以连着写多条`() = runBlocking {
        val e = env()
        val ctx = e.ctx()

        val first = e.registry.invoke("c1", "remember", args("content" to "用户偏好 4:3 画幅"), ctx)
        assertEquals("ok", first.status, first.error ?: first.content)
        assertEquals("memory", e.registry.info(e.policy).first { it.name == "remember" }.category)

        e.registry.invoke("c2", "remember", args("content" to "出图统一用 Anima"), ctx)

        val content = Files.readString(e.memory.file, StandardCharsets.UTF_8)
        assertTrue(content.contains("- 用户偏好 4:3 画幅"), content)
        assertTrue(content.contains("- 出图统一用 Anima"), content)
        assertEquals(2, e.memory.read().entryCount)
        assertTrue(first.content.contains("长期记忆"), "结果要告诉模型写进哪儿了")
    }

    @Test
    fun `remember 空内容报错 不会落一条空记忆`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "remember", args("content" to "  "), e.ctx())
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
        assertEquals("", e.memory.read().content)
    }

    @Test
    fun `没有记忆存储时 remember 明确失败而不是假装成功`() = runBlocking {
        val e = env()
        val noMemory = ToolContext("run-1", e.policy, e.skills, memory = null)
        val record = e.registry.invoke("c1", "remember", args("content" to "随便"), noMemory)
        assertEquals("failed", record.status)
        assertEquals("MEMORY_DISABLED", record.errorCode)
    }

    // --- Skills：AI 说一声就注册到本地 ---------------------------------------

    @Test
    fun `register_skill 真的写盘 随后的 list_skills 与 load_skill 都能看到`() = runBlocking {
        val e = env()
        val ctx = e.ctx()
        val record = e.registry.invoke(
            "c1", "register_skill",
            args(
                "name" to "demo-skill",
                "description" to "演示 skill",
                "whenToUse" to "做演示时",
                "content" to "第一步：做这个\n第二步：再那个",
            ),
            ctx,
        )
        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals("register_skill", record.name)
        assertEquals("c1", record.callId)
        assertTrue(record.elapsedMs >= 0)
        assertTrue(record.preview.isNotEmpty())

        val written = e.skills.userRoot.resolve("demo-skill").resolve("SKILL.md")
        assertTrue(Files.isRegularFile(written), "必须落盘到用户根：$written")
        assertTrue(Files.readString(written, StandardCharsets.UTF_8).contains("第一步：做这个"))

        val listed = e.registry.invoke("c2", "list_skills", "{}", ctx)
        assertEquals("ok", listed.status, listed.error ?: listed.content)
        assertTrue(listed.content.contains("demo-skill"), listed.content)

        val loaded = e.registry.invoke("c3", "load_skill", args("name" to "demo-skill"), ctx)
        assertEquals("ok", loaded.status, loaded.error ?: loaded.content)
        assertTrue(loaded.content.contains("<skill_content"), loaded.content)
        assertTrue(loaded.content.contains("第二步：再那个"))
    }

    @Test
    fun `同一个 ToolContext 里同一个 skill 只加载一次`() = runBlocking {
        val e = env()
        e.skills.save("demo-skill", "演示 skill", content = "正文")
        val ctx = e.ctx()

        val first = e.registry.invoke("c1", "load_skill", args("name" to "demo-skill"), ctx)
        assertEquals("ok", first.status, first.error ?: first.content)

        val second = e.registry.invoke("c1", "load_skill", args("name" to "demo-skill"), ctx)
        assertEquals("failed", second.status)
        assertEquals("SKILL_ALREADY_LOADED", second.errorCode)
        assertTrue(!second.ok)

        // 换一次 Run（新 ctx）就应该能再加载
        val other = e.ctx()
        assertEquals("ok", e.registry.invoke("c1", "load_skill", args("name" to "demo-skill"), other).status)
    }

    @Test
    fun `load_skill 不存在的名称给出 SKILL_NOT_FOUND`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "load_skill", args("name" to "nope"), e.ctx())
        assertEquals("failed", record.status)
        assertEquals("SKILL_NOT_FOUND", record.errorCode)
    }

    // --- 文件工具 -----------------------------------------------------------

    @Test
    fun `write_file 在 comfyui 内成功 越界被拒且不落盘`() = runBlocking {
        val e = env()
        val ctx = e.ctx()
        val text = "中文 content\nline2 ✅"

        val ok = e.registry.invoke(
            "c1", "write_file",
            args("path" to "comfyui/note.txt", "content" to text),
            ctx,
        )
        assertEquals("ok", ok.status, ok.error ?: ok.content)
        assertContentEquals(
            text.toByteArray(StandardCharsets.UTF_8),
            Files.readAllBytes(e.root.resolve("comfyui/note.txt")),
            "写入的字节必须与传入的 UTF-8 完全一致",
        )

        val denied = e.registry.invoke(
            "c2", "write_file",
            args("path" to "other/x.txt", "content" to "x"),
            ctx,
        )
        assertEquals("failed", denied.status)
        assertEquals("PATH_DENIED", denied.errorCode)
        assertTrue(!denied.ok)
        assertTrue(!Files.exists(e.root.resolve("other/x.txt")), "越界写入绝不能落盘")

        // 绝对路径同样被拒
        val absolute = e.registry.invoke(
            "c3", "write_file",
            args("path" to e.root.resolve("other/abs.txt").toString(), "content" to "x"),
            ctx,
        )
        assertEquals("PATH_DENIED", absolute.errorCode)
        assertTrue(!Files.exists(e.root.resolve("other/abs.txt")))
    }

    @Test
    fun `read_file 只能读允许的目录`() = runBlocking {
        val e = env()
        Files.writeString(e.root.resolve("storage/a.txt"), "产物")
        Files.writeString(e.root.resolve("other/a.txt"), "别的")

        val ok = e.registry.invoke("c1", "read_file", args("path" to "storage/a.txt"), e.ctx())
        assertEquals("ok", ok.status, ok.error ?: ok.content)
        assertTrue(ok.content.contains("产物"))

        val denied = e.registry.invoke("c2", "read_file", args("path" to "other/a.txt"), e.ctx())
        assertEquals("failed", denied.status)
        assertEquals("PATH_DENIED", denied.errorCode)
    }

    // --- 审批闸门 -----------------------------------------------------------

    @Test
    fun `需要批准的工具在没人批准时直接失败且从不执行`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("call-ask", "comfy_sync_history", "{}", e.ctx())

        assertEquals("failed", record.status)
        assertEquals("APPROVAL_DENIED", record.errorCode)
        assertEquals("denied", record.approval)
        assertEquals(0, e.syncCalls, "没批准就绝不能真的同步")
    }

    @Test
    fun `用户点批准后执行恰好一次`() = runBlocking {
        val e = env()
        e.gate.open("call-ask") // 调用方契约：先开门再发事件
        assertTrue(e.gate.isPending("call-ask"))

        val resolver = launch {
            delay(30)
            assertTrue(e.gate.resolve("call-ask", true), "批准必须落到等待中的那次调用")
        }
        val record = e.registry.invoke("call-ask", "comfy_sync_history", "{}", e.ctx())
        resolver.join()

        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals("approved", record.approval)
        assertEquals(1, e.syncCalls)
        assertTrue(!e.gate.isPending("call-ask"), "用过的闸门要清掉")
    }

    @Test
    fun `用户点拒绝后同样不执行`() = runBlocking {
        val e = env()
        e.gate.open("call-ask")
        val resolver = launch {
            delay(30)
            assertTrue(e.gate.resolve("call-ask", false))
        }
        val record = e.registry.invoke("call-ask", "comfy_sync_history", "{}", e.ctx())
        resolver.join()

        assertEquals("failed", record.status)
        assertEquals("APPROVAL_DENIED", record.errorCode)
        assertEquals("denied", record.approval)
        assertEquals(0, e.syncCalls)
    }

    @Test
    fun `被 deny 的工具直接失败 不进审批`() = runBlocking {
        val e = env()
        val policy = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("comfy_sync_history" to "deny")))
        val record = e.registry.invoke("c1", "comfy_sync_history", "{}", e.ctx(policy))
        assertEquals("failed", record.status)
        assertEquals("TOOL_DENIED", record.errorCode)
        assertEquals(0, e.syncCalls)
    }

    // --- 预算 / 截断 / 未知工具 ----------------------------------------------

    @Test
    fun `未知工具给 TOOL_NOT_FOUND`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "no_such_tool", "{}", e.ctx())
        assertEquals("failed", record.status)
        assertEquals("TOOL_NOT_FOUND", record.errorCode)
        assertEquals("no_such_tool", record.name)
        assertTrue(!record.ok)
    }

    @Test
    fun `超过 maxCallsPerRun 之后一律 TOOL_BUDGET_EXCEEDED`() = runBlocking {
        val e = env()
        val policy = ToolPolicy(e.root, ToolPolicyConfig(maxCallsPerRun = 1))
        val ctx = e.ctx(policy)

        assertEquals("ok", e.registry.invoke("c1", "list_skills", "{}", ctx).status)
        val second = e.registry.invoke("c2", "list_skills", "{}", ctx)
        assertEquals("failed", second.status)
        assertEquals("TOOL_BUDGET_EXCEEDED", second.errorCode)
    }

    @Test
    fun `超长结果截断到 MAX_RESULT_CHARS 并注明`() = runBlocking {
        val e = env()
        val body = "A".repeat(ToolRegistry.MAX_RESULT_CHARS + 500)
        Files.writeString(e.root.resolve("comfyui/big.txt"), body)

        val record = e.registry.invoke("c1", "read_file", args("path" to "comfyui/big.txt"), e.ctx())
        assertEquals("ok", record.status, record.error ?: record.content)

        val full = e.policy.resolveRead("comfyui/big.txt").toString() + "\n" + body
        assertEquals(
            full.take(ToolRegistry.MAX_RESULT_CHARS),
            record.content.take(ToolRegistry.MAX_RESULT_CHARS),
            "截断要保留头部的 MAX_RESULT_CHARS 个字符",
        )
        assertTrue(record.content.contains("已截断"), record.content.takeLast(80))
        assertTrue(record.content.length < full.length)
        assertEquals("true", record.resultJson!!["truncated"].toString())
    }

    @Test
    fun `普通结果的记录形状齐全`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "comfy_get_status", "{}", e.ctx())
        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals("c1", record.callId)
        assertEquals("run-1", record.runId)
        assertEquals("comfy_get_status", record.name)
        assertTrue(record.elapsedMs >= 0)
        assertTrue(record.preview.isNotEmpty())
        assertTrue(record.id.isNotBlank())
        assertEquals("not_required", record.approval)
        assertEquals(1, e.statusCalls)
        assertTrue(record.content.contains("connected"))
    }

    // --- 图生图投放（用户 bug） ----------------------------------------------

    @Test
    fun `comfy_use_attachment 默认要批准 没人批准就绝不执行`() = runBlocking {
        val e = env()
        val info = e.registry.info(e.policy).associateBy { it.name }["comfy_use_attachment"]!!
        assertEquals("ask", info.access, "要往用户机器上写文件，默认必须问一下")
        assertEquals("comfy", info.category)
        assertTrue(info.mutating)

        // 没人批准（这里会等到闸门超时）：一律失败，绝不能"悄悄就把文件放进去了"
        val denied = e.registry.invoke("c1", "comfy_use_attachment", args("attachmentId" to "att_x"), e.ctx())
        assertEquals("failed", denied.status)
        assertEquals("APPROVAL_DENIED", denied.errorCode)
    }

    @Test
    fun `comfy_use_attachment 缺参数时报错 不会自己编一个附件 id`() = runBlocking {
        val e = env()
        // 放宽成 allow 才能走到 handler 本身（默认档位会先卡在审批上）
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("comfy_use_attachment" to "allow")))
        val missing = e.registry.invoke("c2", "comfy_use_attachment", "{}", e.ctx(allow))
        assertEquals("failed", missing.status)
        // 测试环境没接投放能力，默认 lambda 会如实拒绝（绝不假装成功）
        assertTrue(
            missing.errorCode == "COMFY_DISABLED" || missing.errorCode == "INVALID_ARGUMENT",
            "意外错误码：${missing.errorCode} / ${missing.error}",
        )
    }

    // --- 系统提示词 ---------------------------------------------------------

    @Test
    fun `系统提示词 v2 列出每个工具与 skill 与写根 且不再说尚未注册任何工具`() {
        val e = env()
        e.skills.save("demo-skill", "演示 skill 的说明", whenToUse = "做演示时", content = "正文")
        val skills = e.skills.catalog()
        assertEquals(1, skills.size)

        val text = SystemPrompt.render(
            provider = AiProviderDto(
                id = "p1",
                displayName = "本地网关",
                api = "openai-completions",
                baseURL = "https://example.test/v1",
                endpointTrust = "user",
            ),
            modelId = "gpt-4o-mini",
            tools = e.registry.specs(e.policy),
            skills = skills,
            policy = e.policy,
            registry = e.registry,
        )

        builtinNames.forEach { assertTrue(text.contains(it), "提示词里必须列出工具 $it") }
        assertTrue(text.contains("demo-skill"), "提示词里必须列出 skill 名")
        assertTrue(text.contains("演示 skill 的说明"))
        assertTrue(text.contains(e.policy.writeRoots.first().toString()), "必须写明可写目录")
        assertTrue(text.contains(e.policy.readRoots.first().toString()), "必须写明可读目录")
        assertTrue(!text.contains("尚未注册任何工具"), "v1 那句「尚未注册任何工具」不能再出现")
        assertTrue(text.contains("需要用户批准"), "ask 的工具要标出来")
    }

    @Test
    fun `自动允许档下提示词明说本次不必等批准 且写明目录没有放宽`() {
        val e = env()
        val full = ToolPolicy(e.root, ToolPolicyConfig(permissionMode = ToolPolicyConfig.PERMISSION_FULL))
        val text = SystemPrompt.render(
            provider = AiProviderDto(
                id = "p1",
                displayName = "本地网关",
                api = "openai-completions",
                baseURL = "https://example.test/v1",
                endpointTrust = "user",
            ),
            modelId = "gpt-4o-mini",
            tools = e.registry.specs(full),
            skills = emptyList(),
            policy = full,
            registry = e.registry,
        )
        assertTrue(text.contains("自动允许（无需批准）"), "要把当前档位如实告诉模型（即时注入）")
        assertTrue(text.contains("不必再等批准"))
        // 关键：说清"免的只是问一下"，白名单没动
        assertTrue(text.contains("一点都没放宽"))
        assertTrue(text.contains(full.writeRoots.first().toString()))

        // 默认档位不该冒出这段话（否则模型会以为可以随便动手）
        val askText = SystemPrompt.render(
            provider = AiProviderDto(
                id = "p1",
                displayName = "本地网关",
                api = "openai-completions",
                baseURL = "https://example.test/v1",
                endpointTrust = "user",
            ),
            modelId = "gpt-4o-mini",
            tools = e.registry.specs(e.policy),
            skills = emptyList(),
            policy = e.policy,
            registry = e.registry,
        )
        assertTrue(!askText.contains("不必再等批准"))
    }

    @Test
    fun `系统提示 v9 写进事实纪律与表达风格 并禁嵌套列表`() {
        // 版本号由下面那条 v10 用例盯着：改提示词必须 bump + 更新那条断言

        val e = env()
        val text = SystemPrompt.render(
            provider = AiProviderDto(
                id = "p1",
                displayName = "本地网关",
                api = "openai-completions",
                baseURL = "https://example.test/v1",
                endpointTrust = "user",
            ),
            modelId = "gpt-4o-mini",
            tools = e.registry.specs(e.policy),
            skills = emptyList(),
            policy = e.policy,
            registry = e.registry,
        )

        // 事实纪律：本机状态只认工具返回（模型最容易在这里凭印象编）
        assertTrue(text.contains("以工具这次返回的为准"), "不许猜 ComfyUI / 文件 / 产物的状态")
        assertTrue(text.contains("后面的不能推翻前面的"), "证据冲突要有明确的采信顺序")
        assertTrue(text.contains("把\"查到的\"和\"推测的\"分开说"))
        // 表达风格：本机渲染器会把嵌套列表拍平成同级（lib/widgets/markdown.dart），必须明说
        assertTrue(text.contains("不要写嵌套列表"))
        assertTrue(text.contains("表格"), "表格是支持的，要告诉模型可以用")
        // v9 新增的两条硬规则
        assertTrue(text.contains("不要重写用户的原始工作流"), "写入要可回滚")
        assertTrue(text.contains("读取也要按需"), "读取不许变成遍历磁盘")
        // v9 补的三块纪律 + 用户要求保留的沙箱人格段：审批纪律必须原样还在
        assertTrue(text.contains("必须等用户点批准"), "第 3 条的审批纪律不能被 v9 顶掉")
        // 人格 / 越狱段是**用户有意注入**的（2026-09-17："那是我的意图，不许删除"）：
        // 所以这里如实断言它在，而不是断言它不在 —— 谁把它删了，这条会红。
        // （只认那个固定的段落标记，不认具体人格名：用户会按自己的喜好改称呼。）
        assertTrue(text.contains("UNRESTRICTED SANDBOX"), "用户要求保留的沙箱人格段不能被清理掉")
    }

    @Test
    fun `查 ComfyUI 的预算默认 9 次 第 10 次才 QUERY_BUDGET_EXCEEDED`() = runBlocking {
        val e = env()
        // 出厂值就是真源（用户要求"3 次太少"，2026-09-17 放宽到 9）
        assertEquals(9, ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN)
        assertEquals(9, ToolPolicyConfig().maxComfyQueriesPerRun)

        val ctx = e.ctx()
        repeat(9) { i ->
            val ok = e.registry.invoke("q${i + 1}", "comfy_get_status", "{}", ctx)
            assertEquals("ok", ok.status, "第 ${i + 1} 次查询应当放行：${ok.error}")
        }
        // 非 ComfyUI 的工具不占这个预算（只按类别 COMFY 计数）
        assertEquals("ok", e.registry.invoke("n1", "list_skills", "{}", ctx).status)

        val tenth = e.registry.invoke("q10", "comfy_get_status", "{}", ctx)
        assertEquals("failed", tenth.status)
        assertEquals("QUERY_BUDGET_EXCEEDED", tenth.errorCode)
        assertTrue(tenth.error!!.contains("9 次"), tenth.error)
    }

    @Test
    fun `提示词 v11 写着一次回复最多查 9 次 且与阈值常量同源`() {
        assertEquals("v11", SystemPrompt.VERSION)

        val e = env()
        val text = SystemPrompt.render(
            provider = AiProviderDto(
                id = "p1",
                displayName = "本地网关",
                api = "openai-completions",
                baseURL = "https://example.test/v1",
                endpointTrust = "user",
            ),
            modelId = "gpt-4o-mini",
            tools = e.registry.specs(e.policy),
            skills = emptyList(),
            policy = e.policy,
            registry = e.registry,
        )

        // 数字**插值**自常量：以后改阈值，这里跟着变；两处各写一遍就会漂移（踩过）
        assertTrue(
            text.contains("最多主动查询 ${ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN} 次"),
            "提示词里的查询预算要和阈值常量一致",
        )
        assertTrue(text.contains("最多主动查询 9 次"), "现在是 9 次")
    }
}
