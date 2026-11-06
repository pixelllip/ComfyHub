//! 备份 / 恢复的防线。
//!
//! 每一条都对着一个**具体的坏情形**（"恢复完库坏了""选错备份没退路""路径穿越"），
//! 不是"把函数跑一遍"。

use super::*;
use crate::infra::testpaths;

/// 一套临时现场：`root`（运行根）+ `root\.db\comfyhub.db`。
fn scene(label: &str) -> (PathBuf, PathBuf) {
    let root = testpaths::scratch(label);
    let db = root.join(".db").join("comfyhub.db");
    (root, db)
}

fn seed(db: &Path, title: &str) -> Connection {
    let conn = db::open_or_create(db).expect("建库");
    conn.execute(
        "INSERT INTO prompts (id, title, kind, positive_prompt, favorite, source, created_at, updated_at)
         VALUES (1, ?1, 'IMAGE', 'x', 0, 'Manual', '2026-11-06T00:00:00Z', '2026-11-06T00:00:00Z')",
        rusqlite::params![title],
    )
    .expect("灌一条提示词");
    conn
}

fn title_of(db: &Path) -> String {
    let conn = Connection::open(db).expect("开库");
    conn.query_row("SELECT title FROM prompts WHERE id = 1", [], |r| r.get(0))
        .expect("读标题")
}

#[test]
fn a_backup_is_a_consistent_single_file() {
    let (root, db) = scene("backup-roundtrip");
    let conn = seed(&db, "恢复前的标题");

    let item = create(&conn, &root).expect("备份");
    assert!(item.name.starts_with(AUTO_PREFIX), "{}", item.name);
    assert!(item.bytes > 0);
    assert_eq!(item.kind, "auto");

    // 备份是**单文件**：没有 -wal / -shm 跟着
    let backup = dir(&root).join(&item.name);
    assert!(!PathBuf::from(format!("{}-wal", backup.display())).exists());
    assert_eq!(title_of(&backup), "恢复前的标题");
    assert_eq!(list(&root).unwrap().len(), 1);
}

#[test]
fn restore_swaps_the_database_and_keeps_a_safety_copy() {
    let (root, db) = scene("backup-restore");
    let conn = seed(&db, "备份里的标题");
    let item = create(&conn, &root).expect("备份");

    // 备份之后又改了库：恢复要把这句改回去
    conn.execute("UPDATE prompts SET title = '后来改的标题' WHERE id = 1", [])
        .unwrap();
    drop(conn);

    let conn = db::open_or_create(&db).expect("重新开库");
    let prepared = restore_prepare(&conn, &root, &db, &item.name).expect("准备恢复");
    drop(conn);

    // 准备阶段**不许动当前库**
    assert_eq!(title_of(&db), "后来改的标题");
    assert_eq!(pending(&db).as_deref(), Some(item.name.as_str()));
    let safety = prepared.safety;
    assert!(safety.starts_with(BEFORE_PREFIX));
    assert_eq!(title_of(&dir(&root).join(&safety)), "后来改的标题");

    let outcome = apply_pending(&db).expect("有标记");
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(title_of(&db), "备份里的标题");
    assert!(pending(&db).is_none(), "标记必须清掉");
    assert!(last_restore(&db).unwrap().ok);
}

#[test]
fn apply_is_idempotent_when_the_tmp_file_disappeared() {
    let (root, db) = scene("backup-idempotent");
    let conn = seed(&db, "备份里的标题");
    let item = create(&conn, &root).expect("备份");
    conn.execute("UPDATE prompts SET title = '后来改的标题' WHERE id = 1", [])
        .unwrap();
    drop(conn);

    let conn = db::open_or_create(&db).expect("开库");
    restore_prepare(&conn, &root, &db, &item.name).expect("准备恢复");
    drop(conn);

    // 模拟"复制完 tmp 之后被人清掉了"：启动时应当从备份重建，而不是放弃
    std::fs::remove_file(tmp_path(&db)).unwrap();
    let outcome = apply_pending(&db).expect("有标记");
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(title_of(&db), "备份里的标题");
}

