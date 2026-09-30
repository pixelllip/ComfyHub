# 🔒 RunEvent 协议（冻结契约，M0）

> **状态：已冻结（2026-09-23）。** 依据 `docs/implementation-master-plan.md` §3.2。
> 改这份契约要**同时改两端**（桌面 Rust/Kotlin + 手机 Kotlin harness + Flutter UI）。
>
> 这份契约是"**UI 代码两端共享、零分叉**"的**全部依据**（`mobile-harness-design.md` §5）：
> 桌面走远端 SSE、手机走本地 platform channel，**但事件流长得一模一样** ——
> 于是 `lib/` 下那套 Run 渲染代码一行都不用为手机重写。

---

## 1. 事件信封

一次 Run 内的每个事件都是 `{ seq, type, payload }`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `seq` | int | **一次 Run 内**从 1 起单调递增，**不跳号、不复用**。断线重连一律 `?after=<seq>` |
| `type` | string | 见 §2 的类型清单，**逐字一致**（大小写、点号都不能改） |
| `payload` | object | 各类型自己的字段；**没有 payload 就发 `{}`**，不发 `null` |

**SSE 帧格式**（`RunEventBus.kt:16` 的 `toSse()`，一个字都别改）：

```
id: <seq>
event: <type>
data: <payload 的 JSON 单行>

```
（`data:` 之后必须有一个空行收尾。）

**"一次 Run 内"是关键限定**：`seq` 的作用域是 Run，不是会话、不是全局。
手机重连时给的 `after` 是**这个 Run 的 seq**；跨 Run 续传不在本契约内。

---

## 2. 事件类型清单（18 个，唯一真源 = `server-rs/src/ai/events.rs` 的 `event_type`）

> 2026-09-28 起桌面后端是 Rust（`server-rs/src/ai/events.rs` 的 `event_type::ALL` 与
> `event_types_match_the_contract` 这条用例逐字钉住本表）；Kotlin 那份只在 `legacy/` 留档。

| `type` | 常量 | payload 关键字段 | 谁发 |
| --- | --- | --- | --- |
| `run.started` | `RUN_STARTED` | `runId` | 开始一次 Run |
| `message.started` | `MESSAGE_STARTED` | `messageId` | 助手气泡建立（前端据此挂占位） |
| `reasoning.delta` | `REASONING_DELTA` | `text` | 思维链增量（**与正文分流**） |
| `text.delta` | `TEXT_DELTA` | `text` | 正文增量（**逐字**） |
| `step.started` | `STEP_STARTED` | `messageId`, **`phase`** | **关键步骤（环节）开始**（用户建议 2026-09-28）：后端从正文里摘掉 `[步骤]环节名[/步骤]` 后，把环节名发出来；`phase` 只可能是那六个固定名字（`server-rs/src/ai/step_mark.rs` 的 `PHASES`）。界面靠它**当场**画出可折叠的环节头，把这一环节的工具卡收进一组。⚠ 落库的有序块类型同名（`type = "step"`，`text` = 环节名）—— 重开会话时界面读的是那个块 |
| `tool.requested` | `TOOL_REQUESTED` | `runId`, `callId`, `name`, **`arguments`**, `approval` | 模型要调工具；**需审批的工具在发它之前必须先 `AskGate.open(callId)`**（红线 R8）。⚠ 键名是 `arguments`（不是 `args`）：桌面 `HarnessRunner.kt:724` 与 Flutter 的 `AiRunEvent.arguments` 都读这个键 —— 发错了工具卡里就看不到参数（2026-09-23 手机侧落地时发现文档这一列写错，已按代码改正） |
| `tool.started` | `TOOL_STARTED` | `callId`, `name` | 批准后真正开始执行 |
| `tool.completed` | `TOOL_COMPLETED` | `callId`, `name`, `elapsedMs`, `preview`, `result`（内含 `mediaIds`） | 工具成功 |
| `tool.failed` | `TOOL_FAILED` | `callId`, `name`, `code`, `message`, `approval`, `elapsedMs` | 工具失败 |
| `user.question` | `USER_QUESTION` | `questionId`, `question`, `options?` | **AI 问用户**（不是审批，见 §3） |
| `user.question.answered` | `USER_QUESTION_ANSWERED` | `questionId`, `answer` | 用户答完了 |
| `usage.updated` | `USAGE_UPDATED` | `inputTokens`, `outputTokens`, `cachedTokens`, `reasoningTokens` | token 用量 |
| `message.completed` | `MESSAGE_COMPLETED` | `messageId`, `text`, `reasoning`, `finishReason`, `elapsedMs`, `reasoningEffort`, `steps`, **`parts`**, `usage` | 一条助手消息收口（**落块时机**）。⚠ `parts` 是**权威**：界面在 parts 非空时只渲染 parts |
| `conversation.updated` | `CONVERSATION_UPDATED` | `runId`, `conversationId`, `title` | 标题被总结出来了，前端刷新列表 |
| `run.completed` | `RUN_COMPLETED` | `runId`, `elapsedMs` | Run 正常结束 |
| `run.failed` | `RUN_FAILED` | `runId`, **`code`**, **`message`** | Run 失败（**R6：已说出去的话一个字都不能丢**）。⚠ 键是 `code` / `message`（不是 `errorCode` / `error`），Flutter 的 `AiRunEvent.code` 就读 `code` |
| `run.cancelled` | `RUN_CANCELLED` | `runId` | 用户取消（**R6 同样适用**） |
| `heartbeat` | `HEARTBEAT` | — | 保活；**不参与业务渲染**，前端必须能忽略未知类型 |

