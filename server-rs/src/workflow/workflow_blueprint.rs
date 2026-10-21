//! 工作流「蓝图」：把一份工作流归一化成**可绘制的节点-连线模型**，并在同一次遍历里
//! 做**接线体检**（用户 2026-10-17：「让捕获的工作流能像 ComfyUI 一样用蓝图预览乃至修改，
//! 特别注意有些**前端 only** 的节点」「对于**错误接线**要能向用户指出」）。
//!
//! # 它解决什么
//!
//! 今天「查看工作流」只是把 JSON 原文丢进一个 `SelectableText`（`lib/widgets/workflow_viewer.dart`）——
//! 用户看不出这张图有几个节点、谁连着谁、哪几个是旁路的、哪根线接错了。
//! 而**接线错误是不报错的**：`Anything Everywhere` 广播、旁路穿透、Set/Get 跳线这些语义
//! 只存在于 ComfyUI 前端，服务端拿到的是另一张图 —— 于是最常见的现场是
//! "图看着没问题、提交上去缺一堆必填输入"，或者"跑完了、出图跟预期完全不同"。
//!
//! # 三种格式，三种判据
//!
//! | 输入 | `format` | 能画出什么 | 判据来源 |
//! | --- | --- | --- | --- |
//! | 界面格式（`{nodes,links,groups}`） | `ui` | 真实坐标 / 尺寸 / 分组 / 旁路 / 前端节点 | 文件自己的 `pos`/`size`/`links` |
//! | API 格式（`{"1":{class_type,…}}`） | `api` | 只有逻辑图：**没有坐标**，前端自动分层排 | `inputs` 里值为数组的项 |
//! | 既不是也不是 | `unknown` | 什么都不画，如实说 | — |
//!
//! **两种 `links` 写法**（老版数组 / 新版对象）与 **两份控件值**（`widgets_values` 位置化、
//! `widgets_values_named` 带名字）都必须认：判据只有 [`crate::workflow::workflow_convert::raw_links`]
//! 与 [`crate::workflow::workflow_convert::widget_names`] 一份，别在这里再抄一遍。
//!
//! # 接线体检的判据（一条都不猜）
//!
//! | 码 | 严重度 | 什么时候报 |
//! | --- | --- | --- |
//! | `DUPLICATE_ID` | 错误 | 两个节点用同一个 id（后一个会把前一个顶掉） |
//! | `LINK_DANGLING` | 错误 | 线的一头指向不存在的节点（提交上去 ComfyUI 只会说"节点不存在"） |
//! | `LINK_NOT_FOUND` | 错误 | 节点输入写着"我接了线 N"，但 `links` 表里没有 N |
//! | `LINK_SLOT_MISMATCH` | 警告 | `links` 表与节点插口对不上（存档被外部改过 / 版本迁移残留） |
//! | `TYPE_MISMATCH` | 错误 | 上游输出的类型与下游输入声明的类型不同（ComfyUI 原话 `Return type mismatch between linked nodes`） |
//! | `INPUT_UNCONNECTED` | 错误 / 警告 | 插口空着（有 `/object_info` 且它是必填 → 错误；否则只提醒） |
//! | `CYCLE` | 错误 | 图里有环 |
//! | `NO_OUTPUT_NODE` | 错误 | 一个输出节点都没有（ComfyUI 只会回 `Prompt has no outputs`） |
//! | `UNREACHABLE` | 提示 | 有节点不参与出图（不会被执行） |
//! | `FRONTEND_NODE` | 警告 | 图里还有纯前端节点（服务端没有这个类） |
//! | `UNKNOWN_CLASS` | 错误 | `/object_info` 里没有这个类（没装那个节点包） |
//! | `BROADCAST_STUB` | 警告 | 广播型节点在本机被注册成"零输出桩"：它的语义只在前端，服务端拿到它什么也不产出 |
//! | `CONVERT_GAP` | 错误 | 转换（提交前必走的那一步）留下了缺口：悬挂引用 / 空必填 |
//! | `PARAM_MISMATCH` | 警告 | 控件值个数与输入声明对不上（参数会错位） |
//!
//! **拿不到 `/object_info` 时只做"不需要节点定义也能做"的那几条**（结构、坐标、连线表、
//! 控件值个数），不判类型、不判必填、不判"本机没有这个类" —— 宁可少报，也不要凭"我没问到"
//! 下结论（与 `workflow_audit` 同一条纪律）。
//!
//! # 一处**刻意**的分工
//!
//! 这里**不改图**：它只回答"长什么样、哪里接错了"。真动手的两条路都各有归口 ——
//! 转换/提交走 [`crate::workflow::workflow_convert`] + [`crate::workflow::workflow_submit`]，
//! 编辑（改参数 / 关掉某条分支）由界面把改动按 **节点 id** 写回原文，
//! 下一次体检就能看见结果。蓝图里再长一条"改图"的逻辑，就是第二套转换栈了。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::workflow::workflow_audit::{is_broadcast, is_display, KNOWN_FRONTEND_ONLY};
use crate::workflow::workflow_convert::{
    self as conv, as_int, is_node_graph, is_ui_workflow, node_id, primitive_text, raw_links, ui_class,
};

/// 严重度：只有 [`SEV_ERROR`] 是"这么交上去一定跑不成"。
pub const SEV_ERROR: &str = "error";
pub const SEV_WARN: &str = "warn";
pub const SEV_INFO: &str = "info";

/// 认得出的"张量类"类型：往 COMBO（下拉框）里灌这些一定是错的。
///
/// ⚠ 为什么不把 `STRING` 算进来：ComfyUI 允许把一个字符串输出接进下拉框输入
/// （rgthree 的 Power Prompt / `Anything Everywhere` 的 string_to_combo 都靠它），
/// 把 `STRING → COMBO` 报成错会制造一片假警报。
const TENSOR_TYPES: [&str; 15] = [
    "IMAGE",
    "LATENT",
    "MASK",
    "MODEL",
    "CLIP",
    "VAE",
    "CONDITIONING",
    "SIGMAS",
    "SAMPLER",
    "GUIDER",
    "NOISE",
    "CONTROL_NET",
    "AUDIO",
    "VIDEO",
    "CLIP_VISION",
];

/// 一条插口。
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub name: String,
    /// 声明的类型（界面格式用 `inputs[].type`；API 格式靠 `/object_info`）
    pub type_name: String,
    /// 输入：接的是哪条线（`None` = 空着）
    pub link: Option<i64>,
    /// 输出：下游的线
    pub links: Vec<i64>,
    /// 这个输入是控件（下拉框 / 数字框），不是一个插口
    pub widget: bool,
    /// 控件当前值（能对上名字时才有）
    pub value: Option<Value>,
}

/// 一个控件（按 `widgets_values` 的位置）。
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub index: usize,
    /// `None` = 只有位置、没有名字（旧存档且 ComfyUI 不在线时会出现）
    pub name: Option<String>,
    pub value: Value,
}

/// 一个节点。
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub class: String,
    pub title: String,
    pub pos: Option<(f64, f64)>,
    pub size: Option<(f64, f64)>,
    /// litegraph 的状态：0 正常 / 2 静音 / 4 旁路
    pub mode: i64,
    /// `normal` / `note` / `reroute` / `variable` / `broadcast` / `display` / `bypasser` /
    /// `subgraph` / `unknown`
    pub kind: &'static str,
    /// 服务端没有这个类（纯前端节点，或没装的自定义节点包）
    pub frontend_only: bool,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub widgets: Vec<Widget>,
}

/// 一条连线。
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// 界面格式的连线 id；API 格式没有 id（`implicit`）
    pub id: Option<i64>,
    pub from: (String, i64),
    pub to: (String, i64),
    pub type_name: Option<String>,
    pub implicit: bool,
}

/// 一个分组框（界面格式才有）。
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub title: String,
    pub bounding: Vec<f64>,
    pub color: Option<String>,
}

/// 一条体检结论。
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub code: &'static str,
    pub severity: &'static str,
    pub title: String,
    pub detail: String,
    pub fix: Option<String>,
    pub nodes: Vec<String>,
    pub links: Vec<i64>,
    /// `33.model` 这种"节点.输入名"，界面据此把标记画到具体插口上
    pub slots: Vec<String>,
}

/// 一份蓝图。
#[derive(Debug, Clone)]
pub struct Blueprint {
    /// `ui` / `api` / `unknown`
    pub format: &'static str,
    pub title: Option<String>,
    /// `/object_info` 拿到了没有（拿不到时体检的覆盖面会小一圈，界面要如实说）
    pub object_info: bool,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub groups: Vec<Group>,
    pub issues: Vec<Issue>,
    pub warnings: Vec<String>,
}

impl Blueprint {
    fn empty(format: &'static str, warning: &str) -> Self {
        Self {
            format,
            title: None,
            object_info: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            issues: Vec::new(),
            warnings: vec![warning.to_string()],
        }
    }

