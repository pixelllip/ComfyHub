//! 业务读写的形状与纪律：健康检查、tags / prompts 的读侧、错误体、外键、未迁移端点。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

#[tokio::test]
async fn confinement_endpoint_reports_the_judge_not_just_the_verdict() {
    let (status, body) = get_json(app(), "/api/system/confinement").await;
    assert_eq!(status, StatusCode::OK);
    // ⚠ **只断言"形状"，不断言 restricted 的取值**：真机上那份 ComfyUI 目录到底写不写得进
    //   是环境属性（AGENTS §12 纪律）。要断言的是"判据必须一起给" ——
    //   只返回一个布尔值，界面与模型只能推测（该问题由此变成"改 ACL"）。
    for key in [
        "restricted",
        "serverImage",
        "imageInsideProjectRoot",
        "blockedPaths",
        "checkedPaths",
        "summary",
    ] {
        assert!(body.get(key).is_some(), "缺了 `{key}`：{body}");
    }
    assert!(body["restricted"].is_boolean());
    assert!(
        body["imageInsideProjectRoot"].is_boolean(),
        "判据（映像在不在自己的运行根目录里）必须给：{body}"
    );
    assert!(body["blockedPaths"].is_array());
    let restricted = body["restricted"].as_bool().unwrap();
    assert_eq!(
        body["remedy"].is_null(),
        !restricted,
        "修法只在真受限时给（不受限时那行是噪音）"
    );
}

#[tokio::test]
async fn outbound_endpoint_reports_the_route_and_its_source() {
    let (status, body) = get_json(app(), "/api/system/outbound").await;
    assert_eq!(status, StatusCode::OK);
    // 界面要的四个键（`lib/models/models.dart` 的 OutboundInfo 直接读它们）
    for key in ["route", "proxy", "source", "system"] {
        assert!(body.get(key).is_some(), "缺了 `{key}`：{body}");
    }
    assert!(
        matches!(body["route"].as_str(), Some("proxy" | "direct")),
        "route 只能是 proxy / direct：{body}"
    );
    assert!(
        body["proxy"].is_null() || body["proxy"].is_string(),
        "proxy 要么是 host:port，要么是 null：{body}"
    );
    // ⚠ **只断言来源非空，不断言具体值**：这台机器的系统代理（或环境变量）是环境属性；
    //   需固定的是"来源必须一并返回" —— 只返回一个地址，无法判断是否为自行填写的那一个。
    assert!(
        !body["source"].as_str().unwrap_or("").is_empty(),
        "来源必须给：{body}"
    );
    // 系统代理相关项也原样返回（便于与 Windows 的"代理设置"核对）
    for key in ["enabled", "server", "overrides", "autoConfigUrl"] {
        assert!(
            body["system"].get(key).is_some(),
            "system 缺了 `{key}`：{body}"
        );
    }
    assert!(body["system"]["enabled"].is_boolean());
    assert!(body["system"]["overrides"].is_array());
    // 默认**不发探针**：打开设置页不能往公网送流量（只有 `?probe=1` 才发）
    assert!(
        body.get("probes").is_none(),
        "没要求探针就不该有 probes：{body}"
    );
}

