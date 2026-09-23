package com.comfyhub.harness.store

/**
 * 手机端本地库的 schema（🔒 `docs/sqlite-schema-contract.md` §3.1）。
 *
 * ## 两条形态纪律
 *
 * 1. **镜像表与桌面逐列同形**：`prompts` / `ai_messages` … 这些是从桌面同步下来的**投影**，
 *    列名必须与 `db/schema.sqlite.sql` 完全一致 —— 否则 `/api/sync/{bootstrap,changes}`
 *    发来的整行没法直接 `INSERT`，只能逐列翻译（那正是"两端漂移"的来源）。
 *    这条由 `MirrorSchemaParityTest` 盯着（它**从桌面的 schema 文件里读**列清单来比对）。
 * 2. **手机侧不建账本、不挂触发器**（契约 §3.1）：手机是投影不是副本，
 *    它没有"要让别人同步"的义务，所以 `_sync_log` / `sync_clients` / 那 25 个触发器
 *    在手机上一个都不该有。桌面的 `updated_at` 触发器同理不需要 ——
 *    镜像行的 `updated_at` 是桌面给的，手机只做整行覆盖。
 *
 * ## 本机表（手机自己产生、要上行回去的）
 *
 * - `ai_runs` / `ai_run_events`：本地 Run 与事件流（**R5**：切后台再回来靠它回放）；
 * - `_outbox`：上行队列（契约 §4），每条本地写**在同一个事务里**同时写业务表和它；
 * - `mobile_kv`：同步游标 / 设备号 / run-kit 缓存版本这类零散状态。
 */
object MobileSchema {

    /** 与桌面 `PRAGMA user_version` 是**两本账**：这里是本地库的结构版本。 */
    const val VERSION = 1

    /** 镜像表的同步范围与主键形态（`/api/sync` 那三条读端点的 apply 也按它走）。 */
    enum class PkKind { SINGLE, COMPOSITE }

    /**
     * 一张镜像表。
     *
     * @param pkColumns 单列表是 `[id]`；复合主键按桌面 `_sync_log` 的拼法顺序列出来
     *   （`prompt_tags` → `[prompt_id, tag_id]`，`ai_message_parts` → `[message_id, ordinal]`）
     */
    data class MirrorTable(val name: String, val pkColumns: List<String>, val pkKind: PkKind)

