//! 把**另一份库**里的用户内容合并进当前库（命令行：`--merge-library <源数据目录>`）。
//!
//! # 为什么要有它（2026-11-10）
//!
//! 用户手上常常不止一份库：换过数据目录、从旧构建里拷出来过一份、机器上还有发布包残留的
//! `.db\` / `storage\`。以前这些库只能二选一 —— 选了一份，另一份里的会话 / 画廊 / 角色卡
//! 就等于"看不见了"（东西还在磁盘上，但界面上永远出不来）。
//!
//! 合并的判据只有一条：**只增不减**。源库一个字节都不动（只读打开），目标库先自动备份，
//! 所有写入在一个事务里，`PRAGMA foreign_key_check` 不干净就整体回滚。
//!
//! # 合并什么、不合并什么
//!
//! | 合并（用户内容） | 不合并（本机配置 / 账本） |
//! | --- | --- |
//! | 提示词 + 标签 + 产物 + 集合 + 捕获记录 | `app_settings`（用户当前设置说了算） |
//! | 会话 / 消息 / 有序块 / 运行 / 事件 / 工具调用 / 提问 | `ai_providers` / `ai_models`（本机凭据与模型目录） |
//! | 角色卡 / 风格卡（含例图关系） | `_sync_log` / `sync_clients`（同步游标，合并会让手机重新拉，没必要） |
//! | 工作流配方 / 提交队列 | `sqlite_sequence`（自增，由 SQLite 自己维护） |
//!
//! # 三条纪律（改这个文件之前先读）
//!
//! 1. **不删、不覆盖**：目标库已有的行一律保留；文件同名但内容不同时**不改写**，
//!    只记一条"问题"，让人来决定。
//! 2. **判重靠内容，不靠 id**：id 是各库自己发的号，两份库的 `tags.id = 1` 完全可能是
//!    两个标签。所以先按"标签名 / 内容 SHA-256 / 会话 id"找等价行，找不到才分配新号。
//! 3. **重复跑是安全的**：同一份源库合两次不会翻倍（等价行直接跳过并复用它的号）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::infra::db;

