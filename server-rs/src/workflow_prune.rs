//! `comfy_prune_workflow`：工作流的**摘节点 / 拼节点**（`WorkflowPrune.kt` +
//! `ToolRegistry.kt` 里那条工具处理体的逐条对照，P4 第 63 步）。
//!
//! # 用户的原话（用户建议 ②，2026-09-20）
//!
//! > "可能要设计工具/skill，让 AI 能够基于基本工作流，摘除无关节点或者拼装必要节点
//! > （比如这种情况：用户没有提供参考图，但是他引导的工作流的节点用到参考图）"
//!
//! 后来用户又补了两个现场：
//! > "我没给参考图，但是给了个需要参考图的工作流，要能识别出该工作流的骨干部分，
//! >  把无关内容临时摘掉；又比如我显存不足但是硬要跑，这时候可能需要考虑向工作流加入
//! >  Sage Attention"
//!
//! # 为什么这件事必须由后端**确定性地**做
//!
//! "摘掉一个分支"不是删几个节点就完事：下游那些指着它的连线要**重新接回去**
//! （`KSampler.positive` 原本接在 ControlNet 上，摘掉 ControlNet 之后得接回文本编码器）。
//! 让模型拿 JSON 自己重写，它得先把整张图读进来、再逐条重写引用 —— 32K 字符的结果上限
//! 装不下一份 100+ 节点的工作流，而且一个键写错 ComfyUI 只会回一句"节点不存在"。
//! 所以这里做的是纯函数式的图运算，结果**自带审计清单**（摘了谁、把哪根线接到了哪），
//! 模型与用户都能核对。
//!
//! # 判据是"**可达性**"，不是"节点名字像不像"
//!
//! 摘除的做法只有一条：从**输出节点**（SaveImage / PreviewImage / SaveVideo…）出发
//! 反向遍历，能走到的才是骨干。剩下的一律是"跟出图无关的分支" —— 不靠类名白名单去猜
//! "这个像不像 ControlNet"，所以第三方自定义节点也照样管用。
//!
//! # 两个动作
//!
//!  - **摘除**：`removeFrom` 点名要删的输入（`89.image`）+ 它们所在的分支；
//!  - **拼装**：`injections` 把"必须补上"的节点插进链里（典型是 Sage Attention /
//!    显存优化节点：插在模型加载器与采样器之间的 `model` 线上，
//!    输入接哪根线由 [`pick_carry_input`] 按 `/object_info` 的定义算，不靠硬编码）。
//!
//! # 与 Kotlin 的三处说明（都不是行为差异）
//!
//! 1. Kotlin 的 `ALWAYS_KEEP_CONDITION` 与 `consumersWithInputs` **声明了却一处都没用到**
//!    （前者是"这些类不算无关分支"的打算，后者是"带输入名的消费者"的打算，两条都被
//!    `apply` 的可达性判据取代了）。Rust 不给死代码开口子（`cargo check` 要零 warning），
//!    所以这两个 private 成员**没有对应物** —— 它们本来就不可达。
//! 2. Kotlin 的 `WorkflowPrune.Result` 在 Rust 侧叫 [`PruneResult`]：`Result` 这个名字
//!    与 `std::result::Result` 撞车，而这个模块里到处都在返回 `Result`。
//!    工具返回的 JSON 键一个都没改。
//! 3. `Report.toJson()` 是**手拼**的（Kotlin `buildJsonObject`）：`if (n.usedBy.isNotEmpty())`
//!    那种"有才写"的键这里也**省掉键**，不是写 `null`。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::routes::AppState;
use crate::tools::{ToolFailure, ToolOutcome};

/// 输出节点：从这些出发做反向可达性分析。
///
/// 判据与 `WorkflowInputs` 那边一致（"Save / Preview / VideoCombine 是结果去哪儿"），
/// 这里是它的对偶用法。
const OUTPUT_HINTS: [&str; 4] = ["save", "preview", "videocombine", "export"];

/// 拼装默认最多报几条分支（Kotlin `unusedBranches(maxBranches = 12)`）。
pub const DEFAULT_MAX_BRANCHES: usize = 12;

/// 一次摘除 / 拼装的结果（Kotlin `WorkflowPrune.Result`）。
///
/// [`graph`](Self::graph) 是**改好的新图**（原图不动，见"写入留退路"那条纪律）；
/// 其余字段是给模型与用户核对用的审计清单。
#[derive(Debug, Clone)]
pub struct PruneResult {
    pub graph: Map<String, Value>,
    /// 摘掉了哪些节点，形如 `89 LoadImage`
    pub removed: Vec<String>,
    /// 被重新接过的线，形如 `3.positive ← 6（原来接在 89 上）`
    pub rewired: Vec<String>,
    /// 插进去的节点，形如 `900 SageAttention（插在 3.model 上）`
    pub injected: Vec<String>,
    /// 摘完之后**还悬着**的输入（这些会让 ComfyUI 报缺输入，必须如实报出来）
    pub unresolved: Vec<String>,
    /// 摘完之后空着的连线型输入（模型可以照它补线）
    pub open_inputs: Vec<String>,
    /// 每个输出节点还连不连得通（`false` = 这份图跑不出东西了）
    pub outputs_alive: usize,
    pub outputs_total: usize,
    /// 干不了 / 不敢干的说明（不静默）
    pub warnings: Vec<String>,
}

impl PruneResult {
    pub fn runnable(&self) -> bool {
        self.unresolved.is_empty() && self.outputs_alive > 0 && self.outputs_alive == self.outputs_total
    }

    pub fn summary(&self) -> String {
        let mut out = format!("摘掉 {} 个节点", self.removed.len());
        if !self.rewired.is_empty() {
            out.push_str(&format!("，重接 {} 根线", self.rewired.len()));
        }
        if !self.injected.is_empty() {
            out.push_str(&format!("，插入 {} 个节点", self.injected.len()));
        }
        out.push_str(&format!(
            "；输出节点 {}/{} 仍然连通",
            self.outputs_alive, self.outputs_total
        ));
        if !self.unresolved.is_empty() {
            out.push_str(&format!("；**还有 {} 处悬空**", self.unresolved.len()));
        }
        if !self.warnings.is_empty() {
            out.push_str(&format!("；{} 条提醒", self.warnings.len()));
        }
        out
    }
}

