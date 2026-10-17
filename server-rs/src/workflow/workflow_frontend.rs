//! ## 模拟用户行为 ##（`emulateFrontend`）—— Kotlin `FrontendNodes.kt` 的 Rust 端逐行对照
//! （P4 第 68 步；在这之前它是 `WorkflowConvert` 的两处欠账之一）。
//!
//! 用户的原话是：「加入 ##模拟用户行为## 模式，面对一些**仅面向前端的工作流节点**，
//! 让 AI 有能力在前端读取、编辑、在 comfy 前端提交」。
//!
//! 和真正的浏览器自动化不同（用户当面确认过走**后端 + ComfyUI HTTP API**这条线，
//! 不引入浏览器），这里做的是**把前端 JS 在排队前做的那几件事，在后端等价地做掉**：
//! ComfyUI 官方前端在 `graphToPrompt()` 之后、`POST /prompt` 之前，会调用各扩展的
//! `beforeRegisterNodeDef` / `onExecuted` 钩子去改写节点图。这一步在服务端是拿不到的
//! （那是浏览器里的 JS），所以过去我们只能如实报 `UNSUPPORTED_NODES`。
//!
//! 本模块只做**判据确定、可验证**的那几类，绝不照猫画虎：
//!
//! | 前端节点 | 前端自己干了什么 | 这里怎么等价改写 |
//! | --- | --- | --- |
//! | `Anything Everywhere` 一族（含 `?` / `Prompts` / `Seed` / 数字后缀变体） | 把上游那一路值**广播**到全图所有同类型、且没接线的输入上 | 按它声明的类型 / 名字过滤**填空**，并如实报告灌了哪几条线 |
//! | `PreviewImage` / `PreviewAudio` / `PreviewVideo` / `Preview3D` / `ShowText` / `Display Any` … | 只是"看一眼"的显示节点，不产出模型要的东西 | 丢掉（与官方前端一致），如实报告 |
//! | 没有任何输出节点 | 前端**自动挂一个 `SaveImage`** | 补一个 `SaveImage` 接上同一个 IMAGE 来源 |
//!
//! 判据一律**保守**：说不清类型的广播不猜，照旧报 `unsupported`（出口还是那一套：
//! 用 `comfy_submit(connections=…)` 点名补线，或者在 ComfyUI 里导出 API 格式）。
//!
//! # 「注册成零输出桩」的广播节点也算前端节点（2026-10-17 修）
//!
//! `cg-use-everywhere` 会把 `Anything Everywhere` **注册**进 `/object_info`
//! （`input.optional.anything = ["*"]`、`output = []`）：它在服务端是个什么都不产出的空节点，
//! 广播语义只活在前端。原判据是"`/object_info` 里查得到就原样保留"，于是这类节点
//! 既不会被等价改写、又不会产出任何东西 —— 它该填的那几路一直空着，提交上去直接
//! 缺必填输入。现在只对**广播型**放宽：注册了但 `output` 为空 → 仍然当广播节点改写。
//! 用户的 `krea2 t2i workflow.json` / `krea2 i2i workflow.json` 依赖这一条才跑得起来。
//!
//! # 判据只有一份
//!
//! "这个类是不是广播型 / 显示型"两张表与三条模式在 [`crate::workflow::workflow_audit`]
//! （[`crate::workflow::workflow_audit::is_broadcast`] / [`crate::workflow::workflow_audit::is_display`]，
//! 第 63 步就已经手写好了、**没有 regex 依赖**）；这里**只做改写**，不再抄第三份表。
//! 同理，`/object_info` 的"这个声明是不是控件"用
//! [`crate::workflow::workflow_convert::is_widget_input`]，节点引用写法用
//! [`crate::workflow::workflow_convert::ref_of`]。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::workflow::workflow_audit::{is_broadcast, is_display};
// ⚠ 这几个都从 `workflow_convert` 借（Kotlin 那边 `FrontendNodes.kt` 自己抄了一份
//   `WIDGET_TYPES` / `refOf`；搬过来时**刻意只留一份** —— 两份判据分叉不会报错，
//   只会让"两边对同一份图给出不同结果"）。
use crate::workflow::workflow_convert::{as_int, is_widget_input, node_id, primitive_text, ref_of, ui_class, LinkTable};

