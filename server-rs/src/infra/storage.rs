//! 存储占用与孤儿文件（2026-11-06）。
//!
//! # 为什么要有这一层
//!
//! 桌面端此前**看不到自己占了多少盘**：`storage/` 只会涨（产物、附件、缩略图、MCP 运行时），
//! `ai_run_events` 也只会涨，而手机端早就有"产物缓存"的预算 / 占用 / 清理三件套。
//! 这一层只做两件事：**算**（[`usage`]）和**找**（[`scan_orphans`]），删不删由界面确认。
//!
//! # 什么叫"孤儿"
//!
//! 磁盘上有、库里没有对应行的文件。来源只有三种：
//!
//! 1. 写完文件、还没写库就被杀 / 崩了；
//! 2. **恢复了更早的库备份** —— 备份之后新生成的产物在库里没有记录了；
//! 3. 用户手工往 `storage` 里拷过东西。
//!
//! 第 2 种最容易被误删，所以这里**只扫描不自动删**，而且**跳过 1 小时内改过的文件**
//! （正在上传 / 正在写的中转文件不该被当成孤儿）。界面上必须让用户看清数量与体积再确认。
//!
//! # 只认四个平铺目录
//!
//! `media` / `thumbs` / `ai-attachments` / `ai-thumbs`。它们各自对应一张表的
//! `stored_name` 或 `id`，判据清清楚楚。**不扫** `storage/ai`（Skills / 记忆 / MCP 运行时
//! 由各自的模块管）、也不扫 `storage/tmp`（中转文件有 24 小时的清理规矩，见
//! `attachments::collect_garbage`）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

/// 1 小时内动过的文件不碰（正在上传 / 正在写的中转文件）。
pub const GRACE: Duration = Duration::from_secs(60 * 60);

/// 一次最多列多少条（界面只画得下这么多；总量仍然照实给）。
pub const MAX_LIST: usize = 1000;

/// 一个目录的占用。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirUsage {
    pub key: &'static str,
    pub label: &'static str,
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

/// `GET /api/system/storage` 的磁盘那一半。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub root: String,
    pub dirs: Vec<DirUsage>,
    pub total_bytes: u64,
}

/// 一个孤儿文件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Orphan {
    /// `media` / `thumb` / `attachment` / `attachmentThumb`
    pub kind: &'static str,
    pub name: String,
    pub bytes: u64,
    pub modified_at: String,
}

/// `POST /api/system/storage/orphans`（`apply=false`）的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanScan {
    pub items: Vec<Orphan>,
    pub count: u64,
    pub bytes: u64,
    /// 超过 [`MAX_LIST`] 时只列前 1000 条，`count` / `bytes` 仍然是全量。
    pub truncated: bool,
}

/// 删完之后的账：删了多少、还剩多少、哪几个没删掉。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub deleted: u64,
    pub bytes: u64,
    pub failed: Vec<FailedFile>,
    pub remaining: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedFile {
    pub name: String,
    pub error: String,
}

/// 算占用。**只看不删**，任何目录读不了都按 0 算（占用卡片不该因为一个目录权限问题整页报错）。
pub fn usage(root: &Path, storage: &Path, db_path: &Path) -> Usage {
    let mut dirs = Vec::new();

    // 数据库：`comfyhub.db` 与它的 -wal / -shm 都算（同一份数据的不同部分）
    let db_dir = db_path.parent().unwrap_or(root);
    let db_name = db_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "comfyhub.db".into());
    let mut db_bytes = 0u64;
    let mut db_files = 0u64;
    for (name, bytes, _) in flat_files(db_dir) {
        if name.starts_with(&db_name) {
            db_bytes += bytes;
            db_files += 1;
        }
    }
    dirs.push(DirUsage {
        key: "db",
        label: "数据库",
        path: db_dir.to_string_lossy().to_string(),
        bytes: db_bytes,
        files: db_files,
    });

    let mut push = |key: &'static str, label: &'static str, path: PathBuf| {
        let (bytes, files) = tree_size(&path);
        dirs.push(DirUsage {
            key,
            label,
            path: path.to_string_lossy().to_string(),
            bytes,
            files,
        });
    };
    push("backups", "库备份", root.join("backups"));
    push("media", "产物原图", storage.join("media"));
    push("thumbs", "产物缩略图", storage.join("thumbs"));
    push("aiAttachments", "对话附件", storage.join("ai-attachments"));
    push("aiThumbs", "附件缩略图", storage.join("ai-thumbs"));
    push(
        "aiData",
        "AI 数据（Skills / 记忆 / MCP 运行时）",
        storage.join("ai"),
    );
    push("run", "运行日志与端口记录", root.join(".run"));

    let total_bytes = dirs.iter().map(|d| d.bytes).sum();
    Usage {
        root: root.to_string_lossy().to_string(),
        dirs,
        total_bytes,
    }
}

