//! `events` 的单元测试（主体代码在 [`events`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use serde_json::json;

fn test_state() -> AppState {
    let conn = crate::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("db").join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.execute_batch(
        "INSERT INTO ai_conversations (id, title) VALUES ('c1', 't');
         INSERT INTO ai_runs (id, conversation_id, status) VALUES ('r1', 'c1', 'running');",
    )
    .unwrap();
    AppState::new(conn, ":memory:", crate::testpaths::scratch("media"))
}

/// `seq` 单调递增、绝不回退（**红线 R5 的核心**）。
#[test]
fn seq_is_monotonic_and_never_reused() {
    let state = test_state();
    let bus = RunEventBus::new();
    bus.open("r1");
    for expected in 1..=5 {
        let event = bus.emit(&state, "r1", event_type::TEXT_DELTA, &json!({"text": "x"})).unwrap().unwrap();
        assert_eq!(event.seq, expected);
    }
    bus.flush(&state, "r1").unwrap();
    // 收尾之后再发：**返回 None**，且号不倒退
    bus.close(&state, "r1").unwrap();
    assert!(bus.emit(&state, "r1", event_type::TEXT_DELTA, &json!({})).unwrap().is_none());
    assert!(bus.is_closed("r1"));
    assert_eq!(bus.next_seq("r1"), 6, "号停在原处，不许回退");
}

/// 崩溃恢复：库里已有 `max(seq)=9` 时接管，**从 10 开始**。
///
/// 不这么做的后果很具体：号从 1 重来 → `INSERT OR IGNORE` 把新事件全吞掉
/// （主键 `(run_id, 1)` 已存在）→ 界面一个字都不长，而两端都不报错。
#[test]
fn resume_from_continues_after_the_persisted_max_seq() {
    let state = test_state();
    let bus = RunEventBus::new();
    for seq in 1..=9 {
        runs::append_event(&state, "r1", seq, event_type::TEXT_DELTA, &json!({"n": seq})).unwrap();
    }
    let max = max_seq(&state, "r1").unwrap();
    assert_eq!(max, 9);
    bus.resume_from("r1", max);
    let event = bus.emit(&state, "r1", event_type::RUN_COMPLETED, &json!({})).unwrap().unwrap();
    assert_eq!(event.seq, 10);
    bus.flush(&state, "r1").unwrap();
    let stored = runs::events(&state, "r1", 0).unwrap();
    assert_eq!(stored.len(), 10, "10 条都在（第 10 条没被主键冲突吞掉）");
    assert_eq!(stored[9].seq, 10);
}

/// 环形缓冲只丢**最老的**，且**只影响内存**（库里那条照样在）。
#[test]
fn ring_buffer_drops_the_oldest_only() {
    let state = test_state();
    let bus = RunEventBus::new();
    bus.open("r1");
    for seq in 1..=(MEMORY_KEEP as i64 + 10) {
        bus.emit(&state, "r1", event_type::TEXT_DELTA, &json!({"n": seq})).unwrap();
    }
    let history = bus.history("r1");
    assert_eq!(history.len(), MEMORY_KEEP);
    assert_eq!(history[0].seq, 11, "丢的是最老的 10 条");
    assert_eq!(history.last().unwrap().seq, MEMORY_KEEP as i64 + 10);
    bus.flush(&state, "r1").unwrap();
    assert_eq!(
        runs::events(&state, "r1", 0).unwrap().len(),
        MEMORY_KEEP + 10,
        "库里一条都不少（内存的环形上限与库里的保留窗口是两件事）"
    );
}

/// 收尾必须 flush：**最后那不足一批的事件不能丢**。
#[test]
fn close_flushes_the_partial_batch() {
    let state = test_state();
    let bus = RunEventBus::new();
    bus.open("r1");
    // 故意发得比一批少：只有 flush/close 会把它写下去
    bus.emit(&state, "r1", event_type::RUN_STARTED, &json!({})).unwrap();
    bus.emit(&state, "r1", event_type::RUN_COMPLETED, &json!({})).unwrap();
    assert!(runs::events(&state, "r1", 0).unwrap().is_empty(), "还没到一批，库里是空的");
    bus.close(&state, "r1").unwrap();
    let stored = runs::events(&state, "r1", 0).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[1].r#type, event_type::RUN_COMPLETED, "终态事件落库了（R5）");
}

/// 18 个事件类型与契约文档逐字一致（两边各一份常量，靠这条比对）。
///
/// 字符串**逐字写死**（不是从常量反推）：契约是 `docs/run-event-contract.md`，
/// 拼错一个字母的后果是"客户端一个事件都匹配不上、界面什么都不动"，而两端都不报错。
/// 唯一的无点例外是 `heartbeat`（它本来就不是 `域.动作` 形状）。
#[test]
fn event_types_match_the_contract() {
    assert_eq!(
        event_type::ALL,
        [
            "run.started",
            "message.started",
            "reasoning.delta",
            "text.delta",
            "step.started",
            "tool.requested",
            "tool.started",
            "tool.completed",
            "tool.failed",
            "user.question",
            "user.question.answered",
            "usage.updated",
            "message.completed",
            "conversation.updated",
            "run.completed",
            "run.failed",
            "run.cancelled",
            "heartbeat",
        ]
    );
    let mut sorted = event_type::ALL.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 18, "不许有重复");
    assert_eq!(TERMINAL_EVENTS, ["run.completed", "run.failed", "run.cancelled"]);
    for name in event_type::ALL {
        assert!(!name.is_empty());
        assert!(name.contains('.') || name == event_type::HEARTBEAT, "只有 heartbeat 没有点：{name}");
    }
}
