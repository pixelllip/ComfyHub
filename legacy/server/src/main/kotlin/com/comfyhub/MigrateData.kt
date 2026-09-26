package com.comfyhub

import org.slf4j.LoggerFactory
import java.math.BigDecimal
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.sql.Connection
import java.sql.DriverManager
import java.sql.ResultSet
import java.sql.Timestamp
import java.time.LocalDateTime
import java.time.ZoneId

/**
 * 一次性数据搬迁：**MySQL（只读来源）→ SQLite（目标库）**（P2，总纲 §4）。
 *
 * 为什么要有它：P2 换库时老库里躺着用户真实数据（提示词 / 产物元数据 / 会话 / 工具调用）。
 * 换库不能靠"让用户重新攒一遍"，所以这里把老库读出来写进新库，**并当场核对**。
 *
 * ## 三条纪律
 *
 * 1. **只读来源**：MySQL 侧一条写语句都没有；目标库必须"空"（否则要 `force`，
 *    会先清空再灌 —— 免得把半份数据混进去）。
 * 2. **主键原样保留**（含自增 id）：产物 / 提示词的 id 是前端和 ComfyUI 扩展在用的东西，
 *    重编号会让旧链接、旧截图、旧 run_key 全对不上。SQLite 在插入显式 rowid 时
 *    会自动把 `sqlite_sequence` 顶到最大值，之后的 INSERT 不会撞号。
 * 3. **当场核对**（`Verify`）：逐表 `COUNT(*)` 相等 **且** 全表行哈希相等 ——
 *    "搬完了"和"搬对了"是两件事，抽样看两眼不算核对。
 *    行哈希按"每行算 sha256 → 排序 → 再 sha256"来做，**不依赖两边的排序规则**
 *    （MySQL 的 utf8mb4_unicode_ci 与 SQLite 的 BINARY 对同一批文本顺序不同，
 *    直接按 ORDER BY 比对会假红）。
 *
 * ## 刻意不搬的东西
 *
 * - `ai_run_events`（62 万行调试日志）：总纲 §7 待定项 5 的建议是**丢弃**，
 *   R5 的保留策略已经落地（`pruneEvents` + 环形缓冲），重连靠事件 seq 也只在同一次 Run 内。
 *   要一并搬过来就加 `includeEvents`。
 * - `_sync_log` / `sync_clients` 是 P2 新增的同步账本：搬迁产生的行**不是**"变更"
 *   （手机首次同步走 `/api/sync/bootstrap`），所以搬完直接清空 `_sync_log`。
 */
object MigrateData {
    private val log = LoggerFactory.getLogger(MigrateData::class.java)

    /** 依赖顺序（父表在前）——`PRAGMA foreign_keys=ON` 下必须按这个顺序灌。 */
    val TABLES: List<String> = listOf(
        "prompts", "tags", "prompt_tags", "media_assets", "media_tags", "capture_runs",
        "app_settings", "ai_providers", "ai_models", "ai_conversations", "ai_messages",
        "ai_message_parts", "ai_attachments", "ai_runs", "ai_run_events", "ai_tool_calls",
        "ai_questions",
    )

    /** 默认不搬的表（见类注释）。 */
    val SKIPPED_BY_DEFAULT: Set<String> = setOf("ai_run_events")

    data class TableReport(
        val table: String,
        val sourceRows: Long,
        val targetRows: Long,
        val hash: String,
        val matched: Boolean,
        val skipped: Boolean = false,
        val note: String? = null,
    )

    data class Report(val tables: List<TableReport>, val source: String, val target: String) {
        val ok: Boolean get() = tables.all { it.skipped || it.matched }
        val totalRows: Long get() = tables.sumOf { it.targetRows }
    }

