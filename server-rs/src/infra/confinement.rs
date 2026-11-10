//! **"我这个进程是不是被关在项目里了"** —— 自查 + 留证（`<项目根>\.run\confinement.json`）。
//!
//! # 这条为什么必须单独成文件
//!
//! 2026-09-28 那一轮把现场写进了 `comfy_launch.rs` 与 `docs/pitfalls.md`，现象全部为真，
//! 但**处置建议有误**：文案写着"没有退路，下一步只能手动双击 ComfyUI Desktop"。
//! 2026-09-29 逐条复测，事实是：
//!
//! | 怀疑对象 | 实测 | 结论 |
//! | --- | --- | --- |
//! | 目录 ACL | 诊断脚本报 `NOT_THIS_CLASS`（无 deny ACE、无 package ACE，`WRITE_DAC`/`WRITE_OWNER` 都在） | 无关 |
//! | 路径 / 账户身份 / Defender | 同一账户同一目录 `powershell` 可写入；Defender 无受控文件夹规则 | 无关 |
//! | **映像在工作区内** | **同一个二进制逐字节相同：工作区内 → 写工作区外全 `os error 5`；复制到 `%TEMP%` → 全通** | ✅ 就是它 |
//! | 从哪个会话起 | 双击一样被限；`wmi-hidden`/`wmi-vbs`/`direct-hidden` 三档写同一个探针**都是 OK** | 无关 |
//!
//! # 2026-09-30 更新：机理钉死了 —— 是**完整性标签（Low IL）**，不是"位置"这个玄学
//!
//! 实测确认了上面那条"映像在工作区内"：
//!
//! 1. 工作区目录树带着 `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`，
//!    从它里面构建出来的 exe **继承了 Low 标签**；
//! 2. Windows 创建进程时取 **min(父进程令牌, 映像标签)** —— 映像是 Low，进程令牌就是 Low；
//! 3. Low 写不了工作区外的 Medium 对象（默认 NO_WRITE_UP）→ 一律 `os error 5`。
//!
//! 由此推出三条**处方**（也解释了 2026-09-29 表格里的每一行）：
//!
//! * **只把映像标签修回 Medium（位置一个字节不动）→ 立刻哪儿都能写**（实测）——
//!   `scripts\backend.ps1 start`（脚本路）与 `lib\core\backend_launcher.dart`（App 路，
//!   起之前 best-effort 跑一次 `icacls … /setintegritylevel Medium`）都做这件事；
//! * **链路上任何一环是 Low，子进程照样掉回 Low**：`viewer.exe` 也是工作区里构建的、
//!   也带 Low 标签时，App 里点"启动"救不了后端 —— 所以 release 构建顺手把两个 exe
//!   都归一为 Medium（`windows\cmake\bundle_backend.cmake`），而**受限会话里构建时这一步
//!   必然失败**（Low 写不了向上的标签），只能换到工作区外再跑；
//! * "复制到 `%TEMP%` 就通" = 那边没有 Low 标签可继承，与"谁启动"无关。
//!
//! **退路一直存在**：`COMFYHUB_SERVER_EXE`（`scripts\backend.ps1` 的 `Resolve-RustExe`）
//! 或直接用发布包，都能把后端映像挪出工作区。所以"无退路"是**漏了一个开关**，不是事实。
//!
//! 本模块只回答一个问题：**"现在这个后端受限吗"**，并把答案落到磁盘与
//! `GET /api/system/confinement` 让界面看得见 —— 沿用 `docs/pitfalls.md` 第 151/154 条
//! 已确定的约束（"界面上不可见的现象，模型只能推测"）。
//!
//! # 三条纪律
//!
//!  1. **判据可注入**（[`probe`] 收一个 `&dyn Fn(&Path) -> Result<(), String>`）：
//!     用例不依赖"这台机器的项目外目录可不可写"—— 那正是本机踩过的坑
//!     （`cargo test` 57 例一起红）。**并且必须把系统错误原样带出去**：
//!     只报"哪个目录"不报"报什么错"，就会重演 2026-09-28 那次"现象对、处方错"。
//!  2. **探到受限才保留文件，探到正常则删除**：不能让过期标记持续显示。
//!     标记带 `pid`，`doctor` 还要比对"是不是当前在跑的那个后端"才报。
//!  3. **探不出来 ≠ 受限**：目录不存在 / 配置读不到时**不写标记**（那是别的毛病，
//!     别混进这一栏）。所以 [`refresh`] 在拿不到工作区时只探产物目录。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 标记文件名（`.run` 目录与 [`crate::web::api_port_file::RUN_DIR`] 同一个约定）。
pub const FILE_NAME: &str = "confinement.json";

/// 标记在项目根下的位置。
pub fn path(project_root: &Path) -> PathBuf {
    project_root
        .join(crate::web::api_port_file::RUN_DIR)
        .join(FILE_NAME)
}

