# 手机端 harness 设计（B 方案：手机能独立出方案，桌面可离线）

> 决策（2026-09-23 用户选定）：**选 B** —— "还是需要手机 harness 的"。
> 即：**手机能独立和 AI 出设计方案，此时桌面可以不在线**；设计完再提交给桌面的 ComfyUI。
>
> 上游：`docs/cross-device-sync-design.md` §2.4（B 与 A 的分叉与代价）、
> `docs/embedded-db-sync-options-report.md`（安卓平台约束，均已核实）、
> `docs/ai-tools-and-skills.md`（工具与 Skills 的现存约定）。
>
> ⚠ 本文推翻/修正了两处早先的结论，§1 明确列出。

---

## 0. 一句话

**手机跑 harness，但"harness 的脑子"（系统提示、工具 schema、Skills、记忆、安全文本）
仍然由桌面产出、版本化下发、手机缓存；手机只实现"管道"（HTTP + SSE + 工具循环 + 本地存储）。
提交永远由桌面执行 —— 手机产出的是「待提交工件」，不是提交动作。**

---

## 1. ⚠ 两处自我修正

### 1.1 "手机端不需要 Rust / 约 100 行" 的前提**不再成立**

`cross-device-sync-design.md` §2.4(3) 里，我推荐 A 方案的核心论据之一是
"手机端新增逻辑只有约 100 行（应用增量 + 推进游标）"。
**选 B 之后这个数字变成了一两千行，其中一部分是安全相关逻辑。**

所以"Android 要不要 Rust"这笔账**必须重算**（§2）。

### 1.2 手机从"只读镜像"变成**写者** —— 同步从单向变双向

`cross-device-sync-design.md` 整篇是按"桌面唯一写者、手机只读投影"设计的
（那是它的 §2 第 1 条纪律）。**选 B 之后，手机要本地产生会话/消息/块，并且要传回桌面**，
所以：

- §4 的"游标增量拉取"（桌面 → 手机）**仍然有效**，是下行通道；
- 但必须**新增一条上行通道**（手机 → 桌面）——见 §7。这是本文最重要的新增部分。

---

## 2. ★ 手机 harness 用什么写：**Kotlin**（2026-09-23 用户提议后改判）

用户提议"**手机端用 Kotlin 方便人来管理**"。**这个论点成立，而且比我原先的 Dart 建议更好**
—— 我改判。理由逐条：

| 收益 | 为什么对**这个**项目成立 |
| --- | --- |
| **一套逻辑语言** | 维护者只读/只写 Kotlin。桌面 harness 已经是 Kotlin，手机 harness 用同一门语言，**没有第二套生态要学** |
| **可以直接移植而不是重推** | `protocol/Adapters.kt`、`ConversationTitle.kt`、系统提示拼装、事件类型 —— 这些**算法可以逐行对照地搬**。跨语言重写（翻译）与同语言移植（抄+改）的出错率差一个量级 |
| **Android 上是原生一等公民** | kotlinx-coroutines `Flow` 天然映射 Run 事件流； kotlinx-serialization **与桌面同源**；Ktor-client / OkHttp 与桌面同族。**不需要 NDK、不需要 FFI、不需要 codegen、不需要交叉编译 4 个 ABI** |
| **调试链路最短** | Android Studio 原生断点调试。对比 Rust：FFI 边界的 panic 可能**直接终止 App 进程**，归因困难 |

**Rust 依然否决**（理由不变，已核实）：`cargokit` **2026-03-26 归档**（而它是 `flutter_rust_bridge`
的默认构建后端）、四个包必须严格同版本、NDK + 4 ABI 交叉编译、panic 会 abort 进程。
**Kotlin 把这些成本全部绕开了，而且拿到的是同一个"语言统一"的收益。**

### 2.1 ⚠ 但"用 Kotlin"有两个含义，必须分清

| | (a) **Android Kotlin** ✅ **现在就做** | (b) **Kotlin Multiplatform `commonMain`** ⛔ **现在不要做** |
| --- | --- | --- |
| 形态 | 代码放 `android/app/src/main/kotlin/`，或一个 `com.android.library` 模块；**Flutter UI 通过 platform channel 调用** | 抽一个 KMP 模块，`commonMain` 同时被桌面 JVM 与 Android 依赖 |
| 与桌面的共享 | **不共享代码**（共享的是 §3 的「工件」= 下发的数据） | 共享 harness 纯逻辑 |
| 今天能加吗 | **能，零摩擦**（AGP 9 内建 Kotlin 直接编译） | **不能，会和本工程的 Android 构建刻意设计冲突** |