/// 要合并的表（**顺序即插入顺序**，父表必须排在子表前面，否则外键会拦）。
///
/// `dedupe` 是"等价行"的判据列：源行在这几列上与目标库某行完全相同 → 视为同一个东西，
/// 复用目标那行的 id。`refs` 是外键列 → 引用表（本模块据此重写源 id）。
const TABLES: &[TableSpec] = &[
    TableSpec {
        name: "tags",
        id: Some("id"),
        int_id: true,
        dedupe: &["normalized"],
        refs: &[],
        files: Files::None,
    },
    TableSpec {
        name: "prompts",
        id: Some("id"),
        int_id: true,
        dedupe: &["source", "source_ref"],
        refs: &[],
        files: Files::None,
    },
    TableSpec {
        name: "prompt_tags",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[("prompt_id", "prompts"), ("tag_id", "tags")],
        files: Files::None,
    },
    TableSpec {
        name: "media_collections",
        id: Some("id"),
        int_id: true,
        dedupe: &["group_key"],
        refs: &[],
        files: Files::None,
    },
    TableSpec {
        name: "media_assets",
        id: Some("id"),
        int_id: true,
        dedupe: &["sha256", "original_name"],
        refs: &[("prompt_id", "prompts"), ("collection_id", "media_collections")],
        files: Files::Media,
    },
    TableSpec {
        name: "media_tags",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[("media_id", "media_assets"), ("tag_id", "tags")],
        files: Files::None,
    },
    TableSpec {
        name: "capture_runs",
        id: Some("id"),
        int_id: true,
        dedupe: &["run_key"],
        refs: &[("prompt_id", "prompts")],
        files: Files::None,
    },
    TableSpec {
        name: "character_cards",
        id: Some("id"),
        int_id: true,
        dedupe: &["normalized"],
        refs: &[("reference_media_id", "media_assets")],
        files: Files::None,
    },
    TableSpec {
        name: "character_card_examples",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[("card_id", "character_cards"), ("media_id", "media_assets")],
        files: Files::None,
    },
    TableSpec {
        name: "style_cards",
        id: Some("id"),
        int_id: true,
        dedupe: &["normalized"],
        refs: &[],
        files: Files::None,
    },
    TableSpec {
        name: "style_card_examples",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[("card_id", "style_cards"), ("media_id", "media_assets")],
        files: Files::None,
    },
    TableSpec {
        name: "workflow_recipes",
        id: Some("id"),
        int_id: true,
        dedupe: &["name", "skeleton_hash"],
        refs: &[("prompt_id", "prompts")],
        files: Files::None,
    },
    TableSpec {
        name: "ai_conversations",
        id: Some("id"),
        int_id: false,
        dedupe: &[],
        refs: &[],
        files: Files::None,
    },
    TableSpec {
        name: "ai_messages",
        id: Some("id"),
        int_id: false,
        dedupe: &[],
        refs: &[("conversation_id", "ai_conversations")],
        files: Files::None,
    },
    TableSpec {
        name: "ai_attachments",
        id: Some("id"),
        int_id: false,
        dedupe: &["sha256", "original_name"],
        refs: &[],
        files: Files::Attachment,
    },
    TableSpec {
        name: "ai_runs",
        id: Some("id"),
        int_id: false,
        dedupe: &[],
        refs: &[
            ("conversation_id", "ai_conversations"),
            ("user_message_id", "ai_messages"),
            ("assistant_message_id", "ai_messages"),
        ],
        files: Files::None,
    },
    TableSpec {
        name: "ai_tool_calls",
        id: Some("id"),
        int_id: false,
        dedupe: &[],
        refs: &[("run_id", "ai_runs")],
        files: Files::None,
    },
    TableSpec {
        name: "ai_message_parts",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[
            ("message_id", "ai_messages"),
            ("attachment_id", "ai_attachments"),
            ("tool_call_id", "ai_tool_calls"),
        ],
        files: Files::None,
    },
    TableSpec {
        name: "ai_run_events",
        id: None,
        int_id: false,
        dedupe: &[],
        refs: &[("run_id", "ai_runs")],
        files: Files::None,
    },
    TableSpec {
        name: "ai_questions",
        id: Some("question_id"),
        int_id: false,
        dedupe: &[],
        refs: &[("run_id", "ai_runs"), ("conversation_id", "ai_conversations")],
        files: Files::None,
    },
    TableSpec {
        name: "submit_jobs",
        id: Some("queue_id"),
        int_id: false,
        dedupe: &[],
        refs: &[
            ("conversation_id", "ai_conversations"),
            ("run_id", "ai_runs"),
            ("call_id", "ai_tool_calls"),
            ("prompt_id", "prompts"),
            ("reply_message_id", "ai_messages"),
        ],
        files: Files::None,
    },
];

struct TableSpec {
    name: &'static str,
    /// 单列主键的列名（`None` = 复合主键，只做 `INSERT OR IGNORE`）。
    id: Option<&'static str>,
    /// 主键是整数（各库自己发的号，必须重编）；`false` 时是 UUID / 文本 id（原样保留）。
    int_id: bool,
    /// 等价行的判据列（全非空才比）。
    dedupe: &'static [&'static str],
    /// 外键列 → 引用表。
    refs: &'static [(&'static str, &'static str)],
    /// 这一行还要搬哪些文件。
    files: Files,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Files {
    None,
    Media,
    Attachment,
}

impl Files {
    /// 原件 / 缩略图各自落哪个子目录（与 `media::attachments::attachment_dir` 一致）。
    fn subs(self) -> Option<(&'static str, &'static str)> {
        match self {
            Files::None => None,
            Files::Media => Some(("media", "thumbs")),
            Files::Attachment => Some(("ai-attachments", "ai-thumbs")),
        }
    }
}

/// 一次合并要做的事。
pub struct MergeRequest {
    /// 目标库文件（现在正在用的那份 `comfyhub.db`）。
    pub target_db: PathBuf,
    /// 目标产物目录（`<数据目录>/storage`；扁平布局时就是数据目录本身）。
    pub target_storage: PathBuf,
    /// 源：数据目录（里面要有 `.db/comfyhub.db` 与 `storage/`）。
    pub sources: Vec<PathBuf>,
    /// 额外找文件的 storage 根（文件已经不跟着库走的时候用，比如构建残留里的 `storage/`）。
    pub media_from: Vec<PathBuf>,
    /// 备份放哪儿；`None` = `<库文件所在目录>/backups`。
    pub backup_dir: Option<PathBuf>,
    /// 只演练：跑完整体回滚，一个字节都不写（文件也不搬）。
    pub dry_run: bool,
}

