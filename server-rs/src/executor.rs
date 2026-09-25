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
use crate::protocol::{Api, ChatTurn, ReasoningRequest, StreamEvent, ThinkingFormat, ToolSpec};
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

    // ⚠ 这一版**不下发工具**（第 31 步才搬工具层）：提示词里就不提工具，
    // 免得模型去调一个必然失败的调用（见文件头）。
    let tools: Vec<ToolSpec> = Vec::new();
    let system_prompt = crate::system_prompt::render(
        &provider.display_name,
        &provider.api,
        &model.id,
        &[],
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

    // 系统提示里那段工具说明要跟着实际下发的工具走（`render` 已经按 `&[]` 说了"没有工具"）
    let api = Api::parse(&provider.api).ok_or_else(|| {
        ApiError::ai("CONFIG_ERROR", format!("Provider 协议非法: {}", provider.api))
    })?;

    // ---- 流式 ---------------------------------------------------------------
    //
    // `std::sync::Mutex`（不是 tokio 的那个）是**刻意的**：回调是同步的，
    // 而锁只在 `lock()` 与 `drop()` 之间持有、**从不跨 await** —— 那正是 std 锁的用法。
    let sink = Arc::new(Mutex::new(Sink::default()));

    // 上游要的思考强度：`off` → 一个参数都不发
    let reasoning_request = ReasoningRequest {
        effort: effort_wire.clone(),
        wire_value: effort_wire.clone(),
        format: model.thinking_format.as_deref().and_then(ThinkingFormat::parse),
    };

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
                turns: &turns,
                tools: &tools,
                reasoning: reasoning_request,
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
                        sink.text.push_str(&chunk);
                        emit(json!({ "messageId": assistant, "text": chunk }), event_type::TEXT_DELTA)?;
                    }
                    StreamEvent::ReasoningDelta(chunk) => {
                        sink.reasoning.push_str(&chunk);
                        emit(
                            json!({ "messageId": assistant, "text": chunk }),
                            event_type::REASONING_DELTA,
                        )?;
                    }
                    StreamEvent::ToolCallDelta { .. } => {
                        // 这一版不下发工具，所以理论上到不了这里。真到了就**如实**记一笔：
                        // 不执行、也不假装执行过（第 31 步会把这里接上工具层）。
                        sink.unhandled_tool_call = true;
                    }
                    StreamEvent::Finished(_) | StreamEvent::ProviderId(_) | StreamEvent::Usage(_) => {}
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
            {
                let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
                sink_now(state, &assistant_id, &sink)?;
                crate::runs::update_message(state, &assistant_id, &sink.text, "failed", None, None)?;
            }
            return Err(e);
        }
    };
    let usage = idle.usage.clone();
    // 上游给的响应 id（没有就用 provider id 拼一个稳定串：事后总能追溯是哪一次）
    let provider_response_id = idle
        .provider_response_id
        .clone()
        .unwrap_or_else(|| format!("{}-{}", provider.id, run_id));

    // 取消：**在收尾之前**问一次（在安全点上）
    if state.executor.is_cancelled(run_id) {
        let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        sink_now(state, &assistant_id, &sink)?;
        crate::runs::update_message(state, &assistant_id, &sink.text, "cancelled", None, None)?;
        drop(sink);
        finish_run(&state.bus, state, run_id, "cancelled", None, None, None)?;
        return Ok(());
    }

    // ---- 落库 --------------------------------------------------------------
    let body = {
        let sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        sink_now(state, &assistant_id, &sink)?;
        sink.text.clone()
    };

    // usage 事件的形状：归一到 OpenAI 形状（界面读的是它）
    if let Some(usage) = usage.as_ref() {
        state
            .bus
            .emit(state, run_id, event_type::USAGE_UPDATED, &json!({ "usage": usage }))?;
    }

    crate::runs::update_message(
        state,
        &assistant_id,
        &body,
        "complete",
        Some(&provider_response_id),
        usage.as_ref(),
    )?;
    state.bus.emit(
        state,
        run_id,
        event_type::MESSAGE_COMPLETED,
        &json!({
            "messageId": assistant_id,
            "status": "complete",
            "text": body,
            "usage": usage,
        }),
    )?;
    // ⚠ "上游给了工具调用但这一版执行不了"**不进事件 payload** ——
    // 事件形状是契约的一部分（`docs/run-event-contract.md`），不许私自加键；
    // 它只落进日志，供排查用。
    if sink.lock().unwrap_or_else(|e| e.into_inner()).unhandled_tool_call {
        eprintln!("Run {run_id}：上游返回了工具调用，而这一版桌面执行器还没有工具层（P4 第 31 步）");
    }

    finish_run(&state.bus, state, run_id, "completed", None, None, usage.as_ref())?;
    Ok(())
}

/// 流式期间的攒批（**正文 / 思考 / 工具卡**）。
///
/// 为什么攒着而不是逐字写库：一次 Run 实测 4.7 万个 `text.delta`，
/// 逐个开事务就是 `ai_run_events` 涨到 62 万行的那个成因（总纲 P1 的同一条教训）。
/// 收尾时按 ordinal 一次写完 —— 块顺序 = 思考 → 正文。
#[derive(Default)]
struct Sink {
    text: String,
    reasoning: String,
    /// 上游给了工具调用但这一版执行不了：如实记下来（不许假装没发生）
    unhandled_tool_call: bool,
}

/// 把攒下来的内容落成有序块（**收尾的每一条路都要调**）。
fn sink_now(state: &AppState, assistant_id: &str, sink: &Sink) -> Result<(), ApiError> {
    if assistant_id.is_empty() {
        return Ok(());
    }
    if sink.text.is_empty() && sink.reasoning.is_empty() {
        return Ok(());
    }
    state.write(|tx| {
        // 重试复用同一条消息时可能已经有块了：先删再按当前内容重建（幂等）
        tx.execute("DELETE FROM ai_message_parts WHERE message_id = ?1", rusqlite::params![assistant_id])?;
        let mut ordinal = 0i64;
        if !sink.reasoning.is_empty() {
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text) VALUES (?1, ?2, 'reasoning', ?3)",
                rusqlite::params![assistant_id, ordinal, sink.reasoning],
            )?;
            ordinal += 1;
        }
        if !sink.text.is_empty() {
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text) VALUES (?1, ?2, 'text', ?3)",
                rusqlite::params![assistant_id, ordinal, sink.text],
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
        // 助手轮的工具调用：只有真的发生过工具的会话才有（这一版不下发工具）
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
    // 工具结果：作为独立的一轮回填
    for part in message.parts.iter().filter(|p| p.r#type == "tool_result") {
        out.push(ChatTurn {
            role: "tool".into(),
            content: part.text.clone().unwrap_or_default(),
            tool_calls: Vec::new(),
            tool_call_id: part.tool_call_id.clone(),
        });
    }
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
