package com.comfyhub.harness.sync

import com.comfyhub.harness.net.HttpReply
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessDatabase
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.JdbcSqlSession
import com.comfyhub.harness.store.MobileSchema
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
import java.util.concurrent.CopyOnWriteArrayList

/**
 * **下行通道**（M4 的另一半，契约 🔒 `docs/sync-api-contract.md` §1~§3）。
 *
 * 走**假 transport**（宿主 JVM，**一个真实请求都不发**）+ 真 SQLite（JDBC 引擎、同一份 DDL）。
 * 这里每一条都对着契约里的一句原话或一个已踩过的坑：
 *
 * | 用例 | 契约依据 |
 * | --- | --- |
 * | 游标落后 → 一轮增量就 `up_to_date` | §2 增量是主力 |
 * | `hasMore` 连两轮 | §2"`hasMore: true` 时必须继续拉，**不能只看第一页就更新游标**" |
 * | **610/410 → 丢弃镜像 + 全量** | §2/§5"游标落在窗口之外 → `410 Gone`"（唯一解药） |
 * | `schemaVersion` 不一致 → 全量 | §1"结构对不上，增量一定错" |
 * | 从没同步过（游标 0）→ 全量 | §3"首次全量（手机刚装上）" |
 * | 分页 bootstrap 两页 | §3`{table,page,pageSize,totalPages,rows}` |
 * | 网络失败 → **游标一个字不动** | §2（跳号 = 中间那些变更永远补不回来） |
 * | 游标不前进却 `hasMore` → 报错不动游标 | 防死循环（桌面上"同步转圈转到没电"） |
 */
class SyncPullTest {

    /** 假桌面：按 URL 回执，并把请求原样记下来（形状断言全靠它）。 */
    private class FakeDesktop(private val handler: (String) -> HttpReply) : HttpTransport {
        val requests = CopyOnWriteArrayList<String>()
        val tokens = CopyOnWriteArrayList<String?>()

        override fun get(url: String, headers: Map<String, String>): HttpReply {
            requests += url
            tokens += headers["X-ComfyHub-Token"]
            return handler(url)
        }

        override fun postJson(url: String, body: String, headers: Map<String, String>): HttpReply =
            throw AssertionError("下行通道不该发 POST：$url")

        override fun postJsonStreaming(
            url: String,
            body: String,
            headers: Map<String, String>,
            onLine: (String) -> Unit,
        ): Int = throw AssertionError("下行通道不该开流：$url")

