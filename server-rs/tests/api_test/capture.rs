//! 自动捕获：本机工作流文件、捕获配置与工作流原文、轮询入库、产物下载兜底。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

#[tokio::test]
async fn capture_workflows_lists_files_with_the_same_keys() {
    let (state, _dir) = state_with_workflow_files();
    let (status, body) = get_json(router_for(state), "/api/capture/workflows").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(
        keys(&body),
        vec!["count", "dirs", "files", "hint", "total"],
        "少一个键 = 界面上少一块内容"
    );
    // 只列真的能用的：坏 JSON 不列（列了 AI 会白跑一趟 comfy_load_workflow）
    assert_eq!(body["total"], 1, "body={body}");
    let file = &body["files"][0];
    let mut file_keys = keys(file);
    file_keys.sort();
    assert_eq!(
        file_keys,
        vec![
            "bytes",
            "format",
            "inLibrary",
            "modifiedAt",
            "name",
            "path",
            "promptId"
        ],
        "file 的键集合"
    );
    assert_eq!(file["name"], "krea2_hd.json");
    assert_eq!(file["format"], "ui");
    // `file:<sha256>` 是**同一把钥匙**：在库里就得如实说，并且给出可以直接提交的 promptId
    assert_eq!(file["inLibrary"], true);
    assert_eq!(file["promptId"], 10);
    assert!(
        !body["dirs"].as_array().unwrap().is_empty(),
        "三份来源里的项目内 comfyui 必须被认出来"
    );
}

#[tokio::test]
async fn capture_workflows_query_filters_by_file_name() {
    let (state, _dir) = state_with_workflow_files();
    let (_, hit) = get_json(router_for(state.clone()), "/api/capture/workflows?q=KREA2").await;
    assert_eq!(hit["count"], 1, "query 是文件名的**不区分大小写**子串");
    let (_, miss) = get_json(
        router_for(state),
        "/api/capture/workflows?q=%E6%B2%A1%E6%9C%89",
    )
    .await;
    assert_eq!(miss["count"], 0);
    // 没命中时要说清楚"共有几份、可以不带 query 再列一次"，而不是干巴巴一个空数组
    assert!(miss["message"].as_str().unwrap().contains("文件名匹配"));
}

#[tokio::test]
async fn capture_locate_reports_the_home_and_never_writes_on_its_own() {
    let (state, home) = state_with_workflow_files();
    let (status, body) = get_json(router_for(state.clone()), "/api/capture/locate").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let mut shape = keys(&body);
    shape.sort();
    assert_eq!(
        shape,
        vec!["candidates", "home", "outputDir", "source"],
        "与 Kotlin `ComfyLocation` 同形（`configuredOutputDir` / `note` 是 null → 键不出现）"
    );
    // 夹具把 ComfyUI 摆在项目根下的 `comfyui`（源码形态：main.py + comfy/）
    assert_eq!(
        body["home"].as_str().unwrap(),
        home.to_string_lossy(),
        "body={body}"
    );
    assert_eq!(
        body["outputDir"].as_str().unwrap(),
        home.join("output").to_string_lossy(),
        "输出目录 = <home>\\output（还没跑过 ComfyUI 也照样给）"
    );
    assert!(
        !body
            .as_object()
            .unwrap()
            .contains_key("configuredOutputDir"),
        "没配过就是 null → 这个键不出现，也不编一个：{body}"
    );
    // 候选中记录了"项目内的 comfyui 目录"这一条，且带判定依据（排查时需要）
    let candidates = body["candidates"].as_array().unwrap();
    let hit = candidates
        .iter()
        .find(|c| c["kind"].as_str().unwrap().contains("项目内的 comfyui"))
        .expect("项目内的 comfyui 必须在候选里");
    assert_eq!(hit["looksLikeComfy"], true);
    assert!(hit["reason"].as_str().unwrap().contains("main.py"));

    // **只读**：探测一遍之后配置里不该多出任何东西
    let (_, config) = get_json(router_for(state), "/api/capture/config").await;
    assert!(
        !config.as_object().unwrap().contains_key("outputDir"),
        "探测不许自动生效（要用户点「使用这个目录」）；没配时 outputDir 键也不出现：{config}"
    );
}

#[tokio::test]
async fn locate_apply_validates_the_dir_and_then_persists_it() {
    let (state, home) = state_with_workflow_files();
    let output = home.join("output");
    std::fs::create_dir_all(&output).unwrap();

    // 空 → 400；不存在 → 400（这两条都会把"捕获一条也收不到"的配置写进去，必须先挡住）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": "   "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    let (status, _) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": "D:\\comfyhub-not-here-98765"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 真的存在 → 写进配置，并且**回读得到**（返回的就是归一化后的那份配置）
    let (status, saved) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": output.to_string_lossy()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={saved}");
    assert_eq!(
        saved["outputDir"].as_str().unwrap(),
        output.to_string_lossy()
    );
    let (_, config) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(
        config["outputDir"].as_str().unwrap(),
        output.to_string_lossy()
    );

    // 配置后 locate 即"按该值使用"（尊重已有配置，不再探测）
    let (_, again) = get_json(router_for(state), "/api/capture/locate").await;
    assert_eq!(
        again["configuredOutputDir"].as_str().unwrap(),
        output.to_string_lossy()
    );
    assert!(again["source"].as_str().unwrap().contains("已经配置过了"));
    assert_eq!(again["home"].as_str().unwrap(), home.to_string_lossy());
}
// ===========================================================================
//  捕获配置与工作流原文（P4 第 7 步）
// ===========================================================================