/// 一次改写的总结（进工具结果，给模型和用户看"到底动了哪几根线"）。
///
/// Kotlin `FrontendNodes.Emulation`。
#[derive(Debug, Clone, Default)]
pub struct Emulation {
    /// 已经等价改写 / 丢掉的前端节点，形如 `12 Anything Everywhere（广播 3 处）`
    pub rewritten: Vec<String>,
    /// 广播灌进去的线，形如 `1.positive ← 节点 14（Anything Everywhere）`
    pub broadcast: Vec<String>,
    /// 自动补的输出节点（前端在"一个输出都没有"时也会这么做）
    pub injected: Vec<String>,
    /// 仍然改写不了的前端节点，形如 `37 Anything Everywhere（类型 未声明：全图没有对得上的空输入）`
    pub unsupported: Vec<String>,
    /// 已经等价改写 / 丢掉的前端节点 id（调用方据此从 `unsupportedNodes` 里摘出去）
    pub handled_ids: Vec<String>,
}

/// 对一份**已经转换好**的 API 节点图做前端等价改写（Kotlin `FrontendNodes.emulate`）。
///
/// * `raw_nodes`：界面格式里的原始节点（**展开组节点之后**的那一份：
///   找广播 / 显示节点要靠它，转出来的图里已经没有它们了）
/// * `object_info`：ComfyUI 的 `/object_info`
/// * `graph`：`WorkflowConvert.toApiGraph` 转出来的图（**会被复制**，不改原对象）
/// * `links`：linkId → (上游节点 id, 输出槽位)（展开后的那张表）
pub fn emulate(
    raw_nodes: &[Map<String, Value>],
    object_info: &Map<String, Value>,
    graph: &Map<String, Value>,
    links: &LinkTable,
) -> (Map<String, Value>, Emulation) {
    let mut out: Map<String, Value> = graph.clone();
    let mut report = Emulation::default();

    // ⚠ 判定顺序与 Kotlin 一致：**先看 `/object_info` 里有没有这个类**
    //   （有的版本把它们做成了真节点，那时它就该原样保留）。
    //
    //   2026-10-17 补的一条：**广播型**节点在本机常常被注册成「零输出」的桩 ——
    //   cg-use-everywhere 的 `Anything Everywhere` 就是这样（`input.optional.anything = ["*"]`、
    //   `output = []`）。它在 `/object_info` 里**查得到**，可服务端拿到它什么也不产出，
    //   真正干活的仍旧是前端那次广播。只按「查得到就留下」判，这份图提交上去就会
    //   **缺一堆必填输入**（用户的 `krea2 t2i workflow.json` / `krea2 i2i workflow.json`
    //   就是这么坏的，而且不报错）。
    //
    //   判据只加在**广播型**上：显示型节点（`PreviewImage` 之类）注册了就是真节点，
    //   丢掉它们反而会把「只想看一眼」的图变成「自动补 SaveImage 落盘」。
    let is_virtual = |node: &Map<String, Value>, predicate: fn(&str) -> bool| -> bool {
        let class = ui_class(node);
        if class.is_empty() || !predicate(&class) {
            return false;
        }
        match object_info.get(&class).and_then(Value::as_object) {
            None => true,
            Some(entry) => {
                is_broadcast(&class)
                    && entry
                        .get("output")
                        .and_then(Value::as_array)
                        .map(|list| list.is_empty())
                        .unwrap_or(false)
            }
        }
    };
    let broadcasters: Vec<&Map<String, Value>> =
        raw_nodes.iter().filter(|node| is_virtual(node, is_broadcast)).collect();
    let displays: Vec<&Map<String, Value>> =
        raw_nodes.iter().filter(|node| is_virtual(node, is_display)).collect();

    // 空输入只算一次；每灌一条线就记进 filled，后面的广播不会再重复灌同一个位置
    let all_open = open_inputs(object_info, &out);
    let mut filled: HashSet<(String, String)> = HashSet::new();

    // --- ① 广播：把上游那一路填进所有"同类型且空着"的输入 ---
    for node in &broadcasters {
        let Some(id) = node_id(node) else { continue };
        let class = ui_class(node);
        let spec = broadcast_spec(node);
        let routes = broadcast_routes(node, links, &spec, &out);
        if routes.is_empty() {
            // 它自己都没接上游：没有任何东西可广播（前端这时候也不会灌值）
            report.unsupported.push(format!("{id} {class}（没有接上游，没有东西可以广播）"));
            continue;
        }
        let mut filled_here = 0usize;
        for route in &routes {
            // 每灌一条线就记进 filled，后面的广播不会再重复灌同一个位置
            let hits: Vec<&OpenInput> = all_open
                .iter()
                .filter(|target| !filled.contains(&(target.node_id.clone(), target.name.clone())))
                .filter(|target| matches(&spec, target, route.type_name.as_deref()))
                .collect();
            if hits.is_empty() {
                report.unsupported.push(format!(
                    "{id} {class}（{}：全图没有对得上的空输入）",
                    spec.describe(route.type_name.as_deref())
                ));
                continue;
            }
            for target in &hits {
                let Some(node_body) = out.get(&target.node_id).and_then(Value::as_object) else {
                    continue;
                };
                let mut inputs: Map<String, Value> =
                    node_body.get("inputs").and_then(Value::as_object).cloned().unwrap_or_default();
                inputs.insert(target.name.clone(), json!([ref_of(&route.source.0), route.source.1]));
                let mut rebuilt = node_body.clone();
                rebuilt.insert("inputs".into(), Value::Object(inputs));
                out.insert(target.node_id.clone(), Value::Object(rebuilt));
                filled.insert((target.node_id.clone(), target.name.clone()));
                filled_here += 1;
                report
                    .broadcast
                    .push(format!("{}.{} ← 节点 {id}（{class}）", target.node_id, target.name));
            }
        }
        if filled_here > 0 {
            report.rewritten.push(format!("{id} {class}（广播 {filled_here} 处）"));
        }
    }

    // --- ② 显示节点：丢掉（官方前端也不把这类写进提示词）---
    for node in &displays {
        let Some(id) = node_id(node) else { continue };
        report.rewritten.push(format!("{id} {}（显示节点，已忽略）", ui_class(node)));
    }

    // --- ③ 一个输出节点都没有：前端会自动挂 SaveImage，这里照做 ---
    // 只在"原本一张图都存不出来"时补，绝不覆盖用户自己的工作流（那会多写文件）。
    // 接哪一路？**按类型找产出 IMAGE 的节点**（不能看输入名：VAEDecode 的输出压根不在
    // 它的 `inputs` 里，它是把 `samples` 解成图的那个节点）。
    if !out.is_empty() && out.values().all(|value| !is_save_like_node(value)) {
        let image_source: Option<Value> = out.iter().find_map(|(node_id, value)| {
            let node = value.as_object()?;
            if !produces(object_info, node, "IMAGE") {
                return None;
            }
            Some(json!([ref_of(node_id), 0]))
        });
        if let Some(image_source) = image_source {
            let new_id = fresh_node_id(out.keys());
            let mut inputs = Map::new();
            inputs.insert("images".into(), image_source);
            inputs.insert("filename_prefix".into(), json!("ComfyHub"));
            let mut node = Map::new();
            node.insert("class_type".into(), json!("SaveImage"));
            node.insert("inputs".into(), Value::Object(inputs));
            out.insert(new_id.clone(), Value::Object(node));
            report.injected.push(format!("{new_id} SaveImage（原本没有输出节点，照前端的做法补一个）"));
        }
    }

    let mut handled_ids: Vec<String> = Vec::new();
    for node in broadcasters.iter().chain(displays.iter()) {
        if let Some(id) = node_id(node) {
            handled_ids.push(id);
        }
    }
    report.handled_ids = handled_ids;
    (out, report)
}

