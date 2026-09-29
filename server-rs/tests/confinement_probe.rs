//! **受限到底按什么判** —— 诊断用例（默认 `--ignore`，手动跑）。
//!
//! ```powershell
//! cargo test --test confinement_probe -- --ignored --nocapture
//! ```
//!
//! # 为什么要它
//!
//! 2026-09-29 为了给用户说清"该怎么修"，把两种说法都写进了文档：
//!
//! * **按映像位置**：可执行文件在工作区内的进程，写工作区外一律 `拒绝访问 (os error 5)`；
//! * **按继承**：受限父进程拉起的、工作区外的子进程也写不了工作区外。
//!
//! 这两条给出的处方**不一样**：只按映像位置 ⇒ 把后端挪出工作区（发布包）就够；
//! 真的按继承 ⇒ 从受限会话里启动发布包**照样**被限，必须"换个终端起"。
//! 所以这个事实必须量出来，不能靠推断 —— 本文件就是量它的地方。
//!
//! ⚠ **不许把它的结论写成断言**（AGENTS §12 纪律）："这台机器哪个目录不可写"是环境属性，
//! 不是被测行为。所以这里只**打印**，断言只钉住与机器无关的那一半
//! （"能写"与"不能写"两种结果都算通过）。
//!
//! 顺带提醒：`cargo test` 跑起来的测试二进制**就在工作区内**
//! （`server-rs\target\debug\deps\*.exe`），所以本文件本身就是"工作区内映像"那一档的样本。

use std::path::{Path, PathBuf};
use std::process::Command;

fn outside_probe_dir() -> PathBuf {
    // 工作区之外的一个现成目录（写不写得进去本身就是被测的事，所以不许先建）
    PathBuf::from(std::env::var("TEMP").unwrap_or_else(|_| r"C:\Windows\Temp".to_string()))
}

/// 真去建一个文件，返回系统的原话（`拒绝访问。 (os error 5)` 那一串）。
fn try_write(dir: &Path, name: &str) -> Result<(), String> {
    let target = dir.join(name);
    match std::fs::write(&target, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&target);
            Ok(())
        }
        Err(e) => Err(format!("{e}（os error {:?}）", e.raw_os_error())),
    }
}

#[test]
#[ignore = "诊断用例：结论随会话/沙箱档位而变，只打印不判定"]
fn 测_本进程_以及它拉起的_工作区外子进程_能不能写工作区外() {
    let exe = std::env::current_exe().expect("本测试二进制的路径");
    println!("测试二进制映像：{}", exe.display());
    println!("工作区内？{}", exe.starts_with(env!("CARGO_MANIFEST_DIR")));

    let dir = outside_probe_dir();
    println!("探针目录（工作区外）：{}", dir.display());

    let me = try_write(&dir, &format!("comfyhub-probe-self-{}", std::process::id()));
    println!("① 本进程直接写           → {}", match &me {
        Ok(()) => "写得进".to_string(),
        Err(why) => format!("写不进：{why}"),
    });

    let child_name = format!("comfyhub-probe-child-{}", std::process::id());
    let child_target = dir.join(&child_name);
    // ⚠ **不带引号**：`Command::args` 在 Windows 上会把 `"` 转义成 `\"`，而 cmd.exe 不认那种转义
    // （实测报的是 "The filename, directory name, or volume label syntax is incorrect." ——
    // 那是**命令行语法错**，不是权限错，别把它读成"子进程也被限了"）。
    let child = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()))
        .args(["/c", &format!("echo probe> {}", child_target.display())])
        .output();
    match child {
        Ok(output) if output.status.success() && child_target.exists() => {
            let _ = std::fs::remove_file(&child_target);
            println!("② 工作区外的子进程写     → 写得进（**不受继承影响**）");
        }
        Ok(output) => println!(
            "② 工作区外的子进程写     → 没写成：status={:?} stderr={}（看 stderr 是不是**语法错**：是的话这条无效）",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => println!("② 起子进程失败：{e}"),
    }

    // ③ 与结果无关、但值得钉住的那一半：工作区内**一定**写得进（受限会话里也成立）
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(".scratch");
    std::fs::create_dir_all(&inside).unwrap();
    try_write(&inside, &format!("comfyhub-probe-inside-{}", std::process::id()))
        .expect("工作区内一定写得进 —— 这一半在任何会话里都成立");
    println!("③ 工作区内写             → 写得进（这一半与沙箱档位无关）");

    println!(
        "\n结论判读：① 写不进 + ② 写得进 → **按映像位置**（把后端挪出工作区就够）；\
         ① 写不进 + ② 也写不进 → **按继承**（换终端起发布包才行）"
    );
}
