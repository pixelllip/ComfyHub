//! 布局 → 提示词：AI 译者。
//!
//! ## 边界（已确定）
//!
//! · 只做**自然语言系**第一版方言；
//! · 只走**云端 Provider**（本机 Ollama 还没装，本地运行时之后再接四）。
//! · 后端只负责"翻译 + 校验"。**模板兜底在前端** —— 那份纯函数投影在
//!   `lib/models/layout_spec.dart`，手机端也要用，不能在后端再写一份。
//!
//! ## 为什么要有校验
//!
//! 没有校验的"AI 翻译"就是高级随机数：它会漏框、会编出布局里没有的东西、
//! 会把"镜中同一个人"写成"镜中另一个人"。校验不需要模型，纯文本就能抓住
//! 其中三类；抓不住的（新增主体）宁可不报，也别误报把好结果打回去。
//!
//! 五条规则里这一版落四条：覆盖、格式、关系、方位（保守版）。

use serde_json::{json, Value};

use crate::ai::ai::require_provider_row;
use crate::provider::protocol::{Api, ChatTurn, ReasoningRequest, StreamEvent};
use crate::provider::upstream::{self, ChatRequest};
use crate::web::routes::{ApiError, AppState};

/// 译者指令版本。**改指令必须 bump**：它进接口响应，是回归对比的锚点。
///
/// v2（2026-11-01）：加空间结构（camera / surface）与"镜中要写具体可见内容"。
/// v3（2026-11-01）：把"一致"拆成可核验条目、强制显式排除、接触写实、影子方向统一、
///                  镜面不越框、长度随对象数增长。
/// v4（2026-11-01）：**正文改英文、一个框一段**（英文是这些模型的母语，分段便于逐条对账）；
///                  校验随之改成"ASCII 标签查字面 + 段落数兜底"。
pub const TRANSLATOR_VERSION: &str = "layout-translator/4";

/// 最多试两次（第一次 + 反馈问题后重写一次）。还不过就把 issues 交给前端回落模板。
const MAX_ATTEMPTS: usize = 2;

/// 输出**不设上限**。
///
/// 这里踩过两次：900 时正文一个字不剩（思考把预算吃完），改成 2000/4000 之后
/// 框一多就变成"回答截在半个句子上"——`finish_reason` 是 `incomplete`。
/// 根因是**思考 token 与正文共享同一个 `max_output_tokens`**，而思考长度随布局复杂度涨；
/// 我们没法预测，也不该跟它抢。约束交给指令（120~200 字）+ 收尾的硬截断。
const MAX_OUTPUT_TOKENS: Option<i64> = None;

/// 收尾硬截断：模型失控时保护界面。
///
/// v3 起长度随对象数增长；v4 起正文是英文（40~80 词/框，8 个框 ≈ 4000+ 字符），
/// 所以上限放到 8000 ——**不许**因为长度上限把一段约束切掉。
const MAX_PROMPT_CHARS: usize = 8000;

const INSTRUCTION: &str = r#"You are a picture-layout translator: you compile a structured spatial layout into visual constraints that a text-to-image model can follow literally.

GROUND RULES
1. The layout spec is the single source of truth for space. The user's intent may explain or add to it, but it must never move a box, resize it, change a subject's identity, or change a stated relation. If they conflict irreconcilably, follow the spec.
2. Mention every box that carries a label. For a box that carries a relation, also state the condition under which that relation holds.
3. Do not invent subjects that are not in the layout. "The scene feels incomplete" is not a reason to add anything.
4. Keep style cues verbatim. Do not rewrite, translate, or add style words. If the user supplied none, add none.
5. No quality words (masterpiece, best quality, 8k, highly detailed).
6. Never print coordinates, percentages, box ids, JSON or markdown. Numbers in the spec are only there for you to understand scale.

STRUCTURE
7. One paragraph per object, separated by a blank line. Inside each paragraph keep this order: where it is (position plus how much of the frame it covers) -> what it looks like -> how it touches, occludes or casts onto its neighbours -> what must NOT appear.
8. Lead with the subject the user named. If none was named, lead with the largest box. Then continue in descending area.

