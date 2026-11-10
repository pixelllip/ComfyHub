//! `mcp` 的单元测试（主体代码在 [`mcp`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// 一台**脚本化假 MCP 服务器**（`tokio::io::duplex`，不依赖 Python、不出网）。
fn fake_server<F>(respond: F) -> McpClient
where
    F: Fn(&Value) -> Option<Value> + Send + 'static,
{
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(server_side);
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(response) = respond(&value) {
                let _ = writer.write_all(response.to_string().as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                let _ = writer.flush().await;
            }
        }
    });
    let (reader, writer) = tokio::io::split(client_side);
    McpClient::from_io(reader, writer)
}

fn reply(id: u64, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

#[tokio::test]
async fn 握手与工具清单() {
    let client = fake_server(|request| {
        let id = request.get("id")?.as_u64()?;
        match request.get("method")?.as_str()? {
            "initialize" => Some(reply(
                id,
                json!({ "capabilities": {}, "instructions": "先看这段" }),
            )),
            "tools/list" => Some(reply(
                id,
                json!({ "tools": [
                    { "name": "system_stats", "description": "查显存", "inputSchema": { "type": "object" } },
                    { "name": "run_workflow", "description": "跑工作流", "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } } },
                    { "description": "没有名字的要被丢掉" },
                ]}),
            )),
            _ => None,
        }
    });
    client.handshake().await.unwrap();
    assert_eq!(client.instructions().as_deref(), Some("先看这段"));
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2, "缺 name 的那条要被丢掉");
    assert_eq!(tools[0].name, "system_stats");
    assert_eq!(
        tools[1].input_schema["properties"]["path"]["type"],
        "string"
    );
}

#[tokio::test]
async fn 调用结果按块拼起来_非文本块留占位() {
    let client = fake_server(|request| {
        let id = request.get("id")?.as_u64()?;
        match request.get("method")?.as_str()? {
            "tools/call" => Some(reply(
                id,
                json!({ "content": [
                    { "type": "text", "text": "第一段" },
                    { "type": "image", "data": "…" },
                    { "type": "text", "text": "第二段" },
                ], "isError": false }),
            )),
            _ => None,
        }
    });
    let result = client.call_tool("fetch_outputs", &json!({})).await.unwrap();
    assert!(!result.is_error);
    assert_eq!(
        result.text_blocks,
        vec!["第一段".to_string(), "第二段".to_string()]
    );
    assert!(result.text.starts_with("第一段"));
    assert!(
        result.text.contains("image（ComfyHub 不渲染"),
        "非文本块不许无声丢弃：{}",
        result.text
    );
}

#[tokio::test]
async fn 服务端_is_error_如实带回来() {
    let client = fake_server(|request| {
        let id = request.get("id")?.as_u64()?;
        (request.get("method")?.as_str()? == "tools/call").then(|| {
            reply(id, json!({ "content": [{ "type": "text", "text": "Error executing tool xxx" }], "isError": true }))
        })
    });
    let result = client
        .call_tool("launch_comfyui", &json!({}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.text, "Error executing tool xxx");
}

/// 现象「调用 MCP 的 nodes 工具总是失败」：UnexpectedToolError
/// 要翻成"下一步做什么"的人话，而不是让模型原样复读一段 python traceback。
#[test]
fn unexpected_tool_error_翻成人话并给出下一步() {
    let traceback = "UnexpectedToolError: Error executing tool nodes";
    // 认得的形状 → 人话里带着下一步
    let human = humanize_mcp_failure("nodes", traceback, "").expect("要翻");
    assert!(human.contains("ComfyUI"), "{human}");
    assert!(human.contains("server_info"), "{human}");
    // 同样的话出现在 stderr 尾部（isError 的 content 是空的那个现场）也要认得
    let human = humanize_mcp_failure("nodes", "", traceback).expect("stderr 里也要认");
    assert!(human.contains("nodes"), "{human}");
    // 认不得的失败照旧透传（不编）
    assert!(humanize_mcp_failure("nodes", "HTTP 500: internal", "").is_none());
}

/// 现象「MCP 调用总是失败，被误判为 ComfyUI 未运行」：
/// 真实根因是 `PermissionError [Errno 13]`（受限会话里 comfy-cli 写
/// `%LOCALAPPDATA%\comfy-cli\secrets.json.lock` 被拒），**不是** ComfyUI 没在运行 ——
/// 旧版一律译为"先确认 ComfyUI 在线"，导致模型向错误方向排查。
#[test]
fn 权限被拒_不再误导为_comfy_没在运行() {
    let traceback = "UnexpectedToolError: Error executing tool nodes\n\
         PermissionError: [Errno 13] Permission denied: \
         'C:\\\\Users\\\\Administrator\\\\AppData\\\\Local\\\\comfy-cli\\\\secrets.json.lock'";
    let human = humanize_mcp_failure("nodes", traceback, "").expect("要翻");
    assert!(human.contains("写文件被系统拒绝"), "{human}");
    assert!(
        human.contains("secrets") || human.contains("凭据"),
        "{human}"
    );
    assert!(!human.contains("先用 server_info"), "{human}");
    // errno 5 / 拒绝访问 的变体也要认得（同一个根因家族）
    let human = humanize_mcp_failure(
        "nodes",
        "Error executing tool nodes\nOS Error: 拒绝访问。, errno = 5",
        "",
    )
    .expect("errno 5 也要认");
    assert!(human.contains("写文件被系统拒绝"), "{human}");
}

/// 探测函数本身：项目内的临时目录写得进（true）；不存在的深层路径建不出（false）。
#[test]
fn dir_writable_探测结果可信() {
    // ⚠ 判据真源已经搬到 `crate::infra::confinement::dir_writable`（后端自检与 doctor 都用它），
    // 这里只保留一条"探针语义没被改坏"的回归 —— 用例的临时目录放**项目内**
    // （受限会话中无法写入 `%TEMP%`，正是 `confinement.rs` 文件头记录的同一问题）。
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(".scratch")
        .join(format!("mcp-dw-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    assert!(
        crate::infra::confinement::dir_writable(&dir),
        "项目内的目录应该写得进"
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !crate::infra::confinement::dir_writable(&dir.join("不存在").join("更深")),
        "建不出的目录应报不可写"
    );
}

#[tokio::test]
async fn 噪音行被跳过而不是打死整条链() {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(server_side);
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = value.get("id").and_then(|v| v.as_u64());
            // 先吐两行噪音（MCP/依赖库里很常见），再给真正的回包。
            let _ = writer.write_all(b"warning: something noisy\n").await;
            let _ = writer.write_all(b"{ not json at all\n").await;
            if let Some(id) = id {
                let _ = writer
                    .write_all(reply(id, json!({ "tools": [] })).to_string().as_bytes())
                    .await;
                let _ = writer.write_all(b"\n").await;
            }
            let _ = writer.flush().await;
        }
    });
    let (reader, writer) = tokio::io::split(client_side);
    let client = McpClient::from_io(reader, writer);
    let tools = client.list_tools().await.unwrap();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn 不回复就是_mcp_timeout() {
    let client = fake_server(|_| None);
    let started = std::time::Instant::now();
    let err = client
        .request("tools/call", json!({}), 300)
        .await
        .expect_err("没人回包必须超时");
    assert_eq!(err.code, "MCP_TIMEOUT");
    assert!(err.message.contains("0 秒内没有回复") || err.message.contains("MCP 在"));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn 服务端报错翻成稳定码() {
    let client = fake_server(|request| {
        let id = request.get("id")?.as_u64()?;
        Some(
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "未知工具" } }),
        )
    });
    let err = client
        .call_tool("nope", &json!({}))
        .await
        .expect_err("要报错");
    assert_eq!(err.code, "MCP_ERROR_-32602");
    assert_eq!(err.message, "未知工具");
}

