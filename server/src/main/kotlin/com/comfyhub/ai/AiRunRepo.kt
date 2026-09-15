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
    /** 继续同一条消息的重试时带上（AIH-024） */
    val retryOfRunId: String? = null,
)

@Serializable
data class AiRunStartResponse(val runId: String, val assistantMessageId: String, val userMessageId: String?)

object AiRunRepo {
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
    ): AiRunDto {
        val id = UUID.randomUUID().toString()
        Db.withConnection { conn ->
            conn.execute(
                """
                INSERT INTO ai_runs
                  (id, conversation_id, status, provider_id, model_id, user_message_id, assistant_message_id,
                   provider_snapshot, model_snapshot, prompt_version, retry_of_run_id, started_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP(3))
                """.trimIndent(),
                id, conversationId, RUNNING, providerId, modelId, userMessageId, assistantMessageId,
                AppJson.encodeToString(JsonObject.serializer(), providerSnapshot),
                AppJson.encodeToString(JsonObject.serializer(), modelSnapshot),
                promptVersion, retryOfRunId,
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
        retryOfRunId = getString("retry_of_run_id"),
        startedAt = isoTime("started_at"),
        completedAt = isoTime("completed_at"),
        createdAt = isoTime("created_at"),
    )
}

internal fun jsonString(value: String?): JsonElement =
    if (value == null) JsonNull else JsonPrimitive(value)
