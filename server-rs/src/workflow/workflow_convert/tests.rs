//! `workflow_convert` 的单元测试（主体代码在 [`workflow_convert`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

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