#[tokio::test]
async fn 服务端主动请求会收到方法不支持() {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
    tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(server_side);
        let mut lines = BufReader::new(reader).lines();
        // 服务端**主动**发一个请求（带 id、没有 method 的白名单外的东西）
        let _ = writer
            .write_all(
                json!({ "jsonrpc": "2.0", "id": 99, "method": "sampling/createMessage" })
                    .to_string()
                    .as_bytes(),
            )
            .await;
        let _ = writer.write_all(b"\n").await;
        let _ = writer.flush().await;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
                break;
            }
        }
    });
    let (reader, writer) = tokio::io::split(client_side);
    let _client = McpClient::from_io(reader, writer);
    let got = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["id"], 99);
    assert_eq!(got["error"]["code"], -32601);
}

#[tokio::test]
async fn stderr_尾部会留着给模型看() {
    let (client_side, server_side) = tokio::io::duplex(16 * 1024);
    let (err_client, err_server) = tokio::io::duplex(16 * 1024);
    tokio::spawn(async move {
        let mut writer = err_server;
        for i in 0..(STDERR_TAIL_LINES + 5) {
            let _ = writer.write_all(format!("line {i}\n").as_bytes()).await;
        }
        let _ = writer.flush().await;
        // 让 reader/stderr 排空任务有机会跑
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(server_side);
    });
    let (reader, writer) = tokio::io::split(client_side);
    let client = McpClient::from_io_with_stderr(reader, writer, Some(err_client));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tail = client.stderr_tail(5);
    assert!(tail.contains("line 44"), "尾部要留最后几行：{tail}");
    assert!(!tail.contains("line 0\n"), "老的要被挤掉：{tail}");
}

