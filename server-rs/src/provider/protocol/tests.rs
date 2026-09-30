//! `protocol` 的单元测试（主体代码在 [`protocol`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn turn(role: &str, content: &str) -> ChatTurn {
    ChatTurn { role: role.into(), content: content.into(), ..Default::default() }
}

/// **附件协议的形状**（Kotlin `AttachmentProtocolTest` 的同一条纪律）：
/// 加了新模态 / 新传输方式，就要有一条"发出去的 JSON 到底长什么样"的用例 ——
/// 光断言"没报错"抓不到"'image_url' 写成 'image'"这种静默错（上游会当普通文本）。
#[test]
fn image_blocks_shape_per_adapter() {
    let image = ChatImage {
        mime_type: "image/png".into(),
        base64: "AAA=".into(),
        file_name: "参考图.png".into(),
    };
    let turns = vec![
        turn("system", "规则"),
        ChatTurn { role: "user".into(), content: "看看".into(), images: vec![image.clone()], ..Default::default() },
    ];
    let reasoning = ReasoningRequest::default();

    // ① OpenAI 兼容：`content` 变成有序块数组，图片是 `image_url.url` 的 data URL
    let openai = OpenAiCompletionsAdapter.build_body("m", &turns, &[], &reasoning, None);
    let blocks = openai["messages"][1]["content"].as_array().expect("要变成数组");
    assert_eq!(blocks[0], json!({ "type": "text", "text": "看看" }));
    assert_eq!(blocks[1]["type"], json!("image_url"));
    assert_eq!(blocks[1]["image_url"]["url"], json!("data:image/png;base64,AAA="));
    // 纯文本轮仍然是字符串（兼容性最好）
    let plain = OpenAiCompletionsAdapter.build_body("m", &[turn("user", "只有文字")], &[], &reasoning, None);
    assert!(plain["messages"][0]["content"].is_string());

    // ② Responses：`input[].content[]` 里是 `input_text` + `input_image`
    let responses = OpenAiResponsesAdapter.build_body("m", &turns, &[], &reasoning, None);
    let content = responses["input"][0]["content"].as_array().expect("content 数组");
    assert_eq!(content[0]["type"], json!("input_text"));
    assert_eq!(content[1]["type"], json!("input_image"));
    assert_eq!(content[1]["image_url"], json!("data:image/png;base64,AAA="));

    // ③ Anthropic：`content[]` 里的 `image` 块用 `source.media_type` + base64（**不是** data URL）
    let anthropic = AnthropicMessagesAdapter.build_body("m", &turns, &[], &reasoning, None);
    let blocks = anthropic["messages"][0]["content"].as_array().expect("content 数组");
    assert_eq!(blocks[0]["type"], json!("text"));
    assert_eq!(blocks[1]["type"], json!("image"));
    assert_eq!(blocks[1]["source"]["type"], json!("base64"));
    assert_eq!(blocks[1]["source"]["media_type"], json!("image/png"));
    assert_eq!(blocks[1]["source"]["data"], json!("AAA="));
    assert!(
        !anthropic.to_string().contains("data:image/png"),
        "Anthropic 不吃 data URL，必须是 source.base64"
    );
}

