package com.comfyhub.harness.kit

import com.comfyhub.harness.attachment.InlineBudget
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * **run-kit**：桌面下发的那份"规则工件"在手机侧的形状（🔒 `docs/run-kit-contract.md`）。
 *
 * 为什么手机要原样接住它、而不是自己攒一套：
 * **手机 harness 不许硬编码系统提示、工具描述、安全文案**（红线 **R13**）。
 * 规则只有一份 —— 桌面产出、版本化下发、手机缓存；两端的漂移于是从"代码层"降到"数据层"，
 * 而数据层的漂移是可见、可比对、可一次性修正的（版本号 + diff）。
 *
 * 因此本文件里**一个提示串字面量都没有**（连一句"你是助手"都没有）：
 * 有的只是"把下发的 JSON 读成结构"和"读不回来就明确失败"。
 */
data class RunKit(
    /** 整份工件的版本，形如 `2026-09-23.1`；**随每次 Run 记录**（事后可追溯用了哪版规则）。 */
    val kitVersion: String,
    /** 桌面的 `PRAGMA user_version`（与 `/api/sync/manifest` 同一个数）。 */
    val schemaVersion: Int,
    /** 渲染好的系统提示正文（整段，不是片段）。 */
    val systemPrompt: String,
    /** 规则正文版本（`SystemPrompt.VERSION`）。 */
    val promptVersion: String,
    val nsfwEnabled: Boolean,
    /** 手机可执行的工具子集（桌面已按 ToolScope 过滤）。 */
    val tools: List<KitTool>,
    /** 工具结果 / 网页内容的"不可信数据"横幅（R7，两端不许各写一份）。 */
    val untrustedBanner: String,
    /**
     * 起标题的按轮附加指令（**原样下发**）。
     *
     * 为 `null` 时**不要自己补一句**（那就成了硬编码）：直接不起标题，
     * 会话名留在本地默认值 —— 少一个功能，好过两端规则不一致。
     */
    val titleInstruction: String?,
    /**
     * "输出被长度上限截断时接着写"的指令（**原样下发**，同样不许手机自己写一份）。
     *
     * 为 `null` 时**不做续写**（撞到上限就如实结束），而不是自己编一句。
     */
    val continueInstruction: String?,
    val models: List<KitModel>,
    val inlineBudget: InlineBudget.Limits,
) {
    fun tool(name: String): KitTool? = tools.firstOrNull { it.name == name }

    /** 工具名清单（`ToolRegistry` 的唯一真源在桌面，这里只是它的投影）。 */
    fun toolNames(): Set<String> = tools.map { it.name }.toSet()
}

/** `tools[]` 的一项：形状与桌面 `ToolRegistry.specs()` 一致。 */
data class KitTool(
    val name: String,
    val description: String,
    val inputSchema: JsonObject,
    /** `MOBILE` / `BOTH`（桌面专属的那些不该出现在这里，见 ToolScope 契约 §3）。 */
    val scope: String,
)

/** `models[]` 的一项：**不含密钥**（连 credentialRef 的名字都不带）。 */
data class KitModel(
    val providerId: String,
    val providerName: String,
    val providerApi: String,
    val modelId: String,
    val displayName: String,
    val inputModalities: List<String>,
    val attachmentTransports: Map<String, Set<String>>,
    val tools: Boolean,
    val reasoning: Boolean,
    val thinkingEfforts: Map<String, String>,
    val thinkingFormat: String?,
    val contextWindow: Int?,
    val maxOutputTokens: Int?,
    val capabilitySource: String,
)

/**
 * [RunKit] 的 JSON 读写。
 *
 * 两条纪律：
 *  1. **必需字段缺失就抛**（[KitParseFailure]）—— 一份"缺系统提示"的 kit 不能让 Run 跑起来，
 *     那会变成"模型没有任何规则"的静默降级；
 *  2. 可选字段缺失**给明确的默认**，并且**不编造规则文案**（见 [RunKit.titleInstruction]）。
 */
object RunKitCodec {

    class KitParseFailure(message: String) : RuntimeException(message)

    fun parse(json: String): RunKit {
        val root = runCatching { com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(json) }
            .getOrElse { throw KitParseFailure("run-kit 不是合法 JSON：${it.message}") }
        return parse(root as? JsonObject ?: throw KitParseFailure("run-kit 的顶层必须是对象"))
    }

