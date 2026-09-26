//! 后端实际监听的端口（`<项目根>\.run\api.json`）—— Kotlin `ApiPortFile.kt` 的逐条移植。
//!
//! # 为什么要有这个文件（**切流的硬前提**）
//!
//! 端口过去写死在好几处：`comfyhub.ps1`（探测）、`server.ps1`（启动）、App 的
//! `settings_store.dart`（连谁）、ComfyUI 捕获扩展（往哪推）—— 而且它们互不知道。
//! 现在只有一个真源：**后端绑好端口后写的这一个文件**，其余三方都只读。
//!
//! ⚠ **Rust 侧此前根本没有它**：App 与脚本都靠它发现端口，所以"把 `server.ps1` 指向 Rust"
//! 这件事**必须先有这一步**，否则切过去的表现是 App 对着旧端口等 90 秒（`docs/pitfalls.md`
//! 记过一模一样的现场）。
//!
//! # 四条纪律
//!
//!  1. **只由后端写**（谁在听谁知道）；脚本与前端**只读**，绝不自己编一个端口写进去；
//!  2. **原子写入**（先写 `.tmp` 再 `move` 覆盖），并且**带退避重试**：读侧是轮询的，
//!     读到半个 JSON 就是"启动页偶尔连不上"的幽灵 bug；Windows 上替换一个正被别的进程
//!     打开的文件会直接 IOException（restart 期间旧后端还没退干净、安全软件在扫），
//!     所以失败要重试而不是一次就放弃；
//!  3. **优雅退出时删掉它，但只删"还是本进程写的那一份"**（`pid` 对得上）——
//!     否则重启场景下新进程刚写的记录会被旧进程的收尾删掉；
//!  4. **写不进不要让后端起不来**：端口记不下最多让前端多试几次，服务本身照常可用。

use std::path::{Path, PathBuf};

/// `.run` 目录名（与 `scripts\*.ps1` 的 `$RunDir` 约定一致）。
pub const RUN_DIR: &str = ".run";
/// 文件名固定：脚本 / 前端 / Python 扩展三边都按这个名字找。
pub const FILE_NAME: &str = "api.json";

/// 端口状态（`ApiPortState`）—— 文件里**不含任何机密**（只有端口 / PID / 时间 / URL）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPortState {
    /// 后端实际监听的端口
    pub port: u16,
    /// 写这个文件的后端进程 PID（脚本据此判断"是不是同一个后端"）
    pub pid: u32,
    /// 监听地址（默认回环；开了 `COMFYHUB_ALLOW_REMOTE` 时是 `0.0.0.0`）
    pub host: String,
    /// 给"人"看的完整地址（脚本与扩展直接拿去用，不必自己拼）
    pub url: String,
    /// 写文件的时刻（ISO-8601，排查"这个文件是不是几天前的"）
    pub started_at: String,
    /// 用户/脚本**指定**的候选端口（0 = 一开始就想让系统挑）
    pub requested_port: u16,
    /// 是不是从候选端口回落到随机端口的（true = 候选端口没绑上）
    pub fell_back: bool,
}

