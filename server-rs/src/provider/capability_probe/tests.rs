//! `capability_probe` 的单元测试（主体代码在 [`capability_probe`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 假网关：按调用顺序回预设的状态码，并**记下每一次请求的正文**（用例据此断言"发了什么"）。
struct FakeGateway {
    calls: AtomicUsize,
    bodies: Mutex<Vec<String>>,
    /// 收到这个 body 里带的档位时回什么（`None` = 基准请求）
    plan: Box<dyn Fn(Option<&str>) -> (u16, &'static str) + Send + Sync>,
}

impl FakeGateway {
    fn new(plan: impl Fn(Option<&str>) -> (u16, &'static str) + Send + Sync + 'static) -> Self {
        Self { calls: AtomicUsize::new(0), bodies: Mutex::new(Vec::new()), plan: Box::new(plan) }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

impl Transport for FakeGateway {
    async fn post(
        &self,
        _url: &str,
        _api: &str,
        _secret: Option<&str>,
        _trust: &str,
        body: &str,
        _timeout_seconds: u64,
    ) -> ProbeResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.bodies.lock().unwrap().push(body.to_string());
        let effort = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("reasoning_effort").and_then(Value::as_str).map(str::to_string));
        let (status, text) = (self.plan)(effort.as_deref());
        ProbeResponse {
            status: Some(status),
            body: text.to_string(),
            detail: String::new(),
        }
    }
}

fn declared() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("off".to_string(), "none".to_string()),
        ("low".to_string(), "low".to_string()),
        ("high".to_string(), "high".to_string()),
        ("max".to_string(), "max".to_string()),
    ])
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = crate::infra::testpaths::scratch(&format!("probe-{tag}-{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- 极小请求的形状（这是"会不会花钱"的直接判据）----

#[test]
fn 极小请求的形状与_kotlin_一致() {
    let body = probe_body("m", None, "max_tokens");
    let parsed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["model"], "m");
    assert_eq!(parsed["messages"][0]["role"], "user");
    assert_eq!(parsed["messages"][0]["content"], "ping", "极短 prompt（不是把用户的话发上去）");
    assert_eq!(parsed["max_tokens"], 256, "限制输出");
    assert!(parsed.get("reasoning_effort").is_none(), "基准请求不带档位");

    let with = probe_body("m", Some("xhigh"), "max_completion_tokens");
    let parsed: Value = serde_json::from_str(&with).unwrap();
    assert_eq!(parsed["reasoning_effort"], "xhigh");
    assert_eq!(parsed["max_completion_tokens"], 256);
    assert!(parsed.get("max_tokens").is_none(), "换了字段就不许两个都带");
}

#[test]
fn chat_端点两个候选与_kotlin_一致() {
    assert_eq!(
        chat_url_candidates("https://api.x.com/v1"),
        vec![
            "https://api.x.com/v1/chat/completions".to_string(),
            "https://api.x.com/chat/completions".to_string(),
        ]
    );
    assert_eq!(
        chat_url_candidates("https://api.x.com"),
        vec![
            "https://api.x.com/chat/completions".to_string(),
            "https://api.x.com/v1/chat/completions".to_string(),
        ]
    );
    // 尾斜杠收拾干净
    assert_eq!(
        chat_url_candidates("https://api.x.com/v1/")[0],
        "https://api.x.com/v1/chat/completions"
    );
}

#[test]
fn 档位从最高往下试且_off_永远不试() {
    // 声明了等级表：只试表内的，且**从高到低**（写回时"被接受的那档及其以下"全靠这个顺序）
    let rungs = rungs_for(&declared());
    assert_eq!(
        rungs.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        vec!["max", "high", "low"],
        "声明的档位从高到低；off 不是「网关认不认」的问题"
    );
    // 没声明 → 整条阶梯（除 off），同样是**从高到低**
    let all = rungs_for(&BTreeMap::new());
    assert_eq!(
        all.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        vec!["max", "xhigh", "high", "medium", "low", "minimal"],
        "[`EFFORT_ORDER`] 是低→高的声明顺序，探测方向必须**反过来**"
    );
    // 只有 off → 也算"没有可校准的档位"（回整条阶梯，由网关裁决）
    let only_off = rungs_for(&BTreeMap::from([("off".to_string(), "none".to_string())]));
    assert_eq!(only_off.len(), 6);
}

#[test]
fn 阶梯只含被接受的那档及其以下() {
    let rungs = rungs_for(&declared()); // max, high, low
    let ladder = ladder_from(&rungs, 1); // high 被接受
    assert_eq!(ladder.get("off").map(String::as_str), Some("none"));
    assert_eq!(ladder.get("high").map(String::as_str), Some("high"));
    assert_eq!(ladder.get("low").map(String::as_str), Some("low"));
    assert!(!ladder.contains_key("max"), "被拒的高档绝不写回：{ladder:?}");
}

#[test]
fn 与思考参数无关的_4xx_判据() {
    assert!(mentions_thinking(r#"{"error":{"message":"Invalid reasoning_effort: max"}}"#));
    assert!(mentions_thinking("unknown parameter: thinking"));
    assert!(mentions_thinking("bad EFFORT"));
    assert!(!mentions_thinking(r#"{"error":{"message":"model not found"}}"#));
}

// ---- ② 最高档被拒、往下一档被接受 ----

#[tokio::test]
async fn 最高档被拒就往下一档试_阶梯只含试过的() {
    let gateway = FakeGateway::new(|effort| {
        if effort == Some("max") {
            (400, r#"{"error":{"message":"Invalid reasoning_effort: max"}}"#)
        } else {
            (200, r#"{"id":"1"}"#)
        }
    });
    let rungs = rungs_for(&declared());
    let outcome = run_ladder(
        &gateway,
        "http://127.0.0.1:9/v1",
        "openai-completions",
        "test-model",
        None,
        "loopback",
        &rungs,
    )
    .await;
    assert_eq!(outcome.accepted_top.as_deref(), Some("high"));
    let ladder = outcome.ladder.clone().unwrap();
    assert_eq!(
        ladder.keys().cloned().collect::<Vec<_>>(),
        vec!["high".to_string(), "low".to_string(), "off".to_string()]
    );
    assert!(!ladder.contains_key("max"), "被拒的 max 不许进表");
    assert_eq!(outcome.attempts, 3, "基准 + max + high");
    // 真发出去的正文里只有极小请求
    let bodies = gateway.bodies();
    assert_eq!(bodies.len(), 3);
    assert!(bodies[0].contains("\"content\":\"ping\""));
    assert!(!bodies[0].contains("reasoning_effort"));
    assert!(bodies[1].contains("\"reasoning_effort\":\"max\""));
}

// ---- ③ 全部被拒：如实失败，不发明档位 ----

#[tokio::test]
async fn 全部档位被拒就如实失败且不发明档位() {
    let gateway = FakeGateway::new(|effort| match effort {
        None => (200, r#"{"id":"1"}"#),
        Some(_) => (400, r#"{"error":{"message":"unknown reasoning effort"}}"#),
    });
    let rungs = rungs_for(&declared());
    let outcome = run_ladder(
        &gateway,
        "http://127.0.0.1:9",
        "openai-completions",
        "test-model",
        None,
        "loopback",
        &rungs,
    )
    .await;
    assert!(outcome.accepted_top.is_none(), "一个都没认 → acceptedTop 必须是 None");
    assert!(outcome.ladder.is_none(), "失败时**不写回**（ladder 为 None，不是空表）");
    assert!(outcome.message.contains("拒绝了全部"), "{}", outcome.message);
    assert_eq!(outcome.attempts, 4, "基准 + 三个档位都真试过");
}

#[tokio::test]
async fn 基准请求失败就不下结论() {
    let gateway = FakeGateway::new(|_| (401, r#"{"error":{"message":"invalid api key"}}"#));
    let rungs = rungs_for(&declared());
    let outcome = run_ladder(
        &gateway,
        "http://127.0.0.1:9",
        "openai-completions",
        "test-model",
        None,
        "loopback",
        &rungs,
    )
    .await;
    assert!(outcome.accepted_top.is_none());
    assert!(outcome.ladder.is_none());
    assert!(outcome.message.contains("基准请求"), "{}", outcome.message);
    assert_eq!(outcome.attempts, 1, "只发了基准，一次档位都没试");
}

#[tokio::test]
async fn 与思考参数无关的_400_如实中止而不是继续降档() {
    let gateway = FakeGateway::new(|effort| match effort {
        None => (200, r#"{"id":"1"}"#),
        Some(_) => (400, r#"{"error":{"message":"model not found: test-model"}}"#),
    });
    let rungs = rungs_for(&declared());
    let outcome = run_ladder(
        &gateway,
        "http://127.0.0.1:9",
        "openai-completions",
        "test-model",
        None,
        "loopback",
        &rungs,
    )
    .await;
    assert!(outcome.ladder.is_none());
    assert!(outcome.message.contains("与思考参数无关"), "{}", outcome.message);
    assert_eq!(outcome.attempts, 2, "基准 + 一次 max（绝不把「模型名不对」当成「不支持 max」）");
}

#[tokio::test]
async fn 严格网关只认_max_completion_tokens_时按提示换字段重试() {
    let gateway = FakeGateway::new(|_| (400, r#"{"error":{"message":"Use 'max_completion_tokens' instead of 'max_tokens'."}}"#));
    let rungs = rungs_for(&declared());
    let outcome = run_ladder(
        &gateway,
        "http://127.0.0.1:9",
        "openai-completions",
        "test-model",
        None,
        "loopback",
        &rungs,
    )
    .await;
    // 两次基准（换字段）都过不了 → 如实失败，但**确实换过字段**
    assert!(outcome.ladder.is_none());
    assert_eq!(outcome.attempts, 2);
    let bodies = gateway.bodies();
    assert!(bodies[0].contains("\"max_tokens\""));
    assert!(bodies[1].contains("\"max_completion_tokens\""));
}

// ---- ① 开关关着 → 零请求 ----

#[tokio::test]
async fn 开关没开就一个请求都不发() {
    let dir = temp_dir("off");
    let state = crate::web::routes::AppState::new(
        crate::infra::db::open_memory().unwrap(),
        ":memory:",
        dir.join("media"),
    );
    std::fs::create_dir_all(dir.join("capabilities")).unwrap();
    let gateway = FakeGateway::new(|_| (200, r#"{"id":"1"}"#));
    let response = probe(
        &state,
        &dir.join("capabilities"),
        &ProbeRequest::default(),
        &gateway,
    )
    .await
    .unwrap();
    assert_eq!(gateway.calls(), 0, "★ 开关关着：一个上游请求都不许发");
    assert_eq!(response["ok"], false);
    assert_eq!(response["outcomes"].as_array().unwrap().len(), 0);
    let message = response["message"].as_str().unwrap();
    assert!(message.contains("真实请求"), "{message}");
    assert!(message.contains("费用"), "必须如实说清它会花钱：{message}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- ④ 写回：别的条目一个字节都不变 ----

#[test]
fn 写回只动这一条别的条目一个字节不变() {
    let raw = "{\n  \"source\": \"custom\",\n  \"entries\": {\n    \"other-a\": { \"tools\": false },\n    \"TARGET\": {\n      \"reasoning\": true,\n      \"thinkingEfforts\": { \"off\": \"none\", \"max\": \"max\" }\n    },\n    \"other-b\": { \"modalities\": [\"text\", \"image\"] }\n  }\n}\n";
    let next = upsert_entry_text(
        raw,
        "target",
        &OverlayEntry {
            modalities: None,
            tools: None,
            reasoning: None,
            thinking_efforts: Some(BTreeMap::from([
                ("off".to_string(), "none".to_string()),
                ("high".to_string(), "high".to_string()),
            ])),
            thinking_format: Some("deepseek".to_string()),
        },
    )
    .unwrap();

    // 别人的两行**原样在**（原文是压缩写法，不许被重排）
    assert!(next.contains("\"other-a\": { \"tools\": false },"), "{next}");
    assert!(next.contains("\"other-b\": { \"modalities\": [\"text\", \"image\"] }"), "{next}");
    assert!(next.contains("\"source\": \"custom\""));

    // 目标那一条被换成新的档位，且**大小写按原文**（`TARGET` 没被改成小写）
    let parsed: Value = serde_json::from_str(&next).unwrap();
    let entry = &parsed["entries"]["TARGET"];
    assert_eq!(entry["thinkingEfforts"]["high"], "high");
    assert!(entry["thinkingEfforts"].get("max").is_none(), "旧档位不许留下");
    assert_eq!(entry["thinkingFormat"], "deepseek");
    assert_eq!(entry["reasoning"], true, "没碰的维度原样保留");
    assert!(next.contains("\"TARGET\": {"), "键名的大小写按原文：{next}");
    // 除了那一条之外，别的字节真的一模一样（别人的行、原文的排版都没动）
    let head = "\"other-a\": { \"tools\": false },";
    assert_eq!(next.find(head), raw.find(head));
    assert!(
        next.contains("\"other-b\": { \"modalities\": [\"text\", \"image\"] }\n  }\n}\n"),
        "文件的收尾（别的条目的字节 + 原来的缩进）原样：{next}"
    );
}

#[test]
fn 新增一条时不碰已有条目() {
    let raw = "{\n  \"source\": \"custom\",\n  \"entries\": {\n    \"other\": {\"tools\": false}\n  }\n}\n";
    let next = upsert_entry_text(
        raw,
        "brand-new",
        &OverlayEntry {
            modalities: None,
            tools: None,
            reasoning: None,
            thinking_efforts: Some(BTreeMap::from([("off".to_string(), "none".to_string())])),
            thinking_format: None,
        },
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&next).unwrap();
    assert_eq!(parsed["entries"]["other"]["tools"], false, "别人的条目原样保留");
    assert_eq!(parsed["entries"]["brand-new"]["thinkingEfforts"]["off"], "none");
    assert!(parsed["entries"]["brand-new"].get("thinkingFormat").is_none());
    assert_eq!(parsed["source"], "custom");
}

#[test]
fn 嵌套对象与转义字符串里的_entries_不会认错() {
    // `other` 的值里也有一个叫 entries 的键 —— 扫描器必须只认**最外层**那一份
    let raw = r#"{"entries":{"other":{"entries":{"x":1}},"TARGET":{"reasoning":true}}}"#;
    let next = upsert_entry_text(
        raw,
        "target",
        &OverlayEntry {
            modalities: None,
            tools: None,
            reasoning: None,
            thinking_efforts: Some(BTreeMap::from([("off".to_string(), "none".to_string())])),
            thinking_format: None,
        },
    )
    .unwrap();
    assert!(next.contains(r#""other":{"entries":{"x":1}}"#), "{next}");
    let parsed: Value = serde_json::from_str(&next).unwrap();
    assert_eq!(parsed["entries"]["TARGET"]["thinkingEfforts"]["off"], "none");
}

// ---- ⑤ 坏 JSON：如实报错、不覆盖 ----

#[test]
fn 坏_json_如实报错且一个字节都不覆盖() {
    let dir = temp_dir("broken");
    let file = dir.join(CUSTOM_FILE);
    let broken = "{ 这不是 JSON";
    std::fs::write(&file, broken).unwrap();
    let err = write_entry(
        &dir,
        "m",
        &BTreeMap::from([("off".to_string(), "none".to_string())]),
        None,
    )
    .unwrap_err();
    assert!(err.contains("不是合法 JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), broken, "坏文件不许被覆盖");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 形状不对的_json_也拒绝覆盖() {
    // 顶层不是对象 / `entries` 不是对象 → 拒绝（改下去只会把用户的文件弄坏）
    for raw in [r#"[]"#, r#"{"entries": 3}"#, r#"{"entries": []}"#, "不是 JSON"] {
        let err = upsert_entry_text(raw, "m", &default_entry()).unwrap_err();
        let refuses = err.contains("拒绝覆盖") || err.contains("不是合法 JSON");
        assert!(refuses, "{raw} → {err}");
    }
    // 而 `entries` **缺席**是合法起点（`{}` / `{"source":"custom"}`）：补一个空表，别的键保留
    let next = upsert_entry_text(r#"{"source":"custom"}"#, "m", &default_entry()).unwrap();
    let parsed: Value = serde_json::from_str(&next).unwrap();
    assert_eq!(parsed["source"], "custom", "原有键保留");
    assert!(parsed["entries"]["m"].is_object(), "{next}");
}

#[test]
fn 条目不存在时新建一份文件() {
    let dir = temp_dir("create");
    assert!(!dir.join(CUSTOM_FILE).exists());
    write_entry(
        &dir,
        "m",
        &BTreeMap::from([
            ("off".to_string(), "none".to_string()),
            ("high".to_string(), "high".to_string()),
        ]),
        Some("qwen"),
    )
    .unwrap();
    let text = std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["entries"]["m"]["thinkingEfforts"]["high"], "high");
    assert_eq!(parsed["entries"]["m"]["thinkingFormat"], "qwen");
    // 临时文件不许留在盘上
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 写盘是临时文件加_rename() {
    let dir = temp_dir("atomic");
    std::fs::write(dir.join(CUSTOM_FILE), r#"{"entries":{"m":{"tools":false}}}"#).unwrap();
    write_entry(
        &dir,
        "m",
        &BTreeMap::from([("off".to_string(), "none".to_string())]),
        None,
    )
    .unwrap();
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap()).unwrap();
    assert_eq!(parsed["entries"]["m"]["tools"], false, "原有维度保留");
    assert_eq!(parsed["entries"]["m"]["thinkingEfforts"]["off"], "none");
    let _ = std::fs::remove_dir_all(&dir);
}

fn default_entry() -> OverlayEntry {
    OverlayEntry {
        modalities: None,
        tools: None,
        reasoning: None,
        thinking_efforts: Some(BTreeMap::new()),
        thinking_format: None,
    }
}

// ---- 端到端（内存库 + 假网关，仍然一条网都不出）----

fn seeded_state(dir: &std::path::Path) -> AppState {
    // `dir` 就是 `<storage>\ai\capabilities`（用例直接往这里写 custom.json）
    std::fs::create_dir_all(dir).unwrap();
    let media = dir.join("media");
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO ai_providers (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
        VALUES ('fake', '假网关', 'openai-completions', 'http://127.0.0.1:9/v1', NULL, 'loopback', 1, 1);
        INSERT INTO ai_models (provider_id, model_id, display_name, input_modalities, reasoning,
                               thinking_efforts, thinking_format, capability_source, enabled)
        VALUES ('fake', 'think-a', '推理模型', '["text"]', 1,
                '{"off":"none","low":"low","high":"high","max":"max"}', NULL, 'builtin', 1),
               ('fake', 'plain-b', '纯文本', '["text"]', 0, NULL, NULL, 'builtin', 1),
               ('fake', 'off-c', '被关掉的推理模型', '["text"]', 1,
                '{"off":"none","high":"high"}', NULL, 'builtin', 0);
        "#,
    )
    .unwrap();
    AppState::new(conn, ":memory:", media)
}

fn enable_probe(state: &AppState) {
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO app_settings (k, v) VALUES ('ai.capabilities.probe', '1')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn 端到端_只打推理模型并写回_custom_json() {
    let root = temp_dir("e2e");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    std::fs::write(
        dir.join(CUSTOM_FILE),
        r#"{"source":"custom","entries":{"other-model":{"tools":false}}}"#,
    )
    .unwrap();

    let gateway = FakeGateway::new(|effort| {
        if effort == Some("max") {
            (400, r#"{"error":{"message":"Invalid reasoning_effort"}}"#)
        } else {
            (200, r#"{"id":"1"}"#)
        }
    });
    let response = probe(&state, &dir, &ProbeRequest::default(), &gateway).await.unwrap();

    // 只有 `think-a`（reasoning && enabled）被打；`plain-b` / `off-c` 一个请求都没有
    let outcomes = response["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1, "{response}");
    assert_eq!(outcomes[0]["providerId"], "fake");
    assert_eq!(outcomes[0]["modelId"], "think-a");
    assert_eq!(outcomes[0]["ok"], true);
    assert_eq!(outcomes[0]["acceptedTop"], "high");
    assert_eq!(outcomes[0]["written"], true);
    assert_eq!(outcomes[0]["skipped"], false);
    assert_eq!(response["ok"], true);

    // 写回：只动这一条，别人的条目**原样在**（紧凑写法没被重排）
    let text = std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["entries"]["other-model"],
        serde_json::json!({ "tools": false }),
        "别人的条目语义原样保留：{text}"
    );
    assert!(
        text.contains("\"other-model\":{\"tools\":false},"),
        "别人的条目**字节**也没被重排（它后面直接就是新条目）：{text}"
    );
    let efforts = &parsed["entries"]["think-a"]["thinkingEfforts"];
    assert_eq!(efforts["high"], "high");
    assert_eq!(efforts["low"], "low");
    assert_eq!(efforts["off"], "none");
    assert!(efforts.get("max").is_none(), "被拒的 max 不许写回：{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_全部被拒时_custom_json_一个字节都不改() {
    let root = temp_dir("e2e-reject");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    let before = r#"{"source":"custom","entries":{"other-model":{"tools":false}}}"#;
    std::fs::write(dir.join(CUSTOM_FILE), before).unwrap();

    let gateway = FakeGateway::new(|effort| match effort {
        None => (200, r#"{"id":"1"}"#),
        Some(_) => (400, r#"{"error":{"message":"unknown reasoning effort"}}"#),
    });
    let response = probe(&state, &dir, &ProbeRequest::default(), &gateway).await.unwrap();

    let outcomes = response["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0]["ok"], false);
    assert_eq!(outcomes[0]["written"], false);
    assert!(outcomes[0].get("acceptedTop").is_none(), "不发明档位：{response}");
    assert!(outcomes[0].get("ladder").is_none());
    assert_eq!(response["ok"], false);
    assert_eq!(std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap(), before, "一个字都不许改");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_坏_custom_json_当场如实报错且不开始探测() {
    let root = temp_dir("e2e-broken");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    std::fs::write(dir.join(CUSTOM_FILE), "{ 这不是 JSON").unwrap();

    let gateway = FakeGateway::new(|_| (200, r#"{"id":"1"}"#));
    let response = probe(&state, &dir, &ProbeRequest::default(), &gateway).await.unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(gateway.calls(), 0, "写不回去就别开始花钱");
    assert!(response["message"].as_str().unwrap().contains("不是合法 JSON"), "{response}");
    assert_eq!(std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap(), "{ 这不是 JSON");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_指定不存在的模型不发明条目() {
    let root = temp_dir("e2e-missing");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    std::fs::write(dir.join(CUSTOM_FILE), r#"{"entries":{"other":{"tools":false}}}"#).unwrap();

    let gateway = FakeGateway::new(|_| (200, r#"{"id":"1"}"#));
    let request = ProbeRequest {
        provider_id: Some("fake".to_string()),
        model_id: Some("no-such-model".to_string()),
    };
    let response = probe(&state, &dir, &request, &gateway).await.unwrap();
    assert_eq!(response["outcomes"].as_array().unwrap().len(), 0);
    assert_eq!(gateway.calls(), 0);
    assert_eq!(
        std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap(),
        r#"{"entries":{"other":{"tools":false}}}"#
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_一个模型失败不影响另一个() {
    let root = temp_dir("e2e-two");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    // 再放一个推理模型：它排前面（display_name 排序），必定先跑
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name, input_modalities, reasoning,
                                        thinking_efforts, capability_source, enabled)
                 VALUES ('fake', 'think-0', 'AAA 先跑的', '[\"text\"]', 1,
                         '{\"off\":\"none\",\"high\":\"high\"}', 'builtin', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    // 先跑的那个模型基准就 500（网关炸了）；后跑的那个最高档被拒、逼它降一档
    struct Flaky {
        seen: Mutex<Vec<String>>,
    }
    impl Transport for Flaky {
        async fn post(
            &self,
            _url: &str,
            _api: &str,
            _secret: Option<&str>,
            _trust: &str,
            body: &str,
            _timeout_seconds: u64,
        ) -> ProbeResponse {
            let parsed: Value = serde_json::from_str(body).unwrap();
            let model = parsed["model"].as_str().unwrap().to_string();
            self.seen.lock().unwrap().push(model.clone());
            if model == "think-0" {
                ProbeResponse { status: Some(500), body: "boom".into(), detail: String::new() }
            } else if parsed["reasoning_effort"] == "max" {
                ProbeResponse {
                    status: Some(400),
                    body: r#"{"error":{"message":"Invalid reasoning_effort"}}"#.into(),
                    detail: String::new(),
                }
            } else {
                ProbeResponse::ok(200)
            }
        }
    }
    let transport = Flaky { seen: Mutex::new(Vec::new()) };
    let response = probe(&state, &dir, &ProbeRequest::default(), &transport).await.unwrap();
    // 真的两个都跑到了（第一个炸了之后没有提前收队），而且第二个**真的降过档**
    assert_eq!(
        transport.seen.lock().unwrap().clone(),
        vec![
            "think-0".to_string(), // 基准就炸了 → 一个档位都没试
            "think-a".to_string(), // 基准
            "think-a".to_string(), // max（被拒）
            "think-a".to_string(), // high（接受）
        ],
        "先跑的那个炸一次；后一个的基准 + max（拒）+ high（接受）"
    );
    let outcomes = response["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 2, "{response}");
    let first = &outcomes[0];
    assert_eq!(first["modelId"], "think-0");
    assert_eq!(first["ok"], false);
    assert_eq!(first["written"], false);
    assert!(
        first["message"].as_str().unwrap().contains("基准请求"),
        "连基准都过不了就别下结论：{first}"
    );
    let second = &outcomes[1];
    assert_eq!(second["modelId"], "think-a");
    assert_eq!(second["ok"], true, "一个模型炸了不许把整批拖死：{response}");
    assert_eq!(second["acceptedTop"], "high");
    // 失败的那条**没进** custom.json
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(CUSTOM_FILE)).unwrap()).unwrap();
    assert!(parsed["entries"].get("think-0").is_none(), "{parsed}");
    assert!(parsed["entries"].get("think-a").is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_非_openai_completions_的上游诚实跳过() {
    let root = temp_dir("e2e-anthropic");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    state
        .write(|tx| {
            tx.execute("UPDATE ai_providers SET api='anthropic-messages' WHERE id='fake'", [])?;
            Ok(())
        })
        .unwrap();
    let gateway = FakeGateway::new(|_| (200, r#"{"id":"1"}"#));
    let response = probe(&state, &dir, &ProbeRequest::default(), &gateway).await.unwrap();
    let outcomes = response["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0]["skipped"], true);
    assert!(outcomes[0]["message"].as_str().unwrap().contains("只支持 OpenAI 兼容"), "{response}");
    assert_eq!(gateway.calls(), 0, "协议不支持就别发请求");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn 端到端_每_provider_最多八个模型() {
    let root = temp_dir("e2e-cap");
    let dir = root.join("capabilities");
    let state = seeded_state(&dir);
    enable_probe(&state);
    state
        .write(|tx| {
            for index in 0..12 {
                tx.execute(
                    "INSERT INTO ai_models (provider_id, model_id, display_name, input_modalities, reasoning,
                                            thinking_efforts, capability_source, enabled)
                     VALUES ('fake', ?1, ?2, '[\"text\"]', 1, '{\"off\":\"none\",\"high\":\"high\"}', 'builtin', 1)",
                    rusqlite::params![
                        format!("many-{index:02}"),
                        format!("ZZ 批量 {index:02}")
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let gateway = FakeGateway::new(|_| (200, r#"{"id":"1"}"#));
    let response = probe(&state, &dir, &ProbeRequest::default(), &gateway).await.unwrap();
    assert_eq!(
        response["outcomes"].as_array().unwrap().len(),
        MAX_MODELS_PER_PROVIDER,
        "防手滑全库烧钱"
    );
    let _ = std::fs::remove_dir_all(&root);
}
