package com.comfyhub

import com.zaxxer.hikari.HikariConfig
import com.zaxxer.hikari.HikariDataSource
import org.slf4j.LoggerFactory
import java.math.BigDecimal
import java.sql.Connection
import java.sql.PreparedStatement
import java.sql.ResultSet
import java.sql.Statement
import java.sql.Timestamp
import java.time.Instant
import java.time.LocalDateTime
import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException

/**
 * 底层库方言。**只有两种，而且 MySQL 只留给一次性数据搬迁**。
 *
 * P2（2026-09-23，总纲 §3.1）：运行库从 MySQL 换成 SQLite（`jdbc:sqlite:...`），
 * 老库只在 `MigrateData` 里当**只读来源**。别在这里加第三种方言。
 */
enum class DbDialect { MYSQL, SQLITE }

/**
 * 时间文本格式（契约 §2）：`2026-09-23T12:34:56.789Z`。
 * 与 `db/schema.sqlite.sql` 里 `strftime('%Y-%m-%dT%H:%M:%fZ','now')` 的默认值逐字同形 ——
 * 两处**必须**一致，否则同一列里会混进两种写法，排序就乱了。
 */
private val ISO_UTC_FMT: DateTimeFormatter =
    DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'").withZone(ZoneOffset.UTC)

/** 老写法：`yyyy-MM-dd HH:mm:ss[.SSS]`（SQLite 的 `CURRENT_TIMESTAMP` / MySQL 的 DATETIME 文本）。 */
private val SPACE_TS_FMT: DateTimeFormatter =
    DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss[.SSS]")

/**
 * 极简 JDBC 层：HikariCP 连接池 + 一组小工具函数。
 * 这里不使用 ORM —— 我们的查询里有动态 WHERE / JSON / 聚合等方言细节，
 * 直接写 SQL 更可控，也避免了 ORM 版本兼容问题。
 *
 * ## P2 起的方言纪律（换库时踩到就是这里没守住）
 *
 * 1. **SQL 里不许再写 `CURRENT_TIMESTAMP(3)`** —— 用 [now]（SQLite 侧产出
 *    ISO-8601 UTC 文本，与 `created_at` 的列默认值同形，字典序即时间序）。
 * 2. **`ON DUPLICATE KEY UPDATE` → `ON CONFLICT(<键>) DO UPDATE`**（红线 R3），
 *    **`INSERT IGNORE` → `INSERT OR IGNORE`**。
 * 3. **每条 SQLite 连接都必须带 `PRAGMA foreign_keys=ON`**（红线 R1）：
 *    靠 [init] 把 PRAGMA 拼进 JDBC URL（驱动打开连接时逐条应用），
 *    [verify] 再读回来核对 —— 漏了不报错，只是 `ON DELETE SET NULL` / `CASCADE` 静默失效。
 * 4. **写事务一律 `BEGIN IMMEDIATE`**（红线 R2）：URL 里的 `transaction_mode=IMMEDIATE`
 *    会让 `autoCommit=false` 直接发 `begin immediate`，不会出现"读锁升级写锁
 *    不理 busy_timeout"那种 `SQLITE_BUSY`。
 * 5. 时间列**一律文本**：读用 [instantOrNull] / [isoTime]（SQLite 侧按 ISO 解析，
 *    MySQL 侧沿用 `getTimestamp`），写要么走列默认值、要么绑参数。
 */
object Db {
    private val log = LoggerFactory.getLogger(Db::class.java)

    lateinit var dataSource: HikariDataSource
        private set

    /** 当前连接的方言；[init] 时从 JDBC URL 判定。 */
    var dialect: DbDialect = DbDialect.SQLITE
        private set

    val isSqlite: Boolean get() = dialect == DbDialect.SQLITE

    val isInitialized: Boolean
        get() = ::dataSource.isInitialized && !dataSource.isClosed

    /**
     * "现在"的 **SQL 表达式**（不是值）。
     *
     * SQLite 没有 `CURRENT_TIMESTAMP(3)`：裸 `CURRENT_TIMESTAMP` 产出的是
     * `YYYY-MM-DD HH:MM:SS`（无毫秒、无时区标记），与契约要求的 ISO-8601 UTC 文本不同形，
     * 所以这里显式用 `strftime`。
     */
    val now: String
        get() = if (isSqlite) "strftime('%Y-%m-%dT%H:%M:%fZ','now')" else "CURRENT_TIMESTAMP(3)"

