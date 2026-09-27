//! 图像编辑的**输入准备**（Rust 侧对照 Kotlin `AiImageEdit.kt` + `AiWorkflowSearch.useAttachment`）。
//!
//! # 这一层解决什么
//!
//! 用户的原话是"**保持背景不变，更换有具体动作的人物并保持画风**" ——
//! 做法是**先生成人物、再用图像编辑把人换上去**；反过来"换背景保留人物"用同一个工作流，
//! 只是遮罩取反。这条路缺的不是"提交"，而是**两件只能由我们做的事**：
//!
//! 1. **把图投放进 ComfyUI 的 `input` 目录** —— 工作流里的 `LoadImage` / `LoadImageMask`
//!    **只能读那一个目录**，而阶段一的产物在画廊里（[`use_media`]）、用户随消息发来的图
//!    在附件库（[`use_attachment`]）。模型既改不了这个目录，也编不出一个真实存在的文件名。
//! 2. **生成遮罩**（[`make_mask`]）—— 局部重绘必须有遮罩，而用户在手机 / 平板上画不出来，
//!    ComfyUI 的 MaskEditor 又要人坐在电脑前手描。这里用"归一化矩形 / 椭圆 + 反选 + 羽化"
//!    表达意图，够用且可复现。
//!
//! # 遮罩的语义（**白 = 要重新生成的地方**）
//!
//! 与 ComfyUI 的约定一致（`SetLatentNoiseMask` 认这个方向），所以两种需求**同一个工作流**：
//!
//! | 需求 | region | invert |
//! | --- | --- | --- |
//! | **保背景换人物** | 人物所在的那块 | false |
//! | **换背景保人物** | 人物所在的那块 | **true** |
//! | 整张重画（等价图生图） | `full` | false |
//!
//! ⚠ `region` 是**近似**的：这里不做分割，只是照给的框画。所以结果里必须回
//! `approximate=true` 与它实际用的框，模型要如实转述"框是我估的、不准就报新坐标"。
//!
//! # 与 Kotlin 的逐条对应（R12：行为不能两端分叉）
//!
//! | 这里 | Kotlin |
//! | --- | --- |
//! | [`resolve_input_dir`] | `AiWorkflowSearch.resolveComfyInputDir` |
//! | [`input_filename_for`] | `AiWorkflowSearch.inputFilenameFor` |
//! | [`use_attachment`] | `AiWorkflowSearch.useAttachment` |
//! | [`use_media`] | `AiImageEdit.useMedia` |
//! | [`make_mask`] | `AiImageEdit.makeMask`（含 `parseRegion` / `boxBlur` / `contentTag`） |
//!
//! 逐条对照的**判据**是同一组参数得到的文件名与遮罩像素：文件名靠
//! `<原名>-<id 前 8 位><扩展名>` 这套命名规则（重复调用得到同一个文件，
//! 重跑工作流不会指向一个已经不在的图），遮罩靠"白区在哪"与羽化半径。
//! 单测盯的就是这两件事（`make_mask_白区与羽化` / `投放文件名可重复且不互相覆盖`）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::routes::AppState;
use crate::tools::ToolFailure;

/// 遮罩边长上限（与产物同一量级；再大就是误传了）。
pub const MAX_MASK_EDGE: i64 = 8192;

/// 羽化上限（像素）。
pub const MAX_FEATHER: i64 = 256;

/// `mode="subject"` 的默认容差（背景色距离的归一化阈值）。
///
/// 为什么要有这个旋钮：生成图的"白底"从来不是纯 255（有渐变、噪点、浅浅的圆形水印纹），
/// 卡太死会把背景留成一片斑点；放大一点又会把人物的浅色部分吃掉。
/// 实测 768x1024 的白底人物图上 0.12 两边都稳。
pub const SUBJECT_TOLERANCE_DEFAULT: f64 = 0.12;

/// 判定"背景是**彩色的**（绿幕 / 品红那种 chroma key）"的饱和度门槛（最大通道差）。
///
/// 白 / 浅灰背景的通道差只有几，绿幕 / 品红是 100+ —— 这条线把它们分开，
/// 于是"要不要填内部空洞"能自动分档（见 subject_mask_from_image 的 ③）。
pub const CHROMA_MIN_SATURATION: i32 = 40;

/// `mode="subject"` 默认往里收几像素（choke）—— 扔掉主体与背景交界那圈溢色。
pub const DEFAULT_SUBJECT_SHRINK: i64 = 0;

/// 按颜色收边的默认参数（`band` 像素 / 容差）。
///
/// 0.35 比抠图那档（0.12）宽得多是有意的：只看**边界 3 像素**那条带，
/// 那条带里的像素本来就是"人 + 背景"的混合色，容差放宽才收得掉；
/// 而发丝（头发色，离背景色远）怎么放宽都还在。
pub const DEFAULT_EDGE_BAND: i64 = 3;
pub const DEFAULT_EDGE_TOLERANCE: f64 = 0.25;

/// `matteSource="model"` 档去溢色的**外侧**默认半径（像素）。
///
/// 为什么外侧要单独给一个（而不是沿用内侧那 6）：软遮罩的边界比颜色剪影**宽** ——
/// 模型给的是连续 alpha，"人 + 背景"的混合像素有好几像素宽，且**整段都在剪影外面**
/// （见 [`despill_band`] 的实测表）。8 是绿幕那档量出来"清零"的下界，再大只是浪费。
pub const DEFAULT_OUTER_BAND: i64 = 8;

// ---------------------------------------------------------------------------
//  ComfyUI 的 input 目录
// ---------------------------------------------------------------------------

/// ComfyUI 的 `input` 目录。
///
/// 优先从**已经配置好的输出目录**推（capture 配置里的 outputDir 与 input 是兄弟目录，
/// 这是 ComfyUI 的固定布局）；推不出来再走 [`crate::comfy_dirs::find_home`] 的探测。
/// 探测**只读**：找不到就如实报错，**绝不在随机位置建目录**。
///
/// ⚠ 为什么不能只按安装目录推：ComfyUI Desktop 把"程序"与"共享数据（input / output / models）"
/// 分在两处，跑着的实例读写的是**共享**那一份。本机实测：安装目录
/// `…\ComfyUI-Installs\ComfyUI\ComfyUI\input` 有 15 个文件，而实例真正在用的是
/// `…\ComfyUI-Shared\input`（50 个）—— 写错地方的表现是 `Invalid image file`
/// （看着像"文件不存在"，其实是写进了另一个目录）。判据与 Kotlin 那份**逐字一致**。
pub fn resolve_input_dir(state: &AppState) -> Result<PathBuf, ToolFailure> {
    let configured = crate::capture::capture_config(state)
        .ok()
        .and_then(|config| config.output_dir)
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    let home = crate::comfy_dirs::find_home(&state.project_root(), configured.as_deref());
    input_dir_from(configured.as_deref(), home)
}

/// [`resolve_input_dir`] 的**纯函数那一半**：配置的输出目录 → 探测到的 home → 如实报错。
///
/// 拆出来是为了能测"两处都没有"那一支：本机（开发机）装着 ComfyUI，
/// `find_home` 一定探得到，于是"找不到"这条分支在集成用例里**永远走不到**
/// —— 而它恰恰是新用户第一次用时会撞上的那一支（发布包里没有 ComfyUI 目录）。
fn input_dir_from(configured: Option<&str>, home: Option<PathBuf>) -> Result<PathBuf, ToolFailure> {
    if let Some(dir) = configured.map(str::trim).filter(|dir| !dir.is_empty()) {
        let path = PathBuf::from(dir);
        let is_output = path
            .file_name()
            .map(|name| name.to_string_lossy().eq_ignore_ascii_case("output"))
            .unwrap_or(false);
        let base = if is_output { path.parent().map(Path::to_path_buf) } else { Some(path) };
        if let Some(base) = base {
            let input = base.join("input");
            if input.is_dir() {
                return Ok(input);
            }
        }
    }
    if let Some(home) = home {
        let input = home.join("input");
        if input.is_dir() {
            return Ok(input);
        }
    }
    Err(ToolFailure::new(
        "COMFY_NOT_FOUND",
        "没找到 ComfyUI 的 input 目录（投放的图与遮罩必须放在那里，LoadImage / LoadImageMask 才读得到）。\
         让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置后再试。",
    ))
}

/// 投放进 `input` 目录的文件名。
///
/// 名字里带**来源 id 的前 8 位**：同一个图片来源永远得到同一个名字（重跑工作流不会指向一个
/// 已经没有的文件），不同来源即使原文件名相同也不会互相覆盖（`image.png` 满天飞是常态）。
/// ComfyUI 的 `LoadImage` 只认裸文件名，所以路径分隔符之类的非法字符一律清掉。
pub fn input_filename_for(original_name: &str, tag: &str, wanted_name: Option<&str>) -> String {
    let raw = wanted_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(original_name)
        .trim();
    let safe: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    // Kotlin：`trimStart('.')` → `takeLast(120)` → 空则 `image.png`
    let safe: String = safe.trim_start_matches('.').to_string();
    let chars: Vec<char> = safe.chars().collect();
    let safe: String = if chars.len() > 120 {
        chars[chars.len() - 120..].iter().collect()
    } else {
        safe
    };
    let safe = if safe.is_empty() { "image.png".to_string() } else { safe };

    let dot = safe.rfind('.');
    // `dot > 0` 而不是 `>= 0`：`.png` 这种"只有扩展名"的形状要当**没有扩展名**处理
    let (stem, ext) = match dot {
        Some(index) if index > 0 => (&safe[..index], &safe[index..]),
        _ => (safe.as_str(), ".png"),
    };
    let stem = if stem.is_empty() { "image" } else { stem };
    let ext: String = ext.chars().take(10).collect();
    let tag: String = tag.chars().filter(|c| c.is_alphanumeric()).take(8).collect();
    let tag = if tag.is_empty() { "ref".to_string() } else { tag };
    format!("{stem}-{tag}{ext}")
}

/// 只取文件名（挡 `..\` 之类的目录穿越；ComfyUI 也只认裸文件名）。
fn file_name_only(raw: &str) -> String {
    let tail = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    tail.chars().take(160).collect()
}

/// 拷贝进 input 目录（内容相同就跳过 —— 同一个图重复投放时省一次几 MB 的拷贝）。
fn copy_if_needed(source: &Path, target: &Path) -> Result<(), ToolFailure> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ToolFailure::new(
                "COMFY_INPUT_FAILED",
                format!("写不进 ComfyUI 的 input 目录（{}）：{}", parent.display(), take_text(&e.to_string(), 200)),
            )
        })?;
    }
    let same = match (std::fs::metadata(target), std::fs::metadata(source)) {
        (Ok(to), Ok(from)) => to.len() == from.len(),
        _ => false,
    };
    if !same {
        std::fs::copy(source, target).map_err(|e| {
            ToolFailure::new(
                "COMFY_INPUT_FAILED",
                format!("写不进 ComfyUI 的 input 目录（{}）：{}", target.display(), take_text(&e.to_string(), 200)),
            )
        })?;
    }
    Ok(())
}

