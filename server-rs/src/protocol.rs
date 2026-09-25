//! 上游协议适配器（P4 第 29 步）—— 逐条对照 Kotlin `protocol/Adapters.kt` + `ProtocolAdapter.kt`。
//!
//! # 这个模块只有纯函数
//!
//! 输入是"一轮对话 + 模型 + 思考强度"，输出是**请求体 JSON**；
//! 反向是"一行 SSE 文本"→ 一串 [`StreamEvent`]。
//! **一个字节的网络 I/O 都没有** —— 所以它能被穷举地测：
//! 请求体长什么样、工具调用怎么拼、usage 从哪来、`[DONE]` 怎么处理，
//! 全都是可以直接断言的东西。网络那一半在 `upstream.rs`。
//!
//! # 三家协议的差别（别把它们混起来）
//!
//! | | 路径 | 鉴权头 | 工具调用 | 思考 |
//! | --- | --- | --- | --- | --- |
//! | `openai-completions` | `<base>/chat/completions` | `Authorization: Bearer` | `tools[].function` / `delta.tool_calls` | `reasoning_effort` 等方言 |
//! | `openai-responses` | `<base>/v1/responses` | `Authorization: Bearer` | `function_call` 项 | `reasoning.effort` |
//! | `anthropic-messages` | `<base>/v1/messages` | `x-api-key` + `anthropic-version` | `tools[].input_schema` / `tool_use` 块 | `thinking.budget_tokens` |
//!
//! # 三条纪律
//!
//! 1. **认不出的帧返回空表，不抛**：上游会插 `: comment` 心跳、`event:` 行、
//!    甚至偶发半截 JSON —— 一条读不懂的帧不该让整轮 Run 失败。
//! 2. **工具调用是分片来的**：第一片带 `id` + `name`，后面只有 `index` +
//!    `arguments` 片段。拼装归 [`ToolCallAccumulator`]，**按 index 而不是按到达顺序**
//!    （网关并行下发时顺序是乱的）。
//! 3. **`[DONE]` 不是数据**：OpenAI 兼容网关用它收尾，适配器认得它但
//!    "收尾"这件事由调用方做（这里返回空表）。

use serde_json::{json, Map, Value};

/// 三家协议（wire 名与 Kotlin `AiApi` **逐字一致**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    OpenAiCompletions,
    OpenAiResponses,
    AnthropicMessages,
}

impl Api {
    pub fn parse(wire: &str) -> Option<Self> {
        match wire.trim() {
            "openai-completions" => Some(Self::OpenAiCompletions),
            "openai-responses" => Some(Self::OpenAiResponses),
            "anthropic-messages" => Some(Self::AnthropicMessages),
            _ => None,
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Self::OpenAiCompletions => "openai-completions",
            Self::OpenAiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }

    /// 对话端点（`HarnessRunner.chatUrl` 的逐字对应物）。
    ///
    /// ⚠ `/v1` 的处理**三家不一样**，照抄别"顺手统一"：
    /// completions 是 `<base>/chat/completions`（base 里本来就有 `/v1`），
    /// 另外两家在 base 没带 `/v1` 时**补一个**。
    pub fn chat_url(self, base_url: &str) -> String {
        let base = base_url.trim_end_matches('/');
        match self {
            Self::OpenAiCompletions => format!("{base}/chat/completions"),
            Self::OpenAiResponses => {
                if base.ends_with("/v1") {
                    format!("{base}/responses")
                } else {
                    format!("{base}/v1/responses")
                }
            }
            Self::AnthropicMessages => {
                if base.ends_with("/v1") {
                    format!("{base}/messages")
                } else {
                    format!("{base}/v1/messages")
                }
            }
        }
    }

    /// 附件传输方式（`AdapterCapabilities.transportsFor` 的对应物）。
    ///
    /// ⚠ **只有 completions 声明了图片**：另两家的内联图片还没实现 ——
    /// 声明了却发不出去，比"没声明"糟得多（预检会放行，然后静默丢掉图片）。
    pub fn supports_inline_image(self) -> bool {
        matches!(self, Self::OpenAiCompletions)
    }
}

/// 思考方言（`ThinkingFormat`，AIH-056）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingFormat {
    OpenAi,
    DeepSeek,
    Qwen,
    Zai,
    OpenRouter,
}

impl ThinkingFormat {
    pub fn parse(wire: &str) -> Option<Self> {
        match wire.trim().to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAi),
            "deepseek" => Some(Self::DeepSeek),
            "qwen" => Some(Self::Qwen),
            "zai" => Some(Self::Zai),
            "openrouter" => Some(Self::OpenRouter),
            _ => None,
        }
    }
}

/// 一次请求的思考强度（`ReasoningRequest`）。
///
/// `effort = None` = **不指定**（界面上的「默认」）：一个思考参数都不发。
/// `format` 决定用哪个字段表达（见 `apply_reasoning`）。
#[derive(Debug, Clone, Default)]
pub struct ReasoningRequest {
    pub effort: Option<String>,
    pub wire_value: Option<String>,
    pub format: Option<ThinkingFormat>,
}

/// 对话中的一轮（`ChatTurn`）。
#[derive(Debug, Clone, Default)]
pub struct ChatTurn {
    /// system / user / assistant / tool
    pub role: String,
    pub content: String,
    /// 助手发起的工具调用（回传给上游时必须原样带上）
    pub tool_calls: Vec<ToolCallRef>,
    /// 工具结果这一轮回填的调用 id（`role = "tool"` 时才有）
    pub tool_call_id: Option<String>,
}

