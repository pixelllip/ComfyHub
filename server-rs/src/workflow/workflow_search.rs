//! `comfy_find_workflow`：找一个**能直接跑的工作流**（`AiWorkflowSearch.search` +
//! `ToolRegistry.kt` 里那条工具处理体的逐条对照）。
//!
//! # 它要解决什么
//!
//! 用户 bug ⑤：`prompts` 库**只收"捕获过的运行"与"手动读进来的文件"**，
//! 所以**首次使用时库必然是空的** —— 而用户机器上 `user\<用户>\workflows\*.json` 早就存在。
//! 于是"库里搜不到"绝不能回一句"库里没有"就收摊：模型据此会跟用户说
//! "得先在 ComfyUI 里跑一次才会被捕获"，而那份工作流一直躺在磁盘上。
//!
//! 所以这条路是**两段**：先搜库；库里 0 条时**自动回头看本机的工作流文件**
//! （`localWorkflows` + `dirs` + 一段 hint 说明"拿到 path 直接 `comfy_load_workflow`"）。
//!
//! # 两条纪律
//!
//! 1. **`runnable` 如实标**：只有拿得到 API 节点图（`capture_runs.raw` 里那份）才能直接提交；
//!    老记录 / 只导入了图片的**如实说跑不了**，绝不硬编一张图（R10 的同一个道理）。
//! 2. **"库里没有" ≠ "这份工作流用不了"**：第二段就是为了这条（AGENTS §10）。
//!
//! # 与 Kotlin 的一处形状细节
//!
//! Kotlin 是用 `buildJsonObject { put("negativePrompt", p.negativePrompt?.take(200)) }` 手拼的，
//! 而 `JsonObjectBuilder.put(key, String?)` 遇到 null 会写 **`JsonNull`**（`explicitNulls=false`
//! 只管 `@Serializable` 的 DTO，管不到手拼的对象）。所以这里也**照写 null 键** ——
//! 少一个键与多一个 null 键，模型看到的形状不一样，而"形状逐条对照"是这个仓库的纪律。

use serde_json::{json, Map, Value};

use crate::web::routes::AppState;
use crate::tools::tools::{ToolFailure, ToolOutcome};

/// 工具结果是**截断**过的（`ToolRegistry.MAX_RESULT_CHARS`）：每一类留多少条。
const MAX_LOCAL_FILES: usize = 8;

/// 按关键词找库里能跑的工作流 → `{count, query, workflows:[…]}`。
///
/// 为什么不直接返回界面上那份 `workflow_json`：那是**界面格式**，`/prompt` 不接受。
/// 提交要用的是 `capture_runs.raw` 里那份 API 节点图，所以这里把 `runnable` 如实标出来。
pub fn search_json(
    state: &AppState,
    query: &str,
    limit: i64,
    include_graph: bool,
) -> Result<Value, ToolFailure> {
    let items = crate::web::routes::query_prompts(
        state,
        &crate::web::routes::PromptQuery {
            q: Some(query.to_string()),
            kind: None,
            favorite: None,
            sort: Some("newest".to_string()),
            page: Some(1),
            size: Some(limit),
            // ⚠ **AI 侧要连自动捕获的一起搜**：`comfy_find_workflow` 找的是"库里能跑的
            // 工作流"，而那些正是自动捕获来的 —— 不传 true 就等于把它们藏起来，
            // 表现是"AI 说库里没有，用户却在画廊里看得到"（见 PromptQuery 的注释）。
            include_captured: true,
            // 不按"有没有关联产物"筛（那是界面上的入口）
            has_media: None,
        },
    )
    .map_err(|e| ToolFailure::new("LIBRARY_ERROR", e.human()))?
    .items;

    // 可空键**照写 null**（见文件头那条形状说明）
    let mut workflows: Vec<Value> = Vec::with_capacity(items.len());
    for p in &items {
        // ⚠ **有意与 Kotlin 不同**（2026-09-27）：
        // Kotlin 写的是 `val graph = if (includeGraph) apiGraphOf(p.id) else null`，然后
        // `runnable = graph != null` —— 于是**默认档（includeGraph=false）下每一条都报
        // `runnable: false` + "这条没有 API 节点图（老数据 / 只导入了图片），不能直接提交"**。
        // 那是**假话**（库里大多数提示词都有 API 图），而代价正是这个仓库最怕的那种：
        // AI 会据此告诉用户"这些工作流都不能提交"。所以这里**恒查一次图**来定 `runnable`，
        // 只在 `includeGraph=true` 时才把图本身附上（图的体积才是当初想省的东西）。
        let graph = crate::comfy::capture::prompt_api_graph(state, p.id).ok().flatten();
        let runnable = graph.is_some();

        let mut o = Map::new();
        o.insert("promptId".into(), json!(p.id));
        o.insert("title".into(), json!(p.title));
        o.insert("kind".into(), json!(p.kind));
        o.insert("positivePrompt".into(), json!(take_chars(&p.positive_prompt, 400)));
        o.insert(
            "negativePrompt".into(),
            opt_str(p.negative_prompt.as_deref().map(|s| take_chars(s, 200))),
        );
        o.insert("checkpoint".into(), opt_str(p.checkpoint.clone()));
        o.insert("sampler".into(), opt_str(p.sampler.clone()));
        o.insert("scheduler".into(), opt_str(p.scheduler.clone()));
        o.insert("steps".into(), p.steps.map(|v| json!(v)).unwrap_or(Value::Null));
        o.insert("cfgScale".into(), p.cfg_scale.map(|v| json!(v)).unwrap_or(Value::Null));
        o.insert("seed".into(), p.seed.map(|v| json!(v)).unwrap_or(Value::Null));
        o.insert("width".into(), p.width.map(|v| json!(v)).unwrap_or(Value::Null));
        o.insert("height".into(), p.height.map(|v| json!(v)).unwrap_or(Value::Null));
        o.insert("source".into(), opt_str(p.source.clone()));
        o.insert("hasMedia".into(), json!(p.media_count > 0));
        o.insert("runnable".into(), json!(runnable));
        if include_graph {
            if let Some(g) = &graph {
                o.insert("apiGraph".into(), g.clone());
            }
        }
        if !runnable {
            o.insert(
                "note".into(),
                json!(
                    "这条没有 API 节点图（老数据 / 只导入了图片），不能直接提交；\
                     可以在 ComfyUI 里跑一次让它被捕获，或者换一条 runnable=true 的"
                ),
            );
        }
        workflows.push(Value::Object(o));
    }

    Ok(json!({
        "count": workflows.len(),
        "query": query,
        "workflows": workflows,
    }))
}

