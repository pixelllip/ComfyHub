//! **M5：手机排队 → 桌面真正提交**（契约 🔒 `docs/sync-api-contract.md` §4.1）的回归防线。
//!
//! 与 `submit_test.rs` 同一套路：**进程内的假 ComfyUI（真 socket）+ 真 SQLite + 真入库链路**，
//! 不出网、不装 ComfyUI。区别是这里的入口是 `POST /api/sync/push` 的那条指令
//! （`kind = "submit_job"`），而不是模型调工具。
//!
//! # 六条断言对应任务书的六条硬约束
//!
//! | 用例 | 钉住的东西 |
//! | --- | --- |
//! | `no_approval_means_zero_requests_and_a_rejected_reply` | **没有人批准 → 一个上游请求都不发**，而且**照样回一条**说清"超时按拒绝" |
//! | `approval_then_the_existing_submit_chain_ingests_and_reports_media_ids` | 批准 → 真的走完 R10 那条链路 → 产物入库 → 回执里的 `mediaIds` **是真的** |
//! | `the_same_queue_id_pushed_twice_submits_once` | 幂等：同一条推两次**只提交一次**（手机重试是常态） |
//! | `a_payload_missing_fields_is_rejected_permanently` | 缺字段 → 逐条 `rejected` + **永久性**错误码（手机据此丢掉 outbox） |
//! | `the_approval_is_visible_before_anyone_can_click_it` | **R8 的顺序**：先能在待审批列表里看见，再点批准（手快也不落空） |
//! | `a_missing_conversation_keeps_the_queue_and_delivers_later` | 会话还没同步上来 → **保留队列**、如实记错误，会话到了再补发（别删、别假装成功） |
//! | `the_http_route_and_the_existing_approval_endpoint_are_the_only_channel` | 走**真的 HTTP 路由**：push 受理 → `/api/ai/pending` 看得见 → **现有批准端点**（`/api/ai/tool-calls/{id}/approve\|deny`）能改到它（"不另造审批通道"） |
//! | `an_old_database_gets_the_submit_jobs_table` | 加表**同时 bump `SCHEMA_VERSION`**（总纲 §21 那条静默故障） |
//!
//! ⚠ 这里刻意**不**用 `run_job(默认 300 秒)` 去等审批：集成用例里要么自己给一个短的
//! 审批窗口（第 1 条），要么先把批准落了再等终态（其余几条，`accept` 起的那条后台任务
//! 本来就会看到批准）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State as AxumState};
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use comfyhub_server_rs::mobile::submit_queue::{self, Job};
use comfyhub_server_rs::mobile::sync::{PushItem, PushRequest};
use comfyhub_server_rs::web::routes::AppState;

// ---------------------------------------------------------------------------
//  假 ComfyUI（与 `submit_test.rs` 同一套路：真 socket、真 /prompt、真 /history）
// ---------------------------------------------------------------------------

fn api_graph() -> Value {
    json!({
        "3": { "class_type": "KSampler", "inputs": { "seed": 42, "steps": 20, "cfg": 7.5, "denoise": 1.0, "model": ["4", 0] } },
        "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "krea2.safetensors" } },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "old prompt", "clip": ["4", 1] } },
        "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "unit", "images": ["3", 0] } }
    })
}

