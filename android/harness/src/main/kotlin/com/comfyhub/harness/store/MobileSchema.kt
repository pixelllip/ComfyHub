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
 * - `ai_run_events`：本地 Run 的事件流（**R5**：切后台再回来靠它回放）—— **不进同步范围**
 *   （桌面也不带它：单 Run 上万条）；
 * - `local_runs`：**本机起过哪些 Run** 的标记（`ai_runs` 成了镜像表之后，崩溃恢复必须靠它
 *   把"桌面的 Run"排除掉，见 [HarnessStore.listUnfinishedRuns]）；
 * - `local_deletions`：**本机删除标记**（用户在这台手机上删过的会话及其级联子行）——
 *   下行 apply 要按它跳过，否则桌面那份会把删掉的会话**覆盖回来**（见
 *   [HarnessStore.deleteConversation] 与 [HarnessStore.applyMirrorRow]）；
 * - `_outbox`：上行队列（契约 §4），每条本地写**在同一个事务里**同时写业务表和它；
 * - `mobile_kv`：同步游标 / 设备号 / run-kit 缓存版本这类零散状态。
 *
 * ⚠ `ai_runs` / `ai_tool_calls` 属于**镜像表**那一组（2026-09-27 起）：桌面与手机都会写它们，
 *   下行靠镜像覆盖、上行靠 outbox（wire kind = `ai_run` / `ai_tool_call`）。
 */
object MobileSchema {

    /**
     * 手机侧本地库的结构版本 —— ⚠ **它必须与桌面的 `SCHEMA_VERSION` 同值**
     * （`server-rs/src/db.rs`，2026-11-09 起是 **15**）。
     *
     * ## 这个数代表什么（2026-09-27 拍板；理由写在这里，免得两边各说各话）
     *
     * 它在手机侧**只有一个用处**：与 `/api/sync/manifest` 回的 `schemaVersion` 比一比
     * （`com.comfyhub.harness.sync.SyncPullClient`）。那条判据要回答的是
     * "**桌面那份结构，我这套镜像列形状还接得住吗**" —— 而那件事**完全由桌面决定**
     * （桌面的 `user_version` 管表 / 列 / 触发器；手机的镜像 DDL 是从桌面 schema 抄来的子集）。
     *
     * 所以**不另立一个手机专属的号**：
     *  - 两个号就要一张映射表（"桌面 4 ↔ 手机 2"），还得有人记得同时改 —— 而忘了的后果
     *    **两个方向都是静默的**：要么每次 pull 都丢镜像重来（2026-09-27 之前正是如此：
     *    桌面已经 4、手机还写 1，于是**每一次同步都在丢镜像全量重来**），要么接下一串
     *    自己根本落不了库的增量；
     *  - "跟着桌面一起 +1"的代价只是这段注释要跟着改 —— 防线是 `HarnessDatabaseTest` 里那条
     *    **去读 `server-rs/src/db.rs` 现比**的用例：漏改一边当场变红。
     *
     * 顺带说清"加号"对手机侧意味着什么：**表结构与镜像列先照着桌面补齐**（`MobileSchema`
     * 的 DDL + `HarnessDatabase.migrateColumns` 的补列迁移），号对上之后走增量；
     * 只有真的对不上（桌面又加了我们还没跟上的列）才**丢镜像 + 走全量**
     * （`SyncPullClient.MODE_BOOTSTRAP`）。本地库自己的 `PRAGMA user_version` 是同一个数的
     * 记号（`HarnessDatabase.from` 幂等建完表之后写上它）。
     */
    const val VERSION = 15

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
        // 集合必须排在 media_assets 之前（后者用 collection_id 引用它；外键是开着的）
        MirrorTable("media_collections", listOf("id"), PkKind.SINGLE),
        MirrorTable("media_assets", listOf("id"), PkKind.SINGLE),
        MirrorTable("media_tags", listOf("media_id", "tag_id"), PkKind.COMPOSITE),
        MirrorTable("capture_runs", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_conversations", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_messages", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_message_parts", listOf("message_id", "ordinal"), PkKind.COMPOSITE),
        MirrorTable("ai_attachments", listOf("id"), PkKind.SINGLE),
        // ★ 2026-09-27（总纲 §7 待定项 1 = 进同步范围）：手机的 Run 与工具明细。
        //   桌面的 Run 要能整行落进来（手机上显示"桌面那次跑过哪些工具"），
        //   手机自己的 Run 也要能推回桌面 —— 所以它们是"两边都能写"的表。
        //   顺序：`ai_runs` 必须在 `ai_conversations` 之后（外键），
        //   `ai_tool_calls` 必须在 `ai_runs` 之后（外键）。
        MirrorTable("ai_runs", listOf("id"), PkKind.SINGLE),
        MirrorTable("ai_tool_calls", listOf("id"), PkKind.SINGLE),
    )