**为什么 (b) 现在不能做 —— 这是查过本工程的 `android/` 才敢下的结论：**

本工程刚刚完成 **AGP 9 内建 Kotlin 迁移**（`docs/android-agp9-builtin-kotlin-migration.md`），
当前状态每一处都和 KMP 相冲：

| 现状（`android/` 里的原文） | 与 KMP 的冲突 |
| --- | --- |
| `android.builtInKotlin=true` | AGP **自己**编译 Kotlin，**任何模块都不 apply KGP** |
| `id("org.jetbrains.kotlin.android") version "2.4.0" **apply false**`（只为满足 Flutter 的 KGP ≥ 2.2.20 检查，见 flutter/flutter#192167） | KMP 必须 apply `org.jetbrains.kotlin.multiplatform` —— 正是"不 apply KGP、不抢 `kotlin` 扩展名"这个刻意设计要避开的事 |
| `kotlin.incremental=false`（C:/D: 跨盘会炸） | 共享模块会把这个跨盘问题扩散到更多 module |
| `android.newDsl=false`（Flutter 插件仍未迁完新 DSL） | KMP + 旧 DSL + Flutter 插件三方组合没有成熟路径 |

> `AGENTS.md` §4 对这块的要求是**"动 `android/` 之前先读
> `docs/android-agp9-builtin-kotlin-migration.md`"** —— 上面那张表就是读它的结论。
>
> **所以：手机 harness 用 Android Kotlin 写（(a)），但先不要引入 KMP。**
> 但要**为将来的 KMP 留门**：把 harness 里的**纯逻辑**（协议适配、标题摘取、
> 提示拼装、事件类型、工具循环控制流）写成**不依赖任何 Android API 的普通 Kotlin 类**，
> 只让 I/O（HTTP、SQLite、Keystore）走接口注入。
> 这样将来若要抽 `commonMain`，是"搬文件"而不是"重写"。

### 2.2 代价（诚实清单）

| 代价 | 说明 | 对策 |
| --- | --- | --- |
| **Flutter ↔ Kotlin 的 platform channel** | 这是新增的主要成本：`MethodChannel`（调用）+ `EventChannel`（Run 事件流）。要处理**事件流**、**大 payload**（附件/图片过通道会拷贝）、**前后台生命周期**、**错误传播** | 事件流做成 `Flow` → `EventChannel`；大文件不走通道，走文件路径/HTTP |
| **Dart 侧要有 `RunEventSource` 抽象** | 否则"远端 SSE"与"本地 channel"两套消费者会把 UI 代码撕成两份 | 一个 Dart 接口两个实现；**UI 代码保持共享**（§5 的价值由此保住） |
| **仍是两份 harness**（除非将来 KMP） | 桌面 Kotlin + Android Kotlin | 同语言 → 漂移**可逐行对照**；再加 §3 的"工件下发"纪律，规则永远只有一份 |
| **SQLite 必须用 bundled 驱动** | ⚠ **不能用安卓系统 SQLite**（Android 13 仍是 **3.32a**、`SQLITE_OMIT_LOAD_EXTENSION`、**无 FTS5**）。要 `androidx.sqlite:sqlite-bundled` 或 SQLDelight 的 bundled driver | 这也**抵消了我上一轮"Flutter 在数据层更可靠"的论据** —— Kotlin 侧只要用 bundled 驱动，同样能钉住现代 SQLite |
| 若 P3（桌面 → Rust）发生 | 手机的 Kotlin harness **不受影响**（手机不需要 Rust），但"共享逻辑"的故事到此为止 | 可接受：手机侧本来就不该跟着桌面换语言 |

> 关键判据不变：**重复的是"管道"可以接受，重复的是"规则"不可接受。**
> §3 的设计让规则（系统提示 / 工具 schema / 安全文案）**永远只有一份 —— 下发的数据**，
> 与实现语言无关。这是 (a) 方案能成立的前提。

---

## 3. ★ 核心设计：共享「工件」，不共享「代码」

手机 harness 的行为由**四份输入**决定。这四份**全部由桌面产出、版本化下发、手机缓存**：

