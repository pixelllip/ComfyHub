//! 本地 LoRA 训练引擎（anima-lora-next）的探测 / 起停 / 状态。
//!
//! # 为什么只有这一层
//!
//! 训练栈是 Python + torch + 5GB 权重 + 数据集，**不可能**进 Rust 单文件 exe。
//! 形态与 ComfyUI 完全一样：ComfyHub 负责“目录 / 起停 / 端口 / 界面”，训练本身
//! 跑在引擎自己的进程里。项目、数据集、打标、预处理、训练的**事实**都在
//! `anima_daemon` 的 `/studio/*` 上；Rust 不重写任何训练逻辑，只做：
//!
//! 1. **探测**（`discover_engine`）：引擎目录、解释器、WebUI 源码、守护进程 pidfile；
//! 2. **状态**（[`status`]）：守护进程 / WebUI 是否在线、端口、项目数；
//! 3. **起停**（[`start`] / [`stop`]）：`pythonw` + `CREATE_NO_WINDOW`，日志落
//!    `<根>\.run\train-*.log`（AGENTS §2 / §13）。
//!
//! # 引擎 discovery 的顺序
//!
//! 与 `scripts\train-engine.ps1` 保持一致（改一边必须改另一边）：
//! `COMFYHUB_TRAIN_DIR` / `ANIMA_LORA_ROOT` → `<根>\training\anima_lora` →
//! `<根>\..\anima-lora-next` → `<根>\..\..\anima-lora-next`。
//! WebUI 源码再单独找：引擎自带的 `anima_lora_webui`（并回下游仓库的形态）或
//! 同级/上一级的 `anima-lora-webui` 仓库。
//!
//! # 端口只从文件读
//!
//! 守护进程的端口来自它自己写的 `output\daemon\daemon.json`（`--port 0` 时监听端口
//! 不是常量）；WebUI 的端口来自我们起它时写的 `<根>\.run\train-engine.json`。
//! **不许**在这里再发明一个端口来源。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 引擎的静态位置信息（目录存在 + 解释器存在才算装好）。
#[derive(Debug, Clone)]
pub struct Engine {
    pub root: PathBuf,
    pub python: PathBuf,
    pub pythonw: PathBuf,
    /// WebUI 源码根（`anima_lora_webui` 包所在目录）；没有就是 `None`。
    pub webui_source: Option<PathBuf>,
    /// 守护进程在这个 checkout 里的 pidfile（端口的唯一真源）。
    pub daemon_pidfile: PathBuf,
}

impl Engine {
    pub fn installed(&self) -> bool {
        self.root.join("train.py").is_file() && self.python.is_file()
    }

    pub fn webui_package(&self) -> Option<PathBuf> {
        let src = self.webui_source.as_ref()?;
        let candidate = src.join("anima_lora_webui").join("server.py");
        candidate.is_file().then_some(src.clone())
    }
}

/// 运行时状态文件（WebUI 的 pid / 端口；`train-engine.ps1` 与这里共用同一份）。
pub fn state_file(project_root: &Path) -> PathBuf {
    project_root.join(".run").join("train-engine.json")
}

fn is_engine_root(path: &Path) -> bool {
    path.join("train.py").is_file() && path.join("anima_daemon").is_dir()
}

/// 找引擎。返回 `None` = 没装（可选能力，缺了不拦启动）。
pub fn discover_engine(project_root: &Path) -> Option<Engine> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for key in ["COMFYHUB_TRAIN_DIR", "ANIMA_LORA_ROOT"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                candidates.push(PathBuf::from(trimmed));
            }
        }
    }
    candidates.push(project_root.join("training").join("anima_lora"));
    if let Some(parent) = project_root.parent() {
        candidates.push(parent.join("anima-lora-next"));
        if let Some(grand) = parent.parent() {
            candidates.push(grand.join("anima-lora-next"));
        }
    }

    for root in candidates {
        if !is_engine_root(&root) {
            continue;
        }
        let scripts = root.join(".venv").join("Scripts");
        let python = scripts.join("python.exe");
        if !python.is_file() {
            continue;
        }
        let pythonw = scripts.join("pythonw.exe");
        let pythonw = if pythonw.is_file() { pythonw } else { python.clone() };
        let webui_source = find_webui_source(&root, project_root);
        return Some(Engine {
            daemon_pidfile: root.join("output").join("daemon").join("daemon.json"),
            root,
            python,
            pythonw,
            webui_source,
        });
    }
    None
}

