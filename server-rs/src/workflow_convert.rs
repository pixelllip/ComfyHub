//! ComfyUI **界面格式工作流**（`{nodes,links}`）→ **API 格式节点图**（Kotlin `WorkflowConvert` 的
//! Rust 端逐行对照，P4 第 44 步）。
//!
//! # 为什么必须有它（用户 bug ③）
//!
//! 用户指着本机一份工作流文件让 AI 去跑，而 `comfy_submit` 只认库里的 `promptId` ——
//! 那条工作流从来没被捕获过，于是整件事卡死在"我不能凭一个文件路径提交"。
//! 界面格式里 `widgets_values` **只有位置、没有参数名**，所以转换必须知道每个节点的输入
//! 声明顺序 —— 那份声明只有 ComfyUI 的 `/object_info` 有，因此转换是**在线**的
//! （提交前顺手问一次 ComfyUI），不是纯函数。
//!
//! # 等价改写 vs 如实报错（判据不能含糊）
//!
//! | 节点 / 状态 | 处理 |
//! | --- | --- |
//! | `Note` / `MarkdownNote` / `PrimitiveNode` | 丢掉（官方前端排队前也把它们剔除） |
//! | `Reroute` / `SetNode` / `GetNode` | 把线接到真正的上游（`GetNode` 按变量名找回 `SetNode`） |
//! | `mode=4`（旁路） | 按 ComfyUI 的语义**把输出接到同类型的已连接输入**上（真机一份 91 节点的存档里 61 个是旁路，直接拒会让功能对真实文件完全不可用） |
//! | `mode=2`（静音） | 不进提示词，**并如实警告下游会缺输入** |
//! | 别的 `mode` | `unsupported`（不猜） |
//! | 广播型纯前端节点（`Anything Everywhere` 这类） | **`unsupported`**（见下） |
//! | 组节点（subgraph） | **`unsupported`**（见下） |
//!
//! ## 两处**还没搬**的能力（必须看得见，不能假装）
//!
//! 1. **组节点（subgraph / `definitions.subgraphs`）不展开**：Kotlin 会按定义把它摊平成
//!    内部的真节点（重新编号、把外层控件值填进内部输入）。Rust 这一版**不猜**，
//!    直接把整个实例记进 `unsupportedNodes`（带一句"这一版不展开"）。
//!    表现是：带组节点的工作流会如实被拒（`UNSUPPORTED_NODES`），而不是提交一张错的图。
//! 2. **"模拟用户行为"（`emulateFrontend`）没实现**：Kotlin 会照前端 JS 排队前的行为
//!    把广播型节点等价改写掉（按 `Input Type` / 名字填空、丢掉显示节点、补 `SaveImage`）。
//!    这里同样**不猜**：那些节点留在 `unsupportedNodes` 里，由调用方决定
//!    （`tolerateUnsupported` 摘掉 + 给缺口清单）。
//!    ⚠ 工具描述是从 Kotlin 逐字搬的、里面写了这个能力 —— 所以调用方**必须在结果里
//!    如实说"本后端还没做这一步"**（`workflow_load.rs` 里那句 warning 就是干这个的）。
//!
//! # 缺口清单（宁可失败，但不卡死）
//!
//! `unresolvedInputs` = 指向"没转换出来的节点"的悬挂引用（提交上去 ComfyUI 只会报
//! "节点 xxx 不存在"，用户完全看不出哪一步坏的）；
//! `openInputs` = 现在空着的**连线型**输入（广播型节点原来就是往这些地方灌值的，
//! 所以它正好是"要补哪些线"的作业本）。

use std::collections::HashMap;

use serde_json::{json, Map, Value};

/// links 表：linkId → (上游节点 id, 槽位)。HashMap 就够（只查不遍历）。
type LinkTable = HashMap<i64, (String, i64)>;
/// 改写表：(节点id, 输出槽位) → 真正的上游。
type Redirects = HashMap<(String, i64), (String, i64)>;

/// 转换结果（Kotlin `WorkflowConvert.Result`）。
#[derive(Debug, Clone, Default)]
pub struct Converted {
    pub graph: Map<String, Value>,
    pub node_count: usize,
    /// 被等价改写 / 丢掉的界面专用节点，形如 `12 Note`
    pub rewritten: Vec<String>,
    pub warnings: Vec<String>,
    /// `/object_info` 里没有、也不是已知虚拟节点的类，形如 `37 Anything Everywhere`
    pub unsupported_nodes: Vec<String>,
    /// 图里还**悬着**的输入：形如 `60.vae ← 节点 1`
    pub unresolved_inputs: Vec<String>,
    /// 现在空着的连线型输入，形如 `60.vae (VAE)`
    pub open_inputs: Vec<String>,
}

impl Converted {
    /// 能不能直接跑（`unsupported` 与 `unresolved` 都空才算）。
    pub fn runnable(&self) -> bool {
        self.unsupported_nodes.is_empty() && self.unresolved_inputs.is_empty()
    }
}

/// litegraph 的节点状态：0 = 正常，2 = 静音（永不执行），4 = 旁路（接过去）
const MODE_MUTED: i64 = 2;
const MODE_BYPASS: i64 = 4;

/// 只由前端 JS 实现、后端没有节点定义的"虚拟节点"。
///
/// ⚠ 判定顺序：**先看 `/object_info` 里有没有**。有的版本把 `SetNode` 之类做成了真节点
/// （那时它就该原样保留），所以只有"查不到 + 名字在这张表里"才改写。
const FRONTEND_VIRTUAL: [&str; 6] =
    ["Note", "MarkdownNote", "PrimitiveNode", "Reroute", "SetNode", "GetNode"];

/// 会被当作"控件"（有 `widgets_values` 槽位）的类型。其余都是连线插口。
const WIDGET_TYPES: [&str; 5] = ["INT", "FLOAT", "STRING", "BOOLEAN", "COMBO"];

/// 这份 JSON 是不是界面格式工作流（带 `nodes` 数组）。
pub fn is_ui_workflow(obj: &Map<String, Value>) -> bool {
    obj.get("nodes").map(Value::is_array).unwrap_or(false)
}