/// `comfy_find_workflow`：两段式查找（库 → 本机文件）。
pub fn tool_find_workflow(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let Some(query) = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少参数 query"));
    };
    let limit = arguments.get("limit").and_then(Value::as_i64).unwrap_or(5).clamp(1, 20);
    let include_graph = arguments
        .get("includeGraph")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let found = search_json(state, &query, limit, include_graph)?;
    if found["count"].as_i64().unwrap_or(0) != 0 {
        let text = found.to_string();
        return Ok(ToolOutcome::new(text, found));
    }

    // 库里没有 → **必须**回头看一眼本机（用户 bug ⑤：首次使用时库必然是空的）
    let local = crate::comfy::comfy_dirs::workflows_json(
        state,
        crate::comfy::comfy_dirs::WorkflowQuery { q: Some(query.clone()), limit: Some(MAX_LOCAL_FILES) },
    )
    .map_err(|e| ToolFailure::new("COMFY_DISABLED", e.human()))?;
    let local_count = local["count"].as_i64().unwrap_or(0);
    if local_count > 0 {
        let merged = json!({
            "count": 0,
            "query": query,
            "workflows": [],
            "localWorkflows": local.get("files").cloned().unwrap_or_else(|| json!([])),
            "dirs": local.get("dirs").cloned().unwrap_or_else(|| json!([])),
            "hint": format!(
                "库里（捕获过 / 读进来过的工作流）没有匹配「{query}」的，\
                 但**本机 ComfyUI 里存着上面这些工作流文件**。它们不需要先跑一次：\
                 直接 comfy_load_workflow(path=…, includeGraph=true) 读进库（拿 promptId + 节点/输入名），\
                 再 comfy_submit 提交。**不要跟用户说「得先跑一次才会被捕获」**。"
            ),
        });
        let text = merged.to_string();
        return Ok(ToolOutcome::new(text, merged));
    }

    Err(ToolFailure::new(
        "NOT_FOUND",
        format!(
            "库里没有匹配「{query}」的提示词，本机 ComfyUI 的 workflows 目录里也没有文件名匹配它的工作流。\
             可以先用更短的关键词再搜一次（例如只用模型名），或者不带关键词用 comfy_list_workflows \
             看本机到底存了哪些工作流；再不行就让用户直接把工作流 .json 的完整路径给过来。"
        ),
    ))
}

fn opt_str(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

/// 按**字符**截断（Kotlin 的 `take(n)` 数的是 UTF-16 单元，对 BMP 里的中文与 ASCII 等价）。
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;
