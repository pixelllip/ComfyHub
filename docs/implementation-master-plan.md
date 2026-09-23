# 实施总纲：SQLite + 桌面 Rust + 移动 Kotlin + Flutter

> **状态：方案已敲定，即将实施（2026-09-23）。**
> 目标架构：**两端 SQLite；桌面服务端 Rust；移动端 harness 用 Android Kotlin；UI 两端都是 Flutter。**
>
> 本文是**唯一的实施入口**。之前的评估文档记录了推导过程与多次改判（A→B、Dart→Kotlin…），
> **实施时不要以它们为准**，只查它们的论证细节（§1 有地图）。
>
> 本文里 **🔒 表示已冻结的契约**，改动它要同时改两端。

---

## 1. 目标架构（一页）

```
┌─────────────────────── 桌面（Windows，唯一真源）───────────────────────┐
│  comfyhub-server  (Rust: axum + rusqlite/bundled)                      │
│    ├── 业务 API   /api/{prompts,tags,media,ai,...}   ← 与今天逐字一致   │
│    ├── 同步 API   /api/sync/{manifest,changes,bootstrap,push}          │
│    ├── 工件 API   /api/ai/run-kit{,/status}                            │
│    └── comfyhub.db (WAL) ── 业务表 + _sync_log + sync_clients          │
│  storage/media/  ← 二进制不进库，HTTP + ETag(sha256)                   │
│  jre/ 与 mysql/ 目录 → **删除**（发布包从 ~550MB 降到 ~50MB）           │
└───────────────────────────────┬───────────────────────────────────────┘
                                │ 局域网 HTTP + X-ComfyHub-Token
                                │ 远程走 Tailscale（不映射公网端口）
┌───────────────────────────────▼───────────────────────────────────────┐
│  安卓：Flutter UI  ──platform channel──▶  Kotlin harness               │
│           │                                  ├── LLM 调用 + 工具循环    │
│           │                                  ├── 本地 SQLite(WAL)      │
│           │                                  └── _outbox（上行队列）   │
│           └── 与桌面同一套 RunEvent 事件流（UI 代码共享）                │
└───────────────────────────────────────────────────────────────────────┘
```

**三条不可动摇的架构纪律**（与 `docs/android-client-plan.md` §5 一致）：

1. **桌面是唯一真源**；移动端不直连 ComfyUI、不装 Python/MCP、**不自起服务端**。
2. **提交永远由桌面执行**（`comfy_submit` 只在桌面）。移动端产出「待提交工件」，不是提交动作。
3. **规则只有一份 —— 由桌面产出、版本化下发**（系统提示 / 工具 schema / 安全文案），
   两端都不许硬编码（见 🔒 §3.4）。

---

## 2. 红线：实施时一条都不能破

每条都给出**防线**（删了以后出错还能不能被抓住，是唯一标准）。

