//! 产物：列表 / 文件流（Range + ETag）/ 缩略图 / 写侧 / 上传 / stats。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;
use axum::extract::ConnectInfo;
use std::net::SocketAddr;

/// 发一个带"来源地址"的 POST：导出端点的闸门就是按它判回环的。
fn post_with_remote(
    app: axum::Router,
    uri: &str,
    remote: SocketAddr,
    body: serde_json::Value,
) -> impl std::future::Future<Output = (StatusCode, Value)> {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .extension(ConnectInfo(remote))
        .body(Body::from(body.to_string()))
        .unwrap();
    async move {
        let response = app.oneshot(request).await.expect("请求");
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

// ===========================================================================
//  产物（P4 第 3 步）：列表 / 详情 / 原件文件流
// ===========================================================================

#[tokio::test]
async fn media_export_is_loopback_only_and_writes_the_kind_layout() {
    let state = state();
    let out = comfyhub_server_rs::infra::testpaths::scratch("export-http-out");
    std::fs::create_dir_all(&out).unwrap();
    let body = serde_json::json!({ "dir": out.display().to_string() });

    // 非回环：403，而且**文件一个都不许落**
    let (status, error) = post_with_remote(
        router_for(state.clone()),
        "/api/media/export",
        "192.168.1.50:54321".parse().unwrap(),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body={error}");
    assert!(!out.join("导出清单.json").exists(), "被拒的请求不许留下任何产物");

    // 回环：真的按类型目录落盘（夹具里 id=100 是 a.png）
    let (status, report) = post_with_remote(
        router_for(state),
        "/api/media/export",
        SocketAddr::from(([127, 0, 0, 1], 54321)),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={report}");
    assert_eq!(report["exported"], 1, "body={report}");
    assert_eq!(report["dir"], out.display().to_string());
    assert!(out.join("图片/a.png").is_file(), "按类型目录落盘");
    assert!(out.join("图片/a.png.json").is_file(), "侧车元信息");
    assert!(out.join("导出清单.json").is_file(), "总清单");
}

#[tokio::test]
async fn media_list_uses_the_same_keys_and_filters() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/media?size=10").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(keys(&body), vec!["items", "page", "pages", "size", "total"]);
    assert_eq!(body["total"], 1);
    let item = &body["items"][0];
    assert_eq!(
        keys(item),
        vec![
            "checkpoint", "collectionPreview", "collectionSize", "createdAt", "favorite", "fileUrl",
            "hasWorkflow", "id", "kind", "nsfw", "originalName", "promptId", "promptNegative",
            "promptPositive", "promptTags", "promptTitle", "seed", "sizeBytes", "source",
            "storedName", "thumbUrl", "title", "updatedAt",
        ],
        "少一个键 = 画廊里少一块内容（缩略图 URL 也在这里）"
    );
    // 夹具里这些列是 NULL → Kotlin（`explicitNulls = false`）**整个键都不发**
    for absent in ["mimeType", "width", "height", "durationMs", "sha256", "notes", "collectionId"] {
        assert!(
            !item.as_object().unwrap().contains_key(absent),
            "`{absent}` 是 NULL 就不该有这个键（不是 null）：{item}"
        );
    }
    // `collectionSize` 是**必给**的（i64，不是 Option）：画廊靠它决定要不要叠放封面 / 显示角标。
    // 夹具里这条没进组（孤儿产物）→ 1 = 只有它自己；`collectionPreview` 是空数组。
    assert_eq!(item["collectionSize"], 1, "没进组的产物 collectionSize 是 1");
    assert_eq!(item["collectionPreview"], serde_json::json!([]), "没进组就没有可叠的封面");
    // 关联提示词摘要是直出的（列表不必二次请求）
    assert_eq!(item["promptTitle"], "赛博朋克少女");
    assert_eq!(item["promptTags"], serde_json::json!(["赛博朋克"]), "产物自身标签 ∪ 提示词标签");
    assert_eq!(item["fileUrl"], "/api/media/100/file");
    assert_eq!(item["thumbUrl"], "/api/media/100/thumb");

    // 过滤：kind / promptId / untagged / favorite
    let (_, by_kind) = get_json(router_for(state.clone()), "/api/media?kind=VIDEO").await;
    assert_eq!(by_kind["total"], 0);
    let (_, by_prompt) = get_json(router_for(state.clone()), "/api/media?promptId=10").await;
    assert_eq!(by_prompt["total"], 1);
    let (_, untagged) = get_json(router_for(state.clone()), "/api/media?untagged=true").await;
    assert_eq!(untagged["total"], 0);
    // ★ 布尔筛选**必须收 `1` / `0`**（2026-10-11 修的真 bug）：
    // axum 的 `Query` 对 `Option<bool>` 只认 `true`/`false`，发 `1` 会**整个请求 400** ——
    // 而 Dart 那边（`listMedia` 的 `favorite: favorite ? '1' : '0'`）发的就是 `1`。
    // 表现：画廊点一下「仅收藏」→ "加载失败 HTTP 400"，用户完全猜不到是那个开关干的。
    // 这条用例盯的就是"再也不许回到只认 true/false"。
    for flag in ["1", "true"] {
        let (status, body) =
            get_json(router_for(state.clone()), &format!("/api/media?favorite={flag}")).await;
        assert_eq!(status, StatusCode::OK, "favorite={flag} 不该 400（body={body}）");
        assert_eq!(body["total"], 0, "夹具里没有收藏的产物");
    }
    for flag in ["1", "true"] {
        let (status, body) =
            get_json(router_for(state.clone()), &format!("/api/media?collapse={flag}")).await;
        assert_eq!(status, StatusCode::OK, "collapse={flag} 不该 400（body={body}）");
    }
    // 认不出来的值**当没传**，绝不 400（一个筛选参数写错不该让整个列表挂掉）
    let (status, _) = get_json(router_for(state.clone()), "/api/media?favorite=maybe").await;
    assert_eq!(status, StatusCode::OK, "脏值当没传，不是 400");

    // ★★ 折叠的**总数必须等于"未分组的 + 每个集合一个代表"**（2026-10-11 真 bug）。
    //
    // 曾经写成"未分组的 或 不是任何集合成员" —— 那等于把所有集合成员全排除掉，
    // 于是**集合的格子在画廊里根本不出现**，而且 `pages=1` 翻页也补不回来
    // （用户看到的是"这几张图没了"）。
    //
    // 现场构造：一条未分组的 + 一个两成员的集合 → 折叠后必须剩 **2** 格。
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_collections (group_key, kind, origin, rule_version)
                 VALUES ('k-fold', 'IMAGE', 'auto', 1)",
                [],
            )?;
            let cid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE media_assets SET collection_id = ?1 WHERE id = 100",
                rusqlite::params![cid],
            )?;
            conn.execute(
                "INSERT INTO media_assets (kind, title, original_name, stored_name, collection_id, created_at)
                 VALUES ('IMAGE', 'b', 'b.png', 'b.png', ?1, '2099-01-01T00:00:00.000Z')",
                rusqlite::params![cid],
            )?;
            conn.execute(
                "INSERT INTO media_assets (kind, title, original_name, stored_name)
                 VALUES ('IMAGE', 'c', 'c.png', 'c.png')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (_, folded) = get_json(router_for(state.clone()), "/api/media?collapse=1").await;
    assert_eq!(
        folded["total"], 2,
        "折叠后 = 未分组的 1 条 + 这个集合的 1 个代表；集合成员不许被整批漏掉"
    );
    let (_, plain) = get_json(router_for(state.clone()), "/api/media").await;
    assert_eq!(plain["total"], 3, "不折叠时三条都在");
    // 标签过滤是**精确匹配 normalized**（与 Kotlin `TagRepo.promptTagFilter` 一致，
    // 不是 LIKE）：所以要么给全名，要么筛不到 —— 这里给全名「赛博朋克」
    let (_, by_tag) =
        get_json(router_for(state.clone()), "/api/media?tags=%E8%B5%9B%E5%8D%9A%E6%9C%8B%E5%85%8B").await;
    assert_eq!(by_tag["total"], 1, "按提示词的标签也能筛到产物");
    // 给半截名字筛不到（这条防的是"某天有人顺手把 IN 改成 LIKE"）
    let (_, partial) = get_json(router_for(state.clone()), "/api/media?tags=%E8%B5%9B%E5%8D%9A").await;
    assert_eq!(partial["total"], 0, "标签是精确匹配，不是前缀/模糊匹配");

    let (status, body) = get_json(router_for(state.clone()), "/api/media/100").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["storedName"], "a.png");
    let (status, _) = get_json(router_for(state.clone()), "/api/media/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_file_serves_bytes_with_etag_and_range() {
    let state = state();
    // 给夹具补一个内容哈希：ETag 直接用 sha256（内容寻址）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET sha256 = 'deadbeef' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let app = router_for(state.clone());

    // 200 全量
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/api/media/100/file").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("etag").unwrap(), "\"deadbeef\"");
    assert_eq!(response.headers().get("accept-ranges").unwrap(), "bytes");
    assert_eq!(response.headers().get("content-length").unwrap(), "16");
    assert!(response
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("inline;"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"0123456789abcdef");

    // If-None-Match 命中 → 304（省掉整段视频的流量）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("if-none-match", "\"deadbeef\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

    // Range → 206 + Content-Range（视频拖动进度条靠它）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("range", "bytes=4-7")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers().get("content-range").unwrap(), "bytes 4-7/16");
    assert_eq!(response.headers().get("content-length").unwrap(), "4");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"4567", "Range 要给的是那一段字节，不是从头开始");

    // 多段 Range 不支持 → 回 200 全量（**不是**格式不对的 206）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("range", "bytes=0-1,4-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_file_rejects_path_traversal_and_missing_files() {
    let state = state();
    // 把库里的 stored_name 改成一条越界路径（库被人手改过就是这个样子）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = '../secret.txt' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/file").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "目录穿越必须被挡住");

    // 文件不在磁盘上（比如被用户删了）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = 'gone.png' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/media/100/file").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("文件已丢失"), "body={body}");
}

/// 缩略图：真图能生成 JPEG 并缓存；非图片 204；坏图回退原图；poster/upload 仍是 501。
#[tokio::test]
async fn media_thumb_generates_a_cached_jpeg_and_falls_back_honestly() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();

    // 夹具换成一张**真图**（800×400），不然测的是"坏图回退"那条分支
    let mut img = image::RgbImage::new(800, 400);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([10, 200, 30]);
    }
    img.save(media_dir.join("a.png")).unwrap();

    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/media/100/thumb").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/jpeg");
    assert!(response.headers().get("x-thumbnail").is_some(), "命中缓存的标记要带上");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG 魔数（说明真的编码过，不是把原图透传）");

    // 落盘缓存 + 最长边压到 512
    let thumb_path = media_dir.parent().unwrap().join("thumbs").join("100.jpg");
    assert!(thumb_path.is_file(), "缩略图要落盘（否则每次都重算）");
    let out = image::open(&thumb_path).unwrap();
    assert_eq!((out.width(), out.height()), (512, 256));

    // 非图片 → 204（前端据此不放缩略图）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name, mime_type)
                 VALUES (101, 'VIDEO', 'v.mp4', 'v.mp4', 'video/mp4')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    std::fs::write(media_dir.join("v.mp4"), b"not really a video").unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/101/thumb").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 坏图（后缀是 png、内容不是）→ **回退原图**（200 + 原 MIME + 原字节），不是 500
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name, mime_type)
                 VALUES (102, 'IMAGE', 'broken.png', 'broken.png', 'image/png')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    std::fs::write(media_dir.join("broken.png"), b"definitely not a png").unwrap();
    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/media/102/thumb").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
    assert!(response.headers().get("x-thumbnail").is_none());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"definitely not a png");

    // 文件丢了 → 404（不是 500，也不是空白图）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = 'gone.png' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    std::fs::remove_file(&thumb_path).unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/thumb").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 产物关联的完整提示词也搬过来了（204 = 没关联）
    let (status, _) = get_json(router_for(state.clone()), "/api/media/101/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "视频产物没关联提示词 → 204");

    // 视频封面（P4 第 51 步）：101 是视频、文件也在，但这个夹具的项目根里**没有**
    // `scripts\video-poster.ps1` → 抽不出帧 → **204**（前端退化成"播放器转圈"，不是破图）。
    // "文件丢了 → 404" 那条分支由 `tests/poster_test.rs` 覆盖。
    let (status, _) = get_json(router_for(state.clone()), "/api/media/101/poster").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "抽不出帧要回 204，不许假装有封面");

    // 上传已经搬过来了：空 multipart 是 400（不再是 501）
    let empty = multipart_body("----comfyhubtest", &[], &[]);
    let (status, _) = post_multipart(router_for(state.clone()), "/api/media/upload", empty).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
