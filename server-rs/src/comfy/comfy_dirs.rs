//! 本机 ComfyUI 中**已保存的工作流文件**（`user\<账号>\workflows\*.json`）。
//!
//! Rust 版逐条对照 Kotlin `ComfyWorkflowFiles.kt` + `ComfyLocator.kt` + `ComfyRoots.kt`
//! （总纲 §9 ②）。**三个目录来源一条都不能少** —— 少搬任何一条，接口都会
//! "正常返回、但缺少几个目录"，表现为"本机存在工作流，界面返回未找到"。
//!
//! | 来源 | Kotlin | 为什么非有不可 |
//! | --- | --- | --- |
//! | 配置里生效的输出目录的**父目录** | `ComfyWorkflowFiles.comfyHomes` ① | `<ComfyUI>\output` 的父目录就是 ComfyUI 根 |
//! | `ComfyLocator.find()` 探到的 home | 同上 ② | 未配置输出目录时的兜底 |
//! | `ComfyRoots.autoReadRoots()` | 同上 ③ | ComfyUI **Desktop 把程序与共享数据分家**时，工作流在安装目录下 |
//!
//! 两条约束沿用：**只读**（扫描不改动文件、不写库）；**有界 + 认特征**
//! （只在认出来的 ComfyUI 根底下找 `user\*\workflows`，一眼看完，不递归、不扫全盘）。
//!
//! ⚠ `looks_like_comfy` 的判据现在有**三份实现**（`ComfyLocator.kt`、
//! `scripts\comfy-path.ps1`、这里）。改判据要同时改三处，否则会出现
//! "App 说找到了、脚本说找不到"（AGENTS §10.1）。

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 一份工作流文件最大认多大（与 Kotlin `ComfyWorkflowFiles.MAX_FILE_BYTES` 一致）。
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 一次最多列出几份（工作流目录通常几十份；上万份说明目录识别错误）。
pub const MAX_FILES: usize = 200;

/// 一个 ComfyUI 根下最多查看几个账号目录（`user\` 中通常只有 default / __manager）。
const MAX_USERS: usize = 20;

/// 兄弟目录里最多往下翻几层（`ComfyRoots.MAX_DEPTH`）。
const MAX_DEPTH: usize = 3;

/// 一次探测最多看多少个目录（`ComfyRoots.MAX_VISITED`；防止在盘根上跑很久）。
const MAX_VISITED: usize = 3000;

const ENV_KEYS: [&str; 3] = ["COMFYHUB_COMFY_HOME", "COMFYUI_HOME", "COMFYUI_PATH"];

/// 本机工作流目录里的一份文件。
///
/// `sha256` 是**内容寻址**的键：`comfy_load_workflow` 入库时用的 `run_key = file:<sha256>`，
/// 所以拿它就能问"这份文件是不是已经在库里了"（`promptId` 非空就是）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// `ui` = 界面格式（nodes/links）；`api` =「导出（API）」的节点图；`unknown` = 读不出来
    pub format: String,
    #[serde(skip)]
    pub sha256: String,
    /// 已经在本项目库里的话，这是那份提示词的 id（可以直接 `comfy_submit(promptId=…)`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<i64>,
}

// ---------------------------------------------------------------------------
//  特征判定（三份实现之一，判据必须一致）
// ---------------------------------------------------------------------------

/// 这个目录看起来是不是 ComfyUI（`(是否, 依据)`）。
///
/// 判据刻意严一点：`main.py` + `comfy/` 是**源码形态**，`output/` + `input/`/`models/`
/// 是**任何形态**（含 Desktop / 整合包）都有的。只有一个空 `output` 不算 ——
/// 任意建立 output 目录就会被误判，之后"捕获一条也收不到"。
pub fn looks_like_comfy(dir: &Path) -> (bool, String) {
    let has_main = dir.join("main.py").is_file();
    let has_pkg = dir.join("comfy").is_dir();
    let has_output = dir.join("output").is_dir();
    let has_models = dir.join("models").is_dir();
    let has_input = dir.join("input").is_dir();
    if has_main && has_pkg {
        (true, "有 main.py 与 comfy/（源码形态）".into())
    } else if has_output && (has_models || has_input) {
        (true, "有 output/ 与 models/ 或 input/".into())
    } else if has_main {
        (
            false,
            "有 main.py 但没有 comfy/（可能不是 ComfyUI 根目录）".into(),
        )
    } else if has_output {
        (
            false,
            "只有 output/，没有 models/ 或 input/（不太像 ComfyUI 根目录）".into(),
        )
    } else {
        (false, "没有 main.py / comfy/ / output/".into())
    }
}

