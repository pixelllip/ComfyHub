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
//! 三个路径参数**复用 [`crate::tools::mcp::prepare_launch_with`]**（`launch_comfyui` 那条路也是它）——
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
//! # ⚠ 写不进工作区外：**限制按「映像位置」判，不按路径、也不按会话**（2026-09-29 逐条量过）
//!
//! 起实例的**子进程继承我们的令牌**：我们写不了的地方，它也写不了。实测现场是
//! "命令一模一样、手工起就绪、我们起就 100% CPU 卡在扫自定义节点"，
//! 而实例自己打的那行 `Failed to remove temp directory …: [WinError 5] 拒绝访问` 是铁证。
//! 所以起之前有一道 [`ensure_writable`] 闸门：写不进自己的目录就**当场如实拒绝**
//! （`COMFY_WRITE_BLOCKED`），而不是白起一个卡死的实例。
//!
//! ## 判据（这四条是 2026-09-29 用同一个二进制逐条对照量出来的，别的都排除了）
//!
//! | 怀疑对象 | 实测 | 结论 |
//! | --- | --- | --- |
//! | 目录 ACL | 诊断脚本对两个目录报 `NOT_THIS_CLASS`（无 deny ACE、无 package ACE，`WRITE_DAC`/`WRITE_OWNER` 都在） | 无关 |
//! | 路径配错 / 用户身份 / Defender | 同用户同目录，`powershell` 写得进；Defender 无受控文件夹规则 | 无关 |
//! | **映像在工作区内** | **同一个二进制逐字节相同：放工作区内 → 写 `%TEMP%`、`D:\Comfy-Desktop\…` 全 `os error 5`；`copy` 到 `%TEMP%` → 哪儿都能写** | ✅ 就是它 |
//! | 从哪个会话 / 终端起 | 双击也一样被限 | 无关（限制在令牌里，跟会话走没用） |
//!
//! 另外两条**并存的**事实（别混成一条）：
//! * **子进程继承父进程的限制** —— 受限父进程拉起的、工作区外的 python 照样被拒
//!   （所以"换个 ComfyUI 自带的 python 就好"是错的）；
//! * **WMI / schtasks 被拒只是受限的表现**，不是系统策略，换方式拉起逃不掉 —— 但
//!   `Start-SilentProcess` 的三档**实测都能起**（`wmi-hidden` / `wmi-vbs` / `direct-hidden`
//!   写同一个探针都是 OK），**spawn 档位与写权限无关**。
//!
//! 所以真正的修法只有一个：**把后端映像挪出工作区**（`COMFYHUB_SERVER_EXE`，或用发布包）。
//! 留证交给 [`crate::infra::confinement`] —— `GET /api/system/confinement`（界面上有横幅）看得见，
//! 沿用 `docs/pitfalls.md` 第 151/154 条已经定下的那条纪律。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::web::routes::AppState;
use crate::tools::tools::{str_arg, ToolFailure, ToolOutcome};

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