#[tokio::test]
async fn health_reports_engine_and_schema_version() {
    let (status, body) = get_json(app(), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["impl"], "rust");
    assert_eq!(
        body["schemaVersion"].as_i64(),
        Some(comfyhub_server_rs::infra::db::SCHEMA_VERSION),
        "就是 PRAGMA user_version（真源是 db.rs 的 SCHEMA_VERSION，别写死数字）"
    );
    assert_eq!(body["engine"]["fts5"], true, "R14：bundled 引擎必须带 FTS5");
    assert!(
        body["engine"]["sqliteVersion"]
            .as_str()
            .unwrap()
            .starts_with("3."),
        "engine={}",
        body["engine"]
    );
    // ★ 这四个键是 **Dart 在读的**（Kotlin `HealthDto` 同形）：
    //   `backend_launcher.dart:271` 用 `database == "ok"` 判断"后端已就绪"，
    //   缺了它启动流程会以为库坏了；设置页显示 `version` / `storageDir`。
    for key in ["version", "database", "storageDir", "serverTime"] {
        assert!(
            body.get(key).is_some(),
            "缺了 Dart 会读的键 `{key}`：{body}"
        );
    }
    assert_eq!(body["database"], "ok", "库里真能跑通 SELECT 1 才叫 ok");
    assert_eq!(body["version"], "1.0.0", "Kotlin 的 APP_VERSION");
    // `storageDir` 是 `<storage>`（media 的父目录），不是 media 目录本身
    let storage = body["storageDir"].as_str().unwrap();
    assert!(
        !storage.replace('\\', "/").ends_with("/media"),
        "storageDir 不该指向 media：{storage}"
    );
    // 夹具的 storage 必须落在**测试暂存根**里（`target\.scratch`，见 `testpaths`）——
    // 那既是唯一约定的落盘位置，也顺带钉住"受限会话里用例照样能跑"。
    assert!(
        std::path::Path::new(storage).starts_with(comfyhub_server_rs::infra::testpaths::root()),
        "夹具的 storage 要在测试暂存根下：{storage}"
    );
    assert!(body["serverTime"].as_str().unwrap().contains('T'), "{body}");
}

#[tokio::test]
async fn tags_use_the_same_keys_as_the_kotlin_server() {
    let (status, body) = get_json(app(), "/api/tags").await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 2);
    // popular：使用次数降序
    assert_eq!(items[0]["name"], "国风");
    // 键集合**逐条照 Kotlin 算**（`explicitNulls = false`，真库副本上实测过）：
    //  · `normalized` **不存在** —— Kotlin 的 `TagDto`（`Models.kt:11`）里根本没这个字段；
    //  · `color` / `description` 在库里是 NULL → **键不出现**（不是 `"color": null`）。
    assert_eq!(
        keys(&items[0]),
        vec!["category", "createdAt", "id", "name", "useCount"],
        "国风那条 color/description 是 NULL，键就该不出现（Kotlin 实测形状）"
    );
    assert_eq!(
        keys(&items[1]),
        vec!["category", "color", "createdAt", "id", "name", "useCount"],
        "赛博朋克那条有 color，于是正好多这一个键"
    );
    assert!(
        !items[0].as_object().unwrap().contains_key("color"),
        "NULL 不等于 null"
    );
    assert_eq!(items[0]["useCount"], 9);
}

#[tokio::test]
async fn tag_filters_and_categories() {
    let (status, body) = get_json(app(), "/api/tags?q=%E8%B5%9B%E5%8D%9A").await; // q=赛博
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], 1);

    let (status, body) = get_json(app(), "/api/tags?category=%E9%A3%8E%E6%A0%BC").await; // category=风格
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 2);

    let (status, body) = get_json(app(), "/api/tags?sort=name").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["name"], "国风", "name 排序：按名字");

    let (status, body) = get_json(app(), "/api/tags/categories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!(["风格"]));
}