/// 拼装补丁的声明：插在哪条线、节点定义名、以及它自己的输入从哪来。
#[derive(Debug, Clone)]
pub struct Injection {
    /// 要插进哪个输入：`节点id.输入名`（例如 `3.model`）
    pub at: String,
    /// 节点类名（必须在 `/object_info` 里有定义，或者是个纯字面量节点）
    pub class_type: String,
    /// 这个补丁节点自己的输入：名字 → 值（`"model"` 这类"接上游"的输入自动接）
    pub inputs: Map<String, Value>,
    /// 新节点用什么编号（默认自动取一个没被占用的）
    pub node_id: Option<String>,
}

/// 摘 + 拼。
///
/// * `graph`：API 格式节点图（`{"1":{"class_type":"…","inputs":{…}}}`）。
/// * `remove_from`：要摘掉的输入（`89.image`）或节点 id（`89`）——
///   摘的是**它们所在的那条分支**，不是只摘那一个节点。
/// * `keep`：明确要求保留的节点 id（摘分歧时防止误伤）。
/// * `injections`：要拼进去的节点（典型：Sage Attention）。
/// * `object_info`：ComfyUI 的节点定义表；给了就能算出"补丁节点要接哪些输入"。
///   为 `None` 时只做摘除（拼装需要定义表才不会接错线）。
pub fn apply(
    graph: &Map<String, Value>,
    remove_from: &[String],
    keep: &HashSet<String>,
    injections: &[Injection],
    object_info: Option<&Map<String, Value>>,
) -> PruneResult {
    let mut nodes: Map<String, Value> = Map::new();
    for (id, value) in graph {
        if let Some(object) = value.as_object() {
            nodes.insert(id.clone(), Value::Object(object.clone()));
        }
    }
    if nodes.is_empty() {
        return PruneResult {
            graph: graph.clone(),
            removed: Vec::new(),
            rewired: Vec::new(),
            injected: Vec::new(),
            unresolved: Vec::new(),
            open_inputs: Vec::new(),
            outputs_alive: 0,
            outputs_total: 0,
            warnings: vec!["这张图里一个节点都没有（不是 API 格式节点图？）".to_string()],
        };
    }

    let mut warnings: Vec<String> = Vec::new();
    let outputs: Vec<String> =
        nodes.keys().filter(|id| is_output(id, &nodes)).cloned().collect();

    // ---- 1) 摘除 ------------------------------------------------------
    //
    // 用户点的那个位置**一定**要摘掉，摘完之后：
    //   ① 它自己的连线输入清空（= 这条线不再存在）；
    //   ② 由此"没人再要"的上游跟着级联摘掉（`LoadImage` / `ControlNetLoader`
    //      只喂给 `ControlNetApply`，ControlNetApply 一走它们就再也不会被执行）；
    //   ③ 它下游那些**还有消费者的**人不摘，而是把指着它的线**接回它自己的上游**
    //      （`KSampler.positive` 就是这么回到文本编码器上的）。
    let mut dropped_order: Vec<String> = Vec::new();
    let mut dropped: HashSet<String> = HashSet::new();
    let mut targets: HashSet<String> = HashSet::new();
    for raw in remove_from {
        let target = raw.trim();
        if target.is_empty() {
            continue;
        }
        let node_id = match target.find('.') {
            Some(dot) => &target[..dot],
            None => target,
        };
        if !nodes.contains_key(node_id) {
            let existing: Vec<&str> = nodes.keys().take(20).map(String::as_str).collect();
            warnings.push(format!(
                "要摘的节点 {node_id} 不在这张图里（现有节点 {}…）",
                existing.join("、")
            ));
            continue;
        }
        if outputs.iter().any(|id| id == node_id) {
            warnings.push(format!(
                "节点 {node_id}（{}）是**输出节点**，摘了什么图都出不来 —— 已保留",
                class_of(node_at(&nodes, node_id))
            ));
            continue;
        }
        if keep.contains(node_id) {
            warnings.push(format!(
                "节点 {node_id}（{}）在 keep 里，已保留",
                class_of(node_at(&nodes, node_id))
            ));
            continue;
        }
        targets.insert(node_id.to_string());
        if dropped.insert(node_id.to_string()) {
            dropped_order.push(node_id.to_string());
        }
    }

    // 级联：把"**再也不会被执行**"的节点收掉。
    //
    // 判据只有一条，好解释也好验证：**从输出节点反向再也走不到它**。
    //
    // 两处例外必须留着，否则会变成"顺手删用户的图"：
    //   · **输出节点**永远保留；
    //   · **没有任何连线输入、也没有消费者**的孤立节点不碰 ——
    //     那是用户"还没接完"的东西（比如刚拖进来的 EmptyLatentImage），
    //     不是我们要摘的那条无关分支。第一版就是把它一起删了。
    let mut cascading = true;
    while cascading {
        cascading = false;
        // Kotlin 的 `nodes.filterKeys { … }` 是**即时**过滤（LinkedHashMap 副本），
        // 所以 `live` 在每一轮 while 开头算一次，during 内层循环不会跟着变 —— 照抄。
        let mut remaining: Map<String, Value> = Map::new();
        for (id, node) in &nodes {
            if !dropped.contains(id) {
                remaining.insert(id.clone(), node.clone());
            }
        }
        let live = live_nodes(&remaining, &outputs);
        let ids: Vec<String> = nodes.keys().cloned().collect();
        for id in ids {
            if dropped.contains(&id)
                || keep.contains(&id)
                || outputs.iter().any(|output| *output == id)
                || live.contains(&id)
            {
                continue;
            }
            let node = node_at(&nodes, &id);
            let wired = node
                .get("inputs")
                .and_then(Value::as_object)
                .map(|inputs| inputs.values().any(|v| ref_of(Some(v)).is_some()))
                .unwrap_or(false);
            let has_consumers = !consumers_of(&id, &nodes, &dropped).is_empty();
            if !wired && !has_consumers {
                continue; // 孤立节点：不碰
            }
            if dropped.insert(id.clone()) {
                dropped_order.push(id);
            }
            cascading = true;
        }
    }

    // ---- 2) 收拾残局：指着被摘节点的输入，接回它自己的上游 --------------
    //
    // 这一段是"用户还看得见结果"的关键：
    //  · 目标本身的输入一律清掉（`KSampler.positive` 不再接 ControlNet）；
    //  · **只是**把目标当上游、且还有别的连线输入的人，改接回目标的上游
    //    （`ControlNetApply` 摘下之后，它下游那条链不至于断在半空）。
    let mut rewired: Vec<String> = Vec::new();
    let mut patched: Map<String, Value> = Map::new();
    for (id, value) in &nodes {
        if dropped.contains(id) {
            continue;
        }
        let node = value.as_object().cloned().unwrap_or_default();
        let mut inputs: Map<String, Value> =
            node.get("inputs").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut changed = false;
        let names: Vec<String> = inputs.keys().cloned().collect();
        for name in names {
            let Some(reference) = ref_of(inputs.get(&name)) else { continue };
            if !dropped.contains(&reference.0) {
                continue;
            }
            if targets.contains(id) {
                // 目标自己的输入：清掉（这就是"把这条线拿走"）
                inputs.remove(&name);
                changed = true;
                continue;
            }
            // 别人指着被摘节点：接回它的上游（找不到就置空，并如实报出来）
            match nearest_live_input(&reference.0, &nodes, &dropped) {
                None => {
                    inputs.remove(&name);
                    warnings.push(format!(
                        "{id}.{name} 原本接在被摘掉的 {} 上，而 {} 也没有可用的上游 → 这个输入现在空着",
                        reference.0, reference.0
                    ));
                }
                Some(back) => {
                    inputs.insert(name.clone(), json!([back.0, back.1]));
                    rewired.push(format!(
                        "{id}.{name} ← {}（原来接在被摘掉的 {} 上）",
                        back.0, reference.0
                    ));
                }
            }
            changed = true;
        }
        if changed {
            let mut patched_node = node;
            patched_node.insert("inputs".into(), Value::Object(inputs));
            patched.insert(id.clone(), Value::Object(patched_node));
        } else {
            patched.insert(id.clone(), Value::Object(node));
        }
    }

    // ---- 3) 拼装：插补丁节点 ------------------------------------------
    let mut injected: Vec<String> = Vec::new();
    let mut next_id: i64 =
        patched.keys().filter_map(|id| id.parse::<i64>().ok()).max().unwrap_or(0) + 100;
    for inj in injections {
        let Some((host_id, host_input)) = split_path(&inj.at) else {
            warnings.push(format!("拼装位置要写成 节点id.输入名（收到的是「{}」）", inj.at));
            continue;
        };
        let Some(host_value) = patched.get(&host_id) else {
            warnings.push(format!("拼装位置 {host_id} 不在这张图里（可能刚好被摘掉了）"));
            continue;
        };
        let host = host_value.as_object().cloned().unwrap_or_default();
        let mut host_inputs: Map<String, Value> =
            host.get("inputs").and_then(Value::as_object).cloned().unwrap_or_default();
        let upstream = host_inputs.get(&host_input).cloned();
        if ref_of(upstream.as_ref()).is_none() {
            warnings.push(format!(
                "{host_id}.{host_input} 不是一条线（现在值：{}），没法把 {} 插在它中间",
                upstream
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "空".to_string()),
                inj.class_type
            ));
            continue;
        }
        if let Some(info) = object_info {
            if !info.contains_key(&inj.class_type) {
                warnings.push(format!(
                    "这台 ComfyUI 里没有节点类 {}（/object_info 查不到）——没有插入",
                    inj.class_type
                ));
                continue;
            }
        }
        let new_id = match inj
            .node_id
            .as_ref()
            .filter(|id| !id.trim().is_empty() && !patched.contains_key(*id))
        {
            Some(given) => given.clone(),
            None => loop {
                let candidate = next_id.to_string();
                next_id += 1;
                if !patched.contains_key(&candidate) {
                    break candidate;
                }
            },
        };

        let mut inputs: Map<String, Value> = Map::new();
        // 补丁自己的输入：显式给的先填
        for (key, value) in &inj.inputs {
            inputs.insert(key.clone(), value.clone());
        }
        // 然后把它自己那条"接上游"的线接好：找出它声明的、类型与上游相同的那个输入
        let patch_spec = object_info
            .and_then(|info| info.get(&inj.class_type))
            .and_then(Value::as_object);
        let Some(carry_over) = pick_carry_input(patch_spec, &inputs) else {
            warnings.push(format!(
                "不敢给 {} 自动接线：从定义里看不出哪个输入该接上游，插入位置保持原样。\
                 可以把它的输入用 injections[].inputs 显式写清楚。",
                inj.class_type
            ));
            continue;
        };
        if let Some(upstream) = upstream {
            inputs.insert(carry_over, upstream);
        }
        host_inputs.insert(host_input.clone(), json!([new_id, 0]));
        let mut patched_host = host;
        patched_host.insert("inputs".into(), Value::Object(host_inputs));
        patched.insert(host_id.clone(), Value::Object(patched_host));
        let mut new_node = Map::new();
        new_node.insert("class_type".into(), json!(inj.class_type));
        new_node.insert("inputs".into(), Value::Object(inputs));
        new_node.insert(
            "_meta".into(),
            json!({ "title": format!("{}（自动插入）", inj.class_type) }),
        );
        patched.insert(new_id.clone(), Value::Object(new_node));
        injected.push(format!(
            "{new_id} {}（插在 {host_id}.{host_input} 上）",
            inj.class_type
        ));
        next_id = new_id.parse::<i64>().unwrap_or(next_id) + 1;
    }

    // ---- 4) 体检：悬空引用 / 空着的连线型输入 / 输出还通不通 ----------
    let unresolved = dangling(&patched);
    let open_inputs = open_wired_inputs(&patched, object_info);
    let outputs_alive = outputs.iter().filter(|id| patched.contains_key(*id)).count();

    let removed = dropped_order
        .iter()
        .map(|id| format!("{id} {}", nodes.get(id).map(class_of).unwrap_or_default()))
        .collect::<Vec<_>>();
    let mut removed = removed;
    removed.sort();

    PruneResult {
        graph: patched,
        removed,
        rewired,
        injected,
        unresolved,
        open_inputs,
        outputs_alive,
        outputs_total: outputs.len(),
        warnings,
    }
}

