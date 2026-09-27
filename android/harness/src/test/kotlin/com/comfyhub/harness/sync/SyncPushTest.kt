package com.comfyhub.harness.sync

import com.comfyhub.harness.net.UrlConnectionTransport
import com.comfyhub.harness.store.HarnessDatabase
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.JdbcSqlSession
import com.comfyhub.harness.store.KvKey
import com.comfyhub.harness.store.MessageStatus
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList

/**
 * 上行通道（**M4**）：`_outbox` → `POST /api/sync/push` → `applied` / `rejected`。
 *
 * 走**真 HTTP**（一台绑在 `127.0.0.1` 临时端口的假桌面网关 + 生产用的 `UrlConnectionTransport`）
 * 与**真 SQLite**（JDBC 引擎、同一份 DDL）—— 这条链在手机上跑的就是它。
 *
 * 五条最容易静默出错的纪律，一条一条钉住：
 * ① payload 是**推送时现读的完整行**（不是入队时的半截快照）；
 * ② 只有 `applied` 才清 outbox；
 * ③ `rejected` 要按**永久 / 暂时**分开（永久丢掉、暂时留着重试）；
 * ④ 同一条本地行重复入队只推一次；
 * ⑤ 网络失败**一条都不许丢**。
 */
class SyncPushTest {

    /** 假桌面：记下收到的请求体，按脚本回一个 `/api/sync/push` 回执。 */
    private class FakeDesktop {
        private val server: HttpServer = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val bodies = CopyOnWriteArrayList<String>()
        val tokens = CopyOnWriteArrayList<String?>()

        /** 下一次回的 `(状态码, 正文)`。 */
        var next: Pair<Int, String> = 200 to """{"applied":[],"rejected":[]}"""

        val baseUrl: String get() = "http://127.0.0.1:${server.address.port}"

        fun start() {
            server.createContext("/api/sync/push") { exchange -> handle(exchange) }
            server.start()
        }

        fun stop() = runCatching { server.stop(0) }

        private fun handle(exchange: HttpExchange) {
            bodies += exchange.requestBody.readBytes().toString(StandardCharsets.UTF_8)
            tokens += exchange.requestHeaders.getFirst("X-ComfyHub-Token")
            val (status, text) = next
            val bytes = text.toByteArray(StandardCharsets.UTF_8)
            exchange.responseHeaders.add("Content-Type", "application/json; charset=utf-8")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }
    }

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore
    private lateinit var desktop: FakeDesktop
    private lateinit var client: SyncPushClient

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("sync-push-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
        desktop = FakeDesktop().also { it.start() }
        client = SyncPushClient(store, UrlConnectionTransport(connectTimeoutMs = 5_000, readTimeoutMs = 10_000))

        // 本地造一条"手机自己起的会话 + 一问一答 + 一个块"
        store.createConversation(
            id = "conv-1", title = "离线时起的会话",
            providerId = "p1", modelId = "m1", promptVersion = "v1",
        )
        store.appendMessage(
            conversationId = "conv-1", role = "user", text = "画一只猫",
            status = MessageStatus.COMPLETE, id = "msg-user",
        )
        store.appendMessage(
            conversationId = "conv-1", role = "assistant", text = "",
            status = MessageStatus.STREAMING, id = "msg-a",
        )
        store.appendPart(messageId = "msg-a", ordinal = 0, type = "text", text = "")
    }

