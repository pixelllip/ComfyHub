//! ComfyUI **界面格式工作流**（`{nodes,links}`）→ **API 格式节点图**（Kotlin `WorkflowConvert` 的
//! Rust 端逐行对照，P4 第 44 步；**第 68 步**补齐最后两处：组节点展开 + 「模拟前端行为」）。
//!
//! # 必须存在的依据
//!
//! 指定本机一份工作流文件让 AI 执行，而 `comfy_submit` 只接受库中的 `promptId` ——
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
//! | 广播型 / 显示型纯前端节点（`Anything Everywhere` 这类） | **`emulateFrontend=true` 时等价改写**（见 [`crate::workflow::workflow_frontend`]）；`false` 时留在 `unsupportedNodes` 里如实报 |
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
//! ## 「模拟前端行为」（`emulateFrontend`）所在文件
//!
//! 判据（广播型 / 显示型两张表 + 三条模式）只有一份：`workflow_audit.rs` 的
//! [`crate::workflow::workflow_audit::is_broadcast`] / [`crate::workflow::workflow_audit::is_display`]；改写本身
//! 在 [`crate::workflow::workflow_frontend`]（Kotlin `FrontendNodes.kt` 的对照）。
//!
//! # 缺口清单（宁可失败，但不卡死）
//!
//! `unresolvedInputs` = 指向"没转换出来的节点"的悬挂引用（提交上去 ComfyUI 只会报
//! "节点 xxx 不存在"，无法判断哪一步出错）；
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
    /// 其中**必填**的那一部分（可选插口空着是常态，不算缺口）。
    ///
    /// ⚠ 判据只有一份（[`crate::workflow::workflow_frontend::required_open_inputs_of`]）：
    /// 配方学习、蓝图体检、提交前的缺口清单都读它 —— 各判一遍的下场是
    /// "蓝图说没事、学习说缺件"，而两边都不会报错（2026-10-17 实测到过）。
    pub required_open_inputs: Vec<String>,
    /// ## 模拟前端行为 ## 等价改写的前端节点，形如 `12 Anything Everywhere（广播 3 处）`。
    ///
    /// 只有 `emulateFrontend=true` 才有内容。改动可见，才能判断本次
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
const FRONTEND_VIRTUAL: [&str; 6] = [
    "Note",
    "MarkdownNote",
    "PrimitiveNode",
    "Reroute",
    "SetNode",
    "GetNode",
];

