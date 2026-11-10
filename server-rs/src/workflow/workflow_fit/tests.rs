//! `workflow_fit` 的单元测试（主体代码在 [`workflow_fit`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::comfy::comfy_models::LocalModel;

/// 一个 API 图节点的形状（`inputs` 里给什么就是什么）。
fn node(class_type: &str, inputs: Value) -> Value {
    json!({ "class_type": class_type, "inputs": inputs })
}

fn graph(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    let mut m = Map::new();
    for (id, n) in pairs {
        m.insert(id.to_string(), n);
    }
    m
}

// --- 实际情形：参考图 ---------------------------------------------------

#[test]
fn 用户没给图时工作流里的_load_image_必须报成缺图() {
    let g = graph(vec![
        (
            "1",
            node(
                "CheckpointLoaderSimple",
                json!({"ckpt_name": "sdxl.safetensors"}),
            ),
        ),
        // 绑着上次捕获时的那个 jpg —— 模型改不了它，也不该假装能用
        (
            "89",
            node(
                "LoadImage",
                json!({"image": "4789521234.jpg", "upload": "image"}),
            ),
        ),
        ("3", node("KSampler", json!({"seed": 42, "steps": 20}))),
        (
            "9",
            node(
                "SaveImage",
                json!({"images": ["3", 0], "filename_prefix": "ComfyUI"}),
            ),
        ),
    ]);

    let report = scan(&g, false);

    assert!(report.has_external_asset_gap(), "有图片缺口时必须报出来");
    let img = report
        .missing
        .iter()
        .find(|m| m.input_name == "image")
        .unwrap();
    assert_eq!(img.node_id, "89");
    assert_eq!(img.class_type, "LoadImage");
    assert_eq!(img.kind, InputKind::Image);
    assert!(img.hint.contains("comfy_use_attachment"), "{}", img.hint);
    assert!(
        img.hint.contains("不要硬提交"),
        "要挡住「带着缺口提交」这条路：{}",
        img.hint
    );
    // SaveImage 的 images 是"结果去哪儿"，不是"等谁给输入"
    assert!(report.missing.iter().all(|m| m.class_type != "SaveImage"));
}

#[test]
fn 本次带了图附件时图片位置记为可由附件解决_而不是要用户补() {
    let g = graph(vec![
        ("89", node("LoadImage", json!({"image": "old.jpg"}))),
        ("3", node("KSampler", json!({"seed": 1}))),
    ]);

    let with_image = scan(&g, true);
    assert!(
        !with_image.has_external_asset_gap(),
        "用户刚发了图：不该再去要一张"
    );
    assert!(with_image
        .missing
        .iter()
        .all(|m| m.kind != InputKind::Image));
    assert_eq!(
        with_image.supplied_by_attachment,
        vec!["89.image（本次附件可以投放）"]
    );
    assert!(
        with_image.render().contains("不必问用户"),
        "{}",
        with_image.render()
    );

    // 反面对照：没带图就必须报（否则这条用例证明不了什么）
    assert!(scan(&g, false).has_external_asset_gap());
}

// --- 不能过头：已经填好的不许报 ------------------------------------------

#[test]
fn 参数都有值的工作流报告为空_直接说可以提交() {
    let g = graph(vec![
        (
            "1",
            node(
                "CheckpointLoaderSimple",
                json!({"ckpt_name": "sdxl.safetensors"}),
            ),
        ),
        (
            "3",
            node("KSampler", json!({"seed": 42, "steps": 20, "cfg": 7})),
        ),
        (
            "9",
            node(
                "SaveImage",
                json!({"images": ["3", 0], "filename_prefix": "out"}),
            ),
        ),
    ]);

    let report = scan(&g, false);
    assert!(
        report.is_empty(),
        "没有缺口的图不该报任何东西：{:?}",
        report.missing
    );
    assert!(!report.has_external_asset_gap());
    assert!(
        report.render().contains("可以直接提交"),
        "{}",
        report.render()
    );
}

