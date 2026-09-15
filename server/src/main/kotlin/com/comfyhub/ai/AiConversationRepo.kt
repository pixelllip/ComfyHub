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
import java.sql.ResultSet
import java.util.UUID

/**
 * AI 会话与消息持久化（AIH-018 / AIH-019）。
 *
 * 消息不是"一段纯文本"：正文、附件、工具调用、工具结果按 [AiMessagePartDto] 有序块保存，
 * 这样重开 App 能无损恢复，也方便以后把工具卡直接渲染出来。
 */

@Serializable
data class AiConversationDto(
    val id: String,
    val title: String,
    val providerId: String? = null,
    val modelId: String? = null,
    val systemPromptVersion: String = "v1",
    val archived: Boolean = false,
    val messageCount: Int = 0,
    val createdAt: String? = null,
    val updatedAt: String? = null,
)

@Serializable
data class AiConversationCreate(
    val title: String? = null,
    val providerId: String? = null,
    val modelId: String? = null,
)

@Serializable
data class AiConversationPatch(
    val title: String? = null,
    val providerId: String? = null,
    val modelId: String? = null,
    val archived: Boolean? = null,
)

@Serializable
data class AiMessagePartDto(
    /** text / reasoning / attachment / tool_call / tool_result */
    val type: String,
    val text: String? = null,
    val attachmentId: String? = null,
    val toolCallId: String? = null,
    val jsonPayload: JsonElement? = null,
)

@Serializable
data class AiMessageDto(
    val id: String,
    val conversationId: String,
    val seq: Int,
    /** user / assistant / tool / system_note */
    val role: String,
    /** pending / streaming / complete / failed / cancelled */
    val status: String,
    val text: String,
    val providerId: String? = null,
    val modelId: String? = null,
    val providerResponseId: String? = null,
    val usage: JsonElement? = null,
    val parts: List<AiMessagePartDto> = emptyList(),
    val createdAt: String? = null,
    val updatedAt: String? = null,
)

@Serializable
data class AiMessageAppend(
    val role: String,
    val text: String = "",
    val status: String = "complete",
    val providerId: String? = null,
    val modelId: String? = null,
    val id: String? = null,
    val parts: List<AiMessagePartDto> = emptyList(),
)

object AiConversationRepo {
    private val ROLES = setOf("user", "assistant", "tool", "system_note")
    private val STATUSES = setOf("pending", "streaming", "complete", "failed", "cancelled")

    // -----------------------------------------------------------------------
    //  会话
    // -----------------------------------------------------------------------

    fun list(includeArchived: Boolean = false, limit: Int = 200): List<AiConversationDto> = Db.withConnection { conn ->
        conn.queryList(
            """
            SELECT c.*, (SELECT COUNT(*) FROM ai_messages m WHERE m.conversation_id = c.id) AS message_count
              FROM ai_conversations c
             WHERE (? = 1 OR c.archived = 0)
             ORDER BY c.updated_at DESC
             LIMIT ?
            """.trimIndent(),
            includeArchived, limit.coerceIn(1, 500)
        ) { it.toConversation() }
    }

    fun get(id: String): AiConversationDto? = Db.withConnection { conn ->
        conn.queryOne(
            """
            SELECT c.*, (SELECT COUNT(*) FROM ai_messages m WHERE m.conversation_id = c.id) AS message_count
              FROM ai_conversations c WHERE c.id = ?
            """.trimIndent(),
            id
        ) { it.toConversation() }
    }

    fun create(req: AiConversationCreate): AiConversationDto {
        val id = UUID.randomUUID().toString()
        val title = (req.title?.trim().takeUnless { it.isNullOrEmpty() }) ?: "新对话"
        Db.withConnection { conn ->
            conn.execute(
                "INSERT INTO ai_conversations (id, title, provider_id, model_id) VALUES (?, ?, ?, ?)",
                id, title.take(255), req.providerId, req.modelId
            )
        }
        return get(id) ?: error("会话写入后读不到: $id")
    }