/// 一句人话的截断（与 Kotlin 的 `message?.take(200)` 同义）。
fn take_text(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 原图尺寸（读不出来回 `None` —— 调用方按"如实报错"处理）。
fn read_size(file: &Path) -> Option<(i64, i64)> {
    image::image_dimensions(file).ok().map(|(w, h)| (w as i64, h as i64))
}

// ---------------------------------------------------------------------------
//  投放：画廊产物 / 用户附件
// ---------------------------------------------------------------------------

/// `comfy_use_media`：把**画廊里的一个图片产物**放进 ComfyUI 的 `input` 目录。
///
/// 与 [`use_attachment`] 是**同一个池子的两个来源**：那条走 `<storage>/ai-attachments`，
/// 这条走 `<storage>/media`。为什么非要有它：两阶段流程里"阶段一的产物"天然在画廊，
/// 而不在聊天附件里 —— 没有它，模型只能让用户手动去 ComfyUI 里找那张图。
///
/// 返回 `{mediaId, filename, inputDir, width?, height?, hint}`：`width/height` 是**原图尺寸**，
/// 做遮罩时要照它给（遮罩与底图尺寸不一致 = 采样直接炸）。解不出来就不给这两个键，
/// **但不拦**（模型可以显式给 width/height）。
pub fn use_media(state: &AppState, media_id: i64, wanted_name: Option<&str>) -> Result<Value, ToolFailure> {
    let media = crate::media::get(state, media_id).map_err(|_| {
        ToolFailure::new(
            "MEDIA_NOT_FOUND",
            format!("画廊里没有 id={media_id} 的产物（先用媒体列表 / 刚生成的 mediaIds 确认它还在）"),
        )
    })?;
    if !media.kind.eq_ignore_ascii_case("image") {
        return Err(ToolFailure::new(
            "NOT_AN_IMAGE",
            format!(
                "产物 #{media_id}「{}」是 {}，不是图片。只有图片能放进 ComfyUI 的 input 目录当底图 / 参考图 / 遮罩。",
                media.title, media.kind
            ),
        ));
    }
    let source = crate::media::resolve_media(state.media_dir(), &media.stored_name).ok_or_else(|| {
        ToolFailure::new(
            "MEDIA_NOT_FOUND",
            format!("产物 #{media_id} 的落盘名不合法：{}", media.stored_name),
        )
    })?;
    if !source.is_file() {
        return Err(ToolFailure::new(
            "MEDIA_NOT_FOUND",
            format!("产物 #{media_id} 的原件不在盘上了：{}", source.display()),
        ));
    }

    let input_dir = resolve_input_dir(state)?;
    let filename = input_filename_for(&media.original_name, &format!("m{media_id}"), wanted_name);
    copy_if_needed(&source, &input_dir.join(&filename))?;

    let mut out = json!({
        "mediaId": media_id,
        "filename": filename,
        "inputDir": input_dir.to_string_lossy(),
        "hint": format!(
            "用 comfy_submit 覆盖工作流里 LoadImage 节点的 image 输入：{{\"<节点id>.image\":\"{filename}\"}}；\
             要遮罩就用 comfy_make_mask(baseFilename=\"{filename}\", region=…) —— 它会照这张图的尺寸生成遮罩。"
        ),
    });
    if let Some((w, h)) = read_size(&source) {
        out["width"] = json!(w);
        out["height"] = json!(h);
    }
    Ok(out)
}

/// `comfy_use_attachment`：把**用户随消息发来的图片附件**放进 ComfyUI 的 `input` 目录。
///
/// 三条纪律（与 Kotlin 逐条对应）：
///  1. **只投图片**：视频 / 音频 / 文本附件放进去 ComfyUI 也读不了，直接如实报错；
///  2. **文件名由我们算**（[`input_filename_for`]）：确定、可重复，同名不会互相覆盖；
///  3. **找不到 ComfyUI 就说找不到**：不猜路径、不建目录到随机位置。
pub fn use_attachment(
    state: &AppState,
    attachment_id: &str,
    wanted_name: Option<&str>,
) -> Result<Value, ToolFailure> {
    let dto = crate::attachments::get(state, attachment_id)
        .ok()
        .flatten()
        .ok_or_else(|| {
            ToolFailure::new(
                "ATTACHMENT_NOT_FOUND",
                format!("没有 id={attachment_id} 的附件（附件 id 见用户消息里的附件说明）"),
            )
        })?;
    if !dto.kind.eq_ignore_ascii_case("image") {
        return Err(ToolFailure::new(
            "NOT_AN_IMAGE",
            format!(
                "附件「{}」是 {}，不是图片。只有图片能放进 ComfyUI 的 input 目录当作图生图 / 参考图。",
                dto.name,
                if dto.modality.is_empty() { "未知类型".to_string() } else { dto.modality.clone() }
            ),
        ));
    }
    let source = crate::attachments::file_of(state, attachment_id)
        .ok()
        .flatten()
        .ok_or_else(|| {
            ToolFailure::new(
                "ATTACHMENT_NOT_FOUND",
                format!("附件「{}」的原件已经不在了（可能被清理过）", dto.name),
            )
        })?;
    if !source.is_file() {
        // 库里有行、盘上没文件：如实说，别走到拷贝那一步再报一个看不懂的 IO 错
        return Err(ToolFailure::new(
            "ATTACHMENT_NOT_FOUND",
            format!("附件「{}」的原件文件不在了：{}", dto.name, source.display()),
        ));
    }

    let input_dir = resolve_input_dir(state)?;
    let filename = input_filename_for(&dto.name, attachment_id, wanted_name);
    copy_if_needed(&source, &input_dir.join(&filename))?;

    Ok(json!({
        "attachmentId": attachment_id,
        "filename": filename,
        "inputDir": input_dir.to_string_lossy(),
        "hint": format!(
            "用 comfy_submit 覆盖工作流里 LoadImage 节点的 image 输入：{{\"<节点id>.image\":\"{filename}\"}}\
             （节点 id 用 comfy_list_workflows / comfy_load_workflow 的 includeGraph=true 查）。"
        ),
    }))
}

// ---------------------------------------------------------------------------
//  遮罩
// ---------------------------------------------------------------------------

/// `comfy_make_mask`：生成一张遮罩 PNG 放进 ComfyUI 的 `input` 目录（**白 = 要重新生成的地方**）。
///
/// 参数（类型一律容错，与 Kotlin 的 `str` / `int` / `bool` 同义）：
///  - `mode`：`rect`（默认，按框画）/ **`subject`（照某张图里主体的形状抠** —— 贴人物图必须用它，
///    否则 `ImageCompositeMasked` 会把人物图那块矩形**连背景一起**贴进去 = 一圈白框）；
///  - `subjectFilename`：`mode="subject"` 时要抠形状的那张图（input 目录里的裸文件名）；
///  - `tolerance`：`mode="subject"` 的背景色容差（默认 0.12，越大越激进）；
///  - `baseFilename`：input 目录里的底图文件名 —— 给了就用**它的尺寸**（推荐，最不容易错）；
///  - `width` / `height`：没给 `baseFilename` 时的显式尺寸；
///  - `region`：`full`（默认）/ `x,y,w,h`（归一化 0~1）/ 预设 `left|right|top|bottom|center|person`；
///  - `shape`：`rect`（默认）/ `ellipse`；
///  - `invert`：反选（**换背景保人物**就是 `invert=true` + 人物那块 region）；
///  - `feather`：边缘羽化像素（默认 0；贴人物进新背景时 8~24 会明显更自然）；
///  - `filename`：想固定的文件名（不给自己按内容算一个，重复调用得到同一个文件）。
pub fn make_mask(state: &AppState, args: &Value) -> Result<Value, ToolFailure> {
    let input_dir = resolve_input_dir(state)?;

    let base_filename = str_arg(args, "baseFilename");
    let mut width = int_arg(args, "width");
    let mut height = int_arg(args, "height");
    if let Some(base) = base_filename.as_deref().filter(|name| !name.trim().is_empty()) {
        let base_path = input_dir.join(file_name_only(base));
        if !base_path.is_file() {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!(
                    "input 目录里没有「{base}」。先用 comfy_use_media（画廊产物）或 \
                     comfy_use_attachment（用户发来的图）把底图投放进去，再照它生成遮罩。"
                ),
            ));
        }
        let size = read_size(&base_path).ok_or_else(|| {
            ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("读不出「{base}」的像素尺寸（可能不是 PNG/JPG）：遮罩尺寸必须与底图一致。"),
            )
        })?;
        width = Some(size.0);
        height = Some(size.1);
    }
    let w = width.ok_or_else(|| {
        ToolFailure::new("INVALID_ARGUMENT", "要么给 baseFilename，要么给 width 与 height")
    })?;
    let h = height.ok_or_else(|| {
        ToolFailure::new("INVALID_ARGUMENT", "要么给 baseFilename，要么给 width 与 height")
    })?;
    if w < 16 || h < 16 || w > MAX_MASK_EDGE || h > MAX_MASK_EDGE {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("尺寸不合法：{w}x{h}（要在 16~{MAX_MASK_EDGE} 之间）"),
        ));
    }

    let region_text = str_arg(args, "region").unwrap_or_else(|| "full".to_string());
    let region = parse_region(&region_text)?;
    let shape = str_arg(args, "shape").unwrap_or_else(|| "rect".to_string()).to_lowercase();
    if shape != "rect" && shape != "ellipse" {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("shape 只支持 rect / ellipse，收到「{shape}」"),
        ));
    }
    let invert = bool_arg(args, "invert").unwrap_or(false);
    let feather = int_arg(args, "feather").unwrap_or(0).clamp(0, MAX_FEATHER);
    // `mode`：`rect`（默认，框）/ `subject`（**照某张图里主体的形状抠** —— 贴人物图必须用这个）
    let mode = str_arg(args, "mode").unwrap_or_else(|| "rect".to_string()).to_lowercase();
    if mode != "rect" && mode != "subject" {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("mode 只支持 rect / subject，收到「{mode}」"),
        ));
    }
    let subject_filename = str_arg(args, "subjectFilename");
    // `matteSource`：合成用的遮罩**从哪来**。
    //   · `color`（默认）：今天的路 —— 本工具按颜色抠出剪影并**写一张遮罩文件**（`mode="subject"`），
    //     或按 `region` 画一张（`mode="rect"`）。
    //   · `model`：遮罩由**工作流里的抠图节点**现算（`easy imageRemBg`，
    //     见 `comfyui\workflows\comfyhub_paste_person_matte_anima.json`）。这一档本工具**不写遮罩、不写 ring**，
    //     只做一件事：把**人物图那一份像素**清理干净（反混 + 两侧去溢色）并把文件名回给调用方。
    //     为什么非做不可：`ImageCompositeMasked` 合的是**那张图的像素**，边界那圈"人 + 背景"的混合色
    //     不去掉，软遮罩按 alpha 贴上去就是绕着人的一圈淡背景色（实测 2.56 / max 16 → 清完 0.00 / max 0）。
    let matte_source = str_arg(args, "matteSource").unwrap_or_else(|| "color".to_string()).to_lowercase();
    if matte_source != "color" && matte_source != "model" {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("matteSource 只支持 color / model，收到「{matte_source}」"),
        ));
    }
    if matte_source == "model" && mode != "subject" {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            "matteSource=\"model\" 要跟 mode=\"subject\" 一起用：AI 抠图抠的就是 subjectFilename 那张人物图",
        ));
    }
    let tolerance = args
        .get("tolerance")
        .and_then(Value::as_f64)
        .unwrap_or(SUBJECT_TOLERANCE_DEFAULT);

    // gColor：**显式告诉工具背景是什么颜色**（"#00b140" / "green" / "magenta"）——
    // 抠图最稳的一档：不用去猜边框、也不受"主体挨到边"影响。配合"阶段一用绿幕/品红背景"，
    // 背景色根本不会出现在主体身上，判据就干净了（见 subject_mask_from_image 的 ③）。
    let explicit_background = str_arg(args, "bgColor").and_then(|raw| parse_color(&raw));
    let mut subject_info: Option<(String, [u8; 3], f64)> = None;
    // 收缩前的剪影：ring 要把"被收掉的那条边"也盖住，否则会留一条没被重绘的接缝
    let mut subject_before_shrink: Option<Vec<u8>> = None;
    let mut pixels = if mode == "subject" {
        let subject = subject_filename
            .as_deref()
            .map(file_name_only)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                ToolFailure::new(
                    "INVALID_ARGUMENT",
                    "mode=\"subject\" 要给 subjectFilename（input 目录里那张**要抠形状**的图，\
                     通常是 comfy_use_media / comfy_use_attachment 刚投进去的人物图）",
                )
            })?;
        let subject_path = input_dir.join(&subject);
        if !subject_path.is_file() {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("input 目录里没有「{subject}」—— 先用 comfy_use_media 把它投进去"),
            ));
        }
        let use_region = region_text != "full";
        // 收边要按**人物图自己的颜色**判（发丝 vs 混合像素），所以先把它读成画布尺寸的一张图
        let image_for_edges = {
            let opened = image::open(&subject_path)
                .map_err(|e| ToolFailure::new("COMFY_INPUT_FAILED", format!("读不出人物图：{e}")))?
                .to_rgb8();
            if opened.width() as usize == w as usize && opened.height() as usize == h as usize {
                opened
            } else {
                image::imageops::resize(&opened, w as u32, h as u32, image::imageops::FilterType::Triangle)
            }
        };
        let (mut mask, background, ratio) =
            subject_mask(&subject_path, w as usize, h as usize, tolerance, &region, use_region, explicit_background)?;
        subject_info = Some((subject, background, ratio));
        // **收一刀**（choke）：把主体与背景交界那圈抗锯齿/溢色（绿边、白边）扔掉 ——
        // 不收的话它们会被当成主体贴进去，再被 ring 一晕就是绕着人的一圈泛光。
        // 默认 2 像素（几乎无感）；`shrink=0` 可关掉。
        // ① **按颜色收边**（默认开）：扔掉边界那几个"人+背景"的混合像素 ——
        //    比几何腐蚀强的地方是**不碰细发丝**（发丝是头发色，离背景色远）。
        // ② 几何 `shrink` 默认 **0**（原来是 2，会把比 5 像素细的发丝整条吃掉）。
        // ⚠ matte 档**不做这两步**：这一档的 `pixels` 只当**定位器**（真正的 alpha 在工作流里），
        //    而"按颜色收边 / 几何收缩"都会把它往里啃 —— 啃完外带就从更里面起算，
        //    反而盖不到软遮罩真正的边界。颜色档照旧（那两步是它的既定语义）。
        let edge_band = int_arg(args, "edgeBand").unwrap_or(DEFAULT_EDGE_BAND).clamp(0, 32) as usize;
        let edge_tolerance = args
            .get("edgeTolerance")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_EDGE_TOLERANCE);
        if edge_band > 0 && matte_source == "color" {
            drop_blended_edge(
                &mut mask,
                &image_for_edges,
                w as usize,
                h as usize,
                background,
                edge_band,
                edge_tolerance,
            );
        }
        let shrink = int_arg(args, "shrink").unwrap_or(DEFAULT_SUBJECT_SHRINK).clamp(0, 32) as usize;
        if shrink > 0 && matte_source == "color" {
            subject_before_shrink = Some(mask.clone());
            mask = erode_mask(&mask, w as usize, h as usize, shrink);
        }
        mask
    } else {
        draw_mask(w as usize, h as usize, &region, &shape)
    };

    // **摆放**（用户 2026-09-26 要的能力）：`scale` 缩放、`x`/`y` 把剪影中心搬到归一化坐标。
    // 关键点：**人物图的像素与遮罩一起动**（只动一个就是"人在这儿、遮罩在那儿"的错位）。
    // 实现方式是烘进文件（写一张 canvas 大小的新人物图 + 新遮罩），
    // 这样**不用改工作流**：还是 `20.image` + `12.image` 两个覆盖。
    let mut placed_subject: Option<(String, [f64; 4])> = None;
    let mut placed_image: Option<image::RgbImage> = None;
    let scale = args.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
    let center_x = args.get("x").and_then(Value::as_f64);
    let center_y = args.get("y").and_then(Value::as_f64);
    if mode == "subject" && (scale != 1.0 || center_x.is_some() || center_y.is_some()) {
        if let Some((x0, y0, x1, y1)) = mask_bbox(&pixels, w as usize, h as usize) {
            let subject_file = subject_info.as_ref().map(|info| info.0.clone()).unwrap_or_default();
            let person_image = image::open(input_dir.join(&subject_file))
                .map_err(|e| ToolFailure::new("COMFY_INPUT_FAILED", format!("读不出人物图：{e}")))?
                .to_rgb8();
            // 人物图要先缩到画布尺寸（与工作流里 `ImageScale` 同一个口径），再整体搬
            let canvas_person = image::imageops::resize(
                &person_image,
                w as u32,
                h as u32,
                image::imageops::FilterType::Triangle,
            );
            let (scaled_person, scaled_mask) = place_patch(
                &canvas_person,
                &pixels,
                w as usize,
                h as usize,
                (x0, y0, x1, y1),
                scale,
                center_x.unwrap_or(((x0 + x1) as f64 / 2.0) / w as f64),
                center_y.unwrap_or(((y0 + y1) as f64 / 2.0) / h as f64),
            );
            let placed_name = format!("placed-{}", subject_file);
            let placed_path = input_dir.join(&placed_name);
            if let Some(parent) = placed_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            scaled_person.save(&placed_path).map_err(|e| {
                ToolFailure::new("COMFY_INPUT_FAILED", format!("写停放好的人物图失败：{e}"))
            })?;
            let new_bbox = mask_bbox(&scaled_mask, w as usize, h as usize);
            if let Some((nx0, ny0, nx1, ny1)) = new_bbox {
                placed_subject = Some((
                    placed_name,
                    [
                        (nx0 + nx1) as f64 / 2.0 / w as f64,
                        (ny0 + ny1) as f64 / 2.0 / h as f64,
                        (nx1 - nx0) as f64 / w as f64,
                        (ny1 - ny0) as f64 / h as f64,
                    ],
                ));
            }
            pixels = scaled_mask;
            placed_image = Some(scaled_person);
            // ⚠ 剪影**位置变了**：ring 必须按搬家后的剪影算，
            //   否则重绘区还留在原地（实测：人被搬到左下角，模型却在画面正中重画了一遍）
            subject_before_shrink = None;
        }
    }
    let _ = placed_image;

    // **按背景环境改新图层的光照与阴影**（用户 2026-09-26 的最后一个要求）：
    // 光向/色温从 `baseFilename` 那张底图上估（`light=\"auto\"`，也可以用 `lightDir=\"left\"` 等显式给），
    // 结果烘进人物图的像素 + 烧一层投影（影子落在遮罩里，于是跟着一起贴上去 —— 不改工作流）。
    let mut light_info: Option<([f64; 2], bool, usize)> = None;
    let mut out_light: Option<([f64; 2], bool, usize)> = None;
    let mut despilled = false;
    let mut unmixed_edge = false;
    let light_arg = str_arg(args, "light").unwrap_or_else(|| "none".to_string()).to_lowercase();
    if mode == "subject" && light_arg != "none" && !light_arg.is_empty() {
        let direction = match light_arg.as_str() {
            "auto" => None,
            "left" => Some([-1.0, 0.0]),
            "right" => Some([1.0, 0.0]),
            "top" | "up" => Some([0.0, -1.0]),
            "bottom" | "down" => Some([0.0, 1.0]),
            "topleft" | "left-top" => Some([-0.7, -0.7]),
            "topright" | "right-top" => Some([0.7, -0.7]),
            other => {
                return Err(ToolFailure::new(
                    "INVALID_ARGUMENT",
                    format!("light 只支持 auto / left / right / top / bottom / topleft / topright / none，收到「{other}」"),
                ))
            }
        };
        let base_path = base_filename
            .as_deref()
            .map(file_name_only)
            .map(|name| input_dir.join(name))
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                ToolFailure::new(
                    "INVALID_ARGUMENT",
                    "light 要用**底图**去估光向与色温，所以必须给 baseFilename（input 目录里那张）",
                )
            })?;
        let mut person_image = placed_image
            .take()
            .or_else(|| {
                subject_info
                    .as_ref()
                    .and_then(|info| image::open(input_dir.join(&info.0)).ok())
                    .map(|image| {
                        image::imageops::resize(
                            &image.to_rgb8(),
                            w as u32,
                            h as u32,
                            image::imageops::FilterType::Triangle,
                        )
                    })
            })
            .ok_or_else(|| ToolFailure::new("COMFY_INPUT_FAILED", "读不出人物图（打光要用它的像素）"))?;
        let base_image = image::open(&base_path)
            .map_err(|e| ToolFailure::new("INVALID_ARGUMENT", format!("读不出底图：{e}")))?
            .to_rgb8();
        let base_image = image::imageops::resize(&base_image, w as u32, h as u32, image::imageops::FilterType::Triangle);
        let light = args.get("lightStrength").and_then(Value::as_f64).unwrap_or(0.35);
        let shadow = args.get("shadowStrength").and_then(Value::as_f64).unwrap_or(0.45);
        let offset = int_arg(args, "shadowOffset")
            .unwrap_or_else(|| (w.min(h) as f64 * 0.04).round() as i64)
            .clamp(0, 400) as usize;
        let (dir, estimated) =
            relight_and_shadow(&mut person_image, &mut pixels, &base_image, w as usize, h as usize, direction, light, shadow, offset);
        let placed_name = format!("lit-{}", subject_info.as_ref().map(|i| i.0.clone()).unwrap_or_default());
        person_image
            .save(input_dir.join(&placed_name))
            .map_err(|e| ToolFailure::new("COMFY_INPUT_FAILED", format!("写打好光的人物图失败：{e}")))?;
        if let Some((_, bbox)) = placed_subject.as_mut() {
            if let Some((x0, y0, x1, y1)) = mask_bbox(&pixels, w as usize, h as usize) {
                *bbox = [
                    (x0 + x1) as f64 / 2.0 / w as f64,
                    (y0 + y1) as f64 / 2.0 / h as f64,
                    (x1 - x0) as f64 / w as f64,
                    (y1 - y0) as f64 / h as f64,
                ];
            }
        } else {
            placed_subject = Some((placed_name.clone(), [0.0, 0.0, 0.0, 0.0]));
        }
        // 打光后的人物图**换了文件名**（`lit-…`），使用方必须拿它覆盖 `20.image`
        placed_subject = Some((
            placed_name,
            placed_subject.map(|(_, bbox)| bbox).unwrap_or([0.0, 0.0, 0.0, 0.0]),
        ));
        light_info = Some((dir, estimated, offset));
        out_light = Some((dir, estimated, offset));
    }
    // **去溢色**（despill）：choke 只扔掉了最外 2 像素，羽化那几像素仍是"人+背景"的混合色，
    // 半透明贴上去就是外轮廓上残留的背景色。这里在边界带里把背景主色通道压回去。
    // ⚠ 写回**同一个文件名**（人物图那个），所以工作流里 `20.image` 的覆盖一个字都不用改。
    if mode == "subject" {
        if let Some((subject_file, background, _)) = subject_info.clone() {
            let target_name = placed_subject
                .as_ref()
                .map(|(name, _)| name.clone())
                .unwrap_or(subject_file);
            let target_path = input_dir.join(&target_name);
            if let Ok(opened) = image::open(&target_path) {
                let mut canvas = opened.to_rgb8();
                if canvas.width() as usize != w as usize || canvas.height() as usize != h as usize {
                    canvas = image::imageops::resize(
                        &canvas,
                        w as u32,
                        h as u32,
                        image::imageops::FilterType::Triangle,
                    );
                }
                // ① 先**反混**（把"人+背景"的混合像素还原成人物本体色 —— 这才是去掉
                //    那圈灰绿边的正解）；② 再 despill 兜底（反混没覆盖到的边界像素压通道）。
                //
                // ⚠ `unmix_edge` 会**顺手啃掩码**（`a < 0.45` 的从掩码里去掉）。那是颜色档的
                //    既定语义（下一步写出去的就是被啃过的那张）；但 matte 档的遮罩只当**定位器**
                //    （真正的 alpha 在工作流里），啃掉就定位不到软边了 —— 所以那一档传**副本**进去，
                //    `pixels` 原样留着给 despill 定边界。
                let band = int_arg(args, "edgeBand").unwrap_or(DEFAULT_EDGE_BAND).clamp(1, 16) as usize;
                let mut unmix_mask = pixels.clone();
                let unmixed = unmix_edge(&mut canvas, &mut unmix_mask, w as usize, h as usize, background, band);
                if unmixed { unmixed_edge = true; }
                if matte_source == "color" {
                    pixels = unmix_mask;
                }
                // ② 去溢色：**两侧**都要盖 —— 真正的溢色像素在掩码**外面**（交界那圈抗锯齿像素
                //    颜色更靠近背景，被判成了背景），只往内压等于一个都没盖到（见 [`despill_band`]）。
                //    matte 档的外带开大一点（软遮罩的边界比颜色剪影宽）。
                let outer = if matte_source == "model" {
                    int_arg(args, "outerBand").unwrap_or(DEFAULT_OUTER_BAND).clamp(0, 32) as usize
                } else {
                    6
                };
                let despilled_now =
                    despill_band(&mut canvas, &pixels, w as usize, h as usize, background, 6, outer);
                if despilled_now { despilled = true; }
                // ⚠ 只要**动过像素**就要写回：反混在白 / 浅色背景下不触发 despill（没有溢色通道
                //    可压），而**那一档恰恰只有反混能修**（把"人 + 白底"的混合像素还原成本体色）。
                //    原来只在 despill 为真时才 save → 白底那条路的反混结果被算出来又丢掉（静默）。
                if unmixed || despilled_now {
                    canvas.save(&target_path).map_err(|e| {
                        ToolFailure::new("COMFY_INPUT_FAILED", format!("写回去溢色后的人物图失败：{e}"))
                    })?;
                }
            }
        }
    }

    if invert {
        for value in pixels.iter_mut() {
            *value = 255 - *value;
        }
    }
    if feather > 0 {
        pixels = box_blur(&pixels, w as usize, h as usize, feather);
    }

    let filename = str_arg(args, "filename")
        .filter(|name| !name.trim().is_empty())
        .map(|name| file_name_only(&name))
        .unwrap_or_else(|| {
            format!("comfyhub_mask_{}.png", content_tag(w, h, &region, &shape, invert, feather))
        });
    // matte 档**不写合成遮罩**：真正的 alpha 由工作流里的抠图节点现算（写一张没人用的文件
    // 只会在 input 目录里攒垃圾，还会让调用方以为该去覆盖 LoadImageMask）。
    let mask_written = matte_source == "color";
    if mask_written {
        write_mask_png(&pixels, w as u32, h as u32, &input_dir.join(&filename))?;
    }

    // **只在人物外面一圈重绘**（`ring`，单位像素）：配合带第二个 LoadImageMask 的工作流
    // `comfyhub_paste_person_anima.json`，人物像素就能"原样贴上去、一点不被重画"
    // —— 这是"嵌进去会改变画风"那条的正解（详见 `ring_mask` 的注释）。
    // matte 档不要它：那份工作流用 `GrowMask` + `MaskComposite(subtract)` **从同一份 alpha 现算** ring。
    let ring = int_arg(args, "ring").unwrap_or(0).clamp(0, MAX_FEATHER) as usize;
    let mut ring_filename: Option<String> = None;
    if mode == "subject" && ring > 0 && mask_written {
        let mut ring_pixels = match subject_before_shrink.as_ref() {
            // 用**收缩前**的剪影算外面那一圈，再把"被收掉的那条边"并回来 ——
            // 于是重绘区正好盖住 人物外 N 像素 + 交界那一圈溢色
            Some(orig) => ring_mask(orig, w as usize, h as usize, ring),
            None => ring_mask(&pixels, w as usize, h as usize, ring),
        };
        if let Some(orig) = subject_before_shrink.as_ref() {
            for index in 0..ring_pixels.len() {
                if orig[index] > 127 && pixels[index] <= 127 {
                    ring_pixels[index] = 255;
                }
            }
        }
        let ring_name = format!("ring-{filename}");
        write_mask_png(&ring_pixels, w as u32, h as u32, &input_dir.join(&ring_name))?;
        ring_filename = Some(ring_name);
    }

    let region_joined = region.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
    // matte 档要告诉调用方"拿哪张人物图去覆盖 20.image"（它已经被清理过了）
    let person_filename = placed_subject
        .as_ref()
        .map(|(name, _)| name.clone())
        .or_else(|| subject_info.as_ref().map(|(name, _, _)| name.clone()));
    let mut out = json!({
        "inputDir": input_dir.to_string_lossy(),
        "width": w,
        "height": h,
        "mode": mode,
        "matteSource": matte_source,
        "maskGenerated": mask_written,
        "region": region_text,
        "regionNormalized": format!("[{region_joined}]"),
        "shape": shape,
        "invert": invert,
        "feather": feather,
        "whiteMeans": "要重新生成的地方（重绘区）",
        // 四个预设都是"粗框"，只有显式给坐标才不是估的（与 Kotlin 同一条判据）
        "approximate": region_text != "full" && !region_text.contains(','),
    });
    if mask_written {
        out["filename"] = json!(filename);
        out["hint"] = json!(format!(
            "① 覆盖工作流里 LoadImageMask 节点的 image 输入：{{\"<节点id>.image\":\"{filename}\"}}；\
             ② 若工作流有 ImageScale，把 width/height 也覆盖成 {w}x{h}（必须与底图一致）；\
             ③ 遮罩不准就报一组新的 region（x,y,w,h 归一化）重做 —— 不要让用户去 ComfyUI 里手画。"
        ));
    } else {
        out["personFilename"] = json!(person_filename.clone().unwrap_or_default());
        out["hint"] = json!(format!(
            "遮罩由**工作流里的抠图节点**现算，所以**不要**覆盖任何 LoadImageMask（也没有 12 / 26 可覆盖）。\
             ① comfy_load_workflow(path=\"<项目根>\\comfyui\\workflows\\comfyhub_paste_person_matte_anima.json\")；\
             ② 覆盖 10.image（底图）、20.image=\"{person}\"（**已被清理过**的人物图）、\
             11.width={w}、11.height={h}、21.width={w}、21.height={h}、15.text（只写**人物外面那一圈**该出现什么，写背景）、17.seed；\
             ③ 17.denoise 用 0.3~0.45（只融接缝；0 = 纯合成、一个像素都不改）。\
             ⚠ 那张工作流要求 ComfyUI 里有抠图模型（models\\rembg\\BEN2_Base.pth 或 RMBG-1.4.pth）——\
             没有就如实告诉用户下哪个，别让它静默下载。",
            person = person_filename.unwrap_or_default(),
        ));
    }
    if let Some((subject, background, ratio)) = subject_info {
        out["subjectFilename"] = json!(subject);
        out["background"] = json!(format!("#{:02x}{:02x}{:02x}", background[0], background[1], background[2]));
        out["subjectRatio"] = json!((ratio * 1000.0).round() / 1000.0);
        if mode == "subject" {
            out["shrink"] = json!(int_arg(args, "shrink").unwrap_or(DEFAULT_SUBJECT_SHRINK));
        }
        if despilled {
            out["despilled"] = json!(true);
        }
        if unmixed_edge {
            out["unmixedEdge"] = json!(true);
        }
        if matte_source == "model" {
            // 如实报口径：这一档的遮罩不是我们给的，我们只清干净了人物图那一份像素
            out["outerBand"] = json!(int_arg(args, "outerBand").unwrap_or(DEFAULT_OUTER_BAND));
            out["matteNote"] = json!(
                "matteSource=\"model\"：合成用的**软遮罩**由工作流里的抠图节点（`easy imageRemBg`）现算，\
                 本工具只把 `personFilename` 那份像素做了反混 + **两侧**去溢色（掩码外那圈抗锯齿像素\
                 才是真正的溢色像素 —— 只往内压会留一圈淡背景色）。\
                 所以**不要**覆盖任何 LoadImageMask，用 `comfyhub_paste_person_matte_anima.json` 那份工作流。"
            );
        }
        if let Some((dir, estimated, offset)) = out_light {
            out["lightDir"] = json!(format!("[{:.2},{:.2}]", dir[0], dir[1]));
            // 光向是**从底图估的**还是调用方给的 —— 如实说（估的就别吹成"打光模型"）
            out["lightEstimated"] = json!(estimated);
            out["shadowOffset"] = json!(offset);
            out["lightNote"] = json!(
                "光照是按**底图**估的方向梯度 + 色偏烘进人物图的（近似，不是重打光模型）；\
                 影子沿光向**反方向**烧在了 `placedSubjectFilename` 里并落进了遮罩 —— \
                 提交时**必须**用新的 `placedSubjectFilename` 覆盖 `20.image`（像素已经变了）。"
            );
        }
        let saturation = background.iter().max().copied().unwrap_or(0) as i32
            - background.iter().min().copied().unwrap_or(0) as i32;
        let chroma = saturation >= CHROMA_MIN_SATURATION;
        out["chromaKey"] = json!(chroma);
        // 显式给了 bgColor 但实际用的是另一个色 → 如实说明（模型那块绿/品红与写死的值不一样）
        if let Some(requested) = explicit_background {
            if requested != background {
                out["bgColorIgnored"] = json!(format!(
                    "#{:02x}{:02x}{:02x}",
                    requested[0], requested[1], requested[2]
                ));
            }
        }
        if !chroma {
            // ⚠ 低饱和背景（白 / 浅灰）与"人物身上的白"同色 —— 填洞只能靠连通性猜，
            //   腿间/发丝之间的背景空白可能被当成主体（用户 2026-09-26 现场看到的那圈白）。
            out["chromaAdvice"] = json!(
                "⚠ 背景是白/浅色（低饱和）：它与人物身上的浅色（白裙子/白衬衫/高光）同色，\
                 抠图存在二义 —— 腿间、发丝之间的背景空白可能被当成主体贴进画面。\
                 要干净就把**阶段一的背景换成绿幕或品红**（提示词写 `green background` / \
                 `chroma green background`，并把 `bgColor=\"#00b140\"` 传进来）。"
            );
        }
        let placed_hint = match placed_subject.as_ref() {
            Some((placed_name, bbox)) => format!(
                "⚠ 这次做了**摆放**（scale={scale}，中心 x={:.2} y={:.2}）：\
                 `20.image` **必须**改成 `{placed_name}`（人物像素已经跟着一起搬过去了，\
                 还用原图就是「人在这儿、遮罩在那儿」的错位）；剪影现在占画面 x {:.2}~{:.2} / y {:.2}~{:.2}。",
                bbox[0], bbox[1],
                bbox[0] - bbox[2] / 2.0,
                bbox[0] + bbox[2] / 2.0,
                bbox[1] - bbox[3] / 2.0,
                bbox[1] + bbox[3] / 2.0
            ),
            None => String::new(),
        };
        if mask_written {
            out["hint"] = json!(format!(
                "① 覆盖工作流里 LoadImageMask 节点的 image 输入：{{\"<节点id>.image\":\"{filename}\"}}；\
                 ② ImageScale 的 width/height 覆盖成 {w}x{h}；\
                 ③ 这张遮罩是照「{subject}」的**主体形状**抠的（背景色 {}，占画面 {:.1}%）。\
                 遮罩几乎盖满整张图（>90%）时多半是背景不纯（也可能是主体本来就占满画面）—— 那时改用矩形遮罩 + 更高的 denoise，或换一张纯色背景重做阶段一，\
                 或者改用 mode=\"rect\" 的矩形遮罩 + 更高的 denoise。{placed_hint}",
                format!("#{:02x}{:02x}{:02x}", background[0], background[1], background[2]),
                ratio * 100.0
            ));
        }
        if let Some((placed_name, _)) = placed_subject.as_ref() {
            out["placedSubjectFilename"] = json!(placed_name);
        }
        if let Some(ring_name) = ring_filename.as_deref() {
            out["ringFilename"] = json!(ring_name);
            out["ring"] = json!(ring);
            out["hint"] = json!(format!(
                "① 合成用遮罩覆盖 `12.image`（或工作流里第一个 LoadImageMask）← `{filename}`；\
                 ② **重绘区**覆盖第二个 LoadImageMask（`comfyhub_paste_person_anima.json` 里是 `26.image`）\
                 ← `{ring_name}` —— 这一份只圈人物**外面 {ring} 像素**，于是人物自己的像素与画风一个都不动；\
                 ③ ImageScale 的 width/height 覆盖成 {w}x{h}；\
                 ④ 要用这条「只融接缝」的路，工作流必须是有两个 LoadImageMask 的那一份；\
                 用的是单遮罩工作流时**别传 ring**（否则第二个遮罩没人接，人物会被重画）。"
            ));
        }
    }
    Ok(out)
}

