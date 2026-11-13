//! 执行器与闸门：一次 Run 的主循环、流中断、审批、提问、轮次与预算上限、重试。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

/// 一次完整的流式 Run：正文逐字到达、有序块按序落库、usage 与终态都对。
#[tokio::test]
async fn executor_streams_a_reply_and_persists_parts_in_order() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "打个招呼", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert!(run["completedAt"].is_string());

    // 助手消息：正文是**三段拼起来的**，状态 complete
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["text"], "你好呀", "三段增量拼成完整正文");
    assert_eq!(assistant["status"], "complete");
    assert_eq!(
        assistant["providerResponseId"], "chatcmpl-1",
        "上游给的响应 id 要记下来"
    );
    // usage 落库/下发的是**契约的四个字段**（归一化在 `upstream::TokenUsage`）：
    // 假网关给的是 OpenAI 那套嵌套明细，**缓存命中与思考一个都不许丢** ——
    // 它们曾经在归一化时被整批丢掉，于是界面上那行「缓存命中」在桌面上从未出现过。
    assert_eq!(
        assistant["usage"],
        serde_json::json!({
            "inputTokens": 7,
            "outputTokens": 3,
            "cachedTokens": 4,
            "reasoningTokens": 1,
        }),
        "usage 按契约四字段落库（{assistant}）"
    );

    // 有序块：**思考在前、正文在后**（顺序乱了工具卡位置就错，而正文还在）
    let parts = assistant["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2, "{assistant}");
    assert_eq!(parts[0]["type"], "reasoning");
    assert_eq!(parts[0]["text"], "先想一下");
    assert_eq!(parts[1]["type"], "text");
    assert_eq!(parts[1]["text"], "你好呀");

    // 事件：run.started → … → message.completed → run.completed（seq 连续、不跳号）
    let types = event_types(&state, &run_id);
    assert_eq!(types.first().unwrap(), "run.started");
    assert_eq!(types.get(1).unwrap(), "message.started");
    assert!(types.contains(&"reasoning.delta".to_string()));
    assert_eq!(
        types.iter().filter(|t| *t == "text.delta").count(),
        3,
        "三段正文 → 三条事件（逐字流式）：{types:?}"
    );
    assert!(types.contains(&"usage.updated".to_string()));
    // `usage.updated` 的载荷是**平铺的四个键**（`docs/run-event-contract.md` §2）——
    // 与手机 harness 的 `EventPayloads.usage` 同形，不是 `{"usage": {...}}` 那种包一层。
    // ⚠ 这一条是契约的一致性防线：两端各写一种形状时，界面读哪一端就决定了它有没有 token 显示。
    let usage_payload = event_payload(&state, &run_id, "usage.updated")
        .expect("usage.updated 必须落库 —— 缓存命中率就在它里面，界面那行「缓存命中」靠它");
    assert_eq!(
        usage_payload,
        serde_json::json!({
            "inputTokens": 7,
            "outputTokens": 3,
            "cachedTokens": 4,
            "reasoningTokens": 1,
        }),
        "usage.updated 平铺四字段（契约 §2 / 手机 harness 同形）"
    );
    assert_eq!(types.last().unwrap(), "run.completed");
    let seqs = state
        .with(|conn| {
            let mut stmt =
                conn.prepare("SELECT seq FROM ai_run_events WHERE run_id = ?1 ORDER BY seq")?;
            let rows = stmt
                .query_map(rusqlite::params![run_id], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(
        seqs,
        (1..=seqs.len() as i64).collect::<Vec<_>>(),
        "seq 从 1 起、连续递增（红线 R5 的「绝不跳号」）"
    );

    gateway.abort();
}

/// **R6 的现场**：上游说了两段之后**连接被掐断** —— 已经说出去的话一个字都不能丢。
#[tokio::test]
async fn executor_keeps_what_was_already_said_when_the_stream_breaks() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &format!("{base_url}?upstream=partial"));

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "写一段", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "failed", "{run}");
    assert!(run["errorCode"].is_string());

    // ★ 这一条就是 R6：**已经说出去的两段话必须还在**（正文 + 有序块都得有）
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["status"], "failed");
    assert_eq!(
        assistant["text"], "你好",
        "断流前已经说出去的话不许丢（R6）：{assistant}"
    );
    let parts = assistant["parts"].as_array().unwrap();
    assert!(
        parts
            .iter()
            .any(|p| p["type"] == "text" && p["text"] == "你好"),
        "正文必须**同时**落进有序块（前端在 parts 非空时只渲染 parts）：{assistant}"
    );
    assert!(
        parts.iter().any(|p| p["type"] == "reasoning"),
        "思考段也要留着：{assistant}"
    );

    // 事件序列同样以终态事件收尾（断线重连能从库里补完）
    let types = event_types(&state, &run_id);
    assert_eq!(types.last().unwrap(), "run.failed");
    assert_eq!(types.first().unwrap(), "run.started");

    gateway.abort();
}