/// 这份 JSON 是不是 **API 格式节点图**（非空，且**每个值**都是带 `class_type` 的对象）。
///
/// 判据只有这一条（Kotlin `HistoryEntry.isNodeGraph`）：**一个字符串键就把整份图判死** ——
/// 2026-09-27 活实例上踩过：AI 自己搭的工作流在顶层加了 `_comment`，于是被判成"不是工作流"。
///
/// 三处共用这一份（`workflow_load` / `workflow_recipe` / 提交链路的取图），
/// 各写一份的下场是"某一条路认、另一条不认"，而那种分叉不报错。
pub fn is_node_graph(obj: &Map<String, Value>) -> bool {
    !obj.is_empty()
        && obj
            .values()
            .all(|v| v.as_object().is_some_and(|o| o.contains_key("class_type")))
}

/// 「模拟用户行为」（`emulateFrontend`）**本后端还没实现**时，调用方必须补的那句话。
///
/// 为什么它有资格占一个公共函数：工具描述（`comfy_load_workflow` / `comfy_learn_workflow`）
/// 是从 Kotlin **逐字**搬的，里面写着"默认开着、会照前端行为等价改写" —— 不说这一句，
/// 用户会以为那些纯前端节点已经被照前端的做法接好了。这份文本只有一个真源，
/// 「欠账如实说」这条纪律才不会在第二次搬运时走样。
pub fn emulate_frontend_warning(emulate_frontend: bool, has_unsupported: bool) -> Option<String> {
    if emulate_frontend && has_unsupported {
        Some(
            "⚠ 本后端**还没实现**「模拟用户行为」（emulateFrontend）：上面那些纯前端节点\
             没有被照 ComfyUI 前端的行为等价改写（描述里写的那个能力这一步不做）。"
                .to_string(),
        )
    } else if !emulate_frontend {
        Some("（emulateFrontend=false：一个字都不改，只如实报缺口 —— 与描述一致）".to_string())
    } else {
        None
    }
}

/// 界面格式 → API 节点图（Kotlin `toApiGraph`，**不含**组节点展开与前端模拟）。
pub fn to_api_graph(
    ui: &Map<String, Value>,
    object_info: &Map<String, Value>,
) -> Result<Converted, String> {
    let (raw_nodes, links, handled, mut rewritten, mut unsupported, mut warnings) =
        flatten_top_level(ui, object_info);
    if raw_nodes.is_empty() {
        return Err("工作流里一个节点都没有（nodes 是空的）".to_string());
    }

    // 改写表：(节点id, 输出槽位) → 真正的上游 (节点id, 槽位)
    let mut redirects: Redirects = HashMap::new();

    // --- 第一遍：登记所有可等价改写的界面节点（必须在解析连线之前全部登记完）---
    let mut set_by_name: Vec<(String, (String, i64))> = Vec::new();
    for node in &raw_nodes {
        let class = ui_class(node);
        if class == "SetNode" && object_info.get(&class).is_none() {
            let source = first_linked_input(node, &links).map(|r| resolve(r, &redirects));
            let name = widget_string(node);
            if let (Some(source), Some(name)) = (source, name) {
                if !set_by_name.iter().any(|(existing, _)| existing == &name) {
                    set_by_name.push((name, source));
                }
            }
        }
    }
    for node in &raw_nodes {
        let class = ui_class(node);
        let Some(id) = node_id(node) else { continue };
        if class.is_empty() || object_info.get(&class).is_some() {
            continue;
        }
        match class.as_str() {
            "Reroute" | "SetNode" => {
                if let Some(source) = first_linked_input(node, &links) {
                    redirects.insert((id.clone(), 0), resolve(source, &redirects));
                }
                rewritten.push(format!("{id} {class}"));
            }
            "GetNode" => {
                let name = widget_string(node);
                let source = name
                    .as_deref()
                    .and_then(|n| set_by_name.iter().find(|(existing, _)| existing == n))
                    .map(|(_, source)| source.clone());
                match source {
                    Some(source) => {
                        redirects.insert((id.clone(), 0), resolve(source, &redirects));
                        rewritten.push(format!("{id} {class}"));
                    }
                    None => {
                        unsupported.push(format!(
                            "{id} {class}（找不到同名的 SetNode「{}」）",
                            name.unwrap_or_else(|| "?".to_string())
                        ));
                    }
                }
            }
            other if FRONTEND_VIRTUAL.contains(&other) => rewritten.push(format!("{id} {class}")),
            _ => {}
        }
    }
    // 旁路：按输出槽位登记"接到哪个同类型的已连接输入上"
    for node in &raw_nodes {
        if node_int(node, "mode").unwrap_or(0) != MODE_BYPASS {
            continue;
        }
        let Some(id) = node_id(node) else { continue };
        let class = ui_class(node);
        bypass_redirects(&id, node, &links, &mut redirects, &mut warnings);
        rewritten.push(format!("{id} {class}（旁路）"));
    }

    // --- 第二遍：真正转成节点 ---
    let redirected_ids: Vec<String> = redirects.keys().map(|(id, _)| id.clone()).collect();
    let mut open_inputs: Vec<String> = Vec::new();
    let mut out: Map<String, Value> = Map::new();
    for node in &raw_nodes {
        let Some(id) = node_id(node) else { continue };
        let class = ui_class(node);
        if class.is_empty() {
            continue;
        }
        let mode = node_int(node, "mode").unwrap_or(0);
        // 组节点实例：这一版不展开，**如实报**（见文件头那两条"还没搬的能力"）
        if handled.contains(&id) || redirected_ids.contains(&id) {
            continue;
        }
        if FRONTEND_VIRTUAL.contains(&class.as_str()) && object_info.get(&class).is_none() {
            continue;
        }
        if mode == MODE_MUTED {
            let consumers = consumers_of(&id, &raw_nodes, &links);
            warnings.push(format!(
                "节点 {id}（{class}）是静音的，它的输出不会产生{}",
                if consumers.is_empty() {
                    String::new()
                } else {
                    format!("，下游（{}）可能缺输入", consumers.join("、"))
                }
            ));
            continue;
        }
        if mode == MODE_BYPASS {
            continue;
        }
        if mode != 0 {
            unsupported.push(format!("{id} {class}（未知的节点状态 mode={mode}）"));
            continue;
        }
        let Some(info) = object_info.get(&class).and_then(Value::as_object) else {
            unsupported.push(format!("{id} {class}"));
            continue;
        };

        let mut inputs: Map<String, Value> = Map::new();
        // 1) 连线优先：界面格式里 `inputs[].link` 指向 links 表
        if let Some(list) = node.get("inputs").and_then(Value::as_array) {
            for entry in list {
                let Some(entry) = entry.as_object() else { continue };
                let Some(name) = entry.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty())
                else {
                    continue;
                };
                if inputs.contains_key(name) {
                    continue;
                }
                let Some(link_id) = as_int(entry.get("link")) else { continue };
                let Some(reference) = links.get(&link_id) else { continue };
                // 穿透 Reroute / Set-Get / 旁路的节点，拿到真正的产出节点
                let source = resolve(reference.clone(), &redirects);
                inputs.insert(name.to_string(), json!([ref_of(&source.0), source.1]));
            }
        }
        // 2) 控件值：按 /object_info 的声明顺序贴回名字上
        fill_widgets(node, &widget_specs(info), &mut inputs, &mut warnings, &id, &class);

        let mut node_out = Map::new();
        node_out.insert("class_type".into(), json!(class));
        node_out.insert("inputs".into(), Value::Object(inputs.clone()));
        if let Some(title) = node.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
            node_out.insert("_meta".into(), json!({ "title": title }));
        }
        out.insert(id.clone(), Value::Object(node_out));
        collect_open_inputs(info, &inputs, &id, &mut open_inputs);
    }

    let unresolved = dangling_inputs(&out);
    Ok(Converted {
        node_count: out.len(),
        graph: out,
        rewritten,
        warnings,
        unsupported_nodes: unsupported,
        unresolved_inputs: unresolved,
        open_inputs,
    })
}