#[tokio::test]
async fn prompts_paged_envelope_matches_dart_paged() {
    let (status, body) = get_json(app(), "/api/prompts?page=1&size=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["items", "page", "pages", "size", "total"]);
    assert_eq!(body["total"], 2);
    assert_eq!(body["page"], 1);
    assert_eq!(body["size"], 1);
    assert_eq!(body["pages"], 2);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    // newest：created_at 降序
    assert_eq!(body["items"][0]["title"], "国风山水");
}

/// ★ **预设库只放手动保存的**。
///
/// 原话："从 comfy 输出目录获取的产物，其关联的提示词不需要送进预设库 —— 毕竟都叫这个名字了"。
/// 自动捕获那些仍然是提示词行（画廊详情要按 `promptId` 反查它），只是不进这个列表。
///
/// ⚠ **AI 侧必须传 `includeCaptured=true`**：`search_prompts` / `comfy_find_workflow` 找的是
/// "库里能跑的工作流"，自动捕获那些正是最好用的来源 —— 默认过滤会把它们藏起来，
/// 表现为"AI 报告库中不存在，而画廊中可见"（且不会有用例失败）。
#[tokio::test]
async fn preset_list_hides_captured_prompts_but_ai_can_still_search_them() {
    let state = state();
    // 造两条自动捕获来的提示词（来源是 ComfyUI 系）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO prompts (id, title, kind, positive_prompt, source, created_at, updated_at)
                 VALUES (30, '自动捕获 A', 'IMAGE', 'captured one', 'ComfyUI', '2026-09-23T03:00:00.000Z', '2026-09-23T03:00:00.000Z'),
                        (31, '导入的文件 B', 'IMAGE', 'imported one', 'ComfyUI-Import', '2026-09-23T04:00:00.000Z', '2026-09-23T04:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let app = router_for(state.clone());

    // 默认（界面）：只剩两条 Manual 的
    let (_, body) = get_json(app.clone(), "/api/prompts?size=50").await;
    assert_eq!(body["total"], 2, "自动捕获那两条不许进预设库：{body}");
    let titles: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert!(!titles.contains(&"自动捕获 A"), "{titles:?}");
    assert!(!titles.contains(&"导入的文件 B"), "{titles:?}");

    // AI 侧（`includeCaptured=true`）：四条都在
    let (_, all) = get_json(app.clone(), "/api/prompts?size=50&includeCaptured=true").await;
    assert_eq!(all["total"], 4, "AI 侧要能搜到自动捕获的工作流：{all}");

    // 显式关掉也还是两条（参数语义与"默认"一致）
    let (_, explicit) = get_json(app.clone(), "/api/prompts?size=50&includeCaptured=false").await;
    assert_eq!(explicit["total"], 2);
}

/// ★ 预设的**关联产物**按归一化后的提示词匹配（2026-10-11）。
///
/// 目标是"使用这套预设同款提示词的产物自动关联" —— 因此判据**不能**是
/// `media_assets.prompt_id == 预设 id`（那只有"这条预设自己生成过"才算，
/// 而自动捕获的产物指向的是**另一条**提示词行）。
#[tokio::test]
async fn preset_media_matches_by_normalized_prompt() {
    let state = state();
    // 夹具里 prompt 10 是 `1girl, cyberpunk` + 负面 `lowres`；
    // 再建一条**同款但写法不同**的自动捕获行 + 一张产物。
    // ⚠ 负面也要同款 —— 判据是"归一化后的**正负**提示词都相同"，
    // 少给一边就会（正确地）判成不同款，用例会红在一个看不出原因的地方。
    let media_id = state
        .with(|conn| {
            conn.execute(
                "INSERT INTO prompts (id, title, kind, positive_prompt, negative_prompt, source, created_at, updated_at)
                 VALUES (40, '自动捕获的同款', 'IMAGE', '(1girl:1.2), cyberpunk', '(lowres:1.1)', 'ComfyUI', '2026-09-23T05:00:00.000Z', '2026-09-23T05:00:00.000Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO media_assets (id, prompt_id, kind, title, original_name, stored_name, created_at, updated_at)
                 VALUES (900, 40, 'IMAGE', '同款产物', 'same.png', 'same.png', '2026-09-23T05:01:00.000Z', '2026-09-23T05:01:00.000Z')",
                [],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap();
    let _ = media_id;
    let app = router_for(state.clone());

    // 先确认现场真的造出来了（否则下面那条断言会指向错误的地方）
    let count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM media_assets WHERE id = 900",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1, "前提：同款产物 900 真的入库了");
    let norm = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM media_assets m JOIN prompts p ON p.id = m.prompt_id
                      WHERE m.kind = 'IMAGE'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert!(
        norm >= 2,
        "前提：至少两条 IMAGE 产物进了候选（实际 {norm}）"
    );

    let (status, body) = get_json(app.clone(), "/api/presets/10/media").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_i64().unwrap())
        .collect();
    assert!(
        ids.contains(&900),
        "写法不同（`(1girl:1.2)` ↔ `1girl`）也要认成同款并关联：{ids:?}"
    );

    // 不同款的一条都不许混进来
    let (_, other) = get_json(app.clone(), "/api/presets/11/media").await;
    assert!(
        !other.as_array().unwrap().iter().any(|m| m["id"] == 900),
        "国风山水那条预设不该关联到赛博朋克少女的产物"
    );

    // 预设不存在 → 空列表（与 `/api/prompts/{id}/media` 同一条纪律：不做存在性检查）
    let (status, empty) = get_json(app.clone(), "/api/presets/999/media").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, serde_json::json!([]));
}

#[tokio::test]
async fn prompt_object_has_every_key_dart_reads() {
    let (status, body) = get_json(app(), "/api/prompts/10").await;
    assert_eq!(status, StatusCode::OK);
    // 这一条（prompt 10）**填满了**大部分字段，所以键几乎都在；NULL 的那些键不出现（Kotlin 形状）
    assert_eq!(
        keys(&body),
        vec![
            "cfgScale",
            "checkpoint",
            "createdAt",
            "extraParams",
            "favorite",
            "hasWorkflow",
            "id",
            "kind",
            "loras",
            "mediaCount",
            "negativeMode",
            "negativePrompt",
            "positivePrompt",
            "seed",
            "source",
            "tags",
            "title",
            "updatedAt",
        ],
        "少一个键 = 界面上少一块内容；多一个键 = 与 Kotlin 不一致"
    );
    // `negativeMode` 是**必给**的（不是 Option）：界面拿它决定"只发正面"那个开关的位置。
    // 老库补上该列后默认为 keep —— 绝不能默认为 positive_only（否则会静默丢弃负面提示词）。
    assert_eq!(body["negativeMode"], "keep", "没设过就是 keep");
    for absent in [
        "sampler",
        "scheduler",
        "steps",
        "width",
        "height",
        "batchSize",
        "notes",
        "sourceRef",
    ] {
        assert!(
            !body.as_object().unwrap().contains_key(absent),
            "`{absent}` 在库里是 NULL，Kotlin 那边**整个键都不发**（不是 null）：{body}"
        );
    }
    assert_eq!(body["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(body["cfgScale"], 3.5);
    assert_eq!(body["favorite"], true, "favorite 是布尔（库里是 0/1）");
    assert_eq!(
        body["hasWorkflow"], true,
        "接口只给'有没有'，不给整份 workflow_json"
    );
    assert_eq!(body["loras"][0]["name"], "anima");
    assert_eq!(body["loras"][0]["weight"], 0.8);
    assert_eq!(body["extraParams"], serde_json::json!({}), "NULL 当空对象");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");
    assert_eq!(body["mediaCount"], 1);

    // prompt 11 几乎全空：键集合缩到最小 —— 这条专门盯"NULL 字段不许冒出来"
    let (status, bare) = get_json(app(), "/api/prompts/11").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&bare),
        vec![
            "createdAt",
            "extraParams",
            "favorite",
            "hasWorkflow",
            "id",
            "kind",
            "loras",
            "mediaCount",
            "negativeMode",
            "positivePrompt",
            "source",
            "sourceRef",
            "tags",
            "title",
            "updatedAt",
        ],
        "全空的那条只剩这些键：{bare}"
    );
}

/// 关键字语义照抄 Kotlin `PromptRepo.buildWhere`：**按空白拆词、每个词都要命中（AND）**，
/// 且匹配 **5 列**（title / positive / negative / notes / checkpoint）；
/// `kind` 先大写、`ALL` 等于不过滤。
///
/// 为什么值得单列一条：`search_prompts`（工具）与 `/api/prompts`（界面）**共用这份实现**，
/// 而"界面搜得到、AI 说没有"不会有任何用例变红。
#[tokio::test]
async fn prompt_search_splits_terms_and_covers_five_columns() {
    let state = state();
    // 补一条带备注的：证明 `notes` 也在匹配列里（夹具里另外两条都没有 notes）
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO prompts (id, title, kind, positive_prompt, notes, favorite, created_at, updated_at) \
                 VALUES (12, '备注专用', 'IMAGE', 'a quiet street', '参考 shinku 的构图', 0, \
                         '2026-09-26T00:00:00.000Z', '2026-09-26T00:00:00.000Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    // ① 两个词分别命中**不同列**（标题里的「少女」+ 正向里的「cyberpunk」）→ 都要命中
    let (_, body) = get_json(
        router_for(state.clone()),
        "/api/prompts?q=cyberpunk%20%E5%B0%91%E5%A5%B3",
    )
    .await;
    assert_eq!(body["total"], 1, "两个词是 AND，跨列也要命中：{body}");
    assert_eq!(body["items"][0]["id"], 10);
    // 一个词命中不了 → 整条不命中（整串 LIKE 的实现会把它当成"cyberpunk 少女"一个子串）
    let (_, body) = get_json(
        router_for(state.clone()),
        "/api/prompts?q=cyberpunk%20%E4%B8%8D%E5%AD%98%E5%9C%A8",
    )
    .await;
    assert_eq!(body["total"], 0, "{body}");

    // ② 五列都要能搜到
    for (query, expected) in [
        ("lowres", 10_i64),         // negative_prompt
        ("krea2.safetensors", 10),  // checkpoint
        ("%E5%8F%82%E8%80%83", 12), // notes（「参考」）
        ("quiet", 12),              // positive_prompt
        ("%E5%A4%87%E6%B3%A8", 12), // title（「备注」）
    ] {
        let (_, body) = get_json(
            router_for(state.clone()),
            &format!("/api/prompts?q={query}"),
        )
        .await;
        assert_eq!(body["total"], 1, "q={query} 应该命中：{body}");
        assert_eq!(
            body["items"][0]["id"], expected,
            "q={query} 命中了错的那条：{body}"
        );
    }

    // ③ kind：大小写不敏感 + `ALL` 等于不过滤
    let (_, body) = get_json(router_for(state.clone()), "/api/prompts?kind=image").await;
    assert_eq!(
        body["total"], 3,
        "库里存的是大写 IMAGE，小写也该命中：{body}"
    );
    let (_, body) = get_json(router_for(state.clone()), "/api/prompts?kind=ALL").await;
    assert_eq!(body["total"], 3, "ALL = 不过滤：{body}");
    // `favorite=false` 按 Kotlin 的口径**不过滤**（只有 true 才筛）
    let (_, body) = get_json(router_for(state.clone()), "/api/prompts?favorite=false").await;
    assert_eq!(body["total"], 3, "{body}");

    // 同一套语义在工具里也要成立（两边共用一份实现）
    let out = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "query": "cyberpunk 少女" }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["count"], 1, "{}", out.content);
    assert_eq!(out.payload["items"][0]["id"], 10);

    std::fs::remove_dir_all(state.project_root()).ok();
}

#[tokio::test]
async fn prompt_search_and_favorite_and_missing() {
    let (status, body) = get_json(app(), "/api/prompts?q=cyberpunk").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], 10);

    let (status, body) = get_json(app(), "/api/prompts?favorite=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], 10);

    let (status, body) = get_json(app(), "/api/prompts?kind=VIDEO").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    assert_eq!(body["items"], serde_json::json!([]));

    let (status, body) = get_json(app(), "/api/prompts/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 错误体是 `{error, detail?}`：普通错误**人话就写在 `error` 里**（`detail` 不出现）
    assert_eq!(keys(&body), vec!["error"]);
    assert!(
        body["error"].as_str().unwrap().contains("999"),
        "body={body}"
    );
}

#[tokio::test]
async fn foreign_keys_cascade_is_really_on() {
    // R1：删标签要清掉关联行（`ON DELETE CASCADE`）—— 这条只有在 foreign_keys=ON 时才成立
    let conn = test_conn();
    conn.execute("DELETE FROM tags WHERE id = 1", []).unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM prompt_tags WHERE tag_id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "R1 没生效：删标签留下了孤儿关联行");
}

/// **普通错误**：`error` 就是人话，`detail` 不出现，`message` 这个键根本不存在。
///
/// 为什么这条必须有用例：Dart 的 `ApiClient._decode`（`lib/core/api_client.dart:50`）
/// 把 `error` 当人话读；`AiApiClient._decode` 先读 `detail`、再读 `error`。
/// 早先这里是 `{error:"NOT_FOUND", message:"提示词 999 不存在"}` ——
/// 两个客户端都读不到人话，界面显示的是 `NOT_FOUND`。**形状错不报错，只显示机器码。**
#[tokio::test]
async fn ordinary_errors_put_the_human_text_in_error() {
    for (uri, expected) in [
        ("/api/prompts/999", StatusCode::NOT_FOUND),
        ("/api/prompts/abc", StatusCode::BAD_REQUEST),
        ("/api/media/9999", StatusCode::NOT_FOUND),
        ("/api/nope", StatusCode::NOT_FOUND), // 未知路径也要同一个信封（默认 fallback 是空 body）
    ] {
        let (status, body) = get_json(app(), uri).await;
        assert_eq!(status, expected, "uri={uri} body={body}");
        assert_eq!(
            keys(&body),
            vec!["error"],
            "普通错误只有 `error` 一个键：{body}"
        );
        let text = body["error"].as_str().unwrap();
        assert!(
            !looks_like_a_machine_code(text),
            "`error` 必须是给人看的话：{text}"
        );
        assert!(
            !body.to_string().contains("message"),
            "`message` 这个键不许再出现：{body}"
        );
    }
}

/// **AI 领域错误**（`AiException` 的对应物，AIH-024）：`error` = 稳定码，`detail` = 人话。
///
/// 形状基线是实测出来的（真库副本 + Kotlin 8090）：
/// `GET /api/ai/conversations/999999` → `400 {"error":"CONFIG_ERROR","detail":"会话不存在: 999999"}`。
#[tokio::test]
async fn ai_errors_put_the_stable_code_in_error_and_the_human_text_in_detail() {
    use axum::response::IntoResponse;

    let body_of = |error: comfyhub_server_rs::web::routes::ApiError| async move {
        let response = error.into_response();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&bytes).unwrap())
    };

    let (status, body) = body_of(comfyhub_server_rs::web::routes::ApiError::ai(
        "CONFIG_ERROR",
        "会话不存在: 999999",
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "AI 异常照 Kotlin 固定 400");
    assert_eq!(keys(&body), vec!["detail", "error"]);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "会话不存在: 999999");

    // 没有人类可读说明时 `detail` **整键省略**（Kotlin 那边 `cause.message == null` 就是这个形状）
    let (_, body) = body_of(comfyhub_server_rs::web::routes::ApiError::ai(
        "CONFIG_ERROR",
        "   ",
    ))
    .await;
    assert_eq!(
        keys(&body),
        vec!["error"],
        "`detail` 必须是**不出现**，不是 null：{body}"
    );
}

