//! 桌面端点的 JSON 形状（**与 Kotlin 版逐字一致** —— Flutter 一行不改是 P4 的硬要求）。
//!
//! 这些结构体字段名与 `lib/models/models.dart` 的 `fromJson` 一一对应：
//! Dart 那边读 `positivePrompt` / `cfgScale` / `useCount` / `hasWorkflow` / `mediaCount`，
//! 这里就**一个都不能写错**（写错的后果是界面上那个字段静默为空）。
//!
//! `#[serde(rename_all = "camelCase")]` 负责把 snake_case 的 Rust 字段映射过去；
//! 每个结构体都有一条"键集合"用例（见 `tests/api_test.rs`），改字段会红。
//!
//! # ⚠ 可空字段：**不是 `null`，而是"这个键不出现"**（2026-09-24）
//!
//! Kotlin 的 `AppJson` 是 `explicitNulls = false`（`server/src/main/kotlin/com/comfyhub/Json.kt:12`），
//! 所以 `TagDto.category == null` 时发出去的是 `{...}`，**里面没有 `category` 这个键**。
//! 真库副本上实测（Kotlin 8090 vs Rust 8091，同一份数据）：
//! `GET /api/tags` → Kotlin `{"id":18,"name":"ComfyUI","useCount":44,"createdAt":"…"}`，
//! Rust 当时是 `{…,"normalized":"comfyui","category":null,"color":null,"description":null,…}`。
//!
//! 因此**每一个对应 Kotlin 可空字段的 `Option<T>` 都要带** `skip_serializing_if = "Option::is_none"`。
//! 防线是 `tests/shape_lint.rs`（源码扫描，漏了直接红）——加新 DTO（例如 `/api/ai/models`）时照做。
//!
//! 顺带一条 2026-09-24 的更正：`TagDto` 曾经多写了一个 **`normalized`** 字段 ——
//! Kotlin 的 `TagDto`（`Models.kt:11`）与 `TagRepo.mapTag` 里**根本没有它**，
//! Dart 的 `Tag.fromJson` 也不读。多一个键和少一个键一样，都是"与 Kotlin 不一致"。

use rusqlite::Row;
use serde::Serialize;

/// 标签（对应 `TagRepo.mapTag` 的输出）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagDto {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub use_count: i64,
    /// Kotlin 是 `createdAt: String? = null`：库里为 NULL 时**键不出现**（不是 `null`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    pub loras: Vec<LoraRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfg_scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i64>,
    pub extra_params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// 发送模式：`keep`（默认）/ `positive_only`（不认负面提示词的 turbo 模型用）。
    /// **总是给**（不是 Option）：界面拿它决定那个开关的位置，缺了会显示成"未设置"。
    pub negative_mode: String,
    pub favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// 库里存的是整份 `workflow_json`；接口**只给"有没有"**（整份在工作流弹窗里单独取）
    pub has_workflow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
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
        let pages = if size <= 0 {
            0
        } else {
            (total + size - 1) / size
        };
        Self {
            items,
            total,
            page,
            size,
            pages,
        }
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
        // 老库 / 老查询没带这一列时退回 `keep`（**绝不能变成"只发正面"**：
        // 那会悄悄把用户的负面提示词丢掉，而 turbo 之外的模型很吃负面）
        negative_mode: row
            .get::<_, Option<String>>("negative_mode")?
            .unwrap_or_else(|| "keep".to_string()),
        favorite: row.get::<_, i64>("favorite")? != 0,
        source: row.get("source")?,
        source_ref: row.get("source_ref")?,
        has_workflow: workflow_text.map(|t| !t.trim().is_empty()).unwrap_or(false),
        // 时间列一律过 `iso_time`：**不是透传**（Kotlin 那边先解析、再按 Java
        // `Instant.toString()` 重排，`.000Z` 会变成不带小数点、坏数据整键不出现，见 iso_time.rs 的头注）
        created_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("created_at")?.as_deref(),
        ),
        updated_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("updated_at")?.as_deref(),
        ),
        tags: Vec::new(),
        media_count: 0,
    })
}

pub fn tag_from_row(row: &Row<'_>) -> rusqlite::Result<TagDto> {
    Ok(TagDto {
        id: row.get("id")?,
        name: row.get("name")?,
        category: row.get("category")?,
        color: row.get("color")?,
        description: row.get("description")?,
        use_count: row.get("use_count")?,
        created_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("created_at")?.as_deref(),
        ),
    })
}