/// 工具往返：模型调 `list_skills`（ALLOW，不需审批）→ 执行 → 结果回填 → 模型收口。
#[tokio::test]
async fn executor_runs_a_tool_round_trip_and_orders_the_parts() {
    let (base_url, gateway) = fake_tool_gateway("list_skills", "{}").await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "看看有哪些技能", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 有序块：**工具卡在前、工具结果在中、两轮正文按顺序**
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(
        assistant["text"], "工具用完了",
        "最终正文是**第二轮**的：{assistant}"
    );
    let kinds: Vec<&str> = assistant["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["tool_call", "tool_result", "text"],
        "块顺序 = 事情发生的顺序"
    );
    assert_eq!(assistant["parts"][0]["jsonPayload"]["name"], "list_skills");
    assert_eq!(assistant["parts"][0]["toolCallId"], "call_1");
    // ⚠ 参数是**模型给的原文**（字符串），不是解析后的对象 —— Dart 侧 `map['arguments']`
    // 读的就是它；存成对象会变成 `{query: x}` 这种既不是 JSON、也没法回填的东西。
    assert_eq!(assistant["parts"][0]["jsonPayload"]["arguments"], "{}");
    assert_eq!(assistant["parts"][1]["toolCallId"], "call_1");
    // `list_skills` 的结果是 JSON 文本（含 count / skills）
    assert!(
        assistant["parts"][1]["text"]
            .as_str()
            .unwrap()
            .contains("skills"),
        "工具结果要落进块里（R6）：{assistant}"
    );
    // 结果块的 payload 键集合**就是历史回放的契约**（Kotlin `HarnessRunner:762`）：
    // 少了 `ok`，一条成功的调用在重开 App 之后会被画成失败。
    let result_payload = &assistant["parts"][1]["jsonPayload"];
    assert_eq!(
        keys(result_payload),
        vec!["approval", "code", "elapsedMs", "name", "ok"],
        "{result_payload}"
    );
    assert_eq!(result_payload["ok"], true);
    assert_eq!(result_payload["approval"], "not_required");
    assert_eq!(result_payload["name"], "list_skills");

    // 事件：`tool.requested` **每一个调用都发**（只读工具带 `approval:not_required`）——
    // 界面靠它给工具卡排位置（流式期间"思考 → 工具 → 正文"的交错）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.started".to_string()), "{types:?}");
    assert!(types.contains(&"tool.completed".to_string()), "{types:?}");
    let requested = types
        .iter()
        .position(|t| t == "tool.requested")
        .unwrap_or_else(|| panic!("只读工具也要发 requested（带 not_required）：{types:?}"));
    let started = types.iter().position(|t| t == "tool.started").unwrap();
    assert!(requested < started, "先 requested 再 started：{types:?}");
    assert_eq!(types.last().unwrap(), "run.completed");
    assert_eq!(types.first().unwrap(), "run.started");

    // `tool.completed` 的键集合也是契约的一部分（`preview` 是工具卡上的那一行小字）
    let completed = state
        .with(|conn| {
            conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.completed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    assert_eq!(
        keys(&completed),
        vec!["callId", "elapsedMs", "name", "preview", "result"]
    );

    // 审计表里留了痕
    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "list_skills");
    assert_eq!(calls[0]["status"], "ok");
    assert_eq!(calls[0]["approval"], "not_required");

    gateway.abort();
}

/// **思考必须逐轮回传**（`openai-responses`，2026-10 的真实 400）：
/// 工具结果回填的**第二次请求**必须带上这一轮产出的 `reasoning` 项，否则上游回
/// `The reasoning_text in the thinking mode must be passed back to the API.`、整条 Run 变红。
///
/// 协议层单测（`responses_reasoning_round_trip`）只证明适配器**会**铺这个项；
/// 这里证明**执行器真的把这一轮的思考交了出来**（按轮采集、历史不丢），
/// 而且思考照旧落成有序块、**不混进正文**。
#[tokio::test]
async fn responses_run_echoes_each_round_reasoning_or_the_upstream_400s() {
    let (base_url, gateway, seen) = reasoning_echo_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    // 只换协议：夹具那条 provider 是 `openai-completions`，这条 Bug 只出在 responses 上
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_providers SET api = 'openai-responses' WHERE id = 'goat'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    point_at_gateway(&state, &base_url);

    let (_, assistant_id, run) = start_run_and_wait(&state, "看看有哪些技能").await;
    assert_eq!(
        run["status"], "completed",
        "第 2 轮没带 reasoning 项的话，假网关照上游原话回 400、Run 会失败：{run}"
    );

    let bodies = seen.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "两轮上游请求：要工具 → 回填工具结果后收口");
    assert!(
        !bodies[0].contains("reasoning_text"),
        "第一轮没有历史思考可回传，不许凭空造一个：{}",
        bodies[0]
    );
    let second = &bodies[1];
    assert!(
        second.contains(r#""type":"reasoning""#),
        "第二轮必须带 reasoning 项：{second}"
    );
    assert!(
        second.contains(r#""type":"reasoning_text""#),
        "内容块是 reasoning_text：{second}"
    );
    assert!(
        second.contains(r#""summary":[{"type":"summary_text","text":"先看看技能"}]"#),
        "summary 必须带同一段思考；严格网关缺了它就回 400 input[N].summary: expected an array：{second}"
    );
    assert!(
        second.contains("先看看技能"),
        "回传的是**这一轮**的思维链原文：{second}"
    );
    let reasoning_at = second.find(r#""type":"reasoning""#).unwrap();
    let call_at = second.find(r#""type":"function_call""#).unwrap();
    assert!(
        reasoning_at < call_at,
        "reasoning 项要排在同一轮 function_call 之前：{second}"
    );
    // 调用 id 的**一致性**才是上游要的：无状态 API 靠它把 `function_call` 与
    // `function_call_output` 配起来（两个条目我们都发，所以自洽）。
    // ⚠ 目前取的是参数分片里的 `item_id`（`output_item.added` 里的真 `call_id` 会被它盖掉，
    //    见 `OpenAiResponsesAdapter::interpret`）—— 这条断言只钉"两边一致"这个事实，
    //    不钉具体取值：改它要单独评估（`ToolCallAccumulator` 三家协议共用）。
    assert_eq!(
        second.matches(r#""call_id":"item_1""#).count(),
        2,
        "function_call 与 function_call_output 必须带同一个调用 id：{second}"
    );

    // 界面上：思考落成块、顺序 = 事情发生的顺序，而且**正文里没有思考**（R6）
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["text"], "收口了");
    assert!(
        !assistant["text"].as_str().unwrap().contains("先看看技能"),
        "思考段不许混进正文：{assistant}"
    );
    let kinds: Vec<&str> = assistant["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["reasoning", "tool_call", "tool_result", "text"],
        "{assistant}"
    );
    assert_eq!(assistant["parts"][0]["text"], "先看看技能");

    gateway.abort();
}

/// **R8 的顺序**：需要审批的工具 —— **先落 pending 行**，再发 `tool.requested`，
/// 之后在**任意一条路径**上都能批准（跨端）。
#[tokio::test]
async fn approval_gate_opens_before_the_user_can_click() {
    // 用 `remember`（只写本机那份长期记忆文件、**不出网**）：它出厂是 ALLOW，
    // 这里用**逐工具覆盖**把它变成 ASK。这样这一条测的纯粹是闸门 ——
    // 用 `civitai_search` 那种要出网的工具会让用例因为网络抖动而红。
    let (base_url, gateway) =
        fake_tool_gateway("remember", r#"{"content":"用户偏好 4:3 画幅"}"#).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "permissionMode": "ask", "overrides": { "remember": "ask" } }),
    );

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "记一下我的画幅偏好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();

    // ★ R8 的防线：**点批准的那一刻，库里必须已经有那一行 pending**
    //
    // 这里刻意**不等事件**：界面点击的速度不可预知，而"点下去落空、
    // 只能干等超时"正是这条红线要防的事。所以判据是"立刻点也能被接受"。
    let mut accepted = false;
    for _ in 0..80 {
        let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
        if calls.is_empty() || calls[0]["status"] != "pending" {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        }
        // 库里有 pending 了 → **立即**批准（即"提前点击"那条路径）
        let (status, result) = send_json(
            router_for(state.clone()),
            "POST",
            "/api/ai/tool-calls/call_1/approve",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["approved"], true);
        assert_eq!(result["callId"], "call_1");
        accepted = result["accepted"].as_bool().unwrap_or(false);
        break;
    }
    assert!(
        accepted,
        "手快那一下**必须**被接受（否则用户只能等超时）——R8"
    );

    // 再点一次：没有 pending 行可改了 → `accepted:false`（那不是错误）
    let (status, again) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/tool-calls/call_1/approve",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["accepted"], false);
    assert_eq!(again["approved"], true, "重复点也是「批准」这个答复本身");

    // Run 继续跑完（**这一轮真的执行了那个工具**）
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 落库的行：`approval` 记录决定、`status` 记录执行结果，**且只有一行** ——
    // 一并把 `status` 写成 `approved`（或让两列互相顶替）会使原地更新无法匹配，
    // 同一次调用在库里变成两行，而 `/api/ai/pending` 又会把它当成"还在等人批"。
    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(
        calls.len(),
        1,
        "同一次调用只许一行（两列分开记，不是插新行）：{calls:?}"
    );
    assert_eq!(calls[0]["approval"], "approved");
    assert_eq!(calls[0]["status"], "ok");

    // 事件：`tool.requested` 要在，而且排在 `tool.started` **之前**（先"请批准"，再"开始执行"）
    let types = event_types(&state, &run_id);
    let requested = types
        .iter()
        .position(|t| t == "tool.requested")
        .unwrap_or_else(|| panic!("需要审批的工具必须发 tool.requested：{types:?}"));
    let started = types
        .iter()
        .position(|t| t == "tool.started")
        .unwrap_or_else(|| panic!("批准之后要发 tool.started：{types:?}"));
    assert!(
        requested < started,
        "顺序反了（先暴露工具、再等批准）：{types:?}"
    );
    assert_eq!(types.last().unwrap(), "run.completed");

    // 闸门类事件**立刻落库**（不是攒批）：另一台设备连上来时它必须已经在库里，
    // 否则对方会一直空等（M1 的约束）。
    let requested_seq = state
        .with(|conn| {
            conn.query_row(
                "SELECT MIN(seq) FROM ai_run_events WHERE run_id = ?1 AND event_type = 'tool.requested'",
                rusqlite::params![run_id],
                |row| row.get::<_, Option<i64>>(0),
            )
        })
        .unwrap()
        .expect("tool.requested 已经在库里");
    assert!(requested_seq > 0);

    // 工具**真的执行了**：本机那份长期记忆里出现了这一条
    let memory = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("memory.md");
    let text = std::fs::read_to_string(&memory).unwrap_or_default();
    assert!(
        text.contains("用户偏好 4:3 画幅"),
        "批准后要真的执行：{text}"
    );

    gateway.abort();
}

/// **拒绝那条路**：点了「拒绝」之后 —— 工具**没有被执行**、库里那行记 `denied`/`denied`
/// （一行，不重复）、Run 照样收口，而且模型收到的是"未执行"的如实说明（**不许假装跑过**）。
#[tokio::test]
async fn a_denied_call_is_recorded_and_never_runs() {
    let (base_url, gateway) =
        fake_tool_gateway("remember", r#"{"content":"用户偏好 4:3 画幅"}"#).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "permissionMode": "ask", "overrides": { "remember": "ask" } }),
    );

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "记一下我的画幅偏好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();

    let mut denied = false;
    for _ in 0..80 {
        let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
        if calls.is_empty() || calls[0]["status"] != "pending" {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        }
        let (status, result) = send_json(
            router_for(state.clone()),
            "POST",
            "/api/ai/tool-calls/call_1/deny",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["approved"], false);
        denied = result["accepted"].as_bool().unwrap_or(false);
        break;
    }
    assert!(denied, "拒绝也必须能落到那一行上（R8）");

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "拒绝不代表整轮 Run 失败：{run}");

    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "{calls:?}");
    // 两列的分工（与 Kotlin `ToolRegistry.failure` 逐字一致）：`approval` 记录决定，
    // `status` 记执行结果 —— 被拒绝的那次**没有执行**，所以状态是 `failed` 而不是 `ok`；
    // 界面靠 `approval == "denied"` 把它画成「已拒绝」而不是「出错」。
    assert_eq!(calls[0]["approval"], "denied");
    assert_eq!(calls[0]["status"], "failed");
    assert_eq!(
        calls[0]["error"],
        "用户没有批准这次工具调用（或等待超时）；不要重试同一件事，如实说明即可"
    );

    // **没有执行**：记忆文件里不该出现那一条
    let memory = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("memory.md");
    let text = std::fs::read_to_string(&memory).unwrap_or_default();
    assert!(
        !text.contains("用户偏好 4:3 画幅"),
        "被拒绝的调用不许真的跑：{text}"
    );

    // 事件：`tool.failed` 带稳定码 `code`（界面按 `approval=denied` 画"已拒绝"），
    // 而且**不发 tool.started**（拒绝的调用没开始过）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.failed".to_string()), "{types:?}");
    assert!(
        !types.contains(&"tool.started".to_string()),
        "拒绝的调用没开始过：{types:?}"
    );
    let denied_event = state
        .with(|conn| {
            conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.failed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let denied_event: serde_json::Value = serde_json::from_str(&denied_event).unwrap();
    assert_eq!(denied_event["code"], "APPROVAL_DENIED");
    assert_eq!(denied_event["approval"], "denied");
    assert_eq!(
        keys(&denied_event),
        vec!["approval", "callId", "code", "elapsedMs", "message", "name"]
    );

    gateway.abort();
}

/// **轮数上限**（`maxToolSteps`）：到顶那一轮**不执行**工具，而是把"为什么没继续"
/// 如实补进正文 —— 而且这一轮 Run **照样正常收尾**（不是 `run.failed`）。
///
/// 判据来自 Kotlin `HarnessRunner:657`：`outOfSteps` 时那句
/// 「已达到本次回复的工具轮数上限 N，这次要调用的工具没有执行；再发一条消息它就能接着做。」
/// 写成"整轮失败"的后果是已显示的那段回答被标记失败，而该情形只是"这轮到此为止"。
#[tokio::test]
async fn tool_round_limit_wraps_up_instead_of_failing_the_run() {
    // 模型**每一轮都要工具**（第二轮也会要到）→ 正好撞上限
    let (base_url, gateway) = scripted_tool_gateway(vec![
        vec![("list_skills", "{}")],
        vec![("list_skills", "{}")],
    ])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "maxToolSteps": 1, "maxCallsPerRun": 48 }),
    );

    let (run_id, assistant_id, run) = start_run_and_wait(&state, "看看技能").await;
    assert_eq!(run["status"], "completed", "轮数到顶不是 Run 失败：{run}");

    // 只执行了一次（第 2 轮那批被挡在闸门外），而且**如实说了**
    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "到顶那一轮不许执行：{calls:?}");
    assert_eq!(calls[0]["status"], "ok");
    let text = assistant_text(&state, &assistant_id);
    assert!(text.contains("工具轮数上限 1"), "要如实说明：{text}");
    assert!(text.contains("这次要调用的工具没有执行"), "{text}");
    // 那句话要**同时**落进有序块（前端在 parts 非空时只渲染 parts）
    let messages = comfyhub_server_rs::ai::conversations::list_messages(&state, "conv-0").unwrap();
    let parts = &messages
        .iter()
        .find(|m| m.id == assistant_id)
        .unwrap()
        .parts;
    assert!(
        parts.iter().any(|p| p.r#type == "text" && p.text.as_deref().unwrap_or("").contains("工具轮数上限")),
        "补的说明也要落块：{parts:?}"
    );

    gateway.abort();
}

/// **调用额度**（`maxCallsPerRun`）：一轮要两个工具、额度只剩一个 ——
/// 第二个回 `TOOL_BUDGET_EXCEEDED`（**不是**整轮失败），第一个照常执行并落库。
#[tokio::test]
async fn tool_budget_exceeded_is_reported_per_call() {
    let (base_url, gateway) = scripted_tool_gateway(vec![
        vec![("list_skills", "{}"), ("list_skills", "{}")],
        // 额度用尽之后**还在要工具** —— 这时才该把"额度用尽"如实补进正文
        // （模型自己收口的话就不补了：Kotlin 同）
        vec![("list_skills", "{}")],
    ])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "maxToolSteps": 16, "maxCallsPerRun": 1 }),
    );

    let (run_id, assistant_id, run) = start_run_and_wait(&state, "两次技能").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 2, "两次调用都要留痕：{calls:?}");
    // ⚠ 按 `callId` 找，**不要**按下标：同一毫秒里的两行，`ORDER BY started_at, id`
    // 的 tie-break 是随机 UUID（实测踩过：断言写在下标上，红得莫名其妙）
    let first = calls.iter().find(|c| c["callId"] == "call_1").unwrap();
    let second = calls.iter().find(|c| c["callId"] == "call_2").unwrap();
    assert_eq!(first["status"], "ok", "额度之内的那次要真的执行：{first}");
    assert_eq!(second["status"], "failed");
    assert!(
        second["error"]
            .as_str()
            .unwrap()
            .contains("工具调用次数已达上限"),
        "第二次要如实说清是额度拦住的：{}",
        second["error"]
    );
    // 收口时也要把"额度用尽"补进正文（Kotlin `HarnessRunner:644`）
    let text = assistant_text(&state, &assistant_id);
    assert!(text.contains("工具调用次数已达上限（1 次）"), "{text}");
    // 第 3 次那个调用**没有执行**（额度已经用尽）→ 库里只有两行
    assert_eq!(
        comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id)
            .unwrap()
            .len(),
        2,
        "额度用尽之后不许再执行"
    );

    gateway.abort();
}

