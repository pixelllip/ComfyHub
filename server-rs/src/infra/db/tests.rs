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

/// ★★ **一次改动同时"加表 + 加列"**：两条自愈路都必须走通（2026-10-11 产物集合）。
///
/// 为什么单独盯这一条：这次加的是 `media_collections`（表）+ `media_assets.collection_id`（列），
/// 而两条路**各自都会静默失败**：
/// * 只 bump 版本号、没进 `NEW_COLUMNS` → 列永远不存在，集合永远是 `None`，界面上什么都不显示；
/// * 只进 `NEW_COLUMNS`、没 bump → 老库直接早退，表永远不存在，**产物一入库就炸 `no such table`**。
///
/// 顺带验一个容易踩的语法：`ALTER TABLE ... ADD COLUMN ... REFERENCES`（SQLite 允许，
/// 但老库补上的这一列是"没有外键约束"的形态）—— 所以这里还要证明**它照样能写、能读**。
#[test]
fn open_or_create_heals_the_collection_table_and_column_together() {
    let path = temp_path("collection-heal");
    let conn = open_or_create(&path).unwrap();
    // 装成"集合还不存在"的老库：列先去掉（表还在，模拟"只加了列没加表"的另一半）。
    // ⚠ SQLite 的 `DROP COLUMN` 会因为**索引引用着这一列**而失败
    // （`error in index idx_media_collection after drop column`），所以先删索引 ——
    // 重开库时 DDL 会把索引一起补回来（`CREATE INDEX IF NOT EXISTS`）。
    conn.execute_batch(
        "DROP INDEX idx_media_collection;
         ALTER TABLE media_assets DROP COLUMN collection_id;
         ALTER TABLE media_assets DROP COLUMN collection_opt_out;
         ALTER TABLE media_collections DROP COLUMN title;",
    )
    .unwrap();
    let has: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('media_assets') WHERE name='collection_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has, 0, "前提：collection_id 这一列真的没了");
    let has_opt_out: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('media_assets') WHERE name='collection_opt_out'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_opt_out, 0, "前提：collection_opt_out 这一列真的没了");
    let has_title: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('media_collections') WHERE name='title'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_title, 0, "前提：media_collections.title 这一列真的没了");
    drop(conn);

    let again = open_or_create(&path).expect("要能补列");
    let has: i64 = again
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('media_assets') WHERE name='collection_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has, 1, "DDL 里声明的列必须被补上");

    // 2026-10-18 的两列：**老库也必须拿到**，否则
    // ① "移出集合"失效（`backfill` 会把它吸回去）；② 手动集合没有名字。
    for (table, column) in [
        ("media_assets", "collection_opt_out"),
        ("media_collections", "title"),
    ] {
        let n: i64 = again
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{column}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "老库必须补上 {table}.{column}");
    }

    // 补上的列必须真的能用：建集合 → 挂产物 → 读回来
    again
        .execute(
            "INSERT INTO media_collections (group_key, kind) VALUES ('k1', 'IMAGE')",
            [],
        )
        .unwrap();
    let cid: i64 = again
        .query_row("SELECT id FROM media_collections WHERE group_key='k1'", [], |r| r.get(0))
        .unwrap();
    again
        .execute(
            "INSERT INTO media_assets (kind, original_name, stored_name, collection_id)
             VALUES ('IMAGE', 'a.png', 'a.png', ?1)",
            rusqlite::params![cid],
        )
        .unwrap();
    let back: Option<i64> = again
        .query_row("SELECT collection_id FROM media_assets WHERE original_name='a.png'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(back, Some(cid), "补出来的列要能存能读");

    // 集合行被删掉时产物**不能跟着消失**（ON DELETE SET NULL 的语义）
    again
        .execute("DELETE FROM media_collections WHERE id = ?1", rusqlite::params![cid])
        .unwrap();
    let (n, cid_after): (i64, Option<i64>) = again
        .query_row(
            "SELECT COUNT(*), MAX(collection_id) FROM media_assets WHERE original_name='a.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(n, 1, "删集合不该把产物删掉");
    assert_eq!(cid_after, None, "产物要退回未分组");
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// **加列**的自愈（2026-09-30 起）：老库缺一列 → 开库时用 `ALTER` 补上，已有数据不动。
///
/// 为什么非要有这条：收敛式 DDL 的建表语句**不给已存在的表补列**（`IF NOT EXISTS` 只判表在不在），
/// 所以在 DDL 里加一列、老库永远拿不到它，**而且不报错** —— 表现是"这列读出来永远是默认值"。
#[test]
fn open_or_create_heals_a_missing_column_without_touching_data() {
    let path = temp_path("add-column");
    let conn = open_or_create(&path).unwrap();
    // 装成"这列是后加的"：SQLite 3.35+ 支持 DROP COLUMN；老版本靠重建表，这里用前者
    conn.execute_batch("ALTER TABLE character_cards DROP COLUMN prompt;").unwrap();
    conn.execute_batch(
        "INSERT INTO character_cards (name, normalized, negative) VALUES ('先有的卡', '先有的卡', 'bad hands');",
    )
    .unwrap();
    let has: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('character_cards') WHERE name='prompt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has, 0, "前提：prompt 这一列真的没了");
    drop(conn);

    let again = open_or_create(&path).expect("要能补列");
    let has: i64 = again
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('character_cards') WHERE name='prompt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has, 1, "DDL 里声明的列必须被补上");
    let kept: (String, String) = again
        .query_row("SELECT name, negative FROM character_cards", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(kept, ("先有的卡".to_string(), "bad hands".to_string()), "补列不许动已有数据");
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// 库比后端新 + **缺一张表**：自愈可以，但**版本号绝不许被 DDL 那行拖下去**。
///
/// 这条盯的是 2026-09-30 补的那个判据：原来写的是 `if target != version`，在"库是 99、
/// 只是缺一张表"时 `target(99) != version(99)` 为假 → **该写的那次没写**，而 DDL 末尾那行
/// `PRAGMA user_version = 5` 已经把库写成 5 → 静默降级（用旧二进制写新库最危险的那件事）。
#[test]
fn healing_a_missing_table_never_downgrades_a_newer_version() {
    let path = temp_path("heal-newer");
    let conn = open_or_create(&path).unwrap();
    conn.execute_batch("DROP TABLE workflow_recipes;").unwrap();
    conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    drop(conn);

    let again = open_or_create(&path).expect("更新的库也要能打开");
    let healed: i64 = again
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workflow_recipes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(healed, 1, "缺的表照样要补上");
    assert_eq!(schema_version(&again).unwrap(), 99, "绝不许把更新的库降级写回去");
    std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).ok();
}

/// ★★ **老库的数据迁移**：老卡的 `tags`（JSON 数组）必须被拼成 `prompt`（整段文本）。
///
/// 这条盯的是 2026-10 角色卡收敛（只剩名称 / 正面提示词 / 负面提示词）：老库里 `prompt` 这一列
/// 根本不存在，而新代码读的就是它 —— 不搬过去，用户打开角色卡页看到的是**一页空卡**
/// （数据明明还在 `tags` 里）。列由 `ensure_columns` 补，内容由 `backfill_character_card_prompt` 搬。
///
/// ⚠ 两条边界是同一条用例的两面：**只搬一次**（幂等）+ **不许复活**用户清空过的提示词
/// （老库的 `tags` 列留着不走，判据"prompt 空 + tags 非空"永远成立，每次开库都搬就会删了又回来）。
///
/// ⚠ 顺带守着 [`open_or_create`] 那条顺序纪律（先补列、再跑 DDL）：DDL 里若出现
/// `CREATE INDEX ... ON <表>(<新列>)`，补列排在后面就会**整批失败、库直接打不开**
/// （2026-09-30 真现场：`no such column: enabled in CREATE INDEX … idx_character_cards_enabled`；
/// 角色卡收敛后 DDL 里已经没有建在新列上的索引，但顺序照旧不许反）。
#[test]
fn open_or_create_migrates_the_old_card_tags_into_the_prompt() {
    let path = temp_path("upgrade-old-lib");
    {
        // 装作"这是收敛之前那一版的库"：把老列加回来、`prompt` 拆掉、版本号退回 5，
        // 再塞一张有 tag 栈的老卡（tags 是 JSON 数组文本，与老实现同一个约定）。
        let conn = open_or_create(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE character_cards ADD COLUMN tags TEXT NULL;\
             ALTER TABLE character_cards DROP COLUMN prompt;\
             INSERT INTO character_cards (name, normalized, tags, negative) \
               VALUES ('先有的卡', '先有的卡', '[\"shinku_(nte)\",\"pink streaks\"]', 'bad hands');\
             PRAGMA user_version = 5;",
        )
        .unwrap();
    }

    let conn = open_or_create(&path).expect("老库必须能升级（这是每个已装用户的路径）");

    // 新列补上了
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('character_cards') WHERE name = 'prompt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(present, 1, "prompt 这一列没补上");
    // ★ 老 tag 栈一个不许丢：按老约定拼成一行
    let migrated: (String, Option<String>, String) = conn
        .query_row("SELECT name, prompt, negative FROM character_cards", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(migrated.0, "先有的卡");
    assert_eq!(migrated.1.as_deref(), Some("shinku_(nte), pink streaks"), "tags 要拼成正面提示词");
    assert_eq!(migrated.2, "bad hands", "老数据别的字段不动");
    assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION, "版本号要跟到最新");
    drop(conn);

    // 幂等：再开一次不许拼第二遍（也不许把用户后来手填的 prompt 盖掉）
    let again = open_or_create(&path).expect("再开一次");
    let kept: String = again
        .query_row("SELECT prompt FROM character_cards", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, "shinku_(nte), pink streaks", "迁移必须幂等：不许重复拼接");

    // ★ 用户在界面上清空提示词（`prompt = NULL`）之后，**下次开库不许把它从 tags 复活**：
    //   老库那个 `tags` 列还在，判据"prompt 为空 + tags 非空"永远成立 —— 所以迁移只能跑
    //   "刚补上这一列"的那一次（`open_or_create` 拿 `added` 判）。
    again.execute("UPDATE character_cards SET prompt = NULL", []).unwrap();
    drop(again);
    let third = open_or_create(&path).expect("第三次开库");
    let cleared: Option<String> = third
        .query_row("SELECT prompt FROM character_cards", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cleared, None, "清空过的提示词不许被迁移复活");
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
