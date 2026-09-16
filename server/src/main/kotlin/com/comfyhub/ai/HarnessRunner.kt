package com.comfyhub.ai

import com.comfyhub.ai.protocol.Adapters
import com.comfyhub.ai.protocol.AiApiRef
import com.comfyhub.ai.protocol.ChatTurn
import com.comfyhub.ai.protocol.LevelSpec
import com.comfyhub.ai.protocol.ProtocolFailure
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.ReasoningRequest
import com.comfyhub.ai.protocol.SseAccumulator
import com.comfyhub.ai.protocol.StreamEvent
import com.comfyhub.ai.protocol.ThinkingFormat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.time.Duration
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.coroutineContext

/**
 * Harness 运行器：把一个 Run 从"用户消息"跑到"助手消息落库"（AIH-020 ~ AIH-024）。
 *
 * 设计要点：
 *  - Run 是**独立实体**：HTTP 请求只负责创建（202 + runId），执行在后台协程里，进程崩了也不会
 *    把半截状态留在数据库（启动时把遗留 running 标成 failed）；
 *  - 事件经 [RunEventBus] 统一输出，Flutter 不解析供应商 SSE；
 *  - 取消 = 取消协程 → `runInterruptible` 打断阻塞读 → 上游连接关闭，Run 记为 cancelled；
 *  - **密钥只在构造请求头时出现一次**，不写日志、不进事件、不进快照。
 */
