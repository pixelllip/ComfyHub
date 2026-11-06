//! 角色卡门面的回归：**旧名字仍然指向同一份仓储**。
//!
//! 仓储 / 合并 / 示例的细节用例在 [`crate::library::cards::tests`]；
//! 这里只钉"门面没有接错种类"这一条 —— 接错了表现是"查角色卡却回画风卡"。

use super::*;

fn state() -> AppState {
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .expect("读 schema");
    conn.execute_batch(&schema).expect("建 schema");
    AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"))
}

#[test]
fn 角色卡门面查的是角色卡而不是画风卡() {
    let state = state();
    let saved = tool_save(
        &state,
        &serde_json::json!({ "name": "shinku", "prompt": "A girl with red eyes and silver hair." }),
    )
    .unwrap();
    assert_eq!(saved.payload["name"], "shinku");
    assert_eq!(
        saved.payload["prompt"],
        "A girl with red eyes and silver hair."
    );

    // 画风卡那一边没有这张卡
    let other =
        crate::library::style_cards::tool_search(&state, &serde_json::json!({})).unwrap_err();
    assert_eq!(other.code, "NOT_FOUND");
    assert!(other.message.contains("画风卡"), "{}", other.message);

    let hit = tool_search(&state, &serde_json::json!({ "query": "shinku" })).unwrap();
    assert!(hit.content.contains("角色卡：命中 1 张"), "{}", hit.content);
    assert!(
        hit.content
            .contains("正面提示词: A girl with red eyes and silver hair."),
        "{}",
        hit.content
    );
}