/// `region` 解析成归一化的 `[x, y, w, h]`。
///
/// 四个预设都是"粗框"，够表达"左边那个人 / 中间那个人"：
/// `center` 与 `person` 是竖长方框（人物通常站着），`left` / `right` / `top` / `bottom` 是半张。
pub fn parse_region(raw: &str) -> Result<[f64; 4], ToolFailure> {
    let text = raw.trim().to_lowercase();
    match text.as_str() {
        "full" | "all" | "整张" | "" => return Ok([0.0, 0.0, 1.0, 1.0]),
        "left" => return Ok([0.0, 0.0, 0.5, 1.0]),
        "right" => return Ok([0.5, 0.0, 0.5, 1.0]),
        "top" => return Ok([0.0, 0.0, 1.0, 0.5]),
        "bottom" => return Ok([0.0, 0.5, 1.0, 0.5]),
        "center" | "centre" => return Ok([0.25, 0.1, 0.5, 0.8]),
        "person" => return Ok([0.22, 0.05, 0.56, 0.92]),
        _ => {}
    }
    let parts: Vec<&str> = text.split(',').map(str::trim).filter(|part| !part.is_empty()).collect();
    if parts.len() != 4 {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!(
                "region 只支持 full / left / right / top / bottom / center / person，\
                 或者四个归一化数字 `x,y,w,h`（例如 \"0.3,0.05,0.4,0.9\"）。收到「{raw}」"
            ),
        ));
    }
    let mut nums = [0.0f64; 4];
    for (index, part) in parts.iter().enumerate() {
        let value = part.parse::<f64>().map_err(|_| {
            ToolFailure::new("INVALID_ARGUMENT", format!("region 里的「{part}」不是数字"))
        })?;
        if !(0.0..=1.0).contains(&value) {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("region 的四个数都是**归一化**的（0~1），收到「{raw}」"),
            ));
        }
        nums[index] = value;
    }
    let x = nums[0];
    let y = nums[1];
    let w = nums[2].min(1.0 - x);
    let h = nums[3].min(1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("region 的宽高必须大于 0（收到「{raw}」）"),
        ));
    }
    Ok([x, y, w, h])
}