class HarnessRunner(
    private val credentials: CredentialService,
    private val bus: RunEventBus,
) {
    private val log = LoggerFactory.getLogger(HarnessRunner::class.java)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val jobs = ConcurrentHashMap<String, Job>()

    companion object {
        /** 单次 Run 总时限（AIH-020 建议 15 分钟） */
        private const val RUN_TIMEOUT = "15m"
        const val PROMPT_VERSION = SystemPrompt.VERSION
    }

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(10))
        .followRedirects(HttpClient.Redirect.NEVER)
        .build()

    fun start(runId: String) {
        jobs[runId] = scope.launch {
            try {
                execute(runId)
            } finally {
                jobs.remove(runId)
            }
        }
    }

    fun cancel(runId: String): Boolean {
        val job = jobs[runId] ?: return false
        job.cancel()
        return true
    }

    fun isRunning(runId: String): Boolean = jobs[runId]?.isActive == true

    fun shutdown() {
        scope.cancel()
    }

    // -----------------------------------------------------------------------

    private suspend fun execute(runId: String) {
        val run = AiRunRepo.get(runId) ?: return
        val conversationId = run.conversationId
        val assistantId = run.assistantMessageId ?: return

        fun emit(type: String, payload: JsonObject) {
            val event = bus.emit(runId, type, payload) ?: return
            AiRunRepo.appendEvent(runId, event.seq, type, payload)
        }

        try {
            val provider = AiRepo.getProvider(run.providerId.orEmpty())
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 不存在: ${run.providerId}")
            val api = AiApiRef.parse(provider.api)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "未知协议: ${provider.api}")
            val adapter = Adapters.of(api)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "没有可用的协议适配器: ${provider.api}")

            emit(
                RunEventType.RUN_STARTED,
                payload(
                    "runId" to runId,
                    "conversationId" to conversationId,
                    "providerId" to provider.id,
                    "modelId" to run.modelId,
                    "promptVersion" to PROMPT_VERSION,
                    "reasoningEffort" to run.reasoningEffort,
                )
            )
            emit(RunEventType.MESSAGE_STARTED, payload("messageId" to assistantId, "role" to "assistant"))

            // 历史消息 + 系统提示（只取正文；工具/备注类不进上下文）
            val history = AiConversationRepo.listMessages(conversationId)
                .filter { it.id != assistantId }
                .filter { it.role == "user" || it.role == "assistant" }
                .filter { it.text.isNotBlank() || it.role == "user" }

            val turns = buildList {
                add(ChatTurn("system", SystemPrompt.render(provider, run.modelId.orEmpty())))
                history.forEach { add(ChatTurn(it.role, it.text)) }
            }

            val secret = credentials.resolve(provider.credentialRef)
            if (provider.credentialRef != null && secret.isNullOrEmpty()) {
                throw AiException(
                    AiErrorCode.MISSING_CREDENTIAL,
                    "该 Provider 引用了凭据 ${provider.credentialRef}，但本机没有配置（设置 → AI 模型）"
                )
            }

            val started = System.currentTimeMillis()
            val selfJob = coroutineContext[Job]
            var text = StringBuilder()
            var reasoning = StringBuilder()
            var usage: JsonObject? = null
            var providerResponseId: String? = null
            var lastPersist = 0L

            // 思考强度：真源是模型目录（AIH-056）。Run 记录的是**生效值**；
            // 模型不支持推理时这里已经是 NONE，适配器一个思考字段都不会发。
            val reasoningRequest = buildReasoningRequest(run)

            val outcome = streamUpstream(
                provider = provider,
                api = api,
                adapterVersion = adapter.adapterVersion,
                model = run.modelId.orEmpty(),
                turns = turns,
                secret = secret,
                body = adapter.buildBody(
                    run.modelId.orEmpty(),
                    turns,
                    stream = true,
                    reasoning = reasoningRequest,
                ),
                onEvent = { event ->
                    when (event) {
                        is StreamEvent.TextDelta -> {
                            text.append(event.text)
                            emit(RunEventType.TEXT_DELTA, payload("messageId" to assistantId, "text" to event.text))
                            // 粗粒度落库：UI 靠事件实时刷新，数据库只需要"接近最新"，
                            // 避免每个 token 都打一次 UPDATE
                            val now = System.currentTimeMillis()
                            if (now - lastPersist > 1000) {
                                lastPersist = now
                                AiRunRepo.updateMessage(assistantId, text.toString(), "streaming")
                            }
                        }
                        is StreamEvent.ReasoningDelta -> {
                            reasoning.append(event.text)
                            emit(RunEventType.REASONING_DELTA, payload("messageId" to assistantId, "text" to event.text))
                        }
                        is StreamEvent.Usage -> {
                            usage = (event.usage as? JsonObject)
                            emit(RunEventType.USAGE_UPDATED, payload("usage" to event.usage))
                        }
                        is StreamEvent.ProviderId -> providerResponseId = event.id
                    }
                },
                parse = { frame -> Adapters.of(api)!!.interpret(frame) },
                apiRef = api,
                isCancelled = { selfJob?.isActive == false },
            )

            // 落终态
            AiRunRepo.updateMessage(
                assistantId,
                text.toString(),
                "complete",
                providerResponseId = outcome.providerResponseId ?: providerResponseId,
                usage = usage,
            )
            emit(
                RunEventType.MESSAGE_COMPLETED,
                payload(
                    "messageId" to assistantId,
                    "text" to text.toString(),
                    "reasoning" to reasoning.toString().takeIf { it.isNotEmpty() },
                    "finishReason" to outcome.finishReason,
                    "elapsedMs" to (System.currentTimeMillis() - started),
                    "reasoningEffort" to run.reasoningEffort,
                    // token 统计（AIH-057）：归一化后再给前端，避免前端解析各家的 usage 方言
                    "usage" to TokenUsage.from(usage).let { u ->
                        buildJsonObject {
                            put("inputTokens", u.inputTokens)
                            put("outputTokens", u.outputTokens)
                            put("cachedTokens", u.cachedTokens)
                            put("reasoningTokens", u.reasoningTokens)
                        }
                    },
                )
            )
            AiRunRepo.finish(runId, "completed", usage = usage)
            emit(
                RunEventType.RUN_COMPLETED,
                payload("runId" to runId, "elapsedMs" to System.currentTimeMillis() - started)
            )
        } catch (e: kotlinx.coroutines.CancellationException) {
            val partial = AiConversationRepo.listMessages(conversationId)
                .firstOrNull { it.id == assistantId }?.text.orEmpty()
            AiRunRepo.updateMessage(assistantId, partial, "cancelled")
            AiRunRepo.finish(runId, "cancelled", errorCode = AiErrorCode.ABORTED, errorMessage = "用户已停止")
            emit(RunEventType.RUN_CANCELLED, payload("runId" to runId))
            throw e
        } catch (e: Throwable) {
            val (code, message) = classify(e)
            log.warn("Run 失败 run={} provider={} code={} msg={}", runId, run.providerId, code, message)
            AiRunRepo.updateMessage(assistantId, "", "failed")
            AiRunRepo.finish(runId, "failed", errorCode = code, errorMessage = message)
            emit(RunEventType.RUN_FAILED, payload("runId" to runId, "code" to code, "message" to message))
        } finally {
            bus.close(runId)
        }
    }

    /**
     * 把 Run 记录（生效强度）+ 模型快照（是否支持推理 / 等级表 / 方言）换算成请求设置（AIH-056）。
     *
     * 读**快照**而不是读模型目录：用户在这次 Run 跑起来之后改目录，不应该影响已经在飞的请求。
     */
    private fun buildReasoningRequest(run: AiRunDto): ReasoningRequest {
        val effort = ReasoningEffort.parse(run.reasoningEffort) ?: return ReasoningRequest.NONE
        val model = AiRepo.getModel(run.providerId.orEmpty(), run.modelId.orEmpty())
        val format = ThinkingFormat.parse(model?.thinkingFormat) ?: ThinkingFormat.OPENAI

        // 模型目录已经不在（被删）时，按"不支持推理"处理，宁可不发参数也不要 400
        val declared = model?.reasoning == true
        val levelMap = model?.thinkingEfforts.orEmpty().mapNotNull { (k, v) ->
            LevelSpec.of(v)?.let { k to it }
        }.toMap()
        return ReasoningRequest.of(effort, declaredReasoning = declared, levelMap = levelMap, format = format)
    }

    private fun classify(e: Throwable): Pair<String, String> = when (e) {
        is AiException -> e.code to (e.message ?: "上游请求失败")
        is ProtocolFailure -> AiErrorCode.PROTOCOL_ERROR to (e.message ?: "上游返回了无法解析的内容")
        is java.net.http.HttpTimeoutException -> AiErrorCode.PROVIDER_UNREACHABLE to "上游响应超时"
        is java.io.InterruptedIOException, is InterruptedException ->
            AiErrorCode.ABORTED to "请求被中断"
        else -> AiErrorCode.PROVIDER_UNREACHABLE to "上游请求失败：${e::class.simpleName}"
    }

    // -----------------------------------------------------------------------
    //  上游流式请求
    // -----------------------------------------------------------------------

    private data class Idle(
        val finishReason: String?,
        val providerResponseId: String?,
    )

    private data class Meta(val providerId: String?, val finishReason: String?)

    /** 从任意供应商的 chunk 里尽量抽出 response id 与 finish_reason（尽力而为，失败不影响正文）。 */
    private fun extractMeta(data: String, apiRef: AiApiRef): Meta {
        val root = com.comfyhub.AppJson.parseToJsonElement(data) as? JsonObject ?: return Meta(null, null)
        val providerId = (root["id"] as? JsonPrimitive)?.contentOrNull
        var finishReason: String? = null
        (root["choices"] as? JsonArray)?.forEach { choice ->
            val reason = ((choice as? JsonObject)?.get("finish_reason") as? JsonPrimitive)?.contentOrNull
            if (!reason.isNullOrEmpty()) finishReason = reason
        }
        if (apiRef == AiApiRef.ANTHROPIC_MESSAGES) {
            (root["delta"] as? JsonObject)?.let { delta ->
                (delta["stop_reason"] as? JsonPrimitive)?.contentOrNull?.let { finishReason = it }
            }
        }
        if (apiRef == AiApiRef.OPENAI_RESPONSES) {
            // Responses 的结束状态在 response.completed 的 response.status 里（completed / incomplete）
            val response = (root["response"] as? JsonObject) ?: root
            (response["status"] as? JsonPrimitive)?.contentOrNull?.let { status ->
                if (status == "incomplete") {
                    finishReason = (response["incomplete_details"] as? JsonObject)
                        ?.let { (it["reason"] as? JsonPrimitive)?.contentOrNull }
                        ?: "incomplete"
                } else if (status.isNotEmpty()) {
                    finishReason = status
                }
            }
        }
        return Meta(providerId, finishReason)
    }

    private suspend fun streamUpstream(
        provider: AiProviderDto,
        api: AiApiRef,
        adapterVersion: String,
        model: String,
        turns: List<ChatTurn>,
        secret: String?,
        body: JsonObject,
        onEvent: (StreamEvent) -> Unit,
        parse: (SseAccumulator.Frame) -> List<StreamEvent>,
        apiRef: AiApiRef,
        isCancelled: () -> Boolean,
    ): Idle = runInterruptible(Dispatchers.IO) {
        val trust = EndpointTrust.parse(provider.endpointTrust) ?: EndpointTrust.PUBLIC
        val url = chatUrl(provider.baseURL, api)
        val uri = URI(url)
        // SSRF 复核：每次请求都按真实解析结果再查一次（AIH-017）
        EndpointGuard.resolveAndCheck(uri.host, trust)

        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofMinutes(15))
            .header("Content-Type", "application/json")
            .header("Accept", api.acceptHeader())
            .POST(HttpRequest.BodyPublishers.ofString(body.toString(), StandardCharsets.UTF_8))

        when (api) {
            AiApiRef.OPENAI_COMPLETIONS, AiApiRef.OPENAI_RESPONSES ->
                if (!secret.isNullOrEmpty()) builder.header("Authorization", "Bearer $secret")
            AiApiRef.ANTHROPIC_MESSAGES -> {
                if (!secret.isNullOrEmpty()) builder.header("x-api-key", secret)
                builder.header("anthropic-version", "2023-06-01")
            }
        }

        // 只记录 provider / 端点 / 状态，绝不记录 header 与请求体
        log.info("Run 请求 provider={} endpoint={} adapter={} model={}", provider.id, "${uri.scheme}://${uri.host}:${uri.port}${uri.path}", adapterVersion, model)

        val response = client.send(builder.build(), HttpResponse.BodyHandlers.ofInputStream())
        if (response.statusCode() !in 200..299) {
            val raw = runCatching {
                response.body().use { String(it.readNBytes(1200), StandardCharsets.UTF_8) }
            }.getOrDefault("")
            val code = AiUpstream.refineFromBody(
                AiUpstream.classifyStatus(response.statusCode()) ?: AiErrorCode.PROTOCOL_ERROR,
                raw,
            ) ?: AiErrorCode.PROTOCOL_ERROR
            val detail = redact(raw, secret)
            // 把上游的报错带出来（已脱敏、已截断）—— 否则用户只看到"配置有误"，
            // 根本不知道是参数不支持、端点不对还是别的（400 类问题全靠这句话定位）
            log.warn("上游返回 {}（code={}）: {}", response.statusCode(), code, detail)
            throw AiException(
                code,
                if (detail.isEmpty()) AiUpstream.explain(code, response.statusCode())
                else "${AiUpstream.explain(code, response.statusCode())}｜上游返回：$detail"
            )
        }

        val accumulator = SseAccumulator()
        var finishReason: String? = null
        var providerResponseId: String? = null

        BufferedReader(InputStreamReader(response.body(), StandardCharsets.UTF_8)).use { reader ->
            while (true) {
                // 取消时立刻退出，不再继续读上游（runInterruptible 会把阻塞读打断）
                if (isCancelled()) throw InterruptedException("Run 已取消")
                val line = reader.readLine() ?: break
                val frame = accumulator.line(line) ?: continue
                val data = frame.data.trim()
                if (data == "[DONE]") break

                // finish_reason / response id 由适配器解释不到，这里补一层通用提取
                runCatching { extractMeta(data, apiRef) }.getOrNull()?.let { meta ->
                    if (providerResponseId == null) providerResponseId = meta.providerId
                    if (!meta.finishReason.isNullOrEmpty()) finishReason = meta.finishReason
                }

                parse(frame).forEach(onEvent)
            }
        }
        Idle(finishReason, providerResponseId)
    }

    /** 日志里只出现 scheme://host:port/path，不带 query / fragment / userInfo。 */
    private fun safeEndpoint(uri: URI): String = buildString {
        append(uri.scheme).append("://").append(uri.host)
        if (uri.port > 0) append(':').append(uri.port)
        append(uri.path ?: "")
    }

    /**
     * 上游报错文本脱敏后回显：**必须**先把密钥抹掉，再截断。
     * 有些网关会把请求体/请求头原样回显，直接透传等于把 Key 写进界面和日志（AIH-051）。
     */
    private fun redact(raw: String, secret: String?): String = AiUpstream.redact(raw, secret)

    private fun chatUrl(baseURL: String, api: AiApiRef): String {
        val base = baseURL.trimEnd('/')
        return when (api) {
            AiApiRef.OPENAI_COMPLETIONS -> "$base/chat/completions"
            // Responses 是另一个端点：/v1/responses。（AIH-004）
            AiApiRef.OPENAI_RESPONSES ->
                if (base.endsWith("/v1")) "$base/responses" else "$base/v1/responses"
            AiApiRef.ANTHROPIC_MESSAGES ->
                if (base.endsWith("/v1")) "$base/messages" else "$base/v1/messages"
        }
    }

    private fun AiApiRef.acceptHeader(): String = when (this) {
        AiApiRef.ANTHROPIC_MESSAGES -> "text/event-stream"
        else -> "text/event-stream"
    }
}

