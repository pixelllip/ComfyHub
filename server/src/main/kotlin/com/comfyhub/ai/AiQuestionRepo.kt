package com.comfyhub.ai

import com.comfyhub.AppJson
import com.comfyhub.Db
import com.comfyhub.boolOr
import com.comfyhub.execute
import com.comfyhub.intOrNull
import com.comfyhub.isoTime
import com.comfyhub.queryOne
import com.comfyhub.ai.tools.PendingQuestion
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.sql.ResultSet
import java.sql.Timestamp
import java.time.Instant

/**
 * 提问闸门的落库（M1，2026-09-23）。
 *
 * 为什么要有这张表：提问卡的历史形状本来就会随 tool_result 落进
 * `ai_message_parts` / `ai_tool_calls`，但那是**Run 收尾之后**才写的 ——
 * 在"提问已发出、工具还没返回"的窗口里（用户正在回答 / 模型正在等），
 * 后端重启会让这一切凭空消失：`GET /api/ai/questions/{id}` 只能 404，
 * 跨端也没有任何地方能查出"现在有哪些卡在等人"。
 *
 * 这张表就是那条缝的填补（总纲 M1「AskGate 落库 + 可跨端查询 + 超时恢复」）：
 *
 *  - **track 时就落**（不等 Run 收尾），回答 / 跳过 / 收尾时更新状态；
 *  - 状态机只有四态：`pending → answered / skipped / expired`。
 *    超时、Run 结束、后端重启都算 `expired`（"没有回答就失效"，语义一致，
 *    不去区分"超时还是取消" —— 那个区别在事件流里已经如实记过了）；
 *  - Run 被删除时随 `ai_runs` 级联删除（提问卡是 Run 的附属物）。
 *
 * ⚠ **不进同步范围**：手机看实时提问走 `GET /api/ai/pending`，历史卡片从
 * parts 还原。别把它加进 `sqlite-schema-contract.md` §6 的同步真源表。
 *
 * 所有写操作都包在 runCatching 里：**库坏了不能把正在等回答的 Run 弄死**
 —— 落库失败只降级为"跨端查不到"，闸门本身还在内存里照常工作。
 */
object AiQuestionRepo : QuestionStore {
    private val log = LoggerFactory.getLogger(AiQuestionRepo::class.java)

