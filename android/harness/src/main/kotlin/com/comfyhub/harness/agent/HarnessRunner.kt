package com.comfyhub.harness.agent

import com.comfyhub.harness.event.RunEvent
import com.comfyhub.harness.event.RunEventBus
import com.comfyhub.harness.event.RunEventType
import com.comfyhub.harness.event.payload
import com.comfyhub.harness.kit.RunKit
import com.comfyhub.harness.kit.RunKitGuard
import com.comfyhub.harness.kit.KitStatus
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.protocol.Adapters
import com.comfyhub.harness.protocol.AiApiRef
import com.comfyhub.harness.protocol.ChatTurn
import com.comfyhub.harness.protocol.ReasoningEffort
import com.comfyhub.harness.protocol.ReasoningRequest
import com.comfyhub.harness.protocol.StreamEvent
import com.comfyhub.harness.protocol.ThinkingFormat
import com.comfyhub.harness.protocol.ToolCallRef
import com.comfyhub.harness.protocol.ToolSpec
import com.comfyhub.harness.store.ConversationRecord
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.MessageStatus
import com.comfyhub.harness.store.PartType
import com.comfyhub.harness.store.RunStatus
import com.comfyhub.harness.store.Timestamps
import com.comfyhub.harness.text.TitleStripper
import com.comfyhub.harness.tools.HarnessFiles
import com.comfyhub.harness.tools.MobileAskGate
import com.comfyhub.harness.tools.MobileToolRegistry
import com.comfyhub.harness.tools.ToolCallRequest
import com.comfyhub.harness.tools.ToolContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.put
import java.util.concurrent.ConcurrentHashMap

/**
 * 一次 Run 的输入。
 *
 * ⚠ Provider 的 **baseUrl / api / secret 由调用方给**（不是从 kit 里读）：
 * kit 的 `models[]` **刻意不带 base_url 与凭据引用**（契约 §5：手机拿了没用还多漏一样）。
 * M6（凭据）落地后由平台通道从设置里注入；在那之前，界面必须显式给。
 */
data class RunRequest(
    val conversationId: String,
    val providerId: String,
    val modelId: String,
    val api: AiApiRef,
    val baseUrl: String,
    val credentialRef: String? = null,
    /** 本次 Run 用的规则工件（**必须来自 run-kit**，R13）。 */
    val kit: RunKit,
    val userText: String,
    val reasoningEffort: ReasoningEffort? = null,
    val maxSteps: Int = 8,
    val maxContinuations: Int = 2,
    /**
     * 调用方（平台通道 / 界面）**自己指定**的消息主键。
     *
     * 为什么要有这两个字段：界面在"发出请求"的同一刻就会乐观插一条用户气泡 + 一条空的
     * 助手流式气泡，随后的 `text.delta` 事件要靠 `messageId` **找到**那条气泡往上写。
     * 所以这里必须用调用方给的 id 落库，否则界面手里那个 id 与库里对不上，
     * 表现是"事件一条不少，界面上却一个字都不长"。
     * 传 null 就自己生成（单测与"不用乐观渲染"的调用方）。
     */
    val userMessageId: String? = null,
    val assistantMessageId: String? = null,
)

/**
 * [HarnessRunner.start] 的返回值：一次 Run 的**全部句柄**。
 *
 * 只给 `runId` 是不够的 —— 界面还要拿 [assistantMessageId] 去订阅 / 关联重试
 * （桌面侧对应 `AiRunStart.assistantMessageId`），拿 [userMessageId] 去认自己乐观插的那条。
 */
data class StartedRun(
    val runId: String,
    val conversationId: String,
    val userMessageId: String,
    val assistantMessageId: String,
)

/** 一次 Run 的结果摘要（给平台通道回话用）。 */
data class RunOutcome(
    val runId: String,
    val assistantMessageId: String,
    val status: String,
    val text: String,
    val errorCode: String? = null,
)

