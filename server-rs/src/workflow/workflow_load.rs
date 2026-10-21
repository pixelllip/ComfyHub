//! 把**本机的一份工作流文件**读进库（Kotlin `AiWorkflowSearch.loadWorkflowFromFile` 的 Rust 端对照，
//! P4 第 44 步）。
//!
//! 用户 bug ③ 的正面解法：用户甩过来一个 `.json` 路径，别再回答"我只能提交库里的 promptId"。
//! 流程：读文件 → 认格式（API 原样用 / 界面格式按 `/object_info` 转）→ 入库拿到 `promptId`
//! → 返回节点摘要与**缺口清单**。
//!
//! # 幂等靠"文件内容"（不是路径）
//!
//! `run_key = file:<sha256>` 与 `GET /api/capture/workflows` 的 `inLibrary` 判定
//! 是**同一把钥匙**，所以同一份文件重复加载只有一条提示词。
//!
//! # 一处**只在真机上才出现**的坑（Kotlin 那边踩过，这里是照抄它的解法）
//!
//! `run_key` 是文件内容算出来的，而**用户可以在界面里把那条提示词删掉** ——
//! 删除会把 `capture_runs.prompt_id` 置空，于是这份文件再也导不进来，
//! 只会永远回一句"正在被另一次调用加载"。所以"抢不到 + 那条记录是 `success` 但
//! `prompt_id` 已经没了"这一档要**带 force 重抢**；正在处理中（`running`）的仍然让开。
//!
//! # 两处能力（P4 第 68 步起都在，见 [`crate::workflow::workflow_convert`] 文件头）
//!
//! 1. **组节点（subgraph）按定义展开**成内部真节点；
//! 2. **「模拟用户行为」（`emulateFrontend`）**（[`crate::workflow::workflow_frontend`]）：
//!    广播型节点按声明的类型 / 名字填空、显示节点丢掉、一个输出节点都没有时补 `SaveImage`。
//!    替前端做了什么**如实列在结果里**（`frontendEmulated` / `frontendBroadcast` /
//!    `frontendInjected` + 一句 `frontendHint`）—— 用户看得到才敢信这次跑的就是他工作流里的意思。
//!    `emulateFrontend=false` 时一个字都不改，纯前端节点照旧进 `unsupportedNodes`。

use std::path::Path;

use rusqlite::params;
use serde_json::{json, Map, Value};

use crate::comfy::capture_ingest;
use crate::web::routes::AppState;
use crate::workflow::workflow_convert;
// ⚠ 判据只有一份（`workflow_convert::is_node_graph`，与学习链路共用）——
//   以前这里抄过一份，两处一旦分叉就是"某一条路认、另一条不认"，而那种分叉不报错。
use crate::workflow::workflow_convert::is_node_graph;

/// 来源标记（Kotlin `AiWorkflowSearch.SOURCE`）。
pub const SOURCE: &str = "ComfyUI-File";
/// 工作流文件大小上限（Kotlin `MAX_WORKFLOW_FILE_BYTES`，32MB）。
pub const MAX_WORKFLOW_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SUMMARY_NODES: usize = 120;
const MAX_UNSUPPORTED: usize = 20;
const MAX_GAPS: usize = 40;
const MAX_REWRITTEN: usize = 12;
const MAX_WARNINGS: usize = 5;

/// 工具失败（码 + 人话；与 `tools::ToolFailure` 同形，放在这里免得模块互相依赖）。
#[derive(Debug, Clone)]
pub struct LoadFailure {
    pub code: String,
    pub message: String,
}

