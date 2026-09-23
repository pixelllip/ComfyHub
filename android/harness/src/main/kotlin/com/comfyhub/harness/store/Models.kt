package com.comfyhub.harness.store

/**
 * 本地库的行模型（刻意都是扁平 data class，不做 `@Serializable` 代码生成 —— 见 build.gradle.kts 的说明）。
 *
 * 字段名与桌面 DTO **逐字对齐**（`ai_conversations` / `ai_messages` / `ai_message_parts` / `ai_runs`）：
 * 手机侧的会话上行回桌面时，两边必须是同一套字段，否则 M4 的 apply 只能靠翻译，
 * 而翻译就是漂移的开始。
 */
data class ConversationRecord(
    val id: String,
    val title: String,
    val providerId: String?,
    val modelId: String?,
    val systemPromptVersion: String,
    val archived: Boolean,
    val createdAt: String,
    val updatedAt: String,
)

data class MessageRecord(
    val id: String,
    val conversationId: String,
    val seq: Int,
    val role: String,
    val status: String,
    val text: String?,
    val providerId: String?,
    val modelId: String?,
    val providerResponseId: String?,
    val usageJson: String?,
    val createdAt: String,
    val updatedAt: String,
)

data class MessagePartRecord(
    val messageId: String,
    val ordinal: Int,
    val type: String,
    val text: String?,
    val toolCallId: String?,
    val jsonPayload: String?,
)

data class RunRecord(
    val id: String,
    val conversationId: String,
    val status: String,
    val providerId: String?,
    val modelId: String?,
    val userMessageId: String?,
    val assistantMessageId: String?,
    val promptVersion: String?,
    val reasoningEffort: String?,
    val errorCode: String?,
    val errorMessage: String?,
    val startedAt: String?,
    val completedAt: String?,
    val createdAt: String,
    val updatedAt: String,
)

data class OutboxRecord(
    val id: String,
    val kind: String,
    val refId: String,
    val payload: String,
    val createdAt: String,
    val tries: Int,
    val lastError: String?,
)

/** Run 状态：与桌面 `AiRunRepo` 的四个常量同值（决策/审计两边要能对上）。 */
object RunStatus {
    const val RUNNING = "running"
    const val COMPLETED = "completed"
    const val FAILED = "failed"
    const val CANCELLED = "cancelled"

    /** 终态（除它之外都是"没跑完"，崩溃恢复要收尾）。 */
    fun isTerminal(status: String): Boolean = status == COMPLETED || status == FAILED || status == CANCELLED
}

/** 消息状态：与桌面 `AiConversationRepo.STATUSES` 同值。 */
object MessageStatus {
    const val PENDING = "pending"
    const val STREAMING = "streaming"
    const val COMPLETE = "complete"
    const val FAILED = "failed"
    const val CANCELLED = "cancelled"
}

/** 有序块的类型：与桌面 `HarnessRunner` 落块时用的字符串同值。 */
object PartType {
    const val TEXT = "text"
    const val REASONING = "reasoning"
    const val TOOL_CALL = "tool_call"
    const val TOOL_RESULT = "tool_result"
    const val ATTACHMENT = "attachment"
}

/** 上行队列的种类（契约 §4 的 `kind`）。 */
object OutboxKind {
    const val CONVERSATION = "conversation"
    const val MESSAGE = "message"
    const val PART = "part"
    const val RUN = "run"
    const val SUBMIT_JOB = "submit_job"
}

/** 零散状态（`mobile_kv`）的键。 */
object KvKey {
    /** 同步游标（契约 §2 的 `cursor`）。 */
    const val SYNC_CURSOR = "sync.cursor"

    /** 设备号（契约 §4 的 `deviceId`，配对时生成一次）。 */
    const val DEVICE_ID = "sync.deviceId"

    /** run-kit 的版本号（🔒 §3.4，每次 Run 要记进快照）。 */
    const val KIT_VERSION = "runKit.version"

    /** 桌面的 `schemaVersion`（与自己对不上就得走 bootstrap）。 */
    const val SCHEMA_VERSION = "sync.schemaVersion"
}
