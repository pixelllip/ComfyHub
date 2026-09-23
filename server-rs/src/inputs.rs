//! 写端点的输入形状（与 Kotlin `Models.kt` 的 `PromptInput` / `TagInput` / `FavoriteInput` /
//! `TagNamesInput` 逐字一致）。
//!
//! 三个"缺失即默认"的约定是从 Kotlin 侧抄来的，别改：
//!  - `PromptInput` 的**每个字段都有默认值**（`#[serde(default)]`）：Flutter 的
//!    `toInputJson()` 只会带上它认为有值的键，少一个键就整条 400 是不能接受的；
//!  - 未知字段**忽略**（`camelCase` 映射之外的键不报错）—— 旧客户端多带个字段不该炸；
//!  - `extraParams` 是 `Map<String,String>`（**值也是字符串**）：Dart 那边 `.map((k,v) => ...toString())`
//!    已经把它拍平了，这里跟着就行。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
// ⚠ `rename_all = "camelCase"` **必须写**：Flutter 的 `toInputJson()` 发的是 `positivePrompt`
// 这种驼峰键，而 serde 默认认 Rust 字段名（snake_case）。漏了它**不会报错** ——
// 每个字段都走 `#[serde(default)]` 变成默认值，于是"POST 建提示词"恒定 400
// （2026-09-23 实测踩到：body 里明明有 positivePrompt，服务端读出来是空串）。
#[serde(rename_all = "camelCase")]
pub struct TagInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoraRefInput {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInput {
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub positive_prompt: String,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub loras: Vec<LoraRefInput>,
    #[serde(default)]
    pub sampler: Option<String>,
    #[serde(default)]
    pub scheduler: Option<String>,
    #[serde(default)]
    pub steps: Option<i64>,
    #[serde(default)]
    pub cfg_scale: Option<f64>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub batch_size: Option<i64>,
    #[serde(default)]
    pub extra_params: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_kind() -> String {
    "IMAGE".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteInput {
    pub favorite: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNamesInput {
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 写接口的**成功回执**（`{"deleted": id}` —— 与 Kotlin 的 `mapOf("deleted" to id)` 同形）。
#[derive(Debug, Clone, Serialize)]
pub struct Deleted {
    pub deleted: i64,
}

/// 收藏回执。
///
/// ⚠ Kotlin 侧这里踩过坑并留了注释：写成 `mapOf("id" to id, "favorite" to favorite)` 会因为
/// **混了 Long + Boolean** 让 kotlinx 序列化直接抛 → 库改好了、响应却是 500。
/// Rust 的 `json!` 没有这个毛病，但**字段名要一致**（`id` / `favorite`）。
#[derive(Debug, Clone, Serialize)]
pub struct FavoriteResult {
    pub id: i64,
    pub favorite: bool,
}
