package com.comfyhub.harness.store

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put

/**
 * Provider / 模型目录的 **DTO JSON** 读写（手机侧）。
 *
 * ## 为什么键名要逐字对齐
 *
 * 这份 JSON 就是 Dart 的 `AiProvider.toJson()` / `AiModel.toJson()`（= 后端
 * `AiProviderDto` / `AiModelDto`）的形状：**同一份形状两端都能读**，于是"手机上存进去的
 * 一行"与"桌面上那一行"在界面上长得一模一样，桥那侧也不用翻译（翻译就是漂移的开始）。
 *
 * 三条纪律：
 *  1. **可空的键：读的时候当没给，写的时候省掉**（`credentialRef` / `thinkingFormat` /
 *     三个上限数字）——写 `null` 与"没给"在两端是两件事；
 *  2. **`credential` 不写**：那是"这台机器上有没有 key"的**即时状态**，手机上是本机密钥库
 *     说了算（界面用 `CredentialStore.statusOf` 现问）。桥**不编一个可能过期的状态**；
 *  3. **读不懂就抛**（`ProviderJsonFailure`）：一行缺 `id` 或者 `inputModalities` 不是数组，
 *     都不该静默变成一条"看起来对、其实空了"的配置 —— 那种错没人查得出来。
 *
 * ⚠ 表里的 `compat_json` 只由本文件负责 `query` 这一段（额外查询参数）；
 * `limits_json` / `max_attachment_bytes` / `capability_verified_at` **不在**这份 DTO 里
 * （Dart 的 DTO 也没有它们），新建/更新时一律写 NULL。
 */
object ProviderJson {

    class ProviderJsonFailure(message: String) : RuntimeException(message)

    // -----------------------------------------------------------------------
    //  Provider
    // -----------------------------------------------------------------------

    fun providerToJson(p: ProviderRecord): JsonObject = buildJsonObject {
        put("id", p.id)
        put("displayName", p.displayName)
        put("api", p.api)
        put("baseURL", p.baseUrl)
        p.credentialRef?.takeIf { it.isNotBlank() }?.let { put("credentialRef", it) }
        put("endpointTrust", p.endpointTrust)
        put("enabled", p.enabled)
        put("revision", p.revision)
        put("createdAt", p.createdAt)
        put("updatedAt", p.updatedAt)
        // 空表不出现这个键（与后端 `skip_serializing_if` 一致）；Dart 缺省按空表读
        if (p.headers.isNotEmpty()) put("headers", p.headers.toJsonMap())
        if (p.query.isNotEmpty()) put("query", p.query.toJsonMap())
    }

    /**
     * 一份 provider JSON → [ProviderRecord]。
     *
     * `revision` **缺省 / 0 = "新建"**（与桌面 `POST` 的语义对齐：新建的 revision 由库给 1）；
     * 带正数 = "更新这一版"（走乐观锁，见 [HarnessStore.saveProvider]）。
     * `enabled` 缺省 = true（与 DDL 的默认值一致）；`endpointTrust` 缺省 = `public`
     * （**不做推断** —— 见 [HarnessStore.saveProvider] 的注释）。
     */
    fun providerFromJson(json: JsonObject): ProviderRecord {
        val id = json.str("id") ?: throw ProviderJsonFailure("provider 缺少 id")
        return ProviderRecord(
            id = id,
            displayName = json.str("displayName") ?: id,
            api = json.str("api") ?: throw ProviderJsonFailure("provider 缺少 api"),
            baseUrl = json.str("baseURL") ?: throw ProviderJsonFailure("provider 缺少 baseURL"),
            credentialRef = json.str("credentialRef"),
            endpointTrust = json.str("endpointTrust") ?: "public",
            enabled = json.bool("enabled") ?: true,
            revision = json.int("revision") ?: 0,
            // 时间列由库给（新建）/ 由更新语句自己写；从 JSON 读进来的这两个值**不用**
            createdAt = json.str("createdAt").orEmpty(),
            updatedAt = json.str("updatedAt").orEmpty(),
            headers = json.stringMap("headers"),
            query = json.stringMap("query"),
        )
    }

