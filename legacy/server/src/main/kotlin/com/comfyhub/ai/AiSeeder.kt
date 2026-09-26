package com.comfyhub.ai

import com.comfyhub.SettingsRepo
import org.slf4j.LoggerFactory

/**
 * 一次内置目录落库的结果。**不抛异常**是硬要求：启动路径上的任何失败都只反映在这里 + 日志里。
 *
 * @param created 本次是否真的新建了 provider（含模型）
 * @param skipped provider 行本来就在（模型可能有补齐，详情看 [SeedSyncOutcome]）
 * @param modelCount 操作完成后该 provider 名下的模型数
 */
data class SeedOutcome(
    val version: String,
    val providerId: String? = null,
    val created: Boolean = false,
    val skipped: Boolean = false,
    val modelCount: Int = 0,
    val error: String? = null,
) {
    val ok: Boolean get() = error == null

    override fun toString(): String = buildString {
        append("SeedOutcome(")
        append("version=$version, providerId=$providerId, ")
        append(if (created) "created=true" else if (skipped) "skipped=true" else "created=false")
        append(", modelCount=$modelCount")
        if (error != null) append(", error=$error")
        append(")")
    }
}

/**
 * 登记内置目录的两种模式。
 *
 * 语义差别**必须严格**：用户手工调过的模型能力，绝不能被一次启动悄悄冲掉。
 */
enum class SeedMode {
    /**
     * 默认（启动时会走的那条）：**只补库里没有的 model_id**。
     *
     * provider 不在 → 整份插入；provider 在 → 一条一条补缺的模型，
     * 已存在的行一个字节都不改（displayName / baseURL / credentialRef / revision 也全不碰），
     * 更不删任何模型（包括用户自己加的）。
     */
    ADD_MISSING,

    /**
     * 显式动作（用户点按钮才走）：把**内置目录里已有的那些 model_id** 的能力声明对齐到目录。
     *
     * 不新增、不删除、不碰 provider 行、不碰用户自己加进目录的模型。
     */
    REFRESH_CAPABILITIES,
}

/**
 * [AiSeeder.syncBuiltinProvider] 的结果。
 *
 * `added` / `updated` / `kept` 是**条数**（方便直接 `if (seed.added > 0)`），
 * `divergent` 是 model_id 列表（界面/日志要能说清"哪几个"）。
 */
data class SeedSyncOutcome(
    val mode: SeedMode,
    val version: String,
    val providerId: String? = null,
    /** 本次新增的模型数 */
    val added: Int = 0,
    /** 本次真正写过能力字段的模型数（只有 REFRESH_CAPABILITIES 会 > 0） */
    val updated: Int = 0,
    /** 已存在且与内置目录一致、未改动的模型数 */
    val kept: Int = 0,
    /** 已存在但能力声明与内置目录**不同**的 model_id（ADD_MISSING 下保持用户当前设置，只提示） */
    val divergent: List<String> = emptyList(),
    /** 操作完成后该 provider 名下的模型总数 */
    val modelCount: Int = 0,
    /** 本次是否新建了 provider 行 */
    val providerCreated: Boolean = false,
    val error: String? = null,
) {
    val ok: Boolean get() = error == null

    override fun toString(): String =
        "SeedSyncOutcome(mode=$mode, version=$version, providerId=$providerId, added=$added, " +
            "updated=$updated, kept=$kept, divergent=${divergent.size}, modelCount=$modelCount, " +
            (if (error != null) "error=$error)" else "ok)")
}

/** [AiSeeder.planSeed] 的产物：目录与库的对照结论（纯数据，可单测）。 */
internal data class SeedPlan(
    /** 库里没有、需要 INSERT 的目录条目 */
    val added: List<BuiltinModel>,
    /** 库里已有且与目录一致、保持不动的 model_id */
    val kept: List<String>,
    /** 库里已有但能力声明与目录不同的 model_id（保持用户当前设置，只提示） */
    val divergent: List<String>,
)

/** [AiSeeder.decideActions] 的产物：这次到底要 INSERT 什么、要 UPDATE 什么。 */
internal data class SeedActions(
    val toInsert: List<String>,
    val toRefresh: List<String>,
)

