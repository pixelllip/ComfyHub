//! 2D 布局模板（工具 `layout_propose`）：AI 出草稿 / 按提示词出核对模板 →
//! 在画布上修改 → 提交回工作台。
//!
//! # 两种用途
//!
//! - **`purpose="verify"`（默认入口）**：AI 写完一版生图提示词之后，按这版提示词生成
//!   **恰好一个**构图核对模板。它要在出图之后和产物对照，判断构图是否符合预期；
//!   用户仍可在画布上改框、再提交回对话。`comfy_submit(positivePrompt=…)` 会核对
//!   这一版提示词是否已有对应的核对模板。
//! - **`purpose="draft"`**：用户明确要"几种构图方案"时，给出 1~3 个互不相同的草稿。
//!
//! 两种用途共用同一份 `layoutTemplates` 结构与同一条编辑 / 回传链路。
//!
//! # 三条边界
//!
//! - 它**不生成图片、不写库、不碰 ComfyUI**：出图仍然走 `comfy_submit`。
//! - 坐标只有一套单位：**0~1000 归一化**（与 `lib/models/layout_spec.dart` 的
//!   `LayoutRect` 逐字节对齐）。换算成 MOSS 像素是前端的事，这里不掺第二套单位。
//! - 模型给的坐标经常越界 / 漏 id / 关系指向空气。这里**一律修正并如实记账**
//!   （`issues`）：静默修改会显示未绘制过的框；直接报错会使模型空跑一轮。
//!
//! # 为什么校验放在后端
//!
//! 前端拿到的必须是**能直接画、能直接编辑**的规格。`LayoutSpec.fromJson` 很宽容
//! （认不出的字段回默认），宽容的代价是越界框画到画布外面、`relationTo` 指向不存在的框。
//! 校验只有这一份，就在工具出结果这一处；前端只负责画与改。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::ai::memory::take_utf16;
use crate::tools::tools::{ToolFailure, ToolOutcome, ToolSession};

/// 草稿模式一次最多给几个模板。三版足够选择；再多只会使正文过长。
const MAX_DRAFTS: usize = 3;
/// 一个模板最多几个框。散文稳定承载约 5 个约束（见 `LayoutSpec._warnings`），
/// 但排版类画面（海报 / 分栏）会多一点，这里给到 8。
const MAX_REGIONS: usize = 8;
/// 框的最小边长（0~1000 单位）：再小画不出来、也点不中。
const MIN_SIDE: i64 = 20;

