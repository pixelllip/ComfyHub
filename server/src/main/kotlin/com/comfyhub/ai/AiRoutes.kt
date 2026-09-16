package com.comfyhub.ai

import com.comfyhub.ApiError
import com.comfyhub.FailedInfo
import com.comfyhub.ai.protocol.Adapters
import com.comfyhub.ai.protocol.AttachmentKindRef
import com.comfyhub.ai.protocol.ReasoningEffort
import com.comfyhub.ai.protocol.TransportRef
import com.comfyhub.ai.tools.MemoryStore
import com.comfyhub.ai.tools.SkillDto
import com.comfyhub.ai.tools.SkillRescanDto
import com.comfyhub.ai.tools.SkillStore
import com.comfyhub.ai.tools.ToolApprovalGate
import com.comfyhub.ai.tools.ToolInfoDto
import com.comfyhub.ai.tools.ToolPolicy
import com.comfyhub.ai.tools.ToolPolicyConfig
import com.comfyhub.ai.tools.ToolRegistry
import com.comfyhub.contentTypeFor
import com.comfyhub.setInlineFileHeaders
import io.ktor.http.ContentType
import java.nio.file.Files
import java.nio.file.Path
import io.ktor.http.HttpStatusCode
import io.ktor.http.content.PartData
import io.ktor.http.content.forEachPart
import io.ktor.server.request.receive
import io.ktor.server.request.receiveMultipart
import io.ktor.server.response.respond
import io.ktor.server.response.respondFile
import io.ktor.server.response.respondTextWriter
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.patch
import io.ktor.server.routing.post
import io.ktor.server.routing.put
import io.ktor.server.routing.route
import io.ktor.utils.io.jvm.javaio.toInputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory

