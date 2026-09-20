package com.comfyhub.ai.tools

import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.resume

/**
 * 一条**待用户回答**的提问（用户建议 ⑥，2026-09-20：
 * "让 AI 像 deepseek harness 一样支持提问功能，回答结束时不中断当前对话"）。
 *
 * [options] 是给界面画按钮用的候选答案；用户也可以自己敲一段（[allowFreeText]）。
 * 两者都允许 —— 只给按钮会逼着用户在"都不太对"的选项里挑一个。
 */
data class PendingQuestion(
    val questionId: String,
    val runId: String,
    val conversationId: String,
    val question: String,
    val options: List<String>,
    val allowFreeText: Boolean,
    val askedAtMs: Long,
    /** 可选的小标题（例如「参考图」「输出尺寸」）。 */
    val header: String? = null,
    /** 用户答完之后填上（界面重连时能直接看到结论）。 */
    val answer: String? = null,
    val optionIndex: Int? = null,
) {
    val answered: Boolean get() = answer != null

    /** 发给界面的形状（SSE 事件与 HTTP 查询共用一份）。 */
    fun toJson(): JsonObject = buildJsonObject {
        put("questionId", questionId)
        put("runId", runId)
        put("conversationId", conversationId)
        put("question", question)
        put("allowFreeText", allowFreeText)
        put("answered", answered)
        put("options", buildJsonArray { options.forEach { add(JsonPrimitive(it)) } })
        put("askedAt", askedAtMs)
        header?.let { put("header", it) }
        answer?.let { put("answer", it) }
        optionIndex?.let { put("optionIndex", it) }
    }
}

/** 用户对一条提问的回答。 */
data class AskedAnswer(
    val answer: String,
    /** 用户点的是第几个选项（自己敲的为 null）。 */
    val optionIndex: Int? = null,
)

/**
 * 提问闸门（用户建议 ⑥）。
 *
 * 与 [ToolApprovalGate] 是**兄弟**，但语义不同，别混：
 *
 * | | 审批闸门 | 提问闸门 |
 * | --- | --- | --- |
 * | 用户没反应时 | 当作**拒绝**，工具不执行 | 当作**没回答**，如实告诉模型"用户没答" |
 * | 结果 | 一次允许 / 不允许 | **一段文字**，模型拿着它接着做 |
 * | Run | 停在这里等 | **不终止**，答完继续同一个 Run |
 *
 * ## 为什么要有"答完了还没人来取"这一步
 *
 * 界面把回答 POST 回来的时刻，模型那边的协程**可能正好还没走到 `await`**
 * （事件发出 → 用户秒点 → 协程调度）。那时 `resolve` 找不到等待者，
 * 用户的回答就**静默丢了** —— 而 `ask_user_question` 侧会一直等到超时。
 * 所以回答一律先落进 [answers]（一次性邮箱），`resolve` 与 `await` 都先看它。
 */
class AskGate(private val timeoutMs: Long = 30 * 60 * 1000) {
    private val log = LoggerFactory.getLogger(AskGate::class.java)
    private val pending = ConcurrentHashMap<String, CompletableFuture<AskedAnswer>>()

    /** 已经答完、还没被模型取走的回答（一次性）。 */
    private val answers = ConcurrentHashMap<String, AskedAnswer>()

    /** 挂起一条提问，等用户回答。 */
    fun open(questionId: String) {
        pending[questionId] = CompletableFuture()
    }

    fun isPending(questionId: String): Boolean =
        answers.containsKey(questionId) || pending[questionId]?.isDone == false

    /**
     * 用户回答了。返回 false 表示这条提问已经不在等待
     * （超时 / 已回答过 / Run 结束）—— 界面据此显示"这次回答没被采纳"。
     */
    fun resolve(questionId: String, answer: AskedAnswer): Boolean {
        val future = pending[questionId]
        if (future != null && !future.isDone) {
            log.info("用户回答了提问 {}（{} 字）", questionId, answer.answer.length)
            return future.complete(answer)
        }
        // 没人等着：可能是还没走到 await（正常竞态），也可能是这条提问早就超时了。
        // 前者必须接住（否则用户的话白说），后者由 isWaiting 判断。
        if (isWaiting(questionId)) {
            answers[questionId] = answer
            log.info("用户提前回答了提问 {}（模型还没开始等，先存着）", questionId)
            return true
        }
        return false
    }