| 工件 | 桌面侧的现有真源 | 手机怎么拿 |
| --- | --- | --- |
| **系统提示词**（含工具清单、NSFW 开关、不可信数据禁令、Skills 摘要、长期记忆） | `SystemPrompt.render(version)`；版本常量 `SystemPrompt.VERSION`（改提示必须 bump，AGENTS §10） | `GET /api/ai/run-kit` 一次性取回 |
| **工具 schema**（名称 / 描述 / 参数 JSON Schema） | **唯一真源** `ToolRegistry.kt` 的 `tools` 列表；`GET /api/ai/tools` 已经是它的派生出口 | 同上（按 `ToolScope` 过滤成手机可执行子集） |
| **模型能力目录**（模态、工具、思考方言、上下文窗口） | `AiSeedCatalog` 的 classpath 冻结副本 + `ModelCapabilityCatalog` | 同上（可只带用户实际用的若干模型） |
| **Skills + 长期记忆** | `<storage>\ai\skills` 与 `memory.md`（磁盘即真源，刻意不缓存） | 走同步通道（当普通可同步数据） |

**再加一份文本工件：** `untrustedBanner()` 那段"不可信数据"横幅文案（RSK-004 的防线）。
它是**文本**，由桌面下发 —— 手机**不要自己写一份**。

于是得到一条硬纪律：

> **手机 harness 不许硬编码系统提示、工具描述、安全文案。
> 它们只能来自 run-kit；run-kit 的版本号记进每一次 Run（`promptVersion`），
> 事后可追溯"这次回答用的哪版规则"。**

这条纪律让"两套 harness 漂移"的风险从**代码层**降到了**数据层**，
而数据层的漂移是**可见、可比对、可一次性修正**的（版本号 + diff）。

### 3.1 run-kit 的形状

```json
{
  "kitVersion": "2026-09-23T12:00:00Z#v28",
  "schemaVersion": 3,
  "systemPrompt": "…（完整渲染后的正文，含不可信数据禁令）…",
  "promptVersion": "v28",
  "nsfwEnabled": true,
  "tools": [
    { "name": "web_search", "description": "…", "inputSchema": { … }, "scope": "MOBILE" },
    { "name": "civitai_search", "…": "…" }
  ],
  "untrustedBanner": "…（原样下发）…",
  "models": [ { "providerId": "…", "modelId": "…", "inputModalities": ["text","image"], "tools": true, "thinkingFormat": "deepseek" } ],
  "inlineBudget": { "perImageBytes": 8388608, "totalBytes": 20971520 }
}
```

**吊销**：手机离线时用的可能是旧 kit。联网后 `GET /api/ai/run-kit/status` 返回
`{ current, revoked: [...] }`；命中被吊销的版本 → 手机**提示"规则已更新，请联网同步后再继续"**，
而不是拿旧规则继续跑。（安全文本被修订时，这条是唯一的补救途径。）

---

## 4. 手机 harness 的确切范围

### 4.1 要什么

| 模块 | 对应桌面实现 | 规模估计 |
| --- | --- | --- |
| LLM 上游调用 + SSE 流式解析 | `ai/AiUpstream.kt`(38KB) | 中（**只做 2~3 种方言**，见 4.2） |
| 思考/推理方言解析 | `ai/protocol/Reasoning.kt` | 小 |
| 工具循环 + 审批 + 结果截断 | `HarnessRunner.kt`(88KB) 的子集 | 中 |
| **RunEvent 事件流**（见 §5） | `RunEventBus.kt` 的事件类型定义 | 小，但**必须逐字一致** |
| 本地持久化（会话/消息/块/Run/事件） | `AiRepo`/`AiConversationRepo`/`AiRunRepo` | 中（**Room / SQLDelight + bundled SQLite**，见 §2.2） |
| 标题摘取 | `ConversationTitle.kt`（三条纪律：不吞、不重、回声也要摘） | 小，**可直接移植**（同语言） |
| 失败/取消时"已说的话一个字不丢" | `PartSink.flushInflight()` 那条纪律（AGENTS §10.1） | 小但**必须做** |
| 附件内联 | `InlineBudget` + 模态/传输准入 | 中 |
| 移动端工具的 Kotlin 实现 | `KeenableSearch`(21KB) + `CivitaiSearch`(31KB) | 中，**可直接移植**（同语言） |
| Flutter ↔ Kotlin 桥 | —— **新增，桌面侧没有对应物** | 中（`MethodChannel` + `EventChannel`，见 §2.2） |
| 凭据 | 桌面 `CredentialService`（DPAPI）→ 手机用 **Android Keystore**（§8） | 小 |

