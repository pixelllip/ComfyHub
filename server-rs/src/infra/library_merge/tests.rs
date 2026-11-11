//! `library_merge` 的用例（主体在 [`library_merge`](super)）。
//!
//! 四条要钉住的事实：
//! 1. **东西真的搬过来了**（提示词 / 产物 / 角色卡 / 会话整条链），而且文件跟着搬；
//! 2. **同号不同物时改号**、引用跟着改（`tags.id = 1` 两边都叫 1 的那种）；
//! 3. **同一份库合两次不会翻倍**（判重靠内容，不靠 id）；
//! 4. **演练模式一个字节都不写**。

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::infra::{db, testpaths};

use super::*;

fn sha_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 造一个"数据目录"：`.db\comfyhub.db`（空的）+ 四个 storage 子目录。
fn make_lib(tag: &str) -> PathBuf {
    let root = testpaths::scratch(&format!("merge-{tag}"));
    std::fs::create_dir_all(root.join(".db")).unwrap();
    for sub in ["media", "thumbs", "ai-attachments", "ai-thumbs"] {
        std::fs::create_dir_all(root.join("storage").join(sub)).unwrap();
    }
    root
}

fn open_with_schema(path: &Path) -> Connection {
    let conn = db::open(path).unwrap();
    conn.execute_batch(db::schema_sql()).unwrap();
    conn
}

