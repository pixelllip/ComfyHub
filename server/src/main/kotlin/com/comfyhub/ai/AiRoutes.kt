package com.comfyhub.ai

import com.comfyhub.ApiError
import com.comfyhub.ai.protocol.Adapters
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.TransportRef
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.server.request.receive
import io.ktor.server.response.respond
import io.ktor.server.response.respondTextWriter
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.patch
import io.ktor.server.routing.post
import io.ktor.server.routing.put
import io.ktor.server.routing.route
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * AI 工作台接口。
 *
 * 约定：
 *  - 任何返回 DTO 都不含密钥值，只有 [CredentialStatusDto]（AIH-012）；
 *  - 凭据 `PUT` 时值为空 = 不修改；清除必须显式 `DELETE`（AIH-013）；
 *  - Provider 更新必须带 revision（AIH-007）；
 *  - Run 为独立实体：创建返回 202，事件走 SSE（AIH-020/021）。
 */
fun Route.aiRoutes(
    credentials: CredentialService,
    runner: HarnessRunner,
    bus: RunEventBus,
) {

    route("/ai") {

        // -------------------------------------------------------------------
        //  Provider
        // -------------------------------------------------------------------

        get("/providers") {
            call.respond(AiRepo.listProviders().map { it.withCredential(credentials) })
        }

        post("/providers") {
            val body = call.receive<AiProviderUpsert>()
            val id = AiValidation.requireProviderId(body.id)
            if (AiRepo.getProvider(id) != null) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "Provider ID 已存在: $id（ID 创建后不可修改）")
            }
            val api = AiApi.parse(body.api)
                ?: throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "不支持的协议 ${body.api}；首期仅支持 ${AiApi.wireValues.joinToString(" / ")}"
                )
            val trust = EndpointTrust.parse(body.endpointTrust) ?: EndpointTrust.infer(hostOf(body.baseURL))
            val dto = AiProviderDto(
                id = id,
                displayName = AiValidation.requireDisplayName(body.displayName),
                api = api.wire,
                baseURL = AiValidation.normalizeBaseUrl(body.baseURL, trust),
                credentialRef = AiValidation.requireCredentialRef(body.credentialRef),
                endpointTrust = trust.wire,
                enabled = body.enabled,
            )
            call.respond(HttpStatusCode.Created, AiRepo.insertProvider(dto).withCredential(credentials))
        }

        get("/providers/{id}") {
            val id = call.parameters["id"].orEmpty()
            val p = AiRepo.getProvider(id)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 不存在: $id")
            call.respond(p.withCredential(credentials))
        }

        put("/providers/{id}") {
            val id = AiValidation.requireProviderId(call.parameters["id"])
            val body = call.receive<AiProviderUpsert>()
            if (body.id != null && body.id != id) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "Provider ID 创建后不可修改")
            }
            val api = AiApi.parse(body.api)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "不支持的协议 ${body.api}")
            val trust = EndpointTrust.parse(body.endpointTrust) ?: EndpointTrust.infer(hostOf(body.baseURL))
            val revision = body.revision
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "缺少 revision，请刷新后重试")
            val dto = AiProviderDto(
                id = id,
                displayName = AiValidation.requireDisplayName(body.displayName),
                api = api.wire,
                baseURL = AiValidation.normalizeBaseUrl(body.baseURL, trust),
                credentialRef = AiValidation.requireCredentialRef(body.credentialRef),
                endpointTrust = trust.wire,
                enabled = body.enabled,
            )
            call.respond(AiRepo.updateProvider(dto, revision).withCredential(credentials))
        }

        delete("/providers/{id}") {
            val id = call.parameters["id"].orEmpty()
            val removed = AiRepo.deleteProvider(id)
            if (!removed) throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 不存在: $id")
            call.respond(DeleteResult(deleted = true, id = id))
        }

        // -------------------------------------------------------------------
        //  连接测试（AIH-008）：只返回脱敏结果与稳定错误码，日志不含密钥
        // -------------------------------------------------------------------

        post("/providers/{id}/test") {
            val p = requireProvider(call.parameters["id"].orEmpty())
            // 密钥只在后端内部解析，用完即弃；不写日志、不进响应
            val secret = credentials.resolve(p.credentialRef)
            val result = AiUpstream.testConnection(p, secret)
            call.respond(
                ProviderTestResponse(
                    ok = result.ok,
                    errorCode = result.errorCode,
                    message = result.message,
                    httpStatus = result.httpStatus,
                    modelCount = result.modelCount,
                )
            )
        }

        // -------------------------------------------------------------------
        //  模型发现（AIH-009）：只返回候选，**不落库**；用户勾选后才能进目录
        // -------------------------------------------------------------------

        post("/providers/{id}/discover-models") {
            val p = requireProvider(call.parameters["id"].orEmpty())
            val secret = credentials.resolve(p.credentialRef)
            val result = AiUpstream.discoverModels(p, secret)
            call.respond(
                DiscoverModelsResponse(
                    ok = result.ok,
                    errorCode = result.errorCode,
                    message = result.message,
                    candidates = result.candidates.map {
                        ModelCandidateDto(
                            id = it.id,
                            displayName = it.displayName,
                            contextWindow = it.contextWindow,
                            maxOutputTokens = it.maxOutputTokens,
                            modalities = it.modalities,
                            tools = it.tools,
                            reasoning = it.reasoning,
                            capabilitySource = it.capabilitySource,
                            capabilityNote = it.capabilityNote,
                        )
                    },
                )
            )
        }

        // -------------------------------------------------------------------
        //  凭据：只写 + 状态
        // -------------------------------------------------------------------

        get("/providers/{id}/credentials") {
            val p = requireProvider(call.parameters["id"].orEmpty())
            call.respond(credentials.describe(p.credentialRef))
        }

        put("/providers/{id}/credentials") {
            val p = requireProvider(call.parameters["id"].orEmpty())
            val ref = p.credentialRef
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "该 Provider 未设置凭据引用名（credentialRef）")
            if (credentials.describe(ref).source == "env") {
                throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "凭据 $ref 由环境变量提供，是只读的；请改用受管凭据引用名"
                )
            }
            val body = call.receive<CredentialSetRequest>()
            if (body.value.isBlank()) {
                // 留空 = 不修改（AIH-012）；清除请用 DELETE
                call.respond(credentials.describe(ref))
                return@put
            }
            credentials.set(ref, body.value)
            call.respond(credentials.describe(ref))
        }

        delete("/providers/{id}/credentials") {
            val p = requireProvider(call.parameters["id"].orEmpty())
            val ref = p.credentialRef
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "该 Provider 未设置凭据引用名（credentialRef）")
            if (credentials.describe(ref).source == "env") {
                throw AiException(AiErrorCode.CONFIG_ERROR, "凭据 $ref 由环境变量提供，无法在应用内移除")
            }
            credentials.unset(ref)
            call.respond(credentials.describe(ref))
        }

        // -------------------------------------------------------------------
        //  模型目录
        // -------------------------------------------------------------------

        get("/providers/{id}/models") {
            requireProvider(call.parameters["id"].orEmpty())
            call.respond(AiRepo.listModels(call.parameters["id"].orEmpty()))
        }

        put("/providers/{id}/models") {
            val id = call.parameters["id"].orEmpty()
            requireProvider(id)
            val body = call.receive<AiModelUpsert>()
            body.models.forEach { validateModel(it) }
            call.respond(AiRepo.replaceModels(id, body.models))
        }

        // -------------------------------------------------------------------
        //  会话与消息（AIH-018 / AIH-019）
        // -------------------------------------------------------------------

        get("/conversations") {
            val includeArchived = call.request.queryParameters["includeArchived"]
                ?.let { it == "1" || it.equals("true", true) } ?: false
            call.respond(AiConversationRepo.list(includeArchived))
        }

        post("/conversations") {
            val body = call.receive<AiConversationCreate>()
            call.respond(HttpStatusCode.Created, AiConversationRepo.create(body))
        }

        get("/conversations/{id}") {
            val id = call.parameters["id"].orEmpty()
            call.respond(
                AiConversationRepo.get(id)
                    ?: throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $id")
            )
        }

        patch("/conversations/{id}") {
            val id = call.parameters["id"].orEmpty()
            call.respond(AiConversationRepo.patch(id, call.receive<AiConversationPatch>()))
        }

        delete("/conversations/{id}") {
            val id = call.parameters["id"].orEmpty()
            if (!AiConversationRepo.delete(id)) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $id")
            }
            call.respond(DeleteResult(deleted = true, id = id))
        }

        get("/conversations/{id}/messages") {
            val id = call.parameters["id"].orEmpty()
            AiConversationRepo.get(id)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $id")
            call.respond(AiConversationRepo.listMessages(id))
        }

        post("/conversations/{id}/messages") {
            val id = call.parameters["id"].orEmpty()
            call.respond(
                HttpStatusCode.Created,
                AiConversationRepo.appendMessage(id, call.receive<AiMessageAppend>())
            )
        }

        // -------------------------------------------------------------------
        //  Run：创建 202 → SSE 事件 → 取消（AIH-020 / AIH-021 / AIH-022）
        // -------------------------------------------------------------------

        post("/conversations/{id}/runs") {
            val conversationId = call.parameters["id"].orEmpty()
            AiConversationRepo.get(conversationId)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "会话不存在: $conversationId")
            val body = call.receive<AiRunStartRequest>()
            if (body.text.isBlank()) throw AiException(AiErrorCode.CONFIG_ERROR, "消息内容不能为空")

            val provider = requireProvider(body.providerId)
            if (!provider.enabled) throw AiException(AiErrorCode.CONFIG_ERROR, "该 Provider 已被停用")
            val model = AiRepo.getModel(provider.id, body.modelId)
                ?: throw AiException(AiErrorCode.UNKNOWN_MODEL, "模型不在目录中: ${body.modelId}")
            if (!model.enabled) throw AiException(AiErrorCode.CONFIG_ERROR, "该模型已被停用")

            // 思考强度：先落成枚举（非法值直接拒绝，不能悄悄当 off，AIH-056）
            val requestedEffort = ReasoningEffort.parse(body.reasoningEffort)
                ?: throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "未知的思考强度：${body.reasoningEffort}（可选 ${ReasoningEffort.entries.joinToString(" / ") { it.wire }}）"
                )
            val effort = AiValidation.requireThinkingEffort(model, requestedEffort)

            // 用户消息与助手占位都在"创建 Run"里完成，保证顺序与 seq 稳定
            val userMessage = AiConversationRepo.appendMessage(
                conversationId,
                AiMessageAppend(
                    role = "user",
                    text = body.text,
                    parts = listOf(AiMessagePartDto(type = "text", text = body.text)),
                ),
            )
            val assistantId = AiRunRepo.insertAssistantPlaceholder(conversationId, provider.id, model.id)

            val run = AiRunRepo.create(
                conversationId = conversationId,
                providerId = provider.id,
                modelId = model.id,
                // 快照**不含密钥**，只留引用名（AIH-023）
                providerSnapshot = buildJsonObject {
                    put("id", provider.id)
                    put("displayName", provider.displayName)
                    put("api", provider.api)
                    put("baseURL", provider.baseURL)
                    put("endpointTrust", provider.endpointTrust)
                    provider.credentialRef?.let { put("credentialRef", it) }
                },
                modelSnapshot = buildJsonObject {
                    put("id", model.id)
                    put("displayName", model.displayName)
                    put("tools", model.tools)
                    put("reasoning", model.reasoning)
                    put("capabilitySource", model.capabilitySource)
                },
                promptVersion = HarnessRunner.PROMPT_VERSION,
                userMessageId = userMessage.id,
                assistantMessageId = assistantId,
                retryOfRunId = body.retryOfRunId,
                // 快照里记生效值（不是请求值）：模型不支持推理时会落成 off
                reasoningEffort = (effort ?: ReasoningEffort.OFF).wire,
            )

            bus.open(run.id)
            runner.start(run.id)
            call.respond(
                HttpStatusCode.Accepted,
                AiRunStartResponse(runId = run.id, assistantMessageId = assistantId, userMessageId = userMessage.id),
            )
        }

        get("/runs/{id}") {
            val runId = call.parameters["id"].orEmpty()
            val run = AiRunRepo.get(runId) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Run 不存在: $runId")
            call.respond(run.copy().let { it })
        }

        /**
         * 统一事件流（SSE）。`after=<seq>` 可断线续传；
         * Run 不在内存里（后端重启过）时从数据库回放已持久化的部分。
         */
        get("/runs/{id}/events") {
            val runId = call.parameters["id"].orEmpty()
            val after = call.request.queryParameters["after"]?.toIntOrNull() ?: 0
            if (AiRunRepo.get(runId) == null) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "Run 不存在: $runId")
            }
            call.respondTextWriter(ContentType.Text.EventStream, HttpStatusCode.OK) {
                val channel: ReceiveChannel<RunEvent>? = bus.subscribe(runId, after)
                if (channel == null) {
                    // 进程重启过：只能回放落库的事件
                    AiRunRepo.events(runId, after).forEach {
                        write(it.toSse())
                        flush()
                    }
                    return@respondTextWriter
                }
                try {
                    while (true) {
                        val event = withTimeoutOrNull(15_000) { channel.receiveCatching().getOrNull() }
                        if (event == null) {
                            if (bus.isClosed(runId)) break
                            write("event: ${RunEventType.HEARTBEAT}\ndata: {}\n\n")
                            flush()
                            continue
                        }
                        write(event.toSse())
                        flush()
                        if (event.type in TERMINAL_EVENTS) break
                    }
                } finally {
                    bus.unsubscribe(runId, channel)
                }
            }
        }

        post("/runs/{id}/cancel") {
            val runId = call.parameters["id"].orEmpty()
            val run = AiRunRepo.get(runId) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Run 不存在: $runId")
            if (run.status != "running") {
                call.respond(CancelResult(cancelled = false, status = run.status))
                return@post
            }
            val cancelled = runner.cancel(runId)
            call.respond(CancelResult(cancelled = cancelled, status = if (cancelled) "cancelling" else run.status))
        }

        // -------------------------------------------------------------------
        //  附件准入预检（AIH-029 / AIH-030）
        //
        //  这是纯计算：不产生任何上游请求。前端发送前先调它，
        //  后端 Run 准入还会用同一份规则再验一次。
        // -------------------------------------------------------------------

        post("/preflight") {
            val body = call.receive<PreflightRequest>()
            val provider = requireProvider(body.providerId)
            val api = AiApi.parse(provider.api)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 协议非法")
            val model = AiRepo.getModel(provider.id, body.modelId)
                ?: throw AiException(AiErrorCode.UNKNOWN_MODEL, "模型不存在于目录中: ${body.modelId}")
            val transports = AdapterCapabilities.transportsFor(api)

            val results = body.attachments.mapIndexed { index, fact ->
                val head = fact.headBase64?.let { raw ->
                    runCatching { java.util.Base64.getDecoder().decode(raw) }.getOrNull()
                }
                val resolved = StrictIntake.resolve(
                    name = fact.name,
                    declaredModality = fact.modality,
                    declaredMime = fact.mimeType,
                    sizeBytes = fact.sizeBytes,
                    pixels = fact.pixels,
                    head = head,
                )
                val admission = AttachmentPolicy.evaluate(
                    model = model,
                    attachment = resolved.fact,
                    adapterTransports = transports,
                    currentAttachmentCount = index,
                )
                // 文件头判定失败的阻断理由必须一起带出来（AIH-027/030）
                admission.copy(blockers = resolved.blockers + admission.blockers)
            }
            call.respond(
                PreflightResponse(
                    allowed = results.all { it.allowed },
                    blockers = results.flatMap { it.blockers },
                )
            )
        }
    }
}