/**
 * 把内置目录（[AiSeedCatalog] 里的冻结副本）**幂等**登记进数据库。
 *
 * 用户的要求原话是："把 .dsh/settings.yaml 里的模型挨个抄过来，可以放在数据库里，
 * 不要让程序直接读 settings.yaml"。所以职责分工是：
 *
 *  - [AiSeedCatalog]：读打包进 jar 的冻结 JSON（不碰磁盘上的 YAML）；
 *  - [AiSeeder]：把那份 JSON 写进 `ai_providers` / `ai_models`（复用 [AiRepo] 的写入方法）；
 *  - 之后模型能力的唯一真源是数据库 —— 用户在设置页改了、删了，我们**永不覆盖**。
 *
 * 为什么"已存在就什么都不做"不够（真实踩到的）：本机库里早就有一份**手工/接口导入**的
 * `command-code-goat`（69 个模型，但能力声明是导入时的乐观值：模态全开、reasoning=false），
 * 于是老版 `ensureBuiltinProvider()` 直接 skip，用户看到的仍然是"没登记"。所以现在是
 * [SeedMode.ADD_MISSING] 的语义：**缺哪个补哪个、已有的不动**。
 *
 * 已知取舍（要不要做由产品定，这里不自作主张）：目录升级时不会去改用户改过的模型 ——
 * 那需要一个显式动作（[SeedMode.REFRESH_CAPABILITIES]），而不是启动时的副作用。
 */
object AiSeeder {

    private val log = LoggerFactory.getLogger(AiSeeder::class.java)

    /** `app_settings` 里记"已登记的内置目录版本"的键。 */
    const val VERSION_KEY = "ai.builtin.seed.version"

    /** 未知版本占位（连资源都读不出来时用，避免二次抛异常）。 */
    private const val UNKNOWN_VERSION = "unknown"

    // -----------------------------------------------------------------------
    //  对外入口
    // -----------------------------------------------------------------------

    /**
     * 幂等登记内置目录；**可每次启动调用**，绝不对用户改过的东西下手。
     *
     * @param mode 默认 [SeedMode.ADD_MISSING]（补齐缺失的模型，已存在的不动）
     */
    fun syncBuiltinProvider(mode: SeedMode = SeedMode.ADD_MISSING): SeedSyncOutcome {
        var version = UNKNOWN_VERSION
        return try {
            version = AiSeedCatalog.CATALOG_VERSION
            val provider = AiSeedCatalog.providers().firstOrNull()
                ?: return SeedSyncOutcome(mode = mode, version = version, error = "内置模型目录里没有任何 provider")
            syncProvider(provider, mode)
        } catch (t: Throwable) {
            log.error("内置模型目录登记失败（已忽略，不影响启动）：{}", t.message, t)
            SeedSyncOutcome(mode = mode, version = version, error = describe(t))
        }
    }

    /**
     * 只读预览：不写库，只回答"现在同步会发生什么"。
     *
     * 界面按钮在真正对齐前用它弹确认框（"将新增 N 个模型 / 有 M 个模型的能力与内置目录不同"）。
     *
     * **绝对不写库**：只走 `AiRepo.getProvider` + `AiRepo.listModels` 两条 SELECT，
     * 既不 INSERT / UPDATE，也不写 `ai.builtin.seed.version`。
     *
     * 口径与 [syncBuiltinProvider]`(ADD_MISSING)` 完全一致：`added` = 现在缺的模型数、
     * `kept` = 已存在且一致、`divergent` = 已存在但能力不同（预览里它们**不会**被改）、
     * `updated` 恒为 0、`modelCount` = 跑完之后该 provider 名下的模型总数；
     * provider 不存在时 `added` = 目录里全部、`divergent` 空、`providerCreated = true`。
     */
    fun previewBuiltinSync(): SeedSyncOutcome {
        var version = UNKNOWN_VERSION
        return try {
            version = AiSeedCatalog.CATALOG_VERSION
            val provider = AiSeedCatalog.providers().firstOrNull()
                ?: return SeedSyncOutcome(
                    mode = SeedMode.ADD_MISSING,
                    version = version,
                    error = "内置模型目录里没有任何 provider",
                )
            val id = AiValidation.requireProviderId(provider.id)
            // 两条都是 SELECT；读到什么就交给纯函数算，写完的事一件不做。
            val existing = if (AiRepo.getProvider(id) == null) null else AiRepo.listModels(id).associateBy { it.id }
            previewPlan(version = version, providerId = id, catalog = provider.models, existing = existing)
        } catch (t: Throwable) {
            log.error("内置模型目录预览失败：{}", t.message, t)
            SeedSyncOutcome(mode = SeedMode.ADD_MISSING, version = version, error = describe(t))
        }
    }

