//! Run 的持久化（P4 第 26 步）—— 逐条对照 Kotlin `AiRunRepo`。
//!
//! # 为什么这一层单独先做
//!
//! Run 是"**两条独立的读法**"共用的同一份事实：
//! ① 进程活着时界面走 SSE（内存事件总线，下一条提交）；
//! ② 断线重连 / 后端重启后**只能从库里回放**（`ai_run_events`）。
//! 两边的形状必须一模一样 —— 所以先把"落库与读回"这半边钉住，
//! 再去接执行器（它只负责"产生事件"，不关心事件最终去了哪）。
//!
//! ## 与 Kotlin 的逐条对应
//!
//! | Kotlin | 这里 | 备注 |
//! | --- | --- | --- |
//! | `AiRunRepo.create` | [`create`] | 14 列，`started_at` 用 SQL 侧 `strftime` |
//! | `AiRunRepo.get` | [`get`] | 11 个键，可空的一律 `isoTime` → 键不出现 |
//! | `AiRunRepo.finish` | [`finish`] | **终态只认三个**（写别的当场抛） |
//! | `AiRunRepo.failStaleRunning` | [`fail_stale_running`] | 启动恢复：还在 running 的一律标失败 |
//! | `AiRunRepo.appendEvent` / `EventBatcher` | [`append_event`] / [`EventBatcher`] | 批量写（P1：单 Run 实测 4.7 万条） |
//! | `AiRunRepo.events` | [`events`] | `seq > after ORDER BY seq`（断线续传的唯一读法） |
//! | `AiRunRepo.pruneEvents` | [`prune_events`] | **保留窗口（红线 R5）**：留尾部 4000 条 |
//! | `AiRunRepo.updateMessage` | [`update_message`] | 流式期间不断追加正文 |
//! | `AiRunRepo.resetAssistantForRetry` | [`reset_assistant_for_retry`] | 清空正文 **+ 删光有序块**（R6） |
//! | `AiRunRepo.insertAssistantPlaceholder` | [`insert_assistant_placeholder`] | `seq = max+1` |
//! | `AiRunRepo.recordToolCall` 一族 | [`record_tool_call`] 一族 | pending 行原地更新 |
//!
//! # 三条最容易静默做错的地方
//!
//! 1. **`updated_at` 是承重的**（会话列表按它排序）：追加消息必须把它顶上去。
//!    触发器 `trg_ai_conversations_touch` 只在 `UPDATE` 时点火，而这里改的是 `ai_messages`
//!    —— 会话那一行不动就**不会**自动更新，必须显式写。
//! 2. **`prune_events` 留的是尾部**：`seq <= MAX(seq) - keep` 删掉的是最老的。
//!    写成 `>=` 就把最新的删了 —— 而界面上表现为"刚发的消息一刷新就没了"。
//! 3. **`reset_assistant_for_retry` 必须清块**：只清 `text` 的话，上一次失败前落的
//!    思考段 / 工具卡会混进新一轮回复里（R6 的"绝不重复内容"）。

use rusqlite::Row;
use serde::Serialize;
use serde_json::{json, Value};

use crate::infra::iso_time::iso_time;
use crate::web::routes::{ApiError, AppState};

/// 一次 Run（与 Kotlin `AiRunDto` 逐字同形，11 个键）。
///
/// ⚠ `status` / `conversationId` / `id` **一定在**；其余 8 个可空键为 NULL 时**整个键不出现**
/// （`explicitNulls = false`）。少一个键 = 界面少一块内容；多一个键 = 前端拿不到的类型。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRunDto {
    pub id: String,
    pub conversation_id: String,
    /// running / completed / failed / cancelled
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    /// 这次 Run 能用的 Skill 快照（`{count, skills:[{name,digest,source}]}`）。
    /// 以前只写库、谁也读不到；现在随 Run DTO 交给界面做"当时用的是哪版规则"的追溯。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_snapshot: Option<Value>,
    /// 本次 Run **实际生效**的思考强度（`off` = 不支持或已关闭，AIH-056）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Run 的三个终态（`AiRunRepo.finish` 只认这三个；写第四个当场抛）。
pub const TERMINAL: [&str; 3] = ["completed", "failed", "cancelled"];

/// 还在跑的 Run。
pub const RUNNING: &str = "running";

/// 库里 `ai_run_events` 每个 Run 的**保留窗口**（Kotlin `HarnessRunner.EVENT_KEEP`）。
///
/// ⚠ 这是**红线 R5** 的数字：手机后台被杀是常态，重连靠 `?after=<seq>` 续传。
/// 留得太小，一次长 Run（实测 4.7 万条 `text.delta`）的早期事件就补不回来了；
/// 全量留着则是 P1 要治的"`ai_run_events` 涨到 62 万行 / 112.9MB"。
/// 与内存环形缓冲（`RunEventBus::MEMORY_KEEP = 2000`，下一条提交）**刻意不同数**：
/// 前者管"进程活着时能重放多少"，后者管"后端重启后能回放多少"。
pub const EVENT_KEEP: i64 = 4000;

/// 一次事件批写的默认批量（Kotlin `EventBatcher(batchSize = 200)`）。
pub const EVENT_BATCH: usize = 200;

