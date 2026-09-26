package com.comfyhub

import com.comfyhub.ai.AiConversationCreate
import com.comfyhub.ai.AiConversationRepo
import com.comfyhub.ai.AiMessageAppend
import com.comfyhub.ai.AiMessagePartDto
import com.comfyhub.ai.RunKit
import org.junit.jupiter.api.AfterAll
import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import java.sql.SQLException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/**
 * P2 地基的防线（总纲 §2 的 R1~R4 + §3.1 的类型约定）。
 *
 * 这些用例**必须连真库**：`PRAGMA foreign_keys` 是按连接的、`_sync_log` 的号是
 * rowid 给的、`BEGIN IMMEDIATE` 的表现只有真撞锁才看得见 —— 任何 mock 都验不出来，
 * 而它们失效的方式全是**静默**的（外键不报错、游标跳号不报错）。
 *
 * 库用**临时文件**（不是 `:memory:`）：内存库每个连接一个独立空库，池子、并发写、
 * `-wal` 这些真实行为都验不到。
 */
class SqliteFoundationTest {

    // -----------------------------------------------------------------------
    //  R1：每条连接都必须 foreign_keys=ON，级联/置空才真的生效
    // -----------------------------------------------------------------------

    @Test
    fun `R1 连接自检：foreign_keys 打开、WAL、busy_timeout、user_version 对齐`() {
        TestDb.reset()
        val p = Db.verify() ?: error("SQLite 方言下 verify() 不该返回 null")
        assertEquals(1, p.foreignKeys, "PRAGMA foreign_keys 必须是 ON（漏了外键静默失效）")
        assertEquals("wal", p.journalMode.lowercase(), "必须是 WAL：rollback journal 下读写互斥")
        assertEquals(5000, p.busyTimeoutMs)
        assertEquals(RunKit.SCHEMA_VERSION, p.userVersion, "user_version 必须与 run-kit 的 schemaVersion 同数")
        assertTrue(p.fts5, "xerial 的 bundled SQLite 应当带 FTS5")
    }

    @Test
    fun `R1 删标签会把 prompt_tags 的关联行一起删掉（ON DELETE CASCADE 真生效）`() {
        TestDb.reset()
        val promptId = Db.tx { conn ->
            PromptRepo.create(
                conn,
                PromptInput(title = "t", positivePrompt = "p", tags = listOf("风格A")),
            )
        }
        val tagId = Db.withConnection { conn ->
            TagRepo.tagsForPrompts(conn, listOf(promptId))[promptId]?.first()?.id
        } ?: error("建完应当有关联标签")

        assertEquals(1, countRows("prompt_tags"), "建完应当有一条关联")

        Db.tx { conn -> TagRepo.delete(conn, tagId) }

        assertEquals(0, countRows("prompt_tags"), "删标签后关联行必须跟着没 —— 这就是 R1 的防线")
        assertEquals(1, countRows("prompts"), "提示词本身不许被连带删掉")
    }

    @Test
    fun `R1 删提示词保产物（ON DELETE SET NULL）`() {
        TestDb.reset()
        val promptId = Db.tx { conn ->
            PromptRepo.create(conn, PromptInput(title = "t", positivePrompt = "p"))
        }
        val mediaId = Db.tx { conn ->
            MediaRepo.insert(
                conn = conn, kind = "IMAGE", title = "m", originalName = "a.png",
                storedName = "a.png", mimeType = "image/png", sizeBytes = 1,
                width = 1, height = 1, durationMs = null, sha256 = "x",
                source = "test", promptId = promptId, notes = null,
                sourceRef = null, workflowJson = null, nsfw = false,
            )
        }
        Db.tx { conn -> PromptRepo.delete(conn, promptId) }

        val freed = Db.withConnection { conn ->
            conn.queryOne("SELECT prompt_id FROM media_assets WHERE id = ?", mediaId) { it.longOrNull("prompt_id") }
        }
        assertEquals(null, freed, "删提示词必须把产物的 prompt_id 置空，而不是把产物删掉")
    }

    // -----------------------------------------------------------------------
    //  R2：写事务 BEGIN IMMEDIATE
    // -----------------------------------------------------------------------

