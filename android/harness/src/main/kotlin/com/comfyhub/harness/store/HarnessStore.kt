package com.comfyhub.harness.store

import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.UUID

/**
 * 时间与 ID —— 两件**必须全库一致**的小事。
 *
 *  - 时间列一律 **TEXT ISO-8601 UTC、毫秒三位**（契约 §3.1）：`2026-09-23T05:30:00.000Z`。
 *    格式一致才能"按字符串排序 = 按时间排序"（会话列表就是按 `updated_at` 排的）。
 *    ⚠ 手机上**不用** `java.time`：`minSdk = 24`，`java.time` 要 API 26（要它就得开脱糖，
 *    那是另一个构建决策，不值得为两个格式化函数去动）。
 *  - 主键一律 **UUID 字符串**：两端各自生成不会撞（契约 §7.2）——
 *    这是"手机能当写者"的前提，自增整数做不到。
 */
object Timestamps {

    private const val PATTERN = "yyyy-MM-dd'T'HH:mm:ss.SSS'Z'"

    /** 逐线程一份 `SimpleDateFormat`（它不是线程安全的）。 */
    private val format = ThreadLocal.withInitial {
        SimpleDateFormat(PATTERN, Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }
    }

    /** 现在（UTC，毫秒三位）。 */
    fun now(): String = format.get().format(Date())

    /** 从毫秒时间戳转（测试与迁移用）。 */
    fun of(epochMillis: Long): String = format.get().format(Date(epochMillis))

    /** 新的 UUID 主键（会话 / 消息 / Run / outbox 都用它）。 */
    fun newId(): String = UUID.randomUUID().toString()
}

/**
 * 手机端 harness 的全部本地读写入口。
 *
 * ## 三条纪律
 *
 * 1. **业务行与 outbox 在同一个事务里写**（契约 §7.3）：漏了 outbox 就是"这次会话永远上不去"，
 *    而用户看不出任何异常 —— 所以凡是"手机产生、要上行"的写，都走这里的 `*AndQueue` 方法。
 * 2. **R2：写事务一律 `BEGIN IMMEDIATE`**（[SqlSession.transaction] 唯一实现）。
 * 3. **事件批量写**（P1 的教训）：一次 Run 实测 4.7 万个 `text.delta`，逐条 fsync 会把手机写爆。
 *    批量只影响"什么时候写"，不影响"写了什么"；但**闸门类事件**（`tool.requested` /
 *    `user.question`）必须立刻落库 —— 审批挂着的几分钟里，另一端要能查到"有东西在等人"。
 */
class HarnessStore(private val session: SqlSession) {

    // -----------------------------------------------------------------------
    //  会话
    // -----------------------------------------------------------------------

    fun createConversation(
        title: String = "新对话",
        providerId: String? = null,
        modelId: String? = null,
        promptVersion: String = "v1",
        queue: Boolean = true,
        /**
         * 显式主键。默认自己生成 UUID；调用方**已经有会话 id**时（重试 / 恢复 / 手机侧先建后补）
         * 必须把它传进来，否则会另起一个空会话。
         */
        id: String = Timestamps.newId(),
    ): ConversationRecord {
        val now = Timestamps.now()
        val record = ConversationRecord(
            id = id,
            title = title,
            providerId = providerId,
            modelId = modelId,
            systemPromptVersion = promptVersion,
            archived = false,
            createdAt = now,
            updatedAt = now,
        )
        session.transaction {
            session.exec(
                """
                INSERT INTO ai_conversations
                  (id, title, provider_id, model_id, system_prompt_version, archived, created_at, updated_at)
                VALUES (?,?,?,?,?,0,?,?)
                """.trimIndent(),
                listOf(record.id, record.title, record.providerId, record.modelId, record.systemPromptVersion, now, now),
            )
            if (queue) enqueue(OutboxKind.CONVERSATION, record.id, conversationPayload(record))
        }
        return record
    }

    fun getConversation(id: String): ConversationRecord? =
        session.queryOne("SELECT * FROM ai_conversations WHERE id = ?", listOf(id))?.toConversation()

    fun listConversations(limit: Int = 100, includeArchived: Boolean = false): List<ConversationRecord> {
        val sql = buildString {
            append("SELECT * FROM ai_conversations")
            if (!includeArchived) append(" WHERE archived = 0")
            append(" ORDER BY updated_at DESC LIMIT ?")
        }
        return session.query(sql, listOf(limit.toLong())).map { it.toConversation() }
    }

    /** 改标题（桌面赢 LWW 的另一半：手机只在"自己起的会话"上改名）。 */
    fun renameConversation(id: String, title: String, queue: Boolean = true) {
        val now = Timestamps.now()
        session.transaction {
            session.update(
                "UPDATE ai_conversations SET title = ?, updated_at = ? WHERE id = ?",
                listOf(title, now, id),
            )
            if (queue) getConversation(id)?.let { enqueue(OutboxKind.CONVERSATION, id, conversationPayload(it)) }
        }
    }