/// **失败的调用不消耗额度**（Kotlin `spendCall` 只在 handler 返回之后调）：
/// 额度只有 1 次时，先撞一个"工具不存在"的失败，后面那次**仍然跑得动**。
///
/// 反过来的表现是：**"什么都没干就说预算不足"**。
#[tokio::test]
async fn failed_calls_do_not_spend_the_budget() {
    let (base_url, gateway) = scripted_tool_gateway(vec![
        vec![("read_file", r#"{"path":"a.png"}"#)],
        vec![("list_skills", "{}")],
    ])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "maxToolSteps": 16, "maxCallsPerRun": 1 }),
    );

    let (run_id, _, run) = start_run_and_wait(&state, "先失败一次再查技能").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let failed = calls.iter().find(|c| c["callId"] == "call_1").unwrap();
    let ok = calls.iter().find(|c| c["callId"] == "call_2").unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(ok["status"], "ok", "失败那次不该把额度吃掉：{ok}");

    gateway.abort();
}

/// 模型幻觉出一个清单里没有的工具（或这一批还没搬的）：如实回 `TOOL_NOT_FOUND`，
/// **不许静默**、也不许假装跑过 —— 记录照样落库，模型拿到一句能改变做法的话。
#[tokio::test]
async fn unknown_tool_is_reported_as_tool_not_found() {
    // ⚠ 这里用一个**永远不存在**的名字，而不是"还没搬的工具"。
    //
    // 该用例此前三次因"一并移除某个工具"而失败：第 40 步之前用 `read_file`（已移除），
    // 后来用 `comfy_env_report`（§18 搬了），再后来用 `comfy_find_workflow`（同一轮搬了）——
    // 每次红的原因都变成"撞上真的分支"（撞审批闸门 / 真的去提交 / 真的读库），
    // 而那与"未知工具要回 TOOL_NOT_FOUND"一点关系都没有，白花一轮去查。
    // 未知工具本来就不该是"某个真实工具"，所以**给它一个不属于任何工具的名字**，
    // 这条用例从此与"又搬了哪一件"无关。
    const UNKNOWN_TOOL: &str = "comfy_this_tool_never_existed";
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(UNKNOWN_TOOL, r#"{}"#)]]).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (run_id, _, run) = start_run_and_wait(&state, "读个文件").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::ai::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["status"], "failed");
    assert_eq!(calls[0]["approval"], "denied");
    assert!(calls[0]["error"]
        .as_str()
        .unwrap()
        .contains(&format!("没有名为「{UNKNOWN_TOOL}」的工具")));

    // 失败那次**不进 tool.started**（没有开始过），但要有一条 `tool.failed`
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.failed".to_string()), "{types:?}");
    assert!(!types.contains(&"tool.started".to_string()), "{types:?}");

    gateway.abort();
}

/// **提问闸门**（P4 第 32 步）：模型当场问两张卡里的一整张（2 题），
/// 完成作答后，**同一个 Run 接着往下做** —— 不结束、不重开。
///
/// 这一条盯四件事：① `user.question` 的形状是**嵌套**的 `question` 对象
/// （平铺过一次，界面就解不出提问）；② `GET /api/ai/runs/{id}/question` 能把整张卡补回来；
/// ③ 答完写库 + 落块（`asked` / `askedAll`）；④ 答案真的喂回了模型。
#[tokio::test]
async fn ask_user_question_waits_then_continues_the_same_run() {
    let arguments = serde_json::json!({
        "questions": [
            { "question": "要哪个画幅？", "options": ["4:3", "16:9"], "header": "输出尺寸" },
            { "question": "要几张？", "options": ["1", "4"], "multiSelect": true }
        ]
    })
    .to_string();
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(
        "ask_user_question",
        Box::leak(arguments.into_boxed_str()),
    )]])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "permissionMode": "ask", "overrides": {} }),
    );

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "出张图", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    // ② 等提问卡出现（Run 挂在这里等，**不收尾**）
    let mut card: Vec<serde_json::Value> = Vec::new();
    for _ in 0..200 {
        let (status, q) = get_json(
            router_for(state.clone()),
            &format!("/api/ai/runs/{run_id}/question"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{q}");
        if q["pending"] == true {
            card = q["questions"].as_array().cloned().unwrap_or_default();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(card.len(), 2, "整张卡要一次补回来：{card:?}");
    // ① 事件形状：`question` 必须是**对象**（不是问题文字）
    let question_event = state
        .with(|conn| {
            conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'user.question' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let question_event: serde_json::Value = serde_json::from_str(&question_event).unwrap();
    assert!(
        question_event["question"].is_object(),
        "客户端读的是 event.data['question'] 这个**对象**：{question_event}"
    );
    assert_eq!(
        question_event["question"]["questionId"],
        card[0]["questionId"]
    );
    assert_eq!(question_event["question"]["index"], 0);
    assert_eq!(question_event["question"]["total"], 2);
    assert_eq!(question_event["runId"], run_id.as_str());

    // 闸门类事件**立刻落库**（另一台设备靠它发现"有东西在等人"）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"user.question".to_string()), "{types:?}");

    // ③ 回答：第一题点按钮（`optionIndex`），第二题给自由文本
    let first_id = card[0]["questionId"].as_str().unwrap().to_string();
    let second_id = card[1]["questionId"].as_str().unwrap().to_string();
    let (status, answered) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{first_id}/answer"),
        serde_json::json!({ "optionIndex": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["accepted"], true);
    assert_eq!(
        answered["answer"], "16:9",
        "点了按钮，答案就是那个选项的原文"
    );
    let (status, answered) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{second_id}/answer"),
        serde_json::json!({ "answer": "两张就行" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answered["answer"], "两张就行");

    // Run 收尾（**同一个 Run**，不是另起一条）
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 库里两行都是 answered，且记着答了什么（`GET /api/ai/questions/{id}` 据此回放）
    let (status, stored) = get_json(
        router_for(state.clone()),
        &format!("/api/ai/questions/{first_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["answered"], true);
    assert_eq!(stored["answer"], "16:9");
    assert_eq!(stored["optionIndex"], 1);
    assert_eq!(stored["header"], "输出尺寸");
    assert_eq!(stored["total"], 2);

    // ④ 答案喂回了模型（正文是 Kotlin 那两句的逐字形状），而且落进了有序块
    let messages = comfyhub_server_rs::ai::conversations::list_messages(&state, "conv-0").unwrap();
    let assistant = messages.iter().find(|m| m.id == assistant_id).unwrap();
    assert_eq!(assistant.text, "收口了");
    let result_part = assistant
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .expect("工具结果块");
    let text = result_part.text.clone().unwrap_or_default();
    assert!(text.contains("【用户回答了这张卡（共 2 题）】"), "{text}");
    assert!(text.contains("「要哪个画幅？」 → 16:9"), "{text}");
    assert!(text.contains("「要几张？」 → 两张就行"), "{text}");
    let payload = result_part.json_payload.clone().unwrap();
    // 块上的 payload 是**契约那一小撮键**（Kotlin `HarnessRunner:762`）：历史回放靠
    // `question` 这一条把提问卡照原样画出来（问了什么、答了什么）
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["approval"], "not_required");
    assert_eq!(payload["question"]["questionId"], first_id.as_str());
    assert_eq!(payload["question"]["answer"], "16:9");
    assert_eq!(payload["question"]["options"][0], "4:3");

    // 工具的结构化结果（`tool.completed` 的 `result`）才是预算 / 整组答案那一份
    let completed = state
        .with(|conn| {
            conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.completed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    let result = &completed["result"];
    assert_eq!(result["askedCount"], 2);
    assert_eq!(result["answeredCount"], 2);
    assert_eq!(result["answers"][0]["optionIndex"], 1);
    assert_eq!(result["answers"][1]["answer"], "两张就行");
    assert_eq!(result["askedAll"][1]["answer"], "两张就行");
    assert_eq!(
        result["budgetReset"], true,
        "用户真的答了 → 这一窗的工具额度重置"
    );
    assert_eq!(result["callsUsedInWindow"], 0);
    assert!(result["groupId"].is_string());

    // 答完之后 `/api/ai/pending` 不该再列出这张卡
    let (_, pending) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");

    gateway.abort();
}

/// 结尾提问（非阻塞的变种，2026-11-13）：工具跑完**这一轮就结束**——
/// 不再向上游要下一轮（否则卡片后面还会多一段正文），Run 照样是 completed；
/// 提问不随 Run 收尾失效，也不进「有人等着」总览。
#[tokio::test]
async fn ask_follow_up_ends_the_run_and_keeps_questions_pending() {
    let arguments = serde_json::json!({
        "questions": [
            { "question": "跑哪一份？", "options": ["应用内提交的那份", "ComfyUI 里跑的那份"], "header": "工作流" }
        ],
        "submitText": "按这个继续"
    })
    .to_string();
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(
        "ask_follow_up",
        Box::leak(arguments.into_boxed_str()),
    )]])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(
        &state,
        serde_json::json!({ "permissionMode": "ask", "overrides": {} }),
    );

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "用我最近的工作流画一张", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 脚本第 2 轮会回"收口了"：结尾提问若没有结束这一轮，这段话就会被写进正文。
    let messages = comfyhub_server_rs::ai::conversations::list_messages(&state, "conv-0").unwrap();
    let assistant = messages.iter().find(|m| m.id == assistant_id).unwrap();
    assert!(
        !assistant.text.contains("收口了"),
        "结尾提问之后不许再向上游要下一轮：{}",
        assistant.text
    );
    let last = assistant.parts.last().expect("至少要有工具结果块");
    assert_eq!(last.r#type, "tool_result");
    let payload = last.json_payload.as_ref().expect("工具结果要有结构");
    assert_eq!(payload["submitText"], "按这个继续");
    assert_eq!(payload["askedAll"].as_array().unwrap().len(), 1);

    // 提问**不随 Run 收尾失效**（用户下次打开会话还能答）。
    let group = comfyhub_server_rs::ai::questions::pending_group_for(&state, &run_id).unwrap();
    assert_eq!(group.len(), 1, "结尾提问要留成 pending：{group:?}");

    // 也不进「有人等着」总览（它不是闸门，没有 Run 挂在它上面）。
    let (_, pending) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert!(
        pending["asks"].as_array().unwrap().is_empty(),
        "{pending}"
    );

    gateway.abort();
}

/// **跳过**整张卡：模型收到的是"未回答"（不是阻塞），Run 照样收口 ——
/// 且不得伪装为已作答。
#[tokio::test]
async fn skipped_questions_are_reported_as_unanswered() {
    let arguments =
        serde_json::json!({ "question": "要 NSFW 吗？", "options": ["要", "不要"] }).to_string();
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(
        "ask_user_question",
        Box::leak(arguments.into_boxed_str()),
    )]])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "出张图", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let mut question_id = String::new();
    for _ in 0..200 {
        let (_, q) = get_json(
            router_for(state.clone()),
            &format!("/api/ai/runs/{run_id}/question"),
        )
        .await;
        if q["pending"] == true {
            question_id = q["question"]["questionId"].as_str().unwrap().to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!question_id.is_empty(), "提问卡没出现");

    let (status, skipped) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{question_id}/skip"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(skipped["accepted"], true);

    // 跳过**立刻**放行（不等到 30 分钟超时）—— 这一条就是"别把 Run 挂死"的防线
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    let messages = comfyhub_server_rs::ai::conversations::list_messages(&state, "conv-0").unwrap();
    let assistant = messages.iter().find(|m| m.id == assistant_id).unwrap();
    let text = assistant
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .and_then(|p| p.text.clone())
        .unwrap_or_default();
    assert!(text.contains("【用户未回答】"), "{text}");
    assert!(!text.contains("回答：要"), "不许假装用户答了：{text}");

    // 库里那行是 skipped（不是 pending —— 否则 `/api/ai/pending` 会一直列着它）
    let (_, stored) = get_json(
        router_for(state.clone()),
        &format!("/api/ai/questions/{question_id}"),
    )
    .await;
    assert_eq!(stored["answered"], false);
    let (_, pending) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");

    gateway.abort();
}

/// 上游明确拒绝（429）：错误码要归到 `RATE_LIMIT`，而不是笼统的"失败了"。
#[tokio::test]
async fn executor_reports_the_stable_code_for_an_upstream_rejection() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &format!("{base_url}?upstream=http429"));

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "写一段", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "failed");
    assert_eq!(run["errorCode"], "RATE_LIMIT", "{run}");
    assert!(
        run["errorMessage"]
            .as_str()
            .unwrap()
            .contains("rate limited"),
        "上游的原话（已脱敏、已截断）要带出来，否则用户只看到「配置有误」：{run}"
    );

    gateway.abort();
}

/// 未完成的对话也要统计 token（「未完成的对话不统计 token 用量」）：
/// 第一轮**跑完**（usage 已经烧掉）、"接着写"的第二轮撞 429 —— 失败的助手消息
/// 必须带着第一轮的 usage 落库，会话统计（AiUsageSummary 读消息行）才不会少算。
#[tokio::test]
async fn a_failed_run_still_persists_the_usage_of_completed_rounds() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &format!("{base_url}?upstream=usage-then-die"));

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "写长一点", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "failed", "{run}");
    assert_eq!(run["errorCode"], "RATE_LIMIT");

    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["status"], "failed");
    assert_eq!(
        assistant["text"], "你好",
        "第一轮已经说出去的话照旧要落下来（R6）"
    );
    assert_eq!(
        assistant["usage"]["inputTokens"], 7,
        "跑完的那一轮的 usage 必须留在失败的消息上：{assistant}"
    );
    assert_eq!(assistant["usage"]["outputTokens"], 3);
    // 这一条走的是假网关的 `usage-then-die` 分支（那份 usage **没有**嵌套明细）：
    // 认不出来就如实给 0，**不编数字**（契约：没有就是 0）
    assert_eq!(assistant["usage"]["cachedTokens"], 0);
    assert_eq!(assistant["usage"]["reasoningTokens"], 0);

    gateway.abort();
}