    @Test
    fun `R2 写事务是 BEGIN IMMEDIATE：别人占着写锁时当场报忙，而不是先拿读锁再升级`() {
        TestDb.reset()
        val holder = DriverManager.getConnection(TestDb.jdbcUrl)
        holder.autoCommit = false
        holder.createStatement().use { it.execute("INSERT INTO tags (name, normalized) VALUES ('holder','holder')") }

        try {
            // 与 [Db.init] 拼出来的一模一样的那串 PRAGMA，只把 busy_timeout 缩短，
            // 免得这条用例为了等锁卡 5 秒。
            // ⚠ 参数要用 `?` 起头：用 `&` 拼的话它会落进**文件名**那一段，于是这里开的是
            // 另一个库文件（症状是"锁根本没生效"，本用例第一版就踩了这个坑）。
            val other = DriverManager.getConnection(
                Db.sqliteUrlWithPragmas("${TestDb.jdbcUrl}?busy_timeout=200")
            )
            try {
                // IMMEDIATE：`setAutoCommit(false)` 这一步就要去抢写锁 → 立刻 SQLITE_BUSY。
                // 裸 BEGIN（deferred）在这一步**不会**报错，只会在真正写的时候才失败 ——
                // 那种"读锁升级写锁"可能不理 busy_timeout，直接抛错（契约 §1 的 R2）。
                val err = assertFailsWith<SQLException> { other.autoCommit = false }
                assertTrue(
                    err.message.orEmpty().contains("busy", ignoreCase = true),
                    "应当是 SQLITE_BUSY，实际：${err.message}",
                )
            } finally {
                other.close()
            }
        } finally {
            holder.rollback()
            holder.close()
        }
    }

    @Test
    fun `R2 连接工厂拼出来的 PRAGMA 一条都不少，且不覆盖调用方显式给的`() {
        val url = Db.sqliteUrlWithPragmas("jdbc:sqlite:D:/x/comfyhub.db")
        listOf("journal_mode=WAL", "synchronous=NORMAL", "busy_timeout=5000", "foreign_keys=on", "transaction_mode=IMMEDIATE")
            .forEach { assertTrue(url.contains(it), "URL 少了 $it：$url") }

        val custom = Db.sqliteUrlWithPragmas("jdbc:sqlite:D:/x/comfyhub.db?busy_timeout=200")
        assertTrue(custom.contains("busy_timeout=200"), "显式给的 busy_timeout 不许被覆盖")
        assertEquals(1, Regex("busy_timeout").findAll(custom).count(), "同一个 PRAGMA 不许重复拼：$custom")
    }

    // -----------------------------------------------------------------------
    //  R3：upsert 不产生假 delete（不许 INSERT OR REPLACE）
    // -----------------------------------------------------------------------

    @Test
    fun `R3 同一个 upsert 走两次：_sync_log 里只有 upsert，没有假的 delete`() {
        TestDb.reset()
        Db.withConnection { conn ->
            conn.execute(
                "INSERT INTO app_settings (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                "x", "1",
            )
            conn.execute(
                "INSERT INTO app_settings (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                "x", "2",
            )
        }
        assertEquals(0, syncLogCount(op = "delete"), "INSERT OR REPLACE 会点亮 DELETE 触发器写假 delete（R3）")
        val value = SettingsRepo.get("x")
        assertEquals("2", value)
    }

    @Test
    fun `R3 有序块 upsert（appendPart 覆盖同 ordinal）不产生 delete 条目`() {
        TestDb.reset()
        val convId = AiConversationRepo.create(AiConversationCreate(title = "c")).id
        val msgId = AiConversationRepo.appendMessage(
            convId, AiMessageAppend(role = "assistant", text = ""),
        ).id
        AiConversationRepo.appendPart(msgId, 0, AiMessagePartDto(type = "text", text = "第一版"))
        AiConversationRepo.appendPart(msgId, 0, AiMessagePartDto(type = "text", text = "第二版"))

        assertEquals(0, syncLogCount(tbl = "ai_message_parts", op = "delete"), "覆盖同 ordinal 不是删除（R3）")
        assertEquals(1, countRows("ai_message_parts"))
        val text = Db.withConnection { conn ->
            conn.queryOne("SELECT text FROM ai_message_parts WHERE message_id = ? AND ordinal = 0", msgId) {
                it.getString(1)
            }
        }
        assertEquals("第二版", text, "upsert 要真的覆盖成新值")
    }