/// `ai_runs` 的一行 → DTO（`SELECT *` 即可，列名不变）。
pub fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AiRunDto> {
    Ok(AiRunDto {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        status: row.get::<_, Option<String>>("status")?.unwrap_or_default(),
        provider_id: row.get("provider_id")?,
        model_id: row.get("model_id")?,
        assistant_message_id: row.get("assistant_message_id")?,
        user_message_id: row.get("user_message_id")?,
        error_code: row.get("error_code")?,
        error_message: row.get("error_message")?,
        prompt_version: row.get("prompt_version")?,
        skill_snapshot: row
            .get::<_, Option<String>>("skill_snapshot")?
            .and_then(|text| serde_json::from_str(&text).ok()),
        reasoning_effort: row.get("reasoning_effort")?,
        retry_of_run_id: row.get("retry_of_run_id")?,
        started_at: iso_time(row.get::<_, Option<String>>("started_at")?.as_deref()),
        completed_at: iso_time(row.get::<_, Option<String>>("completed_at")?.as_deref()),
        created_at: iso_time(row.get::<_, Option<String>>("created_at")?.as_deref()),
    })
}

/// Run 不存在时 Kotlin 回的那个错（`AiException(CONFIG_ERROR, "Run 不存在: xx")`，400）。
pub fn run_missing(id: &str) -> ApiError {
    ApiError::ai("CONFIG_ERROR", format!("Run 不存在: {id}"))
}

/// `AiRunRepo.create` 的入参。
///
/// 两个快照是**原样 JSON**（调用方已经拼好）：`provider_snapshot` 里**只有引用名、
/// 没有密钥**（AIH-023），`model_snapshot` 里含 NSFW 开关与生效的思考强度。
#[derive(Debug, Clone)]
pub struct NewRun {
    pub conversation_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub provider_snapshot: Value,
    pub model_snapshot: Value,
    pub prompt_version: String,
    pub user_message_id: Option<String>,
    pub assistant_message_id: String,
    pub retry_of_run_id: Option<String>,
    pub reasoning_effort: Option<String>,
    /// 本次 Run 生效的 Skills（名称 + digest，AIH-047）
    pub skill_snapshot: Option<Value>,
}

/// 建一行 Run（`status='running'`、`started_at=now`）。
pub fn create(state: &AppState, new_run: NewRun) -> Result<AiRunDto, ApiError> {
    let id = crate::ai::conversations::uuid_v4()?;
    let id_in = id.clone();
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO ai_runs
               (id, conversation_id, status, provider_id, model_id, user_message_id, assistant_message_id,
                provider_snapshot, model_snapshot, skill_snapshot, prompt_version, retry_of_run_id,
                reasoning_effort, started_at)
             VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            rusqlite::params![
                id_in,
                new_run.conversation_id,
                new_run.provider_id,
                new_run.model_id,
                new_run.user_message_id,
                new_run.assistant_message_id,
                serde_json::to_string(&new_run.provider_snapshot).unwrap_or_else(|_| "{}".into()),
                serde_json::to_string(&new_run.model_snapshot).unwrap_or_else(|_| "{}".into()),
                new_run
                    .skill_snapshot
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into())),
                new_run.prompt_version,
                new_run.retry_of_run_id,
                new_run.reasoning_effort,
            ],
        )?;
        Ok(())
    })?;
    get(state, &id)
}

/// `GET /api/ai/runs/{id}` 与内部读回都走它。
pub fn get(state: &AppState, id: &str) -> Result<AiRunDto, ApiError> {
    let found = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT * FROM ai_runs WHERE id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![id], run_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    found.ok_or_else(|| run_missing(id))
}

