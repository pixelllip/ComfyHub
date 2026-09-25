//! 桌面执行器（P4 第 30 步）—— 一次 Run 的**主循环**。
//!
//! 端点（`POST /runs` / SSE / cancel）在第 28 步就位，上游客户端在 [`crate::upstream`]。
//! 这里把它们连起来：**取上下文 → 请求上游 → 流式落库 → 收尾**。
//!
//! # 主循环要守的三条纪律（R6）
//!
//! 1. **正文必须落块**：`ai_messages.text` 里有的内容，`ai_message_parts` 里也必须有
//!    —— 前端在 parts 非空时**只渲染 parts**，只写正文不落块 = 用户看不见。
//! 2. **失败 / 取消时把已经说出去的话落下来**：流式期间正文是攒在内存里的，
//!    收尾那一刻（正常 / 失败 / 取消**三条路**）必须写进库，否则一次 429 就把它吞掉。
//! 3. **收尾只有一条路**：[`finish_run`]（落终态 + flush 事件 + 发终态事件 + 关流）。
//!    少了任何一步的表现都是"界面永远转圈"或"最后那句话补不回来"。
//!
//! # 与 Kotlin 的一处刻意差异：**不带工具**
//!
//! 工具层（`comfy_*` / `read_file` / MCP 那 39 个）还没搬（第 31 步）。所以这一版
//! **既不下发工具、系统提示里也不说"可调用的工具"** —— 前后一致。
//! 反过来（提示词里说了、实际调不了）会让模型去调一个必然失败的工具，
//! 那比"暂时没有工具"糟得多。手机 harness 不受影响：它自带执行器与工具层。
//!
//! # 取消
//!
//! `POST /runs/{id}/cancel` 立旗标（[`Executor::request_cancel`]），主循环在**安全点**
//! （每读一个上游分片之前）看到旗标就**收尾**：已经说出去的话照落、终态记 `cancelled`。
//! 直接强杀协程是错的 —— 那会跳过第 2 条纪律。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::conversations::{AiMessageDto, AiMessagePartDto};
use crate::events::event_type;
use crate::protocol::{Api, ChatTurn, ReasoningRequest, StreamEvent, ThinkingFormat};
use crate::routes::{ApiError, AppState};
use crate::run_lifecycle::{finish_run, ModelRow};
use crate::upstream::{stream_chat, ChatRequest};

/// 正文超过这个长度就请求"接着写"（Kotlin `HarnessRunner.CONTINUE_INSTRUCTION` 的触发条件）。
pub const CONTINUE_CHARS: usize = 12_000;

/// 正在跑的 Run 的取消旗标。
///
/// 与 Kotlin `HarnessRunner` 的 `cancelled` 集合同一件事：立旗标 → 主循环在安全点自己收尾。
pub struct Executor {
    running: Mutex<HashSet<String>>,
    cancelled: Mutex<HashSet<String>>,
}

impl Default for Executor {
    fn default() -> Self {
        Self { running: Mutex::new(HashSet::new()), cancelled: Mutex::new(HashSet::new()) }
    }
}

impl Executor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_running(&self, run_id: &str) {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(run_id.to_string());
    }

    pub fn mark_done(&self, run_id: &str) {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(run_id);
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).remove(run_id);
    }

    /// 立取消旗标；返回 false 表示这次 Run 不在这台进程里跑（没什么可取消的）。
    pub fn request_cancel(&self, run_id: &str) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if !running.contains(run_id) {
            return false;
        }
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).insert(run_id.to_string());
        true
    }

    pub fn is_cancelled(&self, run_id: &str) -> bool {
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).contains(run_id)
    }
}

/// 起一次 Run：开一个后台任务跑主循环。
///
/// `effort_wire` 是**要发给上游的**思考强度（`None` = 一个思考参数都不发）。
/// 落库的那个值（`off` / 等级名）与它是两回事 —— 见 `run_lifecycle::thinking_effort`。
pub fn spawn(
    state: &AppState,
    run_id: &str,
    provider: &ModelRow,
    model: &ModelRow,
    effort_wire: Option<String>,
    text: String,
) {
    let executor = state.executor.clone();
    if executor.is_cancelled(run_id) {
        // 建 Run 与起任务之间被取消了：别启动，直接收尾
        finish_quietly(state, run_id, "cancelled", None, None, None);
        return;
    }
    executor.mark_running(run_id);
    let state = state.clone();
    let run_id = run_id.to_string();
    let provider = provider.clone();
    let model = model.clone();
    tokio::spawn(async move {
        // ⚠ **顺序要紧**：先把 `run.started` / `message.started` 落下来，再去连上游。
        // 反过来的话，连不上时 `run.failed` 会先落库、`seq` 就从它开始 ——
        // 而界面与契约都假定第一条是 `run.started`（`seq` 从 1 起说的是**这个** 1）。
        let run = match crate::runs::get(&state, &run_id) {
            Ok(run) => run,
            Err(e) => {
                eprintln!("执行器：读不到刚建好的 Run {run_id}：{}", e.human());
                state.executor.mark_done(&run_id);
                return;
            }
        };
        let assistant_id = run.assistant_message_id.clone().unwrap_or_default();
        let started = started_payload(
            &run_id,
            &run.conversation_id,
            run.reasoning_effort.clone(),
            &provider,
            &model,
            crate::run_kit::nsfw_enabled(&state).unwrap_or(false),
        );
        if let Err(e) = state.bus.emit(&state, &run_id, event_type::RUN_STARTED, &started) {
            eprintln!("执行器：发 run.started 失败：{}", e.human());
        }
        if !assistant_id.is_empty() {
            let _ = state.bus.emit(
                &state,
                &run_id,
                event_type::MESSAGE_STARTED,
                &json!({ "messageId": assistant_id, "role": "assistant" }),
            );
        }
        let outcome = run_once(&state, &run_id, &provider, &model, effort_wire, &text).await;
        if let Err(e) = outcome {
            let code = e.error.clone();
            let detail = e.human().to_string();
            eprintln!("Run {run_id} 失败（code={code}）：{detail}");
            // 失败也要**先把已经说出去的话落下来**（R6），再收尾
            finish_quietly(&state, &run_id, "failed", Some(&code), Some(&detail), None);
        }
        state.executor.mark_done(&run_id);
    });
}

