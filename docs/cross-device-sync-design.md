# 跨端数据传输设计（桌面 Rust + SQLite → 安卓镜像）

> ⚠ **2026-09-23 更新**：本文原按"手机只读"设计。用户随后选定 **B 方案（手机也要跑 harness）**，
> 于是手机成为**写者**，本文的游标拉取只是**下行**通道，**上行**通道与手机 harness
> 见 **`docs/mobile-harness-design.md`**。本文的下行设计（§3~§7）仍然有效。

> 前提（2026-09-23 用户决策）：**桌面 = Rust + SQLite**（内存寸土寸金）；
> **手机端不需要本地跑 AI，可靠性优先，内存不敏感**。
> 问题：**用 SQLite 的话，怎么实现数据跨端传输？**
>
> 本文回答这一个问题。上游文档：`docs/android-client-plan.md`（单写者架构与分步落地）、
> `docs/inbound-auth-design.md`（配对令牌与媒体签名 URL）、
> `docs/memory-and-storage-evaluation.md`（为什么换 SQLite）、
> `docs/embedded-db-sync-options-report.md`（为什么不用现成同步引擎）。

---

## 1. 一句话：SQLite 不参与跨端传输

**MySQL → SQLite 换掉的是"数据库进程"，不是"服务端进程"。**

```
今天：  Flutter ──HTTP──▶ Ktor(后端) ──JDBC──▶ mysqld ──▶ 数据文件
换后：  Flutter ──HTTP──▶ axum(后端) ──rusqlite──▶ SQLite 文件
                 ↑
            跨端传输还是这一层，一个字都没变
```

手机**永远不碰 `.db` 文件**。它讲的是 REST/SSE，和今天完全一样。
所以"SQLite 是本地文件、没法跨端"这个担心不成立 ——
**跨端传输从来不是靠数据库做的，是靠你的 HTTP 服务做的**，而那个服务还在。

反过来说，SQLite 在**"记录什么变了"这件事上比 MySQL 更好用**（见 §4）：
MySQL 的 binlog 是给复制用的、格式是内部的、应用读不了；
SQLite 有触发器 + `user_version` + `VACUUM INTO` + 官方 session 扩展，全是给应用用的。

---

## 2. 整体形状

```
┌──────────────────── 桌面（唯一写者）────────────────────┐
│  axum (Rust)                                            │
│    ├── 业务接口 /api/prompts /api/media /api/ai/...      │  ← 与今天逐字一致
│    ├── 同步接口 /api/sync/{manifest,changes,bootstrap}   │  ← 新增，唯一的新面
│    └── tokio-rusqlite ──▶ comfyhub.db (WAL)             │
│                              ├── 业务表                   │
│                              └── _sync_log（变更账本）    │
│  storage/media/  ← 二进制不进库，走 HTTP + ETag           │
└─────────────────────────┬───────────────────────────────┘
                          │ 局域网 HTTP + X-ComfyHub-Token
                          │ （远程走 Tailscale，见 §9）
┌─────────────────────────▼───────────────────────────────┐
│  安卓（只读镜像）                                          │
│    drift / sqflite  ──▶ 本地镜像库（只读，不含 _sync_log） │
│    游标持久化在 sync_state(cursor)                        │
└─────────────────────────────────────────────────────────┘
```

**三条不可动摇的纪律**（来自 `android-client-plan.md` §5，换栈后依然成立）：

1. **桌面是唯一真源**。手机不直接写库，写操作一律 `POST /api/...` 交给桌面。
2. **手机不装 Python / MCP / 不自起服务**。
   （调研补充的两条硬约束：Android 10+ **禁止 `execve()` 应用私有目录里的文件**，
   所以"内嵌服务端"只能是在进程内的 `.so`，不能是子进程；
   且前台服务类型被限制 **6 小时/24 小时**，做不了常驻。
   → 结论：手机侧就是纯客户端，别想内嵌服务端。）
3. **Rust 只在桌面。手机端一行 Rust 都不编译**（2026-09-23 用户追问后确认，理由见下）。

### 2.1 为什么手机端不用 Rust（免得以后再讨论一遍）

用户 2026-09-23 直接问："安卓倒是不需要 Rust 重写吧？" —— **对，不需要，而且不该。**
完整论证见 `docs/memory-and-storage-evaluation.md` §7.5，这里只留结论：

- **安卓把"内嵌服务端"这条路堵死了**：Android 10+ 禁止对应用私有目录里的文件
  `execve()`（所以只能是进程内的 `.so`，不能是子进程），
  且前台服务类型被限 **6h/24h**（做不了常驻）。
  即便编成 `.so` 塞进 APK，它也只在 App 活着时于进程内运行 ——
  **既不是真服务端，也解决不了"桌面不在线"**（那个问题用 Tailscale 解，见 §9）。
- **手机端的活不需要它**：`http` 取增量 + `drift` 应用增量，
  其中"应用一批 upsert/delete + 维护游标"约 100 行 Dart。
- **是净增复杂度**：NDK + 4 个 ABI 交叉编译、`flutter_rust_bridge` 的默认构建后端
  **`cargokit` 已于 2026-03-26 归档**、四个包必须**严格同版本**、
  FFI 边界的 Rust panic 可能直接终止 App 进程（与"可靠优先"相反）、APK 变大。
- **"共享 Rust 核心"这个理由不成立**：真正值得共享的只有
  **`schema.sql` 那段 SQL 文本**和 **`/api/sync/*` 的 JSON 契约**，两者都是语言中立的；
  而两端对库做的事本质不同（桌面是权威库含迁移/触发器/级联，手机是只读投影），
  硬共用一份代码就得抽象"我现在是哪一侧"，写出来比两份专用实现更长。