fn object_info() -> Value {
    json!({
        "KSampler": { "input": { "required": { "seed": ["INT", {}], "steps": ["INT", {}], "cfg": ["FLOAT", {}] } } },
        "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [["krea2.safetensors"], {}] } } },
        "CLIPTextEncode": { "input": { "required": { "text": ["STRING", {}] } } },
        "SaveImage": { "input": { "required": { "filename_prefix": ["STRING", {}] } } }
    })
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

#[derive(Clone)]
struct FakeState {
    object_info: Value,
    history: Value,
    submitted: Arc<Mutex<Vec<Value>>>,
}

/// 起一台进程内的假 ComfyUI，返回 `(base url, 提交过的 body 列表, JoinHandle)`。
async fn fake_comfy(
    object_info: Value,
    history: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let submitted: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let shared = FakeState {
        object_info,
        history,
        submitted: submitted.clone(),
    };

    let app = Router::new()
        .route(
            "/object_info",
            get(|AxumState(s): AxumState<FakeState>| async move { Json(s.object_info) }),
        )
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
                    let Some(template) = s.history.get(&id) else {
                        return Json(json!({}));
                    };
                    // 与真 ComfyUI 同一条纪律：`/history` 回的是**当时提交的那张图**
                    let mut entry = template.clone();
                    if let Some(graph) = s
                        .submitted
                        .lock()
                        .unwrap()
                        .first()
                        .and_then(|body| body.get("prompt"))
                    {
                        entry["prompt"]["prompt"] = graph.clone();
                    }
                    Json(json!({ id: entry }))
                },
            ),
        )
        .route(
            "/queue",
            get(|| async { Json(json!({ "queue_running": [], "queue_pending": [] })) }),
        )
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 ComfyUI");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), submitted, handle)
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = comfyhub_server_rs::infra::testpaths::scratch(&format!("submit-queue-{label}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_png(path: &std::path::Path, color: [u8; 3]) {
    let mut image = image::RgbImage::new(12, 12);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb(color);
    }
    image.save(path).expect("写 PNG");
}

/// 夹具：真 DDL 的内存库 + 一条带 API 图的提示词（`promptId = 101`，`sourceRef = unit-run-1`）
/// + 一条会话（`c1`）+ 恒定的捕获配置。
struct Fixture {
    state: AppState,
    dir: std::path::PathBuf,
}

impl Fixture {
    /// `with_conversation = false`：会话**还没上行到桌面**（第 6 条用例要这个现场）。
    fn new(label: &str, config_url: &str, with_conversation: bool) -> Self {
        let dir = temp_dir(label);
        let output = dir.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let storage = dir.join("storage");
        std::fs::create_dir_all(storage.join("media")).unwrap();

        let conn = comfyhub_server_rs::infra::db::open_memory().expect("内存库");
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql");
        let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
        conn.execute_batch(&schema).expect("建 schema");

        if with_conversation {
            conn.execute(
                "INSERT INTO ai_conversations (id, title) VALUES ('c1','手机那条会话')",
                [],
            )
            .expect("插会话");
        }
        let raw = json!({
            "prompt": { "prompt": api_graph(), "extra_data": { "client_id": "fixture" } },
            "outputs": { "9": { "images": [{ "filename": "unit.png", "subfolder": "", "type": "output" }] } },
            "status": { "status_str": "success", "completed": true, "messages": [] }
        });
        conn.execute(
            "INSERT INTO prompts (id, title, kind, positive_prompt, source_ref, created_at, updated_at) \
             VALUES (101, 'unit 排队提交', 'IMAGE', 'a cat, masterpiece', 'unit-run-1', \
                     '2026-09-27T00:00:00.000Z', '2026-09-27T00:00:00.000Z')",
            [],
        )
        .expect("插提示词");
        conn.execute(
            "INSERT INTO capture_runs (run_key, source, status, raw, created_at) \
             VALUES ('unit-run-1', 'ComfyUI', 'success', ?1, '2026-09-27T00:00:00.000Z')",
            rusqlite::params![raw.to_string()],
        )
        .expect("插捕获记录");

        let state = AppState::new(conn, ":memory:", storage.join("media"));
        let config = comfyhub_server_rs::comfy::capture::CaptureConfig {
            enabled: true,
            comfy_url: config_url.to_string(),
            output_dir: Some(output.display().to_string()),
            poll_seconds: 4,
            auto_tag: "ComfyUI".to_string(),
            max_per_poll: 20,
            download_fallback: true,
        };
        comfyhub_server_rs::comfy::capture::save_capture_config(&state, config)
            .expect("存捕获配置");
        Self { state, dir }
    }

    fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
//  小工具
// ---------------------------------------------------------------------------

fn submit_job_item(queue_id: &str, payload: Value) -> PushItem {
    PushItem {
        kind: "submit_job".to_string(),
        ref_id: queue_id.to_string(),
        op: Some("upsert".to_string()),
        payload,
    }
}

/// 走**真的** `POST /api/sync/push` 那条路（不是直接调 `accept`）——
/// 任务书第 1 条要钉的正是"`submit_job` 在 push 里被认出来"。
fn push_one(state: &AppState, item: PushItem) -> comfyhub_server_rs::mobile::sync::PushResult {
    comfyhub_server_rs::mobile::sync::push(
        state,
        PushRequest {
            device_id: Some("phone-test".to_string()),
            batch: vec![item],
        },
    )
    .expect("push 不该整批失败")
}

/// 受理之后 payload 里那条 `queueId`（= refId）。
fn job(state: &AppState, queue_id: &str) -> Job {
    submit_queue::load(state, queue_id).expect("这一条排队提交应当还在账上")
}

fn is_terminal(job: &Job) -> bool {
    !matches!(
        job.state.as_str(),
        submit_queue::STATE_AWAITING_APPROVAL | submit_queue::STATE_SUBMITTING
    )
}

/// 等它走到终态（**不预设是哪条路收的尾**：`accept` 起的那条后台任务与用例自己调的那次
/// 是**同一个** worker 实现，谁先认领谁干活 —— 认领是原子的，所以只会提交一次）。
async fn wait_terminal(state: &AppState, queue_id: &str, seconds: u64) -> Job {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let current = job(state, queue_id);
        if is_terminal(&current) {
            return current;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "等终态超时：{current:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 待审批列表里的那条（`GET /api/ai/pending` 的 `approvals[]`）。
fn pending_approval(state: &AppState, call_id: &str) -> Option<Value> {
    let body = comfyhub_server_rs::ai::pending::pending(state).expect("读待办");
    body["approvals"]
        .as_array()
        .expect("approvals 一定是数组")
        .iter()
        .find(|row| row["callId"] == json!(call_id))
        .cloned()
}

fn approve(state: &AppState, call_id: &str) -> bool {
    comfyhub_server_rs::ai::runs::resolve_approval(state, call_id, true).expect("批准")
}

/// 库里**全部**产物的 id（夹具库里本来一张都没有，所以"全部"就是"这一次收进来的"）。
fn all_media_ids(state: &AppState) -> Vec<i64> {
    state
        .with(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM media_assets ORDER BY id")?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap()
}

// ---------------------------------------------------------------------------
//  ① 没有批准 → 一个上游请求都不发，而且照样回一条"被拒"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_approval_means_zero_requests_and_a_rejected_reply() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("no-approval", &base, true);
    write_png(&fixture.dir.join("output").join("unit.png"), [10, 200, 30]);

    let result = push_one(
        &fixture.state,
        submit_job_item(
            "q-nobody-approves",
            json!({ "queueId": "q-nobody-approves", "conversationId": "c1", "runId": "phone-run-1", "promptId": 101 }),
        ),
    );
    assert_eq!(
        result.applied,
        vec!["q-nobody-approves".to_string()],
        "{:?}",
        result.rejected
    );
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);

    // 受理是"立刻回 applied"的（不许被审批挡住）——此刻还在等人批
    assert_eq!(
        job(&fixture.state, "q-nobody-approves").state,
        submit_queue::STATE_AWAITING_APPROVAL
    );
    assert!(
        submitted.lock().unwrap().is_empty(),
        "还没人批准，一个上游请求都不该发"
    );

    // 没人点 + 审批窗口很短（用例自己给它 400ms；生产是 `APPROVAL_TIMEOUT_SECONDS`）
    let report = submit_queue::run_job(
        &fixture.state,
        "q-nobody-approves",
        Duration::from_millis(400),
    )
    .await;
    assert_eq!(report, submit_queue::JobReport::NotWaiting);
    let finished = wait_terminal(&fixture.state, "q-nobody-approves", 5).await;
    assert_eq!(finished.state, submit_queue::STATE_DENIED);
    assert_eq!(finished.approval, "denied");
    assert_eq!(finished.outcome_code.as_deref(), Some("APPROVAL_TIMEOUT"));
    assert!(
        submitted.lock().unwrap().is_empty(),
        "超时同样一个字节都不许发出去"
    );

    // ★ **失败也要回一条**（手机上那条"已排队"永远停在排队中是最坏的表现）
    let message =
        &comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "c1").unwrap()[0];
    assert_eq!(message.role, "assistant");
    assert!(message.text.contains("没有提交"), "{}", message.text);
    let result_part = message
        .parts
        .iter()
        .find(|part| part.r#type == "tool_result")
        .expect("要有 tool_result 块");
    let payload = result_part.json_payload.as_ref().unwrap();
    assert_eq!(payload["ok"], json!(false));
    assert_eq!(payload["code"], json!("APPROVAL_TIMEOUT"));
    assert_eq!(payload["mediaIds"], json!([]), "被拒就不该有任何产物 id");

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ② 批准 → 真的提交 → 产物入库 → 回执里的 mediaIds 是真的
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approval_then_the_existing_submit_chain_ingests_and_reports_media_ids() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("approved", &base, true);
    write_png(&fixture.dir.join("output").join("unit.png"), [20, 30, 200]);

    let queue_id = "q-approved";
    push_one(
        &fixture.state,
        submit_job_item(
            queue_id,
            json!({
                "queueId": queue_id,
                "conversationId": "c1",
                "runId": "phone-run-2",
                "promptId": 101,
                "overrides": { "3.steps": "30", "6.text": "a dog" },
                "nsfw": false,
                "requestedBy": "phone-test"
            }),
        ),
    );

    // 先看得见 pending，再点批准（R8 的顺序，第 5 条用例专门钉它）
    let call_id = submit_queue::call_id_for(queue_id);
    assert!(
        pending_approval(&fixture.state, &call_id).is_some(),
        "批准之前必须能看见它"
    );
    assert!(approve(&fixture.state, &call_id), "手快点了也必须生效");

    // 等它真的跑完（`run_job` 与后台任务同一个实现，谁认领谁干）
    let finished = wait_terminal(&fixture.state, queue_id, 30).await;
    assert_eq!(finished.state, submit_queue::STATE_DONE, "{finished:?}");

    // ① 提交出去的就是覆盖之后的那张图（R10 那条链路真的走了）
    let sent = submitted.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "只许提交一次");
    assert_eq!(
        sent[0]["prompt"]["3"]["inputs"]["steps"],
        json!(30),
        "覆盖要生效"
    );
    assert_eq!(sent[0]["prompt"]["6"]["inputs"]["text"], json!("a dog"));

    // ② `mediaIds` 是真的：与库里那批产物**同一个 id**（夹具库里本来一张产物都没有）
    assert!(!finished.media_ids.is_empty(), "产物应当入库：{finished:?}");
    let in_library = all_media_ids(&fixture.state);
    assert_eq!(
        finished.media_ids, in_library,
        "回执里的 id 必须就是画廊里那几条"
    );

    // ③ 回执：一条助手消息 + 一个 tool_result 块（带 mediaIds）
    let messages =
        comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "c1").unwrap();
    assert_eq!(messages.len(), 1, "回执只许有一条");
    let message = &messages[0];
    assert_eq!(message.role, "assistant");
    assert!(message.text.contains("入库"), "{}", message.text);
    let result_part = message
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .expect("tool_result 块");
    let payload = result_part.json_payload.as_ref().unwrap();
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(payload["name"], json!("comfy_submit"));
    assert_eq!(payload["approval"], json!("approved"));
    assert_eq!(payload["mediaIds"], json!(finished.media_ids));
    // 工具结果块里的 id 与画廊里那批是同一批（界面靠它贴「画廊入口卡」）
    assert_eq!(payload["mediaIds"], json!(in_library));