    /**
     * 删掉本机这条会话（连同它的消息 / 块 / Run / 工具明细 / 事件 —— 外键 `ON DELETE CASCADE`），
     * 并**留下一个本机删除标记**（[local_deletions]，见 [markDeleted] 与 [applyMirrorRow]）。
     *
     * ## 为什么手机需要一个**本地**删除
     *
     * 界面切会话时会顺手清掉"上一条空壳"（AIH-018）。在此之前那一步打的是**远端**
     * `DELETE /api/ai/conversations/{id}` —— 可手机上会话的真源是本地库，
     * 那条请求既够不着本地镜像（空壳越攒越多），离线时还每次白报一个失败。
     *
     * ## 只删本地，**不写 wire 上的 delete**
     *
     * 上行只有 `upsert` 一种 op（🔒 `docs/sync-api-contract.md` §4），手机不许伪造删除
     * （**R3**），而且"手机不修改桌面创建的既成事实"是设计里写死的纪律
     * （`docs/mobile-harness-design.md` §7）。所以两种情形都**如实**：
     *  - 还没推上去的：把它（以及它的消息 / 块 / Run / 工具明细）在 `_outbox` 里的待推行一并清掉，
     *    桌面**根本不会知道它存在过**；
     *  - 已经推上去的：桌面那份**留着**（那要由用户或桌面自己删），这里不假装"两边都删了"。
     *
     * ## 为什么要留标记（这条路唯一的理由）
     *
     * 桌面那份还在 ⇒ 下一次下行 apply 会把它整行覆盖回来 —— 用户刚删掉的会话
     * **当场复活**，而且两端都不报错。标记就是那句"这台手机上它已经被删了"
     * （`docs/implementation-master-plan.md` §25.1 第三条要求的那个形状）。
     * 它**不进镜像表、不进同步白名单、不挂触发器**（[MobileSchema.MIRROR_TABLES] 里没有它）。
     *
     * 标记里除了会话本体，还写上**这次级联删掉的每一个 id**（消息 / Run / 工具明细）：
     * 下行 apply 时 `ai_message_parts` / `ai_tool_calls` 自己不带 `conversation_id`，
     * 它们的父行这会儿在本机**已经不存在了** —— 只靠"父行还在时才查得到会话"是查不中的
     * （见 [applyMirrorRow] 的三条判据）。
     *
     * ⚠ 返回值只说"**本机**是否真的删掉了一行"：id 不存在回 `false`（那不是错误，
     * 重复清理很常见；**但标记照写** —— 用户点过删除这个意图不能因为"本机本来就没有"而丢掉）；
     * 而库真的出错时一律**抛**出去 —— 静默回 `false` 会让界面以为
     * 删掉了，实际空壳还在（那是"报错要如实"里最典型的一种假成功）。
     */
    fun deleteConversation(id: String): Boolean {
        if (id.isBlank()) return false
        var removed = false
        session.transaction {
            // ① 先把这次要级联带走的 id 记下来（删完就查不出来了），逐条写删除标记
            markDeleted(MobileSchema.CONVERSATIONS_TABLE, id)
            session.query("SELECT id FROM ai_messages WHERE conversation_id = ?", listOf(id))
                .forEach { markDeleted("ai_messages", it.str("id")) }
            session.query("SELECT id FROM ai_runs WHERE conversation_id = ?", listOf(id))
                .forEach { markDeleted("ai_runs", it.str("id")) }
            session.query(
                """
                SELECT id FROM ai_tool_calls
                 WHERE run_id IN (SELECT id FROM ai_runs WHERE conversation_id = ?)
                """.trimIndent(),
                listOf(id),
            ).forEach { markDeleted("ai_tool_calls", it.str("id")) }

            // ② 清掉它（以及它的消息 / 块 / Run / 工具明细）在 `_outbox` 里的待推行。
            //    顺序不能反：消息行被删掉之后就查不出它们的 id 了。
            //    part 的 refId 形状是 `<messageId>#<ordinal>`（见 [appendPart]），
            //    所以按 '#' 前缀反推它属于哪条消息。
            session.update(
                """
                DELETE FROM _outbox
                 WHERE ref_id = ?
                    OR ref_id IN (SELECT id FROM ai_messages WHERE conversation_id = ?)
                    OR (kind = ? AND substr(ref_id, 1, instr(ref_id || '#', '#') - 1)
                          IN (SELECT id FROM ai_messages WHERE conversation_id = ?))
                    OR ref_id IN (SELECT id FROM ai_runs WHERE conversation_id = ?)
                    OR ref_id IN (SELECT id FROM ai_tool_calls
                                   WHERE run_id IN (SELECT id FROM ai_runs WHERE conversation_id = ?))
                """.trimIndent(),
                listOf(id, id, OutboxKind.PART, id, id, id),
            )
            removed = session.update("DELETE FROM ai_conversations WHERE id = ?", listOf(id)) > 0
        }
        return removed
    }

    // -----------------------------------------------------------------------
    //  本机删除标记（local_deletions）—— 下行 apply 要按它跳过
    // -----------------------------------------------------------------------

    /**
     * 记一条本机删除标记（**只增不删**）。
     *
     * [kind] 用**镜像表名**（`ai_conversations` / `ai_messages` / `ai_runs` / `ai_tool_calls`），
     * 不是 wire kind（`ai_conversation`）—— 判据与被判定的对象（[MobileSchema.MIRROR_TABLES]）
     * 用同一套词汇才查得中。重复标记是覆盖（`ON CONFLICT DO UPDATE` 只刷新时间），
     * **不是**插入第二行。
     *
     * ⚠ 它**自己不开事务**：调用方（[deleteConversation]）要把标记与删除绑在同一个事务里 ——
     * 分开写就会出现"行删了、标记没写上"的窗口，而那个窗口里的一次同步正好把会话复活。
     */
    fun markDeleted(kind: String, id: String) {
        if (kind.isBlank() || id.isBlank()) return
        session.exec(
            """
            INSERT INTO local_deletions (kind, id, deleted_at) VALUES (?,?,?)
            ON CONFLICT(kind, id) DO UPDATE SET deleted_at = excluded.deleted_at
            """.trimIndent(),
            listOf(kind, id, Timestamps.now()),
        )
    }

    /** 这个 id 在这台手机上被删过吗（[kind] = 镜像表名）。 */
    fun isDeleted(kind: String, id: String): Boolean =
        session.queryOne("SELECT 1 AS x FROM local_deletions WHERE kind = ? AND id = ?", listOf(kind, id)) != null

    /** 本机删除标记清单（用例与诊断用；顺序稳定便于断言）。 */
    fun listDeletions(): List<Pair<String, String>> =
        session.query("SELECT kind, id FROM local_deletions ORDER BY kind ASC, id ASC")
            .map { it.str("kind") to it.str("id") }

    // -----------------------------------------------------------------------
    //  消息与有序块
    // -----------------------------------------------------------------------