| # | 红线 | 防线 |
| --- | --- | --- |
| **R1** | **每条 SQLite 连接都必须 `PRAGMA foreign_keys=ON`** —— 默认是 OFF 且按连接生效。本设计严重依赖 `ON DELETE SET NULL`（删提示词保产物）/ `CASCADE`（删标签清关联）。漏了**不报错**，只是外键静默失效 | 连接工厂里强制设置 + 一条"删标签后关联行也没了"的集成测试 |
| **R2** | 写事务一律 **`BEGIN IMMEDIATE`**，不用裸 `BEGIN`（读锁升级写锁可能**不理会 `busy_timeout`** 直接 `SQLITE_BUSY`） | 代码评审 + 并发写测试 |
| **R3** | **被同步的表禁止 `INSERT OR REPLACE`**（它内部是 DELETE+INSERT，会点亮 DELETE 触发器 → 往 `_sync_log` 写**假的 delete**），必须用 `ON CONFLICT(...) DO UPDATE` | 一条"upsert 不产生 delete 条目"的用例 |
| **R4** | `_sync_log.seq` 必须 **`INTEGER PRIMARY KEY AUTOINCREMENT`**（否则 rowid 会被复用 → 游标收到"用过的号" → 客户端永久丢一段变更） | 一条"删除后再插入，seq 不复用"的用例 |
| **R5** | **`ai_run_events` 的保留窗口必须够长**（手机后台被杀是常态，重连靠 `?after=<seq>`）。当前 `pruneEvents` 是**死代码**、`RunEventBus` 全量留内存 | `pruneEvents` 真的被调用 + "断线重连仍能拿到完整事件"的用例 |
| **R6** | 失败/取消时**"已经说出去的话一个字都不能丢"**（三条纪律：正文必须落块 / 标题缓冲要 flush / 重试不许就地重置有内容的回复）—— AGENTS §10.1 | `e2e-ai-tools-test.ps1` 第 13 幕 + `ai_retry_keep_content_test.dart` |
| **R7** | 工具结果与网页内容当**不可信数据**：不拼进系统提示指令区，不因它说"忽略规则"而改行为（RSK-004） | 不可信横幅文案由 run-kit 下发（🔒 §3.4），**两端不许各写一份** |
| **R8** | 需审批的工具：调用方**必须先 `open(callId)` 再发 `tool.requested`** | 审批顺序用例 |
| **R9** | 附件的**三层准入**（选完预检 → 发送前禁用 → 创建 Run 前用库里事实再验）；失败必须 `400 UNSUPPORTED_CONTENT` 且**上游请求数为 0** | `e2e-ai-tools-test.ps1` 第 9 幕 |
| **R10** | `comfy_submit` **只认 API 格式节点图**（`capture_runs.raw` 里那份），且产物必须走 `capture.captureRun()` 入库 | `e2e-submit-test.ps1` |
| **R11** | **移动端不得复用 `comfy_submit` 这个名字**（桌面离线时它无法履行"提交→等生成→返回 mediaIds"的契约，模型会误报成功） | 手机侧工具名 `comfy_queue_submit` + 系统提示写明"排队 ≠ 已提交" |
| **R12** | 工具清单**只有一个真源**（`ToolRegistry`）；系统提示改必须 **bump `SystemPrompt.VERSION`** | `ToolRegistryTest` + `SystemPromptNsfwTest` |
| **R13** | **两端都不许硬编码系统提示 / 工具描述 / 安全文案**，只能来自 run-kit | 一条"run-kit 之外无硬编码提示串"的检查 |
| **R14** | 安卓侧 **SQLite 必须用 bundled 驱动**（`androidx.sqlite:sqlite-bundled` / SQLDelight bundled），**绝不用系统 SQLite**（Android 13 仍是 3.32a、无 FTS5） | 依赖清单评审 + 一条"FTS5 可用"的启动自检 |

---

## 3. 冻结的契约（实施第一件事就是把它们落成文件）

> 这 5 份契约是**语言与数据库中立**的。**先冻结契约，再动实现** —— 这样桌面 Rust 重写
> 与移动 Kotlin harness 可以并行，互不阻塞。

### 3.1 🔒 SQLite schema 约定

**连接初始化（每一处开连接都要）：**

```sql
PRAGMA journal_mode = WAL;      -- 持久化在文件头，设一次
PRAGMA synchronous  = NORMAL;
PRAGMA busy_timeout = 5000;     -- 按连接
PRAGMA foreign_keys = ON;       -- ★ 按连接，R1
```

**类型映射（MySQL → SQLite，`docs/memory-and-storage-evaluation.md` §5.2.2 有完整表）：**

| MySQL | SQLite |
| --- | --- |
| `BIGINT UNSIGNED AUTO_INCREMENT` | `INTEGER PRIMARY KEY AUTOINCREMENT` |
| `TINYINT(1)` 布尔 | `INTEGER` + `CHECK(x IN (0,1))` |
| `ENUM(...)` | `TEXT` + `CHECK(kind IN (...))` |
| `MEDIUMTEXT` / `JSON` | `TEXT`（JSON1 函数默认可用） |
| `DATETIME(3)` + `CURRENT_TIMESTAMP(3)` | `TEXT` ISO-8601，**统一 UTC** |
| **`ON UPDATE CURRENT_TIMESTAMP(3)`** | **触发器**（SQLite 无此语法，且 `updated_at` 是**承重的**：会话列表按它排序） |
| `ON DUPLICATE KEY UPDATE v=VALUES(v)` | `ON CONFLICT(k) DO UPDATE SET v=excluded.v`（R3） |
| `INSERT IGNORE` | `INSERT OR IGNORE` |
| `information_schema` 探测 | `PRAGMA table_info` / `PRAGMA index_list` / `sqlite_master` |
| `TIMESTAMPDIFF(SECOND,a,NOW(3))` | `CAST((julianday('now')-julianday(a))*86400 AS INTEGER)` |
| `GROUP_CONCAT` | `group_concat` |