    fun providersToJsonArray(rows: List<ProviderRecord>): String =
        buildJsonArray { rows.forEach { add(providerToJson(it)) } }.toString()

    // -----------------------------------------------------------------------
    //  模型目录
    // -----------------------------------------------------------------------

    fun modelToJson(m: ModelRecord): JsonObject = buildJsonObject {
        put("providerId", m.providerId)
        put("id", m.modelId)
        put("displayName", m.displayName)
        put("inputModalities", m.inputModalities.toJsonArray())
        put("attachmentTransports", m.attachmentTransports.toJsonListMap())
        put("mimeAllowlist", m.mimeAllowlist.toJsonArray())
        put("tools", m.tools)
        put("parallelTools", m.parallelTools)
        put("reasoning", m.reasoning)
        put("thinkingEfforts", m.thinkingEfforts.toJsonMap())
        m.thinkingFormat?.takeIf { it.isNotBlank() }?.let { put("thinkingFormat", it) }
        m.contextWindow?.let { put("contextWindow", it) }
        m.maxOutputTokens?.let { put("maxOutputTokens", it) }
        m.maxAttachmentCount?.let { put("maxAttachmentCount", it) }
        put("capabilitySource", m.capabilitySource)
        put("enabled", m.enabled)
    }

    /** 一个模型 JSON → [ModelRecord]（`providerId` 由调用方按**路径/参数**覆盖，见 [HarnessStore.saveModels]）。 */
    fun modelFromJson(json: JsonObject, providerId: String): ModelRecord {
        val id = json.str("id") ?: throw ProviderJsonFailure("模型缺少 id")
        return ModelRecord(
            // ⚠ providerId **一律以调用方给的为准**（与桌面 `replace_models` 同：body 里那一列
            //   是客户端抄来的，信它就会出现"给 A 存了一堆写着 B 的模型"）
            providerId = providerId,
            modelId = id,
            displayName = json.str("displayName") ?: id,
            inputModalities = json.stringList("inputModalities"),
            attachmentTransports = json.stringListMap("attachmentTransports"),
            mimeAllowlist = json.stringList("mimeAllowlist"),
            tools = json.bool("tools") ?: false,
            parallelTools = json.bool("parallelTools") ?: false,
            reasoning = json.bool("reasoning") ?: false,
            thinkingEfforts = json.stringMap("thinkingEfforts"),
            thinkingFormat = json.str("thinkingFormat"),
            contextWindow = json.int("contextWindow"),
            maxOutputTokens = json.int("maxOutputTokens"),
            maxAttachmentCount = json.int("maxAttachmentCount"),
            capabilitySource = json.str("capabilitySource") ?: "manual",
            enabled = json.bool("enabled") ?: true,
        )
    }

    fun modelsToJsonArray(rows: List<ModelRecord>): String =
        buildJsonArray { rows.forEach { add(modelToJson(it)) } }.toString()

    // -----------------------------------------------------------------------
    //  小工具（JSON 文本列 ↔ Kotlin 集合）
    // -----------------------------------------------------------------------

    /** `["text","image"]`。 */
    fun stringListJson(values: List<String>): String =
        buildJsonArray { values.forEach { add(JsonPrimitive(it)) } }.toString()

    /** `{"low":"512"}`。 */
    fun stringMapJson(values: Map<String, String>): String =
        buildJsonObject { values.forEach { (k, v) -> put(k, v) } }.toString()

    /**
     * 额外查询参数 → `compat_json` 的文本（`{"query":{…}}`）。
     *
     * 空表回 `null`（库列写 NULL）。`compat_json` 里将来别的兼容开关不归这里管，
     * 所以读侧只认 `query` 这一段。
     */
    fun compatJson(query: Map<String, String>): String? =
        if (query.isEmpty()) null
        else buildJsonObject { put("query", query.toJsonMap()) }.toString()

