package com.comfyhub.harness.tools

import com.comfyhub.harness.net.HttpReply
import com.comfyhub.harness.net.HttpTransport
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * `DesktopSkills`：桌面 skill 的只读客户端。
 *
 * 这里钉的是**与桌面端点的形状契约**（`GET /api/ai/skills` 与 `GET /api/ai/skills/{name}`）
 * 以及三态语义：**"没有"与"连不上"必须分开**（混成一句会让用户以为 skill 被删了）。
 */
class DesktopSkillsTest {

    /** 记录请求并按 URL 给答复的假传输层。 */
    private class Recorder(private val reply: (String) -> HttpReply) : HttpTransport {
        val urls = mutableListOf<String>()
        val seenHeaders = mutableListOf<Map<String, String>>()

        override fun get(url: String, headers: Map<String, String>): HttpReply {
            urls += url
            seenHeaders += headers
            return reply(url)
        }

        override fun postJson(url: String, body: String, headers: Map<String, String>): HttpReply =
            HttpReply(404, "{}")

        override fun postJsonStreaming(
            url: String,
            body: String,
            headers: Map<String, String>,
            onLine: (String) -> Unit,
        ): Int = 404
    }

    private fun client(recorder: HttpTransport, baseUrl: String = "http://127.0.0.1:4038") =
        DesktopSkills(recorder).also { it.baseUrl = baseUrl; it.token = "tok-1" }

    @Test
    fun `names：读出名字与描述，没有名字的条目跳过`() = runBlocking {
        val recorder = Recorder {
            HttpReply(
                200,
                """[
                  {"name":"r2i-prompt","description":"扩写","version":"3"},
                  {"description":"没有名字"},
                  {"name":"","description":""}
                ]""",
            )
        }
        val skills = client(recorder).names()

        assertEquals(listOf("r2i-prompt"), skills?.map { it.name })
        assertEquals("扩写", skills?.first()?.description)
        assertEquals("3", skills?.first()?.version)
        assertEquals("http://127.0.0.1:4038/api/ai/skills", recorder.urls.single())
        // 闸门打开时桌面只认这个头：少了就是 403
        assertEquals("tok-1", recorder.seenHeaders.single()["X-ComfyHub-Token"])
    }

    @Test
    fun `names：没连过桌面或桌面报错时返回 null（不是空表）`() = runBlocking {
        // 还没连过桌面
        assertNull(DesktopSkills(Recorder { HttpReply(200, "[]") }).names())
        // 桌面 500
        assertNull(client(Recorder { HttpReply(500, "boom") }).names())
        // 返回的不是数组
        assertNull(client(Recorder { HttpReply(200, "{\"oops\":1}") }).names())
    }

    @Test
    fun `body：200 给正文，400 与 404 是「没有」，5xx 与断网是「连不上」`() = runBlocking {
        val found = client(Recorder { HttpReply(200, """{"skill":{"name":"x"},"content":"正文"}""") })
            .body("x")
        assertTrue(found is RemoteSkillBody.Found)
        assertEquals("正文", (found as RemoteSkillBody.Found).body)

        assertEquals(RemoteSkillBody.Missing, client(Recorder { HttpReply(400, "{}") }).body("x"))
        assertEquals(RemoteSkillBody.Missing, client(Recorder { HttpReply(404, "{}") }).body("x"))
        assertEquals(RemoteSkillBody.Unreachable, client(Recorder { HttpReply(500, "{}") }).body("x"))
        // 网络层直接抛（连不上）也是 Unreachable —— 绝不能报成"没有"
        val broken = client(Recorder { throw java.io.IOException("no route") })
        assertEquals(RemoteSkillBody.Unreachable, broken.body("x"))
        // 还没连过桌面同理
        assertEquals(RemoteSkillBody.Unreachable, DesktopSkills(Recorder { HttpReply(200, "{}") }).body("x"))
    }

    @Test
    fun `body：正文为空也算「没有」（别把空壳当成取到了）`() = runBlocking {
        val empty = client(Recorder { HttpReply(200, """{"skill":{"name":"x"},"content":"   "}""") })
        assertEquals(RemoteSkillBody.Missing, empty.body("x"))
    }
}
