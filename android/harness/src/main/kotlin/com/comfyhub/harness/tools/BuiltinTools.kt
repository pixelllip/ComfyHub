package com.comfyhub.harness.tools

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 手机侧内置工具（移植自桌面 `ai/tools/` 的那几个**手机能履行**的）。
 *
 * ## 这一版实现了哪些（诚实清单 —— 与 ToolScope 契约 §2 的 MOBILE 七个**已经对齐**）
 *
 * | 工具 | 状态 | 说明 |
 * | --- | --- | --- |
 * | `remember` | ✅ | 长期记忆是本地文件，两端各一份；上限与桌面同值，**超限报错不静默截断** |
 * | `list_skills` / `load_skill` | ✅ | skill 正文真源在磁盘（本地投放口），**刻意不缓存** |
 * | `ask_user_question` | ✅ | 走本端事件流；**先 open 再发事件**（R8） |
 * | `comfy_queue_submit` | ✅ | 只写本地 outbox，**绝不返回 mediaIds**（R11） |
 * | `web_fetch` | ✅ | 纯出网 + 正文抽取；结果**当不可信数据**加横幅（R7） |
 * | `web_search` | ✅ | Keenable 通用搜索（免密钥可跑）；结果**当不可信数据**加横幅（R7） |
 * | `civitai_search` | ✅ | Civitai 模型库检索（结构化字段）；`allowNsfw` 切 `.red` 站 |
 *
 * ⚠ 这条清单是**实现的现状**，不是规则的真源：下发给模型的清单由
 * [MobileToolRegistry.declaredTo] 现算（= kit ∩ 本地实现），而"手机能做哪些"的真源是桌面
 * （ToolScope 契约 §2、`server-rs/src/mobile_tools.rs`）—— 手机侧**不维护第二份规则**（R12/R13）。
 *
 * 最后补齐的两件（`web_search` / `civitai_search`）在 [WebTools.kt] 里，
 * 与 `web_fetch` **共用同一条 HTTP 客户端与同一份联网预算**（[WebQueryBudget]）。
 */

/** 工具结果正文的上限（与桌面 `ToolRegistry.MAX_RESULT_CHARS` 同值）。 */
object ToolResults {
    const val MAX_RESULT_CHARS = 32_000

    /** 超长一律截断，并**如实说明截断了**（不许假装完整）。 */
    fun truncate(text: String): String =
        if (text.length <= MAX_RESULT_CHARS) text
        else text.take(MAX_RESULT_CHARS) + "\n…（结果过长已截断，共 ${text.length} 字符）"

    /**
     * 不可信数据的出处标记（红线 **R7**）。
     *
     * **文案全部来自 run-kit**：模板与查询行由桌面下发（`untrustedBanner`），
     * 手机只做 `{kind}` / `{query}` 的替换 —— 任何一边都不许再抄一份。
     */
    fun untrustedBanner(template: String, kind: String, query: String?): String {
        val filled = template.replace("{kind}", kind)
        return if (query.isNullOrBlank()) filled.replace("\n查询：{query}", "") else filled.replace("{query}", query)
    }
}

// ---------------------------------------------------------------------------
//  长期记忆（memory.md）
// ---------------------------------------------------------------------------

/**
 * 长期记忆文件 —— 移植自桌面 `MemoryStore` 的**写入语义**（上限、判重、日期前缀）。
 *
 * 为什么连上限数值都照搬：`memory.md` 会随同步通道在两端之间走（契约 §7.1），
 * 两端对"多少算满"的理解不一致时，会出现"手机能记、桌面拒收"这种没法解释的现象。
 */
class MemoryFile(private val file: File) {

    companion object {
        const val MAX_CHARS = 8000
        const val MAX_ENTRY_CHARS = 500
        const val MAX_ENTRIES = 100
        const val MAX_ENTRIES_PER_RUN = 4

        private val DATE_PREFIX = Regex("""^\[\d{4}-\d{2}-\d{2}]\s*""")
        private val BULLET = Regex("""^[-*]\s*""")

        fun countEntries(content: String): Int = content.lines().count { it.trim().isNotEmpty() }

        /** 一行的**正文**：去掉 `- ` 与 `[日期] ` 前缀（判重就靠它）。 */
        fun bodyOf(line: String): String =
            line.trim().replaceFirst(BULLET, "").replaceFirst(DATE_PREFIX, "").trim()

        /** 日期前缀用的今天（本地时区，与桌面 `LocalDate.now()` 同语义）。 */
        fun today(): String = SimpleDateFormat("yyyy-MM-dd", Locale.US).format(Date())
    }

