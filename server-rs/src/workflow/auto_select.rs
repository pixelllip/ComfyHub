//! **自动选工作流**：手机只说"用某族画一张"时，桌面按固定顺序挑一份能跑的工作流
//! （`docs/workflow-auto-select-plan.md` §1.2 的床上场景，第 ① ② 档）。
//!
//! # 为什么要有这一层
//!
//! 手机上**没有**工作流库（`prompts` 由桌面产生，契约 §4），也没有
//! `comfy_list_workflows` / `comfy_find_workflow` 那几条桌面工具 —— 手机 AI 写得出提示词，
//! 但**说不出要提交哪份工作流**。所以"用 anima 画…"这句话必须由桌面翻成
//! `promptId + overrides`，否则那条链只能退回到"人工在手机上挑一份"。
//!
//! # 顺序（硬约束，与 §1.2 逐条对应）
//!
//! 1. **同任务 + 同模型族的最近一次成功**（`capture_runs.status='success'` 且有 API 图）；
//! 2. **配方的已验证绑定**（`workflow_recipes.verified_json` 里有这个族的模型，
//!    即这组槽位覆盖**真跑通过**）；顺带把那条验证记录里的槽位覆盖**原样带上**，
//!    保证"这次用的模型就是上次跑通的那几个"。
//!
//! **只有这两档自动继续**：再往下（完整配方但没跑过 / 换族重绑 / 官方模板）要么需要
//! `exact` 绑定、要么需要 `fit` 现场判断，超出"无人值守零交互"的前提 —— 拿不准就不猜，
//! 由调用方回一条"需要你确认 X"（`docs/workflow-auto-select-plan.md` §1.2 第 1 条）。
//!
//! # "同族"怎么判
//!
//! 族名是**常用称呼**（anima / krea2 / krea 2 / moss image / moss_image），而模型文件名是
//! `silvermoonmixAnimaEvolved_v2329B_int8.safetensors` 这种。判据只有一条：
//! **两边都去掉非字母数字、转小写之后，模型名里包含族名**。
//! 所以 "moss image" 与 "moss_image" 是同一个族、"krea 2" 能命中 `krea2Turbo…`。
//!
//! ⚠ 这条判据与 `comfy_recent_successes` 的 `modelQuery` **共用同一份实现** ——
//! 两处各写一种匹配方式，就会出现"工具说命中、自动选说没有"（不报错，只是行为不一致）。

use rusqlite::Connection;
use serde_json::{Map, Value};

use crate::workflow::workflow_recipe::{analyze, Task, Verified};

/// 一次"想要什么"（手机端能说清楚的只有这些）。
#[derive(Debug, Clone, Default)]
pub struct Wanted {
    /// 任务类型（t2i / i2i / edit …）；`None` = 不限
    pub task: Option<Task>,
    /// 模型族（常用称呼；见模块头的匹配判据）；`None` = 不限
    pub family: Option<String>,
}

/// 挑中的那一份（调用方据此拼 `comfy_submit` 的参数）。
#[derive(Debug, Clone)]
pub struct Picked {
    pub prompt_id: i64,
    /// `recent` / `recipe` —— 直接就是 `capture_runs.source_kind` 的取值（T3 的账要能对上）
    pub source_kind: &'static str,
    pub recipe_id: Option<i64>,
    /// 这一族**验证过**的槽位覆盖（`节点id.输入名 → 文件`）：照抄，不重新猜
    pub overrides: Vec<(String, String)>,
    /// 人话：走了哪一档、用的是哪次运行
    pub note: String,
}

/// 挑的结果：要么挑到一份，要么**如实列出试过哪几档**（给"需要你确认 X"那句话用）。
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub picked: Option<Picked>,
    pub tried: Vec<String>,
}

/// 从 `capture_runs.raw` 里取出 API 节点图 —— **两种存法都认**：
///
/// * 直接存图（`{"1": {"class_type": …}}`）；
/// * 存整条 `/history` 条目（`{"prompt": [序号, prompt_id, 图, …], "outputs": …}`）——
///   **真机库里的 raw 就是这一种**（2026-11-09 抽查 `capture_runs`：顶层键是
///   `prompt` / `outputs` / `status` / `meta`）。
///
/// ⚠ 这条不是"宽厚处理"，是**必须**：只认第一种的代码在真库上会一条都扫不出来，
/// 而它的表现是"工具说没有成功记录"（不报错）。`comfy_recent_successes` 与自动选工作流
/// 共用这一份，判据不许分家。
pub fn graph_of_raw(raw: &str) -> Option<Map<String, Value>> {
    let doc: Value = serde_json::from_str(raw).ok()?;
    let parsed = crate::comfy::capture::history::parse(&doc).graph;
    crate::workflow::workflow_fit::api_graph_of(parsed.as_ref().or(Some(&doc)))
}