/// 落终态（`completed` / `failed` / `cancelled`）。
///
/// 非终态当场抛 —— 与 Kotlin 同："把 running 又写一遍"看着像成功，
/// 实际会让一个卡住的 Run 永远不被 `fail_stale_running` 收走。
pub fn finish(
    state: &AppState,
    id: &str,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    usage: Option<&Value>,
) -> Result<(), ApiError> {
    if !TERMINAL.contains(&status) {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("非法的 Run 终态: {status}"),
        ));
    }
    let usage_json = usage.map(|v| v.to_string());
    state.write(|tx| {
        tx.execute(
            "UPDATE ai_runs
                SET status = ?1, error_code = ?2, error_message = ?3, usage_json = ?4,
                    completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE id = ?5",
            rusqlite::params![status, error_code, error_message, usage_json, id],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 启动恢复：把库里还挂在 `running` 的 Run 全标成 `failed`（否则界面永远转圈）。
///
/// 返回改了几行 —— 启动日志要如实报出来（"收尾了 N 个僵死 Run"）。
///
/// ⚠ **只改 `ai_runs` 是不够的**：助手消息会永远停在 `streaming`、正文为空，
/// 重新打开会话看到的就是一条空气泡（2026-11-14 用户报的"生成中的回答被打断，
/// 切回来不显示已经生成好的部分"）。所以每条僵死 Run 还要把它的助手消息一起收尾，
/// 正文 / 思考从**它自己的事件流**里拼回来（见 [`restore_interrupted_reply`]）。
pub fn fail_stale_running(state: &AppState, reason: &str) -> Result<usize, ApiError> {
    // ⚠ 先把这批 Run 捞出来：下面每条都要再读写消息，不能和 `state.with` 的连接锁嵌套。
    let stale: Vec<(String, Option<String>)> = state.with(|conn| {
        let mut stmt =
            conn.prepare("SELECT id, assistant_message_id FROM ai_runs WHERE status = 'running'")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect()
    })?;

    for (run_id, assistant_id) in &stale {
        let Some(message_id) = assistant_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        if let Err(e) = restore_interrupted_reply(state, message_id, run_id, "failed") {
            // 收不回正文也不能让 Run 停在 running：先把状态收掉，日志留给人看。
            eprintln!("启动恢复：Run {run_id} 的助手消息没收成：{}", e.human());
        }
    }

    state.write(|tx| {
        let n = tx.execute(
            "UPDATE ai_runs
                SET status = 'failed', error_code = 'PROTOCOL_ERROR', error_message = ?1,
                    completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE status = 'running'",
            rusqlite::params![reason],
        )?;
        Ok(n)
    })
}

/// 修补**旧版本留下的半截账**：Run 已经收尾，助手消息却还停在 `streaming`。
///
/// 2026-11-13 及以前的 `fail_stale_running` 只改 `ai_runs`：后端重启后 Run 是 `failed`，
/// 助手消息永远停在 `streaming`、正文为空 —— 用户重开会话看到的是一条空气泡
/// （2026-11-14 报的那条，现场数据就是这样）。新版收尾会一起修；这个补丁再扫一遍
/// 库里**已经坏掉的**那些（升级后第一次启动就修好，不用用户删会话）。
///
/// 每次启动都跑：正常情况下一条都扫不到（`streaming` 只存在于真在跑的 Run 上）。
pub fn repair_orphan_streaming_messages(state: &AppState) -> Result<usize, ApiError> {
    // 同一条消息可能挂在两次 Run 上（重试复用同一条气泡）：只认**最新那次**，
    // 否则先后两次修补会互相覆盖（旧 Run 的正文盖掉重试的）。
    let orphans: Vec<(String, String, String)> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT r.id, m.id, r.status FROM ai_messages m
               JOIN ai_runs r ON r.assistant_message_id = m.id
              WHERE m.status = 'streaming'
                AND r.status IN ('completed', 'failed', 'cancelled')
              ORDER BY r.created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect()
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut repaired = 0;
    for (run_id, message_id, run_status) in &orphans {
        if !seen.insert(message_id.clone()) {
            continue;
        }
        let status = if run_status == "completed" {
            "complete"
        } else {
            run_status.as_str()
        };
        match restore_interrupted_reply(state, message_id, run_id, status) {
            Ok(()) => repaired += 1,
            Err(e) => eprintln!(
                "启动恢复：修补半截消息 {message_id}（Run {run_id}）失败：{}",
                e.human()
            ),
        }
    }
    Ok(repaired)
}

/// 把一条收尾了的 Run 的助手消息从它自己的事件流里救回来（启动恢复专用）。
///
/// 流式正文原先只走"收尾一次写"：进程被强杀 / 崩溃时，已经说出去的那一大段只剩内存，
/// 库里是一条 `streaming` 的空占位。这里做三件事：
/// ① `text.delta` / `reasoning.delta` 按 `seq` 顺序拼回正文与思考（SSE 连着时这些事件
///    早就落库了 —— 它们按 `seq` 批量写，比消息正文更细）；
/// ② 消息里可能还有**流式检查点**写的正文（`executor::CHECKPOINT_INTERVAL`），取更长的那份；
/// ③ 正文 / 思考落成有序块（界面的空气泡判据是"正文与块都空"），状态按调用方给的终态写。
///
/// 工具卡与工具结果**补不回来**（`tool.completed` 只存了 preview）：这里只保证
/// "已经生成的回答"不消失。补不回来时也如实写日志，不假装完整。
fn restore_interrupted_reply(
    state: &AppState,
    message_id: &str,
    run_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    let (event_text, event_reasoning) = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT event_type, payload_json FROM ai_run_events
              WHERE run_id = ?1 AND event_type IN ('text.delta', 'reasoning.delta')
              ORDER BY seq",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut text = String::new();
        let mut reasoning = String::new();
        for row in rows {
            let (kind, payload) = row?;
            let chunk = serde_json::from_str::<Value>(&payload)
                .ok()
                .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_string));
            let Some(chunk) = chunk else { continue };
            if kind == "text.delta" {
                text.push_str(&chunk);
            } else {
                reasoning.push_str(&chunk);
            }
        }
        Ok((text, reasoning))
    })?;

    // 消息里已有的正文（检查点写的）可能比事件更多：取更长的那个。
    // 行不在了（历史夹具 / 被人删掉）就什么都不用做 —— 别去给不存在的消息插块。
    let current = state.with(|conn| {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT text FROM ai_messages WHERE id = ?1",
            rusqlite::params![message_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
    })?;
    let Some(current) = current else {
        return Ok(());
    };
    let current = current.unwrap_or_default();
    let text = if current.len() > event_text.len() {
        current
    } else {
        event_text
    };

    state.write(|tx| {
        tx.execute(
            "UPDATE ai_messages SET text = ?1, status = ?2 WHERE id = ?3",
            rusqlite::params![&text, status, message_id],
        )?;
        // 已经有块（极少见：崩在收尾那一瞬）就别动它 —— 那可能是比这里更完整的版本。
        let has_parts: i64 = tx.query_row(
            "SELECT COUNT(*) FROM ai_message_parts WHERE message_id = ?1",
            rusqlite::params![message_id],
            |row| row.get(0),
        )?;
        if has_parts == 0 {
            let mut ordinal = 0i64;
            if !event_reasoning.is_empty() {
                tx.execute(
                    "INSERT INTO ai_message_parts (message_id, ordinal, type, text)
                     VALUES (?1, ?2, 'reasoning', ?3)",
                    rusqlite::params![message_id, ordinal, event_reasoning],
                )?;
                ordinal += 1;
            }
            if !text.is_empty() {
                tx.execute(
                    "INSERT INTO ai_message_parts (message_id, ordinal, type, text)
                     VALUES (?1, ?2, 'text', ?3)",
                    rusqlite::params![message_id, ordinal, &text],
                )?;
            }
        }
        Ok(())
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
//  事件（断线续传 / 后端重启回放的唯一来源）
// ---------------------------------------------------------------------------

/// 一条事件（`RunEvent` 的存储形态）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub r#type: String,
    pub payload: Value,
}

impl StoredEvent {
    /// SSE 帧（与 Kotlin `RunEvent.toSse()` 逐字节同形：`id:` / `event:` / `data:` 三行 + 空行）。
    pub fn to_sse(&self) -> String {
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.seq,
            self.r#type,
            // payload 可能是 `{}`；`to_string` 不保证键序，但 SSE 的 data 是 JSON 对象、
            // 客户端按 key 读，键序无所谓（契约 §2 只钉了字段名与类型）。
            self.payload
        )
    }
}

/// 单条落库（`INSERT OR IGNORE`：同一个 `(run_id, seq)` 重放是幂等的）。
pub fn append_event(
    state: &AppState,
    run_id: &str,
    seq: i64,
    event_type: &str,
    payload: &Value,
) -> Result<(), ApiError> {
    state.write(|tx| {
        tx.execute(
            "INSERT OR IGNORE INTO ai_run_events (run_id, seq, event_type, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![run_id, seq, event_type, payload.to_string()],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// **批量**写事件（P1）。
///
/// 为什么要有它：`text.delta` 是逐字来的，实测**单 Run 4.7 万个事件**，
/// 而"一条一个事务"就是 `ai_run_events` 涨到 62 万行 / 112.9MB 的直接原因。
///
/// ⚠ 调用方**必须在 Run 收尾时 `flush()`**（正常 / 失败 / 取消三条路都要）：
/// 否则最后那不足一批的 `run.completed` 会丢，断线重连就补不回来了 ——
/// 那正好是 R6"已经说出去的话一个字都不能丢"的反面。
pub struct EventBatcher<'a> {
    state: &'a AppState,
    run_id: String,
    batch_size: usize,
    pending: Vec<(i64, String, Value)>,
}

impl<'a> EventBatcher<'a> {
    pub fn new(state: &'a AppState, run_id: impl Into<String>, batch_size: usize) -> Self {
        Self {
            state,
            run_id: run_id.into(),
            batch_size: batch_size.max(1),
            pending: Vec::new(),
        }
    }

    /// 攒一条；满一批就写下去。
    pub fn add(&mut self, seq: i64, event_type: &str, payload: &Value) -> Result<(), ApiError> {
        self.pending
            .push((seq, event_type.to_string(), payload.clone()));
        if self.pending.len() >= self.batch_size {
            self.flush()?;
        }
        Ok(())
    }

    /// 把剩下的写下去；返回本次写入条数（0 = 没有待写）。
    pub fn flush(&mut self) -> Result<usize, ApiError> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let batch = std::mem::take(&mut self.pending);
        let run_id = self.run_id.clone();
        let n = batch.len();
        // 一个事务写完一批（R2 的 `BEGIN IMMEDIATE` 由 `state.write` 保证）
        self.state.write(move |tx| {
            for (seq, event_type, payload) in &batch {
                tx.execute(
                    "INSERT OR IGNORE INTO ai_run_events (run_id, seq, event_type, payload_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![run_id, seq, event_type, payload.to_string()],
                )?;
            }
            Ok(())
        })?;
        Ok(n)
    }
}

/// 回放：`seq > after` 的全部事件，按 `seq` 升序。
///
/// 这是**断线续传的唯一读法**（契约 §2：游标一律 `?after=<seq>`）。
pub fn events(state: &AppState, run_id: &str, after: i64) -> Result<Vec<StoredEvent>, ApiError> {
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT seq, event_type, payload_json FROM ai_run_events
              WHERE run_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![run_id, after], |row| {
                Ok(StoredEvent {
                    seq: row.get("seq")?,
                    r#type: row.get("event_type")?,
                    payload: parse_payload(
                        row.get::<_, Option<String>>("payload_json")?.as_deref(),
                    ),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// 保留窗口（**红线 R5**）：只留最近 `keep` 条，`seq` 本身绝不回退/复用。
///
/// 返回删掉的条数。⚠ 判断必须留尾部：`seq <= MAX(seq) - keep`。
pub fn prune_events(state: &AppState, run_id: &str, keep: i64) -> Result<usize, ApiError> {
    state.write(|tx| {
        let n = tx.execute(
            "DELETE FROM ai_run_events
              WHERE run_id = ?1 AND seq <= (
                SELECT MAX(seq) - ?2 FROM (SELECT seq FROM ai_run_events WHERE run_id = ?1) t
              )",
            rusqlite::params![run_id, keep],
        )?;
        Ok(n)
    })
}

// ---------------------------------------------------------------------------
//  助手消息的流式写（R6 的落点）
// ---------------------------------------------------------------------------

/// 流式期间不断追加正文 / 收尾落终态（`AiRunRepo.updateMessage`）。
///
/// `provider_response_id` 用 `COALESCE`：**传 None 不覆盖已有的值**（Kotlin 同）。
pub fn update_message(
    state: &AppState,
    message_id: &str,
    text: &str,
    status: &str,
    provider_response_id: Option<&str>,
    usage: Option<&Value>,
) -> Result<(), ApiError> {
    let usage_json = usage.map(|v| v.to_string());
    state.write(|tx| {
        tx.execute(
            "UPDATE ai_messages
                SET text = ?1, status = ?2,
                    provider_response_id = COALESCE(?3, provider_response_id),
                    usage_json = ?4
              WHERE id = ?5",
            rusqlite::params![text, status, provider_response_id, usage_json, message_id],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 重试时把那条**失败 / 取消**的助手消息就地重置成流式占位（AIH-024 / R6）。
///
/// 三条别动：
/// ① **有序块一起清** —— 只清正文的话，上次失败前的思考段与工具卡会混进新一轮；
/// ② `provider_response_id` / `usage_json` 也清（那是上一轮的账）；
/// ③ 消息 `id` **不变**（`seq` 不变、空壳不残）—— 新建一条才是"同一条语句出现两次"的病根。
pub fn reset_assistant_for_retry(
    state: &AppState,
    message_id: &str,
    provider_id: &str,
    model_id: &str,
) -> Result<(), ApiError> {
    state.write(|tx| {
        tx.execute(
            "DELETE FROM ai_message_parts WHERE message_id = ?1",
            rusqlite::params![message_id],
        )?;
        tx.execute(
            "UPDATE ai_messages
                SET status = 'streaming', text = '', provider_id = ?1, model_id = ?2,
                    provider_response_id = NULL, usage_json = NULL
              WHERE id = ?3",
            rusqlite::params![provider_id, model_id, message_id],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 新建一条空的助手占位（`status='streaming'`、`text=''`），`seq = max+1`。
///
/// 返回新消息 id。`seq` 由**服务端**分配：调用方自己算的话，两台设备同时追加就会撞号。
pub fn insert_assistant_placeholder(
    state: &AppState,
    conversation_id: &str,
    provider_id: &str,
    model_id: &str,
) -> Result<String, ApiError> {
    let id = crate::ai::conversations::uuid_v4()?;
    let id_in = id.clone();
    let conversation = conversation_id.to_string();
    let (provider_id, model_id) = (provider_id.to_string(), model_id.to_string());
    state.write(move |tx| {
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM ai_messages WHERE conversation_id = ?1",
            rusqlite::params![conversation],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, provider_id, model_id)
             VALUES (?1, ?2, ?3, 'assistant', 'streaming', '', ?4, ?5)",
            rusqlite::params![id_in, conversation, seq, provider_id, model_id],
        )?;
        // 会话列表按 updated_at 排序：新回复必须把会话顶上去
        tx.execute(
            "UPDATE ai_conversations SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
            rusqlite::params![conversation],
        )?;
        Ok(())
    })?;
    Ok(id)
}

// ---------------------------------------------------------------------------
//  工具调用（审批状态与执行状态分开记）
// ---------------------------------------------------------------------------

/// 一条工具调用记录（`ToolCallRecord` 的存储形态）。
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub id: String,
    pub run_id: String,
    pub call_id: String,
    pub name: String,
    pub arguments_json: String,
    /// not_required / pending / approved / denied
    pub approval: String,
    /// pending / ok / failed / denied
    pub status: String,
    pub result_json: Option<Value>,
    pub content: Option<String>,
    pub error: Option<String>,
    pub elapsed_ms: i64,
}

/// 正文的截断上限（Kotlin `insertToolCall` 里的 `take(64_000)`）。
pub const TOOL_CONTENT_MAX: usize = 64_000;
/// 参数 JSON 的截断上限（Kotlin 的 `take(16_000)`）。
pub const TOOL_ARGS_MAX: usize = 16_000;
/// 错误文本的截断上限（Kotlin 的 `take(2000)`）。
pub const TOOL_ERROR_MAX: usize = 2_000;

/// 审批等待开始时落一行 `pending`（**R8**：`open(callId)` 的同一刻就落库）。
///
/// 为什么必须先落：旧行为是"invoke 返回后才记"，于是**审批正在等人点按钮的那几分钟里
/// 库里什么都没有** —— 后端在这中间重启，"AI 曾想调用什么、审批没被处理"就凭空消失，
/// 另一台设备也看不到有东西在等它批。
pub fn insert_pending_tool_call(
    state: &AppState,
    run_id: &str,
    call_id: &str,
    name: &str,
    arguments_json: &str,
) -> Result<(), ApiError> {
    let id = crate::ai::conversations::uuid_v4()?;
    let args: String = arguments_json.chars().take(TOOL_ARGS_MAX).collect();
    let (run_id, call_id, name) = (run_id.to_string(), call_id.to_string(), name.to_string());
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO ai_tool_calls
               (id, run_id, provider_call_id, name, arguments_json, approval, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            rusqlite::params![id, run_id, call_id, name, args],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 把**同一行** pending 落成终态；返回 false 表示没有 pending 行可更新。
fn resolve_pending_tool_call(state: &AppState, rec: &ToolCallRecord) -> Result<bool, ApiError> {
    let rec = rec.clone();
    state.write(move |tx| {
        let n = tx.execute(
            "UPDATE ai_tool_calls
                SET approval = ?1, status = ?2, result_json = ?3, content = ?4, error = ?5,
                    elapsed_ms = ?6, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE run_id = ?7 AND provider_call_id = ?8 AND status = 'pending'",
            rusqlite::params![
                rec.approval,
                rec.status,
                rec.result_json.as_ref().map(|v| v.to_string()),
                truncate(&rec.content, TOOL_CONTENT_MAX),
                truncate(&rec.error, TOOL_ERROR_MAX),
                rec.elapsed_ms,
                rec.run_id,
                rec.call_id,
            ],
        )?;
        Ok(n > 0)
    })
}

/// 工具调用落库的统一入口：有 pending 行就**原地更新**，没有就按老路径插入。
pub fn record_tool_call(state: &AppState, rec: &ToolCallRecord) -> Result<(), ApiError> {
    if resolve_pending_tool_call(state, rec)? {
        return Ok(());
    }
    let rec = rec.clone();
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO ai_tool_calls
               (id, run_id, provider_call_id, name, arguments_json, approval, status,
                result_json, content, error, elapsed_ms, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, result_json = excluded.result_json,
                content = excluded.content, error = excluded.error,
                approval = excluded.approval, elapsed_ms = excluded.elapsed_ms,
                completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            rusqlite::params![
                rec.id,
                rec.run_id,
                rec.call_id,
                rec.name,
                rec.arguments_json.chars().take(TOOL_ARGS_MAX).collect::<String>(),
                rec.approval,
                rec.status,
                rec.result_json.as_ref().map(|v| v.to_string()),
                truncate(&rec.content, TOOL_CONTENT_MAX),
                truncate(&rec.error, TOOL_ERROR_MAX),
                rec.elapsed_ms,
            ],
        )?;
        Ok(())
    })
}

/// 后端重启恢复：还挂着 `pending` 的审批行**永远不会有人点**了
/// （内存里的闸门随进程消失，Run 也已被 [`fail_stale_running`] 标失败）。
/// 与审批闸门的既有语义一致 —— **超时 / 取消 / 重启都算拒绝**。
pub fn expire_stale_pending_tool_calls(state: &AppState) -> Result<usize, ApiError> {
    state.write(|tx| {
        let n = tx.execute(
            "UPDATE ai_tool_calls
                SET approval = 'denied', status = 'denied',
                    error = '后端重启，审批等待被打断（按拒绝处理）',
                    completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE status = 'pending'",
            [],
        )?;
        Ok(n)
    })
}

/// 某个 Run 用过的工具（界面回放 / 排查用）。
pub fn list_tool_calls(state: &AppState, run_id: &str) -> Result<Vec<Value>, ApiError> {
    state.with(|conn| {
        let mut stmt =
            conn.prepare("SELECT * FROM ai_tool_calls WHERE run_id = ?1 ORDER BY started_at, id")?;
        let rows = stmt
            .query_map(rusqlite::params![run_id], |row| {
                Ok(json!({
                    "id": row.get::<_, Option<String>>("id")?,
                    "callId": row.get::<_, Option<String>>("provider_call_id")?,
                    "name": row.get::<_, Option<String>>("name")?,
                    "argumentsJson": row.get::<_, Option<String>>("arguments_json")?,
                    "approval": row.get::<_, Option<String>>("approval")?,
                    "status": row.get::<_, Option<String>>("status")?,
                    "error": row.get::<_, Option<String>>("error")?,
                    "elapsedMs": row.get::<_, Option<i64>>("elapsed_ms")?.unwrap_or(0),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// 一次审批等待的**当前状态**（桌面执行器的审批闸门轮询它）。
///
/// 为什么闸门读库而不是读内存：这个仓库的既有纪律是
/// **"内存里活着但库里没有"这种状态不许存在**（M1：`track()` 当场落一行 pending）。
/// 读库顺带买到三件事：后端重启后审批不丢、另一台设备（手机 / 第二个窗口）
/// 点的批准能生效、以及**不需要在进程里维护第二个真源**。
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalState {
    /// 还在等（没人点）
    Pending,
    /// 批准了
    Approved,
    /// 拒绝了（含超时 / 取消 —— 与 Kotlin 的"超时与取消都算拒绝"同语义）
    Denied,
}

/// 查一次调用的审批状态。找不到那一行 → `None`（调用方按"拒绝"处理，**不假设通过**）。
///
/// ⚠ **先看 `approval` 再看 `status`**：批准之后那一行的 `status` **仍然是 `pending`**
/// （它要等 invoke 返回才落终态，见 [`resolve_approval`]）。反过来判会把"已经批过"读成
/// "还在等"——而那个分支的后果是**永远等下去**，直到 5 分钟超时按拒绝处理，
/// 已批准但没有任何变化。
pub fn approval_state(state: &AppState, call_id: &str) -> Result<Option<ApprovalState>, ApiError> {
    let row: Option<(String, String)> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT approval, status FROM ai_tool_calls
              WHERE provider_call_id = ?1
              ORDER BY started_at DESC, id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![call_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    let Some((approval, status)) = row else {
        return Ok(None);
    };
    Ok(Some(match approval.as_str() {
        "approved" => ApprovalState::Approved,
        // `not_required` 的行不该出现在闸门里；真出现了按批准处理（**不是**按拒绝 ——
        // 那会把一个本来不需要审批的调用拦下来，而它已经在库里留了痕）
        "not_required" => ApprovalState::Approved,
        "denied" => ApprovalState::Denied,
        "pending" => ApprovalState::Pending,
        // 认不出来的 `approval`：按 `status` 判。已经终态的一律按拒绝
        // （宁可拦下来，也不能让一个来历不明的关系跑起来）
        _ => match status.as_str() {
            "pending" => ApprovalState::Pending,
            _ => ApprovalState::Denied,
        },
    }))
}

/// 已选择「批准」/「拒绝」（`POST /api/ai/tool-calls/{callId}/approve|deny`）。
///
/// 返回 `true` 表示**确实改到了一行 pending**（界面据此回答 `accepted`：
/// 手快先点了、或者这个调用已经超时收尾了，都是 `accepted=false` —— 那不是错误）。
///
/// # 只改 `approval`，**不动 `status`**
///
/// 两列的分工与 Kotlin 逐字一致（`AiRunRepo.insertPendingToolCall` / `resolvePendingToolCall`）：
/// **`approval` 记录决定，`status` 记录调用的最终结果**。
/// 批准之后 `status` 仍是 `pending`（正在执行），invoke 返回时由
/// [`record_tool_call`] 在**同一行**上落成 `ok` / `failed` / `denied`。
///
/// ⚠ 一并把 `status` 写成 `approved` 看似更"整齐"，实际会**中断那条原地更新的路径**：
/// [`resolve_pending_tool_call`] 认的是 `status = 'pending'`，改完之后它再也匹配不到那一行，
/// 于是**同一次调用在库里变成两行**（一行 approved、一行 ok），
/// 而 `/api/ai/pending` 又会把它当成"还在等人批"。
///
/// `WHERE approval = 'pending'` 那一句是**"决定只能下一次"**（Kotlin 的内存闸门同语义：
/// `resolve` 完那一条就没了）：重复点返回 `false`，而且**没人能把已经批过的调用改成拒绝**
/// —— 否则运行中的那种"我点错了、快撤"会撤回一个已经在跑的命令。
///
/// # M5（2026-09-27）：这条闸门也管**手机排队的提交**
///
/// 桌面上那张"等你批准"的卡可能不是某个 Run 的工具调用，而是**手机排上来的一条
/// `submit_job`**（契约 §4.1）。它记在 `submit_jobs` 那张表上（为什么不塞进
/// `ai_tool_calls`：见 [`crate::mobile::submit_queue`] 的文件头），但**审批通道是同一条** ——
/// 同一个端点、同一个函数，所以这里多认一类 `callId`（`submit:<queueId>`）。
///
/// 两条 UPDATE 在**同一个事务**里：谁先谁后都行（两类 callId 不会撞），
/// `true` = 确实改到了一行（不管改的是哪一类）。
pub fn resolve_approval(state: &AppState, call_id: &str, approved: bool) -> Result<bool, ApiError> {
    let call_id = call_id.to_string();
    state.write(move |tx| {
        let n = tx.execute(
            "UPDATE ai_tool_calls
                SET approval = ?1
              WHERE provider_call_id = ?2 AND status = 'pending' AND approval = 'pending'",
            rusqlite::params![if approved { "approved" } else { "denied" }, call_id],
        )?;
        let m = crate::mobile::submit_queue::resolve_approval_on(tx, &call_id, approved)?;
        Ok(n + m > 0)
    })
}

/// `payload_json` 坏掉时**当空对象**，绝不连累整个回放（Kotlin 同）。
fn parse_payload(raw: Option<&str>) -> Value {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return json!({});
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Value::Object(map),
        // 非对象（数组 / 标量）也当空：SSE 的 data 必须是个 JSON 对象，客户端按 key 读
        _ => json!({}),
    }
}

/// 按**字符**截断（不是字节）—— 中文工具结果按字节切会切出半个字。
fn truncate(value: &Option<String>, max: usize) -> Option<String> {
    value.as_ref().map(|v| v.chars().take(max).collect())
}

// ---------------------------------------------------------------------------
//  事件归档（2026-11-06）
// ---------------------------------------------------------------------------

/// 一次最多归档多少条。单 Run 的保留窗口是 4000（R5），2 万条 ≈ 5 个 Run ——
/// 一次把几十万条读进内存不值当，剩下的下次再点。
pub const MAX_ARCHIVE_ROWS: i64 = 20_000;

/// 归档文件保留份数（按 mtime 从新到旧）。不设上限的话"归档"只是把库里的涨
/// 换成了磁盘上的涨，等于没解决问题。
pub const KEEP_ARCHIVES: usize = 10;

/// `ai_run_events` 的规模。界面上要能看见"它涨到多大了"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventStats {
    pub rows: i64,
    pub oldest: Option<String>,
    pub newest: Option<String>,
}

pub fn event_stats(state: &AppState) -> Result<EventStats, ApiError> {
    state.with(|conn| {
        conn.query_row(
            "SELECT count(*), min(created_at), max(created_at) FROM ai_run_events",
            [],
            |row| {
                Ok(EventStats {
                    rows: row.get(0)?,
                    oldest: row.get(1)?,
                    newest: row.get(2)?,
                })
            },
        )
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventArchive {
    pub archived: i64,
    pub bytes: u64,
    /// 归档文件（绝对路径）。
    pub file: String,
    /// 截止时间（ISO）；比它早的事件进了这份归档。
    pub cutoff: String,
    /// 还有多少条同样超期且本次未包含（> 0 时提示再次操作）。
    pub remaining: i64,
}

struct ArchiveRow {
    run_id: String,
    seq: i64,
    event_type: String,
    payload: Option<String>,
    created_at: String,
}

/// 把 `days` 天之前的事件写成 JSONL，再从库里删掉。
///
/// 三条纪律：
/// 1. **正在跑的 Run 一条都不动** —— 它还要靠 `?after=seq` 回放（R5）；
/// 2. **先写成文件、再删库**：写文件失败就什么都不删；删库失败就把文件删掉，
///    绝不出现"文件说归档了、库里其实还在"；
/// 3. 每次都只装 [`MAX_ARCHIVE_ROWS`] 条，`remaining` 如实告诉界面还有多少。
pub fn archive_events(
    state: &AppState,
    days: i64,
    dir: &std::path::Path,
) -> Result<EventArchive, ApiError> {
    use std::io::Write;

    let days = days.clamp(1, 3650);
    let modifier = format!("-{days} days");
    let cutoff = crate::infra::iso_time::from_system_time(
        std::time::SystemTime::now() - std::time::Duration::from_secs(days as u64 * 86_400),
    );

    let rows: Vec<ArchiveRow> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT run_id, seq, event_type, payload_json, created_at
               FROM ai_run_events
              WHERE datetime(created_at) < datetime('now', ?1)
                AND run_id NOT IN (SELECT id FROM ai_runs WHERE status = 'running')
              ORDER BY created_at, run_id, seq
              LIMIT ?2",
        )?;
        let mapped = stmt.query_map(rusqlite::params![modifier, MAX_ARCHIVE_ROWS], |row| {
            Ok(ArchiveRow {
                run_id: row.get(0)?,
                seq: row.get(1)?,
                event_type: row.get(2)?,
                payload: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row?);
        }
        Ok(out)
    })?;

    if rows.is_empty() {
        return Ok(EventArchive {
            archived: 0,
            bytes: 0,
            file: String::new(),
            cutoff,
            remaining: 0,
        });
    }

    std::fs::create_dir_all(dir)
        .map_err(|e| ApiError::internal(format!("建归档目录失败（{}）：{e}", dir.display())))?;
    let iso = crate::infra::iso_time::from_system_time(std::time::SystemTime::now());
    let stamp = iso
        .split('.')
        .next()
        .unwrap_or(&iso)
        .trim_end_matches('Z')
        .replace(['-', ':'], "")
        .replace('T', "-");
    // 同一秒里点两次也不许互相覆盖（Windows 的 rename 不覆盖已存在的目标）
    let mut name = format!("run-events-{stamp}.jsonl");
    let mut seq = 0;
    let path = loop {
        let candidate = dir.join(&name);
        if !candidate.exists() {
            break candidate;
        }
        seq += 1;
        if seq > 99 {
            return Err(ApiError::internal("同一秒里归档太多次，换一秒再试"));
        }
        name = format!("run-events-{stamp}-{seq}.jsonl");
    };
    let tmp = dir.join(format!("{name}.part"));

    let mut bytes = 0u64;
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| ApiError::internal(format!("建归档文件失败：{e}")))?;
        for row in &rows {
            // payload 本来就是 JSON 文本，原样嵌进去（不解析再序列化，省一遍也保真）
            let line = format!(
                "{{\"runId\":{},\"seq\":{},\"type\":{},\"payload\":{},\"createdAt\":{}}}",
                json_string(&row.run_id),
                row.seq,
                json_string(&row.event_type),
                row.payload.as_deref().unwrap_or("null"),
                json_string(&row.created_at),
            );
            bytes += line.len() as u64 + 1;
            writeln!(file, "{line}").map_err(|e| ApiError::internal(format!("写归档失败：{e}")))?;
        }
        file.sync_all()
            .map_err(|e| ApiError::internal(format!("归档落盘失败：{e}")))?;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("归档文件改名失败：{e}")));
    }
    prune_archives(dir);

    // 删除**只删这一批的 (run_id, seq)**，不是"按时间再筛一遍"（那会删掉没进文件的）。
    let deleted = match state.write(|tx| {
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _archive_keys (
                 run_id TEXT NOT NULL,
                 seq    INTEGER NOT NULL,
                 PRIMARY KEY (run_id, seq)
             )",
        )?;
        tx.execute("DELETE FROM _archive_keys", [])?;
        {
            let mut ins =
                tx.prepare("INSERT OR IGNORE INTO _archive_keys (run_id, seq) VALUES (?1, ?2)")?;
            for row in &rows {
                ins.execute(rusqlite::params![row.run_id, row.seq])?;
            }
        }
        let n = tx.execute(
            "DELETE FROM ai_run_events
              WHERE (run_id, seq) IN (SELECT run_id, seq FROM _archive_keys)",
            [],
        )?;
        tx.execute("DELETE FROM _archive_keys", [])?;
        Ok(n as i64)
    }) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };

    let remaining: i64 = state.with(|conn| {
        conn.query_row(
            "SELECT count(*) FROM ai_run_events
              WHERE datetime(created_at) < datetime('now', ?1)
                AND run_id NOT IN (SELECT id FROM ai_runs WHERE status = 'running')",
            rusqlite::params![modifier],
            |row| row.get(0),
        )
    })?;

    Ok(EventArchive {
        archived: deleted,
        bytes,
        file: path.to_string_lossy().to_string(),
        cutoff,
        remaining,
    })
}

/// 只留最新的 [`KEEP_ARCHIVES`] 份归档。不设上限的话"归档"只是把库里的涨
/// 换成了磁盘上的涨，等于没解决问题。
///
/// 只识别自行命名的 `run-events-*.jsonl`（该目录中的其他文件不改动）；
/// 删不掉不算错（可能被杀毒软件占着），下次再删。
fn prune_archives(dir: &std::path::Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("run-events-") || !name.ends_with(".jsonl") {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        files.push((mtime, path));
    }
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    for (_, path) in files.into_iter().skip(KEEP_ARCHIVES) {
        let _ = std::fs::remove_file(path);
    }
}

/// 一个字符串 → JSON 字面量（转义交给 serde，别手拼引号）。
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests;