### 4.2 不要什么（这些留在桌面，**不要抄到手机**）

- 全部 `comfy_*` 工具、`comfy-mcp` 的 39 个工具
- 文件工具（`read_file`/`write_file`/`list_dir`）—— 手机没有"项目根"这个概念
- `register_skill` / `delete_skill`（要写磁盘；手机侧最多是"只读加载"）
- `WorkflowAudit` / `WorkflowFit` / `WorkflowConvert` / `FrontendNodes`（≈130KB）
  —— 工作流校验是**提交前**的事，由桌面的 `comfy_submit` 负责
- `ModelCapabilityCatalog` 的**发现/探测**逻辑（`CapabilityRefresh`）—— 手机只用下发的目录
- 多方言全谱：**只做用户实际在用的那 2~3 种**（openai-completions / anthropic-messages / 某个思考方言）

**规模估计：约 1,500~2,500 行 Kotlin**（不含 Room/SQLDelight 生成的代码），
其中**相当一部分可以从桌面 Kotlin 直接移植**（协议适配、标题摘取、工具实现、
提示拼装），而不是重写 —— 这正是 §2 选 Kotlin 的主要理由。
这个数字是本文所有判断的依据，落地时**第一个动作就是给它设个上限**：
显著超出就说明在重实现规则（而不是移植），该停下来检查 §3 的工件下发是不是没做到位。

---

## 5. 最大的省事：**复用 RunEvent 协议 → 聊天 UI 代码两端共享**

`RunEventBus.kt` 已经定义了一整套事件类型（`run.started` / `text.delta` / `reasoning.delta` /
`tool.requested` / `tool.started` / `tool.completed` / `user.question` / `usage.updated` /
`message.completed` / `run.completed` / `run.failed` / `run.cancelled` / `heartbeat`）。

**只要手机 harness 往本地事件表里写的是同一套事件，Flutter 的聊天界面代码就能两端共用**
（`lib/pages/ai_home_page.dart` + 相关 store 消费的就是这个协议）。

> 这是 B 方案里**最值钱的一条**：手机端新增的 UI 代码趋近于 0，
> 因为它渲染的是同一套事件流，只是事件的生产者从"桌面的 SSE"变成了"本地的 harness"。

**要求**：事件类型、`seq` 单调性、`payload` 字段名逐字一致。
建议把事件协议单独抽成一个**契约文件**（例如 `docs/run-event-contract.md` + 两份常量），
并在两端各加一条"事件类型清单一致"的用例。

---

## 6. 提交：**手机永不直连 ComfyUI，也永不复用 `comfy_submit` 这个名字**

### 6.1 正确性问题（选 B 必须正面处理的那条）

`comfy_submit` 的契约是**"提交 → 等生成 → 返回 `mediaIds`"**（`ai/tools/ToolModel.kt:277-286`）。
桌面离线时它**做不到**，而模型会**以为自己提交成功了并如实向用户汇报** ——
用户以为图在跑了，其实没有。**这是最坏的一种失败。**

### 6.2 做法：队列化 + 语义诚实的新工具

```
手机 harness 的设计阶段
  └─ AI 认为方案已定 → 调用 mobile 侧工具 `comfy_queue_submit`（★ 不叫 comfy_submit）
       └─ 语义：把「提交意图」写进本地 outbox，返回
          "已排队；桌面在线后会自动提交，提交结果会回传到这个会话"
       └─ 绝不返回 mediaIds、绝不说"正在生成"
  ↓（同步，§7）
桌面 outbox 消费
  └─ 用**现有的 comfy_submit 全链路**执行：审批闸门 → ComfySubmitter → capture.captureRun() 入库
       └─ 桌面侧该有的不变式一条不少：审批、run_key 幂等、产物必须入库
  ↓（下行通道）
手机收到 tool_result / mediaIds → 画廊入口卡
```

**要点：**

- **桌面的 `comfy_submit` 一行不改**，所有既有纪律（`AGENTS.md` §10.1：
  `comfy_submit` 只认 API 图、必须走 `capture.captureRun()`、结果只认 `mediaIds`）原样保有效。