MAKE CONSISTENCY CHECKABLE
9. Never settle for "the same person, same pose". Spell it out: which hand, how it grips, finger order, the orientation of whatever is held, its contact point with something else, facial features, hair, clothing. Asymmetric details (a clip on one side, a pocket, a shadow on one side) must land on the MIRRORED side in a reflection, never on the same side.
10. One-to-one correspondence: every visible hand, arm or duplicated object needs a real counterpart in the scene. Immediately after stating a relation, add an explicit exclusion such as "no third hand appears in the mirror" or "only one comb".
11. When gazes meet, say where the look lands - "the reflection's eyes rest on the girl's own eyes, not on the camera" - instead of "they look at each other".

PHYSICS
12. Touching objects need a contact description: contact point, resting surface, what presses on what, which part is hidden. "They touch" is not a description.
13. When there is a light source, say which side each shadow falls on and keep every shadow in the scene consistent; reflections obey the same rule.
14. Only add physical consequences that necessarily follow from subjects already present: light direction, shadow, reflection, occlusion, contact. If unsure, omit it. Never add curtains, lamps or rugs just to complete the room.

FRAMING AND SPACE
15. Use `camera` and each box's `surface` to build three-dimensional relations. Objects on different walls need the corner and the relation of the two walls - "the mirror hangs on the left wall; the bed and the window sit on the right wall that meets it at a corner running down the middle of the frame". Never flatten everything onto one plane.
16. If a subject is cut by the frame edge, say which side and how much. If it is fully inside the frame, say so.
17. Mirror content stays inside the mirror frame: it is not a separate picture, and it reflects the same room, so furniture, walls and window keep their real positions in the reflection.

LENGTH
18. Length grows with the object count: 40 to 80 words per box, no upper limit. Dropping a subject, a relation, a contact or an exclusion just to stay short is the worst possible failure - prefer long over incomplete.

Output only the prompt."#;

/// `POST /api/layout/translate`。
///
/// 入参：`{ spec, intent, providerId, model }`；出参：`{ prompt, ok, issues, attempts, model, translator }`。
/// `ok = false` 时前端回落模板 —— **这条接口不返回模板**，它不持有那份逻辑。
pub async fn translate(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let spec = body
        .get("spec")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| ApiError::bad_request("缺少 spec（布局规格对象）"))?;
    let provider_id = body
        .get("providerId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("缺少 providerId"))?;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("缺少 model"))?;
    let intent = body
        .get("intent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let provider = require_provider_row(state, provider_id)?;
    let api = Api::parse(&provider.api)
        .ok_or_else(|| ApiError::bad_request(format!("不认识的协议：{}", provider.api)))?;

    let user = build_user_prompt(&spec, &intent);
    let mut prompt = String::new();
    let mut issues: Vec<String> = Vec::new();
    let mut attempts = 0usize;
    let mut finish_reason: Option<String> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        attempts = attempt;
        let content = if attempt == 1 || issues.is_empty() {
            // 第一次，或者上一版是"空正文"（不是校验问题）：原样再发一次
            user.clone()
        } else {
            format!(
                "{user}\n\n【上一版的问题，逐条改掉再重写】\n{}\n只输出提示词本身。",
                issues.join("\n")
            )
        };
        let turns = vec![
            ChatTurn {
                role: "system".into(),
                content: INSTRUCTION.into(),
                ..Default::default()
            },
            ChatTurn {
                role: "user".into(),
                content,
                ..Default::default()
            },
        ];
        let (raw, outcome) = complete(state, &provider, api, model, &turns).await?;
        finish_reason = outcome.finish_reason.clone();
        prompt = clean_prompt(&raw);
        if prompt.trim().is_empty() {
            issues = vec![empty_reason(outcome.finish_reason.as_deref())];
            continue;
        }
        if is_truncated(outcome.finish_reason.as_deref()) {
            // 半句话比空字符串更危险：它看起来像成功。宁可重试一次。
            issues = vec![truncated_reason(outcome.finish_reason.as_deref())];
            continue;
        }
        issues = validate(&spec, &prompt);
        if issues.is_empty() {
            break;
        }
    }

    Ok(json!({
        "prompt": prompt,
        "ok": issues.is_empty(),
        "issues": issues,
        "attempts": attempts,
        "model": model,
        "finishReason": finish_reason,
        "translator": TRANSLATOR_VERSION,
    }))
}

/// 上游因为预算/长度把回答截断的两种写法：OpenAI 叫 `length`，DeepSeek 叫 `incomplete`。
///
/// 2026-11-01 的实际教训：只认 `length` 时，DeepSeek 的 `incomplete` 会被当成
/// "正常收尾"，于是半句话被交给界面，还带着 `ok` 之外的 6 条"漏了某某"。
fn is_truncated(finish: Option<&str>) -> bool {
    matches!(
        finish,
        Some("length") | Some("max_tokens") | Some("incomplete") | Some("content_filter")
    )
}