/// 扫最近多少条成功记录（与 `comfy_recent_successes` 同档：一次小查询）。
const SCAN_RECENT: usize = 300;
/// 扫多少条配方（配方库是桌面自己的知识，量级很小）。
const SCAN_RECIPES: usize = 200;

/// 族名 / 模型名归一化成"只留字母数字、小写"。
///
/// 例：`moss image` → `mossimage`；`krea-2` → `krea2`；`silvermoonmixAnima…` → `silvermoonmixanima…`。
pub fn normalize_family(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 这个模型文件名属不属于这个族（`family` 为空 = 不限 → 恒真）。
pub fn family_matches(model: &str, family: &str) -> bool {
    let family = normalize_family(family);
    if family.is_empty() {
        return true;
    }
    normalize_family(model).contains(&family)
}

/// 按 §1.2 的顺序挑一份（`conn` 只读；不碰网络、不碰策略）。
pub fn pick(conn: &Connection, wanted: &Wanted) -> rusqlite::Result<Outcome> {
    let mut outcome = Outcome::default();
    if let Some(picked) = pick_recent(conn, wanted, &mut outcome.tried)? {
        outcome.picked = Some(picked);
        return Ok(outcome);
    }
    if let Some(picked) = pick_verified_recipe(conn, wanted, &mut outcome.tried)? {
        outcome.picked = Some(picked);
        return Ok(outcome);
    }
    Ok(outcome)
}

/// 第 ① 档：同任务 + 同族的**最近一次成功**。
fn pick_recent(
    conn: &Connection,
    wanted: &Wanted,
    tried: &mut Vec<String>,
) -> rusqlite::Result<Option<Picked>> {
    let mut stmt = conn.prepare(
        "SELECT prompt_id, title, raw, created_at FROM capture_runs \
          WHERE status = 'success' AND raw IS NOT NULL AND TRIM(raw) <> '' \
            AND prompt_id IS NOT NULL \
          ORDER BY created_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![SCAN_RECENT as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;

    let mut scanned = 0usize;
    for row in rows {
        let (prompt_id, title, raw, created_at) = row?;
        scanned += 1;
        let Some(graph) = graph_of_raw(&raw) else {
            continue;
        };
        let recipe = analyze(&graph, None);
        if let Some(want) = wanted.task {
            if recipe.task != want {
                continue;
            }
        }
        if let Some(family) = wanted.family.as_deref() {
            let hit = recipe
                .slots
                .iter()
                .any(|slot| family_matches(&slot.file_name, family));
            if !hit {
                continue;
            }
        }
        let label = title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("prompt {prompt_id}"));
        let at = created_at.unwrap_or_default();
        tried.push(format!(
            "① 同任务同族的最近成功：命中「{label}」（{at}）"
        ));
        return Ok(Some(Picked {
            prompt_id,
            source_kind: "recent",
            recipe_id: None,
            overrides: Vec::new(),
            note: format!("沿用同任务同族最近一次成功的那份工作流：{label}"),
        }));
    }
    tried.push(format!(
        "① 同任务同族的最近成功：扫了 {scanned} 条，没有命中的"
    ));
    Ok(None)
}

/// 第 ② 档：配方的**已验证绑定**（`verified_json` 里有这一族的模型）。
fn pick_verified_recipe(
    conn: &Connection,
    wanted: &Wanted,
    tried: &mut Vec<String>,
) -> rusqlite::Result<Option<Picked>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, task, prompt_id, verified_json FROM workflow_recipes \
          WHERE prompt_id IS NOT NULL AND verified_json IS NOT NULL AND TRIM(verified_json) <> '[]' \
          ORDER BY updated_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![SCAN_RECIPES as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        ))
    })?;

    let mut scanned = 0usize;
    for row in rows {
        let (id, name, task, prompt_id, verified_json) = row?;
        scanned += 1;
        if let Some(want) = wanted.task {
            if task != want.wire() {
                continue;
            }
        }
        let verified = parse_verified(&verified_json);
        // ⚠ **同族只看"主模型"那一格**：一次跑通常同时记了底模 / 文本编码器 / VAE 几条，
        // 而族名（anima / krea2 / moss）只出现在主模型的文件名里。按"任一条命中就把
        // 同一次跑的那几条全覆盖"来取 —— 否则换族重绑时会把文本编码器漏掉（静默跑错）。
        let Some(anchor) = verified.iter().find(|entry| match wanted.family.as_deref() {
            Some(family) => family_matches(&entry.model, family),
            None => true,
        }) else {
            continue;
        };
        let hit_run = anchor.run_key.clone();
        let overrides: Vec<(String, String)> = if hit_run.trim().is_empty() {
            // 老记录没有 runKey：只能带上命中的那一条（如实少带，不猜）
            vec![(anchor.override_key.clone(), anchor.model.clone())]
        } else {
            verified
                .iter()
                .filter(|entry| entry.run_key == hit_run)
                .map(|entry| (entry.override_key.clone(), entry.model.clone()))
                .collect()
        };
        tried.push(format!(
            "② 已验证的配方：「{name}」有这一族跑通过的记录（{} 个槽位）",
            overrides.len()
        ));
        return Ok(Some(Picked {
            prompt_id,
            source_kind: "recipe",
            recipe_id: Some(id),
            note: format!(
                "用了配方「{name}」里**已经跑通过**的那组槽位覆盖（{} 个）",
                overrides.len()
            ),
            overrides,
        }));
    }
    tried.push(format!(
        "② 已验证的配方：扫了 {scanned} 条，没有这一族跑通过的记录"
    ));
    Ok(None)
}