### 2.2 手机端技术栈

> ⚠ **2026-09-23 改判**：用户提议"手机端用 Kotlin 方便人来管理"，
> **成立 —— 手机 harness 改用 Android Kotlin**（Flutter UI 通过 platform channel 调用），
> 优于原先的 Dart 方案。完整论证见 **`docs/mobile-harness-design.md` §2**。
> 下面这张表里 **UI 层的 Dart 依赖不变**，**逻辑层（harness）改为 Kotlin**。

**UI 层（仍是 Flutter/Dart，不变）：**

| 用途 | 包 | 备注 |
| --- | --- | --- |
| HTTP / SSE（连桌面） | `http`（**项目已有**） | SSE 是用 `http.Request` 手工分帧，能带 header |
| 本地缓存目录 | `path_provider` | 目前**缺**，要补 |
| 图片缓存 | `cached_network_image` 之类 | 配合 §7 的 `ETag = sha256` |
| 视频 | 官方 `video_player`（安卓）+ 现有 `video_player_win`（Windows） | ⚠ 见 `memory-and-storage-evaluation.md` §6.5：`video_player_win` **只声明 windows 平台**；**别因为要加 `video_player` 就把它换回 pub.dev**（AGENTS §9 的补丁） |

**逻辑层（改为 Android Kotlin，见 `mobile-harness-design.md`）：**

| 用途 | 选型 | 备注 |
| --- | --- | --- |
| harness（LLM 调用 / 工具循环 / 事件流） | Android Kotlin（`android/app/src/main/kotlin/`） | **不引入 KMP**（会与 AGP 9 内建 Kotlin 冲突，详见该文档 §2.1） |
| 本地库 | Room / SQLDelight + **bundled SQLite 驱动** | ⚠ **不能用安卓系统 SQLite**（Android 13 = 3.32a、无 FTS5） |
| 凭据 | Android Keystore（`EncryptedSharedPreferences`） | `CredentialService.kt:190` 的 DPAPI 实现**不可复用** |
| 与 Flutter 的桥 | `MethodChannel` + `EventChannel` | 新增成本；Dart 侧需一个 `RunEventSource` 抽象以保住 UI 代码共享 |

**drift 的两个默认值必须显式改掉**（调研核实）：
**drift 不会替你开 WAL**，且 `readPool` 默认为 0 ——
必须传 `setup:` 执行 `PRAGMA journal_mode = WAL`，否则多 isolate 访问直接
"database is locked"。

**手机端不需要**：`_sync_log`、触发器、外键级联、`app_settings`、`ai_providers`、`ai_models`、
`ai_run_events`。它是投影，不是副本（§4.5 / §5）。

### 2.3 手机端要不要有"后端"？—— 不需要，而且**做不到**（2026-09-23 用户追问）

用户问："那 Kotlin 后端有必要在手机存在吗？只靠 Flutter 处理真的可靠吗？"
答案分三段，第一段是决定性的。

#### (1) Kotlin 后端**结构上无法**在安卓运行 —— 不是"不必"，是"不能"

后端里有三处**硬绑定 Windows / AWT / 子进程**的代码，它们不是疏忽，
**它们就是"桌面能力宿主"这个定位本身**：

| 阻断点 | 证据 | 为什么安卓不行 |
| --- | --- | --- |
| **凭据库** | `ai/CredentialService.kt:190` `if (!isWindows()) error("DPAPI 仅在 Windows 上可用")`；实现是 `powershell.exe -EncodedCommand` 调 `System.Security.Cryptography.ProtectedData`（`:155-214`） | 安卓没有 DPAPI，也没有 `powershell.exe`。**这一层是"直接抛异常"，不是降级** |
| **缩略图 / 解码** | `MediaFiles.kt:4-14` `java.awt.*` / `javax.imageio.ImageIO`，`:32` 强制 headless | **安卓没有 AWT / ImageIO** |
| **子进程** | `ai/mcp/McpStdioClient.kt:227` `ProcessBuilder(command)`；`MediaFiles.kt:299` | 安卓禁止对应用私有目录里的文件 `execve()`（Android 10+），且 MCP 需要 Python |

**推论**：把这三块掏掉之后剩下的，是 **AI harness + 协议适配 + 工具注册表 + CRUD Repo**。
而按 §4.5 与 `android-client-plan.md` §5，**这些正是手机不该有的部分**
（AI 跑在桌面、凭据留在桌面、ComfyUI 在桌面）。

**所以手机需要的不是"后端的一个子集"，而是另一件小得多的事。**

#### (2) 数据层：**Flutter-only 反而更可靠**（这是反直觉的那一条）

> ⚠ **2026-09-23 修正**：这一条**只在"手机侧用 Dart 且用 `package:sqlite3`"时成立**。
> 若手机 harness 改用 Android Kotlin（用户提议，已采纳，见 `mobile-harness-design.md` §2），
> 只要用 **bundled SQLite 驱动**（`androidx.sqlite:sqlite-bundled` / SQLDelight bundled），
> 同样能钉住现代 SQLite。**所以这条不构成"必须用 Dart"的理由** ——
> 它真正的价值是提醒：**无论用哪门语言，都不要用安卓系统自带的 SQLite。**

如果真在安卓上放一个 Kotlin/JVM 后端，它会落在**安卓系统自带的 SQLite** 上 ——
而 AOSP 的版本非常旧，且**编译期就关掉了扩展加载**（`SQLITE_OMIT_LOAD_EXTENSION`）、
**没有 FTS5**（调研核实：Android 13 仍是 **3.32a**）。

