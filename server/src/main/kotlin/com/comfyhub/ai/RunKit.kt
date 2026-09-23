package com.comfyhub.ai

import com.comfyhub.AppJson
import com.comfyhub.SettingsRepo
import com.comfyhub.ai.tools.MemoryStore
import com.comfyhub.ai.tools.SkillStore
import com.comfyhub.ai.tools.ToolAccess
import com.comfyhub.ai.tools.ToolPolicy
import com.comfyhub.ai.tools.ToolRegistry
import com.comfyhub.ai.tools.ToolScope
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.add
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import java.nio.file.Path
import java.security.MessageDigest
import java.time.LocalDate

/**
 * 🔒 run-kit 工件（`docs/run-kit-contract.md`，M1 落地，2026-09-23）。
 *
 * **手机 harness 的所有"规则类"输入都从这一份 JSON 来**（红线 R13：两端都不许硬编码
 * 系统提示 / 工具描述 / 安全文案）。这里只做**组装**—— 每个字段的真源都在别处：
 *
 * | 字段 | 真源 |
 * | --- | --- |
 * | `systemPrompt` | [SystemPrompt.render]（整段渲染好的正文） |
 * | `promptVersion` | `SystemPrompt.VERSION` |
 * | `nsfwEnabled` | `NsfwConfig`（`app_settings` 的 `ai.nsfw`） |
 * | `tools[]` | [ToolRegistry.toolsFor]（🔒 ToolScope 过滤成手机可执行子集）+ 策略 deny 排除 |
 * | `untrustedBanner` | [ToolRegistry.UNTRUSTED_BANNER_TEMPLATE]（R7，两端不许各写一份） |
 * | `models[]` | `AiRepo`（Provider / 模型目录；**绝不含密钥**） |
 * | `inlineBudget` | [AttachmentPolicy.MAX_INLINE_BYTES] / [InlineBudget.MAX_TOTAL_BYTES] |
 *
 * ## kitVersion（契约 §3）
 *
 * 形如 `2026-09-23.1`。签名（[Signature]）里任何一项变了就 bump：
 * 提示词版本、NSFW 注入状态、工具清单结构（名称/描述/schema）、横幅文案、预算数值。
 * 签名与版本持久化在 `app_settings`（`ai.runkit.hash` / `ai.runkit.version`）。
 *
 * 刻意**不**把"渲染时用的 provider/model / Skills / 长期记忆"算进签名：那些是
 * **环境与数据**，不是规则 —— 换模型、记一条记忆不该让"规则版本"跳号
 * （数据走同步通道，环境随每次取 kit 现渲染）。
 *
 * ## 已知的 M1 边界（M3 手机 harness 落地时一起收口）
 *
 * `systemPrompt` 是**取 kit 那一刻**渲染出来的：Provider/Model 取桌面当前默认
 * （或查询参数指定的），附件说明按"无"渲染。手机发起 Run 时应当**现取一份 kit**
 * （局域网内一次 GET 很便宜）；`?providerId=&modelId=` 让手机按它选好的模型取环境头。
 */
object RunKit {

    /**
     * schema 版本真源（契约 §2：与 `/api/sync/manifest` 同一个数）。
     *
     * P2（MySQL → SQLite）落地后，这个数就是 `PRAGMA user_version`（迁移完成时设置）；
     * 在那之前 MySQL 没有 user_version，用常量占位 —— **P2 迁移时把 SQLite 的
     * user_version 设成同一个值**，别让手机端看到两个来源各说各话。
     */
    const val SCHEMA_VERSION = 1

    private const val KEY_VERSION = "ai.runkit.version"
    private const val KEY_HASH = "ai.runkit.hash"
    private const val KEY_REVOKED = "ai.runkit.revoked"

