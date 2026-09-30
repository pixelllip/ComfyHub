//! 会话与消息：读侧、写侧、pending 总览、Run 持久化、事件重放、启动恢复。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;


// ===========================================================================
//  会话与消息的读侧（P4 第 21 步）
// ===========================================================================

/// 会话 / 消息 / **有序块**：形状、顺序、以及"会话不存在"与"会话为空"的分野。
///
/// 夹具里 `conv-0` 只有一条消息（`msg-0` + 一个块）。这里再补一条归档会话、
/// 一条多块消息（text → tool_call → tool_result），把三件最容易静默做错的事钉住：
/// ① 每一条消息内部的块必须按 **ordinal 升序**（工具卡会插在正文之间，乱了看不出来）；
/// ② 消息按 `seq, created_at` 排（同毫秒发的两条只靠 seq）；
/// ③ `includeArchived` 只认 `1` / `true`（别的值当 false），默认不列归档的。
#[tokio::test]
async fn ai_conversations_and_messages_keep_order_and_block_shape() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, provider_id, model_id, archived, created_at, updated_at)
                 VALUES ('conv-1', '未命名', 'p1', 'm1', 0, '2026-09-23T04:00:00.000Z', '2026-09-23T04:30:00.000Z'),
                        ('conv-arch', '归档的', NULL, NULL, 1, '2026-09-23T02:00:00.000Z', '2026-09-23T02:00:00.000Z')",
                [],
            )?;
            // 三条消息：seq 故意与插入顺序不同，验证排序真的按 seq
            conn.execute(
                "INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, provider_id, model_id,
                                          provider_response_id, usage_json, created_at, updated_at)
                 VALUES ('msg-b', 'conv-1', 2, 'assistant', 'complete', '好的', 'p1', 'm1', 'resp-1',
                         '{\"total_tokens\":12}', '2026-09-23T04:10:00.000Z', '2026-09-23T04:10:05.000Z'),
                        ('msg-a', 'conv-1', 1, 'user', 'complete', '画一只猫', NULL, NULL, NULL, NULL,
                         '2026-09-23T04:10:00.000Z', '2026-09-23T04:10:00.000Z')",
                [],
            )?;
            // 块**故意倒序插入**：取出来必须仍是 ordinal 升序
            conn.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text, attachment_id, tool_call_id, json_payload)
                 VALUES ('msg-b', 2, 'tool_result', '工具回来了', NULL, 'call-1', '{\"mediaIds\":[7]}'),
                        ('msg-b', 0, 'text', '先看看', NULL, NULL, NULL),
                        ('msg-b', 1, 'tool_call', NULL, NULL, 'call-1', '{\"name\":\"comfy_submit\"}')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    // ---- 列表：默认不列归档，顺序按 updated_at DESC ----
    let (status, list) = get_json(router_for(state.clone()), "/api/ai/conversations").await;
    assert_eq!(status, StatusCode::OK, "body={list}");
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["conv-1", "conv-0"], "按 updated_at DESC，且归档的默认不出现：{list}");

    let first = &list[0];
    assert_eq!(
        keys(first),
        vec![
            "archived", "createdAt", "id", "messageCount", "modelId", "providerId",
            "systemPromptVersion", "title", "updatedAt",
        ],
        "`systemPromptVersion` / `archived` / `messageCount` 最容易漏（Kotlin encodeDefaults = true）"
    );
    assert_eq!(first["messageCount"], 2);
    assert_eq!(first["archived"], false);
    assert_eq!(first["systemPromptVersion"], "v1");
    // 标题是原样读出来的（NULL → 「新对话」那条回落见 `conversations.rs` 的单元用例：
    // 真 schema 上 `title` 是 NOT NULL，夹具里构造不出 NULL）
    assert_eq!(first["title"], "未命名");
    assert_eq!(first["createdAt"], "2026-09-23T04:00:00Z", "`.000Z` 要按 Java 规则收成整秒");
    assert_eq!(first["updatedAt"], "2026-09-23T04:30:00Z");

    // `includeArchived=1` / `=true` 才列归档的
    let (_, with_arch) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=1").await;
    assert_eq!(with_arch.as_array().unwrap().len(), 3);
    let (_, with_arch2) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=true").await;
    assert_eq!(with_arch2.as_array().unwrap().len(), 3);
    let (_, not_arch) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=yes").await;
    assert_eq!(not_arch.as_array().unwrap().len(), 2, "别的值一律当 false（Kotlin 只认 1/true）");

    // ---- 详情 ----
    let (status, one) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-1").await;
    assert_eq!(status, StatusCode::OK, "body={one}");
    assert_eq!(one["id"], "conv-1");
    assert_eq!(one["providerId"], "p1");
    assert_eq!(one["messageCount"], 2);
    // 没有 provider_id 的会话：可空键**不出现**
    let (_, bare) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-arch").await;
    assert!(!bare.as_object().unwrap().contains_key("providerId"), "{bare}");
    assert!(!bare.as_object().unwrap().contains_key("modelId"));

    // ---- 消息 + 有序块 ----
    let (status, msgs) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-1/messages").await;
    assert_eq!(status, StatusCode::OK, "body={msgs}");
    let msgs = msgs.as_array().unwrap();
    assert_eq!(
        msgs.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["msg-a", "msg-b"],
        "必须按 seq 排，不是插入顺序"
    );
    assert_eq!(
        keys(&msgs[1]),
        vec![
            "conversationId", "createdAt", "id", "modelId", "parts", "providerId",
            "providerResponseId", "role", "seq", "status", "text", "updatedAt", "usage",
        ]
    );
    // 没有 provider / usage 的那条：可空键一个都不出现，但 `parts` 一定在（空数组）
    let user_msg = &msgs[0];
    for absent in ["providerId", "modelId", "providerResponseId", "usage"] {
        assert!(!user_msg.as_object().unwrap().contains_key(absent), "{absent}：{user_msg}");
    }
    assert_eq!(user_msg["parts"], serde_json::json!([]));

    // ★ 块顺序：插入顺序是 2,0,1，取出来必须是 0,1,2 —— 工具卡要插在正文之间
    let parts = msgs[1]["parts"].as_array().unwrap();
    assert_eq!(
        parts.iter().map(|p| p["type"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["text", "tool_call", "tool_result"],
        "块顺序乱了 = 工具卡位置不对（而正文还在，看起来只是『有点怪』）"
    );
    // 每个块：只带自己有的键
    assert_eq!(keys(&parts[0]), vec!["text", "type"]);
    assert_eq!(keys(&parts[1]), vec!["jsonPayload", "toolCallId", "type"]);
    assert_eq!(parts[1]["jsonPayload"], serde_json::json!({"name": "comfy_submit"}));
    assert_eq!(parts[2]["jsonPayload"], serde_json::json!({"mediaIds": [7]}));
    // usage 是**对象**，不是字符串
    assert_eq!(msgs[1]["usage"], serde_json::json!({"total_tokens": 12}));
    assert_eq!(msgs[1]["providerResponseId"], "resp-1");
    assert_eq!(msgs[1]["createdAt"], "2026-09-23T04:10:00Z");
    assert_eq!(msgs[1]["updatedAt"], "2026-09-23T04:10:05Z");

    // ---- 三种"没有"必须分清 ----
    // 会话存在但没有消息 → 200 []
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, archived, created_at, updated_at)
                 VALUES ('conv-empty', '空的', 0, '2026-09-23T06:00:00.000Z', '2026-09-23T06:00:00.000Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, empty) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-empty/messages").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, serde_json::json!([]));

    // 会话不存在 → 400 CONFIG_ERROR（不是 404、不是空数组）
    for path in ["/api/ai/conversations/nope", "/api/ai/conversations/nope/messages"] {
        let (status, err) = get_json(router_for(state.clone()), path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} body={err}");
        assert_eq!(keys(&err), vec!["detail", "error"]);
        assert_eq!(err["error"], "CONFIG_ERROR");
        assert_eq!(err["detail"], "会话不存在: nope");
    }
}

/// 坏 JSON 不许把整段历史打成 500（Kotlin 那边是 `runCatching { … }.getOrNull()`）。
#[tokio::test]
async fn ai_messages_survive_broken_json_columns() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, archived, created_at, updated_at)
                 VALUES ('conv-bad', '脏数据', 0, '2026-09-23T07:00:00.000Z', '2026-09-23T07:00:00.000Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, usage_json, created_at, updated_at)
                 VALUES ('msg-bad', 'conv-bad', 1, 'assistant', 'complete', '正文', '{oops',
                         '2026-09-23T07:00:01.000Z', '2026-09-23T07:00:01.000Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, json_payload)
                 VALUES ('msg-bad', 0, 'tool_call', 'not json')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, msgs) = get_json(router_for(state), "/api/ai/conversations/conv-bad/messages").await;
    assert_eq!(status, StatusCode::OK, "一条脏 json 不该连累整段历史：{msgs}");
    let msg = &msgs[0];
    assert_eq!(msg["text"], "正文");
    assert!(!msg.as_object().unwrap().contains_key("usage"), "坏 usage_json 当『没有』：{msg}");
    assert_eq!(
        keys(&msg["parts"][0]),
        vec!["type"],
        "坏 json_payload 的那一块只留 type（键不出现，不是 null）：{msg}"
    );
}