impl ApiPortState {
    /// 与 Kotlin `ApiPortState.toJson()` **逐键一致**（7 个键，驼峰）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "port": self.port,
            "pid": self.pid,
            "host": self.host,
            "url": self.url,
            "startedAt": self.started_at,
            "requestedPort": self.requested_port,
            "fellBack": self.fell_back,
        })
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        let port = value.get("port")?.as_u64()? as u16;
        Some(Self {
            port,
            pid: value.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            host: value.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string(),
            url: value.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            started_at: value.get("startedAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            requested_port: value.get("requestedPort").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            fell_back: value.get("fellBack").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }
}

/// 状态文件在项目根下的位置。
pub fn path(project_root: &Path) -> PathBuf {
    project_root.join(RUN_DIR).join(FILE_NAME)
}

/// 拼一个"能连上"的地址。
///
/// `COMFYHUB_HOST` 显式给了非回环地址（局域网 / 远程）时，对**本机**客户端来说
/// `0.0.0.0` 这种"全部网卡"地址是连不上的 —— 那种情况回落到 `127.0.0.1`，
/// 否则 App 会拿着一个永远连不上的 URL 干等。
pub fn url_for(host: &str, port: u16) -> String {
    let connect_host = match host {
        "0.0.0.0" | "::" | "[::]" | "" => "127.0.0.1",
        other => other,
    };
    let shown = if connect_host.contains(':') && !connect_host.starts_with('[') {
        format!("[{connect_host}]")
    } else {
        connect_host.to_string()
    };
    format!("http://{shown}:{port}")
}

/// 原子写入（写 `.tmp` → 覆盖），失败**退避重试 5 次**后放弃。
///
/// 放弃时**不报错也不退出**：写不了不该让后端起不来（Kotlin 同）。
pub fn write(project_root: &Path, state: &ApiPortState) -> Option<PathBuf> {
    let target = path(project_root);
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = target.with_file_name(format!("{FILE_NAME}.tmp"));
    let text = state.to_json().to_string();
    if std::fs::write(&tmp, text).is_err() {
        return None;
    }
    for attempt in 0..5u64 {
        if std::fs::rename(&tmp, &target).is_ok() {
            eprintln!(
                "[api-port] 已记录后端端口：{}（port={} pid={}{}）",
                target.display(),
                state.port,
                state.pid,
                if attempt > 0 { format!("，重试 {attempt} 次") } else { String::new() }
            );
            return Some(target);
        }
        // 个别文件系统（网络盘 / FAT）不支持原子替换：退化成"复制 + 删源"
        if std::fs::copy(&tmp, &target).is_ok() {
            let _ = std::fs::remove_file(&tmp);
            return Some(target);
        }
        std::thread::sleep(std::time::Duration::from_millis(200 * (attempt + 1)));
    }
    eprintln!("[api-port] 写端口状态文件失败（试了 5 次），服务照常运行：{}", target.display());
    let _ = std::fs::remove_file(&tmp);
    None
}

/// 读出来（读不到 / 格式坏了都回 `None`，**绝不抛**）。
pub fn read(project_root: &Path) -> Option<ApiPortState> {
    let text = std::fs::read_to_string(path(project_root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    ApiPortState::from_json(&value)
}

/// 删除状态文件 —— **只有确认它还是本进程写的那一份才删**。
///
/// 场景：用户点「重启」，新的后端已经起来并写好了新文件，这时旧进程的收尾才跑完；
/// 无条件删会把**新进程的**记录删掉，前端随后就再也找不到后端了。
pub fn delete_if_owned(project_root: &Path, pid: u32) -> bool {
    let mine = read(project_root).map(|state| state.pid == pid).unwrap_or(false);
    if !mine {
        eprintln!("[api-port] 端口状态文件不属于本进程（pid 不匹配），保留不删");
        return false;
    }
    let removed = std::fs::remove_file(path(project_root)).is_ok();
    if removed {
        eprintln!("[api-port] 已清理端口状态文件");
    }
    removed
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("comfyhub-portfile-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn state(port: u16, pid: u32) -> ApiPortState {
        ApiPortState {
            port,
            pid,
            host: "127.0.0.1".to_string(),
            url: url_for("127.0.0.1", port),
            started_at: "2026-09-26T01:00:00Z".to_string(),
            requested_port: port,
            fell_back: false,
        }
    }

    #[test]
    fn 形状与_kotlin_逐键一致() {
        let json = state(51000, 4242).to_json();
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["port", "pid", "host", "url", "startedAt", "requestedPort", "fellBack"],
            "键集合与 Kotlin `ApiPortState` 必须逐字一致（脚本与前端按名字读）"
        );
        assert_eq!(json["url"], serde_json::json!("http://127.0.0.1:51000"));
        assert_eq!(json["fellBack"], serde_json::json!(false));
    }

    #[test]
    fn 写与读往返一致_且落在_run_目录下() {
        let root = temp_root("roundtrip");
        let written = write(&root, &state(51234, 999)).expect("要写得下去");
        assert_eq!(written, root.join(RUN_DIR).join(FILE_NAME));
        assert!(written.is_file());
        // `.tmp` 不许留下来（原子替换掉了）
        assert!(!root.join(RUN_DIR).join(format!("{FILE_NAME}.tmp")).exists());
        let read_back = read(&root).expect("读得回来");
        assert_eq!(read_back.port, 51234);
        assert_eq!(read_back.pid, 999);
        assert_eq!(read_back.url, "http://127.0.0.1:51234");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 只删自己那一份() {
        let root = temp_root("owned");
        write(&root, &state(52000, 111)).unwrap();
        // 别的进程来删：不许删（重启场景下新进程的记录不能被旧进程收尾删掉）
        assert!(!delete_if_owned(&root, 222));
        assert!(path(&root).is_file(), "不属于自己的那份必须留着");
        // 自己来删：删掉
        assert!(delete_if_owned(&root, 111));
        assert!(!path(&root).is_file());
        // 文件本来就不在：也回 false（不是错误）
        assert!(!delete_if_owned(&root, 111));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 坏文件与缺文件都回_none_而不是崩() {
        let root = temp_root("broken");
        assert!(read(&root).is_none());
        std::fs::create_dir_all(root.join(RUN_DIR)).unwrap();
        std::fs::write(path(&root), "{ 这不是完整 JSON").unwrap();
        assert!(read(&root).is_none(), "半截 JSON 要当读不到");
        std::fs::write(path(&root), r#"{"pid":1}"#).unwrap();
        assert!(read(&root).is_none(), "缺 port 的要当读不到");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 全部网卡地址要给客户端回落到回环() {
        assert_eq!(url_for("0.0.0.0", 8080), "http://127.0.0.1:8080");
        assert_eq!(url_for("::", 8080), "http://127.0.0.1:8080");
        assert_eq!(url_for("", 8080), "http://127.0.0.1:8080");
        assert_eq!(url_for("127.0.0.1", 8080), "http://127.0.0.1:8080");
        assert_eq!(url_for("192.168.1.9", 8080), "http://192.168.1.9:8080");
        // IPv6 字面量要加方括号，否则拼出来的 URL 连不上
        assert_eq!(url_for("::1", 8080), "http://[::1]:8080");
        assert_eq!(url_for("[::1]", 8080), "http://[::1]:8080");
    }

    #[test]
    fn 回落端口也要如实标出来() {
        let root = temp_root("fellback");
        let mut state = state(51234, 7);
        state.requested_port = 8280;
        state.fell_back = true;
        write(&root, &state).unwrap();
        let read_back = read(&root).unwrap();
        assert_eq!(read_back.requested_port, 8280, "要能看出用户原本要的是哪个端口");
        assert!(read_back.fell_back, "`doctor` 靠它说清\"你给的端口被占了，换成了 X\"");
        let _ = std::fs::remove_dir_all(root);
    }
}
