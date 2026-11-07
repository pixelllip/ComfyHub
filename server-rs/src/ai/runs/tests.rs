//! `runs` 的单元测试（主体代码在 [`runs`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// 事件帧逐字节对齐 Kotlin `RunEvent.toSse()`。
#[test]
fn sse_frame_matches_kotlin() {
    let e = StoredEvent {
        seq: 7,
        r#type: "text.delta".into(),
        payload: json!({"text": "你好"}),
    };
    assert_eq!(
        e.to_sse(),
        "id: 7\nevent: text.delta\ndata: {\"text\":\"你好\"}\n\n"
    );
}

/// 坏 / 空的 `payload_json` → **空对象**（不是 `null`、不抛）。
#[test]
fn bad_payload_becomes_empty_object() {
    assert_eq!(parse_payload(None), json!({}));
    assert_eq!(parse_payload(Some("   ")), json!({}));
    assert_eq!(parse_payload(Some("not json")), json!({}));
    assert_eq!(
        parse_payload(Some("[1,2]")),
        json!({}),
        "数组不是事件 payload"
    );
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
    let conn = crate::infra::db::open_memory().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT 'r1' AS id, 'c1' AS conversation_id, NULL AS status, NULL AS provider_id,
                    NULL AS model_id, NULL AS assistant_message_id, NULL AS user_message_id,
                    NULL AS error_code, NULL AS error_message, NULL AS prompt_version,
                    NULL AS reasoning_effort, NULL AS retry_of_run_id, NULL AS started_at,
                    NULL AS completed_at, NULL AS created_at, NULL AS skill_snapshot",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let run = run_from_row(rows.next().unwrap().unwrap()).unwrap();
    assert_eq!(
        run.status, "",
        "status NULL → 空串（Kotlin getString 也给空串）"
    );
    assert!(run.provider_id.is_none() && run.completed_at.is_none());
    // 序列化之后**键整个不出现**（explicitNulls = false）
    let value = serde_json::to_value(&run).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    keys.sort_unstable(); // 键序由 serde_json 的 Map 决定，不承重；这里只钉"哪些键在"
    assert_eq!(
        keys,
        vec!["conversationId", "id", "status"],
        "键集合：只有三个非空键"
    );
}
