//! Provider **连接测试 / 模型发现**（A2）的回归防线 —— 一台进程内的假上游 + 真 SQLite。
//!
//! # 这一组在防什么
//!
//! 设置页那两个按钮是**添加一条 Provider 后的第一件事**：
//! "这个 Base URL + 这把 Key 到底能不能用"、"有哪些模型可以选"。
//! 三件事出错都不会报错，只会导致误判：
//!  1. **`/models` 回 404 被判成鉴权失败** → 会误认为 Key 填错，实际是该网关没有列表端点；
//!  2. **Base URL 带不带 `/v1` 只试一个地址** → 明明能用，界面却说"连接失败"；
//!  3. **候选能力凭印象给出** → 保存后模型"看起来支持图片"实际不支持（或相反）。
//!
//! 所以这里用**真 socket**（axum 起的假上游）把三种回退都走一遍，并盯住鉴权头到底发没发。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

use comfyhub_server_rs::web::routes::{router, AppState};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = comfyhub_server_rs::infra::testpaths::scratch(&format!("provider-probe-{label}"));
    std::fs::create_dir_all(dir.join("storage").join("media")).unwrap();
    dir
}

/// 一个 provider + 一条会话（端点只用到 provider）。
fn state(dir: &std::path::Path, base_url: &str, credential_ref: Option<&str>) -> AppState {
    let conn = Connection::open_in_memory().unwrap();
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
    conn.execute_batch(&schema).expect("建 schema");
    conn.execute(
        "INSERT INTO ai_providers (id, display_name, api, base_url, credential_ref, \
           endpoint_trust, enabled, revision) \
         VALUES ('p1', '假上游', 'openai-completions', ?1, ?2, 'loopback', 1, 1)",
        rusqlite::params![base_url, credential_ref],
    )
    .unwrap();
    AppState::new(conn, ":memory:", dir.join("storage").join("media"))
}

#[derive(Clone)]
struct Upstream {
    /// `/models` 的状态码（200 / 401 / 404）
    status: u16,
    body: String,
    /// 收到的 Authorization 头（记下来断言"该发才发"）
    seen_auth: Arc<Mutex<Vec<String>>>,
    seen_paths: Arc<Mutex<Vec<String>>>,
}

/// 起一台假上游：`/models` 与 `/v1/models` 都给同一个答案（两个候选都会被打到）。
async fn upstream(
    status: u16,
    body: Value,
) -> (
    String,
    Arc<Mutex<Vec<String>>>,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let shared = Upstream {
        status,
        body: body.to_string(),
        seen_auth: Arc::new(Mutex::new(Vec::new())),
        seen_paths: Arc::new(Mutex::new(Vec::new())),
    };
    let auth = shared.seen_auth.clone();
    let paths = shared.seen_paths.clone();

    let app = Router::new()
        .route(
            "/models",
            get(
                |AxumState(s): AxumState<Upstream>,
                 axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
                 headers: axum::http::HeaderMap| async move {
                    handle(s, uri.path(), headers)
                },
            ),
        )
        .route(
            "/v1/models",
            get(
                |AxumState(s): AxumState<Upstream>,
                 axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
                 headers: axum::http::HeaderMap| async move {
                    handle(s, uri.path(), headers)
                },
            ),
        )
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), auth, paths, handle)
}

fn handle(state: Upstream, path: &str, headers: axum::http::HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    state.seen_paths.lock().unwrap().push(path.to_string());
    state.seen_auth.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    );
    let status = StatusCode::from_u16(state.status).unwrap();
    (status, state.body.clone()).into_response()
}

/// 300 条候选的假列表（只有第一条声明了能力，第二条什么都没有）。
fn models_body() -> Value {
    json!({
        "data": [
            {
                "id": "some-vision-model",
                "display_name": "看图模型",
                "input_modalities": ["text", "image"],
                "context_window": 200000,
                "max_output_tokens": 8192,
                "supports_tools": true
            },
            { "id": "完全不认识的模型-xyz" }
        ]
    })
}