fn find_webui_source(engine: &Path, project_root: &Path) -> Option<PathBuf> {
    let mut candidates = vec![engine.to_path_buf()];
    if let Ok(raw) = std::env::var("ANIMA_WEBUI_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            candidates.insert(0, PathBuf::from(trimmed));
        }
    }
    if let Some(parent) = engine.parent() {
        candidates.push(parent.join("anima-lora-webui"));
    }
    if let Some(parent) = project_root.parent() {
        candidates.push(parent.join("anima-lora-webui"));
        if let Some(grand) = parent.parent() {
            candidates.push(grand.join("anima-lora-webui"));
        }
    }
    candidates.push(project_root.join("training").join("anima-lora-webui"));
    candidates.into_iter().find(|candidate| {
        candidate
            .join("anima_lora_webui")
            .join("server.py")
            .is_file()
    })
}

// ---------------------------------------------------------------------------
// HTTP（本机回环，直连；不碰 infra::outbound 的代理选择 —— 代理会把 127.0.0.1 打死）
// ---------------------------------------------------------------------------

async fn http_json(url: &str, timeout: Duration) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "请求超时".to_string()
            } else if e.is_connect() {
                "连接被拒绝（服务没起？）".to_string()
            } else {
                e.to_string()
            }
        })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", text.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("返回的不是 JSON：{e}"))
}