/// 上游给了空正文时，说清楚是哪一种空。
fn empty_reason(finish: Option<&str>) -> String {
    if is_truncated(finish) {
        return format!(
            "模型把输出预算用完了（finish={}，思考与正文共享额度）—— 已重试；仍然为空就说明这次布局太复杂",
            finish.unwrap_or("无")
        );
    }
    format!(
        "模型没有返回正文（finish={}，上游返回了空内容）—— 重试一次通常就好；\
         连续两次说明这次请求被上游空转了",
        finish.unwrap_or("无")
    )
}

fn truncated_reason(finish: Option<&str>) -> String {
    format!(
        "回答被上游截断（finish={}）—— 只有半句，不能当成品用；已重试一次",
        finish.unwrap_or("无")
    )
}

/// 一次非流式（对调用方而言）补全：内部仍走流式通道，只是把分片拼起来。
///
/// 把 [`Outcome`] 一起带回去：`finish_reason` 是判断"空正文到底是哪种空"的唯一线索
/// （`length` = 预算被思考吃光，`stop` = 模型真的什么都没说）。
async fn complete(
    state: &AppState,
    provider: &crate::ai::ai::AiProviderRow,
    api: Api,
    model: &str,
    turns: &[ChatTurn],
) -> Result<(String, upstream::Outcome), ApiError> {
    let mut out = String::new();
    let request = ChatRequest {
        state,
        api,
        base_url: &provider.base_url,
        model,
        credential_ref: provider.credential_ref.as_deref(),
        turns,
        tools: &[],
        // effort = None：**一个思考参数都不发**。译者要的是短输出，
        // 思考模式在这里只会把 300 个 token 变成 3000 个。
        reasoning: ReasoningRequest::default(),
        max_output_tokens: MAX_OUTPUT_TOKENS,
        extras: &provider.extras,
        // 布局翻译是独立请求，不属于任何对话：{{session}} 按固定作用域展开
        conversation_id: None,
    };
    let outcome = upstream::stream_chat(
        request,
        |event| {
            if let StreamEvent::TextDelta(delta) = event {
                out.push_str(&delta);
            }
            Ok(())
        },
        || false,
    )
    .await?;
    Ok((out, outcome))
}

/// 规格 + 意图 → 一段给模型看的输入。
///
/// 规格**压成紧凑 JSON**：数字给模型是用来理解比例的（它比"宽约五成"更能算出相对大小），
/// 但铁律 6 要求正文里不许出现数字 —— 这正是"编译器"该做的事：读得懂数字，说得出口语。
fn build_user_prompt(spec: &Value, intent: &str) -> String {
    let mut regions = Vec::new();
    if let Some(list) = spec.get("regions").and_then(Value::as_array) {
        for (i, r) in list.iter().enumerate() {
            let mut item = serde_json::Map::new();
            item.insert("n".into(), json!(i + 1));
            for (from, to) in [
                ("label", "name"),
                ("kind", "type"),
                ("caption", "desc"),
                ("text", "text"),
                ("relation", "rel"),
                ("relationTo", "relTo"),
                ("surface", "surface"),
            ] {
                let v = r.get(from);
                let empty = match v {
                    Some(Value::String(s)) => s.trim().is_empty(),
                    None | Some(Value::Null) => true,
                    _ => false,
                };
                if !empty {
                    item.insert(to.into(), v.cloned().unwrap_or(Value::Null));
                }
            }
            let boxv: Vec<Value> = ["x", "y", "w", "h"]
                .iter()
                .map(|k| r.get(*k).cloned().unwrap_or(json!(0)))
                .collect();
            item.insert("box".into(), Value::Array(boxv));
            // 空白框（留白）照样给，模型要知道那里不该有东西
            regions.push(Value::Object(item));
        }
    }
    let payload = json!({
        "canvas": {
            "ratio": spec.get("width").cloned().unwrap_or(json!(1024)),
            "w": spec.get("width").cloned().unwrap_or(json!(1024)),
            "h": spec.get("height").cloned().unwrap_or(json!(1024)),
        },
        "brief": spec.get("brief").cloned().unwrap_or(json!("")),
        "camera": spec.get("camera").cloned().unwrap_or(json!("auto")),
        "regions": regions,
        "box_units": "0~1000，左上角原点",
    });
    format!(
        "【布局规格】\n{}\n\n【用户意图】\n{}",
        serde_json::to_string(&payload).unwrap_or_default(),
        if intent.is_empty() {
            "（未提供）"
        } else {
            intent
        }
    )
}