/// 供 [`crate::workflow::workflow_convert::to_api_graph`] 在广播改写之后重算"还空着哪些连线型输入"。
///
/// ⚠ 多个可接受类型之间用 `/` 拼（Kotlin `types.joinToString("/")`）—— 与转换时那一份
/// （单类型）在只有一个类型时**逐字相同**，所以模型不会看到两种写法。
pub fn open_inputs_of(object_info: &Map<String, Value>, graph: &Map<String, Value>) -> Vec<String> {
    open_inputs(object_info, graph)
        .into_iter()
        .map(|target| format!("{}.{} ({})", target.node_id, target.name, target.types.join("/")))
        .collect()
}

/// 还空着的**必填**连线型输入（可选插口不算缺口）。
///
/// ⚠ 与 [`open_inputs_of`] 的差别**只有必填这一条**，而它是"这份图能不能跑"的分水岭：
/// 可选插口空着是常态（`TextGenerate.video` / `.audio`、`RMBG.background_color` 都是可选），
/// 把它们算成缺口的表现是**学一份配方直接被拒**（2026-10-17 在用户的
/// `krea2 i2i workflow.json` 上实测：三条可选插口让 `UNSUPPORTED_NODES` 挡下了整份工作流）。
pub fn required_open_inputs_of(
    object_info: &Map<String, Value>,
    graph: &Map<String, Value>,
) -> Vec<String> {
    open_inputs(object_info, graph)
        .into_iter()
        .filter(|target| target.required)
        .map(|target| format!("{}.{} ({})", target.node_id, target.name, target.types.join("/")))
        .collect()
}

