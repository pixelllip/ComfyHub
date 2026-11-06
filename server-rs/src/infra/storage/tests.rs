//! 占用统计与孤儿扫描的防线。
//!
//! 每一条都对着一个具体的坏情形：把"还在写的文件"当孤儿删掉、把"库里有记录"的删掉、
//! 统计把目录当成文件、恢复旧备份之后的产物被误当成垃圾。

use super::*;
use crate::infra::db;
use crate::infra::testpaths;

struct Scene {
    root: PathBuf,
    storage: PathBuf,
    db_path: PathBuf,
    conn: Connection,
}

fn scene(label: &str) -> Scene {
    let root = testpaths::scratch(label);
    let storage = root.join("storage");
    std::fs::create_dir_all(storage.join("media")).unwrap();
    std::fs::create_dir_all(storage.join("thumbs")).unwrap();
    std::fs::create_dir_all(storage.join("ai-attachments")).unwrap();
    std::fs::create_dir_all(storage.join("ai-thumbs")).unwrap();
    let db_path = root.join(".db").join("comfyhub.db");
    let conn = db::open_or_create(&db_path).expect("建库");
    conn.execute(
        "INSERT INTO media_assets (id, kind, original_name, stored_name, size_bytes)
         VALUES (100, 'IMAGE', 'a.png', 'a.png', 3)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ai_attachments (id, original_name, stored_name, kind, mime_type)
         VALUES ('att-1', 'b.png', 'b.png', 'IMAGE', 'image/png')",
        [],
    )
    .unwrap();
    Scene {
        root,
        storage,
        db_path,
        conn,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// 把 mtime 推到很久以前（`FileTimes` 是标准库的，不用额外依赖）。
fn age(path: &Path, by: Duration) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let when = SystemTime::now() - by;
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
}

fn old() -> Duration {
    Duration::from_secs(48 * 60 * 60)
}

#[test]
fn usage_counts_every_known_directory() {
    let s = scene("storage-usage");
    write(&s.storage.join("media").join("a.png"), b"abc");
    write(&s.storage.join("thumbs").join("100.jpg"), b"12345");
    write(&s.root.join("backups").join("comfyhub-x.db"), &[0u8; 11]);

    let usage = usage(&s.root, &s.storage, &s.db_path);
    let dir = |key: &str| usage.dirs.iter().find(|d| d.key == key).expect(key);
    assert_eq!(dir("media").bytes, 3);
    assert_eq!(dir("media").files, 1);
    assert_eq!(dir("thumbs").bytes, 5);
    assert_eq!(dir("backups").bytes, 11);
    assert!(dir("db").bytes > 0, "库本身要算进去");
    assert_eq!(
        usage.total_bytes,
        usage.dirs.iter().map(|d| d.bytes).sum::<u64>()
    );
    assert!(usage.root.ends_with("storage-usage") || !usage.root.is_empty());
}

#[test]
fn usage_of_missing_directories_is_zero_not_an_error() {
    let root = testpaths::scratch("storage-empty");
    let usage = usage(
        &root,
        &root.join("storage"),
        &root.join(".db").join("comfyhub.db"),
    );
    assert_eq!(usage.total_bytes, 0);
    assert!(usage.dirs.iter().all(|d| d.bytes == 0));
}

#[test]
fn scan_finds_unreferenced_files_and_keeps_referenced_ones() {
    let s = scene("storage-orphans");
    write(&s.storage.join("media").join("a.png"), b"abc"); // 库里有
    write(&s.storage.join("media").join("orphan.png"), b"1234"); // 孤儿
    write(&s.storage.join("thumbs").join("100.jpg"), b"1"); // 库里有
    write(&s.storage.join("thumbs").join("999.jpg"), b"12"); // 孤儿
    write(&s.storage.join("ai-attachments").join("b.png"), b"1"); // 库里有
    write(&s.storage.join("ai-attachments").join("orphan.bin"), b"123"); // 孤儿
    write(&s.storage.join("ai-thumbs").join("att-1.jpg"), b"1"); // 库里有
    write(&s.storage.join("ai-thumbs").join("ghost.jpg"), b"12345"); // 孤儿
    for name in [
        "media/orphan.png",
        "thumbs/999.jpg",
        "ai-attachments/orphan.bin",
        "ai-thumbs/ghost.jpg",
    ] {
        age(&s.storage.join(name), old());
    }
    for name in [
        "media/a.png",
        "thumbs/100.jpg",
        "ai-attachments/b.png",
        "ai-thumbs/att-1.jpg",
    ] {
        age(&s.storage.join(name), old());
    }

    let scan = scan_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(scan.count, 4, "{:?}", scan.items);
    assert_eq!(scan.bytes, 4 + 2 + 3 + 5);
    let kinds: Vec<&str> = scan.items.iter().map(|i| i.kind).collect();
    assert_eq!(kinds.len(), 4);
    assert!(kinds.contains(&"media"));
    assert!(kinds.contains(&"thumb"));
    assert!(kinds.contains(&"attachment"));
    assert!(kinds.contains(&"attachmentThumb"));
}

#[test]
fn recent_files_are_never_treated_as_orphans() {
    let s = scene("storage-grace");
    write(&s.storage.join("media").join("just-written.png"), b"1234");
    // 故意**不**改 mtime：它刚写出来
    let scan = scan_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(scan.count, 0, "1 小时内动过的文件先当成还在写");

    let report = clean_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(report.deleted, 0);
    assert!(s.storage.join("media").join("just-written.png").exists());
}

#[test]
fn clean_deletes_only_the_orphans() {
    let s = scene("storage-clean");
    write(&s.storage.join("media").join("a.png"), b"abc");
    write(&s.storage.join("media").join("orphan.png"), b"1234");
    write(&s.storage.join("thumbs").join("999.jpg"), b"12");
    age(&s.storage.join("media").join("a.png"), old());
    age(&s.storage.join("media").join("orphan.png"), old());
    age(&s.storage.join("thumbs").join("999.jpg"), old());

    let report = clean_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(report.deleted, 2, "{report:?}");
    assert_eq!(report.bytes, 6);
    assert!(report.failed.is_empty());
    assert_eq!(report.remaining, 0);
    assert!(!s.storage.join("media").join("orphan.png").exists());
    assert!(!s.storage.join("thumbs").join("999.jpg").exists());
    assert!(
        s.storage.join("media").join("a.png").exists(),
        "库里有记录的不许删"
    );
}

#[test]
fn nested_directories_are_not_scanned_as_orphans() {
    let s = scene("storage-nested");
    // media 下面冒出一个子目录：它不属于平铺的那一层，扫描不该进去
    write(&s.storage.join("media").join("sub").join("x.png"), b"123");
    age(&s.storage.join("media").join("sub").join("x.png"), old());
    let scan = scan_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(scan.count, 0);
}

#[test]
fn listing_is_truncated_but_totals_are_whole() {
    let s = scene("storage-truncate");
    for i in 0..(MAX_LIST + 5) {
        let path = s.storage.join("media").join(format!("orphan-{i}.png"));
        write(&path, b"1");
        age(&path, old());
    }
    let scan = scan_orphans(&s.conn, &s.storage, SystemTime::now()).unwrap();
    assert_eq!(scan.items.len(), MAX_LIST);
    assert!(scan.truncated);
    assert_eq!(scan.count, (MAX_LIST + 5) as u64);
}
