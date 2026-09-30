//! **跨端待办总览**：`GET /api/ai/pending`（M1 定的契约，P4 第 25 步搬到 Rust）。
//!
//! # 它是给谁用的
//!
//! "另一台设备刚连上来"——手机、第二个窗口、或者刚重启完的桌面 App。它**没有经历**
//! 那条 SSE 事件流，所以那些"正在等人"的卡片（AI 的提问、需要批准的工具调用）
//! 靠这一个接口一次补齐，而不是逐个 Run 去问。
//!
//! # ⚠ 与 Kotlin 侧的一处**必须说清的差异**
//!
//! Kotlin 那边 `asks` 来自**内存里的提问闸门**（`AskRegistry.allPending()`），
//! `approvals` 来自 `ai_tool_calls` 的 pending 行。Rust 侧**没有 Run 执行器**，
//! 也就没有那个内存闸门 —— 所以这里 `asks` 直接读 **`ai_questions` 表里 `status='pending'` 的行**。
//!
//! 为什么这不是"退一步的实现"，而是同一件事：
//! M1 的纪律是 **`AskRegistry.track()` 当场落一行 pending**（正是为了"界面还没挂上、
//! 后端重启了，那张卡也不能丢"），而收尾只把 `pending` 改成终态。
//! 于是"表里还 pending 的行" ≡ "闸门里还活着的提问" —— 前提被写进了 M1 的用例
//! （`AskPersistenceTest`）。反过来，**内存里活着但库里没有**这种状态 M1 起就不存在。
//!
//! 真机上验这一条很容易：真库副本上 `sqlite3 ... "SELECT status, COUNT(*) FROM ai_questions GROUP BY status"`，
//! 有 pending 行时本接口就该回出来那几条。
//!
//! # 形状（与 Kotlin 逐字对齐，键名错一个界面就少一块内容）
//!
//! ```jsonc
//! { "asks":       [ {questionId, runId, conversationId, question, options[], allowFreeText,
//!                    answered, askedAt, index, total, groupId, multiSelect,
//!                    header?, answer?, optionIndex?, optionIndexes?} ],
//!   "approvals":  [ {callId, runId, conversationId, name, argumentsJson, askedAt} ] }
//! ```
//!
//! 两条容易做错的地方：
//! ① **`askedAt` 是数字（epoch 毫秒）**，不是 ISO 文本 —— 提问那份是 `askedAtMs`，
//!    而审批那份是 ISO 字符串（`rs.isoTime`）。两个 `askedAt` 类型不同，别想着"统一一下"；
//! ② 可空键**不出现**（`explicitNulls = false`）：`header` / `answer` / `optionIndex`
//!    没有就整个键不出现，`optionIndexes` 空数组也不出现。
//!
//! # 2026-09-27（M5）：`approvals[]` 里现在有**两类**东西
//!
//! 除了桌面 Run 的工具调用，还并进了**手机排队的提交**（`submit_jobs.state='awaiting_approval'`，
//! 契约 §4.1）。形状**完全同形**（同样 6 个键），所以列表本身、以及点它的那条路
//! （`POST /api/ai/tool-calls/{callId}/{approve,deny}`）**一个字都没变** ——
//! 用户看到的还是"有 N 个东西在等你批"。落点为什么不是 `ai_tool_calls` 那张表：
//! 见 [`crate::mobile::submit_queue`] 的文件头（一句话：那张表的外键要求先有一个**桌面 Run**，
//! 而"为了过闸门捏造一个 Run"是被明确禁止的）。

use rusqlite::Connection;
use serde_json::{Map, Value};

use crate::infra::iso_time::iso_time;
use crate::web::routes::{ApiError, AppState};

/// `GET /api/ai/pending`。
pub fn pending(state: &AppState) -> Result<Value, ApiError> {
    let asks = state.with(crate::ai::questions::list_pending)?;
    let approvals = state.with(list_pending_approvals)?;
    let mut body = Map::new();
    body.insert("asks".into(), Value::Array(asks));
    body.insert("approvals".into(), Value::Array(approvals));
    Ok(Value::Object(body))
}

/// 还在等批准的调用，**两类合成一个列表**（形状逐字一致，客户端不必分两路）：
///
/// 1. **桌面 Run 的工具调用**：`ai_tool_calls.status='pending'` **且 Run 还在 running**；
/// 2. **手机排队的提交**（M5，契约 §4.1）：`submit_jobs.state='awaiting_approval'`
///    —— 见 [`crate::mobile::submit_queue`] 的文件头，落点为什么不是 `ai_tool_calls`
///    （外键 `→ ai_runs(id)` + "不许为了它捏造一个桌面 Run"）。
///
/// 两条为什么都合成 `approvals[]`：**审批在桌面上必须看得见**（契约 §4.1 第一条），
/// 而复用的是**同一条通道** —— 同一个列表 + 同一个
/// `POST /api/ai/tool-calls/{callId}/{approve,deny}`（`runs::resolve_approval` 内部
/// 两类都认）。另造一条通道会出现"手机上排了队、桌面上没有任何东西可以点"。
fn list_pending_approvals(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT tc.provider_call_id, tc.run_id, r.conversation_id, tc.name,
                tc.arguments_json, tc.started_at
           FROM ai_tool_calls tc
           JOIN ai_runs r ON r.id = tc.run_id
          WHERE tc.status = 'pending' AND r.status = 'running'
          ORDER BY tc.started_at, tc.id",
    )?;
    let mut rows = stmt
        .query_map([], |row| {
            let mut out = Map::new();
            out.insert(
                "callId".into(),
                Value::String(row.get::<_, Option<String>>("provider_call_id")?.unwrap_or_default()),
            );
            out.insert("runId".into(), Value::String(row.get::<_, Option<String>>("run_id")?.unwrap_or_default()));
            out.insert(
                "conversationId".into(),
                Value::String(row.get::<_, Option<String>>("conversation_id")?.unwrap_or_default()),
            );
            out.insert("name".into(), Value::String(row.get::<_, Option<String>>("name")?.unwrap_or_default()));
            out.insert(
                "argumentsJson".into(),
                Value::String(row.get::<_, Option<String>>("arguments_json")?.unwrap_or_default()),
            );
            // ⚠ 这一份是 **ISO 文本**（提问那份是毫秒数），照 Kotlin `rs.isoTime` 的规则
            out.insert(
                "askedAt".into(),
                match iso_time(row.get::<_, Option<String>>("started_at")?.as_deref()) {
                    Some(text) => Value::String(text),
                    None => Value::Null,
                },
            );
            Ok(Value::Object(out))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // 手机排队的提交接在后面（同一把锁、同一次读，两类的排序各自稳定）
    rows.extend(crate::mobile::submit_queue::pending_approvals(conn)?);
    Ok(rows)
}

#[cfg(test)]
mod tests;
