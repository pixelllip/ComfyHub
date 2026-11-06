//! 2D 布局模板提议（工具 `layout_propose`）：AI 出草稿 → 用户在画布上改 → 提交回工作台。
//!
//! # 这条链路在做什么（2026-11-03 用户口径）
//!
//! "用户在 AI 工作台提供描述、请求生成提示词，AI 生成提示词的同时给出可用的 2D 布局模板，
//! 我可打开预览并修改，修改后重新提交给工作台。"
//!
//! 所以布局**不是**另一个独立页面，而是提示词那一轮回复的附件：
//!
//! 1. 模型在写提示词的同一轮调一次本工具，带 1~3 个构图草稿；
//! 2. 这里把草稿规范化（越界坐标夹回、缺 id 补上、坏关系清掉）之后放进
//!    `tool_result.jsonPayload`，界面画成缩略图；
//! 3. 用户改完，前端把规格写进一条用户消息（`LayoutResubmit`）重新提交 ——
//!    模型按新框重写提示词。
//!
//! # 三条边界
//!
//! - 它**不生成图片、不写库、不碰 ComfyUI**：出图仍然走 `comfy_submit`。
//! - 坐标只有一套单位：**0~1000 归一化**（与 `lib/models/layout_spec.dart` 的
//!   `LayoutRect` 逐字节对齐）。换算成 MOSS 像素是前端的事，这里不掺第二套单位。
//! - 模型给的坐标经常越界 / 漏 id / 关系指向空气。这里**一律修正并如实记账**
//!   （`issues`）：悄悄改掉会让用户看到一套自己没画过的框；直接报错又会让模型白跑一轮。
//!
//! # 为什么校验放在后端
//!
//! 前端拿到的必须是**能直接画、能直接编辑**的规格。`LayoutSpec.fromJson` 很宽容
//! （认不出的字段回默认），宽容的代价是越界框画到画布外面、`relationTo` 指向不存在的框。
//! 校验只有这一份，就在工具出结果这一处；前端只负责画与改。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::ai::memory::take_utf16;
use crate::tools::tools::{ToolFailure, ToolOutcome};

/// 一次最多给几个模板。三版足够用户挑；再多只会把正文挤长。
const MAX_TEMPLATES: usize = 3;
/// 一个模板最多几个框。散文稳定承载约 5 个约束（见 `LayoutSpec._warnings`），
/// 但排版类画面（海报 / 分栏）会多一点，这里给到 8。
const MAX_REGIONS: usize = 8;
/// 框的最小边长（0~1000 单位）：再小画不出来、也点不中。
const MIN_SIDE: i64 = 20;

/// 工具入口：`layout_propose`。
///
/// 成功时 `payload` 是
/// `{ "layoutTemplates": [{ "title", "note", "spec": {…} }], "issues": [ … ] }`；
/// 前端只认 `layoutTemplates`（`LayoutCandidate.listFromPayload`）。
pub fn tool_propose(arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let (payload, issues) = normalize(arguments)?;
    let templates = payload
        .get("layoutTemplates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut lines = vec![format!(
        "已生成 {} 个 2D 布局模板，用户可以在对话里预览、改完再提交回来。",
        templates.len()
    )];
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
    }
    if !issues.is_empty() {
        lines.push("已自动修正（不用重试）：".to_string());
        for issue in &issues {
            lines.push(format!("· {issue}"));
        }
    }
    lines.push(
        "提示词照常写在代码块里；这里不要再重复贴坐标，也不要说「已经按布局出图」。".to_string(),
    );
    Ok(ToolOutcome::new(lines.join("\n"), payload))
}

/// 把模型给的草稿规范化成 `LayoutSpec` 的形状（纯函数，可直接单测）。
///
/// 返回 `(payload, issues)`；一个可用模板都没有时是 `INVALID_ARGUMENT` ——
/// 那种情况必须让模型重做，而不是把一张空卡摆到界面上。
pub fn normalize(arguments: &Value) -> Result<(Value, Vec<String>), ToolFailure> {
    let brief = text_of(arguments.get("brief"));
    let raw = array_of(arguments.get("templates"))
        .filter(|list| !list.is_empty())
        .ok_or_else(|| {
            ToolFailure::new(
                "INVALID_ARGUMENT",
                "缺少 templates：要给 1~3 个布局模板（每个模板至少一个带 label 的框）。",
            )
        })?;

    let mut issues = Vec::new();
    if raw.len() > MAX_TEMPLATES {
        issues.push(format!(
            "只保留前 {MAX_TEMPLATES} 个模板，多出来的 {} 个已忽略。",
            raw.len() - MAX_TEMPLATES
        ));
    }

    let mut templates = Vec::new();
    for (index, item) in raw.iter().take(MAX_TEMPLATES).enumerate() {
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
        json!({ "layoutTemplates": templates, "issues": issues }),
        issues,
    ))
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
}
