# 🔒 SQLite schema 契约（冻结，M0）

> **状态：已冻结（2026-09-23）。** 依据 `docs/implementation-master-plan.md` §3.1 与
> `docs/memory-and-storage-evaluation.md` §5.2.2。
>
> 这份文件管的是**语言与数据库中立的那部分**：连接怎么初始化、MySQL 类型怎么落到 SQLite、
> 同步账本三张表长什么样。**表清单与列定义的真源仍是 DDL 文件**（`db/schema.sql` 现在是 MySQL 版；
> P2 落 `db/schema.sqlite.sql` 时以本文件的约定为准，逐表对齐）。
>
> 桌面 Rust 重写与手机 Kotlin harness **都按这份契约写**，所以它在 P4 之前冻结是有意义的。

---

## 1. 连接初始化（**每一处开连接都要**，红线 R1）

```sql
PRAGMA journal_mode = WAL;      -- 持久化在文件头，设一次就够
PRAGMA synchronous  = NORMAL;   -- WAL 下的推荐值
PRAGMA busy_timeout = 5000;     -- ★ 按连接，别漏
PRAGMA foreign_keys = ON;       -- ★ 按连接，默认 OFF，漏了不报错、外键静默失效
```

| PRAGMA | 作用域 | 漏了会怎样 |
| --- | --- | --- |
| `journal_mode=WAL` | **文件头（持久）** | 退化成 rollback journal：读写不能并发 |
| `synchronous` | 按连接 | 默认 FULL，比 NORMAL 慢 |
| `busy_timeout` | **按连接** | 撞锁立刻 `SQLITE_BUSY`，而不是等一会儿 |
| `foreign_keys` | **按连接** | **`ON DELETE SET NULL` / `CASCADE` 全部静默失效** —— 本设计重度依赖它们 |

**R1 的防线**：连接工厂里强制设置（不是"记得设"）+ 一条"删标签后 `prompt_tags` 关联行也没了"的集成测试。

**R2**：写事务一律 **`BEGIN IMMEDIATE`**，不用裸 `BEGIN`。
原因：裸 `BEGIN` 是"先拿读锁、要写时再升级"，**升级失败可能不理会 `busy_timeout`**，直接 `SQLITE_BUSY`。
`tokio-rusqlite` 单写连接（§5）那套是更彻底的办法，但 `BEGIN IMMEDIATE` 这条纪律仍然要守。

---

## 2. MySQL → SQLite 类型映射

| MySQL | SQLite | 备注 |
| --- | --- | --- |
| `BIGINT UNSIGNED AUTO_INCREMENT` | `INTEGER PRIMARY KEY AUTOINCREMENT` | 见 R4：**必须有 `AUTOINCREMENT`** |
| `TINYINT(1)` 布尔 | `INTEGER` + `CHECK(x IN (0,1))` | SQLite 没有真布尔 |
| `ENUM(...)` | `TEXT` + `CHECK(kind IN (...))` | 枚举值要靠 CHECK 兜住 |
| `MEDIUMTEXT` / `JSON` | `TEXT` | JSON1 函数默认可用（`json_extract` 等） |
| `DATETIME(3)` + `CURRENT_TIMESTAMP(3)` | `TEXT` ISO-8601，**统一 UTC** | 排序即字典序，别混本地时间 |
| **`ON UPDATE CURRENT_TIMESTAMP(3)`** | **触发器** | SQLite 无此语法；而 `updated_at` 是**承重的**（会话列表按它排序） |
| `ON DUPLICATE KEY UPDATE v=VALUES(v)` | `ON CONFLICT(k) DO UPDATE SET v=excluded.v` | 见 **R3**：**禁止** `INSERT OR REPLACE` |
| `INSERT IGNORE` | `INSERT OR IGNORE` | |
| `information_schema` 探测 | `PRAGMA table_info` / `PRAGMA index_list` / `sqlite_master` | 迁移的"先探测再改"靠这几个 |
| `TIMESTAMPDIFF(SECOND,a,NOW(3))` | `CAST((julianday('now')-julianday(a))*86400 AS INTEGER)` | |
| `GROUP_CONCAT` | `group_concat` | |

