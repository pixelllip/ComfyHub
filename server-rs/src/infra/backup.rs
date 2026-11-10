//! 库备份与恢复（2026-11-06）。
//!
//! # 备份 = `VACUUM INTO`
//!
//! 整个库是一个 SQLite 文件（26 张表：提示词 / 标签 / 角色卡 / 画风卡 / 会话 / 配方…），
//! 而 "另存一份" 最稳的写法是 [`VACUUM INTO`](https://sqlite.org/lang_vacuum.html)：
//! 它在一个**读事务**里把整库写成一个新文件 —— 不用停服务、不用锁表、产物是**没有 WAL 的
//! 单文件**（拷走就能用）。比 `fs::copy` 安全得多：后者会把正在写的 `-wal` 丢在外面，
//! 复制出来的主体可能差最后几个事务。
//!
//! # 恢复 = "先落标记，重启后换文件"
//!
//! **不在活着的连接上覆盖库**。原因：这个进程只有一条被 `Mutex` 护住的连接（`AppState.conn`），
//! 但 SQLite 还握着页缓存、WAL 与文件句柄；在别处把文件换掉，下一次查询读到的可能是
//! 两个库拼起来的页 —— 这类损坏**不报错**，等被发现时已经过去很多天。
//!
//! 所以恢复分两步：
//!
//! 1. **`restore_prepare`（后端活着时）**：校验备份（`integrity_check` + schema 版本 +
//!    必需的表），先用 `VACUUM INTO` 给**当前库**拍一张 `before-restore-*.db` 的保险，
//!    再把备份**复制**成 `<db>.restore-tmp`，最后原子写一个 `<db>.restore-pending.json` 标记；
//! 2. **`apply_pending`（后端下次启动、**开库之前**）**：看到标记就把 `<db>` 换成 tmp，
//!    删掉旧的 `-wal` / `-shm`，写一份 `<db>.restore-result.json`，删标记。
//!
//! 这样"换库"只发生在**没有任何连接**的时刻；App 那边只要在拿到 `restartRequired` 之后
//! 重启一次后端（设置页的「重启」走的就是同一条路）。
//!
//! ## 崩在中间怎么办
//!
//! `apply_pending` 是**幂等**的：tmp 在就换、tmp 不在就从备份重建；`<db>` 已经不在
//! （崩在两次 rename 之间）就直接把 tmp 改名过去；`<db>.restore-old` 还在就兜底搬回。
//! 标记只在**全部做完**之后才删 —— 任何一步崩掉，下次启动都会接着做完。
//!
//! # 只备份库，不备份产物
//!
//! `storage/media` 可能有几十 GB，`VACUUM INTO` 只覆盖 `.db`。界面上必须说清楚：
//! 恢复只把"谁是谁"搬回去，原图仍在 `<根>\storage` 里（同一台机器上通常对得上；
//! 换机器恢复时画廊会出现"有记录、没文件"）。**不要**在这里悄悄去动 storage。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::infra::db;

/// 备份目录名（相对运行根目录）。
pub const DIR_NAME: &str = "backups";
/// 自动备份的前缀（`comfyhub-<时间戳>.db`）。
pub const AUTO_PREFIX: &str = "comfyhub-";
/// 恢复前保险备份的前缀。
pub const BEFORE_PREFIX: &str = "before-restore-";
/// 自动备份保留份数（按 mtime 从新到旧）。
pub const KEEP_AUTO: usize = 10;
/// 保险备份保留份数。其重要性高于自动备份（可能恢复了错误的版本），但不能无限累积。
pub const KEEP_BEFORE: usize = 5;

/// 一条备份的元信息（`GET /api/system/backups` 的 `items[]`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub bytes: u64,
    pub created_at: String,
    /// `auto`（手动/自动备份）或 `beforeRestore`（恢复前的保险）。
    pub kind: &'static str,
}

/// 上一次恢复的结果（`<db>.restore-result.json`）—— 给界面一句**如实的**话。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRestore {
    pub ok: bool,
    pub name: String,
    pub message: String,
    pub at: String,
}

/// `restore_prepare` 成功之后交给界面的东西。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    /// 要恢复的那份备份（文件名）。
    pub name: String,
    /// 恢复前自动拍的那张保险（文件名）。**一定存在** —— 拍不出来就不许恢复
    /// （没有退路的恢复不该开始）。
    pub safety: String,
}

/// 标记文件的内容。**存在 `<db>.restore-pending.json`**，不放运行根目录：
/// 它描述的是"这一个库文件"的恢复，跟 root 是谁无关（测试 / `--db` 指向别处同样成立）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    name: String,
    /// 备份文件的绝对路径（tmp 丢了就靠它重建）。
    source: String,
    /// 已经复制好的新库（`<db>.restore-tmp`）。
    tmp: String,
    created_at: String,
}