    /** 这条提问还在生命周期里（还没被 clear）吗？ */
    fun isWaiting(questionId: String): Boolean = pending.containsKey(questionId)

    /**
     * 等用户回答。返回 null = 没等到（超时 / Run 取消 / 提问已被丢弃）。
     *
     * 等待期间不消耗 Run 的预算、也不阻塞事件流：它就是一个挂起的协程。
     */
    suspend fun await(questionId: String): AskedAnswer? {
        answers.remove(questionId)?.let { return it }
        val future = pending[questionId] ?: return null
        return withTimeoutOrNull(timeoutMs) {
            suspendCancellableCoroutine { cont ->
                future.whenComplete { value, _ -> if (cont.isActive) cont.resume(value) }
                cont.invokeOnCancellation { future.cancel(true) }
            }
        }
    }

    /** 这条提问处理完了：唤醒等待方并清理（Run 取消 / 超时 / 已经拿到答案）。 */
    fun discard(questionId: String) {
        pending.remove(questionId)
        answers.remove(questionId)
    }
}

/**
 * "这个 Run 现在挂着哪些提问"的登记表。
 *
 * 与 [AskGate] 分开是为了三件事：
 *  1. HTTP 层要能回答 `GET /api/ai/runs/{id}/question` —— 界面重开 / 断线重连之后
 *     得知道"还在等你回答什么"（否则那张卡片就永远消失了，Run 一直挂着）；
 *  2. `ask_user_question` 用它挡掉"同一个问题问两遍"（卡片重复弹出很烦人）；
 *  3. Run 收尾时按 runId 把挂着的提问一次放掉（否则协程要等 30 分钟才回收）。
 *
 * 内存态、**不落库**：提问是"当前这次 Run 的状态"，进程重启后这次 Run 本身就已经
 * 被标成失败了（`AiRunRepo.failStaleRunning`），不存在"重启后还有人在等你答"。
 */
class AskRegistry(private val gate: AskGate) {
    private val byRun = ConcurrentHashMap<String, MutableList<String>>()
    private val questions = ConcurrentHashMap<String, PendingQuestion>()

    @Synchronized
    fun track(question: PendingQuestion) {
        questions[question.questionId] = question
        gate.open(question.questionId)
        byRun.computeIfAbsent(question.runId) { mutableListOf() }.add(question.questionId)
    }

    fun get(questionId: String): PendingQuestion? = questions[questionId]

    /** 这个 Run 现在还在等用户回答的提问（最新的那条）。 */
    fun pendingFor(runId: String): PendingQuestion? = byRun[runId].orEmpty()
        .lastOrNull { id -> questions[id]?.let { gate.isWaiting(id) && !it.answered } == true }
        ?.let { questions[it] }

    /** 同一个 Run 里是不是已经问过一模一样的问题（挡重复卡片）。 */
    fun findSameQuestion(runId: String, question: String): PendingQuestion? =
        byRun[runId].orEmpty()
            .mapNotNull { questions[it] }
            .lastOrNull { !it.answered && it.question.trim() == question.trim() }

    /**
     * 用户回答了：记下来并唤醒等待方。
     *
     * 返回 false = 这条提问已经不在等待（早就答过 / 超时），界面要如实告诉用户
     * "这次回答没被采纳"，而不是假装成功。
     */
    @Synchronized
    fun answer(questionId: String, answer: String, optionIndex: Int?): Boolean {
        val current = questions[questionId] ?: return false
        if (current.answered) return false
        val accepted = gate.resolve(questionId, AskedAnswer(answer, optionIndex))
        if (accepted) {
            questions[questionId] = current.copy(answer = answer, optionIndex = optionIndex)
        }
        return accepted
    }

    /**
     * 模型已经拿到答案（或者提问作废）：从"还在等"的池子里摘掉。
     *
     * **保留 [questions] 里的记录**：`user.question.answered` 事件与
     * `GET /api/ai/questions/{id}` 还要靠它回答"这条问过什么、答了什么"。
     */
    fun clear(questionId: String) {
        gate.discard(questionId)
    }

    /** Run 收尾：把这个 Run 上所有还挂着的提问放掉。 */
    fun discardRun(runId: String) {
        byRun.remove(runId).orEmpty().forEach { gate.discard(it) }
    }
}
