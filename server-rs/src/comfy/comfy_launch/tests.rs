//! `comfy_launch` 的单元测试（主体代码在 [`comfy_launch`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

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
    let desktop = crate::tools::mcp::desktop(Some(desktop_dir)).expect("配置要读得到");

    let plan = plan_with(&tmp, ws.clone(), 8188, &[], Some(&desktop)).unwrap();
    assert_eq!(plan.port, 8188);
    assert_eq!(
        plan.python,
        ws.join(".venv").join("Scripts").join("python.exe")
    );
    assert!(
        plan.args
            .contains(&"--extra-model-paths-config".to_string()),
        "{:?}",
        plan.args
    );
    assert!(
        plan.args.contains(&yaml.display().to_string()),
        "{:?}",
        plan.args
    );
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
    let explicit = plan_with(
        &tmp,
        ws.clone(),
        8188,
        &["--port".into(), "8199".into()],
        Some(&desktop),
    )
    .unwrap();
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
    assert!(
        crate::comfy::comfy_dirs::looks_like_comfy(&shared).0,
        "前提：它算'任何形态'的 ComfyUI"
    );
    let err = resolve_workspace(&tmp, None, Some(&shared.display().to_string())).unwrap_err();
    assert_eq!(err.code, "COMFY_NOT_FOUND");
    assert!(
        err.message.contains("共享数据根"),
        "要说清为什么不行：{}",
        err.message
    );
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

/// 启动实例后**必须记录"它读取哪个 input"**（现象：投放工具与实例
/// 各读各的 input，图投进安装目录、实例读共享目录 → 提交 400 `Invalid image file`）。
///
/// 留档里要能看到 argv 与解析出来的 input/output：调用方自己写的 `--input` 也要认
/// （argparse 允许唯一前缀缩写，后写的赢 —— 见 `mcp::mounted_input_dir`）。
#[test]
fn launch_record_remembers_which_input_the_instance_reads() {
    let tmp = scratch("record");
    let workspace = tmp.join("ComfyUI");
    let mine = tmp.join("mine-input");
    let shared = tmp.join("shared-input");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    // 留档与 pid 一样落在 `.run\` 下（读的一方就是照这个位置找的）
    let run_dir = tmp.join(".run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let plan = LaunchPlan {
        workspace,
        python: tmp.join("python.exe"),
        port: 8188,
        // 真实的现场形状：调用方写缩写 `--input`，ComfyHub 后补全名 —— **后写的赢**
        args: vec![
            "--input".into(),
            mine.display().to_string(),
            "--input-directory".into(),
            shared.display().to_string(),
            "--port".into(),
            "8188".into(),
        ],
        log: tmp.join("x.log"),
        pid_file: run_dir.join("comfyui-8188.pid"),
        note: None,
    };
    write_pid_file(&plan, 4321).unwrap();
    write_launch_record(&plan, 4321).unwrap();

    let record = read_launch_record(&tmp, 8188).expect("留档要读得到");
    assert_eq!(record["pid"], 4321);
    assert_eq!(record["port"], 8188);
    assert_eq!(
        record["inputDir"].as_str().unwrap(),
        shared.display().to_string(),
        "留档的必须是**实例真正读的那一份**（后写的赢），不是调用方先写的那份"
    );
    assert!(record["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--input")));

    // 没有留档 / 别的端口 → None（不编一个）
    assert!(read_launch_record(&tmp, 8199).is_none());
    clear_launch_files(&plan.pid_file);
    assert!(
        read_launch_record(&tmp, 8188).is_none(),
        "pid 与留档要成对清掉"
    );
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
    assert!(
        err.message.contains("双击 ComfyUI Desktop"),
        "要给出下一步：{}",
        err.message
    );
    assert!(
        err.message.contains("继承"),
        "要说清是继承来的限制：{}",
        err.message
    );

    // 产物目录写不进 → 也拒，并点名是哪个目录
    let err = ensure_writable(&plan, Some(&out), &|p| p != out).unwrap_err();
    assert_eq!(err.code, "COMFY_WRITE_BLOCKED");
    assert!(
        err.message.contains(&out.display().to_string()),
        "{}",
        err.message
    );

    // 真实判据在项目内必须为真（项目内写不了的话，整个后端都没法干活了）
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    assert!(
        dir_writable(&project),
        "项目内目录应当写得进：{}",
        project.display()
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