而 Dart + **`package:sqlite3` v3 自带 SQLite 3.53.4** ——
**两端用的是同一个引擎版本，与操作系统无关**，并且已经过了 WAL-reset 缺陷的修复线（3.51.3）。

> 也就是说：**"有后端"在这里不是可靠性的来源，反而是可靠性的风险** ——
> 它会把你的数据层绑到一台设备的 OS 版本上。
> 你要的中文全文检索（FTS5）在安卓系统 SQLite 上**根本不存在**。

#### (3) 逻辑层：可靠性的来源是"权威在哪 + 有没有回归防线"，不是"有没有后端"

守住一条纪律就不会漂：

> **手机只做「展示」，不做「校验」。**
> 任何业务规则以桌面为准；手机**不重新实现**验证、不自己算派生字段。
> 要算就让后端算好放进 DTO。

这样手机端新增的**真正逻辑**只有一件事：
**应用一批增量 + 推进游标**（§5，约 100 行），而且它可以写成**纯函数**
（`applyDeltas(localState, response) -> newState`），用 `flutter test` 直接覆盖。
项目已有 23,408 行 Dart / 196 个用例，客户端不是玩具 ——
`_orderedBody`、草稿回声这类竞态都已经有回归用例钉住（`docs/pitfalls.md`）。

加上 §5 的事务 + 游标设计，崩溃恢复是免费的：应用一半崩了，游标没推进，重放幂等。

#### (4) 但必须承认三个真实的可靠性限制（都与"有没有后端"无关，是安卓平台给的）

| 限制 | 后果 | 对策 |
| --- | --- | --- |
| **不能后台常驻** | 前台服务被限 **6h/24h** → **手机在后台不会自动同步**，打开 App 才同步 | 接受它；要周期同步只能用 **WorkManager**（最小间隔 15 分钟） |
| **长任务会被杀** | 一次性下载大量媒体可能被系统中断 | 前台服务 + 断点续传；媒体本来就是按需取（§7） |
| **手机绝不能持有 Provider API Key** | 一旦在手机跑 AI Run，就必须把密钥放手机 —— **破坏桌面的 DPAPI 边界** | Run 一律在**桌面**执行，手机只消费 SSE |

#### (5) 唯一会改变结论的决策：手机要不要跑 AI 对话？

| 形态 | 手机端需要什么 | 可靠吗 |
| --- | --- | --- |
| 只浏览（画廊 / 提示词 / 历史） | drift + 增量应用 | ✅ 明确正确 |
| AI 对话，但 **Run 在桌面执行**、手机只看 SSE | 多一个 SSE 消费 + 审批/提问回传 | ✅ 可靠（手机是纯消费者） |
| AI 对话，且 **Run 在手机执行** | harness + 协议适配 + 工具循环 + 审批闸门 + **RSK-004 注入防线**（≈300KB Kotlin 的等价物） | ⚠️ **这时才真的有风险** —— 但解法**仍然不是"把 Kotlin 搬到手机"**，而是要么别在手机跑 Run，要么用 Dart 重写并**照搬那套回归防线** |

⚠ 注意 `android-client-plan.md:70-72` **留了这个口子**（"已经设计好的提示词类能力…
用户可以在移动端直接跑"）。**这是需要拍板的一个点**：
按用户 2026-09-23 的取向（手机内存不重要、可靠优先、不跑 AI），建议**选第二种形态**。

### 2.4 ★ 手机发起 Run（"在手机出方案 → comfy_submit 提交"）

> 用户 2026-09-23 补充需求：**"手机 Run 还是有一部分需求的，比如可以在手机出设计方案，
> 最后再调用 comfy_submit 提交。"** 这一节说明它落在哪一档、以及缺什么。

#### (1) 关键区分：**手机「发起」Run ≠ 手机「执行」Run**

用户描述的是**工作流发生在手机上**，不是**harness 跑在手机上**。这条链路：

```
手机：和 AI 讨论、出设计方案、挑提示词、配参数
  ↓  （Run 由手机发起）
桌面：harness 跑 LLM 循环 → AI 决定调 comfy_submit
  ↓
桌面：comfy_submit 提交给本机 ComfyUI → 等生成 → capture 入库画廊
  ↓  （mediaIds 随事件流回手机）
手机：看到"已提交"(经审批) → 生成完收到画廊入口卡
```

**为什么 Run 必须执行在桌面 —— 因为 `comfy_submit` 物理上只能在桌面跑：**

| `comfy_submit` 依赖什么 | 在哪 |
| --- | --- |
| ComfyUI 本体（`127.0.0.1:8188`） | 桌面 |
| 工作流的 **API 节点图**（`capture_runs.raw`，`ComfyCapture.apiGraphOf`） | 桌面库 |
| 工作流文件库与 `comfy_list_workflows` / `run_key = file:<sha256>` | 桌面磁盘 |
| `capture.captureRun()` 产物入库（画廊 / NSFW 标记 / `promptId` 绑定） | 桌面 |
| `MediaFiles` 缩略图（`java.awt`/`ImageIO`，§2.3 已证安卓没有） | 桌面 |

**并且：你的场景以 `comfy_submit` 收尾，就意味着桌面此刻必然在线**（ComfyUI 在那儿）。
所以"把 Run 放在桌面执行"**在这个场景里不增加任何前提**。

→ **结论：这条需求完全落在 §2.3 的"第二种形态"里。手机零 harness、零 Rust、零 Kotlin。**

#### (2) 但现有设计有 **5 个必须补的缺口**（这才是真正要干的活）

