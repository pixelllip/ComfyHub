//! 工作流配方（P4 第 67 步）的**集成**用例：真 socket 上的假 ComfyUI + 真 HTTP 端点。
//!
//! # 为什么这一组必须起真 socket
//!
//! 学习这条路最关键的一条纪律是"**不许学一条缺了节点的骨架**" —— 而"缺了节点"这件事
//! **只有在一次真的界面格式转换里才会出现**：转换要问 ComfyUI 要 `/object_info`，
//! 问不到类名就把那个节点摘掉，于是骨架看着像、其实缺了链，**而且不报错**。
//! 所以这里在 `127.0.0.1:0` 上起一个假 ComfyUI（**绝不出网**，与 `danbooru_test.rs`
//! / `submit_test.rs` 同一个做法），喂一份**节点不全**的 `/object_info`，
//! 走的正是真机会走的那条路。
//!
//! 另一半是 HTTP 形状：界面（`AiApiClient`）读的是键名，写错了**不报错**、
//! 只是界面上那一块空着 —— 所以列表 / 详情 / 绑定 / 删除四条各断言一次键集合。

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

use comfyhub_server_rs::routes::AppState;

// ---------------------------------------------------------------------------
//  假 ComfyUI（只有 `/object_info`，配方这条路只问它）
// ---------------------------------------------------------------------------