// ===========================================================================
//  产物写侧 + /api/stats（P4 第 4 步）
// ===========================================================================

#[tokio::test]
async fn patch_media_updates_fields_and_rejects_empty_body() {
    let state = state();
    // 一个字段都不给 → 404（Kotlin 的 `update` 返回 false，路由翻成"不存在或无字段需要更新"）
    let empty = send_json(router_for(state.clone()), "PATCH", "/api/media/100", serde_json::json!({})).await;
    assert_eq!(empty.0, StatusCode::NOT_FOUND, "空 body 不许静默成功");

    let patched = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"title": "改过的标题", "favorite": true, "nsfw": true, "notes": "备注"}),
    )
    .await;
    assert_eq!(patched.0, StatusCode::OK, "body={}", patched.1);
    assert_eq!(patched.1["title"], "改过的标题");
    assert_eq!(patched.1["favorite"], true);
    assert_eq!(patched.1["nsfw"], true, "手动标记 NSFW（只影响预览打码）");
    assert_eq!(patched.1["notes"], "备注");
    assert_eq!(patched.1["promptId"], 10, "没提这一列就不该动它");

    // clearPrompt：明确解除关联（与 promptId=null 不是一回事）
    let cleared = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"clearPrompt": true}),
    )
    .await;
    assert_eq!(cleared.0, StatusCode::OK);
    assert!(
        !cleared.1.as_object().unwrap().contains_key("promptId"),
        "clearPrompt 必须真的置空；Kotlin 那边 null 就是**这个键不出现**：{}",
        cleared.1
    );
    assert_eq!(cleared.1["promptTags"], serde_json::json!([]), "解绑后标签也不该再带上提示词的");

    // 指向不存在的提示词 → 400（不是静默忽略）
    let bogus = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"promptId": 9999}),
    )
    .await;
    assert_eq!(bogus.0, StatusCode::BAD_REQUEST);
    assert!(bogus.1["error"].as_str().unwrap().contains("9999"));

    let missing = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/9999",
        serde_json::json!({"title": "x"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

/// 删产物：库、原件、缩略图一起清；标签计数跟着重算。
#[tokio::test]
async fn delete_media_removes_row_file_and_thumb() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();
    let thumb_dir = media_dir.parent().unwrap().join("thumbs");
    std::fs::create_dir_all(&thumb_dir).unwrap();
    std::fs::write(thumb_dir.join("100.jpg"), b"thumb").unwrap();
    std::fs::write(thumb_dir.join("100.poster.png"), b"poster").unwrap();
    assert!(media_dir.join("a.png").exists());

    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/media/100").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body, serde_json::json!({"deleted": 100}));

    let rows: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM media_assets WHERE id = 100", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(rows, 0);
    assert!(!media_dir.join("a.png").exists(), "原件必须一起删（只删库会留下磁盘孤儿）");
    assert!(!thumb_dir.join("100.jpg").exists());
    assert!(!thumb_dir.join("100.poster.png").exists());

    let again = send_empty(router_for(state.clone()), "DELETE", "/api/media/100").await;
    assert_eq!(again.0, StatusCode::NOT_FOUND, "删第二次要 404");
}

#[tokio::test]
async fn stats_counts_match_the_fixture() {
    let (status, body) = get_json(router_for(state()), "/api/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec!["byKind", "byMediaKind", "favoriteMedia", "favoritePrompts", "media", "prompts", "tags"]
    );
    assert_eq!(body["prompts"], 2);
    assert_eq!(body["media"], 1);
    assert_eq!(body["tags"], 2);
    assert_eq!(body["favoritePrompts"], 1, "夹具里 id=10 是收藏的");
    assert_eq!(body["favoriteMedia"], 0);
    assert_eq!(body["byKind"], serde_json::json!({"IMAGE": 2}));
    assert_eq!(body["byMediaKind"], serde_json::json!({"IMAGE": 1}));
}

#[tokio::test]
async fn upload_stores_files_with_tags_and_dedupes_by_sha256() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();

    // 一张真 PNG + 一个重复（同样内容、不同文件名）+ 一段"音频"
    let mut img = image::RgbImage::new(64, 32);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([1, 2, 3]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png_bytes = png.into_inner();

    let boundary = "----comfyhubtest";
    let body = multipart_body(
        boundary,
        &[("promptId", "10"), ("tags", "上传标签, 第二个"), ("title", "上传的图")],
        &[
            ("files", "shot.png", &png_bytes),
            ("files", "copy.png", &png_bytes),
            ("files", "voice.mp3", b"ID3fakeaudio"),
        ],
    );
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", body).await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_eq!(keys(&body), vec!["duplicates", "failed", "items", "promptId"]);
    assert_eq!(body["promptId"], 10);
    assert_eq!(body["failed"], serde_json::json!([]));

    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "两个**不同内容**的文件入库；重复那份只记 duplicates");
    let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"IMAGE") && kinds.contains(&"AUDIO"), "kind={kinds:?}");
    let image_item = items.iter().find(|i| i["kind"] == "IMAGE").unwrap();
    assert_eq!(image_item["title"], "上传的图", "给了 title 就用它");
    assert_eq!(image_item["width"], 64, "图片要探出尺寸");
    assert_eq!(image_item["height"], 32);
    assert_eq!(image_item["promptId"], 10);
    // promptTags = 产物自身标签 ∪ 关联提示词(10)的标签「赛博朋克」，合并排序去重
    assert_eq!(image_item["promptTags"], serde_json::json!(["上传标签", "第二个", "赛博朋克"]));
    assert!(image_item["sha256"].as_str().unwrap().len() == 64, "sha256 要落库（判重键）");

    assert_eq!(body["duplicates"].as_array().unwrap().len(), 1);
    assert_eq!(body["duplicates"][0]["fileName"], "copy.png");

    // 缩略图跟着生成（图片入库时就写）
    let thumb = media_dir.parent().unwrap().join("thumbs").join(format!("{}.jpg", image_item["id"]));
    assert!(thumb.is_file(), "图片入库要顺手写缩略图：{}", thumb.display());

    // 落盘名是 32 位 hex + 扩展名（与 Kotlin 同一套命名）
    let stored = image_item["storedName"].as_str().unwrap();
    assert!(stored.ends_with(".png") && stored.len() == 36, "storedName={stored}");

    // 判重是**内容**判重：换个名字再传一次仍然是 duplicate
    let again = multipart_body(boundary, &[], &[("files", "another.png", &png_bytes)]);
    let (status, body2) = post_multipart(router_for(state.clone()), "/api/media/upload", again).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body2["items"].as_array().unwrap().is_empty());
    assert_eq!(body2["duplicates"].as_array().unwrap().len(), 1);
    // 这次没带 promptId → Kotlin（`promptId: Long? = null` + `explicitNulls = false`）那边
    // **整个键都不出现**（上面那次带了 promptId=10，所以那次有）
    assert_eq!(keys(&body2), vec!["duplicates", "failed", "items"], "body={body2}");
}

