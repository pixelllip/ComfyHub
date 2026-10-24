//! 卡片库端点（角色卡 / 画风卡）：形状、单版本正文、示例产物、错误码。
//!
//! 仓储细节在 `server-rs/src/library/cards/tests.rs`；这里盯 **HTTP 那一层**：
//! 键名（前端按它解析）、`PUT` 的全量替换语义、`examples` 的 404/400。

use super::common::*;

#[tokio::test]
async fn style_card_has_one_prompt_and_examples_round_trip() {
    let state = state();
    let (status, card) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/style-cards",
        serde_json::json!({
            "name": "赛博国风",
            "prompt": "A digital anime illustration painted semi-thick, low-chroma blue-gray field, luminous finish.",
            "negative": "photo, 3d"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{card}");
    assert_eq!(
        keys(&card),
        vec!["createdAt", "examples", "id", "name", "negative", "prompt", "updatedAt"],
        "卡片只剩一版自然语言正文：不再有 tagPrompt 键"
    );
    assert!(card["prompt"].as_str().unwrap().contains("semi-thick"));
    assert!(card["examples"].as_array().unwrap().is_empty());

    let id = card["id"].as_i64().unwrap();
    // 夹具里的产物 id=100 是真实存在的
    let (status, card) = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/ai/style-cards/{id}/examples"),
        serde_json::json!({ "mediaIds": [100] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert_eq!(card["examples"][0]["id"], 100);
    assert_eq!(card["examples"][0]["thumbUrl"], "/api/media/100/thumb");

    // 列表也带示例（界面不额外发请求）
    let (status, list) = get_json(router_for(state.clone()), "/api/ai/style-cards?q=赛博").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["examples"][0]["id"], 100);

    // 全量替换成空 = 清空示例
    let (status, card) = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/ai/style-cards/{id}/examples"),
        serde_json::json!({ "mediaIds": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(card["examples"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn card_examples_reject_missing_media_and_missing_cards() {
    let state = state();
    let (status, card) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/character-cards",
        serde_json::json!({ "name": "shinku", "prompt": "A girl with red eyes and silver hair." }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = card["id"].as_i64().unwrap();

    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/ai/character-cards/{id}/examples"),
        serde_json::json!({ "mediaIds": [999999] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("999999"), "{body}");

    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/character-cards/424242/examples",
        serde_json::json!({ "mediaIds": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 角色卡与画风卡是两套端点：同名不会互相串
    let (status, styles) = get_json(router_for(state), "/api/ai/style-cards").await;
    assert_eq!(status, StatusCode::OK);
    assert!(styles.as_array().unwrap().is_empty());
}