/// 只有图片、没有正文的一轮也要发出去（`content` 空串 + 图片块）。
#[test]
fn image_only_turn_is_still_sent() {
    let turns = vec![ChatTurn {
        role: "user".into(),
        content: String::new(),
        images: vec![ChatImage { mime_type: "image/webp".into(), base64: "BBB=".into(), file_name: "a.webp".into() }],
        ..Default::default()
    }];
    let body = OpenAiCompletionsAdapter.build_body("m", &turns, &[], &ReasoningRequest::default(), None);
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "没有正文时就只有图片块：{body}");
    assert_eq!(blocks[0]["type"], json!("image_url"));

    let anthropic = AnthropicMessagesAdapter.build_body("m", &turns, &[], &ReasoningRequest::default(), None);
    // Anthropic 那边照样是**一条消息**（blocks 非空就发）
    assert_eq!(anthropic["messages"][0]["content"][0]["type"], json!("image"));
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
        images: Vec::new(),
        // 这一轮也想过了：completions 适配器**一个字节都不发**（见 ChatTurn::reasoning）
        reasoning: Some("先想一下".into()),
    };
    let result = ChatTurn {
        role: "tool".into(),
        content: "结果".into(),
        tool_calls: vec![],
        tool_call_id: Some("call_1".into()),
        images: Vec::new(),
        reasoning: None,
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
    assert!(
        !body.to_string().contains("先想一下"),
        "completions **不回传思考**（要回传的是 responses，见 responses_reasoning_round_trip）"
    );
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
                images: Vec::new(),
                reasoning: None,
            },
            ChatTurn {
                role: "tool".into(),
                content: "结果".into(),
                tool_calls: vec![],
                tool_call_id: Some("call_1".into()),
                images: Vec::new(),
                reasoning: None,
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

/// **思考必须回传**（真实的 400 现场，DeepSeek Responses API）：
/// `The reasoning_text in the thinking mode must be passed back to the API.`
///
/// 那家 API 是**无状态**的，思考模式下"带 `tools` 的请求"要求后续每一次请求都把历史轮次的
/// 思维链按轮回传。形状与顺序两条都盯着（顺序错了会被当成"另一个 assistant 轮"）：
/// `reasoning` 项排在**同一条助手轮**的 `message` / `function_call` 之前，
/// 内容块是 `reasoning_text`（**不是** OpenAI 的 `summary_text` —— DeepSeek 不支持 summary）。
#[test]
fn responses_reasoning_round_trip() {
    let adapter = OpenAiResponsesAdapter;
    let body = adapter.build_body(
        "deepseek-flash",
        &[
            turn("user", "杭州明天天气"),
            ChatTurn {
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![ToolCallRef {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: "{}".into(),
                }],
                reasoning: Some("先查一下天气工具".into()),
                ..Default::default()
            },
            ChatTurn {
                role: "tool".into(),
                content: "多云".into(),
                tool_call_id: Some("call_1".into()),
                ..Default::default()
            },
        ],
        &[],
        &ReasoningRequest::default(),
        None,
    );
    let input = body["input"].as_array().expect("input 数组");
    assert_eq!(input[1]["type"], "reasoning", "思考项要排在 assistant 轮的最前面");
    assert_eq!(input[1]["content"][0]["type"], "reasoning_text");
    assert_eq!(input[1]["content"][0]["text"], "先查一下天气工具");
    assert_eq!(input[2]["type"], "function_call", "思考项后面紧跟同一轮的 function_call");
    assert_eq!(input[3]["type"], "function_call_output");

    // 有正文的那一轮：`reasoning` → `message`（DeepSeek 的输出顺序就是这样回传的）
    let with_text = adapter.build_body(
        "deepseek-flash",
        &[ChatTurn {
            role: "assistant".into(),
            content: "答案是 42".into(),
            reasoning: Some("算一算".into()),
            ..Default::default()
        }],
        &[],
        &ReasoningRequest::default(),
        None,
    );
    assert_eq!(with_text["input"][0]["type"], "reasoning");
    assert_eq!(with_text["input"][1]["type"], "message");
    assert_eq!(with_text["input"][1]["content"][0]["type"], "input_text");

    // 没有思考的那一轮：一个 `reasoning` 项都不许凭空冒出来
    let plain = adapter.build_body(
        "deepseek-flash",
        &[turn("assistant", "随口一句")],
        &[],
        &ReasoningRequest::default(),
        None,
    );
    assert_eq!(plain["input"].as_array().unwrap().len(), 1);
    assert_eq!(plain["input"][0]["type"], "message");
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
                images: Vec::new(),
                reasoning: None,
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
            images: Vec::new(),
            reasoning: None,
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