| # | 缺口 | 证据 | 为什么对手机是致命的 |
| --- | --- | --- | --- |
| **①** | **审批 / 提问闸门是纯内存的** | `ai/tools/AskGate.kt:105` `ConcurrentHashMap<String, CompletableFuture>`；`:103` 超时 **30 分钟** | 后端一重启闸门就没了；**另一台设备也看不到"有个待批准"**。手机发起 Run、走到电脑前想接手批准 → 做不到 |
| **②** | **30 分钟超时对手机太短** | 同上 `AskGate.kt:103` | 手机在后台**不会推送**。用户 2 小时后才打开 App，那次 Run 已经超时失败了 |
| **③** | **Run 的流要能"断线重连"** | 机制**已有**：`RunEventBus.subscribe(runId, after)` + `AiRunRepo.events(runId, after)` 回放；但 `pruneEvents` **是死代码**、`RunEventBus` 把整场事件留内存（`memory-and-storage-evaluation.md` §4.2） | 手机 App 被系统杀掉是**常态**（§2.3 第 4 条）。重连靠 `?after=<seq>`，所以**事件保留窗口必须够长** |
| **④** | **Run 归属与跨端可见性没实现** | `android-client-plan.md` §4 第 4 步已写（"pendingQuestion 是前端本地状态，跨端要落库 + 事件广播"）但未落地 | 手机发起的 Run，用户走到电脑前要看得到、接得上、批得了 |
| **⑤** | `ToolScope`（按端过滤工具）的语义要重想 | `android-client-plan.md` §3 的工具分端表 | Run 在**桌面**执行时，工具**全都可达**。此时过滤不再是"可达性"，而是"要不要允许手机发起的 Run 做桌面写操作"——这是**策略问题**，不是能力问题 |

**顺带三条不用改的**：手机上传参考图走 `/api/ai/attachments`（桌面 harness 会内联进 LLM 请求，
注意 `InlineBudget` 单图 8MB / 合计 20MB）；SSE 用 `http.Request` 手工分帧**能带 header**
（`inbound-auth-design.md` C3）；媒体回显走 §7 的签名 URL。

#### (3) 唯一的分叉：**设计阶段要不要能"桌面不在线"？**

这决定架构，必须你拍板：

| | **A. 设计时桌面必须在线**（推荐） | **B. 设计时桌面可离线** |
| --- | --- | --- |
| Run 在哪执行 | 桌面（手机是 UI + SSE 消费者） | 手机（**mini-harness**） |
| 手机新增逻辑 | ≈0（只加 SSE 消费 + 审批回传） | Dart 重写精简 harness：LLM 调用 + 工具循环 + 审批 + **RSK-004 注入防线** |
| 手机要持有 Provider API Key？ | **否**（留在桌面 DPAPI） | **是** → **破坏桌面的凭据边界**（`CredentialService.kt` 见 §2.3） |
| `comfy_submit` | 原生可用 | **不可用**（桌面离线）→ 见下 |
| 工具集 | 全部 | 只剩 `web_search` / `civitai_search` / `load_skill` / `remember`（**这是个小集合，Dart 实现可行**） |
| 代价 | 离线时手机 AI 不可用 | 两套 harness 会漂移；密钥边界破了 |

**⚠ B 方案有一个必须正面处理的正确性问题：**
`comfy_submit` 的**契约**是"提交 → 等生成 → 返回 `mediaIds`"（`ToolModel.kt:277-286`）。
桌面离线时它**做不到**，但模型会**以为自己提交成功了**并如实向用户汇报 —— 这是最坏的一种失败
（用户以为图在跑了，其实没有）。所以 B 方案**不能**让那个 Run 以 `comfy_submit` 收尾，只能：

- 让设计阶段产出一份**待提交的工件**（工作流路径 + overrides），
  由手机在桌面重新上线后**交给桌面跑一次新的 Run / 直接提交**；或
- 引入一个**语义诚实的新工具**（例如 `comfy_queue_submit`），
  它明确返回"已排队，桌面在线后执行"——**绝不复用 `comfy_submit` 的名字**。

**建议选 A。** 理由：你的场景以 `comfy_submit` 收尾，桌面本来就必须在线；
A 的代价（离线时手机不能设计）恰好是"离线时也提交不了"的同一段时间，**没有净损失**。

> ### ⚠ 2026-09-23 更新：**用户已选 B**
>
> 用户明确"**还是需要手机 harness 的**"。所以：
> **A 案作废，以 B 案为准** —— 手机侧要实现精简 harness（Dart），
> 完整设计见 **`docs/mobile-harness-design.md`**。
>
> 该文档同时修正了本节两点：
> ① "手机端只增加约 100 行"的前提**不再成立**（变成一两千行，含安全相关逻辑），
>    因此 Android 要不要 Rust 这笔账在新文档 §2 重算（结论：仍先用 Dart，但设了触发条件）；
> ② 手机从"只读镜像"变成**写者**，本节的单向游标拉取只是**下行**通道，
>    **上行通道（outbox）是新文档 §7 的内容**。
>
> 本节 (1) (2) 两段的分析**依然有效**（`comfy_submit` 只能在桌面跑、5 个缺口），
> 新文档直接沿用并对缺口 ①②④ 给出了落地阶段（M1）。

---

## 3. 三个同步原语

只有一个新接口面，三个端点：

| 端点 | 作用 | 备注 |
| --- | --- | --- |
| `GET /api/sync/manifest` | 握手：`{ schemaVersion, cursor, serverTime, tables: {...} }` | 手机用它决定"能不能续传"还是"要全量重建" |
| `GET /api/sync/changes?since=<cursor>&limit=500` | **增量拉取**（主力） | 返回 `{ cursor, hasMore, upserts: {表: [行…]}, deletes: {表: [主键…]} }` |
| `GET /api/sync/bootstrap?page=N` | **首次全量**（分页） | 手机第一次连接、或 schema 版本不匹配时用 |

