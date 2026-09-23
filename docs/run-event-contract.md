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

## 2. 事件类型清单（17 个，唯一真源 = `RunEventBus.kt` 的 `RunEventType`）

| `type` | 常量 | payload 关键字段 | 谁发 |
| --- | --- | --- | --- |
| `run.started` | `RUN_STARTED` | `runId` | 开始一次 Run |
| `message.started` | `MESSAGE_STARTED` | `messageId` | 助手气泡建立（前端据此挂占位） |
| `reasoning.delta` | `REASONING_DELTA` | `text` | 思维链增量（**与正文分流**） |
| `text.delta` | `TEXT_DELTA` | `text` | 正文增量（**逐字**） |
| `tool.requested` | `TOOL_REQUESTED` | `callId`, `name`, `args` | 模型要调工具；**需审批的工具在发它之前必须先 `AskGate.open(callId)`**（红线 R8） |
| `tool.started` | `TOOL_STARTED` | `callId` | 批准后真正开始执行 |
| `tool.completed` | `TOOL_COMPLETED` | `callId`, `result`（内含 `mediaIds`） | 工具成功 |
| `tool.failed` | `TOOL_FAILED` | `callId`, `error` | 工具失败 |
| `user.question` | `USER_QUESTION` | `questionId`, `question`, `options?` | **AI 问用户**（不是审批，见 §3） |
| `user.question.answered` | `USER_QUESTION_ANSWERED` | `questionId`, `answer` | 用户答完了 |
| `usage.updated` | `USAGE_UPDATED` | `promptTokens`, `completionTokens` … | token 用量 |
| `message.completed` | `MESSAGE_COMPLETED` | `messageId` | 一条助手消息收口（**落块时机**） |
| `conversation.updated` | `CONVERSATION_UPDATED` | `conversationId`, `title` | 标题被总结出来了，前端刷新列表 |
| `run.completed` | `RUN_COMPLETED` | `runId` | Run 正常结束 |
| `run.failed` | `RUN_FAILED` | `runId`, `error` | Run 失败（**R6：已说出去的话一个字都不能丢**） |
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

- 事件**同时落库**（`AiRunRepo.appendEvent`）→ 后端重启后仍能回放已完成的事件。
- 内存侧 `RunEventBus` 目前保留**整个 Run 的全部事件**（实测单 Run 4.7 万个事件）——
  P1 要把它改成**环形缓冲**，让 `pruneEvents` 真的被调用。
- **保留窗口必须够长**：手机后台被杀是常态，重连靠 `?after=<seq>`；
  窗口太短会表现成"手机少了一段且永远补不回来"。
- 防线：一条"长 Run + 断线重连，事件仍完整"的用例。

---

## 5. 派生物（本契约的下游）

| 下游 | 依赖哪个字段 |
| --- | --- |
| Flutter `RunEventSource` 抽象（远端 SSE / 本地 channel 两个实现） | 整个信封 |
| R6 三条失败纪律（正文落块 / 标题缓冲 flush / 不许就地重置有内容的回复） | `text.delta` + `message.completed` + `run.failed`/`run.cancelled` |
| 产物入口卡（只认后端给的 `mediaIds`） | `tool.completed.payload.result.mediaIds` |
| 审批卡 / 提问卡 | `tool.requested` / `user.question` |
| 会话列表刷新 | `conversation.updated` |