    /**
     * 把 [sourceJdbcUrl]（MySQL）里的业务数据搬进已经建好结构的 [target]（SQLite）。
     *
     * @param force 目标库非空时是否先清空再灌（默认 false：宁可报错，也不混数据）
     */
    fun run(
        sourceJdbcUrl: String,
        sourceUser: String,
        sourcePassword: String,
        target: AppConfig,
        includeEvents: Boolean = false,
        force: Boolean = false,
    ): Report {
        require(target.dbDialect == DbDialect.SQLITE) { "目标库必须是 SQLite（现在写的是 ${target.jdbcUrl}）" }
        require(!sourceJdbcUrl.startsWith("jdbc:sqlite:")) { "来源必须是老 MySQL 库（现在是 ${sourceJdbcUrl}）" }
        check(Db.isInitialized && Db.isSqlite) {
            "请先把 Db 指向目标 SQLite 库（Db.init(target) + Migrate.run()）"
        }

        Class.forName("com.mysql.cj.jdbc.Driver")
        DriverManager.getConnection(sourceJdbcUrl, sourceUser, sourcePassword).use { src ->
            src.isReadOnly = true
            val skipped = TABLES.filter { it in SKIPPED_BY_DEFAULT && !includeEvents }
            val plans = TABLES.filter { it !in skipped }

            // 目标非空？（空 = 一行都没有）——非空要么是重复搬迁，要么是已经跑过 App
            val nonEmpty = Db.withConnection { conn ->
                plans.filter { (conn.queryOne("SELECT COUNT(*) FROM $it") { it.getLong(1) } ?: 0L) > 0L }
            }
            if (nonEmpty.isNotEmpty()) {
                check(force) {
                    "目标库不是空的（${nonEmpty.joinToString(", ")} 有数据）。" +
                        "确认要覆盖就加 --force（会先清空这些表），或换一个新的库文件。"
                }
                log.warn("目标库非空且给了 force：先清空 {} 张表", nonEmpty.size)
                wipe(plans)
            }

            val reports = mutableListOf<TableReport>()
            plans.forEach { table -> reports += copyTable(src, table) }
            skipped.forEach { table ->
                val rows = countOn(src, table).getOrDefault(-1)
                log.info("跳过 {}（{} 行）：按总纲 §7 待定项 5 —— 事件日志是调试数据，R5 的保留策略已落地", table, rows)
                reports += TableReport(table, rows, 0, "", matched = true, skipped = true, note = "按总纲 §7 待定项 5 丢弃")
            }

            // 搬迁不是"变更"：账本清掉（手机首次同步走 /bootstrap）
            Db.withConnection { conn -> conn.execute("DELETE FROM _sync_log") }

            val report = Report(reports, sourceJdbcUrl, target.jdbcUrl)
            report.tables.filter { !it.skipped }.forEach { r ->
                val mark = if (r.matched) "OK" else "**不一致**"
                log.info(
                    "{} {} 来源 {} 行 / 目标 {} 行 / 行哈希 {}",
                    mark, r.table, r.sourceRows, r.targetRows, r.hash.take(16),
                )
            }
            return report
        }
    }

    // -----------------------------------------------------------------------

    private fun copyTable(src: Connection, table: String): TableReport {
        // 列 = 目标表有的 ∩ 来源有的（老库可能缺后来才加的列；反过来也一样）
        val targetCols = Db.withConnection { conn ->
            conn.queryList("PRAGMA table_info($table)") { it.getString("name") }
        }
        val sourceCols = src.prepareStatement("SELECT * FROM `$table` WHERE 1 = 0").use { ps ->
            ps.executeQuery().use { rs ->
                val md = rs.metaData
                (1..md.columnCount).map { md.getColumnName(it) }
            }
        }
        val cols = targetCols.filter { tc -> sourceCols.any { it.equals(tc, ignoreCase = true) } }
        check(cols.isNotEmpty()) { "$table：来源与目标没有共同的列" }
        val missing = targetCols - cols.toSet()
        if (missing.isNotEmpty()) log.info("{}：来源没有这些列，按默认值填：{}", table, missing.joinToString(", "))

        val selectSql = "SELECT ${cols.joinToString(", ") { "`$it`" }} FROM `$table`"
        val insertSql = "INSERT INTO $table (${cols.joinToString(", ")}) VALUES (${cols.joinToString(", ") { "?" }})"

        var sourceRows = 0L
        val rowHashes = ArrayList<String>(1024)

        Db.tx { conn ->
            src.prepareStatement(selectSql).use { ps ->
                ps.fetchSize = 512
                ps.executeQuery().use { rs ->
                    conn.prepareStatement(insertSql).use { ins ->
                        while (rs.next()) {
                            sourceRows++
                            cols.forEachIndexed { i, _ -> ins.setObject(i + 1, fromMySql(rs.getObject(i + 1))) }
                            ins.addBatch()
                            rowHashes += sha256(canonicalRow(rs, cols))
                            if (sourceRows % 500 == 0L) ins.executeBatch()
                        }
                        ins.executeBatch()
                    }
                }
            }
        }

        val targetRows = Db.withConnection { conn -> conn.queryOne("SELECT COUNT(*) FROM $table") { it.getLong(1) } } ?: 0L
        val sourceHash = sha256(rowHashes.sorted().joinToString("\n"))
        val targetHash = hashTargetTable(table, cols)

        return TableReport(
            table = table,
            sourceRows = sourceRows,
            targetRows = targetRows,
            hash = targetHash,
            matched = sourceRows == targetRows && sourceHash == targetHash,
        ).also {
            if (!it.matched) {
                log.error(
                    "{} 核对失败：行数 {}/{}，哈希 {} vs {}",
                    table, sourceRows, targetRows, sourceHash.take(16), targetHash.take(16),
                )
            }
        }
    }

