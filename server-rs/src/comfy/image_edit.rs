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
//! 2. **给局部重绘画一张遮罩**（[`make_mask`]）—— 局部重绘必须有遮罩，而用户在手机 / 平板上
//!    画不出来，ComfyUI 的 MaskEditor 又要人坐在电脑前手描。这里用"归一化矩形 / 椭圆 + 反选
//!    + 羽化"表达意图，够用且可复现。
//!
//! # 贴人物图那条路：**抠图不是我们的事**（2026-09-27 定）
//!
//! `mode="subject"` 只做三件事：确认人物图在 `input` 里、按颜色**估一个定位器剪影**
//! （算 bbox / 支撑摆放 / 报诊断）、把 `personFilename` 回给调用方。真正的合成 alpha 由
//! **工作流里的抠图节点**现算（`easy imageRemBg` + `refine_foreground=true`；仓库不再随包带
//! 模板工作流，这份链在用户自己的贴人工作流里，节点 id 从 `apiGraph` 现读）。
//!
//! ⛔ 以前还有一档"用颜色剪影**当合成遮罩** + Rust 收边/反混/去溢色/现算 ring"（`matteSource="color"`）。
//! 活实例实测：贴出来的人物**轮廓外一圈青绿**（剪影外那条带 39.6% 像素偏绿，比底图多 +34.8pp）、
//! **发梢被整段吃掉**（AI 软遮罩那一档是 2.39% / −2.5pp）。根因是 Rust 手里没有模型的 alpha，
//! 只能按颜色猜 —— 那一档连同补救算法一起删除了（传老参数会被明确拒绝，见 [`REMOVED_ARGS`]）。
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
//! | [`resolve_input_dir`] / [`target_input_dir`] | `AiWorkflowSearch.resolveComfyInputDir` |
//! | [`input_filename_for`] | `AiWorkflowSearch.inputFilenameFor` |
//! | [`use_attachment`] | `AiWorkflowSearch.useAttachment` |
//! | [`use_media`] | `AiImageEdit.useMedia` |
//! | [`make_mask`] | `AiImageEdit.makeMask`（含 `parseRegion` / `boxBlur` / `contentTag`） |
//!
//! 逐条对照的**判据**是同一组参数得到的文件名与遮罩像素：文件名靠
//! `<原名>-<id 前 8 位><扩展名>` 这套命名规则（重复调用得到同一个文件，
//! 重跑工作流不会指向一个已经不在的图），遮罩靠"白区在哪"与羽化半径。
//! 单测盯的就是这两件事（`make_mask_白区与羽化` / `投放文件名可重复且不互相覆盖`）。
//! ⚠ `mode="subject"` **不在这条对照里**：Kotlin 那一版是颜色抠图当遮罩，行为已经分叉
//! （它那边还在写遮罩）—— 这一档是**有意**分叉的（R12 的分叉要有明确的记录）。
//!
//! ⚠ **input 目录的判据也已经分叉，而且是有意的**（2026-09-30）：Kotlin 只按"配置的
//! 输出目录推兄弟目录"，而真机上实例读哪一份由**起实例时挂的参数**决定（Desktop 的
//! `settings.json`、或调用方自己写的 `--input`）。所以这里多两层：**我们起实例时的留档**
//! 与**直接问实例**（[`target_input_dir`]）。写错的表现是提交时 `400 Invalid image file`，
//! 那比"两端不一致"更值得优先防（现场与因果链见 [`input_dir_candidates`]）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::web::routes::AppState;
use crate::tools::tools::ToolFailure;

/// 遮罩边长上限（与产物同一量级；再大就是误传了）。
pub const MAX_MASK_EDGE: i64 = 8192;

/// 羽化上限（像素）。
pub const MAX_FEATHER: i64 = 256;

/// 老参数一律**明确拒绝**（见 [`REMOVED_ARGS`]）。
///
/// 抽成纯函数是为了能单测；`make_mask` 在**最前面**调它（先于 ComfyUI 探测）：
/// 参数写错了就当场报，别等去找实例。
fn reject_removed_args(args: &Value) -> Result<(), ToolFailure> {
    for removed in REMOVED_ARGS {
        if args.get(removed).map(|value| !value.is_null()).unwrap_or(false) {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!(
                    "参数 `{removed}` 已删除（2026-09-27：颜色抠图那一档整档删掉了，抠图交给工作流）。\
                     贴人物图用 mode=\"subject\" + subjectFilename，再照返回的 hint 用 \
                     **用户自己的贴人工作流**（含 `easy imageRemBg` + `refine_foreground=true`）提交；\
                     实例上没有抠图模型时先指引用户下载 \
                     （`comfy_get_status` 的 `mattingModels.usable`）。"
                ),
            ));
        }
    }
    Ok(())
}

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
/// ⚠ 它现在只服务**定位器**的质量提示（`chromaKey` / `chromaAdvice`），不再参与任何画面像素。
pub const CHROMA_MIN_SATURATION: i32 = 40;

/// **已删除那一档的参数名**（2026-09-27：颜色抠图当合成遮罩，整档删除）。
///
/// 传它们会被**明确拒绝**而不是静默忽略 —— 静默忽略更坏：模型按旧描述传
/// `matteSource="model"`、以为遮罩是自己写的，实际拿到的是"不写遮罩"的语义，
/// 它就会去覆盖一个不存在的 `LoadImageMask`。
pub const REMOVED_ARGS: &[&str] = &[
    // 那一档的开关
    "matteSource",
    // "拿猜出来的剪影去合成"才需要的补救
    "ring",
    "shrink",
    "edgeBand",
    "edgeTolerance",
    // 打光那一档（默认 `none`，观感从未合格）
    "light",
    "lightStrength",
    "shadowStrength",
    "shadowOffset",
];

