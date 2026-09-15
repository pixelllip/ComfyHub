package com.comfyhub.ai

import com.comfyhub.AppJson
import com.comfyhub.Db
import com.comfyhub.execute
import com.comfyhub.isoTime
import com.comfyhub.queryList
import com.comfyhub.queryOne
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import org.slf4j.LoggerFactory
import java.sql.ResultSet

/**
 * AI Provider / 模型目录的持久化（AIH-006 / AIH-007 / AIH-010 / AIH-011）。
 *
 * 凭据值**不在**这里：只有 `credential_ref` 这个名字进库，值由 [CredentialService]
 * 用 DPAPI 单独保管（AIH-012）。
 */
object AiRepo {
    private val log = LoggerFactory.getLogger(AiRepo::class.java)

    // -----------------------------------------------------------------------
    //  Provider
    // -----------------------------------------------------------------------

    fun listProviders(): List<AiProviderDto> = Db.withConnection { conn ->
        conn.queryList(
            "SELECT * FROM ai_providers ORDER BY display_name, id"
        ) { it.toProvider() }
    }

    fun getProvider(id: String): AiProviderDto? = Db.withConnection { conn ->
        conn.queryOne("SELECT * FROM ai_providers WHERE id = ?", id) { it.toProvider() }
    }

    fun insertProvider(p: AiProviderDto): AiProviderDto = Db.withConnection { conn ->
        conn.execute(
            """
            INSERT INTO ai_providers
              (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
            VALUES (?, ?, ?, ?, ?, ?, ?, 1)
            """.trimIndent(),
            p.id, p.displayName, p.api, p.baseURL, p.credentialRef, p.endpointTrust, p.enabled
        )
        getProvider(p.id) ?: error("Provider 写入后读不到: ${p.id}")
    }