#[test]
fn 虚拟节点_note_与_reroute_不进清单() {
    let g = graph(vec![
        ("1", node("Note", json!({"text": "这里是说明"}))),
        ("2", node("Reroute", json!({"input": null}))),
        ("3", node("KSampler", json!({"seed": 5}))),
    ]);
    assert!(scan(&g, false).is_empty());
}

#[test]
fn 空连线型输入报成_connection_并明确不要猜着接线() {
    let g = graph(vec![
        ("5", node("IPAdapterApply", json!({"ipadapter": ["7", 0]}))),
        ("7", node("IPAdapterUnifiedLoader", json!({"preset": null}))),
        ("3", node("KSampler", json!({"seed": 1}))),
    ]);

    let report = scan(&g, false);
    let preset = report.missing.iter().find(|m| m.node_id == "7").unwrap();
    assert_eq!(
        preset.kind,
        InputKind::Parameter,
        "控件型没值 = 覆盖参数或问用户"
    );
    let wire = report
        .missing
        .iter()
        .find(|m| m.input_name == "ipadapter")
        .unwrap();
    assert_eq!(wire.kind, InputKind::Connection);
    assert!(wire.hint.contains("不要猜着接一根线"), "{}", wire.hint);
}

#[test]
fn 视频类输入单独标_video_并说明本机没有投放通道() {
    let g = graph(vec![(
        "12",
        node("VHS_LoadVideo", json!({"video": "old.mp4"})),
    )]);
    let report = scan(&g, false);
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].kind, InputKind::Video);
    assert!(
        report.missing[0].hint.contains("投放视频"),
        "{}",
        report.missing[0].hint
    );
}

#[test]
fn 只有控件缺口时_needs_external_asset_为假() {
    let g = graph(vec![("7", node("SomeCustomNode", json!({"prompt": null})))]);
    let report = scan(&g, false);
    assert!(!report.missing.is_empty());
    assert!(
        !report.has_external_asset_gap(),
        "没有图/视频缺口时不该把它说成「提交必然失败」"
    );
}

// --- apiGraphOf：三种来源共用同一条判据 ----------------------------------

#[test]
fn api_graph_of_认_api_图_不认界面格式与空对象() {
    let api = json!({"1": {"class_type": "KSampler"}});
    assert!(api_graph_of(Some(&api)).is_some());

    // 界面格式（nodes / links）**不是** API 图：得先 comfy_load_workflow 读进库
    let ui = json!({"nodes": [], "links": []});
    assert!(
        api_graph_of(Some(&ui)).is_none(),
        "界面格式不能被当成 API 图硬扫"
    );

    assert!(api_graph_of(Some(&json!({}))).is_none(), "空对象不是图");
    assert!(api_graph_of(None).is_none());

    // 模型粘贴时经常带上引号：字符串形态的 JSON 也认（Kotlin `contentOrNull` 那条路）
    let as_text = Value::String(api.to_string());
    assert_eq!(api_graph_of(Some(&as_text)).map(|g| g.len()), Some(1));
    assert!(api_graph_of(Some(&Value::String("{不是JSON".to_string()))).is_none());
}

#[test]
fn 扫描报告_render_结论先行_把图类缺口点出来() {
    let g = graph(vec![("89", node("LoadImage", json!({"image": "old.jpg"})))]);
    let text = scan(&g, false).render();
    assert!(text.contains("等着外部输入"), "{text}");
    assert!(
        text.contains("图 / 视频类的缺口"),
        "结论要先说清「硬提交会失败」：{text}"
    );
    assert!(text.contains("89.image"), "{text}");
}

#[test]
fn 可空键照写_kotlin_的取舍_而不是多写一个_null() {
    let g = graph(vec![("7", node("SomeCustomNode", json!({"prompt": null})))]);
    let json = scan(&g, false).to_json();
    let item = &json["missing"][0];
    // `title` / `currentValue` 都没有 → Kotlin 那边是**省掉键**（不是写 null）
    assert!(item.get("title").is_none(), "{item}");
    assert!(item.get("currentValue").is_none(), "{item}");
    assert_eq!(item["ref"], "7.prompt");
    assert_eq!(item["kind"], "value");
    // 空字符串当前值**要写出来**（那是"有值但是空的"这个事实）
    let g2 = graph(vec![("8", node("SomeCustomNode", json!({"prompt": "  "})))]);
    assert_eq!(
        scan(&g2, false).to_json()["missing"][0]["currentValue"].as_str(),
        Some("空字符串")
    );
}