fn db_of(dir: &Path) -> PathBuf {
    dir.join(".db").join("comfyhub.db")
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn request(target: &Path, source: &Path, media_from: &[PathBuf], dry_run: bool) -> MergeRequest {
    MergeRequest {
        target_db: db_of(target),
        target_storage: target.join("storage"),
        sources: vec![source.to_path_buf()],
        media_from: media_from.to_vec(),
        backup_dir: Some(target.join("merge-backups")),
        dry_run,
    }
}

fn write_file(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn 合并把提示词产物角色卡和文件都搬过来_同号不同物时改号() {
    let target = make_lib("target-a");
    let source = make_lib("source-a");
    let tconn = open_with_schema(&db_of(&target));
    tconn
        .execute_batch(
            "INSERT INTO tags (id, name, normalized) VALUES (1, 'ComfyUI', 'comfyui');
             INSERT INTO media_assets (id, kind, original_name, stored_name, sha256)
               VALUES (1, 'IMAGE', 'live.png', 'live.png', 'aa');",
        )
        .unwrap();
    // 目标库里已有的那份文件（不该被覆盖）
    write_file(
        &target.join("storage").join("media").join("live.png"),
        b"live",
    );

    let sconn = open_with_schema(&db_of(&source));
    let payload = b"old-image";
    sconn
        .execute_batch(&format!(
            "INSERT INTO tags (id, name, normalized) VALUES (1, 'ComfyUI', 'comfyui');
             INSERT INTO prompts (id, positive_prompt) VALUES (7, 'a girl');
             INSERT INTO prompt_tags (prompt_id, tag_id) VALUES (7, 1);
             INSERT INTO media_assets (id, prompt_id, kind, original_name, stored_name, sha256, size_bytes)
               VALUES (1, 7, 'IMAGE', 'a.png', 'a.png', '{}', {});
             INSERT INTO character_cards (id, name, normalized)
               VALUES (1, 'A Girl', 'a girl');
             INSERT INTO character_card_examples (card_id, media_id) VALUES (1, 1);",
            sha_hex(payload),
            payload.len()
        ))
        .unwrap();
    write_file(&source.join("storage").join("media").join("a.png"), payload);
    write_file(
        &source.join("storage").join("thumbs").join("1.jpg"),
        b"thumb",
    );

    let report = merge(&request(&target, &source, &[], false)).unwrap();

    // 标签：同名同 normalized → 复用目标那一行，不重复
    assert_eq!(count(&tconn, "tags"), 1);
    // 提示词搬过来了
    assert_eq!(count(&tconn, "prompts"), 1);
    // 产物：目标库里 id 1 被占了 → 换成 2（引用跟着改）
    assert_eq!(count(&tconn, "media_assets"), 2);
    let (id, stored): (i64, String) = tconn
        .query_row(
            "SELECT id, stored_name FROM media_assets WHERE original_name = 'a.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(id, 2);
    assert_eq!(stored, "a.png");
    // 例图关系跟着改
    let example_media: i64 = tconn
        .query_row("SELECT media_id FROM character_card_examples", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(example_media, 2);
    // 提示词与标签的关系仍在
    assert_eq!(count(&tconn, "prompt_tags"), 1);
    // 文件：原件按名字补、缩略图按**新的** id 改名
    assert_eq!(
        std::fs::read(target.join("storage").join("media").join("a.png")).unwrap(),
        payload
    );
    assert!(
        target
            .join("storage")
            .join("thumbs")
            .join("2.jpg")
            .is_file(),
        "缩略图要跟着新 id 改名"
    );
    // 目标库原有的文件一个字节没动
    assert_eq!(
        std::fs::read(target.join("storage").join("media").join("live.png")).unwrap(),
        b"live"
    );
    // 外键自洽 + 备份在
    assert!(super::foreign_key_violations(&tconn).unwrap().is_empty());
    assert!(
        report.backup.as_ref().is_some_and(|p| p.is_file()),
        "合并前必须留一份备份"
    );
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.tables["media_assets"].renumbered, 1);
}

#[test]
fn 同一份库合两次不会翻倍() {
    let target = make_lib("target-b");
    let source = make_lib("source-b");
    let tconn = open_with_schema(&db_of(&target));
    let sconn = open_with_schema(&db_of(&source));
    let payload = b"pic";
    let hash = sha_hex(payload);
    sconn
        .execute_batch(&format!(
            "INSERT INTO prompts (id, positive_prompt, source, source_ref)
               VALUES (3, 'x', 'ComfyUI', 'ref-1');
             INSERT INTO media_assets (id, kind, original_name, stored_name, sha256, size_bytes)
               VALUES (5, 'IMAGE', 'b.png', 'b.png', '{hash}', {});
             INSERT INTO ai_conversations (id, title) VALUES ('c1', 'hello');
             INSERT INTO ai_messages (id, conversation_id, role) VALUES ('m1', 'c1', 'user');",
            payload.len()
        ))
        .unwrap();
    write_file(&source.join("storage").join("media").join("b.png"), payload);

    merge(&request(&target, &source, &[], false)).unwrap();
    let after_first = (
        count(&tconn, "prompts"),
        count(&tconn, "media_assets"),
        count(&tconn, "ai_conversations"),
        count(&tconn, "ai_messages"),
    );
    merge(&request(&target, &source, &[], false)).unwrap();
    let after_second = (
        count(&tconn, "prompts"),
        count(&tconn, "media_assets"),
        count(&tconn, "ai_conversations"),
        count(&tconn, "ai_messages"),
    );
    assert_eq!(after_first, (1, 1, 1, 1));
    assert_eq!(
        after_first, after_second,
        "同一份源库合两次必须还是同一份内容"
    );
}

#[test]
fn 演练模式一个字节都不写() {
    let target = make_lib("target-c");
    let source = make_lib("source-c");
    let tconn = open_with_schema(&db_of(&target));
    let sconn = open_with_schema(&db_of(&source));
    let payload = b"dry";
    sconn
        .execute_batch(&format!(
            "INSERT INTO prompts (id, positive_prompt) VALUES (9, 'y');
             INSERT INTO media_assets (id, kind, original_name, stored_name, sha256, size_bytes)
               VALUES (9, 'IMAGE', 'c.png', 'c.png', '{}', {});",
            sha_hex(payload),
            payload.len()
        ))
        .unwrap();
    write_file(&source.join("storage").join("media").join("c.png"), payload);

    let report = merge(&request(&target, &source, &[], true)).unwrap();

    assert_eq!(count(&tconn, "prompts"), 0);
    assert_eq!(count(&tconn, "media_assets"), 0);
    assert!(!target.join("storage").join("media").join("c.png").exists());
    assert!(report.backup.is_none());
    assert!(report.problems.iter().any(|p| p.contains("回滚")));
}

#[test]
fn 对话运行事件与工具调用整条链都合并且外键自洽() {
    let target = make_lib("target-d");
    let source = make_lib("source-d");
    let tconn = open_with_schema(&db_of(&target));
    let sconn = open_with_schema(&db_of(&source));
    let payload = b"attach";
    sconn
        .execute_batch(&format!(
            "INSERT INTO ai_conversations (id, title) VALUES ('conv-1', '旧会话');
             INSERT INTO ai_messages (id, conversation_id, seq, role) VALUES ('msg-1', 'conv-1', 1, 'user');
             INSERT INTO ai_messages (id, conversation_id, seq, role) VALUES ('msg-2', 'conv-1', 2, 'assistant');
             INSERT INTO ai_attachments (id, original_name, stored_name, kind, mime_type, sha256)
               VALUES ('att-1', 'x.png', 'x.png', 'image', 'image/png', '{}');
             INSERT INTO ai_runs (id, conversation_id, status, user_message_id, assistant_message_id)
               VALUES ('run-1', 'conv-1', 'done', 'msg-1', 'msg-2');
             INSERT INTO ai_tool_calls (id, run_id, provider_call_id, name, status)
               VALUES ('call-1', 'run-1', 'p1', 'comfy_get_status', 'done');
             INSERT INTO ai_message_parts (message_id, ordinal, type, attachment_id, tool_call_id)
               VALUES ('msg-1', 0, 'text', NULL, NULL);
             INSERT INTO ai_message_parts (message_id, ordinal, type, attachment_id)
               VALUES ('msg-1', 1, 'attachment', 'att-1');
             INSERT INTO ai_message_parts (message_id, ordinal, type, tool_call_id)
               VALUES ('msg-2', 0, 'tool_call', 'call-1');
             INSERT INTO ai_run_events (run_id, seq, event_type) VALUES ('run-1', 1, 'run.started');
             INSERT INTO ai_questions (question_id, run_id, conversation_id, group_id, question, status)
               VALUES ('q-1', 'run-1', 'conv-1', 'g-1', '继续吗', 'answered');",
            sha_hex(payload)
        ))
        .unwrap();
    write_file(
        &source.join("storage").join("ai-attachments").join("x.png"),
        payload,
    );
    write_file(
        &source.join("storage").join("ai-thumbs").join("att-1.jpg"),
        b"t",
    );

    let report = merge(&request(&target, &source, &[], false)).unwrap();

    assert_eq!(count(&tconn, "ai_conversations"), 1);
    assert_eq!(count(&tconn, "ai_messages"), 2);
    assert_eq!(count(&tconn, "ai_message_parts"), 3);
    assert_eq!(count(&tconn, "ai_runs"), 1);
    assert_eq!(count(&tconn, "ai_tool_calls"), 1);
    assert_eq!(count(&tconn, "ai_run_events"), 1);
    assert_eq!(count(&tconn, "ai_questions"), 1);
    assert!(super::foreign_key_violations(&tconn).unwrap().is_empty());
    assert!(target
        .join("storage")
        .join("ai-attachments")
        .join("x.png")
        .is_file());
    assert!(report.problems.is_empty(), "{:?}", report.problems);
}

#[test]
fn 文件不在源库那一份里时从额外目录找() {
    // 现场：库在 A 里，产物文件只剩在构建残留里（2026-11-10 那份 30 张图）。
    let target = make_lib("target-e");
    let source = make_lib("source-e");
    let leftover = make_lib("leftover-e");
    let _tconn = open_with_schema(&db_of(&target));
    let sconn = open_with_schema(&db_of(&source));
    let payload = b"only-leftover";
    sconn
        .execute_batch(&format!(
            "INSERT INTO media_assets (id, kind, original_name, stored_name, sha256, size_bytes)
               VALUES (1, 'IMAGE', 'z.png', 'z.png', '{}', {});",
            sha_hex(payload),
            payload.len()
        ))
        .unwrap();
    write_file(
        &leftover.join("storage").join("media").join("z.png"),
        payload,
    );
    write_file(
        &leftover.join("storage").join("thumbs").join("1.jpg"),
        b"thumb-z",
    );

    let report = merge(&request(
        &target,
        &source,
        &[leftover.join("storage")],
        false,
    ))
    .unwrap();

    assert_eq!(
        std::fs::read(target.join("storage").join("media").join("z.png")).unwrap(),
        payload
    );
    // 缩略图也从额外目录找过来了
    assert!(target
        .join("storage")
        .join("thumbs")
        .join("1.jpg")
        .is_file());
    assert_eq!(report.files.missing, 0);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
}