#[tokio::test]
async fn 读循环结束会让在等的调用当场失败() {
    let (client_side, server_side) = tokio::io::duplex(16 * 1024);
    drop(server_side); // 对端当场关闭
    let (reader, writer) = tokio::io::split(client_side);
    let client = McpClient::from_io(reader, writer);
    let err = client
        .request("tools/list", json!({}), 5_000)
        .await
        .expect_err("对端没了要报错");
    assert_eq!(err.code, "MCP_EXITED");
}

#[test]
fn 训练引擎的权限档按读写分开() {
    // 读状态 / 列项目 / 看 caption → 不需要批准
    for name in [
        "studio_projects",
        "studio_project",
        "studio_images",
        "studio_caption",
        "studio_status",
    ] {
        assert_eq!(anima_access_for(name), "ALLOW", "{name}");
        assert!(!anima_is_mutating(name), "{name}");
    }
    // 导入 / 筛选 / 写 caption / 打标 / 预处理 / 训练 → 会写盘或占显卡
    for name in [
        "studio_import",
        "studio_review",
        "studio_set_caption",
        "studio_apply_trigger",
        "studio_tag",
        "studio_preprocess",
        "studio_train",
    ] {
        assert_eq!(anima_access_for(name), "ASK", "{name}");
        assert!(anima_is_mutating(name), "{name}");
    }
    // 描述前缀必须说对话：只读工具不许被写成“会写数据集”
    assert!(anima_description_prefix("studio_images").contains("只读"));
    assert!(anima_description_prefix("studio_train").contains("会写"));
    assert!(reason_for("anima_studio_train").contains("训练"));
}

#[test]
fn 两个备用裁判不进清单() {
    // 2026-10-25：run_workflow 在子进程里必崩；validate_workflow 会自己归一 id、
    // 给出与 ComfyUI 相反的 valid。两条都从下发给模型的清单里摘掉。
    assert!(is_hidden_tool("run_workflow"));
    assert!(is_hidden_tool("validate_workflow"));
    assert!(!is_hidden_tool("nodes"), "只读的 schema 查询必须留着");
    assert!(!is_hidden_tool("comfy_submit"));
}

#[test]
fn 权限档与_kotlin_一致() {
    assert_eq!(access_for("run_workflow"), "ASK");
    assert_eq!(access_for("launch_comfyui"), "ASK");
    assert_eq!(access_for("update_comfyui"), "DENY");
    assert_eq!(access_for("download_model"), "DENY");
    assert_eq!(access_for("billing_status"), "DENY");
    assert_eq!(access_for("system_stats"), "ALLOW");
    assert_eq!(access_for("nodes"), "ALLOW");
    assert!(is_mutating("run_workflow"));
    assert!(!is_mutating("system_stats"));
    assert!(reason_for("launch_comfyui").contains("启动本机"));
}

#[test]
fn scripts_目录两种布局都认() {
    let venv = PathBuf::from(r"D:\proj\storage\ai\mcp\venv\Scripts\python.exe");
    assert_eq!(
        scripts_dir_of(&venv).unwrap(),
        PathBuf::from(r"D:\proj\storage\ai\mcp\venv\Scripts")
    );
    let portable = PathBuf::from(r"D:\proj\storage\ai\mcp\python\python.exe");
    assert_eq!(
        scripts_dir_of(&portable).unwrap(),
        PathBuf::from(r"D:\proj\storage\ai\mcp\python\Scripts")
    );
}

#[test]
fn extra_args_三种形状都认() {
    assert_eq!(
        as_argv(Some(&json!(["--port", "8189"]))),
        vec!["--port", "8189"]
    );
    assert_eq!(
        as_argv(Some(&json!("[\"--port\", \"8189\"]"))),
        vec!["--port", "8189"]
    );
    assert_eq!(as_argv(Some(&json!("--port 8189"))), vec!["--port", "8189"]);
    assert!(as_argv(None).is_empty());
}

#[test]
fn 没有_desktop_配置时如实警告且不编参数() {
    let plan = prepare_launch_with(&json!({}), None);
    assert_eq!(plan.args.get("extra_args"), None, "不该凭空塞参数");
    let note = plan.note.unwrap();
    assert!(note.contains("没读到 ComfyUI Desktop 的配置"), "{note}");
    assert!(note.contains("别据此断言"), "{note}");
}