/// 助手发起的一次工具调用（参数是**已经拼好的 JSON 文本**，原样回传）。
#[derive(Debug, Clone)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 提供给模型的工具定义（`ToolSpec`）。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// 适配器吐出来的流事件（`StreamEvent`）。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    /// 上游给的 usage（原样 JSON，形状各家不同）
    Usage(Value),
    /// 上游的响应 id（`provider_response_id`，事后追溯用）
    ProviderId(String),
    /// 工具调用分片
    ToolCallDelta {
        index: usize,
        /// 这一片的稳定键：有 id 用 id，没有就用 index（网关两边都可能给）
        key: String,
        call_id: Option<String>,
        name: Option<String>,
        arguments_fragment: Option<String>,
    },
    /// 上游明确说"这一轮结束了"（`finish_reason` / `stop_reason`）
    Finished(String),
}

// ---------------------------------------------------------------------------
//  SSE 分帧
// ---------------------------------------------------------------------------

/// SSE 分帧器（`SseAccumulator`）。
///
/// 一行一行喂进去，遇到**空行**就是帧结束。`event:` 行记下事件名，
/// `data:` 行**多行要拼接**（契约如此）。`:` 开头的是注释（心跳），丢掉。
///
/// ⚠ `[DONE]` 由调用方判（这里照原样交出去）—— 它是 OpenAI 兼容网关的收尾约定，
/// Anthropic 不用它。
#[derive(Debug, Default)]
pub struct SseAccumulator {
    event: Option<String>,
    data: String,
}

/// 一帧（`event` 可能为空 —— 大多数网关不发 `event:` 行）。
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}

impl SseAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一行；返回 `Some(frame)` 表示凑齐了一帧。
    pub fn line(&mut self, line: &str) -> Option<Frame> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if self.event.is_none() && self.data.is_empty() {
                return None;
            }
            let frame = Frame {
                event: self.event.take(),
                data: std::mem::take(&mut self.data),
            };
            return Some(frame);
        }
        if line.starts_with(':') {
            return None; // 注释 / 心跳
        }
        if let Some(rest) = line.strip_prefix("event:") {
            self.event = Some(rest.trim().to_string());
            return None;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            // 只去掉**一个**前导空格（`data:  x` 的第一个空格是分隔符，第二个是内容）
            self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        None
    }

    /// 流结束：把还没收尾的一帧交出来（有的网关最后不发空行）。
    pub fn flush(&mut self) -> Option<Frame> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        Some(Frame {
            event: self.event.take(),
            data: std::mem::take(&mut self.data),
        })
    }
}

// ---------------------------------------------------------------------------
//  工具调用分片拼装
// ---------------------------------------------------------------------------

/// 把分片的工具调用拼成完整调用（`ToolCallAccumulator`）。
///
/// **按 index 归位**，不按到达顺序：网关并行下发时顺序是乱的，
/// 按顺序拼会把 A 的参数接到 B 的调用上（结果是"工具调用参数看着像 JSON、其实张冠李戴"）。
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    drafts: std::collections::BTreeMap<usize, Draft>,
    /// index → 稳定键（后到的分片可能只带 index）
    keys: std::collections::HashMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