/// `run.started` 的 payload（字段与 Kotlin `HarnessRunner` **逐字一致**）。
///
/// 单独一个函数是为了让"发事件的时机"与"payload 拼装"分开 ——
/// 这一条事件的键少一个，界面就少显示一块（NSFW 标记 / 思考强度 / 提示词版本）。
fn started_payload(
    run_id: &str,
    conversation_id: &str,
    reasoning_effort: Option<String>,
    provider: &ModelRow,
    model: &ModelRow,
    nsfw: bool,
) -> Value {
    json!({
        "runId": run_id,
        "conversationId": conversation_id,
        "providerId": provider.id,
        "modelId": model.id,
        "promptVersion": crate::nsfw_prompt::PROMPT_VERSION,
        "nsfw": nsfw,
        "nsfwPromptVersion": crate::nsfw_prompt::VERSION,
        "reasoningEffort": reasoning_effort,
        "tools": {},
    })
}

/// 收尾包装：任何一步出错都只记日志（**不 panic、不把上一步的成果丢掉**）。
fn finish_quietly(
    state: &AppState,
    run_id: &str,
    status: &str,
    code: Option<&str>,
    message: Option<&str>,
    usage: Option<&Value>,
) {
    if let Err(e) = finish_run(&state.bus, state, run_id, status, code, message, usage) {
        eprintln!("执行器：收尾失败（Run 可能停在 running）：{}", e.human());
    }
}

