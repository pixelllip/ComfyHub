//! 产物导出（2026-10-27）：把画廊里的**原件**连同它绑定的工作流 / 提示词信息
//! 一次性落到用户指定的文件夹里。
//!
//! # 为什么要有这一条
//!
//! 库里的产物本来就是**真文件**（`<storage>/media/<stored_name>`），但此前没有任何
//! "带出去"的入口：用户只能一张一张复制，工作流 / 提示词 / 集合归属全留在库里。
//! 换一台机器、交给别人、或者归档时，拿到的是没有上下文的散图。
//!
//! # 落盘布局（用户 2026-10-27 拍板）
//!
//! ```text
//! <目标目录>/
//!   图片/
//!     <单张产物>
//!     <单张产物>.json          ← 这一条的元信息（提示词 / 集合 / 工作流引用）
//!     <集合名>/
//!       <成员产物>
//!       <成员产物>.json
//!       collection.json        ← 集合本身（名字 / 来源 / 成员顺序）
//!   视频/
//!   音频/
//!   其它/
//!   工作流/
//!     <sha256>.ui.json         ← 界面格式工作流（同内容只落一份）
//!     <sha256>.api.json        ← API 格式节点图（有才落）
//!   导出清单.json              ← 本次导出的总账（相对路径 / 数量 / 失败原因）
//! ```
//!
//! 三条纪律：
//!
//! 1. **只读库、写目标目录**：不搬、不删、不改库里的任何一行；
//! 2. **文件名一律先净化再落盘**：`original_name` 来自用户 / 系统，带 `..`、`/`、
//!    `:` 的一律换掉（Windows 上还会撞 `CON` 这类保留名），任何一条都不许写到
//!    目标目录之外；
//! 3. **失败如实报**：某一条的文件不在盘上 / 复制失败，其余照常导出，并在
//!    `failed[]` 与清单里写清楚是哪一条、为什么 —— 不静默跳过，也不整批失败。
//!
//! # 与 `media/{id}/path` 的关系
//!
//! 这里同样只认 [`crate::media::media::resolve_media`]（目录穿越的唯一防线），
//! **不在前端拼 `<storage>/media/...`**（AGENTS §7）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::comfy::capture::history;
use crate::media::media::resolve_media;
use crate::web::routes::{ApiError, AppState};

/// 导出格式版本：布局或 JSON 形状变了就 bump（清单里带着它，读的人能判断）。
pub const EXPORT_SCHEMA_VERSION: i64 = 1;

/// 目标目录里的分类目录（用户原话："按文件类型（图片、视频、音频等）分类"）。
const KIND_DIR_IMAGE: &str = "图片";
const KIND_DIR_VIDEO: &str = "视频";
const KIND_DIR_AUDIO: &str = "音频";
const KIND_DIR_OTHER: &str = "其它";
const WORKFLOW_DIR: &str = "工作流";
const MANIFEST_NAME: &str = "导出清单.json";

/// tags 的 `GROUP_CONCAT` 分隔符：标签名里不会出现 U+001F。
const TAG_SEP: char = '\u{1f}';

/// 文件名主干最多保留多少个字符（给 `.png` / `.json` 留出余量，也避免超长路径）。
const MAX_STEM_CHARS: usize = 80;