/// `POST /api/ai/conversations/{id}/runs` 的完整一条链（Kotlin 顺序照抄）。
#[tokio::test]
async fn start_run_validates_then_creates_messages_and_run() {
    let state = state();
    seed_provider_and_model(&state);

    // ① 空正文（且没附件）→ 400，**一个字节都不写库**
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "   ", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "消息内容不能为空");
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    assert_eq!(
        messages.as_array().unwrap().len(),
        1,
        "被拒的请求不许留下半条消息"
    );

    // ② 会话不存在 → 400
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/nope/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "会话不存在: nope");

    // ③ 模型不在目录里 → `UNKNOWN_MODEL`（**不是** CONFIG_ERROR：前端按码分支）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "nope" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "UNKNOWN_MODEL");
    assert_eq!(body["detail"], "模型不在目录中: nope");

    // ④ 合法的请求 → **202** + 三个主键
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();
    let user_id = body["userMessageId"].as_str().unwrap().to_string();
    assert!(!run_id.is_empty() && !assistant_id.is_empty() && !user_id.is_empty());
    assert_eq!(body.as_object().unwrap().len(), 3, "响应只有这三个键");

    // 请求消息 + 助手占位都落了库，且**请求在前**（顺序是界面的依据）
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let list = messages.as_array().unwrap();
    assert_eq!(list.len(), 3, "夹具 1 条 + 用户 + 助手占位");
    let user = list
        .iter()
        .find(|m| m["id"] == user_id.as_str())
        .expect("用户消息在");
    let assistant = list
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .expect("助手占位在");
    assert_eq!(user["role"], "user");
    assert_eq!(user["text"], "你好");
    assert_eq!(user["seq"], 2);
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["seq"], 3);

    // 执行器是**后台任务**：真实上游是 `127.0.0.1:9`（没人监听），所以它很快会失败。
    // 这条断言钉的是**语义**：Run 绝不会永远停在 running（那会让界面一直转圈）
    let mut run = serde_json::Value::Null;
    for _ in 0..60 {
        let (_, value) =
            get_json(router_for(state.clone()), &format!("/api/ai/runs/{run_id}")).await;
        run = value;
        if run["status"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(run["status"], "failed", "连不上上游要如实失败：{run}");
    assert_eq!(run["errorCode"], "PROVIDER_UNREACHABLE");
    assert_eq!(run["reasoningEffort"], "off");
    assert_eq!(run["userMessageId"], user_id.as_str());
    assert_eq!(run["assistantMessageId"], assistant_id.as_str());
    assert_eq!(run["providerId"], "goat");

    // 失败的助手消息也落了如实的状态（留在 streaming 会让界面一直转圈）
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["status"], "failed");

    // 事件：run.started / message.started / run.failed **都落了库**（断线重连补得回来）
    let types = event_types(&state, &run_id);
    assert_eq!(types[0], "run.started");
    assert_eq!(types[1], "message.started");
    assert_eq!(
        types.last().unwrap(),
        "run.failed",
        "终态事件在最后：{types:?}"
    );
    assert!(
        types.iter().all(|t| !t.is_empty()),
        "seq 从 1 起、连续递增（不许跳号）"
    );

    // 不存在 / 已收尾的 Run 上点取消：**200 + cancelled:false**（不是错误）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cancelled"], false);
    assert_eq!(body["status"], "failed");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/runs/nope/cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Run 不存在: nope");
}

