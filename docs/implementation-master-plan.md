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
| **P2** ✅ | **MySQL → SQLite（保留 Kotlin/Ktor）** —— **已完成（2026-09-23，4 个提交）**。① schema：`db/schema.sqlite.sql`（🔒3.1 全表 + `_sync_log` / `sync_clients` + 22 个 `_sync_log` 触发器 + 8 个 `updated_at` 触发器，`user_version=1`）；② `Db` 方言化（PRAGMA 拼进 JDBC URL 并按连接读回自检、`BEGIN IMMEDIATE`、时间一律 ISO-8601 UTC 文本、自动建库目录）；③ `Migrate` 走 `PRAGMA table_info` 探测 + 触发器"先删再建"的收敛式迁移；④ 数据层 17 个文件改方言；⑤ **默认库 = `<根>\.db\comfyhub.db`**；⑥ 搬迁工具 `MigrateData` + `gradle migrateData`（`scripts\server.ps1 migrate`）；⑦ 脚本与打包去掉 MySQL：`up` 不再起库、老用户**零动作升级**（检测到老库有数据而新库没建时自动搬一次）、`pack-release.ps1` 不再携带 `mysql/`（实测装配 36MB）、`runtime-deps.ps1` 把 MySQL/VC++ 降级成 `Required=$false` | ① R1/R2/R3/R4 各有真库用例（`SqliteFoundationTest` 12 例 + `SqlScriptTest` 2 例 + `MigrateDataTest` 6 例；Gradle 617 例全绿）；② **逐表 `COUNT(*)` + 全表行哈希**双向核对：本机真实数据 16 张表 2778 行**全部一致**（`ai_run_events` 按 §7 待定项 5 丢弃）；③ 搬完用 API 抽查（27 提示词 / 23 产物 / 22 会话 / 196 个消息块 / 缩略图直出 200）；④ 去掉 `mysql/` 后能起：`up` 在 MySQL 停止的情况下起后端并报 `db=ok`，三条 e2e 全绿（submit 41 项 / capture / ai-tools 111 项）、`flutter test` 248 例全绿 | P1 |
| **M0** | **冻结 5 份契约** —— ✅ **全部落文件（2026-09-23）**：① RunEvent 协议（`docs/run-event-contract.md`）② SQLite schema 约定（`docs/sqlite-schema-contract.md`）③ 同步 API 4 条端点（`docs/sync-api-contract.md`）④ ToolScope 清单（`docs/tool-scope-contract.md`，**代码也已落地**：`ToolScope`/`toolsFor`/`ToolScopeTest`）⑤ run-kit 工件（`docs/run-kit-contract.md`，形状已冻结，端点实现归 M1） | 两端"事件类型清单一致"用例：桌面 `RunEventContractTest` ✅ + **手机 `RunEventContractTest` ✅（M3 落地时补上）**；契约文件评审通过 | **无（可立刻做）** |
| **M1** ✅ | **桌面侧支撑** —— **已完成 2026-09-23**（`7f3a351` + 补丁 `1e3ce01`，e2e 修复 `3030d93`）：① 提问落库（新表 `ai_questions` + `AiQuestionRepo`，`AskRegistry` 状态变迁写穿：track 即落 / answer 写终值 / 收尾只动 pending / 重启后 `GET /questions/{id}` 回落查库）；② 审批落库（`ToolApprovalGate.open` 同一刻落 `ai_tool_calls` pending 行，invoke 后原地更新；启动恢复把 pending 审批按"超时=拒绝"落 denied、pending 提问落 expired）；③ `GET /api/ai/pending` 跨端待办总览（asks + approvals，另一台设备不靠 SSE 也能发现"有东西在等人"）；④ `GET /api/ai/run-kit{,/status}`（kitVersion 签名哈希变了才 bump、`tools[]` 按 ToolScope 过滤成手机子集、`untrustedBanner` 模板真源进 ToolRegistry 常量、models[] 无密钥、inlineBudget）；⑤ 闸门类事件（tool.requested / user.question）立即落库（P1 批量化的缺口，见 pitfalls） | 手机发起的 Run 的审批在另一端也能看到并批准（`/api/ai/pending` + 落库行）；后端重启后 pending 不丢（提问卡可回放、审批行有终态）；**AskPersistenceTest 7 例 + RunKitVersionTest 6 例**，gradle 597 例全绿，三条 e2e 全绿 | M0 |
| **M3** | **移动 Kotlin harness** | ✅ **代码已落地（2026-09-23，9 个提交）**：`android/harness/`（`com.android.library`，AGP 内建 Kotlin，**不引 KMP/KSP/序列化编译器插件**）＋ `android/app/.../bridge/`（`MethodChannel` + `EventChannel` + Android Keystore）＋ `lib/core/run_event_source.dart`（`RunEventSource` 抽象两个实现）。内容：① 事件契约（17 类型 + 环形缓冲 + 与契约文档比对的用例）；② 协议层（SSE 分帧 / 三家适配器 / 思考方言 / 工具分片）；③ 标题摘取 + 内联预算；④ 本地库（bundled SQLite：10 张镜像表逐列同形 + 本机表 + `_outbox`，PRAGMA 自检 R1/R14，**不建账本不挂触发器**）；⑤ 仓储层（会话/消息/块/Run/事件/outbox，业务行与 outbox **同事务**，`listUnfinishedRuns` 供崩溃恢复）；⑥ run-kit 客户端（取/缓存/吊销判定）；⑦ 工具层（`remember` / `list_skills` / `load_skill` / `ask_user_question` / `comfy_queue_submit` / `web_fetch`）；⑧ harness 主循环（流式 + 工具往返 + **R6 三条失败纪律** + 截断续写去重）；⑨ event payload 与桌面逐字对齐（**顺带改正契约文档 §2 的 4 处键名错**） | ① **145 个 Kotlin 用例**（`:harness:testDebugUnitTest`，含 R6 三条 / R8 审批顺序 / R11 不复用提交名字 / R13 无规则文案 / R14 FTS5）+ **7 个 Dart 用例**（`test/run_event_source_test.dart`）+ `flutter test` 255 例全绿；② `flutter build apk --debug` 出包；③ 桌面侧 run-kit 补下发 `titleInstruction` / `continueInstruction`（R13：手机零提示串） | M0、M1 |
| **M3.1** | 真机验收（**进行中：设备已到位，2026-09-24**） | 🔶 **本次只验到"harness 能在真机上起来"，UI 侧还是断的**。设备 = OnePlus 8T（KB2000，arm64-v8a，transport 752e8d2f），`build\app\outputs\flutter-apk\app-debug.apk`（168MB）装得上、起得来。**真机证据**：① logcat 里 `libsqliteJni.so` 从 APK 的 `lib/arm64-v8a` 加载成功 → **bundled SQLite 真的是自带的**（R14 在真机上成立，而不是只在单测里）；② `run-as com.example.viewer ls files/` 看到 `comfyhub-harness.db` + `.db-wal`(828KB) + `.db-shm` → 桥在 `attach()` 里建库成功，**PRAGMA 读回自检与 FTS5 探针在真机上没炸**（炸了就是 FATAL，logcat 里没有）；③ `flutter build apk --debug` 的产物在 arm64 上跑 Impeller/Vulkan 正常。**但没验到"真实对话"**：见下面那条阻断项 | M3 |
| **M3.2** | **把 harness 接进 UI**（分步做，**①②③ 已落地**） | ⚠ 起点：**`RunEventSource` 曾是死代码** —— `openRunEventSource` 在 Android 上本该返回 `LocalHarnessRunEventSource`，但全仓没有一个消费者，`lib/state/ai_workspace_store.dart` 在 `send()` / `_consumeRun()` 里仍直接调 `_api.startRun()` / `_api.runEvents()`（`:1355`、`:1423`、`:1670`）。后果：手机上那套 Kotlin harness 根本调不到，桌面离线时手机什么也发不出去。<br>✅ **① 桥能交回消息主键了（`cba4016`）**：`RunRequest` 新增 `userMessageId` / `assistantMessageId`（可空），`HarnessStore.appendMessage` 支持显式主键，`prepare()` 用调用方给的 id 落两条消息并写进 `ai_runs` 两个外键，`start()` 返回值从 `String` 改成 `StartedRun`；桥把两个 id 回给 Dart，`RunStartRequest` / `RunHandles` 补上对应字段（远端实现改用后端回传的那一对 —— 远端才是真源）。**为什么这是硬前提**：界面在发出请求的同一刻就按自己生成的 id 插好"用户气泡 + 空助手气泡"，然后靠事件里的 `messageId` 找到那条气泡往上写字；harness 自己另起 UUID 的话界面那个 id 永远查不到库里那条，表现是**事件一条不少、界面上却一个字都不长**，两端都不报错。<br>✅ **② 本地建会话的桥 + Dart 接线（`39ad4b7`）**：桥新增 `createConversation`（认调用方给的 id、落本地库、回那条会话），Dart `LocalHarnessRunEventSource.createConversation` 两端接上。注意它默认 `queue=true` → **进 `_outbox`**，等桌面在线时由 `/api/sync/push` 上行，正是 M4 那条"手机产生的会话在桌面可见"的前提。防线：`HarnessStoreTest` 一例（认调用方给的 id / 建完立刻查得到 / 进了 outbox / **同一 id 再建要报错，不许静默覆盖**）+ Dart 两例（两个消息 id 真的送到 Kotlin 并读回、`createConversation` 走本地库）。<br>✅ **③ 持久化分叉 + 冷启动分叉都做了（`613531f` / `85b0acc`）**：`send()` / `retry()` / `openConversation()` / `_consumeRun()` / `stop()` / `answerQuestion()` / `_reloadMessages()` 一律走 `RunEventSource`（本地库还是远端 HTTP 由它分路）；`_doLoad()` 也分组 —— 手机走 `_loadLocalStartup()`，**会话只认本地库**，桌面离线也能进聊天页（provider / 模型清单拿不到只如实说一句，不让整页起不来）。防线：`test/ai_offline_source_test.dart` 6 例。**仍未完成**：provider / 模型清单离线可用要等 **M6**；本地空壳会话清理还缺桥上的 `deleteConversation`（两条坑都记在 §9 ①）。<br>⬜ **④ 真机回归**：M3.1 只验到"harness 能在真机上起来"，**离线对话一次都还没在真机上跑过**。 | M3.1 |
| **M4** | 上行通道 | `_outbox` + `/api/sync/push` + 幂等 apply + 服务端赢 LWW | 手机产生的会话在桌面可见可续写；重复推送不产生重复行 | M3 |
| **M5** | 排队提交 | 手机 `comfy_queue_submit` + 桌面 outbox 消费（**走现有 `comfy_submit` 全链路**） | 手机排队 → 桌面在线后自动提交 → 产物入库画廊 → `mediaIds` 回到手机；**桌面侧审批/幂等/入库一条不少（R10/R11）** | M4 |
| **M6** | 凭据 | `ai_providers` 下行（**不含密钥**）+ 手机 Android Keystore + `flutter_secure_storage` | 手机能独立调用 LLM；密钥不出本机；界面明确告知"需在手机上单独填 Key" | M3 |
| **P4** | **桌面 Rust 重写** | 🔵 **进行中（截至 2026-09-25：第 1~44 步已落地，`cargo test` 315 例；工具层 14 个）**。**现状一句话**：非 AI 端点全通；`/api/ai/*` 的界面端点全通；Run 生命周期 / SSE / 执行器 / 工具层（含审批与提问闸门）**已能真的聊**；`/api/sync/*` 四条全通；捕获链（`status` / `jobs` / `poll`）已通并**在真 ComfyUI 上实测过**；界面格式工作流能转成 API 图。**剩下的**：**MCP 桥（39 工具）**、`comfy_submit`（R10）、`comfy_{find_workflow,check_inputs,fit_workflow,env_report,audit_workflow,prune_workflow}`、联网三工具 + `find_model`、附件（R9）、截断续写、**后台捕获轮询器**、`capture/{import,import-workflows}`、`ingest/comfyui`、`capabilities/{refresh,probe}`、`media/{id}/poster`、`WorkflowConvert` 的两处欠账（subgraph / 前端模拟）、**切流收尾**（schema 归 Rust、脚本与发布包、Kotlin 退役）。**下面是逐批落地记录（历史）**：第 1 步（只读）：`server-rs/`（axum 0.8 + tokio + **rusqlite 0.40 bundled** + serde + tower-http）。**第 1 步（只读）**：`db.rs`（连接工厂 + PRAGMA **读回自检** R1 / `BEGIN IMMEDIATE` R2 / FTS5·user_version 自检）、`models.rs`、`routes.rs`（`/api/health`、`/api/tags`(+categories)、`/api/prompts`、`/api/prompts/{id}`）。**第 2 步（写）**：`inputs.rs`、`repo.rs`（逐条对照 `TagRepo`/`PromptRepo`）、`write_routes.rs`（标签 CRUD；提示词 CRUD/收藏/复制/标签维护）。**第 3 步（产物）**：`media.rs`（列表/详情/**原件文件流**：ETag = `sha256` 内容寻址 → `If-None-Match` 304；单段 `Range` → 206 + `Content-Range`；目录穿越双保险拦截）。**第 4 步（产物写侧 + 统计）**：`media_write.rs`（`PATCH`/`DELETE`：`clearPrompt` 与 `promptId=null` 语义分开、空 body 回 404、删除连磁盘原件与缩略图一起清；`GET /api/stats` 七个字段）。**第 5 步（缩略图）**：`thumbs.rs`（`image` crate 替掉 ImageIO + TwelveMonkeys：最长边 512 / 不放大 / 透明先铺 #1E1E22 / JPEG 0.82；失败回退原图）。**第 6 步（上传）**：`media_upload.rs`（multipart 多文件：落盘名 32 位 hex+扩展名、**按内容 sha256 判重**、单文件一个事务、图片顺手写缩略图、单文件失败不整批失败）。**第 7 步（捕获配置 + 工作流原文）**：`capture.rs`（`GET/PUT /api/capture/config` 整体覆盖+归一化；`GET /api/{prompts,media}/{id}/workflow` 的 **404/204/200 三分**与原文直出）。**第 8 步（关联查询）**：`/api/prompts/{id}/media`（提示词不存在也回 200 + `[]`，与 Kotlin 同）、`/api/media/{id}/prompt`（产物不存在 404 / 没关联 204 / 有 200）。未搬的一律 **501**。默认端口 **8081** 与 Kotlin 的 8080 并存灰度 | ① `cargo test` **48 例**全绿；② **对真实库副本 + 产物目录副本起服务抽查**：24 条产物 8 页、`file` 200 且 ETag=sha256、`Range: bytes=0-9` → 206、`If-None-Match` → **304**、`stats` 27/24/16 + 分组、`PATCH` 三个分支（含 `bogus promptId` 400 / 空 body 404）、`DELETE` → `{"deleted":215}` 且**临时副本文件被删、真实产物文件未被动**、`media` 24→23；真库只读未改。**未搬**：`/api/media` 的 poster（要 ffmpeg 抽帧）、需要 ComfyUI 在场的 `/api/capture/{status,jobs,poll,locate*,import*,workflows}`、`/api/ingest/comfyui`、`/api/prompts/{id}/api-graph`、`/api/ai/*`、`/api/sync/*`；**未重建**：R6~R10；RSS ≤30MB 待量。⚠ **已知的一处行为差异**（切流验收清单要过）：悬挂引用时 `/api/media/{id}/prompt` 我们回 404，Kotlin 回 200 + `{error:…}`（前端会解析成空提示词） | P2、M0 |

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
| R1 foreign_keys | 无（MySQL 强制） | ✅ **已补（M3）**：`MobilePragmas` 连接工厂 + `HarnessDatabaseTest`（含"另开一条没上 PRAGMA 的连接 fk=0"的实据 + 级联删除） |
| R2 BEGIN IMMEDIATE | 无 | ✅ **已补（M3）**：`SqlSession.transaction` 唯一实现 + 回滚用例 |
| R3 upsert 不产假 delete | 无 | ✅ **已补（M3）**：手机侧**不建账本**（契约 §3.1），且 `appendPart` 用 `ON CONFLICT DO UPDATE`；桌面侧 P2 已有用例 |
| R4 seq 不复用 | 无 | ✅ 桌面 P2 已补；手机侧不建 `_sync_log`（无此风险），事件 seq 由 `RunEventBus` 单调发号并有用例 |
| R5 重连续传 | `e2e-ai-tools-test.ps1` 部分覆盖 | ✅ **手机侧已补（M3）**：`RunEventBusTest`（截断只丢最老、seq 不回退）+ `HarnessStoreTest`（裁剪留尾部）+ 事件批量写。✅ **Rust 侧已补（P4 第 26/27 步）**：`events` 环形缓冲 2000 与库里 4000 **刻意不同数**、`prune_events` 留尾部、`resume_from` 把号接上 `max(seq)+1` |
| R6 失败不丢内容 | `e2e-ai-tools-test.ps1` 第 13 幕（15 项）+ `ai_retry_keep_content_test.dart` | ✅ **手机侧已补（M3）**：`HarnessRunnerTest` 三例（429 半截 / 首字被压住 / 取消）+ `HarnessStoreTest` 两例。✅ **Rust 侧已补（P4 第 30 步）**：执行器三条收尾路都 `sink_now`（失败/取消也把已说出去的话落块）+ `middle-cut-stream` 用例 |
| R7 注入防线 | `SystemPromptNsfwTest`、`ToolRegistryTest` | ✅ **手机侧已补（M3）**：横幅文案只做 `{kind}`/`{query}` 替换（`MobileToolsTest` 断言前缀来自 kit） |
| R8 审批顺序 | `AskGateTest` | ✅ 桌面（M1）+ **手机侧（M3）**：`MobileAskGate` 先 open 再发事件，`MobileToolsTest` 有两例（手快先答不落空 / 没人答如实报）。✅ **Rust 侧已补（P4 第 31 步）**：`approval_gate_opens_before_the_user_can_click`（**不等事件**，库里有 pending 就立刻点，必须 `accepted=true`）+ `a_denied_call_is_recorded_and_never_runs`（拒绝**不许执行**、只许一行、`approval=denied` + `status=failed`）|
| R9 附件三层准入 | `e2e-ai-tools-test.ps1` 第 9 幕 | ✅ **手机侧已补（M3）**：`InlineBudgetTest` + 适配器未实现模态直接抛（`ProtocolAdapterTest`）。✅ **Rust 侧已补（P4 第 50 步）**：`tests/attachments_test.rs` 4 例 —— **图片真的内联**（`image_url` / `input_image` / `source.base64` 三种形状逐条断言）+ 发不出去的**如实告知**（"以下附件未随本次请求发送"）+ 创建 Run 前拦下时**上游零请求且不落消息** |
| R10 提交链路 | `e2e-submit-test.ps1`、`e2e-capture-test.ps1` | ✅ **Rust 侧已补（P4 第 48 步）**：`tests/submit_test.rs` 6 例（真 socket 假 ComfyUI：提交 → 等 → 入库 → `mediaIds`、nsfw 标记、预检拦截时**零请求**、超时如实报、`NO_API_GRAPH`、参数错误）；"拿 `e2e-submit-test.ps1` 对着 Rust 跑一遍"仍归切流前清单 |
| R11 不复用提交名字 | 无 | ✅ **已补（M3）**：手机侧工具名 `comfy_queue_submit`，`MobileToolsTest` 断言不返回 `mediaIds`、不说"已提交" |
| R12 工具单一真源 | `ToolRegistryTest` | ✅ 桌面有 `ToolScopeTest`；手机侧**不维护自己的规则清单**（`declaredTo` = kit ∩ 本地实现） |
| R13 不硬编码规则 | 无 | ✅ **已补（M3）**：`R13NoPromptLiteralsTest` 扫描手机生产源码（豁免清单只有两个摘取标记常量）；桌面 run-kit 补下发 `titleInstruction` / `continueInstruction` |
| R14 bundled SQLite | 无 | ✅ **已补（M3）**：`sqlite-bundled` 依赖 + `MobilePragmas.problems` 的 FTS5 探针（启动自检，失败直接抛） |

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
| [`docs/workflow-recipe-design.md`](workflow-recipe-design.md) | **工作流配方**（骨架 / 模型槽位 / 参数档位三层分解、学习闭环六步、新模型的三种情形、落地分步 W1~W5、明确不做）—— 见 §12 |
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
>
> ✅ **M3 落地登记（2026-09-23，9 个提交）**：
> | 提交 | 内容 |
> | --- | --- |
> | 第 1 步 | `android/harness` 模块骨架 + RunEvent 契约（17 常量 + 环形缓冲 + 契约比对用例） |
> | 第 2 步 | 协议层（SSE 分帧 / 三家适配器 / 思考方言 / 工具分片） |
> | 第 3 步 | 标题摘取状态机 + 内联预算（含 R6 的 flush 纪律） |
> | 第 4 步 | 本地库地基（bundled SQLite + PRAGMA 自检 + 镜像表与桌面逐列同形） |
> | 第 5 步 | 仓储层（会话/消息/块/Run/事件/outbox + 崩溃恢复） |
> | 第 6a/6b 步 | run-kit 客户端（取/缓存/吊销）+ 工具层（6 个工具，`web_search`/`civitai_search` 待补） |
> | 第 7 步 | **主循环**（流式 + 工具往返 + R6 三条失败纪律 + 截断续写去重） |
> | 第 8a/8b 步 | event payload 与桌面逐字对齐（**改正契约文档 4 处键名**）+ platform channel 桥 + Flutter `RunEventSource` |
> | 第 9 步 | **R13 防线**（扫描手机源码无规则文案） |
>
> **还没做的两件事**（诚实清单）：
> ① **真机验收**（M3.1）—— 本机没有模拟器/真机，`app-debug.apk` 只验证到"能编出来"；
> ② 手机侧 `web_search` / `civitai_search` 两个工具（未实现就**不宣称**：`declaredTo` = kit ∩ 本地实现，
> 模型看不见它们，所以不会"调了必然失败"）。
>
> ✅ **P4 第 1~2 步落地（2026-09-23：`afcf395` 只读 / `bf955bb` 写）**：Rust 版起了骨架并跑通
> **读 + 写**两个灰度批次（`server-rs/`，默认 8081，与 Kotlin 的 8080 并存）。
> 三条纪律在 Rust 侧已经重建：连接工厂 + PRAGMA **读回自检**（R1）、`BEGIN IMMEDIATE` 写事务（R2，
> 错误类型泛型化但**只有一处实现**）、FTS5/user_version 自检（R14）。
> 形状纪律用**精确键集合**断言盯住（"少一个键 = 界面上少一块内容"）；
> 写路径额外盯两件事：**"建提示词+建标签+重算 use_count" 在同一事务里**、**写操作要点亮 `_sync_log`**。
> 踩到并记进 `docs/pitfalls.md` 的坑：① serde 漏 `rename_all=camelCase` → 所有字段静默落默认值；
> ② rusqlite 的 bundled 构建**默认就开** `foreign_keys`（所以防线是自检而不是默认值）。
> **P4 第 3 步（`49e82cd`）**把产物那批搬了过来，其中**文件流是自己写的**（不复用 `tower-http::ServeFile`）：
> 我们的 ETag 本来就是现成的 —— `media_assets.sha256` 是内容哈希，于是"内容寻址 + `immutable` +
> `If-None-Match → 304`"这套缓存语义不用额外存任何东西；`Range` 只做**单段**（多段是多部分响应，
> 宁可不支持也不返回格式不对的 206）。顺带踩到并修掉两处：媒体 COLS 里把 `positive_prompt` 写成了
> `prompt_positive`（列不存在 → 整个列表 500）、`Vec::dedup()` 只去相邻重复（标签参数要的是全局去重）。
> 下一步：`/api/media` 的写侧（upload 要 multipart、thumb 要图像解码器、poster 要 ffmpeg 抽帧），
> 然后是 `/api/capture/*`，最后才是 `/api/ai/*` 与 R6~R10 的重建。
>
> ✅ **P4 第 9 步（`3f10f19`，2026-09-24）**：`/api/prompts/{id}/api-graph` —— **API 格式节点图**。
> 这是 **R10 的前提**：`comfy_submit` 只认 API 图，而它在整个系统里的唯一来源就是这个端点
> （`/workflow` 是界面格式，两个不是一回事）。取数链条照抄 Kotlin `ComfyCapture.apiGraphOf`：
> `prompts.source_ref`(= `run_key`) → `capture_runs.raw` → `HistoryEntry.parse(entry).graph`。
> - **三分语义一条都不能混**（把 204 说成 404，界面就会以为"提示词没了"）：
>   提示词不存在 → **404**；没有 `source_ref` / 没有 `capture_runs` 行 / `raw` 是脏数据 /
>   `raw` 里没有图 → **204**；有 → **200** + 图本体。
> - `HistoryEntry` **逐行移植**（ComfyUI `/history` 条目有三种历史形状：`{prompt:<图>}` 扁平 /
>   `{prompt:{prompt:<图>, extra_data:{…}}}` 具名 / `{prompt:[<图>, <extra>]}` 数组）。
>   判据是**结构特征**（非空且每个值都是带 `class_type` 的对象）而**不是键名** ——
>   猜错的后果是把 `extra_data` 当成节点图发给 ComfyUI，提交必炸且报错毫无线索。
> - **验收**：`cargo test` **54 例**全绿（22 lib + 32 集成），其中 `history` 5 例（三种形状 +
>   不肯猜 + `extra_data` 不被误认成图）、`api_graph_distinguishes_404_from_204_from_200` 走满 6 个分支；
>   **真实库副本起服务抽查**（8082）：id=310/309/308/307/306 **全部 200**，吐的确实是真节点图
>   （`1:LoraLoader` / `1:CheckpointLoaderSimple`，带 `class_type`），`999999` 回 404；真库只读未改。
> - 仍未搬：`/api/capture/{status,jobs,poll,locate*,import*,workflows}`、`/api/ingest/comfyui`
>   （都要 ComfyUI 在场）、`/api/ai/*`、`/api/sync/*`；**R6~R10 的 Rust 侧重建仍未开始**。
>
> ✅ **M3.2 第 1 步（`cba4016`，2026-09-24）**：把"harness 能被 UI 调到"的第一个硬前提补上 ——
> 桥以前只回 `runId`，现在回**一对消息主键**（`StartedRun`），并且 harness 认调用方给的 id。
> 这一步不改变"谁能发起 Run"（那要等 ② 的持久化分叉），但它把一条**最难查的静默故障**堵死了：
> 界面按自己生成的 id 插好气泡、harness 另起 UUID 落库 → 事件齐全但界面一个字都不长，两端都不报错。
> 防线是 `HarnessRunnerTest` 的两例（含 `ai_runs` 外键与 `message.started` 的 `messageId`
> 必须都指向同一对）；`harness` 147 例、`flutter test` 255 例全绿。
>
> ✅ **P4 第 10 步（`7fd2fb0`，2026-09-24）**：`GET /api/sync/manifest`（契约 §1）—— 手机连上来的
> **第一件事**（"我该不该走 bootstrap？我的游标还有效吗？"）。返回 `{schemaVersion, kitVersion,
> cursor, serverTime, tables}`。
> - ⚠ **查出一个有安全后果的坑**：直觉上"被同步的表 = 挂了 `_sync_log` 触发器的表"，
>   但这份 schema 实际建出来的触发器覆盖了 **`app_settings` / `ai_providers` / `ai_models`**
>   （见 `db/schema.sqlite.sql`），而总纲 §3.3 的同步范围**明确排除**它们 ——
>   `app_settings` 里存着**配对令牌明文**。按触发器派生 `synced` 的话，手机第一次连上来就会看到
>   `app_settings` 是"可同步的表"，随后的 `changes` / `bootstrap` 就把令牌送出去了
>   （正是契约 §3"绝不发 .db 文件"要防的事）。
>   所以范围**写死成两张显式清单**（`SYNCED` / `NOT_SYNCED`+理由），且不同步的表**连行数都不给**
>   （行数本身也是信息）。防线：`manifest_never_offers_credentials`（改成触发器派生就红）。
> - 两条如实语义：`kitVersion` 没有就回 **null**（不编一个）；`cursor` 一律 = `MAX(_sync_log.seq)`
>   同源，**不是自己数的行数**。
> - **验收**：`cargo test` **59 例**全绿（25 lib + 34 集成）；真实库副本抽查（8083）：
>   `schemaVersion=1`、`kitVersion=2026-09-23.3`（Kotlin 侧写过的那个值）、`cursor=3328`、
>   `serverTime=2026-09-24T00:14:06Z`；10 张同步表行数与已知事实一致（prompts 27 /
>   media_assets 24 / ai_conversations 23），7 张不同步表带理由；真库只读未改。
> - `sync` 剩下两条未做：`/api/sync/bootstrap`（按表分页）、`/api/sync/push`（幂等 + LWW）。
>
> ✅ **P4 第 11 步（`c1277fb`，2026-09-24）**：`GET /api/sync/changes`（契约 §2）。四条硬约束
> 逐条落地：**游标** = 本次扫描到的最大 `seq`（没扫完停在最后一条，扫完了才跳到账本末尾）；
> **`deletes` 独立成组**，单列主键发标量、复合主键（`prompt_tags`/`media_tags`）发数组 `[a,b]`；
> **窗口外 → `410 Gone`**；`upserts` 给**完整行**（行 → JSON 的泛型映射，**不维护每张表的列清单**
> —— 列清单一漂移就会静默少发一列，而界面上少一列是看不出来的）。
>
> ⚠ **更正第 10 步里的一个错误结论**：第 10 步曾说"触发器覆盖了 `app_settings`，所以从触发器
> 派生同步范围会把配对令牌发出去" —— **那是错的**。核对 `db/schema.sqlite.sql` 后确认：真正挂
> `_sync_log` 触发器的表**正好就是**那 10 张；`app_settings` / `ai_providers` / `ai_models` /
> `ai_runs` 上只有 `trg_*_touch`（维护 `updated_at` 的 `AFTER UPDATE`），**当初把 `touch` 当成了
> `sync`**（一个 grep 匹配到了错误的触发器族）。白名单照样保留，但理由改成如实的那一条：
> 这条路径的失败模式**静默且严重**（哪天有人给设置表补一个同步触发器，派生式实现会立刻开始发
> 令牌，而且没有任何测试会变红），外加契约指定"总纲 §3.3 那张表"为唯一真源。
> 用例也重写成两层：① 断言这两张表**没有**账本触发器；② 手工注入一条假的 `app_settings`
> 账本条目，断言 `changes` 把它挡在门外、且游标照样跨过去。
>
> 📌 **实测记一条"为什么第一页可以是空的"**：真库副本上 `changes?since=0&limit=3` 返回
> `{cursor:2068, hasMore:true, upserts:{}, deletes:{}}` —— 一开始怀疑是 bug，用 Python 直接查
> 账本才看清：真库的 `_sync_log` **已被裁剪**（`MIN(seq)=2066`、`MAX=3328`、1263 行），
> 而 seq 2066/2067/2068 那三条是 `capture_runs 136776` / `prompts 311` / `prompt_tags 311:18`
> 的 **upsert，且这些行后来已经被删掉了**（`rowExists=0`），它们的 delete 条目落在被裁掉的区间里。
> 所以"跳过查不到的行"正是对的行为：手机若从窗口左沿之后开始，它本就不该有这些行；
> 而**已经持有这些行的手机必然游标更老 → 走 410 → bootstrap**，这条链正好闭合。
> 反过来说：**"空的一页"不等于"没有变更"**，`hasMore` 与 `cursor` 才是判据。
> 真实库验收：`since=5` → **410**（真库的账本确实已被裁剪，正好证明这个分支是必需的）；
> `cargo test` **64 例全绿**（25 lib + 39 集成，新增 5 例覆盖上述四条约束）。
>
> ✅ **P4 第 12 步（`7b169c7`，2026-09-24）**：`POST /api/sync/push`（契约 §4）—— **M4 的 DoD
> 就在这里**。三条纪律：**幂等**（重复推同一条靠"主键 UUID + `ON CONFLICT DO UPDATE`"就地更新，
> 第二次落 `STALE_IGNORED`，绝不新增行）、**服务端赢 LWW**（只有 `updated_at` 比桌面新才覆盖，
> 否则丢弃并记 `rejected` —— 不能让手机把桌面改过的行回退成旧值）、**`rejected` 逐条给 `code`**
> 且**永久性/暂时性分清**（给个"暂时"的码会让手机永远重试一条永远不会成功的行）。
> 另外 `prompts` / `media_assets` / `tags` / `prompt_tags` / `media_tags` / `capture_runs`
> 一律 `FORBIDDEN_KIND`（总纲 §3.3：那些由桌面在提交时产生）。
>
> 实现上两条选择：**列清单从 `PRAGMA table_info` 现查**（不维护手写清单 —— 漂移了就会静默少写
> 一列），列名先过字符集再引号包裹（payload 的键来自手机，这里是注入面）；**每条 item 各自一个
> 事务**（`BEGIN IMMEDIATE`，R2），坏行不挡同批其它行，手机才能逐条清 outbox。
>
> ★ **一处契约细节当场定下来**：**信封是 camelCase**（`kind` / `refId` / `deviceId`），
> 但 **payload 里是数据库列名（snake_case）** —— 契约 §2 的例子本来就是
> `{"id":12,"text":"…","updated_at":"…"}`。payload 的语义是"完整行直接覆盖本地行"，
> 再转一道驼峰就会与列名对不上（第一版测试就是这么写错的，被 `SCHEMA_MISMATCH` 抓出来）。
>
> **验收**：`cargo test` **69 例全绿**（25 lib + 44 集成），新增 push 5 例（幂等 / LWW 双向 /
> 逐条 code 6 种 / 单批上限 500→400 / **上行点亮账本且不写假 delete（R3）**）。
> **真实库副本抽查**（8085，只写副本）：`cursor` 3328 → push 一次 `applied=[conv-real-1]` →
> **再推同一条 `applied=0` + `STALE_IGNORED`** → `cursor` = **3329**（两次推送只多了 **1** 条账本
> —— 幂等在真实数据上成立）。
>
> 📌 `sync` 只剩 **`/api/sync/bootstrap`** 未做。⚠ 它有一处**契约留白需要先拍板**：§3 只说
> `?page=N` 与响应形状 `{table, page, pageSize, totalPages, rows[]}`，但**没说 `table` 怎么给**
> （是 `?table=prompts&page=1`，还是"page 的顺序遍历所有同步表"）。这决定手机侧的重建循环怎么写，
> **别猜着实现**——先定契约再写两端（M4 上线前必须定）。
>
> ✅ **P4 第 13 步（`f08d973`，2026-09-24）**：`GET /api/sync/bootstrap` —— **上面那条留白当场拍板了**，
> 四条同步端点**全部落地**（manifest / changes / push / bootstrap）。
> - 决定：**`?table=<表>&page=N&pageSize=N`，`table` 必填**，缺了回 `400` 并**列出合法表名**。
>   理由：只给 `?page=N`（把十几张表"拍平"成一串页）会让响应里的 **`totalPages` 失去意义**
>   （是全部数据还是这张表的？），而"第 N 页是哪张表"又要客户端自己算 —— 可契约偏偏让**服务端**
>   回 `table` 字段。`?table=` 让 `table` / `totalPages` / `rows` 三者自洽。
>   **这条决定已写进 🔒 `docs/sync-api-contract.md` §3**（标注"实现时定"），不是悄悄改行为。
> - 两条纪律：**绝不发 `.db` 文件**（只发那 10 张表，`app_settings` 等一律 400 —— 那里有配对令牌
>   明文）；**按主键稳定升序**（SQLite 不保证无 `ORDER BY` 的顺序，翻页顺序不稳就会重复或漏掉行）。
> - **验收**：`cargo test` **71 例全绿**（25 lib + 46 集成），新增 2 例：翻完所有页去重后**正好 5 条**
>   （不重不漏）+ 键集合就是契约 §3；凭据/令牌表一律 400 且响应里不含令牌字符串；缺 `table` 也是 400。
>
> ✅ **P4 第 15 步（`e3d7f7d`，2026-09-24）**：`GET /api/capture/locate` + `POST /api/capture/locate/apply`
> （用户"其他建议"第 3 条）。`ComfyLocation` 与 Kotlin 逐字同形（home / outputDir / source /
> configuredOutputDir / candidates[] / note）；**探测只读**（用例里专门断言"探一遍之后配置里什么都没多"），
> 是否生效由用户点「使用这个目录」决定。候选顺序照抄 `ComfyLocator.find`，**被否决的候选也记下来**
> （带原因）—— 那几行正是用户排错时要看的。`apply` 的两条准入照抄：不能为空 / 必须真的存在（都 400），
> 不然会写进一个打不开的路径，之后"捕获一条也收不到"而用户不知道为什么。
> 实现上把 `find_home` 改成 `locate(...).home` 的**薄包装**：探测逻辑只有一份，
> "列工作流"与"设置页探测"不可能漂移。
> - **真实验收**（真库副本，8088）：`home=D:\Comfy-Desktop\ComfyUI-Shared`、
>   `source=已经配置过了（就按这个用）`、`candidates=[]`（配过就不再探，与 Kotlin 同）。
> - 📌 **顺带印证了第 14 步"三份来源一条都不能少"**：真机上配置指向 `ComfyUI-Shared`，
>   而工作流文件在 `ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows` 下 ——
>   正是 ComfyUI Desktop"程序与共享数据分家"那种布局，**只按配置的输出目录推是推不到的**。
> - **验收**：`cargo test` **86 例全绿**（33 lib + 53 集成），新增 2 例（locate 形状/只读/候选与依据 +
>   apply 的 400 两个分支与"写入并回读 → 再 locate 变成'已经配置过了'"）。
>
> ✅ **P4 第 16 步（`0ddecbc`，2026-09-24）**：`GET /api/ai/providers` —— **`/api/ai/*` 的第一批**
> （总纲 §9 ③ 的顺序：只读、且不含密钥的清单起手）。新模块 `ai.rs`；`credential` 只有
> `{configured, source, writable}`、**没有值**（AIH-012）；判定照抄 `CredentialService.describe`
> 三分支并抽成纯函数（环境变量与托管文件都注入，所以每档都能单测），托管文件
> `<storage>\ai\credentials.dpapi.json` **只读键名**（DPAPI 的值不解、也不该解；
> 写"永远 false"就是假装没配过）。
> - ★ **用例抓出一个真会静默的错**：`rename_all = "camelCase"` 把 `base_url` 变成了 `baseUrl`，
>   而 Kotlin 的字段**就叫 `baseURL`** —— 差一个大小写，界面读到的 URL 就是空的。
>   精确键集合断言当场抓住，改成 `#[serde(rename = "baseURL")]`。
>   **这就是 §9 ③ 那条"别照着 DTO 猜"的实证**（而且这次猜错的正是最不容易怀疑的那一个）。
> - **真实验收**（真库副本 + 真凭据文件，8089）：2 条 provider，`baseURL` 正常，
>   `credential={configured:true, source:"managed", writable:true}` —— 从真的 DPAPI 文件读出键名，
>   没有解密、也没有把值带出去。
> - **验收**：`cargo test` **88 例全绿**（34 lib + 54 集成），新增 2 例（凭据三分支 +
>   端点形状与"密钥值一个字都不许出"：夹具往托管文件塞哨兵串，断言响应里搜不到）。
>> 📊 **P4 至此的端点覆盖（截至第 24 步）**：业务 `tags` / `prompts` / `media`（列表·详情·文件流·缩略图·上传·写侧）/
> `stats` / 捕获配置 / **本机工作流文件 + 位置探测（第 14·15 步）** / 工作流原文 / `api-graph`
> / **`/api/ai/providers`（第 16 步）** / **模型目录（第 20 步）** / **会话与消息读写（第 21·24 步）** /
> **`run-kit`（第 22 步）** / **`/api/auth*` + 密钥下行（第 23 步）** + **同步 4 条**全部可用。
> 仍未搬：`/api/capture/{status,jobs,poll,import,import-workflows}`（`status`/`jobs`/`poll` 要 ComfyUI 在场；
> `import-workflows` 要整套 `WorkflowConvert` + `AiWorkflowSearch` 转换栈，是**下一个大块**）、
> `/api/ingest/comfyui`、**工具层与审批（R8）/ 附件（R9）**、
> `/api/ai/{skills,memory,tools,nsfw,builtin,capabilities}/*`（界面维护用）。
> ✅ **Run 与 SSE 已落地**（第 26~30 步，见本节上半部分）—— 也就是说
> **桌面执行器已经能真的聊**（不带工具、不带附件），**R5/R6 已在 Rust 侧重建**。
>
> ✅ **P4 第 55 步（2026-09-26）：切流 C-1~C-4 —— 脚本与发布包真的指向 Rust 了**（4 个提交）
>
> **C-1/C-2：`server.ps1` 默认起 Rust**（`bb2354d`）
> - 新增共享选择器 `scripts\backend.ps1`（`server.ps1` 与 `comfyhub.ps1 doctor` **共读一份**）：
>   `-Backend auto|rust|kotlin` > `COMFYHUB_BACKEND` > `auto`；`auto` 的事实依据只有一条 ——
>   **Rust 二进制在不在**（发布包 `<根>\server\comfyhub-server-rs.exe` → 源码树 release → debug）；
>   找不到时**不报错、回退 Kotlin 并显式说一句**（回退绝不静默）。
> - **JDK 改成懒解析**：以前脚本开头就 `Resolve-Jdk`，找不到直接 throw ——
>   而"没装 JDK 21~23 的机器"正是发布包要支持的那一类，等于 **Rust 后端在一台没 JDK 的机器上起不来**。
>   现在只有真的要跑 Kotlin / Gradle 时才找它。
> - `Do-Stop` / `Do-Start` 的"已在运行"判断改成**认两个后端**：切流期间两边都可能在场，
>   只停"当前选中的那个"会让旧的继续占端口（表现是"健康检查打到旧进程上"）。
> - `doctor` 新增两行：**后端实现**（Rust/Kotlin + 正在跑的是谁）与**后端构建新鲜度**
>   （exe 比源码旧就黄牌 —— 与 AGENTS §1 那条"界面类修复先查构建新鲜度"同一个判据）。
> - **实测**：`pwsh -File scripts\comfyhub.ps1 up -SkipBuild` → 起的是
>   `server-rs\target\release\comfyhub-server-rs.exe`（port 4606，`db=ok`），
>   `.run\api.json` 写的是真实端口，**12 个关键端点逐个抽查全通**
>   （health / tags / prompts / media / stats / ai.providers / ai.tools(56) / run-kit /
>   conversations / sync.manifest / capture.status / providers.models）。
>   ⚠ 过程中抓到一个真 bug：`$Script:Backend` 与参数 `$Backend` **同名**，
>   作用域赋值把 Hashtable 塞进了带 `[ValidateSet]` 的参数 → `server.ps1` 当场抛
>   "The variable cannot be validated…"。改名成 `$Script:BackendInfo` 解决。
>
> **C-3：打包只装 Rust 二进制**（`5671e31`）
> - `packaging\manifest.json`：`backend` 组件换成 **`rust_binary`**（装配到 `<根>\server\comfyhub-server-rs.exe`）；
>   `jre` 组件 `required: false` + `optional: true` + **默认不装**（要回退旧后端才 `-WithJre`）；
>   `runtimeRequirements` 里 Java 那条标成 `legacy`。
> - `pack-release.ps1`：新增 `rust_binary` 分支 + `Build-RustRelease`（**构建新鲜度优先**：
>   源码比 exe 新才编，免得每次打包白等三分钟）+ 自检项换成 Rust exe（jar / java.exe 只在 `-WithJre` 时要求）。
> - **实测**：`pack-release.ps1 -SkipApp` 装配出 **13.8MB**（Rust exe 12.8MB），旧的 jre(40MB)+jar(20MB) 全没了。
>
> **C-4：建表/迁移归 Rust（切流里最容易漏的一条）**（同一个提交）
> - 原实现遇到缺库直接 `exit 3`（"先让 Kotlin 起一次库"）—— 那**在发布包里等于永远起不来**
>   （包里根本没有 Kotlin）。新增 `db::open_or_create`：库缺/空 → 建父目录 + 建库 + 跑
>   `db/schema.sqlite.sql`（**编译期 `include_str!` 嵌进二进制**，与 Kotlin 从 jar 里读同一性质）
>   + 写 `user_version`；`user_version == 0` 的老库 → 收敛式跑一遍 DDL；已最新 → **绝不重跑**；
>   库比后端新 → **只警告不拦**（拦了就没法回退版本）。
> - **验收**：`cargo test` **422 例全绿**（279 lib + 121 api_test + 4 attachments + 3 import_folder
>   + 7 provider_probe + 2 shape_lint + 6 submit_test；新增 4 例：全新库 / 已最新不动数据 /
>   无版本号老库收敛 / 更新的库只警告）；**全新目录实测**：`.db` 不存在 → 自己建库建表
>   （schemaVersion=1）→ 写端口文件 → `health.database=ok`；**发布包里的 exe 单独跑同样成立**。
>
> ✅ **P4 第 53 步（2026-09-26）：Provider 连接测试 + 模型发现（A2，设置页那两个按钮）** ——
> 新模块 `server-rs/src/provider_probe.rs`，外加 `upstream::refine_from_body`（Kotlin `AiUpstream`
> 那一段的移植，**与 LLM 调用共用一份**）。A 组（App 直接调用的端点）**至此清零**。
>
> - **四条纪律逐条落地**：① **Base URL 带不带 `/v1` 的两种写法都试**，并把"实际可用的是哪个地址"
>   回报给界面（填错一层就是"连接失败"，而 Key 其实是对的）；② **`/models` 回 404 不算鉴权失败** ——
>   有些网关根本没有列表端点，只要不是 401/403 就认为连通（`MODEL_LIST_UNAVAILABLE` 是内部标记）；
>   ③ **密钥绝不进日志、不进响应**（错误正文先 `redact` 再截断，AIH-051）；④ **能力逐维度合并、不猜**
>   （AIH-011）：接口明确声明的维度用接口的，没声明的用内置目录，两边都没有才用保守默认
>   （仅文本 + 工具默认给上）。
> - ★ **移植时抓到两处"照着直觉写就会错"的地方**（都写进了代码注释与用例）：
>   ① **两家的列表地址规则不一样**：OpenAI 兼容是"Base URL 后面直接拼 `/models`"，
>      Anthropic 才把 `/v1` 归一化 —— 我第一版按"OpenAI 也补 `/v1`"写，候选序列整个反了；
>   ② **混装数组 `capabilities: ["vision","tools"]` 里的 `"tools"` 是个副作用**：
>      Kotlin 对**每个**候选键都真的解析一遍（只有"模态"的赋值带条件），
>      我第一版写成"模态已定就跳过这个键"，于是接口声明的工具能力**静默丢失**（候选里是默认值，
>      界面上完全看不出哪里错）。两条现在都有用例钉住。
> - **验收**：`cargo test` **406 例全绿**（269 lib + 121 api_test + 4 attachments_test + 3 import_folder_test
>   + 7 provider_probe_test + 2 shape_lint + 6 submit_test）；`provider_probe` 6 例纯函数
>   （地址候选 / 数模型 / 细化错误码 / 逐维度声明 / 候选合并与来源 / 内置目录命中）+ 集成 7 例
>   （真 socket 假上游：连通与 `modelCount`、**只有 `/v1/models` 通时的地址回退**、两个候选都 404 算连通、
>   401 报 `MISSING_CREDENTIAL`、**缺凭据时连请求都不发**、发现**不落库**、provider 不存在给人话）。
> - **真机实测（真库副本 8105 + 用户真凭据）**：`deepseek` → `{"ok":true,"message":"连接正常（HTTP 200）",
>   "modelCount":2}`，发现 2 个候选（`contextWindow=1048576`、`maxOutputTokens=393216`、
>   思考档位与方言 `deepseek` 来自内置目录）；`command-code-goat` → `HTTP 200 / modelCount=82`，
>   发现 **82 个候选：接口声明 0 / 内置目录 77 / 未识别 5**（正是"接口不给能力 → 内置目录补"那条规则）；
>   不存在的 provider → `400 {"error":"CONFIG_ERROR","detail":"Provider 不存在: nope"}`。
>
> ✅ **P4 第 52 步（2026-09-26）：`POST /api/capture/import`（导入目录里已生成好的产物，A3 的最后一格）** ——
> 两个新模块：`server-rs/src/png_text.rs`（Kotlin `PngMeta` 的逐条移植：`tEXt` 明文 /
> `zTXt` zlib / `iTXt`（含压缩）三种块全认，CRC 不校验、单块体积有上限）+ `capture_import.rs`
> 的 [`import_folder_json`]。
>
> - **为什么这块重要**：ComfyUI 把**这一次生成用的节点图**写进 PNG 的 `prompt` 文本块里 ——
>   读出来，"一张图"就还原成"一条带参数的提示词"；读不出来，导进来只是一堆没有来历的图片。
>   zlib 解压用 `miniz_oxide`（**纯 Rust**，不引 C 后端：发布包要便携）。
> - **三条纪律逐条对齐 Kotlin `importFolder`**：① 按内容 sha256 判重（同一份文件重复导入 →
>   `duplicates++`）；② **不许把自己 storage 里的文件再导一遍**（`media`/`thumbs`/`ai-attachments`/
>   `ai-thumbs` 全跳过）；③ 单文件失败不整批失败（进 `errors`，**不静默跳过**）。
>   幂等靠 `run_key = import:<sha 前 40 位>`，而且这条路**用 `force` 抢占**：上面刚用 sha256
>   确认过文件不在库里，历史运行记录不该拦住重新导入；导入失败时把抢占到的行删掉
>   （否则下次导入会被判成"正在处理中"）。
> - **验收**：`cargo test` **399 例全绿**（263 lib + 121 api_test + 4 attachments_test + 3 import_folder_test
>   + 2 shape_lint + 6 submit_test）；`png_text` 5 例（三种块 / 压过的 iTXt / 畸形文件不崩 / 同键留先到的）+
>   `import_folder_test` 3 例。集成用例用 `png` crate 写**带真 tEXt 块的真 PNG**（CRC 由它算），
>   所以"读内嵌图 → 还原参数"这条路是真跑过的；顺带抓到一个语义事实：
>   **像素相同的两张图（不同文件名）会被判成同一份内容** → 用例夹具必须让每张图的像素不同。
> - **真机实测（真库副本 8104，对着用户那个有 306 张 png 的产物目录）**：
>   `{"scanned":3,"imported":3,"duplicates":0,"failed":0,"promptsCreated":3,"promptIds":[333,334,335]}`；
>   再导一次 → `imported=0, duplicates=3`（幂等）。还原出来的提示词逐条对：
>   `seed=875817230929465`、`checkpoint=anima-preview3-base.safetensors`、正向提示词正文、
>   `source=ComfyUI-Import`、`source_ref=import:1e4f926b…`（40 位 sha 前缀）—— **都是内嵌图里的事实，不是猜的**。
>
> ✅ **P4 第 51 步（2026-09-26）：视频封面 `GET /api/media/{id}/poster`（A5）** ——
> `server-rs/src/poster.rs`：只做"找到脚本、起子进程、看结果"，**不重写抽帧逻辑** ——
> 抽帧走 `scripts\video-poster.ps1`（Windows 资源管理器那条缩略图管线 / Media Foundation，
> **不依赖 ffmpeg**），重写一份就等于两套行为，而用户看到的是"画廊里的视频有封面、聊天里的没有"。
> 四种结果一条都不能混（Kotlin 同）：产物不存在 → **404**；不是视频 → **204**；
> 视频但抽不出帧（脚本失败 / 没有 pwsh / 没有脚本）→ **204**（前端退化成"播放器转圈"，**不是破图**）；
> 有封面 → 200 + `image/png` + `X-Poster: 1`。抽帧在阻塞线程池上跑（要起子进程，别占事件循环），
> 缓存在 `<storage>/thumbs/<id>.poster.png`（与图片缩略图 `*.jpg` 分开存，互不覆盖）。
> - **验收**：`cargo test` **392 例全绿**（258 lib + 121 api_test + 4 attachments_test + 2 shape_lint + 6 submit_test；
>   `poster` 3 例 + 两处旧断言从"501 未搬"改成真实语义）。
> - **真机实测（真库副本 8103 + 用户库里那条真 mp4）**：第一次请求 `/api/media/49/poster` →
>   **200 + `image/png` + 322386 字节 + `X-Poster: 1`**，缓存目录里出现 `49.poster.png`；
>   读 PNG 头确认尺寸 **640×366**（正好是脚本 `-Size 640` 的最长边）；第二次请求走缓存（同样的 322386 字节）；
>   图片产物 → **204**；不存在的产物 → **404 + `{"error":"产物 999999 不存在"}`**。
>
> ✅ **P4 第 50 步（2026-09-26）：AI 附件（**R9 在 Rust 侧的落点**）** —— 两块，两个提交。
>
> **（上）地基**：`server-rs/src/file_kind.rs`（`FileKindDetector` 的逐条移植：**签名优先**、
> ISO BMFF 按品牌分图片/视频、纯文本要扩展名 + 内容嗅探**同时**成立、认不出来就是 `Unknown`
> 且**一律阻断** —— 绝不做"未知即图片"的乐观回退）+ `server-rs/src/attachments.rs`
> （库 / 文件两层 + 五条端点）。原件落 `<storage>/ai-attachments`、缩略图落 `<storage>/ai-thumbs`，
> 与画廊产物**分开**存（生命周期不同，混在一起以后清孤儿会互相误删）。
> `/file` 复用画廊那份 `serve_path`（**ETag = sha256 + `If-None-Match` 304 + 单段 Range 206**）——
> 抽成公共函数就是为了"聊天里的视频能拖、画廊里的也能拖"，两处各写一份必然漂移。
> 缩略图两级降级（JPEG 缩略图 → 解不了就发原件）；**视频预览帧回 204**（还没搬，见 A5），
> 界面按既有约定退化成文件图标，**不假装**抽过帧。
>
> **（下）三层准入 + 真内联**：`server-rs/src/strict_intake.rs`（`StrictIntake` / `AttachmentPolicy` /
> `InlineBudget` / `AdapterCapabilities.transportsFor` 的逐条移植）+ `POST /api/ai/preflight` +
> `POST /runs` 前的闸门 + **适配器真的把图片内联进请求体**：
> * 三家适配器各按自己的形状铺开（`image_url` data URL / `input_image` / `source.base64`），
>   新增用例 `image_blocks_shape_per_adapter` 逐条断言"发出去的 JSON 长什么样"
>   （AGENTS §10 的纪律：加新模态就要有这条用例）；
> * **模型没声明传输方式时回落到适配器实现的那种**（不回落的后果是内置目录里那些模型
>   "图片永远发不出去"）；
> * **内联预算**：单张 ≤ 8MB、合计 ≤ 20MB，贪心保序；
> * 发不出去的那些（模态不支持 / 协议没实现 / 超预算 / 读不到文件）**一律**写进那一轮的
>   "（以下附件未随本次请求发送：…）"——**不许静默丢掉**；系统提示里那句"附带图片附件"
>   也只在**确实内联了**时才说"有"。
> * **R9 的核心不变式**：创建 Run 之前用**库里的事实**再验一次，不通过 → `400 UNSUPPORTED_CONTENT`
>   + **上游请求数为 0**（用例断言网关一个请求都没收到、会话里一条消息都没落）。
>
> ⚠ 与 Kotlin 的两处**诚实差距**（都写进了文件头）：① 视频 / 音频 / 文档的传输方式两家都没有
> （三家适配器都只有图片的 `inline_base64`）；② 视频预览帧（Kotlin 走 Windows 缩略图管线）没搬。
>
> **验收**：`cargo test` **388 例全绿**（255 lib + 121 api_test + 4 attachments_test + 2 shape_lint
> + 6 submit_test）；`file_kind` 6 例、`strict_intake` 6 例、`attachments` 7 例、协议形状 2 例、
> R9 集成 4 例。**真机真 socket 实测**（真库副本，8102）：上传两张（PNG 13×9 / md）→ 201 +
> 尺寸与 MIME 都对；假图（声称 png、内容是二进制）→ `400 {"error":"UNSUPPORTED_CONTENT","detail":
> "evil.png：无法识别文件 evil.png 的真实类型（签名与声明的 image/png 不符），已拒绝上传"}`；
> `/file` 回 200 + `ETag=sha256` + `inline` 头、`Range: bytes=0-9` → **206** + `Content-Range`、
> `If-None-Match` → **304**；`/thumb` → 200 + `image/jpeg` + `X-Thumbnail: 1`（635 字节）；
> 不存在的 id → `400 CONFIG_ERROR + detail`；`DELETE` → `{"deleted":true,...}`、再删 → 400。
>
> ✅ **P4 第 49 步（2026-09-26）：推送式捕获 + 批量导入工作流**（`server-rs/src/capture_import.rs`）——
> 这两条是"库里怎么才会有东西"的另一半：轮询与 `comfy_submit` 收的是"跑过的运行"，
> 而它们收的是**外部推过来的运行**（ComfyUI 自定义节点 / e2e 脚本）与
> **用户早就存在盘上的工作流文件**（用户 bug ⑤：首次使用时库必然是空的）。
>
> - **`POST /api/ingest/comfyui`**：请求体就是 Kotlin 的 `IngestRequest`（驼峰键，含 `extra.nsfw`），
>   与轮询走**同一条路**（`begin_run` → `ingest_claimed`）：抢不到就**如实**回 `alreadyCaptured`
>   + 已有的 `promptId`，绝不重复入库。⚠ 这层"驼峰 → 内部 snake_case"的映射就是两端形状的唯一接触点，
>   写错了**不会报错**、只会"产物一条都收不进来"，所以键名逐个有用例钉住（`item["type"]` / `nodeId` 数字也要读）。
> - **`POST /api/capture/import-workflows`**（`{dir?, limit?, query?}`）：`dir` 不填就按
>   `comfy_dirs::workflow_dirs` 自动发现，填了就当它是那个目录本身（不存在 → 400 人话）。
>   逐条结果如实分开：`imported` / `duplicates` / `failed` + 每条 `{name, path, status, promptId?, runnable?, reason?}`；
>   **幂等靠 `run_key = file:<sha256>`**（与 `comfy_load_workflow` 同一把钥匙），批量一律
>   `tolerateUnsupported = true` —— 批量时"转换不了"绝不能让整份工作流消失（它照样是用户库里的资产，
>   能不能跑由 `runnable` 如实标出来）。
> - **验收**：`cargo test` **362 例全绿**（233 lib + 121 api_test + 2 shape_lint + 6 submit_test；
>   新增 4 例：驼峰映射逐键、`extra.nsfw` 与顶层 `nsfw` 都认、缺 `runKey` → 400 人话、目录不存在 → 400 人话）。
> - **真库副本实测（8101，只写副本）**：
>   ① 推一条合成运行（真 PNG + `extra.nsfw=true`）→ `{"promptId":333,"mediaIds":[228],"imported":1}`，
>   库里 `media_assets.nsfw=1`、`prompts.source_ref='verify-push-0001'`；**再推一次** →
>   `alreadyCaptured:true`（幂等）；缺 `runKey` → `400 {"error":"runKey 不能为空"}`。
>   ② 批量导入**真的用户 workflows 目录**（17 份界面格式文件）：ComfyUI 没在跑时 **17 份全部如实报
>   `COMFY_UNREACHABLE`**（"这是界面格式工作流，转换需要问 ComfyUI 要节点定义"）—— 不静默跳过、不假成功；
>   接着起假实例把 `comfyUrl` 指过去，导入项目 `comfyui\` 那 11 份：**第一次 imported=11 / failed=0，
>   第二次 duplicates=11 / imported=0**（`file:<sha256>` 判重成立）。
>
> ✅ **P4 第 48 步（2026-09-26）：`comfy_submit`（**R10 在 Rust 侧的落点**）** ——
> 新模块 `server-rs/src/workflow_submit.rs` 逐条对照 `ComfySubmitter`（提交 + 等 + 状态）、
> `ComfySubmitter.WorkflowEdit`（参数覆盖 / 补线）、`AiWorkflowSearch.submit`（工具层的编排与返回的键集合）。
>
> - **只认 API 格式节点图**（R10 一个字没松）：图从 `prompts.source_ref` → `capture_runs.raw` 那条链取
>   （[`capture::prompt_api_graph`](第 9 步)）；取不到就报 `NO_API_GRAPH` 并说清"可以换一条 runnable=true 的，
>   或先在 ComfyUI 里手跑一次让它被捕获"，**绝不猜着造一张**。`workflowPath` 那条路走
>   `workflow_load::load_from_file(…, tolerateUnsupported=true, emulateFrontend=true)`（与 Kotlin 同一档）。
> - **提交后自带"等 + 立即入库"**（O4 的同一条纪律）：`POST /prompt` → 每 1.5 秒查 `/history/<id>`
>   → 跑完立刻 `captureRun`（= `begin_run` + `build_ingest_request` + `ingest_claimed`，与轮询**同一条路**，
>   幂等靠 prompt_id + sha256）。为什么不能只靠轮询：ComfyUI 的 `/history` **是进程内的**，
>   重启实例之后上一次运行就查不回来了。抢不到（轮询先收走了）**如实**回 `alreadyCaptured` + 已有的
>   `promptId` / `mediaIds`（Kotlin `captureRun` 同）。
> - **`overrides` / `connections`** 两个纯函数逐条移植：键是 `节点id.输入名`；覆盖**按原值类型转换**
>   （"原来是整数就不能塞非整数"）、**只允许改本来就有的输入**、错了**抛人话**（绝不静默忽略）；
>   补线允许写当前不存在的输入名（那正是要补的位置），但**上游节点必须在图里真的存在**。
> - **超时不等于失败**：等满 `waitSeconds`（默认 240、上限 900）还没完就回 `timeout` +
>   "跑完之后会自动被捕获，稍后可以在画廊里看到"，既不假装完成也不假装失败。
> - **提交前的两道实例侧预检**（放在 overrides / connections **都应用完之后** —— 模型常常就是在这一次
>   覆盖里把模型名换成能用的那个）：① `class_type` 不在 `/object_info` 里 → 拒；② 类名含 `Loader` 的节点、
>   输入是字符串、而 `/object_info` 给该输入列的候选里有它没有 → 拒。两条都给"该干什么"（找用户装节点 /
>   **重启 ComfyUI，别下载**）。**拿不到 `/object_info` 就一个都不拦**（Kotlin 同：误拦比白跑更糟）。
>   ⚠ 与 Kotlin 的差别如实记在文件头：Kotlin 还做七项 `WorkflowAudit` 与盘扫（`ComfyModels.inventory`），
>   Rust 这一版**只有这两道**，措辞也只说"实例看不到"—— 宁可比 Kotlin 少说，也不谎报检查过的范围。
> - `/api/capture/jobs` 的 `submissions[]` 从此**真的有内容**（第 47 步时还是如实空数组）：
>   记录按 `submittedAt` 倒序、同 `promptId` 就地更新、最多 30 条，形状与 Kotlin `ComfySubmission` 逐字一致
>   （有默认值的键一律出现、可空键为 null 时整键省略）。
> - **验收**：`cargo test` **358 例全绿**（229 lib + 121 api_test + 2 shape_lint + **6 条新的 R10 集成用例**）。
>   新用例 `tests/submit_test.rs` 自带一台**进程内的假 ComfyUI**（axum，`127.0.0.1:0`，真 socket）：
>   ① 提交→等→入库→`mediaIds` 整条链（并断言**提交出去的那张图**里确实有覆盖与补线、
>   入库解析出的正向提示词来自**真正提交的图**、产物挂在新提示词上）；
>   ② `nsfw=true` 一路走到 `media_assets.nsfw`，`nsfwMediaIds` 回给界面；
>   ③ 预检拦截时**一个字节都不许发出去**（假实例记的提交列表为空）；
>   ④ `/history` 永远空 → 如实报 `timeout` + `hint`；⑤ 没有 API 图 → `NO_API_GRAPH`；
>   ⑥ 缺参数 / 覆盖了不存在的输入 → `INVALID_ARGUMENT` 且说清现有输入名。
>   `tools` / `desktop_tools` / `api_test` 里那些"还没搬的名字"清单也一并更新
>   （`comfy_submit` 从"不许出现"挪进"必须出现"）。
>
> ✅ **P4 第 47 步（2026-09-26）：后台捕获轮询器（O5 / O6）** —— `server-rs/src/capture_poller.rs`。
> 此前 Rust 侧只有"端点/工具触发一次"的抓取，于是**用户自己在 ComfyUI 里跑的东西永远不会被收**，
> 而 `/api/capture/status` 的 `lastPollAt` / `lastError` 只能如实回 null。现在两者都补上了，
> 行为逐条对齐 Kotlin `ComfyCapture.start()`：启动先等 **2.5 秒**再跑第一轮、
> `enabled=false` 时**每 5 秒空转但不退出**（用户在设置里打开就立刻生效，不必重启后端）、
> 间隔 = `pollSeconds` 且**夹在 1..600**、**同一时刻只跑一轮**（上一轮没结束就让开）、
> 成功写 `lastPollAt` 并**清空** `lastError`、失败写人话（`comfy_client::friendly_error`）**不假装成功**。
> 一处刻意不同并写进了文件头：`lastPollAt` 存的是 **ISO-8601 文本**（与整库时间列同一个出口
> `iso_time::from_system_time`），不是 Kotlin 那个内存 `Instant` —— 它要经 `/api/capture/status`
> 出去给 Dart 解析，形状必须与别的可空时间键一致。
> - **验收**：`cargo test` **346 例全绿**（223 lib + 121 api_test + 2 shape_lint；新增 4 例：
>   停用时空转且不假装轮询过 / 连不上时如实记 `lastError`（且**不写** `lastPollAt`）/
>   成功会清掉上一次的错误 / 间隔夹在 Kotlin 同一档）。
> - **真机端到端（假 ComfyUI + 真库副本，8100）**：把副本的 `capture.config` 指向
>   `scripts\e2e\fake_comfy.py`（18188）之后**什么都不点** —— 轮询器自己就在跑：
>   ① 先收到 fixture 种下的那条运行（`capturedRuns` 152 → **153**，`lastPollAt` 落成
>   `2026-09-26T00:28:45.052255200Z`，**9 位小数**，正是 Java `Instant.toString()` 的形状）；
>   ② 再向假实例 `POST /prompt` 造一条新运行、把产物文件换成**内容不同**的真 PNG，
>   轮询器**自己**把它收进了库：`media_assets` 26 → **27**、`capture_runs` 153 → **154**、
>   `lastError` 保持为空 —— 这正是 O5 要的那件事（"用户自己跑的东西也要进画廊"）；
>   ③ 把 `comfyUrl` 改成没人听的 `http://127.0.0.1:9`：7 秒后
>   `comfyReachable=false` + `lastError="连不上 ComfyUI（http://127.0.0.1:9）：连接被拒绝 —— ComfyUI 没在运行？"`，
>   而 `lastPollAt` **保留上一次成功的值**（Kotlin 只在成功时写）。
>
> ✅ **P4 第 46 步（2026-09-26）：角色卡五条端点**（`server-rs/src/character_cards.rs`）——
> 界面上那个从「提示词」页右上角进的**角色卡页面**（2026-09-26 那一轮 `2800eda`）此前只对着 Kotlin
> 能用，指向 Rust 就是"接口不存在"。这一步把它补上，**实现逐条对照 `CharacterCardRepo.kt` + `CharacterCardRoutes.kt`**：
>
> - 五条端点与状态码逐字同形：`GET`（`?q=` 关键字 / `?limit=`，默认 50、夹在 1..500）、
>   `POST` → **201** + 卡、`GET /{id}`、`PUT /{id}`（**全量替换** —— 界面弹窗里所有字段都在）、
>   `DELETE /{id}` → `{"deleted": id}`；不存在一律 404。
> - **三条纪律一条不改**：① **同名 = 同一张卡**（靠 `normalized` 唯一索引 + 合并保存，
>   **不用** `INSERT OR REPLACE` —— R3 会写假 delete）；② **来源必须如实**（`source` /
>   `tagsVerifiedAt` 是"我查过 danbooru"还是"我按印象写的"的唯一凭据，核对时间由真去查的那条路写）；
>   ③ **写不进去就报错、不许静默截断**（名字为空 / 任一字段超长 → 400 + 人话）。
> - `merge()`（AI 的"按名字合并保存"）与 `save()` 一并搬了：**空字段保留上一轮的值**、
>   `tags` / `aliases` **追加去重** —— 模型一次只说一件事，全量替换会把它上一轮存的 tag 栈悄悄抹掉。
> - `list` 的搜索命中十个列（名字 / normalized / 英文名 / 别名 / 角色 tag / 作品 tag / tag 栈 /
>   LoRA 名 / LoRA 触发词 / 备注），与 Kotlin 那份 `LIKE` 清单逐列对齐。
> - ⚠ **可空列照样"整键省略"**（Kotlin 的 `explicitNulls = false`）：`negatives` / `seriesTag` 之类
>   为 NULL 时键不出现，空数组（`aliases` / `tags`）**照样给键** —— `tests/shape_lint.rs` 盯着这条。
> - **验收**：`cargo test` **339 例全绿**（216 lib + 121 api_test + 2 shape_lint；新增 8 例：
>   建卡形状 / 同名合并不抹上一轮 / 全量替换清字段 / 超长报错 / 参考图不存在给人话 /
>   tag 清洗与拆分 / 搜索命中十列 / 删除与不存在）。用例**灌的是真 DDL**（`db/schema.sqlite.sql`），
>   所以 `normalized` 唯一索引这类承重约束真的在测。
> - **真库副本抽查**（8097~8099，只写副本）：空列表 → 建卡（回 11 个键、无 `null`）→ 详情
>   → 按别名"真红"搜得到 → `PUT` 全量替换（`characterTag` 键随之消失）→ 四个错误分支
>   逐条对着 curl 看：`404 {"error":"角色卡 999999 不存在"}`、
>   `400 {"error":"参考图（产物 999999）不存在；…"}`、
>   `400 {"error":"备注 超出上限（4001 > 4000 字符）；请拆成多条，不要指望被悄悄截断"}`、
>   `400 {"error":"角色名不能为空"}` → `DELETE` 回 `{"deleted":3}`、列表回到 1 条（用户真库里那张）。
> - ⚠ 记一条**两端已知差异**（与第 20 步 `media/{id}/prompt` 同类，**不是回归**）：
>   ① 404 的 body 在 Kotlin 会被 StatusPages 改写成 `{"error":"接口不存在: <路径>"}`，
>   Rust 给原话（Dart 只判状态码，行为无差异，Rust 那句更有信息量）；
>   ② **请求体 JSON 不合法**时 axum 默认回**纯文本** 400（`Failed to parse the request body as JSON: …`），
>   而 Kotlin 走 StatusPages 回 JSON 信封 —— 这是**所有** axum `Json` 端点共有的老行为（不是这一步引入的），
>   界面自己发的永远是合法 JSON，所以只在手敲 curl 时看得见。
> - 仍未搬（诚实清单）：**AI 侧那两件要联网 / 要提示词库的工具** ——
>   ✅ 同批已搬 **`search_character_cards` + `save_character_card`**（`renderCard` / `cardJson` /
>   `splitTags` 形状逐字对照；`save` 就是"同名合并 + 追加去重"那条路；
>   ⚠ `tagsVerifiedAt` **一律留空并如实写一句** —— 它只认"本轮真查过 danbooru 的 tag"，
>   而 `danbooru_lookup` 还没搬；搬它时必须把"本轮查过的 tag 集合"接进 Run 上下文，**不许自己编时间**）；
>   ⛔ `search_prompts`（要提示词库搜索那几个参数：`query` / `id` / `limit` / `includePrompt`）
>   与 `danbooru_lookup`（要 danbooru 客户端 + 短 UA + `DANBOORU_CLOUDFLARE` 那一档）归下一批。
> - 这一批往后 `cargo test` **342 例全绿**（219 lib + 121 api_test + 2 shape_lint）。
>
> ✅ **P4 第 45 步（2026-09-26）：MCP 桥（官方 comfy-mcp 的 39 个工具）** —— 第 45 步本来是
> §9 起手顺序里的第 3 件（"AI 能不能真的生图"那一档），也是 §10.1 的 O1/O2 与 §10.5 的 O12 的落点。
> 新模块 `server-rs/src/mcp.rs` 逐条对照 Kotlin 的三份实现 + `ComfyEnv.prepareLaunch`。
>
> - **协议层**（`McpClient`）：`initialize` → `notifications/initialized` → `tools/list` → `tools/call`，
>   把 `comfy-mcp` 当子进程驱动。传输层做成**读一半 / 写一半的泛型**（`from_io`）：
>   生产接子进程 stdio，测试用 `tokio::io::duplex` 接一台脚本化假服务器 ——
>   于是协议每一处（噪音行、通知、超时、`isError`、stderr 尾部）都能在**不起 Python** 的前提下测到。
>   五条纪律逐条落地：**进程只起一次**（注册表缓存客户端与清单）、**不弹控制台窗口**
>   （`creation_flags(CREATE_NO_WINDOW)` —— 这条路 Rust 是正规的，Kotlin 只能借 Swing 把 JVM 变成
>   GUI 子系统）、**stdout 只认 JSON-RPC**（噪音行跳过但留痕）、**stderr 单独排空并留尾部 N 行**、
>   **超时按工具区分**（`run_workflow` 那一档 15 分钟，其余 120 秒）。
> - **O12/O13（失败要能看到真因）**：`call_tool` 失败或服务端 `isError` 时，把
>   **子进程 stderr 末尾 12 行**拼进 `ToolFailure.message` —— 少 `COMFY_BIN` 时 MCP 只回
>   `Error executing tool xxx`，真因（`ComfyCliError: …`）一直埋在 stderr 里。
> - **O1/O2（起实例要挂共享模型根）**：`prepare_launch` 是 `ComfyEnv.prepareLaunch` 的 Rust 版 ——
>   `launch_comfyui` 的参数由**我们**补上 `--extra-model-paths-config` / `--input-directory` /
>   `--output-directory`（值来自 `%APPDATA%\Comfy Desktop\settings.json` + `shared_model_paths.yaml`），
>   **模型自己写过的参数一律不覆盖**，补了什么如实回给模型；拿不到 Desktop 配置就**如实警告**
>   （"这次启动的实例看不到共享模型，别据此断言本机没有某个模型"），绝不编参数。
>   `extra_args` 三种形状都认（数组 / JSON 字符串 / 空格分隔）—— 现场实测模型爱写字符串那种。
> - **R12 的接法**：MCP 工具是**运行时**拉来的，而 `ToolView`/`IMPLEMENTED` 是静态的。
>   刻意**不重构工具层**：拉到清单后把 name/description/schema 一次性 `Box::leak` 成 `&'static`
>   （39 条、几十 KB，进程生命周期内固定），于是 `/api/ai/tools`、系统提示、下发的 `tools[]`
>   继续走同一条既有管线；新增 `tools::catalog_with` / `specs_with` 把两个来源合并后仍**按 name 升序**。
>   权限档照 Kotlin `McpToolPolicy`：`update_comfyui` / `install_node` / `download*` = `DENY`，
>   `launch_comfyui` / `run_workflow` / `generate_image` 那一批 = `ASK`，"其余的只读查询" = `ALLOW`。
>   **拿不到 MCP 运行时不算错**：清单里就是空的（与 Kotlin "注册不上就没有这些工具"一致），
>   绝不"下发一个必然失败的调用"。
> - **验收**：`cargo test` **331 例全绿**（208 lib + 121 api_test + 2 shape_lint；新增 16 例：
>   握手与清单 / 结果按块拼起来且非文本块留占位 / `isError` 如实带回 / 噪音行跳过 /
>   不回复就是 `MCP_TIMEOUT` / 服务端报错翻稳定码 / 服务端主动请求收到 `-32601` /
>   stderr 尾部保留 / 读循环结束让在等的调用当场失败 / 权限档 / `scripts` 目录两种布局 /
>   `extra_args` 三种形状 / 没有 Desktop 配置如实警告 / 补挂三参数 / 不覆盖模型自己写的 / 不存在的目录不补）。
> - **真机验收（对着本机真的 comfy-mcp 0.10.0 + comfy-cli 1.20.0）**：起 Rust 后端（8096、**真库副本**、
>   `COMFYHUB_PROJECT_ROOT` 指向真项目根）→ `GET /api/ai/tools` 回来 **53 个**工具
>   （14 个静态 + 39 个 MCP），其中 `system_stats` / `nodes` / `run_workflow` / `launch_comfyui` /
>   `get_logs` 等 28 个特征名字都在，`category=comfy`。
>   也就是说**起子进程 + 握手 + 拉清单这条真路径是通的**（Python 3.12.10 便携版、`state.json` 里那份）。
>   ⚠ 诚实边界：这一步只验到**工具清单**，还没有"让 AI 真的调一次 `run_workflow` 出图"的端到端
>   （那需要真 provider 走一轮 Run，留给下一次真机验收）。
>
> ✅ **P4 第 14 步（`1108673`，2026-09-24）**：`GET /api/capture/workflows` —— 用户 bug ⑤ 那条路
> （`prompts` 库里没有 ≠ 用户机器上没有）。新模块 `comfy_dirs.rs` 逐条对照
> `ComfyWorkflowFiles.kt` + `ComfyLocator.kt` + `ComfyRoots.kt`，**三个目录来源一条不少**：
> ① 配置里生效的输出目录的父目录 ② `ComfyLocator.find()` 探到的 home ③ `ComfyRoots.autoReadRoots()`。
> 少任何一条都是"正常返回、只是少了几个目录"这种**静默**故障（§9 ② 那条警告）。
> `scan` 只列真的能用的（坏 JSON / 空 / 非 `.json` / 格式认不出来的一律跳过）；
> `sha256` 与 `comfy_load_workflow` 的 `run_key = file:<sha256>` 是**同一把钥匙**，
> 所以 `inLibrary` / `promptId` 天然幂等。`/api/capture/import-workflows` **仍是 501**（没有假装成功）。
> - ★ **真库副本抽查又抓到一个输出形状回归（已修）**：`std::fs::canonicalize()` 在 Windows 上
>   返回**逐字前缀** `\\?\D:\Comfy-Desktop\…`，而 Kotlin 的 `toRealPath()` 返回 `D:\Comfy-Desktop\…`。
>   这个前缀会原样出现在 `dirs` 与每个文件的 `path` 里（用户和 AI 都看得到，还会被贴进对话当参数），
>   是"同一个目录、两种写法"那类最难查的不一致 —— 与第 11 步"空的一页不等于没有变更"一样，
>   **只有真库/真目录才摆得出来**。现在所有 `canonicalize` 站点统一过 `plain_path()`，并有用例盯住。
>   同批抽查顺带确认 FK 真的在生效（夹具里塞不存在的 `prompt_id` → 当场 `FOREIGN KEY constraint failed`）。
> - **验收**：`cargo test` **84 例全绿**（33 lib + 51 集成）；真库副本 + 真 ComfyUI 目录（8087）
>   探到 `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows`，15 份列出 3 份，
>   其中 `krea2SFWNSFWUncensoredImageTo_v10.json` → `inLibrary=true, promptId=305`
>   （**正是用户 bug ⑤ 里那份工作流**）；真库只读未改。

> ✅ **P4 第 17 步（`0f4552e`，2026-09-24）：错误体对齐 `{error, detail?}`（Task 1）** ——
> **"指向 Rust 时界面显示机器码"那个 bug 的根**。基线是把 Kotlin（8090）与 Rust（8091）
> 挂在同一份真库副本上**逐条 curl** 出来的：
>
> | 场景 | Kotlin 实测 | Rust（改前） |
> | --- | --- | --- |
> | `GET /api/prompts/abc` | `400 {"error":"非法的 id"}` | `400 {"error":"BAD_REQUEST","message":"非法的 id"}` |
> | `GET /api/ai/conversations/999999` | `400 {"error":"CONFIG_ERROR","detail":"会话不存在: 999999"}` | （未搬） |
>
> 两种形态（`Application.kt:567` 的 StatusPages 就是这么分的）：**普通错误 `error` = 人话**；
> **`AiException` 是 `error` = 稳定码 + `detail` = 人话**（AIH-024）。
> 而 Dart 读的是 `error`（`ApiClient._decode`）/ **先读 `detail`**（`AiApiClient._decode`）——
> 键名一错，两个客户端都读不到人话，**而且不报错**。
> - `ApiError` 改成 `{status, error, detail: Option<String>}`：`detail == None` 时**整键省略**
>   （手拼 `Map`，用 `json!` 会写出 `"detail": null`）；新增 `ApiError::ai(code, message)`；
>   内部取人话统一走 `e.human()`（`sync.push` 的 `rejected[].message`、上传失败原因）。
> - 顺手堵两个静默点：未知路径的默认 fallback 是**空 body** → 补 `{"error":"接口不存在: …"}`；
>   `Path<i64>` 解析失败时 axum 回**纯文本** 400 → 新增 `Id` / `IdPair` 提取器。
> - 防线：`api_test` 两例（普通错误只有 `error` 一个键且是人话 / AI 错误两键、`detail` 空则省略）
>   + Dart 侧 `test/api_error_envelope_test.dart` 5 例（两个客户端的读取顺序各钉一遍）。
>
> ✅ **P4 第 18 步（`99026a2`，2026-09-24）：可空字段一律省略键（`explicitNulls = false`）** ——
> Kotlin 的 `AppJson` 是 `explicitNulls = false`（`Json.kt:12`），真库副本上同一个
> `/api/prompts?size=100` 一比：**27 条里 24 条形状不一致**（Rust 侧的 `notes` / `negativePrompt` /
> `batchSize` / `cfgScale` / `width` / `height` / `sampler` / `scheduler` / `sourceRef` 全是 `null`，
> Kotlin 侧**连键都没有**）；`TagDto` 还私加了一个 Kotlin 根本没定义过的 `normalized`。
> 更糟的是 `api_test` 里那些"精确键集合"断言当初照着**错的形状**写，等于**用测试把错误钉住了**。
> - 所有对应 Kotlin 可空字段的 `Option<T>` 补 `skip_serializing_if = "Option::is_none"`
>   （`models.rs` / `ai.rs` / `capture.rs` / `comfy_dirs.rs` / `media_upload.rs`），删掉私加的 `normalized`；
> - 键集合断言改成**按夹具的 NULL 情况逐条算**（并显式断言 `!contains_key`）；
> - **防新 DTO 复发**：`server-rs/tests/shape_lint.rs` 扫全仓源码 ——
>   `#[derive(… Serialize …)]` 的 `pub struct` 里 `Option<…>` 字段必须有 skip，
>   唯一豁免是契约写死回 null 的 `sync.rs::kit_version`；另有一例专测扫描器**会不会漏报/误报**。
>   ⚠ 下一个 `/api/ai/models` 就靠这条防线（它有一堆 `thinkingFormat` / `maxOutputTokens` /
>   `capabilityVerifiedAt` 这样的可空字段）。
> - **验收判据**：真库副本上 Kotlin 8090 与 Rust 8091 的 **13 个数据端点 canon 后零差异**
>   （tags / prompts 列表与详情 / media 列表与详情 / media prompt / providers / capture config /
>   locate / workflows / stats / categories / prompts-media）。
>
> ✅ **P4 第 19 步（`e6d9947`，2026-09-24）：对照时顺手抓到的另外两处形状/取值不一致** ——
> 1. **`/api/health` 少了 Dart 承重读的三个键**（`database` / `version` / `storageDir`）：
>    `lib/core/backend_launcher.dart:271/379/426` 把 **`health['database'] == "ok"`** 当作
>    "后端已就绪"的**唯一判据** —— 少了它，**P4 切流那天 App 会认为后端没起来**、一直等/去"修复"。
>    现在与 Kotlin `HealthDto` 同形（`database` 是真跑一次 `SELECT 1` 的结果，不是常量），
>    `impl` / `db` / `schemaVersion` / `engine` 作为 Rust 侧诊断**多给**（客户端按 key 取值）。
> 2. **`modifiedAt` 精度**：`chrono_lite` 的注释写着"与 Java `Instant.toString()` 同形"、
>    实现却写死 3 位小数 —— 真机上 Kotlin 是 `…:33.917303200Z`、Rust 是 `…:33.917Z`。
>    改成 Java 的规则（小数位 0/3/6/9，按三位一组去尾零），并补一例把规则钉死。
>
> ✅ **Flutter 侧收口（`2b4f7d2`，2026-09-24）**：新增 `test/api_error_envelope_test.dart` 5 例
> （`ApiClient` 读 `error`、`AiApiClient` 先读 `detail`、只剩 `message` 的老形状读不到），
> 并修掉 `ai_api_client.dart` 里那句"后端的统一错误体是 `{code, message}`"的**过期注释**。
>
> 📌 **这一轮最值钱的教训**：`cargo test` 全绿 ≠ 形状对。**这些差异全是"对着同一份真库副本
> 把两个实现逐条比"才摆出来的**（键集合、错误体、`null` vs 缺键、`modifiedAt` 精度）。
> 下一批（`/api/ai/models` 起）同样要留一次这种对照 —— 夹具只覆盖你**想到**的行。

> ✅ **P4 第 20 步（Task 2）：`GET /api/ai/providers/{id}/models`（模型目录）** ——
> 第 17/18 步那两条基线（错误体 `{error,detail?}`、可空字段省略键）就是为它准备的，
> **照单子做，没有照着 DTO 猜**。
> - **形状基线是真库副本上量出来的**（Kotlin 8090 / Rust 8091，同一份库）：
>   `command-code-goat` **81 条** + `deepseek` **2 条**模型 ——
>   **12 个键永远在**（`providerId` `id` `displayName` `inputModalities` `attachmentTransports`
>   `mimeAllowlist` `tools` `parallelTools` `reasoning` `thinkingEfforts` `capabilitySource` `enabled`），
>   **6 个可空键一个都不能自己冒出来**（`thinkingFormat` 81 条里 10 条没有、
>   `maxOutputTokens` / `maxAttachmentBytes` / `maxAttachmentCount` / `capabilityVerifiedAt`
>   85/85 全都没有、`contextWindow` 时有时无）。用例把这条**逐条算**着写，不写"键一定都在"。
> - **错误语义分三层，一层都不能混**：provider 不存在 → **400 `CONFIG_ERROR`
>   + `detail:"Provider 不存在: xx"`**（是 `AiException`，不是 404 —— 写成 404 就没有 `detail` 了，
>   界面又一次显示不出人话）；provider 在但目录为空 → **200 `[]`**；坏 JSON → 按空处理**不许 500**。
> - **一个静默的坑当场抓住**：`enabled` 为 NULL 时我第一版按"启用"处理，
>   而 Kotlin 的裸 `getBoolean("enabled")` 在 NULL 上给的是 **false**。本 schema 里它是
>   `NOT NULL DEFAULT 1`，所以只有老 MySQL 脏行才走得到 —— 但"NULL 当成启用"会让一条
>   用户关掉的模型悄悄可用（那是**能花钱的**）。同类的还有 `display_name` → `model_id`
>   与 `capability_source` → `manual` 两条回落，都用一条"对着计算出来的行"的用例钉住了
>   （真 schema 上构造不出 NULL，夹具里测不到）。
> - **验收**：`cargo test` **96 例全绿**（37 lib + 57 api_test + 2 shape_lint）；
>   **真库副本 canon 逐条比**：`command-code-goat` / `deepseek` 两个 provider 的模型目录
>   **canon 后零差异**（31014 / 638 字节逐字一致），缺 provider 的 400 错误体也一致。
> - 📌 **顺带把全部已搬端点又扫了一遍**（18 条）—— 只有三处不同，**都不是回归**：
>   ① `/api/sync/*` 三条 Kotlin 侧本来就没有（M0 新契约，Rust 才是实现方）；
>   ② `/api/media/{id}/prompt` 在**产物不存在**时两边都是 404，但 Kotlin 的 body 被
>   StatusPages 的 404 处理器**改写成了 `{"error":"接口不存在: <路径>"}`**
>   （路由里那句 `respondNotFound("产物 $id 不存在")` 被吃掉了）；
>   Rust 给的是原话 `{"error":"产物 100 不存在"}`。Dart 只判 `204` / 2xx，**行为无差异**，
>   而且 Rust 那句更有信息量 —— **不改**，但记在这里免得下次有人当回归去"修"。
>
> **📅 2026-09-24 第四轮（本轮）**：先补上"环境能不能真的跑起来"这件事 ——
> 本机 `JAVA_HOME` 指向的是 **JDK 25**，而后端用的 **Gradle 8.12 不支持 24/25**，
> 于是 `gradle installDist` 直接失败（`* What went wrong: 25.0.1`）。
> 机器上其实有一份 **JDK 21**（`D:\Program Files\Android Studio\jbr`，`flutter doctor` 用的就是它），
> 只是 `server.ps1` 的 `Resolve-Jdk` 没往那儿找。要跑 Kotlin 基线时：
> ```powershell
> $env:JAVA_HOME = 'D:\Program Files\Android Studio\jbr'   # 21.0.10
> cd server; gradle installDist
> ```
> **这条值得记进 pitfalls**：`server.ps1` 的候选表里有 `$env:JAVA_HOME` 与
> `$env:ProgramFiles\Java\jdk-2*`，恰恰没有 Android Studio 的 JBR —— 而**这台机器上唯一的
> 21/22/23 就是 JBR**。手敲 gradle 时先设 `JAVA_HOME`，别用系统的 JDK 25。
>
> ✅ **P4 第 21 步：会话与消息的读侧（`GET /api/ai/conversations{,/{id},/{id}/messages}`）** ——
> 界面打开 AI 工作台的第一件事就是这三条（列历史 → 点进去 → 画消息与**有序块**）。
> 搬完之后**指向 Rust 后端也能看到全部历史**，而不是整页 `接口不存在`。
> - 新模块 `conversations.rs` 逐条对照 `AiConversationRepo`；三条最容易静默做错的地方写进了文件头：
>   ① **块必须按 ordinal 升序**（Kotlin 是按 `message_id, ordinal` 取出来再分组 ——
>   顺序乱了的表现是"工具卡位置不对"，而正文还在，看着只是"有点怪"）；
>   ② 消息按 **`seq, created_at`** 排（同毫秒的两条只靠 seq）；
>   ③ `includeArchived` **只认 `1` / `true`**，列表 `LIMIT` 夹在 1..500。
> - 三种"没有"分清：会话在但没消息 → 200 `[]`；会话不存在 → **400 `CONFIG_ERROR` + `detail`**；
>   坏 `json_payload` / `usage_json` → 那一块少个键，**不连累整段历史**。
> - ★ **顺手拆掉一颗定时炸弹（新增 `iso_time.rs`）**：Kotlin 的时间列走
>   `isoTime()` = **先解析再按 Java `Instant.toString()` 重排**，不是原样透传 ——
>   库里 `…:00.000Z` 在那边发出来**没有小数点**（`…:00Z`）。
>   Rust 侧前几批都是直接透传那一列，本机真库上"恰好"零差异（实测 27 提示词 / 24 产物 /
>   23 会话 / 144 消息里**没有一个** `.000Z`），但 `strftime('%Y-%m-%dT%H:%M:%fZ','now')`
>   的毫秒位是 `000` 的概率是 1/1000 —— 撞上就两端悄悄差一个字符串。
>   现在时间列**只有一个出口**（`iso_time::iso_time`），prompts / media / providers / models
>   全部改走它，`comfy_dirs` 那份私有的 `chrono_lite` 也并了进来（一处实现、一处用例）。
>   用例把 `.000Z → 不带小数`、空格文本按 UTC、带偏移换算、以及"认不出来 → **键不出现**"
>   全钉住了。**这条改动的判据就是"改完 18 个端点仍然零差异"** —— 实测确实零差异。
> - **验收**：`cargo test` **104 例全绿**（43 lib + 59 api_test + 2 shape_lint）；
>   **真库副本 canon 逐条比**：13 条路径零差异，其中包括**消息数最多的那条会话**
>   （16 条消息 / 61 个块，含 23 个 `tool_call` + 23 个 `tool_result` 带 `jsonPayload`）
>   —— **158154 字节逐字一致**。
>
> ---
>
> ✅ **M3.3：手机 harness 的**真 HTTP** 端到端（`HarnessHttpEndToEndTest`，4 例）** ——
> `HarnessRunnerTest` 用的是**脚本化的假 transport**，它证明的是"主循环逻辑对"，
> 但**手机真正在跑的那条 I/O 路径一行都没被走过**（M3.1 只验到"harness 能在真机上起来"）。
> 现在这条用例用**真 socket + 真 `UrlConnectionTransport` + 真 SQLite + 真事件总线**
> （唯一假的是一台绑在 `127.0.0.1` 临时端口的 `com.sun.net.httpserver` 网关，**不出网**）：
> ① 一轮纯文本从 socket 一路走到库里（逐字增量、落库、事件次序、请求体里真有 kit 的系统提示）；
> ② 工具往返两轮（`tool_call → tool_result → text` 的块顺序、第二轮最后一条是 tool 结果）；
> ③ 401 → `MISSING_CREDENTIAL` 且消息/Run 都落 failed；④ 连不上 → `PROVIDER_UNREACHABLE`。
>
> ★ **它当场抓到一个真机路径上的 bug**：`UrlConnectionTransport` 让 `IOException`
> （连不上 / DNS 失败 / 读超时）**直接冒出去**，而上层只接 `HttpFailure` / `ProtocolFailure`
> —— 于是落到通用的 `PROTOCOL_ERROR`，用户看到的是
> **「端点或协议不匹配（HTTP null）」**：明明是手机没网，却被指去查 Base URL 与协议。
> `UpstreamClient.classify(null) → PROVIDER_UNREACHABLE` 那一档**一直是死代码**
> （假 transport 直接抛 `HttpFailure`，所以这条转换从来没被走过）。
> 现在三层 `netGuard` 把网络层异常统一翻成 `HttpFailure(status = null)`。
> **这就是"只跑单测"永远看不到的那类问题**——它只在真 socket 上出现。
>
> `:harness:testDebugUnitTest` **152 例全绿**（148 + 4）。
>
> ---
>
> ✅ **M4 第 1 步：上行通道（`_outbox` → `POST /api/sync/push`）** —— 手机产生的会话终于**能到桌面**了。
> 在此之前 `_outbox` 只在写本地行时入队，**没有任何东西把它推上去**（`pendingOutbox`
> 是死代码），也就是说"手机上能聊、桌面上什么都看不到"，而两端都不报错。
>
> - 新模块 `android/harness/.../sync/SyncPush.kt`：`SyncPushClient.push(baseUrl, token)`，
>   真 HTTP（生产用的 `UrlConnectionTransport`）。三条纪律写在文件头：
>   ① **payload 是"推送时现读的完整行"**，不是入队时的快照 —— 助手消息的正文是流式追加的，
>   入队那一刻的快照必然是半截的，推上去桌面看到的就是**一条空回复**；
>   ② **只有 `applied` 才清 outbox**；`rejected` 按**永久 / 暂时**分开
>   （判据只有一条：**重推一次结果会不一样吗** —— 只有 `DB_ERROR` 是暂时的）；
>   ③ 按 `(kind, refId)` 去重（同一条本地行在 Run 期间会入队好几次）。
> - **kind 有两套名字，刻意不合并**：本地队列是 `conversation` / `message` / `part`，
>   线上一律是**单数表名**（`ai_conversation` / `ai_message` / `ai_message_part`）——
>   混用就会"推上去说 `UNKNOWN_KIND`"（对照 Rust `table_of_kind`）。
> - **payload 的列清单从行本身来**（`Row.asMap()`），不手写一份：
>   手写的那份一旦与 schema 漂移就会静默少推一列，而"桌面上少了一列"看不出来。
>   NULL 一律**不发**（可空列不发 = 不覆盖桌面那一列；发 null 反而会把桌面已有的值抹掉）。
> - 接线：桥新增 `syncNow`；`RunEventSource` 补 `syncNow()`；store 在**冷启动**与
>   **每轮 Run 收尾**时各推一次（收尾那一刻内容才定稿）。离线时只记一句
>   "本机还有 N 条没同步到桌面：原因"，**绝不拦聊天页**。
> - **验收**：`:harness:testDebugUnitTest` **161 例全绿**（152 + 9，`SyncPushTest` 走真 socket）；
>   `flutter test` **283 例全绿**；并且**对着真 Rust 后端实测**（8091 + 真库副本，只写副本）：
>   把客户端组装的同一份 payload 推上去 →
>   第一次 `applied=[conv, msg, part]`；**第二次 `applied=0` + 两条 `STALE_IGNORED`**
>   （服务端赢 LWW），库里三张表各 **1 行**、`_sync_log` 4 条 —— **幂等在真实数据上成立**。
> - 还没做：上行目前只在冷启动与 Run 收尾触发（没有前台定时/网络恢复触发），
>   以及 `run` / `submit_job` 两种 kind 还没有上行通道（归 M5 排队提交）。
>
> ---
>
> ✅✅ **M3.2 ④ 真机回归：安卓侧终于**真的能跑**（2026-09-24，OnePlus 8T / KB2000 实测）** ——
> APK 装上、点发送、**收到真回复**，一路走的是生产代码：
> ```
> Flutter UI → MethodChannel → Kotlin harness → run-kit（桌面下发）
>            → HTTP 上游（经 adb reverse 打到开发机的假网关）→ SSE 流式 → EventChannel → 界面 + 本地 SQLite
> ```
> 真机证据（都在这一轮里量到的，不是推断）：
> ① 界面上**真的出现了一问一答**（`hello` → `（假网关）收到：hello`，AppBar「假模型 · 6 条消息」）；
> ② 手机本地库 `ai_messages` 里那两条 `complete`、`ai_runs` 两条 `completed`；
> ③ `mobile_kv.runKit.version = 2026-09-24.1` —— **run-kit 真的从桌面取下来并缓存了**。
>
> ★ **为了走到这一步，抓出并修掉三个"只在真机上出现"的阻断级问题**（每一个都会让
> "安卓那一半"完全不可用，而**单测全绿**、桌面侧也一切正常）：
>
> | # | 现象 | 根因 | 修法 |
> | --- | --- | --- | --- |
> | 1 | 上行每条都失败：`IOException: Cleartext HTTP traffic to 127.0.0.1 not permitted`；**但模型清单却拉得到**（Flutter 那一侧是通的） | Android 9+ 默认**禁止明文 http**，而手机 harness 直连的恰恰是用户自己的 `http://` 网关。`HttpURLConnection`（Kotlin 侧）走 Java 网络栈 → 被拦；`dart:io` 自己实现 socket → **不受这条策略约束** —— 于是"清单拉得到、上游一条也发不出去" | 新增 `res/xml/network_security_config.xml` + manifest 挂上 `android:networkSecurityConfig`（还补了漏掉的 `INTERNET` 权限）。**"公网必须 https"仍在应用层**（`AiValidation.endpointTrust`）。防线：`AndroidCleartextPolicyTest` 扫真文件 |
> | 2 | run-kit **永远取不到**（→ 发送按钮一直是灰的）、上行永远推不上去；报 `NetworkOnMainThreadException` | 桥的 `fetchRunKit` / `syncNow` 在 `onMethodCall` 里**同步**做网络 I/O —— Android 禁止主线程网络 | 桥新增 `offMain { … }`：后台算 + `withContext(Main)` 回（`MethodChannel.Result` 只能在主线程回调）。`fetchRunKit` 顺手从"自己 result.success"改成"返回 JSON" |
> | 3 | **一发消息 App 就闪退**：`RuntimeException: Methods marked with @UiThread must be executed on the main thread`（栈指向 `HarnessBridge.startPump`） | `EventChannel.EventSink` 是 `@UiThread` API，而事件泵在 `Dispatchers.Default` 的协程里直接 `sink.success(...)` —— **第一条事件就崩**。这是 M3 就写下的代码，假 transport 的 JVM 单测抓不到（那条约束是 Android 运行时的） | `emitToSink` / `endSink` 把每一帧与收尾都 `withContext(Dispatchers.Main)` |
>
> ⚠ **三条都属于同一类**：**"平台约束"只在真机上生效**，而我们的单测跑在 JVM 上。
> M3.1 当时只验到"能起来"，这三条就都躲在那道缝里。**结论：安卓侧每加一条
> 会出网/会碰平台 API 的路径，就得真机跑一次**（`adb reverse tcp:8090 tcp:8090`
> 把手机的回环打到开发机，是最省事的做法）。
>
> 📌 顺带记两条真机操作事实（省下一次重踩）：`adb shell input tap` 在这台机器上
> **坐标与截图对不齐**（同一目标要 +445 才命中，键盘弹出后又变回 1:1），
> 所以**别用固定坐标驱动界面**——用"点输入框→`input text`→`keyevent 66`"这条键盘路径最稳。
> 另外装大 APK 前先 `adb shell svc power stayon usb`，否则传输中途设备会掉线。
>
> 🔶 仍未做：**上行（M4）在真机上还没验通过** —— 它需要桌面有 `/api/sync/push`，
> 而**Kotlin 后端根本没有这条**（它是 M0/M1 的新契约，`POST /api/sync/push` 在 Kotlin 上
> 回 `200 {"error":"接口不存在: …"}`，被客户端当成"没收到回执"→ 记暂时失败）。
> 也就是说：**手机上行只能对着 Rust 后端跑** —— 而 `/api/ai/run-kit` 又只在 Kotlin 侧有。
> 真机上"发得出去"与"推得回去"目前各需要一台不同的后端，**这正是 P4 要把
> `/api/ai/run-kit` 搬到 Rust 的理由**（§9 ③ 的下一件大事）。
>
> ✅ **那条理由已经落地了** —— 见下面这条登记（P4 第 22 步）。
>
> ---
>
> ✅ **P4 第 22 步（2026-09-24）：`GET /api/ai/run-kit{,/status}`** ——
> **R12 / R13 的第一次正面重建**，也是**手机只对着一个后端干活**的前提
> （Rust 侧现在同时有 `/api/ai/run-kit` 与 `/api/sync/push`）。
>
> | 字段 | Rust 侧落点 |
> | --- | --- |
> | `systemPrompt` | `system_prompt.rs`（`render` 的逐行对照）+ `nsfw_prompt.rs`（正文，**生成**） |
> | `promptVersion` / `nsfwEnabled` | `nsfw_prompt::PROMPT_VERSION` / `app_settings` 的 `ai.nsfw` |
> | `tools[]` | `mobile_tools.rs`（7 个 `MOBILE`/`BOTH` 工具，**从 Kotlin 的真实 schema 生成**） |
> | `untrustedBanner` | `mobile_tools::UNTRUSTED_BANNER_TEMPLATE` + `..._QUERY_LINE`（R7） |
> | `models[]` | `ai_providers` / `ai_models`（12 个键永远在、5 个可空键按行给，**连 `credentialRef` 都不下发**） |
> | 白名单 / 权限档 | `policy.rs`（**只搬 run-kit 要的那一半**：展开白名单 + `ask`/`full` + 逐工具覆盖） |
> | Skills / 记忆 | `skills.rs`（扫描 + frontmatter + 校验，非法项不进提示词）+ `memory.rs`（读 + 截断） |
>
> ★ **两个"看起来没事、其实很严重"的坑**（都真踩到，已写进 `server-rs/tools/README.md`）：
> ① **Rust raw string 的 `#` 在开引号之前**（`r#"…"#`），而且开引号与正文之间**不能有换行**
> —— 那是字符串的一部分；第一版生成器写成 `r"#` + 换行，结果是每段正文都以换行开头
> （下发的 JSON Schema 直接非法）。
> ② **`kitVersion` 的签名必须逐字节一致**：canonical 里含 `tools[]` 的序列化文本（键序 + 紧凑分隔符），
> 用 `serde_json::to_string(&Value)` 会因为 `serde_json::Map` **按键排序**而与 Kotlin 不同 ——
> 于是两端各自认为"规则变了"→ **互相把版本号顶上去**。所以那份 `tools_json` 是手工拼的，
> 并用 `server-rs/tools/check_signature.py` 在真库上核对公式（实测与库里 `ai.runkit.hash` 逐字符相同）。
>
> **验收**：① `cargo test` **134 例全绿**（69 lib + 63 api_test + 2 shape_lint；4 条新集成用例：
> 形状与 7 个工具 / 覆盖参数与 400 语义 / `/status` 幂等且只在规则变了才跳号 / NSFW 开关）；
> ② **真库副本 + 真 storage 上 canon 逐字比**：Rust（8091）与 Kotlin（8090）的响应**零差异**
> —— `systemPrompt` **50664 字符**逐字一致、83 条 `models[]` 一致、`tools[]` 一致、
> 横幅与两条按轮指令一致、`inlineBudget` 一致；
> ③ **`kitVersion` 停在 `2026-09-24.1` 没跳号** —— 这条最关键，它证明两端算出的签名 hash 相同。
> 未搬：会话写侧 / Run 与 SSE（R5/R6）/ 工具层与审批（R8）/ 附件（R9）。
>
> ✅ **P4 第 23 步（`366f4ce`，2026-09-24）：`/api/auth` 配对闸门 + `GET /api/ai/credentials` 密钥下行**
> —— **inbound-auth 的 P0（令牌闸门）与配对设计的第 1/2 步**，也是"手机只对着一个后端干活"
> 之后缺的最后一块：在此之前 `X-ComfyHub-Token` 在整个 Rust 侧**一次都没被检查过**，
> 只要 `--host 0.0.0.0` 起在局域网上，同 Wi-Fi 下任何设备都能读媒体原件、翻会话、取 run-kit。
>
> - 形状与判定顺序先冻结进 🔒 `docs/pairing-design.md` **§11**（含一处当场拍板：
>   **本轮只做"手机发起 + 桌面显示 6 位码"，不做扫码** —— 扫码要相机插件 + 二维码包两个新依赖，
>   而它买到的安全增量（"人必须看到桌面屏幕"）手机发起通道一样有；代价是用户手填一次桌面地址。
>   A 通道（`pair/start`）留成**显式 501**，将来加不用改任何已冻结的形状）。
> - `server-rs/src/auth.rs`：闸门是**纯函数** `decide(...)` + 一个只做 I/O 的中间件；
>   **不引任何新依赖**（HMAC-SHA256 用已有的 `sha2` 手写、base64url 自己写、
>   随机数走系统 CSPRNG，取不到就报错而不是退化成一个可预测的假随机数）；
>   密钥下行要**解 DPAPI**（`CryptUnprotectData`，空熵 + CurrentUser，与 Kotlin 同档）。
> - 四条纪律：**默认关着**（三条既有 e2e 与桌面 App 都不带头，靠这条）/ **回环永远放行** /
>   **`requestId` 与 ticket 相互独立**（否则手机能自己算出 6 位码，"必须看到屏幕"当场失效
>   而流程照样跑通）/ **凭据端点永不对匿名开放**（与 `enabled` 无关，全局令牌也不行）。
> - **验收**：`cargo test` **151 例全绿**（80 lib + 69 api_test + 2 shape_lint）；
>   **`scripts\e2e-pair-test.ps1` 48 项全绿**（真库副本 + 真端口 + 真"非回环"来源）；
>   **真机那把 key 真的解开了** —— 对着真实 `storage\ai\credentials.dpapi.json`
>   取回 `COMMAND_CODE_GOAT` / `DEEPSEEK_API_KEY`（**只报长度，不打印值**）。
>
> ✅ **P4 第 26 步（`343b6d9`，2026-09-25）：Run 持久化**（`server-rs/src/runs.rs`）—— 事件流
> **两条读法**（进程内 / 断线重连回放）共用的同一份事实，所以先把"落库与读回"这半边钉死。
> `AiRunDto` 11 个键、`create`（14 列，`started_at` 走 SQL 侧 `strftime`）/ `finish`（终态只认三个）/
> `fail_stale_running` + `expire_stale_pending_tool_calls`（后端重启恢复的两半）/
> `EventBatcher`（P1 批量写）+ `events(?after=)` + `prune_events`（留尾部 4000，红线 R5）/
> 工具调用一族（`insert_pending_tool_call` 是 R8 的落点、`record_tool_call` 原地更新）。
> 三条容易静默做错的地方写进了文件头：**`ai_conversations.updated_at` 是承重的**（改的是
> `ai_messages`，触发器不会替我们点火）；**`prune` 留的是尾部**（写成 `>=` 就是"刚发的消息
> 一刷新就没了"）；**`reset_assistant_for_retry` 必须连有序块一起清**。
> 验收：`cargo test` 177 例全绿（当轮）+ **真库副本抽查**（真会话/真 provider 上建 Run，
> `after=4`/`keep=3`/pending 原地更新/终态全部走通）。
>
> ✅ **P4 第 27+28 步（`effbe30`，2026-09-25）：事件总线 + 生命周期端点** ——
> **指向 Rust 后端时 AI 工作台的「发送」按钮第一次真的有了反应**。
> `events.rs`（seq 分配器 + 写穿 + 环形缓冲 2000 与库里 4000 **刻意不同数** + `resume_from`）；
> `run_lifecycle.rs`（`POST …/runs` 的校验顺序**照抄 Kotlin**、`GET /runs/{id}`、`cancel`、
> **`GET /runs/{id}/events` 的 SSE**）。
> ★ **事件流从库里拉**（不是内存订阅通道）：Kotlin 那种做法有一条缝 —— 环形缓冲只留 2000 条，
> `after` 更老时取不到那段历史，而库里那份是批量写的、可能还没 flush。按 `?after=` 直接拉库，
> **顺序 / 去重 / 续传 / 重启回放全由 `(run_id, seq)` 主键保证**；代价是 120ms 轮询延迟
> （对逐字流式够用），空闲 15s 一条 `heartbeat`。`after` 也认 `Last-Event-ID` 头。
> 验收：真库副本 + 真端口（8093）抽查 POST/GET/SSE/`Last-Event-ID`/cancel 全对。
>
> ✅ **P4 第 29 步（`e9b903e`，2026-09-25）：上游 LLM 客户端 + 协议适配器** ——
> 拆成**纯协议**（`protocol.rs`：三家适配器 / SSE 分帧 / 工具分片按 index 归位 / `chat_url` 的
> `/v1` 三家刻意不一样）与**纯 I/O**（`upstream.rs`：reqwest **只开 rustls-tls**、不跟随重定向、
> 状态码→稳定错误码、上游正文**先脱敏再截断**、usage 跨轮累加），外加 `outbound.rs`
> （AGENTS §4：**公网走代理、本机/内网强制直连**，Windows 系统代理读注册表）。
> ★ 用例当场抓到一个真会静默的错：**Anthropic 的定位键叫 `index`**（`output_index` 是 Responses 的）
> —— 读错键会让工具名与参数分片落到不同 index，参数张冠李戴。
> ★ 新增一条 Kotlin 侧没有的**诚实性防线**：**中途断流必须变成错误**
> （既没有 `finish_reason` 也没有 `[DONE]` → `PROTOCOL_ERROR`）。漏了它，网线一拔、代理一掐，
> 库里就是一条 `complete` 的半截回复，而界面上没有任何异常标记。
>
> ✅ **P4 第 30 步（`f235ec3`，2026-09-25）：桌面执行器主循环** ——
> **指向 Rust 后端时 AI 工作台真的能聊了**（真机真上游实测：`assistant text="好"`、
> `providerResponseId=gen_01M3B6…`、usage 17758/2、事件 seq 1..6 以 `run.completed` 收尾）。
> `executor.rs`：上下文拼装（思考段不进上下文；**工具结果必须带 `tool_call_id` 单独成轮**，
> 否则 OpenAI 直接 400 且错误指不到这里）+ 起标题指令**只按轮带、插在第 2 条**（AGENTS §10.1）+
> 流式攒批 + **R6 三条**（正文必须落块 / 失败与取消也要把已经说出去的话落下来 / 收尾只有
> `finish_run` 一条路）+ 取消走旗标（安全点收尾，不是强杀协程）。
> ⚠ **这一版不下发工具**（第 31 步才搬工具层），系统提示里也**不说**有工具 —— 前后一致；
> 反过来会让模型去调一个必然失败的工具。
> 验收：真 socket 假网关三条用例（正常流 / **中途断流 R6** / 429）+ 真机真上游两条
> （deepseek 余额不足如实回 `QUOTA_EXCEEDED`；command-code-goat 真回复）。

> ✅ **P4 第 31 步（2026-09-25）：桌面工具层地基 + R8 审批闸门（`0f91b9f` + 补丁 `30725e6`）** ——
> 交接单第 1 条的前半：**执行器的工具清单不再是空的**。`server-rs/src/tools.rs`（清单 / 执行 /
> 失败码 / 摘要 / 截断）+ 执行器的**工具往返循环** + 审批闸门 + 两条预算。
> 形状全部**逐条对照 Kotlin**（`ToolRegistry.invoke` / `HarnessRunner:686~805`），
> 因为事件与有序块的键集合就是两端共用契约 —— 这一批顺手抓出并修掉四处**只有对照才看得见**的偏差：
>
> | # | 偏差 | 后果 |
> | --- | --- | --- |
> | 1 | 只给"需要审批"的调用发 `tool.requested` | 界面靠它给工具卡**排位置**（用户 bug⑥ 的交错），只读工具的卡在流式期间没有槽位，收尾才冒出来 |
> | 2 | `tool_result` 块 payload 用工具自己的结构化结果 | 少了 `ok` / `code` / `elapsedMs` / `approval` —— **重开 App 之后一条成功的调用会被画成失败** |
> | 3 | 失败事件的键写成 `error`、正文写成 `CODE：人话` | Kotlin 是 `code` + `[工具失败:CODE] 人话`；模型/界面按码分支，键错了就分支不到 |
> | 4 | 每调一次就 `+1` 额度 | Kotlin 的 `spendCall` 在 handler **返回之后**才调 —— 现在是"越界/被拒/报错都不占额度"（用户原话："什么都没干就说预算不足"） |
>
> 三条纪律的落点：**闸门类事件（`tool.requested`）立即 flush 落库**（另一台设备靠它发现"有东西在等人"，
> M1 的纪律，Kotlin `HarnessRunner:316` 同）；**`resolve_approval` 只改 `approval` 不动 `status`**
> （两列分工与 `AiRunRepo` 一致 —— 顺手把 `status` 也写成 `approved` 会让原地更新匹配不上，
> 同一次调用在库里变成两行，而 `/api/ai/pending` 又会把它当成"还在等人批"）；
> **预算到顶不是失败**：那一轮不执行工具，把"为什么没继续"如实补进正文（判成 `run.failed`
> 会让用户已经看到的回答挂红叉）。
> 验收：`cargo test` **216 例全绿**（124 lib + 90 api + 2 shape_lint）。
>
> ✅ **P4 第 32 步（2026-09-25）：提问闸门 `ask_user_question`** ——
> **交接单第 1 条的后半 + 第 4 条**（`ai_questions` 的写侧与跨端回答）。
> 新模块 `questions.rs`（形状 / 持久层 / 等待）+ 四条界面端点 + 执行器里的等待。
> 与审批闸门**刻意分开**（契约 §3）：审批回"允不允许"，提问回的是一段**文字**，
> 而且那股文字作为工具结果喂回模型、**同一个 Run 接着往下做**。
> - **四条端点**：`GET /api/ai/runs/{id}/question`（整张卡，界面重开时把 N 页补回来）、
>   `GET /api/ai/questions/{id}`（**查库**，后端重启后仍能回答"问过什么、答了什么"）、
>   `POST …/answer`、`POST …/skip`。错误档与 Kotlin 一致：不存在 → 400 `CONFIG_ERROR` + `detail`。
> - **形状只有一份**：`row_to_json` 同时给 SSE 事件、`/questions/{id}`、`/pending` 三处用
>   （`pending.rs` 里那份重复实现已删）。`askedAt` 是**数字毫秒**（审批那份才是 ISO 文本）。
> - **跳过 = 立刻按"没回答"放行**：这一处**有意与 Kotlin 不同** —— Kotlin 的
>   `AskGate.discard`（skip 走它）不唤醒 `awaitAll`，整张卡全跳过时那边要等 30 分钟超时；
>   这里把"跳过 / 已失效"当结论，正是 skip 端点的文档语义（"别把 Run 挂死"）。
> - **Run 收尾统一失效**：`finish_run` 里加 `questions::finalize_run(expired)`（收尾的唯一入口，
>   正常/失败/取消三条路都覆盖）—— 少了它那些行永远停在 pending，`/api/ai/pending` 会列出一张
>   **点了也没用**的卡。
> - ★ **顺手补上一条一直缺的启动恢复**：`main.rs` 从来没调用过 `fail_stale_running` /
>   `expire_stale_pending_tool_calls` —— 后端重启后"还在 running 的 Run"会永远转圈、
>   pending 审批行永远等人点。现在三步（Run / 审批 / 提问）按 Kotlin `Application.module` 同序都调了。
> - 验收：`cargo test` **220 例全绿**（126 lib + 92 api + 2 shape_lint），新增两条端到端
>   （一整张卡问 2 题、点按钮 + 自由文本各一 → 同一 Run 收口、答案喂回模型、块与事件形状逐键断言；
>   跳过整张卡 → 立刻放行且**不许假装用户答了**）+ **真库副本抽查**（8099）：
>   `/api/ai/pending` 形状不变、`/api/ai/questions/<不存在>` 回 400 `{error,detail}`、
>   `/api/ai/runs/<不存在>/question` 回 `{"pending":false}`（两个可选键都**不出现**）。
> - 仍未做：联网三工具（`web_search` / `web_fetch` / `civitai_search`）、附件（R9）、
>   界面端点**其余几批**、桌面专属工具（`comfy_*` / 文件工具）。
>
> ✅ **P4 第 33 步（`cde2e52`，2026-09-25）：界面端点第一批（`tools` / `tools/policy` / `nsfw`）** ——
> 交接单第 3 条的第一刀，也是第 31 步那个审批闸门**能不能被用起来**的前提：
> 少了「权限档」这一条，用户**连"自动允许"都切不了**，`ToolAccess::Ask` 的工具永远拦着等人点。
> - `GET /api/ai/tools`：这台后端**能跑**的工具（只列 `tools::IMPLEMENTED`）。
>   ⚠ 列一个跑不了的工具等于告诉用户"AI 能改文件"，而调用会回 `TOOL_NOT_FOUND`。
> - `GET /api/ai/tools/policy`：11 个键与 `ToolPolicyDto` 逐字对齐；`writeRoots` / `readRoots`
>   回的是**展开后的绝对路径**；`autoReadRoots` **每次现算**（真库副本上真的探到用户那 4 个
>   ComfyUI 目录 —— 这条正是用户 bug ④ 要的行为）。
> - `PUT /api/ai/tools/policy`：只改传了的字段；权限档非法 **400**（悄悄回落成 `ask` 看着更"安全"，
>   实际是把用户的意图静默改成另一种行为）；两条预算夹在 1..24 / 1..100（与 Kotlin `coerceIn` 同）；
>   写库是**合并**不是整份覆盖 —— 同一行里还有 `maxReadBytes` 等字段，覆盖会悄悄删掉它们。
> - `GET/PUT /api/ai/nsfw`：`enabled` / `blur` / `effectiveBlur` / `promptVersion`；缺省一律按**关**
>   （漏传一个字段绝不能让那段越狱提示词被悄悄打开），且与 run-kit 读**同一行**
>   （`run_kit::KEY_NSFW` 改成 `pub`：各写一份键名就会出现"界面里开了、提示词没注入"）。
> - 验收：`cargo test` **223 例全绿**（128 lib + 93 api + 2 shape_lint）+ 真库副本抽查（8098）。

> ✅ **P4 第 34 步（2026-09-25）：凭据写侧（`/api/ai/providers/{id}/credentials` + `/api/ai/web-search/credential`）** ——
> 交接单第 1 条之外的"界面端点剩下那几批"里的第一刀，也是**设置页那两张卡能不能用**的分水岭：
> 在此之前状态读得到、**一把 key 都存不进去**（写侧一条都没搬）。
> - 新模块 `credentials.rs`（`sanitize` / 读写库 / `set` / `unset`），形状与判定顺序逐条对照
>   `CredentialService`：**空值 = 不修改**（清空只能 `DELETE`，AIH-012/013）、
>   **env 来源只读**（写进去也读不出来，界面只会显示"已配置"而不生效）、
>   **粘贴 `NAME=value` / 带引号 / 带空白一律 400**（在界面上看着都"填好了"，直到上游回 401）。
> - **DPAPI 写侧**：`auth.rs` 的 `dpapi` 模块原来只有 `unprotect`（密钥下行用），现在把 `protect`
>   也接了出来（`protect_secret` / `unprotect_secret` 两个公开入口），作用域**空熵 + CurrentUser**，
>   与 Kotlin 的 `ProtectedData.Protect(bytes, null, CurrentUser)` 同档；非 Windows 上**故意失败**
>   （AIH-015：绝不许退化成明文落盘）。编码用**标准 base64**（新加 `base64_std`）——
>   写成 url-safe 的话文件照样写出去，Kotlin 却读不懂。
> - 落盘照抄 Kotlin：先写 `*.tmp` → 收紧权限 → `rename` 覆盖；读坏文件一律当"没有"（**不 500**）。
> - **真库副本双端交叉验收**（Kotlin 8102 / Rust 8101，同一份库与凭据副本）：
>   Rust 写 → Kotlin 状态翻成 managed **且解得开**（`POST /providers/{id}/test` 走到 `PROVIDER_UNREACHABLE`
>   而不是 `MISSING_CREDENTIAL`）；Kotlin 写 → Rust `GET /api/ai/credentials?refs=` 拿回**逐字相等**的明文；
>   删除两端同步；真机原有的 4 把 key（`COMMAND_CODE_GOAT` / `DEEPSEEK_API_KEY` / 两个 e2e 的）**一个都没被动**。
> - ★ **这条"一端写、另一端解"的判据被固化成了 `scripts\e2e-credential-test.ps1`（23 项）** ——
>   因为**单测永远证明不了它**（单测里两边都是我们自己）。脚本会让两边都真起、同一份副本；
>   Kotlin 那半可选，起不来时**大声报"跳过 N 项"**（退出码 2），不假装通过。
> - 验收：`cargo test` **228 例全绿**（130 lib + 96 api_test + 2 shape_lint，新增 2 例 lib 单测
>   + 3 例集成）；`scripts\e2e-credential-test.ps1` **23/23 通过、0 跳过**。
> - 顺手抓到的三个坑（已进 `docs/pitfalls.md`）：① `exit` 写在 `finally` 里会把异常一起吞掉
>   （脚本"什么都没说就报通过 0 失败 0"）；② 拿 `/test` 当判据时被**更早返回**的分支挡住
>   （协议 wire 值写错 / 信任档不允许回环），红绿都可能来自另一个原因；③ Kotlin 对未知路径回的是
>   **200** + `{"error":"接口不存在: …"}`，拿 Rust 独有的新契约端点去问它会得到"看起来正常"的响应。
> - 仍未做：`/api/ai/{skills,memory,builtin,capabilities}/*`（下一个批次，见下）、
>   联网三工具、附件（R9）、桌面专属工具（`comfy_*`）。
>
> **📅 2026-09-25 第八轮**：用户要求"继续按总纲实施，**先做不需要人校验的部分**，每完成一项就 git 保存"。
> 于是把 §9 交接单里的三件按"能不能自己验"重新排序：**界面端点批次**排前（真库副本 + 双端对照
> 就能验穿），联网三工具与桌面专属工具排后（前者要真 key、后者要 ComfyUI 在场）。
>
> | 提交 | 内容 |
> | --- | --- |
> | **凭据写侧** | **P4 第 34 步**（见上）+ `scripts\e2e-credential-test.ps1` |
> | **Skills 六条** | **P4 第 35 步**（见下） |
> | **长期记忆五条** | **P4 第 36 步**（见下） |
> | **Provider 写侧** | **P4 第 37 步**（见下） |
> | **内置目录** | **P4 第 38 步**（见下） |
> | **能力目录状态** | **P4 第 39 步**（见下） |
>
> ✅ **P4 第 39 步（2026-09-25）：能力目录的状态与实测校准开关**
> （`GET /api/ai/capabilities/status` + `PUT /api/ai/capabilities/probe-toggle`）——
> 设置页那张「模型能力目录」卡至此能看新鲜度、能切"允许实测校准"。
> - 新模块 `capability_catalog.rs`（`ModelCapabilityCatalog` 的逐条对照）：三层覆盖层
>   （custom > remote > builtin）的加载与**逐维度合并**、查表 `lookup`、`status`、
>   `remoteAgeHours` / `isStale`（7 天）；`ai_settings.rs` 补 `capability_status` 与
>   `probe_enabled` / `save_probe_enabled`（`ai.capabilities.probe`，**只有 `"1"` 算开**）。
> - ★ **118 条内置规则是生成的，不是手抄的**：新工具 `server-rs/tools/gen_capability_rules.py`
>   从 Kotlin 的 `rules` 表机械提取（`--check` 只校验）。理由同 `mobile_tools.rs`：
>   手抄必然漏几条，而漏掉的表现是"某个模型图片输入没预填"，**界面上完全看不出来**。
>   绊线：`cargo test` 里断言条数与首尾顺序，真机上有"两端 `builtinRules` 相等"。
> - 两条容易做错的地方都按 Kotlin 抄了：**最终 `reasoning=false` 时档位与方言一律清空**
>   （留着会与 `validate_model` 打架 → "导入的模型存不进去"）；**去前缀匹配要求唯一**
>   （两个候选时宁可不命中，猜错就是给模型勾错模态）。
> - `refresh`（要 models.dev 在线数据集）与 `probe`（要真上游请求、会花钱）**明确 501**
>   —— 它们不满足"不需要人工/真机校验"这条，如实不假装（用例钉住两个 501 的文案）。
> - **真库副本双端对照 13/13**：`/api/ai/capabilities/status` 的**键集合与取值 canon 后零差异**，
>   含 `builtinRules` **两端都是 118**（生成表没漏规则）、`remoteFetchedAt` 的时间重排一致；
>   开关跨端可见（Rust 打开 → Kotlin 看到 true；Kotlin 关掉 → Rust 看到 false，
>   连那句人话都一样）；真机两条 provider 与 81 个模型一个字节没动。
> - 验收：`cargo test` **258 例全绿**（148 lib + 108 api_test + 2 shape_lint）。
>
> ✅ **P4 第 38 步（2026-09-25）：内置模型目录的状态与同步（`/api/ai/builtin/{status,sync}`）** ——
> 设置页那张「内置模型目录」卡至此能预览、能同步（在此之前指向 Rust 时是「接口不存在」）。
> - 新模块 `builtin.rs`（`AiSeedCatalog` + `AiSeeder` 的逐条对照）：冻结副本的解析、
>   `plan_seed`（缺的 / 一致的 / **不一致但保持用户设置**的三分）、`capability_differs`
>   （**只比能力那八样**，不比 displayName / enabled / 附件上限 —— 那些是用户的地盘）、
>   `decide_actions`（ADD_MISSING 只插入、REFRESH 只刷新）、`preview` / `sync`。
> - ⚠ **灰渡期刻意做法**：Rust 侧没有 classpath，用 `include_str!` 指向 Kotlin 的**同一个**
>   `builtin-catalog.json`（31KB），而**不是**在 `server-rs/` 下再放一份拷贝 ——
>   两份拷贝迟早漂移，而漂移的表现是"Kotlin 登记了 69 个、Rust 只登记了 67 个"，没人会立刻发现。
>   P4 收尾（Kotlin 退役）时把文件搬进 `server-rs/`，路径就地改一行（注释里写了）。
> - 顺手补上 `ai.rs` 的 `insert_model` / `update_model_capabilities`（内置目录"补齐 / 对齐"两条路，
>   后者**刻意不碰** `display_name` / `enabled` / 附件列 / `capability_verified_at`）。
> - **真库副本双端对照 10/10**：`/api/ai/builtin/status` canon 后**零差异** ——
>   真数据上两边都算出 `added=0 / kept=69 / divergent=0 / modelCount=81`（81 条里 12 条是
>   用户自己的，两边都不碰）；未知 `mode` 两端同码同文案；Rust `add-missing` 与
>   Kotlin `refresh-capabilities` 各跑一次，两端看到的库状态一致、模型目录 canon 零差异、
>   **模型总数一次都没变**（不新增不删除）；真机两条 provider 一个字节没动。
> - 验收：`cargo test` **251 例全绿**（143 lib + 106 api_test + 2 shape_lint）。
>
> **本轮收工（第八轮共 6 个提交，全部真库副本 + 双端对照验穿）**：
> 凭据写侧 → Skills 六条 → 长期记忆五条 → Provider 写侧 → 内置目录 → 能力目录状态。
> **界面端点这一批做完了**（`/api/ai/{tools,nsfw,skills,memory,builtin,capabilities,providers,credentials,web-search}/*`；
> 只剩 `capabilities/{refresh,probe}` 两条 501，理由写在用例与上面的登记里）。
> 之后按 §9 的排序是**联网三工具**（`web_search` / `web_fetch` / `civitai_search`，
> 现在 `EndpointGuard` 与能力目录都已经在了）与**桌面专属工具**（`comfy_*` / 文件工具，R10 的落点）。
>
> ✅ **P4 第 37 步（2026-09-25）：Provider 写侧（`POST/PUT/DELETE /api/ai/providers` + `PUT …/models`）** ——
> 设置页那张「AI 模型与凭据」卡至此能建、能改、能删（在此之前 Rust 侧只读）。
> - 新模块 `endpoint_guard.rs`（**SSRF 防护**，AIH-017）：`AddressScope.of` + `EndpointGuard.assertAllowed`
>   逐条照 Kotlin 写（含 IPv6 的 `fec0::/10` 是 site-local 而 `fc00::/7` 不是这种反直觉细节），
>   四档 × 各类地址的**完整真值表**一条用例钉住，云元数据 / `0.0.0.0` 在**任何档位**都拒绝
>   （这条错了就是 SSRF）。
> - `ai.rs` 补 `AiValidation`（provider id / 显示名 / 凭据引用名 / 模型 ID）与**写侧仓储**
>   （`insert_provider` 给 revision=1、`update_provider` 乐观锁 AIH-007、`delete_provider`、
>   `replace_models` 一个事务里删旧插新）；`AiModelDto` 加上 `Deserialize`（**读写共用同一个形状**
>   —— 另写一个入参迟早会出现"GET 给的键 PUT 不认"）。
> - ★ **`normalize_base_url` 没有用 `url` crate 当裁判**：真 Kotlin 基线（8098）逐条问出来一张
>   真值表，`url` crate 与 Java `URI` 有**四处**不一致（`:99999` 端口、`http://` 抛 vs 空 host、
>   `http:/foo` 的 host 为 null、`%zz` 与 `a b` 这类非法字符），
>   光用它就会"放过一批 Java 拒绝的、拒掉一批 Java 接受的"。现在按 Java 的规则自己解析，
>   **37 条探针两个后端逐字一致**（含 `https://[zzz]:9`、`api.example.com:-1`、
>   非 ASCII 路径要放行这些边界）。
> - **真库副本双端对照 35/35**：11 条校验错误**两端同码同文案**；Rust 建 → Kotlin 读（canon 零差异）；
>   Kotlin 改 → Rust 读（revision 语义两端一致，含"过期 revision"那句人话）；模型目录跨端可见、
>   8 条模型校验文案一致、**校验失败后旧目录原地不动**；删除两端同步且模型级联；
>   真机原有两条 provider（`command-code-goat` / `deepseek`）一个字节没动。
> - 📌 **一处有意保留的行为差异**（写在代码注释里）：body 里少写 `providerId` 时 Kotlin 回
>   `400 {"error":"BadRequestException", …}`（机器码，界面显示不出人话），这里按路径参数覆盖、
>   缺了也收下 —— 真实客户端永远是把 GET 到的那份改几个字段传回来，走不到这条岔路。
> - 验收：`cargo test` **243 例全绿**（137 lib + 104 api_test + 2 shape_lint）。
>
> **下一项（本轮继续）**：`/api/ai/builtin/*`（状态 + 同步）与 `/api/ai/capabilities/*`
> （状态 / 刷新 / 实测校准开关与执行）—— 后者的"状态 + 开关"不出网就能验，刷新与校准要真上游。
>
> ✅ **P4 第 36 步（2026-09-25）：长期记忆五条端点**（`GET/PUT/DELETE /api/ai/memory`、
> `POST /api/ai/memory/entries`、`POST /api/ai/memory/delete`）—— 界面上那张「长期记忆」卡
> 至此能存、能加、能删、能清空（在此之前只有 `remember` 工具那条 AI 写入的路）。
> - `memory.rs` 补 `MemoryDto` / `write` / `append_user` / `delete_entries` / `clear` / `entries`；
>   AI 的 `remember` 与界面的「+ 添加一条」**共用同一个 `push()`**，区别只有一个参数：
>   `today` 是 `Some(日期)` 才加 `[日期] ` 前缀 —— **用户手敲的东西不许被我们改写**。
> - ★ **顺手拆掉一处"靠人话猜码"的地基**：原来 `tools.rs` 有个
>   `code_of_memory_error(message)`，用 `contains("上限 100")` / `contains("太长")` 反推稳定码。
>   界面写入的 `记忆太长（N > 8000 字符）` 里**也有"太长"**，但那一档该是 `MEMORY_TOO_LARGE`
>   （单条 501 字符才是 `INVALID_ARGUMENT`）—— 靠字符串猜必然把这两档混掉，而
>   "去清理记忆"与"把这条拆短"是两种处置。现在 `memory.rs` 直接返回
>   `MemoryFailure{code, message}`（Kotlin `ToolFailure` 的同形物），猜码那一段连同它的用例一起删了。
> - **`updatedAt` 的一处形状对齐**：文件不存在时 Kotlin 回的是 **`Instant.now()`**（不是 null，
>   键一定在）；写成"文件不在就没有这个键"会让新装机器的界面显示成"从未更新过"。
> - **真库副本双端对照 10/10**：`/api/ai/memory`（真机 4 条记忆）canon 后**零差异**；
>   跨端可见性两个方向都通（Rust 整篇替换 → Kotlin 读到同一份；Kotlin 删一条 → Rust 也看不到）；
>   三档超限错误**两端同码同文案**；界面加的那条不带日期前缀；跑完副本恢复原样、真机那份 1321 字节没动。
> - 验收：`cargo test` **235 例全绿**（131 lib + 102 api_test + 2 shape_lint，新增 2 例 lib 单测
>   + 2 例集成，删掉 1 例"猜码"用例）。
>
> **下一项（本轮继续）**：`/api/ai/builtin/*`（状态 + 同步）与 `/api/ai/capabilities/*`
> （状态 / 刷新 / 实测校准开关与执行）—— 后者有一条**不出网就能验**的路径（状态与开关），
> 刷新与校准要真上游，留给人配合。
>
> ✅ **P4 第 35 步（2026-09-25）：Skills 六条端点**（list / read / save / delete / roots / rescan）——
> 磁盘是真源、刻意不缓存（"AI 注册完下一次回复就能用、用户删掉立刻消失"是需求，AGENTS §10）。
> 在此之前 Rust 侧只搬了 run-kit 要的"扫描 + 校验"：**设置页那张 Skills 卡只能看**。
> - `skills.rs` 补齐写侧与界面 DTO：`SkillDto` **15 个键**（可空的那 7 个一个都不许自己冒出来）、
>   `SkillRootsDto`、`SkillAutoRegisterResult`、`SkillFailure{code, message}`；`save` 走
>   `render()` 的**逐字**格式（`version: 1` 是写死的）、路径先过 `safe_child`
>   （`Path::join` 遇绝对路径会整段替换 —— 必须先挡）；`delete` 只许删用户来源、bundle 目录整个删；
>   `auto_register` **只补没有 frontmatter 的文件**（已有的一个字节都不改）。
> - **`ParsedSkill` 扩了 6 个字段**（userInvocable / modelInvocable / sizeBytes / fileCount /
>   updatedAt / validationError），`validationError` 改成 `parse_one` 里那条**原话**
>   （以前是 `scan_all` 按 `valid` 反推一句概括 —— 而"不合法"有五种原因，概括出来的话
>   与界面看到的不一样）；`scan_all` 的 `error` 随之改成原话，`list_skills` 工具的文案更准了。
> - **真库副本双端对照 14/14**：`/api/ai/skills`（真机上 **19 条** = 16 用户 + 3 内置）与
>   `/api/ai/skills/roots` canon 后**零差异**；4 条详情逐键一致；"没有这个名字"两端同码同文案；
>   **跨端可见性两个方向都通**（Rust 写 → Kotlin 读到逐字相等的正文；Kotlin 覆盖 → Rust 读到 Kotlin 那版）；
>   删除两端同步；内置删除两端都是 `SKILL_READONLY` 同一句人话；`rescan` 在真数据上两端一致
>   （`registered=0`、无 errors）。真机 `storage\ai\skills` 一个字节没动。
> - 验收：`cargo test` **232 例全绿**（130 lib + 100 api_test + 2 shape_lint，新增 4 例集成）。
>
> **下一项（本轮继续）**：`/api/ai/memory*` 五条 + `/api/ai/builtin/*` 与 `/api/ai/capabilities/*`。
>

> ✅ **P4 第 40 步（2026-09-25）：桌面文件工具五件 + 路径准入（`resolve_read` / `resolve_write`）** ——
> 交接单第 1 条后半的第一刀，也是"AI 能不能真的动文件"那一档：在此之前 Rust 侧
> **一个桌面专属工具都没有**，模型在桌面上只知道那 7 个手机可执行的工具。
> - 新模块 `desktop_tools.rs`（`ToolScope = DESKTOP`）：`list_dir` / `read_file` / `write_file`
>   / `register_skill` / `delete_skill`，`description` 与 `input_schema` **逐字来自 `ToolRegistry.kt`**。
>   ⚠ 它**不能**生成：`run-kit` 只下发手机可执行的（🔒 §3.5），桌面专属的那些在
>   `GET /api/ai/run-kit` 里根本取不到（`gen_mobile_tools.py` 的数据源就是它）。
> - `policy.rs` 补齐**路径准入**（Kotlin `resolve` 逐行对照）：空/非法字符 → 词法归一 →
>   **禁段**（`.git`/`.mysql`/`.run`/`node_modules`，与用户白名单无关）→ **真实路径**
>   （存在就解符号链接；不存在就沿父目录找到第一个存在的祖先再拼回来 —— `write_file` 的目标
>   通常还不存在，只判词法路径就能被一个符号链接骗过）→ 白名单前缀 → **目录根本身不可写**。
>   顺带把 `comfy_dirs::plain_path` 提成 `pub(crate)`：`canonicalize` 的 `\\?\` 逐字前缀
>   以前只在接口返回里去过一次，工具结果里会**第二次**冒出来（模型还会把它贴进下一次调用）。
> - `auto_read_roots` 挂到 `ToolPolicy::load` 上：**只放宽读**（用户 bug ④）——
>   用户自己的 ComfyUI 工作流文件读得到，写仍然只认 `comfyui/`。
> - **两个清单的差别被打成用例**：`catalog`（下发给模型，剔 `deny`）与 `implemented`
>   （设置页看的，**含**被 `deny` 的 —— 剔掉的话用户点过一次禁用就再也开不回来）。
>   `/api/ai/tools` 从后者取：第 40 步之前它只遍历手机清单，于是文件工具实现了界面也看不见。
> - **run-kit 的 `kitVersion` 一个字符都没动**（真库副本实测仍是 `2026-09-24.1`、`tools[]` 仍是 7 个）
>   —— 桌面专属工具**不许**漏进手机清单（R11 / §3.5）。
> - 一条**有意**与 Kotlin 不同：`list_dir` 的条目按名字排序（Java 的 `Files.walk` 走
>   `DirectoryStream` 的文件系统顺序，同一目录两次跑可能不一样；这份输出会进上下文，
>   顺序飘会让模型的复述与断言都不稳）。内容不变，只改次序。
> - `read_file` 的 `nextOffset`：Kotlin 的描述里写着它、实现里**从没给过这个键**
>   （正文末尾那句"offset=N"才是真给模型的）—— 这里保持逐字一致，**不自己发明一个键**。
> - 验收：`cargo test` **274 例全绿**（159 lib + 113 api_test + 2 shape_lint；新增 6 例路径准入
>   单测 + 5 例工具集成 + 2 例清单差别单测 + 2 例桌面清单自检）；**真库副本抽查**（8096）：
>   `/api/ai/tools` **9 条**（categories `ask`/`skill`×3/`files`×3/`memory`）、
>   `kitVersion=2026-09-24.1`、`tools[]` 7 个，真库只读未改。
> - 仍未搬：联网三工具、`comfy_*` 十三件（R10 的落点 `comfy_submit`）、comfy-mcp 39 个、`find_model`。

> ✅ **P4 第 41 步（2026-09-25）：comfy 查询两件（`comfy_get_run` / `comfy_list_workflows`）** ——
> 第 40 步之后的第二刀，挑的是**只碰本机库与磁盘、不需要 ComfyUI 在场**的那两条：
> 于是"AI 知道自己刚提交的那次跑到哪了"和"首次使用时把用户机器上早就有工作流列出来"
> （用户 bug ⑤）这两件事在 Rust 侧第一次成立。
> - `comfy_get_run`：**两个入口都认** —— UUID 形式的 `run_key`，以及**数字**的
>   `capture_runs.prompt_id`（Kotlin 为此专门加了 `findRunByPromptId`：模型很自然地会拿
>   `comfy_submit` 结果里的数字来查，只认 UUID 就会"刚提交完却说没这次运行"，真实对话里发生过）。
>   `capturedAt` 走 `iso_time`（Java `Instant.toString()` 的规则：`.000Z` → 不带小数）——
>   原样透传就会两端差一个字符串，而这是**只有真库才摆得出来**的那类差异。
> - `comfy_list_workflows`：与 `GET /api/capture/workflows` **同一个实现**
>   （[`crate::comfy_dirs::workflows_json`]）—— 三份目录来源、`file:<sha256>` 那把幂等钥匙、
>   `inLibrary` 判定都不另写一份。用例里把"工具的输出"与"端点的输出"逐键比了一遍：
>   各写一份的后果正是用户 bug ⑤ 的复发形态（界面列得出来、AI 却说没有）。
> - 新增 **`server-rs/tests/spot_check.rs`（真库副本抽查入口，默认 `#[ignore]`）**：
>   端点类批次可以"起服务 + curl"抽查，但**工具层没有 HTTP 入口**，
>   只跑内存夹具恰好会漏掉"夹具里没有的行与列"。现在把库副本 + 真 storage + 真项目根
>   指进去，直接调 `tools::execute`（以后的 `comfy_submit` 也走它）。
> - **真库副本实测**（本机 2026-09-25）：`comfy_list_workflows` 探到
>   `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows`（**1 份目录来源**）、
>   15 份里列出 5 份，其中 `krea2SFWNSFWUncensoredImageTo_v10.json` → `inLibrary=true, promptId=305`
>   （**正是用户 bug ⑤ 里那份**）；`comfy_get_run("file:ebff341d…")` →
>   `status=success / promptId=310 / title=anima_shinku_nte_school / capturedAt=2026-09-23T01:02:26.566Z`。
> - 验收：`cargo test` **276 例全绿**（159 lib + 115 api_test + 2 shape_lint；新增 2 例工具用例：
>   两个入口 + 时间重排 + 可空键省略；工具与端点形状逐键相等）。
> - 仍未搬：`comfy_get_status`（要 ComfyUI HTTP + 队列）、`sync_history`（要 capture 入库）、
>   `find_workflow` / `load_workflow`（要 `WorkflowConvert` 转换栈）、`submit`（R10）、
>   `check_inputs` / `fit_workflow` / `env_report`（要本机模型清单）、`audit` / `prune`。

> ✅ **P4 第 42 步（2026-09-25）：ComfyUI 队列客户端 + `/api/capture/{status,jobs}` + `comfy_get_status`** ——
> 这一批第一次"真的去问 ComfyUI"，也是**桌面 App 的捕获状态卡**能不能指向 Rust 的分水岭。
> - 新模块 `comfy_client.rs`：`GET {comfyUrl}/queue` → 运行中 / 等待中条数 + **队首的任务名**
>   （Kotlin `refreshQueue` + `labelOfQueueItem` 逐行对照：标题藏在
>   `extra_data.extra_pnginfo.workflow.extra.title`，取不到就**不编一个名字**）。
>   三条纪律写在文件头：地址只能来自配置（不接受模型给的 URL，否则就是 SSRF 入口）、
>   本机/内网**强制直连**（复用 `upstream::client_for`，否则打开系统代理就连不上 ComfyUI）、
>   网络异常翻成人话（`ConnectionRefused` → "连接被拒绝 —— ComfyUI 没在运行？"）。
> - `/api/capture/status`（11 个键）与 `/api/capture/jobs`（5 个键）**从 501 变实现**：
>   连不上时**如实** `comfyReachable=false` + 队列 0，而不是 500（状态查询失败会把整页拖垮）。
>   `lastPollAt` / `lastError` 在 Kotlin 那边是 `@Volatile` 的**进程内**字段，Rust 还没有轮询器
>   → 如实回 null（键不出现），等第 43 步的 `pollOnce` 搬过来会跟上来。
>   `jobs.submissions` 是"本进程提交过的任务"：`comfy_submit` 还没搬 → **如实空数组**。
> - 工具 `comfy_get_status` 复用同一份 `status_json`，并**带上一处有意的行为修正**：
>   `recentLimit` 在 Kotlin 那边**读了但没用**（永远给 20 条，只影响末尾那句提示），
>   这里按描述真的裁（正文与 meta 一起裁，并说出还有几条没列）——描述与行为不一致比
>   "和 Kotlin 差几个元素"更坏。端点仍是 20 条（那是**形状**对齐，别一起裁）。
> - **验收**：`cargo test` **280 例全绿**（162 lib + 116 api_test + 2 shape_lint），
>   新增用例用**真 socket 的假 ComfyUI**（不出网）覆盖"连不上 / 连得上 + 队列数 + 队首标题"
>   两支，以及 `recentLimit` 的裁剪；`comfy_client` 自带 3 例单测（URL 解析、标题只认那一个位置、空地址报错不 panic）。
> - **真库副本实测**：`/api/capture/status` → `capturedRuns=144 / capturedMedia=99 /
>   comfyUrl=http://127.0.0.1:8188 / outputDir=D:\Comfy-Desktop\ComfyUI-Shared\output`，
>   此刻 ComfyUI 没在跑 → `comfyReachable=false` 且队列 0（如实），`recent` 20 条真实记录。

> ✅ **P4 第 43 步（第一半，2026-09-25）：`GraphParse` 移植 + 真库逐字段对账** ——
> 捕获入库（`/api/capture/poll` → 建提示词与产物行）的前置件：**一份 API 节点图 → 提示词字段**
> 的启发式解析。它判错的表现是"画廊里那条记录的参数是错的"，而且**不报错**。
> - `graph_parse.rs`（Kotlin `GraphParse` 逐行对照，420 行）：采样器定位五档判据、
>   模型链收集 LoRA / checkpoint、条件链穿透转发与原始值节点、宽高批量、产物类型
>   （图 / 视频 / 音频 / MIXED）、`extra_params`（上限与 Kotlin 一致：`>= 60` 就停，
>   而那条 `__node_id` 不经过检查 → **实际最多 61**）、标题线索。
> - ★ **`serde_json` 打开了 `preserve_order`**（本轮唯一的依赖改动）：Kotlin 的 `JsonObject`
>   是插入序，而 serde_json 默认是 **BTreeMap（按键排序）**，而这些启发式全是"文档顺序里第一个
>   命中的节点"——顺序一变，多候选图上解析出的 seed / 提示词就与 Kotlin 不同，**而且不报错**。
>   实测副作用为零：`kitVersion` 仍是 `2026-09-24.1`、`systemPrompt` 仍是 50664 字符逐字不变。
> - ★★ **真库逐字段对账抓到一个静默 bug**：`capture_runs.raw` 里的连线引用**可以是数字**
>   （`"negative": [76, 0]`，而图的键是字符串 `"76"`）。Kotlin 的 `asRef` 走 `contentOrNull`
>   所以读得到，Rust 第一版只 `as_str()` → **负向提示词 / LoRA 链 / checkpoint / 尺寸整条链静默断掉**。
>   修完 **25 条真实记录逐字段零差异**（`positive` / `negative` / `checkpoint` / `sampler` /
>   `scheduler` / `steps` / `cfg` / `seed` / `width` / `height` / `batch` / `kind`）。
> - 另两处语义坑写进了用例：`maxByOrNull`（并列取第一个）vs Rust `max_by_key`（并列取最后一个）；
>   大 seed（> i64）在 Kotlin 是**饱和**成 `i64::MAX`（第二档 `toDoubleOrNull()?.toLong()` 先命中），
>   不是双曲回绕 —— 第一版按回绕写，被用例当场抓住。
> - 验收：`cargo test` **291 例全绿**（173 lib + 116 api_test + 2 shape_lint；新增 11 例解析单测，
>   含一份"真机形状"的固化夹具）+ **真库对账 25/25 零差异**（`tests/spot_check.rs`，默认 `#[ignore]`）。
> - 第二半（**已落地**）：见下面第 43 步第二半那条登记。
>
> ✅ **P4 第 43 步（第二半，2026-09-25）：捕获入库 —— `POST /api/capture/poll` + `comfy_sync_history`** ——
> **"产物真的进画廊"** 就是这一步：从 ComfyUI 的 `/history` 把跑完的运行收进库（建提示词 →
> 产物落盘入库 → 收尾运行记录）。它是 `comfy_submit`（R10）的直接地基 ——
> 提交完的产物**必须**走这同一条路入库，否则就是"AI 出的图不在画廊里"。
> - 新模块 `capture_ingest.rs`：`begin_run` / `can_reclaim` / `finish_run`（Kotlin `CaptureRepo`）
>   + `import_file`（内容 sha256 判重、图片顺手写缩略图、`move`/`copy` 两种语义）
>   + `create_captured_prompt`（Kotlin `PromptRepo.createCaptured`）+ `poll_once` / `ingest_claimed`
>   + HTTP 下载兜底（`GET /view`，本地 output 目录里没有那份文件时才走）。
> - 四条纪律逐条落地：**`empty` / `error` 可重试**（`success` 才是唯一终态；把 `empty` 当终态
>   就是"AI 说生成了、画廊里却没有"）／**卡住的 `running` 能重抢**（后端被杀过）／
>   **一次运行只建一条提示词**（`run_key` 唯一 + `prompts.source_ref`）／**产物按 sha256 判重**。
> - ★ 两处**只有真库才摆得出来**的细节：① 空集合的写法 —— Kotlin 存 `loras = "[]"` /
>   `extra_params = "{}"`（真库实测 ComfyUI 来源的行全是这两个），写真 NULL 会让"这一列什么形状"
>   两端分叉；② `extra_params` 的**键顺序**是节点图的文档顺序，所以是手工按序拼对象
>   （`BTreeMap`/`json!(map)` 会改成键排序）。
> - **验收**：`cargo test` **300 例全绿**（179 lib + 119 api_test + 2 shape_lint），
>   新增 6 例纯函数单测（`canReclaim` 真值表 / 产物条目收集与过滤 / kind 判定 / 百分号编码 /
>   `IngestResult` 键集合）+ **3 例端到端**（真 socket 假 ComfyUI：本地 output 直接收、
>   下载兜底 + `empty` 重试、端点形状）；以及 `tests/spot_check.rs` 的
>   **真库重放**：拿真库里那条 `fba277c5…`（成功、1 个产物）在副本上重放一遍 →
>   claim 先被幂等键挡住 → 清掉后重收 → **建出的提示词标题与 Kotlin 当年那条逐字相同**
>   （`"Krea2_sage_t2i · seed 20260922"`、seed 对）→ 产物文件已不在盘上且 ComfyUI 没在跑，
>   于是**如实** `imported=0 / failed=1`（不假装收进来了）。
> ✅ **P4 第 44 步（2026-09-25）：`WorkflowConvert` 核心 + `comfy_load_workflow`** ——
> 用户 bug ③ 的正面解法：用户甩过来一个 `.json` 路径，别再回答"我只能提交库里的 promptId"。
> - `workflow_convert.rs`（Kotlin `WorkflowConvert` 逐行对照）：界面格式（`nodes`/`links`）→
>   API 节点图。**能等价改写的都改写**：`Note`/`MarkdownNote`/`PrimitiveNode` 丢掉、
>   `Reroute`/`SetNode`/`GetNode` 穿透到真正的上游、**旁路（`mode=4`）按同类型接过去**
>   （真机一份存档里 61/91 个节点是旁路，直接拒会让功能对真实文件完全不可用）、
>   静音（`mode=2`）如实警告下游、控件值按 `/object_info` 的声明顺序贴回名字
>   （`widgets_values_named` 优先、`control_after_generate` 多占的槽位照跳、`forceInput` 不算控件）。
>   **缺口清单**：`unresolvedInputs`（悬挂引用）/ `openInputs`（空着的连线型输入，正好是补线的作业本）。
> - `workflow_load.rs` + 工具 `comfy_load_workflow`：读文件（过**读白名单**）→ 认格式
>   （API 原样 / 界面格式按 `/object_info` 转）→ 入库拿 `promptId` → 返回节点摘要与缺口清单。
>   幂等靠 **`file:<sha256>`**（与 `GET /api/capture/workflows` 的 `inLibrary` 判定同一把钥匙）。
> - ★ 两处**只在真机上才出现**的细节照抄 Kotlin 的解法：
>   ① **"抢不到 + `success` 但 `prompt_id` 已空"要带 force 重抢** —— 用户把提示词删了之后，
>   `run_key` 是文件内容算的，这份文件否则**再也导不进来**、只会永远回"正在被另一次调用加载"；
>   ② 引用里的节点 id **数字发数字、非数字发字符串**（Kotlin `WorkflowNodeKeys.refOf`）——
>   图的键永远是字符串，全都写字符串虽然也符合 ComfyUI 规范，但会让"两端转同一份文件"
>   的产物逐字节不同，而这类差异不报错、只会让人怀疑。
> - ⚠ **两处欠账，绝不假装**：**组节点（subgraph）不展开**、**「模拟用户行为」
>   （`emulateFrontend`）没实现**。两者都让节点进 `unsupportedNodes`（默认档直接
>   `UNSUPPORTED_NODES` 拒绝、`tolerateUnsupported=true` 则摘掉并给缺口清单）。
>   工具的描述是从 Kotlin **逐字**搬的（R12/R13，里面写着"模拟用户行为默认开着"），
>   所以 `workflow_load` **在结果里主动补一句 warning** 说明这一步没做 ——
>   不说这句，用户会以为那些前端节点已经被照前端的做法接好了。
> - 验收：`cargo test` **315 例全绿**（192 lib + 121 api_test + 2 shape_lint），
>   新增 8 例转换单测（含**两份逐字抄自 Kotlin `WorkflowConvertTest` 的用例**：
>   经典六节点图与 `forceInput`，断言也照它写）+ 2 例端到端（API 文件幂等 + 删掉后能重导；
>   界面格式 + 假 `/object_info`：默认档如实拒绝、`tolerateUnsupported` 给缺口清单）
>   + 3 例 `workflow_load` 自检；**真机抽查**：真 ComfyUI 目录里那 5 份工作流全部被正确
>   认成 `ui`，且因 ComfyUI 没在跑而**如实**回 `COMFY_UNREACHABLE`（带人话"连接被拒绝 ——
>   ComfyUI 没在运行？"），同时证明路径准入确实放行了用户自己的工作流目录（用户 bug ④）。
> - 仍未搬：`comfy_find_workflow`（库内搜索 + 参数摘要）、`comfy_submit`（R10）、
>   `check_inputs`/`fit_workflow`/`env_report`（要本机模型清单）、`audit`/`prune`、
>   comfy-mcp 39 个、以及上面那两处欠账。

> 📅 **2026-09-25 MCP 真机探针（第 45 步的准备，用户要求"让 AI 试试调 comfy mcp 做点具体任务"）** ——
> 用户机器上把整条链走了一遍，**结论是"能用，但有两处必须先补"**，全部记进了 `docs/pitfalls.md`。
>
> | 事实 | 细节 |
> | --- | --- |
> | 39 个工具**真的拉得到** | `python comfy-mcp.exe` + `initialize` → `notifications/initialized` → `tools/list`（与 Kotlin `McpStdioClient` 文件头记的协议逐字一致）。工具覆盖：`run_workflow` / `generate_image` / `run_template` / `job` / `validate_workflow` / `list_workflow_slots` / `set_workflow_slot` / `vary_workflow` / `fetch_outputs` / `launch_comfyui` / `stop_comfyui` / `restart_comfyui` / `system_stats` / `free_memory` / `nodes` / `search_models` / `download_model` / `upload_file` / `install_node` / `get_logs` / `discover` / `which` / `project` / 模板与模型检索若干 |
> | **必须给 `COMFY_BIN`** | 少这一个环境变量，**任何**工具都只回 `Error executing tool xxx`（真因是 comfy-cli 调不起来，被 rich 的方框排版埋在 stderr 里）。子进程环境要给 `COMFY_BIN=<绝对 comfy.exe>` + `PYTHONUTF8=1` + `PYTHONIOENCODING=utf-8`。⚠ 排错捷径：`comfy.exe <子命令> --json` 手敲，1.20 起一律回结构化信封（`error.code/message/hint`） |
> | **`launch_comfyui` 不挂共享模型根** | 起的是安装目录里那个裸实例 → 用户工作流全部预检失败（`workflow_unknown_nodes`：`anima-base-v1.0` / `anima-turbo-lora-v0.2` / `qwen_3_06b_base` 不在已知选项）。这几个文件**就在盘上**（`ComfyUI-Shared\models`）。补 `extra_args=["--extra-model-paths-config","<%APPDATA%>\Comfy Desktop\shared_model_paths.yaml","--input-directory",…,"--output-directory",…]` 之后：unet 2→5、clip 2→5、lora 13→14，用户的 Anima 工作流立刻能跑。**这正是 `ComfyEnv.prepareLaunch` 干的那三个参数** —— MCP 那条路没有，得调用方自己带 |
> | **`/history` 是进程内的** | 关掉旧实例重开之后，重启前那次运行从 `/history` 里**消失了**（只剩重启后跑的那条）。所以"提交完靠轮询补收"只在**同一个实例生命周期内**成立 → `comfy_submit`（第 46 步）**必须自带"提交后自己等结果并立即入库"**，轮询只作兜底（Kotlin `ComfySubmitter` + `captureRun` 就是这么设计的） |
> | 实测耗时（RTX 4060 Laptop 8GB / 24GB RAM） | Krea2 Turbo int8 8 步 1024×1360 = **52.0s**；Anima base + turbo LoRA 10 步 896×1152 = **17.8s**（含加载） |
> | **全链路真的通了** | MCP `run_workflow` 出图 → `POST /api/capture/poll`（Rust，第 43 步）在**真 ComfyUI 上**收下这一次运行：`checked=1 / newRuns=1 / newMedia=1`，`capture_runs` 记 `success media=1 title=anima_base_turbo_sage · seed 20260922`，`media_assets` 那条带上 `896×1152` / `sha256` / `promptId` / `hasWorkflow=true`，并回填 `promptTitle` 与 `promptPositive`。**这是第 43 步第一次对着活实例验收**（此前 ComfyUI 没在跑，只能验"连不上"那一支） |
>
> 第 45 步（Rust 侧 MCP 桥）据此的设计要点：传输层做成**泛型 `AsyncRead+AsyncWrite`**（生产接子进程 stdio、测试用 `tokio::io::duplex` 的脚本化假服务器），
> 子进程环境照上表给；工具声明**原样透传** `inputSchema`（不改写），`run_workflow` / `run_template` / `generate_image` / `download*` 给长超时；
> `launch_comfyui` 由我们**替用户带上共享模型根三个参数**（否则"AI 起来了实例却跑不了用户的工作流"）。

> **📅 2026-09-25 这一轮的交接单**（上一轮留下的，按价值排序；① 的前半已落地）：
> 1. ✅ **工具层与审批（R8）+ 提问闸门**：第 31 / 32 步已落地（四个本机工具 + 闸门 + 两条端点批次）。
>    **还剩**：联网三工具（`web_search` / `web_fetch` / `civitai_search`，要 Keenable / Civitai
>    客户端 + `EndpointGuard`）与**桌面专属**工具（`comfy_*` 十三件 + comfy-mcp 39 个
>    —— 那才是"AI 能不能真的生图"那一档）。**文件工具五件已落地（第 40 步）**；
> 2. **附件（R9）**：`/api/ai/attachments*` 那几条 + 内联预算（现在有附件直接 `UNSUPPORTED_CONTENT`）；
> 3. `/api/ai/{skills,memory,tools,nsfw,builtin,capabilities}/*` 这批**界面端点** ——
>    今天指向 Rust 时设置页那几张卡会说"接口不存在"（**不是错误，是没搬**）；
> 4. **桌面执行器的"截断续写"**（Kotlin 有 `MAX_CONTINUATIONS` + `CONTINUE_INSTRUCTION`）：
>    Rust 侧还没有 —— 输出被长度上限卡断时，现在会把半句话当成结尾。
>
> ✅ **P4 第 25 步（`pending.rs`，2026-09-24）：`GET /api/ai/pending`（跨端待办总览）** ——
> M1 定的契约，Rust 侧一直缺。它是"**另一台设备刚连上来**"（手机 / 第二个窗口 / 刚重启的
> 桌面 App）用的：那台设备没经历 SSE 事件流，靠这一个接口把**所有"在等人"的卡片**一次补齐
> （AI 的提问 + 需要批准的工具调用），而不是逐个 Run 去问。
>
> ★ **与 Kotlin 的一处差异必须说清**：Kotlin 的 `asks` 来自**内存里的提问闸门**
> （`AskRegistry.allPending()`）；Rust 没有 Run 执行器、也就没有那个闸门 ——
> 所以这里直接读 **`ai_questions` 表里 `status='pending'` 的行**。
> 这不是"退一步"，而是同一件事：M1 的纪律就是 **`track()` 当场落一行 pending**
> （正是为了"界面还没挂上 / 后端重启了，卡也不能丢"），收尾只把 pending 改成终态；
> 于是"表里还 pending" ≡ "闸门里还活着"。**"内存里活着但库里没有"这种状态 M1 起就不存在**。
>
> 两处形状细节**刻意不统一**（谁"顺手统一"谁就制造两端差异）：
> `asks[].askedAt` 是 **epoch 毫秒数**（Kotlin `askedAtMs`），
> `approvals[].askedAt` 是 **ISO 文本**（Kotlin `rs.isoTime`）；
> 可空键（`header` / `answer` / `optionIndex` / 空的 `optionIndexes`）**整个键不出现**。
> 另外**只列真的还在等的**：Run 已收尾的 pending 审批行是"永远等不到人来点"的残留
> （后端重启恢复会按拒绝收尾），列出来会让另一台设备冒出一张点了也没用的审批卡。
>
> **验收**：`cargo test` **158 例全绿**（82 lib + 74 api_test + 2 shape_lint），新增 4 例
> （2 条纯函数：毫秒换算与脏 JSON；2 条集成：只列真的在等的 + 没有待办时回两个空数组）；
> **真库副本抽查**：真库此刻没有待办（`ai_questions` 一行都没有、pending 审批 0 条），
> 接口如实回 `{"approvals":[],"asks":[]}`；往副本里**注入**一条 pending 提问 + 一个
> running Run 上的 pending 审批之后，回出来的形状与时间类型都对（毫秒 / ISO 各一份）。

> ✅ **P4 第 24 步（`7a02d1f`，2026-09-24）：会话与消息的写侧** —— 第 21 步只搬了读侧：
> 指向 Rust 后端能**看见**全部历史却动不了它。这一批补上 `POST/PATCH/DELETE conversations`
> 与 `POST conversations/{id}/messages`，`/api/ai/conversations*` 至此读写齐了。
> - 三条纪律：**校验在事务之前**（那三种错是 `AiException` 那一档：`error`=稳定码 +
>   `detail`=人话，而 `state.write` 的错误通道只认普通错误）；`seq` **由服务端分配**
>   （两台设备同时追加就不会撞号）；**消息与有序块同一个 `BEGIN IMMEDIATE` 事务**。
> - **验收**：`cargo test` **153 例全绿**（80 + 71 + 2），新增 2 条集成用例；
>   **真库副本抽查**：建会话 → 追加带工具卡的助手消息 → 读回 → 删除 → 再读 400
>   `{"error":"CONFIG_ERROR","detail":"会话不存在: …"}`；真库只读未改。

> ---
>
> **📅 2026-09-24 第四轮（本轮）**：先补上"环境能不能真的跑起来"这件事 ——

---

## 9. 下一轮从哪起手

> **2026-09-25 第七轮（本轮）**：用户要求"继续按总纲实施"。
> 这一轮做的是**交接单第 1 条**（工具层 + 两个闸门），也就是"桌面执行器能不能真的用工具"。
>
> | 提交 | 内容 |
> | --- | --- |
> | **`0f91b9f`** | **P4 第 31 步：工具层地基 + R8 审批闸门**（`tools.rs` + 工具往返循环 + 两条预算，见 §8） |
> | `30725e6` | 第 31 步补丁：**只有真跑成功才消耗额度**（`spendCall` 的时点） |
> | **`557ab76`** | **P4 第 32 步：提问闸门 `ask_user_question`** + 四条界面端点 + `ai_questions` 写侧（见 §8） |
> | **`cde2e52`** | **P4 第 33 步：界面端点第一批**（`/api/ai/tools` + `/tools/policy` + `/nsfw`）—— 设置页那两张卡的地基 |
>
> **这一轮最值钱的三条教训**（都写进了 §8 的登记与 `docs/pitfalls.md`）：
> ① **"形状"要逐条对着 Kotlin 抄，不能照着 DTO 猜** —— 这一批抓出四处偏差（`tool.requested`
> 该给每个调用发、结果块的 `ok`/`code` 键、失败正文的 `[工具失败:CODE]` 形状、`spendCall`
> 的时点），每一处都**不报错、只是界面上少一块或画错一块**；
> ② **两个"看起来对称"的闸门语义完全不同**：审批"超时=拒绝"，提问"超时/跳过=没回答"，
> 而后者**必须立刻放行**（Kotlin 的 `discard` 不唤醒 `awaitAll`，整卡跳过要等 30 分钟 ——
> 这一处 Rust 侧有意改好，并写进了代码注释与用例）；
> ③ **测试之间共用进程级全局态**会造出"跑单条永远绿、全量偶发红"的假 bug
> （`sessions()` 的 `seen` 被 `rotate` 清掉、`state()` 的临时目录只靠纳秒去重）——
> 两条都修了，防线一条没动。
>
> **下一轮从这三件起手**（按价值）：
> 1. **界面端点剩下那几批**（`skills` / `memory` / `builtin` / `capabilities` / providers 写侧
>    + `web-search/credential`）—— ✅ **已全部落地**（第 33~39 步，只剩 `capabilities/{refresh,probe}`
>    两条如实 501）；
> 2. **联网三工具**（`web_search` / `web_fetch` / `civitai_search`）—— 系统提示里那一整节
>    "联网搜索的纪律"现在指向三个**没下发**的工具；`web-search/credential` 那三条端点
>    也已落地（Keenable 那把 key 就存在那里）；
> 3. **桌面专属工具**（`comfy_submit` / `comfy_*` 十三件 / comfy-mcp 39 个 / `find_model`）——
>    这才是"AI 能不能真的生图"那一档，也是 R10 在 Rust 侧的落点。
>    ✅ **文件工具五件已落地**（第 40 步，含路径准入）；comfy 查询/同步五件见第 41~44 步；
>    剩下的见下面这份重排。
>
> **📌 2026-09-25 两轮实测之后的起手顺序（按"不做会怎样"重排，详见 §10）**：
> 1. **`search_prompts`（新增，只读）+ 角色卡** —— 改动最小、直接决定"出图能不能一次对"
>    （现场：AI 没有"搜自己提示词库"这条路，把 shinku 当 iroi 写了两版废图）；
> 2. **`comfy_env_report` + `ComfyModels` 本机清单** —— "盘上有、实例看不到"的解药，
>    也是用户抱怨"AI 说没有某模型"的第一现场；
> 3. **MCP 桥**（泛型 transport + `COMFY_BIN` + stderr 带进错误 + 替用户带共享模型根三参数）；
> 4. **`comfy_submit`（R10）**：提交后**自带"等结果 + 立即入库"**（`/history` 是进程内的，
>    重启即丢），并补 Rust 侧的**后台捕获轮询器**；
> 5. 附件（R9）、截断续写、`capture/{import,import-workflows}`、切流收尾。
>
> 用户已拍板（2026-09-25）：**这三件都要做**，并且**可以用真 provider 跑一两条真上游验收**；
> 真机回归由用户按屏幕配合（`adb reverse` + 假网关那套先由我把不花钱的部分准备好）。

> **2026-09-24 第六轮**：用户要求"继续按总纲实施，每实现一项就 git 保存；
> 并**测试电脑与手机之间的数据与部分设置（含 API Key 设置）互传**"。
> 于是本轮把"配对 + 密钥下行"整条链做完，**并在真机上验穿了**。
>
> | 提交 | 内容 |
> | --- | --- |
> | **`366f4ce`** | **P4 第 23 步：`/api/auth` 配对闸门 + `GET /api/ai/credentials` 密钥下行**（见 §8） |
> | `7a02d1f` | **P4 第 24 步：会话与消息的写侧**（见 §8） |
> | `50f0f1b` / `95e699c` | 第 23 步两处补丁：兑码也按来源地址限速；设备"最后活跃"立刻可见 |
> | `2e42d8d` | 文档：登记 23/24 步 + `e2e-pair-test.ps1` 进脚本速查 |
> | `9ea9d76` | Flutter：**令牌只留一份**（客户端默认带上）+ 手机上两处溢出的第一版修复 |
> | `dab4dba` | Flutter：手机上那两道溢出**真正**修掉 + 两张维护卡在手机上折叠（用户反馈"排版实用性极差"） |
>
> **真机验收（§9 表里那句"下一件该做的事"这一轮做完了）** ——
> OnePlus 8T 实测，**闸门开着**的状态下：手机举手 → 桌面显示 6 位码 → 敲码配对 →
> 手机密钥库里出现 `auth.pairing` + `COMMAND_CODE_GOAT` + `DEEPSEEK_API_KEY`（AES/GCM 密文）→
> **用下行来的 Key 真的聊通一轮** → 那条会话连同有序块经 `/api/sync/push` 回到桌面库
> （`provider_id=command-code-goat`、`model_id=deepseek/deepseek-v4.1-flash`，正是手机上选的）。
> 逐条证据与操作手册在 🔒 [`docs/pairing-design.md`](pairing-design.md) **§12**，这一轮改掉的东西在 §13。
>
> **真机验收顺手抓到的四条**（都是单测与桌面永远看不见的）：
> ① `adb install -r` 会卡死（改 `push` + `pm install`，debug 包只带 `android-arm64`）；
> ② `input tap` 坐标与 `screencap` 对不上（+495px，且随键盘状态变）→ **别用固定坐标驱动 Flutter 界面**；
> ③ 「AI 模型与凭据」页自己 `new` 了一个**不带令牌**的客户端 → 真机红横幅`需要配对令牌`，
>    而配对明明是成功的（根治：令牌只留一份、**由客户端默认读取**）；
> ④ 手机上两处 `RenderFlex` 溢出（`RIGHT 2px` / `BOTTOM 57px`）→ 修掉并补**按真机视口的防线**
>    （`test/phone_layout_overflow_test.dart`；实测把修复退回后它会红）。
>
> 下一件该做的事（按价值）：
> 1. **Run 与 SSE**（R5 保留窗口 + R6 失败不丢内容）—— P4 剩下最大的一块；
> 2. **工具层与审批（R8）/ 附件（R9）**；
> 3. **`/api/ai/{builtin,capabilities}/*` 那几条**（桌面维护模型目录用的）——
>    今天只有 Kotlin 有，手机连 Rust 时那两张卡会写"接口不存在"（**不是错误，是没搬**）；
> 4. `capture/import-workflows`（要整套 `WorkflowConvert` 转换栈，**要 ComfyUI 在场**）。

> **2026-09-24 第五轮（3 个提交）**：一个 Flutter 界面修复 + 一份配对设计 + **P4 第 22 步**。
>
> | 提交 | 内容 |
> | --- | --- |
> | `5dbc6f8` | 手机 AI 工作台输入区：**操作控件靠左、只有发送贴右** + token 用量按数量级切 K/M/B |
> | `767de0a` | **`docs/pairing-design.md`**：配对流程（扫码 + 滚动 6 位码）、接口形状、防线、三步落地 |
> | `7719372` | **P4 第 22 步：`GET /api/ai/run-kit{,/status}`**（见 §8 那条登记，R12/R13 的第一次重建） |
>
> **下一件该做的事：把真机回归（M3.2 ④ / M4）改成对着 Rust 后端跑。**
> 现在 Rust 侧同时有 `/api/ai/run-kit` 与 `/api/sync/push`，手机上
> "发得出去"+"推得回去"第一次能在**同一台后端**上完成 —— 这正是上一轮卡住的地方
> （§8 M3.2 ④ 的"仍未做"）。做法照 M3.2 ④ 记的那套真机手法：
> `adb reverse` 或局域网地址 + `flutter build apk --debug` + 实测一次"发一条 → 桌面看得到"。
>
> 之后按价值排：
> 1. **会话写侧**（`POST /api/ai/conversations`、追加消息 / 块）—— 手机上行真正落地的前提
>    （现在只有读侧，手机能推的只有它自己产生的行，桌面侧一旦要"就地续写"就还得走 Kotlin）；
> 2. **Run 与 SSE**（R5 保留窗口 + R6 失败不丢内容）—— 这是 P4 剩下最大的一块；
> 3. **工具层与审批（R8）/ 附件（R9）**；
> 4. **配对落地**（`docs/pairing-design.md` 三步里的第 ① 步：`InboundAuth` 闸门 + `auth.pairing`）——
>    它是"开远程"的前置条件，也是手机令牌的唯一来源（现在手机**从来不传令牌**）。
>
> ---
>
> **2026-09-24 第三轮（错误体/形状对齐这一轮）共 4 个提交**，全部测试在当前 HEAD 上是绿的：
> `cargo test` **93 例**（35 lib + 56 api_test + 2 shape_lint）、`flutter test` **269 例**
> （`:harness:testDebugUnitTest` 148 例未动）。
>
> | 提交 | 内容 |
> | --- | --- |
> | `0f4552e` | P4 第 17 步：错误体对齐 `{error, detail?}`（**Task 1**） |
> | `99026a2` | P4 第 18 步：可空字段一律省略键 + `shape_lint` 源码扫描防线 |
> | `e6d9947` | P4 第 19 步：`/api/health` 补齐 Dart 读的三个键 + `modifiedAt` 精度 |
> | `2b4f7d2` | Flutter：`api_error_envelope_test.dart`（读取点钉死）+ 修过期注释 |
>
> **下一件该做的事：`/api/ai/*` 的第三批 = `run-kit`** —— Task 2（`/api/ai/models`）已做掉
> （见 §8 "P4 第 20 步"）。但 **`run-kit` 不是"再搬一个只读端点"**：它要把
> `ToolRegistry`（工具清单 + `inputSchema` + `UNTRUSTED_BANNER_TEMPLATE`）、
> `SystemPrompt`（含 `VERSION`）/ `NsfwPrompt`、`InlineBudget` / `AttachmentPolicy`
> 一起搬过来，`kitVersion` 还要写 `app_settings`（`/api/sync/manifest` 读的就是它）。
> 也就是说它是 **R12 / R13 的第一次正面重建**，量和 `/api/ai/models` 不是一个档。
> 想先要一个"小而稳"的第三批的话，顺序是：**会话 / 消息 CRUD**（只读 + 追加，
> 形状与 `AiConversationRepo` 逐条对照）→ 再啃 run-kit → 最后才是 Run/SSE 与工具层。
>
> ---
>
> **2026-09-24 第二轮（13:06~13:55 这一段）共 9 个提交（5 个代码 + 4 个文档）**：
> `cargo test` **88 例**、`flutter test` **264 例**。
> 这一轮做掉了旧交接单里的 **①（M3.2 ② 离线可用）**、**② 的一半（工作流文件列出来）**，
> 下面按"价值 ÷ 风险"重排剩下的部分。
>
> | 提交 | 内容 |
> | --- | --- |
> | `613531f` | M3.2 第 3 步：持久化分叉（`send()` 全程走 `RunEventSource`） |
> | `85b0acc` | M3.2 第 4 步：手机冷启动分叉（桌面离线也能进聊天页） |
> | `1108673` | P4 第 14 步：`GET /api/capture/workflows` |
> | `e3d7f7d` | P4 第 15 步：`/api/capture/locate` + `/locate/apply` |
> | `0ddecbc` | P4 第 16 步：`GET /api/ai/providers`（`/api/ai/*` 的第一批） |

### ① ✅ 已做：M3.2 ② —— 手机离线能用（`613531f` + `85b0acc`）

三处都落地了：桥的 `createConversation`（上一轮的 `39ad4b7`）、
`ai_workspace_store.dart` 的持久化分叉（`send()` / `retry()` / `openConversation()` /
`_consumeRun()` / `stop()` / `answerQuestion()` / `_reloadMessages()` 一律走 `_source`）、
以及**冷启动分叉**（`_doLoad()` 分组：桌面照旧；手机走 `_loadLocalStartup()`，会话只认本地库）。

> ⚠ 下一轮容易踩的两点（都留了坑，别当成"已经全好了"）：
> 1. **provider / 模型清单在离线时仍然是空的**（`_loadLocalStartup` 里 best-effort 拉一次桌面，
>    失败就如实写进 `error`："桌面不在线，暂时拿不到模型清单…"）。所以**离线只能看历史会话**，
>    发新消息仍会被 `sendBlockReason()` 挡住 —— 要让离线真能发，得等 **M6**（凭据下行 + Android Keystore）。
> 2. 本地库的**空壳会话清理**走的是 `_api.deleteConversation`（桥还没有这个动作），
>    手机会攒下空壳。补法：桥加 `deleteConversation` + `RunEventSource` 加一个方法。

### ② 🔶 已做一半：P4 `/api/capture/workflows`（`1108673`）

**列出来**这一半已完成并过了真库验收（见 §8 那条登记）。**`/import-workflows` 仍是 501**，
它是下一个大块：`importAll` 依赖 `AiWorkflowSearch.loadWorkflowFromFile` + `WorkflowConvert` +
`ComfySubmitter`（界面格式 → API 图的转换、`tolerateUnsupported` 摘纯前端节点、`run_key` 幂等）——
这条链是 `comfy_submit` 的地基，**别为它抄一条捷径**（抄了就出现"列表说能跑、提交却报错"）。
搬它之前先想清楚：它要 ComfyUI 在场（界面格式转换靠 `/object_info`），
所以要么连 `/api/capture/*` 那一批一起搬，要么等 AI 层。

#### ③ P4：`/api/ai/*` 与 **R6~R10 的 Rust 侧重建**（最大的一块，别低估）

🔶 **起手了**：第 16 步做掉了 `GET /api/ai/providers`（见 §8）；**其余一点没动**。按 §6 的归属表逐条对照重建，
**先灰度非 AI 端点**（已经做完），AI 与工具层最后搬。顺序建议：
`/api/ai/providers`（不含密钥）→ `/api/ai/models` → `/api/ai/run-kit`（把 `kitVersion` 的
`app_settings` 写入逻辑一起搬，`/api/sync/manifest` 现在读的就是它）→ 会话/消息 CRUD →
Run 与 SSE（**R5/R6 的保留窗口与"失败不丢内容"**）→ 工具层与审批（**R8**）→ 附件（**R9**）。

> ✅ 可以直接复用的地基：`db.rs` 的 PRAGMA 读回自检（R1）、`BEGIN IMMEDIATE`（R2）、
> `sync.rs` 的行 → JSON 泛型映射与列清单现查（`PRAGMA table_info`）、
> `sync.rs` 里"永久性 / 暂时性错误要分清"的 `Rejected{code}` 形状（R6~R9 的错误码可沿用），
> 以及第 14 步刚铺好的 `comfy_dirs::looks_like_comfy`（`/api/capture/locate*` 直接用它）。

> 🔎 **`/api/ai/providers` 的起手笔记（2026-09-24：**已经照着做完，见 §8 第 16 步**；下面这四条已实测，留着给 `/api/ai/models` 与 run-kit 当模板）** ——
> 它就是 `AiRepo.listProviders().map { it.withCredential(credentials) }`，看着简单，
> 但有 **4 个会让键集合静默对不上**的点：
> 1. `AiProviderDto` 的字段是 `id / displayName / api / baseURL / credentialRef / endpointTrust /
>    enabled / revision / credential / createdAt / updatedAt` —— **后三个别漏**：
>    `Json.kt` 里 `encodeDefaults = true`，所以 `credential`（对象）与两个时间戳**一定会序列化**。
> 2. `credential` 是 `{configured, source, writable}` 三个键，**不含值**（AIH-012）。
>    判定顺序照抄 `CredentialService.describe`：`ref` 为空 → `(false, "none", false)`；
>    环境变量命中 → `(值非空, "env", **false**)`；否则看托管凭据文件里有没有这个 ref →
>    `(有, "managed" / 没有 "none", **true**)`。
> 3. `envValue(ref)` 认**两个**名字：`<ref>` 与 `COMFYHUB_CRED_<ref>`（前缀常量是
>    `CredentialService.ENV_PREFIX = "COMFYHUB_CRED_"` —— **不是** `COMFYHUB_KEY_`，写错了
>    就变成"永远读不到环境变量里的那把 key"）。
> 4. 托管凭据文件是 `<storage>\ai\credentials.dpapi.json`，**值由 DPAPI 加密**。
>    Rust 侧只要"有没有这个 ref"就能如实回答 `configured` / `source`（**不需要、也不该解密**）；
>    写成"永远 false"就是**假装没配过** —— 那属于 R13 那条"不许硬编码 / 不许假装"的范畴。
>
> ⚠ 因此这一步**先在真库副本上把 Kotlin 版的响应抓一份当形状基线**（或跑一次
> `comfyhub.ps1 up` 对着 8080 抄键集合），**别照着 DTO 猜** —— 这个仓库已经因为
> "少一个键 = 界面上少一块内容"栽过好几次（见 `api_test.rs` 里那些精确键集合断言）。

> 🔎 **`/api/ai/models` 的起手笔记（Task 2，2026-09-24 写在第 17/18 步之后 ——
> ✅ 已照此做完，见 §8 "P4 第 20 步"；下面留作"下一个只读 AI 端点"的模板）**
>
> 1. **错误体用 `ApiError::ai(code, 人话)`**，别用 `bad_request(人话)`：
>    `/api/ai/*` 抛的是 `AiException`，Kotlin 侧一律回 **400 + `{error: 稳定码, detail: 人话}`**
>    （`CONFIG_ERROR` / `UNKNOWN_MODEL` / `UNSUPPORTED_CONTENT` / `PROVIDER_UNREACHABLE` …，
>    见 `AiDomain.kt` 的 `AiErrorCode`），前端拿 `error` 分支、拿 `detail` 给用户看。
>    **普通错误反过来**（`error` 就是人话）—— 混了不会报错，只会让界面显示机器码（Task 1 那个 bug）。
> 2. **可空字段一律省略键**（`explicitNulls = false`）——**模型目录正是这条被实测出来时用的样本**：
>    真库副本上 Kotlin 的 `/api/ai/models` 里，`claude-opus-5-5` **没有 `thinkingFormat` 键**，
>    而且**所有模型都没有 `maxOutputTokens` / `capabilityVerifiedAt` 键**（`AiRepo.kt:253/254/274`
>    用 `isoTime(...)`，NULL → `null` → 键不出现）。
>    Rust 侧每写一个 `Option<T>` 就顺手加 `#[serde(skip_serializing_if = "Option::is_none")]`，
>    否则 `server-rs/tests/shape_lint.rs` 会红（那正是它的用途）。
> 3. **别对着 DTO 猜键名**：第 16 步就栽过一次 `base_url` → `baseURL`（差一个大小写、
>    界面读到的 URL 是空的）。做法照旧：真库副本起 Kotlin（8090）/ Rust（8091）、
>    **canon 后逐条比**（第 18 步收尾时那 13 个数据端点已经零差异，models 加进去应当仍是零差异），
>    键集合断言按"哪些字段在真库里有值"逐条算，**不要**写成"所有键一定都在"。
> 4. 顺带把 **`capabilityVerifiedAt` 这类时间戳**的取值口径统一：库里是 ISO-8601 UTC 文本，
>    照 `isoTime` 直出（别自己格式化）。

### ④ 可以顺手做掉的几个小的

- ✅ **`/api/capture/locate` + `/locate/apply`**：已做（第 15 步，`e3d7f7d`）。
- **手机侧的空壳会话清理**：桥缺 deleteConversation，于是本地库会攒下空壳（M3.2 ③ 留下的那条坑）。
- **i_run_events 的保留窗口（R5）**：pruneEvents 在 Kotlin 侧仍是死代码，Rust 侧更没有。
- ✅ **错误体 / 可空字段形状对齐**：已做（第 17~19 步 + Flutter 收口，见本节开头那张提交表）——
  这是 **Task 1**；**Task 2 = `/api/ai/models`**，上面那张起手笔记就是给它写的。

---

### 0. 上一轮收工前做的一次**全端点真库巡检**（结论：没有发现形状错）

用真实库副本（`--port 8086`）把**所有只读端点扫了一遍**，逐个核对状态码与形状 ——
这一步的价值是抓"某个字段名拼错导致整条链路 500"那类静默错误（P4 第 3 步就抓到过
`positive_prompt` 写成 `prompt_positive`）。本次结果全部符合文档：

| 端点 | 实测 |
| --- | --- |
| `health` / `tags` / `tags/categories` / `stats` | 200，形状与字段齐全 |
| `prompts`（含 `q=` 搜索）/ `prompts/{id}` / `prompts/{id}/media` | 200；不存在的 id → 404 |
| `media`（含 `kind=` 过滤）/ `media/{id}` | 200 |
| `media/{id}/file` | 200 `image/png`，**ETag = sha256**；`Range: bytes=0-9` → **206** + `Content-Range: bytes 0-9/2665`；`If-None-Match` → **304** |
| `media/{id}/thumb` | 200 `image/jpeg`（真生成，3129 字节） |
| `media/{id}/prompt` | **204**（这条产物没关联提示词 —— 三分语义正确） |
| `media/{id}/poster` | **501**（要 ffmpeg 抽帧，如实不假装） |
| `capture/config` / `prompts/{id}/workflow` / `api-graph` / `sync/*` | 200；`bootstrap?table=app_settings` → **400 且列出合法表名** |

> 📌 **顺带查清一件事（不是 bug，但要知道）**：`prompts.workflow_json` 里 **15/20 条是
> API 格式**（带 `class_type`）、只有 5 条是界面格式（带 `nodes`）—— 因为用户 `comfyui\*.json`
> 里本来就有一批是 API 格式的（文件名直接叫 `*_api.json`），导入时原文存进去了。
> `/workflow` 与 `/api-graph` **确实返回不同的东西**（20 条里 0 条相同，只是 310 那条恰好
> 一样长 2827 字节，差点被误判成"两个端点返回了同一份"）。
> ⚠ 但**界面格式的工作流查看器**遇到 API 格式的 `workflow_json` 会画不出图 ——
> 这是 **Kotlin 侧今天就有的行为**（Rust 只是原样读同一列，没有回归）。
> 要不要在查看器里识别 API 格式并降级提示，是个**待定项**（别当成 Rust 的 bug 去"修"）。

> ### 🔴 这次巡检**真的抓到一个线上级 bug**（已修，`9ffe17a`）
>
> `GET /api/sync/changes` 在**真实库副本**上直接 **500**：
> `no such column: id in SELECT * FROM ai_message_parts WHERE CAST(id AS TEXT) = ?1`。
> 两个同源缺陷：
>
> 1. **`ai_message_parts` 的主键是 `(message_id, ordinal)`**，触发器把 `pk` 写成
>    `message_id||'#'||ordinal` —— **`#` 分隔、且第一段是 UUID 文本**。
>    实现当初把它当成"单列 `id` 主键"（`prompt_tags`/`media_tags` 是 `:` 且两段都是数字，
>    三种编码各不相同，只对了两张表）。
> 2. **`push` 要求"表必须有 `id` 列"**，于是手机推上来的**消息块全被判 `SCHEMA_MISMATCH`**。
>    这条比 1 更重：错误码是**永久性**的，手机照契约会把 outbox 那行丢掉 → **永久丢数据**，
>    表现是"手机上能聊、桌面看不到内容"。
>
> 修法：`PkKind` 改成能表达三种形态（`Int` / `Text` / `Composite{sep, parts}`，
> 每个分量自带类型）；`push` 的主键列**从 `PRAGMA table_info` 现查**，不再假定叫 `id`。
>
> > ★ **为什么之前 74 条测试全绿也没抓住**：**夹具里没有 `ai_message_parts` 的行**。
> > 所以这次同时补了三道防线：公共夹具给**每张同步表**都放一行；
> > 一条"一次扫完所有同步表、10 张一张不少"的用例；一条"复合主键的 `deletes`
> > 按各自编码发数组"的用例（`["msg-0", 0]` vs `"att-0"`）。
> >
> > 📌 **教训（值得单独记）**：**"接口在夹具上绿"不等于"在真库上能跑"** ——
> > 夹具只覆盖你想到的行；真库会把你没想到的表、没料到的编码一次性摆出来。
> > 每个 P4 批次收尾时那次"真库副本抽查"不是走过场，**这次它挡下了一个会永久丢数据的 bug**。

### 10.9 切流盘点：**Kotlin 版 Windows 后端还差什么**（2026-09-26 逐条对账）

> 做法：把 Flutter 侧**真的会调**的路径全 grep 出来（`lib/**/*.dart` 里的 `/api/...`），
> 与 `server-rs/src/**/*.rs` 的路由表逐条比。**只有这张表清零，`server.ps1` / `pack-release.ps1` /
> `packaging\manifest.json` 才能切到 Rust、Kotlin 才能退役** —— 否则表现是"某个页面/按钮说接口不存在"，
> 而那不会有任何测试变红（e2e 与 flutter test 验的都是 Kotlin）。

**A. App 直接调用、Rust 还没有的（阻断切流，按用户会撞到的顺序）**

| # | 端点 | 用户会看到什么 | 规模 |
| --- | --- | --- | --- |
| A1 | `/api/ai/attachments{,/{id},/{id}/file,/{id}/thumb}` + `POST` 上传 + 创建 Run 前的三层准入（**R9**） | ✅ **已搬（P4 第 50 步）**：五条端点 + `POST /api/ai/preflight` + 创建 Run 前的库里事实闸门 + **图片真的内联**（三家适配器）；⚠ 仍缺**视频预览帧**（`/thumb` 回 204，见 A5）与**视频/音频/文档的传输方式**（三家适配器都只有图片的 `inline_base64`） | — |
| A2 | `/api/ai/providers/{id}/test`、`/{id}/discover-models` | ✅ **已搬（P4 第 53 步）**：真 deepseek / command-code-goat 实测 —— test 回 `HTTP 200` + `modelCount`，discover 回 2 / **82** 个候选（接口声明 0 / 内置目录 77 / 未识别 5，**逐维度合并**生效） | — |
| A3 | `/api/capture/import`（导入目录里的产物：含 PNG 内嵌工作流） | ✅ **已搬（P4 第 52 步）**：真产物目录 306 张里导 3 张 → `imported=3 / promptsCreated=3`（内嵌 prompt 还原成参数），再导一次 `duplicates=3` | — |
| A4 | ~~`/api/ingest/comfyui`~~ | ✅ **已搬（P4 第 49 步）** | — |
| A5 | `/api/media/{id}/poster` | ✅ **已搬（P4 第 51 步）**：起 `scripts\video-poster.ps1`（Windows 缩略图管线，**不依赖 ffmpeg**）；真机实测真 mp4 → 640×366 PNG + 缓存命中 | — |
| A6 | `/api/ai/capabilities/{refresh,probe}` | 能力目录那两个按钮回 **501**（Kotlin 侧是实现的，要在线数据集 / 真上游） | 中（**会花钱**） |
| A7 | `/api/ai/preflight` | 附件预检（选完文件那一刻的准入） | 随 A1 |
| A8 | `/api/auth/pair/start` | 扫码配对（**有意 501**，本轮只做"手机发起 + 桌面显示 6 位码"） | 有意不做 |

**B. AI 工具面（不阻断 App 启动，但"AI 能不能干活"会明显变窄）**

| 工具 | 状态 |
| --- | --- |
| `comfy_submit`（**R10**：审核 → 审批 → 提交 → 自己等 → `captureRun()` 入库 → `mediaIds`） | ✅ **已搬（P4 第 48 步，2026-09-26）**：`workflow_submit.rs` + `tests/submit_test.rs` 6 例；预检只有"节点类 + 实例能不能加载这个模型"两道（七项 audit / 盘扫 / `env_report` 仍未搬） |
| `comfy_find_workflow` / `check_inputs` / `fit_workflow` / `env_report` / `audit_workflow` / `prune_workflow` | ⛔ 未搬（都要 `ComfyModels` 本机清单；`env_report` 是 O3 —— "盘上有、实例看不到"的解药） |
| `comfy_use_media` / `comfy_make_mask` / `comfy_use_attachment`（图像编辑三件） | ✅ **已搬（第 58 步，2026-09-26）**：`server-rs/src/image_edit.rs` + 三条工具（出厂档 ASK）+ 16 例用例；**活实例验收**见 §13（AI 那条链路 30 项全绿、遮罩内 101.95/255 外 3.74/255） |
| 联网三工具 `web_search` / `web_fetch` / `civitai_search` + `find_model` | ⛔ 未搬（要 Keenable / Civitai 客户端；凭据端点已就绪）。⚠ **`danbooru_lookup` 已搬**（第 59 步，见 §14）—— 它是 Rust 侧第一条**联网**工具，顺带把"联网预算"这条只写在提示词里的规则变成真的拦得住 |
| `search_prompts` / `search_character_cards` / `save_character_card` / `danbooru_lookup` | ✅ **已搬**：角色卡两件（第 46 步）+ **`search_prompts` / `danbooru_lookup`（第 59 步，见 §14）**。§11 的"写角色前先查三处"在 Rust 后端**已齐**；`save_character_card` 的核对时间也真的接上了（只有本轮真查过 danbooru 才写得进）。⚠ `character_cards` 那张表**不在同步白名单**里，这条纪律别丢 |
| 工作流配方三件 `comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `workflow_recipes` 表 | ⛔ 未搬（Kotlin 侧 `WorkflowRecipe.kt` 646 行解析核心是主体；**故意不进同步范围**） |
| **后台捕获轮询器** + `lastPollAt` / `lastError`（O5/O6） | ✅ **已做（P4 第 47 步）**（用户自己在 ComfyUI 里跑的东西也会进画廊） |
| **截断续写**（`MAX_CONTINUATIONS` + `CONTINUE_INSTRUCTION`） | ⛔ 未做（输出被长度上限卡断时，现在把半句话当结尾） |
| 官方 comfy-mcp 39 个 | ✅ **第 45 步已做** |