    /** 读全文（去掉尾部空白：落盘时统一带换行，读回来去掉，读写才是幂等往返）。 */
    fun read(): String = runCatching {
        if (file.isFile) file.readText(Charsets.UTF_8) else ""
    }.getOrDefault("").trim()

    /** 一行一条（去掉空行）—— 与桌面界面里编号删除的顺序一致。 */
    fun entries(): List<String> = read().lines().map { it.trim() }.filter { it.isNotEmpty() }

    fun hasEntry(entry: String): Boolean {
        val body = bodyOf(entry)
        if (body.isEmpty()) return false
        return read().lines().any { bodyOf(it).equals(body, ignoreCase = true) }
    }

    private fun persist(text: String) {
        file.parentFile?.mkdirs()
        file.writeText(text + "\n", Charsets.UTF_8)
    }

    /**
     * 追加一条（AI 的 `remember` 走这里，所以带日期前缀）。
     *
     * 三种失败都**明确报错**、绝不静默处理：
     *  - 空内容；
     *  - 单条超长；
     *  - 条数到顶 / 总量到顶（让用户去清理，而不是悄悄挤掉最旧的）。
     */
    fun append(entry: String, dated: Boolean = true, today: String = today()): Result<String> {
        val line = entry.replace("\r\n", "\n").replace('\n', ' ').trim()
        val body = line.replaceFirst(BULLET, "").trim()
        if (body.isEmpty()) return Result.failure(MemoryFailure("INVALID_ARGUMENT", "记的内容不能为空"))
        val stamped = if (dated) "[$today] $body" else body
        if (stamped.length > MAX_ENTRY_CHARS) {
            return Result.failure(
                MemoryFailure("INVALID_ARGUMENT", "一条记忆太长（${stamped.length} > $MAX_ENTRY_CHARS 字符），拆成几条")
            )
        }
        val current = read()
        if (hasEntry(body)) return Result.success(current) // 又说了一遍：不重复添加，也不占额度

        val count = countEntries(current)
        if (count >= MAX_ENTRIES) {
            return Result.failure(
                MemoryFailure(
                    "MEMORY_FULL",
                    "长期记忆已经有 $count 条（上限 $MAX_ENTRIES 条）。请用户删掉一些再记 —— " +
                        "不要丢内容，也不要自己改写已有的条目。",
                )
            )
        }
        val bullet = "- $stamped"
        val next = if (current.isEmpty()) bullet else "$current\n$bullet"
        if (next.length > MAX_CHARS) {
            return Result.failure(
                MemoryFailure("MEMORY_TOO_LARGE", "长期记忆已满（$MAX_CHARS 字符上限）；告诉用户去清理，不要丢内容")
            )
        }
        persist(next)
        return Result.success(next)
    }

    class MemoryFailure(val code: String, message: String) : RuntimeException(message)
}

/** `remember`：记一条长期记忆。 */
class RememberTool(private val memory: MemoryFile? = null) : MobileTool {
    override val name = "remember"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        val store = memory ?: MemoryFile(ctx.files.memoryFile)
        val text = args.str("text") ?: args.str("content") ?: args.str("entry")
            ?: return ToolOutcome.failure("INVALID_ARGUMENT", "remember 需要 text 参数")
        // 判重命中不算新增（与桌面同语义：不占每轮额度）
        val isNew = !store.hasEntry(text)
        if (isNew && ctx.memoryWrites >= ctx.memoryWritesLimit) {
            return ToolOutcome.failure(
                "RATE_LIMITED",
                "本次回复已经记了 ${ctx.memoryWrites} 条（上限 ${ctx.memoryWritesLimit} 条）；" +
                    "剩下的请合并成一条，或下一轮再记。",
            )
        }
        return store.append(text).fold(
            onSuccess = { content ->
                if (isNew) ctx.memoryWrites++
                ToolOutcome.success("已记住。当前长期记忆：\n$content")
            },
            onFailure = { e ->
                val code = (e as? MemoryFile.MemoryFailure)?.code ?: "TOOL_INTERNAL_ERROR"
                ToolOutcome.failure(code, e.message.orEmpty())
            },
        )
    }
}