/// 库里的 `verified_json` 文本 → 验证记录（坏 JSON 当空，不连累整个选择）。
fn parse_verified(raw: &str) -> Vec<Verified> {
    let text = raw.trim();
    if text.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => crate::workflow::workflow_recipe::verified_of(&value),
        Err(_) => Vec::new(),
    }
}

/// 正面提示词最可能落在哪个键上（`positivePrompt` 那条路走不通时的**退一步**选择）。
///
/// 只挑**字面量**旋钮：`analyze` 的 knobs 只收字面量值，被连线的 `text` 根本不在里面 ——
/// 所以"写进这个键"至少不会被一根连线盖掉。优先 `CLIPTextEncode.text`（正面编码器的常规写法），
/// 其次任何提示词类旋钮；**一个都没有就如实回 `None`**（调用方不许自己编键名）。
pub fn prompt_override_key(graph: &Value) -> Option<String> {
    let obj = graph.as_object()?;
    let recipe = analyze(obj, None);
    let mut fallback: Option<String> = None;
    for knob in recipe
        .knobs
        .iter()
        .filter(|k| k.kind == crate::workflow::workflow_recipe::KnobKind::Prompt)
    {
        if knob.class_type.contains("CLIPTextEncode") && knob.input_name == "text" {
            return Some(knob.override_key());
        }
        if fallback.is_none() {
            fallback = Some(knob.override_key());
        }
    }
    fallback
}

/// 把"想要多大"写进目标工作流的宽高键。
///
/// **默认一个都不改**：构图与分辨率是**工作流自己的属性**（anima 的 1368×960、krea2 的
/// 1184×1776 不是一个东西），让手机随手改宽高只会把构图弄坏。只有调用方**明确给了**
/// 宽高才写，而且按工作流自己的键名写（`width` / `height`；认不出就不写并如实说）。
pub fn size_overrides(
    graph: &Value,
    width: Option<i64>,
    height: Option<i64>,
) -> (Vec<(String, String)>, Option<String>) {
    if width.is_none() && height.is_none() {
        return (Vec::new(), None);
    }
    let Some(obj) = graph.as_object() else {
        return (Vec::new(), None);
    };
    let recipe = analyze(obj, None);
    let key_of = |name: &str| -> Option<String> {
        recipe
            .knobs
            .iter()
            .find(|knob| knob.input_name == name)
            .map(|knob| knob.override_key())
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    match (width, key_of("width")) {
        (Some(w), Some(key)) => out.push((key, w.to_string())),
        (Some(_), None) => missing.push("width"),
        _ => {}
    }
    match (height, key_of("height")) {
        (Some(h), Some(key)) => out.push((key, h.to_string())),
        (Some(_), None) => missing.push("height"),
        _ => {}
    }
    if missing.is_empty() {
        (out, None)
    } else {
        let note = format!(
            "这份工作流没有可覆盖的 {}：构图沿用它自己的分辨率",
            missing.join(" / ")
        );
        (out, Some(note))
    }
}

#[cfg(test)]
mod tests;
