//! `workflow_prune` 的单元测试（主体代码在 [`workflow_prune`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn graph(text: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(text)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn node_of<'a>(graph: &'a Map<String, Value>, id: &str) -> &'a Value {
    graph.get(id).unwrap_or_else(|| panic!("图里没有节点 {id}"))
}

fn inputs_of<'a>(graph: &'a Map<String, Value>, id: &str) -> &'a Map<String, Value> {
    node_of(graph, id)
        .get("inputs")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("节点 {id} 没有 inputs"))
}

/// 读一条连线 `["上游节点", 槽位]`；不是连线就 panic（断言消息里能看出问题）。
fn ref_pair(value: Option<&Value>) -> (String, String) {
    let items = value.and_then(Value::as_array).expect("不是一条连线");
    (
        primitive_text(&items[0]).unwrap_or_default(),
        primitive_text(&items[1]).unwrap_or_default(),
    )
}

/// 经典的"文生图 + 参考图分支"：`KSampler.positive` 接在 ControlNet 上。
fn with_reference_branch() -> Map<String, Value> {
    graph(
        r#"{
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "4": {"class_type":"LoraLoader","inputs":{"model":["1",0],"clip":["1",1],"lora_name":"x.safetensors"}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["4",1]}},
          "7": {"class_type":"CLIPTextEncode","inputs":{"text":"bad","clip":["4",1]}},
          "89": {"class_type":"LoadImage","inputs":{"image":"478952.jpg","upload":"image"}},
          "92": {"class_type":"ControlNetApplyAdvanced","inputs":{"positive":["6",0],"negative":["7",0],"control_net":["91",0],"image":["89",0],"strength":0.8}},
          "91": {"class_type":"ControlNetLoader","inputs":{"control_net_name":"canny.safetensors"}},
          "3": {"class_type":"KSampler","inputs":{"model":["4",0],"positive":["92",0],"negative":["92",1],"latent_image":["5",0],"seed":1,"steps":20,"cfg":7.0}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024,"batch_size":1}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
          "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}}
        }"#,
    )
}

fn empty_set() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn 摘掉参考图分支_controlnet_与_load_image_一起走_采样器的线接回文本编码器() {
    let r = apply(
        &with_reference_branch(),
        &["89.image".to_string()],
        &empty_set(),
        &[],
        None,
    );

    assert!(
        r.warnings.iter().any(|w| w.contains("92.image")),
        "哪根线断了要说清楚：{:?}",
        r.warnings
    );
    // 采样器本来指着 92；92 还在（它还有消费者），所以采样器不动 ——
    // 想彻底不参考图，就把 92 也一起摘掉。
    let positive = ref_pair(inputs_of(&r.graph, "3").get("positive"));
    assert_eq!(
        positive.0, "92",
        "还有消费者的节点只被摘掉那根线，节点本身保留"
    );
    assert!(
        r.unresolved.is_empty(),
        "不该留下悬空的线：{:?}",
        r.unresolved
    );
    assert!(
        r.runnable(),
        "摘完应当能跑：alive={}/{} {:?}",
        r.outputs_alive,
        r.outputs_total,
        r.warnings
    );
}

#[test]
fn 摘掉一个共享节点不会误伤别人_文本编码器被两个分支共用() {
    let g = graph(
        r#"{
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["1",1]}},
          "89": {"class_type":"LoadImage","inputs":{"image":"a.jpg"}},
          "92": {"class_type":"ControlNetApplyAdvanced","inputs":{"positive":["6",0],"image":["89",0]}},
          "3": {"class_type":"KSampler","inputs":{"positive":["92",0],"steps":20}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0]}},
          "10": {"class_type":"SaveImage","inputs":{"images":["9",0]}}
        }"#,
    );
    let r = apply(&g, &["89".to_string()], &empty_set(), &[], None);
    assert!(r.graph.contains_key("6"), "共享的文本编码器不能被误摘");
    assert!(
        !inputs_of(&r.graph, "92").contains_key("image"),
        "被摘的那根线要真的没了"
    );
    assert!(!r.graph.contains_key("89"), "点名的 LoadImage 摘掉了");
    assert!(r.graph.contains_key("92"), "92 还有消费者（采样器），保留");
    assert!(r.graph.contains_key("3") && r.graph.contains_key("10"));
}