impl std::fmt::Debug for MergeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeRequest")
            .field("target_db", &self.target_db)
            .field("target_storage", &self.target_storage)
            .field("sources", &self.sources)
            .field("media_from", &self.media_from)
            .field("dry_run", &self.dry_run)
            .finish()
    }
}

/// 合并结果（命令行直接 `render()`；单测断言这里的数字）。
#[derive(Debug, Default)]
pub struct MergeReport {
    pub tables: BTreeMap<String, TableStat>,
    pub files: FileStat,
    pub problems: Vec<String>,
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct TableStat {
    /// 新插入的行。
    pub inserted: u64,
    /// 目标库里已有等价行（内容判重命中）而跳过的行。
    pub deduped: u64,
    /// 主键在目标库里已被别的行占用、换了新号的整数 id（例：标签重新编号）。
    pub renumbered: u64,
    /// 因为父行没进来而丢掉的关联行（`prompt_tags` 这种复合表）。
    pub orphan: u64,
}

#[derive(Debug, Default, Clone)]
pub struct FileStat {
    pub copied: u64,
    pub already_there: u64,
    pub missing: u64,
}

impl MergeReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("\n合并结果\n");
        out.push_str("────────────────────────────────────────────\n");
        for (name, st) in &self.tables {
            if st.inserted == 0 && st.deduped == 0 && st.renumbered == 0 && st.orphan == 0 {
                continue;
            }
            out.push_str(&format!(
                "  {:<24} 新增 {:<5} 已存在 {:<5} 改了号 {:<4} 孤立 {}\n",
                name, st.inserted, st.deduped, st.renumbered, st.orphan
            ));
        }
        if self.files.copied + self.files.already_there + self.files.missing > 0 {
            out.push_str(&format!(
                "  文件：复制 {}，本来就有 {}，找不到 {}\n",
                self.files.copied, self.files.already_there, self.files.missing
            ));
        }
        if let Some(backup) = &self.backup {
            out.push_str(&format!("  目标库已备份到 {}\n", backup.display()));
        }
        if self.problems.is_empty() {
            out.push_str("  没有问题。源库一个字节都没动。\n");
        } else {
            out.push_str(&format!("  ⚠ {} 个问题（见下）：\n", self.problems.len()));
            for p in &self.problems {
                out.push_str(&format!("    · {p}\n"));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
//  行 / 值的搬运
// ---------------------------------------------------------------------------

/// 一行的原样快照：列名 → 值（源库读出来就定下来了，不随目标库 schema 变）。
type Row = Vec<(String, Value)>;

/// 源 id → 目标 id 的映射（按表分开；每次合并一份源库就重来一遍）。
type IdMap = HashMap<&'static str, HashMap<String, Value>>;

fn value_key(v: &Value) -> Option<String> {
    match v {
        Value::Integer(n) => Some(n.to_string()),
        Value::Text(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn read_rows(conn: &Connection, table: &str) -> Result<Vec<Row>, String> {
    let sql = format!("SELECT * FROM \"{table}\"");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("读 {table} 失败：{e}"))?;
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut out = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| format!("读 {table} 失败：{e}"))?;
    while let Some(row) = rows.next().map_err(|e| format!("读 {table} 失败：{e}"))? {
        let mut values = Vec::with_capacity(columns.len());
        for (i, name) in columns.iter().enumerate() {
            let v: Value = row
                .get::<usize, Value>(i)
                .map_err(|e| format!("读 {table}.{name} 失败：{e}"))?;
            values.push((name.clone(), v));
        }
        out.push(values);
    }
    Ok(out)
}

/// 目标库这张表的列：`(列名, 是不是 NOT NULL)`。
fn table_info(conn: &Connection, table: &str) -> Result<Vec<(String, bool)>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|e| format!("读 {table} 的列失败：{e}"))?;
    let columns = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            let not_null: i64 = row.get(3)?;
            Ok((name, not_null != 0))
        })
        .map_err(|e| format!("读 {table} 的列失败：{e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读 {table} 的列失败：{e}"))?;
    Ok(columns)
}

fn exists(conn: &Connection, table: &str, id_col: &str, id: &Value) -> Result<bool, String> {
    let sql = format!("SELECT 1 FROM \"{table}\" WHERE \"{id_col}\" = ?1 LIMIT 1");
    conn.query_row(&sql, rusqlite::params![id], |_| Ok(()))
        .map(|_| true)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(format!("查 {table} 失败：{other}")),
        })
}

