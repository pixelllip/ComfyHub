//! 桌面执行器（P4 第 29~30 步）—— 一次 Run 的**主循环**。
//!
//! # 现在的状态（诚实清单）
//!
//! 端点、持久化、事件总线都好了，**上游 LLM 客户端还没搬**（第 29 步：三家协议适配器）。
//! 所以本模块此刻只做一件事：把 Run 开起来、落 `run.started` / `message.started`，
//! 然后**如实失败**并给出可执行的指引 —— 而不是留一个永远转圈的 Run。
//!
//! 为什么"如实失败"比"假装在跑"重要：界面上一个转圈的 Run 会让用户一直等，
//! 而它永远不会有人来推进（Rust 侧没有执行器）。错误码 `EXECUTOR_UNAVAILABLE`
//! 是**新增**的（Kotlin 侧没有这一档）—— 它只是如实描述"这一端还做不了这件事"，
//! 不是把某个既有错误改个名字。
//!
//! # 主循环要守的三条纪律（第 30 步落地时照这个写）
//!
//! 1. **正文必须落块**（R6）：`message.text` 里有的内容，`ai_message_parts` 里也必须有
//!    —— 前端在 parts 非空时**只渲染 parts**，只写正文不落块 = 用户看不见。
//! 2. **失败 / 取消时 flush 已经说出去的话**：标题摘取缓冲里压住的开头、正在流式的那一段，
//!    都要在这一刻补落（`TitleStripper.flush()` + `PartSink.flushInflight()` 的对应物）。
//! 3. **收尾三条路都要走 [`crate::run_lifecycle::finish_run`]**：落终态 + flush + 发终态事件。
//!    少了任何一步，表现都是"界面永远转圈"或"最后那句话补不回来"。

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::json;

use crate::events::event_type;
use crate::routes::AppState;
use crate::run_lifecycle::{finish_run, ModelRow};

/// 还没搬完的那一档（**新增**错误码，只用于"这一端没有执行器"）。
pub const EXECUTOR_UNAVAILABLE: &str = "EXECUTOR_UNAVAILABLE";

/// 正在跑的 Run → 取消旗标。
///
/// 与 Kotlin `HarnessRunner` 的 `cancelled` 集合同一件事：`POST /runs/{id}/cancel`
/// 立旗标，主循环在**安全点**（每收到一个分片 / 每轮工具往返之间）自己收尾。
/// 直接强杀协程是错的：那会跳过"落终态 + flush 已经说出去的话"（R6）。
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

/// 起一次 Run（`POST /runs` 的最后一步）。
///
/// 现在只发 `run.started` / `message.started` 然后**如实失败** —— 见文件头。
///
/// `effort_wire` 是"要发给上游的思考强度"（`None` = 一个思考参数都不发）。
/// 现在用不到，但**签名先留着** —— 第 30 步的主循环要用它，
/// 而"传给上游的值"与"落库的值"是两回事（落库的 `off` 在上游是"不指定"）。
pub fn spawn(
    state: &AppState,
    run_id: &str,
    provider: &ModelRow,
    model: &ModelRow,
    effort_wire: Option<String>,
    _text: String,
) {
    run_sync(state, run_id, provider, model, effort_wire);
}

/// `spawn` 的同步本体（**测试直接调它**：不引 `tokio::test`，也不用等一个后台任务）。
fn run_sync(
    state: &AppState,
    run_id: &str,
    provider: &ModelRow,
    model: &ModelRow,
    _effort_wire: Option<String>,
) {
    let run = match crate::runs::get(state, run_id) {
        Ok(run) => run,
        Err(e) => {
            // 建完 Run 之后又读不到它 —— 这是真异常（外键/事务出问题了），如实说
            eprintln!("执行器：读不到刚建好的 Run {run_id}：{}", e.human());
            return;
        }
    };
    let assistant_id = run.assistant_message_id.clone().unwrap_or_default();

    // ① `run.started`：字段与 Kotlin `HarnessRunner` 逐字一致（界面靠它显示"这一轮带没带 NSFW、
    //    用了哪些工具、思考强度是多少"）。**一个键都不多、一个都不少**：
    //    事件 payload 也是契约的一部分（`docs/run-event-contract.md` §3）。
    let nsfw = crate::run_kit::nsfw_enabled(state).unwrap_or(false);
    let started = json!({
        "runId": run_id,
        "conversationId": run.conversation_id,
        "providerId": provider.id,
        "modelId": model.id,
        "promptVersion": crate::nsfw_prompt::PROMPT_VERSION,
        "nsfw": nsfw,
        "nsfwPromptVersion": crate::nsfw_prompt::VERSION,
        "reasoningEffort": run.reasoning_effort,
        "tools": {},
    });
    if let Err(e) = state.bus.emit(state, run_id, event_type::RUN_STARTED, &started) {
        eprintln!("执行器：发 run.started 失败：{}", e.human());
    }
    if !assistant_id.is_empty() {
        let _ = state.bus.emit(
            state,
            run_id,
            event_type::MESSAGE_STARTED,
            &json!({ "messageId": assistant_id, "role": "assistant" }),
        );
    }

    // ② 上游客户端还没搬：如实失败，并给一条能照做的指引
    let message = format!(
        "Rust 后端的上游 LLM 客户端还没搬过来（P4 第 29~30 步），所以这次 Run 没能发给 {}；\
         请先在设置里把后端切回 Kotlin 版，或改用手机端 harness（它自带执行器）",
        provider.display_name
    );
    if let Err(e) = finish_run(
        &state.bus,
        state,
        run_id,
        "failed",
        Some(EXECUTOR_UNAVAILABLE),
        Some(&message),
        None,
    ) {
        eprintln!("执行器：收尾失败（Run 可能停在 running）：{}", e.human());
    }
    // 助手消息也要落一个如实的状态：留在 `streaming` 的占位气泡会让界面一直转圈
    if !assistant_id.is_empty() {
        let _ = crate::runs::update_message(state, &assistant_id, "", "failed", None, None);
    }
}

/// `POST /api/ai/runs/{id}/cancel` 的执行器那一半。
pub fn cancel(state: &AppState, run_id: &str) -> bool {
    // 现在没有真正的执行器，所以"正在跑"的 Run 一个都没有 —— 如实回 false
    // （`cancel()` 的调用方按 false 回 `{cancelled:false, status}`，与 Kotlin 同语义：
    //  Run 已经不在跑的时候点取消不是错误）
    let _ = state;
    let _ = run_id;
    false
}

#[cfg(test)]
mod tests {
    use super::*;

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