- 手机侧的新工具**必须有不同的名字和不同的返回值**，让模型无法混淆。
  系统提示里要写清"排队 ≠ 已提交"。
- 排队项在桌面上仍然**要过审批闸门** —— 手机上的"提交"不是批准，
  除非你把手机上的确认也设计成审批（那是另一个决策，见 §11）。
- 排队项需要一个**可见的状态**（`queued` / `submitted` / `done` / `failed`），
  并且状态变化要能让手机看到（复用 §5 的事件协议）。

---

## 7. 上行通道：手机变成写者之后

这是 `cross-device-sync-design.md` 没有覆盖的部分（它假设手机只读）。

### 7.1 什么要上行

| 数据 | 为什么 |
| --- | --- |
| 手机产生的**会话 / 消息 / 块** | 用户要在桌面继续这段对话、也要在桌面看到历史 |
| 手机产生的 **Run / 事件** | 桌面要能显示"这次 Run 用了什么工具"、也要能审计 |
| **待提交工件**（§6.2 的 outbox） | 桌面要执行提交 |
| 手机侧的工具结果（`web_search` 等） | 已经在消息块里，随消息走 |

### 7.2 关键：**主键必须是 UUID，不能是自增整数**

好消息：`ai_conversations` / `ai_messages` / `ai_message_parts` 的主键**本来就是 `CHAR(36)` UUID**
（见 `Migrate.kt`），所以**两端各自生成的 ID 不会撞**。
⚠ 但 `prompts` / `media_assets` / `tags` 用的是 `BIGINT AUTO_INCREMENT` ——
**手机不要创建这几类实体**（手机不建提示词库/画廊记录；那些由桌面在提交时产生）。

### 7.3 outbox 的形状

```sql
CREATE TABLE _outbox (
  id        TEXT PRIMARY KEY,          -- UUID
  kind      TEXT NOT NULL,             -- 'conversation' | 'message' | 'part' | 'run' | 'submit_job'
  ref_id    TEXT NOT NULL,             -- 该实体的 UUID
  payload   TEXT NOT NULL,             -- JSON
  created_at TEXT NOT NULL,
  tries     INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
```

- 每条本地写操作**在同一个事务里**同时写业务表和 outbox（保证不漏）。
- 联网后按 `created_at` 顺序 POST 给桌面；**幂等**（桌面用 UUID `INSERT OR IGNORE` / `ON CONFLICT DO NOTHING`）。
- 成功后删 outbox 行。失败退避重试，`tries` 超限标红给用户看（**不许静默丢弃**）。

### 7.4 冲突策略：**服务端赢 + 消息是追加的，所以几乎没有冲突**

- 会话的 `title` / `updated_at`：LWW，**桌面赢**（单用户，两台设备改同一个标题极罕见）。
- 消息 / 块：**追加为主**（`seq` 单调），冲突面几乎为零。
- **手机不修改桌面创建的既成事实**（不删会话、不改历史消息）——
  这条纪律能把冲突面压到接近 0，建议写死。
- **时间戳不可信**：手机时钟可随便改，所以 `created_at` 以**服务端 apply 时打的**为准
  （本地那份只用于排序展示）。这条在 `cross-device-sync-design.md` §11 已写，这里复用。

---

## 8. 凭据：桌面 DPAPI → 手机 Android Keystore

**现有设计天然适配**（`AGENTS.md` §10）：`ai_providers.credential_ref` 存的是**名字**，
**值由平台自己的密钥库保管**（桌面是 DPAPI，`CredentialService.kt`）。

于是：

- `ai_providers` 表**可以整体下行同步**（里面**没有密钥**，只有 ref 名字与 base_url）；
- 手机把**自己的那份密钥**存在 **Android Keystore** 里
  （Flutter 侧用 `flutter_secure_storage`，底层是 `EncryptedSharedPreferences` + Android Keystore），
  key 用同一个 `credential_ref`；
- **两端的密钥互相独立** —— 这既符合"值不出本机"，也意味着**用户要在手机上单独填一次 API Key**。
  这是 B 方案不可避免的代价，**要在界面上说清楚**，不要假装能自动同步。