/// 工具入口：`layout_propose`。
///
/// 成功时 `payload` 是
/// `{ "layoutTemplates": [{ "title", "note", "purpose", "prompt", "spec": {…} }],
///    "purpose": "verify" | "draft", "verificationPrompt": "…", "issues": [ … ] }`；
/// 前端只认 `layoutTemplates`（`LayoutCandidate.listFromPayload`）。
pub fn tool_propose(arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let (payload, issues) = normalize(arguments)?;
    let templates = payload
        .get("layoutTemplates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let purpose = payload
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or("draft");

    let mut lines = if purpose == "verify" {
        vec!["已生成 1 个构图核对模板，依据这一版提示词。\
             出图后用户会和产物对照，判断构图是否符合预期。"
            .to_string()]
    } else {
        vec![format!(
            "已生成 {} 个 2D 布局模板，用户可以在对话里预览、改完再提交回来。",
            templates.len()
        )]
    };
    for template in &templates {
        let title = template
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("布局");
        let spec = template.get("spec").cloned().unwrap_or(Value::Null);
        let width = spec.get("width").and_then(Value::as_i64).unwrap_or(0);
        let height = spec.get("height").and_then(Value::as_i64).unwrap_or(0);
        let labels: Vec<&str> = spec
            .get("regions")
            .and_then(Value::as_array)
            .map(|regions| {
                regions
                    .iter()
                    .filter_map(|r| r.get("label").and_then(Value::as_str))
                    .take(6)
                    .collect()
            })
            .unwrap_or_default();
        lines.push(format!(
            "- 《{title}》{width}×{height}，{} 个框：{}",
            labels.len(),
            labels.join("、")
        ));
        // 核对模板把**规范化之后**的框回给模型：它自己写的那份可能已被夹边 /
        // 改过 id，出图后要逐框对照，不能只凭记忆。草稿不回坐标，省上下文。
        if purpose == "verify" {
            lines.extend(verify_box_lines(&spec));
        }
    }
    if !issues.is_empty() {
        lines.push("已自动修正（不用重试）：".to_string());
        for issue in &issues {
            lines.push(format!("· {issue}"));
        }
    }
    if purpose == "verify" {
        lines.push(
            "若这次提交用 comfy_submit(positivePrompt=…) 传这一版提示词，必须原样照传，\
             后端会按它核对模板。出图后按上面列出的框逐项对照产物，\
             看不到图就如实说，不要凭提示词猜画面，也不要说「已经按布局出图」。"
                .to_string(),
        );
    } else {
        lines.push(
            "提示词照常写在代码块里；这里不要再重复贴坐标，也不要说「已经按布局出图」。"
                .to_string(),
        );
    }
    Ok(ToolOutcome::new(lines.join("\n"), payload))
}

/// 核对模板的框表（喂给模型逐框对照产物用）。
///
/// 输出的是**规范化之后**的坐标，不贴原始 JSON —— 模型要的是"第几个框在哪、多大、
/// 和谁是什么关系"，可读文本比一整段 JSON 省上下文。
fn verify_box_lines(spec: &Value) -> Vec<String> {
    let regions = spec
        .get("regions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if regions.is_empty() {
        return Vec::new();
    }
    let mut index_of: HashMap<&str, usize> = HashMap::new();
    for (index, region) in regions.iter().enumerate() {
        if let Some(id) = region.get("id").and_then(Value::as_str) {
            index_of.insert(id, index + 1);
        }
    }
    let mut lines = Vec::with_capacity(regions.len());
    for (index, region) in regions.iter().enumerate() {
        let label = region.get("label").and_then(Value::as_str).unwrap_or("框");
        let kind = match region.get("kind").and_then(Value::as_str) {
            Some("character") => "角色",
            Some("text") => "文字",
            Some("blank") => "留白",
            _ => "物体",
        };
        let mut line = format!(
            "    {}. {label} [{kind}] x={} y={} w={} h={}",
            index + 1,
            region.get("x").and_then(Value::as_i64).unwrap_or(0),
            region.get("y").and_then(Value::as_i64).unwrap_or(0),
            region.get("w").and_then(Value::as_i64).unwrap_or(0),
            region.get("h").and_then(Value::as_i64).unwrap_or(0),
        );
        if let Some(surface) = region
            .get("surface")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let name = match surface {
                "left" => "左墙",
                "right" => "右墙",
                "back" => "正墙",
                "floor" => "地面",
                "table" => "桌面",
                other => other,
            };
            line.push_str(&format!("，面={name}"));
        }
        if let Some(relation) = region
            .get("relation")
            .and_then(Value::as_str)
            .filter(|r| *r != "none")
        {
            let name = match relation {
                "mirror" => "镜像",
                "same" => "同一主体",
                "inFront" => "挡在前面",
                other => other,
            };
            let target = region
                .get("relationTo")
                .and_then(Value::as_str)
                .and_then(|id| index_of.get(id))
                .map(|n| format!(" {n}"));
            line.push_str(&format!("，关系={name}{}", target.unwrap_or_default()));
        }
        if let Some(caption) = region
            .get("caption")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            line.push_str(&format!("，说明={}", take_utf16(caption, 40)));
        }
        lines.push(line);
    }
    lines
}

