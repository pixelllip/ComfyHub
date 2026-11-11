package com.comfyhub.harness.store

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * **本机那份 Provider 清单**（M6 的另一半）：`ai_providers` / `ai_models` 的增删改查。
 *
 * 用户拍板的原话是"手机完全可以独立配置 provider，有密钥的也同步到手机端"：
 * 密钥那一半早就做完了，可**清单这一半一直不成立**（页面清一色打桌面 HTTP），
 * 而这两张表**本来就是本机表**（不在 `MobileSchema.MIRROR_TABLES` 里）。
 *
 * 用例盯的都是"会静默出错 / 与桌面不一致"的地方：
 *  - **id 冲突**不许静默覆盖（覆盖会把用户正在用的那条的地址一起改掉）；
 *  - **乐观锁**（AIH-007）的文案与判据与桌面**逐字同句**；
 *  - **删 provider 级联删模型**（R1 的 `foreign_keys` 自检在守着）；
 *  - `saveModels` 是**全量替换**，而"校验失败却已经把旧目录删了"是最坏的一种半途而废；
 *  - JSON 键名与 `AiProviderDto` / `AiModelDto` 逐字对齐，且**不写 `credential`**。
 */
class ProviderStoreTest {

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("provider-store-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    // --- 零件 ---------------------------------------------------------------

    private fun provider(
        id: String = "phone-gw",
        name: String = "手机网关",
        api: String = "openai-completions",
        baseUrl: String = "http://192.168.1.9:11434/v1",
        ref: String? = "PHONE_KEY",
        trust: String = "private-network",
        revision: Int = 0,
        enabled: Boolean = true,
    ) = ProviderRecord(
        id = id, displayName = name, api = api, baseUrl = baseUrl, credentialRef = ref,
        endpointTrust = trust, enabled = enabled, revision = revision,
        createdAt = "", updatedAt = "",
    )

    private fun model(
        modelId: String = "m-text",
        name: String = "纯文本模型",
        modalities: List<String> = listOf("text", "image"),
        tools: Boolean = true,
        thinkingFormat: String? = null,
        contextWindow: Int? = 128000,
    ) = ModelRecord(
        providerId = "", modelId = modelId, displayName = name,
        inputModalities = modalities,
        attachmentTransports = mapOf("image" to listOf("inline_base64")),
        mimeAllowlist = listOf("image/png"),
        tools = tools, parallelTools = false, reasoning = thinkingFormat != null,
        thinkingEfforts = if (thinkingFormat != null) mapOf("low" to "512") else emptyMap(),
        thinkingFormat = thinkingFormat,
        contextWindow = contextWindow, maxOutputTokens = 8192, maxAttachmentCount = 10,
        capabilitySource = "manual", enabled = true,
    )

    // --- 新建 / 读回 --------------------------------------------------------

    @Test
    fun `新建：revision 由库给 1，字段逐条读得回来`() {
        val saved = store.saveProvider(provider())
        assertEquals("新建的 revision 由库给 1（与桌面 insert_provider 同）", 1, saved.revision)
        assertEquals("手机网关", saved.displayName)
        assertEquals("PHONE_KEY", saved.credentialRef)
        assertTrue("created_at / updated_at 由库写", saved.createdAt.isNotBlank() && saved.updatedAt.isNotBlank())

        val read = store.getProvider("phone-gw")!!
        assertEquals(saved, read)
        assertEquals(listOf("phone-gw"), store.listProviders().map { it.id })
    }

    /**
     * 顺序照抄桌面 `ORDER BY display_name, id`：两端列表位置必须一致。
     *
     * ⚠ 排序是 **BINARY（按 UTF-8 码点）**，不是拼音/本地化（桌面也**没有** `COLLATE NOCASE`，
     * 顺手加一个"更合理"的排序就会让两端顺序不同）。这里用两个中文字把这条钉住：
     * "乙"（U+4E59）< "甲"（U+7532），所以乙在前 —— 与拼音直觉相反，但这是**两端一致**的那个顺序。
     */
    @Test
    fun `清单顺序是 display_name 再 id（BINARY 码点序，与桌面同）`() {
        store.saveProvider(provider(id = "b", name = "乙"))
        store.saveProvider(provider(id = "a", name = "乙"))
        store.saveProvider(provider(id = "c", name = "甲"))
        assertEquals(listOf("a", "b", "c"), store.listProviders().map { it.id })
    }

    /**
     * **id 冲突不许静默覆盖**（桌面同句："Provider ID 已存在: x（ID 创建后不可修改）"）。
     *
     * 静默覆盖的后果：用户新建一条同 id 的 provider，把正在用的那条的 baseURL / 协议一起改了，
     * 而界面上只看到"新建成功"。
     */
    @Test
    fun `id 已存在时新建报错，原样留着旧那条`() {
        store.saveProvider(provider(baseUrl = "http://old:1/v1"))
        var message: String? = null
        try {
            store.saveProvider(provider(baseUrl = "http://new:2/v1"))
        } catch (e: ProviderStoreFailure) {
            message = e.message
        }
        assertNotNull(message)
        assertTrue("要指名字冲突：$message", message!!.contains("Provider ID 已存在"))
        assertEquals("旧那条一个字段都不许被改", "http://old:1/v1", store.getProvider("phone-gw")!!.baseUrl)
    }

    /** 乐观锁（AIH-007）：revision 不符 → 桌面同句；相符 → 整份更新且 revision + 1。 */
    @Test
    fun `乐观锁：revision 不符报同一句话，相符则更新并加一`() {
        val first = store.saveProvider(provider())

        var message: String? = null
        try {
            store.saveProvider(provider(name = "改过的", revision = first.revision + 5))
        } catch (e: ProviderStoreFailure) {
            message = e.message
        }
        assertNotNull(message)
        assertTrue("文案要与桌面逐字同句：$message", message!!.contains("配置已被其他窗口修改"))
        assertTrue(message.contains("当前 revision=1"))
        assertTrue(message.contains("提交的=6"))
        assertEquals("冲突时一个字段都不许改", "手机网关", store.getProvider("phone-gw")!!.displayName)

        val updated = store.saveProvider(provider(name = "改过的", baseUrl = "http://new:2/v1", revision = first.revision))
        assertEquals("改过的", updated.displayName)
        assertEquals("http://new:2/v1", updated.baseUrl)
        assertEquals("更新之后 revision + 1", 2, updated.revision)
    }

    /** 带着 revision 却找不到那一行 = 客户端拿着一份过期清单在改（桌面同句"Provider 不存在"）。 */
    @Test
    fun `带 revision 更新一个不存在的那条：如实报 Provider 不存在`() {
        var message: String? = null
        try {
            store.saveProvider(provider(id = "ghost", revision = 3))
        } catch (e: ProviderStoreFailure) {
            message = e.message
        }
        assertTrue("$message", message!!.contains("Provider 不存在: ghost"))
    }

    /** 字段为空不许写进库（写进去界面就有一条"看不见名字 / 发不出去"的 provider）。 */
    @Test
    fun `字段为空当场报错，不落库`() {
        for (bad in listOf(
            provider(id = " "),
            provider(name = " "),
            provider(api = ""),
            provider(baseUrl = ""),
        )) {
            var threw = false
            try {
                store.saveProvider(bad)
            } catch (e: ProviderStoreFailure) {
                threw = true
            }
            assertTrue("空字段必须报错", threw)
        }
        assertTrue("一条都不许落库", store.listProviders().isEmpty())
    }

    // --- 删除级联 -----------------------------------------------------------

    @Test
    fun `删 provider 级联删掉它的模型（R1）`() {
        val p = store.saveProvider(provider())
        store.saveModels(p.id, listOf(model("m1"), model("m2")))
        assertEquals(2, store.listModels(p.id).size)

        assertTrue(store.deleteProvider(p.id))
        assertEquals(0, store.listModels(p.id).size)
        assertEquals(
            "表里也真的没了（不是只查不到）",
            0L,
            db.session.queryOne("SELECT COUNT(*) AS c FROM ai_models WHERE provider_id = ?", listOf(p.id))!!.long("c"),
        )
        assertFalse("删一个不存在的回 false（库这一层不抛）", store.deleteProvider("ghost"))
        assertNull(store.getProvider("phone-gw"))
    }

    // --- 模型目录：全量替换 -------------------------------------------------

    @Test
    fun `saveModels 是全量替换，顺序按 display_name 再 model_id`() {
        val p = store.saveProvider(provider())
        // 同一条 BINARY 排序：乙（U+4E59）< 甲（U+7532），所以乙在前（与桌面一致）
        store.saveModels(p.id, listOf(model("m-b", "乙模型"), model("m-a", "甲模型")))
        assertEquals(listOf("m-b", "m-a"), store.listModels(p.id).map { it.modelId })

        // 再存一份（只剩一个）：旧的那份必须**真的没了**（AIH-011 的"全量替换"）
        val after = store.saveModels(p.id, listOf(model("m-c", "丙模型")))
        assertEquals(listOf("m-c"), after.map { it.modelId })
        assertEquals(listOf("m-c"), store.listModels(p.id).map { it.modelId })
    }

    @Test
    fun `saveModels 的字段与 JSON 列原样读回`() {
        val p = store.saveProvider(provider())
        store.saveModels(p.id, listOf(model("m-think", "会思考", thinkingFormat = "deepseek")))
        val m = store.listModels(p.id).single()
        assertEquals(listOf("text", "image"), m.inputModalities)
        assertEquals(mapOf("image" to listOf("inline_base64")), m.attachmentTransports)
        assertEquals(listOf("image/png"), m.mimeAllowlist)
        assertEquals(mapOf("low" to "512"), m.thinkingEfforts)
        assertEquals("deepseek", m.thinkingFormat)
        assertEquals(128000, m.contextWindow)
        assertEquals(8192, m.maxOutputTokens)
        assertEquals(10, m.maxAttachmentCount)
        assertTrue(m.tools)
        assertTrue("声明了思考方言 = 会推理", m.reasoning)
        assertEquals("manual", m.capabilitySource)
    }

    /** provider 不存在就抛（桌面 `PUT …/models` 的第一条：先 provider 存在）。 */
    @Test
    fun `saveModels 对不存在的 provider 报错`() {
        var message: String? = null
        try {
            store.saveModels("ghost", listOf(model()))
        } catch (e: ProviderStoreFailure) {
            message = e.message
        }
        assertTrue("$message", message!!.contains("Provider 不存在: ghost"))
    }

    /**
     * **校验失败时旧目录一个字节都不许动**。
     *
     * 桌面把顺序写死了（先 provider、再逐条校验、最后一个事务里删旧插新）：反过来的话，
     * 界面看到的是一份**合法的空目录**（"这个网关没有模型"），用户会去查网关。
     */
    @Test
    fun `saveModels 校验不过整批不做，旧目录原样留着`() {
        val p = store.saveProvider(provider())
        store.saveModels(p.id, listOf(model("m-keep", "留着")))
        assertEquals(listOf("m-keep"), store.listModels(p.id).map { it.modelId })

        for (bad in listOf(listOf(model(" ", "没有 id")), listOf(model("m1", "  ")))) {
            var threw = false
            try {
                store.saveModels(p.id, bad)
            } catch (e: ProviderStoreFailure) {
                threw = true
            }
            assertTrue("不合法的模型必须报错", threw)
        }
        assertEquals("旧目录一个字节都不许动", listOf("m-keep"), store.listModels(p.id).map { it.modelId })
    }

    /** `listModels` 对不存在的 provider 回空表（判据只写一处：桥先查 provider 再决定报不报错）。 */
    @Test
    fun `listModels 对不存在的 provider 回空表`() {
        assertTrue(store.listModels("ghost").isEmpty())
    }

    // --- JSON 形状（桥那一侧进出都是它） ------------------------------------

    /**
     * Provider 的 JSON **键集合精确对齐** `AiProviderDto`，而且**不写 `credential`**。
     *
     * `credential` 是"这台机器上有没有 key"的**即时状态**：手机上是本机密钥库说了算
     * （界面用 `CredentialStore.statusOf` 现问）。桥编一个"未配置"会变成那条老坑
     * ——界面上显示"已配置"、点发送却 401。
     */
    @Test
    fun `provider JSON：键集合与 DTO 一致，且没有 credential`() {
        val saved = store.saveProvider(provider())
        val json = ProviderJson.providerToJson(saved)
        assertEquals(
            setOf(
                "id", "displayName", "api", "baseURL", "credentialRef", "endpointTrust",
                "enabled", "revision", "createdAt", "updatedAt",
            ),
            json.keys,
        )
        assertFalse("桥上不许编一个可能过期的凭据状态", json.containsKey("credential"))

        // 往返：读回来还是同一条（可空键省掉，不写 null）
        val back = ProviderJson.providerFromJson(Json.parseToJsonElement(json.toString()).jsonObject)
        assertEquals(saved, back)

        val noRef = ProviderJson.providerToJson(store.saveProvider(provider(id = "p2", ref = null)))
        assertFalse("可空的键要省掉，不是写 null", noRef.containsKey("credentialRef"))
        assertNull(ProviderJson.providerFromJson(noRef).credentialRef)
    }

    /** 额外请求参数：落库、读回、JSON 往返；清空后回到空表（不能残留旧值）。 */
    @Test
    fun `额外请求参数：落库读回与清空`() {
        val saved = store.saveProvider(
            provider().copy(
                headers = mapOf("x-opencode-session" to "ses_abc", "X-Client" to "ComfyHub"),
                query = mapOf("api-version" to "2024-10-21"),
            ),
        )
        assertEquals("ses_abc", saved.headers["x-opencode-session"])
        assertEquals("2024-10-21", saved.query["api-version"])

        // 重新读一行（不是拿内存里那份）
        val fromDb = store.getProvider("phone-gw")!!
        assertEquals(saved.headers, fromDb.headers)
        assertEquals(saved.query, fromDb.query)

        // 桥进出的 JSON 也要原样带上；缺省不写空对象（与后端 skip_serializing_if 一致）
        val json = ProviderJson.providerToJson(fromDb)
        assertEquals(
            setOf("X-Client", "x-opencode-session"),
            (json["headers"] as kotlinx.serialization.json.JsonObject).keys,
        )
        assertEquals(fromDb, ProviderJson.providerFromJson(json))

        // 清空：更新之后两列都回 NULL，读回来是空表
        val cleared = store.saveProvider(
            fromDb.copy(headers = emptyMap(), query = emptyMap(), revision = fromDb.revision),
        )
        assertTrue(cleared.headers.isEmpty())
        assertTrue(cleared.query.isEmpty())
        assertTrue(ProviderJson.providerToJson(cleared).keys.none { it == "headers" || it == "query" })
    }

    /** 模型 JSON：可空键省掉、数组/映射原样，且 **providerId 一律以参数为准**。 */
    @Test
    fun `模型 JSON：往返一致，providerId 以调用方给的为准`() {
        val m = model("m-json", "JSON 模型").copy(providerId = "someone-else")
        val json = ProviderJson.modelToJson(m)
        assertFalse("没给 thinkingFormat 就不该有这个键", json.containsKey("thinkingFormat"))
        assertEquals("模型 JSON 里不许凭空多出 DTO 没有的键", setOf(
            "providerId", "id", "displayName", "inputModalities", "attachmentTransports",
            "mimeAllowlist", "tools", "parallelTools", "reasoning", "thinkingEfforts",
            "contextWindow", "maxOutputTokens", "maxAttachmentCount", "capabilitySource", "enabled",
        ), json.keys)

        val back = ProviderJson.modelFromJson(Json.parseToJsonElement(json.toString()).jsonObject, "phone-gw")
        assertEquals("providerId 是客户端抄来的，**不许信**", "phone-gw", back.providerId)
        assertEquals(m.copy(providerId = "phone-gw"), back)
    }

    /** 读不懂就抛：一行缺 `id`、或者缺 `api`，都不该静默变成一条空配置。 */
    @Test
    fun `JSON 读不懂就抛，不静默造一条空配置`() {
        var messages = mutableListOf<String>()
        for (bad in listOf("""{"displayName":"x","api":"a","baseURL":"b"}""", """{"id":"x","baseURL":"b"}""")) {
            try {
                ProviderJson.providerFromJson(Json.parseToJsonElement(bad).jsonObject)
            } catch (e: ProviderJson.ProviderJsonFailure) {
                messages += e.message.orEmpty()
            }
        }
        assertEquals(2, messages.size)
        assertTrue(messages[0].contains("id"))
        assertTrue(messages[1].contains("api"))

        var threw = false
        try {
            ProviderJson.modelFromJson(Json.parseToJsonElement("""{"displayName":"x"}""").jsonObject, "p")
        } catch (e: ProviderJson.ProviderJsonFailure) {
            threw = true
        }
        assertTrue("模型缺 id 也要抛", threw)
    }

    /** 表里的 JSON 文本列坏了当空（列表列空着，比整行读不出来好）—— 但**不编值**。 */
    @Test
    fun `JSON 文本列坏了当空，不编值`() {
        assertTrue(ProviderJson.stringListOf("{坏}").isEmpty())
        assertTrue(ProviderJson.stringListOf(null).isEmpty())
        assertTrue(ProviderJson.stringMapOf("[]").isEmpty())
        assertTrue(ProviderJson.stringListMapOf("nope").isEmpty())
        assertEquals(listOf("text"), ProviderJson.stringListOf("""["text"]"""))
    }

    /** 这两张表是**本机表**：不在镜像清单里、也不上行（M6 的清单各归各）。 */
    @Test
    fun `ai_providers 与 ai_models 都是本机表，不进同步范围`() {
        assertTrue(MobileSchema.LOCAL_ONLY_TABLES.contains("ai_providers"))
        assertTrue(MobileSchema.LOCAL_ONLY_TABLES.contains("ai_models"))
        assertFalse(MobileSchema.MIRROR_TABLES.any { it.name == "ai_providers" })
        assertFalse(MobileSchema.MIRROR_TABLES.any { it.name == "ai_models" })
    }
}
