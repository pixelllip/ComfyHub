//! `submit_queue` 的单元测试（主体代码在 [`submit_queue`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn state() -> AppState {
    // 内存库要**灌真 DDL**：`submit_jobs` 的列名 / 约束 / 默认值就是被测对象的一部分
    // （`workflow_recipe.rs` 的用例同一条纪律）。
    let conn = crate::infra::db::open_memory().unwrap();
    conn.execute_batch(crate::infra::db::schema_sql())
        .expect("建 schema");
    AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"))
}

fn item(payload: Value) -> PushItem {
    PushItem {
        kind: "submit_job".to_string(),
        ref_id: payload
            .get("queueId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        op: Some("upsert".to_string()),
        payload,
    }
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 缺_conversation_id_是永久性拒绝() {
    let state = state();
    let failure = accept(&state, &item(json!({ "queueId": "q1", "promptId": 1 }))).unwrap_err();
    assert_eq!(failure.code, "MISSING_CONVERSATION");
    assert!(
        failure.message.contains("conversationId"),
        "{}",
        failure.message
    );
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 既没有_prompt_id_也没有_source_ref_是永久性拒绝() {
    let state = state();
    let failure = accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1" })),
    )
    .unwrap_err();
    assert_eq!(failure.code, "MISSING_WORKFLOW");
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn ref_id_与_payload_queue_id_不一致要拒() {
    let state = state();
    let mut pushed = item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 1 }));
    pushed.ref_id = "q2".to_string();
    let failure = accept(&state, &pushed).unwrap_err();
    assert_eq!(failure.code, "REF_MISMATCH");
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn overrides_不是对象要拒() {
    let state = state();
    let failure = accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 1, "overrides": [] })),
    )
    .unwrap_err();
    assert_eq!(failure.code, "BAD_PAYLOAD");
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 受理会落一行并把审批挂成_pending() {
    let state = state();
    let accepted = accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 7 })),
    )
    .expect("形状是对的");
    assert_eq!(accepted, Accepted::Fresh);
    let job = load(&state, "q1").expect("要有那一行");
    assert_eq!(job.state, STATE_AWAITING_APPROVAL);
    assert_eq!(job.approval, "pending");
    assert_eq!(job.call_id, "submit:q1");
    assert_eq!(job.prompt_id, Some(7));
    assert_eq!(job.reply_state, "pending");
    // 审批列表里能看见它（复用 /api/ai/pending 的通道）
    let listed = state.with(pending_approvals).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["callId"], json!("submit:q1"));
    assert_eq!(listed[0]["conversationId"], json!("c1"));
    assert_eq!(listed[0]["name"], json!("comfy_submit"));
    assert!(
        listed[0]["askedAt"].is_string(),
        "askedAt 要按 ISO 文本给：{listed:?}"
    );
}

// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 同一个_queue_id_推两次只受理一次() {
    let state = state();
    let payload = json!({ "queueId": "q1", "conversationId": "c1", "promptId": 7 });
    assert_eq!(
        accept(&state, &item(payload.clone())).unwrap(),
        Accepted::Fresh
    );
    assert_eq!(accept(&state, &item(payload)).unwrap(), Accepted::Duplicate);
    let count: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM submit_jobs", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(count, 1, "同一条推两次只有一行");
}

/// **决定只能下一次**：批过之后再也改不动（与工具闸门的 `resolve_approval` 同语义）。
// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 审批决定只能下一次() {
    let state = state();
    accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 7 })),
    )
    .unwrap();
    let first = state
        .write(|tx| Ok(resolve_approval_on(tx, "submit:q1", true)?))
        .unwrap();
    assert_eq!(first, 1);
    let second = state
        .write(|tx| Ok(resolve_approval_on(tx, "submit:q1", false)?))
        .unwrap();
    assert_eq!(second, 0, "已经批过的不许再改成拒绝");
    assert_eq!(load(&state, "q1").unwrap().approval, "approved");
}