fn find_equivalent(
    conn: &Connection,
    spec: &TableSpec,
    row: &Row,
    available: &HashSet<String>,
) -> Result<Option<Value>, String> {
    let Some(id_col) = spec.id else {
        return Ok(None);
    };
    let mut where_parts: Vec<String> = Vec::new();
    let mut binds: Vec<Value> = Vec::new();
    for col in spec.dedupe {
        if !available.contains(*col) {
            return Ok(None);
        }
        let Some((_, value)) = row.iter().find(|(name, _)| name == col) else {
            return Ok(None);
        };
        // 判据列是空值：这一行没法判重（宁可多一份，也不误合并）
        if value_key(value).is_none() {
            return Ok(None);
        }
        where_parts.push(format!("\"{col}\" IS ?{}", binds.len() + 1));
        binds.push(value.clone());
    }
    if where_parts.is_empty() {
        return Ok(None);
    }
    let sql = format!(
        "SELECT \"{id_col}\" FROM \"{}\" WHERE {} LIMIT 1",
        spec.name,
        where_parts.join(" AND ")
    );
    conn.query_row(&sql, rusqlite::params_from_iter(binds.iter()), |r| {
        r.get::<usize, Value>(0)
    })
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(format!("查 {} 的等价行失败：{other}", spec.name)),
    })
}

fn max_int_id(conn: &Connection, table: &str, id_col: &str) -> Result<i64, String> {
    conn.query_row(
        &format!("SELECT COALESCE(MAX(\"{id_col}\"), 0) FROM \"{table}\""),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|e| format!("读 {table} 的最大 id 失败：{e}"))
}

// ---------------------------------------------------------------------------
//  入口
// ---------------------------------------------------------------------------

