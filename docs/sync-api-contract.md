# 🔒 同步 API 契约（冻结，M0）

> **状态：已冻结（2026-09-23）。** 依据 `docs/implementation-master-plan.md` §3.3、
> `docs/cross-device-sync-design.md`（变更账本与游标推导）、`docs/sqlite-schema-contract.md`（§3 的账本表）。
>
> 这 4 条端点就是**手机与桌面之间的全部协议**。冻结它，桌面 Rust 重写与手机 harness
> 才能各写各的、不互相等。**改这份契约要同时改两端。**

---

## 0. 前置：鉴权与网络

- 全部走局域网 HTTP + `X-ComfyHub-Token` 头（配对令牌，见 `docs/inbound-auth-design.md`）；
  远程走 Tailscale，**不映射公网端口**。
- ⚠ **令牌是"连接鉴权"，不是"操作审批"** —— 两者别混（总纲 §7 待定项 2）。
  手机上点"提交"之后桌面还要不要再过一道审批闸门，是**独立问题**。
- 回环地址豁免（桌面自己调的 `127.0.0.1` 不带令牌），与今天一致。

---

## 1. `GET /api/sync/manifest`

**用途**：手机连上来的第一件事 —— 我该不该走 bootstrap？我的游标还有效吗？

```json
{
  "schemaVersion": 12,
  "kitVersion": "2026-09-23.1",
  "cursor": 918342,
  "serverTime": "2026-09-23T05:30:00Z",
  "tables": {
    "prompts":        { "synced": true,  "rowCount": 27 },
    "media_assets":   { "synced": true,  "rowCount": 20 },
    "ai_run_events":  { "synced": false, "reason": "调试日志，按设计不同步" }
  }
}
```

| 字段 | 说明 |
| --- | --- |
| `schemaVersion` | = 桌面的 `PRAGMA user_version`。**与手机不一致时手机必须先走 bootstrap**（结构对不上，增量一定错） |
| `kitVersion` | = run-kit 版本（🔒 §3.4）。手机据此判断自己手里的系统提示/工具表是不是旧了 |
| `cursor` | = 当前 `MAX(_sync_log.seq)`。手机拿它对比自己的游标决定走增量还是全量 |
| `tables` | 每张表的同步与否 + 行数（**只读诊断**，不含任何用户数据） |

---

## 2. `GET /api/sync/changes?since=<cursor>&limit=500`

**用途**：拿增量。手机断线重连、或每隔一会儿拉一次。

```json
{
  "cursor": 918342,
  "hasMore": false,
  "upserts": {
    "prompts": [ { "id": 12, "text": "…", "updated_at": "2026-09-23T04:00:00.000Z" } ],
    "ai_messages": [ { "id": "9b6d01af-…", "conversation_id": "…", "role": "assistant" } ]
  },
  "deletes": {
    "prompts": [ 9, 15 ],
    "media_tags": [ [4, 7] ]
  }
}
```

硬约束（每一条都对应一条红线或一个已踩过的坑）：

| 约束 | 为什么 |
| --- | --- |
| **游标 = 本次扫描到的最大 `seq`** | 它必须与 `_sync_log` 的单调序号同源。**唯独不能跳号** |
| **`deletes` 必须独立成组** | 手机分不清"我没这行"和"这行被删了"。混在 upserts 里 = 客户端永远留着幽灵行 |
| **重复推送一行无害**（幂等） | 网络重试是常态；`ON CONFLICT DO UPDATE` 保证同一行推两次结果相同（R3） |
| **`hasMore: true` 时必须继续拉** | 分页；手机不能只看第一页就更新游标 |
| **游标落在窗口之外 → `410 Gone`** | ⚠ **这个分支必须实现**，否则表现是"手机少了几百条且**永远补不回来**"。手机收到 410 丢弃镜像、走 `/bootstrap` |
| 每张表**按 `seq` 升序**返回 | 手机按顺序 apply 才能得到与桌面一致的最终状态 |

**主键表示**：单列主键 → 标量（`9`）；复合主键 → 数组（`[4, 7]`，如 `prompt_tags` / `media_tags`）。
`upserts` 里给的是**完整行**（不是 diff），手机直接覆盖本地行。

> 📌 **错误体一律是 `{error, detail?}`**（2026-09-24 与 Kotlin 对齐，P4 第 17 步）：
> `error` 是**人话**、`detail` 可选且**为空时整个键不出现**（Kotlin `explicitNulls = false`）。
> **`410 Gone` 的 `error` 也照此** —— 手机判 410 认的是**状态码**，不要去比对 `error` 的值
> （早先实现里的 `"GONE"` / `"NOT_FOUND"` 这类稳定码已经废弃，见 `server-rs/src/web/routes.rs` 的 `ApiError`）。

