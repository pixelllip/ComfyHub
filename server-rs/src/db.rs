//! SQLite 连接与事务纪律（🔒 `docs/sqlite-schema-contract.md` §2，红线 **R1** / **R2**）。
//!
//! 这个文件**只有一件事**：让"每一条连接都带上正确的 PRAGMA"和"写事务一律 `BEGIN IMMEDIATE`"
//! 成为**唯一入口**，而不是散落在各个 handler 里的自觉。
//!
//! 为什么值得单独一个模块（而不是在 `main.rs` 里开个连接了事）：
//!
//! - **R1**：`PRAGMA foreign_keys` 默认 **OFF**、而且**按连接生效**。漏了不报错，
//!   只是外键静默失效（删会话不级联、删标签留一堆孤儿关联行）。
//!   所以这里不仅有 `apply`，还有 [`assert_ready`]：**把值读回来核对**，
//!   不对就当场抛 —— "写了就算"不算数（连接池里另一条连接可能没带上）。
//! - **R2**：写事务一律 `BEGIN IMMEDIATE`（[`tx_immediate`]）。裸 `BEGIN` 是延迟事务，
//!   读锁升级写锁那一下**可能不理会 `busy_timeout`** 直接 `SQLITE_BUSY`。
//!
//! 与 Kotlin 侧的对应物：`Db.kt`（方言化 + PRAGMA 自检 + `BEGIN IMMEDIATE`）。

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::Path;

/// 一条连接的 PRAGMA 清单（顺序无关，但**一条都不能少**）。
pub const PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous  = NORMAL;
    PRAGMA busy_timeout = 5000;
    PRAGMA foreign_keys = ON;
";

/// 开一条**已经上好 PRAGMA** 的连接（连接工厂 —— 别在别处直接 `Connection::open`）。
///
/// `WAL` 允许读写并发，所以"一条写连接 + 若干只读连接"是安全的；
/// Rust 侧不引连接池（`tokio-rusqlite` 的专属写线程在端点铺开后再加）。
pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(PRAGMAS)?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// 只读连接（`/api/*` 的读端点用它；`mode=ro` 时**不会**因为并发写而拿不到库）。
pub fn open_readonly(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // 只读连接同样要上 `busy_timeout` 与 `foreign_keys`：R1 没有例外
    conn.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// 内存库（单测用；DDL 由调用方从 `db/schema.sqlite.sql` 灌进去）。
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    // 内存库没有 WAL（返回 memory），其余 PRAGMA 照上
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;",
    )?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// **把该为真的东西读回来核对**（R1 / R14）—— 不对就当场抛，绝不允许"带着坏掉的库继续跑"。
pub fn assert_ready(conn: &Connection) -> rusqlite::Result<()> {
    let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if fk != 1 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "R1 未生效：PRAGMA foreign_keys={fk}（应为 1）—— 外键会静默失效"
        )));
    }
    Ok(())
}

/// 引擎信息（`/api/health` 用；也是 R14 的自检口径）。
pub fn engine_info(conn: &Connection) -> rusqlite::Result<serde_json::Value> {
    let version: String = conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    // FTS5 探针：系统 SQLite（Android 13 的 3.32a）没有它；
    // 桌面 bundled 版必须有 —— 缺了就是选型出了问题，要当场看得见
    let fts5 = conn
        .execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_probe USING fts5(x); DROP TABLE IF EXISTS temp.fts_probe;")
        .is_ok();
    Ok(serde_json::json!({
        "sqliteVersion": version,
        "fts5": fts5,
    }))
}

/// schema 版本（= `/api/sync/manifest` 与 run-kit 说的**同一个数**）。
pub fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// 写事务：**`BEGIN IMMEDIATE`**（R2）+ 出错自动回滚。
pub fn tx_immediate<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R1：读回来必须为 1（"写了就算"不算数）。
    #[test]
    fn pragma_foreign_keys_is_on_and_read_back() {
        let conn = open_memory().expect("开库");
        let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
        assert!(assert_ready(&conn).is_ok());
    }

    /// R1 的反面证据：**自检必须抓得住 fk=0**。
    ///
    /// ⚠ 实测发现：bundled 构建（rusqlite 0.40 + SQLite 3.53）**编译期默认就是 ON**
    /// （`SQLITE_DEFAULT_FOREIGN_KEYS`），所以"随手开一条连接就是 0"这句在 Rust 侧**不成立**。
    /// 这恰恰说明 [`assert_ready`] 才是那条真正的防线：默认值会随构建参数变，
    /// 而"读回来必须是 1"不随任何东西变。
    #[test]
    fn assert_ready_catches_a_connection_with_foreign_keys_off() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 0);
        assert!(assert_ready(&conn).is_err(), "自检必须抓住 fk=0");
    }

    /// R2：`BEGIN IMMEDIATE` 的写法能跑，且**出错要回滚**。
    #[test]
    fn tx_immediate_commits_and_rolls_back() {
        let mut conn = open_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);")
            .unwrap();
        tx_immediate(&mut conn, |tx| {
            tx.execute("INSERT INTO t (v) VALUES ('a')", [])
                .map(|_| ())
        })
        .unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);

        let failed: rusqlite::Result<()> = tx_immediate(&mut conn, |tx| {
            tx.execute("INSERT INTO t (v) VALUES ('b')", [])?;
            Err(rusqlite::Error::InvalidQuery) // 故意失败
        });
        assert!(failed.is_err());
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "回滚后不该留下第二条");
    }

    /// R14：bundled 引擎必须带 FTS5。
    #[test]
    fn bundled_engine_has_fts5() {
        let conn = open_memory().unwrap();
        let info = engine_info(&conn).unwrap();
        assert_eq!(info["fts5"], serde_json::json!(true), "engine={info}");
    }
}