    /** Kotlin 侧"现在"的 ISO 文本（要当**参数**绑定时用它，别在拼 SQL 字符串）。 */
    fun nowIso(): String = isoUtc(Instant.now())

    fun init(cfg: AppConfig) {
        dialect = cfg.dbDialect
        if (dialect == DbDialect.SQLITE) ensureSqliteParentDir(cfg.jdbcUrl)
        val hc = HikariConfig().apply {
            jdbcUrl = if (dialect == DbDialect.SQLITE) sqliteUrlWithPragmas(cfg.jdbcUrl) else cfg.jdbcUrl
            // SQLite 没有账号概念：给了用户名/密码反而会被驱动当成未知属性（也可能被当成
            // "要加密的库"），所以这一支显式留空。
            username = if (dialect == DbDialect.SQLITE) null else cfg.dbUser
            password = if (dialect == DbDialect.SQLITE) null else cfg.dbPassword
            driverClassName =
                if (dialect == DbDialect.SQLITE) "org.sqlite.JDBC" else "com.mysql.cj.jdbc.Driver"
            poolName = "comfyhub-pool"
            connectionTimeout = 10_000
            idleTimeout = 60_000
            maxLifetime = 1_800_000
            isAutoCommit = true
            // 不做 fail-fast：启动时由上层带重试地等数据库。
            initializationFailTimeout = -1

            if (dialect == DbDialect.SQLITE) {
                // 内存库每个连接都是一个**独立**的空库 —— 池子只能有一条连接，
                // 否则第二个连接看不到第一个建的表（测试里踩过一次，症状是"表不存在"）。
                val memory = cfg.jdbcUrl.contains(":memory:")
                maximumPoolSize = if (memory) 1 else 4
                minimumIdle = 1
            } else {
                // P0（2026-09-23）：12 → 4。单用户桌面应用，实测真实并发远低于 4。
                maximumPoolSize = 4
                minimumIdle = 2
                addDataSourceProperty("cachePrepStmts", "true")
                addDataSourceProperty("prepStmtCacheSize", "250")
                addDataSourceProperty("prepStmtCacheSqlLimit", "2048")
            }
        }
        dataSource = HikariDataSource(hc)
        log.info(
            "{} 连接池已就绪: {}",
            if (dialect == DbDialect.SQLITE) "SQLite" else "MySQL",
            hc.jdbcUrl,
        )
        verify()
    }

    fun close() {
        if (::dataSource.isInitialized && !dataSource.isClosed) dataSource.close()
    }

    fun <T> withConnection(block: (Connection) -> T): T =
        dataSource.connection.use(block)

    /**
     * 事务块；抛异常自动回滚。
     *
     * ⚠ **不要在一个 [tx] 里再调用 [withConnection]**：那会去池子里再借一条连接，
     * 而写事务此刻已经占着写锁 —— SQLite 侧轻则等 `busy_timeout`、重则 `SQLITE_BUSY`。
     * 事务内部一律用传进来的 `conn`（`XxxRepo.xxx(conn, …)` 那些重载就是为它准备的）。
     */
    fun <T> tx(block: (Connection) -> T): T = dataSource.connection.use { conn ->
        val prev = conn.autoCommit
        conn.autoCommit = false
        try {
            val result = block(conn)
            conn.commit()
            result
        } catch (e: Throwable) {
            runCatching { conn.rollback() }
            throw e
        } finally {
            runCatching { conn.autoCommit = prev }
        }
    }

    // -----------------------------------------------------------------------
    //  连接自检（红线 R1 / R2 的防线）
    // -----------------------------------------------------------------------

