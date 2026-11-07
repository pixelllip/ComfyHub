//! **官方模板工作流库** —— ComfyUI 安装包自带的那套「模板」（Templates）。
//!
//! 为什么需要它：用户换上一个新模型（Qwen-Image / FLUX / Wan…）时，最靠得住的"这个模型怎么用"
//! 就是官方模板 —— 它已经连好了加载器、采样器、VAE、尺寸，还是官方跟着版本维护的。
//! 在这之前，AI 只能看用户的 `user\<用户>\workflows`（那是**用户自己存过的**），
//! 新模型第一次用的时候那里必然是空的。
//!
//! # 真源与形状（2026-11 的 ComfyUI）
//!
//! ```text
//! <ComfyUI 根>\standalone-env\Lib\site-packages\
//!   comfyui_workflow_templates_core\manifest.json              ← 唯一索引
//!   comfyui_workflow_templates_media_image\templates\*.json     ← 模板正文（按 bundle 分包）
//!   comfyui_workflow_templates_media_api\templates\*.json
//!   comfyui_workflow_templates_media_video\templates\*.json
//!   comfyui_workflow_templates_media_other\templates\*.json
//! ```
//!
//! `manifest.json` 里 `templates[]` 每条有 `id` / `bundle` / `version` / `assets[]`
//! （`assets` 里既有模板 `.json`，也有预览图 `.webp`）。**只认这份索引**，不认识的文件一律不碰。
//!
//! # 四条纪律
//!
//! 1. **只读**：不写文件、不写库、不联网（模板跟着 ComfyUI 版本走，不另设更新源）；
//! 2. **有界**：只在认出来的 ComfyUI 根（与它的上一级）底下找，深度 ≤ [`MAX_DEPTH`]、
//!    最多看 [`MAX_VISITED`] 个目录 —— 用户把 ComfyUI 装在盘根也不会扫全盘；
//! 3. **缺件如实计数**：bundle 包没装 / 文件不在，都不许让整次列举失败，
//!    更不许"假装有"（模型会拿一个不存在的 path 去 `comfy_load_workflow`）；
//! 4. **不猜节点**：模板是界面格式（`nodes` / `links`，其中一部分是组节点 subgraph），
//!    转 API 图是 `comfy_load_workflow` 的活，这里一个字节都不改。
//!
//! # 缓存
//!
//! 刻意不做进程内缓存：ComfyUI 升级 / 换实例后模板就换了，而一次列举只是读一份 JSON 索引
//! （几百 KB，几十毫秒）。重复调用由工具结果缓存（`tool_cache`）兜。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::comfy::comfy_dirs::{format_of, sha256_hex, MAX_FILE_BYTES};

/// 索引所在的那个包（它只有 `manifest.json` + `loader.py`）。
pub const CORE_PACKAGE: &str = "comfyui_workflow_templates_core";

/// 想看别处的索引时用（测试 / 用户自己的模板集）—— **最高优先**。
pub const ENV_MANIFEST: &str = "COMFYHUB_TEMPLATE_MANIFEST";

/// `manifest.json` 里的 `bundle` → 实际装模板正文的包名。
///
/// 这四个是 ComfyUI 现在分包的**全部**；库里出现第五个 bundle 时，
/// 表现是"那几条模板不在列表里"（不是崩溃）—— 到时候在这里加一行即可。
const BUNDLE_PACKAGES: [(&str, &str); 4] = [
    ("media-api", "comfyui_workflow_templates_media_api"),
    ("media-video", "comfyui_workflow_templates_media_video"),
    ("media-image", "comfyui_workflow_templates_media_image"),
    ("media-other", "comfyui_workflow_templates_media_other"),
];

/// 找索引时往下翻几层：`standalone-env\Lib\site-packages\<包>` 从安装根算是第 4 层。
const MAX_DEPTH: usize = 6;

/// 一次探测最多看多少个目录（防止在盘根 / 巨型安装目录上跑很久）。
const MAX_VISITED: usize = 3000;

/// 这些目录名一眼就知道不该进（模型、产物、输入、缓存）。
const SKIP_DIRS: [&str; 11] = [
    ".git",
    "__pycache__",
    "node_modules",
    "models",
    "output",
    "input",
    "temp",
    "custom_nodes",
    "web",
    ".dsh",
    ".github",
];

/// 一次最多列几条（模板总量约 400；列太多对模型没有意义）。
pub const MAX_RESULTS: usize = 100;

/// 一套模板库（一个 `site-packages` 下的一份索引）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// `<…>\comfyui_workflow_templates_core\manifest.json`
    pub manifest: PathBuf,
    /// `<…>\site-packages`（解析 bundle 包用）
    pub site_packages: PathBuf,
}