    /** `compat_json` → 额外查询参数；坏 JSON 当空（一行坏数据不该让整页打不开）。 */
    fun queryOf(raw: String?): Map<String, String> {
        val root = rawObject(raw) ?: return emptyMap()
        val query = root["query"] as? JsonObject ?: return emptyMap()
        return query.mapNotNull { (k, v) ->
            (v as? JsonPrimitive)?.contentOrNull?.let { k to it }
        }.toMap()
    }

    /** `{"image":["inline"]}`。 */
    fun stringListMapJson(values: Map<String, List<String>>): String = buildJsonObject {
        values.forEach { (k, v) ->
            put(k, buildJsonArray { v.forEach { add(JsonPrimitive(it)) } })
        }
    }.toString()

    /** 读一个 JSON 文本列成字符串数组；**坏了当空**（列表列空着比整行读不出来好）。 */
    fun stringListOf(raw: String?): List<String> =
        parseArrayOrEmpty(raw).mapNotNull { (it as? JsonPrimitive)?.contentOrNull }

    fun stringMapOf(raw: String?): Map<String, String> =
        (rawObject(raw) ?: emptyMap()).mapNotNull { (k, v) ->
            (v as? JsonPrimitive)?.contentOrNull?.let { k to it }
        }.toMap()

    fun stringListMapOf(raw: String?): Map<String, List<String>> =
        (rawObject(raw) ?: emptyMap()).mapNotNull { (k, v) ->
            val list = (v as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            if (list == null) null else k to list
        }.toMap()

    /** 解析一整份 JSON 对象（桥那侧用：拿到的是 Dart 给的一段字符串）。读不懂回 `null`。 */
    fun parseObject(text: String): JsonObject? =
        runCatching { com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(text) as? JsonObject }
            .getOrNull()

    /** 解析一整份 JSON 数组（桥那侧用：拿到的是 Dart 给的一段字符串）。读不懂回 `null`。 */
    fun parseArray(raw: String?): List<kotlinx.serialization.json.JsonElement>? {
        if (raw.isNullOrBlank()) return null
        return runCatching {
            (com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(raw) as? JsonArray)?.toList()
        }.getOrNull()
    }

    private fun parseArrayOrEmpty(raw: String?): List<kotlinx.serialization.json.JsonElement> =
        parseArray(raw) ?: emptyList()

    private fun rawObject(raw: String?): Map<String, kotlinx.serialization.json.JsonElement>? {
        if (raw.isNullOrBlank()) return null
        return runCatching {
            (com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(raw) as? JsonObject)?.toMap()
        }.getOrNull()
    }

    private fun List<String>.toJsonArray(): JsonArray = buildJsonArray { forEach { add(JsonPrimitive(it)) } }

    private fun Map<String, String>.toJsonMap(): JsonObject =
        buildJsonObject { forEach { (k, v) -> put(k, v) } }

    private fun Map<String, List<String>>.toJsonListMap(): JsonObject = buildJsonObject {
        forEach { (k, v) -> put(k, buildJsonArray { v.forEach { add(JsonPrimitive(it)) } }) }
    }

    private fun JsonObject.str(key: String): String? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

    private fun JsonObject.bool(key: String): Boolean? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.toBooleanStrictOrNull()

    private fun JsonObject.int(key: String): Int? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.toIntOrNull()

    private fun JsonObject.stringList(key: String): List<String> =
        (this[key] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull } ?: emptyList()

    private fun JsonObject.stringMap(key: String): Map<String, String> =
        (this[key] as? JsonObject)?.mapNotNull { (k, v) ->
            (v as? JsonPrimitive)?.contentOrNull?.let { k to it }
        }?.toMap() ?: emptyMap()

    private fun JsonObject.stringListMap(key: String): Map<String, List<String>> =
        (this[key] as? JsonObject)?.mapNotNull { (k, v) ->
            val list = (v as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            if (list == null) null else k to list
        }?.toMap() ?: emptyMap()
}