/// 一个**能跑**的工作区：`main.py` + `comfy/`（= [`crate::comfy::comfy_dirs::looks_like_comfy`]
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
///    与 [`crate::comfy::comfy_dirs`] 同一批键，脚本 / 高级用户用）；
/// 3. 项目内自带的 `<根>\comfyui`；
/// 4. 常见位置与 ComfyUI Desktop 的几套布局（[`crate::comfy::comfy_dirs`] 里那三个有界探测）；
/// 5. [`crate::comfy::comfy_dirs::find_home`]（任何形态，能跑就用）。
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
        let (ok, why) = crate::comfy::comfy_dirs::looks_like_comfy(&dir);
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
    .find_map(|key| consider(crate::comfy::comfy_dirs::env_path(key), key, &mut tried));
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
        .or_else(|| consider(crate::comfy::comfy_dirs::search_user_profile(), "用户目录下的 ComfyUI", &mut tried))
        .or_else(|| {
            consider(crate::comfy::comfy_dirs::search_desktop_installs(), "ComfyUI Desktop 的实例", &mut tried)
        })
        .or_else(|| {
            consider(crate::comfy::comfy_dirs::search_desktop_packages(), "便携包 / 整合包", &mut tried)
        })
        // 最后退回"任何形态"那一档（能跑就用；跑不了只能如实报）
        .or_else(|| consider(crate::comfy::comfy_dirs::find_home(project_root, configured_output), "已配置的目录", &mut tried));

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
    desktop: Option<&crate::tools::mcp::Desktop>,
) -> Result<LaunchPlan, ToolFailure> {
    let python = resolve_python(&workspace)?;
    // 三个路径参数**复用 launch_comfyui 那一份**（同一个真源）
    let prepared = crate::tools::mcp::prepare_launch_with(&json!({ "extra_args": extra_args }), desktop.cloned());
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
/// # 限制是"继承"+"按映像位置"，两条并存（2026-09-28 / 2026-09-29 实测）
///
/// 现场：同一个命令，手工（正常终端）起 ComfyUI 40~60s 就绪；
/// 从**映像在工作区内的后端**再拉它，实例**100% CPU 卡在"扫自定义节点"那一步**
/// （日志停在 `No OpenGL_accelerate…`），而且它自己打了一行
/// `Failed to remove temp directory …\ComfyUI\temp: [WinError 5] 拒绝访问`。
///
/// 量出来的判据（文件头那张表是完整的，这里只留与本函数有关的）：
/// * **按映像位置**：后端 exe 在工作区内 → 写工作区外全 `os error 5`；同一二进制复制出去 → 全通。
///   所以换 ComfyUI 自带的 python **没用**（后端自己就写不了），换 spawn 档位也没用。
/// * **按继承**：受限父进程拉起的、工作区外的 python 照样被拒（`tests/tmp-inherit-probe.rs`）。
///   所以"让用户双击 Desktop"那条路之所以有效，是因为它的 python **映像在 ComfyUI 那边**
///   **且**父进程（Desktop）不受限 —— 两个前提缺一个就不成立。
///
/// 所以在这种后端里**任何由我们拉起的实例都是残废的**（写不了自己的 temp/output/模型目录）。
/// 与其白起一个卡死的实例（用户等两分钟、看到 100% CPU、以为 ComfyUI 坏了），
/// 不如**当场如实拒绝**并说清修法（把后端映像挪出工作区 —— 见下面 `ensure_writable` 的文案）。
fn dir_writable(dir: &Path) -> bool {
    crate::infra::confinement::dir_writable(dir)
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
            "**不起**：本进程写不进项目外的目录 —— {blocked}。\n\
             \n\
             **真因是完整性标签（Low IL），不是 ComfyUI 坏了、也不是路径配错了**：\n\
             工作区里的产物带着 **Low 完整性标签**，Windows 创建进程时取 **min(父进程, 映像)**，\n\
             映像标签是 Low → 进程令牌就是 Low → Low 写不了工作区外的 Medium 对象（一律 os error 5）。\n\
             源码树布局下后端 exe 就在工作区里 —— 于是它写不了 ComfyUI 自己的 `temp/`，\n\
             硬起只会得到一个卡死的实例（实测：100% CPU 卡在扫自定义节点，日志停在 `No OpenGL_accelerate…`）。\n\
             \n\
             **修法（三选一，内容照旧全在它自己的文件夹里）**：\n\
             1. 把整套 ComfyHub 放到工作区外（发布包），从那儿启动：\n\
             ```\n\
             pwsh -File scripts\\pack-release.ps1 -OutDir D:\\dist\\ComfyHub\n\
             ```\n\
             然后运行 `D:\\dist\\ComfyHub\\viewer.exe` —— App 会自己把 `<包根>\\backend\\` 里的后端拉起来。\n\
             发布包的数据在 `<包根>\\.db\\` / `storage\\` / `.run\\` —— \n\
             **既不受限，内容也仍然全在它自己的文件夹内**（这正是我们要的，不要改成往 `%LOCALAPPDATA%` \n\
             之类的系统位置拷）。\n\
             2. 已有工作区外的构建产物：`$env:COMFYHUB_SERVER_EXE` = 那个 `comfyhub-server-rs.exe` 的完整路径。\n\
             3. 就地修标签：`pwsh -File scripts\\backend.ps1 start`（起之前把产物的 Low 标签修回 Medium）。\n\
             ⚠ **当前会话本身是 Low 时修不动**（Low 写不了向上的标签）—— 那就只剩 1/2 两条路。\n\
             \n\
             **判据（2026-09-29/30 逐条量过，别再当成别的毛病）**：同一个二进制**逐字节相同**，\n\
             映像放在工作区内 → 写 `%TEMP%`、`D:\\Comfy-Desktop\\…` 全部 `os error 5`，只写得进工作区；\n\
             `copy` 到 `%TEMP%` 再跑 → **哪儿都能写**（那里没有 Low 标签，拷出去的文件继承到的是 Medium）。\n\
             所以：\n\
             · 与**路径配错、目录 ACL、用户身份、Defender** 都无关（诊断脚本查这些目录的 ACL 是 `NOT_THIS_CLASS`，\n\
               无 deny ACE、`WRITE_DAC`/`WRITE_OWNER` 都在；Defender 也没有受控文件夹规则）；\n\
             · 与**谁拉起 / 从哪个终端起**也无关 —— 决定权在**映像自己的标签**\n\
               （WMI / 计划任务 / Explorer 逐条量过都一样）；但注意 **min(父进程, 映像)**：\n\
               链路上任何一环是 Low，子进程照样掉回 Low（`viewer.exe` 也带着 Low 标签时，App 里点启动没用）；\n\
             · **把映像标签修回 Medium（位置一个字节不动）→ 立刻哪儿都能写**（2026-09-30 实测）——\n\
               这就是修法 3 的依据，也是 release 构建顺手做归一的原因（见 `windows\\cmake\\bundle_backend.cmake`）。\n\
             \n\
             **另一条也行**：让用户自己**双击 ComfyUI Desktop** 起实例（它的 python 映像在 ComfyUI 那边，不受限），\n\
             起完这边用 `comfy_get_status` 就能接着干活。\n\
             \n\
             `GET /api/system/confinement` 会给出同一份结论与判据（界面上有横幅，别再靠猜）。",
            blocked = blocked.join("；")
        ),
    ))
}

