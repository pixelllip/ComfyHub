//! `workflow_frontend` 的单元测试（主体代码在 [`workflow_frontend`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn object(text: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(text).unwrap().as_object().unwrap().clone()
}

/// 广播型判据与显示型判据都借 `workflow_audit`（**没有第三份表**）。
#[test]
fn 判据来自_workflow_audit_那一份() {
    assert!(is_broadcast("Anything Everywhere3?"));
    assert!(is_broadcast("anything everywhere (3)"));
    assert!(is_display("PreviewImage"));
    // 中文类名不命中任何一条模式（不存在"顺手认一下中文"这种事）
    assert!(!is_display("显示节点"));
    assert!(!is_broadcast("我的显示节点"));
}

/// `properties` 里那几个键名不同版本不一样，逐个认；一个都没有 = 类型未声明 = 不猜。
#[test]
fn 广播设定从_properties_里逐个认键名() {
    let spec = broadcast_spec(&object(
        r#"{"properties":{"Input Type":"MODEL","Input Name":"model"}}"#,
    ));
    assert_eq!(spec.type_name.as_deref(), Some("MODEL"));
    assert_eq!(spec.name_filter.as_deref(), Some("model"));
    // `*` / `anything` = 不限类型 → 当成"未声明"（Kotlin 同）
    assert!(broadcast_spec(&object(r#"{"properties":{"Input Type":"*"}}"#)).type_name.is_none());
    assert!(
        broadcast_spec(&object(r#"{"properties":{"Input Type":"anything"}}"#))
            .type_name
            .is_none()
    );
    // 键名一种都不认识 → 类型未声明
    assert!(broadcast_spec(&object(r#"{"properties":{}}"#)).type_name.is_none());
}

#[test]
fn 名字过滤优先于类型且大小写不敏感() {
    let spec = broadcast_spec(&object(r#"{"properties":{"Prefix":"POSIT"}}"#));
    let target = OpenInput {
        node_id: "5".into(),
        name: "positive".into(),
        types: vec!["CONDITIONING".into()],
        required: true,
    };
    assert!(matches(&spec, &target, None), "名字过滤命中时不再要求类型对得上");
    let other = OpenInput { name: "seed".into(), ..target };
    assert!(!matches(&spec, &other, None));
}

/// 新版本 cg-use-everywhere（`properties` 里没有类型）：**每个接了线的插口各一路**，
/// 类型写在插口自己的 `type` 上。
///
/// 判据来源是用户 2026-10-17 存下来的 `krea2 t2i workflow.json`：那个
/// `Anything Everywhere` 长着 `anything[CLIP]` / `anything15[MODEL]` / `anything16[VAE]`
/// 三个接了线的插口，而 `properties` 里只有 `title_regex` 这类正则键。
/// 只认老版本的表现是"一处广播都填不上"，且**不报错**。
#[test]
fn 新版广播节点按每个插口的类型分路() {
    let node = object(
        r#"{"id":36,"type":"Anything Everywhere","properties":{"title_regex":null,"next_input_index":17},
            "inputs":[{"name":"anything","type":"CLIP","link":45},
                      {"name":"anything15","type":"MODEL","link":57},
                      {"name":"anything16","type":"VAE","link":58},
                      {"name":"anything17","type":"*","link":null}]}"#,
    );
    let mut links: LinkTable = LinkTable::new();
    links.insert(45, ("32".to_string(), 1));
    links.insert(57, ("32".to_string(), 0));
    links.insert(58, ("29".to_string(), 0));
    let graph = object(r#"{"32":{"class_type":"Lora Loader Stack (rgthree)","inputs":{}},
                            "29":{"class_type":"VAELoader","inputs":{}}}"#);
    let spec = broadcast_spec(&node);
    assert!(spec.type_name.is_none() && spec.name_filter.is_none(), "老版本的类型声明一个字都没有");
    let routes = broadcast_routes(&node, &links, &spec, &graph);
    assert_eq!(routes.len(), 3, "三路（没接线的那个 `*` 不算）");
    let types: Vec<Option<&str>> = routes.iter().map(|route| route.type_name.as_deref()).collect();
    assert_eq!(types, vec![Some("CLIP"), Some("MODEL"), Some("VAE")]);
    // 每路只填同类型的空输入
    let clip = OpenInput { node_id: "34".into(), name: "clip".into(), types: vec!["CLIP".into()], required: true };
    let model = OpenInput { node_id: "41".into(), name: "model".into(), types: vec!["MODEL".into()], required: true };
    assert!(matches(&spec, &clip, Some("CLIP")));
    assert!(!matches(&spec, &clip, Some("MODEL")));
    assert!(matches(&spec, &model, Some("MODEL")));
    // 类型说不清（`*`）时一路都不灌 —— 宁可少做，也不要把错的线接上去
    assert!(!matches(&spec, &model, Some("*")));
}

/// 空输入清单：控件型不算缺口，`forceInput` 是插口（要算）。
#[test]
fn 空输入清单只算插口型() {
    let object_info = object(
        r#"{"N":{"input":{"required":{
             "model":["MODEL"], "seed":["INT",{"control_after_generate":true}],
             "text":["STRING",{"forceInput":true}]}}}}"#,
    );
    let graph = object(r#"{"1":{"class_type":"N","inputs":{"model":["2",0]}}}"#);
    let open = open_inputs_of(&object_info, &graph);
    assert_eq!(open, vec!["1.text (STRING)".to_string()], "{open:?}");
}

/// 补输出节点：接"按类型找得到产出 IMAGE 的那个节点"，接不上就不补（不猜）。
#[test]
fn 没有输出节点时才补_saveimage() {
    let object_info = object(
        r#"{"VAEDecode":{"input":{"required":{"samples":["LATENT"]}},"output":["IMAGE"]},
             "KSampler":{"input":{"required":{"model":["MODEL"]}},"output":["LATENT"]}}"#,
    );
    let graph = object(
        r#"{"1":{"class_type":"KSampler","inputs":{"model":["9",0]}},
             "2":{"class_type":"VAEDecode","inputs":{"samples":["1",0]}}}"#,
    );
    let (emulated, report) = emulate(&[], &object_info, &graph, &LinkTable::new());
    assert_eq!(emulated["3"]["class_type"], json!("SaveImage"));
    assert_eq!(emulated["3"]["inputs"]["images"], json!([2, 0]));
    assert_eq!(report.injected.len(), 1, "{:?}", report.injected);

    // 已经有 SaveImage 就不再补（绝不覆盖用户自己的工作流）
    let with_save = object(
        r#"{"1":{"class_type":"VAEDecode","inputs":{}},
             "7":{"class_type":"SaveImage","inputs":{"images":["1",0]}}}"#,
    );
    let (kept, report) = emulate(&[], &object_info, &with_save, &LinkTable::new());
    assert_eq!(kept.len(), 2);
    assert!(report.injected.is_empty());
    // 认不出"谁会产出 IMAGE"时**不补**（宁可少补一个，也不把错的线接上去）
    let unknown = object(r#"{"1":{"class_type":"KSampler","inputs":{}}}"#);
    let (kept, report) = emulate(&[], &object_info, &unknown, &LinkTable::new());
    assert_eq!(kept.len(), 1);
    assert!(report.injected.is_empty());
}

#[test]
fn 新节点号跳过已被占用的() {
    let used: Vec<String> = ["10", "12", "11"].iter().map(|s| s.to_string()).collect();
    assert_eq!(fresh_node_id(used.iter()), "13");
    let used: Vec<String> = ["1", "abc"].iter().map(|s| s.to_string()).collect();
    assert_eq!(fresh_node_id(used.iter()), "2");
}