/// 重试：**复用原来那条请求消息**、空壳就地复用、有内容另起一条（AIH-024 / R6）。
#[tokio::test]
async fn retry_reuses_the_user_message_and_never_wipes_a_reply_with_content() {
    let state = state();
    seed_provider_and_model(&state);

    let (_, first) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "再来一张", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let first_run = first["runId"].as_str().unwrap().to_string();
    let first_user = first["userMessageId"].as_str().unwrap().to_string();
    let first_assistant = first["assistantMessageId"].as_str().unwrap().to_string();

    // ⚠ 等第一次 Run 收尾：执行器是后台任务，而"重试"的准入判定读的是**库里那条的状态**。
    // 不等的话会撞上 `streaming`（那条回复还没结束）——那是**对的行为**，只是测试没等它。
    wait_for_run(&state, &first_run).await;

    // ① 空壳（正文空）→ **就地复用**同一条助手消息，不新增气泡
    let (status, retry) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": first_run
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retry}");
    assert_eq!(
        retry["userMessageId"].as_str().unwrap(),
        first_user,
        "复用原来那条用户消息"
    );
    assert_eq!(
        retry["assistantMessageId"].as_str().unwrap(),
        first_assistant,
        "空的失败气泡就地复用（会话里不留空壳）"
    );
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    assert_eq!(
        messages.as_array().unwrap().len(),
        3,
        "重试**不新增**任何消息"
    );
    // 第二次 Run 也在后台跑（上游是 127.0.0.1:9，会失败）—— 等它收尾再往下走
    let second_run = retry["runId"].as_str().unwrap().to_string();
    wait_for_run(&state, &second_run).await;

    // ② 有内容的失败回复 → 原样保留，重试**另起一条**（不得抹除已显示的内容）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_messages SET text = '我已经说了很多话', status = 'failed' WHERE id = ?1",
                rusqlite::params![first_assistant],
            )?;
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text)
                 VALUES (?1, 0, 'text', '我已经说了很多话')",
                rusqlite::params![first_assistant],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, retry2) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": second_run
        }),
    )
    .await;
    assert_ne!(
        retry2["assistantMessageId"].as_str().unwrap(),
        first_assistant,
        "有内容的失败回复**不许**被就地重置"
    );
    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let kept = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == first_assistant.as_str())
        .unwrap();
    assert_eq!(
        kept["text"], "我已经说了很多话",
        "用户看到过的话一个字都不能丢（R6）"
    );
    assert_eq!(
        kept["parts"].as_array().unwrap().len(),
        1,
        "它的工具卡/正文块也留着"
    );

    // ③ 已经答完的回复**不允许**重试（直接发新消息即可）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_messages SET status = 'complete' WHERE id = ?1",
                rusqlite::params![first_assistant],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": second_run
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("不需要（也不能）重试"));
}