    /// 按严重度数的条数。
    pub fn count(&self, severity: &str) -> usize {
        self.issues.iter().filter(|issue| issue.severity == severity).count()
    }

    /// 能不能直接提交：没有任何 `error` 就算过了**这一关**。
    ///
    /// ⚠ 它不是"一定能出图"：参数档位、显存、提示词风格那些在 `workflow_audit` 里，
    /// 这里只管**接线与结构**。
    pub fn clean(&self) -> bool {
        self.count(SEV_ERROR) == 0
    }

    /// 给界面画图用的一份 JSON。
    pub fn to_json(&self) -> Value {
        let nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|node| {
                json!({
                    "id": node.id,
                    "class": node.class,
                    "title": node.title,
                    "pos": node.pos.map(|(x, y)| json!([x, y])),
                    "size": node.size.map(|(w, h)| json!([w, h])),
                    "mode": node.mode,
                    "kind": node.kind,
                    "frontendOnly": node.frontend_only,
                    "inputs": node.inputs.iter().map(|port| json!({
                        "name": port.name,
                        "type": port.type_name,
                        "link": port.link,
                        "links": port.links,
                        "widget": port.widget,
                        "value": port.value,
                    })).collect::<Vec<Value>>(),
                    "outputs": node.outputs.iter().map(|port| json!({
                        "name": port.name,
                        "type": port.type_name,
                        "link": port.link,
                        "links": port.links,
                        "widget": port.widget,
                        "value": port.value,
                    })).collect::<Vec<Value>>(),
                    "widgets": node.widgets.iter().map(|widget| json!({
                        "index": widget.index,
                        "name": widget.name,
                        "value": widget.value,
                    })).collect::<Vec<Value>>(),
                })
            })
            .collect();
        let edges: Vec<Value> = self
            .edges
            .iter()
            .map(|edge| {
                json!({
                    "id": edge.id,
                    "from": { "node": edge.from.0, "slot": edge.from.1 },
                    "to": { "node": edge.to.0, "slot": edge.to.1 },
                    "type": edge.type_name,
                    "implicit": edge.implicit,
                })
            })
            .collect();
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|group| {
                json!({
                    "title": group.title,
                    "bounding": group.bounding,
                    "color": group.color,
                })
            })
            .collect();
        let issues: Vec<Value> = self
            .issues
            .iter()
            .map(|issue| {
                json!({
                    "code": issue.code,
                    "severity": issue.severity,
                    "title": issue.title,
                    "detail": issue.detail,
                    "fix": issue.fix,
                    "nodes": issue.nodes,
                    "links": issue.links,
                    "slots": issue.slots,
                })
            })
            .collect();
        json!({
            "format": self.format,
            "title": self.title,
            "objectInfo": self.object_info,
            "counts": {
                "nodes": self.nodes.len(),
                "edges": self.edges.len(),
                "groups": self.groups.len(),
                "errors": self.count(SEV_ERROR),
                "warnings": self.count(SEV_WARN),
                "infos": self.count(SEV_INFO),
            },
            "nodes": nodes,
            "edges": edges,
            "groups": groups,
            "issues": issues,
            "warnings": self.warnings,
        })
    }

    /// 给模型 / 日志看的文本报告。
    pub fn render(&self) -> String {
        let mut out = String::new();
        let label = match self.format {
            "ui" => "界面格式",
            "api" => "API 格式节点图",
            _ => "认不出的格式",
        };
        out.push_str(&format!(
            "工作流蓝图（{label}）：{} 个节点 / {} 条连线 / {} 个分组\n",
            self.nodes.len(),
            self.edges.len(),
            self.groups.len()
        ));
        if self.format == "api" {
            out.push_str("位置是自动排的（API 格式没有保存画布坐标，与 ComfyUI 里看到的那张图不是同一张）\n");
        }
        if !self.object_info {
            out.push_str("⚠ 这次没拿到 ComfyUI 的 `/object_info`：不判节点类型与「必填输入」，只做结构检查\n");
        }
        if self.issues.is_empty() {
            out.push_str("接线体检：没发现问题\n");
        } else {
            out.push_str(&format!(
                "接线体检：{} 个错误 / {} 个警告 / {} 条提示\n",
                self.count(SEV_ERROR),
                self.count(SEV_WARN),
                self.count(SEV_INFO)
            ));
            for (index, issue) in self.issues.iter().enumerate() {
                let mark = match issue.severity {
                    SEV_ERROR => "错误",
                    SEV_WARN => "警告",
                    _ => "提示",
                };
                out.push_str(&format!("{}. [{mark}] {} —— {}\n", index + 1, issue.title, issue.detail));
                if let Some(fix) = &issue.fix {
                    out.push_str(&format!("   修法：{fix}\n"));
                }
            }
        }
        for warning in &self.warnings {
            out.push_str(&format!("· {warning}\n"));
        }
        out
    }

    /// 问题清单压成一句话（没有任何问题时返回 `None`）——给"先看一眼"的场景用。
    pub fn summary(&self) -> Option<String> {
        let errors = self.count(SEV_ERROR);
        let warns = self.count(SEV_WARN);
        if errors == 0 && warns == 0 {
            return None;
        }
        let mut out = format!("{errors} 个接线错误 / {warns} 个警告");
        let first = self
            .issues
            .iter()
            .find(|issue| issue.severity == SEV_ERROR)
            .or_else(|| self.issues.iter().find(|issue| issue.severity == SEV_WARN));
        if let Some(first) = first {
            let label = if first.severity == SEV_ERROR { "第一条" } else { "最要紧的一条" };
            out.push_str(&format!("；{label}：{}", first.title));
        }
        Some(out)
    }
}

/// 主入口：把一份工作流原文变成蓝图。
///
/// * `raw`：工作流 JSON（界面格式或 API 格式）
/// * `object_info`：ComfyUI 的 `/object_info`（`None` = 没在线 / 没问）
/// * `diagnose`：`false` 时只建模型、不体检（预览用得上，省一次转换）
pub fn build(raw: &Value, object_info: Option<&Map<String, Value>>, diagnose: bool) -> Blueprint {
    let Some(obj) = raw.as_object() else {
        return Blueprint::empty("unknown", "这不是一个 JSON 对象");
    };
    if is_ui_workflow(obj) {
        build_ui(obj, object_info, diagnose)
    } else if is_node_graph(obj) {
        build_api(obj, object_info, diagnose)
    } else {
        Blueprint::empty(
            "unknown",
            "既不是界面格式工作流（没有 `nodes` 数组），也不是 API 格式节点图（不是每个值都带 `class_type`）",
        )
    }
}

// ---------------------------------------------------------------------------
// 界面格式
// ---------------------------------------------------------------------------

