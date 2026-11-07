//! 桌面执行器（P4 第 30 步）—— 一次 Run 的**主循环**。
//!
//! 端点（`POST /runs` / SSE / cancel）在第 28 步就位，上游客户端在 [`crate::provider::upstream`]。
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

use crate::ai::conversations::{AiMessageDto, AiMessagePartDto};
use crate::ai::events::event_type;
use crate::ai::run_lifecycle::{finish_run, ModelRow};
use crate::provider::protocol::{Api, ChatTurn, ReasoningRequest, StreamEvent, ThinkingFormat};
use crate::provider::upstream::{stream_chat, ChatRequest, TokenUsage};
use crate::web::routes::{ApiError, AppState};

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
        Self {
            running: Mutex::new(HashSet::new()),
            cancelled: Mutex::new(HashSet::new()),
        }
    }
}

impl Executor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_running(&self, run_id: &str) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_string());
    }

    pub fn mark_done(&self, run_id: &str) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id);
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(run_id);
    }

    /// 立取消旗标；返回 false 表示这次 Run 不在这台进程里跑（没什么可取消的）。
    pub fn request_cancel(&self, run_id: &str) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if !running.contains(run_id) {
            return false;
        }
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(run_id.to_string());
        true
    }

    pub fn is_cancelled(&self, run_id: &str) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(run_id)
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
        let run = match crate::ai::runs::get(&state, &run_id) {
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
            crate::ai::run_kit::nsfw_enabled(&state).unwrap_or(false),
        );
        if let Err(e) = state
            .bus
            .emit(&state, &run_id, event_type::RUN_STARTED, &started)
        {
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
        "promptVersion": crate::ai::nsfw_prompt::PROMPT_VERSION,
        "nsfw": nsfw,
        "nsfwPromptVersion": crate::ai::nsfw_prompt::VERSION,
        "reasoningEffort": reasoning_effort,
        "tools": {},
    })
}

/// 内存里那份归一化 usage → **落库/发给界面的形状**（契约那四个键）。
///
/// 落库的 `usage_json` 与 `message.completed.usage` 读的是同一个形状，
/// 所以这个转换只有这一处 —— 别在调用点各写一份 `json!`（写歪了界面就少一行）。
fn usage_json(usage: Option<TokenUsage>) -> Option<Value> {
    usage.map(TokenUsage::to_json)
}