    /** 追加一条消息（`seq` 在同一会话内单调，与桌面同约定）。 */
    fun appendMessage(
        conversationId: String,
        role: String,
        text: String? = null,
        status: String = MessageStatus.COMPLETE,
        providerId: String? = null,
        modelId: String? = null,
        queue: Boolean = true,
        /**
         * 显式主键。默认自己生成 UUID。
         *
         * ⚠ 与 [createConversation] 的 `id` 同一个理由：界面在发出请求的**同一刻**就会
         * 乐观插一条占位气泡（它得先有 id 才能被流式增量找到）。如果这里另起一个 UUID，
         * 界面手里那个 id 就永远查不到这条消息 —— 表现是"事件都在，界面上却不长字"。
         */
        id: String = Timestamps.newId(),
    ): MessageRecord {
        val now = Timestamps.now()
        val record = MessageRecord(
            id = id,
            conversationId = conversationId,
            seq = nextMessageSeq(conversationId),
            role = role,
            status = status,
            text = text,
            providerId = providerId,
            modelId = modelId,
            providerResponseId = null,
            usageJson = null,
            createdAt = now,
            updatedAt = now,
        )
        session.transaction {
            session.exec(
                """
                INSERT INTO ai_messages
                  (id, conversation_id, seq, role, status, text, provider_id, model_id, created_at, updated_at)
                VALUES (?,?,?,?,?,?,?,?,?,?)
                """.trimIndent(),
                listOf(
                    record.id, record.conversationId, record.seq.toLong(), record.role, record.status,
                    record.text, record.providerId, record.modelId, now, now,
                ),
            )
            session.update("UPDATE ai_conversations SET updated_at = ? WHERE id = ?", listOf(now, conversationId))
            if (queue) enqueue(OutboxKind.MESSAGE, record.id, messagePayload(record))
        }
        return record
    }

    fun getMessage(id: String): MessageRecord? =
        session.queryOne("SELECT * FROM ai_messages WHERE id = ?", listOf(id))?.toMessage()

    fun listMessages(conversationId: String): List<MessageRecord> =
        session.query(
            "SELECT * FROM ai_messages WHERE conversation_id = ? ORDER BY seq ASC",
            listOf(conversationId),
        ).map { it.toMessage() }

    fun maxMessageSeq(conversationId: String): Int =
        session.queryOne(
            "SELECT COALESCE(MAX(seq), 0) AS m FROM ai_messages WHERE conversation_id = ?",
            listOf(conversationId),
        )?.int("m") ?: 0

    private fun nextMessageSeq(conversationId: String): Int = maxMessageSeq(conversationId) + 1

    /**
     * 更新正文与状态 —— **R6 的落库一半**：失败 / 取消时"已经说出去的话"也要能写进来。
     *
     * 所以这个方法允许把 `text` 写成**非空**且 `status = failed`（而不是抹成空串）：
     * 桌面那边踩过"失败分支把 text 抹空、用户看到的回复消失了"的坑（AGENTS §10.1）。
     */
    fun updateMessage(
        id: String,
        text: String? = null,
        status: String? = null,
        providerResponseId: String? = null,
        usageJson: String? = null,
        queue: Boolean = true,
    ) {
        val now = Timestamps.now()
        val sets = mutableListOf<String>()
        val args = mutableListOf<Any?>()
        text?.let { sets += "text = ?"; args += it }
        status?.let { sets += "status = ?"; args += it }
        providerResponseId?.let { sets += "provider_response_id = ?"; args += it }
        usageJson?.let { sets += "usage_json = ?"; args += it }
        if (sets.isEmpty()) return
        sets += "updated_at = ?"; args += now
        args += id

        session.transaction {
            session.update("UPDATE ai_messages SET ${sets.joinToString(", ")} WHERE id = ?", args)
            if (queue) getMessage(id)?.let {
                enqueue(OutboxKind.MESSAGE, id, messagePayload(it))
                session.update(
                    "UPDATE ai_conversations SET updated_at = ? WHERE id = ?",
                    listOf(now, it.conversationId),
                )
            }
        }
    }

    /** 落一个有序块（`ordinal` 由调用方按到达顺序给，见 `PartSink`）。 */
    fun appendPart(
        messageId: String,
        ordinal: Int,
        type: String,
        text: String? = null,
        toolCallId: String? = null,
        jsonPayload: String? = null,
        queue: Boolean = true,
    ) {
        session.transaction {
            session.exec(
                """
                INSERT INTO ai_message_parts (message_id, ordinal, type, text, tool_call_id, json_payload)
                VALUES (?,?,?,?,?,?)
                ON CONFLICT(message_id, ordinal) DO UPDATE SET
                  type = excluded.type, text = excluded.text,
                  tool_call_id = excluded.tool_call_id, json_payload = excluded.json_payload
                """.trimIndent(),
                listOf(messageId, ordinal.toLong(), type, text, toolCallId, jsonPayload),
            )
            if (queue) {
                enqueue(
                    OutboxKind.PART,
                    "$messageId#$ordinal",
                    """{"message_id":${quote(messageId)},"ordinal":$ordinal,"type":${quote(type)}}""",
                )
            }
        }
    }

    fun listParts(messageId: String): List<MessagePartRecord> =
        session.query(
            "SELECT * FROM ai_message_parts WHERE message_id = ? ORDER BY ordinal ASC",
            listOf(messageId),
        ).map { it.toPart() }

    fun maxPartOrdinal(messageId: String): Int =
        session.queryOne(
            "SELECT COALESCE(MAX(ordinal), -1) AS m FROM ai_message_parts WHERE message_id = ?",
            listOf(messageId),
        )?.int("m") ?: -1

    fun hasParts(messageId: String): Boolean =
        (session.queryOne(
            "SELECT COUNT(*) AS c FROM ai_message_parts WHERE message_id = ?",
            listOf(messageId),
        )?.long("c") ?: 0L) > 0L

    // -----------------------------------------------------------------------
    //  Run（含崩溃恢复）
    // -----------------------------------------------------------------------

