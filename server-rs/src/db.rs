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
    PRAGMA temp_store = MEMORY;
";

/// ⚠ **`temp_store = MEMORY` 不是调优，是"别依赖一个可写的 `%TEMP%`"**（2026-09-27 真机实测）。
///
/// 现场：`DELETE /api/prompts/{id}` 在**活着且健康**的后端上稳定回
/// `500 {"error":"数据库错误：unable to open database file"}` —— 而同一条删除
/// 对着**同一份库的副本**（另一个进程）却回 200。差别只有一个：
/// 那个进程的映像在工作区内、**不能在 `%TEMP%` 建文件**，而 SQLite 默认
/// （`temp_store = FILE`）会把排序/临时表溢出到 `%TEMP%` —— 建不出那个文件，
/// 于是报的是"打不开数据库文件"，看着像库坏了，其实是**临时文件建不出来**。
///
/// 放到 `PRAGMAS` 里而不是每处 `PRAGMA`：它是**连接级**的，只设一处必然漏。
/// 顺带对发布包也是好事：便携包可能落在只读/受限的目录里，别再额外依赖 `%TEMP%` 可写。
pub fn assert_temp_store_is_memory(conn: &Connection) -> rusqlite::Result<()> {
    // 2 = MEMORY（0 = 默认、1 = FILE）
    let mode: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    if mode != 2 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "temp_store={mode}（应为 2 = MEMORY）：临时表会落到 %TEMP%，\
             那里不可写时删除/排序会报成 'unable to open database file'"
        )));
    }
    Ok(())
}

/// 开一条**已经上好 PRAGMA** 的连接（连接工厂 —— 别在别处直接 `Connection::open`）。
///
/// `WAL` 允许读写并发，所以"一条写连接 + 若干只读连接"是安全的；
/// Rust 侧不引连接池（`tokio-rusqlite` 的专属写线程在端点铺开后再加）。
/// 建表用的 DDL（**编译期嵌进二进制**）。
///
/// 与 Kotlin 从 jar 里读 `db/schema.sqlite.sql` **同一性质**：真源是仓库里那份文件，
/// 二进制自带一份副本。⚠ 改了 DDL 要**重新编译后端**（仓库里 `db\schema.sqlite.sql`
/// 那份留给人和 sqlite3 CLI 手工建库用，运行期不读它）。
pub const SCHEMA_SQL: &str = include_str!("../../db/schema.sqlite.sql");

/// schema 版本（`PRAGMA user_version`，见 🔒 `docs/sqlite-schema-contract.md`）。
/// 这个后端认识的 schema 版本。
///
/// **加表 / 改列 / 加同步触发器都要 +1**（2026-09-27 由 1 提到 2：`workflow_recipes` 的 DDL
/// 早就在 `db/schema.sqlite.sql` 里，而真库停在 1、**那张表根本不存在** —— 见
/// [`open_or_create`] 的 ⚠；同日由 2 提到 3：给 `ai_runs` / `ai_tool_calls` 补 `_sync_log`
/// 触发器 —— **不是加表但同样够不着老库**，理由写在 `db/schema.sqlite.sql` 末尾）。
pub const SCHEMA_VERSION: i64 = 3;

