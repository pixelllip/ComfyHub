//! `workflow_search` 的单元测试（主体代码在 [`workflow_search`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 截断按字符数而不是字节数() {
    let s = "雨夜霓虹".repeat(200);
    assert_eq!(take_chars(&s, 400).chars().count(), 400);
    // 中文一个字 3 字节：按字节截早就碎成半个字符了
    assert_eq!(take_chars("雨", 1), "雨");
}

#[test]
fn 可空键照写_null_而不是省掉键() {
    // 这一条盯的是"与 Kotlin 手拼 JSON 的形状一致"（见文件头那条说明）
    let value = opt_str(None);
    assert!(value.is_null());
    let obj = json!({ "a": opt_str(None) });
    assert!(obj.as_object().unwrap().contains_key("a"));
}