/// `<根>\backups`（不存在就建）。
pub fn dir(project_root: &Path) -> PathBuf {
    project_root.join(DIR_NAME)
}

fn ensure_dir(project_root: &Path) -> Result<PathBuf, String> {
    let dir = dir(project_root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("建备份目录失败（{}）：{e}", dir.display()))?;
    Ok(dir)
}

fn marker_path(db_path: &Path) -> PathBuf {
    sibling(db_path, "restore-pending.json")
}

fn result_path(db_path: &Path) -> PathBuf {
    sibling(db_path, "restore-result.json")
}

fn tmp_path(db_path: &Path) -> PathBuf {
    sibling(db_path, "restore-tmp")
}

fn old_path(db_path: &Path) -> PathBuf {
    sibling(db_path, "restore-old")
}

/// `<db>` 同目录下的一个兄弟文件（`comfyhub.db` → `comfyhub.db.restore-tmp`）。
fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let mut name = db_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".");
    name.push(suffix);
    db_path.with_file_name(name)
}

/// 时间戳（**UTC**，`20261106-131211`）—— 既做文件名也做排序键。
fn stamp_now() -> String {
    let iso = crate::infra::iso_time::from_system_time(SystemTime::now());
    let head = iso.split('.').next().unwrap_or(&iso).trim_end_matches('Z');
    head.replace(['-', ':'], "").replace('T', "-")
}

fn iso_now() -> String {
    crate::infra::iso_time::from_system_time(SystemTime::now())
}

/// 文件名白名单：**只认一个段**、只认 `[A-Za-z0-9._-]`、必须以 `.db` 结尾。
///
/// 这是 `/api/system/backups/{name}` 与 `restore.body.name` 共用的唯一判据 ——
/// 少一条 `..` 或一个 `/`，它就成了"任意文件读取 / 任意文件覆盖"。
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.ends_with(".db")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !name.contains("..")
}

/// 列出备份（新的在前）。**只列我们自己命名的两种前缀**，别把目录里别的东西当成备份。
pub fn list(project_root: &Path) -> Result<Vec<Item>, String> {
    let dir = dir(project_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items: Vec<(SystemTime, Item)> = Vec::new();
    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("读备份目录失败（{}）：{e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(kind) = kind_of(name) else { continue };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        items.push((
            mtime,
            Item {
                name: name.to_string(),
                bytes: meta.len(),
                created_at: crate::infra::iso_time::from_system_time(mtime),
                kind,
            },
        ));
    }
    items.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    Ok(items.into_iter().map(|(_, item)| item).collect())
}

fn kind_of(name: &str) -> Option<&'static str> {
    if !is_safe_name(name) {
        return None;
    }
    if name.starts_with(AUTO_PREFIX) {
        Some("auto")
    } else if name.starts_with(BEFORE_PREFIX) {
        Some("beforeRestore")
    } else {
        None
    }
}

/// 拍一张备份。返回刚写出来的那一条（`kind` 由前缀决定）。
pub fn create(conn: &Connection, project_root: &Path) -> Result<Item, String> {
    create_with_prefix(conn, project_root, AUTO_PREFIX)
}

fn create_with_prefix(
    conn: &Connection,
    project_root: &Path,
    prefix: &str,
) -> Result<Item, String> {
    let dir = ensure_dir(project_root)?;
    let stamp = stamp_now();
    let mut seq = 0;
    let path = loop {
        let name = if seq == 0 {
            format!("{prefix}{stamp}.db")
        } else {
            format!("{prefix}{stamp}-{seq}.db")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            break candidate;
        }
        seq += 1;
        if seq > 99 {
            return Err("同一秒里备份太多次，换一秒再试".into());
        }
    };
    // 路径用参数绑定，别拼 SQL：Windows 路径里有反斜杠与盘符冒号
    conn.execute("VACUUM INTO ?1", rusqlite::params![path.to_string_lossy()])
        .map_err(|e| format!("写备份失败（{}）：{e}", path.display()))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("备份写完却读不到它：{e}"))?;
    let item = Item {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        bytes: meta.len(),
        created_at: crate::infra::iso_time::from_system_time(
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        ),
        kind: kind_of(&path.file_name().unwrap_or_default().to_string_lossy()).unwrap_or("auto"),
    };
    prune(project_root, prefix)?;
    Ok(item)
}