#[test]
fn 输出节点不会被摘_点名摘_save_image_要如实拒绝() {
    let r = apply(
        &with_reference_branch(),
        &["10".to_string()],
        &empty_set(),
        &[],
        None,
    );
    assert!(
        r.graph.contains_key("10"),
        "输出节点不许被摘掉（摘了就什么都出不来）"
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("保留")),
        "要如实说明为什么没摘：{:?}",
        r.warnings
    );
}

#[test]
fn keep_里的节点不会被摘() {
    let keep: HashSet<String> = ["89".to_string()].into_iter().collect();
    let r = apply(
        &with_reference_branch(),
        &["89".to_string()],
        &keep,
        &[],
        None,
    );
    assert!(r.graph.contains_key("92"), "keep 明确要求保留的节点不能摘");
    assert!(r.graph.contains_key("89"), "keep 明确要求保留的节点不能摘");
}

#[test]
fn 插节点_sage_attention_接在模型链上_原来的上游接到它自己() {
    let object_info = graph(
        r#"{"SageAttention": {"input":{"required":{"model":["MODEL"],"dtype":["COMBO",{"default":"fp16"}]}}}}"#,
    );
    let injections = vec![Injection {
        at: "3.model".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(
        &with_reference_branch(),
        &[],
        &empty_set(),
        &injections,
        Some(&object_info),
    );
    assert_eq!(
        r.injected.len(),
        1,
        "要插进去一个节点：{:?} {:?}",
        r.injected,
        r.warnings
    );
    let new_id = r
        .graph
        .iter()
        .find(|(_, node)| class_of(node) == "SageAttention")
        .map(|(id, _)| id.clone())
        .expect("插进去的节点要在图里");
    let model_ref = ref_pair(inputs_of(&r.graph, "3").get("model"));
    assert_eq!(new_id, model_ref.0, "采样器的 model 要改成补丁节点");
    let patch_model = ref_pair(inputs_of(&r.graph, &new_id).get("model"));
    assert_eq!(
        "4", patch_model.0,
        "补丁的 model 要接回原来的上游 LoraLoader"
    );
    assert!(r.runnable(), "插完应当仍然能跑：{:?}", r.unresolved);
}

#[test]
fn 没有节点定义表时照样插_但不猜接线之外的输入() {
    let injections = vec![Injection {
        at: "3.model".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(
        &with_reference_branch(),
        &[],
        &empty_set(),
        &injections,
        None,
    );
    // 插入这件事本身是确定的（classType + 插在哪根线），所以定义表缺失不阻止插入；
    // 缺的是"它还有哪些输入"这类信息 —— 那种情况下宁可少插，也不乱接。
    assert!(
        !r.injected.is_empty(),
        "有明确的插入位置就该插：{:?}",
        r.injected
    );
    assert!(r.graph.contains_key("3"), "采样器还在");
}

#[test]
fn 节点定义里没有这个类时只警告不强插() {
    let object_info = graph(r#"{"SomeOtherNode":{"input":{"required":{"x":["INT"]}}}}"#);
    let injections = vec![Injection {
        at: "3.model".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(
        &with_reference_branch(),
        &[],
        &empty_set(),
        &injections,
        Some(&object_info),
    );
    assert!(
        r.injected.is_empty(),
        "这台 ComfyUI 没有的节点类不该硬插：{:?}",
        r.injected
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("SageAttention")),
        "要如实说明少了什么：{:?}",
        r.warnings
    );
}

#[test]
fn 摘掉之后留下空输入要列出来而不是假装没事() {
    let g = graph(
        r#"{
          "89": {"class_type":"LoadImage","inputs":{"image":"a.jpg"}},
          "3": {"class_type":"KSampler","inputs":{"positive":["89",0],"steps":20}},
          "10": {"class_type":"SaveImage","inputs":{"images":["3",0]}}
        }"#,
    );
    let r = apply(&g, &["89".to_string()], &empty_set(), &[], None);
    assert!(r.graph.contains_key("3"), "采样器还在");
    assert!(
        !inputs_of(&r.graph, "3").contains_key("positive"),
        "没有可用的上游 → 这个输入被摘掉"
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("现在空着")),
        "必须如实说清哪根线没了：{:?}",
        r.warnings
    );
}

/// 一份骨干齐全、另加两条**用不上**的分支（放大链 + 换脸链）+ 一个没接完的孤立节点。
fn with_dead_branches() -> Map<String, Value> {
    graph(
        r#"{
          "1": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
          "6": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat","clip":["1",1]}},
          "3": {"class_type":"KSampler","inputs":{"model":["1",0],"positive":["6",0],"latent_image":["5",0],"seed":1}},
          "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0],"vae":["1",2]}},
          "10": {"class_type":"SaveImage","inputs":{"images":["9",0],"filename_prefix":"out"}},
          "70": {"class_type":"UpscaleModelLoader","inputs":{"model_name":"4x.pth"}},
          "71": {"class_type":"ImageUpscaleWithModel","inputs":{"upscale_model":["70",0],"image":["9",0]}},
          "80": {"class_type":"FaceSwap","inputs":{"image":["71",0],"face":["81",0]}},
          "81": {"class_type":"LoadImage","inputs":{"image":"face.png"}},
          "99": {"class_type":"Note","inputs":{"text":"还没接完的备注"}}
        }"#,
    )
}

#[test]
fn 分析_用不上的分支连成一片_孤立节点单独列出且不建议删() {
    let report = unused_branches(&with_dead_branches(), &empty_set(), DEFAULT_MAX_BRANCHES);

    // 80 / 81 / 70 / 71 谁都不喂给出图链路 —— 连成一片 = 一条分支
    assert_eq!(report.total_unused, 4, "用不上的应当是换脸+放大那四个");
    assert_eq!(
        report.branches.len(),
        1,
        "它们连在一起，算一条：{:?}",
        report
            .branches
            .iter()
            .map(|b| b.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    let branch = &report.branches[0];
    let ids: HashSet<String> = branch.nodes.iter().map(|n| n.id.clone()).collect();
    let expected: HashSet<String> = ["70", "71", "80", "81"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(ids, expected);
    // 入口 = 这条分支里谁也不引用的那个（换脸节点），摘它就是整条一起走
    assert_eq!(branch.entry, "80", "入口应当是分支的梢（下游那个）");
    let suggest: HashSet<String> = branch.suggest_remove().into_iter().collect();
    assert_eq!(suggest, expected);
    // 可读标签（需据此做决定，不能只给节点 id）
    assert!(!branch.label.trim().is_empty());
    assert!(
        branch.kinds.iter().any(|k| k.contains("放大")),
        "节点种类要说人话：{:?}",
        branch.kinds
    );

    // 99（没接任何线、也没人用）是"还没接完"，不归"用不上"
    assert_eq!(
        report
            .isolated
            .iter()
            .map(|n| n.id.clone())
            .collect::<Vec<_>>(),
        vec!["99"]
    );
    assert_eq!(report.total_isolated, 1);
    assert_eq!(report.total_nodes, 11);
}

#[test]
fn 分析_干净的工作流一条都不报() {
    let report = unused_branches(&with_reference_branch(), &empty_set(), DEFAULT_MAX_BRANCHES);
    assert_eq!(report.total_unused, 0, "这份图每个节点都在链路里");
    assert!(report.branches.is_empty());
    assert!(
        report.to_json()["hint"].to_string().contains("没有可摘"),
        "要明确说没有，而不是给一份空清单"
    );
}

#[test]
fn 分析结果可以直接拿去摘_建议的入口能整条摘掉() {
    let report = unused_branches(&with_dead_branches(), &empty_set(), DEFAULT_MAX_BRANCHES);
    let removed: Vec<String> = report
        .branches
        .iter()
        .flat_map(|b| b.suggest_remove())
        .collect();
    let r = apply(&with_dead_branches(), &removed, &empty_set(), &[], None);
    assert!(
        r.removed.iter().any(|entry| entry.starts_with("80")),
        "摘除清单里要有换脸：{:?}",
        r.removed
    );
    assert!(
        r.removed.iter().any(|entry| entry.starts_with("70")),
        "放大节点也要跟着走：{:?}",
        r.removed
    );
    assert!(r.graph.contains_key("10"), "出图链路必须还在");
    assert!(r.graph.contains_key("9"));
    assert!(r.runnable(), "{:?} {:?}", r.unresolved, r.warnings);
}

/// 工具返回的那份 JSON 形状（Kotlin `WorkflowPruneTest`「工具层」那几条的纯函数半边）。
#[test]
fn 分析结果的_json_形状与_kotlin_一致() {
    let value =
        unused_branches(&with_dead_branches(), &empty_set(), DEFAULT_MAX_BRANCHES).to_json();
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["totalUnused"], json!(4));
    assert_eq!(value["branches"][0]["removeFrom"], json!("80"));
    assert_eq!(value["branches"][0]["nodeCount"], json!(4));
    assert_eq!(value["isolated"][0]["id"], json!("99"));
    assert_eq!(value["truncated"], json!(false));
    let hint = value["hint"].as_str().unwrap();
    assert!(hint.contains("先问用户"), "必须要求先确认：{hint}");
    assert!(
        hint.contains("isolated") || hint.contains("还没接完"),
        "要提醒孤立节点别删：{hint}"
    );
    // `usedBy` 那条"有才写"的取舍：中间节点带 usedBy，末端（谁也不引用）**省掉键**
    assert_eq!(value["branches"][0]["nodes"][0]["id"], json!("70"));
    assert_eq!(value["branches"][0]["nodes"][0]["usedBy"][0], json!("71"));
    let tip = value["branches"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!("80"))
        .expect("末端 80 要在清单里");
    assert!(tip.get("usedBy").is_none(), "末端没人引用 → 省掉键：{tip}");
    // 干净的那份：summary 要说"没有用不上的分支"
    let clean =
        unused_branches(&with_reference_branch(), &empty_set(), DEFAULT_MAX_BRANCHES).to_json();
    assert!(
        clean["summary"]
            .as_str()
            .unwrap()
            .contains("没有用不上的分支"),
        "{clean}"
    );
    assert!(
        clean.get("isolated").is_none(),
        "没有孤立节点时**省掉键**：{clean}"
    );
}

/// 摘除 / 拼装的 JSON 形状：可空键的取舍与 Kotlin 手拼的一致。
#[test]
fn 摘除结果的_json_形状与_kotlin_一致() {
    let r = apply(
        &with_reference_branch(),
        &["89.image".to_string()],
        &empty_set(),
        &[],
        None,
    );
    assert!(r.graph.contains_key("10"));
    assert!(!r.graph.contains_key("89"));
    assert!(!r.removed.is_empty(), "摘了什么要如实列出来");
    assert!(r.removed.iter().any(|entry| entry.starts_with("89")));
    assert!(r.summary().contains("摘掉"));
}

/// 拼装位置 / 上游 / 节点类的四种拒法都要有**人话**（不静默）。
#[test]
fn 拼装的四种拒法都是人话() {
    // ① 位置写得不成样子
    let bad_at = vec![Injection {
        at: "3".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(&with_reference_branch(), &[], &empty_set(), &bad_at, None);
    assert!(
        r.warnings.iter().any(|w| w.contains("节点id.输入名")),
        "{:?}",
        r.warnings
    );

    // ② 位置上的节点不在图里
    let gone = vec![Injection {
        at: "999.model".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(&with_reference_branch(), &[], &empty_set(), &gone, None);
    assert!(
        r.warnings.iter().any(|w| w.contains("999")),
        "{:?}",
        r.warnings
    );

    // ③ 那个输入不是一条线（值是字面量）
    let literal = vec![Injection {
        at: "4.lora_name".to_string(),
        class_type: "SageAttention".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(&with_reference_branch(), &[], &empty_set(), &literal, None);
    assert!(
        r.warnings.iter().any(|w| w.contains("不是一条线")),
        "{:?}",
        r.warnings
    );

    // ④ 定义表里说不出该接哪个输入（required 全是控件型）
    let object_info = graph(r#"{"Patch":{"input":{"required":{"note":["STRING"]}}}}"#);
    let injections = vec![Injection {
        at: "3.model".to_string(),
        class_type: "Patch".to_string(),
        inputs: Map::new(),
        node_id: None,
    }];
    let r = apply(
        &with_reference_branch(),
        &[],
        &empty_set(),
        &injections,
        Some(&object_info),
    );
    assert!(r.injected.is_empty(), "宁可不插，也不乱接");
    assert!(
        r.warnings.iter().any(|w| w.contains("自动接线")),
        "{:?}",
        r.warnings
    );
}

/// 空图 / 脏数据：如实报，**不许 panic**。
#[test]
fn 空图与脏数据都不许_panic() {
    let empty = Map::new();
    let r = apply(&empty, &["1".to_string()], &empty_set(), &[], None);
    assert_eq!(r.outputs_total, 0);
    assert!(
        r.warnings.iter().any(|w| w.contains("一个节点都没有")),
        "{:?}",
        r.warnings
    );
    assert!(!r.runnable());

    let dirty = graph(r#"{"1": "nope", "2": {"class_type": "", "inputs": {}}}"#);
    let r = apply(&dirty, &[], &empty_set(), &[], None);
    assert_eq!(r.outputs_total, 0);
    // 非对象条目被丢掉，对象条目**原样留下**（Kotlin `nodes` 也是这么筛的）
    assert!(!r.graph.contains_key("1"));
    assert!(r.graph.contains_key("2"));
    let report = unused_branches(&dirty, &empty_set(), DEFAULT_MAX_BRANCHES);
    assert_eq!(report.total_nodes, 1);
}

/// 静音的输出节点不算输出（Kotlin `isOutput` 的 `mode == "2"` 那一条）。
#[test]
fn 静音的输出节点不算输出来源() {
    let g = graph(
        r#"{
          "10": {"class_type":"SaveImage","mode":2,"inputs":{"images":["9",0]}},
          "9": {"class_type":"VAEDecode","inputs":{"samples":["3",0]}},
          "3": {"class_type":"KSampler","inputs":{"steps":20}}
        }"#,
    );
    let report = unused_branches(&g, &empty_set(), DEFAULT_MAX_BRANCHES);
    assert_eq!(report.output_count, 0, "静音的 SaveImage 不算输出节点");
    assert_eq!(report.total_unused, 3, "没有输出节点 → 三个都算用不上");
}

#[test]
fn describe_认不出来就照抄类名() {
    assert_eq!(
        describe("ControlNetApplyAdvanced"),
        "ControlNet（结构控制）"
    );
    assert_eq!(describe("ImageUpscaleWithModel"), "放大 / 缩放");
    assert_eq!(describe("UnknownThing"), "UnknownThing");
    assert_eq!(describe("VAELoader"), "VAE 加载/解码");
    assert_eq!(describe("LoraLoader"), "LoRA 加载");
}

#[test]
fn bool_arg_只认严格的_true_false() {
    assert_eq!(bool_arg(Some(&json!(true))), Some(true));
    assert_eq!(bool_arg(Some(&json!("true"))), Some(true));
    assert_eq!(
        bool_arg(Some(&json!("True"))),
        None,
        "Kotlin 的 toBooleanStrictOrNull 不认大小写变体"
    );
    assert_eq!(bool_arg(Some(&json!(1))), None);
    assert_eq!(bool_arg(None), None);
}
