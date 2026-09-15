package com.comfyhub.ai

import com.comfyhub.ApiError
import io.ktor.http.HttpStatusCode
import io.ktor.server.request.receive
import io.ktor.server.response.respond
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.patch
import io.ktor.server.routing.post
import io.ktor.server.routing.put
import io.ktor.server.routing.route
import kotlinx.serialization.Serializable

/**
 * AI 工作台接口 —— 阶段 1：Provider / 模型目录 / 凭据（M1）。
 *
 * 约定：
 *  - 任何返回 DTO 都不含密钥值，只有 [CredentialStatusDto]（AIH-012）；
 *  - 凭据 `PUT` 时值为空 = 不修改；清除必须显式 `DELETE`（AIH-013）；
 *  - Provider 更新必须带 revision（AIH-007）。
 */
fun Route.aiRoutes(credentials: CredentialService) {

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
}

/**
 * 协议适配器"实际实现"的传输方式。
 *
 * **当前阶段（M1）适配器尚未实现，因此这里诚实地返回空集** —— 预检会把附件判为
 * "协议适配尚未实现"而不是乐观放行。M3 实现图片内联后在此登记，
 * 保证"能不能发"永远以代码事实为准，而不是靠模型能力声明猜。
 */
object AdapterCapabilities {
    fun transportsFor(api: AiApi): Set<Transport> = when (api) {
        AiApi.OPENAI_COMPLETIONS, AiApi.OPENAI_RESPONSES, AiApi.ANTHROPIC_MESSAGES -> emptySet()
    }
}
