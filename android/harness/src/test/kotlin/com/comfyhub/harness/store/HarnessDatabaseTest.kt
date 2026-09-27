package com.comfyhub.harness.store

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * 本地库的地基：PRAGMA / 建表 / 迁移 / 镜像表与桌面同形。
 *
 * 这里每一条都对着一条红线或一份契约：
 *  - **R1**：`foreign_keys=ON` 真的生效（**级联删除看得见**），而且"按连接生效"这件事有实据；
 *  - **R14**：FTS5 真的可用（用系统 SQLite 时这里会红）；
 *  - **契约 §3.1**：手机侧**不建账本**（`_sync_log` / `sync_clients` / 触发器一个都不许有）；
 *  - **契约 §4**：`_outbox` 的列齐全（M4 上行要按它取行）；
 *  - **镜像表与桌面逐列同形**：直接从 `db/schema.sqlite.sql` 读列清单来比对 ——
 *    不同形的话，`/api/sync` 发来的整行没法直接落库。
 */
class HarnessDatabaseTest {

    private lateinit var dir: Path
    private lateinit var dbPath: String

    @Before
    fun setUp() {
        dir = Files.createTempDirectory("harness-db-test")
        dbPath = dir.resolve("harness.db").toString()
    }

    @After
    fun tearDown() {
        runCatching { Files.walk(dir).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) } }
    }

    private fun open(): HarnessDatabase = HarnessDatabase.from(JdbcSqlSession.file(dbPath))

    // --- R1 / R14 / 版本 ---------------------------------------------------

    @Test
    fun `R1：foreign_keys 读回来必须是 1（写了不算数，要读回来）`() {
        val db = open()
        db.use {
            assertEquals(1L, it.session.queryOne("PRAGMA foreign_keys")!!.long("foreign_keys"))
            assertTrue(MobilePragmas.problems(it.session).isEmpty())
        }
    }

    @Test
    fun `R1：删会话要真的级联删掉消息与块、删标签要清掉关联行`() {
        val db = open()
        db.use { d ->
            val s = d.session
            s.exec("INSERT INTO ai_conversations(id, title) VALUES ('c1','t')")
            s.exec("INSERT INTO ai_messages(id, conversation_id, seq, role) VALUES ('m1','c1',1,'user')")
            s.exec("INSERT INTO ai_message_parts(message_id, ordinal, type, text) VALUES ('m1',0,'text','hi')")
            s.exec("INSERT INTO tags(id, name, normalized) VALUES (1,'猫','猫')")
            s.exec("INSERT INTO prompts(id, positive_prompt) VALUES (1,'1girl')")
            s.exec("INSERT INTO prompt_tags(prompt_id, tag_id) VALUES (1,1)")

            s.exec("DELETE FROM ai_conversations WHERE id='c1'")
            assertEquals(0L, s.queryOne("SELECT COUNT(*) AS c FROM ai_messages")!!.long("c"))
            assertEquals(0L, s.queryOne("SELECT COUNT(*) AS c FROM ai_message_parts")!!.long("c"))

            // 删提示词保产物（ON DELETE SET NULL）与删标签清关联（CASCADE）是两条不同的语义
            s.exec("DELETE FROM tags WHERE id=1")
            assertEquals(0L, s.queryOne("SELECT COUNT(*) AS c FROM prompt_tags")!!.long("c"))
        }
    }

    @Test
    fun `R1：另开一条没上 PRAGMA 的连接，foreign_keys 就是 0 —— 所以必须走连接工厂`() {
        open().close() // 先建好库
        JdbcSqlSession.rawFile(dbPath).use { raw ->
            assertEquals(
                "按连接生效：漏了 PRAGMA 的连接外键是关的（这就是 R1 防线只能放连接工厂的原因）",
                0L,
                raw.queryOne("PRAGMA foreign_keys")!!.long("foreign_keys"),
            )
        }
        JdbcSqlSession.file(dbPath).use { wired ->
            assertEquals(1L, wired.queryOne("PRAGMA foreign_keys")!!.long("foreign_keys"))
        }
    }

    @Test
    fun `R14：bundled 引擎必须带 FTS5（系统 SQLite 会在这一步红）`() {
        val db = open()
        db.use { d ->
            d.session.exec("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_ok USING fts5(x)")
            d.session.exec("INSERT INTO temp.fts_ok(x) VALUES ('赛博朋克少女')")
            assertEquals(
                1L,
                d.session.queryOne("SELECT COUNT(*) AS c FROM temp.fts_ok WHERE fts_ok MATCH '赛博*'")!!.long("c"),
            )
        }
    }

    @Test
    fun `建库是幂等的，且 user_version 记在库里`() {
        val first = open()
        val version = first.session.queryOne("PRAGMA user_version")!!.int("user_version")
        first.close()
        assertEquals(MobileSchema.VERSION, version)

        val second = open() // 再开一次不该炸（全部 IF NOT EXISTS）
        second.use {
            assertEquals(MobileSchema.VERSION, it.session.queryOne("PRAGMA user_version")!!.int("user_version"))
        }
    }

    // --- 契约 §3.1：手机不建账本 -------------------------------------------

    @Test
    fun `契约 3_1：手机侧不许有 _sync_log 与 sync_clients，也不许有触发器`() {
        val db = open()
        db.use { d ->
            val tables = d.session.query("SELECT name FROM sqlite_master WHERE type='table'")
                .map { it.str("name") }
            assertFalse("手机是投影不是副本，不许建变更账本", tables.contains("_sync_log"))
            assertFalse("sync_clients 是桌面侧的（每个设备的游标）", tables.contains("sync_clients"))

            val triggers = d.session.query("SELECT name FROM sqlite_master WHERE type='trigger'")
            assertTrue("手机侧一个触发器都不该有（updated_at 由桌面的整行覆盖带来）", triggers.isEmpty())
        }
    }

    @Test
    fun `契约 4：_outbox 的列齐全，且能在同一个事务里随业务行一起写`() {
        val db = open()
        db.use { d ->
            val s = d.session
            val columns = s.query("PRAGMA table_info(_outbox)").map { it.str("name") }
            assertEquals(listOf("id", "kind", "ref_id", "payload", "created_at", "tries", "last_error"), columns)

            s.transaction {
                s.exec("INSERT INTO ai_conversations(id, title) VALUES ('c9','手机起的会话')")
                s.exec(
                    "INSERT INTO _outbox(id, kind, ref_id, payload, created_at) VALUES (?,?,?,?,?)",
                    listOf("o1", "conversation", "c9", "{\"id\":\"c9\"}", "2026-09-23T00:00:00.000Z"),
                )
            }
            assertEquals(1L, s.queryOne("SELECT COUNT(*) AS c FROM _outbox")!!.long("c"))

            // 事务回滚时两边都不留
            runCatching {
                s.transaction {
                    s.exec("INSERT INTO ai_conversations(id, title) VALUES ('c10','回滚')")
                    s.exec(
                        "INSERT INTO _outbox(id, kind, ref_id, payload, created_at) VALUES (?,?,?,?,?)",
                        listOf("o2", "conversation", "c10", "{}", "2026-09-23T00:00:00.000Z"),
                    )
                    error("boom")
                }
            }
            assertFalse(s.queryOne("SELECT id FROM ai_conversations WHERE id='c10'") != null)
            assertFalse(s.queryOne("SELECT id FROM _outbox WHERE id='o2'") != null)
        }
    }

    // --- 镜像表与桌面同形 --------------------------------------------------

    /** 从桌面的 schema 文件里读某张表的列清单（唯一真源 = `db/schema.sqlite.sql`）。 */
    private fun desktopColumns(table: String): List<String> {
        var dir: Path? = Path.of("").toAbsolutePath()
        var file: Path? = null
        while (dir != null) {
            val candidate = dir.resolve("db").resolve("schema.sqlite.sql")
            if (Files.isRegularFile(candidate)) {
                file = candidate
                break
            }
            dir = dir.parent
        }
        val text = String(Files.readAllBytes(file!!), Charsets.UTF_8)
        val body = Regex(
            """CREATE TABLE IF NOT EXISTS\s+$table\s*\((.*?)\n\);""",
            setOf(RegexOption.DOT_MATCHES_ALL),
        ).find(text)?.groupValues?.get(1)
            ?: error("桌面 schema 里找不到表 $table")
        return body.lines()
            .map { it.trim() }
            // 去掉整行注释与行内注释（桌面 schema 里有 `-- NSFW 标记…` 这种独立注释行）
            .map { if (it.startsWith("--")) "" else it.substringBefore(" --").trim() }
            .filter { it.isNotEmpty() }
            .filterNot { line ->
                val head = line.substringBefore(' ').uppercase()
                head in setOf("PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "CONSTRAINT")
            }
            .map { it.substringBefore(' ').trim() }
            .filter { it.isNotEmpty() }
    }

    @Test
    fun `镜像表的列与桌面 schema 逐列一致（否则同步下来的整行没法直接落库）`() {
        val db = open()
        db.use { d ->
            MobileSchema.MIRROR_TABLES.forEach { table ->
                val local = d.session.query("PRAGMA table_info(${table.name})").map { it.str("name") }
                assertEquals(
                    "镜像表 ${table.name} 的列必须与桌面一致（列的**顺序**也要一致，便于逐行对照）",
                    desktopColumns(table.name),
                    local,
                )
            }
        }
    }

    @Test
    fun `镜像表的主键列与 mobile schema 里声明的是一致的`() {
        val db = open()
        db.use { d ->
            MobileSchema.MIRROR_TABLES.forEach { table ->
                val pk = d.session.query("PRAGMA table_info(${table.name})")
                    .filter { it.long("pk") != 0L }
                    .sortedBy { it.long("pk") }
                    .map { it.str("name") }
                assertEquals("${table.name} 的主键列", table.pkColumns, pk)
            }
        }
    }

    // --- 结构版本：必须与桌面同值 -------------------------------------------

    /** 从桌面的 Rust 源里读 `SCHEMA_VERSION`（唯一真源 = `server-rs/src/db.rs`）。 */
    private fun desktopSchemaVersion(): Int {
        var dir: Path? = Path.of("").toAbsolutePath()
        var file: Path? = null
        while (dir != null) {
            val candidate = dir.resolve("server-rs").resolve("src").resolve("db.rs")
            if (Files.isRegularFile(candidate)) {
                file = candidate
                break
            }
            dir = dir.parent
        }
        val text = String(Files.readAllBytes(file!!), Charsets.UTF_8)
        val match = Regex("""pub const SCHEMA_VERSION:\s*i64\s*=\s*(\d+)\s*;""").find(text)
            ?: error("server-rs/src/db.rs 里找不到 SCHEMA_VERSION")
        return match.groupValues[1].toInt()
    }

    /**
     * **手机侧的结构版本必须与桌面同值**（2026-09-27 拍板；理由写在 `MobileSchema.VERSION`）。
     *
     * 这个数在手机侧**只有一个用处**：与 `/api/sync/manifest` 回的 `schemaVersion` 比一比 ——
     * 不一致就丢镜像走全量。它要回答的是"桌面那份结构，我这套镜像还接得住吗"，
     * 而那完全由桌面的 `user_version` 决定。
     *
     * ⚠ 曾经两边不同步（桌面 4、手机 1）：后果是**每一次 pull 都在丢镜像、全量重来**，
     * 而且没有任何东西会报错。这条用例就是那次事故的防线 —— **改一边不改另一边当场变红**。
     */
    @Test
    fun `SCHEMA_VERSION 与桌面 server-rs 里的那个数同值（改一处必须改两处）`() {
        assertEquals(
            "MobileSchema.VERSION 必须与 server-rs/src/db.rs 的 SCHEMA_VERSION 同值；" +
                "两边不等会让每一次下行同步都走全量（丢镜像重来），而且是静默的",
            desktopSchemaVersion(),
            MobileSchema.VERSION,
        )
        // 顺带钉住"库里那个号与常量一致"（HarnessDatabase 幂等建表后写的就是它）
        open().use {
            assertEquals(
                MobileSchema.VERSION,
                it.session.queryOne("PRAGMA user_version")!!.int("user_version"),
            )
        }
    }
}
