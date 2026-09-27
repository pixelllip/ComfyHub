//! `comfy_submit`（**R10**）的回归防线 —— 一个**内置的假 ComfyUI** + 真 SQLite + 真入库链路。
//!
//! # 为什么必须有这一组
//!
//! 总纲 §6 的防线归属表里，R10（"提交链路"）在 Rust 侧一直是空的：`e2e-submit-test.ps1` /
//! `e2e-capture-test.ps1` 验的是**Kotlin 那条**，而 Rust 侧的提交此前根本不存在。
//! 这一组把"提交 → 等 → 入库 → mediaIds"整条链在**不出网、不装 ComfyUI** 的前提下钉住：
//!
//! * 假 ComfyUI 是本进程里的一台 axum 服务器（`127.0.0.1:0` 随机端口），
//!   提供 `/object_info`、`/prompt`（记下提交的图）、`/history/{id}`、`/queue`；
//! * 库是内存库 + **真 DDL**（`db/schema.sqlite.sql`），提示词与 `capture_runs.raw` 由夹具灌进去；
//! * 产物是一张**真 PNG**，落在临时 `output` 目录里 —— 入库那条路（sha256 判重 / 缩略图 / 建提示词）
//!   全是生产代码。
//!
//! 四条断言对应四条纪律：
//!  1. **只有真的提交了、真的收进来了，才有 `mediaIds`**（模型不能凭空说"生成了"）；
//!  2. **`overrides` / `connections` 应用在提交出去的那张图上**（假实例记的 body 就是证据）；
//!  3. **预检拦住时一个字节都不发**（拿不到能加载的模型就当场拒，不白烧一轮显卡）；
//!  4. **超时如实报 `timeout`**，既不假装完成也不假装失败。

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State as AxumState};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use comfyhub_server_rs::routes::AppState;

/// 夹具用的 API 格式节点图（提交出去的就是它 + 覆盖 / 补线的结果）。
fn api_graph() -> Value {
    json!({
        "3": {
            "class_type": "KSampler",
            "inputs": { "seed": 42, "steps": 20, "cfg": 7.5, "denoise": 1.0, "model": ["4", 0] }
        },
        "4": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": { "ckpt_name": "krea2.safetensors" }
        },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "old prompt", "clip": ["4", 1] } },
        "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "unit", "images": ["3", 0] } }
    })
}

/// `/object_info`：只认 `krea2.safetensors` 这一个 checkpoint。
fn object_info() -> Value {
    json!({
        "KSampler": { "input": { "required": { "seed": ["INT", {}], "steps": ["INT", {}], "cfg": ["FLOAT", {}] } } },
        "CheckpointLoaderSimple": {
            "input": { "required": { "ckpt_name": [["krea2.safetensors"], {}] } }
        },
        "CLIPTextEncode": { "input": { "required": { "text": ["STRING", {}] } } },
        "SaveImage": { "input": { "required": { "filename_prefix": ["STRING", {}] } } }
    })
}

#[derive(Clone)]
struct FakeState {
    object_info: Value,
    history: Value,
    submitted: Arc<Mutex<Vec<Value>>>,
}

/// 起一台**进程内的假 ComfyUI**，返回它的 base url。
async fn fake_comfy(object_info: Value, history: Value) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let submitted: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let shared = FakeState { object_info, history, submitted: submitted.clone() };

    let app = Router::new()
        .route("/object_info", get(|AxumState(s): AxumState<FakeState>| async move { Json(s.object_info) }))
        .route(
            "/prompt",
            post(
                |AxumState(s): AxumState<FakeState>, Json(body): Json<Value>| async move {
                    s.submitted.lock().unwrap().push(body);
                    Json(json!({ "prompt_id": "comfy-run-1", "number": 1, "node_errors": {} }))
                },
            ),
        )
        .route(
            "/history/{id}",
            get(
                |AxumState(s): AxumState<FakeState>, Path(id): Path<String>| async move {
                    // 没有这条运行 → 空对象（提交方据此认为"还在队列/在跑"）
                    let Some(template) = s.history.get(&id) else {
                        return Json(json!({}));
                    };
                    // ⚠ 与真 ComfyUI（以及 `scripts\e2e\fake_comfy.py`）同一条纪律：
                    // `/history` 回的是**当时提交的那张图**，不是夹具里写死的那张。
                    // 只有这样才能断言"入库解析出的提示词来自真正提交的图"。
                    let mut entry = template.clone();
                    if let Some(graph) = s.submitted.lock().unwrap().first().and_then(|body| body.get("prompt")) {
                        entry["prompt"]["prompt"] = graph.clone();
                    }
                    Json(json!({ id: entry }))
                },
            ),
        )
        .route("/queue", get(|| async { Json(json!({ "queue_running": [], "queue_pending": [] })) }))
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定假 ComfyUI");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), submitted, handle)
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("comfyhub-submit-{label}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 一张**真 PNG**（入库那条路会算 sha256、写缩略图，假图过不去）。
fn write_png(path: &std::path::Path, color: [u8; 3]) {
    let mut image = image::RgbImage::new(12, 12);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb(color);
    }
    image.save(path).expect("写 PNG");
}