impl Library {
    /// 从索引路径推出 `site-packages`。
    fn of(manifest: PathBuf) -> Option<Self> {
        let core = manifest.parent()?;
        let site_packages = core.parent()?.to_path_buf();
        Some(Self {
            manifest,
            site_packages,
        })
    }
}

/// 一份官方模板。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// 模板 id（`qwen_image_edit` 这种，就是文件名去掉 `.json`）
    pub id: String,
    /// `media-image` / `media-video` / `media-api` / `media-other`
    pub bundle: String,
    pub version: String,
    /// 模板正文的绝对路径 —— 直接喂 `comfy_load_workflow(path=…)`
    pub file: PathBuf,
    /// 预览图（webp），只作展示；没有就是空
    pub previews: Vec<PathBuf>,
}

/// 一次加载的结果。
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub templates: Vec<Template>,
    /// 索引里有、但文件 / bundle 包不在的条数（**如实报，不假装有**）
    pub missing: usize,
    /// 索引里 bundle 名不认识的条数（ComfyUI 加了新分包时的现象）
    pub unknown_bundles: usize,
}

// ---------------------------------------------------------------------------
//  发现（有界、只认特征）
// ---------------------------------------------------------------------------

/// 找出所有可用的模板库。
///
/// `homes` 是 ComfyUI 根目录（`comfy_dirs::comfy_homes` 那一份）：
/// 每个根**和它的上一级**都会找一遍 —— Desktop 把程序与 `standalone-env` 分成两层，
/// 索引在上一级里，模板正文也就在那儿。
pub fn libraries(homes: &[PathBuf]) -> Vec<Library> {
    let mut out: Vec<Library> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();

    // ① 显式指定的索引（测试 / 用户自己的集）
    if let Ok(raw) = std::env::var(ENV_MANIFEST) {
        let raw = raw.trim();
        if !raw.is_empty() {
            let manifest = PathBuf::from(raw);
            if manifest.is_file() {
                if let Some(lib) = Library::of(manifest.clone()) {
                    if seen.insert(manifest) {
                        out.push(lib);
                    }
                }
            }
        }
    }

    // ② ComfyUI 根 + 它的上一级，有界地找 `comfyui_workflow_templates_core`
    let mut budget = MAX_VISITED;
    for home in homes {
        let mut starts: Vec<PathBuf> = vec![home.clone()];
        if let Some(parent) = home.parent() {
            starts.push(parent.to_path_buf());
        }
        for start in &starts {
            let mut cores: Vec<PathBuf> = Vec::new();
            walk_for_core(start, &mut cores, 0, &mut budget);
            for core in cores {
                let manifest = core.join("manifest.json");
                if !manifest.is_file() {
                    continue;
                }
                if !seen.insert(manifest.clone()) {
                    continue;
                }
                if let Some(lib) = Library::of(manifest) {
                    out.push(lib);
                }
            }
        }
        if budget == 0 {
            break;
        }
    }
    out
}

/// 这些库所在的 `site-packages` —— 只放宽**读**（写仍然只认 `<根>\comfyui`）。
///
/// 模板在 `site-packages` 里，常规布局下已经在 `comfy_dirs::auto_read_roots` 的
/// "ComfyUI 根 + 上一级"里；这条是给**装在别处的 venv**（venv 与 ComfyUI 不同盘 / 不同父目录）
/// 留的兜底，免得模板列得出来、`comfy_load_workflow` 却报路径不准入。
pub fn read_roots(homes: &[PathBuf]) -> Vec<PathBuf> {
    libraries(homes)
        .into_iter()
        .map(|lib| lib.site_packages)
        .collect()
}

/// 在 `dir` 底下（深度 ≤ [`MAX_DEPTH`]）找 `comfyui_workflow_templates_core`。
fn walk_for_core(dir: &Path, out: &mut Vec<PathBuf>, depth: usize, budget: &mut usize) {
    if *budget == 0 || depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // 读不了（权限 / 竞态）就跳过，不让整次列举失败
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == CORE_PACKAGE {
            // 命中就收，不再往里翻（包里只有 manifest.json 与 loader.py）
            *budget -= 1;
            if path.join("manifest.json").is_file() {
                out.push(path);
            }
            continue;
        }
        if SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s)) {
            continue;
        }
        *budget -= 1;
        // `is_dir()` 会跟随 junction / 符号链接：安装目录里这两样很常见
        if path.is_dir() {
            walk_for_core(&path, out, depth + 1, budget);
        }
    }
}

// ---------------------------------------------------------------------------
//  解析索引
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct RawManifest {
    #[serde(default)]
    templates: Vec<RawTemplate>,
}