// ---------------------------------------------------------------------------
//  Skills（只读）
// ---------------------------------------------------------------------------

/** 一个 skill 的目录项（手机侧只读：注册 / 删除要写桌面磁盘，属于桌面专属）。 */
data class SkillSummary(
    val name: String,
    val description: String,
    val file: File,
    /** frontmatter 非法时**照样列出来但不可加载**（与桌面同纪律：不静默忽略）。 */
    val validationError: String? = null,
)

/**
 * skill 目录（本地投放口）。
 *
 * 与桌面同一条纪律：**磁盘即真源、刻意不做缓存** ——
 * "AI 注册完下一次回复就能用、用户删掉立刻消失"是需求。
 */
class SkillCatalog(private val dir: File) {

    /**
     * 扫两种布局（与桌面 `skills.rs` 的查找顺序一致）：
     *
     * ```
     * <dir>/<name>/SKILL.md   —— 随包内置 / 从桌面整个拷过来的那种（可以带 references/）
     * <dir>/<name>.md         —— 用户手写的平铺文件
     * ```
     *
     * **同名时目录版胜出**：桌面那条规则是"bundle > 平铺"，两边不一致就会出现
     * "桌面上看到的正文与手机上加载的不是同一份"。
     */
    fun scan(): List<SkillSummary> {
        if (!dir.isDirectory) return emptyList()
        val bundles = dir.listFiles { f -> f.isDirectory }
            .orEmpty()
            .sortedBy { it.name }
            .mapNotNull { d ->
                // 目录里没有 SKILL.md 的（比如刚建的空目录）不算 skill
                File(d, "SKILL.md").takeIf { it.isFile }?.let { parse(it, fallbackName = d.name) }
            }
        val flat = dir.listFiles { f -> f.isFile && f.name.endsWith(".md") }
            .orEmpty()
            .sortedBy { it.name }
            .map { parse(it, fallbackName = it.nameWithoutExtension) }
        val taken = bundles.mapTo(mutableSetOf()) { it.name.lowercase() }
        return (bundles + flat.filter { taken.add(it.name.lowercase()) }).sortedBy { it.name }
    }

    fun find(name: String): SkillSummary? =
        scan().firstOrNull { it.name.equals(name, ignoreCase = true) }

    fun load(name: String): Result<Pair<SkillSummary, String>> {
        val summary = find(name) ?: return Result.failure(
            NoSuchElementException("SKILL_NOT_FOUND")
        )
        summary.validationError?.let { return Result.failure(IllegalStateException(it)) }
        return Result.success(summary to summary.file.readText(Charsets.UTF_8))
    }

    companion object {
        /**
         * 读 frontmatter（`name` / `description`）。
         *
         * 两个与真实文件有关的细节：
         *  - `description: |` 这种 **YAML 块标量**必须能读（真实 `SKILL.md` 大量这么写），
         *    进提示前压成一行；
         *  - 没有 frontmatter 的文件：用 `fallbackName`（平铺文件取文件名，
         *    目录版取**目录名** —— `SKILL.md` 本身当名字会是 "SKILL"）当 name、
         *    正文首行当 description（后端启动时会给它补齐 frontmatter，手机侧只读，
         *    所以这里只做兜底展示）。
         */
        fun parse(file: File, fallbackName: String = file.nameWithoutExtension): SkillSummary {
            val text = runCatching { file.readText(Charsets.UTF_8) }.getOrDefault("")
            if (!text.startsWith("---")) {
                val firstLine = text.lines().firstOrNull { it.trim().isNotEmpty() }?.trim().orEmpty()
                return SkillSummary(fallbackName, firstLine.take(120), file)
            }
            val end = text.indexOf("\n---", startIndex = 3)
            if (end < 0) {
                return SkillSummary(fallbackName, "", file, validationError = "frontmatter 没有结束标记（---）")
            }
            val header = text.substring(3, end).trim()
            var name: String? = null
            var description: String? = null
            val lines = header.lines()
            var i = 0
            while (i < lines.size) {
                val line = lines[i]
                val key = line.substringBefore(':', "").trim()
                val rawValue = line.substringAfter(':', "").trim()
                when {
                    rawValue == "|" || rawValue == ">" -> {
                        // 块标量：把后续缩进行收进来，压成一行
                        val block = mutableListOf<String>()
                        var j = i + 1
                        while (j < lines.size && (lines[j].startsWith(" ") || lines[j].startsWith("\t"))) {
                            block += lines[j].trim()
                            j++
                        }
                        val value = block.joinToString(" ").trim()
                        if (key == "description") description = value
                        if (key == "name") name = value
                        i = j
                        continue
                    }
                    key == "name" -> name = rawValue.trim('"', '\'')
                    key == "description" -> description = rawValue.trim('"', '\'')
                }
                i++
            }
            val finalName = name?.takeIf { it.isNotBlank() } ?: fallbackName
            val finalDesc = description.orEmpty().replace(Regex("\\s+"), " ").trim()
            val error = when {
                name.isNullOrBlank() -> "frontmatter 缺少 name"
                description == null -> "frontmatter 缺少 description"
                else -> null
            }
            return SkillSummary(finalName, finalDesc, file, error)
        }
    }
}