/**
 * 系统提示词 v1（AIH-046）。**版本化**：每次 Run 记录 [VERSION]，
 * 修改模板只影响新 Run，已发生的 Run 行为可追溯。
 */
object SystemPrompt {
    const val VERSION = "v1"

    fun render(provider: AiProviderDto, modelId: String): String = """
        你是 ComfyHub AI 工作台中的创作与生成业务顾问，帮助用户完成生图、生视频、音乐及相关内容生产的
        需求澄清、提示词设计、方案拆解与进度跟进。

        当前运行环境：
        - Provider: ${provider.displayName}（${provider.api}）
        - Model: $modelId
        - 当前时间: ${java.time.Instant.now()}

        必须遵守：
        1. 使用中文回答，除非用户明确要求其他语言；先解决业务问题，再补充必要的技术细节。
        2. 不要声称读取了没有实际发送给你的附件。附件是否可发送由 Harness 准入决定。
        3. 不要自行无限轮询或假设工具存在：当前版本尚未注册任何工具，也不要宣称已经提交了生成任务。
        4. 绝不索取、显示或推测 API Key、数据库密码等机密；配置问题只引导用户前往「设置 → AI 模型」。
        5. 对生图／生视频需求，主动澄清会显著改变结果的关键信息（用途、模型、画幅、时长、风格、交付格式），
           但不要为无关细节反复追问。
        6. 区分"咨询建议"与"已提交/正在运行/已完成/已入库"，不承诺一定生成出某种结果。
    """.trimIndent()
}
