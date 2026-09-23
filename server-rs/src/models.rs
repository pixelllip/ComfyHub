//! 桌面端点的 JSON 形状（**与 Kotlin 版逐字一致** —— Flutter 一行不改是 P4 的硬要求）。
//!
//! 这些结构体字段名与 `lib/models/models.dart` 的 `fromJson` 一一对应：
//! Dart 那边读 `positivePrompt` / `cfgScale` / `useCount` / `hasWorkflow` / `mediaCount`，
//! 这里就**一个都不能写错**（写错的后果是界面上那个字段静默为空）。
//!
//! `#[serde(rename_all = "camelCase")]` 负责把 snake_case 的 Rust 字段映射过去；
//! 每个结构体都有一条"键集合"用例（见 `tests/api_test.rs`），改字段会红。

use rusqlite::Row;
use serde::Serialize;

/// 标签（对应 `TagRepo.list` 的输出）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagDto {
    pub id: i64,
    pub name: String,
    pub normalized: String,
    pub category: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub use_count: i64,
    pub created_at: String,
}

/// 提示词（对应 `PromptRepo.get` / `search` 的 `items[]`）。
///
/// ⚠ `loras` / `extra_params` 在库里是 **JSON 文本**（契约 §3.1 的类型映射），
/// 出接口时**解成对象/数组**（Dart 读的是 `List` / `Map`，给字符串会解析失败）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromptDto {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub positive_prompt: String,
    pub negative_prompt: Option<String>,
    pub checkpoint: Option<String>,
    pub loras: Vec<LoraRef>,
    pub sampler: Option<String>,
    pub scheduler: Option<String>,
    pub steps: Option<i64>,
    pub cfg_scale: Option<f64>,
    pub seed: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub batch_size: Option<i64>,
    pub extra_params: serde_json::Value,
    pub notes: Option<String>,
    pub favorite: bool,
    pub source: Option<String>,
    pub source_ref: Option<String>,
    /// 库里存的是整份 `workflow_json`；接口**只给"有没有"**（整份在工作流弹窗里单独取）
    pub has_workflow: bool,
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<TagDto>,
    pub media_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoraRef {
    pub name: String,
    pub weight: f64,
}

/// 分页信封（对应 Dart `Paged<T>`）。
#[derive(Debug, Clone, Serialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub size: i64,
    pub pages: i64,
}

impl<T> Paged<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, size: i64) -> Self {
        let pages = if size <= 0 { 0 } else { (total + size - 1) / size };
        Self { items, total, page, size, pages }
    }
}

/// 从一行 `prompts` 记录读出 DTO 的**非关联字段**（标签/产物计数由调用方补齐）。
pub fn prompt_from_row(row: &Row<'_>) -> rusqlite::Result<PromptDto> {
    let loras_text: Option<String> = row.get("loras")?;
    let extra_text: Option<String> = row.get("extra_params")?;
    let workflow_text: Option<String> = row.get("workflow_json")?;
    Ok(PromptDto {
        id: row.get("id")?,
        title: row.get("title")?,
        kind: row.get("kind")?,
        positive_prompt: row.get("positive_prompt")?,
        negative_prompt: row.get("negative_prompt")?,
        checkpoint: row.get("checkpoint")?,
        loras: parse_loras(loras_text.as_deref()),
        sampler: row.get("sampler")?,
        scheduler: row.get("scheduler")?,
        steps: row.get("steps")?,
        cfg_scale: row.get("cfg_scale")?,
        seed: row.get("seed")?,
        width: row.get("width")?,
        height: row.get("height")?,
        batch_size: row.get("batch_size")?,
        extra_params: parse_json_object(extra_text.as_deref()),
        notes: row.get("notes")?,
        favorite: row.get::<_, i64>("favorite")? != 0,
        source: row.get("source")?,
        source_ref: row.get("source_ref")?,
        has_workflow: workflow_text.map(|t| !t.trim().is_empty()).unwrap_or(false),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        tags: Vec::new(),
        media_count: 0,
    })
}

pub fn tag_from_row(row: &Row<'_>) -> rusqlite::Result<TagDto> {
    Ok(TagDto {
        id: row.get("id")?,
        name: row.get("name")?,
        normalized: row.get("normalized")?,
        category: row.get("category")?,
        color: row.get("color")?,
        description: row.get("description")?,
        use_count: row.get("use_count")?,
        created_at: row.get("created_at")?,
    })
}

/// `loras` 列是 JSON 数组文本；坏数据**当作空数组**（不让一行脏数据把整个列表打成 500）。
pub fn parse_loras(raw: Option<&str>) -> Vec<LoraRef> {
    let Some(text) = raw else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let weight = item.get("weight").and_then(|w| w.as_f64()).unwrap_or(1.0);
            Some(LoraRef { name, weight })
        })
        .collect()
}

/// `extra_params` 列是 JSON 对象文本；坏数据当空对象。
pub fn parse_json_object(raw: Option<&str>) -> serde_json::Value {
    let Some(text) = raw else { return serde_json::json!({}) };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) if value.is_object() => value,
        _ => serde_json::json!({}),
    }
}