// ---------------------------------------------------------------------------
//  跨端待办总览（P4 第 25 步）
// ---------------------------------------------------------------------------

/// `GET /api/ai/pending`：**只列"真的还在等人"的**，两个 `askedAt` 类型不同也别改。
#[tokio::test]
async fn pending_lists_only_what_is_really_waiting() {
    let state = state();
    // 夹具：一条 pending 提问、一条已答提问；一个 running Run 上的 pending 审批、
    // 一个已收尾 Run 上的 pending 审批（那是"永远等不到人"的残留）、
    // 以及一个 approved 的调用（不该出现）
    state
        .with(|conn| {
            conn.execute_batch(
                r#"
                INSERT INTO ai_runs (id, conversation_id, status, started_at)
                VALUES ('run-live', 'conv-0', 'running', '2026-09-24T14:00:00.000Z'),
                       ('run-done', 'conv-0', 'failed',  '2026-09-24T13:00:00.000Z');

                INSERT INTO ai_questions
                  (question_id, run_id, conversation_id, group_id, card_index, total,
                   question, options_json, allow_free_text, multi_select, header,
                   status, answer, option_index, option_indexes, asked_at)
                VALUES
                  ('q-1', 'run-live', 'conv-0', 'g-1', 1, 2, '要几张图？', '["1","2","4"]', 1, 0, '张数',
                   'pending', NULL, NULL, NULL, '2026-09-24T14:00:05.647Z'),
                  ('q-2', 'run-live', 'conv-0', 'g-1', 2, 2, '要什么风格？', NULL, 0, 1, NULL,
                   'pending', '国风', 0, '[0,2]', '2026-09-24T14:00:06.000Z'),
                  ('q-3', 'run-live', 'conv-0', 'g-1', 1, 1, '已经答过了', NULL, 1, 0, NULL,
                   'answered', '好的', NULL, NULL, '2026-09-24T13:59:00.000Z');

                INSERT INTO ai_tool_calls
                  (id, run_id, provider_call_id, name, arguments_json, approval, status, started_at)
                VALUES
                  ('tc-1', 'run-live', 'call-live', 'comfy_submit', '{"promptId":3}', 'pending', 'pending', '2026-09-24T14:00:10.000Z'),
                  ('tc-2', 'run-done', 'call-dead', 'write_file', '{}', 'pending', 'pending', '2026-09-24T13:00:10.000Z'),
                  ('tc-3', 'run-live', 'call-ok', 'read_file', '{}', 'approved', 'success', '2026-09-24T14:00:11.000Z');
                "#,
            )
            .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["approvals", "asks"], "顶层就两个键");

    // 提问：只回那两条 pending（已答的 q-3 不在），顺序按 asked_at
    let asks = body["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 2, "只列还没答的：{asks:?}");
    assert_eq!(asks[0]["questionId"], "q-1");
    assert_eq!(asks[1]["questionId"], "q-2");

    // 第一条：带 header、没有 answer / optionIndex / optionIndexes（可空键不出现）
    assert_eq!(
        keys(&asks[0]),
        vec!["allowFreeText", "answered", "askedAt", "conversationId", "groupId", "header",
             "index", "multiSelect", "options", "question", "questionId", "runId", "total"]
    );
    assert_eq!(asks[0]["askedAt"], 1_790_258_405_647_i64, "⚠ 提问的 askedAt 是**毫秒数**");
    assert_eq!(asks[0]["options"], serde_json::json!(["1", "2", "4"]));
    assert_eq!(asks[0]["answered"], false);
    assert_eq!(asks[0]["index"], 1);
    assert_eq!(asks[0]["total"], 2);
    assert_eq!(asks[0]["groupId"], "g-1");

    // 第二条：有 answer / optionIndex / optionIndexes，没有 header
    assert_eq!(asks[1]["answer"], "国风");
    assert_eq!(asks[1]["optionIndex"], 0);
    assert_eq!(asks[1]["optionIndexes"], serde_json::json!([0, 2]));
    assert_eq!(asks[1]["multiSelect"], true);
    assert_eq!(asks[1]["allowFreeText"], false);
    assert!(asks[1].get("header").is_none(), "NULL 的 header 整个键不出现");
    assert!(asks[1].get("options").is_some(), "options 一定是数组（空也是 []）");
    assert_eq!(asks[1]["options"], serde_json::json!([]));

    // 审批：只回 running Run 上那条 pending；已收尾 Run 的残留与 approved 的都不算
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "只列真的还在等的：{approvals:?}");
    assert_eq!(
        keys(&approvals[0]),
        vec!["argumentsJson", "askedAt", "callId", "conversationId", "name", "runId"]
    );
    assert_eq!(approvals[0]["callId"], "call-live");
    assert_eq!(approvals[0]["name"], "comfy_submit");
    // ⚠ 这一份是 **ISO 文本**（与提问那份的毫秒数不同 —— Kotlin 侧就是这样，别"统一"）
    assert_eq!(approvals[0]["askedAt"], "2026-09-24T14:00:10Z");
}

/// 库里没有待办时回**两个空数组**（不是 null、也不是 404）—— 另一台设备据此说"没有在等的"。
#[tokio::test]
async fn pending_is_empty_arrays_when_nothing_waits() {
    let (status, body) = get_json(app(), "/api/ai/pending").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["asks"], serde_json::json!([]));
    assert_eq!(body["approvals"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------
//  会话与消息的写侧（P4 第 24 步）
// ---------------------------------------------------------------------------

/// 建会话 / 改会话 / 删会话：**三种"没有"分清**，且删掉之后消息与块一起没。
#[tokio::test]
async fn conversation_write_side_creates_patches_and_deletes() {
    let state = state();

    // 空标题 / 全空白 → 「新对话」（Kotlin 的 `?:`，不是空串）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations",
        serde_json::json!({ "title": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建会话是 201（Kotlin 同）");
    assert_eq!(body["title"], "新对话");
    assert_eq!(body["archived"], false);
    assert_eq!(body["messageCount"], 0);
    assert_eq!(body["systemPromptVersion"], "v1");
    assert!(body["id"].as_str().unwrap().len() >= 32, "id 是 UUID：{}", body["id"]);

    let (status, created) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations",
        serde_json::json!({ "title": "手机来的会话", "providerId": "p1", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["providerId"], "p1");
    assert_eq!(created["modelId"], "m1");

    // PATCH：给了的字段才改
    let (status, patched) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({ "title": "改过的标题", "archived": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["title"], "改过的标题");
    assert_eq!(patched["archived"], true);
    assert_eq!(patched["providerId"], "p1", "没给的字段不许被清掉");

    // 空 body 的 PATCH = 什么都不改（`null` 与"没给"同义）
    let (status, same) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(same["title"], "改过的标题");
    assert_eq!(same["archived"], true);

    // 空标题 = 不改（不是改成空）
    let (_, same) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({ "title": "  " }),
    )
    .await;
    assert_eq!(same["title"], "改过的标题");

    // 不存在的会话：PATCH / DELETE 都是 **400 CONFIG_ERROR + detail**
    let (status, body) = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/ai/conversations/nope",
        serde_json::json!({ "title": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "会话不存在: nope");

    // DELETE：先给夹具那条会话挂一条消息 + 一个块，验级联真的清了（R1 是前提）
    let (_, msg) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "删我试试" }),
    )
    .await;
    assert!(msg["id"].is_string());
    let (status, body) = send_json(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/conversations/conv-0",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);
    assert_eq!(body["id"], "conv-0");
    let (messages, parts): (i64, i64) = state
        .with(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM ai_messages WHERE conversation_id = 'conv-0'", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM ai_message_parts WHERE message_id = 'msg-0'", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(messages, 0, "删会话要连消息一起删（外键级联）");
    assert_eq!(parts, 0, "…连有序块一起删");
    // 再删一次 → 400（不是静默成功）
    let (status, _) = send_json(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/conversations/conv-0",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 追加消息：`seq` 由服务端分配、消息与块**同一个事务**、非法角色/状态一律 400 带稳定码。
#[tokio::test]
async fn append_message_assigns_seq_and_writes_parts_atomically() {
    let state = state();

    // 量的是"这次追加带来几条"：夹具插的那条消息**本身**就会点亮一次账本
    // （触发器挂在表上，裸 INSERT 同样会触发），所以不能拿总数比。
    let before: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'upsert'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();

    // 调用方给 id（界面按它认自己插的那条气泡）+ 两个块（正文 + 工具卡）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({
            "id": "msg-mine",
            "role": "assistant",
            "text": "好的",
            "parts": [
                { "type": "text", "text": "好的" },
                { "type": "tool_call", "toolCallId": "call-1",
                  "jsonPayload": { "name": "web_search", "arguments": { "query": "x" } } }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "追加消息是 201：{body}");
    assert_eq!(body["id"], "msg-mine", "认调用方给的 id");
    assert_eq!(body["seq"], 2, "夹具里已有 seq=1 的那条");
    assert_eq!(body["status"], "complete");
    assert_eq!(body["parts"].as_array().unwrap().len(), 2);
    assert_eq!(body["parts"][0]["type"], "text");
    assert_eq!(body["parts"][1]["type"], "tool_call");
    assert_eq!(body["parts"][1]["jsonPayload"]["name"], "web_search");
    assert!(body["parts"][1]["jsonPayload"].is_object(), "jsonPayload 是对象，不是字符串");
    // 块里没值的键**不出现**（形状纪律）
    assert!(body["parts"][0].get("toolCallId").is_none());

    // 第二条 → seq 递增；没给 id 就自己生成一个
    let (_, second) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "第二条" }),
    )
    .await;
    assert_eq!(second["seq"], 3);
    assert_ne!(second["id"], "msg-mine");

    // 发消息要**把会话顶上去**（列表按 updated_at 排序，不顶就沉底）
    let (_, conversation) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0").await;
    assert_eq!(conversation["messageCount"], 3, "夹具 1 条 + 新加的 2 条");

    // 非法角色 / 非法状态 / 不存在的会话 → 400 + 稳定码 + 人话
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "wizard", "text": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "非法角色: wizard");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "x", "status": "sleeping" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "非法状态: sleeping");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/nope/messages",
        serde_json::json!({ "role": "user", "text": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "会话不存在: nope");

    // 非法的那几次**一行都没写进去**（校验在事务之前）
    let (_, conversation) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0").await;
    assert_eq!(conversation["messageCount"], 3, "被拒的追加不许留下半截数据");

    // 读回来时块是**按 ordinal 升序**的（工具卡位置不能乱）
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let mine = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "msg-mine")
        .expect("刚追加的那条要在列表里");
    assert_eq!(mine["parts"][0]["text"], "好的");
    assert_eq!(mine["parts"][1]["toolCallId"], "call-1");

    // 写侧**点亮同步账本**（手机靠它增量收到桌面写的会话）—— R3：不许出现假的 delete
    let (upserts, deletes): (i64, i64) = state
        .with(|conn| {
            Ok((
                conn.query_row(
                    "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'upsert'",
                    [],
                    |r| r.get(0),
                )?,
                conn.query_row(
                    "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'delete'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(upserts - before, 2, "两次成功追加 = 两条 upsert");
    assert_eq!(deletes, 0, "**不许**出现假 delete（R3）");
}

/// Run 的一生：建 → 落事件 → 终态；键集合与 SQL 里的时间列都要对。
#[tokio::test]
async fn run_persistence_round_trips_shape_and_terminal_state() {
    use comfyhub_server_rs::ai::runs;
    let state = state();
    seed_provider_and_model(&state);

    let run = runs::create(
        &state,
        runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({
                "id": "goat", "displayName": "山羊", "api": "openai-completions",
                "baseURL": "http://127.0.0.1:9/v1", "endpointTrust": "public",
                "credentialRef": "GOAT_KEY"
            }),
            model_snapshot: serde_json::json!({
                "id": "m1", "displayName": "模型一", "tools": true, "reasoning": false,
                "capabilitySource": "manual", "nsfw": false, "nsfwPromptVersion": "v1"
            }),
            prompt_version: "v13".into(),
            user_message_id: Some("msg-0".into()),
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: Some("off".into()),
            skill_snapshot: Some(serde_json::json!({"count": 0, "skills": []})),
        },
    )
    .expect("建 Run");

    assert_eq!(run.status, "running");
    assert_eq!(run.conversation_id, "conv-0");
    assert_eq!(run.assistant_message_id.as_deref(), Some("assistant-1"));
    assert_eq!(run.user_message_id.as_deref(), Some("msg-0"));
    assert_eq!(run.reasoning_effort.as_deref(), Some("off"));
    assert_eq!(run.prompt_version.as_deref(), Some("v13"));
    assert!(run.started_at.is_some(), "started_at 由 SQL 侧 strftime 写，读完要有值");
    assert!(run.completed_at.is_none());
    // 可空键的**键集合**（少一个 = 界面少一块；多一个 = 前端拿不到类型）
    let value = serde_json::to_value(&run).unwrap();
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "assistantMessageId",
            "conversationId",
            "createdAt",
            "id",
            "modelId",
            "promptVersion",
            "providerId",
            "reasoningEffort",
            "startedAt",
            "status",
            "userMessageId",
        ],
        "Run 的键集合（Kotlin AiRunDto 的 11 个非空键）"
    );
    assert!(value.get("completedAt").is_none(), "还没结束 → 键不出现");
    assert!(value.get("errorCode").is_none() && value.get("retryOfRunId").is_none());

    // 快照真的落了库（事后追溯"当时用的是哪版规则 / 带没带 NSFW"靠它）
    let snapshots: (String, String, String) = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT provider_snapshot, model_snapshot, skill_snapshot FROM ai_runs WHERE id = ?1",
                rusqlite::params![run.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();
    assert!(snapshots.0.contains("\"credentialRef\":\"GOAT_KEY\""), "快照里只有引用名");
    assert!(!snapshots.0.contains("sk-"), "快照里**绝不能**有密钥值（AIH-023）");
    assert!(snapshots.1.contains("\"nsfwPromptVersion\":\"v1\""));
    assert_eq!(snapshots.2, r#"{"count":0,"skills":[]}"#);

    // 非终态不许当终态写（写了僵死 Run 就收不走）
    let bad = runs::finish(&state, &run.id, "running", None, None, None);
    assert!(bad.is_err(), "running 不是终态");

    runs::finish(
        &state,
        &run.id,
        "completed",
        None,
        None,
        Some(&serde_json::json!({"inputTokens": 12, "outputTokens": 34})),
    )
    .expect("落终态");
    let done = runs::get(&state, &run.id).unwrap();
    assert_eq!(done.status, "completed");
    assert!(done.completed_at.is_some());
    let value = serde_json::to_value(&done).unwrap();
    assert!(value.get("errorCode").is_none(), "成功时 errorCode 键不出现");

    // 不存在的 Run → 400 + 稳定码 + 人话（**不是** 404：前端按 error 分支）
    let missing = runs::get(&state, "nope").unwrap_err();
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.error, "CONFIG_ERROR");
    assert_eq!(missing.detail.as_deref(), Some("Run 不存在: nope"));
}

/// 事件：批量写 → `?after=` 续传 → **保留窗口留尾部**（红线 R5）。
#[tokio::test]
async fn run_events_replay_after_seq_and_prune_keeps_the_tail() {
    use comfyhub_server_rs::ai::runs;
    let state = state();
    seed_provider_and_model(&state);
    let run = runs::create(
        &state,
        runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({}),
            model_snapshot: serde_json::json!({}),
            prompt_version: "v13".into(),
            user_message_id: None,
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: None,
            skill_snapshot: None,
        },
    )
    .unwrap();

    // 批量写：batch_size=3，塞 7 条 → 中途写 2 批（6 条），flush 补最后 1 条
    let mut batcher = runs::EventBatcher::new(&state, run.id.clone(), 3);
    for seq in 1..=7 {
        batcher
            .add(seq, "text.delta", &serde_json::json!({ "text": format!("t{seq}") }))
            .unwrap();
    }
    assert_eq!(
        runs::events(&state, &run.id, 0).unwrap().len(),
        6,
        "满一批就写：7 条里前 6 条已经落库"
    );
    assert_eq!(batcher.flush().unwrap(), 1, "收尾必须 flush（否则最后那条 run.completed 会丢）");
    assert_eq!(runs::events(&state, &run.id, 0).unwrap().len(), 7);

    // 断线续传：`?after=5` 只拿 6 / 7（游标语义的唯一读法）
    let tail = runs::events(&state, &run.id, 5).unwrap();
    assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![6, 7]);
    assert_eq!(tail[0].to_sse(), "id: 6\nevent: text.delta\ndata: {\"text\":\"t6\"}\n\n");

    // 幂等：同 (run_id, seq) 再写一遍不产生第二行
    runs::append_event(&state, &run.id, 7, "text.delta", &serde_json::json!({"text": "again"}))
        .unwrap();
    let again = runs::events(&state, &run.id, 0).unwrap();
    assert_eq!(again.len(), 7);
    assert_eq!(again[6].payload["text"], "t7", "重放不许改写已有的那一行");

    // 保留窗口（R5）：keep=3 → 只留最后 3 条，**删的是最老的**
    assert_eq!(runs::prune_events(&state, &run.id, 3).unwrap(), 4);
    let kept = runs::events(&state, &run.id, 0).unwrap();
    assert_eq!(kept.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![5, 6, 7]);
    assert_eq!(
        runs::events(&state, &run.id, 4).unwrap().len(),
        3,
        "窗口左沿之后仍然完整（这正是 R5 要的：重连能拿到最新的一段）"
    );

    // 坏 payload 不让整条回放 500
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_run_events (run_id, seq, event_type, payload_json) VALUES (?1, 8, 'x', 'not json')",
                rusqlite::params![run.id],
            )?;
            Ok(())
        })
        .unwrap();
    let events = runs::events(&state, &run.id, 7).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload, serde_json::json!({}), "脏 payload 当空对象");
}