/// 审批窗口过了还没人批 → **不提交**（这一条只测状态机；"零上游请求"在集成用例里）。
#[tokio::test]
async fn 没人批准就按时收尾成被拒() {
    let state = state();
    accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 7 })),
    )
    .unwrap();
    let report = run_job(&state, "q1", Duration::from_millis(50)).await;
    assert_eq!(report, JobReport::NotWaiting);
    let job = load(&state, "q1").unwrap();
    assert_eq!(job.state, STATE_DENIED);
    assert_eq!(
        job.approval, "denied",
        "超时也要把 approval 收成 denied，pending 列表才不挂死卡"
    );
    assert_eq!(job.outcome_code.as_deref(), Some(code::APPROVAL_TIMEOUT));
    // `elapsed_ms` 由库自己按 `created_at` 与 `now` 算（跨进程也不会算错）——
    // 它是回执里给用户看的"卡了多久"。**0 就意味着那段 julianday 算不出来**（返回了 NULL）。
    assert!(job.elapsed_ms > 0, "耗时要按真实读数记，不能是 0：{job:?}");
    assert_eq!(
        state
            .with(|conn| Ok(pending_approvals(conn)?.len()))
            .unwrap(),
        0,
        "收尾之后就不该再出现在待审批列表里"
    );
}

/// 启动恢复：还在等人的按拒绝收尾，正在提交的按"结果不确定"收尾，两条都要有回执。
// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 启动恢复把残局收成终态() {
    let state = state();
    accept(
        &state,
        &item(json!({ "queueId": "waiting", "conversationId": "c1", "promptId": 7 })),
    )
    .unwrap();
    accept(
        &state,
        &item(json!({ "queueId": "inflight", "conversationId": "c1", "promptId": 7 })),
    )
    .unwrap();
    state
        .write(|tx| {
            tx.execute(
                "UPDATE submit_jobs SET state='submitting' WHERE queue_id='inflight'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let touched = resume_in_flight(&state);
    assert!(touched >= 2, "两条都该被收尾：{touched}");
    let waiting = load(&state, "waiting").unwrap();
    assert_eq!(waiting.state, STATE_DENIED);
    assert_eq!(
        waiting.outcome_code.as_deref(),
        Some(code::APPROVAL_INTERRUPTED)
    );
    let inflight = load(&state, "inflight").unwrap();
    assert_eq!(inflight.state, STATE_FAILED);
    assert_eq!(
        inflight.outcome_code.as_deref(),
        Some(code::SUBMIT_INTERRUPTED)
    );
    assert!(inflight.outcome_message.unwrap().contains("结果不确定"));
}

/// 看门狗：租约过期的 `submitting` 会被如实收尾（不重投），没结果的 `awaiting` 按超时拒绝。
// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 看门狗按租约收尾不死等() {
    let state = state();
    accept(
        &state,
        &item(json!({ "queueId": "stuck", "conversationId": "c1", "promptId": 7 })),
    )
    .unwrap();
    state
        .write(|tx| {
            // 造一个"worker 早就死了"的现场：状态是提交中、心跳停在两小时前
            tx.execute(
                "UPDATE submit_jobs SET state='submitting',
                        heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ','now','-2 hours')
                  WHERE queue_id='stuck'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let report = sweep(&state);
    assert!(report.finalized >= 1, "{report:?}");
    let job = load(&state, "stuck").unwrap();
    assert_eq!(job.state, STATE_FAILED);
    assert_eq!(job.outcome_code.as_deref(), Some(code::SUBMIT_INTERRUPTED));
}

/// 会话不存在 → **保留队列** + 如实记错误（别删、别假装成功）；会话补上来之后能补发。
// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 会话还没同步上来时回执留着等() {
    let state = state();
    accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "late", "promptId": 7 })),
    )
    .unwrap();
    state
        .write(|tx| {
            tx.execute(
                "UPDATE submit_jobs
                    SET state='done', ok=1, outcome_message='跑完了', media_ids_json='[3]',
                        approval='approved'
                  WHERE queue_id='q1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    assert_eq!(deliver_pending(&state), 0, "会话不在，谁也送不出去");
    let job = load(&state, "q1").unwrap();
    assert_eq!(job.reply_state, "blocked");
    assert!(
        job.reply_error.unwrap().contains("还没同步到桌面"),
        "要说清为什么"
    );

    // 会话同步上来了（手机把那条会话的行推上来）→ 下一轮清扫补发
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_conversations (id, title) VALUES ('late','手机那条')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(deliver_pending(&state), 1, "会话到了就该补发");
    let job = load(&state, "q1").unwrap();
    assert_eq!(job.reply_state, "delivered");
    let count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_messages WHERE conversation_id='late'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1);
    // 幂等：再扫一轮不会冒出第二条（已送达的不进待投递清单）
    assert_eq!(deliver_pending(&state), 0, "已送达的不用再管");
    let count: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_messages WHERE conversation_id='late'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1, "回执只许有一条");
}