// --- 配对（只报不改）----------------------------------------------------

/// 一份"本机有这些 checkpoint"的清单（体积按序号递增，与 Kotlin 的夹具一致）。
fn inventory_with(names: &[&str]) -> Inventory {
    let models: Vec<LocalModel> = names
        .iter()
        .enumerate()
        .map(|(i, n)| LocalModel {
            name: (*n).to_string(),
            folder: "checkpoints".to_string(),
            path: Some(format!("D:\\x\\{n}")),
            bytes: Some((i as u64 + 1) * 1024 * 1024 * 1024),
            visible_to_instance: true,
        })
        .collect();
    Inventory {
        source: Source::Scan,
        models,
        models_dir: None,
        truncated: false,
        disk_only: Vec::new(),
        merged: false,
    }
}

fn ckpt_graph(name: &str) -> Map<String, Value> {
    graph(vec![(
        "1",
        node("CheckpointLoaderSimple", json!({ "ckpt_name": name })),
    )])
}

#[test]
fn 本机有同名的报_exact_且不用改任何键() {
    let report = fit(
        &ckpt_graph("sd_xl_base_1.0.safetensors"),
        &inventory_with(&["sd_xl_base_1.0.safetensors"]),
        None,
        None,
    );
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].status, FitStatus::Exact);
    assert!(report.items[0].safe_to_auto_apply());
    assert!(
        report.suggested_overrides().is_empty(),
        "同名就没什么要改的"
    );
    assert!(report.render().contains("不用改"), "{}", report.render());
    assert!(!report.needs_action());
}

#[test]
fn 本机没有同名的报_substitute_并把候选与体积列出来_但绝不自动替换() {
    let report = fit(
        &ckpt_graph("krea2_turbo_int8_v2.safetensors"),
        &inventory_with(&["krea2_turbo_int8.safetensors"]),
        None,
        None,
    );
    let item = &report.items[0];
    assert_eq!(item.status, FitStatus::Substitute);
    assert!(!item.safe_to_auto_apply(), "只有同名才允许不问用户就换");
    assert!(
        report.suggested_overrides().is_empty(),
        "替代品不进 suggestedOverrides"
    );
    let text = report.render();
    assert!(text.contains("krea2_turbo_int8.safetensors"), "{text}");
    assert!(
        text.contains("GB"),
        "体积要报出来（昨天真实的门槛就是 13.49GB）：{text}"
    );
    assert!(text.contains("先跟用户确认"), "{text}");
}

#[test]
fn 同名但写法不同时给出可直接贴的覆盖键() {
    // 归一化之后同名（只差大小写与分隔符）→ EXACT 且要改键
    let report = fit(
        &ckpt_graph("Krea2-Turbo-Int8.safetensors"),
        &inventory_with(&["krea2_turbo_int8.safetensors"]),
        None,
        None,
    );
    assert_eq!(report.items[0].status, FitStatus::Exact);
    assert_eq!(
        report.suggested_overrides(),
        vec![(
            "1.ckpt_name".to_string(),
            "krea2_turbo_int8.safetensors".to_string()
        )]
    );
    assert_eq!(
        report.items[0].suggested_override().as_deref(),
        Some("krea2_turbo_int8.safetensors")
    );
    let text = report.render();
    assert!(text.contains("要改的键"), "{text}");
    assert!(
        text.contains("\"1.ckpt_name\": \"krea2_turbo_int8.safetensors\""),
        "{text}"
    );
    assert!(text.contains("写法不同"), "{text}");
    // JSON 里也要有同一份（模型优先读结构化那一半）
    assert_eq!(
        report.to_json()["suggestedOverrides"]["1.ckpt_name"],
        "krea2_turbo_int8.safetensors"
    );
}