/** `list_skills`：列出本地 skill（**只列，不写**）。 */
class ListSkillsTool : MobileTool {
    override val name = "list_skills"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        val skills = SkillCatalog(ctx.files.skillsDir).scan()
        if (skills.isEmpty()) {
            return ToolOutcome.success("本机还没有 skill。skill 目录：${ctx.files.skillsDir.absolutePath}")
        }
        val text = buildString {
            append("本机 skill（${skills.size} 个）：\n")
            skills.forEach { s ->
                append("- ${s.name}：${s.description.ifBlank { "（没有 description）" }}")
                s.validationError?.let { append("  ⚠ ${it}（不可加载）") }
                append('\n')
            }
        }
        return ToolOutcome.success(ToolResults.truncate(text))
    }
}

/** `load_skill`：读正文（正文真源在磁盘，**不缓存**）。 */
class LoadSkillTool : MobileTool {
    override val name = "load_skill"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        val wanted = args.str("name") ?: args.str("skill")
            ?: return ToolOutcome.failure("INVALID_ARGUMENT", "load_skill 需要 name 参数")
        val catalog = SkillCatalog(ctx.files.skillsDir)
        val summary = catalog.find(wanted)
            ?: return ToolOutcome.failure("SKILL_NOT_FOUND", "没有名为 $wanted 的 skill（可用 list_skills 看看）")
        summary.validationError?.let {
            return ToolOutcome.failure("SKILL_INVALID", "skill ${summary.name} 的 frontmatter 非法：$it")
        }
        val body = summary.file.readText(Charsets.UTF_8)
        return ToolOutcome.success(ToolResults.truncate(body))
    }
}

// ---------------------------------------------------------------------------
//  ask_user_question（提问闸门）
// ---------------------------------------------------------------------------

/**
 * `ask_user_question`：向用户提问并**等回答**（Run 不结束）。
 *
 * ⚠ 顺序硬约束（红线 **R8**）：**先 `askGate.open(id)` 再发 `user.question` 事件**。
 * 反过来的话用户手快先答、`answer()` 找不到 pending → 这次 Run 只能等超时。
 */
