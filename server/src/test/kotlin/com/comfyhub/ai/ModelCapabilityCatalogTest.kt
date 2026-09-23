package com.comfyhub.ai

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 能力目录覆盖层（2026-09-23）的合并语义，盯四条：
 *
 *  1. **逐维度、上层赢**：custom > remote > builtin；某层没给这一维度才落到下一层 ——
 *     "在线目录说能吃图"不会顺手把内置表的思考档位丢掉；
 *  2. **一致性**：最终 reasoning=false 时档位/方言必须清空（validateThinkingEfforts 会拦）；
 *  3. **去前缀匹配要求唯一**：网关 id 与数据集 id 一个带 `org/` 一个不带时能对上，
 *     但两个条目都叫 `…/dup` 时宁可不命中；
 *  4. **文件坏了不炸**：custom.json/remote.json 解不开 → 该层当不存在，builtin 照常工作。
 *
 * 目录是全局单例，所有用例走真实磁盘（@TempDir）+ `reload`，结束时必须清回空态。
 */
class ModelCapabilityCatalogTest {

    @AfterTest
    fun restore() {
        ModelCapabilityCatalog.reload(null)
    }

    private fun writeOverlay(dir: Path, name: String, json: String) {
        Files.createDirectories(dir)
        Files.writeString(dir.resolve(name), json)
    }

    @Test
    fun `没有覆盖层时内置表照常工作且未知模型仍为 null`() {
        assertNull(ModelCapabilityCatalog.lookup("acme-mystery-9000"))
        assertEquals(
            listOf("text", "image"),
            ModelCapabilityCatalog.lookup("claude-3-5-sonnet-20241022")!!.modalities,
        )
        val status = ModelCapabilityCatalog.status()
        assertTrue(status.builtinRules > 50, "内置规则表不应被清空")
        assertNull(status.remote)
        assertNull(status.custom)
    }

    @Test
    fun `remote 精确命中补模态、builtin 补档位与方言（逐维度合并）`() {
        val dir = Files.createTempDirectory("cap-test")
        // 内置规则 glm-5.3：TEXT_ONLY？不——TEXT_IMAGE + EFFORT_LOW_HIGH_MAX + zai。
        // remote 只给 tools=false 这一个维度，其余必须原样从 builtin 落下来
        writeOverlay(dir, ModelCapabilityCatalog.REMOTE_FILE, """{"source":"models.dev","entries":{"z-ai/glm-5.3-flash":{"tools":false}}}""")
        ModelCapabilityCatalog.reload(dir)

        val cap = assertNotNull(ModelCapabilityCatalog.lookup("z-ai/glm-5.3-flash"))
        assertEquals(listOf("text", "image"), cap.modalities)   // builtin 落下来
        assertEquals(false, cap.tools)                          // remote 赢
        assertTrue(cap.reasoning)
        assertEquals("high", cap.thinkingEfforts["high"])       // builtin 的档位没被丢
        assertEquals("zai", cap.thinkingFormat)
        assertTrue(cap.matchedBy.contains("在线目录 models.dev"), cap.matchedBy)
        assertTrue(cap.matchedBy.contains("内置规则「glm-5.3」"), cap.matchedBy)
    }

    @Test
    fun `custom 压过 remote 压过 builtin`() {
        val dir = Files.createTempDirectory("cap-test")
        writeOverlay(dir, ModelCapabilityCatalog.REMOTE_FILE, """{"source":"models.dev","entries":{"m/one":{"modalities":["text","image"],"reasoning":true,"thinkingEfforts":{"low":"low"}}}}""")
        writeOverlay(dir, ModelCapabilityCatalog.CUSTOM_FILE, """{"source":"custom","entries":{"m/one":{"modalities":["text"]}}}""")
        ModelCapabilityCatalog.reload(dir)

        val cap = assertNotNull(ModelCapabilityCatalog.lookup("m/one"))
        assertEquals(listOf("text"), cap.modalities)            // custom 赢
        assertTrue(cap.reasoning)                               // remote
        assertEquals(mapOf("low" to "low"), cap.thinkingEfforts) // remote（custom 没给档位）
    }