struct Draft {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一个分片。
    pub fn push(
        &mut self,
        index: usize,
        key: &str,
        call_id: Option<&str>,
        name: Option<&str>,
        arguments_fragment: Option<&str>,
    ) {
        let draft = self.drafts.entry(index).or_default();
        if let Some(id) = call_id.map(str::trim).filter(|s| !s.is_empty()) {
            draft.call_id = Some(id.to_string());
            self.keys.insert(key.to_string(), index);
        }
        if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
            draft.name = Some(name.to_string());
        }
        if let Some(fragment) = arguments_fragment {
            draft.arguments.push_str(fragment);
        }
    }

    /// 这一轮是不是真收到了工具调用（只有一个空壳不算）。
    pub fn is_empty(&self) -> bool {
        self.drafts.values().all(|d| d.name.is_none() && d.call_id.is_none())
    }

    /// 收口成 `(call_id, 完整调用)` 列表（按 index 升序）。
    ///
    /// `call_id` 缺失时**自己编一个**（有的网关不给 id，而回传工具结果时必须有）——
    /// 编出来的 id 前缀 `call_` + index，与上游给的 id 不会撞。
    pub fn finish(self) -> Vec<(String, ToolCallRef)> {
        self.drafts
            .into_iter()
            .filter_map(|(index, draft)| {
                let name = draft.name?;
                let id = draft.call_id.unwrap_or_else(|| format!("call_{index}"));
                Some((
                    id.clone(),
                    ToolCallRef { id, name, arguments: draft.arguments },
                ))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
//  openai-completions
// ---------------------------------------------------------------------------

/// 一个适配器的全部行为（`ProtocolAdapter`）。
pub trait Adapter {
    fn api(&self) -> Api;
    /// 这一轮要发给上游的请求体。
    fn build_body(
        &self,
        model: &str,
        turns: &[ChatTurn],
        tools: &[ToolSpec],
        reasoning: &ReasoningRequest,
        max_output_tokens: Option<i64>,
    ) -> Value;
    /// 一帧 → 一串事件（认不出来就空表）。
    fn interpret(&self, frame: &Frame) -> Vec<StreamEvent>;
}

/// 按 wire 名造适配器。
pub fn adapter_for(api: Api) -> Box<dyn Adapter + Send + Sync> {
    match api {
        Api::OpenAiCompletions => Box::new(OpenAiCompletionsAdapter),
        Api::OpenAiResponses => Box::new(OpenAiResponsesAdapter),
        Api::AnthropicMessages => Box::new(AnthropicMessagesAdapter),
    }
}

/// OpenAI 兼容 Chat Completions（覆盖 OpenAI 官方与绝大多数兼容网关）。
///
/// 纯文本轮的 `content` 用**最朴素的字符串**（兼容性最好），
/// 只有带附件时才变成有序块数组 —— 与 Kotlin 同。
pub struct OpenAiCompletionsAdapter;

impl Adapter for OpenAiCompletionsAdapter {
    fn api(&self) -> Api {
        Api::OpenAiCompletions
    }

    fn build_body(
        &self,
        model: &str,
        turns: &[ChatTurn],
        tools: &[ToolSpec],
        reasoning: &ReasoningRequest,
        max_output_tokens: Option<i64>,
    ) -> Value {
        let mut root = Map::new();
        root.insert("model".into(), json!(model));
        root.insert(
            "messages".into(),
            Value::Array(turns.iter().map(openai_message).collect()),
        );
        root.insert("stream".into(), json!(true));
        // 让上游在最后一个 chunk 里带 usage（网关不一定支持，忽略即可）
        root.insert("stream_options".into(), json!({ "include_usage": true }));
        if !tools.is_empty() {
            root.insert("tools".into(), openai_tools(tools));
        }
        if let Some(max) = max_output_tokens {
            root.insert("max_tokens".into(), json!(max));
        }
        apply_openai_reasoning(&mut root, reasoning);
        Value::Object(root)
    }

    fn interpret(&self, frame: &Frame) -> Vec<StreamEvent> {
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Vec::new();
        }
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(usage) = root.get("usage").filter(|v| !v.is_null()) {
            out.push(StreamEvent::Usage(usage.clone()));
        }
        if let Some(id) = root.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                out.push(StreamEvent::ProviderId(id.to_string()));
            }
        }
        let Some(choices) = root.get("choices").and_then(Value::as_array) else {
            return out;
        };
        for choice in choices {
            // 两种方言：`delta`（流式）与 `message`（个别网关在流里回完整消息）
            let delta = choice.get("delta").or_else(|| choice.get("message"));
            if let Some(delta) = delta {
                if let Some(text) = delta.get("content").and_then(Value::as_str) {
                    if !text.is_empty() {
                        out.push(StreamEvent::TextDelta(text.to_string()));
                    }
                }
                for field in ["reasoning_content", "reasoning"] {
                    if let Some(text) = delta.get(field).and_then(Value::as_str) {
                        if !text.is_empty() {
                            out.push(StreamEvent::ReasoningDelta(text.to_string()));
                            break;
                        }
                    }
                }
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (fallback, call) in calls.iter().enumerate() {
                        let index = call
                            .get("index")
                            .and_then(Value::as_i64)
                            .map(|v| v as usize)
                            .unwrap_or(fallback);
                        let call_id = call.get("id").and_then(Value::as_str);
                        let function = call.get("function");
                        out.push(StreamEvent::ToolCallDelta {
                            index,
                            key: call_id.map(str::to_string).unwrap_or_else(|| index.to_string()),
                            call_id: call_id.map(str::to_string),
                            name: function
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            arguments_fragment: function
                                .and_then(|f| f.get("arguments"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        });
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                if !reason.is_empty() {
                    out.push(StreamEvent::Finished(reason.to_string()));
                }
            }
            // 有的网关把 usage 放在 choice 上
            if let Some(usage) = choice.get("usage").filter(|v| !v.is_null()) {
                out.push(StreamEvent::Usage(usage.clone()));
            }
        }
        out
    }
}

fn openai_message(turn: &ChatTurn) -> Value {
    let mut message = Map::new();
    message.insert("role".into(), json!(turn.role));
    if turn.role == "tool" {
        message.insert("tool_call_id".into(), json!(turn.tool_call_id.clone().unwrap_or_default()));
        message.insert("content".into(), json!(turn.content));
        return Value::Object(message);
    }
    if !turn.tool_calls.is_empty() {
        // `content` 用空串（比 null 有更广的兼容性）
        message.insert("content".into(), json!(turn.content));
        message.insert(
            "tool_calls".into(),
            Value::Array(
                turn.tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        })
                    })
                    .collect(),
            ),
        );
        return Value::Object(message);
    }
    message.insert("content".into(), json!(turn.content));
    Value::Object(message)
}

fn openai_tools(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect(),
    )
}