    /** 参与版本号的内容签名（契约 §3 的四类触发器 + NSFW 注入状态 + 起标题指令）。 */
    data class Signature(
        val promptVersion: String,
        val nsfwEnabled: Boolean,
        val nsfwPromptVersion: String,
        /** 工具清单结构：name + description + inputSchema（按名称排序，增删/改 schema 都会变） */
        val toolsJson: String,
        val untrustedBanner: String,
        val inlineBudgetJson: String,
        /**
         * 起标题那条按轮附加指令（`SystemPrompt.TITLE_INSTRUCTION`）。
         *
         * ⚠ 它**也属于"规则"**：手机 harness 要自己摘标题（`TitleStripper`），
         * 而 R13 明令"两端都不许硬编码系统提示 / 工具描述 / 安全文案" ——
         * 手机上的那句指令只能来自这里。改它必须 bump（不然手机会一直用旧措辞）。
         */
        val titleInstruction: String,
    ) {
        fun canonical(): String =
            listOf(
                "promptVersion=$promptVersion",
                "nsfw=$nsfwEnabled",
                "nsfwPromptVersion=$nsfwPromptVersion",
                "tools=$toolsJson",
                "banner=$untrustedBanner",
                "budget=$inlineBudgetJson",
                "titleInstruction=$titleInstruction",
            ).joinToString("\n")
    }

    /**
     * 版本号推导（纯函数，单测盯着）。
     *
     * `changed=false` 且 previous 非空时**原样返回 previous**；
     * 变了（或还没发过）才起号：同日序号 +1，跨日从 .1 重新起。
     */
    fun nextVersion(previous: String?, changed: Boolean, today: LocalDate = LocalDate.now()): String {
        if (!changed && previous != null) return previous
        val todayStr = today.toString()
        val prevDate = previous?.substringBeforeLast('.', "")
        val prevSeq = previous?.substringAfterLast('.', "")?.toIntOrNull()
        return if (prevDate == todayStr && prevSeq != null) "$todayStr.${prevSeq + 1}" else "$todayStr.1"
    }

    /** 签名的 SHA-256（十六进制小写）。只用于"变没变"的比较，不对外。 */
    fun hashOf(signature: Signature): String =
        MessageDigest.getInstance("SHA-256")
            .digest(signature.canonical().toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }

    /** 当前版本（`/run-kit/status` 用；还没发过 kit 时为 null）。 */
    fun currentVersion(): String? = runCatching { SettingsRepo.get(KEY_VERSION) }.getOrNull()

    /** 已吊销的版本清单（`/run-kit/status` 用；命中后手机必须联网重取再跑）。 */
    fun revoked(): List<String> = revokedList()

    /** 手动吊销一个 kit 版本（安全文本出问题时用）。 */
    fun revoke(version: String) {
        val current = revokedList()
        if (version in current) return
        SettingsRepo.put(KEY_REVOKED, (buildJsonArray {
            (current + version).forEach { add(JsonPrimitive(it)) }
        }).toString())
    }

    private fun revokedList(): List<String> = runCatching {
        SettingsRepo.get(KEY_REVOKED)?.let { raw ->
            (AppJson.parseToJsonElement(raw) as? JsonArray)
                ?.mapNotNull { el -> (el as? JsonPrimitive)?.contentOrNull }
        }
    }.getOrNull() ?: emptyList()

    /**
     * 对齐版本号：签名与库里存的一致就沿用现版本；不一致就 bump 并持久化。
     * 返回 `(kitVersion, changed)`。读接口每次调它 —— "手机来取"正是版本消费的时刻。
     */
    private fun ensureVersion(signature: Signature): Pair<String, Boolean> {
        val previous = currentVersion()
        val storedHash = runCatching { SettingsRepo.get(KEY_HASH) }.getOrNull()
        val newHash = hashOf(signature)
        val changed = storedHash != newHash
        val version = nextVersion(previous, changed)
        if (changed || previous == null) {
            runCatching {
                SettingsRepo.put(KEY_VERSION, version)
                SettingsRepo.put(KEY_HASH, newHash)
            }.onFailure {
                // 持久化失败只能降级为"这次请求内自洽"：下一位取 kit 的还会再试
                org.slf4j.LoggerFactory.getLogger("RunKit")
                    .warn("kitVersion 持久化失败（下次取 kit 会重新起号）: {}", it.message)
            }
        }
        return version to changed
    }

    /**
     * 当前 kit 版本（给 `/run-kit/status` 用）。
     *
     * 不渲染整份提示词 —— 版本签名只由**规则**构成（工具表 / 横幅 / 预算 / 提示词版本），
     * 与 provider/model 环境无关。还没发过任何 kit 时，这一刻也会把版本号起出来
     * （`status` 的 `current` 永远有值；起号是幂等的，重复调不会跳号）。
     */
    fun ensureCurrentVersion(toolRegistry: ToolRegistry, projectRoot: Path): String {
        val policy = ToolPolicy.load(projectRoot)
        val nsfw = NsfwConfig.load()
        return ensureVersion(signatureOf(toolRegistry, policy, nsfw)).first
    }