### 3.1 为什么 `cursor` 必须是"单调序列"，不能是时间戳

时间戳做游标有三个经典死法：同一毫秒多行会漏、多行落在同一 `updated_at`、
时钟回拨/夏令时。**用一个只增不减的整数序列**：

```sql
CREATE TABLE _sync_log (
  seq  INTEGER PRIMARY KEY AUTOINCREMENT,   -- ★ AUTOINCREMENT 是必须的，见 §6.2
  tbl  TEXT    NOT NULL,
  pk   TEXT    NOT NULL,        -- 主键的文本形式（prompts.id 是整数，会话 id 是 UUID）
  op   TEXT    NOT NULL CHECK (op IN ('insert','update','delete')),
  at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX idx_sync_log_tbl ON _sync_log(tbl, seq);

-- 每个客户端的进度（用于日志裁剪，见 §4.4）
CREATE TABLE sync_clients (
  device_id TEXT PRIMARY KEY,
  cursor    INTEGER NOT NULL DEFAULT 0,
  last_seen TEXT
);
```

### 3.2 返回形状（`upserts` / `deletes` 分开）

```json
{
  "cursor": 8412,
  "hasMore": true,
  "upserts": { "prompts": [ {…}, {…} ], "tags": [ … ], "media_assets": [ … ] },
  "deletes": { "prompts": [12, 19], "media_assets": ["uuid-…"] }
}
```

- **`upserts` 里永远给"当前完整行"**，不是"变化的那一列"。
  行级整体覆盖让手机端的应用逻辑退化成 `INSERT OR REPLACE` 一条，
  而且天然幂等（同一批变更应用两遍结果一样）。
- **`deletes` 必须独立成组**，因为手机拿不到"这一行不存在"和"这一行被删了"的区别。
  这就是**墓碑（tombstone）**。

---

## 4. 变更账本怎么在 SQLite 里做

### 4.1 三种做法

| 做法 | 机制 | 取舍 |
| --- | --- | --- |
| **A. 触发器 + `_sync_log`** ⭐ | 每张需同步的表挂 3 个 AFTER 触发器 | **推荐**。可读、可用 `sqlite3` 直接查、易测、与连接无关、不需要任何扩展 |
| B. `sqlite3_update_hook` | `rusqlite` 的 `hooks` feature，C 回调 | 自动、零 DDL；但**只对注册它的那条连接生效**（多连接会漏），且拿不到 DELETE 前的行 |
| C. session 扩展（changeset） | `rusqlite` 的 `session` feature，`sqlite3session_changeset()` | 官方给的正是"同步"场景，带冲突回调；但产出的是二进制 changeset，调试不直观。**留给"将来要双向写"时用** |

**v1 用 A。** 每张表三条：

```sql
CREATE TRIGGER trg_sync_prompts_ai AFTER INSERT ON prompts BEGIN
  INSERT INTO _sync_log(tbl, pk, op) VALUES ('prompts', NEW.id, 'insert');
END;
CREATE TRIGGER trg_sync_prompts_au AFTER UPDATE ON prompts BEGIN
  INSERT INTO _sync_log(tbl, pk, op) VALUES ('prompts', NEW.id, 'update');
END;
CREATE TRIGGER trg_sync_prompts_ad AFTER DELETE ON prompts BEGIN
  INSERT INTO _sync_log(tbl, pk, op) VALUES ('prompts', OLD.id, 'delete');
END;
```

（只有 6~8 张表真的要同步，见 §4.5 —— 触发器是 18~24 条，一次写完。）

### 4.2 增量查询 + "同主键合并"

一个窗口里同一个主键可能出现多次（先 insert 再 update）。
**先按 `(tbl, pk)` 取窗口内最后一次操作**，再回表取当前行：

```sql
-- 1) 扫窗口
SELECT seq, tbl, pk, op FROM _sync_log
 WHERE seq > :since ORDER BY seq LIMIT :limit;

-- 2) 同一主键只保留最新那条（在应用层做，比 SQL 好读）
-- 3) op != 'delete' 的按 (tbl, pk) 回表取完整行
SELECT * FROM prompts WHERE id = :pk;
```

**游标语义**：`cursor` = 本次**扫描到的最大 seq**（不是"返回给客户端的最小 seq"）。
漏掉没变化的行没代价，重复推送一行也没代价（幂等），**唯独不能跳号**。

### 4.3 首次全量（bootstrap）

不要在 `/bootstrap` 里直接发 `.db` 文件。理由：**那会把服务端的内部表一起发出去**
——`_sync_log`、`sync_clients`，以及 `app_settings` 里的**配对令牌明文**
（`inbound-auth-design.md` D3 明确说令牌明文存 `app_settings`）。**这是凭据泄漏。**

正确做法：`/bootstrap` 就是"从 0 开始读，但读的是业务表而不是日志"，
按 `(表, 主键)` 排序分页吐行，最后一次响应带上 `cursor = 当前 max(seq)`。

> 数据集只有 ~24MB（`docs/memory-and-storage-evaluation.md` §4.1），
> gzip 后几 MB，分页 5~10 次就完了。**"数据小"是这里最大的红利**：
> 任何不方便的边界情况都可以用"丢掉缓存重新全量"兜底，成本极低。
> 真到了几 GB 再考虑 `VACUUM INTO` 出快照 + 剔除敏感表。

### 4.4 日志裁剪（必须做，否则 `_sync_log` 自己长成第二个 `ai_run_events`）

```sql
-- 只保留"最慢的那个客户端还没读到"的部分
DELETE FROM _sync_log WHERE seq <= (SELECT MIN(cursor) FROM sync_clients);
```