/// 把模型给的草稿规范化成 `LayoutSpec` 的形状（纯函数，可直接单测）。
///
/// 返回 `(payload, issues)`；一个可用模板都没有时是 `INVALID_ARGUMENT` ——
/// 那种情况必须让模型重做，而不是把一张空卡摆到界面上。
pub fn normalize(arguments: &Value) -> Result<(Value, Vec<String>), ToolFailure> {
    let brief = text_of(arguments.get("brief"));
    let prompt = take_utf16(&text_of(arguments.get("prompt")), 16_000);
    let purpose = match text_of(arguments.get("purpose")).as_str() {
        "verify" => "verify",
        "draft" => "draft",
        // 没写 purpose 但给了 prompt：按"按提示词出核对模板"处理，避免老调用方
        // 漏一个字段就绕过强制检查。
        "" if !prompt.is_empty() => "verify",
        "" => "draft",
        other => {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("purpose 只能是 verify（按提示词出核对模板）或 draft（构图草稿），收到「{other}」。"),
            ))
        }
    };
    let raw = array_of(arguments.get("templates"))
        .filter(|list| !list.is_empty())
        .ok_or_else(|| {
            ToolFailure::new(
                "INVALID_ARGUMENT",
                if purpose == "verify" {
                    "缺少 templates：要给恰好 1 个构图核对模板（至少一个带 label 的框）。"
                } else {
                    "缺少 templates：要给 1~3 个布局模板（每个模板至少一个带 label 的框）。"
                },
            )
        })?;

    if purpose == "verify" {
        if prompt.is_empty() {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                "purpose=verify 时必须给 prompt：把刚写好的那版生图提示词原文放进去。",
            ));
        }
        if raw.len() != 1 {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!(
                    "purpose=verify 只收 1 个构图核对模板，收到 {} 个。\
                     要几种构图方案请另调一次 purpose=draft。",
                    raw.len()
                ),
            ));
        }
    }

    let mut issues = Vec::new();
    let limit = if purpose == "verify" { 1 } else { MAX_DRAFTS };
    if raw.len() > limit {
        issues.push(format!(
            "只保留前 {limit} 个模板，多出来的 {} 个已忽略。",
            raw.len() - limit
        ));
    }

    let mut templates = Vec::new();
    for (index, item) in raw.iter().take(limit).enumerate() {
        let Some(obj) = item.as_object() else {
            issues.push(format!("第 {} 个模板不是对象，已忽略。", index + 1));
            continue;
        };
        let title = {
            let raw_title = take_utf16(&text_of(obj.get("title")), 80);
            if raw_title.is_empty() {
                format!("布局 {}", index + 1)
            } else {
                raw_title
            }
        };
        let note = take_utf16(&text_of(obj.get("note")), 200);
        let width = int_of(obj.get("width")).unwrap_or(1024).clamp(256, 4096);
        let height = int_of(obj.get("height")).unwrap_or(1024).clamp(256, 4096);
        let camera = match text_of(obj.get("camera")).as_str() {
            "topdown" => "topdown",
            "eye" => "eye",
            "low" => "low",
            _ => "auto",
        };
        let regions = regions_of(obj.get("regions"), &title, &mut issues);
        if regions.is_empty() {
            issues.push(format!("《{title}》没有可用的框，已忽略这个模板。"));
            continue;
        }
        templates.push(json!({
            "title": title,
            "note": note,
            "purpose": purpose,
            "prompt": if purpose == "verify" { json!(prompt.clone()) } else { Value::Null },
            "spec": {
                "v": 1,
                "width": width,
                "height": height,
                "brief": brief,
                "camera": camera,
                "regions": regions,
            },
        }));
    }

    if templates.is_empty() {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            "没有可用的布局模板：每个模板至少要有一个带 label 的框（坐标用 0~1000 的整数）。",
        ));
    }
    Ok((
        json!({
            "layoutTemplates": templates,
            "purpose": purpose,
            "verificationPrompt": if purpose == "verify" { json!(prompt) } else { Value::Null },
            "issues": issues,
        }),
        issues,
    ))
}

