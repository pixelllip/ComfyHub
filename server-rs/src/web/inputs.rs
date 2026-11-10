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
    /// 发送模式（2026-10-11）：`keep`（默认，照原样带上负面提示词）/
    /// `positive_only`（有些 turbo 模型不认负面提示词，发给 Comfy 前去掉）。
    ///
    /// 认不出来的值**归一到 `keep`** —— 绝不因为一个模式串写错就让整条提示词存不进去。
    #[serde(default)]
    pub negative_mode: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 把外来的 `negativeMode` 归一到两个合法值之一（认不出来就是 `keep`）。
pub fn normalized_negative_mode(raw: Option<&str>) -> &'static str {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("positive_only") | Some("positiveonly") | Some("positive") => "positive_only",
        _ => "keep",
    }
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
/// `PATCH /api/media/{id}` 的请求体（与 Kotlin `MediaUpdate` 逐字一致）。
///
/// ⚠ `clearPrompt` 与 `promptId: null` 语义不同：前者是"**明确解除关联**"（把列写成 NULL），
/// 后者是"这次不动这一列"。少了 `clearPrompt`，界面就永远解绑不了一条产物。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaUpdate {
    #[serde(default)]
    pub prompt_id: Option<i64>,
    #[serde(default)]
    pub clear_prompt: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub favorite: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
    /// 手动标记 / 取消标记成人内容（只影响预览图打不打码）
    #[serde(default)]
    pub nsfw: Option<bool>,
    /// 手动**移进**某个集合（2026-10-18）。与 `promptId` 同形：`None` = 这次不动归属。
    ///
    /// 目标集合的 `kind` 必须与产物一致，否则 400（"手动集合不混图像和视频"）。
    #[serde(default)]
    pub collection_id: Option<i64>,
    /// 手动**移出**集合（2026-10-18）。与 `clearPrompt` 同形 —— 少了它，界面就永远
    /// 拆不开一组。⚠ 落库时会同时写 `collection_opt_out = 1`，否则下次启动的
    /// `backfill` 会把这一条**静默吸回原组**。
    #[serde(default)]
    pub clear_collection: bool,
    /// **整体替换**产物自身的标签（2026-11-10：支持手动加 / 删标签）。
    ///
    /// `None` = 这次不动标签；空数组 = 把自身标签清空（关联提示词的标签仍会合并展示）。
    /// 与 `promptId: null` / `clearPrompt` 的"缺省 ≠ 清空"是同一套语义。
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}
