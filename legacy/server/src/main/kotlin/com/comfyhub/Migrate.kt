package com.comfyhub

import org.slf4j.LoggerFactory
import java.sql.Connection

/**
 * 轻量级自动迁移。
 *
 * ## 两条路（P2，2026-09-23）
 *
 * - **SQLite（现在的运行库）**：DDL 的真源是 `db/schema.sqlite.sql`（构建时打进 jar），
 *   启动时**逐句幂等执行** + 用 `PRAGMA table_info` 补列，最后把 `PRAGMA user_version`
 *   设成 [com.comfyhub.ai.RunKit.SCHEMA_VERSION]（两类接口都拿它当 schemaVersion）。
 * - **MySQL（只留给一次性数据搬迁）**：原来的 information_schema 探测那套，一个字没改。
 *   新代码不要再往这条路上加东西。
 *
 * 为什么要"先探测再改"而不能只靠 `CREATE TABLE IF NOT EXISTS`：对**已经存在**的表，
 * `IF NOT EXISTS` 不会补新列（MySQL 8.4 也不支持 `ADD COLUMN IF NOT EXISTS`）。
 */
object Migrate {
    private val log = LoggerFactory.getLogger(Migrate::class.java)

    /** SQLite DDL 的 classpath 位置（`server/build.gradle.kts` 从 `<根>/db/` 拷进来）。 */
    const val SQLITE_SCHEMA_RESOURCE = "db/schema.sqlite.sql"

    fun run(): Int {
        val changed = if (Db.isSqlite) runSqlite() else runMysql()
        // --- 工具预算的一次性抬起（用户建议 ③，2026-09-22） ---
        // 走**设置项**而不是 DDL：库里那份 ai.tools.policy 冻着旧出厂值（8 轮 / 16 次），
        // 而这两项是用户可改的，normalizeStored 不许覆盖 —— 只能按值识别旧默认值再抬。
        // 两种方言都要跑（搬迁完第一次启动也得抬）。
        runCatching { com.comfyhub.ai.tools.ToolPolicy.bumpLegacyBudgets() }
            .onFailure { log.warn("工具预算抬起失败（不影响启动）: {}", it.message) }
        return changed
    }

    // -----------------------------------------------------------------------
    //  SQLite
    // -----------------------------------------------------------------------

    private fun runSqlite(): Int {
        val ddl = readSqliteSchema()
        val statements = SqlScript.split(ddl)
        check(statements.isNotEmpty()) { "SQLite schema 解析出 0 句，检查 $SQLITE_SCHEMA_RESOURCE" }

        var changed = 0
        Db.tx { conn ->
            val before = userVersion(conn)
            statements.forEach { stmt ->
                // 触发器必须"先删再建"：`CREATE TRIGGER IF NOT EXISTS` 对**已存在**的同名触发器
                // 什么都不做，改了触发器体的迁移就会静默不生效（这是 SQLite 侧的收敛式迁移陷阱）。
                triggerName(stmt)?.let { name ->
                    conn.execute("DROP TRIGGER IF EXISTS $name")
                }
                conn.execute(stmt)
                changed++
            }
            // 收敛式补列（新加列时在这里登记；老结构也能被补齐）
            changed += addColumnSqlite(conn, "prompts", "source", "TEXT NULL DEFAULT 'Manual'")
            changed += addColumnSqlite(conn, "prompts", "source_ref", "TEXT NULL")
            changed += addColumnSqlite(conn, "prompts", "workflow_json", "TEXT NULL")
            changed += addColumnSqlite(conn, "media_assets", "source_ref", "TEXT NULL")
            changed += addColumnSqlite(
                conn, "media_assets", "nsfw",
                "INTEGER NOT NULL DEFAULT 0 CHECK (nsfw IN (0,1))"
            )
            changed += addColumnSqlite(conn, "ai_models", "thinking_efforts", "TEXT NULL")
            changed += addColumnSqlite(conn, "ai_models", "thinking_format", "TEXT NULL")
            changed += addColumnSqlite(conn, "ai_runs", "reasoning_effort", "TEXT NULL")

            // 版本号与 DDL 在**同一个事务**里落，崩在中间也不会出现"版本说到了、结构没到"
            conn.execute("PRAGMA user_version = ${com.comfyhub.ai.RunKit.SCHEMA_VERSION}")
            if (before != com.comfyhub.ai.RunKit.SCHEMA_VERSION) {
                log.info(
                    "SQLite schema 版本 {} → {}（执行 {} 句 DDL）",
                    before, com.comfyhub.ai.RunKit.SCHEMA_VERSION, statements.size,
                )
            }
        }
        log.info("SQLite 结构已就绪（{} 句 DDL，{} 项补列）", statements.size, changed - statements.size)
        return changed
    }

    /** 读 SQLite DDL：优先 classpath（jar 里有），退回源码树里的 `db/schema.sqlite.sql`。 */
    private fun readSqliteSchema(): String {
        Migrate::class.java.classLoader.getResourceAsStream(SQLITE_SCHEMA_RESOURCE)?.use { input ->
            return input.readBytes().toString(Charsets.UTF_8)
        }
        // 兜底：某些运行方式（IDE 直接跑 main、没走 processResources）classpath 里没有
        val cwd = java.nio.file.Paths.get("").toAbsolutePath().normalize()
        listOf(cwd.resolve("../db/schema.sqlite.sql"), cwd.resolve("db/schema.sqlite.sql"))
            .firstOrNull { java.nio.file.Files.isRegularFile(it) }
            ?.let { return java.nio.file.Files.readString(it) }
        error(
            "找不到 SQLite schema（classpath:$SQLITE_SCHEMA_RESOURCE）。" +
                "重跑一次 `gradle installDist` 会把它打进产物。"
        )
    }