/// 收尾包装：任何一步出错都只记日志（**不 panic、不把上一步的成果丢掉**）。
fn finish_quietly(
    state: &AppState,
    run_id: &str,
    status: &str,
    code: Option<&str>,
    message: Option<&str>,
    usage: Option<TokenUsage>,
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
    let run = crate::ai::runs::get(state, run_id)?;
    let assistant_id = run.assistant_message_id.clone().unwrap_or_default();
    let conversation_id = run.conversation_id.clone();
    // `message.completed` 要报"这一轮跑了多久 / 用的是哪档思考"（与 Kotlin 同一个载荷）
    let started = std::time::Instant::now();
    let effort_stored = run
        .reasoning_effort
        .clone()
        .unwrap_or_else(|| "off".to_string());

    // ---- 上下文（在发请求之前一次取齐）--------------------------------------
    let history = crate::ai::conversations::list_messages(state, &conversation_id)?;
    let nsfw = crate::ai::run_kit::nsfw_enabled(state)?;
    let policy = crate::tools::policy::ToolPolicy::load(state)?;
    let storage = state
        .media_dir()
        .parent()
        .unwrap_or(state.media_dir())
        .to_path_buf();
    let skills = crate::ai::skills::catalog_with(
        &state.project_root().join("skills").join("builtin"),
        &storage.join("ai").join("skills"),
        &crate::ai::skills::disabled_of(state),
    );
    let memory_text = crate::ai::memory::prompt_text(&storage.join("ai"));

    // 工具清单：**只下发这一批真的实现了的**（`declaredTo = 清单 ∩ 本地实现`），
    // 并且按权限策略把 `deny` 的剔掉 —— 系统提示里那段"可调用的工具"读的是**同一个**清单。
    //
    // ⚠ 桌面执行器用 [`crate::tools::tools::catalog`]（手机 + **桌面专属**），不是只取手机清单：
    // Kotlin 的桌面提示词列的是**全量**工具（含 `read_file` / `write_file` / `comfy_*`）。
    // 只取手机那 7 个的话，模型在桌面上永远不知道自己能动文件（P4 第 40 步修）。
    let mcp_views = state.mcp.tools();
    let tools = crate::tools::tools::specs_with(&policy, &mcp_views);
    let tool_refs: Vec<crate::tools::tools::ToolView<'static>> =
        crate::tools::tools::catalog_with(&policy, &mcp_views);
    debug_assert_eq!(tool_refs.len(), tools.len());

    // 附件计划（R9）：哪些图片真的能内联、哪些只能如实说"没发出去"。
    // ⚠ 必须在系统提示之前算出来：提示里那句"本次请求附带图片附件"只有**确实内联了**才说有
    // （否则模型会以为自己看到了图 —— 那正是 AIH-030 要防的"看起来发出去了、其实没有"）。
    let attachment_plan = attachment_plan_of(state, &history, model, &provider.api);
    let has_inlined_attachments = !attachment_plan.inline.is_empty();

    // 工作流参考图（2026-10-25「载入工作流时自动带图」）：`comfy_load_workflow` 的结果里
    // 带 `referenceImages`（`LoadImage` 的真实路径 + 原始分辨率），模型能收图时由
    // `reference_attach` 缩图后挂到下一轮。能力判据与附件内联同一条（模型声明图片模态 +
    // 适配器实现了 inline_base64）；预算与去重是**整个 Run** 的。
    let can_inline_reference_images = crate::ai::reference_attach::supports_images(
        model.input_modalities.iter().any(|m| m == "image"),
        &provider.api,
    );
    let mut reference_budget = crate::ai::reference_attach::Budget::default();
    let mut attached_reference_paths: HashSet<String> = HashSet::new();

    let mut system_prompt = crate::ai::system_prompt::render(
        &provider.display_name,
        &provider.api,
        &model.id,
        // ⚠ **在这里渲染一次、整个 Run 复用**：进工具循环之前就算好，
        // 于是同一 Run 的每一轮前缀逐字节相同（前缀缓存的命中率就是这么来的）。
        // 日期只到"天"，所以同一天的后续 Run 也还是同一份 —— 见 `system_prompt` 文件头。
        &crate::ai::run_kit::local_date(state)?,
        &tool_refs,
        &skills,
        &policy,
        &memory_text,
        has_inlined_attachments,
        // 与附件计划**分开**：模型能不能收图，与这一轮有没有图是两件事。
        model.input_modalities.iter().any(|m| m == "image"),
        nsfw,
    );

    // 用户在消息里用 `/名字` 显式选中的技能：正文在**这一轮**的系统提示里预加载。
    // 追加在最后而不是插在前面 —— 前面几段是跨 Run 稳定的前缀，动它们会打断前缀缓存。
    let explicit_skills = explicit_skills_of(&history, run.user_message_id.as_deref());
    if !explicit_skills.is_empty() {
        system_prompt.push_str("\n\n## 本次用户显式调用的技能（已加载，直接按它执行）\n");
        for (name, version, body) in &explicit_skills {
            let ver = version
                .as_deref()
                .map(|v| format!(" version=\"{v}\""))
                .unwrap_or_default();
            system_prompt.push_str(&format!(
                "用户这条消息手动选择了技能「{name}」。完整规则如下：\n<skill_content name=\"{name}\"{ver}>\n{body}\n</skill_content>\n"
            ));
        }
    }

    let mut turns = vec![ChatTurn {
        role: "system".into(),
        content: system_prompt,
        ..Default::default()
    }];

    // 起标题的附加指令：**只按轮带、且插在系统提示之后（第 2 条）**。
    // 两条理由都踩过（AGENTS §10.1）：常驻会让后面每一轮都再吐一次标记（漏进正文）；
    // 挂末尾会挡住"最后一条消息是工具结果"这个事实（模型以为还没干活）。
    //
    // ⚠ **"还在起标题"这个条件是逐轮重算的**（不是开头算一次）：第一轮摘到标题之后，
    // 后面的工具轮就不该再提醒模型吐标记 —— 那正是"标记漏进正文"的来源。
    // Kotlin 的 `shouldSummarizeTitle`：会话标题还是空的 / 「新对话」，且用户消息 ≤ 1 条。
    let user_count = history.iter().filter(|m| m.role == "user").count();
    let title_wanted = user_count <= 1 && conversation_needs_title(state, &conversation_id);
    // 摘标记的状态机：只有真要起标题时才开（关掉时原样透传）。
    // ⚠ 放在 `Arc<Mutex<…>>` 里：流式回调（同步闭包）与轮循环都要用它。
    let stripper = Arc::new(Mutex::new(crate::ai::title_strip::TitleStripper::new(
        title_wanted,
    )));
    // 关键步骤（环节）标记：`[步骤]正在查资料[/步骤]` → 落一个 `step` 块 + 一条 `step.started` 事件
    // （用户建议 2026-09-28：「关键步骤之间 AI 的操作再折叠」）。**永远开着** ——
    // 它不吃请求、不占预算，模型不写标记时它一个字都不动（见 `step_mark.rs` 的三条纪律）。
    let steps = Arc::new(Mutex::new(crate::ai::step_mark::StepStripper::new()));
    let mut title_applied = false;
    let mut continuations = 0usize;

    // 历史（**不含**当前那条助手占位）：只带正文与工具轮，备注类不进上下文
    for message in history.iter().filter(|m| m.id != assistant_id) {
        if message.role == "system_note" {
            continue;
        }
        turns.extend(turns_of(message, &attachment_plan));
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
            turns.push(ChatTurn {
                role: "assistant".into(),
                content: already,
                // 已经说出去的那半段如果带着思考，思考也要跟着回传（与历史同一条理由）
                reasoning: reasoning_of(&current.parts),
                ..Default::default()
            });
        }
    }

    // 系统提示里那段工具说明要跟着实际下发的工具走（`render` 收到的是同一个清单）
    let api = Api::parse(&provider.api).ok_or_else(|| {
        ApiError::ai(
            "CONFIG_ERROR",
            format!("Provider 协议非法: {}", provider.api),
        )
    })?;

    // `std::sync::Mutex`（不是 tokio 的那个）是**刻意的**：回调是同步的，
    // 而锁只在 `lock()` 与 `drop()` 之间持有、**从不跨 await** —— 那正是 std 锁的用法。
    // 有序块按"发生的顺序"攒（思考 → 正文 → 工具卡 → 工具结果），收尾一次写完。
    let sink = Arc::new(Mutex::new(Sink::default()));

    // 上游要的思考强度：`off` → 一个参数都不发
    let reasoning_request = ReasoningRequest {
        effort: effort_wire.clone(),
        wire_value: effort_wire.clone(),
        format: model
            .thinking_format
            .as_deref()
            .and_then(ThinkingFormat::parse),
        echo_all: false,
    };
    // 上游因为"思维链没回传"拒过一次之后：**每一轮都补空的 reasoning 项**，
    // 但**思考强度一个字节都不动**（补的是缺的条目，不是关掉思考的能力）。
    let mut echo_reasoning = false;

    // ---- 工具往返循环 -------------------------------------------------------
    //
    // 一轮 = 发一次上游 → 收正文/思考/工具调用 → 若有工具就执行完再发下一轮。
    // 两条预算与 Kotlin 逐条对齐（`ToolPolicyConfig`）：
    //  * **轮数** `maxToolSteps`（出厂 25）—— 到顶那一轮**不执行**工具，
    //    而是把"已达轮数上限、再发一条就能接着做"如实补进正文（**不是**把 Run 判失败：
    //    用户已经看到的那段回答不该挂红叉，那件事本来只是"这轮到此为止"）；
    //  * **次数** `maxCallsPerRun`（出厂 48，硬天花板 [`crate::tools::tools::ABSOLUTE_CALL_CAP`]）——
    //    用尽之后这一轮只收口，再要工具就回 `TOOL_BUDGET_EXCEEDED`。
    let max_steps = policy.config.max_tool_steps.max(1) as usize;
    let max_calls = policy.config.max_calls_per_run.max(1);
    let mut session = crate::tools::tools::ToolSession::default();
    // 用户刚发了图：`comfy_check_inputs` 要能区分"投放一下就能解决"与"必须问用户要一张"
    // （Kotlin `ToolContext.hasImageAttachment`，与上面喂给系统提示的是**同一个值**）
    session.has_image_attachment = has_inlined_attachments;
    // 显式调用的技能已经在系统提示里预加载：标记成"已加载"，模型再调 load_skill
    // 只会得到"已经加载过了"，不会把同一份正文灌第二遍。
    for (name, _, _) in &explicit_skills {
        session.mark_explicit_skill(name);
    }
    let mut trailing: Vec<ChatTurn> = Vec::new();
    let mut total_usage: Option<TokenUsage> = None;
    let mut last_provider_response_id: Option<String> = None;
    let mut step = 0usize;
    let mut budget_exhausted = false;
    // 思考被上游逼停（`effort=none` 重发过一次）之后，本次 Run 余下的每一轮都要沿用 ——
    // 那些轮次同样没有思维链可回传，换回思考模式就是一轮一次 400（见 `stream_chat`）。
    let mut thinking_forced_off = false;
    // 上游这一轮是怎么结束的（`length` = 被输出长度上限截断 → 要接着写）
    // 初值只为让编译器看清"每轮都会赋值"；`unused_assignments` 在这里是误报。
    #[allow(unused_assignments)]
    let mut finish_reason: Option<String> = None;

    loop {
        if state.executor.is_cancelled(run_id) {
            let (pending, phases) = drain_strippers(&stripper, &steps);
            let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
            for phase in phases {
                sink.push_step(&phase);
            }
            if !pending.is_empty() {
                sink.push_text(&pending);
            }
            sink_now(state, &assistant_id, &sink)?;
            // 取消也要把**已经烧掉的 token** 记上（用户 bug「未完成的对话不统计 token 用量」）：
            // 前几轮的 usage 是真金白银，取消只代表"不再继续"，不代表"没花过"。
            crate::ai::runs::update_message(
                state,
                &assistant_id,
                &sink.text_body(),
                "cancelled",
                last_provider_response_id.as_deref(),
                usage_json(total_usage).as_ref(),
            )?;
            drop(sink);
            finish_run(
                &state.bus,
                state,
                run_id,
                "cancelled",
                None,
                None,
                total_usage,
            )?;
            return Ok(());
        }

        let mut turns_now = turns.clone();
        // 起标题的指令**按轮决定**：还没摘到标题的那些轮次才带（摘到了就不再提，
        // 否则模型会在后面的轮次里把标记再吐一遍 —— 那行标记就漏进正文了）。
        // 位置固定在系统提示之后（第 2 条）：挂末尾会挡住"最后一条消息是工具结果"这个事实。
        let still_want_title = title_wanted
            && !title_applied
            && stripper
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .title()
                .is_none();
        if still_want_title {
            turns_now.insert(
                1,
                ChatTurn {
                    role: "system".into(),
                    content: crate::ai::nsfw_prompt::TITLE_INSTRUCTION.to_string(),
                    ..Default::default()
                },
            );
        }
        turns_now.extend(trailing.iter().cloned());

        let outcome = {
            let sink = sink.clone();
            let stripper = stripper.clone();
            let steps = steps.clone();
            let state_for_cb = state.clone();
            let run = run_id.to_string();
            let assistant = assistant_id.clone();
            // 思考已被逼停：这一轮（以及后面每一轮）都不发思考参数，
            // 也不带 `format`（那是"开着思考时用哪种方言表达档位"的事）。
            let reasoning_for_round = if thinking_forced_off {
                crate::provider::protocol::ReasoningRequest {
                    effort: Some("none".into()),
                    wire_value: Some("none".into()),
                    format: None,
                    echo_all: true,
                }
            } else {
                let mut r = reasoning_request.clone();
                r.echo_all = echo_reasoning;
                r
            };
            stream_chat(
                ChatRequest {
                    state,
                    api,
                    base_url: &provider.base_url,
                    model: &model.id,
                    credential_ref: provider.credential_ref.as_deref(),
                    turns: &turns_now,
                    tools: &tools,
                    reasoning: reasoning_for_round,
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
                            // 摘掉 `[标题]…[/标题]`（回声也摘）：喂给界面的**只有**摘过之后的文本
                            let visible = stripper
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(&chunk);
                            // 再摘环节标记：摘出来的环节**就地落块 + 当场发事件** ——
                            // 界面要在这个环节的**第一张工具卡之前**就画出环节头
                            // （顺序反过来就是"工具卡先冒出来、分组后到"，与思考那次同一个毛病）。
                            let (visible, phases) = steps
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(&visible);
                            for phase in phases {
                                sink.push_step(&phase);
                                emit(
                                    json!({ "messageId": assistant, "phase": phase }),
                                    event_type::STEP_STARTED,
                                )?;
                            }
                            if !visible.is_empty() {
                                sink.push_text(&visible);
                                emit(
                                    json!({ "messageId": assistant, "text": visible }),
                                    event_type::TEXT_DELTA,
                                )?;
                            }
                        }
                        StreamEvent::ReasoningDelta(chunk) => {
                            sink.push_reasoning(&chunk);
                            emit(
                                json!({ "messageId": assistant, "text": chunk }),
                                event_type::REASONING_DELTA,
                            )?;
                        }
                        // 工具调用的分片在 [`crate::provider::upstream::Outcome::tool_calls`] 里拼装，
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
                // 摘标题 / 环节的两个缓冲里可能还压着东西（还没确认是不是标记），失败时也要放出来。
                let (pending, phases) = drain_strippers(&stripper, &steps);
                let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
                for phase in phases {
                    sink.push_step(&phase);
                }
                if !pending.is_empty() {
                    sink.push_text(&pending);
                }
                sink_now(state, &assistant_id, &sink)?;
                // 已跑完的那几轮的 usage 同样要记上（用户 bug「未完成的对话不统计 token 用量」）：
                // 失败只说明这一轮没跑完，前面几轮烧掉的 token 是真实发生的。
                crate::ai::runs::update_message(
                    state,
                    &assistant_id,
                    &sink.text_body(),
                    "failed",
                    last_provider_response_id.as_deref(),
                    usage_json(total_usage).as_ref(),
                )?;
                return Err(e);
            }
        };

        finish_reason = idle.finish_reason.clone();
        // ① 只是"这一轮没有思维链可回传"：**补条目，不降档**。余下的轮次照样思考。
        if idle.echo_forced && !echo_reasoning {
            echo_reasoning = true;
            eprintln!("[run {run_id}] 思维链回传被拒：改为逐轮补空 reasoning 项，思考强度保持不变");
        }
        // ② 连补了条目都还被拒：只能降档，并且**在正文里说一句**（看不到的现象，
        //    用户与模型都只能靠猜）。
        if idle.thinking_forced_off && !thinking_forced_off {
            thinking_forced_off = true;
            let note = "\n\n（本轮模型没有产生思考，而上游在思考模式下要求逐轮回传思维链；\
                        补上空条目后仍被拒绝，为避免后续请求被拒，本次回复余下的请求已关闭思考。）";
            push_note(state, run_id, &assistant_id, &sink, note)?;
            eprintln!("[run {run_id}] 思维链回传被上游拒绝（补条目无效）：本次 Run 余下请求改用 effort=none");
        }
        // 摘到标题就**立刻**写进会话并广播（界面左侧列表马上改名）；
        // 只有第一次算数 —— 后面那些是回声（`TitleStripper` 里已经只认第一次）。
        if !title_applied {
            let extracted = stripper
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .title()
                .map(|t| t.to_string());
            if let Some(title) = extracted {
                apply_conversation_title(state, run_id, &conversation_id, &title)?;
                title_applied = true;
            }
        }
        total_usage = Some(
            total_usage
                .unwrap_or_default()
                .merge(idle.usage.unwrap_or_default()),
        );
        if idle.provider_response_id.is_some() {
            last_provider_response_id = idle.provider_response_id.clone();
        }

        // 这一轮有没有工具调用？没有就收尾（正常结束）
        let mut accumulator = crate::provider::protocol::ToolCallAccumulator::new();
        for (index, key, call_id, name, fragment) in &idle.tool_calls {
            accumulator.push(
                *index,
                key,
                call_id.as_deref(),
                name.as_deref(),
                fragment.as_deref(),
            );
        }
        let calls = accumulator.finish();
        if calls.is_empty() {
            // **被输出长度上限截断**：再要一轮把话补完，而不是把半句当结尾（Kotlin `MAX_CONTINUATIONS`）。
            // 上限 3 次是个折中：常见模型一轮 4096 tokens（≈3000 汉字），3 次够写完一篇长回答；
            // 再多就是"模型陷在长回复里出不来"，用户等到的是账单和一直转圈的页面。
            if is_truncated_by_output_limit(finish_reason.as_deref())
                && continuations < MAX_CONTINUATIONS
            {
                continuations += 1;
                // ⚠ 续写请求里要带上**已经说过的那半段**（作为最后一条 assistant 轮）：
                // 少了它，模型看到的上下文是"用户突然说'接着写'"，它会从头再讲一遍
                // （Kotlin 那边同样把这一轮的正文留在 turns 里）。
                // 思考也一并取走：这一段思考属于**刚刚这一轮**，留到下一轮会被重复回传
                // （续写的下一轮同样会带上这条 assistant 轮）。
                let (partial, partial_reasoning) = {
                    let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
                    (guard.text.clone(), guard.seal_reasoning())
                };
                if !partial.trim().is_empty() {
                    trailing.push(ChatTurn {
                        role: "assistant".into(),
                        content: partial,
                        reasoning: Some(partial_reasoning).filter(|r| !r.trim().is_empty()),
                        ..Default::default()
                    });
                }
                trailing.push(ChatTurn {
                    role: "user".into(),
                    content: crate::ai::nsfw_prompt::CONTINUE_INSTRUCTION.to_string(),
                    ..Default::default()
                });
                continue;
            }
            break;
        }

        // 额度用尽（用户建议 ③）：**下一轮只让模型收口**，不再执行工具。
        // 如实把"为什么没继续"补进正文 —— 悄悄丢掉那批工具调用等于假装它答完了。
        if budget_exhausted {
            let note = format!(
                "\n\n（{}）",
                crate::tools::tools::budget_exceeded_note(
                    max_calls,
                    session.calls < crate::tools::tools::ABSOLUTE_CALL_CAP
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

        // 助手这一轮的正文 + 工具调用，作为**下一轮的上下文**。
        //
        // ⚠ **封段之后才 take**（R6 的关键一处）：`take_text()` 会把这段正文从 sink 里**拿走**，
        // 而这段正文同时必须留在 `parts` 里 —— 否则"这一轮说完话、下一轮撞 429"时，
        // 已经说出去的正文在库里**一个字都没有**（界面上那半段凭空消失）。
        // Kotlin 那边是"先 `parts.add("text", …)` 再起工具轮"，顺序完全一样。
        let (round_text, round_reasoning) = {
            let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
            let text = guard.text.clone();
            guard.seal_text();
            // 这一轮的**思维链**与正文在**同一个时点**封段：无状态的 responses / DeepSeek
            // 在思考模式下要求把每一轮的思维链原文回传（少了它就是
            // `400 The reasoning_text in the thinking mode must be passed back to the API.`），
            // 而同一段思考只属于一轮 —— 留在 sink 里会被挂到下一轮的助手轮上（重复回传）。
            (text, guard.seal_reasoning())
        };
        trailing.push(ChatTurn {
            role: "assistant".into(),
            content: round_text,
            tool_calls: calls.iter().map(|(_, call)| call.clone()).collect(),
            tool_call_id: None,
            images: Vec::new(),
            reasoning: Some(round_reasoning).filter(|r| !r.trim().is_empty()),
        });

        // 一个一个执行（**串行**：多个调用里只要有一个要用户点头，就得按顺序来）
        //
        // 参考图先攒着、**这一轮所有工具结果都推完之后**再挂成一条 user 轮：
        // 一次回复里并行调好几个工具时，中间插一条 user 消息会打断
        // "assistant(tool_calls) → tool…" 的邻接关系（OpenAI 直接 400，
        // Anthropic 的 tool_result 也要求紧跟着 tool_use）。
        let mut reference_batches: Vec<crate::ai::reference_attach::Batch> = Vec::new();
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
                &conversation_id,
                max_calls,
            )
            .await?;
            // ⚠ **只有真的跑成功才消耗额度**（Kotlin `spendCall` 只在 handler 返回之后调）：
            // 越界被拒、参数不对、用户点拒绝、工具自己报错 —— 那些调用"连世界都没碰到"，
            // 让它们占额度就是用户抱怨过的那种"什么都没干就说预算不足"。
            if outcome.ok {
                // **只有真的跑成功才消耗额度**（Kotlin spendCall 的时点）；
                // 联网那一档另有一份计数（`MAX_WEB_QUERIES_PER_RUN`，见下面的预检）。
                //
                // ⚠ 命中缓存的那一次走 `spend_cached_call`：总调用额度照记
                //   （它同样占了一轮上下文、同样该受 `maxCallsPerRun` 管），
                //   但**不占**联网 / ComfyUI 那两条 —— 它一次真实请求都没发。
                if outcome.cached {
                    session.spend_cached_call(&call.name);
                } else {
                    session.spend_call(&call.name);
                }
            }
            // 工作流参考图 / 产物预览：结果里带 `referenceImages` 时，把图挂到
            // **工具结果之后的一条 user 轮**上（tool 轮装不了图 —— OpenAI / Anthropic
            // 都不接受 tool 消息里的图片）。只有真的跑成功了才挂：失败结果里没有可用的路径。
            //
            // ⚠ 只认**我们自己这两条**工具：MCP / 其它工具的结果是不可信数据，不能让一个
            // 外来的 `referenceImages` 决定"读用户机器上的哪个文件并发给上游"。
            if outcome.ok
                && (call.name == "comfy_load_workflow" || call.name == "comfy_view_outputs")
            {
                if let Some(batch) = crate::ai::reference_attach::plan(
                    &outcome.result,
                    can_inline_reference_images,
                    &mut reference_budget,
                    &mut attached_reference_paths,
                ) {
                    reference_batches.push(batch);
                }
            }
            // 工具结果作为下一轮的 `role = tool`
            trailing.push(ChatTurn {
                role: "tool".into(),
                content: outcome.content,
                tool_calls: Vec::new(),
                tool_call_id: Some(call_id.clone()),
                images: Vec::new(),
                reasoning: None,
            });
        }
        if !reference_batches.is_empty() {
            // 多条合并成**一条** user 轮：连续两条 user 消息在 OpenAI 那边是合法的，
            // 但没必要；图按节点顺序排，正文逐条列出（含"没附上的"那句）。
            let mut text = String::new();
            let mut images = Vec::new();
            for batch in reference_batches {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&batch.text);
                images.extend(batch.images);
            }
            trailing.push(ChatTurn {
                role: "user".into(),
                content: text,
                images,
                ..Default::default()
            });
        }

        step += 1;
        // 每一轮之后重新算：用户刚答完提问的话额度已经被重置（Kotlin 同）
        budget_exhausted = !session.has_calls_left(max_calls);
    }

    // ---- 落库 --------------------------------------------------------------
    // 两个摘标记状态机的缓冲兜底放行（模型没按格式写标记时，那些字一个字都不能少）
    let (pending, phases) = drain_strippers(&stripper, &steps);
    let (body, completed_parts, reasoning_text) = {
        let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        for phase in phases {
            sink.push_step(&phase);
        }
        if !pending.is_empty() {
            sink.push_text(&pending);
        }
        sink_now(state, &assistant_id, &sink)?;
        (sink.text_body(), parts_json(&sink), sink.reasoning.clone())
    };

    // usage 事件的载荷是**平铺的四个键**（契约 §2 的 `inputTokens` / `outputTokens` /
    // `cachedTokens` / `reasoningTokens`）—— 与手机 harness 的 `EventPayloads.usage` 同形。
    // ⚠ 别退回 `{ "usage": {...} }`：契约那一列写的就是这四个键本身。
    if let Some(usage) = total_usage {
        state
            .bus
            .emit(state, run_id, event_type::USAGE_UPDATED, &usage.to_json())?;
    }

    let provider_response_id =
        last_provider_response_id.unwrap_or_else(|| format!("{}-{}", provider.id, run_id));
    crate::ai::runs::update_message(
        state,
        &assistant_id,
        &body,
        "complete",
        Some(&provider_response_id),
        usage_json(total_usage).as_ref(),
    )?;
    state.bus.emit(
        state,
        run_id,
        event_type::MESSAGE_COMPLETED,
        &json!({
            "messageId": assistant_id,
            "text": body,
            // 键集合与 Kotlin `HarnessRunner:838` **逐字对齐**（界面与 e2e 都读它们）：
            // `parts` 是**有序块**（少了它，重开会话/另一台设备画不出工具卡），
            // `finishReason` / `elapsedMs` / `steps` 是界面上的"这一轮怎么结束的"。
            "reasoning": if reasoning_text.is_empty() { Value::Null } else { json!(reasoning_text) },
            "finishReason": finish_reason.clone().unwrap_or_else(|| "stop".to_string()),
            "elapsedMs": started.elapsed().as_millis() as i64,
            "reasoningEffort": effort_stored,
            "steps": step,
            "parts": completed_parts,
            "usage": usage_json(total_usage),
        }),
    )?;

    // 模型这一轮没起标题（没写 `[标题]` / 写坏了）→ 用**首条用户消息**兜底。
    // 用户 2026-10-25：两轮新对话都叫「新对话」，列表里根本分不清谁是谁。
    // 只在标题还是默认值时才写：用户自己改过 / 模型已经起过，一个字都不动。
    if !title_applied && conversation_needs_title(state, &conversation_id) {
        if let Some(text) = first_user_message_text(state, &conversation_id) {
            if let Some(title) = title_from_user_message(&text) {
                apply_conversation_title(state, run_id, &conversation_id, &title)?;
            }
        }
    }

    finish_run(
        &state.bus,
        state,
        run_id,
        "completed",
        None,
        None,
        total_usage,
    )?;
    Ok(())
}