/// 执行合并。**目标库先备份**，所有写入一个事务，外键不自洽就整体回滚。
pub fn merge(req: &MergeRequest) -> Result<MergeReport, String> {
    if !req.target_db.is_file() {
        return Err(format!("目标库不存在：{}", req.target_db.display()));
    }
    if req.sources.is_empty() {
        return Err("没有给源库（--merge-library <源数据目录>）".into());
    }
    let mut report = MergeReport::default();

    // 源库先全部打开并校验：有一份读不了就**在写之前**退出
    let mut sources: Vec<(PathBuf, Connection, PathBuf)> = Vec::new();
    for dir in &req.sources {
        let (db_path, storage) = source_layout(dir)?;
        let conn = db::open_readonly(&db_path)
            .map_err(|e| format!("打开源库 {} 失败（它是不是正在被别的程序写？）：{e}", db_path.display()))?;
        sources.push((dir.clone(), conn, storage));
    }

    // 备份（演练模式不写盘）
    if !req.dry_run {
        let backup_dir = req
            .backup_dir
            .clone()
            .or_else(|| req.target_db.parent().map(|p| p.join("backups")));
        if let Some(dir) = backup_dir {
            let stamp = crate::infra::iso_time::from_system_time(std::time::SystemTime::now())
                .replace([':', '.'], "-");
            let path = dir.join(format!("pre-merge-{stamp}.db"));
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("建备份目录 {} 失败：{e}", dir.display()))?;
            let conn = db::open(&req.target_db).map_err(|e| format!("打开目标库失败：{e}"))?;
            conn.execute("VACUUM INTO ?1", rusqlite::params![path.to_string_lossy()])
                .map_err(|e| {
                    format!(
                        "备份目标库到 {} 失败（后端是不是还在跑？先停掉它）：{e}",
                        path.display()
                    )
                })?;
            report.backup = Some(path);
        }
    }

    let conn = db::open(&req.target_db).map_err(|e| {
        format!(
            "打开目标库 {} 失败（后端是不是还在跑？先停掉它）：{e}",
            req.target_db.display()
        )
    })?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("拿写锁失败（后端是不是还在跑？）：{e}"))?;

    let outcome = (|| -> Result<(), String> {
        for (src_dir, src_conn, src_storage) in &sources {
            merge_one_source(
                &conn,
                src_dir,
                src_conn,
                src_storage,
                req,
                &mut report,
            )?;
        }
        // 外键自洽：不合就不提交（宁可整轮作废，也不把库留成半截）
        let bad = foreign_key_violations(&conn)?;
        if !bad.is_empty() {
            return Err(format!(
                "合并后外键不自洽（已整体回滚）：{}",
                bad.join("；")
            ));
        }
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            if req.dry_run {
                let _ = conn.execute_batch("ROLLBACK");
                report.problems.push("演练模式：所有写入已回滚，一个字节都没改".into());
            } else {
                conn.execute_batch("COMMIT")
                    .map_err(|e| format!("提交失败：{e}"))?;
            }
            Ok(report)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 源数据目录 → （库文件, 产物目录）。两种布局都认（与 `AGENTS §7` 一致）。
fn source_layout(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    if dir.is_file() {
        let parent = dir.parent().ok_or("源库路径没有父目录")?;
        let storage = parent
            .parent()
            .map(|p| p.join("storage"))
            .unwrap_or_else(|| parent.join("storage"));
        return Ok((dir.to_path_buf(), storage));
    }
    let db_path = dir.join(".db").join("comfyhub.db");
    if !db_path.is_file() {
        return Err(format!(
            "{} 里没有 .db\\comfyhub.db（--merge-library 要给**数据目录**）",
            dir.display()
        ));
    }
    Ok((db_path, dir.join("storage")))
}

fn merge_one_source(
    conn: &Connection,
    src_dir: &Path,
    src: &Connection,
    src_storage: &Path,
    req: &MergeRequest,
    report: &mut MergeReport,
) -> Result<(), String> {
    let mut id_map: IdMap = HashMap::new();
    let mut next_int: HashMap<&'static str, i64> = HashMap::new();
    // 插进来的行（按表）—— 搬文件时只认这些 + 判重命中的目标行
    let mut inserted: HashMap<&'static str, HashSet<String>> = HashMap::new();

    for spec in TABLES {
        let info = table_info(conn, spec.name)
            .map_err(|e| format!("{e}（目标库缺表 {0}？）", spec.name))?;
        let available: HashSet<String> = info.iter().map(|(n, _)| n.clone()).collect();
        let not_null: HashSet<String> = info
            .iter()
            .filter(|(_, nn)| *nn)
            .map(|(n, _)| n.clone())
            .collect();
        if available.is_empty() {
            // 目标库里没有这张表（老版本的库）：**不静默跳过**，如实报出来
            let rows = read_rows(src, spec.name)?;
            if !rows.is_empty() {
                report.problems.push(format!(
                    "目标库里没有表 {}（源库有 {} 行），这部分没有合并",
                    spec.name,
                    rows.len()
                ));
            }
            continue;
        }
        let rows = read_rows(src, spec.name)?;
        // 源库有、目标库没有的列：**丢掉要如实说**（源库还留着，别让人以为丢数据了）
        if let Some(first) = rows.first() {
            let dropped: Vec<String> = first
                .iter()
                .map(|(n, _)| n.clone())
                .filter(|n| !available.contains(n))
                .collect();
            if !dropped.is_empty() {
                report.problems.push(format!(
                    "目标库的 {} 没有这些列，本次没搬（源库仍保留）：{}",
                    spec.name,
                    dropped.join("、")
                ));
            }
        }
        let stat = report.tables.entry(spec.name.to_string()).or_default();
        // 悬空引用按列归并计数（**不逐行报**，否则一份老库能刷出上百条）
        let mut dangling: BTreeMap<String, u64> = BTreeMap::new();

        for row in rows {
            // 源行里目标库没有的列：丢了要如实说（比如源库比目标库新）
            let mut cols: Vec<(String, Value)> = Vec::new();
            for (name, value) in &row {
                if available.contains(name) {
                    cols.push((name.clone(), value.clone()));
                }
            }
            if cols.is_empty() {
                continue;
            }

            // 1) 找出这一行在目标库里的 id（可能是"等价行"复用，也可能是新号）
            let (target_id, deduped) = match spec.id {
                None => (None, false),
                Some(id_col) => {
                    let Some((_, raw_id)) = row.iter().find(|(n, _)| n == id_col) else {
                        continue;
                    };
                    if spec.int_id {
                        let src_id = match raw_id {
                            Value::Integer(n) => *n,
                            _ => continue,
                        };
                        // 先看有没有等价行（标签名 / 内容哈希 …）
                        if let Some(existing) = find_equivalent(conn, spec, &row, &available)? {
                            (
                                Some(existing.clone()),
                                true,
                            )
                        } else if exists(conn, spec.name, id_col, &Value::Integer(src_id))? {
                            // 号被目标库里**别的**行占着 → 换个新号
                            let next = next_int
                                .entry(spec.name)
                                .or_insert(max_int_id(conn, spec.name, id_col)? + 1);
                            let id = Value::Integer(*next);
                            *next += 1;
                            stat.renumbered += 1;
                            (Some(id), false)
                        } else {
                            next_int
                                .entry(spec.name)
                                .or_insert(max_int_id(conn, spec.name, id_col)? + 1);
                            (Some(Value::Integer(src_id)), false)
                        }
                    } else {
                        // 文本 id（UUID）：同一行就是同一个对象 → 跳过
                        if exists(conn, spec.name, id_col, raw_id)? {
                            (Some(raw_id.clone()), true)
                        } else {
                            (Some(raw_id.clone()), false)
                        }
                    }
                }
            };

            // 2) 外键重写成目标库的号；主键换成我们算出来的那个号（可能改过）
            let mut mapped_row: Vec<(String, Value)> = Vec::new();
            let mut broken_ref = false;
            for (name, value) in &cols {
                let mut value = value.clone();
                if let (Some(id_col), Some(tid)) = (spec.id, &target_id) {
                    if name == id_col {
                        value = tid.clone();
                    }
                }
                if let Some((_, ref_table)) = spec.refs.iter().find(|(c, _)| c == name) {
                    if let Some(key) = value_key(&value) {
                        let mapped = id_map.get(ref_table).and_then(|m| m.get(&key));
                        match mapped {
                            Some(m) => value = m.clone(),
                            None => {
                                // 引用了一份没进来的行。**NOT NULL 的只能丢这一行**；
                                // 可空的（比如老数据把 `fc_...` 塞进 `tool_call_id`）就置空
                                // 保留 —— 行里的正文 / 有序块 / 结果 JSON 还在，丢了才是真丢数据。
                                if not_null.contains(name) {
                                    broken_ref = true;
                                    break;
                                }
                                *dangling.entry(name.clone()).or_insert(0) += 1;
                                value = Value::Null;
                            }
                        }
                    }
                }
                mapped_row.push((name.clone(), value));
            }
            if broken_ref {
                stat.orphan += 1;
                continue;
            }

            if deduped {
                // 等价行已在目标库里：记住号（子行要用），但不再插入
                stat.deduped += 1;
                if let (Some(id_col), Some(tid)) = (spec.id, &target_id) {
                    if let Some(key) = row
                        .iter()
                        .find(|(n, _)| n == id_col)
                        .and_then(|(_, v)| value_key(v))
                    {
                        id_map.entry(spec.name).or_default().insert(key, tid.clone());
                    }
                }
                continue;
            }

            // 3) 写进去（主键我们自己填，别让 SQLite 再发一个号）
            let insert_cols: Vec<String> = mapped_row.iter().map(|(n, _)| n.clone()).collect();
            let placeholders: Vec<String> = (1..=insert_cols.len())
                .map(|i| format!("?{i}"))
                .collect();
            let verb = if spec.id.is_none() {
                "INSERT OR IGNORE"
            } else {
                "INSERT"
            };
            let sql = format!(
                "{verb} INTO \"{}\" ({}) VALUES ({})",
                spec.name,
                insert_cols
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                placeholders.join(", ")
            );
            let values: Vec<Value> = mapped_row.iter().map(|(_, v)| v.clone()).collect();
            let changed = conn
                .execute(&sql, rusqlite::params_from_iter(values.iter()))
                .map_err(|e| {
                    format!(
                        "写 {} 失败：{e}（源行 {:?}）",
                        spec.name,
                        mapped_row
                            .iter()
                            .take(3)
                            .map(|(n, v)| format!("{n}={v:?}"))
                            .collect::<Vec<_>>()
                    )
                })?;
            if changed == 0 {
                // 复合表被 OR IGNORE 忽略 = 这条关系已经在目标库里
                stat.deduped += 1;
                continue;
            }
            stat.inserted += 1;
            if let Some(id_col) = spec.id {
                if let Some((_, tid)) = mapped_row.iter().find(|(n, _)| n == id_col) {
                    if let Some(key) = value_key(tid) {
                        id_map.entry(spec.name).or_default().insert(
                            row.iter()
                                .find(|(n, _)| n == id_col)
                                .and_then(|(_, v)| value_key(v))
                                .unwrap_or_default(),
                            tid.clone(),
                        );
                        inserted.entry(spec.name).or_default().insert(key);
                    }
                }
            }
        }
        // 悬挂引用统一在这里说一句（源库自己就不自洽时最常见）
        for (column, count) in dangling {
            report.problems.push(format!(
                "{} 有 {count} 行的 {column} 指向不存在的行，已置空保留（旁边的内容没丢）",
                spec.name
            ));
        }
    }

    // 4) 文件：产物原件 + 缩略图（缩略图名就是行的 id，号变了要跟着改名）
    if !req.dry_run {
        copy_files(src, src_storage, src_dir, req, &id_map, &inserted, report)?;
    } else {
        // 演练也如实报"有多少文件缺着"，但一个字节都不写
        count_files(src, src_storage, src_dir, req, report)?;
    }

    Ok(())
}

/// 只读"文件相关的那几列"（别把整份 `workflow_json` 拖进内存）。
fn file_rows(src: &Connection, table: &str) -> Result<Vec<Row>, String> {
    let sql = format!("SELECT id, stored_name, sha256 FROM \"{table}\"");
    match read_rows_with(src, &sql) {
        Ok(rows) => Ok(rows),
        Err(_) => {
            // `submit_jobs` 这类没有 `id` 列的表（理论上不该走到这儿）：退化成全量
            read_rows(src, table)
        }
    }
}

fn read_rows_with(conn: &Connection, sql: &str) -> Result<Vec<Row>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("读失败：{e}"))?;
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut out = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| format!("读失败：{e}"))?;
    while let Some(row) = rows.next().map_err(|e| format!("读失败：{e}"))? {
        let mut values = Vec::with_capacity(columns.len());
        for (i, name) in columns.iter().enumerate() {
            let v: Value = row
                .get::<usize, Value>(i)
                .map_err(|e| format!("读 {name} 失败：{e}"))?;
            values.push((name.clone(), v));
        }
        out.push(values);
    }
    Ok(out)
}

