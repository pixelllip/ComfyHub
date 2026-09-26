//! Run 的生命周期端点（P4 第 28 步）：**创建 / 查询 / 取消 / 事件流（SSE）**。
//!
//! # 这一批解决什么
//!
//! 在此之前，指向 Rust 后端时 AI 工作台的「发送」按钮**什么都不做**（路由不存在 → 404）。
//! 现在这四条端点都在了，界面拿到的东西与 Kotlin 完全一致：
//!
//! | 端点 | 语义 |
//! | --- | --- |
//! | `POST /api/ai/conversations/{id}/runs` | 校验 → 落用户消息 + 助手占位 → 建 Run → **202** |
//! | `GET /api/ai/runs/{id}` | 这一次 Run 的状态（刷新页面后也能看到它是不是还在跑） |
//! | `GET /api/ai/runs/{id}/events?after=N` | **SSE**：事件流，`after` 断线续传 |
//! | `POST /api/ai/runs/{id}/cancel` | 取消（`{cancelled, status}`） |
//!
//! # 事件流为什么**从库里拉**（而不是内存订阅通道）
//!
//! Kotlin 用内存订阅通道，那有一条缝：内存环形缓冲只留最近 2000 条，
//! `after` 比它更老时**取不到**那段历史。而库里那份是批量的、可能还没 flush。
//! 本实现让 SSE 直接按 `?after=` 从 `ai_run_events` 拉，于是：
//! 顺序 / 去重 / 续传 / 重启回放**全都由 `(run_id, seq)` 主键保证**，
//! 一句"从内存订阅"都不需要写 —— 这是"库是唯一真源"的直接好处。
//!
//! 两条副作用要认下来：
//! * 延迟 = 轮询间隔（[`POLL_INTERVAL_MS`]，120ms）—— 对逐字流式完全够用；
//! * 空闲时每 [`HEARTBEAT_SECONDS`] 秒发一条 `heartbeat`（与 Kotlin 的 15s 一致），
//!   否则中间的代理/防火墙会以为连接死了。
//!
//! # 一次 Run 的收尾**必须**两条腿都齐
//!
//! ① `runs::finish` 落终态；② `bus.close`（**flush 掉最后那不足一批的事件**）。
//! 漏了 ②，最后那条 `run.completed` 就补不回来 —— 那正是 R6 的反面。

use std::convert::Infallible;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::response::Response;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::events::{event_type, RunEventBus, TERMINAL_EVENTS};
use crate::routes::{ApiError, AppState};
use crate::runs::{self, NewRun};

/// SSE 轮询间隔（毫秒）。120ms ≈ 每秒 8 次读一条 `seq > cursor` 的索引查询 ——
/// 对 SQLite WAL 是白菜价，而界面上看起来就是"逐字在流"。
pub const POLL_INTERVAL_MS: u64 = 120;

/// 空闲多久发一条 `heartbeat`（与 Kotlin `respondTextWriter` 里的 15s 一致）。
pub const HEARTBEAT_SECONDS: u64 = 15;

/// 一次 Run 的总时限（小时）：到点还没收尾就按失败收（避免僵死 Run 永久占着界面）。
///
/// Kotlin 那边没有这一条 —— 这里加它是因为 Rust 侧的执行器是**进程内**的，
/// 一个卡住的上游连接不会有人来打断它。**不是**与 Kotlin 的行为差异：
/// 超过这个时长的 Run 在两边都是"用户已经关掉页面了"。
pub const RUN_TIMEOUT_HOURS: i64 = 2;

/// `POST /api/ai/conversations/{id}/runs` 的入参（`AiRunStartRequest`）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartRequest {
    pub text: String,
    pub provider_id: String,
    pub model_id: String,
    /// off / low / medium / high / max；**缺省字段 = off**（AIH-056）
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    #[serde(default)]
    pub retry_of_run_id: Option<String>,
}

/// `POST .../runs` 的响应（202）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartResponse {
    pub run_id: String,
    pub assistant_message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
}

/// `POST /api/ai/runs/{id}/cancel` 的响应。
#[derive(Debug, Serialize)]
pub struct CancelResult {
    pub cancelled: bool,
    pub status: String,
}

