//! `executor` 的单元测试（主体代码在 [`executor`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 布局模板随工具结果落进块里_否则重开就没了() {
    let mut sink = Sink::default();
    sink.push_text("提示词写好了");
    let outcome = CallOutcome {
        content: "已生成 1 个 2D 布局模板".into(),
        preview: "已生成 1 个 2D 布局模板".into(),
        ok: true,
        code: None,
        approval: "not_required".into(),
        result: serde_json::json!({
            "layoutTemplates": [{
                "title": "三分法",
                "note": "主体压左",
                "spec": { "width": 768, "height": 1024, "regions": [] }
            }],
            "issues": []
        }),
        elapsed_ms: 3,
        cached: false,
    };
    sink.push_tool_result("call-1", "layout_propose", &outcome);

    let parts = parts_of(&sink);
    let raw = parts
        .iter()
        .find(|(kind, ..)| kind == "tool_result")
        .and_then(|(_, _, _, payload)| payload.clone())
        .expect("tool_result 必须带 payload");
    let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // ⚠ 漏掉这一条的表现是：当轮卡片在、**重开 App 之后整张卡消失**（payload 白名单只放行 mediaIds / asked）
    assert_eq!(
        payload["layoutTemplates"][0]["title"], "三分法",
        "布局规格必须落进 tool_result 块里：{payload}"
    );
}

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

/// 历史 → 上游轮次：思考段**不进正文**，但**要单独挂在那一轮上**（`ChatTurn::reasoning`）。
///
/// 起源是一次真实的 400（DeepSeek Responses）：思考模式下带 `tools` 的后续请求必须把
/// 历史思维链回传（`The reasoning_text in the thinking mode must be passed back to the API.`）。
/// 两条都要守住：**不许混进正文**（模型会把自己的碎片当成说过的话），
/// 也**不许丢**（丢了就是那条 400）。
#[test]
fn history_keeps_reasoning_apart_from_the_text() {
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
    let turns = turns_of(&m, &AttachmentPlan::empty());
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].content, "答案");
    assert!(!turns[0].content.contains("我一直在想"), "思考不许混进正文");
    assert_eq!(
        turns[0].reasoning.as_deref(),
        Some("我一直在想"),
        "思考要挂在同一轮上回传"
    );
    // 没有思考块的那一轮：`None`（而不是空串 —— 适配器据此判断"发不发"）
    let plain = turns_of(
        &message("assistant", vec![text_part("只有正文")]),
        &AttachmentPlan::empty(),
    );
    assert_eq!(plain[0].reasoning, None);
}

/// 每轮思考**只归属一轮**：整条回复的思考（`reasoning`，落成块给界面看）与
/// **本轮**的思考（`seal_reasoning()`，回传给上游）是两份 —— 少了后者，
/// 同一个工具往返里模型想了什么就没人发给上游（那条 400 的半个成因）；
/// 而忘了取走，同一段思考会被挂到后面每一轮的助手轮上（重复回传）。
#[test]
fn sink_seals_reasoning_per_round() {
    let mut sink = Sink::default();
    sink.push_reasoning("第一轮想");
    assert_eq!(sink.seal_reasoning(), "第一轮想");
    assert_eq!(
        sink.seal_reasoning(),
        "",
        "取走之后本轮必须是空的（不许重复归属）"
    );
    sink.push_reasoning("第二轮想");
    assert_eq!(sink.seal_reasoning(), "第二轮想");
    assert_eq!(sink.reasoning, "第一轮想第二轮想", "整条回复的思考照样完整");
    assert!(
        parts_of(&sink)
            .iter()
            .any(|(kind, text, _, _)| kind == "reasoning" && text.contains("第二轮想")),
        "落库的 reasoning 块要在（界面靠它显示思考）"
    );
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
    let turns = turns_of(&m, &AttachmentPlan::empty());
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
    let turns = turns_of(&m, &AttachmentPlan::empty());
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
    let turns = turns_of(&m, &AttachmentPlan::empty());
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].content, "你好");
    // 完全空的（既没有块也没有正文）→ 不产生空轮
    let mut empty = message("user", Vec::new());
    empty.text = "   ".into();
    assert!(turns_of(&empty, &AttachmentPlan::empty()).is_empty());
}