**并且要处理"游标已经落在窗口之外"**：手机离线三个月回来，
它的 `cursor` 比 `MIN(seq)` 还小 —— 这时返回 **`410 Gone`**，
手机收到就丢弃本地镜像、走 `/bootstrap` 重建。
**这个分支必须实现**，否则表现是"手机少了几百条数据，而且永远补不回来"。

### 4.5 哪些表要同步

| 表 | 同步？ | 为什么 |
| --- | --- | --- |
| `prompts` / `tags` / `prompt_tags` | ✅ | 提示词库，手机要看要搜 |
| `media_assets` / `media_tags` | ✅ | 画廊（**元数据进库，字节走 HTTP**，见 §7） |
| `ai_conversations` / `ai_messages` / `ai_message_parts` | ✅ | 聊天历史，手机离线看的主力 |
| `ai_attachments` | ✅ | 附件元数据（字节同样走 HTTP） |
| `capture_runs` | ✅ **（2026-09-27 定：进）** | 是"生成发生过"的账本；原来标"可选"，现在明确进（手机看画廊时能显示"这次生成"） |
| **`ai_run_events`** | ❌ | 单 Run 上万条的流式事件日志，是**调试/断线续传**用的，同步它等于把最大的表搬过去 |
| `ai_runs` / `ai_tool_calls` | ✅ **（2026-09-27 定：进，总纲 §7 待定项 1）** | 手机要能显示桌面那次 Run 的**工具过程**（跑过哪些工具、成没成、花了多久）。⚠ 上行也收（`ai_run` / `ai_tool_call`），而手机侧 `ai_runs` 因此变**镜像表** —— 崩溃恢复要靠本机 `local_runs` 标记，别用 `status='running'` |
| `ai_questions` | ❌ | 提问闸门只在桌面侧；手机看实时提问走 `GET /api/ai/pending` |
| `ai_providers` / `ai_models` / `app_settings` | ❌ | **含凭据引用与端点配置**，手机不该有；`app_settings` 里还有配对令牌明文 |
| `skills`（磁盘） | ❌ | 在 `storage/ai/skills`，不是库表 |

**这张表就是"同步面"的定义**，建议写成一个显式的表清单常量（一个真源），
触发器生成、`/bootstrap`、`/changes` 三处都从它派生 —— 与
`ToolRegistry.kt` 的"工具清单只有一个真源"是同一条纪律。

---

## 5. 手机端怎么应用变更

```
1. 读本地 sync_state.cursor
2. GET /api/sync/changes?since=<cursor>
3. BEGIN IMMEDIATE
4.   deletes: DELETE FROM <表> WHERE pk = ?
5.   upserts: INSERT INTO <表>(…) VALUES(…) ON CONFLICT(pk) DO UPDATE SET …
6.   更新 sync_state.cursor = 响应.cursor
7. COMMIT
8. hasMore ? 回到 2 : 结束
```

要点：

- **整批一个事务**：应用一半就崩，游标也没推进，下次重放同一批 —— 幂等所以安全。
- **`ON CONFLICT(pk) DO UPDATE`，不要用 `INSERT OR REPLACE`**（理由见 §6.3）。
- **`BEGIN IMMEDIATE`**，不要裸 `BEGIN`（理由见 §6.4）。
- 手机端**不需要 `_sync_log`、不需要触发器**：它是只读镜像，只维护一个游标。
  这直接把手机侧的复杂度砍掉一半。

---

## 6. SQLite 特有的坑（每一条都会静默地咬人）

### 6.1 ★ `PRAGMA foreign_keys` 默认是 **OFF**，而且**按连接生效**

你的 schema 严重依赖外键语义（`docs/database-study-notes.md` §2.2：
删提示词 `SET NULL` 保产物、删标签 `CASCADE` 清关联、删会话级联清消息→块→Run→事件）。
**MySQL 里这些是数据库强制的；SQLite 里你不开这个 PRAGMA 就等于没写。**

```rust
// 每一条连接都必须执行（不是一次性的全局设置）
conn.pragma_update(None, "foreign_keys", "ON")?;
```

漏了的症状：删了标签，关联行还在；删了会话，消息还在 —— **而且不报错**。
`Migrate.kt` 那套"靠外键保证一致性"的假设会整体失效。

### 6.2 ★ `_sync_log.seq` 必须 `AUTOINCREMENT`

SQLite 的 `INTEGER PRIMARY KEY` 在删到最大行之后**会复用 rowid**。
`_sync_log` 一边写一边被裁剪（§4.4），于是**游标可能被发一个"已经用过的号"**，
客户端以为"到头了"，中间那批变更永久丢失。
加了 `AUTOINCREMENT` 才会用 `sqlite_sequence` 保证单调不复用。

### 6.3 `INSERT OR REPLACE` 会触发 DELETE 触发器

`INSERT OR REPLACE` 在冲突时的内部实现是**先 DELETE 再 INSERT**，
于是 DELETE 触发器会被点亮，往 `_sync_log` 里写一条**假的 `delete`** ——
手机收到就去删那一行，而桌面其实有这一行。**这是最隐蔽的一个坑。**

→ 在**所有被同步的表**上，upsert 一律写成
`INSERT … ON CONFLICT(pk) DO UPDATE SET …`（它只触发 UPDATE 触发器）。

### 6.4 写事务用 `BEGIN IMMEDIATE`

`busy_timeout` **不覆盖"从读锁升级到写锁"这一步**。
一个延迟事务（裸 `BEGIN`）先读后写时，可能直接拿到 `SQLITE_BUSY` 而**不等待**。
同步接口的写事务"总是要写"，一律 `BEGIN IMMEDIATE` 从一开始就拿写锁。

