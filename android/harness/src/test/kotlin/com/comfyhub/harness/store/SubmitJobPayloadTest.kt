package com.comfyhub.harness.store

import com.comfyhub.harness.net.HttpReply
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.sync.SyncPushClient
import com.comfyhub.harness.tools.BuiltinTools
import com.comfyhub.harness.tools.MobileAskGate
import com.comfyhub.harness.tools.DefaultHarnessFiles
import com.comfyhub.harness.tools.MobileToolRegistry
import com.comfyhub.harness.tools.ToolCallRequest
import com.comfyhub.harness.tools.ToolContext
import com.comfyhub.harness.kit.RunKitCodec
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
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
 * **`submit_job` 的摊平**（契约 🔒 §4.1，M5）：手机排队 → 上行 → 桌面受理。
 *
 * 这条链上最容易踩、也最贵的一脚是**形状**：手机的工具拿到的是自由参数
 * （`{"workflow": …, "prompt": …}` 那类），而桌面只认**扁平的顶层键**。整包塞在一个
 * `args` 子对象里推上去，桌面按**永久性** `MISSING_WORKFLOW` 拒（`submit_queue::accept`），
 * 手机再把永久失败那行 outbox 丢掉 —— 用户看到的是"点了提交，**什么都没发生**"。
 *
 * 所以这里逐条钉：摊平规则、摊不平时**不入队**（当场告诉模型）、老形状在推送那一刻的补救、
 * 以及"推上去的 batch 里 kind 是 `submit_job`、payload 是扁平的那几个键"。
 */
