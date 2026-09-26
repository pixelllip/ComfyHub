package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 内置模型目录的冻结副本（用户报的 bug：运行时不能读 `.dsh/settings.yaml`）。
 *
 * 这里**只做纯单测、不连数据库**：`server.ps1 test` 允许在没有 MySQL 的机器上跑
 * （[AiSeeder] 落库要有库，那部分靠 `AiSeeder.ensureBuiltinProvider()` 的幂等设计保证，
 * 见源码注释；这里只锁"资源抄得对不对"）。
 *
 * 数据真源：`C:\Users\<user>\.dsh\settings.yaml` → `scripts/gen-builtin-catalog.ps1`
 * → `server/src/main/resources/ai/builtin-catalog.json`。
 */
class AiSeedCatalogTest {

    private val provider: BuiltinProvider = AiSeedCatalog.providers().single()

    private fun model(id: String): BuiltinModel =
        provider.models.firstOrNull { it.id == id } ?: error("内置目录里没有模型 $id")

    // --- 目录规模 -----------------------------------------------------------

    @Test
    fun `资源能加载 且正好是 1 个 provider 69 个模型`() {
        val catalog = AiSeedCatalog.load()
        assertEquals(1, AiSeedCatalog.providers().size, "settings.yaml 里只有 1 个 provider")
        assertEquals(69, provider.models.size, "settings.yaml 里正好 69 个模型")
        assertEquals("2026-09b-dsh", catalog.version)
        assertEquals(catalog.version, AiSeedCatalog.CATALOG_VERSION, "版本必须与资源同源")
        // 模型 id 不允许重复（重复就是抄漏/抄重的信号）
        val ids = provider.models.map { it.id }
        assertEquals(ids.size, ids.toSet().size, "模型 id 有重复：$ids")
        assertTrue(provider.models.all { it.displayName.isNotBlank() }, "每个模型都要有显示名")
    }

    @Test
    fun `provider 元信息也一起抄过来了`() {
        assertEquals("command-code-goat", provider.id)
        assertEquals("Command Code GOAT", provider.displayName)
        assertEquals("openai-completions", provider.api)
        assertEquals("https://api.commandcode.ai/provider/v1/", provider.baseURL)
        assertEquals("COMMAND_CODE_GOAT_API_KEY", provider.credentialRef)
        assertEquals("public", provider.endpointTrust)
        // `agent-default-model` 一并抄下来，新建会话有默认模型可用
        val def = AiSeedCatalog.agentDefaultModel()
        assertNotNull(def)
        assertEquals("command-code-goat", def.provider)
        assertEquals("deepseek/deepseek-v4.1-flash", def.model)
        assertNotNull(AiSeedCatalog.find("command-code-goat"))
        assertNull(AiSeedCatalog.find("no-such-provider"), "找不到就是找不到，不猜")
    }

    // --- 逐条抽样（覆盖各家族与声明形态）-------------------------------------

    @Test
    fun `claude-sonnet-5 图片输入 思考档位是 6 档去掉 off`() {
        val m = model("claude-sonnet-5")
        assertEquals("Claude Sonnet 5", m.displayName)
        assertEquals(1000000, m.contextWindow)
        assertEquals(listOf("text", "image"), m.inputModalities)
        assertTrue(m.reasoning)
        assertEquals("openai", m.thinkingFormat)
        assertEquals(
            mapOf("low" to "low", "medium" to "medium", "high" to "high", "xhigh" to "xhigh", "max" to "max"),
            m.thinkingEfforts,
        )
    }

    @Test
    fun `claude-haiku-4-5 有图片但没有思考`() {
        val m = model("claude-haiku-4-5-20251001")
        assertEquals("Claude Haiku 4.5", m.displayName)
        assertEquals(200000, m.contextWindow)
        assertEquals(listOf("text", "image"), m.inputModalities)
        assertFalse(m.reasoning)
        assertTrue(m.thinkingEfforts.isEmpty(), "YAML 里没写 reasoningEfforts，就不能凭空造档位")
    }

    @Test
    fun `deepseek-v4-pro 纯文本 只有 high 和 max`() {
        val m = model("deepseek/deepseek-v4-pro")
        assertEquals("DeepSeek V4 Pro (latest)", m.displayName)
        assertEquals(1000000, m.contextWindow)
        assertEquals(listOf("text"), m.inputModalities, "YAML 里没写 input ⇒ 仅文本")
        assertTrue(m.reasoning)
        assertEquals(mapOf("high" to "high", "max" to "max"), m.thinkingEfforts)
    }

