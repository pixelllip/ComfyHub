//! `workflow_blueprint` 的用例。
//!
//! 夹具全部是**手写的小图**（不是从库中导出的真实文件）：这里固定的是**判据**
//! （哪条线该被报成什么），不是某一份文件的形状。真文件的活体抽查在
//! `tests/spot_check.rs` 与 `docs/workflow-blueprint-design.md` 的验收记录里。

use serde_json::{json, Map, Value};

use super::*;

/// 一份"界面格式"最小图：`SaveImage ← VAEDecode ← KSampler ← CheckpointLoaderSimple`。
///
/// 刻意带一个 `Note`（纯前端节点）与一个 `Reroute`：蓝图必须把它们**画出来**而不是丢掉。
fn ui_fixture(note_title: &str) -> Value {
    json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "CheckpointLoaderSimple", "pos": [0, 0], "size": [270, 98],
             "mode": 0, "inputs": [],
             "outputs": [{"name": "MODEL", "type": "MODEL", "links": [11]},
                         {"name": "CLIP", "type": "CLIP", "links": []},
                         {"name": "VAE", "type": "VAE", "links": []}],
             "widgets_values": ["model.safetensors"]},
            {"id": 2, "type": "KSampler", "pos": [400, 0], "size": [300, 260], "mode": 0,
             "inputs": [{"name": "model", "type": "MODEL", "link": 11},
                        {"name": "positive", "type": "CONDITIONING", "link": null},
                        {"name": "latent_image", "type": "LATENT", "link": null},
                        {"name": "seed", "type": "INT", "link": null, "widget": {"name": "seed"}}],
             "outputs": [{"name": "LATENT", "type": "LATENT", "links": [12]}],
             "widgets_values": [12345, "randomize"]},
            {"id": 3, "type": "VAEDecode", "pos": [800, 0], "size": [140, 46], "mode": 0,
             "inputs": [{"name": "samples", "type": "LATENT", "link": 12},
                        {"name": "vae", "type": "VAE", "link": null}],
             "outputs": [{"name": "IMAGE", "type": "IMAGE", "links": [13]}],
             "widgets_values": []},
            {"id": 4, "type": "SaveImage", "pos": [1000, 0], "size": [270, 270], "mode": 0,
             "inputs": [{"name": "images", "type": "IMAGE", "link": 13},
                        {"name": "filename_prefix", "type": "STRING", "link": null, "widget": {"name": "filename_prefix"}}],
             "outputs": [{"name": "images", "type": "IMAGE", "links": []}],
             "widgets_values": ["test"]},
            {"id": 5, "type": "Note", "pos": [-400, 0], "size": [300, 200], "mode": 0,
             "inputs": [], "outputs": [], "widgets_values": [note_title]},
            {"id": 6, "type": "Reroute", "pos": [700, 200], "size": [60, 30], "mode": 4,
             "inputs": [{"name": "", "type": "*", "link": null}],
             "outputs": [{"name": "", "type": "*", "links": []}], "widgets_values": []}
        ],
        "links": [
            [11, 1, 0, 2, 0, "MODEL"],
            [12, 2, 0, 3, 0, "LATENT"],
            [13, 3, 0, 4, 0, "IMAGE"]
        ],
        "groups": [{"title": "主链", "bounding": [0, 0, 1200, 400], "color": "#3f789e"}]
    })
}

/// 一份最小 `/object_info`：只声明夹具里用到的那几个类。
fn object_info() -> Map<String, Value> {
    let mut info: Map<String, Value> = Map::new();
    let mut add = |class: &str, required: Value, output: Vec<&str>, output_node: bool| {
        info.insert(
            class.to_string(),
            json!({
                "input": {"required": required},
                "output": output,
                "output_name": output,
                "output_node": output_node,
            }),
        );
    };
    add(
        "CheckpointLoaderSimple",
        json!({"ckpt_name": [[], {}]}),
        vec!["MODEL", "CLIP", "VAE"],
        false,
    );
    add(
        "KSampler",
        json!({
            "model": ["MODEL", {}],
            "seed": ["INT", {"control_after_generate": true}],
            "positive": ["CONDITIONING", {}],
            "latent_image": ["LATENT", {}]
        }),
        vec!["LATENT"],
        false,
    );
    add(
        "VAEDecode",
        json!({"samples": ["LATENT", {}], "vae": ["VAE", {}]}),
        vec!["IMAGE"],
        false,
    );
    add(
        "SaveImage",
        json!({"images": ["IMAGE", {}], "filename_prefix": ["STRING", {}]}),
        vec!["IMAGE"],
        true,
    );
    info
}