/// 广播的目标类型 / 名字过滤（Kotlin `BroadcastSpec`）。
///
/// `Anything Everywhere` 把自己的设定存在节点的 `properties` 里
/// （`"Input Type"` / `"Input Name"` / `"Prefix"` / `"Filter"`），不同版本键名不一样，
/// 所以这里逐个认；一个都认不出来时**类型为 None** = 不猜，由调用方报 unsupported。
struct BroadcastSpec {
    type_name: Option<String>,
    name_filter: Option<String>,
    group_mode: Option<String>,
    min_count: Option<i32>,
}

impl BroadcastSpec {
    /// `route_type` = 这一路的类型（新版本把类型挂在**输入插口**上，见 [`broadcast_routes`]）。
    fn describe(&self, route_type: Option<&str>) -> String {
        let mut out = String::from("类型 ");
        out.push_str(self.type_name.as_deref().or(route_type).unwrap_or("未声明"));
        if let Some(filter) = &self.name_filter {
            out.push_str(&format!("，名字含「{filter}」"));
        }
        if let Some(mode) = &self.group_mode {
            out.push_str(&format!("，分组 {mode}"));
        }
        if let Some(count) = self.min_count {
            out.push_str(&format!("，至少 {count} 处"));
        }
        out
    }
}

/// 一路广播：一个来源 + 它自己声明的类型。
struct BroadcastRoute {
    source: (String, i64),
    type_name: Option<String>,
}

/// 广播节点到底广播**哪几路**（判据两套，新旧版本各一套）。
///
/// * **老版本**（`Anything Everywhere` 带 `Input Type` / `Input Name` 控件，存在 `properties` 里）：
///   整台节点只有一路来源 + 一条类型/名字规则 —— 与 [`broadcast_spec`] 一起用。
/// * **新版本**（cg-use-everywhere ≥ 1.x，2026-10-17 在用户的
///   `krea2 t2i workflow.json` 上实测）：`properties` 里**没有**类型，节点自己长着
///   `anything[CLIP]` / `anything15[MODEL]` / `anything16[VAE]` 这样**每个接了线的输入插口各一路**，
///   类型就写在那条插口的 `type` 上。只认老版本的表现是"这份工作流一处广播都填不上"，
///   而它看上去一切正常（缺口留到提交时才炸）。
fn broadcast_routes(
    node: &Map<String, Value>,
    links: &LinkTable,
    spec: &BroadcastSpec,
    graph: &Map<String, Value>,
) -> Vec<BroadcastRoute> {
    // 老版本：声明了类型或名字过滤 → 只认"第一个接了线的输入"当来源
    if spec.type_name.is_some() || spec.name_filter.is_some() {
        return broadcast_source(node, links)
            .and_then(|source| resolve_for(graph, source))
            .map(|source| vec![BroadcastRoute { source, type_name: None }])
            .unwrap_or_default();
    }
    // 新版本：逐个插口
    let mut out: Vec<BroadcastRoute> = Vec::new();
    let Some(entries) = node.get("inputs").and_then(Value::as_array) else { return out };
    for entry in entries.iter().filter_map(Value::as_object) {
        let Some(link_id) = as_int(entry.get("link")) else { continue };
        let Some(source) = links.get(&link_id).cloned() else { continue };
        let Some(source) = resolve_for(graph, source) else { continue };
        let type_name = entry
            .get("type")
            .and_then(primitive_text)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty() && text != "*");
        if out.iter().any(|route| route.source == source) {
            continue;
        }
        out.push(BroadcastRoute { source, type_name });
    }
    out
}