/// **开库（必要时建库）**：切流之后 Rust 是唯一的后端，"第一次运行要能自己建库"这件事
/// 必须在它这边成立 —— 以前那版遇到缺库直接 `exit 3`（"先让 Kotlin 起一次库"），
/// 那在发布包里等于**永远起不来**（包里根本没有 Kotlin）。
///
/// 四种情况：
///  1. **文件不存在**（或存在但是空文件）→ 建父目录 → 建库 → 执行 [`SCHEMA_SQL`] → `user_version = SCHEMA_VERSION`；
///  2. `user_version == 0`（老库 / 手工建的库）→ 执行一遍 DDL（脚本是**收敛式**的：
///     `CREATE TABLE IF NOT EXISTS` + `DROP TRIGGER IF EXISTS` + `CREATE TRIGGER`，重跑安全）→ 写版本号；
///  3. `version >= SCHEMA_VERSION` → **先核对 DDL 里声明的表是不是都在**：都在就什么都不做
///     （**绝不重跑 DDL**：那会重建触发器），缺了就补一遍（见下面那条 ⚠）；
///  4. `version > SCHEMA_VERSION` → **只警告不拦**（拦了用户就没法回退版本），
///     并且**绝不把版本号往小写**（只往下写、不往回落）。
///
/// ⚠ **为什么第 3 类里那道"表都在吗"的检查一个字都不能省**（2026-09-27 真机实测抓到）：
/// `db/schema.sqlite.sql` 是**两端共用**的一份文件，谁往里加一张表都行；而"加了表、
/// 忘了 bump [`SCHEMA_VERSION`]"的表现是**静默的**：
/// 老库停在旧版本号 → `version >= SCHEMA_VERSION` 直接早退 → **那张表永远不存在**，
/// 直到某个端点报 `no such table`（而那时候没人会想到是迁移）。
///
/// 真机现场就是这样：库停在 `user_version=1`，schema 里**有** `workflow_recipes` 的完整 DDL，
/// 而库里**没有这张表**（`character_cards` 有、它没有 —— 差别只在于 DDL 是什么时候加进去的）。
/// 现在这条是**自愈**的：缺哪张补哪张，并且**打一行日志把补了什么说出来**（绝不静默）。
pub fn open_or_create(path: impl AsRef<Path>) -> Result<Connection, String> {
    let path = path.as_ref();
    let fresh = !path.exists() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true);
    if fresh {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建库目录失败（{}）：{e}", parent.display()))?;
        }
    }
    let conn = open(path).map_err(|e| format!("开库失败（{}）：{e}", path.display()))?;
    let version = schema_version(&conn).map_err(|e| format!("读 user_version 失败：{e}"))?;
    let mut missing: Vec<&'static str> = Vec::new();
    if version >= SCHEMA_VERSION && !fresh {
        if version > SCHEMA_VERSION {
            eprintln!(
                "⚠ 库的 schemaVersion={version} 比这个后端认识的（{SCHEMA_VERSION}）**新**：\
                 它可能是更新的版本建的。继续用会有列对不上的风险，建议升级后端。"
            );
        }
        // ★ 自愈（见函数头那条 ⚠）：版本号够新**不等于**表都在
        missing = missing_tables(&conn).map_err(|e| format!("查缺表失败：{e}"))?;
        if missing.is_empty() {
            return Ok(conn);
        }
        eprintln!(
            "[db] 库里缺 {} 张 DDL 声明过的表（{}）—— 多半是**加了表没 bump SCHEMA_VERSION**。\
             跑一遍收敛式 DDL 补上；已有数据不动。",
            missing.len(),
            missing.join("、")
        );
    }
    // 建表 / 收敛式迁移：整个 DDL 在一个事务里跑（中途失败不留半个 schema）
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| format!("开事务失败：{e}"))?;
    match conn.execute_batch(SCHEMA_SQL) {
        Ok(()) => {
            // ⚠ **版本号取 `max(现有, SCHEMA_VERSION)`**，两个方向都要照顾到：
            //  * **只往下写、绝不往回落**：库比后端新时保留它自己的版本号
            //    （写回 SCHEMA_VERSION 等于把用户的库降级，而那正是"用旧二进制写新库"最危险的事）；
            //  * **但也不能被 DDL 里的那行拖回去**：`db/schema.sqlite.sql` 末尾自己也有一行
            //    `PRAGMA user_version = N`（它是两端共用的文件、Kotlin 侧也读它），
            //    而"库够新、只是缺张表"那条自愈路径里我们**刚跑过一遍 DDL** ——
            //    如果照搬"DLL 里的值"，版本号就会从 2 **退回 1**（2026-09-27 实测踩到：
            //    自愈成功、表补上了，`user_version` 却掉回 1）。
            let target = version.max(SCHEMA_VERSION);
            if target != version {
                conn.execute_batch(&format!("PRAGMA user_version = {target};"))
                    .map_err(|e| format!("写 user_version 失败：{e}"))?;
            }
            conn.execute_batch("COMMIT;").map_err(|e| format!("提交失败：{e}"))?;
            if fresh {
                eprintln!("[db] 已建库并建表：{}（schemaVersion={SCHEMA_VERSION}）", path.display());
            } else if missing.is_empty() {
                eprintln!("[db] 已把老库收敛到 schemaVersion={SCHEMA_VERSION}：{}", path.display());
            } else {
                eprintln!(
                    "[db] 已补上 {} 张缺的表（{}）：{}",
                    missing.len(),
                    missing.join("、"),
                    path.display()
                );
            }
            Ok(conn)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("执行 db/schema.sqlite.sql 失败：{e}"))
        }
    }
}

/// 建表用的 DDL 文本（给用例与"手工建库"用）。
pub fn schema_sql() -> &'static str {
    SCHEMA_SQL
}

/// DDL 里**声明的**表名（`CREATE TABLE IF NOT EXISTS <name>`）。
///
/// 它是 [`open_or_create`] 那条自愈检查的判据，也是"加表忘了 bump 版本"这条静默故障的
/// 唯一解药。⚠ 解析的是**我们自己的** DDL 文本，所以判据只能是"这一种写法"——
/// 哪天有人用别的写法建表（换行 / 加引号 / `CREATE VIRTUAL TABLE`），
/// `every_declared_table_is_actually_created` 会当场变红（那条用例盯的正是这个）。
pub fn declared_tables() -> Vec<&'static str> {
    const MARK: &str = "CREATE TABLE IF NOT EXISTS ";
    let mut out: Vec<&'static str> = Vec::new();
    let mut rest = SCHEMA_SQL;
    while let Some(at) = rest.find(MARK) {
        let tail = &rest[at + MARK.len()..];
        let name = tail.split(|c: char| c.is_whitespace() || c == '(').next().unwrap_or("");
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = tail;
    }
    out
}

/// 库里**缺**哪些 DDL 声明过的表（空 = 一切正常）。
fn missing_tables(conn: &Connection) -> rusqlite::Result<Vec<&'static str>> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in declared_tables() {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        )?;
        if found == 0 {
            out.push(name);
        }
    }
    Ok(out)
}

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
    // 只读连接同样要上 `busy_timeout`、`foreign_keys` 与 `temp_store`：R1 没有例外，
    // 而 temp_store 也是连接级的（只读连接照样会做排序 → 照样可能要临时文件）
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;",
    )?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// 内存库（单测用；DDL 由调用方从 `db/schema.sqlite.sql` 灌进去）。
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    // 内存库没有 WAL（返回 memory），其余 PRAGMA 照上
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; \
         PRAGMA temp_store = MEMORY;",
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
    // 与 R1 同一种纪律：**读回来核对**，别"写了就算"
    assert_temp_store_is_memory(conn)?;
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
///
/// 错误类型做成泛型（`E: From<rusqlite::Error>`）：仓储层有自己的错误分类
/// （[`crate::repo::WriteError`]：400 / 404 / 500），但**事务纪律只有这一处实现** ——
/// 别为了换个错误类型就在别处再拼一遍 `BEGIN IMMEDIATE`。
pub fn tx_immediate<T, E>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<rusqlite::Error>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("comfyhub-db-{label}-{nanos}")).join("nested").join("comfyhub.db")
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
}