// ---------------------------------------------------------------------------
//  请求 / 响应 DTO
// ---------------------------------------------------------------------------

@Serializable
data class CredentialSetRequest(val value: String = "")

/**
 * 删除结果。**不要**用 `mapOf("deleted" to true, "id" to id)` ——
 * kotlinx.serialization 不支持「元素类型不同的集合」，运行时会 500。
 */
@Serializable
data class DeleteResult(val deleted: Boolean, val id: String)

/** Run 的终态事件：SSE 流到这里就该关闭了。 */
private val TERMINAL_EVENTS = setOf(
    RunEventType.RUN_COMPLETED,
    RunEventType.RUN_FAILED,
    RunEventType.RUN_CANCELLED,
)

@Serializable
data class CancelResult(val cancelled: Boolean, val status: String)

/**
 * 模型发现候选（AIH-009）。
 *
 * 能力字段是**预填建议**而不是断言：`capabilitySource` 告诉用户判断从哪来
 * （接口声明 / 内置目录 / 未识别），用户可以在加入前改。
 */
@Serializable
data class ModelCandidateDto(
    val id: String,
    val displayName: String,
    val contextWindow: Int? = null,
    val maxOutputTokens: Int? = null,
    val modalities: List<String> = listOf("text"),
    val tools: Boolean = false,
    val reasoning: Boolean = false,
    /** discovered / builtin / unknown */
    val capabilitySource: String = "unknown",
    val capabilityNote: String? = null,
)