    fun createRun(
        conversationId: String,
        providerId: String?,
        modelId: String?,
        userMessageId: String?,
        assistantMessageId: String?,
        promptVersion: String?,
        reasoningEffort: String? = null,
    ): RunRecord {
        val now = Timestamps.now()
        val record = RunRecord(
            id = Timestamps.newId(),
            conversationId = conversationId,
            status = RunStatus.RUNNING,
            providerId = providerId,
            modelId = modelId,
            userMessageId = userMessageId,
            assistantMessageId = assistantMessageId,
            promptVersion = promptVersion,
            reasoningEffort = reasoningEffort,
            errorCode = null,
            errorMessage = null,
            startedAt = now,
            completedAt = null,
            createdAt = now,
            updatedAt = now,
        )
        session.transaction {
            session.exec(
                """
                INSERT INTO ai_runs
                  (id, conversation_id, status, provider_id, model_id, user_message_id, assistant_message_id,
                   prompt_version, reasoning_effort, started_at, created_at, updated_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
                """.trimIndent(),
                listOf(
                    record.id, record.conversationId, record.status, record.providerId, record.modelId,
                    record.userMessageId, record.assistantMessageId, record.promptVersion,
                    record.reasoningEffort, now, now, now,
                ),
            )
            // ★ 同一事务里记一笔"这次 Run 是**本机**起的"。
            //   `ai_runs` 自 2026-09-27 起是**镜像表**（总纲 §7 待定项 1 = 进同步范围）：桌面的 Run
            //   也会整行落进来，而崩溃恢复（[listUnfinishedRuns]）要的是"**我**上次没跑完的那次"。
            //   漏了这一步的表现：桌面的普通 Run 一同步下来就被手机按 R6 标成 failed。
            session.exec("INSERT INTO local_runs (run_id) VALUES (?)", listOf(record.id))
            // ★ 入队（M4）：手机自己跑的 Run 要能上行回桌面（wire kind `ai_run`）。
            //   与业务行**同一个事务**（类注释第 1 条）：漏了就是"这次 Run 桌面上永远看不到"。
            //   ⚠ 只对本机起的 Run 入队 —— 这是"回声"唯一的闸门（[finishRun] 里的
            //   `local_runs` 判据是第二道，[outboxTarget] 里还有第三道）。
            enqueue(OutboxKind.AI_RUN, record.id, runPayload(record))
        }
        return record
    }

    /** 这次 Run 是不是**本机**起的（= `local_runs` 里有它）。 */
    fun isLocalRun(runId: String): Boolean =
        session.queryOne("SELECT 1 AS x FROM local_runs WHERE run_id = ?", listOf(runId)) != null

    fun getRun(id: String): RunRecord? =
        session.queryOne("SELECT * FROM ai_runs WHERE id = ?", listOf(id))?.toRun()

    /**
     * Run 收尾（正常 / 失败 / 取消三条路都汇到这里）。
     *
     * ★ **收尾即入队**（M4）：推到桌面的是**推送那一刻现读的整行**（见 [outboxTarget]），
     * 所以入队本身只是"提醒同步通道：这一行变了"。三条路都不漏 —— 失败 / 取消的 Run
     * 同样是事实，桌面要能看到（"手机那次跑挂了"也是工具过程的一部分）。
     *
     * ⚠ 只有 `local_runs` 里有的 Run 才入队：`ai_runs` 是镜像表，桌面同步下来的 Run 也在库里，
     * 把它再推回去就是一次**回声**（而且它的 `updated_at` 是桌面给的，推回去只会落到
     * `STALE_IGNORED`，白搭一次往返）。崩溃恢复那条路（桥的 `recoverUnfinished`）走的就是
     * 本方法，那里的 Run 一定来自 [listUnfinishedRuns]（已按 `local_runs` 过滤过）。
     */
    fun finishRun(
        id: String,
        status: String,
        errorCode: String? = null,
        errorMessage: String? = null,
        usageJson: String? = null,
    ) {
        val now = Timestamps.now()
        session.transaction {
            session.update(
                """
                UPDATE ai_runs SET status = ?, error_code = ?, error_message = ?, usage_json = ?,
                                   completed_at = ?, updated_at = ?
                 WHERE id = ?
                """.trimIndent(),
                listOf(status, errorCode, errorMessage, usageJson, now, now, id),
            )
            if (isLocalRun(id)) getRun(id)?.let { enqueue(OutboxKind.AI_RUN, id, runPayload(it)) }
        }
    }

    /**
     * **崩溃恢复**：找出没跑完的 Run —— **只找本机起的那些**。
     *
     * 手机上前台服务有 6h/24h 配额、后台随时可能被杀（总纲 §7 的风险表），
     * 所以"进程重启后能看出有一个 Run 没跑完"是需求，不是可选项：
     * 重启后要把它们按 **R6** 收尾（该 flush 的 flush、该标 failed 的标 failed），
     * 而不是留一个永远 `running` 的幽灵。
     *
     * ⚠ **判据不能只是 `status = 'running'`**：`ai_runs` 自 2026-09-27 起是**镜像表**
     * （总纲 §7 待定项 1 = 进同步范围），桌面那次 Run 也会整行同步下来 —— 桌面上真的在跑的
     * Run 会被手机当成"我的未完成 Run"，被按 R6 标成 failed 再推回桌面。
     * 所以这里 `JOIN local_runs`（[createRun] 在同一个事务里写的标记）。
     */
    fun listUnfinishedRuns(): List<RunRecord> =
        session.query(
            """
            SELECT r.* FROM ai_runs r
              JOIN local_runs l ON l.run_id = r.id
             WHERE r.status = ?
             ORDER BY r.created_at ASC
            """.trimIndent(),
            listOf(RunStatus.RUNNING),
        ).map { it.toRun() }

    // -----------------------------------------------------------------------
    //  工具调用（`ai_tool_calls` —— 2026-09-27 起进同步范围）
    // -----------------------------------------------------------------------

