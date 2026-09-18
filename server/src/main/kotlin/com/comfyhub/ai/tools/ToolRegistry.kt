package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.ai.protocol.ToolSpec
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.util.UUID
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.resume

/**
 * 工具审批闸门（AIH-035 / DEC-005 / AIH-049）。
 *
 * `comfy_sync_history` 这类"会写库的读工具"默认要用户点一下：Harness 发 `tool.requested`
 * 事件（带 `approval: "pending"`）之后**在这里等**，前端弹出工具卡上的「批准 / 拒绝」按钮，
 * 走 `POST /api/ai/tool-calls/{callId}/approve|deny` 唤醒。
 *
 * 超时（默认 5 分钟）与 Run 取消都会**当作拒绝**：绝不允许"没人点就默认执行"。
 */
class ToolApprovalGate(private val timeoutMs: Long = 5 * 60 * 1000) {
    private val pending = ConcurrentHashMap<String, CompletableFuture<Boolean>>()
    private val log = LoggerFactory.getLogger(ToolApprovalGate::class.java)

    fun open(callId: String) {
        pending[callId] = CompletableFuture()
    }

    fun isPending(callId: String): Boolean = pending[callId]?.isDone == false

    /** 用户在界面上点了批准/拒绝。返回 false 表示这个 callId 已经不在等待（超时或已处理）。 */
    fun resolve(callId: String, approved: Boolean): Boolean {
        val future = pending[callId] ?: return false
        if (future.isDone) return false
        log.info("工具审批 {}：{}", callId, if (approved) "批准" else "拒绝")
        return future.complete(approved)
    }

    /** 等待用户决定。超时 / Run 取消 / 没人管 → false（拒绝）。 */
    suspend fun await(callId: String): Boolean {
        val future = pending[callId] ?: return false
        return withTimeoutOrNull(timeoutMs) {
            suspendCancellableCoroutine { cont ->
                future.whenComplete { value, _ -> if (cont.isActive) cont.resume(value == true) }
                cont.invokeOnCancellation { future.cancel(true) }
            }
        } ?: false
    }

    fun discard(callId: String) {
        pending.remove(callId)?.complete(false)
    }
}

/**
 * 工具注册表（M4）。
 *
 * 出厂工具集刻意**只有**三类：Skills、受限文件系统、ComfyUI 查询。
 * **没有 shell / 进程 / 网络抓取工具**（AIH-045：第三方 skill 的脚本首期一律不可执行）。
 *
 * 每个工具都能被用户按名字改成 `allow / ask / deny`（见 [ToolPolicy.accessFor]）；
 * `deny` 的工具**不会出现在下发给模型的 `tools` 里** —— 与其让模型看见再被骗着调用，
 * 不如根本不让它知道。
 */