    fun parse(root: JsonObject): RunKit {
        fun required(key: String): String =
            (root[key] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
                ?: throw KitParseFailure("run-kit 缺少必需字段 $key")

        val kitVersion = required("kitVersion")
        val systemPrompt = required("systemPrompt")
        val promptVersion = required("promptVersion")

        val tools = root.array("tools").mapNotNull { el ->
            val obj = el as? JsonObject ?: return@mapNotNull null
            val name = (obj["name"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
            KitTool(
                name = name,
                description = (obj["description"] as? JsonPrimitive)?.contentOrNull.orEmpty(),
                inputSchema = obj["inputSchema"] as? JsonObject ?: JsonObject(emptyMap()),
                scope = (obj["scope"] as? JsonPrimitive)?.contentOrNull.orEmpty(),
            )
        }

        val models = root.array("models").mapNotNull { el ->
            val obj = el as? JsonObject ?: return@mapNotNull null
            val providerId = obj.str("providerId") ?: return@mapNotNull null
            val modelId = obj.str("modelId") ?: return@mapNotNull null
            KitModel(
                providerId = providerId,
                providerName = obj.str("providerName").orEmpty(),
                providerApi = obj.str("providerApi").orEmpty(),
                modelId = modelId,
                displayName = obj.str("displayName") ?: modelId,
                inputModalities = obj.stringList("inputModalities"),
                attachmentTransports = (obj["attachmentTransports"] as? JsonObject)
                    ?.mapValues { (_, v) -> (v as? JsonArray)?.mapNotNull { it.strValue() }?.toSet().orEmpty() }
                    .orEmpty(),
                tools = obj.bool("tools") ?: false,
                reasoning = obj.bool("reasoning") ?: false,
                thinkingEfforts = (obj["thinkingEfforts"] as? JsonObject)
                    ?.mapNotNull { (k, v) -> (v as? JsonPrimitive)?.contentOrNull?.let { k to it } }
                    ?.toMap()
                    .orEmpty(),
                thinkingFormat = obj.str("thinkingFormat"),
                contextWindow = obj.int("contextWindow"),
                maxOutputTokens = obj.int("maxOutputTokens"),
                capabilitySource = obj.str("capabilitySource").orEmpty(),
            )
        }

        val budget = root["inlineBudget"] as? JsonObject
        val limits = InlineBudget.Limits(
            perImageBytes = budget?.get("perImageBytes")?.let { (it as? JsonPrimitive)?.contentOrNull?.toLongOrNull() }
                ?: InlineBudget.MAX_INLINE_BYTES,
            totalBytes = budget?.get("totalBytes")?.let { (it as? JsonPrimitive)?.contentOrNull?.toLongOrNull() }
                ?: InlineBudget.MAX_TOTAL_BYTES,
        )

        return RunKit(
            kitVersion = kitVersion,
            schemaVersion = (root["schemaVersion"] as? JsonPrimitive)?.contentOrNull?.toIntOrNull() ?: 0,
            systemPrompt = systemPrompt,
            promptVersion = promptVersion,
            nsfwEnabled = root.bool("nsfwEnabled") ?: false,
            tools = tools,
            untrustedBanner = root.str("untrustedBanner").orEmpty(),
            titleInstruction = root.str("titleInstruction")?.takeIf { it.isNotBlank() },
            continueInstruction = root.str("continueInstruction")?.takeIf { it.isNotBlank() },
            models = models,
            inlineBudget = limits,
        )
    }

    /**
     * 序列化回 JSON —— **只用来做本地缓存**（手机不产出 kit，只缓存桌面给的）。
     *
     * 缓存的是**原文**还是重编码的？两者都行，但重编码更稳：
     * 桌面加了新字段、手机这版解析器不认识时，缓存里不会有半截形状。
     */
    fun toJson(kit: RunKit): String = buildJsonObject {
        put("kitVersion", kit.kitVersion)
        put("schemaVersion", kit.schemaVersion)
        put("systemPrompt", kit.systemPrompt)
        put("promptVersion", kit.promptVersion)
        put("nsfwEnabled", kit.nsfwEnabled)
        put("tools", buildJsonArray {
            kit.tools.forEach { t ->
                add(buildJsonObject {
                    put("name", t.name)
                    put("description", t.description)
                    put("inputSchema", t.inputSchema)
                    put("scope", t.scope)
                })
            }
        })
        put("untrustedBanner", kit.untrustedBanner)
        kit.titleInstruction?.let { put("titleInstruction", it) }
        kit.continueInstruction?.let { put("continueInstruction", it) }
        put("models", buildJsonArray {
            kit.models.forEach { m ->
                add(buildJsonObject {
                    put("providerId", m.providerId)
                    put("providerName", m.providerName)
                    put("providerApi", m.providerApi)
                    put("modelId", m.modelId)
                    put("displayName", m.displayName)
                    put("inputModalities", buildJsonArray { m.inputModalities.forEach { add(JsonPrimitive(it)) } })
                    put("attachmentTransports", buildJsonObject {
                        m.attachmentTransports.forEach { (kind, transports) ->
                            put(kind, buildJsonArray { transports.forEach { add(JsonPrimitive(it)) } })
                        }
                    })
                    put("tools", m.tools)
                    put("reasoning", m.reasoning)
                    put("thinkingEfforts", buildJsonObject {
                        m.thinkingEfforts.forEach { (k, v) -> put(k, JsonPrimitive(v)) }
                    })
                    m.thinkingFormat?.let { put("thinkingFormat", it) }
                    m.contextWindow?.let { put("contextWindow", it) }
                    m.maxOutputTokens?.let { put("maxOutputTokens", it) }
                    put("capabilitySource", m.capabilitySource)
                })
            }
        })
        put("inlineBudget", buildJsonObject {
            put("perImageBytes", kit.inlineBudget.perImageBytes)
            put("totalBytes", kit.inlineBudget.totalBytes)
        })
    }.toString()

    // --- 小工具 -------------------------------------------------------------

    private fun JsonObject.array(key: String): List<JsonElement> =
        (this[key] as? JsonArray)?.toList() ?: emptyList()

    private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.contentOrNull

    private fun JsonObject.bool(key: String): Boolean? =
        (this[key] as? JsonPrimitive)?.contentOrNull?.toBooleanStrictOrNull()

    private fun JsonObject.int(key: String): Int? = (this[key] as? JsonPrimitive)?.contentOrNull?.toIntOrNull()

    private fun JsonObject.stringList(key: String): List<String> =
        (this[key] as? JsonArray)?.mapNotNull { it.strValue() } ?: emptyList()

    private fun JsonElement.strValue(): String? = (this as? JsonPrimitive)?.contentOrNull
}