/// 主循环（同步本体，**测试直接调它**）。
async fn run_once(
    state: &AppState,
    run_id: &str,
    provider: &ModelRow,
    model: &ModelRow,
    effort_wire: Option<String>,
    _text: &str,
) -> Result<(), ApiError> {
    let run = crate::runs::get(state, run_id)?;
    let assistant_id = run.assistant_message_id.clone().unwrap_or_default();
    let conversation_id = run.conversation_id.clone();

    // ---- 上下文（在发请求之前一次取齐）--------------------------------------
    let history = crate::conversations::list_messages(state, &conversation_id)?;
    let nsfw = crate::run_kit::nsfw_enabled(state)?;
    let policy = crate::policy::ToolPolicy::load(state)?;
    let storage = state.media_dir().parent().unwrap_or(state.media_dir()).to_path_buf();
    let skills = crate::skills::catalog(
        &state.project_root().join("skills").join("builtin"),
        &storage.join("ai").join("skills"),
    );
    let memory_text = crate::memory::prompt_text(&storage.join("ai"));

    // 工具清单：**只下发这一批真的实现了的**（`declaredTo = kit ∩ 本地实现`），
    // 并且按权限策略把 `deny` 的剔掉 —— 系统提示里那段"可调用的工具"读的是**同一个**清单。
    let tools = crate::tools::specs(&policy);
    let tool_refs: Vec<&crate::mobile_tools::MobileToolSpec> = crate::mobile_tools::MOBILE_TOOLS
        .iter()
        .filter(|t| tools.iter().any(|spec| spec.name == t.name))
        .collect();
    let system_prompt = crate::system_prompt::render(
        &provider.display_name,
        &provider.api,
        &model.id,
        &tool_refs,
        &skills,
        &policy,
        &memory_text,
        false,
        nsfw,
    );

    let mut turns = vec![ChatTurn { role: "system".into(), content: system_prompt, ..Default::default() }];

    // 起标题的附加指令：**只按轮带、且插在系统提示之后（第 2 条）**。
    // 两条理由都踩过（AGENTS §10.1）：常驻会让后面每一轮都再吐一次标记（漏进正文）；
    // 挂末尾会挡住"最后一条消息是工具结果"这个事实（模型以为还没干活）。
    let user_count = history.iter().filter(|m| m.role == "user").count();
    if user_count <= 1 {
        turns.push(ChatTurn {
            role: "system".into(),
            content: crate::nsfw_prompt::TITLE_INSTRUCTION.to_string(),
            ..Default::default()
        });
    }

    // 历史（**不含**当前那条助手占位）：只带正文与工具轮，备注类不进上下文
    for message in history.iter().filter(|m| m.id != assistant_id) {
        if message.role == "system_note" {
            continue;
        }
        turns.extend(turns_of(message));
    }
    // 当前助手那条：把**已经说出去的部分**作为上下文（重试时的续写依据）
    if let Some(current) = history.iter().find(|m| m.id == assistant_id) {
        let already: String = current
            .parts
            .iter()
            .filter(|p| p.r#type == "text")
            .filter_map(|p| p.text.clone())
            .collect();
        if !already.trim().is_empty() {
            turns.push(ChatTurn { role: "assistant".into(), content: already, ..Default::default() });
        }
    }

    // 系统提示里那段工具说明要跟着实际下发的工具走（`render` 收到的是同一个清单）
    let api = Api::parse(&provider.api).ok_or_else(|| {
        ApiError::ai("CONFIG_ERROR", format!("Provider 协议非法: {}", provider.api))
    })?;

    // `std::sync::Mutex`（不是 tokio 的那个）是**刻意的**：回调是同步的，
    // 而锁只在 `lock()` 与 `drop()` 之间持有、**从不跨 await** —— 那正是 std 锁的用法。
    // 有序块按"发生的顺序"攒（思考 → 正文 → 工具卡 → 工具结果），收尾一次写完。
    let sink = Arc::new(Mutex::new(Sink::default()));

    // 上游要的思考强度：`off` → 一个参数都不发
    let reasoning_request = ReasoningRequest {
        effort: effort_wire.clone(),
        wire_value: effort_wire.clone(),
        format: model.thinking_format.as_deref().and_then(ThinkingFormat::parse),
    };

    // ---- 工具往返循环 -------------------------------------------------------
    //
    // 一轮 = 发一次上游 → 收正文/思考/工具调用 → 若有工具就执行完再发下一轮。
    // 两条预算与 Kotlin 逐条对齐（`ToolPolicyConfig`）：
    //  * **轮数** `maxToolSteps`（出厂 16）—— 到顶那一轮**不执行**工具，
    //    而是把"已达轮数上限、再发一条就能接着做"如实补进正文（**不是**把 Run 判失败：
    //    用户已经看到的那段回答不该挂红叉，那件事本来只是"这轮到此为止"）；
    //  * **次数** `maxCallsPerRun`（出厂 48，硬天花板 [`crate::tools::ABSOLUTE_CALL_CAP`]）——
    //    用尽之后这一轮只收口，再要工具就回 `TOOL_BUDGET_EXCEEDED`。
    let max_steps = policy.config.max_tool_steps.max(1) as usize;
    let max_calls = policy.config.max_calls_per_run.max(1);
    let mut session = crate::tools::ToolSession::default();
    let mut trailing: Vec<ChatTurn> = Vec::new();
    let mut total_usage: Option<Value> = None;
    let mut last_provider_response_id: Option<String> = None;
    let mut step = 0usize;
    let mut calls_made = 0i64;
    let mut budget_exhausted = false;

    loop {
        if state.executor.is_cancelled(run_id) {
            let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
            sink_now(state, &assistant_id, &sink)?;
            crate::runs::update_message(state, &assistant_id, &sink.text_body(), "cancelled", None, None)?;
            drop(sink);
            finish_run(&state.bus, state, run_id, "cancelled", None, None, None)?;
            return Ok(());
        }

        let mut turns_now = turns.clone();
        turns_now.extend(trailing.iter().cloned());

        let outcome = {
            let sink = sink.clone();
            let state_for_cb = state.clone();
            let run = run_id.to_string();
            let assistant = assistant_id.clone();
            stream_chat(
                ChatRequest {
                    state,
                    api,
                    base_url: &provider.base_url,
                    model: &model.id,
                    credential_ref: provider.credential_ref.as_deref(),
                    turns: &turns_now,
                    tools: &tools,
                    reasoning: reasoning_request.clone(),
                    max_output_tokens: model.max_output_tokens,
                },
                move |event| {
                    // ⚠ 这个闭包**不能 await**（`stream_chat` 的签名要求同步回调）——
                    // 逐字落库是"攒在内存、收尾一次写"（`Sink`），事件本身走总线（攒批）。
                    let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
                    let emit = |payload: Value, ty: &str| -> Result<(), ApiError> {
                        state_for_cb.bus.emit(&state_for_cb, &run, ty, &payload)?;
                        Ok(())
                    };
                    match event {
                        StreamEvent::TextDelta(chunk) => {
                            sink.push_text(&chunk);
                            emit(json!({ "messageId": assistant, "text": chunk }), event_type::TEXT_DELTA)?;
                        }
                        StreamEvent::ReasoningDelta(chunk) => {
                            sink.push_reasoning(&chunk);
                            emit(
                                json!({ "messageId": assistant, "text": chunk }),
                                event_type::REASONING_DELTA,
                            )?;
                        }
                        // 工具调用的分片在 [`crate::upstream::Outcome::tool_calls`] 里拼装，
                        // 这里不发事件 —— `tool.requested` / `tool.completed` 由执行器
                        // 在**真的开始处理**那个调用时发（那才是界面该看到的东西）
                        _ => {}
                    }
                    Ok(())
                },
                || state.executor.is_cancelled(run_id),
            )
            .await
        };

        let idle = match outcome {
            Ok(idle) => idle,
            Err(e) => {
                // **失败也要把已经说出去的话落下来**（R6 第 2 条）——
                // 一次 429 不该让用户已经看到的那段话凭空消失。
                let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
                sink_now(state, &assistant_id, &sink)?;
                crate::runs::update_message(state, &assistant_id, &sink.text_body(), "failed", None, None)?;
                return Err(e);
            }
        };

        total_usage = Some(crate::upstream::merge_usage(
            total_usage,
            idle.usage.as_ref().unwrap_or(&json!({})),
        ));
        if idle.provider_response_id.is_some() {
            last_provider_response_id = idle.provider_response_id.clone();
        }

        // 这一轮有没有工具调用？没有就收尾（正常结束）
        let mut accumulator = crate::protocol::ToolCallAccumulator::new();
        for (index, key, call_id, name, fragment) in &idle.tool_calls {
            accumulator.push(*index, key, call_id.as_deref(), name.as_deref(), fragment.as_deref());
        }
        let calls = accumulator.finish();
        if calls.is_empty() {
            break;
        }

        // 额度用尽（用户建议 ③）：**下一轮只让模型收口**，不再执行工具。
        // 如实把"为什么没继续"补进正文 —— 悄悄丢掉那批工具调用等于假装它答完了。
        if budget_exhausted {
            let note = format!(
                "\n\n（{}）",
                crate::tools::budget_exceeded_note(
                    max_calls,
                    calls_made < crate::tools::ABSOLUTE_CALL_CAP
                )
            );
            push_note(state, run_id, &assistant_id, &sink, &note)?;
            break;
        }
        // 轮数到顶：同样**不执行**，同样如实说明（与上面那句措辞刻意分开）
        if step >= max_steps {
            let note = format!(
                "\n\n（已达到本次回复的工具轮数上限 {max_steps}，这次要调用的工具没有执行；\
                 再发一条消息它就能接着做。）"
            );
            push_note(state, run_id, &assistant_id, &sink, &note)?;
            break;
        }

        // 助手这一轮的正文 + 工具调用，作为**下一轮的上下文**
        let round_text = sink.lock().unwrap_or_else(|e| e.into_inner()).take_text();
        trailing.push(ChatTurn {
            role: "assistant".into(),
            content: round_text,
            tool_calls: calls.iter().map(|(_, call)| call.clone()).collect(),
            tool_call_id: None,
        });

        // 一个一个执行（**串行**：多个调用里只要有一个要用户点头，就得按顺序来）
        for (call_id, call) in &calls {
            let outcome = handle_call(
                state,
                run_id,
                &assistant_id,
                &policy,
                &mut session,
                call_id,
                call,
                &sink,
                calls_made,
                max_calls,
            )
            .await?;
            calls_made += 1;
            // 工具结果作为下一轮的 `role = tool`
            trailing.push(ChatTurn {
                role: "tool".into(),
                content: outcome.content,
                tool_calls: Vec::new(),
                tool_call_id: Some(call_id.clone()),
            });        }

        step += 1;
        if calls_made >= max_calls || calls_made >= crate::tools::ABSOLUTE_CALL_CAP {
            budget_exhausted = true;
        }
    }

    // ---- 落库 --------------------------------------------------------------
    let body = {
        let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        sink_now(state, &assistant_id, &sink)?;
        sink.text_body()
    };

    // usage 事件的形状：归一到 OpenAI 形状（界面读的是它）
    if let Some(usage) = total_usage.as_ref() {
        state
            .bus
            .emit(state, run_id, event_type::USAGE_UPDATED, &json!({ "usage": usage }))?;
    }

    let provider_response_id = last_provider_response_id
        .unwrap_or_else(|| format!("{}-{}", provider.id, run_id));
    crate::runs::update_message(
        state,
        &assistant_id,
        &body,
        "complete",
        Some(&provider_response_id),
        total_usage.as_ref(),
    )?;
    state.bus.emit(
        state,
        run_id,
        event_type::MESSAGE_COMPLETED,
        &json!({
            "messageId": assistant_id,
            "status": "complete",
            "text": body,
            "usage": total_usage,
        }),
    )?;

    finish_run(&state.bus, state, run_id, "completed", None, None, total_usage.as_ref())?;
    Ok(())
}

/// 审批等待的上限（秒）。超时 = 拒绝（与 Kotlin 的 `ToolApprovalGate` 同语义）。
pub const APPROVAL_TIMEOUT_SECONDS: u64 = 300;
/// 审批轮询间隔（毫秒）—— 闸门读的是**库**（跨端可见），所以是轮询而不是通道。
pub const APPROVAL_POLL_MS: u64 = 150;

/// 一次工具调用的结果（Kotlin `ToolCallRecord` 里落库与发事件真正用到的那几项）。
struct CallOutcome {
    /// 喂回模型 / 落进 `ai_message_parts.text` 的正文
    content: String,
    /// 工具卡上的一行摘要（Kotlin `preview`）
    preview: String,
    ok: bool,
    /// 失败时的稳定码（`APPROVAL_DENIED` / `TOOL_NOT_FOUND` / `TOOL_BUDGET_EXCEEDED` …）
    code: Option<String>,
    /// `not_required` / `approved` / `denied`
    approval: String,
    /// 结构化结果（成功时是工具给的 `payload`；失败时 `null`）
    result: Value,
    elapsed_ms: i64,
}

impl CallOutcome {
    /// 失败记录（Kotlin `ToolRegistry.failure`）：**正文与摘要的形状都是契约的一部分** ——
    /// 正文是 `[工具失败:CODE] 人话`（模型据此换做法），摘要是 `[CODE] 人话`（界面一行小字）。
    fn failure(code: &str, message: &str, approval: &str, elapsed_ms: i64) -> Self {
        Self {
            content: format!("[工具失败:{code}] {message}"),
            preview: crate::tools::preview_of(&format!("[{code}] {message}")),
            ok: false,
            code: Some(code.to_string()),
            approval: approval.to_string(),
            result: Value::Null,
            elapsed_ms,
        }
    }
}

/// 处理一次工具调用：**闸门 → 执行 → 落库 → 发事件**，返回喂回模型的那份结果。
///
/// # 顺序不能倒（R8）
///
/// ① 需要审批时先 `insert_pending_tool_call`（**落库**）→ ② 再发 `tool.requested` →
/// ③ 等用户在**任何一台设备**上点批准（读库）→ ④ `tool.started` → ⑤ 执行 →
/// ⑥ `record_tool_call` + `tool.completed` / `tool.failed`。
///
/// ② 排在 ① 后面是因为：用户手快先点了按钮时，闸门必须已经有一行可改
/// （否则那一次点击落空，只能等超时）。而这里读库而不是读内存，顺带买到
/// "另一台设备（手机 / 第二个窗口）也能批"。
///
/// ⚠ `tool.requested` **每一个调用都要发**（只读工具带 `approval:"not_required"`）——
/// 与 Kotlin `HarnessRunner:723` 一致。界面靠它给工具卡**排位置**（用户 bug⑥：流式期间
/// "思考 → 工具 → 正文"的真实交错就是这个事件画出来的）；只给需要审批的调用发，
/// 只读工具的卡在流式期间就没有槽位，一直等到收尾才冒出来。
#[allow(clippy::too_many_arguments)]
async fn handle_call(
    state: &AppState,
    run_id: &str,
    assistant_id: &str,
    policy: &crate::policy::ToolPolicy,
    session: &mut crate::tools::ToolSession,
    call_id: &str,
    call: &crate::protocol::ToolCallRef,
    sink: &Arc<Mutex<Sink>>,
    calls_made: i64,
    max_calls: i64,
) -> Result<CallOutcome, ApiError> {
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
    let access = policy.access_for(&call.name, crate::tools::default_access(&call.name));
    let needs_approval = access == crate::policy::ToolAccess::Ask;
    // 工具卡（界面按它画"正在调用 / 已拒绝"）：先落调用块，再落结果块。
    // 参数用**原文**（不是解析后的对象）：Dart 侧 `map['arguments']` 读的就是这个字符串。
    {
        let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        sink.push_tool_call(call_id, &call.name, &call.arguments);
    }

    // ① 落库（R8：闸门开的同一刻就落，用户手快也点得到）
    if needs_approval {
        crate::runs::insert_pending_tool_call(state, run_id, call_id, &call.name, &call.arguments)?;
    }
    // ② 发事件，而且**立刻落库**
    //
    // 为什么要单独 flush：其余事件是攒批写的（P1 的教训：一条 Run 实测 4.7 万个
    // `text.delta`，逐条开事务就是 `ai_run_events` 涨到 62 万行的成因）。而
    // `tool.requested` 是**闸门类事件** —— 另一台设备（手机 / 第二个窗口 / 刚重启的桌面）
    // 靠它才知道"有东西在等人批"，压在批里等一批才落库，那边就是白等。
    // 与 Kotlin `HarnessRunner:316` 同一条：`TOOL_REQUESTED` / `USER_QUESTION` 立即 flush。
    state.bus.emit(
        state,
        run_id,
        event_type::TOOL_REQUESTED,
        &json!({
            "runId": run_id,
            "callId": call_id,
            "name": call.name,
            "arguments": call.arguments,
            "approval": if needs_approval { "pending" } else { "not_required" },
        }),
    )?;
    state.bus.flush(state, run_id)?;
    // ③ 只读工具：马上就是"开始执行"（需要审批的要等用户点头之后才发，见下）
    if !needs_approval {
        emit_started(state, run_id, call_id, &call.name)?;
    }

    // ④ 三道预检，顺序照抄 Kotlin `ToolRegistry.invoke`（找不到 → 额度 → 策略）
    if !crate::tools::is_implemented(&call.name) {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            call,
            sink,
            CallOutcome::failure(
                "TOOL_NOT_FOUND",
                &format!(
                    "没有名为「{}」的工具；可用工具见系统提示里的清单\
                     （P4 后端还没搬的工具一律不在那份清单里）",
                    call.name
                ),
                "denied",
                0,
            ),
        )
        .await;
    }
    if calls_made >= max_calls || calls_made >= crate::tools::ABSOLUTE_CALL_CAP {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            call,
            sink,
            CallOutcome::failure(
                "TOOL_BUDGET_EXCEEDED",
                &crate::tools::budget_exceeded_note(
                    max_calls,
                    calls_made < crate::tools::ABSOLUTE_CALL_CAP,
                ),
                "denied",
                0,
            ),
        )
        .await;
    }
    if access == crate::policy::ToolAccess::Deny {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            call,
            sink,
            CallOutcome::failure(
                "TOOL_DENIED",
                &format!("工具「{}」被用户策略禁止使用（设置 → AI 工具权限）", call.name),
                "denied",
                0,
            ),
        )
        .await;
    }

    // ⑤ 审批：等用户在**任何一台设备**上点（读库：跨端可见）
    let mut approval = if needs_approval { "pending" } else { "not_required" }.to_string();
    if needs_approval {
        if !wait_for_approval(state, run_id, call_id).await? {
            let outcome = CallOutcome::failure(
                "APPROVAL_DENIED",
                "用户没有批准这次工具调用（或等待超时）；不要重试同一件事，如实说明即可",
                "denied",
                APPROVAL_TIMEOUT_SECONDS as i64 * 1000,
            );
            return finish_failure(state, run_id, assistant_id, call_id, call, sink, outcome).await;
        }
        approval = "approved".to_string();
        // 批准的那一刻就告诉界面"开始跑了"，而不是等它跑完再补发
        emit_started(state, run_id, call_id, &call.name)?;
    }

    // ⑥ 执行
    let started = std::time::Instant::now();
    let result = crate::tools::execute(state, &call.name, &arguments, session);
    let elapsed_ms = started.elapsed().as_millis() as i64;
    let outcome = match result {
        Ok(out) => {
            let (content, truncated) = crate::tools::truncate_result(&out.content);
            // 结构化结果：工具给了就用它，否则"截断过"这件事本身也要让界面知道
            let mut meta = out.payload.clone();
            if truncated {
                if let Some(map) = meta.as_object_mut() {
                    map.insert("truncated".into(), Value::Bool(true));
                } else {
                    meta = json!({ "truncated": true });
                }
            }
            CallOutcome {
                content,
                preview: out.preview,
                ok: true,
                code: None,
                approval,
                result: meta,
                elapsed_ms,
            }
        }
        Err(failure) => CallOutcome::failure(
            &failure.code,
            &failure.message,
            &approval,
            elapsed_ms,
        ),
    };

    // ⑦ 落库 + 发事件 + 落块（失败也走同一条尾巴，**绝不静默**）
    let rec = crate::runs::ToolCallRecord {
        id: crate::conversations::uuid_v4()?,
        run_id: run_id.to_string(),
        call_id: call_id.to_string(),
        name: call.name.clone(),
        arguments_json: call.arguments.clone(),
        approval: outcome.approval.clone(),
        status: if outcome.ok { "ok" } else { "failed" }.to_string(),
        result_json: if outcome.result.is_null() { None } else { Some(outcome.result.clone()) },
        content: Some(outcome.content.clone()),
        error: outcome.code.as_ref().map(|_| outcome.error_text()),
        elapsed_ms: outcome.elapsed_ms,
    };
    crate::runs::record_tool_call(state, &rec)?;
    if outcome.ok {
        state.bus.emit(
            state,
            run_id,
            event_type::TOOL_COMPLETED,
            &json!({
                "callId": call_id,
                "name": call.name,
                "elapsedMs": outcome.elapsed_ms,
                "preview": outcome.preview,
                "result": outcome.result,
            }),
        )?;
    } else {
        state.bus.emit(
            state,
            run_id,
            event_type::TOOL_FAILED,
            &json!({
                "callId": call_id,
                "name": call.name,
                "code": outcome.code,
                "message": outcome.error_text(),
                "approval": outcome.approval,
                "elapsedMs": outcome.elapsed_ms,
            }),
        )?;
    }
    sink.lock().unwrap_or_else(|e| e.into_inner()).push_tool_result(call_id, &call.name, &outcome);
    Ok(outcome)
}