/// 一份**故意少了 Krea2 那几个类**的节点定义表 —— 真机现场就是这样：
/// 用户的 Krea 2 图像编辑工作流里有 `Krea2EditModelPatch`，而他当时跑着的实例没装那个节点包。
fn object_info_fixture() -> Value {
    json!({
        "CheckpointLoaderSimple": { "input": { "required": {
            "ckpt_name": [["krea2.safetensors", "krea2_turbo_v11.safetensors"]] } } },
        "UNETLoader": { "input": { "required": {
            "unet_name": [["new_unet.safetensors", "old_unet.safetensors"]],
            "weight_dtype": [["default", "fp8_e4m3fn"]] } } },
        "CLIPLoader": { "input": { "required": {
            "clip_name": [["qwen3vl_4b.safetensors"]], "type": [["krea2", "qwen_image"]] } } },
        "VAELoader": { "input": { "required": { "vae_name": [["wan_2.1_vae.safetensors"]] } } },
        "CLIPTextEncode": { "input": { "required": {
            "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } } },
        "EmptyLatentImage": { "input": { "required": {
            "width": ["INT"], "height": ["INT"], "batch_size": ["INT"] } } },
        "KSampler": { "input": { "required": {
            "model": ["MODEL"],
            "seed": ["INT", {"default": 0}],
            "steps": ["INT", {"default": 20}],
            "cfg": ["FLOAT", {"default": 8.0}],
            "sampler_name": [["euler", "dpmpp_2m"]],
            "scheduler": [["normal", "simple"]],
            "positive": ["CONDITIONING"],
            "latent_image": ["LATENT"] } } },
        "SaveImage": { "input": { "required": {
            "images": ["IMAGE"], "filename_prefix": ["STRING"] } } }
    })
}

/// 假 ComfyUI：`127.0.0.1:0`（端口由内核选，绝不与真实例撞车），返回它的 base url。
async fn fake_comfyui() -> String {
    let app = Router::new().route(
        "/object_info",
        get(|| async move { Json(object_info_fixture()) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定假 ComfyUI");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("假 ComfyUI 服务");
    });
    // 让 handle 活到测试结束（进程退出时一起结束）
    std::mem::forget(handle);
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
//  界面格式夹具：**含一个当前实例没有的类**（这就是"缺口"的来源）
// ---------------------------------------------------------------------------

/// 界面格式工作流：`58 Krea2EditModelPatch` 挂在 `UNETLoader → KSampler.model` 中间。
/// 假 `/object_info` 里没有这个类 → 转换摘掉它 → KSampler.model 悬空 → 默认**拒学**。
fn ui_workflow_with_a_gap() -> Value {
    json!({
        "nodes": [
            {"id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["krea2.safetensors"]},
            {"id": 2, "type": "CLIPTextEncode",
             "inputs": [{"name": "clip", "type": "CLIP", "link": 4}],
             "widgets_values": ["1girl, ink painting"]},
            {"id": 4, "type": "EmptyLatentImage", "widgets_values": [768, 1024, 1]},
            {"id": 55, "type": "UNETLoader",
             "widgets_values": ["old_unet.safetensors", "default"]},
            {"id": 58, "type": "Krea2EditModelPatch", "title": "参考图编辑",
             "inputs": [{"name": "model", "type": "MODEL", "link": 8}], "widgets_values": []},
            {"id": 5, "type": "KSampler",
             "inputs": [
               {"name": "model", "type": "MODEL", "link": 9},
               {"name": "positive", "type": "CONDITIONING", "link": 2},
               {"name": "latent_image", "type": "LATENT", "link": 6}],
             "widgets_values": [12345, "fixed", 20, 2.5, "euler", "normal"]},
            {"id": 6, "type": "SaveImage",
             "inputs": [{"name": "images", "type": "IMAGE", "link": 7}],
             "widgets_values": ["krea2/t2i"]}
        ],
        "links": [
            [2, 2, 0, 5, 1, "CONDITIONING"],
            [4, 1, 0, 2, 0, "CLIP"],
            [6, 4, 0, 5, 3, "LATENT"],
            [7, 5, 0, 6, 0, "IMAGE"],
            [8, 55, 0, 58, 0, "MODEL"],
            [9, 58, 0, 5, 0, "MODEL"]
        ]
    })
}

/// 一份**零转换**的 API 节点图（学它不需要 ComfyUI，但绑它要用清单）。
fn api_graph() -> Value {
    json!({
        "1": {"class_type": "UNETLoader",
              "inputs": {"unet_name": "old_unet.safetensors", "weight_dtype": "default"}},
        "2": {"class_type": "CLIPLoader",
              "inputs": {"clip_name": "qwen3vl_4b.safetensors", "type": "krea2"}},
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": "wan_2.1_vae.safetensors"}},
        "4": {"class_type": "LoadImage", "inputs": {"image": "ref.png"}},
        "5": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": "换成跑步"}},
        "6": {"class_type": "KSampler",
              "inputs": {"model": ["1", 0], "positive": ["5", 0], "latent_image": ["4", 0],
                         "seed": 1, "steps": 8, "cfg": 1.0, "sampler_name": "euler",
                         "scheduler": "simple", "denoise": 1.0}},
        "7": {"class_type": "VAEDecode", "inputs": {"samples": ["6", 0], "vae": ["3", 0]}},
        "8": {"class_type": "SaveImage", "inputs": {"images": ["7", 0]}}
    })
}

// ---------------------------------------------------------------------------
//  夹具：库 + 项目根 + 一份界面格式工作流文件 + 指向假 ComfyUI 的配置
// ---------------------------------------------------------------------------

struct Fixture {
    state: AppState,
    root: std::path::PathBuf,
}

/// 内存库 + 真 DDL。项目根由 `media_dir` 推出来（`<root>/storage/media` → `<root>`）。
fn fixture_sync(comfy_url: String) -> Fixture {
    let conn: Connection = comfyhub_server_rs::db::open_memory().unwrap();
    conn.execute_batch(comfyhub_server_rs::db::schema_sql()).expect("建 schema");
    comfyhub_server_rs::db::assert_ready(&conn).unwrap();

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir()
        .join(format!("comfyhub-recipe-{}-{unique}-{seq}", std::process::id()));
    std::fs::create_dir_all(root.join("storage").join("media")).unwrap();
    std::fs::create_dir_all(root.join("comfyui")).unwrap();
    std::fs::write(
        root.join("comfyui").join("krea2_edit_ui.json"),
        serde_json::to_string_pretty(&ui_workflow_with_a_gap()).unwrap(),
    )
    .unwrap();

    let state = AppState::new(conn, ":memory:", root.join("storage").join("media"));
    let config = comfyhub_server_rs::capture::CaptureConfig {
        comfy_url,
        ..Default::default()
    };
    comfyhub_server_rs::capture::save_capture_config(&state, config).expect("写捕获配置");
    Fixture { state, root }
}

async fn fixture() -> Fixture {
    let base = fake_comfyui().await;
    fixture_sync(base)
}

// ---------------------------------------------------------------------------
//  小工具
// ---------------------------------------------------------------------------

async fn call_tool(
    state: &AppState,
    name: &str,
    arguments: Value,
) -> Result<comfyhub_server_rs::tools::ToolOutcome, comfyhub_server_rs::tools::ToolFailure> {
    let policy = comfyhub_server_rs::policy::ToolPolicy::load(state).unwrap();
    let mut session = comfyhub_server_rs::tools::ToolSession::default();
    comfyhub_server_rs::tools::execute(state, &policy, "run-x", "conv-x", name, &arguments, &mut session)
        .await
}

async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(HttpRequest::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn send_json(app: Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
//  ① 学习：**不许学一条缺了节点的骨架**
// ---------------------------------------------------------------------------

#[tokio::test]
async fn 学界面格式工作流时默认拒掉有缺口的骨架() {
    let f = fixture().await;
    let path = f.root.join("comfyui").join("krea2_edit_ui.json");
    let args = json!({ "path": path.display().to_string() });

    // 默认档：**拒学**，而且要说清"是实例缺节点，不是工作流坏了"
    let failure = call_tool(&f.state, "comfy_learn_workflow", args.clone()).await.unwrap_err();
    assert_eq!(failure.code, "UNSUPPORTED_NODES", "{}", failure.message);
    assert!(failure.message.contains("摘掉节点类：58 Krea2EditModelPatch"), "{}", failure.message);
    assert!(failure.message.contains("悬空输入"), "{}", failure.message);
    assert!(failure.message.contains("本机这个实例"), "{}", failure.message);
    assert!(failure.message.contains("四个出口"), "{}", failure.message);
    // 拒了就不许留行（"报错的行不许半截写进去"）
    let rows: i64 = f
        .state
        .with(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM workflow_recipes", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 0, "被拒的学习不该往库里写");

    // 带 tolerateUnsupported=true 才学，而且**如实标 complete=false + 缺口清单**
    let out = call_tool(
        &f.state,
        "comfy_learn_workflow",
        json!({ "path": path.display().to_string(), "tolerateUnsupported": true }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["complete"], json!(false));
    assert_eq!(out.payload["format"], "ui");
    assert_eq!(out.payload["sourceKind"], "file");
    assert!(
        out.payload["sourceRef"].as_str().unwrap().starts_with("file:"),
        "幂等键要与 comfy_load_workflow 同一把钥匙：{}",
        out.payload["sourceRef"]
    );
    let gaps = out.payload["gaps"].as_array().unwrap();
    assert!(gaps.iter().any(|g| g.as_str().unwrap().contains("Krea2EditModelPatch")), "{gaps:?}");
    assert!(out.payload["recipeId"].as_i64().unwrap() > 0);
    assert!(out.content.contains("【工作流配方】"), "{}", out.content);
    // ⚠ **有意与 Kotlin 不同**（本后端还没实现「模拟用户行为」）：必须如实补一句，
    //   否则用户以为那些前端节点已经被照前端的做法接好了。
    //   它落在**配方行的 warnings** 里（转换警告与归纳警告同一栏，Kotlin 也是这么存的），
    //   所以走详情端点读回来验（工具当场回的是 `recipe.render`，只有归纳那几条）。
    let id = out.payload["recipeId"].as_i64().unwrap();
    let (status, detail) = get_json(
        comfyhub_server_rs::routes::router(f.state.clone()),
        &format!("/api/ai/workflow-recipes/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["recipe"]["complete"], json!(false), "缺件的配方要如实标 complete=false");
    let warnings: Vec<String> = detail["recipe"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("还没实现")),
        "要说清「模拟用户行为」这一步本后端没做：{warnings:?}"
    );
    let gaps: Vec<String> = detail["recipe"]["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap().to_string())
        .collect();
    assert!(gaps.iter().any(|g| g.contains("Krea2EditModelPatch")), "{gaps:?}");
    // 详情里的 render 与工具当场回的那份**跑的是同一个函数**（`Recipe::render`）。
    // ⚠ 两者**可以**不同，而且这与 Kotlin 一致：工具回的是 `learned.recipe`（只有归纳那几条警告），
    //   而详情是从**落库的行**还原的 —— 那一行的 warnings 里还带着转换警告（含上面那句
    //   「模拟用户行为」的如实说明）。所以这里比的是"同一个头 + 同一份槽位正文"，
    //   不是逐字节相等（逐字节相等只在零转换的 API 图上成立，见下面那条用例）。
    let tool_text = out.content.as_str();
    let detail_text = detail["render"].as_str().unwrap();
    assert_eq!(tool_text.lines().next(), detail_text.lines().next(), "同一个渲染函数、同一个标题");
    assert!(detail_text.contains("55.unet_name = old_unet.safetensors"), "{detail_text}");
    assert!(detail_text.contains("还没实现"), "落库的 warnings 要带出转换那句话：{detail_text}");

    // 同一个文件学两次 = 同一行（幂等靠 file:<sha256>）
    let again = call_tool(
        &f.state,
        "comfy_learn_workflow",
        json!({ "path": path.display().to_string(), "tolerateUnsupported": true }),
    )
    .await
    .unwrap();
    assert_eq!(again.payload["recipeId"], out.payload["recipeId"]);
    let rows: i64 = f
        .state
        .with(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM workflow_recipes", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 1, "同一份文件学两次只有一行");

    std::fs::remove_dir_all(&f.root).ok();
}

#[tokio::test]
async fn 学_api_格式的图零转换_且槽位与旋钮都认得出来() {
    let f = fixture().await;
    let out = call_tool(&f.state, "comfy_learn_workflow", json!({ "graph": api_graph() }))
        .await
        .unwrap();
    assert_eq!(out.payload["complete"], json!(true), "API 格式零转换，不该有缺口");
    assert_eq!(out.payload["task"], "i2i");
    assert_eq!(out.payload["slotCount"], json!(3));
    assert_eq!(out.payload["sourceKind"], "graph");
    assert!(out.payload["sourceRef"].as_str().unwrap().starts_with("graph:"));
    // 正文里要能直接看到"改哪个键"
    assert!(out.content.contains("1.unet_name = old_unet.safetensors"), "{}", out.content);
    assert!(out.content.contains("2.type"), "CLIPLoader.type 是换模型族时要改的加载器参数：{}", out.content);

    std::fs::remove_dir_all(&f.root).ok();
}

// ---------------------------------------------------------------------------
//  ② 列配方
// ---------------------------------------------------------------------------

#[tokio::test]
async fn 列配方_空库要指到学习入口_有配方要能按条件搜() {
    let f = fixture().await;
    // 空库：不是回一个空列表，而是 NOT_FOUND + 明确的下一步
    let failure = call_tool(&f.state, "comfy_list_recipes", json!({})).await.unwrap_err();
    assert_eq!(failure.code, "NOT_FOUND");
    assert!(failure.message.contains("comfy_learn_workflow"), "{}", failure.message);
    assert!(failure.message.contains("comfy_list_workflows"), "{}", failure.message);

    call_tool(&f.state, "comfy_learn_workflow", json!({ "graph": api_graph() }))
        .await
        .unwrap();
    let out = call_tool(&f.state, "comfy_list_recipes", json!({})).await.unwrap();
    assert_eq!(out.payload["count"], json!(1));
    assert!(out.content.contains("配方库：命中 1 条"), "{}", out.content);
    assert!(out.content.contains("4.unet_name") || out.content.contains("1.unet_name"), "{}", out.content);
    // 槽位要以 `键(角色)=文件` 的紧凑形状给出来（模型据此决定绑哪个）
    assert!(out.content.contains("1.unet_name(diffusion_models)=old_unet.safetensors"), "{}", out.content);

    // 按 task 过滤：命中 → 有；不命中 → 空库那句（带"匹配这些条件的"）
    assert!(call_tool(&f.state, "comfy_list_recipes", json!({ "task": "i2i" })).await.is_ok());
    let none = call_tool(&f.state, "comfy_list_recipes", json!({ "task": "audio" }))
        .await
        .unwrap_err();
    assert_eq!(none.code, "NOT_FOUND");
    assert!(none.message.contains("匹配这些条件的"), "{}", none.message);
    // 关键字能搜到槽位里的模型文件名（"这个模型配过哪条配方"）
    assert!(call_tool(&f.state, "comfy_list_recipes", json!({ "q": "old_unet" })).await.is_ok());
    assert!(call_tool(&f.state, "comfy_list_recipes", json!({ "q": "没有这个" })).await.is_err());

    std::fs::remove_dir_all(&f.root).ok();
}

// ---------------------------------------------------------------------------
//  ③ 绑定（**只报不改**）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn 绑定走本机清单_只报不改() {
    let f = fixture().await;
    let learned = call_tool(&f.state, "comfy_learn_workflow", json!({ "graph": api_graph() }))
        .await
        .unwrap();
    let recipe_id = learned.payload["recipeId"].as_i64().unwrap();

    // 同名且实例能看到（假 /object_info 的 unet_name 枚举里有它）→ exact，**可以自动绑**
    let out = call_tool(
        &f.state,
        "comfy_bind_recipe",
        json!({ "recipeId": recipe_id, "model": "new_unet.safetensors" }),
    )
    .await
    .unwrap();
    assert_eq!(out.payload["bindings"][0]["status"], "exact");
    assert_eq!(out.payload["overrides"]["1.unet_name"], "new_unet.safetensors");
    assert_eq!(out.payload["ready"], json!(true));
    assert!(out.content.contains("可以直接跑"), "{}", out.content);
    // 没动的槽位照样列出来
    assert!(out.content.contains("2.clip_name"), "{}", out.content);

    // 本机没有 → missing，且**绝对不许编文件名**
    // （名字要挑得与清单里任何一个都不像，否则走的是 substitute 那一档 —— 那是另一条用例）
    let missing = call_tool(
        &f.state,
        "comfy_bind_recipe",
        json!({ "recipeId": recipe_id, "model": "totally_unknown_thing.safetensors" }),
    )
    .await
    .unwrap();
    assert_eq!(missing.payload["bindings"][0]["status"], "missing");
    assert!(missing.payload["overrides"].as_object().unwrap().is_empty());
    assert!(missing.content.contains("不要自己编"), "{}", missing.content);

    // 库里的配方 id 不存在 → NOT_FOUND（不是空报告）
    let absent = call_tool(
        &f.state,
        "comfy_bind_recipe",
        json!({ "recipeId": 9999, "model": "x.safetensors" }),
    )
    .await
    .unwrap_err();
    assert_eq!(absent.code, "NOT_FOUND");
    // 缺参数：稳定码 + 指到下一步
    let no_id = call_tool(&f.state, "comfy_bind_recipe", json!({ "model": "x.safetensors" }))
        .await
        .unwrap_err();
    assert_eq!(no_id.code, "INVALID_ARGUMENT");
    assert!(no_id.message.contains("comfy_list_recipes"), "{}", no_id.message);
    // `extra` 里的值必须是**字符串文件名**（空串 / 对象都不行 —— Kotlin 那条判据是 `contentOrNull`）
    let bad_extra = call_tool(
        &f.state,
        "comfy_bind_recipe",
        json!({ "recipeId": recipe_id, "model": "x.safetensors", "extra": { "vae": "  " } }),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_extra.code, "INVALID_ARGUMENT");
    assert!(bad_extra.message.contains("extra 里的 vae"), "{}", bad_extra.message);

    std::fs::remove_dir_all(&f.root).ok();
}

// ---------------------------------------------------------------------------
//  ④ HTTP 端点（形状：界面读的是键名，写错不报错只是空一块）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn 配方五条端点的形状与状态码() {
    let f = fixture().await;
    let app = || comfyhub_server_rs::routes::router(f.state.clone());

    // POST 学一份（**201 Created** + `learned.toJson(id)`）：学与工具共用一份实现
    let (status, created) = send_json(
        app(),
        "POST",
        "/api/ai/workflow-recipes",
        json!({ "graph": api_graph(), "name": "krea2 参考图编辑" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["recipeId"].as_i64().unwrap();
    assert_eq!(created["name"], "krea2 参考图编辑");
    assert_eq!(created["task"], "i2i");
    assert_eq!(created["complete"], json!(true));
    assert_eq!(created["format"], "api");
    assert!(created["render"].as_str().unwrap().contains("【工作流配方】"));
    assert_eq!(created["recipe"]["skeletonHash"], created["skeletonHash"]);

    // GET 列表：直接回数组（Kotlin `call.respond(list)`）
    let (status, listed) = get_json(app(), "/api/ai/workflow-recipes").await;
    assert_eq!(status, StatusCode::OK);
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    let mut keys: Vec<&str> = listed[0].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "classCount",
            "classes",
            "complete",
            "createdAt",
            "edges",
            "gaps",
            "hasMask",
            "id",
            "imageInputs",
            "knobs",
            "name",
            "nodeCount",
            "outputs",
            "recipeVersion",
            "skeletonHash",
            "slots",
            "sourceKind",
            "sourceRef",
            "task",
            "updatedAt",
            "warnings",
        ],
        "少一个键 = 界面上少一块内容；多一个 null 键 = 与 Kotlin 不是同一个形状"
    );
    // 过滤参数照 Kotlin 的口径：`complete=true` 认 1/true/yes（大小写不敏感）
    let (_, only_complete) = get_json(app(), "/api/ai/workflow-recipes?complete=YES&task=i2i").await;
    assert_eq!(only_complete.as_array().unwrap().len(), 1);
    let (_, none) = get_json(app(), "/api/ai/workflow-recipes?task=audio").await;
    assert!(none.as_array().unwrap().is_empty());
    let (_, by_q) = get_json(app(), "/api/ai/workflow-recipes?q=old_unet").await;
    assert_eq!(by_q.as_array().unwrap().len(), 1);

    // GET 详情：`{recipe, render}`（render 与工具当场回的那份是同一个函数）
    let (status, detail) = get_json(app(), &format!("/api/ai/workflow-recipes/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let mut keys: Vec<&str> = detail.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["recipe", "render"]);
    assert_eq!(detail["recipe"]["id"], json!(id));
    assert!(detail["render"].as_str().unwrap().contains("1.unet_name = old_unet.safetensors"));

    // POST 绑定：`{recipeId, report, render}`，report 与工具同一份
    let (status, bound) = send_json(
        app(),
        "POST",
        &format!("/api/ai/workflow-recipes/{id}/bind"),
        json!({ "model": "new_unet.safetensors", "extra": { "text_encoders": "qwen3vl_4b.safetensors" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    let mut keys: Vec<&str> = bound.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["recipeId", "render", "report"]);
    assert_eq!(bound["recipeId"], json!(id));
    assert_eq!(bound["report"]["overrides"]["1.unet_name"], "new_unet.safetensors");
    assert_eq!(bound["report"]["bindings"][0]["applied"], json!(true));
    assert!(bound["render"].as_str().unwrap().contains("可以直接跑"), "{}", bound["render"]);

    // DELETE → `{deleted: id}`；再删一次 **404 + 人话**
    let (status, deleted) = send_json(
        app(),
        "DELETE",
        &format!("/api/ai/workflow-recipes/{id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted, json!({ "deleted": id }));
    let (status, gone) = get_json(app(), &format!("/api/ai/workflow-recipes/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(gone["error"], format!("配方 {id} 不存在"));
    assert!(gone.get("detail").is_none(), "普通错误不带 detail（与 Kotlin 同形）");
    let (status, _) = send_json(
        app(),
        "DELETE",
        &format!("/api/ai/workflow-recipes/{id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 非法 id → 400 + 人话（**不是** axum 的纯文本）
    let (status, bad) = get_json(app(), "/api/ai/workflow-recipes/abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(bad["error"], "非法的 id");

    std::fs::remove_dir_all(&f.root).ok();
}

#[tokio::test]
async fn 端点学界面格式文件_缺口时回_400_unsupported_nodes() {
    let f = fixture().await;
    let app = || comfyhub_server_rs::routes::router(f.state.clone());
    let path = f.root.join("comfyui").join("krea2_edit_ui.json").display().to_string();

    // AI 域的稳定码走 `{error: 码, detail: 人话}`（界面读 detail）
    let (status, body) = send_json(
        app(),
        "POST",
        "/api/ai/workflow-recipes",
        json!({ "path": path }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "UNSUPPORTED_NODES");
    assert!(body["detail"].as_str().unwrap().contains("本机这个实例"), "{body}");

    // 带 tolerateUnsupported 才学得进去
    let (status, created) = send_json(
        app(),
        "POST",
        "/api/ai/workflow-recipes",
        json!({ "path": path, "tolerateUnsupported": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["complete"], json!(false));
    assert_eq!(created["format"], "ui");
    // ⚠ **缺省值也要与 `comfy_load_workflow` / 工具那条路一致**：请求里**没传** `emulateFrontend`
    //   （Kotlin 的 DTO 默认 `true`），所以那句"本后端还没实现「模拟用户行为」"必须落进这一行 ——
    //   少了它就成了"一个字都不改"那一种（而界面上看不出区别）。`#[serde(default)]` 对
    //   `bool` 是 `false`，这一条钉的就是那个坑。
    //   注意：转换警告只在**落库的行**里（`learned.toJson` 不含 warnings，Kotlin 也是这么设计的），
    //   所以这里走详情端点读回来验。
    let id = created["recipeId"].as_i64().unwrap();
    let (_, detail) = get_json(app(), &format!("/api/ai/workflow-recipes/{id}")).await;
    let warnings: Vec<String> = detail["recipe"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("还没实现")),
        "emulateFrontend 的缺省值必须是 true：{warnings:?}"
    );

    // ⚠ 路径准入与 `comfy_load_workflow` 同一条：**禁写段读也不许**（与用户白名单无关）。
    //   这里刻意用 `.git` 而不是"项目根之外的文件"——后者是**允许**的：
    //   本机 ComfyUI 根的上一级会被自动放进读白名单（那只放宽读，见 AGENTS §10）。
    let (status, denied) = send_json(
        app(),
        "POST",
        "/api/ai/workflow-recipes",
        json!({ "path": f.root.join(".git").join("hooks").join("evil.json").display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{denied}");
    assert_eq!(denied["error"], "PATH_DENIED", "工具层的失败码要翻成 AI 域的口径：{denied}");
    assert!(denied["detail"].as_str().unwrap().contains("不许读"), "{denied}");

    // 三选一都没给 → INVALID_ARGUMENT + 人话
    let (status, none) = send_json(app(), "POST", "/api/ai/workflow-recipes", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(none["error"], "INVALID_ARGUMENT");
    assert!(none["detail"].as_str().unwrap().contains("要么给 path"), "{none}");

    // 界面格式但**连不上** ComfyUI：说清"要 /object_info"（不是 500）
    let offline = fixture_sync("http://127.0.0.1:1".to_string());
    let (status, body) = send_json(
        comfyhub_server_rs::routes::router(offline.state.clone()),
        "POST",
        "/api/ai/workflow-recipes",
        json!({ "path": offline.root.join("comfyui").join("krea2_edit_ui.json").display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "COMFY_UNREACHABLE");
    assert!(body["detail"].as_str().unwrap().contains("/object_info"), "{body}");

    std::fs::remove_dir_all(&f.root).ok();
    std::fs::remove_dir_all(&offline.root).ok();
}

// ---------------------------------------------------------------------------
//  ⑤ 不进同步白名单（R11：手机拿到一份它跑不了的配方只会误导模型）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn 配方表不在同步白名单里_端点也不在手机_run_kit_里() {
    assert!(
        !comfyhub_server_rs::sync::SYNCED.contains(&"workflow_recipes"),
        "总纲 §3.3：这张表**刻意不同步**（手机提交走 comfy_queue_submit）"
    );
    let f = fixture().await;
    let (status, kit) = get_json(comfyhub_server_rs::routes::router(f.state.clone()), "/api/ai/run-kit").await;
    assert_eq!(status, StatusCode::OK);
    let tools: Vec<&str> = kit["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for absent in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
        assert!(!tools.contains(&absent), "配方工具漏进了手机 run-kit：{tools:?}");
    }
    std::fs::remove_dir_all(&f.root).ok();
}