#[tokio::test]
async fn capture_config_round_trips_and_normalizes() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec![
            "autoTag",
            "comfyUrl",
            "downloadFallback",
            "enabled",
            "maxPerPoll",
            "pollSeconds"
        ],
        "没配过输出目录 → `outputDir` 这个键**根本不出现**（Kotlin 实测形状）：{body}"
    );
    assert_eq!(
        body["comfyUrl"], "http://127.0.0.1:8188",
        "库里没配过就是环境默认值"
    );
    assert_eq!(body["autoTag"], "ComfyUI");
    assert_eq!(body["pollSeconds"], 4);

    // PUT 是**整体覆盖 + 归一化**：尾斜杠去掉、空白目录当 NULL、越界数值夹住
    let (status, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/capture/config",
        serde_json::json!({
            "enabled": false,
            "comfyUrl": "  http://127.0.0.1:8188/  ",
            "outputDir": "   ",
            "pollSeconds": 99999,
            "autoTag": "  ComfyUI  ",
            "maxPerPoll": 0,
            "downloadFallback": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={saved}");
    assert_eq!(saved["comfyUrl"], "http://127.0.0.1:8188");
    assert!(
        !saved.as_object().unwrap().contains_key("outputDir"),
        "空白目录归一化成 NULL → 键不出现（Kotlin `explicitNulls = false`）：{saved}"
    );
    assert_eq!(
        saved["pollSeconds"], 600,
        "夹到上限：轮询太快会把 ComfyUI 问崩"
    );
    assert_eq!(saved["maxPerPoll"], 1);
    assert!(!saved["comfyUrl"].as_str().unwrap().ends_with('/'));

    // 再读一次：落库的确实是归一化后的那份（GET 与 PUT 的响应必须一致）
    let (_, again) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(again, saved);

    // 缺字段的 body 也要能收（每个字段都有默认值：前端版本不一致时不该整条 400）
    let (status, partial) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/capture/config",
        serde_json::json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(partial["enabled"], true);
    assert_eq!(partial["autoTag"], "ComfyUI", "缺的字段走默认值");
}

#[tokio::test]
async fn workflow_endpoints_distinguish_404_from_204() {
    let state = state();
    // 夹具里 id=10 有 workflow_json、id=11 没有
    let response = router_for(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/prompts/10/workflow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], br#"{"nodes":[]}"#, "原文直出，不重新序列化");

    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/workflow").await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "有这条提示词但没有工作流 → 204（不是 404）"
    );

    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/9999/workflow").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "提示词不存在才是 404");
    assert!(body["error"].as_str().unwrap().contains("9999"));

    // 产物侧同一套语义
    state
        .with(|conn| {
            conn.execute(
                "UPDATE media_assets SET workflow_json = '{\"a\":1}' WHERE id = 100",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let response = router_for(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/media/100/workflow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (status, _) = get_json(router_for(state.clone()), "/api/media/9999/workflow").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// `GET /api/prompts/{id}/api-graph` —— **API 格式**节点图。
///
/// 这条是 R10 的前提：`comfy_submit` 只认 API 图，而它的唯一来源就是这里。
/// 所以三种结局必须分得清清楚楚 —— **把 204 说成 404，界面就会以为"提示词没了"**。
#[tokio::test]
async fn api_graph_distinguishes_404_from_204_from_200() {
    let state = state();

    // ① 有来源、有 raw、raw 里是节点图 → 200 + 图本体
    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::OK, "夹具 id=11 的来源指向 run:abc");
    assert_eq!(body["3"]["class_type"], "KSampler");
    assert_eq!(body["3"]["inputs"]["seed"], 1);

    // ② 有这条提示词但没来源 → 204（不是 404、也不是空对象）
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/10/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "source_ref 为空 → 204");

    // ③ 来源指向一次**没有 raw** 的捕获 → 还是 204（"这条跑不了"，而不是"没这条提示词"）
    state
        .with(|conn| {
            conn.execute(
                "UPDATE prompts SET source_ref = 'run:gone' WHERE id = 11",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "来源悬空 → 204");

    // ④ raw 是脏数据（解析不了）也只回 204 —— **绝不猜着转换**
    state
        .with(|conn| {
            conn.execute(
                "UPDATE capture_runs SET raw = '{not json' WHERE run_key = 'run:abc'",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "UPDATE prompts SET source_ref = 'run:abc' WHERE id = 11",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "脏 raw 不猜，回 204");

    // ⑤ raw 里没有节点图（只有 outputs 之类的壳）→ 204
    state
        .with(|conn| {
            conn.execute(
                "UPDATE capture_runs SET raw = '{\"outputs\":{}}' WHERE run_key = 'run:abc'",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "没有图就是没有图");

    // ⑥ 提示词根本不存在才是 404
    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/9999/api-graph").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("9999"));
}

/// `POST /api/capture/poll` 的形状（Kotlin `CapturePollResult`：5 个键、message 可空）。
#[tokio::test]
async fn capture_poll_endpoint_returns_the_kotlin_shape() {
    let state = state();
    // ComfyUI 不在 → ok=false + 一句人话（**不是 500**）
    let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
        comfy_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    comfyhub_server_rs::comfy::capture::save_capture_config(&state, config).unwrap();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/poll",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        keys(&body),
        vec!["checked", "message", "newMedia", "newRuns", "ok"]
    );
    assert_eq!(body["ok"], false);
    assert!(
        body["message"].as_str().unwrap().contains("连不上 ComfyUI"),
        "{body}"
    );

    // 工具层是**同一个实现**（别再写一份）
    let out = call_tool(&state, "comfy_sync_history", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(out.payload["ok"], false);
    assert!(out.content.contains("连不上 ComfyUI"), "{}", out.content);

    std::fs::remove_dir_all(state.project_root()).ok();
}
