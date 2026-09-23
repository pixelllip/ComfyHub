package com.comfyhub.ai

import java.nio.file.Files
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * models.dev 数据集的解析 / 合并 / 落盘 / 生效，全程离线（body 注入、temp 目录）。
 * 盯五条：
 *  1. 字段映射：`modalities.input`（pdf→document）、`tool_call`、`reasoning`、
 *     `reasoning_options` 的 effort 列表（none→off；不认识的值跳过，绝不发明）；
 *  2. **false 是声明不是缺失**：`tool_call:false` 必须落成 tools=false；
 *  3. 多家 provider 撞 id → 逐维度合并，新者优先、旧者补位，reasoning=false 不留档位；
 *  4. 落盘后 `ModelCapabilityCatalog.reload` 当场生效（lookup 能看到在线目录）；
 *  5. 刚刷新不视为过期、没有文件视为过期。
 */
class CapabilityRefreshTest {

    private val dir: java.nio.file.Path = Files.createTempDirectory("cap-refresh")

    @AfterTest
    fun restore() {
        ModelCapabilityCatalog.reload(null)
    }

    private val fixture = """
    {
      "tokengo": {"id":"tokengo","models":{
        "z-ai/glm-5.3-flash": {"id":"z-ai/glm-5.3-flash","tool_call":true,"reasoning":true,
          "reasoning_options":[{"type":"effort","values":["low","high","max"]}],
          "modalities":{"input":["text","image","pdf"]},"last_updated":"2026-08-26"},
        "x/dup": {"id":"x/dup","tool_call":false,"reasoning":false,
          "reasoning_options":[{"type":"effort","values":["high"]}],"last_updated":"2026-02-01"}
      }},
      "bothub": {"id":"bothub","models":{
        "z-ai/glm-5.3-flash": {"id":"z-ai/glm-5.3-flash","reasoning":true,
          "reasoning_options":[{"type":"effort","values":["none","high","max"]}],
          "last_updated":"2026-09-01"},
        "x/dup": {"id":"x/dup","reasoning":true,
          "reasoning_options":[{"type":"effort","values":["low","high","max"]}],
          "modalities":{"input":["text","image"]},"last_updated":"2026-01-01"}
      }}
    }
    """.trimIndent()

    @Test
    fun `解析并落盘后目录当场生效`() {
        val count = CapabilityRefresh.parseAndWrite(fixture, dir, "2026-09-23T02:00:00Z")
        assertEquals(2, count)   // glm-5.3-flash 与 dup（按 id 去重）

        ModelCapabilityCatalog.reload(dir)
        val cap = assertNotNull(ModelCapabilityCatalog.lookup("z-ai/glm-5.3-flash"))
        // 模态来自 tokengo（旧者补位：bothub 那份更新的没给模态）；pdf → document
        assertEquals(listOf("text", "image", "document"), cap.modalities)
        assertTrue(cap.reasoning)
        // 档位来自 bothub（新者优先）：none→off，其余同名
        assertEquals(setOf("off", "high", "max"), cap.thinkingEfforts.keys)
        assertEquals("none", cap.thinkingEfforts["off"])
        assertTrue(cap.matchedBy.contains("在线目录 models.dev"), cap.matchedBy)
    }

    @Test
    fun `撞 id 逐维度合并新者优先旧者补位且 reasoning false 不留档位`() {
        CapabilityRefresh.parseAndWrite(fixture, dir, "2026-09-23T02:00:00Z")
        ModelCapabilityCatalog.reload(dir)
        val cap = assertNotNull(ModelCapabilityCatalog.lookup("x/dup"))
        // 更新的 tokengo 说 tools=false + reasoning=false；更旧的 bothub 补模态
        assertEquals(false, cap.tools)
        assertEquals(listOf("text", "image"), cap.modalities)
        // reasoning=false 赢了 → 档位必须清空（哪怕两家各给了一份）
        assertFalse(cap.reasoning)
        assertTrue(cap.thinkingEfforts.isEmpty())
    }

    @Test
    fun `reasoning_options 存在而 reasoning 缺失时诚实推断为支持推理`() {
        val body = """{"p":{"models":{"m/only-options":{
            "id":"m/only-options","tool_call":true,
            "reasoning_options":[{"type":"effort","values":["low","bogus_value"]}],"last_updated":"2026-01-01"}}}}"""
        CapabilityRefresh.parseAndWrite(body, dir, "2026-09-23T02:00:00Z")
        ModelCapabilityCatalog.reload(dir)
        val cap = assertNotNull(ModelCapabilityCatalog.lookup("m/only-options"))
        assertTrue(cap.reasoning)
        // 不认识的 bogus_value 被跳过，只留 low
        assertEquals(setOf("low"), cap.thinkingEfforts.keys)
    }

    @Test
    fun `刚落盘不算过期、没有文件算过期`() {
        assertTrue(CapabilityRefresh.isStale(dir), "没有 remote.json 应视为过期")
        CapabilityRefresh.parseAndWrite(fixture, dir, "2026-09-23T02:00:00Z")
        assertFalse(CapabilityRefresh.isStale(dir), "刚刷新不应视为过期")
        assertEquals(0L, CapabilityRefresh.remoteAgeHours(dir))
    }

    @Test
    fun `空的非法数据集会抛错而不是写坏旧文件`() {
        val count = CapabilityRefresh.parseAndWrite(fixture, dir, "2026-09-23T02:00:00Z")
        assertEquals(2, count)
        val before = Files.readString(dir.resolve(ModelCapabilityCatalog.REMOTE_FILE))
        assertFailsWith<IllegalStateException> {
            CapabilityRefresh.parseAndWrite("""{"provider":{}}""", dir, "2026-09-23T03:00:00Z")
        }
        assertEquals(before, Files.readString(dir.resolve(ModelCapabilityCatalog.REMOTE_FILE)), "失败时旧文件必须原样保留")
        assertTrue(ModelCapabilityCatalog.status().remote!!.fetchedAt == "2026-09-23T02:00:00Z")
    }
}