impl CallOutcome {
    /// 失败时给用户/模型看的人话（去掉 `[工具失败:CODE] ` 那个前缀）。
    fn error_text(&self) -> String {
        match &self.code {
            Some(code) => self
                .content
                .strip_prefix(&format!("[工具失败:{code}] "))
                .unwrap_or(&self.content)
                .to_string(),
            None => self.content.clone(),
        }
    }
}

/// 只读工具 / 审批通过之后：`tool.started`（形状只有两个键，契约 §1）。
fn emit_started(state: &AppState, run_id: &str, call_id: &str, name: &str) -> Result<(), ApiError> {
    state
        .bus
        .emit(state, run_id, event_type::TOOL_STARTED, &json!({ "callId": call_id, "name": name }))?;
    Ok(())
}

/// 预检失败（没轮到执行）也要**落库 + 发事件 + 落块**：界面上一张"被拒绝 / 不存在"的卡
/// 比"什么都没有"有用得多，模型也要拿到一句能改变下一步做法的正文。
#[allow(clippy::too_many_arguments)]
async fn finish_failure(
    state: &AppState,
    run_id: &str,
    _assistant_id: &str,
    call_id: &str,
    call: &crate::protocol::ToolCallRef,
    sink: &Arc<Mutex<Sink>>,
    outcome: CallOutcome,
) -> Result<CallOutcome, ApiError> {
    let rec = crate::runs::ToolCallRecord {
        id: crate::conversations::uuid_v4()?,
        run_id: run_id.to_string(),
        call_id: call_id.to_string(),
        name: call.name.clone(),
        arguments_json: call.arguments.clone(),
        approval: outcome.approval.clone(),
        status: "failed".into(),
        result_json: None,
        content: Some(outcome.content.clone()),
        error: Some(outcome.error_text()),
        elapsed_ms: outcome.elapsed_ms,
    };
    crate::runs::record_tool_call(state, &rec)?;
    state.bus.emit(
        state,
        run_id,
        event_type::TOOL_FAILED,
        &json!({
            "callId": call_id,
            "name": call.name,
            "code": outcome.code,
            "message": outcome.error_text(),
            "approval": outcome.approval,
            "elapsedMs": outcome.elapsed_ms,
        }),
    )?;
    sink.lock().unwrap_or_else(|e| e.into_inner()).push_tool_result(call_id, &call.name, &outcome);
    Ok(outcome)
}