/// 把源库里的产物 / 附件文件搬到目标 storage（**只补缺，不覆盖**）。
#[allow(clippy::too_many_arguments)]
fn copy_files(
    src: &Connection,
    src_storage: &Path,
    src_dir: &Path,
    req: &MergeRequest,
    id_map: &IdMap,
    inserted: &HashMap<&'static str, HashSet<String>>,
    report: &mut MergeReport,
) -> Result<(), String> {
    for spec in TABLES.iter().filter(|t| t.files != Files::None) {
        let Some((orig_sub, thumb_sub)) = spec.files.subs() else {
            continue;
        };
        let Some(id_col) = spec.id else { continue };
        let rows = file_rows(src, spec.name)?;
        for row in rows {
            let get = |name: &str| -> Option<&Value> {
                row.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            };
            let Some(stored) = get("stored_name").and_then(|v| match v {
                Value::Text(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            }) else {
                continue;
            };
            let Some(src_id) = get(id_col).and_then(value_key) else {
                continue;
            };
            let dst_id = id_map
                .get(spec.name)
                .and_then(|m| m.get(&src_id))
                .and_then(value_key)
                .unwrap_or_else(|| src_id.clone());
            let was_inserted = inserted
                .get(spec.name)
                .is_some_and(|s| s.contains(&dst_id));

            // 原件：源 storage → 目标 storage（找不到就去 --media-from 里翻）
            let dst_orig = req.target_storage.join(orig_sub).join(&stored);
            if dst_orig.is_file() {
                report.files.already_there += 1;
                if was_inserted {
                    // 同名但内容不同：**不改写原件**（宁可报出来，也不覆盖任何一边）
                    if let Some(found) =
                        locate_file(&stored, orig_sub, src_storage, &req.media_from, src_dir)
                    {
                        if !files_equal(&found, &dst_orig) {
                            report.problems.push(format!(
                                "{stored} 在目标库里已经有一份**内容不同**的文件，已保留目标那份；\
                                 源库那份在 {}",
                                found.display()
                            ));
                        }
                    }
                }
            } else {
                match locate_file(&stored, orig_sub, src_storage, &req.media_from, src_dir) {
                    Some(found) => {
                        if let Some(dir) = dst_orig.parent() {
                            std::fs::create_dir_all(dir)
                                .map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
                        }
                        std::fs::copy(&found, &dst_orig)
                            .map_err(|e| format!("复制 {} 失败：{e}", found.display()))?;
                        verify_sha(&dst_orig, get("sha256"), report, &stored);
                        report.files.copied += 1;
                    }
                    None => {
                        report.files.missing += 1;
                        report.problems.push(format!(
                            "{} #{dst_id} 的文件 {stored} 在所有来源里都找不到",
                            spec.name
                        ));
                    }
                }
            }

            // 缩略图：`<行 id>.jpg`（号变了就要改名）。缺了也补上（首屏不用等现生成）；
            // 目标库里**同名但内容不同**（旧的孤儿缩略图）时用源库这份覆盖 —— 缩略图是
            // 派生数据，覆盖它不丢任何东西。
            let dst_thumb = req
                .target_storage
                .join(thumb_sub)
                .join(format!("{dst_id}.jpg"));
            // 补缺（重跑一遍也能把上一轮缺的补上）；已存在的那份不碰，除非这行是这次新插的
            if was_inserted || !dst_thumb.is_file() {
                let src_thumb_name = format!("{src_id}.jpg");
                let found = locate_file(
                    &src_thumb_name,
                    thumb_sub,
                    src_storage,
                    &req.media_from,
                    src_dir,
                );
                if let Some(found) = found {
                    let same = dst_thumb.is_file() && files_equal(&found, &dst_thumb);
                    if !same {
                        if let Some(dir) = dst_thumb.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        if std::fs::copy(&found, &dst_thumb).is_ok() {
                            report.files.copied += 1;
                        }
                    }
                } else if !dst_thumb.is_file() && was_inserted {
                    // 源库也没有：如实记一笔（界面上会按需现生成，不算丢数据）
                    report.problems.push(format!(
                        "{} #{dst_id} 的缩略图源库里也没有，界面会按需现生成",
                        spec.name
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 演练模式：只数"哪些文件缺着"，不写盘。
fn count_files(
    src: &Connection,
    src_storage: &Path,
    src_dir: &Path,
    req: &MergeRequest,
    report: &mut MergeReport,
) -> Result<(), String> {
    for spec in TABLES.iter().filter(|t| t.files != Files::None) {
        let Some((orig_sub, _)) = spec.files.subs() else {
            continue;
        };
        let rows = file_rows(src, spec.name)?;
        for row in rows {
            let get = |name: &str| -> Option<&Value> {
                row.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            };
            let Some(stored) = get("stored_name").and_then(|v| match v {
                Value::Text(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            }) else {
                continue;
            };
            if req.target_storage.join(orig_sub).join(&stored).is_file() {
                report.files.already_there += 1;
            } else if locate_file(&stored, orig_sub, src_storage, &req.media_from, src_dir).is_some()
            {
                report.files.copied += 1;
            } else {
                report.files.missing += 1;
                report.problems.push(format!(
                    "{} 的文件 {} 在所有来源里都找不到",
                    spec.name, stored
                ));
            }
        }
    }
    Ok(())
}

/// 在所有可能的来源里找一份文件（源 storage → 构建残留 → `--media-from`）。
fn locate_file(
    name: &str,
    sub: &str,
    src_storage: &Path,
    media_from: &[PathBuf],
    src_dir: &Path,
) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![src_storage.to_path_buf()];
    roots.extend(media_from.iter().cloned());
    // 源库自己上一级 / 下一级的 storage（旧版把 storage 套了两层）
    roots.push(src_dir.join("storage").join("storage"));
    roots.push(src_dir.join("storage"));
    if let Some(parent) = src_dir.parent() {
        roots.push(parent.join("storage"));
    }
    for root in roots {
        for candidate in [
            root.join(sub).join(name),
            root.join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 两份文件是不是逐字节一样（大小不同直接不同，别读整份）。
fn files_equal(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(ba), Ok(bb)) => ba == bb,
        _ => false,
    }
}

fn verify_sha(path: &Path, expected: Option<&Value>, report: &mut MergeReport, name: &str) {    let Some(Value::Text(expected)) = expected else {
        return;
    };
    if expected.is_empty() {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != *expected {
        report.problems.push(format!(
            "文件 {name} 的 SHA-256 与库里记的不一致（库里 {expected}，磁盘 {actual}）"
        ));
    }
}

fn foreign_key_violations(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("PRAGMA foreign_key_check")
        .map_err(|e| format!("外键自检失败：{e}"))?;
    let mut rows = stmt.query([]).map_err(|e| format!("外键自检失败：{e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| format!("外键自检失败：{e}"))? {
        let table: String = row.get(0).unwrap_or_default();
        let rowid: String = row.get(1).map(|v: i64| v.to_string()).unwrap_or_default();
        let parent: String = row.get(2).unwrap_or_default();
        out.push(format!("{table} 第 {rowid} 行的外键在 {parent} 里找不到"));
        if out.len() >= 5 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