/// 启动恢复：还在 `running` 的 Run 收成 failed、pending 审批按拒绝收尾。
///
/// 两者是**同一次重启**的两半：内存里的 run 与闸门一起消失，
/// 库里留着它们就是"界面永远转圈 + 永远等不到人点的审批卡"。
#[tokio::test]
async fn startup_recovery_fails_stale_runs_and_denies_pending_approvals() {
    use comfyhub_server_rs::ai::runs;
    let state = state();
    seed_provider_and_model(&state);

    let make = |assistant: &str| {
        runs::create(
            &state,
            runs::NewRun {
                conversation_id: "conv-0".into(),
                provider_id: "goat".into(),
                model_id: "m1".into(),
                provider_snapshot: serde_json::json!({}),
                model_snapshot: serde_json::json!({}),
                prompt_version: "v13".into(),
                user_message_id: None,
                assistant_message_id: assistant.into(),
                retry_of_run_id: None,
                reasoning_effort: None,
                skill_snapshot: None,
            },
        )
        .unwrap()
    };
    let running = make("assistant-1");
    let done = make("assistant-2");
    runs::finish(&state, &done.id, "completed", None, None, None).unwrap();

    // 一个等审批的工具调用（R8：open 的同一刻就落 pending）
    runs::insert_pending_tool_call(&state, &running.id, "call-1", "comfy_submit", r#"{"x":1}"#)
        .unwrap();
    let listing = runs::list_tool_calls(&state, &running.id).unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0]["status"], "pending");
    assert_eq!(listing[0]["approval"], "pending");

    // invoke 返回 → **原地更新那一行**（不新增）
    runs::record_tool_call(
        &state,
        &runs::ToolCallRecord {
            id: "tc-1".into(),
            run_id: running.id.clone(),
            call_id: "call-1".into(),
            name: "comfy_submit".into(),
            arguments_json: r#"{"x":1}"#.into(),
            approval: "approved".into(),
            status: "ok".into(),
            result_json: Some(serde_json::json!({"mediaIds": [7]})),
            content: Some("已提交".into()),
            error: None,
            elapsed_ms: 12,
        },
    )
    .unwrap();
    let listing = runs::list_tool_calls(&state, &running.id).unwrap();
    assert_eq!(listing.len(), 1, "有 pending 行就原地更新，不再插一行");
    assert_eq!(listing[0]["status"], "ok");
    assert_eq!(listing[0]["approval"], "approved");

    // 再挂一个 pending，然后走"后端重启"
    runs::insert_pending_tool_call(&state, &running.id, "call-2", "read_file", "{}").unwrap();
    assert_eq!(runs::fail_stale_running(&state, "后端重启导致 Run 中断").unwrap(), 1);
    assert_eq!(runs::expire_stale_pending_tool_calls(&state).unwrap(), 1);

    let stale = runs::get(&state, &running.id).unwrap();
    assert_eq!(stale.status, "failed");
    assert_eq!(stale.error_code.as_deref(), Some("PROTOCOL_ERROR"));
    assert!(stale.completed_at.is_some());
    assert_eq!(
        runs::get(&state, &done.id).unwrap().status,
        "completed",
        "已经收尾的 Run 不许被重启恢复改写"
    );
    let denied = runs::list_tool_calls(&state, &running.id)
        .unwrap()
        .into_iter()
        .find(|c| c["callId"] == "call-2")
        .unwrap();
    assert_eq!(denied["status"], "denied", "重启 = 最彻底的超时，按拒绝处理");
    assert!(denied["error"].as_str().unwrap().contains("重启"));
}

