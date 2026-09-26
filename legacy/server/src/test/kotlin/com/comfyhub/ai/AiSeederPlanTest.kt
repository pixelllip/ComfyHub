package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * [AiSeeder] 的"补齐 / 对齐"决策逻辑（纯单测，**不连数据库**）。
 *
 * 这里的规矩只有一条，但必须严格：**用户手工改过的模型绝不能被悄悄覆盖**。
 * 所以把"目录 × 库"的对照抽成纯函数 [AiSeeder.planSeed] 与 [AiSeeder.decideActions]，
 * 每条分支都用假数据直接钉住；数据库交互（[AiRepo.insertModel] / [AiRepo.updateModelCapabilities]）
 * 只是照着这两个函数的结论执行，没有自己的判断。
 */
class AiSeederPlanTest {

    private val provider: BuiltinProvider = AiSeedCatalog.providers().single()
    private val catalog: List<BuiltinModel> = provider.models

    /**
     * 造一行"我们写进去又读回来"的库数据：直接用 [AiSeeder.toModelDto]（seeder 自己那份映射），
     * 所以它天然等于"与目录一致"。要造差异就在这基础上 copy 改一个字段。
     */
    private fun row(m: BuiltinModel): AiModelDto = with(AiSeeder) { m.toModelDto(provider.id) }

    /** 正常行：`claude-sonnet-5`（有图、有 5 档思考）。 */
    private val sonnet = catalog.first { it.id == "claude-sonnet-5" }

    /** 裸条目：`zai-org/GLM-5.2-Fast`（仅文本、无推理）。 */
    private val bare = catalog.first { it.id == "zai-org/GLM-5.2-Fast" }

    // --- planSeed：三分支 ---------------------------------------------------

    @Test
    fun `空库时目录里的模型全部进 added`() {
        val plan = AiSeeder.planSeed(catalog, emptyMap())
        assertEquals(69, plan.added.size, "一条都没有时应该整份都算缺")
        assertEquals(catalog.map { it.id }, plan.added.map { it.id }, "顺序跟目录一致")
        assertTrue(plan.kept.isEmpty())
        assertTrue(plan.divergent.isEmpty())
    }

    @Test
    fun `库里已有且与目录一致 进 kept 不进 added`() {
        val existing = mapOf(bare.id to row(bare))
        val plan = AiSeeder.planSeed(catalog, existing)
        assertEquals(listOf(bare.id), plan.kept)
        assertTrue(plan.divergent.isEmpty())
        assertEquals(68, plan.added.size)
        assertFalse(plan.added.any { it.id == bare.id }, "已有的模型不该再插一遍")
    }

    @Test
    fun `库里已有但能力声明不同 进 divergent 不进 added 也不进 kept`() {
        // 真实场景：库里那份是接口/手工导入的乐观值（模态全开、reasoning 没开）
        val imported = row(sonnet).copy(
            inputModalities = listOf("text", "image", "video", "audio", "document"),
            reasoning = false,
            thinkingEfforts = emptyMap(),
            thinkingFormat = null,
            capabilitySource = "manual",
        )
        val plan = AiSeeder.planSeed(catalog, mapOf(sonnet.id to imported))
        assertEquals(listOf(sonnet.id), plan.divergent)
        assertTrue(plan.kept.isEmpty())
        assertEquals(68, plan.added.size)
        assertFalse(plan.added.any { it.id == sonnet.id }, "不一致的行保持用户设置，不重插、不覆盖")
    }

    @Test
    fun `用户自己加的模型三列都不进 也就是永远不会被删`() {
        val mine = AiModelDto(
            providerId = provider.id,
            id = "my-private-tuned-model",
            displayName = "我自己接的模型",
            inputModalities = listOf("text"),
            capabilitySource = "manual",
        )
        val plan = AiSeeder.planSeed(catalog, mapOf(mine.id to mine))
        assertFalse(plan.added.any { it.id == mine.id })
        assertFalse(plan.kept.contains(mine.id))
        assertFalse(plan.divergent.contains(mine.id))
        assertEquals(69, plan.added.size, "目录里的 69 个照旧补")
    }