/// 一份"这段是干什么的"人话描述（用户建议 ④：分析出本地**用不上**的节点，
/// 向用户确认后再删）。
///
/// 判据是几条最常见的类名特征，**认不出就照抄类名** —— 编一个听起来很懂的解释
/// 比不解释更坏（用户会照着它做决定）。这一段只影响文案，不影响摘除判据。
pub fn describe(class_type: &str) -> String {
    let lower = class_type.to_lowercase();
    let known = if lower.contains("controlnet") {
        Some("ControlNet（结构控制）")
    } else if lower.contains("ipadapter") {
        Some("IPAdapter（参考图/风格）")
    } else if lower.contains("loadimage") {
        Some("LoadImage（参考图文件）")
    } else if lower.contains("loadvideo") || lower.contains("vhs_loadvideo") {
        Some("LoadVideo（参考视频）")
    } else if lower.contains("loadaudio") {
        Some("LoadAudio（参考音频）")
    } else if lower.contains("loraloader") || lower.contains("lora_tag") {
        Some("LoRA 加载")
    } else if lower.contains("vae") {
        Some("VAE 加载/解码")
    } else if lower.contains("clip") {
        Some("文本编码器加载")
    } else if lower.contains("unetloader")
        || lower.contains("checkpointloader")
        || lower.contains("diffusionmodelloader")
    {
        Some("模型加载")
    } else if lower.contains("upscale") || lower.contains("imagescale") {
        Some("放大 / 缩放")
    } else if lower.contains("faceswap") || lower.contains("facerestore") {
        Some("换脸 / 修脸")
    } else if lower.contains("detailer") || lower.contains("adetailer") {
        Some("细节修复（Detailer）")
    } else if lower.contains("segment") || lower.contains("mask") {
        Some("遮罩 / 分割")
    } else if lower.contains("pose") || lower.contains("openpose") {
        Some("姿态提取")
    } else if lower.contains("depth") {
        Some("深度提取")
    } else if lower.contains("label") {
        Some("Label（纯界面标注，不参与出图）")
    } else if lower.contains("note") || lower.contains("bookmark") {
        Some("笔记 / 书签（纯界面元素）")
    } else if lower.contains("reroute") {
        Some("Reroute（纯转接线）")
    } else {
        // 认不出来就照抄类名，别编
        None
    };
    known.map(str::to_string).unwrap_or_else(|| class_type.to_string())
}