**schema 版本：`PRAGMA user_version`**（不用建表；`/api/sync/manifest` 要吐给手机）。
迁移用"先 `PRAGMA table_info` 探测再改"的幂等写法，风格照搬现有 `Migrate.kt`。
**`db/migrate.sql` 那份存储过程版可以废弃**（SQLite 无存储过程）→ 三份 DDL 变**两份**。

**新增表（桌面）：**
- `_sync_log(seq INTEGER PRIMARY KEY AUTOINCREMENT, tbl, pk, op, at)` + `idx_sync_log_tbl`（R4）
- `sync_clients(device_id TEXT PRIMARY KEY, cursor INTEGER, last_seen TEXT)`
- 每张被同步的表挂 3 个触发器（AFTER INSERT/UPDATE/DELETE → 写 `_sync_log`）

**新增表（手机）：** `_outbox(id, kind, ref_id, payload, created_at, tries, last_error)`；
手机侧**不要** `_sync_log`、不要触发器（它是投影，不是副本）。

### 3.2 🔒 RunEvent 协议

**这是"UI 代码两端共享"的全部依据**（`mobile-harness-design.md` §5）。

- 事件类型与 `RunEventBus.kt` 的 `RunEventType` **逐字一致**：
  `run.started` / `message.started` / `reasoning.delta` / `text.delta` /
  `tool.requested` / `tool.started` / `tool.completed` / `tool.failed` /
  `user.question` / `user.question.answered` / `usage.updated` /
  `message.completed` / `conversation.updated` / `run.completed` / `run.failed` /
  `run.cancelled` / `heartbeat`
- `seq` 在**一次 Run 内单调递增**，从 1 开始；断线重连一律 `?after=<seq>`
- SSE 帧格式 `id: <seq>` / `event: <type>` / `data: <json>`
- **落地物**：`docs/run-event-contract.md`（人看的）+ 两端各一份常量 + 一条"清单一致"的用例

### 3.3 🔒 同步 API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/sync/manifest` | `{schemaVersion, kitVersion, cursor, serverTime, tables:{...}}` |
| `GET` | `/api/sync/changes?since=<cursor>&limit=500` | `{cursor, hasMore, upserts:{表:[完整行]}, deletes:{表:[主键]}}` |
| `GET` | `/api/sync/bootstrap?page=N` | 首次全量（**按业务表分页，绝不发 .db 文件** —— `app_settings` 里有配对令牌明文） |
| `POST` | `/api/sync/push` | 上行 outbox 批次 → `{applied:[], rejected:[]}`，**幂等** |

- **游标 = 本次扫描到的最大 `seq`**；重复推送一行无害（幂等），**唯独不能跳号**
- **`deletes` 必须独立成组**（手机分不清"没这行"和"被删了"）
- **游标落在窗口之外 → `410 Gone`**，手机丢弃镜像走 `/bootstrap`。
  ⚠ **这个分支必须实现**，否则表现是"手机少了几百条且永远补不回来"
- 日志裁剪：`DELETE FROM _sync_log WHERE seq <= (SELECT MIN(cursor) FROM sync_clients)`

**同步范围（唯一真源，触发器/`bootstrap`/`changes` 三处都从它派生）：**

| 同步 | 不同步 |
| --- | --- |
| `prompts` `tags` `prompt_tags` `media_assets` `media_tags` | **`ai_run_events`**（62 万行的调试日志） |
| `ai_conversations` `ai_messages` `ai_message_parts` `ai_attachments` | `ai_providers` `ai_models` `app_settings`（**含凭据引用与令牌明文**） |
| `capture_runs`（可选） | `ai_runs` / `ai_tool_calls`（可选，看你是否要在手机显示工具过程） |

**主键约定**：会话/消息/块是 `CHAR(36)` UUID ✅ 两端各自生成不会撞；
**手机不要创建 `prompts`/`media_assets`/`tags`**（那些由桌面在提交时产生）。