### 6.5 WAL 与连接分工

```sql
PRAGMA journal_mode = WAL;      -- 持久化在文件头，设一次
PRAGMA synchronous  = NORMAL;   -- WAL 下推荐
PRAGMA busy_timeout = 5000;     -- 按连接，必须每次都设
PRAGMA foreign_keys = ON;       -- 按连接，必须每次都设
```

Rust 侧最省心的分工：**`tokio-rusqlite::Connection` 单连接专管写**
（它内部就是一个专属线程，天然把写串行化 → 彻底不会 `SQLITE_BUSY`），
读用一小池只读连接（WAL 允许读写并发）。
**注意 WAL 不能放在网络文件系统上**（SQLite 官方文档专门有一节讲这个）。

### 6.6 schema 版本用 `PRAGMA user_version`

SQLite 自带一个属于应用自己的整数，**不用建表**：

```sql
PRAGMA user_version = 3;   -- 迁移时递增
```

`/api/sync/manifest` 把这个值吐给手机。
**不匹配 → 手机丢弃镜像、走 `/bootstrap`**。
这正好替代了 MySQL 那边 `Migrate.kt` 靠 `information_schema` 探测的复杂逻辑
（SQLite 侧改用 `PRAGMA table_info` 探测列、`user_version` 记版本，
`db/migrate.sql` 那份存储过程版可以直接废弃 → **三份 DDL 变两份**）。

### 6.7 不要用 Syncthing / rsync / 网盘同步 `.db` 文件

SQLite 官方 "Why Can't I Just Use Ordinary rsync?" 一节的原话是
**"the copy might not be consistent… The database copy might be corrupt"**。
必须用 §3 的接口传数据，或者用 `VACUUM INTO` 出快照（在线、一致性有保证）。

### 6.8 备份

`VACUUM INTO 'backup-YYYYMMDD.db'` —— 在线、原子、一条语句。
比 `mysqldump` + 单独 rsync 图片那套简单一个数量级，而且**一个文件就是全部元数据**。

---

## 7. 媒体字节怎么办（不进数据库）

**继续用你已经有的设计**：库只存元数据，字节在 `storage/media/`，
手机通过 `GET /api/media/{id}/thumb|file` 取 —— 这部分**一个字都不用改**。

而且 SQLite 这边顺手白拿一个优化：`media_assets.sha256` **本来就有**
（`docs/database-study-notes.md` §6 的内容去重键），直接当 ETag：

```
ETag: "<sha256>"
Cache-Control: private, max-age=31536000, immutable
```

内容寻址 + 不可变 → 手机端（`cached_network_image` 之类）**一次下载永不重下**，
而且不需要任何同步逻辑。缩略图因为会被重新生成，URL 上带一个生成器版本号。

**鉴权**：媒体 URL 是 `Image.network` / 播放器直接取的，**带不上请求头** ——
所以走 `inbound-auth-design.md` 的 D5（短期 HMAC 签名查询串 `?t=&e=`），
并在返回 `mediaIds` 的 DTO 里直接拼好签名 URL。

---

## 8. 一次性搬迁：MySQL → SQLite

数据只有 24MB，写一个独立小工具即可（**Rust 或复用现有 Kotlin/JDBC 都行，
别让它进主服务**）。逐项映射：

| MySQL | SQLite |
| --- | --- |
| `BIGINT UNSIGNED AUTO_INCREMENT` | `INTEGER PRIMARY KEY AUTOINCREMENT` |
| `TINYINT(1)` 布尔 | `INTEGER` + `CHECK(x IN (0,1))` |
| `ENUM('IMAGE','VIDEO',…)` | `TEXT` + `CHECK(kind IN (…))` |
| `MEDIUMTEXT` / `JSON` | `TEXT` |
| `DATETIME(3)` + `CURRENT_TIMESTAMP(3)` | `TEXT` ISO-8601（**统一 UTC**） |
| `ON UPDATE CURRENT_TIMESTAMP(3)` | **触发器**（SQLite 没有这个语法） |
| `ON DUPLICATE KEY UPDATE v=VALUES(v)` | `ON CONFLICT(k) DO UPDATE SET v=excluded.v` |
| `INSERT IGNORE` | `INSERT OR IGNORE` |
| `information_schema` 探测 | `PRAGMA table_info` / `PRAGMA index_list` / `sqlite_master` |
| `TIMESTAMPDIFF(SECOND, created_at, NOW(3))` | `CAST((julianday('now')-julianday(created_at))*86400 AS INTEGER)` |
| `GROUP_CONCAT` | `group_concat`（3.44+ 支持内部 `ORDER BY`） |

**时间字段最容易出事**：MySQL 的 `DATETIME` 是"字面时间"、无时区
（`database-study-notes.md` §2.2 第 7 条），
搬过去必须**明确按 UTC 存 ISO-8601**，否则跨端一比就是 8 小时差。

**校验**：搬完对每张表比 `COUNT(*)`，再抽样比若干行的字段级哈希。
`ai_run_events` 建议**直接不搬**（62.1 万行的调试日志，且 §4.5 说它不同步）——
顺便把 §4.2 的 `pruneEvents` 那个 bug 一起了结。

---

## 9. 传输与鉴权（复用已设计好的，不新造）