    /**
     * 落一次工具调用的**开始**（`status = pending`）。
     *
     * 为什么要有这张表的行：桌面要能回答"**手机那次 Run 跑过哪些工具、成没成、花了多久**"
     * （总纲 §3.3 的 2026-09-27 拍板：`ai_runs` / `ai_tool_calls` 进同步范围）。
     * 只靠 `ai_message_parts` 里的 `tool_call` / `tool_result` 块是不够的 ——
     * 那是**给界面渲染**的有序块，审批态、耗时、结构化结果都不在里面。
     *
     * ## 三个字段的口径（与桌面 `runs.rs` 逐字对齐，否则两端读同一行会读出两种意思）
     *
     *  - `id`：**本机生成的 UUID**（桌面的 `id` 也是 UUID）。刻意**不用**模型给的
     *    `provider_call_id` 当主键：那个 id 在一次 Run 内唯一，跨 Run 却会重（上游很爱发
     *    `call_0` 这种短名），拿它当全局主键迟早撞 `ON CONFLICT`；
     *  - `provider_call_id`：模型/上游给的那个 id（`tool.requested` 事件里也是它）；
     *  - `approval`：手机侧的工具都不需要审批（没有桌面写操作），恒 `not_required` ——
     *    **键名与取值都必须与桌面一致**，否则共享的 Flutter 界面读不到审批态。
     *
     * @return 这一行的主键（收尾时用它回填；[finishToolCall] 的入参）。
     */
    fun startToolCall(
        runId: String,
        providerCallId: String,
        name: String,
        argumentsJson: String?,
        approval: String = APPROVAL_NOT_REQUIRED,
    ): String {
        val id = Timestamps.newId()
        val now = Timestamps.now()
        session.transaction {
            session.exec(
                """
                INSERT INTO ai_tool_calls
                  (id, run_id, provider_call_id, name, arguments_json, approval, status,
                   elapsed_ms, started_at, created_at)
                VALUES (?,?,?,?,?,?,?,0,?,?)
                """.trimIndent(),
                listOf(id, runId, providerCallId, name, argumentsJson, approval, STATUS_PENDING, now, now),
            )
            enqueue(OutboxKind.AI_TOOL_CALL, id, toolCallPayload(
                id = id, runId = runId, providerCallId = providerCallId, name = name,
                argumentsJson = argumentsJson, approval = approval, status = STATUS_PENDING,
            ))
        }
        return id
    }

    /**
     * 回填一次工具调用的**结果**（`status` / `content` / `result_json` / `error` / `elapsed_ms`）。
     *
     * **失败也要落行**（`status = failed` + `error`）：工具失败是 Run 的一部分事实，
     * 桌面看"这次手机上工具报了什么错"就是看这一行。绝不因为失败就不写。
     *
     * 行不存在时**什么都不做**（`update` 影响 0 行）—— 那时说明这次调用从来没落过库
     * （不该发生），静默按"没有这行"处理比造一行假的更安全。
     */
    fun finishToolCall(
        id: String,
        status: String,
        content: String? = null,
        resultJson: String? = null,
        error: String? = null,
        elapsedMs: Long = 0,
    ) {
        val now = Timestamps.now()
        session.transaction {
            val changed = session.update(
                """
                UPDATE ai_tool_calls
                   SET status = ?, content = ?, result_json = ?, error = ?, elapsed_ms = ?, completed_at = ?
                 WHERE id = ?
                """.trimIndent(),
                listOf(status, content, resultJson, error, elapsedMs, now, id),
            )
            if (changed > 0) {
                getToolCall(id)?.let { row ->
                    enqueue(
                        OutboxKind.AI_TOOL_CALL,
                        id,
                        toolCallPayload(
                            id = row.id, runId = row.runId, providerCallId = row.providerCallId,
                            name = row.name, argumentsJson = row.argumentsJson, approval = row.approval,
                            status = row.status,
                        ),
                    )
                }
            }
        }
    }

    fun getToolCall(id: String): ToolCallRecord? =
        session.queryOne("SELECT * FROM ai_tool_calls WHERE id = ?", listOf(id))?.toToolCall()

    /** 一次 Run 的工具调用（按开始时间；与桌面 `runs.rs` 的 `ORDER BY started_at, id` 同序）。 */
    fun listToolCalls(runId: String): List<ToolCallRecord> =
        session.query(
            "SELECT * FROM ai_tool_calls WHERE run_id = ? ORDER BY started_at ASC, id ASC",
            listOf(runId),
        ).map { it.toToolCall() }

    // -----------------------------------------------------------------------
    //  事件流（R5）
    // -----------------------------------------------------------------------

    /**
     * 落一条事件（`seq` 由 [com.comfyhub.harness.event.RunEventBus] 发号，**不由本方法生成**）。
     *
     * 两条路都写同一张表：进程活着时靠内存环形缓冲回放，进程被杀后靠这张表回放（R5）。
     */
    fun appendEvent(runId: String, seq: Int, type: String, payloadJson: String?) {
        session.exec(
            """
            INSERT INTO ai_run_events (run_id, seq, event_type, payload_json) VALUES (?,?,?,?)
            ON CONFLICT(run_id, seq) DO UPDATE SET event_type = excluded.event_type,
                                                   payload_json = excluded.payload_json
            """.trimIndent(),
            listOf(runId, seq.toLong(), type, payloadJson),
        )
    }

    fun listEvents(runId: String, after: Int = 0, limit: Int = 4000): List<Pair<Int, String>> =
        session.query(
            "SELECT seq, event_type FROM ai_run_events WHERE run_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
            listOf(runId, after.toLong(), limit.toLong()),
        ).map { it.int("seq")!! to it.str("event_type") }

    fun maxEventSeq(runId: String): Int =
        session.queryOne("SELECT COALESCE(MAX(seq), 0) AS m FROM ai_run_events WHERE run_id = ?", listOf(runId))
            ?.int("m") ?: 0

    /**
     * 裁掉最老的、**只留尾部 [keep] 条**（R5 的库侧窗口）。
     *
     * 断线重连永远只看最新的一段（`?after=<seq>`），所以留尾部是对的；
     * 而"全量留"正是 P1 要治的病（单 Run 4.7 万条）。
     */
    fun pruneEvents(runId: String, keep: Int = EVENT_KEEP): Int =
        session.update(
            """
            DELETE FROM ai_run_events
             WHERE run_id = ? AND seq <= (
               SELECT COALESCE(MAX(seq), 0) FROM ai_run_events WHERE run_id = ?
             ) - ?
            """.trimIndent(),
            listOf(runId, runId, keep.toLong()),
        )

    // -----------------------------------------------------------------------
    //  上行队列（契约 §4）
    // -----------------------------------------------------------------------