/// 假网关：第 1 轮调一个工具，第 2 轮收口；**记录每一轮请求体**（用来断言图有没有真的挂上）。
async fn tool_call_then_reply_gateway(
    tool: &'static str,
    arguments: String,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let recorder = seen.clone();
    let handle = tokio::spawn(async move {
        let mut round = 0usize;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            round += 1;
            let recorder = recorder.clone();
            let arguments = arguments.clone();
            tokio::spawn(async move {
                let request = String::from_utf8_lossy(&read_request(&mut socket).await).to_string();
                recorder.lock().unwrap().push(request);
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let frames: Vec<String> = if round == 1 {
                    vec![
                        format!(
                            r#"{{"id":"chatcmpl-tool","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"call_load","function":{{"name":"{tool}","arguments":""}}}}]}}}}]}}"#
                        ),
                        format!(
                            r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":{}}}}}]}}}}]}}"#,
                            serde_json::to_string(&arguments).unwrap()
                        ),
                        r#"{"choices":[{"finish_reason":"tool_calls"}]}"#.to_string(),
                        "[DONE]".to_string(),
                    ]
                } else {
                    vec![
                        r#"{"choices":[{"delta":{"content":"收口了"}}]}"#.to_string(),
                        r#"{"choices":[{"finish_reason":"stop"}]}"#.to_string(),
                        "[DONE]".to_string(),
                    ]
                };
                for frame in frames {
                    let payload = if frame == "[DONE]" {
                        "data: [DONE]\n\n".to_string()
                    } else {
                        format!("data: {frame}\n\n")
                    };
                    if socket.write_all(payload.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = socket.flush().await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), handle, seen)
}

/// **载入工作流时自动带图**（2026-10-25）：`comfy_load_workflow` 的结果里带 `referenceImages`，
/// 执行器把 `LoadImage` 指向的那张图缩到 ≤1024 长边、编码成 JPEG，**挂到下一轮的 user 轮**上。
///
/// 这条用例盯的是**接线**（单图解析 / 缩图逻辑在 `reference_attach` 的单元用例里）：
/// 第 1 轮请求里不许有图，第 2 轮必须有 `image_url` + 原始分辨率 + 缩图后的尺寸。
#[tokio::test]
async fn load_workflow_auto_attaches_the_reference_image_to_the_next_round() {
    let state = state();
    seed_provider_and_model(&state);
    // 工作流 / 参考图 / input 目录都放**测试自己的**暂存目录，不碰本机 ComfyUI
    let root = comfyhub_server_rs::infra::testpaths::scratch("ref-attach-e2e");
    let input = root.join("input");
    let output = root.join("output");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let reference = input.join("ref.png");
    let image = image::RgbImage::from_pixel(1600, 900, image::Rgb([12, 34, 56]));
    image.save(&reference).unwrap();
    let workflow = root.join("auto-attach.json");
    std::fs::write(
        &workflow,
        serde_json::json!({
            "1": {"class_type": "LoadImage", "inputs": {"image": "ref.png"}},
            "2": {"class_type": "SaveImage", "inputs": {"images": ["1", 0]}}
        })
        .to_string(),
    )
    .unwrap();
    // 目录候选：配置里的 outputDir 的兄弟 `input` 就是上面那份（顺序在 desktop/probe 之前）
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO app_settings (k, v) VALUES ('capture.config', ?1)
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                rusqlite::params![serde_json::json!({
                    "enabled": false,
                    "outputDir": output.to_string_lossy()
                })
                .to_string()],
            )?;
            // 读白名单：这次只认测试自己的目录（工具要读得到那份工作流）
            Ok(())
        })
        .unwrap();
    set_policy(
        &state,
        serde_json::json!({ "readRoots": [root.to_string_lossy()] }),
    );
    // 模型得声明识图能力，否则按准入一条图都不发（那是另一条用例的事）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_models SET input_modalities = '[\"text\",\"image\"]' WHERE model_id = 'm1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (base_url, gateway, seen) = tool_call_then_reply_gateway(
        "comfy_load_workflow",
        serde_json::json!({ "path": workflow.to_string_lossy() }).to_string(),
    )
    .await;
    point_at_gateway(&state, &base_url);

    let (run_id, _assistant_id, run) = start_run_and_wait(&state, "用这份工作流画一张").await;
    assert_eq!(run["status"], "completed", "{run}");

    let bodies = seen.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "两轮：load → 回填 → 收口");
    assert!(
        !bodies[0].contains("image_url"),
        "载入之前不该有图：{}",
        bodies[0]
    );
    let second = &bodies[1];
    assert!(
        second.contains(r#""type":"image_url""#),
        "第二轮必须把参考图挂上：{second}"
    );
    assert!(
        second.contains("data:image/jpeg;base64,"),
        "缩图之后是 JPEG：{second}"
    );
    assert!(
        second.contains("原图 1600×900"),
        "原始分辨率要告诉模型：{second}"
    );
    assert!(
        second.contains("1024×576"),
        "附上的是缩到 1024 长边的图：{second}"
    );

    // 工具审计里也要能查得到（`referenceImages` 是这次调用**真实返回过**的事实）
    let result_json: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT result_json FROM ai_tool_calls
                  WHERE run_id = ?1 AND name = 'comfy_load_workflow'",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    let refs = &payload["referenceImages"];
    assert_eq!(refs[0]["nodeId"], "1", "{payload}");
    assert_eq!(refs[0]["width"], 1600);
    assert_eq!(refs[0]["height"], 900);

    gateway.abort();
    comfyhub_server_rs::infra::testpaths::cleanup(&root);
}

