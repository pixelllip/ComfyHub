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

/**
 * 一次工具调用的行（`ai_tool_calls`）—— 桌面要能回答"手机那次 Run 跑过哪些工具"。
 *
 * 字段与桌面 `db/schema.sqlite.sql` 的同名表**逐列对齐**（这张表 2026-09-27 起进同步范围：
 * 上行 wire kind = `ai_tool_call`，下行就是整行落进来）。
 */
data class ToolCallRecord(
    val id: String,
    val runId: String,
    /** 模型/上游给的那个 id（`tool.requested` 事件里也是它）。 */
    val providerCallId: String,
    val name: String,
    val argumentsJson: String?,
    /** `not_required` / `pending` / `approved` / `denied`（与桌面同值）。 */
    val approval: String,
    /** `pending` / `completed` / `failed`（与桌面 `runs.rs` 同值）。 */
    val status: String,
    val resultJson: String?,
    val content: String?,
    val error: String?,
    val elapsedMs: Long,
    val startedAt: String?,
    val completedAt: String?,
    val createdAt: String,
)

/**
 * 一条下行镜像行的落库结果（[HarnessStore.applyMirrorRow]）。
 *
 * 刻意**不用布尔**：`false` 会把"被本机删除标记跳过"和"这张表我不认识"混成同一件事，
 * 而那两件事的处置完全不同（前者是正常且必须的，后者说明两端 schema 对不上）。
 */
enum class MirrorApply {
    /** 整行落库了（新增或覆盖）。 */
    WRITTEN,

    /** 被**本机删除标记**跳过（用户在这台手机上删过它 —— 见 `local_deletions`）。 */
    SKIPPED_DELETED,

    /** 不是本机认识的镜像表（桌面加了张新表而我们还没升 schema）。 */
    UNKNOWN_TABLE,
}

/**
 * 一条 outbox 记录对应的**上行实体**（[HarnessStore.outboxTarget] 的返回值）。
 *
 * [wireKind] 是**线上**的 kind（单数、与桌面表名同形：`ai_conversation` / `ai_message` /
 * `ai_message_part`）—— 刻意与本地 [OutboxKind] 分开：本地 kind 是"队列里排的是什么"，
 * 线上 kind 是"桌面往哪张表写"，两者混用就会出现"推上去说 UNKNOWN_KIND"这种哑巴故障。
 */
data class OutboxTarget(
    val wireKind: String,
    /** 完整一行（列清单来自行本身，见 `Row.asMap`）。 */
    val payload: Map<String, Any?>,
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

/**
 * 上行队列的种类（契约 §4 的 `kind`）。
 *
 * ⚠ 这里有两套命名，**别混**：
 *  - 前五个（`conversation` / `message` / `part` / `run` / `submit_job`）是**本地队列**的名字，
 *    与 wire kind（`ai_conversation` / `ai_message` / …）**不同**，映射在 [HarnessStore.outboxTarget]；
 *  - 后两个（`ai_run` / `ai_tool_call`）刻意**与 wire kind 同名** —— 它们的 payload 就是
 *    `ai_runs` / `ai_tool_calls` 的整行，没有"翻译"这一步可做（多一层翻译就多一处漂移）。
 */
object OutboxKind {
    const val CONVERSATION = "conversation"
    const val MESSAGE = "message"
    const val PART = "part"

    /**
     * ⚠ **旧常量，一个字节都不许删**（M5 的语义还挂在它上面）。
     *
     * 它是 M4 之前那个"上行通道还没有"的占位：那时 `ai_runs` 还是**本机表**、没有任何上行形状，
     * 于是先在队列里排一条 `run` 等 M5。2026-09-27 起手机自己的 Run 走 [AI_RUN]（wire kind
     * `ai_run`，桌面 `sync.rs::table_of_kind` 认它），所以**本仓再没有任何一处 enqueue 这个常量**。
     * `SyncPush` 对它的处置保持原样：**跳过但不清**（清了就等于悄悄丢弃一次要上行的东西）。
     */
    const val RUN = "run"
    const val SUBMIT_JOB = "submit_job"

    /**
     * **本机起的那次 Run** 的整行（wire kind `ai_run` → 桌面 `ai_runs`）。
     *
     * ⚠ 只对"本机起过的 Run"入队（`local_runs` 里有的）：`ai_runs` 自 2026-09-27 起是**镜像表**，
     * 桌面同步下来的 Run 也在库里 —— 把它再推回去就是一次**回声**（见 [HarnessStore.finishRun]）。
     */
    const val AI_RUN = "ai_run"

    /** **本机跑的一次工具调用**的整行（wire kind `ai_tool_call` → 桌面 `ai_tool_calls`）。 */
    const val AI_TOOL_CALL = "ai_tool_call"
}

/** 零散状态（`mobile_kv`）的键。 */
object KvKey {
    /** 同步游标（契约 §2 的 `cursor`）。 */
    const val SYNC_CURSOR = "sync.cursor"

    /**
     * 设备号（契约 §4 的 `deviceId`）。
     *
     * 首次上行时生成一次并落库 —— 生成两次会让桌面把同一台手机当成两台设备，
     * 而 `sync_clients` 是按 device_id 记游标的。
     */
    const val DEVICE_ID = "sync.deviceId"

    /** run-kit 的版本号（🔒 §3.4，每次 Run 要记进快照）。 */
    const val KIT_VERSION = "runKit.version"

    /** 桌面的 `schemaVersion`（与自己对不上就得走 bootstrap）。 */
    const val SCHEMA_VERSION = "sync.schemaVersion"
}