fn build(raw: &Value, info: Option<&Map<String, Value>>) -> Blueprint {
    super::build(raw, info, true)
}

fn codes(blueprint: &Blueprint) -> Vec<&'static str> {
    blueprint.issues.iter().map(|issue| issue.code).collect()
}

#[test]
fn 界面格式能画出真实坐标与分组_前端节点一个都不丢() {
    let raw = ui_fixture("这是备注");
    let blueprint = build(&raw, Some(&object_info()));

    assert_eq!(blueprint.format, "ui");
    assert_eq!(
        blueprint.nodes.len(),
        6,
        "Note / Reroute 必须还在（蓝图不是转换，不许摘节点）"
    );
    assert_eq!(blueprint.edges.len(), 3);
    assert_eq!(blueprint.groups.len(), 1);
    assert_eq!(blueprint.groups[0].title, "主链");

    let note = blueprint
        .nodes
        .iter()
        .find(|node| node.class == "Note")
        .unwrap();
    assert_eq!(note.kind, "note");
    assert!(note.frontend_only, "`Note` 服务端没有这个类 → 标成纯前端");
    assert_eq!(note.pos, Some((-400.0, 0.0)));

    let reroute = blueprint
        .nodes
        .iter()
        .find(|node| node.class == "Reroute")
        .unwrap();
    assert_eq!(reroute.kind, "reroute");
    assert_eq!(reroute.mode, 4, "旁路状态要带出来（界面据此灰显）");

    // 控件值贴回名字上（这里走的是 `inputs[].widget.name`）
    let sampler = blueprint
        .nodes
        .iter()
        .find(|node| node.class == "KSampler")
        .unwrap();
    assert_eq!(sampler.widgets[0].name.as_deref(), Some("seed"));
    assert_eq!(sampler.widgets[0].value, json!(12345));
    assert_eq!(sampler.widgets[1].value, json!("randomize"));
}

#[test]
fn 控件值有名的那一份优先_位置化的那份仍然兜底() {
    // 新版前端会同时写 `widgets_values`（位置）与 `widgets_values_named`（名字）：
    // 名字以 named 为准（位置那份在自定义节点上常对不齐）
    let raw = json!({
        "version": 0.4,
        "nodes": [{
            "id": 7, "type": "KSampler", "pos": [0, 0], "size": [300, 260], "mode": 0,
            "inputs": [
                {"name": "seed", "type": "INT", "link": null, "widget": {"name": "seed"}},
                {"name": "steps", "type": "INT", "link": null, "widget": {"name": "steps"}}
            ],
            "outputs": [],
            "widgets_values": [20, 8],
            "widgets_values_named": {"steps": 8, "seed": 20}
        }],
        "links": []
    });
    let blueprint = build(&raw, None);
    let node = &blueprint.nodes[0];
    assert_eq!(node.widgets[0].name.as_deref(), Some("seed"));
    assert_eq!(node.widgets[0].value, json!(20));
    assert_eq!(node.widgets[1].name.as_deref(), Some("steps"));
    assert_eq!(node.widgets[1].value, json!(8));
}

#[test]
fn 没有任何名字来源时如实说不知道_不猜参数名() {
    // 旧存档（没有 `widgets_values_named`）+ ComfyUI 不在线（没有 `/object_info`）：
    // 那时**只有位置**，不许猜名字（研究文档里那条硬约束）
    let raw = json!({
        "version": 0.4,
        "nodes": [{
            "id": 9, "type": "SomeCustomNode", "pos": [0, 0], "size": [100, 50], "mode": 0,
            "inputs": [{"name": "a", "type": "INT", "link": null}],
            "outputs": [], "widgets_values": [7, 8]
        }],
        "links": []
    });
    let blueprint = build(&raw, None);
    let node = &blueprint.nodes[0];
    assert_eq!(node.widgets.len(), 2);
    assert!(node.widgets.iter().all(|widget| widget.name.is_none()));
    assert_eq!(node.widgets[0].value, json!(7));
}

