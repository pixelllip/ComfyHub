//! ComfyUI **界面格式工作流**（`{nodes,links}`）→ **API 格式节点图**（Kotlin `WorkflowConvert` 的
//! Rust 端逐行对照，P4 第 44 步；**第 68 步**把最后两处欠账补齐：组节点展开 + 「模拟用户行为」）。
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
//! | 组节点（subgraph） | **按定义展开**成内部真节点（见 [`flatten`]） |
//! | 广播型 / 显示型纯前端节点（`Anything Everywhere` 这类） | **`emulateFrontend=true` 时等价改写**（见 [`crate::workflow_frontend`]）；`false` 时留在 `unsupportedNodes` 里如实报 |
//!
//! ## 组节点（subgraph）怎么展开（Kotlin `flatten`，逐条对照）
//!
//! 存档里的形状：`definitions.subgraphs[]` 每个定义有 `id`(UUID) / `nodes` / `links` /
//! `inputs` / `outputs`，以及两个**代理节点** `inputNode.id`（本机实测 -10）与
//! `outputNode.id`（-20）；主图里出现一个 `type` 等于那个 UUID 的节点，它的 `inputs`
//! 与定义里的 `inputs` **按顺序一一对应**（实测 6 个组节点全部如此）。服务端拿不到前端那份 JS，
//! 但这些都是**数据**，可以照实展开：
//!
//! * 内部节点会与主图**撞号** → 重新分配一批没人用过的新号（从"现有最大值 + 1"起）；
//! * 内部连线 id 也是局部编号（实测出现过 14717 这种与主图重号的）→ 整表重新编号；
//! * `origin_id = inputNode` 的连线 = "从实例的第 k 个输入进来"：接上主图那边的真实来源；
//!   那个输入没接线、而是外层控件时，把实例的控件值**直接填给内部节点的对应输入**
//!   （`forcedInputs` —— **不能走控件顺序**，那是另一套编号）；
//! * `target_id = outputNode` 的连线 = "实例的第 k 个输出"：登记成改写，
//!   让下游拿到内部真正产出那个节点；
//! * `mode != 0` 的实例**不展开**（它自己不执行，交给旁路 / 静音那套逻辑接过去）；
//! * 缺代理 / 输入个数对不上 / 组里还套组 → **如实 `unsupported`**，绝不猜着展开。
//!
//! ## 「模拟用户行为」（`emulateFrontend`）在哪个文件
//!
//! 判据（广播型 / 显示型两张表 + 三条模式）只有一份：`workflow_audit.rs` 的
//! [`crate::workflow_audit::is_broadcast`] / [`crate::workflow_audit::is_display`]；改写本身
//! 在 [`crate::workflow_frontend`]（Kotlin `FrontendNodes.kt` 的对照）。
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
pub(crate) type LinkTable = HashMap<i64, (String, i64)>;
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
    /// ## 模拟用户行为 ## 等价改写掉的前端节点，形如 `12 Anything Everywhere（广播 3 处）`。
    ///
    /// 只有 `emulateFrontend=true` 才有内容。改了哪几根线看得见，用户才能判断这次
    /// "替前端做的事"对不对（Kotlin `Result.frontendRewritten`）。
    pub frontend_rewritten: Vec<String>,
    /// 广播灌进去的线，形如 `1.positive ← 节点 14（Anything Everywhere）`。
    pub broadcast_lines: Vec<String>,
    /// 照前端的做法自动补的输出节点。
    pub injected_nodes: Vec<String>,
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

/// 界面格式 → API 节点图（Kotlin `toApiGraph`）。
///
/// `emulate_frontend` = Kotlin 的第三个参数：开着时，转换完再做一遍
/// 「模拟用户行为」（[`crate::workflow_frontend::emulate`]）—— 广播型节点按声明的
/// 类型 / 名字填空、显示节点丢掉、一个输出节点都没有时补一个 `SaveImage`。
/// 关着时纯前端节点照旧留在 `unsupported_nodes` 里（一个字都不改）。
pub fn to_api_graph(
    ui: &Map<String, Value>,
    object_info: &Map<String, Value>,
    emulate_frontend: bool,
) -> Result<Converted, String> {
    let Flat {
        nodes: raw_nodes,
        links,
        mut redirects,
        handled,
        forced_inputs,
        mut rewritten,
        mut unsupported,
        mut warnings,
    } = flatten(ui, object_info);
    if raw_nodes.is_empty() {
        return Err("工作流里一个节点都没有（nodes 是空的）".to_string());
    }

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
        // 组节点实例要么已经被展开成内部的真节点（它自己不再是一个节点），
        // 要么已经带着**具体原因**报过 unsupported 了
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
        // 2) 组节点把内部节点的输入暴露成外层控件时，值直接填进那个输入
        //    （**不能走控件顺序**：实例的控件槽位与内部节点的输入名是两套编号）
        if let Some(forced) = forced_inputs.get(&id) {
            for (name, value) in forced {
                inputs.insert(name.clone(), value.clone());
            }
        }
        // 3) 控件值：按 /object_info 的声明顺序贴回名字上
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

    // --- 第三遍：## 模拟用户行为 ##（`emulateFrontend`，Kotlin `FrontendNodes.emulate`）---
    //
    // 到这一步为止，纯前端节点还都躺在 `unsupported` 里。开着就做"前端 JS 在排队前会做的
    // 那几件事"：广播型节点按它声明的类型 / 名字过滤把上游那一路**填空**、显示节点丢掉、
    // 一个输出节点都没有时补一个 `SaveImage`。判据一律保守：说不清类型的广播**不猜**，
    // 照旧留在 unsupported 里（出口还是一样）。
    let mut final_graph = out;
    let mut final_open = open_inputs;
    let mut frontend_rewritten: Vec<String> = Vec::new();
    let mut broadcast_lines: Vec<String> = Vec::new();
    let mut injected_nodes: Vec<String> = Vec::new();
    if emulate_frontend {
        let (emulated, report) =
            crate::workflow_frontend::emulate(&raw_nodes, object_info, &final_graph, &links);
        final_graph = emulated;
        // 广播灌完之后"还空着"的清单要重算：不然模型会去补一根已经接好的线
        final_open = crate::workflow_frontend::open_inputs_of(object_info, &final_graph);
        frontend_rewritten = report.rewritten;
        broadcast_lines = report.broadcast;
        injected_nodes = report.injected;
        // 已经等价改写掉的节点不该再挂在 unsupported 里
        unsupported.retain(|line| {
            !report.handled_ids.iter().any(|handled_id| line.starts_with(&format!("{handled_id} ")))
        });
        unsupported.extend(report.unsupported);
    }

    let unresolved = dangling_inputs(&final_graph);
    Ok(Converted {
        // ⚠ Kotlin 数的是 **finalGraph**（补完 `SaveImage` 之后）的节点数
        node_count: final_graph.len(),
        graph: final_graph,
        rewritten,
        warnings,
        unsupported_nodes: unsupported,
        unresolved_inputs: unresolved,
        open_inputs: final_open,
        frontend_rewritten,
        broadcast_lines,
        injected_nodes,
    })
}