// ---------------------------------------------------------------------------
//  目录发现
// ---------------------------------------------------------------------------

/// 本机所有**实际存在**的 `user\<账号>\workflows`（去重、保持"最可能命中"的顺序）。
pub fn workflow_dirs(project_root: &Path, configured_output: Option<&str>) -> Vec<PathBuf> {
    dirs_from_homes(&comfy_homes(project_root, configured_output))
}

/// 一组 ComfyUI 根目录 → 其下实际存在的 `user\<账号>\workflows`。
///
/// 与 [`comfy_homes`] 分开是为了能单测：这一半是纯文件系统逻辑（临时目录就能验）。
pub fn dirs_from_homes(homes: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for home in homes {
        user_workflow_dirs(home, &mut out);
    }
    out
}

/// 本机可能装着 ComfyUI 的那些目录（三份来源，都可能为空）。
/// 本机可能装着 ComfyUI 的那些**根**目录（去重、按"最可能命中"排序）。
///
/// 三份来源都要（少一份就会漏掉真机上的那种布局）：
/// ① 配置里已生效的输出目录的父目录；② [`find_home`] 的探测结果（含 Desktop 的
/// `ComfyUI-Installs\<实例>\ComfyUI` 那套嵌套）；③ [`auto_read_roots`]。
///
/// `pub` 是给 `image_edit::matting_models` 用的（2026-09-27）：抠图模型只认
/// `<根>\models\rembg`，而**"配置的输出目录"那一份**与**安装目录那一份**是两个不同的根
/// —— 真机上模型在安装目录、共享目录里那份是空的，两处都要查过才敢说"在不在"。
pub fn comfy_homes(project_root: &Path, configured_output: Option<&str>) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    let push = |p: Option<PathBuf>, homes: &mut Vec<PathBuf>| {
        if let Some(p) = p {
            if !homes.contains(&p) {
                homes.push(p);
            }
        }
    };

    // 1) 配置里已经生效的输出目录：`<ComfyUI>\output` 的父目录就是 ComfyUI 的根。
    //    读库失败（首次启动还没建库）不该让后面两条更可靠的来源一起丢掉。
    if let Some(dir) = configured_output.and_then(|s| real_dir(Path::new(s))) {
        let home = if dir
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("output"))
            .unwrap_or(false)
        {
            dir.parent().map(|p| p.to_path_buf())
        } else {
            Some(dir)
        };
        push(home.and_then(|p| real_dir(&p)), &mut homes);
    }

    // 2) 探测到的 home（只读，找不到就是没有）
    push(find_home(project_root, configured_output), &mut homes);

    // 3) 自动放行的那些 ComfyUI 目录（含 Desktop 的安装目录与它的兄弟目录）
    for root in auto_read_roots(project_root, configured_output) {
        push(Some(root), &mut homes);
    }

    homes
}

/// 一个 ComfyUI 根下的 `user\<账号>\workflows`。
///
/// 查看的是 `user\` 的**直接子目录**（通常只有 `default`，多账号时各一份），
/// 不递归、不跟随符号链接 —— 这一层很浅，而 `models\` 那种几百 GB 的目录根本不在路径上。
fn user_workflow_dirs(home: &Path, out: &mut Vec<PathBuf>) {
    let user = home.join("user");
    if !user.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&user) else {
        return;
    };
    let mut users: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    users.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    for u in users.into_iter().take(MAX_USERS) {
        if u.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        if !u.is_dir() {
            continue;
        }
        let wf = u.join("workflows");
        if wf.is_dir() {
            if let Some(real) = real_dir(&wf) {
                if !out.contains(&real) {
                    out.push(real);
                }
            }
        }
    }
}