/// **用不上的分支**分析（用户建议 ④，2026-09-22）。
///
/// > "设计工具，当用户提出类似'删除无用节点'时，分析出本地用不上的节点并向用户再次确认删除，
/// >  然后执行操作。"
///
/// 判据与摘除完全同源：**从输出节点反向走不到** = 跟出图无关。不去猜"这个类像不像
/// ControlNet"（那样第三方节点就漏了），所以结果和真摘一次是一致的。
///
/// * `keep`：用户点名要保留的节点（不会进建议）。
/// * `max_branches`：最多报几条分支（一次报 40 段没人看得完）。
pub fn unused_branches(
    graph: &Map<String, Value>,
    keep: &HashSet<String>,
    max_branches: usize,
) -> PruneReport {
    let mut nodes: Map<String, Value> = Map::new();
    for (id, value) in graph {
        if let Some(object) = value.as_object() {
            nodes.insert(id.clone(), Value::Object(object.clone()));
        }
    }
    let outputs: Vec<String> =
        nodes.keys().filter(|id| is_output(id, &nodes)).cloned().collect();
    let live = live_nodes(&nodes, &outputs);

    // "还没接完"的孤立节点：**自己没接任何线，也没人用它**。
    //
    // ⚠ 两个条件缺一不可（写第一版时只判了"没接线"，于是把 `FaceSwap.face` 指着的
    // `LoadImage` 也算成了孤立 —— 它是这条死分支的上游，摘的时候要一起走）。
    // 判据与 `apply` 级联摘除里那条"孤立节点不碰"逐字一致。
    let isolated = |id: &str| -> bool {
        if live.contains(id) {
            return false;
        }
        if !consumers_of(id, &nodes, &HashSet::new()).is_empty() {
            return false;
        }
        match nodes.get(id).and_then(|node| node.get("inputs")).and_then(Value::as_object) {
            Some(inputs) => inputs.values().all(|value| ref_of(Some(value)).is_none()),
            None => true,
        }
    };

    let orphan_ids: Vec<String> = nodes
        .keys()
        .filter(|id| !live.contains(*id) && isolated(id.as_str()))
        .map(|id| (*id).clone())
        .collect();
    let orphan_set: HashSet<String> = orphan_ids.iter().cloned().collect();
    let dead_ids: Vec<String> = nodes
        .keys()
        .filter(|id| {
            !live.contains(*id)
                && !orphan_set.contains(*id)
                && !keep.contains(*id)
                && !outputs.iter().any(|output| output == *id)
        })
        .map(|id| (*id).clone())
        .collect();
    let dead_set: HashSet<String> = dead_ids.iter().cloned().collect();

    let mut raw: Vec<Candidate> = Vec::with_capacity(dead_ids.len());
    for id in &dead_ids {
        let mut used_by = consumers_of(id, &nodes, &HashSet::new());
        used_by.sort();
        raw.push(Candidate {
            id: id.clone(),
            cls: nodes.get(id).map(class_of).unwrap_or_default(),
            used_by,
        });
    }

    // 连成一片的算一条分支 —— 但"连成一片"必须**按数据流方向**连，不是无向连通：
    // 一条分支 = 一个末端（谁也不引用的死节点）＋ 喂给它的所有上游。
    // 用户在图里点那个末端，整条分支就跟着被摘掉，这正是 `apply` 的级联判据。
    //
    // （第一版按无向连通分组，把 `FaceSwap.face ← LoadImage` 这种"横向喂料"也连进
    // 同一片，于是一片里出现两个互不喂料的梢，入口就选到了错误的那个。）
    let mut by_tip: Vec<(String, Vec<Candidate>)> = Vec::new();
    for candidate in raw {
        let tip = downstream_tip(&candidate.id, &nodes, &dead_set);
        match by_tip.iter_mut().find(|(key, _)| *key == tip) {
            Some((_, group)) => group.push(candidate),
            None => by_tip.push((tip, vec![candidate])),
        }
    }
    let mut branches: Vec<Branch> = Vec::new();
    for (tip_id, group) in by_tip {
        // `firstOrNull` → `maxByOrNull`（并列取**先出现**的）→ `first()`
        let entry = group
            .iter()
            .find(|candidate| candidate.id == tip_id)
            .or_else(|| {
                let mut best: Option<&Candidate> = None;
                let mut best_count = 0usize;
                for candidate in &group {
                    let count = collect(&candidate.id, &nodes)
                        .iter()
                        .filter(|id| dead_set.contains(*id))
                        .count();
                    if best.is_none() || count > best_count {
                        best = Some(candidate);
                        best_count = count;
                    }
                }
                best
            })
            .or_else(|| group.first())
            .cloned()
            .expect("group 不可能为空");
        let mut classes: Vec<String> = Vec::new();
        for candidate in &group {
            if !classes.contains(&candidate.cls) {
                classes.push(candidate.cls.clone());
            }
        }
        let mut kinds: Vec<String> = Vec::new();
        for class in &classes {
            let label = describe(class);
            if !kinds.contains(&label) {
                kinds.push(label);
            }
        }
        kinds.truncate(6);
        branches.push(Branch {
            index: branches.len() + 1,
            entry: entry.id.clone(),
            entry_cls: entry.cls.clone(),
            label: describe(&entry.cls),
            nodes: group,
            kinds,
        });
    }

    // `compareByDescending { nodes.size }.thenBy { entry }`（Kotlin 的 sortedWith 是稳定排序）
    branches.sort_by(|a, b| {
        b.nodes
            .len()
            .cmp(&a.nodes.len())
            .then_with(|| entry_key(&a.entry).cmp(&entry_key(&b.entry)))
    });
    let mut sorted: Vec<Branch> = branches
        .into_iter()
        .enumerate()
        .map(|(index, mut branch)| {
            branch.index = index + 1;
            branch
        })
        .collect();
    sorted.truncate(max_branches);

    let shown: usize = sorted.iter().map(|branch| branch.nodes.len()).sum();
    let mut orphan_ids = orphan_ids;
    orphan_ids.sort_by_key(|id| entry_key(id));
    let isolated_nodes: Vec<Candidate> = orphan_ids
        .iter()
        .map(|id| Candidate {
            id: id.clone(),
            cls: nodes.get(id).map(class_of).unwrap_or_default(),
            used_by: Vec::new(),
        })
        .collect();

    PruneReport {
        branches: sorted,
        isolated: isolated_nodes,
        total_unused: dead_ids.len(),
        total_isolated: orphan_ids.len(),
        total_nodes: nodes.len(),
        output_count: outputs.len(),
        truncated: dead_ids.len() > shown,
    }
}