/// `properties` 里几个键名各版本不一样，逐个认（Kotlin `broadcastSpec` 的 `prop`）。
fn broadcast_spec(node: &Map<String, Value>) -> BroadcastSpec {
    let props = node.get("properties").and_then(Value::as_object);
    let prop = |keys: &[&str]| -> Option<String> {
        for key in keys {
            let Some(value) = props.and_then(|map| map.get(*key)) else { continue };
            if value.is_null() {
                continue;
            }
            let text = primitive_text(value).map(|text| text.trim().to_string());
            if let Some(text) = text {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    };
    let raw_type = prop(&["Input Type", "inputType", "Type", "type"]);
    // 前端把"不限类型"写成 `*` / `*?` / `anything`
    let type_name =
        raw_type.filter(|text| text != "*" && text != "*?" && !text.eq_ignore_ascii_case("anything"));
    BroadcastSpec {
        type_name,
        name_filter: prop(&["Input Name", "inputName", "Prefix", "Filter", "filter"]),
        group_mode: prop(&["Group Mode", "group_mode", "Mode", "mode"]),
        min_count: prop(&["Broadcast", "broadcast"]).and_then(|text| text.parse::<i32>().ok()),
    }
}

/// 这条空输入接不接受这次广播（Kotlin `matches`）。
///
/// `route_type` = 这一路自己的类型（新版本从插口上读来的）；旧版节点没有它就传 `None`。
fn matches(spec: &BroadcastSpec, target: &OpenInput, route_type: Option<&str>) -> bool {
    // 名字过滤优先（Prompts Everywhere 就是靠它只灌提示词输入）
    if let Some(filter) = &spec.name_filter {
        // Kotlin `contains(other, ignoreCase = true)` 是按 `Char` 折大小写的；
        // 这里用 `to_lowercase()`（完整 Unicode 折叠）—— 对 ASCII 的过滤词**逐字一致**，
        // 非 ASCII 的过滤词只在个别字符（如 `İ`）上比 Kotlin 宽一点，不会漏灌。
        return target.name.to_lowercase().contains(&filter.to_lowercase());
    }
    let wanted = spec.type_name.as_deref().or(route_type).filter(|text| !text.is_empty() && *text != "*");
    let Some(type_name) = wanted else { return false };
    target.types.iter().any(|accepted| accepted.eq_ignore_ascii_case(type_name))
}

/// 广播节点的上游：它自己的那个连线型输入接的是谁（Kotlin `broadcastSource`）。
///
/// 有的版本写成 `widgets_values` 里的上游名字 —— **认不出来就不广播**（不猜）。
fn broadcast_source(node: &Map<String, Value>, links: &LinkTable) -> Option<(String, i64)> {
    for entry in node.get("inputs").and_then(Value::as_array)? {
        let Some(entry) = entry.as_object() else { continue };
        // Kotlin `linkIdOf` 用的是严格 `toLongOrNull()`；这里复用 `as_int`
        // （它多认 `"1.0"` 这种写法）—— 真实存档里 link 都是整数，两者等价。
        let Some(link_id) = as_int(entry.get("link")) else { continue };
        if let Some(source) = links.get(&link_id) {
            return Some(source.clone());
        }
    }
    None
}

/// 穿透 Reroute / 旁路 / 组节点输出留下的改写表，拿到真正的产出节点（Kotlin `resolveFor`）。
///
/// 广播是在**转完之后**做的，改写表已经不在手上了，所以这里只在图里找：
/// 图里没有那个 id 就如实放弃（前端那时候也灌不出值）。
fn resolve_for(graph: &Map<String, Value>, reference: (String, i64)) -> Option<(String, i64)> {
    if graph.contains_key(&reference.0) {
        Some(reference)
    } else {
        None
    }
}

/// 一条"现在空着的连线型输入"（Kotlin `OpenInput`）。
struct OpenInput {
    node_id: String,
    name: String,
    /// `/object_info` 声明的可接受类型（空 = 说不清，不参与广播）
    types: Vec<String>,
    /// 它是**必填**还是可选 —— "缺口"只算必填那一档（见 [`required_open_inputs_of`]）
    required: bool,
}

/// 现在**还空着**的连线型输入（转出来的图里没有这个键 = 空着）。
///
/// 广播节点自己进不了图（`/object_info` 里查得到才进得来），所以这里只认有定义的类。
fn open_inputs(object_info: &Map<String, Value>, graph: &Map<String, Value>) -> Vec<OpenInput> {
    let mut out: Vec<OpenInput> = Vec::new();
    for (node_id, value) in graph {
        let Some(node) = value.as_object() else { continue };
        let Some(class) = node.get("class_type").and_then(primitive_text) else { continue };
        let Some(info) = object_info.get(&class).and_then(Value::as_object) else { continue };
        let Some(input) = info.get("input").and_then(Value::as_object) else { continue };
        let present: HashSet<&String> =
            node.get("inputs").and_then(Value::as_object).map(|map| map.keys().collect()).unwrap_or_default();
        for group in ["required", "optional"] {
            let Some(map) = input.get(group).and_then(Value::as_object) else { continue };
            for (name, raw) in map {
                if present.contains(name) {
                    continue;
                }
                // 控件型（含 COMBO）空着时 ComfyUI 用默认值，不是缺口；forceInput 是插口
                if is_widget_input(raw) {
                    continue;
                }
                let Some(spec) = raw.as_array() else { continue };
                let accepted = accepted_types(spec);
                if accepted.is_empty() {
                    continue;
                }
                out.push(OpenInput {
                    node_id: node_id.clone(),
                    name: name.clone(),
                    types: accepted,
                    required: group == "required",
                });
            }
        }
    }
    out
}

/// `/object_info` 里一个输入声明的可接受类型（`["MODEL"]` / `[[…]]` / `["INT",{…}]`）。
fn accepted_types(spec: &[Value]) -> Vec<String> {
    let Some(head) = spec.first() else { return Vec::new() };
    // COMBO 之类的枚举 = 控件型，上面已经挡掉
    if head.is_array() {
        return Vec::new();
    }
    let Some(text) = primitive_text(head) else { return Vec::new() };
    let text = text.trim();
    // 以 `*` 结尾的老写法（`"MODEL,CLIP"` / `"*"`）在 /object_info 里很少见，认不出来就不猜
    if text.is_empty() || text.starts_with('*') {
        Vec::new()
    } else {
        vec![text.to_string()]
    }
}

/// 这个节点会不会产出一条 `IMAGE`（Kotlin `produces`）。
///
/// 判据按可靠性分三级（**只在能确定时才认**）：
///  1. `/object_info` 的 `output` 声明里有 IMAGE（最准）；
///  2. 没有 `output` 声明时，退回"类名像解码 / 合成图"的那几个（`VAEDecode` / `SaveImage` …）；
///  3. 都认不出来 = 不认（宁可少补一个输出节点，也不能把错的线接上去）。
fn produces(object_info: &Map<String, Value>, node: &Map<String, Value>, type_name: &str) -> bool {
    let Some(class) = node.get("class_type").and_then(primitive_text) else { return false };
    let declared = object_info
        .get(&class)
        .and_then(Value::as_object)
        .and_then(|info| info.get("output"))
        .and_then(Value::as_array);
    if let Some(declared) = declared {
        return declared.iter().any(|item| {
            primitive_text(item).map(|text| text.eq_ignore_ascii_case(type_name)).unwrap_or(false)
        });
    }
    if type_name == "IMAGE" {
        IMAGE_MAKERS.iter().any(|maker| class.starts_with(maker))
    } else {
        false
    }
}

const IMAGE_MAKERS: [&str; 4] = ["VAEDecode", "ImageScale", "ImageBlend", "ImageComposite"];

/// 这个节点算不算"已经会存 / 会显示图"的（Kotlin `isSaveLikeNode`）。
fn is_save_like_node(value: &Value) -> bool {
    let Some(class) =
        value.as_object().and_then(|node| node.get("class_type")).and_then(primitive_text)
    else {
        return false;
    };
    class.starts_with("Save") || class.starts_with("Preview")
}

/// 没人用过的新节点号（Kotlin `freshNodeId`）。
fn fresh_node_id<'a>(existing: impl Iterator<Item = &'a String>) -> String {
    let used: HashSet<&String> = existing.collect();
    let mut next = used.iter().filter_map(|id| id.parse::<i64>().ok()).max().unwrap_or(0) + 1;
    while used.contains(&next.to_string()) {
        next += 1;
    }
    next.to_string()
}

#[cfg(test)]
mod tests;