    private fun userVersion(conn: Connection): Int =
        conn.queryOne("PRAGMA user_version") { it.getInt(1) } ?: 0

    /** 从 `CREATE TRIGGER IF NOT EXISTS <名字>` / `CREATE TRIGGER <名字>` 里取名字。 */
    private fun triggerName(statement: String): String? {
        val m = Regex(
            """^\s*CREATE\s+TRIGGER\s+(?:IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_]*)""",
            RegexOption.IGNORE_CASE,
        ).find(statement) ?: return null
        return m.groupValues[1]
    }

    private fun columnExistsSqlite(conn: Connection, table: String, column: String): Boolean =
        conn.queryList("PRAGMA table_info($table)") { it.getString("name") }
            .any { it.equals(column, ignoreCase = true) }

    private fun addColumnSqlite(conn: Connection, table: String, column: String, ddl: String): Int {
        if (columnExistsSqlite(conn, table, column)) return 0
        log.info("新增列 {}.{}", table, column)
        return conn.execute("ALTER TABLE $table ADD COLUMN $column $ddl")
    }

    // -----------------------------------------------------------------------
    //  MySQL（历史路径，只给一次性搬迁）
    // -----------------------------------------------------------------------

    private fun runMysql(): Int = Db.withConnection { conn ->
        var changed = 0

        // --- prompts：来源标记 + 工作流快照 ---
        changed += addColumn(conn, "prompts", "source", "VARCHAR(32) NULL DEFAULT 'Manual' COMMENT '来源: Manual / ComfyUI / ComfyUI-Import'")
        changed += addColumn(conn, "prompts", "source_ref", "VARCHAR(128) NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）'")
        changed += addColumn(conn, "prompts", "workflow_json", "MEDIUMTEXT NULL COMMENT 'ComfyUI 界面格式工作流快照'")
        changed += addIndex(conn, "prompts", "idx_prompts_source_ref", "source_ref")

        // --- media_assets：产物也记住它来自哪次生成 ---
        changed += addColumn(conn, "media_assets", "source_ref", "VARCHAR(128) NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）'")
        changed += addIndex(conn, "media_assets", "idx_media_source_ref", "source_ref")
        // NSFW 标记（用户 bug 2026-09-20：模糊开关一开就把**所有**缩略图都糊了）。
        // 默认 0 = 没标记 = 不打码：宁可漏糊，也不要用户一开开关整页正常内容全被糊住。
        changed += addColumn(
            conn, "media_assets", "nsfw",
            "TINYINT(1) NOT NULL DEFAULT 0 COMMENT '是否成人内容（只影响预览图打不打码）'"
        )

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

        // --- 提问闸门落库（M1，2026-09-23） ---
        // 提问卡的历史形状已经随 tool_result 落在 ai_message_parts / ai_tool_calls 里，
        // 但"问过什么、答没答、有没有人在等"一直没有一个能按 questionId 直接查的地方：
        // 重启后 `GET /api/ai/questions/{id}` 只能 404，跨端也只能查"某个 Run 还在等什么"。
        // 状态机：pending → answered（用户答了）/ skipped（用户跳过）/ expired（超时、
        // Run 收尾、后端重启 —— 都按"没有回答就失效"收尾，语义一致）。
        // ⚠ 这张表**不进同步范围**（手机看实时提问走 `GET /api/ai/pending`，历史卡片从
        // parts 还原）—— 别把它加进 sqlite-schema-contract.md §6 的同步真源表。
        changed += createTable(
            conn, "ai_questions",
            """
            CREATE TABLE IF NOT EXISTS ai_questions (
              question_id     CHAR(36)     NOT NULL,
              run_id          CHAR(36)     NOT NULL,
              conversation_id CHAR(36)     NOT NULL,
              group_id        CHAR(36)     NOT NULL COMMENT '同一张提问卡的分组号',
              card_index      INT          NOT NULL DEFAULT 0 COMMENT '卡内第几题（0 基）',
              total           INT          NOT NULL DEFAULT 1,
              question        MEDIUMTEXT   NOT NULL,
              options_json    JSON         NULL,
              allow_free_text TINYINT(1)   NOT NULL DEFAULT 1,
              multi_select    TINYINT(1)   NOT NULL DEFAULT 0,
              header          VARCHAR(255) NULL,
              status          VARCHAR(16)  NOT NULL DEFAULT 'pending'
                                           COMMENT 'pending / answered / skipped / expired',
              answer          MEDIUMTEXT   NULL,
              option_index    INT          NULL,
              option_indexes  JSON         NULL COMMENT '多选题勾了哪几个',
              asked_at        DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
              answered_at     DATETIME(3)  NULL,
              PRIMARY KEY (question_id),
              KEY idx_ai_questions_run (run_id),
              KEY idx_ai_questions_status (status),
              CONSTRAINT fk_ai_questions_run FOREIGN KEY (run_id)
                REFERENCES ai_runs (id) ON DELETE CASCADE,
              CONSTRAINT fk_ai_questions_conv FOREIGN KEY (conversation_id)
                REFERENCES ai_conversations (id) ON DELETE CASCADE
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

        if (changed > 0) log.info("MySQL 结构已补齐（{} 项变更）", changed) else log.info("MySQL 结构已是最新")
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