class AskUserQuestionTool(private val newId: () -> String = { com.comfyhub.harness.store.Timestamps.newId() }) :
    MobileTool {
    override val name = "ask_user_question"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        val question = args.str("question")
            ?: return ToolOutcome.failure("INVALID_ARGUMENT", "ask_user_question 需要 question 参数")
        val options = args.strList("options")
        val header = args.str("header")
        val multiSelect = args.bool("multi_select") ?: false
        val allowFreeText = args.bool("allow_free_text") ?: true
        val questionId = newId()

        // ① 先登记（R8）：用户手快先点按钮也不会落空
        ctx.askGate.open(questionId)
        // ② 再发事件：界面据此弹提问卡
        ctx.emit(
            com.comfyhub.harness.event.RunEventType.USER_QUESTION,
            buildJsonObject {
                put("runId", ctx.runId)
                put("conversationId", ctx.conversationId)
                put("questionId", questionId)
                put("question", question)
                header?.let { put("header", it) }
                put("allowFreeText", allowFreeText)
                put("multiSelect", multiSelect)
                put("options", buildJsonArray { options.forEach { add(JsonPrimitive(it)) } })
            },
        )
        // ③ 等回答：超时 / 取消都算"没答"，**不许假装答了**
        val answer = ctx.askGate.await(questionId)
        ctx.emit(
            com.comfyhub.harness.event.RunEventType.USER_QUESTION_ANSWERED,
            buildJsonObject {
                put("questionId", questionId)
                put("answer", answer?.text.orEmpty())
            },
        )
        if (answer == null || answer.text.isBlank()) {
            return ToolOutcome.failure("ASK_UNANSWERED", "用户没有回答这个问题（超时或取消了这次 Run）")
        }
        val chosen = answer.optionIndex?.let { idx -> options.getOrNull(idx) }
        val text = if (chosen != null) "${answer.text}（选项 ${answer.optionIndex!! + 1}：$chosen）" else answer.text
        return ToolOutcome.success(text)
    }
}

// ---------------------------------------------------------------------------
//  comfy_queue_submit（排队提交，R11）
// ---------------------------------------------------------------------------

/**
 * `comfy_queue_submit`：把**提交意图**写进本地 outbox，桌面在线后自动执行。
 *
 * ## 红线 R11：名字与返回值都必须与桌面的 `comfy_submit` 不同
 *
 * `comfy_submit` 的契约是"提交 → 等生成 → 返回 `mediaIds`"。桌面离线时手机**做不到**，
 * 而模型会以为自己提交成功了并如实汇报 —— 用户以为图在跑，其实没有。**这是最坏的一种失败。**
 * 所以：
 *  - 工具叫 `comfy_queue_submit`（模型没法把两者混起来）；
 *  - **绝不返回 `mediaIds`**、绝不说"正在生成"；
 *  - 返回值里明确写"排队 ≠ 已提交"，并说明结果会怎么回来。
 *
 * ## ⚠ 入队前必须**摊平**（契约 §4.1，M5）
 *
 * 桌面只认**扁平的顶层键**（`promptId` / `sourceRef` / `overrides` / `nsfw` / `requestedAt`）；
 * 整包塞在一个 `args` 子对象里推上去，桌面按**永久性** `MISSING_WORKFLOW` 拒，
 * 手机再把永久失败那行丢掉 —— 用户看到的是"点了提交，**什么都没发生**"。
 *
 * 所以这里**摊不平就不排队**，并且当场把"该给什么"回给模型（[SubmitJobPayload.flatten]）：
 *  - 工作流只能用 `promptId`（数字）或 `sourceRef`（`file:<sha256>` 那种运行键）指定 ——
 *    手机这边没有那份工作流库（`prompts` 由桌面产生），**名字解析不出 id**；
 *  - 想改参数要用 `overrides`（键 = `节点id.输入名`）—— 手机看不到节点图，
 *    替模型猜一个键名就是编造；给自由文本 `prompt` 的也在这里当场退回。
 */
class ComfyQueueSubmitTool : MobileTool {
    override val name = "comfy_queue_submit"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        val queueId = com.comfyhub.harness.store.Timestamps.newId()
        val requestedAt = com.comfyhub.harness.store.Timestamps.now()
        // 摊平（与"推送那一刻"**共用同一份实现**：`SubmitJobPayload`）
        val flattened = com.comfyhub.harness.store.SubmitJobPayload.flatten(
            args = args,
            queueId = queueId,
            conversationId = ctx.conversationId,
            runId = ctx.runId,
            requestedAt = requestedAt,
        )
        val payload = when (flattened) {
            is com.comfyhub.harness.store.SubmitJobPayload.Flattened.Bad ->
                return ToolOutcome.failure("INVALID_ARGUMENT", flattened.reason)
            is com.comfyhub.harness.store.SubmitJobPayload.Flattened.Ok -> flattened.payload
        }
        // 存进去的就是**线上形状**：`submit_job` 没有本地行，这行 outbox 的 payload 就是指令本体
        // （推送时 `outboxTarget` 直接用它，只补一个 `requestedBy`）
        ctx.store.enqueue(com.comfyhub.harness.store.OutboxKind.SUBMIT_JOB, queueId, payload.toString())