@Serializable
data class DiscoverModelsResponse(
    val ok: Boolean,
    val errorCode: String? = null,
    val message: String,
    val candidates: List<ModelCandidateDto> = emptyList(),
)

/** 连接测试结果：只含状态与稳定错误码，**不含任何密钥或原始 Header**。 */
@Serializable
data class ProviderTestResponse(
    val ok: Boolean,
    val errorCode: String? = null,
    val message: String,
    val httpStatus: Int? = null,
    val modelCount: Int? = null,
)

@Serializable
data class AttachmentFactDto(
    val name: String,
    /** text / image / video / audio / document；未知类型传 null */
    val modality: String? = null,
    val mimeType: String = "application/octet-stream",
    val sizeBytes: Long = 0,
    val pixels: Long? = null,
    /** 文件头若干字节（base64，≤4KB）：**有它就以服务端签名判定为准**，前端声明仅作线索 */
    val headBase64: String? = null,
)

@Serializable
data class PreflightRequest(
    val providerId: String,
    val modelId: String,
    val attachments: List<AttachmentFactDto> = emptyList(),
)

@Serializable
data class PreflightResponse(val allowed: Boolean, val blockers: List<String>)

// ---------------------------------------------------------------------------
//  内部
// ---------------------------------------------------------------------------

