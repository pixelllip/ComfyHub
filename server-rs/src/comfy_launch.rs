//! ComfyHub **自己起 / 停**本机 ComfyUI 实例（2026-09-28 用户 bug「通过 CLI 启动 comfyui 总是失败」）。
//!
//! # 为什么不走 comfy-mcp 的 `launch_comfyui`
//!
//! `launch_comfyui` 是 comfy-cli 的薄壳（`comfy launch --background`），而 comfy-cli 1.20
//! 起实例时**一定要写两个项目外的文件**：
//!
//! | 写什么 | 谁写的 | 现场报错（本机实测 2026-09-28） |
//! | --- | --- | --- |
//! | `%LOCALAPPDATA%\comfy-cli\config.ini` | comfy-cli（记 background / background_log） | `PermissionError: [Errno 13] Permission denied: 'C:\Users\…\AppData\Local\comfy-cli\config.ini'` |
//! | `<ComfyUI>\user\comfyui_8188.log` | comfy-cli（后台实例的日志重定向） | `Could not open background log file … : [Errno 13] Permission denied` |
//!
//! 受限会话把"**映像在项目内**"的进程关在项目里（见 `docs/pitfalls.md` 那条总根因），
//! 而 MCP 的便携 python 就在 `<项目>\storage\ai\mcp\python` —— 所以这条路**必然**失败，
//! 且失败信息只有一句 `Error executing tool launch_comfyui`（真因埋在 stderr 的 rich 方框里）。
//! 我们替它补的 `USERPROFILE` 重定向只能过第一关：第二个文件由 comfy-cli 写死在
//! ComfyUI 工作区里，没有参数可改（见 `mcp.rs` 的 `apply_comfy_cli_write_redirection`）。
//!
//! 而 **ComfyUI 自己的 python 映像在项目外**（`<工作区>\.venv\Scripts\python.exe`），
//! 写它自己的目录毫无问题 —— 于是正确做法是：**ComfyHub 自己起**（总纲 O1：
//! "ComfyHub 自己拥有起实例这条路"）。
//!
//! # 起法（本机实测可用的那一条）
//!
//! ```text
//! cd <工作区>
//! <工作区>\.venv\Scripts\python.exe main.py \
//!     --extra-model-paths-config "<%APPDATA%>\Comfy Desktop\shared_model_paths.yaml" \
//!     --input-directory  <共享 input>  --output-directory <共享 output>  --port 8188
//! ```
//!
//! 三个路径参数**复用 [`crate::mcp::prepare_launch_with`]**（`launch_comfyui` 那条路也是它）——
//! 分头写必然漂移，而漂移的表现是"ComfyHub 起的实例看不到用户的模型"。
//!
//! 三条纪律（都踩过）：
//!
//! 1. **`PYTHONUTF8=1` / `PYTHONIOENCODING=utf-8` 必须给**：ComfyUI 会打 `✓`，而重定向出来的
//!    文件句柄继承 GBK 代码页 → `UnicodeEncodeError` **把进程抛死**（2026-09-26 那次
//!    "日志有头没有尾"的现场；`chcp 65001` 不管用，Python 只认这两个环境变量）。
//! 2. **日志落项目内**（`<根>\.run\comfyui-<端口>.log`）：日志句柄是**我们**（后端，映像在
//!    项目内的进程）开的，写项目外同样 Errno 13。
//! 3. **不许"起了就算成功"**：本模块一直等到 `GET /system_stats` 真的答话，并且
//!    **把子进程提前退出**（多半就是上面那个编码坑）与"等超时"分开如实报。
//!
//! # 停
//!
//! 只停**我们自己起的那一个**（`.run\comfyui-<端口>.pid` 记着 pid）：没有 pid 记录时
//! **如实说"这不是我起的、我不动它"** —— 用户的桌面实例（ComfyUI Desktop）有它自己的
//! 生命周期，被 AI 顺手杀掉是不可接受的。杀之前还要用 `tasklist` 核一眼那个 pid 现在
//! 是不是 python（pid 会被系统回收，不复核就可能误杀无辜进程）。
//!
//! # ⚠ 受限会话里"起得来"和"起得对"是两件事（2026-09-28 实测）
//!
//! 起实例的**子进程继承我们的令牌**：我们写不了的地方，它也写不了。实测现场是
//! "命令一模一样、手工起就绪、我们起就 100% CPU 卡在扫自定义节点"，
//! 而实例自己打的那行 `Failed to remove temp directory …: [WinError 5] 拒绝访问` 是铁证。
//! 逃逸那条路也堵着（WMI `拒绝访问`、`schtasks` `Access is denied`、
//! `Start-Process -WindowStyle Hidden` 照样继承）——**没有"换个方式拉起就没事"**。
//!
//! 所以起之前有一道 [`ensure_writable`] 闸门：写不进自己的目录就**当场如实拒绝**
//! （`COMFY_WRITE_BLOCKED` + 下一步让人自己做），而不是白起一个卡死的实例。
//! 判据与现场记录在 `tests/tmp-inherit-probe.rs`（那条临时探针）与 `docs/pitfalls.md`。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::routes::AppState;
use crate::tools::{str_arg, ToolFailure, ToolOutcome};