/// 审批等待的上限（秒）。超时 = 拒绝（与 Kotlin 的 `ToolApprovalGate` 同语义）。
pub const APPROVAL_TIMEOUT_SECONDS: u64 = 300;

/// "接着写"最多来几轮（Kotlin `HarnessRunner.MAX_CONTINUATIONS`）。
pub const MAX_CONTINUATIONS: usize = 3;

/// 上游给的结束原因里，哪些表示"是被输出长度上限截断的"
/// （与 Kotlin `isTruncatedByOutputLimit` 逐字一致）。
pub fn is_truncated_by_output_limit(finish_reason: Option<&str>) -> bool {
    matches!(
        finish_reason.map(|r| r.to_lowercase()).as_deref(),
        Some("length") | Some("max_tokens") | Some("max_output_tokens") | Some("max_output_token")
    )
}

/// 首条用户消息的正文（没有就 `None`）。
fn first_user_message_text(state: &AppState, conversation_id: &str) -> Option<String> {
    state
        .with(|conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT text FROM ai_messages \
                      WHERE conversation_id = ?1 AND role = 'user' ORDER BY seq LIMIT 1",
                    rusqlite::params![conversation_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
        .ok()
        .flatten()
}

/// 首条用户消息 → 标题兜底：**去掉卡片槽位标记，取第一行前 20 字**。
///
/// 典型输入（工作台把 `@` 选的卡原样拼在正文前面）：
///
/// ```text
/// [STYLE SPEC]
/// 二次元三次元混搭画风
/// Physically based anime rendering: …
/// ```
///
/// 去掉 `[STYLE SPEC]` / `[CHARACTER CARD]` 这两行之后，第一行是卡名 —— 它比
/// 「新对话」有信息量，也不会把整段画风正文塞进标题。
fn title_from_user_message(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| !matches!(*line, "[STYLE SPEC]" | "[CHARACTER CARD]"))?;
    let title: String = line.chars().take(20).collect();
    let title = title.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// 这个会话还该不该让模型**起标题**（Kotlin `shouldSummarizeTitle`）。
///
/// 标题不是空的、也不是「新对话」→ 说明用户自己改过（或已经起过）→ **尊重用户，不再覆盖**。
///
/// 2026-10-25 起补了一条**兜底**（见 [`title_from_user_message`]）：模型没起标题时，
/// 用首条用户消息的前 20 字当标题 —— 否则两轮对话在列表里都叫「新对话」。
/// 兜底只在**这次 Run 收尾、标题仍是默认值**时触发，模型起的标题 / 用户改的名字优先。
fn conversation_needs_title(state: &AppState, conversation_id: &str) -> bool {
    let title = state
        .with(|conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT title FROM ai_conversations WHERE id = ?1",
                    rusqlite::params![conversation_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
        .ok()
        .flatten();
    matches!(
        title.as_deref().map(str::trim),
        None | Some("") | Some("新对话")
    )
}

/// 把模型给的标题写进会话，并**如实广播**给界面（`conversation.updated`）。
///
/// 写失败只记日志、**不打断 Run**：一个自动标题不值得让整轮回答失败（Kotlin 同）。
fn apply_conversation_title(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    title: &str,
) -> Result<(), ApiError> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let updated = state.write(|tx| {
        tx.execute(
            "UPDATE ai_conversations SET title = ?1 WHERE id = ?2",
            rusqlite::params![
                trimmed.chars().take(255).collect::<String>(),
                conversation_id
            ],
        )?;
        Ok(())
    });
    if let Err(error) = updated {
        eprintln!(
            "[title] 写自动标题失败 conversation={conversation_id}：{}",
            error.human()
        );
        return Ok(());
    }
    state.bus.emit(
        state,
        run_id,
        event_type::CONVERSATION_UPDATED,
        &json!({
            "runId": run_id,
            "conversationId": conversation_id,
            "title": trimmed,
        }),
    )?;
    Ok(())
}
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
    /// 这一次是**命中缓存**得到的（没有真跑）。
    ///
    /// 用途只有一个：额度那边要分开记（[`crate::tools::tools::ToolSession::spend_cached_call`]）——
    /// 命中不发任何真实请求，就不该占"联网 / ComfyUI 查询"那两份额度。
    cached: bool,
}

impl CallOutcome {
    /// 失败记录（Kotlin `ToolRegistry.failure`）：**正文与摘要的形状都是契约的一部分** ——
    /// 正文是 `[工具失败:CODE] 人话`（模型据此换做法），摘要是 `[CODE] 人话`（界面一行小字）。
    fn failure(code: &str, message: &str, approval: &str, elapsed_ms: i64) -> Self {
        Self {
            content: format!("[工具失败:{code}] {message}"),
            preview: crate::tools::tools::preview_of(&format!("[{code}] {message}")),
            ok: false,
            code: Some(code.to_string()),
            approval: approval.to_string(),
            result: Value::Null,
            elapsed_ms,
            // 失败**从不**走缓存（`tool_cache` 纪律 6），所以这里恒为 false
            cached: false,
        }
    }
}

/// 把模型写在参数里的 `reason` 摘出来（见 `tools::spec_of_view`）。
///
/// 返回 `(摘过之后的参数 JSON 文本, reason)`。参数不是对象 / 不是合法 JSON 时原样返回。
/// `reason` 只认字符串；写了别的类型也**照样摘掉**（否则会漏进工具参数）。
fn take_reason(arguments: &mut String) -> Option<String> {
    let mut value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object_mut()?;
    let raw = object.remove("reason")?;
    let reason = raw
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    if let Ok(text) = serde_json::to_string(&value) {
        *arguments = text;
    }
    reason
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
    policy: &crate::tools::policy::ToolPolicy,
    session: &mut crate::tools::tools::ToolSession,
    call_id: &str,
    call: &crate::provider::protocol::ToolCallRef,
    sink: &Arc<Mutex<Sink>>,
    conversation_id: &str,
    max_calls: i64,
) -> Result<CallOutcome, ApiError> {
    // `reason` 是**给用户看的**（工具卡标题），不是工具参数：
    // 先摘掉再解析，后面缓存键、落库、执行、回填历史用的都是摘过的那一份。
    // 只有这一处摘 —— 工具 handler 永远看不到它（见 `tools::spec_of_view` 的注释）。
    let mut call = call.clone();
    // 模型偶尔把整份参数再包一层（`{"arguments": "\"{\\\"path\\\":…}\""}`）：
    // 先解回正常对象，`reason` 才摘得到，工具也才拿得到 `path`。
    // 解包只发生在**执行 / 展示**这一份副本上；回给上游的那条 assistant 轮仍是原文
    // （见下面 `trailing.push(ChatTurn { tool_calls: calls … })`）—— 不许替模型改它的历史。
    call.arguments = crate::tools::tools::normalize_arguments_text(&call.arguments);
    let reason = take_reason(&mut call.arguments);
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
    // **只读工具的缓存命中**（`tools::tool_cache`）：这一句是**纯内存查询**，
    // 什么都不碰、也不改任何闸门的顺序 —— 它只影响两件事：
    //   ① 出网 / ComfyUI 那两条"别拿外部服务当轮询器"的额度**要不要扣**（命中不扣：
    //      没有产生任何真实请求，凭什么占那份额度）；
    //   ② ⑥ 那一步要不要真跑。
    // ⚠ **它排在审批之前只是为了"额度判定"**，真正的取用（`into_wire`）在审批之后 ——
    //    不然"用户没批准"就会被一份缓存悄悄顶掉（见 `tool_cache` 模块头纪律 3）。
    let cached = crate::tools::tool_cache::lookup(policy, &call.name, &arguments);
    // 出厂权限档：静态清单里没有的名字，可能是运行时拉到的 MCP 工具（第 45 步）——
    // 那一批的档位由 `McpToolPolicy` 决定（`run_workflow` = ASK、`update_comfyui` = DENY…）。
    // 两个来源都没这个工具时回 `ASK`（保守：宁可多问一次，也不默默放开一个写操作）。
    let fallback = state
        .mcp
        .access_of(&call.name)
        .unwrap_or_else(|| crate::tools::tools::default_access(&call.name));
    let access = policy.access_for(&call.name, fallback);
    let needs_approval = access == crate::tools::policy::ToolAccess::Ask;
    // 工具卡（界面按它画"正在调用 / 已拒绝"）：先落调用块，再落结果块。
    // 参数用**原文**（不是解析后的对象）：Dart 侧 `map['arguments']` 读的就是这个字符串。
    {
        let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        sink.push_tool_call(call_id, &call.name, &call.arguments, reason.as_deref());
    }

    // ① 落库（R8：闸门开的同一刻就落，用户手快也点得到）
    if needs_approval {
        crate::ai::runs::insert_pending_tool_call(
            state,
            run_id,
            call_id,
            &call.name,
            &call.arguments,
        )?;
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
            // 只给界面看：工具卡标题用它代替机器名（没有就退回到前端那张名字表）
            "reason": reason,
            "approval": if needs_approval { "pending" } else { "not_required" },
        }),
    )?;
    state.bus.flush(state, run_id)?;
    // ③ 只读工具：马上就是"开始执行"（需要审批的要等用户点头之后才发，见下）
    if !needs_approval {
        emit_started(state, run_id, call_id, &call.name)?;
    }

    // ④ 三道预检，顺序照抄 Kotlin `ToolRegistry.invoke`（找不到 → 额度 → 策略）
    if !crate::tools::tools::is_implemented(&call.name) && !state.mcp.has(&call.name) {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            &call,
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
    if !session.has_calls_left(max_calls) {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            &call,
            sink,
            CallOutcome::failure(
                "TOOL_BUDGET_EXCEEDED",
                &crate::tools::tools::budget_exceeded_note(
                    max_calls,
                    session.calls < crate::tools::tools::ABSOLUTE_CALL_CAP,
                ),
                "denied",
                0,
            ),
        )
        .await;
    }
    // 联网那档**另有一条独立预算**（Kotlin `ToolRegistry.invoke` 里 `ToolCategory.WEB` 那一段）：
    // 每搜一次都是一次真实的外部请求 + 一份要烧 token 的正文，"再搜一下确认"很容易刹不住车。
    // 系统提示里写着"一次回复最多联网 N 次"——**写了就必须真拦**，否则那句话是空的。
    //
    // ⚠ 命中缓存的那一次**不占**这份额度：它一次外部请求都没发（`tools::tool_cache`）。
    //    额度拦的是"真请求"，重复的那一次交给缓存拦。
    if cached.is_none()
        && crate::ai::ai_settings::category_of(&call.name) == "web"
        && !session.has_web_queries_left()
    {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            &call,
            sink,
            CallOutcome::failure(
                "QUERY_BUDGET_EXCEEDED",
                &format!(
                    "本次回复联网查询的次数已达上限（{} 次）；\
                     请基于已有的搜索结果作答，需要更多资料时如实告诉用户",
                    crate::tools::policy::MAX_WEB_QUERIES_PER_RUN
                ),
                "not_required",
                0,
            ),
        )
        .await;
    }
    // **ComfyUI 那一档也有一条独立预算**（用户 AIH-036："不许把 ComfyUI 当轮询器"；
    // Kotlin `ToolRegistry.invoke` 里 `ToolCategory.COMFY` 那一段）。系统提示里写着
    // "一次回复最多查 ComfyUI N 次" —— **写了就必须真拦**，否则那句话是空的
    // （这正是 2026-09-25 那次审查记下的"提示词里写了、代码里没有"）。
    // 命中缓存的那一次同样不占（同联网那条）。
    if cached.is_none()
        && crate::ai::ai_settings::is_comfy_query(&call.name)
        && !session.has_comfy_queries_left()
    {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            &call,
            sink,
            CallOutcome::failure(
                "QUERY_BUDGET_EXCEEDED",
                &format!(
                    "本次回复查询 ComfyUI 的次数已达上限（{} 次）；\
                     请基于已有信息作答，或让用户稍后再问",
                    crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN
                ),
                "not_required",
                0,
            ),
        )
        .await;
    }
    if access == crate::tools::policy::ToolAccess::Deny {
        return finish_failure(
            state,
            run_id,
            assistant_id,
            call_id,
            &call,
            sink,
            CallOutcome::failure(
                "TOOL_DENIED",
                &format!(
                    "工具「{}」被用户策略禁止使用（设置 → AI 工具权限）",
                    call.name
                ),
                "denied",
                0,
            ),
        )
        .await;
    }

    // ⑤ 审批：等用户在**任何一台设备**上点（读库：跨端可见）
    let mut approval = if needs_approval {
        "pending"
    } else {
        "not_required"
    }
    .to_string();
    if needs_approval {
        if !wait_for_approval(state, run_id, call_id).await? {
            let outcome = CallOutcome::failure(
                "APPROVAL_DENIED",
                "用户没有批准这次工具调用（或等待超时）；不要重试同一件事，如实说明即可",
                "denied",
                APPROVAL_TIMEOUT_SECONDS as i64 * 1000,
            );
            return finish_failure(state, run_id, assistant_id, call_id, &call, sink, outcome)
                .await;
        }
        approval = "approved".to_string();
        // 批准的那一刻就告诉界面"开始跑了"，而不是等它跑完再补发
        emit_started(state, run_id, call_id, &call.name)?;
    }

    // ⑥ 执行 —— **或者取缓存**（只读工具、参数完全相同、还在新鲜期内）
    let started = std::time::Instant::now();
    let outcome = match cached {
        // 命中：不执行、不烧那两条额度。正文与摘要由 `into_wire` **如实标注**
        // （"同一工具 + 同一组参数在 N 秒前跑过一次，没有重新执行"）——
        // 悄悄返回旧数据会让模型把几分钟前的队列状态当成此刻的事实，见 `tool_cache` 纪律 2。
        Some(hit) => {
            let (content, preview, payload) = hit.into_wire();
            CallOutcome {
                content,
                preview,
                ok: true,
                code: None,
                approval,
                result: payload,
                elapsed_ms: started.elapsed().as_millis() as i64,
                cached: true,
            }
        }
        None => {
            let result = crate::tools::tools::execute(
                state,
                policy,
                run_id,
                conversation_id,
                &call.name,
                &arguments,
                session,
            )
            .await;
            let elapsed_ms = started.elapsed().as_millis() as i64;
            match result {
                Ok(out) => {
                    let (content, truncated) = crate::tools::tools::truncate_result(&out.content);
                    // 结构化结果：工具给了就用它，否则"截断过"这件事本身也要让界面知道
                    let mut meta = out.payload.clone();
                    if truncated {
                        if let Some(map) = meta.as_object_mut() {
                            map.insert("truncated".into(), Value::Bool(true));
                        } else {
                            meta = json!({ "truncated": true });
                        }
                    }
                    // 存缓存：**只对 `freshness` 认的纯读工具**、且**只存成功的那一次**
                    // （一次超时/429 是"这一次"的事实，存下来等于把偶发故障变成长达几分钟的必然失败）
                    crate::tools::tool_cache::store(
                        policy, &call.name, &arguments, &content, &meta,
                    );
                    CallOutcome {
                        content,
                        preview: out.preview,
                        ok: true,
                        code: None,
                        approval,
                        result: meta,
                        elapsed_ms,
                        cached: false,
                    }
                }
                Err(failure) => {
                    CallOutcome::failure(&failure.code, &failure.message, &approval, elapsed_ms)
                }
            }
        }
    };

    // 写类工具跑成功 → 缓存里那份"世界的描述"就过期了（纪律 4：清空是唯一的失效动作）。
    // ⚠ 放在 `store` 之后也无妨：写类工具**本来就不在** `freshness` 名单里，两者不会互相打架。
    if outcome.ok && crate::tools::tool_cache::invalidates(&call.name) {
        crate::tools::tool_cache::invalidate_all();
    }

    // ⑦ 落库 + 发事件 + 落块（失败也走同一条尾巴，**绝不静默**）
    let rec = crate::ai::runs::ToolCallRecord {
        id: crate::ai::conversations::uuid_v4()?,
        run_id: run_id.to_string(),
        call_id: call_id.to_string(),
        name: call.name.clone(),
        arguments_json: call.arguments.clone(),
        approval: outcome.approval.clone(),
        status: if outcome.ok { "ok" } else { "failed" }.to_string(),
        result_json: if outcome.result.is_null() {
            None
        } else {
            Some(outcome.result.clone())
        },
        content: Some(outcome.content.clone()),
        error: outcome.code.as_ref().map(|_| outcome.error_text()),
        elapsed_ms: outcome.elapsed_ms,
    };
    crate::ai::runs::record_tool_call(state, &rec)?;
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
    sink.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_tool_result(call_id, &call.name, &outcome);
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
    state.bus.emit(
        state,
        run_id,
        event_type::TOOL_STARTED,
        &json!({ "callId": call_id, "name": name }),
    )?;
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
    call: &crate::provider::protocol::ToolCallRef,
    sink: &Arc<Mutex<Sink>>,
    outcome: CallOutcome,
) -> Result<CallOutcome, ApiError> {
    let rec = crate::ai::runs::ToolCallRecord {
        id: crate::ai::conversations::uuid_v4()?,
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
    crate::ai::runs::record_tool_call(state, &rec)?;
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
    sink.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_tool_result(call_id, &call.name, &outcome);
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
    sink.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_text(note);
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
async fn wait_for_approval(
    state: &AppState,
    run_id: &str,
    call_id: &str,
) -> Result<bool, ApiError> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECONDS);
    loop {
        if state.executor.is_cancelled(run_id) {
            return Ok(false);
        }
        match crate::ai::runs::approval_state(state, call_id)? {
            Some(crate::ai::runs::ApprovalState::Approved) => return Ok(true),
            Some(crate::ai::runs::ApprovalState::Denied) => return Ok(false),
            // `None`：那一行还没落（同一条连接上的可见性问题，几乎不可能）—— 继续等
            Some(crate::ai::runs::ApprovalState::Pending) | None => {}
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
    /// 思考段（累加：收尾时落在最前面，`parts` 里那一条 `reasoning` 块就是它）
    reasoning: String,
    /// **本轮**的思考（`seal_reasoning()` 时交给刚刚那一轮助手轮）。
    ///
    /// 与 `reasoning` 分开是必须的：`reasoning` 是整条回复的思考（要落成块给界面看），
    /// 而这里要的是"这一轮工具往返里模型想了什么" —— 无状态协议（responses/DeepSeek）
    /// 要求把**每一轮**的思维链按轮回传，把整条回复的思考重复挂到每一轮上就重复了。
    reasoning_round: String,
    /// 已经封好的块（`(type, text, tool_call_id, json_payload)`）
    parts: Vec<(String, String, Option<String>, Option<Value>)>,
}

impl Sink {
    fn push_text(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    fn push_reasoning(&mut self, chunk: &str) {
        self.reasoning.push_str(chunk);
        self.reasoning_round.push_str(chunk);
    }

    /// 当前这段正文封成块（工具卡之前必须调 —— 否则工具卡会插到正文前面去）。
    fn seal_text(&mut self) {
        if !self.text.is_empty() {
            let sealed = std::mem::take(&mut self.text);
            self.parts.push(("text".into(), sealed, None, None));
        }
    }

    /// **本轮**的思考取走（与 [`Sink::seal_text`] 同一个时点：工具往返开始时）。
    ///
    /// 取走而不是读：同一段思考只归属一轮，留着会被挂到下一轮的助手轮上（重复回传）。
    fn seal_reasoning(&mut self) -> String {
        std::mem::take(&mut self.reasoning_round)
    }

    /// 关键步骤（环节）块：`[步骤]正在查资料[/步骤]` 摘出来的那个名字。
    ///
    /// 与工具卡一样**先封正文段**：环节头必须落在"上一段正文之后、这个环节的工具卡之前"，
    /// 界面就是按这个位置把后面的工具卡折进这一组的。
    fn push_step(&mut self, phase: &str) {
        if phase.is_empty() {
            return;
        }
        self.seal_text();
        self.parts
            .push(("step".into(), phase.to_string(), None, None));
    }

    /// 工具卡块。`arguments` 是**模型给的原文**（Kotlin 也把它当字符串放进 payload）——
    /// 存成对象的话，Dart 侧 `map['arguments'].toString()` 会变成 `{query: x}` 这种
    /// 既不是 JSON、也没法回填进输入框的东西。
    fn push_tool_call(&mut self, call_id: &str, name: &str, arguments: &str, reason: Option<&str>) {
        self.seal_text();
        self.parts.push((
            "tool_call".into(),
            String::new(),
            Some(call_id.to_string()),
            Some(json!({ "name": name, "arguments": arguments, "reason": reason })),
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
            // 多问卡其余几题（2026-11-04）：不落的话重开 App 只剩第一题，
            // 裁图与选项示意图也跟着一起丢（`askedAll` 是后端给整组的那一份）。
            if let Some(asked_all) = outcome.result.get("askedAll") {
                map.insert("askedAll".into(), asked_all.clone());
            }
            // 2D 布局模板（2026-11-03）：它不像图片有 id 可以事后重新拉 ——
            // 规格本身必须落进块里，否则重开 App 之后那条回复里的模板卡就没了。
            if let Some(templates) = outcome.result.get("layoutTemplates") {
                map.insert("layoutTemplates".into(), templates.clone());
            }
        }
        self.parts.push((
            "tool_result".into(),
            outcome.content.clone(),
            Some(call_id.to_string()),
            Some(payload),
        ));
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

/// 把两个摘标记状态机的缓冲一起放出来（**收尾那三条路都要走它**：取消 / 失败 / 正常）。
///
/// 顺序必须与流式路径一致（标题 → 环节），否则同一段话在"跑完"与"失败"两条路上的正文顺序会不一样。
/// 返回 `(应当进入正文的文本, 还认出来的环节)`：环节是**真的标记**，照样要落成 `step` 块。
fn drain_strippers(
    stripper: &Arc<Mutex<crate::ai::title_strip::TitleStripper>>,
    steps: &Arc<Mutex<crate::ai::step_mark::StepStripper>>,
) -> (String, Vec<String>) {
    let pending = stripper.lock().unwrap_or_else(|e| e.into_inner()).flush();
    let mut guard = steps.lock().unwrap_or_else(|e| e.into_inner());
    let (visible, phases) = guard.push(&pending);
    let tail = guard.flush();
    (format!("{visible}{tail}"), phases)
}

/// 把攒下来的内容整理成**有序块列表**（`(type, text, tool_call_id, json_payload)` 文本）。
///
/// 抽出来是为了让"落库"与"`message.completed` 事件里的 `parts`"用**同一份**列表 ——
/// 两处各拼一次，迟早出现"库里有的块、事件里没有"（界面读的是事件，重开会话读的是库，
/// 于是同一段回复在两处长得不一样）。
fn parts_of(sink: &Sink) -> Vec<(String, String, Option<String>, Option<String>)> {
    let mut parts: Vec<(String, String, Option<String>, Option<String>)> = Vec::new();
    if !sink.reasoning.is_empty() {
        parts.push(("reasoning".into(), sink.reasoning.clone(), None, None));
    }
    for (kind, text, tool_call_id, payload) in &sink.parts {
        parts.push((
            kind.clone(),
            text.clone(),
            tool_call_id.clone(),
            payload.as_ref().map(|v| v.to_string()),
        ));
    }
    if !sink.text.is_empty() {
        parts.push(("text".into(), sink.text.clone(), None, None));
    }
    parts
}

/// [`parts_of`] → `message.completed` 里的 `parts`（形状 = `AiMessagePartDto`，可空键省略）。
fn parts_json(sink: &Sink) -> Vec<Value> {
    parts_of(sink)
        .into_iter()
        .map(|(kind, text, tool_call_id, payload)| {
            let mut out = serde_json::Map::new();
            out.insert("type".into(), json!(kind));
            if !text.is_empty() {
                out.insert("text".into(), json!(text));
            }
            if let Some(call_id) = tool_call_id.filter(|c| !c.is_empty()) {
                out.insert("toolCallId".into(), json!(call_id));
            }
            if let Some(payload) = payload.and_then(|p| serde_json::from_str::<Value>(&p).ok()) {
                out.insert("jsonPayload".into(), payload);
            }
            Value::Object(out)
        })
        .collect()
}

/// 把攒下来的内容落成有序块（**收尾的每一条路都要调**）。
fn sink_now(state: &AppState, assistant_id: &str, sink: &Sink) -> Result<(), ApiError> {
    if assistant_id.is_empty() || sink.is_empty() {
        return Ok(());
    }
    let parts = parts_of(sink);
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

/// 一次 Run 的附件计划：哪些**内联**、哪些只能如实说"没发出去"。
///
/// 判定顺序（与 Kotlin `HarnessRunner.turnsFromHistory` 同一套）：
///  1. **当前模型声不声明这种模态**（不声明 → 不发）；
///  2. **当前协议适配器实现了这种传输方式吗**（三家都只有图片的 `inline_base64`）；
///  3. **内联预算**：单张 ≤ 8MB、合计 ≤ 20MB（贪心保序：装不下就跳过、继续试后面的小图）；
///  4. 读得到文件吗（读不到就是"发不出去"，同样进未发送那一句）。
///
/// ⚠ 四条里任何一条不过，**都只是"不发"**，绝不让整条请求失败 —— 而且那一项必须出现在
/// "以下附件未随本次请求发送"那句里（AIH-030：不许静默丢掉）。
struct AttachmentPlan {
    names: std::collections::HashMap<String, String>,
    inline: std::collections::HashMap<String, crate::provider::protocol::ChatImage>,
}

impl AttachmentPlan {
    fn empty() -> Self {
        Self {
            names: std::collections::HashMap::new(),
            inline: std::collections::HashMap::new(),
        }
    }

    fn name_of(&self, id: &str) -> String {
        self.names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
}

fn attachment_plan_of(
    state: &AppState,
    history: &[AiMessageDto],
    model: &crate::ai::run_lifecycle::ModelRow,
    api: &str,
) -> AttachmentPlan {
    // ① 历史里出现过的附件，按**首次出现顺序**列出来（保序：预算的"装不下"才有确定答案）
    let mut ids: Vec<String> = Vec::new();
    for message in history {
        for part in message.parts.iter().filter(|p| p.r#type == "attachment") {
            if let Some(id) = part.attachment_id.as_ref() {
                if !id.trim().is_empty() && !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    if ids.is_empty() {
        return AttachmentPlan::empty();
    }

    // ② 查事实（名字 / 模态 / 大小）；查不到的**如实留 id**
    let mut plan = AttachmentPlan::empty();
    let mut facts: Vec<Option<crate::media::attachments::AttachmentDto>> = Vec::new();
    for id in &ids {
        let dto = crate::media::attachments::get(state, id).ok().flatten();
        if let Some(dto) = dto.as_ref() {
            plan.names.insert(id.clone(), dto.name.clone());
        }
        facts.push(dto);
    }

    // ③ 该协议的适配器实现了哪些传输方式（图片才可能内联）
    let can_inline_image =
        crate::media::strict_intake::adapter_transports(api, "image").contains(&"inline_base64");
    let model_takes_image = model.input_modalities.iter().any(|m| m == "image");

    // ④ 预算：只有"图片 + 适配器实现 + 模型声明了图片"的才参与
    let mut sizes: Vec<i64> = Vec::new();
    let mut eligible: Vec<bool> = Vec::new();
    for dto in facts.iter() {
        let ok = can_inline_image
            && model_takes_image
            && dto.as_ref().map(|d| d.modality == "image").unwrap_or(false);
        eligible.push(ok);
        sizes.push(if ok {
            dto.as_ref().map(|d| d.size_bytes).unwrap_or(0)
        } else {
            0
        });
    }
    let chosen = crate::media::strict_intake::inline_budget_plan(&sizes);

    // ⑤ 决定内联的：现在就把 base64 读出来（读不到就**不发**，如实进未发送名单）
    for (index, id) in ids.iter().enumerate() {
        if !eligible[index] || !chosen[index] {
            continue;
        }
        let Some(dto) = facts[index].as_ref() else {
            continue;
        };
        let Some(base64) = crate::media::attachments::inline_base64(state, dto) else {
            continue;
        };
        plan.inline.insert(
            id.clone(),
            crate::provider::protocol::ChatImage {
                mime_type: dto.mime_type.clone(),
                base64,
                file_name: dto.name.clone(),
            },
        );
    }
    plan
}

/// 一条历史消息 → 上游的轮次（`HarnessRunner.turnsFromHistory` 的精简版）。
///
/// 规则：
/// * `user` / `assistant` 的**正文**进上下文；正文空但有工具调用的助手轮也要进
///   （否则上游看到的工具结果**没有对应的调用**，OpenAI 会直接 400）；
/// * `tool_result` 块变成 `role = "tool"` 的一轮（带 `tool_call_id`）；
/// * 思考段**不进**上下文（它是本地展示用的；上游各家都不接受回传思考）；
/// * **附件块**（R9 / AIH-030）：进得了 [`AttachmentPlan::inline`] 的变成真图片块
///   （适配器各自铺开），其余的**必须**在那一轮正文后面附一句"以下附件未随本次请求发送：…"
///   —— 既不能静默丢弃，也不能因为消息里带了附件就把请求打成 400。
/// 本 Run 的用户消息里由 `/名字` 显式调用的技能（正文快照）。
///
/// 快照在 `POST /runs` 落库时由后端现读磁盘写入，不是前端传来的正文 ——
/// 前端只负责说"用了哪个名字"。
fn explicit_skills_of(
    messages: &[crate::ai::conversations::AiMessageDto],
    user_message_id: Option<&str>,
) -> Vec<(String, Option<String>, String)> {
    let Some(user_message_id) = user_message_id else {
        return Vec::new();
    };
    let Some(message) = messages.iter().find(|m| m.id == user_message_id) else {
        return Vec::new();
    };
    message
        .parts
        .iter()
        .filter(|p| p.r#type == "mention")
        .filter_map(|p| {
            let payload = p.json_payload.as_ref()?;
            if payload.get("kind").and_then(|v| v.as_str()) != Some("skill") {
                return None;
            }
            let name = payload.get("name").and_then(|v| v.as_str())?.to_string();
            let body = payload.get("body").and_then(|v| v.as_str())?.to_string();
            if body.trim().is_empty() {
                return None;
            }
            let version = payload
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Some((name, version, body))
        })
        .collect()
}

fn turns_of(message: &AiMessageDto, plan: &AttachmentPlan) -> Vec<ChatTurn> {
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
                images: Vec::new(),
                // ⚠ 思考段**不进正文**（它是展示用的，拼进正文会让模型看到自己的碎片），
                // 但**要跟着这一轮走**：无状态的 responses / DeepSeek 在思考模式下要求
                // 把历史轮次的思维链原文回传（`ChatTurn::reasoning`，completions / anthropic
                // 的适配器一个字节都不发）。少了它，下一轮直接
                // `400 The reasoning_text in the thinking mode must be passed back to the API.`
                reasoning: reasoning_of(&message.parts),
            });
        }
        // ⚠ **助手消息里的工具结果也要作为独立的一轮回填**（`role = "tool"`）。
        // 一次失败/中断的助手消息会把"这一轮的正文 + 工具卡 + 工具结果"落在**同一条**消息里；
        // 只发助手那一轮、把工具结果丢掉，上游看到的最后一条就是 assistant ——
        // 而假网关（以及真实模型）靠"最后一条是不是工具结果"判断"工具跑完了、该收口"，
        // 于是它会**再调一次同一个工具**（实测：重试时多跑了一轮工具）。
        for part in message.parts.iter().filter(|p| p.r#type == "tool_result") {
            out.push(ChatTurn {
                role: "tool".into(),
                content: part.text.clone().unwrap_or_default(),
                tool_calls: Vec::new(),
                tool_call_id: part.tool_call_id.clone(),
                images: Vec::new(),
                reasoning: None,
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
        // 用户消息的 parts 现在是"卡片/技能 mention + 用户正文"：
        // mention 的 `modelText` 是**给模型看的那段原文**（卡正文），text 块是用户敲的字。
        // 二者按 ordinal 顺序拼回去，才是模型这一轮该看到的那条消息。
        let mut out = String::new();
        for part in &message.parts {
            match part.r#type.as_str() {
                "text" => {
                    if let Some(text) = &part.text {
                        out.push_str(text);
                    }
                }
                "mention" => {
                    if let Some(model_text) = part
                        .json_payload
                        .as_ref()
                        .and_then(|p| p.get("modelText"))
                        .and_then(|v| v.as_str())
                    {
                        out.push_str(model_text);
                    }
                }
                _ => {}
            }
        }
        out
    };
    // 附件块：进得了计划的变成真图片块，其余**全部**进"未随本次请求发送"那一句
    let mut images: Vec<crate::provider::protocol::ChatImage> = Vec::new();
    let mut unsent: Vec<String> = Vec::new();
    for id in message
        .parts
        .iter()
        .filter(|p| p.r#type == "attachment")
        .filter_map(|p| p.attachment_id.clone())
    {
        match plan.inline.get(&id) {
            Some(image) => images.push(image.clone()),
            None => unsent.push(format!("{}（id={id}）", plan.name_of(&id))),
        }
    }
    let body = if unsent.is_empty() {
        body
    } else {
        let mut text = body;
        if !text.trim().is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("（以下附件未随本次请求发送：");
        text.push_str(&unsent.join("、"));
        text.push('）');
        text
    };
    // 附件 id 对照（用户 bug 2026-09-28「系统给不出可投放的附件 ID」）：工具描述让模型
    // "见用户消息里的附件说明"，但从来没有人把**内联图片的 id**写进消息 —— 模型手里
    // 没有 id，`comfy_use_attachment` 第一步就走不下去，ComfyUI 端也就读不到聊天里的图。
    // （没内联的那些 id 已经在上面"未随本次请求发送"那句里了，这里不重复。）
    let inlined_ids: Vec<String> = message
        .parts
        .iter()
        .filter(|p| p.r#type == "attachment")
        .filter_map(|p| p.attachment_id.clone())
        .filter(|id| plan.inline.contains_key(id))
        .map(|id| format!("{}=id:{id}", plan.name_of(&id)))
        .collect();
    let body = if inlined_ids.is_empty() {
        body
    } else {
        let mut text = body;
        if !text.trim().is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(
            "（随本条消息内联的附件 —— 要把某张图放进 ComfyUI 的 input 目录，\
             用 comfy_use_attachment 投它对应的 id：",
        );
        text.push_str(&inlined_ids.join("；"));
        text.push('）');
        text
    };
    if !body.trim().is_empty() || !images.is_empty() {
        out.push(ChatTurn {
            role: message.role.clone(),
            content: body,
            tool_calls: Vec::new(),
            tool_call_id: None,
            images,
            reasoning: None,
        });
    }
    // 工具结果：作为**独立的一轮**回填（`role = "tool"` + `tool_call_id`）
    for part in message.parts.iter().filter(|p| p.r#type == "tool_result") {
        out.push(ChatTurn {
            role: "tool".into(),
            content: part.text.clone().unwrap_or_default(),
            tool_calls: Vec::new(),
            tool_call_id: part.tool_call_id.clone(),
            images: Vec::new(),
            reasoning: None,
        });
    }
    out
}

/// 助手轮里的**思维链原文**（`reasoning` 块拼起来）。
///
/// 只有 [`crate::provider::protocol::ChatTurn::reasoning`] 需要它的协议（`openai-responses`）才会发出去；
/// 正文里**一个字都不许混进来**（那是模型自己的碎片，拼进正文会让它以为"这是我说过的话"）。
fn reasoning_of(parts: &[AiMessagePartDto]) -> Option<String> {
    let text: String = parts
        .iter()
        .filter(|p| p.r#type == "reasoning")
        .filter_map(|p| p.text.clone())
        .collect();
    Some(text).filter(|t| !t.trim().is_empty())
}

/// 助手轮里的工具调用（`json_payload` 里是 `{name, arguments}`）。
fn tool_calls_of(parts: &[AiMessagePartDto]) -> Vec<crate::provider::protocol::ToolCallRef> {
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
            Some(crate::provider::protocol::ToolCallRef {
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
mod tests;
