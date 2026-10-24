-- =====================================================================
--  ComfyHub - SQLite schema（P2，2026-09-23）
--
--  真源就是这一份：后端启动时由 `Migrate.run()` 从 classpath 读出并逐句执行
--  （构建时 `server/build.gradle.kts` 把 db/ 下的本文件打进 jar），
--  命令行手工建库也可以（sqlite3 comfyhub.db < db/schema.sqlite.sql）。
--
--  约定全部按 🔒 docs/sqlite-schema-contract.md：
--    · 时间列一律 TEXT ISO-8601 UTC（`...Z`，毫秒三位）——排序即字典序
--    · MySQL 的 `ON UPDATE CURRENT_TIMESTAMP(3)` → 触发器（SQLite 无此语法）
--    · 被同步的表**禁止** `INSERT OR REPLACE`（R3，会点亮 DELETE 触发器写假 delete）
--    · `_sync_log.seq` 必须 AUTOINCREMENT（R4，否则 rowid 复用会让客户端永久丢一段）
--    · 外键要生效必须每条连接 `PRAGMA foreign_keys = ON`（R1，见 Db.kt）
--
--  ⚠ 与 db/schema.sql（MySQL 版）的关系：那份是**历史迁移源**，新代码不要再按它改；
--    MySQL 只在"一次性数据搬迁"里当只读来源（docs/sync-api-contract.md §5 的搬迁工具）。
-- =====================================================================

-- ---------------------------------------------------------------------
-- 提示词主表
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS prompts (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  title             TEXT    NOT NULL DEFAULT '',
  kind              TEXT    NOT NULL DEFAULT 'IMAGE'
                    CHECK (kind IN ('IMAGE','VIDEO','AUDIO','MIXED')),
  positive_prompt   TEXT    NOT NULL,
  negative_prompt   TEXT    NULL,

  checkpoint        TEXT    NULL,
  loras             TEXT    NULL,   -- JSON 数组文本
  sampler           TEXT    NULL,
  scheduler         TEXT    NULL,
  steps             INTEGER NULL,
  cfg_scale         REAL    NULL,
  seed              INTEGER NULL,
  width             INTEGER NULL,
  height            INTEGER NULL,
  batch_size        INTEGER NULL,
  extra_params      TEXT    NULL,   -- JSON 对象文本

  notes             TEXT    NULL,
  -- 发送模式（2026-10-11）：有些 turbo 模型不认负面提示词，用户要能表达"我只发正面"。
  --   'keep'（默认）= 照原样带上负面提示词；'positive_only' = 发给 Comfy 前**去掉**负面。
  -- ⚠ 这是**预设的属性**（写提示词时的意图），与"目标工作流认不认负面"是两件事 ——
  -- 后者是工作流的属性，兜底检查在提交那条路上（`comfy_submit`）。
  negative_mode     TEXT    NOT NULL DEFAULT 'keep',
  favorite          INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0,1)),

  source            TEXT    NULL DEFAULT 'Manual',
  source_ref        TEXT    NULL,
  workflow_json     TEXT    NULL,

  created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_prompts_kind       ON prompts(kind);
CREATE INDEX IF NOT EXISTS idx_prompts_created    ON prompts(created_at);
CREATE INDEX IF NOT EXISTS idx_prompts_favorite   ON prompts(favorite);
CREATE INDEX IF NOT EXISTS idx_prompts_source_ref ON prompts(source_ref);

