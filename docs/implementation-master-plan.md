# 实施总纲：SQLite + 桌面 Rust + 移动 Kotlin + Flutter

> **这份文件只回答三件事：现在是什么样 / 哪些线一条都不能破 / 怎么算做完了。**
> 逐轮流水账（原来的 §9~§27，约 1500 行："这一轮做了什么、实测数据多少"）**2026-09-30 已删** ——
> 要看历史用 `git log -p -- docs/implementation-master-plan.md`，或者按关键词搜提交信息。
>
> **🔒 = 已冻结的契约**：改它要同时改两端 + 那份契约文档（§3 有清单）。

---

## 1. 当前状态（2026-09-30）

```
┌──────────────── 桌面（Windows，唯一真源）────────────────┐
│  comfyhub-server-rs  (Rust 单文件 exe：axum + rusqlite bundled)
│    ├── 业务 API  /api/{prompts,tags,media,stats,...}
│    ├── AI API    /api/ai/*（Run 生命周期 / SSE / 工具层 / run-kit）
│    ├── 同步 API  /api/sync/{manifest,changes,bootstrap,push}
│    └── comfyhub.db (WAL)  ← 业务表 + _sync_log + sync_clients
│  storage/  二进制不进库（HTTP + ETag=sha256）
│  **没有** jre / mysql / JVM：发布包 ~14MB
└──────────────────────┬──────────────────────────────────┘
                       │ 局域网 HTTP + X-ComfyHub-Token（远程走 Tailscale）
┌──────────────────────▼──────────────────────────────────┐
│  安卓：Flutter UI ──platform channel──▶ Kotlin harness   │
│         │                                ├── LLM 调用 + 工具循环
│         │                                ├── 本地 SQLite（bundled）
│         │                                └── _outbox（上行队列）
│         └── 与桌面同一套 RunEvent 事件流（UI 代码零分叉）
└──────────────────────────────────────────────────────────┘
```

**三条不可动摇的架构纪律**：

1. **桌面是唯一真源**；移动端不直连 ComfyUI、不装 Python/MCP、**不自起服务端**。
2. **提交永远由桌面执行**（`comfy_submit` 只在桌面）；移动端产出"待提交工件"，不是提交动作。
3. **规则只有一份 —— 由桌面产出、版本化下发**（系统提示 / 工具 schema / 安全文案），两端都不许硬编码（R13）。

### 已完成的

| 阶段 | 是什么 | 现状 |
| --- | --- | --- |
| P0 / P0.5 / P1 | MySQL 过渡期止痛、binlog、事件日志治跑飞 | ✅（随 P2 作废：MySQL 已退役，那条链的目录也删了） |
| **P2** | MySQL → SQLite（保 Kotlin/Ktor） | ✅ 2026-09-23：逐表 `COUNT(*)` + 全表行哈希双向核对过 |
| **P4** | 桌面后端整体换 Rust（`server-rs/`） | ✅ 2026-09-26 切流完成；**2026-09-30 起 80 个模块分 10 个域**（地图见 `server-rs/src/lib.rs`），旧 Kotlin 源码连目录一起删了 |
| **M0** | 冻结 5 份契约 | ✅ 全部落文件（§3） |
| **M1** | 桌面侧支撑（提问/审批落库、pending 总览、run-kit、闸门类事件立即落库） | ✅ |
| **M3** | 移动端 Kotlin harness（事件/协议/本地库/工具层/主循环） | ✅ 代码已落地（145 Kotlin 用例 + 7 Dart 用例） |
| M3.2 ①②③ | harness 接进 UI（消息主键回传、本地建会话、持久化与冷启动分叉） | ✅ |

### 还剩的（按依赖）

| 剩什么 | 判据 |
| --- | --- |
| **M3.2 ④ 真机回归** | 手机上**离线**聊一次：本地库起会话来、事件一条不少地长到气泡上（M3.1 只验到"harness 能在真机上起来"） |
| M4 上行通道（手机侧接线） | 手机产生的会话在桌面可见可续写；重复推送不产生重复行（`/api/sync/push` 桌面侧已落地并有用例） |
| M5 排队提交（手机侧接线） | 手机排队 → 桌面在线后自动提交 → 产物入库画廊 → `mediaIds` 回到手机；桌面侧的审批/幂等/入库一条不少（`submit_jobs` + 消费者已落地） |
| M6 凭据下行 | 手机用 Android Keystore 独立调 LLM；界面明确说"需在手机上单独填 Key" |