    /** 规则签名 + 随附的 JSON 形状（build 与版本推导共用，保证两处看到的是同一份）。 */
    private data class KitParts(
        val mobileTools: List<com.comfyhub.ai.tools.AgentTool>,
        val toolsArray: kotlinx.serialization.json.JsonArray,
        val toolsJson: String,
        val budget: JsonObject,
        val budgetJson: String,
        val banner: String,
        val titleInstruction: String,
        val signature: Signature,
    )

    private fun signatureOf(toolRegistry: ToolRegistry, policy: ToolPolicy, nsfw: NsfwConfig): Signature =
        kitParts(toolRegistry, policy, nsfw).signature

    private fun kitParts(toolRegistry: ToolRegistry, policy: ToolPolicy, nsfw: NsfwConfig): KitParts {
        // 🔒 工具清单：先按端过滤（MOBILE / BOTH），再按策略剔除 deny ——
        // 与桌面 specs() 同一条规则，只是范围换成了手机侧。
        val mobileTools = toolRegistry.toolsFor(ToolScope.MOBILE)
            .filter { policy.accessFor(it) != ToolAccess.DENY }
            .sortedBy { it.name }

        val toolsArray = buildJsonArray {
            mobileTools.forEach { tool ->
                add(buildJsonObject {
                    put("name", tool.name)
                    put("description", tool.description)
                    put("inputSchema", tool.parameters)
                    put("scope", tool.scope.wire)
                })
            }
        }
        val toolsJson = toolsArray.toString()

        val budget = buildJsonObject {
            put("perImageBytes", AttachmentPolicy.MAX_INLINE_BYTES)
            put("totalBytes", InlineBudget.MAX_TOTAL_BYTES)
        }
        val budgetJson = budget.toString()
        val banner = ToolRegistry.UNTRUSTED_BANNER_TEMPLATE + ToolRegistry.UNTRUSTED_BANNER_QUERY_LINE
        // 起标题指令：手机 harness 也要自动起会话名，而它**不许自己写一份**（R13）
        val titleInstruction = SystemPrompt.TITLE_INSTRUCTION

        val signature = Signature(
            promptVersion = SystemPrompt.VERSION,
            nsfwEnabled = nsfw.enabled,
            nsfwPromptVersion = NsfwPrompt.VERSION,
            toolsJson = toolsJson,
            untrustedBanner = banner,
            inlineBudgetJson = budgetJson,
            titleInstruction = titleInstruction,
        )
        return KitParts(
            mobileTools, toolsArray, toolsJson, budget, budgetJson, banner, titleInstruction, signature,
        )
    }

    /**
     * 组装整份 run-kit（`GET /api/ai/run-kit` 的响应体）。
     *
     * [providerOverride] / [modelOverride]：手机按它选好的模型取对应的环境头；
     * 缺省用桌面当前默认（第一个启用的 Provider + 它下面第一个启用的模型）。
     * 覆盖值必须是**库里存在的** Provider / 它名下的模型，否则 400（不静默回落 ——
     * 那会让手机拿着一份"prompt 里写的模型和它要用的不一样"的 kit 还不自知）。
     */
    fun build(
        toolRegistry: ToolRegistry,
        skills: SkillStore,
        memory: MemoryStore?,
        projectRoot: Path,
        providerOverride: String? = null,
        modelOverride: String? = null,
    ): JsonObject {
        val policy = ToolPolicy.load(projectRoot)
        val nsfw = NsfwConfig.load()
        val parts = kitParts(toolRegistry, policy, nsfw)
        val (kitVersion, _) = ensureVersion(parts.signature)

        val (provider, modelId) = resolveProviderModel(providerOverride, modelOverride)

        val systemPrompt = SystemPrompt.render(
            provider,
            modelId,
            parts.mobileTools.map { com.comfyhub.ai.protocol.ToolSpec(it.name, it.description, it.parameters) },
            skills.catalog(),
            policy,
            toolRegistry,
            memory?.promptText().orEmpty(),
            // kit 是"取那一刻"的工件；单次 Run 带没带图由发起 Run 的那一端在
            // 用户轮里如实说明 —— 这里没有"当前请求"，只能按无附件渲染（M1 边界）。
            hasAttachments = false,
            nsfw = nsfw.enabled,
        )

        return buildJsonObject {
            put("kitVersion", kitVersion)
            put("schemaVersion", SCHEMA_VERSION)
            put("systemPrompt", systemPrompt)
            put("promptVersion", SystemPrompt.VERSION)
            put("nsfwEnabled", nsfw.enabled)
            put("tools", parts.toolsArray)
            put("untrustedBanner", parts.banner)
            // 起标题指令（M3）：手机 harness 按轮附加它、再用同一套 TitleStripper 摘掉标记
            put("titleInstruction", parts.titleInstruction)
            put("models", buildJsonArray { modelsJson(providerOverride).forEach { add(it) } })
            put("inlineBudget", parts.budget)
        }
    }

