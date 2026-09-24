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
| **P4** | **桌面 Rust 重写** | 🔵 **进行中（第 1~8 步已落地 2026-09-23：`afcf395` / `bf955bb` / `49e82cd` / `83bc384` / `0693c12` / `65b6234`）**：`server-rs/`（axum 0.8 + tokio + **rusqlite 0.40 bundled** + serde + tower-http）。**第 1 步（只读）**：`db.rs`（连接工厂 + PRAGMA **读回自检** R1 / `BEGIN IMMEDIATE` R2 / FTS5·user_version 自检）、`models.rs`、`routes.rs`（`/api/health`、`/api/tags`(+categories)、`/api/prompts`、`/api/prompts/{id}`）。**第 2 步（写）**：`inputs.rs`、`repo.rs`（逐条对照 `TagRepo`/`PromptRepo`）、`write_routes.rs`（标签 CRUD；提示词 CRUD/收藏/复制/标签维护）。**第 3 步（产物）**：`media.rs`（列表/详情/**原件文件流**：ETag = `sha256` 内容寻址 → `If-None-Match` 304；单段 `Range` → 206 + `Content-Range`；目录穿越双保险拦截）。**第 4 步（产物写侧 + 统计）**：`media_write.rs`（`PATCH`/`DELETE`：`clearPrompt` 与 `promptId=null` 语义分开、空 body 回 404、删除连磁盘原件与缩略图一起清；`GET /api/stats` 七个字段）。**第 5 步（缩略图）**：`thumbs.rs`（`image` crate 替掉 ImageIO + TwelveMonkeys：最长边 512 / 不放大 / 透明先铺 #1E1E22 / JPEG 0.82；失败回退原图）。**第 6 步（上传）**：`media_upload.rs`（multipart 多文件：落盘名 32 位 hex+扩展名、**按内容 sha256 判重**、单文件一个事务、图片顺手写缩略图、单文件失败不整批失败）。**第 7 步（捕获配置 + 工作流原文）**：`capture.rs`（`GET/PUT /api/capture/config` 整体覆盖+归一化；`GET /api/{prompts,media}/{id}/workflow` 的 **404/204/200 三分**与原文直出）。**第 8 步（关联查询）**：`/api/prompts/{id}/media`（提示词不存在也回 200 + `[]`，与 Kotlin 同）、`/api/media/{id}/prompt`（产物不存在 404 / 没关联 204 / 有 200）。未搬的一律 **501**。默认端口 **8081** 与 Kotlin 的 8080 并存灰度 | ① `cargo test` **48 例**全绿；② **对真实库副本 + 产物目录副本起服务抽查**：24 条产物 8 页、`file` 200 且 ETag=sha256、`Range: bytes=0-9` → 206、`If-None-Match` → **304**、`stats` 27/24/16 + 分组、`PATCH` 三个分支（含 `bogus promptId` 400 / 空 body 404）、`DELETE` → `{"deleted":215}` 且**临时副本文件被删、真实产物文件未被动**、`media` 24→23；真库只读未改。**未搬**：`/api/media` 的 poster（要 ffmpeg 抽帧）、需要 ComfyUI 在场的 `/api/capture/{status,jobs,poll,locate*,import*,workflows}`、`/api/ingest/comfyui`、`/api/prompts/{id}/api-graph`、`/api/ai/*`、`/api/sync/*`；**未重建**：R6~R10；RSS ≤30MB 待量。⚠ **已知的一处行为差异**（切流验收清单要过）：悬挂引用时 `/api/media/{id}/prompt` 我们回 404，Kotlin 回 200 + `{error:…}`（前端会解析成空提示词） | P2、M0 |

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
| R5 重连续传 | `e2e-ai-tools-test.ps1` 部分覆盖 | ✅ **手机侧已补（M3）**：`RunEventBusTest`（截断只丢最老、seq 不回退）+ `HarnessStoreTest`（裁剪留尾部）+ 事件批量写 |
| R6 失败不丢内容 | `e2e-ai-tools-test.ps1` 第 13 幕（15 项）+ `ai_retry_keep_content_test.dart` | ✅ **手机侧已补（M3）**：`HarnessRunnerTest` 三例（429 半截 / 首字被压住 / 取消）+ `HarnessStoreTest` 两例 |
| R7 注入防线 | `SystemPromptNsfwTest`、`ToolRegistryTest` | ✅ **手机侧已补（M3）**：横幅文案只做 `{kind}`/`{query}` 替换（`MobileToolsTest` 断言前缀来自 kit） |
| R8 审批顺序 | `AskGateTest` | ✅ 桌面（M1）+ **手机侧（M3）**：`MobileAskGate` 先 open 再发事件，`MobileToolsTest` 有两例（手快先答不落空 / 没人答如实报） |
| R9 附件三层准入 | `e2e-ai-tools-test.ps1` 第 9 幕 | ✅ **手机侧已补（M3）**：`InlineBudgetTest` + 适配器未实现模态直接抛（`ProtocolAdapterTest`） |
| R10 提交链路 | `e2e-submit-test.ps1`、`e2e-capture-test.ps1` | Rust 侧重建（P4，**未动**） |
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
> `/api/ingest/comfyui`、`/api/ai/*` 的**其余部分**（Run 与 SSE / 工具层与审批 / 附件 ——
> **R6~R9 的 Rust 侧重建仍未开始**，那是 P4 剩下的最大一块，
> 也是 §7 风险表里 "11,103 行 Kotlin 测试要重写"那块）。
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

> **2026-09-24 第六轮（本轮）**：用户要求"继续按总纲实施，每实现一项就 git 保存；
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