/// 把一句"如实说明"补进正文（轮数 / 额度用尽时）：**发 `text.delta` + 落块**。
///
/// 为什么不能只写库：用户必须当场看见"为什么没继续"（否则界面停在半截、像是卡住了），
/// 而块是收尾时按 ordinal 一次写的，这里补的是**当前这一段的结尾**。
fn push_note(
    state: &AppState,
    run_id: &str,
    assistant_id: &str,
    sink: &Arc<Mutex<Sink>>,
    note: &str,
) -> Result<(), ApiError> {
    sink.lock().unwrap_or_else(|e| e.into_inner()).push_text(note);
    state.bus.emit(
        state,
        run_id,
        event_type::TEXT_DELTA,
        &json!({ "messageId": assistant_id, "text": note }),
    )?;
    Ok(())
}

/// 等一次审批：读**库**（跨端可见），超时 → `false`（按拒绝）。
///
/// 为什么轮询库而不是等在内存的通道上：这个仓库的既有纪律是
/// **"内存里活着但库里没有"这种状态不许存在**（M1）。读库顺带买到"另一台设备
/// （手机 / 第二个窗口）点的批准也算数"，以及**不需要在进程里维护第二个真源**。
///
/// 超时**不在这里落终态** —— 返回 `false` 之后由调用方写 `denied`：终态的写法只有
/// 一处（`record_tool_call`），两条路各写一遍迟早会漂移。
async fn wait_for_approval(state: &AppState, run_id: &str, call_id: &str) -> Result<bool, ApiError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECONDS);
    loop {
        if state.executor.is_cancelled(run_id) {
            return Ok(false);
        }
        match crate::runs::approval_state(state, call_id)? {
            Some(crate::runs::ApprovalState::Approved) => return Ok(true),
            Some(crate::runs::ApprovalState::Denied) => return Ok(false),
            // `None`：那一行还没落（同一条连接上的可见性问题，几乎不可能）—— 继续等
            Some(crate::runs::ApprovalState::Pending) | None => {}
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("审批等待超时（{APPROVAL_TIMEOUT_SECONDS}s），按拒绝处理：{call_id}");
            return Ok(false);
        }
        tokio::time::sleep(std::time::Duration::from_millis(APPROVAL_POLL_MS)).await;
    }
}

