//! 运行中的 Run 事件总线（P4 第 27 步）—— 逐条对照 Kotlin `RunEventBus`。
//!
//! # 总线负责什么、不负责什么
//!
//! **负责**：给一次 Run 发**单调递增的 `seq`**（契约 §2：断线重连一律 `?after=<seq>`）
//! 与写穿（push-through）到 `ai_run_events`。
//!
//! **不负责**：把事件推给 SSE 连接。这是**刻意的**——Kotlin 用的是内存里的订阅通道，
//! 而那种做法有一条缝：内存缓冲只留最近 [MEMORY_KEEP] 条，`after` 比它更老时
//! **取不到**那段历史（只能靠库里那份，但库里那份是批量的、可能还没 flush）。
//! 本实现让 SSE 直接**从库里拉**（`?after=`），于是：
//!
//! * 事件顺序、去重、断线续传全都由 `(run_id, seq)` 主键保证；
//! * "批写还没 flush 的那几条"由 [`RunEventBus::flush`] 在订阅前补上；
//! * 后端重启后**什么都不用改**（库里就是全部）。
//!
//! # 三条纪律
//!
//! 1. **`seq` 在一次 Run 内单调递增，绝不回退、绝不复用**。环形的丢弃只影响"回放不到那么老"，
//!    而 `seq` 复用会让客户端游标错位 —— 那才是**真丢数据**（红线 R5 的反面）。
//! 2. **收尾必须 `flush`**（正常 / 失败 / 取消三条路都要）：否则最后那不足一批的
//!    `run.completed` / `run.failed` 会丢，断线重连就补不回来了。
//! 3. **`close` 之后再 emit 一律返回 `None`**（不静默往一个已经收尾的 Run 里塞事件）。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde_json::Value;

use crate::ai::runs::{self, StoredEvent, EVENT_BATCH};
use crate::web::routes::{ApiError, AppState};

/// 内存里每个 Run 最多留多少条（Kotlin `RunEventBus.MEMORY_KEEP`）。
///
/// 与库里 [`runs::EVENT_KEEP`]（4000）**刻意不同数**：前者管"进程活着时能回看多少"，
/// 后者管"后端重启后能回放多少"。2000 的依据：逐字流式的 `text.delta` 一分钟就上万条，
/// 全量留实测单 Run 4.7 万条 —— 那是纯浪费的常驻内存。
pub const MEMORY_KEEP: usize = 2000;

/// 一次 Run 的内存态。
struct Stream {
    /// 环形缓冲：只留最近 [`MEMORY_KEEP`] 条（`VecDeque` 的 `pop_front` 是 O(1)）
    events: VecDeque<StoredEvent>,
    /// 下一个要发的号（**只增不减**）
    next_seq: i64,
    closed: bool,
    /// 这个 Run 的批写器（`close` 时 flush 掉）
    batcher: EventBatcherOwned,
}

/// 批写器的持有形态：`EventBatcher` 借了 `&AppState`，而总线要活得比一次请求长，
/// 所以这里存"参数"，每次 flush 现造一个（写一个批次就完事，没有跨 await 的借用）。
struct EventBatcherOwned {
    batch: Vec<(i64, String, Value)>,
    size: usize,
}

impl EventBatcherOwned {
    fn new(size: usize) -> Self {
        Self {
            batch: Vec::new(),
            size: size.max(1),
        }
    }
}

/// Run 事件总线（`AppState.bus` 里的那一份）。
pub struct RunEventBus {
    streams: Mutex<HashMap<String, Stream>>,
}

impl Default for RunEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl RunEventBus {
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// 开一次 Run 的流（`POST /runs` 时调；重复 open 是幂等的，**不清空已有的 seq**）。
    ///
    /// 为什么重复 open 不能重置：Kotlin 的 `bus.open()` 是"新开一个号从 1 开始"，
    /// 而契约 §2 说 `seq` 在**一次 Run 内**单调递增 —— 一次 Run 只有一次 open。
    /// 这里做成幂等是为了让"重启后进程里没有流、但库里有事件"那条路不把号退回去。
    pub fn open(&self, run_id: &str) {
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams.entry(run_id.to_string()).or_insert_with(|| Stream {
            events: VecDeque::new(),
            next_seq: 1,
            closed: false,
            batcher: EventBatcherOwned::new(EVENT_BATCH),
        });
    }