> ⚠ 对比：`CredentialService.kt:190` 的桌面实现是 `if (!isWindows()) error(...)`，
> 且走 `powershell.exe` 调 .NET。**手机侧不可能复用这段代码**，必须另写。

---

## 9. 可靠性：B 方案最薄的一环（用户明说"可靠优先"）

### 9.1 后台被杀

手机 harness 跑一次 Run 可能几分钟到十几分钟（多轮工具调用）。
用户切走 App，Android 可能杀掉进程 → **Run 断在半路**。

对策（三件一起上）：

1. **前台服务 + 常驻通知**（"正在生成方案…"）。注意上限：
   前台服务类型有 **6 小时 / 24 小时** 的配额（Android 15+），
   对单次 Run 足够，但**不能当常驻同步用**。
2. **Run 状态落本地库**：进程被杀后重启，能看出"有一个 Run 没跑完"。
3. **失败/取消时"已经说出去的话一个字都不能丢"** ——
   直接照搬 `AGENTS.md` §10.1 的三条纪律（正文必须落块、标题缓冲要 flush、
   重试不要就地重置有内容的回复）。**这三条是现成的，
   必须一起搬到手机 harness，否则用户会亲眼看到"话说了一半就没了"。**

### 9.2 断网

- 设计阶段（只调 mobile 工具）可以完全离线，**这是 B 的价值所在**；
- 但 LLM 调用本身要出网 —— **没有网络时连 LLM 都调不了**。
  ⚠ **所以 B 的"离线"要精确定义**：指的是**桌面不在线**，不是**手机没网**。
  如果用户想要的是"手机没网也能出方案"，那需要本地模型，**那是完全另一件事**（见 §11）。

### 9.3 与桌面 Run 的会话互相看见

手机发起的会话上行之后，桌面要能**继续**它 —— 但桌面可能**没有那个 provider 配置**
（手机用的是另一个模型）。两个选择：
① 手机只用桌面已配置的 provider（更简单，推荐）；
② 允许手机用任意 provider，桌面遇到不认识的 provider 时**只读显示、不能续写**。

---

## 10. 分阶段落地

| 阶段 | 内容 | 依赖 |
| --- | --- | --- |
| **M0** | 桌面：`GET /api/ai/run-kit` + `/run-kit/status`（版本化下发系统提示/工具 schema/模型目录/安全文案）；`ToolScope` 落地 | 无（纯增量，Kotlin 侧就能做） |
| **M1** | 桌面：`AskGate` 落库 + 跨端可见 + 超时可恢复（`cross-device-sync-design.md` §2.4 缺口 ①②④） | M0 |
| **M2** | **RunEvent 契约文件** + 两端"事件类型清单一致"用例 | M0 |
| **M3** | 手机 harness：LLM 调用 + SSE + 工具循环 + 本地存储 + 三条失败纪律 | M0/M2 |
| **M4** | 上行通道：outbox + 幂等 apply + 服务端赢 LWW | M3 |
| **M5** | `comfy_queue_submit` + 桌面 outbox 消费（走现有 `comfy_submit` 全链路） | M4 |
| **M6** | 凭据：`flutter_secure_storage` + `ai_providers` 下行同步 | M3 |
| **M7** | 前台服务 + 崩溃恢复 | M3 |

**M0/M1/M2 是纯桌面侧、且与语言无关，可以立刻做**（甚至能在 P0 调优、P2 换 SQLite 之前做）。

---

## 11. 需要你决策的点

1. **"离线"的确切含义**：是"**桌面不在线**，手机有网"（B 能做到），
   还是"**手机也没网**"（需要本地模型，等于换一个产品）？
   我按前者设计的 —— **如果是后者，请现在说，整篇要重构。**
2. **手机上要不要"提交审批"**：手机点"提交"时，桌面还要不要再过一道审批闸门？
   （`inbound-auth-design.md` 的令牌是**连接鉴权**，不是**操作审批**，两者别混。）
3. **手机能不能用桌面没配置的 provider**（§9.3 的 ①/②）？
4. **手机侧密钥要不要用户手填**（§8，B 的必然代价）？
5. **是否接受 §4.2 的"只做 2~3 种方言"**（否则 harness 规模会显著超出 §4.2 的估计，
   并触发 §2 的 Rust 触发条件）？
6. **M0~M2 现在就开始吗？**（纯桌面、纯增量、不依赖 P0/P2/P3。）