/// 从一张图里抠出**主体形状**的遮罩（`mode="subject"`）。
///
/// # 为什么要有它（用户 2026-09-26 提的）
///
/// "贴人物图"那个用法里，`ImageCompositeMasked` 是**按遮罩把 source 的像素搬过去**的：
/// 遮罩给矩形，就会把人物图**那块矩形连背景一起**贴进来 —— 白底人物图贴到夜景上
/// 就是一圈亮框（实测：denoise 0.5 是白框、0.85 是换人、cfg 1.0/4.0 一个样）。
/// 正确的遮罩是**人物的剪影**，于是尺子要按 person 图来量，不是按框来估。
///
/// # 算法（**刻意不依赖任何模型**）
///
/// ① 取**画面四周一圈**像素的中位数当背景色（中位数对"人物挨到边"比均值稳）；
/// ② 每个像素到背景色的距离超过 `tolerance`（归一化 0~1）就是主体；
/// ③ 只保留**最大的一块**连通区（生成图的背景常带噪点 / 水印纹，不去掉就是一片麻子）；
/// ④ 尺寸与画布不一致时按**块平均**缩放（与工作流里 `ImageScale` 的口径一致，
///    这样剪影和它缩放后的像素对得上）；
/// ⑤ 给了 `region` 就与矩形取交集（把人物约束在某块区域里）。
///
/// # 局限（必须如实说，别假装是语义分割）
///
/// 它只认"**背景是纯色 / 浅色渐变**"这一档（棚拍白底、绿幕、纯色背景 ——
/// 也就是阶段一 t2i 常见的产物）。复杂背景（照片里的街道、树林）它抠不出来，
/// 那时**要么用矩形遮罩 + 高 denoise**，要么用户装一个分割 / 抠图模型
/// （`rembg` / SAM 之类）。判定办法就在返回值里：`subjectRatio`
/// 太大（> 0.6）基本就是"背景没抠干净"。
pub fn subject_mask(
    source: &Path,
    out_w: usize,
    out_h: usize,
    tolerance: f64,
    region: &[f64; 4],
    use_region: bool,
    explicit_background: Option<[u8; 3]>,
) -> Result<(Vec<u8>, [u8; 3], f64), ToolFailure> {
    let image = image::open(source)
        .map_err(|e| {
            ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("读不出「{}」的像素（抠主体要用它的形状）：{e}", source.display()),
            )
        })?
        .to_rgb8();
    subject_mask_from_image(&image, out_w, out_h, tolerance, region, use_region, explicit_background)
}

/// [`subject_mask`] 的**不落盘那一半**（用例直接喂一张内存图，不必造临时文件）。
pub fn subject_mask_from_image(
    image: &image::RgbImage,
    out_w: usize,
    out_h: usize,
    tolerance: f64,
    region: &[f64; 4],
    use_region: bool,
    explicit_background: Option<[u8; 3]>,
) -> Result<(Vec<u8>, [u8; 3], f64), ToolFailure> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w < 16 || h < 16 {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("这张图太小了（{w}x{h}）：抠主体至少要 16x16"),
        ));
    }

    // ① 背景色：显式给就用给的（**推荐**，见下面 chroma 那段），否则取四周一圈的**中位数**
    //    （中位数对"人物挨到边"比均值稳）
    let cutoff = (tolerance.clamp(0.01, 1.0) * 255.0).round() as i32;
    let estimated = {
        let border = (w.min(h) / 50).max(2);
        let mut samples: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for y in 0..h {
            for x in 0..w {
                if x >= border && x + border < w && y >= border && y + border < h {
                    continue;
                }
                let pixel = image.get_pixel(x as u32, y as u32);
                for channel in 0..3 {
                    samples[channel].push(pixel[channel]);
                }
            }
        }
        let mut color = [0u8; 3];
        for channel in 0..3 {
            samples[channel].sort_unstable();
            color[channel] = samples[channel][samples[channel].len() / 2];
        }
        color
    };
    // ⚠ **显式给的背景色不能盲信**：实测把 `bgColor=green`（写死的 `#00b140`）传给一张
    //   模型自己生成的"绿底"图，出来的前景占比是 **100%**（模型那块绿不是这个绿）——
    //   于是"整张图都成了主体"。所以这里先按显式色试一次，**占比离谱（>90%）就回落**成
    //   边框估计的那份（并把实际用的颜色如实回给调用方）。
    let key_with = |color: [u8; 3]| -> (Vec<u8>, f64) {
        let mut mask = vec![0u8; w * h];
        let mut foreground = 0usize;
        for y in 0..h {
            for x in 0..w {
                let pixel = image.get_pixel(x as u32, y as u32);
                let distance = (0..3)
                    .map(|c| (pixel[c] as i32 - color[c] as i32).abs())
                    .max()
                    .unwrap_or(0);
                if distance > cutoff {
                    mask[y * w + x] = 255;
                    foreground += 1;
                }
            }
        }
        let ratio = foreground as f64 / (w * h).max(1) as f64;
        (mask, ratio)
    };
    let mut used_background = explicit_background.unwrap_or(estimated);
    let (mut raw, mut foreground_ratio) = key_with(used_background);
    if explicit_background.is_some() && foreground_ratio > 0.9 {
        // 显式色对不上（占比离谱）→ 回落成边框估计的那份
        used_background = estimated;
        let (mask, ratio) = key_with(used_background);
        raw = mask;
        foreground_ratio = ratio;
    }
    let background = used_background;
    let _ = foreground_ratio;



    // ③ 内部空洞：
    //
    // 这里有一条**必须记住的取舍**（用户 2026-09-26 指出的）：
    // **背景色如果与主体自己的颜色一样**（白底 + 白裙子），"这块白是裙子还是背景"在像素上
    // **根本无法区分** —— 填洞只能靠连通性猜，而"两腿之间 / 发丝之间"的背景空白与"裙子内部"
    // 在几何上一模一样，猜错就是**把背景当主体贴进画面**（实测：贴出来一圈白晕）。
    //
    // 所以：**背景用与主体不同的颜色**（绿幕 / 品红）才是正解 —— 那时背景色根本不会出现在
    // 主体身上，判据干净，**不需要填洞**。这里按背景色的**饱和度**自动分档：
    //   · 高饱和（chroma，绿幕/品红那种）→ **不填洞**（背景色不会出现在主体上）；
    //   · 低饱和（白 / 浅灰那种）→ 仍然填，但**只补针眼大的窟窿**（抗锯齿边上的小洞），
    //     大块的一律当背景留着（宁可留背景，也不要把背景当人贴上去）。
    let saturation = background.iter().max().copied().unwrap_or(0) as i32
        - background.iter().min().copied().unwrap_or(0) as i32;
    let chroma = saturation >= CHROMA_MIN_SATURATION;
    if !chroma {
        fill_enclosed(&mut raw, w, h);
    }

    // ④ 只留最大的一块连通区（生成图的背景常带噪点 / 水印纹，不去掉就是一片麻子）
    keep_largest_blob(&mut raw, w, h);

    // ⑤ 缩放到画布尺寸（块平均 —— 与工作流里 ImageScale 的口径一致）
    let scaled = if (w, h) == (out_w, out_h) {
        raw
    } else {
        resize_mask(&raw, w, h, out_w, out_h)
    };

    // ⑥ 与 region 取交集
    let mut mask = scaled;
    if use_region {
        let x = (region[0] * out_w as f64).round().clamp(0.0, out_w as f64) as usize;
        let y = (region[1] * out_h as f64).round().clamp(0.0, out_h as f64) as usize;
        let rw = (region[2] * out_w as f64).round().clamp(0.0, (out_w - x) as f64) as usize;
        let rh = (region[3] * out_h as f64).round().clamp(0.0, (out_h - y) as f64) as usize;
        for py in 0..out_h {
            for px in 0..out_w {
                if px < x || px >= x + rw || py < y || py >= y + rh {
                    mask[py * out_w + px] = 0;
                }
            }
        }
    }

    let white = mask.iter().filter(|v| **v > 0).count();
    let ratio = white as f64 / (out_w * out_h).max(1) as f64;
    Ok((mask, background, ratio))
}

/// 把**被主体包住的背景色窟窿**填成主体（4 邻接连通性，从画面边框出发）。
///
/// 为什么必须有它（2026-09-26 实测）：白底人物图里，**人物自己也是白的**
/// （白裙子 / 白衬衫 / 高光），那些像素与背景同色 → 被判成背景 → 贴出来的人
/// 像"半透明"（裙子那块透出底下的街景）。判据只能是连通性：
/// 从边框沿背景像素走得到的才是真背景，被前景包住的窟窿属于主体。
fn fill_enclosed(mask: &mut [u8], w: usize, h: usize) {
    let mut reachable = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let seed = |index: usize, reachable: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if mask[index] == 0 && !reachable[index] {
            reachable[index] = true;
            stack.push(index);
        }
    };
    for x in 0..w {
        seed(x, &mut reachable, &mut stack);
        seed((h - 1) * w + x, &mut reachable, &mut stack);
    }
    for y in 0..h {
        seed(y * w, &mut reachable, &mut stack);
        seed(y * w + w - 1, &mut reachable, &mut stack);
    }
    while let Some(index) = stack.pop() {
        let (x, y) = (index % w, index / w);
        let push = |nx: usize, ny: usize, reachable: &mut Vec<bool>, stack: &mut Vec<usize>| {
            let next = ny * w + nx;
            if mask[next] == 0 && !reachable[next] {
                reachable[next] = true;
                stack.push(next);
            }
        };
        if x > 0 {
            push(x - 1, y, &mut reachable, &mut stack);
        }
        if x + 1 < w {
            push(x + 1, y, &mut reachable, &mut stack);
        }
        if y > 0 {
            push(x, y - 1, &mut reachable, &mut stack);
        }
        if y + 1 < h {
            push(x, y + 1, &mut reachable, &mut stack);
        }
    }
    for index in 0..w * h {
        if mask[index] == 0 && !reachable[index] {
            mask[index] = 255;
        }
    }
}

/// 方形膨胀（结构元 `radius`）。
fn dilate_box(mask: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 {
        return mask.to_vec();
    }
    let r = radius as i64;
    let mut anywhere = vec![false; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if mask[y as usize * w + x as usize] <= 127 {
                continue;
            }
            for dy in -r..=r {
                let ny = y + dy;
                if ny < 0 || ny >= h as i64 {
                    continue;
                }
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w as i64 {
                        continue;
                    }
                    anywhere[ny as usize * w + nx as usize] = true;
                }
            }
        }
    }
    anywhere.into_iter().map(|v| if v { 255 } else { 0 }).collect()
}

/// 只保留最大的一块 4 邻接连通区（其余清零）。
///
/// ⚠ **不能直接对原掩码取最大连通区**（2026-09-26 用户指出"细的头发也被你去掉了"）：
/// 发梢与主体之间往往只隔着几个半透明（抗锯齿）像素，于是发丝是**独立的连通区**，
/// 直接取最大就把它们整片当噪点删了。正确做法是先**膨胀 3 像素**把这种"细脖子"连上，
/// 取最大连通区之后**再与原掩码取交集** —— 发丝保住，真正的噪点（孤立的远处碎块）照旧删掉。
fn keep_largest_blob(mask: &mut [u8], w: usize, h: usize) {
    let bridged = dilate_box(mask, w, h, 3);
    let mut keep = bridged;
    keep_largest_blob_raw(&mut keep, w, h);
    for index in 0..w * h {
        if mask[index] > 127 && keep[index] <= 127 {
            mask[index] = 0;
        }
    }
}

fn keep_largest_blob_raw(mask: &mut [u8], w: usize, h: usize) {
    let mut seen = vec![false; w * h];
    let mut best: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || mask[start] == 0 {
            continue;
        }
        let mut blob: Vec<usize> = Vec::new();
        stack.push(start);
        seen[start] = true;
        while let Some(index) = stack.pop() {
            blob.push(index);
            let (x, y) = (index % w, index / w);
            let push = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                let next = ny * w + nx;
                if !seen[next] && mask[next] > 0 {
                    seen[next] = true;
                    stack.push(next);
                }
            };
            if x > 0 {
                push(x - 1, y, &mut stack, &mut seen);
            }
            if x + 1 < w {
                push(x + 1, y, &mut stack, &mut seen);
            }
            if y > 0 {
                push(x, y - 1, &mut stack, &mut seen);
            }
            if y + 1 < h {
                push(x, y + 1, &mut stack, &mut seen);
            }
        }
        if blob.len() > best.len() {
            best = blob;
        }
    }
    for index in 0..w * h {
        if mask[index] > 0 {
            mask[index] = 0;
        }
    }
    for index in best {
        mask[index] = 255;
    }
}

/// 遮罩缩放（块平均 → 二值化）：与 `ImageScale` 的"面积法"同一个口径，
/// 缩完仍与缩放后的像素对得上。
fn resize_mask(src: &[u8], w: usize, h: usize, out_w: usize, out_h: usize) -> Vec<u8> {
    let mut out = vec![0u8; out_w * out_h];
    for oy in 0..out_h {
        let y0 = oy * h / out_h;
        let y1 = (((oy + 1) * h + out_h - 1) / out_h).min(h).max(y0 + 1);
        for ox in 0..out_w {
            let x0 = ox * w / out_w;
            let x1 = (((ox + 1) * w + out_w - 1) / out_w).min(w).max(x0 + 1);
            let mut sum = 0u32;
            let mut count = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += src[y * w + x] as u32;
                    count += 1;
                }
            }
            out[oy * out_w + ox] = if count > 0 && sum * 2 >= count * 255 { 255 } else { 0 };
        }
    }
    out
}

/// 把 `image` 与 `mask` 的**同一块区域**（`bbox`）按 `scale` 缩放，再把它俩的中心搬到
/// 归一化坐标 `(cx, cy)` 处 —— **图的像素与遮罩必须一起动**，不然贴上去的就是"人在这儿、
/// 遮罩在那儿"，而那种错位在界面上看起来只是"贴歪了"（很难查）。
///
/// 返回 `(canvas 大小的新图, canvas 大小的新遮罩)`；`bbox` 之外的像素一律给黑 / 0
/// （合成只看遮罩白的地方，图其余部分是死重，但一并搬到同一位置才对齐）。
pub fn place_patch(
    image: &image::RgbImage,
    mask: &[u8],
    w: usize,
    h: usize,
    bbox: (usize, usize, usize, usize),
    scale: f64,
    cx: f64,
    cy: f64,
) -> (image::RgbImage, Vec<u8>) {
    let (x0, y0, x1, y1) = bbox;
    let (bw, bh) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    let scale = scale.clamp(0.05, 4.0);
    let tw = ((bw as f64 * scale).round() as usize).max(1);
    let th = ((bh as f64 * scale).round() as usize).max(1);

    // 抠出这块区域（图与遮罩）
    let mut patch_img = image::RgbImage::new(bw as u32, bh as u32);
    let mut patch_mask = vec![0u8; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            patch_img.put_pixel(x as u32, y as u32, *image.get_pixel((x0 + x) as u32, (y0 + y) as u32));
            patch_mask[y * bw + x] = mask[(y0 + y) * w + x0 + x];
        }
    }
    // 缩放（图用三角滤波、遮罩用最近邻再二值化 —— 遮罩只该是 0/255）
    let scaled_img = image::imageops::resize(&patch_img, tw as u32, th as u32, image::imageops::FilterType::Triangle);
    let mut scaled_mask = vec![0u8; tw * th];
    for y in 0..th {
        let sy = (y * bh / th).min(bh - 1);
        for x in 0..tw {
            let sx = (x * bw / tw).min(bw - 1);
            scaled_mask[y * tw + x] = if patch_mask[sy * bw + sx] > 127 { 255 } else { 0 };
        }
    }

    // 目标左上角（中心对到 cx/cy；越界就裁，允许贴到画布边外一半）
    let left = (cx * w as f64).round() as i64 - tw as i64 / 2;
    let top = (cy * h as f64).round() as i64 - th as i64 / 2;

    let mut out_img = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
    let mut out_mask = vec![0u8; w * h];
    for y in 0..th {
        let dy = top + y as i64;
        if dy < 0 || dy >= h as i64 {
            continue;
        }
        for x in 0..tw {
            let dx = left + x as i64;
            if dx < 0 || dx >= w as i64 {
                continue;
            }
            out_img.put_pixel(dx as u32, dy as u32, *scaled_img.get_pixel(x as u32, y as u32));
            out_mask[dy as usize * w + dx as usize] = scaled_mask[y * tw + x];
        }
    }
    (out_img, out_mask)
}