#[test]
fn 两种links写法都要认() {
    let object_links = json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "A", "pos": [0, 0], "size": [10, 10], "mode": 0, "inputs": [],
             "outputs": [{"name": "out", "type": "IMAGE", "links": [31]}], "widgets_values": []},
            {"id": 2, "type": "B", "pos": [100, 0], "size": [10, 10], "mode": 0,
             "inputs": [{"name": "in", "type": "IMAGE", "link": 31}], "outputs": [], "widgets_values": []}
        ],
        "links": [{"id": 31, "origin_id": 1, "origin_slot": 0, "target_id": 2, "target_slot": 0, "type": "IMAGE"}]
    });
    let blueprint = build(&object_links, None);
    assert_eq!(blueprint.edges.len(), 1);
    assert_eq!(blueprint.edges[0].id, Some(31));
    assert_eq!(blueprint.edges[0].from, ("1".to_string(), 0));
    assert_eq!(blueprint.edges[0].to, ("2".to_string(), 0));
    assert_eq!(blueprint.edges[0].type_name.as_deref(), Some("IMAGE"));
}

#[test]
fn 没有object_info时空插口只提醒_不判必填也不判类型() {
    let raw = ui_fixture("备注");
    let blueprint = build(&raw, None);
    assert!(!blueprint.object_info);
    let issue = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "INPUT_UNCONNECTED")
        .unwrap();
    assert_eq!(issue.severity, SEV_WARN, "判不出必填 → 只提醒");
    // 空着的是哪几个插口要说清楚（界面据此画标记）
    assert!(issue.slots.contains(&"2.positive".to_string()));
    assert!(issue.slots.contains(&"2.latent_image".to_string()));
    assert!(issue.slots.contains(&"3.vae".to_string()));
    // `seed` 是控件不是插口：不许报进来
    assert!(!issue.slots.iter().any(|slot| slot.ends_with(".seed")));
    // 拿不到节点定义就不许冒"本机没有这个类"
    assert!(!codes(&blueprint).contains(&"UNKNOWN_CLASS"));
    assert!(!codes(&blueprint).contains(&"NO_OUTPUT_NODE"));
}

/// 拿到 `/object_info` 之后，空插口由**转换那一遍**来说（判得更准：分得清必填与可选，
/// 也算得上控件填充与穿透），界面里不该出现两条说同一件事的结论。
#[test]
fn 有object_info时空插口归转换那一遍说_且只算必填() {
    let raw = ui_fixture("备注");
    let blueprint = build(&raw, Some(&object_info()));
    assert!(
        !codes(&blueprint).contains(&"INPUT_UNCONNECTED"),
        "转换能判得更准，这里不再重复报：{:?}",
        codes(&blueprint)
    );
    let gap = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "CONVERT_GAP")
        .unwrap();
    assert!(gap.slots.contains(&"2.positive".to_string()));
    assert!(gap.slots.contains(&"3.vae".to_string()));
    assert!(!gap.slots.iter().any(|slot| slot.ends_with(".seed")));
}

#[test]
fn 接错类型要点到具体插口() {
    let mut raw = ui_fixture("备注");
    // 把 SaveImage 的 images 改成"要 CONDITIONING"是不对的 —— 更真实的错法是把
    // 一个 MODEL 接到 IMAGE 上：这里把 VAEDecode 的 samples 声明改成 IMAGE
    raw["nodes"][2]["inputs"][0]["type"] = json!("IMAGE");
    let blueprint = build(&raw, Some(&object_info()));
    let issue = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "TYPE_MISMATCH")
        .unwrap();
    assert_eq!(issue.severity, SEV_ERROR);
    assert_eq!(issue.slots, vec!["3.samples".to_string()]);
    assert_eq!(issue.links, vec![12]);
    assert_eq!(issue.nodes, vec!["2".to_string(), "3".to_string()]);
    assert!(issue.detail.contains("Return type mismatch"));
}

#[test]
fn 环要被抓住() {
    let raw = json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "A", "pos": [0, 0], "size": [10, 10], "mode": 0,
             "inputs": [{"name": "in", "type": "X", "link": 2}],
             "outputs": [{"name": "out", "type": "X", "links": [1]}], "widgets_values": []},
            {"id": 2, "type": "B", "pos": [100, 0], "size": [10, 10], "mode": 0,
             "inputs": [{"name": "in", "type": "X", "link": 1}],
             "outputs": [{"name": "out", "type": "X", "links": [2]}], "widgets_values": []}
        ],
        "links": [[1, 1, 0, 2, 0, "X"], [2, 2, 0, 1, 0, "X"]]
    });
    let blueprint = build(&raw, None);
    let issue = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "CYCLE")
        .unwrap();
    assert_eq!(issue.severity, SEV_ERROR);
    assert!(issue.nodes.contains(&"1".to_string()) && issue.nodes.contains(&"2".to_string()));
}