/// 等实例起来的上限（秒）：本机冷启动实测约 40~60s（加载自定义节点最慢）。
const DEFAULT_WAIT_SECONDS: u64 = 120;
const MAX_WAIT_SECONDS: u64 = 600;
/// 起来之后的探测节奏。
const POLL_MS: u64 = 500;
/// 失败时带回的日志尾巴（字符数）。
const LOG_TAIL_CHARS: usize = 1200;

/// 一次启动的**全部事实**（纯计算结果，方便用例直接断言）。
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// 有 `main.py` 的那个目录（`comfy launch` 的工作区）。
    pub workspace: PathBuf,
    /// 工作区 venv 里的 python（映像在项目外，写项目外不受限）。
    pub python: PathBuf,
    pub port: u16,
    /// 传给 `main.py` 的参数（含 Desktop 那三个 + `--port`）。
    pub args: Vec<String>,
    /// 实例 stdout/stderr 落到哪（**项目内**）。
    pub log: PathBuf,
    /// pid 记在哪（停实例时按它认"这是我起的"）。
    pub pid_file: PathBuf,
    /// 三个参数的来路说明（如实回给模型/用户）。
    pub note: Option<String>,
}

impl LaunchPlan {
    /// 给人看的一行摘要（工具正文与错误信息共用）。
    pub fn summary(&self) -> String {
        format!(
            "工作区 {} / python {} / 端口 {} / 日志 {}",
            self.workspace.display(),
            self.python.display(),
            self.port,
            self.log.display()
        )
    }
}

/// 从 `comfy_url` 里取端口（取不到就 8188，与 `capture::default_comfy_url` 同）。
pub fn port_of(comfy_url: &str) -> u16 {
    let rest = comfy_url.trim().trim_end_matches('/');
    let after_scheme = rest.split("://").nth(1).unwrap_or(rest);
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    host_port
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(8188)
}

/// 一个**能跑**的工作区：`main.py` + `comfy/`（= [`crate::comfy_dirs::looks_like_comfy`]
/// 的"源码形态"那一档）。
///
/// ⚠ 为什么不能只用 `looks_like_comfy`：它还有**第二种形态**（`output/` + `input/`/`models/`），
/// 那是给"找产物目录"用的。本机实测踩到：配置里写了 `…\ComfyUI-Shared\output`，
/// 于是 `find_home` 返回 `…\ComfyUI-Shared`（**共享数据根，没有 main.py**）——
/// 那是个合法的"任何形态"，但拿它去跑 main.py 只能是"文件不存在"。
pub fn looks_runnable(dir: &Path) -> bool {
    dir.join("main.py").is_file() && dir.join("comfy").is_dir()
}