// ---------------------------------------------------------------------------
//  ComfyUI 的 input 目录
// ---------------------------------------------------------------------------

/// 直写 `input` 目录失败时**攒下来的投递任务**（2026-09-28 用户 bug
/// 「comfy_use_attachment 总是失败」；2026-09-29 补上"摆放后的人物图"那一处）。
///
/// 现场：真机上直写 `D:\Comfy-Desktop\ComfyUI-Shared\input` 一律
/// `拒绝访问 (os error 5)` —— **不是路径配错、也不是 ACL**，是"映像在工作区内的进程写不了
/// 工作区外"（见 [`crate::infra::confinement`]，2026-09-29 逐条量过）；而**同一次会话里
/// ComfyUI 自己写得进去** —— MCP 的 `upload_file` 走 `/upload/image` 就成功了。
/// 所以：**直写被拒就改让 ComfyUI 自己写**（HTTP），那条路才是永远成立的。
///
/// 为什么做成"攒下来"而不是"当场报错"：这几个工具的业务函数是 **sync** 的
/// （要能脱离运行时单测），而 HTTP 那条路是 async。所以 sync 侧把内容攒在这里、
/// 照常把结果对象返回给模型，async 的工具入口再兑现它 —— 兑现失败才如实报错。
///
/// ⚠ **攒的是"若干份"，不是一份**：`comfy_make_mask` 一次调用可能同时要写
/// 摆放后的人物图与遮罩（今天主体档不写遮罩，但那是**语义**决定的，不是结构决定的 ——
/// 只留一个槽位就会静默丢掉后一份，那正是 2026-09-29 用户报的"没拿到 personFilename"）。
#[derive(Default)]
pub struct Delivery {
    /// ComfyUI 的 input 目录（算 subfolder 用）
    input_dir: PathBuf,
    /// (目标位置, 内容, 直写被拒的原因)：要写进去的路径 + 字节 + 真实系统错误
    pending: Vec<(PathBuf, Vec<u8>, String)>,
}

impl Delivery {
    /// 绑定这次投放的目标 input 目录（算 subfolder 用）。
    pub fn for_input_dir(input_dir: &Path) -> Self {
        Delivery { input_dir: input_dir.to_path_buf(), pending: Vec::new() }
    }

    /// 记下一次"直写没成功、改走 ComfyUI"的投递。
    ///
    /// `why` 是**系统给的原话**（`拒绝访问。 (os error 5)`）—— §12 的教训就是
    /// "只报哪个目录不报报什么错"会把处方带歪，所以那一半必须一路带到文案里。
    fn defer(&mut self, target: &Path, bytes: Vec<u8>, why: String) {
        self.pending.push((target.to_path_buf(), bytes, why));
    }

    /// 目标相对 input 目录的子目录（`pasted/mask.png` → `pasted`）。
    fn subfolder_of(&self, target: &Path) -> String {
        target
            .parent()
            .and_then(|parent| parent.strip_prefix(&self.input_dir).ok())
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 把攒下的每一份交给 ComfyUI 自己写（`/upload/image`），
    /// 返回 `[(我们算的文件名, ComfyUI 那边真实认下的文件名)]`。
    ///
    /// 没攒下东西就 `Ok(vec![])`（说明直写本来就成功了）。
    ///
    /// ⚠ **必须按 ComfyUI 给的名单回填结果对象**（见 [`apply_uploaded_names`]）：
    /// 工作流里 `LoadImage.image` 填的是**它认得的那个名字**，不是我们以为写进去的那个。
    pub async fn flush(&mut self, state: &AppState) -> Result<Vec<(String, String)>, ToolFailure> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let queued = std::mem::take(&mut self.pending);
        let config = crate::comfy::capture::capture_config(state).map_err(|e| {
            ToolFailure::new("COMFY_NOT_FOUND", format!("读不到 ComfyUI 配置：{}", e.human()))
        })?;
        let mut delivered: Vec<(String, String)> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for (target, bytes, why) in queued {
            let filename = target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "image.png".to_string());
            let subfolder = self.subfolder_of(&target);
            let uploaded = crate::comfy::comfy_client::upload_image(
                &config.comfy_url,
                &filename,
                &subfolder,
                &bytes,
            )
            .await;
            match uploaded {
                // ComfyUI 那边同名会**覆盖**（我们传的是 overwrite=true），所以正常情况下
                // 两边同名；但仍以它回的名字为准 —— 它才是 LoadImage 认得的那一串。
                Ok(name) => delivered.push((filename, name)),
                Err(reason) => failed.push(format!(
                    "{}（直写被系统拒绝：{why}；改让 ComfyUI 自己收也没成功：{reason}）",
                    target.display()
                )),
            }
        }
        if !failed.is_empty() {
            return Err(ToolFailure::new(
                "COMFY_INPUT_FAILED",
                format!(
                    "放不进 ComfyUI 的 input 目录：{}\n\
                     先确认 ComfyUI 还在跑、`comfy_get_status` 能连上；如果它也连不上，那多半是**这个后端\
                     进程自己写不进项目外**（真因是完整性标签：工作区产物带着 Low 标签，Windows 取 \
                     **min(父进程, 映像)** → 进程令牌是 Low，写不了工作区外的 Medium 对象 —— \
                     与路径、目录 ACL、用户身份、Defender 都无关）。修法三选一：\n\
                     1. `pwsh -File scripts\\pack-release.ps1 -OutDir D:\\dist\\ComfyHub`，\
                     然后运行 `D:\\dist\\ComfyHub\\viewer.exe`（App 自己拉起 backend\\ 里的后端）；\n\
                     2. 已有工作区外的构建产物时，设 `$env:COMFYHUB_SERVER_EXE` 指向那个 exe；\n\
                     3. `pwsh -File scripts\\backend.ps1 start`（起之前把 Low 标签修回 Medium；\
                     **当前会话本身是 Low 时修不动**，只剩 1/2 两条路）。",
                    failed.join("；")
                ),
            ));
        }
        Ok(delivered)
    }
}