fn build_ui(
    ui: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
    diagnose: bool,
) -> Blueprint {
    let mut blueprint = Blueprint {
        format: "ui",
        title: ui
            .get("extra")
            .and_then(Value::as_object)
            .and_then(|extra| extra.get("title"))
            .and_then(primitive_text),
        object_info: object_info.is_some(),
        nodes: Vec::new(),
        edges: Vec::new(),
        groups: Vec::new(),
        issues: Vec::new(),
        warnings: Vec::new(),
    };

    let subgraphs = subgraph_ids(ui);
    let raw_nodes: Vec<&Map<String, Value>> = ui
        .get("nodes")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();

    // 第一节：节点
    let mut seen: HashSet<String> = HashSet::new();
    let mut duplicates: Vec<String> = Vec::new();
    for raw in &raw_nodes {
        let Some(id) = node_id(raw) else { continue };
        if !seen.insert(id.clone()) {
            duplicates.push(id.clone());
        }
        let class = ui_class(raw);
        let (kind, frontend_only) = classify(&class, object_info, Some(&subgraphs));
        let widgets = ui_widgets(raw, &class, object_info);
        blueprint.nodes.push(Node {
            id,
            class: class.clone(),
            title: raw
                .get("title")
                .and_then(primitive_text)
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| class.clone()),
            pos: pair(raw.get("pos")),
            size: pair(raw.get("size")),
            mode: as_int(raw.get("mode")).unwrap_or(0),
            kind,
            frontend_only,
            inputs: ui_inputs(raw, &widgets),
            outputs: ui_outputs(raw),
            widgets,
        });
    }

    // 第二节：连线（`links` 表是权威；节点上的 `links` / `inputs[].link` 用来交叉核对）
    let table = raw_links(ui.get("links"));
    for (id, link) in &table {
        blueprint.edges.push(Edge {
            id: Some(*id),
            from: (link.origin.clone(), link.origin_slot),
            to: (link.target.clone(), link.target_slot),
            type_name: link.type_name.clone(),
            implicit: false,
        });
    }
    blueprint.edges.sort_by_key(|edge| edge.id.unwrap_or(i64::MAX));

    // 第三节：分组
    if let Some(list) = ui.get("groups").and_then(Value::as_array) {
        for group in list.iter().filter_map(Value::as_object) {
            blueprint.groups.push(Group {
                title: group.get("title").and_then(primitive_text).unwrap_or_default(),
                bounding: group
                    .get("bounding")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default(),
                color: group.get("color").and_then(primitive_text),
            });
        }
    }

    if !subgraphs.is_empty() {
        blueprint.warnings.push(format!(
            "这份工作流里有 {} 个组节点（subgraph）定义：蓝图上画的是**实例**那一个框，\
             转换时会按定义展开成内部节点",
            subgraphs.len()
        ));
    }
    if !duplicates.is_empty() {
        blueprint.issues.push(Issue {
            code: "DUPLICATE_ID",
            severity: SEV_ERROR,
            title: format!("有 {} 个节点用了重复的 id", duplicates.len()),
            detail: format!("重复的 id：{}。后一个会把前一个顶掉（ComfyUI 加载时就这样）", list_ids(&duplicates)),
            fix: Some("在 ComfyUI 里重新保存一次这份工作流，或手工给重复的节点换 id".to_string()),
            nodes: dedup_sorted(&duplicates),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    if diagnose {
        diagnose_ui(&mut blueprint, ui, object_info, &table);
    }
    blueprint
}

/// `definitions.subgraphs[]` 的 id 集合（组节点在主图里就以这些 UUID 当类名）。
fn subgraph_ids(ui: &Map<String, Value>) -> HashSet<String> {
    ui.get("definitions")
        .and_then(Value::as_object)
        .and_then(|defs| defs.get("subgraphs"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_object)
                .filter_map(|def| def.get("id").and_then(primitive_text))
                .collect()
        })
        .unwrap_or_default()
}

fn ui_inputs(raw: &Map<String, Value>, widgets: &[Widget]) -> Vec<Port> {
    let mut out = Vec::new();
    let Some(list) = raw.get("inputs").and_then(Value::as_array) else { return out };
    for (index, entry) in list.iter().enumerate() {
        let Some(entry) = entry.as_object() else { continue };
        let name = entry.get("name").and_then(primitive_text).unwrap_or_default();
        let widget_name = entry
            .get("widget")
            .and_then(Value::as_object)
            .map(|widget| widget.get("name").and_then(primitive_text).unwrap_or_else(|| name.clone()));
        let widget = widget_name.is_some();
        let value = widget_name
            .as_ref()
            .and_then(|wanted| {
                widgets
                    .iter()
                    .find(|widget| widget.name.as_deref() == Some(wanted.as_str()))
                    .map(|widget| widget.value.clone())
            })
            .or_else(|| widgets.get(index).map(|widget| widget.value.clone()).filter(|_| widget));
        out.push(Port {
            name,
            type_name: entry.get("type").and_then(primitive_text).unwrap_or_default(),
            link: as_int(entry.get("link")),
            links: Vec::new(),
            widget,
            value,
        });
    }
    out
}

fn ui_outputs(raw: &Map<String, Value>) -> Vec<Port> {
    let mut out = Vec::new();
    let Some(list) = raw.get("outputs").and_then(Value::as_array) else { return out };
    for entry in list.iter() {
        let Some(entry) = entry.as_object() else { continue };
        out.push(Port {
            name: entry.get("name").and_then(primitive_text).unwrap_or_default(),
            type_name: entry.get("type").and_then(primitive_text).unwrap_or_default(),
            link: None,
            links: entry
                .get("links")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(|item| as_int(Some(item))).collect())
                .unwrap_or_default(),
            widget: false,
            value: None,
        });
    }
    out
}

/// 控件值 + 名字：优先用 `inputs[].widget.name` 的**顺序**（那就是前端的顺序），
/// 对不上长度再退到 `widgets_values_named` 的键序，最后才用 `/object_info` 的声明顺序。
///
/// ⚠ 只有位置、没有名字的情形是**真实存在**的（旧存档 + ComfyUI 不在线）：
/// 那时如实给 `None`，界面显示"参数 #0"，**不猜名字**。
fn ui_widgets(raw: &Map<String, Value>, class: &str, object_info: Option<&Map<String, Value>>) -> Vec<Widget> {
    let Some(values) = raw.get("widgets_values") else { return Vec::new() };
    // 少数自定义节点把控件存成对象：那时名字是现成的
    if let Value::Object(map) = values {
        return map
            .iter()
            .enumerate()
            .map(|(index, (name, value))| Widget { index, name: Some(name.clone()), value: value.clone() })
            .collect();
    }
    let Some(list) = values.as_array() else { return Vec::new() };
    let from_inputs: Vec<String> = raw
        .get("inputs")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|entry| entry.get("widget").and_then(Value::as_object))
                .map(|widget| widget.get("name").and_then(primitive_text).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let from_named: Vec<String> = raw
        .get("widgets_values_named")
        .and_then(Value::as_object)
        .map(|named| named.keys().cloned().collect())
        .unwrap_or_default();
    let from_object_info: Vec<Option<String>> = object_info
        .and_then(|info| info.get(class))
        .and_then(Value::as_object)
        .map(conv::widget_names)
        .unwrap_or_default();

    let names: Vec<Option<String>> = if from_inputs.len() == list.len() {
        from_inputs.into_iter().map(Some).collect()
    } else if from_named.len() == list.len() {
        from_named.into_iter().map(Some).collect()
    } else if from_object_info.len() == list.len() {
        from_object_info
    } else {
        list.iter()
            .enumerate()
            .map(|(index, _)| from_inputs.get(index).cloned().or_else(|| from_object_info.get(index).cloned().flatten()))
            .collect()
    };

    let named_values = raw.get("widgets_values_named").and_then(Value::as_object);
    list.iter()
        .enumerate()
        .map(|(index, value)| {
            let name = names.get(index).cloned().flatten();
            let value = name
                .as_ref()
                .and_then(|name| named_values.and_then(|map| map.get(name)))
                .cloned()
                .unwrap_or_else(|| value.clone());
            Widget { index, name, value }
        })
        .collect()
}