---

## 3. `GET /api/sync/bootstrap?page=N`

**用途**：首次全量（手机刚装上 / schemaVersion 变了 / 收到 410）。

- **按业务表分页**，每页带 `{ table, page, pageSize, totalPages, rows[] }`。
- ⚠ **绝不发 `.db` 文件**：`app_settings` 里有**配对令牌明文**，
  发整库等于把令牌送到"可能已经丢了"的手机上。分页只发同步范围内的表
  （见 `sqlite-schema-contract.md` §6 的唯一真源表）。
- 全量走完之后手机把游标置为 `manifest.cursor`（**不是 bootstrap 自己的页号**）。

> ### ⚠ §3 补充：`table` 是**必填**参数（2026-09-24 实现时定，P4 第 13 步）
>
> 原文只写了 `?page=N`，**没说 `table` 怎么给** —— 这是一处留白，实现 Rust 版时必须当场拍板。
> 定下来的结果是 **`GET /api/sync/bootstrap?table=<表>&page=1&pageSize=200`**，
> `table` **必填**，缺了回 `400` 并**列出合法表名**（自解释，免得写手机侧的人去翻源码猜）。
>
> 为什么不能只给 `?page=N`（把十几张表"拍平"成一串页）：那样响应里的 **`totalPages`
> 没有意义**（是全部数据的页数，还是这张表的？），而且"第 N 页是哪张表"要靠客户端自己算 ——
> 可契约偏偏让**服务端**回 `table` 字段。`?table=` 的读法让 `table` / `totalPages` / `rows`
> 三者自洽，客户端的外层循环就是"按同步表清单逐表拉到底"。
>
> 排序**按主键稳定升序**：SQLite 不保证无 `ORDER BY` 的顺序，翻页时顺序不稳就会
> **重复或漏掉**行（有专门用例翻完所有页、去重后数条数）。
>
> 合法表名 = §6 的唯一真源表，也就是 `manifest.tables` 里 `synced: true` 的那 **12 张**
> （2026-09-27 由 10 张变 12：`ai_runs` / `ai_tool_calls` 进同步范围，见
> `sqlite-schema-contract.md` §6 那段注）。
> **改这份清单要同时改两端**（手机侧 `MobileSchema` 有一份镜像清单）。

---

## 4. `POST /api/sync/push`

**用途**：上行 —— 把手机 `_outbox` 里攒的东西推上来。

```json
{
  "deviceId": "phone-7f3a…",
  "batch": [
    { "kind": "ai_message", "refId": "9b6d01af-…", "op": "upsert", "payload": { "…": "完整行" } }
  ]
}
```

```json
{ "applied": [ "9b6d01af-…" ], "rejected": [ { "refId": "…", "code": "SCHEMA_MISMATCH", "message": "…" } ] }
```

硬约束：

| 约束 | 为什么 |
| --- | --- |
| **幂等**：同一条推两次结果相同（靠主键 UUID + upsert） | 手机重试是常态；重复推送**不许**产生重复行（M4 的 DoD 就是这一条） |
| **冲突用"服务端赢"（LWW）** | 桌面是唯一真源（架构纪律 1）；`updated_at` 比桌面新才覆盖，否则丢弃并记 `rejected` |
| **`rejected` 必须逐条给 code** | 手机要能区分"永远不可能成功"（丢掉）与"暂时失败"（重试）。不许只回一个总数 |
| 单批上限（默认 500 行） | 与 `changes` 的 limit 对齐，别让一次请求扛几万行 |
| **手机不许创建 `prompts` / `media_assets` / `tags`** | 那些由桌面在提交时产生（总纲 §3.3）。推上来只能是 `rejected` |
| **能推的 `kind` 是一张**表**，不是"什么都能推"** | 桌面 `table_of_kind` 认这 6 个：`ai_conversation` / `ai_message` / `ai_message_part` / `ai_attachment` / `ai_run` / `ai_tool_call`。别的回**永久性** `UNKNOWN_KIND`（手机据此丢掉那条 outbox，不再重试）。⚠ `ai_tool_calls` **没有 `updated_at`** → LWW 对它不生效，纪律是"只推自己写的行" |

---

## 4.1 `kind: "submit_job"` —— 手机排队、桌面真正提交（M5，2026-09-27 拍板）

**它不是"一张表的行"**，而是一条**指令**：所以桌面 `push` 里单独一个分支，
不走 `table_of_kind`；`refId` 是手机生成的 `queueId`（幂等键）。