    /**
     * 兼容入口：等价于 `syncBuiltinProvider(ADD_MISSING)`，只是返回老的 [SeedOutcome]。
     *
     * `skipped=true` 表示 provider 行本来就在（模型可能有补齐，详情看 syncBuiltinProvider 的结果）。
     */
    fun ensureBuiltinProvider(): SeedOutcome {
        val r = syncBuiltinProvider(SeedMode.ADD_MISSING)
        return SeedOutcome(
            version = r.version,
            providerId = r.providerId,
            created = r.providerCreated,
            skipped = !r.providerCreated && r.ok,
            modelCount = r.modelCount,
            error = r.error,
        )
    }

    /**
     * 把目录里**所有** provider 各同步一次（现在只有 1 个）。
     *
     * 同样不抛异常；单个 provider 失败只影响它自己那条结果。
     */
    fun syncAll(mode: SeedMode = SeedMode.ADD_MISSING): List<SeedSyncOutcome> = try {
        AiSeedCatalog.providers().map { provider ->
            try {
                syncProvider(provider, mode)
            } catch (t: Throwable) {
                log.error("内置 Provider {} 登记失败（已忽略）：{}", provider.id, t.message, t)
                SeedSyncOutcome(mode = mode, version = safeVersion(), providerId = provider.id, error = describe(t))
            }
        }
    } catch (t: Throwable) {
        log.error("内置模型目录登记失败（已忽略，不影响启动）：{}", t.message, t)
        listOf(SeedSyncOutcome(mode = mode, version = safeVersion(), error = describe(t)))
    }

    // -----------------------------------------------------------------------
    //  纯逻辑（不碰数据库，单测直接覆盖）
    // -----------------------------------------------------------------------

    /**
     * 目录 × 库 的对照：算出 `added`（缺的）/ `kept`（一致、不动）/ `divergent`（不一致、不动）。
     *
     * 库里**多出来**的 model_id（用户自己加的）不属于这三类里的任何一类 —— 直接忽略，
     * 也就是永远不删、不改（这是刻意的）。
     */
    internal fun planSeed(catalog: List<BuiltinModel>, existing: Map<String, AiModelDto>): SeedPlan {
        val added = mutableListOf<BuiltinModel>()
        val kept = mutableListOf<String>()
        val divergent = mutableListOf<String>()
        for (model in catalog) {
            val row = existing[model.id]
            when {
                row == null -> added += model
                capabilityDiffers(model, row) -> divergent += model.id
                else -> kept += model.id
            }
        }
        return SeedPlan(added = added, kept = kept, divergent = divergent)
    }

    /**
     * 这条库里的行，能力声明与目录是否不同。
     *
     * 只比"能力"那几样（模态 / 工具 / 并行工具 / 推理 / 思考档位 / 思考方言 / 上下文窗口 / 来源），
     * **不比** displayName、enabled、附件上限 —— 那些是用户的地盘（对齐也不该动它们）。
     */
    internal fun capabilityDiffers(catalog: BuiltinModel, row: AiModelDto): Boolean {
        val want = catalog.toModelDto(row.providerId)
        return row.inputModalities.sorted() != want.inputModalities.sorted() ||
            row.tools != want.tools ||
            row.parallelTools != want.parallelTools ||
            row.reasoning != want.reasoning ||
            row.thinkingEfforts != want.thinkingEfforts ||
            row.thinkingFormat != want.thinkingFormat ||
            row.contextWindow != want.contextWindow ||
            row.capabilitySource != want.capabilitySource
    }

    /** 模式 → 动作：ADD_MISSING 只插入、REFRESH_CAPABILITIES 只刷新（刷新目标就是 divergent）。 */
    internal fun decideActions(mode: SeedMode, plan: SeedPlan): SeedActions = when (mode) {
        SeedMode.ADD_MISSING -> SeedActions(toInsert = plan.added.map { it.id }, toRefresh = emptyList())
        SeedMode.REFRESH_CAPABILITIES -> SeedActions(toInsert = emptyList(), toRefresh = plan.divergent)
    }