/// 一条**用不上的分支**（连成一片、可以整条摘掉）。
#[derive(Debug, Clone)]
pub struct Branch {
    pub index: usize,
    /// 建议的入口节点 id（`removeFrom` 就写它）
    pub entry: String,
    pub entry_cls: String,
    /// 这条分支在做什么（人话）
    pub label: String,
    pub nodes: Vec<Candidate>,
    /// 组内出现过的节点种类（人话，最多 6 条）
    pub kinds: Vec<String>,
}

impl Branch {
    /// 这条分支要摘的节点 id。
    ///
    /// **整条都给**（不是只给 entry）：`removeFrom` 只写入口时，`apply` 会按"从输出反向
    /// 再也走不到"级联摘掉同一片 —— 结果是一样的，但显式列全了，用户核对时不会漏。
    pub fn suggest_remove(&self) -> Vec<String> {
        self.nodes.iter().map(|node| node.id.clone()).collect()
    }
}

/// 一个候选节点。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub cls: String,
    /// 图里还有谁在引用它（空 = 这条分支的末端）
    pub used_by: Vec<String>,
}

/// 分析结果（只报不改）。
#[derive(Debug, Clone)]
pub struct PruneReport {
    pub branches: Vec<Branch>,
    /// 未接线（还没接完）的孤立节点 —— 明确**不建议**删
    pub isolated: Vec<Candidate>,
    pub total_unused: usize,
    pub total_isolated: usize,
    pub total_nodes: usize,
    pub output_count: usize,
    /// 超过 [`DEFAULT_MAX_BRANCHES`]，还有没列出来的
    pub truncated: bool,
}

