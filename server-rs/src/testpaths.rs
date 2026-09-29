//! **测试暂存目录的唯一真源**（`cargo test` 的落盘位置）。
//!
//! # 为什么不能直接用 `std::env::temp_dir()`
//!
//! 2026-09-29 实测（AGENTS §12）：**限制是按"映像位置"判的** —— 映像（可执行文件）在
//! 工作区内的进程，写工作区外一律 `拒绝访问 (os error 5)`。`cargo test` 跑起来的
//! 测试二进制正是 `server-rs\target\debug\deps\comfyhub_server_rs-*.exe`，**在工作区内**，
//! 所以在本会话这种受限环境里，任何 `std::env::temp_dir()` 的用例都会红。
//!
//! 本机实测那一轮：`cargo test --lib` **539 过 / 69 红**，红的原因全都是
//! `Os { code: 5, kind: PermissionDenied }`，与被测代码无关 —— 这就是"照着红的用例去改代码"
//! 会把人带偏的场景（也是 2026-09-28 那批现场描述被写歪的同一类问题）。
//!
//! # 规矩
//!
//! * **用例里要落盘，一律用 [`scratch`]**，别自己拼 `temp_dir()`。
//! * 目录落在 `<crate>\target\.scratch\<标签>-<纳秒>`：`target` 已被 git 忽略，
//!   标签与纳秒保证并发的多个测试互不干扰（同一标签多个用例也不会撞）。
//! * 想造"**某个目录写不进去**"的场景（`os error 5`）：**不要去改 ACL**（既慢又依赖机器状态），
//!   用"在目标文件名上放一个同名目录" —— Windows 上"在目录的位置建文件"必然回
//!   `ERROR_ACCESS_DENIED`，与受限现场同一个错码，在任何机器上都成立
//!   （`crate::image_edit` 与 `crate::confinement` 的用例都是这个手法）。

use std::path::{Path, PathBuf};

/// 暂存根：`<crate>\target\.scratch`。已随 `target/` 一起被 git 忽略。
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(".scratch")
}

/// 一个新的、带标签的暂存目录（**已创建**，可以直接往里写）。
///
/// `label` 只用于区分用途与排查残留目录，允许重复（纳秒后缀保证不撞）。
pub fn scratch(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    // 标签来自用例代码，不是用户输入；仍然清一遍路径分隔符，省得拼出子目录
    let safe: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let dir = root().join(format!("{safe}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("建测试暂存目录");
    dir
}

/// 收尾：删掉一个 [`scratch`] 目录（用例失败时也别留下垃圾）。
///
/// 删不掉不报错 —— 收尾不该把用例变成红的。
pub fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 暂存目录在项目内_而且真的建出来了() {
        let dir = scratch("testpaths");
        // ⚠ 钉住"在项目内"这一条：`target` 是 workspace 的一部分，
        // 受限会话里"映像在工作区内 → 只写得进工作区"，反过来用同样成立。
        assert!(dir.starts_with(root()), "{:?}", dir);
        assert!(dir.is_dir(), "{:?} 建不出来", dir);
        cleanup(&dir);
        assert!(!dir.exists());
    }

    #[test]
    fn 同一个标签两次调用不撞车() {
        let a = scratch("same");
        let b = scratch("same");
        assert_ne!(a, b, "并发的同名用例必须各有各的目录");
        cleanup(&a);
        cleanup(&b);
    }

    #[test]
    fn 标签里的路径分隔符不会拼出子目录() {
        let dir = scratch(r"a\b/../c");
        assert_eq!(dir.parent().unwrap(), root());
        cleanup(&dir);
    }
}