/// **人物外面一圈**的遮罩（`dilate(r) 减去 原剪影`）—— 专门给 `SetLatentNoiseMask` 用。
///
/// 为什么需要它（用户 2026-09-26 问的"嵌进去的时候会改变画风怎么解决"）：
/// 现在的自带工作流**一个遮罩两用**（既给 `ImageCompositeMasked` 贴像素、又给
/// `SetLatentNoiseMask` 决定重绘区），于是"贴上去的人"必然被模型重画一遍 ——
/// denoise 0.5 时她的画风就被模型的画风换掉了。
/// 正确的合成做法是：**人物像素原样贴上去、只把她外面那一圈重画**（融接缝 / 补光影）。
/// 这份 ring 遮罩就是那一圈；配合 `comfyhub_paste_person_anima.json`（它有两个
/// `LoadImageMask`：`12` 给合成、`26` 给重绘）用。
///
/// 半径 `r` 建议 8~24：太小接缝明显，太大等于在人物周围重画一片背景（反而更假）。
pub fn ring_mask(mask: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 {
        return mask.to_vec();
    }
    let r = radius as i64;
    let dilate = |x: i64, y: i64| -> bool {
        // 方形结构元（够用；圆形结构元在"融接缝"这件事上没有区别）
        for dy in -r..=r {
            let ny = y + dy;
            if ny < 0 || ny >= h as i64 {
                continue;
            }
            for dx in -r..=r {
                let nx = x + dx;
                if nx < 0 || nx >= w as i64 {
                    continue;
                }
                if mask[ny as usize * w + nx as usize] > 127 {
                    return true;
                }
            }
        }
        false
    };
    let mut out = vec![0u8; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            // 只在**原剪影之外**、且在膨胀范围内：人物自己的像素一个都不动
            if mask[y as usize * w + x as usize] <= 127 && dilate(x, y) {
                out[y as usize * w + x as usize] = 255;
            }
        }
    }
    out
}

/// 解析一个颜色（"#00b140" / "#0b0" / "green" / "magenta" / "white" / "black"）。
///
/// 只认这几种确定写法 —— **不猜**：认不出来就返回 None（调用方回落成"从边框估"）。
pub fn parse_color(raw: &str) -> Option<[u8; 3]> {
    let text = raw.trim().to_lowercase();
    match text.as_str() {
        "green" | "chroma" | "greenscreen" | "绿" | "绿幕" => return Some([0, 177, 64]),
        "magenta" | "pink" | "品红" | "洋红" => return Some([255, 0, 255]),
        "white" | "白" | "白色" => return Some([255, 255, 255]),
        "black" | "黑" | "黑色" => return Some([0, 0, 0]),
        _ => {}
    }
    let hex: Vec<char> = text.strip_prefix('#').unwrap_or(&text).chars().collect();
    // ⚠ **按字符**判长度、**按字符**取位：按字节切片会在中文输入上 panic
    //   （实测 `parse_color("浅蓝")` —— "浅蓝" 6 个字节、正好撞上 len==6 那一档，
    //    而 `&hex[0..2]` 落在"浅"字中间 → `byte index 2 is not a char boundary`）。
    if !hex.iter().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |c: char| c.to_digit(16).unwrap_or(0);
    match hex.len() {
        3 => Some([
            (nibble(hex[0]) * 17) as u8,
            (nibble(hex[1]) * 17) as u8,
            (nibble(hex[2]) * 17) as u8,
        ]),
        6 => Some([
            (nibble(hex[0]) * 16 + nibble(hex[1])) as u8,
            (nibble(hex[2]) * 16 + nibble(hex[3])) as u8,
            (nibble(hex[4]) * 16 + nibble(hex[5])) as u8,
        ]),
        _ => None,
    }
}

/// **按颜色扔掉边缘的"混合像素"**（替代几何腐蚀 —— 用户 2026-09-26：
/// "细的头发也被你去掉了"）。
///
/// 与 `erode_mask` 的区别：腐蚀是**几何**的，比 `2*radius` 细的结构整条消失（发丝正好中招）；
/// 这里是**按颜色**判 —— 边缘那几个像素是"人物 + 背景"的混合色，颜色离背景色**近**，扔掉；
/// 发丝像素是头发色（离背景色远），**天然保住**。这才是绿幕抠图该有的"收边"。
///
/// 只在**边界往内 `band` 像素**这条带上动手；返回是否改过。
pub fn drop_blended_edge(
    mask: &mut [u8],
    image: &image::RgbImage,
    w: usize,
    h: usize,
    background: [u8; 3],
    band: usize,
    tolerance: f64,
) -> bool {
    let cutoff = (tolerance.clamp(0.01, 1.0) * 255.0).round() as i32;
    let r = band as i64;
    let mut changed = false;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let index = y as usize * w + x as usize;
            if mask[index] <= 127 {
                continue;
            }
            // 只在边界带里判
            let mut on_edge = false;
            'scan: for dy in -r..=r {
                let ny = y + dy;
                if ny < 0 || ny >= h as i64 {
                    continue;
                }
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w as i64 {
                        continue;
                    }
                    if mask[ny as usize * w + nx as usize] <= 127 {
                        on_edge = true;
                        break 'scan;
                    }
                }
            }
            if !on_edge {
                continue;
            }
            let pixel = image.get_pixel(x as u32, y as u32);
            let distance = (0..3)
                .map(|c| (pixel[c] as i32 - background[c] as i32).abs())
                .max()
                .unwrap_or(0);
            if distance <= cutoff {
                mask[index] = 0;
                changed = true;
            }
        }
    }
    changed
}

/// 把遮罩**往里收** `radius` 个像素（腐蚀 / choke）。
///
/// 为什么贴人物图必须收一刀（用户 2026-09-26 问的"人物怎么冒背景的泛光"）：
/// 主体与背景交界那一圈是**抗锯齿 / 溢色**像素 —— 绿幕上就是一圈绿边、白底上就是一圈白边。
/// 它们与背景色的差刚好超过容差，于是被判成"主体"贴进画面，再被 ring 那道重绘一晕，
/// 就成了绕着人物的一圈泛光。收 1~3 像素把这圈扔掉，接缝交给 ring 去融。
///
/// 方形的结构元够用（这一刀是"宁少勿多"，不需要圆的精确形状）。
pub fn erode_mask(mask: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 {
        return mask.to_vec();
    }
    let r = radius as i64;
    let mut out = vec![0u8; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut keep = true;
            'outer: for dy in -r..=r {
                let ny = y + dy;
                if ny < 0 || ny >= h as i64 {
                    keep = false;
                    break;
                }
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w as i64 || mask[ny as usize * w + nx as usize] <= 127 {
                        keep = false;
                        break 'outer;
                    }
                }
            }
            if keep {
                out[y as usize * w + x as usize] = 255;
            }
        }
    }
    out
}

/// **去掉边缘的溢色**（despill / de-fringe）——用户 2026-09-26 的收尾要求。
///
/// 光靠 choke 不够：羽化会让最外几像素**半透明**，而那几个像素本身是"人物 + 背景"的混合色
/// （绿幕上偏绿、白底上偏白），半透明地贴上去就是绕着人的一圈背景色。
/// 这里把背景**主色通道**压回另外两个通道的水平：
/// 绿幕（G 最大）→ `g = min(g, (r+b)/2)`；品红（R/B 最大）→ `r = min(r, g)`、`b = min(b, g)`。
/// 这是绿幕合成的标准做法，**只动边界那条带**，人物内部一个像素都不碰。
///
/// 两侧各 `radius` 像素（= [`despill_band`] 的 `inner = outer = radius`）。
///
/// 返回是否真的改过（没改就别去覆盖文件）。
pub fn despill_edges(
    person: &mut image::RgbImage,
    mask: &[u8],
    w: usize,
    h: usize,
    background: [u8; 3],
    radius: usize,
) -> bool {
    despill_band(person, mask, w, h, background, radius, radius)
}

/// [`despill_edges`] 的**两侧带**版本：掩码内 `inner` 像素 + 掩码**外** `outer` 像素。
///
/// # ⚠ 为什么必须有"外带"（2026-09-27 实测，总纲 §16.4）
///
/// 一直只有"往内"那一侧，而**真正的溢色像素在掩码外面**：主体与背景交界那圈抗锯齿像素是
/// "人 + 背景"的混合色，颜色更靠近背景 → **被判成了背景**（不在掩码里）。于是"边界往内 radius
/// 像素"那条带**一个真正的溢色像素都盖不到**，剩下的量正好就是用户看到的"边缘一圈淡淡的背景色"。
///
/// 量到的（768x1024 绿幕动漫人物，合成到**平灰底** —— 底不带绿，所以量到的绿只能来自人物自己）：
///
/// | 作用域 | 边界溢色均值 | max |
/// | --- | --- | --- |
/// | 只往内（旧） | 3.36 | 32 |
/// | **两侧**（`inner=6, outer=6~8`） | **0.00** | **0** |
///
/// 触发条件没变（只有"背景主色通道真的压过另外两个通道"的像素才被压），所以**人物身上真正的
/// 绿色内容不会被误压**；白 / 浅灰背景照旧整支跳过（没有溢色通道可压）。
pub fn despill_band(
    person: &mut image::RgbImage,
    mask: &[u8],
    w: usize,
    h: usize,
    background: [u8; 3],
    inner: usize,
    outer: usize,
) -> bool {
    let saturation = background.iter().max().copied().unwrap_or(0) as i32
        - background.iter().min().copied().unwrap_or(0) as i32;
    // 背景不彩（白 / 浅灰）就没什么"溢色通道"可压 —— 那时靠 choke + 羽化小一点
    if saturation < CHROMA_MIN_SATURATION || (inner == 0 && outer == 0) {
        return false;
    }
    // 背景的主导通道 = 溢色方向
    let dominant = (0..3).max_by_key(|c| background[*c]).unwrap_or(1);
    let others: Vec<usize> = (0..3).filter(|c| *c != dominant).collect();
    let (ri, ro) = (inner as i64, outer as i64);
    let mut changed = false;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let index = y as usize * w + x as usize;
            let inside = mask[index] > 127;
            let r = if inside { ri } else { ro };
            if r == 0 {
                continue;
            }
            // 在带内 = 掩码内的像素邻域里看得到背景、掩码外的像素邻域里看得到主体
            let mut on_edge = false;
            'scan: for dy in -r..=r {
                let ny = y + dy;
                if ny < 0 || ny >= h as i64 {
                    continue;
                }
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w as i64 {
                        continue;
                    }
                    if (mask[ny as usize * w + nx as usize] > 127) != inside {
                        on_edge = true;
                        break 'scan;
                    }
                }
            }
            if !on_edge {
                continue;
            }
            let pixel = person.get_pixel_mut(x as u32, y as u32);
            let limit = ((pixel[others[0]] as f64 + pixel[others[1]] as f64) / 2.0 * 1.06) as u8;
            if pixel[dominant] > limit {
                pixel[dominant] = limit;
                changed = true;
            }
        }
    }
    changed
}

/// **边缘反混（un-mix）**：把"人 + 背景"的混合像素**还原成人物自己的颜色**。
///
/// 为什么 despill 不够（用户 2026-09-26 第五次看边缘）：despill 只是把绿通道**压回**
/// 另外两个通道的水平 —— 压完是"灰绿/橄榄绿"，肉眼仍然是一圈背景色的边。
/// 正确的模型是 `p = a·P + (1-a)·B`（P = 人物本体色、B = 背景色、a = 覆盖率）：
/// 取边界带内侧**干净**的邻居色当 `P`，最小二乘解出 `a`，再 `P' = B + (p-B)/a` —— 反混之后
/// 那个像素就是人物自己的颜色（`a` 太小的丢掉，说明它本来就基本是背景）。
///
/// 与 despill 的区别：despill 是"**压通道**"（仍留混合色），这里是"**解方程**"（还原本体色）。
/// 绿幕、白底、品红**都适用**（不依赖背景是不是彩色）。返回是否改过。
pub fn unmix_edge(
    person: &mut image::RgbImage,
    mask: &mut [u8],
    w: usize,
    h: usize,
    background: [u8; 3],
    band: usize,
) -> bool {
    if band == 0 {
        return false;
    }
    // "干净" = 往里收 `band` 像素之后还在掩码里的部分（它的颜色没被背景混过）
    let clean = erode_mask(mask, w, h, band);
    let r = band as i64;
    let mut changed = false;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let index = y as usize * w + x as usize;
            if mask[index] <= 127 || clean[index] > 127 {
                continue;
            }
            // 边界带里才动手（邻域里有背景像素）
            let mut on_edge = false;
            'scan: for dy in -r..=r {
                let ny = y + dy;
                if ny < 0 || ny >= h as i64 {
                    continue;
                }
                for dx in -r..=r {
                    let nx = x + dx;
                    if nx < 0 || nx >= w as i64 {
                        continue;
                    }
                    if mask[ny as usize * w + nx as usize] <= 127 {
                        on_edge = true;
                        break 'scan;
                    }
                }
            }
            if !on_edge {
                continue;
            }
            // 找最近的一个"干净"邻居当本体色 P
            let reach = r * 2 + 2;
            let mut reference: Option<[f64; 3]> = None;
            'search: for ring in 1..=reach {
                for dy in -ring..=ring {
                    for dx in -ring..=ring {
                        if dx.abs() != ring && dy.abs() != ring {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let nindex = ny as usize * w + nx as usize;
                        if clean[nindex] > 127 {
                            let pixel = person.get_pixel(nx as u32, ny as u32);
                            reference = Some([pixel[0] as f64, pixel[1] as f64, pixel[2] as f64]);
                            break 'search;
                        }
                    }
                }
            }
            let Some(reference) = reference else { continue };
            let b = [background[0] as f64, background[1] as f64, background[2] as f64];
            let pixel = person.get_pixel(x as u32, y as u32);
            let p = [pixel[0] as f64, pixel[1] as f64, pixel[2] as f64];
            // 最小二乘解 a：p = a·P + (1-a)·B
            let mut num = 0.0;
            let mut den = 0.0;
            for c in 0..3 {
                num += (p[c] - b[c]) * (reference[c] - b[c]);
                den += (reference[c] - b[c]) * (reference[c] - b[c]);
            }
            if den < 1.0 {
                continue;
            }
            let a = (num / den).clamp(0.0, 1.0);
            // 判据是**覆盖率 a**，不是"颜色离背景多远"（2026-09-26 两次踩坑的结论）：
            //   · 发丝：a≈1（纯人物色）；皮肤阴影：a≈0.69；混合边：a≈0.28。
            //   · 用颜色距离判 → 中间调的皮肤和混合像素**分不开**：容差开大就把皮肤删掉
            //     （人身上一块块变透明），开小就留一圈绿边。
            //   · 用 a 判 → 一刀两断：a < 0.45 的（不到一半是人物）从掩码里去掉，
            //     其余一律**反混修颜色**。绿边是 a 小的那批，发丝与皮肤都远高于 0.45。
            if a < 0.45 {
                mask[index] = 0;
                changed = true;
                continue;
            }
            let mut out = [0u8; 3];
            for c in 0..3 {
                out[c] = (b[c] + (p[c] - b[c]) / a).clamp(0.0, 255.0) as u8;
            }
            let pixel = person.get_pixel_mut(x as u32, y as u32);
            if (pixel[0], pixel[1], pixel[2]) != (out[0], out[1], out[2]) {
                pixel[0] = out[0];
                pixel[1] = out[1];
                pixel[2] = out[2];
                changed = true;
            }
        }
    }
    changed
}