impl PruneReport {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("ok".into(), json!(true));
        out.insert(
            "summary".into(),
            json!(if self.total_unused == 0 {
                format!(
                    "这份工作流没有用不上的分支：{} 个节点全都在出图链路上。",
                    self.total_nodes
                )
            } else {
                format!(
                    "这份工作流共 {} 个节点，其中 **{} 个** 从任何一个输出节点（共 {} 个）\
                     反向都走不到 —— 也就是跟出图无关，可以摘掉（分成 {} 条分支）。",
                    self.total_nodes,
                    self.total_unused,
                    self.output_count,
                    self.branches.len()
                )
            }),
        );
        out.insert("totalNodes".into(), json!(self.total_nodes));
        out.insert("totalUnused".into(), json!(self.total_unused));
        out.insert("outputCount".into(), json!(self.output_count));
        out.insert("truncated".into(), json!(self.truncated));
        let mut branches: Vec<Value> = Vec::with_capacity(self.branches.len());
        for branch in &self.branches {
            let mut o = Map::new();
            o.insert("index".into(), json!(branch.index));
            o.insert("removeFrom".into(), json!(branch.entry));
            o.insert("label".into(), json!(branch.label));
            o.insert("classType".into(), json!(branch.entry_cls));
            o.insert("nodeCount".into(), json!(branch.nodes.len()));
            let mut nodes: Vec<Value> = Vec::with_capacity(branch.nodes.len());
            for node in &branch.nodes {
                let mut n = Map::new();
                n.insert("id".into(), json!(node.id));
                n.insert("classType".into(), json!(node.cls));
                n.insert("label".into(), json!(describe(&node.cls)));
                // `if (n.usedBy.isNotEmpty())`：省掉键，不是写 null
                if !node.used_by.is_empty() {
                    n.insert("usedBy".into(), json!(node.used_by));
                }
                nodes.push(Value::Object(n));
            }
            o.insert("nodes".into(), Value::Array(nodes));
            o.insert("kinds".into(), json!(branch.kinds));
            branches.push(Value::Object(o));
        }
        out.insert("branches".into(), Value::Array(branches));
        if !self.isolated.is_empty() {
            let mut isolated: Vec<Value> = Vec::with_capacity(self.isolated.len());
            for node in &self.isolated {
                isolated.push(json!({
                    "id": node.id,
                    "classType": node.cls,
                    "label": describe(&node.cls),
                }));
            }
            out.insert("isolated".into(), Value::Array(isolated));
        }
        out.insert(
            "hint".into(),
            json!(if self.total_unused == 0 {
                "没有可摘的分支。别为了'看起来干净'去动还在链路里的节点。"
            } else {
                "**先问用户再动手**（`ask_user_question`，选项用每条的 label）：\
                 把上面每条的 removeFrom / label / 节点数说清楚，用户确认哪几条，\
                 再用 `removeFrom=[那些 entry]` 调一次本工具（这次不带 analyze）真正执行。\
                 `isolated` 里的是**还没接完**的节点，不要删。"
            }),
        );
        Value::Object(out)
    }
}

// ---------------------------------------------------------------------------
//  图运算（全是纯函数）
// ---------------------------------------------------------------------------

/// [id] 这条死数据流最终喂给谁（**顺流而下**找那个谁也不引用的死节点）。
///
/// 它就是"这条分支的梢"：用户在图里点它，`apply` 的级联会把整条一起摘掉。
/// 多个梢都能到（分叉到两个末端）时取编号大的那个，保证同一份图每次报同一条。
fn downstream_tip(id: &str, nodes: &Map<String, Value>, dead: &HashSet<String>) -> String {
    let mut cursor = id.to_string();
    let mut best = id.to_string();
    let mut seen: HashSet<String> = HashSet::new();
    let mut guard = 0;
    while guard < 256 && seen.insert(cursor.clone()) {
        guard += 1;
        best = cursor.clone();
        let next = consumers_of(&cursor, nodes, &HashSet::new())
            .into_iter()
            .filter(|consumer| dead.contains(consumer))
            .max_by_key(|consumer| consumer.parse::<i64>().unwrap_or(i64::MIN));
        match next {
            Some(next) => cursor = next,
            None => break,
        }
    }
    best
}

/// 谁在用它（哪些节点把它当上游）；[ignoring] 里的节点不算（它们已经被决定摘掉了）。
fn consumers_of(
    node_id: &str,
    nodes: &Map<String, Value>,
    ignoring: &HashSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (id, node) in nodes {
        if id == node_id || ignoring.contains(id) {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for value in inputs.values() {
            if ref_of(Some(value)).map(|(source, _)| source == node_id).unwrap_or(false)
                && !out.contains(id)
            {
                out.push(id.clone());
            }
        }
    }
    out
}

/// 从所有输出节点反向能走到的节点集合（= 出图用得上的骨干）。
fn live_nodes(nodes: &Map<String, Value>, outputs: &[String]) -> HashSet<String> {
    let mut live: HashSet<String> = HashSet::new();
    for id in outputs {
        if nodes.contains_key(id) {
            live.extend(collect(id, nodes));
        }
    }
    live
}

/// 从某个输入出发，往回找第一个"没被摘掉"的上游 `(节点id, 槽位)`。
fn nearest_live_input(
    node_id: &str,
    nodes: &Map<String, Value>,
    dropped: &HashSet<String>,
) -> Option<(String, i64)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut cursor: Option<String> = Some(node_id.to_string());
    let mut guard = 0;
    while let Some(current) = cursor.clone() {
        guard += 1;
        if guard > 64 {
            break;
        }
        if !seen.insert(current.clone()) {
            return None;
        }
        let node = nodes.get(&current)?;
        if let Some(inputs) = node.get("inputs").and_then(Value::as_object) {
            // 按声明顺序找第一个还在的连线上游
            for value in inputs.values() {
                if let Some(reference) = ref_of(Some(value)) {
                    if !dropped.contains(&reference.0) && nodes.contains_key(&reference.0) {
                        return Some(reference);
                    }
                }
            }
        }
        // 这一层没有可用的：继续往回（它的上游本身也被摘了）
        cursor = inputs_first_ref(node).filter(|id| nodes.contains_key(id) && !dropped.contains(id));
    }
    None
}

/// 节点第一个连线型输入的**上游节点 id**（Kotlin `firstNotNullOfOrNull`）。
fn inputs_first_ref(node: &Value) -> Option<String> {
    let inputs = node.get("inputs").and_then(Value::as_object)?;
    inputs.values().find_map(|value| ref_of(Some(value)).map(|(source, _)| source))
}

/// 一个输入算不算"接在某个节点上"（值是 `["id", slot]`）。
fn ref_of(value: Option<&Value>) -> Option<(String, i64)> {
    let items = value?.as_array()?;
    if items.len() < 2 {
        return None;
    }
    let id = primitive_text(&items[0])?;
    // Kotlin 的 `contentOrNull?.toIntOrNull() ?: 0`：槽位认不出来就是 0（不是丢弃整条引用）
    let slot = primitive_text(&items[1]).and_then(|text| text.parse::<i64>().ok()).unwrap_or(0);
    Some((id, slot))
}

fn split_path(path: &str) -> Option<(String, String)> {
    let dot = path.rfind('.')?;
    if dot == 0 || dot >= path.len() - 1 {
        return None;
    }
    Some((path[..dot].to_string(), path[dot + 1..].to_string()))
}