/// **看产物时自动带图**（2026-10-29）：`comfy_view_outputs` 读画廊最近的图片并回
/// `referenceImages`，执行器把它挂到下一轮的 user 轮上 —— 与工作流参考图**同一条链路**。
///
/// 这条用例盯的是执行器里的**工具白名单**：只有 `comfy_load_workflow` 与
/// `comfy_view_outputs` 的结果允许触发附图（别让外来工具结果决定读哪个文件）。
#[tokio::test]
async fn view_outputs_auto_attaches_the_gallery_image_to_the_next_round() {
    let state = state();
    seed_provider_and_model(&state);
    // 模型得声明识图能力，否则按准入一条图都不发（与参考图那条同一档）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_models SET input_modalities = '[\"text\",\"image\"]' WHERE model_id = 'm1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    // 画廊里放一张真图（产物目录就是 state.media_dir()）
    write_test_png(&state.media_dir().join("gallery.png"), 1600, 900);
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO media_assets
                   (id, kind, original_name, stored_name, width, height, created_at, source)
                 VALUES (201, 'IMAGE', 'gallery.png', 'gallery.png', 1600, 900,
                         '2026-10-29T10:00:00.000Z', 'capture')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (base_url, gateway, seen) = tool_call_then_reply_gateway(
        "comfy_view_outputs",
        serde_json::json!({ "mediaId": 201 }).to_string(),
    )
    .await;
    point_at_gateway(&state, &base_url);

    let (run_id, _assistant_id, run) = start_run_and_wait(&state, "看看我刚跑的那张图").await;
    assert_eq!(run["status"], "completed", "{run}");

    let bodies = seen.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "两轮：看产物 → 附图 → 收口");
    assert!(
        !bodies[0].contains("image_url"),
        "调用之前不该有图：{}",
        bodies[0]
    );
    let second = &bodies[1];
    assert!(
        second.contains(r#""type":"image_url""#),
        "第二轮必须把产物图挂上：{second}"
    );
    assert!(
        second.contains("data:image/jpeg;base64,"),
        "缩图之后是 JPEG：{second}"
    );
    assert!(
        second.contains("原图 1600×900"),
        "原始分辨率要告诉模型：{second}"
    );
    assert!(
        second.contains("1024×576"),
        "附上的是缩到 1024 长边的图：{second}"
    );

    // 工具审计里也要能查得到（`referenceImages` 是这次调用**真实返回过**的事实）
    let result_json: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT result_json FROM ai_tool_calls
                  WHERE run_id = ?1 AND name = 'comfy_view_outputs'",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert_eq!(
        payload["referenceImages"][0]["label"], "产物 #201",
        "{payload}"
    );

    gateway.abort();
}