/// 流式期间的攒批（**按发生顺序的有序块**）。
///
/// 为什么攒着而不是逐字写库：一次 Run 实测 4.7 万个 `text.delta`，
/// 逐个开事务就是 `ai_run_events` 涨到 62 万行的那个成因（总纲 P1 的同一条教训）。
/// 收尾时按 ordinal 一次写完 —— 块的顺序就是**事情发生的顺序**
/// （思考 → 正文 → 工具卡 → 工具结果 → 正文…），前端就是按这个顺序渲染的。
#[derive(Default)]
struct Sink {
    /// 当前这一段正文（工具调用会把它"封段"，之后的正文算新的一段）
    text: String,
    /// 思考段（只有第一轮有，收尾时落在最前面）
    reasoning: String,
    /// 已经封好的块（`(type, text, tool_call_id, json_payload)`）
    parts: Vec<(String, String, Option<String>, Option<Value>)>,
}

impl Sink {
    fn push_text(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    fn push_reasoning(&mut self, chunk: &str) {
        self.reasoning.push_str(chunk);
    }

    /// 当前这段正文封成块（工具卡之前必须调 —— 否则工具卡会插到正文前面去）。
    fn seal_text(&mut self) {
        if !self.text.is_empty() {
            let sealed = std::mem::take(&mut self.text);
            self.parts.push(("text".into(), sealed, None, None));
        }
    }

    /// 工具卡块。`arguments` 是**模型给的原文**（Kotlin 也把它当字符串放进 payload）——
    /// 存成对象的话，Dart 侧 `map['arguments'].toString()` 会变成 `{query: x}` 这种
    /// 既不是 JSON、也没法回填进输入框的东西。
    fn push_tool_call(&mut self, call_id: &str, name: &str, arguments: &str) {
        self.seal_text();
        self.parts.push((
            "tool_call".into(),
            String::new(),
            Some(call_id.to_string()),
            Some(json!({ "name": name, "arguments": arguments })),
        ));
    }

    /// 工具结果块。payload 的键集合**就是契约**（Kotlin `HarnessRunner:762`）：
    /// `name` / `ok` / `code` / `elapsedMs` / `approval`（+ 有产物时 `mediaIds`、
    /// 有提问时 `question`）。历史回放（重开 App）时界面读的就是这几个键 ——
    /// 少了 `ok`，一条成功的调用会被画成失败。
    fn push_tool_result(&mut self, call_id: &str, name: &str, outcome: &CallOutcome) {
        self.seal_text();
        let mut payload = json!({
            "name": name,
            "ok": outcome.ok,
            "code": outcome.code,
            "elapsedMs": outcome.elapsed_ms,
            "approval": outcome.approval,
        });
        if let Some(map) = payload.as_object_mut() {
            // 产物 id 一起落进块：重开 App 之后回复末尾的「画廊入口卡」才能照样贴出来
            if let Some(media_ids) = outcome.result.get("mediaIds") {
                map.insert("mediaIds".into(), media_ids.clone());
            }
            // 提问卡（用户建议 ⑥）：问了什么、用户答了什么，一起落库
            if let Some(asked) = outcome.result.get("asked") {
                map.insert("question".into(), asked.clone());
            }
        }
        self.parts.push((
            "tool_result".into(),
            outcome.content.clone(),
            Some(call_id.to_string()),
            Some(payload),
        ));
    }

    /// 取走**当前这一段**正文（工具往返时它要作为助手轮的内容回传给上游）。
    ///
    /// 只取"还没封段的那一段"：已经封好的那些块代表更早的轮次，它们**已经**在
    /// `trailing` 里回传过了（重复回传会让上游看到两遍同样的话）。
    fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// 落库用的完整正文（所有正文段拼起来）—— `ai_messages.text` 那一列。
    fn text_body(&self) -> String {
        let mut out = String::new();
        for (kind, text, _, _) in &self.parts {
            if kind == "text" {
                out.push_str(text);
            }
        }
        out.push_str(&self.text);
        out
    }

    fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.text.is_empty() && self.reasoning.is_empty()
    }
}

