//! 训练引擎 MCP 桥的**活体**回归（需要本机装了 anima-lora-next）。
//!
//! 默认 `#[ignore]`：CI / 没装引擎的机器上不该因此变红。手动跑：
//!
//! ```powershell
//! cargo test --test train_mcp_live -- --ignored --nocapture
//! ```
//!
//! 它验证的是 Rust 这一侧的真实行为，而不是 Python 单测：
//! ① 发现引擎 → 起 `anima_daemon/mcp.py` 子进程 → 握手 + 拉工具清单；
//! ② 工具名统一带 `anima_` 前缀；
//! ③ 真的调一次只读工具（`anima_studio_projects`），拿到 daemon 的 JSON。

use serde_json::json;

#[tokio::test]
#[ignore = "需要本机 anima-lora-next（train.py + .venv）"]
async fn 训练引擎的_mcp_桥能列工具并调用只读工具() {
    let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("server-rs 的父目录就是项目根");

    let registry = std::sync::Arc::new(comfyhub_server_rs::tools::mcp::McpRegistry::new());
    registry
        .ensure_anima(project_root)
        .await
        .expect("本机应该能找到 anima-lora-next 引擎并起 MCP 桥");

    let tools = registry.specs();
    let names: Vec<&str> = tools.iter().map(|spec| spec.name).collect();
    assert!(
        names.contains(&"anima_studio_projects"),
        "训练工具必须带 anima_ 前缀；实际：{names:?}"
    );
    assert!(
        names.contains(&"anima_studio_train"),
        "训练提交工具缺失；实际：{names:?}"
    );

    let outcome = registry
        .call(project_root, "anima_studio_projects", &json!({}))
        .await
        .expect("只读的 studio_projects 不该失败");
    assert!(
        outcome.content.trim_start().starts_with('[') || outcome.content.contains("slug"),
        "应该拿到项目数组，实际：{}",
        outcome.content.chars().take(300).collect::<String>()
    );
    assert_eq!(outcome.payload["source"], "anima-daemon");
}