    /**
     * 镜像表清单 —— **顺序就是 apply 顺序**（父表在前）。
     *
     * ⚠ 顺序不是装饰：`PRAGMA foreign_keys = ON`（R1）下，先插 `ai_messages` 再插
     * `ai_conversations` 会直接违反外键。bootstrap 分页与 changes 批次都按这个顺序落库。
     */
    val MIRROR_TABLES: List<MirrorTable> = listOf(
        MirrorTable("tags", listOf("id"), PkKind.SINGLE),
        MirrorTable("prompts", listOf("id"), PkKind.SINGLE),
        MirrorTable("prompt_tags", listOf("prompt_id", "tag_id"), PkKind.COMPOSITE),
        MirrorTable("media_assets", listOf("id"), PkKind.SINGLE),
        MirrorTable("media_tags", listOf("media_id", "tag_id"), PkKind.COMPOSITE),
        MirrorTable("capture_runs", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_conversations", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_messages", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_message_parts", listOf("message_id", "ordinal"), PkKind.COMPOSITE),
        MirrorTable("ai_attachments", listOf("id"), PkKind.SINGLE),
    )

    /** 手机自己写、当期**不上行**的表（Run 过程与凭据引用）。 */
    val LOCAL_ONLY_TABLES: List<String> = listOf(
        "ai_providers", "ai_models", "ai_runs", "ai_run_events", "ai_tool_calls", "_outbox", "mobile_kv",
    )

    // -----------------------------------------------------------------------
    //  镜像表：与 db/schema.sqlite.sql 逐列同形（去掉了 _sync_log / 触发器）
    // -----------------------------------------------------------------------

    private val MIRROR_DDL: List<String> = listOf(
        """
        CREATE TABLE IF NOT EXISTS tags (
          id          INTEGER PRIMARY KEY AUTOINCREMENT,
          name        TEXT    NOT NULL,
          normalized  TEXT    NOT NULL,
          category    TEXT    NULL,
          color       TEXT    NULL,
          description TEXT    NULL,
          use_count   INTEGER NOT NULL DEFAULT 0,
          created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        )
        """.trimIndent(),
        "CREATE UNIQUE INDEX IF NOT EXISTS uk_tags_normalized ON tags(normalized)",
        "CREATE INDEX IF NOT EXISTS idx_tags_category  ON tags(category)",
        "CREATE INDEX IF NOT EXISTS idx_tags_use_count ON tags(use_count)",

        """
        CREATE TABLE IF NOT EXISTS prompts (
          id                INTEGER PRIMARY KEY AUTOINCREMENT,
          title             TEXT    NOT NULL DEFAULT '',
          kind              TEXT    NOT NULL DEFAULT 'IMAGE'
                            CHECK (kind IN ('IMAGE','VIDEO','AUDIO','MIXED')),
          positive_prompt   TEXT    NOT NULL,
          negative_prompt   TEXT    NULL,
          checkpoint        TEXT    NULL,
          loras             TEXT    NULL,
          sampler           TEXT    NULL,
          scheduler         TEXT    NULL,
          steps             INTEGER NULL,
          cfg_scale         REAL    NULL,
          seed              INTEGER NULL,
          width             INTEGER NULL,
          height            INTEGER NULL,
          batch_size        INTEGER NULL,
          extra_params      TEXT    NULL,
          notes             TEXT    NULL,
          favorite          INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0,1)),
          source            TEXT    NULL DEFAULT 'Manual',
          source_ref        TEXT    NULL,
          workflow_json     TEXT    NULL,
          created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_prompts_kind       ON prompts(kind)",
        "CREATE INDEX IF NOT EXISTS idx_prompts_created    ON prompts(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_prompts_favorite   ON prompts(favorite)",
        "CREATE INDEX IF NOT EXISTS idx_prompts_source_ref ON prompts(source_ref)",

        """
        CREATE TABLE IF NOT EXISTS prompt_tags (
          prompt_id INTEGER NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
          tag_id    INTEGER NOT NULL REFERENCES tags(id)    ON DELETE CASCADE,
          PRIMARY KEY (prompt_id, tag_id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_pt_tag ON prompt_tags(tag_id)",

        """
        CREATE TABLE IF NOT EXISTS media_assets (
          id             INTEGER PRIMARY KEY AUTOINCREMENT,
          prompt_id      INTEGER NULL REFERENCES prompts(id) ON DELETE SET NULL,
          kind           TEXT    NOT NULL CHECK (kind IN ('IMAGE','VIDEO','AUDIO')),
          title          TEXT    NOT NULL DEFAULT '',
          original_name  TEXT    NOT NULL,
          stored_name    TEXT    NOT NULL,
          mime_type      TEXT    NULL,
          size_bytes     INTEGER NOT NULL DEFAULT 0,
          width          INTEGER NULL,
          height         INTEGER NULL,
          duration_ms    INTEGER NULL,
          sha256         TEXT    NULL,
          source         TEXT    NULL DEFAULT 'ComfyUI',
          source_ref     TEXT    NULL,
          workflow_json  TEXT    NULL,
          notes          TEXT    NULL,
          favorite       INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0,1)),
          nsfw           INTEGER NOT NULL DEFAULT 0 CHECK (nsfw IN (0,1)),
          created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_media_prompt     ON media_assets(prompt_id)",
        "CREATE INDEX IF NOT EXISTS idx_media_kind       ON media_assets(kind)",
        "CREATE INDEX IF NOT EXISTS idx_media_created    ON media_assets(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_media_sha256     ON media_assets(sha256)",
        "CREATE INDEX IF NOT EXISTS idx_media_source_ref ON media_assets(source_ref)",

        """
        CREATE TABLE IF NOT EXISTS media_tags (
          media_id INTEGER NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
          tag_id   INTEGER NOT NULL REFERENCES tags(id)         ON DELETE CASCADE,
          PRIMARY KEY (media_id, tag_id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_mt_tag ON media_tags(tag_id)",

        """
        CREATE TABLE IF NOT EXISTS capture_runs (
          id          INTEGER PRIMARY KEY AUTOINCREMENT,
          run_key     TEXT    NOT NULL,
          source      TEXT    NOT NULL DEFAULT 'ComfyUI',
          prompt_id   INTEGER NULL REFERENCES prompts(id) ON DELETE SET NULL,
          status      TEXT    NOT NULL DEFAULT 'success',
          media_count INTEGER NOT NULL DEFAULT 0,
          title       TEXT    NULL,
          error       TEXT    NULL,
          raw         TEXT    NULL,
          created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        )
        """.trimIndent(),
        "CREATE UNIQUE INDEX IF NOT EXISTS uk_capture_runs_key ON capture_runs(run_key)",
        "CREATE INDEX IF NOT EXISTS idx_capture_runs_created ON capture_runs(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_capture_runs_prompt  ON capture_runs(prompt_id)",

        """
        CREATE TABLE IF NOT EXISTS ai_conversations (
          id                    TEXT    NOT NULL,
          title                 TEXT    NOT NULL DEFAULT '新对话',
          provider_id           TEXT    NULL,
          model_id              TEXT    NULL,
          system_prompt_version TEXT    NOT NULL DEFAULT 'v1',
          archived              INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0,1)),
          created_at            TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at            TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_conv_updated  ON ai_conversations(updated_at)",
        "CREATE INDEX IF NOT EXISTS idx_ai_conv_archived ON ai_conversations(archived, updated_at)",

        """
        CREATE TABLE IF NOT EXISTS ai_messages (
          id                   TEXT    NOT NULL,
          conversation_id      TEXT    NOT NULL REFERENCES ai_conversations(id) ON DELETE CASCADE,
          seq                  INTEGER NOT NULL DEFAULT 0,
          role                 TEXT    NOT NULL,
          status               TEXT    NOT NULL DEFAULT 'complete',
          text                 TEXT    NULL,
          provider_id          TEXT    NULL,
          model_id             TEXT    NULL,
          provider_response_id TEXT    NULL,
          replay_json          TEXT    NULL,
          usage_json           TEXT    NULL,
          created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_msg_conv ON ai_messages(conversation_id, seq)",

        """
        CREATE TABLE IF NOT EXISTS ai_message_parts (
          message_id    TEXT    NOT NULL REFERENCES ai_messages(id) ON DELETE CASCADE,
          ordinal       INTEGER NOT NULL,
          type          TEXT    NOT NULL,
          text          TEXT    NULL,
          attachment_id TEXT    NULL,
          tool_call_id  TEXT    NULL,
          json_payload  TEXT    NULL,
          created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (message_id, ordinal)
        )
        """.trimIndent(),

        """
        CREATE TABLE IF NOT EXISTS ai_attachments (
          id            TEXT    NOT NULL,
          original_name TEXT    NOT NULL,
          stored_name   TEXT    NOT NULL,
          kind          TEXT    NOT NULL,
          mime_type     TEXT    NOT NULL,
          size_bytes    INTEGER NOT NULL DEFAULT 0,
          width         INTEGER NULL,
          height        INTEGER NULL,
          sha256        TEXT    NULL,
          status        TEXT    NOT NULL DEFAULT 'ready',
          metadata_json TEXT    NULL,
          created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_attachments_created ON ai_attachments(created_at)",
    )

    // -----------------------------------------------------------------------
    //  本机表
    // -----------------------------------------------------------------------

    private val LOCAL_DDL: List<String> = listOf(
        // 凭据引用（值在 Android Keystore，库里只有名字）—— M6 的下行目标
        """
        CREATE TABLE IF NOT EXISTS ai_providers (
          id             TEXT    NOT NULL,
          display_name   TEXT    NOT NULL,
          api            TEXT    NOT NULL,
          base_url       TEXT    NOT NULL,
          credential_ref TEXT    NULL,
          endpoint_trust TEXT    NOT NULL DEFAULT 'public',
          headers_json   TEXT    NULL,
          compat_json    TEXT    NULL,
          enabled        INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
          revision       INTEGER NOT NULL DEFAULT 1,
          created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),

        """
        CREATE TABLE IF NOT EXISTS ai_models (
          provider_id            TEXT    NOT NULL REFERENCES ai_providers(id) ON DELETE CASCADE,
          model_id               TEXT    NOT NULL,
          display_name           TEXT    NOT NULL,
          input_modalities       TEXT    NULL,
          attachment_transports  TEXT    NULL,
          mime_allowlist         TEXT    NULL,
          tools                  INTEGER NOT NULL DEFAULT 0 CHECK (tools IN (0,1)),
          parallel_tools         INTEGER NOT NULL DEFAULT 0 CHECK (parallel_tools IN (0,1)),
          reasoning              INTEGER NOT NULL DEFAULT 0 CHECK (reasoning IN (0,1)),
          thinking_efforts       TEXT    NULL,
          thinking_format        TEXT    NULL,
          context_window         INTEGER NULL,
          max_output_tokens      INTEGER NULL,
          max_attachment_bytes   INTEGER NULL,
          max_attachment_count   INTEGER NULL,
          limits_json            TEXT    NULL,
          capability_source      TEXT    NOT NULL DEFAULT 'manual',
          capability_verified_at TEXT    NULL,
          enabled                INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
          created_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (provider_id, model_id)
        )
        """.trimIndent(),

        """
        CREATE TABLE IF NOT EXISTS ai_runs (
          id                   TEXT    NOT NULL,
          conversation_id      TEXT    NOT NULL REFERENCES ai_conversations(id) ON DELETE CASCADE,
          status               TEXT    NOT NULL,
          provider_id          TEXT    NULL,
          model_id             TEXT    NULL,
          user_message_id      TEXT    NULL,
          assistant_message_id TEXT    NULL,
          provider_snapshot    TEXT    NULL,
          model_snapshot       TEXT    NULL,
          skill_snapshot       TEXT    NULL,
          prompt_version       TEXT    NULL,
          retry_of_run_id      TEXT    NULL,
          reasoning_effort     TEXT    NULL,
          error_code           TEXT    NULL,
          error_message        TEXT    NULL,
          usage_json           TEXT    NULL,
          started_at           TEXT    NULL,
          completed_at         TEXT    NULL,
          created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_runs_conv   ON ai_runs(conversation_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_ai_runs_status ON ai_runs(status)",

        """
        CREATE TABLE IF NOT EXISTS ai_run_events (
          run_id       TEXT    NOT NULL REFERENCES ai_runs(id) ON DELETE CASCADE,
          seq          INTEGER NOT NULL,
          event_type   TEXT    NOT NULL,
          payload_json TEXT    NULL,
          created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (run_id, seq)
        )
        """.trimIndent(),

        """
        CREATE TABLE IF NOT EXISTS ai_tool_calls (
          id               TEXT    NOT NULL,
          run_id           TEXT    NOT NULL REFERENCES ai_runs(id) ON DELETE CASCADE,
          provider_call_id TEXT    NOT NULL,
          name             TEXT    NOT NULL,
          arguments_json   TEXT    NULL,
          approval         TEXT    NOT NULL DEFAULT 'not_required',
          status           TEXT    NOT NULL,
          result_json      TEXT    NULL,
          content          TEXT    NULL,
          error            TEXT    NULL,
          elapsed_ms       INTEGER NOT NULL DEFAULT 0,
          started_at       TEXT    NULL,
          completed_at     TEXT    NULL,
          created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_tool_run  ON ai_tool_calls(run_id, started_at)",
        "CREATE INDEX IF NOT EXISTS idx_ai_tool_name ON ai_tool_calls(name)",

        // 上行队列（🔒 sync-api-contract §4）—— 手机侧**只有**这一本账
        """
        CREATE TABLE IF NOT EXISTS _outbox (
          id         TEXT PRIMARY KEY,
          kind       TEXT NOT NULL,
          ref_id     TEXT NOT NULL,
          payload    TEXT NOT NULL,
          created_at TEXT NOT NULL,
          tries      INTEGER NOT NULL DEFAULT 0,
          last_error TEXT
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_outbox_created ON _outbox(created_at)",

        // 零散状态：同步游标 / 设备号 / run-kit 缓存（键值即可，别为它建一张宽表）
        """
        CREATE TABLE IF NOT EXISTS mobile_kv (
          k          TEXT NOT NULL,
          v          TEXT NULL,
          updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (k)
        )
        """.trimIndent(),
    )

    /** 全部 DDL（按顺序执行，幂等：全部 `IF NOT EXISTS`）。 */
    val DDL: List<String> = MIRROR_DDL + LOCAL_DDL
}