private fun requireProvider(id: String): AiProviderDto =
    AiRepo.getProvider(id) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 不存在: $id")

private fun AiProviderDto.withCredential(credentials: CredentialService): AiProviderDto =
    copy(credential = credentials.describe(credentialRef))

private fun hostOf(url: String): String =
    runCatching { java.net.URI(url.trim()).host.orEmpty() }.getOrDefault("")

private fun validateModel(m: AiModelDto) {
    AiValidation.requireModelId(m.id)
    AiValidation.requireDisplayName(m.displayName)
    val bad = m.inputModalities.filter { Modality.parse(it) == null }
    if (bad.isNotEmpty()) {
        throw AiException(AiErrorCode.CONFIG_ERROR, "未知的输入模态: ${bad.joinToString()}")
    }
    m.attachmentTransports.keys.forEach { key ->
        if (Modality.parse(key) == null) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "未知的传输方式键: $key")
        }
        m.attachmentTransports[key].orEmpty().forEach { t ->
            if (Transport.parse(t) == null) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "未知的传输方式: $t")
            }
        }
    }
    if (m.capabilitySource !in setOf("builtin", "discovered", "manual", "tested")) {
        throw AiException(AiErrorCode.CONFIG_ERROR, "非法的能力来源: ${m.capabilitySource}")
    }
    AiValidation.validateThinkingEfforts(m)
}

/**
 * 协议适配器"实际实现"的传输方式（AIH-028）。
 *
 * 事实来源是 [Adapters]：适配器里 `transports` 为空集，预检就把附件判为
 * "协议适配尚未实现"而不是乐观放行。实现了图片内联之后改适配器即可，
 * 不需要在别处再维护一份"支持矩阵"。
 */
object AdapterCapabilities {
    fun transportsFor(api: AiApi): Set<Transport> {
        val ref = com.comfyhub.ai.protocol.AiApiRef.parse(api.wire) ?: return emptySet()
        val adapter = Adapters.of(ref) ?: return emptySet()
        return adapter.transports.mapNotNull { t -> Transport.parse(t.wire) }.toSet()
    }

    /** 调试/界面用：哪些协议真的能跑。 */
    fun implemented(): List<String> = Adapters.supported().map { it.wire }

    fun transportRefs(api: AiApi): Set<TransportRef> =
        com.comfyhub.ai.protocol.AiApiRef.parse(api.wire)?.let { Adapters.of(it)?.transports } ?: emptySet()
}
