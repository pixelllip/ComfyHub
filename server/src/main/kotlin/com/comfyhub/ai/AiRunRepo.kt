package com.comfyhub.ai

import com.comfyhub.AppJson
import com.comfyhub.Db
import com.comfyhub.execute
import com.comfyhub.isoTime
import com.comfyhub.queryList
import com.comfyhub.queryOne
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import java.sql.ResultSet
import java.util.UUID

/**
 * Run 的持久化（AIH-020 / AIH-023 / AIH-024）。
 *
 * 一个 Run 开始时把 Provider / 模型 / 提示词版本**固化成快照**：之后用户改设置、
 * 换密钥都不影响这次已经开始的 Run（快照里**不含密钥**，只有引用名）。
 */
@Serializable
data class AiRunDto(
    val id: String,
    val conversationId: String,
    /** running / completed / failed / cancelled */
    val status: String,
    val providerId: String? = null,
    val modelId: String? = null,
    val assistantMessageId: String? = null,
    val userMessageId: String? = null,
    val errorCode: String? = null,
    val errorMessage: String? = null,
    val promptVersion: String? = null,
    /** 本次 Run 实际生效的思考强度（`off` 表示不支持或用户关掉了，AIH-056） */
    val reasoningEffort: String? = null,
    val retryOfRunId: String? = null,
    val startedAt: String? = null,
    val completedAt: String? = null,
    val createdAt: String? = null,
)

@Serializable
data class AiRunStartRequest(
    val text: String,
    val providerId: String,
    val modelId: String,
    /** 思考强度：off / low / medium / high / max；缺省按 `off`（AIH-056） */
    val reasoningEffort: String? = null,
    /**
     * 要随这次提问一起发给模型的附件 id（M3）。**准入判定在创建 Run 之前完成**：
     * 任何一个附件不通过就直接 400，不会产生上游请求（AIH-030）。
     */
    val attachmentIds: List<String> = emptyList(),
    /** 继续同一条消息的重试时带上（AIH-024） */
    val retryOfRunId: String? = null,
)

@Serializable
data class AiRunStartResponse(val runId: String, val assistantMessageId: String, val userMessageId: String?)

object AiRunRepo {
    private val log = org.slf4j.LoggerFactory.getLogger(AiRunRepo::class.java)

    private const val RUNNING = "running"
    private const val COMPLETED = "completed"
    private const val FAILED = "failed"
    private const val CANCELLED = "cancelled"

    fun create(
        conversationId: String,
        providerId: String,
        modelId: String,
        providerSnapshot: JsonObject,
        modelSnapshot: JsonObject,
        promptVersion: String,
        userMessageId: String?,
        assistantMessageId: String,
        retryOfRunId: String?,
        reasoningEffort: String? = null,
        /** 本次 Run 生效的 Skills（名称 + digest）：AIH-047 要求能追溯"当时用的是哪版规则" */
        skillSnapshot: JsonObject? = null,
    ): AiRunDto {
        val id = UUID.randomUUID().toString()
        Db.withConnection { conn ->
            conn.execute(
                """
                INSERT INTO ai_runs
                  (id, conversation_id, status, provider_id, model_id, user_message_id, assistant_message_id,
                   provider_snapshot, model_snapshot, skill_snapshot, prompt_version, retry_of_run_id,
                   reasoning_effort, started_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP(3))
                """.trimIndent(),
                id, conversationId, RUNNING, providerId, modelId, userMessageId, assistantMessageId,
                AppJson.encodeToString(JsonObject.serializer(), providerSnapshot),
                AppJson.encodeToString(JsonObject.serializer(), modelSnapshot),
                skillSnapshot?.let { AppJson.encodeToString(JsonObject.serializer(), it) },
                promptVersion, retryOfRunId, reasoningEffort,
            )
        }
        return get(id) ?: error("Run 写入后读不到: $id")
    }

    fun get(id: String): AiRunDto? = Db.withConnection { conn ->
        conn.queryOne("SELECT * FROM ai_runs WHERE id = ?", id) { it.toRun() }
    }