// ===========================================================================
//  写端点（P4 第 2 步）
// ===========================================================================

/// 建标签 → 201 + TagDto；同名（忽略大小写）**不重复建**，返回同一个 id。
#[tokio::test]
async fn post_tags_is_idempotent_by_normalized_name() {
    let state = state();
    let first = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "赛博朋克", "category": "风格", "color": "#ff00ff"}),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED, "body={}", first.1);
    assert_eq!(first.1["name"], "赛博朋克");
    let id = first.1["id"].as_i64().unwrap();

    // 第二次用不同大小写/空格：normalized 相同 → 还是那一条
    let again = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": " 赛博朋克 "}),
    )
    .await;
    assert_eq!(again.0, StatusCode::CREATED);
    assert_eq!(
        again.1["id"].as_i64().unwrap(),
        id,
        "同名标签不许建出第二条"
    );

    let count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tags WHERE normalized = '赛博朋克'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn post_tags_with_blank_name_is_400() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "   "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("不能为空"),
        "body={body}"
    );
}

/// 改名撞上另一个标签 → 400（**不是**静默合并成一条）。
#[tokio::test]
async fn put_tag_conflict_is_400_and_missing_is_404() {
    let state = state();
    let a = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "国风"}),
    )
    .await;
    let b = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "赛博"}),
    )
    .await;
    let (id_a, id_b) = (a.1["id"].as_i64().unwrap(), b.1["id"].as_i64().unwrap());

    let clash = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/tags/{id_b}"),
        serde_json::json!({"name": "国风", "description": "画风"}),
    )
    .await;
    assert_eq!(clash.0, StatusCode::BAD_REQUEST);
    assert!(
        clash.1["error"].as_str().unwrap().contains("已存在"),
        "body={}",
        clash.1
    );

    let renamed = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/tags/{id_a}"),
        serde_json::json!({"name": "国风画", "category": "风格"}),
    )
    .await;
    assert_eq!(renamed.0, StatusCode::OK);
    assert_eq!(renamed.1["name"], "国风画");

    let missing = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/tags/9999",
        serde_json::json!({"name": "随便"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_tag_returns_deleted_and_cascades() {
    let state = state();
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/tags/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"deleted": 1}));

    // R1：关联行被级联删掉
    let left: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM prompt_tags WHERE tag_id = 1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(left, 0);

    let again = send_empty(router_for(state.clone()), "DELETE", "/api/tags/1").await;
    assert_eq!(
        again.0,
        StatusCode::NOT_FOUND,
        "删第二次要 404，不许假装成功"
    );
}