/// 把 ComfyUI 那边真实认下的文件名**回填**到工具结果里（模型照着它去覆盖 `LoadImage`）。
///
/// 三个字段都可能装着"我们打算写进去的那个名字"，所以逐个比对：
/// `filename`（遮罩 / 投放的原图）、`personFilename` 与 `placedSubjectFilename`（贴人物档）。
pub fn apply_uploaded_names(result: &mut Value, delivered: &[(String, String)]) {
    if delivered.is_empty() {
        return;
    }
    result["channel"] = json!("comfy-upload");
    for key in ["filename", "personFilename", "placedSubjectFilename"] {
        let current = result.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        if let Some((_, remote)) = delivered.iter().find(|(local, _)| local == &current) {
            result[key] = json!(remote);
        }
    }
}

/// 写一个文件到 ComfyUI 的 input 目录：**直写被系统拒绝就改让 ComfyUI 自己写**。
///
/// 三个调用点（投放原图 / 摆好的人物图 / 遮罩）**必须都走这里** ——
/// 2026-09-29 用户报的"写停放好的人物图失败：拒绝访问 (os error 5)"就是第三个调用点
/// 当场报错、没走退路，害得整条阶段二停在"没拿到 personFilename"。
fn write_or_defer(
    target: &Path,
    bytes: Vec<u8>,
    delivery: &mut Delivery,
) -> Result<(), ToolFailure> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    match std::fs::write(target, &bytes) {
        Ok(()) => Ok(()),
        Err(error) => {
            delivery.defer(target, bytes, format!("{error}"));
            Ok(())
        }
    }
}

/// ComfyUI 的 `input` 目录：**投放 / 遮罩 / 摆放三处共用同一个真源**。
///
/// # 为什么不能只按"配置的输出目录"推（2026-09-30 用户 bug）
///
/// 这份目录必须**与实例真正在读的那一份是同一个**，否则工作流里的 `LoadImage` 找不到文件 ——
/// 提交时 `400 Invalid image file`，看着像"文件不存在"，其实是"两边读写的不是同一个目录"。
///
/// 而"实例读哪一份"是**起实例时**定下来的，可能来自两处：
///   * 调用方自己写的 `--input` / `--input-directory`（`comfy_start` 的 `extraArgs`）；
///   * ComfyHub 照 ComfyUI Desktop 的 `settings.json` 补的那条 `--input-directory`。
///
/// 原来这里只按"配置里 outputDir 的兄弟目录"推，于是发布包（`capture.config` 里**根本没有
/// outputDir**）落到 `find_home` 探到的**安装目录**那份 input，而实例挂的是 Desktop 配的
/// **共享**那份：底图与摆好的人物图都躺在安装目录的 input 里，提交必 400。
///
/// # 判据（**顺序即优先级**，第一个真实存在的目录就是投放目标）
///
/// 1. **我们上次起实例时挂的那份**（`.run\comfyui-<端口>.launch.json` 的留档）——
///    最硬的证据：那个实例读的就是它（见 [`crate::comfy::comfy_launch::write_launch_record`]）；
/// 2. 配置里生效的 `outputDir` 的兄弟 `input`（用户在「ComfyUI 自动捕获」里显式指的那份）；
/// 3. **ComfyUI Desktop 的 `settings.json` 的 `inputDir`** —— 我们起实例时补的就是它；
/// 4. `find_home` 探到的 `input`（便携包 / 整合包那种"程序与数据在一处"的布局）。
///
/// ⚠ 实例**答话**时它排在上面全部之前：那是"它到底读哪儿"的唯一权威答案
/// （见 [`target_input_dir`] 的探测）。上面四条只服务"问不到"的时候。
pub fn input_dir_candidates(state: &AppState) -> Vec<(PathBuf, &'static str)> {
    let config = crate::comfy::capture::capture_config(state).ok();
    let configured = config
        .as_ref()
        .and_then(|config| config.output_dir.clone())
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    let desktop_input = crate::tools::mcp::desktop(None).and_then(|d| d.input_dir);
    let comfy_url = config
        .as_ref()
        .map(|config| config.comfy_url.clone())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8188".to_string());
    let port = crate::comfy::comfy_launch::port_of(&comfy_url);
    let launched = crate::comfy::comfy_launch::launched_input_dir(&state.project_root(), port);
    let home_input = crate::comfy::comfy_dirs::find_home(&state.project_root(), configured.as_deref())
        .map(|home| home.join("input"));

    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    for (dir, source) in [
        (launched, "launch-record"),
        (sibling_input_of(configured.as_deref()), "capture-output"),
        (desktop_input, "desktop-config"),
        (home_input, "probe"),
    ] {
        let Some(dir) = dir else { continue };
        // 不存在的一律不给（免得投放跑到一个凭空拼出来的目录里）；同一个目录只留第一个来路
        if !dir.is_dir() || out.iter().any(|(kept, _)| paths_equal(kept, &dir)) {
            continue;
        }
        out.push((dir, source));
    }
    out
}