/// 从工具结果里读"这版核对模板依据的提示词"（`purpose != verify` → `None`）。
pub fn verification_prompt(payload: &Value) -> Option<String> {
    if payload.get("purpose").and_then(Value::as_str) != Some("verify") {
        return None;
    }
    payload
        .get("verificationPrompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
}

/// 核对模板的简短复述（标题 + 画幅 + 逐框表）。
///
/// `comfy_submit` 出图后的工具结果会带上它：让模型在**同一屏**里同时看到"产物已附上"
/// 和"该对哪些框"，而不是指望它还记着几十轮之前的模板正文。
pub fn verification_brief(payload: &Value) -> Option<String> {
    let template = payload.get("layoutTemplates")?.as_array()?.first()?;
    let spec = template.get("spec")?;
    let title = template
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("布局");
    let width = spec.get("width").and_then(Value::as_i64).unwrap_or(0);
    let height = spec.get("height").and_then(Value::as_i64).unwrap_or(0);
    let boxes = verify_box_lines(spec);
    if boxes.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(boxes.len() + 1);
    lines.push(format!("《{title}》{width}×{height}"));
    lines.extend(boxes);
    Some(lines.join("\n"))
}

/// 出图成功之后，把"必须核对"这件事和框表一起塞回工具结果。
///
/// 为什么不能只写在系统提示里：规则在几百行之外，产物图在下一轮，模型很容易只回一句
/// "已出图"。这里把核对对象放到它**紧邻**的位置，并把"看不到图就如实说"写进同一段，
/// 避免它凭提示词把画面编一遍。
pub fn append_post_submit_check(
    session: &ToolSession,
    arguments: &Value,
    outcome: &mut ToolOutcome,
) {
    let produced = outcome
        .payload
        .get("referenceImages")
        .and_then(Value::as_array)
        .map(|images| !images.is_empty())
        .unwrap_or(false);
    if !produced {
        return;
    }
    let prompt = arguments
        .get("positivePrompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let Some(brief) = session.layout_brief_for(prompt) else {
        return;
    };
    outcome.content.push_str("\n\n【出图后必须核对构图】\n");
    outcome.content.push_str("这次提交对应的核对模板如下：\n");
    outcome.content.push_str(brief);
    outcome.content.push_str(
        "\n产物是图片时，服务端会把它们附在下一轮。逐项对照主体数量与身份、位置、占比、\
         遮挡、画幅：符合的写「符合」，不符合的写「不符合（差在哪）」。\
         看不到图就如实说，不要凭提示词编画面。\n",
    );
}

/// 提交出图前的闸门：模型自己写了正面提示词时，本 Run 必须已有按这版提示词
/// 生成的核对模板。
///
/// 只拦 `positivePrompt`：它是"这版提示词由模型定稿"的唯一结构化证据。
/// 让模型照本地 `TextGenerate` 扩写、或只改 `overrides` 的提交不在这一闸门里。
pub fn ensure_verification_template(
    session: &ToolSession,
    arguments: &Value,
) -> Result<(), ToolFailure> {
    let Some(prompt) = arguments
        .get("positivePrompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    else {
        return Ok(());
    };
    if session.has_layout_verification(prompt) {
        return Ok(());
    }
    if session.has_any_layout_verification() {
        return Err(ToolFailure::new(
            "LAYOUT_TEMPLATE_MISMATCH",
            "这次提交的提示词和构图核对模板依据的提示词不一致。\
             请按这次提交的 positivePrompt 原文重新调一次 \
             layout_propose(purpose=\"verify\", prompt=\"<这一版提示词>\")，\
             再重新提交。",
        ));
    }
    Err(ToolFailure::new(
        "LAYOUT_TEMPLATE_REQUIRED",
        "提交出图前必须先按这次提示词生成构图核对模板。\
         请调用 layout_propose(purpose=\"verify\", prompt=\"<这一版提示词>\", \
         templates=[恰好 1 个模板])，模板里的主体 / 位置 / 比例要与提示词一致；\
         然后再提交。",
    ))
}

/// 提示词比对口径：去掉首尾空白，连续空白压成一个空格。
///
/// 模型很少会逐字节复述，但"同一版提示词"至少应只差空白。
pub fn prompt_key(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 一个模板里的 `regions` → `LayoutRegion` 的 JSON 形状。
///
/// 两趟：先给每个框分配稳定的 `r1..rN` 并把模型给的 id 记进映射，
/// 第二趟才按映射改写 `relationTo` —— 模型经常用 `r2` 指第 2 个框，
/// 也经常自己写 `"mirror-left"` 这种 id，映射能同时接住两种。
fn regions_of(raw: Option<&Value>, template_title: &str, issues: &mut Vec<String>) -> Vec<Value> {
    let Some(list) = array_of(raw).filter(|l| !l.is_empty()) else {
        return Vec::new();
    };
    if list.len() > MAX_REGIONS {
        issues.push(format!(
            "《{template_title}》有 {} 个框，只保留前 {MAX_REGIONS} 个。",
            list.len()
        ));
    }

    let mut ids: HashMap<String, String> = HashMap::new();
    let mut output: Vec<Value> = Vec::new();
    // (在 output 里的下标, relation, relationTo 原文)
    let mut pending: Vec<(usize, String, String)> = Vec::new();

    for (index, item) in list.iter().take(MAX_REGIONS).enumerate() {
        let Some(obj) = item.as_object() else {
            issues.push(format!(
                "《{template_title}》第 {} 个框不是对象，已忽略。",
                index + 1
            ));
            continue;
        };
        let kind = match text_of(obj.get("kind")).as_str() {
            "character" => "character",
            "text" => "text",
            "blank" => "blank",
            other if other.is_empty() || other == "object" => "object",
            other => {
                issues.push(format!(
                    "《{template_title}》第 {} 个框的 kind「{other}」不认识，按 object 处理。",
                    index + 1
                ));
                "object"
            }
        };

        let (Some(width), Some(height)) = (int_of(obj.get("w")), int_of(obj.get("h"))) else {
            issues.push(format!(
                "《{template_title}》第 {} 个框缺 w/h，已忽略。",
                index + 1
            ));
            continue;
        };
        if width <= 0 || height <= 0 {
            issues.push(format!(
                "《{template_title}》第 {} 个框宽高不是正数，已忽略。",
                index + 1
            ));
            continue;
        }
        let w = width.clamp(MIN_SIDE, 1000);
        let h = height.clamp(MIN_SIDE, 1000);
        let x = int_of(obj.get("x")).unwrap_or(0).clamp(0, 1000 - w);
        let y = int_of(obj.get("y")).unwrap_or(0).clamp(0, 1000 - h);
        if x != int_of(obj.get("x")).unwrap_or(0) || y != int_of(obj.get("y")).unwrap_or(0) {
            issues.push(format!(
                "《{template_title}》的「{}」越出画布，已夹回界内。",
                first_name(obj)
            ));
        }

        let mut label = take_utf16(&text_of(obj.get("label")), 120);
        let caption = take_utf16(&text_of(obj.get("caption")), 400);
        let text = take_utf16(&text_of(obj.get("text")), 300);
        if label.is_empty() && caption.is_empty() && kind != "blank" {
            if kind == "text" && !text.is_empty() {
                label = take_utf16(&text, 40);
            } else {
                issues.push(format!(
                    "《{template_title}》第 {} 个框没有 label / caption，已忽略（模型要给每个框起个名）。",
                    index + 1
                ));
                continue;
            }
        }
        if kind == "text" && text.is_empty() {
            issues.push(format!(
                "《{template_title}》的「{label}」是文字框却没给 text，用户要自己填。",
            ));
        }

        let id = format!("r{}", output.len() + 1);
        let raw_id = text_of(obj.get("id"));
        if !raw_id.is_empty() {
            ids.insert(raw_id, id.clone());
        }
        // 两种「按序号指」的写法都接住：`r2` 与 `2`。
        ids.insert(format!("r{}", index + 1), id.clone());
        ids.insert((index + 1).to_string(), id.clone());

        let relation = match text_of(obj.get("relation")).as_str() {
            "mirror" => "mirror",
            "same" => "same",
            "front" | "inFront" => "inFront",
            _ => "none",
        };
        let surface = match text_of(obj.get("surface")).as_str() {
            "left" => "left",
            "right" => "right",
            "back" => "back",
            "floor" => "floor",
            "table" => "table",
            _ => "",
        };
        if relation != "none" {
            pending.push((
                output.len(),
                relation.to_string(),
                text_of(obj.get("relationTo")),
            ));
        }

        output.push(json!({
            "id": id,
            "x": x,
            "y": y,
            "w": w,
            "h": h,
            "kind": kind,
            "label": label,
            "caption": caption,
            "text": text,
            "relation": "none",
            "relationTo": Value::Null,
            "surface": surface,
        }));
    }

    for (index, relation, target) in pending {
        let target_id = ids.get(&target).cloned();
        let self_id = output[index]
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if target_id.as_deref() == Some(self_id) {
            issues.push(format!(
                "《{template_title}》的「{}」关系指向自己，已清掉。",
                output[index]
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("框")
            ));
            continue;
        }
        match target_id {
            Some(target_id) => {
                output[index]["relation"] = json!(relation);
                output[index]["relationTo"] = json!(target_id);
            }
            None => issues.push(format!(
                "《{template_title}》的「{}」关系指向不存在的框「{target}」，已清掉。",
                output[index]
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("框")
            )),
        }
    }
    output
}

/// 报错 / 提示里怎么称呼一个还没规范化的框。
fn first_name(obj: &serde_json::Map<String, Value>) -> String {
    let label = text_of(obj.get("label"));
    if !label.is_empty() {
        return label;
    }
    let caption = text_of(obj.get("caption"));
    if !caption.is_empty() {
        return take_utf16(&caption, 20);
    }
    "未命名的框".to_string()
}

/// 数组字段：接受数组本身，也接受"被模型序列化成字符串的数组"。
///
/// 现场（2026-10-25 的 `write_file`）证明模型会把整份参数再包一层字符串；
/// 工具入口虽然会解外层，但**字段里**再套一层 JSON 字符串仍然常见。这里解一层，
/// 解不出来就返回 `None`（让调用方按"没给"报错，不吞成空数组）。
fn array_of(value: Option<&Value>) -> Option<Vec<Value>> {
    match value? {
        Value::Array(items) => Some(items.clone()),
        Value::String(text) => serde_json::from_str::<Value>(text.trim())
            .ok()?
            .as_array()
            .cloned(),
        _ => None,
    }
}

/// 任意 JSON 标量 → 去掉首尾空白的字符串（对象 / 数组 / null → 空串）。
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

/// 数字字段：接受整数、浮点（四舍五入）与数字字符串。
fn int_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f.round() as i64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Value {
        json!({
            "brief": "雾夜废墟，一名提灯的旅人正走近教堂",
            "templates": [{
                "title": "三分法：旅人压左",
                "note": "主体压左三分线，右侧留出教堂",
                "width": 768,
                "height": 1024,
                "camera": "eye",
                "regions": [
                    {"id": "person", "label": "提灯的旅人", "caption": "灯是唯一暖色",
                     "kind": "character", "x": 40, "y": 300, "w": 260, "h": 640},
                    {"id": "church", "label": "教堂", "kind": "object",
                     "x": 620, "y": 120, "w": 360, "h": 760},
                    {"id": "glow", "label": "镜中的她", "kind": "character",
                     "x": 700, "y": 200, "w": 120, "h": 200,
                     "relation": "mirror", "relationTo": "person", "surface": "left"}
                ]
            }]
        })
    }

    #[test]
    fn payload形状与前端_layout_spec_对得上() {
        let (payload, issues) = normalize(&valid()).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        let spec = &payload["layoutTemplates"][0]["spec"];
        assert_eq!(spec["v"], 1);
        assert_eq!(spec["width"], 768);
        assert_eq!(spec["height"], 1024);
        assert_eq!(spec["camera"], "eye");
        assert_eq!(spec["brief"], "雾夜废墟，一名提灯的旅人正走近教堂");
        let regions = spec["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 3);
        // id 会被重新编号（模型给的 id 只用来解析关系）
        assert_eq!(regions[0]["id"], "r1");
        assert_eq!(regions[2]["relation"], "mirror");
        assert_eq!(regions[2]["relationTo"], "r1");
        assert_eq!(regions[2]["surface"], "left");
        assert_eq!(regions[2]["kind"], "character");
    }

    #[test]
    fn 越界框被夹回界内并记账() {
        let mut args = valid();
        args["templates"][0]["regions"][0]["x"] = json!(900);
        args["templates"][0]["regions"][0]["w"] = json!(400);
        let (payload, issues) = normalize(&args).unwrap();
        let region = &payload["layoutTemplates"][0]["spec"]["regions"][0];
        assert_eq!(region["w"], 400);
        assert_eq!(region["x"], 600, "900+400 越界，必须夹回 1000-400");
        assert!(issues.iter().any(|i| i.contains("越出画布")), "{issues:?}");
    }

    #[test]
    fn 没有名称的框被丢掉并如实报告() {
        let mut args = valid();
        args["templates"][0]["regions"][1]
            .as_object_mut()
            .unwrap()
            .remove("label");
        let (payload, issues) = normalize(&args).unwrap();
        assert_eq!(
            payload["layoutTemplates"][0]["spec"]["regions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            issues.iter().any(|i| i.contains("没有 label")),
            "{issues:?}"
        );
    }

    #[test]
    fn 指向不存在框的关系被清掉() {
        let mut args = valid();
        args["templates"][0]["regions"][2]["relationTo"] = json!("nobody");
        let (payload, issues) = normalize(&args).unwrap();
        let region = &payload["layoutTemplates"][0]["spec"]["regions"][2];
        assert_eq!(region["relation"], "none");
        assert!(region["relationTo"].is_null());
        assert!(
            issues.iter().any(|i| i.contains("不存在的框")),
            "{issues:?}"
        );
    }

    #[test]
    fn 关系词_front_输出成前端枚举名_in_front() {
        let mut args = valid();
        args["templates"][0]["regions"][2]["relation"] = json!("front");
        let (payload, _) = normalize(&args).unwrap();
        // ⚠ Dart 的 `LayoutRelation.fromJson` 按**枚举 name** 读，所以出口必须是 `inFront`；
        //   写成 `front` 的话卡片上的关系会静默变回"无"。
        assert_eq!(
            payload["layoutTemplates"][0]["spec"]["regions"][2]["relation"],
            "inFront"
        );
    }

    #[test]
    fn 模板被模型序列化成字符串也能读() {
        // 模型时不时会把数组再包成一段 JSON 文本（与 `normalize_arguments_text` 同一类手滑）
        let templates = valid()["templates"].to_string();
        let (payload, _) = normalize(&json!({ "brief": "x", "templates": templates })).unwrap();
        assert_eq!(
            payload["layoutTemplates"][0]["spec"]["regions"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn 空模板表是参数错误() {
        let err = normalize(&json!({"brief": "x", "templates": []})).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        let err = normalize(&json!({"brief": "x"})).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
    }

    #[test]
    fn 最多三个模板_每个最多八个框() {
        let region = json!({"label": "x", "x": 0, "y": 0, "w": 100, "h": 100});
        let regions: Vec<Value> = (0..10).map(|_| region.clone()).collect();
        let template = json!({"title": "t", "regions": regions});
        let args = json!({"brief": "x", "templates": [template.clone(), template.clone(), template.clone(), template]});
        let (payload, issues) = normalize(&args).unwrap();
        let templates = payload["layoutTemplates"].as_array().unwrap();
        assert_eq!(templates.len(), 3);
        assert_eq!(templates[0]["spec"]["regions"].as_array().unwrap().len(), 8);
        assert!(
            issues.iter().any(|i| i.contains("只保留前 3 个")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.contains("只保留前 8 个")),
            "{issues:?}"
        );
    }

    #[test]
    fn 工具正文告诉模型模板数且不复述坐标() {
        let outcome = tool_propose(&valid()).unwrap();
        assert!(
            outcome.content.contains("1 个 2D 布局模板"),
            "{}",
            outcome.content
        );
        assert!(
            outcome.content.contains("《三分法：旅人压左》"),
            "{}",
            outcome.content
        );
        assert!(
            outcome.content.contains("提灯的旅人"),
            "{}",
            outcome.content
        );
        assert!(
            !outcome.content.contains("\"regions\""),
            "正文里不该贴 JSON：{}",
            outcome.content
        );
        assert_eq!(
            outcome.payload["layoutTemplates"].as_array().unwrap().len(),
            1
        );
    }

    fn verify_args() -> Value {
        let mut args = valid();
        args["purpose"] = json!("verify");
        args["prompt"] = json!("1girl, standing left of the church, lantern in hand");
        args
    }

    #[test]
    fn verify_必须给提示词且只收一个模板() {
        let mut args = verify_args();
        args.as_object_mut().unwrap().remove("prompt");
        let err = normalize(&args).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("prompt"), "{}", err.message);

        let mut args = verify_args();
        let template = args["templates"][0].clone();
        args["templates"] = json!([template.clone(), template]);
        let err = normalize(&args).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("只收 1 个"), "{}", err.message);
    }

    #[test]
    fn verify_payload_带提示词与用途() {
        let (payload, issues) = normalize(&verify_args()).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(payload["purpose"], "verify");
        let template = &payload["layoutTemplates"][0];
        assert_eq!(template["purpose"], "verify");
        assert_eq!(
            template["prompt"],
            "1girl, standing left of the church, lantern in hand"
        );
        assert_eq!(
            verification_prompt(&payload).as_deref(),
            Some("1girl, standing left of the church, lantern in hand")
        );
    }

    #[test]
    fn verify_正文说出图后对照() {
        let outcome = tool_propose(&verify_args()).unwrap();
        assert!(
            outcome.content.contains("构图核对模板"),
            "{}",
            outcome.content
        );
        assert!(outcome.content.contains("出图后"), "{}", outcome.content);
        assert!(
            !outcome.content.contains("\"regions\""),
            "{}",
            outcome.content
        );
    }

    #[test]
    fn verify_正文带规范化框表_模型能逐框对照产物() {
        let outcome = tool_propose(&verify_args()).unwrap();
        assert!(
            outcome
                .content
                .contains("1. 提灯的旅人 [角色] x=40 y=300 w=260 h=640"),
            "{}",
            outcome.content
        );
        assert!(
            outcome
                .content
                .contains("2. 教堂 [物体] x=620 y=120 w=360 h=760"),
            "{}",
            outcome.content
        );
        assert!(
            outcome.content.contains("关系=镜像 1"),
            "{}",
            outcome.content
        );
        // 草稿模式不贴框表（模型刚写完，省上下文）
        let draft = tool_propose(&valid()).unwrap();
        assert!(
            !draft.content.contains("x=40 y=300"),
            "草稿不该重复贴坐标：{}",
            draft.content
        );
    }

    #[test]
    fn 出图后核对提醒带框表且只认本次提示词() {
        let mut session = ToolSession::default();
        let proposed = tool_propose(&verify_args()).unwrap();
        let prompt = verification_prompt(&proposed.payload).unwrap();
        let brief = verification_brief(&proposed.payload).unwrap();
        assert!(brief.contains("提灯的旅人 [角色] x=40 y=300 w=260 h=640"));
        session.record_layout_verification(&prompt, brief);

        // 产物 + 同一版提示词 → 提醒里必须带框表（模型不用回头翻几十轮之前的正文）
        let mut submitted = ToolOutcome::new(
            "{}",
            json!({
                "mediaIds": [1, 2],
                "mediaCount": 2,
                "referenceImages": [{ "path": "a.png" }],
            }),
        );
        append_post_submit_check(
            &session,
            &json!({ "positivePrompt": prompt }),
            &mut submitted,
        );
        assert!(submitted.content.contains("【出图后必须核对构图】"));
        assert!(
            submitted
                .content
                .contains("提灯的旅人 [角色] x=40 y=300 w=260 h=640"),
            "{}",
            submitted.content
        );
        assert!(submitted.content.contains("看不到图就如实说"));

        // 提交没带提示词 → 用本 Run 最近那一版模板提醒
        let mut fallback = ToolOutcome::new(
            "{}",
            json!({ "mediaIds": [3], "referenceImages": [{ "path": "b.png" }] }),
        );
        append_post_submit_check(&session, &json!({}), &mut fallback);
        assert!(fallback.content.contains("提灯的旅人"));

        // 提示词对不上 → 不许拿另一版模板硬套
        let mut other = ToolOutcome::new(
            "{}",
            json!({ "mediaIds": [4], "referenceImages": [{ "path": "c.png" }] }),
        );
        append_post_submit_check(
            &session,
            &json!({ "positivePrompt": "另一版完全不同的提示词" }),
            &mut other,
        );
        assert!(!other.content.contains("出图后必须核对构图"));

        // 没出产物 → 不提醒
        let mut empty = ToolOutcome::new("{}", json!({ "mediaIds": [] }));
        append_post_submit_check(&session, &json!({ "positivePrompt": prompt }), &mut empty);
        assert!(!empty.content.contains("出图后必须核对构图"));

        // 视频 / 音频没有可附的图片 → 不假装"已附上"
        let mut video = ToolOutcome::new("{}", json!({ "mediaIds": [9] }));
        append_post_submit_check(&session, &json!({ "positivePrompt": prompt }), &mut video);
        assert!(!video.content.contains("出图后必须核对构图"));

        // 同一 Run 有两版模板、提交又没带提示词 → 不猜，宁可不提醒
        session.record_layout_verification("第二版提示词", "《第二版》1024×1024".to_string());
        let mut ambiguous = ToolOutcome::new(
            "{}",
            json!({ "mediaIds": [5], "referenceImages": [{ "path": "d.png" }] }),
        );
        append_post_submit_check(&session, &json!({}), &mut ambiguous);
        assert!(!ambiguous.content.contains("出图后必须核对构图"));
    }

    #[test]
    fn 提交闸门_没有核对模板就拒绝() {
        let session = ToolSession::default();
        let err =
            ensure_verification_template(&session, &json!({ "positivePrompt": "1girl, lantern" }))
                .unwrap_err();
        assert_eq!(err.code, "LAYOUT_TEMPLATE_REQUIRED");
    }

    #[test]
    fn 提交闸门_提示词不一致要重新生成() {
        let mut session = ToolSession::default();
        session.record_layout_verification("1girl, lantern", "《模板》1024×1024".to_string());
        let err = ensure_verification_template(
            &session,
            &json!({ "positivePrompt": "1girl, lantern, night" }),
        )
        .unwrap_err();
        assert_eq!(err.code, "LAYOUT_TEMPLATE_MISMATCH");
    }

    #[test]
    fn 提交闸门_同一版提示词忽略空白差异() {
        let mut session = ToolSession::default();
        session.record_layout_verification(
            "1girl,  lantern\nin hand",
            "《模板》1024×1024".to_string(),
        );
        assert!(ensure_verification_template(
            &session,
            &json!({ "positivePrompt": "1girl, lantern in hand" }),
        )
        .is_ok());
        // 没有 positivePrompt 的提交（照本地 TextGenerate 扩写 / 只改 overrides）不拦
        assert!(ensure_verification_template(&session, &json!({ "promptId": 1 })).is_ok());
    }
}