/// 展开组节点之后得到的图（Kotlin `Flat`）。
struct Flat {
    /// 顶层节点 + 展开出来的内部真节点（顺序 = Kotlin 的 `nodes`
    /// —— `preserve_order` 下是插入序，与它一致）
    nodes: Vec<Map<String, Value>>,
    /// linkId → (上游节点 id, 槽位)。节点 id 统一是字符串。
    links: LinkTable,
    /// 预置的改写表：组节点的输出槽位 → 内部真正的产出节点
    redirects: Redirects,
    /// 已经处理过的组节点实例 id：要么展开了，要么已经带着**具体原因**报过错了
    handled: Vec<String>,
    /// 内部节点的某个输入由"实例上的控件"提供时，直接填进去的值
    forced_inputs: HashMap<String, Map<String, Value>>,
    rewritten: Vec<String>,
    unsupported: Vec<String>,
    warnings: Vec<String>,
}

/// 把界面格式的图**摊平**：组节点就地展开成内部的真节点，连线合并成一张表（Kotlin `flatten`）。
///
/// 规则见文件头"组节点（subgraph）怎么展开"那一节 —— 每一条都有出处，**没有一条是猜的**。
/// 这里刻意**不做**的三件事（Kotlin 也不做，做了就是猜）：
/// 缺输入 / 输出代理、实例输入个数与定义对不上、组里还套着一层组节点。
fn flatten(ui: &Map<String, Value>, object_info: &Map<String, Value>) -> Flat {
    let defs = subgraph_defs(ui);
    let top_nodes: Vec<Map<String, Value>> = ui
        .get("nodes")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|v| v.as_object().cloned()).collect())
        .unwrap_or_default();
    let top_links = parse_raw_links(ui.get("links"));

    let mut nodes: Vec<Map<String, Value>> = Vec::new();
    let mut links: LinkTable = LinkTable::new();
    let mut redirects: Redirects = Redirects::new();
    let mut forced_inputs: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut handled: Vec<String> = Vec::new();
    let mut rewritten: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let warnings: Vec<String> = Vec::new();

    // 新号从"现有最大值 + 1"开始，保证不与任何已有编号冲突
    let mut next_link_id = top_links.keys().copied().max().unwrap_or(0) + 1;
    let mut next_node_id = top_nodes
        .iter()
        .filter_map(|node| node_id(node).and_then(|id| id.parse::<i64>().ok()))
        .max()
        .unwrap_or(0)
        + 1;

    for (id, link) in &top_links {
        links.insert(*id, (link.origin.clone(), link.origin_slot));
    }

    for node in &top_nodes {
        let id = node_id(node);
        let class = ui_class(node);
        // ⚠ 判定顺序与 Kotlin 一致：**先看 `/object_info` 里有没有这个类**。
        //   有的版本把 `SetNode` 之类做成了真节点，那时它就该原样保留。
        let def = if !class.is_empty() && object_info.get(&class).is_none() {
            defs.get(&class)
        } else {
            None
        };
        let mode = node_int(node, "mode").unwrap_or(0);
        let (Some(def), Some(id)) = (def, id) else {
            // 不是组节点（或 id 认不出来）：原样留到第二遍
            nodes.push(node.clone());
            continue;
        };
        if mode != 0 {
            // 旁路 / 静音的组节点**不展开**（Kotlin 同：它自己不执行，
            // 交给旁路 / 静音那套逻辑按类型接过去）
            nodes.push(node.clone());
            continue;
        }

        let short: String = class.chars().take(8).collect();
        let input_proxy = def.get("inputNode").and_then(Value::as_object).and_then(node_id);
        let output_proxy = def.get("outputNode").and_then(Value::as_object).and_then(node_id);
        let iface: Vec<&Map<String, Value>> = object_list(def.get("inputs"));
        let inst_inputs: Vec<&Map<String, Value>> = object_list(node.get("inputs"));
        let inner_nodes: Vec<&Map<String, Value>> = object_list(def.get("nodes"));

        // 失败的三档：**带具体原因**报 unsupported，并登记 handled（第二遍跳过它）
        if input_proxy.is_none() || output_proxy.is_none() || inner_nodes.is_empty() {
            unsupported
                .push(format!("{id} {short}…（组节点：定义里缺少输入 / 输出代理，不敢猜着展开）"));
            handled.push(id.clone());
            nodes.push(node.clone());
            continue;
        }
        if iface.len() != inst_inputs.len() {
            unsupported.push(format!(
                "{id} {short}…（组节点实例有 {} 个输入，定义里是 {} 个，对不上，不敢猜着展开）",
                inst_inputs.len(),
                iface.len()
            ));
            handled.push(id.clone());
            nodes.push(node.clone());
            continue;
        }
        if inner_nodes.iter().any(|inner| defs.contains_key(&ui_class(inner))) {
            unsupported.push(format!("{id} {short}…（组节点里还套着一层组节点，这一版不展开）"));
            handled.push(id.clone());
            nodes.push(node.clone());
            continue;
        }

        let inner_links = parse_raw_links(def.get("links"));
        // 内部连线整表重新编号（新旧是一一对应的双射，分配顺序不影响语义）
        let mut renumber: HashMap<i64, i64> = HashMap::new();
        for old in inner_links.keys() {
            renumber.insert(*old, next_link_id);
            next_link_id += 1;
        }
        // 内部节点整批换新号（内部 id 与主图撞号是常态）
        let mut id_map: HashMap<String, String> = HashMap::new();
        for inner in &inner_nodes {
            if let Some(raw) = node_id(inner) {
                id_map.insert(raw, next_node_id.to_string());
                next_node_id += 1;
            }
        }

        // 实例的每个输入：接了线 → 真实来源；没接线但是控件 → 值（等会儿填给内部节点）
        let mut source_of_slot: Vec<Option<(String, i64)>> = vec![None; iface.len()];
        let mut widget_of_slot: Vec<Option<Value>> = vec![None; iface.len()];
        for (slot, entry) in inst_inputs.iter().enumerate() {
            if let Some(source) = as_int(entry.get("link"))
                .and_then(|link_id| top_links.get(&link_id))
                .map(|link| (link.origin.clone(), link.origin_slot))
            {
                source_of_slot[slot] = Some(source);
                continue;
            }
            // Kotlin `name?.let { named?.get(it) } ?: positional`：
            // 有名字那份里**没有这个键**时也要退回位置式那一份
            let name = iface.get(slot).and_then(|decl| decl.get("name")).and_then(primitive_text);
            let named = node.get("widgets_values_named").and_then(Value::as_object);
            let positional = node
                .get("widgets_values")
                .and_then(Value::as_array)
                .and_then(|values| values.get(slot));
            let value = name
                .as_deref()
                .and_then(|name| named.and_then(|map| map.get(name)))
                .or(positional);
            if let Some(value) = value {
                if !value.is_null() {
                    widget_of_slot[slot] = Some(value.clone());
                }
            }
        }

        // 内部连线：输入代理换成真实来源；输出代理登记成实例输出的改写
        let mut widget_fix: HashMap<i64, (String, Value)> = HashMap::new();
        for (old_link, link) in &inner_links {
            let Some(new_link) = renumber.get(old_link).copied() else { continue };
            if Some(link.origin.as_str()) == input_proxy.as_deref() {
                let source = source_of_slot.get(link.origin_slot as usize).and_then(Clone::clone);
                let widget = widget_of_slot.get(link.origin_slot as usize).and_then(Clone::clone);
                match (source, widget) {
                    (Some(source), _) => {
                        links.insert(new_link, source);
                    }
                    // 值来自外层控件：记下"哪个内部节点的哪个输入"，等会儿直接填
                    (None, Some(value)) => {
                        if let Some(target) = id_map.get(&link.target) {
                            widget_fix.insert(*old_link, (target.clone(), value));
                        }
                    }
                    // 都没有：这个输入本来就空着 —— 号不登记，内部那个输入就是"悬着"的
                    (None, None) => {}
                }
            } else {
                let origin =
                    id_map.get(&link.origin).cloned().unwrap_or_else(|| link.origin.clone());
                links.insert(new_link, (origin, link.origin_slot));
            }
            if Some(link.target.as_str()) == output_proxy.as_deref() {
                let origin =
                    id_map.get(&link.origin).cloned().unwrap_or_else(|| link.origin.clone());
                redirects.insert((id.clone(), link.target_slot), (origin, link.origin_slot));
            }
        }

        // 内部节点本体：换号、重指连线、把外层控件的值填进对应输入
        for inner in &inner_nodes {
            let Some(raw) = node_id(inner) else { continue };
            let Some(new_id) = id_map.get(&raw).cloned() else { continue };
            let mut fixed: Map<String, Value> = Map::new();
            let mut new_inputs: Vec<Value> = Vec::new();
            for entry in object_list(inner.get("inputs")) {
                let Some(old_link) = as_int(entry.get("link")) else {
                    new_inputs.push(Value::Object(entry.clone()));
                    continue;
                };
                // 外层控件给的值：把这个输入的 link 置空，值走 forcedInputs
                let fix = widget_fix
                    .get(&old_link)
                    .filter(|(target, _)| target == &new_id)
                    .map(|(_, value)| value.clone());
                if let Some(value) = fix {
                    if let Some(name) = entry.get("name").and_then(primitive_text) {
                        fixed.insert(name, value);
                    }
                    let mut patched = entry.clone();
                    patched.insert("link".into(), Value::Null);
                    new_inputs.push(Value::Object(patched));
                    continue;
                }
                let Some(renumbered) = renumber.get(&old_link).copied() else {
                    new_inputs.push(Value::Object(entry.clone()));
                    continue;
                };
                let mut patched = entry.clone();
                patched.insert("link".into(), json!(renumbered));
                new_inputs.push(Value::Object(patched));
            }
            if !fixed.is_empty() {
                forced_inputs.insert(new_id.clone(), fixed);
            }
            let mut merged = (*inner).clone();
            merged.insert("id".into(), json!(new_id));
            merged.insert("inputs".into(), Value::Array(new_inputs));
            nodes.push(merged);
        }
        handled.push(id.clone());
        rewritten.push(format!("{id} {short}…（组节点，展开成 {} 个节点）", inner_nodes.len()));
    }

    Flat { nodes, links, redirects, handled, forced_inputs, rewritten, unsupported, warnings }
}