    /**
     * 乐观锁更新（AIH-007）：`expectedRevision` 与库中不一致就抛冲突，
     * 避免两个窗口互相覆盖设置。
     */
    fun updateProvider(p: AiProviderDto, expectedRevision: Long): AiProviderDto {
        require(expectedRevision > 0) { "缺少 revision，无法进行并发安全更新" }
        val current = getProvider(p.id) ?: throw AiException(AiErrorCode.CONFIG_ERROR, "Provider 不存在: ${p.id}")
        if (current.revision != expectedRevision) {
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "配置已被其他窗口修改（当前 revision=${current.revision}，提交的=${expectedRevision}），请刷新后重试"
            )
        }
        Db.withConnection { conn ->
            conn.execute(
                """
                UPDATE ai_providers
                   SET display_name = ?, api = ?, base_url = ?, credential_ref = ?,
                       endpoint_trust = ?, enabled = ?, revision = revision + 1
                 WHERE id = ? AND revision = ?
                """.trimIndent(),
                p.displayName, p.api, p.baseURL, p.credentialRef, p.endpointTrust, p.enabled,
                p.id, expectedRevision
            )
        }
        return getProvider(p.id) ?: error("Provider 更新后读不到: ${p.id}")
    }

    fun deleteProvider(id: String): Boolean = Db.withConnection { conn ->
        conn.execute("DELETE FROM ai_providers WHERE id = ?", id) > 0
    }

    // -----------------------------------------------------------------------
    //  模型目录
    // -----------------------------------------------------------------------

    fun listModels(providerId: String): List<AiModelDto> = Db.withConnection { conn ->
        conn.queryList(
            "SELECT * FROM ai_models WHERE provider_id = ? ORDER BY display_name, model_id",
            providerId
        ) { it.toModel() }
    }

    /** 全量替换某 Provider 的模型目录；目录是能力的唯一真源（AIH-011）。 */
    fun replaceModels(providerId: String, models: List<AiModelDto>): List<AiModelDto> {
        val normalized = models.map { m ->
            AiValidation.requireModelId(m.id)
            m.copy(providerId = providerId)
        }
        Db.tx { conn ->
            conn.execute("DELETE FROM ai_models WHERE provider_id = ?", providerId)
            normalized.forEach { m ->
                conn.execute(
                    """
                    INSERT INTO ai_models
                      (provider_id, model_id, display_name, input_modalities, attachment_transports,
                       mime_allowlist, tools, parallel_tools, reasoning, context_window, max_output_tokens,
                       max_attachment_bytes, max_attachment_count, capability_source, capability_verified_at, enabled)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                    """.trimIndent(),
                    m.providerId, m.id, m.displayName,
                    AppJson.encodeToString(JsonArray.serializer(), JsonArray(m.inputModalities.map { JsonPrimitive(it) })),
                    AppJson.encodeToString(
                        JsonObject.serializer(),
                        JsonObject(m.attachmentTransports.mapValues { (_, v) -> JsonArray(v.map { JsonPrimitive(it) }) })
                    ),
                    AppJson.encodeToString(JsonArray.serializer(), JsonArray(m.mimeAllowlist.map { JsonPrimitive(it) })),
                    m.tools, m.parallelTools, m.reasoning, m.contextWindow, m.maxOutputTokens,
                    m.maxAttachmentBytes, m.maxAttachmentCount, m.capabilitySource, m.capabilityVerifiedAt, m.enabled
                )
            }
        }
        log.info("已更新 Provider {} 的模型目录（{} 个）", providerId, normalized.size)
        return listModels(providerId)
    }

    fun getModel(providerId: String, modelId: String): AiModelDto? = Db.withConnection { conn ->
        conn.queryOne(
            "SELECT * FROM ai_models WHERE provider_id = ? AND model_id = ?",
            providerId, modelId
        ) { it.toModel() }
    }

    // -----------------------------------------------------------------------
    //  行映射
    // -----------------------------------------------------------------------

    private fun ResultSet.toProvider() = AiProviderDto(
        id = getString("id"),
        displayName = getString("display_name"),
        api = getString("api"),
        baseURL = getString("base_url"),
        credentialRef = getString("credential_ref"),
        endpointTrust = getString("endpoint_trust"),
        enabled = getBoolean("enabled"),
        revision = getLong("revision"),
        // 真正的状态由路由层用 CredentialService 填充，DB 里永远没有值
        credential = CredentialStatusDto(configured = false, source = "none", writable = false),
        createdAt = isoTime("created_at"),
        updatedAt = isoTime("updated_at"),
    )

    private fun ResultSet.toModel() = AiModelDto(
        providerId = getString("provider_id"),
        id = getString("model_id"),
        displayName = getString("display_name") ?: getString("model_id"),
        inputModalities = AppJson.parseStringArray(getString("input_modalities")),
        attachmentTransports = AppJson.parseStringListMap(getString("attachment_transports")),
        mimeAllowlist = AppJson.parseStringArray(getString("mime_allowlist")),
        tools = getBoolean("tools"),
        parallelTools = getBoolean("parallel_tools"),
        reasoning = getBoolean("reasoning"),
        contextWindow = getIntOrNull("context_window"),
        maxOutputTokens = getIntOrNull("max_output_tokens"),
        maxAttachmentBytes = getLongOrNull("max_attachment_bytes"),
        maxAttachmentCount = getIntOrNull("max_attachment_count"),
        capabilitySource = getString("capability_source") ?: "manual",
        capabilityVerifiedAt = isoTime("capability_verified_at"),
        enabled = getBoolean("enabled"),
    )

    private fun ResultSet.getIntOrNull(col: String): Int? {
        val v = getInt(col); return if (wasNull()) null else v
    }

    private fun ResultSet.getLongOrNull(col: String): Long? {
        val v = getLong(col); return if (wasNull()) null else v
    }
}

// ---------------------------------------------------------------------------
//  JSON 列的宽松解析：坏数据不能让整个设置页打不开
// ---------------------------------------------------------------------------

internal fun kotlinx.serialization.json.Json.parseStringArray(raw: String?): List<String> {
    if (raw.isNullOrBlank()) return emptyList()
    return runCatching {
        (parseToJsonElement(raw) as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            ?: emptyList()
    }.getOrDefault(emptyList())
}

internal fun kotlinx.serialization.json.Json.parseStringListMap(raw: String?): Map<String, List<String>> {
    if (raw.isNullOrBlank()) return emptyMap()
    return runCatching {
        (parseToJsonElement(raw) as? JsonObject)?.mapValues { (_, v) ->
            (v as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull } ?: emptyList()
        } ?: emptyMap()
    }.getOrDefault(emptyMap())
}