/// 模型目录里与"起一次 Run"相关的几列（`ai_models`）。
///
/// `Clone`：执行器把它搬进后台任务（`spawn` 不能在请求的生命周期里借东西）。
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub id: String,
    pub display_name: String,
    pub api: String,
    pub base_url: String,
    pub credential_ref: Option<String>,
    pub endpoint_trust: String,
    pub tools: bool,
    pub reasoning: bool,
    pub thinking_efforts: std::collections::BTreeMap<String, String>,
    pub thinking_format: Option<String>,
    pub max_output_tokens: Option<i64>,
    /// `ai_models.input_modalities`（附件那一层要判"这个模型吃不吃图"）
    pub input_modalities: Vec<String>,
}

/// 这条助手消息有没有**有序块**（Kotlin `AiConversationRepo.hasParts()`）。
///
/// 重试判"空壳"时必须问它：一次失败的回复里，内容可能只在块里（工具卡 / 半段正文），
/// 只看 `text` 就会把有内容的回复当空壳清掉。
fn assistant_has_parts(state: &AppState, message_id: &str) -> Result<bool, ApiError> {
    state.with(|conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_message_parts WHERE message_id = ?1",
            rusqlite::params![message_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    })
}

/// 读 provider（不存在 → 400 `CONFIG_ERROR`，与 Kotlin `requireProvider` 同一个信封）。
pub fn provider(state: &AppState, id: &str) -> Result<ModelRow, ApiError> {
    let row = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, display_name, api, base_url, credential_ref, endpoint_trust, enabled
               FROM ai_providers WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok((
                row.get::<_, String>("id")?,
                row.get::<_, String>("display_name")?,
                row.get::<_, String>("api")?,
                row.get::<_, String>("base_url")?,
                row.get::<_, Option<String>>("credential_ref")?,
                row.get::<_, String>("endpoint_trust")?,
                row.get::<_, Option<i64>>("enabled")?.unwrap_or(0) != 0,
            ))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    let Some((id, display_name, api, base_url, credential_ref, endpoint_trust, enabled)) = row else {
        return Err(ApiError::ai("CONFIG_ERROR", format!("Provider 不存在: {id}")));
    };
    if !enabled {
        return Err(ApiError::ai("CONFIG_ERROR", "该 Provider 已被停用"));
    }
    Ok(ModelRow {
        id,
        display_name,
        api,
        base_url,
        credential_ref,
        endpoint_trust,
        // provider 上没有这几个字段：它们属于模型，由 `model()` 覆盖
        tools: false,
        reasoning: false,
        thinking_efforts: Default::default(),
        thinking_format: None,
        max_output_tokens: None,
        input_modalities: Vec::new(),
    })
}

/// 读某个 provider 下的模型（不存在 → 400 `UNKNOWN_MODEL`）。
pub fn model(state: &AppState, provider_id: &str, model_id: &str) -> Result<ModelRow, ApiError> {
    let mut row = provider(state, provider_id)?;
    let found = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT * FROM ai_models WHERE provider_id = ?1 AND model_id = ?2")?;
        let mut rows = stmt.query_map(rusqlite::params![provider_id, model_id], |row| {
            Ok((
                row.get::<_, Option<i64>>("tools")?.unwrap_or(0) != 0,
                row.get::<_, Option<i64>>("reasoning")?.unwrap_or(0) != 0,
                crate::ai::thinking_efforts_of(row)?,
                row.get::<_, Option<String>>("thinking_format")?,
                row.get::<_, Option<i64>>("max_output_tokens")?,
                row.get::<_, Option<i64>>("enabled")?.unwrap_or(0) != 0,
                crate::ai::model_input_modalities_of(row)?,
            ))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    let Some((tools, reasoning, efforts, format, max_out, enabled, modalities)) = found else {
        return Err(ApiError::ai("UNKNOWN_MODEL", format!("模型不在目录中: {model_id}")));
    };
    if !enabled {
        return Err(ApiError::ai("CONFIG_ERROR", "该模型已被停用"));
    }
    row.id = model_id.to_string();
    row.tools = tools;
    row.reasoning = reasoning;
    row.thinking_efforts = efforts;
    row.thinking_format = format.filter(|s| !s.trim().is_empty());
    row.max_output_tokens = max_out;
    row.input_modalities = modalities;
    Ok(row)
}

/// 思考强度（AIH-056）：**模型是唯一真源**。
///
/// * 没传 / `off` → `Ok(None)`（一个思考参数都不发，网关自己决定）；
/// * 模型没声明推理能力 → 400（**不静默当 off**）；
/// * 模型显式列了可选等级 → 只允许表里的；
/// * 都不认识的值 → 400。
///
/// 返回 `(落库值, 发给上游的值)`：落库一律是 `off` 或等级名（Run 的"当时生效值"），
/// 发给上游的 `off` 是 `None`（"不指定"与"明确关掉"在上游是两件事）。
pub fn thinking_effort(m: &ModelRow, requested: Option<&str>) -> Result<(String, Option<String>), ApiError> {
    const LEVELS: [&str; 5] = ["off", "low", "medium", "high", "max"];
    let requested = requested.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("off");
    if !LEVELS.contains(&requested) {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("未知的思考强度：{requested}（可选 {}）", LEVELS.join(" / ")),
        ));
    }
    if requested == "off" {
        return Ok(("off".to_string(), None));
    }
    if !m.reasoning {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("模型 {} 未声明推理能力，不能设置思考强度（可在「设置 → AI 模型」里声明）", m.id),
        ));
    }
    let declared: Vec<String> = m.thinking_efforts.keys().cloned().filter(|k| k != "off").collect();
    if declared.is_empty() {
        return Ok((requested.to_string(), Some(requested.to_string())));
    }
    if !declared.iter().any(|k| k == requested) {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("模型 {} 未声明思考强度 {requested}（可选：{}）", m.id, declared.join(" / ")),
        ));
    }
    Ok((requested.to_string(), Some(requested.to_string())))
}