### 3.4 🔒 run-kit（工件下发）

```
GET /api/ai/run-kit         → { kitVersion, schemaVersion, systemPrompt, promptVersion,
                                nsfwEnabled, tools:[{name,description,inputSchema,scope}],
                                untrustedBanner, models:[...], inlineBudget:{...} }
GET /api/ai/run-kit/status  → { current, revoked:[...] }   ← 命中吊销则手机必须联网再跑
```

**手机 harness 的所有"规则类"输入都从这里来**（R13）。`kitVersion` 记进每次 Run
（对应现有 `promptVersion` 的可追溯性）。

### 3.5 🔒 工具分端（ToolScope）

新增 `ToolScope.DESKTOP / MOBILE / BOTH`，由 `ToolRegistry` 在**生成下发给模型的清单**时按端过滤。

| 端 | 工具 |
| --- | --- |
| **MOBILE 可执行** | `web_search`、`web_fetch`、`civitai_search`、`list_skills`、`load_skill`、`remember`、`ask_user_question`、**`comfy_queue_submit`（新增，R11）** |
| **桌面专属** | 全部 `comfy_*`、comfy-mcp 的 39 个、`read_file`/`write_file`/`list_dir`、`register_skill`/`delete_skill` |

> ⚠ 这不是安全边界（安全边界仍是 `ToolPolicy`），是"别让模型看见它调不到的工具"。
> 而**手机发起的 Run 若在桌面执行，工具全都可达** —— 那是策略问题，见 §7 待定项 3。

---

## 4. 实施路线（两条轨道，可并行）

```
轨道 A（桌面）  P0 过渡缓解 → P2 SQLite(保留Kotlin) → P4 Rust 重写(按端点灰度)
轨道 B（移动）              M0 契约冻结 → M1 桌面侧支撑 → M3 harness → M4 上行 → M5 排队提交
                                        ↑ 依赖 P2 之后的栈，但**契约与语言无关**
```

### 阶段表