    /** 手机自己写、当期**不上行**的表（事件流与凭据引用，外加"哪些 Run 是我起的"标记）。 */
    val LOCAL_ONLY_TABLES: List<String> = listOf(
        "ai_providers", "ai_models", "ai_run_events", "local_runs", "local_deletions", "_outbox", "mobile_kv",
    )

    /** 会话表的表名（镜子里的父表；删除标记与级联跳过多处要用它）。 */
    const val CONVERSATIONS_TABLE = "ai_conversations"

    /**
     * 本机删除标记里用的[kind]就是**镜像表的表名**（不是 wire kind）。
     *
     * 为什么用表名而不是 `ai_conversation` 那种单数 wire kind：下行覆盖是**按表**做的
     * （[MIRROR_TABLES] 就是那张清单），判据与被判定的对象同一套词汇，才不会出现
     * "标记写在 `ai_conversation`、apply 查的是 `ai_conversations`"这种查不中的静默失效。
     */
    fun mirrorTable(name: String): MirrorTable? = MIRROR_TABLES.firstOrNull { it.name == name }

    /** 这张表是不是镜像表（下行 apply 的第一道校验：不是就不许落）。 */
    fun isMirrorTable(name: String): Boolean = mirrorTable(name) != null

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
          -- 2026-11-08：桌面 SCHEMA_VERSION 13 的列（位置照 db/schema.sqlite.sql）
          negative_mode     TEXT    NOT NULL DEFAULT 'keep',
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
        CREATE TABLE IF NOT EXISTS media_collections (
          id           INTEGER PRIMARY KEY AUTOINCREMENT,
          group_key    TEXT    NOT NULL,
          kind         TEXT    NOT NULL DEFAULT 'IMAGE',
          origin       TEXT    NOT NULL DEFAULT 'auto' CHECK (origin IN ('auto','manual')),
          rule_version INTEGER NOT NULL DEFAULT 1,
          title        TEXT    NULL,
          created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          updated_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_collections_key    ON media_collections(group_key)",
        "CREATE INDEX IF NOT EXISTS idx_collections_origin ON media_collections(origin)",

        """
        CREATE TABLE IF NOT EXISTS media_assets (
          id             INTEGER PRIMARY KEY AUTOINCREMENT,
          prompt_id      INTEGER NULL REFERENCES prompts(id) ON DELETE SET NULL,
          -- 2026-11-08：桌面 SCHEMA_VERSION 13 的列（位置照 db/schema.sqlite.sql）
          collection_id      INTEGER NULL REFERENCES media_collections(id) ON DELETE SET NULL,
          collection_opt_out INTEGER NOT NULL DEFAULT 0,
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
          source_kind TEXT    NOT NULL DEFAULT 'ComfyUI',
          recipe_id   INTEGER NULL,
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
          -- 2026-11-08：桌面 SCHEMA_VERSION 13 的列（位置照 db/schema.sqlite.sql）
          origin        TEXT    NOT NULL DEFAULT 'upload',
          metadata_json TEXT    NULL,
          created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (id)
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_ai_attachments_created ON ai_attachments(created_at)",

        // ★ `ai_runs` / `ai_tool_calls`：**既是本机写的，也是同步下来的**（2026-09-27 用户拍板
        //   总纲 §7 待定项 1 = 进同步范围）。所以它们的 DDL 放在**镜像表**这一组里：
        //   ① 下行：桌面的 Run（含它的工具过程）要用整行覆盖进来 → 列必须与桌面逐列同形；
        //   ② 上行：手机自己跑出来的 Run 要能推回桌面（wire kind = `ai_run`）。
        //   ⚠ 代价是"本地库里的 ai_runs 不再全是自己的" —— 崩溃恢复必须靠 `local_runs`
        //   那张本机表把"我起的那次"认出来（见 HarnessStore.listUnfinishedRuns），
        //   否则桌面的 Run 一同步下来就会被手机按 R6 标成 failed。
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
        CREATE TABLE IF NOT EXISTS ai_run_events (
          run_id       TEXT    NOT NULL REFERENCES ai_runs(id) ON DELETE CASCADE,
          seq          INTEGER NOT NULL,
          event_type   TEXT    NOT NULL,
          payload_json TEXT    NULL,
          created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (run_id, seq)
        )
        """.trimIndent(),

        // ★ **本机起过的 Run**（唯一一张"我写的"标记表）。
        //   `ai_runs` 现在是镜像表 —— 桌面的 Run 也会整行落进来。而崩溃恢复（R5/R6）要回答的
        //   是"**我**上次有一次没跑完的 Run"，不是"库里有几行 status=running"：
        //   拿后者当判据，桌面的 Run 一同步下来就会被手机标成 failed，而且推回桌面。
        //   与 `createRun` **同一个事务**写，`run_id` 只增不删（一次 Run 一行，很小）。
        """
        CREATE TABLE IF NOT EXISTS local_runs (
          run_id     TEXT NOT NULL,
          created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (run_id)
        )
        """.trimIndent(),

        // ★ **本机删除标记**（2026-09-27，M4 §25.1 第三条留下的那个形状）。
        //
        //   手机上会话的真源是**本地库**：用户主动删一条会话时，本机把行删掉了（级联带走消息 /
        //   块 / Run / 工具明细），可**桌面那份还在** —— 而且上行**只有 upsert**（R3，
        //   手机不许发明一个 wire 上的 delete op）。于是下一次下行 apply 会把桌面那份
        //   整行覆盖回来，"删掉的会话"当场**复活**，两端都不报错。
        //
        //   这张表就是那个标记：`kind` = 镜像表名（[MIRROR_TABLES] 的 `name`），
        //   `id` = 该表主键列的值（单列表；parts 那种复合主键靠它所属的 message 行来标）。
        //
        //   ⚠ 三条形态纪律：
        //    ① **不进镜像表**（[MIRROR_TABLES] 里没有它）、**不进同步白名单**（[LOCAL_ONLY_TABLES]）、
        //       **不挂触发器** —— 它是"这台手机的意图"，不是要同步给别人的数据；
        //    ② 判据**不参与 LWW**：桌面那份的 `updated_at` 永远比标记旧（手机删它时没改桌面那份），
        //       所以拿"谁更新"去比是没有意义的。用户在这台手机上删掉 = **终局**（见
        //       [HarnessStore.applyMirrorRow] 的注释）；
        //    ③ 标记**只增不删**（id 是 UUID，永不复用）：一次会话一行、几十字节，
        //       而"哪天把标记清了"这件事没有安全判据（桌面删了它？那就更不用复活了）。
        """
        CREATE TABLE IF NOT EXISTS local_deletions (
          kind       TEXT NOT NULL,
          id         TEXT NOT NULL,
          deleted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          PRIMARY KEY (kind, id)
        )
        """.trimIndent(),

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