/// 顶层摊平：**这一版只做"原样保留"**（组节点不展开，只记 `unsupported`）。
///
/// Kotlin 的 `flatten` 会把 `definitions.subgraphs` 里的定义展开成内部真节点
/// （重新编号、把外层控件值填进内部输入、把输出代理登记成改写）。这里**不猜**：
/// 遇到组节点实例就记一条"这一版不展开"，并把它放进 `handled`（第二遍跳过它）。
fn flatten_top_level(
    ui: &Map<String, Value>,
    object_info: &Map<String, Value>,
) -> (
    Vec<Map<String, Value>>,
    LinkTable,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let top_nodes: Vec<Map<String, Value>> = ui
        .get("nodes")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|v| v.as_object().cloned()).collect())
        .unwrap_or_default();
    let top_links = parse_raw_links(ui.get("links"));
    let links: LinkTable =
        top_links.iter().map(|(id, link)| (*id, (link.origin.clone(), link.origin_slot))).collect();

    let defs = subgraph_defs(ui);
    let mut handled: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    if !defs.is_empty() {
        for node in &top_nodes {
            let class = ui_class(node);
            if class.is_empty() || object_info.get(&class).is_some() || !defs.contains(&class) {
                continue;
            }
            let Some(id) = node_id(node) else { continue };
            let mode = node_int(node, "mode").unwrap_or(0);
            if mode != 0 {
                // 旁路 / 静音的组节点**不展开**（Kotlin 同：交给旁路那套逻辑按类型接过去）
                continue;
            }
            unsupported.push(format!(
                "{id} {}…（组节点：**本后端这一版还不展开 subgraph**，请改用「导出（API）」的节点图，\
                 或在 ComfyUI 里跑一次让它被自动捕获）",
                &class.chars().take(8).collect::<String>()
            ));
            handled.push(id);
        }
    }
    (top_nodes, links, handled, Vec::new(), unsupported, Vec::new())
}

/// `definitions.subgraphs[]` → `id` → 定义。
fn subgraph_defs(ui: &Map<String, Value>) -> Vec<String> {
    ui.get("definitions")
        .and_then(Value::as_object)
        .and_then(|defs| defs.get("subgraphs"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_object()?.get("id")?.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 界面格式里的一条连线。
struct RawLink {
    origin: String,
    origin_slot: i64,
}

/// `links` 表：两种写法都认（老写法是数组、新写法是对象）。
fn parse_raw_links(raw: Option<&Value>) -> HashMap<i64, RawLink> {
    let mut out: HashMap<i64, RawLink> = HashMap::new();
    let Some(list) = raw.and_then(Value::as_array) else { return out };
    for entry in list {
        match entry {
            // [link_id, origin_id, origin_slot, target_id, target_slot, type]
            Value::Array(items) => {
                let Some(link_id) = as_int(items.first()) else { continue };
                let Some(origin) = items.get(1).and_then(primitive_text) else { continue };
                out.insert(
                    link_id,
                    RawLink { origin, origin_slot: as_int(items.get(2)).unwrap_or(0) },
                );
            }
            // {id, origin_id, origin_slot, target_id, target_slot, …}
            Value::Object(obj) => {
                let Some(link_id) = as_int(obj.get("id")) else { continue };
                let Some(origin) = obj.get("origin_id").and_then(primitive_text) else { continue };
                out.insert(
                    link_id,
                    RawLink { origin, origin_slot: as_int(obj.get("origin_slot")).unwrap_or(0) },
                );
            }
            _ => {}
        }
    }
    out
}

/// 旁路的语义：这个节点不执行，但"信号"要穿过去 —— 每个输出接到**同类型的某个输入**上。
///
/// 规则与前端一致的地方是"同类型"；槽位对应先试**同名下标**的输入，不行再退到
/// "第一个同类型且有连线的输入"。一个输出找不到可对应的输入时**不静默放过**：
/// 那时下游会缺输入，如实写进警告。
fn bypass_redirects(
    id: &str,
    node: &Map<String, Value>,
    links: &LinkTable,
    redirects: &mut Redirects,
    warnings: &mut Vec<String>,
) {
    let outputs: Vec<&Map<String, Value>> = node
        .get("outputs")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let linked_inputs: Vec<(usize, Option<String>, (String, i64))> = node
        .get("inputs")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    let entry = entry.as_object()?;
                    let link_id = as_int(entry.get("link"))?;
                    let source = links.get(&link_id)?;
                    Some((index, entry.get("type").and_then(primitive_text), source.clone()))
                })
                .collect()
        })
        .unwrap_or_default();

    for (slot, output) in outputs.iter().enumerate() {
        let output_type = output.get("type").and_then(primitive_text);
        let same_slot = linked_inputs
            .iter()
            .find(|(index, input_type, _)| *index == slot && *input_type == output_type);
        let candidate = same_slot
            .or_else(|| linked_inputs.iter().find(|(_, input_type, _)| *input_type == output_type));
        match candidate {
            Some((_, _, source)) => {
                let resolved = resolve(source.clone(), redirects);
                redirects.insert((id.to_string(), slot as i64), resolved);
            }
            None => warnings.push(format!(
                "节点 {id}（{}）是旁路的，但输出「{}」（{}）没有同类型的已连接输入可以接过去 —— 下游可能缺输入",
                ui_class(node),
                output
                    .get("name")
                    .and_then(primitive_text)
                    .unwrap_or_else(|| slot.to_string()),
                output_type.unwrap_or_else(|| "?".to_string())
            )),
        }
    }
}