        // 产物入口**只认后端给的 mediaIds**（AGENTS §10.1）——这里明确"还没有"
        return ToolOutcome.success(
            buildString {
                append("已排队（queueId=$queueId）。\n")
                append("**排队 ≠ 已提交**：桌面不在线时这张图还没有开始生成。")
                append("桌面在线后它**先弹一次审批**（手机上点的那一下不算批准），")
                append("批准了才走它那边的完整提交流程（含审核与入库），")
                append("结果会作为一条助手消息回到这个会话。")
                append("现在**不要**告诉用户图已经在跑了，也不要说已经生成。")
            },
            jsonPayload = buildJsonObject {
                put("queueId", queueId)
                put("status", "queued")
                // 刻意不给 mediaIds：给了就等于宣称产物已经存在
            },
        )
    }
}

// ---------------------------------------------------------------------------
//  web_fetch（出网 + 正文抽取 + 不可信横幅）
// ---------------------------------------------------------------------------

/** `web_fetch`：取一个网页的正文。结果**当不可信数据**（R7）。 */
class WebFetchTool : MobileTool {
    override val name = "web_fetch"

    override suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome {
        // ★ 联网预算：与 `web_search` / `civitai_search` **共用同一条**（`ToolContext.webQueries`），
        //   预检在调用之前、计数只在成功之后 —— 与桌面执行器同一条纪律。
        ctx.webBudgetFailure()?.let { return it }

        val url = args.str("url")
            ?: return ToolOutcome.failure("INVALID_ARGUMENT", "web_fetch 需要 url 参数")
        if (!url.startsWith("http://") && !url.startsWith("https://")) {
            return ToolOutcome.failure("INVALID_ARGUMENT", "url 必须以 http:// 或 https:// 开头")
        }
        val reply = runCatching { ctx.transport.get(url) }
            .getOrElse { e -> return ToolOutcome.failure("PROVIDER_UNREACHABLE", "取不到这个地址：${e.message}") }
        if (!reply.ok) {
            return ToolOutcome.failure("HTTP_${reply.status}", "取不到这个地址（HTTP ${reply.status}）")
        }
        val text = ToolResults.truncate(extractText(reply.body))
        val banner = ToolResults.untrustedBanner(ctx.kit.untrustedBanner, kind = "网页", query = url)
        ctx.webQueries++
        return ToolOutcome.success("$banner\n$text", buildJsonObject { put("url", url) })
    }

    companion object {
        /**
         * 极简 HTML → 文本。
         *
         * 只做"去脚本样式 / 去标签 / 解几个常见实体"三件事：
         * 手机侧不该为了抽正文引一整套 HTML 解析（那是管道，不是规则）。
         */
        fun extractText(body: String): String {
            if (!body.contains("<")) return body.trim()
            var text = body
            text = text.replace(Regex("(?is)<(script|style|noscript|svg)[^>]*>.*?</\\1>"), " ")
            text = text.replace(Regex("(?is)<br\\s*/?>"), "\n")
            text = text.replace(Regex("(?is)</(p|div|li|h[1-6]|tr)>"), "\n")
            text = text.replace(Regex("(?s)<[^>]+>"), " ")
            text = text.replace("&nbsp;", " ")
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
            return text.lines().map { it.trim() }.filter { it.isNotEmpty() }.joinToString("\n")
        }
    }
}

/** 手机侧内置工具表（顺序无关，按名字查）。 */
object BuiltinTools {
    fun all(): List<MobileTool> = listOf(
        RememberTool(),
        ListSkillsTool(),
        LoadSkillTool(),
        AskUserQuestionTool(),
        ComfyQueueSubmitTool(),
        WebFetchTool(),
        // ★ 2026-09-27 补齐：ToolScope 契约 §2 的 MOBILE 七个到这一步**全部有实现**。
        WebSearchTool(),
        CivitaiSearchTool(),
    )

    /**
     * **本版真正实现了的工具名**。
     *
     * 这份清单只用来给单测断言"声明出去的工具都真的能跑"，**不是**规则的真源 ——
     * 规则的真源是桌面的 `ToolRegistry`（红线 R12）。
     */
    fun implementedNames(): Set<String> = all().map { it.name }.toSet()
}