    /**
     * 入队一条上行记录（**在调用方的事务里**执行 —— 它自己不开事务，
     * 就是为了能跟业务行绑在同一个事务里，见类注释第 1 条）。
     */
    fun enqueue(kind: String, refId: String, payloadJson: String) {
        session.exec(
            "INSERT INTO _outbox (id, kind, ref_id, payload, created_at) VALUES (?,?,?,?,?)",
            listOf(Timestamps.newId(), kind, refId, payloadJson, Timestamps.now()),
        )
    }

    fun pendingOutbox(limit: Int = 500): List<OutboxRecord> =
        session.query("SELECT * FROM _outbox ORDER BY created_at ASC LIMIT ?", listOf(limit.toLong()))
            .map { it.toOutbox() }

    /** 失败退避：`tries` 加一、记 `last_error`（**不许静默丢弃**）。 */
    fun markOutboxFailed(id: String, error: String) {
        session.update(
            "UPDATE _outbox SET tries = tries + 1, last_error = ? WHERE id = ?",
            listOf(error, id),
        )
    }

    fun removeOutbox(id: String) {
        session.update("DELETE FROM _outbox WHERE id = ?", listOf(id))
    }

    /**
     * 一条 outbox 记录指向的**本地行**（上行 payload 的真源，契约 §4）。
     *
     * 为什么不在 `enqueue` 的那一刻把 payload 存好：这些行在 Run 期间**一直在变**
     * （助手消息的正文是流式追加的、块是一条条来的、状态从 streaming 变 complete），
     * 入队那一刻的快照必然是半截的 —— 推上去桌面就会看到一条空回复。
     * **推送时现读**才拿得到终值；`_outbox.payload` 那一列从此只是排查用的痕迹。
     *
     * 返回 `null` 的三种情况都要**如实**：① 这个 kind 还没有上行通道
     * （`run` / `submit_job` 归 M5）；② 本地那一行已经没了（推送前被删了）；
     * ③ `ai_run` / `ai_tool_call` 指向的是**桌面同步下来的**行（不在 `local_runs` 里）——
     * 那道闸门是"回声"的最后一道防线（入队那两处已经各判过一次）。
     */
    fun outboxTarget(kind: String, refId: String): OutboxTarget? {
        val wireKind: String
        val row: Row?
        when (kind) {
            OutboxKind.CONVERSATION -> {
                wireKind = "ai_conversation"
                row = session.queryOne("SELECT * FROM ai_conversations WHERE id = ?", listOf(refId))
            }
            OutboxKind.MESSAGE -> {
                wireKind = "ai_message"
                row = session.queryOne("SELECT * FROM ai_messages WHERE id = ?", listOf(refId))
            }
            OutboxKind.PART -> {
                // refId 的形状是 `<messageId>#<ordinal>`（与桌面触发器写账本的编码同形）
                val messageId = refId.substringBeforeLast('#')
                val ordinal = refId.substringAfterLast('#').toLongOrNull() ?: return null
                wireKind = "ai_message_part"
                row = session.queryOne(
                    "SELECT * FROM ai_message_parts WHERE message_id = ? AND ordinal = ?",
                    listOf(messageId, ordinal),
                )
            }
            OutboxKind.AI_RUN -> {
                // ⚠ 桌面同步下来的 Run **不许**推回去（回声）：`ai_runs` 是镜像表，
                //   库里两边的 Run 长得一模一样，唯一的区分就是本机的 `local_runs` 标记。
                if (!isLocalRun(refId)) return null
                wireKind = "ai_run"
                row = session.queryOne("SELECT * FROM ai_runs WHERE id = ?", listOf(refId))
            }
            OutboxKind.AI_TOOL_CALL -> {
                // 同理：工具明细挂在 Run 上，Run 不是我起的就不推（`ai_tool_calls` 没有
                // `updated_at`，桌面那边 LWW 对它不生效 —— 推一条桌面的行回去会**直接覆盖**）。
                val runId = session.queryOne("SELECT run_id FROM ai_tool_calls WHERE id = ?", listOf(refId))
                    ?.str("run_id") ?: return null
                if (!isLocalRun(runId)) return null
                wireKind = "ai_tool_call"
                row = session.queryOne("SELECT * FROM ai_tool_calls WHERE id = ?", listOf(refId))
            }
            else -> return null
        }
        return row?.let { OutboxTarget(wireKind, it.asMap()) }
    }

    // -----------------------------------------------------------------------
    //  下行镜像 apply（`/api/sync/bootstrap` 与 `changes` 的应用路径）
    // -----------------------------------------------------------------------