#[test]
fn 悬挂引用与重复id都要报() {
    let raw = json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "A", "pos": [0, 0], "size": [10, 10], "mode": 0, "inputs": [],
             "outputs": [{"name": "out", "type": "X", "links": [5]}], "widgets_values": []},
            {"id": 1, "type": "A2", "pos": [0, 0], "size": [10, 10], "mode": 0, "inputs": [],
             "outputs": [], "widgets_values": []}
        ],
        "links": [[5, 1, 0, 99, 0, "X"]]
    });
    let blueprint = build(&raw, None);
    assert!(codes(&blueprint).contains(&"DUPLICATE_ID"));
    assert!(codes(&blueprint).contains(&"LINK_DANGLING"));
}

#[test]
fn 转换留下的缺口会被交叉核对出来_并指出开模拟用户行为就能补上() {
    // 这张图缺 `vae`（必填），但图中有一个广播节点 —— 即"开启 ##模拟前端行为## 即可修复"。
    let raw = json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "CheckpointLoaderSimple", "pos": [0, 0], "size": [270, 98], "mode": 0,
             "inputs": [], "outputs": [{"name": "VAE", "type": "VAE", "links": [41]}],
             "widgets_values": ["model.safetensors"]},
            {"id": 2, "type": "Anything Everywhere", "pos": [300, 0], "size": [180, 100], "mode": 0,
             "inputs": [{"name": "anything", "type": "VAE", "link": 41}], "outputs": [], "widgets_values": []},
            {"id": 3, "type": "VAEDecode", "pos": [600, 0], "size": [140, 46], "mode": 0,
             "inputs": [{"name": "samples", "type": "LATENT", "link": 42},
                        {"name": "vae", "type": "VAE", "link": null}],
             "outputs": [{"name": "IMAGE", "type": "IMAGE", "links": [43]}], "widgets_values": []},
            {"id": 4, "type": "SaveImage", "pos": [900, 0], "size": [270, 270], "mode": 0,
             "inputs": [{"name": "images", "type": "IMAGE", "link": 43}], "outputs": [], "widgets_values": []},
            {"id": 5, "type": "EmptyLatentImage", "pos": [300, 200], "size": [270, 100], "mode": 0,
             "inputs": [], "outputs": [{"name": "LATENT", "type": "LATENT", "links": [42]}], "widgets_values": [512, 512, 1]}
        ],
        "links": [[41, 1, 0, 2, 0, "VAE"], [42, 5, 0, 3, 0, "LATENT"], [43, 3, 0, 4, 0, "IMAGE"]]
    });
    let mut info = object_info();
    // 广播节点在本机被注册成"零输出"的桩（cg-use-everywhere 就是这么注册的）
    info.insert(
        "Anything Everywhere".to_string(),
        json!({"input": {"required": {}, "optional": {"anything": ["*", {}]}}, "output": [], "output_node": false}),
    );
    info.insert("EmptyLatentImage".to_string(), json!({"input": {"required": {"width": ["INT", {}], "height": ["INT", {}], "batch_size": ["INT", {}]}}, "output": ["LATENT"], "output_node": false}));
    let blueprint = build(&raw, Some(&info));
    assert!(codes(&blueprint).contains(&"BROADCAST_STUB"));
    let gap = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "CONVERT_GAP")
        .unwrap();
    assert_eq!(
        gap.severity, SEV_WARN,
        "开 ##模拟用户行为## 之后缺口就没了 → 只提醒"
    );
    assert!(gap.fix.as_deref().unwrap().contains("emulateFrontend"));
    assert!(
        gap.slots.contains(&"3.vae".to_string()),
        "空的是 `vae` 那一路：{:?}",
        gap.slots
    );
    // 缺口可通过 ##模拟前端行为## 补上 ⇒ 经提交链可以执行（`clean` 为真），
    // 但**必须**在报告里说清楚"直接交上去会缺输入"
    assert!(blueprint.clean());
    let summary = blueprint.summary().unwrap();
    assert!(summary.contains("0 个接线错误"), "{summary}");
    assert!(summary.contains("警告"), "{summary}");
}

#[test]
fn 本机没装的节点类要报错() {
    let raw = json!({
        "version": 0.4,
        "nodes": [{"id": 1, "type": "NotInstalledNode", "pos": [0, 0], "size": [10, 10], "mode": 0,
                   "inputs": [], "outputs": [], "widgets_values": []}],
        "links": []
    });
    let blueprint = build(&raw, Some(&object_info()));
    let issue = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "UNKNOWN_CLASS")
        .unwrap();
    assert_eq!(issue.severity, SEV_ERROR);
    assert_eq!(issue.nodes, vec!["1".to_string()]);
}