---

## 2. 红线：实施时一条都不能破

每条都给出**防线** —— "删了以后出错还能不能被抓住"是唯一标准。

| # | 红线 | 防线 |
| --- | --- | --- |
| **R1** | **每条 SQLite 连接都必须 `PRAGMA foreign_keys=ON`** —— 默认 OFF 且按连接生效；漏了**不报错**，只是外键静默失效（`ON DELETE SET NULL` / `CASCADE` 全都白写） | 连接工厂里强制设置 + 一条"删标签后关联行也没了"的用例 |
| **R2** | 写事务一律 **`BEGIN IMMEDIATE`**，不用裸 `BEGIN`（读锁升级写锁可能不理会 `busy_timeout`，直接 `SQLITE_BUSY`） | 连接工厂唯一入口 + 并发写用例 |
| **R3** | **被同步的表禁止 `INSERT OR REPLACE`**（它内部是 DELETE+INSERT → 点亮 DELETE 触发器 → 往 `_sync_log` 写**假 delete**），必须 `ON CONFLICT(...) DO UPDATE` | 一条"upsert 不产生 delete 条目"的用例 |
| **R4** | `_sync_log.seq` 必须 **`INTEGER PRIMARY KEY AUTOINCREMENT`**（否则 rowid 复用 → 游标收到"用过的号" → 客户端永久丢一段变更） | 一条"删除后再插入，seq 不复用"的用例 |
| **R5** | **`ai_run_events` 的保留窗口必须够长**（手机后台被杀是常态，重连靠 `?after=<seq>`） | 环形缓冲（内存 2000 / 库里 4000，**刻意不同数**）+ `prune_events` 留尾部 + "断线重连仍完整"的用例 |
| **R6** | 失败/取消时**"已经说出去的话一个字都不能丢"**（正文必须落块 / 标题缓冲要 flush / 重试不许就地重置有内容的回复 —— 三条纪律见 AGENTS §10.1） | `e2e-ai-tools-test.ps1` 第 13 幕 + `test/ai_retry_keep_content_test.dart` |
| **R7** | 工具结果与网页内容当**不可信数据**：不拼进系统提示指令区，不因它说"忽略规则"而改行为（RSK-004） | 不可信横幅文案由 run-kit 下发（🔒 §3），两端不许各写一份 |
| **R8** | 需审批的工具：调用方**必须先 `open(callId)` 再发 `tool.requested`** | `approval_gate_opens_before_the_user_can_click`（不等事件、库里有 pending 就点）+ `a_denied_call_is_recorded_and_never_runs` |
| **R9** | 附件的**三层准入**（选完预检 → 发送前禁用 → 创建 Run 前用库里事实再验）；失败必须 `400 UNSUPPORTED_CONTENT` 且**上游请求数为 0** | `e2e-ai-tools-test.ps1` 第 9 幕 + `tests/attachments_test.rs` 4 例 |
| **R10** | `comfy_submit` **只认 API 格式节点图**（`capture_runs.raw` 里那份），产物必须走 `capture.captureRun()` 入库 | `e2e-submit-test.ps1` + `tests/submit_test.rs` 6 例 |
| **R11** | **移动端不得复用 `comfy_submit` 这个名字**（桌面离线时它无法履行"提交→等生成→返回 mediaIds"的契约，模型会误报成功） | 手机侧叫 `comfy_queue_submit` + 系统提示写明"排队 ≠ 已提交" |
| **R12** | 工具清单**只有一个真源**（`server-rs/src/tools/tools.rs` 的 `catalog`）；改系统提示必须 **bump `SystemPrompt.VERSION`** | `tools/tests.rs` + `system_prompt/tests.rs` |
| **R13** | **两端都不许硬编码系统提示 / 工具描述 / 安全文案**，只能来自 run-kit | 手机侧 `R13NoPromptLiteralsTest` 扫生产源码；桌面侧下发 `titleInstruction` / `continueInstruction` |
| **R14** | 安卓侧 **SQLite 必须用 bundled 驱动**，**绝不用系统 SQLite**（Android 13 仍是 3.32a、无 FTS5） | 依赖清单 + 启动时的 FTS5 探针（失败直接抛） |

---

## 3. 冻结的契约（🔒 真源在各自的文档里，这里只列清单）