/**
 * 手机端 harness 的**主循环**（M3 的核心）。
 *
 * 结构照搬桌面 `HarnessRunner`，只保留手机能履行的那部分：
 * 上游流式调用 → 事件流（同一套 RunEvent 协议）→ 工具循环 → 本地落库 → **R6 的三条失败纪律**。
 *
 * ```
 * run.started → message.started
 *   ┌─ 每一轮：拼上下文 → 流式收（text.delta / reasoning.delta / tool 分片）
 *   │    有工具调用 → tool.requested/started/completed|failed → 结果回填 → 下一轮
 *   └─ 没有工具调用 → 收口
 * message.completed → run.completed（失败/取消则 run.failed / run.cancelled）
 * ```
 *
 * **R6：失败/取消时"已经说出去的话一个字都不能丢"** —— 三条纪律都在 [rescueAlreadySaid]：
 *  ① 正文必须落块（`PartSink.flushInflight`）；② 标题缓冲要 flush；③ 正文绝不抹成空串。
 */
class HarnessRunner(
    private val store: HarnessStore,
    val bus: RunEventBus = RunEventBus(),
    private val tools: MobileToolRegistry,
    private val transport: HttpTransport,
    private val files: HarnessFiles,
    private val askGate: MobileAskGate = MobileAskGate(),
    /** `credential_ref` → 密钥（Android Keystore 实现由平台通道注入；M6）。 */
    private val resolveSecret: (String?) -> String? = { null },
    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
    private val upstream: UpstreamClient = UpstreamClient(transport),
) {

    private val jobs = ConcurrentHashMap<String, Job>()

    /** 库里每条消息保留多少事件（与桌面 `HarnessRunner.EVENT_KEEP` / 手机 store 同值，R5）。 */
    private val eventKeep = HarnessStore.EVENT_KEEP

    // -----------------------------------------------------------------------
    //  生命周期
    // -----------------------------------------------------------------------

    /**
     * 后台起一次 Run（平台通道用这个；事件从 [bus] 走 EventChannel）。
     *
     * 返回 [StartedRun]（不是光一个 `runId`）：界面在同一刻就要用
     * `conversationId` + 两个 `messageId` 去认自己乐观插的那些气泡。
     * 建库与 `bus.open` 都在返回**之前**完成 —— 所以 Dart 拿到返回值再订阅，
     * 一定来得及收到第一个事件。
     */
    fun start(request: RunRequest): StartedRun {
        val prepared = prepare(request)
        jobs[prepared.runId] = scope.launch { execute(prepared) }
        return StartedRun(
            runId = prepared.runId,
            conversationId = prepared.request.conversationId,
            userMessageId = prepared.userMessageId,
            assistantMessageId = prepared.assistantMessageId,
        )
    }

    /**
     * 前台跑完一次 Run（单测与"同步等结果"的调用方用这个）。
     *
     * 返回值是 [RunOutcome]，**失败也算正常返回**（不抛）：失败的事实写在事件流与库里，
     * 抛异常只会让调用方多一层没用的 try/catch。
     */
    suspend fun run(request: RunRequest): RunOutcome {
        val prepared = prepare(request)
        return execute(prepared)
    }

    fun cancel(runId: String): Boolean {
        val job = jobs[runId] ?: return false
        job.cancel()
        return true
    }

    fun isRunning(runId: String): Boolean = jobs[runId]?.isActive == true

    fun shutdown() {
        scope.coroutineContext[Job]?.cancelChildren()
        jobs.clear()
    }

    /** 待回答的提问（界面答完之后调 [MobileAskGate.answer]）。 */
    fun answerQuestion(questionId: String, text: String, optionIndex: Int? = null): Boolean =
        askGate.answer(questionId, text, optionIndex)

    // -----------------------------------------------------------------------
    //  建会话 / 建 Run（写库与事件都在这里，execute 只管跑）
    // -----------------------------------------------------------------------

    private data class Prepared(
        val runId: String,
        val request: RunRequest,
        val userMessageId: String,
        val assistantMessageId: String,
    )

    private fun prepare(request: RunRequest): Prepared {
        // 会话不存在就按**调用方给的 id** 建（不是另起一个，否则界面拿的 id 与库里对不上）
        val conversation = store.getConversation(request.conversationId)
            ?: store.createConversation(
                id = request.conversationId,
                providerId = request.providerId,
                modelId = request.modelId,
                promptVersion = request.kit.promptVersion,
            )

        // 用户轮先落库（它是历史的一部分），再建助手占位（状态 streaming）。
        // 两条都用**调用方给的 id**（给了的话）：界面已经按这两个 id 插好气泡了
        val user = store.appendMessage(
            conversationId = conversation.id,
            role = "user",
            text = request.userText,
            id = request.userMessageId ?: Timestamps.newId(),
        )
        val assistant = store.appendMessage(
            conversationId = conversation.id,
            role = "assistant",
            text = null,
            status = MessageStatus.STREAMING,
            providerId = request.providerId,
            modelId = request.modelId,
            id = request.assistantMessageId ?: Timestamps.newId(),
        )
        val run = store.createRun(
            conversationId = conversation.id,
            providerId = request.providerId,
            modelId = request.modelId,
            userMessageId = user.id,
            assistantMessageId = assistant.id,
            promptVersion = request.kit.promptVersion,
            reasoningEffort = request.reasoningEffort?.wire,
        )
        // 事件总线**在这里就 open**（不是等 execute 跑起来）：
        // 平台通道那侧 `startRun` 一返回，Dart 立刻就会订阅这个 runId ——
        // 若总线还没开，那次订阅会落到"从库里回放"的分支上，于是**一个实时事件都收不到**。
        bus.open(run.id)
        return Prepared(run.id, request.copy(conversationId = conversation.id), user.id, assistant.id)
    }

    // -----------------------------------------------------------------------
    //  主循环
    // -----------------------------------------------------------------------

    private suspend fun execute(prepared: Prepared): RunOutcome = withContext(Dispatchers.Default) {
        val runId = prepared.runId
        val request = prepared.request
        val assistantId = prepared.assistantMessageId
        val conversationId = request.conversationId

        val events = store.EventBatcher(runId)
        fun emit(type: String, body: JsonObject) {
            val event: RunEvent = bus.emit(runId, type, body) ?: return
            events.add(event.seq, type, event.payload.toString())
        }

        // 已说出的正文 / 思考：**声明在 try 外面**，失败分支要用它把已经产生的部分保留下来
        val text = StringBuilder()
        val reasoning = StringBuilder()

        // 有序块落库器（同样声明在 try 外面）：失败 / 取消时要能补落"这一轮已经流出去、
        // 但还没落成 part"的那一段
        val parts = PartSink(store, assistantId)

        // 标题摘取状态机：按 Run 建，但**声明在 try 外面** —— 失败 / 取消时要把缓冲里
        // 压着的那一小段放出来（那也是模型说过的话）
        var titleStripper: TitleStripper? = null
        var status = RunStatus.RUNNING
        var errorCode: String? = null
        var errorMessage: String? = null
        val startedAtMs = System.currentTimeMillis()
        // token 用量**多轮累加**（与桌面同口径：工具循环里每一轮都要加进去）
        var usageTotal = EventPayloads.usage(0, 0)

        try {
            val adapter = Adapters.of(request.api)
                ?: throw UpstreamFailure(UpstreamErrorCode.CONFIG_ERROR, "未知协议：${request.api.wire}")
            val model = request.kit.models.firstOrNull {
                it.providerId == request.providerId && it.modelId == request.modelId
            }

            // 工具清单：kit 说的 ∩ 本地实现的（没实现的不给模型看见）
            val declared = tools.declaredTo(request.kit)
            val toolSpecs = if (model?.tools == false) emptyList() else declared.map {
                ToolSpec(it.name, it.description, it.inputSchema)
            }

            emit(
                RunEventType.RUN_STARTED,
                payload(
                    "runId" to runId,
                    "conversationId" to conversationId,
                    "providerId" to request.providerId,
                    "modelId" to request.modelId,
                    // 规则的可追溯性：这次回答用的是哪版规则（契约 §3.4）
                    "kitVersion" to request.kit.kitVersion,
                    "promptVersion" to request.kit.promptVersion,
                    "nsfw" to request.kit.nsfwEnabled,
                ),
            )
            emit(RunEventType.MESSAGE_STARTED, payload("messageId" to assistantId))

            val history = store.listMessages(conversationId)
            val userMessageCount = history.count { it.role == "user" }
            val conversation = store.getConversation(conversationId)
            val wantsTitle = shouldSummarizeTitle(conversation, userMessageCount) && !request.kit.titleInstruction.isNullOrBlank()
            titleStripper = if (wantsTitle) TitleStripper(enabled = true) else null

            val secret = resolveSecret(request.credentialRef)
            val reasoningRequest = buildReasoningRequest(request, model)
            val maxOutputTokens = model?.maxOutputTokens

            // 历史轮（去掉当前这条助手占位）
            val historyTurns = history.filter { it.id != assistantId }.mapNotNull { m ->
                if (m.role == "user" || (m.role == "assistant" && !m.text.isNullOrEmpty())) {
                    ChatTurn(m.role, m.text.orEmpty())
                } else {
                    null
                }
            }

            val toolTurns = mutableListOf<ChatTurn>()
            var step = 0
            var continuations = 0
            var continueFrom: String? = null
            var finishReason: String? = null

            var titleApplied = false
            fun applyTitleOnce(title: String) {
                if (titleApplied) return
                titleApplied = true
                applyConversationTitle(runId, conversationId, title, ::emit)
            }

            while (true) {
                // 取消检查：`HttpURLConnection` 的阻塞读不是协程取消点，不主动查就一直跑到底
                currentCoroutineContext().ensureActive()
                // 上下文拼装：system → **起标题指令（第 2 条）** → 历史 → 工具轮 → 续写指令
                //
                // ⚠ 起标题指令的位置**必须**在系统提示之后、历史之前：挂到末尾会挡住
                // "最后一条消息是工具结果"这个事实，模型靠它判断"工具跑完了、该收口"（AGENTS §10.1）。
                val roundTurns = buildList {
                    add(ChatTurn("system", request.kit.systemPrompt))
                    if (titleStripper != null && titleStripper.title == null) {
                        add(ChatTurn("system", request.kit.titleInstruction.orEmpty()))
                    }
                    addAll(historyTurns)
                    addAll(toolTurns)
                    continueFrom?.let { add(ChatTurn("system", request.kit.continueInstruction.orEmpty())) }
                }

                val outOfSteps = step >= request.maxSteps
                val roundTools = if (outOfSteps) emptyList() else toolSpecs
                val body = adapter.buildBody(
                    model = request.modelId,
                    messages = roundTurns,
                    stream = true,
                    reasoning = reasoningRequest,
                    tools = roundTools,
                    maxOutputTokens = maxOutputTokens,
                ).toString()

                val dedupe = ContinuationDedupe(continueFrom)
                var idle = true
                val roundText = StringBuilder()
                val round = upstream.stream(
                    baseUrl = request.baseUrl,
                    api = request.api,
                    secret = secret,
                    body = body,
                    adapter = adapter,
                ) { event ->
                    when (event) {
                        is StreamEvent.TextDelta -> {
                            idle = false
                            roundText.append(event.text)
                            val visible = titleStripper?.push(event.text) ?: event.text
                            // 标题**一摘到就落库**：这一轮后面无论怎么结束（正常/失败/取消），
                            // 已经知道的名字都不该丢
                            titleStripper?.title?.let { title -> applyTitleOnce(title) }
                            val out = dedupe.accept(visible, last = false)
                            if (out.isNotEmpty()) {
                                text.append(out)
                                parts.buffer(PartType.TEXT, out)
                                emit(RunEventType.TEXT_DELTA, payload("text" to out))
                            }
                        }
                        is StreamEvent.ReasoningDelta -> {
                            idle = false
                            reasoning.append(event.text)
                            parts.buffer(PartType.REASONING, event.text)
                            emit(RunEventType.REASONING_DELTA, payload("text" to event.text))
                        }
                        is StreamEvent.Usage -> {
                            usageTotal = EventPayloads.usageOf(event.usage)
                            emit(RunEventType.USAGE_UPDATED, usageTotal)
                        }
                        is StreamEvent.ProviderId -> Unit
                        is StreamEvent.ToolCallDelta -> idle = false
                    }
                }

                // 取消检查：流刚跑完也要查一次 —— 否则"取消"落在最后一轮里会被当成正常结束
                currentCoroutineContext().ensureActive()

                // 续写去重的尾巴（这一轮流完了，把压着的那一段放出来）
                val tail = dedupe.accept("", last = true)
                if (tail.isNotEmpty()) {
                    text.append(tail)
                    parts.buffer(PartType.TEXT, tail)
                    emit(RunEventType.TEXT_DELTA, payload("text" to tail))
                }
                if (continueFrom != null) continueFrom = null

                finishReason = round.outcome.finishReason

                // 正文与思考 → 有序块（一轮跑完才落块，顺序 = 到达顺序）
                parts.flushInflight()
                // 标题摘到了就写进会话（**改的一次**才算数）
                titleStripper?.title?.let { title -> applyTitleOnce(title) }

                if (idle) {
                    // 200 但一个事件都没有：如实报"上游没返回内容"，不许假装完成
                    throw UpstreamFailure(
                        UpstreamErrorCode.PROVIDER_UNREACHABLE,
                        "上游没有返回任何内容（空流）",
                    )
                }

                val drafts = round.toolCalls.drafts()
                if (drafts.isEmpty() || outOfSteps) {
                    // 没有工具调用（或轮数用尽）→ 收口
                    if (outOfSteps && drafts.isNotEmpty()) {
                        val note = "\n\n（已达到本次回复的工具轮数上限 ${request.maxSteps}，" +
                            "先在这里收口。你可以直接说「继续」，我再往下做。）"
                        text.append(note)
                        parts.add(PartType.TEXT, note)
                        emit(RunEventType.TEXT_DELTA, payload("text" to note))
                    }
                    // 输出被长度上限截断 → 接着写（措辞来自 run-kit，手机上不许自己写一份）
                    if (isTruncatedByOutputLimit(finishReason) &&
                        continuations < request.maxContinuations &&
                        !request.kit.continueInstruction.isNullOrBlank() &&
                        drafts.isEmpty()
                    ) {
                        // 这一轮到此为止：把标题状态机压着的缓冲放出来（那也是模型说过的话），
                        // 否则"接着写"的去重看不到上一轮的结尾，会把重复的话显示给用户
                        titleStripper?.flush()?.takeIf { it.isNotEmpty() }?.let { held ->
                            text.append(held)
                            parts.add(PartType.TEXT, held)
                            emit(RunEventType.TEXT_DELTA, payload("text" to held))
                        }
                        titleStripper?.title?.let { title -> applyTitleOnce(title) }
                        continuations++
                        continueFrom = text.toString()
                        step++
                        continue
                    }
                    break
                }

                // 工具轮：助手发起调用 → 逐个执行 → 结果回填
                step++
                val calls = drafts.map { ToolCallRef(it.callId, it.name, it.arguments) }
                toolTurns += ChatTurn("assistant", "", toolCalls = calls)
                parts.add(
                    PartType.TOOL_CALL,
                    text = null,
                    toolCallId = calls.joinToString(",") { it.id },
                    jsonPayload = buildJsonArrayOfCalls(calls),
                )
                calls.forEach { call ->
                    // 手机侧的工具都不需要审批（没有桌面写操作），所以 approval 恒为 not_required；
                    // **键名与桌面逐字一致**，否则共享的 Flutter 界面读不到参数/审批态
                    emit(
                        RunEventType.TOOL_REQUESTED,
                        EventPayloads.toolRequested(
                            runId = runId,
                            callId = call.id,
                            name = call.name,
                            arguments = call.arguments,
                            approval = "not_required",
                        ),
                    )
                }

                val ctx = ToolContext(
                    runId = runId,
                    conversationId = conversationId,
                    kit = request.kit,
                    store = store,
                    transport = transport,
                    files = files,
                    askGate = askGate,
                    emit = { type, body -> emit(type, body) },
                )
                calls.forEach { call ->
                    emit(RunEventType.TOOL_STARTED, EventPayloads.toolStarted(call.id, call.name))
                    val startedAt = System.currentTimeMillis()
                    val outcome = tools.invoke(ToolCallRequest(call.id, call.name, call.arguments), ctx)
                    parts.add(
                        PartType.TOOL_RESULT,
                        text = outcome.content,
                        toolCallId = call.id,
                        jsonPayload = outcome.jsonPayload?.toString(),
                    )
                    val (type, body) = EventPayloads.forToolOutcome(
                        callId = call.id,
                        name = call.name,
                        outcome = outcome,
                        elapsedMs = System.currentTimeMillis() - startedAt,
                    )
                    emit(type, body)
                    // 工具结果回填给模型：**当不可信数据**（横幅文案由 run-kit 下发，R7）
                    toolTurns += ChatTurn("tool", outcome.content, toolCallId = call.id)
                }

                if (step >= request.maxSteps) {
                    // 下一轮已经没有工具可用了（outOfSteps），再跑一次让模型收口
                    continue
                }
            }

            // 收尾：flush 标题缓冲（流结束时压着的那一段也是模型说的话）
            titleStripper?.flush()?.takeIf { it.isNotEmpty() }?.let { tail ->
                text.append(tail)
                parts.add(PartType.TEXT, tail)
                emit(RunEventType.TEXT_DELTA, payload("text" to tail))
            }
            titleStripper?.title?.let { title -> applyTitleOnce(title) }

            status = RunStatus.COMPLETED
            store.updateMessage(assistantId, text = text.toString(), status = MessageStatus.COMPLETE)
            store.finishRun(runId, status)
            emit(
                RunEventType.MESSAGE_COMPLETED,
                EventPayloads.messageCompleted(
                    messageId = assistantId,
                    text = text.toString(),
                    reasoning = reasoning.toString().takeIf { it.isNotEmpty() },
                    finishReason = finishReason,
                    elapsedMs = System.currentTimeMillis() - startedAtMs,
                    reasoningEffort = request.reasoningEffort?.wire,
                    steps = step,
                    parts = store.listParts(assistantId),
                    usage = usageTotal,
                ),
            )
            emit(RunEventType.RUN_COMPLETED, EventPayloads.runCompleted(runId, System.currentTimeMillis() - startedAtMs))
        } catch (e: kotlinx.coroutines.CancellationException) {
            // 取消：与失败同一条兜底（已经说出去的话一个字都不能丢）
            status = RunStatus.CANCELLED
            errorCode = "CANCELLED"
            errorMessage = "用户取消了这次回复"
            rescueAlreadySaid(assistantId, text, parts, titleStripper)
            store.updateMessage(assistantId, text = text.toString(), status = MessageStatus.CANCELLED)
            store.finishRun(runId, status, errorCode = errorCode, errorMessage = errorMessage)
            emit(
                RunEventType.MESSAGE_COMPLETED,
                EventPayloads.messageCompleted(
                    messageId = assistantId,
                    text = text.toString(),
                    reasoning = reasoning.toString().takeIf { it.isNotEmpty() },
                    finishReason = null,
                    elapsedMs = System.currentTimeMillis() - startedAtMs,
                    reasoningEffort = request.reasoningEffort?.wire,
                    steps = 0,
                    parts = store.listParts(assistantId),
                    usage = usageTotal,
                ),
            )
            emit(RunEventType.RUN_CANCELLED, EventPayloads.runCancelled(runId))
        } catch (e: Throwable) {
            val (code, message) = when (e) {
                is UpstreamFailure -> e.code to (e.message ?: "上游调用失败")
                else -> UpstreamErrorCode.PROTOCOL_ERROR to "${e::class.simpleName}: ${e.message}"
            }
            status = RunStatus.FAILED
            errorCode = code
            errorMessage = message
            rescueAlreadySaid(assistantId, text, parts, titleStripper)
            store.updateMessage(assistantId, text = text.toString(), status = MessageStatus.FAILED)
            store.finishRun(runId, status, errorCode = code, errorMessage = message)
            emit(
                RunEventType.MESSAGE_COMPLETED,
                EventPayloads.messageCompleted(
                    messageId = assistantId,
                    text = text.toString(),
                    reasoning = reasoning.toString().takeIf { it.isNotEmpty() },
                    finishReason = null,
                    elapsedMs = System.currentTimeMillis() - startedAtMs,
                    reasoningEffort = request.reasoningEffort?.wire,
                    steps = 0,
                    parts = store.listParts(assistantId),
                    usage = usageTotal,
                ),
            )
            emit(RunEventType.RUN_FAILED, EventPayloads.runFailed(runId, code, message))
        } finally {
            // P1 的纪律：事件批量落库，收尾（正常/失败/取消三条路都汇进这里）时 ①flush ②裁剪
            events.flush()
            runCatching { store.pruneEvents(runId, eventKeep) }
            bus.close(runId)
            jobs.remove(runId)
        }

        RunOutcome(
            runId = runId,
            assistantMessageId = assistantId,
            status = status,
            text = text.toString(),
            errorCode = errorCode,
        )
    }

    /**
     * **R6：失败 / 取消时"已经说出去的话一个字都不能丢"。**
     *
     * 三条纪律缺一不可（桌面 2026-09-19 的 429 现场逐条换来）：
     *  ① **正文要落块** —— 前端在 parts 非空时只渲染 parts，只写正文不落块 = 用户看不见；
     *  ② **标题缓冲要 flush** —— 第一句刚开口就断时，那压着的一段就是整段话；
     *  ③ **正文绝不抹空** —— 调用方随后的 `updateMessage` 用的是累积了内容的 [text]。
     */
    private fun rescueAlreadySaid(
        assistantId: String,
        text: StringBuilder,
        parts: PartSink,
        stripper: TitleStripper?,
    ) {
        stripper?.flush()?.takeIf { it.isNotEmpty() }?.let { held ->
            text.append(held)
            parts.buffer(PartType.TEXT, held)
        }
        parts.flushInflight()
    }

    // -----------------------------------------------------------------------
    //  小工具（纯逻辑，单测盯着）
    // -----------------------------------------------------------------------

    private fun shouldSummarizeTitle(conversation: ConversationRecord?, userMessageCount: Int): Boolean {
        if (conversation == null) return false
        // 只在"第一问"上起标题，且**只对还没起过名字的会话**
        return userMessageCount <= 1 && (conversation.title.isBlank() || conversation.title == DEFAULT_TITLE)
    }

    private fun applyConversationTitle(
        runId: String,
        conversationId: String,
        title: String,
        emit: (String, JsonObject) -> Unit,
    ) {
        store.renameConversation(conversationId, title)
        emit(
            RunEventType.CONVERSATION_UPDATED,
            payload("runId" to runId, "conversationId" to conversationId, "title" to title),
        )
    }

    private fun buildReasoningRequest(request: RunRequest, model: com.comfyhub.harness.kit.KitModel?): ReasoningRequest {
        val declared = model?.reasoning ?: false
        val levelMap = model?.thinkingEfforts.orEmpty().mapValues { (_, v) ->
            com.comfyhub.harness.protocol.LevelSpec.of(v)
                ?: com.comfyhub.harness.protocol.LevelSpec.Name(v)
        }
        val format = ThinkingFormat.parse(model?.thinkingFormat) ?: ThinkingFormat.OPENAI
        return ReasoningRequest.of(request.reasoningEffort, declared, levelMap, format)
    }

    /** 工具卡片的 payload：逐个调用的 id/name（界面据此渲染，**不掺正文**）。 */
    private fun buildJsonArrayOfCalls(calls: List<ToolCallRef>): String =
        kotlinx.serialization.json.buildJsonArray {
            calls.forEach { call ->
                add(
                    kotlinx.serialization.json.buildJsonObject {
                        put("id", call.id)
                        put("name", call.name)
                    }
                )
            }
        }.toString()

    companion object {
        /** 会话还没起名时的默认标题（与 schema 的 `DEFAULT '新对话'` 同值）。 */
        const val DEFAULT_TITLE = "新对话"

        /** 续写查重窗口（与桌面 `HarnessRunner.OVERLAP_WINDOW` 同值）。 */
        const val OVERLAP_WINDOW = 80

        /**
         * 上游给出的结束原因里，哪些表示"是被输出长度上限截断的"（三家协议各不相同）。
         *
         * 移植自桌面 `HarnessRunner.isTruncatedByOutputLimit`。
         */
        fun isTruncatedByOutputLimit(finishReason: String?): Boolean = when (finishReason?.lowercase()) {
            "length", "max_tokens", "max_output_tokens", "max_output_token" -> true
            else -> false
        }

        /**
         * [previous] 的**结尾**与 [next] 的**开头**最长的重叠长度（用于去掉续写时的重复）。
         *
         * 只在 [OVERLAP_WINDOW] 范围内找：模型重复的通常是最后半句话，
         * 超过这个长度的"巧合"不值得为它付出截断正文的风险。
         */
        fun longestSuffixOverlap(previous: String?, next: String): Int {
            if (previous.isNullOrEmpty() || next.isEmpty()) return 0
            val max = minOf(next.length, OVERLAP_WINDOW)
            for (k in max downTo 1) {
                if (previous.endsWith(next.substring(0, k))) return k
            }
            return 0
        }
    }
}

/**
 * 续写去重（把"接着写"那一段开头重复的部分剪掉）。
 *
 * 为什么需要一个小类：判断重叠要**等看清开头那一小段**（最多 [HarnessRunner.OVERLAP_WINDOW] 个字符），
 * 而流式增量是一片片到的。所以这里压住开头、够长了才一次性放行 —— 压住的代价是
 * "续写的头几十个字符晚一点点出现"，换来的是**不会把重复的话显示给用户**。
 */
class ContinuationDedupe(private val previous: String?) {

    private val held = StringBuilder()
    private var decided = previous.isNullOrEmpty()

    /** 喂一片（[last] = true 表示这一轮结束了，剩下的全放行）。 */
    fun accept(chunk: String, last: Boolean): String {
        if (decided) return chunk
        held.append(chunk)
        if (!last && held.length <= HarnessRunner.OVERLAP_WINDOW) return ""
        val overlap = HarnessRunner.longestSuffixOverlap(previous, held.toString())
        decided = true
        val out = held.substring(overlap)
        held.setLength(0)
        return out
    }
}