    // -----------------------------------------------------------------------
    //  R4：_sync_log.seq 必须 AUTOINCREMENT，删了也不复用
    // -----------------------------------------------------------------------

    @Test
    fun `R4 删除后再插入，_sync_log 的 seq 不复用（游标不会收到用过的号）`() {
        TestDb.reset()
        // 三次插入 → 三个 upsert 条目
        val ids = (1..3).mapNotNull { i ->
            Db.tx { conn -> TagRepo.ensure(conn, "tag$i") }
        }
        val maxBefore = maxSyncSeq()
        Db.tx { conn -> TagRepo.delete(conn, ids[1]) }

        val newId = Db.tx { conn -> TagRepo.ensure(conn, "tag4") }
        val newSeq = Db.withConnection { conn ->
            conn.queryOne("SELECT MAX(seq) FROM _sync_log WHERE pk = ?", newId.toString()) { it.getLong(1) }
        }
        assertTrue(
            newSeq != null && newSeq > maxBefore,
            "删行之后新插入的 seq 必须**严格大于**之前的最大值（$newSeq vs $maxBefore）：" +
                "rowid 复用会让客户端的增量游标永远跳过这一段（R4）",
        )
    }

    // -----------------------------------------------------------------------
    //  schema 约定：时间文本 / updated_at 触发器 / 幂等迁移
    // -----------------------------------------------------------------------

    @Test
    fun `时间列是 ISO-8601 UTC 文本（排序即字典序，契约 §2）`() {
        TestDb.reset()
        val id = Db.tx { conn -> PromptRepo.create(conn, PromptInput(title = "t", positivePrompt = "p")) }
        val raw = Db.withConnection { conn ->
            conn.queryOne("SELECT created_at FROM prompts WHERE id = ?", id) { it.getString(1) }
        }
        assertTrue(
            raw != null && Regex("""^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$""").matches(raw),
            "created_at 应当是 ISO-8601 UTC 文本，实际：$raw",
        )
        val instant = parseDbInstant(raw)
        assertTrue(instant != null && kotlin.math.abs(instant.epochSecond - java.time.Instant.now().epochSecond) < 60)
        assertEquals(raw, isoUtc(instant), "文本 → Instant → 文本 必须回到同一个串")
    }

    @Test
    fun `updated_at 由触发器维护（替掉 MySQL 的 ON UPDATE）`() {
        TestDb.reset()
        val id = Db.tx { conn -> PromptRepo.create(conn, PromptInput(title = "t", positivePrompt = "p")) }
        // 先把时间钉在一个确定的老值上：直接比"前后是否相等"会被**毫秒精度**坑到
        //（建行与改行落在同一毫秒里，值当然一样）。
        Db.withConnection { conn ->
            conn.execute("UPDATE prompts SET updated_at = ? WHERE id = ?", "2000-01-01T00:00:00.000Z", id)
        }
        // 只改正文、**故意不碰 updated_at** —— MySQL 靠 ON UPDATE 自动顶，SQLite 靠触发器
        Db.tx { conn ->
            conn.execute("UPDATE prompts SET positive_prompt = ? WHERE id = ?", "改过了", id)
        }
        val after = Db.withConnection { conn ->
            conn.queryOne("SELECT updated_at FROM prompts WHERE id = ?", id) { it.getString(1) }
        }
        assertNotEquals(
            "2000-01-01T00:00:00.000Z", after,
            "改了行却没顶 updated_at（提示词/会话列表按它排序）",
        )
    }

    @Test
    fun `迁移是收敛的：连跑两次不报错，且表与触发器数量不变`() {
        TestDb.reset()
        val tablesBefore = tableNames()
        val triggersBefore = triggerNames()
        val second = Migrate.run()
        assertEquals(tablesBefore, tableNames(), "第二次迁移不该多出表")
        assertEquals(triggersBefore, triggerNames(), "第二次迁移不该多出触发器")
        assertTrue(second > 0, "收敛式迁移应当照常把 DDL 走一遍（幂等执行）")
    }

