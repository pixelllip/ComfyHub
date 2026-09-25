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

use crate::iso_time::iso_time;
use crate::routes::{ApiError, AppState};

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
    /// 本次 Run **实际生效**的思考强度（`off` = 不支持或用户关掉了，AIH-056）
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
    let id = crate::conversations::uuid_v4()?;
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
        return Err(ApiError::ai("CONFIG_ERROR", format!("非法的 Run 终态: {status}")));
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
pub fn fail_stale_running(state: &AppState, reason: &str) -> Result<usize, ApiError> {
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
pub fn append_event(state: &AppState, run_id: &str, seq: i64, event_type: &str, payload: &Value) -> Result<(), ApiError> {
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
        self.pending.push((seq, event_type.to_string(), payload.clone()));
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
                    payload: parse_payload(row.get::<_, Option<String>>("payload_json")?.as_deref()),
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
        tx.execute("DELETE FROM ai_message_parts WHERE message_id = ?1", rusqlite::params![message_id])?;
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
    let id = crate::conversations::uuid_v4()?;
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
    let id = crate::conversations::uuid_v4()?;
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
        let mut stmt = conn.prepare(
            "SELECT * FROM ai_tool_calls WHERE run_id = ?1 ORDER BY started_at, id",
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 事件帧逐字节对齐 Kotlin `RunEvent.toSse()`。
    #[test]
    fn sse_frame_matches_kotlin() {
        let e = StoredEvent {
            seq: 7,
            r#type: "text.delta".into(),
            payload: json!({"text": "你好"}),
        };
        assert_eq!(e.to_sse(), "id: 7\nevent: text.delta\ndata: {\"text\":\"你好\"}\n\n");
    }

    /// 坏 / 空的 `payload_json` → **空对象**（不是 `null`、不抛）。
    #[test]
    fn bad_payload_becomes_empty_object() {
        assert_eq!(parse_payload(None), json!({}));
        assert_eq!(parse_payload(Some("   ")), json!({}));
        assert_eq!(parse_payload(Some("not json")), json!({}));
        assert_eq!(parse_payload(Some("[1,2]")), json!({}), "数组不是事件 payload");
        assert_eq!(parse_payload(Some(r#"{"a":1}"#)), json!({"a": 1}));
    }

    /// 截断按**字符**：中文结果按字节切会切出半个字（界面显示成乱码方块）。
    #[test]
    fn truncate_counts_chars_not_bytes() {
        let long = "汉".repeat(TOOL_CONTENT_MAX + 5);
        let out = truncate(&Some(long), TOOL_CONTENT_MAX).unwrap();
        assert_eq!(out.chars().count(), TOOL_CONTENT_MAX);
        assert!(out.chars().all(|c| c == '汉'), "不能切出半个字");
        assert_eq!(truncate(&None, 10), None);
    }

    /// 终态只认三个：`running` 不是终态，写它当场抛（否则僵死 Run 收不走）。
    #[test]
    fn terminal_statuses_are_exactly_three() {
        assert_eq!(TERMINAL, ["completed", "failed", "cancelled"]);
        assert!(!TERMINAL.contains(&RUNNING));
    }

    /// 可空列在 DTO 上的回落：真 schema 上构造不出的 NULL（老 MySQL 脏行才走得到）。
    #[test]
    fn null_columns_fall_back_to_missing_keys() {
        let conn = crate::db::open_memory().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT 'r1' AS id, 'c1' AS conversation_id, NULL AS status, NULL AS provider_id,
                        NULL AS model_id, NULL AS assistant_message_id, NULL AS user_message_id,
                        NULL AS error_code, NULL AS error_message, NULL AS prompt_version,
                        NULL AS reasoning_effort, NULL AS retry_of_run_id, NULL AS started_at,
                        NULL AS completed_at, NULL AS created_at",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let run = run_from_row(rows.next().unwrap().unwrap()).unwrap();
        assert_eq!(run.status, "", "status NULL → 空串（Kotlin getString 也给空串）");
        assert!(run.provider_id.is_none() && run.completed_at.is_none());
        // 序列化之后**键整个不出现**（explicitNulls = false）
        let value = serde_json::to_value(&run).unwrap();
        let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort_unstable(); // 键序由 serde_json 的 Map 决定，不承重；这里只钉"哪些键在"
        assert_eq!(keys, vec!["conversationId", "id", "status"], "键集合：只有三个非空键");
    }
}