    /** Provider / 模型解析：显式覆盖 → 校验存在性；缺省 → 第一个启用的 Provider。 */
    private fun resolveProviderModel(providerOverride: String?, modelOverride: String?): Pair<AiProviderDto, String> {
        val provider = providerOverride?.let { id ->
            AiRepo.getProvider(id)?.takeIf { it.enabled }
                ?: throw AiException(
                    AiErrorCode.CONFIG_ERROR,
                    "run-kit：Provider 不存在或未启用: $id",
                )
        } ?: AiRepo.listProviders().firstOrNull { it.enabled }

        val modelId = when {
            modelOverride != null -> {
                val p = provider
                    ?: throw AiException(AiErrorCode.CONFIG_ERROR, "run-kit：指定了 modelId 但没有任何可用 Provider")
                AiRepo.getModel(p.id, modelOverride)?.takeIf { it.enabled }
                    ?: throw AiException(
                        AiErrorCode.CONFIG_ERROR,
                        "run-kit：模型不存在或未启用: ${p.id}/$modelOverride",
                    )
                modelOverride
            }
            provider != null -> AiRepo.listModels(provider.id).firstOrNull { it.enabled }?.id.orEmpty()
            else -> ""
        }
        return (provider ?: unconfiguredProvider()) to modelId
    }

    /** 一台还没配过 Provider 的机器也要能吐出形状完整的 kit（内容如实写"未配置"）。 */
    private fun unconfiguredProvider() = AiProviderDto(
        id = "unconfigured",
        displayName = "未配置 Provider",
        api = "none",
        baseURL = "",
        endpointTrust = "public",
        enabled = false,
    )

    /**
     * 模型目录（`models[]`）：手机能选哪些模型。**绝不含密钥** ——
     * 连 `credentialRef` 的名字都不带（那是桌面侧凭据体系的引用，手机拿了没用还多漏一样）。
     */
    private fun modelsJson(providerOverride: String?): List<JsonObject> {
        val providers = providerOverride?.let { id ->
            listOfNotNull(AiRepo.getProvider(id)?.takeIf { it.enabled })
        } ?: AiRepo.listProviders().filter { it.enabled }
        return providers.flatMap { p ->
            AiRepo.listModels(p.id).filter { it.enabled }.map { m ->
                buildJsonObject {
                    put("providerId", p.id)
                    put("providerName", p.displayName)
                    put("providerApi", p.api)
                    put("modelId", m.id)
                    put("displayName", m.displayName)
                    put("inputModalities", buildJsonArray { m.inputModalities.forEach { add(JsonPrimitive(it)) } })
                    put(
                        "attachmentTransports",
                        JsonObject(m.attachmentTransports.mapValues { (_, v) ->
                            buildJsonArray { v.forEach { add(JsonPrimitive(it)) } }
                        }),
                    )
                    put("tools", m.tools)
                    put("parallelTools", m.parallelTools)
                    put("reasoning", m.reasoning)
                    put("thinkingEfforts", JsonObject(m.thinkingEfforts.mapValues { (_, v) -> JsonPrimitive(v) }))
                    m.thinkingFormat?.let { put("thinkingFormat", it) }
                    m.contextWindow?.let { put("contextWindow", it) }
                    m.maxOutputTokens?.let { put("maxOutputTokens", it) }
                    m.maxAttachmentBytes?.let { put("maxAttachmentBytes", it) }
                    m.maxAttachmentCount?.let { put("maxAttachmentCount", it) }
                    put("capabilitySource", m.capabilitySource)
                }
            }
        }
    }
}
