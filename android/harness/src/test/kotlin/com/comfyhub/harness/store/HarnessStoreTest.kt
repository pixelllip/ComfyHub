package com.comfyhub.harness.store

import com.comfyhub.harness.event.RunEventType
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
 * 本地仓储层：会话 / 消息 / 块 / Run / 事件 / 上行队列。
 *
 * 用例盯的都是**会静默出错**的地方：
 *  - 业务行和 outbox 必须**同事务**（漏了 = 这次会话永远上不去，界面毫无异常）；
 *  - **R6**：失败 / 取消时正文与块都还在（不许抹空）；
 *  - 崩溃恢复要能**认出没跑完的 Run**（前台服务被杀是常态）；
 *  - 事件**批量**写，但闸门类事件当场落库（否则审批挂着的几分钟里另一端查不到）；
 *  - 事件裁剪只留尾部（R5），而 `seq` 不许回退。
 */
class HarnessStoreTest {

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("harness-store-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    // --- 会话 / 消息 / 块 ---------------------------------------------------

    @Test
    fun `会话与消息的 seq 在同一会话内单调递增`() {
        val c = store.createConversation()
        val m1 = store.appendMessage(c.id, "user", "第一条")
        val m2 = store.appendMessage(c.id, "assistant", "第二条")
        assertEquals(1, m1.seq)
        assertEquals(2, m2.seq)
        assertEquals(listOf(m1.id, m2.id), store.listMessages(c.id).map { it.id })

        // 另一个会话自己从 1 开始
        val other = store.createConversation()
        assertEquals(1, store.appendMessage(other.id, "user", "另起").seq)
    }

    @Test
    fun `有序块按 ordinal 读回来，且 ordinal 从 0 开始`() {
        val c = store.createConversation()
        val m = store.appendMessage(c.id, "assistant", "正文")
        assertEquals("还没有块时最大值是 -1（下一个就是 0）", -1, store.maxPartOrdinal(m.id))
        store.appendPart(m.id, 0, PartType.REASONING, text = "想一想")
        store.appendPart(m.id, 1, PartType.TEXT, text = "正文")
        store.appendPart(m.id, 2, PartType.TOOL_CALL, toolCallId = "call_1", jsonPayload = "{}")

        assertEquals(
            listOf(PartType.REASONING, PartType.TEXT, PartType.TOOL_CALL),
            store.listParts(m.id).map { it.type },
        )
        assertTrue(store.hasParts(m.id))
    }

    @Test
    fun `同一 ordinal 重复落块是覆盖而不是报错（幂等重放）`() {
        val c = store.createConversation()
        val m = store.appendMessage(c.id, "assistant", "正文")
        store.appendPart(m.id, 0, PartType.TEXT, text = "第一版")
        store.appendPart(m.id, 0, PartType.TEXT, text = "第二版")
        assertEquals(1, store.listParts(m.id).size)
        assertEquals("第二版", store.listParts(m.id).single().text)
    }

    // --- R6：失败 / 取消时一个字都不能丢 -----------------------------------

    @Test
    fun `R6：失败时正文与块都留着，不许抹成空串`() {
        val c = store.createConversation()
        val m = store.appendMessage(c.id, "assistant", status = MessageStatus.STREAMING)
        // 流出去的那一段已经写进正文、也落了块
        store.updateMessage(m.id, text = "我已经说了半句话", status = MessageStatus.STREAMING)
        store.appendPart(m.id, 0, PartType.TEXT, text = "我已经说了半句话")
        // 429 / 断流：收尾成 failed
        store.updateMessage(m.id, status = MessageStatus.FAILED, usageJson = null)

        val after = store.getMessage(m.id)!!
        assertEquals(MessageStatus.FAILED, after.status)
        assertEquals("已经说出去的话一个字都不能丢", "我已经说了半句话", after.text)
        assertEquals(1, store.listParts(m.id).size)
    }

    @Test
    fun `R6：标题摘取缓冲 flush 出来的那一段也能单独落块（跟正文同一条路）`() {
        val c = store.createConversation()
        val m = store.appendMessage(c.id, "assistant", status = MessageStatus.STREAMING)
        // 失败分支：把 flush 出来的内容补落成块（标题还没摘到时压着的那一段）
        store.updateMessage(m.id, text = "好的，我", status = MessageStatus.CANCELLED)
        store.appendPart(m.id, 0, PartType.TEXT, text = "好的，我")

        assertEquals("好的，我", store.getMessage(m.id)!!.text)
        assertEquals("好的，我", store.listParts(m.id).single().text)
    }

    // --- 崩溃恢复 -----------------------------------------------------------

    @Test
    fun `崩溃恢复：能认出没跑完的 Run，收尾后不再是未完成`() {
        val c = store.createConversation()
        val assistant = store.appendMessage(c.id, "assistant", status = MessageStatus.STREAMING)
        val run = store.createRun(
            conversationId = c.id,
            providerId = "p1",
            modelId = "m1",
            userMessageId = null,
            assistantMessageId = assistant.id,
            promptVersion = "v28",
        )
        assertEquals(listOf(run.id), store.listUnfinishedRuns().map { it.id })

        store.finishRun(run.id, RunStatus.FAILED, errorCode = "PROVIDER_UNREACHABLE", errorMessage = "断网")
        assertTrue(store.listUnfinishedRuns().isEmpty())
        val after = store.getRun(run.id)!!
        assertEquals(RunStatus.FAILED, after.status)
        assertEquals("PROVIDER_UNREACHABLE", after.errorCode)
        assertNotNull(after.completedAt)
        assertTrue(RunStatus.isTerminal(after.status))
    }

    @Test
    fun `崩溃恢复：桌面同步下来的 running 行不算'我的未完成 Run'`() {
        // `ai_runs` 自 2026-09-27 起是**镜像表**（总纲 §7 待定项 1 = 进同步范围）：
        // 桌面那次 Run 会整行落下来。它要是被当成"我没跑完的 Run"，手机上就会按 R6 把它
        // 标成 failed 再推回桌面 —— 这是**静默改坏别人数据**那一类。
        // 判据：只有 createRun（同一个事务里写了 local_runs）出来的 Run 才算"我的"。
        val c = store.createConversation()
        val mine = store.createRun(c.id, null, null, null, null, "v1")

        // 直接 SQL 插一条"桌面同步下来"的 running 行（镜像 apply 就是这个形状：整行覆盖，
        // **不**写 local_runs）
        db.session.exec(
            """
            INSERT INTO ai_runs (id, conversation_id, status, started_at, created_at, updated_at)
            VALUES ('desktop-run-1', ?, 'running', '2026-09-27T00:00:00.000Z',
                    '2026-09-27T00:00:00.000Z', '2026-09-27T00:00:00.000Z')
            """.trimIndent(),
            listOf(c.id),
        )

        val landed = db.session
            .queryOne("SELECT COUNT(*) AS n FROM ai_runs WHERE conversation_id = ?", listOf(c.id))
            ?.int("n")
        assertEquals("镜像行真的落进库里了（两行都在）", 2, landed)
        assertEquals(
            "桌面那条 running 不许被当成我的未完成 Run",
            listOf(mine.id),
            store.listUnfinishedRuns().map { it.id },
        )
    }

    // --- 事件流（R5 / P1 的批量写） ----------------------------------------

    @Test
    fun `事件批量写：add 之后不落库，flush 一次全落`() {
        val c = store.createConversation()
        val run = store.createRun(c.id, null, null, null, null, "v1")
        val batcher = store.EventBatcher(run.id)

        repeat(50) { i -> batcher.add(i + 1, RunEventType.TEXT_DELTA, "{\"text\":\"x\"}") }
        assertEquals("还没 flush 时库里不该有（这就是批量写的收益）", 0, store.maxEventSeq(run.id))
        assertEquals(50, batcher.flush())
        assertEquals(50, store.maxEventSeq(run.id))
        assertEquals("没有新事件时 flush 是空操作", 0, batcher.flush())
    }

    @Test
    fun `闸门类事件当场落库（另一端要能马上发现'有东西在等人'）`() {
        val c = store.createConversation()
        val run = store.createRun(c.id, null, null, null, null, "v1")
        val batcher = store.EventBatcher(run.id)

        batcher.add(1, RunEventType.TEXT_DELTA, "{}")
        assertEquals(0, store.maxEventSeq(run.id))
        batcher.add(2, RunEventType.TOOL_REQUESTED, "{\"callId\":\"c1\"}")
        assertEquals("审批/提问必须立即落库，不能等 200 条一批", 2, store.maxEventSeq(run.id))
        assertEquals(
            listOf(1 to RunEventType.TEXT_DELTA, 2 to RunEventType.TOOL_REQUESTED),
            store.listEvents(run.id),
        )
    }

    @Test
    fun `R5：裁剪只留尾部 keep 条，且 seq 不回退（重连只看 after）`() {
        val c = store.createConversation()
        val run = store.createRun(c.id, null, null, null, null, "v1")
        repeat(500) { i -> store.appendEvent(run.id, i + 1, RunEventType.TEXT_DELTA, "{}") }

        store.pruneEvents(run.id, keep = 100)
        val left = store.listEvents(run.id)
        assertEquals(100, left.size)
        assertEquals(401, left.first().first)
        assertEquals(500, left.last().first)
        assertEquals("seq 只增不减：裁剪不影响后续发号", 500, store.maxEventSeq(run.id))

        // 断线重连：只拿 after 之后的那一段
        assertEquals(listOf(499, 500), store.listEvents(run.id, after = 498).map { it.first })
    }

    // --- 上行队列（契约 §4 / §7.3） ---------------------------------------

    @Test
    fun `契约 7_3：业务行与 outbox 在同一个事务里，回滚时两边都不留`() {
        val c = store.createConversation(queue = true)
        assertEquals(1, store.pendingOutbox().size)
        assertEquals(OutboxKind.CONVERSATION, store.pendingOutbox().single().kind)

        // 手工造一次回滚：会话行与 outbox 行必须一起消失
        runCatching {
            db.session.transaction {
                store.enqueue(OutboxKind.CONVERSATION, "ghost", "{}")
                error("boom")
            }
        }
        assertEquals("回滚后不许留孤立的 outbox 行", 1, store.pendingOutbox().size)
        assertFalse(store.pendingOutbox().any { it.refId == "ghost" })
    }

    @Test
    fun `上行失败退避：tries 加一、记 last_error，成功后移除（不许静默丢弃）`() {
        val c = store.createConversation()
        val item = store.pendingOutbox().single()
        store.markOutboxFailed(item.id, "HTTP 500")
        val after = store.pendingOutbox().single()
        assertEquals(1, after.tries)
        assertEquals("HTTP 500", after.lastError)
        assertEquals(c.id, after.refId)

        store.removeOutbox(item.id)
        assertTrue(store.pendingOutbox().isEmpty())
    }

    @Test
    fun `手机起的会话先落本地：即使桌面离线，本地会话列表也看得到`() {
        val c = store.createConversation(title = "离线时起的会话")
        val m = store.appendMessage(c.id, "user", "手机先聊")
        store.appendMessage(c.id, "assistant", "手机自己答的")

        assertEquals(listOf("离线时起的会话"), store.listConversations().map { it.title })
        assertEquals("手机先聊", store.getMessage(m.id)!!.text)
        // 三样东西都排进了上行队列：会话 + 两条消息
        assertEquals(3, store.pendingOutbox().size)
    }

    // --- 零散状态 -----------------------------------------------------------

    @Test
    fun `mobile_kv 存游标与 kitVersion，重写是覆盖`() {
        assertNull(store.getKv(KvKey.SYNC_CURSOR))
        store.putKv(KvKey.SYNC_CURSOR, "918342")
        store.putKv(KvKey.KIT_VERSION, "v28")
        store.putKv(KvKey.SYNC_CURSOR, "918400")
        assertEquals("918400", store.getKv(KvKey.SYNC_CURSOR))
        assertEquals("v28", store.getKv(KvKey.KIT_VERSION))
    }

    // --- 时间格式（契约 §3.1） ---------------------------------------------

    @Test
    fun `时间列是 ISO-8601 UTC 毫秒三位，字符串序即时间序`() {
        val a = Timestamps.of(1_700_000_000_000L)
        val b = Timestamps.of(1_700_000_001_000L)
        assertEquals("2023-11-14T22:13:20.000Z", a)
        assertTrue(a < b)
        assertTrue(Regex("""^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$""").matches(Timestamps.now()))
    }

    // --- 本地建会话（M3.2 ② 的第一块砖） ------------------------------------

    /**
     * 桥上新增的 `createConversation` 走的就是这条路，钉住三件事：
     *
     * 1. **用调用方给的 id**（界面按同一个 id 认自己插的那条会话；另起 UUID 就对不上）；
     * 2. 建完立刻能在 `listConversations` 里查到（离线时界面靠它渲染列表）；
     * 3. **进 `_outbox`** —— 这是 M4 的前提：本地建的会话要能在桌面在线时上行，
     *    否则"手机产生的会话在桌面可见"永远不成立。
     */
    @Test
    fun `本地建会话：认调用方给的 id、能立刻查到、并进 outbox`() {
        val before = store.pendingOutbox().size
        val created = store.createConversation(
            id = "conv-ui-1",
            title = "手机上起的",
            providerId = "p1",
            modelId = "m1",
        )

        assertEquals("必须用调用方给的 id", "conv-ui-1", created.id)
        assertEquals("手机上起的", created.title)

        // 立刻查得到（离线渲染列表靠它）
        val listed = store.listConversations()
        assertTrue("建完要能在列表里查到：${listed.map { it.id }}", listed.any { it.id == "conv-ui-1" })
        assertEquals("m1", store.getConversation("conv-ui-1")!!.modelId)

        // 进 outbox：等桌面在线时由 /api/sync/push 上行（幂等靠 UUID 主键）
        val pending = store.pendingOutbox()
        assertTrue("本地建的会话必须排队等上行，否则桌面永远看不到", pending.size > before)
        assertTrue(
            "outbox 里要有一条指向这条会话：${pending.map { it.kind to it.refId }}",
            pending.any { it.kind == OutboxKind.CONVERSATION && it.refId == "conv-ui-1" },
        )

        // 同一个 id 建两次要报错（主键冲突），**不许静默覆盖** ——
        // 静默覆盖会把用户正在写的那条会话连标题一起改掉
        var threw = false
        try {
            store.createConversation(id = "conv-ui-1", title = "覆盖试试")
        } catch (e: Throwable) {
            threw = true
        }
        assertTrue("重复 id 建会话必须报错，不许静默覆盖", threw)
        assertEquals("标题不该被改掉", "手机上起的", store.getConversation("conv-ui-1")!!.title)
    }

    // --- 本地删会话（M3.2 ③ 的第二块砖） ------------------------------------

    /**
     * 桥上新增的 `deleteConversation` 就是这条路。以前界面清"空壳会话"打的是**远端**
     * `DELETE /api/ai/conversations/{id}`，而手机上会话的真源是本地库 ——
     * 那条请求够不着本地镜像，于是空壳越攒越多（M3.2 ③ 留下的坑）。
     *
     * 这里钉四件事：行没了、**级联**把消息/块带走、待推行一并清掉、返回"是否真删掉了一行"。
     */
    @Test
    fun `本地删会话：行连同消息与块一起没了，它的待推行也清掉`() {
        val c = store.createConversation(id = "conv-del-1", title = "空壳")
        val m = store.appendMessage(c.id, "user", "说了一句")
        store.appendPart(m.id, 0, PartType.TEXT, text = "说了一句")
        assertEquals("会话 + 消息 + 块 = 三条待推", 3, store.pendingOutbox().size)

        assertTrue("删的是本机这一行", store.deleteConversation(c.id))

        assertNull(store.getConversation(c.id))
        assertFalse("列表里不该还有它", store.listConversations().any { it.id == c.id })
        assertNull("消息跟着走（外键 ON DELETE CASCADE，R1）", store.getMessage(m.id))
        assertTrue("块也一样", store.listParts(m.id).isEmpty())
        assertTrue(
            "还没推上去的待推行要一起清掉：留着的话桌面会收到一条本机已经没有的东西" +
                "（或者白搭一次往返）——${store.pendingOutbox().map { it.kind to it.refId }}",
            store.pendingOutbox().isEmpty(),
        )
    }

    /**
     * id 不存在**不是错误**（重复清理很常见：界面切会话时每次都会顺手清一次），
     * 但也**不许**顺手把别的行删掉 —— 所以返回值必须能区分"真删掉了"与"本来就没有"。
     */
    @Test
    fun `删一个不存在的 id：回 false，不抛，也不连累别的会话`() {
        val keep = store.createConversation(id = "conv-keep")

        assertFalse("本来就没有 → 不是错误，但也别假装删掉了", store.deleteConversation("conv-never-existed"))
        assertFalse(store.deleteConversation(""))

        assertNotNull("别的会话一个都不能被顺手删掉", store.getConversation(keep.id))
        assertEquals("它的待推行也不许被清", 1, store.pendingOutbox().size)
    }

    /**
     * **报错要如实**：库真的出错时一律抛，绝不静默回 `false`。
     *
     * 静默回 `false` 的后果是"界面以为删掉了、空壳其实还在"，而这是最难查的一类 ——
     * 没有任何一端会报错。所以这里把库关掉再删一次，证明它**抛**。
     */
    @Test
    fun `库真的出错时删会话要抛，不许静默回 false`() {
        val c = store.createConversation(id = "conv-boom")
        db.close()

        var threw = false
        try {
            store.deleteConversation(c.id)
        } catch (e: Throwable) {
            threw = true
        }
        assertTrue("库坏了必须抛出去：静默回 false = 界面以为删掉了，空壳还在", threw)
    }
}