/// 两份路径是不是同一个目录（只做字面比较 + 大小写不敏感 —— Windows 上够了，
/// 也避免为了一次比较去 `canonicalize` 而引入 IO 失败分支）。
fn paths_equal(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// 配置的输出目录 → 它的兄弟 `input` 目录。
///
/// 配置里给的**可能就是 input 本身**（老版本设置页允许直接填 input 目录），那就原样用它；
/// 名字是 `output` 就取它的父目录再拼 `input`（ComfyUI 的固定布局）。
fn sibling_input_of(configured: Option<&str>) -> Option<PathBuf> {
    let dir = configured.map(str::trim).filter(|dir| !dir.is_empty())?;
    let path = PathBuf::from(dir);
    let is_output = path
        .file_name()
        .map(|name| name.to_string_lossy().eq_ignore_ascii_case("output"))
        .unwrap_or(false);
    if is_output {
        path.parent().map(|base| base.join("input"))
    } else {
        Some(path)
    }
}

/// ComfyUI 的 `input` 目录（**只按候选顺序取，不问实例** —— 同步路径用）。
///
/// 顺序见 [`input_dir_candidates`]（留档 → 配置 output 兄弟 → Desktop 配置 → 盘探）。
/// 探测**只读**：一个都不存在就如实报错，**绝不在随机位置建目录**。
///
/// ⚠ 真正调工具的那条路走 [`target_input_dir`]（它会先问实例），这个函数是它的兜底与单测入口。
pub fn resolve_input_dir(state: &AppState) -> Result<PathBuf, ToolFailure> {
    input_dir_from(input_dir_candidates(state))
}

/// [`resolve_input_dir`] 的**纯函数那一半**：按顺序取第一个**真实存在**的目录，都没有就如实报错。
///
/// 拆出来是为了能测"一处都没有"那一支：本机（开发机）装着 ComfyUI，
/// `find_home` 一定探得到，于是"找不到"这条分支在集成用例里**永远走不到**
/// —— 而它恰恰是新用户第一次用时会撞上的那一支（发布包里没有 ComfyUI 目录）。
///
/// ⚠ 存在性**在这里也要判一遍**（[`input_dir_candidates`] 已经滤过）：这条函数是
/// "按顺序取第一个可用的"这个契约本身，调用方（含用例）不该因为传进来一个不存在的路径
/// 就拿到一个凭空拼出来的目录 —— 那正是"投进一个实例根本不读的目录"的开头。
fn input_dir_from(candidates: Vec<(PathBuf, &'static str)>) -> Result<PathBuf, ToolFailure> {
    if let Some((dir, _)) = candidates.into_iter().find(|(dir, _)| dir.is_dir()) {
        return Ok(dir);
    }
    Err(ToolFailure::new(
        "COMFY_NOT_FOUND",
        "没找到 ComfyUI 的 input 目录（投放的图与遮罩必须放在那里，LoadImage / LoadImageMask 才读得到）。\
         让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置后再试。",
    ))
}

/// 投放的目标目录 + **它是怎么定下来的**（给模型看的那一句）。
#[derive(Debug, Clone)]
pub struct TargetInputDir {
    pub dir: PathBuf,
    /// 来路：`instance`（问了实例）/ `launch-record` / `capture-output` / `desktop-config` / `probe`
    pub source: &'static str,
    /// 需要如实说明的一句（"实例读的不是我们推的那个"这种）
    pub note: Option<String>,
}

/// **投放工具真正用的那一个**：先问实例，问不到再按候选顺序。
///
/// 为什么非问不可：`--input` / `--input-directory` 两处都可能给实例赋值（argparse 后写的赢），
/// 光看配置推不出"它到底读哪儿"。实例自己报的 `/object_info/LoadImage` 名单是唯一权威答案 ——
/// 拿它去比对候选目录，**覆盖面最高**的那个就是它的 input（见 [`crate::comfy::comfy_client::dir_owning`]）。
pub async fn target_input_dir(state: &AppState) -> Result<TargetInputDir, ToolFailure> {
    let candidates = input_dir_candidates(state);
    let comfy_url = crate::comfy::capture::capture_config(state).map(|c| c.comfy_url).unwrap_or_default();
    let dirs: Vec<PathBuf> = candidates.iter().map(|(dir, _)| dir.clone()).collect();
    let probed = crate::comfy::comfy_client::instance_input_dir(&comfy_url, &dirs).await;
    choose_input_dir(&candidates, probed)
}

/// [`target_input_dir`] 的**纯函数那一半**（问实例的那一步的结果由调用方注入，可单测）。
///
/// 判据：**实例答话就是终点**（它是唯一权威）；它与我们推的第一名不一致时**必须说出来**——
/// 那正是"配置与实际读的不是同一份"的现场（2026-09-30 用户 bug），用户与模型都该看见。
fn choose_input_dir(
    candidates: &[(PathBuf, &'static str)],
    probed: Option<PathBuf>,
) -> Result<TargetInputDir, ToolFailure> {
    if let Some(probed) = probed {
        let guessed = candidates.iter().find(|(dir, _)| dir.is_dir()).map(|(dir, _)| dir.clone());
        let note = match &guessed {
            Some(first) if !paths_equal(first, &probed) => Some(format!(
                "实例报出来的 input 是 {}，而我们按配置推出来的是 {} —— **以实例为准**\
                 （两边不一致时写错那一份的表现就是提交时 400 `Invalid image file`）。\
                 建议让用户在「设置 → ComfyUI 自动捕获」里把输出目录改成实例真正在用的那一处，\
                 免得每次都要靠探测。",
                probed.display(),
                first.display()
            )),
            _ => None,
        };
        return Ok(TargetInputDir { dir: probed, source: "instance", note });
    }

    let (dir, source) = candidates
        .iter()
        .find(|(dir, _)| dir.is_dir())
        .map(|(dir, source)| (dir.clone(), *source))
        .ok_or_else(|| {
            ToolFailure::new(
                "COMFY_NOT_FOUND",
                "没找到 ComfyUI 的 input 目录（投放的图与遮罩必须放在那里，LoadImage / LoadImageMask 才读得到）。\
                 让用户在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 位置后再试。",
            )
        })?;
    Ok(TargetInputDir { dir, source, note: None })
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
///
/// ⚠ 直写失败**不再当场报错**（2026-09-28）：真机上这条路本来就是被系统拒的，
/// 记进 [Delivery] 交给 ComfyUI 自己写（`/upload/image`）—— 那条路才是永远成立的。
fn copy_if_needed(source: &Path, target: &Path, delivery: &mut Delivery) -> Result<(), ToolFailure> {
    let bytes = std::fs::read(source).map_err(|e| {
        ToolFailure::new(
            "COMFY_INPUT_FAILED",
            format!("读不出要投放的原件（{}）：{}", source.display(), take_text(&e.to_string(), 200)),
        )
    })?;
    let same = match (std::fs::metadata(target), std::fs::metadata(source)) {
        (Ok(to), Ok(from)) => to.len() == from.len(),
        _ => false,
    };
    if !same {
        write_or_defer(target, bytes, delivery)?;
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
/// ⚠ `input_dir` **由工具入口算好传进来**（[`target_input_dir`]），不在这里自己推：
/// "实例读哪个 input"与"往哪个 input 投"必须是同一个答案（2026-09-30 那个 400 的教训）。
///
/// 返回 `{mediaId, filename, inputDir, width?, height?, hint}`：`width/height` 是**原图尺寸**，
/// 做遮罩时要照它给（遮罩与底图尺寸不一致 = 采样直接炸）。解不出来就不给这两个键，
/// **但不拦**（模型可以显式给 width/height）。
pub fn use_media(
    state: &AppState,
    media_id: i64,
    wanted_name: Option<&str>,
    input_dir: &Path,
    delivery: &mut Delivery,
) -> Result<Value, ToolFailure> {
    let media = crate::media::media::get(state, media_id).map_err(|_| {
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
    let source = crate::media::media::resolve_media(state.media_dir(), &media.stored_name).ok_or_else(|| {
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
    let filename = input_filename_for(&media.original_name, &format!("m{media_id}"), wanted_name);
    copy_if_needed(&source, &input_dir.join(&filename), delivery)?;

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
    input_dir: &Path,
    delivery: &mut Delivery,
) -> Result<Value, ToolFailure> {
    let dto = crate::media::attachments::get(state, attachment_id)
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
    let source = crate::media::attachments::file_of(state, attachment_id)
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
    let filename = input_filename_for(&dto.name, attachment_id, wanted_name);
    copy_if_needed(&source, &input_dir.join(&filename), delivery)?;

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
/// 两条用途（**2026-09-27 起只剩这两条**）：
///  - `mode="rect" / "ellipse" / "full"`：照 `region` 画一张**几何**遮罩并写盘 —— 纯提示词重绘
///    （换背景 / 只改一块地方）用它；
///  - `mode="subject"`：贴人物图。这一档**不写遮罩**：合成用的 alpha 由工作流里的抠图节点现算
///    （`easy imageRemBg` + `refine_foreground=true`；这份链在**用户自己的贴人工作流**里，
///    仓库不再随包带模板，id 从 `apiGraph` 现读），本工具只回
///    `personFilename`（拿它覆盖工作流里人物那个 `LoadImage.image`）。颜色剪影**只剩定位器用途**
///    （算 bbox 供摆放、报 `subjectRatio` / `background` 供诊断），不参与画面。
///
/// ⛔ **"颜色抠图当合成遮罩"那一档已经删除**（2026-09-27 用户拍板"放弃纯rust编辑，删除它"）。
/// 活实例实测（768×1024 绿幕人物 + 夜景）：那样贴出来的人物**轮廓外面一圈青绿**
/// （剪影外那条带 39.6% 像素偏绿，比底图自己多 **+34.8pp**；AI 软遮罩那一档是 2.39% /
/// −2.5pp），而且**发梢被整段吃掉**（草从头发里透出来，就是当初登记的"丢 18.4% 前景"）。
/// 根因是 **Rust 手里没有模型的 alpha**，只能按颜色猜覆盖率 —— 猜出来的边界永远差一档，
/// 调参救不了（三次尝试更差的记录见 pitfalls）。所以那一档连同它的补救算法一起删掉：
/// `drop_blended_edge`（按颜色收边）/ `erode_mask`（choke）/ `unmix_edge`（反混）/
/// `despill_edges`（去溢色）/ `ring_mask`（Rust 现算的 ring）/ `relight_and_shadow`（打光，观感从未合格）。
/// 传这些老参数（以及 `matteSource`）会被**明确拒绝**而不是静默忽略。
///
/// 参数（类型一律容错，与 Kotlin 的 `str` / `int` / `bool` 同义）：
///  - `mode`：`rect`（默认，按框画）/ `subject`（贴人物图，抠图交给工作流）；
///  - `subjectFilename`：`mode="subject"` 时的**人物图**（input 目录里的裸文件名）——
///    它就是 `20.image` 要填的那张，也是定位器的输入；
///  - `tolerance`：`mode="subject"` 定位器的背景色容差（默认 0.12，越大越激进）；
///  - `bgColor`：定位器的背景色提示（`green` / `magenta` / `#00b140`；不给就从四周估）——
///    只影响定位器（bbox / `subjectRatio`），**不影响画面**；
///  - `baseFilename`：input 目录里的底图文件名 —— 给了就用**它的尺寸**（推荐，最不容易错）；
///  - `width` / `height`：没给 `baseFilename` 时的显式尺寸；
///  - `region`：`full`（默认）/ `x,y,w,h`（归一化 0~1）/ 预设 `left|right|top|bottom|center|person`；
///  - `shape`：`rect`（默认）/ `ellipse`；
///  - `invert`：反选（**换背景保人物**就是 `invert=true` + 人物那块 region）；
///  - `feather`：边缘羽化像素（默认 0；贴人物进新背景时 8~24 会明显更自然）；
///  - `scale` / `x` / `y`：**摆放**（把人物缩放并搬到归一化坐标；人物像素与定位掩码一起搬）；
///  - `filename`：想固定的文件名（不给自己按内容算一个，重复调用得到同一个文件）。
pub fn make_mask(state: &AppState, args: &Value) -> Result<Value, ToolFailure> {
    // 单测 / 老调用点：直写失败也不攒投递（那几个用例本来就在能写的机器上跑）
    let input_dir = resolve_input_dir(state)?;
    make_mask_with(args, &input_dir, &mut Delivery::default())
}

/// [`make_mask`] 的真实实现：多一个 [Delivery]，好让"直写被拒"的那份遮罩
/// 由 async 的工具入口交给 ComfyUI 自己写（见 [Delivery] 的说明）；
/// 多一个 `input_dir`：投放目标**由工具入口算好**（[`target_input_dir`]），这里不再自己推
/// —— 也因此不再需要 `state`（它当初只为推 input 目录而传）。
pub fn make_mask_with(
    args: &Value,
    input_dir: &Path,
    delivery: &mut Delivery,
) -> Result<Value, ToolFailure> {
    // ⚠ 先于 ComfyUI 探测：参数写错了就当场报，**别等去找实例**（也让它能单测）。
    reject_removed_args(args)?;

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
    let tolerance = args
        .get("tolerance")
        .and_then(Value::as_f64)
        .unwrap_or(SUBJECT_TOLERANCE_DEFAULT);

    // `bgColor`：显式告诉**定位器**"背景是什么颜色"（"#00b140" / "green" / "magenta"）——
    // 只喂定位器（bbox / 摆放 / `subjectRatio` 诊断），**不影响任何画面像素**：
    // 合成用的 alpha 由工作流里的抠图节点现算，我们猜出来的那个剪影不参与合成。
    let explicit_background = str_arg(args, "bgColor").and_then(|raw| parse_color(&raw));
    let mut subject_info: Option<(String, [u8; 3], f64)> = None;
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
        let (mask, background, ratio) =
            subject_mask(&subject_path, w as usize, h as usize, tolerance, &region, use_region, explicit_background)?;
        subject_info = Some((subject, background, ratio));
        // ⚠ 这个剪影**只当定位器**：算 bbox（摆放要用）、报 `subjectRatio` / `background` 供诊断。
        //    它**不写盘、也不当合成遮罩** —— 合成用的 alpha 由工作流里的抠图节点现算。
        //    所以这里没有"按颜色收边（choke）/ 几何收缩 / 反混 / 去溢色"：那些是
        //    "拿猜出来的剪影去合成"才需要的补救，2026-09-27 整档删除（见函数头）。
        mask
    } else {
        draw_mask(w as usize, h as usize, &region, &shape)
    };

    // **摆放**（用户 2026-09-26 要的能力）：`scale` 缩放、`x`/`y` 把剪影中心搬到归一化坐标。
    // 关键点：**人物图的像素与遮罩一起动**（只动一个就是"人在这儿、遮罩在那儿"的错位）。
    // 实现方式是烘进文件（写一张 canvas 大小的新人物图 + 新遮罩），
    // 这样**不用改工作流**：还是 `20.image` + `12.image` 两个覆盖。
    let mut placed_subject: Option<(String, [f64; 4])> = None;
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
            // ⚠ 编码成字节后**一律走 `write_or_defer`**（2026-09-29 用户 bug）：
            // 原来这里是 `scaled_person.save(&placed_path)` 直写 + 当场报错，
            // 于是"投放原图能过（走 HTTP）、摆好的人物图却过不去"—— 阶段二就卡在
            // "没拿到 personFilename"。真因是后端进程写不进项目外，**不是** input 的权限。
            let mut placed_bytes: Vec<u8> = Vec::new();
            {
                use image::ImageEncoder;
                image::codecs::png::PngEncoder::new(&mut placed_bytes)
                    .write_image(
                        scaled_person.as_raw(),
                        w as u32,
                        h as u32,
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(|e| {
                        ToolFailure::new(
                            "COMFY_INPUT_FAILED",
                            format!("编码摆放好的人物图失败（{placed_name}）：{e}"),
                        )
                    })?;
            }
            write_or_defer(&placed_path, placed_bytes, delivery)?;
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
    // `mode="subject"`（贴人物图）**不写遮罩**：真正的 alpha 由工作流里的抠图节点现算
    // （写一张没人用的文件只会在 input 目录里攒垃圾，还会让调用方以为该去覆盖 LoadImageMask）。
    let mask_written = mode != "subject";
    if mask_written {
        write_mask_png(&pixels, w as u32, h as u32, &input_dir.join(&filename), delivery)?;
    }

    let region_joined = region.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
    // `mode="subject"` 要告诉调用方"拿哪张人物图去覆盖 20.image"
    let person_filename = placed_subject
        .as_ref()
        .map(|(name, _)| name.clone())
        .or_else(|| subject_info.as_ref().map(|(name, _, _)| name.clone()));
    let mut out = json!({
        "inputDir": input_dir.to_string_lossy(),
        "width": w,
        "height": h,
        "mode": mode,
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
            "`mode=\"subject\"` **不写遮罩**：合成用的 alpha 由工作流里的抠图节点现算，\
             所以**不要**覆盖任何 LoadImageMask（那份工作流里也没有）。\
             ① comfy_load_workflow(path=<用户自己的贴人工作流>) —— 要有 `easy imageRemBg` + \
             `refine_foreground=true` 那条链；仓库不再随包带模板，用户没有就让他先在 ComfyUI 里存一份；\
             ② 从 apiGraph 现读 id 后覆盖：底图 LoadImage、人物 LoadImage=\"{person}\"、\
             两个 ImageScale 的 width={w} / height={h}、正面提示词（只写**人物外面那一圈**该出现什么，写背景）、seed；\
             ③ 采样器的 denoise 用 0.3~0.45（只融接缝；0 = 纯合成、一个像素都不改）。\
             ⚠ 那份工作流要求 ComfyUI 里有抠图模型（models\\rembg\\BEN2_Base.pth 或 RMBG-1.4.pth，\
             先用 comfy_get_status 的 mattingModels 查）——没有就如实告诉用户下哪个，别让它静默下载。",
            person = person_filename.unwrap_or_default(),
        ));
    }
    if let Some((subject, background, ratio)) = subject_info {
        // 定位器诊断：这几项只说明"我们猜的人物在哪、占多大"，**画面不受它们影响**
        // （合成用的 alpha 是工作流里抠图节点给的）。所以它们只用来判断"摆放靠不靠谱"。
        out["subjectFilename"] = json!(subject);
        out["background"] = json!(format!("#{:02x}{:02x}{:02x}", background[0], background[1], background[2]));
        out["subjectRatio"] = json!((ratio * 1000.0).round() / 1000.0);
        out["locatorNote"] = json!(
            "剪影**只当定位器**（bbox / 摆放 / 这份诊断），不参与合成 —— \
             合成用的 alpha 由工作流里的抠图节点现算。所以 subjectRatio 偏大偏小只影响摆放，\
             不影响贴出来干不干净（边缘干净不干净取决于实例上的抠图模型）。"
        );
        let saturation = background.iter().max().copied().unwrap_or(0) as i32
            - background.iter().min().copied().unwrap_or(0) as i32;
        let chroma = saturation >= CHROMA_MIN_SATURATION;
        out["chromaKey"] = json!(chroma);
        // 显式给了 bgColor 但实际用的是另一个色 → 如实说明（定位器那块绿/品红与写死的值不一样）
        if let Some(requested) = explicit_background {
            if requested != background {
                out["bgColorIgnored"] = json!(format!(
                    "#{:02x}{:02x}{:02x}",
                    requested[0], requested[1], requested[2]
                ));
            }
        }
        if !chroma {
            // ⚠ 低饱和背景（白 / 浅灰）与"人物身上的白"同色 —— 定位器只能靠连通性猜，
            //   估出来的人物可能偏大/偏小（**只影响摆放**；边缘是工作流里的抠图模型做的）。
            out["chromaAdvice"] = json!(
                "⚠ 背景是白/浅色（低饱和）：**定位器**在这种背景上容易把人物估得偏大/偏小\
                 （人物身上的白裙子/高光与背景同色）。要摆放得准就把**阶段一的背景换成绿幕或品红**\
                 （提示词写 `green background`，并把 `bgColor=\"#00b140\"` 传进来）。\
                 ⚠ 这只影响摆放的准确性，**不影响贴出来的边缘** —— 边缘由工作流里的抠图模型做。"
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
        if !placed_hint.is_empty() {
            out["hint"] = json!(format!(
                "{} {placed_hint}",
                out["hint"].as_str().unwrap_or("")
            ));
        }
        if let Some((placed_name, _)) = placed_subject.as_ref() {
            out["placedSubjectFilename"] = json!(placed_name);
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
    mask.fill(0);
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
        let y1 = ((oy + 1) * h).div_ceil(out_h).min(h).max(y0 + 1);
        for ox in 0..out_w {
            let x0 = ox * w / out_w;
            let x1 = ((ox + 1) * w).div_ceil(out_w).min(w).max(x0 + 1);
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
// 几何参数（bbox / scale / 三个中心点）天然是一组，包成结构体只会在调用点再拆一次。
#[allow(clippy::too_many_arguments)]
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

/// 抠图模型在不在 —— **"贴人物图"那条路的前置检查**（2026-09-27）。
///
/// # 为什么非报不可
///
/// `easy imageRemBg`（`comfyui-easy-use`）读的是 `folder_paths.models_dir` **再拼一个 `rembg`**，
/// 也就是 **`<ComfyUI 根>\models\rembg\`** 这一个目录。三个容易搞错的地方：
///
/// 1. 它**不是** ComfyUI 原生的 `models\background_removal\`（那是另一套节点 `LoadBackgroundRemovalModel`）；
/// 2. 它**不认** Desktop 那份"共享数据"里的 `rembg`（easy-use 用的是单个 `models_dir`，
///    不是 ComfyUI 的多路径注册表）—— 所以"`inputDir` 的兄弟目录"是**错的**答案；
/// 3. 文件不在时那个节点会**自己静默下载**（`torch.hub.download_url_to_file` → HuggingFace），
///    国内常常很慢或直接失败 —— 表现就是用户说的「**点了按钮没反应**」。
///
/// 所以这里把"每个候选 ComfyUI 根底下的 `models\rembg` 里到底有什么"如实列出来，
/// 让模型**先查再跑**；`usable=false` 时按 `download` 那两条指引用户放好文件。
///
/// 判据只有**文件名**（easy-use 的 `get_local_filepath` 按 URL 的 basename 找）：
/// `BEN2_Base.pth`（1.1GB，质量最好）/ `RMBG-1.4.pth`（169MB，快一个数量级）。
pub fn matting_models(roots: &[std::path::PathBuf]) -> Value {
    const FILES: [&str; 2] = ["BEN2_Base.pth", "RMBG-1.4.pth"];
    let mut checked: Vec<Value> = Vec::new();
    let mut usable = false;
    for root in roots {
        // 只认"看起来真是 ComfyUI"的目录（auto_read_roots 里还混着我们自己的 comfyui / storage）
        if !crate::comfy::comfy_dirs::looks_like_comfy(root).0 {
            continue;
        }
        let dir = root.join("models").join("rembg");
        let files: Vec<&str> = FILES.iter().copied().filter(|name| dir.join(name).is_file()).collect();
        if !files.is_empty() {
            usable = true;
        }
        checked.push(json!({
            "root": root.to_string_lossy(),
            "dir": dir.to_string_lossy(),
            "files": files,
        }));
    }
    json!({
        "node": "easy imageRemBg",
        "priority": FILES,
        "usable": usable,
        "checked": checked,
        "download": {
            "BEN2_Base.pth": "HuggingFace 仓库 PramaLLC/BEN2 里的 BEN2_Base.pth（1.1GB，首选，边缘最干净）",
            "RMBG-1.4.pth": "HuggingFace 仓库 briaai/RMBG-1.4 里的 model.pth（169MB，快一个数量级；**要存成 RMBG-1.4.pth**）",
            "dir": "<上面 checked 里那个 ComfyUI 根>\\models\\rembg\\"
        },
        "note": "usable=false 时**别硬跑**：那个节点会自己去 HuggingFace 静默下载（常常很慢或失败，\
                 表现是「点了按钮没反应」）。唯一正确的动作是按 download 那两条指引用户把文件放好；\
                 **没有退路** —— 以前那档「Rust 按颜色抠剪影当遮罩」已经删除（轮廓外会留一圈青绿、\
                 发梢会被整段吃掉），别去试它，也别让用户以为还有别的办法。"
    })
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
    let radius = (feather / 2).max(1);
    let window = 2 * radius + 1;
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
fn write_mask_png(
    pixels: &[u8],
    w: u32,
    h: u32,
    target: &Path,
    delivery: &mut Delivery,
) -> Result<(), ToolFailure> {
    // 编成 PNG 字节**只做一次**：直写用它、直写被拒时也用它去走 ComfyUI 的 HTTP 上传
    let mut bytes: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(pixels, w, h, image::ExtendedColorType::L8)
            .map_err(|e| ToolFailure::new("COMFY_INPUT_FAILED", format!("编码遮罩失败：{e}")))?;
    }
    write_or_defer(target, bytes, delivery)
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
pub async fn tool_use_attachment(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, ToolFailure> {
    let attachment_id = arguments
        .get("attachmentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", "缺少参数 attachmentId（见用户消息里的附件说明）"))?
        .to_string();
    let wanted = str_arg(arguments, "filename");
    let target = target_input_dir(state).await?;
    let mut delivery = Delivery::for_input_dir(&target.dir);
    let mut result = use_attachment(state, &attachment_id, wanted.as_deref(), &target.dir, &mut delivery)?;
    apply_uploaded_names(&mut result, &delivery.flush(state).await?);
    annotate_target(&mut result, &target);
    Ok(crate::tools::tools::ToolOutcome::new(result.to_string(), result))
}

/// `comfy_use_media` 的工具入口。
///
/// `mediaId` 认**数字与字符串**（模型会把 id 写成 `"310"`）——与 Kotlin 那边同一条容错。
pub async fn tool_use_media(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, ToolFailure> {
    let media_id = int_arg(arguments, "mediaId")
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", "缺少参数 mediaId（画廊里的图片产物 id）"))?;
    let wanted = str_arg(arguments, "filename");
    let target = target_input_dir(state).await?;
    let mut delivery = Delivery::for_input_dir(&target.dir);
    let mut result = use_media(state, media_id, wanted.as_deref(), &target.dir, &mut delivery)?;
    apply_uploaded_names(&mut result, &delivery.flush(state).await?);
    annotate_target(&mut result, &target);
    Ok(crate::tools::tools::ToolOutcome::new(result.to_string(), result))
}

/// `comfy_make_mask` 的工具入口。
pub async fn tool_make_mask(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, ToolFailure> {
    let target = target_input_dir(state).await?;
    let mut delivery = Delivery::for_input_dir(&target.dir);
    let mut result = make_mask_with(arguments, &target.dir, &mut delivery)?;
    // ⚠ 走 HTTP 的可能是**摆放后的人物图**（`placed-…`）而不只是遮罩 ——
    //   所以按名字回填（`filename` / `personFilename` / `placedSubjectFilename`），不许写死一个键。
    let delivered = delivery.flush(state).await?;
    apply_uploaded_names(&mut result, &delivered);
    annotate_target(&mut result, &target);
    Ok(crate::tools::tools::ToolOutcome::new(result.to_string(), result))
}

/// 把"这份目录是怎么定下来的"如实写进结果里。
///
/// 为什么必须回给模型：`inputDir` 是我们**投进去**的地方，而工作流里的 `LoadImage` 认的是
/// **实例在读**的地方 —— 两边不一致时提交会回 400 `Invalid image file`，而模型光看一个
/// 路径字符串是分辨不出来的（`instance` = 问过实例，最可信；其余是推的，不一致时带 note）。
fn annotate_target(result: &mut Value, target: &TargetInputDir) {
    result["inputDirSource"] = json!(target.source);
    if let Some(note) = &target.note {
        result["inputDirNote"] = json!(note);
    }
}

#[cfg(test)]
mod tests;