    // ④ 回执只送一次：再投递一轮不会冒出第二条（已经送达的**不进**待投递清单）
    assert_eq!(
        submit_queue::deliver_pending(&fixture.state),
        0,
        "已送达的不用再管"
    );
    assert_eq!(
        comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "c1")
            .unwrap()
            .len(),
        1
    );

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ③ 同一个 queueId 推两次 → 只提交一次
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_queue_id_pushed_twice_submits_once() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("idempotent", &base, true);
    write_png(&fixture.dir.join("output").join("unit.png"), [200, 20, 60]);

    let queue_id = "q-twice";
    let payload = json!({ "queueId": queue_id, "conversationId": "c1", "promptId": 101 });
    let first = push_one(&fixture.state, submit_job_item(queue_id, payload.clone()));
    let second = push_one(&fixture.state, submit_job_item(queue_id, payload));
    // 两次都要回 `applied`（手机据此把那行 outbox 删掉），但**只受理一次**
    assert_eq!(first.applied, vec![queue_id.to_string()]);
    assert_eq!(second.applied, vec![queue_id.to_string()]);
    assert!(second.rejected.is_empty(), "{:?}", second.rejected);
    let rows: i64 = fixture
        .state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM submit_jobs", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(rows, 1, "同一条推两次只许有一行账");

    assert!(approve(
        &fixture.state,
        &submit_queue::call_id_for(queue_id)
    ));
    let finished = wait_terminal(&fixture.state, queue_id, 30).await;
    assert_eq!(finished.state, submit_queue::STATE_DONE, "{finished:?}");

    assert_eq!(
        submitted.lock().unwrap().len(),
        1,
        "**只许提交一次**（重复提交 = 白烧一轮显卡）"
    );
    let messages =
        comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "c1").unwrap();
    assert_eq!(messages.len(), 1, "回执也只许有一条");

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ④ payload 缺字段 → 永久性 rejected（手机据此丢掉那行 outbox）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_payload_missing_fields_is_rejected_permanently() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("rejected", &base, true);

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "q-no-conv",
            json!({ "queueId": "q-no-conv", "promptId": 101 }),
            "MISSING_CONVERSATION",
        ),
        (
            "q-no-workflow",
            json!({ "queueId": "q-no-workflow", "conversationId": "c1" }),
            "MISSING_WORKFLOW",
        ),
        (
            "q-bad-ref",
            json!({ "queueId": "q-bad-ref", "conversationId": "c1", "sourceRef": "file:deadbeef" }),
            "UNKNOWN_SOURCE_REF",
        ),
    ];
    for (queue_id, payload, expected) in cases {
        let result = push_one(&fixture.state, submit_job_item(queue_id, payload));
        assert!(
            result.applied.is_empty(),
            "形状错的 payload 不许受理：{:?}",
            result.applied
        );
        assert_eq!(result.rejected.len(), 1);
        let rejected = &result.rejected[0];
        assert_eq!(rejected.ref_id, queue_id);
        assert_eq!(rejected.code, expected, "{}", rejected.message);
        assert!(!rejected.message.trim().is_empty(), "must explain why");
        // 手机（`SyncPushClient.isPermanentReject`）只把 `DB_ERROR` 当暂时性失败 ——
        // 这几个码必须是别的值，否则手机会永远重试一条永远不可能成功的行。
        assert_ne!(
            rejected.code, "DB_ERROR",
            "这几个都是**永久性**的：重推一次结果不会变"
        );
    }

    // 一条都没落进账里（不留半截状态），也一个上游请求都没发
    let rows: i64 = fixture
        .state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM submit_jobs", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(rows, 0);
    assert!(submitted.lock().unwrap().is_empty());
    assert!(
        comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "c1")
            .unwrap()
            .is_empty()
    );

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ⑤ 审批顺序（R8 的老坑）：**先能看见 pending，再批准**
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_approval_is_visible_before_anyone_can_click_it() {
    let (base, _submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("visible", &base, true);

    let queue_id = "q-visible";
    let call_id = submit_queue::call_id_for(queue_id);
    // 推上来**之前**：列表里当然没有它
    assert!(pending_approval(&fixture.state, &call_id).is_none());

    push_one(
        &fixture.state,
        submit_job_item(
            queue_id,
            json!({
                "queueId": queue_id,
                "conversationId": "c1",
                "runId": "phone-run-9",
                "promptId": 101,
                "overrides": { "6.text": "a fox" }
            }),
        ),
    );

    // ★ push 一回来（`applied`）它就**必须**已经挂在待审批列表上了：
    //   手机要先拿到 `applied`、用户才可能在桌面上点 —— 顺序反过来就是 R8 那个坑
    let listed = pending_approval(&fixture.state, &call_id).expect("受理之后必须立刻看得见");
    assert_eq!(listed["name"], json!("comfy_submit"));
    assert_eq!(listed["conversationId"], json!("c1"));
    assert_eq!(listed["runId"], json!("phone-run-9"));
    assert!(
        listed["askedAt"].is_string(),
        "askedAt 要按 ISO 文本给：{listed}"
    );
    let arguments: Value = serde_json::from_str(listed["argumentsJson"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["promptId"], json!(101));
    assert_eq!(
        arguments["overrides"]["6.text"],
        json!("a fox"),
        "批准前要看得见到底要提交什么"
    );

    // 手快先点：闸门已经开着，**必须**收下（返回 false 就是"那一次点击落空"）
    assert!(
        approve(&fixture.state, &call_id),
        "先看得见再批准 —— 点击不许落空"
    );
    // 决定只能下一次：再点（哪怕改成拒绝）都不生效
    assert!(!approve(&fixture.state, &call_id));
    assert!(
        !comfyhub_server_rs::ai::runs::resolve_approval(&fixture.state, &call_id, false).unwrap()
    );
    assert_eq!(job(&fixture.state, queue_id).approval, "approved");
    assert!(
        pending_approval(&fixture.state, &call_id).is_none(),
        "批过的就该从待办里消失"
    );

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ⑥ 会话还没同步上来 → 保留队列 + 如实记错误，会话到了再补发
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_missing_conversation_keeps_the_queue_and_delivers_later() {
    let (base, _submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    // ⚠ 会话**不在**桌面上（手机那条会话还没上行）——但工作流是有的，提交照做
    let fixture = Fixture::new("late-conversation", &base, false);
    write_png(&fixture.dir.join("output").join("unit.png"), [90, 90, 10]);

    let queue_id = "q-late";
    push_one(
        &fixture.state,
        submit_job_item(
            queue_id,
            json!({ "queueId": queue_id, "conversationId": "late", "promptId": 101 }),
        ),
    );
    assert!(approve(
        &fixture.state,
        &submit_queue::call_id_for(queue_id)
    ));
    let finished = wait_terminal(&fixture.state, queue_id, 30).await;
    assert_eq!(finished.state, submit_queue::STATE_DONE, "{finished:?}");

    // 提交是做了的（产物入库），只是**回执送不出去**
    assert!(!finished.media_ids.is_empty());
    let blocked = job(&fixture.state, queue_id);
    assert_eq!(blocked.reply_state, "blocked", "如实记一笔，不是假装成功");
    assert!(
        blocked
            .reply_error
            .as_deref()
            .unwrap_or_default()
            .contains("还没同步到桌面"),
        "{:?}",
        blocked.reply_error
    );
    // **保留队列**：那一行还在（不许删、不许假装成功）
    assert_eq!(job(&fixture.state, queue_id).queue_id, queue_id);

    // 手机把那条会话上行上来 → 下一轮投递补发（不需要人工再做什么）
    fixture
        .state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_conversations (id, title) VALUES ('late','手机那条会话')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        submit_queue::deliver_pending(&fixture.state),
        1,
        "会话到了就该补发"
    );
    let messages =
        comfyhub_server_rs::ai::conversations::list_messages(&fixture.state, "late").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    let payload = messages[0]
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .and_then(|p| p.json_payload.clone())
        .unwrap();
    assert_eq!(
        payload["mediaIds"],
        json!(finished.media_ids),
        "补发的回执里 mediaIds 要是真的"
    );
    assert_eq!(job(&fixture.state, queue_id).reply_state, "delivered");

    gateway.abort();
    fixture.finish();
}

// ---------------------------------------------------------------------------
//  ⑦ 老库升级：`submit_jobs` 必须真的到达已有的库（总纲 §21 那条静默故障）
// ---------------------------------------------------------------------------

/// 加表**同时 bump `SCHEMA_VERSION`** 的回归防线。
///
/// 不 bump 的表现是**静默的**：老库停在旧版本号 → `open_or_create` 直接早退 →
/// `submit_jobs` 永远不存在 → 第一次手机上推 `submit_job` 就 `no such table`
/// （而且炸的是手机那条路，桌面上不看日志根本发现不了）。
///
/// 这里造一个"版本停在 3、表也没建"的库（就是老库的样子），再 `open_or_create` 一次：
/// 表要补上、版本要走到 4。
#[test]
fn an_old_database_gets_the_submit_jobs_table() {
    let dir = temp_dir("upgrade");
    let path = dir.join("comfyhub.db");
    {
        let conn = comfyhub_server_rs::infra::db::open_or_create(&path).expect("第一次建库");
        // 把现场退回到"加这张表之前"：表删掉、版本号也退回 3
        conn.execute_batch("DROP TABLE submit_jobs; PRAGMA user_version = 3;")
            .expect("退回老库现场");
    }
    let conn = comfyhub_server_rs::infra::db::open_or_create(&path).expect("老库要能被收敛");
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='submit_jobs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        exists, 1,
        "老库必须补上 submit_jobs（加表忘了 bump 版本就是这条静默故障）"
    );
    assert_eq!(
        comfyhub_server_rs::infra::db::schema_version(&conn).unwrap(),
        comfyhub_server_rs::infra::db::SCHEMA_VERSION,
        "版本号要跟着走到 4，否则下一次又会被当成老库"
    );
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
//  ⑧ 真的 HTTP 路由 + **现有批准端点**（"不另造审批通道"的落点）
// ---------------------------------------------------------------------------

async fn send(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(body.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("读 body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// 手机那条路的**端到端形状**：`POST /api/sync/push`（真路由）→ `GET /api/ai/pending`
/// （真的看得见）→ `POST /api/ai/tool-calls/{callId}/approve|deny`（**现有端点**，
/// 契约 §4.1 要求"不要另造一条审批通道"）。
#[tokio::test]
async fn the_http_route_and_the_existing_approval_endpoint_are_the_only_channel() {
    let (base, submitted, gateway) = fake_comfy(object_info(), history_with_output()).await;
    let fixture = Fixture::new("http", &base, true);
    write_png(&fixture.dir.join("output").join("unit.png"), [7, 7, 7]);

    // ① push（真路由）：受理 = `applied` + 立刻回，**不等审批也不等生成**
    let (status, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "POST",
        "/api/sync/push",
        Some(json!({
            "deviceId": "phone-http",
            "batch": [{
                "kind": "submit_job",
                "refId": "q-http",
                "op": "upsert",
                "payload": { "queueId": "q-http", "conversationId": "c1", "runId": "phone-run-http", "promptId": 101 }
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], json!(["q-http"]));
    assert!(body["rejected"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(
        job(&fixture.state, "q-http").state,
        submit_queue::STATE_AWAITING_APPROVAL
    );

    // ② 桌面上看得见（`/api/ai/pending` 的 approvals[]，形状与工具审批同形）
    let (status, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "GET",
        "/api/ai/pending",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = body["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["callId"] == json!("submit:q-http"))
        .cloned()
        .expect("受理之后它必须出现在待审批列表里");
    assert_eq!(listed["name"], json!("comfy_submit"));

    // ③ **现有的批准端点**（一个字都没为 M5 新加）→ 真的改到了那条排队提交
    let (status, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "POST",
        "/api/ai/tool-calls/submit:q-http/approve",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["accepted"],
        json!(true),
        "闸门早就在库里，手快点了也必须生效：{body}"
    );
    assert_eq!(job(&fixture.state, "q-http").approval, "approved");

    // ④ 再说"拒绝"就晚了（决定只能下一次）——同一个端点、同一把锁
    let (status, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "POST",
        "/api/ai/tool-calls/submit:q-http/deny",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accepted"], json!(false), "已经批过的不许撤：{body}");

    // ⑤ 另一条走拒绝：`/api/ai/pending` 上立刻消失，而且**一个上游请求都不会发**
    let (_, _) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "POST",
        "/api/sync/push",
        Some(json!({
            "deviceId": "phone-http",
            "batch": [{
                "kind": "submit_job", "refId": "q-http-deny", "op": "upsert",
                "payload": { "queueId": "q-http-deny", "conversationId": "c1", "promptId": 101 }
            }]
        })),
    )
    .await;
    let (status, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "POST",
        "/api/ai/tool-calls/submit:q-http-deny/deny",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accepted"], json!(true));
    let denied = wait_terminal(&fixture.state, "q-http-deny", 10).await;
    assert_eq!(denied.state, submit_queue::STATE_DENIED);
    assert_eq!(denied.outcome_code.as_deref(), Some("APPROVAL_DENIED"));
    assert!(
        submitted.lock().unwrap().is_empty(),
        "被拒的那条一个字节都不许发"
    );

    let (_, body) = send(
        comfyhub_server_rs::web::routes::router(fixture.state.clone()),
        "GET",
        "/api/ai/pending",
        None,
    )
    .await;
    assert!(
        body["approvals"].as_array().unwrap().is_empty(),
        "批 / 拒过的不该再挂在待办里：{body}"
    );

    gateway.abort();
    fixture.finish();
}
