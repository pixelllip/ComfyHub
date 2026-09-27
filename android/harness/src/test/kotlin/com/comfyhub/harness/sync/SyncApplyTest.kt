package com.comfyhub.harness.sync

import com.comfyhub.harness.store.HarnessDatabase
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.JdbcSqlSession
import com.comfyhub.harness.store.MirrorApply
import com.comfyhub.harness.store.MobileSchema
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * **下行 apply**（契约 §2 / §3 的落库那一半）+ **本机删除标记的防线**。
 *
 * 这一条用例要钉住的是一个**已经写进文档的欠账**（总纲 §25.1 第三条）：
 * 手机上会话的真源是本地库，可用户主动删会话时上行**没有任何 delete op**（R3：只有 upsert），
 * 桌面那份不会知道 —— 于是下一次下行 apply 会把它整行覆盖回来、**当场复活**，
 * 而且两端都不报错。`local_deletions` 就是为这一件事存在的。
 *
 * 走**真 SQLite**（JDBC 引擎 + 同一份 DDL），JSON 用**与契约逐字同形的样例**。
 */
class SyncApplyTest {

    private lateinit var dir: Path
    private lateinit var db: HarnessDatabase
    private lateinit var store: HarnessStore

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("sync-apply-test")
        db = HarnessDatabase.from(JdbcSqlSession.file(dir.resolve("h.db").toString()))
        store = HarnessStore(db.session)
    }

    @After
    fun tearDown() {
        runCatching { db.close() }
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    private fun obj(json: String): JsonObject = Json.parseToJsonElement(json).jsonObject

    private fun conversationRow(id: String, title: String, updatedAt: String) = obj(
        """{"id":"$id","title":"$title","system_prompt_version":"v28","archived":0,
            "created_at":"2026-09-27T00:00:00.000Z","updated_at":"$updatedAt"}""",
    )

    private fun messageRow(id: String, conversationId: String, seq: Int) = obj(
        """{"id":"$id","conversation_id":"$conversationId","seq":$seq,"role":"user","status":"complete",
            "created_at":"2026-09-27T00:00:00.000Z","updated_at":"2026-09-27T00:00:00.000Z"}""",
    )

    private fun partRow(messageId: String, ordinal: Int, text: String) = obj(
        """{"message_id":"$messageId","ordinal":$ordinal,"type":"text","text":"$text"}""",
    )

    private fun runRow(id: String, conversationId: String) = obj(
        """{"id":"$id","conversation_id":"$conversationId","status":"completed",
            "started_at":"2026-09-27T00:00:00.000Z","created_at":"2026-09-27T00:00:00.000Z",
            "updated_at":"2026-09-27T00:00:00.000Z"}""",
    )

    private fun toolCallRow(id: String, runId: String) = obj(
        """{"id":"$id","run_id":"$runId","provider_call_id":"call_1","name":"web_search",
            "approval":"not_required","status":"completed","elapsed_ms":42,
            "created_at":"2026-09-27T00:00:00.000Z"}""",
    )

    // --- 正常路径 -----------------------------------------------------------

    @Test
    fun `整行覆盖：bootstrap 的一页按父表在前的顺序落库，重复 apply 不产生第二行`() {
        val apply = SyncApply(store)
        val first = apply.applyBootstrapPage(
            "ai_conversations",
            listOf(conversationRow("c1", "桌面起的会话", "2026-09-27T00:00:00.000Z")),
        )
        assertEquals(1, first.written)
        assertEquals("桌面起的会话", store.getConversation("c1")!!.title)

        // 父表在前：消息 → 块（乱序会当场撞外键，R1 的 foreign_keys 是开着的）
        apply.applyBootstrapPage("ai_messages", listOf(messageRow("m1", "c1", 1)))
        apply.applyBootstrapPage("ai_message_parts", listOf(partRow("m1", 0, "桌面说的第一句")))
        assertEquals("桌面说的第一句", store.listParts("m1").single().text)

        // 同一个 id 再来一次（`changes` 里重复出现是常态）：覆盖，不是第二行
        val again = SyncApply(store).applyChanges(
            upserts = mapOf("ai_conversations" to listOf(conversationRow("c1", "改过的标题", "2026-09-27T01:00:00.000Z"))),
        )
        assertEquals(1, again.written)
        assertEquals("改过的标题", store.getConversation("c1")!!.title)
        assertEquals(1L, db.session.queryOne("SELECT COUNT(*) AS c FROM ai_conversations")!!.long("c"))
    }

    /** `changes` 的 `deletes` 是**桌面说"这行没了"**：手机这边照删（否则本地镜像永远留着幽灵行）。 */
    @Test
    fun `changes 的 deletes 真的删掉本地镜像行`() {
        SyncApply(store).applyChanges(upserts = mapOf("ai_conversations" to listOf(conversationRow("c-del", "会被删", "2026-09-27T00:00:00.000Z"))))
        assertTrue(store.getConversation("c-del") != null)

        val outcome = SyncApply(store).applyChanges(deletes = mapOf("ai_conversations" to listOf(Json.parseToJsonElement("\"c-del\""))))
        assertEquals(1, outcome.deleted)
        assertNull(store.getConversation("c-del"))
    }

    // --- ★ 本机删除标记：不许复活 -------------------------------------------

    /**
     * **核心防线**：用户在这台手机上删掉的会话，下一次下行 apply **不许**把它弄回来。
     *
     * 判据 = `local_deletions`（[HarnessStore.isDeleted]），**不参与 LWW**：
     * 手机侧的删除在 wire 上没有任何表示，桌面那份的 `updated_at` 永远停在原地 ——
     * 拿时间戳去比只会得出"桌面更新 ⇒ 复活"这个恰恰要挡住的结论。
     */
    @Test
    fun `删掉的会话：再同步一次不复活，updated_at 更新的新变更也照样隐藏`() {
        SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(conversationRow("c-gone", "删掉的会话", "2026-09-27T00:00:00.000Z")),
                "ai_messages" to listOf(messageRow("m-gone", "c-gone", 1)),
            ),
        )
        assertTrue(store.deleteConversation("c-gone"))
        assertNull(store.getConversation("c-gone"))

        // ① 桌面那份**原样**再来一次（手机删它时没动桌面那份 ⇒ 这就是真实形状）
        val first = SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(conversationRow("c-gone", "删掉的会话", "2026-09-27T00:00:00.000Z")),
                "ai_messages" to listOf(messageRow("m-gone", "c-gone", 1)),
            ),
        )
        assertEquals("两条都按标记跳过", 2, first.skippedDeleted)
        assertEquals(0, first.written)
        assertNull("不许复活", store.getConversation("c-gone"))
        assertNull("它的消息也不许回来（父行不在，插进去必然撞外键）", store.getMessage("m-gone"))

        // ② 桌面那份在别处被改过（updated_at 更新）：**仍然隐藏** —— 标记是终局意图，
        //    不是一次可以靠时间戳分胜负的并发写
        val second = SyncApply(store).applyChanges(
            upserts = mapOf("ai_conversations" to listOf(conversationRow("c-gone", "桌面改过的标题", "2026-09-28T09:00:00.000Z"))),
        )
        assertEquals(1, second.skippedDeleted)
        assertNull("更新的 updated_at 也不能让它复活", store.getConversation("c-gone"))
    }

    /**
     * 子行也要跟着隐藏，而且**不许撞外键**：删会话在本地是级联的，
     * 桌面那份的块 / Run / 工具明细都还在 —— 只跳过会话行本身的话，
     * 这些行的 INSERT 会直接抛一条 SQL 异常，整页 apply 都得炸。
     */
    @Test
    fun `删掉的会话：它的块 Run 工具明细一并隐藏，不撞外键`() {
        val apply0 = SyncApply(store)
        apply0.applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(conversationRow("c2", "会话", "2026-09-27T00:00:00.000Z")),
                "ai_messages" to listOf(messageRow("m2", "c2", 1)),
                "ai_runs" to listOf(runRow("r2", "c2")),
                "ai_tool_calls" to listOf(toolCallRow("t2", "r2")),
                "ai_message_parts" to listOf(partRow("m2", 0, "块")),
            ),
        )
        assertEquals("块" , store.listParts("m2").single().text)

        assertTrue(store.deleteConversation("c2"))

        // 桌面那份整个再来一遍（父行在前 —— 与契约的 apply 顺序一致）
        val outcome = SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(conversationRow("c2", "会话", "2026-09-27T00:00:00.000Z")),
                "ai_messages" to listOf(messageRow("m2", "c2", 1)),
                "ai_runs" to listOf(runRow("r2", "c2")),
                "ai_tool_calls" to listOf(toolCallRow("t2", "r2")),
                "ai_message_parts" to listOf(partRow("m2", 0, "块")),
            ),
        )
        assertEquals("五行一条都不许落", 0, outcome.written)
        assertEquals(5, outcome.skippedDeleted)
        assertNull(store.getConversation("c2"))
        assertNull(store.getMessage("m2"))
        assertNull(store.getRun("r2"))
        assertNull(store.getToolCall("t2"))
        assertTrue(store.listParts("m2").isEmpty())
    }

    /**
     * 删掉的会话**后面新来的**消息与块也要一起隐藏。
     *
     * 这一条专打"只按自己的 id 标记跳过"的漏法：新消息的 id 没被标过（它是桌面在删除之后
     * 才创建的），而它指向的会话已经被删 —— 判据 ② 靠 `conversation_id` 认出来；
     * 它的**块**连 `conversation_id` 都没有，本机也查不到父行（消息行刚被跳过），
     * 只能靠**同批父行**把会话 id 带下来（`SyncApply` 的父行索引）。
     * 少了那条索引，这里会是一条外键异常，整页 apply 都得炸。
     */
    @Test
    fun `删掉的会话后面新来的消息与块也一起隐藏（父行靠同批索引认出来）`() {
        SyncApply(store).applyChanges(
            upserts = mapOf("ai_conversations" to listOf(conversationRow("c3", "会话", "2026-09-27T00:00:00.000Z"))),
        )
        assertTrue(store.deleteConversation("c3"))

        val outcome = SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_messages" to listOf(messageRow("m-new", "c3", 2)),
                "ai_message_parts" to listOf(partRow("m-new", 0, "新来的")),
            ),
        )
        assertEquals("一条都不许落", 0, outcome.written)
        assertEquals(2, outcome.skippedDeleted)
        assertNull(store.getMessage("m-new"))
        assertTrue(store.listParts("m-new").isEmpty())
    }

    /**
     * 反向验证：**没被标记的**会话照常落库。
     *
     * 少了这一条，一个"永远跳过所有行"的 bug 也会让上面两条用例变绿 ——
     * 那正是最坏的一种"测试全绿、功能全废"。
     */
    @Test
    fun `只有被标记的那些被跳过：别的会话照常落库`() {        SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(
                    conversationRow("c-keep", "留着的会话", "2026-09-27T00:00:00.000Z"),
                    conversationRow("c-drop", "删掉的会话", "2026-09-27T00:00:00.000Z"),
                ),
            ),
        )
        store.deleteConversation("c-drop")

        val outcome = SyncApply(store).applyChanges(
            upserts = mapOf(
                "ai_conversations" to listOf(
                    conversationRow("c-keep", "留着的会话", "2026-09-27T02:00:00.000Z"),
                    conversationRow("c-drop", "删掉的会话", "2026-09-27T02:00:00.000Z"),
                ),
                "ai_messages" to listOf(messageRow("m-keep", "c-keep", 1)),
            ),
        )
        assertEquals(2, outcome.written)
        assertEquals(1, outcome.skippedDeleted)
        assertEquals("留着的照常落", "留着的会话", store.getConversation("c-keep")!!.title)
        assertEquals("留着的会话的消息也照常落", 1, store.listMessages("c-keep").size)
    }

    /** 不认识的表 / 列：如实计数（**不抛、也不假装落库**）—— 桌面比我们新时不该整页作废。 */
    @Test
    fun `不认识的表如实计入 skippedUnknown`() {
        val outcome = SyncApply(store).applyChanges(
            upserts = mapOf("ai_something_new" to listOf(obj("""{"id":"x"}"""))),
            deletes = mapOf("ai_something_new" to listOf(Json.parseToJsonElement("\"x\""))),
        )
        assertEquals(0, outcome.written)
        assertEquals(2, outcome.skippedUnknown)
        assertEquals(MirrorApply.UNKNOWN_TABLE, SyncApply(store).applyRow("ai_something_new", obj("""{"id":"x"}""")))
    }

    /**
     * 桌面发来的行里出现了**本机没有的列** → **抛**，不许静默丢列。
     *
     * 静默丢的后果是"桌面加了一列、手机上永远没有那一列的值"，而且两端都不报错
     * （正常形态下走不到这里：`schemaVersion` 对不上时手机该先走 bootstrap）。
     */
    @Test
    fun `多出来的列要抛，不许静默丢数据`() {
        var threw = false
        try {
            SyncApply(store).applyRow(
                "ai_conversations",
                obj("""{"id":"c1","title":"t","brand_new_column":"x"}"""),
            )
        } catch (e: IllegalStateException) {
            threw = true
            assertTrue("要说清是哪一列：${e.message}", e.message!!.contains("brand_new_column"))
        }
        assertTrue("多出来的列必须抛", threw)
    }

    /** 复合主键的 `deletes` 形状（契约 §2：单列标量、复合是数组）。 */
    @Test
    fun `复合主键的 deletes 按数组解出两个键`() {
        db.session.exec("INSERT INTO tags(id, name, normalized) VALUES (1,'猫','猫')")
        db.session.exec("INSERT INTO prompts(id, positive_prompt) VALUES (1,'1girl')")
        SyncApply(store).applyChanges(upserts = mapOf("prompt_tags" to listOf(obj("""{"prompt_id":1,"tag_id":1}"""))))
        assertEquals(1L, db.session.queryOne("SELECT COUNT(*) AS c FROM prompt_tags")!!.long("c"))

        val deleted = SyncApply(store).applyChanges(
            deletes = mapOf("prompt_tags" to listOf(Json.parseToJsonElement("[1,1]") as JsonArray)),
        )
        assertEquals(1, deleted.deleted)
        assertEquals(0L, db.session.queryOne("SELECT COUNT(*) AS c FROM prompt_tags")!!.long("c"))
    }

    /** `local_deletions` **不在**镜像表清单里（不进同步范围、不挂触发器、没有触发器可挂）。 */
    @Test
    fun `local_deletions 不在镜像表清单里`() {
        assertFalse(MobileSchema.MIRROR_TABLES.any { it.name == "local_deletions" })
        assertTrue(MobileSchema.LOCAL_ONLY_TABLES.contains("local_deletions"))
        // 表真的建出来了（DDL 在 LOCAL_DDL 里）
        val columns = db.session.query("PRAGMA table_info(local_deletions)").map { it.str("name") }
        assertEquals(listOf("kind", "id", "deleted_at"), columns)
    }
}