    override fun insert(q: PendingQuestion) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    """
                    INSERT INTO ai_questions
                      (question_id, run_id, conversation_id, group_id, card_index, total,
                       question, options_json, allow_free_text, multi_select, header,
                       status, asked_at)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)
                    """.trimIndent(),
                    q.questionId, q.runId, q.conversationId, q.groupId, q.index, q.total,
                    q.question,
                    AppJson.encodeToString(JsonArray.serializer(), JsonArray(q.options.map { JsonPrimitive(it) })),
                    q.allowFreeText, q.multiSelect, q.header,
                    "pending",
                    Timestamp.from(Instant.ofEpochMilli(q.askedAtMs)),
                )
            }
        }.onFailure { log.warn("提问落库失败（跨端查询将看不到这张卡）: {}", it.message) }
    }

    override fun recordAnswer(
        questionId: String,
        answer: String,
        optionIndex: Int?,
        optionIndexes: List<Int>,
        status: String,
    ) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    """
                    UPDATE ai_questions
                       SET status = ?, answer = ?, option_index = ?, option_indexes = ?,
                           answered_at = CURRENT_TIMESTAMP(3)
                     WHERE question_id = ?
                    """.trimIndent(),
                    status, answer, optionIndex,
                    AppJson.encodeToString(
                        JsonArray.serializer(),
                        JsonArray(optionIndexes.map { JsonPrimitive(it) }),
                    ),
                    questionId,
                )
            }
        }.onFailure { log.warn("提问回答落库失败: {}", it.message) }
    }

    /** 只把**还在 pending** 的行收尾：已经 answered / skipped 的不许被覆盖（状态只能前进）。 */
    override fun finalizeIfPending(questionId: String, status: String) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    "UPDATE ai_questions SET status = ?, answered_at = CURRENT_TIMESTAMP(3) WHERE question_id = ? AND status = 'pending'",
                    status, questionId,
                )
            }
        }.onFailure { log.warn("提问收尾落库失败: {}", it.message) }
    }

    override fun finalizeRun(runId: String, status: String) {
        runCatching {
            Db.withConnection { conn ->
                conn.execute(
                    "UPDATE ai_questions SET status = ?, answered_at = CURRENT_TIMESTAMP(3) WHERE run_id = ? AND status = 'pending'",
                    status, runId,
                )
            }
        }.onFailure { log.warn("提问按 Run 收尾失败: {}", it.message) }
    }

    override fun find(questionId: String): StoredQuestion? = runCatching {
        Db.withConnection { conn ->
            conn.queryOne(
                "SELECT * FROM ai_questions WHERE question_id = ?",
                questionId,
            ) { it.toStored() }
        }
    }.getOrNull()

    /**
     * 后端重启恢复（M1「超时恢复」的一半另一半）：内存里的闸门随进程消失，
     * 库里还标着 pending 的行都是"再也不会有人来回答"的了 —— 全部按失效收尾。
     * 调用点在 [com.comfyhub.Application] 里、`failStaleRunning` 之后。
     */
    fun expireStalePending(): Int = runCatching {
        Db.withConnection { conn ->
            conn.execute(
                "UPDATE ai_questions SET status = 'expired', answered_at = CURRENT_TIMESTAMP(3) WHERE status = 'pending'",
            )
        }
    }.getOrDefault(0)

    private fun ResultSet.toStored(): StoredQuestion {
        val q = PendingQuestion(
            questionId = getString("question_id"),
            runId = getString("run_id"),
            conversationId = getString("conversation_id"),
            question = getString("question") ?: "",
            options = AppJson.parseStringArray(getString("options_json")),
            allowFreeText = boolOr("allow_free_text", true),
            askedAtMs = getTimestamp("asked_at")?.toInstant()?.toEpochMilli() ?: 0L,
            header = getString("header"),
            answer = getString("answer"),
            optionIndex = intOrNull("option_index"),
            index = getInt("card_index"),
            total = getInt("total"),
            groupId = getString("group_id") ?: getString("question_id"),
            multiSelect = boolOr("multi_select"),
            optionIndexes = AppJson.parseStringArray(getString("option_indexes"))
                .mapNotNull { it.toIntOrNull() },
        )
        return StoredQuestion(
            question = q,
            status = getString("status") ?: "expired",
            answeredAt = isoTime("answered_at"),
        )
    }
}

/** 一条提问的落库形状：[question] 是卡的内容，[status] 是它的结局。 */
data class StoredQuestion(
    val question: PendingQuestion,
    val status: String,
    val answeredAt: String? = null,
) {
    /** 给 `GET /api/ai/questions/{id}` 的回落响应：在原形状上补 status / answeredAt。 */
    fun toJson(): kotlinx.serialization.json.JsonObject = buildJsonObject {
        question.toJson().forEach { (k, v) -> put(k, v) }
        put("status", status)
        answeredAt?.let { put("answeredAt", it) }
    }
}

/**
 * 提问持久化的接缝（M1）。
 *
 * [AskRegistry] 只依赖这个接口 —— 单测用内存实现就能把"落库 + 重启不丢"的语义钉死
 （Gradle 测试不连数据库是本工程的既有约定，见 `AiSeedCatalogTest` 的说明），
 真正的 SQL 实现是 [AiQuestionRepo]。
 */
interface QuestionStore {
    /** 提问卡开出来时落一行（status = pending）。 */
    fun insert(q: PendingQuestion)

    /** 用户回答 / 跳过：写终值。 */
    fun recordAnswer(
        questionId: String,
        answer: String,
        optionIndex: Int?,
        optionIndexes: List<Int> = emptyList(),
        status: String = "answered",
    )

    /** 只把还在 pending 的行收尾（answered / skipped 的不许被覆盖）。 */
    fun finalizeIfPending(questionId: String, status: String)

    /** 按 Run 收尾：这个 Run 上所有还挂着的 pending 行一次放掉。 */
    fun finalizeRun(runId: String, status: String)

    /** 按 questionId 查（重启后内存为空，回落查库用）。 */
    fun find(questionId: String): StoredQuestion?
}