impl LoadFailure {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

/// 把一份工作流文件读进库，返回给模型/界面看的那份 JSON。
pub async fn load_from_file(
    state: &AppState,
    file: &Path,
    title: Option<&str>,
    include_graph: bool,
    tolerate_unsupported: bool,
    emulate_frontend: bool,
) -> Result<Value, LoadFailure> {
    if !file.is_file() {
        return Err(LoadFailure::new("NOT_A_FILE", format!("不是文件：{}", file.display())));
    }
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(LoadFailure::new("EMPTY_FILE", format!("文件是空的：{}", file.display())));
    }
    if size > MAX_WORKFLOW_FILE_BYTES {
        return Err(LoadFailure::new(
            "TOO_LARGE",
            format!("工作流文件太大（{size} 字节 > {MAX_WORKFLOW_FILE_BYTES}）：{}", file.display()),
        ));
    }
    let bytes = std::fs::read(file)
        .map_err(|e| LoadFailure::new("NOT_A_FILE", format!("读不了文件（{}）：{e}", file.display())))?;
    let root: Map<String, Value> = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .ok_or_else(|| {
            LoadFailure::new(
                "NOT_A_WORKFLOW",
                format!(
                    "这份文件不是 ComfyUI 工作流 JSON（解析失败）：{}。\
                     需要 ComfyUI 保存的工作流文件（含 nodes/links），或「导出（API）」得到的节点图。",
                    file.display()
                ),
            )
        })?;