#[test]
fn api格式_边方向要对_位置为空_并如实说布局是排出来的() {
    let raw = json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}},
        "2": {"class_type": "KSampler",
              "inputs": {"model": ["1", 0], "seed": 1, "steps": 8, "cfg": 1.0,
                         "sampler_name": "euler", "scheduler": "simple",
                         "positive": ["3", 0], "negative": ["4", 0], "latent_image": ["5", 0], "denoise": 1.0},
              "_meta": {"title": "采样器"}},
        "3": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": "hi"}},
        "4": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": ""}},
        "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 896, "height": 1344, "batch_size": 1}}
    });
    let blueprint = build(&raw, None);
    assert_eq!(blueprint.format, "api");
    assert!(blueprint.nodes.iter().all(|node| node.pos.is_none()));
    assert_eq!(
        blueprint.nodes[1].title, "采样器",
        "`_meta.title` 优先，缺了才回落到 class_type"
    );
    // 边方向：`["1", 0]` 是"我的 model 来自节点 1 的第 0 个输出"
    let edge = blueprint
        .edges
        .iter()
        .find(|edge| edge.to.0 == "2" && edge.from.0 == "1")
        .unwrap();
    assert!(edge.implicit);
    assert!(blueprint
        .warnings
        .iter()
        .any(|text| text.contains("自动排")));
}

#[test]
fn api格式_没有object_info时不判必填与类型() {
    let raw = json!({
        "1": {"class_type": "KSampler", "inputs": {"model": ["2", 0]}},
        "2": {"class_type": "Unknown", "inputs": {}}
    });
    let blueprint = build(&raw, None);
    assert!(!codes(&blueprint).contains(&"INPUT_UNCONNECTED"));
    assert!(!codes(&blueprint).contains(&"TYPE_MISMATCH"));
    assert!(!codes(&blueprint).contains(&"UNKNOWN_CLASS"));
}

#[test]
fn api格式_类型不匹配与必填缺失() {
    let raw = json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}},
        "2": {"class_type": "VAEDecode", "inputs": {"samples": ["1", 1], "vae": ["1", 2]}},
        "3": {"class_type": "SaveImage", "inputs": {"images": ["2", 0], "filename_prefix": "x"}}
    });
    let blueprint = build(&raw, Some(&object_info()));
    // `1.1` 是 CLIP，接到 VAEDecode.samples（要 LATENT）上 → 类型不匹配
    let issue = blueprint
        .issues
        .iter()
        .find(|issue| issue.code == "TYPE_MISMATCH")
        .unwrap();
    assert_eq!(issue.slots, vec!["2.samples".to_string()]);
    assert_eq!(issue.nodes, vec!["1".to_string(), "2".to_string()]);
    assert!(!blueprint.clean());
}

#[test]
fn 认不出的_json不许炸() {
    for raw in [json!("not json"), json!({}), json!({"a": 1}), json!([])] {
        let blueprint = build(&raw, None);
        assert_eq!(blueprint.format, "unknown");
        assert!(blueprint.nodes.is_empty());
        assert!(!blueprint.warnings.is_empty());
    }
}

#[test]
fn 组节点实例画成subgraph框并如实说明() {
    let raw = json!({
        "version": 0.4,
        "nodes": [
            {"id": 1, "type": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "pos": [0, 0],
             "size": [200, 100], "mode": 0, "inputs": [], "outputs": [], "widgets_values": []}
        ],
        "links": [],
        "definitions": {"subgraphs": [{"id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "nodes": [], "links": []}]}
    });
    let blueprint = build(&raw, None);
    assert_eq!(blueprint.nodes[0].kind, "subgraph");
    assert!(blueprint
        .warnings
        .iter()
        .any(|text| text.contains("组节点")));
}

#[test]
fn 报告文本与_json都带上了错误条数() {
    let raw = ui_fixture("备注");
    let blueprint = build(&raw, Some(&object_info()));
    let json = blueprint.to_json();
    assert_eq!(json["format"], "ui");
    assert_eq!(json["counts"]["nodes"], 6);
    assert_eq!(json["counts"]["errors"], blueprint.count(SEV_ERROR));
    assert!(!json["issues"].as_array().unwrap().is_empty());
    let text = blueprint.render();
    assert!(text.contains("接线体检"));
    assert!(blueprint.summary().is_some());
}