/// 只留最新的 N 份（同前缀）。按 **mtime** 排，不按文件名 —— 同一秒里带 `-2` 后缀的
/// 那份名字反而更小，按名字排会把最新的删掉。
fn prune(project_root: &Path, prefix: &str) -> Result<(), String> {
    let keep = if prefix == BEFORE_PREFIX {
        KEEP_BEFORE
    } else {
        KEEP_AUTO
    };
    let dir = dir(project_root);
    let mut entries: Vec<(SystemTime, PathBuf)> = Vec::new();
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if kind_of(name).is_none() || !name.starts_with(prefix) {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        entries.push((mtime, path));
    }
    entries.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    for (_, path) in entries.into_iter().skip(keep) {
        // 删不掉不是致命错误（可能被杀毒软件占着），下次再删
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// 删一份备份。返回 `false` = 本来就没有这一份（端点据此回 404，而不是假装成功）。
pub fn delete(project_root: &Path, name: &str) -> Result<bool, String> {
    if !is_safe_name(name) {
        return Err("备份名不合法".into());
    }
    let path = dir(project_root).join(name);
    if !path.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| format!("删不掉 {name}：{e}"))?;
    Ok(true)
}

/// 检查一个文件是不是"我们认得的一份备份"。
///
/// 三条判据缺一不可：
/// * `integrity_check` 必须是 `ok`（坏文件宁可当场说，也不能等换进去之后才炸）；
/// * `user_version` 不能**高于**本进程的 [`db::SCHEMA_VERSION`]（新版本写的库，老版本读不懂；
///   反过来允许 —— 老备份会被 `open_or_create` 正常迁移上来）；
/// * 必须有 `prompts` / `media_assets` 这两张表（一个空的 SQLite 文件 user_version 也是 0，
///   只靠版本号分不出"空库"和"很老的库"）。
fn inspect(path: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打不开这份备份：{e}"))?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| {
            // 把 "file is not a database" 翻成人话：这个文件根本不是 SQLite 库
            // （从其他位置复制的 .db / 被其他程序截断的文件，两种情况都归入此处）
            if e.to_string().contains("not a database") {
                "这不是一个 SQLite 数据库（文件头不对）".to_string()
            } else {
                format!("读 integrity_check 失败：{e}")
            }
        })?;
    if integrity != "ok" {
        return Err(format!("这份备份是坏的（{integrity}）"));
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("读版本号失败：{e}"))?;
    if version > db::SCHEMA_VERSION {
        return Err(format!(
            "这份备份来自更新的版本（库 v{version}，本程序只认得 v{}），请先升级程序",
            db::SCHEMA_VERSION
        ));
    }
    if version <= 0 {
        return Err("这不像 ComfyHub 的库（版本号是 0）".into());
    }
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('prompts','media_assets')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("读表清单失败：{e}"))?;
    if tables != 2 {
        return Err("这不像 ComfyHub 的库（缺少 prompts / media_assets 表）".into());
    }
    Ok(())
}

/// 准备恢复：校验 → 拍保险 → 复制成 tmp → 落标记。**不动当前库**。
pub fn restore_prepare(
    conn: &Connection,
    project_root: &Path,
    db_path: &Path,
    name: &str,
) -> Result<Prepared, String> {
    if !is_safe_name(name) {
        return Err("备份名不合法".into());
    }
    if db_path == Path::new(":memory:") {
        return Err("当前用的是内存库，没法恢复（没有可替换的文件）".into());
    }
    let source = dir(project_root).join(name);
    if !source.is_file() {
        return Err(format!("没有这份备份：{name}"));
    }
    inspect(&source)?;

    // ① 保险：先保存"当前这份"。选择错误备份时这是唯一的回退点 ——
    //    它拍不出来（磁盘满 / 目录不可写）就不该继续：没有退路的恢复比不恢复更糟。
    let safety = create_with_prefix(conn, project_root, BEFORE_PREFIX)
        .map_err(|e| format!("恢复前的保险备份没写成，先解决磁盘问题再恢复（{e}）"))?;

    // ② 把备份复制成 tmp（**完整复制完**才写标记：标记一旦存在，启动时就会拿它换库）。
    let tmp = tmp_path(db_path);
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    std::fs::copy(&source, &tmp).map_err(|e| format!("复制备份失败：{e}"))?;
    // 复制长度对不上 = 磁盘写坏了，宁可当场失败
    let (a, b) = (
        std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0),
        std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0),
    );
    if a == 0 || a != b {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("备份复制不完整（{a} → {b} 字节）"));
    }

    // ③ 标记：存在即代表"下次启动把 tmp 换上去"。
    let pending = Pending {
        name: name.to_string(),
        source: source.to_string_lossy().to_string(),
        tmp: tmp.to_string_lossy().to_string(),
        created_at: iso_now(),
    };
    write_json_atomic(&marker_path(db_path), &pending)?;

    Ok(Prepared {
        name: name.to_string(),
        safety: safety.name,
    })
}

