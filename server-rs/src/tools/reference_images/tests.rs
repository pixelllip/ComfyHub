//! `reference_images` 的单元测试（纯函数，不碰网络）。

use super::*;

#[test]
fn tag_串按逗号分组且空格归一成下划线() {
    assert_eq!(build_tags("long hair", false), "long_hair+rating%3Ageneral");
    assert_eq!(
        build_tags("night, city", false),
        "night+city+rating%3Ageneral",
        "逗号 = 多个 tag"
    );
    // 括号（消歧 tag）不能被拆成两个词
    assert_eq!(
        build_tags("shinku (nte)", false),
        "shinku_%28nte%29+rating%3Ageneral"
    );
    // 开 NSFW 时不加 rating 过滤；关着时模型说了也不算
    assert_eq!(build_tags("blunt_bangs", true), "blunt_bangs");
    assert_eq!(
        build_tags("a,b,c,d,e", false),
        "a+b+c+d+rating%3Ageneral",
        "最多 4 个词"
    );
}

#[test]
fn 选图优先采样图_没有才用预览图() {
    let mut post = Post {
        id: 1,
        preview_url: Some("https://cdn.example/preview.jpg".into()),
        large_url: Some("https://cdn.example/large.jpg".into()),
        file_url: Some("https://cdn.example/original.png".into()),
        width: Some(1000),
        height: Some(1500),
        rating: "g".into(),
        score: 10,
        tag_string: "blunt_bangs".into(),
    };
    assert_eq!(best_url(&post), Some("https://cdn.example/large.jpg"));
    post.large_url = None;
    assert_eq!(best_url(&post), Some("https://cdn.example/preview.jpg"));
    post.preview_url = None;
    assert_eq!(best_url(&post), None, "原图刻意不参与：20MB 对确认细节没有额外信息");
}

#[test]
fn 扩展名从地址推_认不出就_jpg() {
    assert_eq!(extension_of("https://cdn.example/a.png?x=1"), "png");
    assert_eq!(extension_of("https://cdn.example/a.webp"), "webp");
    assert_eq!(extension_of("https://cdn.example/a"), "jpg");
}

/// 活体：整条工具链（查 → 下 → 落附件 → 回 attachmentId）。
///
/// 默认忽略；排错时手动跑：
/// `cargo test --lib live_reference_image_tool -- --ignored --nocapture`
#[tokio::test]
#[ignore = "活体：要出网（danbooru 国内需代理）"]
async fn live_reference_image_tool_saves_a_local_attachment() {
    let dir = crate::infra::testpaths::scratch(&format!("ref-images-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    let state = AppState::new(conn, ":memory:", dir.join("media"));

    let outcome = tool_search(&state, &serde_json::json!({ "query": "blunt_bangs", "limit": 1 }))
        .await
        .expect("工具要能跑通");
    let images = outcome.payload["images"].as_array().cloned().unwrap_or_default();
    assert!(!images.is_empty(), "至少要到一张图：{}", outcome.content);
    let id = images[0]["attachmentId"].as_str().unwrap();
    let dto = attachments::get(&state, id).unwrap().expect("图必须真的落成附件");
    assert_eq!(dto.kind, "image");
    assert!(outcome.content.contains(id), "正文里要给出 attachmentId");
    std::fs::remove_dir_all(dir).ok();
}