/// **按背景给新图层打光 + 烧影子**（用户 2026-09-26 的最后一个要求）。
///
/// # 怎么估（不依赖任何模型）
///
/// ① **光向**：拿背景（遮罩之外）在剪影包围盒四周的**亮度差**估 ——
///    `右半 - 左半` 给水平分量、`下半 - 上半` 给垂直分量（还带一点"光从哪来"的常识兜底：
///    差太小就按左上）。这与"影子朝光源反方向"是同一条物理直觉。
/// ② **色温 / 色偏**：背景（剪影附近）的平均色 ÷ 人物的平均色 —— 夜景就把人压冷压暗、
///    暖灯下就把人拉暖（乘法，夹在 0.75~1.3，别把人拉爆）。
/// ③ **方向梯度**：沿光向的线性 ramp 乘上去（亮面亮、暗面暗），强度 `light`（0~1）。
/// ④ **投影**：把剪影沿**光向的反方向**平移 `offset` 并糊一遍当影子，按 `shadow`（0~1）
///    把**背景色**压暗，写进人物图的像素里、并让那几像素落进遮罩 —— 于是合成时影子跟着一起
///    贴上去（不用改工作流）。
///
/// ⚠ 它是**近似**，不是重打光模型（ComfyUI 里要真重打光得有 IC-Light 那类节点）。
/// 如实写在返回值里（`lightDir` / `lightEstimated` / `shadowOffset`），别吹成"重打光"。
pub fn relight_and_shadow(
    person: &mut image::RgbImage,
    mask: &mut [u8],
    base: &image::RgbImage,
    w: usize,
    h: usize,
    direction: Option<[f64; 2]>,
    light: f64,
    shadow: f64,
    shadow_offset: usize,
) -> ([f64; 2], bool) {
    let bbox = mask_bbox(mask, w, h);
    // ① 光向：显式给了就用给的；没给就从背景的四周亮度差里估
    let (mut dir, estimated) = match direction {
        Some(d) => (normalize2(d), false),
        None => (estimate_light_dir(base, mask, w, h, bbox), true),
    };
    if dir == [0.0, 0.0] {
        dir = normalize2([-0.7, -0.7]); // 兜底：光从左上来（影子朝右下）
    }

    // ② 色偏：背景（遮罩外）与人物（遮罩内）的平均色之比
    let mut bg = [0.0f64; 3];
    let mut fg = [0.0f64; 3];
    let (mut bg_n, mut fg_n) = (0.0f64, 0.0f64);
    for y in 0..h {
        for x in 0..w {
            let index = y * w + x;
            if mask[index] > 127 {
                // 人物那一侧只统计**真有像素**的地方（画布其余部分是黑底，算进去会把色偏估反）
                let pixel = *person.get_pixel(x as u32, y as u32);
                if pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32 > 0 {
                    for c in 0..3 {
                        fg[c] += pixel[c] as f64;
                    }
                    fg_n += 1.0;
                }
            } else {
                let pixel = *base.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    bg[c] += pixel[c] as f64;
                }
                bg_n += 1.0;
            }
        }
    }
    let tint = if fg_n > 0.0 && bg_n > 0.0 {
        let mut out = [1.0f64; 3];
        for c in 0..3 {
            let bg_mean = bg[c] / bg_n.max(1.0);
            let fg_mean = fg[c] / fg_n.max(1.0);
            out[c] = if fg_mean > 1.0 { (bg_mean / fg_mean).clamp(0.75, 1.3) } else { 1.0 };
        }
        out
    } else {
        [1.0, 1.0, 1.0]
    };

    // ③ 方向梯度：以包围盒中心为原点，沿光向线性增减
    let (cx, cy, span) = match bbox {
        Some((x0, y0, x1, y1)) => (
            (x0 + x1) as f64 / 2.0,
            (y0 + y1) as f64 / 2.0,
            (((x1 - x0).max(y1 - y0)) as f64 / 2.0).max(1.0),
        ),
        None => (w as f64 / 2.0, h as f64 / 2.0, (w.max(h) as f64) / 2.0),
    };
    let light = light.clamp(0.0, 1.0);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] <= 127 {
                continue;
            }
            let ramp = (((x as f64 - cx) * dir[0] + (y as f64 - cy) * dir[1]) / span).clamp(-1.0, 1.0);
            let factor = (1.0 + light * ramp) * 1.0;
            let pixel = person.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                let value = pixel[c] as f64 * factor * tint[c];
                pixel[c] = value.clamp(0.0, 255.0) as u8;
            }
        }
    }

    // ④ 投影：剪影往光的**反方向**挪、糊一遍，压在背景色上（写进人物图 + 扩遮罩）
    if shadow > 0.0 && shadow_offset > 0 {
        let source = mask.to_vec();
        let dx = (-dir[0] * shadow_offset as f64).round() as i64;
        let dy = (-dir[1] * shadow_offset as f64).round() as i64;
        let mut shadow_mask = vec![0u8; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let (sx, sy) = (x - dx, y - dy);
                if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                    continue;
                }
                if source[sy as usize * w + sx as usize] > 127 {
                    shadow_mask[y as usize * w + x as usize] = 255;
                }
            }
        }
        let blurred = box_blur(&shadow_mask, w, h, 24);
        for y in 0..h {
            for x in 0..w {
                let alpha = blurred[y * w + x] as f64 / 255.0;
                if alpha <= 0.02 {
                    continue;
                }
                // ⚠ **影子不许盖住人物自己**：模糊会把影子扩进剪影内部，不收这一下就把人物的
                //   像素整块换成"压暗的背景"（实测：`lit-*.png` 里那块是人形的一片街景）
                if source[y * w + x] > 127 {
                    continue;
                }
                let darken = 1.0 - shadow.clamp(0.0, 1.0) * alpha;
                let background = *base.get_pixel(x as u32, y as u32);
                let pixel = person.get_pixel_mut(x as u32, y as u32);
                for c in 0..3 {
                    pixel[c] = (background[c] as f64 * darken).clamp(0.0, 255.0) as u8;
                }
                mask[y * w + x] = 255; // 影子也要跟着贴上去
            }
        }
    }

    (dir, estimated)
}

/// 从背景的四周亮度差估光向（`[dx, dy]`，未归一化时为 0 表示"看不出方向"）。
fn estimate_light_dir(base: &image::RgbImage, mask: &[u8], w: usize, h: usize, bbox: Option<(usize, usize, usize, usize)>) -> [f64; 2] {
    let (x0, y0, x1, y1) = bbox.unwrap_or((0, 0, w, h));
    let (mut left, mut right, mut top, mut bottom) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut ln, mut rn, mut tn, mut bn) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (cx, cy) = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] > 127 {
                continue;
            }
            let pixel = base.get_pixel(x as u32, y as u32);
            let luma = (pixel[0] as f64 * 0.299 + pixel[1] as f64 * 0.587 + pixel[2] as f64 * 0.114) / 255.0;
            if (x as f64) < cx {
                left += luma;
                ln += 1.0;
            } else {
                right += luma;
                rn += 1.0;
            }
            if (y as f64) < cy {
                top += luma;
                tn += 1.0;
            } else {
                bottom += luma;
                bn += 1.0;
            }
        }
    }
    if ln == 0.0 || rn == 0.0 || tn == 0.0 || bn == 0.0 {
        return [0.0, 0.0];
    }
    // 亮的一侧就是"光来的方向"
    let dx = right / rn - left / ln;
    let dy = bottom / bn - top / tn;
    if dx.abs() < 0.01 && dy.abs() < 0.01 {
        return [0.0, 0.0];
    }
    [dx, dy]
}

/// 单位化（长度 0 时回 `[0,0]`）。
fn normalize2(v: [f64; 2]) -> [f64; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

/// 遮罩里"有内容"的那块矩形（`(x0, y0, x1, y1)`，右/下开区间）；全黑时回 `None`。
pub fn mask_bbox(mask: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] > 127 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX {
        None
    } else {
        Some((x0, y0, x1, y1))
    }
}

/// 画遮罩：黑底 + 白色区域（0 = 保留，255 = 重绘）。///
/// 椭圆做 **4×4 超采样**的覆盖率（Kotlin 用 `Graphics2D` 的抗锯齿，语义一致：
/// 边缘一圈是灰的，所以贴人物时边界不会出现硬台阶）。矩形保持硬边（Kotlin 那边
/// `fillRect` 在整数坐标下同样没有半像素）。
fn draw_mask(w: usize, h: usize, region: &[f64; 4], shape: &str) -> Vec<u8> {
    let mut pixels = vec![0u8; w * h];
    let x = (region[0] * w as f64).round().clamp(0.0, w as f64) as i64;
    let y = (region[1] * h as f64).round().clamp(0.0, h as f64) as i64;
    let rw = (region[2] * w as f64).round().clamp(0.0, (w as i64 - x) as f64) as i64;
    let rh = (region[3] * h as f64).round().clamp(0.0, (h as i64 - y) as f64) as i64;
    if rw <= 0 || rh <= 0 {
        return pixels;
    }
    if shape == "ellipse" {
        let cx = x as f64 + rw as f64 / 2.0;
        let cy = y as f64 + rh as f64 / 2.0;
        let rx = rw as f64 / 2.0;
        let ry = rh as f64 / 2.0;
        for py in y..(y + rh) {
            for px in x..(x + rw) {
                let mut hits = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let fx = px as f64 + (sx as f64 + 0.5) / 4.0;
                        let fy = py as f64 + (sy as f64 + 0.5) / 4.0;
                        let dx = (fx - cx) / rx;
                        let dy = (fy - cy) / ry;
                        if dx * dx + dy * dy <= 1.0 {
                            hits += 1;
                        }
                    }
                }
                pixels[py as usize * w + px as usize] = (hits * 255 / 16) as u8;
            }
        }
        return pixels;
    }
    for py in y..(y + rh) {
        for px in x..(x + rw) {
            pixels[py as usize * w + px as usize] = 255;
        }
    }
    pixels
}

/// 两次一维均值模糊（可分离），半径 = `feather / 2`（与 Kotlin `boxBlur` 逐行同义，
/// 含**边缘钳位**：越界的取样用边界像素，所以遮罩边缘不会变暗）。
fn box_blur(src: &[u8], w: usize, h: usize, feather: i64) -> Vec<u8> {
    let radius = (feather / 2).max(1) as i64;
    let window = (2 * radius + 1) as i64;
    let idx = |x: i64, y: i64| -> usize {
        y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize
    };
    let mut tmp = vec![0u8; w * h];
    // 横向
    for y in 0..h as i64 {
        let mut sum: i64 = 0;
        for i in -radius..=radius {
            sum += src[idx(i, y)] as i64;
        }
        for x in 0..w as i64 {
            tmp[y as usize * w + x as usize] = (sum / window) as u8;
            sum += src[idx(x + radius + 1, y)] as i64 - src[idx(x - radius, y)] as i64;
        }
    }
    let mut out = vec![0u8; w * h];
    // 纵向
    for x in 0..w as i64 {
        let mut sum: i64 = 0;
        for i in -radius..=radius {
            sum += tmp[idx(x, i)] as i64;
        }
        for y in 0..h as i64 {
            out[y as usize * w + x as usize] = (sum / window).clamp(0, 255) as u8;
            sum += tmp[idx(x, y + radius + 1)] as i64 - tmp[idx(x, y - radius)] as i64;
        }
    }
    out
}

/// 参数指纹（同一组参数 → 同一个文件名，重复调用不会在 input 目录里堆垃圾）。
fn content_tag(w: i64, h: i64, region: &[f64; 4], shape: &str, invert: bool, feather: i64) -> String {
    let region_joined = region.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
    let text = format!("{w}|{h}|{region_joined}|{shape}|{invert}|{feather}");
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(6).map(|byte| format!("{byte:02x}")).collect()
}

/// 写 PNG（**8 位灰度**：ComfyUI 的 `LoadImageMask(channel="red")` 直接取第一个通道，
/// 与 Kotlin 的 `TYPE_BYTE_GRAY` 同一个形状）。
fn write_mask_png(pixels: &[u8], w: u32, h: u32, target: &Path) -> Result<(), ToolFailure> {
    let fail = |e: String| {
        ToolFailure::new(
            "COMFY_INPUT_FAILED",
            format!(
                "遮罩写不进 {}：{}",
                target.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                take_text(&e, 200)
            ),
        )
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    }
    let buffer = image::GrayImage::from_raw(w, h, pixels.to_vec())
        .ok_or_else(|| fail("像素数与尺寸对不上".to_string()))?;
    buffer.save(target).map_err(|e| fail(e.to_string()))
}

// ---------------------------------------------------------------------------
//  参数取值（与 Kotlin `JsonObject.str/int/bool` 同义：**容错，但不猜**）
// ---------------------------------------------------------------------------