#[test]
fn 本机什么都没有时报_missing_且明说不要编一个文件名() {
    let report = fit(
        &ckpt_graph("krea2TurboOfficialComfy_krea2TurboInt8.safetensors"),
        &inventory_with(&["sd15.safetensors", "flux1-dev.safetensors"]),
        None,
        None,
    );
    assert_eq!(report.items[0].status, FitStatus::Missing);
    assert!(report.all_missing());
    let text = report.render();
    assert!(text.contains("不要编一个文件名"), "{text}");
    assert!(
        text.contains("civitai_search"),
        "缺模型要指向「去搜」这条路：{text}"
    );
    assert!(
        text.contains("ask_user_question"),
        "选择权要交给用户：{text}"
    );
}

#[test]
fn 缺节点与缺模型分开报_不混成一件事() {
    let g = graph(vec![
        (
            "1",
            node(
                "CheckpointLoaderSimple",
                json!({"ckpt_name": "sd_xl_base_1.0.safetensors"}),
            ),
        ),
        ("9", node("SomeMissingNode", json!({}))),
    ]);
    let known: HashSet<String> = ["CheckpointLoaderSimple", "KSampler"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let report = fit(
        &g,
        &inventory_with(&["sd_xl_base_1.0.safetensors"]),
        Some(&known),
        None,
    );
    assert_eq!(report.missing_nodes, vec!["SomeMissingNode"]);
    assert_eq!(report.items[0].status, FitStatus::Exact, "模型齐了就是齐了");
    let text = report.render();
    assert!(text.contains("SomeMissingNode"), "{text}");
    assert!(text.contains("节点类"), "要说清缺的是节点不是模型：{text}");
    assert_eq!(report.to_json()["missingNodes"][0], "SomeMissingNode");
}

#[test]
fn 没有点名任何模型时如实说没有_而不是硬凑几条() {
    let g = graph(vec![("3", node("KSampler", json!({"steps": 20})))]);
    let report = fit(
        &g,
        &inventory_with(&["sd_xl_base_1.0.safetensors"]),
        None,
        None,
    );
    assert!(report.items.is_empty());
    assert!(!report.needs_action());
    assert!(report.render().contains("没有点名"), "{}", report.render());
    // 缺节点时那句话要跟着变（"没点名模型"与"缺节点"是两件事）
    let known: HashSet<String> = HashSet::new();
    let with_missing = fit(&g, &inventory_with(&[]), Some(&known), None);
    assert!(with_missing.render_hint().contains("comfy_check_inputs"));
}

#[test]
fn 缺模型时给出该搜什么_装到哪_以及本机沾边但不够像的那几个() {
    let report = fit(
        &ckpt_graph("krea2TurboOfficialComfy_krea2TurboInt8.safetensors"),
        &inventory_with(&["sd15.safetensors", "sd_xl_base_1.0.safetensors"]),
        None,
        None,
    );
    assert_eq!(report.items[0].status, FitStatus::Missing);
    let json = report.to_json();
    let proposal = &json["models"][0]["installProposal"];
    // 直接读字段，别用"整串包含"来断言 —— 这条用例要钉的正是**具体是哪个词**
    assert_eq!(proposal["lookupQuery"], "krea2 turbo int8", "{proposal}");
    assert_eq!(proposal["installInto"], "checkpoints", "{proposal}");
    assert!(
        proposal["note"].as_str().unwrap().contains("体积"),
        "{proposal}"
    );
    let text = report.render();
    assert!(
        text.contains("krea2 turbo int8"),
        "要把建议的查询词写出来：{text}"
    );
    assert!(text.contains("体积"), "提问选项里要带体积：{text}");
}

#[test]
fn 沾边的要报出来_只共用一个通用词的不算替代品() {
    // 报错名称比不报更差：看到"本机有个 sd15.safetensors"会误认为那就是目标模型，
    // 而它跟 krea2TurboInt8 毫无关系。所以"沾边"要有下限（分数 + 至少一个 ≥3 字的公共词）。
    let mut inv = inventory_with(&["sd15.safetensors", "xl_base_1.0.safetensors"]);
    for m in inv.models.iter_mut() {
        m.bytes = None;
        m.path = None;
    }
    assert!(
        inv.near_misses_for("krea2TurboInt8.safetensors", 3)
            .is_empty(),
        "只共用一个通用词的不该被报成替代品"
    );

    // 真的沾边的（词根对得上、只是不够像）要报出来
    let mut inv2 = inventory_with(&["krea_upscale_helper.safetensors"]);
    for m in inv2.models.iter_mut() {
        m.bytes = None;
        m.path = None;
    }
    let near: Vec<String> = inv2
        .near_misses_for("krea2TurboInt8.safetensors", 3)
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(near, vec!["krea_upscale_helper.safetensors"]);
}

// --- 并集那一档：盘上有、实例看不到 --------------------------------------

#[test]
fn 盘上有同名但实例加载不了时报_on_disk_only_并明说不要去下载() {
    let path =
        "D:\\Comfy-Desktop\\ComfyUI-Shared\\models\\diffusion_models\\anima-base-v1.0.safetensors";
    let model = LocalModel {
        name: "anima-base-v1.0.safetensors".to_string(),
        folder: "diffusion_models".to_string(),
        path: Some(path.to_string()),
        bytes: Some(4_182_218_328),
        visible_to_instance: false,
    };
    let inv = Inventory {
        source: Source::ObjectInfo,
        models: vec![model.clone()],
        models_dir: None,
        truncated: false,
        disk_only: vec![model],
        merged: true,
    };
    let g = graph(vec![(
        "79",
        node(
            "UNETLoader",
            json!({ "unet_name": "anima-base-v1.0.safetensors" }),
        ),
    )]);

    let report = fit(&g, &inv, None, None);
    let item = &report.items[0];
    assert_eq!(item.status, FitStatus::OnDiskOnly);
    assert!(!item.safe_to_auto_apply(), "实例加载不了，绝不许自动替换");
    assert!(
        report.suggested_overrides().is_empty(),
        "改了也是文件不存在，不许进 overrides"
    );

    let text = report.render();
    assert!(text.contains("不要下载"), "必须挡住'去下载'这条路：{text}");
    assert!(
        text.contains("重启"),
        "要给出正确动作（重启 / 修挂载）：{text}"
    );
    assert!(text.contains(path), "要说清文件到底在哪：{text}");

    let json = report.to_json();
    assert_eq!(json["onDiskOnly"], 1);
    assert_eq!(json["missing"], 0, "不许算成 missing（那会把用户引去下载）");
    assert_eq!(json["invisibleToInstance"], 1);
    assert!(
        report.to_json()["hint"]
            .as_str()
            .unwrap()
            .contains("不要去下载"),
        "{json}"
    );
}

#[test]
fn 候选全都看不到时_明说别拿它们去_overrides() {
    let model = LocalModel {
        name: "krea2_turbo_int8_v2.safetensors".to_string(),
        folder: "checkpoints".to_string(),
        path: None,
        bytes: None,
        visible_to_instance: false,
    };
    let inv = Inventory {
        source: Source::ObjectInfo,
        models: vec![model.clone()],
        models_dir: None,
        truncated: false,
        disk_only: vec![model],
        merged: true,
    };

    let report = fit(
        &ckpt_graph("krea2_turbo_int8.safetensors"),
        &inv,
        None,
        None,
    );
    assert_eq!(report.items[0].status, FitStatus::Substitute);
    assert_eq!(
        report.to_json()["models"][0]["candidates"][0]["visibleToInstance"],
        false,
        "候选必须带上'实例能不能加载'这一位"
    );
    let text = report.render();
    assert!(text.contains("看不到"), "全看不到时要说清：{text}");
    assert!(text.contains("重启"), "正确动作是重启，不是换模型：{text}");
    // 报告头那一句（并集里有实例看不到的）也要说
    assert!(text.contains("只出现在硬盘上"), "{text}");
}