    /**
     * 读回 PRAGMA 核对（启动时调用，也是 `SqlitePragmaTest` 的入口）。
     *
     * 为什么是"读回来核对"而不是"设了就信"：`foreign_keys` 是**按连接**的，
     * 一旦哪天有人绕过 [init] 自己 `DriverManager.getConnection`，外键就静默失效了 ——
     * 而本设计重度依赖 `ON DELETE SET NULL` / `CASCADE`（删提示词保产物、删标签清关联）。
     */
    fun verify(): SqlitePragmas? {
        if (dialect != DbDialect.SQLITE) return null
        val pragmas = withConnection { conn ->
            SqlitePragmas(
                foreignKeys = conn.queryOne("PRAGMA foreign_keys") { it.getInt(1) } ?: 0,
                busyTimeoutMs = conn.queryOne("PRAGMA busy_timeout") { it.getInt(1) } ?: 0,
                journalMode = conn.queryOne("PRAGMA journal_mode") { it.getString(1) } ?: "",
                userVersion = conn.queryOne("PRAGMA user_version") { it.getInt(1) } ?: 0,
                fts5 = fts5Available(conn),
            )
        }
        check(pragmas.foreignKeys == 1) {
            "SQLite 连接的 PRAGMA foreign_keys 不是 ON（当前=${pragmas.foreignKeys}）：" +
                "外键会静默失效，见红线 R1。"
        }
        if (pragmas.journalMode.lowercase() != "memory") {
            check(pragmas.journalMode.equals("wal", ignoreCase = true)) {
                "SQLite 不是 WAL 模式（当前=${pragmas.journalMode}）：读写会互斥。"
            }
        }
        log.info(
            "SQLite PRAGMA 自检通过: journal_mode={} foreign_keys={} busy_timeout={}ms user_version={} fts5={}",
            pragmas.journalMode, pragmas.foreignKeys, pragmas.busyTimeoutMs, pragmas.userVersion, pragmas.fts5,
        )
        return pragmas
    }

    private fun fts5Available(conn: Connection): Boolean = runCatching {
        conn.prepareStatement("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts5_probe USING fts5(x)").use { it.execute() }
        conn.prepareStatement("DROP TABLE IF EXISTS temp.fts5_probe").use { it.execute() }
        true
    }.getOrDefault(false)

    /** [verify] 读回来的实际值（`fts5` 日后做全文检索时要用）。 */
    data class SqlitePragmas(
        val foreignKeys: Int,
        val busyTimeoutMs: Int,
        val journalMode: String,
        val userVersion: Int,
        val fts5: Boolean,
    )

    /**
     * 建库文件所在目录。
     *
     * SQLite **不会**替你建目录：目录不存在时连接直接失败，而 Hikari 只报
     * "Connection is not available"，根因（`unable to open database file`）被埋在
     * 驱动里 —— 首次启动到全新机器上就是这条坑（本机实测过）。
     */
    internal fun ensureSqliteParentDir(url: String) {
        val raw = url.removePrefix("jdbc:sqlite:").substringBefore('?')
        if (raw.isBlank() || raw == ":memory:" || raw.startsWith("file:")) return
        val parent = java.nio.file.Paths.get(raw).toAbsolutePath().normalize().parent ?: return
        java.nio.file.Files.createDirectories(parent)
    }

    /**
     * 把连接级 PRAGMA 拼进 SQLite 的 JDBC URL（xerial 打开连接时逐条应用）。
     *
     * 显式给过的参数不覆盖（比如测试想用 `busy_timeout=200` 来验证 R2 的
     * `BEGIN IMMEDIATE`）。
     */
    internal fun sqliteUrlWithPragmas(raw: String): String {
        val wanted = listOf(
            "journal_mode" to "WAL",
            "synchronous" to "NORMAL",
            "busy_timeout" to "5000",
            "foreign_keys" to "on",
            "transaction_mode" to "IMMEDIATE",
        )
        val existing = raw.substringAfter('?', "")
            .split('&')
            .mapNotNull { it.substringBefore('=').takeIf { k -> k.isNotBlank() }?.lowercase() }
            .toSet()
        val extra = wanted.filterNot { it.first in existing }
        if (extra.isEmpty()) return raw
        val sep = if (raw.contains('?')) "&" else "?"
        return raw + sep + extra.joinToString("&") { "${it.first}=${it.second}" }
    }
}

// ---------------------------------------------------------------------------
// PreparedStatement 参数绑定
// ---------------------------------------------------------------------------

fun PreparedStatement.bindAll(params: Array<out Any?>) {
    params.forEachIndexed { i, value ->
        val idx = i + 1
        when (value) {
            null -> setObject(idx, null)
            is String -> setString(idx, value)
            is Int -> setInt(idx, value)
            is Long -> setLong(idx, value)
            is Boolean -> setBoolean(idx, value)
            is Double -> setDouble(idx, value)
            is Float -> setFloat(idx, value)
            is BigDecimal -> setBigDecimal(idx, value)
            is ByteArray -> setBytes(idx, value)
            // 时间一律按文本走（SQLite 没有时间类型；MySQL 的 DATETIME 也认 ISO 文本）
            is Instant -> setString(idx, isoUtc(value))
            is Timestamp -> setString(idx, isoUtc(value.toInstant()))
            else -> setObject(idx, value)
        }
    }
}