/// 输出节点：类名带 Save / Preview / VideoCombine / Export。
fn is_output(id: &str, nodes: &Map<String, Value>) -> bool {
    let Some(node) = nodes.get(id) else { return false };
    let class = class_of(node).to_lowercase();
    // 静音的输出节点不算（它自己不执行）
    let muted = node.get("mode").and_then(primitive_text).as_deref() == Some("2");
    !muted && OUTPUT_HINTS.iter().any(|hint| class.contains(hint))
}

/// [from] 出发顺着引用能走到的所有节点（含自己）。
fn collect(from: &str, nodes: &Map<String, Value>) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = vec![from.to_string()];
    while let Some(id) = stack.pop() {
        if !out.insert(id.clone()) {
            continue;
        }
        let Some(node) = nodes.get(&id) else { continue };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for value in inputs.values() {
            if let Some((source, _)) = ref_of(Some(value)) {
                if nodes.contains_key(&source) {
                    stack.push(source);
                }
            }
        }
    }
    out
}

/// 图里还有没有"指向不存在节点"的输入。
fn dangling(nodes: &Map<String, Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (id, node) in nodes {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            let Some((source, _)) = ref_of(Some(value)) else { continue };
            if !nodes.contains_key(&source) {
                out.push(format!("{id}.{name} ← 节点 {source}（已不在图里）"));
            }
        }
    }
    out
}

/// 摘完之后**空着的连线型输入**（有价值的作业本：模型照着它补线）。
///
/// 只看那些"本来就该有连线"的输入：没有 `object_info` 时退化成"值为空 / 空字符串"。
fn open_wired_inputs(
    nodes: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (id, node) in nodes {
        let class = class_of(node);
        // ⚠ 与 Kotlin 同：节点**连 `inputs` 键都没有**时整条跳过（含下面"定义里声明为插口
        //   但图里没有"那一段）—— 它只影响"少报"，不影响任何摘除判据，所以照抄不改。
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            let empty = value.is_null()
                || matches!(value, Value::String(text) if text.trim().is_empty());
            if !empty {
                continue;
            }
            out.push(format!("{id}.{name}（{class}）"));
        }
        let Some(object_info) = object_info else { continue };
        // 定义里声明为插口、但图里没有的输入也算缺口
        let Some(spec) = object_info.get(&class).and_then(Value::as_object) else { continue };
        let Some(required) = spec
            .get("input")
            .and_then(Value::as_object)
            .and_then(|input| input.get("required"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (name, raw) in required {
            if inputs.contains_key(name) {
                continue;
            }
            let Some(decl) = raw.as_array() else { continue };
            let Some(kind) = decl.first().and_then(primitive_text) else { continue };
            if !is_wired_type(&kind, decl) {
                continue;
            }
            out.push(format!("{id}.{name}（{class}，缺 {kind}）"));
        }
    }
    out
}

/// `/object_info` 里的一条输入声明算不算"连线插口"（控件型不算缺口）。
fn is_wired_type(kind: &str, decl: &[Value]) -> bool {
    if let Some(options) = decl.get(1).and_then(Value::as_object) {
        // Kotlin 是 `contentOrNull == "true"`（JSON 布尔 true 的 content 也是 "true"）
        if options.get("forceInput").and_then(primitive_text).as_deref() == Some("true") {
            return true;
        }
    }
    !matches!(kind.to_uppercase().as_str(), "INT" | "FLOAT" | "STRING" | "BOOLEAN" | "COMBO")
}

/// 补丁节点里"该接上游"的那个输入名。
///
/// 优先找 `model`（Sage Attention / 各种 patch 节点都是这个约定），
/// 其次找定义里第一个连线型的必需输入。找不到就返回 `None`（宁可不插，也不乱接）。
///
/// ⚠ Kotlin 那个版本还有一个 `classType` 参数，但函数体里**一个字都没用到** ——
/// Rust 不给未使用参数开口子（`cargo check` 要零 warning），所以这里不收它。
fn pick_carry_input(
    spec: Option<&Map<String, Value>>,
    already: &Map<String, Value>,
) -> Option<String> {
    if !already.contains_key("model") && (spec.is_none() || spec.and_then(|s| s.get("input")).is_some())
    {
        let required = spec
            .and_then(|s| s.get("input"))
            .and_then(Value::as_object)
            .and_then(|input| input.get("required"))
            .and_then(Value::as_object);
        if required.map(|r| r.contains_key("model")).unwrap_or(false) {
            return Some("model".to_string());
        }
        if spec.is_none() && !already.contains_key("model") {
            return Some("model".to_string());
        }
    }
    let spec = spec?;
    let required = spec
        .get("input")?
        .as_object()?
        .get("required")?
        .as_object()?;
    for (name, raw) in required {
        if already.contains_key(name) {
            continue;
        }
        let Some(decl) = raw.as_array() else { continue };
        let Some(kind) = decl.first().and_then(primitive_text) else { continue };
        if is_wired_type(&kind, decl) {
            return Some(name.clone());
        }
    }
    None
}

/// 节点的类名（API 格式里是 `class_type`）。
fn class_of(node: &Value) -> String {
    node.get("class_type").and_then(primitive_text).unwrap_or_default()
}

/// 取一个**调用前刚确认过存在**的节点（Kotlin 的 `nodes[id]` / `nodes[id]?.cls()`）。
fn node_at<'a>(nodes: &'a Map<String, Value>, id: &str) -> &'a Value {
    nodes.get(id).expect("调用前刚确认过这个节点在图里")
}

/// Kotlin 的 `contentOrNull`（数字给字面量、布尔给 `true` / `false`、null 给 `None`）。
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// 排序用的"节点编号"（非数字排最后，Kotlin `toLongOrNull() ?: Long.MAX_VALUE`）。
fn entry_key(id: &str) -> i64 {
    id.parse::<i64>().unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// Kotlin `JsonObject.bool`：只认严格的 `"true"` / `"false"`（`toBooleanStrictOrNull`）。
fn bool_arg(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text == "true" => Some(true),
        Value::String(text) if text == "false" => Some(false),
        _ => None,
    }
}