/// 绝对化 + 规范化 + `canonicalize`（不是目录就返回 None）。
fn real_dir(p: &Path) -> Option<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(p)
    };
    if !abs.is_dir() {
        return None;
    }
    abs.canonicalize().ok().map(plain_path)
}

/// 去掉 Windows 的**逐字（verbatim）前缀** `\\?\`（UNC 的 `\\?\UNC\` → `\\`）。
///
/// 为什么非要这一步：`std::fs::canonicalize()` 在 Windows 上返回的是
/// `\\?\D:\Comfy-Desktop\…`，而 Kotlin 的 `Path.toRealPath()` 返回 `D:\Comfy-Desktop\…`。
/// 该前缀**原样出现在接口返回中**（`dirs` / 每个文件的 `path`），界面与 AI 均可见；
/// 它还会被贴进对话里当参数用 —— 那是"同一个目录、两种写法"，最难查的那类不一致。
/// 实测这条：真库副本上第一次跑出来就是 `\\?\D:\Comfy-Desktop\…\workflows`。
///
/// `pub(crate)`：`ToolPolicy::real_path` 也要过它（P4 第 40 步）—— 同一件事只留一处实现，
/// 否则"工具结果里的路径"与"接口返回里的路径"会各去掉一次前缀、也可能各漏一次。
pub(crate) fn plain_path(p: PathBuf) -> PathBuf {
    let text = p.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    real_dir(Path::new(raw))
}

// ---------------------------------------------------------------------------
//  ComfyLocator.find 的 home 那一半（P4 只用它）
// ---------------------------------------------------------------------------

