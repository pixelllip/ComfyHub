package com.comfyhub.harness.store

/**
 * harness 的 SQL 会话抽象。
 *
 * **为什么要这层抽象**：手机生产环境的 SQLite 必须是 **bundled 驱动**（红线 **R14**：
 * Android 13 的系统 SQLite 还是 3.32a、`SQLITE_OMIT_LOAD_EXTENSION`、**无 FTS5**），
 * 而单元测试跑在 JVM 上、装不了 Android 的 JNI 库。于是：
 *
 * | 实现 | 用在哪 | 引擎 |
 * | --- | --- | --- |
 * | [AndroidSqlSession] | 手机运行时 | `androidx.sqlite` 的 `BundledSQLiteDriver`（R14） |
 * | `JdbcSqlSession`（**test** 源集） | 单元测试 | xerial `sqlite-jdbc`（自带 SQLite 3.53，同样是"带引擎"而不是系统库） |
 *
 * 两条路跑的是**同一份 DDL、同一份 SQL、同一组 PRAGMA**，所以单测真的在测生产 SQL
 * （不是拿一个假实现糊过去）。
 *
 * ⚠ R2：写事务一律 `BEGIN IMMEDIATE`（裸 `BEGIN` 的读锁升级可能**不理会 `busy_timeout`**
 * 直接 `SQLITE_BUSY`）。这条纪律由 [transaction] 唯一实现，调用方别再自己拼 BEGIN。
 */
interface SqlSession : AutoCloseable {

    /** 执行一句不取结果的 SQL（DDL / PRAGMA）。 */
    fun exec(sql: String, args: List<Any?> = emptyList())

    /** 执行一句写语句，返回受影响行数。 */
    fun update(sql: String, args: List<Any?> = emptyList()): Int

    fun query(sql: String, args: List<Any?> = emptyList()): List<Row>

    fun queryOne(sql: String, args: List<Any?> = emptyList()): Row? = query(sql, args).firstOrNull()

    /** 最近一次 INSERT 的 rowid（自增表用）。 */
    val lastInsertRowId: Long

    /** 语句本身出错时的底层消息（实现自己决定要不要暴露）。 */
    fun <T> transaction(block: () -> T): T

    /** 关连接（实现 `AutoCloseable` 是为了 `use {}` 能用在会话上）。 */
    override fun close()
}

/**
 * 一行查询结果（按列名取值）。
 *
 * 取值一律**显式可空**：镜像表（同步下来的行）里 `NULL` 是正常状态，
 * 用 `str()` 之类的非空便捷取法时要清楚自己在做什么。
 */
class Row(private val values: Map<String, Any?>) {
    fun raw(name: String): Any? = values[name]
    fun isNull(name: String): Boolean = values[name] == null
    fun contains(name: String): Boolean = values.containsKey(name)

    fun text(name: String): String? = when (val v = values[name]) {
        null -> null
        is String -> v
        is ByteArray -> String(v, Charsets.UTF_8)
        else -> v.toString()
    }

    /** 取文本，`NULL` 当空串（写库时的默认值大多是 `''`）。 */
    fun str(name: String): String = text(name).orEmpty()

    fun long(name: String): Long? = when (val v = values[name]) {
        null -> null
        is Long -> v
        is Int -> v.toLong()
        is Number -> v.toLong()
        is String -> v.toLongOrNull()
        else -> null
    }

    fun int(name: String): Int? = long(name)?.toInt()

    fun double(name: String): Double? = when (val v = values[name]) {
        null -> null
        is Number -> v.toDouble()
        is String -> v.toDoubleOrNull()
        else -> null
    }

    /** SQLite 没有布尔类型：一律 `INTEGER` + `CHECK(x IN (0,1))`。 */
    fun bool(name: String): Boolean = (long(name) ?: 0L) != 0L

    override fun toString(): String = values.toString()
}

/** 把 Kotlin 值绑成 SQL 参数（只接受 SQLite 真正有的那几类）。 */
internal fun bindable(value: Any?): Any? = when (value) {
    null -> null
    is String, is Long, is Int, is Double, is Float, is Boolean, is ByteArray -> value
    is Enum<*> -> value.name
    else -> value.toString()
}

/** 布尔 → 0/1（SQLite 没有布尔类型）。 */
internal fun param(value: Any?): Any? = when (val v = bindable(value)) {
    is Boolean -> if (v) 1L else 0L
    is Int -> v.toLong()
    is Float -> v.toDouble()
    else -> v
}