#[derive(Debug, serde::Deserialize)]
struct RawTemplate {
    id: String,
    #[serde(default)]
    bundle: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct RawAsset {
    filename: String,
}

/// 读一份索引 → 模板清单。
///
/// 失败只有一种：**索引本身读不出来 / 不是 JSON**（那是安装坏了，必须如实报）。
/// 单条缺件不算失败，计进 [`Loaded::missing`]。
pub fn load(lib: &Library) -> Result<Loaded, String> {
    let text = std::fs::read_to_string(&lib.manifest)
        .map_err(|e| format!("模板索引读不出来（{}）：{e}", lib.manifest.display()))?;
    let raw: RawManifest = serde_json::from_str(&text).map_err(|e| {
        format!(
            "模板索引不是合法 JSON（{}）：{e} —— ComfyUI 可能装坏了，模板暂时列不出来",
            lib.manifest.display()
        )
    })?;

    let mut loaded = Loaded::default();
    for t in raw.templates {
        let Some(pkg) = package_of(&t.bundle) else {
            loaded.unknown_bundles += 1;
            continue;
        };
        let dir = lib.site_packages.join(pkg).join("templates");
        let Some(json) = t
            .assets
            .iter()
            .find(|a| a.filename.to_lowercase().ends_with(".json"))
        else {
            loaded.missing += 1;
            continue;
        };
        let file = dir.join(&json.filename);
        if !file.is_file() {
            loaded.missing += 1;
            continue;
        }
        let previews: Vec<PathBuf> = t
            .assets
            .iter()
            .filter(|a| {
                let name = a.filename.to_lowercase();
                name.ends_with(".webp") || name.ends_with(".png") || name.ends_with(".jpg")
            })
            .map(|a| dir.join(&a.filename))
            .filter(|p| p.is_file())
            .collect();
        loaded.templates.push(Template {
            id: t.id,
            bundle: t.bundle,
            version: t.version,
            file,
            previews,
        });
    }
    // 同名（同一 id 装了两份索引）时后者胜，且**顺序稳定**（按 id）
    loaded.templates.sort_by(|a, b| a.id.cmp(&b.id));
    loaded.templates.dedup_by(|a, b| a.id == b.id);
    Ok(loaded)
}

fn package_of(bundle: &str) -> Option<&'static str> {
    BUNDLE_PACKAGES
        .iter()
        .find(|(name, _)| *name == bundle)
        .map(|(_, pkg)| *pkg)
}

// ---------------------------------------------------------------------------
//  搜索（按模型名找模板）
// ---------------------------------------------------------------------------

/// 按关键词找模板。
///
/// 判据刻意简单、可解释：把 id 按 `_` 拆成词，**查询里的每个词都要在 id 里出现过**
/// （子串，大小写不敏感）。命中越多、前缀越靠前越优先 —— 不引入"相似度"那一套，
/// 因为模板 id 本身就是模型名 + 用途（`qwen_image_edit` / `flux_dev_checkpoint`）。
pub fn search<'a>(
    templates: &'a [Template],
    query: Option<&str>,
    limit: usize,
) -> Vec<&'a Template> {
    let cap = limit.clamp(1, MAX_RESULTS);
    let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return templates.iter().take(cap).collect();
    };
    let terms: Vec<String> = query
        .split(|c: char| c.is_whitespace() || c == ',' || c == '/')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return templates.iter().take(cap).collect();
    }
    let mut scored: Vec<(u32, &Template)> = templates
        .iter()
        .filter_map(|t| {
            let id = t.id.to_lowercase();
            if !terms.iter().all(|term| id.contains(term)) {
                return None;
            }
            let mut score = 0u32;
            for term in &terms {
                if id.starts_with(term.as_str()) {
                    score += 3;
                } else if id.split('_').any(|word| word == term) {
                    score += 2;
                } else {
                    score += 1;
                }
            }
            Some((score, t))
        })
        .collect();
    // 分高的在前；同分时短 id 在前（`qwen_image` 比 `qwen_image_edit_subgraphed` 更像"主力模板"）
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.len().cmp(&b.1.id.len())));
    scored.into_iter().take(cap).map(|(_, t)| t).collect()
}

// ---------------------------------------------------------------------------
//  给模型 / 界面看的 JSON
// ---------------------------------------------------------------------------

/// 一份模板的「可用性」补充（读文件才知道的那几个字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    pub bytes: u64,
    pub format: String,
    pub sha256: String,
}

/// 读一份模板的字节数 / 格式 / sha256。
///
/// 格式与 `comfy_dirs::scan` **同一份判据**（[`format_of`]）—— 两边不一致就会出现
/// "模板列表说能用、`comfy_load_workflow` 说不是工作流"。
pub fn detail(t: &Template) -> Option<Detail> {
    let meta = std::fs::metadata(&t.file).ok()?;
    let bytes = meta.len();
    if bytes == 0 || bytes > MAX_FILE_BYTES {
        return None;
    }
    let raw = std::fs::read(&t.file).ok()?;
    let format = format_of(&raw);
    Some(Detail {
        bytes,
        format,
        sha256: sha256_hex(&raw),
    })
}