/// 探测结果（与 Kotlin `ComfyLocation` 逐字同形）。
///
/// 探测是**只读**的，结果**不会自动生效**：`find` 只回答"在哪"，
/// 是否采用由「使用这个目录」决定（`POST /api/capture/locate/apply`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComfyLocation {
    /// 探测到的 ComfyUI 根目录（找不到时为 null → **键不出现**，Kotlin `explicitNulls = false`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// 大概率能直接读产物的输出目录（`<home>\output`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    /// 来源（界面显示的一句说明）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 当前设置里**已经**生效的输出目录（没设置时为 null）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_output_dir: Option<String>,
    /// 探测到的候选（含被否决的原因），排错用。
    pub candidates: Vec<ComfyCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComfyCandidate {
    pub path: String,
    pub kind: String,
    /// 这个目录看起来是不是一个 ComfyUI 安装（有特征文件）。
    pub looks_like_comfy: bool,
    /// 判定依据 / 否决原因（只读展示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 按"最可能命中"的顺序找 ComfyUI（每个候选都要过 [`looks_like_comfy`] 这一关）。
pub fn locate(project_root: &Path, configured_output: Option<&str>) -> ComfyLocation {
    let mut candidates: Vec<ComfyCandidate> = Vec::new();

    // 1) 已配置：直接使用，不再探测（配置错误时由界面修改）
    if let Some(configured) = configured_output.map(str::trim).filter(|s| !s.is_empty()) {
        let dir = Path::new(configured);
        let home = if dir
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("output"))
            .unwrap_or(false)
        {
            dir.parent().map(|p| p.to_path_buf())
        } else {
            Some(dir.to_path_buf())
        };
        return ComfyLocation {
            home: home.map(|p| p.to_string_lossy().to_string()),
            output_dir: Some(configured.to_string()),
            source: Some("已经配置过了（就按这个用）".into()),
            configured_output_dir: Some(configured.to_string()),
            candidates,
            note: None,
        };
    }

    // `consider`：目录存在才记录候选，无论是否通过特征判定都记录（否决原因需在界面显示）
    let consider = |path: Option<PathBuf>, kind: &str, candidates: &mut Vec<ComfyCandidate>| {
        let path = path?;
        if !path.is_dir() {
            return None;
        }
        let (ok, reason) = looks_like_comfy(&path);
        candidates.push(ComfyCandidate {
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            looks_like_comfy: ok,
            reason: Some(reason),
        });
        if ok {
            Some(path)
        } else {
            None
        }
    };

    // 2) 显式环境变量（脚本 / 高级用法）
    let env_home = [
        ("COMFYHUB_COMFY_HOME", "环境变量 COMFYHUB_COMFY_HOME"),
        ("COMFYUI_HOME", "环境变量 COMFYUI_HOME"),
        ("COMFYUI_PATH", "环境变量 COMFYUI_PATH"),
    ]
    .into_iter()
    .find_map(|(key, kind)| consider(env_path(key), kind, &mut candidates));

    // 3) 项目根下的 comfyui（源码树 / 发布包都能带上这一份）
    let local = consider(
        Some(project_root.join("comfyui")),
        "项目内的 comfyui 目录",
        &mut candidates,
    );

    // 4) 常见安装位置（**每个固定盘的盘根**，不写死盘符 —— 见 `fixed_drive_roots`）
    let located = env_home
        .or(local)
        .or_else(|| {
            drive_install_candidates()
                .into_iter()
                .find_map(|(path, kind)| consider(Some(path), &kind, &mut candidates))
        })
        .or_else(search_user_profile)
        .or_else(search_desktop_installs)
        .or_else(search_desktop_packages);

    let Some(located) = located else {
        return ComfyLocation {
            home: None,
            output_dir: None,
            source: None,
            configured_output_dir: None,
            candidates,
            note: Some(
                "没找到 ComfyUI。可以手工把它的 output 目录填到「ComfyUI 自动捕获」里\
                 （ComfyUI 界面里点齿轮 → 看「输出目录」），或者设一个 COMFYHUB_COMFY_HOME 环境变量。"
                    .into(),
            ),
        };
    };

    // 输出目录：存在则使用实际值，不存在也写出 `<home>\output`（可能尚未运行过 ComfyUI）
    let output = located.join("output");
    let source = candidates
        .iter()
        .rev()
        .find(|c| c.looks_like_comfy)
        .map(|c| c.kind.clone());
    ComfyLocation {
        home: Some(plain_path(located.clone()).to_string_lossy().to_string()),
        output_dir: Some(plain_path(output).to_string_lossy().to_string()),
        source,
        configured_output_dir: None,
        candidates,
        note: None,
    }
}

