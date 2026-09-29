//! **"我这个进程是不是被关在项目里了"** —— 自查 + 留证（`<项目根>\.run\confinement.json`）。
//!
//! # 这条为什么必须单独成文件
//!
//! 2026-09-28 那一轮把现场写进了 `comfy_launch.rs` 与 `docs/pitfalls.md`，现象全部为真，
//! 但**处方下错了**：文案写着"没有退路，下一步只能让用户双击 ComfyUI Desktop"。
//! 2026-09-29 逐条复测，事实是：
//!
//! | 怀疑对象 | 实测 | 结论 |
//! | --- | --- | --- |
//! | 目录 ACL | 诊断脚本报 `NOT_THIS_CLASS`（无 deny ACE、无 package ACE，`WRITE_DAC`/`WRITE_OWNER` 都在） | 无关 |
//! | 路径 / 用户身份 / Defender | 同用户同目录 `powershell` 写得进；Defender 无受控文件夹规则 | 无关 |
//! | **映像在工作区内** | **同一个二进制逐字节相同：工作区内 → 写工作区外全 `os error 5`；复制到 `%TEMP%` → 全通** | ✅ 就是它 |
//! | 从哪个会话起 | 双击一样被限；`wmi-hidden`/`wmi-vbs`/`direct-hidden` 三档写同一个探针**都是 OK** | 无关 |
//!
//! **退路一直存在**：`COMFYHUB_SERVER_EXE`（`scripts\backend.ps1` 的 `Resolve-RustExe`）
//! 或直接用发布包，都能把后端映像挪出工作区。所以"无退路"是**漏了一个开关**，不是事实。
//!
//! 本模块只回答一个问题：**"现在这个后端受限吗"**，并把答案落到磁盘让
//! `comfyhub.ps1 doctor` 看得见 —— 沿用 `docs/pitfalls.md` 第 151/154 条已经定下的那条纪律
//! （"doctor 看不出这一条，所以撞到这个错先问一句'这服务是谁起的'"）。
//!
//! # 三条纪律
//!
//!  1. **判据可注入**（[`probe`] 收一个 `&dyn Fn(&Path) -> Result<(), String>`）：
//!     用例不依赖"这台机器的项目外目录可不可写"—— 那正是本机踩过的坑
//!     （`cargo test` 57 例一起红）。**并且必须把系统错误原样带出去**：
//!     只报"哪个目录"不报"报什么错"，就会重演 2026-09-28 那次"现象对、处方错"。
//!  2. **探到受限才留文件，探到正常就删掉**：绝不能让一个几周前的标记一直亮着吓用户。
//!     标记带 `pid`，`doctor` 还要比对"是不是当前在跑的那个后端"才报。
//!  3. **探不出来 ≠ 受限**：目录不存在 / 配置读不到时**不写标记**（那是别的毛病，
//!     别混进这一栏）。所以 [`refresh`] 在拿不到工作区时只探产物目录。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 标记文件名（`.run` 目录与 [`crate::api_port_file::RUN_DIR`] 同一个约定）。
pub const FILE_NAME: &str = "confinement.json";

/// 标记在项目根下的位置。
pub fn path(project_root: &Path) -> PathBuf {
    project_root.join(crate::api_port_file::RUN_DIR).join(FILE_NAME)
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
        .and_then(|()| {
            let _ = std::fs::remove_file(&probe);
            Ok(())
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
pub fn probe(targets: &[(&str, Option<&Path>)], judge: &dyn Fn(&Path) -> Result<(), String>) -> Report {
    let mut report = Report::default();
    for (label, dir) in targets {
        let Some(dir) = dir else { continue };
        report
            .checked_paths
            .push(format!("{label}：{}", dir.display()));
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
pub fn refresh(state: &crate::routes::AppState) -> Report {
    let project_root = state.project_root();
    let config = crate::capture::capture_config(state).ok();
    let configured_output = config.as_ref().and_then(|c| c.output_dir.clone());
    // 工作区解析不出来（还没配 ComfyUI / 装不上）**不是受限**，当"没这项"跳过
    let workspace = crate::comfy_launch::resolve_workspace(
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
                format!("{}（os error {}）", e.kind().to_string().replace(" ", ""), e.raw_os_error().unwrap_or(0))
            })
        },
    );
    apply(&project_root, &report, std::process::id());
    report
}

/// 界面上要用的那份结论（`GET /api/system/confinement`）。
///
/// **必须带上"判据本身"**（[`server_image`] 与 `imageInsideProject`）：只说"受限"用户没法自查，
/// 说清"因为后端映像在工作区内"他就能自己核对 —— 这正是 2026-09-28 那批现场描述被写歪的原因
/// （现象对、结论错，处方跟着错）。
pub fn describe(state: &crate::routes::AppState) -> serde_json::Value {
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
        //   所以文案只能说"落在自己的根目录里"，让用户自己对照自己的布局。
        "imageInsideProjectRoot": inside_root,
        "checkedPaths": report.checked_paths,
        "blockedPaths": report.blocked_paths,
        "summary": report.summary(),
        "remedy": if report.restricted { Some(remedy_text()) } else { None },
    })
}

