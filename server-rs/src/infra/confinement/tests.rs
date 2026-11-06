//! `confinement` 的单元测试（主体代码在 [`confinement`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

use crate::infra::testpaths::scratch;

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
    assert_eq!(
        report.checked_paths.len(),
        2,
        "探过几处要如实说，否则看不出覆盖没覆盖全"
    );
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
        checked_paths: vec!["产物目录 D:\\o".to_string()],
    };
    assert!(apply(&root, &restricted, 4242).is_some());
    assert!(path(&root).is_file());
    assert_eq!(read(&root).unwrap().blocked_paths, restricted.blocked_paths);

    // 别的后端来读：旧标记无权代表现状
    assert!(belongs_to(&root, 999).is_none());
    assert!(belongs_to(&root, 4242).unwrap().restricted);

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
    std::fs::create_dir_all(root.join(crate::web::api_port_file::RUN_DIR)).unwrap();
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
    assert_eq!(
        denied.raw_os_error(),
        Some(5),
        "要拿到系统的原始错误码：{denied}"
    );
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
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .expect("读得到真 schema");
    conn.execute_batch(&schema).expect("建 schema");
    let state = crate::web::routes::AppState::new(
        conn,
        root.to_string_lossy().as_ref(),
        root.join("media"),
    );
    crate::comfy::capture::save_capture_config(
        &state,
        crate::comfy::capture::CaptureConfig {
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
    assert!(
        text.contains("pack-release.ps1"),
        "要给出真正管用的那一步：{text}"
    );
    assert!(
        text.contains("COMFYHUB_SERVER_EXE"),
        "已有工作区外的产物时也要有路：{text}"
    );
    for wrong in ["权限", "ACL", "换个终端", "双击 ComfyUI"] {
        assert!(!text.contains(wrong), "别再说错的处方（{wrong}）：{text}");
    }
}