#[test]
fn apply_finishes_when_it_crashed_between_the_two_renames() {
    let (root, db) = scene("backup-crash");
    let conn = seed(&db, "备份里的标题");
    let item = create(&conn, &root).expect("备份");
    drop(conn);

    let conn = db::open_or_create(&db).expect("开库");
    restore_prepare(&conn, &root, &db, &item.name).expect("准备恢复");
    drop(conn);

    // 模拟崩在 "db → restore-old" 之后、"tmp → db" 之前
    std::fs::rename(&db, old_path(&db)).unwrap();
    assert!(!db.exists());

    let outcome = apply_pending(&db).expect("有标记");
    assert!(outcome.ok, "{}", outcome.message);
    assert!(db.exists());
    assert_eq!(title_of(&db), "备份里的标题");
    assert!(!old_path(&db).exists(), "旧库收尾时该删掉");
}

#[test]
fn restore_refuses_a_newer_schema() {
    let (root, db) = scene("backup-newer");
    let conn = seed(&db, "t");
    conn.execute_batch(&format!("PRAGMA user_version = {}", db::SCHEMA_VERSION + 1))
        .unwrap();
    drop(conn);
    let name = "comfyhub-newer.db";
    std::fs::create_dir_all(dir(&root)).unwrap();
    std::fs::copy(&db, dir(&root).join(name)).unwrap();
    let conn = db::open_or_create(&db).expect("开库");
    let err = restore_prepare(&conn, &root, &db, name).unwrap_err();
    assert!(err.contains("更新的版本"), "{err}");
    assert!(pending(&db).is_none(), "失败不许留下标记");
}

#[test]
fn restore_refuses_a_foreign_file() {
    let (root, db) = scene("backup-foreign");
    let conn = seed(&db, "t");
    let name = "comfyhub-foreign.db";
    std::fs::create_dir_all(dir(&root)).unwrap();
    std::fs::write(dir(&root).join(name), b"definitely not a database").unwrap();
    let err = restore_prepare(&conn, &root, &db, name).unwrap_err();
    assert!(
        err.contains("不是一个 SQLite 数据库") || err.contains("坏"),
        "要说人话：{err}"
    );
    assert!(pending(&db).is_none());
}

#[test]
fn names_never_escape_the_backup_directory() {
    for bad in ["../evil.db", "a/b.db", "..", "", "x.txt", "a..b.db"] {
        assert!(!is_safe_name(bad), "`{bad}` 不该被当成备份名");
    }
    for good in [
        "comfyhub-20261106-131211.db",
        "before-restore-20261106-131211-2.db",
    ] {
        assert!(is_safe_name(good), "`{good}` 是合法备份名");
    }
    let (root, db) = scene("backup-names");
    let conn = seed(&db, "t");
    assert!(restore_prepare(&conn, &root, &db, "../comfyhub.db").is_err());
    assert!(delete(&root, "../comfyhub.db").is_err());
}

#[test]
fn deleting_a_backup_reports_missing_ones() {
    let (root, db) = scene("backup-delete");
    let conn = seed(&db, "t");
    let item = create(&conn, &root).expect("备份");
    assert!(delete(&root, &item.name).expect("删掉"));
    assert!(list(&root).unwrap().is_empty());
    assert!(
        !delete(&root, &item.name).expect("再删一次"),
        "本来就没有 = false"
    );
}

#[test]
fn pruning_keeps_the_newest_and_never_touches_foreign_files() {
    let (root, db) = scene("backup-prune");
    let conn = seed(&db, "t");
    for _ in 0..(KEEP_AUTO + 2) {
        create(&conn, &root).expect("备份");
    }
    let items = list(&root).unwrap();
    assert_eq!(items.len(), KEEP_AUTO, "只留 {KEEP_AUTO} 份：{items:?}");

    // 不是我们命名的文件，不列也不删
    let stray = dir(&root).join("notes.txt");
    std::fs::write(&stray, b"keep me").unwrap();
    prune(&root, AUTO_PREFIX).unwrap();
    assert!(stray.exists());
    assert!(list(&root).unwrap().iter().all(|i| i.name != "notes.txt"));
}

#[test]
fn two_backups_in_the_same_second_get_distinct_names() {
    let (root, db) = scene("backup-unique");
    let conn = seed(&db, "t");
    let a = create(&conn, &root).unwrap();
    let b = create(&conn, &root).unwrap();
    assert_ne!(a.name, b.name);
    assert_eq!(list(&root).unwrap().len(), 2);
}

#[test]
fn no_marker_means_nothing_happens() {
    let (_root, db) = scene("backup-noop");
    seed(&db, "t");
    assert!(apply_pending(&db).is_none());
    assert!(last_restore(&db).is_none());
}