/// 只在前端存在的**直通**节点（1 进 1 出地把上游那一路递下去）。
///
/// rgthree 那几个开关（`Bypasser` / `Muter` / `Fast Groups …`）都继承同一个前端基类：
/// 基类在 `onConstructed` 里 `addOutput("OPT_CONNECTION", "*")` —— 它的语义就是
/// "把上游那一路直接接到下游"，**只在前端成立**（rgthree 的 Python 包里根本没有这些类）。
///
/// ⚠ 判据来自真实提交记录（2026-10-17 从本机 ComfyUI `/history` 取回的 i2i 运行）：
/// 界面里 `105.model ← 90（Fast Bypasser）← 89`，而**服务端收到的是 `105.model = ["89", 0]`** ——
/// 前端确实是把这条线穿透过去的。不做这一步的表现是 `comfy_load_workflow` 直接报
/// `105.model ← 节点 90（未转换出来）`，i2i 工作流**整体无法导入**。
///
/// 形状不对（不是恰好 1 个接了线的输入）就**不猜**：照旧留在 `unsupported` 里。
const FRONTEND_PASSTHROUGH: [&str; 3] = [
    "Fast Bypasser (rgthree)",
    "Fast Groups Bypasser (rgthree)",
    "Fast Groups Muter (rgthree)",
];

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
/// 「模拟前端行为」（[`crate::workflow::workflow_frontend::emulate`]）—— 广播型节点按声明的
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
            other if FRONTEND_PASSTHROUGH.contains(&other) => {
                // 直通开关：把下游接到它那个唯一接了线的上游上（见 `FRONTEND_PASSTHROUGH` 的注释）
                let linked = linked_input_count(node, &links);
                match (linked, first_linked_input(node, &links)) {
                    (0, _) => rewritten.push(format!("{id} {class}（只是开关，没有接上游）")),
                    (1, Some(source)) => {
                        redirects.insert((id.clone(), 0), resolve(source, &redirects));
                        rewritten.push(format!("{id} {class}（直通）"));
                    }
                    (count, _) => unsupported.push(format!(
                        "{id} {class}（有 {count} 个接了线的输入，判不出该透传哪一路）"
                    )),
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
        if (FRONTEND_VIRTUAL.contains(&class.as_str())
            || FRONTEND_PASSTHROUGH.contains(&class.as_str()))
            && object_info.get(&class).is_none()
        {
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
                let Some(entry) = entry.as_object() else {
                    continue;
                };
                let Some(name) = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.trim().is_empty())
                else {
                    continue;
                };
                if inputs.contains_key(name) {
                    continue;
                }
                let Some(link_id) = as_int(entry.get("link")) else {
                    continue;
                };
                let Some(reference) = links.get(&link_id) else {
                    continue;
                };
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
        fill_widgets(
            node,
            &widget_specs(info),
            &mut inputs,
            &mut warnings,
            &id,
            &class,
        );

        let mut node_out = Map::new();
        node_out.insert("class_type".into(), json!(class));
        node_out.insert("inputs".into(), Value::Object(inputs.clone()));
        if let Some(title) = node
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
        {
            node_out.insert("_meta".into(), json!({ "title": title }));
        }
        out.insert(id.clone(), Value::Object(node_out));
        collect_open_inputs(info, &inputs, &id, &mut open_inputs);
    }

    // --- 第三遍：## 模拟前端行为 ##（`emulateFrontend`，Kotlin `FrontendNodes.emulate`）---
    //
    // 到这一步为止，纯前端节点还都躺在 `unsupported` 里。开着就做"前端 JS 在排队前会做的
    // 那几件事"：广播型节点按它声明的类型 / 名字过滤把上游那一路**填空**、显示节点丢掉、
    // 一个输出节点都没有时补一个 `SaveImage`。判据一律保守：说不清类型的广播**不猜**，
    // 照旧留在 unsupported 里（出口还是一样）。
    let mut final_graph = out;
    let mut final_open = open_inputs;
    let required_open: Vec<String>;
    let mut frontend_rewritten: Vec<String> = Vec::new();
    let mut broadcast_lines: Vec<String> = Vec::new();
    let mut injected_nodes: Vec<String> = Vec::new();
    if emulate_frontend {
        let (emulated, report) = crate::workflow::workflow_frontend::emulate(
            &raw_nodes,
            object_info,
            &final_graph,
            &links,
        );
        final_graph = emulated;
        // 广播灌完之后"还空着"的清单要重算：不然模型会去补一根已经接好的线
        final_open = crate::workflow::workflow_frontend::open_inputs_of(object_info, &final_graph);
        required_open =
            crate::workflow::workflow_frontend::required_open_inputs_of(object_info, &final_graph);
        frontend_rewritten = report.rewritten;
        broadcast_lines = report.broadcast;
        injected_nodes = report.injected;
        // 已经等价改写掉的节点不该再挂在 unsupported 里
        unsupported.retain(|line| {
            !report
                .handled_ids
                .iter()
                .any(|handled_id| line.starts_with(&format!("{handled_id} ")))
        });
        unsupported.extend(report.unsupported);
    } else {
        required_open =
            crate::workflow::workflow_frontend::required_open_inputs_of(object_info, &final_graph);
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
        required_open_inputs: required_open,
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
    let top_links = raw_links(ui.get("links"));

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
        let input_proxy = def
            .get("inputNode")
            .and_then(Value::as_object)
            .and_then(node_id);
        let output_proxy = def
            .get("outputNode")
            .and_then(Value::as_object)
            .and_then(node_id);
        let iface: Vec<&Map<String, Value>> = object_list(def.get("inputs"));
        let inst_inputs: Vec<&Map<String, Value>> = object_list(node.get("inputs"));
        let inner_nodes: Vec<&Map<String, Value>> = object_list(def.get("nodes"));

        // 失败的三档：**带具体原因**报 unsupported，并登记 handled（第二遍跳过它）
        if input_proxy.is_none() || output_proxy.is_none() || inner_nodes.is_empty() {
            unsupported.push(format!(
                "{id} {short}…（组节点：定义里缺少输入 / 输出代理，不敢猜着展开）"
            ));
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
        if inner_nodes
            .iter()
            .any(|inner| defs.contains_key(&ui_class(inner)))
        {
            unsupported.push(format!(
                "{id} {short}…（组节点里还套着一层组节点，这一版不展开）"
            ));
            handled.push(id.clone());
            nodes.push(node.clone());
            continue;
        }

        let inner_links = raw_links(def.get("links"));
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
            let name = iface
                .get(slot)
                .and_then(|decl| decl.get("name"))
                .and_then(primitive_text);
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
            let Some(new_link) = renumber.get(old_link).copied() else {
                continue;
            };
            if Some(link.origin.as_str()) == input_proxy.as_deref() {
                let source = source_of_slot
                    .get(link.origin_slot as usize)
                    .and_then(Clone::clone);
                let widget = widget_of_slot
                    .get(link.origin_slot as usize)
                    .and_then(Clone::clone);
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
                let origin = id_map
                    .get(&link.origin)
                    .cloned()
                    .unwrap_or_else(|| link.origin.clone());
                links.insert(new_link, (origin, link.origin_slot));
            }
            if Some(link.target.as_str()) == output_proxy.as_deref() {
                let origin = id_map
                    .get(&link.origin)
                    .cloned()
                    .unwrap_or_else(|| link.origin.clone());
                redirects.insert((id.clone(), link.target_slot), (origin, link.origin_slot));
            }
        }

        // 内部节点本体：换号、重指连线、把外层控件的值填进对应输入
        for inner in &inner_nodes {
            let Some(raw) = node_id(inner) else { continue };
            let Some(new_id) = id_map.get(&raw).cloned() else {
                continue;
            };
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
        rewritten.push(format!(
            "{id} {short}…（组节点，展开成 {} 个节点）",
            inner_nodes.len()
        ));
    }

    Flat {
        nodes,
        links,
        redirects,
        handled,
        forced_inputs,
        rewritten,
        unsupported,
        warnings,
    }
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
        let Some(def) = item.as_object() else {
            continue;
        };
        let Some(id) = def.get("id").and_then(primitive_text) else {
            continue;
        };
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
///
/// `type_name` 是 litegraph 记在连线上的**类型字符串**（`"MODEL"` / `"LATENT"`…）：
/// 转换本身不看它（转换按输入名对），但蓝图要拿它跟"目标输入声明的类型"对一遍
/// （见 [`crate::workflow::workflow_blueprint`] 的类型检查）。
pub(crate) struct RawLink {
    pub origin: String,
    pub origin_slot: i64,
    pub target: String,
    pub target_slot: i64,
    pub type_name: Option<String>,
}

/// `links` 表：两种写法都认（老写法是数组、新写法是对象）。
///
/// ⚠ 两种写法**必须都认**：老版本 ComfyUI 存数组、新版本存对象，
/// 只认一种的下场是"某一份文件里一条线都读不出来"，而那种失败**不报错**。
/// 判据只有这一份（`flatten` 与蓝图共用）。
pub(crate) fn raw_links(raw: Option<&Value>) -> HashMap<i64, RawLink> {
    let mut out: HashMap<i64, RawLink> = HashMap::new();
    let Some(list) = raw.and_then(Value::as_array) else {
        return out;
    };
    for entry in list {
        match entry {
            // [link_id, origin_id, origin_slot, target_id, target_slot, type]
            Value::Array(items) => {
                let Some(link_id) = as_int(items.first()) else {
                    continue;
                };
                let Some(origin) = items.get(1).and_then(primitive_text) else {
                    continue;
                };
                out.insert(
                    link_id,
                    RawLink {
                        origin,
                        origin_slot: as_int(items.get(2)).unwrap_or(0),
                        target: items.get(3).and_then(primitive_text).unwrap_or_default(),
                        target_slot: as_int(items.get(4)).unwrap_or(0),
                        type_name: items.get(5).and_then(primitive_text),
                    },
                );
            }
            // {id, origin_id, origin_slot, target_id, target_slot, …}
            Value::Object(obj) => {
                let Some(link_id) = as_int(obj.get("id")) else {
                    continue;
                };
                let Some(origin) = obj.get("origin_id").and_then(primitive_text) else {
                    continue;
                };
                out.insert(
                    link_id,
                    RawLink {
                        origin,
                        origin_slot: as_int(obj.get("origin_slot")).unwrap_or(0),
                        target: obj
                            .get("target_id")
                            .and_then(primitive_text)
                            .unwrap_or_default(),
                        target_slot: as_int(obj.get("target_slot")).unwrap_or(0),
                        type_name: obj.get("type").and_then(primitive_text),
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
                    Some((
                        index,
                        entry.get("type").and_then(primitive_text),
                        source.clone(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    for (slot, output) in outputs.iter().enumerate() {
        let output_type = output.get("type").and_then(primitive_text);
        let same_slot = linked_inputs
            .iter()
            .find(|(index, input_type, _)| *index == slot && *input_type == output_type);
        let candidate = same_slot.or_else(|| {
            linked_inputs
                .iter()
                .find(|(_, input_type, _)| *input_type == output_type)
        });
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
fn consumers_of(id: &str, nodes: &[Map<String, Value>], links: &LinkTable) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in nodes {
        let Some(inputs) = node.get("inputs").and_then(Value::as_array) else {
            continue;
        };
        for entry in inputs {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let Some(link_id) = as_int(entry.get("link")) else {
                continue;
            };
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
    //
    //    ⚠ 一个例外：**动态下拉框的子项**。`COMFY_DYNAMICCOMBO_V3` 那种声明里只写了
    //    `sampling_mode`，它的子项（`sampling_mode.temperature` / `.top_k` …）要等
    //    服务端按选中的那一项展开之后才存在，**名字就是带点的**。判据不是猜的：
    //    本机 ComfyUI 的真实提交记录里，`TextGenerate` 的 `inputs` 长的就是
    //    `{"sampling_mode":"on","sampling_mode.temperature":0.7,…}`（见
    //    `docs/pitfalls.md` 的那条实测）。丢掉它们会让这一路退回默认值（或者直接缺必填）。
    if let Some(named) = node.get("widgets_values_named").and_then(Value::as_object) {
        if !named.is_empty() {
            for (name, value) in named {
                if inputs.contains_key(name) || value.is_null() {
                    continue;
                }
                let known = specs.iter().any(|spec| &spec.name == name)
                    || specs
                        .iter()
                        .any(|spec| spec.dynamic && is_dynamic_child(&spec.name, name));
                if known {
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
            let pending: Vec<&WidgetSpec> = specs
                .iter()
                .filter(|spec| !inputs.contains_key(&spec.name))
                .collect();
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
                let Some(value) = values.get(index) else {
                    break;
                };
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
    /// 动态下拉框（`COMFY_DYNAMICCOMBO_*`）：它的子项名字是 `名字.子项`
    dynamic: bool,
}

/// `parent` 这个动态下拉框的子项名（`sampling_mode.temperature`）。
fn is_dynamic_child(parent: &str, name: &str) -> bool {
    name.strip_prefix(parent)
        .map(|rest| rest.starts_with('.') && rest.len() > 1)
        .unwrap_or(false)
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
    let Some(items) = decl.as_array() else {
        return false;
    };
    if let Some(options) = items.get(1).and_then(Value::as_object) {
        if options.get("forceInput").and_then(Value::as_bool) == Some(true) {
            return false;
        }
    }
    match items.first() {
        Some(Value::Array(_)) => true,
        Some(Value::String(text)) => {
            let upper = text.to_uppercase();
            // `COMFY_DYNAMICCOMBO_V3` 之类：**它是控件**，只是子项在展开时才存在
            // （见 [`fill_widgets`] 的 `dynamic` 分支）。把它当插口的表现是
            // "某个必填输入一直为空" —— `krea2 i2i workflow.json` 中
            // `TextGenerate.sampling_mode` 就是这样被误判的。
            WIDGET_TYPES.contains(&upper.as_str()) || upper.starts_with("COMFY_DYNAMIC")
        }
        _ => false,
    }
}

/// 一个类的控件名（**按 `widgets_values` 的位置**，`None` = 那一位是 seed 后面
/// 跟着的 `control_after_generate` 下拉框，没有独立名字）。
///
/// 蓝图要用它把"只有位置"的控件值贴回名字上（`widgets_values_named` 缺失时）。
/// 与 [`fill_widgets`] 共用 [`widget_specs`] 这一份声明 —— 各写一份的下场是
/// "转换时按这套顺序取值、界面上按那套顺序显示"，而错位不会报错。
pub(crate) fn widget_names(info: &Map<String, Value>) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for spec in widget_specs(info) {
        out.push(Some(spec.name));
        if spec.extra_slot {
            out.push(None);
        }
    }
    out
}

/// 一个节点的控件（= 有 `widgets_values` 槽位的输入）名字与顺序，来自 `/object_info`。
fn widget_specs(info: &Map<String, Value>) -> Vec<WidgetSpec> {
    let Some(input) = info.get("input").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<WidgetSpec> = Vec::new();
    for group in ["required", "optional"] {
        let Some(map) = input.get(group).and_then(Value::as_object) else {
            continue;
        };
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
            let dynamic = raw
                .as_array()
                .and_then(|items| items.first())
                .and_then(primitive_text)
                .map(|text| text.to_uppercase().starts_with("COMFY_DYNAMIC"))
                .unwrap_or(false);
            out.push(WidgetSpec {
                name: name.clone(),
                extra_slot,
                dynamic,
            });
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
    let Some(input) = info.get("input").and_then(Value::as_object) else {
        return;
    };
    for group in ["required", "optional"] {
        let Some(map) = input.get(group).and_then(Value::as_object) else {
            continue;
        };
        for (name, raw) in map {
            if inputs.contains_key(name) {
                continue;
            }
            // 控件型（含 COMBO）空着时 ComfyUI 用默认值，不算缺口；
            // forceInput 是**插口**，哪怕类型是 STRING 也算缺口
            if is_widget_input(raw) {
                continue;
            }
            let Some(type_name) = raw
                .as_array()
                .and_then(|items| items.first())
                .and_then(primitive_text)
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
/// "节点 xxx 不存在"，无法判断哪一步出错。
fn dangling_inputs(out: &Map<String, Value>) -> Vec<String> {
    let mut dangling: Vec<String> = Vec::new();
    for (id, value) in out {
        let Some(node) = value.as_object() else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for (name, reference) in inputs {
            let Some(source) = reference
                .as_array()
                .and_then(|items| items.first())
                .and_then(primitive_text)
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
fn first_linked_input(node: &Map<String, Value>, links: &LinkTable) -> Option<(String, i64)> {
    let inputs = node.get("inputs").and_then(Value::as_array)?;
    for entry in inputs {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(link_id) = as_int(entry.get("link")) else {
            continue;
        };
        if let Some(source) = links.get(&link_id) {
            return Some(source.clone());
        }
    }
    None
}

/// 有几个输入真的接了线（直通类节点要靠它判形状，见 [`FRONTEND_PASSTHROUGH`]）。
fn linked_input_count(node: &Map<String, Value>, links: &LinkTable) -> usize {
    let mut count = 0;
    let Some(inputs) = node.get("inputs").and_then(Value::as_array) else {
        return 0;
    };
    for entry in inputs {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(link_id) = as_int(entry.get("link")) else {
            continue;
        };
        if links.contains_key(&link_id) {
            count += 1;
        }
    }
    count
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
    node.get("type")
        .and_then(primitive_text)
        .unwrap_or_default()
}

/// 节点 id：界面格式里是**数字**，统一转成文本（API 格式的键是字符串）。
///
/// ⚠ 空白 id 当**没有 id**（Kotlin `JsonObject.idOf` 的 `takeIf { it.isNotBlank() }`）：
/// 这样它既不会被当成图的键，也不会被当成某个改写表的键。
pub(crate) fn node_id(node: &Map<String, Value>) -> Option<String> {
    node.get("id")
        .and_then(primitive_text)
        .filter(|id| !id.trim().is_empty())
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
    trimmed
        .parse::<f64>()
        .ok()
        .map(|v| v.trunc().clamp(i64::MIN as f64, i64::MAX as f64) as i64)
}

/// 节点引用里的 id 值：**永远是字符串**（`["128", 0]`，不是 `[128, 0]`）。
///
/// ⚠ 这里踩过一次真机事故（2026-10-25，`krea2 r2i workflow.json`）：
/// 早先照 Kotlin 的 `WorkflowNodeKeys.refOf` 把纯数字 id 写成**数字**，
/// 而 ComfyUI 的 `execution.validate_prompt` 是拿引用里的 id **直接索引 prompt 字典**
/// （`prompt[o_id]`）—— 那个字典的键来自 JSON、**永远是字符串**。于是数字引用一律抛
/// `KeyError`，被 `/prompt` 包成
/// `prompt_outputs_failed_validation … {"exception_type":"KeyError","details":"<节点 id>"}`，
/// 界面上只说"提交失败"，一个字都指不到这里。
///
/// ComfyUI 官方前端 `graphToPrompt()` 写的就是 `[String(id), slot]`；
/// 从 ComfyUI `/history` 捕获回来的图（`capture_runs.raw`）同样是字符串引用 ——
/// 也就是说**只有我们自己转出来的那份是数字**，那正是"库里老工作流能跑、新读进来的跑不了"。
pub(crate) fn ref_of(id: &str) -> Value {
    json!(id)
}

#[cfg(test)]
mod tests;