/// 去掉模型偶尔会带的东西：代码块栅栏、"提示词："前缀、首尾引号。
fn clean_prompt(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if text.starts_with("```") {
        text = text
            .trim_start_matches("```")
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim()
            .to_string();
        if let Some(end) = text.rfind("```") {
            text.truncate(end);
        }
    }
    for prefix in ["提示词：", "提示词:", "Prompt:", "prompt:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    let text = text.trim().trim_matches('"').trim().to_string();
    // 硬截断只防失控，不参与正常路径（正常输出 120~200 字）
    if text.chars().count() > MAX_PROMPT_CHARS {
        let cut: String = text.chars().take(MAX_PROMPT_CHARS).collect();
        let end = cut
            .rfind('。')
            .map(|i| i + '。'.len_utf8())
            .unwrap_or(cut.len());
        return cut[..end].to_string();
    }
    text
}

/// 五条校验里这一版能可靠执行的几条。返回空 = 通过。
fn validate(spec: &Value, prompt: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let Some(regions) = spec.get("regions").and_then(Value::as_array) else {
        return issues;
    };
    if prompt.trim().is_empty() {
        issues.push("输出是空的".to_string());
        return issues;
    }

    // ① 覆盖
    //
    // ⚠ 正文是**英文**，而框的标签多半是中文（"镜子"/"女生"）。跨语言做字面子串匹配
    // 必然误报（模型写 mirror / girl），所以：ASCII 标签按词元查，非 ASCII 标签不查字面，
    // 改用**段落数**这个与语言无关的结构指标兜底（规则 7 要求"一个框一段"）。
    let lower = prompt.to_lowercase();
    let mut labelled = 0usize;
    for r in regions {
        let name = r.get("label").and_then(Value::as_str).unwrap_or("").trim();
        let kind = r.get("kind").and_then(Value::as_str).unwrap_or("object");
        if kind == "blank" {
            if !BLANK_WORDS
                .iter()
                .any(|w| prompt.to_lowercase().contains(w))
            {
                issues.push("留白区没有被明确留出来（模型可能往那里塞了东西）".into());
            }
            continue;
        }
        if name.is_empty() {
            continue;
        }
        labelled += 1;
        if name.chars().any(|c| c.is_ascii_alphabetic()) && !covers(name, &lower) {
            issues.push(format!("漏了「{name}」：每个框都必须被提到"));
        }
    }
    // 段落数 < 有名字的框数 = 一定有人被合并或丢掉（语言无关的兜底）
    let paragraphs = prompt
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .count();
    if labelled >= 2 && paragraphs < labelled {
        issues.push(format!(
            "只有 {paragraphs} 段，但有 {labelled} 个框：规则要求一个框一段，有人被合并或漏掉了"
        ));
    }

    // ② 格式：坐标 / 百分比 / JSON 不许进正文
    if prompt.contains('%')
        || prompt.contains('{')
        || prompt.contains('}')
        || prompt.contains("bbox")
        || prompt.contains('【')
    {
        issues.push("正文里出现了数字或结构标记：规格里的数值只用于理解，不要写进提示词".into());
    }

    // ③ 关系：写了关系就必须落成句子
    for r in regions {
        let rel = r.get("relation").and_then(Value::as_str).unwrap_or("none");
        if rel == "none" || rel.is_empty() {
            continue;
        }
        let name = r.get("label").and_then(Value::as_str).unwrap_or("这一块");
        let ok = match rel {
            "mirror" => [
                "同一个人",
                "同一人",
                "镜像",
                "倒影",
                "翻转",
                "一致",
                // ⚠ 不能把 "mirror" 本身算进去：那句话里出现 "mirror" 只说明它在讲镜子，
                // 不说明它写了"镜中是本人"（第一版这么写，测试里立刻漏判）。
                "same person",
                "the same girl",
                "the same woman",
                "mirror image",
                "mirrored",
                "reflection of",
                "identical",
                "flipped",
            ]
            .iter()
            .any(|k| prompt.to_lowercase().contains(k)),
            "same" => ["同一个", "同一", "same person", "the same"]
                .iter()
                .any(|k| prompt.to_lowercase().contains(k)),
            "inFront" | "front" => ["遮", "挡", "前面", "occlud", "in front", "blocks"]
                .iter()
                .any(|k| prompt.to_lowercase().contains(k)),
            _ => true,
        };
        if !ok {
            issues.push(format!("「{name}」与其他框的关系没有写出来（{rel}）"));
        }
    }

    // ④ 方位（保守版）：规格里根本没有的那一侧，正文不许出现"画面右侧"这种明确说法。
    //    只匹配带"画面"的短语，避免把"右手""左右对称"误判。
    let (mut left, mut right, mut top, mut bottom) = (false, false, false, false);
    for r in regions {
        let x = num(r, "x");
        let y = num(r, "y");
        let w = num(r, "w");
        let h = num(r, "h");
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        if cx < 333.0 {
            left = true;
        } else if cx > 667.0 {
            right = true;
        }
        if cy < 333.0 {
            top = true;
        } else if cy > 667.0 {
            bottom = true;
        }
    }
    for (present, words, side) in [
        (
            right,
            [
                "画面右侧",
                "画面右边",
                "画面右半",
                "right side of the frame",
                "right edge of the frame",
            ],
            "右",
        ),
        (
            left,
            [
                "画面左侧",
                "画面左边",
                "画面左半",
                "left side of the frame",
                "left edge of the frame",
            ],
            "左",
        ),
        (
            top,
            [
                "画面顶部",
                "画面最上",
                "画面上缘",
                "top of the frame",
                "top edge of the frame",
            ],
            "上",
        ),
        (
            bottom,
            [
                "画面底部",
                "画面最下",
                "画面下缘",
                "bottom of the frame",
                "bottom edge of the frame",
            ],
            "下",
        ),
    ] {
        if !present
            && words
                .iter()
                .any(|w| prompt.to_lowercase().contains(&w.to_lowercase()))
        {
            issues.push(format!("正文说了画面{side}侧，但布局里那边没有框"));
        }
    }

    issues
}

fn num(r: &Value, key: &str) -> f64 {
    r.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// "留白"这件事在人话里有七八种说法，模型通常不会正好用"留白"两个字。
/// 正文是英文，所以中英都收。
const BLANK_WORDS: [&str; 15] = [
    "留白",
    "空白",
    "空着",
    "空出",
    "留出",
    "不放置",
    "不安排",
    "空无一物",
    "没有物体",
    "empty",
    "left empty",
    "left clear",
    "no objects",
    "nothing",
    "clear floor",
];

/// 标签是否被正文覆盖：整串命中，或**任意一个够长的词元**命中。
///
/// 只认长度 ≥2 的词元，避免"人""窗""门"这种单字把真正的漏项盖过去。
fn covers(label: &str, prompt_lower: &str) -> bool {
    let whole = label.trim().to_lowercase();
    if !whole.is_empty() && prompt_lower.contains(&whole) {
        return true;
    }
    let tokens: Vec<String> = label
        .split(|c: char| {
            c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；' | '/' | '|' | '·')
        })
        .map(str::trim)
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    tokens.iter().any(|t| prompt_lower.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with(regions: Value) -> Value {
        json!({"width": 768, "height": 1024, "brief": "雾夜", "regions": regions})
    }

    fn region(label: &str, x: i64, y: i64, w: i64, h: i64) -> Value {
        json!({"label": label, "kind": "object", "x": x, "y": y, "w": w, "h": h})
    }

    #[test]
    fn 全部提到且没有数字就通过() {
        // 正文是英文（v4），两个框 = 两段
        let spec = spec_with(json!([
            region("mirror", 0, 0, 200, 200),
            region("window", 700, 100, 250, 300)
        ]));
        let prompt = "A gold-rimmed mirror hangs on the left.\n\nA floor-to-ceiling window fills the right, morning light coming through it.";
        assert!(validate(&spec, prompt).is_empty());
    }

    #[test]
    fn 漏了一个框会被抓出来() {
        let spec = spec_with(json!([
            region("mirror", 0, 0, 200, 200),
            region("window", 700, 100, 250, 300)
        ]));
        let issues = validate(&spec, "A gold-rimmed mirror hangs on the left.");
        assert!(issues.iter().any(|i| i.contains("window")), "{issues:?}");
    }

    #[test]
    fn 段落数少于框数会被抓出来() {
        // 标签全是中文时不做字面匹配，这时靠"一个框一段"兜底
        let spec = spec_with(json!([
            region("镜子", 0, 0, 200, 200),
            region("窗", 700, 100, 250, 300)
        ]));
        let merged = validate(
            &spec,
            "The mirror hangs on the left and the window on the right.",
        );
        assert!(merged.iter().any(|i| i.contains("段")), "{merged:?}");
    }

    #[test]
    fn 正文里出现百分比会被打回() {
        let spec = spec_with(json!([region("mirror", 0, 0, 200, 200)]));
        let issues = validate(&spec, "The mirror covers 20% of the width.");
        assert!(issues.iter().any(|i| i.contains("数字")));
    }

    #[test]
    fn 声明了镜像却没写关系会被打回() {
        let spec = spec_with(json!([
            {"label": "girl", "x": 200, "y": 300, "w": 400, "h": 400},
            {"label": "mirror", "x": 100, "y": 50, "w": 300, "h": 300, "relation": "mirror", "relationTo": "1"}
        ]));
        let issues = validate(
            &spec,
            "The girl stands in the centre.\n\nA mirror hangs at the upper left.",
        );
        assert!(issues.iter().any(|i| i.contains("关系")), "{issues:?}");
        let ok = validate(
            &spec,
            "The girl stands in the centre, combing her hair.\n\nA mirror hangs upper-left and shows the same person, mirrored left to right.",
        );
        assert!(ok.is_empty(), "{ok:?}");
    }

    #[test]
    fn 布局左侧没有东西时正文不许说画面右侧() {
        let spec = spec_with(json!([region("桌子", 0, 300, 300, 200)]));
        let issues = validate(&spec, "桌子在画面左侧，画面右侧有一扇窗。");
        assert!(issues.iter().any(|i| i.contains("画面右")));
    }

    #[test]
    fn 留白按语义判而不是按字面() {
        let spec = spec_with(json!([
            {"label": "留白", "kind": "blank", "x": 0, "y": 650, "w": 180, "h": 350},
            region("镜子", 0, 0, 200, 200)
        ]));
        // 写法一：字面
        assert!(validate(&spec, "画面左下留白，左上是一面镜子。").is_empty());
        // 写法二：换了词，仍然是"留出来"的意思
        assert!(validate(
            &spec,
            "画面左下方留出一块空白，不放置任何物体；左上是一面镜子。"
        )
        .is_empty());
        // 真的漏了才报
        let issues = validate(&spec, "左上是一面镜子。");
        assert!(issues.iter().any(|i| i.contains("留白区")));
    }

    #[test]
    fn 带逗号的标签按词元算覆盖() {
        // danbooru 写法：模型会改写成"一位长发女孩"，整串匹配必然误报
        let spec = spec_with(json!([region("1girl, 长发", 200, 300, 400, 400)]));
        assert!(validate(&spec, "画面中央是一位长发女孩，正在梳头。").is_empty());
        let issues = validate(&spec, "画面中央有一座教堂。");
        assert!(issues.iter().any(|i| i.contains("1girl")));
    }

    #[test]
    fn 清理代码块与前缀() {
        assert_eq!(
            clean_prompt("```text\n画面左侧有一张桌子。\n```"),
            "画面左侧有一张桌子。"
        );
        assert_eq!(
            clean_prompt("提示词：画面左侧有一张桌子。"),
            "画面左侧有一张桌子。"
        );
        assert_eq!(clean_prompt("\"画面左侧\""), "画面左侧");
    }

    #[test]
    fn 规格压成紧凑输入时保留框与关系() {
        let spec = spec_with(json!([
            {"label": "女生", "x": 200, "y": 300, "w": 400, "h": 400, "kind": "character"},
            {"label": "镜子", "x": 100, "y": 50, "w": 300, "h": 300, "relation": "mirror", "relationTo": "1", "surface": "left"}
        ]));
        let text = build_user_prompt(&spec, "俯视，女生在梳头");
        assert!(text.contains("\"name\":\"镜子\""));
        assert!(text.contains("\"rel\":\"mirror\""));
        assert!(text.contains("\"surface\":\"left\""));
        assert!(text.contains("俯视，女生在梳头"));
    }

    #[test]
    fn 机位会进输入() {
        let mut spec = spec_with(json!([region("桌子", 0, 0, 200, 200)]));
        spec["camera"] = json!("topdown");
        let text = build_user_prompt(&spec, "");
        assert!(text.contains("\"camera\":\"topdown\""));
        assert!(
            text.contains("（未提供）"),
            "没给意图也要说清楚，不许发空段"
        );
    }
}