| 契约 | 真源文档 | 改它要同时改 |
| --- | --- | --- |
| SQLite 约定（四个 PRAGMA / 类型映射 / `_sync_log`+`sync_clients`+`_outbox` / 加列怎么迁移） | [sqlite-schema-contract.md](sqlite-schema-contract.md) | `db/schema.sqlite.sql` + `infra/db.rs`（`SCHEMA_VERSION`、`NEW_COLUMNS`）+ 手机侧镜像表 |
| RunEvent 协议（信封 / 17 个类型 / SSE 帧 / 保留窗口） | [run-event-contract.md](run-event-contract.md) | `ai/events.rs` + `ai/run_lifecycle.rs` + Kotlin `RunEventBus` + Dart 消费者 |
| 同步 API 4 条端点（游标语义 / 410 分支 / 幂等与 LWW） | [sync-api-contract.md](sync-api-contract.md) | `mobile/sync.rs` + `submit_queue.rs` + 手机 `_outbox` |
| 工具分端 ToolScope（四值语义 / MOBILE 那 7 个 / R11） | [tool-scope-contract.md](tool-scope-contract.md) | `tools/tools.rs` 的 `catalog` + `mobile_tools.rs` |
| run-kit 工件（两条端点 / 字段真源 / `kitVersion` bump 规则 / R13） | [run-kit-contract.md](run-kit-contract.md) | `ai/run_kit.rs` + 手机 harness 的取/缓存/吊销 |

**同步范围**（唯一真源，触发器 / `bootstrap` / `changes` 三处都从它派生）：

| 同步 | 不同步 |
| --- | --- |
| `prompts` `tags` `prompt_tags` `media_assets` `media_tags` | **`ai_run_events`**（单 Run 上万条的调试日志） |
| `ai_conversations` `ai_messages` `ai_message_parts` `ai_attachments` | `ai_providers` `ai_models` `app_settings`（含凭据引用与令牌明文） |
| `capture_runs` `ai_runs` `ai_tool_calls`（2026-09-27 用户拍板：进） | `ai_questions`（提问闸门只在桌面；手机看实时提问走 `GET /api/ai/pending`） |

---

## 4. 怎么算做完了（验收判据）

```powershell
$env:PUB_HOSTED_URL='https://pub.dev'; flutter analyze; flutter test     # 408 例
cargo test --manifest-path server-rs\Cargo.toml                          # lib 630 + 集成 150
cargo test --test spot_check -- --ignored                                # 真机活体抽查（要 ComfyUI 在跑）
pwsh -File scripts\e2e-ai-tools-test.ps1                                 # 工具循环 + Skills + 附件（111 项）
pwsh -File scripts\e2e-submit-test.ps1                                   # 提交链路 6 幕
pwsh -File scripts\e2e-capture-test.ps1                                  # 捕获链路
```

- **纯逻辑改动跑单测就够**；改了 `HarnessRunner` 的上下文拼装 → **三条 e2e 全跑**（AGENTS §10.1）。
- **e2e 纪律**：别在跑完前 Ctrl+C（清理在 finally 里；中断会留下 `e2e-submit-*` 记录，
  把下一次捕获 e2e 的 SHA-256 判重弄乱）；`e2e-submit-test.ps1` 会自己把权限档临时钉成 `ask` 再还原。
- **活体验收**（真 ComfyUI）用 `spot_check`：`live_投放的目录就是实例在读的那一份` 那条的判据是
  "工具投完，实例自己列得出来"。

---

## 5. 回归防线归属（每条红线由谁盯着）

| 红线 | 桌面（Rust） | 手机 |
| --- | --- | --- |
| R1 / R2 | `infra/db.rs` 的连接工厂 + `open_*` 用例 | `MobilePragmas` 连接工厂 + `HarnessDatabaseTest` |
| R3 / R4 | `repo` 的 upsert 用例 + `_sync_log` 触发器 | 手机侧**不建账本**（契约 §3.1），无此风险 |
| R5 | `events` 环形缓冲 + `prune_events` + `resume_from` | `RunEventBusTest` + `HarnessStoreTest` |
| R6 | 执行器三条收尾路都 `sink_now` + `middle-cut-stream` | `HarnessRunnerTest` 三例 + `HarnessStoreTest` 两例 |
| R7 | `system_prompt` / `nsfw_prompt` 的注入防线用例 | 横幅只做 `{kind}`/`{query}` 替换（`MobileToolsTest`） |
| R8 | `ai_executor` 两例（顺序 + 拒绝不执行） | `MobileAskGate` + `MobileToolsTest` 两例 |
| R9 | `tests/attachments_test.rs` 4 例 + e2e 第 9 幕 | `InlineBudgetTest` + 适配器未实现模态直接抛 |
| R10 | `tests/submit_test.rs` 6 例 + `e2e-submit-test.ps1` | 不适用（手机只排队，R11） |
| R11 | — | `MobileToolsTest` 断言不回 `mediaIds`、不说"已提交" |
| R12 | `tools/tests.rs`（清单真源） | 不维护自己的规则清单（`declaredTo` = kit ∩ 本地实现） |
| R13 | run-kit 下发全部提示串 | `R13NoPromptLiteralsTest` 扫生产源码 |
| R14 | — | `sqlite-bundled` 依赖 + FTS5 启动探针 |