        fun paths(): List<String> = requests.map { it.substringAfter("/api/sync/").substringBefore('?') }
    }

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("sync-pull-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    // --- 假桌面的零件 -------------------------------------------------------

    private fun manifest(cursor: Long, schema: Int = MobileSchema.VERSION) =
        """{"schemaVersion":$schema,"kitVersion":"2026-09-27.1","cursor":$cursor,
            "serverTime":"2026-09-27T05:30:00Z","tables":{}}"""

    private fun changes(cursor: Long, hasMore: Boolean = false, upserts: String = "{}", deletes: String = "{}") =
        """{"cursor":$cursor,"hasMore":$hasMore,"upserts":$upserts,"deletes":$deletes}"""

    private fun bootstrapPage(table: String, page: Int, rows: String = "", totalPages: Int = 1) =
        """{"table":"$table","page":$page,"pageSize":200,"totalPages":$totalPages,"rows":[$rows]}"""

    private fun conversationRow(id: String, title: String) =
        """{"id":"$id","title":"$title","system_prompt_version":"v28","archived":0,
            "created_at":"2026-09-27T00:00:00.000Z","updated_at":"2026-09-27T00:00:00.000Z"}"""

    private fun messageRow(id: String, conversationId: String) =
        """{"id":"$id","conversation_id":"$conversationId","seq":1,"role":"user","status":"complete",
            "created_at":"2026-09-27T00:00:00.000Z","updated_at":"2026-09-27T00:00:00.000Z"}"""

    private fun tableOf(url: String): String = url.substringAfter("table=").substringBefore('&')
    private fun pageOf(url: String): Int = url.substringAfter("page=").substringBefore('&').toInt()
    private fun sinceOf(url: String): Long = url.substringAfter("since=").substringBefore('&').toLong()

    private fun pull(
        desktop: FakeDesktop,
        token: String? = "tok-1",
    ): PullOutcome = SyncPullClient(store, desktop).pullOnce("http://fake.desktop:8081/", token)

    // --- ① 增量：一轮就完事 -------------------------------------------------

    @Test
    fun `游标落后一轮增量就 up_to_date，游标推进到响应里的 cursor`() {
        store.setSyncCursor(100)
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(918342))
                url.contains("/api/sync/changes") -> HttpReply(200, changes(918342))
                else -> error("不该请求 $url")
            }
        }

        val outcome = pull(desktop)

        assertTrue(outcome.ok)
        assertEquals(SyncPullClient.MODE_UP_TO_DATE, outcome.mode)
        assertEquals(918342L, outcome.cursor)
        assertEquals("整趟成功才写游标", 918342L, store.syncCursor())
        assertEquals("manifest → changes 两条（多余一条都不打）", listOf("manifest", "changes"), desktop.paths())
        assertEquals("配对令牌要带上（闸门开着时没有它必然 403）", listOf("tok-1", "tok-1"), desktop.tokens)
        assertTrue(
            "查询串形状照契约：since=<本机游标>&limit=500",
            desktop.requests.any { it.contains("/api/sync/changes?since=100&limit=500") },
        )
    }

    /** 契约 §2：`hasMore: true` 时必须继续拉 —— 只看第一页就更新游标 = 丢变更。 */
    @Test
    fun `hasMore 连两轮：第二轮接着上一轮的游标，最后落在第二轮的 cursor 上`() {
        store.setSyncCursor(10)
        var round = 0
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(60))
                url.contains("/api/sync/changes") -> {
                    round++
                    if (round == 1) {
                        HttpReply(
                            200,
                            changes(
                                cursor = 30,
                                hasMore = true,
                                upserts = """{"ai_conversations":[${conversationRow("c-1", "第一轮来的")}]}""",
                            ),
                        )
                    } else {
                        HttpReply(
                            200,
                            changes(
                                cursor = 60,
                                hasMore = false,
                                upserts = """{"ai_conversations":[${conversationRow("c-2", "第二轮来的")}]}""",
                            ),
                        )
                    }
                }
                else -> error("不该请求 $url")
            }
        }

        val outcome = pull(desktop)

        assertTrue(outcome.ok)
        assertEquals(SyncPullClient.MODE_CHANGES, outcome.mode)
        assertEquals(2, outcome.applied)
        assertEquals(60L, outcome.cursor)
        assertEquals("两轮都拉了，而且第二轮带的是第一轮给的游标", listOf(10L, 30L), desktop.requests
            .filter { it.contains("/changes?") }.map { sinceOf(it) })
        assertEquals("两轮的行都落库了", "第一轮来的", store.getConversation("c-1")!!.title)
        assertEquals("第二轮来的", store.getConversation("c-2")!!.title)
    }

    /** 游标不前进却还说 `hasMore` = 死循环的入口：**如实报错、不动游标**。 */
    @Test
    fun `游标不前进却还说 hasMore：如实报错且不推进游标`() {
        store.setSyncCursor(10)
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(60))
                // 每一轮都回同一个游标 + hasMore=true
                else -> HttpReply(200, changes(10, hasMore = true))
            }
        }

        val outcome = pull(desktop)

        assertFalse(outcome.ok)
        assertNotNull(outcome.error)
        assertTrue("要说清是游标没前进：${outcome.error}", outcome.error!!.contains("没有前进"))
        assertEquals("游标一个字都不许动", 10L, store.syncCursor())
    }

    // --- ② 410：丢弃镜像 + 全量 ---------------------------------------------

    /**
     * **本批最重要的一条**（契约 §2/§5）：游标落在保留窗口之外 → `410 Gone` →
     * 丢弃镜像 + 走全量，最后游标 = **manifest 的 cursor**（不是 bootstrap 的页号）。
     *
     * 同时钉住 [HarnessStore.resetMirror] 的那条纪律：**本机独有、还没推上去**的东西
     * 一行都不许丢（无脑清空 = 把用户离线时聊的东西删了，桌面补不回来）。
     */
    @Test
    fun `410 丢弃镜像并转全量，游标最终等于 manifest 的 cursor（本机独有的一行不丢）`() {
        store.setSyncCursor(5)
        // 桌面那份的投影（不该有 outbox）：410 之后该被丢掉、只留 bootstrap 给的
        store.createConversation(id = "stale-mirror", title = "旧镜像", queue = false)
        // 本机独有、**还没推上去**的（有 outbox）：一行都不许丢
        val mine = store.createConversation(id = "mine-unsent", title = "手机离线时聊的")
        assertTrue(store.pendingOutbox().any { it.refId == mine.id })

        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(777))
                url.contains("/api/sync/changes") -> HttpReply(
                    410,
                    """{"error":"游标 5 早于保留窗口（最老 seq = 400）：请丢弃镜像改走 /api/sync/bootstrap"}""",
                )
                url.contains("/api/sync/bootstrap") ->
                    if (tableOf(url) == "ai_conversations" && pageOf(url) == 1) {
                        HttpReply(200, bootstrapPage(tableOf(url), 1, conversationRow("c-after-410", "重建后的会话")))
                    } else {
                        HttpReply(200, bootstrapPage(tableOf(url), pageOf(url)))
                    }
                else -> error("不该请求 $url")
            }
        }

        val outcome = pull(desktop)

        assertTrue("410 之后那次全量要成功：${outcome.error}", outcome.ok)
        assertEquals("走的是全量那条路", SyncPullClient.MODE_BOOTSTRAP, outcome.mode)
        assertEquals("游标 = manifest 的 cursor（不是页号）", 777L, outcome.cursor)
        assertEquals(777L, store.syncCursor())
        assertTrue("丢弃的行数要如实带出来", outcome.mirrorDropped >= 1)
        assertNull("旧镜像被丢掉了（它没有待推行 → 桌面那份说了算）", store.getConversation("stale-mirror"))
        assertEquals("重建后的会话落下来了", "重建后的会话", store.getConversation("c-after-410")!!.title)
        assertNotNull("★ 本机独有、还没推上去的：一行都不许丢", store.getConversation("mine-unsent"))
        assertTrue("它的待推行也还在（下次照样能推上去）", store.pendingOutbox().any { it.refId == mine.id })
        assertEquals(
            "请求序列：manifest → changes(410) → 全量（按镜像表顺序逐表）",
            listOf("manifest", "changes") + List(MobileSchema.MIRROR_TABLES.size) { "bootstrap" },
            desktop.paths(),
        )
        assertTrue(
            "全量的查询串要带 table（契约 §3 的实现补充：table **必填**）",
            desktop.requests.last { it.contains("bootstrap") }.contains("table=ai_tool_calls"),
        )
    }

    /** 契约 §1：`schemaVersion` 与本机不一致 → 全量（结构对不上，增量一定错）。 */
    @Test
    fun `schemaVersion 不一致就走全量（不碰增量那条路）`() {
        store.setSyncCursor(500)
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(900, schema = MobileSchema.VERSION + 1))
                url.contains("/api/sync/bootstrap") ->
                    if (tableOf(url) == "ai_conversations") {
                        HttpReply(200, bootstrapPage(tableOf(url), pageOf(url), conversationRow("c-new-schema", "新结构来的")))
                    } else {
                        HttpReply(200, bootstrapPage(tableOf(url), pageOf(url)))
                    }
                else -> error("schema 不一致时不该请求 $url")
            }
        }

        val outcome = pull(desktop)

        assertTrue(outcome.ok)
        assertEquals(SyncPullClient.MODE_BOOTSTRAP, outcome.mode)
        assertEquals("游标置成 manifest 的 cursor", 900L, store.syncCursor())
        assertFalse("一条 changes 都不许打（结构对不上，增量一定错）", desktop.paths().contains("changes"))
        assertEquals("新结构的行落下来了", "新结构来的", store.getConversation("c-new-schema")!!.title)
    }

    /** 契约 §3："首次全量（手机刚装上）" —— 游标 0 = 从没同步过。 */
    @Test
    fun `从没同步过（游标 0）就走全量`() {
        assertEquals("还没同步过", 0L, store.syncCursor())
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(42))
                url.contains("/api/sync/bootstrap") -> HttpReply(200, bootstrapPage(tableOf(url), pageOf(url)))
                else -> error("第一次同步不该走增量：$url")
            }
        }

        val outcome = pull(desktop)

        assertTrue(outcome.ok)
        assertEquals(SyncPullClient.MODE_BOOTSTRAP, outcome.mode)
        assertEquals(42L, store.syncCursor())
        assertFalse(desktop.paths().contains("changes"))
    }

    /** 契约 §3：`{table,page,pageSize,totalPages,rows}` —— 一张表要**翻到底**。 */
    @Test
    fun `分页全量：一张表翻两页，两页的行都要落库`() {
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(50))
                url.contains("/api/sync/bootstrap") -> {
                    val table = tableOf(url)
                    if (table != "ai_conversations") {
                        HttpReply(200, bootstrapPage(table, pageOf(url)))
                    } else {
                        when (pageOf(url)) {
                            1 -> HttpReply(200, bootstrapPage(table, 1, conversationRow("p1", "第一页"), totalPages = 2))
                            2 -> HttpReply(200, bootstrapPage(table, 2, conversationRow("p2", "第二页"), totalPages = 2))
                            else -> error("翻过了头：$url")
                        }
                    }
                }
                else -> error("不该请求 $url")
            }
        }

        val outcome = pull(desktop, token = null)

        assertTrue(outcome.ok)
        assertEquals(2, outcome.applied)
        assertEquals("第一页", store.getConversation("p1")!!.title)
        assertEquals("第二页", store.getConversation("p2")!!.title)
        assertEquals(
            "每张同步表至少拉一页（空表也是 1 页）",
            MobileSchema.MIRROR_TABLES.size + 1, // +1 = ai_conversations 多翻的那一页
            desktop.paths().count { it == "bootstrap" },
        )
    }

    // --- ③ 失败：游标一个字都不许动 -----------------------------------------

    /**
     * **网络失败 / 回执读不懂时游标一个字都不许动**（契约 §2 的"不能跳号"）：
     * 一旦跳到"还没拉完"的位置，中间那些变更就**永远补不回来**了。
     */
    @Test
    fun `网络失败时游标不变（manifest 拿不到、changes 拿不到两种）`() {
        store.setSyncCursor(123)
        // ① 握手就失败
        val deadManifest = FakeDesktop { HttpReply(500, """{"error":"boom"}""") }
        val first = pull(deadManifest)
        assertFalse(first.ok)
        assertNotNull(first.error)
        assertEquals("握手失败：游标不许动", 123L, store.syncCursor())
        assertEquals(123L, first.cursor)

        // ② 增量拿到一半失败（第一轮成功、第二轮 500）：**已经落库的行不回滚**（幂等），
        //    但游标仍然是原来那个 —— 下次从头再拉一遍
        var round = 0
        val halfBroken = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(999))
                url.contains("/api/sync/changes") -> {
                    round++
                    if (round == 1) {
                        HttpReply(
                            200,
                            changes(
                                cursor = 500,
                                hasMore = true,
                                upserts = """{"ai_conversations":[${conversationRow("c-half", "半路来的")}]}""",
                            ),
                        )
                    } else {
                        HttpReply(500, """{"error":"boom"}""")
                    }
                }
                else -> error("不该请求 $url")
            }
        }
        val second = pull(halfBroken)
        assertFalse(second.ok)
        assertEquals("半路失败：游标还是原来那个（不许跳号）", 123L, store.syncCursor())
        assertEquals("已经落库的行不回滚（整行覆盖是幂等的，下次再拉一遍结果相同）", "半路来的", store.getConversation("c-half")!!.title)
    }

    /** 未知表（桌面比我们新）**不抛**，如实计入 `skippedUnknown`（不该让整趟作废）。 */
    @Test
    fun `增量里有不认识的表：如实计数、不抛、其余照常落库`() {
        store.setSyncCursor(1)
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(2))
                else -> HttpReply(
                    200,
                    changes(
                        cursor = 2,
                        upserts = """{"ai_conversations":[${conversationRow("c-known", "认识的")}],
                                      "ai_brand_new":[{"id":"x"}]}""",
                    ),
                )
            }
        }

        val outcome = pull(desktop)

        assertTrue(outcome.ok)
        assertEquals(1, outcome.applied)
        assertEquals("不认识的表那一行如实计入", 1, outcome.skippedUnknown)
        assertEquals("认识的照常落库", "认识的", store.getConversation("c-known")!!.title)
        assertEquals(2L, store.syncCursor())
    }

    /** 桌面地址都没有（还没配对）：**如实回错误**，一条请求都不打。 */
    @Test
    fun `不知道桌面地址时如实回错误，一条请求都不打`() {
        val desktop = FakeDesktop { error("不该请求 $it") }
        val outcome = SyncPullClient(store, desktop).pullOnce("   ", token = null)

        assertFalse(outcome.ok)
        assertTrue(outcome.error!!.contains("桌面地址"))
        assertTrue(desktop.requests.isEmpty())
        assertEquals(0L, store.syncCursor())
    }

    /** 一页里有一行不是 JSON 对象：**不许静默丢**，整趟如实失败（游标不动）。 */
    @Test
    fun `全量回执里有一行不是对象：如实失败、不静默丢行`() {
        store.setSyncCursor(0)
        val desktop = FakeDesktop { url ->
            when {
                url.endsWith("/api/sync/manifest") -> HttpReply(200, manifest(9))
                url.contains("bootstrap") && tableOf(url) == "ai_conversations" ->
                    HttpReply(200, bootstrapPage("ai_conversations", 1, """{"id":"c1"}, 42"""))
                url.contains("bootstrap") -> HttpReply(200, bootstrapPage(tableOf(url), pageOf(url)))
                else -> error("不该请求 $url")
            }
        }

        val outcome = pull(desktop)

        assertFalse(outcome.ok)
        assertTrue("要说清哪一行不对：${outcome.error}", outcome.error!!.contains("不是 JSON 对象"))
        assertEquals("游标不许动", 0L, store.syncCursor())
    }
}
