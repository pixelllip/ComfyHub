//! `testpaths` 的单元测试（主体代码在 [`testpaths`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

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