---

## 6. 风险与待定项

| 风险 | 缓解 |
| --- | --- |
| 两套 harness 漂移（安全规则两端不一致） | R13（规则下发）+ 事件契约一致性用例 + 同语言可逐行对照 |
| platform channel 的边界（切前后台、事件丢、大 payload） | 事件流 `Flow`→`EventChannel`；大文件绕开通道；"切后台再回来事件不丢"用例 |
| 安卓前台服务 6h/24h 配额 | 前台服务 + Run 状态落本地库，重启后按 R6 收尾 |
| 桌面离线期间的模型目录 | run-kit 版本化 + 吊销（🔒 §3） |
| 手机发起的 Run 在桌面执行时的权限放大 | 判据是"发起端 ≠ 执行端"：`mutating` / ASK 类工具**一律强制走桌面审批**（哪怕桌面设了自动允许）；只读工具照旧 |

**待定项**：只剩一条 —— `ai_run_events` 的历史数据要不要归档（P2 当年是"丢弃 + 先做 R5 的保留策略"）。

---

## 7. 文档地图

**实施入口 = 本文**；"怎么做才不踩坑" = [AGENTS.md](../AGENTS.md)；"怎么用" = [README.md](../README.md)；
排错 = [pitfalls.md](pitfalls.md)。

| 文档 | 看它学什么 |
| --- | --- |
| 🔒 [sqlite-schema-contract.md](sqlite-schema-contract.md) | 四个 PRAGMA / 类型映射 / 迁移（含**加列三处一起改**）/ R1~R4 的具体形态 |
| 🔒 [run-event-contract.md](run-event-contract.md) | RunEvent 信封、17 个类型、SSE 帧格式、保留窗口边界 |
| 🔒 [sync-api-contract.md](sync-api-contract.md) | 同步 4 条端点、游标语义、410 分支、幂等与 LWW |
| 🔒 [tool-scope-contract.md](tool-scope-contract.md) | ToolScope 四值语义、MOBILE 那 7 个、R11 的来由 |
| 🔒 [run-kit-contract.md](run-kit-contract.md) | 工件字段真源、`kitVersion` bump 规则、R13 |
| [ai-tools-and-skills.md](ai-tools-and-skills.md) | AI 工具与 Skills 的接线（含与 DSH 的对照） |
| [permission-audit.md](permission-audit.md) | 写工作区外的 12 处，逐条标注退路 / 闸门 |
| [workflow-recipe-design.md](workflow-recipe-design.md) | 工作流配方：骨架 / 模型槽位 / 参数档位三层分解 |
| [pairing-design.md](pairing-design.md) | 配对闸门与密钥下行（§11 是实现的入口） |
| [mobile-harness-design.md](mobile-harness-design.md) | 手机 harness 的范围与边界、为什么 Kotlin 而非 Dart/Rust |
| [cross-device-sync-design.md](cross-device-sync-design.md) | 同步机制细节（变更账本、游标、墓碑、bootstrap、SQLite 坑） |
| [android-agp9-builtin-kotlin-migration.md](android-agp9-builtin-kotlin-migration.md) | **动 `android/` 之前必读**（AGP 9 内建 Kotlin，不许引 KMP） |
| [comfyui-capture.md](comfyui-capture.md) | 捕获扩展怎么装、三条捕获入口的分工 |

**历史与论证**（只在需要推导细节时查，不作为实施依据）：
`memory-and-storage-evaluation.md`（为什么换 SQLite、MySQL 方言可移植性）、
`embedded-db-sync-options-report.md`（六个同步方案为什么都不用）、
`audit-client-architecture-android-readiness.md`（客户端端点全清单）、
`ai-home-*.md` / `docs/bug and suggestion/`（早期需求与现场记录）。
