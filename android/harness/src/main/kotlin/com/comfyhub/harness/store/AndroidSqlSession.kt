package com.comfyhub.harness.store

import androidx.sqlite.SQLiteConnection
import androidx.sqlite.SQLiteDriver
import androidx.sqlite.SQLiteStatement

/**
 * [SqlSession] 的**生产实现**：`androidx.sqlite` 的 bundled 引擎（红线 **R14**）。
 *
 * 为什么必须是 bundled：Android 13 的系统 SQLite 还是 **3.32a**、
 * `SQLITE_OMIT_LOAD_EXTENSION`、**没有 FTS5** —— 用系统库的话，`PRAGMA` / FTS 相关的行为
 * 会随用户手机的系统版本变，而我们根本没有办法在测试里覆盖那些版本。
 * bundled 引擎把 SQLite **编进 APK**，于是"手机上跑的 SQLite 是哪一版"由我们决定。
 *
 * ⚠ 这个类**不负责** PRAGMA 与建表：[HarnessDatabase.open] 是唯一入口。
 */
class AndroidSqlSession(private val connection: SQLiteConnection) : SqlSession {

    /** 事务嵌套深度（`transaction` 允许嵌套调用，只有最外层真的 BEGIN/COMMIT）。 */
    private var depth = 0

    override fun exec(sql: String, args: List<Any?>) {
        connection.prepare(sql).use { st ->
            bind(st, args)
            st.step()
        }
    }

    override fun update(sql: String, args: List<Any?>): Int {
        exec(sql, args)
        return changes()
    }

    override fun query(sql: String, args: List<Any?>): List<Row> =
        connection.prepare(sql).use { st ->
            bind(st, args)
            val columns = (0 until st.getColumnCount()).map { st.getColumnName(it) }
            val rows = mutableListOf<Row>()
            while (st.step()) {
                val values = HashMap<String, Any?>(columns.size)
                columns.forEachIndexed { i, name -> values[name] = read(st, i) }
                rows += Row(values)
            }
            rows
        }

    override val lastInsertRowId: Long
        get() = queryOne("SELECT last_insert_rowid() AS id")?.long("id") ?: -1L

    /**
     * R2：写事务一律 **`BEGIN IMMEDIATE`**。
     *
     * 裸 `BEGIN` 是"延迟事务"：先拿读锁、写的时候再升级 —— 升级那一下**可能不理会
     * `busy_timeout`** 直接返回 `SQLITE_BUSY`。手机上有前台服务 + 界面两条线程同时写，
     * 这个坑一定会撞上，所以这里不给调用方选择的机会。
     */
    override fun <T> transaction(block: () -> T): T {
        if (depth > 0) {
            depth++
            try {
                return block()
            } finally {
                depth--
            }
        }
        exec("BEGIN IMMEDIATE")
        depth = 1
        return try {
            val result = block()
            exec("COMMIT")
            result
        } catch (e: Throwable) {
            runCatching { exec("ROLLBACK") }
            throw e
        } finally {
            depth = 0
        }
    }

    override fun close() = connection.close()

    private fun changes(): Int = queryOne("SELECT changes() AS c")?.int("c") ?: 0

    private fun bind(st: SQLiteStatement, args: List<Any?>) {
        args.forEachIndexed { i, value ->
            val index = i + 1 // SQLite 的参数下标从 1 开始
            when (val p = param(value)) {
                null -> st.bindNull(index)
                is Long -> st.bindLong(index, p)
                is Double -> st.bindDouble(index, p)
                is String -> st.bindText(index, p)
                is ByteArray -> st.bindBlob(index, p)
                else -> st.bindText(index, p.toString())
            }
        }
    }

    private fun read(st: SQLiteStatement, index: Int): Any? {
        if (st.isNull(index)) return null
        // 列类型常量与 `androidx.sqlite.SQLite`（SQLITE_DATA_*）逐值相同；
        // 那个 object 在 Kotlin 侧不可见（Java 静态常量），所以这里就地声明。
        return when (st.getColumnType(index)) {
            TYPE_INTEGER -> st.getLong(index)
            TYPE_FLOAT -> st.getDouble(index)
            TYPE_TEXT -> st.getText(index)
            TYPE_BLOB -> st.getBlob(index)
            else -> st.getText(index)
        }
    }

    companion object {
        private const val TYPE_INTEGER = 1
        private const val TYPE_FLOAT = 2
        private const val TYPE_TEXT = 3
        private const val TYPE_BLOB = 4

        /** 按"连接工厂"的方式开库：**每条连接**都要过一遍 PRAGMA（R1）。 */
        fun open(driver: SQLiteDriver, fileName: String): AndroidSqlSession =
            AndroidSqlSession(driver.open(fileName)).also { MobilePragmas.apply(it) }
    }
}