/// 思考强度按方言落到不同字段（AIH-056）。
///
/// **不指定强度（界面上的「默认」）时分两种**：`openai` 方言什么都不发
/// （很多网关不认 `reasoning_effort: "none"`），而 `deepseek`/`zai`/`openrouter`
/// 方言会显式发"禁用" —— 它们的网关**默认开思考**。
///
/// ⚠ 这只是"我们能做的最好"，**网关未必认**（实测 command-code-goat 全忽略，照样思考），
/// 所以界面上这一档叫「默认」不叫「关闭」。
fn apply_openai_reasoning(root: &mut Map<String, Value>, r: &ReasoningRequest) {
    let format = r.format.unwrap_or(ThinkingFormat::OpenAi);
    if let (Some(_), Some(wire)) = (r.effort.as_ref(), r.wire_value.as_ref()) {
        match format {
            ThinkingFormat::DeepSeek => {
                root.insert("thinking".into(), json!({ "type": "enabled" }));
                root.insert("reasoning_effort".into(), json!(wire));
            }
            ThinkingFormat::Qwen => {
                root.insert("enable_thinking".into(), json!(true));
                root.insert("reasoning_effort".into(), json!(wire));
            }
            ThinkingFormat::Zai => {
                root.insert("thinking".into(), json!({ "type": "enabled", "clear_thinking": false }));
                root.insert("reasoning_effort".into(), json!(wire));
            }
            ThinkingFormat::OpenRouter => {
                root.insert("reasoning".into(), json!({ "effort": wire }));
            }
            ThinkingFormat::OpenAi => {
                root.insert("reasoning_effort".into(), json!(wire));
            }
        }
        return;
    }
    match format {
        ThinkingFormat::DeepSeek => {
            root.insert("thinking".into(), json!({ "type": "disabled" }));
        }
        ThinkingFormat::Zai => {
            root.insert("thinking".into(), json!({ "type": "disabled" }));
        }
        ThinkingFormat::OpenRouter => {
            root.insert("reasoning".into(), json!({ "effort": "none" }));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
//  openai-responses
// ---------------------------------------------------------------------------

/// OpenAI Responses（AIH-004）。
///
/// 形状与 completions 差得远：`input` 是一个**条目数组**（消息 / 函数调用 / 函数结果
/// 是三种不同的条目），系统提示走 `instructions`，工具是**扁平**的
/// `{type:"function", name, parameters}`（没有 `function` 那层）。
pub struct OpenAiResponsesAdapter;

impl Adapter for OpenAiResponsesAdapter {
    fn api(&self) -> Api {
        Api::OpenAiResponses
    }

    fn build_body(
        &self,
        model: &str,
        turns: &[ChatTurn],
        tools: &[ToolSpec],
        reasoning: &ReasoningRequest,
        max_output_tokens: Option<i64>,
    ) -> Value {
        let mut root = Map::new();
        root.insert("model".into(), json!(model));
        root.insert("stream".into(), json!(true));

        // system 轮走 `instructions`（Responses 没有 system 角色）
        let instructions: Vec<&str> = turns
            .iter()
            .filter(|t| t.role == "system" && !t.content.is_empty())
            .map(|t| t.content.as_str())
            .collect();
        if !instructions.is_empty() {
            root.insert("instructions".into(), json!(instructions.join("\n\n")));
        }

        let mut input: Vec<Value> = Vec::new();
        for turn in turns.iter().filter(|t| t.role != "system") {
            if turn.role == "tool" {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": turn.tool_call_id.clone().unwrap_or_default(),
                    "output": turn.content,
                }));
                continue;
            }
            if !turn.content.is_empty() {
                input.push(json!({
                    "type": "message",
                    "role": turn.role,
                    "content": [{ "type": "input_text", "text": turn.content }],
                }));
            }
            // 助手发起的工具调用要作为独立的 `function_call` 条目回传
            for call in &turn.tool_calls {
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments,
                }));
            }
        }
        root.insert("input".into(), Value::Array(input));

        if !tools.is_empty() {
            root.insert(
                "tools".into(),
                Value::Array(
                    tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "type": "function",
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            })
                        })
                        .collect(),
                ),
            );
        }
        if let Some(max) = max_output_tokens {
            root.insert("max_output_tokens".into(), json!(max));
        }
        if let (Some(_), Some(wire)) = (reasoning.effort.as_ref(), reasoning.wire_value.as_ref()) {
            root.insert("reasoning".into(), json!({ "effort": wire }));
        }
        Value::Object(root)
    }

    fn interpret(&self, frame: &Frame) -> Vec<StreamEvent> {
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Vec::new();
        }
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let kind = root.get("type").and_then(Value::as_str).unwrap_or_default();
        let mut out = Vec::new();

        // 响应 id：`response.created` / 每个事件的 `response.id`
        if let Some(id) = root
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
            .or_else(|| root.get("response_id").and_then(Value::as_str))
        {
            if !id.is_empty() {
                out.push(StreamEvent::ProviderId(id.to_string()));
            }
        }
        // usage 在 `response.completed` 里
        if let Some(usage) = root.get("response").and_then(|r| r.get("usage")).filter(|v| !v.is_null()) {
            out.push(StreamEvent::Usage(usage.clone()));
        }

        match kind {
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(text) = root.get("delta").and_then(Value::as_str) {
                    if !text.is_empty() {
                        out.push(StreamEvent::TextDelta(text.to_string()));
                    }
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(text) = root.get("delta").and_then(Value::as_str) {
                    if !text.is_empty() {
                        out.push(StreamEvent::ReasoningDelta(text.to_string()));
                    }
                }
            }
            // 函数调用参数分片
            "response.function_call_arguments.delta" => {
                let index = output_index(&root);
                out.push(StreamEvent::ToolCallDelta {
                    index,
                    key: root
                        .get("item_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| index.to_string()),
                    call_id: root.get("item_id").and_then(Value::as_str).map(str::to_string),
                    name: None,
                    arguments_fragment: root.get("delta").and_then(Value::as_str).map(str::to_string),
                });
            }
            // 函数调用**开始**：这里才有名字（参数随后分片来）
            "response.output_item.added" => {
                if let Some(item) = root.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        let index = output_index(&root);
                        out.push(StreamEvent::ToolCallDelta {
                            index,
                            key: item
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| index.to_string()),
                            call_id: item.get("call_id").and_then(Value::as_str).map(str::to_string),
                            name: item.get("name").and_then(Value::as_str).map(str::to_string),
                            arguments_fragment: item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string),
                        });
                    }
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                out.push(StreamEvent::Finished(
                    root.get("response")
                        .and_then(|r| r.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or(kind)
                        .to_string(),
                ));
            }
            _ => {}
        }
        out
    }
}

/// Responses 的 `output_index`（函数调用的定位键）。
fn output_index(root: &Value) -> usize {
    root.get("output_index")
        .and_then(Value::as_i64)
        .map(|v| v.max(0) as usize)
        .unwrap_or(0)
}