/// `POST /api/ai/conversations/{id}/runs`。
///
/// 顺序**照抄 Kotlin**（`AiRoutes.kt:358`），每一步都不能挪：先校验（不通过就是一封
/// 400 + 稳定码，**一个字节都不写库**），再落用户消息与助手占位，最后才建 Run。
/// 顺序错了的表现是"消息进去了、Run 没建成"—— 会话里凭空多一条提问。
pub fn start(state: &AppState, conversation_id: &str, req: &RunStartRequest) -> Result<RunStartResponse, ApiError> {
    // ① 会话必须存在（不存在 → CONFIG_ERROR）
    crate::conversations::get(state, conversation_id)?;

    // ② 正文与附件不能都空
    let attachment_ids: Vec<String> = req
        .attachment_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if req.text.trim().is_empty() && attachment_ids.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "消息内容不能为空"));
    }

    // ③ provider + 模型 + 思考强度（三步都只读，任何一步不过就到此为止）
    let provider = provider(state, &req.provider_id)?;
    let model = model(state, &req.provider_id, &req.model_id)?;
    let (effort_stored, effort_wire) = thinking_effort(&model, req.reasoning_effort.as_deref())?;

    // ④ 附件准入（R9）：**创建 Run 之前**用库里的事实再验一次。
    //
    // 三条纪律（AIH-030）：
    //  ① 判定在这里、在 append 用户消息**之前**完成 —— 不通过就直接返回，**上游请求数为 0**；
    //  ② 事实取自**库里**（`ai_attachments`），前端声明只是线索（AIH-027）；
    //  ③ 不许把 blocker 降级成占位符再继续发（"看起来发出去了、其实没有"是明令禁止的）。
    //
    // 顺序也要紧：去重是**保序**的（Kotlin `distinct()` 同）——
    // `currentAttachmentCount` 按序号传，顺序一变"第几个超了数量上限"就变了。
    let mut unique_ids: Vec<String> = Vec::new();
    for id in &attachment_ids {
        if !unique_ids.contains(id) {
            unique_ids.push(id.clone());
        }
    }
    if !unique_ids.is_empty() {
        // 准入要的是模型的**能力声明**（模态 / 传输 / 白名单 / 上限），
        // 那是 `AiModelDto` 才有的字段（`ModelRow` 只带执行要用的那几个）。
        // 目录里找不到就如实报 —— 绝不"读不到声明就当它什么都支持"。
        let admission_model = crate::ai::list_models(state, &req.provider_id)?
            .into_iter()
            .find(|m| m.id == req.model_id)
            .ok_or_else(|| {
                ApiError::ai("UNKNOWN_MODEL", format!("模型不在目录中: {}", req.model_id))
            })?;
        let mut blockers: Vec<String> = Vec::new();
        for (index, id) in unique_ids.iter().enumerate() {
            let dto = crate::attachments::get(state, id)?
                .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件不存在或已被删除: {id}")))?;
            let fact = crate::strict_intake::AttachmentFact {
                name: dto.name.clone(),
                modality: Some(dto.modality.clone()),
                mime_type: dto.mime_type.clone(),
                size_bytes: dto.size_bytes,
                pixels: match (dto.width, dto.height) {
                    (Some(w), Some(h)) => Some(w * h),
                    _ => None,
                },
            };
            let transports = crate::strict_intake::adapter_transports(
                &provider.api,
                fact.modality.as_deref().unwrap_or_default(),
            );
            let result =
                crate::strict_intake::evaluate(&admission_model, &fact, &transports, index as i64);
            for blocker in result.blockers {
                blockers.push(format!("{}：{blocker}", dto.name));
            }
        }
        if !blockers.is_empty() {
            return Err(ApiError::ai(
                "UNSUPPORTED_CONTENT",
                format!(
                    "有附件未通过准入，已阻断发送（未产生任何上游请求）：{}",
                    blockers.join("；")
                ),
            ));
        }
    }

    // ⑤ 重试的三种情况（AIH-024）：旧 Run 必须属于这条会话；旧助手消息只有
    //    failed / cancelled 才允许接着用；**有内容的失败回复原样留着**、重试另起一条（R6）。
    let retry_of = match req.retry_of_run_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let old = runs::get(state, id)?;
            if old.conversation_id != conversation_id {
                return Err(ApiError::ai("CONFIG_ERROR", "要重试的那次运行不属于这条会话"));
            }
            Some(old)
        }
        None => None,
    };
    let retry_assistant = match retry_of.as_ref().and_then(|r| r.assistant_message_id.clone()) {
        Some(message_id) => {
            let message = message_of(state, &message_id)?;
            let status = message.1.clone();
            if status != "failed" && status != "cancelled" {
                return Err(ApiError::ai(
                    "CONFIG_ERROR",
                    format!("那条回复的状态是 {status}，不需要（也不能）重试；直接发一条新消息即可"),
                ));
            }
            Some((message_id, message.0))
        }
        None => None,
    };

    // ⑥ 用户消息：重试时**复用原来那条**，绝不新增一行
    //    （用户报的"发送消息时容易出现同一条语句重复"就是这么来的）
    let user_message_id = match retry_of.as_ref().and_then(|r| r.user_message_id.clone()) {
        Some(id) => Some(id),
        None => {
            // 正文 + **每个附件一个有序块**（Kotlin 同）：块是附件与消息之间唯一的关联，
            // 少了它界面画不出缩略图、执行器也不知道"这条消息带了图"（R9 的"不许静默丢弃"）。
            let mut parts = vec![crate::conversations::AiMessagePartDto {
                r#type: "text".into(),
                text: Some(req.text.clone()),
                attachment_id: None,
                tool_call_id: None,
                json_payload: None,
            }];
            for id in unique_ids.iter() {
                parts.push(crate::conversations::AiMessagePartDto {
                    r#type: "attachment".into(),
                    text: None,
                    attachment_id: Some(id.clone()),
                    tool_call_id: None,
                    json_payload: None,
                });
            }
            let appended = crate::conversations::append_message(
                state,
                conversation_id,
                &crate::conversations::MessageAppend {
                    role: "user".into(),
                    text: req.text.clone(),
                    status: "complete".into(),
                    provider_id: Some(provider.id.clone()),
                    model_id: Some(model.id.clone()),
                    id: None,
                    parts,
                },
            )?;
            Some(appended.id)
        }
    };

    // ⑦ 助手消息：空的失败气泡**就地复用**；有内容的**原样留着**、另起一条
    //
    // ⚠ "空"的判据是 **`isEmptyReply()` 的两条同时成立**（正文空 **且** 一个块都没有）——
    // 只看正文会把"说了半句话 + 一张工具卡"的失败回复当成空壳，重试时**就地清掉**：
    // 用户报过的"429 之后看不到已经回复完的内容"正是这么丢的（R6 第三条纪律）。
    let assistant_message_id = match retry_assistant {
        Some((message_id, text))
            if text.trim().is_empty() && !assistant_has_parts(state, &message_id)? =>
        {
            runs::reset_assistant_for_retry(state, &message_id, &provider.id, &model.id)?;
            message_id
        }
        _ => runs::insert_assistant_placeholder(state, conversation_id, &provider.id, &model.id)?,
    };

    // ⑧ 建 Run（快照**不含密钥**，只有引用名 —— AIH-023）
    let run = runs::create(
        state,
        NewRun {
            conversation_id: conversation_id.to_string(),
            provider_id: provider.id.clone(),
            model_id: model.id.clone(),
            provider_snapshot: json!({
                "id": provider.id,
                "displayName": provider.display_name,
                "api": provider.api,
                "baseURL": provider.base_url,
                "endpointTrust": provider.endpoint_trust,
                "credentialRef": provider.credential_ref,
            }),
            model_snapshot: json!({
                "id": model.id,
                "displayName": model.display_name,
                "tools": model.tools,
                "reasoning": model.reasoning,
                "capabilitySource": "manual",
                "nsfw": crate::run_kit::nsfw_enabled(state)?,
                "nsfwPromptVersion": crate::nsfw_prompt::PROMPT_VERSION,
            }),
            // 与 run-kit 的 `promptVersion` 是**同一个常量**（探针里那个 `PROMPT_VERSION`
            // 就是 `SystemPrompt.VERSION`）：两处各写一份，迟早"kit 说 v28、Run 记 v27"
            prompt_version: crate::nsfw_prompt::PROMPT_VERSION.to_string(),
            user_message_id: user_message_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            retry_of_run_id: req.retry_of_run_id.clone(),
            reasoning_effort: Some(effort_stored),
            skill_snapshot: Some(crate::skills::snapshot(state)?),
        },
    )?;

    // ⑨ 开流 + **交给执行器**（整条链在下一批：上游客户端 → 协议适配器 → 主循环）
    state.bus.open(&run.id);
    crate::executor::spawn(state, &run.id, &provider, &model, effort_wire, req.text.clone());

    Ok(RunStartResponse {
        run_id: run.id,
        assistant_message_id,
        user_message_id,
    })
}