    /**
     * 落一行**桌面同步下来的整行**（契约 §2：`upserts` 里给的是完整行，手机直接覆盖本地行）。
     *
     * ## 三条判据（按顺序）—— 这一节就是"本机删除标记"存在的全部意义
     *
     * 1. **这一行自己的主键被标过删**（标记的 `kind` 就是本表名）；
     * 2. **它所属的会话被标过删**（会话删除在本地是级联的，桌面那份还在；只跳过会话行本身
     *    的话，子行的 INSERT 会直接撞外键 —— `PRAGMA foreign_keys = ON`（R1）下那就是
     *    一条 SQL 异常，整个 apply 都得炸）；
     * 3. 子行自己的父行在本机已经没了（`ai_message_parts` / `ai_tool_calls` 靠各自的 id 标记
     *    认出来 —— [deleteConversation] 会把级联删掉的 id 一并记进标记）。
     *
     * ⚠ **判据不参与 LWW**（这是有意定的，别改成"比 `updated_at`"）：
     * 手机侧的删除**在 wire 上没有任何表示**（R3：上行只有 upsert），所以桌面那份的
     * `updated_at` 永远停在原地，而"用户在这台手机上删掉了"是一个**终局意图**、
     * 不是一次可以靠时间戳分胜负的并发写。拿时间戳去比只会得出"桌面更新 ⇒ 复活"
     * 这个**恰恰是这张标记要挡住**的结论。
     *
     * @param conversationHint 父行在同一批里给出的会话 id（可选）。`ai_message_parts` /
     *   `ai_tool_calls` 自己**没有** `conversation_id` 列，而它们的父行可能刚被本批落库、
     *   也可能被标记跳过了 —— 那时本机查不到父行，只能靠调用方把父行上的会话 id 带下来。
     * @return 落库结果（[MirrorApply]）。**未知表**如实回 [MirrorApply.UNKNOWN_TABLE]，
     *   不抛也不静默当作成功。
     */
    fun applyMirrorRow(
        table: String,
        row: Map<String, Any?>,
        conversationHint: String? = null,
    ): MirrorApply {
        val mirror = MobileSchema.mirrorTable(table) ?: return MirrorApply.UNKNOWN_TABLE

        // 判据 ①：这一行自己被标过删
        if (mirror.pkKind == MobileSchema.PkKind.SINGLE) {
            val id = row[mirror.pkColumns.single()]?.toString()
            if (!id.isNullOrBlank() && isDeleted(mirror.name, id)) return MirrorApply.SKIPPED_DELETED
        }

        // 判据 ③：子行的父行在本机已经不在了（那条父行被删过 ⇒ 这里插进去必然撞外键）
        val orphanParent: Boolean = when (mirror.name) {
            "ai_message_parts" -> {
                val messageId = row["message_id"]?.toString().orEmpty()
                messageId.isNotBlank() && isDeleted("ai_messages", messageId)
            }
            "ai_tool_calls" -> {
                val runId = row["run_id"]?.toString().orEmpty()
                val callId = row["id"]?.toString().orEmpty()
                runId.isNotBlank() && (isDeleted("ai_runs", runId) || isDeleted("ai_tool_calls", callId))
            }
            else -> false
        }
        if (orphanParent) return MirrorApply.SKIPPED_DELETED

        // 判据 ②：它所属的会话被标过删
        val conversationId = conversationIdOf(mirror.name, row, conversationHint)
        if (!conversationId.isNullOrBlank() && isDeleted(MobileSchema.CONVERSATIONS_TABLE, conversationId)) {
            return MirrorApply.SKIPPED_DELETED
        }

        val columns = mirroredColumns(table)
        val unknown = row.keys - columns
        if (unknown.isNotEmpty()) {
            // 静默丢列 = 悄悄丢数据（"桌面加了一列、手机上永远没有"）；抛出去让调用方看得见。
            // 正常情况下到不了这里：schemaVersion 对不上时手机该先走 bootstrap。
            throw IllegalStateException(
                "桌面发来的 $table 行里有本机没有的列：${unknown.sorted()}（schemaVersion 对不上？）",
            )
        }
        val pk = mirror.pkColumns
        if (pk.any { !row.containsKey(it) }) {
            throw IllegalStateException("$table 的行缺主键列 ${pk.filter { !row.containsKey(it) }}")
        }
        val cols = row.keys.toList()
        val placeholders = cols.joinToString(", ") { "?" }
        val assignments = cols.filter { it !in pk }.joinToString(", ") { "$it = excluded.$it" }
        val sql = buildString {
            append("INSERT INTO $table (${cols.joinToString(", ")}) VALUES ($placeholders)")
            // R3：**绝不用** `INSERT OR REPLACE`（它内部是 DELETE+INSERT）；这里手机侧虽然
            // 不建账本，但"整行覆盖"的语义必须与桌面同一套写法，免得将来两端对不上。
            append(" ON CONFLICT(${pk.joinToString(", ")}) DO ")
            if (assignments.isEmpty()) append("NOTHING") else append("UPDATE SET $assignments")
        }
        session.exec(sql, cols.map { row[it] })
        return MirrorApply.WRITTEN
    }

    /** 下行 `deletes` 一组：按主键删掉本机镜像里的那一行（没有就是 no-op）。 */
    fun deleteMirrorRow(table: String, keys: List<Any?>): Boolean {
        val mirror = MobileSchema.mirrorTable(table) ?: return false
        val where = mirror.pkColumns.joinToString(" AND ") { "$it = ?" }
        return session.update("DELETE FROM $table WHERE $where", keys) > 0
    }

    /** 这张镜像表实际的列清单（**从 PRAGMA 现查**，不手写第二份 —— 手写的那份一漂移就静默少写一列）。 */
    private fun mirroredColumns(table: String): Set<String> =
        session.query("PRAGMA table_info($table)").map { it.str("name") }.toSet()

    /**
     * 这一行属于哪个会话（判据 ② 的解析路径）。
     *
     * 顺序：**父行带下来的提示**（同一批里的父行，可能刚落库也可能被标记跳过）→
     * 行自己的 `conversation_id` 列 → 从本机库里反查父行（父行还在时这条路才走得通）。
     */
    private fun conversationIdOf(table: String, row: Map<String, Any?>, hint: String?): String? {
        if (!hint.isNullOrBlank()) return hint
        row["conversation_id"]?.toString()?.takeIf { it.isNotBlank() }?.let { return it }
        return when (table) {
            "ai_message_parts" -> row["message_id"]?.toString()?.takeIf { it.isNotBlank() }?.let { messageId ->
                session.queryOne("SELECT conversation_id FROM ai_messages WHERE id = ?", listOf(messageId))
                    ?.str("conversation_id")
            }
            "ai_tool_calls" -> row["run_id"]?.toString()?.takeIf { it.isNotBlank() }?.let { runId ->
                session.queryOne("SELECT conversation_id FROM ai_runs WHERE id = ?", listOf(runId))
                    ?.str("conversation_id")
            }
            else -> null
        }
    }

    // -----------------------------------------------------------------------
    //  零散状态
    // -----------------------------------------------------------------------

    fun getKv(key: String): String? = session.queryOne("SELECT v FROM mobile_kv WHERE k = ?", listOf(key))?.text("v")

    fun putKv(key: String, value: String?) {
        session.exec(
            """
            INSERT INTO mobile_kv (k, v, updated_at) VALUES (?,?,?)
            ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = excluded.updated_at
            """.trimIndent(),
            listOf(key, value, Timestamps.now()),
        )
    }

    // -----------------------------------------------------------------------
    //  批量事件（P1 的写法，手机侧照搬）
    // -----------------------------------------------------------------------