| 阶段 | 目标 | 交付物 | 验收判据（DoD） | 依赖 |
| --- | --- | --- | --- | --- |
| **P0** ✅ | **过渡期缓解** —— **已完成 2026-09-23**（①MySQL ②JVM ③连接池 ④daemon 全做完）：`Write-Ini` 加 `performance_schema=0`/`pool=32M`/`log_buffer=8M`/`redo=32M`/`max_connections=24`/`table_open_cache=256`/`thread_cache_size=4`；新增 `mysql.ps1 config` 动作就地重写活 `my.ini`（**不用 init/move 动数据目录**）；`build.gradle.kts` 加那一组 JVM 参数；`Db.kt` 池 12→4；`server/gradle.properties` 加 `org.gradle.jvmargs=-Xmx768m` + `kotlin.daemon.jvmargs=-Xmx640m` | 实测 `@@performance_schema=0`、pool **32M**、logbuf **8M**、maxconn **24**、`@@log_bin=0`；真实数据下 mysqld Working Set **158.2MB**（原 638MB）；`gradle test` **573 例全绿**；`DEFAULT_JVM_OPTS` 已带全部新参数（两种布局都走 `installDist` 那一个 `.bat`） | — |
| **P0.5** | binlog | ✅ **已完成**（2026-09-23）：`skip-log-bin` 落在 `Write-Ini` + 活 my.ini，释放 4.27GB | `@@log_bin=0` | — |
| **P1** | 事件日志治跑飞 | 接上 `pruneEvents`、事件批量写、`RunEventBus` 改环形缓冲 | DB 从 112.9MB 降到个位数；单 Run 写入量降 1~2 个数量级；**断线重连续传仍完整**（R5） | 无 |
| **P2** | **MySQL → SQLite（保留 Kotlin/Ktor）** —— **进行中**。第 1 步已落（2026-09-23）：① 新 schema `db/schema.sqlite.sql`（🔒3.1 全表 + `_sync_log` / `sync_clients` + 按同步范围挂的 22 个 `_sync_log` 触发器 + 8 个 `updated_at` 触发器，`user_version=1`）；② `Db` 方言化（PRAGMA 拼进 JDBC URL 并按连接读回自检、`BEGIN IMMEDIATE`、时间一律 ISO-8601 UTC 文本）；③ `Migrate` 走 `PRAGMA table_info` 探测 + 触发器"先删再建"的收敛式迁移；④ 数据层 17 个文件改方言（`ON CONFLICT…DO UPDATE` / `INSERT OR IGNORE` / `strftime` / 去掉只对 MySQL 合法的 UPDATE 别名 / `julianday` 算年龄）；⑤ **默认库已切到 `<根>\.db\comfyhub.db`**；⑥ R1~R4 各有用例（`SqliteFoundationTest` 12 例 + `SqlScriptTest` 2 例，全库 611 例绿），真实后端在 SQLite 上冒烟 30 个端点通过 | 第 1 步已完成：R1/R2/R3/R4 用例 + `PRAGMA` 自检（`user_version` 与 `RunKit.SCHEMA_VERSION` 对齐）。**剩余**：搬迁工具（MySQL→SQLite，逐表 `COUNT(*)` 与抽样字段哈希一致）、脚本/打包/comfyhub.ps1 去掉 MySQL 依赖（发布包去掉 `mysql/` 后能起） | P1 |
| **M0** | **冻结 5 份契约** —— ✅ **全部落文件（2026-09-23）**：① RunEvent 协议（`docs/run-event-contract.md`）② SQLite schema 约定（`docs/sqlite-schema-contract.md`）③ 同步 API 4 条端点（`docs/sync-api-contract.md`）④ ToolScope 清单（`docs/tool-scope-contract.md`，**代码也已落地**：`ToolScope`/`toolsFor`/`ToolScopeTest`）⑤ run-kit 工件（`docs/run-kit-contract.md`，形状已冻结，端点实现归 M1） | 两端"事件类型清单一致"用例（**桌面侧已做**：`RunEventContractTest`；手机侧待 harness 落地）；契约文件评审通过 | **无（可立刻做）** |
| **M1** ✅ | **桌面侧支撑** —— **已完成 2026-09-23**（`7f3a351` + 补丁 `1e3ce01`，e2e 修复 `3030d93`）：① 提问落库（新表 `ai_questions` + `AiQuestionRepo`，`AskRegistry` 状态变迁写穿：track 即落 / answer 写终值 / 收尾只动 pending / 重启后 `GET /questions/{id}` 回落查库）；② 审批落库（`ToolApprovalGate.open` 同一刻落 `ai_tool_calls` pending 行，invoke 后原地更新；启动恢复把 pending 审批按"超时=拒绝"落 denied、pending 提问落 expired）；③ `GET /api/ai/pending` 跨端待办总览（asks + approvals，另一台设备不靠 SSE 也能发现"有东西在等人"）；④ `GET /api/ai/run-kit{,/status}`（kitVersion 签名哈希变了才 bump、`tools[]` 按 ToolScope 过滤成手机子集、`untrustedBanner` 模板真源进 ToolRegistry 常量、models[] 无密钥、inlineBudget）；⑤ 闸门类事件（tool.requested / user.question）立即落库（P1 批量化的缺口，见 pitfalls） | 手机发起的 Run 的审批在另一端也能看到并批准（`/api/ai/pending` + 落库行）；后端重启后 pending 不丢（提问卡可回放、审批行有终态）；**AskPersistenceTest 7 例 + RunKitVersionTest 6 例**，gradle 597 例全绿，三条 e2e 全绿 | M0 |
| **M3** | **移动 Kotlin harness** | `android/app/src/main/kotlin/`：LLM 调用 + SSE + 工具循环 + 本地 SQLite + 三条失败纪律（R6）+ platform channel | 离线（桌面不在线）能完成一次 prompt 设计对话；UI 用**与桌面相同的事件流**渲染，UI 代码零分叉 | M0、M1 |
| **M4** | 上行通道 | `_outbox` + `/api/sync/push` + 幂等 apply + 服务端赢 LWW | 手机产生的会话在桌面可见可续写；重复推送不产生重复行 | M3 |
| **M5** | 排队提交 | 手机 `comfy_queue_submit` + 桌面 outbox 消费（**走现有 `comfy_submit` 全链路**） | 手机排队 → 桌面在线后自动提交 → 产物入库画廊 → `mediaIds` 回到手机；**桌面侧审批/幂等/入库一条不少（R10/R11）** | M4 |
| **M6** | 凭据 | `ai_providers` 下行（**不含密钥**）+ 手机 Android Keystore + `flutter_secure_storage` | 手机能独立调用 LLM；密钥不出本机；界面明确告知"需在手机上单独填 Key" | M3 |
| **P4** | **桌面 Rust 重写** | axum + rusqlite/bundled；**逐一实现现有 `/api/...`，Flutter 一行不改**；按端点灰度（先 `/api/tags`、`/api/prompts`） | 灰度期两端点并存；Rust 侧内存 RSS 目标 ≤30MB；**R6/R7/R8/R9/R10/R12 在 Rust 侧重建**（11,103 行 Kotlin 测试要重写成 Rust 测试） | P2、M0 |