/// 建提示词：自动起名 + 标签自动创建 + `use_count` 重算 + **同步账本被点亮**。
#[tokio::test]
async fn post_prompts_creates_with_tags_and_writes_sync_log() {
    let state = state();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts",
        serde_json::json!({
            "positivePrompt": "1girl, cyberpunk city",
            "titles": "前端多带一个无用键不该炸",
            "cfgScale": 4.5,
            "tags": ["新标签A", "新标签B", "新标签A"],
            "extraParams": {"steps": "30"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_eq!(body["title"], "1girl", "标题为空时按正文第一段自动起名");
    assert_eq!(body["cfgScale"], 4.5);
    assert_eq!(body["extraParams"], serde_json::json!({"steps": "30"}));
    let new_id = body["id"].as_i64().unwrap();

    // 标签自动建出来并去重（"新标签A" 说两遍只关联一次）
    let tag_names: Vec<String> = body["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tag_names, vec!["新标签A", "新标签B"]);
    let use_count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT use_count FROM tags WHERE name = '新标签A'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(use_count, 1, "use_count 必须重算");

    // 同步账本：这条写操作要能被手机看到（触发器自动写的，不需要额外代码）
    let logged: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'prompts' AND pk = ?1 AND op = 'upsert'",
                rusqlite::params![new_id.to_string()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(logged, 1, "写提示词必须点亮 _sync_log（同步契约 §2）");
}

#[tokio::test]
async fn post_prompts_blank_is_400() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/prompts",
        serde_json::json!({"title": "  ", "positivePrompt": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("不能为空"),
        "body={body}"
    );
}

#[tokio::test]
async fn put_prompt_replaces_tags_and_missing_is_404() {
    let state = state();
    let updated = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/10",
        serde_json::json!({
            "title": "改过的标题",
            "kind": "VIDEO",
            "positivePrompt": "1girl, cyberpunk",
            "tags": ["只留这一个"]
        }),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "body={}", updated.1);
    assert_eq!(updated.1["title"], "改过的标题");
    assert_eq!(updated.1["kind"], "VIDEO");
    let tags: Vec<String> = updated.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tags, vec!["只留这一个"], "PUT 是整体替换标签");

    let missing = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/9999",
        serde_json::json!({"positivePrompt": "x"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    // 非法 kind 回落到 IMAGE（与 Kotlin 一致），而不是 400
    let fallback = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/11",
        serde_json::json!({"positivePrompt": "x", "kind": "NOPE"}),
    )
    .await;
    assert_eq!(fallback.1["kind"], "IMAGE");
}

/// 收藏回执是 `{id, favorite}` 两个键（Kotlin 侧出现过"混类型 map → 500"的问题）。
#[tokio::test]
async fn favorite_returns_id_and_flag() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/prompts/11/favorite",
        serde_json::json!({"favorite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["favorite", "id"]);
    assert_eq!(body["id"], 11);
    assert_eq!(body["favorite"], true);

    let missing = send_json(
        router_for(state()),
        "POST",
        "/api/prompts/9999/favorite",
        serde_json::json!({"favorite": true}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_creates_a_copy_and_keeps_tags() {
    let state = state();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/10/duplicate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_ne!(body["id"].as_i64().unwrap(), 10);
    assert_eq!(body["title"], "赛博朋克少女 副本");
    assert_eq!(
        body["favorite"], false,
        "副本不继承收藏状态（与 Kotlin 一致）"
    );
    assert_eq!(body["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(body["loras"][0]["name"], "anima");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");

    let missing = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/9999/duplicate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

/// 追加 / 移除标签（两个端点都回整条 PromptDto，界面据此刷新）。
#[tokio::test]
async fn add_and_remove_prompt_tags() {
    let state = state();
    let added = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/11/tags",
        serde_json::json!({"tags": ["追加的标签"]}),
    )
    .await;
    assert_eq!(added.0, StatusCode::OK);
    let names: Vec<String> = added.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        names.contains(&"追加的标签".to_string()),
        "body={}",
        added.1
    );

    let tag_id = state
        .with(|conn| {
            conn.query_row(
                "SELECT id FROM tags WHERE name = '追加的标签'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap();
    let removed = send_empty(
        router_for(state.clone()),
        "DELETE",
        &format!("/api/prompts/11/tags/{tag_id}"),
    )
    .await;
    assert_eq!(removed.0, StatusCode::OK);
    // 11 上原本还有"国风"（夹具），所以断言"被移除的那个不在了"
    let names: Vec<String> = removed.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"追加的标签".to_string()));
}

/// 删提示词：产物**保留但解除关联**（FK `ON DELETE SET NULL`）+ 账本记 delete。
#[tokio::test]
async fn delete_prompt_keeps_media_and_logs_delete() {
    let state = state();
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/prompts/10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"deleted": 10}));

    let prompt_id: Option<i64> = state
        .with(|conn| {
            conn.query_row(
                "SELECT prompt_id FROM media_assets WHERE id = 100",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        prompt_id, None,
        "产物必须保留、只解除关联（R1 生效才有 ON DELETE SET NULL）"
    );

    let deletes: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'prompts' AND pk = '10' AND op = 'delete'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(deletes, 1);
}

/// 还没搬的端点要**明确 501**，不许拿空数组假装成功；搬过来的则必须真的能用。
#[tokio::test]
async fn unmigrated_endpoints_answer_501_and_migrated_ones_work() {
    // 搬过来了：提示词下的产物列表（200 + 数组，提示词不存在也是空数组——与 Kotlin 同）
    let (status, body) = get_json(router_for(state()), "/api/prompts/10/media").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    let (status, body) = get_json(router_for(state()), "/api/prompts/9999/media").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));

    // 搬过来了：视频封面（P4 第 51 步）。夹具里的 100 是 IMAGE 产物 → **204**（图片不用封面）；
    // 视频分支要起 pwsh 抽帧，归 `tests/poster_test.rs` 那一层（真脚本 + 假视频文件）
    let (status, _) = get_json(router_for(state()), "/api/media/100/poster").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "图片产物不该有封面");
}
