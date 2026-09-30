//! `run_lifecycle` 的单元测试（主体代码在 [`run_lifecycle`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// 内存库 + 建表 + 一条 run（与 `events.rs` 的用例同一套搭法）。
fn test_state() -> AppState {
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("db").join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.execute_batch(
        "INSERT INTO ai_conversations (id, title) VALUES ('c1', 't');
         INSERT INTO ai_runs (id, conversation_id, status) VALUES ('r1', 'c1', 'running');",
    )
    .unwrap();
    AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"))
}

fn model_with(reasoning: bool, efforts: &[(&str, &str)]) -> ModelRow {
    ModelRow {
        id: "m1".into(),
        display_name: "模型".into(),
        api: "openai-completions".into(),
        base_url: "https://x/v1".into(),
        credential_ref: None,
        endpoint_trust: "public".into(),
        tools: true,
        reasoning,
        thinking_efforts: efforts.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        thinking_format: None,
        max_output_tokens: None,
        input_modalities: Vec::new(),
    }
}

/// 思考强度的四个分支（AIH-056）：**不许静默把非法值当 off**。
#[test]
fn thinking_effort_branches() {
    let plain = model_with(false, &[]);
    // 没传 / 空 / off → (off, None)：落库记 off，上游一个参数都不发
    for requested in [None, Some(""), Some("  "), Some("off")] {
        assert_eq!(thinking_effort(&plain, requested).unwrap(), ("off".into(), None));
    }
    // 认不出的值：**400**，不是当 off
    let err = thinking_effort(&plain, Some("turbo")).unwrap_err();
    assert_eq!(err.error, "CONFIG_ERROR");
    assert!(err.detail.unwrap().contains("未知的思考强度"));

    // 模型没声明推理能力 → 400（不悄悄忽略）
    let err = thinking_effort(&plain, Some("high")).unwrap_err();
    assert!(err.detail.unwrap().contains("未声明推理能力"));

    // 声明了推理、但没列等级 → 请求什么给什么
    let reasoning = model_with(true, &[]);
    assert_eq!(
        thinking_effort(&reasoning, Some("high")).unwrap(),
        ("high".into(), Some("high".into()))
    );

    // 列了等级 → 只允许表里的
    let listed = model_with(true, &[("low", "low"), ("max", "4096")]);
    assert!(thinking_effort(&listed, Some("low")).is_ok());
    assert!(thinking_effort(&listed, Some("max")).is_ok());
    let err = thinking_effort(&listed, Some("medium")).unwrap_err();
    assert!(err.detail.unwrap().contains("未声明思考强度 medium"));
}

/// 心跳帧**不带 `id:` 行**（它不是可续传的游标）；正式事件带。
#[test]
fn heartbeat_frames_have_no_id_line() {
    let hb = event_to_sse(0, event_type::HEARTBEAT, &json!({}));
    assert_eq!(hb, "event: heartbeat\ndata: {}\n\n");
    assert!(!hb.contains("id:"));
    let ev = event_to_sse(7, event_type::TEXT_DELTA, &json!({"text": "a"}));
    assert!(ev.starts_with("id: 7\nevent: text.delta\n"));
}

/// **首段思考不必等第一个工具调用**（用户 bug 2026-09-28：
/// 「回复的首次思考过程没有显示 —— 等输出第一个工具调用的时候才显示最早的思考结果」）。
///
/// 现场：事件总线是批写的（200 条一次事务），而这条流**只从库里读**。
/// 一次 Run 开头的 `run.started` / `message.started` / 几条 `reasoning.delta`
/// 加起来远不到一批，全压在批里看不见；直到第一条 `tool.requested`
/// （闸门类事件，那边会 flush）才把憋着的思考一起放出来 ——
/// 用户看到的就是"思考跟第一个工具卡同时冒出来"。
///
/// 这条用例就是那个现场的最小复现：**只 emit、故意不 flush**，
/// 然后要求 SSE 的第一次 poll 就能读到它。去掉 `poll_next` 里那次 flush 必红。
#[test]
fn sse_sees_events_without_waiting_for_a_full_batch() {
    let state = test_state();
    state.bus.open("r1");
    // 真实路径上这里就是"思考增量"：一条低产事件，谁也不会为它开事务
    state
        .bus
        .emit(
            &state,
            "r1",
            event_type::REASONING_DELTA,
            &json!({ "messageId": "a1", "text": "先想一想" }),
        )
        .unwrap()
        .expect("事件要真的发出去");
    assert!(
        runs::events(&state, "r1", 0).unwrap().is_empty(),
        "前提：这时候库里确实还没有（还压在批里）"
    );

    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async {
        use futures_util::StreamExt;
        let mut stream = Box::pin(SseStream {
            state: state.clone(),
            run_id: "r1".into(),
            cursor: 0,
            idle: Duration::ZERO,
            done: false,
        });
        let got = tokio::time::timeout(Duration::from_secs(1), stream.next()).await;
        let bytes = match got {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(other) => panic!("流不该是 {other:?}"),
            Err(_) => panic!("1 秒内一个字都没读到 —— 事件还压在批里没落库"),
        };
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("event: reasoning.delta"), "读到的帧是：{text}");
        assert!(text.contains("先想一想"), "思考正文要在帧里：{text}");
    });
}