### 轨道依赖的关键判断

- **P2 先于 P4**：先换库（改动集中在数据层、可逐表验证），再换语言。
  **不要合成一次大爆炸** —— 同时改语言和数据库会把两类失败混在一起，无法归因。
- **M 轨道不阻塞于 P4**：契约（M0）与移动端（M3）都能在 **Kotlin 后端**上开发测试。
  Rust 重写是几个月的工程，移动端不该等它。
- **P0 是过渡期止痛**，不是终局：SQLite 落地后 MySQL 那半张调优表作废，
  但它用 1 天换几个月的不难受，值。**JVM 参数到 P4 才作废。**

---

## 5. 各端实现要点（照抄级）

### 5.1 Rust 桌面端

```toml
axum            = "0.8"
tokio           = { version = "1", features = ["full"] }
rusqlite        = { version = "0.32", features = ["bundled"] }   # ★ bundled 自带 SQLite 3.53.x
tokio-rusqlite  = "0.6"        # SQLite 跑专属线程 → 写天然串行化，彻底不会 SQLITE_BUSY
serde / serde_json
tower-http      = { features = ["cors", "compression-gzip", "fs"] }  # ServeFile 原生 Range，替 Ktor PartialContent
image           = "0.25"       # 缩略图，替 ImageIO + TwelveMonkeys(WebP)
hmac / sha2 / base64           # 媒体签名 URL、令牌比较、sha256 去重
```

- **`bundled` 是战略决定**：SQLite 编进二进制 → 发布包**不再需要 MySQL 目录（304MB）、
  JRE、VC++ 运行时**（`packaging\manifest.json` 的 `runtimeRequirements` 可整条删）。
- **连接分工**：`tokio-rusqlite::Connection` 单连接专管写；读用一小池只读连接（WAL 允许读写并发）。
- **迁移**：手写 + `PRAGMA user_version`（贴合项目"不引 Flyway"的现有取向），或 `rusqlite_migration`。
- ⚠ **不要**把 `comfy_submit` 的行为简化：它必须仍走"审核 → 审批 → 提交 → `capture.captureRun()` 入库"。

### 5.2 Kotlin 移动端 harness

- 代码放 `android/app/src/main/kotlin/`（或一个 `com.android.library` 模块）。
  **⚠ 动 `android/` 之前先读 `docs/android-agp9-builtin-kotlin-migration.md`**（AGENTS §4）；
  **不要引入 KMP**（会与 AGP 9 内建 Kotlin 冲突，理由见 `mobile-harness-design.md` §2.1）。
- **为将来的 KMP 留门**：纯逻辑（协议适配、标题摘取、提示拼装、事件类型、工具循环控制流）
  写成**不依赖任何 Android API 的普通 Kotlin 类**，I/O 走接口注入。将来抽 `commonMain` 是搬文件不是重写。
- **优先移植而不是重写**：`protocol/Adapters.kt`、`ConversationTitle.kt`、`KeenableSearch`、`CivitaiSearch`
  都可以逐行对照地搬（这是选 Kotlin 的主要理由）。
- **本地库**：Room / SQLDelight + **bundled SQLite**（R14）。
- **凭据**：Android Keystore；`CredentialService.kt` 的 DPAPI 实现（`:190` `if (!isWindows()) error(...)`）**不可复用**。
- **platform channel**：`MethodChannel`（调用）+ `EventChannel`（Run 事件流）。
  大文件**不走通道**（走文件路径 / HTTP）；`Flow` → `EventChannel`。
