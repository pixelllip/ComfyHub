//! `db` 的单元测试（主体代码在 [`db`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn temp_path(label: &str) -> std::path::PathBuf {
    crate::infra::testpaths::scratch(&format!("db-{label}")).join("nested").join("comfyhub.db")
}

/// **首次运行要能自己建库**（切流的硬要求）：目录不存在、库文件不存在 → 建目录 + 建表 + 写版本号。
#[test]
fn open_or_create_makes_a_fresh_database() {
    let path = temp_path("fresh");
    assert!(!path.exists());
    let conn = open_or_create(&path).expect("要能自己建库");
    assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    // 关键表都在（少一张 = 端点 500）
    for table in ["prompts", "tags", "media_assets", "ai_conversations", "ai_attachments", "_sync_log"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "表 {table} 没建出来");
    }
    // PRAGMA 照样是自检过的（R1）
    let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(fk, 1);
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// 已经建好的库：**绝不重跑 DDL**（重跑会重建触发器；这个库现在还不算错，但很浪费）。
#[test]
fn open_or_create_leaves_a_current_database_alone() {
    let path = temp_path("current");
    let conn = open_or_create(&path).unwrap();
    conn.execute(
        "INSERT INTO tags (id, name, normalized, use_count, created_at) \
         VALUES (1, '留着的', '留着的', 0, '2026-09-26T00:00:00.000Z')",
        [],
    )
    .unwrap();
    drop(conn);

    let again = open_or_create(&path).unwrap();
    let count: i64 = again.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1, "重开不该动数据");
    assert_eq!(schema_version(&again).unwrap(), SCHEMA_VERSION);
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// 老库（`user_version = 0`，例如手工建的 / P2 之前留下的）：收敛式跑一遍 DDL 并写版本号。
#[test]
fn open_or_create_converges_an_unversioned_database() {
    let path = temp_path("legacy");
    // 先造一个"只有一张表、没有版本号"的库
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE legacy_marker (id INTEGER PRIMARY KEY);").unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 0);
    }
    let conn = open_or_create(&path).unwrap();
    assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    let marker: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='legacy_marker'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, 1, "老库里的东西不许被清掉");
    let prompts: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='prompts'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(prompts, 1, "缺的表要补上");
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// 库比后端新：**只警告不拦**（拦了用户就没法回退版本）。
#[test]
fn open_or_create_warns_but_opens_a_newer_database() {
    let path = temp_path("newer");
    let conn = open_or_create(&path).unwrap();
    conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    drop(conn);
    let again = open_or_create(&path).expect("更新的库也要能打开（只警告）");
    assert_eq!(schema_version(&again).unwrap(), 99, "不许把它降级写回去");
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// ★ **自愈**：库的版本号够新、但 DDL 里声明的一张表不存在 —— 必须把它补上。
///
/// 这一条复刻的是 2026-09-27 的真机现场：库停在 `user_version=1`，`db/schema.sqlite.sql`
/// 里**有** `workflow_recipes` 的完整 DDL，而库里**没有这张表**（`character_cards` 有）。
/// 修之前 `open_or_create` 会在"版本号够新"处直接早退，于是那张表**永远不会被建出来**，
/// 直到某个端点报 `no such table` —— 而那时候没人会想到是迁移。
#[test]
fn open_or_create_heals_a_database_missing_a_declared_table() {
    let path = temp_path("heal");
    let conn = open_or_create(&path).unwrap();
    // 装作这张表是**后加的 DDL**：删掉它，版本号原样留着（= "加了表没 bump 版本"的现场）
    conn.execute_batch("DROP TABLE workflow_recipes;").unwrap();
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workflow_recipes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0, "前提：这张表真的被删掉了");
    assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION, "版本号仍是新的");
    drop(conn);

    let again = open_or_create(&path).expect("要能自愈");
    let healed: i64 = again
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workflow_recipes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(healed, 1, "DDL 声明过、库里没有的表必须被补上");
    assert_eq!(schema_version(&again).unwrap(), SCHEMA_VERSION);
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// DDL 末尾那行 `PRAGMA user_version = N` 与 [`SCHEMA_VERSION`] **必须是同一个数**。
///
/// 它俩是两个真源（一份给两端共用的 DDL、一份给 Rust 的迁移逻辑），而漂了的表现是
/// 静默的：自愈路径会**跑一遍 DDL**，那行就把版本号写回旧值（2026-09-27 实测踩到：
/// 表补上了，`user_version` 从 2 退回 1）。Rust 侧现在取 `max(...)` 兜住这个方向，
/// 但"两个数本来就该相等"这件事仍然要用一条用例钉住。
#[test]
fn ddl_declared_version_matches_the_constant() {
    let declared: i64 = SCHEMA_SQL
        .lines()
        .filter_map(|line| line.trim().strip_prefix("PRAGMA user_version = "))
        .find_map(|value| value.trim_end_matches(';').trim().parse().ok())
        .expect("DDL 末尾应当有一行 `PRAGMA user_version = N;`");
    assert_eq!(
        declared, SCHEMA_VERSION,
        "db/schema.sqlite.sql 末尾的 user_version 与 db.rs 的 SCHEMA_VERSION 漂了 —— \
         加表/改列时两个一起 +1"
    );
}

/// 钉住那条解析器：**DDL 里声明的每一张表，灌完 DDL 之后都必须真的存在**。
///
/// 少了它，`declared_tables()` 一旦漏读一张（换了写法 / 漏了 `CREATE TABLE IF NOT EXISTS`
/// 这个前缀），自愈检查就会**静默地不检查那一张** —— 而那正是它存在的全部意义。
#[test]
fn every_declared_table_is_actually_created() {
    let conn = open_memory().unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    let declared = declared_tables();
    assert!(declared.contains(&"workflow_recipes"), "至少得认得出这张表：{declared:?}");
    assert!(declared.contains(&"character_cards"), "{declared:?}");
    assert!(declared.contains(&"_sync_log"), "{declared:?}");
    for name in &declared {
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "{name} 在 DDL 里声明了却没建出来（解析器漏了？）");
    }
    // 反过来也要成立：库里建出来的表**一张都不许是**"声明之外"的
    // （漏读一张 = 自愈检查会漏掉那一张，那是最危险的方向）
    assert!(declared.len() >= 20, "表数明显变少说明解析器坏了：{}", declared.len());
}

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
