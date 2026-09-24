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
        }
        return record
    }

    fun getRun(id: String): RunRecord? =
        session.queryOne("SELECT * FROM ai_runs WHERE id = ?", listOf(id))?.toRun()

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
        }
    }

    /**
     * **崩溃恢复**：找出没跑完的 Run。
     *
     * 手机上前台服务有 6h/24h 配额、后台随时可能被杀（总纲 §7 的风险表），
     * 所以"进程重启后能看出有一个 Run 没跑完"是需求，不是可选项：
     * 重启后要把它们按 **R6** 收尾（该 flush 的 flush、该标 failed 的标 failed），
     * 而不是留一个永远 `running` 的幽灵。
     */
    fun listUnfinishedRuns(): List<RunRecord> =
        session.query("SELECT * FROM ai_runs WHERE status = ? ORDER BY created_at ASC", listOf(RunStatus.RUNNING))
            .map { it.toRun() }

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

/** 上行 payload 的**最小**形状（完整行的组装在 M4，那里有 `/api/sync/push` 的形状）。 */
private fun conversationPayload(c: ConversationRecord): String =
    """{"id":${quote(c.id)},"title":${quote(c.title)},"updated_at":${quote(c.updatedAt)}}"""

private fun messagePayload(m: MessageRecord): String =
    """{"id":${quote(m.id)},"conversation_id":${quote(m.conversationId)},"seq":${m.seq},""" +
        """"role":${quote(m.role)},"status":${quote(m.status)}}"""