/// SSE：`?after=` 续传、帧形状、以及"收尾之后把库里的都发完再结束"。
#[tokio::test]
async fn run_events_stream_replays_and_resumes() {
    let state = state();
    seed_provider_and_model(&state);
    let run = comfyhub_server_rs::ai::runs::create(
        &state,
        comfyhub_server_rs::ai::runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({}),
            model_snapshot: serde_json::json!({}),
            prompt_version: "v28".into(),
            user_message_id: None,
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: Some("off".into()),
            skill_snapshot: None,
        },
    )
    .unwrap();

    // 一次真实的流式：三段正文 + 收尾（总线负责发号，收尾走 finish_run）
    state.bus.open(&run.id);
    for text in ["你", "好", "呀"] {
        state
            .bus
            .emit(&state, &run.id, "text.delta", &serde_json::json!({ "messageId": "assistant-1", "text": text }))
            .unwrap();
    }
    comfyhub_server_rs::ai::run_lifecycle::finish_run(
        &state.bus,
        &state,
        &run.id,
        "completed",
        None,
        None,
        None,
    )
    .unwrap();

    // 全量回放：4 条事件、`seq` 从 1 连续到 4、终态在最后
    let body = read_sse(router_for(state.clone()), &format!("/api/ai/runs/{}/events", run.id), "").await;
    let frames = parse_sse(&body);
    assert_eq!(frames.len(), 4, "三段正文 + run.completed：{body}");
    assert_eq!(frames[0].0, Some(1));
    assert_eq!(frames[0].1, "text.delta");
    assert_eq!(frames[0].2["text"], "你");
    assert_eq!(frames[3].1, "run.completed");
    assert_eq!(frames[3].2["runId"], run.id.as_str());
    assert!(!body.contains("event: heartbeat"), "收尾之后立刻结束，不发心跳");

    // 断线续传：`?after=2` 只拿后两条
    let body = read_sse(
        router_for(state.clone()),
        &format!("/api/ai/runs/{}/events", run.id),
        "?after=2",
    )
    .await;
    let frames = parse_sse(&body);
    assert_eq!(frames.iter().map(|f| f.0.unwrap()).collect::<Vec<_>>(), vec![3, 4]);

    // 浏览器 `EventSource` 自动重连带的是 `Last-Event-ID` 头（不认它就永远从 0 重放）
    let body = read_sse_with_headers(
        router_for(state.clone()),
        &format!("/api/ai/runs/{}/events", run.id),
        "",
        &[("last-event-id", "3")],
    )
    .await;
    let frames = parse_sse(&body);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].1, "run.completed");

    // Run 不存在 → 400（**在建立流之前**判：SSE 一旦开始就没法改状态码了）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/runs/nope/events").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Run 不存在: nope");
}