    @After
    fun tearDown() {
        runCatching { desktop.stop() }
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    /** 解析推上去的那个 batch（方便逐条断言）。 */
    private fun batchOf(index: Int = 0): List<Map<String, kotlinx.serialization.json.JsonElement>> {
        val body = Json.parseToJsonElement(desktop.bodies[index]).jsonObject
        return body["batch"]!!.jsonArray.map { it.jsonObject }
    }

    @Test
    fun `完整行：kind 是线上名、payload 是推送时现读的完整行`() {
        // 把助手消息补完（模拟 Run 结束）—— payload 必须反映**这一刻**的值
        store.updateMessage("msg-a", text = "这是最终回复，带\"引号\"与换行\n第二行", status = MessageStatus.COMPLETE)
        store.appendPart(messageId = "msg-a", ordinal = 0, type = "text", text = "这是最终回复，带\"引号\"与换行\n第二行")

        desktop.next = 200 to """{"applied":["conv-1","msg-user","msg-a","msg-a#0"],"rejected":[]}"""
        val outcome = client.push(desktop.baseUrl, token = "tok-1")

        assertEquals("四条都推上去了", 4, outcome.applied)
        assertEquals(0, outcome.pending)
        assertEquals("tok-1", desktop.tokens.single())

        val body = Json.parseToJsonElement(desktop.bodies.single()).jsonObject
        assertTrue("要有 deviceId：$body", body["deviceId"]!!.jsonPrimitive.content.startsWith("phone-"))

        val kinds = batchOf().map { it["kind"]!!.jsonPrimitive.content }
        assertEquals(
            "kind 必须是**线上的**名字（单数、与桌面表名同形），不是本地 outbox 的 conversation/message/part",
            listOf("ai_conversation", "ai_message", "ai_message", "ai_message_part"),
            kinds,
        )
        assertTrue(batchOf().all { it["op"]!!.jsonPrimitive.content == "upsert" })

        // 会话那条：完整行（不止 id+title）
        val conv = batchOf()[0]["payload"]!!.jsonObject
        assertEquals("conv-1", conv["id"]!!.jsonPrimitive.content)
        assertEquals("离线时起的会话", conv["title"]!!.jsonPrimitive.content)
        assertEquals("p1", conv["provider_id"]!!.jsonPrimitive.content)
        assertEquals("v1", conv["system_prompt_version"]!!.jsonPrimitive.content)
        assertNotNull("created_at 要带上（否则桌面用「收到的时间」）", conv["created_at"])

        // ★ 助手消息：正文是**推送那一刻**的终值，不是入队时的空串
        val assistant = batchOf()[2]["payload"]!!.jsonObject
        assertEquals("msg-a", assistant["id"]!!.jsonPrimitive.content)
        assertEquals("complete", assistant["status"]!!.jsonPrimitive.content)
        assertEquals(
            "入队时 text 还是空的：payload 若用入队快照，桌面就会看到一条空回复",
            "这是最终回复，带\"引号\"与换行\n第二行",
            assistant["text"]!!.jsonPrimitive.content,
        )

        // 块那条：复合主键的两列都在
        val part = batchOf()[3]["payload"]!!.jsonObject
        assertEquals("msg-a", part["message_id"]!!.jsonPrimitive.content)
        assertEquals(0, part["ordinal"]!!.jsonPrimitive.content.toInt())
        assertEquals("text", part["type"]!!.jsonPrimitive.content)
        assertEquals("这是最终回复，带\"引号\"与换行\n第二行", part["text"]!!.jsonPrimitive.content)
    }

    @Test
    fun `只有 applied 才清 outbox —— 暂时失败留着重试、永久失败才丢`() {
        // 会话与助手消息成功、用户消息暂时失败（DB_ERROR）、块永久失败（SCHEMA_MISMATCH）
        desktop.next = 200 to """
            {"applied":["conv-1","msg-a"],
             "rejected":[{"refId":"msg-user","code":"DB_ERROR","message":"落库失败：锁住了"},
                         {"refId":"msg-a#0","code":"SCHEMA_MISMATCH","message":"没有列 json_payload"}]}
        """.trimIndent()
        val outcome = client.push(desktop.baseUrl, token = null)

        assertEquals(2, outcome.applied)
        assertEquals("永久失败丢掉", 1, outcome.rejectedPermanent)
        assertEquals("暂时失败留着", 1, outcome.rejectedTransient)

        val left = store.pendingOutbox()
        assertEquals("只剩暂时失败的那一条：${left.map { it.refId }}", 1, left.size)
        assertEquals("msg-user", left.single().refId)
        assertEquals("失败要记一次 tries（不许静默丢弃）", 1, left.single().tries)
        assertTrue(left.single().lastError!!.contains("DB_ERROR"))

        // 再推一次：这回成功了 → 队列清空
        desktop.next = 200 to """{"applied":["msg-user","msg-a"],"rejected":[]}"""
        val second = client.push(desktop.baseUrl, token = null)
        assertEquals(1, second.applied)
        assertEquals(0, store.pendingOutbox().size)
    }

    @Test
    fun `回执里既没 applied 也没 rejected 的那条当暂时失败，绝不当成功`() {
        desktop.next = 200 to """{"applied":[],"rejected":[]}"""
        val outcome = client.push(desktop.baseUrl, token = null)

        assertEquals(0, outcome.applied)
        assertEquals("桌面漏回的那几条一条都不许丢", 4, outcome.rejectedTransient)
        assertEquals(4, store.pendingOutbox().size)
    }

    @Test
    fun `重复入队只推一次（桌面靠主键幂等，但白推会让后面几条落到 STALE_IGNORED）`() {
        // 再改一次同一条消息与同一个块：outbox 里现在有 6 条，但只有 4 个不同实体
        store.updateMessage("msg-a", text = "第二版", status = MessageStatus.COMPLETE)
        store.appendPart(messageId = "msg-a", ordinal = 0, type = "text", text = "第二版")
        assertEquals(6, store.pendingOutbox().size)

        desktop.next = 200 to """{"applied":["conv-1","msg-user","msg-a","msg-a#0"],"rejected":[]}"""
        client.push(desktop.baseUrl, token = null)

        assertEquals("batch 里每个实体只出现一次", 4, batchOf().size)
        assertEquals("重复入队的那几条要在本地合成一条", 0, store.pendingOutbox().size)
        val texts = batchOf().filter { it["kind"]!!.jsonPrimitive.content == "ai_message" }
            .map { it["payload"]!!.jsonObject["text"]?.jsonPrimitive?.content }
        assertEquals(listOf("画一只猫", "第二版"), texts)
    }

    @Test
    fun `网络失败一条都不丢，全部记一次 tries`() {
        val dead = "http://127.0.0.1:1"
        val outcome = client.push(dead, token = null)

        assertEquals(0, outcome.applied)
        assertEquals(4, outcome.rejectedTransient)
        assertNotNull("要如实说出失败原因", outcome.error)
        val left = store.pendingOutbox()
        assertEquals(4, left.size)
        assertTrue("全记了一次失败", left.all { it.tries == 1 })
    }

    @Test
    fun `桌面回 500 也当暂时失败（不是把队列清空）`() {
        desktop.next = 500 to """{"error":"boom"}"""
        val outcome = client.push(desktop.baseUrl, token = null)

        assertEquals(0, outcome.applied)
        assertNotNull(outcome.error)
        assertEquals(4, store.pendingOutbox().size)
    }

    @Test
    fun `设备 id 只生成一次：生成两次桌面会当成两台设备`() {
        val first = client.deviceId()
        val second = client.deviceId()
        assertEquals(first, second)
        assertEquals(first, store.getKv(KvKey.DEVICE_ID))
        assertTrue("形状要能认出来是手机", first.startsWith("phone-"))
    }

    @Test
    fun `本地那行已经没了 —— 清掉那条 outbox，不推也不假装`() {
        // 直接把本地那一行删掉（外键级联会把块一起带走）
        db.session.update("DELETE FROM ai_messages WHERE id = ?", listOf("msg-a"))

        desktop.next = 200 to """{"applied":["conv-1","msg-user"],"rejected":[]}"""
        val outcome = client.push(desktop.baseUrl, token = null)

        assertEquals(2, outcome.applied)
        assertEquals("块那两条跟着消息一起没了", 2, outcome.droppedLocal)
        assertEquals(0, store.pendingOutbox().size)
        assertEquals("没有块那种 kind 被推上去", 2, batchOf().size)
    }

    @Test
    fun `永久与暂时的判据：只有 DB_ERROR 是暂时的`() {
        for (code in listOf(
            "UNKNOWN_KIND", "UNSUPPORTED_OP", "BAD_PAYLOAD", "UNKNOWN_TABLE",
            "SCHEMA_MISMATCH", "FORBIDDEN_KIND", "STALE_IGNORED",
        )) {
            assertTrue("$code 应当当永久失败丢掉", SyncPushClient.isPermanentReject(code))
        }
        assertFalse("DB_ERROR 是暂时的（重试有意义）", SyncPushClient.isPermanentReject("DB_ERROR"))
    }

    /**
     * **M4 的缺口回归**：手机自己那次 Run 与它的工具明细要能上行。
     *
     * 在此之前 `OutboxKind.RUN` 全仓一次都没被 enqueue 过（`ai_runs` 还是本机表），
     * 而桌面的 `sync.rs::table_of_kind` **早就收** `ai_run` / `ai_tool_call` ——
     * 表现是"手机跑过的 Run 与工具过程，桌面上永远看不到"，两端都不报错。
     *
     * 这条用例盯四件事：kind 是**线上名**、payload 是**推送那一刻现读的整行**、
     * 桌面 `applied` 之后 outbox **清干净**、以及**桌面同步下来的 Run 不在这批里**（不许回声）。
     */
    @Test
    fun `本机 Run 与工具明细上行：kind 是 ai_run ai_tool_call，推成功后 outbox 清干净`() {
        val run = store.createRun(
            conversationId = "conv-1", providerId = "p1", modelId = "m1",
            userMessageId = "msg-user", assistantMessageId = "msg-a", promptVersion = "v28",
        )
        val call = store.startToolCall(run.id, "call_1", "web_search", """{"query":"水墨风"}""")
        store.finishToolCall(
            id = call, status = "completed", content = "搜到两条", resultJson = """{"count":2}""",
            elapsedMs = 88,
        )
        store.finishRun(run.id, "completed")

        // 桌面同步下来的一行（**不在 local_runs 里**）：手工塞进 outbox，证明它不会被推出去
        db.session.exec(
            """
            INSERT INTO ai_runs (id, conversation_id, status, created_at, updated_at)
            VALUES ('desktop-run', 'conv-1', 'completed', '2026-09-27T00:00:00.000Z', '2026-09-27T00:00:00.000Z')
            """.trimIndent(),
        )
        store.enqueue("ai_run", "desktop-run", "{}")

        // setUp 里那四条（会话 / 两条消息 / 一个块）也在这批里：回执要把它们一起认掉
        desktop.next = 200 to
            """{"applied":["conv-1","msg-user","msg-a","msg-a#0","${run.id}","$call"],"rejected":[]}"""
        val outcome = client.push(desktop.baseUrl, token = null)

        val kinds = batchOf().map { it["kind"]!!.jsonPrimitive.content }
        assertTrue("本机 Run 要推上去：$kinds", kinds.contains("ai_run"))
        assertTrue("工具明细要推上去：$kinds", kinds.contains("ai_tool_call"))
        assertTrue(
            "桌面同步下来的那一条**不许**出现在 batch 里（回声）",
            batchOf().none { it["refId"]!!.jsonPrimitive.content == "desktop-run" },
        )

        // payload 是推送那一刻现读的完整行（不只是 refId）
        val toolPayload = batchOf().first { it["kind"]!!.jsonPrimitive.content == "ai_tool_call" }["payload"]!!.jsonObject
        assertEquals(call, toolPayload["id"]!!.jsonPrimitive.content)
        assertEquals(run.id, toolPayload["run_id"]!!.jsonPrimitive.content)
        assertEquals("completed", toolPayload["status"]!!.jsonPrimitive.content)
        assertEquals("web_search", toolPayload["name"]!!.jsonPrimitive.content)
        assertEquals(88, toolPayload["elapsed_ms"]!!.jsonPrimitive.content.toInt())
        val runPayload = batchOf().first { it["kind"]!!.jsonPrimitive.content == "ai_run" }["payload"]!!.jsonObject
        assertEquals(run.id, runPayload["id"]!!.jsonPrimitive.content)
        assertEquals("completed", runPayload["status"]!!.jsonPrimitive.content)

        assertEquals("六条（会话 / 两条消息 / 一个块 / Run / 工具明细）都推成功了", 6, outcome.applied)
        assertTrue(
            "推成功 + 那条本就不该推的 = 队列必须空：${store.pendingOutbox().map { it.kind to it.refId }}",
            store.pendingOutbox().isEmpty(),
        )
        assertTrue("不该推的那条算 droppedLocal（如实记，不假装推过）", outcome.droppedLocal >= 1)
    }
}