/// `POST /api/media/export` 的请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// 目标目录（必须是**已存在的绝对路径**）。导出直接写在这里，不额外套子目录。
    pub dir: String,
    /// 只导出这几条（画廊多选）。`Some([])` = 选了 0 条，导 0 条。
    #[serde(default)]
    pub media_ids: Option<Vec<i64>>,
    /// 只导出这个集合的成员。
    #[serde(default)]
    pub collection_id: Option<i64>,
    /// 只导出收藏的产物（没有 `mediaIds` / `collectionId` 时生效）。
    #[serde(default)]
    pub favorites: bool,
    /// 是否导出界面格式工作流（默认 true）。
    #[serde(default = "default_true")]
    pub include_workflow: bool,
    /// 是否导出 API 格式节点图（默认 true；没有捕获记录时自然没有）。
    #[serde(default = "default_true")]
    pub include_api_workflow: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/media/export` 的响应体。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    /// 实际写入的根目录（用户给的那条路径，不是 canonical 形式）。
    pub dir: String,
    /// 清单文件的绝对路径。
    pub manifest: String,
    /// 成功落盘的产物条数。
    pub exported: i64,
    /// 落盘的集合文件夹数。
    pub collections: i64,
    /// 工作流文件数（界面 + API 合计，去重后）。
    pub workflows: i64,
    /// 成功落盘的字节数（原件大小之和）。
    pub total_bytes: i64,
    /// 各类产物的条数（`IMAGE` / `VIDEO` / `AUDIO` / `OTHER`）。
    pub by_kind: BTreeMap<String, i64>,
    /// 没导出的条目（文件不在盘上 / 复制失败 …）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<ExportFailure>,
    /// 产物导出了、但附带信息没写全（例如工作流写失败）—— 如实说，不假装完整。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// 单个条目失败的原因。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFailure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub reason: String,
}

impl ExportFailure {
    fn media(id: i64, name: &str, reason: impl Into<String>) -> Self {
        Self {
            media_id: Some(id),
            name: Some(name.to_string()),
            reason: reason.into(),
        }
    }
}

// ---------------------------------------------------------------------------
//  取数
// ---------------------------------------------------------------------------

/// 一条产物的全部导出素材（一次性取完，后面只碰磁盘，不再占着库锁）。
#[derive(Debug)]
struct ExportRow {
    id: i64,
    kind: String,
    title: String,
    original_name: String,
    stored_name: String,
    mime_type: Option<String>,
    size_bytes: i64,
    width: Option<i64>,
    height: Option<i64>,
    duration_ms: Option<i64>,
    sha256: Option<String>,
    source: Option<String>,
    source_ref: Option<String>,
    notes: Option<String>,
    favorite: bool,
    nsfw: bool,
    created_at: Option<String>,
    updated_at: Option<String>,
    workflow_json: Option<String>,
    prompt: Option<PromptInfo>,
    collection: Option<CollectionInfo>,
    media_tags: Vec<String>,
    prompt_tags: Vec<String>,
}

#[derive(Debug, Clone)]
struct PromptInfo {
    id: i64,
    title: String,
    kind: String,
    positive: String,
    negative: Option<String>,
    checkpoint: Option<String>,
    loras: Option<String>,
    sampler: Option<String>,
    scheduler: Option<String>,
    steps: Option<i64>,
    cfg_scale: Option<f64>,
    seed: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    batch_size: Option<i64>,
    extra_params: Option<String>,
    notes: Option<String>,
    negative_mode: String,
    workflow_json: Option<String>,
    source_ref: Option<String>,
    tags: Vec<String>,
}

#[derive(Debug, Clone)]
struct CollectionInfo {
    id: i64,
    title: Option<String>,
    origin: String,
    kind: String,
}

const EXPORT_SQL: &str = r#"
SELECT m.id, m.prompt_id, m.collection_id, m.kind, m.title, m.original_name, m.stored_name,
       m.mime_type, m.size_bytes, m.width, m.height, m.duration_ms, m.sha256, m.source,
       m.source_ref, m.notes, m.favorite, m.nsfw, m.created_at, m.updated_at, m.workflow_json,
       p.id AS prompt_id_joined, p.title AS prompt_title, p.kind AS prompt_kind,
       p.positive_prompt, p.negative_prompt, p.checkpoint, p.loras, p.sampler, p.scheduler,
       p.steps, p.cfg_scale, p.seed, p.width AS prompt_width, p.height AS prompt_height,
       p.batch_size, p.extra_params, p.notes AS prompt_notes, p.negative_mode,
       p.workflow_json AS prompt_workflow_json, p.source_ref AS prompt_source_ref,
       c.title AS collection_title, c.origin AS collection_origin, c.kind AS collection_kind,
       (SELECT GROUP_CONCAT(t.name, CHAR(31))
          FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
         WHERE mt.media_id = m.id) AS media_tags,
       (SELECT GROUP_CONCAT(t.name, CHAR(31))
          FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
         WHERE pt.prompt_id = m.prompt_id) AS prompt_tags
  FROM media_assets m
  LEFT JOIN prompts p ON p.id = m.prompt_id
  LEFT JOIN media_collections c ON c.id = m.collection_id
"#;

fn load_rows(state: &AppState, req: &ExportRequest) -> Result<Vec<ExportRow>, ApiError> {
    state.with(|conn| {
        let mut sql = String::from(EXPORT_SQL);
        let mut values: Vec<SqlValue> = Vec::new();

        // 优先级：明确选了 id > 集合 > 仅收藏 > 全部。
        // `Some([])` 是"用户确实选了 0 条"，不能退化成"全部"。
        if let Some(ids) = req.media_ids.as_ref() {
            if ids.is_empty() {
                sql.push_str(" WHERE 1 = 0");
            } else {
                let marks = vec!["?"; ids.len()].join(",");
                sql.push_str(&format!(" WHERE m.id IN ({marks})"));
                values.extend(ids.iter().copied().map(SqlValue::Integer));
            }
        } else if let Some(collection_id) = req.collection_id {
            sql.push_str(" WHERE m.collection_id = ?");
            values.push(SqlValue::Integer(collection_id));
        } else if req.favorites {
            sql.push_str(" WHERE m.favorite = 1");
        }

        // 集合成员必须挨着（同一个文件夹里按 created_at 排），散图排在后面。
        sql.push_str(
            " ORDER BY (m.collection_id IS NULL), m.collection_id, m.created_at ASC, m.id ASC",
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            let prompt = match row.get::<_, Option<i64>>("prompt_id_joined")? {
                Some(id) => Some(PromptInfo {
                    id,
                    title: row.get::<_, Option<String>>("prompt_title")?.unwrap_or_default(),
                    kind: row
                        .get::<_, Option<String>>("prompt_kind")?
                        .unwrap_or_else(|| "IMAGE".to_string()),
                    positive: row.get::<_, Option<String>>("positive_prompt")?.unwrap_or_default(),
                    negative: row.get("negative_prompt")?,
                    checkpoint: row.get("checkpoint")?,
                    loras: row.get("loras")?,
                    sampler: row.get("sampler")?,
                    scheduler: row.get("scheduler")?,
                    steps: row.get("steps")?,
                    cfg_scale: row.get("cfg_scale")?,
                    seed: row.get("seed")?,
                    width: row.get("prompt_width")?,
                    height: row.get("prompt_height")?,
                    batch_size: row.get("batch_size")?,
                    extra_params: row.get("extra_params")?,
                    notes: row.get("prompt_notes")?,
                    negative_mode: row
                        .get::<_, Option<String>>("negative_mode")?
                        .unwrap_or_else(|| "keep".to_string()),
                    workflow_json: row.get("prompt_workflow_json")?,
                    source_ref: row.get("prompt_source_ref")?,
                    tags: split_tags(row.get("prompt_tags")?),
                }),
                None => None,
            };

            let collection = match row.get::<_, Option<i64>>("collection_id")? {
                Some(id) => Some(CollectionInfo {
                    id,
                    title: row.get("collection_title")?,
                    origin: row
                        .get::<_, Option<String>>("collection_origin")?
                        .unwrap_or_else(|| "auto".to_string()),
                    kind: row
                        .get::<_, Option<String>>("collection_kind")?
                        .unwrap_or_else(|| "IMAGE".to_string()),
                }),
                None => None,
            };

            Ok(ExportRow {
                id: row.get("id")?,
                kind: row.get("kind")?,
                title: row.get("title")?,
                original_name: row.get("original_name")?,
                stored_name: row.get("stored_name")?,
                mime_type: row.get("mime_type")?,
                size_bytes: row.get("size_bytes")?,
                width: row.get("width")?,
                height: row.get("height")?,
                duration_ms: row.get("duration_ms")?,
                sha256: row.get("sha256")?,
                source: row.get("source")?,
                source_ref: row.get("source_ref")?,
                notes: row.get("notes")?,
                favorite: row.get::<_, i64>("favorite")? != 0,
                nsfw: row.get::<_, i64>("nsfw")? != 0,
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
                workflow_json: row.get("workflow_json")?,
                prompt,
                collection,
                media_tags: split_tags(row.get("media_tags")?),
                prompt_tags: split_tags(row.get("prompt_tags")?),
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    })
}

fn split_tags(raw: Option<String>) -> Vec<String> {
    let Some(text) = raw else { return Vec::new() };
    let mut out = Vec::new();
    for item in text.split(TAG_SEP) {
        let name = item.trim();
        if !name.is_empty() && !out.iter().any(|existing| existing == name) {
            out.push(name.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
//  目标目录 / 文件名
// ---------------------------------------------------------------------------

/// 校验目标目录：存在、是目录、可写，而且**不能**落在产物存储目录里
/// （那等于把文件再拷进库的原料区，既重复又可能被下一次捕获当成新产物）。
fn prepare_root(raw: &str, media_dir: &Path) -> Result<PathBuf, ApiError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("请选择一个导出目录"));
    }
    let requested = PathBuf::from(text);
    if !requested.is_absolute() {
        return Err(ApiError::bad_request(format!(
            "导出目录必须是绝对路径：{text}"
        )));
    }
    let root = fs::canonicalize(&requested).map_err(|e| {
        ApiError::bad_request(format!("导出目录不可用（不存在或没有访问权限）：{text}（{e}）"))
    })?;
    if !root.is_dir() {
        return Err(ApiError::bad_request(format!("导出目标不是文件夹：{text}")));
    }

    let media_root = fs::canonicalize(media_dir).unwrap_or_else(|_| media_dir.to_path_buf());
    let storage_root = media_root
        .parent()
        .unwrap_or(media_root.as_path())
        .to_path_buf();
    if root == media_root
        || root.starts_with(&media_root)
        || root == storage_root
        || root.starts_with(&storage_root)
    {
        return Err(ApiError::bad_request(
            "不能导出到 ComfyHub 自己的存储目录（storage / storage\\media）里 —— 请换一个文件夹",
        ));
    }

    // 先探一次可写：目录存在不等于写得进去（只读盘 / 权限不足都在这时暴露）。
    // 探针只有一个真源（`infra::confinement::probe_dir`，带 pid、不覆盖别人的文件）。
    if let Err(e) = crate::infra::confinement::probe_dir(&root) {
        let mut message = format!("导出目录不可写：{text}（{e}）");
        // os error 5 的常见真因不是目录 ACL，而是本进程带 Low 完整性标签
        // （AGENTS §12）。修法只有一份，别在文案里另编一套。
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            message.push_str("\n\n");
            message.push_str(&crate::infra::confinement::remedy_text());
        }
        return Err(ApiError::bad_request(message));
    }
    Ok(root)
}

fn kind_dir(kind: &str) -> &'static str {
    match kind.trim().to_ascii_uppercase().as_str() {
        "IMAGE" => KIND_DIR_IMAGE,
        "VIDEO" => KIND_DIR_VIDEO,
        "AUDIO" => KIND_DIR_AUDIO,
        _ => KIND_DIR_OTHER,
    }
}

fn kind_key(kind: &str) -> String {
    let upper = kind.trim().to_ascii_uppercase();
    match upper.as_str() {
        "IMAGE" | "VIDEO" | "AUDIO" => upper,
        _ => "OTHER".to_string(),
    }
}

/// Windows 保留设备名（不区分大小写）。
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 把一个任意字符串变成**单段**安全文件名：去掉路径分隔符 / 控制字符 / Windows 非法字符，
/// 削掉结尾的点和空格，避开保留设备名，并限制长度。
fn sanitize_component(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    // 开头 / 结尾的点和空格都削掉：`..` 与 `.hidden` 在 Windows / 压缩包里都容易出怪事。
    let mut out = out
        .trim()
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    if out.is_empty() {
        out = fallback.to_string();
    }

    // 保留名只看**主干**（`CON.txt` 同样打不开）。
    let stem = out.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        out = format!("_{out}");
    }
    truncate_chars(&out, MAX_STEM_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// 拆出 (主干, 扩展名)。扩展名只保留 ASCII 字母数字，避免 `a.png.` / `a.p?g` 这类怪名字。
fn split_name(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    if let Some((stem, ext)) = trimmed.rsplit_once('.') {
        if !stem.is_empty() && !ext.is_empty() && ext.chars().count() <= 12 {
            let clean_ext: String = ext
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(12)
                .collect();
            if !clean_ext.is_empty() && clean_ext.len() == ext.len() {
                return (stem.to_string(), clean_ext.to_ascii_lowercase());
            }
        }
    }
    (trimmed.to_string(), String::new())
}

/// 目标目录里的重名登记：同一层目录内**不分大小写**去重，并避开磁盘上已有的同名项。
#[derive(Default)]
struct NameBook {
    used: HashSet<String>,
}

impl NameBook {
    fn claim(&mut self, dir: &Path, base: &str, ext: &str, id: i64) -> String {
        let suffix = if ext.is_empty() {
            String::new()
        } else {
            format!(".{ext}")
        };
        let base = if base.trim().is_empty() {
            format!("产物-{id}")
        } else {
            base.to_string()
        };
        let mut candidate = format!("{base}{suffix}");
        let mut n = 1;
        loop {
            let key = candidate.to_lowercase();
            let exists = dir.join(&candidate).exists();
            if !self.used.contains(&key) && !exists {
                self.used.insert(key);
                return candidate;
            }
            candidate = if n == 1 {
                format!("{base}_{id}{suffix}")
            } else {
                format!("{base}_{id}_{n}{suffix}")
            };
            n += 1;
        }
    }
}

fn rel_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
//  工作流
// ---------------------------------------------------------------------------

/// 工作流文件落盘：同样内容只写一份（按 SHA-256 去重）。
struct WorkflowStore {
    dir: PathBuf,
    dir_ready: bool,
    /// `(kind, sha)` → 相对路径
    written: HashMap<String, String>,
    count: i64,
}

impl WorkflowStore {
    fn new(root: &Path) -> Self {
        Self {
            dir: root.join(WORKFLOW_DIR),
            dir_ready: false,
            written: HashMap::new(),
            count: 0,
        }
    }

    /// 写一份工作流，返回相对目标根目录的路径。
    fn write(&mut self, content: &str, suffix: &str) -> std::io::Result<String> {
        let sha = crate::comfy::comfy_dirs::sha256_hex(content.as_bytes());
        let key = format!("{suffix}:{sha}");
        if let Some(existing) = self.written.get(&key) {
            return Ok(existing.clone());
        }
        if !self.dir_ready {
            fs::create_dir_all(&self.dir)?;
            self.dir_ready = true;
        }
        let name = format!("{sha}.{suffix}.json");
        fs::write(self.dir.join(&name), content.as_bytes())?;
        let rel = format!("{WORKFLOW_DIR}/{name}");
        self.written.insert(key, rel.clone());
        self.count += 1;
        Ok(rel)
    }
}

// ---------------------------------------------------------------------------
//  导出主流程
// ---------------------------------------------------------------------------

/// 导出一次。**同步阻塞**（调用方应放到 `spawn_blocking` 里）。
pub fn export(state: &AppState, req: ExportRequest) -> Result<ExportReport, ApiError> {
    let root = prepare_root(&req.dir, state.media_dir())?;
    let rows = load_rows(state, &req)?;
    let exported_at = crate::infra::iso_time::from_system_time(std::time::SystemTime::now());

    let mut books: HashMap<String, NameBook> = HashMap::new();
    let mut workflows = WorkflowStore::new(&root);
    let mut api_cache: HashMap<String, Option<Value>> = HashMap::new();
    let mut failed: Vec<ExportFailure> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut by_kind: BTreeMap<String, i64> = BTreeMap::new();
    let mut items: Vec<Value> = Vec::new();
    let mut collections: BTreeMap<i64, CollectionAccum> = BTreeMap::new();
    let mut total_bytes = 0i64;

    for row in &rows {
        let Some(source) = resolve_media(state.media_dir(), &row.stored_name) else {
            failed.push(ExportFailure::media(
                row.id,
                &row.original_name,
                format!("文件不在盘上：{}", row.stored_name),
            ));
            continue;
        };

        let kind_dir_name = kind_dir(&row.kind);
        let kind_path = root.join(kind_dir_name);
        if let Err(e) = fs::create_dir_all(&kind_path) {
            failed.push(ExportFailure::media(
                row.id,
                &row.original_name,
                format!("建不了分类目录 {kind_dir_name}：{e}"),
            ));
            continue;
        }

        // 集合成员进集合文件夹；散图直接放在类型目录下。
        let (dest_dir, collection_rel) = match row.collection.as_ref() {
            Some(collection) => {
                let folder = collections.entry(collection.id).or_insert_with(|| {
                    // 手动集合没起名时叫「集合-N」；算法分出来的叫「算法分组-N」
                    // （与画廊里的显示口径一致，别让用户对着「集合-7」猜这是什么）。
                    let fallback = if collection.origin == "manual" {
                        format!("集合-{}", collection.id)
                    } else {
                        format!("算法分组-{}", collection.id)
                    };
                    let base = sanitize_component(
                        collection.title.as_deref().unwrap_or(""),
                        &fallback,
                    );
                    CollectionAccum {
                        id: collection.id,
                        title: collection.title.clone(),
                        origin: collection.origin.clone(),
                        kind: collection.kind.clone(),
                        dir: kind_path.clone(),
                        folder: base,
                        folder_rel: String::new(),
                        members: Vec::new(),
                    }
                });
                // 第一次见这个集合时才登记文件夹名（同一层目录里避开同名文件和文件夹）
                if folder.folder_rel.is_empty() {
                    let book = books.entry(kind_dir_name.to_string()).or_default();
                    let claimed = book.claim(&kind_path, &folder.folder, "", folder.id);
                    let dir = kind_path.join(&claimed);
                    if let Err(e) = fs::create_dir_all(&dir) {
                        failed.push(ExportFailure::media(
                            row.id,
                            &row.original_name,
                            format!("建不了集合文件夹 {claimed}：{e}"),
                        ));
                        continue;
                    }
                    folder.folder = claimed;
                    folder.folder_rel = format!("{kind_dir_name}/{}", folder.folder);
                }
                (folder.dir.join(&folder.folder), Some(folder.folder_rel.clone()))
            }
            None => (kind_path.clone(), None),
        };

        let (stem, ext) = split_name(&row.original_name);
        let fallback = format!("产物-{}", row.id);
        let base = sanitize_component(&stem, &fallback);
        let ext = if ext.is_empty() {
            split_name(&row.stored_name).1
        } else {
            ext
        };
        let book_key = collection_rel
            .clone()
            .unwrap_or_else(|| kind_dir_name.to_string());
        let book = books.entry(book_key).or_default();
        let file_name = book.claim(&dest_dir, &base, &ext, row.id);
        let dest = dest_dir.join(&file_name);

        let copied_size = match fs::copy(&source, &dest) {
            Ok(copied) => copied as i64,
            Err(e) => {
                failed.push(ExportFailure::media(
                    row.id,
                    &row.original_name,
                    format!("复制失败：{e}"),
                ));
                continue;
            }
        };

        // 工作流：优先这条产物自己的，其次关联提示词的（老数据只有提示词那一份）。
        let mut workflow_rel = None;
        let mut workflow_source = None;
        let mut workflow_format = "comfyui-ui";
        let mut api_rel = None;
        if req.include_workflow {
            let picked = non_empty(row.workflow_json.clone())
                .map(|text| (text, "media"))
                .or_else(|| {
                    row.prompt
                        .as_ref()
                        .and_then(|p| non_empty(p.workflow_json.clone()))
                        .map(|text| (text, "prompt"))
                });
            if let Some((text, source_name)) = picked {
                // 老数据的 `workflow_json` 有时存的是 API 图（提交给 ComfyUI 的那份），
                // 不能一律标成界面格式 —— 读的人会拿它去 ComfyUI 拖，然后发现拖不进去。
                let is_api = looks_like_api_graph(&text);
                let suffix = if is_api { "api" } else { "ui" };
                workflow_format = if is_api { "comfyui-api" } else { "comfyui-ui" };
                match workflows.write(&text, suffix) {
                    Ok(rel) => {
                        workflow_rel = Some(rel);
                        workflow_source = Some(source_name);
                    }
                    Err(e) => warnings.push(format!("产物 #{} 的工作流没写出去：{e}", row.id)),
                }
            }
        }

        if req.include_api_workflow {
            let run_key = non_empty(row.source_ref.clone()).or_else(|| {
                row.prompt
                    .as_ref()
                    .and_then(|p| non_empty(p.source_ref.clone()))
            });
            if let Some(run_key) = run_key {
                if !api_cache.contains_key(&run_key) {
                    api_cache.insert(run_key.clone(), api_graph_for(state, &run_key));
                }
                if let Some(Some(graph)) = api_cache.get(&run_key) {
                    let text = serde_json::to_string_pretty(graph)
                        .unwrap_or_else(|_| graph.to_string());
                    match workflows.write(&text, "api") {
                        Ok(rel) => api_rel = Some(rel),
                        Err(e) => warnings.push(format!(
                            "产物 #{} 的 API 节点图没写出去：{e}",
                            row.id
                        )),
                    }
                }
            }
        }

        let rel_file = rel_string(
            dest.strip_prefix(&root)
                .unwrap_or(dest.as_path()),
        );
        let meta_name = book.claim(&dest_dir, &file_name, "json", row.id);
        let meta_rel = match collection_rel.as_deref() {
            Some(dir) => format!("{dir}/{meta_name}"),
            None => format!("{kind_dir_name}/{meta_name}"),
        };
        let meta = media_meta_json(
            row,
            &exported_at,
            &rel_file,
            &file_name,
            workflow_rel.as_deref(),
            workflow_source,
            workflow_format,
            api_rel.as_deref(),
            collection_rel.as_deref(),
        );
        let meta_text = match serde_json::to_string_pretty(&meta) {
            Ok(text) => text,
            Err(e) => {
                let _ = fs::remove_file(&dest);
                failed.push(ExportFailure::media(
                    row.id,
                    &row.original_name,
                    format!("元信息序列化失败：{e}"),
                ));
                continue;
            }
        };
        if let Err(e) = fs::write(root.join(&meta_rel), meta_text.as_bytes()) {
            let _ = fs::remove_file(&dest);
            failed.push(ExportFailure::media(
                row.id,
                &row.original_name,
                format!("元信息写入失败：{e}"),
            ));
            continue;
        }

        // 只有**原件 + 侧车都落盘**才算这一条成功 —— 否则 `exported`、`totalBytes`
        // 与 `byKind` 会和实际文件对不上（用户看到的数字就成了假的）。
        total_bytes += copied_size;
        *by_kind.entry(kind_key(&row.kind)).or_insert(0) += 1;

        if let Some(collection) = row.collection.as_ref() {
            if let Some(accum) = collections.get_mut(&collection.id) {
                accum.members.push(json!({
                    "mediaId": row.id,
                    "file": rel_file,
                    "meta": meta_rel,
                    "order": accum.members.len() + 1,
                }));
            }
        }

        items.push(json!({
            "mediaId": row.id,
            "kind": kind_key(&row.kind),
            "title": row.title,
            "file": rel_file,
            "meta": meta_rel,
            "sizeBytes": row.size_bytes,
            "sha256": row.sha256,
            "collectionId": row.collection.as_ref().map(|c| c.id),
            "collectionFolder": collection_rel,
            "workflow": workflow_rel,
            "apiWorkflow": api_rel,
        }));
    }

    // 集合清单：一个集合一份，成员顺序就是文件的导出顺序。
    let mut collections_written = 0i64;
    for (_, accum) in &collections {
        if accum.members.is_empty() {
            continue;
        }
        let payload = json!({
            "schemaVersion": EXPORT_SCHEMA_VERSION,
            "id": accum.id,
            "title": accum.title,
            "origin": accum.origin,
            "kind": kind_key(&accum.kind),
            "folder": accum.folder_rel,
            "members": accum.members,
        });
        let text = serde_json::to_string_pretty(&payload)
            .unwrap_or_else(|_| payload.to_string());
        if let Err(e) = fs::write(accum.dir.join(&accum.folder).join("collection.json"), text) {
            warnings.push(format!(
                "集合 #{} 的 collection.json 没写出去：{e}",
                accum.id
            ));
        } else {
            collections_written += 1;
        }
    }

    let manifest_path = root.join(MANIFEST_NAME);
    let exported_count = items.len();
    let failed_count = failed.len();
    let manifest = json!({
        "schemaVersion": EXPORT_SCHEMA_VERSION,
        "app": "ComfyHub",
        "exportedAt": exported_at,
        "root": req.dir.trim(),
        "layout": {
            "kinds": [KIND_DIR_IMAGE, KIND_DIR_VIDEO, KIND_DIR_AUDIO, KIND_DIR_OTHER],
            "workflows": WORKFLOW_DIR,
            "manifest": MANIFEST_NAME,
        },
        "counts": {
            "exported": exported_count,
            "collections": collections_written,
            "workflows": workflows.count,
            "totalBytes": total_bytes,
            "failed": failed_count,
        },
        "byKind": &by_kind,
        "collections": collections.values().filter(|c| !c.members.is_empty()).map(|c| json!({
            "id": c.id,
            "title": c.title,
            "origin": c.origin,
            "kind": kind_key(&c.kind),
            "folder": c.folder_rel,
            "members": c.members.len(),
        })).collect::<Vec<_>>(),
        "items": &items,
        "failed": &failed,
        "warnings": &warnings,
    });
    let manifest_text =
        serde_json::to_string_pretty(&manifest).map_err(|e| ApiError::internal(format!("清单序列化失败：{e}")))?;
    fs::write(&manifest_path, manifest_text.as_bytes())
        .map_err(|e| ApiError::internal(format!("导出清单写入失败：{e}")))?;

    Ok(ExportReport {
        dir: req.dir.trim().to_string(),
        // 回执里用**用户给的那条路径**拼清单名，不用 canonical 形式：
        // Windows 的 canonical 带 `\\?\` 前缀，界面照着打开会显得像坏路径。
        manifest: PathBuf::from(req.dir.trim())
            .join(MANIFEST_NAME)
            .display()
            .to_string(),
        exported: items.len() as i64,
        collections: collections_written,
        workflows: workflows.count,
        total_bytes,
        by_kind,
        failed,
        warnings,
    })
}

struct CollectionAccum {
    id: i64,
    title: Option<String>,
    origin: String,
    kind: String,
    dir: PathBuf,
    folder: String,
    folder_rel: String,
    members: Vec<Value>,
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

/// 这段工作流文本是不是 **API 格式节点图**（每个值都是带 `class_type` 的对象）。
///
/// 判据只有一份，在 `capture::history::is_node_graph`（捕获那边也用它），
/// 这里只负责把文本解析成对象；解析不了就当界面格式（原样写出，不改一个字节）。
fn looks_like_api_graph(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| value.as_object().map(history::is_node_graph))
        .unwrap_or(false)
}

/// 这条产物绑定的 **API 格式节点图**：`source_ref` → `capture_runs.raw` → `prompt` 字段。
///
/// 与 `GET /api/prompts/{id}/api-graph` 同一条取数链条；没有 / 解析不了就是 `None`
/// （老数据本来就没有，别猜着转换）。
fn api_graph_for(state: &AppState, run_key: &str) -> Option<Value> {
    let raw: Option<String> = state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT raw FROM capture_runs WHERE run_key = ?1",
                    rusqlite::params![run_key],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
        .ok()?;
    let text = raw.filter(|t| !t.trim().is_empty())?;
    let entry: Value = serde_json::from_str(&text).ok()?;
    let graph = history::parse(&entry).graph?;
    crate::workflow::workflow_fit::api_graph_of(Some(&graph)).map(Value::Object)
}

fn media_meta_json(
    row: &ExportRow,
    exported_at: &str,
    file_rel: &str,
    file_name: &str,
    workflow_rel: Option<&str>,
    workflow_source: Option<&str>,
    workflow_format: &str,
    api_rel: Option<&str>,
    collection_rel: Option<&str>,
) -> Value {
    let prompt = row.prompt.as_ref().map(|p| {
        json!({
            "id": p.id,
            "title": p.title,
            "kind": p.kind,
            "positivePrompt": p.positive,
            "negativePrompt": p.negative,
            "checkpoint": p.checkpoint,
            "loras": json_or_string(p.loras.as_deref()),
            "sampler": p.sampler,
            "scheduler": p.scheduler,
            "steps": p.steps,
            "cfgScale": p.cfg_scale,
            "seed": p.seed,
            "width": p.width,
            "height": p.height,
            "batchSize": p.batch_size,
            "extraParams": json_or_string(p.extra_params.as_deref()),
            "notes": p.notes,
            "negativeMode": p.negative_mode,
            "tags": p.tags,
        })
    });

    let mut tags = row.media_tags.clone();
    for tag in &row.prompt_tags {
        if !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }

    json!({
        "schemaVersion": EXPORT_SCHEMA_VERSION,
        "exportedAt": exported_at,
        "media": {
            "id": row.id,
            "title": row.title,
            "kind": kind_key(&row.kind),
            "originalName": row.original_name,
            "fileName": file_name,
            "file": file_rel,
            "mimeType": row.mime_type,
            "sizeBytes": row.size_bytes,
            "width": row.width,
            "height": row.height,
            "durationMs": row.duration_ms,
            "sha256": row.sha256,
            "source": row.source,
            "sourceRef": row.source_ref,
            "favorite": row.favorite,
            "nsfw": row.nsfw,
            "notes": row.notes,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "tags": tags,
            "mediaTags": row.media_tags,
            "promptTags": row.prompt_tags,
        },
        "prompt": prompt,
        "collection": row.collection.as_ref().map(|c| json!({
            "id": c.id,
            "title": c.title,
            "origin": c.origin,
            "kind": kind_key(&c.kind),
            "folder": collection_rel,
        })),
        "workflow": workflow_rel.map(|file| json!({
            "format": workflow_format,
            "source": workflow_source.unwrap_or("media"),
            "file": file,
        })),
        "apiWorkflow": api_rel.map(|file| json!({
            "format": "comfyui-api",
            "file": file,
        })),
    })
}

/// `loras` / `extra_params` 在库里是 JSON 文本；能解析就嵌对象，坏数据如实成字符串。
fn json_or_string(raw: Option<&str>) -> Value {
    let Some(text) = raw else { return Value::Null };
    if text.trim().is_empty() {
        return Value::Null;
    }
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

#[cfg(test)]
mod tests;