**C. 切流动作本身（代码之外）**

0. ✅ **`<根>\.run\api.json`（端口状态文件）已由 Rust 写**（P4 第 54 步，2026-09-26）——
   这是"把 `server.ps1` 指向 Rust"的**硬前提**：App、`scripts\api-port.ps1` 与 ComfyUI 捕获扩展
   都只读这一个文件；不写它，切过去的表现是 **App 对着旧端口等 90 秒**（`docs/pitfalls.md` 记过现场）。
   纪律逐条照 Kotlin `ApiPortFile`：只由后端写、原子替换 + 带退避重试（读侧是轮询的，半截 JSON 就是幽灵 bug）、
   优雅退出时**只删自己那一份**（`pid` 对得上，否则重启时旧进程的收尾会删掉新进程的记录）、
   候选端口被占/落在 Windows 保留段就**回落到系统挑的端口**，并把 `requestedPort` + `fellBack` 如实记下来。
   **真机实测**：`--port 0` → 文件里写的是**真实端口**（13644，健康检查通）；
   占住 8106 再起 → 回落到 13650 + `fellBack:true` + `requestedPort:8106`，服务照常可用；
   `COMFYHUB_NO_PORT_FILE=1` 时一个字节都不写（只给我的验收脚本用，免得覆盖真后端那份记录）。
   ⚠ 顺带记一个坑：第一版成功分支用的是**请求的**端口，`--port 0` 就把 `0` 写进了文件
   （App 会去连 `http://127.0.0.1:0`）—— **实际端口必须从 `listener.local_addr()` 读回来**。