**schema 版本 = `PRAGMA user_version`**（不建版本表）。
迁移用"先 `PRAGMA table_info` 探测再改"的幂等写法，风格照搬现有 `Migrate.kt`。

> **加列（2026-09-30 起有先例了）**：`db/schema.sqlite.sql` 是**收敛式**的，但建表语句
> **不给已存在的表补列** —— 只在 DDL 里加一列的后果是**老库永远拿不到它，而且不报错**
> （读出来永远是默认值）。所以加列要**三处一起改**：
>
> 1. `db/schema.sqlite.sql` 的建表 DDL（新库直接就有）；
> 2. `server-rs/src/infra/db.rs` 的 `NEW_COLUMNS` 加一条 `(表, 列, ALTER 语句)` ——
>    `open_or_create` **每次开库**都按 `PRAGMA table_info` 探测，缺了才 `ALTER`（幂等）；
> 3. `SCHEMA_VERSION` +1，`db/schema.sqlite.sql` 末尾那行 `PRAGMA user_version = N` 同步。
>
> 只能加**可空列**或**带默认值的列**（SQLite `ADD COLUMN` 的限制）；改类型 / 改名要重建表，
> 那种迁移不在 `NEW_COLUMNS` 里，得单独写。防线：`db.rs` 的
> `open_or_create_heals_a_missing_column_without_touching_data` 与
> `healing_a_missing_table_never_downgrades_a_newer_version`（后者盯的是"自愈时不许把
> 更新的库降级写回去"—— DDL 末尾那行会把版本号写成 `SCHEMA_VERSION`，所以判据要比
> **DDL 跑完之后**的实测值，不能比"跑之前"读到的那些）。

> `db/migrate.sql` 那份**存储过程版可以废弃**（SQLite 无存储过程）→ 三份 DDL 变**两份**。

---

## 3. 新增表（桌面）

```sql
-- 变更账本：每张被同步的表挂 3 个触发器（AFTER INSERT/UPDATE/DELETE）往这里写一行
CREATE TABLE _sync_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,   -- ★ R4：必须 AUTOINCREMENT，否则 rowid 复用 → 客户端永久丢一段
  tbl TEXT NOT NULL,
  pk  TEXT NOT NULL,
  op  TEXT NOT NULL CHECK(op IN ('upsert','delete')),
  at  TEXT NOT NULL
);
CREATE INDEX idx_sync_log_tbl ON _sync_log(tbl);

-- 每个设备的游标（日志裁剪靠它：DELETE FROM _sync_log WHERE seq <= MIN(cursor)）
CREATE TABLE sync_clients (
  device_id TEXT PRIMARY KEY,
  cursor    INTEGER NOT NULL DEFAULT 0,
  last_seen TEXT
);
```

### 三条红线的具体形态

| # | 规矩 | 为什么 | 防线 |
| --- | --- | --- | --- |
| **R3** | 被同步的表**禁止 `INSERT OR REPLACE`**，用 `ON CONFLICT(...) DO UPDATE` | `INSERT OR REPLACE` 内部是 DELETE+INSERT，会点亮 DELETE 触发器 → 往 `_sync_log` 写**假的 delete** → 别的设备把好好的行删掉 | 一条"upsert 不产生 delete 条目"的用例 |
| **R4** | `_sync_log.seq` 必须 `INTEGER PRIMARY KEY AUTOINCREMENT` | 不写 `AUTOINCREMENT` 时 rowid 会被复用，游标收到"用过的号" → 客户端**永久丢一段变更** | 一条"删除后再插入，seq 不复用"的用例 |
| **R1** | 见 §1 | — | 连接工厂 + 级联删除用例 |

---

## 4. 新增表（手机）

```sql
CREATE TABLE _outbox (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,   -- 本地序号，不是同步 seq
  kind       TEXT NOT NULL,
  ref_id     TEXT,
  payload    TEXT NOT NULL,
  created_at TEXT NOT NULL,
  tries      INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
```

**手机侧刻意不要 `_sync_log`、不要触发器** —— 手机是**投影（projection）不是副本**：
它只需要记住"我有哪些还没推上去"（`_outbox`），下行方向由桌面的 `_sync_log` 驱动。
在手机上也挂触发器会造出**第二本账**，两边永远对不齐。