/// 这台机器上**真正存在**的固定盘根（`C:\`、`D:\`、`E:\` …，按盘符排序）。
///
/// # 为什么要有它（2026-11-10 复查）
///
/// 以前这里写死了 `D:\ComfyUI`、`D:\ComfyUI_windows_portable\ComfyUI`、
/// `D:\Comfy-Desktop`、甚至整个 `D:\` 顶层 —— 那是**开发机**的布局。换台机器
/// （ComfyUI 装在 `C:\` 或 `E:\`，或者根本没有 D 盘）就永远"找不到 ComfyUI"，
/// 而界面只显示未找到。这不是个别情况，**任何环境**都可能出现。
///
/// 用 `GetDriveTypeW` 只留 `DRIVE_FIXED`：网络映射盘 / 光驱不去碰
/// （`is_dir()` 在断开的网络盘上会卡住几秒到几十秒），也免得把别人的共享盘
/// 当成"本机 ComfyUI"。
#[cfg(windows)]
pub(crate) fn fixed_drive_roots() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    // winbase.h 的 `DRIVE_FIXED`。windows-sys 把它放在 `Win32::System::WindowsProgramming`
    // 里，为一个整数去开那一整个 API 集不值当。
    const DRIVE_FIXED: u32 = 3;

    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for bit in 0..26u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let letter = (b'A' + bit as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` 是以 NUL 结尾的 UTF-16 串，生命周期覆盖这次调用
        if unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_FIXED {
            out.push(PathBuf::from(root));
        }
    }
    out
}

#[cfg(not(windows))]
pub(crate) fn fixed_drive_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// 常见安装位置：**每个固定盘的盘根**（`X:\ComfyUI`、`X:\ComfyUI_windows_portable\ComfyUI`）。
///
/// 返回 `(路径, 给人看的说明)`，由调用方逐个 `consider` 记进候选 —— 这样"找到没找到、
/// 哪几个不像"都能如实列出来，而不是只报一句"没找到"。
pub(crate) fn drive_install_candidates() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for drive in fixed_drive_roots() {
        let letter = drive.to_string_lossy().trim_end_matches('\\').to_string();
        out.push((
            drive.join("ComfyUI"),
            format!("常见位置 {letter}\\ComfyUI"),
        ));
        out.push((
            drive.join("ComfyUI_windows_portable").join("ComfyUI"),
            format!("免安装包 {letter}\\ComfyUI_windows_portable"),
        ));
    }
    out
}

/// 按"最可能命中"的顺序找 ComfyUI 的**根目录**（[`locate`] 只要 home 那一半）。
pub fn find_home(project_root: &Path, configured_output: Option<&str>) -> Option<PathBuf> {
    locate(project_root, configured_output)
        .home
        .map(PathBuf::from)
}

/// `%USERPROFILE%\Documents\ComfyUI`（ComfyUI Desktop 的默认安装位置）。
pub(crate) fn search_user_profile() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE").ok()?;
    let base = PathBuf::from(home);
    let tries = [
        base.join("Documents").join("ComfyUI"),
        base.join("ComfyUI"),
        // ComfyUI Desktop（新版）常把源码检出放在 Documents\comfy\ComfyUI
        base.join("Documents").join("comfy").join("ComfyUI"),
        base.join("AppData")
            .join("Local")
            .join("Programs")
            .join("@comfyorgcomfyui-electron"),
    ];
    tries
        .into_iter()
        .find(|p| p.is_dir() && looks_like_comfy(p).0)
}

/// ComfyUI **Desktop**（新版 Electron 启动器）的嵌套布局。
///
/// 它把源码检出放在 `<应用目录>\ComfyUI-Installs\<实例名>\ComfyUI` 下，
/// 而不是 `Documents\ComfyUI` —— 仅查找顶层时无法找到（实测为该布局）。
/// 探测**有界**：固定几个应用目录 → 每个目录下的 `ComfyUI-Installs\*` 再拼一层 `ComfyUI`。
pub(crate) fn search_desktop_installs() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local_app_data);
        roots.push(base.join("Programs").join("@comfyorgcomfyui-electron"));
        roots.push(base.join("Comfy-Desktop"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(home).join("Documents").join("comfy"));
    }
    // 有人把 ComfyUI Desktop 装在别的盘（`E:\Comfy-Desktop` 之类）：每个固定盘都看
    // 一眼，别只认开发机的 `D:\Comfy-Desktop`（2026-11-10）。
    for drive in fixed_drive_roots() {
        roots.push(drive.join("Comfy-Desktop"));
    }

    for root in roots {
        if !root.is_dir() {
            continue;
        }
        // (a) <root>\ComfyUI-Installs\<实例>\ComfyUI   (b) <root>\<实例>\ComfyUI
        for base in [root.join("ComfyUI-Installs"), root.clone()] {
            if !base.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            children.sort();
            for child in children {
                let hit = child.join("ComfyUI");
                if looks_like_comfy(&hit).0 {
                    return Some(plain_path(hit.canonicalize().unwrap_or(hit)));
                }
            }
        }
    }
    None
}

/// 秋叶整合包 / 便携包常见的 `*ComfyUI*` 顶层目录（只翻一层，避免全盘扫描）。
pub(crate) fn search_desktop_packages() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE") {
        let base = PathBuf::from(home);
        roots.push(base.join("Desktop"));
        roots.push(base.join("Documents"));
        roots.push(base.join("Downloads"));
    }
    // 整合包常被解压到"某个盘的盘根"（`D:\ComfyUI-aki-v1.6` 这种）。以前只扫 `D:\`，
    // 换台机器（解压在 C: / E:）就找不到 —— 现在每个固定盘都扫一层（2026-11-10）。
    roots.extend(fixed_drive_roots());

    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            if !child.is_dir() {
                continue;
            }
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.contains("comfy") && looks_like_comfy(&child).0 {
                return Some(plain_path(child.canonicalize().unwrap_or(child)));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
//  ComfyRoots.autoReadRoots
// ---------------------------------------------------------------------------

/// 除了出厂默认（`<根>\comfyui` + `<根>\storage`）之外，**还要放行只读**的本机 ComfyUI 目录。
///
/// 返回的都是真实路径、去重、按"最可能命中"排序。找不到就是空列表（不是错误：
/// 可能根本没有安装 ComfyUI）。
pub fn auto_read_roots(project_root: &Path, configured_output: Option<&str>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if !out.contains(&p) {
            out.push(p);
        }
    };

    let seeds = seed_roots(project_root, configured_output);
    for seed in &seeds {
        push(seed.clone(), &mut out);
    }
    // 每一个已知的 ComfyUI 目录，都在它的"上一级"里找找同机的**其它**安装
    // （Desktop 的 Installs / Shared 分家就是靠这一步找到工作流目录的）
    for seed in &seeds {
        if let Some(parent) = seed.parent() {
            for found in find_nearby(parent) {
                push(found, &mut out);
            }
        }
    }
    // 约束：**放行"已知 ComfyUI 根的上一级"（只读）** ——
    // "往上走一层看看旁边还有什么"正是发现 Shared 目录的唯一路径。
    // 只放**一级**，而且**绝不放盘根**（放了就等于整盘只读，那是另一档权限）。
    let snapshot = out.clone();
    for root in snapshot {
        let Some(parent) = root.parent() else {
            continue;
        };
        if parent.parent().is_none() {
            continue;
        }
        if parent.is_dir() {
            push(parent.to_path_buf(), &mut out);
        }
    }
    out
}

/// 已知的 ComfyUI 根目录：环境变量 → 配置的输出目录 → 项目内的 `comfyui`。
fn seed_roots(project_root: &Path, configured_output: Option<&str>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for key in ENV_KEYS {
        if let Some(p) = env_path(key) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    if let Some(dir) = configured_output.and_then(|s| real_dir(Path::new(s))) {
        let looks_like_output = dir
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("output"))
            .unwrap_or(false);
        let target = if looks_like_output {
            dir.parent().map(|p| p.to_path_buf())
        } else {
            Some(dir)
        };
        if let Some(t) = target {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    if let Some(local) = real_dir(&project_root.join("comfyui")) {
        if !out.contains(&local) {
            out.push(local);
        }
    }
    out.retain(|p| p.is_dir());
    out
}

/// 在 `start` 底下找 ComfyUI 目录（ComfyUI Desktop 那套"程序装在一处、共享数据放在另一处"）。
pub fn find_nearby(start: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !start.is_dir() {
        return out;
    }
    let mut budget = MAX_VISITED;
    scan_nearby(start, &mut out, 1, &mut budget);
    out
}

/// 在 `start` 底下找 ComfyUI 目录。
///
/// `depth` 从 1 开始；**第一层随便进，更深一层要求目录名里带 `comfy`** ——
/// 这一条同时解决两个问题：能找到 `ComfyUI-Installs\ComfyUI\ComfyUI` 这种三层嵌套，
/// 又不会翻进 `models\` 这种几百 GB 的目录。一旦某个目录被认成 ComfyUI 就不再往里翻。
fn scan_nearby(start: &Path, out: &mut Vec<PathBuf>, depth: usize, budget: &mut usize) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(start) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });

    for child in children {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if child
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        if !child.is_dir() {
            continue;
        }
        let Ok(real) = child.canonicalize().map(plain_path) else {
            continue;
        };
        if looks_like_comfy(&real).0 {
            if !out.contains(&real) {
                out.push(real);
            }
            continue;
        }
        let name_looks_comfy = real
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains("comfy"))
            .unwrap_or(false);
        if depth == 1 || name_looks_comfy {
            scan_nearby(&real, out, depth + 1, budget);
        }
    }
}

// ---------------------------------------------------------------------------
//  列文件（只读）
// ---------------------------------------------------------------------------

/// 列出这些目录里的工作流文件（按修改时间倒序，最多 `limit` 份）。
///
/// `query` 是**文件名**的子串匹配（大小写不敏感）—— 例如"krea2 那条"可直接命中。
/// 只列**真的能用**的：非 `.json` / 空文件 / 坏 JSON / 既不是界面格式也不是 API 格式的
/// 全部跳过（列出来只会让 AI 白跑一趟 `comfy_load_workflow`）。
pub fn scan(dirs: &[PathBuf], query: Option<&str>, limit: usize) -> Vec<WorkflowFile> {
    let q = query
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let cap = limit.clamp(1, MAX_FILES);
    let mut out: Vec<WorkflowFile> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if !name.to_lowercase().ends_with(".json") {
                continue;
            }
            if let Some(q) = &q {
                if !name.to_lowercase().contains(q) {
                    continue;
                }
            }
            if !path.is_file() {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let size = meta.len();
            if size == 0 || size > MAX_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let modified_at = meta
                .modified()
                .ok()
                .map(crate::infra::iso_time::from_system_time);
            let format = format_of(&bytes);
            // 解析不出来、或者既不是界面格式也不是 API 格式的一律不列：
            // 它们 `comfy_load_workflow` 一定失败（NOT_A_WORKFLOW）
            if format == "unknown" {
                continue;
            }
            out.push(WorkflowFile {
                name,
                path: path.to_string_lossy().to_string(),
                bytes: size,
                modified_at,
                format,
                sha256: sha256_hex(&bytes),
                prompt_id: None,
            });
        }
    }
    // ISO-8601 字符串的字典序就是时间序（同一时区、同一格式），不必再解析
    out.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    out.truncate(cap);
    out
}

/// 这份文件是界面格式、API 格式，还是读不出来。
///
/// 与 `WorkflowConvert.isUiWorkflow` / `HistoryEntry.isNodeGraph` 的判据**完全同一份** ——
/// 两边不一致就会出现"列表说能跑、加载说不是工作流"这种最难查的分歧。
pub(crate) fn format_of(bytes: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return "unknown".into();
    };
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return "unknown".into();
    };
    if crate::comfy::capture::history::is_node_graph(&root) {
        "api".into()
    } else if root.get("nodes").map(|n| n.is_array()).unwrap_or(false) {
        "ui".into()
    } else {
        "unknown".into()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
//  端点：GET /api/capture/workflows
// ---------------------------------------------------------------------------

/// 查询参数（与 Kotlin `CaptureRoutes` 逐字一致：`q` + `limit`，limit 缺省 50）。
#[derive(Debug, serde::Deserialize)]
pub struct WorkflowQuery {
    pub q: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /api/capture/locate` 的处理体。
pub fn locate_json(
    state: &crate::web::routes::AppState,
) -> Result<ComfyLocation, crate::web::routes::ApiError> {
    // 读配置失败不该让探测整个失败：那只是"有没有配过"这一个输入（Kotlin 用的是 runCatching）
    let configured = crate::comfy::capture::capture_config(state)
        .unwrap_or_default()
        .output_dir;
    Ok(locate(&state.project_root(), configured.as_deref()))
}

/// `GET /api/capture/workflows` 的处理体。
pub fn workflows_json(
    state: &crate::web::routes::AppState,
    query: WorkflowQuery,
) -> Result<serde_json::Value, crate::web::routes::ApiError> {
    // 读配置失败（首次启动还没建库）**不该**让"列目录"整个失败 ——
    // 那只影响三份来源里的第一份，后两份（探测、自动放行）更可靠（Kotlin 用的是 runCatching）。
    let output_dir = crate::comfy::capture::capture_config(state)
        .unwrap_or_default()
        .output_dir;

    let dirs = workflow_dirs(&state.project_root(), output_dir.as_deref());
    let limit = query.limit.unwrap_or(50);
    Ok(list_json(&dirs, query.q.as_deref(), limit, |sha| {
        // 库里已有的对应关系：`comfy_load_workflow` 用的是**同一把钥匙** `file:<sha256>`，
        // 所以这里天然幂等（查库失败按"没入库"处理，不能让一次查询失败炸掉整个列表）
        library_prompt_id(state, sha)
    }))
}

/// 拿 `file:<sha256>` 问库："这份文件已经在库里了吗"。
pub(crate) fn library_prompt_id(state: &crate::web::routes::AppState, sha: &str) -> Option<i64> {
    let run_key = format!("file:{sha}");
    state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT prompt_id FROM capture_runs WHERE run_key = ?1",
                    rusqlite::params![run_key],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .ok()
                .flatten())
        })
        .ok()
        .flatten()
}