/// `definitions.subgraphs[]` → `id` → 定义（Kotlin `subgraphDefs`，重复 id 时后者胜）。
fn subgraph_defs(ui: &Map<String, Value>) -> HashMap<String, Map<String, Value>> {
    let mut out: HashMap<String, Map<String, Value>> = HashMap::new();
    let Some(list) = ui
        .get("definitions")
        .and_then(Value::as_object)
        .and_then(|defs| defs.get("subgraphs"))
        .and_then(Value::as_array)
    else {
        return out;
    };
    for item in list {
        let Some(def) = item.as_object() else { continue };
        let Some(id) = def.get("id").and_then(primitive_text) else { continue };
        out.insert(id, def.clone());
    }
    out
}

/// 一个值是不是数组、里面的对象逐条借出来（Kotlin `(x as? JsonArray).orEmpty().mapNotNull`）。
fn object_list(raw: Option<&Value>) -> Vec<&Map<String, Value>> {
    raw.and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

/// 界面格式里的一条连线（Kotlin `RawLink`）。
struct RawLink {
    origin: String,
    origin_slot: i64,
    target: String,
    target_slot: i64,
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
                    RawLink {
                        origin,
                        origin_slot: as_int(items.get(2)).unwrap_or(0),
                        target: items.get(3).and_then(primitive_text).unwrap_or_default(),
                        target_slot: as_int(items.get(4)).unwrap_or(0),
                    },
                );
            }
            // {id, origin_id, origin_slot, target_id, target_slot, …}
            Value::Object(obj) => {
                let Some(link_id) = as_int(obj.get("id")) else { continue };
                let Some(origin) = obj.get("origin_id").and_then(primitive_text) else { continue };
                out.insert(
                    link_id,
                    RawLink {
                        origin,
                        origin_slot: as_int(obj.get("origin_slot")).unwrap_or(0),
                        target: obj.get("target_id").and_then(primitive_text).unwrap_or_default(),
                        target_slot: as_int(obj.get("target_slot")).unwrap_or(0),
                    },
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
///
/// ⚠ 判据只有这一份：`workflow_frontend` 的"现在空着哪些连线型输入"也用它
/// （Kotlin 那边是 `WorkflowConvert.isWidgetInput` 与 `FrontendNodes.isWidgetSpec` 两份，
/// 那两份在"声明是数组"这个前提下**判得一样**，搬过来就不必再抄一遍）。
pub(crate) fn is_widget_input(decl: &Value) -> bool {
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

/// 界面格式里节点的类名（**不是** API 格式那个 `class_type`）。
///
/// ⚠ 这两个键名混过一次、代价是**一张空图**（每个节点都被当成"没有类名"跳过，
/// 而空图提交上去只得到 ComfyUI 一句 `Prompt has no outputs`）。
pub(crate) fn ui_class(node: &Map<String, Value>) -> String {
    node.get("type").and_then(primitive_text).unwrap_or_default()
}

/// 节点 id：界面格式里是**数字**，统一转成文本（API 格式的键是字符串）。
///
/// ⚠ 空白 id 当**没有 id**（Kotlin `JsonObject.idOf` 的 `takeIf { it.isNotBlank() }`）：
/// 这样它既不会被当成图的键，也不会被当成某个改写表的键。
pub(crate) fn node_id(node: &Map<String, Value>) -> Option<String> {
    node.get("id").and_then(primitive_text).filter(|id| !id.trim().is_empty())
}

fn node_int(node: &Map<String, Value>, field: &str) -> Option<i64> {
    as_int(node.get(field))
}

/// 原始值 → 文本（Kotlin `contentOrNull`：数字给字面量、布尔给 `true`/`false`）。
pub(crate) fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Kotlin `asLong` / `asInt`：先按字面量、再按 double 截断（`"20.0"` → 20），越界饱和。
pub(crate) fn as_int(value: Option<&Value>) -> Option<i64> {
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
pub(crate) fn ref_of(id: &str) -> Value {
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

    /// **逐字抄 Kotlin `WorkflowConvertTest.objectInfo()`**（第 68 步起，所有 Kotlin 对照用例共用它）。
    ///
    /// 为什么专门抄一遍：这些断言是**Kotlin 那边已经验过的预期** —— 拿它当"两份实现转换同一份
    /// 文件"的判据，比我自己想一份图更接近真实存档的形状（控件顺序 / 种子附加槽位 / 强制插口）。
    fn kotlin_object_info() -> Map<String, Value> {
        object(
            r#"{
              "CheckpointLoaderSimple": {
                "input": { "required": { "ckpt_name": [["a.safetensors","b.safetensors"]] } }
              },
              "CLIPTextEncode": {
                "input": { "required": { "text": ["STRING", {"multiline": true}], "clip": ["CLIP"] } }
              },
              "EmptyLatentImage": {
                "input": { "required": {
                  "width": ["INT", {"default": 512}],
                  "height": ["INT", {"default": 512}],
                  "batch_size": ["INT", {"default": 1}]
                } }
              },
              "KSampler": {
                "input": { "required": {
                  "model": ["MODEL"],
                  "seed": ["INT", {"default": 0, "control_after_generate": true}],
                  "steps": ["INT", {"default": 20}],
                  "cfg": ["FLOAT", {"default": 7.0}],
                  "sampler_name": [["euler","dpmpp_2m"]],
                  "scheduler": [["normal","karras"]],
                  "positive": ["CONDITIONING"],
                  "negative": ["CONDITIONING"],
                  "latent_image": ["LATENT"],
                  "denoise": ["FLOAT", {"default": 1.0}]
                } }
              },
              "SaveImage": {
                "input": {
                  "required": { "images": ["IMAGE"] },
                  "optional": { "filename_prefix": ["STRING", {"default": "ComfyUI"}] }
                }
              },
              "ForceInputNode": {
                "input": { "required": {
                  "value": ["STRING", {"forceInput": true}],
                  "note": ["STRING", {"default": ""}]
                } }
              },
              "ModelPassthrough": { "input": { "required": { "model": ["MODEL"] } } }
            }"#,
        )
    }

    /// **逐字抄 Kotlin `WorkflowConvertTest.uiWorkflow()`**：六个节点、七个 link 的真实形状。
    fn kotlin_ui_workflow() -> Map<String, Value> {
        object(
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
                    {"name":"model","link":3},
                    {"name":"positive","link":4},
                    {"name":"negative","link":5},
                    {"name":"latent_image","link":6}
                  ] },
                { "id": 9, "type": "SaveImage", "mode": 0,
                  "widgets_values": ["krea2/out"],
                  "inputs": [{"name":"images","link":7}] }
              ],
              "links": [
                [1, 4, 1, 6, 0, "CLIP"],
                [2, 4, 1, 7, 0, "CLIP"],
                [3, 4, 0, 3, 0, "MODEL"],
                [4, 6, 0, 3, 1, "CONDITIONING"],
                [5, 7, 0, 3, 2, "CONDITIONING"],
                [6, 5, 0, 3, 3, "LATENT"],
                [7, 3, 0, 9, 0, "IMAGE"]
              ]
            }"#,
        )
    }

    /// **逐字抄 Kotlin `WorkflowConvertTest.subgraphWorkflow()`**：本机真实存档的形状。
    ///
    /// 刻意让内部节点 id 与主图**撞号**（内部也有 1 号节点）、内部连线 id 与主图也不共用一套
    /// 编号 —— 这两件事真实文件里都会发生，正是"必须重新编号"的原因。
    fn kotlin_subgraph_workflow() -> Map<String, Value> {
        object(
            r#"{
              "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [],
                  "outputs": [{"name":"MODEL","type":"MODEL"}] },
                { "id": 10, "type": "11111111-2222-3333-4444-555555555555", "mode": 0,
                  "widgets_values_named": {"width": 768},
                  "inputs": [
                    {"name":"model","type":"MODEL","link":1},
                    {"name":"width","type":"INT","widget":{"name":"width"},"link":null}
                  ],
                  "outputs": [{"name":"LATENT","type":"LATENT","links":[2]}] },
                { "id": 20, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [
                    {"name":"model","type":"MODEL","link":3},
                    {"name":"latent_image","type":"LATENT","link":2}
                  ] }
              ],
              "links": [
                [1, 1, 0, 10, 0, "MODEL"],
                [2, 10, 0, 20, 1, "LATENT"],
                [3, 1, 0, 20, 0, "MODEL"]
              ],
              "definitions": { "subgraphs": [
                { "id": "11111111-2222-3333-4444-555555555555", "name": "新组节点",
                  "inputNode": {"id": -10}, "outputNode": {"id": -20},
                  "inputs": [
                    {"name":"model","type":"MODEL","linkIds":[1401]},
                    {"name":"width","type":"INT","linkIds":[1402]}
                  ],
                  "outputs": [ {"name":"LATENT","type":"LATENT","linkIds":[1403]} ],
                  "nodes": [
                    { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                      "widgets_values": [512, 512, 1],
                      "inputs": [{"name":"width","type":"INT","widget":{"name":"width"},"link":1402}],
                      "outputs": [{"name":"LATENT","type":"LATENT","links":[1403]}] },
                    { "id": 2, "type": "ModelPassthrough", "mode": 0, "widgets_values": [],
                      "inputs": [{"name":"model","type":"MODEL","link":1401}] }
                  ],
                  "links": [
                    {"id":1401,"origin_id":-10,"origin_slot":0,"target_id":2,"target_slot":0,"type":"MODEL"},
                    {"id":1402,"origin_id":-10,"origin_slot":1,"target_id":1,"target_slot":0,"type":"INT"},
                    {"id":1403,"origin_id":1,"origin_slot":0,"target_id":-20,"target_slot":0,"type":"LATENT"}
                  ] }
              ] }
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
        let converted = to_api_graph(&ui(), &info(), false).unwrap();
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
        let converted = to_api_graph(&workflow, &info(), false).unwrap();
        let inputs = &converted.graph["5"]["inputs"];
        assert_eq!(inputs["seed"], 777);
        assert_eq!(inputs["steps"], 8);
        assert!(inputs.get("sampler_name").is_none(), "named 那份只认声明过的键");
        assert!(
            converted.warnings.iter().any(|w| w.contains("unknown_widget")),
            "多出来的键要如实警告：{:?}",
            converted.warnings
        );
        // Kotlin 同一条用例还钉了一句：带名字的那份**不该再报「个数对不上」**
        assert!(
            !converted.warnings.iter().any(|w| w.contains("控件值有")),
            "带名字的那份不该再按位置数个数：{:?}",
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
        let converted = to_api_graph(&workflow, &info, false).unwrap();
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
        let converted = to_api_graph(&workflow, &info(), false).unwrap();
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
        let converted = to_api_graph(&workflow, &info(), false).unwrap();
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

    /// **逐字抄 Kotlin `WorkflowConvertTest.连线还原成上游节点与槽位 控件值按声明顺序贴回名字`**：
    /// `model ← [4,0]` / `seed 20260918` / `steps 30` / `denoise 1.0` / 宽高 896×1264。
    ///
    /// 为什么要照着抄：这些断言是**Kotlin 那边已经验过的预期** ——
    /// 拿它当"两份实现转换同一份文件"的判据，比我自己想一份图更接近真实存档的形状
    /// （六个节点、七个 link、`control_after_generate` 占位、optional 里的控件）。
    #[test]
    fn kotlin_case_classic_ui_workflow() {
        let object_info = kotlin_object_info();
        let ui = kotlin_ui_workflow();
        let converted = to_api_graph(&ui, &object_info, false).unwrap();
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
        let object_info = kotlin_object_info();
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "ForceInputNode", "mode": 0,
                             "widgets_values": ["hello"],
                             "inputs": [{"name":"value","link":9}] } ],
                "links": [[9, 2, 0, 1, 0, "STRING"]] }"#,
        );
        let converted = to_api_graph(&ui, &object_info, false).unwrap();
        assert_eq!(converted.graph["1"]["inputs"]["value"], json!([2, 0]));
        assert_eq!(converted.graph["1"]["inputs"]["note"], "hello");
    }

    /// 空图 / 脏数据：报错或跳过，**不许 panic**。
    #[test]
    fn empty_and_dirty_workflows_are_safe() {
        let empty = object(r#"{"nodes": [], "links": []}"#);
        assert!(to_api_graph(&empty, &info(), false).is_err());
        let dirty = object(
            r#"{"nodes": ["nope", {"id": 1, "type": "Unknown Thing"}], "links": [null, 7]}"#,
        );
        let converted = to_api_graph(&dirty, &info(), false).unwrap();
        assert!(converted.unsupported_nodes.iter().any(|u| u.contains("Unknown Thing")));
        assert!(converted.graph.is_empty());
    }

    #[test]
    fn ui_detection_only_looks_at_nodes() {
        assert!(is_ui_workflow(&object(r#"{"nodes": []}"#)));
        assert!(!is_ui_workflow(&object(r#"{"1": {"class_type": "KSampler"}}"#)));
        assert!(!is_ui_workflow(&object(r#"{"nodes": "nope"}"#)));
    }

    // -----------------------------------------------------------------------
    //  逐字抄 Kotlin `WorkflowConvertTest` 的其余断言（第 68 步补齐）
    // -----------------------------------------------------------------------

    /// Kotlin `备注与原始值节点被丢掉 不会进提示词`。
    #[test]
    fn kotlin_case_note_like_nodes_are_dropped() {
        // ⚠ 用 `r##"…"##`：正文里有 `"# 标题"`，`r#"…"#` 会在那个 `"#` 处提前收尾
        let ui = object(
            r##"{ "nodes": [
                { "id": 1, "type": "Note", "mode": 0, "widgets_values": ["写点说明"] },
                { "id": 2, "type": "MarkdownNote", "mode": 0, "widgets_values": ["# 标题"] },
                { "id": 3, "type": "PrimitiveNode", "mode": 0, "widgets_values": [42] },
                { "id": 4, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512, 1] }
              ],
              "links": [] }"##,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.node_count, 1, "界面专用节点不该进 API 图");
        assert!(converted.graph.contains_key("4"));
        assert!(!converted.graph.contains_key("1"));
        assert!(converted.rewritten.iter().any(|r| r.contains("Note")), "{:?}", converted.rewritten);
    }

    /// Kotlin `纯前端节点转换不了就如实报错 不猜`（`emulateFrontend=false` 的那一档）。
    #[test]
    fn kotlin_case_pure_frontend_node_is_reported_when_not_emulating() {
        let ui = object(
            r#"{ "nodes": [
                { "id": 1, "type": "Anything Everywhere", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"anything","link":1}] },
                { "id": 2, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [512, 512, 1] }
              ],
              "links": [[1, 2, 0, 1, 0, "LATENT"]] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.unsupported_nodes.len(), 1, "{:?}", converted.unsupported_nodes);
        assert!(
            converted.unsupported_nodes[0].contains("Anything Everywhere"),
            "{:?}",
            converted.unsupported_nodes
        );
        // 转不出来的节点**不进**图（宁可整体失败，也不要半个提示词）
        assert!(!converted.graph.contains_key("1"));
    }

    /// Kotlin `没有节点定义的自定义节点也如实报错`。
    #[test]
    fn kotlin_case_unknown_custom_node_is_reported() {
        let ui = object(
            r#"{ "nodes": [ { "id": 7, "type": "MyCustomNode", "mode": 0, "widgets_values": [1] } ],
                "links": [] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.unsupported_nodes, vec!["7 MyCustomNode".to_string()]);
    }

    /// Kotlin `控件值个数对不上时给警告 但仍然转出来`。
    #[test]
    fn kotlin_case_widget_count_mismatch_warns_but_still_converts() {
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                             "widgets_values": [512, 512, 1, "多出来的一个"] } ],
                "links": [] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.node_count, 1);
        assert!(
            converted.warnings.iter().any(|w| w.contains("控件值")),
            "{:?}",
            converted.warnings
        );
    }

    /// Kotlin `控件值存成对象时按名字取`。
    #[test]
    fn kotlin_case_widgets_values_as_object() {
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "EmptyLatentImage", "mode": 0,
                             "widgets_values": {"width": 1024, "height": 1024, "batch_size": 2} } ],
                "links": [] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.graph["1"]["inputs"]["width"], 1024);
        assert_eq!(converted.graph["1"]["inputs"]["batch_size"], 2);
    }

    /// Kotlin `新写法 links 对象也认`。
    #[test]
    fn kotlin_case_new_style_object_links() {
        let ui = object(
            r#"{ "nodes": [
                { "id": 6, "type": "CLIPTextEncode", "mode": 0, "widgets_values": ["hi"], "inputs": [] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"positive","link":4}] }
              ],
              "links": [ {"id": 4, "origin_id": 6, "origin_slot": 0, "target_id": 3, "target_slot": 1} ] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.graph["3"]["inputs"]["positive"], json!([6, 0]));
    }

    /// Kotlin `旁路的输出找不到同类型输入时留警告 不静默`。
    #[test]
    fn kotlin_case_bypass_without_same_type_input_warns() {
        let ui = object(
            r#"{ "nodes": [
                { "id": 2, "type": "ModelPassthrough", "mode": 4, "widgets_values": [],
                  "inputs": [], "outputs": [{"name":"MODEL","type":"MODEL"}] }
              ],
              "links": [] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert!(
            converted.warnings.iter().any(|w| w.contains("旁路") && w.contains("同类型")),
            "接不过去必须说清楚：{:?}",
            converted.warnings
        );
    }

    /// Kotlin `Reroute 与 SetNode GetNode 把连线接到真正的上游`（`GetNode → SetNode → Reroute → 1` 一路穿透）。
    #[test]
    fn kotlin_case_reroute_set_get_chain() {
        let ui = object(
            r#"{ "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
                  "widgets_values": ["a.safetensors"], "inputs": [] },
                { "id": 2, "type": "Reroute", "mode": 0, "widgets_values": [],
                  "inputs": [{"name":"","link":10}] },
                { "id": 3, "type": "SetNode", "mode": 0, "widgets_values": ["MODEL_BASE"],
                  "inputs": [{"name":"MODEL","link":11}] },
                { "id": 4, "type": "GetNode", "mode": 0, "widgets_values": ["MODEL_BASE"],
                  "inputs": [] },
                { "id": 5, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"model","link":12}] }
              ],
              "links": [
                [10, 1, 0, 2, 0, "MODEL"],
                [11, 2, 0, 3, 0, "MODEL"],
                [12, 4, 0, 5, 0, "MODEL"]
              ] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert!(converted.unsupported_nodes.is_empty(), "{:?}", converted.unsupported_nodes);
        assert_eq!(converted.graph["5"]["inputs"]["model"], json!([1, 0]));
        assert!(!converted.graph.contains_key("2"));
        assert!(!converted.graph.contains_key("3"));
        assert!(!converted.graph.contains_key("4"));
    }

    /// Kotlin `静音节点留下的悬挂输入要点名报出来（不然 ComfyUI 只会说节点不存在）`。
    #[test]
    fn kotlin_case_muted_node_leaves_a_named_dangling_input() {
        let ui = object(
            r#"{ "nodes": [
                { "id": 1, "type": "CLIPTextEncode", "mode": 2, "widgets_values": ["不跑"], "inputs": [] },
                { "id": 3, "type": "KSampler", "mode": 0,
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal", 1.0],
                  "inputs": [{"name":"positive","type":"CONDITIONING","link":8}] }
              ],
              "links": [[8, 1, 0, 3, 1, "CONDITIONING"]] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert_eq!(converted.unresolved_inputs, vec!["3.positive ← 节点 1（没有转换出来）".to_string()]);
    }

    /// Kotlin `空着的连线型输入列成清单（纯前端广播节点原来就是往这些地方灌值的）`。
    #[test]
    fn kotlin_case_open_inputs_list_only_sockets() {
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "ForceInputNode", "mode": 0,
                             "widgets_values": ["hello"], "inputs": [] } ],
                "links": [] }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        // value 是 forceInput（插口）：空着 = 缺输入，要列出来
        assert!(converted.open_inputs.contains(&"1.value (STRING)".to_string()), "{:?}", converted.open_inputs);
        // note 是普通控件：空着时 ComfyUI 用默认值，不算缺口
        assert!(!converted.open_inputs.iter().any(|i| i.contains("note")), "{:?}", converted.open_inputs);
    }

    // -----------------------------------------------------------------------
    //  组节点（subgraph）展开 —— Kotlin `flatten` 的对照用例（第 68 步）
    // -----------------------------------------------------------------------

    /// Kotlin `组节点按定义展开：内部节点换新号 连线与控件值都接对 输出穿透到真正的产出节点`。
    #[test]
    fn kotlin_case_subgraph_is_flattened() {
        let converted = to_api_graph(&kotlin_subgraph_workflow(), &kotlin_object_info(), false).unwrap();

        assert!(
            converted.unsupported_nodes.is_empty(),
            "组节点该被展开而不是报错：{:?}",
            converted.unsupported_nodes
        );
        // 顶层 1（CheckpointLoaderSimple）+ 20（KSampler）+ 展开出来的两个内部节点
        assert_eq!(converted.node_count, 4);
        assert!(!converted.graph.contains_key("10"), "被展开的组节点实例自己不该再出现");
        assert!(
            converted.rewritten.iter().any(|r| r.contains("组节点")),
            "{:?}",
            converted.rewritten
        );

        // 内部节点拿到的是没人用过的新号（内部 id 与主图撞号，必须重编）
        let (inner_latent_id, inner_latent) = converted
            .graph
            .iter()
            .find(|(id, value)| {
                *id != "1" && *id != "20" && value["class_type"] == json!("EmptyLatentImage")
            })
            .expect("内部 EmptyLatentImage 要展开出来");
        let inner_model = converted
            .graph
            .values()
            .find(|value| value["class_type"] == json!("ModelPassthrough"))
            .expect("内部 ModelPassthrough 要展开出来");

        // ① 实例上的控件值（width=768）要填进内部节点的对应输入，而不是走控件顺序
        assert_eq!(
            inner_latent["inputs"]["width"],
            json!(768),
            "组节点把 width 暴露到了外层，值必须原样传进去"
        );
        // ② 组节点的"连线型输入"要接上主图那边真正的来源
        assert_eq!(
            inner_model["inputs"]["model"],
            json!([1, 0]),
            "组节点输入喂进来的 MODEL 应该接回主图的 CheckpointLoaderSimple"
        );
        // ③ 组节点的输出要穿透到内部真正产出它的节点
        let inner_id: i64 = inner_latent_id.parse().unwrap();
        assert_eq!(
            converted.graph["20"]["inputs"]["latent_image"],
            json!([inner_id, 0]),
            "下游拿到的必须是内部那个 EmptyLatentImage"
        );
        assert!(converted.unresolved_inputs.is_empty(), "{:?}", converted.unresolved_inputs);
    }

    /// Kotlin `组节点里的组节点这一版不展开 如实报错（不猜）`。
    #[test]
    fn kotlin_case_nested_subgraph_is_reported() {
        let ui = object(
            r#"{ "nodes": [ { "id": 10, "type": "aaaaaaaa-0000-0000-0000-000000000000", "mode": 0,
                             "widgets_values": [], "inputs": [] } ],
                "links": [],
                "definitions": { "subgraphs": [
                  { "id": "aaaaaaaa-0000-0000-0000-000000000000",
                    "inputNode": {"id": -10}, "outputNode": {"id": -20},
                    "inputs": [], "outputs": [],
                    "nodes": [ { "id": 1, "type": "bbbbbbbb-0000-0000-0000-000000000000" } ],
                    "links": [] },
                  { "id": "bbbbbbbb-0000-0000-0000-000000000000",
                    "inputNode": {"id": -10}, "outputNode": {"id": -20},
                    "inputs": [], "outputs": [], "nodes": [], "links": [] }
                ] } }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert!(
            converted.unsupported_nodes.iter().any(|u| u.contains("还套着一层组节点")),
            "{:?}",
            converted.unsupported_nodes
        );
    }

    /// 组节点定义里**缺输入 / 输出代理**（或内部一个节点都没有）→ 如实报错，不许猜着展开。
    ///
    /// Kotlin 的第一档失败分支：这三件事只要中一条就没法把实例的输入 / 输出对上内部节点。
    #[test]
    fn subgraph_without_proxies_is_reported_not_guessed() {
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "def-uuid-1", "inputs": [], "outputs": [] },
                            { "id": 2, "type": "CheckpointLoaderSimple",
                              "widgets_values": ["krea2.safetensors"] } ],
                "links": [],
                "definitions": { "subgraphs": [
                  { "id": "def-uuid-1", "inputs": [], "outputs": [],
                    "nodes": [ {"id": 1, "type": "EmptyLatentImage", "widgets_values": [512,512,1]} ],
                    "links": [] } ] } }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert!(
            converted.unsupported_nodes.iter().any(|u| u.contains("缺少输入 / 输出代理")),
            "{:?}",
            converted.unsupported_nodes
        );
        assert!(!converted.runnable(), "有组节点没展开 → 不许当成能跑");
        assert!(converted.graph.contains_key("2"), "别的节点照样要转换出来");
    }

    /// 组节点实例的输入个数与定义**对不上** → 如实报错（对不齐就会把线接到错的输入上）。
    #[test]
    fn subgraph_with_mismatched_input_count_is_reported() {
        let ui = object(
            r#"{ "nodes": [ { "id": 1, "type": "def-uuid-1", "inputs": [],
                              "outputs": [{"name":"LATENT","type":"LATENT"}] } ],
                "links": [],
                "definitions": { "subgraphs": [
                  { "id": "def-uuid-1", "inputNode": {"id": -10}, "outputNode": {"id": -20},
                    "inputs": [ {"name":"width","type":"INT"},
                                {"name":"height","type":"INT"} ],
                    "outputs": [],
                    "nodes": [ {"id": 1, "type": "EmptyLatentImage", "widgets_values": [512,512,1]} ],
                    "links": [] } ] } }"#,
        );
        let converted = to_api_graph(&ui, &kotlin_object_info(), false).unwrap();
        assert!(
            converted.unsupported_nodes.iter().any(|u| u.contains("对不上，不敢猜着展开")),
            "{:?}",
            converted.unsupported_nodes
        );
    }

    // -----------------------------------------------------------------------
    //  ## 模拟用户行为 ##（`emulateFrontend`）—— 第 68 步补上的第二处欠账
    // -----------------------------------------------------------------------

    /// 这份夹具刻意同时包含三种情形，好让"开 / 关"两种模式**逐条比差异**：
    ///
    /// * ① 空着的**连线型**输入（`Anything Everywhere` 的作业本）：`5.positive` / `5.negative`；
    /// * ② 显示节点 `7 PreviewImage`（只负责"看一眼"）；
    /// * ③ 一个输出节点都没有（`6 VAEDecode` 出图但没人存）→ 前端会自己挂 `SaveImage`。
    ///
    /// 广播节点**只在界面格式里**存在（`/object_info` 里没有它）—— 这正是它被当作虚拟节点的原因。
    fn emulate_workflow_object_info() -> Map<String, Value> {
        object(
            r#"{
              "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [["a.safetensors"]] } },
                "output": ["MODEL"] },
              "CLIPTextEncode": { "input": { "required": { "text": ["STRING"], "clip": ["CLIP"] } },
                "output": ["CONDITIONING"] },
              "EmptyLatentImage": { "input": { "required": {
                  "width": ["INT"], "height": ["INT"], "batch_size": ["INT"] } },
                "output": ["LATENT"] },
              "KSampler": { "input": { "required": {
                  "model": ["MODEL"],
                  "seed": ["INT", {"control_after_generate": true}],
                  "steps": ["INT"], "cfg": ["FLOAT"],
                  "sampler_name": [["euler"]], "scheduler": [["normal"]],
                  "positive": ["CONDITIONING"], "negative": ["CONDITIONING"],
                  "latent_image": ["LATENT"] } },
                "output": ["LATENT"] },
              "VAEDecode": { "input": { "required": { "samples": ["LATENT"] } }, "output": ["IMAGE"] }
            }"#,
        )
    }

    fn emulate_workflow() -> Map<String, Value> {
        object(
            r#"{
              "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["a.safetensors"] },
                { "id": 2, "type": "CLIPTextEncode",
                  "inputs": [{"name":"clip","type":"CLIP","link":2}],
                  "widgets_values": ["1girl"] },
                { "id": 4, "type": "EmptyLatentImage", "widgets_values": [512, 512, 1] },
                { "id": 5, "type": "KSampler",
                  "inputs": [
                    {"name":"model","type":"MODEL","link":1},
                    {"name":"latent_image","type":"LATENT","link":6}],
                  "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal"] },
                { "id": 6, "type": "VAEDecode",
                  "inputs": [{"name":"samples","type":"LATENT","link":7}] },
                { "id": 7, "type": "PreviewImage",
                  "inputs": [{"name":"images","type":"IMAGE","link":8}] },
                { "id": 14, "type": "Anything Everywhere",
                  "inputs": [{"name":"anything","link":9}],
                  "properties": { "Input Type": "CONDITIONING" } }
              ],
              "links": [
                [1, 1, 0, 5, 0, "MODEL"],
                [2, 1, 1, 2, 0, "CLIP"],
                [6, 4, 0, 5, 3, "LATENT"],
                [7, 5, 0, 6, 0, "LATENT"],
                [8, 6, 0, 7, 0, "IMAGE"],
                [9, 2, 0, 14, 0, "CONDITIONING"]
              ]
            }"#,
        )
    }

    /// **`emulateFrontend=true` vs `false`：两种模式在同一个图上逐条比差异**。
    #[test]
    fn kotlin_case_emulate_frontend_changes_the_graph() {
        let object_info = emulate_workflow_object_info();
        let ui = emulate_workflow();

        // --- emulateFrontend = false：一个字都不改，只如实报缺口 ---
        let raw = to_api_graph(&ui, &object_info, false).unwrap();
        assert!(!raw.runnable(), "纯前端节点还在 unsupported 里");
        assert!(
            raw.unsupported_nodes.iter().any(|u| u.contains("Anything Everywhere")),
            "{:?}",
            raw.unsupported_nodes
        );
        assert!(
            raw.unsupported_nodes.iter().any(|u| u.contains("PreviewImage")),
            "{:?}",
            raw.unsupported_nodes
        );
        assert_eq!(raw.graph["5"]["inputs"].get("positive"), None, "关着就不许自己灌值");
        assert!(
            raw.open_inputs.iter().any(|input| input.contains("5.positive (CONDITIONING)")),
            "{:?}",
            raw.open_inputs
        );
        assert!(
            raw.graph.values().all(|value| value["class_type"] != json!("SaveImage")),
            "关着就不许自己补输出节点"
        );
        assert!(
            raw.frontend_rewritten.is_empty()
                && raw.broadcast_lines.is_empty()
                && raw.injected_nodes.is_empty(),
            "关着时「替前端做了什么」这份清单必须是空的"
        );
        assert_eq!(raw.node_count, 5);

        // --- emulateFrontend = true：照 ComfyUI 前端排队前的行为等价改写 ---
        let emulated = to_api_graph(&ui, &object_info, true).unwrap();
        assert!(emulated.unsupported_nodes.is_empty(), "{:?}", emulated.unsupported_nodes);
        assert!(emulated.runnable(), "{:?}", emulated.unsupported_nodes);
        // ① 广播：上游那一路（节点 2 的 CONDITIONING）填进所有同类型且空着的输入
        assert_eq!(emulated.graph["5"]["inputs"]["positive"], json!([2, 0]));
        assert_eq!(emulated.graph["5"]["inputs"]["negative"], json!([2, 0]));
        assert_eq!(emulated.broadcast_lines.len(), 2, "{:?}", emulated.broadcast_lines);
        assert!(
            emulated.broadcast_lines.iter().any(|line| line.contains("5.positive ← 节点 14")),
            "{:?}",
            emulated.broadcast_lines
        );
        // 灌完之后"还空着"的清单要重算（不然模型会去补一根已经接好的线）
        assert!(
            !emulated.open_inputs.iter().any(|input| input.contains("5.positive")),
            "{:?}",
            emulated.open_inputs
        );
        // ② 显示节点：丢掉并如实报告
        assert!(
            emulated.frontend_rewritten.iter().any(|line| line.contains("Anything Everywhere（广播 2 处）")),
            "{:?}",
            emulated.frontend_rewritten
        );
        assert!(
            emulated.frontend_rewritten.iter().any(|line| line.contains("显示节点，已忽略")),
            "{:?}",
            emulated.frontend_rewritten
        );
        // ③ 一个输出节点都没有 → 照前端的做法补一个 SaveImage，接在产出 IMAGE 的那个节点上
        assert_eq!(emulated.injected_nodes.len(), 1, "{:?}", emulated.injected_nodes);
        assert_eq!(emulated.graph["7"]["class_type"], json!("SaveImage"));
        assert_eq!(emulated.graph["7"]["inputs"]["images"], json!([6, 0]));
        assert_eq!(emulated.graph["7"]["inputs"]["filename_prefix"], json!("ComfyHub"));
        assert_eq!(emulated.node_count, 6, "补完之后节点数要多一个");
    }

    /// 判据保守：**说不清类型的广播不猜**，照旧报 unsupported（Kotlin 同）。
    #[test]
    fn emulate_does_not_guess_an_undeclared_broadcast_type() {
        let object_info = emulate_workflow_object_info();
        let mut ui = emulate_workflow();
        let nodes = ui.get_mut("nodes").unwrap().as_array_mut().unwrap();
        let broadcaster = nodes.iter_mut().find(|node| node["id"] == 14).unwrap();
        broadcaster.as_object_mut().unwrap().remove("properties");
        let emulated = to_api_graph(&ui, &object_info, true).unwrap();
        assert_eq!(emulated.unsupported_nodes.len(), 1, "{:?}", emulated.unsupported_nodes);
        assert!(
            emulated.unsupported_nodes[0].contains("类型 未声明：全图没有对得上的空输入"),
            "{:?}",
            emulated.unsupported_nodes
        );
        assert!(emulated.broadcast_lines.is_empty());
        assert_eq!(emulated.graph["5"]["inputs"].get("positive"), None, "不猜就不许灌值");
    }

    /// 广播节点自己**没接上游**时：没有东西可广播，如实报（前端这时候也不会灌值）。
    #[test]
    fn emulate_reports_a_broadcaster_without_upstream() {
        let object_info = emulate_workflow_object_info();
        let mut ui = emulate_workflow();
        let nodes = ui.get_mut("nodes").unwrap().as_array_mut().unwrap();
        let broadcaster = nodes.iter_mut().find(|node| node["id"] == 14).unwrap();
        broadcaster.as_object_mut().unwrap().insert("inputs".into(), json!([]));
        let emulated = to_api_graph(&ui, &object_info, true).unwrap();
        assert_eq!(
            emulated.unsupported_nodes,
            vec!["14 Anything Everywhere（没有接上游，没有东西可以广播）".to_string()]
        );
        assert!(emulated.broadcast_lines.is_empty());
    }

    /// 一条线都不想被替前端接的调用方：`emulateFrontend=false` 时**连显示节点都不动**，
    /// 结果里也不会冒出任何"前端改写清单"（工具描述里那半句"一个字都不改"说的就是这个）。
    #[test]
    fn emulate_off_leaves_the_frontend_report_empty() {
        let emulated = to_api_graph(&emulate_workflow(), &emulate_workflow_object_info(), false).unwrap();
        assert!(emulated.frontend_rewritten.is_empty());
        assert!(emulated.broadcast_lines.is_empty());
        assert!(emulated.injected_nodes.is_empty());
        assert_eq!(emulated.node_count, 5);
    }
}
