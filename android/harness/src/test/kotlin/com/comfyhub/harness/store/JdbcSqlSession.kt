package com.comfyhub.harness.store

import java.sql.Connection
import java.sql.DriverManager
import java.sql.ResultSet

/**
 * [SqlSession] 的**测试实现**：xerial `sqlite-jdbc`（自带 SQLite 3.53）。
 *
 * 为什么单测要另找一个引擎：Android 的 bundled 驱动是 JNI + 四套 ABI 的原生库，
 * 单元测试跑在宿主 JVM 上装不了。用 JDBC 的 SQLite 跑**同一份 DDL、同一句 SQL、同一组 PRAGMA**，
 * 测的就是真 SQL（不是拿一个假实现糊过去）；而"哪一版 SQLite"这件事在生产侧由
 * `BundledSQLiteDriver` 钉住（红线 R14），单测不承担那个职责。
 */
class JdbcSqlSession private constructor(private val conn: Connection) : SqlSession {

    private var depth = 0

    override fun exec(sql: String, args: List<Any?>) {
        conn.prepareStatement(sql).use { ps ->
            bind(ps, args)
            ps.execute()
        }
    }

    override fun update(sql: String, args: List<Any?>): Int =
        conn.prepareStatement(sql).use { ps ->
            bind(ps, args)
            ps.executeUpdate()
        }

    override fun query(sql: String, args: List<Any?>): List<Row> =
        conn.prepareStatement(sql).use { ps ->
            bind(ps, args)
            ps.executeQuery().use { rs -> readRows(rs) }
        }

    override val lastInsertRowId: Long
        get() = queryOne("SELECT last_insert_rowid() AS id")?.long("id") ?: -1L

    /** R2：与生产实现同一纪律 —— `BEGIN IMMEDIATE`，不接受裸 `BEGIN`。 */
    override fun <T> transaction(block: () -> T): T {
        if (depth > 0) {
            depth++
            try {
                return block()
            } finally {
                depth--
            }
        }
        conn.createStatement().use { it.execute("BEGIN IMMEDIATE") }
        depth = 1
        return try {
            val result = block()
            conn.createStatement().use { it.execute("COMMIT") }
            result
        } catch (e: Throwable) {
            runCatching { conn.createStatement().use { it.execute("ROLLBACK") } }
            throw e
        } finally {
            depth = 0
        }
    }

    override fun close() = conn.close()

    private fun bind(ps: java.sql.PreparedStatement, args: List<Any?>) {
        args.forEachIndexed { i, value ->
            when (val p = param(value)) {
                null -> ps.setNull(i + 1, java.sql.Types.NULL)
                is Long -> ps.setLong(i + 1, p)
                is Double -> ps.setDouble(i + 1, p)
                is String -> ps.setString(i + 1, p)
                is ByteArray -> ps.setBytes(i + 1, p)
                else -> ps.setString(i + 1, p.toString())
            }
        }
    }

    private fun readRows(rs: ResultSet): List<Row> {
        val meta = rs.metaData
        val columns = (1..meta.columnCount).map { meta.getColumnLabel(it) }
        val rows = mutableListOf<Row>()
        while (rs.next()) {
            val values = HashMap<String, Any?>(columns.size)
            columns.forEachIndexed { i, name -> values[name] = rs.getObject(i + 1) }
            rows += Row(values)
        }
        return rows
    }

    companion object {
        init {
            // 显式加载，免得某些 classloader 下 SPI 不生效
            Class.forName("org.sqlite.JDBC")
        }

        /** 临时文件库（多连接共享同一个库时用它 —— 内存库是**每连接一个**）。 */
        fun file(path: String): JdbcSqlSession =
            JdbcSqlSession(DriverManager.getConnection("jdbc:sqlite:$path")).also { MobilePragmas.apply(it) }

        /** 只要单连接的用例用它（快）。 */
        fun memory(): JdbcSqlSession =
            JdbcSqlSession(DriverManager.getConnection("jdbc:sqlite::memory:")).also { MobilePragmas.apply(it) }

        /**
         * 刻意**不上 PRAGMA** 的连接 —— 用来证明 R1 是"按连接生效"的
         * （也就是防线为什么必须放在连接工厂里）。
         */
        fun rawFile(path: String): JdbcSqlSession =
            JdbcSqlSession(DriverManager.getConnection("jdbc:sqlite:$path"))
    }
}