    /// 发一个事件：分配 `seq` → 进环形缓冲 → 攒进批写队列（满一批就落库）。
    ///
    /// 返回落地的 [`StoredEvent`]；Run 不在流里或已经收尾 → `None`（**不报错**：
    /// 流没收尾时 emit 是编程错误，但把整轮 Run 弄崩更糟 —— 调用方按 `None` 记一笔日志）。
    pub fn emit(
        &self,
        state: &AppState,
        run_id: &str,
        event_type: &str,
        payload: &Value,
    ) -> Result<Option<StoredEvent>, ApiError> {
        let mut to_write: Option<Vec<(i64, String, Value)>> = None;
        let event = {
            let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
            let Some(stream) = streams.get_mut(run_id) else {
                return Ok(None);
            };
            if stream.closed {
                return Ok(None);
            }
            let event = StoredEvent {
                seq: stream.next_seq,
                r#type: event_type.to_string(),
                payload: payload.clone(),
            };
            stream.next_seq += 1;
            stream.events.push_back(event.clone());
            while stream.events.len() > MEMORY_KEEP {
                stream.events.pop_front();
            }
            stream
                .batcher
                .batch
                .push((event.seq, event.r#type.clone(), payload.clone()));
            if stream.batcher.batch.len() >= stream.batcher.size {
                to_write = Some(std::mem::take(&mut stream.batcher.batch));
            }
            event
        };
        if let Some(batch) = to_write {
            write_batch(state, run_id, &batch)?;
        }
        Ok(Some(event))
    }

    /// 把攒着的事件落库（**收尾与订阅前必须调**）。
    ///
    /// 返回写出条数。收尾那三条路（正常 / 失败 / 取消）都要调 —— 漏了就是
    /// "最后那条 `run.completed` 没落库 → 断线重连补不回来"。
    pub fn flush(&self, state: &AppState, run_id: &str) -> Result<usize, ApiError> {
        let batch = {
            let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
            let Some(stream) = streams.get_mut(run_id) else {
                return Ok(0);
            };
            std::mem::take(&mut stream.batcher.batch)
        };
        if batch.is_empty() {
            return Ok(0);
        }
        write_batch(state, run_id, &batch)?;
        Ok(batch.len())
    }

    /// 收尾：标记关闭并 flush 剩下的（之后 emit 一律 `None`）。
    pub fn close(&self, state: &AppState, run_id: &str) -> Result<(), ApiError> {
        {
            let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(stream) = streams.get_mut(run_id) {
                stream.closed = true;
            }
        }
        self.flush(state, run_id)?;
        Ok(())
    }

    /// 内存里的最近事件（排查用；**回放一律从库里读**）。
    pub fn history(&self, run_id: &str) -> Vec<StoredEvent> {
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams
            .get(run_id)
            .map(|s| s.events.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 流是否已经收尾（不在流里也算收尾 —— "进程不知道这次 Run"）。
    pub fn is_closed(&self, run_id: &str) -> bool {
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams.get(run_id).map(|s| s.closed).unwrap_or(true)
    }

    /// 下一个要用的号（**崩溃恢复**：库里已有事件时要把号接上去，不能从 1 重来）。
    pub fn next_seq(&self, run_id: &str) -> i64 {
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams.get(run_id).map(|s| s.next_seq).unwrap_or(1)
    }

    /// 后端重启 / 断线后重新接管一个**库里已经有事件**的 Run：把号接到 `max(seq)+1`。
    ///
    /// 不做这一步的后果很具体：`seq` 从 1 重来 → `INSERT OR IGNORE` 把新事件全吞掉
    /// （主键 `(run_id, 1)` 已存在）→ **界面上一句话都不长**，而两端都不报错。
    pub fn resume_from(&self, run_id: &str, max_seq: i64) {
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        let stream = streams.entry(run_id.to_string()).or_insert_with(|| Stream {
            events: VecDeque::new(),
            next_seq: 1,
            closed: false,
            batcher: EventBatcherOwned::new(EVENT_BATCH),
        });
        if stream.next_seq <= max_seq {
            stream.next_seq = max_seq + 1;
        }
        stream.closed = false;
    }
}

/// 一批事件落库（**一个事务**，R2 的 `BEGIN IMMEDIATE` 由 `state.write` 保证）。
fn write_batch(
    state: &AppState,
    run_id: &str,
    batch: &[(i64, String, Value)],
) -> Result<(), ApiError> {
    let run_id = run_id.to_string();
    let batch: Vec<(i64, String, String)> = batch
        .iter()
        .map(|(seq, t, payload)| (*seq, t.clone(), payload.to_string()))
        .collect();
    state.write(move |tx| {
        for (seq, event_type, payload_json) in &batch {
            tx.execute(
                "INSERT OR IGNORE INTO ai_run_events (run_id, seq, event_type, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![run_id, seq, event_type, payload_json],
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

/// 一次 Run 的终态事件类型（SSE 收到它们就该收尾了，契约 §2）。
pub const TERMINAL_EVENTS: [&str; 3] = ["run.completed", "run.failed", "run.cancelled"];

/// 事件类型常量（与 `docs/run-event-contract.md` 和 Kotlin `RunEventType` **逐字一致**）。
pub mod event_type {
    pub const RUN_STARTED: &str = "run.started";
    pub const MESSAGE_STARTED: &str = "message.started";
    pub const REASONING_DELTA: &str = "reasoning.delta";
    pub const TEXT_DELTA: &str = "text.delta";
    /// 关键步骤（环节）开始：`[步骤]正在查资料[/步骤]` 摘出的名称。
    /// 界面靠它**当场**画出环节头，把这个环节的工具卡折进一组。
    pub const STEP_STARTED: &str = "step.started";
    pub const TOOL_REQUESTED: &str = "tool.requested";
    pub const TOOL_STARTED: &str = "tool.started";
    pub const TOOL_COMPLETED: &str = "tool.completed";
    pub const TOOL_FAILED: &str = "tool.failed";
    pub const USER_QUESTION: &str = "user.question";
    pub const USER_QUESTION_ANSWERED: &str = "user.question.answered";
    pub const USAGE_UPDATED: &str = "usage.updated";
    pub const MESSAGE_COMPLETED: &str = "message.completed";
    pub const CONVERSATION_UPDATED: &str = "conversation.updated";
    pub const RUN_COMPLETED: &str = "run.completed";
    pub const RUN_FAILED: &str = "run.failed";
    pub const RUN_CANCELLED: &str = "run.cancelled";
    pub const HEARTBEAT: &str = "heartbeat";

    /// 18 个类型（**顺序即契约文档的顺序**，两边逐字比对靠它）。
    pub const ALL: [&str; 18] = [
        RUN_STARTED,
        MESSAGE_STARTED,
        REASONING_DELTA,
        TEXT_DELTA,
        STEP_STARTED,
        TOOL_REQUESTED,
        TOOL_STARTED,
        TOOL_COMPLETED,
        TOOL_FAILED,
        USER_QUESTION,
        USER_QUESTION_ANSWERED,
        USAGE_UPDATED,
        MESSAGE_COMPLETED,
        CONVERSATION_UPDATED,
        RUN_COMPLETED,
        RUN_FAILED,
        RUN_CANCELLED,
        HEARTBEAT,
    ];
}

/// 从库里取号：接上 `max(seq)`（崩溃恢复用）。
pub fn max_seq(state: &AppState, run_id: &str) -> Result<i64, ApiError> {
    state.with(|conn| {
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM ai_run_events WHERE run_id = ?1",
            rusqlite::params![run_id],
            |row| row.get(0),
        )
    })
}

/// 把事件落库（`runs::append_event` 的别名，给不经过总线的调用方用）。
pub fn persist(state: &AppState, event: &StoredEvent, run_id: &str) -> Result<(), ApiError> {
    runs::append_event(state, run_id, event.seq, &event.r#type, &event.payload)
}

#[cfg(test)]
mod tests;