### 防线：清单一致用例

两端各有一份常量，必须有一条第 "**清单逐项相等**" 的用例盯着（M0 交付物之一）：

- 桌面：`RunEventContractTest`（Kotlin，对照本文件 §2 的表）
- 手机：harness 侧同序同值的 `RunEventTypes.kt` + 同一组断言
- Flutter：`RunEventSource` 的实现只做**转发**，不自己造类型

> ⚠ **前端必须忽略未知类型**（`heartbeat` 之外的将来新增类型也是）。
> 契约演进只能**加**类型，不能改名/改语义；改名等于两端同时炸。

---

## 3. `tool.requested` ≠ `user.question`（别合并）

两者在界面上长得像（都是一张卡等用户操作），语义完全不同：

| | `tool.requested` | `user.question` |
| --- | --- | --- |
| 等的是什么 | **批准 / 拒绝**（允许不允许） | **一段文字回答** |
| Run 会怎样 | 被闸门挡住，**超时或取消都算拒绝** | `ask_user_question` 挂在这里等，**Run 不结束** |
| 答完以后 | 批准 → `tool.started` | 回答**作为工具结果喂回模型**，同一个 Run 接着做 |

顺序硬约束（R8）：**需审批的工具，调用方必须先 `AskGate.open(callId)` 再发 `tool.requested`**。
反过来的话用户手快先点按钮会落空，只能等超时。

---

## 4. 持久化与续传（红线 R5）

- 事件**同时落库**（`AiRunRepo.EventBatcher`，P1 起批量写）→ 后端重启后仍能回放已完成的事件。
- **内存侧是有上限的**（P1，2026-09-23）：`RunEventBus` 每个 Run 只留最近
  `MEMORY_KEEP = 2000` 条（环形缓冲），**不再全量留**（实测单 Run 4.7 万个 `text.delta`）。
  ✅ **`seq` 绝不回退、绝不复用** —— 内存里丢最老的只是"回放不到那么老"，
  而 `nextSeq` 一旦重置，客户端游标会永久错位（那才是真丢数据）。`RunEventBusTest` 盯着这条。
- **库里也有保留窗口**：`HarnessRunner.EVENT_KEEP = 4000`，Run 收尾（正常 / 失败 / 取消三条路
  都汇进那个 `finally`）时 `pruneEvents` 真的被调用 —— 在 P1 之前它是**死代码**。
- 两侧都"留尾部"，因为断线重连永远只看最新的一段（`?after=<seq>`）。

**诚实的窗口边界**：被裁掉的只是"**后端重启后**能回放得到的最老那一段"。

> ⚠ **批写 + 只从库里读 = 必须有人 flush**（2026-09-28 修的 bug：用户报"回复的首段思考
> 要等到第一个工具调用才显示"）。事件是**攒够一批才落库**的（`EVENT_BATCH = 200`），
> 而 SSE **只从库里读**（`?after=`）：攒在批里、还没满一批的事件对界面**不可见**。
> 一次 Run 的开头恰好就是这样（`run.started` + `message.started` + 几条 `reasoning.delta`
> 远不到 200 条），而第一条 `tool.requested` 是闸门类事件、那边会 `flush` ——
> 那一下把憋着的思考**一起**放出来，用户看到的就是"思考跟第一个工具卡同时冒出来"。
> 现在 `SseStream::poll_next` 每轮读库**之前先 `flush`**（没攒到东西时空批不开事务），
> 于是**任何**事件都不再依赖"攒满一批"或"恰好撞上某个闸门类事件"。防线：
> `run_lifecycle.rs` 的 `sse_sees_events_without_waiting_for_a_full_batch`（去掉那次 flush 必红）。

| 场景 | 能不能补全 |
| --- | --- |
| 同一进程生命周期内，手机断线重连 | ✅ 完整（订阅者是实时的，内存上限不影响后续事件） |
| 后端重启过、且客户端游标早于保留窗口 | ❌ 只能给窗口内的（`subscribe` 会给窗口起点开始的那些） |
| 长 Run（>2000 条）中手机一直离线，之后回来要全量 | ❌ 同进程内只能给内存窗口内的 → 更长的要靠库（4000）或调大常量 |

**保留窗口必须够长**是需求（手机后台被杀是常态）；两个常量都是"够用就好"的折中，
要更长的窗口就调大它们 —— 但**别调回"全量留"**，那正是 P1 要治的病。
防线：`RunEventBusTest`（4 例，含 R5 的"订阅后一条不落"与"seq 不回退"）。

---

## 5. 派生物（本契约的下游）

| 下游 | 依赖哪个字段 |
| --- | --- |
| Flutter `RunEventSource` 抽象（远端 SSE / 本地 channel 两个实现） | 整个信封 |
| R6 三条失败纪律（正文落块 / 标题缓冲 flush / 不许就地重置有内容的回复） | `text.delta` + `message.completed` + `run.failed`/`run.cancelled` |
| 产物入口卡（只认后端给的 `mediaIds`） | `tool.completed.payload.result.mediaIds` |
| 审批卡 / 提问卡 | `tool.requested` / `user.question` |
| 会话列表刷新 | `conversation.updated` |
