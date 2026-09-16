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

        // --- AI 工作台：Provider 与模型目录（M1 / AIH-006 / AIH-010） ---
        // 注意：凭据值永远不进库，只有 credential_ref 这个名字（AIH-012）。
        changed += createTable(
            conn, "ai_providers",
            """
            CREATE TABLE IF NOT EXISTS ai_providers (
              id             VARCHAR(96)   NOT NULL COMMENT '小写 kebab-case，创建后不可改',
              display_name   VARCHAR(128)  NOT NULL,
              api            VARCHAR(32)   NOT NULL COMMENT 'openai-completions / openai-responses / anthropic-messages',
              base_url       VARCHAR(1024) NOT NULL,
              credential_ref VARCHAR(128)  NULL COMMENT '凭据引用名；值由 DPAPI 单独保管',
              endpoint_trust VARCHAR(32)   NOT NULL DEFAULT 'public',
              headers_json   JSON          NULL COMMENT '高级配置；禁止放密钥',
              compat_json    JSON          NULL,
              enabled        TINYINT(1)    NOT NULL DEFAULT 1,
              revision       BIGINT        NOT NULL DEFAULT 1 COMMENT '乐观锁',
              created_at     DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              updated_at     DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                           ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        changed += createTable(
            conn, "ai_models",
            """
            CREATE TABLE IF NOT EXISTS ai_models (
              provider_id            VARCHAR(96)  NOT NULL,
              model_id               VARCHAR(191) NOT NULL,
              display_name           VARCHAR(191) NOT NULL,
              input_modalities       JSON         NULL COMMENT '能力真源，未知即不支持',
              attachment_transports  JSON         NULL,
              mime_allowlist         JSON         NULL,
              tools                  TINYINT(1)   NOT NULL DEFAULT 0,
              parallel_tools         TINYINT(1)   NOT NULL DEFAULT 0,
              reasoning              TINYINT(1)   NOT NULL DEFAULT 0,
              thinking_efforts       JSON         NULL COMMENT '可选的思考等级：等级 -> 过线拼写/预算（AIH-056）',
              thinking_format        VARCHAR(16)  NULL COMMENT '思考方言：openai/deepseek/qwen/openrouter/zai',
              context_window         INT          NULL,
              max_output_tokens      INT          NULL,
              max_attachment_bytes   BIGINT       NULL,
              max_attachment_count   INT          NULL,
              limits_json            JSON         NULL,
              capability_source      VARCHAR(16)  NOT NULL DEFAULT 'manual'
                                                  COMMENT 'builtin / discovered / manual / tested',
              capability_verified_at DATETIME(3)  NULL,
              enabled                TINYINT(1)   NOT NULL DEFAULT 1,
              created_at             DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              updated_at             DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                                  ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (provider_id, model_id),
              CONSTRAINT fk_ai_models_provider FOREIGN KEY (provider_id)
                REFERENCES ai_providers (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        // --- AI 会话 / 消息 / 有序消息块（M2 / AIH-018 / AIH-019） ---
        changed += createTable(
            conn, "ai_conversations",
            """
            CREATE TABLE IF NOT EXISTS ai_conversations (
              id                   CHAR(36)     NOT NULL,
              title                VARCHAR(255) NOT NULL DEFAULT '新对话',
              provider_id          VARCHAR(96)  NULL COMMENT '当前选择（消息里另有来源快照）',
              model_id             VARCHAR(191) NULL,
              system_prompt_version VARCHAR(32) NOT NULL DEFAULT 'v1',
              archived             TINYINT(1)   NOT NULL DEFAULT 0,
              created_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              updated_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                               ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              KEY idx_ai_conv_updated (updated_at),
              KEY idx_ai_conv_archived (archived, updated_at)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        changed += createTable(
            conn, "ai_messages",
            """
            CREATE TABLE IF NOT EXISTS ai_messages (
              id                   CHAR(36)     NOT NULL,
              conversation_id      CHAR(36)     NOT NULL,
              seq                  INT          NOT NULL DEFAULT 0 COMMENT '会话内单调序号',
              role                 VARCHAR(16)  NOT NULL COMMENT 'user / assistant / tool / system_note',
              status               VARCHAR(16)  NOT NULL DEFAULT 'complete'
                                                COMMENT 'pending / streaming / complete / failed / cancelled',
              text                 MEDIUMTEXT   NULL,
              provider_id          VARCHAR(96)  NULL COMMENT 'assistant 来源快照',
              model_id             VARCHAR(191) NULL,
              provider_response_id VARCHAR(191) NULL,
              replay_json          JSON         NULL COMMENT '协议原生续写所需状态（含版本）',
              usage_json           JSON         NULL,
              created_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              updated_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                               ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              KEY idx_ai_msg_conv (conversation_id, seq),
              CONSTRAINT fk_ai_msg_conv FOREIGN KEY (conversation_id)
                REFERENCES ai_conversations (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        changed += createTable(
            conn, "ai_message_parts",
            """
            CREATE TABLE IF NOT EXISTS ai_message_parts (
              message_id    CHAR(36)    NOT NULL,
              ordinal       INT         NOT NULL,
              type          VARCHAR(24) NOT NULL COMMENT 'text / reasoning / attachment / tool_call / tool_result',
              text          MEDIUMTEXT  NULL,
              attachment_id CHAR(36)    NULL,
              tool_call_id  VARCHAR(191) NULL,
              json_payload  JSON        NULL,
              created_at    DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              PRIMARY KEY (message_id, ordinal),
              CONSTRAINT fk_ai_part_msg FOREIGN KEY (message_id)
                REFERENCES ai_messages (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        // --- AI Run 与事件（M2 / AIH-020 ~ AIH-024） ---
        changed += createTable(
            conn, "ai_runs",
            """
            CREATE TABLE IF NOT EXISTS ai_runs (
              id                   CHAR(36)     NOT NULL,
              conversation_id      CHAR(36)     NOT NULL,
              status               VARCHAR(16)  NOT NULL COMMENT 'running / completed / failed / cancelled',
              provider_id          VARCHAR(96)  NULL,
              model_id             VARCHAR(191) NULL,
              user_message_id      CHAR(36)     NULL,
              assistant_message_id CHAR(36)     NULL,
              provider_snapshot    JSON         NULL COMMENT '脱敏快照：不含密钥',
              model_snapshot       JSON         NULL,
              skill_snapshot       JSON         NULL,
              prompt_version       VARCHAR(32)  NULL,
              retry_of_run_id      CHAR(36)     NULL,
              reasoning_effort     VARCHAR(16)  NULL COMMENT '本次 Run 实际使用的思考强度（AIH-056）',
              error_code           VARCHAR(48)  NULL,
              error_message        TEXT         NULL,
              usage_json           JSON         NULL,
              started_at           DATETIME(3)  NULL,
              completed_at         DATETIME(3)  NULL,
              created_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              updated_at           DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                               ON UPDATE CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              KEY idx_ai_runs_conv (conversation_id, created_at),
              KEY idx_ai_runs_status (status),
              CONSTRAINT fk_ai_runs_conv FOREIGN KEY (conversation_id)
                REFERENCES ai_conversations (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        changed += createTable(
            conn, "ai_run_events",
            """
            CREATE TABLE IF NOT EXISTS ai_run_events (
              run_id       CHAR(36)    NOT NULL,
              seq          INT         NOT NULL,
              event_type   VARCHAR(32) NOT NULL,
              payload_json JSON        NULL,
              created_at   DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              PRIMARY KEY (run_id, seq),
              CONSTRAINT fk_ai_events_run FOREIGN KEY (run_id)
                REFERENCES ai_runs (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        // --- 工具调用（M4 / AIH-033 ~ AIH-036 / AIH-049） ---
        // 审批状态与执行状态分开记：被用户拒绝的调用也必须留痕。
        changed += createTable(
            conn, "ai_tool_calls",
            """
            CREATE TABLE IF NOT EXISTS ai_tool_calls (
              id               CHAR(36)      NOT NULL,
              run_id           CHAR(36)      NOT NULL,
              provider_call_id VARCHAR(191)  NOT NULL COMMENT '上游的 tool_call_id / tool_use_id / call_id',
              name             VARCHAR(96)   NOT NULL,
              arguments_json   MEDIUMTEXT    NULL COMMENT '模型给的参数（已截断），可能含用户内容',
              approval         VARCHAR(16)   NOT NULL DEFAULT 'not_required'
                                             COMMENT 'not_required / pending / approved / denied',
              status           VARCHAR(16)   NOT NULL COMMENT 'ok / failed / denied',
              result_json      JSON          NULL COMMENT '结构化结果（不含密钥）',
              content          MEDIUMTEXT    NULL COMMENT '回灌给模型的文本（已截断）',
              error            VARCHAR(2000) NULL,
              elapsed_ms       BIGINT        NOT NULL DEFAULT 0,
              started_at       DATETIME(3)   NULL,
              completed_at     DATETIME(3)   NULL,
              created_at       DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              KEY idx_ai_tool_run (run_id, started_at),
              KEY idx_ai_tool_name (name),
              CONSTRAINT fk_ai_tool_run FOREIGN KEY (run_id)
                REFERENCES ai_runs (id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
            """.trimIndent()
        )

        // --- 思考强度（AIH-056）：老库补列，模型目录声明可选等级与网关方言 ---
        changed += addColumn(conn, "ai_models", "thinking_efforts", "JSON NULL COMMENT '可选的思考等级：等级 -> 过线拼写/预算'")
        changed += addColumn(conn, "ai_models", "thinking_format", "VARCHAR(16) NULL COMMENT '思考方言：openai/deepseek/qwen/openrouter/zai'")
        changed += addColumn(conn, "ai_runs", "reasoning_effort", "VARCHAR(16) NULL COMMENT '本次 Run 实际使用的思考强度'")

        // --- AI 附件（M3）：上传的原件 + 缩略图/预览帧。
        // 刻意**不**对 ai_message_parts.attachment_id 加外键：附件是用户可删的临时对象，
        // 删掉之后消息块还要能如实显示"这个附件已经不在了"，而不是被级联删掉半条消息。
        changed += createTable(
            conn, "ai_attachments",
            """
            CREATE TABLE IF NOT EXISTS ai_attachments (
              id            CHAR(36)     NOT NULL,
              original_name VARCHAR(255) NOT NULL,
              stored_name   VARCHAR(255) NOT NULL COMMENT 'storage/ai-attachments/<uuid>.<ext>',
              kind          VARCHAR(16)  NOT NULL COMMENT 'image / video / audio / document / text',
              mime_type     VARCHAR(128) NOT NULL,
              size_bytes    BIGINT       NOT NULL DEFAULT 0,
              width         INT          NULL,
              height        INT          NULL,
              sha256        CHAR(64)     NULL,
              status        VARCHAR(16)  NOT NULL DEFAULT 'ready' COMMENT 'ready / rejected / deleted',
              metadata_json JSON         NULL,
              created_at    DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              PRIMARY KEY (id),
              KEY idx_ai_attachments_created (created_at)
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
