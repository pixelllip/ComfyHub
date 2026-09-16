package com.comfyhub.ai

import com.comfyhub.ApiError
import com.comfyhub.ai.protocol.Adapters
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.TransportRef
import com.comfyhub.ai.tools.SkillDto
import com.comfyhub.ai.tools.SkillStore
import com.comfyhub.ai.tools.ToolApprovalGate
import com.comfyhub.ai.tools.ToolInfoDto
import com.comfyhub.ai.tools.ToolPolicy
import com.comfyhub.ai.tools.ToolPolicyConfig
import com.comfyhub.ai.tools.ToolRegistry
import io.ktor.http.ContentType
import java.nio.file.Path
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
    skills: SkillStore,
    toolRegistry: ToolRegistry,
    approvals: ToolApprovalGate,
    projectRoot: Path,
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
                            thinkingEfforts = it.thinkingEfforts,
                            thinkingFormat = it.thinkingFormat,
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

            // 思考强度：先落成枚举（非法值直接拒绝，不能悄悄当 off，AIH-056）。
            // **缺省字段 = off**：DTO 注释与 `AiRunRepo` 都是这么声明的，前端在"模型不支持推理"
            // 时本来就不传这个字段 —— 早先这里对 null 直接抛错，等于任何不支持推理的模型都发不出消息。
            val requestedEffort = if (body.reasoningEffort == null || body.reasoningEffort.isBlank()) {
                ReasoningEffort.OFF
            } else {
                ReasoningEffort.parse(body.reasoningEffort)
                    ?: throw AiException(
                        AiErrorCode.CONFIG_ERROR,
                        "未知的思考强度：${body.reasoningEffort}（可选 ${ReasoningEffort.entries.joinToString(" / ") { it.wire }}）"
                    )
            }
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
                // Skills 快照（名称 + digest）：AIH-047 要求事后能看出当时用的是哪版规则
                skillSnapshot = run {
                    val current = skills.catalog()
                    buildJsonObject {
                        put("count", current.size)
                        put(
                            "skills",
                            kotlinx.serialization.json.buildJsonArray {
                                current.forEach { s ->
                                    add(
                                        buildJsonObject {
                                            put("name", s.name)
                                            put("digest", s.digest)
                                            put("source", s.source)
                                        }
                                    )
                                }
                            },
                        )
                    }
                },
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
        //  Skills（M5 / AIH-037 ~ AIH-045）
        //
        //  磁盘是正文真源，这里只做"列表 / 读 / 写 / 删 / 导入"四件事；
        //  **不缓存**：AI 刚注册完、用户在右侧栏刚删掉，下一次请求就能看到。
        // -------------------------------------------------------------------

        get("/skills") {
            call.respond(skills.scan())
        }

        get("/skills/{name}") {
            val name = call.parameters["name"].orEmpty()
            val (dto, content) = toolGuard {
                skills.read(name) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "没有名为 $name 的 skill")
            }
            call.respond(SkillDetailDto(skill = dto, content = content))
        }

        post("/skills") {
            val body = call.receive<SkillUpsertRequest>()
            val dto = toolGuard {
                skills.save(
                    name = body.name,
                    description = body.description,
                    whenToUse = body.whenToUse,
                    content = body.content,
                    origin = "ui",
                )
            }
            call.respond(HttpStatusCode.Created, dto)
        }

        delete("/skills/{name}") {
            val name = call.parameters["name"].orEmpty()
            val deleted = toolGuard { skills.delete(name) }
            call.respond(DeleteResult(deleted = deleted, id = name))
        }

        /** 把本机 `%USERPROFILE%\.dsh\skills` 里的 skills 复制进来（**用户显式动作**）。 */
        post("/skills/import-dsh") {
            val dir = SkillStore.dshRoot()
                ?: throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "本机没有 %USERPROFILE%\\.dsh\\skills 目录，没什么可导入的",
                )
            call.respond(toolGuard { skills.importFrom(dir, originLabel = "dsh") })
        }

        // -------------------------------------------------------------------
        //  工具与权限（M4，用户要求：默认不能改 comfy 目录以外的内容）
        // -------------------------------------------------------------------

        get("/tools") {
            val policy = ToolPolicy.load(projectRoot)
            call.respond(toolRegistry.info(policy))
        }

        get("/tools/policy") {
            val policy = ToolPolicy.load(projectRoot)
            call.respond(
                ToolPolicyDto(
                    writeRoots = policy.writeRoots.map { it.toString() },
                    readRoots = policy.readRoots.map { it.toString() },
                    overrides = policy.config.overrides,
                    maxToolSteps = policy.config.maxToolSteps,
                    maxCallsPerRun = policy.config.maxCallsPerRun,
                    maxComfyQueriesPerRun = policy.config.maxComfyQueriesPerRun,
                    maxReadBytes = policy.config.maxReadBytes,
                    maxWriteBytes = policy.config.maxWriteBytes,
                    defaultWriteRoot = projectRoot.resolve(ToolPolicyConfig.DEFAULT_WRITE_DIR).toString(),
                )
            )
        }

        put("/tools/policy") {
            val body = call.receive<ToolPolicyUpdate>()
            val current = ToolPolicy.load(projectRoot).config
            // 只做"校验 + 存"，真正的路径判定每次调用现算（改了立刻生效，不用重启）
            val next = current.copy(
                writeRoots = body.writeRoots ?: current.writeRoots,
                readRoots = body.readRoots ?: current.readRoots,
                overrides = body.overrides ?: current.overrides,
                maxToolSteps = (body.maxToolSteps ?: current.maxToolSteps).coerceIn(1, 24),
                maxCallsPerRun = (body.maxCallsPerRun ?: current.maxCallsPerRun).coerceIn(1, 100),
            )
            ToolPolicy.save(next)
            call.respond(
                ToolPolicyDto(
                    writeRoots = ToolPolicy(projectRoot, next).writeRoots.map { it.toString() },
                    readRoots = ToolPolicy(projectRoot, next).readRoots.map { it.toString() },
                    overrides = next.overrides,
                    maxToolSteps = next.maxToolSteps,
                    maxCallsPerRun = next.maxCallsPerRun,
                    maxReadBytes = next.maxReadBytes,
                    maxWriteBytes = next.maxWriteBytes,
                    defaultWriteRoot = projectRoot.resolve(ToolPolicyConfig.DEFAULT_WRITE_DIR).toString(),
                )
            )
        }

        // -------------------------------------------------------------------
        //  内置模型目录（用户要求：把 settings.yaml 里的模型抄进我们自己的库）
        //
        //  运行时只读 classpath 里的冻结副本；这两个接口让**用户**决定什么时候把库里的
        //  旧数据与那份副本对齐（启动时只做"补齐缺失模型"，绝不覆盖用户改过的行）。
        // -------------------------------------------------------------------

        /** 只读预览：现在同步会发生什么（不写库）。界面按钮拿它弹确认框。 */
        get("/builtin/status") {
            call.respond(AiSeeder.previewBuiltinSync().toDto())
        }

        post("/builtin/sync") {
            val body = call.receive<BuiltinSyncRequest>()
            val mode = when (body.mode?.lowercase()) {
                null, "", "add-missing" -> SeedMode.ADD_MISSING
                "refresh-capabilities", "refresh" -> SeedMode.REFRESH_CAPABILITIES
                else -> throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "未知的同步模式：${body.mode}（可选 add-missing / refresh-capabilities）",
                )
            }
            call.respond(AiSeeder.syncBuiltinProvider(mode).toDto())
        }

        /** 工具卡上的「批准」（AIH-035 / AIH-049）。 */
        post("/tool-calls/{callId}/approve") {            val callId = call.parameters["callId"].orEmpty()
            call.respond(ToolApprovalResult(callId, approved = true, accepted = runner.resolveApproval(callId, true)))
        }

        post("/tool-calls/{callId}/deny") {
            val callId = call.parameters["callId"].orEmpty()
            call.respond(ToolApprovalResult(callId, approved = false, accepted = runner.resolveApproval(callId, false)))
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

// ---------------------------------------------------------------------------
//  Skills / 工具（M4 / M5）
// ---------------------------------------------------------------------------

@Serializable
data class SkillUpsertRequest(
    val name: String,
    val description: String,
    val whenToUse: String? = null,
    val content: String,
)

/** skill 详情：元数据 + 正文（正文只在打开详情时读，列表不读） */
@Serializable
data class SkillDetailDto(val skill: SkillDto, val content: String)

/** 工具权限视图（界面用）：写/读白名单 + 逐工具覆盖 + 预算 */
@Serializable
data class ToolPolicyDto(
    val writeRoots: List<String>,
    val readRoots: List<String>,
    val overrides: Map<String, String> = emptyMap(),
    val maxToolSteps: Int = 8,
    val maxCallsPerRun: Int = 16,
    /** AIH-036：一次回复最多主动查 ComfyUI 几次 */
    val maxComfyQueriesPerRun: Int = 3,
    val maxReadBytes: Int = 0,
    val maxWriteBytes: Int = 0,
    /** 出厂默认的写入目录（界面里显示"默认只能写这里"） */
    val defaultWriteRoot: String? = null,
)

@Serializable
data class ToolPolicyUpdate(
    val writeRoots: List<String>? = null,
    val readRoots: List<String>? = null,
    val overrides: Map<String, String>? = null,
    val maxToolSteps: Int? = null,
    val maxCallsPerRun: Int? = null,
)

@Serializable
data class ToolApprovalResult(val callId: String, val approved: Boolean, val accepted: Boolean)

// ---------------------------------------------------------------------------
//  内置模型目录的同步（ADD_MISSING / REFRESH_CAPABILITIES）
// ---------------------------------------------------------------------------

@Serializable
data class BuiltinSyncRequest(val mode: String? = null)

@Serializable
data class BuiltinSyncResult(
    /** add-missing / refresh-capabilities */
    val mode: String,
    /** 冻结目录的版本号（`server/src/main/resources/ai/builtin-catalog.json`） */
    val version: String,
    val providerId: String? = null,
    /** 这次新增的模型数 */
    val added: Int = 0,
    /** 这次被对齐能力声明的模型数（只有 refresh 模式会 > 0） */
    val updated: Int = 0,
    /** 与目录一致、未改动的模型数 */
    val kept: Int = 0,
    /** 库里已有但能力声明与内置目录不同的 model_id（保持用户当前设置，等用户决定） */
    val divergent: List<String> = emptyList(),
    /** 库里该 provider 当前的模型总数 */
    val modelCount: Int = 0,
    val providerCreated: Boolean = false,
    val error: String? = null,
)

private fun SeedSyncOutcome.toDto() = BuiltinSyncResult(
    mode = if (mode == SeedMode.REFRESH_CAPABILITIES) "refresh-capabilities" else "add-missing",
    version = version,
    providerId = providerId,
    added = added,
    updated = updated,
    kept = kept,
    divergent = divergent,
    modelCount = modelCount,
    providerCreated = providerCreated,
    error = error,
)

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
    /** 预填的思考档位：等级 → 线上表达（来自内置目录，可改）。 */
    val thinkingEfforts: Map<String, String> = emptyMap(),
    /** 预填的思考方言；null = 按协议默认。 */
    val thinkingFormat: String? = null,
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

/**
 * 工具层的 [ToolFailure] → [AiException]。
 *
 * 工具层用的是自己的稳定 code（`PATH_DENIED` / `INVALID_SKILL_NAME` …），
 * 直接抛出去会落到 StatusPages 的 Throwable 分支变成 500；这里换成带 code 的 AI 异常，
 * 前端就能把"为什么被拒绝"原样显示给用户。
 */
private inline fun <T> toolGuard(block: () -> T): T = try {
    block()
} catch (e: com.comfyhub.ai.tools.ToolFailure) {
    throw AiException(e.code, e.message ?: "工具层拒绝了这次操作")
}

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