    /**
     * [previewBuiltinSync] 的纯逻辑：给定**已经读到的**库状态，算出"跑 ADD_MISSING 会怎样"。
     *
     * 没有任何写库动作，也不碰 AiRepo —— 单测直接喂假数据即可覆盖每条分支。
     *
     * @param existing null 表示 provider 行不存在；否则是它名下的模型（键为 model_id）
     */
    internal fun previewPlan(
        version: String,
        providerId: String,
        catalog: List<BuiltinModel>,
        existing: Map<String, AiModelDto>?,
    ): SeedSyncOutcome {
        if (existing == null) {
            // provider 不在 → ADD_MISSING 会把整份目录插进去
            return SeedSyncOutcome(
                mode = SeedMode.ADD_MISSING,
                version = version,
                providerId = providerId,
                added = catalog.size,
                kept = 0,
                divergent = emptyList(),
                modelCount = catalog.size,
                providerCreated = true,
            )
        }
        val plan = planSeed(catalog, existing)
        return SeedSyncOutcome(
            mode = SeedMode.ADD_MISSING,
            version = version,
            providerId = providerId,
            added = plan.added.size,
            updated = 0, // 预览不刷新：divergent 的那些在 ADD_MISSING 下保持用户设置
            kept = plan.kept.size,
            divergent = plan.divergent,
            // 跑完之后的总数 = 现在库里的（含用户自己加的）+ 这次要补的
            modelCount = existing.size + plan.added.size,
            providerCreated = false,
        )
    }

    // -----------------------------------------------------------------------
    //  内部（碰数据库）
    // -----------------------------------------------------------------------

    private fun syncProvider(p: BuiltinProvider, mode: SeedMode): SeedSyncOutcome {
        val id = AiValidation.requireProviderId(p.id)
        val version = AiSeedCatalog.CATALOG_VERSION

        if (AiRepo.getProvider(id) == null) {
            if (mode == SeedMode.REFRESH_CAPABILITIES) {
                // REFRESH 的语义是"只对齐、不新增"：没有 provider 就没有可对齐的行。
                // 不动库、也不写版本号（写一个没生效的版本只会误导人）。
                log.info("内置 Provider {} 尚未登记，REFRESH_CAPABILITIES 不做任何改动（先走 ADD_MISSING 登记）", id)
                return SeedSyncOutcome(mode = mode, version = version, providerId = id)
            }
            return insertAll(p, id, version, mode)
        }
        return reconcile(p, id, version, mode)
    }

    /** provider 不在：整份插入（原来的 ensureBuiltinProvider 路径）。 */
    private fun insertAll(p: BuiltinProvider, id: String, version: String, mode: SeedMode): SeedSyncOutcome {
        val dto = p.toProviderDto()
        val models = p.models.map { it.toModelDto(id) }
        models.forEach { validate(it) }

        AiRepo.insertProvider(dto)
        val stored = AiRepo.replaceModels(id, models)
        SettingsRepo.put(VERSION_KEY, version)

        log.info("内置 Provider {} 已登记：{} 个模型（目录版本 {}）", id, stored.size, version)
        return SeedSyncOutcome(
            mode = mode,
            version = version,
            providerId = id,
            added = stored.size,
            kept = 0,
            modelCount = stored.size,
            providerCreated = true,
        )
    }

    /** provider 已在：按模式补缺 / 对齐能力，**绝不**碰 provider 行与用户自有模型。 */
    private fun reconcile(p: BuiltinProvider, id: String, version: String, mode: SeedMode): SeedSyncOutcome {
        val existing = AiRepo.listModels(id).associateBy { it.id }
        val plan = planSeed(p.models, existing)
        val actions = decideActions(mode, plan)

        plan.divergent.forEach { modelId ->
            log.info(
                "内置目录里的 {} 与库里已有的同 id 模型能力声明不同（保持用户当前设置；" +
                    "需要对齐请走 REFRESH_CAPABILITIES）",
                modelId,
            )
        }

        val byId = p.models.associateBy { it.id }
        val addedIds = actions.toInsert.map { modelId ->
            val dto = byId.getValue(modelId).toModelDto(id)
            validate(dto)
            AiRepo.insertModel(dto).id
        }
        val updatedIds = actions.toRefresh.map { modelId ->
            val dto = byId.getValue(modelId).toModelDto(id)
            validate(dto)
            AiRepo.updateModelCapabilities(dto).id
        }

        SettingsRepo.put(VERSION_KEY, version)
        val total = AiRepo.listModels(id).size
        log.info(
            "内置 Provider {} 已同步（mode={}）：新增 {}，刷新 {}，保持 {}，与目录不一致 {}，现有模型 {}",
            id, mode, addedIds.size, updatedIds.size, plan.kept.size, plan.divergent.size, total,
        )
        return SeedSyncOutcome(
            mode = mode,
            version = version,
            providerId = id,
            added = addedIds.size,
            updated = updatedIds.size,
            kept = plan.kept.size,
            divergent = plan.divergent,
            modelCount = total,
            providerCreated = false,
        )
    }

