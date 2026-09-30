//! `pending` 的单元测试（主体代码在 [`pending`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

//! ⚠ 提问那半边（形状 / ISO→毫秒 / 脏 JSON）的用例跟着实现搬到了
//! [`crate::ai::questions::tests`] —— 形状只有一份，用例也就不该有两份。

use super::*;

#[test]
fn 没有待办时也是两个空数组() {
    let mut body = Map::new();
    body.insert("asks".into(), Value::Array(Vec::new()));
    body.insert("approvals".into(), Value::Array(Vec::new()));
    assert_eq!(body.len(), 2, "两个键都要在（不是 null、也不是少一个）");
}