    @Test
    fun `deepseek-v4点1-flash 是 agent 默认模型`() {
        val m = model("deepseek/deepseek-v4.1-flash")
        assertEquals(AiSeedCatalog.agentDefaultModel()!!.model, m.id)
        assertEquals("DeepSeek V4.1 Flash", m.displayName)
        assertEquals(1000000, m.contextWindow)
        assertEquals(listOf("text", "image"), m.inputModalities)
        assertTrue(m.reasoning)
        assertEquals(mapOf("low" to "low", "high" to "high", "max" to "max"), m.thinkingEfforts)
    }

    @Test
    fun `GLM-5点2-Fast 是裸条目 仅 id name contextWindow`() {
        // YAML 那行就三个字段：{ id: zai-org/GLM-5.2-Fast, name: GLM-5.2 Fast, contextWindow: 1000000 }
        val m = model("zai-org/GLM-5.2-Fast")
        assertEquals("GLM-5.2 Fast", m.displayName)
        assertEquals(1000000, m.contextWindow)
        assertEquals(listOf("text"), m.inputModalities)
        assertFalse(m.reasoning)
        assertTrue(m.thinkingEfforts.isEmpty())
    }

    @Test
    fun `grok-4点6 的 xhigh 没被降级`() {
        val m = model("xai/grok-4.6")
        assertEquals("Grok 4.6", m.displayName)
        assertEquals(500000, m.contextWindow)
        assertEquals(listOf("text", "image"), m.inputModalities)
        assertTrue(m.reasoning)
        assertEquals(mapOf("low" to "low", "medium" to "medium", "high" to "high", "xhigh" to "xhigh"), m.thinkingEfforts)
        assertFalse(m.thinkingEfforts.containsKey("max"), "它没有 max 档就不该声明")
    }

    // --- 一致性不变量（全文扫描）-------------------------------------------

    @Test
    fun `没有任何模型把 off 写进思考档位`() {
        for (m in provider.models) {
            assertFalse(m.thinkingEfforts.containsKey("off"), "${m.id} 的 thinkingEfforts 里不该有 off（off 是隐含的）")
        }
    }

    @Test
    fun `reasoning 与非空思考档位一一对应`() {
        for (m in provider.models) {
            assertEquals(
                m.reasoning,
                m.thinkingEfforts.isNotEmpty(),
                "${m.id}: reasoning=${m.reasoning} 但档位=${m.thinkingEfforts}（保存时会被 validateThinkingEfforts 拒绝）",
            )
            // 档位的键必须是认识的等级，值必须与键同形的线上表达
            m.thinkingEfforts.forEach { (level, wire) ->
                assertNotNull(com.comfyhub.ai.protocol.ReasoningEffort.parse(level), "${m.id} 的等级 $level 不认识")
                assertTrue(wire.isNotBlank(), "${m.id} 的 $level 表达为空")
            }
            // 项目约定：工具默认给上，并行工具不支持
            assertTrue(m.tools, "${m.id} 应该支持工具")
            assertFalse(m.parallelTools, "${m.id} 未声明并行工具")
            // 模态只能是 text / image（YAML 里没有别的）
            assertTrue(
                m.inputModalities.all { it == "text" || it == "image" },
                "${m.id} 出现了意外的模态：${m.inputModalities}",
            )
            assertTrue(m.inputModalities.contains("text"), "${m.id} 至少要吃文本")
        }
    }

    @Test
    fun `资源里没有凭空发明的附件字段`() {
        // 「不要发明 attachmentTransports / mimeAllowlist」这条规则直接锁在资源文本上：
        // 适配器还没实现附件传输，写了就是假承诺（AIH-028）。
        val text = javaClass.getResourceAsStream(AiSeedCatalog.RESOURCE)!!
            .use { it.readBytes().toString(Charsets.UTF_8) }
        for (invented in listOf("attachmentTransports", "mimeAllowlist", "maxAttachmentBytes", "maxAttachmentCount")) {
            assertFalse(text.contains(invented), "资源里不该出现 $invented")
        }
        assertTrue(text.contains("\"thinkingEfforts\": {}"), "没推理能力的模型应显式给空档位表")
    }

    /*
     * 与生成器的一致性：
     *
     *   pwsh -File scripts\gen-builtin-catalog.ps1 -OutPath $env:TEMP\catalog-gen.json
     *   Get-FileHash server\src\main\resources\ai\builtin-catalog.json
     *   Get-FileHash $env:TEMP\catalog-gen.json
     *
     * 2026-09-15 手工执行过：两边 sha256 都是
     *   4d18a7d10dac5066f00fa97321aeed2d3f36598af6d9877b06ea5ee0d614fc9b
     * 也就是"生成器产物 == 资源文件"逐字节相同（生成器写 LF + UTF-8 无 BOM，末尾带换行）。
     * 不放进单测是因为测试进程里再起一个 pwsh 解析 YAML 又慢又依赖开发机有 .dsh\settings.yaml，
     * 而 CI / 别人机器上恰恰没有那个文件 —— 与本次修复的初衷冲突。
     */
}
