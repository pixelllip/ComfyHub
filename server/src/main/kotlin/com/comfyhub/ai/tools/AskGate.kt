package com.comfyhub.ai.tools

import com.comfyhub.ai.QuestionStore
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
    /**
     * 同一张提问卡里的第几题（0 基）与总题数；单问时是 `0 / 1`。
     *
     * 2026-09-22（用户拍的形态，照 DeepSeek Harness）：**一张卡装多个问题、分页、
     * 可逐题跳过、最后一页按钮变「提交」、答案可为空**。同一次 `ask_user_question`
     * 调用生成的多条共享 [groupId]，界面按它把这几条画成同一张卡的 N 页。
     */
    val index: Int = 0,
    val total: Int = 1,
    /** 这张卡的分组号（单问时就是自己的 questionId）。 */
    val groupId: String = questionId,
    /** 多选题：选项可以多选（界面画复选框）。 */
    val multiSelect: Boolean = false,
    /** 多选题用户勾了哪几个（自己敲的自由文本在 [answer] 里）。 */
    val optionIndexes: List<Int> = emptyList(),
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
        put("index", index)
        put("total", total)
        put("groupId", groupId)
        put("multiSelect", multiSelect)
        header?.let { put("header", it) }
        answer?.let { put("answer", it) }
        optionIndex?.let { put("optionIndex", it) }
        if (optionIndexes.isNotEmpty()) {
            put("optionIndexes", buildJsonArray { optionIndexes.forEach { add(JsonPrimitive(it)) } })
        }
    }
}