    /**
     * 与 HTTP 路径同一套校验规则。
     *
     * `AiRoutes.validateModel` 是**文件级 private**，外部拿不到，所以这里按它的规则逐条复核
     * （校验的公共部分都在 [AiValidation] 里，实际是同一份真源；只有"输入模态/传输方式/能力来源"
     * 三条是照抄的）。若哪天把它提升为 internal，应改成直接调用，避免两处漂移。
     */
    private fun validate(m: AiModelDto) {
        AiValidation.requireModelId(m.id)
        AiValidation.requireDisplayName(m.displayName)
        val badModalities = m.inputModalities.filter { Modality.parse(it) == null }
        if (badModalities.isNotEmpty()) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "未知的输入模态: ${badModalities.joinToString()}")
        }
        m.attachmentTransports.forEach { (key, values) ->
            if (Modality.parse(key) == null) {
                throw AiException(AiErrorCode.CONFIG_ERROR, "未知的传输方式键: $key")
            }
            values.forEach { t ->
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

    private fun BuiltinProvider.toProviderDto(): AiProviderDto {
        val api = AiApi.parse(this.api)
            ?: throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "内置目录里的协议 ${this.api} 不受支持（可选 ${AiApi.wireValues.joinToString(" / ")}）",
            )
        val trust = EndpointTrust.parse(endpointTrust) ?: EndpointTrust.infer(hostOf(baseURL))
        return AiProviderDto(
            id = AiValidation.requireProviderId(id),
            displayName = AiValidation.requireDisplayName(displayName),
            api = api.wire,
            baseURL = AiValidation.normalizeBaseUrl(baseURL, trust),
            credentialRef = AiValidation.requireCredentialRef(credentialRef),
            endpointTrust = trust.wire,
            enabled = true,
        )
    }

    /**
     * 目录条目 → `AiModelDto`（写入形状）。做成 internal 是为了让单测能拿它
     * 构造"我们写进去、再读回来的那一行"，从而只测 planSeed 的分支而不碰数据库。
     */
    internal fun BuiltinModel.toModelDto(providerId: String): AiModelDto = AiModelDto(
        providerId = providerId,
        id = AiValidation.requireModelId(id),
        displayName = AiValidation.requireDisplayName(displayName),
        inputModalities = inputModalities,
        // 附件传输方式 / MIME 白名单**故意留空**：适配器还没实现附件传输，
        // 现在写死一份只会让预检给出"能发图片"的假承诺（AIH-028）。
        attachmentTransports = emptyMap(),
        mimeAllowlist = emptyList(),
        tools = tools,
        parallelTools = parallelTools,
        reasoning = reasoning,
        thinkingEfforts = thinkingEfforts,
        thinkingFormat = thinkingFormat,
        contextWindow = contextWindow,
        capabilitySource = CapabilitySource.BUILTIN.wire,
        enabled = true,
    )

    /** 与 `AiRoutes.hostOf` 同义：拿不到 host 就交给 [EndpointTrust.infer] 空串的保守结果。 */
    private fun hostOf(url: String): String =
        runCatching { java.net.URI(url.trim()).host.orEmpty() }.getOrDefault("")

    private fun safeVersion(): String = runCatching { AiSeedCatalog.CATALOG_VERSION }.getOrDefault(UNKNOWN_VERSION)

    private fun describe(t: Throwable): String = "${t::class.simpleName}: ${t.message ?: "无详情"}"
}
