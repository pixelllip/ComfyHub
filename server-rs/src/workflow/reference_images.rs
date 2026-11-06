//! 工作流里 **`LoadImage` 指向的参考图**：把 widget 里的文件名解析成磁盘上的真实路径、
//! 原始分辨率与体积，交给执行器**自动带进下一轮**。
//!
//! # 为什么需要它（2026-10-25 的现场）
//!
//! 用户的工作流里 `LoadImage.image` 写着 `image (15).png`，文件就在 ComfyUI 的 `input`
//! 目录里。可模型只拿到这个名字：既没有路径、也没有像素 —— 于是它**照着文件名把画面编了出来**
//! （参考图的粉色房间、猫气球、显示器一个没写，倒多出木地板和窗帘）。
//! 「图在工作流里」和「模型看得到图」之间缺的就是这一条路。
//!
//! # 只解析、不投放
//!
//! 这里**不拷任何文件**：`LoadImage` 读的就是 ComfyUI `input` 目录里那一份，模型要看的
//! 也是同一份。投放（`comfy_use_attachment` / `comfy_use_media`）是另一个方向的事。
//!
//! # 判据只有这一份
//!
//! 路径按 [`crate::comfy::image_edit::input_dir_candidates`]（跟着实例留档 / 配置 /
//! Desktop 设置 / 盘探走）去找，**不自己拼** ComfyUI 的安装路径。找不到就如实
//! `exists: false` + 一句说明 —— 不猜、不静默丢（AIH-030 的同一条纪律）。
//!
//! # 文件名可以做坏事
//!
//! 工作流文件是用户从网上拿来的，`image` 里写 `..\..\..\Windows\win.ini` 完全可能。
//! 这里只接受**相对路径**（逐段只放行普通段，`..` / 盘符 / 根目录一律拒绝）——
//! 解析出来的东西会被读成 base64 发给模型，越界读等于把用户机器上的文件送出去。

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::web::routes::AppState;

/// 一份工作流最多解析几张参考图（超过的部分不列 —— 结果要塞进模型的上下文）。
pub const MAX_REFERENCE_IMAGES: usize = 8;

/// 一张参考图（一个 `LoadImage` 节点对应一条）。
#[derive(Debug, Clone)]
pub struct ReferenceImage {
    /// 节点 id（`comfy_submit` 的 overrides 键要用它：`<id>.image`）
    pub node_id: String,
    /// 节点里写的文件名（ComfyUI 视角，可能是 `子目录/文件.png`）
    pub file: String,
    /// 磁盘上的真实路径（解析到了才有）
    pub path: Option<PathBuf>,
    pub exists: bool,
    /// 原始分辨率（读不出来就没有这两个键）
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<u64>,
    /// `exists=false` 时的一句人话
    pub note: Option<String>,
}

impl ReferenceImage {
    /// 给模型 / 界面看的那一份（键名与 `comfy_load_workflow` 结果里的 `referenceImages` 一致）。
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("nodeId".into(), json!(self.node_id));
        out.insert("file".into(), json!(self.file));
        out.insert(
            "path".into(),
            match &self.path {
                Some(path) => json!(path.to_string_lossy()),
                None => Value::Null,
            },
        );
        out.insert("exists".into(), json!(self.exists));
        if let Some(width) = self.width {
            out.insert("width".into(), json!(width));
        }
        if let Some(height) = self.height {
            out.insert("height".into(), json!(height));
        }
        if let Some(bytes) = self.bytes {
            out.insert("bytes".into(), json!(bytes));
        }
        if let Some(note) = &self.note {
            out.insert("note".into(), json!(note));
        }
        Value::Object(out)
    }
}

/// 从一份 API 节点图里找出所有 `LoadImage` 的参考图（真实实现：目录按本机候选推）。
pub fn collect(state: &AppState, graph: &Map<String, Value>) -> Vec<ReferenceImage> {
    // 候选目录**只探一次**：`find_home` / Desktop 设置都要碰盘，两次调用白白翻倍
    let candidates = crate::comfy::image_edit::input_dir_candidates(state);
    let inputs: Vec<PathBuf> = candidates.into_iter().map(|(dir, _)| dir).collect();
    let outputs = output_dirs_of(state, &inputs);
    collect_in(graph, &inputs, &outputs)
}