| 场景 | 做法 |
| --- | --- |
| **局域网**（主力） | 后端绑 `0.0.0.0` + `X-ComfyHub-Token`（`inbound-auth-design.md` 全套：回环豁免、非回环默认拒绝、`?token=` 兼容、常数时间比较）。**这是"开远程的前置条件"，必须先做** |
| 发现 | mDNS `_comfyhub._tcp` 广播地址端口；或**二维码配对**（`baseUrl` + 一次性 token 编进二维码给手机扫），省掉手输 IP + 抄 token |
| **不在同一局域网** | **优先 Tailscale / WireGuard：零代码、零端口暴露**（`docs/remote-access-security.md` 已有立场）。**不要**把后端端口直接映射到公网 |
| SSE | 继续用（手机端的流式对话）。`inbound-auth-design.md` C3 已确认客户端是 `http.Request` 手工分帧、**能带 header** |
| 媒体 | §7 的签名 URL |
| 日志 | `CallLogging` 必须把 `token` / `t` 查询参数打码（`inbound-auth-design.md` P1 的前置项），否则日志里全是凭据 |

---

## 10. Rust 侧技术栈（具体到 crate）

```toml
axum            = "0.8"          # HTTP，tokio 生态主流
tokio           = { version = "1", features = ["full"] }
tokio-rusqlite  = "0.6"          # SQLite 跑在专属线程上 → 写天然串行化，不会 SQLITE_BUSY
rusqlite        = { version = "0.32", features = ["bundled"] }   # ★ bundled 自带 SQLite 3.53.x
serde / serde_json
tower-http      = { features = ["cors", "compression-gzip", "fs"] }  # ServeFile 原生支持 Range，替代 Ktor PartialContent
image           = "0.25"         # 缩略图，替代 ImageIO + TwelveMonkeys(WebP)
hmac / sha2 / base64             # 媒体签名 URL、配对令牌比较、sha256 去重
```

**选 `bundled` 是一个战略性的决定**：SQLite 被编译进二进制，
于是发布包里**再没有 MySQL 目录（304MB）、没有 JRE、没有 VC++ 运行时依赖**
（`packaging/manifest.json` 的 `runtimeRequirements` 里那两条可以整条删掉）。
这和 `docs/memory-and-storage-evaluation.md` §5.2.4 的结论是同一件事。

**迁移**：用 `PRAGMA user_version` 手写（贴合项目现有"不引 Flyway、自己写 `Migrate.kt`"的取向），
或 `rusqlite_migration`。前者更能保持"三份 DDL 同源"的纪律。

**接口兼容**：Rust 版**必须逐一实现今天 Ktor 的那套 `/api/...`**，
Flutter 端一行不改。可以按端点分组灰度（先换 `/api/tags`、`/api/prompts`，用反向代理分流），
`ai/tools`（6,759 行）最后搬。

---

## 11. 如果将来要"手机离线也能写"

**现在不要做。** 但提前知道最小正确实现长什么样：

- 手机端加一个 **outbox**（本地表 `_outbox(op, tbl, pk, payload, created_at)`），
  离线时只写 outbox，联网后按顺序 `POST` 给桌面；
- **冲突策略只能是"服务端赢"**：单用户不会同时用两台设备改同一条数据，
  所以不需要 CRDT、不需要向量时钟、不需要三向合并；
- **时间戳不可信**：不要用手机时间做 LWW 比较，
  由**服务端在 apply 时打时间戳**（手机时钟可以随便改）；
- 桌面的 `_sync_log` 已经给了下游推送能力，双向时**再引入 SQLite 的 session 扩展
  （changeset）**，让官方扩展处理冲突回调 —— 这就是 §4.1 里 C 那条路的用途。

---

## 12. 分阶段落地

| 阶段 | 内容 | 前置 | 手机侧改动 |
| --- | --- | --- | --- |
| **S0** | 入站鉴权（`inbound-auth-design.md` 的 P0） | 无 | 无 |
| **S1** | MySQL → SQLite（**保留 Kotlin/Ktor**） | S0 无关，可并行 | **零** |
| **S2** | 加 `_sync_log` + 触发器 + `/api/sync/*`；日志裁剪与 410 分支 | S1 | 零（新接口，纯增量） |
| **S3** | 按端点把服务端迁到 Rust（接口逐字不变） | S1 | **零** |
| **S4** | 安卓只读镜像（drift）+ 游标拉取 + 媒体缓存 | S2 | 全部 |
| **S5**（可选） | 离线写（outbox + 服务端赢） | S4 | 增量 |

**关键顺序**：**S1 与 S3 拆开**。先换数据库（改动集中在 17 个文件的数据层、可逐表测），
再换语言（接口不变、可灰度）。**不要合并成一次"大爆炸"重写** ——
这一条同时回答了 `docs/memory-and-storage-evaluation.md` §7.4。

**S2 可以完全独立于 S1/S3**：`_sync_log` + 三个端点在 MySQL 上也能先做
（触发器和游标序列 MySQL 都有），这样安卓端不必等 Rust 重写完。

---

## 13. 需要你决策的点

1. **同步范围**：§4.5 那张表认不认？特别是 `capture_runs` / `ai_runs` / `ai_tool_calls`
   要不要同步（同步 = 手机能看到"这次生成用了什么工具"，代价是三个表进镜像）。
   ✅ **2026-09-27 用户已拍板：都进**（总纲 §7 待定项 1 = "进"，落地见 §4.5 表与 🔒 `sqlite-schema-contract.md` §6）。
2. **`ai_run_events` 确认不同步**？（62.1 万行，且是本该被裁剪的调试日志。）
3. **手机端要不要"离线读"**？不要的话 S2/S4 都不用做，
   手机就是纯瘦客户端（`android-client-plan.md` 的原始设计），
   那本文只有 §9 的传输与鉴权部分需要落地。
4. **S1 与 S3 是否同意拆开**？（先换库、再换语言。）
5. **搬迁时 `ai_run_events` 直接丢弃**，还是要留在 MySQL 里归档？