/// 一句人话的修法（界面、工具文案、doctor 共用同一份说法 —— 别各写各的）。
pub fn remedy_text() -> String {
    "把整套 ComfyHub 放到**工作区外**再启动（内容仍然全在它自己的文件夹里）：\n\
     pwsh -File scripts\\pack-release.ps1 -OutDir D:\\dist\\ComfyHub\n\
     pwsh -File D:\\dist\\ComfyHub\\scripts\\comfyhub.ps1 up\n\
     已经有工作区外的构建产物时，一句 $env:COMFYHUB_SERVER_EXE = \"<完整路径>\" 就够。"
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
    let at = crate::iso_time::from_system_time(SystemTime::now());
    let text = report.to_json(pid, at).to_string();
    if std::fs::write(&target, text).is_err() {
        return None;
    }
    eprintln!(
        "[confinement] 本进程写不进项目外：{}（留证：{}）—— \
         修法：把整套 ComfyHub 放到工作区外（发布包），或设 COMFYHUB_SERVER_EXE 指向工作区外那个后端 exe。\
         **换个终端重起没用**：限制跟的是**映像位置 + 父子继承**，跟会话无关\
         （2026-09-29 实测：受限进程拉起的、工作区外的 cmd.exe 一样写不进 %TEMP%，见 tests\\confinement_probe.rs）",
        report.blocked_paths.join("、"),
        target.display()
    );
    Some(target)
}