---

## 5. 连接分工（Rust 桌面侧，§5.1）

- **写**：`tokio_rusqlite::Connection` **单连接**专管写 → 写天然串行化，**彻底不会 `SQLITE_BUSY`**。
- **读**：一小池**只读**连接（WAL 允许读写并发）。
- **`rusqlite` 必须 `features = ["bundled"]`**：SQLite 编进二进制，
  于是发布包**不再需要 `mysql/`(304MB)、`jre/`、VC++ 运行时**（`R14` 同一条理由，只是换到桌面上）。

---

## 6. 同步范围（唯一真源，触发器 / `bootstrap` / `changes` 三处都从它派生）

| 同步 | 不同步 |
| --- | --- |
| `prompts` `tags` `prompt_tags` `media_assets` `media_tags` `media_collections` | **`ai_run_events`**（单 Run 上万条的调试日志） |
| `ai_conversations` `ai_messages` `ai_message_parts` `ai_attachments` | `ai_providers` `ai_models` `app_settings`（**含凭据引用与令牌明文**） |
| `capture_runs` `ai_runs` `ai_tool_calls` | `ai_questions`（提问闸门只在桌面侧；手机看实时提问走 `GET /api/ai/pending`） |

> **2026-10-11：`media_collections` 进同步范围**（产物集合：同源微调的产物归到一组）。
> 它必须跟着 `media_assets` 一起走 —— 否则手机上的 `collection_id` 指向一个**它那边不存在的
> 集合**，表现是"折叠不出组、也解释不了这几张为什么在一起"，而且**两端都不报错**。
> 连带：`SCHEMA_VERSION` 由 6 提到 7（一次同时加表 + 加列，两条路都要走，见 §2）；
> `media_collections` 也进了 `PHONE_FORBIDDEN`（分组由桌面算，手机不许自己造组）。
> 判定规则本身**不进库** —— 真源是 `server-rs/src/media/collection.rs` 的纯函数，
> 库里只存 `group_key` / `origin` / `rule_version`。

> ✅ **2026-09-27 用户拍板（总纲 §7 待定项 1 = 进同步范围）**：`ai_runs` / `ai_tool_calls`
> 从"可选"变成**进**，理由是手机要能显示桌面那次 Run 的**工具过程**（跑过哪些工具、成没成、
> 花了多久）。三条连带：
> ① 两张表的 `_sync_log` 触发器在 `db/schema.sqlite.sql` 里补上了，而 `SCHEMA_VERSION`
> 由 2 提到 3 —— **不 bump 的话老库直接早退、触发器一条都不会有**（`open_or_create`
> 对"版本够新、表都在"的库不重跑 DDL）；顺带 `manifest.schemaVersion` 变了，
> 手机据此重新 bootstrap；
> ② 上行也要收（`table_of_kind` 认 `ai_run` / `ai_tool_call`），否则"手机发起的 Run
> 在桌面上看不到工具过程"；
> ③ 手机侧 `ai_runs` 变成**镜像表**之后，"崩溃恢复哪些 Run"必须靠本机的 `local_runs`
> 标记来判（`status='running'` 会把桌面的 Run 也算进去，按 R6 标 failed 再推回桌面）。
> ⚠ `ai_tool_calls` **没有 `updated_at` 列** → 上行的 LWW 对它不生效（`push` 只在 payload
> 带 `updated_at` 时才比新旧）。纪律是"**只推自己写的行**"：手机不重推从桌面收到的行。

- **主键约定**：会话 / 消息 / 块是 `CHAR(36)` UUID → 两端各自生成不会撞。
- **手机不要创建 `prompts` / `media_assets` / `tags`**（那些由桌面在提交时产生）。
- ⚠ **`/api/sync/bootstrap` 绝不发 `.db` 文件**（`app_settings` 里有配对令牌明文），按业务表分页。
- ⚠ **游标落在窗口之外必须回 `410 Gone`**，手机丢弃镜像走 `/bootstrap`。
  这个分支**必须实现**，否则表现是"手机少了几百条且永远补不回来"。
