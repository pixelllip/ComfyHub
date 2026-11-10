//! `mobile_tools` 的单元测试（主体代码在 [`mobile_tools`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn tools_are_sorted_and_mobile_only() {
    let mut names: Vec<&str> = MOBILE_TOOLS.iter().map(|t| t.name).collect();
    let sorted = {
        let mut n = names.clone();
        n.sort();
        n
    };
    assert_eq!(
        names, sorted,
        "工具必须按 name 升序（顺序进了 kitVersion 签名）"
    );
    names.dedup();
    assert_eq!(names.len(), MOBILE_TOOLS.len(), "工具名不许重复");
    for tool in MOBILE_TOOLS {
        assert!(
            tool.scope == "MOBILE" || tool.scope == "BOTH",
            "{} 的 scope 不是手机可执行档",
            tool.name
        );
        assert!(
            !tool.name.starts_with("comfy_submit"),
            "R11：手机侧不许复用提交工具名"
        );
    }
}

/// 排队提交**必须在**手机目录里：`declaredTo = kit ∩ 本地实现` —— 不在 kit 里的话，
/// 手机 harness 就算实现了它也**不会被宣称**，模型永远看不到它（M5 手机侧那次实测就是这样）。
#[test]
fn queue_submit_is_declared_and_the_desktop_catalog_does_not_have_it() {
    assert!(
        MOBILE_TOOLS.iter().any(|t| t.name == "comfy_queue_submit"),
        "comfy_queue_submit 不在手机工具目录里 → 模型看不到它（M5 的端到端会断在这里）"
    );
    assert!(
        !crate::tools::tools::implemented()
            .iter()
            .any(|t| t.name == "comfy_queue_submit"),
        "它是**手机专属**（R11）：桌面那份（已实现清单）里不许有这个名字"
    );
}

#[test]
fn every_schema_is_parseable_json() {
    for tool in MOBILE_TOOLS {
        let value: serde_json::Value = serde_json::from_str(tool.input_schema)
            .unwrap_or_else(|e| panic!("{} 的 schema 不是 JSON: {e}", tool.name));
        assert_eq!(
            value["type"], "object",
            "{} 的 schema 顶层必须是 object",
            tool.name
        );
    }
}