/// 夹具：真 DDL 的内存库 + 一条带 API 图的提示词 + 恒定的捕获配置。
struct Fixture {
    state: AppState,
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(label: &str, config_url: &str) -> Self {
        let dir = temp_dir(label);
        let output = dir.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let storage = dir.join("storage");
        std::fs::create_dir_all(storage.join("media")).unwrap();

        let conn = comfyhub_server_rs::db::open_memory().expect("内存库");
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql");
        let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
        conn.execute_batch(&schema).expect("建 schema");

        // 提示词 + 它那次捕获的 raw（`prompt_api_graph` 就是从这条链取图的）
        let raw = json!({
            "prompt": { "prompt": api_graph(), "extra_data": { "client_id": "fixture" } },
            "outputs": { "9": { "images": [{ "filename": "unit.png", "subfolder": "", "type": "output" }] } },
            "status": { "status_str": "success", "completed": true, "messages": [] }
        });
        conn.execute(
            "INSERT INTO prompts (id, title, kind, positive_prompt, source_ref, created_at, updated_at) \
             VALUES (101, 'unit 提交', 'IMAGE', 'a cat, masterpiece', 'unit-run-1', \
                     '2026-09-26T00:00:00.000Z', '2026-09-26T00:00:00.000Z')",
            [],
        )
        .expect("插提示词");
        conn.execute(
            "INSERT INTO capture_runs (run_key, source, status, raw, created_at) \
             VALUES ('unit-run-1', 'ComfyUI', 'success', ?1, '2026-09-26T00:00:00.000Z')",
            rusqlite::params![raw.to_string()],
        )
        .expect("插捕获记录");

        let state = AppState::new(conn, ":memory:", storage.join("media"));
        let config = comfyhub_server_rs::capture::CaptureConfig {
            enabled: true,
            comfy_url: config_url.to_string(),
            output_dir: Some(output.display().to_string()),
            poll_seconds: 4,
            auto_tag: "ComfyUI".to_string(),
            max_per_poll: 20,
            download_fallback: true,
        };
        comfyhub_server_rs::capture::save_capture_config(&state, config).expect("存捕获配置");
        Self { state, dir }
    }

    fn policy(&self) -> comfyhub_server_rs::policy::ToolPolicy {
        comfyhub_server_rs::policy::ToolPolicy::new(self.dir.clone(), Default::default())
    }

    fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn history_with_output() -> Value {
    json!({
        "comfy-run-1": {
            "prompt": { "prompt": api_graph(), "extra_data": { "client_id": "unit" } },
            "outputs": { "9": { "images": [{ "filename": "unit.png", "subfolder": "", "type": "output" }] } },
            "status": { "status_str": "success", "completed": true, "messages": [] }
        }
    })
}

/// 把夹具那条捕获记录里的 API 图改一改（`prompt_api_graph` 就是从这条链取图的）。
fn patch_graph(fixture: &Fixture, edit: impl FnOnce(&mut Value)) {
    fixture
        .state
        .write(|conn| {
            let raw: String = conn.query_row(
                "SELECT raw FROM capture_runs WHERE run_key = 'unit-run-1'",
                [],
                |row| row.get(0),
            )?;
            let mut entry: Value = serde_json::from_str(&raw).unwrap();
            let mut graph = entry["prompt"]["prompt"].clone();
            edit(&mut graph);
            entry["prompt"]["prompt"] = graph;
            conn.execute(
                "UPDATE capture_runs SET raw = ?1 WHERE run_key = 'unit-run-1'",
                rusqlite::params![entry.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn submit_runs_the_workflow_then_ingests_the_product() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("ingest", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [10, 200, 30]);

    let arguments = json!({
        "promptId": 101,
        "title": "单元提交",
        "overrides": { "3.steps": "30", "6.text": "a dog" },
        "connections": { "9.images": ["3", 0] },
        "wait": true,
        "waitSeconds": 20
    });
    let outcome = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect("提交应当成功");

    // ① 提交出去的那张图必须带上覆盖与补线
    let sent = submitted.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "只许提交一次");
    let graph = &sent[0]["prompt"];
    assert_eq!(graph["3"]["inputs"]["steps"], json!(30), "覆盖没生效：{graph}");
    assert_eq!(graph["6"]["inputs"]["text"], json!("a dog"));
    assert_eq!(graph["9"]["inputs"]["images"], json!(["3", 0]));
    assert!(sent[0]["client_id"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "要带 client_id");

    // ② 产物真的入库了，`mediaIds` 才可能出现
    let payload = &outcome.payload;
    assert_eq!(payload["promptId"], json!(101));
    assert_eq!(payload["runKey"], json!("comfy-run-1"));
    assert_eq!(payload["status"], json!("success"), "{payload}");
    let media_ids = payload["mediaIds"].as_array().expect("mediaIds 必须是数组").clone();
    assert_eq!(media_ids.len(), 1, "应当收进 1 个产物：{payload}");
    assert_eq!(payload["mediaCount"], json!(1));
    assert!(payload["capturedPromptId"].as_i64().is_some(), "要有捕获后的提示词 id：{payload}");
    assert_eq!(payload["nsfw"], json!(false));
    assert_eq!(payload["nsfwMediaIds"], json!([]));
    assert!(payload["idHint"].as_str().unwrap().contains("comfy_get_run"));
    assert_eq!(payload["appliedOverrides"], json!(["3.steps ← 30", "6.text ← a dog", "9.images ← [3, 0]"]));

    // ③ 库里那两条记录（capture_runs 收尾 + media_assets 关联到新提示词）
    let run_status: String = fixture
        .state
        .with(|conn| conn.query_row("SELECT status FROM capture_runs WHERE run_key='comfy-run-1'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(run_status, "success");
    let (media_prompt, media_count): (i64, i64) = fixture
        .state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT p.id, (SELECT COUNT(*) FROM media_assets m WHERE m.prompt_id = p.id) \
                 FROM prompts p WHERE p.id = ?1",
                rusqlite::params![payload["capturedPromptId"].as_i64().unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert!(media_prompt > 0 && media_count == 1, "产物要挂在那条新提示词上");
    // 新提示词的正向提示词来自**提交出去的那张图**（模型改了 text，库里就该是新值）
    let positive: String = fixture
        .state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT positive_prompt FROM prompts WHERE id = ?1",
                rusqlite::params![payload["capturedPromptId"].as_i64().unwrap()],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(positive, "a dog", "入库解析出的正向提示词要来自真正提交的图");

    // ④ 提交记录（`/api/capture/jobs` 的 submissions[] 就是它）
    let recorded = fixture.state.submissions.find("comfy-run-1").expect("要有提交记录");
    assert_eq!(recorded["status"], json!("success"));
    assert_eq!(recorded["title"], json!("单元提交"));

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn nsfw_flag_travels_into_the_library() {
    let (base, _submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("nsfw", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [200, 20, 60]);

    let arguments = json!({ "promptId": 101, "nsfw": true, "waitSeconds": 20 });
    let outcome = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect("提交应当成功");
    let media_id = outcome.payload["mediaIds"][0].as_i64().expect("有产物");
    assert_eq!(outcome.payload["nsfwMediaIds"], json!([media_id]), "界面靠它决定打不打码");
    let nsfw: i64 = fixture
        .state
        .with(|conn| {
            Ok(conn.query_row("SELECT nsfw FROM media_assets WHERE id = ?1", rusqlite::params![media_id], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(nsfw, 1, "产物要带上 NSFW 标记");

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn preflight_blocks_unknown_node_classes_before_anything_is_sent() {
    // 实例不认识 Krea2CustomNode（第 12 号节点）—— 必须在**提交之前**拒，
    // 而且**一个字节都不许发出去**（节点类这一档 `/object_info` 是权威的）
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("preflight", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [1, 2, 3]);
    // 图里塞一个实例不认识的节点类
    patch_graph(&fixture, |graph| {
        graph["12"] = json!({ "class_type": "Krea2CustomNode", "inputs": {} });
    });

    let arguments = json!({ "promptId": 101, "waitSeconds": 20 });
    let failure = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect_err("节点类不认识的图必须拒");
    assert_eq!(failure.code, "MODEL_NOT_LOADABLE");
    assert!(failure.message.contains("实例里没有这些节点类"), "{}", failure.message);
    assert!(failure.message.contains("Krea2CustomNode"), "{}", failure.message);
    assert!(submitted.lock().unwrap().is_empty(), "预检拦下时**一个字节都不许发出去**");

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn preflight_warns_about_models_it_cannot_see_but_still_submits() {
    // 模型文件不在实例候选里 → **只警告不拦**（盘扫还没搬，见 `preflight_refusal` 的说明）。
    // 这一条直接钉住"误拦比白跑更糟"这个口径：提交照常发生，警告如实带回。
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("preflight-warn", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [1, 2, 3]);

    let arguments = json!({
        "promptId": 101,
        "overrides": { "4.ckpt_name": "anima-base-v1.0.safetensors" },
        "waitSeconds": 20
    });
    let outcome = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect("这一档现在不拦");
    assert_eq!(submitted.lock().unwrap().len(), 1, "该提交还是要提交");
    let warnings = outcome.payload["preflightWarnings"].as_array().expect("要带回警告");
    let joined = warnings.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("anima-base-v1.0.safetensors"), "{joined}");
    assert!(joined.contains("没有拦"), "{joined}");

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn the_audit_blocks_pure_frontend_nodes_before_anything_is_sent() {
    // 纯前端节点（`Label (rgthree)` / `Anything Everywhere` 那一族）只存在于 ComfyUI **前端**，
    // 服务端根本没有这个类 —— 带着它提交，ComfyUI 会在验证阶段整片丢节点（0.01 秒空跑、零产物）。
    //
    // ⚠ 这一条钉的是**审核那一档真的接上了**：实例侧的 `preflight_refusal` 对这类**故意跳过**
    // （它的措辞是"让用户装/启用节点包"，而前端类装也装不上），所以只有
    // `workflow_audit::blocking_refusal` 接着才算拦得住 —— 没接线时这条图会直接发出去。
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("audit-frontend", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [1, 2, 3]);
    patch_graph(&fixture, |graph| {
        graph["12"] = json!({ "class_type": "Label (rgthree)", "inputs": {} });
        graph["13"] = json!({ "class_type": "Anything Everywhere", "inputs": {} });
    });

    // 覆盖也一起给上：overrides **改不了** class_type，审核看的是覆盖之后的那张图，照样得拦
    let arguments = json!({ "promptId": 101, "overrides": { "3.steps": 30 }, "waitSeconds": 20 });
    let failure = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect_err("纯前端节点必须被审核拦下");
    assert_eq!(failure.code, "MODEL_NOT_LOADABLE", "错误码与 Kotlin 同（`AiWorkflowSearch.submit`）");
    assert!(failure.message.contains("纯前端节点"), "{}", failure.message);
    assert!(failure.message.contains("rgthree"), "{}", failure.message);
    assert!(
        failure.message.contains("没有提交"),
        "被拒的话要说清「没有提交」：{}",
        failure.message
    );
    assert!(submitted.lock().unwrap().is_empty(), "拦下时一个字节都不许发出去");

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn the_audit_warns_but_still_submits_on_the_post_override_graph() {
    // 只带**非阻断**结论的图必须照常提交，而结论要如实带回给模型（`preflightWarnings`）。
    //
    // 这一条同时钉住**审核跑在覆盖之后的那张图上**：库里的 `6.text` 是 "old prompt"
    // （0 个逗号 → 提示词风格那一条**不会**报），只有把覆盖应用完才会变成
    // "给 Krea 2 堆 Danbooru tag" 那条 🟠 —— 若审核跑的是库里的原图，它就报不出来。
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("audit-warn", &base);
    write_png(&fixture.dir.join("output").join("unit.png"), [4, 5, 6]);

    let tags = "masterpiece, best quality, 1girl, solo, long hair, red eyes, smile, looking at viewer, detailed";
    let arguments = json!({
        "promptId": 101,
        "overrides": { "6.text": tags },
        "waitSeconds": 20
    });
    let outcome = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect("非阻断的结论不许拦");

    // 提交照常发生，而且发出去的就是覆盖之后的那张图
    let sent = submitted.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "只许提交一次");
    assert_eq!(sent[0]["prompt"]["6"]["inputs"]["text"], json!(tags), "覆盖要真的生效");

    // 审核的非阻断结论进了 `preflightWarnings`（与"实例看不到模型文件"同一个通道）
    let warnings = outcome.payload["preflightWarnings"].as_array().expect("要带回警告");
    let joined = warnings.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("没有拦"), "{joined}");
    assert!(joined.contains("Danbooru tag"), "提示词风格那一条要带回：{joined}");
    assert!(joined.contains("Krea 2"), "{joined}");
    assert!(joined.contains("🟠"), "非阻断的项用 🟠 标：{joined}");
    assert!(!joined.contains("🔴"), "没有阻断项时不许出现 🔴：{joined}");
    // 库里那张图的 steps=20 / cfg=7.5 本来就偏出 Krea 2 Turbo 档（4~8 / 0~1.5）→ 参数那一条也报
    assert!(joined.contains("参数与"), "参数档位那一条要带回：{joined}");

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn timeout_is_reported_as_timeout_not_failure() {
    // 假实例的 /history 永远空（运行还在队列里）—— 提交方必须如实报 timeout
    let (base, submitted, gateway) = fake_comfy(object_info(), json!({})).await;
    let fixture = Fixture::new("timeout", &base);

    let arguments = json!({ "promptId": 101, "wait": true, "waitSeconds": 1 });
    let outcome = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect("超时不是失败");
    assert_eq!(submitted.lock().unwrap().len(), 1, "提交是发生了的");
    assert_eq!(outcome.payload["status"], json!("timeout"), "{}", outcome.payload);
    assert_eq!(outcome.payload["mediaIds"], json!([]));
    assert!(outcome.payload["hint"].as_str().unwrap().contains("还没跑完"), "{}", outcome.payload);
    let recorded = fixture.state.submissions.find("comfy-run-1").unwrap();
    assert_eq!(recorded["status"], json!("timeout"));

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn without_an_api_graph_it_refuses_instead_of_guessing() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("no-graph", &base);
    // 把 source_ref 抹掉 = 老数据／导入项那种"没有 API 图"
    fixture
        .state
        .write(|conn| {
            conn.execute("UPDATE prompts SET source_ref = NULL WHERE id = 101", [])?;
            Ok(())
        })
        .unwrap();

    let arguments = json!({ "promptId": 101, "waitSeconds": 20 });
    let failure = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &arguments)
        .await
        .expect_err("没有 API 图就不能猜着提交");
    assert_eq!(failure.code, "NO_API_GRAPH");
    assert!(failure.message.contains("没有 API 格式节点图"), "{}", failure.message);
    assert!(submitted.lock().unwrap().is_empty());

    gateway.abort();
    fixture.finish();
}

#[tokio::test]
async fn missing_arguments_are_reported_as_invalid() {
    let (base, _submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("args", &base);

    let failure = comfyhub_server_rs::workflow_submit::tool_submit(&fixture.state, &fixture.policy(), &json!({}))
        .await
        .expect_err("既没 promptId 也没 workflowPath");
    assert_eq!(failure.code, "INVALID_ARGUMENT");
    assert!(failure.message.contains("promptId"), "{}", failure.message);

    // 覆盖里写了一个不存在的输入 → INVALID_ARGUMENT 且说得清有哪些
    let failure = comfyhub_server_rs::workflow_submit::tool_submit(
        &fixture.state,
        &fixture.policy(),
        &json!({ "promptId": 101, "overrides": { "3.nope": 1 } }),
    )
    .await
    .expect_err("不存在的输入要拒");
    assert_eq!(failure.code, "INVALID_ARGUMENT");
    assert!(failure.message.contains("没有名为「nope」的输入"), "{}", failure.message);

    gateway.abort();
    fixture.finish();
}