/// 找一个**能跑**的 ComfyUI 工作区（顺序即优先级，全部要求 [`looks_runnable`]）：
///
/// 1. 显式参数 `workspace`（给了就必须能跑，否则当场如实报错，不悄悄换一个）；
/// 2. 环境变量（`COMFYHUB_COMFY_WORKSPACE` → `COMFYHUB_COMFY_HOME` / `COMFYUI_HOME` / `COMFYUI_PATH`；
///    与 [`crate::comfy_dirs`] 同一批键，脚本 / 高级用户用）；
/// 3. 项目内自带的 `<根>\comfyui`；
/// 4. 常见位置与 ComfyUI Desktop 的几套布局（[`crate::comfy_dirs`] 里那三个有界探测）；
/// 5. [`crate::comfy_dirs::find_home`]（任何形态，能跑就用）。
///
/// 都找不到就如实失败（`COMFY_NOT_FOUND`）并列出**试过哪些地方** —— 不猜路径。
pub fn resolve_workspace(
    project_root: &Path,
    configured_output: Option<&str>,
    explicit: Option<&str>,
) -> Result<PathBuf, ToolFailure> {
    if let Some(raw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let dir = PathBuf::from(raw);
        if looks_runnable(&dir) {
            return Ok(dir);
        }
        let (ok, why) = crate::comfy_dirs::looks_like_comfy(&dir);
        return Err(ToolFailure::new(
            "COMFY_NOT_FOUND",
            format!(
                "你给的 workspace「{}」不是一个**能跑**的 ComfyUI 目录：{}。\
                 要的是**有 main.py 与 comfy/ 的那个目录**（例如 `…\\ComfyUI-Installs\\ComfyUI\\ComfyUI`）—— \
                 只有 `output/` 的那个是**共享数据根**，拿去跑 main.py 只会得到「文件不存在」。",
                dir.display(),
                if ok { "它有 output/ 但没有 main.py".to_string() } else { why }
            ),
        ));
    }

    let mut tried: Vec<String> = Vec::new();
    let consider = |dir: Option<PathBuf>, label: &str, tried: &mut Vec<String>| -> Option<PathBuf> {
        let dir = dir?;
        if looks_runnable(&dir) {
            return Some(dir);
        }
        tried.push(format!("{label}（{}）", dir.display()));
        None
    };

    let from_env = [
        "COMFYHUB_COMFY_WORKSPACE",
        "COMFYHUB_COMFY_HOME",
        "COMFYUI_HOME",
        "COMFYUI_PATH",
    ]
    .into_iter()
    .find_map(|key| consider(crate::comfy_dirs::env_path(key), key, &mut tried));
    let found = from_env
        .or_else(|| consider(Some(project_root.join("comfyui")), "项目内的 comfyui", &mut tried))
        .or_else(|| {
            consider(Some(PathBuf::from("D:\\ComfyUI")), "常见位置 D:\\ComfyUI", &mut tried)
        })
        .or_else(|| {
            consider(
                Some(PathBuf::from("D:\\ComfyUI_windows_portable\\ComfyUI")),
                "免安装包",
                &mut tried,
            )
        })
        // ComfyUI Desktop 的几套布局（有界探测，与"找产物目录"共用同一批函数）
        .or_else(|| consider(crate::comfy_dirs::search_user_profile(), "用户目录下的 ComfyUI", &mut tried))
        .or_else(|| {
            consider(crate::comfy_dirs::search_desktop_installs(), "ComfyUI Desktop 的实例", &mut tried)
        })
        .or_else(|| {
            consider(crate::comfy_dirs::search_desktop_packages(), "便携包 / 整合包", &mut tried)
        })
        // 最后退回"任何形态"那一档（能跑就用；跑不了只能如实报）
        .or_else(|| consider(crate::comfy_dirs::find_home(project_root, configured_output), "已配置的目录", &mut tried));

    found.ok_or_else(|| {
        ToolFailure::new(
            "COMFY_NOT_FOUND",
            format!(
                "没找到**能跑**的 ComfyUI 工作区（要有 main.py + comfy/ 的那个目录）。试过：{}\
                 \n下一步：让用户用 ComfyUI Desktop 打开一次实例，或者用 workspace 参数直接给目录。",
                if tried.is_empty() { "（一处都没有）".to_string() } else { tried.join("、") }
            ),
        )
    })
}

