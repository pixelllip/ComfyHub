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

/**
 * 极简 JDBC 层：HikariCP 连接池 + 一组小工具函数。
 * 这里不使用 ORM —— 我们的查询里有 GROUP_CONCAT / JSON / 动态条件等 MySQL 方言，
 * 直接写 SQL 更可控，也避免了 ORM 版本兼容问题。
 */
object Db {
    private val log = LoggerFactory.getLogger(Db::class.java)

    lateinit var dataSource: HikariDataSource
        private set

    val isInitialized: Boolean
        get() = ::dataSource.isInitialized && !dataSource.isClosed

    fun init(cfg: AppConfig) {
        val hc = HikariConfig().apply {
            jdbcUrl = cfg.jdbcUrl
            username = cfg.dbUser
            password = cfg.dbPassword
            driverClassName = "com.mysql.cj.jdbc.Driver"
            maximumPoolSize = 12
            minimumIdle = 2
            poolName = "comfyhub-pool"
            connectionTimeout = 10_000
            idleTimeout = 60_000
            maxLifetime = 1_800_000
            isAutoCommit = true
            // 不做 fail-fast：启动时由上层带重试地等数据库，
            // 这样 MySQL 比后端晚几秒起来也不会把后端直接弄挂。
            initializationFailTimeout = -1
            addDataSourceProperty("cachePrepStmts", "true")
            addDataSourceProperty("prepStmtCacheSize", "250")
            addDataSourceProperty("prepStmtCacheSqlLimit", "2048")
        }
        dataSource = HikariDataSource(hc)
        log.info("MySQL 连接池已就绪: {}", cfg.jdbcUrl)
    }

    fun close() {
        if (::dataSource.isInitialized && !dataSource.isClosed) dataSource.close()
    }

    fun <T> withConnection(block: (Connection) -> T): T =
        dataSource.connection.use(block)

    /** 事务块；抛异常自动回滚 */
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
            is Instant -> setTimestamp(idx, Timestamp.from(value))
            else -> setObject(idx, value)
        }
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

fun ResultSet.instantOrNull(col: String): Instant? = getTimestamp(col)?.toInstant()

fun ResultSet.isoTime(col: String): String? = instantOrNull(col)?.toString()