/// `GROUP_CONCAT` 出来的 id 列表（`"12,15,31"`）→ `Vec<i64>`。
///
/// 坏数据**当作空数组**（与 `parse_loras` 同一条纪律：不让一行脏数据把整个列表打成 500）。
/// `None`（子查询没命中）也当空数组 —— 界面拿到空数组就是"没进组 / 没有封面可叠"。
fn parse_id_list(raw: Option<&str>) -> Vec<i64> {
    let Some(text) = raw else { return Vec::new() };
    text.split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// `loras` 列是 JSON 数组文本；坏数据**当作空数组**（不让一行脏数据把整个列表打成 500）。
pub fn parse_loras(raw: Option<&str>) -> Vec<LoraRef> {
    let Some(text) = raw else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
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
    let Some(text) = raw else {
        return serde_json::json!({});
    };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) if value.is_object() => value,
        _ => serde_json::json!({}),
    }
}
/// 产物（对应 `MediaRepo.mapMedia` 的输出）。
///
/// 三条"照抄"的地方别改：
///  - `promptTags` 是**产物自身标签 ∪ 关联提示词标签**（去重排序）——画廊里筛选看到的集合；
///  - `fileUrl` / `thumbUrl` 是**后端算好的路径**，前端不许自己拼（AGENTS §10.1）；
///  - `promptTitle` / `promptPositive` / `promptNegative` / `checkpoint` / `seed` 是
///    **关联提示词摘要**（列表直出，省一次请求）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaDto {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<i64>,
    pub kind: String,
    pub title: String,
    pub original_name: String,
    pub stored_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// NSFW 标记：**只影响预览图打不打码**（默认 false = 不糊）
    pub nsfw: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub has_workflow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_positive: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_negative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 所属**集合**（同源微调的产物归到一组；判定见 `media/collection.rs`）。
    ///
    /// `None` = 没进组（没关联提示词的孤儿产物，或规则判定它自成一类）。
    /// 界面上 `collection_size <= 1` 与 `None` 一样按普通格子显示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<i64>,
    /// 这个集合有几个成员（`1` = 只有它自己）。画廊据此决定要不要叠放封面、要不要显示角标。
    pub collection_size: i64,
    /// 集合**头三张**成员的 id（叠放封面用）。空数组 = 没进组。
    ///
    /// 后端在列表 SQL 里一次算好，所以画廊**不必**为每个集合格子再发一次请求
    /// （一页 24 格就是 24 次）。成员数超过 3 时只给前 3 张，界面靠 `collectionSize` 显示总数。
    pub collection_preview: Vec<i64>,
    pub prompt_tags: Vec<String>,
    pub file_url: String,
    /// Kotlin 是 `thumbUrl: String? = null`；这里**总是算得出来**，所以键总是在（值不会是 null）
    pub thumb_url: String,
}

/// 从一行 `media_assets LEFT JOIN prompts` 读 DTO（标签与 URL 由调用方/本函数补齐）。
pub fn media_from_row(row: &Row<'_>) -> rusqlite::Result<MediaDto> {
    let id: i64 = row.get("id")?;
    Ok(MediaDto {
        id,
        prompt_id: row.get("prompt_id")?,
        kind: row.get("kind").unwrap_or_else(|_| "IMAGE".to_string()),
        title: row.get("title")?,
        original_name: row.get("original_name")?,
        stored_name: row.get("stored_name")?,
        mime_type: row.get("mime_type")?,
        size_bytes: row.get::<_, Option<i64>>("size_bytes")?.unwrap_or(0),
        width: row.get("width")?,
        height: row.get("height")?,
        duration_ms: row.get("duration_ms")?,
        sha256: row.get("sha256")?,
        source: row.get("source")?,
        favorite: row.get::<_, i64>("favorite").unwrap_or(0) != 0,
        notes: row.get("notes")?,
        nsfw: row.get::<_, i64>("nsfw").unwrap_or(0) != 0,
        created_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("created_at")?.as_deref(),
        ),
        updated_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("updated_at")?.as_deref(),
        ),
        has_workflow: row.get::<_, i64>("has_workflow").unwrap_or(0) != 0,
        prompt_title: row.get("prompt_title")?,
        prompt_positive: row.get("prompt_positive")?,
        prompt_negative: row.get("prompt_negative")?,
        checkpoint: row.get("checkpoint")?,
        seed: row.get("seed")?,
        collection_id: row.get("collection_id")?,
        // 老调用点（`get` 那条按 id 取单条）也用同一个 COLS，所以这里一定读得到；
        // 万一某个查询没带这一列，退回 1（= 只有它自己，界面按普通格子显示）。
        collection_size: row.get::<_, Option<i64>>("collection_size")?.unwrap_or(1),
        collection_preview: parse_id_list(
            row.get::<_, Option<String>>("collection_preview")?
                .as_deref(),
        ),
        prompt_tags: Vec::new(),
        file_url: format!("/api/media/{id}/file"),
        thumb_url: format!("/api/media/{id}/thumb"),
    })
}

/// 一个**集合**的摘要（2026-10-18 手动集合）。
///
/// 给"移到集合…"的选择器用：它要知道有哪些集合、各是什么 kind（不混图像和视频）、
/// 有几个成员、是算法分的还是用户建的。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCollectionDto {
    pub id: i64,
    pub kind: String,
    /// `auto` = 算法分组出来的；`manual` = 用户建的（或被用户改过的）
    pub origin: String,
    /// 用户起的名字。**算法分出来的没有名字**（界面按"算法分组"标记显示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub size: i64,
    /// 选择器里给这一组显示一张封面（第一张成员的 media id）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_media_id: Option<i64>,
    /// 封面是不是 NSFW（选择器里照样打码）。
    pub cover_nsfw: bool,
}

impl From<crate::media::collection::CollectionSummary> for MediaCollectionDto {
    fn from(value: crate::media::collection::CollectionSummary) -> Self {
        Self {
            id: value.id,
            kind: value.kind,
            origin: value.origin,
            title: value.title,
            size: value.size,
            cover_media_id: value.cover_media_id,
            cover_nsfw: value.cover_nsfw,
        }
    }
}