/// 回执消息的形状：一句人话 + `tool_result` 块（`mediaIds` 是真的才带）。
// ⚠ 一律 `#[tokio::test]`：`accept` 会起后台任务（`tokio::spawn`），
//   没有运行时会在**落库成功之后**炸在 spawn 上 —— 那正是 R8 顺序的反面。
#[tokio::test]
async fn 回执消息是助手消息加一个工具结果块() {
    let state = state();
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_conversations (id, title) VALUES ('c1','会话')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    accept(
        &state,
        &item(json!({ "queueId": "q1", "conversationId": "c1", "promptId": 7, "nsfw": true })),
    )
    .unwrap();
    state
        .write(|tx| {
            tx.execute(
                "UPDATE submit_jobs
                    SET state='done', ok=1, approval='approved',
                        outcome_message='跑完了，产物已入库（1 个）。', media_ids_json='[42]'
                  WHERE queue_id='q1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(deliver_pending(&state), 1);

    let messages = crate::ai::conversations::list_messages(&state, "c1").unwrap();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.role, "assistant");
    assert_eq!(message.text, "跑完了，产物已入库（1 个）。");
    let types: Vec<&str> = message.parts.iter().map(|p| p.r#type.as_str()).collect();
    assert_eq!(types, vec!["text", "tool_call", "tool_result"]);
    let result = message.parts[2]
        .json_payload
        .as_ref()
        .expect("要有结构化负载");
    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["mediaIds"], json!([42]));
    assert_eq!(result["name"], json!("comfy_submit"));
    assert_eq!(result["queueId"], json!("q1"));
    assert!(message.parts[2].tool_call_id.as_deref() == Some("submit:q1"));
    // `nsfw=true` 是受理时存的，工具卡上显示的参数与真正提交的那份同源
    let args = message.parts[1].json_payload.as_ref().unwrap();
    assert!(
        args["arguments"]
            .as_str()
            .unwrap()
            .contains("\"nsfw\":true"),
        "{args}"
    );
}

// ---------------------------------------------------------------------------
//  免桌面批准（按设备放权，`autoApproveSubmits`）
// ---------------------------------------------------------------------------

fn seed_device(state: &AppState, id: &str, auto_approve: bool) {
    state
        .with(|conn| {
            let mut file = crate::mobile::auth::load(conn);
            file.devices.push(crate::mobile::auth::DeviceRecord {
                id: id.to_string(),
                token: format!("token-{id}"),
                auto_approve_submits: auto_approve,
                ..Default::default()
            });
            crate::mobile::auth::save(conn, &file)
        })
        .unwrap();
}

#[tokio::test]
async fn 设备开了自动提交时_受理直接落_auto_且不再进待办() {
    let state = state();
    seed_device(&state, "phone-auto", true);
    accept(
        &state,
        &item(json!({
            "queueId": "q-auto",
            "conversationId": "c1",
            "promptId": 1438,
            "requestedBy": "phone-auto"
        })),
    )
    .unwrap();

    let approval: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT approval FROM submit_jobs WHERE queue_id = 'q-auto'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(approval, "auto", "开了自动提交就不能再挂 pending");
    // `auto` 不是"待审批"：桌面上不该再多一张点了也没用的卡。
    let pending = state.with(pending_approvals).unwrap();
    assert!(pending.is_empty(), "{pending:?}");
}

#[tokio::test]
async fn 设备没开自动提交时_仍然挂_pending() {
    let state = state();
    seed_device(&state, "phone-manual", false);
    accept(
        &state,
        &item(json!({
            "queueId": "q-manual",
            "conversationId": "c1",
            "promptId": 1438,
            "requestedBy": "phone-manual"
        })),
    )
    .unwrap();

    let approval: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT approval FROM submit_jobs WHERE queue_id = 'q-manual'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(approval, "pending", "默认必须等人批准");
    assert!(!claim(&state, "q-manual"), "没批准不许认领");
    let pending = state.with(pending_approvals).unwrap();
    assert_eq!(pending.len(), 1);
}

#[tokio::test]
async fn claim_认_approved_与_auto_不认_pending() {
    let state = state();
    for (queue, approval) in [("q-a", "approved"), ("q-b", "auto"), ("q-c", "pending")] {
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO submit_jobs
                       (queue_id, call_id, conversation_id, payload_json, state, approval)
                     VALUES (?1, ?2, 'c1', '{}', 'awaiting_approval', ?3)",
                    rusqlite::params![queue, format!("submit:{queue}"), approval],
                )?;
                Ok(())
            })
            .unwrap();
    }
    assert!(claim(&state, "q-a"));
    assert!(claim(&state, "q-b"), "auto 必须能被 worker 认领");
    assert!(!claim(&state, "q-c"));
}