1. `server.ps1`：`Resolve-Backend` 认 Rust 二进制（`<根>\server\comfyhub-server-rs.exe` 与
   `server-rs\target\release\comfyhub-server-rs.exe` 两种布局）—— ✅ **端口文件那一半已经好了**（见 C-0），
   剩下的是脚本侧的路径解析与启动参数（传 `--port 0` 让后端自己挑，与 Kotlin 的自动端口同义）；
2. `comfyhub.ps1 doctor`：报"当前是哪种后端 / 二进制构建新鲜度"；
3. ✅ **已做（第 55 步，2026-09-26）**：`pack-release.ps1` + `packaging\manifest.json` 的 `backend`
   组件换成 **`rust_binary`**（装配 `<根>\server\comfyhub-server-rs.exe`），`jre` 组件降级成
   `required: false` + **默认不装**（要回退旧 Kotlin 后端才加 `-WithJre`），
   `runtimeRequirements` 里 Java 那条也标成 `legacy`。**实测**：`pack-release.ps1 -SkipApp` 装配出
   **13.8MB**（Rust exe 12.8MB；旧的 jre 40MB + jar 20MB 全没了），发布包里的 exe 在**全新目录**
   （无库、无 JRE）里直接起来：自己建库（schemaVersion=1）→ 写端口文件 → `db=ok` + 17 个工具。