/// 谁引用了这个节点的输出（给"静音节点"的警告用）。
fn consumers_of(
    id: &str,
    nodes: &[Map<String, Value>],
    links: &LinkTable,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in nodes {
        let Some(inputs) = node.get("inputs").and_then(Value::as_array) else { continue };
        for entry in inputs {
            let Some(entry) = entry.as_object() else { continue };
            let Some(link_id) = as_int(entry.get("link")) else { continue };
            if links.get(&link_id).map(|(origin, _)| origin.as_str()) == Some(id) {
                let label = format!(
                    "{}（{}）",
                    node_id(node).unwrap_or_else(|| "?".to_string()),
                    ui_class(node)
                );
                if !out.contains(&label) {
                    out.push(label);
                }
            }
        }
    }
    out
}

/// 控件值 → 按 `/object_info` 的声明顺序贴回名字上（Kotlin `fillWidgets`）。
fn fill_widgets(
    node: &Map<String, Value>,
    specs: &[WidgetSpec],
    inputs: &mut Map<String, Value>,
    warnings: &mut Vec<String>,
    id: &str,
    class: &str,
) {
    if specs.is_empty() {
        return;
    }
    // 0) **首选带名字的那份**（`widgets_values_named`）：有它就不必按位置猜。
    //    只认 `/object_info` **声明过**的控件名（多出来的键会让 ComfyUI 拒掉整张图）。
    if let Some(named) = node.get("widgets_values_named").and_then(Value::as_object) {
        if !named.is_empty() {
            for (name, value) in named {
                if inputs.contains_key(name) || value.is_null() {
                    continue;
                }
                if specs.iter().any(|spec| &spec.name == name) {
                    inputs.insert(name.clone(), value.clone());
                } else {
                    warnings.push(format!(
                        "节点 {id}（{class}）的控件值里有 `{name}`，但节点的输入声明里没有它 —— 已忽略\
                         （多半是界面版本与后端节点版本不一致）"
                    ));
                }
            }
            return;
        }
    }
    match node.get("widgets_values") {
        // 少数自定义节点把控件存成对象：按名字取，最稳
        Some(Value::Object(values)) => {
            for spec in specs {
                if inputs.contains_key(&spec.name) {
                    continue;
                }
                if let Some(value) = values.get(&spec.name) {
                    if !value.is_null() {
                        inputs.insert(spec.name.clone(), value.clone());
                    }
                }
            }
        }
        Some(Value::Array(values)) => {
            // 已经被连线占掉的输入不重复写（ComfyUI 会当未知参数拒掉）
            let pending: Vec<&WidgetSpec> =
                specs.iter().filter(|spec| !inputs.contains_key(&spec.name)).collect();
            let expected = pending.len() + pending.iter().filter(|spec| spec.extra_slot).count();
            if values.len() != expected {
                warnings.push(format!(
                    "节点 {id}（{class}）的控件值有 {} 个，按输入声明应该是 {expected} 个\
                     （多半是自定义节点多了界面专用的控件）—— 参数可能对不齐，跑之前留意 ComfyUI 的报错",
                    values.len()
                ));
            }
            let mut index = 0usize;
            for spec in pending {
                let Some(value) = values.get(index) else { break };
                index += 1;
                if !value.is_null() {
                    inputs.insert(spec.name.clone(), value.clone());
                }
                // 种子后面跟的那个「control_after_generate」下拉框也占一个槽位
                if spec.extra_slot {
                    index += 1;
                }
            }
        }
        _ => {}
    }
}

struct WidgetSpec {
    name: String,
    extra_slot: bool,
}

/// 一个输入声明算不算"控件"（= 有 `widgets_values` 槽位）。
///
/// ⚠ `forceInput` 的例外很关键：它被**强制成连线插口**了，哪怕类型是 STRING 也没有控件槽位 ——
/// 把它当控件会让后面所有控件的取值整体错位。
fn is_widget_input(decl: &Value) -> bool {
    let Some(items) = decl.as_array() else { return false };
    if let Some(options) = items.get(1).and_then(Value::as_object) {
        if options.get("forceInput").and_then(Value::as_bool) == Some(true) {
            return false;
        }
    }
    match items.first() {
        Some(Value::Array(_)) => true,
        Some(Value::String(text)) => WIDGET_TYPES.contains(&text.to_uppercase().as_str()),
        _ => false,
    }
}