/// 把攒下来的内容落成有序块（**收尾的每一条路都要调**）。
fn sink_now(state: &AppState, assistant_id: &str, sink: &Sink) -> Result<(), ApiError> {
    if assistant_id.is_empty() || sink.is_empty() {
        return Ok(());
    }
    let reasoning = sink.reasoning.clone();
    let mut parts: Vec<(String, String, Option<String>, Option<String>)> = Vec::new();
    if !reasoning.is_empty() {
        parts.push(("reasoning".into(), reasoning, None, None));
    }
    for (kind, text, tool_call_id, payload) in &sink.parts {
        parts.push((kind.clone(), text.clone(), tool_call_id.clone(), payload.as_ref().map(|v| v.to_string())));
    }
    if !sink.text.is_empty() {
        parts.push(("text".into(), sink.text.clone(), None, None));
    }
    state.write(move |tx| {
        // 重试复用同一条消息时可能已经有块了：先删再按当前内容重建（幂等）
        tx.execute("DELETE FROM ai_message_parts WHERE message_id = ?1", rusqlite::params![assistant_id])?;
        for (ordinal, (kind, text, tool_call_id, payload)) in parts.iter().enumerate() {
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text, tool_call_id, json_payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![assistant_id, ordinal as i64, kind, text, tool_call_id, payload],
            )?;
        }
        Ok(())
    })
}