    let api_format = is_node_graph(&root);
    let ui_format = !api_format && workflow_convert::is_ui_workflow(&root);
    if !api_format && !ui_format {
        // ⚠ 这条提示在 2026-09-27 的活实例测试里救过一次场：AI 自己搭的工作流**第一次被拒**，
        //   原因是它在顶层与节点里加了 `_comment` / `_meta` 这类"给人看"的键 ——
        //   API 格式的判据是"每个值都是带 class_type 的对象"，一个字符串就把整份图判死。
        //   它自己改成纯净格式就好了，但那是**多花一轮**；把原因直接写进错误里就省下那一轮。
        let hint = api_format_hint(&root);
        return Err(LoadFailure::new(
            "NOT_A_WORKFLOW",
            format!(
                "这份 JSON 既不是 API 格式节点图，也不是界面格式工作流（没有 nodes / links）：{}{hint}",
                file.display()
            ),
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut rewritten: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut open_inputs: Vec<String> = Vec::new();
    let mut frontend_rewritten: Vec<String> = Vec::new();
    let mut broadcast_lines: Vec<String> = Vec::new();
    let mut injected_nodes: Vec<String> = Vec::new();

    let api_graph: Map<String, Value> = if api_format {
        root.clone()
    } else {
        // 界面格式的转换**必须**问 ComfyUI 要节点定义（widgets_values 只有位置没有名字）。
        //
        // ⚠ 这里必须走 `object_info_of`（带进程内缓存）：`comfy_load_workflow` 是"每用一次
        // 工作流就调一次"的工具，直连 `/object_info` 等于每次都重新拉 4~15MB + 重新解析，
        // 典型体感是"读个工作流要等半秒到两秒"。缓存键是 ComfyUI 地址，换实例天然失效。
        let object_info = crate::workflow::workflow_recipe::object_info_of(state)
            .await
            .ok_or_else(|| {
                LoadFailure::new(
                    "COMFY_UNREACHABLE",
                    "这是界面格式工作流，转换需要问 ComfyUI 要节点定义（/object_info），但连不上。\
                     先 comfy_get_status 看 ComfyUI 在不在跑。"
                        .to_string(),
                )
            })?;
        let converted = workflow_convert::to_api_graph(&root, object_info.as_ref(), emulate_frontend)
            .map_err(|message| {
                LoadFailure::new("INVALID_ARGUMENT", format!("转换工作流失败：{message}"))
            })?;
        if !converted.unsupported_nodes.is_empty() && !tolerate_unsupported {
            // ⚠ 这四条出口是 `AiWorkflowSearch.loadWorkflowFromFile` 里那段的**逐字**搬运：
            //   第 68 步之后 ① 是真的（广播型节点会被照前端的行为等价改写），
            //   组节点也不再需要"只能走②或③"那句话了。
            return Err(LoadFailure::new(
                "UNSUPPORTED_NODES",
                format!(
                    "这条工作流用了 ComfyUI **前端**才有的节点，服务端没法等价转换：{}。四个出口：\
                     ① 把 emulateFrontend=true（默认就是）确认开着 —— 广播型节点（Anything Everywhere \
                     这类）会被照前端的行为等价改写；\
                     ② 把 tolerateUnsupported=true 再来一次 —— 我会把这些节点摘掉、\
                     并把**缺哪些线**列出来（openInputs / unresolvedInputs），\
                     你照着用 comfy_submit(connections=…) 补上就能跑；\
                     ③ 在 ComfyUI 里用「工作流菜单 → 导出（API）」存成 API 格式，再把那份文件路径给我\
                     （API 格式是原样提交，零转换）；\
                     ④ 在 ComfyUI 里点一次 Queue 让它跑一次，跑完会被自动捕获，之后就能按 promptId 提交。",
                    converted
                        .unsupported_nodes
                        .join("、")
                        .chars()
                        .take(400)
                        .collect::<String>()
                ),
            ));
        }
        if converted.graph.is_empty() {
            return Err(LoadFailure::new(
                "NO_API_GRAPH",
                format!("转换之后一个可提交的节点都没有：{}", file.display()),
            ));
        }
        warnings = converted.warnings.iter().take(MAX_WARNINGS).cloned().collect();
        rewritten = converted.rewritten.iter().take(MAX_REWRITTEN).cloned().collect();
        unsupported = converted.unsupported_nodes.iter().take(MAX_UNSUPPORTED).cloned().collect();
        unresolved = converted.unresolved_inputs.iter().take(MAX_GAPS).cloned().collect();
        open_inputs = converted.open_inputs.iter().take(MAX_GAPS).cloned().collect();
        frontend_rewritten = converted.frontend_rewritten.iter().take(MAX_REWRITTEN).cloned().collect();
        broadcast_lines = converted.broadcast_lines.iter().take(MAX_GAPS).cloned().collect();
        injected_nodes = converted.injected_nodes.iter().take(MAX_REWRITTEN).cloned().collect();
        converted.graph
    };

    // ---- 入库（幂等靠 file:<sha256>）----------------------------------------
    let sha = sha256_hex(&bytes);
    let run_key = format!("file:{sha}");
    let raw = json!({ "prompt": Value::Object(api_graph.clone()) });
    let claim = capture_ingest::begin_run(state, &run_key, SOURCE, Some(&raw), false)
        .map_err(|e| LoadFailure::new("CONFIG_ERROR", e.human()))?;
    let retried = if !claim.claimed
        && claim.existing_prompt_id.is_none()
        && claim.existing_status.as_deref() == Some("success")
    {
        // 库里的提示词被用户删掉了（`prompt_id` 被置空）→ 这份文件再也不该永远说"正在加载"
        capture_ingest::begin_run(state, &run_key, SOURCE, Some(&raw), true)
            .map_err(|e| LoadFailure::new("CONFIG_ERROR", e.human()))?
    } else {
        claim
    };

    let prompt_id = if !retried.claimed {
        retried.existing_prompt_id.ok_or_else(|| {
            LoadFailure::new(
                "LOAD_IN_PROGRESS",
                "这份工作流正在被另一次调用加载，稍等一下再试（或直接用 comfy_find_workflow 找它）",
            )
        })?
    } else {
        let parsed = crate::workflow::graph_parse::parse(Some(&api_graph));
        let label = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(200).collect::<String>())
            .unwrap_or_else(|| {
                file.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "工作流".to_string())
                    .chars()
                    .take(200)
                    .collect()
            });
        let workflow_json = Value::Object(if ui_format { root.clone() } else { api_graph.clone() })
            .to_string();
        let notes = format!(
            "从本机工作流文件加载：{}{}",
            file.display(),
            if ui_format {
                "（界面格式，已按 ComfyUI /object_info 转成 API 节点图）"
            } else {
                "（API 格式，原样使用）"
            }
        );
        let tags = vec![SOURCE.to_string()];
        let created = state
            .write(|tx| {
                capture_ingest::create_captured_prompt(
                    tx,
                    &parsed,
                    &label,
                    SOURCE,
                    Some(&run_key),
                    Some(&workflow_json),
                    &tags,
                    Some(&notes),
                )
            })
            .map_err(|e| LoadFailure::new("CONFIG_ERROR", e.human()))?;
        capture_ingest::finish_run(state, &run_key, Some(created), "success", 0, Some(&label), None)
            .map_err(|e| LoadFailure::new("CONFIG_ERROR", e.human()))?;
        created
    };

    let stored_title: Option<String> = state
        .with(|conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row("SELECT title FROM prompts WHERE id = ?1", params![prompt_id], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten())
        })
        .map_err(|e| LoadFailure::new("CONFIG_ERROR", e.human()))?;

