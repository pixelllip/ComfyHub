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

    let mut pixels = draw_mask(w as usize, h as usize, &region, &shape);
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
        .unwrap_or_else(|| format!("comfyhub_mask_{}.png", content_tag(w, h, &region, &shape, invert, feather)));
    let target = input_dir.join(&filename);
    write_mask_png(&pixels, w as u32, h as u32, &target)?;

    let region_joined = region.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
    Ok(json!({
        "filename": filename,
        "inputDir": input_dir.to_string_lossy(),
        "width": w,
        "height": h,
        "region": region_text,
        "regionNormalized": format!("[{region_joined}]"),
        "shape": shape,
        "invert": invert,
        "feather": feather,
        "whiteMeans": "要重新生成的地方（重绘区）",
        // 四个预设都是"粗框"，只有显式给坐标才不是估的（与 Kotlin 同一条判据）
        "approximate": region_text != "full" && !region_text.contains(','),
        "hint": format!(
            "① 覆盖工作流里 LoadImageMask 节点的 image 输入：{{\"<节点id>.image\":\"{filename}\"}}；\
             ② 若工作流有 ImageScale，把 width/height 也覆盖成 {w}x{h}（必须与底图一致）；\
             ③ 遮罩不准就报一组新的 region（x,y,w,h 归一化）重做 —— 不要让用户去 ComfyUI 里手画。"
        ),
    }))
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

/// 画遮罩：黑底 + 白色区域（0 = 保留，255 = 重绘）。
///
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

    #[test]
    fn 只取文件名挡目录穿越() {
        assert_eq!(file_name_only(r"..\..\windows\system32\x.png"), "x.png");
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