/// 请求消息中的卡片 mention：`modelText` 必须按序拼回正文，模型才不会丢失卡片。
#[test]
fn mention_parts_contribute_their_model_text() {
    let m = message(
        "user",
        vec![
            crate::ai::conversations::AiMessagePartDto {
                r#type: "mention".into(),
                text: None,
                attachment_id: None,
                tool_call_id: None,
                json_payload: Some(json!({
                    "kind": "character_card",
                    "name": "凛",
                    "modelText": "[CHARACTER CARD]\n凛\n银发",
                    "snapshot": { "prompt": "银发" },
                })),
            },
            crate::ai::conversations::AiMessagePartDto {
                r#type: "text".into(),
                text: Some("保留构图".into()),
                attachment_id: None,
                tool_call_id: None,
                json_payload: None,
            },
        ],
    );
    let turns = turns_of(&m, &AttachmentPlan::empty());
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].content, "[CHARACTER CARD]\n凛\n银发保留构图");
}

/// `/技能` 的正文快照只从请求消息的 skill mention 中获取。
#[test]
fn explicit_skill_body_comes_from_the_user_message() {
    let mut m = message(
        "user",
        vec![crate::ai::conversations::AiMessagePartDto {
            r#type: "mention".into(),
            text: None,
            attachment_id: None,
            tool_call_id: None,
            json_payload: Some(json!({
                "kind": "skill",
                "name": "demo-skill",
                "version": "3",
                "body": "按这个做",
            })),
        }],
    );
    m.id = "u1".into();
    let found = explicit_skills_of(&[m], Some("u1"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "demo-skill");
    assert_eq!(found[0].1.as_deref(), Some("3"));
    assert_eq!(found[0].2, "按这个做");
    // 无请求消息 id / 不匹配：不加载
    assert!(explicit_skills_of(&[], Some("u1")).is_empty());
}

/// 取消旗标：**立起来 ≠ 立刻停**（主循环在安全点自己收尾，R6 要求它先把话说完）。
#[test]
fn cancel_flag_lifecycle() {
    let executor = Executor::new();
    assert!(
        !executor.request_cancel("r1"),
        "不在跑的 Run 没什么可取消的"
    );
    executor.mark_running("r1");
    assert!(executor.request_cancel("r1"));
    assert!(executor.is_cancelled("r1"));
    executor.mark_done("r1");
    assert!(
        !executor.is_cancelled("r1"),
        "收尾之后旗标必须清掉（否则下一次 Run 一开就取消）"
    );
    assert!(!executor.request_cancel("r1"));
}

/// `reason` 只给界面看：**必须摘掉**，不能漏进工具参数。
#[test]
fn take_reason_strips_the_ui_only_field() {
    let mut raw = r#"{"query":"krea","reason":"先确认本机有没有这个工作流"}"#.to_string();
    let reason = take_reason(&mut raw);
    assert_eq!(reason.as_deref(), Some("先确认本机有没有这个工作流"));
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert!(parsed.get("reason").is_none(), "reason 不许传给工具");
    assert_eq!(parsed["query"], "krea", "其余参数一个字都不许动");

    // 没写就是没有；参数不是对象时原样返回
    let mut none = r#"{"query":"x"}"#.to_string();
    assert!(take_reason(&mut none).is_none());
    assert_eq!(none, r#"{"query":"x"}"#);
    let mut array = "[1,2]".to_string();
    assert!(take_reason(&mut array).is_none());
    assert_eq!(array, "[1,2]");
}

/// 首轮模型未生成标题时的兜底（现象：两轮都显示「新对话」，列表中无法区分）。
#[test]
fn 首条用户消息兜底成标题且去掉卡片标记() {
    // 工作台把 `@` 选的卡拼在正文前面：去掉槽位标记后，第一行是卡名 —— 比「新对话」有信息量
    assert_eq!(
        title_from_user_message("[STYLE SPEC]\n二次元三次元混搭画风\nPhysically based anime…")
            .as_deref(),
        Some("二次元三次元混搭画风")
    );
    assert_eq!(
        title_from_user_message("[CHARACTER CARD]\n成熟deepseek鲸鱼娘\n正文").as_deref(),
        Some("成熟deepseek鲸鱼娘")
    );
    // 普通消息：第一行前 20 个字（按**字符**截，别把中文切坏）
    assert_eq!(
        title_from_user_message("一二三四五六七八九十一二三四五六七八九十甲乙丙").as_deref(),
        Some("一二三四五六七八九十一二三四五六七八九十")
    );
    // 只有标记 / 空消息：不编标题
    assert_eq!(
        title_from_user_message("[STYLE SPEC]\n\n[CHARACTER CARD]\n  "),
        None
    );
    assert_eq!(title_from_user_message(""), None);
}