async fn post(state: &AppState, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn 测试连通_把模型个数的候选与地址都如实回报() {
    let dir = temp_dir("test-ok");
    let (base, auth, paths, gateway) = upstream(200, models_body()).await;
    let state = state(&dir, &base, None);

    let (status, body) = post(&state, "/api/ai/providers/p1/test").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");
    assert_eq!(body["modelCount"], json!(2), "{body}");
    assert_eq!(body["httpStatus"], json!(200));
    assert!(
        body["message"].as_str().unwrap().contains("连接正常"),
        "{body}"
    );
    assert!(!paths.lock().unwrap().is_empty(), "真的打到了假上游");
    // 没有 credentialRef → **一个鉴权头都不该发**
    assert!(
        auth.lock().unwrap().iter().all(|value| value.is_empty()),
        "没配凭据时不许发 Authorization：{:?}",
        auth.lock().unwrap()
    );

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn base_url_带与不带_v1_都要能试通() {
    let dir = temp_dir("test-v1");
    // 只有 `/v1/models` 通：假上游对 `/models` 回 404、对 `/v1/models` 回 200
    #[derive(Clone)]
    struct Split;
    let app = Router::new()
        .route("/models", get(|| async { (StatusCode::NOT_FOUND, "{}") }))
        .route("/v1/models", get(|| async { Json(models_body()) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let gateway = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = format!("http://{addr}");
    let state = state(&dir, &base, None);

    // Base URL 不带 /v1 → 首选 `/models` 404 → 回退到 `/v1/models` 200
    let (status, body) = post(&state, "/api/ai/providers/p1/test").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "回退之后应当连通：{body}");
    assert_eq!(body["modelCount"], json!(2));
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("实际可用地址"),
        "要说清实际用的是哪个地址：{message}"
    );

    let _ = std::mem::size_of::<Split>();
    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 两个候选都只有_404_时算连通_但不能算鉴权失败() {
    let dir = temp_dir("test-404");
    let (base, _auth, _paths, gateway) = upstream(404, json!({})).await;
    let state = state(&dir, &base, None);

    let (status, body) = post(&state, "/api/ai/providers/p1/test").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "404 只说明没有列表端点：{body}");
    assert!(
        body["message"].as_str().unwrap().contains("不提供模型列表"),
        "{body}"
    );

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 鉴权失败要报_missing_credential_而不是继续试别的地址() {
    let dir = temp_dir("test-401");
    let (base, _auth, _paths, gateway) = upstream(401, json!({"error": "Invalid API key"})).await;
    let state = state(&dir, &base, None);

    let (status, body) = post(&state, "/api/ai/providers/p1/test").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "探测结果本身是 200，业务失败在 ok=false：{body}"
    );
    assert_eq!(body["ok"], json!(false), "{body}");
    assert_eq!(body["errorCode"], json!("MISSING_CREDENTIAL"), "{body}");
    assert_eq!(body["httpStatus"], json!(401));

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 缺凭据时先报凭据缺失_不发任何请求() {
    let dir = temp_dir("test-missing");
    let (base, auth, _paths, gateway) = upstream(200, models_body()).await;
    let state = state(&dir, &base, Some("probe-missing-key"));

    let (status, body) = post(&state, "/api/ai/providers/p1/test").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(false), "{body}");
    assert_eq!(body["errorCode"], json!("MISSING_CREDENTIAL"));
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("probe-missing-key"),
        "{body}"
    );
    assert!(auth.lock().unwrap().is_empty(), "缺凭据时连请求都不该发");

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 模型发现_逐维度合并能力且不落库() {
    let dir = temp_dir("discover");
    let (base, auth, _paths, gateway) = upstream(200, models_body()).await;
    let state = state(&dir, &base, None);

    let (status, body) = post(&state, "/api/ai/providers/p1/discover-models").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");
    let candidates = body["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{body}");

    // 第一条：接口声明的都用接口的
    assert_eq!(candidates[0]["id"], json!("some-vision-model"));
    assert_eq!(candidates[0]["displayName"], json!("看图模型"));
    assert_eq!(candidates[0]["modalities"], json!(["text", "image"]));
    assert_eq!(candidates[0]["tools"], json!(true));
    assert_eq!(candidates[0]["contextWindow"], json!(200000));
    assert_eq!(candidates[0]["maxOutputTokens"], json!(8192));
    assert_eq!(candidates[0]["capabilitySource"], json!("discovered"));

    // 第二条：谁都没说 → 仅文本 + 工具默认给上 + unknown
    assert_eq!(candidates[1]["modalities"], json!(["text"]));
    assert_eq!(candidates[1]["tools"], json!(true));
    assert_eq!(candidates[1]["reasoning"], json!(false));
    assert_eq!(candidates[1]["capabilitySource"], json!("unknown"));

    // **不落库**（AIH-009）：探测结果只是候选，勾选后才保存
    let count: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM ai_models", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 0, "发现不许写目录");

    // 鉴权头：没配凭据就不发（这条与 test 端点同一条纪律）
    assert!(auth.lock().unwrap().iter().all(|v| v.is_empty()));

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn provider_不存在时两条端点都给人话() {
    let dir = temp_dir("missing-provider");
    let (base, _auth, _paths, gateway) = upstream(200, models_body()).await;
    let state = state(&dir, &base, None);
    for path in [
        "/api/ai/providers/nope/test",
        "/api/ai/providers/nope/discover-models",
    ] {
        let (status, body) = post(&state, path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {body}");
        assert!(
            body["error"].as_str().unwrap().contains("Provider 不存在")
                || body["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Provider 不存在"),
            "{path} {body}"
        );
    }
    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}
