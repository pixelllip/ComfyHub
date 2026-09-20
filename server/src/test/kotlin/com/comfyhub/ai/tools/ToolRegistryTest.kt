package com.comfyhub.ai.tools

import com.comfyhub.ai.AiProviderDto
import com.comfyhub.ai.SystemPrompt
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
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

    private class Env(
        val root: Path,
        /**
         * 库里搜工作流的结果（默认"一条都没有"）。用户 bug ⑤ 的回落那条路要用：
         * 库里搜不到时，工具必须回头看**本机 ComfyUI 已保存的工作流文件**。
         */
        val findWorkflow: (String, Int, Boolean) -> WorkflowSearch = { _, _, _ ->
            WorkflowSearch(0, buildJsonObject { put("count", 0) })
        },
        /** 本机工作流文件的假清单（默认空）。 */
        val listWorkflows: (String?, Int) -> JsonObject = { _, _ ->
            buildJsonObject { put("count", 0) }
        },
        /** 联网搜索客户端（默认 null = 走 SEARCH_DISABLED 那条路）。 */
        val search: WebSearchClient? = null,
        /** Civitai 搜索客户端（默认 null = 走 CIVITAI_DISABLED 那条路）。 */
        val civitai: CivitaiClient? = null,
    ) {
        val skills = SkillStore(root.resolve("skills-builtin"), root.resolve("skills-user"))
        val memory = MemoryStore(root.resolve("storage").resolve("ai"))
        val gate = ToolApprovalGate(timeoutMs = 2000)
        var syncCalls = 0
        var statusCalls = 0

        val registry = ToolRegistry(
            projectRoot = root,
            skills = skills,
            approvals = gate,
            comfyFindWorkflow = findWorkflow,
            comfyListWorkflows = listWorkflows,
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
            search = search,
            civitai = civitai,
        )

        val policy = ToolPolicy(root, ToolPolicyConfig())

        fun ctx(p: ToolPolicy = policy) = ToolContext("run-1", p, skills, memory)
    }

    private fun env(search: WebSearchClient? = null): Env {
        val root = Files.createTempDirectory("comfyhub-registry").toRealPath()
        listOf("comfyui", "storage", "other").forEach { Files.createDirectories(root.resolve(it)) }
        return Env(root, search = search)
    }

    private fun envCivitai(civitai: CivitaiClient?): Env {
        val root = Files.createTempDirectory("comfyhub-registry").toRealPath()
        listOf("comfyui", "storage", "other").forEach { Files.createDirectories(root.resolve(it)) }
        return Env(root, civitai = civitai)
    }

    /** 假 Civitai 客户端：只记"被谁用什么参数调了"，不起 HTTP（真 HTTP 在 CivitaiSearchTest 里钉）。 */
    private class FakeCivitai(
        private val models: List<CivitaiModel> = listOf(
            CivitaiModel(
                id = 42,
                name = "水墨风 LoRA",
                type = "LORA",
                baseModel = "SDXL 1.0",
                nsfw = false,
                creator = "someone",
                downloads = 1234,
                thumbsUp = 56,
                pageUrl = "https://civitai.com/models/42",
                description = "把照片转成水墨画风格，适合风景与人物。",
                trainedWords = listOf("inkwash", "chinese painting"),
            ),
        ),
    ) : CivitaiClient {
        var calls = 0
        var lastNsfw: Boolean? = null
        var lastType: String? = null

        override suspend fun search(
            query: String,
            allowNsfw: Boolean,
            type: String?,
            limit: Int,
        ): CivitaiOutcome {
            calls++
            lastNsfw = allowNsfw
            lastType = type
            val context = models.joinToString("\n\n") { m ->
                buildString {
                    append("[1] ").append(m.name).append('\n')
                    append("用途：").append(m.description)
                    if (m.trainedWords.isNotEmpty()) {
                        append("\n触发词：").append(m.trainedWords.joinToString(", "))
                    }
                }
            }
            return CivitaiOutcome(
                query = query,
                models = models,
                context = context,
                meta = buildJsonObject {
                    put("count", models.size)
                    put("nsfw", allowNsfw)
                },
            )
        }
    }

    private fun args(vararg pairs: Pair<String, String>): String =
        buildJsonObject { pairs.forEach { (k, v) -> put(k, v) } }.toString()

    /** 假联网客户端：不碰 HTTP，只记录调用并吐固定结果（HTTP 层由 KeenableSearchTest 单独钉）。 */
    private class FakeSearch(
        var results: List<WebResult> = listOf(
            WebResult("标题一", "https://example.com/a", "正文一", "2026-09-20"),
        ),
    ) : WebSearchClient {
        var searchCalls = 0
        var pageCalls = 0

        override suspend fun search(
            query: String,
            site: String?,
            publishedAfter: String?,
            publishedBefore: String?,
            mode: String?,
        ): WebSearchOutcome {
            searchCalls++
            val rs = results
            return WebSearchOutcome(
                query = query,
                results = rs,
                context = rs.mapIndexed { i, r -> "[${i + 1}] ${r.title} (${r.url})\n${r.snippet}" }
                    .joinToString("\n\n"),
                meta = buildJsonObject { put("count", rs.size); put("query", query); put("keyless", true) },
            )
        }

        override suspend fun page(url: String, maxChars: Int): WebPage {
            pageCalls++
            return WebPage(url = url, title = "页标题", content = "页正文一。")
        }
    }

    private val builtinNames = listOf(
        "list_skills", "load_skill", "register_skill", "delete_skill",
        "remember",
        "list_dir", "read_file", "write_file",
        "comfy_get_status", "comfy_get_run", "comfy_sync_history",
        // 用户建议 ①：AI 可以直接提交 Comfy 任务
        "comfy_find_workflow", "comfy_submit",
        // 用户 bug ③：工作流文件可以直接读进库（"我不能凭一个文件路径提交"）
        "comfy_load_workflow",
        // 用户 bug ⑤：库只收"捕获过的运行"，所以要能列出**本机 ComfyUI 已保存的工作流文件**
        "comfy_list_workflows",
        // 用户 bug：图生图要把用户发来的图片投放进 ComfyUI 的 input 目录
        "comfy_use_attachment",
        // 2026-09-20：联网搜索（用户定的规则——只用通用方案 Keenable，不碰各家原生内置搜索）
        "web_search", "web_fetch",
        // 用户建议 ④：允许 AI 搜 Civitai 的模型库（NSFW 档走 .red 且要令牌）
        "civitai_search",
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
        assertEquals("web", defaults["web_search"]!!.category)
        assertEquals("web", defaults["web_fetch"]!!.category)
        assertEquals("web", defaults["civitai_search"]!!.category)
        // 联网是唯一会把数据送出本机的类别：出厂必须停在「询问」档
        assertEquals("ask", defaults["web_search"]!!.access)
        assertEquals("ask", defaults["web_fetch"]!!.access)
        // Civitai 搜索同理（NSFW 那一档用户的原话就是"这种情况可以询问用户"）
        assertEquals("ask", defaults["civitai_search"]!!.access)

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
        // AI 写的每条会带 `[日期] ` 前缀（用户要求：便于按时间查找 / 清理）
        val today = java.time.LocalDate.now().toString()
        assertTrue(content.contains("- [$today] 用户偏好 4:3 画幅"), content)
        assertTrue(content.contains("- [$today] 出图统一用 Anima"), content)
        assertEquals(2, e.memory.read().entryCount)
        assertTrue(first.content.contains("长期记忆"), "结果要告诉模型写进哪儿了")
    }

    @Test
    fun `remember 一轮最多新增 N 条 超了直接拒绝（用户要求：别让条数失控）`() = runBlocking {
        val e = env()
        val ctx = e.ctx()

        repeat(MemoryStore.MAX_ENTRIES_PER_RUN) { i ->
            val r = e.registry.invoke("c$i", "remember", args("content" to "偏好第 $i 条"), ctx)
            assertEquals("ok", r.status, r.error ?: r.content)
        }
        val extra = e.registry.invoke("cX", "remember", args("content" to "再来一条"), ctx)
        assertEquals("failed", extra.status)
        assertEquals("MEMORY_RATE_LIMITED", extra.errorCode)
        assertEquals(MemoryStore.MAX_ENTRIES_PER_RUN, e.memory.read().entryCount, "被拒的那条不该落盘")

        // 判重命中的"重复说一遍"不占额度：模型啰嗦不该让它后面记不了真正的新事实
        val again = e.registry.invoke("cY", "remember", args("content" to "偏好第 0 条"), ctx)
        assertEquals("ok", again.status, "判重命中不算新增，不该被节流挡住")
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
    fun `read_file 超过单次上限时按 offset 分段读 不会丢内容`() = runBlocking {
        val e = env()
        // 用一份"一次只让读 1000 字节"的策略，把分段读这条路走出来（跟上限多大无关）
        val tight = ToolPolicy(e.root, ToolPolicyConfig(maxReadBytes = 1000))
        val ctx = e.ctx(tight)
        val body = (1..300).joinToString("\n") { "第 $it 行内容" }
        Files.writeString(e.root.resolve("comfyui/workflow.json"), body)

        val head = e.policy.resolveRead("comfyui/workflow.json").toString() + "\n"
        fun chunk(record: com.comfyhub.ai.tools.ToolCallRecord): String =
            record.content.removePrefix(head).substringBefore("\n…（")

        // 一页一页接着读，直到读完（按返回的 offset 走，不自己猜）
        val first = e.registry.invoke(
            "c0", "read_file",
            args("path" to "comfyui/workflow.json", "maxBytes" to "1000"),
            ctx,
        )
        assertEquals("ok", first.status, first.error ?: first.content)
        assertTrue(first.content.contains("offset="), "没读完必须告诉模型下一段从哪读：${first.content.takeLast(140)}")

        var joined = chunk(first)
        var offset = Regex("offset=(\\d+)").find(first.content)!!.groupValues[1].toInt()
        var noMoreNote = false
        var guard = 0
        while (guard++ < 12) {
            val next = e.registry.invoke(
                "c$guard", "read_file",
                args("path" to "comfyui/workflow.json", "maxBytes" to "1000", "offset" to offset.toString()),
                ctx,
            )
            assertEquals("ok", next.status, next.error ?: next.content)
            joined += chunk(next)
            val m = Regex("offset=(\\d+)").find(next.content)
            if (m == null) {
                noMoreNote = true
                break
            }
            offset = m.groupValues[1].toInt()
        }
        assertTrue(noMoreNote, "读到文件尾之后不该再提示接着读")
        assertEquals(body, joined, "分段读拼起来必须和原文一模一样（中文不能被切成两半）")
        assertTrue(!joined.contains('\uFFFD'), "页尾不能留乱码")
    }

    @Test
    fun `单次读写上限的出厂值不会被库里冻住的旧值盖回去`() {
        // 老库里的 ai.tools.policy 冻着 256KB / 1MB（本机实测就是这个值）：
        // 只在代码里上调默认值对老用户完全无效，必须过 normalizeStored
        val frozen = ToolPolicyConfig(
            maxComfyQueriesPerRun = 3,
            maxWebQueriesPerRun = 2,
            maxReadBytes = 256 * 1024,
            maxWriteBytes = 1024 * 1024,
        )
        val normalized = ToolPolicyConfig.normalizeStored(frozen)
        assertEquals(ToolPolicyConfig.DEFAULT_MAX_READ_BYTES, normalized.maxReadBytes)
        assertEquals(ToolPolicyConfig.DEFAULT_MAX_WRITE_BYTES, normalized.maxWriteBytes)
        assertEquals(ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN, normalized.maxComfyQueriesPerRun)
        assertEquals(ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN, normalized.maxWebQueriesPerRun)
        // 用户可改的那两项**不能**被覆盖（库里的值必须生效）
        assertEquals(8, normalized.maxToolSteps)
        assertEquals(16, normalized.maxCallsPerRun)
    }

    @Test
    fun `write_file 超限时报 TOO_LARGE 并指出用 append 分多次写`() = runBlocking {
        val e = env()
        val tight = ToolPolicy(e.root, ToolPolicyConfig(maxWriteBytes = 16))
        val record = e.registry.invoke(
            "c1", "write_file",
            args("path" to "comfyui/big.txt", "content" to "x".repeat(64)),
            e.ctx(tight),
        )
        assertEquals("failed", record.status)
        assertEquals("TOO_LARGE", record.errorCode)
        assertTrue(record.error!!.contains("append"), record.error!!)
        assertTrue(!Files.exists(e.root.resolve("comfyui/big.txt")))
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

    // --- 本机工作流文件（用户 bug ⑤） ----------------------------------------

    @Test
    fun `comfy_list_workflows 只读、默认不需要批准`() {
        val e = env()
        val info = e.registry.info(e.policy).associateBy { it.name }["comfy_list_workflows"]!!
        assertEquals("allow", info.access, "只列用户的文件，不该要批准")
        assertEquals("comfy", info.category)
        assertTrue(!info.mutating, "它既不改用户的文件，也不写库")
    }

    @Test
    fun `comfy_find_workflow 库里搜不到时自动列出本机工作流文件`() = runBlocking {
        // 现场（用户 bug ⑤）：首次使用时库里一条工作流都没有（库只收"捕获过的运行"），
        // 而用户机器上的 krea2 工作流文件一直在磁盘上。AI 必须能自己找到它，
        // 而不是回一句"库里没有"、让用户先手跑一次。
        val local = buildJsonObject {
            put("count", 1)
            put("total", 1)
            put(
                "files",
                buildJsonArray {
                    add(
                        buildJsonObject {
                            put("name", "krea2SFWNSFWUncensoredImageTo_v10.json")
                            put("path", "D:\\Comfy-Desktop\\ComfyUI-Installs\\ComfyUI\\ComfyUI\\user\\default\\workflows\\krea2SFWNSFWUncensoredImageTo_v10.json")
                            put("format", "ui")
                            put("inLibrary", false)
                        },
                    )
                },
            )
        }
        val e = Env(Files.createTempDirectory("comfyhub-registry-local").toRealPath(), listWorkflows = { _, _ -> local })
        val record = e.registry.invoke("c1", "comfy_find_workflow", args("query" to "krea2"), e.ctx())

        assertEquals("ok", record.status, record.error ?: record.content)
        assertTrue(record.content.contains("krea2SFWNSFWUncensoredImageTo_v10.json"), "文件名要出现在给模型的正文里")
        val json = record.resultJson!!
        assertEquals(1, (json["localWorkflows"] as JsonArray).size)
        assertEquals(0, (json["count"] as JsonPrimitive).content.toInt(), "库里确实是 0 条")
        val hint = (json["hint"] as JsonPrimitive).content
        assertTrue(hint.contains("comfy_load_workflow"), "要告诉模型下一步怎么把文件读进库：$hint")
    }

    @Test
    fun `两个来源都没有时 NOT_FOUND 指向 comfy_list_workflows`() = runBlocking {
        val e = env()
        val record = e.registry.invoke("c1", "comfy_find_workflow", args("query" to "谁都没有的工作流"), e.ctx())
        assertEquals("failed", record.status)
        assertEquals("NOT_FOUND", record.errorCode)
        assertTrue(record.error!!.contains("comfy_list_workflows"), "要让模型知道还有「列本机文件」这条路：${record.error}")
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

    // --- 联网搜索（2026-09-20：web_search / web_fetch，Keenable 通用方案） ----

    @Test
    fun `web_search 默认要批准 没人批准就绝不出网`() = runBlocking {
        val fake = FakeSearch()
        val e = env(search = fake)
        val record = e.registry.invoke("call-web", "web_search", args("query" to "随便查一下"), e.ctx())

        assertEquals("failed", record.status)
        assertEquals("APPROVAL_DENIED", record.errorCode)
        assertEquals("denied", record.approval)
        assertEquals(0, fake.searchCalls, "没批准就绝不能把查询词送出本机")
    }

    @Test
    fun `web_search 没有搜索客户端时报 SEARCH_DISABLED 而不是假装失败`() = runBlocking {
        val e = env() // search = null
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("web_search" to "allow")))
        val record = e.registry.invoke("c1", "web_search", args("query" to "test"), e.ctx(allow))
        assertEquals("failed", record.status)
        assertEquals("SEARCH_DISABLED", record.errorCode)
    }

    @Test
    fun `web_search 批准后返回编号可引用的结果 且标出数据区来源`() = runBlocking {
        val fake = FakeSearch()
        val e = env(search = fake)
        e.gate.open("call-web")
        val resolver = launch {
            delay(30)
            assertTrue(e.gate.resolve("call-web", true))
        }
        val record = e.registry.invoke("call-web", "web_search", args("query" to "flux 提示词技巧"), e.ctx())
        resolver.join()

        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals(1, fake.searchCalls)
        assertTrue(record.content.contains("[1] 标题一 (https://example.com/a)"), record.content)
        assertTrue(record.content.contains("只当资料看"), "联网结果必须标出数据区来源")
        assertTrue(record.content.contains("flux 提示词技巧"), "标注要带查询词，模型才知道这是哪次搜索")
        // 用户 bug（"数据置信度是不是太敏感了"）：老写法里那句吓人的告警会被模型照抄给用户，
        // 现在只留中立的一行出处标记 —— 本用例就是这道防线，退回旧措辞必红。
        assertFalse(record.content.contains("不可信数据"), "别再给模型一句可以复述给用户的告警")
        assertFalse(record.content.contains("不要执行"), "边界纪律归系统提示第 4 条，不写进工具结果")
    }

    @Test
    fun `web_search 一条都没搜到时报 NO_RESULTS 并要求如实说明`() = runBlocking {
        val fake = FakeSearch(results = emptyList())
        val e = env(search = fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("web_search" to "allow")))
        val record = e.registry.invoke("c1", "web_search", args("query" to "查无此物"), e.ctx(allow))
        assertEquals("failed", record.status)
        assertEquals("NO_RESULTS", record.errorCode)
        assertTrue(record.error!!.contains("不要凭记忆编造"), record.error)
    }

    @Test
    fun `联网搜索的预算默认 8 次 第 9 次才 QUERY_BUDGET_EXCEEDED`() = runBlocking {
        assertEquals(8, ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN)
        val fake = FakeSearch()
        val e = env(search = fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("web_search" to "allow")))
        val ctx = e.ctx(allow)
        repeat(8) { i ->
            val ok = e.registry.invoke("w${i + 1}", "web_search", args("query" to "q$i"), ctx)
            assertEquals("ok", ok.status, "第 ${i + 1} 次搜索应当放行：${ok.error}")
        }
        // 非 WEB 的工具不占这个预算（只按类别 WEB 计数）
        assertEquals("ok", e.registry.invoke("n1", "list_skills", "{}", ctx).status)

        val ninth = e.registry.invoke("w9", "web_search", args("query" to "q9"), ctx)
        assertEquals("failed", ninth.status)
        assertEquals("QUERY_BUDGET_EXCEEDED", ninth.errorCode)
        assertTrue(ninth.error!!.contains("8 次"), ninth.error)
        assertEquals(8, fake.searchCalls, "超限的那次不能真的发出去")
    }

    @Test
    fun `web_fetch 把页面正文带回 并标出数据区来源`() = runBlocking {
        val fake = FakeSearch()
        val e = env(search = fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("web_fetch" to "allow")))
        val record = e.registry.invoke("c1", "web_fetch", args("url" to "https://example.com/a"), e.ctx(allow))
        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals(1, fake.pageCalls)
        assertTrue(record.content.contains("页标题"), record.content)
        assertTrue(record.content.contains("只当资料看"))
        assertFalse(record.content.contains("不可信数据"), "不要在工具结果里留一句会被模型复述的告警")
        assertTrue(record.content.contains("https://example.com/a"))
    }

    // --- Civitai 模型库搜索（用户建议 ④/⑦，2026-09-20） -------------------

    @Test
    fun `civitai_search 默认要批准 没人批准就绝不出网`() = runBlocking {
        val fake = FakeCivitai()
        val e = envCivitai(fake)
        val record = e.registry.invoke("call-civ", "civitai_search", args("query" to "水墨风"), e.ctx())

        assertEquals("failed", record.status)
        assertEquals("APPROVAL_DENIED", record.errorCode)
        assertEquals("denied", record.approval)
        assertEquals(0, fake.calls, "没批准就绝不能把查询词送去 Civitai")
    }

    @Test
    fun `civitai_search 没有客户端时报 CIVITAI_DISABLED 而不是假装没搜到`() = runBlocking {
        val e = envCivitai(null)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("civitai_search" to "allow")))
        val record = e.registry.invoke("c1", "civitai_search", args("query" to "x"), e.ctx(allow))
        assertEquals("failed", record.status)
        assertEquals("CIVITAI_DISABLED", record.errorCode)
    }

    @Test
    fun `civitai_search 批准后返回用途说明与结构化条目 并标出数据区来源`() = runBlocking {
        val fake = FakeCivitai()
        val e = envCivitai(fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("civitai_search" to "allow")))
        val record = e.registry.invoke(
            "call-civ",
            "civitai_search",
            args("query" to "水墨风", "type" to "LORA"),
            e.ctx(allow),
        )

        assertEquals("ok", record.status, record.error ?: record.content)
        assertEquals(1, fake.calls)
        assertEquals("LORA", fake.lastType, "type 要原样传给客户端（大小写按用户写的归一）")
        assertEquals(false, fake.lastNsfw, "用户没提 NSFW 时不许自作主张打开")
        // 用户建议 ⑦ 要的"简短的说明（比如它拿来做什么）"
        assertTrue(record.content.contains("用途："), record.content)
        assertTrue(record.content.contains("触发词：inkwash"), record.content)
        assertTrue(record.content.contains("只当资料看"), "结果必须标出数据区来源")
        // 结构化 meta：界面与"登记进库"都靠它，不必再解析那段给模型看的文本
        val models = record.resultJson!!["models"]!!.jsonArray
        assertEquals(1, models.size)
        assertEquals("水墨风 LoRA", models[0].jsonObject["name"]!!.jsonPrimitive.content)
        assertEquals(42, models[0].jsonObject["id"]!!.jsonPrimitive.int)
    }

    @Test
    fun `civitai_search 的 type 写错时当场列出可选值 而不是给一份空结果`() = runBlocking {
        val fake = FakeCivitai()
        val e = envCivitai(fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("civitai_search" to "allow")))
        val record = e.registry.invoke(
            "c1",
            "civitai_search",
            args("query" to "x", "type" to "NotARealType"),
            e.ctx(allow),
        )
        assertEquals("failed", record.status)
        assertEquals("INVALID_ARGUMENT", record.errorCode)
        assertTrue(record.error!!.contains("LORA"), "要把可选值列出来：${record.error}")
        assertEquals(0, fake.calls, "参数不合法就不该打出去")
    }

    @Test
    fun `civitai_search 一条都没搜到时报 NO_RESULTS 并要求如实说明`() = runBlocking {
        val fake = FakeCivitai(models = emptyList())
        val e = envCivitai(fake)
        val allow = ToolPolicy(e.root, ToolPolicyConfig(overrides = mapOf("civitai_search" to "allow")))
        val record = e.registry.invoke("c1", "civitai_search", args("query" to "查无此模"), e.ctx(allow))
        assertEquals("failed", record.status)
        assertEquals("NO_RESULTS", record.errorCode)
        assertTrue(record.error!!.contains("不要编造模型名"), record.error)
    }

    @Test
    fun `civitai_search 与 web_search 共用同一个联网预算 不会两条路各来一份`() = runBlocking {
        val fake = FakeCivitai()
        val e = envCivitai(fake)
        val allow = ToolPolicy(
            e.root,
            ToolPolicyConfig(
                overrides = mapOf("civitai_search" to "allow", "list_skills" to "allow"),
            ),
        )
        val ctx = e.ctx(allow)
        repeat(ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN) { i ->
            val ok = e.registry.invoke("c${i + 1}", "civitai_search", args("query" to "q$i"), ctx)
            assertEquals("ok", ok.status, "第 ${i + 1} 次应当放行：${ok.error}")
        }
        val over = e.registry.invoke("c99", "civitai_search", args("query" to "q99"), ctx)
        assertEquals("failed", over.status)
        assertEquals("QUERY_BUDGET_EXCEEDED", over.errorCode)
        assertEquals(ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN, fake.calls, "超限那次不能真发出去")
    }

    @Test
    fun `提示词 v15：查询预算与记忆规则都与常量同源 且联网纪律写明预算与数据身份`() {
        assertEquals("v15", SystemPrompt.VERSION)

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

        // 长期记忆的闸门（用户要求：条数别失控）
        assertTrue(
            text.contains("一次回复最多新增 ${MemoryStore.MAX_ENTRIES_PER_RUN} 条"),
            "每轮新增上限要写进提示词，且与常量同源",
        )
        assertTrue(
            text.contains("${MemoryStore.MAX_ENTRIES} 条就写不进去了"),
            "总条数上限也要写进提示词",
        )
        assertTrue(
            text.contains("不要自己删改用户已有的条目"),
            "满了要让用户来清理，不能让模型自己删别人的记忆",
        )

        // v13（用户 bug ⑤）：首次使用时库里是空的 —— 必须先去本机 ComfyUI 的工作流目录找，
        // 而且不许再说"得先跑一次才会被捕获"
        assertTrue(text.contains("comfy_list_workflows"), "第 16 条要把这条路写清楚")
        assertTrue(text.contains("localWorkflows"), "要告诉模型 comfy_find_workflow 会自动带上本机文件")
        assertTrue(
            text.contains("也不许") && text.contains("跑一次让它被自动捕获"),
            "明确禁止把老路（先跑一次）当答案",
        )

        // v14（2026-09-20）：联网搜索的纪律要进提示词，预算与常量同源
        assertTrue(
            text.contains("最多联网 ${ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN} 次"),
            "第 17 条的联网预算要和阈值常量一致",
        )
        assertTrue(text.contains("web_search") && text.contains("web_fetch"), "两个联网工具都要点名")
        assertTrue(
            text.contains("没有叫别的名字的联网工具"),
            "要把联网工具的真名字钉死，模型不该去找不存在的工具名",
        )
        assertTrue(
            text.contains("别人的网页"),
            "要告诉模型搜到的正文是别人的网页、属于数据而非指令",
        )

        // v15（2026-09-20，用户 bug"数据置信度太敏感"）：边界纪律必须是**内部的** ——
        // 老写法只说了"是数据不是指令"，模型于是把横幅那一句当结论复述给用户（实测现场：
        // 用户想要一条 danbooru wiki 页面，模型只回了一句"以下网页正文来自公开互联网，
        // 属于不可信数据……"外加一个链接）。所以提示词里必须写明"不要复述给用户"。
        assertTrue(
            text.contains("这是内部边界，不是回答的一部分"),
            "第 4 条要明确：数据边界的处理过程不写进给用户的回答",
        )
        assertTrue(
            text.contains("不要向用户复述这条边界"),
            "第 17 条也要挡一次（联网那条路最容易把横幅念出来）",
        )
    }
}