/// 非空字符串参数（空串 / 非字符串都当"没给"）。
fn str_arg(args: &Value, field: &str) -> Option<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 整数参数：**数字与字符串都认**（模型经常把 `768` 写成 `"768"`）。
fn int_arg(args: &Value, field: &str) -> Option<i64> {
    match args.get(field)? {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|f| f as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 布尔参数：只认真布尔与 `"true"` / `"false"`（与 Kotlin `toBooleanStrictOrNull` 同义，
/// **不**把 `"yes"` / `1` 猜成 true —— 遮罩反选猜错方向等于把整张图重画）。
fn bool_arg(args: &Value, field: &str) -> Option<bool> {
    match args.get(field)? {
        Value::Bool(value) => Some(*value),
        Value::String(text) => match text.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
//  工具入口（`tools::execute` 调它们）
// ---------------------------------------------------------------------------

/// `comfy_use_attachment` 的工具入口（参数从模型给的 JSON 里取，**当不可信数据**）。
pub fn tool_use_attachment(state: &AppState, arguments: &Value) -> Result<crate::tools::ToolOutcome, ToolFailure> {
    let attachment_id = arguments
        .get("attachmentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", "缺少参数 attachmentId（见用户消息里的附件说明）"))?
        .to_string();
    let wanted = str_arg(arguments, "filename");
    let result = use_attachment(state, &attachment_id, wanted.as_deref())?;
    Ok(crate::tools::ToolOutcome::new(result.to_string(), result))
}

/// `comfy_use_media` 的工具入口。
///
/// `mediaId` 认**数字与字符串**（模型会把 id 写成 `"310"`）——与 Kotlin 那边同一条容错。
pub fn tool_use_media(state: &AppState, arguments: &Value) -> Result<crate::tools::ToolOutcome, ToolFailure> {
    let media_id = int_arg(arguments, "mediaId")
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", "缺少参数 mediaId（画廊里的图片产物 id）"))?;
    let wanted = str_arg(arguments, "filename");
    let result = use_media(state, media_id, wanted.as_deref())?;
    Ok(crate::tools::ToolOutcome::new(result.to_string(), result))
}

/// `comfy_make_mask` 的工具入口。
pub fn tool_make_mask(state: &AppState, arguments: &Value) -> Result<crate::tools::ToolOutcome, ToolFailure> {
    let result = make_mask(state, arguments)?;
    Ok(crate::tools::ToolOutcome::new(result.to_string(), result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 投放文件名可重复且不互相覆盖() {
        // 同一个来源 → 同一个名字（重跑工作流不会指向一个已经没有的文件）
        assert_eq!(
            input_filename_for("image.png", "att_7f3a9c21", None),
            input_filename_for("image.png", "att_7f3a9c21", None)
        );
        // 不同来源、同一个原文件名 → 不互相覆盖
        assert_ne!(
            input_filename_for("image.png", "att_aaaabbbb", None),
            input_filename_for("image.png", "att_ccccdddd", None)
        );
        // 路径分隔符与奇怪字符一律清掉（ComfyUI 只认裸文件名）——
        // ⚠ 这里与 Kotlin **逐字一致**：它把整串都做字符映射（不先取 basename），
        //   所以结果里会留下一串下划线开头。**没有穿越风险**：分隔符已经全变成 `_`，
        //   再 join 到 input 目录之下也拼不出第二个目录。
        assert_eq!(input_filename_for(r"..\..\etc\pass wd.PNG", "m310", None), "_.._etc_pass_wd-m310.PNG");
        // 全是点 → 清完之后空掉 → 回落到 image.png（否则会拼出一个没有名字的文件）
        assert_eq!(input_filename_for("....", "m310", None), "image-m310.png");
        // 显式文件名优先（id 后缀插在**扩展名之前**，所以 LoadImage 还认得出它是 png）
        assert_eq!(input_filename_for("orig.png", "m310", Some("bg.png")), "bg-m310.png");
        // 没有任何扩展名时补 .png（ComfyUI 的 LoadImage 按扩展名找解码器）
        assert_eq!(input_filename_for("noext", "m1", None), "noext-m1.png");
        // 纯扩展名（`.png`）要当"没有扩展名"处理，别把 stem 变成空串
        assert_eq!(input_filename_for(".png", "m1", None), "png-m1.png");
    }

    #[test]
    fn region_预设与显式坐标都对() {
        assert_eq!(parse_region("full").unwrap(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(parse_region("PERSON").unwrap(), [0.22, 0.05, 0.56, 0.92]);
        assert_eq!(parse_region("").unwrap(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(parse_region("0.5,0,0.5,1").unwrap(), [0.5, 0.0, 0.5, 1.0]);
        // 越界的宽高要**夹到边界内**（0.8+0.5 > 1），而不是画到画布外面去
        assert_eq!(parse_region("0.8,0.8,0.5,0.5").unwrap(), [0.8, 0.8, 0.19999999999999996, 0.19999999999999996]);
    }

    #[test]
    fn region_四种拒绝都是人话() {
        let too_few = parse_region("0.1,0.2").unwrap_err();
        assert_eq!(too_few.code, "INVALID_ARGUMENT");
        assert!(too_few.message.contains("x,y,w,h"), "要说清该给什么：{}", too_few.message);
        assert!(parse_region("0.1,0.2,0.3,第四").is_err(), "非数字要报错，不许静默当 0");
        assert!(parse_region("1.2,0,0.5,0.5").is_err(), "超过 1 要报错（它是归一化的）");
        assert!(parse_region("1,0,0,0.5").is_err(), "宽高为 0 要报错（否则遮罩是全黑）");
    }

    #[test]
    fn 矩形遮罩_白区的位置与尺寸都对() {
        // 40x40 的画布上取中间那一半：白区必须**恰好在** [4,4)~(20,20)
        let region = [0.1, 0.1, 0.4, 0.4];
        let pixels = draw_mask(40, 40, &region, "rect");
        let at = |x: usize, y: usize| pixels[y * 40 + x];
        assert_eq!(at(0, 0), 0, "左上角是黑（保留）");
        assert_eq!(at(4, 4), 255, "白区左上角");
        assert_eq!(at(19, 19), 255, "白区右下角（含）");
        assert_eq!(at(20, 20), 0, "白区之外一个像素就是黑");
        assert_eq!(at(39, 39), 0);
        assert_eq!(pixels.iter().filter(|v| **v == 255).count(), 16 * 16);
    }

    #[test]
    fn 椭圆遮罩_边缘是抗锯齿的灰() {
        let pixels = draw_mask(64, 64, &[0.0, 0.0, 1.0, 1.0], "ellipse");
        let center = pixels[32 * 64 + 32];
        assert_eq!(center, 255, "中心是纯白");
        assert_eq!(pixels[0], 0, "四角在椭圆之外 → 黑");
        // 边缘必须存在中间值（全 0/255 就说明没做抗锯齿，贴人物时会出现硬台阶）
        assert!(
            pixels.iter().any(|v| *v > 0 && *v < 255),
            "椭圆边缘应该有抗锯齿的灰阶"
        );
    }

    #[test]
    fn 反选把里外翻过来() {
        let normal = draw_mask(32, 32, &[0.25, 0.25, 0.5, 0.5], "rect");
        let inverted: Vec<u8> = normal.iter().map(|v| 255 - *v).collect();
        assert_eq!(normal[16 * 32 + 16], 255);
        assert_eq!(inverted[16 * 32 + 16], 0);
        assert_eq!(normal[0], 0);
        assert_eq!(inverted[0], 255);
    }

    #[test]
    fn 羽化_只糊边缘不动中心() {
        // 白区右边一半，羽化 16 → 半径 8 的均值模糊
        let region = [0.5, 0.0, 0.5, 1.0];
        let sharp = draw_mask(64, 64, &region, "rect");
        let blurred = box_blur(&sharp, 64, 64, 16);
        let at = |p: &Vec<u8>, x: usize, y: usize| p[y * 64 + x];
        assert_eq!(at(&sharp, 32, 32), 255, "硬边界：第 32 列是白的");
        assert!(at(&blurred, 32, 32) < 255 && at(&blurred, 32, 32) > 0, "羽化后边界变成过渡值");
        assert_eq!(at(&blurred, 63, 32), 255, "远离边界的地方仍是纯白");
        assert_eq!(at(&blurred, 0, 32), 0, "远离边界的地方仍是纯黑");
        // 羽化**不该**让整张图变暗（边缘钳位保证这一点）
        assert_eq!(at(&blurred, 63, 0), 255);
    }

    #[test]
    fn 指纹只由参数决定() {
        let a = content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 16);
        let b = content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 16);
        assert_eq!(a, b, "同一组参数必须得到同一个文件名");
        assert_eq!(a.len(), 12, "6 个字节的十六进制");
        assert_ne!(a, content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", true, 16));
        assert_ne!(a, content_tag(768, 1024, &[0.58, 0.26, 0.36, 0.74], "rect", false, 24));
        assert_ne!(a, content_tag(768, 1024, &[0.5, 0.26, 0.36, 0.74], "rect", false, 16));
    }

    #[test]
    fn 取值容错但不猜() {
        let args = json!({"w": "768", "h": 1024.0, "b": "true", "n": "yes", "s": "  bg.png  "});
        assert_eq!(int_arg(&args, "w"), Some(768));
        assert_eq!(int_arg(&args, "h"), Some(1024));
        assert_eq!(int_arg(&args, "missing"), None);
        assert_eq!(bool_arg(&args, "b"), Some(true));
        assert_eq!(bool_arg(&args, "n"), None, "「yes」不许猜成 true");
        assert_eq!(str_arg(&args, "s"), Some("bg.png".to_string()));
        assert_eq!(str_arg(&args, "empty"), None);
    }

    /// 造一张"白底 + 一个红方块"的图（模拟阶段一的白底产物）。
    fn white_bg_with_block(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> image::RgbImage {
        let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([252, 252, 250]));
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, image::Rgb([200, 30, 40]));
            }
        }
        img
    }

    /// `mode="subject"`：白底上抠一个方块，遮罩就该是**那个方块的形状**（不是整块矩形画面）。
    ///
    /// 这是"贴人物图出一圈白框"那条现场的解法（用户 2026-09-26 提的）：
    /// `ImageCompositeMasked` 按遮罩搬像素，遮罩是矩形就会把人物图那块矩形连背景一起搬进去。
    #[test]
    fn 主体遮罩抠出的是形状不是整块矩形() {
        let dir = std::env::temp_dir().join(format!("comfyhub-subject-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("person.png");
        // 400x400 的白底，中间一个 100x200 的红方块（模拟站着的人）
        white_bg_with_block(400, 400, 150, 100, 250, 300).save(&path).unwrap();

        let (mask, background, ratio) =
            subject_mask(&path, 400, 400, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        assert_eq!(background, [252, 252, 250], "背景色要从四周一圈估出来");
        let at = |x: usize, y: usize| mask[y * 400 + x];
        assert_eq!(at(200, 200), 255, "方块里是白的（要重绘）");
        assert_eq!(at(20, 20), 0, "白底是黑的（保留）—— 这一条就是白框的解药");
        assert_eq!(at(390, 390), 0);
        // 占比 ≈ 100*200/400*400 = 12.5%
        assert!((ratio - 0.125).abs() < 0.02, "占比应当接近 12.5%，实测 {ratio}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 主体遮罩只留最大的一块连通区并支持与矩形取交集() {
        let dir = std::env::temp_dir().join(format!("comfyhub-subject2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("person.png");
        let mut img = white_bg_with_block(300, 300, 120, 60, 180, 260);
        // 角落里一个孤立的小红块：背景噪点 / 水印纹的替身 —— 必须被丢掉
        for y in 10..20 {
            for x in 10..20 {
                img.put_pixel(x, y, image::Rgb([210, 40, 40]));
            }
        }
        img.save(&path).unwrap();

        let (mask, _, _) =
            subject_mask(&path, 300, 300, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        assert_eq!(mask[15 * 300 + 15], 0, "孤立的小块要当噪点丢掉");
        assert_eq!(mask[150 * 300 + 150], 255);

        // 与 region 取交集：方块横跨 x=120..180，画布 300 → 中线在 150
        let (left, _, _) =
            subject_mask(&path, 300, 300, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 0.5, 1.0], true, None).unwrap();
        assert_eq!(left[150 * 300 + 140], 255, "左半边里、方块内的点要留着");
        assert_eq!(left[150 * 300 + 160], 0, "region 之外必须清零");
        let (right, _, _) =
            subject_mask(&path, 300, 300, SUBJECT_TOLERANCE_DEFAULT, &[0.5, 0.0, 0.5, 1.0], true, None).unwrap();
        assert_eq!(right[150 * 300 + 160], 255, "右半边里、方块内的点要留着");
        assert_eq!(right[150 * 300 + 130], 0, "region 之外必须清零");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 尺寸不一致时遮罩要**按块平均缩放到画布**（与工作流里 `ImageScale` 的口径一致），
    /// 否则剪影与缩放后的像素对不上，贴出来就是错位的。
    #[test]
    fn 主体遮罩会缩放到画布尺寸() {
        let dir = std::env::temp_dir().join(format!("comfyhub-subject3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("person.png");
        white_bg_with_block(400, 400, 150, 100, 250, 300).save(&path).unwrap();

        let (mask, _, ratio) =
            subject_mask(&path, 200, 200, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        assert_eq!(mask.len(), 200 * 200);
        assert_eq!(mask[100 * 200 + 100], 255, "缩放后方块中心仍是白的");
        assert_eq!(mask[10 * 200 + 10], 0);
        assert!((ratio - 0.125).abs() < 0.03, "缩放后占比不变：{ratio}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **主体自己也与背景同色**时（白裙子 / 白衬衫 / 高光）要能填回来 ——
    /// 2026-09-26 现场：白底人物图贴进夜景后"人变成半透明"（裙子那块透出街景）。
    /// 判据只能是连通性：被深色描边围住的白色窟窿属于主体。
    #[test]
    fn 主体遮罩要把被包住的同色窟窿填回来() {
        let dir = std::env::temp_dir().join(format!("comfyhub-subject4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("person.png");
        // 白底 + 一个**深色描边的空心方块**（方块内部也是白的 = 白裙子）
        let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([252, 252, 250]));
        for y in 40..160 {
            for x in 60..140 {
                let edge = y < 44 || y >= 156 || x < 64 || x >= 136;
                img.put_pixel(x, y, if edge { image::Rgb([20, 20, 24]) } else { image::Rgb([250, 250, 248]) });
            }
        }
        img.save(&path).unwrap();

        let (mask, _, ratio) =
            subject_mask(&path, 200, 200, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        assert_eq!(mask[100 * 200 + 100], 255, "被描边围住的白色内部必须算主体（否则人变半透明）");
        assert_eq!(mask[10 * 200 + 10], 0, "画面四角的真背景不许被填");
        // 实心方块 80x120 ≈ 24%（填洞前后都是这个数）
        assert!((ratio - 0.24).abs() < 0.03, "占比应当接近 24%，实测 {ratio}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **绿幕背景**：主体不含背景色 → **不填内部空洞**，腿间/发丝之间的背景老实留着。
    ///
    /// 这是用户 2026-09-26 指出的正解："应该用不同于画面主体颜色的颜色作为背景" ——
    /// 白底 + 白裙子在像素上无法区分，绿幕就完全没有这个二义性。
    #[test]
    fn 绿幕背景不填洞且腿间留空() {
        let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([0, 177, 64]));
        // 主体：两条"腿" + 上面一条"胯"把它们连成**一块**（模拟人形），
        // 腿间留一条绿缝（就是用户看到的那圈白 —— 它必须留在背景里）
        for y in 40..60 {
            for x in 70..130 {
                img.put_pixel(x, y, image::Rgb([30, 30, 40]));
            }
        }
        for y in 60..160 {
            for x in 70..95 {
                img.put_pixel(x, y, image::Rgb([30, 30, 40]));
            }
            for x in 105..130 {
                img.put_pixel(x, y, image::Rgb([30, 30, 40]));
            }
        }
        let (mask, background, _) =
            subject_mask_from_image(&img, 200, 200, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None)
                .unwrap();
        assert_eq!(background, [0, 177, 64], "绿幕要能从边框认出来");
        let saturation = background.iter().max().unwrap() - background.iter().min().unwrap();
        assert!(saturation as i32 >= CHROMA_MIN_SATURATION, "绿幕是高饱和 → 走不填洞那一档");
        assert_eq!(mask[100 * 200 + 100], 0, "腿间那条绿缝必须留在背景里（这就是用户看到的那圈白）");
        assert_eq!(mask[100 * 200 + 80], 255, "左边那条腿是主体");
        assert_eq!(mask[100 * 200 + 115], 255, "右边那条腿是主体");
        assert_eq!(mask[5 * 200 + 5], 0);
    }

    /// 白底 + 白裙子：仍然要填洞（否则人变半透明），但这正是**二义**所在 ——
    /// 所以返回值里会带 `chromaAdvice` 提醒改用绿幕。
    #[test]
    fn 白底仍然填洞但会提醒改绿幕() {
        let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([252, 252, 250]));
        for y in 40..160 {
            for x in 70..130 {
                let edge = y < 44 || y >= 156 || x < 74 || x >= 126;
                img.put_pixel(x, y, if edge { image::Rgb([20, 20, 24]) } else { image::Rgb([250, 250, 248]) });
            }
        }
        let (mask, background, _) =
            subject_mask_from_image(&img, 200, 200, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None)
                .unwrap();
        let saturation = background.iter().max().unwrap() - background.iter().min().unwrap();
        assert!((saturation as i32) < CHROMA_MIN_SATURATION, "白底是低饱和 → 走填洞那一档");
        assert_eq!(mask[100 * 200 + 100], 255, "被描边围住的白色内部要填回来");
    }

    /// `bgColor`：显式给就用给的（不再去猜边框），认不出来的写法**不猜**。
    #[test]
    fn 背景色解析只认确定写法() {
        assert_eq!(parse_color("#00b140"), Some([0, 177, 64]));
        assert_eq!(parse_color("green"), Some([0, 177, 64]));
        assert_eq!(parse_color("绿幕"), Some([0, 177, 64]));
        assert_eq!(parse_color("magenta"), Some([255, 0, 255]));
        assert_eq!(parse_color("#0b0"), Some([0, 187, 0]));
        assert_eq!(parse_color("white"), Some([255, 255, 255]));
        assert_eq!(parse_color("浅蓝"), None, "认不出来就回落成「从边框估」，不猜");
    }
    /// **细发丝不许被删**（用户 2026-09-26："细的头发也被你去掉了"）：
    /// 与主体只隔着几个半透明像素的细条，必须靠"膨胀后取最大连通区再取交集"保住。
    #[test]
    fn 与主体细连接的细发丝不会被当噪点删掉() {
        let (w, h) = (80usize, 80usize);
        let mut mask = vec![0u8; w * h];
        // 主体：40x40 的方块
        for y in 20..60 {
            for x in 20..60 {
                mask[y * w + x] = 255;
            }
        }
        // 细化丝：从主体右侧伸出一条 1 像素宽的线，中间**断开 2 像素**（模拟被半透明像素隔开）
        for x in 60..72 {
            mask[40 * w + x] = 255;
        }
        for x in 74..80 {
            mask[40 * w + x] = 255;
        }
        keep_largest_blob(&mut mask, w, h);
        assert_eq!(mask[40 * w + 65], 255, "连着主体的发丝要保住");
        assert_eq!(mask[40 * w + 76], 255, "断口只有 2 像素、膨胀 3 能连上 → 也要保住");
        // 真正孤立的远处碎块（离主体 > 3 像素）照旧删掉
        for y in 2..5 {
            for x in 2..5 {
                mask[y * w + x] = 255;
            }
        }
        keep_largest_blob(&mut mask, w, h);
        assert_eq!(mask[3 * w + 3], 0, "孤立的噪点还是要删");
    }

    /// **按颜色收边**：边界那几个"人+背景"的混合像素扔掉，发丝（头发色）保住。
    #[test]
    fn 按颜色收边不碰发丝只扔混合像素() {
        let (w, h) = (40usize, 40usize);
        let mut image = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
        let mut mask = vec![0u8; w * h];
        for y in 10..30 {
            for x in 10..30 {
                let edge = x == 10 || x == 29 || y == 10 || y == 29;
                // 边界像素 = 人 + 绿幕的混合色（离背景色近）；内部与"发丝"是深色（离背景远）
                image.put_pixel(x as u32, y as u32, if edge { image::Rgb([60, 150, 70]) } else { image::Rgb([180, 40, 50]) });
                mask[y * w + x] = 255;
            }
        }
        // 一条 1 像素宽的深色发丝（颜色离绿幕很远）横穿左侧边界
        for x in 6..12 {
            image.put_pixel(x as u32, 20, image::Rgb([120, 90, 110]));
            mask[20 * w + x] = 255;
        }
        let changed = drop_blended_edge(&mut mask, &image, w, h, [0, 177, 64], 3, 0.35);
        assert!(changed);
        assert_eq!(mask[10 * w + 20], 0, "边界上的混合像素要扔掉");
        assert_eq!(mask[20 * w + 20], 255, "人物内部不动");
        assert_eq!(mask[20 * w + 7], 255, "深色发丝（离背景色远）必须保住");
    }

    /// **反混**：`50% 人 + 50% 绿幕` 的边界像素要被还原成人物本体色（而不是压成灰绿）。
    #[test]
    fn 反混把混合像素还原成人物颜色() {
        let (w, h) = (30usize, 30usize);
        let bg = [0u8, 177, 64];
        // 本体色（皮肤/裙子那种）：偏红
        let body = [200.0f64, 60.0, 80.0];
        // 边界像素 = 0.5*body + 0.5*bg
        let mixed = [
            ((0.5 * body[0] + 0.5 * bg[0] as f64)) as u8,
            ((0.5 * body[1] + 0.5 * bg[1] as f64)) as u8,
            ((0.5 * body[2] + 0.5 * bg[2] as f64)) as u8,
        ];
        let mut image = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb(bg));
        let mut mask = vec![0u8; w * h];
        // 10..20 是本体（纯 body 色），最外一圈是混合色
        for y in 10..20 {
            for x in 10..20 {
                let edge = x == 10 || x == 19 || y == 10 || y == 19;
                image.put_pixel(x as u32, y as u32, if edge { image::Rgb(mixed) } else { image::Rgb([200, 60, 80]) });
                mask[y * w + x] = 255;
            }
        }
        let mut mask2 = mask.clone();
        let changed = unmix_edge(&mut image, &mut mask2, w, h, bg, 2);
        assert!(changed, "混合像素应当被反混");
        let fixed = image.get_pixel(10, 15);
        // 反混之后应当接近 body（200,60,80），而不是混合色（100,118,72）
        assert!(
            (fixed[0] as i32 - 200).abs() < 25 && (fixed[1] as i32 - 60).abs() < 25,
            "反混后应当接近人物本体色，实测 {fixed:?}（混合色是 {mixed:?}）"
        );
        // 内部不碰
        assert_eq!(image.get_pixel(15, 15), &image::Rgb([200, 60, 80]).into());
    }

    /// **中间调的像素不许被删**（用户 2026-09-26："人的某些部分变透明了"）。
    ///
    /// 皮肤的阴影色恰好"介于人物本体色与背景色之间" —— 用第二个阈值去删（曾经把收边容差
    /// 开到 0.5）会把它们一起删掉，于是人身上一块块透出背景。现在的分工是：
    /// **只有覆盖率 a < 0.2 的才删**（那本来基本就是背景），其余一律反混修颜色。
    #[test]
    fn 中间调的皮肤像素不会被当成混合色删掉() {
        let (w, h) = (40usize, 40usize);
        let bg = [0u8, 177, 64];
        let mut image = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb(bg));
        let mut mask = vec![0u8; w * h];
        // 本体是深红，边界带里放"中间调的皮肤阴影"（介于本体与绿幕之间，但不是"基本是背景"）
        let skin_mid = image::Rgb([150, 120, 90]);
        for y in 10..30 {
            for x in 10..30 {
                let edge = x <= 12 || x >= 27 || y <= 12 || y >= 27;
                image.put_pixel(x as u32, y as u32, if edge { skin_mid } else { image::Rgb([200, 60, 80]) });
                mask[y * w + x] = 255;
            }
        }
        let before = mask.iter().filter(|v| **v > 127).count();
        let _ = unmix_edge(&mut image, &mut mask, w, h, bg, 3);
        let after = mask.iter().filter(|v| **v > 127).count();
        assert_eq!(mask[12 * w + 20], 255, "中间调的皮肤（覆盖率 > 0.2）不许被删");
        assert!(after as f64 > before as f64 * 0.95, "掩码不该被大幅削掉：{before} → {after}");
    }

    /// **去溢色**：绿幕边上那圈偏绿的像素要被压回中性，人物内部的像素**不许动**。
    #[test]
    fn 去溢色只压边缘的绿并且不动内部() {
        let (w, h) = (40usize, 40usize);
        // 人：中间 20x20 的红块，但最外一圈被绿幕"污染"成偏绿
        let mut person = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
        let mut mask = vec![0u8; w * h];
        for y in 10..30 {
            for x in 10..30 {
                let edge = x == 10 || x == 29 || y == 10 || y == 29;
                // 边缘：红 + 绿（典型溢色）；内部：纯红
                person.put_pixel(x as u32, y as u32, if edge { image::Rgb([200, 190, 60]) } else { image::Rgb([200, 40, 60]) });
                mask[y * w + x] = 255;
            }
        }
        let changed = despill_edges(&mut person, &mask, w, h, [0, 177, 64], 2);
        assert!(changed, "绿幕背景应当触发去溢色");
        let edge = person.get_pixel(10, 20);
        assert!(edge[1] < 150, "边缘的绿要被压下来，实测 {edge:?}");
        let inner = person.get_pixel(20, 20);
        assert_eq!((inner[0], inner[1], inner[2]), (200, 40, 60), "人物内部一个像素都不许动");
        // 白底（低饱和）没有"溢色通道"可压 → 一个像素都不动
        let mut white_bg = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
        for y in 10..30 {
            for x in 10..30 {
                let edge = x == 10 || x == 29 || y == 10 || y == 29;
                white_bg.put_pixel(x as u32, y as u32, if edge { image::Rgb([200, 190, 60]) } else { image::Rgb([200, 40, 60]) });
            }
        }
        assert!(!despill_edges(&mut white_bg, &mask, w, h, [252, 252, 250], 2), "白底不该触发去溢色");
        let untouched = white_bg.get_pixel(10, 20);
        assert_eq!((untouched[0], untouched[1], untouched[2]), (200, 190, 60), "白底那一档不许乱压");
    }

    /// **去溢色必须盖到掩码外面那一圈**（2026-09-27 实测出来的那条：总纲 §16.4）。
    ///
    /// 真正的溢色像素在剪影**外面**：主体与背景交界那圈抗锯齿像素是"人 + 背景"的混合色，
    /// 颜色更靠近背景 → 被判成了**背景**（不在掩码里）。只往内压 = 那条带里一个真正的
    /// 溢色像素都盖不到，剩下的正好是用户看到的"外轮廓一圈很淡的背景色"。
    #[test]
    fn 去溢色要盖到掩码外面那一圈() {
        let (w, h) = (40usize, 40usize);
        let bg = [0u8, 177, 64];
        // 底：一片绿幕
        let mut person = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb(bg));
        let mut mask = vec![0u8; w * h];
        // 人：中间 20x20 的纯红块
        for y in 10..30 {
            for x in 10..30 {
                person.put_pixel(x as u32, y as u32, image::Rgb([200, 40, 60]));
                mask[y * w + x] = 255;
            }
        }
        // 掩码**外面**紧挨着的那两圈 = "人 + 绿幕"的混合色（抗锯齿）：
        // 颜色离背景很近（所以颜色键把它判成背景），但绿分量仍然压过另外两个通道（所以该压）
        let mixed = image::Rgb([20u8, 170, 60]);
        for y in 6..34 {
            for x in 6..34 {
                if mask[y * w + x] == 0 && x >= 8 && x < 32 && y >= 8 && y < 32 {
                    person.put_pixel(x as u32, y as u32, mixed);
                }
            }
        }

        // ① 只往内（outer = 0）：外面那圈一个像素都盖不到 —— 这就是旧口径的失败模式
        let mut only_inner = person.clone();
        assert!(
            !despill_band(&mut only_inner, &mask, w, h, bg, 2, 0),
            "只往内压时，这个夹具上根本没什么可压的（正是问题的样子）"
        );
        assert_eq!(
            only_inner.get_pixel(8, 20),
            &mixed,
            "掩码外那圈溢色像素必须原样留着 —— 这就是那条淡背景色"
        );

        // ② 两侧都盖：外面那圈的绿被压下去，人物内部一个像素都不动
        let mut both = person.clone();
        assert!(despill_band(&mut both, &mask, w, h, bg, 2, 4), "盖到外面那一圈时应当真的改过像素");
        let fixed = both.get_pixel(8, 20);
        assert!(fixed[1] < 130, "掩码外那圈溢色要被压下来，实测 {fixed:?}");
        assert_eq!(
            both.get_pixel(20, 20),
            &image::Rgb([200, 40, 60]),
            "人物内部一个像素都不许动"
        );
        // 远处（超出外带）的绿幕不该被动 —— 免得整张背景被压成灰
        assert_eq!(both.get_pixel(35, 35), &image::Rgb(bg), "外带之外不许动");
    }

    /// **按背景打光 + 烧影子**：背景亮的一侧 = 光来的方向；影子落在**反方向**。
    #[test]
    fn 按背景估光向并烧出反向影子() {
        let (w, h) = (120usize, 120usize);
        // 背景：左暗右亮（光从右边来）
        let mut base = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([20, 20, 20]));
        for y in 0..h as u32 {
            for x in (w as u32 / 2)..w as u32 {
                base.put_pixel(x, y, image::Rgb([220, 210, 200]));
            }
        }
        // 人物：中间一个白方块（有像素）
        let mut person = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
        let mut mask = vec![0u8; w * h];
        for y in 50..70 {
            for x in 50..70 {
                person.put_pixel(x as u32, y as u32, image::Rgb([180, 180, 180]));
                mask[y * w + x] = 255;
            }
        }
        let (dir, estimated) =
            relight_and_shadow(&mut person, &mut mask, &base, w, h, None, 0.5, 0.5, 6);
        assert!(estimated, "没给方向时应当是从背景估的");
        assert!(dir[0] > 0.5, "光应当从**右**边来（右半更亮），实测 dir={dir:?}");

        // 亮面在右、暗面在左（方向梯度生效）
        let left = person.get_pixel(52, 60)[0] as i32;
        let right = person.get_pixel(67, 60)[0] as i32;
        assert!(right > left, "受光面应当更亮：左 {left} / 右 {right}");

        // 影子落在**反方向**（左边）：左边应当是"压暗后的背景色"且落进了遮罩
        let shadow_x = 46;
        assert_eq!(mask[60 * w + shadow_x], 255, "影子那几像素要落进遮罩（否则贴不上去）");
        let shadow_pixel = person.get_pixel(shadow_x as u32, 60)[0];
        // 左半背景是 20，压暗之后只会更暗
        assert!(shadow_pixel < 25, "影子应当是压暗后的背景色，实测 {shadow_pixel}");
    }

    /// 显式给方向就不去估（`light="left"` 之类）。
    #[test]
    fn 显式光向不再从背景估() {
        let (w, h) = (60usize, 60usize);
        let base = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([128, 128, 128]));
        let mut person = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([0, 0, 0]));
        let mut mask = vec![0u8; w * h];
        for y in 20..40 {
            for x in 20..40 {
                person.put_pixel(x as u32, y as u32, image::Rgb([200, 200, 200]));
                mask[y * w + x] = 255;
            }
        }
        let (dir, estimated) =
            relight_and_shadow(&mut person, &mut mask, &base, w, h, Some([0.0, -1.0]), 0.6, 0.0, 0);
        assert!(!estimated);
        assert_eq!(dir, [0.0, -1.0]);
        assert!(person.get_pixel(30, 22)[0] > person.get_pixel(30, 38)[0], "光从上面来 → 上面更亮");
    }
    /// **收缩（choke）**：把主体与背景交界那圈抗锯齿 / 溢色扔掉 ——
    /// 用户 2026-09-26 问的"人物怎么冒背景的泛光"就是没收这一刀（绿边被当主体贴进去）。
    #[test]
    fn 收缩把交界那圈溢色扔掉() {
        let mut mask = vec![0u8; 100 * 100];
        for y in 20..80 {
            for x in 20..80 {
                mask[y * 100 + x] = 255;
            }
        }
        let eroded = erode_mask(&mask, 100, 100, 2);
        assert_eq!(eroded[50 * 100 + 50], 255, "内部保留");
        assert_eq!(eroded[20 * 100 + 50], 0, "原来那条边（y=20）被扔掉");
        assert_eq!(eroded[21 * 100 + 50], 0);
        assert_eq!(eroded[22 * 100 + 50], 255, "往里收 2 像素");
        assert_eq!(eroded[79 * 100 + 79], 0, "右下角也收");
        // radius=0 不动
        assert_eq!(erode_mask(&mask, 100, 100, 0), mask);
        // 收没了就是空的（别 panic）
        let tiny = erode_mask(&mask, 100, 100, 40);
        assert!(tiny.iter().all(|v| *v == 0));
    }

    /// **摆放**（用户 2026-09-26 要的能力）：缩放到一半、搬到左下角 ——
    /// 遮罩与人物像素必须落在**同一个**位置（只动一个就是「人在这儿、遮罩在那儿」的错位）。
    #[test]
    fn 摆放要缩放并搬到位且图与遮罩不脱节() {
        let mut img = image::RgbImage::from_pixel(400, 400, image::Rgb([250, 250, 250]));
        for y in 100..300 {
            for x in 150..250 {
                img.put_pixel(x, y, image::Rgb([200, 30, 40]));
            }
        }
        let (mask, _, _) =
            subject_mask_from_image(&img, 400, 400, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        let bbox = mask_bbox(&mask, 400, 400).unwrap();
        assert_eq!(bbox, (150, 100, 250, 300));

        let (placed_img, placed_mask) = place_patch(&img, &mask, 400, 400, bbox, 0.5, 0.25, 0.75);
        let new_box = mask_bbox(&placed_mask, 400, 400).unwrap();
        let (cx, cy) = ((new_box.0 + new_box.2) / 2, (new_box.1 + new_box.3) / 2);
        assert_eq!((cx, cy), (100, 300), "中心要搬到 (0.25,0.75) x 400");
        assert_eq!((new_box.2 - new_box.0, new_box.3 - new_box.1), (50, 100), "宽高各缩一半");

        // 图与遮罩不脱节
        let pixel = *placed_img.get_pixel(cx as u32, cy as u32);
        assert!(pixel[0] > 150 && pixel[1] < 80, "新位置上的像素应当是人物（红），实测 {pixel:?}");
        assert_eq!(placed_mask[cy * 400 + cx], 255);
        assert_eq!(placed_mask[200 * 400 + 200], 0, "原位置必须清掉（否则老地方留个鬼影）");
        let old = *placed_img.get_pixel(200, 200);
        assert!(old[0] < 40 && old[1] < 40, "原位置的像素也要清掉，实测 {old:?}");
    }

    /// `ring`：**只在人物外面一圈**重绘（人物自己的像素一个都不动）。
    ///
    /// 2026-09-26 用户问的"嵌进去会改变画风怎么解决"的正解：自带工作流一个遮罩两用，
    /// 于是贴上去的人必然被模型重画一遍（denoise 0.5 就把她的画风换掉了）。
    #[test]
    fn 环形遮罩只圈人物外面一圈() {
        let mut img = image::RgbImage::from_pixel(200, 200, image::Rgb([252, 252, 250]));
        for y in 60..140 {
            for x in 80..120 {
                img.put_pixel(x, y, image::Rgb([200, 30, 40]));
            }
        }
        let (mask, _, _) =
            subject_mask_from_image(&img, 200, 200, SUBJECT_TOLERANCE_DEFAULT, &[0.0, 0.0, 1.0, 1.0], false, None).unwrap();
        assert_eq!(mask[100 * 200 + 100], 255);

        let ring = ring_mask(&mask, 200, 200, 5);
        // 人物自己（含边界）**一个像素都不许在重绘区里**
        for y in 60..140 {
            for x in 80..120 {
                assert_eq!(ring[y * 200 + x], 0, "人物内部不该被重绘（({x},{y})）");
            }
        }
        // 外面一圈 5 像素之内是白的（要重绘），再远就不是
        assert_eq!(ring[100 * 200 + (80 - 3)], 255, "人物左边 3 像素处应当在环里");
        assert_eq!(ring[(60 - 3) * 200 + 100], 255, "人物上方 3 像素处应当在环里");
        assert_eq!(ring[100 * 200 + (80 - 20)], 0, "离得太远的背景不该被重绘");
        assert_eq!(ring[10 * 200 + 10], 0);
        // 半径 0 = 原样返回（不生成环）
        assert_eq!(ring_mask(&mask, 200, 200, 0), mask);
    }

    #[test]
    fn 只取文件名挡目录穿越() {        assert_eq!(file_name_only(r"..\..\windows\system32\x.png"), "x.png");
        assert_eq!(file_name_only("/etc/passwd"), "passwd");
        assert_eq!(file_name_only("  bg.png  "), "bg.png");
    }

    /// 两处都没有 → **如实报 `COMFY_NOT_FOUND`**（这一支在开发机上走不到：
    /// `find_home` 一定探得到本机的 ComfyUI，而它恰恰是新用户第一次用时会撞上的那一支）。
    #[test]
    fn 找不到_comfyui_时如实报错() {
        let failure = input_dir_from(None, None).unwrap_err();
        assert_eq!(failure.code, "COMFY_NOT_FOUND");
        assert!(failure.message.contains("设置"), "要告诉用户去哪儿确认：{}", failure.message);
    }

    /// 配置的输出目录优先；它推不出 `input` 时才回落到探测到的 home。
    #[test]
    fn 配置的输出目录优先于探测到的_home() {
        let root = std::env::temp_dir().join(format!("comfyhub-input-{}", std::process::id()));
        let configured = root.join("shared");
        let probed = root.join("probed");
        for dir in [configured.join("input"), probed.join("input")] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let from_config = input_dir_from(Some(&configured.join("output").display().to_string()), Some(probed.clone()))
            .unwrap();
        assert_eq!(from_config, configured.join("input"), "配置里已经指明了，就不该用探测结果");

        // 配置给的不是 `output` 目录本身（用户可能直接填了父目录）→ 也在它下面找 input
        let from_parent = input_dir_from(Some(&configured.display().to_string()), Some(probed.clone())).unwrap();
        assert_eq!(from_parent, configured.join("input"));

        // 配置指向一个**没有 input 的目录** → 回落到探测结果
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let fallback =
            input_dir_from(Some(&empty.display().to_string()), Some(probed.clone())).unwrap();
        assert_eq!(fallback, probed.join("input"));

        // 空白字符串当没配
        let blank = input_dir_from(Some("   "), Some(probed.clone())).unwrap();
        assert_eq!(blank, probed.join("input"));

        std::fs::remove_dir_all(&root).ok();
    }
}