    fun finish(
        id: String,
        status: String,
        errorCode: String? = null,
        errorMessage: String? = null,
        usage: JsonElement? = null,
    ) {
        if (status !in setOf(COMPLETED, FAILED, CANCELLED)) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "非法的 Run 终态: $status")
        }
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_runs
                   SET status = ?, error_code = ?, error_message = ?, usage_json = ?,
                       completed_at = CURRENT_TIMESTAMP(3)
                 WHERE id = ?
                """.trimIndent(),
                status, errorCode, errorMessage,
                usage?.let { AppJson.encodeToString(JsonElement.serializer(), it) },
                id,
            )
        }
    }

    /** 进程重启后把还在 running 的 Run 标成失败：不然它们会永远"转圈"。 */
    fun failStaleRunning(reason: String = "后端重启导致 Run 中断"): Int = Db.withConnection { conn ->
        conn.execute(
            """
            UPDATE ai_runs
               SET status = ?, error_code = ?, error_message = ?, completed_at = CURRENT_TIMESTAMP(3)
             WHERE status = ?
            """.trimIndent(),
            FAILED, AiErrorCode.PROTOCOL_ERROR, reason, RUNNING,
        )
    }

    // --- 工具调用（M4 / AIH-035 / AIH-049） ---------------------------------

    /**
     * 落一条工具调用记录（审计用）。
     *
     * `approval` 与 `status` 分开记：需要审批的工具即使被拒绝也要留痕
     * （"AI 想同步历史但用户拒绝了"这种事必须能回溯）。
     */
    fun insertToolCall(rec: com.comfyhub.ai.tools.ToolCallRecord) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    """
                    INSERT INTO ai_tool_calls
                      (id, run_id, provider_call_id, name, arguments_json, approval, status,
                       result_json, content, error, elapsed_ms, started_at, completed_at)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP(3), CURRENT_TIMESTAMP(3))
                    ON DUPLICATE KEY UPDATE status = VALUES(status), result_json = VALUES(result_json),
                       content = VALUES(content), error = VALUES(error),
                       approval = VALUES(approval), elapsed_ms = VALUES(elapsed_ms),
                       completed_at = CURRENT_TIMESTAMP(3)
                    """.trimIndent(),
                    rec.id, rec.runId, rec.callId, rec.name, rec.argumentsJson.take(16_000),
                    rec.approval, rec.status,
                    rec.resultJson?.let { AppJson.encodeToString(JsonObject.serializer(), it) },
                    rec.content.take(64_000), rec.error?.take(2000), rec.elapsedMs,
                )
            }
        }.onFailure { log.warn("工具调用记录落库失败: {}", it.message) }
    }

    /** 某个 Run 用过的工具（界面回放 / 排查用）。 */
    fun listToolCalls(runId: String): List<Map<String, Any?>> = Db.withConnection { conn ->
        conn.queryList(
            "SELECT * FROM ai_tool_calls WHERE run_id = ? ORDER BY started_at, id",
            runId,
        ) { rs ->
            mapOf(
                "id" to rs.getString("id"),
                "callId" to rs.getString("provider_call_id"),
                "name" to rs.getString("name"),
                "argumentsJson" to rs.getString("arguments_json"),
                "approval" to rs.getString("approval"),
                "status" to rs.getString("status"),
                "error" to rs.getString("error"),
                "elapsedMs" to rs.getLong("elapsed_ms"),
            )
        }
    }

    // --- 待审批的调用也要留痕（M1，2026-09-23） -----------------------------
    //
    // 旧行为是"invoke 返回后才落 ai_tool_calls"，于是存在一个盲区：
    // **审批正在等用户点按钮的那几分钟里，库里什么都没有**。后端在这中间重启，
    // "AI 曾想调用什么、审批没被处理"就凭空消失 —— 既没法跨端查（另一台设备
    // 连上来不知道有东西在等它批），审计上也是空白（R8 落库防线）。
    //
    // 现在的做法：`ToolApprovalGate.open()` 的同一刻就落一行 `status='pending'`，
    // invoke 返回时**原地更新**那一行（找不到 pending 行再按老路径插入）。

    /** 审批等待开始时落一行 pending（只在 `needsApproval` 时调，R8）。 */
    fun insertPendingToolCall(runId: String, callId: String, name: String, argumentsJson: String) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    """
                    INSERT INTO ai_tool_calls
                      (id, run_id, provider_call_id, name, arguments_json, approval, status, started_at)
                    VALUES (?,?,?,?,?,'pending','pending', CURRENT_TIMESTAMP(3))
                    """.trimIndent(),
                    UUID.randomUUID().toString(), runId, callId, name, argumentsJson.take(16_000),
                )
            }
        }.onFailure { log.warn("待审批工具调用落库失败（审批期间跨端将看不到它）: {}", it.message) }
    }

    /**
     * invoke 返回后把**同一行** pending 落成终态。
     * 返回 false 表示没有 pending 行可更新（老路径 / 非审批调用）—— 调用方改走 [insertToolCall]。
     */
    private fun resolvePendingToolCall(rec: com.comfyhub.ai.tools.ToolCallRecord): Boolean = runCatching {
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_tool_calls
                   SET approval = ?, status = ?, result_json = ?, content = ?, error = ?,
                       elapsed_ms = ?, completed_at = CURRENT_TIMESTAMP(3)
                 WHERE run_id = ? AND provider_call_id = ? AND status = 'pending'
                """.trimIndent(),
                rec.approval, rec.status,
                rec.resultJson?.let { AppJson.encodeToString(JsonObject.serializer(), it) },
                rec.content.take(64_000), rec.error?.take(2000), rec.elapsedMs,
                rec.runId, rec.callId,
            ) > 0
        }
    }.getOrDefault(false)

    /** 工具调用落库的统一入口：有 pending 行就原地更新，没有就按老路径插入。 */
    fun recordToolCall(rec: com.comfyhub.ai.tools.ToolCallRecord) {
        if (resolvePendingToolCall(rec)) return
        insertToolCall(rec)
    }

    /** 现在还在等用户批准的调用（跨端查询 `GET /api/ai/pending` 用）。 */
    fun listPendingApprovals(): List<Map<String, Any?>> = runCatching {
        Db.withConnection { conn ->
            conn.queryList(
                """
                SELECT tc.provider_call_id, tc.run_id, r.conversation_id, tc.name,
                       tc.arguments_json, tc.started_at
                  FROM ai_tool_calls tc
                  JOIN ai_runs r ON r.id = tc.run_id
                 WHERE tc.status = 'pending' AND r.status = 'running'
                 ORDER BY tc.started_at, tc.id
                """.trimIndent(),
            ) { rs ->
                mapOf(
                    "callId" to rs.getString("provider_call_id"),
                    "runId" to rs.getString("run_id"),
                    "conversationId" to rs.getString("conversation_id"),
                    "name" to rs.getString("name"),
                    "argumentsJson" to rs.getString("arguments_json"),
                    "askedAt" to rs.isoTime("started_at"),
                )
            }
        }
    }.getOrDefault(emptyList())

    /**
     * 后端重启恢复：库里还挂着 pending 的审批行都是"永远不会有人点批准"的了
     * （内存里的闸门随进程消失，Run 也已被 [failStaleRunning] 标失败）。
     * 与审批闸门的既有语义一致 —— **超时 / 取消都算拒绝**，重启是最彻底的超时。
     */
    fun expireStalePendingToolCalls(): Int = runCatching {
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_tool_calls
                   SET approval = 'denied', status = 'denied',
                       error = '后端重启，审批等待被打断（按拒绝处理）',
                       completed_at = CURRENT_TIMESTAMP(3)
                 WHERE status = 'pending'
                """.trimIndent(),
            )
        }
    }.getOrDefault(0)

    // --- 事件 ---------------------------------------------------------------

    fun appendEvent(runId: String, seq: Int, type: String, payload: JsonObject) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    "INSERT IGNORE INTO ai_run_events (run_id, seq, event_type, payload_json) VALUES (?,?,?,?)",
                    runId, seq, type, AppJson.encodeToString(JsonObject.serializer(), payload),
                )
            }
        }
    }

    /**
     * **批量**写事件（P1，2026-09-23）。
     *
     * 为什么需要它：流式 Run 的 `text.delta` 是逐字来的，实测**单 Run 4.7 万个事件**，
     * 而旧写法是"每发一个事件就单独开一条连接 + 一个隐式事务 + 一次 fsync"
     * —— 4.7 万次，这正是 `ai_run_events` 涨到 62 万行 / 112.9MB 的直接原因（总纲 P1）。
     *
     * 改进有两条，**都不改变可观测行为**：
     *  1. 攒够 [batchSize] 条（或调用方主动 [flush]）**在一个事务里**写下去；
     *  2. 事件在 Run 期间**不落库也不丢** —— 它同时还在 `RunEventBus` 的内存缓冲里，
     *     SSE 订阅者拿到的是内存那份（不是这一份）。落库只为"断线重连 + 后端重启后回放"。
     *
     * ⚠ 调用方**必须在 Run 收尾时 `flush()`**（正常 / 失败 / 取消三条路都要）：
     * 否则最后那不足一批的 `run.completed` / `run.failed` 会丢，断线重连就补不回来了。
     */
    class EventBatcher(private val runId: String, private val batchSize: Int = 200) {
        private val pending = ArrayList<Triple<Int, String, JsonObject>>(batchSize)

        @Synchronized
        fun add(seq: Int, type: String, payload: JsonObject) {
            pending += Triple(seq, type, payload)
            if (pending.size >= batchSize) flushLocked()
        }

        /** 把剩下的写下去；返回本次写入的条数（0 表示没有待写）。 */
        @Synchronized
        fun flush(): Int = flushLocked()

        private fun flushLocked(): Int {
            if (pending.isEmpty()) return 0
            val batch = pending.toList()
            pending.clear()
            runCatching {
                Db.withConnection { conn ->
                    val prev = conn.autoCommit
                    conn.autoCommit = false
                    try {
                        batch.forEach { (seq, type, payload) ->
                            conn.execute(
                                "INSERT IGNORE INTO ai_run_events (run_id, seq, event_type, payload_json) VALUES (?,?,?,?)",
                                runId, seq, type, AppJson.encodeToString(JsonObject.serializer(), payload),
                            )
                        }
                        conn.commit()
                    } catch (e: Exception) {
                        runCatching { conn.rollback() }
                        throw e
                    } finally {
                        conn.autoCommit = prev
                    }
                }
            }
            return batch.size
        }
    }

    fun events(runId: String, after: Int = 0): List<RunEvent> = Db.withConnection { conn ->
        conn.queryList(
            "SELECT seq, event_type, payload_json FROM ai_run_events WHERE run_id = ? AND seq > ? ORDER BY seq",
            runId, after,
        ) { rs ->
            RunEvent(
                seq = rs.getInt("seq"),
                type = rs.getString("event_type"),
                payload = runCatching {
                    AppJson.parseToJsonElement(rs.getString("payload_json") ?: "{}") as JsonObject
                }.getOrDefault(JsonObject(emptyMap())),
            )
        }
    }

    /** 事件只用于断线续传与审计，保留有限数量即可（AIH-018 的保留策略）。 */
    fun pruneEvents(runId: String, keep: Int = 4000) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    """
                    DELETE FROM ai_run_events
                     WHERE run_id = ? AND seq <= (
                       SELECT MAX(seq) - ? FROM (SELECT seq FROM ai_run_events WHERE run_id = ?) t
                     )
                    """.trimIndent(),
                    runId, keep, runId,
                )
            }
        }
    }

    // --- 消息 ---------------------------------------------------------------

    /** 助手消息在流式期间不断被追加正文；结束时落终态（AIH-019）。 */
    fun updateMessage(
        messageId: String,
        text: String,
        status: String,
        providerResponseId: String? = null,
        usage: JsonElement? = null,
    ) {
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_messages
                   SET text = ?, status = ?, provider_response_id = COALESCE(?, provider_response_id),
                       usage_json = ?
                 WHERE id = ?
                """.trimIndent(),
                text, status, providerResponseId,
                usage?.let { AppJson.encodeToString(JsonElement.serializer(), it) },
                messageId,
            )
        }
    }

    /**
     * 重试时把**那条失败 / 取消的助手消息就地重置**成流式占位（AIH-024）。
     *
     * 为什么不新建一条：新建等于在会话里再插一个气泡，而旧的那条会作为空壳留下
     * （用户报的"重试之后同一条语句出现两次"就是这一类）。就地重置之后
     * seq 不变、空壳不残、`retryOfRunId` 也照样能追溯到是哪次失败的重放。
     */
    fun resetAssistantForRetry(messageId: String, providerId: String, modelId: String) {
        Db.tx { conn ->
            // 上一次失败前可能已经落过 part（思考 / 工具卡），一并清掉，别让它混进新回复
            conn.execute("DELETE FROM ai_message_parts WHERE message_id = ?", messageId)
            conn.execute(
                """
                UPDATE ai_messages
                   SET status = 'streaming', text = '', provider_id = ?, model_id = ?,
                       provider_response_id = NULL, usage_json = NULL
                 WHERE id = ?
                """.trimIndent(),
                providerId, modelId, messageId,
            )
        }
    }

    fun insertAssistantPlaceholder(conversationId: String, providerId: String, modelId: String): String {
        val id = UUID.randomUUID().toString()
        Db.withConnection { conn ->
            val seq = conn.queryOne(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM ai_messages WHERE conversation_id = ?",
                conversationId,
            ) { it.getInt(1) } ?: 1
            conn.execute(
                """
                INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, provider_id, model_id)
                VALUES (?,?,?,'assistant','streaming','',?,?)
                """.trimIndent(),
                id, conversationId, seq, providerId, modelId,
            )
        }
        return id
    }

    private fun ResultSet.toRun() = AiRunDto(
        id = getString("id"),
        conversationId = getString("conversation_id"),
        status = getString("status"),
        providerId = getString("provider_id"),
        modelId = getString("model_id"),
        assistantMessageId = getString("assistant_message_id"),
        userMessageId = getString("user_message_id"),
        errorCode = getString("error_code"),
        errorMessage = getString("error_message"),
        promptVersion = getString("prompt_version"),
        reasoningEffort = getString("reasoning_effort"),
        retryOfRunId = getString("retry_of_run_id"),
        startedAt = isoTime("started_at"),
        completedAt = isoTime("completed_at"),
        createdAt = isoTime("created_at"),
    )
}

internal fun jsonString(value: String?): JsonElement =
    if (value == null) JsonNull else JsonPrimitive(value)