/// 读一条消息的 `(正文, 状态)`。
fn message_of(state: &AppState, message_id: &str) -> Result<(String, String), ApiError> {
    state.with(|conn| {
        let mut stmt = conn.prepare("SELECT text, status FROM ai_messages WHERE id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![message_id], |row| {
            Ok((
                row.get::<_, Option<String>>("text")?.unwrap_or_default(),
                row.get::<_, Option<String>>("status")?.unwrap_or_else(|| "complete".into()),
            ))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?
    .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("消息不存在: {message_id}")))
}

/// `POST /api/ai/runs/{id}/cancel`。
///
/// 已经不是 running 的 Run **照样回 200**（`{cancelled:false, status}`）——
/// 界面上"我点了取消，但它在 0.2 秒前刚答完"是常态，那不是错误。
pub fn cancel(state: &AppState, run_id: &str) -> Result<CancelResult, ApiError> {
    let run = runs::get(state, run_id)?;
    if run.status != runs::RUNNING {
        return Ok(CancelResult { cancelled: false, status: run.status });
    }
    // 执行器是进程内的：把取消旗标立起来，主循环下一轮自己收尾
    // （它负责落终态 + flush + 发 run.cancelled —— 见 `executor`）
    let cancelled = crate::executor::cancel(state, run_id);
    Ok(CancelResult {
        cancelled,
        status: if cancelled { "cancelling".to_string() } else { run.status },
    })
}

/// `GET /api/ai/runs/{id}/events?after=N` 的 SSE 响应体。
///
/// # 为什么是"轮询 + 从库里拉"
///
/// 内存订阅通道有一条缝：环形缓冲只留 2000 条，`after` 更老时取不到那段历史；
/// 而库里那份是批量的、可能还没 flush。直接按 `?after=` 拉库里的，
/// **顺序 / 去重 / 续传 / 重启回放全都由 `(run_id, seq)` 主键保证**。
/// 代价是延迟 = [`POLL_INTERVAL_MS`]（对逐字流式完全够用）。
pub fn sse(state: AppState, run_id: String, after: i64) -> Response {
    let stream = SseStream {
        state,
        run_id,
        cursor: after,
        idle: Duration::ZERO,
        done: false,
    };
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .header(axum::http::header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

struct SseStream {
    state: AppState,
    run_id: String,
    cursor: i64,
    idle: Duration,
    done: bool,
}

impl Stream for SseStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }
        let this = &mut *self;
        // 一次 poll 里做完所有同步的活；**绝不在这里 await 一个未被 poll 的 future**
        // （那正是"第一个 await 还没完成就被丢弃 → 事务半截回滚"那类 bug 的来源）
        let batch = runs::events(&this.state, &this.run_id, this.cursor);
        match batch {
            Ok(events) if !events.is_empty() => {
                let mut out = String::new();
                for event in &events {
                    this.cursor = event.seq;
                    out.push_str(&event.to_sse());
                }
                this.idle = Duration::ZERO;
                return std::task::Poll::Ready(Some(Ok(Bytes::from(out))));
            }
            Ok(_) => {}
            Err(e) => {
                // 读库失败不该把连接静默掐掉：发一条 frame 让界面能说出原因，然后收尾
                this.done = true;
                return std::task::Poll::Ready(Some(Ok(Bytes::from(event_to_sse(
                    0,
                    "run.failed",
                    &json!({ "error": "DB_ERROR", "detail": format!("读事件失败：{}", e.human()) }),
                )))));
            }
        }

        // 没有新事件：终态 + 流已收尾 → 结束（**先确认库里的都发完了**，这一条要紧：
        // 否则"刚收尾那一刻的最后几条"会被掐掉，表现为界面少一个 run.completed）
        if this.state.bus.is_closed(&this.run_id) {
            if let Ok(run) = runs::get(&this.state, &this.run_id) {
                if !TERMINAL_EVENTS.contains(&run.status.as_str()) && run.status != runs::RUNNING {
                    this.done = true;
                    return std::task::Poll::Ready(None);
                }
            }
        }
        if let Ok(run) = runs::get(&this.state, &this.run_id) {
            if run.status != runs::RUNNING {
                // 已经收尾：库里的发完了就结束（不再等心跳）
                this.done = true;
                return std::task::Poll::Ready(None);
            }
        } else {
            this.done = true;
            return std::task::Poll::Ready(None);
        }

        // 空闲：到点发心跳
        if this.idle >= Duration::from_secs(HEARTBEAT_SECONDS) {
            this.idle = Duration::ZERO;
            return std::task::Poll::Ready(Some(Ok(Bytes::from(event_to_sse(
                0,
                event_type::HEARTBEAT,
                &json!({}),
            )))));
        }
        this.idle += Duration::from_millis(POLL_INTERVAL_MS);
        let waker = cx.waker().clone();
        let delay = Duration::from_millis(POLL_INTERVAL_MS);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            waker.wake();
        });
        std::task::Poll::Pending
    }
}