/// `@` 卡片与 `/技能`：请求消息落 mention 块，技能正文由后端现读磁盘。
#[tokio::test]
async fn run_mentions_persist_card_snapshots_and_load_skills_server_side() {
    let state = state();
    seed_provider_and_model(&state);
    let (_, user_root) = skill_dirs(&state);
    std::fs::create_dir_all(user_root.join("demo-skill")).unwrap();
    std::fs::write(
        user_root.join("demo-skill").join("SKILL.md"),
        "---\nname: demo-skill\ndescription: 演示\ndisable-model-invocation: true\n---\n第一步\n第二步",
    )
    .unwrap();

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "照这个做",
            "displayText": "照这个做",
            "providerId": "goat",
            "modelId": "m1",
            "mentions": [
                {
                    "kind": "character_card",
                    "name": "凛",
                    "modelText": "[CHARACTER CARD]\n凛\n银发",
                    "snapshot": { "prompt": "银发", "negative": "低质量" }
                },
                { "kind": "skill", "name": "demo-skill" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let user_id = body["userMessageId"].as_str().unwrap().to_string();
    let run_id = body["runId"].as_str().unwrap().to_string();

    let (_, messages) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let user = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == user_id.as_str())
        .expect("用户消息在");
    assert_eq!(user["text"], "照这个做");
    let parts = user["parts"].as_array().unwrap();
    assert_eq!(parts[0]["type"], "mention");
    assert_eq!(parts[0]["jsonPayload"]["kind"], "character_card");
    assert_eq!(parts[0]["jsonPayload"]["snapshot"]["prompt"], "银发");
    assert_eq!(
        parts[0]["jsonPayload"]["modelText"],
        "[CHARACTER CARD]\n凛\n银发"
    );
    assert_eq!(parts[1]["type"], "mention");
    assert_eq!(parts[1]["jsonPayload"]["kind"], "skill");
    assert!(
        parts[1]["jsonPayload"]["body"]
            .as_str()
            .unwrap()
            .contains("第一步"),
        "技能正文必须由后端从磁盘读出来"
    );
    assert_eq!(parts[2]["type"], "text");
    assert_eq!(parts[2]["text"], "照这个做");

    // Run DTO 能查到这次生效的 skill 快照（以前只写库、谁都不读）
    let (_, run) = get_json(router_for(state.clone()), &format!("/api/ai/runs/{run_id}")).await;
    assert!(
        run.get("skillSnapshot").is_some(),
        "Run 详情要带 skillSnapshot：{run}"
    );

    // 停用后：显式调用也拒绝，且不留下不完整的请求消息
    let (status, _) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/skills/demo-skill/enabled",
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, before) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    let before_count = before.as_array().unwrap().len();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再试一次",
            "providerId": "goat",
            "modelId": "m1",
            "mentions": [{ "kind": "skill", "name": "demo-skill" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "SKILL_DISABLED");
    let (_, after) = get_json(
        router_for(state.clone()),
        "/api/ai/conversations/conv-0/messages",
    )
    .await;
    assert_eq!(
        after.as_array().unwrap().len(),
        before_count,
        "被拒的请求不许留下半条消息"
    );
}
