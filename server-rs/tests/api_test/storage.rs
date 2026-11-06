//! 存储占用 / 孤儿清理 / 事件归档的端点（2026-11-06）。
//!
//! 与 `infra/storage/tests.rs` 的分工：那边测扫描与删除的判据，这边测
//! **HTTP 形状、`apply` 的默认值、以及"归档文件真的写出来了吗"**。

use super::common::*;
use serde_json::json;
use std::time::{Duration, SystemTime};

fn old_file(path: &std::path::Path) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(
        std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(48 * 3600)),
    )
    .unwrap();
}

#[tokio::test]
async fn storage_usage_lists_every_known_directory() {
    let state = state();
    let media = state.media_dir().to_path_buf();
    std::fs::write(media.join("big.png"), vec![0u8; 1024]).unwrap();

    let (status, body) = get_json(router_for(state), "/api/system/storage").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(keys(&body), vec!["dirs", "events", "root", "totalBytes"]);
    assert!(body["totalBytes"].as_u64().unwrap() > 0);

    let dirs = body["dirs"].as_array().unwrap();
    let find = |key: &str| {
        dirs.iter()
            .find(|d| d["key"] == key)
            .unwrap_or_else(|| panic!("缺了 {key}：{body}"))
    };
    for key in [
        "db",
        "backups",
        "media",
        "thumbs",
        "aiAttachments",
        "aiThumbs",
        "aiData",
        "run",
    ] {
        let dir = find(key);
        for field in ["key", "label", "path", "bytes", "files"] {
            assert!(dir.get(field).is_some(), "{key} 缺了 {field}：{dir}");
        }
    }
    assert!(
        find("media").as_object().unwrap()["bytes"]
            .as_u64()
            .unwrap()
            >= 1024
    );
    assert_eq!(body["events"]["rows"], 0);
}

#[tokio::test]
async fn orphan_scan_is_the_default_and_deleting_is_explicit() {
    let state = state();
    let media = state.media_dir().to_path_buf();
    // a.png 在夹具里有对应的 media_assets 行；这个没有
    let orphan = media.join("orphan.png");
    std::fs::write(&orphan, vec![7u8; 2048]).unwrap();
    old_file(&orphan);

    let (status, scan) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/storage/orphans",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scan}");
    assert_eq!(scan["count"], 1, "缺省只扫描：{scan}");
    assert_eq!(scan["bytes"], 2048);
    assert_eq!(scan["truncated"], false);
    assert_eq!(scan["items"][0]["name"], "orphan.png");
    assert!(orphan.exists(), "扫描不许删文件");

    let (status, report) = send_json(
        router_for(state),
        "POST",
        "/api/system/storage/orphans",
        json!({ "apply": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["deleted"], 1, "{report}");
    assert_eq!(report["bytes"], 2048);
    assert!(!orphan.exists());
    assert!(media.join("a.png").exists(), "库里有记录的不许删");
}

#[tokio::test]
async fn archiving_writes_a_jsonl_file_and_keeps_running_runs() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_runs (id, conversation_id, status)
                 VALUES ('run-old', 'conv-0', 'completed'), ('run-live', 'conv-0', 'running')",
                [],
            )?;
            conn.execute(
                "INSERT INTO ai_run_events (run_id, seq, event_type, payload_json, created_at)
                 VALUES ('run-old',  1, 'text.delta', '{\"text\":\"a\"}', '2026-01-01T00:00:00Z'),
                        ('run-old',  2, 'text.delta', '{\"text\":\"b\"}', '2026-01-01T00:00:01Z'),
                        ('run-live', 1, 'text.delta', '{\"text\":\"live\"}', '2026-01-01T00:00:00Z'),
                        ('run-old',  3, 'text.delta', '{\"text\":\"fresh\"}', '2099-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/events/archive",
        json!({ "days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let report = &body["report"];
    assert_eq!(report["archived"], 2, "{report}");
    assert!(report["bytes"].as_u64().unwrap() > 0);
    assert_eq!(report["remaining"], 0, "{report}");

    let file = report["file"].as_str().unwrap();
    let path = std::path::Path::new(file);
    assert!(path.is_file(), "归档文件没写出来：{file}");
    let text = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    let last: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(last["runId"], "run-old");
    assert_eq!(last["type"], "text.delta");
    assert_eq!(last["payload"]["text"], "b");

    // 库里只剩"新的"和"正在跑的"
    let left: i64 = state
        .with(|conn| conn.query_row("SELECT count(*) FROM ai_run_events", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(left, 2);
}

#[tokio::test]
async fn archiving_nothing_is_honest() {
    let state = state();
    let (status, body) = send_json(
        router_for(state),
        "POST",
        "/api/system/events/archive",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["archived"], 0);
    assert_eq!(body["report"]["file"], "");
}

#[tokio::test]
async fn storage_endpoints_reject_unknown_paths() {
    let (status, _) = get_json(app(), "/api/system/storage/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
