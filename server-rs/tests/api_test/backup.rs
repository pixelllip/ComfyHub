//! 备份 / 恢复的端点形状与纪律（2026-11-06）。
//!
//! 与 `infra/backup/tests.rs` 的分工：
//! * 那边测"换文件"本身（含崩在中间的恢复）；
//! * 这边测**HTTP 那一层** —— 状态码、字段名、路径穿越、以及"准备恢复**不许动当前库**"。
//!
//! 夹具必须是**文件库**（`infra/backup` 要 `VACUUM INTO` 一个真实文件），
//! 所以这里不用 `common::app()` 的内存库，自己开一个。

use super::common::*;
use serde_json::json;
use std::path::{Path, PathBuf};

use comfyhub_server_rs::infra::backup;
use comfyhub_server_rs::web::routes::AppState;

const TITLE: &str = "恢复前的标题";

fn file_state(label: &str) -> (AppState, PathBuf) {
    let root = comfyhub_server_rs::infra::testpaths::scratch(label);
    let db = root.join(".db").join("comfyhub.db");
    let conn = comfyhub_server_rs::infra::db::open_or_create(&db).expect("建库");
    conn.execute(
        "INSERT INTO prompts (id, title, kind, positive_prompt, favorite, source, created_at, updated_at)
         VALUES (1, ?1, 'IMAGE', 'x', 0, 'Manual', '2026-11-06T00:00:00Z', '2026-11-06T00:00:00Z')",
        rusqlite::params![TITLE],
    )
    .expect("灌一条提示词");
    let media = root.join("storage").join("media");
    std::fs::create_dir_all(&media).expect("建 storage");
    (
        AppState::new(conn, db.to_string_lossy().to_string(), media),
        root,
    )
}

fn title_in(db: &Path) -> String {
    let conn = Connection::open(db).expect("开库");
    conn.query_row("SELECT title FROM prompts WHERE id = 1", [], |r| r.get(0))
        .expect("读标题")
}

fn item_name(body: &serde_json::Value) -> String {
    body["item"]["name"].as_str().expect("item.name").to_string()
}

#[tokio::test]
async fn a_fresh_install_lists_no_backups_but_says_where_they_would_go() {
    let (state, root) = file_state("api-backup-empty");
    let (status, body) = get_json(router_for(state), "/api/system/backups").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["dir", "items", "lastRestore", "pending"]);
    assert!(body["items"].as_array().unwrap().is_empty());
    assert!(body["pending"].is_null());
    assert!(body["lastRestore"].is_null());
    let dir = body["dir"].as_str().unwrap();
    assert!(
        Path::new(dir).ends_with("backups"),
        "备份目录要在运行根目录下：{dir}"
    );
    assert!(Path::new(dir).starts_with(&root), "{dir}");
}

#[tokio::test]
async fn creating_a_backup_writes_a_real_restorable_file() {
    let (state, root) = file_state("api-backup-create");
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/backups",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let name = item_name(&body);
    assert!(name.ends_with(".db"), "{name}");
    assert!(body["item"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(body["item"]["kind"], "auto");

    let path = root.join("backups").join(&name);
    assert!(path.is_file(), "备份文件没落盘：{}", path.display());
    assert_eq!(title_in(&path), TITLE, "备份里必须真有数据");

    let (status, list) = get_json(router_for(state), "/api/system/backups").await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], name.as_str());
    for key in ["name", "bytes", "createdAt", "kind"] {
        assert!(items[0].get(key).is_some(), "缺了 `{key}`：{}", items[0]);
    }
}

#[tokio::test]
async fn restoring_only_prepares_a_restart_and_leaves_the_live_database_alone() {
    let (state, root) = file_state("api-backup-restore");
    let (_, created) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/backups",
        json!({}),
    )
    .await;
    let name = item_name(&created);

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/restore",
        json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["restartRequired"], true, "换库必须等重启");
    assert_eq!(body["name"], name.as_str());
    let safety = body["safety"].as_str().expect("保险备份");
    assert!(safety.starts_with("before-restore-"), "{safety}");

    // 当前库一个字节都没动 —— 真正换文件在下次启动（`db::open_or_create` 里）
    let db = PathBuf::from(&state.db_path);
    assert_eq!(title_in(&db), TITLE);
    assert_eq!(backup::pending(&db).as_deref(), Some(name.as_str()));
    assert!(root.join("backups").join(safety).is_file());
    assert!(PathBuf::from(&state.db_path).with_extension("db.restore-tmp").is_file());
}

#[tokio::test]
async fn restore_rejects_path_traversal_and_unknown_backups() {
    let (state, _root) = file_state("api-backup-bad");
    for (name, must_say) in [
        ("../comfyhub.db", "不合法"),
        ("sub/dir.db", "不合法"),
        ("comfyhub-nope.db", "没有这份备份"),
    ] {
        let (status, body) = send_json(
            router_for(state.clone()),
            "POST",
            "/api/system/restore",
            json!({ "name": name }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name} → {body}");
        let text = body["error"].as_str().unwrap_or_default();
        assert!(text.contains(must_say), "`{name}` 的报错要说人话：{body}");
    }
}

#[tokio::test]
async fn deleting_a_backup_is_honest_about_missing_ones() {
    let (state, root) = file_state("api-backup-delete");
    let (_, created) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/system/backups",
        json!({}),
    )
    .await;
    let name = item_name(&created);

    let (status, body) = send_empty(
        router_for(state.clone()),
        "DELETE",
        &format!("/api/system/backups/{name}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], name.as_str());
    assert!(!root.join("backups").join(&name).exists());

    let (status, body) = send_empty(
        router_for(state),
        "DELETE",
        &format!("/api/system/backups/{name}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "删第二遍要如实说没有：{body}");
}