/// [`collect`] 的纯函数那一半（用例直接喂目录，不依赖本机装没装 ComfyUI）。
pub fn collect_in(
    graph: &Map<String, Value>,
    input_dirs: &[PathBuf],
    output_dirs: &[PathBuf],
) -> Vec<ReferenceImage> {
    let mut out: Vec<ReferenceImage> = Vec::new();
    for (node_id, node) in graph {
        let Some(object) = node.as_object() else {
            continue;
        };
        if object.get("class_type").and_then(Value::as_str) != Some("LoadImage") {
            continue;
        }
        let image = object
            .get("inputs")
            .and_then(Value::as_object)
            .and_then(|inputs| inputs.get("image"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        // 连线（数组）不是文件名；空值也没有可解析的东西
        let Some(image) = image else { continue };
        out.push(resolve_one(node_id, image, input_dirs, output_dirs));
        if out.len() >= MAX_REFERENCE_IMAGES {
            break;
        }
    }
    out
}

/// `LoadImage.image` 的三种写法（ComfyUI 的形状，别按我们以为的归一）：
///
/// * `image (15).png` —— 就是 `input` 目录下的裸文件名；
/// * `子目录/xxx.png` —— `input` 下的子目录；
/// * `clipspace/xxx.png [input]` —— 前端临时图（`[input]` / `[output]` / `[temp]` 是
///   ComfyUI 标"这个文件在哪个目录"的后缀）。`clipspace` 是浏览器里那块临时画布，
///   磁盘上通常没有，找不到就如实说。
fn resolve_one(
    node_id: &str,
    raw: &str,
    input_dirs: &[PathBuf],
    output_dirs: &[PathBuf],
) -> ReferenceImage {
    let (name, hint) = split_hint(raw);
    let mut entry = ReferenceImage {
        node_id: node_id.to_string(),
        file: name.clone(),
        path: None,
        exists: false,
        width: None,
        height: None,
        bytes: None,
        note: None,
    };
    let Some(relative) = safe_relative(&name) else {
        entry.note = Some(format!(
            "文件名不受支持（只接受 ComfyUI input 目录下的相对路径）：{raw}"
        ));
        return entry;
    };
    // `[output]` 的文件在 output 目录；其余（含没后缀的）先看 input，再看 output ——
    // 有些人的工作流是拿 output 里的旧产物当参考图（ComfyUI 自己也会这么存）。
    let mut dirs: Vec<&PathBuf> = Vec::new();
    match hint {
        Some("output") => {
            dirs.extend(output_dirs.iter());
            dirs.extend(input_dirs.iter());
        }
        Some("temp") => {
            // 临时目录不在候选里（它随实例重启就清空）；只留一句说明
            entry.note =
                Some("这是 ComfyUI 的临时图（temp / clipspace），磁盘上通常不留档".to_string());
        }
        _ => {
            dirs.extend(input_dirs.iter());
            dirs.extend(output_dirs.iter());
        }
    }
    for dir in dirs {
        let candidate = dir.join(&relative);
        if candidate.is_file() && stays_inside(dir, &candidate) {
            let size = std::fs::metadata(&candidate).ok().map(|meta| meta.len());
            let dimensions = image::image_dimensions(&candidate).ok();
            entry.path = Some(candidate);
            entry.exists = true;
            entry.bytes = size;
            entry.width = dimensions.map(|(w, _)| w);
            entry.height = dimensions.map(|(_, h)| h);
            if dimensions.is_none() {
                entry.note = Some(
                    "文件在，但读不出图片尺寸（可能不是图片，或用了没有解码器的格式）".to_string(),
                );
            }
            return entry;
        }
    }
    if entry.note.is_none() {
        entry.note = Some(format!(
            "ComfyUI 的 input / output 目录里都没有这个文件（工作流可能是从别的实例拷来的，或文件已被删）：{name}"
        ));
    }
    entry
}

/// `image (15).png [input]` → `("image (15).png", Some("input"))`。
fn split_hint(raw: &str) -> (String, Option<&'static str>) {
    let trimmed = raw.trim();
    for (suffix, kind) in [
        (" [input]", "input"),
        (" [output]", "output"),
        (" [temp]", "temp"),
    ] {
        if let Some(name) = trimmed.strip_suffix(suffix) {
            return (name.trim().to_string(), Some(kind));
        }
    }
    (trimmed.to_string(), None)
}

/// 只放行**相对路径**：逐段只收普通段（`..`、盘符、根目录一律拒绝 —— 见文件头）。
fn safe_relative(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 真实路径还在那个目录里吗（防 input 目录里有人放了符号链接 / 目录联接指向外面）。
///
/// 词法上的 `..` 已经在 [`safe_relative`] 挡住；这一层防的是"名字合法、链接指外面"。
/// 解析不了就按词法路径放行 —— 能走到这里说明 `is_file()` 已经为真，解析失败极罕见。
fn stays_inside(dir: &Path, candidate: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::fs::canonicalize(candidate)) {
        (Ok(root), Ok(real)) => real.starts_with(&root),
        _ => true,
    }
}

/// 本机的 ComfyUI `output` 目录候选（`[output]` 后缀与"拿旧产物当参考图"那一档用）。
///
/// 与 input 不同，output **没有**"实例自己报"的入口（`/object_info` 里没有它），
/// 所以顺序是：配置里的输出目录 → input 候选的兄弟 `output`。两个都没有就空着 ——
/// 反正 input 已经覆盖绝大多数工作流。
fn output_dirs_of(state: &AppState, inputs: &[PathBuf]) -> Vec<PathBuf> {
    let configured = crate::comfy::capture::capture_config(state)
        .ok()
        .and_then(|config| config.output_dir)
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(dir) = configured {
        let path = PathBuf::from(&dir);
        let is_input = path
            .file_name()
            .map(|name| name.to_string_lossy().eq_ignore_ascii_case("input"))
            .unwrap_or(false);
        let candidate = if is_input {
            path.parent().map(|base| base.join("output"))
        } else {
            Some(path)
        };
        if let Some(dir) = candidate {
            out.push(dir);
        }
    }
    for input in inputs {
        if let Some(parent) = input.parent() {
            out.push(parent.join("output"));
        }
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for dir in out {
        if !dir.is_dir() {
            continue;
        }
        if unique.iter().any(|kept| {
            kept.to_string_lossy()
                .eq_ignore_ascii_case(&dir.to_string_lossy())
        }) {
            continue;
        }
        unique.push(dir);
    }
    unique
}

#[cfg(test)]
mod tests;