class ToolRegistry(
    private val projectRoot: Path,
    private val skills: SkillStore,
    private val approvals: ToolApprovalGate,
    /** `ComfyCapture.status()` 的序列化结果 */
    private val comfyStatus: () -> JsonElement,
    /**
     * 按 runKey / 数字 prompt_id 查一条捕获记录，查不到返回 null。
     *
     * **两个都要认**：`comfy_submit` 的结果里同时给了 ComfyUI 的 UUID（`runKey`）与
     * 捕获记录里的数字 id，模型会拿哪个来查并不确定（实测它用了数字那个，
     * 而旧实现只认 UUID → "刚提交完却说没有这次运行"）。
     */
    private val comfyFindRun: (String) -> JsonElement?,
    /** `ComfyCapture.pollOnce()` 的结果 */
    private val comfySync: () -> JsonElement,
    /**
     * 在库里找能跑的工作流（用户建议 ①）。
     *
     * `query` 关键词、`limit` 条数、`includeGraph` 是否带上 API 节点图。
     * 查不到时返回 `count=0`（由工具报 NOT_FOUND）。
     */
    private val comfyFindWorkflow: (String, Int, Boolean) -> WorkflowSearch = { _, _, _ ->
        WorkflowSearch(0, buildJsonObject { put("count", 0) })
    },
    /** 提交一个工作流给 ComfyUI 跑（用户建议 ①）；`waitSeconds=0` 表示只提交不等。 */
    private val comfySubmit: suspend (Long, JsonObject?, String?, Int) -> ComfySubmitOutcome =
        { _, _, _, _ -> throw ToolFailure("COMFY_DISABLED", "本次运行没有启用 ComfyUI 提交能力") },
    /**
     * 把一份**本机工作流文件**读进库（用户 bug ③：`comfy_submit` 只认库里的 promptId，
     * "我不能凭一个文件路径提交"）。返回的结构里有 `promptId`（下一步就能提交）与节点摘要。
     */
    private val comfyLoadWorkflow: suspend (String, String?, Boolean) -> JsonObject =
        { _, _, _ -> throw ToolFailure("COMFY_DISABLED", "本次运行没有启用工作流文件加载能力") },
    /**
     * 把用户发来的**图片附件**放进 ComfyUI 的 `input/` 目录，返回它在那边真实可用的文件名。
     *
     * 图生图 / 参考图 / 结构参考的唯一正路：LoadImage 只能读 input 目录里真实存在的文件，
     * 而工作流被捕获时绑定的那个文件名跟我们库里的附件毫无关系（用户报的 bug）。
     * 返回 `{attachmentId, filename, inputDir, hint}`。
     */
    private val comfyUseAttachment: suspend (String, String?) -> JsonObject =
        { _, _ -> throw ToolFailure("COMFY_DISABLED", "本次运行没有启用图片投放能力") },
) {
    private val log = LoggerFactory.getLogger(ToolRegistry::class.java)

    companion object {
        /** 单条工具结果回给模型的字符上限（超出保留头尾并注明） */
        const val MAX_RESULT_CHARS = 8_000
        /** 工具卡上显示的摘要长度 */
        const val MAX_PREVIEW_CHARS = 400
    }

    private fun schema(json: String): JsonObject =
        AppJson.parseToJsonElement(json) as JsonObject

    val tools: List<AgentTool> = listOf(
        // ------------------------------------------------------------------
        //  Skills：注册 / 加载 / 列举 / 删除（用户要求：跟 AI 说一声就注册到本地）
        // ------------------------------------------------------------------
        AgentTool(
            name = "list_skills",
            description = "列出本机已安装的 Skills（名称 + 描述 + 是否可用）。做任务前先看这里，别凭记忆假设有某个 Skill。",
            parameters = ToolSpec.EMPTY_PARAMS,
            category = ToolCategory.SKILL,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { _, ctx ->
            val all = ctx.skills.scan()
            val json = buildJsonObject {
                put("count", all.size)
                put(
                    "skills",
                    buildJsonArray {
                        all.forEach { s ->
                            add(
                                buildJsonObject {
                                    put("name", s.name)
                                    put("description", s.description)
                                    put("whenToUse", s.whenToUse)
                                    put("source", s.source)
                                    put("available", s.validationError == null)
                                    put("validationError", s.validationError)
                                }
                            )
                        }
                    }
                )
            }
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), json), json)
        },

        AgentTool(
            name = "load_skill",
            description = "加载某个 Skill 的完整正文。任务与某个 Skill 匹配时**先加载它再动手**，" +
                "不要只根据目录摘要臆造规则。同一次回复里同一个 Skill 只会返回一次。",
            parameters = schema(
                """{"type":"object","properties":{"name":{"type":"string","description":"skill 名（kebab-case），见 list_skills"}},"required":["name"],"additionalProperties":false}"""
            ),
            category = ToolCategory.SKILL,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val name = args.str("name") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 name")
            if (!ctx.loadedSkills.add(name)) {
                throw ToolFailure("SKILL_ALREADY_LOADED", "skill「$name」在本次回复里已经加载过了，直接按它的规则继续")
            }
            val (dto, body) = ctx.skills.read(name)
                ?: throw ToolFailure("SKILL_NOT_FOUND", "没有名为「$name」的 skill；先用 list_skills 看有哪些")
            val text = buildString {
                append("<skill_content name=\"").append(dto.name).append("\"")
                dto.version?.let { append(" version=\"").append(it).append("\"") }
                append(">\n")
                append(body)
                append("\n</skill_content>")
            }
            ToolOutput(text, buildJsonObject { put("name", dto.name); put("digest", dto.digest) })
        },

        AgentTool(
            name = "register_skill",
            description = "把一个新的 Skill 注册到本机（写入项目自己的 skills 目录，立刻生效，不用重启应用）。" +
                "用户说\"记住这个流程 / 注册一个 skill\"时用它。name 用小写 kebab-case，正文写清步骤与判据。",
            parameters = schema(
                """{"type":"object","properties":{"name":{"type":"string","description":"kebab-case，例如 anima-prompt-helper"},"description":{"type":"string","description":"一句话说明这个 skill 干什么（必填，≤600 字）"},"whenToUse":{"type":"string","description":"什么情况下该用（可选）"},"content":{"type":"string","description":"Markdown 正文：完整规则与步骤"}},"required":["name","description","content"],"additionalProperties":false}"""
            ),
            category = ToolCategory.SKILL,
            mutating = true,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val name = args.str("name") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 name")
            val description = args.str("description") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 description")
            val content = args.str("content") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 content")
            val dto = ctx.skills.save(
                name = name,
                description = description,
                whenToUse = args.str("whenToUse"),
                content = content,
                origin = "ai",
            )
            val json = AppJson.encodeToJsonElement(SkillDto.serializer(), dto) as? JsonObject
            ToolOutput(
                "已注册 skill「${dto.name}」（${dto.sizeBytes} 字节，digest ${dto.digest}）。" +
                    "下一次回复即可用 load_skill 加载它；用户也能在右侧栏看到并删除。",
                json,
            )
        },

        AgentTool(
            name = "delete_skill",
            description = "删除一个**用户来源**的 Skill。内置 skill 不能删。这是不可逆操作，用户确认后再调用。",
            parameters = schema(
                """{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}"""
            ),
            category = ToolCategory.SKILL,
            mutating = true,
            defaultAccess = ToolAccess.ASK,
        ) { args, ctx ->
            val name = args.str("name") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 name")
            val deleted = ctx.skills.delete(name)
            if (!deleted) throw ToolFailure("SKILL_NOT_FOUND", "没有名为「$name」的 skill")
            ToolOutput("已删除 skill「$name」。", buildJsonObject { put("name", name); put("deleted", true) })
        },

        // ------------------------------------------------------------------
        //  长期记忆（M6）：跨对话记住用户的偏好与固定约定
        // ------------------------------------------------------------------
        AgentTool(
            name = "remember",
            description = "把一条**长期有效**的信息写进长期记忆（用户偏好、固定约定、称呼、常用参数等），" +
                "以后每次对话都会带上它。只记用户明确说过、且跨对话仍然成立的事；一次只写一条。" +
                "写进去的每条会自动带上 `[日期]` 前缀；**一次回复最多新增 ${MemoryStore.MAX_ENTRIES_PER_RUN} 条**，" +
                "总条数上限 ${MemoryStore.MAX_ENTRIES} 条（满了要请用户去界面里清理，不能自己删别人的条目）。" +
                "**不要**记录本次任务的临时状态、文件内容、以及任何密钥 / 口令 / 隐私凭据。",
            parameters = schema(
                """{"type":"object","properties":{"content":{"type":"string","description":"一条记忆，直接写事实，例如：用户偏好 4:3 画幅，出图统一用 Aesthetic 模型"}},"required":["content"],"additionalProperties":false}"""
            ),
            category = ToolCategory.MEMORY,
            mutating = true,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val content = args.str("content") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 content")
            val store = ctx.memory
                ?: throw ToolFailure("MEMORY_DISABLED", "本次运行没有启用长期记忆")
            // 写入节流（用户要求）：不限制的话模型很容易把整段对话都"记下来"，
            // 条数一多，用户想查找 / 改动 / 删除都会变难。
            // 判重命中的"又说了一遍"不算新增，所以它既不占额度、也不该被这条挡住。
            if (!store.hasEntry(content) && ctx.memoryWrites >= MemoryStore.MAX_ENTRIES_PER_RUN) {
                throw ToolFailure(
                    "MEMORY_RATE_LIMITED",
                    "本次回复已经新增了 ${ctx.memoryWrites} 条记忆（每轮上限 ${MemoryStore.MAX_ENTRIES_PER_RUN} 条）。" +
                        "挑最要紧的留下，其余的等用户明确让你记再写。",
                )
            }
            val before = store.read().entryCount
            val dto = store.append(content, dated = true)
            // 判重命中时条数没变 —— 那是"模型把同一件事说了两遍"，不该占额度
            if (dto.entryCount > before) ctx.memoryWrites++
            val json = buildJsonObject {
                put("entryCount", dto.entryCount)
                put("maxEntries", dto.maxEntries)
                put("path", dto.path)
            }
            ToolOutput(
                "已记住（长期记忆现有 ${dto.entryCount}/${dto.maxEntries} 条，本次回复已记 ${ctx.memoryWrites} 条）。" +
                    "用户可以在 AI 工作台右侧栏的「长期记忆」里搜索、修改或删除。",
                json,
            )
        },

        // ------------------------------------------------------------------
        //  文件系统：默认只能碰 ComfyUI 目录（用户要求：默认不能改 comfy 目录以外的内容）
        // ------------------------------------------------------------------
        AgentTool(
            name = "list_dir",
            description = "列目录（默认 ComfyUI 目录）。只允许读允许的目录，越界会被拒绝。",
            parameters = schema(
                """{"type":"object","properties":{"path":{"type":"string","description":"相对项目根或绝对路径；缺省 = ComfyUI 目录"},"depth":{"type":"integer","description":"1 或 2，默认 1"}},"additionalProperties":false}"""
            ),
            category = ToolCategory.FILES,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val raw = args.str("path") ?: ctx.policy.writeRoots.firstOrNull()?.toString().orEmpty()
            val dir = ctx.policy.resolveRead(raw)
            if (!Files.isDirectory(dir)) throw ToolFailure("NOT_A_DIRECTORY", "不是目录：$dir")
            val depth = (args.int("depth") ?: 1).coerceIn(1, 2)
            val maxEntries = 200
            val lines = mutableListOf<String>()
            var truncated = false
            Files.walk(dir, depth).use { stream ->
                val it = stream.iterator()
                while (it.hasNext()) {
                    val p = it.next()
                    if (p == dir) continue
                    if (lines.size >= maxEntries) {
                        truncated = true
                        break
                    }
                    val rel = runCatching { dir.relativize(p).toString() }.getOrDefault(p.toString())
                    val kind = if (Files.isDirectory(p)) "dir " else "file"
                    val size = if (Files.isDirectory(p)) "" else " ${runCatching { Files.size(p) }.getOrDefault(0L)}B"
                    lines += "$kind $rel$size"
                }
            }
            val text = buildString {
                append(dir).append('\n')
                append(lines.joinToString("\n").ifEmpty { "(空目录)" })
                if (truncated) append("\n…（超过 $maxEntries 条已截断）")
            }
            ToolOutput(text)
        },

        AgentTool(
            name = "read_file",
            description = "读一个文本文件（默认限制在允许读取的目录内，超过 256KB 会截断）。",
            parameters = schema(
                """{"type":"object","properties":{"path":{"type":"string"},"maxBytes":{"type":"integer","description":"可选，默认 262144"}},"required":["path"],"additionalProperties":false}"""
            ),
            category = ToolCategory.FILES,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val raw = args.str("path") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 path")
            val file = ctx.policy.resolveRead(raw)
            if (!Files.isRegularFile(file)) throw ToolFailure("NOT_A_FILE", "不是文件：$file")
            val limit = (args.int("maxBytes") ?: ctx.policy.config.maxReadBytes)
                .coerceIn(1, ctx.policy.config.maxReadBytes)
            val size = Files.size(file)
            val bytes = Files.newInputStream(file).use { it.readNBytes(limit) }
            val text = String(bytes, StandardCharsets.UTF_8)
            val note = if (size > bytes.size) "\n…（文件 $size 字节，已截断到 ${bytes.size} 字节）" else ""
            ToolOutput("$file\n$text$note")
        },

        AgentTool(
            name = "write_file",
            description = "写文件。**默认只允许写 ComfyUI 目录**（`comfyui/`）；写到别处会被权限策略直接拒绝，" +
                "不要试图绕开。写前先想清楚路径，必要时先 list_dir 看一眼。",
            parameters = schema(
                """{"type":"object","properties":{"path":{"type":"string","description":"目标路径（相对项目根或绝对路径）"},"content":{"type":"string","description":"完整内容"},"mode":{"type":"string","enum":["overwrite","append"],"description":"默认 overwrite"}},"required":["path","content"],"additionalProperties":false}"""
            ),
            category = ToolCategory.FILES,
            mutating = true,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val raw = args.str("path") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 path")
            val content = args.str("content") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 content")
            val bytes = content.toByteArray(StandardCharsets.UTF_8)
            if (bytes.size > ctx.policy.config.maxWriteBytes) {
                throw ToolFailure(
                    "TOO_LARGE",
                    "内容太大（${bytes.size} > ${ctx.policy.config.maxWriteBytes} 字节），拆成多次写"
                )
            }
            val target = ctx.policy.resolveWrite(raw)
            target.parent?.let { Files.createDirectories(it) }
            val append = args.str("mode") == "append"
            if (append) {
                Files.write(target, bytes, StandardOpenOption.CREATE, StandardOpenOption.APPEND)
            } else {
                Files.write(target, bytes, StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING)
            }
            log.info("工具写文件: {}（{} 字节，append={}）", target, bytes.size, append)
            ToolOutput(
                "已写入 $target（${bytes.size} 字节，${if (append) "追加" else "覆盖"}）。",
                buildJsonObject { put("path", target.toString()); put("bytes", bytes.size) },
            )
        },

        // ------------------------------------------------------------------
        //  ComfyUI 查询（AIH-033 / 034 / 035）
        // ------------------------------------------------------------------
        AgentTool(
            name = "comfy_get_status",
            description = "查询 ComfyUI 的连通性、队列（运行中 / 等待中）与最近的捕获记录。地址由应用配置决定，" +
                "不接受任意 URL。",
            parameters = schema(
                """{"type":"object","properties":{"includeRecent":{"type":"boolean","description":"默认 true"},"recentLimit":{"type":"integer","description":"默认 5，最多 20"}},"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, _ ->
            val limit = (args.int("recentLimit") ?: 5).coerceIn(0, 20)
            val includeRecent = args.bool("includeRecent") ?: true
            val status = comfyStatus()
            val text = AppJson.encodeToString(JsonElement.serializer(), status)
            val meta = when {
                status !is JsonObject -> null
                includeRecent -> status
                else -> JsonObject(status.filterKeys { it != "recent" })
            }
            val note = if (limit == 0 || !includeRecent) "\n（最近捕获记录已省略）" else ""
            ToolOutput(text + note, meta)
        },

        AgentTool(
            name = "comfy_get_run",
            description = "按 runKey 查一次已捕获运行的状态、错误与产物数。" +
                "runKey 可以用三样中的任何一个：ComfyUI 的 prompt_id（UUID，comfy_submit 结果里的 runKey）、" +
                "捕获记录里的数字 id（capturedPromptId），或 import:<sha256>。",
            parameters = schema(
                """{"type":"object","properties":{"runKey":{"type":"string","description":"ComfyUI prompt_id（UUID）/ 数字 prompt_id / import:<sha256>"}},"required":["runKey"],"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, _ ->
            val key = args.str("runKey") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 runKey")
            val found = comfyFindRun(key)
                ?: throw ToolFailure(
                    "NOT_FOUND",
                    "没有 runKey=$key 的捕获记录（可能还没被轮询到，或不属于本机）。" +
                        "注意 comfy_submit 的结果里 `runKey` 是 ComfyUI 的 UUID、" +
                        "`capturedPromptId` 是捕获记录的编号，两者都能查。",
                )
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), found), found as? JsonObject)
        },

        AgentTool(
            name = "comfy_sync_history",
            description = "立刻同步一次 ComfyUI /history（**会往本项目库里写数据**，属于轻度写操作）。" +
                "需要用户批准；用之前先说明你想同步什么。",
            parameters = schema(
                """{"type":"object","properties":{"reason":{"type":"string","description":"为什么要同步（给用户看的）"}},"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = true,
            defaultAccess = ToolAccess.ASK,
        ) { _, _ ->
            val result = comfySync()
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), result), result as? JsonObject)
        },

        // ------------------------------------------------------------------
        //  提交任务（用户建议 ①：让 AI 可以直接调用 Comfy 提交任务）
        // ------------------------------------------------------------------
        AgentTool(
            name = "comfy_find_workflow",
            description = "在库里的提示词中找一个**能直接跑的工作流**（按标题/正文关键词搜索）。" +
                "返回它的参数摘要（采样器 / steps / cfg / seed / 尺寸 / 提示词）与可覆盖的参数路径，" +
                "然后就能用 comfy_submit 按同一个工作流再跑一次（改提示词或参数）。",
            parameters = schema(
                """{"type":"object","properties":{"query":{"type":"string","description":"关键词，匹配标题与正/负面提示词"},"limit":{"type":"integer","description":"最多返回几条，默认 5，最多 20"},"includeGraph":{"type":"boolean","description":"是否带上完整 API 节点图（默认 false；要看节点编号与输入名时才带上）"}},"required":["query"],"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, _ ->
            val query = args.str("query") ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 query")
            val limit = (args.int("limit") ?: 5).coerceIn(1, 20)
            val includeGraph = args.bool("includeGraph") ?: false
            val found = comfyFindWorkflow(query, limit, includeGraph)
            if (found.count == 0) {
                throw ToolFailure(
                    "NOT_FOUND",
                    "库里没有匹配「$query」的提示词。可以先用更短的关键词再搜一次，" +
                        "或者让用户先在 ComfyUI 里手动跑一次（跑过之后就会自动被捕获）。",
                )
            }
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), found.json), found.json)
        },

        AgentTool(
            name = "comfy_load_workflow",
            description = "把**一份本机工作流文件**（ComfyUI 保存的 .json，或「导出（API）」得到的节点图）" +
                "读进库里，换成一个能提交的 promptId。用户甩给你一个工作流文件路径时用它 —— " +
                "别再回答\"我只能提交库里的 promptId\"。" +
                "界面格式（nodes/links）会按 ComfyUI 的 /object_info 转成 API 节点图；" +
                "用了纯前端节点（Anything Everywhere 之类）转不出来的会如实报错，那时让用户在 ComfyUI 里" +
                "「导出（API）」一次，或点一次 Queue 让它被捕获。" +
                "返回里有 promptId 与每个节点的 id / 可覆盖的输入名（用于 comfy_submit 的 overrides）。" +
                "只读用户机器上的文件，不改它。",
            parameters = schema(
                """{"type":"object","properties":{"path":{"type":"string","description":"工作流文件的完整路径（绝对路径最稳）"},"title":{"type":"string","description":"库里显示的名字（可选，默认取文件名）"},"includeGraph":{"type":"boolean","description":"是否连完整 API 节点图一起返回（默认 false，只有要手写参数路径时才需要）"}},"required":["path"],"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = false,
            defaultAccess = ToolAccess.ALLOW,
        ) { args, ctx ->
            val raw = args.str("path")
                ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 path（工作流文件的完整路径）")
            // 路径判定走读白名单：本机 ComfyUI 的目录已经被自动放行（见 ComfyRoots），
            // 用户工作流一般就躺在它下面的 user/default/workflows 里
            val file = ctx.policy.resolveRead(raw)
            val json = try {
                comfyLoadWorkflow(file.toString(), args.str("title"), args.bool("includeGraph") ?: false)
            } catch (e: ToolFailure) {
                throw e
            } catch (e: Exception) {
                throw ToolFailure("COMFY_LOAD_FAILED", "加载工作流失败：${e.message?.take(400)}")
            }
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), json), json)
        },

        AgentTool(
            name = "comfy_submit",
            description = "**把一个工作流提交给 ComfyUI 真的跑一次**（会消耗显卡时间，默认需要用户批准）。" +
                "用 promptId 指定库里的工作流（先 comfy_find_workflow 或 comfy_load_workflow），" +
                "或者直接用 workflowPath 指一份本机工作流文件（等价于先 comfy_load_workflow 再提交）。" +
                "用 overrides 覆盖参数（键写成 节点id.输入名，例如 \"3.steps\"、\"6.text\"）。" +
                "跑完后产物会自动入库到画廊。不要凭想象编造工作流；说不清要跑什么就先问用户。",
            parameters = schema(
                """{"type":"object","properties":{"promptId":{"type":"integer","description":"库里提示词的 id（来自 comfy_find_workflow / comfy_load_workflow）"},"workflowPath":{"type":"string","description":"或者：本机工作流文件的完整路径（.json）"},"overrides":{"type":"object","description":"要覆盖的参数：{\"6.text\":\"新提示词\",\"3.steps\":30}","additionalProperties":true},"title":{"type":"string","description":"这次运行的标题（给用户认，可选）"},"wait":{"type":"boolean","description":"是否等它跑完（默认 true；false 表示排上队就返回）"},"waitSeconds":{"type":"integer","description":"最长等多少秒，默认 240，最多 900"}},"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = true,
            defaultAccess = ToolAccess.ASK,
        ) { args, ctx ->
            val overrides = args.obj("overrides")
            val wait = args.bool("wait") ?: true
            val waitSeconds = (args.int("waitSeconds") ?: 240).coerceIn(0, 900)
            var promptId = args.int("promptId")?.toLong()
            val path = args.str("workflowPath")
            if (promptId == null && path == null) {
                throw ToolFailure(
                    "INVALID_ARGUMENT",
                    "要么给 promptId（先 comfy_find_workflow 找），要么给 workflowPath（本机工作流文件路径）",
                )
            }
            // 给了文件路径就先加载：界面格式会在这里被转成 API 节点图
            val loaded = if (path != null) {
                val file = ctx.policy.resolveRead(path)
                try {
                    comfyLoadWorkflow(file.toString(), args.str("title"), false)
                } catch (e: ToolFailure) {
                    throw e
                } catch (e: Exception) {
                    throw ToolFailure("COMFY_LOAD_FAILED", "加载工作流失败：${e.message?.take(400)}")
                }
            } else {
                null
            }
            if (promptId == null) {
                promptId = (loaded?.get("promptId") as? JsonPrimitive)?.content?.toLongOrNull()
                    ?: throw ToolFailure("COMFY_LOAD_FAILED", "加载了工作流文件，但没拿到 promptId")
            }
            val result = try {
                comfySubmit(
                    promptId,
                    overrides,
                    args.str("title"),
                    if (wait) waitSeconds else 0,
                )
            } catch (e: ToolFailure) {
                throw e
            } catch (e: Exception) {
                // 连不上 / 被 ComfyUI 拒绝：如实报出来，并告诉模型它还能做什么
                throw ToolFailure(
                    "COMFY_SUBMIT_FAILED",
                    "提交失败：${e.message?.take(400)}。" +
                        "可以先 comfy_get_status 看 ComfyUI 是否在跑；不要反复重试同一个工作流。",
                )
            }
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), result.json), result.json)
        },

        // ------------------------------------------------------------------
        //  图生图：把用户发来的图片放进 ComfyUI 的 input 目录（用户 bug）
        // ------------------------------------------------------------------
        AgentTool(
            name = "comfy_use_attachment",
            description = "把**用户随消息发来的图片附件**放进 ComfyUI 的 input 目录，返回它在那边的真实文件名。" +
                "做图生图 / 参考图 / 结构参考 / 换脸时必须先用它：工作流里 LoadImage 的 image 输入" +
                "**只能填 ComfyUI input 目录里真实存在的文件名**，你不能改也编不出一个。" +
                "拿到 filename 之后再用 comfy_submit 覆盖 LoadImage 节点的输入，" +
                "例如 overrides={\"89.image\":\"<返回的 filename>\"}。" +
                "只接受图片附件；附件 id 见用户消息末尾的附件说明。",
            parameters = schema(
                """{"type":"object","properties":{"attachmentId":{"type":"string","description":"用户消息里那张图片的附件 id（形如 att_xxx / 32 位十六进制）"},"filename":{"type":"string","description":"可选：希望它在 ComfyUI 里叫什么（不填就用原文件名；会自动加一段 id 后缀防重名）"}},"required":["attachmentId"],"additionalProperties":false}"""
            ),
            category = ToolCategory.COMFY,
            mutating = true,
            // 要往用户机器上写文件（ComfyUI input 目录）→ 默认要批准；
            // 切到「自动允许（无需批准）」后就不必等了（用户建议 ⑤）
            defaultAccess = ToolAccess.ASK,
        ) { args, _ ->
            val attachmentId = args.str("attachmentId")
                ?: throw ToolFailure("INVALID_ARGUMENT", "缺少参数 attachmentId（见用户消息里的附件说明）")
            val result = comfyUseAttachment(attachmentId, args.str("filename"))
            ToolOutput(AppJson.encodeToString(JsonElement.serializer(), result), result)
        },
    )

    private val byName = tools.associateBy { it.name }

    fun find(name: String): AgentTool? = byName[name]

    /** 下发给模型的工具定义：`deny` 的直接不出现。 */
    fun specs(policy: ToolPolicy): List<ToolSpec> = tools
        .filter { policy.accessFor(it) != ToolAccess.DENY }
        .map { ToolSpec(it.name, it.description, it.parameters) }

    fun info(policy: ToolPolicy): List<ToolInfoDto> = tools.map {
        ToolInfoDto(
            name = it.name,
            description = it.description,
            category = it.category.wire,
            mutating = it.mutating,
            access = policy.accessFor(it).wire,
            overridden = policy.isOverridden(it),
        )
    }

    /**
     * 执行一次工具调用。
     *
     * 顺序：预算 → 策略（allow/ask/deny）→（必要时）等审批 → 执行 → 截断。
     * **任何异常都变成本次调用的失败结果**（`tool.failed`），不打断整个 Run：
     * 模型看到错误可以自己纠正或如实告诉用户（AIH-036）。
     *
     * 需要审批时，调用方必须**先** [ToolApprovalGate.open] 再发 `tool.requested` 事件 ——
     * 否则用户手快先点了按钮会落空，只能等超时。
     *
     * [onApproved] 在"用户点了批准、即将执行"这一刻回调：界面靠它把工具卡从"待批准"改成
     * "运行中"，而不是等工具跑完才补发 `tool.started`（同步工具可能跑好几秒）。
     */
    suspend fun invoke(
        callId: String,
        name: String,
        argumentsJson: String,
        ctx: ToolContext,
        onApproved: suspend () -> Unit = {},
    ): ToolCallRecord {
        val started = System.currentTimeMillis()
        val tool = find(name) ?: return failure(
            ctx.runId, callId, name, argumentsJson, "denied", started,
            "TOOL_NOT_FOUND", "没有名为「$name」的工具；可用工具见系统提示里的清单",
        )

        if (ctx.calls >= ctx.policy.config.maxCallsPerRun) {
            return failure(
                ctx.runId, callId, name, argumentsJson, "denied", started,
                "TOOL_BUDGET_EXCEEDED",
                "本次回复的工具调用次数已达上限（${ctx.policy.config.maxCallsPerRun}）；请基于已有信息作答",
            )
        }
        ctx.calls++

        // AIH-036：不许把 ComfyUI 当轮询器 —— 一次回复的查询次数有上限
        // （数字的唯一真源是 ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN）
        if (tool.category == ToolCategory.COMFY &&
            ctx.comfyQueries >= ctx.policy.config.maxComfyQueriesPerRun
        ) {
            return failure(
                ctx.runId, callId, name, argumentsJson, "not_required", started,
                "QUERY_BUDGET_EXCEEDED",
                "本次回复查询 ComfyUI 的次数已达上限（${ctx.policy.config.maxComfyQueriesPerRun} 次）；" +
                    "请基于已有信息作答，或让用户稍后再问",
            )
        }
        if (tool.category == ToolCategory.COMFY) ctx.comfyQueries++

        val access = ctx.policy.accessFor(tool)
        if (access == ToolAccess.DENY) {
            return failure(
                ctx.runId, callId, name, argumentsJson, "denied", started,
                "TOOL_DENIED", "工具「$name」被用户策略禁止使用（设置 → AI 工具权限）",
            )
        }

        val args = runCatching {
            val el = AppJson.parseToJsonElement(argumentsJson.ifBlank { "{}" })
            el as? JsonObject ?: JsonObject(emptyMap())
        }.getOrElse { JsonObject(emptyMap()) }

        var approval = "not_required"
        if (access == ToolAccess.ASK) {
            approval = "pending"
            val approved = approvals.await(callId)
            approvals.discard(callId)
            if (!approved) {
                return failure(
                    ctx.runId, callId, name, argumentsJson, "denied", started,
                    "APPROVAL_DENIED", "用户没有批准这次工具调用（或等待超时）；不要重试同一件事，如实说明即可",
                ).copy(approval = "denied")
            }
            approval = "approved"
            onApproved()
        }

        return try {
            val output = tool.handler(args, ctx)
            val elapsed = System.currentTimeMillis() - started
            val (content, truncated) = truncate(output.content)
            val meta = output.meta?.let {
                runCatching { it }.getOrNull()
            }
            ToolCallRecord(
                id = UUID.randomUUID().toString(),
                runId = ctx.runId,
                callId = callId,
                name = name,
                argumentsJson = argumentsJson,
                approval = approval,
                status = "ok",
                resultJson = meta ?: buildJsonObject { if (truncated) put("truncated", true) },
                content = content,
                preview = preview(content),
                elapsedMs = elapsed,
            )
        } catch (e: ToolFailure) {
            failure(ctx.runId, callId, name, argumentsJson, approval, started, e.code, e.message ?: "工具执行失败")
        } catch (e: Exception) {
            log.warn("工具 {} 执行异常: {}", name, e.message)
            failure(
                ctx.runId, callId, name, argumentsJson, approval, started,
                "TOOL_ERROR", "工具执行失败：${e::class.simpleName}（${e.message?.take(200)}）",
            )
        }
    }

    private fun failure(
        runId: String,
        callId: String,
        name: String,
        argumentsJson: String,
        approval: String,
        started: Long,
        code: String,
        message: String,
    ) = ToolCallRecord(
        id = UUID.randomUUID().toString(),
        runId = runId,
        callId = callId,
        name = name,
        argumentsJson = argumentsJson,
        approval = approval,
        status = "failed",
        content = "[工具失败:$code] $message",
        preview = "[$code] $message".take(MAX_PREVIEW_CHARS),
        error = message,
        errorCode = code,
        elapsedMs = System.currentTimeMillis() - started,
    )

    private fun truncate(text: String): Pair<String, Boolean> =
        if (text.length <= MAX_RESULT_CHARS) text to false
        else (text.take(MAX_RESULT_CHARS) + "\n…（结果过长已截断，共 ${text.length} 字符）") to true

    private fun preview(text: String): String {
        val flat = text.replace(Regex("\\s+"), " ").trim()
        return if (flat.length <= MAX_PREVIEW_CHARS) flat else flat.take(MAX_PREVIEW_CHARS) + "…"
    }
}

// ---------------------------------------------------------------------------
//  参数取值小工具（模型给的 JSON 不保证类型正确，一律容错）
// ---------------------------------------------------------------------------

internal fun JsonObject.str(field: String): String? =
    (this[field] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

internal fun JsonObject.int(field: String): Int? =
    (this[field] as? JsonPrimitive)?.let { runCatching { it.intOrNull ?: it.content.toInt() }.getOrNull() }

internal fun JsonObject.bool(field: String): Boolean? =
    (this[field] as? JsonPrimitive)?.let { runCatching { it.content.toBooleanStrictOrNull() }.getOrNull() }

/** 对象参数（例如 `overrides`）；不是对象就当作没给（模型偶尔会给字符串）。 */
internal fun JsonObject.obj(field: String): JsonObject? = this[field] as? JsonObject