/// 一个 SSE 帧（`id` 为 0 时**不写 id 行** —— 心跳不是可续传的游标）。
fn event_to_sse(seq: i64, event_type: &str, payload: &Value) -> String {
    if seq <= 0 {
        format!("event: {event_type}\ndata: {payload}\n\n")
    } else {
        format!("id: {seq}\nevent: {event_type}\ndata: {payload}\n\n")
    }
}

/// 执行器收尾的**唯一入口**：落终态 + flush + 发终态事件 + 关流。
///
/// 三条路（正常 / 失败 / 取消）都必须走它 —— 少任何一步的表现都是
/// "界面永远转圈"或"断线重连补不回最后那句话"（R6）。
pub fn finish_run(
    bus: &RunEventBus,
    state: &AppState,
    run_id: &str,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    usage: Option<&Value>,
) -> Result<(), ApiError> {
    let event = match status {
        "completed" => event_type::RUN_COMPLETED,
        "cancelled" => event_type::RUN_CANCELLED,
        _ => event_type::RUN_FAILED,
    };
    let mut payload = json!({ "runId": run_id });
    if let Some(code) = error_code {
        // ⚠ 键名是 **`code` / `message`**（Kotlin `HarnessRunner` 的 `run.failed` 就是这么发的）。
        // 写成 `error` / `detail` 的话界面读不到失败原因 —— 只看到"运行失败"四个字。
        payload["code"] = json!(code);
    }
    if let Some(message) = error_message {
        payload["message"] = json!(message);
    }
    // 顺序：**先发事件再落终态**。反过来的话 SSE 可能先看到 `status != running`
    // 而结束流，那条终态事件就永远发不出去了。
    bus.emit(state, run_id, event, &payload)?;
    runs::finish(state, run_id, status, error_code, error_message, usage)?;
    // 这个 Run 上还没人回答的提问一律按失效收尾（Kotlin `AskRegistry.discardRun`）。
    // ⚠ 少了这一步，那些行会永远停在 pending：`/api/ai/pending` 一直把它们列出来，
    // 而点进去谁也答不了（Run 已经结束）—— 那就是一张"点了也没用"的卡。
    // 放在这里是因为它是**收尾的唯一入口**（正常 / 失败 / 取消三条路都走它）。
    if let Err(e) = crate::questions::finalize_run(state, run_id, "expired") {
        eprintln!("收尾提问失败（Run {run_id} 上还挂着 pending 行）：{}", e.human());
    }
    bus.close(state, run_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