4. ✅ **已做（第 55 步）**：**建表/迁移归 Rust** —— 新增 `db::open_or_create`：
   ① 库文件不存在（或空）→ 建父目录 + 建库 + 跑 `db/schema.sqlite.sql`（**编译期嵌进二进制**，
   与 Kotlin 从 jar 里读同一性质）+ 写 `user_version`；② `user_version == 0` 的老库 → 收敛式跑一遍 DDL；
   ③ 已是最新 → 什么都不做（**绝不重跑 DDL**）；④ 库比后端新 → **只警告不拦**（拦了就没法回退版本）。
   ⚠ 这是切流里被漏掉的一条硬要求：以前缺库直接 `exit 3`（"先让 Kotlin 起一次库"），
   **在发布包里等于永远起不来**（包里根本没有 Kotlin）。4 条用例 + 全新目录实测都过了。
5. ✅ **已做（第 56 步，2026-09-26）：四条 e2e + `flutter test` 全部对着 Rust 跑通了** —— 这就是 C-5。
   **过程本身就是价值**：第一次跑 `e2e-ai-tools-test.ps1` 是 **108 项 / 13 红**，13 条红全是 Rust 侧
   **真的漏了东西**（不是脚本的锅），逐条修完才全绿：
   * **`message.completed` 少 `parts`**（还少了 `finishReason` / `elapsedMs` / `reasoningEffort` / `steps` / `reasoning`）——
     按 Kotlin `HarnessRunner:838` 的键集合补齐；`parts` 与落库**共用同一份**列表（`parts_of`），
     免得"库里有的块、事件里没有"；
   * **没有标题状态机**（`[标题]…[/标题]` 既不摘、也不写会话名，标记直接漏进正文）——
     新增 `title_strip.rs`（`ConversationTitle.kt` 逐条移植：四个状态、回声只摘标记不改名、
     绝不吞内容 / 绝不重复），并改成**按轮**决定要不要带那条指令（摘到就不再提）；
   * **R6 在 429 上不成立**（说出去的正文一个字都没留、重试还把有内容的失败回复就地重置）——
     两处真因：① 起工具轮时 `take_text()` 把正文从 sink 里**拿走**了（现在先 `seal_text` 再 take，
     正文同时留在 `parts` 里）；② 重试判"空壳"只看 `text`、不看 `parts`（`isEmptyReply()` 是两条都要成立）；
   * **截断续写没实现**（`finish_reason=length` 把半句当结尾）—— 按 Kotlin 补上 `MAX_CONTINUATIONS = 3`
     + `isTruncatedByOutputLimit` + 续写请求里带上"已经说过的那半段"；
   * **助手消息里的 `tool_result` 没回填成 `role = tool`** —— 模型看到最后一条是 assistant，
     **又调了一次同一个工具**（实测抓到"重试多跑一轮工具"）；
   * **模型预检那一档过严**（只按 `/object_info` 的候选判，误拦了 e2e 第 4/5 幕的合法提交）——
     改成**只警告不拦**（盘扫还没搬；"误拦比白跑更糟"，与 Kotlin 一侧的实际行为一致），
     节点类那一档照旧拦，警告进 `preflightWarnings` 回给模型。
   **结果**：`e2e-ai-tools-test.ps1` **111 项全绿**、`e2e-submit-test.ps1` 全绿、
   `e2e-capture-test.ps1` 全绿、`e2e-pair-test.ps1` **50 项全绿**、`flutter test` **302 例全绿**、
   `cargo test` **429 例全绿**。