/// 目录树的（字节数, 文件数）。不跟随目录符号链接（Windows 的 junction 可能成环）。
fn tree_size(dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let Ok(read) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in read.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let (b, f) = tree_size(&entry.path());
            bytes += b;
            files += f;
        } else if kind.is_file() {
            if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn flat_files(dir: &Path) -> Vec<(String, u64, SystemTime)> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push((
            entry.file_name().to_string_lossy().to_string(),
            meta.len(),
            meta.modified().unwrap_or(UNIX_EPOCH),
        ));
    }
    out
}

fn load_set(conn: &Connection, sql: &str) -> Result<HashSet<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("读库失败：{e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("读库失败：{e}"))?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row.map_err(|e| format!("读库失败：{e}"))?);
    }
    Ok(set)
}

/// 扫描孤儿。`now` 由调用方给（测试里要造"很旧的文件"）。
pub fn scan_orphans(
    conn: &Connection,
    storage: &Path,
    now: SystemTime,
) -> Result<OrphanScan, String> {
    let media = load_set(
        conn,
        "SELECT stored_name FROM media_assets WHERE stored_name IS NOT NULL",
    )?;
    let media_ids = load_set(conn, "SELECT CAST(id AS TEXT) FROM media_assets")?;
    let attachments = load_set(conn, "SELECT stored_name FROM ai_attachments")?;
    let attachment_ids = load_set(conn, "SELECT id FROM ai_attachments")?;

    let mut items: Vec<Orphan> = Vec::new();
    let mut count = 0u64;
    let mut bytes = 0u64;

    let mut collect =
        |kind: &'static str, dir: &Path, referenced: &HashSet<String>, strip_ext: bool| {
            for (name, size, mtime) in flat_files(dir) {
                if now.duration_since(mtime).unwrap_or_default() < GRACE {
                    continue; // 刚写过的，先当成"还在写"
                }
                let key = if strip_ext {
                    Path::new(&name)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| name.clone())
                } else {
                    name.clone()
                };
                if referenced.contains(&key) {
                    continue;
                }
                count += 1;
                bytes += size;
                if items.len() < MAX_LIST {
                    items.push(Orphan {
                        kind,
                        name,
                        bytes: size,
                        modified_at: crate::infra::iso_time::from_system_time(mtime),
                    });
                }
            }
        };

    collect("media", &storage.join("media"), &media, false);
    collect("thumb", &storage.join("thumbs"), &media_ids, true);
    collect(
        "attachment",
        &storage.join("ai-attachments"),
        &attachments,
        false,
    );
    collect(
        "attachmentThumb",
        &storage.join("ai-thumbs"),
        &attachment_ids,
        true,
    );

    let truncated = count > items.len() as u64;
    Ok(OrphanScan {
        items,
        count,
        bytes,
        truncated,
    })
}

/// 删孤儿。**重新扫一遍再删**（不拿界面传来的路径清单当判据 —— 那等于把删文件的权利
/// 交给请求体）。返回的 `remaining` = 这次没删完的（超过 [`MAX_LIST`] 或删失败）。
pub fn clean_orphans(
    conn: &Connection,
    storage: &Path,
    now: SystemTime,
) -> Result<CleanReport, String> {
    let scan = scan_orphans(conn, storage, now)?;
    let mut deleted = 0u64;
    let mut bytes = 0u64;
    let mut failed = Vec::new();
    let mut removed = 0u64;

    for item in &scan.items {
        let dir = match item.kind {
            "media" => storage.join("media"),
            "thumb" => storage.join("thumbs"),
            "attachment" => storage.join("ai-attachments"),
            "attachmentThumb" => storage.join("ai-thumbs"),
            _ => continue,
        };
        // 名字来自 read_dir，本来就不带分隔符；这里再挡一次，免得将来有人从别处构造
        if item.name.contains('/') || item.name.contains('\\') || item.name.contains("..") {
            failed.push(FailedFile {
                name: item.name.clone(),
                error: "文件名不合法".into(),
            });
            continue;
        }
        let path = dir.join(&item.name);
        if path.parent() != Some(dir.as_path()) || !path.is_file() {
            failed.push(FailedFile {
                name: item.name.clone(),
                error: "文件已经不在了".into(),
            });
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                deleted += 1;
                bytes += item.bytes;
                removed += 1;
            }
            Err(e) => failed.push(FailedFile {
                name: item.name.clone(),
                error: format!("删不掉：{e}"),
            }),
        }
    }

    Ok(CleanReport {
        deleted,
        bytes,
        failed,
        remaining: scan.count.saturating_sub(removed),
    })
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
