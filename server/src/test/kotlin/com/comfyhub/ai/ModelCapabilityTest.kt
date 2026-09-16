package com.comfyhub.ai

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 模型能力预填（AIH-009 / AIH-011）。
 *
 * 三条不能破的规矩：
 *  1. 接口声明的能力**优先于**内置目录；
 *  2. 内置目录命中的要**标明来源**（界面上要显示"内置目录"）；
 *  3. 两边都没有的模型只能给"仅文本"，**绝不按名字猜图片能力**。
 */
class ModelCapabilityTest {

    @Test
    fun `OpenRouter 的 architecture 声明优先于内置目录`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"data":[
              {"id":"some-vision-model","architecture":{"input_modalities":["text","image"],"modality":"text+image->text"}}
            ]}
            """.trimIndent()
        )
        val c = candidates.single()
        assertEquals(listOf("text", "image"), c.modalities)
        assertEquals(CapabilitySource.DISCOVERED.wire, c.capabilitySource)
    }

    @Test
    fun `布尔开关形态也能识别 含 capabilities 对象`() {
        val flags = AiUpstream.parseCandidates(
            """{"data":[{"id":"m1","supports_vision":true,"supports_tools":true}]}"""
        ).single()
        assertTrue(flags.modalities.contains("image"))
        assertTrue(flags.tools)
        assertEquals(CapabilitySource.DISCOVERED.wire, flags.capabilitySource)

        val caps = AiUpstream.parseCandidates(
            """{"data":[{"id":"m2","capabilities":{"vision":true,"function_calling":true}}]}"""
        ).single()
        assertTrue(caps.modalities.contains("image"))
        assertTrue(caps.tools)
    }

    @Test
    fun `接口没说时回落到内置目录 并标明来源`() {
        val c = AiUpstream.parseCandidates("""{"data":[{"id":"gpt-4o-mini"}]}""").single()
        assertEquals(listOf("text", "image"), c.modalities)
        assertTrue(c.tools)
        assertEquals(CapabilitySource.BUILTIN.wire, c.capabilitySource)
        assertTrue(c.capabilityNote!!.contains("内置目录"))
    }

    @Test
    fun `内置目录覆盖常见多模态与纯文本系列`() {
        assertEquals(listOf("text", "image"), ModelCapabilityCatalog.lookup("claude-3-5-sonnet-20241022")!!.modalities)
        assertEquals(listOf("text", "image"), ModelCapabilityCatalog.lookup("gpt-4.1-mini")!!.modalities)
        assertEquals(listOf("text", "image"), ModelCapabilityCatalog.lookup("qwen2.5-vl-72b")!!.modalities)
        assertEquals(listOf("text"), ModelCapabilityCatalog.lookup("o1-mini")!!.modalities)
        assertTrue(ModelCapabilityCatalog.lookup("o1-mini")!!.reasoning)
        assertEquals(listOf("text"), ModelCapabilityCatalog.lookup("deepseek-reasoner")!!.modalities)
        assertTrue(ModelCapabilityCatalog.lookup("deepseek-reasoner")!!.reasoning)
    }

    @Test
    fun `目录里没有的模型只给文本 绝不猜图片`() {
        val c = AiUpstream.parseCandidates("""{"data":[{"id":"acme-mystery-9000"}]}""").single()
        assertEquals(listOf("text"), c.modalities)
        assertEquals("unknown", c.capabilitySource)
        assertTrue(!c.tools)
        assertNull(ModelCapabilityCatalog.lookup("acme-mystery-9000"))
        assertTrue(c.capabilityNote!!.contains("未声明"))
    }

    @Test
    fun `老格式的 data 数组依然照常解析`() {
        val candidates = AiUpstream.parseCandidates(
            """{"object":"list","data":[{"id":"gpt-4o","object":"model","owned_by":"openai"}]}"""
        )
        assertEquals(1, candidates.size)
        assertEquals("gpt-4o", candidates.single().id)
        assertTrue(candidates.single().modalities.contains("image"))
    }

    @Test
    fun `语料里混入未知字段不影响解析`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"data":[
              {"id":"glm-4v-plus","weird_field":{"nested":[1,2,3]}},
              {"id":"llava-1.6","pricing":{"prompt":"0.1"}}
            ]}
            """.trimIndent()
        )
        assertEquals(2, candidates.size)
        assertTrue(candidates.all { it.modalities == listOf("text", "image") })
    }

    // --- 思考档位预填（AIH-056）---------------------------------------------

    @Test
    fun `内置目录会预填思考档位 并带上方言`() {
        val candidates = AiUpstream.parseCandidates(
            """
            {"data":[
              {"id":"deepseek-reasoner","object":"model"},
              {"id":"gpt-5.6-sol","object":"model"}
            ]}
            """.trimIndent()
        )
        val ds = candidates.first { it.id == "deepseek-reasoner" }
        assertTrue(ds.reasoning, "内置目录知道它支持推理")
        assertTrue(ds.thinkingEfforts.containsKey("high"), "预填的档位：${ds.thinkingEfforts}")
        assertEquals("deepseek", ds.thinkingFormat, "DeepSeek 的思考开关要发 thinking{type}")

        val gpt5 = candidates.first { it.id == "gpt-5.6-sol" }
        assertTrue(gpt5.reasoning)
        assertTrue(gpt5.thinkingEfforts.containsKey("low"))
        assertNull(gpt5.thinkingFormat, "OpenAI 官方方言不需要额外字段")
    }

    @Test
    fun `没声明推理的模型绝不带思考档位`() {
        // 否则保存时会被 validateThinkingEfforts 拒绝（"声明了档位却没勾推理"），
        // 用户看到的就是"获取模型后加不进去"。
        val candidates = AiUpstream.parseCandidates(
            """{"data":[{"id":"deepseek-chat","object":"model"},{"id":"gpt-4o","object":"model"}]}"""
        )
        for (c in candidates) {
            assertTrue(c.thinkingEfforts.isEmpty(), "${c.id} 不该有思考档位")
            assertNull(c.thinkingFormat, "${c.id} 不该有思考方言")
        }
    }

    @Test
    fun `接口自己声明了推理时 不叠加内置目录的档位`() {
        // capabilities.reasoning 是接口声明的形态之一
        val c = AiUpstream.parseCandidates(
            """{"data":[{"id":"deepseek-reasoner","capabilities":{"reasoning":true}}]}"""
        ).single()
        assertEquals(CapabilitySource.DISCOVERED.wire, c.capabilitySource)
        assertTrue(c.thinkingEfforts.isEmpty(), "接口声明优先，档位交给用户自己填")
    }
}