/// 一条历史消息 → 上游的轮次（`HarnessRunner.turnsFromHistory` 的精简版）。
///
/// 规则：
/// * `user` / `assistant` 的**正文**进上下文；正文空但有工具调用的助手轮也要进
///   （否则上游看到的工具结果**没有对应的调用**，OpenAI 会直接 400）；
/// * `tool_result` 块变成 `role = "tool"` 的一轮（带 `tool_call_id`）；
/// * 思考段**不进**上下文（它是本地展示用的；上游各家都不接受回传思考）。
fn turns_of(message: &AiMessageDto) -> Vec<ChatTurn> {
    let mut out = Vec::new();
    if message.role == "assistant" {
        let body: String = message
            .parts
            .iter()
            .filter(|p| p.r#type == "text")
            .filter_map(|p| p.text.clone())
            .collect();
        // 助手轮的工具调用：只有真的发生过工具的会话才有
        let calls = tool_calls_of(&message.parts);
        if !body.is_empty() || !calls.is_empty() {
            out.push(ChatTurn {
                role: "assistant".into(),
                content: body,
                tool_calls: calls,
                tool_call_id: None,
            });
        }
        return out;
    }
    // ⚠ 顺序要紧：**正文先成一轮、工具结果各成一轮**，而工具结果必须紧跟在
    // 发起它的那个助手工具轮之后 —— OpenAI 会校验"每个 tool_call_id 都要有对应的结果"，
    // 顺序错了直接 400（而错误信息完全指不到这里）。
    let body = if message.parts.is_empty() {
        message.text.clone()
    } else {
        message
            .parts
            .iter()
            .filter(|p| p.r#type == "text")
            .filter_map(|p| p.text.clone())
            .collect()
    };
    if !body.trim().is_empty() {
        out.push(ChatTurn {
            role: message.role.clone(),
            content: body,
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
    }
    // 工具结果：作为**独立的一轮**回填（`role = "tool"` + `tool_call_id`）
    for part in message.parts.iter().filter(|p| p.r#type == "tool_result") {
        out.push(ChatTurn {
            role: "tool".into(),
            content: part.text.clone().unwrap_or_default(),
            tool_calls: Vec::new(),
            tool_call_id: part.tool_call_id.clone(),
        });
    }
    out
}

/// 助手轮里的工具调用（`json_payload` 里是 `{name, arguments}`）。
fn tool_calls_of(parts: &[AiMessagePartDto]) -> Vec<crate::protocol::ToolCallRef> {
    parts
        .iter()
        .filter(|p| p.r#type == "tool_call")
        .filter_map(|p| {
            let payload = p.json_payload.as_ref()?;
            let name = payload.get("name")?.as_str()?.to_string();
            let arguments = payload
                .get("arguments")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "{}".into());
            Some(crate::protocol::ToolCallRef {
                id: p.tool_call_id.clone().unwrap_or_default(),
                name,
                arguments,
            })
        })
        .collect()
}

/// `POST /api/ai/runs/{id}/cancel` 的执行器那一半。
pub fn cancel(state: &AppState, run_id: &str) -> bool {
    state.executor.request_cancel(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, parts: Vec<AiMessagePartDto>) -> AiMessageDto {
        AiMessageDto {
            id: "m".into(),
            conversation_id: "c".into(),
            seq: 1,
            role: role.into(),
            status: "complete".into(),
            text: String::new(),
            provider_id: None,
            model_id: None,
            provider_response_id: None,
            usage: None,
            parts,
            created_at: None,
            updated_at: None,
        }
    }

    fn text_part(text: &str) -> AiMessagePartDto {
        AiMessagePartDto {
            r#type: "text".into(),
            text: Some(text.into()),
            attachment_id: None,
            tool_call_id: None,
            json_payload: None,
        }
    }

    /// 历史 → 上游轮次：思考段**不进**上下文（各家都不接受回传思考）。
    #[test]
    fn history_drops_reasoning_and_keeps_text() {
        let mut m = message(
            "assistant",
            vec![
                AiMessagePartDto {
                    r#type: "reasoning".into(),
                    text: Some("我一直在想".into()),
                    attachment_id: None,
                    tool_call_id: None,
                    json_payload: None,
                },
                text_part("答案"),
            ],
        );
        m.text = "答案".into();
        let turns = turns_of(&m);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].content, "答案");
        assert!(!turns[0].content.contains("我一直在想"));
    }

    /// 工具结果必须**带着 `tool_call_id`** 单独成轮：否则上游看到的工具结果没有对应的调用
    /// （OpenAI 会直接 400，而错误信息完全指不到这里）。
    #[test]
    fn tool_results_become_their_own_turn_with_the_call_id() {
        let m = message(
            "tool",
            vec![AiMessagePartDto {
                r#type: "tool_result".into(),
                text: Some("结果正文".into()),
                attachment_id: None,
                tool_call_id: Some("call_1".into()),
                json_payload: None,
            }],
        );
        let turns = turns_of(&m);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].role, "tool");
        assert_eq!(turns[0].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(turns[0].content, "结果正文");
    }

    /// 助手的工具调用要从 `json_payload` 里还原（名字 + 参数 JSON 文本）。
    #[test]
    fn assistant_tool_calls_round_trip_through_parts() {
        let m = message(
            "assistant",
            vec![AiMessagePartDto {
                r#type: "tool_call".into(),
                text: None,
                attachment_id: None,
                tool_call_id: Some("call_9".into()),
                json_payload: Some(json!({ "name": "web_search", "arguments": { "q": "x" } })),
            }],
        );
        let turns = turns_of(&m);
        assert_eq!(turns.len(), 1, "正文空但有工具调用 → 助手轮仍要进上下文");
        assert_eq!(turns[0].tool_calls.len(), 1);
        assert_eq!(turns[0].tool_calls[0].id, "call_9");
        assert_eq!(turns[0].tool_calls[0].name, "web_search");
        assert_eq!(turns[0].tool_calls[0].arguments, "{\"q\":\"x\"}");
    }

    /// 没有块的旧消息（或纯文本消息）**按正文兜底** —— 历史里少一轮会让模型答非所问。
    #[test]
    fn messages_without_parts_fall_back_to_text() {
        let mut m = message("user", Vec::new());
        m.text = "你好".into();
        let turns = turns_of(&m);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].content, "你好");
        // 完全空的（既没有块也没有正文）→ 不产生空轮
        let mut empty = message("user", Vec::new());
        empty.text = "   ".into();
        assert!(turns_of(&empty).is_empty());
    }

    /// 取消旗标：**立起来 ≠ 立刻停**（主循环在安全点自己收尾，R6 要求它先把话说完）。
    #[test]
    fn cancel_flag_lifecycle() {
        let executor = Executor::new();
        assert!(!executor.request_cancel("r1"), "不在跑的 Run 没什么可取消的");
        executor.mark_running("r1");
        assert!(executor.request_cancel("r1"));
        assert!(executor.is_cancelled("r1"));
        executor.mark_done("r1");
        assert!(!executor.is_cancelled("r1"), "收尾之后旗标必须清掉（否则下一次 Run 一开就取消）");
        assert!(!executor.request_cancel("r1"));
    }
}
