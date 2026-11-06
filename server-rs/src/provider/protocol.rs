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
//! 4. **思考有时必须回传**：`openai-responses`（DeepSeek 那家是**无状态**的）在思考模式下
//!    要求把历史轮次的思维链原文放回 `input`（`reasoning` 项）——
//!    少一个就是 `400 The reasoning_text in the thinking mode must be passed back to the API.`
//!    所以 [`ChatTurn::reasoning`] 是**逐协议**用的：responses 发、另两家一个字节都不发。

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

    /// 线上的拼写（Kotlin `ThinkingFormat.wire`）—— **保存模型目录时给用户看的那串**
    /// （"未知的思考方言: x（可选 openai / deepseek / qwen / openrouter / zai）"）。
    pub fn wire(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::Qwen => "qwen",
            Self::OpenRouter => "openrouter",
            Self::Zai => "zai",
        }
    }

    /// 全部方言（顺序与 Kotlin 的 `entries` 一致，**文案里的那一串就是它**）。
    pub const ALL: [ThinkingFormat; 5] = [
        ThinkingFormat::OpenAi,
        ThinkingFormat::DeepSeek,
        ThinkingFormat::Qwen,
        ThinkingFormat::OpenRouter,
        ThinkingFormat::Zai,
    ];
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
    /// **每一轮助手消息都回传 `reasoning` 项（空的也发）**。
    ///
    /// 和 [`format`] 是两个轴：`format` 说的是"用哪种方言表达强度"，
    /// 这个开关说的是"要不要把思维链回传给上游"。
    ///
    /// 为什么要有它：有些模型**某些轮次根本不思考**（实测 2026-10-25：只发一个工具调用、
    /// 2.3 秒返回、`reasoning_tokens` 为 0），而思考模式下带 tools 的 Responses 请求
    /// 要求逐轮对得上。补 `content: []` 是唯一诚实的形状（编一段假的等于把别人的思考
    /// 塞进它的历史）。以前这个判断绑在 `format == DeepSeek` 上 —— 模型没声明方言时
    /// 就漏发，于是一路 400，最后只能把整个 Run 的思考关掉。现在改成"被拒过一次就置位"。
    pub echo_all: bool,
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
    /// **已经决定内联**的图片附件（M3 / R9）。
    ///
    /// ⚠ 只放**真的发得出去**的那些：判断（模态 / 传输方式 / 内联预算）在
    /// [`crate::ai::executor`] 那一层做完，没通过的不在这里、而是变成正文里的一句
    /// "未随本次请求发送"（AIH-030：不许静默丢掉）。
    pub images: Vec<ChatImage>,
    /// 这一轮助手的**思维链原文**（只有助手轮会有）。
    ///
    /// 起源是一次真实的 400（DeepSeek Responses API）：
    /// `The reasoning_text in the thinking mode must be passed back to the API.`
    /// —— 那家 API 是**无状态**的，思考模式下"带 `tools` 的请求"要求后续每一次请求都把
    /// 历史轮次的思维链**完整回传**，否则直接 400（[DeepSeek Responses API 指南]：
    /// 输入 item 里的 `reasoning` 明文 `content` 会被归并进相邻的 assistant 消息）。
    ///
    /// ⚠ **只有需要回传的协议才发它**：`openai-responses` 的适配器把它铺成 `reasoning` 项；
    /// completions / anthropic 两个适配器**一个字节都不发**（它们的方言要不要回传是另一件事，
    /// 没实测过就不许顺手加）。所以"历史里的思考进上下文"这件事是**逐协议**的。
    ///
    /// [DeepSeek Responses API 指南]: https://api-docs.deepseek.com/zh-cn/guides/responses_api
    pub reasoning: Option<String>,
}

/// 一张内联进请求体的图片（适配器只管按各家的形状铺开）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatImage {
    pub mime_type: String,
    /// 原始字节的 base64（**已经编码好**：适配器不做 IO）
    pub base64: String,
    /// 原始文件名（Anthropic 不写它，OpenAI 也不写；留着是为了排错与日志）
    pub file_name: String,
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
        self.drafts
            .values()
            .all(|d| d.name.is_none() && d.call_id.is_none())
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
                    ToolCallRef {
                        id,
                        name,
                        arguments: draft.arguments,
                    },
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
                            key: call_id
                                .map(str::to_string)
                                .unwrap_or_else(|| index.to_string()),
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
        message.insert(
            "tool_call_id".into(),
            json!(turn.tool_call_id.clone().unwrap_or_default()),
        );
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
    // 带图片时 `content` 必须变成**有序块数组**（纯文本轮保持最朴素的字符串，兼容性最好）
    if !turn.images.is_empty() {
        let mut blocks: Vec<Value> = Vec::new();
        if !turn.content.is_empty() {
            blocks.push(json!({ "type": "text", "text": turn.content }));
        }
        for image in &turn.images {
            blocks.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", image.mime_type, image.base64) },
            }));
        }
        message.insert("content".into(), Value::Array(blocks));
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
                root.insert(
                    "thinking".into(),
                    json!({ "type": "enabled", "clear_thinking": false }),
                );
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