/// `comfy_start`：把实例起到"`/system_stats` 真的答话"为止。
pub async fn start(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let config = crate::comfy::capture::capture_config(state)
        .map_err(|e| ToolFailure::new("CONFIG_ERROR", format!("读 ComfyUI 配置失败：{}", e.human())))?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();

    // 已经在跑就**不重复起**（同一端口硬起第二个必失败，而用户其实只想用它）。
    // ⚠ 这里也要如实说"它读哪个 input"：那不是我们起的，只有**上次留档**或**问实例**知道
    //   —— 投放工具会自己去问（`image_edit::target_input_dir`），这里的 payload 是给人看的。
    if let Some(stats) = probe(&base).await {
        let known = launched_input_dir(&state.project_root(), port_of(&base));
        return Ok(running_outcome(&base, &stats, None, known));
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
    let desktop = crate::tools::mcp::desktop(None);
    let plan = plan_with(&state.project_root(), workspace, port, &extra, desktop.as_ref())?;
    // 先确认"子进程写得进它自己的目录"（写不进就别白起一个卡死的实例，见 ensure_writable）
    let output_dir = desktop.as_ref().and_then(|d| d.output_dir.clone());
    // 顺手留证：探一次就把"本进程受不受限"落到 `.run\confinement.json`，doctor 靠它报出来 ——
    // 沿用 pitfalls 151/154 的纪律（doctor 看不出这一条，用户就只能一直靠猜）。
    // 探到正常时 `refresh` 会把旧标记清掉，所以每次调 comfy_start 都顺手校正一次。
    crate::infra::confinement::refresh(state);
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
    // 把"这个实例读哪个 input"一起记下来（投放侧要照它对上，见 write_launch_record）
    let _ = write_launch_record(&plan, pid);

    let outcome = wait_until_up(&base, &plan, &mut child, wait, started).await;
    match outcome {
        Ok(stats) => {
            let known = crate::tools::mcp::mounted_input_dir(&plan.args);
            let mut out = running_outcome(&base, &stats, Some((&plan, pid, started.elapsed())), known);
            if let Some(note) = &plan.note {
                out.payload["note"] = json!(note);
                out.content.push('\n');
                out.content.push_str(note);
            }
            Ok(out)
        }
        Err(failure) => {
            // 起不来就别留下一个"以为在跑"的 pid / 留档
            let _ = std::fs::remove_file(&plan.pid_file);
            let _ = std::fs::remove_file(record_file_of(&plan.pid_file));
            Err(failure)
        }
    }
}

/// `comfy_stop`：停**我们自己起的**那一个实例。
pub async fn stop(state: &AppState) -> Result<ToolOutcome, ToolFailure> {
    let config = crate::comfy::capture::capture_config(state)
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
            clear_launch_files(&pid_file);
            return Err(ToolFailure::new(
                "COMFY_NOT_OURS",
                format!(
                    "pid {pid} 现在是「{name}」，不是我们起的那个 python（pid 被系统回收过？）—— \
                     我没有动它，pid 记录也清掉了。"
                ),
            ));
        }
        None => {
            clear_launch_files(&pid_file);
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
            clear_launch_files(&pid_file);
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
    crate::comfy::comfy_client::get_json(&format!("{base}/system_stats")).await.ok()
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
        Ok(text.trim().replace('\n', " / "))
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

/// 那次启动的**留档**文件（与 pid 文件同目录、同名不同后缀）。
fn record_file_of(pid_file: &Path) -> PathBuf {
    pid_file.with_extension("launch.json")
}

/// 一起清掉 pid 记录与留档（**必须成对**：只清一个就会留下"pid 没了、input 还指着一个
/// 早就不是我们起的实例"的假事实，而投放侧正是照它选目录的）。
fn clear_launch_files(pid_file: &Path) {
    let _ = std::fs::remove_file(pid_file);
    let _ = std::fs::remove_file(record_file_of(pid_file));
}

/// 把"这个实例读哪个 input / 写哪个 output"记下来。
///
/// # 为什么非记不可（2026-09-30 用户 bug）
///
/// 起实例时挂的 input 有两处可能给它赋值：调用方自己写的 `--input`、以及 ComfyHub 照
/// ComfyUI Desktop 的 `settings.json` 补的 `--input-directory`。**实例最终读哪一个**，
/// 光看配置是推不出来的（两处都可能生效，argparse 还是后写的赢）。
///
/// 而投放工具（[`crate::comfy::image_edit`] 的三件）必须写进**实例真正在读的那一个** ——
/// 写错地方的表现是提交时 `400 Invalid image file`，看着像"文件不存在"。
/// 所以这里把**起完之后实际生效的那份 argv** 连同解析出来的 input/output 一起落盘：
/// 投放侧读同一份留档，两边就出自同一个答案（`image_edit::input_dir_candidates`）。
fn write_launch_record(plan: &LaunchPlan, pid: u32) -> std::io::Result<()> {
    let record = json!({
        "pid": pid,
        "port": plan.port,
        "workspace": plan.workspace.display().to_string(),
        "python": plan.python.display().to_string(),
        "log": plan.log.display().to_string(),
        // argv 原样留着：出问题时"到底挂了哪些参数"不用再猜
        "argv": plan.args,
        "inputDir": crate::tools::mcp::mounted_input_dir(&plan.args).map(|p| p.display().to_string()),
        "outputDir": crate::tools::mcp::mounted_output_dir(&plan.args).map(|p| p.display().to_string()),
        "startedAt": crate::infra::iso_time::from_system_time(std::time::SystemTime::now()),
    });
    std::fs::write(record_file_of(&plan.pid_file), record.to_string())
}

/// 读那次启动的留档（没有 / 不是 JSON 就是 `None`）。
pub fn read_launch_record(project_root: &Path, port: u16) -> Option<Value> {
    let path = record_file_of(&project_root.join(".run").join(format!("comfyui-{port}.pid")));
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

/// **我们上次起实例时挂的那个 input 目录**（投放侧拿它对准）。
///
/// 只认**自己起的、而且现在还活着**的那一个：pid 记录还在、而且那个 pid 现在真的是 python
/// （pid 会被系统回收，不复核就可能把别人的进程当成自己的实例）。
/// 用户自己用 ComfyUI Desktop 起的实例我们没起过 → `None`；那条路靠**直接问实例**
/// （`image_edit::target_input_dir` 的探测），而不是靠猜。
pub fn launched_input_dir(project_root: &Path, port: u16) -> Option<PathBuf> {
    let pid_file = project_root.join(".run").join(format!("comfyui-{port}.pid"));
    let pid = read_pid_file(&pid_file)?;
    let image = process_image_name(pid)?;
    if !image.to_ascii_lowercase().contains("python") {
        return None;
    }
    read_launch_record(project_root, port)?
        .get("inputDir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
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
///
/// `known_input` 是**这个实例读哪个 input 目录** —— 起实例时挂的那份（我们起的）或留档里那份。
/// 它是投放工具的目标目录，所以必须回给模型：两边不一致的表现是提交时 400
/// `Invalid image file`，而模型光看配置是推不出来的（2026-09-30 用户 bug）。
fn running_outcome(
    base: &str,
    stats: &Value,
    fresh: Option<(&LaunchPlan, u32, Duration)>,
    known_input: Option<PathBuf>,
) -> ToolOutcome {
    let system = stats.get("system").cloned().unwrap_or(Value::Null);
    let mut payload = json!({
        "started": true,
        "alreadyRunning": fresh.is_none(),
        "comfyUrl": base,
        "comfyReachable": true,
        "comfyuiVersion": system.get("comfyui_version").cloned().unwrap_or(Value::Null),
        "pythonVersion": system.get("python_version").cloned().unwrap_or(Value::Null),
        "inputDir": known_input
            .as_ref()
            .map(|p| Value::String(p.display().to_string()))
            .unwrap_or(Value::Null),
        "device": stats
            .get("devices")
            .and_then(Value::as_array)
            .and_then(|d| d.first())
            .and_then(|d| d.get("name"))
            .cloned()
            .unwrap_or(Value::Null),
    });
    let input_line = match known_input.as_ref() {
        Some(dir) => format!(
            "\n· **这个实例的 input = {}** —— `comfy_use_media` / `comfy_use_attachment` / \
             `comfy_make_mask` 会投到这里（它们会先问实例确认，投错目录的表现是提交时 400 \
             `Invalid image file`）",
            dir.display()
        ),
        None => "\n· 这个实例**不是 ComfyHub 起的**，所以这里不知道它读哪个 input；\
                 投放工具会直接问实例自己（`/object_info/LoadImage`），不用你猜。"
            .to_string(),
    };
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
                "ComfyHub 自己把 ComfyUI 起起来了（{:.1}s，pid {pid}）：{}{input_line}\n\
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
            "ComfyUI 已经在跑（{base}），没有再起一个 —— 直接用就行。{input_line}\
             （要知道状态与队列用 `comfy_get_status`；要重启就先 `comfy_stop` 再 `comfy_start`。）"
        ),
    };
    ToolOutcome::new(text, payload)
}

#[cfg(test)]
mod tests;