#[tokio::test]
async fn upload_rejects_empty_and_bad_prompt_id() {
    let state = state();
    // 一个文件都没有 → 400（不静默成功）
    // ⚠ boundary 必须与 `post_multipart` 里的那一个一致（不一致的话解析就失败了，
    // 测到的不是"没有文件"那条分支）
    let empty = multipart_body("----comfyhubtest", &[("title", "只有字段")], &[]);
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", empty).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("files"));

    // promptId 不存在 → 400
    let body = multipart_body("----comfyhubtest", &[("promptId", "9999")], &[("files", "a.png", b"x")]);
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("9999"));

    // 校验失败时**临时文件不许留在盘上**（tmp 目录要干净）
    let tmp = state.media_dir().parent().unwrap().join("tmp");
    let leftovers = std::fs::read_dir(&tmp).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "校验失败要清掉临时文件");
}
#[tokio::test]
async fn media_prompt_has_three_outcomes() {
    let state = state();
    // 有关联 → 200 + 完整提示词（含标签与产物计数）
    let (status, body) = get_json(router_for(state.clone()), "/api/media/100/prompt").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["id"], 10);
    assert_eq!(body["title"], "赛博朋克少女");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");

    // 没关联 → 204（Dart 的 `mediaPrompt` 把 204 当 null，界面退化成"没有提示词"）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name) VALUES (103, 'IMAGE', 'z.png', 'z.png')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/103/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 正常路径下**根本不会出现悬挂引用**：R1 生效时 `prompt_id` 是 ON DELETE SET NULL，
    // 删提示词只会把它解绑 → 下一条查询变成 204（产物照样留着，这正是"删提示词保产物"）
    state
        .with(|conn| conn.execute("DELETE FROM prompts WHERE id = 10", []).map(|_| ()))
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "删提示词后产物被解绑（不是悬挂）");

    // 悬挂引用只在"外键没生效时写进去的库"里才会出现 —— 把 FK 关掉造一个，验证 404 分支
    // （Kotlin 那条路由这里回的是 200 + {"error":…}，前端会把它解析成一条空提示词；
    //  我们回 404，Dart 的 mediaPrompt 对非 2xx 是抛异常 —— 这是有意的行为差异）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, prompt_id, kind, original_name, stored_name)
                 VALUES (104, 11, 'IMAGE', 'd.png', 'd.png')",
                [],
            )?;
            conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
            conn.execute("DELETE FROM prompts WHERE id = 11", [])?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            Ok(())
        })
        .unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/media/104/prompt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("已被删除"));

    // 产物本身不存在 → 404
    let (status, _) = get_json(router_for(state.clone()), "/api/media/9999/prompt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