/// 给模型 / 界面看的清单 JSON（形状与 Kotlin `ComfyWorkflowFiles.listJson` 逐字一致）。
///
/// `library_prompt_id` 由调用方注入（拿 sha256 问库）—— 这样磁盘扫描那部分
/// 可以在没有数据库的情况下单测。
pub fn list_json(
    dirs: &[PathBuf],
    query: Option<&str>,
    limit: usize,
    library_prompt_id: impl Fn(&str) -> Option<i64>,
) -> serde_json::Value {
    let mut files = scan(dirs, query, MAX_FILES);
    for f in files.iter_mut() {
        f.prompt_id = library_prompt_id(&f.sha256);
    }
    let total = files.len();
    let shown: Vec<&WorkflowFile> = files.iter().take(limit.clamp(1, MAX_FILES)).collect();

    let mut obj = serde_json::Map::new();
    obj.insert("count".into(), serde_json::json!(shown.len()));
    obj.insert("total".into(), serde_json::json!(total));
    obj.insert(
        "dirs".into(),
        serde_json::json!(dirs
            .iter()
            .map(|d| d.to_string_lossy().to_string())
            .collect::<Vec<_>>()),
    );
    obj.insert(
        "files".into(),
        serde_json::json!(shown
            .iter()
            .map(|f| {
                let mut m = serde_json::Map::new();
                m.insert("name".into(), serde_json::json!(f.name));
                m.insert("path".into(), serde_json::json!(f.path));
                m.insert("bytes".into(), serde_json::json!(f.bytes));
                m.insert("modifiedAt".into(), serde_json::json!(f.modified_at));
                m.insert("format".into(), serde_json::json!(f.format));
                m.insert("inLibrary".into(), serde_json::json!(f.prompt_id.is_some()));
                if let Some(id) = f.prompt_id {
                    m.insert("promptId".into(), serde_json::json!(id));
                }
                serde_json::Value::Object(m)
            })
            .collect::<Vec<_>>()),
    );
    obj.insert(
        "hint".into(),
        serde_json::json!(
            "这些工作流**不需要先跑一次**：拿到 path 直接 comfy_load_workflow(path=…) 读进库，\
             再用返回的 promptId 提交。inLibrary=true 的那几份库里已经有了（promptId 就是它）。"
        ),
    );
    if dirs.is_empty() {
        obj.insert(
            "message".into(),
            serde_json::json!(
                "没找到本机 ComfyUI 的 workflows 目录（`user\\<用户>\\workflows`）。\
                 可能是没装 ComfyUI，或者它的位置没被认出来 —— 让用户在「设置 → ComfyUI 自动捕获」里\
                 确认 ComfyUI 位置，或者直接把工作流文件的完整路径给过来。"
            ),
        );
    } else if shown.is_empty() {
        let dirs_text = dirs
            .iter()
            .map(|d| d.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let message = match query.map(str::trim).filter(|s| !s.is_empty()) {
            None => format!("这些目录里一个 .json 工作流都没有：{dirs_text}"),
            Some(q) => format!(
                "本机工作流里没有文件名匹配「{q}」的（共 {total} 份，可以不带 query 再列一次）"
            ),
        };
        obj.insert("message".into(), serde_json::json!(message));
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests;