/// Kotlin `JsonObject.str`：非空白的文本。
fn obj_str(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(primitive_text).filter(|text| !text.trim().is_empty())
}

/// `comfy_prune_workflow`：给一份工作流做减法（摘节点）/ 加法（拼节点）。
///
/// 三条纪律都在返回里体现（与 Kotlin 逐字一致）：
///  1. **只接受"要摘哪个输入 / 要插什么节点"**，图由后端算 —— 不让模型重写 JSON；
///  2. `analyze=true` **一个字都不改**（只分析哪条分支用不上，拿到清单先问用户）；
///  3. 拿不到 `/object_info` 时**只摘不拼**，并把"为什么没插"如实说清楚 ——
///     乱接一根线的代价是 ComfyUI 回一句看不懂的报错，用户根本查不出来。
pub async fn tool_prune_workflow(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let graph = crate::workflow_fit::api_graph_of(arguments.get("graph")).ok_or_else(|| {
        ToolFailure::new(
            "INVALID_ARGUMENT",
            "graph 不是一份 API 格式节点图（应当形如 \
             {\"1\":{\"class_type\":\"KSampler\",\"inputs\":{…}}}）。\
             界面格式（nodes/links）请先用 comfy_load_workflow 读进库。",
        )
    })?;
    let remove_from: Vec<String> = arguments
        .get("removeFrom")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(primitive_text).collect())
        .unwrap_or_default();
    let keep: HashSet<String> = arguments
        .get("keep")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(primitive_text).collect())
        .unwrap_or_default();
    let injections: Vec<Injection> = arguments
        .get("injections")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|raw| {
                    let object = raw.as_object()?;
                    let at = obj_str(object, "at")?;
                    let class_type = obj_str(object, "classType")?;
                    Some(Injection {
                        at,
                        class_type,
                        inputs: object
                            .get("inputs")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                        node_id: obj_str(object, "nodeId"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if remove_from.is_empty() && injections.is_empty() {
        // `analyze=true`（用户建议 ④）：只分析"哪些分支用不上"，一个字都不改。
        // 用户说"删掉没用的节点"时走的就是这一条 —— 拿到清单**先问他**，再摘。
        if bool_arg(arguments.get("analyze")) == Some(true) {
            let report = unused_branches(&graph, &keep, DEFAULT_MAX_BRANCHES);
            let value = report.to_json();
            return Ok(ToolOutcome::new(value.to_string(), value));
        }
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            "至少给一样：removeFrom（摘哪些）或 injections（插什么）——\
             什么都没要求的话，这份图原样就是你要的。\
             想知道哪些分支用不上，先带 `analyze=true` 调一次（只分析、不改图）。",
        ));
    }

    // 节点定义表：只有"要插节点"时才需要（摘除是纯图运算）
    let mut object_info: Option<Map<String, Value>> = None;
    if !injections.is_empty() {
        let config = crate::capture::capture_config(state).ok();
        let base = config
            .as_ref()
            .map(|c| c.comfy_url.trim().trim_end_matches('/').to_string())
            .unwrap_or_default();
        object_info = if base.is_empty() {
            None
        } else {
            crate::comfy_client::get_json(&format!("{base}/object_info"))
                .await
                .ok()
                .and_then(|v| v.as_object().cloned())
        };
        if object_info.is_none() {
            // 摘除照样做完（那是纯图运算），只是**不插** —— 乱接一根线的代价
            // 是 ComfyUI 回一句看不懂的报错，用户根本查不出来。
            let detached = apply(&graph, &remove_from, &keep, &[], None);
            let mut out = Map::new();
            out.insert("ok".into(), json!(false));
            out.insert("reason".into(), json!("COMFY_UNREACHABLE"));
            out.insert(
                "hint".into(),
                json!(
                    "拿不到 ComfyUI 的节点定义（/object_info），**不敢**自动插入节点。\
                     摘除部分已经做了，结果见 detachedGraph；要插节点请先 comfy_get_status \
                     确认 ComfyUI 在跑，或者让用户自己在 ComfyUI 里加。"
                ),
            );
            out.insert("detachedGraph".into(), Value::Object(detached.graph));
            out.insert("removed".into(), json!(detached.removed));
            out.insert("warnings".into(), json!(detached.warnings));
            let value = Value::Object(out);
            return Ok(ToolOutcome::new(value.to_string(), value));
        }
    }

    let result = apply(&graph, &remove_from, &keep, &injections, object_info.as_ref());
    let runnable = result.runnable();
    let hint = if runnable {
        "改好的图在 `graph`（**内联**的，不是库里的 promptId）。\
         要真跑它：comfy_submit(workflowPath=…) 不行（那要文件），\
         正确做法是把 `graph` 写成一份 .json 文件（write_file，允许写 comfyui 目录）\
         再 comfy_submit(workflowPath=那个路径) —— 或者把改动说清楚，让用户在 ComfyUI 里改。\
         **如实告诉用户你摘掉了什么、插了什么**（那是他的工作流）。"
    } else {
        "**这份图现在还不能跑**：见 unresolvedInputs（悬空的线）与 openInputs（空着的插口）。\
         照 openInputs 用 comfy_submit(connections=…) 补线，或者问用户；\
         **不要**带着缺口硬提交。"
    };
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.insert("runnable".into(), json!(runnable));
    out.insert("graph".into(), Value::Object(result.graph));
    out.insert("removed".into(), json!(result.removed));
    out.insert("rewired".into(), json!(result.rewired));
    out.insert("injected".into(), json!(result.injected));
    out.insert("unresolvedInputs".into(), json!(result.unresolved));
    out.insert("openInputs".into(), json!(result.open_inputs));
    out.insert("outputsAlive".into(), json!(result.outputs_alive));
    out.insert("outputsTotal".into(), json!(result.outputs_total));
    if !result.warnings.is_empty() {
        out.insert("warnings".into(), json!(result.warnings));
    }
    out.insert("hint".into(), json!(hint));
    let value = Value::Object(out);
    Ok(ToolOutcome::new(value.to_string(), value))
}

// ---------------------------------------------------------------------------
//  测试（`WorkflowPruneTest.kt` 的断言逐条移植）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