    /**
     * 事件批量落库器。
     *
     * 用法：`emit(type)` 往 [RunEventBus] 发一条、同时 `batcher.add(...)`；
     * **闸门类事件**（[RunEventType.TOOL_REQUESTED] / [RunEventType.USER_QUESTION]）立刻 flush，
     * 其余攒着，Run 收尾（正常 / 失败 / 取消三条路）时 [flush]。
     */
    inner class EventBatcher(private val runId: String) {
        private val buffered = mutableListOf<Triple<Int, String, String?>>()

        fun add(seq: Int, type: String, payloadJson: String?) {
            buffered += Triple(seq, type, payloadJson)
            if (type == com.comfyhub.harness.event.RunEventType.TOOL_REQUESTED ||
                type == com.comfyhub.harness.event.RunEventType.USER_QUESTION
            ) {
                flush()
            }
        }

        /** @return 真正写进去的条数（便于日志/用例断言"批量确实生效"）。 */
        fun flush(): Int {
            if (buffered.isEmpty()) return 0
            val batch = buffered.toList()
            buffered.clear()
            session.transaction { batch.forEach { (seq, type, payload) -> appendEvent(runId, seq, type, payload) } }
            return batch.size
        }

        fun pendingCount(): Int = buffered.size
    }

    /** 每个 Run 在库里保留多少条事件（与桌面 `HarnessRunner.EVENT_KEEP` 同值）。 */
    companion object {
        const val EVENT_KEEP = 4000

        /**
         * 工具调用的审批态 —— 手机侧的工具**都不需要审批**（没有桌面写操作），所以恒这个值。
         *
         * ⚠ 取值必须与桌面 `ai_tool_calls.approval` 同一套（`not_required` / `pending` /
         * `approved` / `denied`）：共享的 Flutter 界面就是按它画审批态的，写一个别的字符串
         * 会变成"这张卡永远显示未审批"，而两端都不报错。
         */
        const val APPROVAL_NOT_REQUIRED = "not_required"

        /**
         * 工具调用行的状态（与桌面 `runs.rs` 同一套：`pending` → `completed` / `failed`）。
         *
         * ⚠ 注意与 `ToolOutcome.status`（`success` / `failed`，那是**给模型的**口径）分开：
         * 落库这一列是给**界面与桌面**看的。
         */
        const val STATUS_PENDING = "pending"
        const val STATUS_COMPLETED = "completed"
        const val STATUS_FAILED = "failed"
    }
}

// ---------------------------------------------------------------------------
//  行映射 / 序列化辅助
// ---------------------------------------------------------------------------

private fun quote(value: String): String =
    "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

private fun Row.toConversation() = ConversationRecord(
    id = str("id"),
    title = str("title"),
    providerId = text("provider_id"),
    modelId = text("model_id"),
    systemPromptVersion = str("system_prompt_version"),
    archived = bool("archived"),
    createdAt = str("created_at"),
    updatedAt = str("updated_at"),
)

private fun Row.toMessage() = MessageRecord(
    id = str("id"),
    conversationId = str("conversation_id"),
    seq = int("seq") ?: 0,
    role = str("role"),
    status = str("status"),
    text = text("text"),
    providerId = text("provider_id"),
    modelId = text("model_id"),
    providerResponseId = text("provider_response_id"),
    usageJson = text("usage_json"),
    createdAt = str("created_at"),
    updatedAt = str("updated_at"),
)

private fun Row.toPart() = MessagePartRecord(
    messageId = str("message_id"),
    ordinal = int("ordinal") ?: 0,
    type = str("type"),
    text = text("text"),
    toolCallId = text("tool_call_id"),
    jsonPayload = text("json_payload"),
)

private fun Row.toRun() = RunRecord(
    id = str("id"),
    conversationId = str("conversation_id"),
    status = str("status"),
    providerId = text("provider_id"),
    modelId = text("model_id"),
    userMessageId = text("user_message_id"),
    assistantMessageId = text("assistant_message_id"),
    promptVersion = text("prompt_version"),
    reasoningEffort = text("reasoning_effort"),
    errorCode = text("error_code"),
    errorMessage = text("error_message"),
    startedAt = text("started_at"),
    completedAt = text("completed_at"),
    createdAt = str("created_at"),
    updatedAt = str("updated_at"),
)

private fun Row.toOutbox() = OutboxRecord(
    id = str("id"),
    kind = str("kind"),
    refId = str("ref_id"),
    payload = str("payload"),
    createdAt = str("created_at"),
    tries = int("tries") ?: 0,
    lastError = text("last_error"),
)

private fun Row.toToolCall() = ToolCallRecord(
    id = str("id"),
    runId = str("run_id"),
    providerCallId = str("provider_call_id"),
    name = str("name"),
    argumentsJson = text("arguments_json"),
    approval = str("approval"),
    status = str("status"),
    resultJson = text("result_json"),
    content = text("content"),
    error = text("error"),
    elapsedMs = long("elapsed_ms") ?: 0L,
    startedAt = text("started_at"),
    completedAt = text("completed_at"),
    createdAt = str("created_at"),
)

/**
 * 上行 payload 的**最小**形状（`/api/sync/push` 推的是推送那一刻现读的完整行，
 * 见 [HarnessStore.outboxTarget]；这一列只是排查用的痕迹）。
 */
private fun conversationPayload(c: ConversationRecord): String =
    """{"id":${quote(c.id)},"title":${quote(c.title)},"updated_at":${quote(c.updatedAt)}}"""

private fun messagePayload(m: MessageRecord): String =
    """{"id":${quote(m.id)},"conversation_id":${quote(m.conversationId)},"seq":${m.seq},""" +
        """"role":${quote(m.role)},"status":${quote(m.status)}}"""

private fun runPayload(r: RunRecord): String =
    """{"id":${quote(r.id)},"conversation_id":${quote(r.conversationId)},"status":${quote(r.status)}}"""

private fun toolCallPayload(
    id: String,
    runId: String,
    providerCallId: String,
    name: String,
    argumentsJson: String?,
    approval: String,
    status: String,
): String = buildString {
    append("""{"id":${quote(id)},"run_id":${quote(runId)},"provider_call_id":${quote(providerCallId)}""")
    append(""","name":${quote(name)},"approval":${quote(approval)},"status":${quote(status)}""")
    argumentsJson?.let { append(""","arguments_json":${quote(it)}""") }
    append('}')
}