-- ---------------------------------------------------------------------
-- 标签表（全局词表）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tags (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT    NOT NULL,
  normalized  TEXT    NOT NULL,
  category    TEXT    NULL,
  color       TEXT    NULL,
  description TEXT    NULL,
  use_count   INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS uk_tags_normalized ON tags(normalized);
CREATE INDEX IF NOT EXISTS idx_tags_category  ON tags(category);
CREATE INDEX IF NOT EXISTS idx_tags_use_count ON tags(use_count);

-- ---------------------------------------------------------------------
-- 提示词 <-> 标签
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS prompt_tags (
  prompt_id INTEGER NOT NULL REFERENCES prompts(id) ON DELETE CASCADE,
  tag_id    INTEGER NOT NULL REFERENCES tags(id)    ON DELETE CASCADE,
  PRIMARY KEY (prompt_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_pt_tag ON prompt_tags(tag_id);

-- ---------------------------------------------------------------------
-- 产物「集合」：同源、只做了一点微调的产物归到一组
--
-- 判定真源是 `server-rs/src/media/collection.rs` 的 `group_key`（**纯函数**，入库与
-- 存量回填共用同一份实现）。这里只存"算出来的结果"，规则本身一个字都不进库。
--
--   group_key     = kind ⊗ 归一化正提示词 ⊗ 归一化负提示词（含长度前缀，见那个文件）
--   origin        = auto（算法分出来的）| manual（用户拆开/合并出来的）
--   rule_version  = 产出这条集合时的判定规则版本；**规则改了要把老的重新分一遍**
--
-- ⚠ 两张表都要有：`media_collections` 本身也要同步（否则手机上折叠不出集合）。
-- ⚠ 这张表**必须建在 `media_assets` 之前** —— 后者用 REFERENCES 引它。
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS media_collections (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  group_key    TEXT    NOT NULL,
  kind         TEXT    NOT NULL DEFAULT 'IMAGE',
  origin       TEXT    NOT NULL DEFAULT 'auto' CHECK (origin IN ('auto','manual')),
  rule_version INTEGER NOT NULL DEFAULT 1,
  -- 2026-10-18：用户给**手动集合**起的名字。算法分出来的集合没有名字（NULL），
  -- 界面上按"算法分组"标记显示 —— 两种集合本来就不是一种东西。
  title        TEXT    NULL,
  created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_collections_key    ON media_collections(group_key);
CREATE INDEX IF NOT EXISTS idx_collections_origin ON media_collections(origin);

-- ---------------------------------------------------------------------
-- 生成产物（图片 / 视频 / 音频）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS media_assets (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  prompt_id      INTEGER NULL REFERENCES prompts(id) ON DELETE SET NULL,
  -- 所属「集合」（同源、只做了一点微调的产物归到一组；判定见 media/collection.rs）
  -- ON DELETE SET NULL：集合行被清掉时产物**不能跟着消失**（集合是视图性质的分组）
  collection_id  INTEGER NULL REFERENCES media_collections(id) ON DELETE SET NULL,
  -- 2026-10-18：用户**明确把这一条移出集合**了（"我碰过它，它归我"）。
  --
  -- ⚠ 这一列是**正确性的前提**，不是装饰：`backfill` 只处理 `collection_id IS NULL`
  -- 的行，而"移出集合"恰恰就是把它置成 NULL —— 少了它，用户刚拖出去的那张会在
  -- 下次启动被回填**静默吸回原组**（`media/collection.rs` 那句"手动拆开过的一律不碰"
  -- 在没有这一列时只是一句空承诺）。回填的 WHERE 必须带 `AND collection_opt_out = 0`。
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
  -- NSFW 标记（只影响预览图打不打码；默认 0 = 没标记 = 不打码）
  nsfw           INTEGER NOT NULL DEFAULT 0 CHECK (nsfw IN (0,1)),
  created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_media_prompt     ON media_assets(prompt_id);
CREATE INDEX IF NOT EXISTS idx_media_kind       ON media_assets(kind);
CREATE INDEX IF NOT EXISTS idx_media_created    ON media_assets(created_at);
CREATE INDEX IF NOT EXISTS idx_media_sha256     ON media_assets(sha256);
CREATE INDEX IF NOT EXISTS idx_media_source_ref ON media_assets(source_ref);
CREATE INDEX IF NOT EXISTS idx_media_collection ON media_assets(collection_id);

-- ---------------------------------------------------------------------
-- 生成产物 <-> 标签
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS media_tags (
  media_id INTEGER NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
  tag_id   INTEGER NOT NULL REFERENCES tags(id)         ON DELETE CASCADE,
  PRIMARY KEY (media_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_mt_tag ON media_tags(tag_id);

-- ---------------------------------------------------------------------
-- 自动捕获：每一次 ComfyUI 运行的记录（run_key 唯一 => 同一次生成只入库一次）
-- ---------------------------------------------------------------------
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
);
CREATE UNIQUE INDEX IF NOT EXISTS uk_capture_runs_key ON capture_runs(run_key);
CREATE INDEX IF NOT EXISTS idx_capture_runs_created ON capture_runs(created_at);
CREATE INDEX IF NOT EXISTS idx_capture_runs_prompt  ON capture_runs(prompt_id);

-- ---------------------------------------------------------------------
-- 运行期设置（自动捕获开关 / ComfyUI 地址 / 输出目录等，App 与后端共用）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_settings (
  k          TEXT NOT NULL,
  v          TEXT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (k)
);

-- ---------------------------------------------------------------------
-- AI 工作台：Provider 与模型目录
-- 凭据值不进库：只有 credential_ref 这个名字，值由 DPAPI 加密单独保管
-- ---------------------------------------------------------------------
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
);

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
);

-- ---------------------------------------------------------------------
-- AI 工作台：会话 / 消息 / 有序消息块
-- ---------------------------------------------------------------------
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
);
CREATE INDEX IF NOT EXISTS idx_ai_conv_updated  ON ai_conversations(updated_at);
CREATE INDEX IF NOT EXISTS idx_ai_conv_archived ON ai_conversations(archived, updated_at);

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
);
CREATE INDEX IF NOT EXISTS idx_ai_msg_conv ON ai_messages(conversation_id, seq);

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
);

-- ---------------------------------------------------------------------
-- AI 附件：刻意**不**对 ai_message_parts.attachment_id 加外键
-- （附件是用户可删的临时对象，删掉之后消息块还要能如实显示"这个附件已经不在了"）
-- ---------------------------------------------------------------------
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
);
CREATE INDEX IF NOT EXISTS idx_ai_attachments_created ON ai_attachments(created_at);

-- ---------------------------------------------------------------------
-- AI Run 与事件
-- ---------------------------------------------------------------------
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
);
CREATE INDEX IF NOT EXISTS idx_ai_runs_conv   ON ai_runs(conversation_id, created_at);
CREATE INDEX IF NOT EXISTS idx_ai_runs_status ON ai_runs(status);

CREATE TABLE IF NOT EXISTS ai_run_events (
  run_id       TEXT    NOT NULL REFERENCES ai_runs(id) ON DELETE CASCADE,
  seq          INTEGER NOT NULL,
  event_type   TEXT    NOT NULL,
  payload_json TEXT    NULL,
  created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (run_id, seq)
);

-- ---------------------------------------------------------------------
-- AI 工具调用：审批状态与执行状态分开记（被用户拒绝的调用也必须留痕）
-- ---------------------------------------------------------------------
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
);
CREATE INDEX IF NOT EXISTS idx_ai_tool_run  ON ai_tool_calls(run_id, started_at);
CREATE INDEX IF NOT EXISTS idx_ai_tool_name ON ai_tool_calls(name);