/// 这一份请求会不会被要求"逐轮回传思维链"（DeepSeek 的 Responses）。
///
/// 判据只有两条，都是实测出来的：① 模型目录里声明的思考方言是 `deepseek`；
/// ② 思考**没被关掉** —— `effort=none` 那一档上游不校验回传（2026-10-25 的对照实验）。
///
/// ⚠ 别的方言没有实测过，不许顺手加：空 `content` 的 `reasoning` 项对 OpenAI 官方
/// 是不合法条目（它要 `id` / 加密内容），多塞一个就把本来能跑的请求变成 400。
fn reasoning_echo_required(reasoning: &ReasoningRequest) -> bool {
    // `effort: none` 时不校验回传（实测：同一个请求体 200），补了反而可能被拒
    if matches!(reasoning.effort.as_deref(), Some("none")) {
        return false;
    }
    reasoning.echo_all || matches!(reasoning.format, Some(ThinkingFormat::DeepSeek))
}

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
            // ⚠ 思考项必须排在**同一条助手轮的其余条目之前**（DeepSeek 的输出顺序就是
            // `reasoning` → `message` → `function_call`，回传时按同一顺序才不会被当成
            // "另一个 assistant 轮"）。少了它，思考模式下带 tools 的后续请求会 400：
            // "The reasoning_text in the thinking mode must be passed back to the API."
            //
            // 正文空、只有工具调用那一轮也要发（模型想完直接调工具是常态）—— 它归并进的是
            // **相邻的**助手轮，而下面紧跟的正是那一轮的 `function_call`。
            if let Some(reasoning) = turn.reasoning.as_deref().filter(|r| !r.trim().is_empty()) {
                input.push(json!({
                    "type": "reasoning",
                    "content": [{ "type": "reasoning_text", "text": reasoning }],
                }));
            } else if !turn.tool_calls.is_empty() && reasoning_echo_required(reasoning) {
                // **这一轮模型根本没思考**（实测 2026-10-25：`krea2 r2i` 那次的 16 轮里，
                // 有一轮只发了一个工具调用、2.3 秒就回来了，思维链 0 字符）。
                //
                // 思考模式下带 `tools` 的请求要求"每个工具轮都能对上思维链"，少一个就是
                // `400 The reasoning_text in the thinking mode must be passed back to the API.`
                // —— 而这一轮**没有思维链可回传**，编一段假的等于把别人的思考塞进它的历史。
                //
                // 所以补一个**空 `content` 的 reasoning 项**：这一轮确实没有思考文本（诚实），
                // 而空 content 正是那家明确接受的形状（被拒的是"空白 reasoning_text"，
                // 见 `docs/pitfalls.md` 里那行 2026-10-25 的实测）。
                input.push(json!({ "type": "reasoning", "content": [] }));
            }
            if !turn.content.is_empty() || !turn.images.is_empty() {
                let mut content: Vec<Value> = Vec::new();
                if !turn.content.is_empty() {
                    content.push(json!({ "type": "input_text", "text": turn.content }));
                }
                for image in &turn.images {
                    content.push(json!({
                        "type": "input_image",
                        "image_url": format!("data:{};base64,{}", image.mime_type, image.base64),
                    }));
                }
                input.push(json!({
                    "type": "message",
                    "role": turn.role,
                    "content": content,
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
        if let Some(usage) = root
            .get("response")
            .and_then(|r| r.get("usage"))
            .filter(|v| !v.is_null())
        {
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
                    call_id: root
                        .get("item_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    name: None,
                    arguments_fragment: root
                        .get("delta")
                        .and_then(Value::as_str)
                        .map(str::to_string),
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
                            call_id: item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .map(str::to_string),
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

/// 打一个 Anthropic 的显式缓存断点（`cache_control: {"type":"ephemeral"}`）。
///
/// 幂等且无副作用：目标不是对象（理论上不会发生）就原样不动 ——
/// **宁可少一个断点，也不要为它把请求体拼坏**（那会变成一个 400）。
fn mark_for_cache(block: &mut Value) {
    if let Some(map) = block.as_object_mut() {
        map.insert("cache_control".into(), json!({ "type": "ephemeral" }));
    }
}

/// 把断点打在**最后一条消息的最后一个内容块**上（"滚动断点"，见
/// [`AnthropicMessagesAdapter`] 的类型文档）。
///
/// 空消息 / 没有内容块时什么都不做：Anthropic 对空 `content` 数组直接 400，
/// 这里不去替调用方造一个块出来。
fn mark_last_block_for_cache(messages: Option<&mut Value>) {
    let Some(Value::Array(items)) = messages else {
        return;
    };
    let Some(last_message) = items.last_mut() else {
        return;
    };
    let Some(blocks) = last_message
        .get_mut("content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Some(last_block) = blocks.last_mut() {
        mark_for_cache(last_block);
    }
}

/// Anthropic Messages。
///
/// 形状要点（照抄 Kotlin）：系统提示是**顶层 `system` 字符串**（不是消息）；
/// `max_tokens` **必填**（没有默认值，缺了直接 400）；工具的参数 schema 叫
/// `input_schema`；内容块是 `content: [{type:"text"}]`；
/// 工具结果是**用户轮的 `tool_result` 块**（不是 `role:"tool"`）。
///
/// # 显式缓存断点（2026-10，只加在 Anthropic 这一家）
///
/// Anthropic 的前缀缓存是**显式**的：不发 `cache_control` 就一次都不命中 ——
/// 而工具往返循环每一轮都把"系统提示 + 整段历史"重发一遍，那是这个仓库里最贵的一笔输入。
/// 另外两家（OpenAI 系）是**自动**的，什么都不用发。
///
/// 断点打在**两处**（上限 4 个，这里刻意只用 2 个）：
///
/// 1. **最后一个工具**：把 `tools[]` 整段钉进缓存（它们每个 Run 都逐字节相同）；
/// 2. **最后一条消息的最后一个内容块**：这是"滚动断点"—— 下一轮的上下文是这一轮的
///    **超集**，前缀逐字节相同，所以整段（工具 + 系统提示 + 历史）直接命中，
///    只有新增的那几条按 1.25× 写一次。
///
/// ⚠ **不动 `system` 的形状**（它本可以写成带断点的块数组，那样能多一个断点）：
/// 从字符串改成数组是**线上的形状变更**，而第三方 anthropic 兼容网关未必都收；
/// 而"最后一个工具 + 滚动断点"这两个已经把系统提示含在缓存前缀里了 ——
/// 用一个兼容性风险去换一个多余的断点，不值得。
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
        root.insert(
            "max_tokens".into(),
            json!(max_output_tokens.unwrap_or(4096)),
        );

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
                    // 图片块：Anthropic 用 `source.media_type` + base64（**不用** data URL）
                    for image in &turn.images {
                        blocks.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.mime_type,
                                "data": image.base64,
                            },
                        }));
                    }
                    for call in &turn.tool_calls {
                        // 参数在 Anthropic 这边是**对象**（completions 那边是字符串）
                        let input: Value =
                            serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
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
        // 滚动缓存断点：打在**最后一条消息的最后一个内容块**上（见类型文档）。
        // 下一轮的上下文是这一轮的超集，所以整段（工具 + 系统提示 + 历史）直接命中。
        mark_last_block_for_cache(root.get_mut("messages"));

        if !tools.is_empty() {
            let mut specs: Vec<Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    })
                })
                .collect();
            // 工具定义也逐字节稳定（`catalog_with` 按 name 升序），整段钉进缓存
            if let Some(last) = specs.last_mut() {
                mark_for_cache(last);
            }
            root.insert("tools".into(), Value::Array(specs));
        }
        // 思考：Anthropic 要的是**预算 token 数**，不是等级名
        if let (Some(_), Some(budget)) = (reasoning.effort.as_ref(), reasoning.wire_value.as_ref())
        {
            if let Ok(tokens) = budget.parse::<i64>() {
                root.insert(
                    "thinking".into(),
                    json!({ "type": "enabled", "budget_tokens": tokens }),
                );
            } else {
                // 等级名（low/high…）在 Anthropic 上表达不了 —— 按"最小可用预算"给，
                // 并**不**假装它等于用户选的那个等级（宁可少给，也不要给出一个假的精确值）
                root.insert(
                    "thinking".into(),
                    json!({ "type": "enabled", "budget_tokens": 1024 }),
                );
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
                            name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::to_string),
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
mod tests;
