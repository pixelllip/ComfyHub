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
                       mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                       context_window, max_output_tokens,
                       max_attachment_bytes, max_attachment_count, capability_source, capability_verified_at, enabled)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                    """.trimIndent(),
                    m.providerId, m.id, m.displayName,
                    AppJson.encodeToString(JsonArray.serializer(), JsonArray(m.inputModalities.map { JsonPrimitive(it) })),
                    AppJson.encodeToString(
                        JsonObject.serializer(),
                        JsonObject(m.attachmentTransports.mapValues { (_, v) -> JsonArray(v.map { JsonPrimitive(it) }) })
                    ),
                    AppJson.encodeToString(JsonArray.serializer(), JsonArray(m.mimeAllowlist.map { JsonPrimitive(it) })),
                    m.tools, m.parallelTools, m.reasoning,
                    AppJson.encodeToString(
                        JsonObject.serializer(),
                        JsonObject(m.thinkingEfforts.mapValues { (_, v) -> JsonPrimitive(v) })
                    ),
                    m.thinkingFormat?.takeIf { it.isNotBlank() },
                    m.contextWindow, m.maxOutputTokens,
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

    /**
     * **只插一条**模型；不 DELETE、不改同 provider 下的其它模型（内置目录"补齐"用）。
     *
     * 与 [replaceModels] 的区别就在这：主键是 `(provider_id, model_id)`，所以"库里已经有同 id 的行"
     * 时这里会直接主键冲突报错 —— 调用方必须先查库、只把**缺失**的那些传进来，
     * 这正是"绝不覆盖用户改过的行"所需要的保护（[com.comfyhub.ai.AiSeeder] 就是这么用的）。
     */
    fun insertModel(m: AiModelDto): AiModelDto = Db.withConnection { conn ->
        conn.execute(
            """
            INSERT INTO ai_models
              (provider_id, model_id, display_name, input_modalities, attachment_transports,
               mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
               context_window, max_output_tokens,
               max_attachment_bytes, max_attachment_count, capability_source, capability_verified_at, enabled)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            """.trimIndent(),
            m.providerId, m.id, m.displayName,
            encodeStringArray(m.inputModalities),
            encodeStringListMap(m.attachmentTransports),
            encodeStringArray(m.mimeAllowlist),
            m.tools, m.parallelTools, m.reasoning,
            encodeStringMap(m.thinkingEfforts),
            m.thinkingFormat?.takeIf { it.isNotBlank() },
            m.contextWindow, m.maxOutputTokens,
            m.maxAttachmentBytes, m.maxAttachmentCount, m.capabilitySource, m.capabilityVerifiedAt, m.enabled
        )
        getModel(m.providerId, m.id) ?: error("模型写入后读不到: ${m.providerId}/${m.id}")
    }

    /**
     * **只更新"能力声明"相关的列**（内置目录对齐用）。
     *
     * 刻意不碰 `display_name` / `enabled` / 附件相关列 / `capability_verified_at`：
     * 用户改过的显示名、用户关掉的模型、用户的实测时间，都不是"对齐能力"该动的东西。
     */
    fun updateModelCapabilities(m: AiModelDto): AiModelDto = Db.withConnection { conn ->
        conn.execute(
            """
            UPDATE ai_models
               SET input_modalities = ?, tools = ?, parallel_tools = ?, reasoning = ?,
                   thinking_efforts = ?, thinking_format = ?, context_window = ?,
                   capability_source = ?
             WHERE provider_id = ? AND model_id = ?
            """.trimIndent(),
            encodeStringArray(m.inputModalities),
            m.tools, m.parallelTools, m.reasoning,
            encodeStringMap(m.thinkingEfforts),
            m.thinkingFormat?.takeIf { it.isNotBlank() },
            m.contextWindow,
            m.capabilitySource,
            m.providerId, m.id
        )
        getModel(m.providerId, m.id) ?: error("模型更新后读不到: ${m.providerId}/${m.id}")
    }

    // JSON 列编码（insertModel / updateModelCapabilities 用；replaceModels 的历史写法保持不动）

    private fun encodeStringArray(values: List<String>): String =
        AppJson.encodeToString(JsonArray.serializer(), JsonArray(values.map { JsonPrimitive(it) }))

    private fun encodeStringMap(values: Map<String, String>): String =
        AppJson.encodeToString(JsonObject.serializer(), JsonObject(values.mapValues { (_, v) -> JsonPrimitive(v) }))

    private fun encodeStringListMap(values: Map<String, List<String>>): String =
        AppJson.encodeToString(
            JsonObject.serializer(),
            JsonObject(values.mapValues { (_, v) -> JsonArray(v.map { JsonPrimitive(it) }) })
        )

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
        thinkingEfforts = AppJson.parseStringMap(getString("thinking_efforts")),
        thinkingFormat = getString("thinking_format")?.takeIf { it.isNotBlank() },
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

/** 思考等级表：值既可能是改名（字符串）也可能是预算（数字），统一按字符串存读（AIH-056）。 */
internal fun kotlinx.serialization.json.Json.parseStringMap(raw: String?): Map<String, String> {
    if (raw.isNullOrBlank()) return emptyMap()
    return runCatching {
        (parseToJsonElement(raw) as? JsonObject)
            ?.mapNotNull { (k, v) -> (v as? JsonPrimitive)?.contentOrNull?.let { k to it } }
            ?.toMap()
            ?: emptyMap()
    }.getOrDefault(emptyMap())
}