- **规模上限：约 1,500~2,500 行 Kotlin**。显著超出说明在"重实现规则"而不是"移植" → 回头查 R13。

### 5.3 Flutter 端（两端共用）

- **加 `RunEventSource` 抽象**：一个 Dart 接口，两个实现 ——
  「远端 SSE（桌面）」与「本地 platform channel（手机）」。**UI 代码保持零分叉**（🔒3.2 的价值全靠它）。
- `pubspec.yaml` 待补：`path_provider`（缓存目录）；安卓视频要加官方 `video_player`。
  ⚠ **不要因为要加 `video_player` 就把 `video_player_win` 换回 pub.dev** —— 那是打过补丁的本地副本（AGENTS §9）。
- 图片缓存用 `cached_network_image` 之类，配合 `ETag = sha256`（内容寻址 + `immutable`）。
- 列表/懒构建/`AppMenuButton`/`AdaptiveColumnList` 等既有界面纪律**全部继续适用**（AGENTS §1/§6）。

---

## 6. 回归防线归属表

| 红线 | 现有防线 | 迁移后要补/重建 |
| --- | --- | --- |
| R1 foreign_keys | 无（MySQL 强制） | **新增**：连接工厂单测 + 删标签级联的集成测试 |
| R2 BEGIN IMMEDIATE | 无 | **新增**：并发写测试 |
| R3 upsert 不产假 delete | 无 | **新增**：`_sync_log` 断言 |
| R4 seq 不复用 | 无 | **新增**：删后插的 seq 断言 |
| R5 重连续传 | `e2e-ai-tools-test.ps1` 部分覆盖 | 补"长 Run + 断线重连"用例；`pruneEvents` 真的被调用 |
| R6 失败不丢内容 | `e2e-ai-tools-test.ps1` 第 13 幕（15 项）+ `ai_retry_keep_content_test.dart` | 手机 harness 侧**照搬同一组用例** |
| R7 注入防线 | `SystemPromptNsfwTest`、`ToolRegistryTest` | 文案改由 run-kit 下发后，补"两端拿到的是同一份"的用例 |
| R8 审批顺序 | `AskGateTest` | ✅ **已落（M1）**：pending 行落库 + `AskPersistenceTest`（写穿 / 收尾守卫 / 重启回落 / 跨端口径）+ `GET /api/ai/pending`；手机侧发起 Run 时也要过 |
| R9 附件三层准入 | `e2e-ai-tools-test.ps1` 第 9 幕 | 手机侧发起 Run 时也要过 |
| R10 提交链路 | `e2e-submit-test.ps1`、`e2e-capture-test.ps1` | Rust 侧重建 |
| R11 不复用提交名字 | 无 | **新增**：ToolRegistry 断言不存在移动端 `comfy_submit` |
| R12 工具单一真源 | `ToolRegistryTest` | 加 `ToolScope` 过滤用例 |
| R13 不硬编码规则 | 无 | **新增**：扫描手机源码无提示串 |
| R14 bundled SQLite | 无 | **新增**：启动自检 FTS5 可用 |

> ⚠ 跑 e2e 的既有纪律：**别在跑完前 Ctrl+C**（清理在 finally 里，中断会留下 `e2e-submit-*`
> 记录把下一次的 SHA-256 判重弄乱）；`e2e-submit-test.ps1` 会自己把权限档临时钉成 `ask` 再还原。

---

## 7. 风险登记与待定项

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| **Rust 侧 11,103 行 Kotlin 测试要重写** | 这是 P4 最大的成本与风险，安全类防线（R6~R10）必须重建 | 按 §6 逐条对照重建；**先灰度非 AI 端点**（tags/prompts），AI 与工具层最后搬 |
| 两套 harness 漂移 | 安全规则两端不一致 | R13（规则下发）+ 同语言可逐行对照 + 事件契约一致性用例 |
| platform channel 的边界情况 | 前后台切换、事件丢失、大 payload | 事件流做 `Flow`→`EventChannel`；大文件绕开通道；补"切后台再回来事件不丢"的用例 |
| 安卓前台服务 6h/24h 配额 | 长 Run 可能被杀 | 前台服务 + **Run 状态落本地库**，重启后能识别未完成 Run 并按 R6 收尾 |
| 桌面离线期间的模型/模型目录 | 手机用旧目录 | run-kit 版本化 + 吊销（🔒3.4） |