/** 用户对一条提问的回答。 */
data class AskedAnswer(
    val answer: String,
    /** 用户点的是第几个选项（自己敲的为 null）。 */
    val optionIndex: Int? = null,
    /** 多选题勾了哪几个。 */
    val optionIndexes: List<Int> = emptyList(),
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

    /**
     * 等**一整张提问卡**（同一次调用里的多条 questionId）。
     *
     * 返回 `questionId → 回答`；**没答的（跳过的 / 界面没提交的）是 null** ——
     * 调用方要按"用户没回答"处理，而不是"这条不存在"。
     *
     * ## 为什么要"先等任意一条，再给一个短宽限窗"
     *
     * 新界面（分页卡）是一次把整张卡提交上来的，所以正常情况下几条答复几乎同时到，
     * 宽限窗用不到。但**旧界面只认第一条**（它按 `tool.requested` 的 callId 挂卡，
     * 多条事件会互相覆盖）：那时只有一条答复会来，而我们**不能**因此把 Run 挂到
     * 30 分钟超时 —— 所以任何一条答复到达后就开一个 [graceMs] 的窗口收剩下的，
     * 收不到就按"没回答"继续。宁可少答几题，也不能让用户对着一个卡死的 Run 干等。
     */
    suspend fun awaitAll(ids: List<String>, graceMs: Long = DEFAULT_GRACE_MS): Map<String, AskedAnswer?> {
        val out = LinkedHashMap<String, AskedAnswer?>()
        if (ids.isEmpty()) return out
        // 先把"已经答了、放在一次性邮箱里"的收掉（用户手快，抢在 await 之前的正常竞态）
        ids.forEach { id -> answers.remove(id)?.let { out[id] = it } }

        val waiting = ids.filter { it !in out }
        if (waiting.isNotEmpty()) {
            val got = withTimeoutOrNull(timeoutMs) {
                suspendCancellableCoroutine<Unit> { cont ->
                    val futures = waiting.mapNotNull { pending[it] }
                    if (futures.isEmpty()) {
                        cont.resume(Unit)
                        return@suspendCancellableCoroutine
                    }
                    val any = CompletableFuture.anyOf(*futures.toTypedArray())
                    any.whenComplete { _, _ -> if (cont.isActive) cont.resume(Unit) }
                    cont.invokeOnCancellation { any.cancel(true) }
                }
            }
            if (got == null) {
                // 一条都没等到（超时 / Run 取消）：剩下的如实记成"没回答"
                ids.forEach { out.putIfAbsent(it, null) }
                return out
            }
        }

        // ② 先把**已经完成的**一次收干净 —— 不管宽限窗还剩多少，它们已经有了。
        // （顺序很要紧：先按题号去等"还没答"的那题，会把窗口耗光，
        //   反过来把**已经答了的**饿成 null —— 这是实测抓到的。）
        ids.forEach { id ->
            if (out.containsKey(id)) return@forEach
            answers.remove(id)?.let { out[id] = it; return@forEach }
            pending[id]?.takeIf { it.isDone }?.getNow(null)?.let { out[id] = it }
        }

        // ③ 剩下真正还没到的，用宽限窗收（新界面几乎同时到；旧界面只有一条时在这里收口）
        val deadline = System.currentTimeMillis() + graceMs
        ids.forEach { id ->
            if (out.containsKey(id)) return@forEach
            val left = deadline - System.currentTimeMillis()
            out[id] = if (left <= 0) null else withTimeoutOrNull(left) { await(id) }
        }
        return out
    }

    /** 这条提问处理完了：唤醒等待方并清理（Run 取消 / 超时 / 已经拿到答案）。 */
    fun discard(questionId: String) {
        pending.remove(questionId)
        answers.remove(questionId)
    }

    companion object {
        /**
         * 拿到任意一条答复之后，再给其余几条多久（毫秒）。
         *
         * 20 秒足够"整张卡一次提交"的界面把剩下的送达，又不至于让旧界面（只交一题）
         * 把 Run 拖住 —— 这个数字是"降级体验"的边界，不是功能参数。
         */
        const val DEFAULT_GRACE_MS = 20_000L
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
 * ## 落库（M1，2026-09-23）
 *
 * 上面三条都只活在内存里，后端一重启就什么都没了。M1 起所有状态变迁**写穿**到
 * [QuestionStore]（真实现 `AiQuestionRepo`，即 `ai_questions` 表）：
 *
 *  - [track] 时就落一行 pending（不等 Run 收尾 —— 那个窗口正是要防的）；
 *  - [answer] 被采纳时写回答；
 *  - [clear] / [discardRun] 收尾时把还在 pending 的行标成失效（**已经 answered /
 *    skipped 的不许被覆盖**，store 侧有状态守卫）；
 *  - [get] 在内存查不到时回落查库 —— 这就是"后端重启后 pending 不丢"的查询面。
 *
 * store 传 null = 不落库（老行为，测试里用；线上由 Application 传入真实现）。
 */
class AskRegistry(private val gate: AskGate, private val store: QuestionStore? = null) {
    private val byRun = ConcurrentHashMap<String, MutableList<String>>()
    private val questions = ConcurrentHashMap<String, PendingQuestion>()

    @Synchronized
    fun track(question: PendingQuestion) {
        questions[question.questionId] = question
        gate.open(question.questionId)
        byRun.computeIfAbsent(question.runId) { mutableListOf() }.add(question.questionId)
        store?.insert(question)
    }

    /** 内存里的（活的）提问 —— 提问闸门的即时状态以它为准。 */
    fun getLive(questionId: String): PendingQuestion? = questions[questionId]

    /**
     * 按 questionId 查提问：先内存（这次进程里开出来的），查不到回落 [QuestionStore.find]
     * （后端重启后内存是空的，但"问过什么、答了什么"必须还查得到 —— M1 落库的意义所在）。
     */
    fun get(questionId: String): PendingQuestion? =
        questions[questionId] ?: store?.find(questionId)?.question

    /** 这个 Run 现在还在等用户回答的提问（最新的那条）。 */
    fun pendingFor(runId: String): PendingQuestion? = byRun[runId].orEmpty()
        .lastOrNull { id -> questions[id]?.let { gate.isWaiting(id) && !it.answered } == true }
        ?.let { questions[it] }

    /** 所有 Run 现在还在等用户回答的提问（跨端查询 `GET /api/ai/pending` 用）。 */
    fun allPending(): List<PendingQuestion> = questions.values
        .filter { gate.isWaiting(it.questionId) && !it.answered }
        .sortedBy { it.askedAtMs }

    /**
     * 这个 Run 现在还挂着的**整张卡**（同一 groupId 的那几条，按题号排）。
     *
     * 界面重开 / 断线重连时靠它把 N 页都补回来 —— 只回第一条的话，
     * 用户看到的是一张缺了几题的卡。
     */
    fun pendingGroupFor(runId: String): List<PendingQuestion> {
        val waiting = byRun[runId].orEmpty()
            .mapNotNull { questions[it] }
            .filter { gate.isWaiting(it.questionId) && !it.answered }
        val groupId = waiting.lastOrNull()?.groupId ?: return emptyList()
        return groupBy(groupId, waiting)
    }

    /** 一次提问里的所有条目（含已答的），按题号排 —— 历史卡片回放要用。 */
    fun groupBy(groupId: String, all: List<PendingQuestion>): List<PendingQuestion> {
        val mine = all.filter { it.groupId == groupId }
        return (mine.ifEmpty { all }).sortedBy { it.index }
    }

    /** 同一次提问里的全部条目（含已答的）。 */
    fun group(groupId: String): List<PendingQuestion> =
        groupBy(groupId, questions.values.filter { it.groupId == groupId })

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
    fun answer(questionId: String, answer: String, optionIndex: Int?, optionIndexes: List<Int> = emptyList()): Boolean {
        val current = questions[questionId] ?: return false
        if (current.answered) return false
        val accepted = gate.resolve(questionId, AskedAnswer(answer, optionIndex, optionIndexes))
        if (accepted) {
            questions[questionId] = current.copy(
                answer = answer,
                optionIndex = optionIndex,
                optionIndexes = optionIndexes,
            )
            store?.recordAnswer(questionId, answer, optionIndex, optionIndexes)
        }
        return accepted
    }

    /**
     * 模型已经拿到答案（或者提问作废）：从"还在等"的池子里摘掉。
     *
     * **保留 [questions] 里的记录**：`user.question.answered` 事件与
     * `GET /api/ai/questions/{id}` 还要靠它回答"这条问过什么、答了什么"。
     *
     * [storeStatus] 给了就把库里还挂着的 pending 行按它收尾（skip 传 "skipped"、
     * Run 收尾传 "expired"）—— 只动 pending，已 answered 的不受影响。
     */
    fun clear(questionId: String, storeStatus: String? = null) {
        gate.discard(questionId)
        if (storeStatus != null) store?.finalizeIfPending(questionId, storeStatus)
    }

    /** Run 收尾：把这个 Run 上所有还挂着的提问放掉，库里按失效收尾。 */
    fun discardRun(runId: String) {
        byRun.remove(runId).orEmpty().forEach { gate.discard(it) }
        store?.finalizeRun(runId, "expired")
    }
}