/// 有没有"等下次启动完成"的恢复；返回备份名（界面据此提示）。
pub fn pending(db_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(marker_path(db_path)).ok()?;
    let pending: Pending = serde_json::from_str(&raw).ok()?;
    Some(pending.name)
}

/// 上一次恢复的结果（没有就 `None`）。
pub fn last_restore(db_path: &Path) -> Option<LastRestore> {
    let raw = std::fs::read_to_string(result_path(db_path)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// **开库之前**调用：有标记就把它做完。
///
/// 返回 `Some(结果)` 表示这次启动真的处理了一次恢复（成功或失败都会写结果文件）。
/// 失败时**不抛出**：后端需正常启动 —— 无法启动时连"恢复失败"的提示都看不到。
pub fn apply_pending(db_path: &Path) -> Option<LastRestore> {
    let marker = marker_path(db_path);
    if !marker.exists() {
        return None;
    }
    let outcome = match std::fs::read_to_string(&marker)
        .map_err(|e| format!("读恢复标记失败：{e}"))
        .and_then(|raw| {
            serde_json::from_str::<Pending>(&raw).map_err(|e| format!("恢复标记坏了：{e}"))
        }) {
        Ok(pending) => match swap(db_path, &pending) {
            Ok(()) => LastRestore {
                ok: true,
                name: pending.name,
                message:
                    "已恢复这份备份。恢复前的库另存了一份 before-restore 备份，需要时可以再换回来。"
                        .into(),
                at: iso_now(),
            },
            Err(e) => LastRestore {
                ok: false,
                name: pending.name.clone(),
                message: format!("恢复没有完成，当前库没有改动。{e}"),
                at: iso_now(),
            },
        },
        Err(e) => LastRestore {
            ok: false,
            name: String::new(),
            message: format!("恢复没有完成，当前库没有改动。{e}"),
            at: iso_now(),
        },
    };
    // 结果文件写失败不影响"库已经换好"这个事实，但也别把标记留着让下次再换一遍
    let _ = write_json_atomic(&result_path(db_path), &outcome);
    let _ = std::fs::remove_file(&marker);
    eprintln!("[db] 恢复：{}", outcome.message);
    Some(outcome)
}

/// 真正换库。**幂等**：tmp 在就换，tmp 不在就从备份重建，`<db>` 不在就直接改名。
fn swap(db_path: &Path, pending: &Pending) -> Result<(), String> {
    let tmp = PathBuf::from(&pending.tmp);
    if !tmp.exists() {
        let source = PathBuf::from(&pending.source);
        if !source.is_file() {
            return Err(format!(
                "临时文件与备份文件都不见了（{}），请重新选一份备份",
                pending.name
            ));
        }
        std::fs::copy(&source, &tmp).map_err(|e| format!("复制备份失败：{e}"))?;
    }

    if db_path.exists() {
        let old = old_path(db_path);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(db_path, &old).map_err(|e| format!("先挪开当前库失败：{e}"))?;
        if let Err(e) = std::fs::rename(&tmp, db_path) {
            // 换失败就把旧库搬回去，别留下一个"库不见了"的现场
            let _ = std::fs::rename(&old, db_path);
            return Err(format!("换上新库失败：{e}"));
        }
        let _ = std::fs::remove_file(&old);
    } else {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建库目录失败：{e}"))?;
        }
        std::fs::rename(&tmp, db_path).map_err(|e| format!("放上新库失败：{e}"))?;
    }
    // 崩在两次 rename 之间（`db → restore-old` 之后）会留下这个文件；换成功了就收掉。
    // 不收的话下次列表 / 排查会看到一个"来历不明的大文件"，还会白占一份库的空间。
    let _ = std::fs::remove_file(old_path(db_path));
    remove_sidecars(db_path);
    Ok(())
}

/// 旧库的 `-wal` / `-shm` **必须跟着删**：它们属于被换掉的那份库，留着会被 SQLite
/// 当成新库的日志应用上去 —— 现场是"恢复完打开就报 database disk image is malformed"。
fn remove_sidecars(db_path: &Path) {
    for suffix in ["-wal", "-shm"] {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        let path = PathBuf::from(name);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败：{e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写临时文件失败（{}）：{e}", tmp.display()))?;
    // 用 copy 覆盖而不是"先删再 rename"：删掉之后崩在 rename 之前，标记就**丢了** ——
    // 会误认为恢复成功，而下次启动不会发生任何事。copy 最差只留下不完整 JSON（可被
    // `apply_pending` 认出来并如实报错），目标文件本身永远不会消失。
    std::fs::copy(&tmp, path).map_err(|e| format!("替换文件失败（{}）：{e}", path.display()))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