class SubmitJobPayloadTest {

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("submit-job-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    private fun args(json: String): JsonObject = Json.parseToJsonElement(json).jsonObject

    private fun flatten(json: String): SubmitJobPayload.Flattened = SubmitJobPayload.flatten(
        args = args(json),
        queueId = "q-1",
        conversationId = "conv-1",
        runId = "run-1",
        requestedAt = "2026-09-27T00:00:00.000Z",
    )

    private fun ok(json: String): JsonObject {
        val f = flatten(json)
        assertTrue("本该摊平成功：$f", f is SubmitJobPayload.Flattened.Ok)
        return (f as SubmitJobPayload.Flattened.Ok).payload
    }

    private fun bad(json: String): String {
        val f = flatten(json)
        assertTrue("本该摊不平：$f", f is SubmitJobPayload.Flattened.Bad)
        return (f as SubmitJobPayload.Flattened.Bad).reason
    }

    // --- 摊平规则（契约 §4.1 的顶层键） --------------------------------------

    @Test
    fun `promptId 形状：顶层键与契约逐字一致`() {
        val payload = ok("""{"promptId":310,"nsfw":true,"overrides":{"6.text":"赛博朋克"}}""")
        assertEquals(
            setOf("queueId", "conversationId", "runId", "promptId", "overrides", "nsfw", "requestedAt"),
            payload.keys,
        )
        assertEquals("q-1", payload["queueId"]!!.jsonPrimitive.content)
        assertEquals("conv-1", payload["conversationId"]!!.jsonPrimitive.content)
        assertEquals("run-1", payload["runId"]!!.jsonPrimitive.content)
        assertEquals(310L, payload["promptId"]!!.jsonPrimitive.content.toLong())
        assertEquals("赛博朋克", payload["overrides"]!!.jsonObject["6.text"]!!.jsonPrimitive.content)
        assertTrue(payload["nsfw"]!!.jsonPrimitive.content.toBoolean())
        assertEquals("2026-09-27T00:00:00.000Z", payload["requestedAt"]!!.jsonPrimitive.content)
    }

    /** `sourceRef`（运行键）直接过；`workflow` 那一格给了运行键也认。 */
    @Test
    fun `sourceRef 与 workflow 里的运行键都认`() {
        val direct = ok("""{"sourceRef":"file:abc123"}""")
        assertEquals("file:abc123", direct["sourceRef"]!!.jsonPrimitive.content)
        assertFalse("没给 promptId 就不该有这个键", direct.containsKey("promptId"))

        val viaWorkflow = ok("""{"workflow":"file:abc123"}""")
        assertEquals("file:abc123", viaWorkflow["sourceRef"]!!.jsonPrimitive.content)
    }

    /** `workflow` 里给的是**数字 id** 时当 `promptId`（模型很自然会那么写）。 */
    @Test
    fun `workflow 给数字时当 promptId`() {
        assertEquals(310L, ok("""{"workflow":"310"}""")["promptId"]!!.jsonPrimitive.content.toLong())
        assertEquals(310L, ok("""{"workflowId":310}""")["promptId"]!!.jsonPrimitive.content.toLong())
    }

    /** 没给 `nsfw` 也**要写出来**（契约 §4.1 有这个键；缺省 = false，与桌面 `accept` 同义）。 */
    @Test
    fun `nsfw 缺省也写 false`() {
        assertEquals("false", ok("""{"promptId":1}""")["nsfw"]!!.jsonPrimitive.content)
    }

    /** 空 `overrides` / 空 `runId` 都**省掉键**（写上去是噪音，桌面两件事都不看它们）。 */
    @Test
    fun `空的 overrides 与 runId 省掉键`() {
        val payload = SubmitJobPayload.flatten(
            args = args("""{"promptId":1,"overrides":{}}"""),
            queueId = "q",
            conversationId = "c",
            runId = null,
            requestedAt = "t",
        ).let { (it as SubmitJobPayload.Flattened.Ok).payload }
        assertFalse(payload.containsKey("overrides"))
        assertFalse(payload.containsKey("runId"))
    }

    // --- 摊不平的三种：都要**当场说清** --------------------------------------

    /** 只给"工作流名字"→ 摊不平（手机没有那本库，名字解析不出 id）。 */
    @Test
    fun `只给工作流名字摊不平，且说清该给什么`() {
        val reason = bad("""{"workflow":"krea2_t2i"}""")
        assertTrue(reason, reason.contains("promptId"))
        assertTrue(reason, reason.contains("sourceRef"))
        assertTrue("要把收到的东西说出来：$reason", reason.contains("krea2_t2i"))
        assertTrue("要说清为什么不行（手机没有那本库）", reason.contains("桌面产生"))
    }

    /**
     * 自由文本 `prompt` → 摊不平，**绝不静默丢掉用户那句话**。
     *
     * overrides 的键是 `节点id.输入名`，而手机看不到节点图 —— 猜一个键名就是编造。
     */
    @Test
    fun `自由文本 prompt 摊不平，并告诉模型用 overrides`() {
        val reason = bad("""{"promptId":1,"prompt":"赛博朋克少女"}""")
        assertTrue(reason, reason.contains("overrides"))
        assertTrue("要把原文带上（用户那句话不能被吞）：$reason", reason.contains("赛博朋克少女"))
    }

    @Test
    fun `overrides 不是对象摊不平`() {
        assertTrue(bad("""{"promptId":1,"overrides":[]}""").contains("overrides"))
        assertTrue(bad("""{"sourceRef":"nope"}""").contains("sourceRef"))
    }

    // --- 推送那一刻（含老形状补救） ------------------------------------------

    @Test
    fun `forPush：新形状补 queueId 与 requestedBy，其余原样`() {
        val stored = ok("""{"promptId":310,"nsfw":false}""").toString()
        val wire = SubmitJobPayload.forPush(stored, refId = "q-1", requestedBy = "phone-abc")!!
        assertEquals("q-1", wire["queueId"]!!.jsonPrimitive.content)
        assertEquals("phone-abc", wire["requestedBy"]!!.jsonPrimitive.content)
        assertEquals(310L, wire["promptId"]!!.jsonPrimitive.content.toLong())
        assertFalse("`requestedBy` 只写一次", wire.keys.count { it == "requestedBy" } > 1)
    }

    /** ⚠ 老形状（`{runId, conversationId, args, queuedAt}`）在推送时补救摊平。 */
    @Test
    fun `forPush：老形状用里面的 args 补摊一次`() {
        val legacy = """
            {"runId":"run-9","conversationId":"conv-9","queuedAt":"2026-09-26T10:00:00.000Z",
             "args":{"workflow":"file:deadbeef","overrides":{"3.seed":123}}}
        """.trimIndent()
        val wire = SubmitJobPayload.forPush(legacy, refId = "q-legacy", requestedBy = null)!!
        assertEquals("q-legacy", wire["queueId"]!!.jsonPrimitive.content)
        assertEquals("conv-9", wire["conversationId"]!!.jsonPrimitive.content)
        assertEquals("run-9", wire["runId"]!!.jsonPrimitive.content)
        assertEquals("file:deadbeef", wire["sourceRef"]!!.jsonPrimitive.content)
        assertEquals(123, wire["overrides"]!!.jsonObject["3.seed"]!!.jsonPrimitive.content.toInt())
        assertEquals("排队那一刻要带过来", "2026-09-26T10:00:00.000Z", wire["requestedAt"]!!.jsonPrimitive.content)
        assertFalse("老包装不许原样推上去（桌面只认扁平键）", wire.containsKey("args"))
        assertFalse(wire.containsKey("queuedAt"))
    }

    /** 老形状也摊不平（比如只有工作流名字）→ 回 null（那行 outbox 不该推）。 */
    @Test
    fun `forPush：摊不平回 null`() {
        assertNull(SubmitJobPayload.forPush("""{"conversationId":"c","args":{"workflow":"名字"}}""", "q", null))
        assertNull(SubmitJobPayload.forPush("""{"promptId":1}""", "q", null)) // 缺 conversationId
        assertNull(SubmitJobPayload.forPush("这不是 JSON", "q", null))
        assertNull(SubmitJobPayload.forPush(null, "q", null))
    }

    // --- 端到端：工具入队 → outboxTarget → push 的 batch ----------------------

    private class FakeDesktop : HttpTransport {
        val bodies = mutableListOf<String>()
        var next: Pair<Int, String> = 200 to """{"applied":[],"rejected":[]}"""

        override fun get(url: String, headers: Map<String, String>): HttpReply = HttpReply(404, "no route")

        override fun postJson(url: String, body: String, headers: Map<String, String>): HttpReply {
            bodies += body
            return HttpReply(next.first, next.second)
        }

        override fun postJsonStreaming(
            url: String,
            body: String,
            headers: Map<String, String>,
            onLine: (String) -> Unit,
        ): Int = 404
    }

    private fun toolContext(): ToolContext = ToolContext(
        runId = "run-1",
        conversationId = "conv-1",
        kit = RunKitCodec.parse(
            """
            {"kitVersion":"2026-09-27.1","schemaVersion":4,"systemPrompt":"系统提示","promptVersion":"v30",
             "nsfwEnabled":false,"tools":[],"untrustedBanner":"‹{kind}›","titleInstruction":"t","models":[],
             "inlineBudget":{"perImageBytes":1,"totalBytes":1}}
            """.trimIndent(),
        ),
        store = store,
        transport = FakeDesktop(),
        files = DefaultHarnessFiles(dir.toFile()),
        askGate = MobileAskGate(),
        emit = { _, _ -> },
    )

    /**
     * **工具 → `_outbox` → `outboxTarget` → `/api/sync/push` 的 batch**：整条链走一遍。
     *
     * `kind` 必须是 `submit_job`（桌面 `table_of_kind` 里**没有**它，`push` 单独一个分支受理），
     * 而 payload 必须是**扁平的顶层键**。
     */
    @Test
    fun `端到端：工具排队后 payload 已是扁平形状，推上去的 batch 里 kind 是 submit_job`() = runBlocking {
        val registry = MobileToolRegistry(BuiltinTools.all())
        val outcome = registry.invoke(
            ToolCallRequest("call_1", "comfy_queue_submit", """{"promptId":310,"overrides":{"6.text":"水墨"},"nsfw":true}"""),
            toolContext(),
        )
        assertTrue(outcome.ok)
        assertFalse("R11：永远不许给 mediaIds", outcome.content.contains("mediaIds"))

        val queued = store.pendingOutbox().single()
        assertEquals(OutboxKind.SUBMIT_JOB, queued.kind)
        val stored = Json.parseToJsonElement(queued.payload).jsonObject
        assertEquals("入队那一刻就已经是扁平形状", 310L, stored["promptId"]!!.jsonPrimitive.content.toLong())
        assertFalse("不许再有 args 包装", stored.containsKey("args"))

        // 推上去
        val target = store.outboxTarget(OutboxKind.SUBMIT_JOB, queued.refId)!!
        assertEquals("submit_job", target.wireKind)
        assertEquals(queued.refId, target.payload["queueId"])
        assertEquals(310L, target.payload["promptId"])
        assertFalse("扁平：payload 里不许有 args", target.payload.containsKey("args"))

        val desktop = FakeDesktop().also {
            it.next = 200 to """{"applied":["${queued.refId}"],"rejected":[]}"""
        }
        val push = SyncPushClient(store, desktop).push("http://127.0.0.1:1", token = null)

        val batch = Json.parseToJsonElement(desktop.bodies.single()).jsonObject["batch"]!!.jsonArray
        assertEquals(1, batch.size)
        val item = batch.single().jsonObject
        assertEquals("submit_job", item["kind"]!!.jsonPrimitive.content)
        assertEquals(queued.refId, item["refId"]!!.jsonPrimitive.content)
        assertEquals("upsert", item["op"]!!.jsonPrimitive.content)
        val payload = item["payload"]!!.jsonObject
        assertEquals(queued.refId, payload["queueId"]!!.jsonPrimitive.content)
        assertEquals("conv-1", payload["conversationId"]!!.jsonPrimitive.content)
        assertEquals("run-1", payload["runId"]!!.jsonPrimitive.content)
        assertEquals(310L, payload["promptId"]!!.jsonPrimitive.content.toLong())
        assertTrue(payload["nsfw"]!!.jsonPrimitive.content.toBoolean())
        assertNotNull("requestedAt 要有", payload["requestedAt"])
        assertEquals("推成功就清队列", 1, push.applied)
        assertTrue(store.pendingOutbox().isEmpty())
    }

    /** 摊不平的意图**根本不排队**（用户不会看到"点了提交、什么都没发生"）。 */
    @Test
    fun `摊不平的意图不入队，当场把原因回给模型`() = runBlocking {
        val registry = MobileToolRegistry(BuiltinTools.all())
        val out = registry.invoke(
            ToolCallRequest("call_1", "comfy_queue_submit", """{"workflow":"krea2_t2i","prompt":"赛博朋克"}"""),
            toolContext(),
        )
        assertFalse(out.ok)
        assertEquals("INVALID_ARGUMENT", out.errorCode)
        assertTrue(out.content.contains("promptId"))
        assertTrue("队列里不许留摊不平的意图", store.pendingOutbox().isEmpty())
    }
}