async fn post_json(url: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;
    let response = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", text.chars().take(300).collect::<String>()));
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| format!("返回的不是 JSON：{e}"))
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn pid_alive(pid: u64) -> bool {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok();
        if let Some(out) = out {
            let text = String::from_utf8_lossy(&out.stdout);
            return text.contains(&format!("\"{pid}\"")) || text.contains(&pid.to_string());
        }
        false
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub up: bool,
    pub port: Option<u16>,
    pub pid: Option<u64>,
    pub url: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WebUiState {
    pub up: bool,
    pub port: Option<u16>,
    pub pid: Option<u64>,
    pub url: Option<String>,
    /// 可选访问令牌（Rust 起 WebUI 时生成；手跑的旧实例没有）。
    ///
    /// WebUI 会写数据集、起几小时的 GPU 作业，所以绑 `0.0.0.0` 给手机用时
    /// 不能是“局域网谁都能点”。首跳 `/?token=…` 种 HttpOnly Cookie，
    /// 之后静态资源 / API / SSE 都自动带。
    pub token: Option<String>,
    pub detail: Option<String>,
}

/// 读守护进程 pidfile + 探 `/health`。端口只认文件里的那个。
pub async fn daemon_state(engine: &Engine, project_root: &Path) -> DaemonState {
    let mut candidates = vec![engine.daemon_pidfile.clone()];
    candidates.push(project_root.join(".run").join("anima-daemon.json"));
    let mut info = None;
    for path in candidates {
        if let Some(value) = read_json(&path) {
            if value.get("port").and_then(Value::as_u64).is_some() {
                info = Some(value);
                break;
            }
        }
    }
    let Some(info) = info else {
        return DaemonState { up: false, port: None, pid: None, url: None, detail: None };
    };
    let port = info.get("port").and_then(Value::as_u64).map(|p| p as u16);
    let pid = info.get("pid").and_then(Value::as_u64);
    let Some(port) = port else {
        return DaemonState { up: false, port: None, pid, url: None, detail: None };
    };
    let url = format!("http://127.0.0.1:{port}");
    match http_json(&format!("{url}/health"), Duration::from_secs(2)).await {
        Ok(health) => DaemonState {
            up: health.get("ok").and_then(Value::as_bool).unwrap_or(false),
            port: Some(port),
            pid,
            url: Some(url),
            detail: None,
        },
        Err(e) => DaemonState {
            up: false,
            port: Some(port),
            pid,
            url: Some(url),
            detail: Some(format!("pidfile 有记录但 /health 不答话：{e}")),
        },
    }
}

/// 读 state 文件里的 WebUI 端口并探 `/api/_env`；没有 state 时只试脚本默认的 8810。
pub async fn webui_state(project_root: &Path) -> WebUiState {
    let state = read_json(&state_file(project_root));
    let mut port = state
        .as_ref()
        .and_then(|s| s.get("webuiPort"))
        .and_then(Value::as_u64)
        .map(|p| p as u16);
    let pid = state
        .as_ref()
        .and_then(|s| s.get("webuiPid"))
        .and_then(Value::as_u64);
    let token = state
        .as_ref()
        .and_then(|s| s.get("webuiToken"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if port.is_none() {
        // 脚本 / 手动起的默认端口。只作为**探测**，不写回任何真源。
        if http_json("http://127.0.0.1:8810/api/_env", Duration::from_secs(1)).await.is_ok() {
            port = Some(8810);
        }
    }
    let Some(port) = port else {
        return WebUiState { up: false, port: None, pid, url: None, token, detail: None };
    };
    let url = format!("http://127.0.0.1:{port}");
    let probe = match token.as_deref() {
        Some(t) => format!("{url}/api/_env?token={t}"),
        None => format!("{url}/api/_env"),
    };
    match http_json(&probe, Duration::from_secs(2)).await {
        Ok(_) => WebUiState {
            up: true,
            port: Some(port),
            pid,
            url: Some(url),
            token,
            detail: None,
        },
        Err(e) => WebUiState {
            up: false,
            port: Some(port),
            pid,
            url: Some(url),
            token,
            detail: Some(format!("state 文件有记录但 WebUI 不答话：{e}")),
        },
    }
}

/// 完整的训练能力状态（界面 / `/api/train/status`）。
pub async fn status(project_root: &Path) -> Value {
    let Some(engine) = discover_engine(project_root) else {
        return json!({
            "installed": false,
            "engineDir": Value::Null,
            "python": Value::Null,
            "webuiSource": Value::Null,
            "daemon": { "up": false, "port": Value::Null, "url": Value::Null },
            "webui": { "up": false, "port": Value::Null, "url": Value::Null },
            "projects": 0,
            "problems": ["没有找到 anima-lora-next 引擎（需要 train.py + .venv\\Scripts\\python.exe）"],
        });
    };
    let daemon = daemon_state(&engine, project_root).await;
    let webui = webui_state(project_root).await;
    let mut problems: Vec<String> = Vec::new();
    if !engine.installed() {
        problems.push("引擎目录缺 train.py 或 .venv".to_string());
    }
    if engine.webui_package().is_none() {
        problems.push("没有找到 anima_lora_webui 源码（WebUI 起不来，但守护进程与训练照常）".to_string());
    }
    if let Some(detail) = &daemon.detail {
        problems.push(detail.clone());
    }
    if let Some(detail) = &webui.detail {
        problems.push(detail.clone());
    }
    if webui.up && webui.token.is_none() {
        problems.push(
            "WebUI 是手动/旧版起的（没有访问令牌，只绑回环）：桌面能用，手机看不到。\
             点「停止」再「启动」会换成带 token、监听局域网的实例。"
                .to_string(),
        );
    }

    let projects = match daemon.url.as_ref() {
        Some(url) => http_json(&format!("{url}/studio/projects"), Duration::from_secs(3))
            .await
            .ok()
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or_else(|| count_projects(&engine.root)),
        None => count_projects(&engine.root),
    };

    json!({
        "installed": engine.installed(),
        "engineDir": engine.root.display().to_string(),
        "python": engine.python.display().to_string(),
        "webuiSource": engine.webui_package().map(|p| p.display().to_string()),
        "daemon": {
            "up": daemon.up,
            "port": daemon.port,
            "pid": daemon.pid,
            "url": daemon.url,
        },
        "webui": {
            "up": webui.up,
            "port": webui.port,
            "pid": webui.pid,
            "url": webui.url,
            "token": webui.token,
        },
        "projects": projects,
        "problems": problems,
    })
}

fn count_projects(engine_root: &Path) -> usize {
    let dir = engine_root.join("training_projects");
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("project.json").is_file())
                .count()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 起停
// ---------------------------------------------------------------------------

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("挑不到空闲端口：{e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("挑不到空闲端口：{e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// 给 WebUI 生成一次性访问令牌（起进程时通过 `--token` 传，状态文件里存一份）。
///
/// 用 `mobile::auth::random_bytes`（系统 CSPRNG）：这个令牌是**训练台对局域网的
/// 唯一门禁**，不能用时间 / pid 拼。
fn new_token() -> Result<String, String> {
    let bytes = crate::mobile::auth::random_bytes(18).map_err(|e| e.error)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn spawn_hidden(
    program: &Path,
    args: &[String],
    cwd: &Path,
    envs: &[(String, String)],
    log_path: &Path,
) -> Result<u32, String> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建不出日志目录：{e}"))?;
    }
    let log = std::fs::File::create(log_path)
        .map_err(|e| format!("建不出日志 {}：{e}", log_path.display()))?;
    let err_log = log.try_clone().map_err(|e| format!("复制日志句柄失败：{e}"))?;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err_log));
    for (key, value) in envs {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("起 {} 失败：{e}", program.display()))?;
    Ok(child.id())
}

/// 起守护进程 + WebUI。已经起来的就不重复起（幂等）。
pub async fn start(project_root: &Path) -> Result<Value, String> {
    let engine = discover_engine(project_root)
        .ok_or_else(|| "没有找到 anima-lora-next 引擎（需要 train.py + .venv\\Scripts\\python.exe）".to_string())?;
    if !engine.installed() {
        return Err(format!("引擎没装好：{}（缺 train.py 或 .venv）", engine.root.display()));
    }
    let mut actions: Vec<String> = Vec::new();

    // ① 守护进程
    let daemon = daemon_state(&engine, project_root).await;
    if !daemon.up {
        let pidfile = project_root.join(".run").join("anima-daemon.json");
        let envs = vec![
            ("ANIMA_LORA_ROOT".to_string(), engine.root.display().to_string()),
            ("ANIMA_DAEMON_PIDFILE".to_string(), pidfile.display().to_string()),
        ];
        let log = project_root.join(".run").join("train-daemon.log");
        let pid = spawn_hidden(
            &engine.pythonw,
            &["-m".to_string(), "anima_daemon".to_string()],
            &engine.root,
            &envs,
            &log,
        )?;
        actions.push(format!("起守护进程 pid {pid}"));
        let started = Instant::now();
        let mut up = false;
        while started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if daemon_state(&engine, project_root).await.up {
                up = true;
                break;
            }
        }
        if !up {
            return Err(format!(
                "守护进程 20 秒内没起来；日志：{}",
                log.display()
            ));
        }
    }

    // ② WebUI
    let webui = webui_state(project_root).await;
    if !webui.up {
        let source = engine
            .webui_package()
            .ok_or_else(|| "没有找到 anima_lora_webui 源码，WebUI 起不来（守护进程已起，训练仍可用）".to_string())?;
        let port = free_port()?;
        let token = new_token()?;
        let envs = vec![
            ("PYTHONPATH".to_string(), source.display().to_string()),
            ("ANIMA_LORA_ROOT".to_string(), engine.root.display().to_string()),
        ];
        let log = project_root.join(".run").join("train-webui.log");
        let args = vec![
            "-m".to_string(),
            "anima_lora_webui.server".to_string(),
            "--root".to_string(),
            engine.root.display().to_string(),
            "--port".to_string(),
            port.to_string(),
            // 手机要连桌面：必须监听局域网；门禁是 --token（首跳种 Cookie）。
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--token".to_string(),
            token.clone(),
        ];
        let pid = spawn_hidden(&engine.pythonw, &args, &source, &envs, &log)?;
        actions.push(format!("起 WebUI pid {pid}（端口 {port}）"));
        let started = Instant::now();
        let mut up = false;
        while started.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if http_json(
                &format!("http://127.0.0.1:{port}/api/_env?token={token}"),
                Duration::from_millis(800),
            )
            .await
            .is_ok()
            {
                up = true;
                break;
            }
        }
        if !up {
            return Err(format!("WebUI 15 秒内没起来；日志：{}", log.display()));
        }
        let state = json!({
            "webuiPid": pid,
            "webuiPort": port,
            "webuiToken": token,
            "engineDir": engine.root.display().to_string(),
            "startedAt": chrono_like_now(),
        });
        let path = state_file(project_root);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&state).unwrap_or_default())
            .map_err(|e| format!("写状态文件失败（{}）：{e}", path.display()))?;
    }

    let mut value = status(project_root).await;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("actions".to_string(), json!(actions));
    }
    Ok(value)
}

