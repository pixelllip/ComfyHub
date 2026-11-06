//! `--watch-stdin`：**App 强杀后不留孤儿**的那条保障，端到端量一遍。
//!
//! # 它防的是哪个现场
//!
//! 桌面 App（`lib\core\backend_launcher.dart`）直接起这个 exe，正常关窗会先 `kill`；
//! 但**任务管理器 / 崩溃**那条路上没人来杀 —— 后端靠"stdin 是 App 给的管道，
//! 写端只有 App 持有"这一点自救：App 一死，`read` 回 `0`（EOF），后端优雅退出
//! （见 `main.rs` 的 `spawn_stdin_watchdog`）。
//!
//! # 怎么测（不依赖"这台机器的端口/目录状态"）
//!
//! 1. `Command` 起真二进制（`CARGO_BIN_EXE_*`），stdin 用 **piped**、stdout piped，
//!    `COMFYHUB_NO_PORT_FILE=1`（不覆盖真后端的端口记录）、库落在
//!    `testpaths::scratch()`（AGENTS §1：Rust 用例只许写在 `target\.scratch\`）；
//! 2. 从 stdout 里读出 `监听 http://127.0.0.1:<port>`（`main.rs` 绑好后打的那行）；
//! 3. **丢掉子进程的 stdin 写端**（drop 掉 `ChildStdin`）= 模拟"App 进程消失"；
//! 4. 断言进程在 15 秒内退出 —— 不开 `--watch-stdin` 时它只会永远挂着（对照组在下面）。
//!
//! ⚠ 端口给 `0`（让操作系统挑），不与任何在跑的服务抢位置。

use std::io::Read;
use std::process::{Child, Command, Stdio};

use comfyhub_server_rs::infra::testpaths;

struct Server {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn scratch_root(tag: &str) -> std::path::PathBuf {
    let dir = testpaths::scratch(tag);
    std::fs::create_dir_all(&dir).expect("建 scratch 目录");
    dir
}

/// 起一个真后端，返回（进程 + 它监听的端口）。
fn spawn(watch_stdin: bool, tag: &str) -> (Server, u16) {
    let root = scratch_root(tag);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_comfyhub-server-rs"));
    if watch_stdin {
        // ⚠ 必须真的把开关传下去：少了它两组用例都只是"没开 watchdog"的假绿
        cmd.arg("--watch-stdin");
    }
    cmd.arg("--db")
        .arg(root.join("comfyhub.db"))
        .arg("--storage")
        .arg(root.join("storage"))
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("0")
        .env("COMFYHUB_NO_PORT_FILE", "1")
        .env("COMFYHUB_PROJECT_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("起后端进程");
    let stdin = child.stdin.take().expect("stdin 已按 piped 配好");

    // 从 stdout 读出"监听 http://127.0.0.1:<port>"那一行（绑好端口就会打，最多等 30 秒）
    let mut stdout = child.stdout.take().expect("stdout 已按 piped 配好");
    let mut port = 0u16;
    let mut buf = [0u8; 512];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut carry = String::new();
    while std::time::Instant::now() < deadline {
        let n = match stdout.read(&mut buf) {
            Ok(0) => panic!("后端 stdout 提前关闭（进程可能没起来）"),
            Ok(n) => n,
            Err(e) => panic!("读后端 stdout 失败：{e}"),
        };
        carry.push_str(&String::from_utf8_lossy(&buf[..n]));
        while let Some(pos) = carry.find('\n') {
            let line: String = carry.drain(..=pos).collect();
            if let Some(rest) = line.trim().strip_prefix("监听 http://127.0.0.1:") {
                let end = rest.find(['/', '（', '(']).unwrap_or(rest.len());
                port = rest[..end].parse().expect("端口是数字");
            }
        }
        if port != 0 {
            break;
        }
    }
    assert_ne!(
        port, 0,
        "30 秒内没读到监听端口（--watch-stdin={watch_stdin}）"
    );

    (
        Server {
            child,
            stdin: if watch_stdin { Some(stdin) } else { None },
        },
        port,
    )
}

/// 丢弃 stdin 写端后，后端应在 15 秒内自己退出（优雅退出，不是被 kill）。
#[test]
fn stdin关闭后端自己退出() {
    let (mut server, port) = spawn(true, "watch-stdin");
    // 先确认它真的在听（起来的是个活服务，而不是一个马上要死的进程）
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "端口 {port} 没在听"
    );

    // ⚠ 这一行就是"App 进程消失"：写端一 drop，管道关，后端的 read 回 0
    drop(server.stdin.take());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        match server.child.try_wait().expect("try_wait") {
            Some(_) => {
                exited = true;
                break;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    assert!(
        exited,
        "stdin 关闭 15 秒后后端还活着 —— watchdog 失效，App 强杀会留孤儿"
    );
}

/// 对照组：**不丢** stdin（App 还活着），后端就**不该**退出。
///
/// 这一半是防"watchdog 太敏感"的回归：管道开着时它必须安静等着，
/// 不然 App 正常用着用着后端自己退了，比孤儿更糟。
#[test]
fn stdin还开着就不许自己退出() {
    let (mut server, port) = spawn(true, "watch-stdin-alive");
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());

    // App 活着的 3 秒里，后端必须一直活着（正常启动到可服务的实测 <1s，3 秒足够说明问题）
    for _ in 0..30 {
        assert!(
            server.child.try_wait().expect("try_wait").is_none(),
            "stdin 还开着，后端却自己退了 —— watchdog 太敏感"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