**待定项（实施中会撞到，需当场拍板）：**

1. **`capture_runs` / `ai_runs` / `ai_tool_calls` 要不要同步**（要不要在手机显示工具过程）？
2. **手机上点"提交"时，桌面还要不要再过一道审批闸门**？
   （注意 `inbound-auth-design.md` 的令牌是**连接鉴权**，不是**操作审批**，两者别混。）
3. **手机发起的 Run 若被路由到桌面执行**（桌面在线时），是否允许它做桌面写操作？
   这决定 `ToolScope` 到底是"可达性过滤"还是"策略开关"。
4. **手机能不能用桌面没配置的 provider**？（建议：只用桌面已配置的，简化续写）
5. **`ai_run_events` 搬迁时直接丢弃，还是留在 MySQL 里归档？**（建议丢弃 + 先把 R5 的保留策略做掉）

---

## 8. 文档地图

**实施入口 = 本文。** 论证与细节按需查：

| 文档 | 看它学什么 |
| --- | --- |
| **本文** | **实施总纲：契约、红线、阶段、验收** |
| 🔒 [`docs/run-event-contract.md`](run-event-contract.md) | **RunEvent 协议**（信封 / 17 个类型 / SSE 帧格式 / tool.requested≠user.question / 保留窗口边界）—— M0 已冻结 |
| 🔒 [`docs/sqlite-schema-contract.md`](sqlite-schema-contract.md) | **SQLite 约定**（四个 PRAGMA / 类型映射 / `_sync_log`+`sync_clients`+`_outbox` DDL / R1~R4 的具体形态）—— M0 已冻结 |
| 🔒 [`docs/sync-api-contract.md`](sync-api-contract.md) | **同步 API 4 条端点**（manifest/changes/bootstrap/push、游标语义、410 分支、幂等与 LWW）—— M0 已冻结 |
| 🔒 [`docs/tool-scope-contract.md`](tool-scope-contract.md) | **工具分端 ToolScope**（四值语义、MOBILE 那 7 个、R11 为什么不许复用 `comfy_submit`）—— M0 已冻结 |
| 🔒 [`docs/run-kit-contract.md`](run-kit-contract.md) | **run-kit 工件**（两条端点、字段真源、`kitVersion` 的 bump 规则、R13 "两端不许硬编码规则"）—— M0 已冻结（实现归 M1） |
| `docs/memory-and-storage-evaluation.md` | 为什么要换（实测内存数据 §3.2）、MySQL 方言逐条可移植性（§5.2.2）、发布包收益（§5.2.4） |
| `docs/cross-device-sync-design.md` | 同步机制细节（变更账本、游标、墓碑、bootstrap、SQLite 坑 §6）、安卓客户端现状（§6.5） |
| `docs/mobile-harness-design.md` | 手机 harness 的范围与边界（§4）、为什么 Kotlin 而非 Dart/Rust（§2）、run-kit（§3） |
| `docs/embedded-db-sync-options-report.md` | 六个同步方案为什么都不用（§4）、安卓平台硬约束（§3/§6.3，已核实） |
| `docs/audit-client-architecture-android-readiness.md` | 客户端端点全清单 + 桌面耦合清单（做安卓要用） |
| `docs/android-client-plan.md` | 单写者架构、工具分端、跨端接力的原始分步 |
| `docs/inbound-auth-design.md` | 配对令牌、回环豁免、媒体签名 URL（**开远程的前置条件**） |
| `docs/database-study-notes.md` | 数据模型、外键语义、迁移三条路径 |
| `docs/ai-tools-and-skills.md` + `AGENTS.md` §10 | 工具/Skills/审批/附件的既有纪律 |
| `docs/android-agp9-builtin-kotlin-migration.md` | **动 `android/` 之前必读** |
| `docs/pitfalls.md` | 50+ 条踩坑记录 |

> ✅ **两处登记已同步（2026-09-23）**：
> ① `AGENTS.md` §11 已加"目标架构与实施总纲"指向本文；
> ② `README.md` 的架构/目录表已补一句"目标架构"指引（**不改现状描述** —— 现在跑的还是 MySQL + Kotlin）。