    /** 目标侧同一套算法（同一列集 → 每行 sha256 → 排序 → 再 sha256）。 */
    private fun hashTargetTable(table: String, cols: List<String>): String = Db.withConnection { conn ->
        val hashes = conn.queryList("SELECT ${cols.joinToString(", ")} FROM $table") { rs -> sha256(canonicalRow(rs, cols)) }
        sha256(hashes.sorted().joinToString("\n"))
    }

    /** 清空（逆依赖顺序，避开外键）。 */
    private fun wipe(tables: List<String>) {
        Db.tx { conn ->
            tables.reversed().forEach { conn.execute("DELETE FROM $it") }
            conn.execute("DELETE FROM _sync_log")
        }
    }

    private fun countOn(src: Connection, table: String): Result<Long> = runCatching {
        src.prepareStatement("SELECT COUNT(*) FROM `$table`").use { ps ->
            ps.executeQuery().use { rs -> if (rs.next()) rs.getLong(1) else 0L }
        }
    }

    // -----------------------------------------------------------------------
    //  取值：MySQL → SQLite
    // -----------------------------------------------------------------------

    /**
     * MySQL 的值 → SQLite 能原样存下来的值。
     *
     * 三处必须转换：
     *  · **时间**：老库是 `DATETIME(3)`，新库是 ISO-8601 UTC 文本。用 `ZoneId.systemDefault()`
     *    解释 LocalDateTime —— 与老代码 `rs.getTimestamp(col).toInstant()` 的口径**完全一致**
     *    （否则历史时间会整体偏移一个时区，而且不会报错）。
     *  · `TINYINT(1)` 布尔 → 1/0。
     *  · `JSON` 列有时以 `byte[]` 回来 → UTF-8 文本。
     */
    internal fun fromMySql(value: Any?): Any? = when (value) {
        null -> null
        is Timestamp -> isoUtc(value.toInstant())
        is LocalDateTime -> isoUtc(value.atZone(ZoneId.systemDefault()).toInstant())
        is java.util.Date -> isoUtc(value.toInstant())
        is Boolean -> if (value) 1 else 0
        is BigDecimal -> value.toDouble()
        is ByteArray -> String(value, StandardCharsets.UTF_8)
        is String, is Int, is Long, is Double, is Float, is Short -> value
        else -> value.toString()
    }

    /**
     * 一行 → 一段**两边都会算出同一个串**的规范文本（核对用）。
     *
     * 规范化规则要挡住三类"看起来不一样、其实一样"：
     *  · `DECIMAL(6,2)` 的 `7.50` ↔ SQLite REAL 的 `7.5`；
     *  · `TINYINT(1)` 的 `true` ↔ `1`；
     *  · `NULL` ↔ 空串（用 `\u0000` 当哨兵，空串本身是合法值）。
     */
    internal fun canonicalField(value: Any?): String = when (value) {
        null -> "\u0000"
        is Boolean -> if (value) "1" else "0"
        is BigDecimal -> BigDecimal.valueOf(value.toDouble()).stripTrailingZeros().toPlainString()
        is Double -> BigDecimal.valueOf(value).stripTrailingZeros().toPlainString()
        is Float -> BigDecimal.valueOf(value.toDouble()).stripTrailingZeros().toPlainString()
        is Timestamp -> isoUtc(value.toInstant())
        is LocalDateTime -> isoUtc(value.atZone(ZoneId.systemDefault()).toInstant())
        is java.util.Date -> isoUtc(value.toInstant())
        is ByteArray -> String(value, StandardCharsets.UTF_8)
        else -> value.toString()
    }

    private fun canonicalRow(rs: ResultSet, cols: List<String>): String =
        cols.joinToString("\u0001") { canonicalField(rs.getObject(it)) }

    private fun sha256(text: String): String =
        MessageDigest.getInstance("SHA-256").digest(text.toByteArray(StandardCharsets.UTF_8))
            .joinToString("") { "%02x".format(it) }
}