private val log = LoggerFactory.getLogger("com.comfyhub.AiRoutes")

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
    memory: MemoryStore,
    toolRegistry: ToolRegistry,
    approvals: ToolApprovalGate,
    projectRoot: Path,
    /** AI 附件（M3）：上传 / 缩略图 / 删 / 读成内联附件都走它 */
    attachments: AiAttachmentStore,
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
            val attachmentIds = body.attachmentIds.distinct()
            if (body.text.isBlank() && attachmentIds.isEmpty()) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "消息内容不能为空")
            }

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

            // ---- 附件准入（AIH-030 的"事务内再用快照验一次"）-------------------
            // 前端预检只是**提前告知**；真正的放行判定在这里、在创建 Run **之前**完成：
            // 不通过就直接报错返回，不会有任何上游请求（零请求证明见 e2e 脚本）。
            val api = AiApi.parse(provider.api)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 协议非法: ${provider.api}")
            val attached = attachmentIds.map { id ->
                AiAttachmentRepo.get(id)
                    ?: throw AiException(AiErrorCode.CONFIG_ERROR, "附件不存在或已被删除: $id")
            }
            val attachmentBlockers = attached.flatMapIndexed { index, dto ->
                val fact = dto.toFact()
                val transports = fact.modality?.let { AdapterCapabilities.transportsFor(api, it) }.orEmpty()
                AttachmentPolicy.evaluate(model, fact, transports, currentAttachmentCount = index).let { result ->
                    result.blockers.map { "${dto.name}：$it" }
                }
            }
            if (attachmentBlockers.isNotEmpty()) {
                throw AiException(
                    AiErrorCode.UNSUPPORTED_CONTENT,
                    "有附件未通过准入，已阻断发送（未产生任何上游请求）：${attachmentBlockers.joinToString("；")}",
                )
            }

            // 用户消息与助手占位都在"创建 Run"里完成，保证顺序与 seq 稳定
            val userMessage = AiConversationRepo.appendMessage(
                conversationId,
                AiMessageAppend(
                    role = "user",
                    text = body.text,
                    parts = buildList {
                        add(AiMessagePartDto(type = "text", text = body.text))
                        // 附件的引用落成有序块：重开 App 还要能把缩略图/预览帧显示出来
                        attached.forEach { add(AiMessagePartDto(type = "attachment", text = it.name, attachmentId = it.id)) }
                    },
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

        /**
         * skills 投放口的位置。界面上要显示绝对路径 —— 用户得知道往哪个文件夹拷。
         *
         * 两种布局都由 `storageDir` 决定：源码树是 `<项目根>\storage\ai\skills`，
         * 发布包是 `<根>\storage\ai\skills`（便携式，跟着包走）。
         */
        get("/skills/roots") {
            call.respond(skills.roots())
        }

        /**
         * 重新扫描投放口：给"拷进来但没写 frontmatter"的 skill 自动补上并登记，
         * 然后返回最新列表。
         *
         * 启动时后端已经扫过一次（见 Application.module）；这里是给
         * "应用开着的时候又拷进来一个"用的，不用重启。
         */
        post("/skills/rescan") {
            val result = toolGuard { skills.autoRegister() }
            call.respond(
                SkillRescanDto(
                    registered = result.registered,
                    names = result.names,
                    errors = result.errors,
                    skills = skills.scan(),
                )
            )
        }

        // -------------------------------------------------------------------
        //  长期记忆（M6，用户建议）
        //  真源是 `<storage>\ai\memory.md`：人能看懂、能手改，AI 也能用 remember 追加。
        // -------------------------------------------------------------------

        get("/memory") {
            call.respond(memory.read())
        }

        put("/memory") {
            val body = call.receive<MemoryUpdateRequest>()
            call.respond(toolGuard { memory.write(body.content) })
        }

        post("/memory/entries") {
            val body = call.receive<MemoryEntryRequest>()
            call.respond(toolGuard { memory.append(body.content) })
        }

        delete("/memory") {
            call.respond(toolGuard { memory.clear() })
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
        //  附件（M3 / AIH-027 ~ AIH-031）
        //
        //  原件落 `storage/ai-attachments`，缩略图 / 视频预览帧落 `storage/ai-thumbs`，
        //  与画廊产物分开（生命周期不同，混在一起以后清孤儿会互相误删）。
        //  类型判定**只认签名**：认不出来直接拒收，绝不做"未知即图片"的乐观回退。
        // -------------------------------------------------------------------

        post("/attachments") {
            data class Incoming(val name: String, val tmp: Path, val mime: String?)

            val incoming = mutableListOf<Incoming>()
            val failed = mutableListOf<FailedInfo>()
            try {
                call.receiveMultipart(formFieldLimit = AiAttachmentStore.MAX_UPLOAD_BYTES).forEachPart { part ->
                    try {
                        if (part is PartData.FileItem) {
                            val name = part.originalFileName?.takeIf { it.isNotBlank() } ?: "attachment.bin"
                            val tmp = attachments.tempFile()
                            try {
                                part.provider().toInputStream().use { input ->
                                    Files.newOutputStream(tmp).use { out -> input.copyTo(out) }
                                }
                            } catch (e: Exception) {
                                Files.deleteIfExists(tmp)
                                throw e
                            }
                            incoming += Incoming(name, tmp, part.contentType?.toString())
                        }
                    } catch (e: Exception) {
                        log.warn("附件 multipart 分段处理失败: {}", e.message)
                        failed += FailedInfo("(part)", e.message ?: "解析失败")
                    } finally {
                        part.dispose()
                    }
                }
            } catch (e: Exception) {
                incoming.forEach { runCatching { Files.deleteIfExists(it.tmp) } }
                throw AiException(AiErrorCode.CONFIG_ERROR, "附件上传解析失败：${e.message}")
            }

            if (incoming.isEmpty() && failed.isEmpty()) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "没有收到文件（表单字段名请用 files）")
            }

            val items = mutableListOf<AiAttachmentDto>()
            incoming.forEach { item ->
                try {
                    items += attachments.save(item.name, item.tmp, item.mime)
                } catch (e: AiException) {
                    failed += FailedInfo(item.name, e.message ?: "入库失败")
                } catch (e: Exception) {
                    log.error("附件入库失败 {}", item.name, e)
                    runCatching { Files.deleteIfExists(item.tmp) }
                    failed += FailedInfo(item.name, e.message ?: "入库失败")
                }
            }
            // 一个都没成 → 400（前端要能直接看到"为什么这张图发不上去"）；
            // 部分成功仍然 201，失败的逐条列在 failed 里（不静默丢弃）。
            if (items.isEmpty()) {
                throw AiException(
                    AiErrorCode.UNSUPPORTED_CONTENT,
                    failed.joinToString("；") { "${it.fileName}：${it.reason}" }.ifEmpty { "附件入库失败" },
                )
            }
            call.respond(HttpStatusCode.Created, AiAttachmentUploadResult(items = items, failed = failed))
        }

        get("/attachments/{id}") {
            val id = call.parameters["id"].orEmpty()
            val dto = AiAttachmentRepo.get(id)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "附件不存在: $id")
            call.respond(dto)
        }

        /** 原件（点开看大图 / 视频播放都走它，支持 Range 以便视频拖动）。 */
        get("/attachments/{id}/file") {
            val id = call.parameters["id"].orEmpty()
            val dto = AiAttachmentRepo.get(id)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "附件不存在: $id")
            val path = attachments.fileOf(id)
                ?: throw AiException(AiErrorCode.CONFIG_ERROR, "附件文件已丢失: ${dto.name}")
            call.setInlineFileHeaders(dto.name, contentTypeFor(dto.mimeType))
            call.respondFile(path.toFile())
        }

        /**
         * 缩略图 / 视频预览帧：**同一张接口**，界面按 `kind` 决定显示成缩略图还是播放器封面。
         * 没有可看的图（音频 / 文档 / 抽帧失败）回 204，界面退化成文件图标（不是破图）。
         */
        get("/attachments/{id}/thumb") {
            val id = call.parameters["id"].orEmpty()
            AiAttachmentRepo.get(id) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "附件不存在: $id")
            // 抽帧要起子进程，放到 IO 线程上，别占着事件循环
            val thumb = withContext(Dispatchers.IO) { attachments.thumbnailOf(id) }
                ?: return@get call.respond(HttpStatusCode.NoContent)
            call.response.headers.append("X-Thumbnail", "1")
            call.setInlineFileHeaders("thumb-$id", contentTypeFor(thumb.second))
            call.respondFile(thumb.first.toFile())
        }

        delete("/attachments/{id}") {
            val id = call.parameters["id"].orEmpty()
            val deleted = attachments.delete(id)
            if (!deleted) throw AiException(AiErrorCode.CONFIG_ERROR, "附件不存在: $id")
            call.respond(DeleteResult(deleted = true, id = id))
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

            // 两种入参：已上传的 id（**库里的事实为准**）或前端直接给的线索（老用法）。
            // 严格分类器都在这一层跑：前端谎报 image 骗不过准入（AIH-027）。
            data class Item(val dto: AiAttachmentDto?, val resolved: StrictIntake.Resolved, val id: String?)

            val items = mutableListOf<Item>()
            body.attachmentIds.distinct().forEach { id ->
                val dto = AiAttachmentRepo.get(id)
                if (dto == null) {
                    items += Item(
                        dto = null,
                        resolved = StrictIntake.Resolved(
                            AttachmentFact(name = id, modality = null, mimeType = "application/octet-stream", sizeBytes = 0),
                            listOf("附件不存在或已被删除：$id"),
                        ),
                        id = id,
                    )
                } else {
                    items += Item(dto = dto, resolved = StrictIntake.Resolved(dto.toFact(), emptyList()), id = id)
                }
            }
            body.attachments.forEach { fact ->
                val head = fact.headBase64?.let { raw ->
                    runCatching { java.util.Base64.getDecoder().decode(raw) }.getOrNull()
                }
                items += Item(
                    dto = null,
                    resolved = StrictIntake.resolve(
                        name = fact.name,
                        declaredModality = fact.modality,
                        declaredMime = fact.mimeType,
                        sizeBytes = fact.sizeBytes,
                        pixels = fact.pixels,
                        head = head,
                    ),
                    id = null,
                )
            }

            val results = items.mapIndexed { index, item ->
                val fact = item.resolved.fact
                val transports = fact.modality?.let { AdapterCapabilities.transportsFor(api, it) }.orEmpty()
                val admission = AttachmentPolicy.evaluate(
                    model = model,
                    attachment = fact,
                    adapterTransports = transports,
                    currentAttachmentCount = index,
                )
                // 文件头判定失败的阻断理由必须一起带出来（AIH-027/030）
                admission.copy(blockers = item.resolved.blockers + admission.blockers)
            }
            call.respond(
                PreflightResponse(
                    allowed = results.all { it.allowed },
                    blockers = results.flatMap { it.blockers },
                    items = items.mapIndexed { index, item ->
                        PreflightItemDto(
                            index = index,
                            name = item.resolved.fact.name,
                            allowed = results[index].allowed,
                            blockers = results[index].blockers,
                            attachmentId = item.id,
                        )
                    },
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

/** 长期记忆：整篇替换（界面「保存」） */
@Serializable
data class MemoryUpdateRequest(val content: String = "")

/** 长期记忆：追加一条（界面「+ 添加一条」；AI 走 remember 工具，不经过这里） */
@Serializable
data class MemoryEntryRequest(val content: String = "")

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
    /** 前端直接给的线索（可含文件头）；有 [attachmentIds] 时以库里的为准 */
    val attachments: List<AttachmentFactDto> = emptyList(),
    /** 已上传附件的 id（M3）：**服务端以库里的事实为准**，前端声明只是线索 */
    val attachmentIds: List<String> = emptyList(),
)

/** 逐个附件的准入结论：界面用它给对应的缩略图打红框 / 显示原因。 */
@Serializable
data class PreflightItemDto(
    val index: Int,
    val name: String,
    val allowed: Boolean,
    val blockers: List<String> = emptyList(),
    val attachmentId: String? = null,
)

@Serializable
data class PreflightResponse(
    val allowed: Boolean,
    val blockers: List<String>,
    /** 与请求的附件顺序一一对应 */
    val items: List<PreflightItemDto> = emptyList(),
)

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
 * 事实来源是 [Adapters] 的 `attachmentTransports`：**按模态分开**声明，
 * 没实现的那种模态预检就判"协议适配尚未实现"而不是乐观放行。实现了新模态（或新传输方式）
 * 之后改适配器即可，不需要在别处再维护一份"支持矩阵"。
 */
object AdapterCapabilities {
    fun transportsFor(api: AiApi, modality: Modality): Set<Transport> {
        val ref = com.comfyhub.ai.protocol.AiApiRef.parse(api.wire) ?: return emptySet()
        val adapter = Adapters.of(ref) ?: return emptySet()
        val kind = AttachmentKindRef.parse(modality.wire) ?: return emptySet()
        return adapter.attachmentTransports[kind].orEmpty().mapNotNull { t -> Transport.parse(t.wire) }.toSet()
    }

    /** 调试/界面用：哪些协议真的能跑。 */
    fun implemented(): List<String> = Adapters.supported().map { it.wire }

    /** 该协议**所有**已实现的传输方式（不分模态），只用于展示与排错。 */
    fun transportRefs(api: AiApi): Set<TransportRef> =
        com.comfyhub.ai.protocol.AiApiRef.parse(api.wire)?.let { Adapters.of(it)?.attachmentTransports }
            ?.values?.flatten()?.toSet() ?: emptySet()
}