/** Instant → `2026-09-23T12:34:56.789Z`（契约 §2 的时间文本，UTC）。 */
fun isoUtc(value: Instant): String = ISO_UTC_FMT.format(value)

/**
 * 把库里的时间文本解析成 [Instant]。
 *
 * 认四种写法，都是"库自己写出来的"：
 *  · `2026-09-23T12:34:56.789Z`（我们的默认值 / 迁移工具写的）
 *  · `2026-09-23T12:34:56Z`（`Instant.toString()` 省略毫秒）
 *  · `2026-09-23T12:34:56.789+08:00`（带偏移）
 *  · `2026-09-23 12:34:56[.789]`（**SQLite 的 `CURRENT_TIMESTAMP` / 老 MySQL 的 DATETIME 文本**）
 *
 * ⚠ 最后一种按 **UTC** 解释：SQLite 的 `CURRENT_TIMESTAMP` 本来就是 UTC，
 * 而 MySQL 那条路不走这里（见 [ResultSet.instantOrNull]）。
 */
fun parseDbInstant(raw: String?): Instant? {
    val text = raw?.trim().orEmpty()
    if (text.isEmpty()) return null
    try {
        return Instant.parse(text)
    } catch (_: DateTimeParseException) {
    }
    try {
        return OffsetDateTime.parse(text).toInstant()
    } catch (_: DateTimeParseException) {
    }
    return try {
        LocalDateTime.parse(text, SPACE_TS_FMT).toInstant(ZoneOffset.UTC)
    } catch (_: DateTimeParseException) {
        null
    }
}

inline fun <T> Connection.queryList(sql: String, vararg params: Any?, crossinline map: (ResultSet) -> T): List<T> =
    prepareStatement(sql).use { ps ->
        ps.bindAll(params)
        ps.executeQuery().use { rs ->
            val out = ArrayList<T>()
            while (rs.next()) out.add(map(rs))
            out
        }
    }

inline fun <T> Connection.queryOne(sql: String, vararg params: Any?, crossinline map: (ResultSet) -> T): T? =
    prepareStatement(sql).use { ps ->
        ps.bindAll(params)
        ps.executeQuery().use { rs ->
            if (rs.next()) map(rs) else null
        }
    }

fun Connection.execute(sql: String, vararg params: Any?): Int =
    prepareStatement(sql).use { ps ->
        ps.bindAll(params)
        ps.executeUpdate()
    }

fun Connection.executeReturningKey(sql: String, vararg params: Any?): Long =
    prepareStatement(sql, Statement.RETURN_GENERATED_KEYS).use { ps ->
        ps.bindAll(params)
        ps.executeUpdate()
        ps.generatedKeys.use { keys ->
            if (keys.next()) keys.getLong(1) else error("INSERT 未返回自增主键")
        }
    }

// ---------------------------------------------------------------------------
// ResultSet 读取工具（全部对 NULL 安全）
// ---------------------------------------------------------------------------

fun ResultSet.str(col: String): String? = getString(col)

fun ResultSet.strOr(col: String, fallback: String = ""): String = getString(col) ?: fallback

fun ResultSet.intOrNull(col: String): Int? {
    val v = getInt(col)
    return if (wasNull()) null else v
}

fun ResultSet.longOrNull(col: String): Long? {
    val v = getLong(col)
    return if (wasNull()) null else v
}

fun ResultSet.doubleOrNull(col: String): Double? {
    val v = getDouble(col)
    return if (wasNull()) null else v
}

fun ResultSet.boolOr(col: String, fallback: Boolean = false): Boolean {
    val v = getBoolean(col)
    return if (wasNull()) fallback else v
}

/**
 * 时间列 → [Instant]。
 *
 * SQLite 侧时间列是**文本**（ISO-8601 UTC），所以按文本解析（xerial 的
 * `getTimestamp` 对 ISO 文本的容忍度不保证）；MySQL 侧沿用原来的 `getTimestamp`，
 * 老行为一个字不改（它只在搬迁工具里用）。
 */
fun ResultSet.instantOrNull(col: String): Instant? =
    if (Db.isSqlite) parseDbInstant(getString(col)) else getTimestamp(col)?.toInstant()

fun ResultSet.isoTime(col: String): String? = instantOrNull(col)?.toString()