/// Anthropic 的定位键**叫 `index`**（不是 `output_index`）。
///
/// ⚠ 读错键的后果很具体：`content_block_start`（带名字，index=1）与
/// `content_block_delta`（只有参数分片，index=1）会一个落到 0、一个落到 1
/// —— 于是参数被拼到另一个调用上，看起来是"工具参数像 JSON 但张冠李戴"。
fn content_index(root: &Value) -> usize {
    root.get("index")
        .and_then(Value::as_i64)
        .map(|v| v.max(0) as usize)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
//  anthropic-messages
// ---------------------------------------------------------------------------

/// Anthropic Messages。
///
/// 形状要点（照抄 Kotlin）：系统提示是**顶层 `system` 字符串**（不是消息）；
/// `max_tokens` **必填**（没有默认值，缺了直接 400）；工具的参数 schema 叫
/// `input_schema`；内容块是 `content: [{type:"text"}]`；
/// 工具结果是**用户轮的 `tool_result` 块**（不是 `role:"tool"`）。
pub struct AnthropicMessagesAdapter;

impl Adapter for AnthropicMessagesAdapter {
    fn api(&self) -> Api {
        Api::AnthropicMessages
    }

    fn build_body(
        &self,
        model: &str,
        turns: &[ChatTurn],
        tools: &[ToolSpec],
        reasoning: &ReasoningRequest,
        max_output_tokens: Option<i64>,
    ) -> Value {
        let mut root = Map::new();
        root.insert("model".into(), json!(model));
        root.insert("stream".into(), json!(true));
        // `max_tokens` 是**必填**：给一个保守默认值，别让用户看到一个没有线索的 400
        root.insert("max_tokens".into(), json!(max_output_tokens.unwrap_or(4096)));

        let system = turns
            .iter()
            .filter(|t| t.role == "system" && !t.content.is_empty())
            .map(|t| t.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !system.is_empty() {
            root.insert("system".into(), json!(system));
        }

        let mut messages: Vec<Value> = Vec::new();
        for turn in turns.iter().filter(|t| t.role != "system") {
            match turn.role.as_str() {
                "tool" => {
                    // 工具结果是**用户轮**里的 `tool_result` 块
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": turn.tool_call_id.clone().unwrap_or_default(),
                            "content": turn.content,
                        }],
                    }));
                }
                role => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !turn.content.is_empty() {
                        blocks.push(json!({ "type": "text", "text": turn.content }));
                    }
                    for call in &turn.tool_calls {
                        // 参数在 Anthropic 这边是**对象**（completions 那边是字符串）
                        let input: Value = serde_json::from_str(&call.arguments)
                            .unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": input,
                        }));
                    }
                    if !blocks.is_empty() {
                        messages.push(json!({ "role": role, "content": blocks }));
                    }
                }
            }
        }
        root.insert("messages".into(), Value::Array(messages));

        if !tools.is_empty() {
            root.insert(
                "tools".into(),
                Value::Array(
                    tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "name": tool.name,
                                "description": tool.description,
                                "input_schema": tool.parameters,
                            })
                        })
                        .collect(),
                ),
            );
        }
        // 思考：Anthropic 要的是**预算 token 数**，不是等级名
        if let (Some(_), Some(budget)) = (reasoning.effort.as_ref(), reasoning.wire_value.as_ref()) {
            if let Ok(tokens) = budget.parse::<i64>() {
                root.insert("thinking".into(), json!({ "type": "enabled", "budget_tokens": tokens }));
            } else {
                // 等级名（low/high…）在 Anthropic 上表达不了 —— 按"最小可用预算"给，
                // 并**不**假装它等于用户选的那个等级（宁可少给，也不要给出一个假的精确值）
                root.insert("thinking".into(), json!({ "type": "enabled", "budget_tokens": 1024 }));
            }
        }
        Value::Object(root)
    }

    fn interpret(&self, frame: &Frame) -> Vec<StreamEvent> {
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Vec::new();
        }
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let kind = root.get("type").and_then(Value::as_str).unwrap_or_default();
        let mut out = Vec::new();

        if let Some(message) = root.get("message") {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                out.push(StreamEvent::ProviderId(id.to_string()));
            }
            if let Some(usage) = message.get("usage").filter(|v| !v.is_null()) {
                out.push(StreamEvent::Usage(usage.clone()));
            }
        }
        if let Some(usage) = root.get("usage").filter(|v| !v.is_null()) {
            out.push(StreamEvent::Usage(usage.clone()));
        }

        match kind {
            "content_block_delta" => {
                let index = content_index(&root);
                if let Some(delta) = root.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    out.push(StreamEvent::TextDelta(text.to_string()));
                                }
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    out.push(StreamEvent::ReasoningDelta(text.to_string()));
                                }
                            }
                        }
                        Some("input_json_delta") => {
                            out.push(StreamEvent::ToolCallDelta {
                                index,
                                key: index.to_string(),
                                call_id: None,
                                name: None,
                                arguments_fragment: delta
                                    .get("partial_json")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                            });
                        }
                        _ => {}
                    }
                }
            }
            // 工具调用块开始：这里才有 id 与 name
            "content_block_start" => {
                if let Some(block) = root.get("content_block") {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        out.push(StreamEvent::ToolCallDelta {
                            index: content_index(&root),
                            key: block
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| content_index(&root).to_string()),
                            call_id: block.get("id").and_then(Value::as_str).map(str::to_string),
                            name: block.get("name").and_then(Value::as_str).map(str::to_string),
                            arguments_fragment: None,
                        });
                    }
                }
            }
            "message_delta" => {
                if let Some(usage) = root.get("usage").filter(|v| !v.is_null()) {
                    out.push(StreamEvent::Usage(usage.clone()));
                }
                if let Some(reason) = root
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    out.push(StreamEvent::Finished(reason.to_string()));
                }
            }
            "message_stop" => out.push(StreamEvent::Finished("message_stop".into())),
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, content: &str) -> ChatTurn {
        ChatTurn { role: role.into(), content: content.into(), ..Default::default() }
    }

    /// SSE 分帧：多行 data 拼接、注释丢掉、`\r\n` 认、结尾无空行也能靠 flush 收尾。
    #[test]
    fn sse_framing_rules() {
        let mut acc = SseAccumulator::new();
        assert!(acc.line(": keep-alive").is_none(), "注释不算帧");
        assert!(acc.line("event: message").is_none());
        assert!(acc.line("data: {\"a\":").is_none());
        assert!(acc.line("data: 1}").is_none(), "**空行才是帧结束**：data 行本身不产生帧");
        let frame = acc.line("").unwrap();
        assert_eq!(frame.event.as_deref(), Some("message"));
        assert_eq!(frame.data, "{\"a\":\n1}", "多行 data 用换行拼接");
        assert!(acc.line("").is_none(), "空行之后没有内容 → 不产生第二帧");
        // `\r\n`
        let mut acc = SseAccumulator::new();
        assert!(acc.line("data: x\r").is_none());
        assert_eq!(acc.line("\r").unwrap().data, "x");
        // 只有一行的 data：`data:` 后面那个空格是分隔符，第二个空格是内容
        let mut acc = SseAccumulator::new();
        assert!(acc.line("data:  two").is_none());
        assert_eq!(acc.line("").unwrap().data, " two");
        // 流结束时还没收尾的一帧
        let mut acc = SseAccumulator::new();
        acc.line("data: tail");
        assert_eq!(acc.flush().unwrap().data, "tail");
        assert!(acc.flush().is_none(), "flush 之后不重复吐");
    }

    /// completions：纯文本请求体与 Kotlin 同形（`messages` 是字符串 content）。
    #[test]
    fn completions_body_is_minimal_for_text() {
        let adapter = OpenAiCompletionsAdapter;
        let body = adapter.build_body(
            "deepseek-v4",
            &[turn("system", "你是助手"), turn("user", "你好")],
            &[],
            &ReasoningRequest::default(),
            None,
        );
        assert_eq!(body["model"], "deepseek-v4");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][1]["content"], "你好", "纯文本轮用字符串，不是数组");
        assert!(body.get("tools").is_none(), "没工具就不下发 tools");
        assert!(body.get("reasoning_effort").is_none(), "不指定强度 → 一个思考参数都不发");
    }

    /// 工具轮：`role=tool` 带 `tool_call_id`；助手带 `tool_calls`。
    #[test]
    fn completions_tool_turns() {
        let adapter = OpenAiCompletionsAdapter;
        let assistant = ChatTurn {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: vec![ToolCallRef {
                id: "call_1".into(),
                name: "web_search".into(),
                arguments: "{\"query\":\"x\"}".into(),
            }],
            tool_call_id: None,
        };
        let result = ChatTurn {
            role: "tool".into(),
            content: "结果".into(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".into()),
        };
        let body = adapter.build_body(
            "m",
            &[turn("user", "查一下"), assistant, result],
            &[ToolSpec {
                name: "web_search".into(),
                description: "搜".into(),
                parameters: json!({"type": "object", "properties": {}}),
            }],
            &ReasoningRequest::default(),
            None,
        );
        assert_eq!(body["messages"][1]["tool_calls"][0]["function"]["name"], "web_search");
        assert_eq!(body["messages"][1]["content"], "", "助手工具轮的 content 是空串（兼容性）");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "web_search");
    }

    /// 思考强度按方言落到不同字段（AIH-056 的五个方言）。
    #[test]
    fn completions_reasoning_dialects() {
        let adapter = OpenAiCompletionsAdapter;
        let build = |format: ThinkingFormat, wire: Option<&str>| {
            adapter.build_body(
                "m",
                &[turn("user", "hi")],
                &[],
                &ReasoningRequest {
                    effort: wire.map(str::to_string),
                    wire_value: wire.map(str::to_string),
                    format: Some(format),
                },
                None,
            )
        };
        assert_eq!(build(ThinkingFormat::OpenAi, Some("high"))["reasoning_effort"], "high");
        let deepseek = build(ThinkingFormat::DeepSeek, Some("high"));
        assert_eq!(deepseek["thinking"]["type"], "enabled");
        assert_eq!(deepseek["reasoning_effort"], "high");
        let qwen = build(ThinkingFormat::Qwen, Some("low"));
        assert_eq!(qwen["enable_thinking"], true);
        let zai = build(ThinkingFormat::Zai, Some("max"));
        assert_eq!(zai["thinking"]["clear_thinking"], false);
        let or = build(ThinkingFormat::OpenRouter, Some("low"));
        assert_eq!(or["reasoning"]["effort"], "low");

        // **默认档（不指定）**：只有"默认会思考"的方言才值得试着显式关闭
        assert!(build(ThinkingFormat::OpenAi, None).get("reasoning_effort").is_none());
        assert_eq!(build(ThinkingFormat::DeepSeek, None)["thinking"]["type"], "disabled");
        assert_eq!(build(ThinkingFormat::Zai, None)["thinking"]["type"], "disabled");
        assert_eq!(build(ThinkingFormat::OpenRouter, None)["reasoning"]["effort"], "none");
        assert!(build(ThinkingFormat::Qwen, None).get("enable_thinking").is_none());
    }

    /// completions 的流事件：正文 / 思考 / usage / 工具分片 / finish_reason。
    #[test]
    fn completions_interpret() {
        let adapter = OpenAiCompletionsAdapter;
        let frame = |data: &str| Frame { event: None, data: data.to_string() };

        let events = adapter.interpret(&frame(
            r#"{"id":"r1","choices":[{"delta":{"content":"你"}}]}"#,
        ));
        assert!(events.contains(&StreamEvent::ProviderId("r1".into())));
        assert!(events.contains(&StreamEvent::TextDelta("你".into())));

        // 思考：两个方言字段都认，且只取一个
        let events = adapter.interpret(&frame(
            r#"{"choices":[{"delta":{"reasoning_content":"想一下","reasoning":"重复"}}]}"#,
        ));
        assert_eq!(events, vec![StreamEvent::ReasoningDelta("想一下".into())]);

        // usage 在根上
        let events = adapter.interpret(&frame(
            r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#,
        ));
        assert!(matches!(events[0], StreamEvent::Usage(_)));

        // 工具分片
        let events = adapter.interpret(&frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_a","function":{"name":"web_search","arguments":"{\"q\":"}}]}}]}"#,
        ));
        assert_eq!(
            events,
            vec![StreamEvent::ToolCallDelta {
                index: 1,
                key: "call_a".into(),
                call_id: Some("call_a".into()),
                name: Some("web_search".into()),
                arguments_fragment: Some("{\"q\":".into()),
            }]
        );

        // finish_reason + `[DONE]` + 坏 JSON
        assert_eq!(
            adapter.interpret(&frame(r#"{"choices":[{"finish_reason":"stop"}]}"#)),
            vec![StreamEvent::Finished("stop".into())]
        );
        assert!(adapter.interpret(&frame("[DONE]")).is_empty());
        assert!(adapter.interpret(&frame("not json")).is_empty(), "一条读不懂的帧不该让整轮失败");
        assert!(adapter.interpret(&frame("")).is_empty());

        // 有的网关在流里回完整 message（不是 delta）
        let events = adapter.interpret(&frame(r#"{"choices":[{"message":{"content":"完整"}}]}"#));
        assert_eq!(events, vec![StreamEvent::TextDelta("完整".into())]);
    }

    /// 工具调用分片拼装：**按 index 归位**，不按到达顺序；缺 id 自己编一个。
    #[test]
    fn tool_call_accumulator_joins_by_index() {
        let mut acc = ToolCallAccumulator::new();
        // 先到 index=1 的名字，再到 index=0 的 —— 顺序是乱的
        acc.push(1, "b", Some("call_b"), Some("web_fetch"), None);
        acc.push(0, "a", Some("call_a"), Some("web_search"), Some("{\"q\":"));
        acc.push(1, "b", None, None, Some("{\"url\":"));
        acc.push(0, "a", None, None, Some("\"x\"}"));
        acc.push(1, "b", None, None, Some("\"y\"}"));
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "call_a");
        assert_eq!(calls[0].1.name, "web_search");
        assert_eq!(calls[0].1.arguments, "{\"q\":\"x\"}", "参数按 index 归位，不许串线");
        assert_eq!(calls[1].1.name, "web_fetch");
        assert_eq!(calls[1].1.arguments, "{\"url\":\"y\"}");

        // 缺 id：自己编一个（回传工具结果时必须有）
        let mut acc = ToolCallAccumulator::new();
        acc.push(2, "2", None, Some("read_file"), Some("{}"));
        let calls = acc.finish();
        assert_eq!(calls[0].0, "call_2");
        // 只有一个空壳不算"有工具调用"
        let mut acc = ToolCallAccumulator::new();
        acc.push(0, "0", None, None, Some("{}"));
        assert!(acc.is_empty());
    }

    /// responses：系统提示走 `instructions`、工具结果走 `function_call_output`。
    #[test]
    fn responses_body_shape() {
        let adapter = OpenAiResponsesAdapter;
        let body = adapter.build_body(
            "m",
            &[
                turn("system", "规则"),
                turn("user", "你好"),
                ChatTurn {
                    role: "assistant".into(),
                    content: String::new(),
                    tool_calls: vec![ToolCallRef {
                        id: "call_1".into(),
                        name: "web_search".into(),
                        arguments: "{}".into(),
                    }],
                    tool_call_id: None,
                },
                ChatTurn {
                    role: "tool".into(),
                    content: "结果".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("call_1".into()),
                },
            ],
            &[ToolSpec {
                name: "web_search".into(),
                description: "搜".into(),
                parameters: json!({"type": "object"}),
            }],
            &ReasoningRequest::default(),
            None,
        );
        assert_eq!(body["instructions"], "规则", "system 轮走 instructions（没有 system 角色）");
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["output"], "结果");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "web_search", "responses 的工具是扁平的");
        assert!(body["input"].as_array().unwrap().iter().all(|i| i["role"] != "system"));
    }

    /// responses 的流事件：文本 / 思考 / 函数调用开始与参数分片 / 收尾。
    #[test]
    fn responses_interpret() {
        let adapter = OpenAiResponsesAdapter;
        let frame = |data: &str| Frame { event: None, data: data.to_string() };
        assert_eq!(
            adapter.interpret(&frame(r#"{"type":"response.output_text.delta","delta":"甲"}"#)),
            vec![StreamEvent::TextDelta("甲".into())]
        );
        assert_eq!(
            adapter.interpret(&frame(
                r#"{"type":"response.reasoning_summary_text.delta","delta":"想"}"#
            )),
            vec![StreamEvent::ReasoningDelta("想".into())]
        );
        let events = adapter.interpret(&frame(
            r#"{"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"item_1","call_id":"call_1","name":"web_search"}}"#,
        ));
        assert_eq!(
            events,
            vec![StreamEvent::ToolCallDelta {
                index: 2,
                key: "item_1".into(),
                call_id: Some("call_1".into()),
                name: Some("web_search".into()),
                arguments_fragment: None,
            }]
        );
        let events = adapter.interpret(&frame(
            r#"{"type":"response.function_call_arguments.delta","output_index":2,"item_id":"item_1","delta":"{\"q\""}"#,
        ));
        assert_eq!(
            events,
            vec![StreamEvent::ToolCallDelta {
                index: 2,
                key: "item_1".into(),
                call_id: Some("item_1".into()),
                name: None,
                arguments_fragment: Some("{\"q\"".into()),
            }]
        );
        let events = adapter.interpret(&frame(
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":1}}}"#,
        ));
        assert!(events.contains(&StreamEvent::ProviderId("resp_1".into())));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(_))));
        assert!(events.contains(&StreamEvent::Finished("completed".into())));
    }

    /// anthropic：`max_tokens` 必填、system 在顶层、工具结果是用户轮的块。
    #[test]
    fn anthropic_body_shape() {
        let adapter = AnthropicMessagesAdapter;
        let body = adapter.build_body(
            "claude",
            &[
                turn("system", "规则"),
                turn("user", "你好"),
                ChatTurn {
                    role: "tool".into(),
                    content: "结果".into(),
                    tool_calls: vec![],
                    tool_call_id: Some("toolu_1".into()),
                },
            ],
            &[ToolSpec {
                name: "web_search".into(),
                description: "搜".into(),
                parameters: json!({"type": "object"}),
            }],
            &ReasoningRequest::default(),
            None,
        );
        assert_eq!(body["system"], "规则");
        assert_eq!(body["max_tokens"], 4096, "Anthropic 的 max_tokens 必填，缺了直接 400");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object", "Anthropic 叫 input_schema");

        // 工具调用的参数在 Anthropic 是**对象**（completions 那边是字符串）
        let with_call = adapter.build_body(
            "claude",
            &[ChatTurn {
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![ToolCallRef {
                    id: "toolu_2".into(),
                    name: "web_search".into(),
                    arguments: "{\"q\":\"x\"}".into(),
                }],
                tool_call_id: None,
            }],
            &[],
            &ReasoningRequest::default(),
            None,
        );
        assert_eq!(with_call["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(with_call["messages"][0]["content"][0]["input"]["q"], "x");
        assert!(with_call["messages"][0]["content"][0]["input"].is_object());

        // 思考强度在 Anthropic 是**预算 token**（等级名表达不了 → 给最小可用预算，不假装）
        let budget = adapter.build_body(
            "claude",
            &[turn("user", "hi")],
            &[],
            &ReasoningRequest {
                effort: Some("max".into()),
                wire_value: Some("4096".into()),
                format: None,
            },
            None,
        );
        assert_eq!(budget["thinking"]["budget_tokens"], 4096);
        let named = adapter.build_body(
            "claude",
            &[turn("user", "hi")],
            &[],
            &ReasoningRequest {
                effort: Some("high".into()),
                wire_value: Some("high".into()),
                format: None,
            },
            None,
        );
        assert_eq!(named["thinking"]["budget_tokens"], 1024);
    }

    /// anthropic 的流事件：工具块开始 / 参数分片 / stop_reason。
    #[test]
    fn anthropic_interpret() {
        let adapter = AnthropicMessagesAdapter;
        let frame = |data: &str| Frame { event: None, data: data.to_string() };
        let events = adapter.interpret(&frame(
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":5}}}"#,
        ));
        assert!(events.contains(&StreamEvent::ProviderId("msg_1".into())));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(_))));

        let events = adapter.interpret(&frame(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"web_search"}}"#,
        ));
        assert_eq!(
            events,
            vec![StreamEvent::ToolCallDelta {
                index: 1,
                key: "toolu_1".into(),
                call_id: Some("toolu_1".into()),
                name: Some("web_search".into()),
                arguments_fragment: None,
            }]
        );
        let events = adapter.interpret(&frame(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\""}}"#,
        ));
        assert_eq!(
            events,
            vec![StreamEvent::ToolCallDelta {
                index: 1,
                key: "1".into(),
                call_id: None,
                name: None,
                arguments_fragment: Some("{\"q\"".into()),
            }],
            "分片的 index 必须与 content_block_start 的 index 对齐，否则参数会拼到别的调用上"
        );
        assert_eq!(
            adapter.interpret(&frame(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"嗯"}}"#
            )),
            vec![StreamEvent::ReasoningDelta("嗯".into())]
        );
        assert_eq!(
            adapter.interpret(&frame(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#
            )),
            vec![StreamEvent::Finished("end_turn".into())]
        );
        assert_eq!(
            adapter.interpret(&frame(r#"{"type":"message_stop"}"#)),
            vec![StreamEvent::Finished("message_stop".into())]
        );
    }

    /// 对话端点：三家对 `/v1` 的处理**刻意不一样**（照抄 Kotlin，别顺手统一）。
    #[test]
    fn chat_urls_match_kotlin() {
        assert_eq!(
            Api::OpenAiCompletions.chat_url("https://api.deepseek.com"),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            Api::OpenAiCompletions.chat_url("https://x/v1/"),
            "https://x/v1/chat/completions"
        );
        assert_eq!(
            Api::OpenAiResponses.chat_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/responses"
        );
        assert_eq!(
            Api::OpenAiResponses.chat_url("https://x/v1"),
            "https://x/v1/responses"
        );
        assert_eq!(
            Api::AnthropicMessages.chat_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(Api::parse("openai-completions"), Some(Api::OpenAiCompletions));
        assert_eq!(Api::parse("  grok  "), None);
        assert_eq!(Api::AnthropicMessages.wire(), "anthropic-messages");
    }
}