/// 一次自查的结论（纯计算结果，方便用例直接断言）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// 有没有目录写不进。
    pub restricted: bool,
    /// 写不进的那些（`写不进：<路径>` 的人话形式）。
    pub blocked_paths: Vec<String>,
    /// 探过的（探到几个就说明结论覆盖了几个）。
    pub checked_paths: Vec<String>,
}

impl Report {
    /// 给 `doctor` / 工具文案用的一行人话。
    pub fn summary(&self) -> String {
        if self.restricted {
            format!("受限（写不进 {}）", self.blocked_paths.join("、"))
        } else {
            format!("正常（探过 {} 处）", self.checked_paths.len())
        }
    }

    fn to_json(&self, pid: u32, at: String) -> serde_json::Value {
        serde_json::json!({
            "restricted": self.restricted,
            "detectedAt": at,
            "pid": pid,
            "blockedPaths": self.blocked_paths,
            "checkedPaths": self.checked_paths,
        })
    }
}

/// 真的探一次，**把真实的 OS 错误带回来**。
///
/// ⚠ 千万别再写成一个 `-> bool` 然后把 `Err` 吞掉：2026-09-29 这次就是"探针静默失败"
/// 把 doctor 指向了错的结论（2026-09-28 那批现场描述就是这么被写歪的）。
/// 所以 [`dir_writable`] 只是本函数的布尔视图，而**留证与文案一律用带错误的那一份**。
pub fn probe_dir(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "目录不存在",
        ));
    }
    // 探针名带 pid（并发起两个后端时互不干扰），`create_new` —— 已存在就报错，
    // 不去覆盖别人的东西。删不掉的也**当成功**（已经证明建得进，写权限是拿到了）。
    let probe = dir.join(format!(".comfyhub-write-probe-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .map(|_| ())
        .map(|()| {
            let _ = std::fs::remove_file(&probe);
        })
}

/// [`probe_dir`] 的布尔视图（用例与不关心原因的调用方用这个）。
pub fn dir_writable(dir: &Path) -> bool {
    probe_dir(dir).is_ok()
}

/// 探一批目录（`None` 的跳过 —— 那是"没配"，不是"写不进"）。
///
/// 判据可注入，用例因此**不依赖这台机器的目录可不可写**。注入的判据返回 `Err(原因)`，
/// 原因会原样进留证与文案（排查时"哪个目录、报什么错"是最有价值的两半）。
pub fn probe(
    targets: &[(&str, Option<&Path>)],
    judge: &dyn Fn(&Path) -> Result<(), String>,
) -> Report {
    let mut report = Report::default();
    for (label, dir) in targets {
        let Some(dir) = dir else { continue };
        report
            .checked_paths
            .push(format!("{label} {}", dir.display()));
        match judge(dir) {
            Ok(()) => {}
            Err(reason) => report
                .blocked_paths
                .push(format!("{label} {}（{reason}）", dir.display())),
        }
    }
    report.restricted = !report.blocked_paths.is_empty();
    report
}

/// 按当前后端的实际情况刷新标记文件，返回结论。
///
/// **这就是 `doctor` 唯一的判据来源**，所以它必须老实：探不全就只报探到的那部分，
/// 探到正常就把旧标记删掉（别让过期标记一直亮着）。
pub fn refresh(state: &crate::web::routes::AppState) -> Report {
    let project_root = state.project_root();
    let config = crate::comfy::capture::capture_config(state).ok();
    let configured_output = config.as_ref().and_then(|c| c.output_dir.clone());
    // 工作区解析不出来（还没配 ComfyUI / 装不上）**不是受限**，当"没这项"跳过
    let workspace = crate::comfy::comfy_launch::resolve_workspace(
        &project_root,
        configured_output.as_deref(),
        None,
    )
    .ok();

    let report = probe(
        &[
            ("产物目录", configured_output.as_deref().map(Path::new)),
            ("ComfyUI 工作区", workspace.as_deref()),
        ],
        &|dir| {
            probe_dir(dir).map_err(|e| {
                format!(
                    "{}（os error {}）",
                    e.kind().to_string().replace(" ", ""),
                    e.raw_os_error().unwrap_or(0)
                )
            })
        },
    );
    apply(&project_root, &report, std::process::id());
    report
}

/// 界面上要用的那份结论（`GET /api/system/confinement`）。
///
/// **必须包含"判据本身"**（[`server_image`] 与 `imageInsideProject`）：只说明"受限"无法自查，
/// 说清"因为后端映像在工作区内"他就能自己核对 —— 这正是 2026-09-28 那批现场描述被写歪的原因
/// （现象对、结论错，处方跟着错）。
pub fn describe(state: &crate::web::routes::AppState) -> serde_json::Value {
    let report = refresh(state);
    let image = server_image();
    let inside_root = image
        .as_deref()
        .map(|path| path.starts_with(state.project_root()))
        .unwrap_or(false);
    serde_json::json!({
        "restricted": report.restricted,
        "pid": std::process::id(),
        "projectRoot": state.project_root().display().to_string(),
        "serverImage": image.map(|p| p.display().to_string()),
        // ⚠ 判据：后端映像落在**自己的运行根目录**里没有。
        //   注意别把它说成"在工作区内"——源码树布局下两者相等（所以那句话成立），
        //   但发布包里根目录就是包目录、并不是被沙箱框住的地方（实测那里 `restricted=false`）。
        //   因此文案只能说明"落在自身根目录中"，以便对照自身布局。
        "imageInsideProjectRoot": inside_root,
        "checkedPaths": report.checked_paths,
        "blockedPaths": report.blocked_paths,
        "summary": report.summary(),
        "remedy": if report.restricted { Some(remedy_text()) } else { None },
    })
}

/// 一句人话的修法（界面、工具文案、doctor 共用同一份说法 —— 别各写各的）。
///
/// ⚠ 措辞约束（相关回归用例见 `tests/`）：**不得再给出错误处置**。
/// 实测已确认真因：**工作区产物带有 Low 完整性标签，Windows 取
/// min(父进程, 映像)** —— 所以修法是"换到没有 Low 标签的地方 / 把标签修回 Medium"，
/// 而不是改 ACL、也不是"换个终端重起"（终端换了、映像标签还是 Low，照样受限）。
pub fn remedy_text() -> String {
    "真因是本进程用了低完整性（Low IL）令牌。工作区里的产物带着 Low 标签，\
     Windows 取 min(父进程, 映像)，Low 写不了工作区外的 Medium 对象，所以一律是 os error 5。\n\
     修法有三种，内容照旧全在它自己的文件夹里：\n\
     1. 把整套 ComfyHub 放到工作区外（放哪个盘、哪个目录由用户定，只要不在工作区里）。\
     运行 pwsh -File scripts\\pack-release.ps1 -OutDir \"$env:USERPROFILE\\ComfyHub\"（目录换成用户自己的），\
     然后启动那里的 viewer.exe，App 会自己把 backend\\ 里的后端拉起来。\n\
     2. 已有工作区外的构建产物时，设 $env:COMFYHUB_SERVER_EXE = \"<完整路径>\"。\n\
     3. 就地修标签。运行 pwsh -File scripts\\backend.ps1 start，它会在启动前把 Low 标签修回 Medium；\
     当前会话本身是 Low 时修不动，那就只剩前两条路。"
        .to_string()
}

/// 后端映像（自己的 exe）现在在哪儿 —— **判据的第一半**。
pub fn server_image() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// 把结论落到磁盘：**受限才写，正常就删**。
///
/// 写不进标记文件不算失败（那正是受限的表现之一，不能因此让后端起不来）。
///
/// ⚠ **项目根不是一个真目录时连目录都不建**：单测里的 `AppState` 用的是 `:memory:`
/// 这种占位根，不挡一下就会在当前工作目录里造出一个叫 `:memory:` 的文件夹。
pub fn apply(project_root: &Path, report: &Report, pid: u32) -> Option<PathBuf> {
    let target = path(project_root);
    if !report.restricted {
        let _ = std::fs::remove_file(&target);
        return None;
    }
    if !project_root.is_dir() {
        return None;
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let at = crate::infra::iso_time::from_system_time(SystemTime::now());
    let text = report.to_json(pid, at).to_string();
    if std::fs::write(&target, text).is_err() {
        return None;
    }
    eprintln!(
        "[confinement] 本进程是低完整性（Low）令牌，写不进项目外：{}（留证：{}）—— \
         真因是工作区产物带着 Low 完整性标签，Windows 取 min(父进程, 映像)；\
         修法：pack-release.ps1 打包到工作区外、或设 COMFYHUB_SERVER_EXE 指向工作区外那个后端 exe、\
         或就地跑 scripts\\backend.ps1 start（起之前把 Low 标签修回 Medium）。\
         换个终端重起没用：映像标签还是 Low，进程照样是 Low。",
        report.blocked_paths.join("、"),
        target.display()
    );
    Some(target)
}

/// 读标记（读不到 / 格式坏了都回 `None`，**绝不抛** —— `doctor` 是诊断工具，坏了不能连带崩）。
pub fn read(project_root: &Path) -> Option<Report> {
    let text = std::fs::read_to_string(path(project_root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if !value.get("restricted")?.as_bool()? {
        return None;
    }
    let list = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(Report {
        restricted: true,
        blocked_paths: list("blockedPaths"),
        checked_paths: list("checkedPaths"),
    })
}

/// 标记是不是**当前在跑的这个后端**留下的。
///
/// `doctor` 必须过这一关：换了后端就该重新判，旧标记无权代表现状。
pub fn belongs_to(project_root: &Path, pid: u32) -> Option<Report> {
    let text = std::fs::read_to_string(path(project_root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if value.get("pid")?.as_u64()? as u32 != pid {
        return None;
    }
    read(project_root)
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