    @Test
    fun `全库一致时没有新增也没有分歧`() {
        val existing = catalog.associate { it.id to row(it) }
        val plan = AiSeeder.planSeed(catalog, existing)
        assertTrue(plan.added.isEmpty())
        assertTrue(plan.divergent.isEmpty())
        assertEquals(69, plan.kept.size)
    }

    // --- capabilityDiffers：逐字段 -----------------------------------------

    @Test
    fun `完全一致时不算不同`() {
        assertFalse(AiSeeder.capabilityDiffers(sonnet, row(sonnet)))
        assertFalse(AiSeeder.capabilityDiffers(bare, row(bare)))
    }

    @Test
    fun `模态 工具 并行工具 推理 档位 方言 上下文 来源 任一不同都算不同`() {
        val base = row(sonnet)
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(inputModalities = listOf("text"))), "模态")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(tools = false)), "工具")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(parallelTools = true)), "并行工具")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(reasoning = false)), "推理")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(thinkingEfforts = emptyMap())), "思考档位")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(thinkingEfforts = base.thinkingEfforts + ("minimal" to "minimal"))), "多一档也算")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(thinkingFormat = null)), "思考方言")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(contextWindow = 200000)), "上下文窗口")
        assertTrue(AiSeeder.capabilityDiffers(sonnet, base.copy(capabilitySource = "manual")), "能力来源")
    }

    @Test
    fun `模态顺序不同不算不同`() {
        val base = row(sonnet)
        assertFalse(AiSeeder.capabilityDiffers(sonnet, base.copy(inputModalities = listOf("image", "text"))))
    }

    @Test
    fun `显示名 启用状态 附件上限属于用户地盘 不算能力不同`() {
        // 对齐能力不该因为用户改了显示名 / 停用了模型 / 设了附件上限就去 UPDATE 它
        val base = row(sonnet)
        assertFalse(AiSeeder.capabilityDiffers(sonnet, base.copy(displayName = "我给它起的名字")))
        assertFalse(AiSeeder.capabilityDiffers(sonnet, base.copy(enabled = false)))
        assertFalse(AiSeeder.capabilityDiffers(sonnet, base.copy(maxAttachmentBytes = 5_000_000, maxAttachmentCount = 3)))
        assertFalse(AiSeeder.capabilityDiffers(sonnet, base.copy(maxOutputTokens = 8192)))
    }

    // --- decideActions：两种模式的边界 --------------------------------------

    @Test
    fun `ADD_MISSING 只插入不刷新`() {
        val plan = SeedPlan(
            added = listOf(bare),
            kept = listOf(sonnet.id),
            divergent = listOf("some-existing-model"),
        )
        val actions = AiSeeder.decideActions(SeedMode.ADD_MISSING, plan)
        assertEquals(listOf(bare.id), actions.toInsert)
        assertTrue(actions.toRefresh.isEmpty(), "启动路径绝不能去改已有行")
    }

    @Test
    fun `REFRESH_CAPABILITIES 只刷新不新增`() {
        val plan = SeedPlan(
            added = listOf(bare),
            kept = listOf(sonnet.id),
            divergent = listOf("some-existing-model"),
        )
        val actions = AiSeeder.decideActions(SeedMode.REFRESH_CAPABILITIES, plan)
        assertTrue(actions.toInsert.isEmpty(), "显式对齐也不能新增")
        assertEquals(listOf("some-existing-model"), actions.toRefresh)
    }

    @Test
    fun `REFRESH 的刷新目标就是与目录不一致的那些`() {
        val existing = catalog.associate { it.id to row(it) } +
            (sonnet.id to row(sonnet).copy(reasoning = false, thinkingEfforts = emptyMap()))
        val plan = AiSeeder.planSeed(catalog, existing)
        val actions = AiSeeder.decideActions(SeedMode.REFRESH_CAPABILITIES, plan)
        assertEquals(listOf(sonnet.id), actions.toRefresh)
        assertEquals(68, plan.kept.size, "一致的那些一个都不动")
        assertTrue(plan.added.isEmpty())
    }

    @Test
    fun `两种模式都不会碰用户的模型`() {
        val mine = AiModelDto(providerId = provider.id, id = "mine", displayName = "mine")
        val existing = catalog.associate { it.id to row(it) } + (mine.id to mine)
        val plan = AiSeeder.planSeed(catalog, existing)
        for (mode in SeedMode.entries) {
            val actions = AiSeeder.decideActions(mode, plan)
            assertFalse(actions.toInsert.contains(mine.id), "$mode 不该动用户自己加的模型")
            assertFalse(actions.toRefresh.contains(mine.id), "$mode 不该动用户自己加的模型")
        }
    }

    // --- previewBuiltinSync：只读预览 ---------------------------------------
    //
    // 这里测的是 `previewBuiltinSync()` 的**纯逻辑那一层**（`AiSeeder.previewPlan`：
    // 喂"已经读到的库状态"，返回"会发生什么"），它自己不碰 AiRepo，所以不需要 MySQL。
    // "真的一个字节都没写"由读路径保证（只有 getProvider / listModels 两条 SELECT），
    // 另有临时库探针手工验证：调完预览后 provider 行与 `ai.builtin.seed.version` 都没变。

    private val V = AiSeedCatalog.CATALOG_VERSION

    @Test
    fun `预览- provider 不存在 报整份目录且标记会新建`() {
        val p = AiSeeder.previewPlan(version = V, providerId = provider.id, catalog = catalog, existing = null)
        assertEquals(SeedMode.ADD_MISSING, p.mode)
        assertEquals(69, p.added, "provider 不在时要插目录里全部")
        assertEquals(0, p.kept)
        assertEquals(0, p.updated, "预览永远不刷新")
        assertTrue(p.divergent.isEmpty())
        assertEquals(69, p.modelCount)
        assertTrue(p.providerCreated)
        assertTrue(p.ok)
    }

    @Test
    fun `预览- provider 已存在时口径与 ADD_MISSING 的 plan 一致 且绝不刷新`() {
        // 一条一致（sonnet）、一条被用户改过（bare）、一条用户自己加的
        val mine = AiModelDto(providerId = provider.id, id = "mine", displayName = "mine")
        val existing = mapOf(
            sonnet.id to row(sonnet),
            bare.id to row(bare).copy(reasoning = true, thinkingEfforts = mapOf("high" to "high")),
            mine.id to mine,
        )
        val p = AiSeeder.previewPlan(version = V, providerId = provider.id, catalog = catalog, existing = existing)
        val plan = AiSeeder.planSeed(catalog, existing)
        val actions = AiSeeder.decideActions(SeedMode.ADD_MISSING, plan)

        assertEquals(plan.added.size, p.added, "预览的 added 就是 ADD_MISSING 会 INSERT 的条数")
        assertEquals(actions.toInsert.size, p.added)
        assertEquals(plan.kept.size, p.kept)
        assertEquals(plan.divergent, p.divergent)
        assertEquals(listOf(bare.id), p.divergent, "被用户改过的那条只是被报出来，不在刷新目标里")
        assertTrue(actions.toRefresh.isEmpty(), "ADD_MISSING 没有刷新动作")
        assertEquals(0, p.updated, "预览的 updated 恒为 0")
        assertEquals(existing.size + plan.added.size, p.modelCount, "跑完后的总数 = 现有（含用户自加）+ 要补的")
        assertFalse(p.providerCreated)
    }

    @Test
    fun `预览- 全部一致时什么都不用做`() {
        val existing = catalog.associate { it.id to row(it) }
        val p = AiSeeder.previewPlan(version = V, providerId = provider.id, catalog = catalog, existing = existing)
        assertEquals(0, p.added)
        assertEquals(69, p.kept)
        assertTrue(p.divergent.isEmpty())
        assertEquals(0, p.updated)
        assertEquals(69, p.modelCount)
    }
}