```json
{ "kind": "submit_job", "refId": "<queueId>", "op": "upsert",
  "payload": {
    "queueId": "…", "conversationId": "…", "runId": "…",
    "promptId": 310, "sourceRef": "file:<sha256>",
    "task": "t2i", "modelFamily": "anima", "positivePrompt": "…",
    "width": 832, "height": 1216,
    "overrides": { "6.text": "…" }, "nsfw": false,
    "requestedAt": "2026-09-27T…Z", "requestedBy": "phone-7f3a…"
  } }
```

**两条"给它工作流"的路**（2026-11-09 加，`docs/workflow-auto-select-plan.md` §1.2）：

| 路 | 键 | 桌面做什么 |
| --- | --- | --- |
| **精确指定** | `promptId` / `sourceRef` | 今天的行为，一个字没变（`sourceRef` 在受理时解析成 `promptId`） |
| **只说族** | `task` + `modelFamily`（可选 `positivePrompt` / `width` / `height`） | 桌面按 **① 同任务同族的最近成功 → ② 配方的已验证绑定** 自己挑一份，把提示词写进去再跑；挑不到就回一条 `NO_WORKFLOW_MATCH`（列出试过哪两档），**不猜** |

- **`promptId` / `sourceRef` 与"只说族"互斥地优先**：给了前两者就按精确的那份走，族只用来记账。
- **`task` 只认七档**（t2i / i2i / edit / inpaint / upscale / video / audio）；`width` / `height`
  只认 64~8192 —— 形状错是**永久性**拒绝（重推一万次也一样）。
- **默认不改构图分辨率**：`width` / `height` 不给就沿用目标工作流自己的宽高；
  给了才写进那份工作流自己的 `width` / `height` 键（它没有就如实写在回执里）。
- 走① / ② 那一路时，`capture_runs.source_kind` 记成 `recent` / `recipe`
  （精确指定的不声明，保持列默认 `'ComfyUI'`）。

四条硬约束：

| 约束 | 为什么 |
| --- | --- |
| **桌面收到后不许直接提交**，必须先 `ToolApprovalGate.open(callId)` 再发 `tool.requested`，**在桌面上等人点批准** | R8 的顺序；**"手机点过提交"不算批准**（总纲 §7 待定项 2）。手机上那一下只是"排队" |
| 批准后走**现有 `comfy_submit` 全链路**（审核 → 提交 → 等 → `capture.captureRun()` 入库），**一条捷径都不许抄** | R10。抄捷径的表现是"手机上排了队、画廊里没有" |
| **结果走普通下行同步回手机**：桌面往**发起那条会话**追加一条助手消息（`role = assistant`，正文一句人话 + 一个 `tool_result` 块，`json_payload` 里带 `mediaIds`） | 不为 M5 另造第二条回传通道 —— 手机本来就在同步 `ai_conversations` / `ai_messages` / `ai_message_parts` |
| **失败也要回一条**（`tool_result` 带 `ok=false` + 稳定码） | 手机上那条"已排队"永远停在排队中是最坏的表现：用户以为在跑，其实什么都没发生 |

- **幂等**：`queueId` 已有终态 → 回 `applied` 且**不重复提交**（手机重试是常态）。
- **排队 ≠ 已提交**：手机侧工具结果里**不许**出现 `mediaIds`、**不许**说"已提交"（R11）；
  桌面那条回传消息才是唯一凭据。
- 手机侧旧的 `OutboxKind.RUN` **不再需要**：手机自己的 Run 行走 `ai_run`（§4 那张表），
  只有"排队提交"这条指令用 `submit_job`。

---

## 5. 账本裁剪（桌面侧的后台任务）

```sql
-- 保留窗口 = 所有客户端的游标里最小的那个
DELETE FROM _sync_log WHERE seq <= (SELECT MIN(cursor) FROM sync_clients);
```

- 没有客户端（`sync_clients` 为空）时**不能裁**（否则新手机一来就必然 410）。
- 裁剪与 §2 的 410 是**一对**：正因为裁掉了老 `seq`，才需要 410 这个分支。
- ⚠ 与 **R5** 分清楚：R5 说的是 `ai_run_events`（**Run 事件流**，手机重连靠 `?after=<seq>`）
  和这里的 `_sync_log`（**变更账本**）是**两本不同的账**，各自的保留窗口各自定，别互相顶替。

---

## 6. 交付物清单（M1 落地时要对着勾）

| 落地物 | 在哪 |
| --- | --- |
| 4 条端点的 OpenAPI 片段 | 本文件（§1~§4 的请求/响应就是冻结形状） |
| `_sync_log` / `sync_clients` DDL | `docs/sqlite-schema-contract.md` §3 |
| 触发器（每张被同步的表 3 个） | 同上 |
| 手机 `_outbox` DDL | 同上 §4 |
| 410 分支 | §2 —— **不是可选项** |
| 幂等用例（重复推送不产生重复行） | M4 的 DoD |