#[test]
fn 补挂共享模型根的三参数() {
    let tmp = temp_dir("comfyenv");
    let input = tmp.join("input");
    let output = tmp.join("output");
    let models = tmp.join("models");
    for dir in [&input, &output, &models] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let yaml = tmp.join(SHARED_YAML);
    std::fs::write(&yaml, "comfyui:\n  base_path: x\n").unwrap();
    std::fs::write(
        tmp.join(SETTINGS_FILE),
        json!({
            "inputDir": input.display().to_string(),
            "outputDir": output.display().to_string(),
            "modelsDirs": [models.display().to_string()],
        })
        .to_string(),
    )
    .unwrap();

    let d = desktop(Some(tmp.clone())).expect("配置要读得到");
    let plan = prepare_launch_with(&json!({}), Some(d));
    let argv = as_argv(plan.args.get("extra_args"));
    assert_eq!(
        argv,
        vec![
            FLAG_MODEL_PATHS.to_string(),
            yaml.display().to_string(),
            FLAG_INPUT.to_string(),
            input.display().to_string(),
            FLAG_OUTPUT.to_string(),
            output.display().to_string(),
        ]
    );
    let note = plan.note.unwrap();
    assert!(
        note.contains("ComfyUI Desktop 的 settings.json 与 shared_model_paths.yaml"),
        "{note}"
    );
    assert!(note.contains("模型根："), "{note}");
    assert!(note.contains("这些参数是 **ComfyHub 补的**"), "{note}");
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn 模型自己写过的参数一律不覆盖() {
    let tmp = temp_dir("comfyenv-respect");
    let yaml = tmp.join(SHARED_YAML);
    std::fs::write(&yaml, "x: 1\n").unwrap();
    std::fs::write(
        tmp.join(SETTINGS_FILE),
        json!({ "inputDir": "D:\\nope" }).to_string(),
    )
    .unwrap();
    let d = desktop(Some(tmp.clone())).unwrap();

    // ① 模型自己写了 --extra-model-paths-config
    let plan = prepare_launch_with(
        &json!({ "extra_args": ["--extra-model-paths-config", "D:\\mine.yaml"] }),
        Some(d.clone()),
    );
    let argv = as_argv(plan.args.get("extra_args"));
    assert_eq!(argv, vec!["--extra-model-paths-config", "D:\\mine.yaml"]);
    assert!(plan.note.unwrap().contains("尊重模型自己写的"));

    // ② 模型自己写了 --base-directory：input/output 都不补
    let plan = prepare_launch_with(
        &json!({ "extra_args": "--base-directory D:\\base" }),
        Some(d),
    );
    let argv = as_argv(plan.args.get("extra_args"));
    assert!(
        argv.contains(&"--extra-model-paths-config".to_string()),
        "{argv:?}"
    );
    assert!(
        !argv.contains(&FLAG_INPUT.to_string()),
        "有 --base-directory 时不该补 input：{argv:?}"
    );
    assert!(!argv.contains(&FLAG_OUTPUT.to_string()));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn 不存在的输入输出目录不补() {
    let tmp = temp_dir("comfyenv-missing");
    std::fs::write(
        tmp.join(SETTINGS_FILE),
        json!({ "inputDir": "D:\\definitely-not-here" }).to_string(),
    )
    .unwrap();
    let d = desktop(Some(tmp.clone())).unwrap();
    assert!(
        d.input_dir.is_none(),
        "不存在的目录不许进参数（会让启动直接失败）"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

/// ⚠ **`--input` 是 `--input-directory` 的 argparse 缩写**，判"调用方写过没有"不能只比全名
/// （调用方写的 `--input <安装目录>\input` 未被识别，后补的
/// `--input-directory <共享>\input` 排在后面 —— argparse 后写的赢，于是实例挂的是共享那份，
/// 而投放工具按另一份配置推、把底图与人物图写进了安装目录 → 提交 400 `Invalid image file`）。
///
/// 实测证据（实例自身的 `/system_stats`）：
/// `main.py --input …\ComfyUI-Installs\ComfyUI\ComfyUI\input …
///  --input-directory …\ComfyUI-Shared\input …`，而 `LoadImage` 的候选名单是**共享**那份。
#[test]
fn 调用方写的_input_缩写也算写过不许覆盖() {
    let tmp = temp_dir("comfyenv-input-alias");
    let input = tmp.join("install-input");
    let output = tmp.join("shared-output");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(
        tmp.join(SETTINGS_FILE),
        json!({ "outputDir": output }).to_string(),
    )
    .unwrap();
    // 注意：Desktop 配置里**没有** inputDir —— 正是发布包那台的形状（少了 outputDir 那一半）
    let d = desktop(Some(tmp.clone())).unwrap();

    // ① 调用方写的是缩写 `--input`：**不许**再补 `--input-directory`
    let plan = prepare_launch_with(
        &json!({ "extra_args": ["--input", input.display().to_string()] }),
        Some(d.clone()),
    );
    let argv = as_argv(plan.args.get("extra_args"));
    assert!(
        !argv.contains(&FLAG_INPUT.to_string()),
        "调用方写过 input 了，ComfyHub 不许再补一条（argparse 后写的会盖住它）：{argv:?}"
    );
    assert_eq!(
        mounted_input_dir(&argv),
        Some(input.clone()),
        "生效的必须是调用方那一份"
    );
    assert!(
        plan.note.unwrap().contains("尊重调用方自己写的"),
        "{:?}",
        plan.args
    );

    // ② `--input=<目录>` 这种写法也要认
    let plan = prepare_launch_with(
        &json!({ "extra_args": [format!("--input={}", input.display())] }),
        Some(d.clone()),
    );
    assert!(!as_argv(plan.args.get("extra_args")).contains(&FLAG_INPUT.to_string()));

    // ③ 调用方没写时才轮到我们：Desktop 有 outputDir 就补上，没有 inputDir 就**不编一个**
    let plan = prepare_launch_with(&json!({}), Some(d));
    let argv = as_argv(plan.args.get("extra_args"));
    assert!(argv.contains(&FLAG_OUTPUT.to_string()), "{argv:?}");
    assert!(!argv.contains(&FLAG_INPUT.to_string()), "{argv:?}");
    assert_eq!(
        mounted_input_dir(&argv),
        None,
        "Desktop 没配 inputDir 时我们不编一个"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

/// `mounted_input_dir` / `mounted_output_dir`：**取最后一次出现**（argparse 的语义），
/// 而且不给值的那一次（后面紧跟另一个 `--`）不算数。
#[test]
fn 读生效的_input_output_取最后一次() {
    let argv: Vec<String> = ["--input", "A", "--port", "8188", "--input-directory", "B"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        mounted_input_dir(&argv),
        Some(PathBuf::from("B")),
        "后写的赢"
    );
    assert_eq!(mounted_output_dir(&argv), None);

    let argv: Vec<String> = ["--output-directory=C", "--input"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        mounted_output_dir(&argv),
        Some(PathBuf::from("C")),
        "`=` 写法也要认"
    );
    assert_eq!(
        mounted_input_dir(&argv),
        None,
        "参数写漏了值就别把下一个参数名当路径"
    );
}

/// 受限会话中将 comfy-cli 的"家目录 + config.ini"一并指向项目内（
/// 「通过 CLI 启动 comfyui 总是失败」的那半：config.ini 那关就是靠这个绕过去的）。
///
/// 为什么必须抄 config.ini：comfy-cli 的工作区（`default_workspace`）就记在里面，
/// 不抄的话它会拿"假家目录"下的默认工作区 —— 那是 `~\Documents\comfy\ComfyUI`，
/// 根本不存在，于是正式启动 / 跑工作流全错，而且**不报错**。
#[test]
fn 受限时把_comfy_cli_的家目录与_config_ini_一并指进项目内() {
    // 用例自己的临时目录放项目内（%TEMP% 在受限会话里写不了，见 comfy_launch 的同名注释）
    let tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(".scratch")
        .join(format!("mcp-comfyhome-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let home = tmp.join("comfy-cli-home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = tokio::process::Command::new("python");

    let dir = redirect_comfy_cli_home(&mut cmd, &home).expect("要能建出目录");
    assert!(dir.ends_with("comfy-cli"), "{}", dir.display());
    assert!(dir.starts_with(&home), "必须落在项目内：{}", dir.display());

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let real = Path::new(local.trim()).join("comfy-cli").join("config.ini");
        if real.is_file() {
            let copied = dir.join("config.ini");
            assert!(copied.is_file(), "要抄一份过来：{}", copied.display());
            assert_eq!(
                std::fs::read(&copied).unwrap(),
                std::fs::read(&real).unwrap(),
                "抄的必须是用户那份的**逐字节**副本（工作区就在里面）"
            );
        }
    }
    std::fs::remove_dir_all(&tmp).ok();
}

fn temp_dir(label: &str) -> PathBuf {
    crate::infra::testpaths::scratch(&format!("mcp-{label}"))
}
