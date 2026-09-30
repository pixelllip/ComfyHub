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
/// 触发器 —— **不是加表但同样够不着老库**，理由写在 `db/schema.sqlite.sql` 末尾；
/// 同日由 3 提到 4：加 `submit_jobs` 表 = M5「手机排队 → 桌面真正提交」的持久账）。
pub const SCHEMA_VERSION: i64 = 4;

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
mod tests;