/// 一个节点的控件（= 有 `widgets_values` 槽位的输入）名字与顺序，来自 `/object_info`。
fn widget_specs(info: &Map<String, Value>) -> Vec<WidgetSpec> {
    let Some(input) = info.get("input").and_then(Value::as_object) else { return Vec::new() };
    let mut out: Vec<WidgetSpec> = Vec::new();
    for group in ["required", "optional"] {
        let Some(map) = input.get(group).and_then(Value::as_object) else { continue };
        for (name, raw) in map {
            if !is_widget_input(raw) {
                continue;
            }
            let extra_slot = raw
                .as_array()
                .and_then(|items| items.get(1))
                .and_then(Value::as_object)
                .and_then(|options| options.get("control_after_generate"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            out.push(WidgetSpec { name: name.clone(), extra_slot });
        }
    }
    out
}

/// 现在**空着**的连线型输入（广播型虚拟节点原来就是往这些地方灌值的）。
fn collect_open_inputs(
    info: &Map<String, Value>,
    inputs: &Map<String, Value>,
    id: &str,
    into: &mut Vec<String>,
) {
    let Some(input) = info.get("input").and_then(Value::as_object) else { return };
    for group in ["required", "optional"] {
        let Some(map) = input.get(group).and_then(Value::as_object) else { continue };
        for (name, raw) in map {
            if inputs.contains_key(name) {
                continue;
            }
            // 控件型（含 COMBO）空着时 ComfyUI 用默认值，不算缺口；
            // forceInput 是**插口**，哪怕类型是 STRING 也算缺口
            if is_widget_input(raw) {
                continue;
            }
            let Some(type_name) = raw.as_array().and_then(|items| items.first()).and_then(primitive_text)
            else {
                continue;
            };
            into.push(format!("{id}.{name} ({type_name})"));
        }
    }
}

/// 图里有没有"指向一个不存在的节点"的输入。
///
/// 这不是吹毛求疵：被摘掉的节点留下的**悬挂引用**提交上去，ComfyUI 只会回一句
/// "节点 xxx 不存在"，用户完全看不出是哪一步坏的。
fn dangling_inputs(out: &Map<String, Value>) -> Vec<String> {
    let mut dangling: Vec<String> = Vec::new();
    for (id, value) in out {
        let Some(node) = value.as_object() else { continue };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, reference) in inputs {
            let Some(source) = reference.as_array().and_then(|items| items.first()).and_then(primitive_text)
            else {
                continue;
            };
            if !out.contains_key(&source) {
                dangling.push(format!("{id}.{name} ← 节点 {source}（没有转换出来）"));
            }
        }
    }
    dangling
}

/// 一个引用一路解到"真正的产出节点"（穿透 Reroute / Set-Get / 旁路）。带步数上限防环。
fn resolve(reference: (String, i64), redirects: &Redirects) -> (String, i64) {
    let mut current = reference;
    let mut guard = 0;
    while guard < 64 {
        guard += 1;
        match redirects.get(&current) {
            Some(next) if next != &current => current = next.clone(),
            _ => return current,
        }
    }
    current
}

/// 节点第一个有连线的输入 → (上游节点, 槽位)。
fn first_linked_input(
    node: &Map<String, Value>,
    links: &LinkTable,
) -> Option<(String, i64)> {
    let inputs = node.get("inputs").and_then(Value::as_array)?;
    for entry in inputs {
        let Some(entry) = entry.as_object() else { continue };
        let Some(link_id) = as_int(entry.get("link")) else { continue };
        if let Some(source) = links.get(&link_id) {
            return Some(source.clone());
        }
    }
    None
}

/// `SetNode` / `GetNode` 的"变量名"（唯一那个控件值）。
fn widget_string(node: &Map<String, Value>) -> Option<String> {
    let value = match node.get("widgets_values")? {
        Value::Array(items) => items.first().and_then(primitive_text),
        Value::Object(map) => map.values().next().and_then(primitive_text),
        _ => None,
    }?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn ui_class(node: &Map<String, Value>) -> String {
    node.get("type").and_then(primitive_text).unwrap_or_default()
}

/// 节点 id：界面格式里是**数字**，统一转成文本（API 格式的键是字符串）。
fn node_id(node: &Map<String, Value>) -> Option<String> {
    node.get("id").and_then(primitive_text)
}

fn node_int(node: &Map<String, Value>, field: &str) -> Option<i64> {
    as_int(node.get(field))
}

/// 原始值 → 文本（Kotlin `contentOrNull`：数字给字面量、布尔给 `true`/`false`）。
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Kotlin `asInt`：先按 double 截断（`"20.0"` → 20），越界饱和。
fn as_int(value: Option<&Value>) -> Option<i64> {
    let text = value.and_then(primitive_text)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = trimmed.parse::<i64>() {
        return Some(parsed);
    }
    trimmed.parse::<f64>().ok().map(|v| v.trunc().clamp(i64::MIN as f64, i64::MAX as f64) as i64)
}

/// 节点引用里的 id 值：**数字 id 发数字、非数字发字符串**（Kotlin `WorkflowNodeKeys.refOf`）。
///
/// ⚠ 这条看着"无所谓"，其实是**两端一致**的问题：图的**键**永远是字符串（`"4"`），
/// 而引用里 Kotlin 会把纯数字 id 写成 `4`。真机上 Kotlin 提交的就是这种形状
/// （用户的工作流一直在跑），所以这里照它 —— 全都写字符串虽然也符合 ComfyUI 的规范，
/// 但会让"两端转换同一份文件"的产物**逐字节不同**，而这类差异不会报错、只会让人怀疑。
fn ref_of(id: &str) -> Value {
    match id.parse::<i64>() {
        Ok(number) => json!(number),
        Err(_) => json!(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(text: &str) -> Map<String, Value> {
        serde_json::from_str::<Value>(text).unwrap().as_object().unwrap().clone()
    }

    /// 一份最小但真实的 `/object_info`（三个节点，含 `control_after_generate`）。
    fn info() -> Map<String, Value> {
        object(
            r#"{
              "KSampler": {
                "input": {
                  "required": {
                    "model": ["MODEL"],
                    "seed": ["INT", {"default": 0, "control_after_generate": true}],
                    "steps": ["INT", {"default": 20}],
                    "cfg": ["FLOAT", {"default": 8.0}],
                    "sampler_name": [["euler","dpmpp_2m"]],
                    "scheduler": [["normal","simple"]],
                    "positive": ["CONDITIONING"],
                    "negative": ["CONDITIONING"],
                    "latent_image": ["LATENT"]
                  }
                }
              },
              "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [["krea2.safetensors"]] } } },
              "CLIPTextEncode": {
                "input": { "required": { "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } }
              },
              "EmptyLatentImage": {
                "input": { "required": {
                  "width": ["INT"], "height": ["INT"], "batch_size": ["INT"] } }
              },
              "SaveImage": {
                "input": { "required": {
                  "images": ["IMAGE"], "filename_prefix": ["STRING", {"forceInput": false}] } }
              }
            }"#,
        )
    }

    /// 一份界面格式工作流（经典 KSampler 五节点 + 连线 + widgets_values 位置式）。
    fn ui() -> Map<String, Value> {
        object(
            r#"{
              "nodes": [
                {"id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["krea2.safetensors"]},
                {"id": 2, "type": "CLIPTextEncode", "inputs": [{"name":"clip","type":"CLIP","link":4}],
                 "widgets_values": ["1girl, ink painting"]},
                {"id": 3, "type": "CLIPTextEncode", "inputs": [{"name":"clip","type":"CLIP","link":5}],
                 "widgets_values": ["lowres"]},
                {"id": 4, "type": "EmptyLatentImage", "widgets_values": [768, 1024, 1]},
                {"id": 5, "type": "KSampler",
                 "inputs": [
                   {"name":"model","type":"MODEL","link":1},
                   {"name":"positive","type":"CONDITIONING","link":2},
                   {"name":"negative","type":"CONDITIONING","link":3},
                   {"name":"latent_image","type":"LATENT","link":6}],
                 "widgets_values": [12345, "fixed", 20, 2.5, "euler", "normal"]},
                {"id": 6, "type": "SaveImage",
                 "inputs": [{"name":"images","type":"IMAGE","link":7}],
                 "widgets_values": ["krea2/t2i"]}
              ],
              "links": [
                [1, 1, 0, 5, 0, "MODEL"],
                [2, 2, 0, 5, 1, "CONDITIONING"],
                [3, 3, 0, 5, 2, "CONDITIONING"],
                [4, 1, 0, 2, 0, "CLIP"],
                [5, 1, 0, 3, 0, "CLIP"],
                [6, 4, 0, 5, 3, "LATENT"],
                [7, 5, 0, 6, 0, "IMAGE"]
              ]
            }"#,
        )
    }

    #[test]
    fn converts_a_classic_ui_workflow() {
        let converted = to_api_graph(&ui(), &info()).unwrap();
        assert!(converted.runnable(), "{:?}", converted.unsupported_nodes);
        assert_eq!(converted.node_count, 6);
        let sampler = converted.graph.get("5").unwrap();
        assert_eq!(sampler["class_type"], "KSampler");
        assert_eq!(sampler["inputs"]["model"], json!([1, 0]));
        assert_eq!(sampler["inputs"]["positive"], json!([2, 0]));
        // 控件按声明顺序贴回名字上；`seed` 后面那个 control_after_generate 槽位要跳过去
        assert_eq!(sampler["inputs"]["seed"], 12345);
        assert_eq!(sampler["inputs"]["steps"], 20);
        assert_eq!(sampler["inputs"]["cfg"], 2.5);
        assert_eq!(sampler["inputs"]["sampler_name"], "euler");
        assert_eq!(sampler["inputs"]["scheduler"], "normal");
        assert_eq!(converted.graph["2"]["inputs"]["text"], "1girl, ink painting");
        assert_eq!(converted.graph["4"]["inputs"]["width"], 768);
        assert!(converted.warnings.is_empty(), "{:?}", converted.warnings);
    }

    /// `widgets_values_named` 优先（新版前端会写一份 `{控件名: 值}`）。
    #[test]
    fn named_widgets_win_over_positional() {
        let mut workflow = ui();
        let nodes = workflow.get_mut("nodes").unwrap().as_array_mut().unwrap();
        let sampler = nodes.iter_mut().find(|n| n["id"] == 5).unwrap().as_object_mut().unwrap();
        sampler.insert(
            "widgets_values_named".into(),
            json!({"seed": 777, "steps": 8, "unknown_widget": "x"}),
        );
        let converted = to_api_graph(&workflow, &info()).unwrap();
        let inputs = &converted.graph["5"]["inputs"];
        assert_eq!(inputs["seed"], 777);
        assert_eq!(inputs["steps"], 8);
        assert!(inputs.get("sampler_name").is_none(), "named 那份只认声明过的键");
        assert!(
            converted.warnings.iter().any(|w| w.contains("unknown_widget")),
            "多出来的键要如实警告：{:?}",
            converted.warnings
        );
    }

    /// Reroute / Set-Get 要穿透到真正的上游；`Note` 丢掉。
    #[test]
    fn reroute_and_set_get_are_rewritten() {
        let workflow = object(
            r#"{
              "nodes": [
                {"id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["krea2.safetensors"]},
                {"id": 2, "type": "Reroute", "inputs": [{"name":"","link":1}]},
                {"id": 3, "type": "Note", "widgets_values": ["这是个备注"]},
                {"id": 4, "type": "SetNode", "inputs": [{"name":"","link":1}], "widgets_values": ["model"]},
                {"id": 5, "type": "GetNode", "widgets_values": ["model"]},
                {"id": 6, "type": "KSampler",
                 "inputs": [
                   {"name":"model","type":"MODEL","link":2},
                   {"name":"latent_image","type":"LATENT","link":3},
                   {"name":"positive","type":"CONDITIONING","link":99},
                   {"name":"negative","type":"CONDITIONING","link":99}],
                 "widgets_values": [1, "fixed", 4, 1.0, "euler", "normal"]},
                {"id": 7, "type": "EmptyLatentImage", "widgets_values": [512, 512, 1]}
              ],
              "links": [[1,1,0,2,0,"MODEL"],[2,4,0,6,0,"MODEL"],[3,7,0,6,1,"LATENT"],[9,5,0,6,0,"MODEL"]]
            }"#,
        );
        let mut info = info();
        // GetNode 指向的 SetNode 那条线：把 6 的 model 从 GetNode(5) 挪到 link 9 上验证
        info.insert("GetNode".into(), json!({"input": {}}));
        let converted = to_api_graph(&workflow, &info).unwrap();
        let inputs = &converted.graph["6"]["inputs"];
        // Reroute 穿透：model ← 节点 1（不是节点 2）
        assert_eq!(inputs["model"], json!([1, 0]), "{:?}", converted.rewritten);
        assert!(
            converted.rewritten.iter().any(|r| r.contains("Reroute"))
                && converted.rewritten.iter().any(|r| r.contains("Note")),
            "{:?}",
            converted.rewritten
        );
        // `positive` / `negative` 指向的 link 99 在 links 表里**根本不存在** →
        // 它们不是"悬挂引用"（那是"指向一个没转换出来的节点"），而是**空着的连线型输入**，
        // 正好是广播型节点原来要灌值的地方（也是模型补线的作业本）
        assert!(
            converted.open_inputs.iter().any(|u| u.contains("6.positive (CONDITIONING)")),
            "{:?}",
            converted.open_inputs
        );
        assert!(converted.unresolved_inputs.is_empty(), "{:?}", converted.unresolved_inputs);
        assert!(converted.runnable(), "空着的连线型输入不算「不能跑」：{:?}", converted.unsupported_nodes);
    }

    /// 旁路（mode=4）：输出要接到**同类型**的已连接输入上；找不到就如实警告。
    #[test]
    fn bypass_redirects_to_the_same_type_input() {
        let workflow = object(
            r#"{
              "nodes": [
                {"id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["krea2.safetensors"]},
                {"id": 2, "type": "LoraLoader", "mode": 4,
                 "inputs": [{"name":"model","type":"MODEL","link":1}],
                 "outputs": [{"name":"MODEL","type":"MODEL"}]},
                {"id": 3, "type": "KSampler",
                 "inputs": [
                   {"name":"model","type":"MODEL","link":2},
                   {"name":"positive","type":"CONDITIONING","link":9},
                   {"name":"negative","type":"CONDITIONING","link":9},
                   {"name":"latent_image","type":"LATENT","link":9}],
                 "widgets_values": [1, "fixed", 4, 1.0, "euler", "normal"]}
              ],
              "links": [[1,1,0,2,0,"MODEL"],[2,2,0,3,0,"MODEL"]]
            }"#,
        );
        let converted = to_api_graph(&workflow, &info()).unwrap();
        // 旁路的 LoraLoader 自己不进图；KSampler 的 model 直接接到节点 1
        assert!(converted.graph.get("2").is_none(), "旁路节点不该出现在图里");
        assert_eq!(converted.graph["3"]["inputs"]["model"], json!([1, 0]));
        assert!(
            converted.rewritten.iter().any(|r| r.contains("旁路")),
            "{:?}",
            converted.rewritten
        );
    }

    /// 静音（mode=2）：不进图 + 如实警告下游；未知 mode：unsupported。
    #[test]
    fn muted_and_unknown_modes_are_reported() {
        let workflow = object(
            r#"{
              "nodes": [
                {"id": 1, "type": "CheckpointLoaderSimple", "mode": 2, "widgets_values": ["krea2.safetensors"]},
                {"id": 2, "type": "KSampler", "mode": 7,
                 "inputs": [{"name":"model","type":"MODEL","link":1}],
                 "widgets_values": [1, "fixed", 4, 1.0, "euler", "normal"]},
                {"id": 3, "type": "SaveImage", "inputs": [{"name":"images","type":"IMAGE","link":1}]}
              ],
              "links": [[1,1,0,2,0,"MODEL"]]
            }"#,
        );
        let converted = to_api_graph(&workflow, &info()).unwrap();
        assert!(converted.graph.get("1").is_none(), "静音节点不进图");
        assert!(converted.graph.get("2").is_none(), "未知 mode 的节点不进图");
        assert!(
            converted.warnings.iter().any(|w| w.contains("是静音的")),
            "{:?}",
            converted.warnings
        );
        assert!(
            converted.unsupported_nodes.iter().any(|u| u.contains("mode=7")),
            "{:?}",
            converted.unsupported_nodes
        );
        // 静音节点留下的悬挂引用必须报出来（下游会缺输入）
        assert!(
            converted.unresolved_inputs.iter().any(|u| u.contains("3.images")),
            "{:?}",
            converted.unresolved_inputs
        );
    }

    /// 组节点（subgraph）：**这一版不展开，必须如实报**（不许猜着展开）。
    #[test]
    fn subgraph_instances_are_reported_not_guessed() {
        let workflow = object(
            r#"{
              "definitions": { "subgraphs": [ {"id": "def-uuid-1", "nodes": [], "links": []} ] },
              "nodes": [
                {"id": 1, "type": "def-uuid-1", "inputs": [], "outputs": []},
                {"id": 2, "type": "CheckpointLoaderSimple", "widgets_values": ["krea2.safetensors"]}
              ],
              "links": []
            }"#,
        );
        let converted = to_api_graph(&workflow, &info()).unwrap();
        assert!(
            converted.unsupported_nodes.iter().any(|u| u.contains("这一版还不展开 subgraph")),
            "{:?}",
            converted.unsupported_nodes
        );
        assert!(!converted.runnable(), "有组节点 → 不许当成能跑");
        assert!(converted.graph.contains_key("2"), "别的节点照样要转换出来");
    }

    /// **逐字抄 Kotlin `WorkflowConvertTest` 的那份 `objectInfo` 与 `uiWorkflow`**，
    /// 断言也照它写（`model ← [4,0]` / `seed 20260918` / `steps 30` / `denoise 1.0` / 宽高 896×1264）。
    ///
    /// 为什么要专门抄一遍：这些断言是**Kotlin 那边已经验过的预期** ——
    /// 拿它当"两份实现转换同一份文件"的判据，比我自己想一份图更接近真实存档的形状
    /// （六个节点、七个 link、`control_after_generate` 占位、optional 里的控件）。
    #[test]
    fn kotlin_case_classic_ui_workflow() {
        let object_info = object(
            r#"{
              "CheckpointLoaderSimple": { "input": { "required": {
                "ckpt_name": [["a.safetensors"]] } } },
              "CLIPTextEncode": { "input": { "required": {
                "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } } },
              "EmptyLatentImage": { "input": { "required": {
                "width": ["INT", {"default": 512}], "height": ["INT", {"default": 512}],
                "batch_size": ["INT", {"default": 1}] } } },
              "KSampler": { "input": { "required": {
                "model": ["MODEL"],
                "seed": ["INT", {"default": 0, "control_after_generate": true}],
                "steps": ["INT", {"default": 20}],
                "cfg": ["FLOAT", {"default": 7.0}],
                "sampler_name": [["euler","dpmpp_2m"]],
                "scheduler": [["normal","karras"]],
                "positive": ["CONDITIONING"], "negative": ["CONDITIONING"],
                "latent_image": ["LATENT"], "denoise": ["FLOAT", {"default": 1.0}] } } },
              "SaveImage": { "input": {
                "required": { "images": ["IMAGE"] },
                "optional": { "filename_prefix": ["STRING", {"default": "ComfyUI"}] } } }
            }"#,
        );
        let ui = object(
            r#"{
              "nodes": [
                { "id": 4, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [], "outputs": [{"name":"MODEL"}] },
                { "id": 6, "type": "CLIPTextEncode", "mode": 0,
                  "widgets_values": ["一只赛博朋克猫"], "inputs": [{"name":"clip","link":1}] },
                { "id": 7, "type": "CLIPTextEncode", "mode": 0,
                  "widgets_values": ["worst quality"], "inputs": [{"name":"clip","link":2}] },
                { "id": 5, "type": "EmptyLatentImage", "mode": 0,
                  "widgets_values": [896, 1264, 1], "inputs": [] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [20260918, "randomize", 30, 3.5, "euler", "normal", 1.0],
                  "inputs": [
                    {"name":"model","link":3}, {"name":"positive","link":4},
                    {"name":"negative","link":5}, {"name":"latent_image","link":6} ] },
                { "id": 9, "type": "SaveImage", "mode": 0,
                  "widgets_values": ["krea2/out"],
                  "inputs": [{"name":"images","link":7}] }
              ],
              "links": [
                [1, 4, 1, 6, 0, "CLIP"], [2, 4, 1, 7, 0, "CLIP"], [3, 4, 0, 3, 0, "MODEL"],
                [4, 6, 0, 3, 1, "CONDITIONING"], [5, 7, 0, 3, 2, "CONDITIONING"],
                [6, 5, 0, 3, 3, "LATENT"], [7, 3, 0, 9, 0, "IMAGE"]
              ]
            }"#,
        );
        let converted = to_api_graph(&ui, &object_info).unwrap();
        assert!(converted.unsupported_nodes.is_empty(), "{:?}", converted.unsupported_nodes);
        assert_eq!(converted.node_count, 6);
        let sampler = &converted.graph["3"]["inputs"];
        assert_eq!(sampler["model"], json!([4, 0]));
        assert_eq!(sampler["positive"], json!([6, 0]));
        // seed 之后跳过 control_after_generate 那个槽位，steps 才拿到 30
        assert_eq!(sampler["seed"], 20260918);
        assert_eq!(sampler["steps"], 30);
        assert_eq!(sampler["cfg"], 3.5);
        assert_eq!(sampler["sampler_name"], "euler");
        assert_eq!(sampler["scheduler"], "normal");
        assert_eq!(sampler["denoise"], 1.0);
        let latent = &converted.graph["5"]["inputs"];
        assert_eq!(
            (latent["width"].clone(), latent["height"].clone(), latent["batch_size"].clone()),
            (json!(896), json!(1264), json!(1))
        );
        assert_eq!(converted.graph["6"]["inputs"]["text"], "一只赛博朋克猫");
        // SaveImage 的 `filename_prefix` 在 optional 里 —— 也要按声明顺序拿到
        assert_eq!(converted.graph["9"]["inputs"]["filename_prefix"], "krea2/out");
    }

    /// Kotlin `forceInput 的输入不算控件 位置不会被它挤位`：`forceInput` 是**插口**，
    /// 哪怕类型是 STRING 也没有 `widgets_values` 槽位 —— 当成控件会让后面所有控件整体错位。
    #[test]
    fn kotlin_case_force_input_is_not_a_widget() {
        let object_info = object(
            r#"{"ForceInputNode": { "input": { "required": {
                 "value": ["STRING", {"forceInput": true}],
                 "note": ["STRING", {"default": ""}] } } }}"#,
        );
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "ForceInputNode", "mode": 0,
                             "widgets_values": ["hello"],
                             "inputs": [{"name":"value","link":9}] } ],
                "links": [[9, 2, 0, 1, 0, "STRING"]] }"#,
        );
        let converted = to_api_graph(&ui, &object_info).unwrap();
        assert_eq!(converted.graph["1"]["inputs"]["value"], json!([2, 0]));
        assert_eq!(converted.graph["1"]["inputs"]["note"], "hello");
    }

    /// 空图 / 脏数据：报错或跳过，**不许 panic**。
    #[test]
    fn empty_and_dirty_workflows_are_safe() {
        let empty = object(r#"{"nodes": [], "links": []}"#);
        assert!(to_api_graph(&empty, &info()).is_err());
        let dirty = object(
            r#"{"nodes": ["nope", {"id": 1, "type": "Unknown Thing"}], "links": [null, 7]}"#,
        );
        let converted = to_api_graph(&dirty, &info()).unwrap();
        assert!(converted.unsupported_nodes.iter().any(|u| u.contains("Unknown Thing")));
        assert!(converted.graph.is_empty());
    }

    #[test]
    fn ui_detection_only_looks_at_nodes() {
        assert!(is_ui_workflow(&object(r#"{"nodes": []}"#)));
        assert!(!is_ui_workflow(&object(r#"{"1": {"class_type": "KSampler"}}"#)));
        assert!(!is_ui_workflow(&object(r#"{"nodes": "nope"}"#)));
    }
}
