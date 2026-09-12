package com.comfyhub

import org.slf4j.LoggerFactory
import java.sql.Connection

/**
 * 轻量级自动迁移。
 *
 * `db/schema.sql` 用的是 `CREATE TABLE IF NOT EXISTS`，对**已经存在**的库不会补新列；
 * 而 MySQL 8.4 又不支持 `ADD COLUMN IF NOT EXISTS`。所以这里先查 information_schema
 * 再决定是否执行 DDL —— 幂等、可重复执行，后端每次启动都会把结构补齐。
 *
 * 这样「App 自动拉起后端」这条路上就不需要用户手动跑任何 SQL。
 * 同样的 DDL 也放在 `db/migrate.sql` 里，供命令行单独使用。
 */
object Migrate {
    private val log = LoggerFactory.getLogger(Migrate::class.java)

    fun run(): Int = Db.withConnection { conn ->
        var changed = 0

        // --- prompts：来源标记 + 工作流快照 ---
        changed += addColumn(conn, "prompts", "source", "VARCHAR(32) NULL DEFAULT 'Manual' COMMENT '来源: Manual / ComfyUI / ComfyUI-Import'")
        changed += addColumn(conn, "prompts", "source_ref", "VARCHAR(128) NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）'")
        changed += addColumn(conn, "prompts", "workflow_json", "MEDIUMTEXT NULL COMMENT 'ComfyUI 界面格式工作流快照'")
        changed += addIndex(conn, "prompts", "idx_prompts_source_ref", "source_ref")

        // --- media_assets：产物也记住它来自哪次生成 ---
        changed += addColumn(conn, "media_assets", "source_ref", "VARCHAR(128) NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）'")
        changed += addIndex(conn, "media_assets", "idx_media_source_ref", "source_ref")

        // --- 自动捕获的运行记录（run_key 唯一 => 同一次生成只入库一次） ---
        changed += createTable(
            conn, "capture_runs",
            """
            CREATE TABLE IF NOT EXISTS capture_runs (
              id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
              run_key     VARCHAR(191)    NOT NULL COMMENT '来源侧唯一键（ComfyUI prompt_id / import:<sha256>）',
              source      VARCHAR(32)     NOT NULL DEFAULT 'ComfyUI',
              prompt_id   BIGINT UNSIGNED NULL,
              status      VARCHAR(32)     NOT NULL DEFAULT 'success' COMMENT 'success / error / running',
              media_count INT             NOT NULL DEFAULT 0,
              title       VARCHAR(255)    NULL,
              error       TEXT            NULL,
              raw         JSON            NULL COMMENT '原始 history 片段（便于排查）',
              created_at  DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              UNIQUE KEY uk_capture_runs_key (run_key),
              KEY idx_capture_runs_created (created_at),
              KEY idx_capture_runs_prompt (prompt_id),
              CONSTRAINT fk_capture_runs_prompt FOREIGN KEY (prompt_id) REFERENCES prompts (id) ON DELETE SET NULL
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        // --- 运行期设置（自动捕获的开关 / 地址 / 目录等，App 与后端共用一份） ---
        changed += createTable(
            conn, "app_settings",
            """
            CREATE TABLE IF NOT EXISTS app_settings (
              k          VARCHAR(96)  NOT NULL,
              v          MEDIUMTEXT   NULL,
              updated_at DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                      ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (k)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        if (changed > 0) log.info("数据库结构已补齐（{} 项变更）", changed) else log.info("数据库结构已是最新")
        changed
    }

    // -----------------------------------------------------------------------

    private fun columnExists(conn: Connection, table: String, column: String): Boolean =
        (conn.queryOne(
            """
            SELECT COUNT(*) FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = ?
            """.trimIndent(), table, column
        ) { it.getInt(1) } ?: 0) > 0

    private fun indexExists(conn: Connection, table: String, index: String): Boolean =
        (conn.queryOne(
            """
            SELECT COUNT(*) FROM information_schema.STATISTICS
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND INDEX_NAME = ?
            """.trimIndent(), table, index
        ) { it.getInt(1) } ?: 0) > 0

    private fun tableExists(conn: Connection, table: String): Boolean =
        (conn.queryOne(
            """
            SELECT COUNT(*) FROM information_schema.TABLES
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?
            """.trimIndent(), table
        ) { it.getInt(1) } ?: 0) > 0

    private fun addColumn(conn: Connection, table: String, column: String, ddl: String): Int {
        if (columnExists(conn, table, column)) return 0
        log.info("新增列 {}.{}", table, column)
        return conn.execute("ALTER TABLE $table ADD COLUMN $column $ddl")
    }

    private fun addIndex(conn: Connection, table: String, index: String, columns: String): Int {
        if (indexExists(conn, table, index)) return 0
        log.info("新增索引 {} on {}", index, table)
        return conn.execute("ALTER TABLE $table ADD INDEX $index ($columns)")
    }

    private fun createTable(conn: Connection, table: String, ddl: String): Int {
        if (tableExists(conn, table)) return 0
        log.info("新建表 {}", table)
        conn.execute(ddl)
        return 1
    }
}