    fun patch(id: String, req: AiConversationPatch): AiConversationDto {
        val current = get(id) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $id")
        val title = req.title?.trim()?.takeIf { it.isNotEmpty() }?.take(255) ?: current.title
        val providerId = req.providerId ?: current.providerId
        val modelId = req.modelId ?: current.modelId
        val archived = req.archived ?: current.archived
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_conversations
                   SET title = ?, provider_id = ?, model_id = ?, archived = ?
                 WHERE id = ?
                """.trimIndent(),
                title, providerId, modelId, archived, id
            )
        }
        return get(id) ?: error("会话更新后读不到: $id")
    }

    /** 删除会话：消息与有序块由外键级联清理（AIH-018）。 */
    fun delete(id: String): Boolean = Db.withConnection { conn ->
        conn.execute("DELETE FROM ai_conversations WHERE id = ?", id) > 0
    }

    // -----------------------------------------------------------------------
    //  消息
    // -----------------------------------------------------------------------

    fun listMessages(conversationId: String): List<AiMessageDto> = Db.withConnection { conn ->
        val messages = conn.queryList(
            "SELECT * FROM ai_messages WHERE conversation_id = ? ORDER BY seq, created_at",
            conversationId
        ) { it.toMessage() }
        if (messages.isEmpty()) return@withConnection emptyList()

        val parts = conn.queryList(
            """
            SELECT p.* FROM ai_message_parts p
              JOIN ai_messages m ON m.id = p.message_id
             WHERE m.conversation_id = ?
             ORDER BY p.message_id, p.ordinal
            """.trimIndent(),
            conversationId
        ) { rs ->
            Triple(
                rs.getString("message_id"),
                rs.getInt("ordinal"),
                AiMessagePartDto(
                    type = rs.getString("type"),
                    text = rs.getString("text"),
                    attachmentId = rs.getString("attachment_id"),
                    toolCallId = rs.getString("tool_call_id"),
                    jsonPayload = rs.getString("json_payload")?.let { raw ->
                        runCatching { AppJson.parseToJsonElement(raw) }.getOrNull()
                    },
                )
            )
        }.groupBy({ it.first }, { it.third })

        messages.map { it.copy(parts = parts[it.id].orEmpty()) }
    }

    /**
     * 追加一条消息（可带有序块）。同一个事务里写入，避免出现"有消息没片段"的半截状态。
     * `seq` 由服务端分配，保证恢复顺序稳定。
     */
    fun appendMessage(conversationId: String, req: AiMessageAppend): AiMessageDto {
        if (req.role !in ROLES) throw AiException(AiErrorCode.CONFIG_ERROR, "非法角色: ${req.role}")
        if (req.status !in STATUSES) throw AiException(AiErrorCode.CONFIG_ERROR, "非法状态: ${req.status}")
        get(conversationId) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $conversationId")

        val id = req.id ?: UUID.randomUUID().toString()
        Db.tx { conn ->
            val seq = conn.queryOne(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM ai_messages WHERE conversation_id = ?",
                conversationId
            ) { it.getInt(1) } ?: 1

            conn.execute(
                """
                INSERT INTO ai_messages
                  (id, conversation_id, seq, role, status, text, provider_id, model_id)
                VALUES (?,?,?,?,?,?,?,?)
                """.trimIndent(),
                id, conversationId, seq, req.role, req.status, req.text, req.providerId, req.modelId
            )
            req.parts.forEachIndexed { index, part ->
                conn.execute(
                    """
                    INSERT INTO ai_message_parts
                      (message_id, ordinal, type, text, attachment_id, tool_call_id, json_payload)
                    VALUES (?,?,?,?,?,?,?)
                    """.trimIndent(),
                    id, index, part.type, part.text, part.attachmentId, part.toolCallId,
                    part.jsonPayload?.let { AppJson.encodeToString(JsonElement.serializer(), it) }
                )
            }
            // 会话列表按 updated_at 排序；发消息必须把它顶上去
            conn.execute("UPDATE ai_conversations SET updated_at = CURRENT_TIMESTAMP(3) WHERE id = ?", conversationId)
        }
        return listMessages(conversationId).first { it.id == id }
    }

    // -----------------------------------------------------------------------

    private fun ResultSet.toConversation() = AiConversationDto(
        id = getString("id"),
        title = getString("title") ?: "新对话",
        providerId = getString("provider_id"),
        modelId = getString("model_id"),
        systemPromptVersion = getString("system_prompt_version") ?: "v1",
        archived = getBoolean("archived"),
        messageCount = runCatching { getInt("message_count") }.getOrDefault(0),
        createdAt = isoTime("created_at"),
        updatedAt = isoTime("updated_at"),
    )

    private fun ResultSet.toMessage() = AiMessageDto(
        id = getString("id"),
        conversationId = getString("conversation_id"),
        seq = getInt("seq"),
        role = getString("role"),
        status = getString("status") ?: "complete",
        text = getString("text") ?: "",
        providerId = getString("provider_id"),
        modelId = getString("model_id"),
        providerResponseId = getString("provider_response_id"),
        usage = getString("usage_json")?.let { runCatching { AppJson.parseToJsonElement(it) }.getOrNull() },
        createdAt = isoTime("created_at"),
        updatedAt = isoTime("updated_at"),
    )
}

/** 供 `jsonPayload` 序列化使用；与 [JsonObject] 保持同一依赖，避免调用方再多引一个类型。 */
internal val JsonPayloadSerializer = JsonElement.serializer()