/// 界面格式的接线体检。
fn diagnose_ui(
    blueprint: &mut Blueprint,
    ui: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
    table: &HashMap<i64, conv::RawLink>,
) {
    let by_id: HashMap<String, usize> =
        blueprint.nodes.iter().enumerate().map(|(index, node)| (node.id.clone(), index)).collect();

    // ① 线的一头指向不存在的节点
    let mut dangling: Vec<String> = Vec::new();
    for edge in &blueprint.edges {
        let mut missing: Vec<String> = Vec::new();
        if !by_id.contains_key(&edge.from.0) {
            missing.push(format!("起点 {}", edge.from.0));
        }
        if !by_id.contains_key(&edge.to.0) {
            missing.push(format!("终点 {}", edge.to.0));
        }
        if !missing.is_empty() {
            dangling.push(format!(
                "线 {}{}（{} → {}）",
                edge.id.map(|id| id.to_string()).unwrap_or_else(|| "?".to_string()),
                if missing.len() > 1 { " 两头都缺" } else { "" },
                if missing.iter().any(|m| m.starts_with("起点")) { "?" } else { edge.from.0.as_str() },
                if missing.iter().any(|m| m.starts_with("终点")) { "?" } else { edge.to.0.as_str() },
            ));
        }
    }
    if !dangling.is_empty() {
        blueprint.issues.push(Issue {
            code: "LINK_DANGLING",
            severity: SEV_ERROR,
            title: format!("有 {} 条线接到了不存在的节点", dangling.len()),
            detail: format!(
                "{}。这种图提交上去，ComfyUI 只会回一句「某个节点不存在」，用户完全看不出是哪一步坏的",
                dangling.join("、")
            ),
            fix: Some("在 ComfyUI 里重新打开这份工作流并另存一次（多半是被外部工具改过）".to_string()),
            nodes: Vec::new(),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    // ② 节点输入写着接了线，但 `links` 表里没有
    let mut missing_links: Vec<String> = Vec::new();
    for node in &blueprint.nodes {
        for input in &node.inputs {
            if let Some(id) = input.link {
                if !table.contains_key(&id) {
                    missing_links.push(format!("线 {id}（{}．{} 说它接了这条）", node.id, input.name));
                }
            }
        }
        for output in &node.outputs {
            for id in &output.links {
                if !table.contains_key(id) {
                    missing_links.push(format!("线 {id}（{} 的 {} 说它连出去了）", node.id, output.name));
                }
            }
        }
    }
    if !missing_links.is_empty() {
        blueprint.issues.push(Issue {
            code: "LINK_NOT_FOUND",
            severity: SEV_ERROR,
            title: format!("有 {} 处引用了 `links` 表里不存在的线", missing_links.len()),
            detail: list_ids(&missing_links),
            fix: Some("同样是在 ComfyUI 里重新保存一次".to_string()),
            nodes: Vec::new(),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    // ③ 连线表与插口对不上（改了图但没同步）
    let mut mismatched: Vec<String> = Vec::new();
    for (id, link) in table.iter() {
        let Some(&target) = by_id.get(&link.target) else { continue };
        if link.target_slot < 0 {
            continue;
        }
        let node = &blueprint.nodes[target];
        match node.inputs.get(link.target_slot as usize) {
            None => mismatched.push(format!("线 {id} 说它插在 {} 的第 {} 个输入，但那个节点只有 {} 个输入", node.id, link.target_slot, node.inputs.len())),
            Some(input) => {
                if input.link != Some(*id) {
                    mismatched.push(format!(
                        "线 {id} 说它插在 {}．{}，但那个插口上写的是 {:?}",
                        node.id, input.name, input.link
                    ));
                }
            }
        }
    }
    if !mismatched.is_empty() {
        blueprint.issues.push(Issue {
            code: "LINK_SLOT_MISMATCH",
            severity: SEV_WARN,
            title: format!("连线表与节点插口对不上（{} 处）", mismatched.len()),
            detail: list_ids(&mismatched),
            fix: Some("多数是版本迁移的残留；跑之前让 ComfyUI 重新保存一次就正常了".to_string()),
            nodes: Vec::new(),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    // ④ 类型：连线上记的类型 vs 两头声明的类型
    let mut mismatches: Vec<Issue> = Vec::new();
    for edge in &blueprint.edges {
        let (Some(&target), Some(&source)) = (by_id.get(&edge.to.0), by_id.get(&edge.from.0)) else {
            continue;
        };
        let target_node = &blueprint.nodes[target];
        let source_node = &blueprint.nodes[source];
        let input = target_node.inputs.get(edge.to.1.max(0) as usize);
        let output = source_node.outputs.get(edge.from.1.max(0) as usize);
        let link_type = match edge.type_name.as_deref() {
            Some(text) if !text.is_empty() => text.to_string(),
            _ => continue,
        };
        // 上游输出声明的类型 vs 连线上记的类型：对不上说明存档被改过（只提醒）
        if let Some(output) = output {
            let declared = output.type_name.as_str();
            if !declared.is_empty() && declared != "*" && declared != link_type {
                mismatches.push(Issue {
                    code: "LINK_SLOT_MISMATCH",
                    severity: SEV_WARN,
                    title: "连线的类型与上游输出声明的类型不一致".to_string(),
                    detail: format!(
                        "线 {}（{}．{} → {}．{}）上记的是 {link_type}，但 {} 的输出声明是 {declared}",
                        edge.id.map(|id| id.to_string()).unwrap_or_else(|| "?".to_string()),
                        source_node.id,
                        output.name,
                        target_node.id,
                        input.map(|port| port.name.as_str()).unwrap_or("?"),
                        source_node.id,
                    ),
                    fix: Some("在 ComfyUI 里重新保存一次这份工作流".to_string()),
                    nodes: vec![source_node.id.clone(), target_node.id.clone()],
                    links: edge.id.into_iter().collect(),
                    slots: Vec::new(),
                });
            }
        }
        // 目标输入声明的类型 vs 连线上记的类型：**这是真正的接错线**
        if let Some(input) = input {
            let declared = input.type_name.as_str();
            if declared.is_empty() || declared == "*" || declared == link_type {
                continue;
            }
            if !type_compatible(Decl::Name(declared.to_string()), &link_type) {
                mismatches.push(Issue {
                    code: "TYPE_MISMATCH",
                    severity: SEV_ERROR,
                    title: format!("{}．{} 接错了类型", target_node.id, input.name),
                    detail: format!(
                        "线 {} 把 {} 的 {}（{link_type}）接到 {}．{}（要 {declared}）——\
                         ComfyUI 的原话是 `Return type mismatch between linked nodes`",
                        edge.id.map(|id| id.to_string()).unwrap_or_else(|| "?".to_string()),
                        source_node.id,
                        output.map(|port| port.name.as_str()).unwrap_or("?"),
                        target_node.id,
                        input.name,
                    ),
                    fix: Some(format!(
                        "断开这条线：{}．{} 需要的是 {declared}",
                        target_node.id, input.name
                    )),
                    nodes: vec![source_node.id.clone(), target_node.id.clone()],
                    links: edge.id.into_iter().collect(),
                    slots: vec![format!("{}.{}", target_node.id, input.name)],
                });
            }
        }
    }
    blueprint.issues.extend(mismatches);

    // ⑤ 空着的插口（广播型节点就是往这些地方灌值的）
    //
    // ⚠ 只在**拿不到 `/object_info`** 时由这里说：拿到节点定义时，⑨ 那一节用
    // **转换之后的图**判得更准（控件填充、穿透、悬挂引用都在里面），
    // 两处都报就是同一个事实写两遍 —— 而且这里分不出"必填"与"可选"，
    // 会把可选插口也报成错误（`TextGenerate.video` 那种）。
    let mut open: Vec<(String, String, String)> = Vec::new();
    for node in &blueprint.nodes {
        if node.mode != 0 {
            continue;
        }
        for input in &node.inputs {
            if input.widget || input.link.is_some() {
                continue;
            }
            open.push((node.id.clone(), input.name.clone(), input.type_name.clone()));
        }
    }
    if !open.is_empty() && object_info.is_none() {
        let listed = open
            .iter()
            .map(|(id, name, type_name)| format!("{id}.{name}({type_name})"))
            .collect::<Vec<_>>();
        blueprint.issues.push(Issue {
            code: "INPUT_UNCONNECTED",
            severity: SEV_WARN,
            title: format!("有 {} 个输入插口空着", open.len()),
            detail: format!(
                "这些插口现在空着（没拿到 `/object_info`，判不出是不是必填，只提醒）：{}",
                list_ids(&listed)
            ),
            fix: Some(
                "两条路：① 在 ComfyUI 里把线连上（或让广播节点接管）；\
                 ② 提交时开 ##模拟用户行为##（`comfy_load_workflow(emulateFrontend=true)`），\
                 由后端照前端的做法把同类型的上游值填进去"
                    .to_string(),
            ),
            nodes: dedup_sorted(&open.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>()),
            links: Vec::new(),
            slots: open.iter().map(|(id, name, _)| format!("{id}.{name}")).collect(),
        });
    }

    // ⑥ 环
    if let Some(cycle) = find_cycle(&blueprint.edges) {
        blueprint.issues.push(Issue {
            code: "CYCLE",
            severity: SEV_ERROR,
            title: "图里有环（连线绕回了自己的上游）".to_string(),
            detail: format!("环上的节点：{}", list_ids(&cycle)),
            fix: Some("断开其中一条线 —— ComfyUI 不接受有环的图".to_string()),
            nodes: cycle,
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    // ⑦ 纯前端节点 / 本机没有的类 / 零输出桩的广播节点
    diagnose_classes(blueprint, object_info);

    // ⑧ 一个输出节点都没有
    if let Some(info) = object_info {
        let outputs = blueprint
            .nodes
            .iter()
            .filter(|node| {
                info.get(&node.class)
                    .and_then(Value::as_object)
                    .and_then(|entry| entry.get("output_node"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count();
        if outputs == 0 {
            blueprint.issues.push(Issue {
                code: "NO_OUTPUT_NODE",
                severity: SEV_ERROR,
                title: "这张图一个输出节点都没有".to_string(),
                detail: "（`SaveImage` / `PreviewImage` / `SaveVideo` 这类 `output_node=true` 的节点）提交上去只会得到 `Prompt has no outputs`".to_string(),
                fix: Some("让 ##模拟用户行为## 自动补一个 `SaveImage`，或在 ComfyUI 里手动接一个".to_string()),
                nodes: Vec::new(),
                links: Vec::new(),
                slots: Vec::new(),
            });
        }
    }

    // ⑨ 拿转换那一遍的结论做交叉核对（提交前走的就是同一条转换链）。
    //
    // ⚠ 「哪些节点不参与出图」也在这一节里判：判据必须是**转换之后**的图 ——
    // 界面图里 `Anything Everywhere` 的下游看不出连线（它们由前端广播补上），
    // 直接按界面图算会把"真正在干活的那一串"全报成"不参与出图"（2026-10-17
    // 在用户的 `krea2 t2i workflow.json` 上实测到：28/29/30/32/35 全被误报）。
    if let (true, Some(info)) = (blueprint.object_info, object_info) {
        diagnose_conversion(blueprint, ui, info);
    }
}

/// 纯前端节点 / 本机没有的类 / 零输出桩的广播节点。
fn diagnose_classes(blueprint: &mut Blueprint, object_info: Option<&Map<String, Value>>) {
    let mut frontend: Vec<(String, String)> = Vec::new();
    let mut unknown: Vec<(String, String)> = Vec::new();
    let mut stubs: Vec<(String, String)> = Vec::new();
    for node in &blueprint.nodes {
        if node.frontend_only {
            frontend.push((node.id.clone(), node.class.clone()));
            continue;
        }
        let Some(info) = object_info else { continue };
        match info.get(&node.class).and_then(Value::as_object) {
            None => unknown.push((node.id.clone(), node.class.clone())),
            // 注册了、但注册成"零输出"：它的语义只在前端（cg-use-everywhere 的
            // `Anything Everywhere` 就是这么注册的）—— 服务端拿到它什么也不产出
            Some(entry) => {
                if is_broadcast(&node.class) && outputs_of(entry).map(|out| out.is_empty()).unwrap_or(false) {
                    stubs.push((node.id.clone(), node.class.clone()));
                }
            }
        }
    }
    if !frontend.is_empty() {
        blueprint.issues.push(Issue {
            code: "FRONTEND_NODE",
            severity: SEV_WARN,
            title: format!("图里还有 {} 个纯前端节点（服务端没有这些类）", frontend.len()),
            detail: format!(
                "{}。这些类只活在前端（注释框 / 转发点 / Set-Get / 广播 / 显示）；\
                 服务端拿到它们会在验证阶段整片丢掉",
                list_pairs(&frontend)
            ),
            fix: Some(
                "转换时开 ##模拟用户行为##（`emulateFrontend=true`），或让 `comfy_prune_workflow` 把它们摘掉并补缺口"
                    .to_string(),
            ),
            nodes: dedup_sorted(&frontend.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }
    if !unknown.is_empty() {
        blueprint.issues.push(Issue {
            code: "UNKNOWN_CLASS",
            severity: SEV_ERROR,
            title: format!("本机 ComfyUI 里没有这 {} 个节点类", unknown.len()),
            detail: format!("`/object_info` 里查不到：{}。提交一定会被验证拒绝", list_pairs(&unknown)),
            fix: Some("装上对应的自定义节点包（ComfyUI-Manager），或换个装了它的实例".to_string()),
            nodes: dedup_sorted(&unknown.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }
    if !stubs.is_empty() {
        blueprint.issues.push(Issue {
            code: "BROADCAST_STUB",
            severity: SEV_WARN,
            title: format!("有 {} 个广播节点在本机被注册成「零输出」的桩", stubs.len()),
            detail: format!(
                "{}。它们的「把这一路值广播给全图同类型的空输入」是**前端**行为：\
                 服务端拿到它就是一个什么都不产出的空节点，而它该填的那几路会一直空着",
                list_pairs(&stubs)
            ),
            fix: Some(
                "① 提交时开 ##模拟用户行为##（`comfy_load_workflow(emulateFrontend=true)`）由后端等价改写；\
                 ② 或者在 ComfyUI 里把这些线显式连好再另存"
                    .to_string(),
            ),
            nodes: dedup_sorted(&stubs.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }
}

/// 把转换那一遍的结论搬进体检报告：**提交前走的就是这条链**，
/// 于是"蓝图说没问题、提交却缺输入"这种自相矛盾不可能出现。
///
/// 用的是**开 ##模拟用户行为## 之后**的那张图（提交链推荐走的那一份）：广播补的线、
/// 丢掉的显示节点、自动补的 `SaveImage` 都在里面 —— 判"谁在干活"只能看它。
fn diagnose_conversion(blueprint: &mut Blueprint, ui: &Map<String, Value>, object_info: &Map<String, Value>) {
    let plain = conv::to_api_graph(ui, object_info, false);
    let emulated = conv::to_api_graph(ui, object_info, true);
    let Some(report) = emulated.as_ref().ok().or_else(|| plain.as_ref().ok()) else { return };
    let graph = report.graph.clone();
    for warning in &report.warnings {
        blueprint.warnings.push(warning.clone());
    }
    if !report.warnings.is_empty() {
        blueprint.issues.push(Issue {
            code: "PARAM_MISMATCH",
            severity: SEV_WARN,
            title: format!("转换时有 {} 处参数对不齐的提醒", report.warnings.len()),
            detail: report.warnings.join("；"),
            fix: Some("多半是自定义节点的控件与后端声明不一致：跑之前留意 ComfyUI 的报错".to_string()),
            nodes: Vec::new(),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }
    if let Some(issue) = unreachable_issue(blueprint, &graph, object_info) {
        blueprint.issues.push(issue);
    }

    // 直接转换留下的缺口（悬挂引用 / 空必填）：这是"交上去一定跑不成"的硬结论。
    //
    // ⚠ 只算**必填**（`required_open_inputs`）：转换器给的 `open_inputs` 把**可选**插口
    // 也算进去了（那是给广播用的候选清单），拿它当"缺必填"会凭空多出一片假警报
    // （`TextGenerate.video` / `audio` 就是这种可选项）。
    let Ok(plain) = &plain else { return };
    let required_open: Vec<String> = plain.required_open_inputs.clone();
    if !plain.unresolved_inputs.is_empty() || !required_open.is_empty() {
        let mut detail = String::new();
        if !plain.unresolved_inputs.is_empty() {
            detail.push_str(&format!("悬挂引用：{}。", list_ids(&plain.unresolved_inputs)));
        }
        if !required_open.is_empty() {
            detail.push_str(&format!("空着的必填输入：{}。", list_ids(&required_open)));
        }
        let saved = emulated.as_ref().ok().filter(|report| {
            report.unresolved_inputs.is_empty()
                && report.open_inputs.iter().all(|text| !required_open.contains(text))
        });
        let (severity, fix) = match saved {
            Some(_) => (
                SEV_WARN,
                "这份图**开 ##模拟用户行为## 之后缺口就没了**：走 \
                 `comfy_load_workflow(emulateFrontend=true)`（或带 `emulateFrontend=true` 的提交链）即可"
                    .to_string(),
            ),
            None => (
                SEV_ERROR,
                "按提示把这些输入接上（界面里能直接看到是哪几个插口）".to_string(),
            ),
        };
        blueprint.issues.push(Issue {
            code: "CONVERT_GAP",
            severity,
            title: "按提交链转换这份图会留下缺口".to_string(),
            detail,
            fix: Some(fix),
            nodes: Vec::new(),
            links: Vec::new(),
            // 界面按 `节点id.输入名` 匹配插口（`open_inputs` 里那串带类型的只给报告看）
            slots: required_open.iter().map(|text| slot_key(text)).collect(),
        });
    }
}

/// 「有节点不参与出图」这一条：判据是**转换之后的图**（API 图）里从输出节点往回走。
///
/// 返回 `None` 表示没有这种情况（或这张图连输出节点都没有 —— 那由 `NO_OUTPUT_NODE` 说）。
fn unreachable_issue(
    blueprint: &Blueprint,
    graph: &Map<String, Value>,
    object_info: &Map<String, Value>,
) -> Option<Issue> {
    let mut upstream: HashMap<String, Vec<String>> = HashMap::new();
    for (id, value) in graph {
        let Some(node) = value.as_object() else { continue };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        let entry = upstream.entry(id.clone()).or_default();
        for value in inputs.values() {
            let Some(reference) = value.as_array() else { continue };
            let Some(source) = reference.first().and_then(primitive_text) else { continue };
            if graph.contains_key(&source) {
                entry.push(source);
            }
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = graph
        .iter()
        .filter(|(_, value)| {
            value
                .as_object()
                .and_then(|node| node.get("class_type"))
                .and_then(primitive_text)
                .map(|class| {
                    object_info
                        .get(&class)
                        .and_then(Value::as_object)
                        .and_then(|entry| entry.get("output_node"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(parents) = upstream.get(id.as_str()) {
            for parent in parents {
                stack.push((*parent).to_string());
            }
        }
    }
    let dead: Vec<String> = blueprint
        .nodes
        .iter()
        .filter(|node| node.mode == 0 && !node.frontend_only && !seen.contains(&node.id))
        .map(|node| format!("{} {}", node.id, node.title))
        .collect();
    if dead.is_empty() {
        return None;
    }
    Some(Issue {
        code: "UNREACHABLE",
        severity: SEV_INFO,
        title: format!("有 {} 个节点不参与出图（ComfyUI 不会执行它们）", dead.len()),
        detail: format!(
            "{}。多半是旁路 / 未启用的分支，或者忘了接到输出节点上（广播节点本身也算：它不产出任何东西）",
            list_ids(&dead)
        ),
        fix: None,
        nodes: dead.iter().filter_map(|text| text.split(' ').next().map(str::to_string)).collect(),
        links: Vec::new(),
        slots: Vec::new(),
    })
}

/// 图里有没有环；有就回环上的节点（最多 12 个，够说明问题）。
fn find_cycle(edges: &[Edge]) -> Option<Vec<String>> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        adjacency.entry(edge.from.0.as_str()).or_default().push(edge.to.0.as_str());
    }
    let mut state: HashMap<&str, u8> = HashMap::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut cycle: Option<Vec<String>> = None;
    // 迭代式 DFS（显式栈）—— 上千节点的图不能靠递归把测试线程的栈吃光
    let mut roots: Vec<&str> = adjacency.keys().copied().collect();
    roots.sort_unstable();
    for root in roots {
        if state.get(root).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut work: Vec<(&str, usize)> = vec![(root, 0)];
        while let Some((node, index)) = work.pop() {
            if index == 0 {
                if state.get(node).copied().unwrap_or(0) == 1 {
                    continue;
                }
                state.insert(node, 1);
                stack.push(node);
            }
            let children = adjacency.get(node).cloned().unwrap_or_default();
            if index < children.len() {
                work.push((node, index + 1));
                let child = children[index];
                match state.get(child).copied().unwrap_or(0) {
                    1 => {
                        let start = stack.iter().position(|item| *item == child).unwrap_or(0);
                        cycle = Some(stack[start..].iter().map(|item| (*item).to_string()).collect());
                        break;
                    }
                    0 => work.push((child, 0)),
                    _ => {}
                }
                continue;
            }
            state.insert(node, 2);
            stack.pop();
        }
        if cycle.is_some() {
            break;
        }
    }
    cycle.map(|mut ids| {
        ids.truncate(12);
        ids
    })
}

// ---------------------------------------------------------------------------
// API 格式
// ---------------------------------------------------------------------------

fn build_api(
    graph: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
    diagnose: bool,
) -> Blueprint {
    let mut blueprint = Blueprint {
        format: "api",
        title: None,
        object_info: object_info.is_some(),
        nodes: Vec::new(),
        edges: Vec::new(),
        groups: Vec::new(),
        issues: Vec::new(),
        warnings: vec![
            "API 格式节点图**没有保存画布坐标**：界面上的位置是自动排出来的，\
             与你在 ComfyUI 里看到的那张图不是同一张（它只含真正被执行的节点）"
                .to_string(),
        ],
    };

    for (id, value) in graph {
        let Some(node) = value.as_object() else { continue };
        let class = node.get("class_type").and_then(primitive_text).unwrap_or_default();
        if class.trim().is_empty() {
            continue;
        }
        let (kind, frontend_only) = classify(&class, object_info, None);
        let declared = object_info
            .and_then(|info| info.get(&class))
            .and_then(Value::as_object)
            .map(|entry| api_ports(entry, node));
        let (inputs, outputs, widgets) = match declared {
            Some(ports) => ports,
            None => (api_ports_offline(node), Vec::new(), Vec::new()),
        };
        blueprint.nodes.push(Node {
            id: id.clone(),
            class: class.clone(),
            title: node
                .get("_meta")
                .and_then(Value::as_object)
                .and_then(|meta| meta.get("title"))
                .and_then(primitive_text)
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(class),
            pos: None,
            size: None,
            mode: 0,
            kind,
            frontend_only,
            inputs,
            outputs,
            widgets,
        });
    }

    let ids: HashSet<String> = blueprint.nodes.iter().map(|node| node.id.clone()).collect();
    for node in &blueprint.nodes {
        let Some(raw) = graph.get(&node.id).and_then(Value::as_object) else { continue };
        let Some(inputs) = raw.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            let Some(reference) = value.as_array() else { continue };
            let Some(source) = reference.first().and_then(primitive_text) else { continue };
            if ids.contains(&source) {
                blueprint.edges.push(Edge {
                    id: None,
                    from: (source, as_int(reference.get(1)).unwrap_or(0)),
                    to: (node.id.clone(), node.inputs.iter().position(|port| &port.name == name).unwrap_or(0) as i64),
                    type_name: None,
                    implicit: true,
                });
            }
        }
    }

    if diagnose {
        diagnose_api(&mut blueprint, graph, object_info);
    }
    blueprint
}

/// `/object_info` 在手上的 API 图：插口按声明顺序建，值从 `inputs` 里取。
fn api_ports(
    entry: &Map<String, Value>,
    node: &Map<String, Value>,
) -> (Vec<Port>, Vec<Port>, Vec<Widget>) {
    let given = node.get("inputs").and_then(Value::as_object);
    let mut inputs: Vec<Port> = Vec::new();
    let mut widgets: Vec<Widget> = Vec::new();
    if let Some(declared) = entry.get("input").and_then(Value::as_object) {
        let mut widget_index = 0usize;
        for group in ["required", "optional"] {
            let Some(map) = declared.get(group).and_then(Value::as_object) else { continue };
            for (name, decl) in map {
                let value = given.and_then(|map| map.get(name));
                let widget = conv::is_widget_input(decl);
                if widget {
                    widgets.push(Widget {
                        index: widget_index,
                        name: Some(name.clone()),
                        value: value.cloned().unwrap_or(Value::Null),
                    });
                    widget_index += 1;
                }
                inputs.push(Port {
                    name: name.clone(),
                    type_name: type_text(decl),
                    link: None,
                    links: Vec::new(),
                    widget,
                    value: value.cloned(),
                });
            }
        }
    }
    let outputs = outputs_of(entry)
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(index, item)| Port {
                    name: format!("输出 {index}"),
                    type_name: item.as_str().unwrap_or_default().to_string(),
                    link: None,
                    links: Vec::new(),
                    widget: false,
                    value: None,
                })
                .collect()
        })
        .unwrap_or_default();
    (inputs, outputs, widgets)
}

/// 没有 `/object_info` 时的兜底：只能按 `inputs` 的键建插口，类型与输出都判不出来。
fn api_ports_offline(node: &Map<String, Value>) -> Vec<Port> {
    node.get("inputs")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, value)| Port {
                    name: name.clone(),
                    type_name: String::new(),
                    link: None,
                    links: Vec::new(),
                    widget: !value.is_array(),
                    value: Some(value.clone()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnose_api(
    blueprint: &mut Blueprint,
    graph: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
) {
    let index: HashMap<String, usize> =
        blueprint.nodes.iter().enumerate().map(|(position, node)| (node.id.clone(), position)).collect();

    // ① 指向不存在的节点
    let mut dangling: Vec<String> = Vec::new();
    for (id, value) in graph {
        let Some(node) = value.as_object() else { continue };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            let Some(reference) = value.as_array() else { continue };
            let Some(source) = reference.first().and_then(primitive_text) else { continue };
            if !index.contains_key(&source) {
                dangling.push(format!("{id}.{name} → 节点 {source}"));
            }
        }
    }
    if !dangling.is_empty() {
        blueprint.issues.push(Issue {
            code: "LINK_DANGLING",
            severity: SEV_ERROR,
            title: format!("有 {} 个输入指向不存在的节点", dangling.len()),
            detail: list_ids(&dangling),
            fix: Some("改掉这些引用（提交上去 ComfyUI 只会说「节点不存在」）".to_string()),
            nodes: Vec::new(),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    let mut unknown: Vec<(String, String)> = Vec::new();
    if let Some(info) = object_info {
        for node in &blueprint.nodes {
            if info.get(&node.class).is_none() && !node.frontend_only {
                unknown.push((node.id.clone(), node.class.clone()));
            }
        }
    }
    if !unknown.is_empty() {
        blueprint.issues.push(Issue {
            code: "UNKNOWN_CLASS",
            severity: SEV_ERROR,
            title: format!("本机 ComfyUI 里没有这 {} 个节点类", unknown.len()),
            detail: format!("`/object_info` 里查不到：{}", list_pairs(&unknown)),
            fix: Some("装上对应的自定义节点包，或换个装了它的实例".to_string()),
            nodes: dedup_sorted(&unknown.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()),
            links: Vec::new(),
            slots: Vec::new(),
        });
    }

    // ② 必填输入空着 + 类型对不上（都要 `/object_info`）
    if let Some(info) = object_info {
        let mut missing: Vec<String> = Vec::new();
        let mut mismatches: Vec<Issue> = Vec::new();
        for node in &blueprint.nodes {
            let Some(entry) = info.get(&node.class).and_then(Value::as_object) else { continue };
            let given = graph
                .get(&node.id)
                .and_then(Value::as_object)
                .and_then(|body| body.get("inputs"))
                .and_then(Value::as_object);
            let Some(declared) = entry.get("input").and_then(Value::as_object) else { continue };
            let Some(required) = declared.get("required").and_then(Value::as_object) else { continue };
            for (name, decl) in required {
                let value = given.and_then(|map| map.get(name));
                let linked = value.map(Value::is_array).unwrap_or(false);
                if !linked && conv::is_widget_input(decl) {
                    // 控件没给值时 ComfyUI 用默认值，不算缺口
                    continue;
                }
                match value {
                    None => missing.push(format!("{}.{name} ({})", node.id, type_text(decl))),
                    Some(reference) if linked => {
                        let Some(source) = reference.as_array().and_then(|items| items.first()).and_then(primitive_text) else {
                            continue;
                        };
                        let source_type = reference
                            .as_array()
                            .and_then(|items| as_int(items.get(1)))
                            .and_then(|slot| {
                                index
                                    .get(&source)
                                    .and_then(|position| blueprint.nodes[*position].outputs.get(slot.max(0) as usize))
                                    .map(|port| port.type_name.clone())
                            });
                        let Some(source_type) = source_type.filter(|text| !text.is_empty()) else { continue };
                        if type_compatible(decl_of(decl), &source_type) {
                            continue;
                        }
                        mismatches.push(Issue {
                            code: "TYPE_MISMATCH",
                            severity: SEV_ERROR,
                            title: format!("{}．{name} 接错了类型", node.id),
                            detail: format!(
                                "节点 {source} 的输出是 {source_type}，而 {}.{name} 要 {}（ComfyUI 的原话是 \
                                 `Return type mismatch between linked nodes`）",
                                node.id,
                                type_text(decl)
                            ),
                            fix: Some(format!("换一条线：{}．{name} 需要的是 {}", node.id, type_text(decl))),
                            nodes: vec![source, node.id.clone()],
                            links: Vec::new(),
                            slots: vec![format!("{}.{name}", node.id)],
                        });
                    }
                    _ => {}
                }
            }
        }
        if !missing.is_empty() {
            blueprint.issues.push(Issue {
                code: "INPUT_UNCONNECTED",
                severity: SEV_ERROR,
                title: format!("有 {} 个必填输入空着", missing.len()),
                detail: format!("提交时 ComfyUI 会拒掉整张图：{}", list_ids(&missing)),
                fix: Some("把线接上，或用 `comfy_submit(connections=…)` 点名补线".to_string()),
                nodes: Vec::new(),
                links: Vec::new(),
                slots: missing,
            });
        }
        blueprint.issues.extend(mismatches);

        let outputs = blueprint
            .nodes
            .iter()
            .filter(|node| {
                info.get(&node.class)
                    .and_then(Value::as_object)
                    .and_then(|entry| entry.get("output_node"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count();
        if outputs == 0 {
            blueprint.issues.push(Issue {
                code: "NO_OUTPUT_NODE",
                severity: SEV_ERROR,
                title: "这张图一个输出节点都没有".to_string(),
                detail: "提交上去只会得到 `Prompt has no outputs`".to_string(),
                fix: Some("接一个 `SaveImage` / `PreviewImage`".to_string()),
                nodes: Vec::new(),
                links: Vec::new(),
                slots: Vec::new(),
            });
        } else if let Some(issue) = unreachable_issue(blueprint, graph, info) {
            blueprint.issues.push(issue);
        }

        let frontend: Vec<(String, String)> = blueprint
            .nodes
            .iter()
            .filter(|node| node.frontend_only)
            .map(|node| (node.id.clone(), node.class.clone()))
            .collect();
        if !frontend.is_empty() {
            blueprint.issues.push(Issue {
                code: "FRONTEND_NODE",
                severity: SEV_ERROR,
                title: format!("API 图里混进了 {} 个纯前端节点", frontend.len()),
                detail: format!(
                    "{}。API 格式是「交给服务端的那一份」，正常不该有它们 —— \
                     说明这份图是手拼的，或者是从界面格式直接拷过来的",
                    list_pairs(&frontend)
                ),
                fix: Some("用 `comfy_load_workflow` 从界面格式转换（它会照前端行为等价改写）".to_string()),
                nodes: dedup_sorted(&frontend.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()),
                links: Vec::new(),
                slots: Vec::new(),
            });
        }
    }

    if let Some(cycle) = find_cycle(&blueprint.edges) {
        blueprint.issues.push(Issue {
            code: "CYCLE",
            severity: SEV_ERROR,
            title: "图里有环".to_string(),
            detail: format!("环上的节点：{}", list_ids(&cycle)),
            fix: Some("断开其中一条线".to_string()),
            nodes: cycle,
            links: Vec::new(),
            slots: Vec::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 类型声明的三种形态（都来自 `/object_info`）。
#[derive(Debug, Clone, PartialEq)]
enum Decl {
    /// `"*"`：什么都能接
    Wildcard,
    /// 一个具体类型名
    Name(String),
    /// 一组候选（下拉框）
    Choices(Vec<String>),
}

fn decl_of(raw: &Value) -> Decl {
    let Some(items) = raw.as_array() else { return Decl::Wildcard };
    match items.first() {
        Some(Value::Array(choices)) => Decl::Choices(
            choices.iter().filter_map(primitive_text).collect(),
        ),
        Some(Value::String(text)) if text == "*" => Decl::Wildcard,
        Some(Value::String(text)) => Decl::Name(text.clone()),
        _ => Decl::Wildcard,
    }
}

/// 类型文本（给报告看）。
fn type_text(raw: &Value) -> String {
    match decl_of(raw) {
        Decl::Wildcard => "*".to_string(),
        Decl::Name(name) => name,
        Decl::Choices(_) => "COMBO".to_string(),
    }
}

/// 上游给的 `source` 能不能接进 `decl` 声明的输入。
///
/// 判据刻意**保守**（宁可不报，也不要制造假警报）：
/// * `*` 两边都算通过；
/// * 下拉框（`Choices`）只在来源是**张量类**时才算错 —— 字符串进下拉框是 ComfyUI 允许的。
fn type_compatible(decl: Decl, source: &str) -> bool {
    if source.is_empty() || source == "*" {
        return true;
    }
    match decl {
        Decl::Wildcard => true,
        Decl::Name(name) => {
            name == "*"
                || name == source
                || (name.eq_ignore_ascii_case("combo") && (source == "STRING" || source == "COMBO"))
        }
        Decl::Choices(choices) => {
            if choices.is_empty() {
                return true;
            }
            if choices.iter().any(|choice| choice == source) {
                return true;
            }
            !TENSOR_TYPES.contains(&source)
        }
    }
}

fn outputs_of(entry: &Map<String, Value>) -> Option<&Vec<Value>> {
    entry.get("output").and_then(Value::as_array)
}

/// 一个类归到哪一类（`kind`）+ 是不是"服务端没有这个类"。
fn classify(
    class: &str,
    object_info: Option<&Map<String, Value>>,
    subgraphs: Option<&HashSet<String>>,
) -> (&'static str, bool) {
    if class.trim().is_empty() {
        return ("unknown", false);
    }
    if let Some(subgraphs) = subgraphs {
        if subgraphs.contains(class) {
            return ("subgraph", false);
        }
    }
    if is_broadcast(class) {
        return ("broadcast", registered(object_info, class).is_none());
    }
    if is_display(class) {
        return ("display", registered(object_info, class).is_none());
    }
    if KNOWN_FRONTEND_ONLY.contains(&class) {
        return (frontend_kind(class), true);
    }
    match object_info {
        // 拿不到 `/object_info` 时**不下结论**：按普通节点画（宁可少标，也不要乱标"未安装"）
        None => ("normal", false),
        Some(info) => {
            if info.contains_key(class) {
                ("normal", false)
            } else {
                ("unknown", false)
            }
        }
    }
}

fn registered<'a>(object_info: Option<&'a Map<String, Value>>, class: &str) -> Option<&'a Value> {
    object_info.and_then(|info| info.get(class))
}

fn frontend_kind(class: &str) -> &'static str {
    let lower = class.to_ascii_lowercase();
    if lower.contains("note") || lower.contains("label") || lower.contains("bookmark") || lower.contains("comment") {
        "note"
    } else if lower.contains("reroute") {
        "reroute"
    } else if lower.contains("setnode") || lower.contains("getnode") {
        "variable"
    } else if lower.contains("bypass") || lower.contains("muter") {
        "bypasser"
    } else {
        "unknown"
    }
}

fn pair(value: Option<&Value>) -> Option<(f64, f64)> {
    let list = value?.as_array()?;
    let x = list.first()?.as_f64()?;
    let y = list.get(1)?.as_f64()?;
    Some((x, y))
}

/// `"33.model (MODEL)"` → `"33.model"`（转换器给的清单带类型，界面只认键）。
fn slot_key(text: &str) -> String {
    match text.split_once(" (") {
        Some((key, _)) => key.to_string(),
        None => text.to_string(),
    }
}

/// 列表压成一行（**最多 20 项**，多了就报个数 —— 报告不是 dump）。
fn list_ids(items: &[String]) -> String {    const MAX: usize = 20;
    if items.len() <= MAX {
        return items.join("、");
    }
    format!("{}…（共 {} 项）", items[..MAX].join("、"), items.len())
}

fn list_pairs(items: &[(String, String)]) -> String {
    let listed: Vec<String> = items.iter().map(|(id, class)| format!("{id} {class}")).collect();
    list_ids(&listed)
}

/// 去重 + 按数字升序（与 `workflow_audit` 的 `sorted_ids` 同款：界面上"哪几个节点"要稳定）。
fn dedup_sorted(ids: &[String]) -> Vec<String> {
    let mut seen: HashSet<&String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if seen.insert(id) {
            out.push(id.clone());
        }
    }
    out.sort_by_key(|id| id.parse::<i64>().unwrap_or(i64::MAX));
    out
}

// ---------------------------------------------------------------------------
// HTTP：把蓝图交给界面
// ---------------------------------------------------------------------------

/// ```text
/// POST /api/workflow/blueprint   {raw, diagnose?} → 蓝图（含接线体检）
/// POST /api/workflow/save        {name?, raw}     → 把（改过的）工作流存成一份文件
/// ```
///
/// **为什么要收原文而不是只认 id**：界面允许在蓝图上改参数，改完要**就地再体检一次**
/// （不落库、不改原文件）。于是这一条是"给我这段 JSON，告诉我哪里接错了"。
/// 两条路共用 [`build`] 一份实现。
///
/// 错误体是 `{error, detail}`（与其余 `/api/*` 一致；界面的 `ApiClient` 先读 `detail`）。
pub fn router() -> axum::Router<crate::web::routes::AppState> {
    axum::Router::new()
        .route("/api/workflow/blueprint", axum::routing::post(post_blueprint))
        .route("/api/workflow/save", axum::routing::post(post_save))
}

/// `POST /api/workflow/blueprint` 的请求体。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BlueprintInput {
    /// 工作流原文（界面格式或 API 格式的 JSON 文本）
    pub raw: String,
    /// 要不要跑接线体检（默认 true）
    #[serde(default = "yes")]
    pub diagnose: bool,
}

/// `POST /api/workflow/save` 的请求体。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveInput {
    /// 文件名（可以带 `.json`，也可以不带）。**只取最后一段**，目录由后端定。
    pub name: Option<String>,
    /// 工作流原文
    pub raw: String,
}

/// `serde(default)` 对 `bool` 是 `false`，而这里的缺省必须是 `true`
/// （界面不传这个字段时是"我要体检"，不是"别体检"）。
fn yes() -> bool {
    true
}

async fn post_blueprint(
    axum::extract::State(state): axum::extract::State<crate::web::routes::AppState>,
    axum::Json(input): axum::Json<BlueprintInput>,
) -> Result<axum::Json<Value>, crate::web::routes::ApiError> {
    let raw: Value = parse_raw(&input.raw)?;
    // ⚡ `diagnose=false` 是界面的**快路径**：只画结构，不判接线。
    // 它**不需要** `/object_info`（兆字节级、冷缓存时要几百毫秒），所以这里连取都不取 ——
    // 否则"先秒开一张图、再补体检"就白设计了。判据本身仍只有 `build` 一份。
    let object_info = if input.diagnose {
        crate::workflow::workflow_recipe::object_info_of(&state).await
    } else {
        None
    };
    Ok(axum::Json(build(&raw, object_info.as_deref(), input.diagnose).to_json()))
}

/// 存一份（多半是刚在蓝图上改过的）工作流。
///
/// # 为什么落点是 `<项目根>\comfyui\workflows\` 而不是用户自己的 ComfyUI 目录
///
/// 写入白名单**只有一份**（[`crate::tools::policy::ToolPolicy::resolve_write`]，
/// AGENTS §10）。在这里另开一条"写用户 ComfyUI 目录"的路，等于让"什么能写"出现第二个判据 ——
/// 而第二个判据出问题的表现是**静默写到别人机器上的任意路径**。
/// 存到白名单里那份之后，用户拖进 ComfyUI（或让 AI 用 `comfy_load_workflow` 读回来）都行。
async fn post_save(
    axum::extract::State(state): axum::extract::State<crate::web::routes::AppState>,
    axum::Json(input): axum::Json<SaveInput>,
) -> Result<axum::Json<Value>, crate::web::routes::ApiError> {
    // 1) 必须是合法 JSON —— 存一份坏文件比不存更糟（用户拖进 ComfyUI 才发现）
    let value = parse_raw(&input.raw)?;
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| crate::web::routes::ApiError::internal(format!("序列化失败：{e}")))?;

    // 2) 文件名：**只取最后一段**（用户给的名字里带目录也没用，目录由这里定）
    let name = safe_file_name(input.name.as_deref());
    let policy = crate::tools::policy::ToolPolicy::load(&state)?;
    let target = policy
        .resolve_write(&format!("comfyui/workflows/{name}"))
        // 码原样带出去（`PATH_DENIED` 是用户要按提示去改权限设置的那一档）
        .map_err(|failure| crate::web::routes::ApiError::ai(failure.code, failure.message))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::web::routes::ApiError::internal(format!("建目录失败 {}：{e}", parent.display()))
        })?;
    }
    std::fs::write(&target, text.as_bytes()).map_err(|e| {
        crate::web::routes::ApiError::internal(format!("写文件失败 {}：{e}", target.display()))
    })?;
    Ok(axum::Json(json!({
        "path": target.display().to_string(),
        "bytes": text.len(),
        // 用户下一步要干什么，一次说清（界面直接展示这一句）
        "hint": "把这个文件拖进 ComfyUI 就能打开；也可以让 AI 用 comfy_load_workflow 读回来（会自动转成 API 图）",
    })))
}

/// 解析工作流原文（错误是 400，不是 500 —— 那是调用方给错了）。
fn parse_raw(text: &str) -> Result<Value, crate::web::routes::ApiError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(crate::web::routes::ApiError::bad_request("工作流原文是空的"));
    }
    serde_json::from_str(trimmed)
        .map_err(|e| crate::web::routes::ApiError::bad_request(format!("工作流不是合法 JSON：{e}")))
}

/// 文件名净化：只留最后一段、去掉非法字符、补 `.json`、给个兜底名。
fn safe_file_name(raw: Option<&str>) -> String {
    let fallback = "blueprint-workflow.json".to_string();
    let Some(raw) = raw else { return fallback };
    // 反斜杠与斜杠都算分隔符（Windows 上 `Path::file_name` 对 `/` 也认，但两种都挡更稳）
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        return fallback;
    }
    if cleaned.to_ascii_lowercase().ends_with(".json") {
        cleaned
    } else {
        format!("{cleaned}.json")
    }
}

// ---------------------------------------------------------------------------
// 工具：comfy_diagnose_workflow
// ---------------------------------------------------------------------------

/// `comfy_diagnose_workflow`：接线体检（**只读**）。
///
/// 三种来源任选一个：`path` / `promptId` / `graph` —— 与 `comfy_learn_workflow`
/// 同一条准入（路径过 [`crate::tools::policy::ToolPolicy::resolve_read`]）。
///
/// 与 `comfy_audit_workflow` 的分工写进了工具描述：那条审"跑起来会不会崩"（只认 API 图），
/// 这条审"接线本身"（界面格式也认，能看到广播节点该填却没填的那几路）。
pub async fn tool_diagnose_workflow(
    state: &crate::web::routes::AppState,
    policy: &crate::tools::policy::ToolPolicy,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    use crate::tools::tools::{ToolFailure, ToolOutcome};

    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let prompt_id = arguments.get("promptId").and_then(|value| match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    });
    let graph = arguments.get("graph").and_then(Value::as_object).cloned();

    let raw: Value = match (path, prompt_id, graph) {
        (Some(path), _, _) => {
            let resolved = policy
                .resolve_read(path)
                .map_err(|failure| ToolFailure::new(&failure.code, failure.message))?;
            let text = std::fs::read_to_string(&resolved).map_err(|e| {
                ToolFailure::new("IO_ERROR", format!("读不了文件（{}）：{e}", resolved.display()))
            })?;
            serde_json::from_str(&text).map_err(|e| {
                ToolFailure::new("NOT_A_WORKFLOW", format!("这份文件不是合法 JSON：{e}"))
            })?
        }
        (None, Some(id), _) => {
            let text = crate::comfy::capture::prompt_workflow(state, id)
                .map_err(|e| ToolFailure::new("TOOL_ERROR", format!("读工作流原文出错：{}", e.human())))?
                .ok_or_else(|| {
                    ToolFailure::new(
                        "NO_WORKFLOW",
                        format!("提示词 {id} 没存过工作流（只有捕获到 / 手动读进来的运行才有）"),
                    )
                })?;
            serde_json::from_str(&text).map_err(|e| {
                ToolFailure::new("NOT_A_WORKFLOW", format!("库里这份工作流不是合法 JSON：{e}"))
            })?
        }
        (None, None, Some(graph)) => Value::Object(graph),
        (None, None, None) => {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                "要给我一份工作流：path（文件路径）/ promptId（库里的）/ graph（API 格式节点图）三选一",
            ))
        }
    };

    let object_info = crate::workflow::workflow_recipe::object_info_of(state).await;
    let blueprint = build(&raw, object_info.as_deref(), true);
    let mut text = blueprint.render();
    if let Some(summary) = blueprint.summary() {
        // 报告开头先给一句"结论"，模型不必读完 30 行才知道有没有问题
        text = format!("{summary}\n{text}");
    }
    Ok(ToolOutcome::new(text, blueprint.to_json()))
}

#[cfg(test)]
mod tests;