    @Test
    fun `schema 里同步范围与契约一致：ai_run_events 没有触发器`() {
        TestDb.reset()
        val tables = triggerTargets()
        listOf(
            "prompts", "tags", "prompt_tags", "media_assets", "media_tags",
            "capture_runs", "ai_conversations", "ai_messages", "ai_message_parts", "ai_attachments",
        ).forEach { t ->
            assertTrue(t in tables, "$t 在同步范围内，必须有 _sync_log 触发器")
        }
        listOf("ai_run_events", "ai_runs", "ai_tool_calls", "ai_providers", "ai_models", "app_settings", "ai_questions")
            .forEach { t ->
                assertTrue(t !in tables, "$t 不在同步范围（契约 §6），不该有 _sync_log 触发器")
            }
    }

    // -----------------------------------------------------------------------
    //  helpers
    // -----------------------------------------------------------------------

    private fun countRows(table: String): Long =
        Db.withConnection { conn -> conn.queryOne("SELECT COUNT(*) FROM $table") { it.getLong(1) } } ?: -1

    private fun syncLogCount(tbl: String? = null, op: String? = null): Long = Db.withConnection { conn ->
        val where = buildList {
            tbl?.let { add("tbl = '$it'") }
            op?.let { add("op = '$it'") }
        }
        val sql = "SELECT COUNT(*) FROM _sync_log" + if (where.isEmpty()) "" else " WHERE " + where.joinToString(" AND ")
        conn.queryOne(sql) { it.getLong(1) }
    } ?: -1

    private fun maxSyncSeq(): Long =
        Db.withConnection { conn -> conn.queryOne("SELECT COALESCE(MAX(seq), 0) FROM _sync_log") { it.getLong(1) } } ?: 0

    private fun tableNames(): List<String> = Db.withConnection { conn ->
        conn.queryList("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name") { it.getString(1) }
    }

    private fun triggerNames(): List<String> = Db.withConnection { conn ->
        conn.queryList("SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name") { it.getString(1) }
    }

    private fun triggerTargets(): Set<String> = Db.withConnection { conn ->
        conn.queryList("SELECT tbl_name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'trg_%_sync_%'") {
            it.getString(1)
        }.toSet()
    }

    companion object {
        @AfterAll
        @JvmStatic
        fun cleanup() = TestDb.cleanup()
    }
}

/**
 * 测试用的临时 SQLite 库。
 *
 * `Db` 是单例（整个 JVM 一份连接池），所以用例之间靠 [reset] 换一个**全新的库文件** ——
 * 删库时必须连 `-wal` / `-shm` 一起删，否则残留的 WAL 会被新库当成自己的日志。
 */
internal object TestDb {
    private val dir: Path = Files.createTempDirectory("comfyhub-sqlite-test")

    val dbFile: Path = dir.resolve("comfyhub.db")

    /** 不带 PRAGMA 的裸 URL：测试要自己拼参数时用（[Db.init] 会再补默认那几条）。 */
    val jdbcUrl: String = "jdbc:sqlite:" + dbFile.toString().replace('\\', '/')

    private val cfg: AppConfig
        get() = AppConfig(
            host = "127.0.0.1",
            port = 0,
            jdbcUrl = jdbcUrl,
            dbUser = "",
            dbPassword = "",
            storageDir = dir.resolve("storage"),
            maxUploadBytes = 1,
            projectRoot = dir,
        )

    fun reset() {
        Db.close()
        Files.deleteIfExists(dbFile)
        Files.deleteIfExists(dbFile.resolveSibling("comfyhub.db-wal"))
        Files.deleteIfExists(dbFile.resolveSibling("comfyhub.db-shm"))
        Db.init(cfg)
        Migrate.run()
    }

    fun cleanup() {
        Db.close()
        runCatching {
            Files.list(dir).use { stream -> stream.forEach { Files.deleteIfExists(it) } }
            Files.deleteIfExists(dir)
        }
    }
}