-- ---------------------------------------------------------------------
-- 提问闸门落库（M1）
-- 状态机：pending → answered / skipped / expired
-- ⚠ 这张表**不进同步范围**（手机看实时提问走 GET /api/ai/pending）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ai_questions (
  question_id     TEXT    NOT NULL,
  run_id          TEXT    NOT NULL REFERENCES ai_runs(id) ON DELETE CASCADE,
  conversation_id TEXT    NOT NULL REFERENCES ai_conversations(id) ON DELETE CASCADE,
  group_id        TEXT    NOT NULL,
  card_index      INTEGER NOT NULL DEFAULT 0,
  total           INTEGER NOT NULL DEFAULT 1,
  question        TEXT    NOT NULL,
  options_json    TEXT    NULL,
  allow_free_text INTEGER NOT NULL DEFAULT 1 CHECK (allow_free_text IN (0,1)),
  multi_select    INTEGER NOT NULL DEFAULT 0 CHECK (multi_select IN (0,1)),
  header          TEXT    NULL,
  status          TEXT    NOT NULL DEFAULT 'pending',
  answer          TEXT    NULL,
  option_index    INTEGER NULL,
  option_indexes  TEXT    NULL,
  asked_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  answered_at     TEXT    NULL,
  PRIMARY KEY (question_id)
);
CREATE INDEX IF NOT EXISTS idx_ai_questions_run    ON ai_questions(run_id);
CREATE INDEX IF NOT EXISTS idx_ai_questions_status ON ai_questions(status);