    @Test
    fun `最终 reasoning=false 时档位与方言必须清空`() {
        val dir = Files.createTempDirectory("cap-test")
        // 内置 deepseek-r1：reasoning=true + EFFORT_HIGH_ONLY + deepseek。
        // remote 硬说 reasoning=false → 档位/方言必须跟着消失，不能留半截
        writeOverlay(dir, ModelCapabilityCatalog.REMOTE_FILE, """{"source":"models.dev","entries":{"deepseek-r1-0528":{"reasoning":false}}}""")
        ModelCapabilityCatalog.reload(dir)

        val cap = assertNotNull(ModelCapabilityCatalog.lookup("deepseek-r1-0528"))
        assertEquals(false, cap.reasoning)
        assertTrue(cap.thinkingEfforts.isEmpty(), "reasoning=false 时档位必须清空")
        assertNull(cap.thinkingFormat)
    }

    @Test
    fun `builtin 不认识的 id 只要 remote 有就命中，缺的维度保守回退`() {
        val dir = Files.createTempDirectory("cap-test")
        writeOverlay(dir, ModelCapabilityCatalog.REMOTE_FILE, """{"source":"models.dev","entries":{"acme/brand-new-9000":{"reasoning":true,"thinkingEfforts":{"off":"none","max":"max"}}}}""")
        ModelCapabilityCatalog.reload(dir)

        val cap = assertNotNull(ModelCapabilityCatalog.lookup("acme/brand-new-9000"))
        assertEquals(listOf("text"), cap.modalities)   // builtin/remote 都没说模态 → 保守仅文本
        assertTrue(cap.reasoning)
        assertEquals("max", cap.thinkingEfforts["max"])
    }

    @Test
    fun `去前缀匹配在唯一时生效、有歧义时宁可不命中`() {
        val dir = Files.createTempDirectory("cap-test")
        writeOverlay(
            dir, ModelCapabilityCatalog.REMOTE_FILE,
            """{"source":"models.dev","entries":{"z-ai/unique-model":{"tools":false},"a/dup":{"tools":false},"b/dup":{"tools":true}}}""",
        )
        ModelCapabilityCatalog.reload(dir)

        // 我们不带前缀、数据集带 → 唯一命中
        assertEquals(false, ModelCapabilityCatalog.lookup("unique-model")!!.tools)
        // 两个条目都叫 …/dup → 不猜
        assertNull(ModelCapabilityCatalog.lookup("dup"))
    }

    @Test
    fun `文件坏了解析失败该层当不存在，builtin 照常`() {
        val dir = Files.createTempDirectory("cap-test")
        writeOverlay(dir, ModelCapabilityCatalog.CUSTOM_FILE, "{ 这不是 JSON ")
        ModelCapabilityCatalog.reload(dir)

        assertNull(ModelCapabilityCatalog.status().custom)
        assertEquals(
            listOf("text", "image"),
            ModelCapabilityCatalog.lookup("claude-3-5-sonnet-20241022")!!.modalities,
        )
    }

    @Test
    fun `reload 后 status 带上来源与条目数`() {
        val dir = Files.createTempDirectory("cap-test")
        writeOverlay(dir, ModelCapabilityCatalog.REMOTE_FILE, """{"source":"models.dev","fetchedAt":"2026-09-23T02:00:00Z","entries":{"a/1":{},"b/2":{}}}""")
        writeOverlay(dir, ModelCapabilityCatalog.CUSTOM_FILE, """{"entries":{"c/3":{}}}""")
        ModelCapabilityCatalog.reload(dir)

        val status = ModelCapabilityCatalog.status()
        assertEquals(2, status.remote!!.entryCount)
        assertEquals("models.dev", status.remote.source)
        assertEquals("2026-09-23T02:00:00Z", status.remote.fetchedAt)
        assertEquals(1, status.custom!!.entryCount)
    }
}