/// 工作区里那个**能跑 ComfyUI** 的 python：`.venv\Scripts\python.exe` → `venv\Scripts\python.exe`。
pub fn resolve_python(workspace: &Path) -> Result<PathBuf, ToolFailure> {
    for venv in [".venv", "venv"] {
        let candidate = workspace.join(venv).join("Scripts").join("python.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
        // 非 Windows 布局（用例与将来移植用；本机发布路径是 Windows）
        let unix = workspace.join(venv).join("bin").join("python");
        if unix.is_file() {
            return Ok(unix);
        }
    }
    Err(ToolFailure::new(
        "COMFY_PYTHON_MISSING",
        format!(
            "工作区 {} 里没有虚拟环境 python（`.venv\\Scripts\\python.exe`）。\
             这个实例得先用 ComfyUI Desktop 起过一次、把依赖装好，ComfyHub 才起得动它 —— \
             **不要**拿别的 python 硬跑 main.py。",
            workspace.display()
        ),
    ))
}

/// 算出"该怎么起"（不改任何文件；`desktop` 由调用方注入，用例不必依赖真的 `%APPDATA%`）。
pub fn plan_with(
    project_root: &Path,
    workspace: PathBuf,
    port: u16,
    extra_args: &[String],
    desktop: Option<&crate::mcp::Desktop>,
) -> Result<LaunchPlan, ToolFailure> {
    let python = resolve_python(&workspace)?;
    // 三个路径参数**复用 launch_comfyui 那一份**（同一个真源）
    let prepared = crate::mcp::prepare_launch_with(&json!({ "extra_args": extra_args }), desktop.cloned());
    let mut args: Vec<String> = prepared
        .args
        .get("extra_args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !args.iter().any(|a| a == "--port") {
        args.push("--port".into());
        args.push(port.to_string());
    }
    let run_dir = project_root.join(".run");
    std::fs::create_dir_all(&run_dir).map_err(|e| {
        ToolFailure::new(
            "COMFY_START_FAILED",
            format!("建不出 {}：{e}（日志与 pid 都要落在这里）", run_dir.display()),
        )
    })?;
    Ok(LaunchPlan {
        workspace,
        python,
        port,
        args,
        log: run_dir.join(format!("comfyui-{port}.log")),
        pid_file: run_dir.join(format!("comfyui-{port}.pid")),
        note: prepared.note,
    })
}

/// 子进程**继承我们的令牌** —— 我们写不了的地方，它也写不了。
///
/// # 这条是本机实测出来的（2026-09-28），不是推断
///
/// 现场：同一个命令，手工（pwsh，映像在项目外）起 ComfyUI 40~60s 就绪；
/// 从 `cargo test`（映像在项目内）起，实例**100% CPU 卡在"扫自定义节点"那一步**
/// （日志停在 `No OpenGL_accelerate…`），而且它自己打了一行
/// `Failed to remove temp directory …\ComfyUI\temp: [WinError 5] 拒绝访问`。
///
/// 量出来的判据（`tests/tmp-inherit-probe.rs` 的两条探针）：
/// * 映像**在项目外**的 python，由**受限于项目的父进程**拉起时，写项目外照样
///   `PermissionError [Errno 13]` —— 限制是**继承**的（原先记成"按映像判"是不完整的）；
/// * 逃逸那条路也堵着：WMI `Win32_Process.Create` → `拒绝访问`；
///   `schtasks /create` → `Access is denied`；`Start-Process -WindowStyle Hidden`
///   不脱离作业对象，照样继承。
///
/// 所以在这种会话里**任何由我们拉起的实例都是残废的**（写不了自己的 temp/output/模型目录）。
/// 与其白起一个卡死的实例（用户等两分钟、看到 100% CPU、以为 ComfyUI 坏了），
/// 不如**当场如实拒绝**并说清下一步。
fn dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".comfyhub-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new().create_new(true).write(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 起实例前的"写得了吗"闸门（**可注入**判据，方便用例不依赖真机器）。
///
/// 探两处：**工作区本身**（ComfyUI 起手就写它的 `temp/`）与**产物目录**（被自动捕获盯着的
/// 那个 output）。两处都写得进才放行。
pub fn ensure_writable(
    plan: &LaunchPlan,
    output_dir: Option<&Path>,
    writable: &dyn Fn(&Path) -> bool,
) -> Result<(), ToolFailure> {
    let mut blocked: Vec<String> = Vec::new();
    if !writable(&plan.workspace) {
        blocked.push(format!("工作区 {}（实例起手就要写它的 temp/）", plan.workspace.display()));
    }
    if let Some(out) = output_dir {
        if !writable(out) {
            blocked.push(format!("产物目录 {}（跑出来的图要落在这里）", out.display()));
        }
    }
    if blocked.is_empty() {
        return Ok(());
    }
    Err(ToolFailure::new(
        "COMFY_WRITE_BLOCKED",
        format!(
            "**不起**：这个会话不允许本进程（因此也不允许我起的子进程）写 ComfyUI 自己的目录 —— {}。\n\
             这不是 ComfyUI 坏了，也不是路径配错了：子进程**继承**当前进程的限制，\
             实测 WMI 的 `Win32_Process.Create`（拒绝访问）与 `schtasks /create`（Access is denied）\
             都被系统挡掉，所以没有「换个方式拉起就没事」的退路；硬起只会得到一个卡死的实例\
             （实测：100% CPU 卡在扫自定义节点，日志停在 `No OpenGL_accelerate…`）。\n\
             **下一步（让用户自己做）**：让用户**双击 ComfyUI Desktop** 把实例起起来\
             （那是由系统直接拉起的，不吃这个限制）——起完我这边用 `comfy_get_status` 就能接着干活；\
             或者把 ComfyHub 从受限终端里挪出来（正常双击启动）之后再用 `comfy_start`。",
            blocked.join("；")
        ),
    ))
}

/// `comfy_start`：把实例起到"`/system_stats` 真的答话"为止。
pub async fn start(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let config = crate::capture::capture_config(state)
        .map_err(|e| ToolFailure::new("CONFIG_ERROR", format!("读 ComfyUI 配置失败：{}", e.human())))?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();

    // 已经在跑就**不重复起**（同一端口硬起第二个必失败，而用户其实只想用它）
    if let Some(stats) = probe(&base).await {
        return Ok(running_outcome(&base, &stats, None));
    }

    let extra: Vec<String> = arguments
        .get("extraArgs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let port = port_of(&base);
    let workspace = resolve_workspace(
        &state.project_root(),
        config.output_dir.as_deref(),
        str_arg(arguments, "workspace").as_deref(),
    )?;
    let desktop = crate::mcp::desktop(None);
    let plan = plan_with(&state.project_root(), workspace, port, &extra, desktop.as_ref())?;
    // 先确认"子进程写得进它自己的目录"（写不进就别白起一个卡死的实例，见 ensure_writable）
    let output_dir = desktop.as_ref().and_then(|d| d.output_dir.clone());
    ensure_writable(&plan, output_dir.as_deref(), &dir_writable)?;
    let wait = arguments
        .get("waitSeconds")
        .and_then(Value::as_i64)
        .map(|v| v.clamp(5, MAX_WAIT_SECONDS as i64) as u64)
        .unwrap_or(DEFAULT_WAIT_SECONDS);

    let started = Instant::now();
    let mut child = spawn_detached(&plan)?;
    let pid = child.id();
    // 先记 pid 再等：等的过程里用户点「停止」也要停得掉
    let _ = write_pid_file(&plan, pid);

    let outcome = wait_until_up(&base, &plan, &mut child, wait, started).await;
    match outcome {
        Ok(stats) => {
            let mut out = running_outcome(&base, &stats, Some((&plan, pid, started.elapsed())));
            if let Some(note) = &plan.note {
                out.payload["note"] = json!(note);
                out.content.push('\n');
                out.content.push_str(note);
            }
            Ok(out)
        }
        Err(failure) => {
            // 起不来就别留下一个"以为在跑"的 pid 记录
            let _ = std::fs::remove_file(&plan.pid_file);
            Err(failure)
        }
    }
}

/// `comfy_stop`：停**我们自己起的**那一个实例。
pub async fn stop(state: &AppState) -> Result<ToolOutcome, ToolFailure> {
    let config = crate::capture::capture_config(state)
        .map_err(|e| ToolFailure::new("CONFIG_ERROR", format!("读 ComfyUI 配置失败：{}", e.human())))?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();
    let port = port_of(&base);
    let pid_file = state.project_root().join(".run").join(format!("comfyui-{port}.pid"));
    let pid = read_pid_file(&pid_file).ok_or_else(|| {
        ToolFailure::new(
            "COMFY_NOT_OURS",
            format!(
                "没有 {} —— 这个实例**不是 ComfyHub 起的**，我不动它。\
                 用户自己用 ComfyUI Desktop 起的实例请让他自己在那个窗口里关（或者用 Desktop 的退出菜单）。",
                pid_file.display()
            ),
        )
    })?;

    let image = process_image_name(pid);
    match image {
        Some(name) if name.to_ascii_lowercase().contains("python") => {}
        Some(name) => {
            let _ = std::fs::remove_file(&pid_file);
            return Err(ToolFailure::new(
                "COMFY_NOT_OURS",
                format!(
                    "pid {pid} 现在是「{name}」，不是我们起的那个 python（pid 被系统回收过？）—— \
                     我没有动它，pid 记录也清掉了。"
                ),
            ));
        }
        None => {
            let _ = std::fs::remove_file(&pid_file);
            return Err(ToolFailure::new(
                "COMFY_NOT_OURS",
                format!("pid {pid} 已经不在了（实例大概自己退过），pid 记录已清掉；不用再停。"),
            ));
        }
    }

    let started = Instant::now();
    let killed = kill_tree(pid)?;
    // 等端口真的不再答话（只杀到"命令返回"不算停干净）
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if probe(&base).await.is_none() {
            let _ = std::fs::remove_file(&pid_file);
            let payload = json!({
                "stopped": true,
                "pid": pid,
                "port": port,
                "comfyUrl": base,
                "elapsedMs": started.elapsed().as_millis() as u64,
            });
            return Ok(ToolOutcome::new(
                format!("已停掉 ComfyHub 起的那个实例（pid {pid}，{base} 不再答话）。"),
                payload,
            ));
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    Err(ToolFailure::new(
        "COMFY_STOP_FAILED",
        format!(
            "对 pid {pid} 执行了停止（{killed}），但 30 秒后 {base} 还在答话 —— \
             可能还有另一个实例占着那个端口（用 `tasklist | findstr python` 看一眼），我没有硬杀别的东西。"
        ),
    ))
}

// ---------------------------------------------------------------------------
//  内部：探测 / 起进程 / pid
// ---------------------------------------------------------------------------

/// 问一次 `GET /system_stats`；答话就是"实例起来了"（连不上 / 非 200 → `None`）。
async fn probe(base: &str) -> Option<Value> {
    crate::comfy_client::get_json(&format!("{base}/system_stats")).await.ok()
}

/// 一直在等，直到实例答话（或子进程提前退出 / 超时）。
async fn wait_until_up(
    base: &str,
    plan: &LaunchPlan,
    child: &mut std::process::Child,
    wait_seconds: u64,
    started: Instant,
) -> Result<Value, ToolFailure> {
    let deadline = started + Duration::from_secs(wait_seconds);
    loop {
        if let Some(stats) = probe(base).await {
            return Ok(stats);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let tail = log_tail(&plan.log);
            return Err(ToolFailure::new(
                "COMFY_START_FAILED",
                format!(
                    "ComfyUI 进程刚起来就退出了（{status}，{:.1}s）—— {}\n日志尾巴（{}）：\n{tail}",
                    started.elapsed().as_secs_f64(),
                    plan.summary(),
                    plan.log.display()
                ),
            ));
        }
        if Instant::now() >= deadline {
            let tail = log_tail(&plan.log);
            return Err(ToolFailure::new(
                "COMFY_START_TIMEOUT",
                format!(
                    "等了 {wait_seconds}s 还是连不上 {base}（进程还活着、多半还在加载自定义节点）—— {}\n\
                     再来一次会接着等；日志尾巴（{}）：\n{tail}",
                    plan.summary(),
                    plan.log.display()
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

/// 静默、脱离式地起实例（不弹控制台窗口；日志落项目内）。
fn spawn_detached(plan: &LaunchPlan) -> Result<std::process::Child, ToolFailure> {
    let log = std::fs::File::create(&plan.log).map_err(|e| {
        ToolFailure::new(
            "COMFY_START_FAILED",
            format!("建不出日志文件 {}：{e}", plan.log.display()),
        )
    })?;
    let err_log = log.try_clone().map_err(|e| {
        ToolFailure::new("COMFY_START_FAILED", format!("复制日志句柄失败：{e}"))
    })?;
    let mut command = std::process::Command::new(&plan.python);
    command
        .arg("main.py")
        .args(&plan.args)
        // 工作目录 = 工作区（comfy-cli 也是这么起的；等价于 `--base-directory`，但少一个参数）
        .current_dir(&plan.workspace)
        // ① 这两条是"日志有头没有尾"那个坑的解药（AGENTS/pitfalls：ComfyUI 会打 ✓）
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err_log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：控制台程序默认会拿到自己的窗口（AGENTS §2 那条纪律）
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // CREATE_NEW_PROCESS_GROUP：与后端自己的 Ctrl 事件脱钩（后端被 Ctrl+C 收走时
        // 实例不该跟着一起死 —— 它是用户要用的长跑进程）
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map_err(|e| {
        ToolFailure::new(
            "COMFY_START_FAILED",
            format!("起 ComfyUI 失败（{} main.py）：{e}", plan.python.display()),
        )
    })
}

/// 进程现在叫什么映像（不在就 `None`）。Windows 用 `tasklist`；其它平台返回 `None`。
fn process_image_name(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let first = text.lines().next()?.trim().to_string();
        // 没有匹配时 tasklist 打的是「信息: 没有运行的任务匹配指定标准。」
        if !first.starts_with('"') {
            return None;
        }
        let name = first.trim_start_matches('"').split('"').next()?.to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// 结束整棵进程树（ComfyUI 会再起子进程/子线程，只杀父会留下占着显存的孩子）。
fn kill_tree(pid: u32) -> Result<String, ToolFailure> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| ToolFailure::new("COMFY_STOP_FAILED", format!("taskkill 起不来：{e}")))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return Ok(text.trim().replace('\n', " / "));
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Err(ToolFailure::new(
            "COMFY_STOP_FAILED",
            "这个平台还不支持停实例（ComfyHub 目前只在 Windows 上管实例）。",
        ))
    }
}

fn write_pid_file(plan: &LaunchPlan, pid: u32) -> std::io::Result<()> {
    let mut file = std::fs::File::create(&plan.pid_file)?;
    writeln!(
        file,
        "pid={pid}\nworkspace={}\npython={}\nport={}",
        plan.workspace.display(),
        plan.python.display(),
        plan.port
    )
}

fn read_pid_file(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// 日志尾巴（**按字符**截，别把 UTF-8 切坏）。
fn log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return "（日志还没写出来）".to_string();
    };
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(LOG_TAIL_CHARS);
    chars[start..].iter().collect()
}

/// "已经在跑 / 刚起来"的统一回报（**同一份措辞**：模型与用户看到的是同一件事）。
fn running_outcome(base: &str, stats: &Value, fresh: Option<(&LaunchPlan, u32, Duration)>) -> ToolOutcome {
    let system = stats.get("system").cloned().unwrap_or(Value::Null);
    let mut payload = json!({
        "started": true,
        "alreadyRunning": fresh.is_none(),
        "comfyUrl": base,
        "comfyReachable": true,
        "comfyuiVersion": system.get("comfyui_version").cloned().unwrap_or(Value::Null),
        "pythonVersion": system.get("python_version").cloned().unwrap_or(Value::Null),
        "device": stats
            .get("devices")
            .and_then(Value::as_array)
            .and_then(|d| d.first())
            .and_then(|d| d.get("name"))
            .cloned()
            .unwrap_or(Value::Null),
    });
    let text = match fresh {
        Some((plan, pid, elapsed)) => {
            payload["pid"] = json!(pid);
            payload["port"] = json!(plan.port);
            payload["workspace"] = json!(plan.workspace.display().to_string());
            payload["python"] = json!(plan.python.display().to_string());
            payload["log"] = json!(plan.log.display().to_string());
            payload["pidFile"] = json!(plan.pid_file.display().to_string());
            payload["args"] = json!(plan.args);
            payload["elapsedMs"] = json!(elapsed.as_millis() as u64);
            format!(
                "ComfyHub 自己把 ComfyUI 起起来了（{:.1}s，pid {pid}）：{}\n\
                 · 日志：{}（**项目内** —— comfy-cli 那条路写不出来，见下）\n\
                 · 停在 `comfy_stop`（只停这一个；用户自己的 Desktop 实例不会被碰）\n\
                 · 起法绕开了 comfy-cli：它启动时要写 `%LOCALAPPDATA%\\comfy-cli\\config.ini` 与 \
                 `<ComfyUI>\\user\\comfyui_8188.log`，受限会话里那两个必然 Errno 13",
                elapsed.as_secs_f64(),
                plan.summary(),
                plan.log.display()
            )
        }
        None => format!(
            "ComfyUI 已经在跑（{base}），没有再起一个 —— 直接用就行。\
             （要知道状态与队列用 `comfy_get_status`；要重启就先 `comfy_stop` 再 `comfy_start`。）"
        ),
    };
    ToolOutcome::new(text, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用例的临时目录**放在项目内**（`server-rs\target\.scratch`）。
    ///
    /// ⚠ 不用 `std::env::temp_dir()`：受限会话里"映像在项目内"的进程写 `%TEMP%` 会被拒
    /// （本机实测 `cargo test` 的二进制就在 `server-rs\target\` 里，`mcp::tests` 那几条
    /// 用例正是因此在本机红）—— 用例不该依赖"这台机器的 %TEMP% 可写"。
    fn scratch(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(".scratch")
            .join(format!("comfy-launch-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn port_comes_from_the_configured_url() {
        assert_eq!(port_of("http://127.0.0.1:8188"), 8188);
        assert_eq!(port_of("http://127.0.0.1:8188/"), 8188);
        assert_eq!(port_of("http://127.0.0.1"), 8188, "没写端口就是默认那个");
        assert_eq!(port_of("  http://192.168.1.5:8199  "), 8199);
        assert_eq!(port_of(""), 8188);
    }

    /// 三个路径参数的注入复用 `launch_comfyui` 那一份（同一个真源），并且**端口自己补**。
    #[test]
    fn plan_reuses_the_desktop_flags_and_adds_the_port() {
        let tmp = scratch("plan");
        let ws = tmp.join("ComfyUI");
        std::fs::create_dir_all(ws.join(".venv").join("Scripts")).unwrap();
        std::fs::write(ws.join("main.py"), "print(1)").unwrap();
        std::fs::create_dir_all(ws.join("comfy")).unwrap();
        std::fs::write(ws.join(".venv").join("Scripts").join("python.exe"), "").unwrap();
        let desktop_dir = tmp.join("desktop");
        std::fs::create_dir_all(&desktop_dir).unwrap();
        let yaml = desktop_dir.join("shared_model_paths.yaml");
        std::fs::write(&yaml, "a: b").unwrap();
        let input = tmp.join("in");
        let output = tmp.join("out");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            desktop_dir.join("settings.json"),
            json!({ "inputDir": input, "outputDir": output }).to_string(),
        )
        .unwrap();
        let desktop = crate::mcp::desktop(Some(desktop_dir)).expect("配置要读得到");

        let plan = plan_with(&tmp, ws.clone(), 8188, &[], Some(&desktop)).unwrap();
        assert_eq!(plan.port, 8188);
        assert_eq!(plan.python, ws.join(".venv").join("Scripts").join("python.exe"));
        assert!(plan.args.contains(&"--extra-model-paths-config".to_string()), "{:?}", plan.args);
        assert!(plan.args.contains(&yaml.display().to_string()), "{:?}", plan.args);
        assert!(plan.args.contains(&"--input-directory".to_string()));
        assert!(plan.args.contains(&"--output-directory".to_string()));
        assert_eq!(
            plan.args.iter().rev().take(2).cloned().collect::<Vec<_>>(),
            vec!["8188".to_string(), "--port".to_string()],
            "端口要补在最后：{:?}",
            plan.args
        );
        // 日志/pid **必须在项目内**（受限会话里项目外写不了）
        assert!(plan.log.starts_with(&tmp), "{:?}", plan.log);
        assert!(plan.pid_file.starts_with(&tmp));

        // 模型自己带了 --port 就不重复补
        let explicit = plan_with(&tmp, ws.clone(), 8188, &["--port".into(), "8199".into()], Some(&desktop)).unwrap();
        assert_eq!(
            explicit.args.iter().filter(|a| *a == "--port").count(),
            1,
            "{:?}",
            explicit.args
        );

        // 没有 venv → 如实失败（不猜别的 python）
        let bare = tmp.join("bare");
        std::fs::create_dir_all(bare.join("comfy")).unwrap();
        std::fs::write(bare.join("main.py"), "print(1)").unwrap();
        let err = plan_with(&tmp, bare, 8188, &[], Some(&desktop)).unwrap_err();
        assert_eq!(err.code, "COMFY_PYTHON_MISSING");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 不是 ComfyUI 的目录**不许**被当成工作区（否则会拿它去跑 main.py）。
    ///
    /// ⚠ 第二种情况是**本机真踩到的**：`…\ComfyUI-Shared`（配置里写的 output 的父目录）
    /// 有 `output/` + `models/`，`looks_like_comfy` 认它是"任何形态"的 ComfyUI，
    /// 但它**没有 main.py** —— 拿它起实例只会得到"文件不存在"。
    #[test]
    fn explicit_workspace_must_be_runnable() {
        let tmp = scratch("ws");
        let err = resolve_workspace(&tmp, None, Some(&tmp.display().to_string())).unwrap_err();
        assert_eq!(err.code, "COMFY_NOT_FOUND");

        let shared = tmp.join("ComfyUI-Shared");
        for sub in ["output", "input", "models"] {
            std::fs::create_dir_all(shared.join(sub)).unwrap();
        }
        assert!(crate::comfy_dirs::looks_like_comfy(&shared).0, "前提：它算'任何形态'的 ComfyUI");
        let err = resolve_workspace(&tmp, None, Some(&shared.display().to_string())).unwrap_err();
        assert_eq!(err.code, "COMFY_NOT_FOUND");
        assert!(err.message.contains("共享数据根"), "要说清为什么不行：{}", err.message);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pid_file_round_trip() {
        let tmp = scratch("pid");
        let path = tmp.join("comfyui-8188.pid");
        let plan = LaunchPlan {
            workspace: tmp.clone(),
            python: tmp.join("python.exe"),
            port: 8188,
            args: vec![],
            log: tmp.join("x.log"),
            pid_file: path.clone(),
            note: None,
        };
        write_pid_file(&plan, 4321).unwrap();
        assert_eq!(read_pid_file(&path), Some(4321));
        assert_eq!(read_pid_file(&tmp.join("nope.pid")), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **起之前**那道闸门：写不进自己的目录就当场拒（别白起一个卡死的实例）。
    ///
    /// 判据是可注入的，所以这条用例不依赖"这台机器当前受不受限"。
    #[test]
    fn write_gate_refuses_before_spawning_a_crippled_instance() {
        let tmp = scratch("gate");
        let ws = tmp.join("ComfyUI");
        let out = tmp.join("out");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let plan = LaunchPlan {
            workspace: ws.clone(),
            python: ws.join("python.exe"),
            port: 8188,
            args: vec![],
            log: tmp.join("x.log"),
            pid_file: tmp.join("x.pid"),
            note: None,
        };

        // 两处都写得进 → 放行
        assert!(ensure_writable(&plan, Some(&out), &|_| true).is_ok());

        // 工作区写不进 → 拒，而且要说清下一步（视觉上"起不来"和"不许起"必须能分辨）
        let err = ensure_writable(&plan, None, &|p| p != ws).unwrap_err();
        assert_eq!(err.code, "COMFY_WRITE_BLOCKED");
        assert!(err.message.contains("双击 ComfyUI Desktop"), "要给出下一步：{}", err.message);
        assert!(err.message.contains("继承"), "要说清是继承来的限制：{}", err.message);

        // 产物目录写不进 → 也拒，并点名是哪个目录
        let err = ensure_writable(&plan, Some(&out), &|p| p != out).unwrap_err();
        assert_eq!(err.code, "COMFY_WRITE_BLOCKED");
        assert!(err.message.contains(&out.display().to_string()), "{}", err.message);

        // 真实判据在项目内必须为真（项目内写不了的话，整个后端都没法干活了）
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        assert!(dir_writable(&project), "项目内目录应当写得进：{}", project.display());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