    let runnable = unsupported.is_empty() && unresolved.is_empty();
    let mut out = Map::new();
    out.insert("promptId".into(), json!(prompt_id));
    out.insert("runKey".into(), json!(run_key));
    out.insert(
        "title".into(),
        json!(stored_title.unwrap_or_else(|| {
            title
                .map(str::to_string)
                .unwrap_or_else(|| file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
        })),
    );
    out.insert("path".into(), json!(file.display().to_string()));
    out.insert("format".into(), json!(if ui_format { "ui" } else { "api" }));
    out.insert("nodeCount".into(), json!(api_graph.len()));
    // 「能不能直接跑」是模型最需要知道的一件事：缺口的图提交上去 ComfyUI 只会报缺输入
    out.insert("runnable".into(), json!(runnable));
    out.insert(
        "hint".into(),
        json!(if runnable {
            format!(
                "现在可以 comfy_submit(promptId={prompt_id}, overrides={{\"<节点id>.<输入名>\": 值}}) 提交它；\
                 参数路径见下面的 nodes（linked 里的输入是连线，不能覆盖）。"
            )
        } else {
            format!(
                "**这条现在还不能跑**：转换时摘掉了 {} 个 ComfyUI 前端节点，\
                 留下的缺口见 openInputs / unresolvedInputs。请照着缺口用 \
                 comfy_submit(promptId={prompt_id}, connections={{\"<节点id>.<输入名>\": \
                 [\"<上游节点id>\", 槽位]}}) 把线补上再提交（要改的字面量仍走 overrides）。\
                 不确定某条线该接哪时，先问用户，别乱接。{}",
                unsupported.len(),
                if broadcast_lines.is_empty() {
                    String::new()
                } else {
                    "（其中一部分已经被「模拟用户行为」等价改写掉了，见 frontendBroadcast）"
                        .to_string()
                }
            )
        }),
    );
    if !rewritten.is_empty() {
        out.insert("rewrittenNodes".into(), json!(rewritten));
    }
    if !unsupported.is_empty() {
        out.insert("unsupportedNodes".into(), json!(unsupported));
    }
    if !unresolved.is_empty() {
        out.insert("unresolvedInputs".into(), json!(unresolved));
    }
    if !open_inputs.is_empty() {
        out.insert("openInputs".into(), json!(open_inputs));
    }
    if !warnings.is_empty() {
        out.insert("warnings".into(), json!(warnings));
    }
    // ## 模拟用户行为 ##：把"替前端做了哪几件事"如实列出来 ——
    // 用户看得到才敢信这次跑的就是他工作流里的意思（用户建议 ②）
    if !frontend_rewritten.is_empty() {
        out.insert("frontendEmulated".into(), json!(frontend_rewritten));
    }
    if !broadcast_lines.is_empty() {
        out.insert("frontendBroadcast".into(), json!(broadcast_lines));
    }
    if !injected_nodes.is_empty() {
        out.insert("frontendInjected".into(), json!(injected_nodes));
    }
    if !frontend_rewritten.is_empty() || !broadcast_lines.is_empty() || !injected_nodes.is_empty() {
        out.insert(
            "frontendHint".into(),
            json!(
                "这条工作流里有 ComfyUI **前端**才有的节点（服务端拿不到那份 JS）。\
                 已经照前端排队前的行为**等价改写**：见 frontendEmulated / frontendBroadcast / \
                 frontendInjected。提交前请扫一眼这些改动是不是你要的；\
                 不想让它替你做，就带 emulateFrontend=false 重新读一次（那时会如实报缺口）。"
            ),
        );
    }
    out.insert("nodes".into(), summarize_nodes(&api_graph));
    if include_graph {
        out.insert("apiGraph".into(), Value::Object(api_graph));
    }
    Ok(Value::Object(out))
}

/// 给 `NOT_A_WORKFLOW` 那条错误凑一句"**到底哪个顶层键不像节点**"的提示。
///
/// 为什么要有它（2026-09-27 活实例实测）：让 AI 自己搭一份工作流时，它**第一次写的那份被拒了** ——
/// 因为它在顶层加了 `_comment`、在节点里也塞了注释键。API 格式的判据是"每个值都是带 `class_type`
/// 的对象"，**一个字符串就把整份图判死**。它自己改成纯净格式就好了，但那是**白花一轮**；
/// 把原因与改法直接写进错误里，模型下一轮就能一次写对。
fn api_format_hint(root: &Map<String, Value>) -> String {
    let offenders: Vec<String> = root
        .iter()
        .filter(|(_, v)| v.as_object().map(|o| !o.contains_key("class_type")).unwrap_or(true))
        .map(|(k, _)| k.clone())
        .take(5)
        .collect();
    if offenders.is_empty() {
        return String::new();
    }
    format!(
        "。⚠ 顶层这些键不是合法节点（值不是带 `class_type` 的对象）：{} —— \
         自己写的 API 图要**纯净**：顶层只有节点 id，每个节点只有 `class_type` 与 `inputs`，\
         把 `_comment` / `_meta` / `title` 这类注释键删掉再存一次",
        offenders.join(", ")
    )
}

/// 给模型看的节点摘要：`id` / 类型 / 标题 / 可覆盖的控件名 / 已连线的输入名。
///
/// 为什么不直接把整张图丢过去：113 个节点的 API 图有 100+ KB，一次工具结果上限只有
/// `ToolRegistry.MAX_RESULT_CHARS`（32K 字符），模型会看到半个 JSON —— 那比一张
/// "哪个节点能改什么"的表更难用。
fn summarize_nodes(api_graph: &Map<String, Value>) -> Value {
    let mut items: Vec<Value> = Vec::new();
    for (id, value) in api_graph.iter().take(MAX_SUMMARY_NODES) {
        let Some(node) = value.as_object() else { continue };
        let mut widgets: Vec<String> = Vec::new();
        let mut linked: Vec<String> = Vec::new();
        if let Some(inputs) = node.get("inputs").and_then(Value::as_object) {
            for (name, v) in inputs {
                if v.is_array() {
                    linked.push(name.clone());
                } else {
                    widgets.push(name.clone());
                }
            }
        }
        let mut entry = Map::new();
        entry.insert("id".into(), json!(id));
        entry.insert(
            "classType".into(),
            json!(node.get("class_type").and_then(Value::as_str).unwrap_or_default()),
        );
        if let Some(title) = node
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("title"))
            .and_then(Value::as_str)
        {
            entry.insert("title".into(), json!(title));
        }
        entry.insert("widgets".into(), json!(widgets));
        entry.insert("linked".into(), json!(linked));
        items.push(Value::Object(entry));
    }
    Value::Array(items)
}

/// 文件内容的 sha256（与 `GET /api/capture/workflows` 的 `inLibrary` 判定同一把钥匙）。
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