-- ---------------------------------------------------------------------
-- 角色卡（O8，2026-09-25）：一张卡 = 一个角色，**只有三个字段**：
-- **名称 / 正面提示词 / 负面提示词**（2026-10 按用户要求收敛，见下面那条 ⚠）。
--
-- 2026-10-24 用户再次收敛：**正面提示词只留自然语言一版**（tag 版整列删除）——
-- 一句话描述比 tag 栈更耐改（换底模 / 换 tag 体系都不用重写），要 tag 版时按用户
-- 要求当场再翻一版即可。老库里 `tag_prompt` 的内容由 `infra::db` 的迁移转成自然语言
-- 灌进 `prompt`（只填空卡；已有自然语言版的一个字节都不动）。
--
-- 为什么是一张表而不是长期记忆：记忆是"一行一条文本"、有 100 条上限、超限**直接报错**
-- （MemoryStore），而角色卡要按名字幂等地反复覆盖（同名 = 同一张卡，不许攒出三四份）。
--
-- `normalized` = name 的小写，是幂等 upsert 的键（照 tags.normalized 的写法）。
-- `prompt` / `negative` 都是**整段文本**（不是 tag 数组、不拼 SQL）。
--
-- ⚠ 2026-10 收敛（用户原话："角色卡只保留：名称、正面提示词、负面提示词"）：
--   删掉了两批东西 ——
--     ① 资料库那批：`name_en` / `aliases` / `character_tag` / `series_tag` / `tags` /
--        `lora_name` / `lora_weight` / `lora_trigger` / `checkpoint` / `reference_media_id` /
--        `source` / `tags_verified_at` / `notes`；
--     ② "生图角色卡框架"那两件：`position`（5×5 位置）/ `enabled`（最多同时启用 6 个 + token 口径），
--        `GET /api/ai/character-cards/summary` 与 `MAX_ENABLED` 一起删。
--   ⚠ **老库里的那些列不去动**（SQLite 删列要重建表，不值当）：它们只是**再没人读**，
--     新库建出来就没有。`tags` 里已有的内容由 `infra::db` 的迁移**拼成一行**灌进 `prompt`
--     （不丢用户数据），随后那两个列同样是死列。
--
-- ⚠ **不进同步范围**（总纲 §3.3 的白名单里没有它）：手机侧目前没有角色卡的读写界面。
--   要同步时 schema 的 3 条 `_sync_log` 触发器、`server-rs/src/sync.rs` 的 `SYNCED` /
--   `pk_kind` / `PHONE_FORBIDDEN`、`MobileSchema.kt` 的两份清单、两份契约文档**要一起改**。
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS character_cards (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  name               TEXT    NOT NULL,          -- 用户叫的名字（中文 / 原名）
  normalized         TEXT    NOT NULL,          -- name 的小写（幂等 upsert 的键）
  prompt             TEXT    NULL,              -- 正面提示词（整段自然语言）
  negative           TEXT    NULL,              -- 负面提示词（整段文本）
  created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
-- 幂等 upsert 的键：同名 = 同一张卡（`ON CONFLICT(normalized) DO UPDATE`，R3 不许用 OR REPLACE）
CREATE UNIQUE INDEX IF NOT EXISTS uk_character_cards_norm ON character_cards(normalized);
CREATE INDEX IF NOT EXISTS idx_character_cards_created  ON character_cards(created_at);

-- ---------------------------------------------------------------------
-- 画风卡（2026-10-21）：与角色卡**同等地位**，一个画风的标准容器。
--
-- 字段与 `character_cards` 逐列同形（名称 / 正面提示词 / 负面提示词）：
-- 角色卡管"画的是谁"，画风卡管"画成什么样"（画师 / 媒介 / 上色 / 光线 / 质感）。
-- 两张表刻意不合并：同名角色与同名画风是两条独立的知识，不能互相覆盖。
-- 同样**不进同步范围**（理由与 character_cards 那节相同）。
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS style_cards (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  name               TEXT    NOT NULL,          -- 用户叫的画风名
  normalized         TEXT    NOT NULL,          -- name 的小写（幂等 upsert 的键）
  prompt             TEXT    NULL,              -- 正面提示词（整段自然语言，100~200 token）
  negative           TEXT    NULL,              -- 负面提示词（整段文本）
  created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS uk_style_cards_norm ON style_cards(normalized);
CREATE INDEX IF NOT EXISTS idx_style_cards_created  ON style_cards(created_at);

-- 卡片示例产物：一张卡可以挂若干条画廊产物，作为"这套设定 / 这个画风长什么样"的样本。
--
-- 两张关联表刻意分开（不是一张 `card_examples(kind, card_id, media_id)`）：
-- 外键要能 `ON DELETE CASCADE`，而单表的多态 `card_id` 没有一张表可以指向。
-- `PRIMARY KEY (card_id, media_id)` = 同一张卡里同一条产物只出现一次；
-- 顺序按加入顺序（`rowid`），界面与 AI 看到的顺序一致。
CREATE TABLE IF NOT EXISTS character_card_examples (
  card_id     INTEGER NOT NULL REFERENCES character_cards(id) ON DELETE CASCADE,
  media_id    INTEGER NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
  added_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (card_id, media_id)
);
CREATE INDEX IF NOT EXISTS idx_character_card_examples_media
  ON character_card_examples(media_id);

CREATE TABLE IF NOT EXISTS style_card_examples (
  card_id     INTEGER NOT NULL REFERENCES style_cards(id) ON DELETE CASCADE,
  media_id    INTEGER NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
  added_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (card_id, media_id)
);
CREATE INDEX IF NOT EXISTS idx_style_card_examples_media
  ON style_card_examples(media_id);

-- ---------------------------------------------------------------------
-- 工作流配方（W2，2026-09-26）：把一份**具体**工作流归纳成
-- 「任务 + 骨架指纹 + 模型槽位 + 参数旋钮」，让"新模型该接哪条骨架"不必每次重新推。
-- 设计与判据在 docs/workflow-recipe-design.md，解析真源是 WorkflowRecipe.analyze（纯函数）。
--
-- `source_ref` 是幂等 upsert 的键，三种来源各有前缀：
--   `file:<sha256>`（本机工作流文件，与 comfy_load_workflow 的 run_key 同一把钥匙）、
--   `prompt:<id>`（库里已有 API 图的提示词）、`graph:<sha256>`（直接给的节点图）。
-- `skeleton_hash` 是**泛用**的键：同一族工作流（换底模 / 换节点编号 / 换参数）指纹相同，
--   `comfy_list_recipes` 按它分组。配方规则一改指纹就该变 —— 规则版本进哈希（WorkflowRecipe.VERSION）。
-- `complete` = 这张图**没有**转换缺口（没被摘掉节点、没有悬空输入）。
--   为 0 的配方照样保留，但读的时候必须如实说"骨架缺件"，别拿它当完整骨架用
--   （现场：当前跑着的实例里没有 Krea2 那几个自定义节点，转换会把整条编辑链摘掉而不报错）。
-- `prompt_id` 指回 `prompts`：**ON DELETE SET NULL** —— 用户删掉那条提示词，配方还在
--   （配方是"这一族长什么样"的知识，不依赖某一条具体记录）。
--
-- ⚠ **不进同步范围**（总纲 §3.3 的白名单里没有它）：手机提交走 `comfy_queue_submit`（R11），
--   拿到一份它自己跑不了的配方只会误导模型。要同步时与 character_cards 一样，
--   schema 的 `_sync_log` 触发器、`server-rs/src/sync.rs` 的 `SYNCED` / `pk_kind`、
--   `MobileSchema.kt` 的两份清单、两份契约文档**要一起改**。
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS workflow_recipes (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  name            TEXT    NOT NULL,          -- 给用户/模型看的名字（文件名或提示词标题）
  task            TEXT    NOT NULL,          -- t2i / i2i / edit / inpaint / upscale / video / audio / unknown
  skeleton_hash   TEXT    NOT NULL,          -- 骨架指纹（同一族 = 同一个值）
  recipe_version  INTEGER NOT NULL DEFAULT 1,-- WorkflowRecipe.VERSION（解析规则版本）
  source_kind     TEXT    NOT NULL DEFAULT 'file',  -- file / prompt / graph
  source_ref      TEXT    NULL,              -- 幂等键（见文件头三种前缀）
  source_path     TEXT    NULL,              -- 本机文件路径（只有 file 来源有）
  prompt_id       INTEGER NULL,              -- 能提交的那条提示词（有就是指向 prompts.id）
  node_count      INTEGER NOT NULL DEFAULT 0,
  class_count     INTEGER NOT NULL DEFAULT 0,
  image_inputs    INTEGER NOT NULL DEFAULT 0,
  has_mask        INTEGER NOT NULL DEFAULT 0 CHECK (has_mask IN (0,1)),
  complete        INTEGER NOT NULL DEFAULT 1 CHECK (complete IN (0,1)),
  slots_json      TEXT    NOT NULL DEFAULT '[]',  -- 模型槽位（角色 / 覆盖键 / 文件名）
  knobs_json      TEXT    NOT NULL DEFAULT '[]',  -- 可覆盖的旋钮
  classes_json    TEXT    NOT NULL DEFAULT '[]',  -- 执行相关节点类（带重数）
  edges_json      TEXT    NOT NULL DEFAULT '[]',  -- 类级连线（进指纹）
  outputs_json    TEXT    NOT NULL DEFAULT '[]',  -- 输出节点
  warnings_json   TEXT    NOT NULL DEFAULT '[]',
  gaps_json       TEXT    NOT NULL DEFAULT '[]',  -- 转换缺口（摘掉的节点类 / 悬空输入）
  notes           TEXT    NULL,
  created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  FOREIGN KEY (prompt_id) REFERENCES prompts(id) ON DELETE SET NULL
);
-- 幂等 upsert 的键：同一份来源 = 同一条配方（`ON CONFLICT(source_ref) DO UPDATE`，R3 不许用 OR REPLACE）。
--   部分索引（`WHERE source_ref IS NOT NULL`）：SQLite 的唯一索引允许多个 NULL，
--   但把"没有来源"的那些也纳进来只会白白多一个索引项。
CREATE UNIQUE INDEX IF NOT EXISTS uk_workflow_recipes_source ON workflow_recipes(source_ref)
  WHERE source_ref IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_workflow_recipes_skeleton ON workflow_recipes(skeleton_hash);
CREATE INDEX IF NOT EXISTS idx_workflow_recipes_task     ON workflow_recipes(task);
CREATE INDEX IF NOT EXISTS idx_workflow_recipes_created  ON workflow_recipes(created_at);

-- ---------------------------------------------------------------------
-- M5 手机排队提交（`kind: "submit_job"`，契约 §4.1）—— **一张指令的落点，不是表行的镜像**
--
-- 手机「点一下提交」推上来的是一条**指令**（`sync.rs` 的 push 里单独一个分支，
-- **不走 `table_of_kind`**）。这张表就是那条指令的**持久账**：受理过没有、审批走到哪、
-- 提交成没成、回执发给手机那条会话没有。
--
-- 三条设计约束（改这张表之前先读 `server-rs/src/submit_queue.rs` 的文件头）：
--
--  ① **`queue_id` 是幂等键**（手机生成）。同一个 `queueId` 再推一次 → 回 `applied`
--     且**绝不重复提交**（手机重试是常态）。所以它是主键、插入用 `DO NOTHING`。
--  ② **不挂 `_sync_log` 触发器、不进 `SYNCED`**：它不是给手机镜像的行
--     （手机知道自己在等什么），结果只经"往发起那条会话追加一条助手消息"回传。
--  ③ **`conversation_id` / `run_id` 故意不加外键**：手机那条会话（或它的 Run）
--     可能**还没上行到桌面**（契约没保证顺序）。加外键会让"排队"直接失败，
--     而正确处置是"受理、如实记回执失败、等它同步上来再补发"
--     （`reply_state='blocked'` + 投递循环重试）。
--
-- `state` 状态机：awaiting_approval → submitting → done / failed；awaiting_approval → denied。
--   **没有 approved 这个状态**：人点头落在 `approval` 列，由后台任务原子地
--   `awaiting_approval + approval='approved' → submitting`（那种"两步状态"会多一个
--   "批了但没人认领"的窗口，而它正是 R8 顺序要防的东西）。
-- `approval` 与桌面审批闸门同语义：pending / approved / denied（**超时也写 denied**，
--   与 `ToolApprovalGate` 一致），所以 `GET /api/ai/pending` 上不会留一张点了没用的卡。
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS submit_jobs (
  queue_id        TEXT    NOT NULL,          -- 手机生成的 queueId（幂等键）
  call_id         TEXT    NOT NULL,          -- 审批闸门的调用号（`submit:<queueId>`，与 ai_tool_calls 不撞）
  conversation_id TEXT    NOT NULL,          -- 回执要追加到哪条会话（**不加外键**，见上 ③）
  run_id          TEXT    NULL,              -- 手机那次 Run（如实存，不加外键）
  payload_json    TEXT    NOT NULL,          -- 推上来的原始 payload（排查用，不改它）
  prompt_id       INTEGER NULL,              -- 受理时就解析好：payload.promptId 或 sourceRef 查到的那条
  source_ref      TEXT    NULL,              -- `file:<sha256>` 那种来源（受理时解析成 prompt_id）
  overrides_json  TEXT    NULL,              -- 参数覆盖（原样透给 comfy_submit）
  nsfw            INTEGER NOT NULL DEFAULT 0 CHECK (nsfw IN (0,1)),
  state           TEXT    NOT NULL,          -- awaiting_approval / submitting / done / failed / denied
  approval        TEXT    NOT NULL DEFAULT 'pending',  -- pending / approved / denied
  ok              INTEGER NULL CHECK (ok IS NULL OR ok IN (0,1)),  -- 终态的成败（审批中为 NULL）
  outcome_code    TEXT    NULL,              -- 终态稳定码（成功为 NULL；失败/被拒必填）
  outcome_message TEXT    NULL,              -- 终态人话（回执正文用它）
  media_ids_json  TEXT    NULL,              -- 入库产物的 mediaIds（**只有真的入库了才有**）
  comfy_prompt_id TEXT    NULL,              -- ComfyUI 的 prompt_id（提交成功后记下，排查/对账用）
  elapsed_ms      INTEGER NOT NULL DEFAULT 0,-- 受理到终态（或审批等了多久）
  reply_state     TEXT    NOT NULL DEFAULT 'pending',  -- pending / blocked / delivered（回执投递的账）
  reply_error     TEXT    NULL,              -- 回执没送出去的原因（会话还没同步上来等）
  reply_message_id TEXT   NULL,              -- 送出去的那条助手消息（幂等：同一个 id 不重发）
  requested_at    TEXT    NULL,              -- payload.requestedAt（手机的排队时刻）
  requested_by    TEXT    NULL,              -- payload.requestedBy（哪台手机）
  heartbeat_at    TEXT    NULL,              -- 后台任务的租约心跳（看门狗判"这个 worker 死了没有"）
  created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (queue_id)
);
-- 审批列表 / 投递循环 / 看门狗都按状态扫，索引跟着它们走
CREATE INDEX IF NOT EXISTS idx_submit_jobs_state   ON submit_jobs(state, created_at);
CREATE INDEX IF NOT EXISTS idx_submit_jobs_reply   ON submit_jobs(reply_state, updated_at);
CREATE INDEX IF NOT EXISTS idx_submit_jobs_call    ON submit_jobs(call_id);

-- =====================================================================
--  同步账本（🔒 sqlite-schema-contract.md §3）—— 桌面是唯一真源
-- =====================================================================

-- 变更账本：每张被同步的表挂 3 个触发器往这里写一行
CREATE TABLE IF NOT EXISTS _sync_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,   -- ★ R4：必须 AUTOINCREMENT
  tbl TEXT NOT NULL,
  pk  TEXT NOT NULL,
  op  TEXT NOT NULL CHECK (op IN ('upsert','delete')),
  at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sync_log_tbl ON _sync_log(tbl);

-- 每个设备的游标（日志裁剪：DELETE FROM _sync_log WHERE seq <= MIN(cursor)）
CREATE TABLE IF NOT EXISTS sync_clients (
  device_id TEXT PRIMARY KEY,
  cursor    INTEGER NOT NULL DEFAULT 0,
  last_seen TEXT
);

-- ---------------------------------------------------------------------
-- updated_at 维护（替掉 MySQL 的 ON UPDATE CURRENT_TIMESTAMP(3)）
--   WHEN 里的 `NEW.updated_at IS OLD.updated_at`：只有"这次 UPDATE 没自己动
--   updated_at"时才补一下 —— 应用显式改了时间就不覆盖它（AiConversationRepo
--   发消息那条就是显式改的）。
-- ---------------------------------------------------------------------
CREATE TRIGGER IF NOT EXISTS trg_prompts_touch AFTER UPDATE ON prompts FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE prompts SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_media_assets_touch AFTER UPDATE ON media_assets FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE media_assets SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_media_collections_touch AFTER UPDATE ON media_collections FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE media_collections SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_app_settings_touch AFTER UPDATE ON app_settings FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE app_settings SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE k = NEW.k;
END;

CREATE TRIGGER IF NOT EXISTS trg_ai_providers_touch AFTER UPDATE ON ai_providers FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE ai_providers SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_ai_models_touch AFTER UPDATE ON ai_models FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE ai_models SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
   WHERE provider_id = NEW.provider_id AND model_id = NEW.model_id;
END;

CREATE TRIGGER IF NOT EXISTS trg_ai_conversations_touch AFTER UPDATE ON ai_conversations FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE ai_conversations SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_ai_messages_touch AFTER UPDATE ON ai_messages FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE ai_messages SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_ai_runs_touch AFTER UPDATE ON ai_runs FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE ai_runs SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_character_cards_touch AFTER UPDATE ON character_cards FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE character_cards SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_style_cards_touch AFTER UPDATE ON style_cards FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE style_cards SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_workflow_recipes_touch AFTER UPDATE ON workflow_recipes FOR EACH ROW
WHEN NEW.updated_at IS OLD.updated_at
BEGIN
  UPDATE workflow_recipes SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

-- ---------------------------------------------------------------------
-- _sync_log 触发器（只挂**同步范围内**的表，🔒 sqlite-schema-contract.md §6）
--   ⚠ 同步范围的真源就是这一节：`/api/sync/{bootstrap,changes}` 也从它派生。
--   不同步：ai_run_events（62 万行调试日志）、ai_providers / ai_models /
--   app_settings（含凭据引用与令牌明文）、ai_runs / ai_tool_calls / ai_questions /
--   character_cards / style_cards / workflow_recipes（桌面侧的知识，手机提交走 comfy_queue_submit）。
--
--   pk 用的都是**各表自己的稳定标识**（INTEGER 主键转文本；两处复合主键用
--   `a:b` 拼，块用 `message#ordinal`），手机侧按同一约定删本地投影行。
-- ---------------------------------------------------------------------

-- prompts
CREATE TRIGGER IF NOT EXISTS trg_prompts_sync_ai AFTER INSERT ON prompts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('prompts', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_prompts_sync_au AFTER UPDATE ON prompts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('prompts', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_prompts_sync_ad AFTER DELETE ON prompts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('prompts', CAST(OLD.id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- tags
CREATE TRIGGER IF NOT EXISTS trg_tags_sync_ai AFTER INSERT ON tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('tags', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_tags_sync_au AFTER UPDATE ON tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('tags', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_tags_sync_ad AFTER DELETE ON tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('tags', CAST(OLD.id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- prompt_tags（复合主键）
CREATE TRIGGER IF NOT EXISTS trg_prompt_tags_sync_ai AFTER INSERT ON prompt_tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('prompt_tags', CAST(NEW.prompt_id AS TEXT)||':'||CAST(NEW.tag_id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_prompt_tags_sync_ad AFTER DELETE ON prompt_tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('prompt_tags', CAST(OLD.prompt_id AS TEXT)||':'||CAST(OLD.tag_id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- media_assets
CREATE TRIGGER IF NOT EXISTS trg_media_assets_sync_ai AFTER INSERT ON media_assets FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_assets', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_media_assets_sync_au AFTER UPDATE ON media_assets FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_assets', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_media_assets_sync_ad AFTER DELETE ON media_assets FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_assets', CAST(OLD.id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- media_collections（pk 用自增 id；产物靠 `collection_id` 指过来）
CREATE TRIGGER IF NOT EXISTS trg_media_collections_sync_ai AFTER INSERT ON media_collections FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_collections', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_media_collections_sync_au AFTER UPDATE ON media_collections FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_collections', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_media_collections_sync_ad AFTER DELETE ON media_collections FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_collections', CAST(OLD.id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- media_tags（复合主键）
CREATE TRIGGER IF NOT EXISTS trg_media_tags_sync_ai AFTER INSERT ON media_tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_tags', CAST(NEW.media_id AS TEXT)||':'||CAST(NEW.tag_id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_media_tags_sync_ad AFTER DELETE ON media_tags FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('media_tags', CAST(OLD.media_id AS TEXT)||':'||CAST(OLD.tag_id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- capture_runs（pk 用自增 id；run_key 是业务唯一键，手机侧按 id 删）
CREATE TRIGGER IF NOT EXISTS trg_capture_runs_sync_ai AFTER INSERT ON capture_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('capture_runs', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_capture_runs_sync_au AFTER UPDATE ON capture_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('capture_runs', CAST(NEW.id AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_capture_runs_sync_ad AFTER DELETE ON capture_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('capture_runs', CAST(OLD.id AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- ai_conversations
CREATE TRIGGER IF NOT EXISTS trg_ai_conversations_sync_ai AFTER INSERT ON ai_conversations FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_conversations', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_conversations_sync_au AFTER UPDATE ON ai_conversations FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_conversations', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_conversations_sync_ad AFTER DELETE ON ai_conversations FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_conversations', OLD.id, 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- ai_messages
CREATE TRIGGER IF NOT EXISTS trg_ai_messages_sync_ai AFTER INSERT ON ai_messages FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_messages', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_messages_sync_au AFTER UPDATE ON ai_messages FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_messages', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_messages_sync_ad AFTER DELETE ON ai_messages FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_messages', OLD.id, 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- ai_message_parts（复合主键：message_id#ordinal）
CREATE TRIGGER IF NOT EXISTS trg_ai_message_parts_sync_ai AFTER INSERT ON ai_message_parts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_message_parts', NEW.message_id||'#'||CAST(NEW.ordinal AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_message_parts_sync_au AFTER UPDATE ON ai_message_parts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_message_parts', NEW.message_id||'#'||CAST(NEW.ordinal AS TEXT), 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_message_parts_sync_ad AFTER DELETE ON ai_message_parts FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_message_parts', OLD.message_id||'#'||CAST(OLD.ordinal AS TEXT), 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- ai_attachments
CREATE TRIGGER IF NOT EXISTS trg_ai_attachments_sync_ai AFTER INSERT ON ai_attachments FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_attachments', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_attachments_sync_au AFTER UPDATE ON ai_attachments FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_attachments', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_attachments_sync_ad AFTER DELETE ON ai_attachments FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_attachments', OLD.id, 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- ai_runs / ai_tool_calls（2026-09-27 用户拍板：总纲 §7 待定项 1 = **进同步范围**）
--
-- 为什么要它们：手机要能显示桌面那次 Run 的**工具过程**（跑过哪些工具、成没成、花了多久）。
-- 那件事只有这两张表有；`ai_run_events`（62 万行调试日志）照旧不同步。
--
-- ⚠ 与 `server-rs/src/sync.rs` 的 `SYNCED` / `pk_kind()` **必须一致**（表名 / 主键形态）：
--   两张表都是 TEXT 主键（UUID），所以 pk 直接写列值、不加 CAST。
-- ⚠ ai_runs 上还有 `trg_ai_runs_touch`（维护 updated_at）—— 一次更新会点亮两条账本行
--   （touch 那次 UPDATE 又触发一次 sync_au）。幂等 upsert 无害，**别去"优化"它**：
--   删掉 sync 触发器才是真错（手机永远收不到这次状态变化）。
CREATE TRIGGER IF NOT EXISTS trg_ai_runs_sync_ai AFTER INSERT ON ai_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_runs', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_runs_sync_au AFTER UPDATE ON ai_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_runs', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_runs_sync_ad AFTER DELETE ON ai_runs FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_runs', OLD.id, 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

CREATE TRIGGER IF NOT EXISTS trg_ai_tool_calls_sync_ai AFTER INSERT ON ai_tool_calls FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_tool_calls', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_tool_calls_sync_au AFTER UPDATE ON ai_tool_calls FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_tool_calls', NEW.id, 'upsert', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER IF NOT EXISTS trg_ai_tool_calls_sync_ad AFTER DELETE ON ai_tool_calls FOR EACH ROW
BEGIN INSERT INTO _sync_log(tbl,pk,op,at) VALUES ('ai_tool_calls', OLD.id, 'delete', strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;

-- schema 版本（`PRAGMA user_version` 是 SQLite 自带的，不建版本表）
--
-- ⚠ 这里与 `server-rs/src/db.rs` 的 `SCHEMA_VERSION` **必须一致**（有用例
--   `ddl_declared_version_matches_the_constant` 钉着 —— 两份漂了就是一整套静默故障）。
--   Rust 侧写版本号时取的是 `max(库里的值, SCHEMA_VERSION)`：既不许把更新的库降级，
--   也不会被这一行拖回去。**加表 / 改列就把两边一起 +1**。
--
-- 2026-09-27 由 2 提到 3：**不是为了加表，是为了让"新触发器"到达已有的库**。
--   `open_or_create` 对"版本号够新、表也都在"的老库是**直接早退**的（不重跑 DDL），
--   所以只往这里加 `_sync_log` 触发器，真库上一条都不会有 —— 表现是"AI 的工具过程
--   手机上永远收不到"，而且不报任何错。bump 版本号让老库走一遍收敛式 DDL
--   （触发器 `DROP + CREATE`），顺带把 `manifest.schemaVersion` 变了，
--   手机据此重新 bootstrap —— 这正是"同步范围变了"该有的信号。
--
-- 2026-09-27 由 3 提到 4（M5）：**加了一张表 `submit_jobs`**。
--   它是"手机排队 → 桌面真正提交"那条指令的持久账，真库上不存在就会在
--   第一次 `POST /api/sync/push` 时炸 `no such table`（而且是**手机那条路**炸，
--   桌面上不看日志根本发现不了）—— 正是总纲 §21 那条静默故障的同一族。
--   ⚠ 连带后果：`manifest.schemaVersion` 3 → 4，手机会按契约 §1 重走一次 bootstrap
--   （`MobileSchema.VERSION` 得由手机那一侧跟着改，本仓库 android/** 不归这次改动）。
--
-- 2026-10 由 5 提到 6：角色卡**只留三个字段**（名称 / 正面提示词 / 负面提示词）。
--   建表 DDL 那边少了十几个列，但收敛式 DDL **不给已存在的表删列** —— 所以老库靠
--   `infra::db` 里那条迁移：补 `prompt` 列 + 把 `tags`（JSON 数组）拼成一行灌进去；
--   其余老列原样留着、只是没人再读（删列要重建表，不值当）。同步范围没变，
--   但 manifest 的版本号变了，手机照样重走一次 bootstrap —— 无害。
-- 2026-10-11 由 6 提到 7：加产物**集合**（`media_collections` 表 + `media_assets.collection_id`）。
--   一次改动同时踩了"加表"和"加列"两条路，**两条都必须走**：
--   ① 加表靠 bump 版本号让老库重跑一遍收敛式 DDL（`CREATE TABLE IF NOT EXISTS`）；
--   ② 加列 `CREATE TABLE IF NOT EXISTS` **补不了**，靠 `infra::db` 的 `NEW_COLUMNS` 那套
--      "先 PRAGMA table_info 探测再 ALTER"。
--   只做①的表现是"集合永远是 None、界面上什么都不显示"；只做②的表现是产物一入库就炸
--   `no such table: media_collections` —— 两种都是静默的（后者只在入库那条路上炸）。
--   ⚠ 连带后果：`manifest.schemaVersion` 6 → 7，手机会按契约重走一次 bootstrap
--   （`MobileSchema.VERSION` 要由手机那一侧跟着改，本仓库 `android/**` 不归这次改动）。
--
-- 2026-10-11 由 7 提到 8：给 `prompts` 加 `negative_mode`（发送模式：保留 / 只发正面）。
--   加的是**列**，走 `infra::db` 的 `NEW_COLUMNS` 那套（`CREATE TABLE IF NOT EXISTS` 补不了列）；
--   bump 版本号是为了让**新加的列**在老库上也有一趟自愈的机会
--   （`ensure_columns` 每次开库都跑，其实不 bump 也能补上；bump 是为了让手机那边
--   拿到新的 `manifest.schemaVersion`、顺带重走一次 bootstrap —— 提示词形状变了）。
-- 2026-10-18 由 8 提到 9：**手动集合**两列。
--   ① `media_collections.title` —— 用户给集合起的名字（算法分出来的没有名字）；
--   ② `media_assets.collection_opt_out` —— 用户**明确把这一条移出集合**了。
--   ⚠ ② 是**正确性的前提**：`backfill` 只处理 `collection_id IS NULL` 的行，
--   而"移出集合"恰恰就是置 NULL —— 少了这一列，"移出"会在下次启动被静默吸回原组。
--   两列都走 `NEW_COLUMNS`（`CREATE TABLE IF NOT EXISTS` 补不了已存在表的列）。
-- 2026-10-21 由 9 提到 10：**画风卡 + 两种卡片版本的示例产物**。
--   ① 新增 `style_cards`（与角色卡同形）；
--   ② `character_cards` / `style_cards` 各加 `tag_prompt`（Danbooru tag 版），
--      原有 `prompt` 明确为**自然语言版**；
--   ③ 新增 `character_card_examples` / `style_card_examples` 两张关联表。
--   `character_cards.tag_prompt` 是"加列"，必须同时写进 `infra::db` 的 `NEW_COLUMNS`；
--   其余都是新表，靠 bump 版本号重跑收敛式 DDL 就能建出来。
--   （⚠ 这一条在 v11 被**撤回**了：`tag_prompt` 已从两个清单里删掉，见下。）
-- 2026-10-24 由 10 提到 11：**卡片回到单版本**（只要自然语言）。
--   ① 新库的两张卡表不再建 `tag_prompt`（老库那一列留在原地，再没人读）；
--   ② 升级到 11 的那一次由 `infra::db::migrate_card_tags_to_prompt` 把老的 tag 版
--      **转成自然语言**灌进 `prompt` —— 只填空卡，已有自然语言版的卡一个字都不动。
PRAGMA user_version = 11;