/// 停 WebUI + 守护进程（不 kill 正在跑的训练：走 `/shutdown {kill_jobs:false}`）。
pub async fn stop(project_root: &Path) -> Result<Value, String> {
    let mut actions: Vec<String> = Vec::new();
    let state = read_json(&state_file(project_root));
    if let Some(pid) = state
        .as_ref()
        .and_then(|s| s.get("webuiPid"))
        .and_then(Value::as_u64)
    {
        if pid_alive(pid) {
            kill_tree(pid)?;
            actions.push(format!("停 WebUI pid {pid}"));
        }
    }
    if let Some(engine) = discover_engine(project_root) {
        let daemon = daemon_state(&engine, project_root).await;
        if let Some(url) = daemon.url.as_ref() {
            if daemon.up {
                let _ = post_json(
                    &format!("{url}/shutdown"),
                    &json!({ "kill_jobs": false }),
                    Duration::from_secs(10),
                )
                .await;
                actions.push("停守护进程（训练若在跑，会先让它跑完）".to_string());
            }
        }
    }
    let _ = std::fs::remove_file(state_file(project_root));
    let mut value = status(project_root).await;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("actions".to_string(), json!(actions));
    }
    Ok(value)
}

fn kill_tree(pid: u64) -> Result<(), String> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("taskkill 起不来：{e}"))?;
        if !out.status.success() {
            return Err(format!(
                "taskkill 失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Ok(())
    }
}

/// 不引 chrono：给状态文件一个足够用的本地时间串。
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_file_lives_under_run() {
        let root = Path::new(r"D:\proj");
        assert!(state_file(root).ends_with(Path::new(".run").join("train-engine.json")));
    }

    #[test]
    fn empty_dir_is_not_an_engine() {
        let dir = crate::infra::testpaths::scratch("train-discovery");
        let _ = std::fs::create_dir_all(&dir);
        assert!(!is_engine_root(&dir));
    }
}