/// 组装工具 / 端点的返回体。
///
/// `prompt_id_of` 由调用方注入（拿 sha256 问库"这份是不是已经在库里了"）——
/// 这样磁盘那一半可以在没有数据库的情况下单测（与 `comfy_dirs::list_json` 同一条理由）。
pub fn list_json(
    libs: &[Library],
    query: Option<&str>,
    limit: usize,
    prompt_id_of: impl Fn(&str) -> Option<i64>,
) -> serde_json::Value {
    let mut all: Vec<Template> = Vec::new();
    let mut missing = 0usize;
    let mut unknown_bundles = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for lib in libs {
        match load(lib) {
            Ok(loaded) => {
                missing += loaded.missing;
                unknown_bundles += loaded.unknown_bundles;
                all.extend(loaded.templates);
            }
            // 一份索引坏了不该让别的索引也列不出来
            Err(message) => errors.push(message),
        }
    }
    // 多份索引时同一 id 只留一份（顺序稳定）
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all.dedup_by(|a, b| a.id == b.id);

    let total = all.len();
    let matched: Vec<&Template> = search(&all, query, limit.clamp(1, MAX_RESULTS));
    let entries: Vec<serde_json::Value> = matched
        .iter()
        .map(|t| {
            let detail = detail(t);
            let prompt_id = detail.as_ref().and_then(|d| prompt_id_of(&d.sha256));
            let available = detail
                .as_ref()
                .map(|d| d.format != "unknown")
                .unwrap_or(false);
            let mut m = serde_json::Map::new();
            m.insert("id".into(), serde_json::json!(t.id));
            m.insert("bundle".into(), serde_json::json!(t.bundle));
            m.insert("version".into(), serde_json::json!(t.version));
            m.insert(
                "path".into(),
                serde_json::json!(t.file.to_string_lossy().to_string()),
            );
            m.insert(
                "bytes".into(),
                serde_json::json!(detail.as_ref().map(|d| d.bytes)),
            );
            m.insert(
                "format".into(),
                serde_json::json!(detail.as_ref().map(|d| d.format.clone())),
            );
            // 转不了的模板**仍列出来**（模型可能想知道它在不在），但明说别拿它开跑
            m.insert("loadable".into(), serde_json::json!(available));
            m.insert("previews".into(), serde_json::json!(t.previews.len()));
            m.insert("inLibrary".into(), serde_json::json!(prompt_id.is_some()));
            if let Some(id) = prompt_id {
                m.insert("promptId".into(), serde_json::json!(id));
            }
            serde_json::Value::Object(m)
        })
        .collect();

    let mut obj = serde_json::Map::new();
    obj.insert("ok".into(), serde_json::json!(errors.is_empty()));
    obj.insert("total".into(), serde_json::json!(total));
    obj.insert("count".into(), serde_json::json!(entries.len()));
    obj.insert("templates".into(), serde_json::json!(entries));
    if missing > 0 {
        obj.insert("missingFiles".into(), serde_json::json!(missing));
    }
    if unknown_bundles > 0 {
        obj.insert("unknownBundles".into(), serde_json::json!(unknown_bundles));
    }
    if !errors.is_empty() {
        obj.insert("errors".into(), serde_json::json!(errors));
    }

    // 正文那句话：只说事实 + 下一步该怎么走（不写 Markdown，工具结果进的是 Text）
    let hint = "这些是 ComfyUI 自带的官方模板。拿到 path 直接 comfy_load_workflow(path=…) 读进库，\
                能跑就用 comfy_submit 提交；想以后换模型只改参数，再跟一句 comfy_learn_workflow 把它学成配方。";
    obj.insert("hint".into(), serde_json::json!(hint));

    if total == 0 {
        obj.insert(
            "message".into(),
            serde_json::json!(
                "没找到 ComfyUI 自带的模板库（site-packages 里的 comfyui_workflow_templates_core）。\
                 可能是这个版本的 ComfyUI 没带模板，或者它的位置没被认出来。\
                 可以先用 comfy_list_workflows 看用户自己保存的工作流。"
            ),
        );
    } else if entries.is_empty() {
        let message = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => format!(
                "自带的 {total} 份模板里没有匹配「{q}」的（查询词要按模型名来，例如 qwen、flux、wan；不带 query 可以看全部）"
            ),
            None => format!("自带的 {total} 份模板一份也读不出来"),
        };
        obj.insert("message".into(), serde_json::json!(message));
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests;