6. ✅ **已做（第 57 步，2026-09-26）：Kotlin 后端退役** —— 这就是 C-6。
   `git mv server legacy/server`（**保留历史**，源码留档当行为参照）；`.gitignore` 里
   `/server/build`、`/server/storage` 的规则跟着搬到 `/legacy/server/...`（这一步踩到了：
   `git mv` 会把**原本被忽略的构建产物与一份凭据文件**一起带进索引 —— 已 `git rm --cached` 清掉并补上忽略规则）。
   脚本侧：
   * `server.ps1` 的 Kotlin 路径指向 `legacy\server`，且**在 `-Backend kotlin` 时先检查源码在不在**，
     不在就抛一句人话（"Kotlin 已退役，默认就是 Rust"）；
   * `comfyhub.ps1` 的 `Get-ApiProcess` / `Get-ServerBuildStaleness` / `Get-RunningServerProcess`
     **全部换成认两个后端**（旧构建体检从 jar 换成 **Rust exe**，与 doctor 那条同一判据）；
   * **`up` 不再为 Rust 后端下 JRE**（只按需检查 Java 的旧路径整个跳过 —— 否则没装 JDK 的机器会白下 40MB）；
   * `runtime-deps.ps1` 的 Java 从 `Required = $true` 降成 `$false`（legacy）；
   * `pack-release.ps1` **删掉 `jre` 组件与 `-WithJre` / `-SkipJre`**，自检项只剩 Rust exe
     （`gradle_install_dist` / `jdk_runtime` 两个分支保留但明确标注"已退役的 legacy 组件"）。
   **文档**：README 的标题/现状/架构图/目录树/脚本表/环境表/打包说明、AGENTS 第 4/7/11 节与工具清单真源
   （`server-rs/src/tools.rs`）全部改成 Rust 现状。
   **验收**：`comfyhub.ps1 doctor` 报到 `后端二进制 … comfyhub-server-rs.exe`、`后端构建 最新`；
   `restart` 正常（Rust、`db=ok`）；`pack-release.ps1 -SkipApp` 自检只剩 `viewer.exe`（因 `-SkipApp`），**合计 13.9MB**。
   ⚠ **搬目录当场炸了一次**（值得记）：`server-rs/src/builtin.rs` 用
   `include_str!("../../server/src/main/resources/ai/builtin-catalog.json")` 编译期读 Kotlin 那一份
   冻结模型目录 —— 目录一搬，**cargo 直接编不过**。按该模块头注释里早就写好的计划办：
   把 JSON **搬成 Rust 侧资产**（`server-rs/assets/ai/builtin-catalog.json`，31KB，运行期唯一真源），
   老那份留 `legacy\` 当历史。**教训：退役一棵树之前，先 `grep include_str!|include_bytes!|Copy-Item.*<老路径>`**。

> **结论（2026-09-26，更新到第 54 步）**：**App 直接调用的 A 组已经清零**（A1~A5、A7 全部搬完；
> A6 要在线数据集/会花钱、A8 有意 501），所以"切过去之后界面报接口不存在"这一类**不会再发生**。
> 仍然不建议今天就默认切流，原因只剩两条：
> ① **A6** `capabilities/{refresh,probe}` 还是 501（设置页两个按钮，**要联网数据集 / 会花钱**，要用户拍板）；
> ② **B 组（AI 能力）还缺一块**：`comfy_*` 六件、图像编辑三件、联网三工具 + `find_model`、配方三件、
> `search_prompts` / `danbooru_lookup` —— 切过去 AI 仍能干活（MCP 39 工具 + `comfy_submit` + 附件都在），
> 但"AI 自己查模型 / 自己修工作流 / 自己上网找参考"这几条路会窄一截。
> **下一步**：C-1~C-3（脚本与发布包指向 Rust）+ C-5（四条 e2e 对着 Rust 跑一遍）；A6 单独问用户。
>
> **2026-09-26 这一轮补上的（第 47~49 步，都已 git 保存 + 实测）**：
> **后台捕获轮询器（O5/O6）** → **`comfy_submit`（R10）** → **推送式捕获 + 批量导入工作流**。
> 也就是说 §10.9 的 B 组里"`comfy_submit` + 轮询器"这一对已经清零，A3/A4 的
> `import-workflows` / `ingest/comfyui` 也清零；**剩下的排序不变**：
>
> 1. **A1 附件（R9）** —— 唯一一条"用户天天用"的阻断项（贴图 / 参考图 → 图像编辑链的地基）；
> 2. **A3 剩下的 `/api/capture/import`** —— 已经量过：要补一个 `PngMeta.readTextChunks` 的 Rust 版
>    （tEXt / iTXt / zTXt 三种块，~60 行）+ 目录遍历，其余全部现成
>    （`begin_run(force=true)`、`create_captured_prompt`、`import_file` 都在）；
> 3. **A2 设置页两个按钮**（`providers/{id}/test`、`/discover-models`）+ **A5 视频封面**（`media/{id}/poster`，要 ffmpeg）；
> 4. **C 组脚本切流**（含"Rust 要开始写 `.run\api.json`"这一条，否则 App 会对着旧端口等）；
> 5. 其余按 A/B 清单逐个清（B 组还有 `comfy_*` 六件、联网三工具 + `find_model`、
>   配方三件、`search_prompts`/`danbooru_lookup`）。
>
> **📌 后记（第 55~58 步，2026-09-26 晚）**：上面这份结论里的第 1~5 条**已经全部落地**
> （A1 附件 R9、A3 `capture/import`、A2 两个按钮、A5 视频封面、C 组切流 + Kotlin 退役），
> **图像编辑三件也已搬到 Rust**（第 58 步，见 §13）—— 所以"B 组还缺哪几块"以 §13.3 的
> 诚实清单为准（`comfy_*` 六件 / 联网三工具 / 配方三件 / `search_prompts` / `danbooru_lookup` 仍未搬）。



### 📜 上一轮（2026-09-24 第一轮）的交接单原文 — 保留作为对照

> 下面这三条是**上一轮留下的**，现在 ① ② 已完成/做了一半（见本节开头）。
> **原文一字未改**：里面那些"为什么非这么做"的论证（尤其"只搬一半 = 静默少结果"）
> 在第 14 步里就是照着执行的，删掉它们 = 下一轮有人把三份目录来源里的某一份"顺手简化掉"。

#### ① M3.2 ② —— 让手机**离线真的能用**（最高优先）

现在只到"harness **能被调到**"（`cba4016` 把桥的消息主键补齐了），但 `ai_workspace_store.dart`
在离线时仍会走 `_api.createConversation` / `listMessages` 打远端。所以**桌面不在线时手机还是发不出东西**。

要动的三处（顺序不能倒）：

1. `HarnessBridge` 补 `createConversation` / `listMessages` / `listParts` 的**本地**实现（`listMessages` /
   `listParts` 已经有，缺 `createConversation`）。
2. `ai_workspace_store.dart` 的**持久化分叉**：`send()` 里 `conversation == null` 那条分支与
   `_consumeRun()` 收尾的"与服务端对齐"都要按"本地源 / 远端源"分路。
3. 补一条 Dart 用例：**Android 上 `send()` 走的确实是本地源**（`openRunEventSource` 的返回值类型）。

> ⚠ 别做成"只把事件流换个来源"：那样离线时**事件是有的、界面上却不长东西**（会话/消息都查不到），
> 而两端都不报错 —— 和 `cba4016` 修掉的那个坑是同一类。

#### ② P4：`/api/capture/workflows` + `/import-workflows`（用户可见，但有个陷阱）

这两个端点背后是 `ComfyWorkflowFiles.kt`（411 行）+ `ComfyLocator.kt`（235 行）。**必须整体搬**：

> ⚠ **只搬一半 = 静默少结果**。`dirs(cfg)` 的目录来源有三条（配置里生效的输出目录的父目录 →
> `ComfyLocator.find()` 探到的 home → `ComfyRoots.autoReadRoots()`）。少搬任何一条，接口都会
> **正常返回、只是少了几个目录** —— 表现就是 AGENTS §10 那条"别把'库里没有'当成'这条工作流用不了'"
> 的复发：用户机器上明明有工作流，界面却说"没找到"。
>
> 另外 `ComfyLocator` 的判据**已经有两份实现**（`ComfyLocator.kt` 与 `scripts\comfy-path.ps1`），
> Rust 版是**第三份**。判据必须三边一致：源码形态 = `main.py` + `comfy/`；任何形态 = `output/` +
> `models/` 或 `input/`。改判据要同时改三处，否则会出现"App 说找到了、脚本说找不到"。

`sha2` 已经在依赖里（上传判重用着），列文件时要的 `file:<sha256>` 与
`comfy_load_workflow` 是**同一把钥匙**（所以导入天然幂等）。
必测：临时目录里摆几种形状的文件（正常界面格式 / API 格式 / 坏 JSON / 空文件 / 非 `.json`），
断言"只列真的能用的" + 目录发现覆盖三条来源。

#### ③ P4：`/api/ai/*` 与 **R6~R10 的 Rust 侧重建**（最大的一块，别低估）

这是 §7 风险表里"11,103 行 Kotlin 测试要重写"那块，**一点没动**。按 §6 的归属表逐条对照重建，
**先灰度非 AI 端点**（已经做完），AI 与工具层最后搬。顺序建议：
`/api/ai/providers`（不含密钥）→ `/api/ai/models` → `/api/ai/run-kit`（把 `kitVersion` 的
`app_settings` 写入逻辑一起搬，`/api/sync/manifest` 现在读的就是它）→ 会话/消息 CRUD →
Run 与 SSE（**R5/R6 的保留窗口与"失败不丢内容"**）→ 工具层与审批（**R8**）→ 附件（**R9**）。

> ✅ 可以在 Rust 侧直接复用的已有地基：`db.rs` 的 PRAGMA 读回自检（R1）、`BEGIN IMMEDIATE`（R2）、
> `sync.rs` 的行 → JSON 泛型映射与列清单现查（PRAGMA `table_info`），以及 `sync.rs` 里那套
> "永久性 / 暂时性错误要分清"的 `Rejected{code}` 形状 —— R6~R9 的错误码可以沿用同一套写法。

---

## 10. 两轮实测得到的优化清单 + 未完成清单（2026-09-25）

> **现场**：`让 AI 真的调 comfy mcp 出一张图` —— 起实例 → 跑用户自己的工作流 → 出图 →
> 按需求迭代提示词（4 版）→ 把产物收回库。每一条优化都标了**现场证据**与**该落在哪一步**，
> 按"不做会怎样"排序。三条实测坑的原始记录在 `docs/pitfalls.md`（`COMFY_BIN` / 共享模型根 /
> `/history` 是进程内的）。

### 10.1 实例生命周期：谁起 ComfyUI、带哪些参数（🔴 最高优先）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O1 | **ComfyHub 自己拥有"起实例"这条路**：Rust 侧搬 `ComfyEnv.prepareLaunch` 的三个参数（`--extra-model-paths-config` / `--input-directory` / `--output-directory`）+ 一个「带共享根重启」的动作 | `launch_comfyui` 起的**裸实例**让用户三条主工作流全部预检失败（`workflow_unknown_nodes`）；补上三参数后 unet 2→5、clip 2→5、lora 13→14，Anima 立刻能跑 | ✅ **已做（P4 第 45 步）**：`mcp.rs::prepare_launch` —— `launch_comfyui` 的参数由我们补，模型自己写过的一律不覆盖 |
| O2 | **MCP 桥若下发 `launch_comfyui`，必须替用户带上那三个参数** —— 不能指望模型自己知道这台机器有共享模型根 | 同上（模型只会调 `launch_comfyui()`，然后所有工作流"缺模型"） | ✅ **已做（P4 第 45 步）**（桥接层统一补，工具描述一个字没改） |
| O3 | **搬 `comfy_env_report`（+ `ComfyModels.inventory`）**：回答"实例能加载几个 / 盘上有几个 / 盘上有但实例看不到几个 → **重启实例，别下载**" | 这次的现场正是它要拦的那一类：`anima-base-v1.0` 明明在 `ComfyUI-Shared\models`（3.89GB），实例却看不到 | 优先于 `comfy_submit` 做 |

### 10.2 捕获与提交的时序（🔴）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O4 | **`comfy_submit` 自带"提交 → 自己等 → 立即 `captureRun()` 入库"**，轮询只作兜底 | 我关掉旧实例重开之后，**重启前那次运行从 `/history` 消失**（只剩重启后那条）—— ComfyUI 的 `/history` 是**进程内**的 | 第 46 步（R10） |
| O5 | **Rust 补后台捕获轮询器**（Kotlin `capture.start()` 的对应物） | 现在只有端点/工具触发一次：App 开着但没人点按钮时，**用户自己在 ComfyUI 里跑的东西不会被收** —— "AI 说生成了、画廊里没有"的另一半 | ✅ **已做（P4 第 47 步）**：`capture_poller.rs`；真机实测"什么都不点也收得到"（media 26→27 / runs 153→154） |
| O6 | 启动时也 poll 一次，并且**真的维护 `lastPollAt` / `lastError`** | 现在这两项如实回 null（没有轮询器），`/api/capture/status` 的这两个键永远不出现 | ✅ **已做（P4 第 47 步）**（成功写时间并清空错误、失败写人话；失败**不写** `lastPollAt`） |

### 10.3 "先查再写"的角色一致性（🟠 高，直接决定出图能不能一次对）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O7 | **新增 `search_prompts` 工具（桌面、只读）**：AI 现在**没有**"搜我自己的提示词库"这条路，只能盲写 tag | 我把用户参考图前景的 **shinku** 当成 iroi 写了两版废图；而库里 id 261 / 310 **就有 shinku 的完整 tag 栈**（`shinku_(nte), neverness_to_everness, pink streaks, purple horns…`）—— 一搜就知道 | P4 新批次（工具层，最小改动） |
| O8 | **角色卡**：把"角色的规范 tag 栈 + 来源 + 配哪条 LoRA / 权重"存成长期记忆或一张小表，生成前自动带上 | iroi 的规范 tag（绿发 / 黑卷羊角 / 羊耳 / 白蓝荷叶边裙）是从 Civitai 上**专为 Anima 训练**的 iroi LoRA 触发词里拿到的 —— 这正是"联网三工具 + `search_models`"的真实用途 | 工具层 + `MemoryStore` |
| O9 | **系统提示补一条纪律**：写角色前先查三处 → ① 用户自己的历史提示词（库里）② Civitai / LoRA 的触发词（联网）③ 再自己编；并且**把来源如实说出来** | 同上（"先参考 iroi(nte)"就是用户对这条纪律的原话要求） | `SystemPrompt`（要 bump `VERSION`，R12） |

### 10.4 生成参数的物理事实（🟠）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O10 | 系统提示 / 工具描述补一条：**turbo（加速 LoRA）通常要求 cfg≈1，此时负向提示词几乎不起作用** —— 想排除某元素要写进正向的反面描述，或提高 cfg 并接受质量变化 | 实测：把 `ferris wheel` 写进负向、cfg=1.0，摩天轮照样出现（v3/v4 都这样） | 系统提示 + `comfy_audit_workflow` 的一条警告 |
| O11 | `steps / cfg / sampler` 的推荐区间，由 `comfy_env_report` / audit 结合"当前 checkpoint 是不是 turbo"给出，别让模型猜 | 同上 | 同 O3 / audit |

### 10.5 观测与排错（🟠）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O12 | **MCP 桥必须抓子进程 stderr，失败时把最后 N 行放进错误的 `detail` 返回** | 少 `COMFY_BIN` 时，MCP 只回 `Error executing tool xxx` —— 真因被 rich 的方框排版埋在 stderr 里，Kotlin 侧也只记 debug 日志 | ✅ **已做（P4 第 45 步）**：stderr 单独排空留尾 40 行，失败时把末尾 12 行拼进 `ToolFailure.message` |
| O13 | 我们的工具错误统一带"最后几行原始输出"（`detail`），不只给稳定码 | 同上（外部进程失败时，码+人话不够定位） | 工具层（`ToolFailure` 的约定） |
| O14 | 把**排错捷径**写进 pitfalls 与系统提示：`comfy.exe <子命令> --json` 手敲 → 结构化信封（`error.code/message/hint`） | 这次就是靠它一眼看出"缺哪个模型、有哪些候选"（比我们自己的预检还清楚） | 文档 + 系统提示 |

### 10.6 出图迭代的方法（🟡）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O15 | "复刻参考图布局"要**逐标志物点名 + 位置**（左/中/右），只写大类 tag 模型会搬库存意象 | v1 只写 `theme park`，模型自己加了摩天轮、换了城堡样式；v3/v4 点名"粉顶城堡在左 / 巨型蛋筒在右 / 彩虹轨道 / 石栏杆前景"之后构图才稳住 | 生成纪律（系统提示）+ pitfalls |
| O16 | **固定 seed 的"改一处、只比一处"对照流程**固化成脚本/工具（这次是手搓的 4 版） | 4 版里只有 1 个变量在变才好归因；手搓容易一次改三样 | `scripts\anima-gen.ps1` 扩展 |

### 10.7 可复用的排错 / 验收资产（🟡）

| # | 优化 | 现场证据 | 落点 |
| --- | --- | --- | --- |
| O17 | **`.run/mcp-probe*.py` 收进 `scripts\`**（`scripts\mcp-probe.ps1 -Tool … -Args …`）：怀疑 MCP 不通时有一条现成命令 | 这次全靠它才把 39 个工具、`COMFY_BIN`、真因 stderr 摸出来；留在 `.run\`（不进版本库）等于下次重写一遍 | `scripts\` + README 脚本速查 |
| O18 | **活实例端到端收成 `scripts\e2e-live-comfy-test.ps1`**：copy DB → 起 Rust → `POST /api/capture/poll` → 查 media | 这次的"真库 + 真 ComfyUI"验收是临时拼的；而 `spot_check`（`#[ignore]`）只能验"连不上"那一支 | `scripts\` + README |

### 10.8 还没做完的清单（截至 2026-09-25）

> ⚠ **这是 2026-09-25 的快照**（保留下来是为了看当时的判断，别当现状读）。
> 它下面这张 Rust 表里的多行**已经被后来的批次做掉了**：附件 R9 → 第 50 步；`/api/media/{id}/poster`
> → 第 51 步；`/api/capture/import` → 第 52 步；`providers/{id}/test` 与 `discover-models` → 第 53 步；
> **图像编辑三件 → 第 58 步（§13）**；**`search_prompts` / `danbooru_lookup` → 第 59 步（§14）**。
> 最新状态一律看 §10.9 的 A/B 表与 §13/§14 的诚实清单。

**桌面 Rust（P4 轨道）**

| 项 | 状态 |
| --- | --- |
| **MCP 桥（官方 comfy-mcp 39 个工具）** | ✅ **已做（P4 第 45 步，2026-09-26）**：`server-rs/src/mcp.rs`（stdio JSON-RPC + 注册表 + 权限档 + `prepare_launch`）；真机验到 `/api/ai/tools` 回 53 个（39 个 MCP） |
| **`comfy_submit`**（R10：审核 → 审批 → 提交 → 等 → 入库 → `mediaIds`） | ✅ **已做（P4 第 48 步，2026-09-26）**：`workflow_submit.rs` + `tests/submit_test.rs` 6 例；预检只有"节点类 + 实例能不能加载这个模型"两道（七项 audit 与盘扫仍未搬） |
| `comfy_find_workflow` / `check_inputs` / `fit_workflow` / `env_report` / `audit_workflow` / `prune_workflow` | ⛔ **未做（2026-09-27 复核：`tools.rs` 的 `IMPLEMENTED`（22 个）里确实没有它们）**（前四个要 `ComfyModels` 本机清单）。⚠ **这条现在有直接后果**：AI **查不了**"本机有哪些模型 / 这份工作流的输入齐不齐"，于是 `inpaint-composite` 里凡是写 `comfy_env_report` / `comfy_check_inputs` 的地方**都执行不了**，只能退化成 `list_dir` 或直接问用户（skill v2 已按这个事实改写） |
| `comfy_use_attachment`（把附件投进 ComfyUI `input\`） | ✅ **已做（P4 第 58 步，2026-09-26）**：图像编辑三件之一（`image_edit.rs`，见 §13） |
| **联网三工具** `web_search` / `web_fetch` / `civitai_search` + `find_model` | ⛔ 未做（系统提示里那一节仍指向没下发的工具；凭据端点已就绪） |
| **附件（R9）**：`/api/ai/attachments*` + 三层准入 + 内联预算 | ✅ **已做（P4 第 50 步）**：`attachments.rs`（`routes.rs:545~551`）+ `tests/attachments_test.rs` 4 例（见 §6） |
| **截断续写**（`MAX_CONTINUATIONS` + `CONTINUE_INSTRUCTION`） | ✅ **已做**：`executor.rs:460~478` —— 被输出长度上限卡断时再要一轮把话补完，**不把半句当结尾** |
| **后台捕获轮询器** + `lastPollAt` / `lastError` | ✅ **已做（P4 第 47 步，2026-09-26）**：`capture_poller.rs` + `/api/capture/status` 两个键真的维护；真机"什么都不点也收得到" |
| `/api/capture/{import,import-workflows}`、`/api/ingest/comfyui` | ✅ **三条全搬（P4 第 49/52 步）**：`capture_import.rs`（`import` 会把 PNG 内嵌的 prompt 还原成提示词）+ `tests/import_folder_test.rs`；路由在 `routes.rs:302/304/306` |
| `/api/media/{id}/poster`（视频首帧封面） | ✅ **已做（P4 第 51 步）**：`poster.rs` 起 `scripts\video-poster.ps1`（Windows 缩略图管线，**不依赖 ffmpeg**）；抽不出帧如实回 204 |
| `/api/ai/capabilities/{refresh,probe}` | ✅ **已做**：`routes.rs:290/293`（另有 `probe-toggle`）；`tests/provider_probe_test.rs` |
| `POST /api/auth/pair/start`（扫码配对） | ⛔ 501（**有意**：本轮只做"手机发起 + 桌面显示 6 位码"） |
| **`WorkflowConvert` 两处欠账**：subgraph（组节点）展开、**`emulateFrontend`（模拟用户行为）** | ⚠ 都进 `unsupportedNodes` 如实报（默认档直接拒、`tolerateUnsupported` 摘掉给缺口清单）；工具描述是逐字搬的 → 结果里主动补一句 warning |
| **切流收尾**：schema 建表/迁移归 Rust、`server.ps1` / `pack-release.ps1` / `manifest.json` 切到 Rust、Kotlin 退役 | ✅ **已完成（2026-09-26）**：桌面后端已是 Rust，脚本与发布包都切过去了，旧 Kotlin 源码在 `legacy\server\`（留档）—— 见 AGENTS §11 |
| 已知行为差异（切流验收清单）：悬挂引用时 `/api/media/{id}/prompt` 回 404，Kotlin 回 200 + `{error:…}` | ⚠ 待确认（Dart 只判 204/2xx，行为无差异） |

**移动（M 轨道）**

| 项 | 状态 |
| --- | --- |
| **M5 排队提交**：手机 `comfy_queue_submit` → 桌面 outbox 消费者 → 产物 + `mediaIds` 回手机 | ⛔ **未开始**（`server-rs` 里 `submit_job` 这个 kind 一次都没出现；手机侧工具与 outbox 入队已在） |
| M4 补全：`run` / `submit_job` 两种 kind 的上行通道 | ⛔ 未做（现在只推 `conversation` / `message` / `part`） |
| M4 补全：前台定时 / 网络恢复时触发上行 | ⛔ 未做（只在**冷启动**与**每轮 Run 收尾**各推一次） |
| **M6 剩余**：离线时 provider / 模型清单可用（本地目录缓存） | ⛔ 未做（离线只能看历史会话；凭据下行 + Keystore 已在第六轮验穿） |
| 手机侧 `web_search` / `civitai_search` | ⛔ 未实现（`declaredTo = kit ∩ 本地实现`，所以模型**看不到**这两个工具，不会"调了必然失败"） |
| 手机空壳会话清理（桥缺 `deleteConversation`） | ⛔ 未做（M3.2 ③ 留下的坑） |
| **M3.2 ④ 真机对着 Rust 后端回归** | ⛔ 未做（上一次真机验收是对 **Kotlin**；对着 Rust 只需要"发一条 → 桌面看得到"） |

**防线与验收资产**

| 项 | 状态 |
| --- | --- |
| **R9 / R10 的 Rust 侧防线** | ✅ **已重建**：`tests/attachments_test.rs` 4 例（R9）+ `tests/submit_test.rs` 6 例（R10）—— 见 §6 |
| 活实例端到端脚本（O18） | ✅ 已有：`scripts\e2e-inpaint-test.ps1`（工作流本身）/ `e2e-image-edit-test.ps1`（AI 那条链路）/ `e2e-recipe-test.ps1` |
| `e2e-submit` / `e2e-capture` / `e2e-ai-tools` / `e2e-pair` 四条脚本 + `flutter test` + harness 用例**对着 Rust** 跑一遍 | ✅ **已做（2026-09-26 切流）**：四条 e2e + `flutter test` 对着 Rust 全绿（AGENTS §11）。⚠ **抠图这一摊落地后要重跑**（`e2e-image-edit-test.ps1` 会加 `-Matte` 一档） |
| 文档随切流更新（README 的"现状"描述、AGENTS §11 的阶段标记） | ✅ 已做；**§11 这张表本身在 2026-09-27 复核时校正过一遍**（原先有 8 行写着"⛔ 未做"而其实早已搬完） |

---

## 11. 2026-09-26 这一轮：角色卡（含 danbooru）+ 图像编辑（5 个提交）

> **用户这一轮的三句话**：① "我觉得人物卡那块还有得优化：对于支持 tag 的模型（比如 Anima），
> 除了专门 LoRA 内置的提示词外**也要查查 danbooru**"；② "目前我们的项目还不会自己做图像编辑
> 工作流。然而我之前有'保持背景不变，更换有具体动作的人物并保持画风'的需求，这时候就应该
> **先生成人物，然后再用图像编辑把人换上去**吧？"；③ "继续做下去。每完成一项就 git 保存。
> **最好能让我在真实 app 而不只是后端上测试**"。
>
> 落点与 §10 的 O7/O8/O9 对齐，**Kotlin 优先**（App 现在连的就是 Kotlin，这样第 ③ 条才成立；
> Rust 侧按老规矩随后逐条对照搬）。

| 提交 | 内容 |
| --- | --- |
| `9e96135` | **角色卡数据层**：`character_cards` 表（`normalized` 唯一索引做幂等 upsert 的键、R3 不用 `INSERT OR REPLACE`、**刻意不进同步范围**）+ `CharacterCardRepo` + `/api/ai/character-cards` 五条端点。`save` 是**同名合并**（`tags` 追加去重）—— 全量替换会把上一轮存的 tag 栈悄悄抹掉；超长字段与不存在的参考图一律**报错**（400 + 人话），不静默截断。8 例仓储回归 + 真后端逐条 curl（含 6 个错误分支） |
| `3b78fc9` | **AI 工具四件**：`search_prompts`（搜自己的库 / 按 id 取一条）、`search_character_cards` / `save_character_card`（**没查过 danbooru 就写不出 `tags_verified_at`** —— `ToolContext.danbooruVerified` 记"这一轮真查过的 tag"，核对时间是**事实**不是模型的自称）、`danbooru_lookup`（一次 ≤4 个请求：精确 tag → 模糊 tag（换装版本）→ wiki（别名 + 简介）→ **同现 tag**（按 frequency 排序 = 规范 tag 栈的来源））。新 `ToolCategory.LIBRARY` |
| `e665e10` | **系统提示 v29 第 18 条**（写角色前先查三处，并写明"LoRA 触发词只是来源之一"）+ **删掉 `NSFW_BLOCK` 尾部那份陈旧的纪律重复**（用户拍板"纪律只留一份"）—— 它在 NSFW 开着时会被读第二遍、而且**排在后面占上风**（还写着 v19 已推翻的"civitai 要先配令牌"、v18 已改掉的"以问句收尾"）。越狱正文一个字没动；两个版本号一起 bump（`nsfw-v2` → `nsfw-v3`），生成物重跑（金标 739→575 行、`nsfw_prompt.rs` 69KB→35KB） |
| `2800eda` | **App 里的角色卡页面**（用户第 ③ 条）：从「提示词」页右上角进（**不做第六个一级导航项** —— 手机窄屏六个图标会挤成两行，而 `Navigator.push` 的系统返回天然正确）。搜索 + 多列卡片 + 编辑弹窗 + 删除确认；**"没跟 danbooru 核对过"是一行显式提示**；编辑保存**原样带回 `tagsVerifiedAt`**（弹窗不给改，但漏带就把事实抹了）。4 例 widget 用例（含真机尺寸 384x853 不溢出） |
| `ef8af86` | **图像编辑**：`AiImageEdit`（`comfy_use_media` 把**画廊产物**投放进 ComfyUI input；`comfy_make_mask` 生成**白=重绘区**的遮罩，region 支持 `full`/`left`/`right`/`top`/`bottom`/`center`/`person`/归一化 `x,y,w,h`，`invert` 反选、`feather` 羽化，文件名按参数指纹算）+ **项目自带两条 inpaint 工作流** + skill `inpaint-composite` + 系统提示 v30 第 19 条 + `scripts\e2e-inpaint-test.ps1` |

**这一轮的硬证据**（不是"跑过了"，是量出来的）：

- `gradle test` **642 例全绿**（新增 8 例角色卡 + 9 例 danbooru + 6 例图像编辑 + 若干纪律断言）；
  `cargo test` **315 例全绿**（192 lib + 121 api_test + 2 shape_lint，含 run-kit 签名与 promptVersion v30 的更新）；
  `flutter test` **302 例全绿**、`flutter analyze` 无问题。
- **图像编辑在活实例上验穿**（真 ComfyUI + 真画廊底图 896x1152，区域 `0.58,0.26,0.36,0.74`、羽化 16）：
  **遮罩内平均像素差 100.5/255、遮罩外 0.73/255** —— 该换的真的换了、该保的一个像素都没动。
  工作流先过 `GET /object_info/<class>` 逐节点校验（类型 / 必填输入 / 连线）再真跑
  （校验脚本在 `.run/validate-workflows.py`，可重建）。
- **danbooru 真站抽查**（opt-in：`COMFYHUB_LIVE_DANBOORU=1`）在 JVM + 代理上跑通：
  拿到 `shinku_(nte)`、别名"真红"、同现 tag（`horns` 等）、wiki 正文。

**顺带记进 [`docs/pitfalls.md`](pitfalls.md) 的四条真坑**（都是这一轮实际撞的）：

1. **PowerShell 变量名大小写不敏感**：`$W`（画布）与 `$w`（区域）是同一个变量 —— e2e 脚本里
   画布被区域尺寸覆盖，提交上去的 `ImageScale` 变成 323x852（产物尺寸"莫名其妙"就是这么来的）。
2. **ComfyUI Desktop 的安装目录与共享目录是两处**：实例真正读写的是 `…\ComfyUI-Shared\{input,output}`
   （判据要按**配置里生效的输出目录**推；`comfy-path.ps1` 的 `Resolve-ComfyOutputDir` 给的是安装形态，
   拿它的父目录推 input 会写错地方 → `Invalid image file`）。
3. **danbooru 前面有 Cloudflare managed challenge，按 UA 形状挑人**（同一 URL 只换 UA 实测 6 种：
   短 UA 200、描述型/Mozilla 403）。所以 `DANBOORU_CLOUDFLARE` 单列错误码 +
   `COMFYHUB_DANBOORU_UA` 口子，**绝不说成"没有这个角色"**。
4. **`LatentComposite` 没有 mask 输入**（潜空间整块合成会把背景一起覆盖）—— 要按"白=重绘区"
   贴人得用**图像空间**的 `ImageCompositeMasked(destination, source, x, y, resize_source, mask)`，
   之后再 `VAEEncode → SetLatentNoiseMask → KSampler`。写工作流必须对着 `object_info` 核必填输入。

**这一轮没做的（诚实清单）**：

| 项 | 状态 |
| --- | --- |
| 这四件工具 / 五条端点在 **Rust 侧**的对照实现 | ✅ **已做（第 58 步，2026-09-26）**：图像编辑三件（`server-rs/src/image_edit.rs`，见 §13）。⚠ **角色卡那四条端点/两件工具与 `danbooru_lookup` / `search_prompts` 仍未搬** —— 搬的时候注意 `character_cards` **不在同步白名单**里（总纲 §3.3） |
| 角色卡同步到手机（`/api/sync/*` 加表） | ⛔ 未做（要连 `sync.rs` 的 `SYNCED` / `pk_kind` / `PHONE_FORBIDDEN`、`MobileSchema.kt` 两份清单、测试夹具一起改） |
| "抠人保背景"要**干净的边缘**（带 alpha 的 matting / rembg） | ⛔ 未做：现在只有"框 + 羽化"，边缘一圈会被一起重绘。用户要"抠得干净"就得有分割模型 —— 有没有先用 `comfy_env_report` 查，没有如实说 |
| `comfy_audit_workflow` 里加一条"角色 tag 与角色卡/danbooru 是否一致"的检查 | ⛔ 未做（现在只在系统提示与工具描述里约束） |
| 真实 App 的人手验收（打开页面、让 AI 真跑一遍两阶段） | 🔶 本轮已构建 debug 版并截图确认角色卡页可用（见下）；**让 AI 真跑一遍两阶段**需要真 provider（会花钱）与用户的 ComfyUI，留给用户按屏幕确认 |

---

## 12. 2026-09-26 这一轮：工作流配方（让 AI 学会"新模型该怎么接上去"）

> **用户这一轮的原话**：「我想可能很多模型都有 image edit 工作流，比如我刚在
> `D:\Comfy-Desktop\...\user\default\workflows` 里导入了 krea2 的图像编辑工作流。
> 甚至视频模型都有作为 image edit 基底模型的资质。**如果以后出了新的模型，
> 如何让 AI 自主学习新模型泛用/特定工作流的搭建？** 继续完成，每完成一项就 git 保存。」
>
> **设计（为什么这么做）在 [`docs/workflow-recipe-design.md`](workflow-recipe-design.md)**：
> 工作流拆成**任务骨架（泛用）+ 模型槽位（特定）+ 参数档位（特定）**三层，
> AI 从磁盘上已有的具体工作流里把前两层**归纳**出来、**落库沉淀**，新模型只需"绑到槽位"。
> 本节只登记**落地进度与硬证据**。

| 提交 | 内容 |
| --- | --- |
| `b12ee5d` | **工作流配方 1/5：设计**（`docs/workflow-recipe-design.md`：骨架/槽位/档位三层 + 学习闭环六步 + 新模型的三种情形 + W1~W5 + 明确不做 + 真文件抽查 §5.1） |
| `0b4177b` | **W1 解析核心**（`WorkflowRecipe.kt`：任务七档 + 骨架指纹 + 模型槽位 + 参数旋钮 + 输出 + `render`/`toJson`；11 例单测；真机 16 份文件抽查抓出三件事） |
| `f567bbe` | **W2 配方表 + 仓储 + 四条端点**（`workflow_recipes` 表 + `WorkflowRecipeRepo` + `WorkflowRecipeLearner` + `/api/ai/workflow-recipes`；9 例 + 真库副本抽查） |
| `164b94a` | **W3 三件工具 + 绑定层**（`comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `WorkflowRecipeBinder`；11+5 例） |
| `394461c` | **绑定端 + e2e 脚本**（`POST /api/ai/workflow-recipes/{id}/bind`，与工具共用同一份实现；`scripts\e2e-recipe-test.ps1` 35 项 + 两条脚本自带防线） |
| `37f3c42` | **系统提示 v31 第 20 条 + 内置 skill `workflow-recipe` + README/文档** |

**这一轮的硬证据（不是"跑过了"，是量出来的）**

- `gradle test` **681 例全绿**（新增 `WorkflowRecipeTest` 11 + `WorkflowRecipeRepoTest` 9 +
  `WorkflowRecipeBinderTest` 11 + `ToolRegistryTest` 6 + 系统提示版本/第 20 条 1）。
- `pwsh -File scripts\e2e-recipe-test.ps1` → **35 项全部通过**（真库副本 + 真端口，跑完删掉）。
- **真机抽查 16 份工作流文件**（用户在 `…\user\default\workflows` 里那批）：
  任务分档全中（t2i / i2i / inpaint / edit / video / audio），`edit` 那两份正是用户点名的
  Krea 2 图像编辑；抓出三件事（见设计 §5.1）——**其中一条直接定下了"默认拒学缺件图"这条纪律**。
- **真库副本 + 真 ComfyUI 抽查**（W2 那次，8095 只写副本）：项目自带的 API 格式 inpaint 工作流
  学成 `recipeId=1 / inpaint / complete=true / 3 槽位 / 26 旋钮`；用户真实那份 UI 格式的 Krea 2
  编辑工作流 → **400 `UNSUPPORTED_NODES`**（逐条列出摘掉的 4 个类与 4 条悬空输入 ——
  因为**当前跑着的实例里没有那几个 Krea2 自定义节点**）；`tolerateUnsupported=true` 才学，
  `task=edit`、`complete=false`；四条错误分支与 list/detail/delete 全对；
  真库副本上 `user_version` 仍是 1、`workflow_recipes` 只有 1 个 touch 触发器、`_sync_log` 里 0 行。
- 内置 skill `workflow-recipe` **真机验过能被加载**（`/api/ai/skills` 里 `source=builtin`、无 `validationError`）。

**这一轮没做的（诚实清单）**：

| 项 | 状态 |
| --- | --- |
| Rust 侧的对照实现（配方三条工具 + `workflow_recipes` 表 + `bind` 端点） | ⛔ 未做（沿用本轮 Kotlin 优先：App 连的是 Kotlin；Rust 侧按老规矩随后逐条搬，"不进同步白名单"这条也要一起搬） |
| **骨架改写**（unet+clip+vae ↔ 整包 checkpoint 的节点增删与重连线） | ⛔ 未做（设计 §4 W5；本轮只**如实报**"现有骨架接不上、要改写骨架，本轮不做"并给两条路） |
| 配方同步到手机 | ⛔ 未做（**有意**：手机提交走 `comfy_queue_submit`，R11；拿到它跑不了的配方只会误导模型） |
| 界面上看配方 / 绑定的页面 | ⛔ 未做（列表 / 详情 / 绑定端点已就绪，界面按需再接；AI 那侧已经全通） |
| "跑通之后自动回写配方"（把 `capture_runs` 里真跑过的模型与档位标到配方上） | ⛔ 未做（现在靠模型自己如实转述 + `notes`；要自动化得从提交后的最终图反推骨架指纹） |
| 真机上第 4 幕（"有缺口就拒学"）的**自动**回归 | 🔶 半自动：ComfyUI 不在跑时脚本自动改验"连不上要如实报 `COMFY_UNREACHABLE`"；"有缺口就拒学"那条本轮是**手动**跑出来的（当时 ComfyUI 在场、而实例里没有那几个 Krea2 节点） |

---

## 13. 2026-09-26 这一轮：图像编辑三件搬到 Rust + 活实例验收（第 58 步）

> **用户这一轮的原话**：「继续实施…你可以自行实测 AI 编辑图片功能，使用 `ref_photos/` 里的图片。
> 每完成一项就 git 保存。」
>
> **起点**：§11 的图像编辑在 **Kotlin** 侧落地（`ef8af86`），但 Kotlin 已退役（C-6，`legacy\server`）——
> 也就是说**当时运行的那个后端根本没有这三件工具**，AI 走不了"保背景换人物"这条路。
> 缺口在 §10.9 的 B 组里写着"⛔ 未搬"。

| 提交 | 内容 |
| --- | --- |
| `3fa1784` | **图像编辑三件（Rust）**：新增 `server-rs/src/image_edit.rs`（逐条对照 `AiImageEdit.kt` + `AiWorkflowSearch`）—— `resolve_input_dir` / `input_filename_for` / `use_media` / `use_attachment` / `make_mask`（region 七档 + rect/ellipse 抗锯齿 + invert + 羽化 + 参数指纹做文件名）；三条工具进 `desktop_tools`（描述与 schema **逐字**搬 Kotlin，R12/R13）与 `tools.rs` 的 `IMPLEMENTED`（17 → 20）；**12 例单测 + 4 例集成用例** |
| `0c48c83` | **活实例验收资产**：`scripts\e2e-image-edit-test.ps1`（假网关驱动三轮工具调用 + 30 项断言 + 像素级判据）；`fake_title_gateway.py` 新增模板 G「编辑图片」/ H「投放产物」 |
| 本提交 | 文档登记（本节 + README 脚本速查与功能表 + `docs/pitfalls.md` 两条真坑） |

### 13.1 三个"为什么这么做"（改这块之前先读）

1. **`resolve_input_dir` 拆成纯函数那一半**（`input_dir_from(configured, home)`）。
   配置里生效的输出目录优先 → 探到的 home 兜底 → 都没有就如实报 `COMFY_NOT_FOUND`。
   拆出来不是为了好看：**开发机装着 ComfyUI**（`find_home` 连 Desktop 安装形态都认），
   于是"两处都没有"那一支在集成用例里**永远走不到** —— 而它恰恰是新用户第一次用时会撞上的那一支
   （发布包里没有 ComfyUI 目录）。现在那一条由单测直接钉住。
2. **投放的文件名必须确定、可重复**（`<原名>-<来源id前8位><扩展名>`）。
   模型是**照着工具返回的 filename 写 `overrides`** 的，名字每次变一点就会出现
   "重跑工作流指向一个已经不在的文件"，而报错文案是 `Invalid image file`（像"文件不存在"）。
   单测盯的就是这条（同来源同名字、不同来源不互相覆盖、纯点号/无扩展名的边界）。
3. **布尔参数只认 `true` / `false`**（不把 `"yes"` / `1` 猜成真）。整数参数则相反（认 `"768"`）。
   判据是**猜错的代价不对称**：遮罩 `invert` 猜错方向 = 把整张图重画一遍（用户看到的是一张废图），
   而 `width` 猜不出来只会得到一个人话报错。

### 13.2 硬证据（不是"跑过了"，是量出来的）

- `cargo test` **446 例全绿**（298 lib + 125 api_test + 23 其他；本轮新增 12 例单测 + 4 例集成用例）。
  新用例同时挡住两处"清单漂移"：`/api/ai/tools` 与下发 `tools[]` 的顺序（20 个）、
  `desktop_tools` 清单与 `IMPLEMENTED` 的交集。
- **活实例（真 ComfyUI + RTX 4060 Laptop + 真照片）**：`scripts\e2e-image-edit-test.ps1`
  对**两条自带工作流各 30 项检查全绿**。底图 = `ref_photos\ref_bg_1.jpg`（3072x4096）缩到
  **768x1024**（长边 1024、向下取 16 的倍数 —— 8GB 显存上直接跑 3072x4096 会 OOM）：
  | 工作流 | 遮罩内平均像素差 | 遮罩外 |
  | --- | --- | --- |
  | `comfyhub_inpaint_anima.json` | **101.95 / 255** | **3.74 / 255** |
  | `comfyhub_inpaint_krea2.json` | **120.26 / 255** | **3.75 / 255** |
  （遮罩外不是 0 是对的：非遮罩区也要过一遍 VAE 编码→解码，那一步有损。）
- **只读 ComfyUI 的 `/history` 逐字核对过"执行的到底是哪张图"**（比工具自己的回包更硬）：
  两份 `LoadImage` 都指向投放的文件名 `edit-bg-00000000.png`、`LoadImageMask` 指向 `edit-mask.png`、
  两处 `ImageScale` 都是 768x1024、`CLIPTextEncode` 就是我们给的提示词、`KSampler.denoise = 1.0`
  —— 覆盖参数真的落到了**执行的那张图**上，而不是只出现在工具回包里。
- 端到端还盯住两件只有活实例才抓得住的事：**投放写进了实例真正在读的 `input` 目录**
  （Desktop 把程序与共享数据分在两处，写错地方的表现是 `Invalid image file`），
  以及**投放的文件与上传的附件逐字节相同**。

### 13.3 这一轮没做的（诚实清单）

| 项 | 状态 |
| --- | --- |
| **krea2 那份自带 inpaint 工作流不适合做图像编辑**（2026-09-26 活实例实测，**结论**） | ⛔ **已定论**：① 纯重绘（按提示词重画遮罩区）——同一组参数只换 seed 跑 3 次，**只有 1 次跟随提示词**，另两次把重绘区里原有的灯柱 / 围墙照旧渲染；② 贴人物图（两阶段）——denoise 0.5 时人物图那块**带一圈白框**，而且**cfg 1.0 与 4.0 的白框一模一样**（→ 白框不是引导强弱的问题，是 `ImageCompositeMasked` 把人物图**整块像素矩形**贴进遮罩区、不做抠图），denoise 提高到 0.85 白框没了但**贴进来的人变成了另一个人**。同一天 anima 那份在纯重绘这一档 **3/3 跟随提示词**。→ **图像编辑一律用 `comfyhub_inpaint_anima.json`**；krea2 那份留给**文生图**（它在 t2i 上没问题：阶段一那张白底红裙人物图就是它生成的）。写实风格要做图像编辑，只能如实说明限制（抽卡 + 人工挑，或换一个写实的 inpaint 底模），**不许硬用之后说"做好了"**。落点：`inpaint-composite` skill 开头那张实测表 + 工作流自己的 `_meta.title` + `docs/pitfalls.md` |
| 两阶段（"先生成人物 → 再 `20.image` + `denoise 0.4~0.6` 贴上去"）的**融合档** | ✅ **已实测并落地**（2026-09-26，用户连问三个问题之后）：① **矩形遮罩 → 一圈白框**（`ImageCompositeMasked` 把人物图那块矩形连背景一起贴）→ `comfy_make_mask(mode="subject")` 照**人物形状**抠剪影（背景色取四周中位数 + 只留最大连通区 + **填被包住的同色窟窿** —— 白裙子那种"人变半透明"就是靠填洞修的）；② **人物被重画 / 画风被换** → 新增 `comfyhub_paste_person_anima.json`（**两个 LoadImageMask**：`12` 合成用剪影、`26` 只喂 `SetLatentNoiseMask`）+ `comfy_make_mask(..., ring=N)` 产出"只圈人物外面 N 像素"的环 —— 人物像素一个都不在重绘区里；③ **摆放**（`scale` + `x`/`y`）已做，人物像素与遮罩**一起搬**（脱节就是"人在这儿、遮罩在那儿"）。实测：单遮罩 denoise 0.5 时**她的红裙子被模型换成白蓝裙**；ring=14 时**同脸同发色同红裙原样保留、只融接缝**（`e2e-image-edit-test.ps1 -TwoStage -Ring 14` **38 项全绿**，遮罩内 88.77/255、遮罩外 5.49/255；环形遮罩占画面 7.8%）。④ **背景色要与主体不同**（用户 2026-09-26 指出"背景色里面的空白没剔除"）：白底 + 白裙子在像素上无法区分，填洞只能猜（腿间/发丝之间的背景空白会被当成主体 → 那圈白晕）；正解是阶段一用**绿幕/品红**并把同色传给 `bgColor` —— 工具按背景饱和度分档（高饱和 → **不填洞**），显式色**自纠**（占比 >90% 说明色号不对 → 回落成边框估计并回 `bgColorIgnored`）。实测绿幕图**无白边**（遮罩内 77.04/255、外 4.92/255）。⑤ **按背景改新图层的光照/阴影方向**（用户最后一个要求）：机制已做（`light` = `auto`/`left`/`right`/`top`/`bottom`：光向从底图的四周亮度差**估**、色偏按背景/人物均色比、沿光向线性 ramp、影子按反方向平移+羽化后**烧进人物图像素并落进遮罩**，附 2 例单测），但 **`light=auto` 的观感仍不合格**：查清并修掉两个真因（ring 用了摆放前的剪影 → 重画区留原地；影子的羽化扩进剪影内部 → 人物像素被换成了压暗的背景），修完**人回来了**，但影子的偏移/羽化比例不对（贴着人形成黑边）、人物偏暗、剪影里还留绿幕块 —— **默认 `none`，不许宣称能用**；下一步见 pitfalls。⚠ **没做的**：抠图仍是**颜色抠图**，复杂照片背景抠不出来（要任意背景得装分割模型：本机 `rembg` 有包无模型、easy-use / SAM 目录是空的 → 真要就得下 ~176MB，**要先问用户**）；跨画风（动画人物 + 照片背景）本身仍违和，那要阶段一就用同风格底模 |
| 角色卡四条端点 / `danbooru_lookup` / `search_prompts` 在 Rust 侧的对照实现 | ⛔ 未搬（§11 那张表里只剩这几条；`character_cards.rs` 的两件工具是第 46 步搬的） |
| skill `inpaint-composite` 与系统提示第 19 条的**复核** | ✅ **已复核并改（2026-09-27）**：文案里提到的三件工具早就在 Rust 后端下发；这次按调研结论**改了正文** —— 第 19 条补上"先想能不能不抠（两条更稳的路）/ 贴人物图首选 AI 软遮罩 + 先查 `mattingModels.usable` / 缺模型指引用户下载、别硬跑也别默默降级 / **krea2 不适合做编辑**"，并把 `PROMPT_VERSION` 从 **v30 bump 到 v32**（⚠ 跳过 v31：Kotlin 那条线的 v31 带第 20 条配方规则，而配方三件在 Rust 侧还没搬，不能写那一节）。`kitVersion` 签名随之变化，下次开 App 会 bump 一档（这正是那条闸要的效果）。用例：`第19条钉住图像编辑的四条实测结论` |
| `scripts\e2e-image-edit-test.ps1` 进**常规**回归 | 🔶 它要真 ComfyUI 在跑（会花显卡时间），所以和 `e2e-inpaint-test.ps1` 一样属于"手动/按需"那一档，不进无网无卡的日常四连 |

### 13.4 顺带记进 [`docs/pitfalls.md`](pitfalls.md) 的两条真坑

1. **用独立进程起 ComfyUI 做活实例验收，日志一重定向就"跑一半没了"** ——
   warning 正文里的 `✓` 撞 GBK，`UnicodeEncodeError` 直接把进程抛死（栈套在 `logging` 里，
   看着像"某个自定义节点装坏了"）。**要带 `PYTHONUTF8=1` / `PYTHONIOENCODING=utf-8`，
   并用 `scripts\silent-process.ps1` 脱离进程树**（`Start-Process` 起的进程会被命令行结束一起回收，
   症状一模一样：日志有头没有尾）。
2. **PowerShell 读 ComfyUI 的 `/object_info` 会直接抛**（`keys with different casing`）——
   自定义节点里真有只差大小写的两个键，`ConvertFrom-Json` 默认那个大小写不敏感的对象建不出来。
   一律 `-AsHashTable`；坏的不是 ComfyUI 的 JSON，是 PowerShell 的解析器。


---

## 14. 2026-09-26 这一轮：资料库两件搬到 Rust（第 59 步）

> **用户这一轮的原话**：「继续实施…每完成一项就 git 保存。」
>
> **起点**：§11 的"写角色前先查三处"（O7/O8）在 Kotlin 侧落地（`3b78fc9`），
> 第 46 步把**角色卡**那一半搬到了 Rust，剩下的两条（`search_prompts` / `danbooru_lookup`）
> 一直挂在 §10.9 的 B 组里 —— 而它们是那句纪律里**模型真正要调的两个工具**。

| 提交 | 内容 |
| --- | --- |
| `a3470d6` | **资料库两件 + 联网预算**：`search_prompts`（与 `/api/prompts` 同一个实现）+ 新增 `server-rs/src/danbooru.rs`（四步有界查询 / 归一化 / 六种失败分开 / 挑战页单列）；`save_character_card` 的 `tagsVerifiedAt` 真的接上了（`ToolSession.danbooru_verified`）；**`MAX_WEB_QUERIES_PER_RUN` 从"只写在提示词里"变成执行器真的拦**，顺带修掉 `danbooru_lookup` 的分类漂移（Kotlin 是 WEB 档，Rust 表里写在 library） |
| 本提交 | **顺手对齐提示词搜索的语义**（`routes::query_prompts` 按 Kotlin `PromptRepo.buildWhere` 重写：拆词 AND + 5 列 + `kind` 大写/`ALL` + `favorite` 只在 true 时筛）+ 文档登记（本节 + §10.8/§10.9 清单更新） |

### 14.1 三个"为什么这么做"

1. **`search_prompts` 与 `/api/prompts` 必须同一份实现**（`routes::query_prompts` / `get_prompt_dto`
   抽成 `pub(crate)`）。各写一份 SQL 的后果是**"界面搜得到、AI 说没有"** —— 而那种不一致
   不会有任何用例变红（两边各自都对）。这一条与第 41 步"`comfy_list_workflows` 与
   `/api/capture/workflows` 同一个实现"是同一条纪律。
2. **danbooru 的六种失败必须分开**：`CLOUDFLARE`（挑战页，**绝不说成"没有这个角色"**）/
   `UA_REJECTED` / `NOT_FOUND` / `RATE_LIMIT` / `TIMEOUT` / `FAILED`。
   判据不只是状态码：**403 要看 body 是不是挑战页**，2xx 也要看（Cloudflare 有时用 200 送挑战页）。
   `wiki_pages` 与 `related_tag` 的 404 是**"没有这条增值信息"**，不许把已经拿到的角色 tag
   一起丢掉；429 / 5xx 才是"没查成"。
3. **核对时间是事实、不是自述**：`save_character_card` 的 `tagsVerifiedAt` 只由
   "本轮 `danbooru_lookup` 真的成功返回过这个 tag"写（查到的角色 tag 也算，
   归一之后进 `ToolSession.danbooru_verified`）。模型把 `source` 写成 `danbooru` 不算数 ——
   假时间比没有时间更坏。

### 14.2 硬证据

- `cargo test` **463 例全绿**（307 lib + 126 api_test + 7 danbooru_test + 23 其他）。
  新增用例里最要紧的几条：
  * 假 danbooru（**真 socket**，不是"调了个假函数"）：模糊那一路的**通配符真的发出去了**
    （`name_matches%5D=*shinku_%28nte%29*` —— 编码成 `%2A` 就再也搜不到换装版本，且不报错）、
    UA 就是那一种短的、wiki 404 不丢主结果、403 挑战页走 `DANBOORU_CLOUDFLARE`、
    429/5xx 与 404 分开、没解析出角色时**不发** wiki/related（一次查询最多 4 个请求）；
  * `search_prompts` 与 `/api/prompts` 搜到的是**同一条**，且正文里带够"照抄"需要的东西
    （正负提示词 / tag / LoRA@权重 / 参数 / 产物数）；
  * 联网额度：只认联网类工具、只在成功时计数、答题时与总额度一起归零；
  * 角色卡：查过 → 必须写核对时间；没查过（哪怕 `source` 写着 `danbooru`）→ 必须留空。
- **真站抽查**（`COMFYHUB_LIVE_DANBOORU=1 cargo test --test spot_check -- --ignored --nocapture`）：
  `shinku (nte)` → **`shinku_(nte)`（340 张）** + wiki 正文 + 别名 + 同现 tag
  （`shirt` / `two-tone_hair` / `white_shirt` / `hair_between_eyes` / `black_hair` / `purple_hair`…），
  5 秒内返回。这条证明的不是"代码能编译"，而是**本机这条链路真的通**
  （短 UA 过得去 + 公网走代理）——假服务永远覆盖不到这两件事。

### 14.3 这一轮没做的（诚实清单）

| 项 | 状态 |
| --- | --- |
| `web_search` / `web_fetch` / `civitai_search` + `find_model` | ⛔ 未搬（要 Keenable / Civitai 客户端 + `EndpointGuard`；`/api/ai/credentials` 已就绪）。⚠ **`danbooru_lookup` 是 Rust 侧第一条联网工具**，所以这一批把"联网预算"这条规则真的落地了 —— 后面三条搬过来时**直接复用**那份计数，别再各写一份 |
| `comfy_find_workflow` / `check_inputs` / `fit_workflow` / `env_report` / `audit_workflow` / `prune_workflow` | ⛔ 未搬（要 `ComfyModels` 本机模型清单） |
| 配方三件（`comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `workflow_recipes` 表） | ⛔ 未搬（`WorkflowRecipe.kt` 646 行的解析核心是主体；**故意不进同步范围**） |
| **`comfy_query` 那一档的预算** | ✅ **已补（2026-09-27）**：`ToolSession.comfy_queries_in_window` + `has_comfy_queries_left()`，执行器在**权限档之前**按**类别**（`ai_settings::category_of(..) == "comfy"`）拦，回 `QUERY_BUDGET_EXCEEDED`（文案与 Kotlin 逐字同义："本次回复查询 ComfyUI 的次数已达上限（9 次）；请基于已有信息作答，或让用户稍后再问"）。数字真源仍是 `policy::MAX_COMFY_QUERIES_PER_RUN = 9`（系统提示第 7 条 `«MAX_COMFY»` 就是替换它）。**没有另发明一套** —— 结构照 `has_web_queries_left` 那一条；用户答完提问时 `reset_budget` 与联网额度一起归零。用例：`comfy_查询额度只认_comfy_类工具且只在成功时计数`。⚠ 顺带钉住一个日后会漂移的点：Kotlin 里 **`comfy_env_report` 是 `FILES` 类**（只扫盘、不碰服务），所以不能靠 `_ => "comfy"` 兜底 —— 已在 `category_of` 里提前写好那一条 |
| `search_prompts` 的**多词 AND** 语义 | ✅ **顺手对齐了**（它本来是个静默漂移）：Rust 的 `query_prompts` 之前是**整串 LIKE + 3 列**（title / positive / negative），而 Kotlin `PromptRe| 两阶段（"先生成人物 → 再 `20.image` + `denoise 0.4~0.6` 贴上去"）的**融合档** | ✅ **已实测并落地**（2026-09-26）：① **矩形遮罩 → 一圈白框**（`ImageCompositeMasked` 把人物图那块矩形连背景一起贴）→ 用 `comfy_make_mask(mode="subject")` 照**人物形状**抠剪影（背景色取四周中位数 + 只留最大连通区 + **填被包住的同色窟窿**，白裙子那种"人变半透明"就是靠填洞修的）；② **人物被重画/画风被换** → 新增 `comfyhub_paste_person_anima.json`（**两个 LoadImageMask**：`12` 合成用剪影、`26` 只给 `SetLatentNoiseMask`）+ `comfy_make_mask(..., ring=N)` 产出"只圈人物外面 N 像素"的环 —— 人物像素一个都不在重绘区里，实测 ring=14 **同脸同发色原样保留、只融接缝**；③ **摆放**（`scale` + `x`/`y`）已做，人物像素与遮罩**一起搬**（脱节就是"人在这儿、遮罩在那儿"）。⚠ 没做的：抠图仍是**颜色抠图**，只对纯色/浅色背景有效（要任意背景得装分割模型：本机 `rembg` 有包无模型、easy-use/SAM 目录是空的 → 真要就得下 ~176MB，**要先问用户**）；跨画风（动画人 + 照片景）本身仍违和，那是阶段一底模的事 |

---

## 15. 📌 上一轮待办（2026-09-26 收工时的拍板）—— **15.1 已调研完（2026-09-27），落点见 §16**

> 用户收工前的原话：「**人变糊了**」+ 从两条路里选：「**上 matting 模型（rembg / BiRefNet 这类做软遮罩的）**」
> 与「**绕开抠图：阶段一就按最终背景生成（同底模 + 同场景，denoise 0.35~0.5 直接融），或用 denoise 0 只做纯合成**」，
> 并要求「**做标记，明天再做**」。
>
> 本节就是那份标记：**结论 + 待查 + 动手顺序**。上一轮的全部现场证据在 §13.3、§14 与 `docs/pitfalls.md`。
>
> ✅ **2026-09-27：本节已完成调研** —— 15.1 的"该不该上 AI 抠图"已用真图量出结论（**该上，而且模型早就在盘上**），
> 15.2 的"人变糊"顺带查出了**真因（前景被删掉 18%）**。**全部数字、机制与新发现见 §16**；
> 本节保留为当时的判断依据，**动手请以 §16 为准**。

### 15.1 拍板：抠图改用**软遮罩（matting）**，现有颜色抠图降级成近似

**用户的授权（已明确同意）**：可以**下载 matting 模型**（`~/.u2net` 不存在，u2net ≈ **176MB**；
easy-use / SAM / LayerStyle 的模型目录也都是空的）。所以下一轮**不用再问**，直接下。

为什么必须换：现在的边缘处理全是**颜色近似**（`drop_blended_edge` 按色距离、`despill` 压通道、
`unmix_edge` 按覆盖率反混），实测最干净也只有 **1.11% 像素带绿、最大超出 37/255**，
而且**一开大就吃人、一开小就留绿边**（用户连提三次，pitfalls 里三条记录都在）。
软遮罩（alpha matte）是这件事的**正解**：它对细发丝给的是**连续 alpha**，不是"删/不删"。

**动手顺序（照这个做，别自己发明）**：
1. **先看这台实例上现成有什么**（`/object_info` 已经列过一遍，四个候选）：
   `Image Rembg (Remove Background)`（`model` 可选 `u2net` / `u2net_human_seg` / `isnet-anime` ——
   动漫画风就选 `isnet-anime`）、`easy humanSegmentation`、`LayerMask: RemBgUltra`、
   `LoadBackgroundRemovalModel` + `Image Remove Background (Alpha)`。
   ⚠ **先探模型在不在**：这些节点多半会在**首次调用时静默下载**，所以要么先手动下到对应目录
   （`ComfyUI/models/rembg` 或 `~/.u2net`），要么盯住 `ComfyUI` 控制台的下载行 —— 别让"点了按钮没反应"变成新的坑。
2. **优先在工作流里算软遮罩**（不改 Rust 的颜色逻辑）：`图片 → 抠图节点(得到带 alpha 的 IMAGE 或 MASK)
   → MaskToImage → ImageScale(画布尺寸) → ImageToMask`，把这张**软遮罩**接到
   `comfyhub_paste_person_anima.json` 的 `12`（合成用）。已有两个 `LoadImageMask` 的结构**不用动**，
   只是 `12.image` 从"我们生成的硬剪影"换成"模型给的软遮罩文件"。
   —— 但软遮罩**不是一张 PNG 文件**而是节点输出，所以要么在**工作流里多加一段**（推荐：新一份
   `comfyhub_paste_person_matte_anima.json`），要么让 Rust 侧把 matting 节点的结果**落盘成一张图**再照旧走 `12.image`。
3. **Rust 侧**：`despill_edges` / `unmix_edge` / `drop_blended_edge` 保留，但**降级成"没有 matting 时的近似"**，
   并在工具返回值里如实说明用的是哪一档（`matteSource: "model" | "color-approx"`）。
   `mode="subject"` 的硬剪影照旧保留（矩形/剪影两条路都还要能用）。
4. **验收**（照老规矩量化，别只看图）：同一个人物图 + 同一张底图，比 **① 现在（颜色近似）
   ② 软遮罩** 两档：细发丝的**保留条数/面积**（`alpha∈(0.05,0.95)` 的像素数）、
   边缘带的**绿分量超出量**、以及**人物内部的锐度**（见 15.2 那把尺子）。

### 15.2 待查 bug：**"人变糊了"**（下一轮第一件事）

用户看图说"人变糊了"。**先查清再改**，怀疑三处（按可疑度排序）：
1. **`unmix_edge` 的除法**：`P' = B + (p-B)/a` —— `a` 越小放大越狠，边缘像素被拉成"半透明感的糊边"；
   而且它只该动**边界带**，若 `band` 算错会波及内部；
2. **ring 重绘跨过边界**：`SetLatentNoiseMask(ring)` 在 denoise 0.3~0.45 下会把**人物最外几像素**
   一起重新采样（环是从剪影**外面**画的，但羽化/掩码插值会让边缘那 1~2 像素落进重绘区）→ 边缘变糊；
3. **feather**：`feather 4~8` 让边界半透明，视觉上就是"糊"。

**动手方式（单变量对照，别一次改三样）**：同底图 + 同人物图 + 固定 seed/denoise，
只改一个变量：① 关 ring（`ring=0`）② 关反混（新加一个开关）③ 关 feather（`feather=0`），
各放大 1:1 存图对比，**并用一把客观尺子**——在人物**内部**（离边界 > 8 像素）取一块，
量**拉普拉斯方差**（或边缘梯度能量）：数值明显下降就是真的糊了，不是错觉。
判据写在用例里（同一个合成结果，内部锐度不许比"纯合成 denoise 0"低 20% 以上）。

### 15.3 同时保留"不抠图"的两条路（用户一起点的）

它们是**比抠图更稳**的两条：
1. **阶段一按最终背景生成**：底图当 i2i 起点 / 同底模同场景，阶段二 `denoise 0.35~0.5` 直接融
   —— **完全不需要抠图**，没有边缘问题。落点：skill 里把它写成**首选**，`mode="subject"` 变成"没有别的办法时才用"；
2. **`denoise 0` 纯合成**：一个像素都不改（画风 100% 保留），只靠 choke/软遮罩的 alpha 贴。
   ⚠ 前提是 **alpha 要准**（所以第 1 条修好之后这条路才真正可用）。

### 15.4 还挂着的（别忘）

| 项 | 状态 |
| --- | --- |
| `light`（按背景改光照/阴影方向） | ⛔ **默认 `none`，不许宣称能用**：机制 + 2 例单测在，但活实例观感不合格（影子贴着人形成黑边、人物偏暗、剪影里留绿块），四步修法见 pitfalls |
| 绿边 | 🔶 **未清零**：1.11% 像素带绿（最大 37/255）—— 15.1 的软遮罩是它的正解 |
| 边缘"糊" | ⛔ 未查（15.2） |
| 本项目**没有**的：分割/抠图模型 | ✅ **用户已批准下载**（176MB），下一轮直接下 |

---

## 16. 抠图调研：**该上 AI 软遮罩，而且模型早就在盘上**（2026-09-27）

> 用户这一轮的原话：「**继续。先调研抠图问题，用AI抠图比较好吧？**」
>
> 本节就是那份调研：**结论 + 硬证据 + 还没落地的部分**。尺子是 [`scripts\matte-probe.ps1`](../scripts/matte-probe.ps1)
> （`run` 跑模型、`measure` 出对照表），**不需要 ComfyUI 在跑**。证据图在 `.run\matte-research\`。

### 16.1 一句话结论

**该上，而且比总纲 §15.1 预想的便宜得多**：本机 `ComfyUI\models\rembg\` 里**已经有**两个像样的抠图模型
（BEN2 1.13GB + RMBG-1.4 176MB），而 ComfyUI 的 `comfyui-easy-use` 有个 `easy imageRemBg` 节点正好读这个目录
—— 所以"换软遮罩"这件事**零下载**就能做。§15.1 里写的"要下 ~176MB"是**当时没看到盘上已有文件**得出的结论。

**并且顺带查出了一个比"糊"更严重的问题**：现在的颜色抠图在这张绿幕人物图上**把 18.4% 的前景删掉了**
（眼睛、发髻内部、颈部）—— 那才是用户看到的"人身上一块块变透明"。

### 16.2 本机现成资产（决定了"要不要下"）

| 节点 | 用的模型 | 本机能不能**零下载**用 |
| --- | --- | --- |
| **`easy imageRemBg`**（`comfyui-easy-use`，显示名 "Image Remove Bg"） | `rem_mode` ∈ `RMBG-2.0` / `RMBG-1.4` / `Inspyrenet` / **`BEN2`** | ✅ **RMBG-1.4**（`models\rembg\RMBG-1.4.pth`）与 **BEN2**（`models\rembg\BEN2_Base.pth`）**已在盘上**，`get_local_filepath` 按 URL 的 basename 找，正好命中。RMBG-2.0 / Inspyrenet 要下（本机 HF 缓存里 RMBG-2.0 **只有 refs、没有 blobs**） |
| `Image Rembg (Remove Background)`（`was-ns`） | u2net / u2net_human_seg / **isnet-anime** / … | 🔶 走 `rembg` pip 包，模型在 `~/.rembg\models`（本机原本没有）。**isnet-anime 已于本次调研下好（176MB，用户已授权）** |
| `easy humanSegmentation`（`comfyui-easy-use`） | selfie_multiclass / human_parsing_lip…（mediapipe） | ⛔ `models\mediapipe` 不存在 → 要下 |
| `LayerMask: RemBgUltra`（`comfyui_layerstyle`） | 自带 `BriaRMBG` | ⛔ 它按 `models\rmbg\RMBG-1.4\model.pth` 或 `folder_names_and_paths['rmbg']` 找 —— **本机既没有这个目录，也没有任何节点注册 `rmbg` 这个文件夹**（grep 全仓只有一个读它的地方）→ 会静默失败 |
| `LoadBackgroundRemovalModel` + `Image Remove Background (Alpha)`（**ComfyUI 原生**） | `models\background_removal\` | ⛔ 目录里只有一个占位文件 → 要下 |

**环境（换机器要重新确认）**

- 跑模型的那个 python 是 **`D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\.venv\Scripts\python.exe`**
  （torch 2.10.0+cu130 / RTX 4060 Laptop / timm 1.0.28 / cv2 / einops / rembg 2.0.84 全齐）。
  ⚠ **`standalone-env\python.exe` 里没有 torch 也没有 timm** —— `e2e-image-edit-test.ps1` 的 `Find-Python`
  把它排在 `.venv` 前面，那是因为它只用来起假网关；**跑模型不能照抄那个顺序**。探针脚本是按
  "**第一个真能 `import torch, timm` 的**"挑的。
- 耗时（4060）：BEN2 加载 1.4~2.4s、推理 **0.86~3.46s**；RMBG-1.4 加载 0.7s、推理 **0.24~0.55s**。
- ⚠ **两个模型的推理分辨率都是 1024×1024**（内部 resize 再 resize 回原图）——
  长边明显大于 1024 的人物图，细发丝会在这一步被降采样抹掉。落地时留意。
- 依赖（BEN2 要 `einops` / `timm` / `cv2`；RMBG-1.4 只要 torch）**本机都装好了**，不会触发节点的静默 pip/下载。

### 16.3 实测数字（同一张真图，768×1024 绿幕动漫人物 + 夜景底图）

素材：主体 `comfyhub_t2i_anima_00023_.png`（双马尾、极细飘发、绿幕）、底图 `ref_photos\ref_bg_1.jpg`。

**① 模型本身的质量**（溢色 = 源图**半透明像素**上的绿分量超出，未做任何去溢色）

| 模型 | 耗时s | 前景占比 | **软像素占比** | 溢色均值 | 溢色max | 细碎连通区 |
| --- | --- | --- | --- | --- | --- | --- |
| **BEN2** | 1.13~3.46 | 0.42565 | **0.03157** | **11.57** | 36 | 24 / 380px |
| BEN2(`refine_foreground`) | 0.86~1.36 | 0.42566 | 0.02975 | 11.61 | 35 | 23 / 396px |
| isnet-anime（动漫专用） | 0.97~1.13 | 0.42775 | 0.02703 | 12.87 | 35 | 4 / 313px |
| RMBG-1.4 | 0.24~0.55 | 0.43132 | 0.02305 | 14.94 | 39 | 14 / 238px |
| 现状 颜色抠图 | — | 0.35635 | **0.00000** | — | — | — |

→ **BEN2 最好**（溢色最低、软像素最多）；isnet-anime 几乎相当（连通区更整、少"碎屑"）；
RMBG-1.4 最差但**快一个数量级**。**颜色抠图的软像素是 0** —— 它是硬剪影，没有连续 alpha 可言。

**② 前景到底掉了多少**（现状那条链，逐档量 —— 这才是"人身上一块块变透明"的实据）

| 档 | 前景占比 | 相对上一档 |
| --- | --- | --- |
| `subject_mask_from_image(tolerance=0.12)` 原始 | 0.43651 | — |
| + `drop_blended_edge(band=3, tolerance=0.35)` | **0.36295** | **−7.36 个百分点** |
| + `unmix_edge(band=3)` | 0.35635 | −0.66 个百分点 |

→ 合计 **−8.01 个百分点 = 前景的 18.4%**。差异图（`.run\matte-research\ab-diff.png`，蓝 = "只有 BEN2 认为是前景"）
上那 50,306 个像素**正好落在眼睛、发髻内部、颈部、双手、小腿**上 —— 也就是**五官和身上成片的地方**。

**③ 接同一套 Rust 后处理之后**（`unmix_edge` + `despill_edges`，参数两端一致 → 唯一变量是 alpha 的来源）

| 方案 | 前景占比 | 平灰底边界溢色均值 | max |
| --- | --- | --- | --- |
| 现状 颜色抠图 | 0.35635 | 0.00（**无软像素 → 指标空转**，它的病是掉前景） | 0 |
| 提议 BEN2 | 0.41496 | 2.56 | 16 |
| 提议 isnet-anime | 0.41675 | 3.68 | 33 |

### 16.4 换掉之后还剩的那点绿边：**despill 的作用域错了**

上表 BEN2 还剩 2.56/16，根因不是模型：`despill_edges` 只处理"剪影**边界往内** radius 像素"，
而**真正的溢色像素在掩码外面** —— 抗锯齿那几像素（人 + 绿幕的混合色）在颜色键里被判成了**背景**，
于是"往内"那条带**一个都没盖到**，而软遮罩会把这些像素按 α>0 合成上去。

实测（同一张图、同一个 BEN2 alpha）：把作用域换成"**半透明像素（α<0.98）**"或"**边界两侧各一圈**"，
平灰底边界溢色 **3.36 → 0.00、max 32 → 0**；只扩内侧怎么调 radius 都留一线。
→ 落地时 despill 的判据要从"**几何内带**"改成"**α<1 的那一圈（含掩码外）**"。

### 16.5 落地清单（①②③ 已做，④ 待 ComfyUI）

| # | 做什么 | 关键点 / 坑 |
| --- | --- | --- |
| 1 | **新工作流** `comfyhub_paste_person_matte_anima.json` | ✅ **已建（2026-09-27）**：`21(ImageScale) → 27(easy imageRemBg, rem_mode=BEN2, image_output="Hide") → 22.mask ← 27.MASK`；**ring 也从同一份 alpha 现算**（`28 = GrowMask(expand=14)` → `29 = MaskComposite(subtract)` = 长出来的那圈 **减去** 遮罩本身），不再要 Rust 落盘 ring 文件。**静态核过**：节点名 / 输入名 / 链接都对着本机 ComfyUI 源码核了一遍（`scripts\matte-probe.ps1` 是另一把尺子）。⚠ **还没在活实例上端到端跑过** —— 第一次跑要如实回报结果；`comfy_check_inputs` 这类预检工具当前后端**没有** |
| 2 | **Rust `comfy_make_mask` 加 `matteSource`**（`color` 默认 / `model`） | ✅ **已做（2026-09-27）**：`model` 档**不写合成遮罩、不写 ring**（遮罩由工作流的抠图节点现算），颜色剪影只当**定位器**（所以跳过 `drop_blended_edge` / `shrink`，免得把定位器往里啃）；只做"洗人物图那一份像素"（反混 + **两侧**去溢色），回 `personFilename`。顺带把 `despill_edges` 改成**两侧带**（`despill_band`）—— 默认路径一起修，实测绿边 3.36/max 32 → **0.00/max 0**；并修掉"反混结果只在 despill 为真时才落盘"（白底那条路的反混被算出来又丢掉）。⚠ `mode="subject"` 照旧保留（实例上没有抠图节点时它仍是唯一近似） |
| 2b | **`comfy_get_status` 报抠图模型在不在**（`mattingModels`） | ✅ **已做（2026-09-27）**：skill 原来让模型"自己找 ComfyUI 的 models 目录"，而它**找不到**（`comfy_env_report` 未搬、`inputDir` 指向的是**共享**那份、模型却在**安装**那份）。现在 `comfy_get_status` 返回 `mattingModels{usable, checked[每根各一行], download, note}` —— 真机实测：`ComfyUI-Shared\models\rembg` 空、`ComfyUI-Installs\ComfyUI\ComfyUI\models\rembg` 里两个模型都在，`usable=true`。⚠ 这个键**只加在工具返回里**，不加在 `/api/capture/status` 那个端点（它的键集合是界面契约，有用例钉着） |
| 3 | **skill `inpaint-composite`** | ✅ **已改（2026-09-27，version 3）**：贴人物图**首选** matte 那份工作流；新增"**抠图：先查模型、缺了指引用户下载**"整节（下哪个 / 体积 / 目标路径 / 为什么不能让它静默下载）；把颜色抠图的实测代价（掉 18.4% 前景）写进去；并把两处**执行不了**的旧指引（`comfy_env_report` / `comfy_check_inputs`）换掉 —— 先换成 `list_dir` + 问用户，再在 2b 落地后换成 **`comfy_get_status` 的 `mattingModels`**。另补一节"**有两条路比抠图更稳**"（§15.3 那两条）放在抠图之前 |
| 4 | **e2e 加一档 `-Matte`** | ⛔ **未做**：断言用 §16.3 的指标 —— **软像素占比 > 1%**、**平灰底边界溢色 ≈ 0**、**前景占比不低于纯合成档**。⚠ 它要**真 ComfyUI 在跑**才能写一版跑得通的（跑一次才知道从产物里能量到什么），所以留在"ComfyUI 起来之后"那一批 |

### 16.6 复现方式（这套尺子已经进版本库）

```powershell
# 跑模型（BEN2 / RMBG-1.4 零下载；isnet-anime 等走 rembg 会话，缺了会下）
pwsh -File scripts\matte-probe.ps1 run -Subject <人物图> -Models BEN2,RMBG-1.4 -Refine -Background <底图>

# 同一套尺子量整个目录（含"现状"那一份，用来出对照表与并排图）
pwsh -File scripts\matte-probe.ps1 measure -Out .run\matte-research -Subject <人物图> -Comparison <底图>
```

"现状（颜色抠图）"那一份基线是**一次性**用 Rust 真实现落的（跑完已删），调用链与 `make_mask` 的 subject 分支同序：

```
subject_mask_from_image(tolerance=0.12) → drop_blended_edge(band=3, tolerance=0.35)
→ shrink=0 → unmix_edge(band=3) → despill_edges(radius=6)
```
落盘成 `alpha-color.png` / `person-color.png`，再用 `measure` 与 AI 那几档同尺对比。

### 16.7 仍未做的（诚实清单）

| 项 | 状态 |
| --- | --- |
| `light`（按背景改光照/阴影） | ⛔ 仍**默认 `none`**，观感不合格（见 §15.4 / pitfalls 的 `light=auto` 那条） |
| "人变糊"的**拉普拉斯方差**那把尺子 | ⛔ 没做。但本次已找出"糊"的一个主因（**无软 alpha + 前景被删**），§15.2 的三处怀疑里第 1、3 条（`unmix_edge` 除法 / `feather`）仍待单变量对照 |
| 长边 > 1024 的人物图 | ⚠ 未处理：BEN2 / RMBG-1.4 内部都固定 1024 推理，发丝会被降采样 |
| 跨画风（动画人 + 照片景）本身仍违和 | ⚠ 那是**阶段一底模**的事，不是抠图能解决的 |
| **§15.3 那两条"不抠图"的路要写进 skill 当首选** | ⛔ 没做：① **阶段一就按最终背景生成**（同底模同场景，阶段二 `denoise 0.35~0.5` 直接融）—— **完全不需要抠图**，没有边缘问题；② **`denoise 0` 纯合成**（一个像素都不改，画风 100% 保留，前提是 alpha 要准 —— 软遮罩已解决）。skill v2 现在只是把 matte 提到首选，还**没**把这两条写成"能用就别抠" |
| **matte 工作流的活实例验收** | ⛔ 没做：文件已在版本库、静态核过，但**一次都没在真实例上跑过**（写这段时 8188 没在跑）。第一次跑要如实回报 |