/// 读标记（读不到 / 格式坏了都回 `None`，**绝不抛** —— `doctor` 是诊断工具，坏了不能连带崩）。
pub fn read(project_root: &Path) -> Option<Report> {
    let text = std::fs::read_to_string(path(project_root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if value.get("restricted")?.as_bool()? != true {
        return None;
    }
    let list = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).map(String::from).collect())
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
mod tests {
    use super::*;

    use crate::testpaths::scratch;

    #[test]
    fn 探到写不进就如实报_且把系统错误原样带出去() {
        let good = Path::new("C:/good");
        let bad = Path::new("D:/bad");
        let report = probe(
            &[("产物目录", Some(good)), ("ComfyUI 工作区", Some(bad))],
            &|p| {
                if p == good {
                    Ok(())
                } else {
                    Err("PermissionDenied（os error 5）".to_string())
                }
            },
        );
        assert!(report.restricted);
        // ⚠ 错误原因必须在里面：只报"哪个目录"不报"报什么错"，
        // 就会重演 2026-09-28 那次"现象对、结论错"（当时把 Err 吞成了 false）。
        assert_eq!(
            report.blocked_paths,
            vec!["ComfyUI 工作区 D:/bad（PermissionDenied（os error 5））".to_string()]
        );
        assert_eq!(report.checked_paths.len(), 2, "探过几处要如实说，否则看不出覆盖没覆盖全");
    }

    #[test]
    fn 没配的目录跳过_不算受限() {
        // 传 `None` = "没配"，不是"写不进" —— 混进受限里会误导成另一个毛病
        let report = probe(&[("ComfyUI 工作区", None)], &|_| Err("x".into()));
        assert!(!report.restricted, "{:?}", report);
        assert!(report.checked_paths.is_empty());
        assert_eq!(report.summary(), "正常（探过 0 处）");
    }

    #[test]
    fn 全都写得进就不受限() {
        let report = probe(&[("产物目录", Some(Path::new("C:/x")))], &|_| Ok(()));
        assert!(!report.restricted);
        assert_eq!(report.summary(), "正常（探过 1 处）");
    }

    #[test]
    fn 留证与清证_并且只认当前后端() {
        let root = scratch("marker");
        let restricted = Report {
            restricted: true,
            blocked_paths: vec!["ComfyUI 工作区 D:\\x".to_string()],
            checked_paths: vec!["产物目录：D:\\o".to_string()],
        };
        assert!(apply(&root, &restricted, 4242).is_some());
        assert!(path(&root).is_file());
        assert_eq!(read(&root).unwrap().blocked_paths, restricted.blocked_paths);

        // 别的后端来读：旧标记无权代表现状
        assert!(belongs_to(&root, 999).is_none());
        assert_eq!(belongs_to(&root, 4242).unwrap().restricted, true);

        // 探到正常 → 立刻清掉（不能让过期标记一直亮着吓用户）
        assert!(apply(&root, &Report::default(), 4242).is_none());
        assert!(!path(&root).exists());
        assert!(read(&root).is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 坏文件与缺文件都回_none_而不是崩() {
        let root = scratch("broken");
        assert!(read(&root).is_none());
        assert!(belongs_to(&root, 1).is_none());
        std::fs::create_dir_all(root.join(crate::api_port_file::RUN_DIR)).unwrap();
        std::fs::write(path(&root), "{ 半截 JSON").unwrap();
        assert!(read(&root).is_none());
        // 写出了但 `restricted: false` 的也要当"没受限"
        std::fs::write(path(&root), r#"{"restricted":false,"pid":1}"#).unwrap();
        assert!(read(&root).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 真探一次_项目内的临时目录一定写得进() {
        // `probe_dir` 本身不许依赖"这台机器的项目外目录可不可写"，
        // 所以只断言项目内这一条 —— 它正是受限会话里仍然成立的那一半。
        let root = scratch("real");
        assert!(dir_writable(&root));
        probe_dir(&root).expect("项目内一定写得进");
        // 不存在的目录：NotFound + 人话原因，不是"写不进"
        let missing = root.join("不存在");
        let err = probe_dir(&missing).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(err.to_string().contains("目录不存在"), "{}", err);

        // ⚠ **不依赖这台机器哪个目录不可写**（那正是本模块存在的理由）。
        // 用一个同名**目录**占住探针名，`create_new` 必然撞上真实的系统错误
        // （Windows 上"在目录的位置建文件"回 ERROR_ACCESS_DENIED）。
        // 这样"OS 错误被原样带出去"这条就能在**任何**机器上钉住。
        let blocker = root.join(format!(".comfyhub-write-probe-{}", std::process::id()));
        std::fs::create_dir(&blocker).unwrap();
        let denied = probe_dir(&root).unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(5), "要拿到系统的原始错误码：{denied}");
        assert!(
            format!("{denied}").contains("os error 5"),
            "系统错误要带 os error：{denied}"
        );
        std::fs::remove_dir(&blocker).unwrap();
        assert!(dir_writable(&root), "占位去掉后要恢复写得进");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 界面上那份 JSON（`GET /api/system/confinement`）：**判据与结论一起给**。
    ///
    /// 只说"受限"用户没法自查，说清"因为后端映像在工作区内"他就能自己核对 ——
    /// 2026-09-28 那批现场描述就是这么被写歪的（现象对、结论错）。
    #[test]
    fn 界面那份结论要带判据与修法() {
        let root = scratch("describe");
        let conn = crate::db::open_memory().unwrap();
        let schema = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("db").join("schema.sqlite.sql"),
        )
        .expect("读得到真 schema");
        conn.execute_batch(&schema).expect("建 schema");
        let state =
            crate::routes::AppState::new(conn, root.to_string_lossy().as_ref(), root.join("media"));
        crate::capture::save_capture_config(
            &state,
            crate::capture::CaptureConfig {
                output_dir: Some(root.join("output").display().to_string()),
                ..Default::default()
            },
        )
        .expect("存捕获配置");
        std::fs::create_dir_all(root.join("output")).unwrap();

        // ⚠ **不断言 restricted 的取值**：`resolve_workspace` 在本机是能探到真的 ComfyUI 的，
        //   那份目录在本会话（映像在工作区内的测试二进制）恰好写不进去 → 结论是 true。
        //   "这台机器哪个目录可写"是环境属性，不是被测行为（AGENTS §12 纪律）。
        let json = describe(&state);
        let restricted = json["restricted"].as_bool().expect("restricted 必须是布尔");
        assert!(json["blockedPaths"].is_array());
        assert!(json["checkedPaths"].is_array());
        assert_eq!(
            json["remedy"].is_null(),
            !restricted,
            "修法只在真受限时给（不受限时那行是噪音）"
        );
        // 判据那一半：映像路径与"它在不在自己的运行根目录里"
        assert!(json["serverImage"].is_string(), "要说清后端映像在哪儿");
        assert!(json["imageInsideProjectRoot"].is_boolean());
        assert_eq!(json["pid"], serde_json::json!(std::process::id()));
        assert!(!json["summary"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 修法文案指向发布包而不是让用户改_acl() {
        let text = remedy_text();
        assert!(text.contains("pack-release.ps1"), "要给出真正管用的那一步：{text}");
        assert!(text.contains("COMFYHUB_SERVER_EXE"), "已有工作区外的产物时也要有路：{text}");
        for wrong in ["权限", "ACL", "换个终端", "双击 ComfyUI"] {
            assert!(!text.contains(wrong), "别再说错的处方（{wrong}）：{text}");
        }
    }
}
